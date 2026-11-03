//! What a notification shows, laid out the same on screen and in the
//! history's cards: its picture (square; round for a message, from
//! someone; large for something playing), the title, a line with its
//! badge and subtitle, the body (in a balloon for a message), a little
//! table of its fields, a large picture under it all, and a progress bar.
//! An error's title starts with a small stop sign, as a BAlert's would;
//! the app's colour is only in its badge, its tile and its bar.
//! [`lay`] measures (so a stack or a grid can be arranged first),
//! [`paint`] draws.

use olo_kit_base::metrics as m;
use olo_protocol::{NoteLook, NoteStyle};
use olo_theme::Rgba;
use olo_ui::{
    agent,
    gpui::{self, App, BorderStyle, Bounds, Corners, Font, FontStyle, FontWeight, Hsla, Pixels, Point, SharedString, TextAlign, TextRun, UnderlineStyle, Window, WrappedLine, point, px, size},
    style, theme, widgets,
};

use crate::{
    markup::Span,
    pictures::{self, Shape, Source},
};

/// Between the picture and the text, and between blocks.
const GAP: f32 = 8.0;
/// Around the body inside a message's balloon.
const BALLOON_PAD: (f32, f32) = (7.0, 4.0);
/// A field's label, at most this much of the width.
const LABEL_SHARE: f32 = 0.4;
/// A progress bar's height.
const BAR: f32 = 10.0;
/// The stop sign before an error's title, and the room it takes.
const MARK: f32 = 12.0;
pub const MARK_ROOM: f32 = MARK + 4.0;

/// What a note shows.
pub struct Face<'a> {
    pub title: &'a str,
    pub body: &'a [Span],
    pub look: &'a NoteLook,
    pub progress: Option<f32>,
    pub picture: Option<Source>,
    /// It reports a failure: its title starts with a stop sign.
    pub failed: bool,
}

/// Colours for a face on its background.
#[derive(Clone, PartialEq)]
pub struct Ink {
    pub text: Hsla,
    /// The body.
    pub soft: Hsla,
    /// Labels, the subtitle.
    pub dim: Hsla,
    pub link: Hsla,
    /// A message's balloon, and its edge.
    pub balloon: Hsla,
    pub balloon_edge: Hsla,
    /// The app's colour (for its badge and bar).
    pub accent: Option<Rgba>,
}

/// How much room a face has.
pub struct Room {
    pub width: f32,
    pub title_rows: usize,
    pub body_rows: usize,
    /// Kept clear at the title's right (for a close box).
    pub title_reserve: f32,
    /// A standard picture's side.
    pub picture: f32,
    /// The large picture's greatest height.
    pub image_max: f32,
    pub fields_max: usize,
}

/// Text wraps at a width rounded down to a multiple of this: as a window
/// is resized live, text then breaks into lines again (laid out anew)
/// every few points rather than every frame. It is left-aligned, so where
/// its letters go doesn't change.
const WRAP_STEP: f32 = 8.0;

fn wrap_width(width: f32) -> f32 {
    ((width / WRAP_STEP).floor() * WRAP_STEP).max(10.0)
}

/// Text laid out: wrapped to a width and cut short (with "…") at a
/// number of rows; each row a whole number of device pixels tall, so
/// every one's baseline is on a pixel once the first is.
#[derive(Clone)]
pub struct Text {
    lines: Vec<WrappedLine>,
    line_height: Pixels,
    /// Room before its first row (a stop sign's).
    lead: Pixels,
    /// The widest row, and all the rows.
    pub width: f32,
    pub height: f32,
    /// What it says (as laid out: cut short, after `lead`).
    pub text: SharedString,
}

/// A run of `len` bytes in `font`, `color`.
pub fn run(len: usize, font: Font, color: Hsla) -> TextRun {
    TextRun { len, font, color, background_color: None, underline: None, strikethrough: None, size: None }
}

/// Plain text: one run.
pub fn text(window: &mut Window, s: &str, size: f32, font: Font, color: Hsla, width: f32, rows: usize) -> Text {
    lay_runs(window, s.to_owned(), vec![run(s.len(), font, color)], size, width, rows)
}

