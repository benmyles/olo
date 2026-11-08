//! Running an app and opening its windows.

use std::any::Any;

use futures::{StreamExt, channel::mpsc};
use gpui::{
    AnyView, App, AppContext as _, Application, Bounds, Context, Entity, FocusHandle, Global, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    Styled as _, TitlebarOptions, Window, WindowBounds, WindowHandle, canvas, div, px, size,
};
use olo_kit_base::Proxy;

use crate::{agent, style, theme};

/// How a new window should appear.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: SharedString,
    /// The content's logical size (decorations are app_server's).
    pub size: (f32, f32),
    pub min_size: Option<(f32, f32)>,
    /// Its largest (0: free along that side); no larger than its smallest
    /// along a side, it's fixed along it, and app_server neither tiles
    /// nor groups it (a Find panel's height).
    pub max_size: Option<(f32, f32)>,
    /// Paint the panel colour behind the content (most windows want it).
    pub panel: bool,
    /// A utility window (a small tool such as DeskCalc) that floats over
    /// auto-tiled windows rather than taking a tile (the xdg-toplevel-tag
    /// `floating`).
    pub floating: bool,
    /// A surface of the desktop's own (Deskbar, notifications, an
    /// overlay) rather than a window: where it sits.
    pub layer: Option<gpui::LayerShellOptions>,
    /// A dialog (never a tab), modal or not.
    pub dialog: Option<gpui::Dialog>,
    /// Never resized (an alert).
    pub fixed: bool,
    /// Covers this output while the session is locked (`lock_session`).
    pub lock_output: Option<String>,
    /// Belongs to the app's window that has the keyboard (a chat's Info
    /// over its chat): opens over it, never a tab beside it.
    pub child: bool,
    /// Belongs to another program's window, by the handle it exported
    /// (xdg-foreign): a portal's dialog over the app that asked.
    pub foreign_parent: Option<String>,
    /// Opens with its tab under the pointer (the xdg-toplevel-tag
    /// `under-pointer`): a tab torn off a strip, to move with it
    /// ([`begin_move`]).
    pub under_pointer: bool,
    /// What app_server knows this window by when the app comes back (a
    /// restart, an upgrade): its place, workspace, tab's place in its
    /// group and tile are this window's, not another's of the same title
    /// (the xdg-toplevel-tag word `restore:KEY`; [`Self::restore_key`]).
    pub restore_key: Option<String>,
    /// Opens full screen, on the display the person is at (asked before
    /// its first frame, so it's never drawn at `size` first): something
    /// that takes the whole of their attention, done with the keyboard
    /// (the palm trainer). `size` is what it's laid out at until then.
    pub fullscreen: bool,
    /// Opens at the size the window of this name was last left at, `size`
    /// the first time (`kept`): an app's main window, a document's.
    pub keep_size: Option<SharedString>,
    /// What it shows needs this height: in a window less tall (on a short
    /// screen) it's laid out at it all the same and scrolls (the window's
    /// `Frame` does it; a panel needn't).
    pub content_height: Option<f32>,
    /// As tall as what it shows needs (`size`'s height at least), never
    /// taller than the screen has room for, where it scrolls: see
    /// [`Self::fits_content`].
    pub fit_content: bool,
}

impl WindowOptions {
    pub fn new(title: impl Into<SharedString>, width: f32, height: f32) -> Self {
        WindowOptions {
            title: title.into(),
            size: (width, height),
            min_size: None,
            max_size: None,
            panel: true,
            floating: false,
            layer: None,
            dialog: None,
            fixed: false,
            lock_output: None,
            child: false,
            foreign_parent: None,
            under_pointer: false,
            restore_key: None,
            fullscreen: false,
            keep_size: None,
            content_height: None,
            fit_content: false,
        }
    }

    /// The window follows what it shows (HACKING, "Windows fit what they
    /// show"): a `fixed` one as tall as its content needs, `size`'s height
    /// at least, growing and shrinking with it (a tab with more rows, a
    /// display's brightness, a longer message); one the person sizes is
    /// never shorter than its content needs (grown when it needs more).
    /// Never taller than the shortest screen less room for its tab
    /// ([`screen_room`]): past that, its Frame scrolls what it shows, the
    /// window widened by the scroll bar. Every alert and dialog has it;
    /// give it to every other window whose content sets its size (forms,
    /// panels); a document's window is the person's to size. The content
    /// is laid out at its own height: a list in it shrinks (in a
    /// `flex_1().min_h_0()` box) and scrolls, never the whole of it.
    pub fn fits_content(mut self) -> Self {
        self.fit_content = true;
        self
    }

    /// A BAlert window `width` × `height`: fixed, modal over the app's
    /// window that has the keyboard (if any), never tiled or a tab.
    /// Follows what it shows (`fits_content`): `height` is its least.
    pub fn alert(title: impl Into<SharedString>, width: f32, height: f32) -> Self {
        WindowOptions { floating: true, dialog: Some(gpui::Dialog::Modal), fixed: true, fit_content: true, ..WindowOptions::new(title, width, height) }
    }

    /// A dialog (a panel, a sheet): floats, never a tab. Never smaller
    /// than what it shows needs, nor taller than the screen
    /// (`fits_content`).
    pub fn dialog(mut self) -> Self {
        self.floating = true;
        self.dialog = Some(gpui::Dialog::Plain);
        self.fit_content = true;
        self
    }

