//! Notification settings (`notifications.json` in Olo's config
//! directory): Do Not Disturb, how the history fades, and what each app
//! may do. olo-notifications reads them (again whenever the file
//! changes); Preferences ▸ Notifications and Deskbar's menu write them.
//!
//! And [`NoteLook`]: what an app may say, beyond the spec, so its
//! notification looks its part (the `x-olo-*` hints in [`hint`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    /// Do Not Disturb, turned on by hand: nothing pops up or sounds
    /// (errors still show); everything goes to the history.
    pub dnd: bool,
    /// When a Do Not Disturb turned on for a while ends (seconds since
    /// the epoch); None: until turned off.
    pub dnd_until: Option<u64>,
    /// Do Not Disturb every day between these times (minutes after
    /// midnight, local time; the end may be the next morning).
    pub schedule: Option<(u16, u16)>,
    /// Do Not Disturb while the screen is shared (`SharingState`) or
    /// recorded (a recording going on the display), and stricter: errors
    /// wait in the history too, and banners on screen when it starts go.
    pub while_sharing: bool,
    /// How long a notification stays (seconds) when its app leaves it to us;
    /// 0: no banners at all but errors ("Never").
    pub seconds: u32,
    /// By app name, as apps give it.
    pub apps: BTreeMap<String, AppNotifications>,
    /// How the History window shows them.
    pub view: HistoryView,
    /// The history fades as it ages, the newest strongest, and each
    /// notification goes once it has faded away.
    pub fade: bool,
    /// Over how long (minutes).
    pub fade_minutes: u32,
    /// Deskbar shows how many notifications each app has beside its dot.
    pub deskbar_count: bool,
    /// One arriving while another of the same app with the same title
    /// and text is still active (a banner up, or unread in the history)
    /// takes its place: the earlier one goes.
    pub one_of_each: bool,
}

/// The History window's views (its View menu).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryView {
    /// Newest first under day headings.
    #[default]
    List,
    /// Under their apps, each group folding away.
    Apps,
    /// A grid of cards.
    Cards,
    /// The same app and title stacked into one, with a count.
    Conversations,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        NotificationSettings {
            dnd: false,
            dnd_until: None,
            schedule: None,
            while_sharing: true,
            seconds: 6,
            apps: BTreeMap::new(),
            view: HistoryView::List,
            fade: true,
            fade_minutes: 60,
            deskbar_count: false,
            one_of_each: true,
        }
    }
}

/// What one app's notifications may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppNotifications {
    /// Pop up on screen.
    pub banners: bool,
    pub sounds: bool,
    /// Kept in the history (and counted in Deskbar).
    pub history: bool,
}

impl Default for AppNotifications {
    fn default() -> Self {
        AppNotifications { banners: true, sounds: true, history: true }
    }
}

impl AppNotifications {
    /// Off altogether.
    pub fn blocked(&self) -> bool {
        !self.banners && !self.sounds && !self.history
    }
}

impl NotificationSettings {
    pub fn app(&self, name: &str) -> AppNotifications {
        self.apps.get(name).copied().unwrap_or_default()
    }

    /// A website's (`name`) shown through its browser (`via`): its own
    /// choices if it has any, else the browser's.
    pub fn app_via(&self, name: &str, via: Option<&str>) -> AppNotifications {
        match (self.apps.get(name), via) {
            (Some(a), _) => *a,
            (None, Some(v)) => self.app(v),
            (None, None) => AppNotifications::default(),
        }
    }

    /// How strongly a notification `age` seconds old shows in the
    /// history: 1 new, falling slowly at first, then faster, to 0 when it
    /// has faded away (always 1 with fading off).
    pub fn strength(&self, age: u64) -> f32 {
        fade_strength(self.fade.then_some(self.fade_minutes), age)
    }

    /// Whether a notification `age` seconds old has faded away.
    pub fn faded(&self, age: u64) -> bool {
        self.fade && age >= self.fade_minutes.max(1) as u64 * 60
    }

