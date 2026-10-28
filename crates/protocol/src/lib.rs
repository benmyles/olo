//! The private IPC between `app_server` and Olo's own apps (Deskbar,
//! Preferences, ...). Plain Wayland clients never need it.
//!
//! Transport: newline-delimited JSON over the Unix socket named by
//! `$OLO_SOCKET` (set by app_server for every child it starts). Each line a
//! client writes is a [`Request`]; app_server answers each with exactly one
//! [`Reply`] line. After [`Request::Subscribe`] the connection also receives
//! unsolicited [`Reply::Event`] lines.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod notifications;
pub mod shortcuts;
pub mod touchpad;
pub use notifications::{AppNotifications, HistoryView, NotificationSettings, NotificationState};
pub use shortcuts::{Chord, Shortcut, ShortcutSettings, Tile, TilingSettings};

pub const SOCKET_ENV: &str = "OLO_SOCKET";

/// Opaque, stable for the lifetime of a window.
pub type WindowId = u64;

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // one message at a time over a socket
pub enum Request {
    Ping,
    /// Start receiving [`Event`]s on this connection.
    Subscribe,
    ListWindows,
    Activate { id: WindowId },
    /// An xdg-activation token for a program about to be started (as
    /// `XDG_ACTIVATION_TOKEN`), so the window it opens, or the running
    /// instance it hands its work to (a link opening as a new tab in
    /// Chrome), comes forward, from whatever workspace. For launchers run
    /// without a click to take a token from (a link opened from a shell).
    ActivationToken,
    /// BeOS "minimize": hide the window; Deskbar shows it greyed out.
    Minimize { id: WindowId },
    Close { id: WindowId },
    ToggleZoom { id: WindowId },
    /// Push a window behind all the others (BeOS's right-click on the tab).
    SendToBack { id: WindowId },
    /// Put a window's frame (tab and border included, as
    /// [`WindowInfo::frame`]) here, at this size; no longer zoomed. It
    /// may hang off a display's edge, but no further than a person could
    /// leave it: its top edge on the display, enough of it to take hold of.
    Place { id: WindowId, frame: Frame },
    Workspaces,
    SwitchWorkspace { index: u32 },
    MoveToWorkspace { id: WindowId, index: u32 },
    Outputs,
    ConfigureOutput { name: String, config: OutputConfig },
    /// The pointing devices plugged in: touchpads (with the kind of pad and
    /// the profile that suits it) and mice.
    InputDevices,
    GetSettings,
    /// The keyboard hints showing now (Preferences ▸ Keyboard): each
    /// letter, where it is and what it picks.
    Hints,
    SetSettings { settings: Settings },
    /// Save a PNG of one screen (`output`, or the one the pointer is on)
    /// or of one window, answering once the file is complete.
    Screenshot {
        path: String,
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        window: Option<WindowId>,
        /// With `window`: include its tab and border.
        #[serde(default = "yes")]
        border: bool,
        /// Draw the pointer in.
        #[serde(default)]
        pointer: bool,
        /// PNG, or raw pixels for a live view (see [`ImageFormat`]).
        #[serde(default)]
        format: ImageFormat,
    },
    /// Lock the screen now.
    Lock,
    /// Start the screen saver now (Preferences' Test).
    StartScreenSaver,
    /// Idle state, caffeinated mode and the built-in display's brightness.
    PowerStatus,
    /// Keep the machine awake (no display sleep, system sleep, screen
    /// saver or lock) until turned off, or for `minutes`. With `lid`,
    /// closing the lid doesn't sleep either. Not persisted.
    SetCaffeinated {
        on: bool,
        #[serde(default)]
        minutes: Option<u32>,
        #[serde(default)]
        lid: bool,
    },
    /// Built-in display brightness, 0.0–1.0 (perceptual).
    SetBrightness { level: f32 },
    /// A display's brightness (`name` its connector), kept for it.
    SetOutputBrightness { name: String, level: f32 },
    /// Keyboard backlight, 0.0–1.0 (perceptual).
    SetKeyboardBacklight { level: f32 },
    /// Show each display's name on it for a few seconds.
    IdentifyOutputs,
    /// Take the next key pressed (with its modifiers) as a shortcut rather
    /// than acting on it; it comes back as [`Event::ShortcutRecorded`].
    /// `on: false` stops waiting.
    RecordShortcut { on: bool },
    /// Do one of the shortcuts (its id in [`shortcuts::SHORTCUTS`]), to
    /// `window` rather than the focused one if given (the command palette
    /// holds the keyboard while it is open).
    RunShortcut {
        id: String,
        #[serde(default)]
        window: Option<WindowId>,
    },
    /// Tile the resizable windows of app `app_id` on this workspace in a
    /// grid on the active display, in front of the rest.
    TileApp { app_id: String },
    /// The active display's tiling area and the windows on it (front
    /// first, with frames), for an arranger such as Smart Tiling.
    Arrangement,
    /// Put windows of the active display in parts of its tiling area,
    /// tiled there (dragging an edge resizes the neighbours) and undoable
    /// with Undo Window Arrangement. Windows not listed stay as they are.
    Arrange { windows: Vec<Placement> },
    /// Inject input as if from a device (for automated UI tests).
    Synthetic { events: Vec<SyntheticEvent> },
    Quit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyntheticEvent {
    /// Move the pointer to global logical coordinates.
    Move { x: f64, y: f64 },
    /// evdev button code (0x110 left, 0x111 right).
    Button { button: u32, pressed: bool },
    /// evdev key code (e.g. 30 = A, 29 = left Ctrl).
    Key { key: u32, pressed: bool },
    /// Scroll by logical pixels, as on a touchpad (0, 0: the fingers lift).
    Scroll { dx: f64, dy: f64 },
    /// Turn a mouse wheel by whole notches (positive: down / right).
    Wheel {
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Ok,
    Pong { version: String },
    Error { message: String },
    ActivationToken { token: String },
    Windows { windows: Vec<WindowInfo> },
    Workspaces { current: u32, count: u32 },
    Outputs { outputs: Vec<OutputInfo> },
    InputDevices { devices: Vec<touchpad::InputDeviceInfo> },
    Settings { settings: Settings },
    /// The keyboard hints showing now (none when they're off).
    Hints { hints: Vec<HintInfo> },
    /// [`Request::Arrangement`]'s answer: the display's connector, the
    /// area tiling uses (global logical pixels: the display less Deskbar
    /// and the margins), and its windows on this workspace, front first.
    Arrangement { output: String, area: Frame, windows: Vec<WindowInfo> },
    Power { status: PowerStatus },
    Event { event: Event },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    WindowsChanged { windows: Vec<WindowInfo> },
    WorkspaceChanged { current: u32, count: u32 },
    SettingsChanged { settings: Settings },
    OutputsChanged { outputs: Vec<OutputInfo> },
    /// Caffeinated, display asleep, brightness or app inhibitors changed.
    PowerChanged { status: PowerStatus },
    /// The key pressed after [`Request::RecordShortcut`], as a [`Chord`]
    /// ("Shift+Cmd+Left"); "" for Backspace or Delete alone (no keys);
    /// None if it was cancelled (Escape alone, or nothing for a while).
    ShortcutRecorded { keys: Option<String> },
}

/// What app_server's power management is doing now.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PowerStatus {
    pub caffeinated: bool,
    /// Seconds since the last input (or since something kept us awake).
    pub idle_seconds: u64,
    /// The displays are off for idleness.
    pub display_asleep: bool,
    /// Apps asking to stay awake (video players, presentations).
    pub app_inhibitors: u32,
    /// The built-in display's backlight, 0.0–1.0, if there is one.
    pub brightness: Option<f32>,
    /// The screen saver is running.
    #[serde(default)]
    pub screen_saver: bool,
    /// Caffeinated for a set time: when it ends (seconds since the epoch).
    pub caffeinated_until: Option<u64>,
    /// Caffeinated with the lid closed too.
    pub caffeinated_lid: bool,
    /// The keyboard's backlight, 0.0–1.0, if it has one.
    pub keyboard_backlight: Option<f32>,
    /// The screen is locked.
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub app_id: String,
    pub title: String,
    pub pid: Option<i32>,
    pub workspace: u32,
    pub minimized: bool,
    pub focused: bool,
    /// Rolled up to just its tab.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rolled: bool,
    /// Zoomed (its zoom box's state): filling the tiling area until it is
    /// zoomed again or restored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub zoomed: bool,
    /// Its program keeps it through a window server restart (an upgrade):
    /// Olo's own apps, and Wayland programs that came through
    /// olo-wlproxy.
    #[serde(default)]
    pub kept: bool,
    /// Where it is on screen, tab and border included (global logical
    /// pixels), while it is on screen. Only in [`Reply::Windows`] (the
    /// list [`Event::WindowsChanged`] carries leaves it out, or a window
    /// being dragged would change it every frame).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Frame>,
    /// Where what the app draws is (inside the tab and border), likewise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Frame>,
    /// Its open popups (menus, dropdowns, tooltips), submenus after the
    /// menus they open from, likewise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub popups: Vec<Frame>,
}

/// A rectangle in global logical pixels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Where [`Request::Arrange`] puts a window: fractions (0–1) of the
/// tiling area, from its top left.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Placement {
    pub id: WindowId,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// How [`Request::Screenshot`] writes its picture.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    #[default]
    Png,
    /// Width and height (little-endian u32 each), then the rows of
    /// straight (not premultiplied) RGBA, top first: no encoding, for a
    /// live view.
    Rgba,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    /// millihertz
    pub refresh: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputInfo {
    /// The connector ("eDP-1", "DP-2").
    pub name: String,
    /// Where its settings are kept (see [`display_key`]).
    #[serde(default)]
    pub key: String,
    pub make: String,
    pub model: String,
    pub physical_size_mm: (i32, i32),
    pub modes: Vec<Mode>,
    pub current_mode: Option<Mode>,
    pub scale: f64,
    pub position: (i32, i32),
    /// Clockwise rotation in degrees: 0, 90, 180 or 270.
    #[serde(default)]
    pub rotation: u16,
    /// The main display: Deskbar and the desktop live here.
    #[serde(default)]
    pub main: bool,
    /// The display this one mirrors (its connector), if it does.
    #[serde(default)]
    pub mirror_of: Option<String>,
    /// Its brightness, 0–1, when it can be set from here.
    #[serde(default)]
    pub brightness: Option<f32>,
}

impl OutputInfo {
    /// What people call this display: "Built-in Display", the monitor's
    /// model, or its connector.
    pub fn display_name(&self) -> String {
        let model = self.model.trim();
        if self.name.starts_with("eDP") || self.name.starts_with("LVDS") {
            "Built-in Display".into()
        } else if !model.is_empty() && model != "Unknown" {
            model.to_owned()
        } else {
            self.name.clone()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputConfig {
    pub mode: Option<Mode>,
    pub scale: Option<f64>,
    pub position: Option<(i32, i32)>,
    /// Clockwise degrees: 0, 90, 180 or 270.
    #[serde(default)]
    pub rotation: Option<u16>,
    /// Show another display's picture (its key) instead of extending the
    /// desktop. In `ConfigureOutput`, `Some("")` goes back to extending.
    #[serde(default)]
    pub mirror: Option<String>,
    /// Brightness, 0–1, for a display whose brightness can be set (the
    /// built-in one, a USB-controlled monitor). None: full brightness.
    #[serde(default)]
    pub brightness: Option<f32>,
}

impl OutputConfig {
    /// These settings, with what they leave unset taken from `other`.
    pub fn or(self, other: &OutputConfig) -> OutputConfig {
        OutputConfig {
            mode: self.mode.or_else(|| other.mode.clone()),
            scale: self.scale.or(other.scale),
            position: self.position.or(other.position),
            rotation: self.rotation.or(other.rotation),
            mirror: self.mirror.or_else(|| other.mirror.clone()),
            brightness: self.brightness.or(other.brightness),
        }
    }
}

/// The rotations a display can have, clockwise degrees.
pub const ROTATIONS: [u16; 4] = [0, 90, 180, 270];

/// Everything Preferences can change live in app_server. Persisted by
/// app_server in `$XDG_CONFIG_HOME/olo/app_server.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub workspace_count: u32,
    /// Per-workspace desktop colour, BeOS-style (Backgrounds/Workspaces prefs).
    pub workspace_colors: Vec<[u8; 3]>,
    /// The wallpaper: `pack:NAME` (the wallpaper pack, which follows dark
    /// mode and each screen's shape) or a picture file; None for just the
    /// workspace colour.
    pub wallpaper: Option<String>,
    pub focus_follows_mouse: bool,
    pub touchpad: TouchpadSettings,
    pub keyboard: KeyboardSettings,
    pub mouse_speed: f64,
    /// Font size (logical pixels) used for window tab titles.
    pub tab_font_size: f32,
    /// A tab shows at most this many characters of its window's title,
    /// then an ellipsis.
    pub tab_title_chars: u32,
    pub appearance: Appearance,
    pub display: DisplaySettings,
    pub power: PowerSettings,
    pub screen_saver: ScreenSaverSettings,
    pub clock: ClockSettings,
    /// app_server's keyboard shortcuts changed from their defaults.
    pub shortcuts: ShortcutSettings,
    pub tiling: TilingSettings,
    pub tracker: TrackerSettings,
    pub security: SecuritySettings,
    pub ai: AiSettings,
}

/// Preferences ▸ AI: how Olo reaches models, and its AI features.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AiSettings {
    /// Claude models through the Token Pool (its accounts, spent in its
    /// order); off: straight to Anthropic with the saved API key. None is
    /// offered now: OpenAI's always go through the pool.
    pub use_pool: bool,
    /// Smart Tiling: the model arranges the windows on the screen.
    pub smart_tiling: AiFeature,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings { use_pool: true, smart_tiling: AiFeature { enabled: true, model: AI_MODELS[0].0.into(), effort: "medium".into() } }
    }
}

/// One AI feature: whether it is on, and the model and effort it uses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AiFeature {
    pub enabled: bool,
    /// A model id ([`AI_MODELS`]).
    pub model: String,
    /// How hard it thinks ([`AI_EFFORTS`]).
    pub effort: String,
}

