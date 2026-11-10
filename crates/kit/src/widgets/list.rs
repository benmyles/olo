//! BColumnListView: a sunken list with a header of columns (a click
//! sorts, again reverses; the dividers drag to resize; a title dragged
//! sideways moves its column, the others making room as it goes, as
//! BColumnListView's did; the order and widths kept for the app by the
//! list's id), the selection every file manager has ([`ListState`]: a
//! click, ⇧ for a range, ⌘ to add or take away, a band dragged over
//! rows, the arrows, Page Up and Down, Home and End, ⇧ with them to grow
//! it, ⌘A all, typing to go to a name), a double click or Return to
//! open, a right click for a context menu, and BeOS scroll bars
//! (sideways too, when the columns are wider together than the view).
//!
//! The app's view keeps the list's state in a [`ListModel`] and draws
//! each cell itself ([`Cell`]). Only the rows near what shows exist
//! (`uniform_rows`): each is an element of its own in the window's
//! tree, drawn once and kept (while its [`ColumnListView::row_key`]
//! says it shows the same), and the window's stage scrolls them with
//! nothing drawn again; the window makes more, a run at a time, as they
//! come near, without the app's view saying what it shows again. Agents read each row
//! (`Row`, named by its cells' text, selected or not) and each column
//! (`ColumnHeader`, sorted or not).

use std::{
    cell::{Ref, RefCell, RefMut},
    collections::HashMap,
    rc::Rc,
};

use olo_interface::{
    Bounds, ContentMask, CursorStyle, Cx, Element, FocusHandle, Font, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Painter, ParentElement as _, PathBuilder, Pixels, Point, ScrollHandle, SharedString, Styled as _, TextRun, View, WindowCx, agent, canvas, control_canvas, div, point, px, size, uniform_rows,
    view_canvas,
};
use olo_kit_base::{
    list::{ListState, Mods, SelectionBand, TypeSelect},
    metrics as m,
};

use super::scroll::{SCROLL_BAR, ScrollBar};
use crate::{style, theme};

/// The header's height.
pub const HEADER_HEIGHT: f32 = 18.0;
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
    /// A column's title held (by its own index, wherever it shows): it
    /// sorts at the release over it, or, dragged sideways (`moved`), it
    /// moves the column.
    Title { column: usize, x: Pixels, moved: bool },
    /// A divider dragged: which column's, from where, at what width.
    Divider { column: usize, x: Pixels, width: f32 },
}

struct Model {
    state: ListState,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// The widths the person dragged columns to, by their titles (a
    /// table whose columns come and go, as Activity's GPUs', keeps them).
    widths: HashMap<SharedString, f32>,
    /// The columns left to right, by their titles, as the person moved
    /// them (empty: as given; a column not in it where it was given).
    order: Vec<SharedString>,
    /// The order and widths kept for the list's id were read.
    arranged: bool,
    typing: TypeSelect,
    drag: Option<Drag>,
    /// How many rows it had and how tall each was, as last shown.
    rows: usize,
    row_height: f32,
}

/// A list's state, kept by the app's view from one `body` to the next
/// (a handle: clones are the same list).
#[derive(Clone)]
pub struct ListModel(Rc<RefCell<Model>>);

impl ListModel {
    /// A list's state; `multiple`: more than one row may be selected.
    pub fn new(multiple: bool) -> ListModel {
        ListModel(Rc::new(RefCell::new(Model {
            state: ListState { multiple, ..Default::default() },
            scroll: ScrollHandle::new(),
            focus: FocusHandle::new(),
            widths: HashMap::new(),
            order: Vec::new(),
            arranged: false,
            typing: TypeSelect::default(),
            drag: None,
            rows: 0,
            row_height: m::LIST_ROW,
        })))
    }

