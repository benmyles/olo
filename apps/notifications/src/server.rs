//! The freedesktop notification service (org.freedesktop.Notifications,
//! spec 1.2) on the session bus: calls come in on zbus's thread and go
//! to the UI as [`In`]; the UI answers with [`Bus::closed`],
//! [`Bus::action`] (after [`Bus::activation_token`]) and [`Bus::replied`].

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use olo_protocol::{
    CardSize, NoteLook, NoteStyle,
    notifications::{FileId, hint},
};
use zbus::zvariant::OwnedValue;

use crate::markup::{self, Span};

pub const NAME: &str = "org.freedesktop.Notifications";
pub const PATH: &str = "/org/freedesktop/Notifications";
/// Ours, beside the standard one: the history and Do Not Disturb, for the
/// command line, Deskbar and agents (and what apps say has been seen).
pub use olo_protocol::notifications::{CONTROL, CONTROL_PATH};

/// How long a notification stays when its sender leaves it to us.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(6);

/// A picture sent with a notification (the `image-data` hint): RGBA.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// What kind of news it is, as Haiku's BNotification has it: each looks
/// a little different. What it *is*, not how it arrives: how pressing it
/// is, is `Note::urgent` (a call is urgent and no error; a backup that
/// failed overnight is an error and not urgent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Information,
    /// Wants attention (a message for you, a call).
    Important,
    /// Something went wrong (its sender says so: `x-olo-type` error, or
    /// a category of the spec's that ends in ".error"): a stop sign, and
    /// it stays until dismissed.
    Error,
    /// A job under way, with a bar (the `value` hint).
    Progress,
}

/// What to play when it comes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sound {
    /// The desktop's notification sound.
    Default,
    /// A freedesktop sound theme name (`message-new-instant`).
    Name(String),
    File(std::path::PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: u32,
    /// Who sent it: its connection's unique name on the bus (":1.42").
    /// Its actions reach the app only while that connection is there: an
    /// app that quit, or started again (an upgrade re-executes Olo's
    /// apps), knows nothing of it ([`In::SenderGone`]).
    pub sender: Option<String>,
    /// The generation of its sender's claim on its number (`Owners`): its
    /// close is told only while that's still the note its sender has (0:
    /// nobody's, a GTK app's, one brought back from the list).
    pub generation: u64,
    /// It replaces the one with this id (shown in its place).
    pub replaces: bool,
    pub app_name: String,
    /// The sending app's desktop entry (`org.gnome.Calendar`), when it says.
    pub desktop_entry: Option<String>,
    /// An icon name or path (ours, the theme's, or a file).
    pub icon: Option<String>,
    pub picture: Option<Picture>,
    pub summary: String,
    pub body: Vec<Span>,
    /// (key, label); "default" is clicking the notification.
    pub actions: Vec<(String, String)>,
    pub kind: Kind,
    /// Urgent (the spec's critical urgency; GTK's "urgent" priority): it
    /// stays on screen until it's dealt with, and comes through Do Not
    /// Disturb. How it arrives, not what it is: no stop sign for it.
    pub urgent: bool,
    /// 0–1, for progress.
    pub progress: Option<f32>,
    /// None: until dismissed.
    pub timeout: Option<Duration>,
    /// Keep it after an action is chosen.
    pub resident: bool,
    /// Not kept in the history once gone.
    pub transient: bool,
    /// None: silent.
    pub sound: Option<Sound>,
    pub category: Option<String>,
    /// It can be answered in place: the field's placeholder.
    pub reply: Option<String>,
    pub time: std::time::SystemTime,
    /// How it looks, beyond its kind (the `x-olo-*` hints; a website's
    /// colours).
    pub look: NoteLook,
    /// A website's: the browser it came through (`app_name` is the site).
    pub via: Option<String>,
    /// What shows the thing it's about (`x-olo-show`): arguments for its
    /// app's own program, for a click once it's no longer open there.
    pub show: Vec<String>,
    /// What its actions do once it's no longer open at its app
    /// (`x-olo-runs`): for an action's key, arguments for its app's own
    /// program (for a click, `show`).
    pub runs: Vec<(String, Vec<String>)>,
}

impl Note {
    /// What action `key` does once the note is no longer open at its app.
    pub fn runs_for(&self, key: &str) -> Option<&[String]> {
        runs_for(&self.runs, &self.show, key)
    }
}

/// What action `key` does once a note is no longer open at its app: a
/// click (`default`), what shows what it's about; another, what it said
/// that one runs.
pub fn runs_for<'a>(runs: &'a [(String, Vec<String>)], show: &'a [String], key: &str) -> Option<&'a [String]> {
    if key == "default" {
        return (!show.is_empty()).then_some(show);
    }
    runs.iter().find(|(k, _)| k == key).map(|(_, args)| args.as_slice())
}

/// The most of `x-olo-runs` kept: actions, arguments each, bytes each.
const RUNS_MOST: (usize, usize, usize) = (16, 32, 4096);

/// Maximum number of targets accepted in one `ClearMany` D-Bus request
/// (`History::KEEP` is 200): checked at bus ingress before posting to the UI
/// or allocating collections.
pub const MAX_CLEAR_MANY: usize = 512;

/// Maximum batch id length accepted by `UndoClear`.
const MAX_BATCH_ID_LEN: usize = 128;

/// What the bus asks of the UI.
#[derive(Debug)]
pub enum In {
    Show(Box<Note>),
    Close(u32),
    /// A connection that sent notifications left the bus (its unique
    /// name): what it sent can't be acted on there any more.
    SenderGone(String),
    /// Told once it has been done (so `List` reflects it).
    Control(Control, Option<Done>),
    /// An app's banners (by its desktop entry) off the screen as if their
    /// time had run out; `Done` once the screen no longer shows them.
    PutAway(String, Done),
    /// Clear matching notification targets `(id, instance)` in one batch and
    /// one Undo step (never pinned, snoozed or waiting items; when `unread_only`
    /// is true, only notifications still unread in the Inbox and matching
    /// non-zero `instance`). Replies with `(cleared_ids, batch_id, persisted)`
    /// where `persisted` resolves once both atomic file writes finish on disk.
    ClearMany {
        targets: Vec<(u32, u64)>,
        unread_only: bool,
        reply: futures_channel::oneshot::Sender<(Vec<u32>, String, Option<futures_channel::oneshot::Receiver<()>>)>,
    },
    /// Undo the ClearMany batch identified by `batch`, reconciling against
    /// current archive and history membership. Replies with `(restored_ids,
    /// persisted)`.
    UndoClear {
        batch: String,
        reply: futures_channel::oneshot::Sender<(Vec<u32>, Option<futures_channel::oneshot::Receiver<()>>)>,
    },
}

/// Sent once the UI has done what it was asked.
pub type Done = futures_channel::oneshot::Sender<()>;

/// Asked over `com.olodesktop.Notifications1` (the CLI, Deskbar, agents).
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// Run action `key` ("default": what a click does).
    Invoke(u32, String),
    Dismiss(u32),
    ClearHistory,
    ShowHistory,
    /// Do Not Disturb on (for so many minutes; 0: until turned off) or off.
    DoNotDisturb(bool, u32),
    /// Mark one read (true) or unread.
    SetRead(u32, bool),
    MarkAllRead,
    /// Pin one (it stays put: no fading, no going with Clear All) or not.
    Pin(u32, bool),
    /// Out of sight for so many minutes, then back as new.
    Snooze(u32, u32),
    /// The person is looking at a page of this host in a browser.
    VisitedSite(String),
}

/// Why a notification went (the spec's reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

/// Which connection each live notification is (its sender's unique
/// name): only that one replaces it or closes it, and only that one is
/// told what's done with it (its actions, its closing). Anyone on the bus
/// can name a number; a note is its sender's.
/// Each note's owner with the generation of its claim: a note sent again
/// with its number (replacing itself) is a new generation, and a close
/// queued for the one before (`BusSignal::Closed`) neither reaches nor
/// lets go of the new one.
#[derive(Default)]
pub(crate) struct Owners {
    by_id: HashMap<u32, (String, u64)>,
    next_gen: u64,
}

