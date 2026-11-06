//! Remote control over libei (the RemoteDesktop portal's ConnectToEIS):
//! a program the person has let control the computer (Chrome Remote
//! Desktop, a KVM such as Deskflow, an agent) sends input as a libei
//! sender. app_server listens on a socket of its own
//! (`olo_protocol::eis_socket`) that nothing announces (no
//! `LIBEI_SOCKET` for every program to find, as before): it takes a
//! connection only from olo-portal, and only once the portal has granted
//! one (`Request::RemoteControl`, after the person said yes in its
//! picker), with the devices granted and no others. The portal hands the
//! connected socket to the program. Its input becomes synthetic input
//! (`SyntheticEvent`), as an agent's does, on every backend. The session
//! ending (the program closing it, or Stop in Deskbar) closes its
//! connections (`Request::RemoteControlEnd`).

use std::{
    os::fd::AsFd,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use olo_protocol::{SyntheticEvent as S, TouchPhase, remote};
use smithay::{
    backend::{
        input::InputEvent,
        libei::{EiInput, EiInputConnection, EiInputEvent, EiInputSeat, EiRegion, ScrollEvent},
    },
    input::keyboard::XkbConfig,
    reexports::{
        calloop::{LoopHandle, PostAction, RegistrationToken},
        reis::{calloop::EisListenerSource, eis},
    },
    utils::Rectangle,
};
use tracing::{info, warn};

use crate::state::{AnvilState, Backend};

/// A grant not taken by then lapses.
const GRANT_LAPSES: Duration = Duration::from_secs(10);
/// A wheel's notch, in libei's 120ths.
const NOTCH: i32 = 120;

/// A connection the portal may make, for a session.
struct Grant {
    /// olo-portal's process (who asked, who must connect).
    pid: i32,
    session: String,
    devices: u32,
    at: Instant,
}

/// A program's connection.
struct Live {
    id: u64,
    session: String,
    devices: u32,
    token: RegistrationToken,
    /// Once it has connected: its seat (its absolute devices' regions
    /// follow the displays), and the connection (to close it from here).
    seat: Option<EiInputSeat>,
    connection: Option<eis::Connection>,
    /// Wheel turned in 120ths of a notch, not yet a whole notch.
    wheel: (i32, i32),
}

#[derive(Default)]
pub struct Remote {
    path: Option<PathBuf>,
    grants: Vec<Grant>,
    live: Vec<Live>,
    next: u64,
    /// Keys pressed by keysym with Shift added (`keysym`): Shift goes up
    /// with them.
    shifted: Vec<u32>,
}

impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remote").field("grants", &self.grants.len()).field("live", &self.live.len()).finish()
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        // (reis unlinks its socket as the listener drops; this is the
        // path's, in case it was never made.)
        if let Some(p) = &self.path {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The process at the other end of a socket.
fn peer_pid(fd: impl AsFd) -> Option<i32> {
    use std::os::fd::AsRawFd;
    // SAFETY: an all-zero ucred is valid, and getsockopt writes at most
    // `len` bytes into it.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let got = unsafe { libc::getsockopt(fd.as_fd().as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len) };
    (got == 0).then_some(cred.pid)
}

/// The libei socket for `display`, listened on: connections go to
/// `AnvilState::remote_accept`.
pub fn listen<B: Backend>(handle: &LoopHandle<'static, AnvilState<B>>, display: &str) -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let path = olo_protocol::eis_socket(Path::new(&runtime), display);
    // (Left by an app_server before this one on the display: only one
    // holds the display at a time.)
    let _ = std::fs::remove_file(&path);
    let listener = match eis::Listener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            warn!(path = %path.display(), "remote control: can't listen: {e}");
            return None;
        }
    };
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let inserted = handle.insert_source(EisListenerSource::new(listener), |context, _, state| {
        state.remote_accept(context);
        Ok(PostAction::Continue)
    });
    if let Err(e) = inserted {
        warn!("remote control: can't listen: {e}");
        return None;
    }
    Some(path)
}

