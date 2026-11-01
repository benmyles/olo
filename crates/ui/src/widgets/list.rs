//! BColumnListView, as the egui kit draws and drives it: a sunken list
//! with a header of columns (a click sorts, again reverses; the dividers
//! drag to resize), only the rows in view built (`uniform_list`), the
//! selection every file manager has ([`ListState`]: a click, ⇧ for a
//! range, ⌘ to add or take away, a band dragged over rows, the arrows,
//! Page Up and Down, Home and End, ⇧ with them to grow it, ⌘A all,
//! typing to go to a name), a double click or Return to open, a right
//! click for a context menu, and BeOS scroll bars (sideways too, when
//! the columns are wider together than the view). The app keeps the
//! list's state in a [`ListModel`] entity and draws each cell itself
//! ([`Cell`]). Agents read each row (`Row`, named by its cells' text,
//! selected or not) and each column (`ColumnHeader`, sorted or not).

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, ContentMask, CursorStyle, Entity, FocusHandle, Focusable, Font, FontWeight, Hitbox, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, RenderOnce, ScrollStrategy, SharedString, Styled as _, UniformListScrollHandle, Window, actions, canvas, div, point, px, size, uniform_list,
};
use olo_kit_base::{
    list::{ListState, Mods, SelectionBand, TypeSelect},
    metrics as m,
};

use super::scroll::{SCROLL_BAR, ScrollBar};
use crate::{agent, style, theme};

/// The header's height.
pub const HEADER_HEIGHT: f32 = 18.0;

actions!(olo_list, [Down, Up, PageDown, PageUp, Home, End, SelectDown, SelectUp, SelectPageDown, SelectPageUp, SelectHome, SelectEnd, Open, SelectAll]);

const CONTEXT: &str = "OloList";

pub(crate) fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", Down, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-pagedown", SelectPageDown, c),
        KeyBinding::new("shift-pageup", SelectPageUp, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("enter", Open, c),
        KeyBinding::new("ctrl-a", SelectAll, c),
    ]);
}

/// A column: its title, width, and which side its text keeps to.
#[derive(Clone, Debug)]
pub struct Column {
    pub title: SharedString,
    pub width: f32,
    pub align: style::Align,
    pub min_width: f32,
}

impl Column {
    pub fn new(title: impl Into<SharedString>, width: f32) -> Column {
        Column { title: title.into(), width, align: style::Align::Left, min_width: 24.0 }
    }

    /// Its text on the right (numbers).
    pub fn right(mut self) -> Self {
        self.align = style::Align::Right;
        self
    }
}

/// What the pointer is doing to the list.
#[derive(Clone)]
enum Drag {
    /// A press on the rows: where (in the content, so a band stays put
    /// as it scrolls, and in the window), on which row, the selection a
    /// band would start from, whether it has become one and where the
    /// pointer is; `defer`: a press on a selected row leaves the
    /// selection (to drag it) and takes the row alone at the release.
    Press { from: Point<Pixels>, at: Point<Pixels>, row: Option<usize>, base: SelectionBand, band: bool, now: Point<Pixels>, defer: bool },
    /// A column's title held: it sorts at the release over it.
    Title(usize),
    /// A divider dragged: which column's, from where, at what width.
    Divider { column: usize, x: Pixels, width: f32 },
}

/// A list's state from frame to frame, kept by the app.
pub struct ListModel {
    pub state: ListState,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// The widths the person dragged the columns to.
    widths: Vec<f32>,
    typing: TypeSelect,
    drag: Option<Drag>,
    /// Where the rows showed, last drawn (without the header and bars).
    body: Bounds<Pixels>,
    /// Columns wider together than the view: they scroll sideways.
    sideways: bool,
}

impl ListModel {
    /// A list's state; `multiple`: more than one row may be selected.
    pub fn new(cx: &mut App, multiple: bool) -> Entity<ListModel> {
        cx.new(|cx| ListModel {
            state: ListState { multiple, ..Default::default() },
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            widths: Vec::new(),
            typing: TypeSelect::default(),
            drag: None,
            body: Bounds::default(),
            sideways: false,
        })
    }

    /// Scrolls so `row` shows, in the middle when it didn't (a selection
    /// made in code: what's round it shows too).
    pub fn reveal(&self, row: usize) {
        self.scroll.scroll_to_item(row, ScrollStrategy::Center);
    }

    /// Whether its last row shows (scrolled to the end, or all fit): a
    /// list that follows what's added (Log Viewer's live lines) keeps
    /// following only then.
    pub fn at_end(&self) -> bool {
        let h = self.scroll.0.borrow().base_handle.clone();
        f32::from(h.offset().y) <= -f32::from(h.max_offset().height) + 1.0
    }

