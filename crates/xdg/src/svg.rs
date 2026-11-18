//! SVG icons, rendered for the size they're shown at (resvg, in process).
//!
//! Olo's own icons are drawings (`<data>/icons/svg/NAME.svg`, a theme's in
//! `themes/ID/icons/svg/`), hand-written on a 64 by 64 grid (Ben,
//! 2026-11-17: "convert the large high quality one to an SVG ... add SVG
//! support to olo for all icons/graphics"; "manually look at each icon and
//! then write out a picture perfect SVG for each one"). A picture shrunk
//! to 16 or 32 pixels is a blur of what it was at 256; a drawing rendered
//! at 32 pixels is drawn for 32 pixels: every edge where the drawing put
//! it, at any size and any scale (20 and 28 pixels too, which no
//! pre-scaled picture was).
//!
//! **Drawn for its size.** A drawing says what shows at which sizes, and
//! its outline keeps its weight, by classes this module answers with a
//! style sheet injected as it's parsed ([`sheet`]):
//!
//! - `o`: a form with its outline: a filled shape whose stroke is painted
//!   under its fill (`paint-order: stroke`), so only the half outside
//!   shows and the fill's own edge stays where it was drawn. The outline's
//!   weight is set for the size: a device pixel up to 80 pixels, then an
//!   eightieth of the size ([`line`]: as heavy, large, as the pictures'
//!   outlines were); `l`, a line of that weight on its
//!   own (a plain stroke: the edge of a hole seen through); `i`, an inner
//!   line, three fifths of it.
//! - `fromN`: shown at N pixels and larger (fine detail: `from48`,
//!   `from96`); `uptoN`: shown up to the step after N (what stands in
//!   for it small: `upto32`, to 47 pixels, where `from48` takes over).
//!   N is one of [`STEPS`], and a size between two is drawn as the
//!   lower ([`step`]).
//!
//! In a program that knows nothing of this (a browser, GTK through the
//! icon theme) the drawing shows whole, with the weights its own `<style>`
//! gives. Other programs' SVG icons are rendered as they are.
//!
//! **Rendered once.** Drawing costs more than decoding a picture did (a
//! fifth of a millisecond at 32 pixels, two at 256, against 10 and 500
//! microseconds), so what's rendered is kept ([`own`]): each drawing at
//! each size asked for, as its pixels, in `~/.cache/olo/icons`, named by
//! the drawing's path, size and when it was last changed. The next
//! program to ask, and the next start, reads it back (as fast as the
//! picture was); a drawing changed (an upgrade) is rendered again and its
//! old pixels removed.
//!
//! **In a theme's colours.** A theme without a drawing of its own for an
//! icon shows the classic set's in its colours ([`recolour`],
//! [`own_recoloured`]): one drawing, so every theme's icon has the same
//! angle, size and detail.

use std::{
    hash::{Hash as _, Hasher as _},
    path::{Path, PathBuf},
};

use crate::Image;

pub mod recolour;
pub use recolour::Recolour;

/// The sizes a drawing's `fromN` and `uptoN` classes can name.
pub const STEPS: [u32; 9] = [16, 24, 32, 48, 64, 96, 128, 256, 512];

/// The grid Olo's drawings are on, when one doesn't say (`viewBox`).
const GRID: f32 = 64.0;

/// An outline's weight at `px` device pixels, in device pixels: one, up
/// to 80 pixels; from there an eightieth of the size.
pub fn line(px: u32) -> f32 {
    (px as f32 / 80.0).max(1.0)
}

/// Whether `path` is one of Olo's own drawings (in an `icons/svg`
/// folder of its data).
pub fn is_own(path: &Path) -> bool {
    let mut up = path.parent().into_iter().flat_map(Path::ancestors);
    up.next().is_some_and(|d| d.ends_with("svg")) && up.next().is_some_and(|d| d.ends_with("icons"))
}

