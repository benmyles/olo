//! Olo's look in GPUI: colours from the palette, and the BeOS drawing
//! primitives (lines, frames, bevels, wells, gradients, labels), each on
//! whole device pixels at any scale: a line
//! is one device pixel at 1× and 1.5× and two at 2× (`line_pixels`), and
//! every edge is rounded onto the device grid, so nothing is smeared
//! across two pixels at 1.5×. Painting functions take the window and are
//! called from an element's paint (`canvas`, a custom `Element`).

use gpui::{App, Bounds, Font, FontWeight, Hsla, PathBuilder, Pixels, Point, SharedString, TextRun, Window, fill, linear_color_stop, linear_gradient, point, px, size};
use olo_theme::{Palette, Rgba};

/// Olo's text sizes, in points: plain (labels, controls), small, and the
/// monospaced face's.
pub const PLAIN_SIZE: f32 = 12.0;
pub const SMALL_SIZE: f32 = 10.5;
/// A full-screen view's title (the palm trainer's step) and its lead
/// text under it.
pub const TITLE_SIZE: f32 = 24.0;
pub const LEAD_SIZE: f32 = 15.0;
/// Secondary text a point under the plain size: a row's subtitle and
/// kind, a chip, a caption.
pub const DETAIL_SIZE: f32 = PLAIN_SIZE - 1.0;
pub const MONO_SIZE: f32 = 12.0;

/// Olo's faces.
pub const UI_FONT: &str = "Noto Sans";
pub const MONO_FONT: &str = "Noto Sans Mono";

/// The line height of wrapped text, as a fraction of the font size
/// (Noto Sans's ascent and descent: 1362 of its 1000 units).
pub const LINE_HEIGHT: f32 = 1.362;

/// Cap height as a fraction of the font size (Noto Sans's: 714 of its
/// 1000 units).
pub const CAP_HEIGHT: f32 = 0.714;

/// The row a line of text `size` points tall sits in: its line height,
/// up to the 2-point grid (`metrics`).
pub fn line_row(size: f32) -> f32 {
    (size * LINE_HEIGHT / 2.0).ceil() * 2.0
}

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

/// A font of Olo's monospaced face.
pub fn mono_font(weight: FontWeight) -> Font {
    Font { family: MONO_FONT.into(), weight, ..gpui::font(MONO_FONT) }
}

/// The width of `text` in `font` at `size` points.
pub fn text_width(window: &mut Window, text: impl Into<SharedString>, size: f32, font: Font) -> Pixels {
    let text = text.into();
    let run = TextRun { len: text.len(), font, color: Hsla::default(), background_color: None, underline: None, strikethrough: None, size: None };
    window.text_system().shape_line(text, px(size), &[run], None).width
}

/// One line of text with its capitals centred on `center_y` and its
/// baseline on a device pixel, `x` its left edge, centre or right edge
/// (`align`). Returns where it was drawn.
#[allow(clippy::too_many_arguments)]
pub fn label(window: &mut Window, cx: &mut App, x: Pixels, center_y: Pixels, align: Align, text: impl Into<SharedString>, font_size: f32, font: Font, color: Hsla) -> Bounds<Pixels> {
    let text = text.into();
    let run = TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None, size: None };
    let line = window.text_system().shape_line(text.clone(), px(font_size), &[run], None);
    paint_label(window, cx, x, center_y, align, &text, font_size, line)
}

/// A line shaped for [`label`], painted.
#[allow(clippy::too_many_arguments)]
fn paint_label(window: &mut Window, cx: &mut App, x: Pixels, center_y: Pixels, align: Align, text: &str, font_size: f32, line: gpui::ShapedLine) -> Bounds<Pixels> {
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
    crate::review::text(window, at, baseline, text, font_size);
    crate::cut::note_if_clipped(window, text, at);
    at
}

