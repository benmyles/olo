//! The portal backend on D-Bus: `org.freedesktop.impl.portal.desktop.olo`,
//! which xdg-desktop-portal calls for `org.freedesktop.portal.ScreenCast`
//! (and Inhibit, `inhibit.rs`, Screenshot, `shot.rs`, Access, the
//! permission dialog, `access.rs`, and FileChooser, `files.rs`) when
//! `olo-portals.conf` names us for it. An app's share is a session:
//! CreateSession makes it (an `impl.portal.Session` object at the handle
//! the portal chose, closed by it or by us), SelectSources says what the
//! app wants, and Start asks the person (the picker, `picker.rs`, unless a
//! restore token or a repeat request answers it), starts a stream of each
//! screen chosen (`stream.rs`) and hands back their PipeWire nodes. The
//! portal itself gives the app a PipeWire connection that sees only those.
//! RemoteDesktop (`remote.rs`) shares the sessions: one may share screens
//! and control the computer.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::Arc,
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
    /// A file to open or save (the FileChooser portal).
    Files(crate::files::Ask),
    /// Which app opens something (the AppChooser portal).
    Choose(crate::chooser::Ask),
    /// The apps a chooser (its request) offers changed.
    Offers(String, Vec<crate::chooser::Offer>),
    /// A picture to make the desktop background (the Wallpaper portal).
    Wallpaper(crate::wallpaper::Ask),
    /// An app's keys to allow (the GlobalShortcuts portal).
    Shortcuts(crate::shortcuts::Ask),
    /// Where and how to print (the Print portal).
    Print(crate::print::Ask),
    /// The app gave up on the request (its handle): close its picker.
    Cancel(String),
    /// A session ended (an upgrade may have waited for it: `App::busy`).
    Ended,
    /// A session's stream ended by itself (app_server never came back):
    /// the session ends too (`end`).
    Lost(String),
}

/// An app's share (and, for RemoteDesktop, its control).
#[derive(Default)]
pub(crate) struct Session {
    pub(crate) app: String,
    /// What it may share (`MONITOR`, `WINDOW`, both).
    types: u32,
    /// What people call the app and the screens shared, and since when
    /// (for Deskbar: `olo_protocol::SharingState`).
    app_name: String,
    labels: Vec<String>,
    /// Its windows' ids (app_server's), of those shared that are windows.
    windows: Vec<u64>,
    since: u64,
    cursor: u32,
    persist: u32,
    /// A screen chosen before (a restore token), to share without asking.
    restore: Option<String>,
    /// Its streams (each stops as it drops: when the session closes).
    shares: Vec<stream::Share>,
    /// A RemoteDesktop session (`remote.rs`): the devices it asked for
    /// (SelectDevices), those the person let it control (Start), and
    /// whether it took a libei connection (then the Notify calls are
    /// refused, as the portal says).
    pub(crate) remote: bool,
    pub(crate) devices: u32,
    pub(crate) controls: u32,
    pub(crate) eis: bool,
    /// Its streams' PipeWire nodes and what each shows: where absolute
    /// input on a stream goes.
    pub(crate) nodes: Vec<(u32, Shown)>,
}

/// What a stream shows, for input given in its coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Shown {
    /// A screen, at this place on the desktop (logical).
    Screen(i32, i32),
    /// A window (app_server's id): its content.
    Window(u64),
    /// Something not on this desktop (an agent's screen).
    Elsewhere,
}

pub(crate) type Sessions = Arc<Mutex<HashMap<OwnedObjectPath, Session>>>;

/// Tell Deskbar what is shared now.
pub(crate) fn publish(sessions: &HashMap<OwnedObjectPath, Session>) {
    let mut shares: Vec<olo_protocol::sharing::Share> = sessions
        .iter()
        .filter(|(_, s)| !s.shares.is_empty() || s.controls != 0)
        .map(|(path, s)| olo_protocol::sharing::Share {
            session: path.to_string(),
            app: s.app_name.clone(),
            screens: s.labels.clone(),
            since: s.since,
            windows: s.windows.clone(),
            control: crate::remote::devices_named(s.controls),
        })
        .collect();
    shares.sort_by_key(|s| s.since);
    // app_server rings the windows shared, while they are.
    let mut windows: Vec<u64> = shares.iter().flat_map(|s| s.windows.iter().copied()).collect();
    windows.sort_unstable();
    windows.dedup();
    let state = olo_protocol::SharingState { pid: std::process::id(), shares };
    if let Err(e) = state.save() {
        tracing::warn!("portal: can't say what is shared: {e}");
    }
    tell_shared_windows(windows);
}

/// Tell app_server which windows are shared (the last told, kept: a
/// session's change that leaves them as they were says nothing).
fn tell_shared_windows(windows: Vec<u64>) {
    static TOLD: std::sync::Mutex<Option<Vec<u64>>> = std::sync::Mutex::new(None);
    {
        let mut told = TOLD.lock().unwrap_or_else(|e| e.into_inner());
        if told.as_ref() == Some(&windows) {
            return;
        }
        *told = Some(windows.clone());
    }
    std::thread::spawn(move || {
        if let Ok(mut c) = olo_protocol::Client::connect()
            && let Err(e) = c.request(&olo_protocol::Request::SharedWindows { ids: windows })
        {
            tracing::warn!("portal: app_server wasn't told which windows are shared: {e}");
        }
    });
}