    /// A layer surface (see [`Self::layer`]), `namespace` saying what it
    /// is to the compositor, anchored to the edges given (top, bottom,
    /// left, right), on `layer`; refine it with [`Self::layer_options`].
    pub fn layer(mut self, namespace: &str, layer: gpui::LayerShellLayer, anchor: (bool, bool, bool, bool)) -> Self {
        self.layer = Some(gpui::LayerShellOptions { layer, anchor, exclusive_zone: 0, margin: (0, 0, 0, 0), keyboard: gpui::LayerKeyboard::None, namespace: namespace.to_owned(), output: None });
        self
    }

    /// Changes the layer surface's options (exclusive zone, margins,
    /// keyboard, output).
    pub fn layer_options(mut self, f: impl FnOnce(&mut gpui::LayerShellOptions)) -> Self {
        if let Some(l) = self.layer.as_mut() {
            f(l);
        }
        self
    }

    /// Without the panel colour behind it (an overlay draws all of it).
    pub fn no_panel(mut self) -> Self {
        self.panel = false;
        self
    }

    /// Known by `key` when the app comes back (see [`Self::restore_key`]
    /// the field): the same window's key each time, unique among the
    /// app's windows (a document's path, a number the app keeps with the
    /// window). Spaces are taken out.
    pub fn restore_key(mut self, key: impl Into<String>) -> Self {
        self.restore_key = Some(key.into());
        self
    }

    /// Floats over auto-tiled windows (see [`Self::floating`]).
    pub fn floating(mut self) -> Self {
        self.floating = true;
        self
    }

    /// Opens over the app's window that has the keyboard, as its own
    /// (see [`Self::child`]).
    pub fn child(mut self) -> Self {
        self.child = true;
        self
    }

    /// Opens full screen (see [`Self::fullscreen`]).
    pub fn fullscreen(mut self) -> Self {
        self.fullscreen = true;
        self
    }

    pub fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = Some((width, height));
        self
    }

    /// What it shows needs `height`: shorter, the window scrolls it.
    pub fn content_height(mut self, height: f32) -> Self {
        self.content_height = Some(height);
        self
    }

    /// Opens at the size the app's window called `name` was last left at
    /// (`size` until there is one), kept as the person changes it.
    pub fn keep_size(mut self, name: impl Into<SharedString>) -> Self {
        self.keep_size = Some(name.into());
        self
    }

    pub fn max_size(mut self, width: f32, height: f32) -> Self {
        self.max_size = Some((width, height));
        self
    }

    /// Opens over another program's window (see [`Self::foreign_parent`]),
    /// given as a portal gives it (`wayland:HANDLE`, or the handle
    /// alone); nothing if empty.
    pub fn foreign_parent(mut self, handle: impl Into<String>) -> Self {
        let handle = handle.into();
        self.foreign_parent = Some(handle.strip_prefix("wayland:").map(str::to_owned).unwrap_or(handle)).filter(|h| !h.is_empty());
        self
    }
}

/// A window's root: the panel behind the app's view, Olo's font and the
/// palette's text colour for what it draws, and the start of each frame
/// for agents ([`agent`]).
pub struct Frame {
    title: SharedString,
    content: AnyView,
    panel: bool,
    /// Has the keyboard when nothing in the window does, so the menu
    /// bar's keys reach it.
    focus: FocusHandle,
    /// Its size noted as it changes (`WindowOptions::keep_size`).
    _kept: Option<gpui::Subscription>,
    /// The height what it shows needs, and where it's scrolled to in a
    /// window less tall (`WindowOptions::content_height`).
    content_height: Option<f32>,
    scroll: gpui::ScrollHandle,
    /// The window follows what it shows (`WindowOptions::fits_content`).
    fit: Option<Fit>,
}

/// A window that follows what it shows: its size as designed, what its
/// content needed as last laid out, and the size last asked for.
struct Fit {
    design: (f32, f32),
    /// Its size is its content's, not the person's.
    fixed: bool,
    need: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    /// The size last asked for (so each is asked once).
    asked: std::rc::Rc<std::cell::Cell<Option<(f32, f32)>>>,
    /// A resize asked for and not made yet: the size it was then, and
    /// when (no frame is shown until it's another, or `RESIZE_WAIT`).
    resizing: std::rc::Rc<std::cell::Cell<Option<(gpui::Size<gpui::Pixels>, std::time::Instant)>>>,
    /// What it waits for before it's first shown ([`hold_first_show`]).
    first: std::sync::Arc<FirstShow>,
    _displays: gpui::Subscription,
}

/// The longest a window that fits what it shows holds its frames back for
/// a resize it asked for: a compositor that keeps it at its size (a tile,
/// Focus's zoom) never makes it, and the window was never seen
/// (DiskBackup in Focus, Ben 2026-11-08: "an entry in the deskbar but no
/// windows").
pub const RESIZE_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

/// Whether the compositor holds the window's size (a side of it in a tile,
/// maximized, full screen): it isn't the window's to set, so one that
/// fits what it shows lays out at the size it's given, scrolled if its
/// content needs more.
fn held(window: &Window) -> bool {
    let t = window.tiling();
    window.is_maximized() || window.is_fullscreen() || t.top || t.bottom || t.left || t.right
}

