//! Desktop notifications for Olo's own apps, through the session's
//! notification service (org.freedesktop.Notifications; Olo's is
//! apps/notifications). The bus is talked to on a thread of its own, so a
//! slow or missing service never holds up a frame. A [`Notifier`] hands
//! out [`NoteId`]s; showing again with the same id updates that
//! notification in place (a progress note as a job goes on). What the
//! user does with one comes back to `App::message`
//! as a [`NoteEvent`].
//!
//! ```ignore
//! let notes = Notifier::new(cx.proxy(), "Tracker", "olo-tracker");
//! let id = notes.show(None, Note::progress("Copying 3 items", 0.4));
//! notes.show(Some(id), Note::new("Copied 3 items").action("show", "Show"));
//! // later, in App::message: NoteEvent::Action { id, key: "show", .. }
//! ```
//!
//! A note can say how it should look (the `x-olo-*` hints of
//! [`olo_protocol::notifications::hint`]): `.accent(rgb)`, `.subtitle()`,
//! `.badge()`, `.field(label, value)`, `.image(path)` (a picture shown
//! large), `.style(NoteStyle::Message)` and `.size(CardSize::Large)`.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

use olo_protocol::{CardSize, NoteLook, NoteStyle, notifications::hint};

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
    /// Wants attention (a message for you): its own look, and a sound.
    Important,
    /// Something went wrong: a stop sign beside its title, a sound; it
    /// stays until dismissed, and is sent as urgent (it comes through
    /// Do Not Disturb).
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
    /// Can be answered with text from the banner (KDE's `inline-reply`
    /// action), this its field's placeholder.
    pub reply: Option<String>,
    /// Not kept in the history (a progress note that will be replaced).
    pub transient: bool,
    /// No sound.
    pub silent: bool,
    /// How it looks, beyond its kind.
    pub look: NoteLook,
    /// What shows the thing it's about ([`Note::shows`]).
    pub shows: Vec<String>,
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

    /// What shows the thing this note is about, for as long as the note
    /// is in the history: arguments for this app's own program, which
    /// the notifications centre starts with them when the note is
    /// clicked after it closed (as things moved on, or after either
    /// program started again), where it could otherwise only bring the
    /// app forward. Open still, its `default` action is run as ever:
    /// give both, doing the same.
    pub fn shows(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Note {
        self.shows = args.into_iter().map(Into::into).collect();
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

    /// Answerable with text from the banner itself (a chat message): a
    /// reply field whose placeholder is `placeholder` ("Reply to Ann").
    /// The answer comes back as [`NoteEvent::Replied`].
    pub fn reply(mut self, placeholder: impl Into<String>) -> Note {
        self.reply = Some(placeholder.into());
        self
    }

    /// The app's colour, for the card's band and badge.
    pub fn accent(mut self, rgb: [u8; 3]) -> Note {
        self.look.accent = Some(rgb);
        self
    }

    /// A line under the title.
    pub fn subtitle(mut self, text: impl Into<String>) -> Note {
        self.look.subtitle = Some(text.into());
        self
    }

    /// A short tag beside the title.
    pub fn badge(mut self, text: impl Into<String>) -> Note {
        self.look.badge = Some(text.into());
        self
    }

    /// A row of its little table of facts.
    pub fn field(mut self, label: impl Into<String>, value: impl Into<String>) -> Note {
        self.look.fields.push((label.into(), value.into()));
        self
    }

    /// A picture to show large (a screenshot, a photo).
    pub fn image(mut self, path: impl AsRef<std::path::Path>) -> Note {
        self.look.image = Some(path.as_ref().to_string_lossy().into_owned());
        self
    }

    pub fn style(mut self, style: NoteStyle) -> Note {
        self.look.style = style;
        self
    }

    /// How big a card it wants in the history.
    pub fn size(mut self, size: CardSize) -> Note {
        self.look.size = size;
        self
    }
}

/// What came back, as an app message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteEvent {
    /// An action was chosen (`"default"`: the notification was clicked),
    /// with the activation token to raise a window with, when there is one.
    Action { id: NoteId, key: String, token: Option<String> },
    /// It was answered from its reply field ([`Note::reply`]).
    Replied { id: NoteId, text: String },
    /// It went (expired, dismissed or closed).
    Closed { id: NoteId },
}

enum Ask {
    Show(NoteId, Box<Note>),
    Close(NoteId),
    Seen(NoteId),
    /// The count, and what waits where it's said (None: a count alone).
    Badge(u32, Option<Vec<olo_protocol::notifications::Waits>>),
}

