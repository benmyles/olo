//! Folder window contents: icon, mini-icon and list views, plus the Get
//! Info panel. Shared with the desktop for drawing icons and labels.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use olo_kit::{
    Menu,
    egui::{self, Align, Color32, FontId, Key, Pos2, Rect, Sense, Ui, pos2, vec2},
    dnd::{self, Hover},
    icons, metrics as m,
    style::{self, c},
    widgets::{self, Cell, Column, ColumnListView, InlineEdit, InlineResult, ScrollView, SCROLL_BAR},
};
use olo_kit::theme::Palette;

use crate::{FolderWin, SortKey, ViewMode, fs, fs::Entry};

pub enum Action {
    Open(Entry),
    Command(String),
    /// An in-place name edit was committed.
    Rename(PathBuf, String),
    /// Items started being dragged: start the drag with this (only while
    /// the button is still down, so in this frame).
    Drag(Drag),
}

/// Items to drag, and how.
pub struct Drag {
    pub paths: Vec<PathBuf>,
    /// The icon of the one the pointer is on.
    pub icon: &'static str,
    /// Its size, logical, and where on it the pointer is.
    pub size: f32,
    pub hotspot: egui::Vec2,
    /// With the secondary button: the place it is dropped on asks what to
    /// do (Move Here, Copy Here, Create Link Here), as on BeOS.
    pub ask: bool,
    /// The name, under the icon (an icon view) or beside it (a list).
    pub name: String,
    pub below: bool,
}

impl Drag {
    /// What it carries: the files, plus Olo-only types saying which
    /// file system they are on (so a folder they are dragged over knows
    /// before the drop whether it would move or copy them) and whether
    /// the drop should ask.
    pub fn data(&self, ctx: &egui::Context) -> dnd::DragData {
        use std::os::unix::fs::MetadataExt;
        // Which folder it comes from, so dropping back there is refused
        // but other folders in the same window (columns) take it.
        let from = self.paths.first().and_then(|p| p.parent()).map(Path::to_path_buf);
        ctx.data_mut(|d| d.insert_temp(drag_from_id(), from.clone()));
        let mut data = dnd::DragData::files(&self.paths, dnd::Actions::ALL);
        if let Some(dir) = &from {
            data = data.from_folder(dir);
        }
        if let Some(dev) = self.paths.first().and_then(|p| std::fs::symlink_metadata(p).ok()).map(|m| m.dev()) {
            data = data.with(&device_type(dev), Vec::new());
        }
        if self.ask {
            data = data.with(ASK_TYPE, Vec::new());
        }
        let n = self.paths.len();
        let name = if n > 1 { format!("{n} items") } else { self.name.clone() };
        let label = if self.below { icons::DragLabel::Below(&name) } else { icons::DragLabel::Right(&name) };
        match icons::drag_icon(ctx, self.icon, self.size, n, self.hotspot, label) {
            Some(icon) => data.icon(icon),
            None => data,
        }
    }
}

/// Marks a drag started with the secondary button.
pub const ASK_TYPE: &str = "application/x-olo-ask";

fn device_type(dev: u64) -> String {
    format!("application/x-olo-device-{dev}")
}

/// Where a drop goes; [`DropTarget::name`] is what [`dnd::accept`] is
/// given, and comes back in the drop.
#[derive(Clone, Debug, PartialEq)]
pub enum DropTarget {
    /// Into a folder (files) or as a clipping file in it (text).
    Folder(PathBuf),
    Trash,
    /// Icons dragged about the desktop: put them where they are dropped.
    Place,
}

impl DropTarget {
    pub fn name(&self) -> String {
        match self {
            DropTarget::Folder(p) => format!("folder:{}", p.display()),
            DropTarget::Trash => "trash".into(),
            DropTarget::Place => "place".into(),
        }
    }

    pub fn parse(name: &str) -> Option<DropTarget> {
        match name {
            "trash" => Some(DropTarget::Trash),
            "place" => Some(DropTarget::Place),
            _ => name.strip_prefix("folder:").map(|p| DropTarget::Folder(PathBuf::from(p))),
        }
    }
}

