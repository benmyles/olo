//! Tabula's own applications, which need no .desktop entry to be named
//! and drawn (Deskbar, Twitcher).

/// Tabula's own applications: binary, name, icon.
pub const OUR_APPS: &[(&str, &str, &str)] = &[
    ("tabula-tracker", "Tracker", "tracker"),
    ("tabula-terminal", "Terminal", "terminal"),
    ("tabula-chrome", "Google Chrome", "google-chrome"),
    ("tabula-styled-edit", "StyledEdit", "styled-edit"),
    ("tabula-passwords", "Passwords", "passwords"),
    ("tabula-deskcalc", "DeskCalc", "calculator"),
    ("tabula-viewer", "Viewer", "show-image"),
    ("tabula-activity", "Activity Monitor", "activity"),
    ("tabula-drive-setup", "DriveSetup", "drive-setup"),
    ("tabula-screen", "Agent Screens", "agent-screens"),
    ("tabula-logs", "Log Viewer", "logs"),
    ("tabula-screenshot", "Screenshot", "screenshot"),
];

/// The panels `tabula-preferences <arg>` opens: argument, name, icon.
/// Deskbar's Preferences menu lists exactly these.
pub const PREFERENCE_PANELS: &[(&str, &str, &str)] = &[
    ("appearance", "Appearance", "appearance"),
    ("backgrounds", "Backgrounds", "backgrounds"),
    ("backup", "Backup", "backup"),
    ("bluetooth", "Bluetooth", "bluetooth"),
    ("datetime", "Date & Time", "datetime"),
    ("display", "Display", "display"),
    ("keyboard", "Keyboard", "keyboard"),
    ("network", "Network", "network-prefs"),
    ("notifications", "Notifications", "notifications"),
    ("power", "Power", "power"),
    ("screensaver", "Screen Saver", "screensaver"),
    ("security", "Security", "security"),
    ("sound", "Sound", "sound"),
    ("touchid", "Touch ID", "fingerprint"),
    ("touchpad", "Touchpad", "touchpad"),
    ("tracker", "Tracker", "tracker"),
    ("workspaces", "Workspaces", "workspaces"),
];

/// App ids of our own apps map straight to names and icons.
pub fn our_app(app_id: &str) -> Option<(&'static str, &'static str)> {
    let short = app_id.strip_prefix("tabula.").unwrap_or(app_id);
    OUR_APPS
        .iter()
        .find(|(bin, _, _)| bin.strip_prefix("tabula-") == Some(short))
        .map(|(_, name, icon)| (*name, *icon))
        .or(match short {
            "preferences" => Some(("Preferences", "preferences")),
            "notifications" => Some(("Notifications", "notifications")),
            "kit-demo" => Some(("Kit Demo", "application")),
            _ => None,
        })
}