/// How long a window that fits what it shows waits, at most, for what it
/// asked for as it opened before it's first shown ([`hold_first_show`]):
/// an answer slower than this shows it as it is, and it grows when the
/// answer comes.
pub const FIRST_SHOW_WAIT: std::time::Duration = std::time::Duration::from_millis(150);

/// A window's first showing, held ([`hold_first_show`]).
pub struct FirstShow {
    opened: std::time::Instant,
    /// The holds not let go yet.
    waiting: std::sync::atomic::AtomicUsize,
    /// It was shown (or stopped waiting): no more holds.
    over: std::sync::atomic::AtomicBool,
    /// Told as each hold is let go (from any thread).
    let_go: futures::channel::mpsc::UnboundedSender<()>,
    heard: std::sync::Mutex<Option<futures::channel::mpsc::UnboundedReceiver<()>>>,
}

impl FirstShow {
    fn new() -> std::sync::Arc<Self> {
        let (let_go, heard) = futures::channel::mpsc::unbounded();
        std::sync::Arc::new(FirstShow { opened: std::time::Instant::now(), waiting: Default::default(), over: Default::default(), let_go, heard: std::sync::Mutex::new(Some(heard)) })
    }

    /// Whether the frame being drawn is held back (something asked for
    /// is still to come, and there's time to wait for it).
    fn holding(&self) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        if self.over.load(SeqCst) {
            return false;
        }
        if self.waiting.load(SeqCst) > 0 && self.opened.elapsed() < FIRST_SHOW_WAIT {
            return true;
        }
        self.over.store(true, SeqCst);
        false
    }
}

/// A hold on a window's first showing ([`hold_first_show`]): let go by
/// dropping it, on any thread.
pub struct FirstShowHold(std::sync::Arc<FirstShow>);

impl Drop for FirstShowHold {
    fn drop(&mut self) {
        self.0.waiting.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        let _ = self.0.let_go.unbounded_send(());
    }
}

thread_local! {
    /// The windows not shown yet that fit what they show: their holds.
    static FIRST_SHOWS: std::cell::RefCell<std::collections::HashMap<gpui::WindowId, std::sync::Arc<FirstShow>>> = Default::default();
}

/// Hold a window that fits what it shows (`WindowOptions::fits_content`)
/// from being first shown until what it asked for as it opened has come
/// (a worker's answer that adds to what it shows: drop the hold when it's
/// in), for [`FIRST_SHOW_WAIT`] at most: so it opens at the size of what
/// it shows, not at a size it grows from a moment later. Nothing waits
/// for it: the window's frames before are drawn and not shown. None once
/// it has been shown, and for other windows.
pub fn hold_first_show(window: &Window) -> Option<FirstShowHold> {
    let first = FIRST_SHOWS.with(|f| f.borrow().get(&window.window_handle().window_id()).cloned())?;
    if first.over.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    first.waiting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Some(FirstShowHold(first))
}

/// Draw a held window again as its holds are let go, until none is left
/// or its wait is over.
fn wait_for_holds(first: std::sync::Arc<FirstShow>, window: &mut Window, cx: &mut App) {
    // (One waits at a time; it hands the listening back as it ends, for
    // a hold taken after.)
    let Some(mut heard) = first.heard.lock().ok().and_then(|mut h| h.take()) else { return };
    window
        .spawn(cx, async move |cx| {
            use futures::{FutureExt, StreamExt};
            let left = FIRST_SHOW_WAIT.saturating_sub(first.opened.elapsed());
            let mut timer = cx.background_executor().timer(left).fuse();
            loop {
                futures::select! {
                    _ = heard.next() => {
                        if first.waiting.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                            break;
                        }
                    }
                    _ = timer => break,
                }
            }
            if let Ok(mut h) = first.heard.lock() {
                *h = Some(heard);
            }
            let _ = cx.update(|window, _| window.refresh());
        })
        .detach();
}

/// The size a window that follows what it shows should be (`viewport` now,
/// its content needing `need`), and whether its content scrolls there.
fn fitted(design: (f32, f32), fixed: bool, viewport: gpui::Size<gpui::Pixels>, need: f32, room: f32) -> ((f32, f32), bool) {
    // (One the person sizes keeps their size when it's enough.)
    let least = if fixed { design.1 } else { f32::from(viewport.height) };
    let need = need.ceil().max(least);
    let scrolls = need > room + 0.5;
    let width = if fixed { design.0 } else { f32::from(viewport.width) };
    let size = if scrolls { (width + if fixed { crate::widgets::SCROLL_BAR } else { 0.0 }, room.floor()) } else { (width, need) };
    (size, scrolls)
}

/// Room left above a window's tab and around it on a screen.
const SCREEN_EDGES: f32 = 64.0;

/// The tallest a window may be: the shortest display's height in points,
/// less room for the window's tab and edges (never under 240).
pub fn screen_room(cx: &App) -> f32 {
    cx.displays().iter().map(|d| f32::from(d.bounds().size.height)).fold(f32::INFINITY, f32::min).max(240.0 + SCREEN_EDGES) - SCREEN_EDGES
}