/// Shows an app's notifications. Cheap to clone; one thread per notifier.
#[derive(Clone)]
pub struct Notifier {
    to: mpsc::Sender<Ask>,
    next: Arc<AtomicU64>,
}

impl Notifier {
    /// `app` is the name shown, `desktop_entry` the app's id
    /// (`olo-tracker`), for its icon and for bringing it forward.
    pub fn new(proxy: Proxy, app: &str, desktop_entry: &str) -> Notifier {
        Notifier::with_events(app, desktop_entry, move |e| proxy.send(e))
    }

    /// For a program without windows (a helper the window server
    /// starts): what the user does comes to `events`, on a thread of the
    /// notifier's.
    pub fn with_events(app: &str, desktop_entry: &str, events: impl Fn(NoteEvent) + Send + Sync + 'static) -> Notifier {
        let (to, from) = mpsc::channel();
        let (app, entry) = (app.to_owned(), desktop_entry.to_owned());
        let events: Events = Arc::new(events);
        let _ = std::thread::Builder::new().name("notify".into()).spawn(move || run(from, events, app, entry));
        Notifier { to, next: Arc::new(AtomicU64::new(1)) }
    }

    /// Show `note`, or update the one `id` names; its id.
    pub fn show(&self, id: Option<NoteId>, note: Note) -> NoteId {
        let id = id.unwrap_or_else(|| NoteId(self.next.fetch_add(1, Ordering::Relaxed)));
        let _ = self.to.send(Ask::Show(id, Box::new(note)));
        id
    }

    pub fn close(&self, id: NoteId) {
        let _ = self.to.send(Ask::Close(id));
    }

    /// The person has seen what `id` is about, in the app itself (the
    /// tab it names is the one showing, in the window in front): it's
    /// read, and leaves the screen. An app says this for a notification
    /// about one place within it; one about the app as a whole is read
    /// by its window being gone to, with nothing said. (Where the
    /// notifications aren't Olo's, it only closes.)
    pub fn seen(&self, id: NoteId) {
        let _ = self.to.send(Ask::Seen(id));
    }

    /// How many things are unread in the app (0: none): Deskbar lights
    /// its row's dot (`com.canonical.Unity.LauncherEntry`'s count, for
    /// the app's desktop entry, as chat apps send it).
    pub fn badge(&self, count: u32) {
        let _ = self.to.send(Ask::Badge(count, None));
    }