    /// Whether Do Not Disturb is on now: `now` in seconds since the epoch,
    /// `minute` the local time of day in minutes.
    pub fn quiet(&self, now: u64, minute: u16) -> bool {
        let by_hand = self.dnd && self.dnd_until.is_none_or(|t| now < t);
        let scheduled = self.schedule.is_some_and(|(from, to)| match from.cmp(&to) {
            std::cmp::Ordering::Less => (from..to).contains(&minute),
            std::cmp::Ordering::Greater => minute >= from || minute < to,
            std::cmp::Ordering::Equal => false,
        });
        by_hand || scheduled
    }

    pub fn path() -> std::path::PathBuf {
        olo_xdg::config_dir().join("notifications.json")
    }

    pub fn load() -> NotificationSettings {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Write them (atomically: readers never see half a file).
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
}

/// The hints an app may add to a notification (`Notify`'s hints) to say
/// how it should look. All are optional; the service reads the standard
/// ones too (`image-path`, `category`, …).
pub mod hint {
    /// "#rrggbb": the app's colour, for the card's band and badge.
    pub const ACCENT: &str = "x-olo-accent";
    /// A line under the title: who it's from, a channel, an album.
    pub const SUBTITLE: &str = "x-olo-subtitle";
    /// A short tag beside the title ("PR #42", "3 new").
    pub const BADGE: &str = "x-olo-badge";
    /// Label and value pairs shown as a little table (`a(ss)` or `a{ss}`).
    pub const FIELDS: &str = "x-olo-fields";
    /// A picture (a path) to show large: a screenshot, a photo.
    pub const IMAGE: &str = "x-olo-image";
    /// How it is laid out: [`NoteStyle`]'s names.
    pub const STYLE: &str = "x-olo-style";
    /// How big a card it wants in the history: [`CardSize`]'s names.
    pub const SIZE: &str = "x-olo-size";
    /// Where in the app it came from: a website, an account, a project.
    pub const SOURCE: &str = "x-olo-source";
    /// Haiku's notification types: information, important, error, progress.
    pub const TYPE: &str = "x-olo-type";
}

/// How a notification is laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteStyle {
    /// Its picture square beside the text.
    #[default]
    Standard,
    /// From someone: their picture round, the body in a balloon
    /// (`im.*` categories, chat sites).
    Message,
    /// Something playing: the artwork large, the title and artist by it.
    Media,
}

impl NoteStyle {
    pub fn parse(s: &str) -> Option<NoteStyle> {
        match s {
            "standard" | "default" => Some(NoteStyle::Standard),
            "message" | "chat" => Some(NoteStyle::Message),
            "media" | "music" => Some(NoteStyle::Media),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            NoteStyle::Standard => "standard",
            NoteStyle::Message => "message",
            NoteStyle::Media => "media",
        }
    }
}

/// How big a card a notification wants in the history's Cards view
/// (`Auto`: as big as what it holds).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardSize {
    #[default]
    Auto,
    /// Its title only, if it can.
    Small,
    /// One column.
    Medium,
    /// Two columns, with room for more of its body.
    Large,
}

impl CardSize {
    pub fn parse(s: &str) -> Option<CardSize> {
        match s {
            "auto" => Some(CardSize::Auto),
            "small" => Some(CardSize::Small),
            "medium" => Some(CardSize::Medium),
            "large" | "wide" => Some(CardSize::Large),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CardSize::Auto => "auto",
            CardSize::Small => "small",
            CardSize::Medium => "medium",
            CardSize::Large => "large",
        }
    }
}

/// What a notification says about how it looks, from its hints (and,
/// for a browser's, from the website it came from).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoteLook {
    /// The app's (or site's) colour.
    pub accent: Option<[u8; 3]>,
    pub subtitle: Option<String>,
    pub badge: Option<String>,
    pub fields: Vec<(String, String)>,
    /// A picture to show large (a path), and its size in pixels.
    pub image: Option<String>,
    pub image_size: Option<(u32, u32)>,
    pub style: NoteStyle,
    pub size: CardSize,
    /// Where in the app it came from (a website's address, an account).
    pub source: Option<String>,
}