impl Default for AiFeature {
    fn default() -> Self {
        AiSettings::default().smart_tiling
    }
}

impl AiFeature {
    /// The model it uses: the one chosen, or the default if that isn't
    /// offered any more (a Claude model, chosen before they were dropped).
    pub fn model_id(&self) -> &str {
        AI_MODELS.iter().find(|(id, _)| *id == self.model).map_or(AI_MODELS[0].0, |(id, _)| id)
    }

    /// Its model's name.
    pub fn model_name(&self) -> &str {
        AI_MODELS.iter().find(|(id, _)| *id == self.model_id()).map_or(AI_MODELS[0].1, |(_, name)| name)
    }
}

/// The models AI features can use, the default first: id, name. OpenAI's
/// current ones for Codex, reached through the Token Pool's Codex
/// accounts.
pub const AI_MODELS: &[(&str, &str)] = &[
    ("gpt-6-astra", "Astra 6"),
    ("gpt-6.1-sol", "Sol 6.1"),
    ("gpt-6.1-terra", "Terra 6.1"),
    ("gpt-6-luna", "Luna 6"),
];

/// Effort levels, least first: id, name.
pub const AI_EFFORTS: &[(&str, &str)] = &[("low", "Low"), ("medium", "Medium"), ("high", "High"), ("xhigh", "Extra High"), ("max", "Max")];

