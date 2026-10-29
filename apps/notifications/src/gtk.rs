//! GLib's own notification interface, `org.gtk.Notifications`: what
//! GApplication apps (`g_application_send_notification`) use when the name
//! is on the bus, as under GNOME. Notes are keyed by (app id, the app's
//! own id); their actions are the app's `app.*` actions, run by calling
//! `org.freedesktop.Application` on the app itself (which D-Bus starts if
//! it isn't running) rather than by a signal.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::SystemTime,
};

use zbus::zvariant::{OwnedValue, Value};

use crate::{
    markup::{self, Span},
    server::{DEFAULT_TIMEOUT, In, Kind, Note, Picture, Sound},
};

pub const NAME: &str = "org.gtk.Notifications";
pub const PATH: &str = "/org/gtk/Notifications";

/// One of the app's actions, with its parameter.
#[derive(Debug)]
pub struct Action {
    /// Without the `app.` prefix.
    pub name: String,
    pub target: Option<OwnedValue>,
}

impl Clone for Action {
    fn clone(&self) -> Self {
        Action { name: self.name.clone(), target: self.target.as_ref().and_then(|t| t.try_clone().ok()) }
    }
}

/// Where a note came from, to answer it.
#[derive(Debug, Clone)]
pub struct Origin {
    pub app_id: String,
    pub gtk_id: String,
    /// None: activate the app.
    pub default: Option<Action>,
    /// The buttons, as `gtk-0`, `gtk-1`, … in the note's actions.
    pub buttons: Vec<Action>,
}

#[derive(Debug, Default)]
pub struct Registry {
    by_id: HashMap<u32, Origin>,
    by_key: HashMap<(String, String), u32>,
    /// Activation tokens handed over just before an action.
    tokens: HashMap<u32, String>,
}

pub type Shared = Arc<Mutex<Registry>>;

impl Registry {
    pub fn contains(&self, id: u32) -> bool {
        self.by_id.contains_key(&id)
    }

    /// The note went: forget it.
    pub fn forget(&mut self, id: u32) {
        if let Some(o) = self.by_id.remove(&id) {
            self.by_key.remove(&(o.app_id, o.gtk_id));
        }
        self.tokens.remove(&id);
    }

    pub fn set_token(&mut self, id: u32, token: &str) {
        if self.by_id.contains_key(&id) {
            self.tokens.insert(id, token.to_owned());
        }
    }

    /// What running action `key` of note `id` means: the app, the action
    /// (None: just activate it) and the token to hand on.
    pub fn resolve(&mut self, id: u32, key: &str) -> Option<(String, Option<Action>, Option<String>)> {
        let o = self.by_id.get(&id)?;
        let action = match key {
            "default" => o.default.clone(),
            k => Some(o.buttons.get(k.strip_prefix("gtk-")?.parse::<usize>().ok()?)?.clone()),
        };
        Some((o.app_id.clone(), action, self.tokens.remove(&id)))
    }
}

pub struct Service {
    pub next: Arc<AtomicU32>,
    pub registry: Shared,
    pub to_ui: Box<dyn Fn(In) + Send + Sync>,
}

#[zbus::interface(name = "org.gtk.Notifications")]
impl Service {
    fn add_notification(&self, app_id: String, id: String, notification: HashMap<String, OwnedValue>) {
        if app_id.is_empty() {
            return;
        }
        let (number, replaces) = {
            let mut reg = self.registry.lock().unwrap();
            match reg.by_key.get(&(app_id.clone(), id.clone())) {
                Some(&n) => (n, true),
                None => {
                    let n = self.next.fetch_add(1, Ordering::Relaxed);
                    reg.by_key.insert((app_id.clone(), id.clone()), n);
                    (n, false)
                }
            }
        };
        let (note, origin) = note(number, replaces, &app_id, &id, &notification);
        self.registry.lock().unwrap().by_id.insert(number, origin);
        (self.to_ui)(In::Show(Box::new(note)));
    }

    fn remove_notification(&self, app_id: String, id: String) {
        let number = self.registry.lock().unwrap().by_key.get(&(app_id, id)).copied();
        if let Some(n) = number {
            (self.to_ui)(In::Close(n));
        }
    }
}

/// An app's name from its id: its desktop entry's, else the id's last part.
pub fn app_name(app_id: &str) -> String {
    let id = app_id.trim_end_matches(".desktop");
    olo_xdg::desktop_entry::find(id).map(|e| e.name).filter(|n| !n.is_empty()).unwrap_or_else(|| id.rsplit('.').next().unwrap_or(id).to_owned())
}

