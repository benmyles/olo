//! olo-slack's Chrome, joined to Olo: the host's side (the extension's is
//! `slack-page.js` and `slack.js` in the Slack page, and background.js's
//! "olo-slack" part). Only a host started by olo-slack's Chrome does this:
//! olo-slack gives Chrome `OLO_SLACK_URL` (Slack's client, or a page
//! standing in for it) and `OLO_SLACK_PROFILE` (its user data folder),
//! which reach us as Chrome starts us; we take them out of our own
//! environment at once, so nothing we start (a link's browser) inherits
//! them.
//!
//! As the host starts it tells the extension `{slack: {url, open}}` (open:
//! a link olo-slack was started with, left in the profile folder for us,
//! taken once). Then, from the extension:
//!
//! * `{slack: {badge: N}}`: Slack's unread count, for its Deskbar entry
//!   (a `com.canonical.Unity.LauncherEntry` update for olo-slack).
//! * `{slack: {note: {n, title, body, tag, icon, silent}}}` and
//!   `{slack: {unnote: n}}`: the page's notifications as Olo's (the
//!   page's `Notification`, answered by slack-page.js); a click on one
//!   brings Slack forward and is the page's `click` (`{slack: {clicked:
//!   n}}` back), its going the page's `close` (`{slack: {closed: n}}`).
//! * `{slack: {open: URL}}`: a link out of Slack, opened with Olo's
//!   default app for it.
//! * `{slack: {picture: {name, data}}}`: a picture (base64) to show in
//!   Olo's Viewer.
//! * `{slack: {menus: k, items}}`: the answer to our `{slack: {menus:
//!   k}}`, Slack's commands for the palette (its channels and people, Jump
//!   To…, Search…), asked as the palette opens; choosing one sends `{slack:
//!   {go: COMMAND}}`.
//!
//! And from us, as Olo's state changes: `{slack: {status: {away, dnd,
//! dnd_until, call}}}`, which Slack's status follows (slack-page.js):
//! away while the screen is locked (app_server's power state), Do Not
//! Disturb with Olo's (Notifications' state, and until when its settings
//! say), in a meeting during a call (an app recording from a microphone
//! for 20 s, until none has for 10: `olo_system::audio::watch_listening`).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use olo_kit_base::{
    commands::Entry,
    notify::{Note, NoteEvent, NoteId, NoteKind, Notifier},
};
use olo_sync::Mutex;
use serde_json::{Value, json};

/// Slack's colour (aubergine), its notifications' edge.
const ACCENT: [u8; 3] = [0x4a, 0x15, 0x4b];

/// How long the extension has to list Slack's commands (the palette is
/// opening: it asks with 400 ms to spare).
const MENUS_WAIT: Duration = Duration::from_millis(250);

/// Pictures and senders' faces kept in the cache this long.
const KEEP_FOR: Duration = Duration::from_secs(30 * 24 * 3600);

/// The largest picture taken for the Viewer.
pub const MAX_PICTURE: usize = 40 << 20;

/// Our app id and desktop entry (olo-slack's).
const APP_ID: &str = "olo.slack";
const ENTRY: &str = "olo-slack";

/// What olo-slack told Chrome, if it was olo-slack that started it.
#[derive(Debug, Clone, Default)]
pub struct Here {
    /// Slack's client's address.
    pub url: String,
    /// Its Chrome's user data folder.
    pub profile: Option<PathBuf>,
}

/// Whether this Chrome is olo-slack's: what it was told, taken out of our
/// environment. Call first thing, before any thread starts.
pub fn take_env() -> Option<Here> {
    let url = std::env::var("OLO_SLACK_URL").ok().filter(|u| u.starts_with("http"));
    let profile = std::env::var_os("OLO_SLACK_PROFILE").map(PathBuf::from).filter(|p| p.is_absolute());
    for key in ["OLO_SLACK_URL", "OLO_SLACK_PROFILE"] {
        // SAFETY: called before the host starts any thread.
        unsafe { std::env::remove_var(key) };
    }
    Some(Here { url: url?, profile })
}

/// The file olo-slack leaves a link to open in, in Slack's profile folder.
pub fn open_file(profile: &Path) -> PathBuf {
    profile.join("Olo Slack Link")
}

/// Who writes to Chrome (the extension), shared with the host.
pub type Out = Arc<Mutex<std::io::Stdout>>;

/// The page's notifications shown: theirs ↔ ours, and by tag.
#[derive(Default)]
struct Shown {
    ours: HashMap<u64, NoteId>,
    theirs: HashMap<NoteId, u64>,
    tags: HashMap<String, NoteId>,
}