/// The rows' height for text of `size`, on the device grid.
pub fn line_height(window: &Window, size: f32) -> Pixels {
    let s = window.scale_factor();
    px((size * style::LINE_HEIGHT * s).round() / s)
}

/// `s` in `runs` (covering it), wrapped to `width` and at most `rows`
/// rows, cut short with "…" in the last run's look.
pub fn lay_runs(window: &mut Window, s: String, runs: Vec<TextRun>, size: f32, width: f32, rows: usize) -> Text {
    let (lh, width, rows) = (line_height(window, size), wrap_width(width), rows.max(1));
    let shape = |window: &mut Window, s: &str, runs: &[TextRun]| -> Vec<WrappedLine> {
        window.text_system().shape_text(SharedString::from(s.to_owned()), px(size), runs, Some(px(width)), None).map(|l| l.into_iter().collect()).unwrap_or_default()
    };
    let count = |lines: &[WrappedLine]| lines.iter().map(|l| l.wrap_boundaries.len() + 1).sum::<usize>();
    let mut lines = shape(window, &s, &runs);
    let mut shown = s.clone();
    if count(&lines) > rows {
        // Where the last row that fits ends, then back a character at a
        // time until it fits with "…".
        let mut end = s.len();
        let (mut row, mut offset) = (0, 0);
        'find: for line in &lines {
            for b in &line.wrap_boundaries {
                row += 1;
                if row == rows {
                    end = offset + line.unwrapped_layout.runs[b.run_ix].glyphs[b.glyph_ix].index;
                    break 'find;
                }
            }
            row += 1;
            if row == rows {
                end = offset + line.len();
                break;
            }
            offset += line.len() + 1;
        }
        let mut cut = s[..end].trim_end().to_owned();
        loop {
            let candidate = format!("{cut}…");
            let mut cut_runs = cut_runs(&runs, cut.len());
            if let Some(last) = cut_runs.last_mut() {
                last.len += "…".len();
            }
            let laid = shape(window, &candidate, &cut_runs);
            if count(&laid) <= rows || cut.is_empty() {
                lines = laid;
                shown = candidate;
                break;
            }
            cut.pop();
            let trimmed = cut.trim_end().len();
            cut.truncate(trimmed);
        }
    }
    let rows_n = count(&lines);
    let widest = lines.iter().map(|l| f32::from(l.size(lh).width)).fold(0.0, f32::max);
    Text { lines, line_height: lh, lead: px(0.0), width: widest.ceil(), height: f32::from(lh) * rows_n as f32, text: shown.into() }
}

/// As [`lay_runs`], `lead` before the first row (which is that much
/// narrower), as a typesetter indents a paragraph: that row is laid out
/// to its own width, the rest beneath it to the whole width.
fn lay_led(window: &mut Window, s: String, runs: Vec<TextRun>, size: f32, width: f32, rows: usize, lead: f32) -> Text {
    let narrow = (width - lead).max(10.0);
    // Where the first row ends at the narrower width.
    let end = window
        .text_system()
        .shape_text(SharedString::from(s.clone()), px(size), &runs, Some(px(wrap_width(narrow))), None)
        .ok()
        .and_then(|lines| {
            let l = lines.into_iter().next()?;
            Some(l.wrap_boundaries.first().map_or(l.len(), |b| l.unwrapped_layout.runs[b.run_ix].glyphs[b.glyph_ix].index))
        })
        .unwrap_or(s.len());
    let rest_at = end + s[end..].len() - s[end..].trim_start().len();
    let mut first = if rows <= 1 || rest_at >= s.len() {
        lay_runs(window, s, runs, size, narrow, 1)
    } else {
        let head = s[..end].trim_end().to_owned();
        let mut first = lay_runs(window, head.clone(), cut_runs(&runs, head.len()), size, narrow, 1);
        let rest_runs = tail_runs(&runs, rest_at);
        let rest = lay_runs(window, s[rest_at..].to_owned(), rest_runs, size, width, rows - 1);
        first.lines.extend(rest.lines);
        first.width = (first.width + lead).max(rest.width) - lead;
        first.height += rest.height;
        first.text = format!("{} {}", first.text, rest.text).into();
        first
    };
    first.lead = px(lead);
    first.width += lead;
    first
}