    /// Gives the list the keyboard.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    /// A name is being typed to select (a space now is part of it, not
    /// Quick Look's).
    pub fn typing(&self) -> bool {
        self.typing.typing()
    }

    /// The scroll handle's offset (negative as the rows go up).
    fn offset(&self) -> Point<Pixels> {
        self.scroll.0.borrow().base_handle.offset()
    }

    /// Scrolls the least that shows `row` of `rows`, `row_h` tall (the
    /// keyboard moving).
    fn scroll_to(&self, row: usize, rows: usize, row_h: f32) {
        let handle = self.scroll.0.borrow().base_handle.clone();
        let view = f32::from(self.body.size.height);
        let top_now = -f32::from(handle.offset().y);
        let (top, bottom) = (row as f32 * row_h, (row + 1) as f32 * row_h);
        let to = if top < top_now {
            top
        } else if bottom > top_now + view {
            bottom - view
        } else {
            return;
        };
        let max = (rows as f32 * row_h - view).max(0.0);
        handle.set_offset(point(handle.offset().x, px(-to.clamp(0.0, max))));
    }

    /// The row at `p` (in the window), if there's one.
    fn row_at(&self, p: Point<Pixels>, rows: usize, row_h: f32) -> Option<usize> {
        let y = f32::from(p.y - self.body.top() - self.offset().y);
        (y >= 0.0 && self.body.contains(&p)).then(|| (y / row_h) as usize).filter(|&r| r < rows)
    }

    /// `p` (in the window) in the content's coordinates.
    fn content(&self, p: Point<Pixels>) -> Point<Pixels> {
        let off = self.offset();
        point(p.x - self.body.left() - off.x, p.y - self.body.top() - off.y)
    }

    /// A band from where it started to the pointer selects the rows it
    /// covers (with what it started from); whether that changed.
    fn band_over(&mut self, rows: usize, row_h: f32) -> bool {
        let Some(Drag::Press { from, now, base, .. }) = &self.drag else { return false };
        let now_y = f32::from(self.content(*now).y);
        let (top, bottom) = (f32::from(from.y).min(now_y), f32::from(from.y).max(now_y));
        let first = (top / row_h).floor().max(0.0) as usize;
        let end = ((bottom / row_h).ceil().max(0.0) as usize).min(rows);
        let selected = base.over(first..end.max(first));
        let changed = selected != self.state.selected;
        self.state.selected = selected;
        changed
    }
}

impl Focusable for ListModel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A cell being drawn: where, which, and how its text is coloured.
pub struct Cell<'a> {
    pub bounds: Bounds<Pixels>,
    pub row: usize,
    pub column: usize,
    pub selected: bool,
    pub text_color: Hsla,
    pub align: style::Align,
    texts: &'a mut Vec<SharedString>,
}

impl Cell<'_> {
    /// Its text, inset, kept to its side, cut short with "…" when it
    /// doesn't fit.
    pub fn text(&mut self, window: &mut Window, cx: &mut App, text: impl Into<SharedString>) {
        let color = self.text_color;
        self.text_with(window, cx, text, style::ui_font(FontWeight::NORMAL), color);
    }

    /// Its text in runs (a log line's colours, what a search found marked
    /// behind), cut short with "…" when it doesn't fit.
    pub fn text_runs(&mut self, window: &mut Window, cx: &mut App, text: impl Into<SharedString>, runs: &[gpui::TextRun]) {
        let text = text.into();
        let inset = px(m::CELL_INSET);
        let x = match self.align {
            style::Align::Left => self.bounds.left() + inset,
            style::Align::Center => self.bounds.center().x,
            style::Align::Right => self.bounds.right() - inset,
        };
        style::label_runs_fit(window, cx, x, self.bounds.center().y, self.align, text.clone(), style::PLAIN_SIZE, runs, self.bounds.size.width - inset * 2.0);
        self.texts.push(text);
    }

    /// Its text in `font` and `color`.
    pub fn text_with(&mut self, window: &mut Window, cx: &mut App, text: impl Into<SharedString>, font: Font, color: Hsla) {
        let text = text.into();
        cell_text(window, cx, self.bounds, self.align, text.clone(), font, color);
        self.texts.push(text);
    }

    /// A 16-point icon at its left (`icon`, else `fallback`), then its
    /// text: an app's or a file's name.
    pub fn icon_text(&mut self, window: &mut Window, cx: &mut App, icon: &str, fallback: &str, text: impl Into<SharedString>) {
        let text = text.into();
        let at = Bounds::centered_at(point(self.bounds.left() + px(m::CELL_INSET + 8.0), self.bounds.center().y), size(px(16.0), px(16.0)));
        if !crate::icons::paint(window, at, icon) {
            crate::icons::paint(window, at, fallback);
        }
        let rest = Bounds::from_corners(point(at.right() + px(2.0), self.bounds.top()), self.bounds.bottom_right());
        let color = self.text_color;
        cell_text(window, cx, rest, style::Align::Left, text.clone(), style::ui_font(FontWeight::NORMAL), color);
        self.texts.push(text);
    }
}

