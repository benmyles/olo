//! Column view (macOS-style "Miller columns"): the window's folder in the
//! first column, each selected folder's contents in the next, and a
//! preview of a selected file at the end. Browsing happens in place; the
//! keyboard moves through the hierarchy (←/→ between columns).

use std::path::PathBuf;

use olo_kit::{
    egui::{self, Align, Color32, FontId, Key, Pos2, Rect, Sense, Ui, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    theme::{Palette, mix},
    widgets::{self, ScrollView},
};

use crate::{
    FolderWin,
    fs::{self, Entry, Kind},
    view::{self, Action, ItemLayout},
};

/// Width of each column, and of the preview after the last one.
pub const COLUMN_WIDTH: f32 = 220.0;
const PREVIEW_WIDTH: f32 = 260.0;
/// The preview's image (or icon) box.
const PREVIEW_IMAGE: f32 = 200.0;
const PREVIEW_ICON: f32 = 96.0;
/// The ▸ marking folders: its size, and the gap left for it at the right.
const DISCLOSURE: egui::Vec2 = egui::vec2(4.0, 8.0);
const DISCLOSURE_ROOM: f32 = m::CELL_INSET + 4.0 + m::MARK_GAP;

/// What happened in a column this frame (applied once drawing is done).
enum Event {
    /// `toggle`: ⌘ (add or take away one); `range`: ⇧ (from the last one
    /// clicked to this).
    Click { col: usize, row: Option<usize>, toggle: bool, range: bool },
    Double { col: usize, row: usize },
    Context { col: usize, row: Option<usize>, pos: Pos2 },
    /// A drag started on a row; where on its icon the pointer is.
    Drag { col: usize, row: usize, hotspot: egui::Vec2, ask: bool },
    /// A selection band (drawn from below a column's items) now selects
    /// these.
    Band { col: usize, selected: std::collections::BTreeSet<usize> },
}

fn column(f: &FolderWin, k: usize) -> (&std::path::Path, &[Entry], &widgets::ListState) {
    if k == 0 { (&f.path, &f.entries, &f.list) } else { let p = &f.panes[k - 1]; (&p.path, &p.entries, &p.list) }
}

fn column_mut(f: &mut FolderWin, k: usize) -> (&mut Vec<Entry>, &mut widgets::ListState) {
    if k == 0 { (&mut f.entries, &mut f.list) } else { let p = &mut f.panes[k - 1]; (&mut p.entries, &mut p.list) }
}

/// Select `row` in column `k` and show what it holds in the next column.
fn choose(f: &mut FolderWin, k: usize, row: usize) {
    let hidden = f.hidden;
    let (entries, list) = column_mut(f, k);
    let Some(e) = entries.get(row).cloned() else { return };
    list.select(row);
    f.active = k;
    if e.is_dir {
        if f.panes.get(k).is_none_or(|p| p.path != e.path) {
            f.open_column(k, e.path, hidden);
        }
    } else {
        f.panes.truncate(k);
    }
}

/// The file whose preview follows the last column, if one is selected.
fn previewed(f: &FolderWin) -> Option<Entry> {
    let (_, entries, list) = column(f, f.panes.len());
    if list.selected.len() != 1 {
        return None;
    }
    let i = *list.selected.iter().next()?;
    entries.get(i).filter(|e| !e.is_dir).cloned()
}

pub fn columns_view(ui: &mut Ui, id: egui::Id, f: &mut FolderWin, actions: &mut Vec<Action>) {
    let p = style::palette(ui.ctx());
    let whole = ui.available_rect_before_wrap();
    // A scroll bar along the bottom for columns wider than the window.
    let body = Rect::from_min_max(whole.min, pos2(whole.right(), whole.bottom() - widgets::SCROLL_BAR));
    let bar = Rect::from_min_max(pos2(whole.left(), body.bottom()), whole.max);
    ui.painter().rect_filled(body, 0.0, c(p.document_background));
    let n = 1 + f.panes.len();
    f.active = f.active.min(n - 1);
    let preview = previewed(f);
    let total = n as f32 * COLUMN_WIDTH + preview.as_ref().map_or(0.0, |_| PREVIEW_WIDTH);

    keyboard(ui, f, actions);
    let n = 1 + f.panes.len();

    // Horizontal scrolling: the touchpad, Shift+wheel and the bar scroll
    // sideways. When the active column or what it opened changes, it is
    // brought into view; otherwise the scroll stays where it was put.
    let max_off = (total - body.width()).max(0.0);
    if ui.rect_contains_pointer(whole) {
        let d = ui.input(|i| i.smooth_scroll_delta);
        if d.x != 0.0 {
            f.col_scroll -= d.x;
        }
    }
    let focus = (f.active, n, preview.is_some());
    if f.col_focus != Some(focus) {
        f.col_focus = Some(focus);
        let focus_right = ((f.active + 2).min(n) as f32 * COLUMN_WIDTH).max(if f.active + 1 >= n { total } else { 0.0 });
        let focus_left = f.active as f32 * COLUMN_WIDTH;
        if focus_right - f.col_scroll > body.width() {
            f.col_scroll = focus_right - body.width();
        }
        if focus_left < f.col_scroll {
            f.col_scroll = focus_left;
        }
    }
    widgets::ScrollBar::horizontal(id.with("columns")).show(ui, bar, &mut f.col_scroll, total, body.width());
    f.col_scroll = f.col_scroll.clamp(0.0, max_off).round();

    let mut events = Vec::new();
    let mut edit_row: Option<(Rect, usize)> = None;
    let clip = body.intersect(ui.clip_rect());
    for k in 0..n {
        let x = body.left() + k as f32 * COLUMN_WIDTH - f.col_scroll;
        let rect = Rect::from_min_max(pos2(x, body.top()), pos2(x + COLUMN_WIDTH, body.bottom()));
        if !rect.intersects(clip) {
            continue;
        }
        let mut col_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        col_ui.set_clip_rect(rect.intersect(clip));
        draw_column(&mut col_ui, f, k, rect, &p, &mut events, &mut edit_row);
    }
    // Past the last column (the preview, or nothing): into its folder.
    let end = body.left() + n as f32 * COLUMN_WIDTH - f.col_scroll;
    if let Some(h) = view_hover(ui, clip).filter(|h| h.pos.x >= end) {
        let last = column(f, n - 1).0.to_owned();
        if view::item_drop(ui.ctx(), &h, None, Some(&last)).0 {
            olo_kit::dnd::outline(ui, Rect::from_min_max(pos2(end.max(clip.left()), clip.top()), clip.max));
        }
    }
    if let Some(e) = &preview {
        let x = body.left() + n as f32 * COLUMN_WIDTH - f.col_scroll;
        let rect = Rect::from_min_max(pos2(x, body.top()), pos2(x + PREVIEW_WIDTH, body.bottom()));
        let mut pv = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        pv.set_clip_rect(rect.intersect(clip));
        if draw_preview(&mut pv, e, rect, &p) {
            actions.push(Action::Open(e.clone()));
        }
    }

    // Editing a name in place, over its row in the active column.
    if let Some((row_rect, row)) = edit_row {
        let (_, entries, _) = column(f, f.active);
        let e = entries[row].clone();
        let font = if e.is_link { style::italic_font() } else { style::plain_font() };
        let g = ui.painter().layout_no_wrap(e.name.clone(), font, Color32::BLACK);
        let x = row_rect.left() + m::CELL_INSET + view::MINI + m::MARK_GAP;
        let origin = style::text_origin(ui.ctx(), &g, x, row_rect.center().y, style::PLAIN_SIZE);
        let l = ItemLayout::text_only(origin, g);
        if let Some(a) = view::edit_label(ui, &l, &mut f.editing, e.is_link) {
            actions.push(a);
        }
    } else if f.editing.as_ref().is_some_and(|(p, _)| !column(f, f.active).1.iter().any(|e| &e.path == p)) {
        f.editing = None;
    }

    for ev in events {
        match ev {
            Event::Click { col, row: Some(row), range: true, toggle: false } => {
                let (_, list) = column_mut(f, col);
                let a = list.anchor.unwrap_or(row);
                list.selected = (a.min(row)..=a.max(row)).collect();
                f.active = col;
                f.panes.truncate(col);
            }
            Event::Click { col, row: Some(row), toggle: true, .. } => {
                let (_, list) = column_mut(f, col);
                if !list.selected.remove(&row) {
                    list.selected.insert(row);
                }
                list.anchor = Some(row);
                f.active = col;
                f.panes.truncate(col);
            }
            Event::Click { col, row: Some(row), .. } => choose(f, col, row),
            Event::Click { col, row: None, .. } => {
                column_mut(f, col).1.selected.clear();
                f.panes.truncate(col);
                f.active = col;
            }
            Event::Double { col, row } => {
                if let Some(e) = column(f, col).1.get(row).filter(|e| !e.is_dir) {
                    actions.push(Action::Open(e.clone()));
                }
            }
            Event::Drag { col, row, hotspot, ask } => {
                if !column(f, col).2.selected.contains(&row) {
                    choose(f, col, row);
                }
                f.active = col;
                let (_, entries, list) = column(f, col);
                let paths = list.selected.iter().filter_map(|&i| entries.get(i)).map(|e| e.path.clone()).collect();
                if let Some(e) = entries.get(row) {
                    actions.push(Action::Drag(view::Drag { paths, icon: view::icon_of(e), size: view::MINI, hotspot, ask, name: e.name.clone(), below: false }));
                }
            }
            Event::Band { col, selected } => {
                let (_, list) = column_mut(f, col);
                if list.selected != selected {
                    list.anchor = selected.first().copied().or(list.anchor);
                    list.selected = selected;
                    f.panes.truncate(col);
                }
                f.active = col;
            }
            Event::Context { col, row, pos } => {
                match row {
                    Some(row) if !column(f, col).2.selected.contains(&row) => choose(f, col, row),
                    Some(_) => f.active = col,
                    None => {
                        column_mut(f, col).1.selected.clear();
                        f.panes.truncate(col);
                        f.active = col;
                    }
                }
                let menu = if row.is_some() { view::item_menu(f) } else { view::background_menu(f, true) };
                widgets::popup_menu_at(ui.ctx(), id, pos, menu);
            }
        }
    }
}

fn draw_column(
    ui: &mut Ui,
    f: &FolderWin,
    k: usize,
    rect: Rect,
    p: &Palette,
    events: &mut Vec<Event>,
    edit_row: &mut Option<(Rect, usize)>,
) {
    let (path, entries, list) = column(f, k);
    // Etched separator on the right edge.
    let sep = rect.right() - 2.0;
    style::vline(ui.painter(), sep, rect.top(), rect.bottom(), style::shade(p, p.document_background, 1.15));
    style::vline(ui.painter(), sep + 1.0, rect.top(), rect.bottom(), c(p.shine));
    let inner = Rect::from_min_max(rect.min, pos2(sep, rect.max.y));
    let active = k == f.active;
    let editing = f.editing.as_ref().and_then(|(p, _)| entries.iter().position(|e| &e.path == p)).filter(|_| active);
    let rows = entries.len();
    ScrollView::new(("col", path)).border(false).auto_hide(true).show_in(ui, inner, |ui, _vp| {
        let top = ui.min_rect().top() + m::TIGHT;
        let content = Rect::from_min_size(
            pos2(ui.min_rect().left(), ui.min_rect().top()),
            vec2(ui.available_width(), (rows as f32 * m::LIST_ROW + 2.0 * m::TIGHT).max(ui.available_height())),
        );
        let resp = ui.allocate_rect(content, Sense::click_and_drag());
        let clip = ui.clip_rect();
        let row_at = |pos: Pos2| {
            let r = ((pos.y - top) / m::LIST_ROW).floor();
            (r >= 0.0 && (r as usize) < rows).then_some(r as usize)
        };
        let first = (((clip.top() - top) / m::LIST_ROW).floor().max(0.0)) as usize;
        let last = ((((clip.bottom() - top) / m::LIST_ROW).ceil().max(0.0)) as usize).min(rows);
        let painter = ui.painter();
        // Selection: solid in the active column, a quieter tint along the
        // path in the others (as in Finder).
        let sel_bg = if active { c(p.list_selected_background) } else { c(mix(p.list_background, p.list_selected_background, 0.5)) };
        // Something dragged over this column: onto a folder in it, or into it.
        let (took, drop_on) = match view_hover(ui, clip) {
            Some(h) => {
                let over = row_at(h.pos).map(|i| (i, &entries[i], list.selected.contains(&i)));
                view::item_drop(ui.ctx(), &h, over, Some(path))
            }
            None => (false, None),
        };
        for (i, e) in entries.iter().enumerate().take(last).skip(first) {
            let r = Rect::from_min_size(pos2(content.left(), top + i as f32 * m::LIST_ROW), vec2(content.width(), m::LIST_ROW));
            let selected = list.selected.contains(&i);
            let text_color = if selected { c(p.list_selected_item_text) } else { c(p.list_item_text) };
            if selected {
                painter.rect_filled(r, 0.0, sel_bg);
            }
            style::describe_item(ui.ctx(), &e.name, r.intersect(painter.clip_rect()), selected);
            let icon = Rect::from_min_size(pos2(r.left() + m::CELL_INSET, r.center().y - view::MINI / 2.0), vec2(view::MINI, view::MINI));
            icons::paint(painter, icon, view::icon_of(e));
            let x = icon.right() + m::MARK_GAP;
            let right = r.right() - if e.is_dir { DISCLOSURE_ROOM } else { m::CELL_INSET };
            if editing == Some(i) {
                *edit_row = Some((r, i));
            } else {
                let font = if e.is_link { style::italic_font() } else { style::plain_font() };
                let mut job = egui::text::LayoutJob::simple_singleline(e.name.clone(), font, text_color);
                job.wrap = egui::text::TextWrapping::truncate_at_width((right - x).max(0.0));
                let g = painter.layout_job(job);
                let pos = style::text_origin(ui.ctx(), &g, x, r.center().y, style::PLAIN_SIZE);
                painter.galley(pos, g, text_color);
            }
            if took && drop_on == Some(i) {
                painter.rect_stroke(r.shrink(0.5), 0.0, egui::Stroke::new(1.0, c(p.keyboard_navigation)), egui::StrokeKind::Inside);
            }
            if e.is_dir {
                let tip = pos2(r.right() - m::CELL_INSET, r.center().y);
                let tri = vec![
                    pos2(tip.x - DISCLOSURE.x, tip.y - DISCLOSURE.y / 2.0),
                    pos2(tip.x, tip.y),
                    pos2(tip.x - DISCLOSURE.x, tip.y + DISCLOSURE.y / 2.0),
                ];
                let color = if selected { text_color } else { style::disabled_text(p) };
                painter.add(egui::Shape::convex_polygon(tri, color, egui::Stroke::NONE));
            }
        }
        if took && drop_on.is_none() {
            olo_kit::dnd::outline(ui, clip);
        }
        let mods = ui.input(|i| i.modifiers);
        // A selection band, from below the items (where it started, in
        // content coordinates, and what ⇧ or Ctrl kept selected).
        let band_id = egui::Id::unique(("col-band", path));
        let origin = ui.min_rect().top();
        if resp.drag_started() && resp.dragged_by(egui::PointerButton::Primary)
            && let Some(at) = ui.input(|i| i.pointer.press_origin()).filter(|&at| row_at(at).is_none())
        {
            let base = if mods.ctrl || mods.shift { list.selected.clone() } else { Default::default() };
            ui.ctx().data_mut(|d| d.insert_temp(band_id, (pos2(at.x, at.y - origin), base)));
        }
        let band = ui.ctx().data(|d| d.get_temp::<(Pos2, std::collections::BTreeSet<usize>)>(band_id));
        match (band, resp.dragged(), ui.input(|i| i.pointer.latest_pos())) {
            (Some((from, base)), true, Some(now)) => {
                let (y0, y1) = (from.y.min(now.y - origin) + origin - top, from.y.max(now.y - origin) + origin - top);
                let first = (y0 / m::LIST_ROW).floor().max(0.0) as usize;
                let end = ((y1 / m::LIST_ROW).ceil().max(0.0) as usize).min(rows);
                let mut selected = base;
                selected.extend(first..end.max(first));
                events.push(Event::Band { col: k, selected });
                let r = Rect::from_two_pos(pos2(from.x, from.y + origin), now);
                style::selection_band(&ui.painter().with_clip_rect(clip), r.intersect(clip), p);
                // Near or past the column's top or bottom: scroll on.
                let edge = m::LIST_ROW / 2.0;
                let step = if now.y > clip.bottom() - edge {
                    ((now.y - clip.bottom() + edge) / 2.0).clamp(2.0, 2.0 * m::LIST_ROW)
                } else if now.y < clip.top() + edge {
                    -((clip.top() + edge - now.y) / 2.0).clamp(2.0, 2.0 * m::LIST_ROW)
                } else {
                    0.0
                };
                if step != 0.0 {
                    let y = if step > 0.0 { clip.bottom() - origin + step } else { clip.top() - origin + step };
                    ScrollView::reveal(ui.ctx(), ("col", path), (y - 1.0).max(0.0), (y + 1.0).min(content.height()));
                }
            }
            (Some(_), _, _) => ui.ctx().data_mut(|d| d.remove::<(Pos2, std::collections::BTreeSet<usize>)>(band_id)),
            _ => {}
        }
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.drag_started() {
                // Where the button went down, not where the drag got to.
                let pos = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
                if let Some(row) = row_at(pos) {
                    let icon = pos2(content.left() + m::CELL_INSET, top + row as f32 * m::LIST_ROW + (m::LIST_ROW - view::MINI) / 2.0);
                    let hotspot = (pos - icon).clamp(egui::Vec2::ZERO, vec2(view::MINI, view::MINI));
                    events.push(Event::Drag { col: k, row, hotspot, ask: resp.dragged_by(egui::PointerButton::Secondary) });
                }
            } else if resp.double_clicked() {
                if let Some(row) = row_at(pos) {
                    events.push(Event::Double { col: k, row });
                }
            } else if resp.clicked() {
                events.push(Event::Click { col: k, row: row_at(pos), toggle: mods.ctrl, range: mods.shift });
            } else if resp.secondary_clicked() {
                events.push(Event::Context { col: k, row: row_at(pos), pos });
            }
        }
    });
}

