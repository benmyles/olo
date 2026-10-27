//! Pages' notifications (the web Notification API) shown by the desktop's
//! notification service through the kit's [`Notifier`] (its own thread,
//! so a slow or missing service never holds up a frame); a click on one
//! comes back as a [`NoteEvent`] to bring its tab forward.

use std::collections::HashMap;

use servo::WebViewId;
use tabula_kit::{
    Proxy,
    notify::{self, NoteEvent, NoteId},
};

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

pub struct Notifier {
    notes: notify::Notifier,
    /// Which tab each came from, and (site, tag) → the one it replaces.
    views: HashMap<NoteId, WebViewId>,
    tags: HashMap<(String, String), NoteId>,
}

impl Notifier {
    pub fn new(proxy: Proxy) -> Notifier {
        Notifier { notes: notify::Notifier::new(proxy, "NetPositive", "tabula-browser"), views: HashMap::new(), tags: HashMap::new() }
    }

    pub fn show(&mut self, note: Note) {
        let key = (note.origin.clone(), note.tag.clone());
        let replaces = if note.tag.is_empty() { None } else { self.tags.get(&key).copied() };
        let site = url::Url::parse(&note.origin).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| note.origin.clone());
        let body = if note.body.is_empty() { site } else { format!("{}\n{site}", note.body) };
        let n = notify::Note::new(note.title).body(body).icon("browser").action("default", "Show").sticky(note.sticky);
        let id = self.notes.show(replaces, n);
        self.views.insert(id, note.view);
        if !note.tag.is_empty() {
            self.tags.insert(key, id);
        }
    }

    /// The tab a click on one should bring forward.
    pub fn event(&mut self, e: &NoteEvent) -> Option<WebViewId> {
        match e {
            NoteEvent::Action { id, .. } => self.views.get(id).copied(),
            NoteEvent::Closed { id } => {
                self.views.remove(id);
                self.tags.retain(|_, v| v != id);
                None
            }
        }
    }
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