impl Owners {
    /// Whether `sender` may replace or close note `id`: only its own, now.
    /// (A number nobody owns, none sent on this run, its sender gone, or a
    /// GTK app's, is nobody's to take: the asker gets a number of its own.)
    pub(crate) fn may(&self, id: u32, sender: Option<&str>) -> bool {
        sender.is_some_and(|s| self.by_id.get(&id).is_some_and(|(owner, _)| owner == s))
    }

    /// Note `id` is `sender`'s from now, a new generation (returned; 0 for
    /// nobody's).
    pub(crate) fn claim(&mut self, id: u32, sender: Option<String>) -> u64 {
        match sender {
            Some(s) => {
                self.next_gen += 1;
                self.by_id.insert(id, (s, self.next_gen));
                self.next_gen
            }
            None => {
                self.by_id.remove(&id);
                0
            }
        }
    }

    /// The connection note `id` is told of things on, and its generation.
    pub(crate) fn of(&self, id: u32) -> Option<(&str, u64)> {
        self.by_id.get(&id).map(|(s, g)| (s.as_str(), *g))
    }

    /// Note `id`'s generation `generation` closed: let go of, if it's still that
    /// one (a newer generation stays its sender's).
    pub(crate) fn retire(&mut self, id: u32, generation: u64) {
        if self.by_id.get(&id).is_some_and(|(_, g)| *g == generation) {
            self.by_id.remove(&id);
        }
    }

    /// `sender` left the bus: its notes are nobody's.
    pub(crate) fn gone(&mut self, sender: &str) {
        self.by_id.retain(|_, (s, _)| s != sender);
    }
}

pub(crate) type SharedOwners = std::sync::Arc<olo_sync::Mutex<Owners>>;

struct Service {
    next: std::sync::Arc<AtomicU32>,
    /// The connections that have sent notifications (their unique
    /// names): only their leaving is told to the UI.
    senders: Senders,
    owners: SharedOwners,
    /// Ids by (sender, app, tag) for the hints that say "replace the last
    /// one with this tag" (volume and brightness pop-ups, a download's
    /// progress): each sender's own (two senders of one app and tag each
    /// took the other's place in turn, and a new note came each time).
    tags: olo_sync::Mutex<HashMap<(Option<String>, String, String), u32>>,
    to_ui: Box<dyn Fn(In) + Send + Sync>,
}

/// What we can do, for GetCapabilities.
pub const CAPABILITIES: &[&str] =
    &["actions", "body", "body-hyperlinks", "body-markup", "icon-static", "inline-reply", "persistence", "sound", "x-canonical-private-synchronous", "x-olo-progress", "x-olo-look"];

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Service {
    fn get_capabilities(&self) -> Vec<&'static str> {
        CAPABILITIES.to_vec()
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let tag = ["x-canonical-private-synchronous", "synchronous", "x-dunst-stack-tag"]
            .iter()
            .find_map(|k| hints.get(*k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()))
            .filter(|t| !t.is_empty());
        let sender = header.sender().map(|s| s.to_string());
        // (Followed from now, before anything else: one that leaves while
        // its note is read is heard leaving.)
        if let Some(s) = &sender {
            self.senders.lock().unwrap().insert(s.clone());
        }
        let mut owners = self.owners.lock().unwrap();
        // (Only the sender's own note, live now, is its to replace; any
        // other number, another's, one nobody owns, one a stale replace
        // names that is the next to come, gets a number of its own. Before,
        // a connection naming a number took that note over.)
        let mut id = if replaces_id != 0 && owners.may(replaces_id, sender.as_deref()) { replaces_id } else { 0 };
        let key = tag.map(|t| (sender.clone(), app_name.clone(), t));
        if id == 0
            && let Some(k) = &key
        {
            id = self.tags.lock().unwrap().get(k).copied().filter(|&t| owners.may(t, sender.as_deref())).unwrap_or(0);
        }
        let replaces = id != 0;
        if id == 0 {
            id = self.next.fetch_add(1, Ordering::Relaxed);
        }
        let generation = owners.claim(id, sender.clone());
        drop(owners);
        if let Some(k) = key {
            self.tags.lock().unwrap().insert(k, id);
        }
        let mut note = note(id, replaces, app_name, app_icon, summary, body, actions, &hints, expire_timeout);
        note.sender = sender;
        note.generation = generation;
        (self.to_ui)(In::Show(Box::new(note)));
        id
    }

    /// Closes note `id` if it's the asker's (another's, or one nobody owns
    /// now, is left alone).
    fn close_notification(&self, #[zbus(header)] header: zbus::message::Header<'_>, id: u32) {
        if self.owners.lock().unwrap().may(id, header.sender().map(|s| s.as_str())) {
            (self.to_ui)(In::Close(id));
        }
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("Olo", "Olo", env!("CARGO_PKG_VERSION"), "1.2")
    }
}

/// `com.olodesktop.Notifications1`: what is on screen and in the history, and
/// acting on it.
pub(crate) struct Controls {
    /// `List`'s answer, kept current by the UI.
    snapshot: std::sync::Arc<olo_sync::Mutex<String>>,
    /// `ListArchived`'s.
    archived: std::sync::Arc<olo_sync::Mutex<String>>,
    to_ui: std::sync::Arc<dyn Fn(In) + Send + Sync>,
}

impl Controls {
    pub(crate) fn new(snapshot: std::sync::Arc<olo_sync::Mutex<String>>, archived: std::sync::Arc<olo_sync::Mutex<String>>, to_ui: std::sync::Arc<dyn Fn(In) + Send + Sync>) -> Self {
        Controls { snapshot, archived, to_ui }
    }
}

#[zbus::interface(name = "com.olodesktop.Notifications1")]
impl Controls {
    /// JSON: `{"quiet": bool, "unread": n, "notifications": [...]}`, oldest
    /// first; each with its id, app, summary, text, actions, kind, time,
    /// whether it is `unread`, still `live` at its app (actions can run)
    /// and `shown` on screen, its `look` (the `x-olo-*` hints), `via` (a
    /// website's browser) and `strength` (how faded: 1 new, 0 gone).
    fn list(&self) -> String {
        self.snapshot.lock().unwrap().clone()
    }

    /// JSON: `{"archived": [...]}`, oldest first: what has left the
    /// history (faded away, dismissed, cleared, forgotten), each as
    /// `List` gives one, with `archived` (when it went, seconds since
    /// the epoch). Kept for 30 days, the last 1000.
    fn list_archived(&self) -> String {
        self.archived.lock().unwrap().clone()
    }

    // Each returns once the UI has done it, so a `List` straight after
    // shows the change.

    async fn invoke(&self, id: u32, action: String) {
        self.ask(Control::Invoke(id, action)).await
    }

    async fn dismiss(&self, id: u32) {
        self.ask(Control::Dismiss(id)).await
    }

    /// Clear matching `(id, instance)` targets in one batch and one Undo step,
    /// skipping pinned, snoozed, missing and waiting items. When `unread_only`
    /// is true (Deskbar's Inbox), only notifications still unread in the Inbox
    /// whose `uid` matches non-zero `instance` are cleared; when false (CLI),
    /// read history entries are eligible too. Returns the ids actually cleared
    /// and a globally unique batch id (empty string if nothing was cleared).
    async fn clear_many(&self, targets: Vec<(u32, u64)>, unread_only: bool) -> zbus::fdo::Result<(Vec<u32>, String)> {
        if targets.len() > MAX_CLEAR_MANY {
            return Err(zbus::fdo::Error::InvalidArgs(format!("ClearMany accepts at most {MAX_CLEAR_MANY} targets, got {}", targets.len())));
        }
        if targets.is_empty() {
            return Ok((Vec::new(), String::new()));
        }
        let (reply, applied) = futures_channel::oneshot::channel();
        (self.to_ui)(In::ClearMany { targets, unread_only, reply });
        let Ok((cleared, batch, persisted)) = applied.await else {
            return Ok((Vec::new(), String::new()));
        };
        if let Some(persisted) = persisted {
            let _ = persisted.await;
        }
        Ok((cleared, batch))
    }