/// `runs` covering what follows the first `from` bytes.
fn tail_runs(runs: &[TextRun], from: usize) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut at = 0;
    for r in runs {
        let (start, end) = (at, at + r.len);
        at = end;
        if end > from {
            out.push(TextRun { len: end - start.max(from), ..r.clone() });
        }
    }
    out
}

/// `runs` covering only the first `len` bytes.
fn cut_runs(runs: &[TextRun], len: usize) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut at = 0;
    for r in runs {
        if at >= len {
            break;
        }
        out.push(TextRun { len: r.len.min(len - at), ..r.clone() });
        at += r.len;
    }
    out
}

impl Text {
    /// Where its first row's baseline is, for a top at `top`.
    pub fn baseline(&self, top: Pixels) -> Pixels {
        let Some(l) = self.lines.first() else { return top + self.line_height };
        let (a, d) = (l.unwrapped_layout.ascent, l.unwrapped_layout.descent);
        top + (self.line_height - a - d) / 2.0 + a
    }

    /// The width of its first row (from its left, its lead and all).
    pub fn first_row_width(&self) -> f32 {
        let Some(l) = self.lines.first() else { return 0.0 };
        f32::from(self.lead)
            + match l.wrap_boundaries.first() {
                Some(b) => f32::from(l.unwrapped_layout.runs[b.run_ix].glyphs[b.glyph_ix].position.x),
                None => f32::from(l.unwrapped_layout.width),
            }
    }

    /// Draw it with its top left at `at` (on the device grid).
    pub fn paint(&self, window: &mut Window, cx: &mut App, at: Point<Pixels>) {
        let at = point(style::snap_coord(window, at.x), style::snap_coord(window, at.y));
        let mut y = at.y;
        for (i, line) in self.lines.iter().enumerate() {
            // (The lead is whole points; snapped, so the first row's
            // letters stay on the grid at any scale.)
            let x = if i == 0 { style::snap_coord(window, at.x + self.lead) } else { at.x };
            let _ = line.paint(point(x, y), self.line_height, TextAlign::Left, None, window, cx);
            y += self.line_height * (line.wrap_boundaries.len() + 1) as f32;
        }
    }

    /// The byte of it under `pos` (its top left at the origin).
    pub fn index_at(&self, pos: Point<Pixels>) -> Option<usize> {
        let (mut y, mut offset) = (px(0.0), 0);
        for (i, line) in self.lines.iter().enumerate() {
            let h = self.line_height * (line.wrap_boundaries.len() + 1) as f32;
            let lead = if i == 0 { self.lead } else { px(0.0) };
            if pos.y >= y && pos.y < y + h {
                return line.index_for_position(point(pos.x - lead, pos.y - y), self.line_height).ok().map(|i| offset + i);
            }
            y += h;
            offset += line.len() + 1;
        }
        None
    }
}

/// A title in `font`, a stop sign's room before it if `failed`.
#[allow(clippy::too_many_arguments)]
pub fn title(window: &mut Window, s: &str, failed: bool, size: f32, font: Font, color: Hsla, width: f32, rows: usize) -> Text {
    let runs = vec![run(s.len(), font, color)];
    if failed { lay_led(window, s.to_owned(), runs, size, width, rows, MARK_ROOM) } else { lay_runs(window, s.to_owned(), runs, size, width, rows) }
}

/// The body with its markup (bold, italics, underlines, links).
pub fn body(window: &mut Window, spans: &[Span], ink: &Ink, width: f32, rows: usize) -> Text {
    let mut s = String::new();
    let mut runs = Vec::new();
    for sp in spans {
        if sp.text.is_empty() {
            continue;
        }
        let weight = if sp.bold { FontWeight::BOLD } else { FontWeight::NORMAL };
        let mut font = style::ui_font(weight);
        if sp.italic {
            font.style = FontStyle::Italic;
        }
        let color = if sp.link.is_some() { ink.link } else { ink.soft };
        let mut r = run(sp.text.len(), font, color);
        if sp.underline || sp.link.is_some() {
            r.underline = Some(UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false });
        }
        s.push_str(&sp.text);
        runs.push(r);
    }
    lay_runs(window, s, runs, style::PLAIN_SIZE, width, rows)
}