/// Preferences ▸ Security.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SecuritySettings {
    pub passwords: PasswordsSecurity,
}

/// How Passwords keeps its secrets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PasswordsSecurity {
    /// Keep in step with 1Password on its own (on unlocking, every few
    /// minutes and soon after a change); off: only with File ▸ Sync Now.
    pub auto_sync: bool,
    /// Lock after this many minutes without use (0: only when asked).
    pub lock_minutes: u32,
    /// Lock when the screen locks.
    pub lock_with_screen: bool,
    /// Take copied secrets off the clipboard after this many seconds.
    pub clear_seconds: u32,
    /// Olo's `op` (1Password's command-line tool, answered by
    /// Passwords) is installed and answers.
    pub op_cli: bool,
}

impl Default for PasswordsSecurity {
    fn default() -> Self {
        PasswordsSecurity { auto_sync: true, lock_minutes: 10, lock_with_screen: true, clear_seconds: 90, op_cli: true }
    }
}

/// Tracker's settings (Preferences ▸ Tracker).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TrackerSettings {
    /// How a folder window shows its folder when it opens.
    pub default_view: FolderView,
}

/// How a Tracker window shows its folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FolderView {
    Icons,
    MiniIcons,
    List,
    /// macOS-style columns: each selected folder opens in the next column.
    #[default]
    Columns,
}