impl Frame {
    /// A window that follows what it shows, whose content as last
    /// measured needs more than the screen has room for: the height to
    /// lay it out at, scrolled. (Its size is set as it's measured, as
    /// each frame is painted: the measure in `render`.)
    fn follow(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<f32> {
        let fit = self.fit.as_ref()?;
        let need = fit.need.get()?;
        let now = window.viewport_size();
        if held(window) {
            return (need > f32::from(now.height) + 0.5).then(|| need.ceil());
        }
        let (_, scrolls) = fitted(fit.design, fit.fixed, now, need, screen_room(cx));
        // (One the person sizes keeps their size when it's enough.)
        let least = if fit.fixed { fit.design.1 } else { f32::from(now.height) };
        scrolls.then(|| need.ceil().max(least))
    }
}

impl Frame {
    pub fn set_title(&mut self, title: impl Into<SharedString>, window: &mut Window) {
        self.title = title.into();
        window.set_window_title(&self.title);
    }
}

gpui::actions!(olo_frame, [FocusNext, FocusPrev]);

type DefaultClick = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// Each window's default button, as last drawn.
#[derive(Default)]
struct DefaultButtons(std::collections::HashMap<gpui::WindowId, DefaultClick>);

impl Global for DefaultButtons {}

/// The window's default button is drawn: Return clicks it.
pub(crate) fn set_default_button(window: &Window, cx: &mut App, click: DefaultClick) {
    cx.default_global::<DefaultButtons>().0.insert(window.window_handle().window_id(), click);
}

/// The window's default button, if one is drawn (and can be clicked).
pub(crate) fn default_button(window: &Window, cx: &App) -> Option<DefaultClick> {
    cx.try_global::<DefaultButtons>().and_then(|d| d.0.get(&window.window_handle().window_id())).cloned()
}

type TabStep = std::rc::Rc<dyn Fn(i32, &mut Window, &mut App)>;

/// Each window's tab keys' taker (its first tab view), as last drawn.
#[derive(Default)]
struct TabKeys(std::collections::HashMap<gpui::WindowId, TabStep>);

impl Global for TabKeys {}

/// A tab view is drawn: Next and Previous Tab step it, unless one drawn
/// before it in the window has them.
pub(crate) fn set_tab_keys(window: &Window, cx: &mut App, step: TabStep) {
    cx.default_global::<TabKeys>().0.entry(window.window_handle().window_id()).or_insert(step);
}

/// Next Tab (+1: ⌃Tab, ⌃Page Down) or Previous Tab (−1: ⌃⇧Tab, ⌃Page Up);
/// 0 for any other key.
pub fn tab_step(k: &gpui::Keystroke) -> i32 {
    let m = &k.modifiers;
    if !m.control || m.alt || m.platform {
        return 0;
    }
    match k.key.as_str() {
        "tab" if m.shift => -1,
        "tab" => 1,
        "pageup" if !m.shift => -1,
        "pagedown" if !m.shift => 1,
        _ => 0,
    }
}

/// Tab and ⇧Tab move the keyboard through the window's controls, fields
/// and lists, as BeOS's keyboard navigation does (a view of its own that
/// takes Tab, a terminal or an editor, binds it in its key context).
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([gpui::KeyBinding::new("tab", FocusNext, None), gpui::KeyBinding::new("shift-tab", FocusPrev, None)]);
}

