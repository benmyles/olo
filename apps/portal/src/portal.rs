//! The portal backend on D-Bus: `org.freedesktop.impl.portal.desktop.olo`,
//! which xdg-desktop-portal calls for `org.freedesktop.portal.ScreenCast`
//! when `olo-portals.conf` names us for it. An app's share is a session:
//! CreateSession makes it (an `impl.portal.Session` object at the handle
//! the portal chose, closed by it or by us), SelectSources says what the
//! app wants, and Start asks the person (the picker, `picker.rs`, unless a
//! restore token or a repeat request answers it), starts a stream of each
//! screen chosen (`stream.rs`) and hands back their PipeWire nodes. The
//! portal itself gives the app a PipeWire connection that sees only those.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use zbus::{
    object_server::SignalEmitter,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

use crate::{picker, stream};

pub const NAME: &str = "org.freedesktop.impl.portal.desktop.olo";
pub const PATH: &str = "/org/freedesktop/portal/desktop";

/// Source types (bits): screens; windows come later.
const MONITOR: u32 = 1;
/// Cursor modes (bits): hidden, drawn into the picture.
const HIDDEN: u32 = 1;
const EMBEDDED: u32 = 2;
/// Responses.
const OK: u32 = 0;
const CANCELLED: u32 = 1;
const FAILED: u32 = 2;
/// A choice answers another Start from the same app this soon after it
/// (Chromium opens several sessions for one share: one picker, not three).
const REPEAT: Duration = Duration::from_secs(5);

/// What the UI (the picker) is asked from here.
pub enum ToUi {
    Pick(picker::Ask),
    /// The app gave up on the request (its handle): close its picker.
    Cancel(String),
    /// A session ended (an upgrade may have waited for it: `App::busy`).
    Ended,
}

/// An app's share.
#[derive(Default)]
struct Session {
    app: String,
    cursor: u32,
    persist: u32,
    /// A screen chosen before (a restore token), to share without asking.
    restore: Option<String>,
    /// Its streams (each stops as it drops: when the session closes).
    shares: Vec<stream::Share>,
}

type Sessions = Arc<Mutex<HashMap<OwnedObjectPath, Session>>>;

pub struct ScreenCast {
    sessions: Sessions,
    to_ui: Arc<dyn Fn(ToUi) + Send + Sync>,
    /// The last choice, by app, and when.
    last: Arc<Mutex<Option<(String, Instant, Vec<String>)>>>,
}

type Results = HashMap<String, OwnedValue>;

fn value(v: impl Into<Value<'static>>) -> OwnedValue {
    OwnedValue::try_from(v.into()).expect("no file descriptors")
}

#[zbus::interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, Results) {
        let object = SessionObject { path: session_handle.clone(), sessions: self.sessions.clone(), to_ui: self.to_ui.clone() };
        if server.at(session_handle.as_ref(), object).await.is_err() {
            return (FAILED, Results::new());
        }
        self.sessions.lock().unwrap().insert(session_handle.clone(), Session { app: app_id, ..Default::default() });
        (OK, HashMap::from([("session_id".to_owned(), value(session_handle.as_str().to_owned()))]))
    }

    async fn select_sources(&self, _handle: OwnedObjectPath, session_handle: OwnedObjectPath, _app_id: String, options: HashMap<String, OwnedValue>) -> (u32, Results) {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(s) = sessions.get_mut(&session_handle) else { return (FAILED, Results::new()) };
        let get_u32 = |k: &str| options.get(k).and_then(|v| u32::try_from(v).ok());
        s.cursor = get_u32("cursor_mode").unwrap_or(HIDDEN);
        s.persist = get_u32("persist_mode").unwrap_or(0);
        s.restore = options.get("restore_data").and_then(restored);
        tracing::info!(app = s.app, cursor = s.cursor, persist = s.persist, restore = ?s.restore, "portal: sources selected");
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
        let (app, restore, cursor, persist) = {
            let sessions = self.sessions.lock().unwrap();
            let Some(s) = sessions.get(&session_handle) else { return (FAILED, Results::new()) };
            (s.app.clone(), s.restore.clone(), s.cursor, s.persist)
        };
        let screens = crate::screens();
        let known = |o: &String| screens.iter().any(|s| &s.output == o);
        // Without asking: a screen this app chose before (still there), or
        // the one it chose moments ago in another session.
        let repeat = self.last.lock().unwrap().as_ref().filter(|(a, at, _)| *a == app && at.elapsed() < REPEAT).map(|(_, _, c)| c.clone());
        let chosen = match restore.filter(known).map(|o| vec![o]).or(repeat) {
            Some(c) => Some(c),
            None => {
                let (tx, rx) = async_channel::bounded(1);
                let request = RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
                let _ = server.at(handle.as_ref(), request).await;
                (self.to_ui)(ToUi::Pick(picker::Ask { request: handle.to_string(), app: crate::app_name(&app), parent: parent_window, reply: tx }));
                let answer = rx.recv().await.ok().flatten();
                let _ = server.remove::<RequestObject, _>(handle.as_ref()).await;
                answer
            }
        };
        let Some(chosen) = chosen.filter(|c| !c.is_empty()) else { return (CANCELLED, Results::new()) };
        *self.last.lock().unwrap() = Some((app, Instant::now(), chosen.clone()));
        // The streams, each on its own thread (PipeWire's start can take a
        // moment: off the bus's executor).
        let draw_cursor = cursor & EMBEDDED != 0;
        let outputs = chosen.clone();
        let started = blocking::unblock(move || outputs.into_iter().map(|o| stream::run(Some(o), draw_cursor)).collect::<Result<Vec<_>, String>>()).await;
        let shares = match started {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("portal: can't share: {e}");
                return (FAILED, Results::new());
            }
        };
        let streams: Vec<(u32, HashMap<String, OwnedValue>)> = shares
            .iter()
            .zip(&chosen)
            .map(|(share, output)| {
                let screen = screens.iter().find(|s| &s.output == output);
                let (x, y) = screen.map(|s| s.position).unwrap_or_default();
                let (w, h) = screen.map(|s| s.logical).unwrap_or((share.size.0 as i32, share.size.1 as i32));
                let props = HashMap::from([
                    ("source_type".to_owned(), value(MONITOR)),
                    ("position".to_owned(), value((x, y))),
                    ("size".to_owned(), value((w, h))),
                    ("id".to_owned(), value(output.clone())),
                ]);
                (share.node, props)
            })
            .collect();
        let mut results = HashMap::from([("streams".to_owned(), value(streams))]);
        if persist != 0
            && let [output] = chosen.as_slice()
        {
            results.insert("persist_mode".into(), value(persist.min(2)));
            results.insert("restore_data".into(), restore_data(output));
        }
        match self.sessions.lock().unwrap().get_mut(&session_handle) {
            Some(s) => s.shares = shares,
            // Closed while the picker was up: the shares stop as they drop.
            None => return (CANCELLED, Results::new()),
        }
        (OK, results)
    }

    #[zbus(property, name = "AvailableSourceTypes")]
    fn available_source_types(&self) -> u32 {
        MONITOR
    }

    #[zbus(property, name = "AvailableCursorModes")]
    fn available_cursor_modes(&self) -> u32 {
        HIDDEN | EMBEDDED
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        5
    }
}