/// Where an absolute device may point: each display, in the global
/// logical space (`mapping_id`: its connector, as the portal's screen
/// shares name it).
fn regions<B: Backend>(state: &AnvilState<B>) -> Vec<EiRegion> {
    state
        .space
        .outputs()
        .filter_map(|o| {
            let rect: Rectangle<i32, smithay::utils::Logical> = state.space.output_geometry(o)?;
            Some(EiRegion { rect, scale: o.current_scale().fractional_scale() as f32, mapping_id: Some(o.name()) })
        })
        .collect()
}

impl<B: Backend> AnvilState<B> {
    /// Listen for libei connections on this display (at start).
    pub fn remote_listen(&mut self) {
        let display = self.socket_name.clone().unwrap_or_else(olo_xdg::display_name);
        self.remote.path = listen(&self.handle, &display);
    }

    /// `Request::RemoteControl` from `pid` (olo-portal's, checked by
    /// the caller): its next connection is `session`'s.
    pub fn remote_grant(&mut self, pid: i32, session: String, devices: u32) {
        self.remote.grants.retain(|g| g.at.elapsed() < GRANT_LAPSES);
        info!(session, devices, "remote control granted");
        self.remote.grants.push(Grant { pid, session, devices, at: Instant::now() });
    }

    /// `Request::RemoteControlEnd`: the session's connections close.
    pub fn remote_end(&mut self, session: &str) {
        self.remote.grants.retain(|g| g.session != session);
        let (gone, kept): (Vec<Live>, Vec<Live>) = std::mem::take(&mut self.remote.live).into_iter().partition(|l| l.session == session);
        self.remote.live = kept;
        for l in gone {
            if let Some(c) = &l.connection {
                c.disconnected(0, eis::connection::DisconnectReason::Disconnected, Some("The remote control session ended."));
                if let Some(b) = smithay::reexports::reis::Interface::as_object(c).backend() {
                    let _ = b.flush();
                }
            }
            self.handle.remove(l.token);
            info!(session, "remote control connection closed");
        }
        if self.remote.live.is_empty() {
            self.remote_release_keys();
        }
    }

    /// A connection to the socket: taken if olo-portal granted it.
    fn remote_accept(&mut self, context: eis::Context) {
        let pid = peer_pid(&context);
        self.remote.grants.retain(|g| g.at.elapsed() < GRANT_LAPSES);
        let Some(i) = self.remote.grants.iter().position(|g| Some(g.pid) == pid) else {
            warn!(?pid, "remote control: a connection nobody granted, refused");
            return;
        };
        let grant = self.remote.grants.remove(i);
        let id = self.remote.next;
        self.remote.next += 1;
        let source = EiInput::new(context);
        let token = match self.handle.insert_source(source, move |event, connection, state| state.remote_event(id, event, connection)) {
            Ok(t) => t,
            Err(e) => {
                warn!("remote control: can't take the connection: {}", e.error);
                return;
            }
        };
        info!(session = grant.session, devices = grant.devices, "remote control connected");
        self.remote.live.push(Live { id, session: grant.session, devices: grant.devices, token, seat: None, connection: None, wheel: (0, 0) });
    }

