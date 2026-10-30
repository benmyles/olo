//! Olo's own applications, which need no .desktop entry to be named
//! and drawn (Deskbar, Twitcher).

/// Olo's own applications: binary, name, icon.
pub const OUR_APPS: &[(&str, &str, &str)] = &[
    ("olo-tracker", "Tracker", "tracker"),
    ("olo-terminal", "Terminal", "terminal"),
    ("olo-chrome", "Google Chrome", "google-chrome"),
    ("olo-styled-edit", "StyledEdit", "styled-edit"),
    ("olo-passwords", "Passwords", "passwords"),
    ("olo-deskcalc", "DeskCalc", "calculator"),
    ("olo-viewer", "Viewer", "show-image"),
    ("olo-activity", "Activity Monitor", "activity"),
    ("olo-drive-setup", "DriveSetup", "drive-setup"),
    ("olo-disk-usage", "DiskUsage", "disk-usage"),
    ("olo-screen", "Agent Screens", "agent-screens"),
    ("olo-logs", "Log Viewer", "logs"),
    ("olo-screenshot", "Screenshot", "screenshot"),
    ("olo-pool", "Token Pool", "token-pool"),
];

/// Our applications that open another window when asked while running
/// (`--new-window`, handed to the running copy): Tracker's New Window.
pub const NEW_WINDOW: &[&str] = &["olo-tracker", "olo-terminal", "olo-styled-edit"];

/// The app id our program `bin` runs as ("olo-styled-edit" is
/// "olo.styled-edit"), for `instance`.
pub fn app_id_of(bin: &str) -> Option<String> {
    bin.strip_prefix("olo-").map(|rest| format!("olo.{rest}"))
}

/// The panels `olo-preferences <arg>` opens: argument, name, icon.
/// Deskbar's Preferences menu lists exactly these.
pub const PREFERENCE_PANELS: &[(&str, &str, &str)] = &[
    ("ai", "AI", "ai"),
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
    ("themes", "Themes", "themes"),
    ("touchid", "Touch ID", "fingerprint"),
    ("touchpad", "Touchpad & Mouse", "touchpad"),
    ("tracker", "Tracker", "tracker"),
    ("windows", "Windows", "windows"),
    ("workspaces", "Workspaces", "workspaces"),
];

/// App ids of our own apps map straight to names and icons.
pub fn our_app(app_id: &str) -> Option<(&'static str, &'static str)> {
    let short = app_id.strip_prefix("olo.").unwrap_or(app_id);
    OUR_APPS
        .iter()
        .find(|(bin, _, _)| bin.strip_prefix("olo-") == Some(short))
        .map(|(_, name, icon)| (*name, *icon))
        .or(match short {
            "preferences" => Some(("Preferences", "preferences")),
            "notifications" => Some(("Notifications", "notifications")),
            // Its windows are the password alerts polkit and the keyring ask for.
            "polkit-agent" => Some(("Authentication", "authentication")),
            "kit-demo" => Some(("Kit Demo", "application")),
            _ => None,
        })
}