/// Take what is dragged over folder `dir` if it would go there, choosing
/// as BeOS's Tracker did: move within a file system, copy across them,
/// ⌥ to copy anyway, the secondary button to ask; the Trash takes files
/// only, and moves them. Text becomes a clipping file. Returns whether it
/// took it (to highlight `dir`).
pub fn take_drop(ctx: &egui::Context, h: &Hover, dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let trash = dir == fs::trash_files();
    let target = if trash { DropTarget::Trash } else { DropTarget::Folder(dir.to_owned()) };
    if h.has_files() {
        let action = if h.has(ASK_TYPE) && h.actions.ask && !trash {
            dnd::Action::Ask
        } else if trash {
            if !h.actions.move_ {
                return false;
            }
            dnd::Action::Move
        } else {
            let same_fs = std::fs::metadata(dir).is_ok_and(|m| h.has(&device_type(m.dev())));
            let copy = ctx.input(|i| i.modifiers.alt) || !same_fs || !h.actions.move_;
            // Only our own drags move: another program might delete what
            // it gave us once told it moved.
            if copy || !h.from_us {
                if !h.actions.copy {
                    return false;
                }
                dnd::Action::Copy
            } else {
                dnd::Action::Move
            }
        };
        dnd::accept(ctx, target.name(), "text/uri-list", action);
        true
    } else if trash || !h.actions.copy {
        false
    } else if let Some(t) = h.named_file_type() {
        // A picture dragged out of a web page: the file itself.
        dnd::accept(ctx, target.name(), t, dnd::Action::Copy);
        true
    } else if h.has("text/x-moz-url") {
        // A link: a clipping of it, named by its text.
        dnd::accept(ctx, target.name(), "text/x-moz-url", dnd::Action::Copy);
        true
    } else if let Some(t) = h.text_type() {
        dnd::accept(ctx, target.name(), t, dnd::Action::Copy);
        true
    } else {
        false
    }
}

/// The folder an item dragged things go into, if it is one (and isn't
/// itself being dragged).
fn drop_folder(e: &Entry, dragged: bool) -> Option<&Path> {
    (e.is_dir && !dragged).then_some(e.path.as_path())
}

/// Over an item view: the item it would be dropped on (highlighted as
/// selected), or None for the folder itself; false if it wouldn't take it.
fn drag_from_id() -> egui::Id {
    egui::Id::unique("tracker-drag-from")
}

pub fn item_drop(ctx: &egui::Context, h: &Hover, over: Option<(usize, &Entry, bool)>, here: Option<&Path>) -> (bool, Option<usize>) {
    if let Some((i, e, selected)) = over
        && let Some(dir) = drop_folder(e, h.from_here && selected)
    {
        let took = take_drop(ctx, h, dir);
        if took {
            spring(ctx, dir);
        }
        return (took, Some(i));
    }
    match here {
        // Dropping in the folder it came from would do nothing (another
        // column of the same window is another folder); nor from another
        // app's drag of a file here (Viewer, Quick Look).
        Some(dir) if !(h.from_here && ctx.data(|d| d.get_temp::<Option<PathBuf>>(drag_from_id())).flatten().as_deref() == Some(dir)) && !h.from_folder(dir) => {
            (take_drop(ctx, h, dir), None)
        }
        _ => (false, None),
    }
}

/// How long a drag rests on a folder before it springs open.
pub const SPRING: Duration = Duration::from_millis(1000);

/// The folder a drag is resting on, since when, and whether it opened.
#[derive(Clone)]
struct Spring {
    dir: PathBuf,
    since: Instant,
    opened: bool,
}

fn spring_id() -> egui::Id {
    egui::Id::unique("tracker-spring")
}

