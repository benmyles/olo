//! Column view (macOS-style "Miller columns"): the window's folder in the
//! first column, each selected folder's contents in the next, and a
//! preview of a selected file at the end. Browsing happens in place; the
//! keyboard moves through the hierarchy (←/→ between columns).

use std::{collections::HashMap, path::PathBuf};

use olo_kit::{
    Bounds, Cx, DropAccept, DropOffer, Element, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter,
    ParentElement as _, PathBuilder, Pixels, Point, ScrollHandle, Styled as _, WindowCx, div, icons, metrics as m, point, px, size,
    style::{self, Align},
    theme,
    widgets::{SCROLL_BAR, ScrollBar, ScrollView},
};
use olo_kit_base::list::{ListState, SelectionBand};
use olo_theme::mix;

use crate::{
    folder::{FolderMsg, FolderWin, mods, plain_extra},
    fs::{self, Entry, Kind},
    items,
};

/// Width of each column, and of the preview after the last one.
pub const COLUMN_WIDTH: f32 = 220.0;
const PREVIEW_WIDTH: f32 = 260.0;
/// The preview's image (or icon) box.
const PREVIEW_IMAGE: f32 = 200.0;
const PREVIEW_ICON: f32 = 96.0;
/// The ▸ marking folders: its size, and the room left for it at the right.
const DISCLOSURE: (f32, f32) = (4.0, 8.0);
const DISCLOSURE_ROOM: f32 = m::CELL_INSET + 4.0 + m::MARK_GAP;
const DRAG_SLOP: f32 = 3.0;

/// The column view's own state: its scrolls and what the pointer is
/// doing.
#[derive(Default)]
pub struct Cols {
    /// Across the columns.
    scroll: ScrollHandle,
    /// Down each column, by its folder.
    lists: HashMap<PathBuf, ScrollHandle>,
    press: Option<ColPress>,
    /// A band: its column, where it began (down the column's rows) and
    /// across, what was selected, and where it reaches now (in the
    /// window).
    band: Option<(usize, Pixels, SelectionBand, Point<Pixels>)>,
}

impl Cols {
    /// The pointer's press is over (a drag it began ended elsewhere).
    pub fn end_press(&mut self) {
        self.press = None;
        self.band = None;
    }
}

/// A column where it is in the window now (`FolderWin::col_geo`): where
/// its content and its rows start (it scrolls, down and across), what of
/// it shows, and how many rows it has.
#[derive(Clone, Copy, Default)]
struct ColGeo {
    /// The content's top left.
    origin: Point<Pixels>,
    top: Pixels,
    view: Bounds<Pixels>,
    rows: usize,
}

impl ColGeo {
    fn row_at(&self, p: Point<Pixels>) -> Option<usize> {
        let r = (f32::from(p.y - self.top) / m::LIST_ROW).floor();
        (r >= 0.0 && (r as usize) < self.rows && self.view.contains(&p)).then_some(r as usize)
    }
}

struct ColPress {
    col: usize,
    row: Option<usize>,
    at: Point<Pixels>,
    button: MouseButton,
    mods: Modifiers,
    dragging: bool,
}

impl FolderWin {
    /// Column `k`'s folder, items and selection (0: the root).
    fn column(&self, k: usize) -> (&std::path::Path, &[Entry], &ListState) {
        if k == 0 {
            (&self.path, &self.entries, &self.list)
        } else {
            let p = &self.panes[k - 1];
            (&p.path, &p.entries, &p.list)
        }
    }

    fn column_mut(&mut self, k: usize) -> (&mut Vec<Entry>, &mut ListState) {
        if k == 0 {
            (&mut self.entries, &mut self.list)
        } else {
            let p = &mut self.panes[k - 1];
            (&mut p.entries, &mut p.list)
        }
    }

    /// Select `row` in column `k` and show what it holds in the next
    /// column.
    pub fn choose(&mut self, k: usize, row: usize) {
        let hidden = self.hidden;
        let (entries, list) = self.column_mut(k);
        let Some(e) = entries.get(row).cloned() else { return };
        list.select(row);
        self.active = k;
        if e.is_dir {
            if self.panes.get(k).is_none_or(|p| p.path != e.path) {
                self.open_column(k, e.path, hidden);
            }
        } else {
            self.panes.truncate(k);
        }
    }

