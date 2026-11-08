//! Form controls the engine draws itself (`appearance: auto`), as Chrome
//! draws them (its NativeThemeBase): checkboxes, radio buttons and a
//! select's arrow, at device scale so they are sharp at every scale.
use super::scene::{ControlKind, ControlLook};
use super::*;
use crate::layout::Rect;
use tiny_skia::{FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, Stroke, Transform};

fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], c[3]);
    p.anti_alias = true;
    p
}
fn round_rect(r: tiny_skia::Rect, radius: f32) -> Option<tiny_skia::Path> {
    let radius = radius.min(r.width() / 2.).min(r.height() / 2.).max(0.);
    // A circle's quarter as a cubic: the control point at 0.5523 of the radius.
    let k = radius * (1. - 0.552_284_8);
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let mut p = PathBuilder::new();
    p.move_to(l + radius, t);
    p.line_to(rt - radius, t);
    p.cubic_to(rt - k, t, rt, t + k, rt, t + radius);
    p.line_to(rt, b - radius);
    p.cubic_to(rt, b - k, rt - k, b, rt - radius, b);
    p.line_to(l + radius, b);
    p.cubic_to(l + k, b, l, b - k, l, b - radius);
    p.line_to(l, t + radius);
    p.cubic_to(l, t + k, l + k, t, l + radius, t);
    p.close();
    p.finish()
}
#[allow(clippy::too_many_arguments)]
pub(super) fn raster(out: &mut Pixmap, look: &ControlLook, bounds: Rect, scale: f64, region: DeviceRect, mask: Option<&Mask>, transform: Transform) {
    let s = scale as f32;
    // On whole device pixels, as Chrome's theme is given them.
    let left = ((bounds.x * scale).round() - f64::from(region.x)) as f32;
    let top = ((bounds.y * scale).round() - f64::from(region.y)) as f32;
    let right = (((bounds.x + bounds.width) * scale).round() - f64::from(region.x)) as f32;
    let bottom = (((bounds.y + bounds.height) * scale).round() - f64::from(region.y)) as f32;
    let (w, h) = (right - left, bottom - top);
    if w <= 0. || h <= 0. {
        return;
    }
    match look.kind {
        ControlKind::Checkbox { checked } | ControlKind::Radio { checked } => {
            let radio = matches!(look.kind, ControlKind::Radio { .. });
            // The largest square in the box, centred.
            let size = w.min(h);
            let Some(r) = tiny_skia::Rect::from_xywh(left + (w - size) / 2., top + (h - size) / 2., size, size) else { return };
            let border = s;
            let radius = if radio { size / 2. } else { 2. * s };
            // The background, a little inside the border so no seam shows
            // at the corners; checked, a checkbox is all accent.
            let inset = border * 0.2;
            let fill = if checked && !radio { look.accent } else { look.fill };
            if let Some(back) = tiny_skia::Rect::from_ltrb(r.left() + inset, r.top() + inset, r.right() - inset, r.bottom() - inset)
                && let Some(path) = round_rect(back, radius)
            {
                out.fill_path(&path, &paint(fill), FillRule::Winding, transform, mask);
            }
            if !(checked && !radio)
                && let Some(edge) = tiny_skia::Rect::from_ltrb(r.left() + border / 2., r.top() + border / 2., r.right() - border / 2., r.bottom() - border / 2.)
                && let Some(path) = round_rect(edge, radius)
            {
                let stroke = Stroke { width: border, ..Default::default() };
                out.stroke_path(&path, &paint(if checked { look.accent } else { look.border }), &stroke, transform, mask);
            }
            if !checked {
                return;
            }
            if radio {
                // The dot: a fifth of the size in from each side.
                let d = size * 0.2;
                if let Some(dot) = tiny_skia::Rect::from_ltrb(r.left() + d, r.top() + d, r.right() - d, r.bottom() - d)
                    && let Some(path) = round_rect(dot, size)
                {
                    out.fill_path(&path, &paint(look.accent), FillRule::Winding, transform, mask);
                }
            } else {
                let mut p = PathBuilder::new();
                p.move_to(r.left() + size * 0.2, r.top() + size * 0.5);
                p.line_to(r.left() + size * 0.4, r.top() + size * 0.7);
                p.line_to(r.right() - size * 0.2, r.top() + size * 0.3);
                if let Some(path) = p.finish() {
                    let stroke = Stroke { width: size * 0.16, ..Default::default() };
                    out.stroke_path(&path, &paint(look.mark), &stroke, transform, mask);
                }
            }
        }
        ControlKind::Menulist => {
            let Some(r) = tiny_skia::Rect::from_ltrb(left, top, right, bottom) else { return };
            let (border, radius) = (s, 1.5 * s);
            let inset = border * 0.2;
            if let Some(back) = tiny_skia::Rect::from_ltrb(r.left() + inset, r.top() + inset, r.right() - inset, r.bottom() - inset)
                && let Some(path) = round_rect(back, radius)
            {
                out.fill_path(&path, &paint(look.fill), FillRule::Winding, transform, mask);
            }
            if let Some(edge) = tiny_skia::Rect::from_ltrb(r.left() + border / 2., r.top() + border / 2., r.right() - border / 2., r.bottom() - border / 2.)
                && let Some(path) = round_rect(edge, radius - border / 2.)
            {
                let stroke = Stroke { width: border, ..Default::default() };
                out.stroke_path(&path, &paint(look.border), &stroke, transform, mask);
            }
        }
        ControlKind::Arrow => {
            // A chevron: two strokes at 45° meeting at the bottom, butt-ended.
            let k = h / (7.6 * s);
            let (half, width) = (4.4 * k * s, 2.1 * k * s);
            let cx = (left + right) / 2.;
            let vertex = bottom - width * std::f32::consts::FRAC_1_SQRT_2;
            let mut p = PathBuilder::new();
            p.move_to(cx - half, vertex - half);
            p.line_to(cx, vertex);
            p.line_to(cx + half, vertex - half);
            if let Some(path) = p.finish() {
                let stroke = Stroke { width, line_cap: LineCap::Butt, line_join: LineJoin::Miter, ..Default::default() };
                out.stroke_path(&path, &paint(look.mark), &stroke, transform, mask);
            }
        }
    }
}