/// What ScreenCast and RemoteDesktop share: the sessions (a remote
/// desktop session may share screens too), the UI, the last choice.
#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) sessions: Sessions,
    pub(crate) to_ui: Arc<dyn Fn(ToUi) + Send + Sync>,
    /// The last choice, by app, and when.
    last: Arc<Mutex<Option<(String, Instant, Vec<String>)>>>,
}

pub struct ScreenCast {
    shared: Shared,
}

pub(crate) type Results = HashMap<String, OwnedValue>;

pub(crate) fn value(v: impl Into<Value<'static>>) -> OwnedValue {
    OwnedValue::try_from(v.into()).expect("no file descriptors")
}

impl Shared {
    /// CreateSession (either portal's): the session's object, and it
    /// kept.
    pub(crate) async fn create(&self, session_handle: OwnedObjectPath, app_id: String, remote: bool, server: &zbus::ObjectServer) -> (u32, Results) {
        let object = SessionObject { path: session_handle.clone(), sessions: self.sessions.clone(), to_ui: self.to_ui.clone() };
        if server.at(session_handle.as_ref(), object).await.is_err() {
            return (FAILED, Results::new());
        }
        self.sessions.lock().unwrap().insert(session_handle.clone(), Session { app: app_id, remote, ..Default::default() });
        (OK, HashMap::from([("session_id".to_owned(), value(session_handle.as_str().to_owned()))]))
    }

    /// Start (either portal's): asks the person (the picker, saying it
    /// will control `control` too; for control alone, the permission
    /// dialog) and starts a stream of each screen chosen. The results
    /// (`streams`, …) or the response.
    pub(crate) async fn start(&self, handle: OwnedObjectPath, session_handle: OwnedObjectPath, parent_window: String, control: u32, server: &zbus::ObjectServer) -> Result<Results, u32> {
        let (app, restore, cursor, persist, types) = {
            let sessions = self.sessions.lock().unwrap();
            let Some(s) = sessions.get(&session_handle) else { return Err(FAILED) };
            (s.app.clone(), s.restore.clone(), s.cursor, s.persist, s.types)
        };
        if types == 0 && control != 0 {
            return self.start_control(handle, session_handle, parent_window, &app, control, server).await;
        }
        let types = if types == 0 { MONITOR } else { types };
        let all = crate::shareables();
        let kind = |window: bool| if window { WINDOW } else { MONITOR };
        let known = |k: &String| all.iter().any(|s| &s.key == k && (types & kind(s.window)) != 0);
        // Without asking: a screen this app chose before (still there), or
        // the one it chose moments ago in another session. (Never to
        // control: that is asked every time.)
        let repeat = self.last.lock().unwrap().as_ref().filter(|(a, at, _)| *a == app && at.elapsed() < REPEAT).map(|(_, _, c)| c.clone());
        let unasked = if control != 0 { None } else { restore.filter(known).map(|o| vec![o]).or(repeat) };
        let chosen = match unasked {
            Some(c) => Some(c),
            None => {
                let (tx, rx) = async_channel::bounded(1);
                let request = RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
                let _ = server.at(handle.as_ref(), request).await;
                (self.to_ui)(ToUi::Pick(picker::Ask { request: handle.to_string(), app: crate::app_name(&app), parent: parent_window, types, control, reply: tx }));
                let answer = rx.recv().await.ok().flatten();
                let _ = server.remove::<RequestObject, _>(handle.as_ref()).await;
                answer
            }
        };
        let Some(chosen) = chosen.filter(|c| !c.is_empty()) else { return Err(CANCELLED) };
        // What each is (gone since it was chosen: the share fails).
        let Some(picked) = chosen.iter().map(|k| all.iter().find(|s| &s.key == k)).collect::<Option<Vec<_>>>() else { return Err(FAILED) };
        if control == 0 {
            *self.last.lock().unwrap() = Some((app, Instant::now(), chosen.clone()));
        }
        // The streams, each on its own thread (PipeWire's start can take a
        // moment: off the bus's executor).
        let draw_cursor = cursor & EMBEDDED != 0;
        let sources: Vec<olo_capture::Source> = picked.iter().map(|s| s.source.clone()).collect();
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
                return Err(FAILED);
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
                    // (As app_server's libei regions name the screens.)
                    ("mapping_id".to_owned(), value(what.key.clone())),
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
            && control == 0
            && let [output] = chosen.as_slice()
        {
            results.insert("persist_mode".into(), value(persist.min(2)));
            results.insert("restore_data".into(), restore_data(output));
        }
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(&session_handle) {
            Some(s) => {
                s.nodes = shares.iter().zip(&picked).map(|(share, p)| (share.node, shown(p))).collect();
                s.shares = shares;
                s.app_name = crate::app_name(&s.app);
                s.labels = picked.iter().map(|p| p.label.clone()).collect();
                s.windows = picked.iter().filter(|p| p.window).filter_map(|p| p.key.strip_prefix("window:")?.parse().ok()).collect();
                s.since = now();
                s.controls = control;
            }
            // Closed while the picker was up: the shares stop as they drop.
            None => return Err(CANCELLED),
        }
        publish(&sessions);
        Ok(results)
    }