    /// The file whose preview follows the last column, if one is selected.
    fn previewed(&self) -> Option<Entry> {
        let (_, entries, list) = self.column(self.panes.len());
        if list.selected.len() != 1 {
            return None;
        }
        let i = *list.selected.iter().next()?;
        entries.get(i).filter(|e| !e.is_dir).cloned()
    }

    /// Column `k`'s scroll.
    fn col_scroll(&mut self, k: usize) -> ScrollHandle {
        let path = self.column(k).0.to_owned();
        self.cols.lists.entry(path).or_default().clone()
    }

    /// Scrolls column `k` to `row`, with `around` rows above and below it.
    fn reveal_row(&mut self, k: usize, row: usize, around: f32, cx: &mut WindowCx) {
        let h = self.col_scroll(k);
        let view = Some(h.bounds().size.height).filter(|h| *h > px(0.0)).unwrap_or(px(300.0));
        let top = m::TIGHT + row as f32 * m::LIST_ROW;
        let (lo, hi) = (top - m::TIGHT - around * m::LIST_ROW, top + m::LIST_ROW + m::TIGHT + around * m::LIST_ROW);
        let now = -f32::from(h.offset().y);
        let to = if lo < now {
            lo.max(0.0)
        } else if hi > now + f32::from(view) {
            hi - f32::from(view)
        } else {
            return;
        };
        h.set_offset(cx, point(px(0.0), px(-to)));
    }

    /// ↑/↓ move within the active column, → enters the selected folder, ←
    /// goes back to its parent's column, Return opens, typing selects.
    pub fn column_key(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) -> bool {
        let key = e.keystroke.key.as_str();
        let shift = e.keystroke.modifiers.shift;
        let k = self.active.min(self.panes.len());
        let (_, entries, list) = self.column(k);
        let n = entries.len();
        let cur = list.cursor().filter(|c| list.selected.contains(c)).or_else(|| list.selected.iter().next().copied());
        let to = match key {
            "down" => list.step(1, n),
            "up" => list.step(-1, n),
            "home" => (n > 0).then_some(0),
            "end" => n.checked_sub(1),
            _ => None,
        };
        match key {
            // ⇧ grows or shrinks the selection from the anchor (no column
            // follows a selection of several).
            "down" | "up" | "home" | "end" if shift => {
                if let Some(next) = to {
                    self.column_mut(k).1.move_to(next, true);
                    self.panes.truncate(k);
                    self.reveal_row(k, next, 0.0, cx);
                }
            }
            "down" | "up" | "home" | "end" => {
                if let Some(next) = to {
                    self.choose(k, next);
                    self.reveal_row(k, next, 0.0, cx);
                }
            }
            "right" => {
                if cur.is_some_and(|c| entries[c].is_dir) && self.panes.len() > k {
                    self.active = k + 1;
                    let p = &mut self.panes[k];
                    if p.list.selected.is_empty() {
                        if !p.entries.is_empty() {
                            self.choose(k + 1, 0);
                        } else if p.loading {
                            // Not listed yet: its first item once it is.
                            p.first = true;
                        }
                    }
                }
            }
            "left" if k > 0 => {
                self.panes[k - 1].list.selected.clear();
                self.panes.truncate(k);
                self.active = k - 1;
            }
            "enter" => self.run("open", cx),
            _ => {
                // Typing selects within the active column.
                let Some(typed) = e.keystroke.key_char.clone().filter(|c| !c.chars().any(char::is_control)) else { return false };
                // (The column's names as they are, not copied for each key.)
                let (entries, list) = if k == 0 { (&self.entries, &self.list) } else { (&self.panes[k - 1].entries, &self.panes[k - 1].list) };
                if let Some(row) = self.typing.typed(&typed, entries.len(), list.cursor(), |i| entries[i].name.as_str()) {
                    self.choose(k, row);
                    self.reveal_row(k, row, 0.0, cx);
                }
            }
        }
        let _ = plain_extra;
        self.record_location();
        cx.invalidate();
        true
    }

    // ---- the pointer -------------------------------------------------------

