//! Dragging out of a window to another app or a folder (Wayland's data
//! device, through `WindowCx::start_drag`): the same data in several types, most faithful
//! first (files as `text/uri-list` and their paths as text), the actions
//! the place it's dropped on may take, and a picture under the pointer:
//! the thing's icon, its name below in the selection's colours. Start one
//! as a press drags (the button still down); what it was dropped as comes
//! back when it's over.

use std::{path::PathBuf, sync::Arc};

use olo_interface::{Cx, DragAction, DragImage, Font, FontWeight, OutgoingDrag, Pixels, Point, View, WindowCx, px};

use crate::{style, theme};

/// The type, with no data, asking a folder to make links to the files
/// rather than copies (an application dragged out of Deskbar).
pub const LINK_TYPE: &str = "application/x-olo-make-link";

/// Files: `text/uri-list` (file:// URIs, what file managers, browsers and
/// editors take) and plain text (the paths, a line each, for terminals
/// and text fields).
pub fn files(paths: &[PathBuf], copy: bool, move_: bool) -> OutgoingDrag {
    let uris: String = paths.iter().map(|p| format!("{}\r\n", olo_kit_base::files::file_uri(p))).collect();
    let text = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
    OutgoingDrag {
        types: vec![
            ("text/uri-list".into(), Arc::from(uris.into_bytes())),
            ("text/plain;charset=utf-8".into(), Arc::from(text.clone().into_bytes())),
            ("text/plain".into(), Arc::from(text.into_bytes())),
        ],
        copy,
        move_,
        ask: false,
        icon: None,
    }
}

/// `drag`, asking a folder to make links rather than copies.
pub fn as_link(mut drag: OutgoingDrag) -> OutgoingDrag {
    drag.types.insert(0, (LINK_TYPE.into(), Arc::from(Vec::new())));
    drag
}

/// Icon `icon` `logical` points square, faded a little, with `label`
/// below it in the selection's colours, for this window's scale (whole
/// device pixels: a picture's pixel to a device pixel); the pointer at
/// `hotspot` on the icon. None while the icon loads (ask for it ahead
/// with [`crate::icons::bitmap`]).
pub fn picture(cx: &WindowCx, icon: &str, logical: f32, label: Option<&str>, hotspot: Point<Pixels>) -> Option<DragImage> {
    // Whole device pixels: the compositor scales a buffer by whole numbers.
    let scale = cx.scale_factor().ceil().max(1.0) as u32;
    let sc = scale as usize;
    let side = (logical * scale as f32).round().max(1.0) as u32;
    let bmp = crate::icons::bitmap(icon, side)?;
    let s = bmp.size(0);
    let (w, h) = (s.width.0 as usize, s.height.0 as usize);
    let src = bmp.as_bytes(0)?;
    let text = label.filter(|l| !l.is_empty()).and_then(|l| {
        let font: Font = style::ui_font(FontWeight::NORMAL);
        let fitted = style::fit_with(cx.text_system(), l.to_owned().into(), style::PLAIN_SIZE, &font, px(220.0));
        cx.text_system().line_mask(&fitted, &font, px(style::PLAIN_SIZE), scale as f32)
    });
    let (pad_x, pad_y, gap) = (4 * sc, sc, 2 * sc);
    let (lw, lh) = text.as_ref().map_or((0, 0), |(size, ..)| (size.width.0 as usize + 2 * pad_x, size.height.0 as usize + 2 * pad_y));
    let cw = w.max(lw);
    let ch = h + if lh > 0 { gap + lh } else { 0 };
    let (ix, lx, ly) = ((cw - w) / 2, (cw - lw) / 2, h + gap);
    // Premultiplied RGBA, source over destination.
    let mut out = vec![0u8; cw * ch * 4];
    let over = |out: &mut [u8], x: usize, y: usize, rgba: [f32; 4]| {
        let o = (y * cw + x) * 4;
        let a = rgba[3];
        for c in 0..3 {
            out[o + c] = (rgba[c] * a * 255.0 + out[o + c] as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
        }
        out[o + 3] = (a * 255.0 + out[o + 3] as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    };
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            // (BGRA, straight alpha.)
            let px = [src[i + 2] as f32 / 255.0, src[i + 1] as f32 / 255.0, src[i] as f32 / 255.0, src[i + 3] as f32 / 255.0 * 0.75];
            over(&mut out, ix + x, y, px);
        }
    }
    if let Some((size, mask, _)) = text {
        let p = theme::palette();
        let rgba = |c: olo_theme::Rgba, a: f32| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0 * a];
        let (bg, fg) = (rgba(p.list_selected_background, 0.7), rgba(p.list_selected_item_text, 0.9));
        for y in ly..ly + lh {
            for x in lx..lx + lw {
                over(&mut out, x, y, bg);
            }
        }
        let (mw, mh) = (size.width.0 as usize, size.height.0 as usize);
        for y in 0..mh {
            for x in 0..mw {
                let cov = mask[y * mw + x] as f32 / 255.0;
                if cov > 0.0 {
                    over(&mut out, lx + pad_x + x, ly + pad_y + y, [fg[0], fg[1], fg[2], fg[3] * cov]);
                }
            }
        }
    }
    let hotspot = Point { x: hotspot.x + px((ix / sc) as f32), y: hotspot.y };
    Some(DragImage { rgba: Arc::from(out), width: cw as u32, height: ch as u32, scale, hotspot })
}

