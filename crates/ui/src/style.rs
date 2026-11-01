//! Olo's look in GPUI: colours from the palette, and the BeOS drawing
//! primitives (lines, frames, bevels, wells, gradients, labels), each on
//! whole device pixels at any scale, as the egui kit's `style`: a line
//! is one device pixel at 1× and 1.5× and two at 2× (`line_pixels`), and
//! every edge is rounded onto the device grid, so nothing is smeared
//! across two pixels at 1.5×. Painting functions take the window and are
//! called from an element's paint (`canvas`, a custom `Element`).

use gpui::{App, Bounds, Font, FontWeight, Hsla, Pixels, Point, SharedString, TextRun, Window, fill, linear_color_stop, linear_gradient, point, px, size};
use olo_theme::{Palette, Rgba};

/// Olo's text sizes, in points: plain (labels, controls), small, and the
/// monospaced face's.
pub const PLAIN_SIZE: f32 = 12.0;
pub const SMALL_SIZE: f32 = 10.5;
/// Secondary text a point under the plain size: a row's subtitle and
/// kind, a chip, a caption (the egui kit's `small_font`).
pub const DETAIL_SIZE: f32 = PLAIN_SIZE - 1.0;
pub const MONO_SIZE: f32 = 12.0;

/// Olo's faces.
pub const UI_FONT: &str = "Noto Sans";
pub const MONO_FONT: &str = "Noto Sans Mono";

/// The line height of wrapped text, as a fraction of the font size
/// (Noto Sans's ascent and descent: 1362 of its 1000 units, as egui
/// spaces it).
pub const LINE_HEIGHT: f32 = 1.362;

/// Cap height as a fraction of the font size (Noto Sans's: 714 of its
/// 1000 units).
pub const CAP_HEIGHT: f32 = 0.714;

/// A palette colour.
pub fn c(rgba: Rgba) -> Hsla {
    gpui::Rgba { r: f32::from(rgba[0]) / 255.0, g: f32::from(rgba[1]) / 255.0, b: f32::from(rgba[2]) / 255.0, a: f32::from(rgba[3]) / 255.0 }.into()
}

/// `rgba` darkened (tint above 1) or lightened (below) the BeOS way.
pub fn tint(rgba: Rgba, tint: f32) -> Hsla {
    c(olo_theme::tint(rgba, tint))
}

/// `rgba` shaded for the palette's mode (darker in light mode, lighter
/// in dark), as the kit's `shade`.
pub fn shade(palette: &Palette, rgba: Rgba, tint: f32) -> Hsla {
    c(palette.shade(rgba, tint))
}

/// Text that can't be used now.
pub fn disabled_text(p: &Palette) -> Hsla {
    c(olo_theme::mix(p.panel_text, p.panel_background, 0.55))
}

/// A one-point line's width in device pixels at `scale`.
pub fn line_pixels(scale: f32) -> f32 {
    scale.floor().max(1.0)
}

/// A one-point line's width in points at this window's scale.
pub fn hairline(window: &Window) -> Pixels {
    let s = window.scale_factor();
    px(line_pixels(s) / s)
}

/// `v` on the device pixel grid.
pub fn snap_coord(window: &Window, v: Pixels) -> Pixels {
    let s = window.scale_factor();
    px((f32::from(v) * s).round() / s)
}

/// `b`'s edges on the device pixel grid.
pub fn snap(window: &Window, b: Bounds<Pixels>) -> Bounds<Pixels> {
    let min = point(snap_coord(window, b.origin.x), snap_coord(window, b.origin.y));
    let max = point(snap_coord(window, b.origin.x + b.size.width), snap_coord(window, b.origin.y + b.size.height));
    Bounds::from_corners(min, max)
}

/// A `w` × `h` box centred on `center`, its size and edges on whole
/// device pixels (a swatch, a mark: sharp, and as near the middle as the
/// pixels allow).
pub fn snap_centred(window: &Window, center: Point<Pixels>, w: f32, h: f32) -> Bounds<Pixels> {
    let s = window.scale_factor();
    let (dw, dh) = ((w * s).round(), (h * s).round());
    let min = point(px(((f32::from(center.x) * s - dw / 2.0).round()) / s), px(((f32::from(center.y) * s - dh / 2.0).round()) / s));
    Bounds { origin: min, size: size(px(dw / s), px(dh / s)) }
}

