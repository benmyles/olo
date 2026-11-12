//! Olo apps' own keyboard shortcuts, their menus' (Copy, Close, Move to
//! Trash, New Tab, …): which command each presses, and its keys on each
//! platform a keyboard profile follows, as [`crate::shortcuts::SHORTCUTS`]
//! has app_server's. Changes are kept in the profile's `shortcuts` beside
//! app_server's, by id. The kit gives a menu item the keys its command has
//! here (`olo_kit::menu::resolve`), so an app's menus, Preferences, the
//! shortcuts sheet and agents all agree.
//!
//! Keys are written as app_server's are, physically: on a Mac "Cmd+C" (⌘,
//! the Super key), on Windows "Ctrl+C", on BeOS "Alt+C" (its command
//! key). Every app sees its command key as Ctrl, so most only change the
//! key's name; some are the platform's own (Windows' Redo is Ctrl+Y, Find
//! Again F3, Move to Trash Delete, Get Info Alt+Enter, Open Parent
//! Alt+↑). A terminal's command key arrives with Shift, so on Windows
//! Terminal's keys are Ctrl+Shift ones.

/// One app command's keys.
pub struct AppKey {
    /// Its name in a profile's `shortcuts`: "app.copy" for every app's,
    /// "tracker.trash" for one app's.
    pub id: &'static str,
    /// The app it is for (its app id); None for every app with the command.
    pub app: Option<&'static str>,
    /// The menu command it presses.
    pub command: &'static str,
    pub title: &'static str,
    /// Mac, Windows, BeOS.
    pub keys: [&'static [&'static str]; 3],
}

const fn k(
    id: &'static str,
    app: Option<&'static str>,
    command: &'static str,
    title: &'static str,
    mac: &'static [&'static str],
    windows: &'static [&'static str],
    beos: &'static [&'static str],
) -> AppKey {
    AppKey { id, app, command, title, keys: [mac, windows, beos] }
}

const TRACKER: Option<&str> = Some("olo.tracker");
const TERMINAL: Option<&str> = Some("olo.terminal");
const STYLED_EDIT: Option<&str> = Some("olo.styled-edit");
const VIEWER: Option<&str> = Some("olo.viewer");
const PASSWORDS: Option<&str> = Some("olo.passwords");
const LOGS: Option<&str> = Some("olo.logs");
const ACTIVITY: Option<&str> = Some("olo.activity");
const DISK_USAGE: Option<&str> = Some("olo.disk-usage");
const NOTIFICATIONS: Option<&str> = Some("olo.notifications");
const PALETTE: Option<&str> = Some("olo.palette");
const TELEGRAM: Option<&str> = Some("olo.telegram");
const DOCTOR: Option<&str> = Some("olo.doctor");
/// Chrome's keys (its own, not Olo's: what Chrome takes on Linux is
/// [`CHROME_NATIVE`]), kept in a profile under its app id, and used for
/// every Chromium browser ([`CHROMIUM_APPS`]).
const CHROME: Option<&str> = Some("google-chrome");

/// The apps with keys of their own, as Preferences names them.
pub const APPS: &[(&str, &str)] = &[
    ("olo.tracker", "Tracker"),
    ("olo.terminal", "Terminal"),
    ("olo.styled-edit", "StyledEdit"),
    ("olo.viewer", "Viewer"),
    ("olo.passwords", "Passwords"),
    ("olo.logs", "Logs"),
    ("olo.activity", "Activity Monitor"),
    ("olo.disk-usage", "Disk Usage"),
    ("olo.drive-setup", "Drive Setup"),
    ("olo.pool", "Token Pool"),
    ("olo.screen", "Agent Screens"),
    ("olo.notifications", "Notifications"),
    ("olo.palette", "Command Palette"),
    ("olo.telegram", "Telegram"),
    ("olo.doctor", "Olo Doctor"),
    // (Slack is Chrome too, olo-slack: these are its keys as well.)
    ("google-chrome", "Chrome and Slack"),
];

/// The browsers that are Chrome underneath (their app ids): Chrome's keys
/// ([`CHROME_NATIVE`]) are theirs.
pub const CHROMIUM_APPS: &[&str] = &["google-chrome", "google-chrome-beta", "google-chrome-unstable", "chromium", "chromium-browser", "brave-browser", "microsoft-edge"];