impl Render for Frame {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        agent::begin_frame(window, cx, &self.title);
        // Drawn again below if there still is one.
        cx.default_global::<DefaultButtons>().0.remove(&window.window_handle().window_id());
        cx.default_global::<TabKeys>().0.remove(&window.window_handle().window_id());
        let p = theme::palette(cx);
        let mut root = div()
            .size_full()
            .track_focus(&self.focus)
            // A press anywhere in the window puts away a menu open over it
            // (and a cut text's tip).
            .capture_any_mouse_down(|e, window, cx| {
                crate::cut::left(window, cx, true);
                crate::menu::press_in(e.position, window, cx)
            })
            // Text cut short: its whole in a tip (`cut`), at once with ⌘.
            .on_mouse_move(|e: &gpui::MouseMoveEvent, window, cx| crate::cut::moved(window, cx, e.position, crate::cut::command_held(&e.modifiers)))
            .on_modifiers_changed(|e: &gpui::ModifiersChangedEvent, window, cx| crate::cut::modifiers(window, cx, window.mouse_position(), crate::cut::command_held(&e.modifiers)))
            .on_action(|_: &FocusNext, window, _| window.focus_next())
            .on_action(|_: &FocusPrev, window, _| window.focus_prev())
            // Escape ends a drag within the window (GPUI's: a tab along a
            // strip, rows in a list), nothing dropped, before anything
            // in it takes the key. (app_server ends drags between windows
            // and programs, and window moves, the same way.)
            .capture_key_down(|e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" && cx.stop_active_drag(window) {
                    cx.stop_propagation();
                }
            })
            // The menu bar's keys, when nothing in the window took them
            // (a text field's ⌘C is its own).
            .on_key_down(|e: &KeyDownEvent, window, cx| {
                if let Some((command, on_command)) = crate::menu::bar_shortcut(window, cx, &e.keystroke) {
                    on_command(&command, window, cx);
                    cx.stop_propagation();
                    return;
                }
                // ⌘W (the profile's Close) on a window whose app has no
                // Close of its own: closed as its tab's close button does,
                // the app's confirmations first. (Every window closes with
                // it: Preferences, DeskCalc and Logs didn't.)
                if crate::keys::pressed(&olo_kit_base::keys::Shortcut::command(crate::keys::evdev("w")), &e.keystroke) {
                    window.request_close();
                    cx.stop_propagation();
                    return;
                }
                // Undo and Redo, for a window without a menu bar that
                // takes them (`undo::keys`).
                if let Some(redo) = crate::undo::pressed(&e.keystroke)
                    && let Some(take) = cx.try_global::<crate::undo::UndoKeys>().and_then(|u| u.0.get(&window.window_handle().window_id())).cloned()
                {
                    take(redo, window, cx);
                    cx.stop_propagation();
                    return;
                }
                // Next and Previous Tab: the window's tab view.
                let step = tab_step(&e.keystroke);
                if step != 0
                    && let Some(f) = cx.try_global::<TabKeys>().and_then(|t| t.0.get(&window.window_handle().window_id())).cloned()
                {
                    f(step, window, cx);
                    cx.stop_propagation();
                    return;
                }
                // Return nothing took: the default button.
                if e.keystroke.key == "enter"
                    && !e.keystroke.modifiers.modified()
                    && let Some(click) = cx.try_global::<DefaultButtons>().and_then(|d| d.0.get(&window.window_handle().window_id())).cloned()
                {
                    click(window, cx);
                    cx.stop_propagation();
                }
            })
            .font_family(style::UI_FONT)
            .text_size(px(style::PLAIN_SIZE))
            .line_height(gpui::relative(style::LINE_HEIGHT))
            .text_color(style::c(p.panel_text));
        if self.panel {
            root = root.bg(style::c(p.panel_background));
        }
        let title = self.title.clone();
        // Over all of it: a control lit to say "here" (`spotlight`).
        let lit = canvas(
            |_, _, _| {},
            |_, _, window, cx| {
                crate::spotlight::paint(window, cx);
                crate::cut::painted(window, cx);
            },
        )
        .absolute()
        .size_full();
        // Last: the frame is drawn (for a review asked for).
        let done = canvas(|_, _, _| {}, move |_, _, window, cx| crate::review::finish(window, &title, theme::palette(cx).mode != olo_theme::ThemeMode::Light)).absolute().size_0();
        // Following what it shows: laid out at what it needs (its size
        // as designed at least), measured, and scrolled past the screen's
        // room.
        if self.fit.is_some() {
            let scrolls = self.follow(window, cx);
            let fit = self.fit.as_ref().expect("fitting");
            // Measured as the frame is drawn: a window to be another size
            // isn't shown this frame (`skip_present`), and is resized and
            // drawn again at once, so it's never seen at the wrong size (it
            // opened at its designed size, then grew: Ben, 2026-11-04).
            // (Not shown at all while what it asked for as it opened is
            // still to come: `hold_first_show`.)
            let (need, asked, resizing, design, fixed, first) = (fit.need.clone(), fit.asked.clone(), fit.resizing.clone(), fit.design, fit.fixed, fit.first.clone());
            let measure = canvas(
                move |b, _, _| need.set(Some(f32::from(b.size.height))),
                move |b, _, window, cx| {
                    if !first.over.load(std::sync::atomic::Ordering::SeqCst) {
                        if first.holding() {
                            window.skip_present();
                            wait_for_holds(first.clone(), window, cx);
                            return;
                        }
                        // (Shown from now: no more holds.)
                        FIRST_SHOWS.with(|f| f.borrow_mut().remove(&window.window_handle().window_id()));
                    }
                    // Its size the compositor's (a tile, a zoom): as given,
                    // asking for nothing (and asking again once it isn't).
                    if held(window) {
                        resizing.set(None);
                        asked.set(None);
                        return;
                    }
                    let ((w, h), _) = fitted(design, fixed, window.viewport_size(), f32::from(b.size.height), screen_room(cx));
                    let now = window.viewport_size();
                    let off = (f32::from(now.width) - w).abs() > 0.5 || (f32::from(now.height) - h).abs() > 0.5;
                    // (The resize asked for is made when its size is
                    // another: that frame is drawn at it, or at what the
                    // compositor gave instead; or it never comes, and the
                    // window is shown as it is after `RESIZE_WAIT`.)
                    if resizing.get().is_some_and(|(then, at)| then != now || at.elapsed() > RESIZE_WAIT) {
                        resizing.set(None);
                    }
                    if off && asked.get() != Some((w, h)) {
                        asked.set(Some((w, h)));
                        resizing.set(Some((now, std::time::Instant::now())));
                        window.defer(cx, move |window, _| window.resize(gpui::size(px(w), px(h))));
                        // (Drawn again once the wait is over, made or not:
                        // a compositor that keeps its size sends no frame.)
                        window
                            .spawn(cx, async move |cx| {
                                cx.background_executor().timer(RESIZE_WAIT + std::time::Duration::from_millis(16)).await;
                                let _ = cx.update(|window, _| window.refresh());
                            })
                            .detach();
                    }
                    if resizing.get().is_some() {
                        window.skip_present();
                    }
                },
            )
            .absolute()
            .size_full();
            // (A flex item based at the designed height, its minimum what
            // its content needs: so as tall as the larger. One the person
            // or the app sizes: as the window, or what its content needs if
            // more.)
            let basis = if fit.fixed { fit.design.1 } else { 0.0 };
            let laid = div().relative().w_full().flex_basis(px(basis)).flex_shrink_0().flex().flex_col().child(self.content.clone()).child(measure);
            let laid = if fit.fixed { laid } else { laid.flex_grow() };
            let content = match scrolls {
                Some(_) => crate::widgets::ScrollView::new("frame-scroll", &self.scroll).border(false).child(div().w_full().flex().flex_col().child(laid)).into_any_element(),
                None => laid.into_any_element(),
            };
            return root.flex().flex_col().child(content).child(lit).child(done);
        }
        // Taller than the window (a short screen): laid out at its own
        // height all the same, and scrolled.
        let height = window.viewport_size().height;
        let content = match self.content_height {
            Some(h) if f32::from(height) + 0.5 < h => {
                let sized = div().w_full().h(px(h)).flex().flex_col().child(self.content.clone());
                crate::widgets::ScrollView::new("frame-scroll", &self.scroll).border(false).child(sized).into_any_element()
            }
            _ => self.content.clone().into_any_element(),
        };
        root.child(content).child(lit).child(done)
    }
}