/// Text in a cell `b`: inset, kept to `align`'s side, cut short to fit.
fn cell_text(window: &mut Window, cx: &mut App, b: Bounds<Pixels>, align: style::Align, text: SharedString, font: Font, color: Hsla) {
    let inset = px(m::CELL_INSET);
    let x = match align {
        style::Align::Left => b.left() + inset,
        style::Align::Center => b.center().x,
        style::Align::Right => b.right() - inset,
    };
    style::label_fit(window, cx, x, b.center().y, align, text, style::PLAIN_SIZE, font, color, b.size.width - inset * 2.0);
}

type CellFn = Rc<dyn Fn(&mut Cell, &mut Window, &mut App)>;
type RowFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type AtFn = Rc<dyn Fn(usize, Point<Pixels>, &mut Window, &mut App)>;
type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// What the app is told.
#[derive(Clone, Default)]
struct Handlers {
    open: Option<RowFn>,
    /// A row dragged out of the list (Disk Usage's Collector, Tracker).
    drag: Option<RowFn>,
    context: Option<AtFn>,
    select: Option<Handler>,
    sort: Option<Handler>,
}

impl Handlers {
    fn selected(&self, window: &mut Window, cx: &mut App) {
        if let Some(f) = &self.select {
            f(window, cx);
        }
    }
}

/// The list, drawn from its [`ListModel`].
#[derive(IntoElement)]
pub struct ColumnListView {
    id: SharedString,
    model: Entity<ListModel>,
    columns: Rc<[Column]>,
    rows: usize,
    row_height: f32,
    cell: CellFn,
    names: Option<Rc<dyn Fn(usize) -> SharedString>>,
    empty: Option<SharedString>,
    handlers: Handlers,
}

impl ColumnListView {
    /// `rows` rows of `columns`, each cell drawn by `cell`.
    pub fn new(id: impl Into<SharedString>, model: &Entity<ListModel>, columns: impl Into<Rc<[Column]>>, rows: usize, cell: impl Fn(&mut Cell, &mut Window, &mut App) + 'static) -> ColumnListView {
        ColumnListView { id: id.into(), model: model.clone(), columns: columns.into(), rows, row_height: m::LIST_ROW, cell: Rc::new(cell), names: None, empty: None, handlers: Handlers::default() }
    }

    pub fn row_height(mut self, h: f32) -> Self {
        self.row_height = h;
        self
    }

    /// Typing goes to the row whose name (`name(row)`) starts so.
    pub fn type_select(mut self, name: impl Fn(usize) -> SharedString + 'static) -> Self {
        self.names = Some(Rc::new(name));
        self
    }

    /// Said in the middle while there are no rows ("No lines match.").
    pub fn empty_text(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = Some(text.into());
        self
    }

    /// A row double-clicked, or Return on it.
    pub fn on_open(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.handlers.open = Some(Rc::new(f));
        self
    }

    /// A row right-clicked, where (for a context menu); the selection is
    /// it, or the rows selected with it.
    pub fn on_context(mut self, f: impl Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static) -> Self {
        self.handlers.context = Some(Rc::new(f));
        self
    }

    /// Rows can be dragged out: a press on a selected row, or on a
    /// row's first column, that moves starts a drag of it (selected, if
    /// it wasn't), which `f` takes on (following the pointer, dropping
    /// it); elsewhere a press that moves draws a band.
    pub fn on_drag(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.handlers.drag = Some(Rc::new(f));
        self
    }

    /// The selection changed.
    pub fn on_select(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.handlers.select = Some(Rc::new(f));
        self
    }

    /// The sort changed (`state.sort`): sort the rows again.
    pub fn on_sort(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.handlers.sort = Some(Rc::new(f));
        self
    }
}

/// The columns' widths: as dragged, else as given.
fn widths(model: &ListModel, columns: &[Column]) -> Vec<f32> {
    if model.widths.len() == columns.len() { model.widths.clone() } else { columns.iter().map(|c| c.width).collect() }
}

