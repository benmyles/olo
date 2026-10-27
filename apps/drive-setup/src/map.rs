//! The disk map: a disk from start to end as a bar, each partition and
//! stretch of free space to scale (but never too thin to click), coloured
//! by what is on it, as in Haiku's DriveSetup.

use olo_kit::{
    egui::{self, Align, Color32, Id, Pos2, Rect, Sense, Ui, pos2, vec2},
    files::format_size,
    style::{self, c},
};
use olo_system::disks::{Disk, Segment, Volume};
use olo_theme::{Palette, ThemeMode, mix};

use crate::Item;

/// The narrowest a segment is drawn.
const MIN_W: f32 = 48.0;
/// Encryption shows as a band along the top of what is inside it.
const BAND: f32 = 6.0;

pub struct MapResponse {
    pub clicked: Option<Item>,
    pub context: Option<(Item, Pos2)>,
}

/// The colour for content of `kind` (by family: Linux, Windows and
/// everywhere, encrypted, swap, other, nothing).
fn hue(content: &str) -> [u8; 4] {
    match content {
        "ext2" | "ext3" | "ext4" | "btrfs" | "xfs" | "f2fs" | "nilfs2" => [120, 160, 222, 255],
        "vfat" | "exfat" | "ntfs" => [236, 196, 104, 255],
        "crypto_LUKS" => [176, 146, 214, 255],
        "swap" => [222, 140, 128, 255],
        "" => [196, 196, 196, 255],
        _ => [128, 196, 180, 255],
    }
}

fn fill(p: &Palette, content: &str) -> [u8; 4] {
    match p.mode {
        ThemeMode::Light => hue(content),
        ThemeMode::Dark => mix(hue(content), p.document_background, 0.45),
    }
}

fn text_color(p: &Palette) -> Color32 {
    match p.mode {
        ThemeMode::Light => Color32::from_gray(24),
        ThemeMode::Dark => Color32::from_gray(236),
    }
}

/// The widths of `sizes` across `width`: each at least [`MIN_W`], the
/// rest shared by size.
fn widths(sizes: &[u64], width: f32) -> Vec<f32> {
    let total: u64 = sizes.iter().sum();
    let spare = (width - MIN_W * sizes.len() as f32).max(0.0);
    sizes.iter().map(|&s| MIN_W + if total == 0 { spare / sizes.len() as f32 } else { spare * s as f32 / total as f32 }).collect()
}

pub fn show(ui: &mut Ui, rect: Rect, disk: &Disk, selected: Option<&Item>) -> MapResponse {
    let p = style::palette(ui.ctx());
    let painter = ui.painter().clone();
    let inner = rect.shrink(2.0);
    painter.rect_filled(inner, 0.0, c(p.document_background));
    style::sunken(&painter, rect, &p, false);
    let mut out = MapResponse { clicked: None, context: None };

    // A disk without partitions or content is one empty stretch.
    let mut segments: Vec<(Item, u64)> = disk
        .segments()
        .into_iter()
        .map(|s| match s {
            Segment::Volume(i) => (Item::Volume(disk.volumes[i].path.clone()), disk.volumes[i].size),
            Segment::Free { offset, size } => (Item::Free { disk: disk.path.clone(), offset, size }, size),
        })
        .collect();
    if segments.is_empty() {
        segments.push((Item::Disk(disk.path.clone()), disk.size));
    }
    let sizes: Vec<u64> = segments.iter().map(|s| s.1).collect();
    let line = c(mix(p.document_text, p.document_background, 0.55));
    let mut x = inner.left();
    let n = segments.len();
    for (i, ((item, size), w)) in segments.iter().zip(widths(&sizes, inner.width())).enumerate() {
        let right = if i + 1 == n { inner.right() } else { (x + w).round() };
        let r = Rect::from_min_max(pos2(x, inner.top()), pos2(right, inner.bottom()));
        x = right;
        let volume = match item {
            Item::Volume(path) => disk.volumes.iter().find(|v| &v.path == path),
            _ => None,
        };
        let (title, detail) = match volume {
            Some(v) => paint_volume(&painter, &p, r, v),
            None => {
                paint_free(&painter, &p, r);
                (if matches!(item, Item::Disk(_)) { "Empty".to_owned() } else { "Free".to_owned() }, format_size(*size))
            }
        };
        if i + 1 < n {
            style::vline(&painter, r.right() - 0.5, r.top(), r.bottom(), line);
        }
        // Words when they fit.
        if r.width() >= 44.0 {
            let clip = painter.with_clip_rect(r.shrink2(vec2(4.0, 2.0)));
            let color = if volume.is_some() { text_color(&p) } else { c(p.document_text) };
            let mid = r.center().y;
            style::label(&clip, r.left() + 5.0, mid - 7.0, Align::Min, title, egui::FontId::new(style::PLAIN_SIZE - 1.0, style::bold()), color);
            style::label(&clip, r.left() + 5.0, mid + 7.0, Align::Min, detail, style::small_font(), color);
        }
        if selected == Some(item) || selected.is_some_and(|s| matches!((s, volume), (Item::Volume(p), Some(v)) if v.cleartext.as_ref().is_some_and(|c| &c.path == p))) {
            painter.rect_stroke(r.shrink(1.0), 0.0, egui::Stroke::new(2.0, c(p.keyboard_navigation)), egui::StrokeKind::Inside);
        }
        let resp = ui.interact(r, Id::unique(("drive-map", disk.path.as_str(), i)), Sense::click());
        if resp.clicked() {
            out.clicked = Some(item.clone());
        }
        if resp.secondary_clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            out.context = Some((item.clone(), pos));
        }
    }
    out
}

/// A volume: its colour (an encrypted one unlocked shows what is inside,
/// under a band); its title and detail.
fn paint_volume(painter: &egui::Painter, p: &Palette, r: Rect, v: &Volume) -> (String, String) {
    let shown = v.cleartext.as_deref().unwrap_or(v);
    let content = if shown.content.is_empty() && !shown.usage.is_empty() { "?" } else { shown.content.as_str() };
    painter.rect_filled(r, 0.0, c(fill(p, content)));
    if v.cleartext.is_some() {
        painter.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), BAND)), 0.0, c(fill(p, "crypto_LUKS")));
    }
    // A thin lighter edge along the top, as raised.
    style::hline(painter, r.left(), r.right(), r.top() + 0.5, c(mix(fill(p, content), [255, 255, 255, 255], 0.35)));
    (v.title(), format!("{} {}", format_size(v.size), shown.content_name()))
}

/// Free space: hatched.
fn paint_free(painter: &egui::Painter, p: &Palette, r: Rect) {
    let hatch = c(mix(p.document_text, p.document_background, 0.82));
    let clip = painter.with_clip_rect(r);
    let mut x = r.left() - r.height();
    while x < r.right() {
        clip.line_segment([pos2(x, r.bottom()), pos2(x + r.height(), r.top())], egui::Stroke::new(1.0, hatch));
        x += 7.0;
    }
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
