//! Twitcher: the strip of app icons shown while ⌘Tab / ⌘\` is held, as on
//! BeOS, a row for each workspace with windows on it (numbered at the
//! left when there are more than one). `desktop::twitch_step` decides what
//! is chosen; this draws it.
//!
//! App names and icons come from .desktop entries and icon themes, which
//! means file system work, so they are looked up on a worker thread the
//! first time an app id is seen (`prewarm`, called whenever the window
//! list changes) and drawn from the cache. Until an app is resolved it
//! shows under its app id with a generic icon.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

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
use olo_theme::Palette;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform as SkTransform};

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

/// Device sizes icons are decoded at; others are scaled from the next one up.
const ICON_PX: [u32; 2] = [48, 96];

#[derive(Clone)]
struct AppInfo {
    name: String,
    icons: Vec<(u32, Arc<Pixmap>)>,
}

type Cache = Mutex<HashMap<String, Option<AppInfo>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn to_pixmap(img: olo_xdg::Image) -> Option<Pixmap> {
    let img = img.into_premultiplied();
    let size = tiny_skia::IntSize::from_wh(img.width, img.height)?;
    Pixmap::from_vec(img.rgba, size)
}

fn resolve(app_id: &str) -> AppInfo {
    let (name, icon) = match olo_xdg::apps::our_app(app_id) {
        Some((name, icon)) => (name.to_owned(), icon.to_owned()),
        None => {
            let entry = olo_xdg::desktop_entry::find(app_id);
            let name = entry.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| fallback_name(app_id));
            (name, entry.and_then(|e| e.icon).unwrap_or_else(|| "application".into()))
        }
    };
    let icons = ICON_PX
        .iter()
        .filter_map(|&px| {
            let img = olo_xdg::load_icon(&icon, px).or_else(|| olo_xdg::load_icon("application", px))?;
            Some((px, Arc::new(to_pixmap(img)?)))
        })
        .collect();
    AppInfo { name, icons }
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
            cache().lock().unwrap().insert(id, Some(info));
        }
    });
}

fn info(app_id: &str) -> Option<AppInfo> {
    cache().lock().unwrap().get(app_id).cloned().flatten()
}

/// One cell: an app (⌘Tab) or a window (⌘\`).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub app_id: String,
    /// The window title for ⌘\`; the app's name is used for ⌘Tab.
    pub title: Option<String>,
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
    pub rows: Vec<Row>,
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
    /// Which apps have resolved (so a late lookup redraws).
    resolved: Vec<bool>,
    scale_milli: i64,
    generation: u64,
}

#[derive(Default)]
pub struct Overlay {
    cached: Option<(Key, MemoryRenderBuffer, Size<i32, Physical>)>,
}

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay").field("cached", &self.cached.as_ref().map(|(k, _, s)| (k, s))).finish()
    }
}

impl Overlay {
    /// The Twitcher panel centred in an output of `output_size` (logical)
    /// at `scale`, or None if there is nothing to show.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        shown: &Shown,
        output_size: Size<i32, smithay::utils::Logical>,
        scale: f64,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if shown.cells() < 2 {
            return None;
        }
        let key = Key {
            shown: shown.clone(),
            resolved: shown.rows.iter().flat_map(|r| &r.entries).map(|e| info(&e.app_id).is_some()).collect(),
            scale_milli: (scale * 1000.0).round() as i64,
            generation: theme::generation(),
        };
        if self.cached.as_ref().is_none_or(|(k, _, _)| *k != key) {
            let pixmap = draw(&theme::palette(), shown, scale);
            let size: Size<i32, Buffer> = (pixmap.width() as i32, pixmap.height() as i32).into();
            let buffer = MemoryRenderBuffer::from_slice(
                pixmap.data(),
                Fourcc::Abgr8888,
                size,
                1,
                Transform::Normal,
                Some(vec![Rectangle::from_size(size)]),
            );
            self.cached = Some((key, buffer, (size.w, size.h).into()));
        }
        let (_, buffer, phys) = self.cached.as_ref()?;
        let out = output_size.to_f64().to_physical(scale);
        let loc: Point<f64, Physical> =
            (((out.w - phys.w as f64) / 2.0).round(), ((out.h - phys.h as f64) / 2.0).round()).into();
        let logical = (phys.w as f64 / scale).round() as i32;
        let logical_h = (phys.h as f64 / scale).round() as i32;
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            loc,
            buffer,
            None,
            Some(Rectangle::from_size((phys.w as f64, phys.h as f64).into())),
            Some((logical, logical_h).into()),
            Kind::Unspecified,
        )
        .ok()
    }
}