impl NoteLook {
    /// Whether it says nothing beyond the standard.
    pub fn is_plain(&self) -> bool {
        *self == NoteLook::default()
    }
}

/// "#rrggbb" (or "rrggbb", "#rgb") as a colour.
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    let digits = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
    match h.len() {
        6 if h.is_ascii() => Some([digits(0, 2)?, digits(2, 2)?, digits(4, 2)?]),
        3 if h.is_ascii() => Some([digits(0, 1)? * 17, digits(1, 1)? * 17, digits(2, 1)? * 17]),
        _ => None,
    }
}

/// How strongly a notification `age` seconds old shows, with the history
/// fading over `fade_minutes` (None: it doesn't fade): 1 new, falling
/// slowly at first, then faster, to 0 when it has faded away.
pub fn fade_strength(fade_minutes: Option<u32>, age: u64) -> f32 {
    let Some(minutes) = fade_minutes else { return 1.0 };
    let t = (age as f32 / (minutes.max(1) as f32 * 60.0)).clamp(0.0, 1.0);
    1.0 - t.powf(1.6)
}

/// What olo-notifications tells Deskbar (a small JSON file in the
/// runtime directory, rewritten on every change).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationState {
    /// In the history and not yet read (clicked, opened or acted on).
    pub unread: u32,
    /// Do Not Disturb is on now (by hand or by its schedule).
    pub quiet: bool,
    /// The apps with notifications unread (Deskbar's dots).
    pub apps: Vec<NoteApp>,
    /// How the history fades (minutes; None: it doesn't), so each dot
    /// drains as its app's notifications fade.
    pub fade_minutes: Option<u32>,
    /// Deskbar shows each app's count beside its dot (the setting).
    pub show_count: bool,
}

/// An app with notifications unread, as Deskbar finds it among its
/// applications.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoteApp {
    /// Its desktop entry, when it said (`olo-terminal`, `com.slack.Slack`).
    pub desktop_entry: Option<String>,
    /// Its name as shown; a website's notes are its browser's.
    pub name: String,
    /// The newest unread one: (when, seconds since the epoch; its id).
    /// One arriving is newer than any before it.
    pub newest: (u64, u32),
    /// How many are unread.
    pub count: u32,
    /// When each unread one that fades came (seconds since the epoch; a
    /// pinned one doesn't fade, and isn't here).
    pub fading: Vec<u64>,
}

impl NoteApp {
    /// How fresh its notifications are now (`now`, seconds since the
    /// epoch), all together: each counts for 1/n, its strength as the
    /// history fades (pinned ones are whole). 1 while none has begun to
    /// fade (or without counts: an older service).
    pub fn freshness(&self, fade_minutes: Option<u32>, now: u64) -> f32 {
        if self.count == 0 {
            return 1.0;
        }
        let fading = self.fading.len().min(self.count as usize);
        let whole = (self.count as usize - fading) as f32;
        let faded: f32 = self.fading.iter().take(fading).map(|&t| fade_strength(fade_minutes, now.saturating_sub(t))).sum();
        ((whole + faded) / self.count as f32).clamp(0.0, 1.0)
    }
}

