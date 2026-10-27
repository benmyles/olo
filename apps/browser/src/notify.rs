//! Pages' notifications (the web Notification API) shown by the desktop's
//! notification service (org.freedesktop.Notifications; Tabula's is
//! apps/notifications). The bus is talked to on a thread of its own, so a
//! slow or missing service never holds up a frame; a click on one comes
//! back as [`Clicked`] to bring its tab forward.

use std::{collections::HashMap, sync::mpsc};

use servo::WebViewId;
use tabula_kit::Proxy;

/// A page's notification.
pub struct Note {
    pub view: WebViewId,
    /// The page's site ("https://example.com"), shown as where it's from.
    pub origin: String,
    pub title: String,
    pub body: String,
    /// Replaces the site's last one with the same tag.
    pub tag: String,
    /// Stays until dismissed.
    pub sticky: bool,
}

/// The user clicked a page's notification.
pub struct Clicked {
    pub view: WebViewId,
}

pub struct Notifier {
    to: mpsc::Sender<Note>,
}

impl Notifier {
    pub fn new(proxy: Proxy) -> Notifier {
        let (to, from) = mpsc::channel();
        let _ = std::thread::Builder::new().name("notifications".into()).spawn(move || run(from, proxy));
        Notifier { to }
    }

    pub fn show(&self, note: Note) {
        let _ = self.to.send(note);
    }
}

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

fn run(from: mpsc::Receiver<Note>, proxy: Proxy) {
    let mut conn: Option<zbus::blocking::Connection> = None;
    // Notification ids → their tab, and (site, tag) → the id it replaces.
    let views = std::sync::Arc::new(std::sync::Mutex::new(HashMap::<u32, WebViewId>::new()));
    let mut tags = HashMap::<(String, String), u32>::new();
    for note in from {
        if conn.is_none() {
            conn = connect(&views, &proxy);
        }
        let Some(c) = &conn else { continue };
        let replaces = if note.tag.is_empty() { 0 } else { tags.get(&(note.origin.clone(), note.tag.clone())).copied().unwrap_or(0) };
        let site = url::Url::parse(&note.origin).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| note.origin.clone());
        // The body is markup to the service; the page's text is not.
        let body = if note.body.is_empty() { site.clone() } else { format!("{}\n{}", escape(&note.body), escape(&site)) };
        let hints: HashMap<&str, zbus::zvariant::Value> = HashMap::from([("desktop-entry", "tabula-browser".into())]);
        let reply = c.call_method(
            Some(NAME),
            PATH,
            Some(NAME),
            "Notify",
            &("NetPositive", replaces, "browser", note.title.as_str(), body.as_str(), vec!["default", "Show"], hints, if note.sticky { 0 } else { -1 }),
        );
        match reply.and_then(|r| r.body().deserialize::<u32>()) {
            Ok(id) => {
                views.lock().unwrap().insert(id, note.view);
                if !note.tag.is_empty() {
                    tags.insert((note.origin, note.tag), id);
                }
            }
            Err(e) => {
                tracing::info!("couldn't show a page's notification: {e}");
                // Reconnect next time (the service or the bus went).
                conn = None;
            }
        }
    }
}

/// The session bus, listening for clicks on ours.
fn connect(views: &std::sync::Arc<std::sync::Mutex<HashMap<u32, WebViewId>>>, proxy: &Proxy) -> Option<zbus::blocking::Connection> {
    let conn = zbus::blocking::Connection::session().map_err(|e| tracing::info!("no session bus for notifications: {e}")).ok()?;
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(NAME).ok()?.path(PATH).ok()?.build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(16)).ok()?;
    let (views, proxy) = (views.clone(), proxy.clone());
    let _ = std::thread::Builder::new().name("notification-clicks".into()).spawn(move || {
        for msg in signals.flatten() {
            let header = msg.header();
            match header.member().map(|m| m.as_str()) {
                Some("ActionInvoked") => {
                    if let Ok((id, _key)) = msg.body().deserialize::<(u32, String)>()
                        && let Some(&view) = views.lock().unwrap().get(&id)
                    {
                        proxy.send(Clicked { view });
                    }
                }
                Some("NotificationClosed") => {
                    if let Ok((id, _reason)) = msg.body().deserialize::<(u32, u32)>() {
                        views.lock().unwrap().remove(&id);
                    }
                }
                _ => {}
            }
        }
    });
    Some(conn)
}

/// Servo 0.7 shows a notification once its pictures have loaded, so one
/// without any is never shown: give those a one-pixel icon (we show our
/// own icon, not the page's).
pub fn script() -> String {
    r#"(function () {
  var N = window.Notification;
  if (!N || N.__tabula) return;
  var pixel = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';
  class Notification extends N {
    constructor(title, options) {
      var o = Object.assign({}, options || {});
      if (!o.icon && !o.image && !o.badge) o.icon = pixel;
      super(title, o);
    }
  }
  Object.defineProperty(Notification, '__tabula', {value: true});
  window.Notification = Notification;
})();"#
        .into()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_markup() {
        assert_eq!(super::escape("1 < 2 & <b>"), "1 &lt; 2 &amp; &lt;b&gt;");
    }
}
