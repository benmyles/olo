//! Colour emoji for apps that draw text cell by cell or glyph by glyph
//! (Terminal). egui's own text is monochrome, so these are painted as
//! textures: [`paint`] draws a cluster fitted into a rectangle. The colour
//! font is found with the system fonts and read on a worker thread; until
//! it is ready, and for clusters it has no colour glyph for, `paint`
//! returns false and the caller draws text as usual.
//!
//! In laid-out text (a chat message, a card's body), [`make_room`] turns
//! each emoji of a `LayoutJob` into a placeholder as wide as an emoji
//! (epaint's `LayoutJob::advances`) and [`paint_in`] paints them in
//! colour where the galley put them; [`clusters`] finds them.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use egui::{Color32, Context, Painter, Rect, TextureHandle, TextureOptions, pos2, vec2};
use olo_emoji::ColorFont;

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

/// The colour font is loaded (until then [`make_room`] leaves text as
/// it is). Part of the key of anything laid out with it.
pub fn ready() -> bool {
    matches!(FONT.get(), Some(Some(_)))
}

/// Where a cluster starting with `c` (followed by `next`) is an emoji
/// to show in colour.
fn starts_emoji(c: char, next: Option<char>) -> bool {
    let u = c as u32;
    // Below U+1F000, those shown as emoji by default (Emoji_Presentation).
    const BMP: &[(u32, u32)] = &[
        (0x231A, 0x231B), (0x23E9, 0x23EC), (0x23F0, 0x23F0), (0x23F3, 0x23F3), (0x25FD, 0x25FE), (0x2614, 0x2615), (0x2648, 0x2653),
        (0x267F, 0x267F), (0x2693, 0x2693), (0x26A1, 0x26A1), (0x26AA, 0x26AB), (0x26BD, 0x26BE), (0x26C4, 0x26C5), (0x26CE, 0x26CE),
        (0x26D4, 0x26D4), (0x26EA, 0x26EA), (0x26F2, 0x26F3), (0x26F5, 0x26F5), (0x26FA, 0x26FA), (0x26FD, 0x26FD), (0x2705, 0x2705),
        (0x270A, 0x270B), (0x2728, 0x2728), (0x274C, 0x274C), (0x274E, 0x274E), (0x2753, 0x2755), (0x2757, 0x2757), (0x2795, 0x2797),
        (0x27B0, 0x27B0), (0x27BF, 0x27BF), (0x2B1B, 0x2B1C), (0x2B50, 0x2B50), (0x2B55, 0x2B55),
    ];
    (0x1F000..=0x1FAFF).contains(&u) || BMP.iter().any(|&(a, b)| (a..=b).contains(&u)) || is_regional(c) || next == Some('\u{FE0F}') || next == Some('\u{20E3}') && !c.is_alphabetic()
}

/// The emoji in `text`, as byte ranges: each a whole sequence (joined
/// with ZWJ, a skin tone, a flag's two letters, a keycap).
pub fn clusters(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let next = it.peek().map(|&(_, n)| n);
        if !starts_emoji(c, next) {
            continue;
        }
        let mut end = i + c.len_utf8();
        let mut flag = is_regional(c);
        while let Some(&(j, n)) = it.peek() {
            let joins = n == '\u{FE0F}' || n == '\u{20E3}' || ('\u{1F3FB}'..='\u{1F3FF}').contains(&n) || ('\u{E0020}'..='\u{E007F}').contains(&n);
            if joins || flag && is_regional(n) {
                flag = false;
                end = j + n.len_utf8();
                it.next();
            } else if n == '\u{200D}' {
                it.next();
                end = j + n.len_utf8();
                if let Some((k, m)) = it.next() {
                    end = k + m.len_utf8();
                }
            } else {
                break;
            }
        }
        out.push(i..end);
    }
    out
}

/// How wide an emoji is beside text of `size` points.
pub const ADVANCE: f32 = 1.25;

