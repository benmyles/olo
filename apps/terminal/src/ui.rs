//! Terminal's windows on GPUI. Terminal is one entity holding every window
//! (`Terminal::windows`, by a `WindowId` of its own); each window's view
//! (`Shell`) knows only which it is and has Terminal draw it, so the code
//! that decides what a window does (main.rs) works on Terminal alone, and
//! the window operations it asks for (open, close, redraw, activate,
//! resize, title) are `WinOps` on its context, carried out once the update
//! that asked is over.
//!
//! A terminal window is drawn here: the menu bar, the tab strip (along
//! the top or down the side), the screen (or, Tile All Tabs, a pane for
//! each tab with its head, rows split by draggable dividers), the visual
//! bell. Keys go to the shell unless they're one of Terminal's menu
//! shortcuts; text typed (and an input method's) comes through
//! `EntityInputHandler`.

use std::{collections::HashMap, ops::Range, path::PathBuf, rc::Rc, time::Duration};

use olo_ui::{
    Frame,
    gpui::{
        self, AnyElement, App, AppContext as _, Bounds, Context, CursorStyle, DispatchPhase, ElementInputHandler, Entity, EntityId, EntityInputHandler, FocusHandle, Focusable, FontWeight, Hitbox,
        HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NoAction, ParentElement as _,
        Pixels, Render, ScrollWheelEvent, SharedString, Styled as _, UTF16Selection, WeakEntity, Window, WindowHandle, canvas, div, point, px, size,
    },
    icons,
    menu::{self, Menu, MenuBar},
    metrics as m, style, theme,
    widgets::{self, Button, CheckBox, StripAction, StripGroup, StripTab, TabStrip, TextControl, TextField, TextFieldEvent},
};

use crate::{
    Terminal, Win,
    agent::State,
    groups, repos, view,
    view::{Metrics, Mods},
};

/// One of Terminal's windows, by its own count (never reused).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WindowId(u64);

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
    /// What app_server knows the window by when Terminal comes back (the
    /// kit's `restore_key`).
    pub restore_key: Option<String>,
}

impl WindowOptions {
    pub fn new(title: impl Into<String>, w: u32, h: u32) -> WindowOptions {
        WindowOptions { title: title.into(), size: (w, h), min_size: None, max_size: None, under_pointer: false, alert: None, restore_key: None }
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
}

/// A window as GPUI has it.
struct Handle {
    window: WindowHandle<Frame>,
    shell: Entity<Shell>,
    /// Its content size as last drawn.
    size: (u32, u32),
    /// On the workspace showing (as last drawn; until then, yes).
    here: bool,
}

/// Terminal's windows (a global: window operations are asked for while
/// Terminal is being updated).
#[derive(Default)]
struct Wins {
    map: HashMap<WindowId, Handle>,
    next: u64,
}

impl gpui::Global for Wins {}

/// Terminal's context: its entity's.
pub type Cx<'a> = Context<'a, Terminal>;

/// Window operations, done once the update asking for them is over (a
/// window can't be changed while it's handling the event that asked).
pub trait WinOps {
    fn open_window(&mut self, options: WindowOptions) -> WindowId;
    fn close_window(&mut self, id: WindowId);
    fn redraw(&mut self, id: WindowId);
    fn redraw_all(&mut self);
    fn activate(&mut self, id: WindowId);
    fn activate_with(&mut self, id: WindowId, token: Option<String>);
    fn resize(&mut self, id: WindowId, size: (u32, u32));
    fn window_size(&self, id: WindowId) -> Option<(u32, u32)>;
    /// Whether it's on the workspace showing (else it draws nothing until
    /// it is, and would jump then if resized now).
    fn is_shown(&self, id: WindowId) -> bool;
    fn on_this_workspace(&self, id: WindowId) -> bool;
    fn copy(&mut self, text: String);
    fn begin_move(&mut self, id: WindowId);
    fn proxy(&self) -> olo_kit_base::Proxy;
    fn set_title(&mut self, id: WindowId, title: String);
    fn set_path(&mut self, id: WindowId, path: Option<PathBuf>);
    /// Does `f` with window `id`, after this update.
    fn with_window(&mut self, id: WindowId, f: impl FnOnce(&mut Frame, &mut Window, &mut Context<Frame>) + 'static);
}

impl WinOps for Context<'_, Terminal> {
    fn open_window(&mut self, o: WindowOptions) -> WindowId {
        let wins = self.default_global::<Wins>();
        wins.next += 1;
        let id = WindowId(wins.next);
        let (w, h) = match &o.alert {
            Some(message) => {
                let w = widgets::alert_width(self, &["Cancel", "Close Anyway"]);
                (w, widgets::alert_height(self, message, w))
            }
            None => (o.size.0 as f32, o.size.1 as f32),
        };
        let mut options = if o.alert.is_some() { olo_ui::WindowOptions::alert(o.title.clone(), w, h) } else { olo_ui::WindowOptions::new(o.title.clone(), w, h) };
        options.panel = o.alert.is_some() || !matches!(o.title.as_str(), "Terminal");
        options.min_size = o.min_size.map(|(w, h)| (w as f32, h as f32));
        options.max_size = o.max_size.map(|(w, h)| (w as f32, h as f32));
        if o.max_size.is_some_and(|max| Some(max) == o.min_size) {
            options.fixed = true;
        }
        options.under_pointer = o.under_pointer;
        options.restore_key = o.restore_key.clone();
        let me = self.weak_entity();
        let made: Rc<std::cell::RefCell<Option<Entity<Shell>>>> = Rc::default();
        let keep = made.clone();
        let window = olo_ui::open_window(self, options, move |window, cx| {
            let shell = cx.new(|cx| Shell { me: me.clone(), id, focus: cx.focus_handle() });
            let focus = shell.read(cx).focus.clone();
            window.focus(&focus);
            // Its close box asks Terminal, which may ask "Close anyway?".
            let closing = me.clone();
            window.on_window_should_close(cx, move |_, cx| {
                let _ = closing.update(cx, |t, cx| t.close_requested(cx, id));
                false
            });
            // What's dropped on a screen (accepted in its paint).
            let dropping = me.clone();
            window.on_dropped(move |d, _, cx| {
                let paths = if d.mime == "text/uri-list" { d.paths() } else { Vec::new() };
                let text = String::from_utf8_lossy(&d.data).into_owned();
                let _ = dropping.update(cx, |t, cx| t.dropped(cx, id, &paths, Some(&text)));
            });
            *keep.borrow_mut() = Some(shell.clone());
            shell
        });
        if let (Some(window), Some(shell)) = (window, made.borrow_mut().take()) {
            let size = (w as u32, h as u32);
            self.default_global::<Wins>().map.insert(id, Handle { window, shell, size, here: true });
        }
        id
    }