pub struct Slack {
    here: Here,
    out: Out,
    notes: Notifier,
    shown: Arc<Mutex<Shown>>,
    /// The badge Deskbar was last given.
    badge: Option<u32>,
    /// Palette questions waiting for the extension, by number.
    asking: Arc<Mutex<HashMap<u64, mpsc::Sender<Vec<Entry>>>>>,
    next: Arc<AtomicU64>,
}

impl Slack {
    pub fn new(here: Here, out: Out) -> Slack {
        let shown: Arc<Mutex<Shown>> = Arc::default();
        let (about, to) = (shown.clone(), out.clone());
        let notes = Notifier::with_events("Slack", ENTRY, move |e| match e {
            NoteEvent::Action { id, .. } => {
                let n = about.lock().unwrap_or_else(|e| e.into_inner()).theirs.get(&id).copied();
                // Slack forward (the page's own `window.focus()` can't), then
                // the page's click: it goes to the conversation.
                activate();
                if let Some(n) = n {
                    crate::host::send(&to, &json!({ "slack": { "clicked": n } }));
                }
            }
            NoteEvent::Closed { id } => {
                let mut s = about.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(n) = s.theirs.remove(&id) {
                    s.ours.remove(&n);
                    s.tags.retain(|_, v| *v != id);
                    crate::host::send(&to, &json!({ "slack": { "closed": n } }));
                }
            }
            NoteEvent::Replied { .. } => {}
        });
        std::thread::Builder::new().name("slack-cache".into()).spawn(|| tidy(&cache())).ok();
        follow_olo(out.clone());
        Slack { here, out, notes, shown, badge: None, asking: Arc::default(), next: Arc::new(AtomicU64::new(1)) }
    }

    /// Tell the extension it is olo-slack's, with the link to open if
    /// olo-slack left one.
    pub fn hello(&self) {
        let open = self.here.profile.as_deref().map(open_file).and_then(|f| {
            let link = std::fs::read_to_string(&f).ok();
            let _ = std::fs::remove_file(&f);
            link.map(|l| l.trim().to_owned()).filter(|l| l.starts_with("http"))
        });
        crate::host::send(&self.out, &json!({ "slack": { "url": self.here.url, "open": open } }));
    }

    /// A `{slack: …}` message from the extension.
    pub fn message(&mut self, m: &Value) {
        if let Some(n) = m.get("badge") {
            let count = n.as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
            if self.badge != Some(count) {
                self.badge = Some(count);
                self.notes.badge(count);
            }
        } else if let Some(note) = m.get("note") {
            self.note(note);
        } else if let Some(n) = m.get("unnote").and_then(Value::as_u64) {
            let mut s = self.shown.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(id) = s.ours.remove(&n) {
                s.theirs.remove(&id);
                s.tags.retain(|_, v| *v != id);
                self.notes.close(id);
            }
        } else if let Some(url) = m.get("open").and_then(Value::as_str) {
            open_link(url);
        } else if let Some(p) = m.get("picture") {
            let (name, data) = (p.get("name").and_then(Value::as_str).unwrap_or("picture"), p.get("data").and_then(Value::as_str).unwrap_or(""));
            show_picture(name, data);
        } else if let Some(k) = m.get("menus").and_then(Value::as_u64) {
            let items = m.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(entry).collect()).unwrap_or_default();
            if let Some(tx) = self.asking.lock().unwrap_or_else(|e| e.into_inner()).remove(&k) {
                let _ = tx.send(items);
            }
        }
    }

    fn note(&self, m: &Value) {
        let text = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let Some(n) = m.get("n").and_then(Value::as_u64) else { return };
        let (title, body, tag) = (text("title"), text("body"), text("tag"));
        let mut note = Note::new(if title.is_empty() { "Slack".to_owned() } else { title })
            .body(body)
            .accent(ACCENT)
            .style(olo_protocol::NoteStyle::Message)
            .kind(NoteKind::Information)
            .action("default", "Show");
        // (Slack plays its own sound unless the person turned it off.)
        note.silent = true;
        if let Some(icon) = m.get("icon").and_then(Value::as_str).and_then(|d| face(d, &text("icon_url"))) {
            note = note.icon(icon.display().to_string());
        }
        let mut s = self.shown.lock().unwrap_or_else(|e| e.into_inner());
        // The same tag: the one before, updated in place.
        let before = (!tag.is_empty()).then(|| s.tags.get(&tag).copied()).flatten();
        if let Some(old) = before
            && let Some(n_old) = s.theirs.remove(&old)
        {
            s.ours.remove(&n_old);
        }
        let id = self.notes.show(before, note);
        s.ours.insert(n, id);
        s.theirs.insert(id, n);
        if !tag.is_empty() {
            s.tags.insert(tag, id);
        }
    }

    /// Slack's commands for the palette: asked of the extension, which
    /// reads them from the page (nothing is kept or watched between
    /// questions).
    pub fn menus(&self) -> impl Fn() -> Vec<Entry> + Send + 'static {
        let (out, asking, next) = (self.out.clone(), self.asking.clone(), self.next.clone());
        move || {
            let k = next.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = mpsc::channel();
            asking.lock().unwrap_or_else(|e| e.into_inner()).insert(k, tx);
            crate::host::send(&out, &json!({ "slack": { "menus": k } }));
            let got = rx.recv_timeout(MENUS_WAIT).unwrap_or_default();
            asking.lock().unwrap_or_else(|e| e.into_inner()).remove(&k);
            got
        }
    }

    /// Do one of [`Slack::menus`]' commands (or olo-slack's `open URL`).
    pub fn invoke(&self) -> impl Fn(&str) -> bool + Send + 'static {
        let out = self.out.clone();
        move |command| {
            let ok = command == "jump" || command == "search" || command.strip_prefix("open ").is_some_and(|u| u.starts_with("http"));
            if ok {
                crate::host::send(&out, &json!({ "slack": { "go": command } }));
            }
            ok
        }
    }
}

