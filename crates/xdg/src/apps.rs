//! Tabula's own applications, which need no .desktop entry to be named
//! and drawn (Deskbar, Twitcher).

/// Tabula's own applications: binary, name, icon.
pub const OUR_APPS: &[(&str, &str, &str)] = &[
    ("tabula-tracker", "Tracker", "tracker"),
    ("tabula-terminal", "Terminal", "terminal"),
    ("tabula-web", "WebPositive", "browser"),
    ("tabula-styled-edit", "StyledEdit", "styled-edit"),
    ("tabula-deskcalc", "DeskCalc", "calculator"),
    ("tabula-viewer", "Viewer", "show-image"),
    ("tabula-activity", "Activity Monitor", "activity"),
    ("tabula-screenshot", "Screenshot", "screenshot"),
];

/// The panels `tabula-preferences <arg>` opens: argument, name, icon.
/// Deskbar's Preferences menu lists exactly these.
pub const PREFERENCE_PANELS: &[(&str, &str, &str)] = &[
    ("appearance", "Appearance", "appearance"),
    ("backgrounds", "Backgrounds", "backgrounds"),
    ("bluetooth", "Bluetooth", "bluetooth"),
    ("datetime", "Date & Time", "datetime"),
    ("display", "Display", "display"),
    ("keyboard", "Keyboard", "keyboard"),
    ("network", "Network", "network-prefs"),
    ("power", "Power", "power"),
    ("screensaver", "Screen Saver", "screensaver"),
    ("sound", "Sound", "sound"),
    ("touchid", "Touch ID", "fingerprint"),
    ("touchpad", "Touchpad", "touchpad"),
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
            "kit-demo" => Some(("Kit Demo", "application")),
            _ => None,
        })
}

