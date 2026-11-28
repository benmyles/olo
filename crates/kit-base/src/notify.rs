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
//! large), `.style(NoteStyle::Message)` and `.size(CardSize::Large)`;
//! what it is about, `.file(path)` (its picture dragged out is that
//! file); and what each of its actions does once this app has gone,
//! `.runs(key, args)`, for a program that doesn't stay (Screenshot's
//! saved picture: Open, Copy, Delete from the history an hour on).

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
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
    /// What its actions do once it's no longer open at this app
    /// ([`Note::runs`]): (key, arguments).
    pub runs: Vec<(String, Vec<String>)>,
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

    /// What action `key` does, run by the centre whatever this app is
    /// doing: arguments for this app's own program, which the
    /// notifications centre starts with them when the action is chosen,
    /// the app open or not (sent to an app about to end, an action went
    /// unheard), and the note then closed here ([`NoteEvent::Closed`])
    /// but for a resident one; once the app has gone its button stays.
    /// Only for an action that needs nothing of the running app (a saved
    /// file's Copy or Delete): one that does is an ordinary action, and
    /// a click is [`Note::shows`]. The program started is to make sure of
    /// what it's asked to act on (any program may send a note that says
    /// it's this app's).
    pub fn runs(mut self, key: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Note {
        let key = key.into();
        self.runs.retain(|(k, _)| *k != key);
        self.runs.push((key, args.into_iter().map(Into::into).collect()));
        self
    }

    /// The file the note is about (a screenshot, a recording): its large
    /// picture ([`Note::image`]) pressed and dragged out of it is that
    /// file. Without one, the picture's own file is.
    pub fn file(mut self, path: impl AsRef<std::path::Path>) -> Note {
        self.look.file = Some(path.as_ref().to_string_lossy().into_owned());
        self
    }

    /// Action `key` stands apart from the rest, at the start of their
    /// row, as a BAlert's leftmost button with `B_OFFSET_SPACING`: a
    /// Delete away from the Copy a person reaches for.
    pub fn apart(mut self, key: impl Into<String>) -> Note {
        self.look.apart = Some(key.into());
        self
    }

    /// How long what its large picture shows plays (a recording's), said
    /// on the picture as a player would ("▶ 0:04").
    pub fn length(mut self, seconds: f64) -> Note {
        self.look.length = Some(seconds.max(0.0).round().min(f64::from(u32::MAX)) as u32);
        self
    }

    /// The file it's about ([`Note::file`]) as this program wrote it
    /// (`x-olo-file-id`): the service takes the file only if it's still
    /// that one.
    pub fn file_as(mut self, path: impl AsRef<std::path::Path>, id: olo_protocol::notifications::FileId) -> Note {
        self.look.file = Some(path.as_ref().to_string_lossy().into_owned());
        self.look.file_id = Some(id);
        self
    }

    /// Its file and picture shown, never handed on (`x-olo-shown-only`):
    /// one this program can't vouch for.
    pub fn shown_only(mut self) -> Note {
        self.look.shown_only = true;
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
    /// Told once what was asked before it has been sent ([`Notifier::sync`]):
    /// whether all of it was, since the last time asked.
    Sync(mpsc::Sender<bool>),
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

    /// Waits until what was asked before this has been sent to the
    /// notifications service (`within` at most): a program about to end,
    /// whose last note would go with it unsent. Whether all of it was
    /// (since the last `sync`): false with no bus to send it on, or a
    /// service that refused it.
    pub fn sync(&self, within: Duration) -> bool {
        let (done, sent) = mpsc::channel();
        self.to.send(Ask::Sync(done)).is_ok() && sent.recv_timeout(within).unwrap_or(false)
    }
}

/// Takes `desktop_entry`'s banners off the screen now, as if their time
/// had run out (they stay in the list as they were, unread, their
/// actions as they were), and returns once the screen no longer shows
/// them, `within` at most: whether they were (true with none there).
/// Whatever is about to take a picture of the screen, so that a banner
/// of its own isn't in it (Screenshot's, of the picture before, as each
/// new one comes). Only Olo's notifications do this; elsewhere nothing
/// is asked of them, and other apps' banners stay where they are (a
/// picture of one may be what's wanted). On a thread of its own: a
/// service that doesn't answer holds up nothing past `within`.
pub fn put_away(desktop_entry: &str, within: Duration) -> bool {
    put_away_via(session_stream, desktop_entry, within)
}

/// [`put_away`], asked over the socket `connect` opens.
fn put_away_via(connect: impl FnOnce() -> std::io::Result<std::os::unix::net::UnixStream> + Send + 'static, desktop_entry: &str, within: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    // (Whether the caller has given up, and a hold on the socket.)
    let cut: Arc<Mutex<(bool, Option<std::os::unix::net::UnixStream>)>> = Arc::default();
    let entry = desktop_entry.to_owned();
    let theirs = cut.clone();
    let spawned = std::thread::Builder::new().name("notify-put-away".into()).spawn(move || {
        // The bus's socket opened here, and a hold on it left for the
        // caller: the whole of it (the bus's handshake, which comes
        // before any call's time runs, as well as the call) is cut off
        // when the caller gives up, and the thread and its socket go then
        // too, never waiting on a bus that took the socket and said nothing.
        let asked = connect().and_then(|s| {
            let mut cut = theirs.lock().unwrap();
            if cut.0 {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "given up on"));
            }
            cut.1 = Some(s.try_clone()?);
            Ok(s)
        });
        let asked = asked.map_err(zbus::Error::from).and_then(|s| zbus::blocking::connection::Builder::async_io_unix_stream(s).method_timeout(within).build()).and_then(|c| {
            let r = c.call_method(Some(NAME), olo_protocol::notifications::CONTROL_PATH, Some(olo_protocol::notifications::CONTROL), "PutAway", &(entry.as_str(),)).map(|_| ());
            let _ = c.close();
            r
        });
        if let Err(e) = &asked {
            tracing::debug!("notifications: their banners not put away: {e}");
        }
        let _ = tx.send(asked.is_ok());
    });
    let answered = spawned.is_ok() && rx.recv_timeout(within).unwrap_or(false);
    // (Answered or given up on, done with: whatever it's still doing ends.)
    let mut cut = cut.lock().unwrap();
    cut.0 = true;
    if let Some(socket) = cut.1.take() {
        let _ = socket.shutdown(std::net::Shutdown::Both);
    }
    answered
}

