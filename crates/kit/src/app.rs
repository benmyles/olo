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
    pub keyboard: bool,
    pub namespace: String,
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
    pub fn raw_keys(mut self) -> Self {
        self.raw_keys = true;
        self
    }
}

pub(crate) enum Request {
    Open(WindowId, WindowOptions),
    Close(WindowId),
    SetTitle(WindowId, String),
    Resize(WindowId, (u32, u32)),
    Redraw(WindowId),
    RedrawAll,
    #[allow(dead_code)] // needs xdg-activation; kept for the API
    Activate(WindowId),
    Copy(String),
    Paste(WindowId),
    Quit,
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
    pub fn redraw(&mut self, window: WindowId) {
        self.requests.push(Request::Redraw(window));
    }
    pub fn redraw_all(&mut self) {
        self.requests.push(Request::RedrawAll);
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
    pub fn paste(&mut self, window: WindowId) {
        self.requests.push(Request::Paste(window));
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
