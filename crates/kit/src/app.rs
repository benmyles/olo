//! The application side of the Interface Kit: what an app implements
//! ([`App`]) and what it can ask the kit to do ([`Cx`]).

use std::{any::Any, sync::Arc};

use tabula_theme::Palette;

use crate::menu::Menu;

/// Identifies one of the app's windows (toplevel or layer surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub(crate) u32);

/// A command chosen from a menu or triggered by its shortcut. BeOS apps
/// identify these by message constant; we use short strings.
pub type Command = String;

/// Everything an app does lives behind this trait. One instance per process
/// (the BApplication); it is told which window it is drawing.
pub trait App: 'static {
    /// Draw the contents of `window` below its menu bar.
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui);

    /// The window's menu bar, if it has one. Called every frame; return the
    /// same structure each time (it is cheap to rebuild).
    fn menu_bar(&mut self, _window: WindowId) -> Option<Vec<Menu>> {
        None
    }

    /// A menu item was chosen (or its shortcut pressed) in `window`.
    fn command(&mut self, _cx: &mut Cx, _window: WindowId, _command: &str) {}

    /// The window's close box was clicked. Default: close it.
    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        cx.close_window(window);
    }

    /// A message posted from another thread with [`Proxy::send`].
    fn message(&mut self, _cx: &mut Cx, _message: Box<dyn Any + Send>) {}

    /// Light/dark changed; the kit already restyled every window.
    fn theme_changed(&mut self, _cx: &mut Cx, _palette: &Palette) {}

    /// Files dropped on a window (Tracker drags etc.).
    fn files_dropped(&mut self, _cx: &mut Cx, _window: WindowId, _paths: Vec<std::path::PathBuf>) {}

    /// State to carry across an in-place upgrade. When Tabula is reloaded
    /// (`tabula-session reload`) and this app's binary has changed, the kit
    /// re-executes the new binary; whatever this returns is handed to it as
    /// [`Cx::restored_state`] so it can reopen its windows as they were.
    /// Apps whose binary didn't change simply reconnect, keeping all state.
    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        None
    }
}

/// How a new window should appear.
#[derive(Debug, Clone)]
pub struct WindowOptions {
    pub title: String,
    /// Logical size of the content area (decorations are app_server's).
    pub size: (u32, u32),
    pub min_size: Option<(u32, u32)>,
    pub max_size: Option<(u32, u32)>,
    pub kind: WindowKind,
    /// Paint the panel background behind `ui` (most windows want this).
    pub panel: bool,
    /// Terminal-style keyboard: Ctrl+key reaches the app as a key event
    /// (Ctrl+C is not Copy), Ctrl+Shift+C/X/V are Copy/Cut/Paste, and text
    /// is delivered with Alt held too.
    pub raw_keys: bool,
    /// The window this one belongs to (an alert's document): app_server
    /// opens it centred over the parent instead of at the cascade slot.
    pub parent: Option<WindowId>,
    /// An alert: without a `parent`, it belongs to whichever of the app's
    /// windows has the keyboard when it opens.
    pub alert: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowKind {
    /// A normal titled window.
    Normal,
    /// A desktop shell surface (Deskbar, the Tracker desktop).
    Layer(LayerOptions),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerOptions {
    pub layer: Layer,
    /// Edges to anchor to: top, bottom, left, right.
    pub anchor: (bool, bool, bool, bool),
    pub exclusive_zone: i32,
    pub margin: (i32, i32, i32, i32),
    pub keyboard: Keyboard,
    pub namespace: String,
    /// The output to sit on, by name (see [`Cx::outputs`]); None lets the
    /// compositor choose.
    pub output: Option<String>,
}

/// Whether a layer surface takes the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyboard {
    None,
    /// When clicked, like a window (the desktop).
    OnDemand,
    /// All keys while it is up (pickers and other modal overlays).
    Exclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Background,
    Bottom,
    Top,
    Overlay,
}

impl Default for WindowOptions {
    fn default() -> Self {
        WindowOptions {
            title: String::new(),
            size: (400, 300),
            min_size: None,
            max_size: None,
            kind: WindowKind::Normal,
            panel: true,
            raw_keys: false,
            parent: None,
            alert: false,
        }
    }
}

impl WindowOptions {
    pub fn new(title: impl Into<String>, width: u32, height: u32) -> Self {
        WindowOptions { title: title.into(), size: (width, height), ..Default::default() }
    }
    pub fn min_size(mut self, w: u32, h: u32) -> Self {
        self.min_size = Some((w, h));
        self
    }
    pub fn max_size(mut self, w: u32, h: u32) -> Self {
        self.max_size = Some((w, h));
        self
    }
    pub fn raw_keys(mut self) -> Self {
        self.raw_keys = true;
        self
    }
    /// A BAlert window sized for `message` (see `widgets::alert`), opened
    /// over the app's focused window.
    pub fn alert(title: impl Into<String>, message: &str) -> Self {
        let (w, h) = (crate::widgets::ALERT_WIDTH, crate::widgets::alert_height(message));
        WindowOptions { alert: true, ..WindowOptions::new(title, w, h).min_size(w, h) }
    }
    /// Open over `parent`, as its alert or sheet.
    pub fn parent(mut self, parent: WindowId) -> Self {
        self.parent = Some(parent);
        self
    }
}

/// Clipboard contents: (MIME type, bytes), in order of preference.
pub type Clip = Vec<(String, std::sync::Arc<[u8]>)>;

pub(crate) enum Request {
    Open(WindowId, WindowOptions),
    Close(WindowId),
    SetTitle(WindowId, String),
    Resize(WindowId, (u32, u32)),
    KeepAwake(WindowId, bool),
    Redraw(WindowId),
    RedrawAll,
    Activate(WindowId),
    Copy(String),
    CopyData(Clip),
    Paste(WindowId),
    LockSession,
    UnlockSession,
    Quit,
}

/// Delivered to [`App::message`] after [`Cx::lock_session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLock {
    /// The session is locked; these windows cover the outputs (one each).
    /// Sent again with the new set when outputs come and go.
    Locked(Vec<WindowId>),
    /// The compositor refused or ended the lock.
    Finished,
}

