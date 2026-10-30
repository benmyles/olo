//! The portal backend on D-Bus: `org.freedesktop.impl.portal.desktop.olo`,
//! which xdg-desktop-portal calls for `org.freedesktop.portal.ScreenCast`
//! (and Inhibit, `inhibit.rs`, Screenshot, `shot.rs`, and Access, the
//! permission dialog, `access.rs`) when `olo-portals.conf` names us for
//! it. An app's share is a session:
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

/// Source types (bits): screens, windows.
const MONITOR: u32 = 1;
const WINDOW: u32 = 2;
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
    /// A permission to ask (the Access portal).
    Access(crate::access::Ask),
    /// The app gave up on the request (its handle): close its picker.
    Cancel(String),
    /// A session ended (an upgrade may have waited for it: `App::busy`).
    Ended,
    /// A session's stream ended by itself (app_server never came back):
    /// the session ends too (`end`).
    Lost(String),
}

/// An app's share.
#[derive(Default)]
struct Session {
    app: String,
    /// What it may share (`MONITOR`, `WINDOW`, both).
    types: u32,
    /// What people call the app and the screens shared, and since when
    /// (for Deskbar: `olo_protocol::SharingState`).
    app_name: String,
    labels: Vec<String>,
    since: u64,
    cursor: u32,
    persist: u32,
    /// A screen chosen before (a restore token), to share without asking.
    restore: Option<String>,
    /// Its streams (each stops as it drops: when the session closes).
    shares: Vec<stream::Share>,
}

type Sessions = Arc<Mutex<HashMap<OwnedObjectPath, Session>>>;