    /// What's selected, and how it's sorted.
    pub fn state(&self) -> Ref<'_, ListState> {
        Ref::map(self.0.borrow(), |m| &m.state)
    }

    /// The same, to change (the view says what it shows again after:
    /// `cx.invalidate()`).
    pub fn state_mut(&self) -> RefMut<'_, ListState> {
        RefMut::map(self.0.borrow_mut(), |m| &mut m.state)
    }

    fn scroll(&self) -> ScrollHandle {
        self.0.borrow().scroll.clone()
    }

    /// Scrolls so `row` shows, in the middle when it didn't (a selection
    /// made in code: what's round it shows too).
    pub fn reveal(&self, cx: &mut WindowCx, row: usize) {
        let (scroll, row_h) = (self.scroll(), self.0.borrow().row_height);
        let view = f32::from(scroll.bounds().size.height);
        let top_now = -f32::from(scroll.offset().y);
        let (top, bottom) = (row as f32 * row_h, (row + 1) as f32 * row_h);
        if top < top_now || bottom > top_now + view {
            scroll.set_offset(cx, point(scroll.offset().x, px(-(top - (view - row_h) / 2.0).max(0.0))));
        }
    }

    /// Whether its last row shows (scrolled to the end, or all fit): a
    /// list that follows what's added (Log Viewer's live lines) keeps
    /// following only then.
    pub fn at_end(&self) -> bool {
        let h = self.scroll();
        f32::from(h.offset().y) <= -f32::from(h.max_offset().height) + 1.0
    }

    /// Scrolls to its last row, once the rows it has now are laid out.
    pub fn scroll_to_end(&self, cx: &mut WindowCx) {
        self.scroll().scroll_to_bottom(cx);
    }

    /// `columns` left to right, by their own indices: as the person
    /// moved them, else as given. What an agent reads as the order of
    /// the `ColumnHeader`s.
    pub fn column_order(&self, columns: &[Column]) -> Vec<usize> {
        order(&self.0.borrow(), columns)
    }

    /// Show `columns` in `order` (their own indices, left to right): an
    /// app's arrangement.
    pub fn set_column_order(&self, columns: &[Column], order: &[usize]) {
        self.0.borrow_mut().order = order.iter().filter_map(|&c| columns.get(c)).map(|c| c.title.clone()).collect();
    }

    /// Gives the list the keyboard.
    pub fn focus(&self, cx: &mut WindowCx) {
        let focus = self.0.borrow().focus.clone();
        focus.focus(cx);
    }

    /// Whether the list has the keyboard.
    pub fn is_focused(&self, cx: &WindowCx) -> bool {
        self.0.borrow().focus.is_focused(cx)
    }

    /// A name is being typed to select (a space now is part of it, not
    /// Quick Look's).
    pub fn typing(&self) -> bool {
        self.0.borrow().typing.typing()
    }

    /// Scrolls the least that shows `row` (the keyboard moving).
    fn scroll_to(&self, cx: &mut WindowCx, row: usize) {
        let (scroll, rows, row_h) = {
            let m = self.0.borrow();
            (m.scroll.clone(), m.rows, m.row_height)
        };
        let view = f32::from(scroll.bounds().size.height);
        let top_now = -f32::from(scroll.offset().y);
        let (top, bottom) = (row as f32 * row_h, (row + 1) as f32 * row_h);
        let to = if top < top_now {
            top
        } else if bottom > top_now + view {
            bottom - view
        } else {
            return;
        };
        let max = (rows as f32 * row_h - view).max(0.0);
        scroll.set_offset(cx, point(scroll.offset().x, px(-to.clamp(0.0, max))));
    }

    /// The row at `p` (in the window), the rows showing in `body`.
    fn row_at(&self, p: Point<Pixels>, body: Bounds<Pixels>) -> Option<usize> {
        let m = self.0.borrow();
        let y = f32::from(p.y - body.top() - m.scroll.offset().y);
        (y >= 0.0 && body.contains(&p)).then(|| (y / m.row_height) as usize).filter(|&r| r < m.rows)
    }

    /// `p` (in the window) in the content's coordinates.
    fn content(&self, p: Point<Pixels>, body: Bounds<Pixels>) -> Point<Pixels> {
        let off = self.scroll().offset();
        point(p.x - body.left() - off.x, p.y - body.top() - off.y)
    }

    /// A band from where it started to the pointer selects the rows it
    /// covers (with what it started from); whether that changed.
    fn band_over(&self, body: Bounds<Pixels>) -> bool {
        let (from, now, base) = match &self.0.borrow().drag {
            Some(Drag::Press { from, now, base, .. }) => (*from, *now, base.clone()),
            _ => return false,
        };
        let now_y = f32::from(self.content(now, body).y);
        let mut m = self.0.borrow_mut();
        let row_h = m.row_height;
        let (top, bottom) = (f32::from(from.y).min(now_y), f32::from(from.y).max(now_y));
        let first = (top / row_h).floor().max(0.0) as usize;
        let end = ((bottom / row_h).ceil().max(0.0) as usize).min(m.rows);
        let selected = base.over(first..end.max(first));
        let changed = selected != m.state.selected;
        m.state.selected = selected;
        changed
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
    pub fn text(&mut self, g: &mut Painter, text: impl Into<SharedString>) {
        let color = self.text_color;
        self.text_with(g, text, style::ui_font(FontWeight::NORMAL), color);
    }

    /// Its text in runs (a log line's colours, what a search found marked
    /// behind), cut short with "…" when it doesn't fit.
    pub fn text_runs(&mut self, g: &mut Painter, text: impl Into<SharedString>, runs: &[TextRun]) {
        let text = text.into();
        let inset = px(m::CELL_INSET);
        let x = match self.align {
            style::Align::Left => self.bounds.left() + inset,
            style::Align::Center => self.bounds.center().x,
            style::Align::Right => self.bounds.right() - inset,
        };
        style::label_runs_fit(g, x, self.bounds.center().y, self.align, text.clone(), style::PLAIN_SIZE, runs, self.bounds.size.width - inset * 2.0);
        self.texts.push(text);
    }

    /// What agents read of a cell the caller draws itself (its row is
    /// named by its cells' text).
    pub fn say(&mut self, text: impl Into<SharedString>) {
        self.texts.push(text.into());
    }

    /// Its text in `font` and `color`.
    pub fn text_with(&mut self, g: &mut Painter, text: impl Into<SharedString>, font: Font, color: Hsla) {
        let text = text.into();
        cell_text(g, self.bounds, self.align, text.clone(), font, color);
        self.texts.push(text);
    }

    /// A 16-point icon at its left (`icon`, else `fallback`), then its
    /// text: an app's or a file's name.
    pub fn icon_text(&mut self, g: &mut Painter, icon: &str, fallback: &str, text: impl Into<SharedString>) {
        let text = text.into();
        let at = Bounds::centered_at(point(self.bounds.left() + px(m::CELL_INSET + 8.0), self.bounds.center().y), size(px(16.0), px(16.0)));
        if !crate::icons::paint(g, at, icon) {
            crate::icons::paint(g, at, fallback);
        }
        let rest = Bounds::from_corners(point(at.right() + px(2.0), self.bounds.top()), self.bounds.bottom_right());
        let color = self.text_color;
        cell_text(g, rest, style::Align::Left, text.clone(), style::ui_font(FontWeight::NORMAL), color);
        self.texts.push(text);
    }
}

/// Text in a cell `b`: inset, kept to `align`'s side, cut short to fit.
fn cell_text(g: &mut Painter, b: Bounds<Pixels>, align: style::Align, text: SharedString, font: Font, color: Hsla) {
    let inset = px(m::CELL_INSET);
    let x = match align {
        style::Align::Left => b.left() + inset,
        style::Align::Center => b.center().x,
        style::Align::Right => b.right() - inset,
    };
    style::label_fit(g, x, b.center().y, align, text, style::PLAIN_SIZE, font, color, b.size.width - inset * 2.0);
}