/// A GNotification (as `g_notification_serialize` gives it) as a note.
pub fn note(number: u32, replaces: bool, app_id: &str, gtk_id: &str, n: &HashMap<String, OwnedValue>) -> (Note, Origin) {
    let text = |k: &str| n.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).filter(|s| !s.is_empty());
    let action = |name: String, target: Option<&OwnedValue>| {
        name.strip_prefix("app.").map(|a| Action {
            name: a.to_owned(),
            target: target.and_then(|t| OwnedValue::try_from(unwrap(t).try_clone().ok()?).ok()),
        })
    };
    let app_name = app_name(app_id);
    let body = match (text("markup-body"), text("body")) {
        (Some(m), _) => markup::parse(&m),
        (None, Some(b)) => vec![Span { text: b, ..Default::default() }],
        (None, None) => Vec::new(),
    };
    let (icon, picture) = n.get("icon").map(icon).unwrap_or_default();
    let kind = match text("priority").as_deref() {
        Some("urgent") => Kind::Error,
        Some("high") => Kind::Important,
        _ => Kind::Information,
    };
    let default = text("default-action").and_then(|a| action(a, n.get("default-action-target")));
    let mut buttons = Vec::new();
    let mut actions = vec![("default".to_owned(), String::new())];
    let list = n.get("buttons").and_then(|v| Vec::<HashMap<String, OwnedValue>>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
    for b in list {
        let get = |k: &str| b.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        let (Some(label), Some(name)) = (get("label"), get("action")) else { continue };
        if let Some(a) = action(name, b.get("target")) {
            actions.push((format!("gtk-{}", buttons.len()), label));
            buttons.push(a);
        }
    }
    let note = Note {
        id: number,
        replaces,
        app_name,
        desktop_entry: Some(app_id.to_owned()),
        icon,
        picture,
        summary: text("title").unwrap_or_default(),
        body,
        actions,
        kind,
        progress: None,
        timeout: (kind != Kind::Error).then_some(DEFAULT_TIMEOUT),
        resident: false,
        transient: false,
        sound: (text("priority").as_deref() != Some("low")).then_some(Sound::Default),
        category: text("category"),
        reply: None,
        time: SystemTime::now(),
        look: Default::default(),
        via: None,
    };
    (note, Origin { app_id: app_id.to_owned(), gtk_id: gtk_id.to_owned(), default, buttons })
}

/// The value inside a variant (`v`), however deeply wrapped.
fn unwrap<'a>(v: &'a Value<'a>) -> &'a Value<'a> {
    match v {
        Value::Value(b) => unwrap(b),
        v => v,
    }
}

/// A serialized GIcon: a theme name (the first of `('themed', <as>)`), a
/// file (`('file', <s>)` or a plain path or URI), or image bytes
/// (`('bytes', <ay>)`, decoded to a picture).
fn icon(v: &OwnedValue) -> (Option<String>, Option<Picture>) {
    let v = unwrap(v);
    let path = |s: &str| s.strip_prefix("file://").unwrap_or(s).to_owned();
    match v {
        Value::Str(s) => (Some(path(s.as_str())).filter(|s| !s.is_empty()), None),
        Value::Structure(st) => {
            let f = st.fields();
            let (Some(Value::Str(kind)), Some(payload)) = (f.first(), f.get(1)) else { return (None, None) };
            let payload = unwrap(payload);
            match (kind.as_str(), payload) {
                ("themed", Value::Array(a)) => {
                    let first = a.iter().find_map(|n| if let Value::Str(s) = n { Some(s.to_string()) } else { None });
                    (first, None)
                }
                ("file", Value::Str(s)) => (Some(path(s.as_str())), None),
                ("bytes", Value::Array(a)) => {
                    let bytes: Vec<u8> = a.iter().filter_map(|b| if let Value::U8(b) = b { Some(*b) } else { None }).collect();
                    (None, decode(&bytes))
                }
                _ => (None, None),
            }
        }
        _ => (None, None),
    }
}

/// Image bytes (PNG, JPEG, …) as a picture, at most 256 px.
fn decode(bytes: &[u8]) -> Option<Picture> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > 256 || img.height() > 256 { img.thumbnail(256, 256) } else { img };
    let rgba = img.to_rgba8();
    Some(Picture { width: rgba.width() as usize, height: rgba.height() as usize, rgba: rgba.into_raw() })
}