/// What Chrome itself takes on Linux for each of its commands here: the
/// keys a profile gives one are sent as these (app_server's keymap), so a
/// Mac's ⌘Y opens History (Ctrl+H) and ⌥⌘I the developer tools. Written
/// as clients see them: Ctrl, Alt, Shift.
pub const CHROME_NATIVE: &[(&str, &str)] = &[
    ("chrome.new-tab", "Ctrl+T"),
    ("chrome.new-window", "Ctrl+N"),
    ("chrome.incognito", "Ctrl+Shift+N"),
    ("chrome.reopen-tab", "Ctrl+Shift+T"),
    ("chrome.close-tab", "Ctrl+W"),
    ("chrome.close-window", "Ctrl+Shift+W"),
    ("chrome.quit", "Ctrl+Shift+Q"),
    ("chrome.location", "Ctrl+L"),
    ("chrome.reload", "Ctrl+R"),
    ("chrome.hard-reload", "Ctrl+Shift+R"),
    ("chrome.back", "Alt+Left"),
    ("chrome.forward", "Alt+Right"),
    ("chrome.home", "Alt+Home"),
    ("chrome.find-next", "Ctrl+G"),
    ("chrome.find-previous", "Ctrl+Shift+G"),
    ("chrome.zoom-in", "Ctrl+Equal"),
    ("chrome.zoom-out", "Ctrl+Minus"),
    ("chrome.actual-size", "Ctrl+0"),
    ("chrome.bookmark", "Ctrl+D"),
    ("chrome.bookmarks-bar", "Ctrl+Shift+B"),
    ("chrome.bookmark-manager", "Ctrl+Shift+O"),
    ("chrome.history", "Ctrl+H"),
    ("chrome.downloads", "Ctrl+J"),
    ("chrome.clear-data", "Ctrl+Shift+Delete"),
    ("chrome.devtools", "Ctrl+Shift+I"),
    ("chrome.console", "Ctrl+Shift+J"),
    ("chrome.view-source", "Ctrl+U"),
    ("chrome.full-screen", "F11"),
];

/// The name of the group of every app's commands.
pub const EVERY_APP: &str = "Every Application";

