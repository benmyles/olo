//! Twitcher: the strip of app icons shown while ⌘Tab / ⌘\` is held, as on
//! BeOS. `desktop::twitch_step` decides what is chosen; this draws it.
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
use tabula_theme::Palette;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform as SkTransform};

use crate::{shell::fill, text, theme};

/// Logical sizes.
const ICON: i32 = 48;
const CELL: i32 = 64;
const PAD: i32 = 12;
const LABEL: i32 = 22;
const MIN_WIDTH: i32 = 240;
const FONT: f32 = 12.0;

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

fn to_pixmap(img: tabula_xdg::Image) -> Option<Pixmap> {
    let img = img.into_premultiplied();
    let size = tiny_skia::IntSize::from_wh(img.width, img.height)?;
    Pixmap::from_vec(img.rgba, size)
}

fn resolve(app_id: &str) -> AppInfo {
    let (name, icon) = match tabula_xdg::apps::our_app(app_id) {
        Some((name, icon)) => (name.to_owned(), icon.to_owned()),
        None => {
            let entry = tabula_xdg::desktop_entry::find(app_id);
            let name = entry.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| fallback_name(app_id));
            (name, entry.and_then(|e| e.icon).unwrap_or_else(|| "application".into()))
        }
    };
    let icons = ICON_PX
        .iter()
        .filter_map(|&px| {
            let img = tabula_xdg::load_icon(&icon, px).or_else(|| tabula_xdg::load_icon("application", px))?;
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

#[derive(Debug, Clone, PartialEq)]
struct Key {
    entries: Vec<Entry>,
    /// Which of them have resolved (so a late lookup redraws).
    resolved: Vec<bool>,
    selected: usize,
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
        entries: &[Entry],
        selected: usize,
        output_size: Size<i32, smithay::utils::Logical>,
        scale: f64,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if entries.len() < 2 {
            return None;
        }
        let key = Key {
            entries: entries.to_vec(),
            resolved: entries.iter().map(|e| info(&e.app_id).is_some()).collect(),
            selected,
            scale_milli: (scale * 1000.0).round() as i64,
            generation: theme::generation(),
        };
        if self.cached.as_ref().is_none_or(|(k, _, _)| *k != key) {
            let pixmap = draw(&theme::palette(), entries, selected, scale);
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

/// Panel size in logical pixels for `n` cells.
pub fn panel_size(n: usize) -> (i32, i32) {
    ((2 * PAD + n as i32 * CELL).max(MIN_WIDTH), PAD + CELL + LABEL + PAD)
}

fn draw(p: &Palette, entries: &[Entry], selected: usize, s: f64) -> Pixmap {
    let (lw, lh) = panel_size(entries.len());
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

    let row_w = entries.len() as i32 * CELL;
    let x0 = (lw - row_w) / 2;
    let icon_px = px(ICON) as u32;
    for (i, e) in entries.iter().enumerate() {
        let cx = x0 + i as i32 * CELL;
        if i == selected {
            let (x, y, cw) = (px(cx), px(PAD), px(cx + CELL) - px(cx));
            fill(&mut pm, x, y, cw, px(PAD + CELL) - y, p.menu_selected_border);
            fill(&mut pm, x + u, y + u, cw - 2 * u, px(PAD + CELL) - y - 2 * u, p.menu_selected_background);
        }
        let Some(info) = info(&e.app_id) else { continue };
        // The smallest decoded size at least as large as needed.
        let Some((_, icon)) = info
            .icons
            .iter()
            .find(|(s, _)| *s >= icon_px)
            .or(info.icons.last())
        else {
            continue;
        };
        let k = icon_px as f32 / icon.width() as f32;
        let off = (CELL - ICON) / 2;
        let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() };
        let t = SkTransform::from_row(k, 0.0, 0.0, k, px(cx + off) as f32, px(PAD + off) as f32);
        pm.draw_pixmap(0, 0, icon.as_ref().as_ref(), &paint, t, None);
    }

    // The chosen app's name (or window's title) beneath.
    if let Some(e) = entries.get(selected) {
        let label = e
            .title
            .clone()
            .or_else(|| info(&e.app_id).map(|i| i.name))
            .unwrap_or_else(|| fallback_name(&e.app_id));
        let size = FONT * s as f32;
        let label = text::ellipsize(&label, size, true, (px(lw - 2 * PAD)) as f32);
        let tw = text::measure(&label, size, true);
        let y = px(PAD + CELL) as f32 + (px(LABEL) as f32 - size * 1.25) / 2.0;
        text::draw(&mut pm, &label, ((w as f32 - tw) / 2.0).round(), y.round(), size, true, p.panel_text);
    }
    pm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_sizes() {
        assert_eq!(fallback_name("tabula.tracker"), "Tracker");
        assert_eq!(fallback_name("foot"), "Foot");
        assert_eq!(panel_size(2), (MIN_WIDTH, PAD + CELL + LABEL + PAD));
        assert_eq!(panel_size(10).0, 2 * PAD + 10 * CELL);
    }
}
