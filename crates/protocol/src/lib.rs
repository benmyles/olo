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

pub mod airpods;
pub mod app_keys;
pub mod color;
pub mod doctor;
pub mod keyboards;
pub mod mice;
pub mod notifications;
pub mod panels;
pub mod performance;
pub mod recording;
pub mod rescue;
pub mod sharing;
pub mod shortcuts;
pub mod sound;
pub mod touchpad;
pub mod voice;
pub use notifications::{AppNotifications, CardSize, HistoryView, NoteApp, NoteLook, NoteStyle, NotificationSettings, NotificationState};
pub use sharing::SharingState;
pub use shortcuts::{AutoTiling, Chord, CommandKey, KeyboardProfile, ModeChoice, Platform, Shortcut, ShortcutSettings, Tile, TilingSettings, WindowingMode};

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
    /// Start receiving [`Event`]s on this connection: all of them.
    Subscribe,
    /// Start receiving only these kinds of [`Event`] on this connection
    /// (an app_server from before this answers an error: then
    /// [`Request::Subscribe`]; [`Client::into_events_of`] does both).
    SubscribeTo {
        events: Vec<EventKind>,
    },
    /// Take these keys for this connection, each `(id, keys)` with keys as
    /// a [`shortcuts::Chord`] is written ("Ctrl+Alt+K"): another program's
    /// global shortcuts (the GlobalShortcuts portal). They replace what
    /// the connection took before, come back as [`Event::KeyGrab`] on this
    /// connection alone (pressed and released; not to the focused window,
    /// nor while the screen is locked), and are let go when it closes.
    /// Keys app_server has (its own shortcuts) or another connection took
    /// are refused: [`Reply::KeysGrabbed`] says which.
    GrabKeys {
        keys: Vec<(String, String)>,
    },
    ListWindows,
    Activate {
        id: WindowId,
    },
    /// Bring these windows forward together, as clicking an application
    /// in Deskbar does: minimized ones come back, and if they are on
    /// another workspace it goes there. Among themselves they keep the
    /// order they were used in, so the one used last ends in front, with
    /// the focus.
    ActivateAll {
        ids: Vec<WindowId>,
    },
    /// An xdg-activation token for a program about to be started (as
    /// `XDG_ACTIVATION_TOKEN`), so the window it opens, or the running
    /// instance it hands its work to (a link opening as a new tab in
    /// Chrome), comes forward, from whatever workspace. For launchers run
    /// without a click to take a token from (a link opened from a shell).
    ActivationToken,
    /// BeOS "minimize": hide the window; Deskbar shows it greyed out.
    Minimize {
        id: WindowId,
    },
    Close {
        id: WindowId,
    },
    ToggleZoom {
        id: WindowId,
    },
    /// Push a window behind all the others (BeOS's right-click on the tab).
    SendToBack {
        id: WindowId,
    },
    /// Put a window in the group of window `with` (or a new group with
    /// it, if it is in none), whatever their apps, its tab at `index`
    /// among the group's (else last), as ⌘-dragging its tab onto that
    /// window's does: it takes the group's frame and comes to the front.
    /// Both must be showing.
    GroupWindow {
        id: WindowId,
        with: WindowId,
        #[serde(default)]
        index: Option<u32>,
    },
    /// Take a window out of its group, as ⌘-dragging its tab away does: on
    /// its own from then on (it no longer joins its app's group), stepped
    /// out so the group shows, in front.
    UngroupWindow {
        id: WindowId,
    },
    /// Move a window's tab to `index` among its group's tabs.
    MoveTab {
        id: WindowId,
        index: u32,
    },
    /// Put a window's frame (tab and border included, as
    /// [`WindowInfo::frame`]) here, at this size; no longer zoomed. It
    /// may hang off a display's edge, but no further than a person could
    /// leave it: its top edge on the display, enough of it to take hold of.
    Place {
        id: WindowId,
        frame: Frame,
    },
    /// The workspaces: how many each display has, and which each shows.
    Workspaces,
    /// Show workspace `index` on a display (`output`, a connector; else
    /// the active one). Every display has its own workspaces, and
    /// switching one leaves the others as they are.
    SwitchWorkspace {
        index: u32,
        #[serde(default)]
        output: Option<String>,
    },
    /// Move a window to workspace `index` of the display it is on, or of
    /// display `output` (a connector): there, to the same place on it
    /// relatively, tiled or zoomed as it was.
    MoveToWorkspace {
        id: WindowId,
        index: u32,
        #[serde(default)]
        output: Option<String>,
    },
    /// Move every window on workspace `index` of display `output` to
    /// workspace `to_index` (else the same number) of display `to` (both
    /// connectors), each to the same place on it relatively, tiled or
    /// zoomed as it was; hidden ones stay hidden. As Deskbar's workspace
    /// headings offer.
    MoveWorkspace {
        output: String,
        index: u32,
        to: String,
        #[serde(default)]
        to_index: Option<u32>,
    },
    Outputs,
    /// Go to display `output` (a connector), as Deskbar's display menu
    /// does: the pointer to the middle of it, the keyboard to its front
    /// window (none: its desktop).
    FocusDisplay {
        output: String,
    },
    ConfigureOutput {
        name: String,
        config: OutputConfig,
    },
    /// The pointing devices plugged in: touchpads (with the kind of pad and
    /// the profile that suits it) and mice.
    InputDevices,
    /// The keyboards plugged in, by name (real ones: with letter keys).
    Keyboards,
    /// Every GPU and the displays wired to it ([`Reply::Gpus`]): each
    /// display is composed by its own GPU.
    Gpus,
    GetSettings,
    /// Every theme and its colour sets, and the palette in use: what
    /// Preferences ▸ Themes offers. Choose with `SetSettings`
    /// (`appearance.icon_theme`, `appearance.colors`).
    Looks,
    /// One look's palette: every colour, `#rrggbb`. Each part left out is
    /// the one in use.
    Palette {
        #[serde(default)]
        theme: Option<String>,
        #[serde(default)]
        colors: Option<String>,
        #[serde(default)]
        mode: Option<olo_theme::ThemeMode>,
    },
    /// The keyboard hints showing now (Preferences ▸ Keyboard): each
    /// letter, where it is and what it picks.
    Hints,
    /// The overview (⌃Space): with `show`, open it (true) or leave it
    /// (false, everything as it was); answers what it shows.
    Overview {
        #[serde(default)]
        show: Option<bool>,
    },
    SetSettings {
        settings: Settings,
    },
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
        /// Headless only: what the screen itself holds, as the display
        /// would be sent it (in its colours: P3's numbers, PQ in HDR),
        /// not the sRGB picture a screenshot is.
        #[serde(default)]
        as_shown: bool,
    },
    /// Headless only, for finding flicker: save what `output` (or the
    /// first screen) shows after each of app_server's next `frames` turns
    /// as PNGs in `path` (`NNNN-MS.png`, milliseconds from the start;
    /// one the same as the one before is skipped), on a thread.
    Record {
        path: String,
        frames: u32,
        #[serde(default)]
        output: Option<String>,
    },
    /// Lock the screen now.
    Lock,
    /// Olo Rescue (⌃⇧⌘Delete, or Super for ⌘) on or off: on puts every
    /// window away, runs the displays, keyboard and pointer on safe
    /// settings and starts olo-rescue; off brings back what was, with
    /// the person's own settings. Meanwhile `GetSettings`, `SetSettings`
    /// and `ConfigureOutput` are the person's own settings, saved, and
    /// applied as Rescue ends.
    Rescue {
        on: bool,
    },
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
    /// Keep the screens awake (no dimming, display or system sleep, screen
    /// saver or lock) for as long as this connection stays open, for
    /// `why` (an app's call or video: the Inhibit portal, the
    /// ScreenSaver service). Counted among the app inhibitors.
    KeepAwake {
        why: String,
    },
    /// Approve requests without asking (sudo in Terminal, unlocking
    /// Passwords) for `minutes` (5 to a day), or stop (None). Turning it
    /// on takes the account password, checked through PAM before the
    /// answer (`Reply::Ok`, or an error saying why); while it is on,
    /// another length needs none. It ends at the time, when the screen
    /// locks (sleeping locks first) and when app_server restarts. Not
    /// persisted.
    SetAutoApprove {
        minutes: Option<u32>,
        #[serde(default)]
        password: Option<String>,
    },
    /// The password auto-approval was turned on with
    /// ([`Reply::Secret`]), for Terminal to answer sudo with; only
    /// Olo's Terminal (by its executable, beside app_server's) is
    /// answered, and only while it is on.
    AutoApproveSecret,
    /// Built-in display brightness, 0.0–1.0 (perceptual).
    SetBrightness {
        level: f32,
    },
    /// A display's brightness (`name` its connector), kept for it.
    SetOutputBrightness {
        name: String,
        level: f32,
    },
    /// Keyboard backlight, 0.0–1.0 (perceptual).
    SetKeyboardBacklight {
        level: f32,
    },
    /// Show each display's name on it for a few seconds.
    IdentifyOutputs,
    /// Test sessions (headless) only: the screen on connector `name`
    /// unplugged (`plugged: false`) or plugged in again, as a monitor's
    /// cable would be (`tools/dev/layout-check --unplug`).
    PlugDisplay {
        name: String,
        plugged: bool,
    },
    /// Take the next key pressed (with its modifiers) as a shortcut rather
    /// than acting on it; it comes back as [`Event::ShortcutRecorded`].
    /// `on: false` stops waiting.
    RecordShortcut {
        on: bool,
    },
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
    TileApp {
        app_id: String,
    },
    /// The active display's tiling area and the windows on it (front
    /// first, with frames), for an arranger such as Smart Tiling.
    Arrangement,
    /// Put windows of the active display in parts of its tiling area,
    /// tiled there (dragging an edge resizes the neighbours) and undoable
    /// with Undo Window Arrangement. Windows not listed stay as they are.
    Arrange {
        windows: Vec<Placement>,
    },
    /// Each display's windowing mode on a workspace (each display's
    /// showing unless given).
    WindowingMode {
        #[serde(default)]
        workspace: Option<u32>,
    },
    /// Choose a windowing mode (Deskbar's tray) for one display
    /// (`output`, a connector; else the active display), on one of its
    /// workspaces (the one showing unless given). Standard leaves windows where they
    /// are put. Auto-Tiling tiles every window (see `SetAutoTiling`).
    /// Focus shows only the active window, zoomed, with its app's other
    /// windows grouped behind it and dialogs and floating windows over
    /// it; activating another window (Deskbar, ⌘Tab, a click on a tab)
    /// shows that one instead. Remembered (`TilingSettings::modes`); where
    /// none is chosen, `TilingSettings::default_mode`.
    SetWindowingMode {
        mode: WindowingMode,
        #[serde(default)]
        workspace: Option<u32>,
        #[serde(default)]
        output: Option<String>,
    },
    /// Whether auto-tiling is on for each display on a workspace (each
    /// display's showing unless given).
    AutoTiling {
        #[serde(default)]
        workspace: Option<u32>,
    },
    /// Auto-tiling on (`on`), off (the Standard windowing mode), or
    /// (left out) the other way, as ⌃⌥⌘T does, for one display (`output`,
    /// a connector; else the active display) on one of its workspaces
    /// (the one showing unless given). While it is on, every window there is
    /// tiled: a new one takes its share, a closed one's room goes to its
    /// neighbours, and dialogs and fixed-size windows float over the
    /// tiles. Remembered (`TilingSettings::modes`); `SetWindowingMode`
    /// says the rest.
    SetAutoTiling {
        #[serde(default)]
        on: Option<bool>,
        #[serde(default)]
        workspace: Option<u32>,
        #[serde(default)]
        output: Option<String>,
    },
    /// Float a window over auto-tiled ones (`floating: true`), tile it
    /// (`false`), or (left out) leave it to app_server, which floats
    /// dialogs, alerts, fixed-size windows and windows tagged `floating`
    /// ([`WindowInfo::floating`]).
    SetFloating {
        id: WindowId,
        #[serde(default)]
        floating: Option<bool>,
    },
    /// Inject input as if from a device (for automated UI tests).
    Synthetic {
        events: Vec<SyntheticEvent>,
    },
    /// Put `text` into the focused field as an input method would (the
    /// emoji picker; any text, not only what a keyboard types). It waits,
    /// a second at most, while the asking program has the keyboard; a
    /// window with no text-input-v3 field gets it as keys.
    InsertText {
        text: String,
    },
    /// Press `keys` together and let them go (evdev codes, as a keyboard
    /// sends them: through the keyboard profile, so ⌘ is the person's
    /// command key), into whatever has the keyboard once the asking
    /// program hasn't (the command palette, on its way out, doing a text
    /// shortcut in the window it was opened over). It waits a second at
    /// most; the reply comes once they're pressed.
    PressKeys {
        keys: Vec<u32>,
    },
    /// Displays with nothing new rest (look again only when woken, or now
    /// and then) or look every frame: `on`, `off`, or (None) as app_server
    /// has it for its kind of display. Until app_server restarts.
    SetResting {
        #[serde(default)]
        on: Option<bool>,
    },
    /// How app_server has kept up since the last ask ([`Timings`]), and
    /// keep watching for another minute (for tools/dev/perf-fuzz). `quiet`:
    /// without the heartbeat (which wakes app_server every 4 ms), to
    /// measure what it costs.
    Timings {
        #[serde(default)]
        quiet: bool,
    },
    /// How each display keeps up ([`DisplayFrames`]: its frames a second,
    /// the ones presented late, what its frames cost), for Activity
    /// Monitor's Displays. Counted always, at no cost worth measuring.
    Frames,
    /// Log what went into every frame that misses its vblank (`on`), as
    /// a `frames` record in the log: Activity Monitor's "Capture debug
    /// information". Until app_server restarts.
    CaptureDrops {
        on: bool,
    },
    /// The last 120 frames of display `output` that drew the pointer
    /// ([`PointerFrame`]): to measure how evenly it moves.
    PointerTrace {
        output: String,
    },
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
    /// `at_us`: when the fingers were there, on the monotonic clock (as a
    /// pad's kernel time); without it, when app_server reads it.
    Scroll {
        dx: f64,
        dy: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at_us: Option<u64>,
    },
    /// Turn a mouse wheel by whole notches (positive: down / right).
    Wheel {
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    /// What's on a touchpad, as a pad's report would say (a MacBook pad's
    /// axes): sent on as [`Event::TouchContacts`] (headless sessions,
    /// testing the palm trainer).
    Contacts { contacts: Vec<touchpad::TouchContact> },
    /// A finger on a touch screen, at global logical coordinates: down,
    /// moved or lifted (`slot` tells fingers apart). As a real touch is:
    /// touch for an app that takes it, else the pointer with the left
    /// button held.
    Touch {
        #[serde(default)]
        slot: u32,
        phase: TouchPhase,
        #[serde(default)]
        x: f64,
        #[serde(default)]
        y: f64,
    },
}