/// The side of the drawing's grid: its `viewBox`'s larger side.
fn grid(data: &[u8]) -> f32 {
    let text = std::str::from_utf8(&data[..data.len().min(600)]).unwrap_or_default();
    let Some(at) = text.find("viewBox=\"") else { return GRID };
    let rest = &text[at + 9..];
    let nums: Vec<f32> = rest[..rest.find('"').unwrap_or(0)].split([' ', ',']).filter_map(|n| n.parse().ok()).collect();
    match nums[..] {
        [_, _, w, h] if w > 0.0 && h > 0.0 => w.max(h),
        _ => GRID,
    }
}

/// The step a size of `px` device pixels is drawn as: the largest of
/// [`STEPS`] it reaches (the smallest, under that). What a drawing shows
/// changes only at a step, so `upto32` and `from48` between them cover
/// every size, 40 pixels too. They didn't: the classes were compared
/// with the size itself, and from 33 to 47 pixels a drawing showed
/// neither (a 20 point icon on a 2× screen is 40: Slack's tile with no
/// mark on it in Deskbar, on that screen alone; Ben, 2026-11-18).
pub fn step(px: u32) -> u32 {
    STEPS.iter().rev().copied().find(|&n| n <= px).unwrap_or(STEPS[0])
}

/// The style sheet for one of Olo's drawings (on a grid `grid` units a
/// side) rendered at `px` device pixels: what its size leaves out, and
/// its lines' weights.
pub fn sheet(px: u32, grid: f32) -> String {
    use std::fmt::Write as _;
    let mut css = String::new();
    let at = step(px);
    for n in STEPS {
        if at < n {
            let _ = write!(css, ".from{n}{{display:none !important}}");
        }
        if at > n {
            let _ = write!(css, ".upto{n}{{display:none !important}}");
        }
    }
    let unit = grid / px.max(1) as f32;
    let w = line(px) * unit;
    // (An outline's stroke is twice its weight: half of it is under the fill.)
    let _ = write!(css, ".o{{stroke-width:{:.3} !important}}.l{{stroke-width:{w:.3} !important}}.i{{stroke-width:{:.3} !important}}", w * 2.0, w * 0.6);
    css
}

/// SVG `data` rendered into a square `px` device pixels a side (centred,
/// its proportions kept); `own`: one of Olo's drawings, drawn for the
/// size ([`sheet`]).
pub fn render(data: &[u8], px: u32, own: bool) -> Option<Image> {
    let options = resvg::usvg::Options { style_sheet: own.then(|| sheet(px, grid(data))), ..Default::default() };
    let tree = resvg::usvg::Tree::from_data(data, &options).ok()?;
    let size = tree.size();
    let scale = px as f32 / size.width().max(size.height());
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
    let dx = (px as f32 - size.width() * scale) / 2.0;
    let dy = (px as f32 - size.height() * scale) / 2.0;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut pixmap.as_mut());
    Some(Image { width: px, height: px, rgba: pixmap.take(), premultiplied: true })
}

/// What changes when what's kept must be rendered again whatever the
/// drawing: the style sheet's rules.
const KEPT_VERSION: u32 = 3;

/// Where the pixels of drawing `path` at `px` are kept, and the start of
/// the names of every keeping of it at that size (an older drawing's);
/// `theme`: in that theme's colours (its id, its rules' key).
fn kept_at(cache: &Path, path: &Path, px: u32, theme: Option<(&str, u64)>) -> Option<(PathBuf, String)> {
    let meta = std::fs::metadata(path).ok()?;
    let changed = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    let mut rules = String::new();
    if let Some((id, key)) = theme {
        // (The theme is part of which drawing this is; its rules, like
        // the drawing's date, of which keeping: changed, the old one goes.)
        id.hash(&mut h);
        rules = format!("-{key:x}");
    }
    let stem = format!("{:016x}-{px}-", h.finish());
    let name = format!("{stem}{:x}-{:x}-{KEPT_VERSION}{rules}.rgba", changed.as_nanos(), meta.len());
    Some((cache.join(name), stem))
}

