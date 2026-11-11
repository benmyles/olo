//! Terminal's windows on olo-kit. Terminal is one struct holding every
//! window's state (`Terminal::windows`, by a `WindowId` of its own),
//! behind a lock ([`terminal`]): the kit gives each window a looper of
//! its own, and Terminal's tabs move between its windows, so what a
//! window does is decided on Terminal alone (main.rs), whichever window's
//! looper asks. Each window's own view ([`Shell`]) knows only which it is
//! and has Terminal say what it shows; the window operations Terminal
//! asks for (open, close, redraw, activate, resize, title) are [`Cx`]'s,
//! posted to the window, so carried out once what asked is over.
//!
//! A terminal window is drawn here: the menu bar, the tab strip (along
//! the top or down the side), the screens (one, or, Tile All Tabs, a
//! pane for each tab with its head, rows split by draggable dividers),
//! the visual bell. The screens are a view of their own ([`Panes`]):
//! a shell's output draws them again and nothing else of the window.
//! Keys go to the shell unless they're one of Terminal's menu shortcuts;
//! text typed (and an input method's) comes through `TextInput`.
//!
//! Nothing that waits is done with Terminal locked, and nothing takes
//! the lock while holding it (a handler locks it, has it do something,
//! lets go).

use std::{
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use olo_kit::{
    Bounds, Child, CursorStyle, DragAction, DropAccept, DropOffer, Dropped, Element, FocusHandle, FontWeight, Hsla, InteractiveElement as _, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Painter, ParentElement as _, PathBuilder, Pixels, Point, Rgba, ScrollWheelEvent, SharedString, Styled as _, TextInput, View, WindowCx, WindowHandle, canvas, div,
    fill, icons,
    menu::{self, Menu, MenuBar},
    metrics as m, outline, point, px, quad, size, style, theme,
    widgets::{self, Button, CheckBox, ListModel, StripAction, StripGroup, StripTab, TabStrip, TextControl, TextField, TextFieldEvent},
};

use crate::{
    Terminal, Win,
    agent::State,
    groups, repos, view,
    view::{Metrics, Mods},
};

/// A view's context in one of Terminal's windows.
pub type ShellCx<'a, 'b> = olo_kit::Cx<'a, 'b, Shell>;

/// One of Terminal's windows, by its own count (never reused).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WindowId(pub(crate) u64);

/// The text field one of Terminal's small windows has (Find's, a group's
/// name, a password): made with the window, by its view.
#[derive(Clone, Default)]
pub struct FieldSpec {
    pub text: String,
    pub name: Option<String>,
    pub hint: Option<String>,
    pub password: bool,
    /// Its text selected as it opens.
    pub select_all: bool,
    /// In the fixed-width font (a branch's name).
    pub mono: bool,
    /// Return in it: the window's default button.
    pub entered: Option<fn(&mut Terminal, &mut Cx, WindowId)>,
    /// Something typed in it (the window is drawn again either way).
    pub changed: Option<fn(&mut Terminal, &mut Cx, WindowId)>,
}

/// What a window was opened with (in points).
#[derive(Clone)]
pub struct WindowOptions {
    pub title: String,
    pub size: (u32, u32),
    pub min_size: Option<(u32, u32)>,
    pub max_size: Option<(u32, u32)>,
    /// Opens with its tab under the pointer, to move with it (a tab torn off).
    pub under_pointer: bool,
    /// An alert for `message` (sized for it, fixed, modal).
    alert: Option<String>,
    /// A dialog: as tall as what it shows needs (`fits_content`), fixed.
    dialog: bool,
    /// What app_server knows the window by when Terminal comes back (the
    /// kit's `restore_key`).
    pub restore_key: Option<String>,
    /// Its text field, if it has one, and whether it has a list.
    pub field: Option<FieldSpec>,
    pub list: bool,
}

impl WindowOptions {
    pub fn new(title: impl Into<String>, w: u32, h: u32) -> WindowOptions {
        WindowOptions { title: title.into(), size: (w, h), min_size: None, max_size: None, under_pointer: false, alert: None, dialog: false, restore_key: None, field: None, list: false }
    }

    /// A dialog `w` wide: fixed, floating, as tall as what it shows needs
    /// (`h` at least).
    pub fn dialog(title: impl Into<String>, w: u32, h: u32) -> WindowOptions {
        WindowOptions { dialog: true, ..WindowOptions::new(title, w, h) }
    }

    /// An alert saying `message`: as tall as it needs, fixed, over the
    /// window with the keyboard.
    pub fn alert(title: impl Into<String>, message: &str) -> WindowOptions {
        WindowOptions { alert: Some(message.to_owned()), ..WindowOptions::new(title, 0, 0) }
    }

    pub fn min_size(mut self, w: u32, h: u32) -> Self {
        self.min_size = Some((w, h));
        self
    }

    pub fn max_size(mut self, w: u32, h: u32) -> Self {
        self.max_size = Some((w, h));
        self
    }

    /// With a text field (its view's: `Cx::field_text` reads it).
    pub fn field(mut self, field: FieldSpec) -> Self {
        self.field = Some(field);
        self
    }

    /// With a list (its view's `Shell::list`).
    pub fn list(mut self) -> Self {
        self.list = true;
        self
    }
}

/// A window as the kit has it, and what Terminal asks of it without
/// waiting for its looper: its size as last drawn, whether it's on the
/// workspace showing, its surface, what its text field says.
struct Handle {
    window: WindowHandle<Shell>,
    size: (u32, u32),
    here: bool,
    surface: Option<u32>,
    text: String,
}

/// Terminal's windows. (A lock of its own, never held while Terminal's
/// is taken: window operations are asked for while Terminal is locked.)
#[derive(Default)]
struct Wins {
    map: HashMap<WindowId, Handle>,
    next: u64,
}

fn wins() -> MutexGuard<'static, Wins> {
    static WINS: OnceLock<Mutex<Wins>> = OnceLock::new();
    WINS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

static TERMINAL: OnceLock<Mutex<Terminal>> = OnceLock::new();

/// Terminal, locked: every window's state and what's done with it. (Not
/// while it's held already, and held for nothing that waits. A window
/// opened as Terminal starts waits here until it's made.)
pub fn terminal() -> MutexGuard<'static, Terminal> {
    TERMINAL.wait().lock().unwrap_or_else(|e| e.into_inner())
}

/// Sets a timer of Terminal's on the app's looper: what it's to do then
/// is done with Terminal locked, and again in as long as it says.
pub fn set_timer(app: &mut olo_kit::AppCx, later: Later) {
    let Later { wait, stopped, mut then } = later;
    app.looper().after(wait, move |looper| {
        if stopped.load(Ordering::Acquire) {
            return;
        }
        let next = {
            let mut t = terminal();
            then(&mut t, &mut Cx)
        };
        if let Some(wait) = next {
            set_timer(&mut olo_kit::AppCx::on(looper), Later { wait, stopped, then });
        }
    });
}

/// Terminal as it starts.
pub fn set_terminal(t: Terminal) {
    let _ = TERMINAL.set(Mutex::new(t));
}

/// A handler that has Terminal do something: locked while it does, and
/// handed the window's context.
pub fn on<E: ?Sized + 'static>(f: impl Fn(&mut Terminal, &E, &mut WindowCx) + 'static) -> impl Fn(&E, &mut WindowCx) + 'static {
    move |e, w| {
        let mut t = terminal();
        f(&mut t, e, w)
    }
}

/// What a timer of Terminal's does when it's due: how long till it's to
/// do it again, if it is.
type Turn = Box<dyn FnMut(&mut Terminal, &mut Cx) -> Option<Duration> + Send>;

/// Something for Terminal to do later, on the app's looper (main.rs sets
/// the timer and has Terminal do it).
pub struct Later {
    pub wait: Duration,
    pub stopped: Arc<AtomicBool>,
    pub then: Turn,
}

/// A timer of Terminal's: dropped, what it was to do isn't done.
pub struct Timer(Arc<AtomicBool>);