    /// Column `k` where it is in the window now: from where it and the
    /// columns are scrolled to, not where it was last drawn (its rows are
    /// drawn only when they change). None before it's laid out.
    fn col_geo(&self, k: usize) -> Option<ColGeo> {
        let (path, entries, _) = self.column(k);
        let down = self.cols.lists.get(path)?;
        let b = down.bounds();
        if b.size.width <= px(0.0) {
            return None;
        }
        // (A column is laid out where it is with the columns scrolled to
        // their start.)
        let at = point(b.left() + self.cols.scroll.offset().x, b.top());
        let view = Bounds { origin: at, size: b.size }.intersect(&self.cols.scroll.bounds());
        let origin = point(at.x, at.y + down.offset().y);
        Some(ColGeo { origin, top: origin.y + px(m::TIGHT), view, rows: entries.len() })
    }

    /// What shows of column `k`'s content, in the content.
    fn column_shown(&self, k: usize) -> Option<Bounds<Pixels>> {
        let down = self.cols.lists.get(self.column(k).0)?;
        Some(Bounds { origin: point(px(0.0), -down.offset().y), size: down.bounds().size })
    }

    /// The outline round what shows of column `k` while what's dragged
    /// over it would go into its folder, in its content.
    fn column_outline(&self, k: usize) -> Option<Bounds<Pixels>> {
        if self.dropping == Some((k, None)) { self.column_shown(k) } else { None }
    }

    /// The band being drawn in column `k`, in its content, as far as it
    /// shows.
    fn column_band_rect(&self, k: usize) -> Option<Bounds<Pixels>> {
        let (_, from, _, now) = self.cols.band.as_ref().filter(|(c, ..)| *c == k)?;
        let g = self.col_geo(k)?;
        let x = self.cols.press.as_ref().map_or(now.x, |p| p.at.x);
        let (s, e) = (point(x - g.origin.x, *from + px(m::TIGHT)), point(now.x - g.origin.x, now.y - g.origin.y));
        let r = Bounds::from_corners(point(s.x.min(e.x), s.y.min(e.y)), point(s.x.max(e.x), s.y.max(e.y)));
        Some(r.intersect(&self.column_shown(k)?))
    }

    /// Row `i` of column `k`, in `r`.
    fn paint_column_row(&self, g: &mut Painter, r: Bounds<Pixels>, k: usize, i: usize) {
        if k > self.panes.len() {
            return;
        }
        let (_, entries, list) = self.column(k);
        let Some(e) = entries.get(i) else { return };
        let p = theme::palette();
        let active = self.active == k;
        let selected = list.selected.contains(&i);
        let text_color = if selected { style::c(p.list_selected_item_text) } else { style::c(p.list_item_text) };
        if selected {
            // Solid in the active column, a quieter tint along the path
            // in the others (as in Finder).
            let sel_bg = if active { style::c(p.list_selected_background) } else { style::c(mix(p.list_background, p.list_selected_background, 0.5)) };
            style::fill_rect(g, r, sel_bg);
        }
        olo_kit::agent::record(
            g,
            olo_kit::agent::Node { role: "ListItem", name: Some(e.name.clone().into()), bounds: r, enabled: true, clickable: true, selected: Some(selected), ..Default::default() },
        );
        let icon = Bounds { origin: point(r.left() + px(m::CELL_INSET), r.center().y - px(items::MINI / 2.0)), size: size(px(items::MINI), px(items::MINI)) };
        icons::paint(g, icon, items::icon_of(e));
        if crate::backups::backed_up(&e.path) {
            items::paint_mark(g, icon, e.label(), &p);
        }
        let x = icon.right() + px(m::MARK_GAP);
        let right = r.right() - px(if e.is_dir { DISCLOSURE_ROOM } else { m::CELL_INSET });
        if !(active && self.editing.as_ref().is_some_and(|r| r.path == e.path)) {
            style::label_fit(g, x, r.center().y, Align::Left, e.label().to_owned(), style::PLAIN_SIZE, items::name_font(e.is_link), text_color, (right - x).max(px(0.0)));
        }
        // Something dragged over it that would go into it.
        if self.dropping == Some((k, Some(i))) {
            style::frame_rect(g, r, style::c(p.keyboard_navigation));
        }
        if e.is_dir {
            let tip = point(r.right() - px(m::CELL_INSET), r.center().y);
            let color = if selected { text_color } else { style::disabled_text(&p) };
            let mut path = PathBuilder::fill();
            path.move_to(point(tip.x - px(DISCLOSURE.0), tip.y - px(DISCLOSURE.1 / 2.0)));
            path.line_to(tip);
            path.line_to(point(tip.x - px(DISCLOSURE.0), tip.y + px(DISCLOSURE.1 / 2.0)));
            path.close();
            if let Ok(path) = path.build() {
                g.paint_path(path, color);
            }
        }
    }