/// How Deskbar shows the time (Preferences ▸ Date & Time).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ClockSettings {
    /// 13:05 rather than 1:05 PM.
    pub twenty_four_hour: bool,
    pub show_seconds: bool,
    /// "Mon 1:05 PM".
    pub show_day: bool,
}

impl ClockSettings {
    /// The time as Deskbar shows it; `weekday` counts from Sunday = 0.
    pub fn format(&self, hour: u32, minute: u32, second: u32, weekday: u32) -> String {
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        let seconds = if self.show_seconds { format!(":{:02}", second.min(59)) } else { String::new() };
        let time = if self.twenty_four_hour {
            format!("{hour:02}:{minute:02}{seconds}")
        } else {
            let h = hour % 12;
            format!("{}:{minute:02}{seconds} {}", if h == 0 { 12 } else { h }, if hour < 12 { "AM" } else { "PM" })
        };
        if self.show_day { format!("{} {time}", DAYS[weekday as usize % 7]) } else { time }
    }
}

/// A corner of the screen (BeOS's "Fade now" / "Don't fade" monitors).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// The screen saver (Preferences ▸ Screen Saver, after R5's Fade and
/// Modules tabs). Turning the screen off is `PowerSettings`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ScreenSaverSettings {
    pub enabled: bool,
    /// A module from olo-screensavers ("SuperString", …).
    pub module: String,
    /// Minutes of idleness before it starts.
    pub run_minutes: u32,
    /// Lock the screen once idle this long (while the saver runs).
    pub lock: bool,
    pub lock_minutes: u32,
    /// Pointer in this corner: start now.
    pub fade_now: Option<Corner>,
    /// Pointer in this corner: never start.
    pub never_fade: Option<Corner>,
    /// Each module's own settings.
    pub options: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

