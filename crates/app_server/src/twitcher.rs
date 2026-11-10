//! Twitcher: the strip of app icons shown while ⌘Tab, ⌥Tab or ⌘\` is
//! held, as on BeOS: ⌥Tab's the apps on the workspaces showing, ⌘Tab's a
//! row for each workspace number with windows on it (numbered at the left
//! when there are more than one). With more than one display (each has
//! its own workspaces), each display's windows are a column of their own,
//! in an etched box named across its top edge as a BeOS BBox is ("D1
//! Studio Display", so a cell is that display's workspace of its row's
//! number), the displays in the order
//! they stand on the desk; the box of the display a copy of the panel is
//! on has its little screen lit. `desktop::twitch_step` decides what is
//! chosen; this draws it.
//!
//! App names and icons come from .desktop entries and icon themes, which
//! means file system work, so they are looked up on a worker thread the
//! first time an app id is seen (`prewarm`, called whenever the window
//! list changes) and drawn from the cache. Until an app is resolved it
//! shows under its app id with a generic icon.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use crate::render::Exact;
use olo_theme::Palette;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    utils::{Buffer, Physical, Point, Rectangle, Size, Transform},
};
use tiny_skia::{FillRule, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint, Transform as SkTransform};

use crate::{shell::fill, text, theme};

/// Logical sizes.
const ICON: i32 = 48;
const CELL: i32 = 64;
const PAD: i32 = 12;
const LABEL: i32 = 22;
const MIN_WIDTH: i32 = 240;
const FONT: f32 = 12.0;
/// The workspace numbers' column, and the gap between rows.
const GUTTER: i32 = 24;
const ROW_GAP: i32 = 6;
/// Grouped by display: the line the boxes' names sit across, the padding
/// inside a box, the gap between boxes, and how far in a box's name
/// starts (after a little screen, `SCREEN`).
const CAPTION: i32 = 20;
const BOX_PAD: i32 = 10;
const BOX_GAP: i32 = 10;
const NAME_IN: i32 = 10;
const NAME_FONT: f32 = 11.0;
const SCREEN: (i32, i32) = (13, 10);
const SCREEN_GAP: i32 = 4;
/// The chosen app's plate: how far it reaches beyond the icon, and its
/// corners (the kit's `metrics::SELECTION_PAD` and `CARD_RADIUS`).
const PLATE_PAD: i32 = 4;
const PLATE_RADIUS: f64 = 5.0;

/// Device sizes icons are decoded at; others are scaled from the next one up.
const ICON_PX: [u32; 2] = [48, 96];

#[derive(Clone)]
struct AppInfo {
    name: String,
    icons: Vec<(u32, Arc<Pixmap>)>,
    /// Ours, or an entry gave it: else looked up again once the
    /// applications folders change ([`recheck`]).
    found: bool,
}

type Cache = Mutex<HashMap<String, Option<AppInfo>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Bumped whenever an app's name and icons are (re)looked up, so the
/// strip is drawn again with them.
static LOOKED_UP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn store(app_id: String, info: AppInfo) {
    cache().lock().unwrap().insert(app_id, Some(info));
    LOOKED_UP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn to_pixmap(img: olo_xdg::Image) -> Option<Pixmap> {
    let img = img.into_premultiplied();
    let size = tiny_skia::IntSize::from_wh(img.width, img.height)?;
    Pixmap::from_vec(img.rgba, size)
}

fn resolve(app_id: &str) -> AppInfo {
    let (name, icon, found) = match olo_xdg::apps::our_app(app_id) {
        Some((name, icon)) => (name.to_owned(), icon.to_owned(), true),
        None => {
            let entry = olo_xdg::desktop_entry::find(app_id);
            let name = entry.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| fallback_name(app_id));
            let found = entry.is_some();
            (name, entry.and_then(|e| e.icon).unwrap_or_else(|| "application".into()), found)
        }
    };
    let icons = ICON_PX
        .iter()
        .filter_map(|&px| {
            let img = olo_xdg::load_icon(&icon, px).or_else(|| olo_xdg::load_icon("application", px))?;
            Some((px, Arc::new(to_pixmap(img)?)))
        })
        .collect();
    AppInfo { name, icons, found }
}