    /// What row `i` of column `k` shows, as a number that changes when
    /// it does: its item's look (`row_key`), whether it's selected (and
    /// in the active column), dropped on or being renamed.
    fn column_row_key(&self, k: usize, i: usize) -> u64 {
        if k > self.panes.len() {
            return 0;
        }
        let (_, entries, list) = self.column(k);
        let Some(e) = entries.get(i) else { return 0 };
        let (selected, active) = (list.selected.contains(&i), self.active == k);
        let editing = active && self.editing.as_ref().is_some_and(|r| r.path == e.path);
        style::key((crate::folder::row_key(e), selected, selected && active, self.dropping == Some((k, Some(i))), editing))
    }

    fn column_press(&mut self, k: usize, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        if self.editing.is_some() {
            self.end_rename(true, cx);
        }
        self.focus.focus(cx);
        self.col_scroll(k);
        let Some(g) = self.col_geo(k) else { return };
        let row = g.row_at(e.position);
        if e.button == MouseButton::Left && e.click_count >= 2 {
            if let Some(entry) = row.and_then(|r| self.column(k).1.get(r)).filter(|e| !e.is_dir).cloned() {
                self.open(&entry);
            }
            self.cols.press = None;
            return;
        }
        if e.button == MouseButton::Left && row.is_none() {
            // A selection band, from below the items: ⇧ adds, ⌘ flips.
            let base = self.column(k).2.band(mods(&e.modifiers));
            self.cols.band = Some((k, e.position.y - g.top, base, e.position));
        }
        self.cols.press = Some(ColPress { col: k, row, at: e.position, button: e.button, mods: e.modifiers, dragging: false });
        cx.invalidate();
    }

    fn column_move(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        let Some(press) = &mut self.cols.press else { return };
        if e.pressed_button.is_none() {
            return;
        }
        let (k, at) = (press.col, press.at);
        let moved = (e.position - at).magnitude() >= DRAG_SLOP as f64;
        match press.row {
            Some(row) if moved && !press.dragging => {
                press.dragging = true;
                let ask = press.button == MouseButton::Right;
                if !self.column(k).2.selected.contains(&row) {
                    self.choose(k, row);
                }
                self.active = k;
                let (_, entries, list) = self.column(k);
                let paths = list.selected.iter().filter_map(|&i| entries.get(i)).map(|e| e.path.clone()).collect();
                let Some(entry) = entries.get(row) else { return };
                let g = self.col_geo(k);
                let top = g.map_or(at.y, |g| g.top);
                let icon_top = top + px(row as f32 * m::LIST_ROW + (m::LIST_ROW - items::MINI) / 2.0);
                let left = g.map_or(at.x, |g| g.origin.x) + px(m::CELL_INSET);
                let d = items::Drag {
                    paths,
                    icon: items::icon_of(entry),
                    size: items::MINI,
                    hotspot: point((at.x - left).clamp(px(0.0), px(items::MINI)), (at.y - icon_top).clamp(px(0.0), px(items::MINI))),
                    ask,
                    name: entry.name.clone(),
                };
                items::start(cx, self.id, d, || FolderMsg::DragOver);
                cx.invalidate();
            }
            None if press.button == MouseButton::Left => {
                self.column_band(e.position, cx);
                cx.invalidate();
            }
            _ => {}
        }
    }