/// What `run` sets up that a window needs, for a window drawn in GPUI's
/// test app (`crate::lint`): no app_server, no agents' socket.
#[cfg(feature = "lint")]
pub(crate) fn init_for_lint(cx: &mut App) {
    cx.set_global(AppId("olo.lint".into()));
    bind_keys(cx);
    crate::widgets::bind_keys(cx);
    crate::editor::bind_keys(cx);
    crate::icons::init(cx);
    cx.set_global(Messages { handler: None, proxy: Proxy::new(|_| {}), quitting: false });
}

struct AppId(String);

impl Global for AppId {}

/// A window's content needs `height` (None: just the window's): taller
/// than the window, the Frame lays it out at that height and scrolls it
/// (`WindowOptions::content_height`, changed as what it shows changes).
/// Call it outside the window's drawing (deferred).
pub fn set_content_height(window: &mut Window, cx: &mut App, height: Option<f32>) {
    if let Some(Some(frame)) = window.root::<Frame>() {
        frame.update(cx, |f, cx| {
            if f.content_height != height {
                f.content_height = height;
                cx.notify();
            }
        });
    }
}

/// Moves `window` with the pointer, as if dragged by its tab, while the
/// button pressed in another of the app's windows is still held: a tab
/// torn off a strip, now a window (opened [`WindowOptions::under_pointer`]),
/// follows the pointer, and let go over another window's strip goes in
/// there (`widgets::strip::on_window_dropped`). Once it has drawn (a
/// window that isn't on screen yet can't be moved).
pub fn begin_move(window: &mut Window) {
    window.on_next_frame(|window, _| window.on_next_frame(|window, _| window.start_window_move()));
}

/// The app's id ("olo.deskcalc"), as [`run`] was given it.
pub fn app_id(cx: &App) -> String {
    cx.try_global::<AppId>().map(|a| a.0.clone()).unwrap_or_default()
}

/// Brings `window` forward if it's still open: whether it was (a
/// window an app keeps one of, asked for again or launched again).
pub fn raise(window: Option<impl Into<gpui::AnyWindowHandle>>, cx: &mut App) -> bool {
    window.is_some_and(|w| w.into().update(cx, |_, window, _| window.activate_window()).is_ok())
}

/// A window the app keeps one of (its main window, History, a review):
/// `kept` brought forward, or, if it isn't open, one opened with
/// `options` showing what `build` makes; `keep` is handed the one
/// showing. Done once the update this is called from is over: what the
/// window shows may read the app, and a window can't be brought forward
/// while it's being updated (it would look closed, and open twice).
pub fn open_or_raise<T: 'static, V: Render + 'static>(
    kept: Option<WindowHandle<Frame>>,
    cx: &mut gpui::Context<T>,
    options: WindowOptions,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V> + 'static,
    keep: impl FnOnce(&mut T, Option<WindowHandle<Frame>>) + 'static,
) {
    let me = cx.entity();
    cx.defer(move |cx| {
        let window = if raise(kept, cx) { kept } else { open_window(cx, options, build) };
        me.update(cx, |t, _| keep(t, window));
    });
}