fn fallback_name(app_id: &str) -> String {
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    let mut c = last.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Start looking up any app ids not seen before.
pub fn prewarm<'a>(app_ids: impl IntoIterator<Item = &'a str>) {
    let mut todo = Vec::new();
    {
        let mut c = cache().lock().unwrap();
        for id in app_ids {
            if !c.contains_key(id) {
                c.insert(id.to_owned(), None);
                todo.push(id.to_owned());
            }
        }
    }
    if todo.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for id in todo {
            let info = resolve(&id);
            store(id, info);
        }
    });
}

/// The Twitcher is shown: apps no entry gave are looked up again, on a
/// worker, if an applications folder changed since the last time (an
/// AppImage's entry, made as it ran: `olo_appimage`; an app installed
/// while it ran). Nothing while every app's is known.
pub fn recheck() {
    let ids: Vec<String> = cache().lock().unwrap().iter().filter(|(_, i)| i.as_ref().is_some_and(|i| !i.found)).map(|(id, _)| id.clone()).collect();
    if ids.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        static STAMPS: Mutex<Vec<Option<std::time::SystemTime>>> = Mutex::new(Vec::new());
        let now = olo_xdg::desktop_entry::stamps();
        if std::mem::replace(&mut *STAMPS.lock().unwrap(), now.clone()) == now {
            return;
        }
        for id in ids {
            let info = resolve(&id);
            store(id, info);
        }
    });
}

/// Look every app seen so far up again, in the theme just chosen (each
/// keeps its old icons until its new ones are ready).
pub fn theme_changed() {
    let ids: Vec<String> = cache().lock().unwrap().keys().cloned().collect();
    std::thread::spawn(move || {
        let theme = olo_xdg::themes::current().id;
        for id in ids {
            let info = resolve(&id);
            // Chosen again meanwhile: that change's lookup takes over.
            if olo_xdg::themes::current().id != theme {
                return;
            }
            store(id, info);
        }
    });
}

fn info(app_id: &str) -> Option<AppInfo> {
    cache().lock().unwrap().get(app_id).cloned().flatten()
}

/// One cell: an app (⌘Tab, ⌥Tab) or a window (⌘\`).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub app_id: String,
    /// The window title for ⌘\`; the app's name is used for ⌘Tab and ⌥Tab.
    pub title: Option<String>,
    /// Its display's place in [`Shown::displays`] (0 when not grouped).
    pub display: usize,
}

/// A display whose windows are grouped together.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    /// Its connector, to know which display a copy of the panel is on.
    pub output: String,
    /// What it is called (`OutputInfo::display_name`, numbered when two
    /// are called the same).
    pub name: String,
}

/// A workspace's cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub workspace: u32,
    pub entries: Vec<Entry>,
}

/// What Twitcher shows: its rows (in workspace order), the chosen cell,
/// and whether the cells are lettered (keyboard hints are on).
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    /// Each row's cells, those of each display together, in display order.
    pub rows: Vec<Row>,
    /// With more than one display, those with windows, as they stand left
    /// to right: the cells are grouped by them. Empty otherwise.
    pub displays: Vec<Display>,
    pub row: usize,
    pub index: usize,
    pub letters: bool,
}

impl Shown {
    fn cells(&self) -> usize {
        self.rows.iter().map(|r| r.entries.len()).sum()
    }

    /// The letter of cell `index` of row `row`, if it has one: every cell
    /// in reading order, while letters last.
    pub fn letter(&self, row: usize, index: usize) -> Option<char> {
        if !self.letters {
            return None;
        }
        let n = self.rows[..row].iter().map(|r| r.entries.len()).sum::<usize>() + index;
        (n < crate::hints::LETTERS.len()).then(|| crate::hints::label(n))
    }