/// What a finger on a touch screen did ([`SyntheticEvent::Touch`]).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TouchPhase {
    Down,
    Motion,
    Up,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Ok,
    Pong {
        version: String,
    },
    Error {
        message: String,
    },
    ActivationToken {
        token: String,
    },
    Windows {
        windows: Vec<WindowInfo>,
    },
    /// How many workspaces each display has, the one showing on the
    /// active display (`current`), and each display's (`displays`).
    Workspaces {
        current: u32,
        count: u32,
        #[serde(default)]
        displays: Vec<DisplayWorkspace>,
    },
    Outputs {
        outputs: Vec<OutputInfo>,
    },
    InputDevices {
        devices: Vec<touchpad::InputDeviceInfo>,
    },
    Keyboards {
        names: Vec<String>,
    },
    Gpus {
        gpus: Vec<GpuInfo>,
    },
    Settings {
        settings: Settings,
    },
    /// [`Request::Looks`]' answer: the themes in Preferences' order, and
    /// the palette in use.
    Looks {
        themes: Vec<LookInfo>,
        palette: olo_theme::Palette,
    },
    Palette {
        palette: olo_theme::Palette,
    },
    /// The keyboard hints showing now (none when they're off).
    Hints {
        hints: Vec<HintInfo>,
    },
    /// [`Request::Overview`]'s answer.
    Overview {
        overview: OverviewInfo,
    },
    /// [`Request::Arrangement`]'s answer: the display's connector, the
    /// area tiling uses (global logical pixels: the display less Deskbar
    /// and the margins), and its windows on this workspace, front first.
    Arrangement {
        output: String,
        area: Frame,
        windows: Vec<WindowInfo>,
    },
    /// [`Request::AutoTiling`]'s answer: each display, on the workspace
    /// asked about (`workspace`: the active display's showing if none was).
    AutoTiling {
        workspace: u32,
        displays: Vec<AutoTilingInfo>,
    },
    /// [`Request::WindowingMode`]'s answer: each display, as AutoTiling's.
    WindowingMode {
        workspace: u32,
        displays: Vec<WindowingModeInfo>,
    },
    Power {
        status: PowerStatus,
    },
    /// After [`Request::GrabKeys`]: the ids whose keys weren't taken.
    KeysGrabbed {
        refused: Vec<String>,
    },
    /// [`Request::AutoApproveSecret`]'s answer.
    Secret {
        password: String,
    },
    Timings {
        timings: Timings,
    },
    /// [`Request::Frames`]: each display, the pointer's reports, and
    /// whether drops are captured.
    Frames {
        displays: Vec<DisplayFrames>,
        capture: bool,
        #[serde(default)]
        pointer: PointerHealth,
        #[serde(default)]
        touch: TouchHealth,
        #[serde(default)]
        inputs: InputActivity,
    },
    PointerTrace {
        frames: Vec<PointerFrame>,
    },
    Event {
        event: Event,
    },
}

