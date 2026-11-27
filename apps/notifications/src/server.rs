//! The freedesktop notification service (org.freedesktop.Notifications,
//! spec 1.2) on the session bus: calls come in on zbus's thread and go
//! to the UI as [`In`]; the UI answers with [`Bus::closed`],
//! [`Bus::action`] (after [`Bus::activation_token`]) and [`Bus::replied`].

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use olo_protocol::{CardSize, NoteLook, NoteStyle, notifications::hint};
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
}

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

struct Service {
    next: std::sync::Arc<AtomicU32>,
    /// The connections that have sent notifications (their unique
    /// names): only their leaving is told to the UI.
    senders: Senders,
    /// Ids by (app, tag) for the hints that say "replace the last one with
    /// this tag" (volume and brightness pop-ups, a download's progress).
    tags: olo_sync::Mutex<HashMap<(String, String), u32>>,
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
        let mut id = replaces_id;
        if id == 0
            && let Some(t) = &tag
        {
            id = self.tags.lock().unwrap().get(&(app_name.clone(), t.clone())).copied().unwrap_or(0);
        }
        let replaces = id != 0;
        if id == 0 {
            id = self.next.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(t) = tag {
            self.tags.lock().unwrap().insert((app_name.clone(), t), id);
        }
        let mut note = note(id, replaces, app_name, app_icon, summary, body, actions, &hints, expire_timeout);
        note.sender = header.sender().map(|s| s.to_string());
        if let Some(s) = &note.sender {
            self.senders.lock().unwrap().insert(s.clone());
        }
        (self.to_ui)(In::Show(Box::new(note)));
        id
    }

    fn close_notification(&self, id: u32) {
        (self.to_ui)(In::Close(id));
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
    let path_hint = path_hint.filter(|_| file.is_none());
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
    let image = text(hint::IMAGE).map(|p| p.strip_prefix("file://").map(str::to_owned).unwrap_or(p));
    let look = NoteLook {
        accent: text(hint::ACCENT).as_deref().and_then(olo_protocol::notifications::parse_color),
        subtitle: text(hint::SUBTITLE),
        badge: text(hint::BADGE),
        fields: hint(hint::FIELDS).map(fields).unwrap_or_default(),
        // (Its size from the file's header, here off the UI thread.)
        image_size: image.as_deref().and_then(|p| image::image_dimensions(p).ok()),
        image,
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
        show: hint(olo_protocol::notifications::hint::SHOW).and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default(),
    };
    // A website's, through a browser: the site's own.
    crate::web::adopt(&mut note);
    note
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
    let path = path.strip_prefix("file://").unwrap_or(path);
    if !path.starts_with('/') {
        return None;
    }
    let img = image::open(path).ok()?.thumbnail(PICTURE_MAX, PICTURE_MAX).to_rgba8();
    Some(Picture { width: img.width() as usize, height: img.height() as usize, rgba: img.into_raw() })
}

/// The most a picture keeps, in pixels on a side (a card's 40 points at 2×,
/// with room).
pub const PICTURE_MAX: u32 = 96;

/// `image-data`: (width, height, rowstride, has alpha, bits, channels, data).
fn picture(v: &OwnedValue) -> Option<Picture> {
    let (w, h, stride, alpha, bits, channels, data) = <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(v.try_clone().ok()?).ok()?;
    let (w, h, stride, channels) = (w as usize, h as usize, stride as usize, channels as usize);
    if bits != 8 || w == 0 || h == 0 || w > 4096 || h > 4096 || channels != if alpha { 4 } else { 3 } || data.len() < stride * (h - 1) + w * channels {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &data[y * stride..];
        for x in 0..w {
            let p = &row[x * channels..x * channels + channels];
            rgba.extend_from_slice(&[p[0], p[1], p[2], if alpha { p[3] } else { 255 }]);
        }
    }
    Some(Picture { width: w, height: h, rgba })
}

/// Outbound D-Bus signal emitted off the UI thread and outside the model lock
/// in strict FIFO order (preserving `ActivationToken` → `ActionInvoked` →
/// `NotificationClosed` spec order without ever blocking banners or History).
enum BusSignal {
    Closed(u32, Reason),
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
        let service = Service { next: next.clone(), senders: senders.clone(), tags: Default::default(), to_ui: Box::new(move |m| ui(m)) };
        let gtk: crate::gtk::Shared = Default::default();
        let ui = to_ui.clone();
        let gtk_service = crate::gtk::Service { next, registry: gtk.clone(), to_ui: Box::new(move |m| ui(m)) };
        let to_ui_gone = to_ui.clone();
        let controls = Controls::new(snapshot, archived, to_ui);
        let conn = builder.serve_at(PATH, service)?.serve_at(crate::gtk::PATH, gtk_service)?.serve_at(CONTROL_PATH, controls)?.build()?;
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
        follow_senders(&conn, senders, to_ui_gone);
        let (signals, rx) = std::sync::mpsc::channel::<BusSignal>();
        let sig_conn = conn.clone();
        let _ = std::thread::Builder::new().name("notes-signals".into()).spawn(move || {
            for sig in rx {
                match sig {
                    BusSignal::Closed(id, reason) => {
                        let _ = sig_conn.emit_signal(None::<()>, PATH, NAME, "NotificationClosed", &(id, reason as u32));
                    }
                    BusSignal::Action(id, key) => {
                        let _ = sig_conn.emit_signal(None::<()>, PATH, NAME, "ActionInvoked", &(id, key));
                    }
                    BusSignal::ActivationToken(id, token) => {
                        let _ = sig_conn.emit_signal(None::<()>, PATH, NAME, "ActivationToken", &(id, token));
                    }
                    BusSignal::Replied(id, text) => {
                        let _ = sig_conn.emit_signal(None::<()>, PATH, NAME, "NotificationReplied", &(id, text));
                    }
                }
            }
        });
        Ok(Bus { conn, gtk, signals })
    }

    pub fn closed(&self, id: u32, reason: Reason) {
        let mut gtk = self.gtk.lock().unwrap();
        if gtk.contains(id) {
            // GLib's interface has no such signal.
            gtk.forget(id);
            return;
        }
        let _ = self.signals.send(BusSignal::Closed(id, reason));
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
fn follow_senders(conn: &zbus::blocking::Connection, senders: Senders, to_ui: std::sync::Arc<dyn Fn(In) + Send + Sync>) {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else { return };
    // (Only names left without an owner: a connection gone.)
    let Ok(changes) = dbus.receive_name_owner_changed_with_args(&[(2, "")]) else { return };
    let _ = std::thread::Builder::new().name("notes-senders".into()).spawn(move || {
        for c in changes {
            let Ok(args) = c.args() else { continue };
            if args.new_owner().is_none() && args.name().starts_with(':') && senders.lock().unwrap().remove(args.name().as_str()) {
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

    #[test]
    fn image_data() {
        // 2×1 RGB with a padded row.
        let v = zbus::zvariant::Value::from((2i32, 1i32, 8i32, false, 8i32, 3i32, vec![255u8, 0, 0, 0, 255, 0, 9, 9]));
        let p = picture(&OwnedValue::try_from(v).unwrap()).unwrap();
        assert_eq!((p.width, p.height), (2, 1));
        assert_eq!(p.rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
        let bad = zbus::zvariant::Value::from((2i32, 1i32, 8i32, true, 8i32, 3i32, vec![0u8; 8]));
        assert!(picture(&OwnedValue::try_from(bad).unwrap()).is_none());
    }
}