    /// The cell lettered `c`.
    pub fn find(&self, c: char) -> Option<(usize, usize)> {
        let n = crate::hints::LETTERS.find(c)?;
        let mut first = 0;
        for (r, row) in self.rows.iter().enumerate() {
            if n < first + row.entries.len() {
                return self.letters.then_some((r, n - first));
            }
            first += row.entries.len();
        }
        None
    }

    fn selected(&self) -> Option<&Entry> {
        self.rows.get(self.row)?.entries.get(self.index)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Key {
    shown: Shown,
    /// The display this copy is on, among `shown.displays`.
    here: Option<usize>,
    /// Lookups so far (so a late one, or a new theme's, redraws).
    looked_up: u64,
    scale_milli: i64,
    generation: u64,
    /// The display's width (logical): how wide the label's tip may be.
    width: i32,
}

/// How many looks of the panel are kept drawn while it is up, for each
/// display it is drawn differently on.
const LOOKS: usize = 8;

/// A picture, ready for the GPU, and its size.
type Picture = (MemoryRenderBuffer, Size<i32, Physical>);

/// The panel as drawn for its last few looks (each selection), the
/// current one first, with the chosen label's tip if the panel cuts it:
/// cycling back to one shows the texture already on the GPU rather than
/// uploading another, which can hold up a frame by milliseconds while
/// the GPU is busy. Forgotten when the panel goes.
#[derive(Default)]
pub struct Overlay {
    cached: Vec<(Key, Picture, Option<Picture>)>,
}

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay").field("cached", &self.cached.iter().map(|(k, (_, s), _)| (k, s)).collect::<Vec<_>>()).finish()
    }
}

impl Overlay {
    /// The panel is gone: let its pictures go too.
    pub fn forget(&mut self) {
        self.cached.clear();
    }

    /// The Twitcher panel centred in output `output` of `output_size`
    /// (logical) at `scale`, and under it the chosen label whole on a
    /// tooltip's plate if the panel cuts it short (at once: ⌘ is held, as
    /// it shows Olo's other tips at once); nothing if there is nothing to
    /// show.
    pub fn elements<R>(&mut self, renderer: &mut R, shown: &Shown, output: &str, output_size: Size<i32, smithay::utils::Logical>, scale: f64) -> Vec<Exact<MemoryRenderBufferRenderElement<R>>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if shown.cells() < 2 {
            return Vec::new();
        }
        let here = shown.displays.iter().position(|d| d.output == output);
        let key = Key {
            shown: shown.clone(),
            here,
            looked_up: LOOKED_UP.load(std::sync::atomic::Ordering::Relaxed),
            scale_milli: (scale * 1000.0).round() as i64,
            generation: theme::generation(),
            width: output_size.w,
        };
        if let Some(i) = self.cached.iter().position(|(k, _, _)| *k == key) {
            let looked = self.cached.remove(i);
            self.cached.insert(0, looked);
        } else {
            let p = theme::palette();
            let panel = gpu(&draw(&p, shown, here, scale));
            let tip =
                cut_label(shown, scale).and_then(|label| crate::tabtip::plate(&p, &label, ((output_size.w as f64 - 2.0 * crate::tabtip::MARGIN) * scale).floor() as i32, scale)).map(|pm| gpu(&pm));
            self.cached.insert(0, (key, panel, tip));
            self.cached.truncate(LOOKS * shown.displays.len().max(1));
        }
        let Some((_, (buffer, phys), tip)) = self.cached.first() else { return Vec::new() };
        let out = output_size.to_f64().to_physical(scale);
        let loc: Point<f64, Physical> = (((out.w - phys.w as f64) / 2.0).round(), ((out.h - phys.h as f64) / 2.0).round()).into();
        let mut shown: Vec<_> = crate::render::picture(renderer, loc, buffer, (phys.w, phys.h), None, Kind::Unspecified).into_iter().collect();
        if let Some((buffer, size)) = tip {
            let at: Point<f64, Physical> = (((out.w - size.w as f64) / 2.0).round(), loc.y + phys.h as f64 + (crate::tabtip::GAP * scale).round()).into();
            shown.extend(crate::render::picture(renderer, at, buffer, (size.w, size.h), None, Kind::Unspecified));
        }
        shown
    }
}