    /// The band in column `k` reaches `now`: the rows it covers selected,
    /// and near the column's top or bottom it scrolls on.
    fn column_band(&mut self, now: Point<Pixels>, cx: &mut WindowCx) {
        let Some((k, from, base, _)) = self.cols.band.clone() else { return };
        if let Some(b) = &mut self.cols.band {
            b.3 = now;
        }
        let Some(g) = self.col_geo(k) else { return };
        let here = now.y - g.top;
        let (y0, y1) = (f32::from(from.min(here)), f32::from(from.max(here)));
        let rows = self.column(k).1.len();
        let first = (y0 / m::LIST_ROW).floor().max(0.0) as usize;
        let end = ((y1 / m::LIST_ROW).ceil().max(0.0) as usize).min(rows);
        let selected = base.over(first..end.max(first));
        let ends = (first < end).then(|| if here >= from { (first, end - 1) } else { (end - 1, first) });
        let changed = self.column(k).2.selected != selected;
        let (_, list) = self.column_mut(k);
        if changed {
            if let Some((a, b)) = ends {
                list.anchor = Some(a);
                list.cursor = Some(b);
            }
            list.selected = selected;
            self.panes.truncate(k);
        }
        self.active = k;
        // Near or past the column's top or bottom: scroll on.
        let edge = px(m::LIST_ROW / 2.0);
        let step = if now.y > g.view.bottom() - edge {
            f32::from((now.y - g.view.bottom() + edge) / 2.0).clamp(2.0, 2.0 * m::LIST_ROW)
        } else if now.y < g.view.top() + edge {
            -f32::from((g.view.top() + edge - now.y) / 2.0).clamp(2.0, 2.0 * m::LIST_ROW)
        } else {
            0.0
        };
        if step != 0.0 {
            let h = self.col_scroll(k);
            let (o, max) = (h.offset(), f32::from(h.max_offset().height));
            let to = (f32::from(o.y) - step).clamp(-max, 0.0);
            h.set_offset(cx, point(o.x, px(to)));
        }
    }

    fn column_release(&mut self, e: &MouseUpEvent, cx: &mut Cx<Self>) {
        let Some(press) = self.cols.press.take() else { return };
        self.cols.band = None;
        if press.dragging {
            cx.invalidate();
            return;
        }
        let (col, moved) = (press.col, (e.position - press.at).magnitude() >= DRAG_SLOP as f64);
        match (press.button, press.row) {
            (MouseButton::Left, _) if moved => {}
            (MouseButton::Left, Some(row)) if press.mods.control || press.mods.platform || press.mods.shift => {
                self.column_mut(col).1.click(row, mods(&press.mods));
                self.active = col;
                self.panes.truncate(col);
            }
            (MouseButton::Left, Some(row)) => self.choose(col, row),
            (MouseButton::Left, None) => {
                self.column_mut(col).1.selected.clear();
                self.panes.truncate(col);
                self.active = col;
            }
            (MouseButton::Right, row) => {
                match row {
                    Some(row) if !self.column(col).2.selected.contains(&row) => self.choose(col, row),
                    Some(_) => self.active = col,
                    None => {
                        self.column_mut(col).1.selected.clear();
                        self.panes.truncate(col);
                        self.active = col;
                    }
                }
                self.context_menu(row.is_some(), e.position, cx);
            }
            _ => {}
        }
        self.record_location();
        cx.invalidate();
    }

    // ---- what's dragged over it ----------------------------------------------

    /// What's dragged over the columns at `at`: onto a folder in a
    /// column, into a column's folder, or (past the last one) into the
    /// last one's.
    pub fn columns_drop_over(&mut self, o: &DropOffer, at: Point<Pixels>, cx: &mut Cx<Self>) -> Option<DropAccept> {
        let n = 1 + self.panes.len();
        let view = self.cols.scroll.bounds();
        if !view.contains(&at) {
            self.dropping = None;
            self.rest_on(None, cx);
            return None;
        }
        let col = (0..n).find_map(|k| self.col_geo(k).filter(|g| g.view.contains(&at)).map(|g| (k, g)));
        let (accept, on, k) = match col {
            Some((k, g)) => {
                let r = (f32::from(at.y - g.top) / m::LIST_ROW).floor();
                let (dir, entries, list) = self.column(k);
                let over = (r >= 0.0).then_some(r as usize).and_then(|r| entries.get(r).map(|e| (r, e, list.selected.contains(&r))));
                let (accept, on) = items::item_drop(cx, self.id, o, over, Some(dir));
                (accept, on, k)
            }
            // Past the last column (the preview, or nothing): into its
            // folder.
            None => {
                let last = self.column(n - 1).0.to_owned();
                (items::item_drop(cx, self.id, o, None, Some(&last)).0, None, n)
            }
        };
        let spring_to = on.filter(|_| accept.is_some() && k < n).and_then(|i| self.column(k).1.get(i)).map(|e| e.path.clone());
        self.dropping = accept.is_some().then_some((k, on));
        self.rest_on(spring_to.as_deref(), cx);
        accept
    }

