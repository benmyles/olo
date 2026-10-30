//! The GlobalShortcuts portal (`org.freedesktop.impl.portal.GlobalShortcuts`):
//! an app's keys that work whatever has the keyboard (Discord's
//! push-to-talk, a browser extension's command, a recorder's start/stop).
//! The first time an app binds them, Olo asks (a BAlert listing each
//! shortcut and its keys; ones Olo or another app already has are shown
//! as unavailable); what was allowed is remembered per app
//! (`$XDG_CONFIG_HOME/olo/global-shortcuts.json`) so the same keys don't
//! ask again. A session holds a connection to app_server that took the
//! keys (`GrabKeys`); their presses and releases come back on it and go
//! to the app as Activated and Deactivated; closing the session lets them
//! go.

use std::{
    collections::HashMap,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};

use olo_kit::{
    Cx, WindowId, WindowOptions,
    egui::{self, pos2},
    metrics as m,
    style::{self, c},
    widgets,
};
use olo_protocol::shortcuts::Chord;
use serde::{Deserialize, Serialize};
use zbus::{
    object_server::SignalEmitter,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

use crate::portal::ToUi;

const PATH: &str = "/org/freedesktop/portal/desktop";
const WIDTH: u32 = 440;
const ICON: &str = "keyboard";
const COLUMN: f32 = WIDTH as f32 - widgets::ALERT_TEXT_X - m::WINDOW_INSET;

/// One of an app's shortcuts: its id, what it does, the keys it wants
/// (as a [`Chord`] is written; None: none suggested or not readable).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wanted {
    pub id: String,
    pub description: String,
    pub keys: Option<String>,
}

/// An XDG shortcut trigger ("CTRL+SHIFT+a", "LOGO+F12": xkb's modifier
/// and keysym names) as a [`Chord`] is written ("Ctrl+Shift+A").
pub fn trigger(s: &str) -> Option<String> {
    let mut parts: Vec<&str> = s.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    let key = parts.pop()?;
    let mut mods = String::new();
    for p in parts {
        mods.push_str(match p.to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" | "PRIMARY" => "Ctrl+",
            "ALT" => "Alt+",
            "SHIFT" => "Shift+",
            "LOGO" | "SUPER" | "META" => "Cmd+",
            // Num Lock isn't a modifier here.
            "NUM" => "",
            _ => return None,
        });
    }
    let named = match key {
        "BackSpace" => "Backspace".to_owned(),
        "Prior" | "Page_Up" => "PageUp".into(),
        "Next" | "Page_Down" => "PageDown".into(),
        "KP_Enter" => "KeypadEnter".into(),
        "bracketleft" => "BracketLeft".into(),
        "bracketright" => "BracketRight".into(),
        k => {
            let mut c = k.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    };
    let written = format!("{mods}{named}");
    written.parse::<Chord>().ok().map(|c| c.to_string())
}

/// What each app was allowed, by app id: its shortcuts with their keys.
type Allowed = HashMap<String, Vec<Wanted>>;

fn allowed_path() -> Option<std::path::PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(config.join("olo").join("global-shortcuts.json"))
}