/// The session bus's socket, connected: a path's, or an abstract one's,
/// as its address says (the only kinds Olo's session has).
fn session_stream() -> std::io::Result<std::os::unix::net::UnixStream> {
    use zbus::address::transport::{Transport, UnixSocket};
    let unsupported = || std::io::Error::new(std::io::ErrorKind::Unsupported, "the session bus isn't at a unix socket");
    let address = zbus::Address::session().map_err(|_| unsupported())?;
    let Transport::Unix(unix) = address.transport() else { return Err(unsupported()) };
    match unix.path() {
        UnixSocket::File(path) => std::os::unix::net::UnixStream::connect(path),
        UnixSocket::Abstract(name) => {
            use std::os::linux::net::SocketAddrExt as _;
            std::os::unix::net::UnixStream::connect_addr(&std::os::unix::net::SocketAddr::from_abstract_name(name.as_encoded_bytes())?)
        }
        _ => Err(unsupported()),
    }
}

/// Ours ↔ the service's ids, both ways; and who the service is.
#[derive(Default)]
struct Ids {
    server: HashMap<NoteId, u32>,
    ours: HashMap<u32, NoteId>,
    tokens: HashMap<u32, String>,
    /// The service's connection (its unique name): the one that answered
    /// the last note sent. Only its signals are its: anyone on the bus
    /// may send one that says it's an action on a note of ours.
    service: Option<String>,
}

impl Ids {
    /// `service` answered a note just sent: if it's another than before
    /// (the service started again), the ids of the one before are no
    /// longer anything's.
    fn answered_by(&mut self, service: Option<String>) {
        if service != self.service {
            self.server.clear();
            self.ours.clear();
            self.tokens.clear();
            self.service = service;
        }
    }