/// What Start handed out to restore a choice by: ("olo", 1, the screen's
/// connector).
fn restore_data(output: &str) -> OwnedValue {
    value(("olo".to_owned(), 1u32, Value::from(output.to_owned())))
}

/// The screen a restore token (as the portal gives it back) chose.
fn restored(v: &OwnedValue) -> Option<String> {
    let Value::Structure(s) = &**v else { return None };
    let [Value::Str(vendor), Value::U32(1), data] = s.fields() else { return None };
    if vendor.as_str() != "olo" {
        return None;
    }
    // The data is a variant: its value, however deep the portal wrapped it.
    let mut data = data;
    while let Value::Value(inner) = data {
        data = inner;
    }
    match data {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// `org.freedesktop.impl.portal.Session`: one app's share, until it (or
/// the portal for it) closes it.
struct SessionObject {
    path: OwnedObjectPath,
    sessions: Sessions,
    to_ui: Arc<dyn Fn(ToUi) + Send + Sync>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Session")]
impl SessionObject {
    async fn close(&self, #[zbus(object_server)] server: &zbus::ObjectServer) {
        self.sessions.lock().unwrap().remove(&self.path);
        (self.to_ui)(ToUi::Ended);
        let _ = server.remove::<SessionObject, _>(self.path.as_ref()).await;
    }

    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

/// `org.freedesktop.impl.portal.Request`: the picker, while it is up; the
/// app giving up closes it.
struct RequestObject {
    handle: String,
    to_ui: Arc<dyn Fn(ToUi) + Send + Sync>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
impl RequestObject {
    fn close(&self) {
        (self.to_ui)(ToUi::Cancel(self.handle.clone()));
    }
}

/// Serve the backend on the session bus (or `address`, a test's), taking
/// the name; until the connection drops.
pub fn serve(address: Option<&str>, to_ui: impl Fn(ToUi) + Send + Sync + 'static) -> zbus::Result<zbus::blocking::Connection> {
    let builder = match address {
        Some(a) => zbus::blocking::connection::Builder::address(a)?,
        None => zbus::blocking::connection::Builder::session()?,
    };
    let cast = ScreenCast { sessions: Arc::default(), to_ui: Arc::new(to_ui), last: Arc::default() };
    let conn = builder.serve_at(PATH, cast)?.build()?;
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    match conn.request_name_with_flags(NAME, flags)? {
        zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner => Ok(conn),
        _ => Err(zbus::Error::NameTaken),
    }
}

/// End `session` from our side (its streams stopped): tell the portal.
#[allow(dead_code)]
pub fn close(conn: &zbus::blocking::Connection, session: &ObjectPath<'_>) {
    let server = conn.object_server();
    if let Ok(iface) = server.interface::<_, SessionObject>(session) {
        let _ = zbus::block_on(SessionObject::closed(iface.signal_emitter()));
        let _ = server.remove::<SessionObject, _>(session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_data_comes_back() {
        assert_eq!(restored(&restore_data("DP-1")).as_deref(), Some("DP-1"));
        // As the portal hands it back: the data a variant in a variant.
        let wrapped = value(("olo".to_owned(), 1u32, Value::Value(Box::new(Value::from("eDP-1")))));
        assert_eq!(restored(&wrapped).as_deref(), Some("eDP-1"));
        // Another backend's, or another version's: not ours to restore.
        assert_eq!(restored(&value(("gnome".to_owned(), 1u32, Value::from("DP-1")))), None);
        assert_eq!(restored(&value(("olo".to_owned(), 2u32, Value::from("DP-1")))), None);
    }
}