type RowFn = Rc<dyn Fn(&usize, &mut WindowCx)>;
type AtFn = Rc<dyn Fn(&(usize, Point<Pixels>), &mut WindowCx)>;
type PressFn = Rc<dyn Fn(&(usize, Point<Pixels>), &mut WindowCx) -> Option<bool>>;
type Handler = Rc<dyn Fn(&(), &mut WindowCx)>;

/// What the app is told.
#[derive(Clone, Default)]
struct Handlers {
    open: Option<RowFn>,
    /// A row dragged out of the list (Disk Usage's Collector, Tracker).
    drag: Option<RowFn>,
    context: Option<AtFn>,
    /// A press on a row, before it selects: taken (true) or not.
    press: Option<PressFn>,
    select: Option<Handler>,
    sort: Option<Handler>,
}

impl Handlers {
    /// The selection changed: the app hears, and the view that holds
    /// the list says what it shows again (its rows' look).
    fn selected(&self, cx: &mut WindowCx) {
        cx.invalidate_owner();
        if let Some(f) = &self.select {
            f(&(), cx);
        }
    }
}

/// The list, drawn from its [`ListModel`], for a view of type `V` (its
/// cells are drawn from the view as it is when they're wanted).
pub struct ColumnListView<V> {
    id: SharedString,
    model: ListModel,
    columns: Rc<[Column]>,
    rows: usize,
    row_height: f32,
    cell: Rc<dyn Fn(&V, &mut Cell, &mut Painter)>,
    names: Option<Rc<dyn Fn(&V, usize) -> SharedString>>,
    empty: Option<SharedString>,
    header: bool,
    handlers: Handlers,
    row_key: Option<Rc<dyn Fn(&V, usize) -> u64>>,
    /// The list has the keyboard (as its view's `body` found it).
    focused: bool,
}

impl<V: View> ColumnListView<V> {
    /// `rows` rows of `columns`, each cell drawn by `cell` from the
    /// view.
    pub fn new(cx: &Cx<V>, id: impl Into<SharedString>, model: &ListModel, columns: impl Into<Rc<[Column]>>, rows: usize, cell: impl Fn(&V, &mut Cell, &mut Painter) + 'static) -> ColumnListView<V> {
        ColumnListView {
            id: id.into(),
            model: model.clone(),
            columns: columns.into(),
            rows,
            row_height: m::LIST_ROW,
            cell: Rc::new(cell),
            names: None,
            empty: None,
            header: true,
            handlers: Handlers::default(),
            row_key: None,
            focused: model.is_focused(cx),
        }
    }

    /// What row `row` shows, as a number that changes when it does (its
    /// line's id and what a search marks in it, say): a row whose key,
    /// and whose look (selected, the columns' widths), are as they were
    /// keeps its drawing when the view says what it shows again. Without
    /// it every row near what shows is drawn again each time.
    pub fn row_key(mut self, key: impl Fn(&V, usize) -> u64 + 'static) -> Self {
        self.row_key = Some(Rc::new(key));
        self
    }

    pub fn row_height(mut self, h: f32) -> Self {
        self.row_height = h;
        self
    }

    /// No header: a list of things rather than a table (Passwords'
    /// items), its one column as wide as the list.
    pub fn no_header(mut self) -> Self {
        self.header = false;
        self
    }

    /// Typing goes to the row whose name (`name(view, row)`) starts so.
    pub fn type_select(mut self, name: impl Fn(&V, usize) -> SharedString + 'static) -> Self {
        self.names = Some(Rc::new(name));
        self
    }

    /// Said in the middle while there are no rows ("No lines match.").
    pub fn empty_text(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = Some(text.into());
        self
    }

    /// A row double-clicked, or Return on it
    /// (`cx.msg_with(|row: &usize| Msg::Open(*row))`).
    pub fn on_open(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.handlers.open = Some(Rc::new(f));
        self
    }

    /// A row right-clicked, where (for a context menu); the selection is
    /// it, or the rows selected with it.
    pub fn on_context(mut self, f: impl Fn(&(usize, Point<Pixels>), &mut WindowCx) + 'static) -> Self {
        self.handlers.context = Some(Rc::new(f));
        self
    }

    /// Rows can be dragged out: a press on a selected row, or on a
    /// row's first column, that moves starts a drag of it (selected, if
    /// it wasn't), which `f` takes on (following the pointer, dropping
    /// it); elsewhere a press that moves draws a band.
    pub fn on_drag(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.handlers.drag = Some(Rc::new(f));
        self
    }

    /// A row pressed (the left button), where: `f` takes it (`Some(true)`:
    /// an outline's disclosure triangle, say) or leaves it to select
    /// (`cx.asks(..)` makes one that asks the view).
    pub fn on_press(mut self, f: impl Fn(&(usize, Point<Pixels>), &mut WindowCx) -> Option<bool> + 'static) -> Self {
        self.handlers.press = Some(Rc::new(f));
        self
    }

    /// The selection changed (`cx.msg(Msg::Selected)`).
    pub fn on_select(mut self, f: impl Fn(&(), &mut WindowCx) + 'static) -> Self {
        self.handlers.select = Some(Rc::new(f));
        self
    }

    /// The sort changed (the model's `state().sort`): sort the rows
    /// again.
    pub fn on_sort(mut self, f: impl Fn(&(), &mut WindowCx) + 'static) -> Self {
        self.handlers.sort = Some(Rc::new(f));
        self
    }
}

/// The columns' widths, by their own indices: as dragged, else as given.
fn widths(model: &Model, columns: &[Column]) -> Vec<f32> {
    columns.iter().map(|c| model.widths.get(&c.title).copied().unwrap_or(c.width)).collect()
}