/// The app's handle to the kit during a callback.
pub struct Cx {
    pub(crate) requests: Vec<Request>,
    pub(crate) next_id: u32,
    pub(crate) palette: Arc<Palette>,
    pub(crate) proxy: Proxy,
    pub(crate) app_id: String,
    /// Toplevel content sizes, as of the start of the callback.
    pub(crate) sizes: std::collections::HashMap<WindowId, (u32, u32)>,
    /// Logical size of the (first) screen, once known.
    pub(crate) screen: Option<(u32, u32)>,
    pub(crate) outputs: Vec<String>,
    pub(crate) resident: bool,
}

/// Where an upgrading app leaves its saved state for its new self.
pub(crate) const RESTORE_ENV: &str = "TABULA_RESTORE";

impl Cx {
    /// What the previous version of this app saved with
    /// [`App::save_state`] before an in-place upgrade (once; `None` on a
    /// normal start).
    pub fn restored_state(&mut self) -> Option<String> {
        let path = std::env::var_os(RESTORE_ENV)?;
        // SAFETY: start-up, before the app starts threads of its own.
        unsafe { std::env::remove_var(RESTORE_ENV) };
        let text = std::fs::read_to_string(&path).ok();
        let _ = std::fs::remove_file(&path);
        text
    }

    /// The logical content size of one of the app's toplevel windows.
    pub fn window_size(&self, window: WindowId) -> Option<(u32, u32)> {
        self.sizes.get(&window).copied()
    }

    /// The logical size of the screen (the first output), once known.
    /// Use it to fit windows that size to their content (previews).
    pub fn screen_size(&self) -> Option<(u32, u32)> {
        self.screen
    }

    /// The names of the connected outputs ("eDP-1", …), as far as known.
    pub fn outputs(&self) -> &[String] {
        &self.outputs
    }

    /// Open a window; it appears after the current callback returns.
    pub fn open_window(&mut self, options: WindowOptions) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        self.requests.push(Request::Open(id, options));
        id
    }
    pub fn close_window(&mut self, window: WindowId) {
        self.requests.push(Request::Close(window));
    }
    pub fn set_title(&mut self, window: WindowId, title: impl Into<String>) {
        self.requests.push(Request::SetTitle(window, title.into()));
    }
    /// Ask app_server to resize the window's content area.
    pub fn resize(&mut self, window: WindowId, size: (u32, u32)) {
        self.requests.push(Request::Resize(window, size));
    }
    /// Keep the screen from dimming, locking or sleeping while the window
    /// is visible (a playing video), until called with `false` or closed.
    pub fn keep_awake(&mut self, window: WindowId, on: bool) {
        self.requests.push(Request::KeepAwake(window, on));
    }
    pub fn redraw(&mut self, window: WindowId) {
        self.requests.push(Request::Redraw(window));
    }
    pub fn redraw_all(&mut self) {
        self.requests.push(Request::RedrawAll);
    }
    /// Bring the window to the front and give it the keyboard (restoring
    /// it if minimized or on another workspace).
    /// Lock the session (ext-session-lock): when the compositor agrees, the
    /// app is sent [`SessionLock::Locked`] with a window on every output,
    /// drawn through `App::ui` like any other. Only the lock screen does this.
    pub fn lock_session(&mut self) {
        self.requests.push(Request::LockSession);
    }
    /// End the session lock (after the user proved who they are).
    pub fn unlock_session(&mut self) {
        self.requests.push(Request::UnlockSession);
    }
    pub fn activate(&mut self, window: WindowId) {
        self.requests.push(Request::Activate(window));
    }
    /// Put text on the clipboard.
    pub fn copy(&mut self, text: impl Into<String>) {
        self.requests.push(Request::Copy(text.into()));
    }
    /// Deliver the clipboard's text to `window` as an `egui::Event::Paste`
    /// (for menu Paste; ⌘V arrives that way by itself).
    /// Put data on the clipboard in one or more forms, by MIME type
    /// (e.g. `[("image/png", png)]`); pasters pick the type they want.
    pub fn copy_data(&mut self, clip: Clip) {
        self.requests.push(Request::CopyData(clip));
    }

    pub fn paste(&mut self, window: WindowId) {
        self.requests.push(Request::Paste(window));
    }
    /// Keep running when the last window closes (a background service
    /// that opens windows on demand); [`Cx::quit`] still ends it.
    pub fn stay_resident(&mut self) {
        self.resident = true;
    }
    pub fn quit(&mut self) {
        self.requests.push(Request::Quit);
    }
    pub fn palette(&self) -> &Arc<Palette> {
        &self.palette
    }
    /// A handle background threads can use to message the app.
    pub fn proxy(&self) -> Proxy {
        self.proxy.clone()
    }
    pub fn app_id(&self) -> &str {
        &self.app_id
    }
}

/// Send messages to [`App::message`] from any thread.
#[derive(Clone)]
pub struct Proxy {
    pub(crate) sender: smithay_client_toolkit::reexports::calloop::channel::Sender<Box<dyn Any + Send>>,
}

impl Proxy {
    pub fn send<T: Any + Send>(&self, message: T) {
        let _ = self.sender.send(Box::new(message));
    }
}
