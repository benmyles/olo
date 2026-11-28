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
            title: crate::words::of(title),
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
    /// A control lit to say "here" (`spotlight`): where, and since when.
    lit: Option<(Bounds<Pixels>, Instant)>,
}

/// A window that follows what it shows: its size as designed, what its
/// content needed as last laid out, and the size last asked for.
struct Fit {
    design: (f32, f32),
    /// The narrowest it can be laid out (a fixed one's least width,
    /// `WindowOptions::min_size`: its columns stacked), its own width if
    /// it can't be narrower.
    narrowest: f32,
    /// Its size is its content's, not the person's.
    fixed: bool,
    need: Option<f32>,
    /// The size last asked for (so each is asked once).
    asked: Option<(f32, f32)>,
    /// A resize asked for and not made yet: the size it was then, and
    /// when (nothing is shown until it's another, or `RESIZE_WAIT`).
    resizing: Option<(Size<Pixels>, Instant)>,
    /// What it shows needs more than the screen has room for: scrolled,
    /// down and across ([`fitted`]).
    scrolls: (bool, bool),
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
    /// A control here is lit for a moment (`spotlight::show`).
    Light(Bounds<Pixels>),
    /// The lit control's moment is over.
    Unlit,
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
/// its content needing `need`, the screen having `room` across and down:
/// [`screen_room_across`], [`screen_room`]), and whether its content
/// scrolls there: down, and across. A fixed window wider than the room
/// (a panel made for 1280 points on a screen of 720: 1440 × 900 at 2×)
/// is as wide as the room, its content laid out at its own width and
/// scrolled across, the bar along its foot counted in its height; it had
/// run off the screen, its scroll bar and last column out of reach (QA,
/// 2026-11-28).
fn fitted(design: (f32, f32), narrowest: f32, fixed: bool, viewport: Size<Pixels>, need: f32, room: (f32, f32)) -> ((f32, f32), (bool, bool)) {
    let bar = crate::widgets::SCROLL_BAR;
    // (One the person sizes keeps their size when it's enough.)
    let least = if fixed { design.1 } else { f32::from(viewport.height) };
    let need = need.ceil().max(least);
    // A fixed one that can be laid out narrower (`narrowest`, a panel
    // whose columns stack) is as wide as the room has for it, down to
    // that, and scrolled only down: at its own width, scrolled across,
    // its second column was cut mid-word (Opus's look, 2026-11-28).
    let width = |down: bool| if fixed { design.0.min(room.0 - if down { bar } else { 0.0 }).max(narrowest.min(design.0)) } else { f32::from(viewport.width) };
    let mut down = need > room.1 + 0.5;
    // (The person's size is the person's: only a fixed one is held to
    // the room across, the bar down its side with it.)
    let across = fixed && width(down) + if down { bar } else { 0.0 } > room.0 + 0.5;
    if across {
        down |= need + bar > room.1 + 0.5;
    }
    let w = if across { room.0.floor() } else { width(down) + if down && fixed { bar } else { 0.0 } };
    let h = if down { room.1.floor() } else { need + if across { bar } else { 0.0 } };
    ((w, h), (down, across))
}

/// Whether a window that follows what it shows scrolls down and across
/// at `now`, the size it's held at (by the compositor: a tile, a zoom,
/// or less than it asked for, put beside Deskbar), as [`fitted`] has it
/// in the room it asks within: a fixed one narrower than its own width
/// is laid out at that and scrolled across, its bar along its foot
/// taking from the height (held, it had only scrolled down, its right
/// cut off: review, 2026-11-28).
fn scrolls_at(design: (f32, f32), narrowest: f32, fixed: bool, now: Size<Pixels>, need: f32) -> (bool, bool) {
    let bar = crate::widgets::SCROLL_BAR;
    let (w, h) = (f32::from(now.width), f32::from(now.height));
    let mut down = need > h + 0.5;
    let across = fixed && narrowest.min(design.0) + if down { bar } else { 0.0 } > w + 0.5;
    if across {
        down |= need + bar > h + 0.5;
    }
    (down, across)
}

/// Room left above a window's tab and around it on a screen.
const SCREEN_EDGES: f32 = 64.0;

/// Room across a screen that isn't a window's: Deskbar's column, the gap
/// beside it (app_server's `usable_area`, which windows stop at as on
/// BeOS) and a window's frame each side.
const BESIDE_DESKBAR: f32 = crate::metrics::DESKBAR_WIDTH + 4.0 + 2.0 * 5.0;

/// The height of the smallest screen Olo is made for (1280 × 800), in
/// points.
pub const SMALLEST_SCREEN_HEIGHT: f32 = 800.0;

/// Its width.
pub const SMALLEST_SCREEN_WIDTH: f32 = 1280.0;

/// The widest a window may be: the narrowest display's width in points,
/// less Deskbar's column and a window's frame ([`BESIDE_DESKBAR`]: windows
/// stop beside Deskbar, and at the room round the window alone a fixed
/// panel ran under it, its scroll bar out of reach, on a screen of 720
/// points: reviews, 2026-11-28), never under 320. On the smallest screen
/// Olo is made for, 1,094: every window made to fit it fits (the widest,
/// Preferences' Display, is 940).
pub fn screen_room_across() -> f32 {
    #[cfg(feature = "lint")]
    if let Some(room) = crate::lint::room_across_for_tests() {
        return room;
    }
    // (Before any display is known, the smallest screen's.)
    let narrowest = olo_interface::displays().iter().map(|d| f32::from(d.size.width)).reduce(f32::min).unwrap_or(SMALLEST_SCREEN_WIDTH);
    room_across(narrowest)
}

/// [`screen_room_across`] on a display `width` points wide.
fn room_across(width: f32) -> f32 {
    let beside = BESIDE_DESKBAR.max(SCREEN_EDGES);
    width.max(320.0 + beside) - beside
}

/// The height in points of the display `cx`'s window is on; until it's
/// been told which (a popup just opened: a submenu, opened from another
/// popup), the shortest display the app knows of, else the smallest
/// screen. Never unbounded: what's sized by it stays drawable (Tracker
/// went down on a popup 23,750 points tall, more than the GPU makes a
/// surface for, 2026-11-14; a menu's height went unlimited without its
/// display).
pub fn display_height(cx: &olo_interface::WindowCx) -> f32 {
    cx.display_bounds().map(|d| f32::from(d.size.height)).or_else(|| olo_interface::displays().iter().map(|d| f32::from(d.size.height)).reduce(f32::min)).unwrap_or(SMALLEST_SCREEN_HEIGHT)
}

/// The tallest a window may be: the shortest display's height in points,
/// less room for the window's tab and edges (never under 240).
pub fn screen_room() -> f32 {
    #[cfg(feature = "lint")]
    if let Some(room) = crate::lint::room_for_tests() {
        return room;
    }
    // (Before any display is known, the smallest screen's: never unbounded.)
    let shortest = olo_interface::displays().iter().map(|d| f32::from(d.size.height)).reduce(f32::min).unwrap_or(SMALLEST_SCREEN_HEIGHT);
    shortest.max(240.0 + SCREEN_EDGES) - SCREEN_EDGES
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
    /// An outstanding fit request can outlast the hold on presenting frames.
    pub(crate) fn awaiting_fit(&self) -> bool {
        self.fit.as_ref().is_some_and(|fit| fit.asked.is_some())
    }

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
            scrolls_at(fit.design, fit.narrowest, fit.fixed, now, need)
        } else {
            let ((w, h), mut scrolls) = fitted(fit.design, fit.narrowest, fit.fixed, now, need, (screen_room_across(), screen_room()));
            let off = (f32::from(now.width) - w).abs() > 0.5 || (f32::from(now.height) - h).abs() > 0.5;
            // (A size asked for and got is forgotten: sized another way
            // after (by its app: Quick Look to its next picture), it's
            // asked again. Kept, the window stayed past the room.)
            if !off {
                fit.asked = None;
            }
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
                // (Followed again once the wait is up: a resize refused, the
                // size never changing, sent nothing else to follow, and its
                // bars stayed as for the size asked for, what it shows out
                // of reach: review, 2026-11-28.)
                cx.after(RESIZE_WAIT + Duration::from_millis(2), FrameMessage::HoldLetGo);
            } else if off && fit.resizing.is_none() {
                // Given another size than asked, and that's that (fitted
                // beside Deskbar, onto a smaller display): scrolled as it
                // is, not as it would be at the size asked for.
                scrolls = scrolls_at(fit.design, fit.narrowest, fit.fixed, now, need);
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
            FrameMessage::Light(bounds) => {
                self.lit = Some((bounds, Instant::now()));
                cx.after(crate::spotlight::LIT, FrameMessage::Unlit);
                cx.invalidate();
            }
            // (Lit again since: that one's own end comes.)
            FrameMessage::Unlit => {
                if self.lit.is_some_and(|(_, since)| since.elapsed() >= crate::spotlight::LIT) {
                    self.lit = None;
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
        let shown = self.shown(cx);
        // Over all of it: a control lit to say "here" (`spotlight`).
        match self.lit {
            Some((bounds, since)) => shown.relative().child(crate::spotlight::ring(bounds, since)),
            None => shown,
        }
    }
}

impl Frame {
    /// The window's ground, its keys, and what it shows.
    fn shown(&mut self, cx: &mut Cx<Self>) -> Element {
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
            // dragged over cut text has no tip.) The window is sent the
            // moves across a cut text's edges (they're noted as it's
            // drawn) and every move while a button is held: no more.
            .on_drag_move(|e, cx| {
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
                // ⌘W and ⌘Q (the profile's Close and Quit) in a window
                // whose menus have neither (a dialog, an alert, a form):
                // closed as its tab's close button does, or the app quit,
                // the app's confirmations first, as BeOS's Alt+Q quits from
                // any of an app's windows. As its menus would match them,
                // by the profile (`known`): in a terminal app every window
                // gets the command key's chords with a Shift app_server
                // adds, and matched as a plain ⌘W neither did anything in
                // Rex's Edit Repository (Ben, 2026-11-23: "in a floating
                // window like the edit repository one i don't seem to be
                // able to press cmd+q or cmd+w but i can right click and
                // choose close").
                match crate::keys::closes_or_quits(&e.keystroke, olo_kit_base::keys::terminal_app()) {
                    Some(crate::keys::Leave::Close) => {
                        cx.request_close();
                        cx.stop_propagation();
                    }
                    // (Only where the app can be quit from its menus:
                    // `menu::app_quits`.)
                    Some(crate::keys::Leave::Quit) if crate::menu::app_quits() => {
                        olo_interface::quit();
                        cx.stop_propagation();
                    }
                    _ => {}
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
            // (Scrolled, laid out as tall as the window at least: what
            // fills the window, a picture's or a list's box, keeps the
            // room it had rather than none. Quick Look sized past the
            // screen's room showed an empty body over a scroll bar.
            // Across, at its own width.)
            let content = match fit.scrolls {
                (false, false) => laid,
                (down, across) => {
                    // (Less the bar along its foot, scrolled across: the
                    // window's whole height left that much past the
                    // view, scrolled to as nothing, or cut off.)
                    let tall = (cx.viewport_size().height - px(if across { crate::widgets::SCROLL_BAR } else { 0.0 })).max(px(0.0));
                    let inside = div().min_h(tall).flex().flex_col().child(laid);
                    // (Scrolled across: laid out as narrow as it can be.)
                    let inside = if across { inside.w(px(fit.narrowest.min(fit.design.0))).flex_shrink_0() } else { inside.w_full() };
                    #[cfg(feature = "lint")]
                    crate::lint::note_frame_scroll(cx.window_id(), &self.scroll);
                    crate::widgets::ScrollView::new(&self.scroll).border(false).vertical(down).horizontal(across).child(inside).into_element()
                }
            };
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
    let designed_width = options.spec().size.0;
    // (A fixed one's least width, before it's held to the room below.)
    let narrowest = options.min_size.map_or(designed_width, |m| m.0).min(designed_width);
    // (Not a layer's, a lock screen's or a full screen one's: the screen
    // or the app sizes those.)
    let fit_content = options.fit_content && options.layer.is_none() && options.lock_output.is_none() && !options.fullscreen;
    // (One that follows what it shows opens no taller than the screen;
    // a fixed one no wider than its room either, laid out at its own
    // width and scrolled across (`fitted`): it opened at its own, under
    // Deskbar, and was sized down only after.)
    if fit_content {
        options.size.1 = options.size.1.min(screen_room());
        if options.fixed {
            let room = screen_room_across().floor();
            options.size.0 = options.size.0.min(room);
            if let Some(min) = options.min_size.as_mut() {
                min.0 = min.0.min(room);
            }
        }
    }
    let spec = options.spec();
    let design = (designed_width, designed_height);
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
            fit: fit_content.then(|| Fit { design, narrowest, fixed, need: None, asked: None, resizing: None, scrolls: (false, false), first }),
            lit: None,
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
    // (A question, `--help`, never opens a window.)
    olo_kit_base::instance::no_window_for_a_question(app_id, std::env::args().nth(1).as_deref());
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
    // (Two libraries warn of what's ordinary: usvg of what it doesn't
    // draw, and the MP4 reader behind a video's sound of each table it
    // has no use for, "ignoring stss atom", at every video opened.)
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,usvg=error,symphonia_format_isomp4=error,olo=info"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::SCROLL_BAR;

    fn now(w: f32, h: f32) -> Size<Pixels> {
        size(px(w), px(h))
    }

    /// A window that follows what it shows: as big as it needs where the
    /// screen has room; past the room down, as tall as the room and
    /// widened by its bar; past it across (a fixed panel wider than the
    /// screen), as wide as the room and scrolled across too.
    #[test]
    fn a_fitted_window_stays_in_the_screens_room() {
        // The smallest screen, beside Deskbar: Display (950 × 551) fits,
        // as it is.
        let room = (1094.0, 736.0);
        assert_eq!(fitted((950.0, 400.0), 950.0, true, now(950.0, 400.0), 551.0, room), ((950.0, 551.0), (false, false)));
        // Too tall: the room's height, the bar beside it.
        assert_eq!(fitted((950.0, 400.0), 950.0, true, now(950.0, 551.0), 900.0, room), ((950.0 + SCROLL_BAR, 736.0), (true, false)));
        // 1440 × 900 at 2× (720 × 450): too wide and too tall, both ways.
        let small = (656.0, 386.0);
        assert_eq!(fitted((950.0, 400.0), 950.0, true, now(950.0, 551.0), 551.0, small), ((656.0, 386.0), (true, true)));
        // Too wide only, but the bar across takes it past the room down.
        assert_eq!(fitted((700.0, 300.0), 700.0, true, now(700.0, 380.0), 380.0, small), ((656.0, 386.0), (true, true)));
        // Too wide, room down for the bar across: as tall as it needs and the bar.
        assert_eq!(fitted((700.0, 300.0), 700.0, true, now(700.0, 300.0), 300.0, small), ((656.0, 300.0 + SCROLL_BAR), (false, true)));
        // Its bar down would take it past the room across.
        assert_eq!(fitted((1088.0, 400.0), 1088.0, true, now(1088.0, 400.0), 900.0, room), ((1094.0, 736.0), (true, true)));
        // One the person sizes: their width, never scrolled across.
        assert_eq!(fitted((950.0, 400.0), 950.0, false, now(900.0, 500.0), 551.0, small), ((900.0, 386.0), (true, false)));
    }

    /// A fixed window that can be laid out narrower (its columns stacked:
    /// `narrowest`) is as wide as the room has for it, scrolled only
    /// down; narrower than that room, it is laid out at its narrowest and
    /// scrolled across; as wide as its own where there's room.
    #[test]
    fn a_window_that_can_narrow_scrolls_only_down() {
        let small = (534.0, 386.0);
        // Display (940, its columns stacked at 480): 534 across, its bar
        // down its side, laid out at 520.
        assert_eq!(fitted((940.0, 520.0), 480.0, true, now(940.0, 520.0), 900.0, small), ((534.0, 386.0), (true, false)));
        assert_eq!(fitted((940.0, 300.0), 480.0, true, now(940.0, 300.0), 300.0, small), ((534.0, 300.0), (false, false)));
        // A room narrower than it can be: at its narrowest, across.
        assert_eq!(fitted((940.0, 300.0), 480.0, true, now(940.0, 300.0), 300.0, (400.0, 386.0)), ((400.0, 300.0 + SCROLL_BAR), (false, true)));
        // Room for it: its own width.
        assert_eq!(fitted((940.0, 520.0), 480.0, true, now(940.0, 520.0), 551.0, (1094.0, 736.0)), ((940.0, 551.0), (false, false)));
        // Held at a width it can be laid out at: down only.
        assert_eq!(scrolls_at((940.0, 520.0), 480.0, true, now(534.0, 386.0), 900.0), (true, false));
    }

    /// The room across is beside Deskbar: on 1440 × 900 at 2× (720
    /// points), 534, as app_server's `usable_area` leaves a window's
    /// content there (Deskbar's 172, its gap of 4, the frame's 5 each
    /// side); on the smallest screen, 1,094, Display's 940 fitting whole.
    #[test]
    fn the_room_across_is_beside_deskbar() {
        assert_eq!(room_across(720.0), 534.0);
        assert_eq!(room_across(1280.0), 1094.0);
        assert!(room_across(1280.0) >= 950.0);
        // Never under 320.
        assert_eq!(room_across(200.0), 320.0);
    }

    /// Held at a size (a tile, a zoom, given less than asked): scrolled
    /// as it is there, across too where a fixed window is narrower than
    /// its own width, the bar across taking from the height.
    #[test]
    fn a_held_window_scrolls_as_it_is() {
        // Put beside Deskbar on a 720-point screen: across, and down for
        // the bar along its foot.
        assert_eq!(scrolls_at((940.0, 520.0), 940.0, true, now(534.0, 386.0), 380.0), (true, true));
        // Room down for the bar: across only.
        assert_eq!(scrolls_at((940.0, 300.0), 940.0, true, now(534.0, 386.0), 300.0), (false, true));
        // As wide as it is: down only, as before.
        assert_eq!(scrolls_at((520.0, 300.0), 520.0, true, now(534.0, 300.0), 386.0), (true, false));
        // Its bar down takes it past the width held.
        assert_eq!(scrolls_at((530.0, 300.0), 530.0, true, now(534.0, 300.0), 386.0), (true, true));
        // One the person sizes is never scrolled across.
        assert_eq!(scrolls_at((940.0, 520.0), 940.0, false, now(534.0, 386.0), 380.0), (false, false));
        // Nothing to scroll.
        assert_eq!(scrolls_at((520.0, 300.0), 520.0, true, now(534.0, 386.0), 300.0), (false, false));
    }
}
