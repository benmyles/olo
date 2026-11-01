//! Websites' notifications, through Chrome (or Chromium), made to look
//! like their own apps' rather than the browser's. Chrome sends a site's
//! `new Notification(…)` as its own (its name, its desktop entry) with
//! the site as a link heading the body, the page's icon as the picture,
//! and critical urgency for `requireInteraction`. Here the site becomes
//! the app: "GitHub" or "WhatsApp" heads the card, in its colour, and
//! groups, sorts and has its own choices in Preferences (falling back to
//! the browser's); the link comes off the body (it's the card's source
//! line); a chat site's note shows its sender's picture round; and
//! "stay until dismissed" is just that, not an error.

use olo_protocol::NoteStyle;

use crate::server::{Kind, Note};

/// Browsers whose notes carry a site: (desktop entry, name).
const BROWSERS: &[(&str, &str)] = &[
    ("google-chrome", "Google Chrome"),
    ("google-chrome-beta", "Google Chrome Beta"),
    ("google-chrome-unstable", "Google Chrome Dev"),
    ("com.google.Chrome", "Google Chrome"),
    ("chromium", "Chromium"),
    ("chromium-browser", "Chromium"),
    ("org.chromium.Chromium", "Chromium"),
];

/// Sites people get notifications from: the end of the host, the name
/// to show, its colour and how it looks.
const SITES: &[(&str, &str, [u8; 3], NoteStyle)] = &[
    ("mail.google.com", "Gmail", [0xea, 0x43, 0x35], NoteStyle::Standard),
    ("calendar.google.com", "Google Calendar", [0x1a, 0x73, 0xe8], NoteStyle::Standard),
    ("chat.google.com", "Google Chat", [0x00, 0xac, 0x47], NoteStyle::Message),
    ("meet.google.com", "Google Meet", [0x00, 0x89, 0x7b], NoteStyle::Standard),
    ("messages.google.com", "Messages", [0x1a, 0x73, 0xe8], NoteStyle::Message),
    ("docs.google.com", "Google Docs", [0x42, 0x85, 0xf4], NoteStyle::Standard),
    ("drive.google.com", "Google Drive", [0x1f, 0xa4, 0x63], NoteStyle::Standard),
    ("music.youtube.com", "YouTube Music", [0xff, 0x00, 0x33], NoteStyle::Media),
    ("youtube.com", "YouTube", [0xff, 0x00, 0x33], NoteStyle::Standard),
    ("web.whatsapp.com", "WhatsApp", [0x25, 0xd3, 0x66], NoteStyle::Message),
    ("slack.com", "Slack", [0x61, 0x1f, 0x69], NoteStyle::Message),
    ("discord.com", "Discord", [0x58, 0x65, 0xf2], NoteStyle::Message),
    ("web.telegram.org", "Telegram", [0x2a, 0xab, 0xee], NoteStyle::Message),
    ("messenger.com", "Messenger", [0x00, 0x84, 0xff], NoteStyle::Message),
    ("teams.microsoft.com", "Microsoft Teams", [0x62, 0x64, 0xa7], NoteStyle::Message),
    ("teams.live.com", "Microsoft Teams", [0x62, 0x64, 0xa7], NoteStyle::Message),
    ("outlook.office.com", "Outlook", [0x00, 0x78, 0xd4], NoteStyle::Standard),
    ("outlook.live.com", "Outlook", [0x00, 0x78, 0xd4], NoteStyle::Standard),
    ("app.element.io", "Element", [0x0d, 0xbd, 0x8b], NoteStyle::Message),
    ("github.com", "GitHub", [0x6e, 0x76, 0x81], NoteStyle::Standard),
    ("gitlab.com", "GitLab", [0xfc, 0x6d, 0x26], NoteStyle::Standard),
    ("linear.app", "Linear", [0x5e, 0x6a, 0xd2], NoteStyle::Standard),
    ("app.asana.com", "Asana", [0xf0, 0x6a, 0x6a], NoteStyle::Standard),
    ("notion.so", "Notion", [0x78, 0x78, 0x74], NoteStyle::Standard),
    ("linkedin.com", "LinkedIn", [0x0a, 0x66, 0xc2], NoteStyle::Message),
    ("x.com", "X", [0x71, 0x76, 0x7b], NoteStyle::Standard),
    ("open.spotify.com", "Spotify", [0x1d, 0xb9, 0x54], NoteStyle::Media),
    ("claude.ai", "Claude", [0xd9, 0x77, 0x57], NoteStyle::Standard),
    ("chatgpt.com", "ChatGPT", [0x10, 0xa3, 0x7f], NoteStyle::Standard),
    ("zoom.us", "Zoom", [0x0b, 0x5c, 0xff], NoteStyle::Standard),
];