/// [`Request::Timings`]'s answer: how app_server kept up since the last
/// ask. Durations in microseconds.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Timings {
    /// Each display's frames.
    pub outputs: Vec<FrameTimings>,
    /// Beats of a heartbeat that runs while anyone is watching, and the
    /// latest one came past when it was due: whatever blocks app_server's
    /// event loop (a slow request, input handler or commit) shows here.
    pub beats: u32,
    pub latest_beat_us: u64,
    /// The slowest thing app_server did between beats (what it was, how
    /// long it took): a request ("request run_shortcut"), an input event,
    /// clients' requests (their commits among them), or the "tick" after
    /// each turn of the loop; to tell what made a beat late.
    pub slowest: Option<(String, u64)>,
    /// What the loop spent its time on, by part ("pre_repaint", "tick",
    /// "commit", …): how often, in all and the slowest, to find where the
    /// CPU goes.
    #[serde(default)]
    pub phases: Vec<PhaseTimings>,
}

/// One part of app_server's loop ([`Timings::phases`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PhaseTimings {
    pub name: String,
    pub count: u32,
    pub total_us: u64,
    pub slowest_us: u64,
}

/// How one display keeps up ([`Request::Frames`]): what app_server
/// counts at every vblank and frame, always. Times in microseconds.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DisplayFrames {
    pub output: String,
    /// The mode's refresh rate, in millihertz (59 940 for 59.94 Hz).
    pub refresh_mhz: u32,
    /// Frames presented in the last second. A display with nothing new
    /// presents none (it rests), so 0 on an idle display is right.
    pub fps: f64,
    /// Frames presented since app_server started, and of them the ones
    /// presented a vblank or more after the one they were drawn for
    /// (dropped: whoever watched saw the one before held a frame longer).
    pub frames: u64,
    pub late: u64,
    /// Late ones in the last minute.
    pub late_minute: u32,
    /// What its last frames cost app_server to draw (CPU, from gathering
    /// what shows to handing the frame to the GPU), over the last 120.
    pub render_min_us: u64,
    pub render_p50_us: u64,
    pub render_p95_us: u64,
    pub render_max_us: u64,
    /// How much of a frame's time is kept for drawing (`budget`), and so
    /// how long after a vblank the next frame starts (`delay`): the
    /// pacing's current choice (app_server frames.rs).
    pub budget_us: u64,
    pub delay_us: u64,
    /// The pointer is on a plane of the display's own, so moving it
    /// draws nothing.
    pub cursor_plane: bool,
    /// What the last frame's elements came to: how many went straight
    /// to a plane of their own (nothing drawn for them) and how many had
    /// to be drawn, with why ("1 on a plane; 6 drawn (1 overlaps, …)").
    #[serde(default)]
    pub scanout: String,
    /// Resting: nothing new, looking only now and then.
    pub resting: bool,
}

/// What the person did with each kind of input in the last minute
/// ([`Reply::Frames`]): clicks, scrolls, gestures begun and keys pressed
/// (how many, never which).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct InputActivity {
    pub clicks_minute: u32,
    pub scrolls_minute: u32,
    pub gestures_minute: u32,
    pub keys_minute: u32,
}

/// The pointer's reports as app_server gets them ([`Reply::Frames`]):
/// how many a second, gaps between reports within a run of motion (over
/// 5 ms; a receiver asleep or dropping packets), the longest in the
/// last minute, and the device's usual interval (1 ms for a 1 kHz mouse).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PointerHealth {
    pub reports_second: u32,
    pub gaps_minute: u32,
    pub longest_gap_us: u64,
    pub interval_us: u64,
}

/// The touchpad's reports as app_server gets them ([`Reply::Frames`]),
/// through libinput and Olo's touch filter with its palm model: how many
/// a second, and how long one took from the pad to app_server (the
/// kernel's time to its handling: libinput's and the filter's work, and
/// any wait for the main thread, which draws frames too), over the last
/// minute: typically, at the 99th percentile and the longest. And what
/// judges palms, and the main thread's scheduling: it reads the pad.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TouchHealth {
    pub reports_second: u32,
    /// The reports the latencies are of (the last minute's).
    pub reports_minute: u32,
    pub latency_p50_us: u64,
    pub latency_p99_us: u64,
    pub latency_max_us: u64,
    /// Olo's touch filter is in libinput, and with it the person's
    /// trained palm model.
    pub filter: bool,
    pub model: bool,
    /// The model's training (`touchpad::PalmModel`): when (seconds since
    /// the epoch), from how many contacts, and on those kept back the
    /// share of palms it caught and of fingers it took for palms.
    pub trained: u64,
    pub contacts: u32,
    pub palms_caught: f32,
    pub fingers_lost: f32,
    /// app_server's main thread: its nice, whether it's realtime, and its
    /// minimum utilisation (of 1024: a fast core at a high clock).
    pub nice: i32,
    pub realtime: bool,
    pub util_min: u32,
}