/// Every app command with keys, every app's first.
pub const APP_KEYS: &[AppKey] = &[
    k("app.new", None, "new", "New", &["Cmd+N"], &["Ctrl+N"], &["Alt+N"]),
    k("app.open", None, "open", "Open", &["Cmd+O"], &["Ctrl+O"], &["Alt+O"]),
    k("app.save", None, "save", "Save", &["Cmd+S"], &["Ctrl+S"], &["Alt+S"]),
    k("app.save-as", None, "save-as", "Save As", &["Shift+Cmd+S"], &["Ctrl+Shift+S"], &["Alt+Shift+S"]),
    k("app.close", None, "close", "Close the window", &["Cmd+W"], &["Ctrl+W"], &["Alt+W"]),
    k("app.quit", None, "quit", "Quit", &["Cmd+Q"], &["Ctrl+Q"], &["Alt+Q"]),
    k("app.undo", None, "undo", "Undo", &["Cmd+Z"], &["Ctrl+Z"], &["Alt+Z"]),
    k("app.redo", None, "redo", "Redo", &["Shift+Cmd+Z"], &["Ctrl+Y"], &["Alt+Shift+Z"]),
    k("app.cut", None, "cut", "Cut", &["Cmd+X"], &["Ctrl+X"], &["Alt+X"]),
    k("app.copy", None, "copy", "Copy", &["Cmd+C"], &["Ctrl+C"], &["Alt+C"]),
    k("app.paste", None, "paste", "Paste", &["Cmd+V"], &["Ctrl+V"], &["Alt+V"]),
    k("app.select-all", None, "select-all", "Select All", &["Cmd+A"], &["Ctrl+A"], &["Alt+A"]),
    k("app.find", None, "find", "Find", &["Cmd+F"], &["Ctrl+F"], &["Alt+F"]),
    k("app.find-again", None, "find-again", "Find Again", &["Cmd+G"], &["F3"], &["Alt+G"]),
    k("app.find-selection", None, "find-selection", "Find the selection", &["Cmd+E"], &["Ctrl+F3"], &["Alt+H"]),
    k("app.new-tab", None, "new-tab", "New Tab", &["Cmd+T"], &["Ctrl+T"], &["Alt+T"]),
    k("app.bigger", None, "bigger", "Bigger text", &["Cmd+Equal"], &["Ctrl+Equal"], &["Alt+Equal"]),
    k("app.smaller", None, "smaller", "Smaller text", &["Cmd+Minus"], &["Ctrl+Minus"], &["Alt+Minus"]),
    k("app.default-size", None, "default-size", "Text at its own size", &["Cmd+0"], &["Ctrl+0"], &["Alt+0"]),
    // A side pane put away or brought back (`olo_kit::widgets::SidePane`):
    // View ▸ Hide Sidebar, ⌥⌘S as Finder's was (⌃⌘S is ⌘S to a kit app:
    // its keys have no Control of their own).
    k("app.sidebar", None, "sidebar", "Hide or show the sidebar", &["Alt+Cmd+S"], &["Ctrl+Alt+S"], &["Alt+Cmd+S"]),
    // Tracker: Finder's on a Mac, Explorer's on Windows, Tracker's on BeOS.
    // (A new window: ⌘N as Finder's, Ctrl+N as Explorer's; BeOS's Tracker
    // had none, Alt+N its New Folder.)
    k("tracker.new", TRACKER, "new", "New Window", &["Cmd+N"], &["Ctrl+N"], &[]),
    k("tracker.new-folder", TRACKER, "new-folder", "New Folder", &["Shift+Cmd+N"], &["Ctrl+Shift+N"], &["Alt+N"]),
    k("tracker.parent", TRACKER, "parent", "Open Parent", &["Cmd+Up"], &["Alt+Up"], &["Alt+Up"]),
    // In the window, as Logs' Back (and Finder's and Explorer's).
    k("tracker.back", TRACKER, "back", "Back", &["Cmd+BracketLeft"], &["Alt+Left"], &["Alt+BracketLeft"]),
    k("tracker.forward", TRACKER, "forward", "Forward", &["Cmd+BracketRight"], &["Alt+Right"], &["Alt+BracketRight"]),
    k("tracker.info", TRACKER, "info", "Get Info", &["Cmd+I"], &["Alt+Return"], &["Alt+I"]),
    k("tracker.quick-look", TRACKER, "quick-look", "Quick Look", &["Cmd+Y"], &["Ctrl+Space"], &["Alt+Y"]),
    k("tracker.rename", TRACKER, "rename", "Edit Name", &["Cmd+E"], &["F2"], &["Alt+E"]),
    k("tracker.duplicate", TRACKER, "duplicate", "Duplicate", &["Cmd+D"], &[], &["Alt+D"]),
    k("tracker.trash", TRACKER, "trash", "Move to Trash", &["Cmd+Backspace"], &["Delete"], &["Alt+T"]),
    k("tracker.invert", TRACKER, "invert", "Invert Selection", &["Shift+Cmd+I"], &[], &["Alt+Shift+S"]),
    k("tracker.view-icons", TRACKER, "view-icons", "Icon View", &["Cmd+1"], &["Ctrl+Shift+1"], &["Alt+1"]),
    k("tracker.view-mini", TRACKER, "view-mini", "Mini Icon View", &["Cmd+2"], &["Ctrl+Shift+2"], &["Alt+2"]),
    k("tracker.view-list", TRACKER, "view-list", "List View", &["Cmd+3"], &["Ctrl+Shift+3"], &["Alt+3"]),
    k("tracker.view-columns", TRACKER, "view-columns", "Column View", &["Cmd+4"], &["Ctrl+Shift+4"], &["Alt+4"]),
    // Terminal: the command key reaches it with Shift (the shell keeps
    // Ctrl), so on Windows its keys are Ctrl+Shift ones, as in Windows
    // Terminal.
    k("terminal.new", TERMINAL, "new", "New Terminal", &["Cmd+N"], &["Ctrl+Shift+N"], &["Alt+N"]),
    k("terminal.new-tab", TERMINAL, "new-tab", "New Tab", &["Cmd+T"], &["Ctrl+Shift+T"], &["Alt+T"]),
    k("terminal.close", TERMINAL, "close", "Close Tab", &["Cmd+W"], &["Ctrl+Shift+W"], &["Alt+W"]),
    k("terminal.quit", TERMINAL, "quit", "Quit", &["Cmd+Q"], &["Ctrl+Shift+Q"], &["Alt+Q"]),
    k("terminal.undo", TERMINAL, "undo", "Undo", &["Cmd+Z"], &["Ctrl+Shift+Z"], &["Alt+Z"]),
    k("terminal.redo", TERMINAL, "redo", "Redo", &["Shift+Cmd+Z"], &["Ctrl+Shift+Y"], &["Alt+Shift+Z"]),
    k("terminal.copy", TERMINAL, "copy", "Copy", &["Cmd+C"], &["Ctrl+Shift+C"], &["Alt+C"]),
    k("terminal.paste", TERMINAL, "paste", "Paste", &["Cmd+V"], &["Ctrl+Shift+V"], &["Alt+V"]),
    k("terminal.select-all", TERMINAL, "select-all", "Select All", &["Cmd+A"], &["Ctrl+Shift+A"], &["Alt+A"]),
    k("terminal.find", TERMINAL, "find", "Find", &["Cmd+F"], &["Ctrl+Shift+F"], &["Alt+F"]),
    k("terminal.find-again", TERMINAL, "find-again", "Find Again", &["Cmd+G"], &["Ctrl+Shift+G"], &["Alt+G"]),
    k("terminal.find-selection", TERMINAL, "find-selection", "Find the selection", &["Cmd+E"], &["Ctrl+Shift+E"], &["Alt+E"]),
    k("terminal.clear", TERMINAL, "clear", "Clear Scrollback", &["Cmd+K"], &["Ctrl+Shift+K"], &["Alt+K"]),
    k("terminal.tile-tabs", TERMINAL, "tile-tabs", "Show All Tabs", &["Cmd+D"], &["Ctrl+Shift+D"], &["Alt+D"]),
    k("terminal.bigger", TERMINAL, "bigger", "Bigger text", &["Cmd+Equal"], &["Ctrl+Shift+Equal"], &["Alt+Equal"]),
    k("terminal.smaller", TERMINAL, "smaller", "Smaller text", &["Cmd+Minus"], &["Ctrl+Shift+Minus"], &["Alt+Minus"]),
    k("terminal.default-size", TERMINAL, "default-size", "Text at its own size", &["Cmd+0"], &["Ctrl+Shift+0"], &["Alt+0"]),
    k("terminal.prev-tab", TERMINAL, "prev-tab", "Previous Tab", &["Shift+Cmd+BracketLeft"], &["Ctrl+Shift+BracketLeft"], &["Alt+BracketLeft"]),
    k("terminal.next-tab", TERMINAL, "next-tab", "Next Tab", &["Shift+Cmd+BracketRight"], &["Ctrl+Shift+BracketRight"], &["Alt+BracketRight"]),
    k("terminal.tab-1", TERMINAL, "tab:0", "Tab 1", &["Cmd+1"], &["Ctrl+Shift+1"], &["Alt+1"]),
    k("terminal.tab-2", TERMINAL, "tab:1", "Tab 2", &["Cmd+2"], &["Ctrl+Shift+2"], &["Alt+2"]),
    k("terminal.tab-3", TERMINAL, "tab:2", "Tab 3", &["Cmd+3"], &["Ctrl+Shift+3"], &["Alt+3"]),
    k("terminal.tab-4", TERMINAL, "tab:3", "Tab 4", &["Cmd+4"], &["Ctrl+Shift+4"], &["Alt+4"]),
    k("terminal.tab-5", TERMINAL, "tab:4", "Tab 5", &["Cmd+5"], &["Ctrl+Shift+5"], &["Alt+5"]),
    k("terminal.tab-6", TERMINAL, "tab:5", "Tab 6", &["Cmd+6"], &["Ctrl+Shift+6"], &["Alt+6"]),
    k("terminal.tab-7", TERMINAL, "tab:6", "Tab 7", &["Cmd+7"], &["Ctrl+Shift+7"], &["Alt+7"]),
    k("terminal.tab-8", TERMINAL, "tab:7", "Tab 8", &["Cmd+8"], &["Ctrl+Shift+8"], &["Alt+8"]),
    k("terminal.tab-9", TERMINAL, "tab:8", "Tab 9", &["Cmd+9"], &["Ctrl+Shift+9"], &["Alt+9"]),
    // StyledEdit: TextEdit's on a Mac, WordPad's on Windows, StyledEdit's
    // on BeOS.
    k("styled-edit.replace", STYLED_EDIT, "replace", "Replace", &["Alt+Cmd+F"], &["Ctrl+H"], &["Alt+R"]),
    k("styled-edit.replace-same", STYLED_EDIT, "replace-same", "Replace and find again", &["Alt+Cmd+G"], &["Ctrl+Shift+H"], &["Alt+T"]),
    k("styled-edit.bold", STYLED_EDIT, "bold", "Bold", &["Cmd+B"], &["Ctrl+B"], &["Alt+B"]),
    k("styled-edit.italic", STYLED_EDIT, "italic", "Italic", &["Cmd+I"], &["Ctrl+I"], &["Alt+I"]),
    k("styled-edit.underline", STYLED_EDIT, "underline", "Underline", &["Cmd+U"], &["Ctrl+U"], &["Alt+U"]),
    // Viewer: Preview's on a Mac.
    k("viewer.zoom-in", VIEWER, "zoom-in", "Zoom In", &["Cmd+Equal"], &["Ctrl+Equal"], &["Alt+Equal"]),
    k("viewer.zoom-out", VIEWER, "zoom-out", "Zoom Out", &["Cmd+Minus"], &["Ctrl+Minus"], &["Alt+Minus"]),
    k("viewer.zoom-100", VIEWER, "zoom-100", "Actual Size", &["Cmd+0"], &["Ctrl+0"], &["Alt+0"]),
    k("viewer.zoom-fit", VIEWER, "zoom-fit", "Zoom to Fit", &["Cmd+9"], &["Ctrl+9"], &["Alt+9"]),
    k("viewer.rotate-left", VIEWER, "rotate-left", "Rotate Left", &["Cmd+L"], &["Ctrl+L"], &["Alt+L"]),
    k("viewer.rotate-right", VIEWER, "rotate-right", "Rotate Right", &["Cmd+R"], &["Ctrl+R"], &["Alt+R"]),
    k("viewer.crop", VIEWER, "crop", "Crop", &["Cmd+K"], &["Ctrl+K"], &["Alt+K"]),
    k("viewer.prev-file", VIEWER, "prev-file", "Previous File", &["Cmd+BracketLeft"], &["Ctrl+BracketLeft"], &["Alt+BracketLeft"]),
    k("viewer.next-file", VIEWER, "next-file", "Next File", &["Cmd+BracketRight"], &["Ctrl+BracketRight"], &["Alt+BracketRight"]),
    k("viewer.tool-select", VIEWER, "tool-select", "Select tool", &["Cmd+1"], &["Ctrl+1"], &["Alt+1"]),
    k("viewer.tool-text", VIEWER, "tool-text", "Text tool", &["Cmd+2"], &["Ctrl+2"], &["Alt+2"]),
    k("viewer.tool-draw", VIEWER, "tool-draw", "Draw tool", &["Cmd+3"], &["Ctrl+3"], &["Alt+3"]),
    k("passwords.new-login", PASSWORDS, "new:Login", "New Login", &["Cmd+N"], &["Ctrl+N"], &["Alt+N"]),
    k("passwords.generator", PASSWORDS, "generator", "Password Generator", &["Cmd+G"], &["Ctrl+G"], &["Alt+G"]),
    k("passwords.lock", PASSWORDS, "lock", "Lock", &["Cmd+L"], &["Ctrl+L"], &["Alt+L"]),
    k("passwords.copy-username", PASSWORDS, "copy:username", "Copy Username", &["Cmd+C"], &["Ctrl+C"], &["Alt+C"]),
    k("passwords.copy-password", PASSWORDS, "copy:password", "Copy Password", &["Shift+Cmd+C"], &["Ctrl+Shift+C"], &["Alt+Shift+C"]),
    k("passwords.copy-otp", PASSWORDS, "copy:otp", "Copy One-Time Password", &["Shift+Cmd+O"], &["Ctrl+Shift+O"], &["Alt+Shift+O"]),
    k("passwords.edit", PASSWORDS, "edit", "Edit", &["Cmd+E"], &["Ctrl+E"], &["Alt+E"]),
    k("passwords.all", PASSWORDS, "source:all", "All Items", &["Cmd+0"], &["Ctrl+0"], &["Alt+0"]),
    k("passwords.vault-1", PASSWORDS, "vault:0", "Vault 1", &["Cmd+1"], &["Ctrl+1"], &["Alt+1"]),
    k("passwords.vault-2", PASSWORDS, "vault:1", "Vault 2", &["Cmd+2"], &["Ctrl+2"], &["Alt+2"]),
    k("passwords.vault-3", PASSWORDS, "vault:2", "Vault 3", &["Cmd+3"], &["Ctrl+3"], &["Alt+3"]),
    k("passwords.vault-4", PASSWORDS, "vault:3", "Vault 4", &["Cmd+4"], &["Ctrl+4"], &["Alt+4"]),
    k("passwords.vault-5", PASSWORDS, "vault:4", "Vault 5", &["Cmd+5"], &["Ctrl+5"], &["Alt+5"]),
    k("passwords.vault-6", PASSWORDS, "vault:5", "Vault 6", &["Cmd+6"], &["Ctrl+6"], &["Alt+6"]),
    k("passwords.vault-7", PASSWORDS, "vault:6", "Vault 7", &["Cmd+7"], &["Ctrl+7"], &["Alt+7"]),
    k("passwords.vault-8", PASSWORDS, "vault:7", "Vault 8", &["Cmd+8"], &["Ctrl+8"], &["Alt+8"]),
    k("passwords.vault-9", PASSWORDS, "vault:8", "Vault 9", &["Cmd+9"], &["Ctrl+9"], &["Alt+9"]),
    k("logs.open-file", LOGS, "open-file", "Open Log File…", &["Cmd+O"], &["Ctrl+O"], &["Alt+O"]),
    k("logs.live", LOGS, "live", "Live", &["Cmd+L"], &["Ctrl+L"], &["Alt+L"]),
    k("logs.end", LOGS, "end", "Go to End", &["Cmd+Down"], &["Ctrl+End"], &["Alt+End"]),
    k("logs.context", LOGS, "context", "Show in Context", &["Cmd+K"], &["Ctrl+K"], &["Alt+K"]),
    k("logs.back", LOGS, "back", "Back", &["Cmd+BracketLeft"], &["Alt+Left"], &["Alt+BracketLeft"]),
    k("activity.overview", ACTIVITY, "section:overview", "Overview", &["Cmd+1"], &["Ctrl+1"], &["Alt+1"]),
    k("activity.processes", ACTIVITY, "section:processes", "Processes", &["Cmd+2"], &["Ctrl+2"], &["Alt+2"]),
    k("activity.cpu", ACTIVITY, "section:cpu", "CPU", &["Cmd+3"], &["Ctrl+3"], &["Alt+3"]),
    k("activity.memory", ACTIVITY, "section:memory", "Memory", &["Cmd+4"], &["Ctrl+4"], &["Alt+4"]),
    k("activity.disks", ACTIVITY, "section:disks", "Disks", &["Cmd+5"], &["Ctrl+5"], &["Alt+5"]),
    k("activity.network", ACTIVITY, "section:network", "Network", &["Cmd+6"], &["Ctrl+6"], &["Alt+6"]),
    k("activity.gpu", ACTIVITY, "section:gpu", "GPU", &["Cmd+7"], &["Ctrl+7"], &["Alt+7"]),
    k("activity.sensors", ACTIVITY, "section:sensors", "Sensors", &["Cmd+8"], &["Ctrl+8"], &["Alt+8"]),
    k("activity.system", ACTIVITY, "section:system", "System", &["Cmd+9"], &["Ctrl+9"], &["Alt+9"]),
    k("activity.inspect", ACTIVITY, "inspect-process", "Inspect Process", &["Cmd+I"], &["Ctrl+I"], &["Alt+I"]),
    k("disk-usage.scan-home", DISK_USAGE, "scan-home", "Scan Home", &["Shift+Cmd+H"], &["Alt+Home"], &["Alt+H"]),
    k("disk-usage.scan-folder", DISK_USAGE, "scan-folder", "Scan a Folder", &["Cmd+O"], &["Ctrl+O"], &["Alt+O"]),
    k("disk-usage.rescan", DISK_USAGE, "rescan", "Rescan", &["Cmd+R"], &["F5"], &["Alt+R"]),
    k("disk-usage.stop", DISK_USAGE, "stop", "Stop Scanning", &["Cmd+Period"], &["Ctrl+Period"], &["Alt+Period"]),
    k("disk-usage.collect", DISK_USAGE, "collect", "Add to Collector", &["Cmd+D"], &["Ctrl+D"], &["Alt+D"]),
    k("disk-usage.trash", DISK_USAGE, "trash", "Move to Trash", &["Cmd+Backspace"], &["Delete"], &["Alt+T"]),
    k("disk-usage.up", DISK_USAGE, "up", "Up a level", &["Cmd+Up"], &["Alt+Up"], &["Alt+Up"]),
    k("drive-setup.eject", Some("olo.drive-setup"), "eject", "Eject", &["Cmd+E"], &["Ctrl+E"], &["Alt+E"]),
    k("doctor.check", DOCTOR, "check", "Check Again", &["Cmd+R"], &["F5"], &["Alt+R"]),
    k("doctor.fix-all", DOCTOR, "fix-all", "Fix All", &["Shift+Cmd+F"], &["Ctrl+Shift+F"], &["Shift+Alt+F"]),
    k("pool.refresh", Some("olo.pool"), "refresh", "Read Quota Now", &["Cmd+R"], &["F5"], &["Alt+R"]),
    k("screen.control", Some("olo.screen"), "control", "Take or give back control", &["Cmd+K"], &["Ctrl+K"], &["Alt+K"]),
    k("screen.export", Some("olo.screen"), "export", "Export Video", &["Cmd+E"], &["Ctrl+E"], &["Alt+E"]),
    k("screen.tab-live", Some("olo.screen"), "tab-live", "Live", &["Cmd+1"], &["Ctrl+1"], &["Alt+1"]),
    k("screen.tab-history", Some("olo.screen"), "tab-history", "History", &["Cmd+2"], &["Ctrl+2"], &["Alt+2"]),
    k("screen.tab-stats", Some("olo.screen"), "tab-stats", "Stats", &["Cmd+3"], &["Ctrl+3"], &["Alt+3"]),
    k("notifications.toggle-read", NOTIFICATIONS, "toggle-read", "Mark as Read or Unread", &["Cmd+U"], &["Ctrl+U"], &["Alt+U"]),
    k("notifications.view-list", NOTIFICATIONS, "view-list", "List", &["Cmd+1"], &["Ctrl+1"], &["Alt+1"]),
    k("notifications.view-apps", NOTIFICATIONS, "view-apps", "By App", &["Cmd+2"], &["Ctrl+2"], &["Alt+2"]),
    k("notifications.view-cards", NOTIFICATIONS, "view-cards", "Cards", &["Cmd+3"], &["Ctrl+3"], &["Alt+3"]),
    k("notifications.view-conversations", NOTIFICATIONS, "view-conversations", "Conversations", &["Cmd+4"], &["Ctrl+4"], &["Alt+4"]),
    k("palette.alternate", PALETTE, "alternate", "Do the result's other action", &["Cmd+Return"], &["Ctrl+Return"], &["Alt+Return"]),
    // Telegram: Telegram Desktop's where it has them.
    k("telegram.contacts", TELEGRAM, "contacts", "Contacts", &["Cmd+K"], &["Ctrl+K"], &["Alt+K"]),
    k("telegram.attach", TELEGRAM, "attach", "Attach Files", &["Cmd+O"], &["Ctrl+O"], &["Alt+O"]),
    k("telegram.info", TELEGRAM, "info", "Chat Info", &["Cmd+I"], &["Ctrl+I"], &["Alt+I"]),
    k("telegram.all-chats", TELEGRAM, "folder:all", "All Chats", &["Cmd+0"], &["Ctrl+0"], &["Alt+0"]),
    k("telegram.folder-1", TELEGRAM, "folder:1", "First folder", &["Cmd+1"], &["Ctrl+1"], &["Alt+1"]),
    k("telegram.folder-2", TELEGRAM, "folder:2", "Second folder", &["Cmd+2"], &["Ctrl+2"], &["Alt+2"]),
    k("telegram.folder-3", TELEGRAM, "folder:3", "Third folder", &["Cmd+3"], &["Ctrl+3"], &["Alt+3"]),
    k("telegram.folder-4", TELEGRAM, "folder:4", "Fourth folder", &["Cmd+4"], &["Ctrl+4"], &["Alt+4"]),
    k("telegram.folder-5", TELEGRAM, "folder:5", "Fifth folder", &["Cmd+5"], &["Ctrl+5"], &["Alt+5"]),
    k("telegram.folder-6", TELEGRAM, "folder:6", "Sixth folder", &["Cmd+6"], &["Ctrl+6"], &["Alt+6"]),
    k("telegram.folder-7", TELEGRAM, "folder:7", "Seventh folder", &["Cmd+7"], &["Ctrl+7"], &["Alt+7"]),
    k("telegram.folder-8", TELEGRAM, "folder:8", "Eighth folder", &["Cmd+8"], &["Ctrl+8"], &["Alt+8"]),
    k("telegram.archive", TELEGRAM, "archive", "Archived Chats", &["Cmd+9"], &["Ctrl+9"], &["Alt+9"]),
    k("telegram.prev", TELEGRAM, "prev", "Previous Chat", &["Cmd+Up"], &["Alt+Up"], &["Alt+Up"]),
    k("telegram.next", TELEGRAM, "next", "Next Chat", &["Cmd+Down"], &["Alt+Down"], &["Alt+Down"]),
    // Chrome's own, each platform's as Chrome has them there (BeOS's as a
    // Mac's, Alt its command key); sent as Linux Chrome's (CHROME_NATIVE).
    k("chrome.new-tab", CHROME, "new-tab", "New Tab", &["Cmd+T"], &["Ctrl+T"], &["Alt+T"]),
    k("chrome.new-window", CHROME, "new-window", "New Window", &["Cmd+N"], &["Ctrl+N"], &["Alt+N"]),
    k("chrome.incognito", CHROME, "incognito", "New Incognito Window", &["Shift+Cmd+N"], &["Ctrl+Shift+N"], &["Alt+Shift+N"]),
    k("chrome.reopen-tab", CHROME, "reopen-tab", "Reopen Closed Tab", &["Shift+Cmd+T"], &["Ctrl+Shift+T"], &["Alt+Shift+T"]),
    k("chrome.close-tab", CHROME, "close-tab", "Close Tab", &["Cmd+W"], &["Ctrl+W"], &["Alt+W"]),
    k("chrome.close-window", CHROME, "close-window", "Close Window", &["Shift+Cmd+W"], &["Ctrl+Shift+W"], &["Alt+Shift+W"]),
    k("chrome.quit", CHROME, "quit", "Quit Chrome", &["Cmd+Q"], &["Ctrl+Shift+Q"], &["Alt+Q"]),
    k("chrome.location", CHROME, "location", "Open Location", &["Cmd+L"], &["Ctrl+L"], &["Alt+L"]),
    k("chrome.reload", CHROME, "reload", "Reload", &["Cmd+R"], &["Ctrl+R", "F5"], &["Alt+R"]),
    k("chrome.hard-reload", CHROME, "hard-reload", "Reload Without the Cache", &["Shift+Cmd+R"], &["Ctrl+Shift+R"], &["Alt+Shift+R"]),
    k("chrome.back", CHROME, "back", "Back", &["Cmd+BracketLeft"], &["Alt+Left"], &["Alt+BracketLeft"]),
    k("chrome.forward", CHROME, "forward", "Forward", &["Cmd+BracketRight"], &["Alt+Right"], &["Alt+BracketRight"]),
    k("chrome.home", CHROME, "home", "Home Page", &["Shift+Cmd+H"], &["Alt+Home"], &["Alt+Shift+H"]),
    k("chrome.find-next", CHROME, "find-next", "Find Next", &["Cmd+G"], &["Ctrl+G"], &["Alt+G"]),
    k("chrome.find-previous", CHROME, "find-previous", "Find Previous", &["Shift+Cmd+G"], &["Ctrl+Shift+G"], &["Alt+Shift+G"]),
    k("chrome.zoom-in", CHROME, "zoom-in", "Zoom In", &["Cmd+Equal"], &["Ctrl+Equal"], &["Alt+Equal"]),
    k("chrome.zoom-out", CHROME, "zoom-out", "Zoom Out", &["Cmd+Minus"], &["Ctrl+Minus"], &["Alt+Minus"]),
    k("chrome.actual-size", CHROME, "actual-size", "Actual Size", &["Cmd+0"], &["Ctrl+0"], &["Alt+0"]),
    k("chrome.bookmark", CHROME, "bookmark", "Bookmark This Tab", &["Cmd+D"], &["Ctrl+D"], &["Alt+D"]),
    k("chrome.bookmarks-bar", CHROME, "bookmarks-bar", "Show Bookmarks Bar", &["Shift+Cmd+B"], &["Ctrl+Shift+B"], &["Alt+Shift+B"]),
    k("chrome.bookmark-manager", CHROME, "bookmark-manager", "Bookmark Manager", &["Alt+Cmd+B"], &["Ctrl+Shift+O"], &["Alt+Shift+O"]),
    k("chrome.history", CHROME, "history", "History", &["Cmd+Y"], &["Ctrl+H"], &["Alt+Y"]),
    k("chrome.downloads", CHROME, "downloads", "Downloads", &["Alt+Cmd+L"], &["Ctrl+J"], &["Alt+Shift+L"]),
    k("chrome.clear-data", CHROME, "clear-data", "Clear Browsing Data", &["Shift+Cmd+Backspace"], &["Ctrl+Shift+Delete"], &["Alt+Shift+Backspace"]),
    k("chrome.devtools", CHROME, "devtools", "Developer Tools", &["Alt+Cmd+I"], &["Ctrl+Shift+I", "F12"], &["Alt+Shift+I"]),
    k("chrome.console", CHROME, "console", "JavaScript Console", &["Alt+Cmd+J"], &["Ctrl+Shift+J"], &["Alt+Shift+J"]),
    k("chrome.view-source", CHROME, "view-source", "View Source", &["Alt+Cmd+U"], &["Ctrl+U"], &["Alt+U"]),
    k("chrome.full-screen", CHROME, "full-screen", "Enter Full Screen", &["Ctrl+Cmd+F"], &["F11"], &["Alt+Shift+F"]),
];

