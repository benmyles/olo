//! `org.freedesktop.impl.portal.RemoteDesktop`: a program controls the
//! computer (Chrome Remote Desktop, a KVM such as Deskflow, a video call's
//! remote control, an agent), once the person has said yes. Its sessions
//! are ScreenCast's (`portal::Shared`): CreateSession makes one,
//! SelectDevices says what it wants to control (keyboard, pointer, touch
//! screen), ScreenCast's SelectSources may add screens to share, and
//! Start asks the person: the picker, saying it will control them too,
//! or, for control alone, the permission dialog. Never remembered: asked
//! every time.
//!
//! Its input comes one of two ways, as the portal says. The Notify calls
//! (an event each) become app_server's synthetic input over its IPC, an
//! absolute position on a stream put where that stream's screen or window
//! is. Or ConnectToEIS: app_server is told to take olo-portal's next libei
//! connection as the session's, with the devices granted and no others
//! (`Request::RemoteControl`); the portal connects to app_server's EIS
//! socket and hands the program that socket. Deskbar shows the session
//! while it lasts, with Stop; its end closes the libei connection
//! (`Request::RemoteControlEnd`).

use std::collections::HashMap;

use olo_protocol::{Request, SyntheticEvent as S, TouchPhase, remote};
use olo_sync::Mutex;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue};

use crate::portal::{Results, Shared, Shown, value};

const OK: u32 = 0;
const FAILED: u32 = 2;
/// Every device Olo can be controlled with.
const ALL: u32 = remote::KEYBOARD | remote::POINTER | remote::TOUCHSCREEN;

/// What `devices` are, said ("keyboard and pointer").
pub fn devices_said(devices: u32) -> String {
    olo_protocol::sharing::and_list(&devices_named(devices))
}

/// What `devices` are, each named.
pub fn devices_named(devices: u32) -> Vec<String> {
    [(remote::KEYBOARD, "keyboard"), (remote::POINTER, "pointer"), (remote::TOUCHSCREEN, "touch screen")].into_iter().filter(|(bit, _)| devices & bit != 0).map(|(_, name)| name.to_owned()).collect()
}

/// A session went (closed by its program, by the person, or its stream
/// was lost): if it controlled the computer, app_server closes its libei
/// connection and lets go of keys it left down.
pub fn ended(session: &str, s: &crate::portal::Session) {
    if s.controls == 0 {
        return;
    }
    let session = session.to_owned();
    std::thread::spawn(move || {
        if let Ok(mut c) = olo_protocol::Client::connect()
            && let Err(e) = c.request(&Request::RemoteControlEnd { session })
        {
            tracing::warn!("portal: app_server wasn't told remote control ended: {e}");
        }
    });
}

pub struct RemoteDesktop {
    shared: Shared,
    /// app_server's IPC, kept for the Notify calls (one event each).
    ipc: Mutex<Option<olo_protocol::Client>>,
}

impl RemoteDesktop {
    pub fn new(shared: Shared) -> RemoteDesktop {
        RemoteDesktop { shared, ipc: Mutex::new(None) }
    }

    /// The session's granted devices, if it may send `needs` by Notify
    /// calls now (started, granted them, no libei connection).
    fn may(&self, session: &OwnedObjectPath, needs: u32) -> Option<u32> {
        let sessions = self.shared.sessions.lock().unwrap();
        let s = sessions.get(session)?;
        (s.remote && !s.eis && s.controls & needs != 0).then_some(s.controls)
    }

    /// What stream `node` of the session shows.
    fn stream(&self, session: &OwnedObjectPath, node: u32) -> Option<Shown> {
        let sessions = self.shared.sessions.lock().unwrap();
        sessions.get(session)?.nodes.iter().find(|(n, _)| *n == node).map(|(_, shown)| *shown)
    }

    /// Input for app_server (connected again if it restarted).
    fn send(&self, events: Vec<S>) {
        let mut ipc = self.ipc.lock().unwrap();
        for _ in 0..2 {
            if ipc.is_none() {
                *ipc = olo_protocol::Client::connect().ok();
            }
            let Some(c) = ipc.as_mut() else { return };
            match c.request(&Request::Synthetic { events: events.clone() }) {
                Ok(_) => return,
                Err(_) => *ipc = None,
            }
        }
    }

