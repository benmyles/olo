//! The disk map: a disk from start to end as a bar, each partition and
//! stretch of free space to scale (but never too thin to click), coloured
//! by what is on it, as in Haiku's DriveSetup.

use olo_kit::{
    Bounds, FontWeight, Painter, Pixels, Point,
    files::format_size,
    point, px,
    style::{self, c},
    theme,
};
use olo_system::disks::{Disk, Segment, Volume};
use olo_theme::{Hue, Palette, mix};

use crate::Item;

/// The narrowest a segment is drawn.
const MIN_W: f32 = 48.0;
/// Encryption shows as a band along the top of what is inside it.
const BAND: f32 = 6.0;

/// The look's hue for content of `kind` (by family: Linux, Windows and
/// everywhere, encrypted, swap, other, nothing).
fn hue(content: &str) -> Hue {
    match content {
        "ext2" | "ext3" | "ext4" | "btrfs" | "xfs" | "f2fs" | "nilfs2" => Hue::Blue,
        "vfat" | "exfat" | "ntfs" => Hue::Yellow,
        "crypto_LUKS" => Hue::Purple,
        "swap" => Hue::Red,
        "" => Hue::Gray,
        _ => Hue::Teal,
    }
}

/// A segment's fill: its hue, softened toward the document colour so
/// the document's text reads on it in either mode.
fn fill(p: &Palette, content: &str) -> [u8; 4] {
    mix(p.hue(hue(content)), p.document_background, 0.5)
}

/// The widths of `sizes` across `width`: each at least [`MIN_W`], the
/// rest shared by size.
fn widths(sizes: &[u64], width: f32) -> Vec<f32> {
    let total: u64 = sizes.iter().sum();
    let spare = (width - MIN_W * sizes.len() as f32).max(0.0);
    sizes.iter().map(|&s| MIN_W + if total == 0 { spare / sizes.len() as f32 } else { spare * s as f32 / total as f32 }).collect()
}

/// The disk's segments, start to end: what each is and its size (a disk
/// without partitions or content is one empty stretch).
fn segments(disk: &Disk) -> Vec<(Item, u64)> {
    let mut out: Vec<(Item, u64)> = disk
        .segments()
        .into_iter()
        .map(|s| match s {
            Segment::Volume(i) => (Item::Volume(disk.volumes[i].path.clone()), disk.volumes[i].size),
            Segment::Free { offset, size } => (Item::Free { disk: disk.path.clone(), offset, size }, size),
        })
        .collect();
    if out.is_empty() {
        out.push((Item::Disk(disk.path.clone()), disk.size));
    }
    out
}

/// Each segment's place in the map at `rect`, on device pixels at
/// `scale` (as drawn, and as a click is found).
fn places(scale: f32, rect: Bounds<Pixels>, sizes: &[u64]) -> (Bounds<Pixels>, Vec<Bounds<Pixels>>) {
    // (Inside its two-line bevel, as `style::inset`.)
    let edge = px(2.0 * style::line_pixels(scale) / scale);
    let inner = Bounds::from_corners(point(rect.left() + edge, rect.top() + edge), point(rect.right() - edge, rect.bottom() - edge));
    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    let mut x = inner.left();
    let n = sizes.len();
    let places = widths(sizes, f32::from(inner.size.width))
        .into_iter()
        .enumerate()
        .map(|(i, w)| {
            let right = if i + 1 == n { inner.right() } else { snap(x + px(w)) };
            let r = Bounds::from_corners(point(x, inner.top()), point(right, inner.bottom()));
            x = right;
            r
        })
        .collect();
    (inner, places)
}

/// What of `disk` is at `at` in its map drawn in `rect`.
pub fn item_at(scale: f32, rect: Bounds<Pixels>, disk: &Disk, at: Point<Pixels>) -> Option<Item> {
    let segments = segments(disk);
    let sizes: Vec<u64> = segments.iter().map(|s| s.1).collect();
    let (_, places) = places(scale, rect, &sizes);
    let i = places.iter().position(|r| r.contains(&at))?;
    segments.into_iter().nth(i).map(|s| s.0)
}