    /// Undo the ClearMany batch identified by `batch`, putting back exactly
    /// those entries of that batch that are still among the cleared and not
    /// already back in the history. Independent of History's Undo stack.
    /// Returns the ids actually put back.
    async fn undo_clear(&self, batch: String) -> zbus::fdo::Result<Vec<u32>> {
        if batch.is_empty() || batch.len() > MAX_BATCH_ID_LEN {
            return Ok(Vec::new());
        }
        let (reply, applied) = futures_channel::oneshot::channel();
        (self.to_ui)(In::UndoClear { batch, reply });
        let Ok((restored, persisted)) = applied.await else {
            return Ok(Vec::new());
        };
        if let Some(persisted) = persisted {
            let _ = persisted.await;
        }
        Ok(restored)
    }

    /// The banners of the app with `desktop_entry` (`olo-screenshot`) off
    /// the screen now, as if their time had run out: still in the list,
    /// unread, still open at their app. Returns once the screen no longer
    /// shows them (the banners drawn without them, and that on the
    /// screen; their surface gone with the last of them). Whatever is
    /// about to take a picture of the screen asks it, so that its own
    /// banner isn't in it: Screenshot's, of the picture before. Other
    /// apps' banners stay (a picture of one may be what's wanted). An
    /// error if they weren't seen to go (within a second: frames no
    /// longer coming), so that the asker knows.
    async fn put_away(&self, desktop_entry: String) -> zbus::fdo::Result<()> {
        if desktop_entry.is_empty() || desktop_entry.len() > 256 {
            return Err(zbus::fdo::Error::InvalidArgs("no such app".into()));
        }
        let (done, applied) = futures_channel::oneshot::channel();
        (self.to_ui)(In::PutAway(desktop_entry, done));
        applied.await.map_err(|_| zbus::fdo::Error::Failed("its banners weren't seen off the screen".into()))
    }

    async fn clear_history(&self) {
        self.ask(Control::ClearHistory).await
    }

    async fn show_history(&self) {
        self.ask(Control::ShowHistory).await
    }

    /// Mark one read (as a click on it does), or unread again.
    async fn set_read(&self, id: u32, read: bool) {
        self.ask(Control::SetRead(id, read)).await
    }

    async fn mark_all_read(&self) {
        self.ask(Control::MarkAllRead).await
    }

    /// On for `minutes` (0: until turned off), or off.
    async fn set_do_not_disturb(&self, on: bool, minutes: u32) {
        self.ask(Control::DoNotDisturb(on, minutes)).await
    }

    /// Pin one (it stays put until unpinned), or unpin it.
    async fn set_pinned(&self, id: u32, pinned: bool) {
        self.ask(Control::Pin(id, pinned)).await
    }

    /// Put one away for `minutes`; then it comes back as new.
    async fn snooze(&self, id: u32, minutes: u32) {
        self.ask(Control::Snooze(id, minutes)).await
    }

    /// The person is looking at a page of `host` in a browser (its tab
    /// showing, its window in front: Olo's Chrome says so as it
    /// happens): that site's notifications are read, as an app's are
    /// when its window is gone to.
    async fn visited_site(&self, host: String) {
        self.ask(Control::VisitedSite(host)).await
    }
}

impl Controls {
    /// Hand `c` to the UI and wait, without holding up the bus, until it
    /// has been done (or the UI has gone).
    async fn ask(&self, c: Control) {
        let (done, applied) = futures_channel::oneshot::channel();
        (self.to_ui)(In::Control(c, Some(done)));
        let _ = applied.await;
    }
}

/// A Notify call as the UI wants it.
#[allow(clippy::too_many_arguments)]
pub fn note(id: u32, replaces: bool, app_name: String, app_icon: String, summary: String, body: String, actions: Vec<String>, hints: &HashMap<String, OwnedValue>, expire_timeout: i32) -> Note {
    let hint = |k: &str| hints.get(k);
    let text = |k: &str| hint(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).filter(|s| !s.is_empty());
    let flag = |k: &str| hint(k).and_then(|v| bool::try_from(v).ok().or_else(|| u8::try_from(v).ok().map(|u| u != 0))).unwrap_or(false);
    let urgency = hint("urgency").and_then(|v| u8::try_from(v).ok()).unwrap_or(1);
    let picture = ["image-data", "image_data", "icon_data"].iter().find_map(|k| hint(k).and_then(picture));
    let path_hint = ["image-path", "image_path"].iter().find_map(|k| text(k));
    // A picture given as a file: shown as one, like image-data (and not
    // the icon too: that's an icon's name).
    let file = path_hint.as_deref().and_then(picture_file);
    // (A file that wasn't a picture isn't the icon either: only a theme
    // icon's name given there is. A file's icon, `app_icon`'s, is read as
    // a picture is: `pictures::paint_file_icon`.)
    let path_hint = path_hint.filter(|p| file.is_none() && local_path(p).is_none());
    let picture = picture.or(file);
    let desktop_entry = text("desktop-entry");
    let icon = [path_hint, Some(app_icon)]
        .into_iter()
        .flatten()
        .map(|s| s.strip_prefix("file://").map(str::to_owned).unwrap_or(s))
        .find(|s| !s.is_empty())
        .or_else(|| desktop_entry.as_deref().and_then(olo_xdg::icon_for_app));
    let mut actions: Vec<(String, String)> = actions.chunks(2).filter_map(|p| Some((p.first()?.clone(), p.get(1)?.clone()))).collect();
    // KDE's inline reply: an action named so, answered with text.
    let reply = actions.iter().position(|(k, _)| k == "inline-reply").map(|i| {
        let (_, label) = actions.remove(i);
        text("x-kde-reply-placeholder-text").unwrap_or(if label.is_empty() { "Reply".into() } else { label })
    });
    // Senders that give no name (the GTK portal's): the desktop entry's.
    let app_name = match &desktop_entry {
        Some(e) if app_name.trim().is_empty() => crate::gtk::app_name(e),
        _ => app_name,
    };
    let progress = hint("value").and_then(|v| i32::try_from(v).ok().or_else(|| u32::try_from(v).ok().map(|u| u.min(100) as i32))).map(|v| v.clamp(0, 100) as f32 / 100.0);
    let category = text("category");
    let kind = match text("x-olo-type").as_deref() {
        Some("important") => Kind::Important,
        Some("error") => Kind::Error,
        Some("progress") => Kind::Progress,
        Some("information") => Kind::Information,
        _ if progress.is_some() => Kind::Progress,
        // (The spec's own way to say something went wrong: "device.error",
        // "network.error", "transfer.error", "im.error".)
        _ if category.as_deref().is_some_and(|c| c.ends_with(".error")) => Kind::Error,
        // Critical urgency is how pressing it is, not that it's an error:
        // a call, an alarm, a battery about to die. It wants attention.
        _ if urgency >= 2 => Kind::Important,
        _ if category.as_deref().is_some_and(|c| c.starts_with("im.") || c == "call" || c.starts_with("call.")) => Kind::Important,
        _ => Kind::Information,
    };
    let urgent = urgency >= 2;
    let sound = if flag("suppress-sound") || urgency == 0 || kind == Kind::Progress {
        None
    } else if let Some(f) = text("sound-file") {
        Some(Sound::File(f.strip_prefix("file://").unwrap_or(&f).into()))
    } else if let Some(n) = text("sound-name") {
        Some(Sound::Name(n))
    } else {
        Some(Sound::Default)
    };
    // The picture it shows large and the file it's about: each a path or
    // a file:// URI of this computer's ([`local_path`]). The picture's
    // size from its file's header, and the file as it is now, here off
    // the UI thread: what shows, and what's dragged out, is this file,
    // not another given its name later. (Opened without waiting, and
    // only a file of its own read: a pipe given as a picture would have
    // held this thread, and every note after it, for ever.) A picture
    // that isn't there isn't shown, rather than its place for good.
    // Either is known as itself only if it's a file of its own, not a link
    // to another (what's dragged out is the very file, `banners::
    // draggable`; a link's picture is shown, never handed on), and the
    // file only if it's what its sender says it wrote (`x-olo-file-id`:
    // another given its name between the save and the note isn't it).
    let own = |p: &str| std::fs::symlink_metadata(p).ok().filter(|m| m.file_type().is_file()).map(|m| FileId::of_meta(&m));
    let image = text(hint::IMAGE).as_deref().and_then(local_path);
    let opened = image.as_deref().and_then(|p| crate::pictures::open_file(p, None));
    let image_id = opened.as_ref().and_then(FileId::of_file).filter(|id| image.as_deref().and_then(own) == Some(*id));
    let image_size = opened.as_ref().and_then(|f| f.try_clone().ok()).and_then(crate::pictures::file_size);
    let image = image.filter(|_| opened.is_some());
    let file = text(hint::FILE).as_deref().and_then(local_path);
    // (Said, it must be read: one said but not readable matches nothing,
    // rather than taken as never said.)
    let said = hint(hint::FILE_ID).map(|_| text(hint::FILE_ID).as_deref().and_then(FileId::parse));
    let file_id = file.as_deref().and_then(own).filter(|id| said.is_none_or(|said| said == Some(*id)));
    // (Its file known as it came, a file of its own: the first drag of its
    // picture needn't look again, `pictures::checked_file`.)
    if let (Some(path), Some(id)) = (file.as_deref(), file_id) {
        crate::pictures::seed_checked(path, id);
    }
    let look = NoteLook {
        shown_only: flag(hint::SHOWN_ONLY),
        accent: text(hint::ACCENT).as_deref().and_then(olo_protocol::notifications::parse_color),
        subtitle: text(hint::SUBTITLE),
        badge: text(hint::BADGE),
        fields: hint(hint::FIELDS).map(fields).unwrap_or_default(),
        image_size,
        image_id,
        file_id,
        file,
        image,
        length: hint(hint::LENGTH).and_then(|v| u32::try_from(v).ok()),
        // (One of its actions, if it names one.)
        apart: text(hint::APART).filter(|k| k != "default" && actions.iter().any(|(a, _)| a == k)),
        style: text(hint::STYLE).as_deref().and_then(NoteStyle::parse).unwrap_or(if kind == Kind::Important && category.as_deref().is_some_and(|c| c.starts_with("im.")) {
            NoteStyle::Message
        } else {
            NoteStyle::Standard
        }),
        size: text(hint::SIZE).as_deref().and_then(CardSize::parse).unwrap_or_default(),
        source: text(hint::SOURCE),
    };
    // (One that can be answered goes in its time too, as a message's
    // banner does on a Mac, to the history: it stays while it's being
    // answered. Telegram's stayed on the screen for good.)
    let timeout = match expire_timeout {
        0 => None,
        _ if kind == Kind::Error || urgent => None,
        t if t < 0 => Some(DEFAULT_TIMEOUT),
        t => Some(Duration::from_millis(t as u64)),
    };
    let mut note = Note {
        id,
        sender: None,
        generation: 0,
        replaces,
        app_name,
        desktop_entry,
        icon,
        picture,
        summary: markup::plain(&markup::parse(&summary)),
        body: markup::parse(&body),
        actions,
        kind,
        urgent,
        progress,
        timeout,
        resident: flag("resident"),
        transient: flag("transient"),
        sound,
        category,
        reply,
        time: std::time::SystemTime::now(),
        look,
        via: None,
        show: hint(olo_protocol::notifications::hint::SHOW).and_then(words).unwrap_or_default(),
        runs: Vec::new(),
    };
    note.runs = hint(hint::RUNS).map(|v| runs(v, &note.actions)).unwrap_or_default();
    // (What a click does, said there: what shows what it's about.)
    if note.show.is_empty()
        && let Some(at) = note.runs.iter().position(|(k, _)| k == "default")
    {
        note.show = note.runs.remove(at).1;
    }
    // Only what its app is started again with for a note, and for each
    // action its own ([`crate::relaunch_for`]: Screenshot's Open never runs
    // its Delete, and only on the note's own file as it came): kept,
    // nothing else is.
    let entry = note.desktop_entry.as_deref();
    let (file, file_id) = (note.look.file.clone(), note.look.file_id);
    if !crate::relaunch_for(entry, "default", &note.show, file.as_deref(), file_id) {
        note.show.clear();
    }
    note.runs.retain(|(key, args)| crate::relaunch_for(entry, key, args, file.as_deref(), file_id));
    // (Screenshot's actions said as what they do, whatever a note that
    // says it's Screenshot's calls them: Delete is never shown as Open.)
    if crate::own_program(entry) == Some("olo-screenshot") {
        let runs: Vec<String> = note.runs.iter().map(|(k, _)| k.clone()).collect();
        for (key, label) in note.actions.iter_mut() {
            if let Some(said) = runs.contains(key).then(|| crate::screenshot_label(key)).flatten() {
                *label = said.to_owned();
            }
        }
    }
    // A website's, through a browser: the site's own.
    crate::web::adopt(&mut note);
    note
}