/// The columns left to right, by their own indices: as moved, those
/// not moved (new since) where they were given.
fn order(model: &Model, columns: &[Column]) -> Vec<usize> {
    if model.order.is_empty() {
        return (0..columns.len()).collect();
    }
    let mut used = vec![false; columns.len()];
    let mut out: Vec<usize> = model
        .order
        .iter()
        .filter_map(|t| {
            let c = columns.iter().enumerate().position(|(i, c)| !used[i] && &c.title == t)?;
            used[c] = true;
            Some(c)
        })
        .collect();
    // (A column new since goes after the one given before it.)
    for c in 0..columns.len() {
        if !used[c] {
            let at = if c == 0 { 0 } else { out.iter().position(|&o| o == c - 1).map_or(out.len(), |p| p + 1) };
            out.insert(at, c);
            used[c] = true;
        }
    }
    out
}

/// The columns' widths left to right.
fn shown_widths(model: &Model, columns: &[Column]) -> Vec<f32> {
    let w = widths(model, columns);
    order(model, columns).into_iter().map(|c| w[c]).collect()
}

/// The order and widths kept for list `id` (by the columns' titles), put
/// in `model` as it's first shown.
fn arrange(model: &mut Model, id: &str, columns: &[Column]) {
    model.arranged = true;
    let Some(kept) = crate::kept::table(id) else { return };
    model.order = kept.order.into_iter().map(SharedString::from).collect();
    model.widths = kept
        .widths
        .into_iter()
        .map(|(t, w)| {
            let least = columns.iter().find(|c| c.title.as_ref() == t).map_or(0.0, |c| c.min_width);
            (SharedString::from(t), w.max(least))
        })
        .collect();
}

