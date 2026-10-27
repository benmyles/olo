//! Desktop notifications for Tabula's own apps, through the session's
//! notification service (org.freedesktop.Notifications; Tabula's is
//! apps/notifications). The bus is talked to on a thread of its own, so a
//! slow or missing service never holds up a frame. A [`Notifier`] hands
//! out [`NoteId`]s; showing again with the same id updates that
//! notification in place (a progress note as a job goes on). What the
//! user does with one comes back to [`App::message`](crate::App::message)
//! as a [`NoteEvent`].
//!
//! ```ignore
//! let notes = Notifier::new(cx.proxy(), "Tracker", "tabula-tracker");
//! let id = notes.show(None, Note::progress("Copying 3 items", 0.4));
//! notes.show(Some(id), Note::new("Copied 3 items").action("show", "Show"));
//! // later, in App::message: NoteEvent::Action { id, key: "show", .. }
//! ```

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

use crate::Proxy;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// One of ours (stable across updates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoteId(u64);

/// How it looks and behaves (Haiku's notification types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoteKind {
    #[default]
    Information,
    /// Wants attention: a blue edge and a sound.
    Important,
    /// Something went wrong: a red edge, a sound; stays until dismissed.
    Error,
    /// A job under way, with a bar.
    Progress,
}

/// A notification to show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Note {
    pub summary: String,
    /// Plain text (escaped for the service).
    pub body: String,
    /// An icon name; the app's own by default.
    pub icon: Option<String>,
    pub kind: NoteKind,
    /// 0–1, for progress notes.
    pub progress: Option<f32>,
    /// (key, label); "default" is a click on the notification itself.
    pub actions: Vec<(String, String)>,
    /// Stays until dismissed.
    pub sticky: bool,
    /// Not kept in the history (a progress note that will be replaced).
    pub transient: bool,
    /// No sound.
    pub silent: bool,
}

impl Note {
    pub fn new(summary: impl Into<String>) -> Note {
        Note { summary: summary.into(), ..Default::default() }
    }

    /// A job under way, `fraction` done.
    pub fn progress(summary: impl Into<String>, fraction: f32) -> Note {
        Note { summary: summary.into(), kind: NoteKind::Progress, progress: Some(fraction.clamp(0.0, 1.0)), transient: true, silent: true, ..Default::default() }
    }

    pub fn body(mut self, body: impl Into<String>) -> Note {
        self.body = body.into();
        self
    }

    pub fn icon(mut self, icon: impl Into<String>) -> Note {
        self.icon = Some(icon.into());
        self
    }

    pub fn kind(mut self, kind: NoteKind) -> Note {
        self.kind = kind;
        self
    }

    pub fn action(mut self, key: impl Into<String>, label: impl Into<String>) -> Note {
        self.actions.push((key.into(), label.into()));
        self
    }

    pub fn sticky(mut self, on: bool) -> Note {
        self.sticky = on;
        self
    }
}

/// What came back, as an app message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteEvent {
    /// An action was chosen (`"default"`: the notification was clicked),
    /// with the activation token to raise a window with, when there is one.
    Action { id: NoteId, key: String, token: Option<String> },
    /// It went (expired, dismissed or closed).
    Closed { id: NoteId },
}

enum Ask {
    Show(NoteId, Note),
    Close(NoteId),
}

/// Shows an app's notifications. Cheap to clone; one thread per notifier.
#[derive(Clone)]
pub struct Notifier {
    to: mpsc::Sender<Ask>,
    next: Arc<AtomicU64>,
}

impl Notifier {
    /// `app` is the name shown, `desktop_entry` the app's id
    /// (`tabula-tracker`), for its icon and for bringing it forward.
    pub fn new(proxy: Proxy, app: &str, desktop_entry: &str) -> Notifier {
        let (to, from) = mpsc::channel();
        let (app, entry) = (app.to_owned(), desktop_entry.to_owned());
        let _ = std::thread::Builder::new().name("notify".into()).spawn(move || run(from, proxy, app, entry));
        Notifier { to, next: Arc::new(AtomicU64::new(1)) }
    }

    /// Show `note`, or update the one `id` names; its id.
    pub fn show(&self, id: Option<NoteId>, note: Note) -> NoteId {
        let id = id.unwrap_or_else(|| NoteId(self.next.fetch_add(1, Ordering::Relaxed)));
        let _ = self.to.send(Ask::Show(id, note));
        id
    }

    pub fn close(&self, id: NoteId) {
        let _ = self.to.send(Ask::Close(id));
    }
}

/// Ours ↔ the service's ids, both ways.
#[derive(Default)]
struct Ids {
    server: HashMap<NoteId, u32>,
    ours: HashMap<u32, NoteId>,
    tokens: HashMap<u32, String>,
}