/// What a drag of a picture carries under the pointer when what's
/// dragged is the picture (a screenshot out of its notification): the
/// picture itself, small (128 points at most on its long side; one of a
/// few pixels made larger as blocks of whole device pixels, 48 points or
/// so, never smoothed) and seven tenths opaque, as the kit's own drags
/// are, the pointer in its middle. `picture` straight (not
/// premultiplied). Its size in device pixels is a whole number of points
/// times the whole scale its buffer has: a buffer whose size isn't a
/// multiple of its scale is an error to the display (a 1000 × 667
/// window's at 2× was 256 × 171). Its edge a line a point wide in `edge`
/// (the frame its picture had where it was shown), wholly opaque: over
/// a ground of its own colours (a picture of the desktop over the
/// desktop), it doesn't melt into it. Milliseconds for a large picture:
/// made off the window's looper, ahead of the drag.
pub fn picture_of(picture: &image::RgbaImage, scale: f32, edge: [u8; 4]) -> Option<DragImage> {
    let (size, by, blocks) = picture_size(picture.width(), picture.height(), scale)?;
    let small = if blocks { image::imageops::resize(picture, size.0, size.1, image::imageops::FilterType::Nearest) } else { image::imageops::thumbnail(picture, size.0, size.1) };
    let mut rgba = small.into_raw();
    let (w, h, line) = (size.0 as usize, size.1 as usize, by as usize);
    // (Premultiplied, at seven tenths; its edge whole.)
    for (i, p) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % w, i / w);
        if x < line || y < line || x + line >= w || y + line >= h {
            *p = [edge[0], edge[1], edge[2], 255];
            continue;
        }
        let a = p[3] as f32 / 255.0 * 0.7;
        for c in &mut p[..3] {
            *c = (*c as f32 * a).round() as u8;
        }
        p[3] = (a * 255.0).round() as u8;
    }
    Some(DragImage { rgba: Arc::from(rgba), width: size.0, height: size.1, scale: by, hotspot: olo_interface::point(px(size.0 as f32 / by as f32 / 2.0), px(size.1 as f32 / by as f32 / 2.0)) })
}

/// [`picture_of`]'s size for a picture of `w` × `h` pixels: in device
/// pixels, the whole scale its buffer has, and whether it is made of
/// blocks (a picture smaller than 48 points, made larger).
fn picture_size(w: u32, h: u32, scale: f32) -> Option<((u32, u32), u32, bool)> {
    if w == 0 || h == 0 {
        return None;
    }
    let by = scale.ceil().max(1.0) as u32;
    let long = w.max(h);
    // (In points, whole: then times the scale.)
    let points = |pixels: f32| ((pixels / by as f32).round() as u32).max(1);
    if long < 48 * by {
        let times = (48 * by).div_ceil(long);
        return Some(((points((w * times) as f32) * by, points((h * times) as f32) * by), by, true));
    }
    let shrink = ((128 * by) as f32 / long as f32).min(1.0);
    Some(((points(w as f32 * shrink) * by, points(h as f32 * shrink) * by), by, false))
}

/// Drags `drag` out of the view's window (while the press that began it
/// is down); the view is sent what `done` makes of what it was dropped
/// as (None: cancelled) when it's over.
pub fn start<V: View>(cx: &mut Cx<V>, drag: OutgoingDrag, done: impl FnOnce(Option<DragAction>) -> V::Message + 'static)
where
    V::Message: Send,
{
    let (over, to) = (cx.start_drag(drag), cx.messenger());
    cx.spawn(|_| async move {
        let action = over.await.unwrap_or(None);
        to.send(done(action));
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drag's picture has a size its buffer may have: a whole number of points times its scale.
    #[test]
    fn a_drags_picture_is_whole_points() {
        // (A 1000 × 667 window's at 2× was 256 × 171: no multiple of 2.)
        for (w, h, scale) in [(1000, 667, 2.0), (1000, 667, 1.5), (1280, 800, 1.0), (333, 1001, 1.75), (3, 1001, 2.0), (4, 8, 2.0), (1, 1, 3.0), (2560, 1600, 1.25)] {
            let ((dw, dh), by, _) = picture_size(w, h, scale).unwrap();
            assert!(dw > 0 && dh > 0 && dw % by == 0 && dh % by == 0 && by == scale.ceil() as u32, "{w} × {h} at {scale}: {dw} × {dh}, scale {by}");
            assert!(dw.max(dh) <= 128 * by + by, "{w} × {h} at {scale}: {dw} × {dh}");
        }
        // A few pixels are carried as blocks, large enough to see; nothing is nothing.
        assert_eq!(picture_size(4, 8, 1.0), Some(((24, 48), 1, true)));
        assert_eq!(picture_size(0, 8, 1.0), None);
        let tiny = image::RgbaImage::from_pixel(4, 8, image::Rgba([10, 20, 30, 255]));
        let edge = [90, 100, 110, 255];
        let carried = picture_of(&tiny, 2.0, edge).unwrap();
        assert_eq!((carried.width, carried.height, carried.scale, carried.rgba.len()), (48, 96, 2, 48 * 96 * 4));
        // Seven tenths opaque, premultiplied, inside an edge a point wide (two device pixels at 2×), wholly
        // opaque; the pointer in its middle (points).
        let at = |x: usize, y: usize| &carried.rgba[(y * 48 + x) * 4..][..4];
        assert_eq!((at(0, 0), at(1, 1), at(47, 95), at(1, 50)), (&edge[..], &edge[..], &edge[..], &edge[..]));
        assert_eq!(at(2, 2), &[7, 14, 21, 179]);
        assert_eq!((f32::from(carried.hotspot.x), f32::from(carried.hotspot.y)), (12.0, 24.0));
        // A screen's picture: 128 points on its long side, its shape kept.
        let screen = image::RgbaImage::from_pixel(2560, 1600, image::Rgba([200, 200, 200, 255]));
        let carried = picture_of(&screen, 2.0, edge).unwrap();
        assert_eq!((carried.width, carried.height), (256, 160));
    }
}