/// List `id`'s columns as arranged now, kept (by their titles).
fn note_arranged(model: &Model, id: &str, columns: &[Column]) {
    // (Titles kept but not shown now, as a GPU's column without the GPU,
    // keep their places: after the column before them.)
    let mut order: Vec<String> = order(model, columns).into_iter().map(|c| columns[c].title.to_string()).collect();
    for (i, t) in model.order.iter().enumerate() {
        if !order.iter().any(|o| o == t.as_ref()) {
            let at = i.checked_sub(1).and_then(|p| order.iter().position(|o| o == model.order[p].as_ref())).map_or(order.len(), |p| p + 1);
            order.insert(at, t.to_string());
        }
    }
    let widths = model.widths.iter().map(|(t, w)| (t.to_string(), *w)).collect();
    crate::kept::note_table(id, crate::kept::TableLayout { order, widths });
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
fn sort_arrow(g: &mut Painter, x: Pixels, y: Pixels, ascending: bool, color: Hsla) {
    let (tip, base) = if ascending { (y - px(2.0), y + px(2.0)) } else { (y + px(2.0), y - px(2.0)) };
    let mut path = PathBuilder::fill();
    path.move_to(point(x - px(3.5), base));
    path.line_to(point(x + px(3.5), base));
    path.line_to(point(x, tip));
    path.close();
    if let Ok(path) = path.build() {
        g.paint_path(path, color);
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

/// Where the list's parts go, in points on device pixels.
#[derive(Clone, Copy)]
struct Parts {
    /// Inside the sunken frame's two lines.
    edge: f32,
    /// The header's bottom (the frame's edge, with none).
    head: f32,
    /// The scroll bars' inner edges, from the list's right and bottom.
    bar: f32,
}

impl<V: View> IntoElement for ColumnListView<V> {
    fn into_element(self) -> Element {
        let model = self.model.clone();
        // (The person's arrangement of its columns, as they left it.)
        if self.header && !model.0.borrow().arranged {
            arrange(&mut model.0.borrow_mut(), &self.id, &self.columns);
        }
        {
            let mut m = model.0.borrow_mut();
            m.rows = self.rows;
            m.row_height = self.row_height;
        }
        let (rows, row_h, columns, handlers, focused) = (self.rows, self.row_height, self.columns.clone(), self.handlers.clone(), self.focused);
        let (scroll, focus) = {
            let m = model.0.borrow();
            (m.scroll.clone(), m.focus.clone())
        };
        let total: f32 = widths(&model.0.borrow(), &columns).iter().sum();
        let first_width = shown_widths(&model.0.borrow(), &columns).first().copied().unwrap_or(0.0);
        // Columns wider together than the view scroll sideways (as the
        // view was last laid out: it says so again when that changes).
        let view = scroll.bounds().size;
        let sideways = view.width > px(0.0) && total > f32::from(view.width) + 0.5;
        // Inside the sunken frame's two lines, on device pixels; the
        // header's bottom and the bars' edges too (layout rounds a box's
        // offset from its parent but its size from where it ends, so a
        // header 22.5 device pixels tall at 1.25× and rows 24.5 down
        // round a pixel apart).
        let s = olo_interface::scale_factor();
        let dev = |v: f32| (v * s).round() / s;
        let edge = style::line_pixels(s) * 2.0 / s;
        let parts = Parts { edge, head: if self.header { dev(edge + HEADER_HEIGHT) } else { edge }, bar: dev(edge + SCROLL_BAR) };
        let hbar = if sideways { parts.bar } else { edge };

        // The rows, each an element of its own drawing its cells: made
        // by the window as they come near what shows.
        let make = {
            let (model, columns, cell, row_key, id) = (model.clone(), columns.clone(), self.cell.clone(), self.row_key.clone(), self.id.clone());
            move |near: std::ops::Range<usize>| -> Vec<Element> {
                let m = model.0.borrow();
                let (widths, order) = (Rc::<[f32]>::from(shown_widths(&m, &columns)), Rc::<[usize]>::from(order(&m, &columns)));
                let look = style::key((&id, widths.iter().map(|w| w.to_bits()).collect::<Vec<u32>>(), &order, sideways));
                near.map(|row| {
                    let selected = m.state.selected.contains(&row);
                    let (cell, columns, widths, order) = (cell.clone(), columns.clone(), widths.clone(), order.clone());
                    let mut row_el = view_canvas::<V>(move |view, g, r| paint_row(view, g, r, row, selected, focused, sideways, &columns, &widths, &order, &*cell));
                    if let Some(key) = row_key.clone() {
                        // (Focus shows only in a selected row.)
                        row_el = row_el.key_by::<V>(move |view| style::key((look, key(view, row), selected, selected && focused)));
                    }
                    let row_el = row_el.id(row);
                    if sideways { row_el.w(px(total)) } else { row_el.w_full() }
                })
                .collect()
            }
        };
        let mut content = uniform_rows(rows, px(row_h), make);
        if sideways {
            content = content.w(px(total));
        }
        let mut list = div().absolute().inset_0().track_scroll(&scroll);
        list = if sideways { list.overflow_scroll() } else { list.overflow_y_scroll() };
        // Sideways or not as the view's width changes: the view that
        // holds the list says what it shows again.
        let (near_scroll, near_columns, near_model) = (scroll.clone(), columns.clone(), model.clone());
        list = list.on_scroll(move |_, cx| {
            let view = near_scroll.bounds().size;
            let total: f32 = widths(&near_model.0.borrow(), &near_columns).iter().sum();
            if (view.width > px(0.0) && total > f32::from(view.width) + 0.5) != sideways {
                cx.invalidate_owner();
            }
        });

        // Over the rows: the pointer (a press, a band, a right click) and
        // the band drawn.
        let (band_model, down_model, move_model, up_model) = (model.clone(), model.clone(), model.clone(), model.clone());
        let (down_handlers, move_handlers, up_handlers) = (handlers.clone(), handlers.clone(), handlers.clone());
        let pointer = canvas(move |g, b| {
            crate::review::scroll_area(g, b);
            let m = band_model.0.borrow();
            if let Some(Drag::Press { from, now, band: true, .. }) = &m.drag {
                let off = m.scroll.offset();
                let start = point(b.left() + off.x + from.x, b.top() + off.y + from.y);
                let r = Bounds::from_corners(point(start.x.min(now.x), start.y.min(now.y)), point(start.x.max(now.x), start.y.max(now.y)));
                let p = theme::palette();
                g.with_content_mask(Some(ContentMask { bounds: b }), |g| style::selection_band(g, r, &p));
            }
        })
        .paint_key(0)
        .absolute()
        .inset_0()
        .on_any_mouse_down(move |e, cx| rows_pressed(&down_model, &down_handlers, e, cx))
        .on_window_mouse_move(move |e, cx| rows_dragged(&move_model, &move_handlers, first_width, e, cx))
        .on_window_mouse_up(move |e, cx| rows_released(&up_model, &up_handlers, e, cx));

        // Behind the rows (where layout put them, as it put the header):
        // what's said when there are none.
        let empty = (rows == 0).then(|| self.empty.clone()).flatten();
        let ground = canvas({
            let empty = empty.clone();
            move |g, b| {
                let p = theme::palette();
                if let Some(text) = &empty {
                    let dim = style::c(olo_theme::mix(p.list_item_text, p.list_background, 0.5));
                    let font = style::ui_font(FontWeight::NORMAL);
                    style::label_fit(g, b.center().x, b.center().y, style::Align::Center, text.clone(), style::PLAIN_SIZE, font, dim, b.size.width - px(2.0 * m::SPACING));
                }
            }
        })
        .paint_key(style::key(&empty))
        .absolute()
        .inset_0();

        // The list's own ground and frame, inside the frame's two lines
        // as it draws them (on device pixels from the list's own edges);
        // the header and bars go over it. It has the keyboard.
        let mut root = control_canvas(|g, b, state| {
            let p = theme::palette();
            style::fill_rect(g, style::inset(g, b, 2.0), style::c(p.list_background));
            style::sunken(g, b, &p, state.focused);
        })
        .paint_key(0)
        .relative()
        .size_full()
        .track_focus(&focus)
        .tab_stop()
        // (A selected row shows whether the list has the keyboard.)
        .on_focus_changed(|_, cx| cx.invalidate_owner())
        .child(div().absolute().left(px(edge)).top(px(parts.head)).right(px(parts.bar)).bottom(px(hbar)).child(ground).child(list.child(content)).child(pointer));
        if self.header {
            root = root.child(div().absolute().left(px(edge)).top(px(edge)).right(px(edge)).h(px(parts.head - edge)).child(header(&self.id, &model, &columns, &handlers, parts, sideways)));
        }
        root = root.child(div().absolute().top(px(parts.head)).right(px(edge)).bottom(px(hbar)).w(px(parts.bar - edge)).child(ScrollBar::vertical(&scroll)));
        if sideways {
            let corner = canvas(|g, b| style::fill_rect(g, b, style::c(theme::palette().panel_background))).paint_key(0).size_full();
            root = root
                .child(div().absolute().left(px(edge)).right(px(parts.bar)).bottom(px(edge)).h(px(parts.bar - edge)).child(ScrollBar::horizontal(&scroll)))
                .child(div().absolute().right(px(edge)).bottom(px(edge)).w(px(parts.bar - edge)).h(px(parts.bar - edge)).child(corner));
        }

        // Keys: moving (with ⇧, growing from the anchor), opening, all,
        // typing to a name.
        let names = self.names.clone();
        let key_model = model.clone();
        root.on_key_down(move |e: &KeyDownEvent, cx| {
            let k = &e.keystroke;
            let command = k.modifiers.control || k.modifiers.platform;
            let page = ((f32::from(key_model.scroll().bounds().size.height) - 4.0) / row_h).floor().max(1.0) as isize;
            let last = rows.saturating_sub(1);
            // `by` rows on, or to a row.
            let (by, to): (Option<isize>, Option<usize>) = match (k.key.as_str(), command, k.modifiers.alt) {
                ("down", false, false) => (Some(1), None),
                ("up", false, false) => (Some(-1), None),
                ("pagedown", false, false) => (Some(page), None),
                ("pageup", false, false) => (Some(-page), None),
                ("home", false, false) => (None, Some(0)),
                ("end", false, false) => (None, Some(last)),
                ("enter", false, false) if !k.modifiers.shift => {
                    let at = key_model.state().cursor();
                    if let (Some(r), Some(f)) = (at, &handlers.open) {
                        f(&r, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                ("a", true, false) if !k.modifiers.shift => {
                    key_model.state_mut().select_all(rows);
                    handlers.selected(cx);
                    cx.stop_propagation();
                    return;
                }
                _ => {
                    // Typing goes to the row whose name starts so. (⌘,
                    // Ctrl and Alt with a key are shortcuts.)
                    let Some(names) = &names else { return };
                    if command || k.modifiers.alt {
                        return;
                    }
                    let Some(typed) = k.key_char.clone() else { return };
                    let Some(all) = cx.owner_view::<V>().map(|view| (0..rows).map(|r| names(view, r)).collect::<Vec<SharedString>>()) else { return };
                    let to = {
                        let mut m = key_model.0.borrow_mut();
                        let from = m.state.cursor();
                        m.typing.typed(&typed, rows, from, |i| all[i].as_ref())
                    };
                    if let Some(to) = to {
                        move_to(&key_model, &handlers, to, false, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
            };
            let target = to.or_else(|| by.and_then(|d| key_model.state().step(d, rows)));
            if let Some(n) = target.filter(|&n| n < rows) {
                move_to(&key_model, &handlers, n, k.modifiers.shift, cx);
            }
            cx.stop_propagation();
        })
    }
}

/// The keyboard goes to row `n` (growing the selection, `extend`), which
/// is scrolled into view.
fn move_to(model: &ListModel, handlers: &Handlers, n: usize, extend: bool, cx: &mut WindowCx) {
    let changed = {
        let mut m = model.0.borrow_mut();
        let before = m.state.selected.clone();
        m.state.move_to(n, extend);
        m.state.selected != before
    };
    model.scroll_to(cx, n);
    if changed {
        handlers.selected(cx);
    }
}

/// Draws a row: its ground when it's selected, then each cell, by its
/// column's own index wherever the column shows.
#[allow(clippy::too_many_arguments)]
fn paint_row<V>(
    view: &V,
    g: &mut Painter,
    r: Bounds<Pixels>,
    row: usize,
    selected: bool,
    focused: bool,
    sideways: bool,
    columns: &[Column],
    widths: &[f32],
    order: &[usize],
    cell: &dyn Fn(&V, &mut Cell, &mut Painter),
) {
    let p = theme::palette();
    if selected {
        let sel = if focused { p.list_selected_background } else { olo_theme::mix(p.list_selected_background, p.list_background, 0.35) };
        style::fill_rect(g, r, style::c(sel));
    }
    let text_color = style::c(if selected { p.list_selected_item_text } else { p.list_item_text });
    let mut texts = Vec::new();
    for (at, (x0, x1)) in spans(widths, r.left(), r.right(), sideways).into_iter().enumerate() {
        let i = order[at];
        let b = Bounds::from_corners(point(x0, r.top()), point(x1, r.bottom()));
        g.with_content_mask(Some(ContentMask { bounds: b }), |g| {
            cell(view, &mut Cell { bounds: b, row, column: i, selected, text_color, align: columns[i].align, texts: &mut texts }, g);
        });
    }
    let name = texts.iter().map(|t| t.as_ref()).collect::<Vec<_>>().join("  ");
    g.agent(agent::Node { role: "Row", name: Some(name.into()), bounds: r, enabled: true, clickable: true, selected: Some(selected), ..Default::default() });
}

/// A button went down over the rows.
fn rows_pressed(model: &ListModel, h: &Handlers, e: &MouseDownEvent, cx: &mut WindowCx) {
    let body = cx.listener_bounds();
    model.focus(cx);
    let mods = Mods { command: e.modifiers.control || e.modifiers.platform, shift: e.modifiers.shift };
    let row = model.row_at(e.position, body);
    match e.button {
        MouseButton::Left if e.click_count >= 2 => {
            if let (Some(r), Some(f)) = (row, &h.open) {
                f(&r, cx);
            }
        }
        MouseButton::Left => {
            if let (Some(r), Some(f)) = (row, &h.press)
                && f(&(r, e.position), cx) == Some(true)
            {
                cx.invalidate_owner();
                return;
            }
            let from = model.content(e.position, body);
            let changed = {
                let mut m = model.0.borrow_mut();
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
                m.drag = Some(Drag::Press { from, at: e.position, row, base, band: false, now: e.position, defer });
                m.state.selected != before
            };
            if changed {
                h.selected(cx);
            }
        }
        MouseButton::Right => {
            let Some(r) = row else { return };
            // A selection of several is kept for the menu.
            if !model.state().selected.contains(&r) {
                model.state_mut().select(r);
                h.selected(cx);
            }
            if let Some(f) = &h.context {
                f(&(r, e.position), cx);
            }
        }
        _ => {}
    }
}

/// The pointer moved with something of the rows held.
fn rows_dragged(model: &ListModel, h: &Handlers, first_width: f32, e: &MouseMoveEvent, cx: &mut WindowCx) {
    if !e.dragging() {
        return;
    }
    let body = cx.listener_bounds();
    let drag = model.0.borrow().drag.clone();
    // A press on a selected row, or on a row's first column, that moves:
    // the row dragged out.
    if let (Some(dragged), Some(Drag::Press { at, row: Some(r), band: false, .. })) = (&h.drag, &drag) {
        let d = e.position - *at;
        let on_item = model.state().selected.contains(r) || at.x < body.left() + px(first_width);
        if on_item && f32::from(d.x).hypot(f32::from(d.y)) > 3.0 {
            let fresh = {
                let mut m = model.0.borrow_mut();
                m.drag = None;
                let fresh = !m.state.selected.contains(r);
                if fresh {
                    m.state.select(*r);
                }
                fresh
            };
            if fresh {
                h.selected(cx);
            }
            dragged(r, cx);
            return;
        }
    }
    let band = {
        let mut m = model.0.borrow_mut();
        let multiple = m.state.multiple;
        let Some(Drag::Press { at, band, now, .. }) = m.drag.as_mut() else { return };
        *now = e.position;
        // A band once it has moved a little.
        if !*band {
            let d = e.position - *at;
            *band = multiple && f32::from(d.x).hypot(f32::from(d.y)) > 3.0;
        }
        *band
    };
    if band {
        if model.band_over(body) {
            h.selected(cx);
        }
        cx.repaint();
        band_scroll(model, h, cx);
    }
}

/// A band held near or past the top or bottom of the rows: on they
/// scroll, a step a frame, while it is.
fn band_scroll(model: &ListModel, h: &Handlers, cx: &mut WindowCx) {
    let Some(node) = cx.listener_node() else { return };
    let body = cx.listener_bounds();
    let (now, scroll, row_h) = {
        let m = model.0.borrow();
        let Some(Drag::Press { now, band: true, .. }) = &m.drag else { return };
        (*now, m.scroll.clone(), m.row_height)
    };
    let step = band_step(body, now.y, row_h);
    let top_now = -f32::from(scroll.offset().y);
    let to = (top_now + step).clamp(0.0, f32::from(scroll.max_offset().height));
    if to == top_now {
        return;
    }
    scroll.set_offset(cx, point(scroll.offset().x, px(-to)));
    if model.band_over(body) {
        h.selected(cx);
    }
    cx.repaint();
    let (model, h) = (model.clone(), h.clone());
    cx.on_next_frame(move |cx| cx.with_node(node, |cx| band_scroll(&model, &h, cx)));
}

/// The left button came up, wherever.
fn rows_released(model: &ListModel, h: &Handlers, e: &MouseUpEvent, cx: &mut WindowCx) {
    if e.button != MouseButton::Left {
        return;
    }
    let Some(Drag::Press { row, band, defer, .. }) = model.0.borrow().drag.clone() else { return };
    model.0.borrow_mut().drag = None;
    // A press on a selected row, not dragged: it alone.
    if let (Some(r), false, true) = (row, band, defer) {
        let changed = {
            let mut m = model.0.borrow_mut();
            let before = m.state.selected.clone();
            m.state.select(r);
            m.state.selected != before
        };
        if changed {
            h.selected(cx);
        }
    }
    if band {
        cx.repaint();
    }
}

/// The header: each column's title, sorted or not; dividers to drag;
/// titles to drag sideways.
fn header(id: &SharedString, model: &ListModel, columns: &Rc<[Column]>, handlers: &Handlers, parts: Parts, sideways: bool) -> Element {
    // What shows of it: above the rows, to the edge they end on, so a
    // column's title keeps to its cells' edge at any scale.
    let shown_of = move |b: Bounds<Pixels>| Bounds::from_corners(b.origin, point(b.right() - px(parts.bar - parts.edge), b.bottom()));
    let geometry = {
        let (model, columns) = (model.clone(), columns.clone());
        move |b: Bounds<Pixels>| {
            let m = model.0.borrow();
            let shown = shown_of(b);
            (spans(&shown_widths(&m, &columns), b.left() + m.scroll.offset().x, shown.right(), sideways), order(&m, &columns), shown)
        }
    };
    let key = {
        let m = model.0.borrow();
        let pressed = if let Some(Drag::Title { column, .. }) = m.drag { Some(column) } else { None };
        let titles: Vec<(&SharedString, u8)> = columns.iter().map(|c| (&c.title, c.align as u8)).collect();
        let widths: Vec<u32> = shown_widths(&m, columns).iter().map(|w| w.to_bits()).collect();
        style::key((titles, widths, order(&m, columns), m.state.sort, pressed, sideways))
    };
    let (paint_model, paint_columns, paint_geometry) = (model.clone(), columns.clone(), geometry.clone());
    let (down_model, down_columns, down_geometry) = (model.clone(), columns.clone(), geometry.clone());
    let (move_model, move_columns, move_geometry) = (model.clone(), columns.clone(), geometry.clone());
    let (up_model, up_columns, up_geometry, up_handlers, up_id) = (model.clone(), columns.clone(), geometry, handlers.clone(), id.clone());
    canvas(move |g, b| {
        let p = theme::palette();
        let (spans, order, shown) = paint_geometry(b);
        let m = paint_model.0.borrow();
        let (sort, pressed) = (m.state.sort, if let Some(Drag::Title { column, .. }) = m.drag { Some(column) } else { None });
        let bg = p.panel_background;
        style::vgradient(g, b, style::tint(bg, 0.8), style::shade(&p, bg, 1.04));
        style::hline_above(g, b.left(), b.right(), b.bottom(), style::shade(&p, bg, 1.45));
        // (It scrolls sideways with the rows: what it cuts is out of
        // sight, as theirs.)
        crate::review::scroll_area(g, shown);
        let last = spans.len().saturating_sub(1);
        g.with_content_mask(Some(ContentMask { bounds: shown }), |g| {
            for (at, &(x0, x1)) in spans.iter().enumerate() {
                let i = order[at];
                let col = &paint_columns[i];
                let r = Bounds::from_corners(point(x0, b.top()), point(x1, b.bottom() - px(1.0)));
                if pressed == Some(i) {
                    style::fill_rect(g, r, style::shade(&p, bg, 1.1));
                }
                // Light leading edge, dark trailing edge.
                style::vline(g, r.left(), r.top(), r.bottom(), style::c(p.shine));
                style::vline_left_of(g, r.right(), r.top() + px(2.0), r.bottom() - px(2.0), style::shade(&p, bg, 1.3));
                let sorted = sort.filter(|(c, _)| *c == i);
                let font = style::ui_font(if sorted.is_some() { FontWeight::BOLD } else { FontWeight::NORMAL });
                let text = Bounds::from_corners(r.origin, point(r.right() - px(if sorted.is_some() { 12.0 } else { 0.0 }), r.bottom()));
                cell_text(g, text, col.align, col.title.clone(), font, style::c(p.panel_text));
                if let Some((_, ascending)) = sorted {
                    let y = style::snap_coord(g, r.center().y);
                    sort_arrow(g, r.right() - px(10.0), y, ascending, style::c(p.panel_text));
                }
                // (One scrolled out of view is there, none wide, at the
                // view's edge: not to be clicked.)
                let seen = r.intersect(&shown);
                let seen = Bounds { origin: seen.origin, size: size(seen.size.width.max(px(0.0)), seen.size.height.max(px(0.0))) };
                let clickable = seen.size.width > px(0.0);
                g.agent(agent::Node { role: "ColumnHeader", name: Some(col.title.clone()), bounds: seen, enabled: true, clickable, selected: Some(sorted.is_some()), ..Default::default() });
                // Each divider resizes the column left of it.
                if at != last && x1 >= shown.left() && x1 <= shown.right() {
                    g.set_cursor_style(CursorStyle::ResizeColumn, divider_at(x1, b));
                }
            }
        });
        // Above the scroll bar: a blank header cell.
        style::vline(g, shown.right(), b.top(), b.bottom() - px(1.0), style::c(p.shine));
    })
    .paint_key(key)
    .size_full()
    .cursor(CursorStyle::Arrow)
    .watch_scroll_x(&model.scroll())
    .on_mouse_down(MouseButton::Left, move |e, cx| {
        let b = cx.listener_bounds();
        let (spans, order, shown) = down_geometry(b);
        if !shown.contains(&e.position) {
            return;
        }
        let last = spans.len().saturating_sub(1);
        let divider = spans.iter().enumerate().find(|&(at, &(_, x1))| at != last && x1 >= shown.left() && x1 <= shown.right() && divider_at(x1, b).contains(&e.position)).map(|(at, _)| order[at]);
        let drag = match divider {
            Some(column) => Drag::Divider { column, x: e.position.x, width: widths(&down_model.0.borrow(), &down_columns)[column] },
            None => match spans.iter().position(|&(x0, x1)| e.position.x >= x0 && e.position.x < x1) {
                Some(at) => Drag::Title { column: order[at], x: e.position.x, moved: false },
                None => return,
            },
        };
        down_model.0.borrow_mut().drag = Some(drag);
        cx.invalidate_owner();
        cx.stop_propagation();
    })
    .on_window_mouse_move(move |e, cx| {
        let drag = move_model.0.borrow().drag.clone();
        match drag {
            Some(Drag::Divider { column, x, width }) => {
                let w = (width + f32::from(e.position.x - x)).max(move_columns[column].min_width);
                move_model.0.borrow_mut().widths.insert(move_columns[column].title.clone(), w);
                cx.invalidate_owner();
            }
            // A title dragged sideways: its column moves past a neighbour
            // once the pointer is past the neighbour's middle (so it
            // doesn't swing back between columns of other widths).
            Some(Drag::Title { column, x, moved }) => {
                if !moved && (e.position.x - x).abs() <= px(4.0) {
                    return;
                }
                let (spans, mut ord, _) = move_geometry(cx.listener_bounds());
                let mid = |(x0, x1): (Pixels, Pixels)| (x0 + x1) / 2.0;
                if let Some(at) = ord.iter().position(|&c| c == column) {
                    if at + 1 < ord.len() && e.position.x > mid(spans[at + 1]) {
                        ord.swap(at, at + 1);
                    } else if at > 0 && e.position.x < mid(spans[at - 1]) {
                        ord.swap(at, at - 1);
                    }
                }
                let mut m = move_model.0.borrow_mut();
                m.order = ord.iter().map(|&c| move_columns[c].title.clone()).collect();
                m.drag = Some(Drag::Title { column, x, moved: true });
                drop(m);
                cx.invalidate_owner();
            }
            _ => {}
        }
    })
    .on_window_mouse_up(move |e, cx| {
        if e.button != MouseButton::Left {
            return;
        }
        let drag = up_model.0.borrow().drag.clone();
        match drag {
            Some(Drag::Title { moved: true, .. }) | Some(Drag::Divider { .. }) => {
                let mut m = up_model.0.borrow_mut();
                m.drag = None;
                note_arranged(&m, &up_id, &up_columns);
                drop(m);
                cx.invalidate_owner();
            }
            Some(Drag::Title { column, moved: false, .. }) => {
                up_model.0.borrow_mut().drag = None;
                // Released over the title still: sorted by it (again:
                // the other way).
                let b = cx.listener_bounds();
                let (spans, order, _) = up_geometry(b);
                let over = order.iter().position(|&c| c == column).map(|at| spans[at]);
                if let Some((x0, x1)) = over
                    && e.position.x >= x0
                    && e.position.x < x1
                    && b.contains(&e.position)
                {
                    {
                        let mut m = up_model.0.borrow_mut();
                        let sorted = m.state.sort.filter(|(col, _)| *col == column);
                        m.state.sort = Some((column, sorted.is_none_or(|(_, ascending)| !ascending)));
                    }
                    if let Some(f) = &up_handlers.sort {
                        f(&(), cx);
                    }
                }
                cx.invalidate_owner();
            }
            _ => {}
        }
    })
}

/// Where the divider at `x` in a header `b` takes the pointer.
fn divider_at(x: Pixels, b: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds { origin: point(x - px(3.0), b.top()), size: size(px(6.0), b.size.height) }
}