impl NotificationState {
    pub fn path() -> std::path::PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join(format!("olo-notifications-{}.json", olo_xdg::display_name()))
    }

    pub fn load() -> Option<NotificationState> {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn do_not_disturb() {
        let mut s = NotificationSettings::default();
        assert!(!s.quiet(1000, 600));
        s.dnd = true;
        assert!(s.quiet(1000, 600));
        s.dnd_until = Some(2000);
        assert!(s.quiet(1999, 600) && !s.quiet(2000, 600));
        s.dnd = false;
        // 22:00 to 07:00, across midnight.
        s.schedule = Some((22 * 60, 7 * 60));
        assert!(s.quiet(0, 23 * 60) && s.quiet(0, 60) && !s.quiet(0, 7 * 60) && !s.quiet(0, 12 * 60));
        // 13:00 to 14:00.
        s.schedule = Some((13 * 60, 14 * 60));
        assert!(s.quiet(0, 13 * 60 + 30) && !s.quiet(0, 14 * 60));
        // Quiet while sharing the screen unless asked not to, settings
        // from before it was a choice too.
        assert!(NotificationSettings::default().while_sharing);
        let old: NotificationSettings = serde_json::from_str(r#"{"dnd": false, "seconds": 6}"#).unwrap();
        assert!(old.while_sharing);
    }

    #[test]
    fn apps_default_to_everything() {
        let mut s = NotificationSettings::default();
        assert_eq!(s.app("Mail"), AppNotifications::default());
        s.apps.insert("Mail".into(), AppNotifications { banners: false, sounds: false, history: false });
        assert!(s.app("Mail").blocked());
        let back: NotificationSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        // Older files, fewer fields.
        let partial: NotificationSettings = serde_json::from_str(r#"{"dnd": true}"#).unwrap();
        assert!(partial.dnd && partial.seconds == 6);
        assert!(partial.fade && partial.fade_minutes == 60, "fading on, over an hour, unless turned off");
    }

    #[test]
    fn a_site_falls_back_to_its_browser() {
        let mut s = NotificationSettings::default();
        let quiet = AppNotifications { banners: false, sounds: false, history: true };
        s.apps.insert("Google Chrome".into(), quiet);
        assert_eq!(s.app_via("GitHub", Some("Google Chrome")), quiet);
        s.apps.insert("GitHub".into(), AppNotifications::default());
        assert_eq!(s.app_via("GitHub", Some("Google Chrome")), AppNotifications::default());
        assert_eq!(s.app_via("Mail", None), AppNotifications::default());
    }

    #[test]
    fn fading() {
        let mut s = NotificationSettings { fade_minutes: 60, ..Default::default() };
        assert_eq!(s.strength(0), 1.0);
        // Slowly at first: still strong at a quarter of the way.
        assert!(s.strength(15 * 60) > 0.85);
        assert!(s.strength(30 * 60) < s.strength(15 * 60));
        assert_eq!(s.strength(3600), 0.0);
        assert!(!s.faded(3599) && s.faded(3600));
        s.fade = false;
        assert!(s.strength(100_000) == 1.0 && !s.faded(100_000));
    }

    #[test]
    fn an_apps_notifications_drain_together() {
        let now = 10_000;
        // Two new, one pinned: full.
        let mut app = NoteApp { count: 3, fading: vec![now, now - 5], ..Default::default() };
        assert!(app.freshness(Some(60), now) > 0.9999);
        // One of the two faded away: its third is gone.
        app.fading[1] = now - 3600;
        assert!((app.freshness(Some(60), now) - 2.0 / 3.0).abs() < 1e-6);
        // Halfway through its fade: part of its third.
        app.fading[1] = now - 1800;
        let half = app.freshness(Some(60), now);
        assert!(half > 2.0 / 3.0 && half < 1.0);
        // Fading off, or an older service that didn't count: full.
        assert_eq!(app.freshness(None, now), 1.0);
        assert_eq!(NoteApp::default().freshness(Some(60), now), 1.0);
        // An older service's state still reads.
        let old: NotificationState = serde_json::from_str(r#"{"unread":1,"quiet":false,"apps":[{"name":"Slack","newest":[5,1]}]}"#).unwrap();
        assert_eq!((old.apps[0].count, old.fade_minutes, old.show_count), (0, None, false));
    }

    #[test]
    fn looks() {
        assert_eq!(parse_color("#25D366"), Some([0x25, 0xd3, 0x66]));
        assert_eq!(parse_color("f80"), Some([0xff, 0x88, 0x00]));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(NoteStyle::parse("chat"), Some(NoteStyle::Message));
        assert_eq!(CardSize::parse("wide"), Some(CardSize::Large));
        assert!(NoteLook::default().is_plain());
        let look = NoteLook { badge: Some("PR #42".into()), fields: vec![("Branch".into(), "main".into())], ..Default::default() };
        let back: NoteLook = serde_json::from_str(&serde_json::to_string(&look).unwrap()).unwrap();
        assert_eq!(back, look);
    }
}