/// Each column's left and right, from `left`: the last fills to `right`
/// unless they scroll sideways.
fn spans(widths: &[f32], left: Pixels, right: Pixels, sideways: bool) -> Vec<(Pixels, Pixels)> {
    let last = widths.len().saturating_sub(1);
    let mut x = left;
    widths
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let end = if i == last && !sideways { right.max(x) } else { x + px(*w) };
            let span = (x, end);
            x = end;
            span
        })
        .collect()
}

/// The sort arrow, centred on (`x`, `y`): its point up when ascending.
fn sort_arrow(window: &mut Window, x: Pixels, y: Pixels, ascending: bool, color: Hsla) {
    let (tip, base) = if ascending { (y - px(2.0), y + px(2.0)) } else { (y + px(2.0), y - px(2.0)) };
    let mut path = PathBuilder::fill();
    path.move_to(point(x - px(3.5), base));
    path.line_to(point(x + px(3.5), base));
    path.line_to(point(x, tip));
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// How far a band held near or past the top or bottom of the rows `b`
/// scrolls them a frame (none away from them).
fn band_step(b: Bounds<Pixels>, y: Pixels, row_h: f32) -> f32 {
    let edge = px(row_h / 2.0);
    if y > b.bottom() - edge {
        (f32::from(y - b.bottom() + edge) / 2.0).clamp(2.0, row_h * 2.0)
    } else if y < b.top() + edge {
        -(f32::from(b.top() + edge - y) / 2.0).clamp(2.0, row_h * 2.0)
    } else {
        0.0
    }
}

impl RenderOnce for ColumnListView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let model = self.model.clone();
        let (focus, scroll, sideways) = {
            let m = model.read(cx);
            (m.focus.clone(), m.scroll.clone(), m.sideways)
        };
        let base_handle = scroll.0.borrow().base_handle.clone();
        let (rows, row_h, columns, handlers) = (self.rows, self.row_height, self.columns.clone(), self.handlers.clone());
        let total: f32 = widths(model.read(cx), &columns).iter().sum();
        let first_width = widths(model.read(cx), &columns).first().copied().unwrap_or(0.0);
        // Inside the sunken frame's two lines, on device pixels; the
        // header's bottom and the bars' edges too (layout rounds a box's
        // offset from its parent but its size from where it ends, so a
        // header 22.5 device pixels tall at 1.25× and rows 24.5 down
        // round a pixel apart).
        let s = window.scale_factor();
        let dev = |v: f32| (v * s).round() / s;
        let edge = style::line_pixels(s) * 2.0 / s;
        let head = dev(edge + HEADER_HEIGHT);
        let bar = dev(edge + SCROLL_BAR);
        let hbar = if sideways { bar } else { edge };

        // The rows in view, each a canvas drawing its cells.
        let (list_model, list_columns, cell) = (model.clone(), columns.clone(), self.cell.clone());
        let list = uniform_list(SharedString::from(format!("{}-rows", self.id)), rows, move |range, window, cx| {
            let state = list_model.read(cx);
            let widths = widths(state, &list_columns);
            let focused = state.focus.is_focused(window);
            let sideways = state.sideways;
            let selected: Vec<bool> = range.clone().map(|r| state.state.selected.contains(&r)).collect();
            range
                .zip(selected)
                .map(|(row, selected)| {
                    let (cell, columns, widths) = (cell.clone(), list_columns.clone(), widths.clone());
                    let row_el = canvas(
                        |_, _, _| {},
                        move |r, _, window, cx| {
                            let p = theme::palette(cx);
                            if selected {
                                let sel = if focused { p.list_selected_background } else { olo_theme::mix(p.list_selected_background, p.list_background, 0.35) };
                                style::fill_rect(window, r, style::c(sel));
                            }
                            let text_color = style::c(if selected { p.list_selected_item_text } else { p.list_item_text });
                            let mut texts = Vec::new();
                            for (i, (x0, x1)) in spans(&widths, r.left(), r.right(), sideways).into_iter().enumerate() {
                                let b = Bounds::from_corners(point(x0, r.top()), point(x1, r.bottom()));
                                window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
                                    cell(&mut Cell { bounds: b, row, column: i, selected, text_color, align: columns[i].align, texts: &mut texts }, window, cx);
                                });
                            }
                            let name = texts.iter().map(|t| t.as_ref()).collect::<Vec<_>>().join("  ");
                            agent::record(window, cx, agent::Node { role: "Row", name: Some(name.into()), bounds: r, enabled: true, clickable: true, selected: Some(selected), ..Default::default() });
                        },
                    )
                    .h(px(row_h));
                    if sideways { row_el.w(px(total)) } else { row_el.w_full() }
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(scroll.clone())
        .with_horizontal_sizing_behavior(if sideways { ListHorizontalSizingBehavior::Unconstrained } else { ListHorizontalSizingBehavior::FitList })
        .size_full();

        // Over the rows: the pointer (a press, a band, a right click) and
        // the band drawn.
        let (body_model, body_handlers) = (model.clone(), handlers.clone());
        let (laid_model, laid_columns) = (model.clone(), columns.clone());
        let pointer = canvas(
            move |b, window, cx| {
                let total: f32 = widths(laid_model.read(cx), &laid_columns).iter().sum();
                let sideways = total > f32::from(b.size.width) + 0.5;
                let changed = laid_model.update(cx, |m, _| {
                    m.body = b;
                    std::mem::replace(&mut m.sideways, sideways) != sideways
                });
                if changed {
                    window.request_animation_frame();
                }
                window.insert_hitbox(b, HitboxBehavior::Normal)
            },
            move |b, hitbox, window, cx| {
                let model = body_model.clone();
                if let Some(Drag::Press { from, now, band: true, .. }) = model.read(cx).drag.clone() {
                    // Held near or past the top or bottom: on, a frame at
                    // a time.
                    let step = band_step(b, now.y, row_h);
                    let handle = model.read(cx).scroll.0.borrow().base_handle.clone();
                    let max = (rows as f32 * row_h - f32::from(b.size.height)).max(0.0);
                    let top_now = -f32::from(handle.offset().y);
                    let to = (top_now + step).clamp(0.0, max);
                    if to != top_now {
                        handle.set_offset(point(handle.offset().x, px(-to)));
                        if model.update(cx, |m, _| m.band_over(rows, row_h)) {
                            body_handlers.selected(window, cx);
                        }
                        window.request_animation_frame();
                    }
                    let off = handle.offset();
                    let start = point(b.left() + off.x + from.x, b.top() + off.y + from.y);
                    let r = Bounds::from_corners(point(start.x.min(now.x), start.y.min(now.y)), point(start.x.max(now.x), start.y.max(now.y)));
                    let p = theme::palette(cx);
                    window.with_content_mask(Some(ContentMask { bounds: b }), |window| style::selection_band(window, r, &p));
                }

                let (m, h, hb) = (model.clone(), body_handlers.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || !hb.is_hovered(window) {
                        return;
                    }
                    let focus = m.read(cx).focus.clone();
                    window.focus(&focus);
                    let mods = Mods { command: e.modifiers.control || e.modifiers.platform, shift: e.modifiers.shift };
                    let row = m.read(cx).row_at(e.position, rows, row_h);
                    match e.button {
                        MouseButton::Left if e.click_count >= 2 => {
                            if let (Some(r), Some(f)) = (row, &h.open) {
                                f(r, window, cx);
                            }
                        }
                        MouseButton::Left => {
                            let changed = m.update(cx, |m, _| {
                                let base = m.state.band(mods);
                                let before = m.state.selected.clone();
                                let alone = !(mods.command || mods.shift);
                                let defer = row.is_some_and(|r| alone && m.state.selected.contains(&r));
                                match row {
                                    Some(r) if !defer => m.state.click(r, mods),
                                    None if alone => {
                                        m.state.selected.clear();
                                        m.state.anchor = None;
                                    }
                                    _ => {}
                                }
                                let from = m.content(e.position);
                                m.drag = Some(Drag::Press { from, at: e.position, row, base, band: false, now: e.position, defer });
                                m.state.selected != before
                            });
                            if changed {
                                h.selected(window, cx);
                            }
                        }
                        MouseButton::Right => {
                            let Some(r) = row else { return };
                            // A selection of several is kept for the menu.
                            if !m.read(cx).state.selected.contains(&r) {
                                m.update(cx, |m, _| m.state.select(r));
                                h.selected(window, cx);
                            }
                            if let Some(f) = &h.context {
                                f(r, e.position, window, cx);
                            }
                        }
                        _ => return,
                    }
                    window.refresh();
                });
                let (m, h) = (model.clone(), body_handlers.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() || !e.dragging() {
                        return;
                    }
                    // A press on a selected row, or on a row's first
                    // column, that moves: the row dragged out.
                    if let (Some(drag), Some(Drag::Press { at, row: Some(r), band: false, .. })) = (&h.drag, m.read(cx).drag.clone()) {
                        let d = e.position - at;
                        let on_item = m.read(cx).state.selected.contains(&r) || at.x < m.read(cx).body.left() + px(first_width);
                        if on_item && f32::from(d.x).hypot(f32::from(d.y)) > 3.0 {
                            let changed = m.update(cx, |m, _| {
                                m.drag = None;
                                let fresh = !m.state.selected.contains(&r);
                                if fresh {
                                    m.state.select(r);
                                }
                                fresh
                            });
                            if changed {
                                h.selected(window, cx);
                            }
                            drag(r, window, cx);
                            window.refresh();
                            return;
                        }
                    }
                    let changed = m.update(cx, |m, _| {
                        let multiple = m.state.multiple;
                        let Some(Drag::Press { at, band, now, .. }) = m.drag.as_mut() else { return None };
                        *now = e.position;
                        // A band once it has moved a little.
                        if !*band {
                            let d = e.position - *at;
                            *band = multiple && f32::from(d.x).hypot(f32::from(d.y)) > 3.0;
                        }
                        let band = *band;
                        Some(band && m.band_over(rows, row_h))
                    });
                    if changed == Some(true) {
                        h.selected(window, cx);
                    }
                    if changed.is_some() {
                        window.refresh();
                    }
                });
                let (m, h) = (model, body_handlers.clone());
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || e.button != MouseButton::Left {
                        return;
                    }
                    let Some(Drag::Press { row, band, defer, .. }) = m.read(cx).drag.clone() else { return };
                    m.update(cx, |m, _| m.drag = None);
                    // A press on a selected row, not dragged: it alone.
                    if let (Some(r), false, true) = (row, band, defer) {
                        let changed = m.update(cx, |m, _| {
                            let before = m.state.selected.clone();
                            m.state.select(r);
                            m.state.selected != before
                        });
                        if changed {
                            h.selected(window, cx);
                        }
                    }
                    window.refresh();
                });
            },
        )
        .absolute()
        .inset_0();

        // The header: each column's title, sorted or not; dividers to drag.
        let (header_model, header_columns, header_handlers) = (model.clone(), columns.clone(), handlers.clone());
        let (laid_model, laid_columns) = (model.clone(), columns.clone());
        let header = canvas(
            move |b, window, cx| {
                let m = laid_model.read(cx);
                // What shows of it: above the rows, to the device pixel
                // they end on (laid out before it), so a column's title
                // keeps to its cells' edge at any scale.
                let right = if m.body.size.width > px(0.0) { m.body.right() } else { b.right() - px(SCROLL_BAR) };
                let shown = Bounds::from_corners(b.origin, point(right, b.bottom()));
                let spans = spans(&widths(m, &laid_columns), b.left() + m.offset().x, shown.right(), m.sideways);
                let last = spans.len().saturating_sub(1);
                let dividers: Vec<(usize, Hitbox)> = spans
                    .iter()
                    .enumerate()
                    .filter(|&(i, &(_, x1))| i != last && x1 >= shown.left() && x1 <= shown.right())
                    .map(|(i, &(_, x1))| (i, window.insert_hitbox(Bounds { origin: point(x1 - px(3.0), b.top()), size: size(px(6.0), b.size.height) }, HitboxBehavior::Normal)))
                    .collect();
                (window.insert_hitbox(shown, HitboxBehavior::Normal), dividers, spans, shown)
            },
            move |b, (hitbox, dividers, spans, shown), window, cx| {
                let p = theme::palette(cx);
                let m = header_model.read(cx);
                let (sort, pressed) = (m.state.sort, if let Some(Drag::Title(c)) = m.drag { Some(c) } else { None });
                let bg = p.panel_background;
                style::vgradient(window, b, style::tint(bg, 0.8), style::shade(&p, bg, 1.04));
                style::hline_above(window, b.left(), b.right(), b.bottom(), style::shade(&p, bg, 1.45));
                window.with_content_mask(Some(ContentMask { bounds: shown }), |window| {
                    for (i, (col, &(x0, x1))) in header_columns.iter().zip(&spans).enumerate() {
                        let r = Bounds::from_corners(point(x0, b.top()), point(x1, b.bottom() - px(1.0)));
                        if pressed == Some(i) {
                            style::fill_rect(window, r, style::shade(&p, bg, 1.1));
                        }
                        // Light leading edge, dark trailing edge.
                        style::vline(window, r.left(), r.top(), r.bottom(), style::c(p.shine));
                        style::vline_left_of(window, r.right(), r.top() + px(2.0), r.bottom() - px(2.0), style::shade(&p, bg, 1.3));
                        let sorted = sort.filter(|(c, _)| *c == i);
                        let font = style::ui_font(if sorted.is_some() { FontWeight::BOLD } else { FontWeight::NORMAL });
                        let text = Bounds::from_corners(r.origin, point(r.right() - px(if sorted.is_some() { 12.0 } else { 0.0 }), r.bottom()));
                        cell_text(window, cx, text, col.align, col.title.clone(), font, style::c(p.panel_text));
                        if let Some((_, ascending)) = sorted {
                            sort_arrow(window, r.right() - px(10.0), style::snap_coord(window, r.center().y), ascending, style::c(p.panel_text));
                        }
                        agent::record(window, cx, agent::Node { role: "ColumnHeader", name: Some(col.title.clone()), bounds: r.intersect(&shown), enabled: true, clickable: true, selected: Some(sorted.is_some()), ..Default::default() });
                    }
                });
                // Above the scroll bar: a blank header cell.
                style::vline(window, shown.right(), b.top(), b.bottom() - px(1.0), style::c(p.shine));
                for (_, d) in &dividers {
                    window.set_cursor_style(CursorStyle::ResizeColumn, d);
                }

                let (m, cols, hb, spans_down) = (header_model.clone(), header_columns.clone(), hitbox, spans.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left || !hb.is_hovered(window) {
                        return;
                    }
                    let drag = match dividers.iter().find(|(_, d)| d.is_hovered(window)) {
                        Some(&(column, _)) => Drag::Divider { column, x: e.position.x, width: widths(m.read(cx), &cols)[column] },
                        None => match spans_down.iter().position(|&(x0, x1)| e.position.x >= x0 && e.position.x < x1) {
                            Some(c) => Drag::Title(c),
                            None => return,
                        },
                    };
                    m.update(cx, |m, _| m.drag = Some(drag));
                    window.refresh();
                });
                let (m, cols) = (header_model.clone(), header_columns.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let Some(Drag::Divider { column, x, width }) = m.read(cx).drag.clone() else { return };
                    let w = (width + f32::from(e.position.x - x)).max(cols[column].min_width);
                    m.update(cx, |m, _| {
                        if m.widths.len() != cols.len() {
                            m.widths = cols.iter().map(|c| c.width).collect();
                        }
                        m.widths[column] = w;
                    });
                    window.refresh();
                });
                let (m, h) = (header_model.clone(), header_handlers.clone());
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || e.button != MouseButton::Left {
                        return;
                    }
                    match m.read(cx).drag.clone() {
                        Some(Drag::Title(c)) => {
                            m.update(cx, |m, _| m.drag = None);
                            // Released over the title still: sorted by it
                            // (again: the other way).
                            let (x0, x1) = spans[c];
                            if e.position.x >= x0 && e.position.x < x1 && b.contains(&e.position) {
                                m.update(cx, |m, _| {
                                    let sorted = m.state.sort.filter(|(col, _)| *col == c);
                                    m.state.sort = Some((c, sorted.is_none_or(|(_, ascending)| !ascending)));
                                });
                                if let Some(f) = &h.sort {
                                    f(window, cx);
                                }
                            }
                            window.refresh();
                        }
                        Some(Drag::Divider { .. }) => {
                            m.update(cx, |m, _| m.drag = None);
                            window.refresh();
                        }
                        _ => {}
                    }
                });
            },
        )
        .absolute()
        .inset_0();

        // Behind the rows (where layout put them, as it put the header):
        // their ground, and what's said when there are none.
        let empty = (rows == 0).then(|| self.empty.clone()).flatten();
        let ground = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                style::fill_rect(window, b, style::c(p.list_background));
                if let Some(text) = &empty {
                    let dim = style::c(olo_theme::mix(p.list_item_text, p.list_background, 0.5));
                    let font = style::ui_font(FontWeight::NORMAL);
                    style::label_fit(window, cx, b.center().x, b.center().y, style::Align::Center, text.clone(), style::PLAIN_SIZE, font, dim, b.size.width - px(2.0 * m::SPACING));
                }
            },
        )
        .absolute()
        .inset_0();
        let frame_focus = focus.clone();
        let frame = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                style::sunken(window, b, &p, frame_focus.is_focused(window));
            },
        )
        .absolute()
        .inset_0();

        let mut root = div()
            .id(self.id.clone())
            .relative()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&focus)
            .child(div().absolute().left(px(edge)).top(px(head)).right(px(bar)).bottom(px(hbar)).child(ground).child(list).child(pointer))
            .child(div().absolute().left(px(edge)).top(px(edge)).right(px(edge)).h(px(head - edge)).child(header))
            .child(div().absolute().top(px(head)).right(px(edge)).bottom(px(hbar)).w(px(bar - edge)).child(ScrollBar::vertical(SharedString::from(format!("{}-v", self.id)), base_handle.clone())));
        if sideways {
            let corner = canvas(|_, _, _| {}, |b, _, window, cx| style::fill_rect(window, b, style::c(theme::palette(cx).panel_background))).size_full();
            root = root
                .child(div().absolute().left(px(edge)).right(px(bar)).bottom(px(edge)).h(px(bar - edge)).child(ScrollBar::horizontal(SharedString::from(format!("{}-h", self.id)), base_handle)))
                .child(div().absolute().right(px(edge)).bottom(px(edge)).w(px(bar - edge)).h(px(bar - edge)).child(corner));
        }

        // Keys: moving (with ⇧, growing from the anchor), opening, all,
        // typing to a name.
        let step: Rc<dyn Fn(Option<isize>, Option<usize>, bool, &mut Window, &mut App)> = {
            let (model, handlers) = (model.clone(), handlers.clone());
            Rc::new(move |d, to, extend, window, cx| {
                let target = to.or_else(|| d.and_then(|d| model.read(cx).state.step(d, rows)));
                let Some(n) = target.filter(|&n| n < rows) else { return };
                let changed = model.update(cx, |m, _| {
                    let before = m.state.selected.clone();
                    m.state.move_to(n, extend);
                    m.scroll_to(n, rows, row_h);
                    m.state.selected != before
                });
                if changed {
                    handlers.selected(window, cx);
                }
                window.refresh();
            })
        };
        // `by` rows on, or `pages` pages, or to `to`.
        let key = |by: isize, pages: isize, to: Option<usize>, extend: bool| {
            let (step, model) = (step.clone(), model.clone());
            move |window: &mut Window, cx: &mut App| {
                let page = ((f32::from(model.read(cx).body.size.height) - 4.0) / row_h).floor().max(1.0) as isize;
                let d = (to.is_none()).then_some(by + pages * page);
                step(d, to, extend, window, cx)
            }
        };
        let last = Some(rows.saturating_sub(1));
        let (down, up, page_down, page_up, home, end) = (key(1, 0, None, false), key(-1, 0, None, false), key(0, 1, None, false), key(0, -1, None, false), key(0, 0, Some(0), false), key(0, 0, last, false));
        let (s_down, s_up, s_page_down, s_page_up, s_home, s_end) = (key(1, 0, None, true), key(-1, 0, None, true), key(0, 1, None, true), key(0, -1, None, true), key(0, 0, Some(0), true), key(0, 0, last, true));
        root = root
            .on_action(move |_: &Down, w, cx| down(w, cx))
            .on_action(move |_: &Up, w, cx| up(w, cx))
            .on_action(move |_: &PageDown, w, cx| page_down(w, cx))
            .on_action(move |_: &PageUp, w, cx| page_up(w, cx))
            .on_action(move |_: &Home, w, cx| home(w, cx))
            .on_action(move |_: &End, w, cx| end(w, cx))
            .on_action(move |_: &SelectDown, w, cx| s_down(w, cx))
            .on_action(move |_: &SelectUp, w, cx| s_up(w, cx))
            .on_action(move |_: &SelectPageDown, w, cx| s_page_down(w, cx))
            .on_action(move |_: &SelectPageUp, w, cx| s_page_up(w, cx))
            .on_action(move |_: &SelectHome, w, cx| s_home(w, cx))
            .on_action(move |_: &SelectEnd, w, cx| s_end(w, cx))
            .on_action({
                let (model, handlers) = (model.clone(), handlers.clone());
                move |_: &Open, window, cx| {
                    if let (Some(r), Some(f)) = (model.read(cx).state.cursor(), &handlers.open) {
                        f(r, window, cx);
                    }
                }
            })
            .on_action({
                let (model, handlers) = (model.clone(), handlers);
                move |_: &SelectAll, window, cx| {
                    model.update(cx, |m, _| m.state.select_all(rows));
                    handlers.selected(window, cx);
                    window.refresh();
                }
            });
        if let Some(names) = self.names.clone() {
            root = root.on_key_down(move |e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                // (⌘, Ctrl and Alt with them are shortcuts.)
                if k.modifiers.control || k.modifiers.platform || k.modifiers.alt {
                    return;
                }
                let Some(typed) = k.key_char.clone() else { return };
                let names: Vec<SharedString> = (0..rows).map(|r| names(r)).collect();
                let to = model.update(cx, |m, _| {
                    let from = m.state.cursor();
                    m.typing.typed(&typed, rows, from, |i| names[i].as_ref())
                });
                if let Some(to) = to {
                    step(None, Some(to), false, window, cx);
                    cx.stop_propagation();
                }
            });
        }
        root.child(frame)
    }
}
