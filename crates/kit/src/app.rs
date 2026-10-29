//! The application side of the Interface Kit: what an app implements
//! ([`App`]) and what it can ask the kit to do ([`Cx`]).

use std::{any::Any, sync::Arc};

use olo_theme::Palette;

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

    /// Something was dropped on a part of `window` that took it with
    /// [`crate::dnd::accept`] (see [`crate::dnd`]).
    fn dropped(&mut self, _cx: &mut Cx, _window: WindowId, _drop: crate::dnd::Drop) {}

    /// A drag started with [`Cx::start_drag`] in `window` is over: dropped
    /// with `action`, or None if nothing took it. After a move, remove
    /// what was dragged unless it was dropped in this same app (which did
    /// the move itself).
    fn drag_ended(&mut self, _cx: &mut Cx, _window: WindowId, _action: Option<crate::dnd::Action>) {}

    /// State to carry across an in-place upgrade. When Olo is reloaded
    /// (`olo-session reload`) and this app's binary has changed, the kit
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
    /// A dialog or panel (Quick Look, an inspector) rather than a
    /// document window: app_server never makes it one of the app's tabs,
    /// so it keeps the size it asks for, bigger than its parent if it
    /// likes; with a `parent`, it opens over it and stays in front of it.
    pub dialog: bool,
    /// A utility window (a palette, a small tool such as DeskCalc) that
    /// floats over auto-tiled windows rather than taking a tile, though it
    /// can be any size. Dialogs and alerts float anyway. (The
    /// xdg-toplevel-tag `floating`.)
    pub floating: bool,
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
            dialog: false,
            floating: false,
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
        // Fixed, as BeOS alerts are: never resized, tiled or grouped with
        // the app's other windows (also when no window of ours has the
        // focus to be its parent, as when chosen from a popup menu).
        WindowOptions { alert: true, ..WindowOptions::new(title, w, h).min_size(w, h).max_size(w, h) }
    }
    /// Open over `parent`, as its alert or sheet.
    pub fn parent(mut self, parent: WindowId) -> Self {
        self.parent = Some(parent);
        self
    }
    /// [`parent`](Self::parent), if there is one.
    pub fn parent_opt(mut self, parent: Option<WindowId>) -> Self {
        self.parent = parent;
        self
    }
    /// A dialog or panel, not a document window (see [`Self::dialog`]).
    pub fn dialog(mut self) -> Self {
        self.dialog = true;
        self
    }
    /// A utility window that floats over auto-tiled ones (see
    /// [`Self::floating`]).
    pub fn floating(mut self) -> Self {
        self.floating = true;
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
    Fullscreen(WindowId, bool),
    KeepAwake(WindowId, bool),
    Redraw(WindowId),
    RedrawAll,
    Activate(WindowId),
    ActivateWith(WindowId, String),
    Token(WindowId, u64),
    SetKeyboard(WindowId, Keyboard),
    Copy(String),
    CopySecret(String, std::time::Duration),
    CopyData(Clip),
    StartDrag(WindowId, crate::dnd::DragData),
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

/// Delivered to [`App::message`] after [`Cx::activation_token`]: an
/// xdg-activation token for another program to bring its window forward
/// with (`XDG_ACTIVATION_TOKEN`, a notification's `ActivationToken`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationToken {
    pub tag: u64,
    pub token: String,
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
    /// Toplevels filling their screen ([`Cx::set_fullscreen`]).
    pub(crate) fullscreen: std::collections::HashSet<WindowId>,
    /// Logical size of the (first) screen, once known.
    pub(crate) screen: Option<(u32, u32)>,
    pub(crate) outputs: Vec<String>,
    pub(crate) resident: bool,
}

/// Where an upgrading app leaves its saved state for its new self.
pub(crate) const RESTORE_ENV: &str = "OLO_RESTORE";

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

    /// Whether one of the app's toplevel windows is still open. After
    /// [`Cx::quit`] an app holding the clipboard runs on with its windows
    /// gone, and a launch then must open a new window, not activate the
    /// old one.
    pub fn is_open(&self, window: WindowId) -> bool {
        self.sizes.contains_key(&window)
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
    /// Ask app_server to resize the window's content area. A zoomed
    /// window keeps the size it was zoomed to, and app_server holds a
    /// tiled one to its tile and keeps any on its display (an app that
    /// grows it past the screen is asked to fit, down to its minimum).
    pub fn resize(&mut self, window: WindowId, size: (u32, u32)) {
        self.requests.push(Request::Resize(window, size));
    }
    /// Fill the screen with the window (no tab or border), or put it back
    /// at its own size and place.
    pub fn set_fullscreen(&mut self, window: WindowId, on: bool) {
        self.requests.push(Request::Fullscreen(window, on));
    }
    /// Whether the window fills its screen, as of the start of the
    /// callback (the compositor may take it out of fullscreen itself).
    pub fn is_fullscreen(&self, window: WindowId) -> bool {
        self.fullscreen.contains(&window)
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
    /// Bring `window` forward with a token another program handed over
    /// (a notification's click, as [`notify::NoteEvent::Action`](crate::notify::NoteEvent)
    /// gives it); without one, as [`activate`](Self::activate).
    pub fn activate_with(&mut self, window: WindowId, token: Option<String>) {
        self.requests.push(match token {
            Some(t) => Request::ActivateWith(window, t),
            None => Request::Activate(window),
        });
    }
    /// Ask for an activation token from the latest input in `window` (a
    /// click), to hand to another program so its window may come forward;
    /// it arrives as an [`ActivationToken`] message with `tag`.
    pub fn activation_token(&mut self, window: WindowId, tag: u64) {
        self.requests.push(Request::Token(window, tag));
    }
    /// Change whether a layer window takes the keyboard (one showing a
    /// text field only while it does).
    pub fn set_keyboard(&mut self, window: WindowId, keyboard: Keyboard) {
        self.requests.push(Request::SetKeyboard(window, keyboard));
    }
    /// Put text on the clipboard.
    /// Start dragging `data` out of `window` (see [`crate::dnd`]): call it
    /// when an egui drag starts, while the pointer button is still down.
    pub fn start_drag(&mut self, window: WindowId, data: crate::dnd::DragData) {
        self.requests.push(Request::StartDrag(window, data));
    }

    pub fn copy(&mut self, text: impl Into<String>) {
        self.requests.push(Request::Copy(text.into()));
    }
    /// Put a secret (a password, a one-time code) on the clipboard: it is
    /// marked for clipboard managers to leave alone, and taken off again
    /// after `clear_after` unless something else was copied meanwhile.
    pub fn copy_secret(&mut self, text: impl Into<String>, clear_after: std::time::Duration) {
        self.requests.push(Request::CopySecret(text.into(), clear_after));
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
    /// Ask app_server to tile this app's resizable windows in a grid on
    /// the active display, in front (a way to see all its documents at
    /// once). Off the UI thread; does nothing outside Olo.
    pub fn tile_my_windows(&self) {
        let app_id = self.app_id.clone();
        std::thread::spawn(move || {
            if let Ok(mut client) = olo_protocol::Client::connect() {
                let _ = client.request(&olo_protocol::Request::TileApp { app_id });
            }
        });
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