/// One frame that drew the pointer ([`Request::PointerTrace`]): the
/// vblank it was for, where the pointer really was and where it was
/// drawn (`cursor::Track`), and the last report's age at that vblank and
/// interval from the one before. Times in microseconds of the input
/// clock, positions logical.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PointerFrame {
    pub vblank_us: u64,
    pub real: (f64, f64),
    pub drawn: (f64, f64),
    pub report_age_us: i64,
    pub report_interval_us: u64,
}

/// How long one display's frames took to draw: from gathering what is on
/// it to handing the frame over.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FrameTimings {
    pub output: String,
    pub frames: u32,
    pub slowest_us: u64,
    pub total_us: u64,
}

/// The kinds of [`Event`] a connection can ask for
/// ([`Request::SubscribeTo`]), so a program wakes only for what it uses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// [`Event::WindowsChanged`]: every window, whenever anything about
    /// any of them changes (a title too).
    Windows,
    /// [`Event::WindowsChanged`] with only the connection's own process's
    /// windows, and only when where they are changes (workspace, display,
    /// hidden, behind another in its group): not their titles or sizes.
    OwnWindows,
    /// [`Event::WindowsChanged`] with only the focused window (none when
    /// nothing is), when another window gets the focus.
    Focus,
    Workspace,
    Settings,
    Outputs,
    Power,
    ShortcutRecorded,
    /// [`Event::InputDevicesChanged`].
    InputDevices,
    /// [`Event::Painted`]: only when asked for by name (not with
    /// [`Request::Subscribe`]), as it comes as often as frames do.
    Painted,
    /// [`Event::TouchContacts`]: only by name, as it comes with each of
    /// the pad's reports (Preferences' palm trainer).
    TouchContacts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    WindowsChanged {
        windows: Vec<WindowInfo>,
    },
    /// A display switched workspace, another display became the active
    /// one, or the count changed: as [`Reply::Workspaces`].
    WorkspaceChanged {
        current: u32,
        count: u32,
        #[serde(default)]
        displays: Vec<DisplayWorkspace>,
    },
    SettingsChanged {
        settings: Settings,
    },
    OutputsChanged {
        outputs: Vec<OutputInfo>,
    },
    /// Caffeinated, display asleep, brightness or app inhibitors changed.
    PowerChanged {
        status: PowerStatus,
    },
    /// The key pressed after [`Request::RecordShortcut`], as a [`Chord`]
    /// ("Shift+Cmd+Left"); "" for Backspace or Delete alone (no keys);
    /// None if it was cancelled (Escape alone, or nothing for a while).
    ShortcutRecorded {
        keys: Option<String>,
    },
    /// A pointing device came or went: every one now, as
    /// [`Reply::InputDevices`].
    InputDevicesChanged {
        devices: Vec<touchpad::InputDeviceInfo>,
    },
    /// A key taken with [`Request::GrabKeys`] went down or up (sent only
    /// to the connection that took it).
    KeyGrab {
        id: String,
        pressed: bool,
    },
    /// A display drew a frame with something new on it: what a
    /// screenshot shows has changed (headless screens: Agent Screens'
    /// watchers take a picture then, not on a timer).
    Painted {
        output: String,
    },
    /// What's on a touchpad now, as the pad reports it (each contact's
    /// place, size and pressure), after each of its reports while
    /// anyone asks: the palm trainer's live view and its samples.
    TouchContacts {
        pad: touchpad::PadAxes,
        time_us: u64,
        contacts: Vec<touchpad::TouchContact>,
        /// Microseconds since a key was last pressed (palms rest while
        /// typing); u64::MAX if none was.
        #[serde(default = "no_key")]
        since_key_us: u64,
    },
}

fn no_key() -> u64 {
    u64::MAX
}

impl Event {
    /// Its kind, for subscriptions (a key grab goes only to the connection
    /// that took the keys).
    pub fn kind(&self) -> Option<EventKind> {
        Some(match self {
            Event::WindowsChanged { .. } => EventKind::Windows,
            Event::WorkspaceChanged { .. } => EventKind::Workspace,
            Event::SettingsChanged { .. } => EventKind::Settings,
            Event::OutputsChanged { .. } => EventKind::Outputs,
            Event::PowerChanged { .. } => EventKind::Power,
            Event::ShortcutRecorded { .. } => EventKind::ShortcutRecorded,
            Event::Painted { .. } => EventKind::Painted,
            Event::TouchContacts { .. } => EventKind::TouchContacts,
            Event::InputDevicesChanged { .. } => EventKind::InputDevices,
            Event::KeyGrab { .. } => return None,
        })
    }
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
    /// Olo Rescue is on ([`Request::Rescue`]).
    #[serde(default)]
    pub rescue: bool,
    /// Requests are approved without asking until then (seconds since
    /// the epoch; [`Request::SetAutoApprove`]).
    #[serde(default)]
    pub auto_approve_until: Option<u64>,
}

/// The way a window is zoomed alone (`WindowInfo::zoomed_along`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ZoomAxis {
    Vertical,
    Horizontal,
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
    /// Twitcher (⌘Tab, ⌥Tab, ⌘\`) is on it, while its keys are held:
    /// Deskbar highlights its application.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chosen: bool,
    /// Rolled up to just its tab.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rolled: bool,
    /// Behind the window Focus mode shows on its display
    /// ([`WindowingMode::Focus`]): not drawn until it is activated.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub covered: bool,
    /// Zoomed (its zoom box's state): filling the tiling area until it is
    /// zoomed again or restored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub zoomed: bool,
    /// Zoomed one way only (Zoom Vertically, Zoom Horizontally): its
    /// height, or width, held to the tiling area's, the other its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoomed_along: Option<ZoomAxis>,
    /// Its program keeps it through a window server restart (an upgrade):
    /// Olo's own apps, and Wayland programs that came through
    /// olo-wlproxy.
    #[serde(default)]
    pub kept: bool,
    /// The group it is in on screen, sharing a frame with the others with
    /// this number, each its own tab along the top. An app's windows are
    /// grouped by themselves; any window can be put in a group or taken
    /// out by hand (⌘-dragging its tab, [`Request::GroupWindow`],
    /// [`Request::UngroupWindow`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u64>,
    /// It can be put in a group: a document window, not a dialog, a
    /// fixed-size panel or fullscreen.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub groupable: bool,
    /// In a tile: auto-tiled, or tiled by a command or Tile All (dragging
    /// its edges drags its neighbours').
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tiled: bool,
    /// Its tile's part of its display's tiling area, as fractions (x, y,
    /// width, height) of it, while tiled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<[f32; 4]>,
    /// It floats over auto-tiled windows rather than being tiled: a
    /// dialog, an alert, a fixed-size window, one tagged `floating`
    /// (xdg-toplevel-tag), or one floated by hand.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub floating: bool,
    /// Floated (true) or tiled (false) by hand
    /// ([`Request::SetFloating`]), over what app_server would judge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub float: Option<bool>,
    /// The display it is on (its connector), or was on while hidden or
    /// on another workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Its tab's place in its group, from the left ([`Request::MoveTab`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u32>,
    /// A tab behind another in its group: not on screen (it draws
    /// nothing) till it comes to the front.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub behind: bool,
    /// Where it is on screen, tab and border included (global logical
    /// pixels), while it is on screen. Only in [`Reply::Windows`] (the
    /// list [`Event::WindowsChanged`] carries leaves it out, or a window
    /// being dragged would change it every frame).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Frame>,
    /// Where what the app draws is (inside the tab and border), likewise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Frame>,
    /// The smallest its frame can be (its app's least size, with the tab
    /// and border): a group is no smaller than its biggest member's.
    /// Likewise only while it is on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub least: Option<[i32; 2]>,
    /// Its tab (to click, or ⌘-drag along its group or to another),
    /// likewise; none for a window without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_frame: Option<Frame>,
    /// Its open popups (menus, dropdowns, tooltips), submenus after the
    /// menus they open from, likewise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub popups: Vec<Frame>,
    /// Its program hasn't yet drawn what it was last asked (a size, a
    /// state): it is on its way to another frame. Likewise only while it
    /// is on screen; a window is settled once no window is settling.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub settling: bool,
    /// Its place in the stack of the windows on screen, every display's,
    /// 0 the front one (so a window is in front on its display when none
    /// there has a smaller one). Likewise only while it is on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<u32>,
    /// Glowing as a new window (Preferences ▸ Appearance).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub glowing: bool,
    /// Its surface as its program knows it (the `wl_surface`'s object id
    /// on the program's own connection), for a program to tell which of
    /// its windows this is: Olo's kit knows so which of an app's windows
    /// are on the workspace showing. (A program behind olo-wlproxy knows
    /// its surface by another id.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<u32>,
}

