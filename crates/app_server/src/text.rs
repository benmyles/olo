//! Text for app_server's own drawing (window tab titles), via cosmic-text
//! onto tiny-skia pixmaps.

use std::sync::{Mutex, OnceLock};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use tiny_skia::Pixmap;

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
    ui_family: String,
}

fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let system = FontSystem::new();
        let ui_family = olo_theme::fonts::UI
            .iter()
            .find(|name| {
                system
                    .db()
                    .faces()
                    .any(|f| f.families.iter().any(|(fam, _)| fam == *name))
            })
            .map(|s| s.to_string())
            .unwrap_or_else(|| "sans-serif".into());
        tracing::info!(font = ui_family, "tab font");
        Mutex::new(Fonts { system, cache: SwashCache::new(), ui_family })
    })
}

/// Load fonts on a background thread so the first window doesn't stall.
pub fn warm_up() {
    std::thread::spawn(|| {
        drop(fonts().lock());
    });
}

fn shaped(fonts: &mut Fonts, text: &str, size: f32, bold: bool) -> Buffer {
    let Fonts { system, ui_family, .. } = fonts;
    let mut buffer = Buffer::new(system, Metrics::new(size, size * 1.25));
    let mut b = buffer.borrow_with(system);
    b.set_size(None, None);
    let family = ui_family.clone();
    let attrs = Attrs::new()
        .family(Family::Name(&family))
        .weight(if bold { Weight::BOLD } else { Weight::NORMAL });
    b.set_text(text, &attrs, Shaping::Advanced, None);
    b.shape_until_scroll(true);
    buffer
}

/// Width in pixels of `text` at `size` pixels.
pub fn measure(text: &str, size: f32, bold: bool) -> f32 {
    let mut fonts = fonts().lock().unwrap();
    let buffer = shaped(&mut fonts, text, size, bold);
    buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
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
    let mut fonts = fonts().lock().unwrap();
    let mut buffer = shaped(&mut fonts, text, size, bold);
    let Fonts { system, cache, .. } = &mut *fonts;
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