    /// What waits in the app, each thing with where it is and how it's
    /// shown (`Waits`; empty: nothing): its count on its Deskbar row, and
    /// each listed in Notifications' "Waiting in apps" and Deskbar's
    /// Inbox, where a click goes to it (its `shows`, which the app takes
    /// as it takes a notification's).
    pub fn waiting(&self, waits: Vec<olo_protocol::notifications::Waits>) {
        let _ = self.to.send(Ask::Badge(u32::try_from(waits.len()).unwrap_or(u32::MAX), Some(waits)));
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
    hints.insert(hint::TYPE, Value::from(kind));
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
    let look = &note.look;
    if let Some([r, g, b]) = look.accent {
        hints.insert(hint::ACCENT, Value::from(format!("#{r:02x}{g:02x}{b:02x}")));
    }
    for (key, text) in [(hint::SUBTITLE, &look.subtitle), (hint::BADGE, &look.badge), (hint::IMAGE, &look.image), (hint::SOURCE, &look.source)] {
        if let Some(t) = text {
            hints.insert(key, Value::from(t.clone()));
        }
    }
    if !look.fields.is_empty() {
        hints.insert(hint::FIELDS, Value::from(look.fields.clone()));
    }
    if look.style != NoteStyle::Standard {
        hints.insert(hint::STYLE, Value::from(look.style.name()));
    }
    if look.size != CardSize::Auto {
        hints.insert(hint::SIZE, Value::from(look.size.name()));
    }
    if !note.shows.is_empty() {
        hints.insert(hint::SHOW, Value::from(note.shows.clone()));
    }
    let mut actions: Vec<String> = note.actions.iter().flat_map(|(k, l)| [k.clone(), l.clone()]).collect();
    if let Some(placeholder) = &note.reply {
        actions.extend(["inline-reply".to_owned(), "Reply".to_owned()]);
        hints.insert("x-kde-reply-placeholder-text", Value::from(placeholder.clone()));
    }
    (escape(&note.body), actions, hints, if note.sticky || note.kind == NoteKind::Error { 0 } else { -1 })
}

type Events = Arc<dyn Fn(NoteEvent) + Send + Sync>;

fn run(from: mpsc::Receiver<Ask>, proxy: Events, app: String, entry: String) {
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
            // (The map let go before the call: the signals' thread uses it.)
            Ask::Close(id) => match { ids.lock().unwrap().server.get(&id).copied() } {
                Some(server) => c.call_method(Some(NAME), PATH, Some(NAME), "CloseNotification", &(server,)).map(|_| ()),
                None => Ok(()),
            },
            Ask::Seen(id) => match { ids.lock().unwrap().server.get(&id).copied() } {
                Some(server) => {
                    // (Read first: closed, it's no longer open at its app,
                    // but stays in the history, and must be read there.)
                    let _ = c.call_method(Some(NAME), olo_protocol::notifications::CONTROL_PATH, Some(olo_protocol::notifications::CONTROL), "SetRead", &(server, true));
                    c.call_method(Some(NAME), PATH, Some(NAME), "CloseNotification", &(server,)).map(|_| ())
                }
                None => Ok(()),
            },
            Ask::Badge(count, waits) => {
                use zbus::zvariant::Value;
                let mut props: HashMap<&str, Value> = HashMap::new();
                props.insert("count", Value::from(count as i64));
                props.insert("count-visible", Value::from(count > 0));
                if let Some(w) = waits {
                    props.insert(olo_protocol::notifications::WAITS_PROPERTY, Value::from(serde_json::to_string(&w).unwrap_or_default()));
                }
                let uri = format!("application://{entry}.desktop");
                c.emit_signal(None::<&str>, "/com/canonical/unity/launcherentry/olo", "com.canonical.Unity.LauncherEntry", "Update", &(uri.as_str(), props))
            }
        };
        if let Err(e) = r {
            tracing::info!("notifications: {e}");
            // A connection that is gone is let go of whole, and another
            // made at the next ask: closed, so that its listener's
            // thread ends and its socket goes with it. (Only dropped,
            // it stayed for good, the listener holding it; and it was
            // dropped at every error, the bus's "nobody answers to that
            // name" among them: with no notification service on the
            // bus, as while it starts again in an upgrade, every notice
            // left a socket and two threads behind. 195 of each after
            // 200 notices, in a test desktop, 2026-11-21.) An error the
            // bus itself answered with is not a lost connection: the
            // same one asks again.
            if lost(&e)
                && let Some(old) = conn.take()
            {
                let _ = old.close();
            }
        }
    }
    // The notifier is gone (the last of its clones dropped): its
    // connection with it, and the listener's thread.
    if let Some(conn) = conn {
        let _ = conn.close();
    }
}

/// Whether `error` says the connection itself is gone (the bus went, the
/// socket broke), as against an answer that came over it (no such
/// service, a call refused): only the first wants another connection.
fn lost(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::InputOutput(_))
}

/// The session bus, listening for what happens to ours.
fn connect(ids: &Arc<Mutex<Ids>>, proxy: &Events) -> Option<zbus::blocking::Connection> {
    let conn = zbus::blocking::Connection::session().map_err(|e| tracing::info!("no session bus for notifications: {e}")).ok()?;
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(NAME).ok()?.path(PATH).ok()?.build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(64)).ok()?;
    listen(signals, ids, proxy)?;
    Some(conn)
}