/// `x-olo-runs` (`a{sas}`): what each of its actions does once it's no
/// longer open at its app, for the actions it has (`default` among them),
/// in their order; within bounds (`RUNS_MOST`), the rest left out. Each
/// looked at where it lies in the message, and only what's kept copied:
/// what's left out costs nothing.
fn runs(v: &OwnedValue, actions: &[(String, String)]) -> Vec<(String, Vec<String>)> {
    use zbus::zvariant::Value;
    let (keys, _, _) = RUNS_MOST;
    let Value::Dict(map) = &**v else { return Vec::new() };
    // (The actions it has, looked up once each; a key longer than any of
    // theirs, or more entries than could matter, aren't looked at.)
    let place: HashMap<&str, usize> = actions.iter().enumerate().map(|(i, (k, _))| (k.as_str(), i)).chain(std::iter::once(("default", usize::MAX))).collect();
    let mut found: Vec<(&str, Vec<String>)> = Vec::new();
    for (k, a) in map.iter().take(RUNS_LOOKED_AT) {
        if found.len() == keys {
            break;
        }
        let Value::Str(k) = k else { continue };
        let k = k.as_str();
        if k.len() <= KEY_MOST
            && place.contains_key(k)
            && !found.iter().any(|(f, _)| *f == k)
            && let Some(w) = words_of(a)
        {
            found.push((k, w));
        }
    }
    found.sort_by_key(|(k, _)| place.get(k).copied().unwrap_or(usize::MAX));
    found.into_iter().map(|(k, w)| (k.to_owned(), w)).collect()
}

/// At most this many of an `x-olo-runs` dictionary's entries are looked
/// at, and its keys at most this long (an action's key).
const RUNS_LOOKED_AT: usize = 64;
const KEY_MOST: usize = 64;

/// An `as` within bounds (`RUNS_MOST`: its words, each one's bytes), its
/// shape and sizes looked at where it lies before anything's copied:
/// `x-olo-show`, or an action's in `x-olo-runs`.
fn words_of(v: &zbus::zvariant::Value) -> Option<Vec<String>> {
    use zbus::zvariant::Value;
    let (_, args, bytes) = RUNS_MOST;
    let Value::Array(a) = v else { return None };
    if a.is_empty() || a.len() > args || !a.iter().all(|w| matches!(w, Value::Str(s) if s.len() <= bytes)) {
        return None;
    }
    Some(a.iter().filter_map(|w| if let Value::Str(s) = w { Some(s.as_str().to_owned()) } else { None }).collect())
}

/// `x-olo-show`: [`words_of`].
fn words(v: &OwnedValue) -> Option<Vec<String>> {
    words_of(v)
}

/// A local file's path from a hint: a whole path, or a `file://` URI of
/// this computer's (no host, or `localhost`), its escapes read once.
/// None for anything else: another computer's file isn't the one here
/// of its name, and a name relative to nothing is no file.
pub fn local_path(said: &str) -> Option<String> {
    let path = match said.strip_prefix("file://") {
        None => said.to_owned(),
        Some(rest) => {
            let (host, path) = rest.split_at(rest.find('/')?);
            if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) || path.contains(['?', '#']) {
                return None;
            }
            String::from_utf8(unescape(path)?).ok()?
        }
    };
    (path.starts_with('/') && !path.contains('\0')).then_some(path)
}