impl Timer {
    /// One that has nothing to do.
    pub fn none() -> Timer {
        Timer(Arc::new(AtomicBool::new(true)))
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// What a window's view is told.
pub enum ShellMsg {
    /// What it shows changed: all of it said again.
    Redraw,
    /// Its screens changed (a shell's output): they alone are.
    Screens,
    /// Something to do with the view, on its looper.
    Do(Box<dyn FnOnce(&mut Shell, &mut ShellCx) + Send>),
}

/// Terminal's context: what it asks of its windows, each posted to the
/// window (done once what asked is over: a window can't be changed while
/// it's handling the event that asked), and of the app.
pub struct Cx;

impl Cx {
    pub fn open_window(&mut self, o: WindowOptions) -> WindowId {
        let id = {
            let mut wins = wins();
            wins.next += 1;
            WindowId(wins.next)
        };
        let (w, h) = match &o.alert {
            Some(message) => {
                let w = widgets::alert::alert_width(&["Cancel", "Close Anyway"]);
                (w, widgets::alert::alert_height(message, w))
            }
            None => (o.size.0 as f32, o.size.1 as f32),
        };
        let mut options = if o.alert.is_some() { olo_kit::WindowOptions::alert(o.title.clone(), w, h) } else { olo_kit::WindowOptions::new(o.title.clone(), w, h) };
        options.panel = o.alert.is_some() || !matches!(o.title.as_str(), "Terminal");
        options.min_size = o.min_size.map(|(w, h)| (w as f32, h as f32));
        options.max_size = o.max_size.map(|(w, h)| (w as f32, h as f32));
        if o.max_size.is_some_and(|max| Some(max) == o.min_size) {
            options.fixed = true;
        }
        if o.dialog {
            options = options.dialog();
            options.fixed = true;
        }
        options.under_pointer = o.under_pointer;
        options.restore_key = o.restore_key.clone();
        let (field, list) = (o.field.clone(), o.list);
        let text = field.as_ref().map(|f| f.text.clone()).unwrap_or_default();
        let window = olo_kit::open_window(options, move |cx| Shell::new(id, field, list, cx));
        wins().map.insert(id, Handle { window, size: (w as u32, h as u32), here: true, surface: None, text });
        id
    }

    pub fn close_window(&mut self, id: WindowId) {
        if let Some(h) = wins().map.remove(&id) {
            h.window.close();
        }
    }

    fn tell(&self, id: WindowId, message: ShellMsg) {
        if let Some(h) = wins().map.get(&id) {
            h.window.messenger().send(message);
        }
    }

    pub fn redraw(&mut self, id: WindowId) {
        self.tell(id, ShellMsg::Redraw);
    }

    /// A shell's output: the window's screens alone are drawn again.
    pub fn redraw_screens(&mut self, id: WindowId) {
        self.tell(id, ShellMsg::Screens);
    }

    pub fn redraw_all(&mut self) {
        for h in wins().map.values() {
            h.window.messenger().send(ShellMsg::Redraw);
        }
    }

    pub fn activate(&mut self, id: WindowId) {
        self.with_window(id, |_, cx| cx.activate_window());
    }

    pub fn activate_with(&mut self, id: WindowId, token: Option<String>) {
        self.with_window(id, move |_, cx| match &token {
            Some(t) => cx.activate_window_with_token(t),
            None => cx.activate_window(),
        });
    }

    pub fn resize(&mut self, id: WindowId, (w, h): (u32, u32)) {
        if let Some(handle) = wins().map.get_mut(&id) {
            handle.size = (w, h);
        }
        self.with_window(id, move |_, cx| cx.resize_window(size(px(w as f32), px(h as f32))));
    }

    /// Whether it's on the workspace showing (else it draws nothing until
    /// it is, and would jump then if resized now).
    pub fn is_shown(&self, id: WindowId) -> bool {
        self.on_this_workspace(id)
    }

    pub fn on_this_workspace(&self, id: WindowId) -> bool {
        wins().map.get(&id).is_some_and(|h| h.here)
    }

    pub fn copy(&mut self, text: String) {
        olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(text));
    }

    pub fn begin_move(&mut self, id: WindowId) {
        self.with_window(id, |_, cx| olo_kit::begin_move(cx));
    }

    pub fn proxy(&self) -> olo_kit_base::Proxy {
        olo_kit::proxy()
    }

    pub fn set_title(&mut self, id: WindowId, title: String) {
        self.with_window(id, move |_, cx| cx.set_window_title(title));
    }

    pub fn set_path(&mut self, id: WindowId, path: Option<PathBuf>) {
        self.with_window(id, move |_, cx| olo_kit::agent::set_path(cx, path));
    }

    /// Does `f` with window `id`'s view, on its looper, after this.
    pub fn with_window(&mut self, id: WindowId, f: impl FnOnce(&mut Shell, &mut ShellCx) + Send + 'static) {
        self.tell(id, ShellMsg::Do(Box::new(f)));
    }

    /// What window `id`'s text field says (as its view last told).
    pub fn field_text(&self, id: WindowId) -> String {
        wins().map.get(&id).map(|h| h.text.clone()).unwrap_or_default()
    }

    /// What window `id`'s field said, taken (a password: nothing of it
    /// is left here; whoever takes it wipes it when done).
    pub fn take_field_text(&mut self, id: WindowId) -> String {
        wins().map.get_mut(&id).map(|h| std::mem::take(&mut h.text)).unwrap_or_default()
    }

    /// Has Terminal do `f` in `wait` (on the app's looper).
    pub fn later(&mut self, wait: Duration, f: impl FnOnce(&mut Terminal, &mut Cx) + Send + 'static) {
        let mut f = Some(f);
        olo_kit::proxy().send(Later {
            wait,
            stopped: Arc::new(AtomicBool::new(false)),
            then: Box::new(move |t, cx| {
                if let Some(f) = f.take() {
                    f(t, cx);
                }
                None
            }),
        });
    }

    /// Has Terminal do `f` in `wait`, and again in as long as it says
    /// each time (None: no more), until the [`Timer`] is dropped.
    pub fn repeating(&mut self, wait: Duration, f: impl FnMut(&mut Terminal, &mut Cx) -> Option<Duration> + Send + 'static) -> Timer {
        let stopped = Arc::new(AtomicBool::new(false));
        olo_kit::proxy().send(Later { wait, stopped: stopped.clone(), then: Box::new(f) });
        Timer(stopped)
    }

    pub fn quit(&mut self) {
        olo_kit::quit();
    }
}

/// Window `id`'s content size as last drawn.
pub fn window_size(id: WindowId) -> Option<(u32, u32)> {
    wins().map.get(&id).map(|h| h.size)
}

/// Which of Terminal's windows the one with surface `surface` is, for a
/// window let go on another's strip.
pub fn window_of(surface: u32) -> Option<WindowId> {
    wins().map.iter().find(|(_, h)| h.surface == Some(surface)).map(|(id, _)| *id)
}

/// A window of Terminal's: which one it is, and what only a window has
/// (the keyboard's place, its text field, its list, its screens' view);
/// Terminal says what it shows.
pub struct Shell {
    pub id: WindowId,
    pub focus: FocusHandle,
    pub field: Option<Child<TextField>>,
    pub list: Option<ListModel>,
    /// A terminal window's screens.
    panes: Option<Child<Panes>>,
    /// A drop is over its screen (drawn round it).
    dropping: bool,
}

impl Shell {
    fn new(id: WindowId, field: Option<FieldSpec>, list: bool, cx: &mut ShellCx) -> Shell {
        let focus = FocusHandle::new();
        let field = field.map(|spec| {
            let f = widgets::text_field(cx, |mut f| {
                if let Some(name) = &spec.name {
                    f = f.name(name.clone());
                }
                if let Some(hint) = &spec.hint {
                    f = f.hint(hint.clone());
                }
                if spec.mono {
                    f = f.font(style::mono_font(FontWeight::NORMAL));
                }
                f.password(spec.password)
            });
            let (text, select_all) = (spec.text.clone(), spec.select_all);
            f.update(cx, |f, cx| {
                f.set_text(text, cx);
                if select_all {
                    f.select_all_text(cx);
                }
            });
            let (entered, changed, secret) = (spec.entered, spec.changed, spec.password);
            cx.subscribe(&f, move |sh: &mut Shell, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => {
                    sh.tell_text(cx);
                    if let Some(entered) = entered {
                        let mut t = terminal();
                        entered(&mut t, &mut Cx, sh.id);
                    }
                }
                TextFieldEvent::Changed => {
                    // (A password stays the field's until it's sent.)
                    if !secret {
                        sh.tell_text(cx);
                    }
                    if let Some(changed) = changed {
                        let mut t = terminal();
                        changed(&mut t, &mut Cx, sh.id);
                    }
                    cx.invalidate();
                }
                TextFieldEvent::Typed(_) => {}
            });
            f.focus(cx);
            f
        });
        if field.is_none() {
            focus.focus(cx);
        }
        Shell { id, focus, field, list: list.then(|| ListModel::new(false)), panes: None, dropping: false }
    }

    /// What the field says, where Terminal reads it (`Cx::field_text`).
    pub fn tell_text(&mut self, cx: &mut ShellCx) {
        if let Some(f) = &self.field {
            let text = f.read(cx).text().to_owned();
            if let Some(h) = wins().map.get_mut(&self.id) {
                h.text = text;
            }
        }
    }

    /// The field forgets what was typed in it (a password, sent or
    /// given up).
    pub fn clear_field(&mut self, cx: &mut ShellCx) {
        if let Some(f) = &self.field {
            f.update(cx, |f, cx| f.clear_secret(cx));
        }
    }
}

impl View for Shell {
    type Message = ShellMsg;

    fn message(&mut self, message: ShellMsg, cx: &mut ShellCx) {
        match message {
            ShellMsg::Redraw => {
                if let Some(p) = &self.panes {
                    p.update(cx, |_, cx| cx.invalidate());
                }
                cx.invalidate();
            }
            // (For the next frame drawn, once: a shell that floods says
            // so hundreds of times between two frames, and each was laid
            // out and painted, 15 ms of them a frame.)
            ShellMsg::Screens => match &self.panes {
                Some(p) => p.update(cx, |_, cx| cx.invalidate_at_frame()),
                None => cx.invalidate_at_frame(),
            },
            ShellMsg::Do(f) => f(self, cx),
        }
    }

    fn command(&mut self, command: &str, _: &mut ShellCx) -> bool {
        terminal().command(&mut Cx, self.id, command);
        true
    }

    /// Its close box asks Terminal, which may ask "Close anyway?".
    fn close_requested(&mut self, _: &mut ShellCx) -> bool {
        terminal().close_requested(&mut Cx, self.id);
        false
    }

    fn window_activated(&mut self, _: bool, cx: &mut ShellCx) {
        cx.invalidate();
    }

    fn takes_drops(&self) -> bool {
        true
    }

    /// Files dropped on a screen are typed as their paths, text as if
    /// pasted.
    fn drop_over(&mut self, offer: Option<&DropOffer>, at: Point<Pixels>, cx: &mut ShellCx) -> Option<DropAccept> {
        let over = offer.filter(|o| o.copy).and_then(|o| {
            let mime = if o.types.iter().any(|t| t == "text/uri-list") {
                Some("text/uri-list")
            } else {
                ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"].into_iter().find(|t| o.types.iter().any(|x| x == t))
            }?;
            terminal().screen_at(self.id, at).then(|| DropAccept { target: "screen".into(), mime: mime.into(), action: DragAction::Copy })
        });
        if self.dropping != over.is_some() {
            self.dropping = over.is_some();
            if let Some(p) = &self.panes {
                let on = self.dropping;
                p.update(cx, |p, cx| {
                    p.dropping = on;
                    cx.invalidate();
                });
            }
        }
        over
    }

    fn dropped(&mut self, d: Dropped, cx: &mut ShellCx) {
        self.drop_over(None, d.position, cx);
        let paths = if d.mime == "text/uri-list" { d.paths() } else { Vec::new() };
        let text = String::from_utf8_lossy(&d.data).into_owned();
        terminal().dropped(&mut Cx, self.id, &paths, Some(&text));
    }

    fn save_state(&mut self, _: &mut ShellCx) -> Option<String> {
        None
    }

    fn body(&mut self, cx: &mut ShellCx) -> Element {
        let s = cx.viewport_size();
        let surface = cx.surface_id();
        if let Some(h) = wins().map.get_mut(&self.id) {
            h.size = (f32::from(s.width).round() as u32, f32::from(s.height).round() as u32);
            h.surface = surface;
            h.here = surface.and_then(olo_kit::workspaces::here_surface).unwrap_or(true);
        }
        let mut t = terminal();
        t.render(self, cx)
    }
}

/// Text typed, and an input method's, for the shell showing.
impl TextInput for Shell {
    fn insert_text(&mut self, text: &str, _: &mut ShellCx) {
        let mut t = terminal();
        t.set_preedit(&mut Cx, self.id, None);
        t.typed_text(&mut Cx, self.id, text);
    }

    fn set_marked_text(&mut self, text: &str, _: &mut ShellCx) {
        terminal().set_preedit(&mut Cx, self.id, Some(text.to_owned()));
    }

    fn unmark_text(&mut self, _: &mut ShellCx) {
        terminal().set_preedit(&mut Cx, self.id, None);
    }

    fn delete_marked_text(&mut self, _: &mut ShellCx) {
        terminal().set_preedit(&mut Cx, self.id, None);
    }
}

/// A terminal window's screens: one, or a pane for each tab tiled. A
/// view of its own, so a shell's output draws them again and nothing
/// else of the window.
pub struct Panes {
    id: WindowId,
    /// The keyboard is in the window's screen.
    focused: bool,
    /// A drop is over the screen.
    dropping: bool,
    /// Where the input method was last told the cursor is.
    ime: Option<Bounds<Pixels>>,
}

impl View for Panes {
    type Message = ();

    fn body(&mut self, cx: &mut olo_kit::Cx<Panes>) -> Element {
        let mut t = terminal();
        let screens = t.render_panes(self.id, self.focused, self.dropping, cx);
        // Where an input method's window goes: the cursor's cell, as it
        // moves with what's typed (told when it's another).
        let at = if self.focused { t.cursor_bounds(self.id) } else { None };
        drop(t);
        if at != self.ime {
            self.ime = at;
            if let Some(at) = at {
                cx.set_ime_position(at);
            }
        }
        screens
    }
}

/// How tall the head over each pane in Tile All Tabs is: as the tabs over
/// it, or beside a side strip as tall as its first tab with the room
/// round it, so the tab is level with the heads.
pub fn pane_head_height(side: bool) -> f32 {
    if side { widgets::STRIP_SIDE_HEAD } else { widgets::STRIP_ROW }
}

