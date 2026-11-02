//! Keeping the screens awake for apps: during a call, a video, a
//! presentation. Two ways in, both here: the Inhibit portal
//! (`org.freedesktop.impl.portal.Inhibit`: an inhibition lasts until the
//! portal closes its request object), and `org.freedesktop.ScreenSaver`
//! (what Chromium's and Electron's wake locks, and older apps, call: a
//! cookie, until UnInhibit or the caller leaves the bus). Each one holds
//! a connection to app_server open with `KeepAwake`, which keeps the
//! screens from dimming, sleeping or locking until it closes; app_server
//! counts them with the app inhibitors (Deskbar's cup shows them).

use olo_sync::Mutex;
use std::{collections::HashMap, sync::Arc};

use zbus::{
    message::Header,
    zvariant::{OwnedObjectPath, OwnedValue},
};

/// Inhibition flags (bits): logging out, switching user, suspending, idle.
const SUSPEND: u32 = 4;
const IDLE: u32 = 8;

/// A connection to app_server keeping the screens awake while it lives.
pub struct Awake(#[allow(dead_code)] olo_protocol::Client);

impl Awake {
    pub fn keep(why: &str) -> Option<Awake> {
        let mut c = olo_protocol::Client::connect().ok()?;
        match c.request(&olo_protocol::Request::KeepAwake { why: why.to_owned() }) {
            Ok(_) => Some(Awake(c)),
            Err(e) => {
                tracing::warn!("portal: app_server won't keep awake: {e}");
                None
            }
        }
    }
}

/// `org.freedesktop.impl.portal.Inhibit`.
pub struct Inhibit;

#[zbus::interface(name = "org.freedesktop.impl.portal.Inhibit")]
impl Inhibit {
    async fn inhibit(&self, handle: OwnedObjectPath, app_id: String, _window: String, flags: u32, options: HashMap<String, OwnedValue>, #[zbus(object_server)] server: &zbus::ObjectServer) {
        if flags & (SUSPEND | IDLE) == 0 {
            return;
        }
        let reason = options.get("reason").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
        let app = crate::app_name(&app_id);
        let why = [app, reason].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(": ");
        let awake = blocking::unblock(move || Awake::keep(&why)).await;
        // Kept until the portal closes the request (the app let go, or left).
        let _ = server.at(handle.as_ref(), Held { awake: Mutex::new(awake) }).await;
    }

    /// Session state isn't reported (logging out isn't held up).
    async fn create_monitor(&self, _handle: OwnedObjectPath, _session_handle: OwnedObjectPath, _app_id: String, _window: String) -> u32 {
        2
    }

    fn query_end_response(&self, _session_handle: OwnedObjectPath) {}

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        3
    }
}

/// The request an inhibition lives in.
struct Held {
    awake: Mutex<Option<Awake>>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
impl Held {
    async fn close(&self, #[zbus(object_server)] server: &zbus::ObjectServer, #[zbus(header)] header: Header<'_>) {
        self.awake.lock().unwrap().take();
        if let Some(path) = header.path() {
            let _ = server.remove::<Held, _>(path).await;
        }
    }
}

/// `org.freedesktop.ScreenSaver`'s inhibitions: cookie → who asked (its
/// bus name) and what keeps the screens awake for it.
#[derive(Default, Clone)]
pub struct ScreenSaver {
    held: Arc<Mutex<HashMap<u32, (String, Awake)>>>,
    next: Arc<Mutex<u32>>,
}

impl ScreenSaver {
    /// The caller `name` left the bus: its inhibitions go with it.
    pub fn left(&self, name: &str) {
        self.held.lock().unwrap().retain(|_, (who, _)| who != name);
    }
}

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl ScreenSaver {
    async fn inhibit(&self, application_name: String, reason_for_inhibit: String, #[zbus(header)] header: Header<'_>) -> u32 {
        let who = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let why = [application_name, reason_for_inhibit].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(": ");
        let Some(awake) = blocking::unblock(move || Awake::keep(&why)).await else { return 0 };
        let cookie = {
            let mut next = self.next.lock().unwrap();
            *next = next.wrapping_add(1).max(1);
            *next
        };
        self.held.lock().unwrap().insert(cookie, (who, awake));
        cookie
    }

    fn un_inhibit(&self, cookie: u32) {
        self.held.lock().unwrap().remove(&cookie);
    }

    fn get_active(&self) -> bool {
        false
    }

    fn get_active_time(&self) -> u32 {
        0
    }

    fn simulate_user_activity(&self) {}
}

/// Serve `org.freedesktop.ScreenSaver` on `conn` (both paths apps use),
/// if no one else has the name, releasing a caller's inhibitions when it
/// leaves the bus.
pub fn serve_screensaver(conn: &zbus::blocking::Connection) -> zbus::Result<()> {
    let saver = ScreenSaver::default();
    for path in ["/org/freedesktop/ScreenSaver", "/ScreenSaver"] {
        conn.object_server().at(path, saver.clone())?;
    }
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    match conn.request_name_with_flags("org.freedesktop.ScreenSaver", flags)? {
        zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner => {}
        _ => return Err(zbus::Error::NameTaken),
    }
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn)?;
    let changes = dbus.receive_name_owner_changed()?;
    std::thread::Builder::new()
        .name("portal-screensaver".into())
        .spawn(move || {
            for change in changes {
                if let Ok(args) = change.args()
                    && args.new_owner().is_none()
                {
                    saver.left(args.name());
                }
            }
        })
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    Ok(())
}