/// `b` on device pixels, `lines` lines in from each side: where to fill
/// inside a frame or bevel of that many lines, flush against them.
pub fn inset(window: &Window, b: Bounds<Pixels>, lines: f32) -> Bounds<Pixels> {
    let d = snap(window, b);
    let w = hairline(window) * lines;
    Bounds::from_corners(point(d.origin.x + w, d.origin.y + w), point(d.right() - w, d.bottom() - w))
}

fn rect(window: &mut Window, min: Point<Pixels>, max: Point<Pixels>, color: Hsla) {
    if max.x > min.x && max.y > min.y {
        let b = Bounds::from_corners(min, max);
        window.paint_quad(fill(b, color));
        if color.a > 0.0 {
            crate::review::fill(window, b);
        }
    }
}

/// Fills `b`, on device pixels.
pub fn fill_rect(window: &mut Window, b: Bounds<Pixels>, color: Hsla) {
    let d = snap(window, b);
    rect(window, d.origin, d.bottom_right(), color);
}

/// A line's span across from `edge` (snapped), or back from it when the
/// line ends there.
fn line_span(window: &Window, edge: Pixels, ends_there: bool) -> (Pixels, Pixels) {
    let e = snap_coord(window, edge);
    let w = hairline(window);
    if ends_there { (e - w, e) } else { (e, e + w) }
}

/// One-point horizontal line from x0 to x1 whose top is at y.
pub fn hline(window: &mut Window, x0: Pixels, x1: Pixels, y: Pixels, color: Hsla) {
    let (t, b) = line_span(window, y, false);
    let (x0, x1) = (snap_coord(window, x0), snap_coord(window, x1));
    rect(window, point(x0, t), point(x1, b), color);
}

/// One-point horizontal line whose bottom is at y.
pub fn hline_above(window: &mut Window, x0: Pixels, x1: Pixels, y: Pixels, color: Hsla) {
    let (t, b) = line_span(window, y, true);
    let (x0, x1) = (snap_coord(window, x0), snap_coord(window, x1));
    rect(window, point(x0, t), point(x1, b), color);
}

/// One-point vertical line whose left edge is at x.
pub fn vline(window: &mut Window, x: Pixels, y0: Pixels, y1: Pixels, color: Hsla) {
    let (l, r) = line_span(window, x, false);
    let (y0, y1) = (snap_coord(window, y0), snap_coord(window, y1));
    rect(window, point(l, y0), point(r, y1), color);
}

/// One-point vertical line whose right edge is at x.
pub fn vline_left_of(window: &mut Window, x: Pixels, y0: Pixels, y1: Pixels, color: Hsla) {
    let (l, r) = line_span(window, x, true);
    let (y0, y1) = (snap_coord(window, y0), snap_coord(window, y1));
    rect(window, point(l, y0), point(r, y1), color);
}

fn frame_on_pixels(window: &mut Window, d: Bounds<Pixels>, w: Pixels, color: Hsla) {
    let (l, t, r, b) = (d.origin.x, d.origin.y, d.right(), d.bottom());
    rect(window, point(l, t), point(r, t + w), color);
    rect(window, point(l, b - w), point(r, b), color);
    rect(window, point(l, t + w), point(l + w, b - w), color);
    rect(window, point(r - w, t + w), point(r, b - w), color);
}

/// One-point outline just inside `b`.
pub fn frame_rect(window: &mut Window, b: Bounds<Pixels>, color: Hsla) {
    let d = snap(window, b);
    let w = hairline(window);
    frame_on_pixels(window, d, w, color);
}

