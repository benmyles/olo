//! The freedesktop notification service (org.freedesktop.Notifications,
//! spec 1.2) on the session bus: calls come in on zbus's thread and go
//! to the UI as [`In`]; the UI answers with [`Bus::closed`],
//! [`Bus::action`] (after [`Bus::activation_token`]) and [`Bus::replied`].

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use zbus::zvariant::OwnedValue;

use crate::markup::{self, Span};

pub const NAME: &str = "org.freedesktop.Notifications";
pub const PATH: &str = "/org/freedesktop/Notifications";
/// Ours, beside the standard one: the history and Do Not Disturb, for the
/// command line, Deskbar and agents.
pub const CONTROL: &str = "org.tabula.Notifications1";
pub const CONTROL_PATH: &str = "/org/tabula/Notifications";

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
}

/// What the bus asks of the UI.
#[derive(Debug)]
pub enum In {
    Show(Box<Note>),
    Close(u32),
    Control(Control),
}

/// Asked over `org.tabula.Notifications1` (the CLI, Deskbar, agents).
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// Run action `key` ("default": what a click does).
    Invoke(u32, String),
    Dismiss(u32),
    ClearHistory,
    ShowHistory,
    /// Do Not Disturb on (for so many minutes; 0: until turned off) or off.
    DoNotDisturb(bool, u32),
}

/// Why a notification went (the spec's reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

struct Service {
    next: AtomicU32,
    /// Ids by (app, tag) for the hints that say "replace the last one with
    /// this tag" (volume and brightness pop-ups, a download's progress).
    tags: std::sync::Mutex<HashMap<(String, String), u32>>,
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
    "x-tabula-progress",
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
        ("Tabula", "Tabula", env!("CARGO_PKG_VERSION"), "1.2")
    }
}

/// `org.tabula.Notifications1`: what is on screen and in the history, and
/// acting on it.
struct Controls {
    /// `List`'s answer, kept current by the UI.
    snapshot: std::sync::Arc<std::sync::Mutex<String>>,
    to_ui: std::sync::Arc<dyn Fn(In) + Send + Sync>,
}

#[zbus::interface(name = "org.tabula.Notifications1")]
impl Controls {
    /// JSON: `{"quiet": bool, "unread": n, "notifications": [...]}`, oldest
    /// first; each with its id, app, summary, text, actions, kind, time,
    /// and whether it is `unread`, still `live` at its app (actions can run)
    /// and `shown` on screen.
    fn list(&self) -> String {
        self.snapshot.lock().unwrap().clone()
    }

    fn invoke(&self, id: u32, action: String) {
        (self.to_ui)(In::Control(Control::Invoke(id, action)));
    }

    fn dismiss(&self, id: u32) {
        (self.to_ui)(In::Control(Control::Dismiss(id)));
    }

    fn clear_history(&self) {
        (self.to_ui)(In::Control(Control::ClearHistory));
    }

    fn show_history(&self) {
        (self.to_ui)(In::Control(Control::ShowHistory));
    }

    /// On for `minutes` (0: until turned off), or off.
    fn set_do_not_disturb(&self, on: bool, minutes: u32) {
        (self.to_ui)(In::Control(Control::DoNotDisturb(on, minutes)));
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
    let desktop_entry = text("desktop-entry");
    let icon = [path_hint, Some(app_icon)]
        .into_iter()
        .flatten()
        .map(|s| s.strip_prefix("file://").map(str::to_owned).unwrap_or(s))
        .find(|s| !s.is_empty())
        .or_else(|| desktop_entry.as_deref().and_then(tabula_xdg::icon_for_app));
    let mut actions: Vec<(String, String)> = actions.chunks(2).filter_map(|p| Some((p.first()?.clone(), p.get(1)?.clone()))).collect();
    // KDE's inline reply: an action named so, answered with text.
    let reply = actions.iter().position(|(k, _)| k == "inline-reply").map(|i| {
        let (_, label) = actions.remove(i);
        text("x-kde-reply-placeholder-text").unwrap_or(if label.is_empty() { "Reply".into() } else { label })
    });
    let progress = hint("value")
        .and_then(|v| i32::try_from(v).ok().or_else(|| u32::try_from(v).ok().map(|u| u.min(100) as i32)))
        .map(|v| v.clamp(0, 100) as f32 / 100.0);
    let category = text("category");
    let kind = match text("x-tabula-type").as_deref() {
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
    let timeout = match expire_timeout {
        0 => None,
        _ if kind == Kind::Error || reply.is_some() => None,
        t if t < 0 => Some(DEFAULT_TIMEOUT),
        t => Some(Duration::from_millis(t as u64)),
    };
    Note {
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
    }
}

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
}

impl Bus {
    /// Serve notifications on the session bus (or `address`, for tests).
    /// Fails if another notification service has the name.
    /// Ids start at `first` (after the history's, so they stay unique
    /// across a restart).
    pub fn serve(
        address: Option<&str>,
        first: u32,
        snapshot: std::sync::Arc<std::sync::Mutex<String>>,
        to_ui: impl Fn(In) + Send + Sync + 'static,
    ) -> zbus::Result<Bus> {
        let builder = match address {
            Some(a) => zbus::blocking::connection::Builder::address(a)?,
            None => zbus::blocking::connection::Builder::session()?,
        };
        let to_ui: std::sync::Arc<dyn Fn(In) + Send + Sync> = std::sync::Arc::new(to_ui);
        let ui = to_ui.clone();
        let service = Service { next: AtomicU32::new(first.max(1)), tags: Default::default(), to_ui: Box::new(move |m| ui(m)) };
        let controls = Controls { snapshot, to_ui };
        let conn = builder.serve_at(PATH, service)?.serve_at(CONTROL_PATH, controls)?.build()?;
        // (The builder's `name` counts "someone else has it" as success.)
        match conn.request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())? {
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner => Ok(Bus { conn }),
            _ => Err(zbus::Error::NameTaken),
        }
    }

    pub fn closed(&self, id: u32, reason: Reason) {
        let _ = self.conn.emit_signal(None::<()>, PATH, NAME, "NotificationClosed", &(id, reason as u32));
    }

    pub fn action(&self, id: u32, key: &str) {
        let _ = self.conn.emit_signal(None::<()>, PATH, NAME, "ActionInvoked", &(id, key));
    }

    /// Sent just before an action, so the app may bring its window forward.
    pub fn activation_token(&self, id: u32, token: &str) {
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