/// What Slack's status follows, as it changes.
enum Change {
    Locked(bool),
    Quiet {
        on: bool,
        until: Option<u64>,
    },
    /// The apps recording from a microphone (None: no sound server).
    Listening(Option<Vec<String>>),
}

/// A call is an app recording this long (a voice clip isn't), and is
/// over when none has for this long.
const CALL_STARTS: Duration = Duration::from_secs(20);
const CALL_ENDS: Duration = Duration::from_secs(10);

/// Olo's state, followed (each part on a thread of its own, woken by its
/// source: app_server's events, the kernel's for Notifications' state
/// file, the sound server's), and told the extension when it changes.
fn follow_olo(out: Out) {
    let (tx, rx) = mpsc::channel::<Change>();
    let locked = tx.clone();
    let spawned = std::thread::Builder::new().name("slack-lock".into()).spawn(move || {
        use olo_protocol::{Client, Event, EventKind, Reply, Request};
        loop {
            if let Ok(mut c) = Client::connect() {
                if let Ok(Reply::Power { status }) = c.request(&Request::PowerStatus)
                    && locked.send(Change::Locked(status.locked)).is_err()
                {
                    return;
                }
                if let Ok(events) = c.into_events_of(&[EventKind::Power]) {
                    for e in events {
                        match e {
                            Ok(Event::PowerChanged { status }) => {
                                if locked.send(Change::Locked(status.locked)).is_err() {
                                    return;
                                }
                            }
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                }
            }
            // app_server restarting (an upgrade): its next one.
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    let quiet = tx.clone();
    let watched = std::thread::Builder::new().name("slack-quiet".into()).spawn(move || {
        use olo_protocol::notifications::{NotificationSettings, NotificationState};
        // Quiet now (by hand or by its schedule), and until when if it
        // was turned on for a while.
        let read = move || {
            let on = NotificationState::load().is_some_and(|s| s.quiet);
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let settings = NotificationSettings::load();
            let until = settings.dnd_until.filter(|&t| on && settings.dnd && t > now);
            Change::Quiet { on, until }
        };
        if quiet.send(read()).is_err() {
            return;
        }
        let again = quiet.clone();
        let _watch = olo_kit_base::watch::watch([NotificationState::path(), NotificationSettings::path()], move |_| {
            let _ = again.send(read());
        });
        // (The watch lives as long as this thread: the host's.)
        loop {
            std::thread::park();
        }
    });
    let listening = tx;
    let heard = std::thread::Builder::new().name("slack-call".into()).spawn(move || {
        olo_system::audio::watch_listening(move |apps| {
            let _ = listening.send(Change::Listening(apps));
        })
    });
    if let Err(e) = spawned.and(watched).and(heard.map(|_| ())) {
        tracing::warn!("olo-slack: Slack's status won't follow Olo's: {e}");
    }
    let told = std::thread::Builder::new().name("slack-status".into()).spawn(move || {
        status_of(&rx, CALL_STARTS, CALL_ENDS, |status| crate::host::send(&out, &json!({ "slack": { "status": status } })));
    });
    if let Err(e) = told {
        tracing::warn!("olo-slack: Slack's status won't follow Olo's: {e}");
    }
}

/// Olo's state from its changes, `tell` given it each time it changes
/// (`{away, dnd, dnd_until, call}`), until the changes end. A call is
/// recording for `starts`, over after none for `ends`.
fn status_of(rx: &mpsc::Receiver<Change>, starts: Duration, ends: Duration, mut tell: impl FnMut(Value)) {
    use std::time::Instant;
    let (mut locked, mut quiet, mut until) = (false, false, None);
    // Recording since, and quiet since (a call starts and ends).
    let (mut recording, mut silent): (Option<Instant>, Option<Instant>) = (None, None);
    let mut call = false;
    let mut said: Option<Value> = None;
    loop {
        // Waiting for a change, or for a call to start or end.
        let wait = match (recording, silent) {
            (Some(t), _) if !call => Some(starts.saturating_sub(t.elapsed())),
            (_, Some(t)) if call => Some(ends.saturating_sub(t.elapsed())),
            _ => None,
        };
        let change = match wait {
            Some(w) => match rx.recv_timeout(w) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            },
        };
        match change {
            Some(Change::Locked(l)) => locked = l,
            Some(Change::Quiet { on, until: u }) => (quiet, until) = (on, u),
            Some(Change::Listening(apps)) => {
                if apps.is_some_and(|a| !a.is_empty()) {
                    recording.get_or_insert_with(Instant::now);
                    silent = None;
                } else {
                    recording = None;
                    if call {
                        silent.get_or_insert_with(Instant::now);
                    }
                }
            }
            None => {}
        }
        if recording.is_some_and(|t| t.elapsed() >= starts) {
            call = true;
        }
        if silent.is_some_and(|t| t.elapsed() >= ends) {
            call = false;
            silent = None;
        }
        let status = json!({ "away": locked, "dnd": quiet, "dnd_until": until, "call": call });
        if said.as_ref() != Some(&status) {
            tell(status.clone());
            said = Some(status);
        }
    }
}

/// A palette entry from the extension's `{path, label, command}`.
fn entry(v: &Value) -> Option<Entry> {
    let label = v.get("label")?.as_str()?.trim();
    let command = v.get("command")?.as_str()?;
    if label.is_empty() || !(command == "jump" || command == "search" || command.starts_with("open http")) {
        return None;
    }
    let path = v.get("path").and_then(Value::as_array).map(|p| p.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    Some(Entry { path, label: label.chars().take(120).collect(), command: command.to_owned(), keys: None, enabled: true, checked: false })
}

/// Slack's window forward (ours: its app id is `olo.slack`, its process
/// the Chrome that started us).
fn activate() {
    use olo_protocol::{Client, Reply, Request};
    let chrome = std::os::unix::process::parent_id() as i32;
    let Ok(mut c) = Client::connect() else { return };
    let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
    if let Some(w) = windows.iter().find(|w| w.app_id == APP_ID && w.pid == Some(chrome)).or_else(|| windows.iter().find(|w| w.app_id == APP_ID)) {
        let _ = c.request(&Request::Activate { id: w.id });
    }
}

/// A link out of Slack, with Olo's default app for it (the person's
/// browser for a web page), on a thread (it may start one).
fn open_link(url: &str) {
    let ok = ["https://", "http://", "mailto:", "tel:"].iter().any(|p| url.starts_with(p));
    if !ok {
        return;
    }
    let url = url.to_owned();
    std::thread::spawn(move || {
        if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Url(url.clone())) {
            eprintln!("olo-chrome: couldn't open {url}: {e}");
        }
    });
}

fn cache() -> PathBuf {
    olo_xdg::cache_dir().join("slack")
}

/// A name safe for a file, from `name` (a picture's, from its address).
fn file_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).take(100).collect();
    let clean = clean.trim_start_matches('.').to_owned();
    if clean.is_empty() { "picture".into() } else { clean }
}

fn decode(data: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

/// A sender's face (a notification's icon), from its bytes (`data`,
/// base64) and where they came from: kept in the cache by address, as
/// the same faces come again and again.
fn face(data: &str, url: &str) -> Option<PathBuf> {
    use sha2::Digest as _;
    let bytes = decode(data).filter(|b| !b.is_empty() && b.len() < 1 << 20)?;
    let key = if url.is_empty() { sha2::Sha256::digest(&bytes) } else { sha2::Sha256::digest(url.as_bytes()) };
    let hex: String = key.iter().take(12).map(|b| format!("{b:02x}")).collect();
    let dir = cache().join("faces");
    let path = dir.join(hex);
    if std::fs::read(&path).ok().as_deref() != Some(&bytes[..]) {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, &bytes).ok()?;
    }
    Some(path)
}

/// A picture from Slack in Olo's Viewer: written to the cache (as Slack
/// named it), then opened.
fn show_picture(name: &str, data: &str) {
    let Some(bytes) = decode(data).filter(|b| !b.is_empty() && b.len() <= MAX_PICTURE) else { return };
    let dir = cache().join("pictures");
    let path = dir.join(file_name(name));
    if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &bytes)).is_err() {
        eprintln!("olo-chrome: couldn't keep a picture from Slack in {}", dir.display());
        return;
    }
    std::thread::spawn(move || {
        let r = match olo_xdg::find_program("olo-viewer") {
            Some(viewer) => olo_xdg::open::spawn(&[viewer.into_os_string(), path.clone().into_os_string()]),
            None => olo_xdg::open::open(&olo_xdg::open::Target::Path(path.clone())),
        };
        if let Err(e) = r {
            eprintln!("olo-chrome: couldn't show {}: {e}", path.display());
        }
    });
}

