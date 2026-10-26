//! Folder window contents: icon, mini-icon and list views, plus the Get
//! Info panel. Shared with the desktop for drawing icons and labels.

use std::time::{Duration, Instant};

use tabula_kit::{
    Menu,
    egui::{self, Align, Color32, FontId, Key, Pos2, Rect, Sense, Ui, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    widgets::{self, Cell, Column, ColumnListView, ScrollView, SCROLL_BAR},
};
use tabula_kit::theme::Palette;

use crate::{FolderWin, SortKey, ViewMode, fs, fs::Entry};

pub enum Action {
    Open(Entry),
    Command(String),
}

/// Icon view: 48 px icons in 96 × 84 cells (icon, gap, two label lines).
pub const ICON: f32 = 48.0;
pub const ICON_CELL: egui::Vec2 = egui::vec2(96.0, 84.0);
/// Mini-icon view: 16 px icons with the label beside them.
pub const MINI: f32 = 16.0;
pub const MINI_CELL: egui::Vec2 = egui::vec2(184.0, m::LIST_ROW);
/// Padding between a selected label's text and its highlight.
const LABEL_PAD: egui::Vec2 = egui::vec2(4.0, 3.0);
/// The selected icon is darkened, as on BeOS.
const SELECTED_TINT: Color32 = Color32::from_gray(185);

#[derive(Clone, Copy, PartialEq)]
pub enum Look {
    Icons,
    Mini,
}

/// Where an item's icon and label land inside its cell.
pub struct ItemLayout {
    pub icon: Rect,
    /// Where the galley is drawn (its rows may extend left of it).
    origin: Pos2,
    galley: std::sync::Arc<egui::Galley>,
}

impl ItemLayout {
    pub fn hit(&self) -> Rect {
        self.icon.union(self.highlight())
    }
    pub fn highlight(&self) -> Rect {
        style::text_highlight(&self.galley, self.origin, style::PLAIN_SIZE, LABEL_PAD)
    }
}

fn snap(ctx: &egui::Context, p: Pos2) -> Pos2 {
    let ppp = ctx.pixels_per_point();
    pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp)
}

pub fn layout_item(ui: &Ui, cell: Rect, label: &str, look: Look, selected: bool, link: bool) -> ItemLayout {
    let font = if link { style::italic_font() } else { style::plain_font() };
    let ctx = ui.ctx();
    match look {
        Look::Icons => {
            let icon = Rect::from_min_size(pos2(cell.center().x - ICON / 2.0, cell.top() + m::TIGHT), vec2(ICON, ICON));
            let width = cell.width() - 2.0 * m::TIGHT;
            let mut job = egui::text::LayoutJob::single_section(label.to_owned(), egui::TextFormat::simple(font.clone(), Color32::PLACEHOLDER));
            job.wrap = egui::text::TextWrapping {
                max_width: width,
                // Selected (or hovered-for-rename) items show their full name.
                max_rows: if selected { 4 } else { 2 },
                break_anywhere: false,
                overflow_character: Some('…'),
            };
            job.halign = Align::Center;
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            // Centred rows are laid out around x = 0; galley.rect holds
            // their true extent.
            let origin = snap(ctx, pos2(cell.center().x, icon.bottom() + m::TIGHT));
            ItemLayout { icon, origin, galley }
        }
        Look::Mini => {
            let icon = Rect::from_min_size(pos2(cell.left() + m::TIGHT, cell.center().y - MINI / 2.0), vec2(MINI, MINI));
            let x = icon.right() + m::MARK_GAP;
            let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), font, Color32::PLACEHOLDER);
            job.wrap = egui::text::TextWrapping::truncate_at_width((cell.right() - x - m::TIGHT).max(0.0));
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let origin = style::text_origin(ctx, &galley, x, cell.center().y, style::PLAIN_SIZE);
            ItemLayout { icon, origin, galley }
        }
    }
}

/// Draw an item. On the desktop labels are drawn in the desktop text
/// colour with a shadow instead of on a document background.
pub fn paint_item(ui: &Ui, l: &ItemLayout, icon: &str, selected: bool, desktop: bool, p: &Palette) {
    let painter = ui.painter();
    if selected {
        icons::paint_tinted(painter, l.icon, icon, SELECTED_TINT);
        // BeOS inverts a selected icon's label; in dark mode that makes it
        // light on dark-on-light, which reads the same way.
        painter.rect_filled(l.highlight(), 2.0, c(p.document_text));
        painter.galley(l.origin, l.galley.clone(), c(p.document_background));
    } else {
        icons::paint(painter, l.icon, icon);
        if desktop {
            painter.galley(l.origin + vec2(1.0, 1.0), l.galley.clone(), c(p.desktop_text_shadow));
            painter.galley(l.origin, l.galley.clone(), c(p.desktop_text));
        } else {
            painter.galley(l.origin, l.galley.clone(), c(p.document_text));
        }
    }
}