/// Opens a window showing the view `build` makes.
pub fn open_window<V: Render + 'static>(cx: &mut App, options: WindowOptions, build: impl FnOnce(&mut Window, &mut App) -> Entity<V>) -> Option<WindowHandle<Frame>> {
    // (A surface no bigger than nothing has no buffer to show: app_server
    // ends the connection of a program that offers one.)
    let (mut w, mut h) = options.keep_size.as_ref().and_then(|name| crate::kept::size(cx, name)).unwrap_or(options.size);
    // (Held to what it may be now.)
    if let Some((min_w, min_h)) = options.min_size {
        (w, h) = (w.max(min_w), h.max(min_h));
    }
    if let Some((max_w, max_h)) = options.max_size {
        (w, h) = (if max_w > 0.0 { w.min(max_w) } else { w }, if max_h > 0.0 { h.min(max_h) } else { h });
    }
    let (w, h) = (w.max(1.0), h.max(1.0));
    // (One that follows what it shows opens no taller than the screen.)
    let h = if options.fit_content && options.layer.is_none() && !options.fullscreen { h.min(screen_room(cx)) } else { h };
    let design = (w, options.size.1);
    let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
    let keep = options.keep_size.clone();
    let content_height = options.content_height;
    // (Not a layer's, a lock screen's or a full screen one's: the screen
    // or the app sizes those.)
    let fit_content = options.fit_content && options.layer.is_none() && options.lock_output.is_none() && !options.fullscreen;
    let fixed = options.fixed;
    let title = options.title.clone();
    let panel = options.panel;
    let namespace = options.layer.as_ref().map(|l| l.namespace.clone());
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(if options.fullscreen { WindowBounds::Fullscreen(bounds) } else { WindowBounds::Windowed(bounds) }),
            titlebar: Some(TitlebarOptions { title: Some(title.clone()), ..Default::default() }),
            app_id: Some(app_id(cx)),
            window_min_size: options.min_size.map(|(w, h)| size(px(w), px(h))),
            window_max_size: options.max_size.map(|(w, h)| size(px(w), px(h))),
            toplevel_tag: {
                let restore = options.restore_key.as_ref().map(|k| format!("restore:{}", k.split_whitespace().collect::<String>()));
                let mut tags: Vec<&str> = [(options.floating, "floating"), (options.under_pointer, "under-pointer")].into_iter().filter_map(|(on, tag)| on.then_some(tag)).collect();
                tags.extend(restore.as_deref());
                (!tags.is_empty()).then(|| tags.join(" "))
            },
            kind: if options.lock_output.is_some() {
                gpui::WindowKind::SessionLock
            } else if options.layer.is_some() {
                gpui::WindowKind::LayerShell
            } else if options.child {
                // (Its parent is the window with the keyboard.)
                gpui::WindowKind::Floating
            } else {
                gpui::WindowKind::Normal
            },
            session_lock_output: options.lock_output.clone(),
            // A layer that draws no panel (an overlay drawing its own
            // panel, the command palette's) shows what's under it.
            window_background: if options.layer.is_some() && !options.panel { gpui::WindowBackgroundAppearance::Transparent } else { gpui::WindowBackgroundAppearance::Opaque },
            layer_shell: options.layer.clone(),
            dialog: options.dialog,
            foreign_parent: options.foreign_parent.clone(),
            is_resizable: !options.fixed,
            focus: options.layer.as_ref().is_none_or(|l| l.keyboard != gpui::LayerKeyboard::None),
            ..Default::default()
        },
        move |window, cx| {
            if let Some(n) = &namespace {
                agent::set_layer(window, cx, n);
            }
            // (Its first showing can be held from now: its content may
            // ask as it's made.)
            let first = FirstShow::new();
            if fit_content {
                let id = window.window_handle().window_id();
                FIRST_SHOWS.with(|f| f.borrow_mut().insert(id, first.clone()));
            }
            let content = build(window, cx).into();
            let frame = cx.new(|cx| {
                let _kept = keep.map(|name| cx.observe_window_bounds(window, move |_, window, cx| crate::kept::note(cx, &name, window)));
                // (A display come or gone changes the room there is.)
                // (Deferred: displays are told of inside a Wayland
                // roundtrip, as `on_displays_changed`.)
                let me = cx.weak_entity();
                let fit = fit_content.then(|| Fit {
                    design,
                    fixed,
                    need: Default::default(),
                    asked: Default::default(),
                    resizing: Default::default(),
                    first,
                    _displays: cx.on_displays_changed(move |cx| {
                        let me = me.clone();
                        cx.defer(move |cx| {
                            let _ = me.update(cx, |f: &mut Frame, cx| {
                                if let Some(fit) = &f.fit {
                                    fit.asked.set(None);
                                }
                                cx.notify();
                            });
                        });
                    }),
                });
                Frame { title, content, panel, focus: cx.focus_handle(), _kept, content_height, scroll: gpui::ScrollHandle::new(), fit }
            });
            if window.focused(cx).is_none() {
                let focus = frame.read(cx).focus.clone();
                window.focus(&focus);
            }
            frame
        },
    )
    .ok()
}

/// What the app does with messages from other threads ([`proxy`]).
struct Messages {
    handler: Option<Box<dyn FnMut(Box<dyn Any + Send>, &mut App)>>,
    proxy: Proxy,
    /// Its last window closed and it is ending: a launch forwarded now is
    /// refused (the launching process starts a copy of its own).
    quitting: bool,
}

impl Global for Messages {}

/// Something any thread can send messages to the app with (a launch
/// forwarded by another copy, a worker's result); [`message_handler`]
/// takes them on the UI thread.
pub fn proxy(cx: &App) -> Proxy {
    cx.global::<Messages>().proxy.clone()
}

/// Takes the messages sent through [`proxy`].
pub fn message_handler(cx: &mut App, handler: impl FnMut(Box<dyn Any + Send>, &mut App) + 'static) {
    cx.global_mut::<Messages>().handler = Some(Box::new(handler));
}