/// A rectangle in global logical pixels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Whether auto-tiling is on for a display (its connector).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoTilingInfo {
    pub output: String,
    pub on: bool,
    /// The workspace it is about (the display's showing, unless asked).
    #[serde(default)]
    pub workspace: u32,
}

/// A display's (its connector) windowing mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowingModeInfo {
    pub output: String,
    pub mode: WindowingMode,
    /// The workspace it is about (the display's showing, unless asked).
    #[serde(default)]
    pub workspace: u32,
}

/// The workspace a display (its connector) shows. Each display has its
/// own workspaces, as many as `Settings::workspace_count`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisplayWorkspace {
    pub output: String,
    pub current: u32,
    /// The active display (the focused window's, else the one last
    /// used): its workspace showing is "this workspace", where a window
    /// opened or launched goes. `WorkspaceChanged` is sent when it
    /// changes too.
    #[serde(default)]
    pub active: bool,
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
    /// The main display: the desktop lives here.
    #[serde(default)]
    pub main: bool,
    /// The display this one mirrors (its connector), if it does.
    #[serde(default)]
    pub mirror_of: Option<String>,
    /// Its brightness, 0–1, when it can be set from here.
    #[serde(default)]
    pub brightness: Option<f32>,
    /// The number it goes by, from the main display: 0 the main one, −1
    /// the one left of it, 1 the one right of it ([`display_number`]).
    /// None with one display, or for a mirror.
    #[serde(default)]
    pub number: Option<i32>,
    /// What kind of panel it is and how Olo draws for it
    /// ([`panels`]).
    #[serde(default)]
    pub panel: Option<panels::Panel>,
    /// Its variable refresh rate: None if it has none, else whether it's
    /// on (`OutputConfig::vrr`).
    #[serde(default)]
    pub vrr: Option<bool>,
    /// What it can show (its EDID). None: it doesn't say.
    #[serde(default)]
    pub color: Option<color::DisplayColor>,
    /// How sRGB's colours are shown on it; None when it's no wider than
    /// sRGB (nothing to choose).
    #[serde(default)]
    pub color_mode: Option<color::ColorMode>,
    /// HDR: None when it can't, else whether it's on.
    #[serde(default)]
    pub hdr: Option<bool>,
    /// SDR's white in HDR, cd/m².
    #[serde(default)]
    pub sdr_nits: f32,
    /// Its HDR waits on a setting at the computer's start (a built-in
    /// panel whose HDR Intel's driver doesn't drive: Olo Doctor offers it).
    #[serde(default)]
    pub hdr_needs_boot: bool,
}

/// The number display `output` (a connector) goes by, from the main
/// display: the main one is D0, the displays left of it (then above)
/// D−1, D−2, …, those right of it D1, D2, …, in their order left to
/// right, then top to bottom. So a number says where a display is, and
/// changes only when the displays are moved about or another is made
/// the main one. None with only one display, which needs no number, and
/// for a mirror (it shows another's picture).
pub fn display_number(outputs: &[OutputInfo], output: &str) -> Option<i32> {
    let mut shown: Vec<&OutputInfo> = outputs.iter().filter(|o| o.mirror_of.is_none()).collect();
    if shown.len() < 2 {
        return None;
    }
    shown.sort_by_key(|o| o.position);
    let this = shown.iter().position(|o| o.name == output)?;
    let main = shown.iter().position(|o| o.main).unwrap_or(0);
    Some(this as i32 - main as i32)
}

/// A display's number as shown, "0", "−1" (a minus sign, not a hyphen).
pub fn display_number_text(n: i32) -> String {
    if n < 0 { format!("−{}", -n) } else { n.to_string() }
}

/// A display's label from its number: "D0", "D−1".
pub fn display_label(n: i32) -> String {
    format!("D{}", display_number_text(n))
}

/// Workspace `index`'s name (from 0): "Workspace 2", or on display
/// `display` of several (`display_number`), "D1 Workspace 2" (or "D−1
/// Workspace 2"), as Deskbar, Twitcher and the overview call it.
pub fn workspace_name(display: Option<i32>, index: u32) -> String {
    match display {
        Some(d) => format!("{} Workspace {}", display_label(d), index + 1),
        None => format!("Workspace {}", index + 1),
    }
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
    /// Corrections to what Olo works out about the panel ([`panels`]):
    /// its colour stripes' order, its technology, how its text is drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpixel: Option<panels::Subpixel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub technology: Option<panels::Technology>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<panels::TextMode>,
    /// Variable refresh rate (VRR, Adaptive-Sync), for a display that has
    /// it: frames shown as they come within its range (a ThinkPad P1
    /// Gen 9's panel, 40–120 Hz). None: off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vrr: Option<bool>,
    /// How a wide-gamut display shows sRGB's colours. None: accurately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_mode: Option<color::ColorMode>,
    /// High dynamic range, for a display that has it. None: off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr: Option<bool>,
    /// SDR's white in HDR, cd/m² (`color::SDR_NITS` unless set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdr_nits: Option<f32>,
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
            subpixel: self.subpixel.or(other.subpixel),
            technology: self.technology.or(other.technology),
            text: self.text.or(other.text),
            vrr: self.vrr.or(other.vrr),
            color_mode: self.color_mode.or(other.color_mode),
            hdr: self.hdr.or(other.hdr),
            sdr_nits: self.sdr_nits.or(other.sdr_nits),
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
    /// Mice with settings of their own, or following their model's
    /// profile, by `mice::key` ("046d:40b1"); the rest follow
    /// `mouse_speed` and `touchpad.mouse_accel_profile`.
    pub mice: std::collections::BTreeMap<String, mice::MouseTuning>,
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
    /// Before keyboard profiles: app_server's shortcuts changed from
    /// their defaults (read once, into `keyboard.profile`: [`Settings::migrate`]).
    #[serde(skip_serializing)]
    pub shortcuts: Option<ShortcutSettings>,
    pub tiling: TilingSettings,
    pub tracker: TrackerSettings,
    /// Screen recordings (Preferences ▸ GPUs ▸ Screen Recordings).
    pub screen_recording: ScreenRecordingSettings,
    pub terminal: TerminalSettings,
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
    /// The emoji picker's ⌘↩: five emoji for a description.
    pub emoji: AiFeature,
    /// Agent Screens: how much of each screen's history (its actions and
    /// the frames it drew) is kept, in megabytes, and for how many days
    /// (0: for ever).
    pub screen_history_mb: u32,
    pub screen_history_days: u32,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            use_pool: true,
            smart_tiling: AiFeature { enabled: true, model: AI_MODELS[0].0.into(), effort: "medium".into() },
            // Asked while the person waits: quick.
            emoji: AiFeature { enabled: true, model: AI_MODELS[0].0.into(), effort: "low".into() },
            screen_history_mb: 512,
            screen_history_days: 30,
        }
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
pub const AI_MODELS: &[(&str, &str)] = &[("gpt-6-astra", "Astra 6"), ("gpt-6.1-sol", "Sol 6.1"), ("gpt-6.1-terra", "Terra 6.1"), ("gpt-6-luna", "Luna 6")];

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