    // ---- drawing -----------------------------------------------------------

    pub fn columns_view(&mut self, cx: &mut Cx<Self>) -> Element {
        let n = 1 + self.panes.len();
        self.active = self.active.min(n - 1);
        let preview = self.previewed();
        let total = n as f32 * COLUMN_WIDTH + preview.as_ref().map_or(0.0, |_| PREVIEW_WIDTH);
        let width = f32::from(self.cols.scroll.bounds().size.width);
        // Selections made in code (Show in Tracker, a new item, columns
        // put back after a reload): into view, with some of what's
        // around them.
        for (k, row) in std::mem::take(&mut self.show) {
            if k < n {
                let h = f32::from(self.col_scroll(k).bounds().size.height);
                let around = if h > 0.0 { ((h / m::LIST_ROW / 2.0).floor() - 1.0).max(0.0) } else { 4.0 };
                self.reveal_row(k, row, around, cx);
            }
        }
        // When the active column or what it opened changes, it is brought
        // into view; otherwise the scroll stays where it was put.
        let focus = (self.active, n, preview.is_some());
        if self.col_focus != Some(focus) {
            if width > 0.0 {
                self.col_focus = Some(focus);
                let mut x = -f32::from(self.cols.scroll.offset().x);
                let right = ((self.active + 2).min(n) as f32 * COLUMN_WIDTH).max(if self.active + 1 >= n { total } else { 0.0 });
                let left = self.active as f32 * COLUMN_WIDTH;
                if right - x > width {
                    x = right - width;
                }
                if left < x {
                    x = left;
                }
                let x = x.clamp(0.0, (total - width).max(0.0)).round();
                self.cols.scroll.set_offset_once_laid_out(cx, point(px(-x), px(0.0)));
            } else {
                // (Not laid out yet: once it has been.)
                cx.after_do(std::time::Duration::ZERO, |_, cx| cx.invalidate());
            }
        }
        let p = theme::palette();
        let mut row = div().flex().flex_row().h_full().w(px(total));
        for k in 0..n {
            row = row.child(self.column_element(k, cx));
        }
        // Past the last column: outlined while what's dragged there would
        // go into its folder.
        let outlined = self.dropping == Some((n, None));
        let past = cx
            .canvas(move |f: &FolderWin, g, b| {
                if f.dropping == Some((1 + f.panes.len(), None)) {
                    let clip = g.visible_bounds().intersect(&b);
                    items::outline(g, clip, &theme::palette());
                }
            })
            .paint_key(outlined as u64)
            .absolute()
            .inset_0();
        let preview_el = preview.map(|e| {
            let entry = e.clone();
            // (Drawn again when the file changes, or a picture's thumbnail
            // is made.)
            let key = style::key((crate::folder::row_key(&e), crate::thumbs::made()));
            div()
                .w(px(PREVIEW_WIDTH))
                .h_full()
                .relative()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |f: &mut FolderWin, ev: &MouseDownEvent, _| {
                        if ev.click_count >= 2 {
                            f.open(&entry);
                        }
                    }),
                )
                .child(olo_kit::canvas(move |g, b| draw_preview(g, &e, b)).paint_key(key).size_full())
        });
        let rest = div().flex_1().h_full().relative().children(preview_el).child(past);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().relative().bg(style::c(p.document_background)).overflow_x_scroll().track_scroll(&self.cols.scroll).child(row.child(rest)))
            .child(div().w_full().h(px(SCROLL_BAR)).relative().child(ScrollBar::horizontal(&self.cols.scroll)))
            .into_any_element()
    }

    fn column_element(&mut self, k: usize, cx: &mut Cx<Self>) -> Element {
        let (_, entries, _) = self.column(k);
        let rows = entries.len();
        let handle = self.col_scroll(k);
        let height = rows as f32 * m::LIST_ROW + 2.0 * m::TIGHT;
        let editing_here = self.editing.as_ref().filter(|_| k == self.active).and_then(|r| self.column(k).1.iter().position(|e| e.path == r.path));
        // Editing a name, over its row in the active column (in the
        // column's content: it scrolls with it).
        let field_el = match (editing_here, &self.editing) {
            (Some(row), Some(r)) => {
                let e = self.column(k).1[row].clone();
                let cell = Bounds { origin: point(px(m::CELL_INSET - m::TIGHT), px(m::TIGHT + row as f32 * m::LIST_ROW)), size: size(px(COLUMN_WIDTH - 2.0 - m::CELL_INSET), px(m::LIST_ROW)) };
                let l = items::layout_item(cell, e.label(), items::Look::Mini, true, e.is_link);
                Some(r.element(cx, &l, Point::default()))
            }
            _ => None,
        };
        // Its items a row each, each drawn again only when what it shows
        // changes, and made as it comes into view: a step of the selection
        // draws the row it left and the one it's on, not every column.
        let rows_el = olo_kit::uniform_rows(rows, px(m::LIST_ROW), move |near| {
            near.map(|i| olo_kit::view_canvas::<FolderWin>(move |f, g, r| f.paint_column_row(g, r, k, i)).key_by::<FolderWin>(move |f| f.column_row_key(k, i)).id(i).w_full()).collect()
        })
        .row_key_by::<FolderWin>(move |f, i| f.column_row_key(k, i))
        .absolute()
        .left(px(0.0))
        .top(px(m::TIGHT));
        // Over them: the outline while what's dragged over would go into
        // its folder, and a band (drawn from below the items).
        let bits = |r: Option<Bounds<Pixels>>| r.map(|r| [r.origin.x, r.origin.y, r.size.width, r.size.height].map(|v| f32::from(v).to_bits()));
        let over = olo_kit::view_canvas::<FolderWin>(move |f, g, b| {
            let p = theme::palette();
            let at = |r: Bounds<Pixels>| Bounds { origin: r.origin + b.origin, size: r.size };
            if let Some(r) = f.column_outline(k) {
                items::outline(g, at(r), &p);
            }
            if let Some(r) = f.column_band_rect(k) {
                style::selection_band(g, at(r), &p);
            }
        })
        .key_by::<FolderWin>(move |f| style::key((bits(f.column_outline(k)), bits(f.column_band_rect(k)))))
        .absolute()
        .inset_0();
        let sep = olo_kit::canvas(|g, b| {
            let p = theme::palette();
            // Etched on the right edge: the light line right against
            // the dark one (a point over left a device pixel between
            // them at 1.5×, where a line is two thirds of a point).
            let x = b.right() - px(2.0);
            style::vline(g, x, b.top(), b.bottom(), style::shade(&p, p.document_background, 1.15));
            let lit = style::snap_coord(g, x) + style::hairline(g);
            style::vline(g, lit, b.top(), b.bottom(), style::c(p.shine));
        })
        .absolute()
        .inset_0();
        let mut inner = div()
            .relative()
            .w_full()
            .h(px(height))
            .min_h_full()
            .on_mouse_down(MouseButton::Left, cx.listener(move |f: &mut FolderWin, e: &MouseDownEvent, cx| f.column_press(k, e, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |f: &mut FolderWin, e: &MouseDownEvent, cx| f.column_press(k, e, cx)));
        if k == 0 {
            // The pointer held down goes on being followed past a
            // column's edge.
            inner = inner
                .on_drag_move(cx.listener(|f: &mut FolderWin, e: &MouseMoveEvent, cx| f.column_move(e, cx)))
                .on_window_mouse_up(cx.listener(|f: &mut FolderWin, e: &MouseUpEvent, cx| f.column_release(e, cx)));
        }
        div()
            .w(px(COLUMN_WIDTH))
            .h_full()
            .relative()
            .child(sep)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(px(COLUMN_WIDTH - 2.0))
                    .child(ScrollView::new(&handle).border(false).auto_hide(true).child(inner.child(rows_el).child(over).children(field_el))),
            )
            .into_any_element()
    }
}