/// One of Olo's own drawings at `px` device pixels: read back if it was
/// rendered before (by any program), else rendered and kept.
pub fn own(path: &Path, px: u32) -> Option<Image> {
    own_in(&kept_dir(), path, px)
}

/// Where renderings are kept: the person's cache. A test's (anything
/// cargo runs: it sets `CARGO_MANIFEST_DIR`) are kept in scratch on
/// disk instead: a test never writes in the person's folders.
fn kept_dir() -> PathBuf {
    if std::env::var_os("CARGO_MANIFEST_DIR").is_some() { PathBuf::from("/var/tmp/olo-test-cache/icons") } else { crate::cache_dir().join("icons") }
}

/// [`own`], what's rendered kept in folder `cache`.
pub fn own_in(cache: &Path, path: &Path, px: u32) -> Option<Image> {
    kept(cache, path, px, None)
}

/// The classic set's drawing `path` at `px` device pixels in theme
/// `theme`'s colours (`rules`: [`crate::themes::recolour`]), kept like
/// [`own`].
pub fn own_recoloured(path: &Path, px: u32, theme: &str, rules: &Recolour) -> Option<Image> {
    own_recoloured_in(&kept_dir(), path, px, theme, rules)
}

/// [`own_recoloured`], what's rendered kept in folder `cache`.
pub fn own_recoloured_in(cache: &Path, path: &Path, px: u32, theme: &str, rules: &Recolour) -> Option<Image> {
    kept(cache, path, px, Some((theme, rules)))
}