/// A drag rests on folder `dir` this frame: after [`SPRING`] it opens
/// (spring-loaded folders), so the drag can go on into it.
fn spring(ctx: &egui::Context, dir: &Path) {
    let now = Instant::now();
    let mut s = ctx.data(|d| d.get_temp::<Spring>(spring_id())).filter(|s| s.dir == dir).unwrap_or(Spring { dir: dir.to_owned(), since: now, opened: false });
    // A pause in hovering (the drag left and came back) starts again.
    if ctx.data(|d| d.get_temp::<Instant>(spring_id().with("seen"))).is_some_and(|t| now - t > Duration::from_millis(250)) {
        s = Spring { dir: dir.to_owned(), since: now, opened: false };
    }
    ctx.data_mut(|d| d.insert_temp(spring_id().with("seen"), now));
    if !s.opened && now - s.since >= SPRING {
        s.opened = true;
        ctx.data_mut(|d| d.insert_temp(spring_id().with("open"), Some(dir.to_owned())));
    }
    if !s.opened {
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    ctx.data_mut(|d| d.insert_temp(spring_id(), s));
}

/// A folder a drag sprang open this frame (the window opens it).
pub fn take_spring(ctx: &egui::Context) -> Option<PathBuf> {
    ctx.data_mut(|d| d.remove_temp::<Option<PathBuf>>(spring_id().with("open"))).flatten()
}

/// Dragging from and dropping on a [`ColumnListView`] of `entries` (a
/// list view, or a pane of the column view, in folder `here`).
pub fn list_drag(ui: &Ui, r: &widgets::ListResponse, entries: &[Entry], list: &widgets::ListState, here: Option<&Path>, actions: &mut Vec<Action>) {
    if let Some((row, ask)) = r.drag_started
        && let Some(e) = entries.get(row)
        && let Some(pos) = ui.input(|i| i.pointer.press_origin())
        && let Some((_, rect)) = r.visible.iter().find(|(i, _)| *i == row)
    {
        let size = MINI;
        let paths = list.selected.iter().filter_map(|&i| entries.get(i)).map(|e| e.path.clone()).collect();
        // Held as the row's icon would be, wherever along the row.
        let icon = rect.center().y - MINI / 2.0;
        let hotspot = vec2((pos.x - rect.left() - m::CELL_INSET).clamp(0.0, size), (pos.y - icon).clamp(0.0, size));
        actions.push(Action::Drag(Drag { paths, icon: icon_of(e), size, hotspot, ask, name: e.name.clone(), below: false }));
    }
    let Some(h) = dnd::hover(ui.ctx()) else { return };
    let over = r.row_at(h.pos).and_then(|i| Some((i, entries.get(i)?, list.selected.contains(&i))));
    let (took, on) = item_drop(ui.ctx(), &h, over, here);
    let p = style::palette(ui.ctx());
    match on.and_then(|i| r.visible.iter().find(|(j, _)| *j == i)) {
        Some((_, rect)) if took => {
            ui.painter().rect_stroke(rect.shrink(0.5), 0.0, egui::Stroke::new(1.0, c(p.keyboard_navigation)), egui::StrokeKind::Inside);
        }
        _ if took => dnd::outline(ui, ui.max_rect()),
        _ => {}
    }
}


/// How long after a click on a selected item's name editing starts (a
/// double-click within it opens the item instead), as on BeOS.
const EDIT_DELAY: Duration = Duration::from_millis(600);

/// Icon view: 48 px icons in 96 × 84 cells (icon, gap, two label lines).
pub const ICON: f32 = 48.0;
pub const ICON_CELL: egui::Vec2 = egui::vec2(96.0, 84.0);
/// Mini-icon view: 16 px icons with the label beside them.
pub const MINI: f32 = 16.0;
pub const MINI_CELL: egui::Vec2 = egui::vec2(184.0, m::LIST_ROW);
/// Padding between a selected label's text and its highlight.
const LABEL_PAD: egui::Vec2 = widgets::INLINE_PAD;
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
    /// A label with no icon (list rows), for in-place editing.
    pub fn text_only(origin: Pos2, galley: std::sync::Arc<egui::Galley>) -> Self {
        ItemLayout { icon: Rect::NOTHING, origin, galley }
    }
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

impl ItemLayout {
    /// Where the label's galley is drawn, and how it is anchored.
    pub fn label_anchor(&self) -> (Pos2, Align) {
        let centred = self.galley.rect.left() < 0.0;
        (self.origin, if centred { Align::Center } else { Align::Min })
    }
}

/// Edit an item's name in place of its label; returns a rename action
/// when committed and clears `editing` once finished.
pub fn edit_label(ui: &mut Ui, l: &ItemLayout, editing: &mut Option<(PathBuf, String)>, link: bool) -> Option<Action> {
    let (path, text) = editing.as_mut()?;
    let (origin, align) = l.label_anchor();
    let font = if link { style::italic_font() } else { style::plain_font() };
    match InlineEdit::new(("rename", path.clone()), text, origin, align).font(font).show(ui) {
        InlineResult::Editing => None,
        InlineResult::Cancel => {
            *editing = None;
            None
        }
        InlineResult::Commit => editing.take().map(|(p, t)| Action::Rename(p, t)),
    }
}

/// Draw an item. On the desktop labels are drawn in the desktop text
/// colour with a shadow instead of on a document background. With
/// `label` false only the icon is drawn (its name is being edited).
pub fn paint_item(ui: &Ui, l: &ItemLayout, icon: &str, selected: bool, desktop: bool, label: bool, p: &Palette) {
    style::describe_item(ui.ctx(), l.galley.text(), l.hit().intersect(ui.clip_rect()), selected);
    let painter = ui.painter();
    if !label {
        icons::paint(painter, l.icon, icon);
        return;
    }
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

/// How far along a download is, as a bar across the foot of its icon
/// (as Finder shows one).
pub fn paint_progress(ui: &Ui, icon: Rect, done: f32) {
    let ctx = ui.ctx();
    let h = (icon.height() / 8.0).clamp(5.0, 8.0).round();
    let w = (icon.width() * 0.75).round();
    let bar = style::snap(ctx, Rect::from_min_size(pos2(icon.center().x - w / 2.0, icon.bottom() - h), vec2(w, h)));
    let p = style::palette(ctx);
    let ink = p.document_text;
    let painter = ui.painter();
    // A round-ended well, and as much of it filled as is done (never
    // less than its round end).
    painter.rect_filled(bar, h / 2.0, Color32::from_rgba_unmultiplied(ink[0], ink[1], ink[2], 46));
    let ppp = ctx.pixels_per_point();
    let end = ((bar.left() + bar.width() * done.clamp(0.0, 1.0)) * ppp).round() / ppp;
    painter.rect_filled(Rect::from_min_max(bar.min, pos2(end.max(bar.left() + bar.height()), bar.bottom())), h / 2.0, c(p.status_bar));
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
    e.app_icon.unwrap_or_else(|| e.kind.icon())
}

/// The rect a list name cell's text occupies (after its icon).
fn name_text_rect(cell: Rect) -> Rect {
    let mut rest = cell;
    rest.min.x = cell.left() + m::CELL_INSET + MINI + m::MARK_GAP - m::CELL_INSET;
    rest
}

/// A list cell with a 16 px icon before the name.
pub fn name_cell(cell: &Cell, e: &Entry) {
    name_cell_with(cell, e, true)
}

fn name_cell_with(cell: &Cell, e: &Entry, text: bool) {
    let icon = Rect::from_min_size(pos2(cell.rect.left() + m::CELL_INSET, cell.rect.center().y - MINI / 2.0), vec2(MINI, MINI));
    if cell.selected {
        icons::paint_tinted(cell.painter, icon, icon_of(e), SELECTED_TINT);
    } else {
        icons::paint(cell.painter, icon, icon_of(e));
    }
    if text {
        let font = if e.is_link { style::italic_font() } else { style::plain_font() };
        Cell { rect: name_text_rect(cell.rect), ..*cell }.text_with(e.label(), font, cell.text_color);
    }
}

pub fn item_menu(f: &FolderWin) -> Menu {
    let one = f.current().1.selected.len() == 1;
    let in_trash = f.current_dir().starts_with(fs::trash_files());
    let mut menu = Menu::new("")
        .item("Open", "open", Some(Key::O))
        .item("Quick Look", "quick-look", Some(Key::Y))
        .enabled(one);
    // A running app of ours can open another window.
    if one && f.selected_entries().first().and_then(fs::running_app).is_some() {
        menu = menu.item("New Window", "new-window", None);
    }
    // An application's windows: grouped in one frame, or each its own.
    if one {
        menu = fs::grouping_item(menu, f.selected_entries().first());
    }
    menu = menu
        .item("Get Info", "info", Some(Key::I))
        .item("Edit Name", "rename", Some(Key::E))
        .enabled(one)
        .item("Duplicate", "duplicate", Some(Key::D))
        .item("Create Link", "link", None);
    // A folder opens in Terminal itself.
    if f.selected_entries().iter().all(|e| e.is_dir) {
        menu = menu.item("Open in Terminal", "terminal-selected", None);
    }
    menu = menu
        .separator()
        .item("Cut", "cut", Some(Key::X))
        .item("Copy", "copy", Some(Key::C))
        .separator();
    if !in_trash {
        menu = menu.item("Move to Trash", "trash", Some(Key::T));
    } else if f.current_dir() == fs::trash_files() {
        // Items at the top of the Trash go back where they came from.
        menu = menu.item("Restore", "restore", None);
    }
    menu
}

pub fn background_menu(f: &FolderWin, can_paste: bool) -> Menu {
    let in_trash = f.current_dir().starts_with(fs::trash_files());
    Menu::new("")
        .item("New Folder", "new-folder", Some(Key::N))
        .enabled(!in_trash)
        .item("Paste", "paste", Some(Key::V))
        .enabled(can_paste && !in_trash)
        .separator()
        .item("Icon View", "view-icons", Some(Key::Num1))
        .checked(f.view == ViewMode::Icons)
        .item("Mini Icon View", "view-mini", Some(Key::Num2))
        .checked(f.view == ViewMode::MiniIcons)
        .item("List View", "view-list", Some(Key::Num3))
        .checked(f.view == ViewMode::List)
        .item("Column View", "view-columns", Some(Key::Num4))
        .checked(f.view == ViewMode::Columns)
        .separator()
        .item("Get Info", "info", Some(Key::I))
        .item("Open in Terminal", "terminal", None)
        .item("Open Parent", "parent", Some(Key::ArrowUp))
        .enabled(f.path.parent().is_some())
}

/// The bar along the bottom of a folder window: item count on the left,
/// free space on the right (BeOS put the count beside the scroll bar).
fn count_bar(ui: &Ui, rect: Rect, f: &FolderWin, p: &Palette) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, c(p.panel_background));
    style::hline(painter, rect.left(), rect.right(), rect.top(), style::shade(p, p.panel_background, 1.3));
    style::hline(painter, rect.left(), rect.right(), rect.top() + style::hairline(ui.ctx()), c(p.shine));
    let (entries, list) = f.current();
    let n = entries.len();
    let loading = f.loading && f.entries.is_empty() && f.error.is_none();
    if loading {
        // Listing runs in the background; keep polling its counter.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    let count = match (n, list.selected.len()) {
        _ if loading => format!("Loading… {} items", f.listed.load(std::sync::atomic::Ordering::Relaxed)),
        (1, 0) => "1 item".to_owned(),
        (n, 0) => format!("{n} items"),
        (n, s) => format!("{s} of {n} selected"),
    };
    let small = FontId::proportional(style::SMALL_SIZE);
    let cy = rect.center().y + style::hairline(ui.ctx()); // below the two-line etched edge
    // In line with the list's first column (its icons) above.
    style::label(painter, rect.left() + m::CELL_INSET, cy, Align::Min, &count, small.clone(), c(p.panel_text));
    if let Some((free, _)) = fs::fs_space(&f.current_dir()) {
        let text = format!("{} free", fs::format_size(free));
        style::label(painter, rect.right() - m::CELL_INSET, cy, Align::Max, &text, small, style::disabled_text(p));
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
        ViewMode::Columns => crate::columns::columns_view(&mut body_ui, id, f, &mut actions),
    }
    if let Some(cmd) = widgets::popup_result(ui.ctx(), id) {
        actions.push(Action::Command(cmd));
    }
    actions
}