/// The link at byte `at` of a body made of `spans`.
pub fn link_at(spans: &[Span], at: usize) -> Option<String> {
    let mut start = 0;
    for s in spans {
        let n = s.text.len();
        if at >= start && at < start + n {
            return s.link.clone();
        }
        start += n;
    }
    None
}

/// A face measured for its room.
#[derive(Clone)]
pub struct Laid {
    picture: f32,
    round: bool,
    title: Text,
    badge: Option<f32>,
    subtitle: Option<Text>,
    body: Option<Text>,
    balloon: bool,
    fields: Vec<(Text, Text)>,
    label_w: f32,
    image: Option<f32>,
    pub height: f32,
}

/// Measure `face` for `room`.
pub fn lay(window: &mut Window, face: &Face, ink: &Ink, room: &Room) -> Laid {
    let look = face.look;
    let picture = match (face.picture.is_some(), look.style) {
        (false, _) => 0.0,
        (true, NoteStyle::Message) => room.picture.max(32.0),
        (true, NoteStyle::Media) => 48.0,
        (true, NoteStyle::Standard) => room.picture,
    };
    let text_x = if picture > 0.0 { picture + GAP } else { 0.0 };
    let text_w = room.width - text_x;
    let bold = style::ui_font(FontWeight::BOLD);
    let small = style::ui_font(FontWeight::NORMAL);
    let title = title(window, face.title, face.failed, style::PLAIN_SIZE, bold, ink.text, text_w - room.title_reserve, room.title_rows);
    let badge = look.badge.as_deref().filter(|b| !b.trim().is_empty()).map(|b| style::pill_width(window, b, style::DETAIL_SIZE, &small).min(text_w));
    let subtitle_w = text_w - badge.map_or(0.0, |w| w + 5.0);
    let subtitle = look.subtitle.as_deref().filter(|s| !s.trim().is_empty()).map(|s| text(window, s, style::DETAIL_SIZE, small.clone(), ink.dim, subtitle_w, 1));
    let has_body = face.body.iter().any(|s| !s.text.trim().is_empty());
    let balloon = has_body && look.style == NoteStyle::Message;
    let body_w = if balloon { text_w - 2.0 * BALLOON_PAD.0 } else { text_w };
    let body = has_body.then(|| body(window, face.body, ink, body_w, room.body_rows));
    let fields: Vec<(String, String)> = look.fields.iter().take(room.fields_max).cloned().collect();
    let label_w = fields.iter().map(|(l, _)| f32::from(style::text_width(window, l.clone(), style::DETAIL_SIZE, small.clone()))).fold(0.0, f32::max).min(text_w * LABEL_SHARE).ceil();
    let fields: Vec<(Text, Text)> = fields
        .iter()
        .map(|(l, v)| (text(window, l, style::DETAIL_SIZE, small.clone(), ink.dim, label_w + WRAP_STEP, 1), text(window, v, style::DETAIL_SIZE, small.clone(), ink.text, text_w - label_w - 6.0, 2)))
        .collect();
    // The column beside the picture.
    let mut column = title.height;
    if badge.is_some() || subtitle.is_some() {
        column += 2.0 + style::pill_height(style::DETAIL_SIZE);
    }
    if let Some(b) = &body {
        column += m::TIGHT + b.height + if balloon { 2.0 * BALLOON_PAD.1 } else { 0.0 };
    }
    if !fields.is_empty() {
        column += m::TIGHT + fields.iter().map(|(l, v)| l.height.max(v.height) + 1.0).sum::<f32>();
    }
    if face.progress.is_some() {
        column += m::TIGHT + BAR;
    }
    let mut height = column.max(picture);
    let image = look.image.as_ref().map(|_| {
        let (w, h) = look.image_size.unwrap_or((16, 10));
        (room.width * h as f32 / w.max(1) as f32).min(room.image_max).round()
    });
    if let Some(h) = image {
        height += GAP + h;
    }
    Laid { picture, round: look.style == NoteStyle::Message, title, badge, subtitle, body, balloon, fields, label_w, image, height: height.ceil() }
}