impl Default for ScreenSaverSettings {
    fn default() -> Self {
        ScreenSaverSettings {
            enabled: true,
            module: "SuperString".into(),
            run_minutes: 5,
            lock: false,
            lock_minutes: 5,
            fade_now: None,
            never_fade: None,
            options: Default::default(),
        }
    }
}

/// When an idle machine turns off its displays and goes to sleep.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PowerSettings {
    /// Minutes of idleness before the displays turn off; 0 is never.
    pub display_off_minutes: u32,
    /// Minutes of idleness before the machine sleeps; 0 is never.
    pub sleep_minutes: u32,
    /// Only sleep for idleness on battery, not when plugged in.
    pub sleep_on_battery_only: bool,
    /// Lock the screen when the displays turn off for idleness. (Sleep
    /// always locks.)
    pub lock_on_display_off: bool,
}

impl Default for PowerSettings {
    fn default() -> Self {
        PowerSettings { display_off_minutes: 10, sleep_minutes: 30, sleep_on_battery_only: false, lock_on_display_off: true }
    }
}

/// Screen scaling. Olo is HiDPI-first: everything is laid out in logical
/// pixels and rendered at the output's scale (2x "Retina" by default).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DisplaySettings {
    /// Scale for outputs without their own entry in `outputs`.
    pub scale: f64,
    /// The main display's key (Deskbar and the desktop live there); None,
    /// or one that isn't connected, means the first.
    pub main: Option<String>,
    /// Per-display overrides, keyed by [`display_key`]: the monitor itself
    /// when its EDID names it, else the connector ("eDP-1"). Entries saved
    /// under a connector by older versions still apply, and move to the
    /// monitor's key when next changed.
    pub outputs: std::collections::BTreeMap<String, OutputConfig>,
}

/// Where a display's settings are kept: "make model serial" from its EDID
/// when all three are known, so they follow the monitor between ports;
/// else its connector.
pub fn display_key(connector: &str, make: &str, model: &str, serial: &str) -> String {
    let known = |s: &str| !s.trim().is_empty() && s != "Unknown";
    if known(make) && known(model) && known(serial) {
        format!("{} {} {}", make.trim(), model.trim(), serial.trim())
    } else {
        connector.to_owned()
    }
}

impl Default for DisplaySettings {
    fn default() -> Self {
        DisplaySettings { scale: 2.0, main: None, outputs: Default::default() }
    }
}

impl DisplaySettings {
    /// A display's saved settings, by its key or (as older versions saved
    /// them) its connector, field by field: a key entry holding only a
    /// brightness mustn't hide the position saved under the connector.
    pub fn config(&self, key: &str, connector: &str) -> Option<OutputConfig> {
        let by_key = self.outputs.get(key);
        let by_connector = (key != connector).then(|| self.outputs.get(connector)).flatten();
        match (by_key, by_connector) {
            (Some(k), Some(c)) => Some(k.clone().or(c)),
            (k, c) => k.or(c).cloned(),
        }
    }

    /// A display's settings to change, moved from its connector to its key
    /// (merged into what the key has).
    pub fn config_mut(&mut self, key: &str, connector: &str) -> &mut OutputConfig {
        if key != connector
            && let Some(old) = self.outputs.remove(connector)
        {
            let merged = match self.outputs.remove(key) {
                Some(k) => k.or(&old),
                None => old,
            };
            self.outputs.insert(key.to_owned(), merged);
        }
        self.outputs.entry(key.to_owned()).or_default()
    }

    pub fn scale_for(&self, key: &str, connector: &str) -> f64 {
        self.config(key, connector).and_then(|c| c.scale).unwrap_or(self.scale).clamp(0.5, 4.0)
    }

    /// The brightness kept for a display (full when none was).
    pub fn brightness_for(&self, key: &str, connector: &str) -> f32 {
        self.config(key, connector).and_then(|c| c.brightness).unwrap_or(1.0).clamp(0.0, 1.0)
    }

    pub fn rotation_for(&self, key: &str, connector: &str) -> u16 {
        self.config(key, connector).and_then(|c| c.rotation).filter(|r| ROTATIONS.contains(r)).unwrap_or(0)
    }

    pub fn is_main(&self, key: &str, connector: &str) -> bool {
        self.main.as_deref().is_some_and(|m| m == key || m == connector)
    }
}

