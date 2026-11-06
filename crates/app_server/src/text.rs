//! Text for app_server's own drawing (window tab titles), via cosmic-text
//! onto tiny-skia pixmaps.
//!
//! Each thread that draws or measures text has a font system of its own
//! (made the first time from one database of the fonts, found once), so
//! none ever waits on another's: the main loop measuring a new tab's
//! title once waited on the tab worker drawing five others (a held lock,
//! the loop stalled up to 13 ms as windows opened).

use std::{cell::RefCell, sync::OnceLock};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use tiny_skia::Pixmap;

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
    ui_family: String,
}

/// The fonts, found once (scanning the system's takes a while: `warm_up`
/// does it off the loop): their database and locale, and the family tabs
/// are in.
struct Base {
    locale: String,
    db: cosmic_text::fontdb::Database,
    ui_family: String,
}

fn base() -> &'static Base {
    static BASE: OnceLock<Base> = OnceLock::new();
    BASE.get_or_init(|| {
        let (locale, mut db) = font_db();
        let ui_family = olo_theme::fonts::UI.iter().find(|name| db.faces().any(|f| f.families.iter().any(|(fam, _)| fam == *name))).map(|s| s.to_string()).unwrap_or_else(|| "sans-serif".into());
        // The tab family's faces read once and shared by every thread's
        // font system (mapped, not copied each time).
        let ids: Vec<_> = db.faces().filter(|f| f.families.iter().any(|(fam, _)| *fam == ui_family)).map(|f| f.id).collect();
        for id in ids {
            // SAFETY: font files aren't changed under a running
            // app_server (Olo's own are its install's).
            unsafe { db.make_shared_face_data(id) };
        }
        tracing::info!(font = ui_family, "tab font");
        Base { locale, db, ui_family }
    })
}

thread_local! {
    static FONTS: RefCell<Option<Fonts>> = const { RefCell::new(None) };
}

/// This thread's fonts (made from `base` the first time).
fn with_fonts<R>(f: impl FnOnce(&mut Fonts) -> R) -> R {
    FONTS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let fonts = slot.get_or_insert_with(|| {
            let b = base();
            Fonts { system: FontSystem::new_with_locale_and_db(b.locale.clone(), b.db.clone()), cache: SwashCache::new(), ui_family: b.ui_family.clone() }
        });
        f(fonts)
    })
}

/// The system's fonts, with Olo's own (data/fonts) in place of the
/// system's copies of the same families: tabs in the faces the kit draws
/// in, whatever the system has.
fn font_db() -> (String, cosmic_text::fontdb::Database) {
    let (locale, mut db) = FontSystem::new().into_locale_and_db();
    if let Some(dir) = olo_xdg::fonts_dir() {
        let mut own = cosmic_text::fontdb::Database::new();
        own.load_fonts_dir(&dir);
        let ours: std::collections::HashSet<&str> = own.faces().flat_map(|f| f.families.iter().map(|(name, _)| name.as_str())).collect();
        let theirs: Vec<_> = db.faces().filter(|f| f.families.iter().any(|(name, _)| ours.contains(name.as_str()))).map(|f| f.id).collect();
        for id in theirs {
            db.remove_face(id);
        }
        db.load_fonts_dir(&dir);
    }
    (locale, db)
}

/// Find the fonts on a background thread so the first window doesn't
/// stall.
pub fn warm_up() {
    std::thread::spawn(|| {
        base();
    });
}

fn shaped(fonts: &mut Fonts, text: &str, size: f32, bold: bool) -> Buffer {
    let Fonts { system, ui_family, .. } = fonts;
    let mut buffer = Buffer::new(system, Metrics::new(size, size * 1.25));
    let mut b = buffer.borrow_with(system);
    b.set_size(None, None);
    let family = ui_family.clone();
    let attrs = Attrs::new().family(Family::Name(&family)).weight(if bold { Weight::BOLD } else { Weight::NORMAL });
    b.set_text(text, &attrs, Shaping::Advanced, None);
    b.shape_until_scroll(true);
    buffer
}

/// Width in pixels of `text` at `size` pixels.
pub fn measure(text: &str, size: f32, bold: bool) -> f32 {
    with_fonts(|fonts| {
        let buffer = shaped(fonts, text, size, bold);
        buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    })
}