/// Where things go in the panel, in logical pixels.
#[derive(Debug, PartialEq)]
struct Layout {
    size: (i32, i32),
    /// Each column's left edge and width: one across the panel, or a box
    /// for each display.
    columns: Vec<(i32, i32)>,
    /// The first row's top.
    top: i32,
    /// Each display's name, cut to fit its box.
    names: Vec<String>,
}

fn layout(shown: &Shown) -> Layout {
    let rows = shown.rows.len().max(1) as i32;
    let cells_h = rows * CELL + (rows - 1) * ROW_GAP;
    let gutter = if shown.rows.len() > 1 { GUTTER } else { 0 };
    if shown.displays.is_empty() {
        let widest = shown.rows.iter().map(|r| r.entries.len()).max().unwrap_or(0) as i32;
        let w = (2 * PAD + gutter + widest * CELL).max(MIN_WIDTH);
        let columns = vec![(PAD + gutter, w - 2 * PAD - gutter)];
        return Layout { size: (w, PAD + cells_h + LABEL + PAD), columns, top: PAD, names: Vec::new() };
    }
    let (mut columns, mut names, mut x) = (Vec::new(), Vec::new(), 0);
    let label = NAME_IN + SCREEN.0 + SCREEN_GAP;
    for (d, display) in shown.displays.iter().enumerate() {
        let most = shown.rows.iter().map(|r| r.entries.iter().filter(|e| e.display == d).count()).max().unwrap_or(0);
        let cells = most as i32 * CELL + 2 * BOX_PAD;
        // A name may make a box wider, up to three cells' worth.
        let room = cells.max(3 * CELL + 2 * BOX_PAD) - label - NAME_IN;
        let name = text::ellipsize(&display.name, NAME_FONT, true, room as f32);
        let w = cells.max(label + text::measure(&name, NAME_FONT, true).ceil() as i32 + NAME_IN);
        columns.push((x, w));
        names.push(name);
        x += w + BOX_GAP;
    }
    let boxes = x - BOX_GAP;
    let w = (2 * PAD + gutter + boxes).max(MIN_WIDTH);
    let left = PAD + gutter + (w - 2 * PAD - gutter - boxes) / 2;
    for c in &mut columns {
        c.0 += left;
    }
    Layout { size: (w, PAD + CAPTION + cells_h + BOX_PAD + LABEL + PAD), columns, top: PAD + CAPTION, names }
}

/// Each cell (row, index) and its top left in the panel (logical): each
/// row's cells of each column centred in it.
fn cell_places(shown: &Shown, lay: &Layout) -> Vec<((usize, usize), (i32, i32))> {
    let grouped = !shown.displays.is_empty();
    let mut out = Vec::new();
    for (r, row) in shown.rows.iter().enumerate() {
        let top = lay.top + r as i32 * (CELL + ROW_GAP);
        for (d, &(x, cw)) in lay.columns.iter().enumerate() {
            let cells: Vec<usize> = (0..row.entries.len()).filter(|&i| row.entries[i].display == d || !grouped).collect();
            let x0 = x + (cw - cells.len() as i32 * CELL) / 2;
            out.extend(cells.iter().enumerate().map(|(k, &i)| ((r, i), (x0 + k as i32 * CELL, top))));
        }
    }
    out
}