/// A stop sign before the first row of `title` (laid out by [`title`]
/// with `failed`) drawn with its top left at `at`, level with its
/// capitals.
pub fn paint_stop(window: &mut Window, cx: &mut App, title: &Text, at: Point<Pixels>, size: f32) {
    let middle = title.baseline(style::snap_coord(window, at.y)) - px(size * style::CAP_HEIGHT / 2.0);
    let r = Bounds::centered_at(point(at.x + px(MARK / 2.0), middle), gpui::size(px(MARK), px(MARK)));
    style::stop_mark(window, r, &theme::palette(cx));
    agent::record(window, cx, agent::Node { role: "Image", name: Some("Error".into()), bounds: r, enabled: true, ..Default::default() });
}

/// Where the body went, for finding links under the pointer.
pub struct Painted {
    pub body: Option<(Point<Pixels>, Text)>,
}

/// `img` drawn at its own size centred in `r`, from a whole device pixel.
fn centred(window: &mut Window, r: Bounds<Pixels>, img: std::sync::Arc<gpui::RenderImage>) {
    let s = window.scale_factor();
    let d = img.size(0);
    let (w, h) = (d.width.0 as f32 / s, d.height.0 as f32 / s);
    let at = style::snap_centred(window, r.center(), w, h);
    let _ = window.paint_image(at, Corners::default(), img, 0, false);
}

/// The device pixels a place takes.
pub fn device(window: &Window, r: Bounds<Pixels>) -> (u32, u32) {
    let s = window.scale_factor();
    ((f32::from(r.size.width) * s).round() as u32, (f32::from(r.size.height) * s).round() as u32)
}