    fn remote_event(&mut self, id: u64, event: EiInputEvent, connection: &mut EiInputConnection) {
        let Some(i) = self.remote.live.iter().position(|l| l.id == id) else { return };
        match event {
            EiInputEvent::Connected => {
                let devices = self.remote.live[i].devices;
                let seat = connection.add_seat("Olo");
                if devices & remote::KEYBOARD != 0 {
                    let k = &self.settings.keyboard;
                    let xkb = XkbConfig { layout: &k.layout, variant: &k.variant, options: if k.options.is_empty() { None } else { Some(k.options.clone()) }, ..Default::default() };
                    if seat.add_keyboard("Olo keyboard", xkb).is_err() {
                        let _ = seat.add_keyboard("Olo keyboard", XkbConfig::default());
                    }
                    seat.add_text("Olo text");
                }
                let regions = regions(self);
                if devices & remote::POINTER != 0 {
                    seat.add_pointer("Olo pointer");
                    seat.add_pointer_absolute("Olo absolute pointer", &regions);
                }
                if devices & remote::TOUCHSCREEN != 0 {
                    seat.add_touch("Olo touch screen", &regions);
                }
                let l = &mut self.remote.live[i];
                l.seat = Some(seat);
                l.connection = Some(connection.eis_connection().clone());
            }
            EiInputEvent::Disconnected => {
                let l = self.remote.live.remove(i);
                info!(session = l.session, "remote control disconnected");
                if self.remote.live.is_empty() {
                    self.remote_release_keys();
                }
            }
            EiInputEvent::Event(e) => {
                let devices = self.remote.live[i].devices;
                for s in self.remote_synthetic(i, e) {
                    // Only what was granted (the seat has no other devices,
                    // so this is belt and braces).
                    let needs = match &s {
                        S::Key { .. } | S::Keysym { .. } => remote::KEYBOARD,
                        S::Touch { .. } => remote::TOUCHSCREEN,
                        _ => remote::POINTER,
                    };
                    if devices & needs != 0 {
                        self.synthetic_input(s);
                    }
                }
            }
            EiInputEvent::TextKeysym { keysym, state } => {
                if self.remote.live[i].devices & remote::KEYBOARD != 0 {
                    self.synthetic_input(S::Keysym { keysym, pressed: state == smithay::backend::input::KeyState::Pressed });
                }
            }
            EiInputEvent::TextUtf8 { text } => {
                if self.remote.live[i].devices & remote::KEYBOARD != 0 {
                    self.type_text(&text);
                }
            }
        }
    }

    /// A libei event as synthetic input (none for what isn't input).
    fn remote_synthetic(&mut self, i: usize, e: InputEvent<EiInput>) -> Vec<S> {
        use smithay::reexports::reis::eis::{button::ButtonState, keyboard::KeyState};
        match e {
            InputEvent::PointerMotion { event } => vec![S::MoveBy { dx: event.dx.into(), dy: event.dy.into() }],
            InputEvent::PointerMotionAbsolute { event } => vec![S::Move { x: event.dx_absolute.into(), y: event.dy_absolute.into() }],
            InputEvent::PointerButton { event } => vec![S::Button { button: event.button, pressed: event.state == ButtonState::Press }],
            InputEvent::Keyboard { event } => vec![S::Key { key: event.key, pressed: event.state == KeyState::Press }],
            InputEvent::PointerAxis { event } => match event {
                ScrollEvent::Delta(d) => vec![S::Scroll { dx: d.dx.into(), dy: d.dy.into(), at_us: None }],
                // Stopped (a glide may follow) or cancelled: the fingers
                // lifted, either way.
                ScrollEvent::Stop(_) | ScrollEvent::Cancel(_) => vec![S::Scroll { dx: 0.0, dy: 0.0, at_us: None }],
                ScrollEvent::Discrete(d) => {
                    let w = &mut self.remote.live[i].wheel;
                    w.0 += d.discrete_dx;
                    w.1 += d.discrete_dy;
                    let (dx, dy) = (w.0 / NOTCH, w.1 / NOTCH);
                    w.0 -= dx * NOTCH;
                    w.1 -= dy * NOTCH;
                    if dx == 0 && dy == 0 { Vec::new() } else { vec![S::Wheel { dx, dy }] }
                }
            },
            InputEvent::TouchDown { event } => vec![S::Touch { slot: event.touch_id, phase: TouchPhase::Down, x: event.x.into(), y: event.y.into(), window: None }],
            InputEvent::TouchMotion { event } => vec![S::Touch { slot: event.touch_id, phase: TouchPhase::Motion, x: event.x.into(), y: event.y.into(), window: None }],
            InputEvent::TouchUp { event } => vec![S::Touch { slot: event.touch_id, phase: TouchPhase::Up, x: 0.0, y: 0.0, window: None }],
            InputEvent::TouchCancel { event } => vec![S::Touch { slot: event.touch_id, phase: TouchPhase::Up, x: 0.0, y: 0.0, window: None }],
            _ => Vec::new(),
        }
    }