/// For a profile: each of Chrome's keys it has (its own, changed or not)
/// and what Chrome takes for that command on Linux ([`CHROME_NATIVE`]).
pub fn chrome_keys(profile: &crate::shortcuts::KeyboardProfile) -> Vec<(crate::Chord, crate::Chord)> {
    let mut out = Vec::new();
    for (id, native) in CHROME_NATIVE {
        let (Some(e), Ok(to)) = (crate::shortcuts::entry(id), native.parse::<crate::Chord>()) else { continue };
        for from in e.keys(profile) {
            out.push((from, to));
        }
    }
    out
}

/// The entry for `command` in app `app`: the app's own, else every app's.
pub fn app_key(app: &str, command: &str) -> Option<&'static AppKey> {
    APP_KEYS.iter().find(|e| e.app == Some(app) && e.command == command).or_else(|| APP_KEYS.iter().find(|e| e.app.is_none() && e.command == command))
}

/// The entry with this id.
pub fn by_id(id: &str) -> Option<&'static AppKey> {
    APP_KEYS.iter().find(|e| e.id == id)
}

/// The app's name, as Preferences shows it ([`EVERY_APP`] for every app's).
pub fn app_title(app: Option<&str>) -> &'static str {
    match app {
        None => EVERY_APP,
        Some(id) => APPS.iter().find(|(a, _)| *a == id).map_or("Other", |(_, t)| t),
    }
}