/// A thread that tells the app what happens to its notifications, as
/// `signals` brings it; it ends when they do (the connection closed:
/// the first error is the last thing read, where skipping errors would
/// have had it spin on a closed connection).
fn listen(signals: zbus::blocking::MessageIterator, ids: &Arc<Mutex<Ids>>, proxy: &Events) -> Option<std::thread::JoinHandle<()>> {
    let (ids, proxy) = (ids.clone(), proxy.clone());
    let spawned = std::thread::Builder::new().name("notify-events".into()).spawn(move || {
        for msg in signals {
            let Ok(msg) = msg else { break };
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
                        proxy(NoteEvent::Action { id, key, token });
                    }
                }
                Some("NotificationReplied") => {
                    if let Ok((server, text)) = msg.body().deserialize::<(u32, String)>()
                        && let Some(&id) = ids.ours.get(&server)
                    {
                        proxy(NoteEvent::Replied { id, text });
                    }
                }
                Some("NotificationClosed") => {
                    if let Ok((server, _reason)) = msg.body().deserialize::<(u32, u32)>()
                        && let Some(id) = ids.ours.remove(&server)
                    {
                        // Showing it again makes a new one.
                        ids.server.remove(&id);
                        ids.tokens.remove(&server);
                        proxy(NoteEvent::Closed { id });
                    }
                }
                _ => {}
            }
        }
    });
    spawned.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a broken connection wants another: an answer from the bus
    /// (nobody has that name, the call refused) came over a working one.
    #[test]
    fn an_answer_from_the_bus_is_not_a_lost_connection() {
        assert!(lost(&zbus::Error::InputOutput(Arc::new(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))));
        assert!(!lost(&zbus::Error::FDO(Box::new(zbus::fdo::Error::ServiceUnknown("org.freedesktop.Notifications".into())))));
        assert!(!lost(&zbus::Error::Failure("refused".into())));
        assert!(!lost(&zbus::Error::InvalidReply));
    }

    /// The listener's thread ends when its connection is closed, and the
    /// connection's own thread and socket with it: a notifier that
    /// makes another connection leaves nothing of the old one. (Two ends
    /// of a socket pair speaking D-Bus to each other: no bus needed.)
    #[test]
    fn a_closed_connection_takes_its_listener_with_it() {
        use zbus::blocking::connection::Builder;
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let server = std::thread::spawn(move || Builder::async_io_unix_stream(a).server(guid).unwrap().p2p().build().unwrap());
        let client = Builder::async_io_unix_stream(b).p2p().build().unwrap();
        let _server = server.join().unwrap();
        let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(NAME).unwrap().path(PATH).unwrap().build();
        let signals = zbus::blocking::MessageIterator::for_match_rule(rule, &client, Some(64)).unwrap();
        let ids = Arc::new(Mutex::new(Ids::default()));
        let events: Events = Arc::new(|_| {});
        let listener = listen(signals, &ids, &events).unwrap();
        assert!(!listener.is_finished());
        client.close().unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !listener.is_finished() && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(listener.is_finished(), "the listener outlived its connection");
    }

    #[test]
    fn notify_arguments() {
        let n = Note::progress("Copying", 0.425);
        let (_, actions, hints, timeout) = args(&n, "olo-tracker");
        assert!(actions.is_empty() && timeout == -1);
        assert_eq!(hints["value"], zbus::zvariant::Value::from(43i32));
        assert_eq!(hints["x-olo-type"], zbus::zvariant::Value::from("progress"));
        assert_eq!(hints["transient"], zbus::zvariant::Value::from(true));
        let e = Note::new("Couldn't copy").body("a < b").kind(NoteKind::Error).action("default", "Show");
        let (body, actions, hints, timeout) = args(&e, "olo-tracker");
        assert_eq!(body, "a &lt; b");
        assert_eq!(actions, vec!["default".to_owned(), "Show".to_owned()]);
        assert_eq!((hints["urgency"].clone(), timeout), (zbus::zvariant::Value::from(2u8), 0));
        assert!(!hints.contains_key(hint::ACCENT), "no look unless asked");
        let shot = Note::new("Screenshot").accent([0x25, 0xd3, 0x66]).badge("PNG").field("Size", "1.2 MB").image("/tmp/a.png").size(CardSize::Large);
        let (_, _, hints, _) = args(&shot, "olo-screenshot");
        assert_eq!(hints[hint::ACCENT], zbus::zvariant::Value::from("#25d366"));
        assert_eq!(hints[hint::IMAGE], zbus::zvariant::Value::from("/tmp/a.png"));
        assert_eq!(hints[hint::SIZE], zbus::zvariant::Value::from("large"));
        assert_eq!(hints[hint::FIELDS], zbus::zvariant::Value::from(vec![("Size".to_owned(), "1.2 MB".to_owned())]));
        let chat = Note::new("Ann").body("lunch?").action("default", "Show").reply("Reply to Ann");
        let (_, actions, hints, _) = args(&chat, "olo-telegram");
        assert_eq!(actions, ["default", "Show", "inline-reply", "Reply"]);
        assert_eq!(hints["x-kde-reply-placeholder-text"], zbus::zvariant::Value::from("Reply to Ann"));
        // What shows the thing it's about: only when said.
        assert!(!hints.contains_key(hint::SHOW));
        let agent = Note::new("Claude Code needs your answer").action("default", "Show").shows(["--show-shell", "17"]);
        let (_, _, hints, _) = args(&agent, "olo-terminal");
        assert_eq!(hints[hint::SHOW], zbus::zvariant::Value::from(vec!["--show-shell".to_owned(), "17".to_owned()]));
    }
}