fn load_allowed() -> Allowed {
    allowed_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_allowed(a: &Allowed) {
    let Some(p) = allowed_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(a) {
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// The app_server connection holding a session's keys: made again when
/// app_server restarts (an upgrade), until the session closes.
struct Grab {
    closed: std::sync::atomic::AtomicBool,
    stream: Mutex<Option<UnixStream>>,
}

impl Grab {
    /// Let the keys go, for good.
    fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(s) = self.stream.lock().unwrap().take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A session: its app, its shortcuts as bound, and the keys it holds.
struct Session {
    app: String,
    bound: Vec<Wanted>,
    grab: Option<Arc<Grab>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(g) = self.grab.take() {
            g.close();
        }
    }
}

type Sessions = Arc<Mutex<HashMap<String, Session>>>;

/// What the dialog was asked.
pub struct Ask {
    pub request: String,
    pub parent: String,
    pub app: String,
    /// Each shortcut, and whether its keys are free to take.
    pub rows: Vec<(Wanted, bool)>,
    pub reply: async_channel::Sender<bool>,
}

pub struct GlobalShortcuts {
    pub to_ui: Arc<dyn Fn(ToUi) + Send + Sync>,
    sessions: Sessions,
}

impl GlobalShortcuts {
    pub fn new(to_ui: Arc<dyn Fn(ToUi) + Send + Sync>) -> Self {
        GlobalShortcuts { to_ui, sessions: Arc::default() }
    }
}

/// The shortcuts as the portal gives them back: each id with its
/// description and how its keys read ("⌃⌥K"; "" for none).
fn listed(bound: &[Wanted]) -> Vec<(String, HashMap<String, OwnedValue>)> {
    bound
        .iter()
        .map(|w| {
            let keys = w.keys.as_deref().and_then(|k| k.parse::<Chord>().ok()).map(|c| c.symbols()).unwrap_or_default();
            let props = HashMap::from([
                ("description".to_owned(), OwnedValue::try_from(Value::from(w.description.clone())).expect("a string")),
                ("trigger_description".to_owned(), OwnedValue::try_from(Value::from(keys)).expect("a string")),
            ]);
            (w.id.clone(), props)
        })
        .collect()
}

fn results(bound: &[Wanted]) -> HashMap<String, OwnedValue> {
    HashMap::from([("shortcuts".to_owned(), OwnedValue::try_from(Value::from(listed(bound))).expect("no file descriptors"))])
}

/// Take `bound`'s keys from app_server for `session`, passing their
/// presses on as signals over `conn`; the keys refused lose them. If
/// app_server goes (an upgrade), they are taken again from the next one.
fn grab(conn: zbus::Connection, session: String, bound: &mut [Wanted]) -> Option<Arc<Grab>> {
    use olo_protocol::{Client, Event, Reply, Request};
    let keys: Vec<(String, String)> = bound.iter().filter_map(|w| Some((w.id.clone(), w.keys.clone()?))).collect();
    let take = |keys: &[(String, String)]| -> Option<(Client, Vec<String>)> {
        let mut client = Client::connect().ok()?;
        match client.request(&Request::GrabKeys { keys: keys.to_vec() }) {
            Ok(Reply::KeysGrabbed { refused }) => Some((client, refused)),
            _ => None,
        }
    };
    let (mut client, refused) = take(&keys)?;
    for w in bound.iter_mut().filter(|w| refused.contains(&w.id)) {
        w.keys = None;
    }
    let keys: Vec<(String, String)> = keys.into_iter().filter(|(id, _)| !refused.contains(id)).collect();
    let held = Arc::new(Grab { closed: Default::default(), stream: Mutex::new(client.closer().ok()) });
    let grab = held.clone();
    let _ = std::thread::Builder::new().name("portal-shortcuts".into()).spawn(move || {
        let Ok(path) = zbus::zvariant::ObjectPath::try_from(session.as_str()) else { return };
        loop {
            let reply = match client.read() {
                Ok(r) => r,
                Err(_) if grab.closed() => return,
                Err(_) => {
                    // app_server went: take them again from the next one.
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        if grab.closed() {
                            return;
                        }
                        if let Some((c, _)) = take(&keys) {
                            *grab.stream.lock().unwrap() = c.closer().ok();
                            client = c;
                            tracing::info!("portal: shortcuts taken again for {session}");
                            break;
                        }
                    }
                    continue;
                }
            };
            let Reply::Event { event: Event::KeyGrab { id, pressed } } = reply else { continue };
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            let Ok(emitter) = SignalEmitter::new(&conn, PATH) else { continue };
            let options: HashMap<String, OwnedValue> = HashMap::new();
            let r = if pressed {
                zbus::block_on(GlobalShortcuts::activated(&emitter, path.clone().into(), &id, now, options))
            } else {
                zbus::block_on(GlobalShortcuts::deactivated(&emitter, path.clone().into(), &id, now, options))
            };
            if let Err(e) = r {
                tracing::debug!("portal: shortcut {id}: {e}");
            }
        }
    });
    Some(held)
}

#[zbus::interface(name = "org.freedesktop.impl.portal.GlobalShortcuts")]
impl GlobalShortcuts {
    async fn create_session(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let key = session_handle.to_string();
        self.sessions.lock().unwrap().insert(key.clone(), Session { app: app_id, bound: Vec::new(), grab: None });
        let _ = server.at(session_handle.as_ref(), SessionObject { path: key.clone(), sessions: self.sessions.clone() }).await;
        (0, HashMap::from([("session_handle".to_owned(), OwnedValue::try_from(Value::from(key)).expect("a string"))]))
    }

    #[allow(clippy::too_many_arguments)]
    async fn bind_shortcuts(
        &self,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
        parent_window: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let key = session_handle.to_string();
        // Bound again: the old keys go first (or they'd be "in use").
        let (app, old) = match self.sessions.lock().unwrap().get_mut(&key) {
            Some(s) => (s.app.clone(), s.grab.take()),
            None => return (2, HashMap::new()),
        };
        if let Some(old) = old {
            old.close();
        }
        let get = |props: &HashMap<String, OwnedValue>, k: &str| props.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
        let wanted: Vec<Wanted> = shortcuts
            .iter()
            .map(|(id, props)| Wanted { id: id.clone(), description: get(props, "description"), keys: trigger(&get(props, "preferred_trigger")) })
            .collect();
        // Allowed before, as they are now: no need to ask again.
        let mut allowed = blocking::unblock(load_allowed).await;
        let known = !app.is_empty() && allowed.get(&app).is_some_and(|a| *a == wanted);
        if !known {
            // Which keys are free: those app_server and other apps don't
            // have (asked by trying them on a connection of our own).
            let probe = wanted.clone();
            let free: Vec<bool> = blocking::unblock(move || {
                use olo_protocol::{Client, Reply, Request};
                let keys: Vec<(String, String)> = probe.iter().filter_map(|w| Some((w.id.clone(), w.keys.clone()?))).collect();
                let refused = match Client::connect().and_then(|mut c| c.request(&Request::GrabKeys { keys })) {
                    Ok(Reply::KeysGrabbed { refused }) => refused,
                    _ => probe.iter().map(|w| w.id.clone()).collect(),
                };
                probe.iter().map(|w| w.keys.is_some() && !refused.contains(&w.id)).collect()
            })
            .await;
            let (tx, rx) = async_channel::bounded(1);
            let request = crate::portal::RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
            let _ = server.at(handle.as_ref(), request).await;
            let name = crate::app_name(&app);
            (self.to_ui)(ToUi::Shortcuts(Ask { request: handle.to_string(), parent: parent_window, app: name, rows: wanted.iter().cloned().zip(free).collect(), reply: tx }));
            let yes = rx.recv().await.unwrap_or(false);
            let _ = server.remove::<crate::portal::RequestObject, _>(handle.as_ref()).await;
            if !yes {
                return (1, HashMap::new());
            }
            if !app.is_empty() {
                allowed.insert(app.clone(), wanted.clone());
                blocking::unblock(move || save_allowed(&allowed)).await;
            }
        }
        let (conn, session) = (conn.clone(), key.clone());
        let mut bound = wanted;
        let (bound, stream) = blocking::unblock(move || {
            let stream = grab(conn, session, &mut bound);
            (bound, stream)
        })
        .await;
        let mut sessions = self.sessions.lock().unwrap();
        let Some(s) = sessions.get_mut(&key) else { return (2, HashMap::new()) };
        s.grab = stream;
        s.bound = bound;
        (0, results(&s.bound))
    }

    async fn list_shortcuts(&self, _handle: OwnedObjectPath, session_handle: OwnedObjectPath) -> (u32, HashMap<String, OwnedValue>) {
        match self.sessions.lock().unwrap().get(session_handle.as_str()) {
            Some(s) => (0, results(&s.bound)),
            None => (2, HashMap::new()),
        }
    }

    #[zbus(signal)]
    async fn activated(emitter: &SignalEmitter<'_>, session_handle: OwnedObjectPath, shortcut_id: &str, timestamp: u64, options: HashMap<String, OwnedValue>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn deactivated(emitter: &SignalEmitter<'_>, session_handle: OwnedObjectPath, shortcut_id: &str, timestamp: u64, options: HashMap<String, OwnedValue>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

/// `org.freedesktop.impl.portal.Session`: an app's shortcuts, until it
/// closes them.
struct SessionObject {
    path: String,
    sessions: Sessions,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Session")]
impl SessionObject {
    async fn close(&self, #[zbus(object_server)] server: &zbus::ObjectServer) {
        // (Dropping it lets its keys go.)
        self.sessions.lock().unwrap().remove(&self.path);
        let _ = server.remove::<SessionObject, _>(self.path.as_str()).await;
    }

    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

/// The question on screen.
pub struct Dialog {
    ask: Ask,
}

fn body(ask: &Ask) -> String {
    let who = if ask.app.is_empty() { "An app".to_owned() } else { format!("“{}”", ask.app) };
    format!("{who} would like these keys to work whatever you're using:")
}

fn height(ask: &Ask) -> u32 {
    let text = m::LABEL_ROW + m::TIGHT + style::plain_text_height(&body(ask), COLUMN) + m::SPACING;
    let rows = ask.rows.len() as f32 * m::LABEL_ROW;
    (2.0 * m::WINDOW_INSET + text + rows + m::SPACING + m::CONTROL_HEIGHT + m::WINDOW_INSET).ceil() as u32
}

impl Dialog {
    pub fn open(cx: &mut Cx, ask: Ask) -> (WindowId, Dialog) {
        let size = (WIDTH, height(&ask));
        let mut options = WindowOptions { size, min_size: Some(size), max_size: Some(size), ..WindowOptions::alert("Keyboard Shortcuts", "") };
        if !ask.parent.is_empty() {
            options = options.foreign_parent(ask.parent.clone());
        }
        let id = cx.open_window(options);
        cx.activate(id);
        (id, Dialog { ask })
    }

    pub fn request(&self) -> &str {
        &self.ask.request
    }

    pub fn answer(&self, yes: bool) {
        let _ = self.ask.reply.try_send(yes);
    }

    /// Draw it; Some(allowed) once answered.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<bool> {
        let p = style::palette(ui.ctx());
        let col = widgets::alert_frame(ui, ICON);
        let mut y = col.top();
        style::label(ui.painter(), col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, egui::Align::Min, "Allow Keyboard Shortcuts?", style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;
        y = widgets::alert_text(ui, col, y, &body(&self.ask)) + m::SPACING;
        // Each shortcut: what it does, and its keys at the right (or why
        // it has none).
        for (w, free) in &self.ask.rows {
            let mid = y + m::LABEL_ROW / 2.0;
            let keys = match (&w.keys, free) {
                (Some(k), true) => k.parse::<Chord>().map(|c| c.symbols()).unwrap_or_default(),
                (Some(_), false) => "Keys in use".to_owned(),
                (None, _) => "No keys".to_owned(),
            };
            let keys_color = if w.keys.is_some() && *free { c(p.panel_text) } else { style::disabled_text(&p) };
            let keys_w = ui.painter().layout_no_wrap(keys.clone(), style::plain_font(), keys_color).size().x;
            let what = if w.description.is_empty() { w.id.as_str() } else { w.description.as_str() };
            style::label_fit(ui.painter(), col.left(), mid, egui::Align::Min, what, style::plain_font(), c(p.panel_text), (col.width() - keys_w - m::SPACING).max(0.0));
            style::label(ui.painter(), col.right(), mid, egui::Align::Max, &keys, style::plain_font(), keys_color);
            style::describe_text(ui.ctx(), egui::accesskit::Role::Label, &format!("{what}: {keys}"), egui::Rect::from_min_max(pos2(col.left(), y), pos2(col.right(), y + m::LABEL_ROW)), None);
            y += m::LABEL_ROW;
        }
        widgets::alert_buttons(ui, col, &["Don't Allow", "Allow"], true).map(|i| i == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers() {
        assert_eq!(trigger("CTRL+SHIFT+a").as_deref(), Some("Ctrl+Shift+A"));
        assert_eq!(trigger("LOGO+F12").as_deref(), Some("Cmd+F12"));
        assert_eq!(trigger("ALT+Page_Up").as_deref(), Some("Alt+PageUp"));
        assert_eq!(trigger("CTRL+ALT+space").as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(trigger("").as_deref(), None);
        assert_eq!(trigger("HYPER+x").as_deref(), None);
    }
}