/// `%xx` escapes read (once); None if one is cut short or not hex.
fn unescape(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |c: Option<&u8>| (*c? as char).to_digit(16);
            out.push((hex(b.get(i + 1))? * 16 + hex(b.get(i + 2))?) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

/// `x-olo-fields`: `a(ss)` in order, or `a{ss}` (sorted by label).
fn fields(v: &OwnedValue) -> Vec<(String, String)> {
    let v = || v.try_clone().ok();
    if let Some(list) = v().and_then(|v| Vec::<(String, String)>::try_from(v).ok()) {
        return list;
    }
    let mut map: Vec<(String, String)> = v().and_then(|v| HashMap::<String, String>::try_from(v).ok()).unwrap_or_default().into_iter().collect();
    map.sort();
    map
}

/// The picture in an image file, no bigger than a card shows it (decoded
/// here, on the bus's thread).
fn picture_file(path: &str) -> Option<Picture> {
    let path = local_path(path)?;
    // (Opened without waiting, and only a file of its own read.)
    let img = crate::pictures::decode(crate::pictures::open_file(&path, None)?)?.thumbnail(PICTURE_MAX, PICTURE_MAX).to_rgba8();
    Some(Picture { width: img.width() as usize, height: img.height() as usize, rgba: img.into_raw() })
}

/// The most a picture keeps, in pixels on a side (a card's 40 points at 2×,
/// with room).
pub const PICTURE_MAX: u32 = 96;

/// `image-data`: (width, height, rowstride, has alpha, bits, channels, data).
fn picture(v: &OwnedValue) -> Option<Picture> {
    use zbus::zvariant::Value;
    // (Looked at where it lies in the message, its sizes first, before any
    // of it is copied: a negative row stride made a huge one, and a short
    // one read rows over each other, a small body made a full picture.)
    let Value::Structure(s) = &**v else { return None };
    let f = s.fields();
    let (Some(Value::I32(w)), Some(Value::I32(h)), Some(Value::I32(stride)), Some(Value::Bool(alpha)), Some(Value::I32(bits)), Some(Value::I32(channels)), Some(Value::Array(data))) =
        (f.first(), f.get(1), f.get(2), f.get(3), f.get(4), f.get(5), f.get(6))
    else {
        return None;
    };
    if *bits != 8 || *w <= 0 || *h <= 0 || *w > 4096 || *h > 4096 || *stride <= 0 || *channels != if *alpha { 4 } else { 3 } {
        return None;
    }
    let (w, h, stride, channels, alpha) = (*w as usize, *h as usize, *stride as usize, *channels as usize, *alpha);
    // (A row at least as long as its pixels; the last row whole; no more
    // bytes than the largest picture's rows take.)
    let row = w.checked_mul(channels)?;
    let need = stride.checked_mul(h - 1)?.checked_add(row)?;
    if stride < row || data.len() < need || data.len() > stride.checked_mul(h)? {
        return None;
    }
    let bytes = data.inner();
    let byte = |i: usize| match bytes.get(i) {
        Some(Value::U8(b)) => Some(*b),
        _ => None,
    };
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let at = y * stride + x * channels;
            rgba.extend_from_slice(&[byte(at)?, byte(at + 1)?, byte(at + 2)?, if alpha { byte(at + 3)? } else { 255 }]);
        }
    }
    Some(Picture { width: w, height: h, rgba })
}

/// Outbound D-Bus signal emitted off the UI thread and outside the model lock
/// in strict FIFO order (preserving `ActivationToken` → `ActionInvoked` →
/// `NotificationClosed` spec order without ever blocking banners or History).
enum BusSignal {
    /// Note `id` closed: the generation of it that closed (`Owners`), if
    /// known (None: whichever is its sender's now).
    Closed(u32, Option<u64>, Reason),
    Action(u32, String),
    ActivationToken(u32, String),
    Replied(u32, String),
}

/// The service, while it runs.
pub struct Bus {
    conn: zbus::blocking::Connection,
    /// Notes that came over org.gtk.Notifications, answered differently.
    gtk: crate::gtk::Shared,
    /// Non-blocking ordered emitter for freedesktop signals.
    signals: std::sync::mpsc::Sender<BusSignal>,
}

/// Where a signal about note `id` goes: to its sender alone (None: a note
/// nobody owns now is told to nobody: another app hearing "Delete chosen"
/// on a number it also knows acted on its own file).
#[cfg(test)]
fn addressee(owners: &SharedOwners, id: u32) -> Option<String> {
    owners.lock().unwrap().of(id).map(|(s, _)| s.to_owned())
}

impl Bus {
    /// Serve notifications on the session bus (or `address`, for tests).
    /// Fails if another notification service has the name.
    /// Ids start at `first` (after the history's, so they stay unique
    /// across a restart).
    pub fn serve(
        address: Option<&str>,
        next: std::sync::Arc<AtomicU32>,
        (snapshot, archived): (std::sync::Arc<olo_sync::Mutex<String>>, std::sync::Arc<olo_sync::Mutex<String>>),
        to_ui: impl Fn(In) + Send + Sync + 'static,
    ) -> zbus::Result<Bus> {
        let builder = match address {
            Some(a) => zbus::blocking::connection::Builder::address(a)?,
            None => zbus::blocking::connection::Builder::session()?,
        };
        let to_ui: std::sync::Arc<dyn Fn(In) + Send + Sync> = std::sync::Arc::new(to_ui);
        let ui = to_ui.clone();
        let senders: Senders = Default::default();
        let owners: SharedOwners = Default::default();
        let service = Service { next: next.clone(), senders: senders.clone(), owners: owners.clone(), tags: Default::default(), to_ui: Box::new(move |m| ui(m)) };
        let gtk: crate::gtk::Shared = Default::default();
        let ui = to_ui.clone();
        let gtk_service = crate::gtk::Service { next, registry: gtk.clone(), to_ui: Box::new(move |m| ui(m)) };
        let to_ui_gone = to_ui.clone();
        let controls = Controls::new(snapshot, archived, to_ui);
        let conn = builder.serve_at(PATH, service)?.serve_at(crate::gtk::PATH, gtk_service)?.serve_at(CONTROL_PATH, controls)?.build()?;
        // Senders followed before the name is ours: one that sends and
        // leaves at once is heard leaving.
        follow_senders(&conn, senders, owners.clone(), to_ui_gone);
        // (The builder's `name` counts "someone else has it" as success.)
        let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
        match conn.request_name_with_flags(NAME, flags)? {
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner => {}
            _ => return Err(zbus::Error::NameTaken),
        }
        // GLib's interface too, so GApplication apps use it (their actions
        // then reach the app even after it quit). Not ours if a GNOME Shell
        // has it.
        if let Err(e) = conn.request_name_with_flags(crate::gtk::NAME, flags) {
            tracing::info!("notifications: no {}: {e}", crate::gtk::NAME);
        }
        let (signals, rx) = std::sync::mpsc::channel::<BusSignal>();
        let sig_conn = conn.clone();
        let sig_owners = owners.clone();
        let _ = std::thread::Builder::new().name("notes-signals".into()).spawn(move || {
            for sig in rx {
                // (Each to the note's own sender alone; a note nobody owns
                // now is told to nobody. A close of a generation that isn't
                // the note's now, its sender having sent it again since, is
                // told to nobody and lets go of nothing: it went with the
                // replacing.)
                let (id, closes) = match &sig {
                    BusSignal::Closed(id, generation, _) => (*id, Some(*generation)),
                    BusSignal::Action(id, _) | BusSignal::ActivationToken(id, _) | BusSignal::Replied(id, _) => (*id, None),
                };
                let Some((to, now)) = sig_owners.lock().unwrap().of(id).map(|(s, g)| (s.to_owned(), g)) else { continue };
                if closes.is_some_and(|generation| generation.is_some_and(|g| g != now)) {
                    continue;
                }
                let to = Some(to.as_str());
                let _ = match sig {
                    BusSignal::Closed(id, _, reason) => sig_conn.emit_signal(to, PATH, NAME, "NotificationClosed", &(id, reason as u32)),
                    BusSignal::Action(id, key) => sig_conn.emit_signal(to, PATH, NAME, "ActionInvoked", &(id, key)),
                    BusSignal::ActivationToken(id, token) => sig_conn.emit_signal(to, PATH, NAME, "ActivationToken", &(id, token)),
                    BusSignal::Replied(id, text) => sig_conn.emit_signal(to, PATH, NAME, "NotificationReplied", &(id, text)),
                };
                if closes.is_some() {
                    sig_owners.lock().unwrap().retire(id, now);
                }
            }
        });
        Ok(Bus { conn, gtk, signals })
    }

    /// Note `id` closed (`generation`: the generation of it that closed, from its
    /// `Note::generation`, if known).
    pub fn closed(&self, id: u32, generation: Option<u64>, reason: Reason) {
        let mut gtk = self.gtk.lock().unwrap();
        if gtk.contains(id) {
            // GLib's interface has no such signal.
            gtk.forget(id);
            return;
        }
        let _ = self.signals.send(BusSignal::Closed(id, generation, reason));
    }