/// Paints the map of `disk` in `rect` (`selected` washed).
pub fn paint(g: &mut Painter, rect: Bounds<Pixels>, disk: &Disk, selected: Option<&Item>) {
    let p = theme::palette();
    let p = &*p;
    let segments = segments(disk);
    let sizes: Vec<u64> = segments.iter().map(|s| s.1).collect();
    let (inner, places) = places(g.scale_factor(), style::snap(g, rect), &sizes);
    style::fill_rect(g, inner, c(p.document_background));
    let line = c(mix(p.document_text, p.document_background, 0.55));
    let n = segments.len();
    for (i, ((item, size), r)) in segments.iter().zip(places.iter().copied()).enumerate() {
        let volume = match item {
            Item::Volume(path) => disk.volumes.iter().find(|v| &v.path == path),
            _ => None,
        };
        let (title, detail) = match volume {
            Some(v) => paint_volume(g, p, r, v),
            None => {
                paint_free(g, p, r);
                (if matches!(item, Item::Disk(_)) { "Empty".to_owned() } else { "Free".to_owned() }, format_size(*size))
            }
        };
        if i + 1 < n {
            style::vline_left_of(g, r.right(), r.top(), r.bottom(), line);
        }
        if selected == Some(item) || selected.is_some_and(|s| matches!((s, volume), (Item::Volume(p), Some(v)) if v.cleartext.as_ref().is_some_and(|c| &c.path == p))) {
            style::selection_wash(g, r, p);
        }
        // Where a segment after the first starts follows the sizes, not
        // the window's grid.
        if i > 0 {
            olo_kit::review::own_grid(g, r);
        }
        // Words when they fit.
        if f32::from(r.size.width) >= 44.0 {
            let color = c(p.document_text);
            let mid = r.center().y;
            // Inset as the list's cells are, so the first lines up with
            // its names.
            let inset = px(olo_kit::metrics::CELL_INSET);
            let w = r.size.width - inset * 2.0;
            style::label_fit(g, r.left() + inset, mid - px(7.0), style::Align::Left, title, style::PLAIN_SIZE - 1.0, style::ui_font(FontWeight::BOLD), color, w);
            style::label_fit(g, r.left() + inset, mid + px(7.0), style::Align::Left, detail, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), color, w);
        }
    }
    style::sunken(g, rect, p, false);
}

/// A volume: its colour (an encrypted one unlocked shows what is inside,
/// under a band); its title and detail.
fn paint_volume(g: &mut Painter, p: &Palette, r: Bounds<Pixels>, v: &Volume) -> (String, String) {
    let shown = v.cleartext.as_deref().unwrap_or(v);
    let content = if shown.content.is_empty() && !shown.usage.is_empty() { "?" } else { shown.content.as_str() };
    style::fill_rect(g, r, c(fill(p, content)));
    if v.cleartext.is_some() {
        style::fill_rect(g, Bounds::from_corners(r.origin, point(r.right(), r.top() + px(BAND))), c(fill(p, "crypto_LUKS")));
    }
    // A thin lighter edge along the top, as raised.
    style::hline(g, r.left(), r.right(), r.top(), c(p.gloss));
    (v.title(), format!("{} {}", format_size(v.size), shown.content_name()))
}

/// Free space: hatched (one layer for all its lines).
fn paint_free(g: &mut Painter, p: &Palette, r: Bounds<Pixels>) {
    let hatch = c(mix(p.document_text, p.document_background, 0.82));
    let w = style::hairline(g).max(px(1.0));
    g.with_content_mask(Some(olo_kit::ContentMask { bounds: r }), |g| {
        g.paint_layer(r, |g| {
            let h = r.size.height;
            let mut x = r.left() - h;
            while x < r.right() {
                g.paint_segment(point(x, r.bottom()), point(x + h, r.top()), w, false, hatch);
                x += px(7.0);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn widths_share_by_size_with_a_minimum() {
        let w = super::widths(&[1, 1000, 999], 1000.0);
        assert_eq!(w.len(), 3);
        assert!(w[0] >= super::MIN_W);
        assert!((w.iter().sum::<f32>() - 1000.0).abs() < 0.01);
        assert!((w[1] - w[2]).abs() < 2.0);
    }
}