/// Whether a note from `app` (with this desktop entry) is a browser's.
fn browser(app: &str, desktop_entry: Option<&str>) -> Option<&'static str> {
    let entry = desktop_entry.map(|e| e.trim_end_matches(".desktop"));
    BROWSERS.iter().find(|(e, name)| entry == Some(*e) || app == *name).map(|(_, name)| *name)
}

/// The host in `url` ("https://mail.google.com/mail/u/0/" → "mail.google.com").
fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

/// The known site `host` is (or is under).
fn site(host: &str) -> Option<&'static (&'static str, &'static str, [u8; 3], NoteStyle)> {
    SITES.iter().find(|(end, ..)| host == *end || host.strip_suffix(end).is_some_and(|h| h.ends_with('.')))
}

/// Make a browser's note the site's own (nothing changes for anyone
/// else's).
pub fn adopt(note: &mut Note) {
    let Some(browser) = browser(&note.app_name, note.desktop_entry.as_deref()) else { return };
    // The site heads the body as a link, then a blank line.
    let Some(first) = note.body.first() else { return };
    let Some(url) = first.link.clone() else { return };
    let shown = first.text.trim().to_owned();
    let follows = note.body.get(1).map(|s| s.text.starts_with('\n'));
    if follows == Some(false) {
        return;
    }
    note.body.remove(0);
    if let Some(next) = note.body.first_mut() {
        next.text = next.text.trim_start_matches('\n').to_owned();
        if next.text.is_empty() {
            note.body.remove(0);
        }
    }
    let host = host(&url).map(str::to_owned);
    let known = host.as_deref().and_then(site);
    let name = match known {
        Some((_, name, ..)) => (*name).to_owned(),
        // An extension's, or a site we don't know: as Chrome shows it.
        None => shown.strip_prefix("www.").unwrap_or(&shown).to_owned(),
    };
    if name.is_empty() {
        return;
    }
    note.look.source = note.look.source.take().or(host.map(|h| h.strip_prefix("www.").map(str::to_owned).unwrap_or(h)).filter(|h| *h != name));
    if let Some((_, _, accent, style)) = known {
        note.look.accent = note.look.accent.or(Some(*accent));
        if note.look.style == NoteStyle::Standard {
            note.look.style = *style;
        }
    }
    note.via = Some(browser.to_owned());
    note.app_name = name;
    // requireInteraction: it stays until dismissed; that's all.
    if note.kind == Kind::Error {
        note.kind = Kind::Important;
        note.timeout = None;
    }
    // Chrome's Settings button is the site's notification settings.
    for (key, label) in &mut note.actions {
        if key == "settings" {
            *label = "Site Settings".into();
        }
    }
}