    /// Whether note `id` came over GLib's interface and is still live
    /// there (its actions reach its app by its id, `gtk::activate`,
    /// whatever its connection: there's none to follow).
    pub fn has_gtk(&self, id: u32) -> bool {
        self.gtk.lock().unwrap().contains(id)
    }

    pub fn action(&self, id: u32, key: &str) {
        if let Some((app, action, token)) = self.gtk.lock().unwrap().resolve(id, key) {
            crate::gtk::activate(self.conn.clone(), app, action, token);
            return;
        }
        let _ = self.signals.send(BusSignal::Action(id, key.to_owned()));
    }

    /// Sent just before an action, so the app may bring its window forward.
    pub fn activation_token(&self, id: u32, token: &str) {
        let mut gtk = self.gtk.lock().unwrap();
        if gtk.contains(id) {
            gtk.set_token(id, token);
            return;
        }
        let _ = self.signals.send(BusSignal::ActivationToken(id, token.to_owned()));
    }

    /// The answer typed into an inline reply.
    pub fn replied(&self, id: u32, text: &str) {
        let _ = self.signals.send(BusSignal::Replied(id, text.to_owned()));
    }
}

/// The connections that have sent notifications, by unique name.
type Senders = std::sync::Arc<olo_sync::Mutex<std::collections::HashSet<String>>>;

/// Tell the UI of each connection that leaves the bus having sent
/// notifications (asleep between them; any other leaving is passed over
/// here): what it sent stops being open at its app.
fn follow_senders(conn: &zbus::blocking::Connection, senders: Senders, owners: SharedOwners, to_ui: std::sync::Arc<dyn Fn(In) + Send + Sync>) {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else { return };
    // (Only names left without an owner: a connection gone.)
    let Ok(changes) = dbus.receive_name_owner_changed_with_args(&[(2, "")]) else { return };
    let _ = std::thread::Builder::new().name("notes-senders".into()).spawn(move || {
        for c in changes {
            let Ok(args) = c.args() else { continue };
            if args.new_owner().is_none() && args.name().starts_with(':') && senders.lock().unwrap().remove(args.name().as_str()) {
                // (Its notes are nobody's now: a later connection given its
                // number, or one naming theirs, takes none of them over.)
                owners.lock().unwrap().gone(args.name().as_str());
                to_ui(In::SenderGone(args.name().to_string()));
            }
        }
    });
}