/// Tell Deskbar what is shared now.
fn publish(sessions: &HashMap<OwnedObjectPath, Session>) {
    let mut shares: Vec<olo_protocol::sharing::Share> = sessions
        .iter()
        .filter(|(_, s)| !s.shares.is_empty())
        .map(|(path, s)| olo_protocol::sharing::Share { session: path.to_string(), app: s.app_name.clone(), screens: s.labels.clone(), since: s.since })
        .collect();
    shares.sort_by_key(|s| s.since);
    let state = olo_protocol::SharingState { pid: std::process::id(), shares };
    if let Err(e) = state.save() {
        tracing::warn!("portal: can't say what is shared: {e}");
    }
}

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
        s.types = match get_u32("types").unwrap_or(MONITOR) & (MONITOR | WINDOW) {
            0 => MONITOR,
            t => t,
        };
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
        let (app, restore, cursor, persist, types) = {
            let sessions = self.sessions.lock().unwrap();
            let Some(s) = sessions.get(&session_handle) else { return (FAILED, Results::new()) };
            (s.app.clone(), s.restore.clone(), s.cursor, s.persist, if s.types == 0 { MONITOR } else { s.types })
        };
        let all = crate::shareables();
        let kind = |window: bool| if window { WINDOW } else { MONITOR };
        let known = |k: &String| all.iter().any(|s| &s.key == k && (types & kind(s.window)) != 0);
        // Without asking: a screen this app chose before (still there), or
        // the one it chose moments ago in another session.
        let repeat = self.last.lock().unwrap().as_ref().filter(|(a, at, _)| *a == app && at.elapsed() < REPEAT).map(|(_, _, c)| c.clone());
        let chosen = match restore.filter(known).map(|o| vec![o]).or(repeat) {
            Some(c) => Some(c),
            None => {
                let (tx, rx) = async_channel::bounded(1);
                let request = RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
                let _ = server.at(handle.as_ref(), request).await;
                (self.to_ui)(ToUi::Pick(picker::Ask { request: handle.to_string(), app: crate::app_name(&app), parent: parent_window, types, reply: tx }));
                let answer = rx.recv().await.ok().flatten();
                let _ = server.remove::<RequestObject, _>(handle.as_ref()).await;
                answer
            }
        };
        let Some(chosen) = chosen.filter(|c| !c.is_empty()) else { return (CANCELLED, Results::new()) };
        // What each is (gone since it was chosen: the share fails).
        let Some(picked) = chosen.iter().map(|k| all.iter().find(|s| &s.key == k)).collect::<Option<Vec<_>>>() else { return (FAILED, Results::new()) };
        *self.last.lock().unwrap() = Some((app, Instant::now(), chosen.clone()));
        // The streams, each on its own thread (PipeWire's start can take a
        // moment: off the bus's executor).
        let draw_cursor = cursor & EMBEDDED != 0;
        let sources: Vec<crate::capture::Source> = picked.iter().map(|s| s.source.clone()).collect();
        let (to_ui, path) = (self.to_ui.clone(), session_handle.to_string());
        let started = blocking::unblock(move || {
            sources
                .into_iter()
                .map(|source| {
                    let (to_ui, path) = (to_ui.clone(), path.clone());
                    stream::run(source, draw_cursor, move || to_ui(ToUi::Lost(path)))
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .await;
        let shares = match started {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("portal: can't share: {e}");
                return (FAILED, Results::new());
            }
        };
        let streams: Vec<(u32, HashMap<String, OwnedValue>)> = shares
            .iter()
            .zip(&picked)
            .map(|(share, what)| {
                let mut props = HashMap::from([
                    ("source_type".to_owned(), value(if what.window { WINDOW } else { MONITOR })),
                    ("size".to_owned(), value(what.logical)),
                    ("id".to_owned(), value(what.key.clone())),
                ]);
                // (A screen's place on the desktop; a window has none to give.)
                if !what.window {
                    props.insert("position".to_owned(), value(what.position));
                }
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
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(&session_handle) {
            Some(s) => {
                s.shares = shares;
                s.app_name = crate::app_name(&s.app);
                s.labels = picked.iter().map(|p| p.label.clone()).collect();
                s.since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            }
            // Closed while the picker was up: the shares stop as they drop.
            None => return (CANCELLED, Results::new()),
        }
        publish(&sessions);
        (OK, results)
    }

    #[zbus(property, name = "AvailableSourceTypes")]
    fn available_source_types(&self) -> u32 {
        MONITOR | WINDOW
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
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.remove(&self.path);
            publish(&sessions);
        }
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

/// `org.freedesktop.impl.portal.Request`: a picker or dialog, while it is
/// up; the app giving up closes it.
pub struct RequestObject {
    pub handle: String,
    pub to_ui: Arc<dyn Fn(ToUi) + Send + Sync>,
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
    // Nothing is shared yet (a file a crashed portal left goes).
    publish(&HashMap::new());
    let access = crate::access::Access { to_ui: cast.to_ui.clone() };
    let conn = builder.serve_at(PATH, cast)?.serve_at(PATH, crate::inhibit::Inhibit)?.serve_at(PATH, crate::shot::Screenshot)?.serve_at(PATH, access)?.build()?;
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    match conn.request_name_with_flags(NAME, flags)? {
        zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner => {}
        _ => return Err(zbus::Error::NameTaken),
    }
    // Wake locks the older way, unless something else has it.
    if let Err(e) = crate::inhibit::serve_screensaver(&conn) {
        tracing::info!("portal: not serving org.freedesktop.ScreenSaver: {e}");
    }
    Ok(conn)
}

/// End `session` from our side (Stop in Deskbar, or its stream died):
/// its streams stop, and the portal (so the app) is told it closed.
pub fn end(conn: &zbus::blocking::Connection, session: &str) {
    let Ok(path) = ObjectPath::try_from(session) else { return };
    let server = conn.object_server();
    if let Ok(cast) = server.interface::<_, ScreenCast>(PATH) {
        let sessions = cast.get().sessions.clone();
        let mut sessions = sessions.lock().unwrap();
        sessions.remove(&OwnedObjectPath::from(path.clone()));
        publish(&sessions);
    }
    if let Ok(iface) = server.interface::<_, SessionObject>(&path) {
        let _ = zbus::block_on(SessionObject::closed(iface.signal_emitter()));
        let _ = server.remove::<SessionObject, _>(&path);
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