/// Panel size in logical pixels for rows of `lens` cells.
pub fn panel_size(lens: &[usize]) -> (i32, i32) {
    let gutter = if lens.len() > 1 { GUTTER } else { 0 };
    let widest = lens.iter().copied().max().unwrap_or(0) as i32;
    let rows = lens.len().max(1) as i32;
    ((2 * PAD + gutter + widest * CELL).max(MIN_WIDTH), PAD + rows * CELL + (rows - 1) * ROW_GAP + LABEL + PAD)
}

fn draw(p: &Palette, shown: &Shown, s: f64) -> Pixmap {
    let lens: Vec<usize> = shown.rows.iter().map(|r| r.entries.len()).collect();
    let (lw, lh) = panel_size(&lens);
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

    let numbered = shown.rows.len() > 1;
    let gutter = if numbered { GUTTER } else { 0 };
    let icon_px = px(ICON) as u32;
    let size = FONT * s as f32;
    for (r, row) in shown.rows.iter().enumerate() {
        let top = PAD + r as i32 * (CELL + ROW_GAP);
        if r > 0 {
            // An etched line between workspaces.
            let y = px(top - ROW_GAP / 2);
            fill(&mut pm, px(PAD), y - u, w - 2 * px(PAD), u, p.shadow);
            fill(&mut pm, px(PAD), y, w - 2 * px(PAD), u, p.shine);
        }
        if numbered {
            let n = (row.workspace + 1).to_string();
            let tw = text::measure(&n, size, true);
            let color = if r == shown.row { p.panel_text } else { p.tab_text_unfocused };
            let y = px(top) as f32 + (px(CELL) as f32 - size * 1.25) / 2.0;
            text::draw(&mut pm, &n, (px(PAD) as f32 + (px(GUTTER) as f32 - tw) / 2.0).round(), y.round(), size, true, color);
        }
        let x0 = PAD + gutter + (lw - 2 * PAD - gutter - row.entries.len() as i32 * CELL) / 2;
        for (i, e) in row.entries.iter().enumerate() {
            let cx = x0 + i as i32 * CELL;
            if r == shown.row && i == shown.index {
                let (x, y, cw) = (px(cx), px(top), px(cx + CELL) - px(cx));
                let ch = px(top + CELL) - y;
                fill(&mut pm, x, y, cw, ch, p.menu_selected_border);
                fill(&mut pm, x + u, y + u, cw - 2 * u, ch - 2 * u, p.menu_selected_background);
            }
            if let Some(info) = info(&e.app_id) {
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
    }

    // The chosen app's name (or window's title) beneath.
    if let Some(e) = shown.selected() {
        let label = e
            .title
            .clone()
            .or_else(|| info(&e.app_id).map(|i| i.name))
            .unwrap_or_else(|| fallback_name(&e.app_id));
        let label = text::ellipsize(&label, size, true, (px(lw - 2 * PAD)) as f32);
        let tw = text::measure(&label, size, true);
        let y = px(lh - PAD - LABEL) as f32 + (px(LABEL) as f32 - size * 1.25) / 2.0;
        text::draw(&mut pm, &label, ((w as f32 - tw) / 2.0).round(), y.round(), size, true, p.panel_text);
    }
    pm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_sizes() {
        assert_eq!(fallback_name("olo.tracker"), "Tracker");
        assert_eq!(fallback_name("foot"), "Foot");
        assert_eq!(panel_size(&[2]), (MIN_WIDTH, PAD + CELL + LABEL + PAD));
        assert_eq!(panel_size(&[10]).0, 2 * PAD + 10 * CELL);
        // More than one workspace: numbered rows.
        assert_eq!(panel_size(&[10, 3]), (2 * PAD + GUTTER + 10 * CELL, PAD + 2 * CELL + ROW_GAP + LABEL + PAD));
    }

    #[test]
    fn letters_run_across_rows() {
        let e = |a: &str| Entry { app_id: a.into(), title: None };
        let mut shown = Shown { rows: vec![Row { workspace: 0, entries: vec![e("a"), e("b")] }, Row { workspace: 2, entries: vec![e("c")] }], row: 0, index: 0, letters: true };
        assert_eq!(shown.letter(1, 0), Some('d'));
        assert_eq!(shown.find('d'), Some((1, 0)));
        assert_eq!(shown.find('f'), None);
        shown.letters = false;
        assert_eq!((shown.letter(0, 0), shown.find('a')), (None, None));
    }
}