/// Tile All Tabs' grid for `n` tabs in `area` that no groups put in
/// rows: as many columns as keep each pane nearest a terminal's usual
/// shape (the last row has the rest, across the width).
pub fn grid_columns(area: Bounds<Pixels>, n: usize) -> usize {
    let (aw, ah) = (f32::from(area.size.width), f32::from(area.size.height));
    let shape = |cols: usize| {
        let rows = n.div_ceil(cols);
        let aspect = (aw / cols as f32) / (ah / rows as f32);
        (aspect / 1.8).ln().abs()
    };
    (1..=n.max(1)).min_by(|&a, &b| shape(a).total_cmp(&shape(b))).unwrap_or(1)
}

/// Spans from `start` to `end` (logical pixels), a line apart, on device
/// pixels at `scale`, so the line between two meets both. Each has
/// `least` (when they all fit), and what is left is shared by `weights`
/// (as the person dragged them: 1 each at first).
pub fn spans_between(start: f32, end: f32, weights: &[f32], least: f32, scale: f32) -> Vec<(f32, f32)> {
    let line = style::line_pixels(scale);
    let d = |v: f32| (v * scale).round();
    let (top, bottom) = (d(start), d(end));
    let n = weights.len().max(1);
    let all = bottom - top + line;
    let least = if d(least) * n as f32 <= all { d(least) } else { 0.0 };
    let spare = all - least * n as f32;
    let total: f32 = weights.iter().map(|w| w.max(0.01)).sum::<f32>().max(0.01);
    let mut out = Vec::with_capacity(n);
    let mut y = top;
    for (r, w) in weights.iter().enumerate() {
        let h = if r + 1 == n { top + all - y } else { least + (spare * w.max(0.01) / total).floor() };
        let y1 = if r + 1 == n { bottom } else { y + h - line };
        out.push((y / scale, y1 / scale));
        y += h;
    }
    out
}

/// Tile All Tabs' rows in `area`: each row's top and bottom
/// ([`spans_between`]).
pub fn row_spans(area: Bounds<Pixels>, weights: &[f32], least: f32, scale: f32) -> Vec<(f32, f32)> {
    spans_between(f32::from(area.top()), f32::from(area.bottom()), weights, least, scale)
}

/// Tile All Tabs' panes in rows: row `r` as tall as `heights` gives it,
/// its panes side by side as wide as `widths[r]` gives them, each pane at
/// least `least` (wide, tall). Row by row, left to right.
pub fn tile_rows(area: Bounds<Pixels>, widths: &[Vec<f32>], heights: &[f32], least: (f32, f32), scale: f32) -> Vec<Vec<Bounds<Pixels>>> {
    row_spans(area, heights, least.1, scale)
        .into_iter()
        .zip(widths)
        .map(|((y0, y1), w)| {
            let w = if w.is_empty() { vec![1.0] } else { w.clone() };
            spans_between(f32::from(area.left()), f32::from(area.right()), &w, least.0, scale).into_iter().map(|(x0, x1)| Bounds::from_corners(point(px(x0), px(y0)), point(px(x1), px(y1)))).collect()
        })
        .collect()
}

/// Tile All Tabs' rows as laid out: each row's key (what its share of
/// the height is kept by), its top and bottom, its tabs and their panes'
/// left and right edges.
#[derive(Clone)]
pub(crate) struct LaidRow {
    pub key: String,
    pub span: (f32, f32),
    pub tabs: Vec<usize>,
    pub cols: Vec<(f32, f32)>,
}

/// A divider in Tile All Tabs: over row `.0` (from 1), or before pane
/// `.1` (from 1) of row `.0`.
#[derive(Clone, Copy)]
pub(crate) enum Divider {
    Row(usize),
    Pane(usize, usize),
}

/// Which way a divider being dragged moves.
#[derive(Clone)]
pub(crate) enum Across {
    /// Between rows: their heights, by the rows' keys.
    Rows(Vec<String>),
    /// Between panes in row `.0` (its key): their widths.
    Panes(String),
}

/// Tile All Tabs' divider being dragged: what it divides, as they were
/// when it was pressed, and where.
pub(crate) struct DividerDrag {
    across: Across,
    /// The span after the divider (from 1).
    at: usize,
    /// The pointer's position along the drag at the press.
    from: f32,
    spans: Vec<(f32, f32)>,
    weights: Vec<f32>,
    /// The least a span may be (a row: its head and two lines; a pane:
    /// a few columns).
    least: f32,
}

/// The weights after the divider before span `r` moved `dy` (logical
/// pixels): spans `r - 1` and `r` grow and shrink, each keeping `least`,
/// the others as they were. `spans` are as shown (rows, or a row's
/// panes).
pub fn drag_rows(weights: &[f32], spans: &[(f32, f32)], r: usize, dy: f32, least: f32) -> Vec<f32> {
    let mut out = weights.to_vec();
    if r == 0 || r >= weights.len() || spans.len() != weights.len() {
        return out;
    }
    let h = |i: usize| spans[i].1 - spans[i].0;
    let (a, b) = (h(r - 1) - least, h(r) - least);
    if a + b <= 0.0 {
        return out;
    }
    let a2 = (a + dy).clamp(0.0, a + b);
    let pair = weights[r - 1] + weights[r];
    out[r - 1] = pair * a2 / (a + b);
    out[r] = pair - out[r - 1];
    out
}

/// The colour `t` of the way from `a` to `b`.
fn lerp(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    Hsla::from(Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t })
}

/// Auto Approve's countdown over `screen`'s bottom right corner: "Yes in
/// N s" and Stop (the question is then the person's). Agents read it as a
/// Status, its Stop a button.
fn auto_approve_box(screen: Bounds<Pixels>, shell: u64, left: u32) -> Element {
    let text: SharedString = format!("Auto Approve: Yes in {left} s").into();
    let name = text.clone();
    let back = canvas(move |g, b| {
        let p = theme::palette();
        widgets::banner::banner_back(g, b, None, &p);
        g.agent(olo_kit::agent::Node { role: "Status", name: Some(name.clone()), bounds: b, enabled: true, ..Default::default() });
    })
    .paint_key(style::key(&text))
    .absolute()
    .inset_0();
    let stop = Button::new("Stop").small().id(SharedString::from(format!("auto-approve-stop-{shell}"))).on_click(on(move |t, _: &olo_kit::ClickEvent, _| t.auto_approve_stop(&mut Cx, shell)));
    let inset = px(m::SPACING);
    div().absolute().left(screen.left()).top(screen.top()).w(screen.size.width).h(screen.size.height).child(
        div()
            .absolute()
            .right(inset)
            .bottom(inset)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .pl(px(m::SPACING))
            .pr(px(m::TIGHT))
            .py(px(m::TIGHT))
            .child(back)
            .child(widgets::StringView::new(text))
            .child(stop),
    )
}

/// A tab's icon: Terminal's, with a badge at its corner: a turning arc
/// while an agent works, an orange "!" when it needs an answer, a green
/// dot when it's done and no one has looked, a blue dot for an unseen
/// bell; and at its bottom corner, while Keep It Going is on, a blue
/// disc with a "go on" triangle; at its other bottom corner, while Auto
/// Approve is on, a green one with a check.
pub fn tab_icon(rang: bool, agent: Option<State>, attention: bool, keep_going: bool, auto_approve: bool) -> widgets::strip::PaintFn {
    Rc::new(move |g: &mut Painter, r: Bounds<Pixels>| {
        icons::paint(g, r, "terminal");
        let p = theme::palette();
        let disc_at = |g: &mut Painter, c: Point<Pixels>, radius: f32, color: Hsla| {
            let b = Bounds { origin: point(c.x - px(radius), c.y - px(radius)), size: size(px(2.0 * radius), px(2.0 * radius)) };
            g.paint_quad(fill(b, color).corner_radii(px(radius)));
        };
        if auto_approve {
            let c = point(r.left() + px(1.5), r.bottom() - px(3.5));
            disc_at(g, c, 5.5, style::c(p.panel_background));
            disc_at(g, c, 4.5, style::c(p.success));
            let mut path = PathBuilder::stroke(px(1.25));
            path.move_to(point(c.x - px(2.0), c.y));
            path.line_to(point(c.x - px(0.5), c.y + px(1.5)));
            path.line_to(point(c.x + px(2.0), c.y - px(1.5)));
            if let Ok(path) = path.build() {
                g.paint_path(path, style::ink_on(p.success));
            }
        }
        if keep_going {
            // A disc in the accent colour with a "go on" triangle, as the
            // question's is orange with a "!".
            let c = point(r.right() - px(1.5), r.bottom() - px(3.5));
            disc_at(g, c, 5.5, style::c(p.panel_background));
            let accent = p.keyboard_navigation;
            disc_at(g, c, 4.5, style::c(accent));
            let mut path = PathBuilder::fill();
            path.move_to(point(c.x - px(1.25), c.y - px(2.25)));
            path.line_to(point(c.x + px(2.25), c.y));
            path.line_to(point(c.x - px(1.25), c.y + px(2.25)));
            path.close();
            if let Ok(path) = path.build() {
                g.paint_path(path, style::ink_on(accent));
            }
        }
        let at = point(r.right() - px(1.5), r.top() + px(3.5));
        let back = style::c(p.panel_background);
        match agent {
            Some(State::Working) => {
                disc_at(g, at, 5.0, back);
                // A quarter turn every … : by the clock, so each time it's
                // drawn shows where it is now; and drawn again ten times
                // a second while it turns (it alone: what it's drawn in).
                let turn = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f32() % 1000.0) * 5.0;
                let pts: Vec<(Point<Pixels>, Point<Pixels>)> = (0..16)
                    .map(|k| {
                        let a = |k: i32| turn + k as f32 * (1.5 * std::f32::consts::PI / 16.0);
                        let p = |a: f32| point(at.x + px(3.0 * a.cos()), at.y + px(3.0 * a.sin()));
                        (p(a(k)), p(a(k + 1)))
                    })
                    .collect();
                g.paint_segments(&pts, px(1.5), true, style::c(p.keyboard_navigation));
                g.paint_again_within(0.1);
            }
            Some(State::Blocked) => {
                disc_at(g, at, 5.5, back);
                let orange = p.hue(olo_theme::Hue::Orange);
                disc_at(g, at, 4.5, style::c(orange));
                let white = style::ink_on(orange);
                g.paint_quad(fill(Bounds::centered_at(point(at.x, at.y - px(1.0)), size(px(1.5), px(4.0))), white));
                g.paint_quad(fill(Bounds::centered_at(point(at.x, at.y + px(2.25)), size(px(1.5), px(1.5))), white));
            }
            Some(State::Idle) if attention => {
                disc_at(g, at, 4.0, back);
                disc_at(g, at, 3.0, style::c(p.success));
            }
            _ if rang => {
                disc_at(g, at, 3.5, back);
                disc_at(g, at, 2.5, style::c(p.keyboard_navigation));
            }
            _ => {}
        }
    })
}