    fn close_window(&mut self, id: WindowId) {
        if let Some(h) = self.default_global::<Wins>().map.remove(&id) {
            self.defer(move |cx| {
                let _ = h.window.update(cx, |_, window, _| window.remove_window());
            });
        }
    }

    fn redraw(&mut self, id: WindowId) {
        if let Some(e) = self.try_global::<Wins>().and_then(|w| w.map.get(&id)).map(|h| h.shell.entity_id()) {
            App::notify(self, e);
        }
    }

    fn redraw_all(&mut self) {
        let all: Vec<EntityId> = self.try_global::<Wins>().map(|w| w.map.values().map(|h| h.shell.entity_id()).collect()).unwrap_or_default();
        for e in all {
            App::notify(self, e);
        }
    }

    fn activate(&mut self, id: WindowId) {
        self.with_window(id, |_, window, _| window.activate_window());
    }

    fn activate_with(&mut self, id: WindowId, token: Option<String>) {
        self.with_window(id, move |_, window, _| match &token {
            Some(t) => window.activate_window_with_token(t),
            None => window.activate_window(),
        });
    }

    fn resize(&mut self, id: WindowId, (w, h): (u32, u32)) {
        if let Some(handle) = self.default_global::<Wins>().map.get_mut(&id) {
            handle.size = (w, h);
        }
        self.with_window(id, move |_, window, _| window.resize(size(px(w as f32), px(h as f32))));
    }

    fn window_size(&self, id: WindowId) -> Option<(u32, u32)> {
        window_size(self, id)
    }

    fn is_shown(&self, id: WindowId) -> bool {
        self.on_this_workspace(id)
    }

    fn on_this_workspace(&self, id: WindowId) -> bool {
        self.try_global::<Wins>().and_then(|w| w.map.get(&id)).is_some_and(|h| h.here)
    }

    fn copy(&mut self, text: String) {
        self.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    fn begin_move(&mut self, id: WindowId) {
        self.with_window(id, |_, window, _| olo_ui::begin_move(window));
    }

    fn proxy(&self) -> olo_kit_base::Proxy {
        olo_ui::proxy(self)
    }

    fn set_title(&mut self, id: WindowId, title: String) {
        self.with_window(id, move |frame, window, _| frame.set_title(title, window));
    }

    fn set_path(&mut self, id: WindowId, path: Option<PathBuf>) {
        self.with_window(id, move |_, window, cx| olo_ui::agent::set_path(window, cx, path));
    }

    fn with_window(&mut self, id: WindowId, f: impl FnOnce(&mut Frame, &mut Window, &mut Context<Frame>) + 'static) {
        let Some(handle) = self.try_global::<Wins>().and_then(|w| w.map.get(&id)).map(|h| h.window) else { return };
        self.defer(move |cx| {
            let _ = handle.update(cx, f);
        });
    }
}

/// Window `id`'s content size as last drawn.
pub fn window_size(cx: &App, id: WindowId) -> Option<(u32, u32)> {
    cx.try_global::<Wins>()?.map.get(&id).map(|h| h.size)
}

/// Which of Terminal's windows a GPUI window is, for a window let go on
/// another's strip.
pub fn window_of(cx: &App, handle: gpui::AnyWindowHandle) -> Option<WindowId> {
    cx.try_global::<Wins>()?.map.iter().find(|(_, h)| gpui::AnyWindowHandle::from(h.window) == handle).map(|(id, _)| *id)
}

/// Terminal's keys: Tab and ⇧Tab are the shell's in a terminal (not the
/// window's keyboard navigation).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("tab", NoAction, Some(SCREEN)), KeyBinding::new("shift-tab", NoAction, Some(SCREEN))]);
}

/// The key context of a terminal window.
const SCREEN: &str = "OloTerminal";

/// A window of Terminal's: which one it is; Terminal draws it.
pub struct Shell {
    me: WeakEntity<Terminal>,
    id: WindowId,
    focus: FocusHandle,
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (id, focus, view) = (self.id, self.focus.clone(), cx.entity());
        if let Some(handle) = cx.default_global::<Wins>().map.get_mut(&id) {
            let s = window.viewport_size();
            handle.size = (f32::from(s.width).round() as u32, f32::from(s.height).round() as u32);
            handle.here = olo_ui::workspaces::here(window).unwrap_or(true);
        }
        self.me.update(cx, |t, cx| t.render(id, focus, view, window, cx)).unwrap_or_else(|_| div().into_any_element())
    }
}

/// Text typed, and an input method's, for the shell showing.
impl EntityInputHandler for Shell {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<Range<usize>> {
        let n = self.me.upgrade()?.read(cx).preedit(self.id).map(|p| p.encode_utf16().count())?;
        (n > 0).then_some(0..n)
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let id = self.id;
        let _ = self.me.update(cx, |t, cx| t.set_preedit(cx, id, None));
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let (id, text) = (self.id, text.to_owned());
        let _ = self.me.update(cx, |t, cx| {
            t.set_preedit(cx, id, None);
            t.typed_text(cx, id, &text);
        });
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let (id, text) = (self.id, text.to_owned());
        let _ = self.me.update(cx, |t, cx| t.set_preedit(cx, id, Some(text)));
    }

    fn bounds_for_range(&mut self, _: Range<usize>, element: Bounds<Pixels>, _: &mut Window, cx: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        // The cursor's cell, for the input method's window.
        let t = self.me.upgrade()?;
        t.read(cx).cursor_bounds(self.id).or(Some(element))
    }

    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
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
    let (a, b) = (gpui::Rgba::from(a), gpui::Rgba::from(b));
    Hsla::from(gpui::Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t })
}