/// What screen recordings are encoded on (olo-screenshot's recorder).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ScreenRecordingSettings {
    /// "auto" (the most powerful there is: an NVIDIA GPU's encoder
    /// where it opens, else the processor), "nvidia" or "cpu".
    pub encoder: String,
}

impl Default for ScreenRecordingSettings {
    fn default() -> Self {
        ScreenRecordingSettings { encoder: "auto".into() }
    }
}

/// Terminal's settings (Preferences ▸ Terminal).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TerminalSettings {
    /// Where New Worktree… makes a repository's worktrees:
    /// `<this>/<repository>/<worktree>` (`~` for the home folder).
    pub worktrees: String,
    /// Claude Code and Codex in a tab of a group are told the group's
    /// name and the repositories its tabs are in (Terminal's
    /// agent_context.rs; off by default).
    pub agent_context: bool,
    /// Claude Code and Codex in a tab get a tool to set its title and
    /// are told to keep it saying what they're working on (Terminal's
    /// agent_title.rs; off by default).
    pub agent_titles: bool,
    /// Claude Code and Codex are told to use Olo's Agent Screens for
    /// anything graphical, and how, Olo's Chrome included (Terminal's
    /// agent_context.rs, apps/screen/agent-context.md; off by default).
    pub agent_screens: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        TerminalSettings { worktrees: "~/worktrees".into(), agent_context: false, agent_titles: false, agent_screens: false }
    }
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
        ScreenSaverSettings { enabled: true, module: "SuperString".into(), run_minutes: 5, lock: false, lock_minutes: 5, fade_now: None, never_fade: None, options: Default::default() }
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
    /// With a presence sensor: lock the screen this many seconds after
    /// the person walks away (0: never).
    pub lock_when_away_seconds: u32,
    /// With a presence sensor: wake the displays (asleep for idleness)
    /// when the person comes back.
    pub wake_on_return: bool,
}

impl Default for PowerSettings {
    fn default() -> Self {
        PowerSettings { display_off_minutes: 10, sleep_minutes: 30, sleep_on_battery_only: false, lock_on_display_off: true, lock_when_away_seconds: 0, wake_on_return: false }
    }
}

/// Screen scaling. Olo is HiDPI-first: everything is laid out in logical
/// pixels and rendered at the output's scale (2x "Retina" by default).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DisplaySettings {
    /// Scale for outputs without their own entry in `outputs`.
    pub scale: f64,
    /// The main display's key (the desktop lives there); None, or one
    /// that isn't connected, means the first. Every display has a
    /// Deskbar, showing the applications with windows on it, and
    /// workspaces of its own.
    pub main: Option<String>,
    /// Per-display overrides, keyed by [`display_key`]: the monitor itself
    /// when its EDID names it, else the connector ("eDP-1"). Entries saved
    /// under a connector by older versions still apply, and move to the
    /// monitor's key when next changed.
    pub outputs: std::collections::BTreeMap<String, OutputConfig>,
}

/// A GPU, as `Request::Gpus` tells of it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Its PCI address ("0000:01:00.0").
    pub pci: String,
    /// "Intel Graphics", "NVIDIA RTX PRO 2000 Blackwell Generation Laptop GPU".
    pub name: String,
    pub driver: String,
    /// The one the computer started on.
    pub boot: bool,
    /// app_server's primary GPU (windows' buffers, the GPU for displays
    /// without one of their own): the one the computer started on.
    pub rendering: bool,
    /// The display connectors wired to it.
    pub connectors: Vec<String>,
}

/// Where a display's settings are kept: "make model serial" from its EDID
/// when all three are known, so they follow the monitor between ports;
/// else its connector.
pub fn display_key(connector: &str, make: &str, model: &str, serial: &str) -> String {
    let known = |s: &str| !s.trim().is_empty() && s != "Unknown";
    if known(make) && known(model) && known(serial) { format!("{} {} {}", make.trim(), model.trim(), serial.trim()) } else { connector.to_owned() }
}

/// A [`display_key`] that names a connector ("eDP-1"), not a monitor
/// (those are made, model and serial, spaced): what's on that port of
/// that computer, which means nothing on another (a backup brought there
/// leaves them out).
pub fn is_connector_key(key: &str) -> bool {
    !key.trim().contains(' ')
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

/// The look (theme, colour set, light or dark) and night mode. app_server
/// resolves `Auto` against the schedule and broadcasts the effective
/// theme in [`Appearance::effective_theme`]; [`Appearance::palette`] is
/// what everything draws with then.
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
    /// `olo_theme::looks::THEMES`; unknown ids draw the default).
    pub icon_theme: String,
    /// Which of the theme's colour sets everything is drawn in: `theme`
    /// (its own), `beos` or `catppuccin` (`olo_theme::looks::ColorSet`;
    /// unknown ids are the theme's own). [`Request::Looks`] lists them.
    pub colors: String,
    /// A new window glows for a moment, to draw the eye to it.
    pub glow_new_windows: bool,
    /// For how long (seconds).
    pub glow_seconds: u32,
}

impl Appearance {
    /// The palette everything draws with now.
    pub fn palette(&self) -> olo_theme::Palette {
        olo_theme::looks::palette(&self.icon_theme, &self.colors, self.effective_theme)
    }
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
            icon_theme: olo_theme::looks::DEFAULT.into(),
            colors: olo_theme::looks::ColorSet::Theme.id().into(),
            glow_new_windows: false,
            glow_seconds: 5,
        }
    }
}

/// A theme, as [`Reply::Looks`] lists it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LookInfo {
    /// What `Appearance::icon_theme` holds.
    pub id: String,
    pub name: String,
    pub about: String,
    pub colors: Vec<ColorSetInfo>,
}

/// One of a theme's colour sets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColorSetInfo {
    /// What `Appearance::colors` holds.
    pub id: String,
    pub name: String,
    pub about: String,
    /// Its terminal colours' ids, light and dark (Terminal's settings).
    pub terminal_light: String,
    pub terminal_dark: String,
}

