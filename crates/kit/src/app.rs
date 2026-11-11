//! Running an app and opening its windows.
//!
//! [`run`] starts the program (its look, its keyboard profile, the
//! agents' socket) and runs its looper on the main thread;
//! [`open_window`] opens a window on a looper of its own, the app's
//! view inside a [`Frame`], which paints the panel behind it and sets
//! Olo's font and the palette's text colour.

use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use olo_interface::{
    AnyChild, AppCx, Bounds, Cx, Element, FocusHandle, InteractiveElement as _, IntoElement as _, ParentElement as _, Pixels, ScrollHandle, SharedString, Size, Styled as _, View, WindowCx,
    WindowHandle, WindowId, WindowSpec, div, px, size,
};
use olo_kit_base::Proxy;

use crate::{style, theme};

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
    pub layer: Option<olo_interface::LayerShellOptions>,
    /// A dialog (never a tab), modal or not.
    pub dialog: Option<olo_interface::Dialog>,
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
        WindowOptions { floating: true, dialog: Some(olo_interface::Dialog::Modal), fixed: true, fit_content: true, ..WindowOptions::new(title, width, height) }
    }

    /// A dialog (a panel, a sheet): floats, never a tab. Never smaller
    /// than what it shows needs, nor taller than the screen
    /// (`fits_content`).
    pub fn dialog(mut self) -> Self {
        self.floating = true;
        self.dialog = Some(olo_interface::Dialog::Plain);
        self.fit_content = true;
        self
    }

    /// A layer surface (see [`Self::layer`]), `namespace` saying what it
    /// is to the compositor, anchored to the edges given (top, bottom,
    /// left, right), on `layer`; refine it with [`Self::layer_options`].
    pub fn layer(mut self, namespace: &str, layer: olo_interface::LayerShellLayer, anchor: (bool, bool, bool, bool)) -> Self {
        self.layer = Some(olo_interface::LayerShellOptions {
            layer,
            anchor,
            exclusive_zone: 0,
            margin: (0, 0, 0, 0),
            keyboard: olo_interface::LayerKeyboard::None,
            namespace: namespace.to_owned(),
            output: None,
        });
        self
    }

    /// Changes the layer surface's options (exclusive zone, margins,
    /// keyboard, output).
    pub fn layer_options(mut self, f: impl FnOnce(&mut olo_interface::LayerShellOptions)) -> Self {
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

impl WindowOptions {
    /// What the display thread is asked for.
    fn spec(&self) -> WindowSpec {
        let (mut w, mut h) = self.size;
        // (Held to what it may be now.)
        if let Some((min_w, min_h)) = self.min_size {
            (w, h) = (w.max(min_w), h.max(min_h));
        }
        if let Some((max_w, max_h)) = self.max_size {
            (w, h) = (if max_w > 0.0 { w.min(max_w) } else { w }, if max_h > 0.0 { h.min(max_h) } else { h });
        }
        // (A surface no bigger than nothing has no buffer to show:
        // app_server ends the connection of a program that offers one.)
        let (w, h) = (w.max(1.0), h.max(1.0));
        let restore = self.restore_key.as_ref().map(|k| format!("restore:{}", k.split_whitespace().collect::<String>()));
        let mut tags: Vec<&str> = [(self.floating, "floating"), (self.under_pointer, "under-pointer")].into_iter().filter_map(|(on, tag)| on.then_some(tag)).collect();
        tags.extend(restore.as_deref());
        WindowSpec {
            title: self.title.clone(),
            size: (w, h),
            min_size: self.min_size,
            max_size: self.max_size,
            kind: if self.lock_output.is_some() {
                olo_interface::WindowKind::SessionLock
            } else if self.layer.is_some() {
                olo_interface::WindowKind::LayerShell
            } else if self.child {
                // (Its parent is the window with the keyboard.)
                olo_interface::WindowKind::Floating
            } else {
                olo_interface::WindowKind::Normal
            },
            layer_shell: self.layer.clone(),
            dialog: self.dialog,
            toplevel_tag: (!tags.is_empty()).then(|| tags.join(" ")),
            foreign_parent: self.foreign_parent.clone(),
            session_lock_output: self.lock_output.clone(),
            // A layer that draws no panel (an overlay drawing its own
            // panel, the command palette's) shows what's under it.
            background: if self.layer.is_some() && !self.panel { olo_interface::WindowBackgroundAppearance::Transparent } else { olo_interface::WindowBackgroundAppearance::Opaque },
            fullscreen: self.fullscreen,
            resizable: !self.fixed,
            focus: self.layer.as_ref().is_none_or(|l| l.keyboard != olo_interface::LayerKeyboard::None),
        }
    }
}

/// A window's first view: the panel behind the app's view, Olo's font
/// and the palette's text colour for what it draws, the window's own
/// keys, and, for a window that follows what it shows
/// (`WindowOptions::fits_content`), its size.
pub struct Frame {
    content: AnyChild,
    panel: bool,
    /// Has the keyboard when nothing in the window does, so the menu
    /// bar's keys reach it.
    focus: FocusHandle,
    /// Its size is noted as it changes (`WindowOptions::keep_size`).
    keep: Option<SharedString>,
    /// The height what it shows needs, and where it's scrolled to in a
    /// window less tall (`WindowOptions::content_height`).
    content_height: Option<f32>,
    scroll: ScrollHandle,
    /// The window follows what it shows (`WindowOptions::fits_content`).
    fit: Option<Fit>,
}

/// A window that follows what it shows: its size as designed, what its
/// content needed as last laid out, and the size last asked for.
struct Fit {
    design: (f32, f32),
    /// Its size is its content's, not the person's.
    fixed: bool,
    need: Option<f32>,
    /// The size last asked for (so each is asked once).
    asked: Option<(f32, f32)>,
    /// A resize asked for and not made yet: the size it was then, and
    /// when (nothing is shown until it's another, or `RESIZE_WAIT`).
    resizing: Option<(Size<Pixels>, Instant)>,
    /// What it shows needs more than the screen has room for: scrolled.
    scrolls: bool,
    /// What it waits for before it's first shown ([`hold_first_show`]).
    first: Arc<FirstShow>,
}

/// What a window's frame is told.
#[derive(Clone)]
pub enum FrameMessage {
    /// What it shows was laid out this tall.
    Measured(f32),
    /// The window is another size, or held another way.
    Sized,
    /// A hold on its first showing was let go, or its wait is over.
    HoldLetGo,
    /// A display came or went: the room there is may be another.
    Displays,
    /// What it shows needs this height (`set_content_height`).
    ContentHeight(Option<f32>),
}

/// The longest a window that fits what it shows holds back what it
/// shows for a resize it asked for: a compositor that keeps it at its
/// size (a tile, Focus's zoom) never makes it, and the window was never
/// seen (DiskBackup in Focus, Ben 2026-11-08: "an entry in the deskbar
/// but no windows").
pub const RESIZE_WAIT: Duration = Duration::from_millis(300);

/// How long a window that fits what it shows waits, at most, for what it
/// asked for as it opened before it's first shown ([`hold_first_show`]):
/// an answer slower than this shows it as it is, and it grows when the
/// answer comes.
pub const FIRST_SHOW_WAIT: Duration = Duration::from_millis(150);

/// A window's first showing, held ([`hold_first_show`]).
pub struct FirstShow {
    opened: Instant,
    /// The holds not let go yet.
    waiting: AtomicUsize,
    /// It was shown (or stopped waiting): no more holds.
    over: AtomicBool,
    /// Its frame, told as each hold is let go (from any thread).
    frame: olo_interface::app_kit::Messenger<FrameMessage>,
}

impl FirstShow {
    /// Whether what it shows is held back (something asked for is still
    /// to come, and there's time to wait for it).
    fn holding(&self) -> bool {
        if self.over.load(Ordering::SeqCst) {
            return false;
        }
        if self.waiting.load(Ordering::SeqCst) > 0 && self.opened.elapsed() < FIRST_SHOW_WAIT {
            return true;
        }
        self.over.store(true, Ordering::SeqCst);
        false
    }
}

/// A hold on a window's first showing ([`hold_first_show`]): let go by
/// dropping it, on any thread.
pub struct FirstShowHold(Arc<FirstShow>);

impl Drop for FirstShowHold {
    fn drop(&mut self) {
        self.0.waiting.fetch_sub(1, Ordering::SeqCst);
        self.0.frame.send(FrameMessage::HoldLetGo);
    }
}

/// The windows not shown yet that fit what they show: their holds.
#[derive(Default)]
struct FirstShows(std::collections::HashMap<WindowId, Arc<FirstShow>>);

/// Hold a window that fits what it shows (`WindowOptions::fits_content`)
/// from being first shown until what it asked for as it opened has come
/// (a worker's answer that adds to what it shows: drop the hold when it's
/// in), for [`FIRST_SHOW_WAIT`] at most: so it opens at the size of what
/// it shows, not at a size it grows from a moment later. Nothing waits
/// for it: what the window would have shown before is held back. None
/// once it has been shown, and for other windows.
pub fn hold_first_show(cx: &mut WindowCx) -> Option<FirstShowHold> {
    let id = cx.window_id();
    let first = cx.looper().state::<FirstShows>()?.0.get(&id).cloned()?;
    if first.over.load(Ordering::SeqCst) {
        return None;
    }
    first.waiting.fetch_add(1, Ordering::SeqCst);
    Some(FirstShowHold(first))
}

/// The size a window that follows what it shows should be (`viewport` now,
/// its content needing `need`), and whether its content scrolls there.
fn fitted(design: (f32, f32), fixed: bool, viewport: Size<Pixels>, need: f32, room: f32) -> ((f32, f32), bool) {
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
pub fn screen_room() -> f32 {
    olo_interface::displays().iter().map(|d| f32::from(d.size.height)).fold(f32::INFINITY, f32::min).max(240.0 + SCREEN_EDGES) - SCREEN_EDGES
}

/// Next Tab (+1: ⌃Tab, ⌃Page Down) or Previous Tab (−1: ⌃⇧Tab, ⌃Page Up);
/// 0 for any other key.
pub fn tab_step(k: &olo_interface::Keystroke) -> i32 {
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

impl Frame {
    /// A window that follows what it shows is brought in line with what
    /// its content needs now: asked to be that size (held back until it
    /// is, so it's never seen at the wrong one), or, past the screen's
    /// room, or where the compositor holds its size, scrolled.
    fn follow(&mut self, cx: &mut Cx<Self>) {
        let Some(fit) = self.fit.as_mut() else { return };
        let Some(need) = fit.need else { return };
        // (Not shown at all while what it asked for as it opened is
        // still to come: `hold_first_show`.)
        if !fit.first.over.load(Ordering::SeqCst) {
            if fit.first.holding() {
                let left = FIRST_SHOW_WAIT.saturating_sub(fit.first.opened.elapsed());
                cx.hold_commit(left);
                cx.after(left + Duration::from_millis(2), FrameMessage::HoldLetGo);
                return;
            }
            // (Shown from now: no more holds.)
            let id = cx.window_id();
            if let Some(shows) = cx.looper().state_mut::<FirstShows>() {
                shows.0.remove(&id);
            }
        }
        let now = cx.viewport_size();
        // Its size the compositor's (a tile, a zoom): as given, asking
        // for nothing (and asking again once it isn't), scrolled if what
        // it shows needs more.
        let scrolls = if cx.held_state().held() {
            fit.resizing = None;
            fit.asked = None;
            need > f32::from(now.height) + 0.5
        } else {
            let ((w, h), scrolls) = fitted(fit.design, fit.fixed, now, need, screen_room());
            let off = (f32::from(now.width) - w).abs() > 0.5 || (f32::from(now.height) - h).abs() > 0.5;
            // (The resize asked for is made when its size is another:
            // laid out at it, or at what the compositor gave instead; or
            // it never comes, and the window is shown as it is after
            // `RESIZE_WAIT`.)
            if fit.resizing.is_some_and(|(then, at)| then != now || at.elapsed() > RESIZE_WAIT) {
                fit.resizing = None;
            }
            if off && fit.asked != Some((w, h)) {
                fit.asked = Some((w, h));
                fit.resizing = Some((now, Instant::now()));
                cx.resize_window(size(px(w), px(h)));
                cx.hold_commit(RESIZE_WAIT);
            }
            scrolls
        };
        if std::mem::replace(&mut fit.scrolls, scrolls) != scrolls {
            cx.invalidate();
        }
        if fit.resizing.is_none() {
            cx.release_commit();
        }
    }
}

impl View for Frame {
    type Message = FrameMessage;

    fn message(&mut self, message: FrameMessage, cx: &mut Cx<Self>) {
        match message {
            FrameMessage::Measured(need) => {
                if let Some(fit) = self.fit.as_mut() {
                    fit.need = Some(need);
                }
                self.follow(cx);
            }
            FrameMessage::Sized => {
                // Its size noted as the person changes it (not one the
                // compositor chose: a tile, maximized, full screen).
                if let Some(name) = &self.keep
                    && !cx.held_state().held()
                {
                    let s = cx.viewport_size();
                    crate::kept::note_size(name, (f32::from(s.width).round(), f32::from(s.height).round()));
                }
                if self.content_height.is_some() {
                    cx.invalidate();
                }
                self.follow(cx);
            }
            FrameMessage::HoldLetGo => self.follow(cx),
            FrameMessage::Displays => {
                if let Some(fit) = self.fit.as_mut() {
                    fit.asked = None;
                }
                self.follow(cx);
            }
            FrameMessage::ContentHeight(height) => {
                if self.content_height != height {
                    self.content_height = height;
                    cx.invalidate();
                }
            }
        }
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        crate::kept::write();
    }

    // (A display come or gone changes the room there is.)
    fn displays_changed(&mut self, cx: &mut Cx<Self>) {
        self.message(FrameMessage::Displays, cx);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let mut root = div()
            .size_full()
            .track_focus(&self.focus)
            .on_bounds_changed(cx.msg(FrameMessage::Sized))
            // A press anywhere in the window puts away a menu open over it
            // (and a cut text's tip).
            .on_window_mouse_down(|e, cx| {
                crate::cut::left(cx, true);
                crate::menu::press_in(e.position, cx)
            })
            // Text cut short: its whole in a tip (`cut`), at once with ⌘.
            // (Not while a button is held: a divider or a selection
            // dragged over cut text has no tip.)
            .on_window_mouse_move(|e, cx| {
                if e.pressed_button.is_some() {
                    crate::cut::left(cx, false);
                } else {
                    crate::cut::moved(cx, e.position, crate::cut::command_held(&e.modifiers))
                }
            })
            .on_hover(|over, cx| {
                if !*over {
                    crate::cut::left(cx, false);
                }
            })
            .on_modifiers_changed(|e, cx| {
                let at = cx.mouse_position();
                crate::cut::modifiers(cx, at, crate::cut::command_held(&e.modifiers))
            })
            // The menu bar's keys, when nothing in the window took them
            // (a text field's ⌘C is its own).
            .on_key_down(|e, cx| {
                if let Some((command, on_command)) = crate::menu::bar_shortcut(cx, &e.keystroke) {
                    on_command(&command, cx);
                    cx.stop_propagation();
                    return;
                }
                // Undo and Redo (the profile's keys) where no menu has
                // them: the window's own view's `undo` and `redo`.
                if let Some(redo) = crate::undo::pressed(&e.keystroke) {
                    crate::menu::on_command(if redo { "redo" } else { "undo" }, cx);
                    cx.stop_propagation();
                    return;
                }
                // ⌘W (the profile's Close) on a window whose app has no
                // Close of its own: closed as its tab's close button does,
                // the app's confirmations first.
                if crate::keys::pressed(&olo_kit_base::keys::Shortcut::command(crate::keys::evdev("w")), &e.keystroke) {
                    cx.request_close();
                    cx.stop_propagation();
                }
            })
            .font_family(style::UI_FONT)
            .text_size(px(style::PLAIN_SIZE))
            .line_height(olo_interface::relative(style::LINE_HEIGHT))
            .text_color(style::c(p.panel_text));
        if self.panel {
            root = root.bg(style::c(p.panel_background));
        }
        // Following what it shows: laid out at what it needs (its size
        // as designed at least), measured as it's laid out, and scrolled
        // past the screen's room.
        if let Some(fit) = &self.fit {
            // (A flex item based at the designed height, its minimum what
            // its content needs: so as tall as the larger. One the person
            // or the app sizes: as the window, or what its content needs
            // if more.)
            let basis = if fit.fixed { fit.design.1 } else { 0.0 };
            let mut laid = div()
                .relative()
                .w_full()
                .flex_basis(px(basis))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .on_bounds_changed(cx.msg_with(|b: &Bounds<Pixels>| FrameMessage::Measured(f32::from(b.size.height))))
                .child(self.content.element());
            if !fit.fixed {
                laid = laid.flex_grow();
            }
            let content = if fit.scrolls { crate::widgets::ScrollView::new(&self.scroll).border(false).child(div().w_full().flex().flex_col().child(laid)).into_element() } else { laid };
            return root.flex().flex_col().child(content);
        }
        // Taller than the window (a short screen): laid out at its own
        // height all the same, and scrolled.
        let height = cx.viewport_size().height;
        match self.content_height {
            Some(h) if f32::from(height) + 0.5 < h => {
                let sized = div().w_full().h(px(h)).flex().flex_col().child(self.content.element());
                root.child(crate::widgets::ScrollView::new(&self.scroll).border(false).child(sized))
            }
            _ => root.child(self.content.element()),
        }
    }
}

/// A window's content needs `height` (None: just the window's): taller
/// than the window, the Frame lays it out at that height and scrolls it
/// (`WindowOptions::content_height`, changed as what it shows changes).
pub fn set_content_height(cx: &mut WindowCx, height: Option<f32>) {
    cx.send_root::<Frame>(FrameMessage::ContentHeight(height));
}

/// Moves the window with the pointer, as if dragged by its tab, while the
/// button pressed in another of the app's windows is still held: a tab
/// torn off a strip, now a window (opened [`WindowOptions::under_pointer`]),
/// follows the pointer, and let go over another window's strip goes in
/// there. Once it has drawn (a window that isn't on screen yet can't be
/// moved).
pub fn begin_move(cx: &mut WindowCx) {
    cx.on_next_frame(|cx| cx.on_next_frame(|cx| cx.start_window_move()));
}

/// The app's id ("olo.deskcalc"), as [`run`] was given it.
pub fn app_id() -> &'static str {
    olo_interface::app_id()
}

/// Brings `window` forward if it's still open: whether it was (a window
/// an app keeps one of, asked for again or launched again).
pub fn raise<V: View>(window: Option<&WindowHandle<V>>) -> bool {
    match window {
        Some(window) if window.is_open() => {
            window.with(|cx| cx.activate_window());
            true
        }
        _ => false,
    }
}

/// A window the app keeps one of (its main window, History, a review):
/// `kept` brought forward, or, if it isn't open, one opened with
/// `options` showing what `build` makes and kept there.
pub fn open_or_raise<V: View>(kept: &mut Option<WindowHandle<V>>, options: WindowOptions, build: impl FnOnce(&mut Cx<V>) -> V + Send + 'static) {
    if !raise(kept.as_ref()) {
        *kept = Some(open_window(options, build));
    }
}

/// Opens a window on a looper of its own, showing the view `build`
/// makes there (the view the window's handle reaches).
pub fn open_window<V: View>(options: WindowOptions, build: impl FnOnce(&mut Cx<V>) -> V + Send + 'static) -> WindowHandle<V> {
    let (spec, frame) = framed(options, build);
    olo_interface::open_window(spec, frame).retyped()
}

/// Opens a window on this window's own looper, showing the view `build`
/// makes: one that belongs to this window and is told things by it at
/// once, and tells it (an alert, a panel of its own), where a window of
/// its own work gets a looper of its own ([`open_window`]). It closes
/// with this window.
pub fn open_window_here<V: View>(cx: &mut WindowCx, options: WindowOptions, build: impl FnOnce(&mut Cx<V>) -> V + 'static) -> Option<olo_interface::LocalWindow<V>> {
    let (spec, frame) = framed(options, build);
    cx.open_window_here(spec, frame).map(|window| window.retyped())
}

/// What a window is opened with: what the display thread is asked for,
/// and its frame round the view `build` makes.
fn framed<V: View, B: FnOnce(&mut Cx<V>) -> V>(mut options: WindowOptions, build: B) -> (WindowSpec, impl FnOnce(&mut Cx<Frame>) -> Frame + use<V, B>) {
    // The size it was last left at, held to what it may be now.
    if let Some(size) = options.keep_size.as_ref().and_then(|name| crate::kept::size(name)) {
        options.size = size;
    }
    let designed_height = options.size.1;
    // (Not a layer's, a lock screen's or a full screen one's: the screen
    // or the app sizes those.)
    let fit_content = options.fit_content && options.layer.is_none() && options.lock_output.is_none() && !options.fullscreen;
    // (One that follows what it shows opens no taller than the screen.)
    if fit_content {
        options.size.1 = options.size.1.min(screen_room());
    }
    let spec = options.spec();
    let design = (spec.size.0, designed_height);
    let (panel, fixed, keep, content_height) = (options.panel, options.fixed, options.keep_size.clone(), options.content_height);
    let namespace = options.layer.as_ref().map(|l| l.namespace.clone());
    let frame = move |cx: &mut Cx<Frame>| {
        if let Some(namespace) = &namespace {
            crate::agent::set_layer(cx, namespace);
        }
        // (Its first showing can be held from now: its content may ask
        // as it's made.)
        let first = Arc::new(FirstShow { opened: Instant::now(), waiting: AtomicUsize::new(0), over: AtomicBool::new(!fit_content), frame: cx.messenger() });
        if fit_content {
            let id = cx.window_id();
            let looper = cx.looper();
            if looper.state::<FirstShows>().is_none() {
                looper.set_state(FirstShows::default());
            }
            if let Some(shows) = looper.state_mut::<FirstShows>() {
                shows.0.insert(id, first.clone());
            }
        }
        let content = cx.new_view(build);
        cx.set_main_view(&content);
        let focus = FocusHandle::new();
        // (The keyboard is the frame's until something in it takes it,
        // and again whenever nothing has it.)
        focus.focus_if_none(cx);
        Frame {
            content: content.into(),
            panel,
            focus,
            keep,
            content_height,
            scroll: ScrollHandle::new(),
            fit: fit_content.then(|| Fit { design, fixed, need: None, asked: None, resizing: None, scrolls: false, first }),
        }
    };
    (spec, frame)
}

/// What the app does with messages from other threads ([`proxy`]).
#[derive(Default)]
struct Messages {
    handler: Option<Box<dyn FnMut(Box<dyn Any + Send>, &mut AppCx)>>,
}

static PROXY: std::sync::OnceLock<Proxy> = std::sync::OnceLock::new();

/// Something any thread can send messages to the app with (a launch
/// forwarded by another copy, a worker's result); [`message_handler`]
/// takes them on the app's looper.
pub fn proxy() -> Proxy {
    PROXY.get().cloned().unwrap_or_else(|| Proxy::new(|_| {}))
}

/// Takes the messages sent through [`proxy`], on the app's looper.
pub fn message_handler(app: &mut AppCx, handler: impl FnMut(Box<dyn Any + Send>, &mut AppCx) + 'static) {
    let looper = app.looper();
    if looper.state::<Messages>().is_none() {
        looper.set_state(Messages::default());
    }
    if let Some(messages) = looper.state_mut::<Messages>() {
        messages.handler = Some(Box::new(handler));
    }
}

/// Keeps the app running with no windows open (an agent that opens them
/// when asked: the polkit agent, Notifications).
pub fn stay_resident() {
    olo_interface::stay_resident()
}

/// Called (on the app's looper) when a display came or went, as the
/// compositor told this app (`olo_interface::displays()` lists it
/// then). A window for a display the app is told of some other way
/// (app_server's `OutputsChanged`) may be asked for before that, and
/// refused: ask again from here.
pub fn on_displays_changed(f: impl Fn() + Send + Sync + 'static) {
    olo_interface::on_displays_changed(f)
}

/// How many arenas glibc's malloc keeps for a kit app's threads.
const ARENAS: libc::c_int = 4;

/// glibc's malloc gives each thread that allocates an arena of its own
/// (eight for each core, at most), and what a thread frees stays in its
/// arena: a kit app has a looper for each window, the display's thread
/// and its workers (18 to 29 threads), and rested 7 to 17 MB bigger for
/// it than with [`ARENAS`] between them (2026-11-11, release, resident:
/// Terminal with three tabs 153 → 136 MB, StyledEdit 103 → 96, Tracker
/// 99 → 91), its frames costing the same (Terminal's 329 → 342 µs,
/// StyledEdit's 925 → 907, within what two takes differ by). Two saved
/// no more on Terminal and its frames ran slower (415 µs): threads
/// waiting on each other's arena.
/// Not if the environment says how many (`MALLOC_ARENA_MAX`): to
/// measure another number without a build.
fn few_arenas() {
    #[cfg(target_env = "gnu")]
    if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
        // SAFETY: sets a number malloc reads as it makes an arena.
        unsafe { libc::mallopt(libc::M_ARENA_MAX, ARENAS) };
    }
}

/// Runs the program as `app_id` ("olo.deskcalc"): its theme, its socket
/// for agents, and `build`, which opens its windows. It returns when the
/// program quits: when its last window closes, unless it's
/// [`stay_resident`].
pub fn run(app_id: &str, build: impl FnOnce(&mut AppCx)) {
    few_arenas();
    // Olo's apps render with the GPU the compositor's primary is (the one
    // the computer started on, which the renderer takes first): a device
    // chosen for it (`ZED_DEVICE_ID`), as briefly Olo did and its
    // programs' children inherit, would have each frame copied between
    // GPUs.
    // SAFETY: the app's first act, before any thread is started.
    unsafe { std::env::remove_var("ZED_DEVICE_ID") };
    // The state an upgrade handed over, its file gone whether or not the
    // app asks for it.
    olo_kit_base::restore::claim();
    olo_kit_base::moving::claim();
    crate::upgrade::claim();
    // Olo's own crates keep their info (what is rare and worth knowing
    // after), dependencies only their warnings.
    // (Ready for a crash first: its report, and stderr to the journal
    // when it went nowhere; `main` has done it already, as every
    // program's should.)
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,usvg=error,olo=info"));
    // Colours only for a terminal: not in the journal's lines.
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(ansi).with_writer(std::io::stderr).try_init();
    olo_kit_base::keys::set_app_id(app_id);
    theme::init();
    // (Quit with what it copied still on the clipboard, it runs on for
    // that alone: launches start a copy of their own meanwhile, and what
    // it keeps is on disk.)
    olo_interface::before_lingering(|| {
        olo_kit_base::instance::close();
        crate::kept::write();
        olo_kit_base::store::flush();
    });
    olo_interface::run(app_id, |app| {
        crate::agent::init();
        crate::upgrade::init();
        // What other threads send the app, taken on its looper.
        let messenger = olo_interface::app_kit::Messenger::new(app.looper().handle(), |looper, message: Box<dyn Any + Send>| {
            // A launch forwarded from another process is told the app
            // took it, or dropped unanswered (so that process opens a
            // copy of its own) if the app is ending or has nothing to
            // take it with, or the process gave up.
            let has_handler = looper.state::<Messages>().is_some_and(|m| m.handler.is_some());
            if let Some(launch) = message.downcast_ref::<olo_kit_base::instance::Launch>()
                && (olo_interface::is_quitting() || !has_handler || !launch.take())
            {
                return;
            }
            let Some(mut handler) = looper.state_mut::<Messages>().and_then(|m| m.handler.take()) else { return };
            handler(message, &mut AppCx::on(looper));
            if let Some(messages) = looper.state_mut::<Messages>()
                && messages.handler.is_none()
            {
                messages.handler = Some(handler);
            }
        });
        let _ = PROXY.set(Proxy::new(move |message| {
            messenger.send(message);
        }));
        build(app)
    });
    // (Launches from now on start a copy of their own at once.) What the
    // app saved (`store::save`), and how its windows were left, is on
    // disk before it goes.
    olo_kit_base::instance::close();
    crate::kept::write();
    olo_kit_base::store::flush();
}
