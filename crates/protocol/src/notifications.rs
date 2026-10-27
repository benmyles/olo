//! Notification settings (`notifications.json` in Tabula's config
//! directory): Do Not Disturb and what each app may do. tabula-notifications
//! reads them (again whenever the file changes); Preferences ▸
//! Notifications and Deskbar's menu write them.

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
    /// How long a notification stays (seconds) when its app leaves it to us.
    pub seconds: u32,
    /// By app name, as apps give it.
    pub apps: BTreeMap<String, AppNotifications>,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        NotificationSettings { dnd: false, dnd_until: None, schedule: None, seconds: 6, apps: BTreeMap::new() }
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
        tabula_xdg::config_dir().join("notifications.json")
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

/// What tabula-notifications tells Deskbar (a small JSON file in the
/// runtime directory, rewritten on every change).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationState {
    /// In the history and not yet seen.
    pub unread: u32,
    /// Do Not Disturb is on now (by hand or by its schedule).
    pub quiet: bool,
}

impl NotificationState {
    pub fn path() -> std::path::PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
        dir.join(format!("tabula-notifications-{display}.json"))
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
    }
}