/// Run the action (or activate the app) the way GApplication expects,
/// on a thread: the app may take a while to start.
pub fn activate(conn: zbus::blocking::Connection, app_id: String, action: Option<Action>, token: Option<String>) {
    std::thread::spawn(move || {
        let path = format!("/{}", app_id.replace('.', "/").replace('-', "_"));
        let mut platform: HashMap<&str, Value> = HashMap::new();
        if let Some(t) = token {
            platform.insert("activation-token", Value::from(t.clone()));
            platform.insert("desktop-startup-id", Value::from(t));
        }
        let iface = Some("org.freedesktop.Application");
        let r = match action {
            Some(a) => {
                let params: Vec<Value> = a.target.as_ref().and_then(|t| Value::try_clone(t).ok()).into_iter().collect();
                conn.call_method(Some(app_id.as_str()), path.as_str(), iface, "ActivateAction", &(a.name.as_str(), params, &platform))
            }
            None => conn.call_method(Some(app_id.as_str()), path.as_str(), iface, "Activate", &(&platform,)),
        };
        if let Err(e) = r {
            tracing::warn!("notifications: {app_id} didn't take the action: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: impl Into<Value<'static>>) -> OwnedValue {
        OwnedValue::try_from(x.into()).unwrap()
    }

    #[test]
    fn gnotification() {
        let mut button = HashMap::new();
        button.insert("label".to_owned(), v("Reply"));
        button.insert("action".to_owned(), v("app.reply"));
        button.insert("target".to_owned(), v(Value::Value(Box::new(Value::from("thread-7")))));
        let mut other = HashMap::new();
        other.insert("label".to_owned(), v("Window"));
        other.insert("action".to_owned(), v("win.close"));
        let mut n = HashMap::new();
        n.insert("title".to_owned(), v("Ada"));
        n.insert("body".to_owned(), v("Lunch? <b>not markup</b>"));
        n.insert("priority".to_owned(), v("high"));
        n.insert("icon".to_owned(), v(Value::Value(Box::new(Value::from(("themed", Value::Value(Box::new(Value::from(vec!["mail-unread", "mail"])))))))));
        n.insert("default-action".to_owned(), v("app.open"));
        n.insert("buttons".to_owned(), v(Value::from(vec![button, other].into_iter().map(|b| b.into_iter().map(|(k, x)| (k, Value::from(x))).collect::<HashMap<String, Value>>()).collect::<Vec<_>>())));
        let (note, origin) = note(5, false, "org.example.Chat", "msg-1", &n);
        assert_eq!(note.summary, "Ada");
        assert_eq!(markup::plain(&note.body), "Lunch? <b>not markup</b>", "body is plain text");
        assert_eq!(note.kind, Kind::Important);
        assert_eq!(note.icon.as_deref(), Some("mail-unread"));
        assert_eq!(note.desktop_entry.as_deref(), Some("org.example.Chat"));
        // Only app actions become buttons.
        assert_eq!(note.actions, vec![("default".to_owned(), String::new()), ("gtk-0".to_owned(), "Reply".to_owned())]);
        let mut reg = Registry::default();
        reg.by_id.insert(5, origin);
        reg.set_token(5, "tok");
        let (app, action, token) = reg.resolve(5, "gtk-0").unwrap();
        let action = action.unwrap();
        assert_eq!((app.as_str(), action.name.as_str(), token.as_deref()), ("org.example.Chat", "reply", Some("tok")));
        assert_eq!(action.target.map(|t| String::try_from(t).unwrap()).as_deref(), Some("thread-7"));
        assert_eq!(reg.resolve(5, "default").unwrap().1.unwrap().name, "open");
        assert!(reg.resolve(5, "gtk-9").is_none());
    }

    #[test]
    fn icons() {
        assert_eq!(icon(&v("file:///usr/share/a.png")).0.as_deref(), Some("/usr/share/a.png"));
        assert_eq!(icon(&v(("file", Value::Value(Box::new(Value::from("/tmp/b.png")))))).0.as_deref(), Some("/tmp/b.png"));
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 3, image::Rgba([255, 0, 0, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let p = icon(&v(("bytes", Value::Value(Box::new(Value::from(png)))))).1.unwrap();
        assert_eq!((p.width, p.height, &p.rgba[..4]), (4, 3, &[255, 0, 0, 255][..]));
    }
}