/// Draw `face`, laid out as `laid`, with its top left at `at`.
#[allow(clippy::too_many_arguments)]
pub fn paint(window: &mut Window, cx: &mut App, face: &Face, laid: &Laid, ink: &Ink, at: Point<Pixels>, width: f32) -> Painted {
    let text_x = at.x + px(if laid.picture > 0.0 { laid.picture + GAP } else { 0.0 });
    let text_w = f32::from(at.x - text_x) + width;
    // The picture.
    if laid.picture > 0.0
        && let Some(src) = &face.picture
    {
        let r = style::snap(window, Bounds::new(at, size(px(laid.picture), px(laid.picture))));
        let media = face.look.style == NoteStyle::Media;
        let shape = if laid.round || media { Shape::Cover } else { Shape::Fit };
        if let Some(img) = pictures::get(cx, src, device(window, r), shape) {
            if laid.round {
                let _ = window.paint_image(r, Corners::all(r.size.width / 2.0), img, 0, false);
                window.paint_quad(gpui::outline(r, ink.balloon_edge, BorderStyle::Solid).corner_radii(r.size.width / 2.0).border_widths(style::hairline(window)));
            } else if media {
                let _ = window.paint_image(r, Corners::all(px(4.0)), img, 0, false);
            } else {
                centred(window, r, img);
            }
        }
    }
    let mut y = at.y;
    let title_at = point(text_x, y);
    laid.title.paint(window, cx, title_at);
    let title_r = Bounds::new(title_at, size(px(laid.title.width), px(laid.title.height)));
    agent::record(window, cx, agent::Node::label(face.title.to_owned(), title_r));
    if face.failed {
        paint_stop(window, cx, &laid.title, title_at, style::PLAIN_SIZE);
    }
    y += px(laid.title.height);
    // The badge and subtitle.
    if laid.badge.is_some() || laid.subtitle.is_some() {
        let h = style::pill_height(style::DETAIL_SIZE);
        let cy = y + px(2.0 + h / 2.0);
        let mut x = text_x;
        if let (Some(w), Some(b)) = (laid.badge, face.look.badge.as_deref()) {
            let fill = ink.accent.map(style::c).unwrap_or(ink.dim);
            let on = ink.accent.map_or_else(|| style::c([255, 255, 255, 255]), style::ink_on);
            let r = style::pill(window, cx, x, cy, b, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), fill, on);
            x += px(w.max(f32::from(r.size.width)) + 5.0);
        }
        if let Some(t) = &laid.subtitle {
            let top = style::snap_coord(window, cy - px(t.height / 2.0));
            t.paint(window, cx, point(x, top));
            agent::record(window, cx, agent::Node::label(t.text.clone(), Bounds::new(point(x, top), size(px(t.width), px(t.height)))));
        }
        y += px(2.0 + h);
    }
    // The body, in a balloon for a message.
    let mut painted = Painted { body: None };
    if let Some(b) = &laid.body {
        y += px(m::TIGHT);
        let origin = if laid.balloon {
            let r = style::snap(window, Bounds::new(point(text_x, y), size(px((b.width + 2.0 * BALLOON_PAD.0).min(text_w)), px(b.height + 2.0 * BALLOON_PAD.1))));
            // Its corner by the sender's picture is the speaker's.
            let corners = Corners { top_left: px(2.0), top_right: px(9.0), bottom_right: px(9.0), bottom_left: px(9.0) };
            window.paint_quad(gpui::fill(r, ink.balloon).corner_radii(corners).border_widths(style::hairline(window)).border_color(ink.balloon_edge));
            y += px(2.0 * BALLOON_PAD.1);
            r.origin + point(px(BALLOON_PAD.0), px(BALLOON_PAD.1))
        } else {
            point(text_x, y)
        };
        let origin = point(style::snap_coord(window, origin.x), style::snap_coord(window, origin.y));
        b.paint(window, cx, origin);
        let all: String = face.body.iter().map(|s| s.text.as_str()).collect();
        agent::record(window, cx, agent::Node::label(all, Bounds::new(origin, size(px(b.width), px(b.height)))));
        painted.body = Some((origin, b.clone()));
        y += px(b.height);
    }
    // The fields, as a little table.
    if !laid.fields.is_empty() {
        y += px(m::TIGHT);
        for ((l, v), (label, value)) in laid.fields.iter().zip(&face.look.fields) {
            let h = l.height.max(v.height);
            l.paint(window, cx, point(text_x + px(laid.label_w - l.width), y));
            let vr = Bounds::new(point(text_x + px(laid.label_w + 6.0), y), size(px(v.width), px(v.height)));
            v.paint(window, cx, vr.origin);
            agent::record(window, cx, agent::Node::label(format!("{label}: {value}"), vr));
            y += px(h + 1.0);
        }
    }
    if let Some(f) = face.progress {
        y += px(m::TIGHT);
        let p = theme::palette(cx);
        widgets::level_bar(window, Bounds::new(point(text_x, y), size(px(text_w), px(BAR))), f, ink.accent.map(style::c), &p);
        y += px(BAR);
    }
    // The large picture, under it all.
    if let (Some(h), Some(path)) = (laid.image, face.look.image.as_deref()) {
        let top = (at.y + px(laid.picture)).max(y) + px(GAP);
        let r = style::snap(window, Bounds::new(point(at.x, top), size(px(width), px(h))));
        match pictures::get(cx, &Source::File(path.into()), device(window, r), Shape::Cover) {
            Some(img) => {
                let _ = window.paint_image(r, Corners::all(px(4.0)), img, 0, false);
            }
            // Still loading (or gone): its place.
            None => window.paint_quad(gpui::fill(r, ink.balloon).corner_radii(px(4.0))),
        }
        // Edged, so a picture as pale (or dark) as the note doesn't run into it.
        window.paint_quad(gpui::outline(r, ink.balloon_edge, BorderStyle::Solid).corner_radii(px(4.0)).border_widths(style::hairline(window)));
        let name = path.rsplit('/').next().unwrap_or(path);
        agent::record(window, cx, agent::Node { role: "Image", name: Some(format!("Picture: {name}").into()), bounds: r, enabled: true, ..Default::default() });
    }
    painted
}
