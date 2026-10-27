//! The freedesktop notification service (org.freedesktop.Notifications,
//! spec 1.2) on the session bus: calls come in on zbus's thread and go
//! to the UI as [`In`]; the UI answers with [`Bus::closed`] and
//! [`Bus::action`].

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use zbus::zvariant::OwnedValue;

pub const NAME: &str = "org.freedesktop.Notifications";
pub const PATH: &str = "/org/freedesktop/Notifications";

/// How long a notification stays when its sender leaves it to us.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(6);

/// A picture sent with a notification (the `image-data` hint): RGBA.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: u32,
    /// It replaces the one with this id (shown in its place).
    pub replaces: bool,
    pub app_name: String,
    /// An icon name or path (ours, the theme's, or a file).
    pub icon: Option<String>,
    pub picture: Option<Picture>,
    pub summary: String,
    pub body: String,
    /// (key, label); "default" is clicking the notification.
    pub actions: Vec<(String, String)>,
    /// Critical ones stay until dismissed.
    pub critical: bool,
    /// None: until dismissed.
    pub timeout: Option<Duration>,
    /// Keep it after an action is chosen.
    pub resident: bool,
}

/// What the bus asks of the UI.
#[derive(Debug)]
pub enum In {
    Show(Note),
    Close(u32),
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
    to_ui: Box<dyn Fn(In) + Send + Sync>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Service {
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body", "actions", "icon-static", "persistence"]
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
        let id = if replaces_id != 0 { replaces_id } else { self.next.fetch_add(1, Ordering::Relaxed) };
        (self.to_ui)(In::Show(note(id, replaces_id != 0, app_name, app_icon, summary, body, actions, &hints, expire_timeout)));
        id
    }

    fn close_notification(&self, id: u32) {
        (self.to_ui)(In::Close(id));
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("Tabula", "Tabula", env!("CARGO_PKG_VERSION"), "1.2")
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
    let critical = hint("urgency").and_then(|v| u8::try_from(v).ok()).is_some_and(|u| u >= 2);
    let resident = hint("resident").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
    let picture = ["image-data", "image_data", "icon_data"].iter().find_map(|k| hint(k).and_then(picture));
    let path_hint = ["image-path", "image_path"].iter().find_map(|k| hint(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()));
    let entry = hint("desktop-entry").and_then(|v| String::try_from(v.try_clone().ok()?).ok());
    let icon = [path_hint, Some(app_icon)]
        .into_iter()
        .flatten()
        .map(|s| s.strip_prefix("file://").map(str::to_owned).unwrap_or(s))
        .find(|s| !s.is_empty())
        .or_else(|| entry.as_deref().and_then(tabula_xdg::icon_for_app));
    let actions = actions.chunks(2).filter_map(|p| Some((p.first()?.clone(), p.get(1)?.clone()))).collect();
    let timeout = match expire_timeout {
        0 => None,
        _ if critical => None,
        t if t < 0 => Some(DEFAULT_TIMEOUT),
        t => Some(Duration::from_millis(t as u64)),
    };
    Note { id, replaces, app_name, icon, picture, summary: plain(&summary), body: plain(&body), actions, critical, timeout, resident }
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

/// The body's markup (b, i, u, a, img) and entities as plain text.
pub fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(['<', '&']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if rest.starts_with('<') {
            match rest.find('>') {
                Some(end) => {
                    let tag = &rest[1..end];
                    if tag.trim_start_matches('/').trim().eq_ignore_ascii_case("br") || tag.eq_ignore_ascii_case("br/") {
                        out.push('\n');
                    } else if let Some(alt) = tag.strip_prefix("img ").and_then(|t| t.split("alt=\"").nth(1)).and_then(|t| t.split('"').next()) {
                        out.push_str(alt);
                    }
                    rest = &rest[end + 1..];
                }
                None => {
                    out.push_str(rest);
                    rest = "";
                }
            }
        } else {
            let entity = rest.find(';').filter(|&e| e <= 8).map(|e| &rest[1..e]);
            let ch = entity.and_then(|e| match e {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => e.strip_prefix("#x").and_then(|h| u32::from_str_radix(h, 16).ok()).or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok())).and_then(char::from_u32),
            });
            match (entity, ch) {
                (Some(e), Some(c)) => {
                    out.push(c);
                    rest = &rest[e.len() + 2..];
                }
                _ => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// The service, while it runs.
pub struct Bus {
    conn: zbus::blocking::Connection,
}

impl Bus {
    /// Serve notifications on the session bus (or `address`, for tests).
    /// Fails if another notification service has the name.
    pub fn serve(address: Option<&str>, to_ui: impl Fn(In) + Send + Sync + 'static) -> zbus::Result<Bus> {
        let builder = match address {
            Some(a) => zbus::blocking::connection::Builder::address(a)?,
            None => zbus::blocking::connection::Builder::session()?,
        };
        let service = Service { next: AtomicU32::new(1), to_ui: Box::new(to_ui) };
        let conn = builder.serve_at(PATH, service)?.build()?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_becomes_text() {
        assert_eq!(plain("<b>Ben</b> said: 1 &lt; 2 &amp;&amp; <i>yes</i>"), "Ben said: 1 < 2 && yes");
        assert_eq!(plain("a<br/>b<br>c"), "a\nb\nc");
        assert_eq!(plain("<a href=\"x\">link</a> &#x263A; &#65; R&D"), "link ☺ A R&D");
        assert_eq!(plain("<img src=\"x.png\" alt=\"[cat]\"/> unclosed <b"), "[cat] unclosed <b");
    }

    #[test]
    fn hints_and_timeouts() {
        let mut hints = HashMap::new();
        let n = note(3, false, "App".into(), "".into(), "Hi".into(), "There".into(), vec!["default".into(), "Open".into(), "later".into(), "Later".into()], &hints, -1);
        assert_eq!(n.timeout, Some(DEFAULT_TIMEOUT));
        assert_eq!(n.actions, [("default".to_owned(), "Open".to_owned()), ("later".to_owned(), "Later".to_owned())]);
        assert!(!n.critical && n.icon.is_none());
        assert_eq!(note(3, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, 0).timeout, None);
        assert_eq!(note(3, false, "".into(), "".into(), "".into(), "".into(), vec![], &hints, 1500).timeout, Some(Duration::from_millis(1500)));
        hints.insert("urgency".to_owned(), OwnedValue::from(2u8));
        let n = note(3, false, "".into(), "file:///tmp/x.png".into(), "".into(), "".into(), vec![], &hints, 1500);
        assert!(n.critical && n.timeout.is_none());
        assert_eq!(n.icon.as_deref(), Some("/tmp/x.png"));
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