/// `text` cut short with "…" to fit `max_width` (whole as it is when it
/// fits), measured as it's shaped, kerning and all.
pub fn fit(window: &mut Window, text: SharedString, font_size: f32, font: &Font, max_width: Pixels) -> SharedString {
    let shape = |window: &mut Window, t: SharedString| {
        let run = TextRun { len: t.len(), font: font.clone(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None, size: None };
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
    let run = TextRun { len: part.len(), font: font.clone(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None, size: None };
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
    let text = text.into();
    // (Shaped once when it fits, as nearly everything does: a table's
    // screen of cells is 1,500 of them.)
    let run = TextRun { len: text.len(), font: font.clone(), color, background_color: None, underline: None, strikethrough: None, size: None };
    let line = window.text_system().shape_line(text.clone(), px(font_size), &[run], None);
    if line.width <= max_width.max(px(0.0)) {
        return paint_label(window, cx, x, center_y, align, &text, font_size, line);
    }
    let shown = fit(window, text.clone(), font_size, &font, max_width.max(px(0.0)));
    let at = label(window, cx, x, center_y, align, shown, font_size, font, color);
    // Its whole, in a tip for the pointer resting on it.
    crate::cut::note(window, text, at);
    at
}

/// [`label`] in runs (colours, what a search found marked behind): one
/// line, the capitals centred on `center_y`, the baseline on a pixel.
pub fn label_runs(window: &mut Window, cx: &mut App, x: Pixels, center_y: Pixels, align: Align, text: SharedString, font_size: f32, runs: &[TextRun]) -> Bounds<Pixels> {
    let line = window.text_system().shape_line(text.clone(), px(font_size), runs, None);
    let left = snap_coord(window, match align {
        Align::Left => x,
        Align::Center => x - line.width / 2.0,
        Align::Right => x - line.width,
    });
    let baseline = snap_coord(window, center_y + px(font_size * CAP_HEIGHT / 2.0));
    let top = baseline - line.ascent;
    let height = line.ascent + line.descent;
    let _ = line.paint_background(point(left, top), height, window, cx);
    let _ = line.paint(point(left, top), height, window, cx);
    let at = Bounds { origin: point(left, top), size: size(line.width, height) };
    crate::review::text(window, at, baseline, &text, font_size);
    crate::cut::note_if_clipped(window, &text, at);
    at
}

/// [`label_runs`] cut short with "…" (in the last run's look) to fit
/// `max_width`.
pub fn label_runs_fit(window: &mut Window, cx: &mut App, x: Pixels, center_y: Pixels, align: Align, text: SharedString, font_size: f32, runs: &[TextRun], max_width: Pixels) -> Bounds<Pixels> {
    let max_width = max_width.max(px(0.0));
    let line = window.text_system().shape_line(text.clone(), px(font_size), runs, None);
    if line.width <= max_width || runs.is_empty() {
        return label_runs(window, cx, x, center_y, align, text, font_size, runs);
    }
    let ellipsis_run = |r: &TextRun| TextRun { len: "…".len(), background_color: None, ..r.clone() };
    let ellipsis = window.text_system().shape_line("…".into(), px(font_size), &[ellipsis_run(&runs[0])], None).width;
    let mut end = line.index_for_x(max_width - ellipsis).unwrap_or(text.len());
    loop {
        let cut = text[..end].trim_end();
        // The runs as far as the cut, then the ellipsis.
        let mut cut_runs: Vec<TextRun> = Vec::new();
        let mut at = 0;
        for r in runs {
            if at >= cut.len() {
                break;
            }
            let len = r.len.min(cut.len() - at);
            cut_runs.push(TextRun { len, ..r.clone() });
            at += r.len;
        }
        let last = cut_runs.last().cloned().unwrap_or_else(|| runs[0].clone());
        cut_runs.push(ellipsis_run(&last));
        let shown: SharedString = format!("{cut}…").into();
        let width = window.text_system().shape_line(shown.clone(), px(font_size), &cut_runs, None).width;
        if end == 0 || width <= max_width {
            let at = label_runs(window, cx, x, center_y, align, shown, font_size, &cut_runs);
            crate::cut::note(window, text, at);
            return at;
        }
        end = text[..end].char_indices().next_back().map_or(0, |(i, _)| i);
    }
}

/// Selection over `r`, for a thing selected in place with nothing round
/// it (a part of a bar, a cell flush with its neighbours): the list's
/// selection colour, see-through, and its edge hairline just inside.
/// Paint it after the thing's ground and before its text.
pub fn selection_wash(window: &mut Window, r: Bounds<Pixels>, palette: &Palette) {
    fill_rect(window, r, c(palette.list_selected_background).opacity(0.7));
    frame_rect(window, r, c(palette.selection_edge()));
}

/// The plate a selected picture, swatch or preview sits on: a rounded
/// ground in the selection's colours, `metrics::SELECTION_PAD` beyond `r`
/// all round. Paint it before the thing, as Tracker
/// sets a selected icon on its highlight.
pub fn selection_plate(window: &mut Window, r: Bounds<Pixels>, palette: &Palette) {
    let r = snap(window, r.dilate(px(olo_kit_base::metrics::SELECTION_PAD)));
    window.paint_quad(
        gpui::fill(r, c(palette.list_selected_background))
            .corner_radii(px(olo_kit_base::metrics::CARD_RADIUS))
            .border_widths(hairline(window))
            .border_color(c(palette.selection_edge())),
    );
}

/// The ground of a group that is current as a whole (Deskbar's workspace
/// showing): a BListView selection as big as the group, its heading and
/// rows together. It is `under`, the ground it sits on, taken most of the
/// way to the selection's, so the rows in it still mark their own focus,
/// hover and separators (shading this ground as they would `under`); a
/// row chosen inside it takes the menu's selection, which stands out on
/// it where the list's would not. Edge it with [`lit_edges`].
pub fn lit_ground(palette: &Palette, under: olo_theme::Rgba) -> olo_theme::Rgba {
    olo_theme::mix(under, palette.list_selected_background, 0.7)
}

/// A lit group's edges (see [`lit_ground`]): a hairline in the
/// selection's edge colour across the top and bottom of `r`, its sides
/// running off the panel's. Paint them after its rows, whose fills and
/// separators would cover the bottom one.
pub fn lit_edges(window: &mut Window, r: Bounds<Pixels>, palette: &Palette) {
    let edge = c(palette.selection_edge());
    hline(window, r.left(), r.right(), r.top(), edge);
    hline_above(window, r.left(), r.right(), r.bottom(), edge);
}

/// Where the line of `font_size` text whose capitals are centred on
/// `center_y` has its baseline, on a device pixel: for setting other
/// things (a count, a smaller time) on that line.
pub fn baseline_at(window: &Window, center_y: Pixels, font_size: f32) -> Pixels {
    snap_coord(window, center_y + px(font_size * CAP_HEIGHT / 2.0))
}

/// One line of text on `baseline` (beside a label of another size, on
/// the baseline it was drawn on), `x` its left, centre or right.
#[allow(clippy::too_many_arguments)]
pub fn label_on_baseline(window: &mut Window, cx: &mut App, x: Pixels, baseline: Pixels, align: Align, text: impl Into<SharedString>, font_size: f32, font: Font, color: Hsla) -> Bounds<Pixels> {
    label(window, cx, x, baseline - px(font_size * CAP_HEIGHT / 2.0), align, text, font_size, font, color)
}

/// [`label_on_baseline`] cut short with "…" to fit `max_width` ([`label_fit`]:
/// its whole in a tip for the pointer resting on it).
#[allow(clippy::too_many_arguments)]
pub fn label_on_baseline_fit(window: &mut Window, cx: &mut App, x: Pixels, baseline: Pixels, align: Align, text: impl Into<SharedString>, font_size: f32, font: Font, color: Hsla, max_width: Pixels) -> Bounds<Pixels> {
    label_fit(window, cx, x, baseline - px(font_size * CAP_HEIGHT / 2.0), align, text, font_size, font, color, max_width)
}

/// How tall a [`pill`] of `font_size` text is.
pub fn pill_height(font_size: f32) -> f32 {
    (font_size + 5.0).round()
}

/// How wide a [`pill`] saying `text` is.
pub fn pill_width(window: &mut Window, text: &str, font_size: f32, font: &Font) -> f32 {
    let h = pill_height(font_size);
    (f32::from(text_width(window, text.to_owned(), font_size, font.clone())) + h * 0.9).max(h).round()
}

/// A short tag in a pill (a badge, a count), its left edge at `x` and
/// centred on `cy`, on device pixels. Returns where it is.
#[allow(clippy::too_many_arguments)]
pub fn pill(window: &mut Window, cx: &mut App, x: Pixels, cy: Pixels, text: &str, font_size: f32, font: Font, fill: Hsla, ink: Hsla) -> Bounds<Pixels> {
    let (w, h) = (pill_width(window, text, font_size, &font), pill_height(font_size));
    let r = snap(window, Bounds { origin: point(x, cy - px(h / 2.0)), size: size(px(w), px(h)) });
    window.paint_quad(gpui::fill(r, fill).corner_radii(r.size.height / 2.0));
    label(window, cx, r.center().x, r.center().y, Align::Center, text.to_owned(), font_size, font, ink);
    r
}

/// A [`pill`] whose text sits on `baseline` (a count beside a label of
/// another size: centring each on one middle leaves them a pixel apart
/// at some scales).
#[allow(clippy::too_many_arguments)]
pub fn pill_on_baseline(window: &mut Window, cx: &mut App, x: Pixels, baseline: Pixels, text: &str, font_size: f32, font: Font, fill: Hsla, ink: Hsla) -> Bounds<Pixels> {
    let (w, h) = (pill_width(window, text, font_size, &font), pill_height(font_size));
    let cy = baseline - px(font_size * CAP_HEIGHT / 2.0);
    let r = snap(window, Bounds { origin: point(x, cy - px(h / 2.0)), size: size(px(w), px(h)) });
    window.paint_quad(gpui::fill(r, fill).corner_radii(r.size.height / 2.0));
    label_on_baseline(window, cx, r.center().x, baseline, Align::Center, text.to_owned(), font_size, font, ink);
    r
}

/// A card's colours: the gutter it sits
/// in, the card, its edge, a well in it, the card under the pointer, and
/// text on it (the document's, and quieter).
#[derive(Clone, Copy, Debug)]
pub struct CardInk {
    pub gutter: Hsla,
    pub card: Hsla,
    pub edge: Hsla,
    pub well: Hsla,
    pub hover: Hsla,
    pub text: Hsla,
    pub dim: Hsla,
}

/// The gutter and card colours: in light mode a card is the paper on a
/// slightly darker gutter, in dark mode a little lighter than its paper.
pub fn card_paper(palette: &Palette) -> (Rgba, Rgba) {
    let paper = palette.document_background;
    let ink = palette.document_text;
    if palette.mode == olo_theme::ThemeMode::Dark { (paper, olo_theme::mix(paper, ink, 0.055)) } else { (olo_theme::mix(paper, ink, 0.045), paper) }
}

pub fn card_ink(palette: &Palette) -> CardInk {
    let (gutter, card) = card_paper(palette);
    let ink = palette.document_text;
    let dark = palette.mode == olo_theme::ThemeMode::Dark;
    let see = |a: f32| c([ink[0], ink[1], ink[2], (a * 255.0).round() as u8]);
    CardInk {
        gutter: c(gutter),
        card: c(card),
        edge: c(olo_theme::mix(card, ink, if dark { 0.09 } else { 0.13 })),
        well: see(if dark { 0.06 } else { 0.05 }),
        hover: c(olo_theme::mix(card, ink, 0.025)),
        text: c(ink),
        dim: see(0.52),
    }
}

/// A card's paper over its shadow (a dashboard's card, a note in
/// Notifications' history): lifted a
/// little more under the pointer (`hover`), in the selection's colours
/// and lifted more when `selected`.
pub fn card(window: &mut Window, r: Bounds<Pixels>, palette: &Palette, hover: bool, selected: bool) {
    let ink = card_ink(palette);
    let r = snap(window, r);
    let radius = px(olo_kit_base::metrics::CARD_RADIUS);
    let dark = palette.mode == olo_theme::ThemeMode::Dark;
    let (y, blur, a) = if selected { (2.0, 10.0, if dark { 110 } else { 40 }) } else { (1.0, 4.0, if dark { 70 } else { 22 }) };
    let shadow = gpui::BoxShadow { color: c([0, 0, 0, a]), offset: point(px(0.0), px(y)), blur_radius: px(blur / 2.0), spread_radius: px(0.0) };
    window.paint_shadows(r, gpui::Corners::all(radius), &[shadow]);
    let (fill, edge) = if selected { (c(palette.list_selected_background), c(palette.selection_edge())) } else { (if hover { ink.hover } else { ink.card }, ink.edge) };
    window.paint_quad(gpui::fill(r, fill).corner_radii(radius).border_widths(hairline(window)).border_color(edge));
}

/// `n` with commas between its thousands ("12,345").
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A rubber-band selection rectangle (Tracker, desktop, any list): a
/// one-pixel navigation-blue frame over a faint tint of the same colour.
pub fn selection_band(window: &mut Window, r: Bounds<Pixels>, palette: &Palette) {
    let r = snap(window, r);
    let [red, green, blue, _] = palette.keyboard_navigation;
    fill_rect(window, r, c([red, green, blue, 40]));
    frame_rect(window, r, c(palette.keyboard_navigation));
}

/// A busy spinner round `center`,
/// `radius` points: an arc three points wide that turns and breathes.
/// It moves: the caller asks for the next frame while it shows.
pub fn spinner(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let t = START.get_or_init(std::time::Instant::now).elapsed().as_secs_f32();
    let r = radius - 2.0;
    let (mut start, mut sweep) = (t * std::f32::consts::TAU, 240f32.to_radians() * t.sin());
    if sweep < 0.0 {
        start += sweep;
        sweep = -sweep;
    }
    window.paint_sector(center, px(r - 1.5), px(r + 1.5), start, sweep.max(0.05), color);
}

/// Black or white, whichever reads on `fill`.
pub fn ink_on(fill: Rgba) -> Hsla {
    let l = 0.2126 * fill[0] as f32 + 0.7152 * fill[1] as f32 + 0.0722 * fill[2] as f32;
    if l > 150.0 { c([20, 20, 20, 255]) } else { c([255, 255, 255, 255]) }
}

/// A rounded tile in `fill` with `letter` on it, filling `r`: the icon
/// of something that has a colour but no icon of its own (a website, a
/// contact). Lit along its top, edged a
/// shade darker.
pub fn monogram(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, letter: &str, fill: Rgba) {
    // The letter centred where `r` is (as a label beside it is), not on
    // the tile snapped to the grid, half a pixel away; it's the tile's,
    // as an icon's picture is (not set on the baseline of text beside it,
    // in another size).
    let middle = r.center();
    let r = snap(window, r);
    crate::review::own_grid(window, r);
    let radius = (f32::from(r.size.height) * 0.22).round();
    window.paint_quad(gpui::fill(r, c(fill)).corner_radii(px(radius)).border_widths(hairline(window)).border_color(tint(fill, 1.25)));
    hline(window, r.left() + px(radius), r.right() - px(radius), r.top() + px(1.0), tint(fill, 0.7));
    let size = (f32::from(r.size.height) * 0.62).round();
    label(window, cx, middle.x, middle.y, Align::Center, letter.to_owned(), size, ui_font(FontWeight::BOLD), ink_on(fill));
}

/// A stop sign filling `r` (square, on the device grid): what failed,
/// as a BAlert's stop icon, small (a notification's title, a row).
pub fn stop_mark(window: &mut Window, r: Bounds<Pixels>, palette: &Palette) {
    let side = f32::from(r.size.width.min(r.size.height));
    let r = snap_centred(window, r.center(), side, side);
    let (m, h) = (r.center(), side / 2.0 - 0.5);
    let corner = h / (std::f32::consts::PI / 8.0).cos();
    let corners: Vec<Point<Pixels>> = (0..8)
        .map(|k| {
            let a = std::f32::consts::PI / 8.0 + k as f32 * std::f32::consts::PI / 4.0;
            point(m.x + px(corner * a.cos()), m.y + px(corner * a.sin()))
        })
        .collect();
    let outline = |mut path: gpui::PathBuilder| {
        path.move_to(corners[0]);
        for p in &corners[1..] {
            path.line_to(*p);
        }
        path.close();
        path.build().ok()
    };
    if let Some(path) = outline(gpui::PathBuilder::fill()) {
        window.paint_path(path, c(palette.failure));
    }
    if let Some(path) = outline(gpui::PathBuilder::stroke(px(1.0))) {
        window.paint_path(path, tint(palette.failure, 1.3));
    }
    let ink = ink_on(palette.failure);
    let w = (side * 0.15).max(1.5);
    let bar = Bounds::from_corners(point(m.x - px(w / 2.0), m.y - px(h * 0.6)), point(m.x + px(w / 2.0), m.y + px(h * 0.12)));
    window.paint_quad(gpui::fill(bar, ink).corner_radii(px(w / 2.0)));
    window.paint_sector(point(m.x, m.y + px(h * 0.45)), px(0.0), px(w * 0.6), 0.0, std::f32::consts::TAU, ink);
}

/// BOutlineListView's latch: a small solid triangle centred in `r`,
/// pointing right while its group is folded and down while open.
pub fn latch(window: &mut Window, r: Bounds<Pixels>, open: bool, color: Hsla) {
    let m = snap_centred(window, r.center(), 8.0, 8.0).center();
    let at = |x: f32, y: f32| point(m.x + px(x), m.y + px(y));
    let pts = if open { [at(-4.0, -2.0), at(4.0, -2.0), at(0.0, 2.5)] } else { [at(-2.0, -4.0), at(2.5, 0.0), at(-2.0, 4.0)] };
    let mut path = gpui::PathBuilder::fill();
    path.move_to(pts[0]);
    path.line_to(pts[1]);
    path.line_to(pts[2]);
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// A small × filling `r` (a close box on a card or a row).
pub fn close_box(window: &mut Window, r: Bounds<Pixels>, color: Hsla) {
    let k = Bounds::from_corners(r.origin + point(px(3.0), px(3.0)), r.bottom_right() - point(px(3.0), px(3.0)));
    window.paint_segment(k.origin, k.bottom_right(), px(1.2), true, color);
    window.paint_segment(point(k.right(), k.top()), point(k.left(), k.bottom()), px(1.2), true, color);
}

/// What [`pull_request_mark`] shows: GitHub's four marks for a pull
/// request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestShape {
    /// A branch coming back into another, an arrow at its head.
    Open,
    /// The same, the coming branch dotted.
    Draft,
    /// Joined: the branch curving into the other.
    Merged,
    /// The coming branch ends in a cross.
    Closed,
}

/// GitHub's pull request mark, as big as `r` is high (drawn on a grid of
/// 16), its lines in `color` and whole device pixels at every scale: the
/// commits rings, the head one filled with `node` if given (its checks'
/// colour).
pub fn pull_request_mark(window: &mut Window, r: Bounds<Pixels>, shape: PullRequestShape, color: Hsla, node: Option<Hsla>) {
    let s = window.scale_factor();
    let side = f32::from(r.size.height.min(r.size.width));
    let size_of = |w: f32, h: f32| size(px(w), px(h));
    let k = side / 16.0;
    let dev = (1.5 * k * s).round().max(1.0);
    let w = dev / s;
    // A line's middle where its edges fall on device pixels.
    let at = |v: f32| ((v * s - dev / 2.0).round() + dev / 2.0) / s;
    let o = snap(window, Bounds { origin: r.origin, size: size_of(side, side) }).origin;
    let (ox, oy) = (f32::from(o.x), f32::from(o.y));
    let x = |v: f32| at(ox + v * k);
    let y = |v: f32| at(oy + v * k);
    let p = |x: f32, y: f32| point(px(x), px(y));
    // A commit: a ring this wide (to its line's middle), a hole of a
    // pixel at least; at 1× a whole pixel's, so the one-pixel ring is on
    // pixels.
    let cr = if s < 1.25 { (4.0 * k).round().max(2.0) / 2.0 } else { (1.75 * k).max(w / 2.0 + 1.0 / s) };
    let ring = |window: &mut Window, cx: f32, cy: f32| {
        let b = Bounds { origin: p(cx - cr - w / 2.0, cy - cr - w / 2.0), size: size_of(2.0 * cr + w, 2.0 * cr + w) };
        window.paint_quad(gpui::quad(b, px(cr + w / 2.0), gpui::transparent_black(), px(w), color, gpui::BorderStyle::Solid));
    };
    let disc = |window: &mut Window, cx: f32, cy: f32, radius: f32, fill: Hsla| {
        let b = Bounds { origin: p(cx - radius, cy - radius), size: size_of(2.0 * radius, 2.0 * radius) };
        window.paint_quad(gpui::fill(b, fill).corner_radii(px(radius)));
    };
    let commit = |window: &mut Window, cx: f32, cy: f32| match node {
        Some(fill) => disc(window, cx, cy, cr + w / 2.0, fill),
        None => ring(window, cx, cy),
    };
    let line = |window: &mut Window, pts: &[(f32, f32)]| {
        let mut b = PathBuilder::stroke(px(w));
        for (i, &(x, y)) in pts.iter().enumerate() {
            if i == 0 {
                b.move_to(p(x, y));
            } else {
                b.line_to(p(x, y));
            }
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    };
    let (lx, rx) = (x(3.5), x(12.5));
    let (top, bottom) = (y(3.5), y(12.5));
    ring(window, lx, top);
    ring(window, lx, bottom);
    window.paint_segment(p(lx, top + cr), p(lx, bottom - cr), px(w), false, color);
    match shape {
        PullRequestShape::Merged => {
            // From under the top commit, round into the one on the right.
            let mid = y(8.5);
            let curve: Vec<(f32, f32)> = (0..=10)
                .map(|i| {
                    let t = i as f32 / 10.0;
                    let (a, c, b) = ((lx, top + cr), (lx, mid), (rx - cr, mid));
                    let u = 1.0 - t;
                    (u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1)
                })
                .collect();
            line(window, &curve);
            commit(window, rx, mid);
        }
        _ => {
            match shape {
                PullRequestShape::Open => {
                    // Up from its commit, round to the left, to an arrow
                    // pointing at the base branch.
                    let (bend, turn) = (2.5 * k, top + 2.5 * k);
                    let mut pts = vec![(rx, bottom - cr), (rx, turn)];
                    pts.extend((1..=6).map(|i| {
                        let a = i as f32 / 6.0 * std::f32::consts::FRAC_PI_2;
                        (rx - bend + bend * a.cos(), turn - bend * a.sin())
                    }));
                    let base = ox + 10.0 * k;
                    pts.push((base, top));
                    line(window, &pts);
                    let (tip, half) = (ox + 7.75 * k, 2.0 * k);
                    let mut b = PathBuilder::fill();
                    b.move_to(p(tip, top));
                    b.line_to(p(base, top - half));
                    b.line_to(p(base, top + half));
                    b.close();
                    if let Ok(path) = b.build() {
                        window.paint_path(path, color);
                    }
                }
                PullRequestShape::Draft => {
                    for v in [8.0, 4.5] {
                        disc(window, rx, y(v), w * 0.75 + 0.25 * k, color);
                    }
                }
                _ => {
                    window.paint_segment(p(rx, bottom - cr), p(rx, y(7.5)), px(w), false, color);
                    let d = 1.75 * k;
                    window.paint_segment(p(rx - d, top - d), p(rx + d, top + d), px(w), false, color);
                    window.paint_segment(p(rx - d, top + d), p(rx + d, top - d), px(w), false, color);
                }
            }
            // Over the line coming out of it.
            commit(window, rx, bottom);
        }
    }
}