/// The service's markup is not wanted for plain text.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The Notify call's arguments for `note`.
fn args(note: &Note, entry: &str) -> (String, Vec<String>, HashMap<&'static str, zbus::zvariant::Value<'static>>, i32) {
    use zbus::zvariant::Value;
    let mut hints: HashMap<&'static str, Value<'static>> = HashMap::new();
    hints.insert("desktop-entry", Value::from(entry.to_owned()));
    let (kind, urgency) = match note.kind {
        NoteKind::Information => ("information", 1u8),
        NoteKind::Important => ("important", 1),
        NoteKind::Error => ("error", 2),
        NoteKind::Progress => ("progress", 1),
    };
    hints.insert("x-tabula-type", Value::from(kind));
    hints.insert("urgency", Value::from(urgency));
    if let Some(p) = note.progress {
        hints.insert("value", Value::from((p.clamp(0.0, 1.0) * 100.0).round() as i32));
    }
    if note.transient {
        hints.insert("transient", Value::from(true));
    }
    if note.silent {
        hints.insert("suppress-sound", Value::from(true));
    }
    let actions = note.actions.iter().flat_map(|(k, l)| [k.clone(), l.clone()]).collect();
    (escape(&note.body), actions, hints, if note.sticky || note.kind == NoteKind::Error { 0 } else { -1 })
}

fn run(from: mpsc::Receiver<Ask>, proxy: Proxy, app: String, entry: String) {
    let ids = Arc::new(Mutex::new(Ids::default()));
    let mut conn: Option<zbus::blocking::Connection> = None;
    let icon = crate::apps::OUR_APPS.iter().find(|(bin, _, _)| *bin == entry).map(|(_, _, icon)| icon.to_string()).unwrap_or_default();
    for ask in from {
        if conn.is_none() {
            conn = connect(&ids, &proxy);
        }
        let Some(c) = &conn else { continue };
        let r = match ask {
            Ask::Show(id, note) => {
                let replaces = ids.lock().unwrap().server.get(&id).copied().unwrap_or(0);
                let (body, actions, hints, timeout) = args(&note, &entry);
                let note_icon = note.icon.clone().unwrap_or_else(|| icon.clone());
                c.call_method(Some(NAME), PATH, Some(NAME), "Notify", &(app.as_str(), replaces, note_icon.as_str(), note.summary.as_str(), body.as_str(), actions, hints, timeout))
                    .and_then(|r| r.body().deserialize::<u32>())
                    .map(|server| {
                        let mut ids = ids.lock().unwrap();
                        ids.server.insert(id, server);
                        ids.ours.insert(server, id);
                    })
            }
            Ask::Close(id) => match ids.lock().unwrap().server.get(&id).copied() {
                Some(server) => c.call_method(Some(NAME), PATH, Some(NAME), "CloseNotification", &(server,)).map(|_| ()),
                None => Ok(()),
            },
        };
        if let Err(e) = r {
            tracing::info!("notifications: {e}");
            // Reconnect next time (the service or the bus went).
            conn = None;
        }
    }
}

/// The session bus, listening for what happens to ours.
fn connect(ids: &Arc<Mutex<Ids>>, proxy: &Proxy) -> Option<zbus::blocking::Connection> {
    let conn = zbus::blocking::Connection::session().map_err(|e| tracing::info!("no session bus for notifications: {e}")).ok()?;
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(NAME).ok()?.path(PATH).ok()?.build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(64)).ok()?;
    let (ids, proxy) = (ids.clone(), proxy.clone());
    let _ = std::thread::Builder::new().name("notify-events".into()).spawn(move || {
        for msg in signals.flatten() {
            let header = msg.header();
            let mut ids = ids.lock().unwrap();
            match header.member().map(|m| m.as_str()) {
                Some("ActivationToken") => {
                    if let Ok((server, token)) = msg.body().deserialize::<(u32, String)>()
                        && ids.ours.contains_key(&server)
                    {
                        ids.tokens.insert(server, token);
                    }
                }
                Some("ActionInvoked") => {
                    if let Ok((server, key)) = msg.body().deserialize::<(u32, String)>()
                        && let Some(&id) = ids.ours.get(&server)
                    {
                        let token = ids.tokens.remove(&server);
                        proxy.send(NoteEvent::Action { id, key, token });
                    }
                }
                Some("NotificationClosed") => {
                    if let Ok((server, _reason)) = msg.body().deserialize::<(u32, u32)>()
                        && let Some(id) = ids.ours.remove(&server)
                    {
                        // Showing it again makes a new one.
                        ids.server.remove(&id);
                        ids.tokens.remove(&server);
                        proxy.send(NoteEvent::Closed { id });
                    }
                }
                _ => {}
            }
        }
    });
    Some(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_arguments() {
        let n = Note::progress("Copying", 0.425);
        let (_, actions, hints, timeout) = args(&n, "tabula-tracker");
        assert!(actions.is_empty() && timeout == -1);
        assert_eq!(hints["value"], zbus::zvariant::Value::from(43i32));
        assert_eq!(hints["x-tabula-type"], zbus::zvariant::Value::from("progress"));
        assert_eq!(hints["transient"], zbus::zvariant::Value::from(true));
        let e = Note::new("Couldn't copy").body("a < b").kind(NoteKind::Error).action("default", "Show");
        let (body, actions, hints, timeout) = args(&e, "tabula-tracker");
        assert_eq!(body, "a &lt; b");
        assert_eq!(actions, vec!["default".to_owned(), "Show".to_owned()]);
        assert_eq!((hints["urgency"].clone(), timeout), (zbus::zvariant::Value::from(2u8), 0));
    }
}