/// What is dragged over `clip`, if anything.
fn view_hover(ui: &Ui, clip: Rect) -> Option<olo_kit::dnd::Hover> {
    olo_kit::dnd::hover(ui.ctx()).filter(|h| clip.contains(h.pos))
}

/// The preview after the last column: the image itself or a large icon,
/// the name and kind, and a few facts. True if double-clicked (open).
fn draw_preview(ui: &mut Ui, e: &Entry, rect: Rect, p: &Palette) -> bool {
    let area = rect.shrink2(vec2(m::WINDOW_INSET, m::WINDOW_INSET));
    let image_box = Rect::from_min_size(pos2(area.center().x - PREVIEW_IMAGE / 2.0, area.top()), vec2(PREVIEW_IMAGE, PREVIEW_IMAGE * 0.75));
    let painter = ui.painter();
    let mut shown = image_box;
    let thumb = (e.kind == Kind::Image).then(|| icons::thumbnail(ui.ctx(), &e.path, PREVIEW_IMAGE)).flatten();
    match thumb {
        Some(tex) => {
            shown = icons::fit(ui.ctx(), &tex, image_box);
            painter.image(tex.id(), shown, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            style::frame_rect(painter, shown.expand(1.0), style::shade(p, p.document_background, 1.15));
        }
        None => {
            let icon = Rect::from_center_size(image_box.center(), vec2(PREVIEW_ICON, PREVIEW_ICON));
            icons::paint(painter, icon, view::icon_of(e));
        }
    }
    let response = ui.interact(shown, egui::Id::unique(("preview", &e.path)), Sense::click());

    // Name (bold, up to three lines), kind, then facts in two columns
    // meeting at the centre, keys right-aligned.
    let mut y = image_box.bottom() + m::SPACING;
    let mut job = egui::text::LayoutJob::single_section(e.name.clone(), egui::TextFormat::simple(style::bold_font(), c(p.document_text)));
    job.wrap = egui::text::TextWrapping { max_width: area.width(), max_rows: 3, break_anywhere: true, overflow_character: Some('…') };
    job.halign = Align::Center;
    let g = painter.layout_job(job);
    let h = g.size().y;
    painter.galley(pos2(area.center().x, y), g, c(p.document_text));
    y += h;
    style::label(painter, area.center().x, y + m::LABEL_ROW / 2.0, Align::Center, e.kind.name(), style::plain_font(), style::disabled_text(p));
    y += m::LABEL_ROW + m::SPACING;
    let created = std::fs::metadata(&e.path).ok().and_then(|md| md.created().ok());
    let small = FontId::proportional(style::SMALL_SIZE);
    for (k, v) in [
        ("Size", fs::format_size(e.size)),
        ("Created", fs::format_time(created)),
        ("Modified", fs::format_time(e.modified)),
    ] {
        let cy = y + m::LABEL_ROW / 2.0;
        style::label(painter, area.center().x - m::TIGHT, cy, Align::Max, k, small.clone(), style::disabled_text(p));
        style::label(painter, area.center().x + m::TIGHT, cy, Align::Min, &v, small.clone(), c(p.document_text));
        y += m::LABEL_ROW;
    }
    response.double_clicked()
}

/// ↑/↓ move within the active column, → enters the selected folder, ←
/// goes back to its parent's column, Enter / ⌘↓ opens, typing selects.
fn keyboard(ui: &Ui, f: &mut FolderWin, actions: &mut Vec<Action>) {
    if ui.ctx().egui_wants_keyboard_input() || f.editing.is_some() {
        return;
    }
    let keys: Vec<(Key, bool)> = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, modifiers.ctrl)),
                _ => None,
            })
            .collect()
    });
    for (key, cmd) in keys {
        let k = f.active;
        let (_, entries, list) = column(f, k);
        let n = entries.len();
        let cur = list.anchor.filter(|&a| list.selected.contains(&a)).or_else(|| list.selected.iter().next().copied());
        match (key, cmd) {
            (Key::ArrowDown, false) | (Key::ArrowUp, false) if n > 0 => {
                let next = match (cur, key) {
                    (None, _) => 0,
                    (Some(c), Key::ArrowDown) => (c + 1).min(n - 1),
                    (Some(c), _) => c.saturating_sub(1),
                };
                choose(f, k, next);
                reveal(ui, f, k, next);
            }
            (Key::ArrowRight, false) => {
                if cur.is_some_and(|c| entries[c].is_dir) && f.panes.len() > k {
                    f.active = k + 1;
                    if f.panes[k].list.selected.is_empty() && !f.panes[k].entries.is_empty() {
                        choose(f, k + 1, 0);
                    }
                }
            }
            (Key::ArrowLeft, false) if k > 0 => {
                f.panes[k - 1].list.selected.clear();
                f.panes.truncate(k);
                f.active = k - 1;
            }
            (Key::Enter, false) | (Key::ArrowDown, true) => actions.push(Action::Command("open".into())),
            (Key::Delete, false) => actions.push(Action::Command("trash".into())),
            (Key::F2, false) => actions.push(Action::Command("rename".into())),
            _ => {}
        }
    }
    // Type-ahead within the active column.
    let typed: String = ui.input(|i| {
        if i.modifiers.ctrl {
            return String::new();
        }
        i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.clone()) } else { None }).collect()
    });
    if !typed.is_empty() {
        if f.typed.1.elapsed() > std::time::Duration::from_millis(900) {
            f.typed.0.clear();
        }
        f.typed.0.push_str(&typed.to_lowercase());
        f.typed.1 = std::time::Instant::now();
        let k = f.active;
        let prefix = f.typed.0.clone();
        if let Some(row) = column(f, k).1.iter().position(|e| e.name.to_lowercase().starts_with(&prefix)) {
            choose(f, k, row);
            reveal(ui, f, k, row);
        }
    }
}

fn reveal(ui: &Ui, f: &FolderWin, k: usize, row: usize) {
    let path: PathBuf = column(f, k).0.to_owned();
    let top = m::TIGHT + row as f32 * m::LIST_ROW;
    ScrollView::reveal(ui.ctx(), ("col", &path), top - m::TIGHT, top + m::LIST_ROW + m::TIGHT);
}