fn bevel_on_pixels(window: &mut Window, d: Bounds<Pixels>, w: Pixels, tl: Hsla, br: Hsla) {
    let (l, t, r, b) = (d.origin.x, d.origin.y, d.right(), d.bottom());
    rect(window, point(l, t), point(r - w, t + w), tl);
    rect(window, point(l, t + w), point(l + w, b - w), tl);
    rect(window, point(l, b - w), point(r, b), br);
    rect(window, point(r - w, t), point(r, b - w), br);
}

/// Two-tone bevel just inside `b`: `tl` on top and left, `br` on bottom
/// and right.
pub fn bevel(window: &mut Window, b: Bounds<Pixels>, tl: Hsla, br: Hsla) {
    let d = snap(window, b);
    let w = hairline(window);
    bevel_on_pixels(window, d, w, tl, br);
}

/// The BeOS sunken well of text fields, lists and scroll views: a dark
/// and light outer bevel and a darker inner line on top and left (the
/// keyboard's colour all round when focused).
pub fn sunken(window: &mut Window, b: Bounds<Pixels>, palette: &Palette, focused: bool) {
    let bg = palette.panel_background;
    let d = snap(window, b);
    let w = hairline(window);
    bevel_on_pixels(window, d, w, tint(bg, 1.147), c(palette.shine));
    let inner = Bounds::from_corners(point(d.origin.x + w, d.origin.y + w), point(d.right() - w, d.bottom() - w));
    if focused {
        frame_on_pixels(window, inner, w, c(palette.keyboard_navigation));
    } else {
        bevel_on_pixels(window, inner, w, tint(bg, 1.55), tint(bg, 1.06));
    }
}

/// A raised panel edge (light top and left, dark bottom and right).
pub fn raised(window: &mut Window, b: Bounds<Pixels>, palette: &Palette) {
    bevel(window, b, c(palette.shine), tint(palette.panel_background, 1.3));
}

/// A vertical gradient filling `b`, `top` to `bottom`.
pub fn vgradient(window: &mut Window, b: Bounds<Pixels>, top: Hsla, bottom: Hsla) {
    let d = snap(window, b);
    if d.size.width > px(0.0) && d.size.height > px(0.0) {
        window.paint_quad(fill(d, linear_gradient(180.0, linear_color_stop(top, 0.0), linear_color_stop(bottom, 1.0))));
        if top.a >= 1.0 && bottom.a >= 1.0 {
            crate::review::gradient(window, d);
        }
    }
}

/// A horizontal gradient filling `b`, `left` to `right`.
pub fn hgradient(window: &mut Window, b: Bounds<Pixels>, left: Hsla, right: Hsla) {
    let d = snap(window, b);
    if d.size.width > px(0.0) && d.size.height > px(0.0) {
        window.paint_quad(fill(d, linear_gradient(90.0, linear_color_stop(left, 0.0), linear_color_stop(right, 1.0))));
        if left.a >= 1.0 && right.a >= 1.0 {
            crate::review::gradient(window, d);
        }
    }
}

/// The width of `text` in `font` at `size` points, without a window (to
/// size one before it opens): its characters' advances.
pub fn text_width_app(cx: &App, text: &str, size: f32, font: &Font) -> f32 {
    let ts = cx.text_system();
    let id = ts.resolve_font(font);
    text.chars().filter_map(|ch| ts.advance(id, px(size), ch).ok()).map(|a| f32::from(a.width)).sum()
}

/// Which edge of the text `x` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A font of Olo's UI face.
pub fn ui_font(weight: FontWeight) -> Font {
    Font { family: UI_FONT.into(), weight, ..gpui::font(UI_FONT) }
}

/// The width of `text` in `font` at `size` points.
pub fn text_width(window: &mut Window, text: impl Into<SharedString>, size: f32, font: Font) -> Pixels {
    let text = text.into();
    let run = TextRun { len: text.len(), font, color: Hsla::default(), background_color: None, underline: None, strikethrough: None };
    window.text_system().shape_line(text, px(size), &[run], None).width
}