/// The icon name for an entry (folders that are special get their own).
pub fn icon_of(e: &Entry) -> &'static str {
    if e.is_dir {
        let home = fs::home();
        if e.path == home {
            return "home";
        }
        if e.path == home.join("Desktop") {
            return "desktop";
        }
        if e.path == fs::trash_files() {
            return if fs::trash_is_empty() { "trash-empty" } else { "trash-full" };
        }
    }
    e.kind.icon()
}

/// A list cell with a 16 px icon before the name.
pub fn name_cell(cell: &Cell, e: &Entry) {
    let icon = Rect::from_min_size(pos2(cell.rect.left() + m::CELL_INSET, cell.rect.center().y - MINI / 2.0), vec2(MINI, MINI));
    if cell.selected {
        icons::paint_tinted(cell.painter, icon, icon_of(e), SELECTED_TINT);
    } else {
        icons::paint(cell.painter, icon, icon_of(e));
    }
    let mut rest = cell.rect;
    rest.min.x = icon.right() + m::MARK_GAP - m::CELL_INSET;
    let font = if e.is_link { style::italic_font() } else { style::plain_font() };
    Cell { rect: rest, ..*cell }.text_with(&e.name, font, cell.text_color);
}

fn item_menu(f: &FolderWin) -> Menu {
    let one = f.list.selected.len() == 1;
    let in_trash = f.path.starts_with(fs::trash_files());
    let mut menu = Menu::new("")
        .item("Open", "open", Some(Key::O))
        .item("Get Info", "info", Some(Key::I))
        .item("Edit Name", "rename", Some(Key::E))
        .enabled(one)
        .item("Duplicate", "duplicate", Some(Key::D))
        .item("Create Link", "link", None)
        .separator()
        .item("Cut", "cut", Some(Key::X))
        .item("Copy", "copy", Some(Key::C))
        .separator();
    if !in_trash {
        menu = menu.item("Move to Trash", "trash", Some(Key::T));
    }
    menu
}

fn background_menu(f: &FolderWin, can_paste: bool) -> Menu {
    Menu::new("")
        .item("New Folder", "new-folder", Some(Key::N))
        .item("Paste", "paste", Some(Key::V))
        .enabled(can_paste)
        .separator()
        .item("Icon View", "view-icons", Some(Key::Num1))
        .checked(f.view == ViewMode::Icons)
        .item("Mini Icon View", "view-mini", Some(Key::Num2))
        .checked(f.view == ViewMode::MiniIcons)
        .item("List View", "view-list", Some(Key::Num3))
        .checked(f.view == ViewMode::List)
        .separator()
        .item("Get Info", "info", Some(Key::I))
        .item("Open Parent", "parent", Some(Key::ArrowUp))
        .enabled(f.path.parent().is_some())
}

/// The bar along the bottom of a folder window: item count on the left,
/// free space on the right (BeOS put the count beside the scroll bar).
fn count_bar(ui: &Ui, rect: Rect, f: &FolderWin, p: &Palette) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, c(p.panel_background));
    style::hline(painter, rect.left(), rect.right(), rect.top(), style::shade(p, p.panel_background, 1.3));
    style::hline(painter, rect.left(), rect.right(), rect.top() + 1.0, c(p.shine));
    let n = f.entries.len();
    let count = match (n, f.list.selected.len()) {
        (1, 0) => "1 item".to_owned(),
        (n, 0) => format!("{n} items"),
        (n, s) => format!("{s} of {n} selected"),
    };
    let small = FontId::proportional(style::SMALL_SIZE);
    let cy = rect.center().y + 1.0; // below the 2 px etched edge
    style::label(painter, rect.left() + m::SPACING, cy, Align::Min, &count, small.clone(), c(p.panel_text));
    if let Some((free, _)) = fs::fs_space(&f.path) {
        let text = format!("{} free", fs::format_size(free));
        style::label(painter, rect.right() - m::SPACING, cy, Align::Max, &text, small, style::disabled_text(p));
    }
}