/// A tab's icon: Terminal's, with a badge at its corner: a turning arc
/// while an agent works, an orange "!" when it needs an answer, a green
/// dot when it's done and no one has looked, a blue dot for an unseen
/// bell; and at its bottom corner, while Keep It Going is on, a blue
/// disc with a "go on" triangle.
pub fn tab_icon(rang: bool, agent: Option<State>, attention: bool, keep_going: bool) -> widgets::strip::PaintFn {
    Rc::new(move |window: &mut Window, cx: &mut App, r: Bounds<Pixels>| {
        icons::paint(window, r, "terminal");
        let p = theme::palette(cx);
        if keep_going {
            // A disc in the accent colour with a "go on" triangle, as the
            // question's is orange with a "!".
            let c = point(r.right() - px(1.5), r.bottom() - px(3.5));
            let disc = |window: &mut Window, radius: f32, color: Hsla| {
                let b = Bounds { origin: point(c.x - px(radius), c.y - px(radius)), size: size(px(2.0 * radius), px(2.0 * radius)) };
                window.paint_quad(gpui::fill(b, color).corner_radii(px(radius)));
            };
            disc(window, 5.5, style::c(p.panel_background));
            let accent = p.keyboard_navigation;
            disc(window, 4.5, style::c(accent));
            let mut path = gpui::PathBuilder::fill();
            path.move_to(point(c.x - px(1.25), c.y - px(2.25)));
            path.line_to(point(c.x + px(2.25), c.y));
            path.line_to(point(c.x - px(1.25), c.y + px(2.25)));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, style::ink_on(accent));
            }
        }
        let at = point(r.right() - px(1.5), r.top() + px(3.5));
        let back = style::c(p.panel_background);
        let disc = |window: &mut Window, radius: f32, color: Hsla| {
            let b = Bounds { origin: point(at.x - px(radius), at.y - px(radius)), size: size(px(2.0 * radius), px(2.0 * radius)) };
            window.paint_quad(gpui::fill(b, color).corner_radii(px(radius)));
        };
        match agent {
            Some(State::Working) => {
                disc(window, 5.0, back);
                // A quarter turn every … : by the clock, so each frame
                // drawn shows where it is now.
                let turn = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f32() % 1000.0) * 5.0;
                let pts: Vec<(gpui::Point<Pixels>, gpui::Point<Pixels>)> = (0..16)
                    .map(|k| {
                        let a = |k: i32| turn + k as f32 * (1.5 * std::f32::consts::PI / 16.0);
                        let p = |a: f32| point(at.x + px(3.0 * a.cos()), at.y + px(3.0 * a.sin()));
                        (p(a(k)), p(a(k + 1)))
                    })
                    .collect();
                window.paint_segments(&pts, px(1.5), true, style::c(p.keyboard_navigation));
            }
            Some(State::Blocked) => {
                disc(window, 5.5, back);
                let orange = p.hue(olo_theme::Hue::Orange);
                disc(window, 4.5, style::c(orange));
                let white = style::ink_on(orange);
                window.paint_quad(gpui::fill(Bounds::centered_at(point(at.x, at.y - px(1.0)), size(px(1.5), px(4.0))), white));
                window.paint_quad(gpui::fill(Bounds::centered_at(point(at.x, at.y + px(2.25)), size(px(1.5), px(1.5))), white));
            }
            Some(State::Idle) if attention => {
                disc(window, 4.0, back);
                disc(window, 3.0, style::c(p.success));
            }
            _ if rang => {
                disc(window, 3.5, back);
                disc(window, 2.5, style::c(p.keyboard_navigation));
            }
            _ => {}
        }
    })
}

/// A pane's head in Tile All Tabs: its tab's icon and title (cut short
/// with "…"), the one typed into lit as the strip's selected tab is, the
/// others quiet; its group's chip at its right, so a group's panes show
/// together; a line around each pane keeps them apart.
fn pane_head(window: &mut Window, cx: &mut App, pane: Bounds<Pixels>, height: f32, title: &str, icon: &widgets::strip::PaintFn, active: bool, group: Option<(SharedString, Hsla)>) {
    let p = theme::palette(cx);
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let bg = p.panel_background;
    let band = style::shade(&p, bg, 1.06);
    let edge = if dark { lerp(style::c(bg), gpui::white(), 0.14) } else { style::shade(&p, bg, 1.22) };
    let head = Bounds { origin: pane.origin, size: size(pane.size.width, px(height)) };
    let ground = if active { style::c(bg) } else { band };
    style::fill_rect(window, head, ground);
    style::hline_above(window, head.left(), head.right(), head.bottom(), edge);
    // Lines between panes (the window's frame does the outer edges).
    let line = style::hairline(window);
    style::vline(window, pane.right(), pane.top(), pane.bottom() + line, edge);
    style::hline(window, pane.left(), pane.right() + line, pane.bottom(), edge);
    // Centred on the band above the line under it, as near as the
    // device pixels allow.
    let cy = (head.top() + head.bottom() - line) / 2.0;
    // The icon over the pane's text, as a tab's is over the menu bar's
    // first title.
    let at = style::snap_centred(window, point(head.left() + px(view::PAD.0 + 8.0), cy), 16.0, 16.0);
    icon(window, cx, at);
    let x = at.right() + px(m::MARK_GAP);
    let mut end = head.right() - px(m::SPACING);
    if let Some((name, color)) = group {
        let w = widgets::strip::chip_width(window, &name);
        let r = style::snap_centred(window, point(end - px(w / 2.0), cy), w, widgets::strip::CHIP_HEIGHT);
        widgets::strip::chip(window, cx, r, &name, color, true, false);
        end = r.left() - px(m::SPACING);
    }
    let font = style::ui_font(if active { FontWeight::MEDIUM } else { FontWeight::NORMAL });
    let text = style::c(p.panel_text);
    let color = if active { text } else { lerp(text, ground, 0.32) };
    style::label_fit(window, cx, x, cy, style::Align::Left, title.to_owned(), style::PLAIN_SIZE, font, color, end - x);
}

/// What a mouse event on a screen is, for Terminal.
pub enum PaneEvent {
    Down(MouseDownEvent),
    Move(MouseMoveEvent),
    Up(MouseUpEvent),
    Wheel(ScrollWheelEvent),
}

/// What a terminal window lays out each frame.
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

impl Terminal {
    /// Draws window `id`.
    fn render(&mut self, id: WindowId, focus: FocusHandle, view: Entity<Shell>, window: &mut Window, cx: &mut Cx) -> AnyElement {
        match self.windows.get(&id) {
            Some(Win::Term(_)) => self.render_term(id, focus, view, window, cx),
            Some(Win::ConfirmClose { message, .. }) => self.render_alert(id, "terminal", message.clone(), &["Cancel", "Close"], focus, cx),
            Some(Win::ConfirmStartOver { .. }) => self.render_alert(id, "terminal", crate::START_OVER.to_owned(), &["Cancel", "Start Over"], focus, cx),
            Some(Win::ConfirmRemove { message, .. }) => self.render_alert(id, "warning", message.clone(), &["Cancel", "Remove"], focus, cx),
            Some(Win::Failed { message }) => self.render_alert(id, "warning", message.clone(), &["OK"], focus, cx),
            Some(Win::Find { .. }) => self.render_find(id, focus, window, cx),
            Some(Win::NameGroup { .. }) => self.render_name_group(id, focus, window, cx),
            Some(Win::Daemon(_)) => self.render_daemon(id, window, cx),
            Some(Win::Sudo(_)) => self.render_sudo(id, focus, window, cx),
            Some(Win::Worktree(_)) => self.render_worktree(id, focus, window, cx),
            None => div().into_any_element(),
        }
    }