/// Light/dark theme and night mode. app_server resolves `Auto` against the
/// schedule and broadcasts the effective theme in [`Settings::effective_theme`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Appearance {
    pub theme: ThemePreference,
    pub night_light: NightLight,
    /// Filled in by app_server: the theme currently in effect.
    pub effective_theme: olo_theme::ThemeMode,
    /// Filled in by app_server: whether night mode is warming the screen now.
    pub night_light_active: bool,
    /// Deskbar's menu shows each item's icon: applications, preferences,
    /// commands.
    pub menu_icons: bool,
    /// The theme every icon and the wordmark are drawn in (an id from
    /// `olo_xdg::themes::THEMES`; unknown ids draw the default).
    pub icon_theme: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    Light,
    Dark,
    /// Dark between the night-light schedule's start and end times.
    Auto,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NightLightMode {
    #[default]
    Off,
    On,
    Scheduled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NightLight {
    pub mode: NightLightMode,
    /// Local time "HH:MM".
    pub start: String,
    pub end: String,
    /// Kelvin; 6500 is neutral.
    pub temperature: u32,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            theme: ThemePreference::Light,
            night_light: NightLight::default(),
            effective_theme: olo_theme::ThemeMode::Light,
            night_light_active: false,
            menu_icons: true,
            icon_theme: "bebox".into(),
        }
    }
}

impl Default for NightLight {
    fn default() -> Self {
        NightLight { mode: NightLightMode::Off, start: "20:00".into(), end: "07:00".into(), temperature: 3600 }
    }
}

impl NightLight {
    /// Is `minutes` (since local midnight) inside the start..end window,
    /// which may wrap past midnight?
    pub fn in_window(&self, minutes: u32) -> bool {
        let (Some(start), Some(end)) = (parse_hhmm(&self.start), parse_hhmm(&self.end)) else {
            return false;
        };
        if start <= end { minutes >= start && minutes < end } else { minutes >= start || minutes < end }
    }
}

pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TouchpadSettings {
    pub tap_to_click: bool,
    pub tap_and_drag: bool,
    pub natural_scroll: bool,
    pub disable_while_typing: bool,
    pub clickfinger: bool,
    /// libinput accel speed, -1.0 ..= 1.0
    pub speed: f64,
    /// The tuning profile (`touchpad::PROFILES`); "custom" once changed by
    /// hand.
    pub profile: String,
    /// Olo's touch filter for pads that report pressure.
    pub filter: touchpad::TouchFilter,
    /// "adaptive" or "flat" (no acceleration).
    pub accel_profile: String,
    /// Touchpad scrolling distance, 1.0 as measured against Olo's apps.
    pub scroll_speed: f64,
    /// A flick keeps scrolling after the fingers lift (`momentum.rs`).
    pub momentum: bool,
    /// How far a flick glides, 1.0 as macOS's normal deceleration.
    pub glide: f64,
    /// After tap-and-drag lifts: "off", "timeout" or "sticky".
    pub drag_lock: String,
    /// Three-finger drag: 0 off, 3 or 4 fingers.
    pub three_finger_drag: u8,
    /// Tapping (and clicking with) two fingers: "lrm" (two right-click,
    /// three middle-click) or "lmr".
    pub tap_button_map: String,
    pub clickfinger_button_map: String,
    /// Pressing both buttons at once is a middle click.
    pub middle_emulation: bool,
    /// Ignore the touchpad while the pointing stick is used.
    pub dwtp: bool,
    /// How long after typing the touchpad stays ignored, 0 for libinput's
    /// own.
    pub dwt_timeout_ms: u32,
    /// "two-finger" or "edge".
    pub scroll_method: String,
    pub left_handed: bool,
    /// Turn the touchpad off while a mouse is plugged in.
    pub off_with_mouse: bool,
    /// Mice: "adaptive" or "flat".
    pub mouse_accel_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct KeyboardSettings {
    pub layout: String,
    pub variant: String,
    pub options: String,
    pub repeat_delay_ms: i32,
    pub repeat_rate_hz: i32,
    /// ⌘ (the Super/Meta key) is the command key: apps see Ctrl, so ⌘C
    /// copies, and app_server takes ⌘Tab, ⌘`, ⌘Space and ⌘ arrows.
    pub command_key: bool,
    /// Caps Lock is another Ctrl.
    pub caps_lock_control: bool,
    /// Num Lock is on when Olo starts (and is set so when this
    /// changes); the Num Lock key still toggles it.
    pub num_lock: bool,
    /// Tapping ⌘ on its own opens Find (off: only ⌘Space does). Renamed
    /// from `tap_command_find` when it stopped being on by default, so the
    /// old default saved in settings files doesn't keep it on.
    pub find_on_command_tap: bool,
    /// Apps that keep physical Ctrl for themselves (terminals): ⌘ reaches
    /// them as Ctrl+Shift, so Ctrl+C still interrupts. Matched on app_id.
    pub terminal_apps: Vec<String>,
    /// Keyboard hints: pressing Control twice quickly puts a letter on
    /// everything that can be picked (windows, controls, menu items);
    /// typing one picks it, or letters what's inside it.
    pub hints: bool,
    /// How quickly a double press (Control twice, for hints) must come,
    /// in milliseconds: the second press within this of the first.
    pub double_press_ms: u32,
}

/// One keyboard hint: the letters that pick it, where it is (global
/// logical pixels), and what it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HintInfo {
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Its name ("Save", a window's title), if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// AccessKit's role ("Button", "MenuItem", …), "Window", or "Group" for
    /// a part of a window whose own hints come next.
    pub role: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            workspace_count: 4,
            // BeOS R5's default desktop blue, then a few Haiku-ish alternates.
            workspace_colors: vec![[51, 102, 152], [0, 128, 128], [102, 51, 153], [96, 96, 96]],
            // A new install starts with the default theme's own wallpaper
            // (`olo_xdg::themes::wallpaper`), the pack's first.
            wallpaper: Some("pack:bebox".into()),
            focus_follows_mouse: false,
            touchpad: TouchpadSettings::default(),
            keyboard: KeyboardSettings::default(),
            mouse_speed: 0.0,
            tab_font_size: 12.0,
            tab_title_chars: 100,
            appearance: Appearance::default(),
            display: DisplaySettings::default(),
            power: PowerSettings::default(),
            screen_saver: ScreenSaverSettings::default(),
            clock: ClockSettings::default(),
            shortcuts: ShortcutSettings::default(),
            tiling: TilingSettings::default(),
            tracker: TrackerSettings::default(),
            security: SecuritySettings::default(),
            ai: AiSettings::default(),
        }
    }
}