/// Draw a folder window's content; returns what the user asked for.
pub fn folder_view(ui: &mut Ui, f: &mut FolderWin) -> Vec<Action> {
    let p = style::palette(ui.ctx());
    let mut actions = Vec::new();
    let full = ui.available_rect_before_wrap();
    let bar = Rect::from_min_max(pos2(full.left(), full.bottom() - SCROLL_BAR), full.max);
    let body = Rect::from_min_max(full.min, pos2(full.right(), bar.top()));
    count_bar(ui, bar, f, &p);

    if let Some(err) = &f.error {
        ui.painter().rect_filled(body, 0.0, c(p.document_background));
        style::label(ui.painter(), body.center().x, body.center().y, Align::Center, err, style::plain_font(), style::disabled_text(&p));
        return actions;
    }

    let id = egui::Id::unique(("folder", &f.path));
    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    match f.view {
        ViewMode::List => list_view(&mut body_ui, f, &mut actions),
        ViewMode::Icons => icon_view(&mut body_ui, id, f, Look::Icons, &mut actions),
        ViewMode::MiniIcons => icon_view(&mut body_ui, id, f, Look::Mini, &mut actions),
    }
    if let Some(cmd) = widgets::popup_result(ui.ctx(), id) {
        actions.push(Action::Command(cmd));
    }
    actions
}

fn list_view(ui: &mut Ui, f: &mut FolderWin, actions: &mut Vec<Action>) {
    let cols = [
        Column::new("Name", 220.0),
        Column::new("Size", 80.0).right(),
        Column::new("Modified", 160.0),
        Column::new("Kind", 120.0),
    ];
    let key_col = |k: SortKey| match k {
        SortKey::Name => 0,
        SortKey::Size => 1,
        SortKey::Modified => 2,
        SortKey::Kind => 3,
    };
    f.list.sort = Some((key_col(f.sort.0), f.sort.1));
    let entries = &f.entries;
    let r = ColumnListView::new(("list", &f.path), &cols, entries.len(), &mut f.list).show(ui, |cell| {
        let e = &entries[cell.row];
        match cell.column {
            0 => name_cell(cell, e),
            1 => cell.text(&if e.is_dir { "—".into() } else { fs::format_size(e.size) }),
            2 => cell.text(&fs::format_time(e.modified)),
            _ => cell.text(e.kind.name()),
        }
    });
    if r.sort_changed
        && let Some((col, asc)) = f.list.sort
    {
        let key = [SortKey::Name, SortKey::Size, SortKey::Modified, SortKey::Kind][col.min(3)];
        let cmd = match key {
            SortKey::Name => "sort-name",
            SortKey::Size => "sort-size",
            SortKey::Modified => "sort-modified",
            SortKey::Kind => "sort-kind",
        };
        // sort-* toggles direction when the key is unchanged.
        if f.sort.0 != key || f.sort.1 != asc {
            f.sort = (key, !asc);
            actions.push(Action::Command(cmd.into()));
        }
    }
    if let Some(i) = r.activated
        && let Some(e) = f.entries.get(i)
    {
        actions.push(Action::Open(e.clone()));
    }
    if let Some((_, pos)) = r.context {
        let id = egui::Id::unique(("folder", &f.path));
        widgets::popup_menu_at(ui.ctx(), id, pos, item_menu(f));
    }
}