/// The cell (row, index) at `at` (logical, from the display's top left)
/// of the panel as `Overlay::element` centres it on a display of
/// `output_size`: the pointer choosing while ⌘ is held.
pub fn cell_at(shown: &Shown, output_size: (i32, i32), at: (f64, f64)) -> Option<(usize, usize)> {
    if shown.cells() < 2 {
        return None;
    }
    let lay = layout(shown);
    let (x, y) = (at.0 - (output_size.0 - lay.size.0) as f64 / 2.0, at.1 - (output_size.1 - lay.size.1) as f64 / 2.0);
    cell_places(shown, &lay).into_iter().find_map(|(cell, (cx, cy))| ((cx as f64..(cx + CELL) as f64).contains(&x) && (cy as f64..(cy + CELL) as f64).contains(&y)).then_some(cell))
}

/// `pm` ready for the GPU, opaque throughout.
fn gpu(pm: &Pixmap) -> Picture {
    let size: Size<i32, Buffer> = (pm.width() as i32, pm.height() as i32).into();
    let buffer = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, size, 1, Transform::Normal, Some(vec![Rectangle::from_size(size)]));
    (buffer, (size.w, size.h).into())
}

/// What the chosen cell is called: its window's title (⌘\`) or its app's
/// name.
fn label(shown: &Shown) -> Option<String> {
    let e = shown.selected()?;
    Some(e.title.clone().or_else(|| info(&e.app_id).map(|i| i.name)).unwrap_or_else(|| fallback_name(&e.app_id)))
}

/// The chosen cell's label, whole, if the panel at `s` cuts it short.
fn cut_label(shown: &Shown, s: f64) -> Option<String> {
    let label = label(shown)?;
    let room = (layout(shown).size.0 - 2 * PAD) as f64 * s;
    (text::measure(&label, FONT * s as f32, true) > room.round() as f32).then_some(label)
}

/// Twitcher at `s` device pixels per logical one, on display `here` of
/// `shown.displays`.
fn draw(p: &Palette, shown: &Shown, here: Option<usize>, s: f64) -> Pixmap {
    let lay = layout(shown);
    let (lw, lh) = lay.size;
    let px = |l: i32| (l as f64 * s).round() as i32;
    let u = px(1).max(1);
    let (w, h) = (px(lw), px(lh));
    let mut pm = Pixmap::new(w as u32, h as u32).expect("twitcher pixmap");
    // A raised BeOS panel with a dark outline.
    fill(&mut pm, 0, 0, w, h, p.frame_focused[0]);
    fill(&mut pm, u, u, w - 2 * u, h - 2 * u, p.panel_background);
    fill(&mut pm, u, u, w - 2 * u, u, p.shine);
    fill(&mut pm, u, u, u, h - 2 * u, p.shine);
    fill(&mut pm, u, h - 2 * u, w - 2 * u, u, p.shadow);
    fill(&mut pm, w - 2 * u, u, u, h - 2 * u, p.shadow);

    let grouped = !shown.displays.is_empty();
    let rows = shown.rows.len().max(1) as i32;
    let bottom = lay.top + rows * CELL + (rows - 1) * ROW_GAP;
    if grouped {
        for (d, &(x, cw)) in lay.columns.iter().enumerate() {
            display_box(&mut pm, p, (x, cw), (PAD + CAPTION / 2, bottom + BOX_PAD), &lay.names[d], here == Some(d), s);
        }
    }

    let numbered = shown.rows.len() > 1;
    let icon_px = px(ICON) as u32;
    let size = FONT * s as f32;
    for (r, row) in shown.rows.iter().enumerate() {
        let top = lay.top + r as i32 * (CELL + ROW_GAP);
        if r > 0 {
            // An etched line between workspaces, across each box when
            // grouped.
            let y = px(top - ROW_GAP / 2);
            let etch = p.shade(p.panel_background, 1.22);
            let spans: Vec<(i32, i32)> = if grouped { lay.columns.iter().map(|&(x, cw)| (x + BOX_PAD, cw - 2 * BOX_PAD)).collect() } else { vec![(PAD, lw - 2 * PAD)] };
            for (x, sw) in spans {
                fill(&mut pm, px(x), y - u, px(x + sw) - px(x), u, etch);
                fill(&mut pm, px(x), y, px(x + sw) - px(x), u, p.shine);
            }
        }
        if numbered {
            let n = (row.workspace + 1).to_string();
            let tw = text::measure(&n, size, true);
            let color = if r == shown.row { p.panel_text } else { p.tab_text_unfocused };
            let y = text::top_for_centre(px(top) as f32 + px(CELL) as f32 / 2.0, size, true);
            text::draw(&mut pm, &n, (px(PAD) as f32 + (px(GUTTER) as f32 - tw) / 2.0).round(), y, size, true, color);
        }
    }
    for (cell, at) in cell_places(shown, &lay) {
        draw_cell(&mut pm, p, shown, cell, at, icon_px, s);
    }

    // The chosen app's name (or window's title) beneath.
    if let Some(label) = label(shown) {
        let label = text::ellipsize(&label, size, true, (px(lw - 2 * PAD)) as f32);
        let tw = text::measure(&label, size, true);
        let y = text::top_for_centre(px(lh - PAD - LABEL) as f32 + px(LABEL) as f32 / 2.0, size, true);
        text::draw(&mut pm, &label, ((w as f32 - tw) / 2.0).round(), y, size, true, p.panel_text);
    }
    pm
}