impl Default for TouchpadSettings {
    fn default() -> Self {
        TouchpadSettings {
            tap_to_click: true,
            tap_and_drag: false,
            natural_scroll: false,
            disable_while_typing: true,
            clickfinger: true,
            speed: 0.0,
            profile: "macbook".into(),
            filter: touchpad::TouchFilter::default(),
            accel_profile: "adaptive".into(),
            scroll_speed: 1.0,
            momentum: true,
            glide: 1.0,
            drag_lock: "off".into(),
            three_finger_drag: 0,
            tap_button_map: "lrm".into(),
            clickfinger_button_map: "lrm".into(),
            middle_emulation: false,
            dwtp: true,
            dwt_timeout_ms: 0,
            scroll_method: "two-finger".into(),
            left_handed: false,
            off_with_mouse: false,
            mouse_accel_profile: "adaptive".into(),
        }
    }
}

impl Default for KeyboardSettings {
    fn default() -> Self {
        KeyboardSettings {
            layout: "us".into(),
            variant: String::new(),
            options: String::new(),
            repeat_delay_ms: 300,
            repeat_rate_hz: 35,
            command_key: true,
            caps_lock_control: true,
            num_lock: true,
            find_on_command_tap: false,
            hints: false,
            double_press_ms: 400,
            terminal_apps: [
                "olo.terminal",
                "org.gnome.Ptyxis",
                "org.gnome.Console",
                "org.gnome.Terminal",
                "com.mitchellh.ghostty",
                "foot",
                "kitty",
                "Alacritty",
                "org.wezfurlong.wezterm",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

impl Settings {
    pub fn workspace_color(&self, index: u32) -> [u8; 3] {
        self.workspace_colors
            .get(index as usize)
            .or_else(|| self.workspace_colors.first())
            .copied()
            .unwrap_or([51, 102, 152])
    }
}

pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os(SOCKET_ENV).map(PathBuf::from)
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("{SOCKET_ENV} is not set; not running under Olo's app_server")]
    NoSocket,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("app_server: {0}")]
    Server(String),
    #[error("app_server closed the connection")]
    Closed,
}

/// A blocking connection to app_server.
pub struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

/// An xdg-activation token from app_server ([`Request::ActivationToken`])
/// for a program about to be started, unless the environment already
/// carries one (a launch from a click); None after `timeout` or outside
/// Olo. Set it as `XDG_ACTIVATION_TOKEN`, so the window the program opens,
/// or the running instance it hands its work to, comes forward.
pub fn launch_activation_token(timeout: std::time::Duration) -> Option<String> {
    if std::env::var_os("XDG_ACTIVATION_TOKEN").is_some() {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let token = Client::connect().and_then(|mut c| c.request(&Request::ActivationToken));
        if let Ok(Reply::ActivationToken { token }) = token {
            let _ = tx.send(token);
        }
    });
    rx.recv_timeout(timeout).ok()
}