impl LookInfo {
    pub fn all() -> Vec<LookInfo> {
        use olo_theme::{ThemeMode, looks};
        looks::THEMES
            .iter()
            .map(|t| LookInfo {
                id: t.id.into(),
                name: t.name.into(),
                about: t.about.into(),
                colors: looks::ColorSet::ALL
                    .into_iter()
                    .map(|c| ColorSetInfo {
                        id: c.id().into(),
                        name: c.name(t).into(),
                        about: c.about(t),
                        terminal_light: looks::palette(t.id, c.id(), ThemeMode::Light).terminal.id,
                        terminal_dark: looks::palette(t.id, c.id(), ThemeMode::Dark).terminal.id,
                    })
                    .collect(),
            })
            .collect()
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
    /// Two fingers pinching reach the window under them (zooming in a
    /// browser, a picture). Off by default: a pinch made by accident
    /// while scrolling zoomed Chrome's pages.
    pub pinch_zoom: bool,
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
    /// A pointing stick (a TrackPoint) scrolls while its middle button
    /// is held, as a ThinkPad's does.
    pub pointing_stick_scroll: bool,
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
    /// The keyboard profile in use (Mac, Windows, BeOS, or one changed or
    /// saved from them): the command key, how keys behave, and every
    /// shortcut.
    pub profile: KeyboardProfile,
    /// Profiles saved under their own names, and a platform's once changed
    /// ("Mac (Customized)"), to pick again.
    pub profiles: Vec<KeyboardProfile>,
    /// The keyboards (by name, as `Request::Keyboards` lists them) whose
    /// Alt and Super (Windows) keys change places, left and right: on a
    /// PC keyboard Alt is where a Mac's ⌘ is, so with the Mac profile the
    /// key beside the space bar is ⌘ again (Ben's ThinkPad), while an
    /// Apple keyboard on the same computer is left as it is.
    pub swap_alt_super: Vec<String>,
    /// Num Lock is on when Olo starts (and is set so when this
    /// changes); the Num Lock key still toggles it.
    pub num_lock: bool,
    /// Before keyboard profiles (read once, into `profile`): ⌘ was the
    /// command key, Caps Lock a Ctrl, a tap of ⌘ the palette.
    #[serde(skip_serializing)]
    pub command_key: Option<bool>,
    #[serde(skip_serializing)]
    pub caps_lock_control: Option<bool>,
    #[serde(skip_serializing)]
    pub find_on_command_tap: Option<bool>,
    /// Apps that keep physical Ctrl for themselves (terminals): ⌘ reaches
    /// them as Ctrl+Shift, so Ctrl+C still interrupts. Matched on app_id.
    pub terminal_apps: Vec<String>,
    /// Web browsers: ⌘⇧[ / ⌘⇧] and ⌘⌥← / ⌘⌥→ reach them as the tab
    /// before / after (Ctrl+PgUp / Ctrl+PgDn), as in Olo's own apps.
    /// (⌘[ / ⌘] are Back / Forward, Alt+← / Alt+→, in every program that
    /// isn't Olo's.) Matched on app_id.
    pub browser_apps: Vec<String>,
    /// Keyboard hints: pressing Control twice quickly puts a letter on
    /// everything that can be picked (windows, controls, menu items);
    /// typing one picks it, or letters what's inside it.
    pub hints: bool,
    /// How quickly a double press (Control twice, for hints) must come,
    /// in milliseconds: the second press within this of the first.
    pub double_press_ms: u32,
}

/// [`Request::Overview`]'s answer: what the overview shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OverviewInfo {
    /// It is out (not leaving).
    pub shown: bool,
    /// Each display's workspaces, global logical pixels.
    pub panes: Vec<OverviewPane>,
    /// Each window, where it shows in its pane and the letter that picks
    /// it (a click there does the same).
    pub windows: Vec<OverviewWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OverviewPane {
    pub output: String,
    pub workspace: u32,
    /// The workspace showing.
    pub current: bool,
    pub frame: Frame,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OverviewWindow {
    pub id: WindowId,
    pub title: String,
    pub app_id: String,
    pub output: String,
    pub workspace: u32,
    pub frame: Frame,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter: Option<String>,
    /// Ringed: under the pointer, or the arrows' choice.
    pub chosen: bool,
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
            mice: Default::default(),
            tab_font_size: 12.0,
            tab_title_chars: 100,
            appearance: Appearance::default(),
            display: DisplaySettings::default(),
            power: PowerSettings::default(),
            screen_saver: ScreenSaverSettings::default(),
            clock: ClockSettings::default(),
            shortcuts: None,
            tiling: TilingSettings::default(),
            tracker: TrackerSettings::default(),
            screen_recording: ScreenRecordingSettings::default(),
            terminal: TerminalSettings::default(),
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
            pinch_zoom: false,
            tap_button_map: "lrm".into(),
            clickfinger_button_map: "lrm".into(),
            middle_emulation: false,
            dwtp: true,
            dwt_timeout_ms: 0,
            scroll_method: "two-finger".into(),
            left_handed: false,
            off_with_mouse: false,
            pointing_stick_scroll: true,
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
            profile: KeyboardProfile::default(),
            profiles: Vec::new(),
            swap_alt_super: Vec::new(),
            num_lock: true,
            command_key: None,
            caps_lock_control: None,
            find_on_command_tap: None,
            hints: false,
            double_press_ms: 400,
            terminal_apps: ["olo.terminal", "org.gnome.Ptyxis", "org.gnome.Console", "org.gnome.Terminal", "com.mitchellh.ghostty", "foot", "kitty", "Alacritty", "org.wezfurlong.wezterm"]
                .map(String::from)
                .to_vec(),
            browser_apps: ["google-chrome", "google-chrome-beta", "google-chrome-unstable", "chromium", "chromium-browser", "brave-browser", "microsoft-edge", "firefox", "org.mozilla.firefox"]
                .map(String::from)
                .to_vec(),
        }
    }
}

impl Settings {
    pub fn workspace_color(&self, index: u32) -> [u8; 3] {
        self.workspace_colors.get(index as usize).or_else(|| self.workspace_colors.first()).copied().unwrap_or([51, 102, 152])
    }

    /// Settings saved before keyboard profiles: their command key, Caps
    /// Lock, tap and shortcuts become the profile's, a Mac's (named
    /// "Mac (Customized)" if they aren't as a Mac's come). Done on load
    /// and on every change given (a backup restored from then).
    pub fn migrate(&mut self) {
        let k = &mut self.keyboard;
        let old = self.shortcuts.take();
        if k.command_key.is_none() && k.caps_lock_control.is_none() && k.find_on_command_tap.is_none() && old.is_none() {
            return;
        }
        let p = &mut k.profile;
        if let Some(on) = k.command_key.take() {
            p.command_key = if on { CommandKey::Command } else { CommandKey::Control };
        }
        if let Some(on) = k.caps_lock_control.take() {
            p.caps_lock_control = on;
        }
        if let Some(on) = k.find_on_command_tap.take() {
            p.palette_on_tap = on;
        }
        p.shortcuts.extend(old.unwrap_or_default());
        k.profile_changed();
    }
}

impl KeyboardSettings {
    /// The profiles to pick from: Mac, Windows and BeOS as they come, then
    /// those saved.
    pub fn choices(&self) -> Vec<KeyboardProfile> {
        let mut all: Vec<KeyboardProfile> = Platform::ALL.into_iter().map(KeyboardProfile::builtin).collect();
        all.extend(self.profiles.iter().filter(|p| !Platform::ALL.iter().any(|b| b.name() == p.name)).cloned());
        all
    }