    /// An absolute position on a stream, as input for where it shows.
    fn on_stream(&self, session: &OwnedObjectPath, node: u32, x: f64, y: f64) -> Option<(Option<u64>, f64, f64)> {
        match self.stream(session, node)? {
            Shown::Screen(sx, sy) => Some((None, sx as f64 + x, sy as f64 + y)),
            Shown::Window(id) => Some((Some(id), x, y)),
            Shown::Elsewhere => None,
        }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.RemoteDesktop")]
impl RemoteDesktop {
    async fn create_session(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, Results) {
        self.shared.create(session_handle, app_id, true, server).await
    }

    async fn select_devices(&self, _handle: OwnedObjectPath, session_handle: OwnedObjectPath, _app_id: String, options: HashMap<String, OwnedValue>) -> (u32, Results) {
        let mut sessions = self.shared.sessions.lock().unwrap();
        let Some(s) = sessions.get_mut(&session_handle).filter(|s| s.remote) else { return (FAILED, Results::new()) };
        // (All of them, unless it says.)
        s.devices = match options.get("types").and_then(|v| u32::try_from(v).ok()).unwrap_or(ALL) & ALL {
            0 => ALL,
            d => d,
        };
        tracing::info!(app = s.app, devices = s.devices, "portal: devices selected");
        (OK, Results::new())
    }

    async fn start(
        &self,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        _app_id: String,
        parent_window: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, Results) {
        let devices = {
            let sessions = self.shared.sessions.lock().unwrap();
            match sessions.get(&session_handle).filter(|s| s.remote) {
                Some(s) if s.devices != 0 => s.devices,
                Some(_) => ALL,
                None => return (FAILED, Results::new()),
            }
        };
        match self.shared.start(handle, session_handle, parent_window, devices, server).await {
            Ok(mut results) => {
                results.insert("devices".into(), value(devices));
                results.insert("clipboard_enabled".into(), value(false));
                (OK, results)
            }
            Err(response) => (response, Results::new()),
        }
    }

    fn notify_pointer_motion(&self, session_handle: OwnedObjectPath, _options: HashMap<String, OwnedValue>, dx: f64, dy: f64) {
        if self.may(&session_handle, remote::POINTER).is_some() {
            self.send(vec![S::MoveBy { dx, dy }]);
        }
    }

    fn notify_pointer_motion_absolute(&self, session_handle: OwnedObjectPath, _options: HashMap<String, OwnedValue>, stream: u32, x: f64, y: f64) {
        if self.may(&session_handle, remote::POINTER).is_none() {
            return;
        }
        match self.on_stream(&session_handle, stream, x, y) {
            Some((Some(window), x, y)) => self.send(vec![S::MoveOn { window, x, y }]),
            Some((None, x, y)) => self.send(vec![S::Move { x, y }]),
            None => {}
        }
    }

    fn notify_pointer_button(&self, session_handle: OwnedObjectPath, _options: HashMap<String, OwnedValue>, button: i32, state: u32) {
        if self.may(&session_handle, remote::POINTER).is_some() && button >= 0 {
            self.send(vec![S::Button { button: button as u32, pressed: state == 1 }]);
        }
    }

    fn notify_pointer_axis(&self, session_handle: OwnedObjectPath, options: HashMap<String, OwnedValue>, dx: f64, dy: f64) {
        if self.may(&session_handle, remote::POINTER).is_none() {
            return;
        }
        let finish = options.get("finish").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        let mut events = Vec::new();
        if dx != 0.0 || dy != 0.0 {
            events.push(S::Scroll { dx, dy, at_us: None });
        }
        // The fingers lifted (a glide may follow).
        if finish {
            events.push(S::Scroll { dx: 0.0, dy: 0.0, at_us: None });
        }
        if !events.is_empty() {
            self.send(events);
        }
    }

    fn notify_pointer_axis_discrete(&self, session_handle: OwnedObjectPath, _options: HashMap<String, OwnedValue>, axis: u32, steps: i32) {
        if self.may(&session_handle, remote::POINTER).is_none() || steps == 0 {
            return;
        }
        // 0: vertical, 1: horizontal.
        let (dx, dy) = if axis == 1 { (steps, 0) } else { (0, steps) };
        self.send(vec![S::Wheel { dx, dy }]);
    }

    fn notify_keyboard_keycode(&self, session_handle: OwnedObjectPath, _options: HashMap<String, OwnedValue>, keycode: i32, state: u32) {
        if self.may(&session_handle, remote::KEYBOARD).is_some() && keycode >= 0 {
            self.send(vec![S::Key { key: keycode as u32, pressed: state == 1 }]);
        }
    }

    fn notify_keyboard_keysym(&self, session_handle: OwnedObjectPath, _options: HashMap<String, OwnedValue>, keysym: i32, state: u32) {
        if self.may(&session_handle, remote::KEYBOARD).is_some() && keysym > 0 {
            self.send(vec![S::Keysym { keysym: keysym as u32, pressed: state == 1 }]);
        }
    }

    fn notify_touch_down(&self, session_handle: OwnedObjectPath, _options: HashMap<String, OwnedValue>, stream: u32, slot: u32, x: f64, y: f64) {
        self.touch(&session_handle, stream, slot, TouchPhase::Down, x, y);
    }

    fn notify_touch_motion(&self, session_handle: OwnedObjectPath, _options: HashMap<String, OwnedValue>, stream: u32, slot: u32, x: f64, y: f64) {
        self.touch(&session_handle, stream, slot, TouchPhase::Motion, x, y);
    }

    fn notify_touch_up(&self, session_handle: OwnedObjectPath, _options: HashMap<String, OwnedValue>, slot: u32) {
        if self.may(&session_handle, remote::TOUCHSCREEN).is_some() {
            self.send(vec![S::Touch { slot, phase: TouchPhase::Up, x: 0.0, y: 0.0, window: None }]);
        }
    }

    /// A libei connection for the session (its program sends input on
    /// it): app_server takes olo-portal's next connection as the
    /// session's, and the program gets the socket.
    #[zbus(name = "ConnectToEIS")]
    async fn connect_to_eis(&self, session_handle: OwnedObjectPath, _app_id: String, _options: HashMap<String, OwnedValue>) -> zbus::fdo::Result<OwnedFd> {
        let devices = {
            let mut sessions = self.shared.sessions.lock().unwrap();
            let Some(s) = sessions.get_mut(&session_handle).filter(|s| s.remote && s.controls != 0) else {
                return Err(zbus::fdo::Error::AccessDenied("The session isn't started, or controls nothing.".into()));
            };
            if s.eis {
                return Err(zbus::fdo::Error::Failed("The session has its connection already.".into()));
            }
            s.eis = true;
            s.controls
        };
        let session = session_handle.to_string();
        let connected = blocking::unblock(move || -> Result<std::os::unix::net::UnixStream, String> {
            // (One grant and connection at a time: app_server matches the
            // connection to the grant by who makes it, olo-portal.)
            static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
            let mut c = olo_protocol::Client::connect().map_err(|e| e.to_string())?;
            match c.request(&Request::RemoteControl { session, devices }) {
                Ok(olo_protocol::Reply::Ok) => {}
                Ok(olo_protocol::Reply::Error { message }) => return Err(message),
                Ok(other) => return Err(format!("{other:?}")),
                Err(e) => return Err(e.to_string()),
            }
            let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("no runtime directory")?;
            let path = olo_protocol::eis_socket(std::path::Path::new(&runtime), &olo_xdg::display_name());
            std::os::unix::net::UnixStream::connect(&path).map_err(|e| format!("{}: {e}", path.display()))
        })
        .await;
        match connected {
            Ok(stream) => {
                tracing::info!(session = session_handle.as_str(), devices, "portal: libei connection handed over");
                Ok(OwnedFd::from(std::os::fd::OwnedFd::from(stream)))
            }
            Err(e) => {
                tracing::warn!("portal: no libei connection: {e}");
                if let Some(s) = self.shared.sessions.lock().unwrap().get_mut(&session_handle) {
                    s.eis = false;
                }
                Err(zbus::fdo::Error::Failed(format!("Couldn't connect: {e}")))
            }
        }
    }

    #[zbus(property, name = "AvailableDeviceTypes")]
    fn available_device_types(&self) -> u32 {
        ALL
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

impl RemoteDesktop {
    fn touch(&self, session: &OwnedObjectPath, stream: u32, slot: u32, phase: TouchPhase, x: f64, y: f64) {
        if self.may(session, remote::TOUCHSCREEN).is_none() {
            return;
        }
        if let Some((window, x, y)) = self.on_stream(session, stream, x, y) {
            self.send(vec![S::Touch { slot, phase, x, y, window }]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_are_said() {
        assert_eq!(devices_said(remote::KEYBOARD | remote::POINTER), "keyboard and pointer");
        assert_eq!(devices_said(ALL), "keyboard, pointer and touch screen");
        assert_eq!(devices_said(remote::POINTER), "pointer");
    }
}