impl Client {
    pub fn connect() -> Result<Self, ClientError> {
        Self::connect_to(&socket_path().ok_or(ClientError::NoSocket)?)
    }

    /// Connect to the app_server listening at `path` (another session's).
    pub fn connect_to(path: &std::path::Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path)?;
        Ok(Client { reader: BufReader::new(stream.try_clone()?), writer: stream })
    }

    pub fn request(&mut self, req: &Request) -> Result<Reply, ClientError> {
        self.send(req)?;
        loop {
            match self.read()? {
                // Events can interleave with replies on a subscribed connection.
                Reply::Event { .. } => continue,
                Reply::Error { message } => return Err(ClientError::Server(message)),
                reply => return Ok(reply),
            }
        }
    }

    pub fn send(&mut self, req: &Request) -> Result<(), ClientError> {
        let mut line = serde_json::to_vec(req)?;
        line.push(b'\n');
        self.writer.write_all(&line)?;
        Ok(())
    }

    /// Read the next line (a reply or, when subscribed, an event).
    pub fn read(&mut self) -> Result<Reply, ClientError> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(ClientError::Closed);
        }
        Ok(serde_json::from_str(&line)?)
    }

    /// Consume this client as an event stream (after sending Subscribe).
    pub fn into_events(mut self) -> Result<impl Iterator<Item = Result<Event, ClientError>>, ClientError> {
        match self.request(&Request::Subscribe)? {
            Reply::Ok => {}
            other => return Err(ClientError::Server(format!("unexpected reply {other:?}"))),
        }
        Ok(std::iter::from_fn(move || match self.read() {
            Ok(Reply::Event { event }) => Some(Ok(event)),
            Ok(_) => None,
            Err(ClientError::Closed) => None,
            Err(e) => Some(Err(e)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_settings_merge_key_and_connector() {
        let mut d = DisplaySettings::default();
        // Position saved by connector (older versions), brightness by key.
        d.outputs.insert("DP-1".into(), OutputConfig { position: Some((1920, 1016)), ..Default::default() });
        d.outputs.insert("Apple Studio 1".into(), OutputConfig { brightness: Some(0.5), ..Default::default() });
        let c = d.config("Apple Studio 1", "DP-1").unwrap();
        assert_eq!((c.position, c.brightness), (Some((1920, 1016)), Some(0.5)));
        // Changing it moves everything to the key.
        d.config_mut("Apple Studio 1", "DP-1").scale = Some(1.5);
        assert!(!d.outputs.contains_key("DP-1"));
        let c = &d.outputs["Apple Studio 1"];
        assert_eq!((c.position, c.brightness, c.scale), (Some((1920, 1016)), Some(0.5), Some(1.5)));
    }

    #[test]
    fn request_wire_format_is_tagged() {
        let s = serde_json::to_string(&Request::Activate { id: 7 }).unwrap();
        assert_eq!(s, r#"{"type":"activate","id":7}"#);
        let back: Request = serde_json::from_str(&s).unwrap();
        assert_eq!(back, Request::Activate { id: 7 });
    }

    #[test]
    fn display_keys_follow_the_monitor() {
        assert_eq!(display_key("DP-1", "Dell", "U2720Q", "ABC123"), "Dell U2720Q ABC123");
        assert_eq!(display_key("DP-1", "Dell", "U2720Q", "Unknown"), "DP-1");
        let mut d = DisplaySettings::default();
        d.outputs.insert("DP-1".into(), OutputConfig { scale: Some(1.5), ..Default::default() });
        assert_eq!(d.scale_for("Dell U2720Q ABC123", "DP-1"), 1.5);
        d.config_mut("Dell U2720Q ABC123", "DP-1").rotation = Some(90);
        assert!(!d.outputs.contains_key("DP-1"));
        assert_eq!(d.rotation_for("Dell U2720Q ABC123", "DP-2"), 90);
        assert_eq!(d.scale_for("Dell U2720Q ABC123", "DP-2"), 1.5);
    }

    #[test]
    fn night_window_wraps_midnight() {
        let n = NightLight::default(); // 20:00 - 07:00
        assert!(n.in_window(22 * 60));
        assert!(n.in_window(3 * 60));
        assert!(!n.in_window(12 * 60));
        assert!(!n.in_window(7 * 60));
    }

    #[test]
    fn settings_tolerate_missing_fields() {
        let s: Settings = serde_json::from_str(r#"{"workspace_count": 8}"#).unwrap();
        assert_eq!(s.workspace_count, 8);
        assert!(s.touchpad.tap_to_click);
        assert_eq!(s.display.scale_for("eDP-1", "eDP-1"), 2.0);
    }
}