fn kept(cache: &Path, path: &Path, px: u32, theme: Option<(&str, &Recolour)>) -> Option<Image> {
    let kept = kept_at(cache, path, px, theme.map(|(id, rules)| (id, rules.key())));
    let want = (px as usize).pow(2) * 4;
    if let Some((file, _)) = &kept
        && let Ok(rgba) = std::fs::read(file)
        && rgba.len() == want
    {
        return Some(Image { width: px, height: px, rgba, premultiplied: true });
    }
    let drawing = std::fs::read(path).ok()?;
    let image = match theme {
        Some((_, rules)) => {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            render(rules.apply(&name, std::str::from_utf8(&drawing).ok()?).as_bytes(), px, true)?
        }
        None => render(&drawing, px, true)?,
    };
    if let Some((file, stem)) = kept
        && let Some(dir) = file.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        // (Whole or not there: written beside, then named.)
        let tmp = dir.join(format!(".{}-{}", std::process::id(), file.file_name().unwrap_or_default().to_string_lossy()));
        if std::fs::write(&tmp, &image.rgba).and_then(|()| std::fs::rename(&tmp, &file)).is_ok() {
            // An older drawing's pixels at this size go.
            for old in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let name = old.file_name();
                if name.to_string_lossy().starts_with(&stem) && Some(name.as_os_str()) != file.file_name() {
                    let _ = std::fs::remove_file(old.path());
                }
            }
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAWING: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
<style>.o{stroke-width:1;paint-order:stroke}</style>
<rect class="o" x="8" y="8" width="48" height="48" fill="#ffffff" stroke="#000000"/>
<rect class="from48" x="24" y="24" width="16" height="16" fill="#ff0000"/>
<rect class="upto32" x="24" y="24" width="16" height="16" fill="#0000ff"/>
</svg>"##;

    fn pixel(img: &Image, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3]]
    }

    /// A drawing is drawn for its size: what's for large sizes is left
    /// out small, what stands in for it shows instead, and its outline
    /// is a device pixel at 16 and at 64, and more at 256.
    #[test]
    fn drawn_for_its_size() {
        let small = render(DRAWING.as_bytes(), 32, true).unwrap();
        assert_eq!(pixel(&small, 16, 16), [0, 0, 255, 255], "the small stand-in at 32");
        let large = render(DRAWING.as_bytes(), 64, true).unwrap();
        assert_eq!(pixel(&large, 32, 32), [255, 0, 0, 255], "the detail at 64");
        // Every size shows one or the other, the sizes between two steps
        // too (20 points at 2× is 40 pixels, at 1.5× 30): the stand-in up
        // to 47, the detail from 48.
        for px in 16..=130 {
            let img = render(DRAWING.as_bytes(), px, true).unwrap();
            let want = if px < 48 { [0, 0, 255, 255] } else { [255, 0, 0, 255] };
            assert_eq!(pixel(&img, px / 2, px / 2), want, "at {px} pixels");
        }
        assert_eq!((step(1), step(16), step(31), step(40), step(48), step(600)), (16, 16, 24, 32, 48, 512));
        // The outline, outside the rect's edge at 8 units (2 pixels at
        // 16): the one pixel beside it, whole, at each side; the rect's
        // own first pixel is its fill's.
        let tiny = render(DRAWING.as_bytes(), 16, true).unwrap();
        let dark = |img: &Image, y: u32| (0..img.width).filter(|&x| pixel(img, x, y)[3] > 0 && pixel(img, x, y)[0] < 200).count();
        // (A row clear of the square in the middle.)
        assert_eq!(dark(&tiny, 4), 2, "a pixel's weight at each side");
        assert_eq!((pixel(&tiny, 1, 4), pixel(&tiny, 2, 4)), ([0, 0, 0, 255], [255, 255, 255, 255]), "the outline whole on its pixel, the fill's edge where it was drawn");
        let big = render(DRAWING.as_bytes(), 256, true).unwrap();
        assert!(dark(&big, 50) >= 6, "heavier at 256");
        // Not one of Olo's: as it is (both squares, the last drawn over).
        let plain = render(DRAWING.as_bytes(), 64, false).unwrap();
        assert_eq!(pixel(&plain, 32, 32), [0, 0, 255, 255]);
        assert_eq!((line(16), line(64), line(256)), (1.0, 1.0, 3.2));
        assert!(is_own(Path::new("/x/data/icons/svg/folder.svg")) && is_own(Path::new("/x/themes/platinum/icons/svg/folder.svg")));
        assert!(!is_own(Path::new("/usr/share/icons/hicolor/scalable/apps/folder.svg")));
        assert_eq!(grid(DRAWING.as_bytes()), 64.0);
    }

    /// What's rendered is kept and read back; a drawing changed is
    /// rendered again, and what was kept of the old one goes.
    #[test]
    fn rendered_once() {
        // (On disk: /tmp is memory here.)
        let dir = Path::new("/var/tmp").join(format!("olo-svg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (cache, drawing) = (dir.join("cache"), dir.join("icons/svg/square.svg"));
        std::fs::create_dir_all(drawing.parent().unwrap()).unwrap();
        std::fs::write(&drawing, DRAWING).unwrap();
        let kept = || std::fs::read_dir(&cache).map(|d| d.flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>()).unwrap_or_default();
        let first = own_in(&cache, &drawing, 32).unwrap();
        assert_eq!(kept().len(), 1);
        // Read back: the same pixels, though the drawing's file is gone
        // from under its name's keeping... (still there: only read).
        let again = own_in(&cache, &drawing, 32).unwrap();
        assert_eq!(first.rgba, again.rgba);
        // Kept pixels are what's shown: a keeping tampered with shows so
        // (it was read, not rendered).
        let file = cache.join(&kept()[0]);
        let mut bytes = std::fs::read(&file).unwrap();
        bytes[0] ^= 0xff;
        std::fs::write(&file, &bytes).unwrap();
        assert_ne!(own_in(&cache, &drawing, 32).unwrap().rgba, first.rgba, "read back, not rendered again");
        // Another size beside it.
        own_in(&cache, &drawing, 16).unwrap();
        assert_eq!(kept().len(), 2);
        // The drawing changed: rendered again, the old keeping gone.
        std::fs::write(&drawing, DRAWING.replace("#0000ff", "#00ff00")).unwrap();
        let changed = own_in(&cache, &drawing, 32).unwrap();
        assert_eq!(pixel(&changed, 16, 16), [0, 255, 0, 255]);
        assert_eq!(kept().len(), 2, "one a size");
        // In a theme's colours: kept beside the classic rendering, not in
        // its place; the theme's rules changed, its old keeping goes.
        let blue_to_red = Recolour::parse("turn 100-140 to 0 1 0\n").unwrap();
        let themed = own_recoloured_in(&cache, &drawing, 32, "test", &blue_to_red).unwrap();
        assert_eq!(pixel(&themed, 16, 16), [255, 0, 0, 255], "the green, as the theme has it");
        assert_eq!(kept().len(), 3);
        assert_eq!(own_in(&cache, &drawing, 32).unwrap().rgba, changed.rgba, "the classic rendering is still the classic one");
        let to_blue = Recolour::parse("turn 100-140 to 240 1 0\n").unwrap();
        assert_eq!(pixel(&own_recoloured_in(&cache, &drawing, 32, "test", &to_blue).unwrap(), 16, 16), [0, 0, 255, 255]);
        assert_eq!(kept().len(), 3, "the theme's old keeping went");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every drawing in Olo's sets is on the 64 grid, has its outlines'
    /// style, and renders to something at 16 and at 256 pixels.
    #[test]
    fn every_drawing_renders() {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let mut folders = vec![data.join("icons/svg")];
        folders.extend(std::fs::read_dir(data.join("themes")).into_iter().flatten().flatten().map(|t| t.path().join("icons/svg")));
        let mut seen = 0;
        for folder in folders {
            for f in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let path = f.path();
                if path.extension().is_none_or(|e| e != "svg") {
                    continue;
                }
                seen += 1;
                let bytes = std::fs::read(&path).unwrap();
                let text = String::from_utf8(bytes.clone()).unwrap_or_else(|_| panic!("{}: not text", path.display()));
                assert!(text.contains("viewBox=\"0 0 64 64\""), "{}: not on the 64 grid", path.display());
                assert!(text.contains("paint-order:stroke"), "{}: no outline style", path.display());
                for px in [16, 256] {
                    let img = render(&bytes, px, true).unwrap_or_else(|| panic!("{}: can't be drawn", path.display()));
                    let shown = img.rgba.chunks_exact(4).filter(|p| p[3] > 0).count();
                    assert!(shown * 5 > (px * px) as usize, "{} at {px}: nearly nothing drawn", path.display());
                    if px != 256 {
                        continue;
                    }
                    // The thing itself (its shadow is fainter than this) stays on the
                    // canvas with two units to spare, outline and all: what touches
                    // the edge is cut off.
                    let unit = px as usize / 64;
                    let solid = |x: usize, y: usize| img.rgba[(y * px as usize + x) * 4 + 3] > 100;
                    let (mut x0, mut y0, mut x1, mut y1) = (px as usize, px as usize, 0, 0);
                    for y in 0..px as usize {
                        for x in 0..px as usize {
                            if solid(x, y) {
                                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                            }
                        }
                    }
                    let side = px as usize;
                    assert!(
                        x0 + 1 >= 2 * unit && y0 + 1 >= 2 * unit && x1 <= side - 2 * unit + 1 && y1 <= side - 2 * unit + 1,
                        "{}: drawn outside 2..62 of the grid: {:.1},{:.1} to {:.1},{:.1}",
                        path.display(),
                        x0 as f32 / unit as f32,
                        y0 as f32 / unit as f32,
                        x1 as f32 / unit as f32,
                        y1 as f32 / unit as f32
                    );
                }
            }
        }
        assert!(seen >= 4, "the drawings weren't found");
    }
}
