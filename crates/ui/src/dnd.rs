//! Dragging out of a window to another app or a folder (Wayland's data
//! device, through GPUI's `Window::start_drag`): the same data in several types, most faithful
//! first (files as `text/uri-list` and their paths as text), the actions
//! the place it's dropped on may take, and a picture under the pointer:
//! the thing's icon, its name below in the selection's colours. Start one
//! as a press drags (the button still down); what it was dropped as comes
//! back when it's over.

use std::{path::PathBuf, sync::Arc};

use gpui::{App, DragAction, DragImage, Font, OutgoingDrag, Pixels, Point, Window, px};

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
pub fn picture(window: &mut Window, cx: &App, icon: &str, logical: f32, label: Option<&str>, hotspot: Point<Pixels>) -> Option<DragImage> {
    // Whole device pixels: the compositor scales a buffer by whole numbers.
    let scale = window.scale_factor().ceil().max(1.0) as u32;
    let sc = scale as usize;
    let side = (logical * scale as f32).round().max(1.0) as u32;
    let bmp = crate::icons::bitmap(icon, side)?;
    let s = bmp.size(0);
    let (w, h) = (s.width.0 as usize, s.height.0 as usize);
    let src = bmp.as_bytes(0)?;
    let text = label.filter(|l| !l.is_empty()).and_then(|l| {
        let font: Font = style::ui_font(gpui::FontWeight::NORMAL);
        let fitted = style::fit(window, l.to_owned().into(), style::PLAIN_SIZE, &font, px(220.0));
        window.text_system().line_mask(&fitted, &font, px(style::PLAIN_SIZE), scale as f32)
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
        let p = theme::palette(cx);
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

/// Drags `drag` out of `window` (while the press that began it is down);
/// `done` gets what it was dropped as (None: cancelled) when it's over.
pub fn start(window: &Window, cx: &mut App, drag: OutgoingDrag, done: impl FnOnce(Option<DragAction>, &mut App) + 'static) {
    let over = window.start_drag(drag);
    cx.spawn(async move |cx| {
        let action = over.await.unwrap_or(None);
        let _ = cx.update(|cx| done(action, cx));
    })
    .detach();
}