    /// An alert's window: `message` beside `icon`, `buttons` (the last the
    /// default: Return; Escape the first).
    fn render_alert(&mut self, id: WindowId, icon: &str, message: String, buttons: &[&str], focus: FocusHandle, cx: &mut Cx) -> AnyElement {
        let n = buttons.len();
        let mut row = widgets::button_row();
        for (i, label) in buttons.iter().enumerate() {
            let choose = cx.listener(move |t, _: &(), _, cx| t.alert_chosen(cx, id, i));
            row = row.child(Button::new(label.to_string()).default(i + 1 == n).on_click(move |window, cx| choose(&(), window, cx)));
        }
        div()
            .size_full()
            .key_context(widgets::alert::CONTEXT)
            .track_focus(&focus)
            .on_action(cx.listener(move |t, _: &widgets::alert::Default, _, cx| t.alert_chosen(cx, id, n.saturating_sub(1))))
            .on_action(cx.listener(move |t, _: &widgets::alert::Cancel, _, cx| {
                if n > 1 {
                    t.alert_chosen(cx, id, 0);
                }
            }))
            .child(widgets::AlertFrame::new(icon.to_owned()).child(widgets::paragraph(message)).child(div().flex_1()).child(row))
            .into_any_element()
    }

    /// The cell metrics at the window's scale, made again when the text
    /// size or the scale changes (cells snapped to another scale's pixels
    /// can still be whole ones here and put the rows between pixels).
    pub(crate) fn metrics_for(&mut self, window: &Window) -> Metrics {
        let size = self.settings.text_size;
        match self.metrics {
            Some(m) if m.size == size && m.scale == window.scale_factor() => m,
            _ => {
                let m = Metrics::new(window, size);
                self.metrics = Some(m);
                m
            }
        }
    }

    /// Where a terminal window's parts go.
    fn lay_out(&self, id: WindowId, window: &Window, metrics: &Metrics) -> Option<Layout> {
        let Some(Win::Term(t)) = self.windows.get(&id) else { return None };
        let vs = window.viewport_size();
        let top = style::snap_coord(window, px(menu::BAR_HEIGHT));
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
        let scale = window.scale_factor();
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

    fn render_term(&mut self, id: WindowId, focus: FocusHandle, view: Entity<Shell>, window: &mut Window, cx: &mut Cx) -> AnyElement {
        // The desktop's look, when it's another.
        let look = theme::palette(cx);
        if !std::sync::Arc::ptr_eq(&look, &self.look) {
            self.follow(&look);
        }
        let metrics = self.metrics_for(window);
        let scheme = self.scheme();
        let focused = window.is_window_active() && focus.is_focused(window);
        if focused {
            self.front = Some(id);
        }
        // (Whatever changed shows here: kept on disk soon.)
        self.remember_soon(cx);
        let Some(layout) = self.lay_out(id, window, &metrics) else { return div().into_any_element() };
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return div().into_any_element() };
        if focused != t.focused {
            t.focused = focused;
            t.tab().report_focus(focused);
        }
        if focused {
            t.tab_mut().attention = false;
        }
        // (Every pane drawn has taken its wake-up, the tiles too.)
        for &(i, _, _) in &layout.panes {
            t.tabs[i].session.woke();
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
                if turned && session.shared.reports.load(std::sync::atomic::Ordering::Relaxed) {
                    session.write(crate::io::light_or_dark(scheme.dark()).to_vec());
                }
            }
        }
        // The window's size for its grid, once the cells are known.
        if let Some((cols, rows)) = t.snap_to.take().filter(|_| !t.tiling()) {
            let (w, h) = metrics.content_size(cols, rows);
            let (cw, ch) = crate::chrome(&self.settings);
            let want = (w as u32 + cw, h as u32 + ch);
            if cx.window_size(id) != Some(want) {
                cx.resize(id, want);
            }
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return div().into_any_element() };
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
        if t.tabs.iter().any(|x| matches!(x.agent, Some((_, State::Working)))) && !t.spinning {
            // The spinner turns, ten times a second.
            t.spinning = true;
            cx.spawn_in(window, async move |me, cx| {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let _ = me.update(cx, |t, cx| {
                    if let Some(Win::Term(t)) = t.windows.get_mut(&id) {
                        t.spinning = false;
                    }
                    cx.redraw(id);
                });
            })
            .detach();
        }

        // The strip.
        let side = self.settings.side_tabs;
        let Some(Win::Term(t)) = self.windows.get(&id) else { return div().into_any_element() };
        let strip_groups: Vec<StripGroup> = t.groups.iter().map(|g| StripGroup { name: g.name.clone().into(), color: groups::color(g.color, &self.look) }).collect();
        let tabs: Vec<StripTab> = t
            .tabs
            .iter()
            .map(|tab| {
                let pr = repos::pr_of(&self.prs, tab);
                let status = match (tab.status(), pr.map(|p| p.describe())) {
                    (Some(a), Some(b)) => Some(format!("{a}; {b}")),
                    (a, b) => a.or(b),
                };
                StripTab {
                    label: tab.title.clone().into(),
                    icon: Some(tab_icon(tab.rang, tab.agent.map(|(_, s)| s), tab.attention, tab.keep_going.is_some())),
                    status: status.map(Into::into),
                    badge: pr.map(repos::pr_badge),
                    group: tab.group,
                    faded: tab.hidden_tiled.then(|| "hidden when tiled".into()),
                }
            })
            .collect();
        let mut strip = TabStrip::new(format!("terminal-tabs-{}", id.0), tabs, t.active).groups(strip_groups);
        if t.tiled && t.tabs.len() > 1 {
            // Tile All Tabs: a group's chip chooses which are tiled; each
            // pane has its own head.
            strip = strip.group_selection((0..t.groups.len()).map(|g| t.tile_group_shown(g)).collect());
        }
        if side {
            strip = strip.side();
        }
        let act = cx.listener(move |t, a: &StripAction, window, cx| t.strip_action(id, *a, window, cx));
        let strip = strip.on_action(move |a, window, cx| act(&a, window, cx));
        let strip = {
            let action = strip;
            div().absolute().left(layout.strip.left()).top(layout.strip.top()).w(layout.strip.size.width).h(layout.strip.size.height).child(action)
        };