/// Cell `(r, i)` of `shown` with its top left at `at` (logical): the
/// selection behind it, its icon and its letter.
fn draw_cell(pm: &mut Pixmap, p: &Palette, shown: &Shown, (r, i): (usize, usize), (cx, top): (i32, i32), icon_px: u32, s: f64) {
    let px = |l: i32| (l as f64 * s).round() as i32;
    let u = px(1).max(1);
    if r == shown.row && i == shown.index {
        // On a selection plate, as the kit sets a chosen picture
        // (`style::selection_plate`): a rounded ground in the list
        // selection's colour with a hairline edge.
        let inset = (CELL - ICON) / 2 - PLATE_PAD;
        let (x, y) = (px(cx + inset), px(top + inset));
        let side = px(cx + CELL - inset) - x;
        let radius = (PLATE_RADIUS * s) as f32;
        rounded(pm, (x, y, side, side), radius, p.selection_edge());
        rounded(pm, (x + u, y + u, side - 2 * u, side - 2 * u), radius - u as f32, p.list_selected_background);
    }
    if let Some(info) = info(&shown.rows[r].entries[i].app_id) {
        // The smallest decoded size at least as large as needed.
        if let Some((_, icon)) = info.icons.iter().find(|(s, _)| *s >= icon_px).or(info.icons.last()) {
            let k = icon_px as f32 / icon.width() as f32;
            let off = (CELL - ICON) / 2;
            let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() };
            let t = SkTransform::from_row(k, 0.0, 0.0, k, px(cx + off) as f32, px(top + off) as f32);
            pm.draw_pixmap(0, 0, icon.as_ref().as_ref(), &paint, t, None);
        }
    }
    if let Some(c) = shown.letter(r, i) {
        // Its hint, on the cell's lower right.
        let b = crate::hints::badge_pixmap(p, c, s);
        let bx = px(cx + CELL) - u - b.width() as i32;
        let by = px(top + CELL) - u - b.height() as i32;
        pm.draw_pixmap(bx, by, b.as_ref(), &PixmapPaint::default(), SkTransform::identity(), None);
    }
}

/// Fill `(x, y, w, h)` (device px) with corners of `radius`, antialiased.
fn rounded(pm: &mut Pixmap, (x, y, w, h): (i32, i32, i32, i32), radius: f32, rgba: [u8; 4]) {
    let (x, y, w, h) = (x as f32, y as f32, w as f32, h as f32);
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    // How far a cubic's handles reach to bend a quarter circle.
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    let Some(path) = pb.finish() else { return };
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    paint.anti_alias = true;
    pm.fill_path(&path, &paint, FillRule::Winding, SkTransform::identity(), None);
}