struct Resident;

impl Global for Resident {}

/// Keeps the app running with no windows open (an agent that opens them
/// when asked: the polkit agent, Notifications).
pub fn stay_resident(cx: &mut App) {
    cx.set_global(Resident);
}

/// The app runs with no windows open (`stay_resident`): a service of the
/// display it's on, which a move leaves there.
pub(crate) fn resident(cx: &App) -> bool {
    cx.has_global::<Resident>()
}

/// Runs the app: its theme, its socket for agents, and `build`, which
/// opens its windows. It ends when its last window closes.
pub fn run(app_id: &str, build: impl FnOnce(&mut App) + 'static) {
    // Olo's apps render with the GPU the compositor's primary is (the one
    // the computer started on, which GPUI takes first): a device chosen
    // for GPUI (`ZED_DEVICE_ID`), as briefly Olo did and its programs'
    // children inherit, would have each frame copied between GPUs.
    // SAFETY: the app's first act, before any thread is started.
    unsafe { std::env::remove_var("ZED_DEVICE_ID") };
    // The state an upgrade handed over, its file gone whether or not the
    // app asks for it.
    olo_kit_base::restore::claim();
    olo_kit_base::moving::claim();
    crate::upgrade::claim();
    // Olo's own crates keep their info (what is rare and worth knowing
    // after: a chat list loaded, sudo approved without asking),
    // dependencies only their warnings.
    // (Ready for a crash first: its report, and stderr to the journal
    // when it went nowhere; `main` has done it already, as every
    // program's should.)
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,usvg=error,olo=info"));
    // Colours only for a terminal: not in the journal's lines.
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(ansi).with_writer(std::io::stderr).try_init();
    let app_id = app_id.to_owned();
    Application::new().run(move |cx: &mut App| {
        cx.set_global(AppId(app_id.clone()));
        olo_kit_base::keys::set_app_id(&app_id);
        theme::init(cx);
        agent::init(cx, &app_id);
        // The window's own keys first: a binding in a view's key context
        // as deep as a window-wide one wins only when it was bound later.
        bind_keys(cx);
        crate::widgets::bind_keys(cx);
        crate::editor::bind_keys(cx);
        crate::upgrade::init(cx);
        crate::icons::init(cx);
        let (tx, mut rx) = mpsc::unbounded::<Box<dyn Any + Send>>();
        let proxy = Proxy::new(move |m| {
            let _ = tx.unbounded_send(m);
        });
        cx.set_global(Messages { handler: None, proxy, quitting: false });
        // What the app saved (`store::save`), and the sizes its windows
        // were left at, are on disk before it goes.
        cx.on_app_quit(|cx| {
            olo_kit_base::instance::close();
            crate::kept::write(cx);
            olo_kit_base::store::flush();
            async {}
        })
        .detach();
        cx.spawn(async move |cx| {
            while let Some(message) = rx.next().await {
                let _ = cx.update(|cx| {
                    let messages = cx.global_mut::<Messages>();
                    // A launch forwarded from another process is told the
                    // app took it, or dropped unanswered (so that process
                    // opens a copy of its own) if the app is ending or has
                    // nothing to take it with, or the process gave up.
                    if let Some(launch) = message.downcast_ref::<olo_kit_base::instance::Launch>()
                        && (messages.quitting || messages.handler.is_none() || !launch.take())
                    {
                        return;
                    }
                    if let Some(mut handler) = messages.handler.take() {
                        handler(message, cx);
                        cx.global_mut::<Messages>().handler = Some(handler);
                    }
                });
            }
        })
        .detach();
        cx.on_window_closed(|cx| {
            crate::kept::write(cx);
            agent::forget_closed(cx);
            crate::cut::forget_closed(cx);
            let open: std::collections::HashSet<gpui::WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
            cx.default_global::<DefaultButtons>().0.retain(|id, _| open.contains(id));
            cx.default_global::<TabKeys>().0.retain(|id, _| open.contains(id));
            cx.default_global::<crate::undo::UndoKeys>().0.retain(|id, _| open.contains(id));
            crate::menu::forget_closed(cx);
            if cx.windows().is_empty() && !cx.has_global::<Resident>() {
                cx.global_mut::<Messages>().quitting = true;
                // (Launches from now on start a copy of their own at once.)
                olo_kit_base::instance::close();
                cx.quit();
            }
        })
        .detach();
        build(cx);
    });
}

/// Called when a display came or went, as the compositor told this app
/// (its `cx.displays()` lists it then). A window for a display the app
/// is told of some other way (app_server's `OutputsChanged`) may be
/// asked for before that, and refused: ask again from here.
pub fn on_displays_changed(cx: &mut App, f: impl FnMut(&mut App) + 'static) {
    // Deferred: the platform tells of a display inside a Wayland roundtrip,
    // which a window being opened makes, so this may come while the app's
    // entity is being updated (Deskbar: "cannot update while it is already
    // being updated", 2026-11-01). After that update, then.
    let f = std::rc::Rc::new(std::cell::RefCell::new(f));
    cx.on_displays_changed(move |cx| {
        let f = f.clone();
        cx.defer(move |cx| (f.borrow_mut())(cx));
    })
    .detach();
}