/// Where to `draw` text at `size` pixels so its capitals are centred on
/// `centre` (pixels down the pixmap), its baseline on a whole pixel: from
/// the face's own ascent and cap height, so whichever font the UI is in
/// sits in the middle of a tab or badge.
pub fn top_for_centre(centre: f32, size: f32, bold: bool) -> f32 {
    with_fonts(|fonts| {
        let buffer = shaped(fonts, "H", size, bold);
        let Some(run) = buffer.layout_runs().next() else {
            return (centre - size * 0.625).round();
        };
        let cap = run
            .glyphs
            .first()
            .and_then(|g| fonts.system.get_font(g.font_id, g.font_weight))
            .map(|f| {
                let m = f.as_swash().metrics(&[]);
                m.cap_height / f32::from(m.units_per_em)
            })
            .unwrap_or(0.7);
        // `draw` puts the baseline at the top plus `line_y`, cut to a pixel.
        (centre + cap * size / 2.0).round() - run.line_y.trunc()
    })
}

/// Truncate with an ellipsis so the text fits in `max_width` pixels.
pub fn ellipsize(text: &str, size: f32, bold: bool, max_width: f32) -> String {
    if measure(text, size, bold) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if measure(&candidate, size, bold) <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

/// Draw `text` with its top-left at (x, y); vertically, `y` is the top of
/// the line box. Colour is straight-alpha RGBA.
pub fn draw(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, bold: bool, rgba: [u8; 4]) {
    with_fonts(|fonts| draw_with(fonts, pixmap, text, x, y, size, bold, rgba));
}

#[allow(clippy::too_many_arguments)]
fn draw_with(fonts: &mut Fonts, pixmap: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, bold: bool, rgba: [u8; 4]) {
    let mut buffer = shaped(fonts, text, size, bold);
    let Fonts { system, cache, .. } = fonts;
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
    let data = pixmap.data_mut();
    let color = Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]);
    buffer.draw(system, cache, color, |gx, gy, w, h, c| {
        let a = c.a() as u32;
        if a == 0 {
            return;
        }
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                let px = x as i32 + gx + dx;
                let py = y as i32 + gy + dy;
                if px < 0 || py < 0 || px >= pw || py >= ph {
                    continue;
                }
                // tiny-skia stores premultiplied RGBA.
                let i = ((py * pw + px) * 4) as usize;
                let inv = 255 - a;
                let blend = |src: u8, dst: u8| ((src as u32 * a + dst as u32 * inv) / 255) as u8;
                data[i] = blend(c.r(), data[i]);
                data[i + 1] = blend(c.g(), data[i + 1]);
                data[i + 2] = blend(c.b(), data[i + 2]);
                data[i + 3] = (a + data[i + 3] as u32 * inv / 255).min(255) as u8;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capital drawn at `top_for_centre` has its middle there, give or
    /// take the half pixel the baseline is rounded to, at every scale.
    #[test]
    fn capitals_centre_where_asked() {
        for s in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            let size = olo_theme::fonts::TAB_SIZE * s;
            let (w, h) = (40u32, (21.0 * s).round() as u32);
            let centre = h as f32 / 2.0;
            let mut pm = Pixmap::new(w, h).unwrap();
            draw(&mut pm, "H", 2.0, top_for_centre(centre, size, true), size, true, [0, 0, 0, 255]);
            // The stem: the column with the most ink, weighed by coverage.
            let alpha = |x: u32, y: u32| f32::from(pm.data()[((y * w + x) * 4 + 3) as usize]) / 255.0;
            let stem = (0..w).max_by(|&a, &b| {
                let sum = |x| (0..h).map(|y| alpha(x, y)).sum::<f32>();
                sum(a).total_cmp(&sum(b))
            });
            let x = stem.unwrap();
            let ink: f32 = (0..h).map(|y| alpha(x, y)).sum();
            let middle = (0..h).map(|y| (y as f32 + 0.5) * alpha(x, y)).sum::<f32>() / ink;
            assert!((middle - centre).abs() <= 0.5, "at {s}×, the H's middle is {middle}, asked {centre}");
        }
    }
}