    /// Start for control alone (no screen asked for): the permission
    /// dialog says what it would control.
    async fn start_control(&self, handle: OwnedObjectPath, session_handle: OwnedObjectPath, parent_window: String, app: &str, control: u32, server: &zbus::ObjectServer) -> Result<Results, u32> {
        let name = crate::app_name(app);
        let who = if name.is_empty() { "an app".to_owned() } else { name.clone() };
        let (tx, rx) = async_channel::bounded(1);
        let request = RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
        let _ = server.at(handle.as_ref(), request).await;
        (self.to_ui)(ToUi::Access(crate::access::Ask {
            request: handle.to_string(),
            parent: parent_window,
            title: format!("Allow {who} to control this computer?"),
            subtitle: format!("It can use your {} as you do: type, click and move the pointer, until you stop it.", crate::remote::devices_said(control)),
            body: "Deskbar shows it while it does, with Stop.".into(),
            deny: "Don’t Allow".into(),
            grant: "Allow".into(),
            icon: String::new(),
            choices: Vec::new(),
            reply: tx,
        }));
        let answer = rx.recv().await.ok().flatten();
        let _ = server.remove::<RequestObject, _>(handle.as_ref()).await;
        if answer.is_none() {
            return Err(CANCELLED);
        }
        let mut sessions = self.sessions.lock().unwrap();
        let Some(s) = sessions.get_mut(&session_handle) else { return Err(CANCELLED) };
        s.app_name = name;
        s.since = now();
        s.controls = control;
        publish(&sessions);
        Ok(Results::new())
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What a shareable shows, for input in its stream's coordinates.
fn shown(s: &crate::Shareable) -> Shown {
    if s.window {
        return s.key.strip_prefix("window:").and_then(|id| id.parse().ok()).map_or(Shown::Elsewhere, Shown::Window);
    }
    if s.key.starts_with("agent:") { Shown::Elsewhere } else { Shown::Screen(s.position.0, s.position.1) }
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
        self.shared.create(session_handle, app_id, false, server).await
    }

    async fn select_sources(&self, _handle: OwnedObjectPath, session_handle: OwnedObjectPath, _app_id: String, options: HashMap<String, OwnedValue>) -> (u32, Results) {
        let mut sessions = self.shared.sessions.lock().unwrap();
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
        match self.shared.start(handle, session_handle, parent_window, 0, server).await {
            Ok(results) => (OK, results),
            Err(response) => (response, Results::new()),
        }
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
            if let Some(gone) = sessions.remove(&self.path) {
                crate::remote::ended(self.path.as_str(), &gone);
            }
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
    let shared = Shared { sessions: Arc::default(), to_ui: Arc::new(to_ui), last: Arc::default() };
    // Nothing is shared yet (a file a crashed portal left goes).
    publish(&HashMap::new());
    let to_ui = shared.to_ui.clone();
    let access = crate::access::Access { to_ui: to_ui.clone() };
    let account = crate::account::Account { to_ui: to_ui.clone() };
    let files = crate::files::FileChooser { to_ui: to_ui.clone() };
    let chooser = crate::chooser::AppChooser { to_ui: to_ui.clone() };
    let wallpaper = crate::wallpaper::Wallpaper { to_ui: to_ui.clone() };
    let shortcuts = crate::shortcuts::GlobalShortcuts::new(to_ui.clone());
    let print = crate::print::Print { to_ui: to_ui.clone(), tokens: Arc::default(), next: Default::default() };
    let remote = crate::remote::RemoteDesktop::new(shared.clone());
    let conn = builder
        .serve_at(PATH, ScreenCast { shared })?
        .serve_at(PATH, remote)?
        .serve_at(PATH, crate::inhibit::Inhibit)?
        .serve_at(PATH, crate::shot::Screenshot)?
        .serve_at(PATH, access)?
        .serve_at(PATH, account)?
        .serve_at(PATH, files)?
        .serve_at(PATH, chooser)?
        .serve_at(PATH, crate::background::Background)?
        .serve_at(PATH, crate::email::Email)?
        .serve_at(PATH, wallpaper)?
        .serve_at(PATH, shortcuts)?
        .serve_at(PATH, print)?
        .build()?;
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
        let sessions = cast.get().shared.sessions.clone();
        let mut sessions = sessions.lock().unwrap();
        if let Some(gone) = sessions.remove(&OwnedObjectPath::from(path.clone())) {
            crate::remote::ended(session, &gone);
        }
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