    /// Whether a signal from `sender` is the service's.
    fn is_service(&self, sender: Option<&str>) -> bool {
        sender.is_some() && sender == self.service.as_deref()
    }
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
    for (key, text) in [(hint::SUBTITLE, &look.subtitle), (hint::BADGE, &look.badge), (hint::IMAGE, &look.image), (hint::SOURCE, &look.source), (hint::FILE, &look.file), (hint::APART, &look.apart)] {
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
    if let Some(s) = look.length {
        hints.insert(hint::LENGTH, Value::from(s));
    }
    if let Some(id) = look.file_id.filter(|_| look.file.is_some()) {
        hints.insert(hint::FILE_ID, Value::from(id.to_string()));
    }
    if look.shown_only {
        hints.insert(hint::SHOWN_ONLY, Value::from(true));
    }
    if !note.shows.is_empty() {
        hints.insert(hint::SHOW, Value::from(note.shows.clone()));
    }
    if !note.runs.is_empty() {
        let runs: HashMap<String, Vec<String>> = note.runs.iter().cloned().collect();
        hints.insert(hint::RUNS, Value::from(runs));
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
    serve(from, proxy, app, entry, connect);
}

/// [`run`], connecting with `connect` (a test's: none).
fn serve(from: mpsc::Receiver<Ask>, proxy: Events, app: String, entry: String, connect: impl Fn(&Arc<Mutex<Ids>>, &Events) -> Option<zbus::blocking::Connection>) {
    let ids = Arc::new(Mutex::new(Ids::default()));
    let mut conn: Option<zbus::blocking::Connection> = None;
    let icon = crate::apps::OUR_APPS.iter().find(|(bin, _, _)| *bin == entry).map(|(_, _, icon)| icon.to_string()).unwrap_or_default();
    // Whether something asked since the last `sync` wasn't sent.
    let mut unsent = false;
    for ask in from {
        // (Everything before it sent, or not sendable: said either way,
        // the asker's wait ending.)
        if let Ask::Sync(done) = ask {
            let _ = done.send(!std::mem::take(&mut unsent));
            continue;
        }
        if conn.is_none() {
            conn = connect(&ids, &proxy);
        }
        let Some(c) = &conn else {
            unsent = true;
            continue;
        };
        let r = match ask {
            Ask::Sync(_) => Ok(()),
            Ask::Show(id, note) => {
                // An update goes to the service that has the note, by its
                // own connection: one started again since knows nothing of
                // its number (another app's note may have it now). Gone, the
                // note is sent anew, to whichever service there is.
                let (replaces, service) = known(&ids, id);
                let (body, actions, hints, timeout) = args(&note, &entry);
                let note_icon = note.icon.clone().unwrap_or_else(|| icon.clone());
                let notify = |to: &str, replaces: u32| {
                    c.call_method(Some(to), PATH, Some(NAME), "Notify", &(app.as_str(), replaces, note_icon.as_str(), note.summary.as_str(), body.as_str(), actions.clone(), hints.clone(), timeout))
                        .and_then(|r| Ok((r.header().sender().map(|s| s.to_string()), r.body().deserialize::<u32>()?)))
                };
                let sent = match &service {
                    Some(to) if replaces != 0 => match notify(to, replaces) {
                        Err(e) if no_such_peer(&e) => {
                            ids.lock().unwrap().answered_by(None);
                            notify(NAME, 0)
                        }
                        other => other,
                    },
                    _ => notify(NAME, 0),
                };
                sent.map(|(service, server)| {
                    let mut ids = ids.lock().unwrap();
                    ids.answered_by(service);
                    ids.server.insert(id, server);
                    ids.ours.insert(server, id);
                })
            }
            // (The map let go before the call: the signals' thread uses it.
            // To the service that has the note: another's numbers aren't
            // this note's; gone, there's nothing to close.)
            Ask::Close(id) => match known(&ids, id) {
                (server, Some(to)) if server != 0 => forget_if_gone(&ids, c.call_method(Some(to.as_str()), PATH, Some(NAME), "CloseNotification", &(server,)).map(|_| ())),
                _ => Ok(()),
            },
            Ask::Seen(id) => match known(&ids, id) {
                (server, Some(to)) if server != 0 => {
                    // (Read first: closed, it's no longer open at its app,
                    // but stays in the history, and must be read there.)
                    let _ = c.call_method(Some(to.as_str()), olo_protocol::notifications::CONTROL_PATH, Some(olo_protocol::notifications::CONTROL), "SetRead", &(server, true));
                    forget_if_gone(&ids, c.call_method(Some(to.as_str()), PATH, Some(NAME), "CloseNotification", &(server,)).map(|_| ()))
                }
                _ => Ok(()),
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
            unsent = true;
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
                // (Unique names are a bus's own, for its life: another bus,
                // or this one again, may give the service's name to another,
                // and this sender is another sender to it. Its numbers and
                // tokens go with the connection, before another listens.)
                forget_connection(&ids);
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

/// The connection dropped (lost): every number, token and the service's
/// name it knew forgotten, so that a note is sent anew (never by a number
/// another note may have now, nor marked read on another service).
fn forget_connection(ids: &Arc<Mutex<Ids>>) {
    let mut ids = ids.lock().unwrap();
    ids.server.clear();
    ids.ours.clear();
    ids.tokens.clear();
    ids.service = None;
}

/// Note `id`'s number at the service, and that service's connection (0
/// and None: not sent, or forgotten).
fn known(ids: &Arc<Mutex<Ids>>, id: NoteId) -> (u32, Option<String>) {
    let ids = ids.lock().unwrap();
    (ids.server.get(&id).copied().unwrap_or(0), ids.service.clone())
}

/// Whether `error` says the connection a call was addressed to isn't on
/// the bus (the service that had a note has gone: started again, or
/// stopped).
fn no_such_peer(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::FDO(e) => matches!(**e, zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_)),
        zbus::Error::MethodError(name, ..) => matches!(name.as_str(), "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner"),
        _ => false,
    }
}

/// `r`, a call to the service that had a note; if it has gone, its
/// numbers are forgotten (they're nothing's now) and that's no error.
fn forget_if_gone(ids: &Arc<Mutex<Ids>>, r: zbus::Result<()>) -> zbus::Result<()> {
    match r {
        Err(e) if no_such_peer(&e) => {
            ids.lock().unwrap().answered_by(None);
            Ok(())
        }
        other => other,
    }
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
            // (Only the service's: another's that says a Delete was
            // chosen is nobody's choice.)
            if !ids.is_service(header.sender().map(|s| s.as_str())) {
                continue;
            }
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
        // The service that had a note gone from the bus (started again): its numbers are forgotten, and the
        // note is sent anew to whichever service there is, never by a number another note may have now.
        assert!(no_such_peer(&zbus::Error::FDO(Box::new(zbus::fdo::Error::ServiceUnknown(":1.42".into())))));
        assert!(no_such_peer(&zbus::Error::FDO(Box::new(zbus::fdo::Error::NameHasNoOwner(":1.42".into())))));
        assert!(!no_such_peer(&zbus::Error::Failure("refused".into())));
        let ids = Arc::new(Mutex::new(Ids::default()));
        {
            let mut i = ids.lock().unwrap();
            i.answered_by(Some(":1.42".into()));
            i.server.insert(NoteId(3), 7);
            i.ours.insert(7, NoteId(3));
        }
        assert_eq!(known(&ids, NoteId(3)), (7, Some(":1.42".to_owned())));
        assert!(forget_if_gone(&ids, Err(zbus::Error::FDO(Box::new(zbus::fdo::Error::ServiceUnknown(":1.42".into()))))).is_ok());
        assert_eq!(known(&ids, NoteId(3)), (0, None));
        // The connection itself lost: everything it knew forgotten with it, its tokens too (a new bus may give
        // the same unique name to another service).
        {
            let mut i = ids.lock().unwrap();
            i.answered_by(Some(":1.7".into()));
            i.server.insert(NoteId(4), 9);
            i.ours.insert(9, NoteId(4));
            i.tokens.insert(9, "token".into());
        }
        forget_connection(&ids);
        let i = ids.lock().unwrap();
        assert!(i.server.is_empty() && i.ours.is_empty() && i.tokens.is_empty() && i.service.is_none());
    }

    /// A put-away given up on is given up on whole: a bus that takes the socket and then says nothing (in its
    /// handshake, before any call's time runs) keeps neither the caller past its time nor the thread and its
    /// socket after (they piled up, one a capture, while it said nothing).
    #[test]
    fn a_put_away_given_up_on_keeps_nothing() {
        use std::io::Read as _;
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-put-away-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bus");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let started = std::time::Instant::now();
        let to = path.clone();
        assert!(!put_away_via(move || std::os::unix::net::UnixStream::connect(&to), "olo-screenshot", Duration::from_millis(200)));
        assert!(started.elapsed() < Duration::from_secs(2), "the caller waited {:?}", started.elapsed());
        // The bus's end: what the client said (its handshake's start), then the end of it, the socket let go of.
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut said = Vec::new();
        let read = s.read_to_end(&mut said);
        assert!(read.is_ok(), "the socket was kept open: {read:?}");
        assert!(said.starts_with(b"\0AUTH"), "{said:?}");
        let _ = std::fs::remove_dir_all(&dir);
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

    /// A signal is the service's only from the service (the connection that answered the last note sent): one
    /// from another connection saying a Delete was chosen on a note of ours is nobody's choice. Another
    /// service answering (it started again), the ids of the one before are no longer anything's.
    #[test]
    fn only_the_services_signals_are_heard() {
        use zbus::blocking::connection::Builder;
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let server = std::thread::spawn(move || Builder::async_io_unix_stream(a).server(guid).unwrap().p2p().build().unwrap());
        let client = Builder::async_io_unix_stream(b).p2p().build().unwrap();
        let server = server.join().unwrap();
        let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(NAME).unwrap().path(PATH).unwrap().build();
        let signals = zbus::blocking::MessageIterator::for_match_rule(rule, &client, Some(64)).unwrap();
        let ids = Arc::new(Mutex::new(Ids::default()));
        {
            let mut ids = ids.lock().unwrap();
            ids.answered_by(Some(":1.5".into()));
            ids.server.insert(NoteId(1), 7);
            ids.ours.insert(7, NoteId(1));
        }
        let (tx, rx) = mpsc::channel();
        let events: Events = Arc::new(move |e| {
            let _ = tx.send(e);
        });
        let _listener = listen(signals, &ids, &events).unwrap();
        let send = |sender: &str, key: &str| {
            let m = zbus::Message::signal(PATH, NAME, "ActionInvoked").unwrap().sender(sender).unwrap().build(&(7u32, key)).unwrap();
            server.send(&m).unwrap();
        };
        send(":1.9", "delete");
        send(":1.5", "copy");
        let heard = rx.recv_timeout(Duration::from_secs(5)).expect("the service's action wasn't heard");
        assert!(matches!(&heard, NoteEvent::Action { id: NoteId(1), key, .. } if key == "copy"), "another's was heard first");
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "another's was heard");
        ids.lock().unwrap().answered_by(Some(":1.6".into()));
        assert!(ids.lock().unwrap().ours.is_empty() && ids.lock().unwrap().server.is_empty());
        client.close().unwrap();
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
        assert!(!hints.contains_key(hint::RUNS) && !hints.contains_key(hint::FILE));
        // What each action does once the app has gone (the last said for a key is what's kept), and the file
        // it's about.
        let saved = Note::new("screenshot4.png")
            .image("/p/screenshot4.png")
            .file("/p/screenshot4.png")
            .action("copy", "Copy")
            .runs("copy", ["--saved", "copy"])
            .runs("copy", ["--saved", "copy", "/p/screenshot4.png"])
            .action("delete", "Delete")
            .runs("delete", ["--saved", "delete", "/p/screenshot4.png"]);
        let (_, actions, hints, _) = args(&saved, "olo-screenshot");
        assert_eq!(actions, ["copy", "Copy", "delete", "Delete"]);
        assert_eq!(hints[hint::FILE], zbus::zvariant::Value::from("/p/screenshot4.png"));
        let runs: HashMap<String, Vec<String>> = HashMap::try_from(hints[hint::RUNS].try_clone().unwrap()).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs["copy"], ["--saved", "copy", "/p/screenshot4.png"]);
        assert_eq!(runs["delete"], ["--saved", "delete", "/p/screenshot4.png"]);
    }

    /// `sync` is answered by the notifier's thread in its turn, without a connection made for it (a test
    /// asks nothing else of it, so no bus is reached: nothing goes to a desktop's notifications), and a
    /// notifier whose thread has gone doesn't wait.
    #[test]
    fn sync_is_answered_in_its_turn() {
        let (to, from) = mpsc::channel();
        let events: Events = Arc::new(|_| {});
        let thread = std::thread::spawn(move || run(from, events, "Screenshot".into(), "olo-screenshot".into()));
        let notes = Notifier { to, next: Arc::new(AtomicU64::new(1)) };
        let started = std::time::Instant::now();
        assert!(notes.sync(Duration::from_secs(5)), "sync said nothing");
        assert!(notes.sync(Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(1));
        let (to, from) = mpsc::channel::<Ask>();
        drop(from);
        assert!(!Notifier { to, next: Arc::new(AtomicU64::new(1)) }.sync(Duration::from_secs(5)));
        drop(notes);
        thread.join().unwrap();
    }

    /// A note that couldn't be sent (no bus to send it on: a test's, which reaches none) is said by the next
    /// `sync`, and only by it.
    #[test]
    fn sync_says_a_note_went_unsent() {
        let (to, from) = mpsc::channel();
        let events: Events = Arc::new(|_| {});
        let thread = std::thread::spawn(move || serve(from, events, "Screenshot".into(), "olo-screenshot".into(), |_, _| None));
        let notes = Notifier { to, next: Arc::new(AtomicU64::new(1)) };
        notes.show(None, Note::new("screenshot1.png"));
        assert!(!notes.sync(Duration::from_secs(5)), "an unsent note was said sent");
        assert!(notes.sync(Duration::from_secs(5)), "nothing asked since");
        drop(notes);
        thread.join().unwrap();
    }
}