        // The panes: each a screen, drawn by view.rs.
        // Behind everything, the screen's background: the device pixel a
        // fractional scale leaves past the last pane's edge (at 1.5×, the
        // window's bottom row) is the terminal's colour, not black.
        let behind = view::screen_colors(&t.tab().session, &scheme).0;
        let mut root = div().size_full().relative().bg(behind).key_context(SCREEN).track_focus(&focus);
        root = root.on_key_down(cx.listener(move |t, e: &KeyDownEvent, window, cx| t.key_down(id, e, window, cx))).on_key_up(cx.listener(move |t, e: &KeyUpEvent, _, cx| t.key_up(id, e, cx)));
        let p = theme::palette(cx);
        let me = cx.weak_entity();
        let bell = t.tab().bell.map(|at| at.elapsed().as_secs_f32()).filter(|a| *a < 0.15);
        let flash_color = {
            let [r, g, b, _] = self.look.panel_text;
            gpui::Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
        };
        let group_chips: Vec<(SharedString, Hsla)> = t.groups.iter().map(|g| (SharedString::from(g.name.clone()), groups::color(g.color, &self.look))).collect();
        let preedit = t.preedit.clone();
        for &(i, pane, screen) in &layout.panes {
            let tab = &t.tabs[i];
            let shell = tab.session.id;
            let active = i == t.active;
            let source = tab.session.source();
            let pane_cache = tab.pane.clone();
            let hover = tab.mouse.hover.clone();
            let scheme = scheme.clone();
            let head =
                several.then(|| (tab.title.clone(), tab_icon(tab.rang, tab.agent.map(|(_, s)| s), tab.attention, tab.keep_going.is_some()), tab.group.and_then(|g| group_chips.get(g).cloned())));
            let dim = (several && !active).then(|| view::screen_colors(&tab.session, &scheme).0);
            let preedit = preedit.clone().filter(|_| active);
            let input = active.then(|| (view.clone(), focus.clone()));
            let me = me.clone();
            let el = canvas(
                move |_, window, _| window.insert_hitbox(screen, HitboxBehavior::Normal),
                move |_, hitbox: Hitbox, window, cx| {
                    if let Some((title, icon, group)) = &head {
                        pane_head(window, cx, pane, pane_head_height(side), title, icon, active, group.clone());
                        // Panes after the first are where dividing the
                        // window puts them: level with the strip's tabs
                        // only by chance.
                        olo_ui::review::own_grid(window, pane);
                    }
                    // Its lines are a cell apart, not on the window's grid.
                    olo_ui::review::own_grid(window, screen);
                    let paint = view::Paint { source: &source, scheme: &scheme, metrics, focused: focused && active, hover: hover.as_ref(), preedit: preedit.as_deref() };
                    window.with_content_mask(Some(gpui::ContentMask { bounds: screen }), |window| view::paint(window, cx, screen, paint, &pane_cache));
                    if let Some(back) = dim {
                        // The panes not typed into, dimmed a little.
                        window.paint_quad(gpui::fill(screen, back.opacity(0.3)));
                    }
                    if let Some((view, focus)) = &input {
                        window.handle_input(focus, ElementInputHandler::new(screen, view.clone()), cx);
                        // Files dropped on it are typed as their paths,
                        // text as if pasted.
                        if hitbox.is_hovered(window)
                            && let Some(o) = window.drop_offer().filter(|o| o.copy)
                        {
                            let mime = if o.types.iter().any(|t| t == "text/uri-list") {
                                Some("text/uri-list")
                            } else {
                                ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"].into_iter().find(|t| o.types.iter().any(|x| x == t))
                            };
                            if let Some(mime) = mime {
                                window.accept_drop("screen", mime, gpui::DragAction::Copy);
                                let p = theme::palette(cx);
                                window.paint_quad(gpui::outline(style::inset(window, screen, 1.0), style::c(p.keyboard_navigation), gpui::BorderStyle::Solid).border_widths(px(2.0)));
                            }
                        }
                    }
                    let link = hover.as_ref().is_some_and(|h| h.target.is_some());
                    window.set_cursor_style(if link { CursorStyle::PointingHand } else { CursorStyle::IBeam }, &hitbox);
                    // The pointer: to Terminal, with which shell it's over.
                    let (m1, h1) = (me.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble && h1.is_hovered(window) {
                            let _ = m1.update(cx, |t, cx| t.pane_event(id, shell, screen, PaneEvent::Down(e.clone()), window, cx));
                        }
                    });
                    let m2 = me.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = m2.update(cx, |t, cx| t.pane_event(id, shell, screen, PaneEvent::Move(e.clone()), window, cx));
                        }
                    });
                    let m3 = me.clone();
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = m3.update(cx, |t, cx| t.pane_event(id, shell, screen, PaneEvent::Up(e.clone()), window, cx));
                        }
                    });
                    let (m4, h4) = (me.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble && h4.is_hovered(window) {
                            let _ = m4.update(cx, |t, cx| t.pane_event(id, shell, screen, PaneEvent::Wheel(e.clone()), window, cx));
                        }
                    });
                },
            )
            .absolute()
            .inset_0();
            root = root.child(el);
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
            let bar = widgets::SlimScrollBar::new(("terminal-scroll", shell as usize), top, shown, total).on_scroll(move |to, window, _| {
                use alacritty_terminal::grid::{Dimensions as _, Scroll};
                let mut t = term.lock();
                let history = t.grid().history_size();
                let (want, now) = (history.saturating_sub(to.round().max(0.0) as usize), t.grid().display_offset());
                if want != now {
                    t.scroll_display(Scroll::Delta(want as i32 - now as i32));
                    drop(t);
                    window.refresh();
                }
            });
            root = root.child(div().absolute().left(screen.left()).top(screen.top()).w(screen.size.width).h(screen.size.height).child(bar));
        }
        // Between rows, the divider and the head of the row under it drag
        // the rows' heights (the pointer shows it on the divider); clicked,
        // the pane there is picked. Between panes side by side, the line
        // drags their widths. (The cursor is on a hitbox of its own: one
        // set as the window was drawn showed only when the pointer had
        // been there then.)
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
            let me = me.clone();
            let el = canvas(
                move |_, window, _| (window.insert_hitbox(press, HitboxBehavior::Normal), window.insert_hitbox(line, HitboxBehavior::Normal)),
                move |_, (hitbox, line_hitbox): (Hitbox, Hitbox), window, cx| {
                    let (cursor, name) = match divider {
                        Divider::Row(r) => (CursorStyle::ResizeUpDown, format!("Row Height {r}")),
                        Divider::Pane(r, c) => (CursorStyle::ResizeLeftRight, format!("Pane Width {r}.{c}")),
                    };
                    window.set_cursor_style(cursor, &line_hitbox);
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Splitter", name: Some(name.into()), bounds: press, enabled: true, ..Default::default() });
                    let (m1, h1) = (me.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble && h1.is_hovered(window) {
                            cx.stop_propagation();
                            let _ = m1.update(cx, |t, cx| t.divider_down(id, divider, e.position, e.button, window, cx));
                        }
                    });
                },
            )
            .absolute()
            .inset_0();
            root = root.child(el);
        }
        // A divider being dragged: what it divides follows the pointer
        // till the button goes up, listened for in each frame while it
        // lasts (a frame's listeners are that frame's: one taken at the
        // press followed the first move only).
        let dragging = match self.windows.get(&id) {
            Some(Win::Term(t)) => t.dragging.as_ref().map(|d| matches!(d.across, Across::Rows(_))),
            _ => None,
        };
        if let Some(rows) = dragging {
            let me = me.clone();
            let el = canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    window.set_window_cursor_style(if rows { CursorStyle::ResizeUpDown } else { CursorStyle::ResizeLeftRight });
                    let m1 = me.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let along = if rows { e.position.y } else { e.position.x };
                            let _ = m1.update(cx, |t, cx| t.divider_drag(id, e.dragging().then_some(along), cx));
                        }
                    });
                    let m2 = me.clone();
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = m2.update(cx, |t, cx| t.divider_drag(id, None, cx));
                        }
                    });
                },
            )
            .absolute()
            .inset_0();
            root = root.child(el);
        }
        // Visual bell: a brief flash.
        if let Some(age) = bell {
            let a = (1.0 - age / 0.15) * 60.0 / 255.0 / 2.0;
            let area = layout.area;
            root = root.child(canvas(|_, _, _| {}, move |_, _, window, _| window.paint_quad(gpui::fill(area, gpui::Rgba { a, ..flash_color }))).absolute().inset_0());
            window.request_animation_frame();
        }
        let menus = self.menus(id).unwrap_or_default();
        let bar = MenuBar::new(menus, {
            let me = me.clone();
            move |cmd, _, cx| {
                let cmd = cmd.to_owned();
                let _ = me.update(cx, |t, cx| t.command(cx, id, &cmd));
            }
        })
        .raw();
        let _ = p;
        root.child(strip).child(div().absolute().top_0().left_0().right_0().child(bar)).into_any_element()
    }

    /// A key pressed in a terminal window.
    fn key_down(&mut self, id: WindowId, e: &KeyDownEvent, window: &mut Window, cx: &mut Cx) {
        let k = &e.keystroke;
        // The command key (⌘, with a Mac's keys: the right Ctrl, as
        // app_server sends it; with others, Ctrl itself).
        let command = k.modifiers.control && (gpui::right_control_held() || !olo_kit_base::keys::command_key());
        // ⌘ with a menu shortcut's key is the app's (⌘C arrives as
        // Ctrl+Shift+C): left to the window's menu bar.
        if command && menu::shortcut_command(window, cx, k).is_some() {
            return;
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        // Ctrl+Tab and Ctrl+Page Up/Down go through the tabs, as in other
        // terminals; the shell never sees them.
        let step = olo_ui::tab_step(k);
        if step != 0 && t.tabs.len() > 1 {
            let n = t.tabs.len() as i32;
            t.select((t.active as i32 + step).rem_euclid(n) as usize);
            cx.stop_propagation();
            cx.redraw(id);
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
        if view::key(&tab.session, key, true, e.is_held) {
            tab.mouse.hover = None;
            cx.stop_propagation();
            cx.redraw(id);
            self.typed(cx);
        }
    }

    fn key_up(&mut self, id: WindowId, e: &KeyUpEvent, cx: &mut Cx) {
        let Some(Win::Term(t)) = self.windows.get(&id) else { return };
        let key = view::Key::of(&e.keystroke);
        if view::key(&t.tab().session, key, false, false) {
            cx.stop_propagation();
        }
    }

    /// Text typed (or an input method's) in window `id`.
    pub(crate) fn typed_text(&mut self, cx: &mut Cx, id: WindowId, text: &str) {
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        let tab = t.tab_mut();
        if let Some(k) = tab.keep_going.as_mut() {
            k.since = std::time::Instant::now();
        }
        view::text(&tab.session, text);
        tab.mouse.hover = None;
        cx.redraw(id);
        self.typed(cx);
    }

    /// The text an input method is composing in window `id`.
    pub(crate) fn preedit(&self, id: WindowId) -> Option<&str> {
        match self.windows.get(&id) {
            Some(Win::Term(t)) => t.preedit.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn set_preedit(&mut self, cx: &mut Cx, id: WindowId, text: Option<String>) {
        if let Some(Win::Term(t)) = self.windows.get_mut(&id)
            && t.preedit != text
        {
            t.preedit = text;
            cx.redraw(id);
        }
    }

    /// Where the cursor is in window `id` (for an input method's window).
    pub(crate) fn cursor_bounds(&self, id: WindowId) -> Option<Bounds<Pixels>> {
        let Some(Win::Term(t)) = self.windows.get(&id) else { return None };
        let m = self.metrics?;
        let rect = t.tab().pane.bounds.get();
        let term = t.tab().session.term.try_lock_unfair()?;
        let c = term.grid().cursor.point;
        let offset = term.grid().display_offset() as i32;
        let x = f32::from(rect.left()) + view::PAD.0 + c.column.0 as f32 * m.cell.0;
        let y = f32::from(rect.top()) + view::PAD.1 + (c.line.0 + offset) as f32 * m.cell.1;
        Some(Bounds { origin: point(px(x), px(y)), size: size(px(m.cell.0), px(m.cell.1)) })
    }

    /// The pointer on the screen of tab `shell` of window `id`, drawn in
    /// `rect`.
    fn pane_event(&mut self, id: WindowId, shell: u64, rect: Bounds<Pixels>, e: PaneEvent, window: &mut Window, cx: &mut Cx) {
        let Some(m) = self.metrics else { return };
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        let Some(i) = t.position(shell) else { return };
        let modifiers = match &e {
            PaneEvent::Down(e) => e.modifiers,
            PaneEvent::Move(e) => e.modifiers,
            PaneEvent::Up(e) => e.modifiers,
            PaneEvent::Wheel(e) => e.modifiers,
        };
        let command = modifiers.control && (gpui::right_control_held() || !olo_kit_base::keys::command_key());
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
        // Where the pointer is over the rows as drawn (scrolled by the
        // point, or stretched: below their places).
        let shift = px(tab.pane.shift.get());
        let on_rows = |p: gpui::Point<Pixels>| point(p.x, p.y - shift);
        match e {
            PaneEvent::Down(e) => {
                tab.mouse.hover_at(&tab.session, &m, rect, Some(on_rows(e.position)), mods.command, &cwd);
                tab.mouse.down(&tab.session, &m, rect, on_rows(e.position), e.button, e.click_count, mods, &mut out);
                redraw = true;
                // A press on a pane not typed into picks it.
                if several && i != t.active {
                    t.select(i);
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
                tab.mouse.up(&tab.session, &m, rect, on_rows(e.position), e.button, mods);
                redraw = true;
            }
            PaneEvent::Wheel(e) => {
                let dy = f32::from(e.delta.pixel_delta(px(m.cell.1)).y);
                tab.mouse.wheel(&tab.session, &tab.pane, &m, rect, on_rows(e.position), dy, (e.kind, e.touch_phase), mods);
                redraw = true;
            }
        }
        if redraw {
            cx.redraw(id);
        }
        if let Some(target) = &out.open {
            crate::links::open(target);
        }
        if out.paste {
            self.paste(cx, id);
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
            self.popup(id, menu, pos, window, cx);
        }
    }

    /// Opens `menu` at `pos` in window `id`; what's chosen is a command.
    pub(crate) fn popup(&mut self, id: WindowId, mut menu: Menu, pos: gpui::Point<Pixels>, window: &mut Window, cx: &mut Cx) {
        // The bar's keys as its MenuBar shows them (raw: Terminal's own).
        if let Some(mut bar) = self.menus(id) {
            menu::resolve(&mut bar, true);
            menu.keys_from(&bar);
        }
        let me = cx.weak_entity();
        menu::context_menu(window, cx, menu, pos, move |cmd, _, cx| {
            let cmd = cmd.to_owned();
            let _ = me.update(cx, |t, cx| t.command(cx, id, &cmd));
        });
    }

    /// The clipboard's text into the shell showing in window `id`, once
    /// it's read (off the UI thread: `paste_item`).
    pub(crate) fn paste(&mut self, cx: &mut Cx, id: WindowId) {
        let read = cx.paste_item();
        cx.spawn(async move |this, cx| {
            let Some(text) = read.await.and_then(|c| c.text()) else { return };
            let _ = this.update(cx, |me, cx| {
                if let Some(Win::Term(t)) = me.windows.get(&id) {
                    view::paste(&t.tab().session, &text);
                    cx.redraw(id);
                }
            });
        })
        .detach();
    }

    /// A divider in Tile All Tabs pressed: dragged, the rows (or the
    /// panes) either side grow and shrink; a row's clicked, the pane there
    /// is picked; the right button, the row's menu.
    fn divider_down(&mut self, id: WindowId, divider: Divider, at: gpui::Point<Pixels>, button: MouseButton, window: &mut Window, cx: &mut Cx) {
        let Some(m) = self.metrics else { return };
        let Some(layout) = self.lay_out(id, window, &m) else { return };
        let least = least_pane(self.settings.side_tabs, &m);
        let Some(Win::Term(t)) = self.windows.get_mut(&id) else { return };
        let row = match divider {
            Divider::Row(r) | Divider::Pane(r, _) => r,
        };
        if button == MouseButton::Right {
            let group = layout.rows.get(row).and_then(|row| row.tabs.first()).and_then(|&i| t.tabs[i].group);
            let changed = t.sizes_changed();
            let menu = match group {
                Some(g) => crate::group_menu(g, t),
                None => Menu::new("").item("Tile All Tabs", "tile-tabs").checked(t.tiled),
            };
            let menu = menu.item("Reset Pane Sizes", "tile-reset-heights").enabled(changed);
            let menu = if group.is_some() { self.worktrees_menu(id, menu) } else { menu };
            return self.popup(id, menu, at, window, cx);
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
                    cx.redraw(id);
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
        // Followed till the button goes up (`divider_drag`, from the
        // frames drawn meanwhile).
        t.dragging = Some(drag);
        cx.redraw(id);
    }

    /// The divider being dragged in window `id` with the pointer at
    /// `along` (its y between rows, its x between panes); None: let go.
    fn divider_drag(&mut self, id: WindowId, along: Option<Pixels>, cx: &mut Cx) {
        let Some(Win::Term(w)) = self.windows.get_mut(&id) else { return };
        let Some(d) = &w.dragging else { return };
        let Some(along) = along else {
            w.dragging = None;
            cx.redraw(id);
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
            cx.redraw(id);
        }
    }

    /// What was done to window `id`'s tab strip.
    fn strip_action(&mut self, id: WindowId, action: StripAction, window: &mut Window, cx: &mut Cx) {
        let side = self.settings.side_tabs;
        // (A tab or a group dragged along the strip: a step to undo.)
        let moved = match &action {
            StripAction::Move { .. } => Some(("Move Tab", self.tab_order(id))),
            StripAction::MoveGroup { .. } => Some(("Move Group", self.tab_order(id))),
            _ => None,
        };
        self.strip_did(id, action, side, window, cx);
        if let Some((name, before)) = moved {
            self.tabs_moved(id, before, name);
        }
    }

    fn strip_did(&mut self, id: WindowId, action: StripAction, side: bool, window: &mut Window, cx: &mut Cx) {
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
                if tab.group.is_some() && group.is_none() {
                    // Dragged out of its group by the person.
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
                let mut menu = crate::group_menu(g, t);
                if tiling_rows {
                    menu = menu.item("Reset Pane Sizes", "tile-reset-heights").enabled(t.sizes_changed());
                }
                let menu = self.worktrees_menu(id, menu);
                self.popup(id, menu, pos, window, cx);
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
                // when it stops (Keep It Going).
                let keep = t.tabs[i].keep_going.is_some();
                let menu = if t.tabs[i].agent.is_some() || keep { menu.separator().item("Keep It Going", format!("tab-keep-going:{shell}")).checked(keep) } else { menu };
                self.popup(id, menu, pos, window, cx);
            }
        }
    }

    /// The Find window for terminal window `target`.
    fn render_find(&mut self, id: WindowId, focus: FocusHandle, _: &mut Window, cx: &mut Cx) -> AnyElement {
        let Some(Win::Find { field, .. }) = self.windows.get(&id) else { return div().into_any_element() };
        let field = field.clone();
        let s = &self.search;
        let ready = !field.read(cx).text().is_empty();
        let check = |label: &str, on: bool, f: fn(&mut crate::find::Search, bool), cx: &mut Cx| {
            let set = cx.listener(move |t, v: &bool, _, cx| {
                f(&mut t.search, *v);
                cx.redraw(id);
            });
            CheckBox::new(label.to_owned(), on).on_toggle(move |v, window, cx| set(&v, window, cx))
        };
        let (case, words, wrap, back) = (s.case, s.words, s.wrap, s.backwards);
        let col = div()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(TextControl::new(field.clone()).label("Find:").divider(40.0))
            .child(div().h(px(m::SPACING - m::TIGHT)))
            .child(check("Case-sensitive", case, |s, v| s.case = v, cx))
            .child(check("Whole words only", words, |s, v| s.words = v, cx))
            .child(check("Wrap-around search", wrap, |s, v| s.wrap = v, cx))
            .child(check("Search backwards", back, |s, v| s.backwards = v, cx));
        let cancel = cx.listener(move |t, _: &(), _, cx| t.find_done(cx, id, false));
        let go = cx.listener(move |t, _: &(), _, cx| t.find_done(cx, id, true));
        let row = widgets::button_row()
            .child(Button::new("Cancel").on_click(move |window, cx| cancel(&(), window, cx)))
            .child(Button::new("Find").default(true).enabled(ready).on_click(move |window, cx| go(&(), window, cx)));
        div()
            .size_full()
            .key_context(widgets::alert::CONTEXT)
            .track_focus(&focus)
            .on_action(cx.listener(move |t, _: &widgets::alert::Default, _, cx| t.find_done(cx, id, true)))
            .on_action(cx.listener(move |t, _: &widgets::alert::Cancel, _, cx| t.find_done(cx, id, false)))
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .child(col)
            .child(div().flex_1())
            .child(row)
            .into_any_element()
    }

    /// The Find window answered: search (and close it) or just close it.
    pub(crate) fn find_done(&mut self, cx: &mut Cx, id: WindowId, go: bool) {
        let Some(Win::Find { target, field }) = self.windows.get(&id) else { return };
        let target = *target;
        let text = field.read(cx).text().to_owned();
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

    /// A text field for one of Terminal's windows, with `text`; Return in
    /// it is the window's default button.
    pub(crate) fn field(cx: &mut Cx, text: &str, build: impl FnOnce(TextField) -> TextField) -> Entity<TextField> {
        let f = widgets::text_field(cx, build);
        let text = text.to_owned();
        f.update(cx, |f, cx| f.set_text(text, cx));
        f
    }

    /// The Group window: a name and a colour for a group.
    fn render_name_group(&mut self, id: WindowId, focus: FocusHandle, window: &mut Window, cx: &mut Cx) -> AnyElement {
        let Some(Win::NameGroup { name, color, .. }) = self.windows.get(&id) else { return div().into_any_element() };
        let (name, color) = (name.clone(), *color);
        let _ = window;
        let ready = !name.read(cx).text().trim().is_empty();
        let look = self.look.clone();
        let me = cx.weak_entity();
        // The colours, as swatches; the chosen one on the selection.
        let swatches = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                style::label(window, cx, b.left(), b.center().y, style::Align::Left, "Color:", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
                for i in 0..groups::COLORS.len() {
                    let r = Bounds { origin: point(b.left() + px(50.0 + i as f32 * (SWATCH + m::SPACING)), b.top()), size: size(px(SWATCH), px(SWATCH)) };
                    if color == i {
                        let ring = Bounds::centered_at(r.center(), size(px(SWATCH + 2.0), px(SWATCH + 2.0)));
                        let (fill, edge) = (style::c(style::lit_ground(&p, p.panel_background)), style::c(p.selection_edge()));
                        window.paint_quad(gpui::quad(ring, px(SWATCH / 2.0 + 1.0), fill, px(1.0), edge, gpui::BorderStyle::Solid));
                    }
                    let dot = Bounds::centered_at(r.center(), size(px(SWATCH - 6.0), px(SWATCH - 6.0)));
                    window.paint_quad(gpui::fill(dot, groups::color(i, &look)).corner_radii(px(SWATCH / 2.0 - 3.0)));
                    olo_ui::agent::record(
                        window,
                        cx,
                        olo_ui::agent::Node {
                            role: "RadioButton",
                            name: Some(groups::COLORS[i].name().into()),
                            bounds: r,
                            enabled: true,
                            clickable: true,
                            selected: Some(color == i),
                            ..Default::default()
                        },
                    );
                    let me = me.clone();
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && e.button == MouseButton::Left && r.contains(&e.position) {
                            let _ = me.update(cx, |t, cx| {
                                if let Some(Win::NameGroup { color, .. }) = t.windows.get_mut(&id) {
                                    *color = i;
                                }
                                cx.redraw(id);
                            });
                        }
                    });
                }
            },
        )
        .h(px(SWATCH))
        .w_full();
        let cancel = cx.listener(move |t, _: &(), _, cx| t.name_group_done(cx, id, false));
        let ok = cx.listener(move |t, _: &(), _, cx| t.name_group_done(cx, id, true));
        let row = widgets::button_row()
            .child(Button::new("Cancel").on_click(move |window, cx| cancel(&(), window, cx)))
            .child(Button::new("OK").default(true).enabled(ready).on_click(move |window, cx| ok(&(), window, cx)));
        div()
            .size_full()
            .key_context(widgets::alert::CONTEXT)
            .track_focus(&focus)
            .on_action(cx.listener(move |t, _: &widgets::alert::Default, _, cx| t.name_group_done(cx, id, true)))
            .on_action(cx.listener(move |t, _: &widgets::alert::Cancel, _, cx| t.name_group_done(cx, id, false)))
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .child(TextControl::new(name).label("Name:").divider(50.0))
            .child(div().h(px(2.0 * m::SPACING)))
            .child(swatches)
            .child(div().flex_1())
            .child(row)
            .into_any_element()
    }
}

/// A group colour's swatch.
pub const SWATCH: f32 = 22.0;

/// The keys a field typed into sends its window: Return is the default
/// button's (`TextFieldEvent::Entered`).
pub fn submit_on_enter(cx: &mut Cx, field: &Entity<TextField>, id: WindowId, f: fn(&mut Terminal, &mut Cx, WindowId)) {
    cx.subscribe(field, move |t, _, e: &TextFieldEvent, cx| match e {
        TextFieldEvent::Entered => f(t, cx, id),
        TextFieldEvent::Changed => cx.redraw(id),
        TextFieldEvent::Typed(_) => {}
    })
    .detach();
}

#[cfg(test)]
mod tile_tests {
    use olo_ui::gpui::{Bounds, point, px};

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Bounds<olo_ui::gpui::Pixels> {
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
