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
/// command line, Deskbar and agents.
pub const CONTROL: &str = "com.olodesktop.Notifications1";
pub const CONTROL_PATH: &str = "/com/olodesktop/Notifications";

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
/// a little different.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Information,
    /// Wants attention (a message for you, a call).
    Important,
    /// Something went wrong (urgency critical): stays until dismissed.
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
}

/// What the bus asks of the UI.
#[derive(Debug)]
pub enum In {
    Show(Box<Note>),
    Close(u32),
    /// Told once it has been done (so `List` reflects it).
    Control(Control, Option<Done>),
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
    /// Ids by (app, tag) for the hints that say "replace the last one with
    /// this tag" (volume and brightness pop-ups, a download's progress).
    tags: olo_sync::Mutex<HashMap<(String, String), u32>>,
    to_ui: Box<dyn Fn(In) + Send + Sync>,
}

/// What we can do, for GetCapabilities.
pub const CAPABILITIES: &[&str] = &[
    "actions",
    "body",
    "body-hyperlinks",
    "body-markup",
    "icon-static",
    "inline-reply",
    "persistence",
    "sound",
    "x-canonical-private-synchronous",
    "x-olo-progress",
    "x-olo-look",
];

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Service {
    fn get_capabilities(&self) -> Vec<&'static str> {
        CAPABILITIES.to_vec()
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
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
        (self.to_ui)(In::Show(Box::new(note(id, replaces, app_name, app_icon, summary, body, actions, &hints, expire_timeout))));
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
struct Controls {
    /// `List`'s answer, kept current by the UI.
    snapshot: std::sync::Arc<olo_sync::Mutex<String>>,
    to_ui: std::sync::Arc<dyn Fn(In) + Send + Sync>,
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

    // Each returns once the UI has done it, so a `List` straight after
    // shows the change.

    async fn invoke(&self, id: u32, action: String) {
        self.ask(Control::Invoke(id, action)).await
    }

    async fn dismiss(&self, id: u32) {
        self.ask(Control::Dismiss(id)).await
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
pub fn note(
    id: u32,
    replaces: bool,
    app_name: String,
    app_icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    hints: &HashMap<String, OwnedValue>,
    expire_timeout: i32,
) -> Note {
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
    let progress = hint("value")
        .and_then(|v| i32::try_from(v).ok().or_else(|| u32::try_from(v).ok().map(|u| u.min(100) as i32)))
        .map(|v| v.clamp(0, 100) as f32 / 100.0);
    let category = text("category");
    let kind = match text("x-olo-type").as_deref() {
        Some("important") => Kind::Important,
        Some("error") => Kind::Error,
        Some("progress") => Kind::Progress,
        Some("information") => Kind::Information,
        _ if progress.is_some() => Kind::Progress,
        _ if urgency >= 2 => Kind::Error,
        _ if category.as_deref().is_some_and(|c| c.starts_with("im.") || c == "call" || c.starts_with("call.")) => Kind::Important,
        _ => Kind::Information,
    };
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
    let timeout = match expire_timeout {
        0 => None,
        _ if kind == Kind::Error || reply.is_some() => None,
        t if t < 0 => Some(DEFAULT_TIMEOUT),
        t => Some(Duration::from_millis(t as u64)),
    };
    let mut note = Note {
        id,
        replaces,
        app_name,
        desktop_entry,
        icon,
        picture,
        summary: markup::plain(&markup::parse(&summary)),
        body: markup::parse(&body),
        actions,
        kind,
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

/// The service, while it runs.
pub struct Bus {
    conn: zbus::blocking::Connection,
    /// Notes that came over org.gtk.Notifications, answered differently.
    gtk: crate::gtk::Shared,
}

impl Bus {
    /// Serve notifications on the session bus (or `address`, for tests).
    /// Fails if another notification service has the name.
    /// Ids start at `first` (after the history's, so they stay unique
    /// across a restart).
    pub fn serve(
        address: Option<&str>,
        first: u32,
        snapshot: std::sync::Arc<olo_sync::Mutex<String>>,
        to_ui: impl Fn(In) + Send + Sync + 'static,
    ) -> zbus::Result<Bus> {
        let builder = match address {
            Some(a) => zbus::blocking::connection::Builder::address(a)?,
            None => zbus::blocking::connection::Builder::session()?,
        };
        let to_ui: std::sync::Arc<dyn Fn(In) + Send + Sync> = std::sync::Arc::new(to_ui);
        let ui = to_ui.clone();
        let next = std::sync::Arc::new(AtomicU32::new(first.max(1)));
        let service = Service { next: next.clone(), tags: Default::default(), to_ui: Box::new(move |m| ui(m)) };
        let gtk: crate::gtk::Shared = Default::default();
        let ui = to_ui.clone();
        let gtk_service = crate::gtk::Service { next, registry: gtk.clone(), to_ui: Box::new(move |m| ui(m)) };
        let controls = Controls { snapshot, to_ui };
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
        Ok(Bus { conn, gtk })
    }

    pub fn closed(&self, id: u32, reason: Reason) {
        let mut gtk = self.gtk.lock().unwrap();
        if gtk.contains(id) {
            // GLib's interface has no such signal.
            gtk.forget(id);
            return;
        }
        let _ = self.conn.emit_signal(None::<()>, PATH, NAME, "NotificationClosed", &(id, reason as u32));
    }

    pub fn action(&self, id: u32, key: &str) {
        if let Some((app, action, token)) = self.gtk.lock().unwrap().resolve(id, key) {
            crate::gtk::activate(self.conn.clone(), app, action, token);
            return;
        }
        let _ = self.conn.emit_signal(None::<()>, PATH, NAME, "ActionInvoked", &(id, key));
    }

    /// Sent just before an action, so the app may bring its window forward.
    pub fn activation_token(&self, id: u32, token: &str) {
        let mut gtk = self.gtk.lock().unwrap();
        if gtk.contains(id) {
            gtk.set_token(id, token);
            return;
        }
        let _ = self.conn.emit_signal(None::<()>, PATH, NAME, "ActivationToken", &(id, token));
    }

    /// The answer typed into an inline reply.
    pub fn replied(&self, id: u32, text: &str) {
        let _ = self.conn.emit_signal(None::<()>, PATH, NAME, "NotificationReplied", &(id, text));
    }
}

/// The command line's side: a call to the running service.
pub fn call(address: Option<&str>, method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> zbus::Result<zbus::Message> {
    let conn = match address {
        Some(a) => zbus::blocking::connection::Builder::address(a)?.build()?,
        None => zbus::blocking::Connection::session()?,
    };
    conn.call_method(Some(NAME), CONTROL_PATH, Some(CONTROL), method, body)
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
        let n = note(3, false, "".into(), "file:///tmp/x.png".into(), "".into(), "".into(), vec![], &hints, 1500);
        assert!(n.kind == Kind::Error && n.timeout.is_none());
        assert_eq!(n.icon.as_deref(), Some("/tmp/x.png"));
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
        assert_eq!((n.reply.as_deref(), n.actions.len(), n.timeout), (Some("Answer Ann"), 0, None));
        assert_eq!(markup::plain(&n.body), "hey you");
        hints.insert("suppress-sound".to_owned(), OwnedValue::from(true));
        hints.insert("transient".to_owned(), OwnedValue::from(true));
        let n = note(2, false, "Chat".into(), "".into(), "Ann".into(), "".into(), vec![], &hints, -1);
        assert!(n.sound.is_none() && n.transient);
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