/// A display's box, as a BeOS BBox: an etched frame over `(x, width)`
/// from `top` to `bottom` (logical), its name across the top edge after a
/// little screen, lit when the panel is on that display (`here`).
fn display_box(pm: &mut Pixmap, p: &Palette, (x, bw): (i32, i32), (top, bottom): (i32, i32), name: &str, here: bool, s: f64) {
    let px = |l: i32| (l as f64 * s).round() as i32;
    let u = px(1).max(1);
    let (x0, x1, y0, y1) = (px(x), px(x + bw), px(top), px(bottom));
    // The name's size and where it breaks the top edge.
    let size = NAME_FONT * s as f32;
    let tw = text::measure(name, size, true).ceil() as i32;
    let (gap0, gap1) = (px(x + NAME_IN - 4), px(x + NAME_IN + SCREEN.0 + SCREEN_GAP) + tw + px(4));
    // A groove, as the kit's BBox: a dark line, then a shine one, all
    // round (so shine ends the bottom and right edges, lit from the top
    // left).
    let dark = p.shade(p.panel_background, 1.22);
    let edge = |pm: &mut Pixmap, x: i32, y: i32, ew: i32, eh: i32, color| fill(pm, x, y, ew.max(0), eh.max(0), color);
    edge(pm, x0, y0, gap0 - x0, u, dark);
    edge(pm, gap1, y0, x1 - u - gap1, u, dark);
    edge(pm, x0, y0, u, y1 - y0 - u, dark);
    edge(pm, x0, y1 - 2 * u, x1 - x0 - u, u, dark);
    edge(pm, x1 - 2 * u, y0, u, y1 - y0 - u, dark);
    edge(pm, x0 + u, y0 + u, gap0 - x0 - u, u, p.shine);
    edge(pm, gap1, y0 + u, x1 - 2 * u - gap1, u, p.shine);
    edge(pm, x0 + u, y0 + u, u, y1 - y0 - 3 * u, p.shine);
    edge(pm, x0, y1 - u, x1 - x0, u, p.shine);
    edge(pm, x1 - u, y0, u, y1 - y0, p.shine);
    // A little screen on a stand, its glass lit on this display.
    let ink = if here { p.panel_text } else { p.tab_text_unfocused };
    let (sx, sw) = (px(x + NAME_IN), px(SCREEN.0));
    let sh = px(SCREEN.1 - 2);
    let sy = y0 - px(SCREEN.1) / 2;
    fill(pm, sx, sy, sw, sh, ink);
    fill(pm, sx + u, sy + u, sw - 2 * u, sh - 2 * u, if here { p.control_highlight } else { p.panel_background });
    if here {
        // A glint on the glass.
        fill(pm, sx + 2 * u, sy + 2 * u, px(3), u, p.shine);
    }
    let neck = px(3);
    fill(pm, sx + (sw - neck) / 2, sy + sh, neck, u, ink);
    let foot = px(7);
    fill(pm, sx + (sw - foot) / 2, sy + sh + u, foot, u, ink);
    let ty = text::top_for_centre(y0 as f32, size, true);
    let color = if here { p.panel_text } else { p.tab_text_unfocused };
    text::draw(pm, name, (sx + sw + px(SCREEN_GAP)) as f32, ty, size, true, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_sizes() {
        assert_eq!(fallback_name("olo.tracker"), "Tracker");
        assert_eq!(fallback_name("foot"), "Foot");
        let e = |a: &str, display| Entry { app_id: a.into(), title: None, display };
        let row = |workspace, entries| Row { workspace, entries };
        let shown = |rows, displays| Shown { rows, displays, row: 0, index: 0, letters: false };
        assert_eq!(layout(&shown(vec![row(0, vec![e("a", 0); 2])], vec![])).size, (MIN_WIDTH, PAD + CELL + LABEL + PAD));
        assert_eq!(layout(&shown(vec![row(0, vec![e("a", 0); 10])], vec![])).size.0, 2 * PAD + 10 * CELL);
        // More than one workspace: numbered rows.
        let two = vec![row(0, vec![e("a", 0); 10]), row(1, vec![e("a", 0); 3])];
        assert_eq!(layout(&shown(two, vec![])).size, (2 * PAD + GUTTER + 10 * CELL, PAD + 2 * CELL + ROW_GAP + LABEL + PAD));
        // By display: a box each, as wide as its most cells in a row.
        let d = |n: &str| Display { output: n.into(), name: "D".into() };
        let rows = vec![row(0, vec![e("a", 0), e("b", 0), e("c", 0), e("d", 0), e("e", 1)]), row(1, vec![e("a", 1), e("b", 1), e("c", 1), e("f", 1)])];
        let lay = layout(&shown(rows, vec![d("A"), d("B")]));
        let boxed = 4 * CELL + 2 * BOX_PAD;
        assert_eq!(lay.columns, vec![(PAD + GUTTER, boxed), (PAD + GUTTER + boxed + BOX_GAP, boxed)]);
        assert_eq!(lay.size, (2 * PAD + GUTTER + 2 * boxed + BOX_GAP, PAD + CAPTION + 2 * CELL + ROW_GAP + BOX_PAD + LABEL + PAD));
    }

    #[test]
    fn cells_under_the_pointer() {
        let entry = |display| Entry { app_id: "olo.terminal".into(), title: None, display };
        let shown =
            Shown { rows: vec![Row { workspace: 0, entries: vec![entry(0), entry(0)] }, Row { workspace: 2, entries: vec![entry(0)] }], displays: Vec::new(), row: 0, index: 0, letters: false };
        let lay = layout(&shown);
        let out = (1600, 1000);
        let origin = ((out.0 - lay.size.0) as f64 / 2.0, (out.1 - lay.size.1) as f64 / 2.0);
        // The middle of each cell is that cell; between them and outside, none.
        for (cell, (x, y)) in cell_places(&shown, &lay) {
            let mid = (origin.0 + x as f64 + CELL as f64 / 2.0, origin.1 + y as f64 + CELL as f64 / 2.0);
            assert_eq!(cell_at(&shown, out, mid), Some(cell));
        }
        assert_eq!(cell_at(&shown, out, (origin.0 + 1.0, origin.1 + 1.0)), None);
        assert_eq!(cell_at(&shown, out, (5.0, 5.0)), None);
        assert_eq!(cell_places(&shown, &lay).len(), 3);
    }

    #[test]
    fn a_cut_label_shows_whole_under_the_panel() {
        let e = |title: &str| Entry { app_id: "olo.terminal".into(), title: Some(title.into()), display: 0 };
        let shown = |title| Shown { rows: vec![Row { workspace: 0, entries: vec![e(title), e("b")] }], displays: Vec::new(), row: 0, index: 0, letters: false };
        let long = "cargo build --release --workspace — ~/src/olo/crates/app_server";
        for s in [1.0, 1.25, 1.5, 1.75, 2.0] {
            assert_eq!(cut_label(&shown("Terminal"), s), None);
            assert_eq!(cut_label(&shown(long), s).as_deref(), Some(long));
        }
    }

    #[test]
    fn letters_run_across_rows() {
        let e = |a: &str| Entry { app_id: a.into(), title: None, display: 0 };
        let rows = vec![Row { workspace: 0, entries: vec![e("a"), e("b")] }, Row { workspace: 2, entries: vec![e("c")] }];
        let mut shown = Shown { rows, displays: Vec::new(), row: 0, index: 0, letters: true };
        assert_eq!(shown.letter(1, 0), Some('d'));
        assert_eq!(shown.find('d'), Some((1, 0)));
        assert_eq!(shown.find('f'), None);
        shown.letters = false;
        assert_eq!((shown.letter(0, 0), shown.find('a')), (None, None));
    }
}