/// Bounded timeout for CLI calls so a stalled service never hangs indefinitely.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// The command line's side: a call to the running service on a connection.
pub fn call_on(conn: &zbus::blocking::Connection, destination: Option<&str>, method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> zbus::Result<zbus::Message> {
    conn.call_method(destination, CONTROL_PATH, Some(CONTROL), method, body)
}

/// The command line's side: a call to the running service.
pub fn call(address: Option<&str>, method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> zbus::Result<zbus::Message> {
    let builder = match address {
        Some(a) => zbus::blocking::connection::Builder::address(a)?,
        None => zbus::blocking::connection::Builder::session()?,
    };
    let conn = builder.method_timeout(CALL_TIMEOUT).build()?;
    call_on(&conn, Some(NAME), method, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> OwnedValue {
        OwnedValue::try_from(zbus::zvariant::Value::from(v)).unwrap()
    }

    #[test]
    fn hints_and_timeouts() {
        let mut hints = HashMap::new();
        let n = note(3, false, "App".into(), "".into(), "Hi".into(), "There".into(), vec!["default".into(), "Open".into(), "later".into(), "Later".into()], &hints, -1);
        assert_eq!(n.timeout, Some(DEFAULT_TIMEOUT));
        assert_eq!(n.actions, [("default".to_owned(), "Open".to_owned()), ("later".to_owned(), "Later".to_owned())]);
        assert!(n.kind == Kind::Information && n.icon.is_none() && n.sound == Some(Sound::Default));
        assert_eq!(note(3, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, 0).timeout, None);
        assert_eq!(note(3, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, 1500).timeout, Some(Duration::from_millis(1500)));
        hints.insert("urgency".to_owned(), OwnedValue::from(2u8));
        // Critical urgency: urgent (it stays), and no error for that.
        let n = note(3, false, "".into(), "file:///tmp/x.png".into(), "".into(), "".into(), vec![], &hints, 1500);
        assert!(n.kind == Kind::Important && n.urgent && n.timeout.is_none());
        assert_eq!(n.icon.as_deref(), Some("/tmp/x.png"));
        // An error is one its sender says is one: Olo's own hint (with
        // or without urgency), or the spec's ".error" categories; a call
        // that's critical is urgent and important, never an error.
        let with = |pairs: &[(&str, OwnedValue)], timeout: i32| {
            let hints: HashMap<String, OwnedValue> = pairs.iter().map(|(k, v)| ((*k).to_owned(), v.try_clone().unwrap())).collect();
            let n = note(3, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, timeout);
            (n.kind, n.urgent, n.timeout.is_none())
        };
        let text = |s: &str| OwnedValue::try_from(zbus::zvariant::Value::from(s)).unwrap();
        assert!(with(&[("x-olo-type", text("error")), ("urgency", OwnedValue::from(2u8))], -1) == (Kind::Error, true, true));
        assert!(with(&[("x-olo-type", text("error"))], -1) == (Kind::Error, false, true));
        assert!(with(&[("category", text("transfer.error"))], -1) == (Kind::Error, false, true));
        assert!(with(&[("category", text("call.incoming")), ("urgency", OwnedValue::from(2u8))], -1) == (Kind::Important, true, true));
        assert!(with(&[("category", text("call.incoming"))], -1) == (Kind::Important, false, false));
        assert!(with(&[("urgency", OwnedValue::from(0u8))], -1) == (Kind::Information, false, false));
    }

    #[test]
    fn progress_reply_sound_and_kinds() {
        let mut hints = HashMap::new();
        hints.insert("value".to_owned(), OwnedValue::from(42i32));
        let n = note(1, false, "Files".into(), "".into(), "Copying".into(), "".into(), vec![], &hints, -1);
        assert_eq!((n.kind, n.progress, n.sound), (Kind::Progress, Some(0.42), None));
        let mut hints = HashMap::new();
        hints.insert("category".to_owned(), s("im.received"));
        hints.insert("sound-name".to_owned(), s("message-new-instant"));
        hints.insert("x-kde-reply-placeholder-text".to_owned(), s("Answer Ann"));
        let n = note(2, false, "Chat".into(), "".into(), "Ann".into(), "<b>hey</b> you".into(), vec!["inline-reply".into(), "".into()], &hints, -1);
        assert_eq!(n.kind, Kind::Important);
        assert_eq!(n.sound, Some(Sound::Name("message-new-instant".into())));
        // (It goes in its time, as any other: it stays while answered.)
        assert_eq!((n.reply.as_deref(), n.actions.len(), n.timeout), (Some("Answer Ann"), 0, Some(DEFAULT_TIMEOUT)));
        assert_eq!(markup::plain(&n.body), "hey you");
        hints.insert("suppress-sound".to_owned(), OwnedValue::from(true));
        hints.insert("transient".to_owned(), OwnedValue::from(true));
        let n = note(2, false, "Chat".into(), "".into(), "Ann".into(), "".into(), vec![], &hints, -1);
        assert!(n.sound.is_none() && n.transient);
        // What shows the thing a note is about (`x-olo-show`): kept with
        // it, through the history's file too; none unless said.
        assert!(n.show.is_empty());
        hints.insert("x-olo-show".to_owned(), OwnedValue::try_from(zbus::zvariant::Value::from(vec!["--show-shell".to_owned(), "17".to_owned()])).unwrap());
        hints.insert("desktop-entry".to_owned(), s("olo-terminal"));
        let n = note(5, false, "Terminal".into(), "".into(), "Claude Code needs your answer".into(), "".into(), vec!["default".into(), "Show".into()], &hints, -1);
        assert_eq!(n.show, ["--show-shell", "17"]);
        let kept: crate::store::Entry = serde_json::from_str(&serde_json::to_string(&crate::store::Entry::from_note(&n, true)).unwrap()).unwrap();
        assert_eq!((kept.show.as_slice(), kept.desktop_entry.as_deref()), (&["--show-shell".to_owned(), "17".to_owned()][..], Some("olo-terminal")));
        // Only Olo's own programs are ever started for it, by their
        // desktop entry; anything else's click brings its app forward.
        assert_eq!(crate::own_program(Some("olo-terminal")), Some("olo-terminal"));
        assert_eq!(crate::own_program(Some("olo-terminal.desktop")), Some("olo-terminal"));
        assert_eq!(crate::own_program(Some("org.gnome.Calendar")), None);
        assert_eq!(crate::own_program(Some("../../bin/sh")), None);
        assert_eq!(crate::own_program(None), None);
        assert!(!crate::show_what(Some("org.gnome.Calendar"), &["--version".to_owned()]));
        assert!(!crate::show_what(Some("olo-terminal"), &[]));
    }

    /// What its actions do once it's no longer open at its app (`x-olo-runs`), and the file it's about
    /// (`x-olo-file`): kept with it, through the history's file too; a click's said as what shows it.
    #[test]
    fn what_actions_run_and_the_file_its_about() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-notes-runs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("screenshot 4.png");
        image::RgbaImage::from_pixel(8, 5, image::Rgba([1, 2, 3, 255])).save(&shot).unwrap();
        let p = shot.to_string_lossy().into_owned();
        let id = FileId::of(&shot).unwrap();
        let receipt = id.to_string();
        let saved = |verb: &str| vec!["--saved".to_owned(), verb.to_owned(), p.clone(), receipt.clone()];
        let mut runs: HashMap<String, Vec<String>> = HashMap::new();
        for (key, verb) in [("default", "open"), ("copy", "copy"), ("delete", "delete"), ("nothing-of-its", "copy")] {
            runs.insert(key.into(), saved(verb));
        }
        // (One with too many arguments is left out; so is one that isn't what Screenshot is started with
        // for a note: another of its modes, or another option slipped in; an action whose verb isn't its
        // own; one on another file than the note's; one with another receipt.)
        runs.insert("show".into(), vec!["x".into(); 33]);
        runs.insert("edit".into(), vec!["--toggle-record".into()]);
        runs.insert("more".into(), vec!["--saved".into(), "copy".into(), "--record".into(), receipt.clone()]);
        runs.insert("trap".into(), saved("delete"));
        runs.insert("elsewhere".into(), vec!["--saved".into(), "copy".into(), "/home/a/precious.png".into(), receipt.clone()]);
        runs.insert("stale".into(), vec!["--saved".into(), "stale".into(), p.clone(), "1-2-3".into()]);
        let mut hints: HashMap<String, OwnedValue> = HashMap::new();
        hints.insert(hint::RUNS.into(), OwnedValue::try_from(zbus::zvariant::Value::from(runs)).unwrap());
        // (A file:// URI's escapes read once, for the picture as for the file.)
        hints.insert(hint::IMAGE.into(), s(&format!("file://{}", p.replace(' ', "%20"))));
        hints.insert(hint::FILE.into(), s(&format!("file://{}", p.replace(' ', "%20"))));
        hints.insert(hint::FILE_ID.into(), s(&receipt));
        hints.insert(hint::LENGTH.into(), OwnedValue::from(4u32));
        hints.insert("desktop-entry".into(), s("olo-screenshot"));
        // (Its Delete called "Open" by whoever sent it: said as what it does.)
        let actions = ["default", "Open", "copy", "Copy", "show", "Show in Tracker", "edit", "Edit", "more", "More", "trap", "Open", "elsewhere", "Copy", "stale", "Stale", "delete", "Open"]
            .map(String::from)
            .to_vec();
        let n = note(9, false, "Screenshot".into(), "".into(), "screenshot 4.png".into(), "".into(), actions, &hints, -1);
        assert_eq!(n.show, saved("open"));
        assert_eq!(n.runs.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["copy", "delete"]);
        assert_eq!(n.runs_for("delete"), Some(&saved("delete")[..]));
        assert_eq!((n.runs_for("show"), n.runs_for("default").map(|a| a[1].as_str())), (None, Some("open")));
        assert_eq!(n.actions.iter().find(|(k, _)| k == "delete").map(|(_, l)| l.as_str()), Some("Delete"));
        // The file as a path from the root (its escapes read), as its sender wrote it, and the picture as it was.
        assert_eq!(n.look.file.as_deref(), Some(p.as_str()));
        assert_eq!((n.look.file_id, n.look.image_id, n.look.image_size, n.look.image.as_deref()), (Some(id), Some(id), Some((8, 5)), Some(p.as_str())));
        assert_eq!((n.look.length, n.look.shown_only), (Some(4), false));
        let kept: crate::store::Entry = serde_json::from_str(&serde_json::to_string(&crate::store::Entry::from_note(&n, true)).unwrap()).unwrap();
        assert_eq!((kept.runs.clone(), kept.look.clone()), (n.runs.clone(), n.look.clone()));
        // No longer open at its app, its actions that run something are still its; the rest aren't.
        let mut closed = kept.clone();
        closed.live = false;
        let again = closed.to_note(None);
        assert_eq!(again.actions.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["copy", "delete"]);
        // Another file given its name between the save and the note: not the file its sender wrote, so not
        // known as it, and none of Screenshot's actions offered on it.
        hints.insert(hint::FILE_ID.into(), s("1-2-3"));
        let n = note(9, false, "Screenshot".into(), "".into(), "".into(), "".into(), vec!["copy".into(), "Copy".into()], &hints, -1);
        assert_eq!((n.look.file_id, n.runs.len(), n.show.len()), (None, 0, 0));
        hints.remove(hint::FILE_ID);
        // A link is shown, never known as the file it names (nothing's dragged out through it); shown only, too.
        let link = dir.join("latest.png");
        std::os::unix::fs::symlink(&shot, &link).unwrap();
        hints.insert(hint::IMAGE.into(), s(&link.to_string_lossy()));
        hints.insert(hint::FILE.into(), s(&link.to_string_lossy()));
        hints.insert(hint::SHOWN_ONLY.into(), OwnedValue::from(true));
        let n = note(9, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, -1);
        assert_eq!((n.look.image.is_some(), n.look.image_id, n.look.file_id, n.look.shown_only), (true, None, None, true));
        assert_eq!(n.look.dragged(), None);
        hints.remove(hint::SHOWN_ONLY);
        // A relative file is nobody's.
        hints.insert(hint::FILE.into(), s("Videos/a.mp4"));
        assert_eq!(note(9, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, -1).look.file, None);
        // A pipe given as its picture (or a picture that isn't there) isn't waited on, and isn't shown.
        let fifo = dir.join("pipe.png");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        for picture in [fifo.to_string_lossy().into_owned(), dir.join("gone.png").to_string_lossy().into_owned()] {
            hints.insert(hint::IMAGE.into(), s(&picture));
            let started = std::time::Instant::now();
            let n = note(9, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, -1);
            assert!(started.elapsed() < Duration::from_secs(2), "{picture} was waited on");
            assert_eq!((n.look.image, n.look.image_id, n.look.image_size), (None, None, None), "{picture}");
        }
        hints.remove(hint::IMAGE);
        // The standard picture (`image-path`) the same: a pipe, a file that isn't there, one that isn't a
        // picture, none of them waited on or shown, and none of them the icon (the theme's loader would read
        // it whole, however large, and wait on a pipe); a theme icon's name given there is the icon.
        let text = dir.join("notes.png");
        std::fs::write(&text, "not a picture").unwrap();
        for picture in [fifo.to_string_lossy().into_owned(), dir.join("gone.png").to_string_lossy().into_owned(), format!("file://{}", text.display())] {
            hints.insert("image-path".into(), s(&picture));
            let started = std::time::Instant::now();
            let n = note(9, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, -1);
            assert!(started.elapsed() < Duration::from_secs(2), "{picture} was waited on");
            assert_eq!((n.picture.is_none(), n.icon), (true, None), "{picture}");
        }
        hints.insert("image-path".into(), s(&shot.to_string_lossy()));
        assert!(note(9, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, -1).picture.is_some());
        hints.insert("image-path".into(), s("dialog-information"));
        assert_eq!(note(9, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, -1).icon.as_deref(), Some("dialog-information"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A note is its sender's: another connection naming its number gets a note of its own, closes nothing
    /// of it, and hears nothing done with it (two live senders each knowing number 5); a number nobody owns
    /// now (none sent it on this run, its sender left, a GTK app's) is nobody's to take, the asker given a
    /// number of its own; and a close of a generation its sender has sent again since lets go of nothing.
    #[test]
    fn a_note_is_its_senders() {
        let mut owners = Owners::default();
        let first = owners.claim(5, Some(":1.10".into()));
        assert!(owners.may(5, Some(":1.10")) && !owners.may(5, Some(":1.20")) && !owners.may(5, None));
        assert_eq!(owners.of(5), Some((":1.10", first)));
        assert!(!owners.may(6, Some(":1.20")), "a number nobody has is nobody's to take");
        owners.claim(6, Some(":1.20".into()));
        // Sent again by its sender (replacing itself): a new generation; the close of the one before, queued
        // meanwhile, lets go of nothing.
        let again = owners.claim(5, Some(":1.10".into()));
        assert!(again > first);
        owners.retire(5, first);
        assert_eq!(owners.of(5), Some((":1.10", again)), "the close of the one before took the new one's owner");
        owners.retire(5, again);
        assert_eq!(owners.of(5), None);
        owners.claim(5, Some(":1.10".into()));
        owners.gone(":1.10");
        assert_eq!((owners.of(5).is_some(), owners.of(6).map(|(s, _)| s)), (false, Some(":1.20")));
        assert!(!owners.may(5, Some(":1.20")), "its sender gone, it's nobody's to take");
        let shared: SharedOwners = std::sync::Arc::new(olo_sync::Mutex::new(owners));
        assert_eq!((addressee(&shared, 6).as_deref(), addressee(&shared, 5)), (Some(":1.20"), None));
    }

    /// A file hint is a whole path, or a file:// URI of this computer's, read once; anything else is no file.
    #[test]
    fn a_file_is_one_here() {
        assert_eq!(local_path("/home/a/Pictures/a.png").as_deref(), Some("/home/a/Pictures/a.png"));
        assert_eq!(local_path("file:///home/a/My%20Shot.png").as_deref(), Some("/home/a/My Shot.png"));
        assert_eq!(local_path("file://localhost/home/a/b.png").as_deref(), Some("/home/a/b.png"));
        // (Read once: an escaped escape is a percent sign.)
        assert_eq!(local_path("file:///a/100%2525.png").as_deref(), Some("/a/100%25.png"));
        // (A plain path is taken as it is.)
        assert_eq!(local_path("/a/100%20.png").as_deref(), Some("/a/100%20.png"));
        for bad in [
            "file://other-machine/home/a/b.png",
            "file://",
            "file:///a/%2",
            "file:///a/%zz.png",
            "file:///a/b.png?x",
            "file:///a/%00.png",
            "Videos/a.mp4",
            "",
            "~/a.png",
            "http://x/a.png",
            "file:///a/%ff.png",
        ] {
            assert_eq!(local_path(bad), None, "{bad:?}");
        }
    }

    /// Olo's apps are started again for a note only as they are for one: the option they show (or do) a
    /// note's thing with, and its words, none of them another option; never another mode, nor anything for
    /// an app that isn't one of Olo's.
    #[test]
    fn an_app_is_started_again_only_as_it_is_for_a_note() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let ok = |e: &str, a: &[&str]| crate::relaunch_allowed(Some(e), &v(a));
        assert!(ok("olo-terminal", &["--show-shell", "17"]) && ok("olo-terminal.desktop", &["--show-shell", "17"]));
        assert!(ok("olo-calendar", &["--show", "https://x/work/0.ics", "2026-11-18T14:00:00"]) && ok("olo-todo", &["--show", "uid-1"]));
        assert!(ok("olo-rex", &["--focus-block", "h", "s", "block:1"]) && ok("olo-rex", &["--session", "first", "work"]) && ok("olo-rex", &["--focus-host", "h"]));
        assert!(ok("olo-screenshot", &["--saved", "delete", "/home/a/s.png", "1-2-3"]));
        assert!(!ok("olo-screenshot", &["--toggle-record"]), "another mode");
        assert!(!ok("olo-screenshot", &["--saved", "delete", "/home/a/s.png"]), "too few words");
        assert!(!ok("olo-screenshot", &["--saved", "delete", "/home/a/s.png", "1-2-3", "--record"]), "too many");
        assert!(!ok("olo-screenshot", &["--saved", "copy", "--record", "1-2-3"]), "an option slipped in");
        assert!(!ok("olo-terminal", &["-e", "rm -rf ~"]) && !ok("olo-terminal", &["--show-shell", ""]));
        assert!(!ok("org.gnome.Calendar", &["--show", "a", "b"]) && !crate::relaunch_allowed(None, &v(&["--show-shell", "1"])));
        assert!(!ok("olo-terminal", &[]));
        assert!(!crate::show_what(Some("olo-terminal"), &v(&["-e", "sh"])), "refused before anything's started");
        // Screenshot's, each action its own verb, on the note's own file as it came; never a default Delete.
        let id = Some(FileId(1, 2, 3));
        let file = Some("/home/a/s.png");
        let for_key = |key: &str, a: &[&str]| crate::relaunch_for(Some("olo-screenshot"), key, &v(a), file, id);
        assert!(for_key("default", &["--saved", "open", "/home/a/s.png", "1-2-3"]) && for_key("delete", &["--saved", "delete", "/home/a/s.png", "1-2-3"]));
        assert!(!for_key("default", &["--saved", "delete", "/home/a/s.png", "1-2-3"]), "a click that deletes");
        assert!(!for_key("copy", &["--saved", "delete", "/home/a/s.png", "1-2-3"]), "a Copy that deletes");
        assert!(!for_key("delete", &["--saved", "delete", "/home/a/other.png", "1-2-3"]), "another file than the note's");
        assert!(!for_key("delete", &["--saved", "delete", "/home/a/s.png", "1-2-4"]), "another receipt");
        assert!(!crate::relaunch_for(Some("olo-screenshot"), "delete", &v(&["--saved", "delete", "/home/a/s.png", "1-2-3"]), file, None), "a file not known as it came");
        assert!(!for_key("wipe", &["--saved", "delete", "/home/a/s.png", "1-2-3"]), "an action Screenshot doesn't have");
        assert!(crate::relaunch_for(Some("olo-terminal"), "default", &v(&["--show-shell", "17"]), None, None), "others' as before");
    }

    #[test]
    fn image_data() {
        // 2×1 RGB with a padded row.
        let v = zbus::zvariant::Value::from((2i32, 1i32, 8i32, false, 8i32, 3i32, vec![255u8, 0, 0, 0, 255, 0, 9, 9]));
        let p = picture(&OwnedValue::try_from(v).unwrap()).unwrap();
        assert_eq!((p.width, p.height), (2, 1));
        assert_eq!(p.rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
        let bad = zbus::zvariant::Value::from((2i32, 1i32, 8i32, true, 8i32, 3i32, vec![0u8; 8]));
        assert!(picture(&OwnedValue::try_from(bad).unwrap()).is_none());
        // A row stride that's negative, shorter than its row, or a last row cut short: nothing, never a
        // picture read out of rows over each other or past the end.
        let bad = |w: i32, h: i32, stride: i32, data: Vec<u8>| picture(&OwnedValue::try_from(zbus::zvariant::Value::from((w, h, stride, false, 8i32, 3i32, data))).unwrap()).is_none();
        assert!(bad(2, 2, -6, vec![0; 12]), "a negative stride");
        assert!(bad(2, 2, 3, vec![0; 12]), "a stride shorter than its row");
        assert!(bad(2, 2, 6, vec![0; 11]), "the last row cut short");
        assert!(bad(0, 2, 6, vec![]) && bad(2, -1, 6, vec![]), "no size");
        assert!(bad(2, 1, 6, vec![0; 64]), "far more than its rows take");
        // Two rows, each padded: read as it says.
        let v = zbus::zvariant::Value::from((1i32, 2i32, 4i32, false, 8i32, 3i32, vec![1u8, 2, 3, 0, 4, 5, 6]));
        assert_eq!(picture(&OwnedValue::try_from(v).unwrap()).unwrap().rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
    }
}