    /// The displays changed: what absolute devices point at follows.
    pub fn remote_displays_changed(&mut self) {
        if self.remote.live.iter().all(|l| l.seat.is_none()) {
            return;
        }
        let regions = regions(self);
        for l in &self.remote.live {
            if let Some(seat) = &l.seat {
                seat.update_regions(&regions);
            }
        }
    }

    /// The last connection went: keys it left down go up (a program that
    /// died mid-shortcut mustn't leave ⌘ held).
    fn remote_release_keys(&mut self) {
        self.remote.shifted.clear();
        self.release_all_keys();
    }

    /// Text typed as keys, each character on a keymap of its own (libei's
    /// text device; as `insert_keys` does).
    pub fn type_text(&mut self, text: &str) {
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let keysyms: Vec<_> = text
            .chars()
            .map(|c| match c {
                '\n' => smithay::input::keyboard::xkb::Keysym::Return,
                '\t' => smithay::input::keyboard::xkb::Keysym::Tab,
                c => smithay::input::keyboard::xkb::utf32_to_keysym(c as u32),
            })
            .filter(|k| k.raw() != 0)
            .collect();
        keyboard.inject_text_keysyms(self, &keysyms);
    }

    /// The key (evdev code) that types `keysym` on the keyboard's layout
    /// as it is, and whether with Shift; None if none does without more
    /// (AltGr's level, or not on the layout).
    pub fn key_for_keysym(&mut self, keysym: u32) -> Option<(u32, bool)> {
        let keyboard = self.seat.get_keyboard()?;
        keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            // SAFETY: read only, the state's own keymap, not kept.
            let keymap = unsafe { xkb.keymap() };
            let layout = xkb.active_layout().0;
            let want = smithay::input::keyboard::xkb::Keysym::new(keysym);
            let mut found = None;
            keymap.key_for_each(|_, code| {
                if found.is_some_and(|(_, shift): (u32, bool)| !shift) {
                    return;
                }
                for level in 0..keymap.num_levels_for_key(code, layout).min(2) {
                    if keymap.key_get_syms_by_level(code, layout, level).contains(&want) && code.raw() >= 8 {
                        let better = match found {
                            None => true,
                            Some((_, shift)) => shift && level == 0,
                        };
                        if better {
                            found = Some((code.raw() - 8, level == 1));
                        }
                    }
                }
            });
            found
        })
    }

    /// `SyntheticEvent::Keysym`: the key that types it, with Shift if it
    /// takes it (and Shift isn't down already); typed if no key does.
    pub fn keysym_input(&mut self, keysym: u32, pressed: bool) {
        const SHIFT: u32 = 42;
        match self.key_for_keysym(keysym) {
            Some((code, shift)) => {
                if pressed {
                    let held = self.seat.get_keyboard().is_some_and(|k| k.modifier_state().shift);
                    if shift && !held {
                        self.synthetic_input(S::Key { key: SHIFT, pressed: true });
                        self.remote.shifted.push(keysym);
                    }
                    self.synthetic_input(S::Key { key: code, pressed: true });
                } else {
                    self.synthetic_input(S::Key { key: code, pressed: false });
                    if let Some(i) = self.remote.shifted.iter().position(|&k| k == keysym) {
                        self.remote.shifted.remove(i);
                        self.synthetic_input(S::Key { key: SHIFT, pressed: false });
                    }
                }
            }
            // (On a keymap of its own, as text is typed: any keysym.)
            None if pressed => {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.inject_text_keysyms(self, &[smithay::input::keyboard::xkb::Keysym::new(keysym)]);
                }
            }
            None => {}
        }
    }
}
