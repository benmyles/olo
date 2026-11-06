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
    ("olo-telegram", "Telegram", "telegram"),
    ("olo-slack", "Slack", "slack"),
    ("olo-doctor", "Olo Doctor", "doctor"),
    ("olo-suggestions", "Olo Suggestions", "suggestions"),
];

/// Our apps that are a web app in Olo's Chrome (olo-slack): the Wayland
/// app id Chrome gives the app's window (`--app=URL`: `chrome-`, the
/// URL's host and path, `-`, the profile folder's name), and ours.
pub const WEB_APPS: &[(&str, &str)] = &[("chrome-app.slack.com__client-Default", "olo.slack")];

/// The app id a window goes by: the one it gave, or ours for one of our
/// web apps' windows ([`WEB_APPS`]).
pub fn window_app_id(given: String) -> String {
    match WEB_APPS.iter().find(|(chrome, _)| *chrome == given) {
        Some((_, ours)) => (*ours).to_owned(),
        None => given,
    }
}

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
    ("default-apps", "Default Apps", "default-apps"),
    ("display", "Display", "display"),
    ("gpus", "GPUs", "gpu"),
    ("integrations", "Integrations", "integrations"),
    ("keyboard", "Keyboard", "keyboard"),
    ("network", "Network", "network-prefs"),
    ("notifications", "Notifications", "notifications"),
    ("performance", "Performance", "performance"),
    ("power", "Power", "power"),
    ("screensaver", "Screen Saver", "screensaver"),
    ("security", "Security", "security"),
    ("sound", "Sound", "sound"),
    ("terminal", "Terminal", "terminal"),
    ("themes", "Themes", "themes"),
    ("touchid", "Touch ID", "fingerprint"),
    ("touchpad", "Touchpad & Mouse", "touchpad"),
    ("tracker", "Tracker", "tracker"),
    ("windows", "Windows", "windows"),
    ("workspaces", "Workspaces", "workspaces"),
];

/// What to call the app `app_id` (or a program's name) and its icon:
/// ours, else its desktop entry's (`entry`, as `desktop_entry::find`
/// finds it), else the id's last part capitalised.
pub fn app_name(app_id: &str, entry: Option<&crate::desktop_entry::DesktopEntry>) -> (String, Option<String>) {
    if let Some((n, i)) = our_app(app_id) {
        (n.to_owned(), Some(i.to_owned()))
    } else if let Some(e) = entry {
        (e.name.clone(), e.icon.clone())
    } else if !app_id.is_empty() {
        let last = app_id.rsplit('.').next().unwrap_or(app_id);
        let mut name: String = last.to_owned();
        if let Some(f) = name.get_mut(0..1) {
            f.make_ascii_uppercase();
        }
        (name, Some(app_id.to_owned()))
    } else {
        (String::new(), None)
    }
}

/// App ids of our own apps map straight to names and icons.
pub fn our_app(app_id: &str) -> Option<(&'static str, &'static str)> {
    let short = app_id.strip_prefix("olo.").unwrap_or(app_id);
    OUR_APPS.iter().find(|(bin, _, _)| bin.strip_prefix("olo-") == Some(short)).map(|(_, name, icon)| (*name, *icon)).or(match short {
        "preferences" => Some(("Preferences", "preferences")),
        "notifications" => Some(("Notifications", "notifications")),
        // Its windows are the password alerts polkit and the keyring ask for.
        "polkit-agent" => Some(("Authentication", "authentication")),
        // Its windows are the alerts about stuck processes and memory.
        "supervisor" => Some(("Performance", "performance")),
        "kit-demo" => Some(("Kit Demo", "application")),
        _ => None,
    })
}