/// The preview after the last column: the picture itself or a large
/// icon, the name and kind, and a few facts.
fn draw_preview(g: &mut Painter, e: &Entry, rect: Bounds<Pixels>) {
    let p = theme::palette();
    let area = Bounds::from_corners(point(rect.left() + px(m::WINDOW_INSET), rect.top() + px(m::WINDOW_INSET)), point(rect.right() - px(m::WINDOW_INSET), rect.bottom() - px(m::WINDOW_INSET)));
    let image_box = Bounds { origin: point(area.center().x - px(PREVIEW_IMAGE / 2.0), area.top()), size: size(px(PREVIEW_IMAGE), px(PREVIEW_IMAGE * 0.75)) };
    let scale = g.scale_factor();
    let thumb = (e.kind == Kind::Image).then(|| crate::thumbs::get(&e.path, (PREVIEW_IMAGE * scale).round() as u32)).flatten();
    match thumb {
        Some(img) => {
            let s = img.size(0);
            let (w, h) = (s.width.0 as f32 / scale, s.height.0 as f32 / scale);
            let k = (PREVIEW_IMAGE / w).min(PREVIEW_IMAGE * 0.75 / h).min(1.0);
            let shown = style::snap_centred(g, image_box.center(), w * k, h * k);
            let _ = g.paint_image(shown, Default::default(), img, 0, false);
            let edge = Bounds::from_corners(point(shown.left() - px(1.0), shown.top() - px(1.0)), point(shown.right() + px(1.0), shown.bottom() + px(1.0)));
            style::frame_rect(g, edge, style::shade(&p, p.document_background, 1.15));
        }
        None => {
            let icon = Bounds { origin: point(image_box.center().x - px(PREVIEW_ICON / 2.0), image_box.center().y - px(PREVIEW_ICON / 2.0)), size: size(px(PREVIEW_ICON), px(PREVIEW_ICON)) };
            icons::paint(g, icon, items::icon_of(e));
            if let Some(done) = e.progress() {
                items::paint_progress(g, icon, done, &p);
            }
        }
    }
    // Name (bold, up to three lines), kind, then facts in two columns
    // meeting at the centre, keys right-aligned.
    let mut y = image_box.bottom() + px(m::SPACING);
    let bold = style::ui_font(FontWeight::BOLD);
    for line in items::wrap_lines(e.label(), &bold, area.size.width, 3) {
        let at = style::label(g, area.center().x, y + px(m::LABEL_ROW / 2.0), Align::Center, line.clone(), style::PLAIN_SIZE, bold.clone(), style::c(p.document_text));
        olo_kit::agent::record(g, olo_kit::agent::Node::label(line, at));
        y += px(style::PLAIN_SIZE * style::LINE_HEIGHT);
    }
    let plain = style::ui_font(FontWeight::NORMAL);
    style::label(g, area.center().x, y + px(m::LABEL_ROW / 2.0), Align::Center, e.kind_text(), style::PLAIN_SIZE, plain.clone(), style::disabled_text(&p));
    y += px(m::LABEL_ROW + m::SPACING);
    let created = std::fs::metadata(&e.path).ok().and_then(|md| md.created().ok());
    for (k, v) in [("Size", e.size_text()), ("Created", fs::format_time(created)), ("Modified", fs::format_time(e.modified))] {
        let cy = y + px(m::LABEL_ROW / 2.0);
        style::label(g, area.center().x - px(m::TIGHT), cy, Align::Right, k, style::SMALL_SIZE, plain.clone(), style::disabled_text(&p));
        let at = style::label(g, area.center().x + px(m::TIGHT), cy, Align::Left, v.clone(), style::SMALL_SIZE, plain.clone(), style::c(p.document_text));
        olo_kit::agent::record(g, olo_kit::agent::Node::label(v, at));
        y += px(m::LABEL_ROW);
    }
}