/// Remove what the cache has kept longer than [`KEEP_FOR`].
fn tidy(dir: &Path) {
    let now = std::time::SystemTime::now();
    for sub in ["faces", "pictures"] {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else { continue };
        for e in entries.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > KEEP_FOR);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_entries_are_only_slacks_commands() {
        let e = entry(&json!({ "path": ["Go"], "label": "#general", "command": "open https://app.slack.com/client/T1/C1" })).unwrap();
        assert_eq!((e.path, e.label.as_str()), (vec!["Go".to_owned()], "#general"));
        assert!(entry(&json!({ "label": "Search…", "command": "search" })).is_some());
        // Nothing else passes (the page can't have the palette run things).
        assert!(entry(&json!({ "label": "x", "command": "run:rm" })).is_none());
        assert!(entry(&json!({ "label": "x", "command": "open file:///etc/passwd" })).is_none());
        assert!(entry(&json!({ "label": " ", "command": "jump" })).is_none());
    }

    #[test]
    fn picture_names_stay_in_their_folder() {
        assert_eq!(file_name("../../.bashrc"), "_.._.bashrc");
        assert_eq!(file_name("..."), "picture");
        assert_eq!(file_name("image 720.png"), "image_720.png");
        assert_eq!(file_name(""), "picture");
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    /// Locking, Do Not Disturb and a call each say so once; a short
    /// recording (a voice clip) isn't a call; a call ends after the
    /// quiet that ends it, not at a moment's pause.
    #[test]
    fn slack_s_status_follows_olo_s() {
        let (tx, rx) = mpsc::channel();
        let (said_tx, said) = mpsc::channel();
        let ms = Duration::from_millis;
        let t = std::thread::spawn(move || status_of(&rx, ms(200), ms(150), |v| said_tx.send(v).unwrap()));
        let next = || said.recv_timeout(ms(1000)).expect("a status");
        let none = || assert!(said.recv_timeout(ms(80)).is_err(), "nothing more");
        let mic = |on: bool| Change::Listening(Some(if on { vec!["Google Chrome".to_owned()] } else { Vec::new() }));

        tx.send(Change::Locked(false)).unwrap();
        assert_eq!(next(), json!({ "away": false, "dnd": false, "dnd_until": null, "call": false }));
        tx.send(Change::Locked(true)).unwrap();
        assert_eq!(next()["away"], true);
        tx.send(Change::Locked(true)).unwrap();
        none();
        tx.send(Change::Quiet { on: true, until: Some(2000) }).unwrap();
        let s = next();
        assert_eq!((s["dnd"].clone(), s["dnd_until"].clone()), (json!(true), json!(2000)));
        // A voice clip: recorded for less than a call takes.
        tx.send(mic(true)).unwrap();
        std::thread::sleep(ms(100));
        tx.send(mic(false)).unwrap();
        none();
        // A call: recording on.
        tx.send(mic(true)).unwrap();
        let s = next();
        assert_eq!(s["call"], true);
        // A moment's pause doesn't end it; a long one does.
        tx.send(mic(false)).unwrap();
        std::thread::sleep(ms(50));
        tx.send(mic(true)).unwrap();
        none();
        tx.send(mic(false)).unwrap();
        assert_eq!(next()["call"], false);
        drop(tx);
        t.join().unwrap();
    }
}