/// A pane's head in Tile All Tabs: its tab's icon and title (cut short
/// with "…"), the one typed into lit as the strip's selected tab is, the
/// others quiet; its group's chip at its right, so a group's panes show
/// together; a line around each pane keeps them apart.
#[allow(clippy::too_many_arguments)]
fn pane_head(g: &mut Painter, pane: Bounds<Pixels>, height: f32, title: &str, icon: &widgets::strip::PaintFn, active: bool, group: Option<(SharedString, Hsla)>) {
    let p = theme::palette();
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let bg = p.panel_background;
    let band = style::shade(&p, bg, 1.06);
    let edge = if dark { lerp(style::c(bg), olo_kit::white(), 0.14) } else { style::shade(&p, bg, 1.22) };
    let head = Bounds { origin: pane.origin, size: size(pane.size.width, px(height)) };
    let ground = if active { style::c(bg) } else { band };
    style::fill_rect(g, head, ground);
    style::hline_above(g, head.left(), head.right(), head.bottom(), edge);
    // Lines between panes (the window's frame does the outer edges).
    let line = style::hairline(g);
    style::vline(g, pane.right(), pane.top(), pane.bottom() + line, edge);
    style::hline(g, pane.left(), pane.right() + line, pane.bottom(), edge);
    // Centred on the band above the line under it, as near as the
    // device pixels allow.
    let cy = (head.top() + head.bottom() - line) / 2.0;
    // The icon over the pane's text, as a tab's is over the menu bar's
    // first title.
    let at = style::snap_centred(g, point(head.left() + px(view::PAD.0 + 8.0), cy), 16.0, 16.0);
    icon(g, at);
    let x = at.right() + px(m::MARK_GAP);
    let mut end = head.right() - px(m::SPACING);
    if let Some((name, color)) = group {
        let w = widgets::strip::chip_width(&name);
        let r = style::snap_centred(g, point(end - px(w / 2.0), cy), w, widgets::strip::CHIP_HEIGHT);
        widgets::strip::chip(g, r, &name, color, true, false);
        end = r.left() - px(m::SPACING);
    }
    let font = style::ui_font(if active { FontWeight::MEDIUM } else { FontWeight::NORMAL });
    let text = style::c(p.panel_text);
    let color = if active { text } else { lerp(text, ground, 0.32) };
    style::label_fit(g, x, cy, style::Align::Left, title.to_owned(), style::PLAIN_SIZE, font, color, end - x);
}

/// What a mouse event on a screen is, for Terminal.
pub enum PaneEvent {
    Down(MouseDownEvent),
    Move(MouseMoveEvent),
    Up(MouseUpEvent),
    Wheel(ScrollWheelEvent),
}

/// What a terminal window lays out.
struct Layout {
    strip: Bounds<Pixels>,
    area: Bounds<Pixels>,
    /// Each pane: its tab, its whole box, its screen.
    panes: Vec<(usize, Bounds<Pixels>, Bounds<Pixels>)>,
    /// Tile All Tabs' rows (none showing one tab).
    rows: Vec<LaidRow>,
}

/// The least a pane in Tile All Tabs may be dragged to: wide, tall (its
/// head and two lines).
fn least_pane(side: bool, metrics: &Metrics) -> (f32, f32) {
    let (w, h) = metrics.content_size(8, 2);
    (w, pane_head_height(side) + h)
}

/// An alert's keys: Return its default button, Escape its cancel.
fn alert_key(e: &KeyDownEvent) -> Option<bool> {
    if e.keystroke.modifiers.modified() {
        return None;
    }
    match e.keystroke.key.as_str() {
        "enter" => Some(true),
        "escape" => Some(false),
        _ => None,
    }
}

impl Terminal {
    /// Says what window `sh` shows.
    pub(crate) fn render(&mut self, sh: &mut Shell, cx: &mut ShellCx) -> Element {
        let id = sh.id;
        let alert = match self.windows.get(&id) {
            Some(Win::Term(_)) => return self.render_term(sh, cx),
            Some(Win::Find { .. }) => return self.render_find(sh, cx),
            Some(Win::NameGroup { .. }) => return self.render_name_group(sh, cx),
            Some(Win::KeepGoingMessage { .. }) => return self.render_keep_going_message(sh, cx),
            Some(Win::Daemon(_)) => return self.render_daemon(sh, cx),
            Some(Win::Sudo(_)) => return self.render_sudo(sh, cx),
            Some(Win::Worktree(_)) => return self.render_worktree(sh, cx),
            Some(Win::ConfirmClose { message, .. }) => ("terminal", message.clone(), &["Cancel", "Close"][..], None),
            Some(Win::ConfirmStartOver { .. }) => ("terminal", crate::START_OVER.to_owned(), &["Cancel", "Start Over"][..], None),
            Some(Win::ConfirmAutoApprove { message, .. }) => ("warning", message.clone(), &["Turn On", "Cancel"][..], Some((1, 1))),
            Some(Win::ConfirmRemove { message, .. }) => ("warning", message.clone(), &["Cancel", "Remove"][..], None),
            Some(Win::Failed { message }) => ("warning", message.clone(), &["OK"][..], None),
            None => return div(),
        };
        let (icon, message, buttons, keys) = alert;
        let (default, cancel) = keys.unwrap_or((buttons.len().saturating_sub(1), 0));
        self.render_alert_with(sh, icon, message, buttons, default, cancel)
    }

    /// An alert whose Return chooses button `default` and Escape `cancel`
    /// (the last and the first, but for one a program can bring up, as
    /// Auto Approve's: Cancel both, so a Return typed as it comes doesn't
    /// say yes).
    fn render_alert_with(&mut self, sh: &mut Shell, icon: &str, message: String, buttons: &[&str], default: usize, cancel: usize) -> Element {
        let (id, n) = (sh.id, buttons.len());
        let mut row = widgets::button_row();
        for (i, label) in buttons.iter().enumerate() {
            row = row.child(Button::new(label.to_string()).default(i == default).on_click(on(move |t, _: &olo_kit::ClickEvent, _| t.alert_chosen(&mut Cx, id, i))));
        }
        div()
            .size_full()
            .track_focus(&sh.focus)
            .on_key_down(on(move |t, e: &KeyDownEvent, w| match alert_key(e) {
                Some(true) => {
                    w.stop_propagation();
                    t.alert_chosen(&mut Cx, id, default);
                }
                Some(false) if n > 1 => {
                    w.stop_propagation();
                    t.alert_chosen(&mut Cx, id, cancel);
                }
                _ => {}
            }))
            .child(widgets::AlertFrame::new(icon.to_owned()).child(widgets::paragraph(message)).child(div().flex_1()).child(row))
    }

    /// The cell metrics at the window's scale, made again when the text
    /// size or the scale changes (cells snapped to another scale's pixels
    /// can still be whole ones here and put the rows between pixels).
    pub(crate) fn metrics_for(&mut self, scale: f32) -> Metrics {
        let size = self.settings.text_size;
        match self.metrics {
            Some(m) if m.size == size && m.scale == scale => m,
            _ => {
                let m = Metrics::new(scale, size);
                self.metrics = Some(m);
                m
            }
        }
    }

    /// Where a terminal window's parts go, in a window `vs` at `scale`.
    fn lay_out(&self, id: WindowId, vs: olo_kit::Size<Pixels>, scale: f32, metrics: &Metrics) -> Option<Layout> {
        let Some(Win::Term(t)) = self.windows.get(&id) else { return None };
        let top = px((menu::BAR_HEIGHT * scale).round() / scale);
        let full = Bounds::from_corners(point(px(0.0), top), point(vs.width, vs.height));
        let side = self.settings.side_tabs;
        let (strip, area) = if side {
            // As wide as chosen, leaving the screen some room.
            let width = px(self.settings.side_width.min((f32::from(full.size.width) / 2.0).floor()));
            (Bounds { origin: full.origin, size: size(width, full.size.height) }, Bounds::from_corners(point(full.left() + width, full.top()), full.bottom_right()))
        } else {
            let h = px(widgets::STRIP_HEIGHT);
            (Bounds { origin: full.origin, size: size(full.size.width, h) }, Bounds::from_corners(point(full.left(), full.top() + h), full.bottom_right()))
        };
        // A row for each group; tabs no groups put in rows, a grid's rows
        // (each kept by its group's key and its place: "build/1").
        let tiles = t.tile_rows();
        let several = t.tabs.len() > 1 && t.tiling_in(&tiles);
        let mut laid: Vec<(String, Vec<usize>)> = tiles.into_iter().map(|r| (t.row_key(&r), r)).collect();
        if let [(key, one)] = laid.as_slice()
            && one.len() > 1
        {
            let cols = grid_columns(area, one.len());
            laid = one.chunks(cols).enumerate().map(|(k, c)| (format!("{key}/{k}"), c.to_vec())).collect();
        }
        let least = least_pane(side, metrics);
        let mut rows = Vec::new();
        let boxes: Vec<(usize, Bounds<Pixels>)> = if several {
            let heights: Vec<f32> = laid.iter().map(|(k, _)| t.tile_heights.get(k).copied().unwrap_or(1.0)).collect();
            let widths: Vec<Vec<f32>> = laid.iter().map(|(k, tabs)| t.tile_widths.get(k).filter(|w| w.len() == tabs.len()).cloned().unwrap_or_else(|| vec![1.0; tabs.len()])).collect();
            let tiles = tile_rows(area, &widths, &heights, least, scale);
            let mut boxes = Vec::new();
            for ((key, tabs), panes) in laid.into_iter().zip(tiles) {
                let span = panes.first().map_or((0.0, 0.0), |p| (f32::from(p.top()), f32::from(p.bottom())));
                rows.push(LaidRow { key, span, tabs: tabs.clone(), cols: panes.iter().map(|p| (f32::from(p.left()), f32::from(p.right()))).collect() });
                boxes.extend(tabs.into_iter().zip(panes));
            }
            boxes
        } else {
            vec![(t.active, area)]
        };
        let panes = boxes
            .into_iter()
            .map(|(i, b)| {
                let screen = if several { Bounds::from_corners(point(b.left(), b.top() + px(pane_head_height(side))), b.bottom_right()) } else { b };
                (i, b, screen)
            })
            .collect();
        Some(Layout { strip, area, panes, rows })
    }

    /// Whether `at` in window `id` is on a screen (for what's dropped).
    pub(crate) fn screen_at(&self, id: WindowId, at: Point<Pixels>) -> bool {
        let Some(Win::Term(t)) = self.windows.get(&id) else { return false };
        t.tabs.iter().any(|tab| tab.pane.lock().unwrap_or_else(|e| e.into_inner()).bounds.get().contains(&at))
    }