fn icon_view(ui: &mut Ui, id: egui::Id, f: &mut FolderWin, look: Look, actions: &mut Vec<Action>) {
    let p = style::palette(ui.ctx());
    let cell = if look == Look::Icons { ICON_CELL } else { MINI_CELL };
    let rect = ui.available_rect_before_wrap();
    let inset = if look == Look::Icons { m::SPACING } else { m::TIGHT };
    let width = rect.width() - SCROLL_BAR - 2.0 * inset;
    let cols = ((width / cell.x).floor() as usize).max(1);
    let rows = f.entries.len().div_ceil(cols);
    let height = rows as f32 * cell.y + 2.0 * inset;
    let can_paste = true;
    let focused = ui.ctx().memory(|mm| mm.focused().is_none());

    // Keyboard: arrows move, Enter opens, Delete trashes, typing selects.
    if focused && !f.entries.is_empty() {
        let mut moved = None;
        let cur = f.list.anchor.unwrap_or(0) as isize;
        let n = f.entries.len() as isize;
        let step = |d: isize| (cur + d).clamp(0, n - 1) as usize;
        ui.input(|i| {
            for e in &i.events {
                let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { continue };
                if modifiers.ctrl {
                    if *key == Key::ArrowDown {
                        actions.push(Action::Command("open".into()));
                    }
                    continue;
                }
                match key {
                    Key::ArrowLeft => moved = Some(step(-1)),
                    Key::ArrowRight => moved = Some(step(1)),
                    Key::ArrowUp => moved = Some(step(-(cols as isize))),
                    Key::ArrowDown => moved = Some(step(cols as isize)),
                    Key::Home => moved = Some(0),
                    Key::End => moved = Some(n as usize - 1),
                    Key::Enter => actions.push(Action::Command("open".into())),
                    Key::Delete => actions.push(Action::Command("trash".into())),
                    Key::F2 => actions.push(Action::Command("rename".into())),
                    _ => {}
                }
            }
            for e in &i.events {
                if let egui::Event::Text(t) = e
                    && !i.modifiers.ctrl
                {
                    if f.typed.1.elapsed() > Duration::from_millis(900) {
                        f.typed.0.clear();
                    }
                    f.typed.0.push_str(&t.to_lowercase());
                    f.typed.1 = Instant::now();
                    let typed = f.typed.0.clone();
                    if let Some(k) = f.entries.iter().position(|e| e.name.to_lowercase().starts_with(&typed)) {
                        moved = Some(k);
                    }
                }
            }
        });
        if let Some(k) = moved {
            if ui.input(|i| i.modifiers.shift) {
                f.list.selected.insert(k);
                f.list.anchor = Some(k);
            } else {
                f.list.select(k);
            }
            let top = inset + (k / cols) as f32 * cell.y;
            ScrollView::reveal(ui.ctx(), ("icons", &f.path), top - inset, top + cell.y + inset);
        }
    }

    ScrollView::new(("icons", &f.path)).border(false).background(p.document_background).show(ui, |ui| {
        let origin = ui.min_rect().min;
        let content = Rect::from_min_size(origin, vec2(ui.available_width(), height.max(ui.available_height())));
        let response = ui.allocate_rect(content, Sense::click_and_drag());
        let clip = ui.clip_rect();

        let cell_rect = |i: usize| {
            Rect::from_min_size(origin + vec2(inset + (i % cols) as f32 * cell.x, inset + (i / cols) as f32 * cell.y), cell)
        };
        // Lay out (and hit test) only what can be seen.
        let first = (((clip.top() - origin.y - inset) / cell.y).floor().max(0.0) as usize) * cols;
        let last = ((((clip.bottom() - origin.y - inset) / cell.y).ceil().max(0.0) as usize) + 1) * cols;
        let range = first.min(f.entries.len())..last.min(f.entries.len());
        let layouts: Vec<(usize, ItemLayout)> = range
            .map(|i| (i, layout_item(ui, cell_rect(i), &f.entries[i].name, look, f.list.selected.contains(&i), f.entries[i].is_link)))
            .collect();
        let hit = |pos: Pos2| layouts.iter().find(|(_, l)| l.hit().contains(pos)).map(|(i, _)| *i);
        let mods = ui.input(|i| i.modifiers);

        // Pointer: click selects, double-click opens, drag on the
        // background draws a selection band.
        if response.drag_started() || response.clicked() || response.secondary_clicked() {
            let pos = response.interact_pointer_pos().unwrap_or_default();
            match hit(pos) {
                Some(i) if mods.ctrl || mods.shift => {
                    if response.clicked() || response.drag_started() {
                        if !f.list.selected.remove(&i) {
                            f.list.selected.insert(i);
                        }
                        f.list.anchor = Some(i);
                    }
                }
                Some(i) => {
                    if !f.list.selected.contains(&i) {
                        f.list.select(i);
                    }
                    f.list.anchor = Some(i);
                }
                None => {
                    if !(mods.ctrl || mods.shift) {
                        f.list.selected.clear();
                    }
                    if response.drag_started() {
                        f.band = Some(pos - origin.to_vec2());
                    }
                }
            }
            ui.ctx().memory_mut(|mm| mm.stop_text_input());
        }
        if response.double_clicked()
            && let Some(i) = response.interact_pointer_pos().and_then(hit)
        {
            actions.push(Action::Open(f.entries[i].clone()));
        }
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let menu = if hit(pos).is_some() { item_menu(f) } else { background_menu(f, can_paste) };
            widgets::popup_menu_at(ui.ctx(), id, pos, menu);
        }
        let band = match (f.band, response.dragged(), response.interact_pointer_pos()) {
            (Some(start), true, Some(now)) => {
                let r = Rect::from_two_pos(start + origin.to_vec2(), now);
                // Band selection considers every item, not just visible ones.
                f.list.selected = (0..f.entries.len())
                    .filter(|&i| layout_item(ui, cell_rect(i), &f.entries[i].name, look, false, f.entries[i].is_link).hit().intersects(r))
                    .collect();
                Some(r)
            }
            _ => {
                f.band = None;
                None
            }
        };

        for (i, l) in &layouts {
            let e = &f.entries[*i];
            paint_item(ui, l, icon_of(e), f.list.selected.contains(i), false, &p);
        }
        if let Some(r) = band {
            style::selection_band(ui.painter(), r, &p);
        }
    });
}