/// A site's icon, as it has none here: the first letter of the last word
/// of its name ("Google Calendar": C) on its colour, or on one its name
/// picks.
pub fn tile(name: &str, accent: Option<[u8; 3]>) -> (String, [u8; 4]) {
    const HUES: [[u8; 3]; 6] = [[0x3b, 0x7d, 0xd8], [0x2e, 0x9e, 0x6a], [0xd9, 0x6b, 0x2b], [0x8e, 0x5b, 0xc9], [0xc9, 0x4a, 0x6a], [0x4a, 0x8f, 0x9e]];
    let letter = name.split_whitespace().last().and_then(|w| w.chars().find(|c| c.is_alphanumeric())).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
    let [r, g, b] = accent.unwrap_or_else(|| HUES[name.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)) % HUES.len()]);
    (letter, [r, g, b, 255])
}

/// Whether an action is the browser's own (left off a banner's buttons).
pub fn browser_action(note_via: Option<&str>, key: &str) -> bool {
    note_via.is_some() && key == "settings"
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zbus::zvariant::OwnedValue;

    use super::*;
    use crate::{markup, server};

    fn chrome(body: &str, hints: &HashMap<String, OwnedValue>) -> Note {
        let mut h = hints.clone();
        h.insert("desktop-entry".into(), OwnedValue::try_from(zbus::zvariant::Value::from("google-chrome")).unwrap());
        server::note(1, false, "Google Chrome".into(), "".into(), "Ann".into(), body.into(), vec!["default".into(), "".into(), "settings".into(), "Settings".into()], &h, -1)
    }

    #[test]
    fn a_site_becomes_the_app() {
        let n = chrome("<a href=\"https://web.whatsapp.com/\">web.whatsapp.com</a>\n\nSee you at 6?", &HashMap::new());
        assert_eq!(n.app_name, "WhatsApp");
        assert_eq!(n.via.as_deref(), Some("Google Chrome"));
        assert_eq!(markup::plain(&n.body), "See you at 6?");
        assert_eq!(n.look.style, NoteStyle::Message);
        assert_eq!(n.look.accent, Some([0x25, 0xd3, 0x66]));
        assert_eq!(n.look.source.as_deref(), Some("web.whatsapp.com"));
        assert_eq!(n.actions[1].1, "Site Settings");
        assert!(browser_action(n.via.as_deref(), "settings") && !browser_action(None, "settings"));
    }

    #[test]
    fn unknown_sites_and_sticky_ones() {
        let mut hints = HashMap::new();
        hints.insert("urgency".to_owned(), OwnedValue::from(2u8));
        let n = chrome("<a href=\"https://www.example.org:8443/x\">www.example.org</a>\n\nBuild failed", &hints);
        assert_eq!(n.app_name, "example.org");
        assert_eq!(n.look.source, None, "the name already says it");
        assert_eq!((n.kind, n.timeout), (Kind::Important, None), "requireInteraction stays, but isn't an error");
        // Under a known site's host.
        assert_eq!(chrome("<a href=\"https://acme.slack.com/\">acme.slack.com</a>\n\nhi", &HashMap::new()).app_name, "Slack");
        // Not a browser's: as it came.
        let n = server::note(1, false, "Mail".into(), "".into(), "Hi".into(), "<a href=\"https://x.com/\">x.com</a>\n\nhey".into(), vec![], &HashMap::new(), -1);
        assert!(n.app_name == "Mail" && n.via.is_none());
        // A body without the site: left alone.
        let n = chrome("Just text", &HashMap::new());
        assert!(n.app_name == "Google Chrome" && n.via.is_none());
    }

    #[test]
    fn tiles() {
        assert_eq!(tile("Google Calendar", Some([1, 2, 3])), ("C".into(), [1, 2, 3, 255]));
        assert_eq!(tile("example.org", None).0, "E");
        assert_eq!(tile("example.org", None), tile("example.org", None), "always the same colour");
    }

    #[test]
    fn hosts() {
        assert_eq!(host("https://user@mail.google.com:443/mail?x#y"), Some("mail.google.com"));
        assert_eq!(host("chrome-extension://abcdef/"), Some("abcdef"));
        assert!(site("evilgithub.com").is_none() && site("gist.github.com").is_some());
    }
}