/// One line of text with its capitals centred on `center_y` and its
/// baseline on a device pixel, `x` its left edge, centre or right edge
/// (`align`). Returns where it was drawn.
#[allow(clippy::too_many_arguments)]
pub fn label(window: &mut Window, cx: &mut App, x: Pixels, center_y: Pixels, align: Align, text: impl Into<SharedString>, font_size: f32, font: Font, color: Hsla) -> Bounds<Pixels> {
    let text = text.into();
    let run = TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None };
    let line = window.text_system().shape_line(text.clone(), px(font_size), &[run], None);
    let left = match align {
        Align::Left => x,
        Align::Center => x - line.width / 2.0,
        Align::Right => x - line.width,
    };
    let left = snap_coord(window, left);
    let baseline = snap_coord(window, center_y + px(font_size * CAP_HEIGHT / 2.0));
    let top = baseline - line.ascent;
    let height = line.ascent + line.descent;
    let _ = line.paint(point(left, top), height, window, cx);
    let at = Bounds { origin: point(left, top), size: size(line.width, height) };
    crate::review::text(window, at, baseline, &text, font_size);
    at
}

/// `text` cut short with "…" to fit `max_width` (whole as it is when it
/// fits), measured as it's shaped, kerning and all.
pub fn fit(window: &mut Window, text: SharedString, font_size: f32, font: &Font, max_width: Pixels) -> SharedString {
    let shape = |window: &mut Window, t: SharedString| {
        let run = TextRun { len: t.len(), font: font.clone(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None };
        window.text_system().shape_line(t, px(font_size), &[run], None)
    };
    let line = shape(window, text.clone());
    if line.width <= max_width {
        return text;
    }
    let ellipsis = shape(window, "…".into()).width;
    let mut end = line.index_for_x(max_width - ellipsis).unwrap_or(text.len());
    // Shaped as a whole, a cut can still come out a little wider (a
    // kern, a ligature taken apart): one character back until it fits.
    loop {
        let cut: SharedString = format!("{}…", text[..end].trim_end()).into();
        if end == 0 || shape(window, cut.clone()).width <= max_width {
            return cut;
        }
        end = text[..end].char_indices().next_back().map_or(0, |(i, _)| i);
    }
}

/// Keys as menus and the shortcuts sheet write them ("⇧⌘P", "Ctrl+Q"),
/// in the plain size, `right_center` their right edge and middle: each
/// symbol laid out on its own, ⌘ and ⇧ a point closer to what follows
/// than their advance, all on one baseline on a device pixel. Returns
/// where they were drawn.
pub fn key_combo(window: &mut Window, cx: &mut App, right_center: Point<Pixels>, keys: &str, color: Hsla) -> Bounds<Pixels> {
    let font = ui_font(FontWeight::NORMAL);
    let parts = combo_parts(keys);
    let mut x = right_center.x;
    for (i, part) in parts.iter().enumerate().rev() {
        let lift = px(symbol_lift(window, part, &font));
        let lift = snap_coord(window, lift);
        let at = label(window, cx, x, right_center.y + lift, Align::Right, part.clone(), PLAIN_SIZE, font.clone(), color);
        x = at.left();
        if i > 0 {
            x += px(SHORTCUT_KERN);
        }
    }
    Bounds::from_corners(point(x, right_center.y - px(PLAIN_SIZE / 2.0)), point(right_center.x, right_center.y + px(PLAIN_SIZE / 2.0)))
}

/// The width of keys as [`key_combo`] draws them.
pub fn key_combo_width(window: &mut Window, keys: &str) -> f32 {
    let font = ui_font(FontWeight::NORMAL);
    let parts = combo_parts(keys);
    parts.iter().map(|t| f32::from(text_width(window, t.clone(), PLAIN_SIZE, font.clone()))).sum::<f32>() - SHORTCUT_KERN * parts.len().saturating_sub(1) as f32
}

/// ⌘ and ⇧ sit this much closer to what follows than their advance width.
const SHORTCUT_KERN: f32 = 1.0;

thread_local! {
    static LIFTS: std::cell::RefCell<std::collections::HashMap<char, f32>> = Default::default();
}

/// How far down (in points) to set a key symbol about as tall as the
/// capitals (⇧ ⌘ ↵ ⌫ ⏏) so it stands on their line, by its outline: its
/// face may set it a pixel off (Noto's ⇧ is one low). Smaller ones
/// (← ⌥ ⌃ ⇥) and letters sit as their face has them. Kept per symbol.
fn symbol_lift(window: &Window, part: &str, font: &Font) -> f32 {
    let mut chars = part.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return 0.0 };
    if ch.is_ascii() {
        return 0.0;
    }
    if let Some(lift) = LIFTS.with(|l| l.borrow().get(&ch).copied()) {
        return lift;
    }
    let ts = window.text_system().clone();
    let size = px(PLAIN_SIZE);
    let run = TextRun { len: part.len(), font: font.clone(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None };
    // The face it comes from (a fallback, often).
    let line = ts.shape_line(SharedString::from(part.to_owned()), size, &[run], None);
    let lift = match (line.runs.first().map(|r| ts.typographic_bounds(r.font_id, size, ch)), ts.typographic_bounds(ts.resolve_font(font), size, 'H')) {
        // (Bounds rise from the baseline: their origin is the foot.)
        (Some(Ok(g)), Ok(h)) if (g.size.height - h.size.height).abs() <= px(2.0) => f32::from(g.origin.y - h.origin.y),
        _ => 0.0,
    };
    LIFTS.with(|l| l.borrow_mut().insert(ch, lift));
    lift
}

/// The keys split so each symbol (⌘ ⇧ ↑ ⌫ …) is laid out on its own.
fn combo_parts(label: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut run = String::new();
    for ch in label.chars() {
        if ch.is_ascii() {
            run.push(ch);
        } else {
            if !run.is_empty() {
                parts.push(std::mem::take(&mut run));
            }
            parts.push(ch.to_string());
        }
    }
    if !run.is_empty() {
        parts.push(run);
    }
    parts
}

/// A soft shadow under a panel floating over everything (the command
/// palette, the shortcuts sheet): rings of the look's drop shadow, each
/// a point wider and fainter, a little lower.
pub fn soft_shadow(window: &mut Window, r: Bounds<Pixels>, palette: &Palette) {
    let [red, green, blue, alpha] = palette.drop_shadow;
    for i in 1..=6u32 {
        let a = (alpha as u32 * (26 - i * 4) / 40) as u8;
        let grow = px(i as f32);
        let ring = snap(window, Bounds::from_corners(point(r.left() - grow, r.top() - grow + grow * 0.5), point(r.right() + grow, r.bottom() + grow + grow * 0.5)));
        window.paint_quad(gpui::fill(ring, c([red, green, blue, a])).corner_radii(px(4.0 + i as f32)));
    }
}

/// [`label`] cut short with "…" to fit `max_width` ([`fit`]).
#[allow(clippy::too_many_arguments)]
pub fn label_fit(window: &mut Window, cx: &mut App, x: Pixels, center_y: Pixels, align: Align, text: impl Into<SharedString>, font_size: f32, font: Font, color: Hsla, max_width: Pixels) -> Bounds<Pixels> {
    let text = fit(window, text.into(), font_size, &font, max_width.max(px(0.0)));
    label(window, cx, x, center_y, align, text, font_size, font, color)
}

/// Selection over `r`, for a thing selected in place with nothing round
/// it (a part of a bar, a cell flush with its neighbours): the list's
/// selection colour, see-through, and its edge hairline just inside.
/// Paint it after the thing's ground and before its text.
pub fn selection_wash(window: &mut Window, r: Bounds<Pixels>, palette: &Palette) {
    fill_rect(window, r, c(palette.list_selected_background).opacity(0.7));
    frame_rect(window, r, c(palette.selection_edge()));
}

/// A rubber-band selection rectangle (Tracker, desktop, any list): a
/// one-pixel navigation-blue frame over a faint tint of the same colour.
pub fn selection_band(window: &mut Window, r: Bounds<Pixels>, palette: &Palette) {
    let r = snap(window, r);
    let [red, green, blue, _] = palette.keyboard_navigation;
    fill_rect(window, r, c([red, green, blue, 40]));
    frame_rect(window, r, c(palette.keyboard_navigation));
}