    /// A terminal window: its menu bar, its strip, its screens (a view of
    /// their own), the dividers between tiles, the bell's flash.
    fn render_term(&mut self, sh: &mut Shell, cx: &mut ShellCx) -> Element {
        let id = sh.id;
        // The desktop's look, when it's another.
        let look = theme::palette();
        if !Arc::ptr_eq(&look, &self.look) {
            self.follow(&look);
        }
        let scale = cx.scale_factor();
        let vs = cx.viewport_size();
        let metrics = self.metrics_for(scale);
        let scheme = self.scheme();
        let focused = cx.is_window_active() && sh.focus.is_focused(cx);
        if focused {
            self.front = Some(id);
        }
        // (Whatever changed shows here: kept on disk soon.)
        self.remember_soon(&mut Cx);
        let Some(layout) = self.lay_out(id, vs, scale, &metrics) else { return div() };
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return div() };
        if focused != t.focused {
            t.focused = focused;
            t.tab().report_focus(focused);
        }
        if focused {
            t.tab_mut().attention = false;
        }
        {
            let session = &t.tab().session;
            let mut current = session.shared.scheme.lock().unwrap();
            if *current != scheme {
                // Turned light or dark: a program that asked to hear is
                // told (DEC mode 2031), so it picks its colours again.
                let turned = current.dark() != scheme.dark();
                *current = scheme.clone();
                drop(current);
                if turned && session.shared.reports.load(Ordering::Relaxed) {
                    session.write(crate::io::light_or_dark(scheme.dark()).to_vec());
                }
            }
        }
        // The window's size for its grid, once the cells are known.
        if let Some((cols, rows)) = t.snap_to.take().filter(|_| !t.tiling()) {
            let (w, h) = metrics.content_size(cols, rows);
            let (cw, ch) = crate::chrome(&self.settings);
            let want = (w as u32 + cw, h as u32 + ch);
            if window_size(id) != Some(want) {
                Cx.resize(id, want);
            }
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return div() };
        let several = layout.panes.len() > 1;
        // Every tab has the window's grid, so a program in one that isn't
        // showing still lays itself out right; tiled, each its pane's.
        for &(i, _, screen) in &layout.panes {
            let size = metrics.grid_size(f32::from(screen.size.width), f32::from(screen.size.height));
            if several {
                t.tabs[i].session.resize(size, metrics.device_cell());
            } else {
                for tab in t.tabs.iter_mut() {
                    tab.session.resize(size, metrics.device_cell());
                }
            }
        }

        // The strip.
        let side = self.settings.side_tabs;
        let Some(Win::Term(t)) = self.windows.get(&id) else { return div() };
        let strip_groups: Vec<StripGroup> = t.groups.iter().map(|g| StripGroup { name: g.name.clone().into(), color: groups::color(g.color, &self.look) }).collect();
        let tabs: Vec<StripTab> = t
            .tabs
            .iter()
            .map(|tab| {
                let pr = repos::pr_of(&self.prs, tab);
                let status = match (tab.status(&self.keep_going_says), pr.map(|p| p.describe())) {
                    (Some(a), Some(b)) => Some(format!("{a}; {b}")),
                    (a, b) => a.or(b),
                };
                StripTab {
                    label: tab.title.clone().into(),
                    icon: Some(tab_icon(tab.rang, tab.agent.map(|(_, s)| s), tab.attention, tab.keep_going.is_some(), tab.auto_approve.is_some())),
                    status: status.map(Into::into),
                    badge: pr.map(repos::pr_badge),
                    group: tab.group,
                    faded: tab.hidden_tiled.then(|| "hidden when tiled".into()),
                }
            })
            .collect();
        let mut strip = TabStrip::new(format!("terminal-tabs-{}", id.0), tabs, t.active).groups(strip_groups).in_window(cx.surface_id());
        if t.tiled && t.tabs.len() > 1 {
            // Tile All Tabs: a group's chip chooses which are tiled; each
            // pane has its own head.
            strip = strip.group_selection((0..t.groups.len()).map(|g| t.tile_group_shown(g)).collect());
        }
        if side {
            strip = strip.side();
        }
        let strip = strip.on_action(on(move |t, a: &StripAction, w| t.strip_action(id, *a, w)));
        let strip = div().absolute().left(layout.strip.left()).top(layout.strip.top()).w(layout.strip.size.width).h(layout.strip.size.height).child(strip);

