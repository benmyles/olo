//! Colour emoji for apps that draw text cell by cell or glyph by glyph
//! (Terminal). egui's own text is monochrome, so these are painted as
//! textures: [`paint`] draws a cluster fitted into a rectangle. The colour
//! font is found with the system fonts and read on a worker thread; until
//! it is ready, and for clusters it has no colour glyph for, `paint`
//! returns false and the caller draws text as usual.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use egui::{Color32, Context, Painter, Rect, TextureHandle, TextureOptions, pos2, vec2};
use tabula_emoji::ColorFont;

static FONT: OnceLock<Option<ColorFont>> = OnceLock::new();

/// Called with the system font database: start reading the colour font.
pub(crate) fn locate(db: &fontdb::Database) {
    let query = fontdb::Query { families: &[fontdb::Family::Name("Noto Color Emoji")], ..Default::default() };
    let face = db.query(&query).and_then(|id| db.face(id)).and_then(|f| match &f.source {
        fontdb::Source::File(p) => Some((p.clone(), f.index)),
        fontdb::Source::SharedFile(p, _) => Some((p.clone(), f.index)),
        fontdb::Source::Binary(_) => None,
    });
    let Some((path, index)) = face else {
        let _ = FONT.set(None);
        return;
    };
    let _ = std::thread::Builder::new().name("emoji-font".into()).spawn(move || {
        FONT.get_or_init(|| load(path, index));
    });
}

fn load(path: PathBuf, index: u32) -> Option<ColorFont> {
    let font = ColorFont::from_static(crate::style::mapped(&path)?, index);
    if font.is_none() {
        tracing::warn!(?path, "not a colour font");
    }
    font
}

/// Whether `cluster` should be drawn in colour: emoji presentation by
/// default (they are wide in a terminal) or asked for with U+FE0F.
pub fn wants_color(cluster: &str, wide: bool) -> bool {
    let mut chars = cluster.chars();
    let Some(first) = chars.next() else { return false };
    (wide && !first.is_ascii()) || cluster.contains('\u{FE0F}') || is_regional(first)
}

/// Whether `next` (a cell's text) goes on with the sequence in `prev`:
/// after a zero-width joiner, a skin tone, or a flag's second letter.
pub fn continues(prev: &str, next: &str) -> bool {
    let Some(first) = next.chars().next() else { return false };
    let skin = ('\u{1F3FB}'..='\u{1F3FF}').contains(&first);
    let flag = is_regional(first) && prev.chars().count() == 1 && prev.chars().all(is_regional);
    prev.ends_with('\u{200D}') || skin || flag
}

fn is_regional(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

#[derive(Default)]
struct Cache {
    glyphs: HashMap<String, Option<u32>>,
    textures: HashMap<(u32, u32), TextureHandle>,
}

type Shared = Arc<Mutex<Cache>>;

/// Paint `cluster` in colour, as large as fits `rect` and centred in it.
/// False if there is no colour glyph for it (draw it as text instead).
pub fn paint(painter: &Painter, rect: Rect, cluster: &str) -> bool {
    let Some(Some(font)) = FONT.get() else { return false };
    let ctx = painter.ctx();
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Shared>(egui::Id::NULL).clone());
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let glyph = match cache.glyphs.get(cluster) {
        Some(g) => *g,
        None => {
            let g = font.glyph(cluster);
            cache.glyphs.insert(cluster.to_owned(), g);
            g
        }
    };
    let Some(glyph) = glyph else { return false };
    let ppp = ctx.pixels_per_point();
    let aspect = font.aspect(glyph);
    let h = rect.height().min(rect.width() / aspect);
    let px = (h * ppp).round().max(1.0) as u32;
    let key = (glyph, px);
    if !cache.textures.contains_key(&key) {
        let Some(tex) = texture(ctx, font, glyph, px) else { return false };
        // A bound: sizes change with text size and scale.
        if cache.textures.len() >= 2048 {
            cache.textures.clear();
        }
        cache.textures.insert(key, tex);
    }
    let tex = &cache.textures[&key];
    let size = vec2(tex.size()[0] as f32, tex.size()[1] as f32) / ppp;
    // Whole device pixels, so the bitmap isn't resampled.
    let snap = |v: f32| (v * ppp).round() / ppp;
    let min = pos2(snap(rect.center().x - size.x / 2.0), snap(rect.center().y - size.y / 2.0));
    painter.image(tex.id(), Rect::from_min_size(min, size), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    true
}

fn texture(ctx: &Context, font: &ColorFont, glyph: u32, px: u32) -> Option<TextureHandle> {
    let img = font.render(glyph, px, [0, 0, 0, 255])?;
    let image = egui::ColorImage::from_rgba_premultiplied([img.width as usize, img.height as usize], &img.rgba);
    Some(ctx.load_texture(format!("emoji-{glyph}-{px}"), image, TextureOptions::LINEAR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequences_join() {
        assert!(continues("👍", "🏽"));
        assert!(continues("👩\u{200D}", "💻"));
        assert!(continues("🇳", "🇴"));
        assert!(!continues("🇳🇴", "🇳"));
        assert!(!continues("😀", "😀"));
        assert!(wants_color("😀", true));
        assert!(wants_color("❤\u{FE0F}", false));
        assert!(!wants_color("❤", false));
    }
}