/// `job` with room made for its emoji, and the emoji in order (for
/// [`paint_in`]): each becomes U+FFFC, unseen, as wide as an emoji at its
/// section's size. With no colour font yet, `job` as it is.
pub fn make_room(job: &egui::text::LayoutJob) -> (egui::text::LayoutJob, Vec<String>) {
    if !ready() || !job.text.chars().any(|c| c as u32 >= 0x2190) {
        return (job.clone(), Vec::new());
    }
    let mut out = egui::text::LayoutJob { text: String::new(), sections: Vec::new(), advances: Vec::new(), ..job.clone() };
    let mut found = Vec::new();
    for s in &job.sections {
        let part = &job.text[s.byte_range.start.0..s.byte_range.end.0];
        let mut from = 0;
        let push = |out: &mut egui::text::LayoutJob, text: &str, format: egui::TextFormat, leading: f32| {
            if text.is_empty() {
                return;
            }
            let start = out.text.len();
            out.text.push_str(text);
            out.sections.push(egui::text::LayoutSection { leading_space: leading, byte_range: egui::text::ByteIndex(start)..egui::text::ByteIndex(out.text.len()), format });
        };
        let mut first = true;
        for r in clusters(part) {
            push(&mut out, &part[from..r.start], s.format.clone(), if first { s.leading_space } else { 0.0 });
            first = false;
            let at = out.text.len();
            let mut f = s.format.clone();
            f.color = Color32::TRANSPARENT;
            f.underline = egui::Stroke::NONE;
            f.strikethrough = egui::Stroke::NONE;
            push(&mut out, "\u{FFFC}", f, 0.0);
            out.advances.push((at, (s.format.font_id.size * ADVANCE).round()));
            found.push(part[r.clone()].to_owned());
            from = r.end;
        }
        push(&mut out, &part[from..], s.format.clone(), if first { s.leading_space } else { 0.0 });
    }
    (out, found)
}

/// Paint the emoji [`make_room`] made room for, in `galley` drawn at
/// `origin`, each centred on its line's text. Ones the font has no
/// colour glyph for are drawn as plain text in `fallback`.
pub fn paint_in(painter: &Painter, origin: egui::Pos2, galley: &egui::Galley, emoji: &[String], fallback: Color32) {
    if emoji.is_empty() {
        return;
    }
    let mut k = 0;
    for row in &galley.rows {
        for g in &row.glyphs {
            if g.chr != '\u{FFFC}' {
                continue;
            }
            let Some(cluster) = emoji.get(k) else { return };
            k += 1;
            let size = g.font_height.max(1.0);
            let cy = origin.y + row.pos.y + g.pos.y - g.font_ascent + g.font_height / 2.0;
            let x = origin.x + row.pos.x + g.pos.x;
            let rect = Rect::from_center_size(pos2(x + g.advance_width / 2.0, cy), vec2(g.advance_width, size * 1.05));
            if !paint(painter, rect, cluster) {
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, cluster, egui::FontId::proportional(size * 0.8), fallback);
            }
        }
    }
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
            // A character shown as emoji by default may be in the font
            // only as its emoji form (with U+FE0F).
            let g = font.glyph(cluster).or_else(|| (!cluster.contains('\u{FE0F}') && cluster.chars().count() == 1).then(|| font.glyph(&format!("{cluster}\u{FE0F}"))).flatten());
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
        let t = "hi 👍🏽 and 👩\u{200D}💻, 🇳🇴! ❤\u{FE0F} 1\u{FE0F}\u{20E3}";
        let found: Vec<&str> = clusters(t).into_iter().map(|r| &t[r]).collect();
        assert_eq!(found, ["👍🏽", "👩\u{200D}💻", "🇳🇴", "❤\u{FE0F}", "1\u{FE0F}\u{20E3}"]);
        assert!(clusters("plain text — “quotes” → arrows").is_empty());
        assert_eq!(clusters("☕ ok").len(), 1);
    }
}