        // Behind everything, the screen's background: the device pixel a
        // fractional scale leaves past the last pane's edge (at 1.5×, the
        // window's bottom row) is the terminal's colour, not black.
        let behind = view::screen_colors(&t.tab().session, &scheme).0;
        // The screens: a view of their own (`render_panes`).
        let panes = match &sh.panes {
            Some(p) => {
                p.update(cx, |p, cx| {
                    if p.focused != focused {
                        p.focused = focused;
                        cx.invalidate();
                    }
                });
                p
            }
            None => sh.panes.insert(cx.new_view(|_| Panes { id, focused, dropping: false, ime: None })),
        };
        let mut root = div()
            .size_full()
            .relative()
            .bg(behind)
            .track_focus(&sh.focus)
            .on_text_input(cx.text_input())
            .on_key_down(on(move |t, e: &KeyDownEvent, w| t.key_down(id, e, w)))
            .on_key_up(on(move |t, e: &KeyUpEvent, w| t.key_up(id, e, w)))
            .child(div().absolute().inset_0().child(panes));
        let Some(Win::Term(t)) = self.windows.get(&id) else { return div() };
        // Between rows, the divider and the head of the row under it drag
        // the rows' heights (the pointer shows it on the divider); clicked,
        // the pane there is picked. Between panes side by side, the line
        // drags their widths.
        let dividers: Vec<(Divider, Bounds<Pixels>, Bounds<Pixels>)> = {
            let mut v = Vec::new();
            for (r, row) in layout.rows.iter().enumerate().skip(1) {
                let top = row.span.0;
                let press = Bounds::from_corners(point(layout.area.left(), px(top - 4.0)), point(layout.area.right(), px(top + pane_head_height(side))));
                let line = Bounds::from_corners(point(layout.area.left(), px(top - 4.0)), point(layout.area.right(), px(top + 4.0)));
                v.push((Divider::Row(r), press, line));
            }
            for (r, row) in layout.rows.iter().enumerate() {
                for c in 1..row.cols.len() {
                    let x = (row.cols[c - 1].1 + row.cols[c].0) / 2.0;
                    let b = Bounds::from_corners(point(px(x - 4.0), px(row.span.0)), point(px(x + 4.0), px(row.span.1)));
                    v.push((Divider::Pane(r, c), b, b));
                }
            }
            v
        };
        for (divider, press, line) in dividers {
            let (cursor, name) = match divider {
                Divider::Row(r) => (CursorStyle::ResizeUpDown, format!("Row Height {r}")),
                Divider::Pane(r, c) => (CursorStyle::ResizeLeftRight, format!("Pane Width {r}.{c}")),
            };
            let mark = canvas(move |g, _| {
                g.set_cursor_style(cursor, line);
                g.agent(olo_kit::agent::Node { role: "Splitter", name: Some(name.clone().into()), bounds: press, enabled: true, ..Default::default() });
            })
            .size_full();
            root = root.child(
                div()
                    .absolute()
                    .left(press.left())
                    .top(press.top())
                    .w(press.size.width)
                    .h(press.size.height)
                    .on_any_mouse_down(on(move |t, e: &MouseDownEvent, w| {
                        w.stop_propagation();
                        t.divider_down(id, divider, e.position, e.button, w);
                    }))
                    .child(mark),
            );
        }
        // A divider being dragged: what it divides follows the pointer
        // till the button goes up, wherever in the window it goes.
        if let Some(rows) = t.dragging.as_ref().map(|d| matches!(d.across, Across::Rows(_))) {
            let everywhere = canvas(move |g, b| g.set_cursor_style(if rows { CursorStyle::ResizeUpDown } else { CursorStyle::ResizeLeftRight }, b)).absolute().inset_0();
            root = root
                .on_window_mouse_move(on(move |t, e: &MouseMoveEvent, _| {
                    let along = if rows { e.position.y } else { e.position.x };
                    t.divider_drag(id, (e.pressed_button == Some(MouseButton::Left)).then_some(along));
                }))
                .on_window_mouse_up(on(move |t, _: &MouseUpEvent, _| t.divider_drag(id, None)))
                .child(everywhere);
        }
        // Visual bell: a brief flash, fading by the clock (drawn again
        // each frame while it lasts, it alone).
        if let Some(rang) = t.tab().bell.filter(|at| at.elapsed().as_secs_f32() < 0.15) {
            let [r, g, b, _] = self.look.panel_text;
            let color = Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 };
            let area = layout.area;
            root = root.child(
                canvas(move |g, _| {
                    let age = rang.elapsed().as_secs_f32();
                    if age < 0.15 {
                        let a = (1.0 - age / 0.15) * 60.0 / 255.0 / 2.0;
                        g.paint_quad(fill(area, Rgba { a, ..color }));
                        g.paint_again_within(0.0);
                    }
                })
                .absolute()
                .inset_0(),
            );
        }
        let menus = self.menus(id).unwrap_or_default();
        let bar = MenuBar::raw(cx, menus);
        root.child(strip).child(div().absolute().top_0().left_0().right_0().child(bar))
    }

    /// A terminal window's screens: each drawn by view.rs, with its head
    /// when they're tiled, its scrollback's bar and Auto Approve's
    /// countdown; the pointer on each goes to Terminal with which shell
    /// it's over.
    pub(crate) fn render_panes(&mut self, id: WindowId, focused: bool, dropping: bool, cx: &mut olo_kit::Cx<Panes>) -> Element {
        let scale = cx.scale_factor();
        let vs = cx.viewport_size();
        let metrics = self.metrics_for(scale);
        let scheme = self.scheme();
        let side = self.settings.side_tabs;
        let Some(layout) = self.lay_out(id, vs, scale, &metrics) else { return div() };
        let Some(Win::Term(t)) = self.windows.get(&id) else { return div() };
        // (Every pane drawn has taken its wake-up, the tiles too.)
        for &(i, _, _) in &layout.panes {
            t.tabs[i].session.woke();
        }
        let several = layout.panes.len() > 1;
        let group_chips: Vec<(SharedString, Hsla)> = t.groups.iter().map(|g| (SharedString::from(g.name.clone()), groups::color(g.color, &self.look))).collect();
        let preedit = t.preedit.clone();
        let mut root = div().size_full().relative();
        for &(i, pane, screen) in &layout.panes {
            let tab = &t.tabs[i];
            let shell = tab.session.id;
            let active = i == t.active;
            let source = tab.session.source();
            let pane_cache = tab.pane.clone();
            let hover = tab.mouse.hover.clone();
            let scheme = scheme.clone();
            let head = several.then(|| {
                (
                    tab.title.clone(),
                    tab_icon(tab.rang, tab.agent.map(|(_, s)| s), tab.attention, tab.keep_going.is_some(), tab.auto_approve.is_some()),
                    tab.group.and_then(|g| group_chips.get(g).cloned()),
                )
            });
            let dim = (several && !active).then(|| view::screen_colors(&tab.session, &scheme).0);
            let preedit = preedit.clone().filter(|_| active);
            let link = hover.as_ref().is_some_and(|h| h.target.is_some());
            let dropping = dropping && active;
            let el = canvas(move |g, _| {
                if let Some((title, icon, group)) = &head {
                    pane_head(g, pane, pane_head_height(side), title, icon, active, group.clone());
                    // Panes after the first are where dividing the
                    // window puts them: level with the strip's tabs
                    // only by chance.
                    olo_kit::review::own_grid(g, pane);
                }
                // Its lines are a cell apart, not on the window's grid.
                olo_kit::review::own_grid(g, screen);
                let paint = view::Paint { source: &source, scheme: &scheme, metrics, focused: focused && active, hover: hover.as_ref(), preedit: preedit.as_deref() };
                let cache = pane_cache.lock().unwrap_or_else(|e| e.into_inner());
                g.with_content_mask(Some(olo_kit::ContentMask { bounds: screen }), |g| view::paint(g, screen, paint, &cache));
                drop(cache);
                if let Some(back) = dim {
                    // The panes not typed into, dimmed a little.
                    g.paint_quad(fill(screen, back.opacity(0.3)));
                }
                if dropping {
                    // Files dropped on it are typed as their paths,
                    // text as if pasted.
                    let p = theme::palette();
                    let inside = style::inset(g, screen, 1.0);
                    g.paint_quad(outline(inside, style::c(p.keyboard_navigation), olo_kit::BorderStyle::Solid).border_widths(px(2.0)));
                }
                g.set_cursor_style(if link { CursorStyle::PointingHand } else { CursorStyle::IBeam }, screen);
            })
            .absolute()
            .inset_0();
            root = root.child(el);
            // The pointer on the screen: presses and the wheel over it;
            // moves and releases wherever in the window (a selection
            // dragged past its edge, a button let go outside).
            root = root.child(
                div()
                    .absolute()
                    .left(screen.left())
                    .top(screen.top())
                    .w(screen.size.width)
                    .h(screen.size.height)
                    .on_any_mouse_down(on(move |t, e: &MouseDownEvent, w| t.pane_event(id, shell, screen, PaneEvent::Down(e.clone()), w)))
                    .on_window_mouse_move(on(move |t, e: &MouseMoveEvent, w| t.pane_event(id, shell, screen, PaneEvent::Move(e.clone()), w)))
                    .on_window_mouse_up(on(move |t, e: &MouseUpEvent, w| t.pane_event(id, shell, screen, PaneEvent::Up(e.clone()), w)))
                    .on_scroll_wheel(on(move |t, e: &ScrollWheelEvent, w| t.pane_event(id, shell, screen, PaneEvent::Wheel(e.clone()), w))),
            );
            // Its scrollback: a slim bar over the screen's right edge, there
            // when the pointer comes near or it scrolls; its thumb drags the
            // view, a click in its track pages (Ben, 2026-11-03).
            let (top, shown, total) = {
                use alacritty_terminal::grid::Dimensions as _;
                let term = tab.session.term.lock();
                let history = term.grid().history_size() as f32;
                let rows = term.screen_lines() as f32;
                (history - term.grid().display_offset() as f32, rows, history + rows)
            };
            let term = tab.session.term.clone();
            let bar = widgets::SlimScrollBar::new(SharedString::from(format!("terminal-scroll-{shell}")), top, shown, total).on_scroll(move |to: &f32, _| {
                use alacritty_terminal::grid::{Dimensions as _, Scroll};
                let mut t = term.lock();
                let history = t.grid().history_size();
                let (want, now) = (history.saturating_sub(to.round().max(0.0) as usize), t.grid().display_offset());
                if want != now {
                    t.scroll_display(Scroll::Delta(want as i32 - now as i32));
                    drop(t);
                    Cx.redraw_screens(id);
                }
            });
            root = root.child(div().absolute().left(screen.left()).top(screen.top()).w(screen.size.width).h(screen.size.height).child(bar));
            // Auto Approve waiting on a question: its countdown, in the
            // pane's corner, with Stop.
            if let Some(p) = tab.auto_approve.as_ref().and_then(|a| a.pending.as_ref()).filter(|p| !p.stopped) {
                let left = p.due.saturating_duration_since(std::time::Instant::now()).as_secs_f32().ceil() as u32;
                root = root.child(auto_approve_box(screen, shell, left));
            }
        }
        root
    }

    /// A key pressed in a terminal window.
    fn key_down(&mut self, id: WindowId, e: &KeyDownEvent, w: &mut WindowCx) {
        let k = &e.keystroke;
        // The command key (⌘, with a Mac's keys: the right Ctrl, as
        // app_server sends it; with others, Ctrl itself).
        let command = k.modifiers.control && (olo_kit::right_control_held() || !olo_kit_base::keys::command_key());
        // ⌘ with a menu shortcut's key is the app's (⌘C arrives as
        // Ctrl+Shift+C): left to the window's menu bar.
        if command && menu::shortcut_command(w, k).is_some() {
            return;
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        // Ctrl+Tab and Ctrl+Page Up/Down go through the tabs, as in other
        // terminals; the shell never sees them.
        let step = olo_kit::tab_step(k);
        if step != 0 && t.tabs.len() > 1 {
            let n = t.tabs.len() as i32;
            t.select((t.active as i32 + step).rem_euclid(n) as usize);
            w.stop_propagation();
            Cx.redraw(id);
            return;
        }
        let mut key = view::Key::of(k);
        if command {
            // Any other ⌘ goes to the shell as Ctrl+key (⌘R searches
            // history like Ctrl+R): without the Shift ⌘ brings.
            key.shift = false;
        }
        let tab = t.tab_mut();
        // (Typing is something happening: Keep It Going's quiet starts again.)
        if let Some(k) = tab.keep_going.as_mut() {
            k.since = std::time::Instant::now();
        }
        // (And a question Auto Approve waits on is the person's now.)
        let waiting = tab.auto_approve.as_ref().and_then(|a| a.pending.as_ref()).is_some_and(|p| !p.stopped).then_some(tab.session.id);
        if let Some(shell) = waiting {
            self.auto_approve_stop(&mut Cx, shell);
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        let tab = t.tab_mut();
        if view::key(&tab.session, key, true, e.is_held) {
            let hovered = tab.mouse.hover.take().is_some();
            w.stop_propagation();
            // (What it types comes back as the shell's output, which
            // draws the screen; a link lit under the pointer goes now.)
            if hovered {
                Cx.redraw_screens(id);
            }
            self.typed(&mut Cx);
        }
    }

    fn key_up(&mut self, id: WindowId, e: &KeyUpEvent, w: &mut WindowCx) {
        let Some(Win::Term(t)) = self.windows.get(&id) else { return };
        let key = view::Key::of(&e.keystroke);
        if view::key(&t.tab().session, key, false, false) {
            w.stop_propagation();
        }
    }

    /// Text typed (or an input method's) in window `id`.
    pub(crate) fn typed_text(&mut self, cx: &mut Cx, id: WindowId, text: &str) {
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        let tab = t.tab_mut();
        if let Some(k) = tab.keep_going.as_mut() {
            k.since = std::time::Instant::now();
        }
        let waiting = tab.auto_approve.as_ref().and_then(|a| a.pending.as_ref()).is_some_and(|p| !p.stopped).then_some(tab.session.id);
        if let Some(shell) = waiting {
            self.auto_approve_stop(cx, shell);
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        let tab = t.tab_mut();
        view::text(&tab.session, text);
        if tab.mouse.hover.take().is_some() {
            cx.redraw_screens(id);
        }
        self.typed(cx);
    }

    pub(crate) fn set_preedit(&mut self, cx: &mut Cx, id: WindowId, text: Option<String>) {
        if let Some(Win::Term(t)) = self.windows.get_mut(&id)
            && t.preedit != text
        {
            t.preedit = text;
            cx.redraw_screens(id);
        }
    }

    /// Where the cursor is in window `id` (for an input method's window).
    pub(crate) fn cursor_bounds(&self, id: WindowId) -> Option<Bounds<Pixels>> {
        let Some(Win::Term(t)) = self.windows.get(&id) else { return None };
        let m = self.metrics?;
        let rect = t.tab().pane.lock().unwrap_or_else(|e| e.into_inner()).bounds.get();
        let term = t.tab().session.term.try_lock_unfair()?;
        let c = term.grid().cursor.point;
        let offset = term.grid().display_offset() as i32;
        let x = f32::from(rect.left()) + view::PAD.0 + c.column.0 as f32 * m.cell.0;
        let y = f32::from(rect.top()) + view::PAD.1 + (c.line.0 + offset) as f32 * m.cell.1;
        Some(Bounds { origin: point(px(x), px(y)), size: size(px(m.cell.0), px(m.cell.1)) })
    }

    /// The pointer on the screen of tab `shell` of window `id`, drawn in
    /// `rect`.
    fn pane_event(&mut self, id: WindowId, shell: u64, rect: Bounds<Pixels>, e: PaneEvent, w: &mut WindowCx) {
        let Some(m) = self.metrics else { return };
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        let Some(i) = t.position(shell) else { return };
        let modifiers = match &e {
            PaneEvent::Down(e) => e.modifiers,
            PaneEvent::Move(e) => e.modifiers,
            PaneEvent::Up(e) => e.modifiers,
            PaneEvent::Wheel(e) => e.modifiers,
        };
        let command = modifiers.control && (olo_kit::right_control_held() || !olo_kit_base::keys::command_key());
        let mods = Mods::of(&modifiers, command);
        let mut out = view::Output { paste: false, context_menu: None, open: None };
        // (Only a press asks: a pointer moving costs nothing more.)
        let several = matches!(e, PaneEvent::Down(_)) && t.tiling();
        let tab = &mut t.tabs[i];
        let cwd = {
            let shared = tab.session.shared.clone();
            move || {
                let pid = shared.foreground().or(shared.shell())?;
                std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
            }
        };
        let mut redraw = false;
        let mut picked = false;
        // Where the pointer is over the rows as drawn (scrolled by the
        // point, or stretched: below their places).
        let pane = tab.pane.clone();
        let shift = px(pane.lock().unwrap_or_else(|e| e.into_inner()).shift.get());
        let on_rows = |p: Point<Pixels>| point(p.x, p.y - shift);
        match e {
            PaneEvent::Down(e) => {
                tab.mouse.hover_at(&tab.session, &m, rect, Some(on_rows(e.position)), mods.command, &cwd);
                tab.mouse.down(&tab.session, &m, rect, on_rows(e.position), e.button, e.click_count, mods, &mut out);
                redraw = true;
                // A press on a pane not typed into picks it.
                if several && i != t.active {
                    t.select(i);
                    picked = true;
                }
            }
            PaneEvent::Move(e) => {
                if !rect.contains(&e.position) && e.pressed_button.is_none() && tab.mouse.hover.is_none() {
                    return;
                }
                redraw |= tab.mouse.hover_at(&tab.session, &m, rect, Some(on_rows(e.position)), mods.command, &cwd);
                redraw |= tab.mouse.moved(&tab.session, &m, rect, on_rows(e.position), e.pressed_button == Some(MouseButton::Left), mods);
            }
            PaneEvent::Up(e) => {
                // (Only what was pressed here, or is over it.)
                if !rect.contains(&e.position) && !tab.mouse.held() {
                    return;
                }
                tab.mouse.up(&tab.session, &m, rect, on_rows(e.position), e.button, mods);
                redraw = true;
            }
            PaneEvent::Wheel(e) => {
                let dy = f32::from(e.delta.pixel_delta(px(m.cell.1)).y);
                let cache = pane.lock().unwrap_or_else(|e| e.into_inner());
                tab.mouse.wheel(&tab.session, &cache, &m, rect, on_rows(e.position), dy, (e.kind, e.touch_phase), mods);
                redraw = true;
            }
        }
        if picked {
            // (The strip's selected tab, the heads: all of it.)
            Cx.redraw(id);
        } else if redraw {
            Cx.redraw_screens(id);
        }
        if let Some(target) = &out.open {
            crate::links::open(target);
        }
        if out.paste {
            self.paste(&mut Cx, id);
        }
        if let Some(pos) = out.context_menu {
            let Some(Win::Term(t)) = self.windows.get(&id) else { return };
            let menu = Menu::new("")
                .item("Copy", "copy")
                .enabled(t.tab().session.has_selection())
                .item("Paste", "paste")
                .separator()
                .item("Select All", "select-all")
                .item("Clear Scrollback", "clear")
                .separator()
                .item(olo_kit_base::tracker::LABEL, olo_kit_base::tracker::COMMAND);
            self.popup(id, menu, pos, w);
        }
    }

    /// Opens `menu` at `pos` in window `id`; what's chosen is a command.
    pub(crate) fn popup(&mut self, id: WindowId, mut menu: Menu, pos: Point<Pixels>, w: &mut WindowCx) {
        // The bar's keys as its MenuBar shows them (raw: Terminal's own).
        if let Some(mut bar) = self.menus(id) {
            menu::resolve(&mut bar, true);
            menu.keys_from(&bar);
        }
        menu::context_menu(w, menu, pos, move |cmd, _| {
            let cmd = cmd.to_owned();
            terminal().command(&mut Cx, id, &cmd);
        });
    }

    /// The clipboard's text into the shell showing in window `id`, once
    /// it's read (off the looper: `paste_item`).
    pub(crate) fn paste(&mut self, _: &mut Cx, id: WindowId) {
        olo_kit::clipboard::paste_item(move |item| {
            let Some(text) = item.and_then(|c| c.text()) else { return };
            let t = terminal();
            if let Some(Win::Term(w)) = t.windows.get(&id) {
                view::paste(&w.tab().session, &text);
                Cx.redraw_screens(id);
            }
        });
    }

    /// A divider in Tile All Tabs pressed: dragged, the rows (or the
    /// panes) either side grow and shrink; a row's clicked, the pane there
    /// is picked; the right button, the row's menu.
    fn divider_down(&mut self, id: WindowId, divider: Divider, at: Point<Pixels>, button: MouseButton, w: &mut WindowCx) {
        let Some(m) = self.metrics else { return };
        let Some(layout) = self.lay_out(id, w.viewport_size(), w.scale_factor(), &m) else { return };
        let least = least_pane(self.settings.side_tabs, &m);
        // (For a group's menu: the windows its tabs could go to.)
        let others = self.other_windows(id);
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        let row = match divider {
            Divider::Row(r) | Divider::Pane(r, _) => r,
        };
        if button == MouseButton::Right {
            let group = layout.rows.get(row).and_then(|row| row.tabs.first()).and_then(|&i| t.tabs[i].group);
            let changed = t.sizes_changed();
            let menu = match group {
                Some(g) => crate::group_menu(g, t, crate::group_to_window_menu(Some(g), others)),
                None => Menu::new("").item("Tile All Tabs", "tile-tabs").checked(t.tiled),
            };
            let menu = menu.item("Reset Pane Sizes", "tile-reset-heights").enabled(changed);
            let menu = if group.is_some() { self.worktrees_menu(id, menu) } else { menu };
            return self.popup(id, menu, at, w);
        }
        if button != MouseButton::Left {
            return;
        }
        let drag = match divider {
            Divider::Row(r) => {
                // A click picks the pane there.
                if let Some(&(i, _, _)) = layout.panes.iter().find(|(_, p, _)| p.contains(&at))
                    && i != t.active
                {
                    t.select(i);
                }
                let keys: Vec<String> = layout.rows.iter().map(|row| row.key.clone()).collect();
                let weights = keys.iter().map(|k| t.tile_heights.get(k).copied().unwrap_or(1.0)).collect();
                let spans = layout.rows.iter().map(|row| row.span).collect();
                DividerDrag { across: Across::Rows(keys), at: r, from: f32::from(at.y), spans, weights, least: least.1 }
            }
            Divider::Pane(r, c) => {
                let Some(row) = layout.rows.get(r) else { return };
                let weights = t.tile_widths.get(&row.key).filter(|w| w.len() == row.cols.len()).cloned().unwrap_or_else(|| vec![1.0; row.cols.len()]);
                DividerDrag { across: Across::Panes(row.key.clone()), at: c, from: f32::from(at.x), spans: row.cols.clone(), weights, least: least.0 }
            }
        };
        // Followed till the button goes up (`divider_drag`).
        t.dragging = Some(drag);
        Cx.redraw(id);
    }

    /// The divider being dragged in window `id` with the pointer at
    /// `along` (its y between rows, its x between panes); None: let go.
    fn divider_drag(&mut self, id: WindowId, along: Option<Pixels>) {
        let Some(Win::Term(w)) = self.windows.get_mut(&id) else { return };
        let Some(d) = &w.dragging else { return };
        let Some(along) = along else {
            w.dragging = None;
            Cx.redraw(id);
            return;
        };
        let new = drag_rows(&d.weights, &d.spans, d.at, f32::from(along) - d.from, d.least);
        let changed = match &d.across {
            Across::Rows(keys) => {
                let changed = keys.iter().zip(&new).any(|(k, v)| w.tile_heights.get(k) != Some(v));
                for (key, v) in keys.clone().into_iter().zip(new) {
                    w.tile_heights.insert(key, v);
                }
                changed
            }
            Across::Panes(key) => w.tile_widths.insert(key.clone(), new.clone()).as_ref() != Some(&new),
        };
        if changed {
            Cx.redraw(id);
        }
    }

    /// What was done to window `id`'s tab strip.
    fn strip_action(&mut self, id: WindowId, action: StripAction, w: &mut WindowCx) {
        let side = self.settings.side_tabs;
        // (A tab or a group dragged along the strip: a step to undo.)
        let moved = match &action {
            StripAction::Move { .. } => Some(("Move Tab", self.tab_order(id))),
            StripAction::MoveGroup { .. } => Some(("Move Group", self.tab_order(id))),
            _ => None,
        };
        self.strip_did(id, action, side, w);
        if let Some((name, before)) = moved {
            self.tabs_moved(id, before, name);
        }
    }

    fn strip_did(&mut self, id: WindowId, action: StripAction, side: bool, w: &mut WindowCx) {
        let cx = &mut Cx;
        // (For a group's menu: the windows its tabs could go to.)
        let others = if matches!(action, StripAction::Group(..)) { self.other_windows(id) } else { Vec::new() };
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        match action {
            StripAction::Select(i) => {
                t.select(i);
                cx.redraw(id);
            }
            StripAction::Close(i) => {
                let shell = t.tabs[i].session.id;
                self.close_tab_requested(cx, id, shell, "the tab's close box or a middle-click");
            }
            StripAction::New => self.command(cx, id, "new-tab"),
            StripAction::Move { from, to, group } => {
                let shown = t.tab().session.id;
                let mut tab = t.tabs.remove(from);
                if tab.group != group {
                    // Dragged out of its group, or into one, by the person.
                    tab.own_grouping = true;
                }
                tab.group = group;
                t.tabs.insert(to.min(t.tabs.len()), tab);
                t.active = t.position(shown).unwrap_or(0);
                t.tidy();
                cx.redraw(id);
            }
            StripAction::MoveGroup { group, to } => {
                let shown = t.tab().session.id;
                let (moving, mut rest): (Vec<crate::Tab>, Vec<crate::Tab>) = std::mem::take(&mut t.tabs).into_iter().partition(|x| x.group == Some(group));
                let at = to.min(rest.len());
                rest.splice(at..at, moving);
                t.tabs = rest;
                t.active = t.position(shown).unwrap_or(0);
                t.tidy();
                cx.redraw(id);
            }
            StripAction::ToggleGroup(g) => {
                t.toggle_tile_group(g);
                cx.redraw(id);
            }
            StripAction::Group(g, pos) => {
                let tiling_rows = t.tiled && t.tabs.len() > 1 && t.tile_rows().len() > 1;
                let mut menu = crate::group_menu(g, t, crate::group_to_window_menu(Some(g), others));
                if tiling_rows {
                    menu = menu.item("Reset Pane Sizes", "tile-reset-heights").enabled(t.sizes_changed());
                }
                let menu = self.worktrees_menu(id, menu);
                self.popup(id, menu, pos, w);
            }
            // Pulled out of the strip: a window under the pointer, moving
            // with it till let go (onto another window's strip: in there).
            StripAction::TearOff { tab, .. } => {
                let shell = t.tabs[tab].session.id;
                if let Some(new) = self.detach(cx, id, shell, true) {
                    cx.begin_move(new);
                }
            }
            StripAction::TearOffGroup { group, .. } => {
                if let Some(new) = self.group_to_window(cx, id, group, true) {
                    cx.begin_move(new);
                }
            }
            StripAction::Resize { width, done } => {
                // Every window's: it's a setting.
                self.settings.side_width = width;
                if done {
                    self.settings.save();
                }
                cx.redraw_all();
            }
            StripAction::Menu(i, pos) => {
                let shell = t.tabs[i].session.id;
                let (place, label) = if side { ("tabs-top", "Tabs Along the Top") } else { ("tabs-side", "Tabs Down the Side") };
                let menu = Menu::new("").item("New Tab", "new-tab").separator().item(olo_kit_base::tracker::LABEL, format!("{}:{shell}", olo_kit_base::tracker::COMMAND));
                let Some(Win::Term(t)) = self.windows.get(&id) else { return };
                let menu = self
                    .tab_repo_menu(&t.tabs[i], menu)
                    .separator()
                    .submenu(self.add_to_group_menu(id, shell, true))
                    .item("Remove from Group", format!("tab-ungroup:{shell}"))
                    .enabled(t.tabs[i].group.is_some())
                    .item("Move to New Window", format!("tab-detach:{shell}"))
                    .enabled(t.tabs.len() > 1)
                    .submenu(self.move_to_window_menu(id, shell, true));
                // Or to an agent screen, its shell running on (the person's
                // own Terminal: one started on a screen keeps its shells).
                let menu = if olo_kit_base::moving::menus_offer() {
                    menu.submenu(Menu::agent_screens("Move to Agent Screen", &olo_kit_base::moving::screens(), &format!("tab-screen:{shell}:")))
                } else {
                    menu
                };
                let menu = menu
                    .separator()
                    .item("Close Other Tabs", format!("tab-close-others:{shell}"))
                    .item("Close Tab", format!("tab-close:{shell}"))
                    .shift_key("w")
                    .separator()
                    .item(if t.tabs[i].hidden_tiled { "Show When Tiled" } else { "Hide When Tiled" }, format!("tab-tile-hide:{shell}"))
                    .item(label, place);
                // A tab with a coding agent in it: told "please continue"
                // (or its own message) when it stops (Keep It Going).
                let tab = &t.tabs[i];
                let keep = tab.keep_going.is_some();
                let approve = tab.auto_approve.is_some();
                let menu = if tab.agent.is_some() || keep || approve {
                    menu.separator().submenu(self.keep_going_menu(tab)).item("Auto Approve", format!("tab-auto-approve:{shell}")).checked(approve)
                } else {
                    menu
                };
                self.popup(id, menu, pos, w);
            }
        }
    }

    /// A small window's keys: Return its default button (`done(true)`),
    /// Escape its Cancel.
    fn sheet(sh: &Shell, done: fn(&mut Terminal, &mut Cx, WindowId, bool)) -> Element {
        let id = sh.id;
        div().size_full().track_focus(&sh.focus).on_key_down(on(move |t, e: &KeyDownEvent, w| {
            if let Some(go) = alert_key(e) {
                w.stop_propagation();
                done(t, &mut Cx, id, go);
            }
        }))
    }

    /// The Find window for a terminal window.
    fn render_find(&mut self, sh: &mut Shell, cx: &mut ShellCx) -> Element {
        let id = sh.id;
        let Some(field) = &sh.field else { return div() };
        let s = &self.search;
        let ready = !field.read(cx).text().is_empty();
        let check = |label: &str, on_now: bool, f: fn(&mut crate::find::Search, bool)| {
            CheckBox::new(label.to_owned(), on_now).on_toggle(on(move |t, v: &bool, _| {
                f(&mut t.search, *v);
                Cx.redraw(id);
            }))
        };
        let (case, words, wrap, back) = (s.case, s.words, s.wrap, s.backwards);
        let col = div()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(TextControl::new(cx, field).label("Find:").divider(40.0))
            .child(div().h(px(m::SPACING - m::TIGHT)))
            .child(check("Case-sensitive", case, |s, v| s.case = v))
            .child(check("Whole words only", words, |s, v| s.words = v))
            .child(check("Wrap-around search", wrap, |s, v| s.wrap = v))
            .child(check("Search backwards", back, |s, v| s.backwards = v));
        let row = widgets::button_row()
            .child(Button::new("Cancel").on_click(on(move |t, _: &olo_kit::ClickEvent, _| t.find_done(&mut Cx, id, false))))
            .child(Button::new("Find").default(true).enabled(ready).on_click(on(move |t, _: &olo_kit::ClickEvent, _| t.find_done(&mut Cx, id, true))));
        Terminal::sheet(sh, Terminal::find_done).p(px(m::WINDOW_INSET)).flex().flex_col().child(col).child(div().flex_1()).child(row)
    }

    /// The Find window answered: search (and close it) or just close it.
    pub(crate) fn find_done(&mut self, cx: &mut Cx, id: WindowId, go: bool) {
        let Some(Win::Find { target }) = self.windows.get(&id) else { return };
        let target = *target;
        let text = cx.field_text(id);
        if go && text.is_empty() {
            return;
        }
        self.windows.remove(&id);
        cx.close_window(id);
        if go {
            self.search.text = text;
            self.find_in(cx, target);
            cx.activate(target);
        }
    }

    /// The Group window: a name and a colour for a group.
    fn render_name_group(&mut self, sh: &mut Shell, cx: &mut ShellCx) -> Element {
        let id = sh.id;
        let Some(Win::NameGroup { color, .. }) = self.windows.get(&id) else { return div() };
        let Some(name) = &sh.field else { return div() };
        let color = *color;
        let ready = !name.read(cx).text().trim().is_empty();
        let look = self.look.clone();
        // The colours, as swatches; the chosen one on the selection.
        let swatch = |b: Bounds<Pixels>, i: usize| Bounds { origin: point(b.left() + px(50.0 + i as f32 * (SWATCH + m::SPACING)), b.top()), size: size(px(SWATCH), px(SWATCH)) };
        let swatches = canvas(move |g, b| {
            let p = theme::palette();
            style::label(g, b.left(), b.center().y, style::Align::Left, "Color:", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
            for i in 0..groups::COLORS.len() {
                let r = swatch(b, i);
                if color == i {
                    let ring = Bounds::centered_at(r.center(), size(px(SWATCH + 2.0), px(SWATCH + 2.0)));
                    let (ground, edge) = (style::c(style::lit_ground(&p, p.panel_background)), style::c(p.selection_edge()));
                    g.paint_quad(quad(ring, px(SWATCH / 2.0 + 1.0), ground, px(1.0), edge, olo_kit::BorderStyle::Solid));
                }
                let dot = Bounds::centered_at(r.center(), size(px(SWATCH - 6.0), px(SWATCH - 6.0)));
                g.paint_quad(fill(dot, groups::color(i, &look)).corner_radii(px(SWATCH / 2.0 - 3.0)));
                g.agent(olo_kit::agent::Node {
                    role: "RadioButton",
                    name: Some(groups::COLORS[i].name().into()),
                    bounds: r,
                    enabled: true,
                    clickable: true,
                    selected: Some(color == i),
                    ..Default::default()
                });
            }
        })
        .paint_key(style::key(color))
        .h(px(SWATCH))
        .w_full()
        .on_mouse_down(
            MouseButton::Left,
            on(move |t, e: &MouseDownEvent, w| {
                let b = w.listener_bounds();
                let Some(i) = (0..groups::COLORS.len()).find(|&i| swatch(b, i).contains(&e.position)) else { return };
                if let Some(Win::NameGroup { color, .. }) = t.windows.get_mut(&id) {
                    *color = i;
                }
                Cx.redraw(id);
            }),
        );
        let row = widgets::button_row()
            .child(Button::new("Cancel").on_click(on(move |t, _: &olo_kit::ClickEvent, _| t.name_group_done(&mut Cx, id, false))))
            .child(Button::new("OK").default(true).enabled(ready).on_click(on(move |t, _: &olo_kit::ClickEvent, _| t.name_group_done(&mut Cx, id, true))));
        Terminal::sheet(sh, Terminal::name_group_done)
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .child(TextControl::new(cx, name).label("Name:").divider(50.0))
            .child(div().h(px(2.0 * m::SPACING)))
            .child(swatches)
            .child(div().flex_1())
            .child(row)
    }

    /// Keep It Going ▸ Custom Message…: what Keep It Going says to one
    /// tab's agent, Preferences' for every tab under it.
    fn render_keep_going_message(&mut self, sh: &mut Shell, cx: &mut ShellCx) -> Element {
        let id = sh.id;
        let Some(Win::KeepGoingMessage { about, .. }) = self.windows.get(&id) else { return div() };
        let Some(field) = &sh.field else { return div() };
        let about = about.clone();
        let p = theme::palette();
        let default = crate::keep_going_note(&self.keep_going_says);
        let row = widgets::button_row()
            .child(Button::new("Cancel").on_click(on(move |t, _: &olo_kit::ClickEvent, _| t.keep_going_message_done(&mut Cx, id, false))))
            .child(Button::new("Keep It Going").default(true).on_click(on(move |t, _: &olo_kit::ClickEvent, _| t.keep_going_message_done(&mut Cx, id, true))));
        Terminal::sheet(sh, Terminal::keep_going_message_done).child(
            widgets::AlertFrame::new("terminal")
                .child(widgets::alert::alert_title("Keep It Going"))
                .child(div().h(px(m::TIGHT)).flex_shrink_0())
                .child(widgets::paragraph(about))
                .child(div().h(px(m::SPACING)).flex_shrink_0())
                .child(TextControl::new(cx, field))
                .child(div().h(px(m::TIGHT)).flex_shrink_0())
                .child(widgets::paragraph(default).text_color(style::disabled_text(&p)))
                .child(div().flex_1().min_h(px(m::SPACING)))
                .child(row),
        )
    }
}

/// A group colour's swatch.
pub const SWATCH: f32 = 22.0;

#[cfg(test)]
mod tile_tests {
    use olo_kit::{Bounds, point, px};

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Bounds<olo_kit::Pixels> {
        Bounds::from_corners(point(px(x0), px(y0)), point(px(x1), px(y1)))
    }

    #[test]
    fn a_row_for_each_group() {
        // Two rows (three tabs, then two) in 300 × 201 at 1×: rows a line
        // apart sharing the height, each row's panes its width.
        let area = rect(0.0, 0.0, 300.0, 201.0);
        let even = |n: usize| vec![1.0; n];
        let rows = super::tile_rows(area, &[even(3), even(2)], &[1.0, 1.0], (0.0, 0.0), 1.0);
        let panes: Vec<_> = rows.concat();
        assert_eq!(panes.len(), 5);
        assert!(panes[..3].iter().all(|p| p.top() == px(0.0) && p.bottom() == panes[0].bottom()));
        assert!(panes[3..].iter().all(|p| p.bottom() == px(201.0) && p.top() > panes[0].bottom()));
        assert_eq!((panes[0].left(), panes[2].right(), panes[3].left(), panes[4].right()), (px(0.0), px(300.0), px(0.0), px(300.0)));
        // Panes side by side don't overlap, a line between them.
        assert!(panes[0].right() < panes[1].left() && panes[3].right() < panes[4].left());
        // Every edge on a device pixel at 1.5×, the widths dragged too.
        for p in super::tile_rows(area, &[vec![1.0, 2.0, 0.5], even(2), even(1)], &[1.0, 2.5, 0.7], (40.0, 30.0), 1.5).concat() {
            for v in [p.left(), p.top(), p.right(), p.bottom()].map(f32::from) {
                assert!((v * 1.5 - (v * 1.5).round()).abs() < 1e-3, "{v}");
            }
        }
        // A pane's width dragged: the line between the first two 40 to
        // the right, the third as it was.
        let cols = super::spans_between(0.0, 300.0, &even(3), 40.0, 1.0);
        let w = super::drag_rows(&even(3), &cols, 1, 40.0, 40.0);
        let after = super::spans_between(0.0, 300.0, &w, 40.0, 1.0);
        assert!((after[0].1 - cols[0].1 - 40.0).abs() <= 1.0, "{after:?}");
        assert_eq!(after[2], cols[2]);
    }

    #[test]
    fn a_grid_for_tabs_in_no_group() {
        // Four in a wide window: two by two, not four in a row.
        assert_eq!(super::grid_columns(rect(0.0, 0.0, 1200.0, 700.0), 4), 2);
        assert_eq!(super::grid_columns(rect(0.0, 0.0, 1200.0, 700.0), 1), 1);
    }

    #[test]
    fn rows_dragged() {
        let area = rect(0.0, 0.0, 300.0, 401.0);
        let h = |s: &[(f32, f32)], i: usize| s[i].1 - s[i].0;
        // Even at first: a line between rows.
        let even = super::row_spans(area, &[1.0, 1.0], 40.0, 1.0);
        assert_eq!(even, vec![(0.0, 200.0), (201.0, 401.0)]);
        // The divider dragged down 50: the first row 50 taller.
        let w = super::drag_rows(&[1.0, 1.0], &even, 1, 50.0, 40.0);
        let spans = super::row_spans(area, &w, 40.0, 1.0);
        assert!((h(&spans, 0) - 250.0).abs() <= 1.0, "{spans:?}");
        assert_eq!(spans[1].1, 401.0);
        // Dragged right down: the second keeps its least.
        let w = super::drag_rows(&w, &spans, 1, 1000.0, 40.0);
        let spans = super::row_spans(area, &w, 40.0, 1.0);
        assert!((h(&spans, 1) - 40.0).abs() <= 1.0, "{spans:?}");
        // Three rows: dragging between the last two leaves the first.
        let three = super::row_spans(area, &[1.0, 1.0, 1.0], 40.0, 1.0);
        let w = super::drag_rows(&[1.0, 1.0, 1.0], &three, 2, -30.0, 40.0);
        let after = super::row_spans(area, &w, 40.0, 1.0);
        assert_eq!(after[0], three[0]);
        assert!(h(&after, 1) < h(&three, 1) && h(&after, 2) > h(&three, 2));
        // Too small for every row's least: shared by weight alone.
        let tiny = super::row_spans(rect(0.0, 0.0, 300.0, 60.0), &[1.0, 1.0], 40.0, 1.0);
        assert_eq!(tiny.len(), 2);
    }
}