    /// The profile in use was changed by hand: renamed if it was a
    /// platform's ("Mac (Customized)"), and kept among the saved ones so
    /// picking another doesn't lose it.
    pub fn profile_changed(&mut self) {
        self.profile.changed();
        if !Platform::ALL.iter().any(|b| b.name() == self.profile.name) {
            self.save_profile();
        }
    }

    /// Keep the profile in use among the saved ones, under its name.
    pub fn save_profile(&mut self) {
        match self.profiles.iter_mut().find(|p| p.name == self.profile.name) {
            Some(p) => *p = self.profile.clone(),
            None => self.profiles.push(self.profile.clone()),
        }
    }

    /// Use the profile named `name` (from [`Self::choices`]).
    pub fn pick_profile(&mut self, name: &str) -> bool {
        match self.choices().into_iter().find(|p| p.name == name) {
            Some(p) => {
                self.profile = p;
                true
            }
            None => false,
        }
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
    issue_activation_token(timeout)
}

/// A token from app_server that brings forward whatever window uses it
/// ([`Request::ActivationToken`]), whatever is in our environment (a
/// token there may be our opener's own, long spent): None if app_server
/// doesn't answer within `timeout`.
pub fn issue_activation_token(timeout: std::time::Duration) -> Option<String> {
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

    /// A handle on the connection that can shut it (from another thread,
    /// to end a [`Client::read`] waiting on it, and with it what the
    /// connection holds: grabbed keys, a `KeepAwake`).
    pub fn closer(&self) -> std::io::Result<UnixStream> {
        self.writer.try_clone()
    }

    /// Consume this client as an event stream: every event (after
    /// sending Subscribe).
    pub fn into_events(mut self) -> Result<impl Iterator<Item = Result<Event, ClientError>>, ClientError> {
        match self.request(&Request::Subscribe)? {
            Reply::Ok => {}
            other => return Err(ClientError::Server(format!("unexpected reply {other:?}"))),
        }
        Ok(self.events())
    }

    /// Consume this client as a stream of only these kinds of event
    /// ([`EventKind`]). An app_server from before kinds sends them all
    /// (so match on the events wanted, and filter `OwnWindows`' by pid).
    pub fn into_events_of(mut self, kinds: &[EventKind]) -> Result<impl Iterator<Item = Result<Event, ClientError>>, ClientError> {
        match self.request(&Request::SubscribeTo { events: kinds.to_vec() }) {
            Ok(Reply::Ok) => {}
            Err(ClientError::Server(_)) => match self.request(&Request::Subscribe)? {
                Reply::Ok => {}
                other => return Err(ClientError::Server(format!("unexpected reply {other:?}"))),
            },
            Ok(other) => return Err(ClientError::Server(format!("unexpected reply {other:?}"))),
            Err(e) => return Err(e),
        }
        Ok(self.events())
    }

    fn events(mut self) -> impl Iterator<Item = Result<Event, ClientError>> {
        std::iter::from_fn(move || match self.read() {
            Ok(Reply::Event { event }) => Some(Ok(event)),
            Ok(_) => None,
            Err(ClientError::Closed) => None,
            Err(e) => Some(Err(e)),
        })
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn settings_before_profiles_become_one() {
        // As saved before: nothing changed but Caps Lock.
        let old = r#"{"keyboard":{"command_key":true,"caps_lock_control":false,"find_on_command_tap":false,"hints":true},"shortcuts":{}}"#;
        let mut s: Settings = serde_json::from_str(old).unwrap();
        s.migrate();
        let k = &s.keyboard;
        assert_eq!((k.profile.name.as_str(), k.profile.caps_lock_control, k.hints), ("Mac (Customized)", false, true));
        assert_eq!(k.profiles.len(), 1);
        let saved = serde_json::to_value(&s).unwrap();
        assert!(saved.get("shortcuts").is_none() && saved["keyboard"].get("command_key").is_none(), "{saved}");
        // As a Mac's come, with a shortcut of its own.
        let old = r#"{"keyboard":{"command_key":true,"caps_lock_control":true},"shortcuts":{"lock":["Ctrl+Cmd+L"]}}"#;
        let mut s: Settings = serde_json::from_str(old).unwrap();
        s.migrate();
        assert_eq!(s.keyboard.profile.shortcuts["lock"], ["Ctrl+Cmd+L"]);
        assert_eq!(s.keyboard.profile.name, "Mac (Customized)");
        // Nothing changed: a Mac's.
        let mut s: Settings = serde_json::from_str(r#"{"keyboard":{"command_key":true},"shortcuts":{}}"#).unwrap();
        s.migrate();
        assert_eq!(s.keyboard.profile, KeyboardProfile::default());
        assert!(s.keyboard.profiles.is_empty());
        // Picking another keeps the changed one to come back to.
        let mut k = KeyboardSettings::default();
        k.profile.palette_on_tap = true;
        k.profile_changed();
        assert!(k.pick_profile("BeOS"));
        assert_eq!(k.profile.command_key, CommandKey::Alt);
        assert!(k.pick_profile("Mac (Customized)") && k.profile.palette_on_tap);
        assert_eq!(k.choices().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Mac", "Windows", "BeOS", "Mac (Customized)"]);
    }

    #[test]
    fn displays_numbered_left_to_right() {
        let out = |name: &str, x: i32, mirror: Option<&str>| OutputInfo {
            name: name.into(),
            key: name.into(),
            make: String::new(),
            model: String::new(),
            physical_size_mm: (0, 0),
            modes: Vec::new(),
            current_mode: None,
            scale: 1.0,
            position: (x, 0),
            rotation: 0,
            main: false,
            mirror_of: mirror.map(Into::into),
            brightness: None,
            number: None,
            panel: None,
            vrr: None,
            color: None,
            color_mode: None,
            hdr: None,
            sdr_nits: color::SDR_NITS,
            hdr_needs_boot: false,
        };
        let one = [out("eDP-1", 0, None), out("HDMI-A-1", 0, Some("eDP-1"))];
        assert_eq!(display_number(&one, "eDP-1"), None);
        assert_eq!(workspace_name(display_number(&one, "eDP-1"), 0), "Workspace 1");
        // No main display: the leftmost is 0.
        let three = [out("eDP-1", 4480, None), out("DP-1", 1920, None), out("DP-2", 0, None), out("HDMI-A-1", 0, Some("DP-2"))];
        let numbers: Vec<Option<i32>> = ["DP-2", "DP-1", "eDP-1", "HDMI-A-1"].iter().map(|o| display_number(&three, o)).collect();
        assert_eq!(numbers, [Some(0), Some(1), Some(2), None]);
        assert_eq!(workspace_name(Some(2), 3), "D2 Workspace 4");
        // From the main display: the one left of it is −1.
        let main = [out("eDP-1", 4480, None), OutputInfo { main: true, ..out("DP-1", 1920, None) }, out("DP-2", 0, None)];
        let numbers: Vec<Option<i32>> = ["DP-2", "DP-1", "eDP-1"].iter().map(|o| display_number(&main, o)).collect();
        assert_eq!(numbers, [Some(-1), Some(0), Some(1)]);
        assert_eq!(workspace_name(Some(-1), 1), "D−1 Workspace 2");
        assert_eq!(display_label(0), "D0");
    }
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
        let s = serde_json::to_string(&Request::ActivateAll { ids: vec![7, 9] }).unwrap();
        assert_eq!(s, r#"{"type":"activate_all","ids":[7,9]}"#);
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