fn list_view(ui: &mut Ui, f: &mut FolderWin, actions: &mut Vec<Action>) {
    let reveal = f.take_show();
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
    let editing_row = f.editing.as_ref().and_then(|(p, _)| entries.iter().position(|e| &e.path == p));
    let mut edit_cell = None;
    let name = |i: usize| entries[i].name.as_str();
    let r = ColumnListView::new(("list", &f.path), &cols, entries.len(), &mut f.list).draggable().reveal(reveal).type_select(&name).show(ui, |cell| {
        let e = &entries[cell.row];
        match cell.column {
            0 if Some(cell.row) == editing_row => {
                name_cell_with(cell, e, false);
                edit_cell = Some(cell.rect);
            }
            0 => name_cell(cell, e),
            1 => cell.text(&e.size_text()),
            2 => cell.text(&fs::format_time(e.modified)),
            _ => cell.text(&e.kind_text()),
        }
    });
    if let (Some(rect), Some(row)) = (edit_cell, editing_row) {
        let e = &f.entries[row];
        let font = if e.is_link { style::italic_font() } else { style::plain_font() };
        let g = ui.painter().layout_no_wrap(e.name.clone(), font, Color32::BLACK);
        let text = name_text_rect(rect);
        let origin = style::text_origin(ui.ctx(), &g, text.left() + m::CELL_INSET, text.center().y, style::PLAIN_SIZE);
        let l = ItemLayout { icon: Rect::NOTHING, origin, galley: g };
        if let Some(a) = edit_label(ui, &l, &mut f.editing, e.is_link) {
            actions.push(a);
        }
    } else if editing_row.is_none() {
        f.editing = None;
    }
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
    // Enter or a double-click: what's selected, if it's one of them.
    if let Some(i) = r.activated
        && let Some(e) = f.entries.get(i)
    {
        actions.push(if f.list.selected.contains(&i) { Action::Command("open".into()) } else { Action::Open(e.clone()) });
    }
    list_drag(ui, &r, &f.entries, &f.list, Some(&f.current_dir()), actions);
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

    // A selection made in code (Show in Tracker, a new item, one put back
    // after a reload): into view, with some of what's around it.
    if let Some(row) = f.take_show() {
        let around = ((rect.height() / cell.y / 2.0).floor() - 1.0).max(0.0) * cell.y;
        let top = inset + (row / cols) as f32 * cell.y;
        ScrollView::reveal(ui.ctx(), ("icons", &f.path), (top - inset - around).max(0.0), top + cell.y + inset + around);
    }

    // Keyboard: the arrows, Page Up and Down, Home and End move (with ⇧,
    // grow or shrink the selection from the anchor), Enter opens, Delete
    // trashes, typing selects.
    if focused && !f.entries.is_empty() {
        let n = f.entries.len();
        let page = (((rect.height() - 2.0 * inset) / cell.y).floor().max(1.0) as isize) * cols as isize;
        let mut moved = None;
        let list = &f.list;
        ui.input(|i| {
            for e in &i.events {
                let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { continue };
                // Alt's are the menus' (Windows' Alt+↑, Alt+Enter).
                if modifiers.alt {
                    continue;
                }
                if modifiers.ctrl {
                    if *key == Key::ArrowDown {
                        actions.push(Action::Command("open".into()));
                    }
                    continue;
                }
                let to = match key {
                    Key::ArrowLeft => list.step(-1, n),
                    Key::ArrowRight => list.step(1, n),
                    Key::ArrowUp => list.step(-(cols as isize), n),
                    Key::ArrowDown => list.step(cols as isize, n),
                    Key::PageUp => list.step(-page, n),
                    Key::PageDown => list.step(page, n),
                    Key::Home => Some(0),
                    Key::End => Some(n - 1),
                    Key::Enter => {
                        actions.push(Action::Command("open".into()));
                        None
                    }
                    Key::Delete if plain_extra("trash", Key::Delete) => {
                        actions.push(Action::Command("trash".into()));
                        None
                    }
                    Key::F2 if plain_extra("rename", Key::F2) => {
                        actions.push(Action::Command("rename".into()));
                        None
                    }
                    _ => None,
                };
                if let Some(to) = to {
                    moved = Some((to, modifiers.shift));
                }
            }
        });
        let entries = &f.entries;
        if let Some(k) = widgets::type_select(ui, id, n, f.list.cursor(), |i| entries[i].name.as_str()) {
            moved = Some((k, false));
        }
        if let Some((k, extend)) = moved {
            f.list.move_to(k, extend);
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
            .map(|i| (i, layout_item(ui, cell_rect(i), f.entries[i].label(), look, f.list.selected.contains(&i), f.entries[i].is_link)))
            .collect();
        let hit = |pos: Pos2| layouts.iter().find(|(_, l)| l.hit().contains(pos)).map(|(i, _)| *i);
        let mods = ui.input(|i| i.modifiers);

        // Pointer: click selects, double-click opens, drag on the
        // background draws a selection band.
        if response.drag_started() || response.clicked() || response.secondary_clicked() {
            // A drag: where the button went down, not where it got to.
            let pos = if response.drag_started() { ui.input(|i| i.pointer.press_origin()) } else { None };
            let pos = pos.or(response.interact_pointer_pos()).unwrap_or_default();
            // A plain click on the name of the only selected item edits it
            // (after a moment, so a double-click can still open it).
            if response.clicked() && !(mods.ctrl || mods.shift) {
                let on_label = layouts.iter().find(|(_, l)| l.highlight().contains(pos)).map(|(i, _)| *i);
                f.pending_edit = match on_label {
                    Some(i) if f.list.selected.len() == 1 && f.list.selected.contains(&i) => {
                        Some((f.entries[i].path.clone(), Instant::now()))
                    }
                    _ => None,
                };
            }
            match hit(pos) {
                // Kept for the menu, if it's one of them.
                Some(i) if response.secondary_clicked() => {
                    if !f.list.selected.contains(&i) {
                        f.list.select(i);
                    }
                }
                // Alone; ⌘ adds or takes away; ⇧ everything from the
                // anchor to it (in the folder's order), as everywhere else.
                Some(i) if response.clicked() => f.list.click(i, widgets::mods(mods)),
                // A drag takes what's selected, it too (with ⌘ or ⇧, added
                // to them).
                Some(i) => {
                    if !f.list.selected.contains(&i) {
                        if mods.ctrl || mods.shift {
                            f.list.selected.insert(i);
                            f.list.cursor = Some(i);
                        } else {
                            f.list.select(i);
                        }
                    }
                    if response.drag_started()
                        && let Some((_, l)) = layouts.iter().find(|(j, _)| *j == i)
                    {
                        f.pending_edit = None;
                        actions.push(Action::Drag(Drag {
                            paths: f.selected_paths(),
                            icon: icon_of(&f.entries[i]),
                            size: l.icon.width(),
                            hotspot: (pos - l.icon.min).clamp(egui::Vec2::ZERO, l.icon.size()),
                            ask: response.dragged_by(egui::PointerButton::Secondary),
                            name: f.entries[i].name.clone(),
                            below: look == Look::Icons,
                        }));
                    }
                }
                None => {
                    // A band: with ⇧ adding to what's selected, with ⌘
                    // flipping what it covers.
                    if response.drag_started() {
                        f.band = Some((pos - origin.to_vec2(), f.list.band(widgets::mods(mods))));
                    }
                    if !(mods.ctrl || mods.shift) {
                        f.list.selected.clear();
                    }
                }
            }
            ui.ctx().memory_mut(|mm| mm.stop_text_input());
        }
        if response.double_clicked()
            && let Some(i) = response.interact_pointer_pos().and_then(hit)
        {
            f.pending_edit = None;
            actions.push(Action::Open(f.entries[i].clone()));
        }
        if let Some((path, at)) = f.pending_edit.clone() {
            if at.elapsed() >= EDIT_DELAY {
                if f.list.selected.len() == 1 && f.selected_paths().first() == Some(&path) {
                    f.edit_name(&path);
                } else {
                    f.pending_edit = None;
                }
            } else {
                ui.ctx().request_repaint_after(EDIT_DELAY - at.elapsed());
            }
        }
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let menu = if hit(pos).is_some() { item_menu(f) } else { background_menu(f, can_paste) };
            widgets::popup_menu_at(ui.ctx(), id, pos, menu);
        }
        let band = match (f.band.clone(), response.dragged(), response.interact_pointer_pos()) {
            (Some((start, base)), true, Some(now)) => {
                let r = Rect::from_two_pos(start + origin.to_vec2(), now);
                // Every item it covers, not just those showing: those in the
                // rows it crosses.
                let row = |y: f32| ((y - origin.y - inset) / cell.y).floor().max(0.0) as usize;
                let items = (row(r.top()) * cols).min(f.entries.len())..((row(r.bottom()) + 1) * cols).min(f.entries.len());
                f.list.selected = base.over(
                    items.filter(|&i| layout_item(ui, cell_rect(i), f.entries[i].label(), look, false, f.entries[i].is_link).hit().intersects(r)),
                );
                // Near or past the top or bottom: scroll on.
                let edge = cell.y / 4.0;
                let step = if now.y > clip.bottom() - edge {
                    ((now.y - clip.bottom() + edge) / 2.0).clamp(2.0, cell.y)
                } else if now.y < clip.top() + edge {
                    -((clip.top() + edge - now.y) / 2.0).clamp(2.0, cell.y)
                } else {
                    0.0
                };
                if step != 0.0 {
                    let y = if step > 0.0 { clip.bottom() - origin.y + step } else { clip.top() - origin.y + step };
                    ScrollView::reveal(ui.ctx(), ("icons", &f.path), (y - 1.0).max(0.0), (y + 1.0).min(height));
                    ui.ctx().request_repaint();
                }
                Some(r)
            }
            _ => {
                f.band = None;
                None
            }
        };

        // Something dragged over it: onto a folder in it, or into it.
        let (took, drop_on) = match dnd::hover(ui.ctx()) {
            Some(h) => {
                let over = hit(h.pos).filter(|_| clip.contains(h.pos)).map(|i| (i, &f.entries[i], f.list.selected.contains(&i)));
                item_drop(ui.ctx(), &h, over, Some(&f.current_dir()))
            }
            None => (false, None),
        };

        let editing = f.editing.as_ref().and_then(|(p, _)| f.entries.iter().position(|e| &e.path == p));
        if f.editing.is_some() && editing.is_none() {
            f.editing = None; // the item went away
        }
        for (i, l) in &layouts {
            let e = &f.entries[*i];
            paint_item(ui, l, icon_of(e), f.list.selected.contains(i) || drop_on == Some(*i), false, editing != Some(*i), &p);
            if let Some(done) = e.progress() {
                paint_progress(ui, l.icon, done);
            }
        }
        if took && drop_on.is_none() {
            dnd::outline(ui, clip);
        }
        if let Some(i) = editing {
            match layouts.iter().find(|(j, _)| *j == i) {
                Some((_, l)) => {
                    if let Some(a) = edit_label(ui, l, &mut f.editing, f.entries[i].is_link) {
                        actions.push(a);
                    }
                }
                None => {
                    // Scroll the item into view; editing resumes there.
                    let top = inset + (i / cols) as f32 * cell.y;
                    ScrollView::reveal(ui.ctx(), ("icons", &f.path), top - inset, top + cell.y + inset);
                }
            }
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

/// Get Info's contents; a drag of the item out by its icon, when one starts.
pub fn info_view(ui: &mut Ui, e: &Entry, size: Option<(u64, u64)>) -> Option<Drag> {
    let p = style::palette(ui.ctx());
    let size_text = if e.is_dir {
        match size {
            Some((b, 1)) => format!("{} (1 item)", fs::format_size(b)),
            Some((b, n)) => format!("{} ({n} items)", fs::format_size(b)),
            None => "Calculating…".into(),
        }
    } else if e.size < 1024 || e.downloading.is_some() {
        e.size_text()
    } else {
        format!("{} ({} bytes)", fs::format_size(e.size), e.size)
    };
    let location = e.path.parent().map(fs::display_path).unwrap_or_default();
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
    // The icon is the item: it drags out, as from a folder window.
    let grab = ui.interact(icon, ui.make_persistent_id("info-icon"), Sense::drag());
    let drag = grab.drag_started().then(|| {
        let at = ui.input(|i| i.pointer.press_origin()).map_or(vec2(INFO_ICON / 2.0, INFO_ICON / 2.0), |o| o - icon.min);
        Drag { paths: vec![e.path.clone()], icon: icon_of(e), size: INFO_ICON, hotspot: at, ask: grab.dragged_by(egui::PointerButton::Secondary), name: e.name.clone(), below: true }
    });
    let tx = icon.right() + m::SPACING;
    let cy = icon.center().y;
    style::label(painter, tx, cy - (m::LABEL_ROW + m::TIGHT) / 2.0, Align::Min, e.label(), style::bold_font(), c(p.panel_text));
    style::label(painter, tx, cy + (m::LABEL_ROW + m::TIGHT) / 2.0, Align::Min, e.kind_text(), style::plain_font(), style::disabled_text(&p));

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
    drag
}

/// Delete and F2 trash and rename in a view as well as the menus' keys,
/// unless they are the menus' keys for it already (Windows'), which would
/// do it twice.
pub fn plain_extra(command: &str, key: Key) -> bool {
    olo_kit::keyboard::shortcut(command).is_none_or(|s| !(s.key == key && !s.command && !s.alt && !s.shift))
}