// ---- Get Info --------------------------------------------------------------

/// Rows in a Get Info window, for sizing it to fit.
pub fn info_rows(e: &Entry) -> usize {
    5 + usize::from(e.is_link)
}

/// Height of a Get Info window's content for `rows` rows.
pub fn info_height(rows: usize) -> f32 {
    2.0 * m::WINDOW_INSET + INFO_ICON + 2.0 * m::SPACING + 2.0 + rows as f32 * (m::LABEL_ROW + m::TIGHT) - m::TIGHT
}

const INFO_ICON: f32 = 64.0;
/// Width of the right-aligned key column in Get Info.
const INFO_KEY_WIDTH: f32 = 84.0;

pub fn info_view(ui: &mut Ui, e: &Entry, size: Option<(u64, u64)>) {
    let p = style::palette(ui.ctx());
    let size_text = if e.is_dir {
        match size {
            Some((b, n)) => format!("{} ({} items)", fs::format_size(b), n),
            None => "Calculating…".into(),
        }
    } else if e.size < 1024 {
        fs::format_size(e.size)
    } else {
        format!("{} ({} bytes)", fs::format_size(e.size), e.size)
    };
    let location = e.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
    let created = std::fs::metadata(&e.path).ok().and_then(|md| md.created().ok());
    let mut rows = vec![
        ("Size:", size_text),
        ("Where:", location),
        ("Created:", fs::format_time(created)),
        ("Modified:", fs::format_time(e.modified)),
        ("Permissions:", fs::permissions_string(e.mode, e.is_dir)),
    ];
    if e.is_link
        && let Ok(t) = std::fs::read_link(&e.path)
    {
        rows.push(("Link to:", t.display().to_string()));
    }

    let area = ui.available_rect_before_wrap().shrink(m::WINDOW_INSET);
    let painter = ui.painter();
    // Header: the icon, with the name and kind centred beside it.
    let icon = Rect::from_min_size(area.min, vec2(INFO_ICON, INFO_ICON));
    icons::paint(painter, icon, icon_of(e));
    let tx = icon.right() + m::SPACING;
    let cy = icon.center().y;
    style::label(painter, tx, cy - (m::LABEL_ROW + m::TIGHT) / 2.0, Align::Min, &e.name, style::bold_font(), c(p.panel_text));
    style::label(painter, tx, cy + (m::LABEL_ROW + m::TIGHT) / 2.0, Align::Min, e.kind.name(), style::plain_font(), style::disabled_text(&p));

    // An etched rule, then the rows on an exact grid.
    let y = icon.bottom() + m::SPACING;
    style::hline(painter, area.left(), area.right(), y, style::shade(&p, p.panel_background, 1.2));
    style::hline(painter, area.left(), area.right(), y + 1.0, c(p.shine));
    let mut y = y + 2.0 + m::SPACING;
    for (k, v) in rows {
        let cy = y + m::LABEL_ROW / 2.0;
        let kx = area.left() + INFO_KEY_WIDTH;
        style::label(painter, kx, cy, Align::Max, k, style::plain_font(), style::disabled_text(&p));
        let vx = kx + m::LABEL_GAP;
        let mut job = egui::text::LayoutJob::simple_singleline(v, style::plain_font(), c(p.panel_text));
        job.wrap = egui::text::TextWrapping::truncate_at_width((area.right() - vx).max(0.0));
        let g = painter.layout_job(job);
        let pos = style::text_origin(ui.ctx(), &g, vx, cy, style::PLAIN_SIZE);
        painter.galley(pos, g, c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;
    }
}
