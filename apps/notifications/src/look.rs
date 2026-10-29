//! What a notification shows, laid out the same on screen and in the
//! history's cards: its picture (square; round for a message, from
//! someone; large for something playing), the title, a line with its
//! badge and subtitle, the body (in a balloon for a message), a little
//! table of its fields, a large picture under it all, and a progress bar.
//! An error's title starts with a small stop sign, as a BAlert's would;
//! the app's colour is only in its badge, its tile and its bar.
//! [`lay`] measures (so a stack or a grid can be arranged first),
//! [`paint`] draws.

use std::sync::Arc;

use olo_kit::{
    egui::{self, Color32, CornerRadius, Galley, Pos2, Rect, TextureHandle, pos2, vec2},
    icons, metrics as m, style, widgets,
};
use olo_protocol::{NoteLook, NoteStyle};

use crate::markup::Span;

/// Between the picture and the text, and between blocks.
const GAP: f32 = 8.0;
/// Around the body inside a message's balloon.
const BALLOON_PAD: egui::Vec2 = vec2(7.0, 4.0);
/// A field's label, at most this much of the width.
const LABEL_SHARE: f32 = 0.4;
/// A progress bar's height.
const BAR: f32 = 10.0;
/// The stop sign before an error's title, and the room it takes.
const MARK: f32 = 12.0;
pub const MARK_ROOM: f32 = MARK + 4.0;

/// A note's picture: decoded already (on screen), or kept in a file (in
/// the history).
#[derive(Clone, Copy)]
pub enum Picture<'a> {
    None,
    Texture(&'a TextureHandle),
    File(&'a str),
}

/// What a note shows.
pub struct Face<'a> {
    pub title: &'a str,
    pub body: &'a [Span],
    pub look: &'a NoteLook,
    pub progress: Option<f32>,
    pub picture: Picture<'a>,
    /// It reports a failure: its title starts with a stop sign.
    pub failed: bool,
}

/// Colours for a face on its background.
pub struct Ink {
    pub text: Color32,
    /// The body.
    pub soft: Color32,
    /// Labels, the subtitle.
    pub dim: Color32,
    pub link: Color32,
    /// A message's balloon, and its edge.
    pub balloon: Color32,
    pub balloon_edge: Color32,
    /// The app's colour (for its badge).
    pub accent: Option<Color32>,
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

/// A face measured for its room.
pub struct Laid {
    picture: f32,
    round: bool,
    title: Arc<Galley>,
    badge: Option<f32>,
    subtitle: Option<Arc<Galley>>,
    body: Option<Arc<Galley>>,
    balloon: bool,
    fields: Vec<(Arc<Galley>, Arc<Galley>)>,
    label_w: f32,
    image: Option<f32>,
    pub height: f32,
}

/// Where the body went, for finding links under the pointer.
pub struct Painted {
    pub body: Option<(Pos2, Arc<Galley>)>,
}

/// Text in `font`, wrapped to `width` and at most `rows` lines.
fn text(ui: &egui::Ui, s: &str, font: egui::FontId, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    led(ui, s, 0.0, font, color, width, rows)
}

/// Text as [`text`], its first line `lead` in (for a mark before it).
pub fn led(ui: &egui::Ui, s: &str, lead: f32, font: egui::FontId, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(s, lead, egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping { max_width: width.max(10.0), max_rows: rows.max(1), break_anywhere: false, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The body with its markup (bold, italics, underlines, links).
pub fn body(ui: &egui::Ui, spans: &[Span], ink: &Ink, width: f32, rows: usize) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::default();
    for s in spans {
        let font = match (s.bold, s.italic) {
            (true, _) => style::bold_font(),
            (false, true) => style::italic_font(),
            _ => style::plain_font(),
        };
        let mut f = egui::TextFormat::simple(font, if s.link.is_some() { ink.link } else { ink.soft });
        // (There is no bold italic face: slanted bold.)
        f.italics = s.bold && s.italic;
        if s.underline || s.link.is_some() {
            f.underline = egui::Stroke::new(1.0, f.color);
        }
        job.append(&s.text, 0.0, f);
    }
    job.wrap = egui::text::TextWrapping { max_width: width.max(10.0), max_rows: rows.max(1), break_anywhere: false, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// Measure `face` for `room`.
pub fn lay(ui: &egui::Ui, face: &Face, ink: &Ink, room: &Room) -> Laid {
    let look = face.look;
    let has_picture = !matches!(face.picture, Picture::None);
    let picture = match (has_picture, look.style) {
        (false, _) => 0.0,
        (true, NoteStyle::Message) => room.picture.max(32.0),
        (true, NoteStyle::Media) => 48.0,
        (true, NoteStyle::Standard) => room.picture,
    };
    let text_x = if picture > 0.0 { picture + GAP } else { 0.0 };
    let text_w = room.width - text_x;
    let lead = if face.failed { MARK_ROOM } else { 0.0 };
    let title = led(ui, face.title, lead, style::bold_font(), ink.text, text_w - room.title_reserve, room.title_rows);
    let badge = look.badge.as_deref().filter(|b| !b.trim().is_empty()).map(|b| style::pill_width(ui.ctx(), b, style::small_font()).min(text_w));
    let subtitle_w = text_w - badge.map_or(0.0, |w| w + 5.0);
    let subtitle = look.subtitle.as_deref().filter(|s| !s.trim().is_empty()).map(|s| text(ui, s, style::small_font(), ink.dim, subtitle_w, 1));
    let has_body = face.body.iter().any(|s| !s.text.trim().is_empty());
    let balloon = has_body && look.style == NoteStyle::Message;
    let body_w = if balloon { text_w - 2.0 * BALLOON_PAD.x } else { text_w };
    let body = has_body.then(|| body(ui, face.body, ink, body_w, room.body_rows));
    let fields: Vec<(String, String)> = look.fields.iter().take(room.fields_max).cloned().collect();
    let label_w = fields.iter().map(|(l, _)| style::label_width(ui.ctx(), l, style::small_font())).fold(0.0, f32::max).min(text_w * LABEL_SHARE).ceil();
    let fields: Vec<(Arc<Galley>, Arc<Galley>)> = fields
        .iter()
        .map(|(l, v)| (text(ui, l, style::small_font(), ink.dim, label_w, 1), text(ui, v, style::small_font(), ink.text, text_w - label_w - 6.0, 2)))
        .collect();
    // The column beside the picture.
    let mut column = title.size().y;
    if badge.is_some() || subtitle.is_some() {
        column += 2.0 + style::pill_height(&style::small_font());
    }
    if let Some(b) = &body {
        column += m::TIGHT + b.size().y + if balloon { 2.0 * BALLOON_PAD.y } else { 0.0 };
    }
    if !fields.is_empty() {
        column += m::TIGHT + fields.iter().map(|(l, v)| l.size().y.max(v.size().y) + 1.0).sum::<f32>();
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

/// A stop sign before the first line of `title` (laid out with [`led`],
/// [`MARK_ROOM`] in) drawn at `at`, level with its capitals.
pub fn paint_stop(painter: &egui::Painter, title: &Galley, at: Pos2) {
    let middle = at.y + style::cap_middle(title, style::PLAIN_SIZE);
    let r = Rect::from_center_size(pos2(at.x + MARK / 2.0, middle), vec2(MARK, MARK));
    style::stop_mark(painter, r, &style::palette(painter.ctx()));
    style::describe_text(painter.ctx(), egui::accesskit::Role::Image, "Error", r, None);
}

/// Draw `face`, laid out as `laid`, with its top left at `at`.
pub fn paint(ui: &egui::Ui, painter: &egui::Painter, face: &Face, laid: &Laid, ink: &Ink, at: Pos2, width: f32) -> Painted {
    let ctx = ui.ctx();
    let text_x = at.x + if laid.picture > 0.0 { laid.picture + GAP } else { 0.0 };
    let text_w = at.x + width - text_x;
    // The picture.
    if laid.picture > 0.0 {
        let r = Rect::from_min_size(at, vec2(laid.picture, laid.picture));
        let tex = match face.picture {
            Picture::Texture(t) => Some(t.clone()),
            Picture::File(path) => icons::thumbnail(ctx, std::path::Path::new(path), laid.picture * 2.0),
            Picture::None => None,
        };
        if let Some(tex) = tex {
            if laid.round {
                icons::paint_round(painter, r, &tex, ink.balloon_edge);
            } else if face.look.style == NoteStyle::Media {
                icons::paint_cover(painter, r, &tex, 4.0);
            } else {
                let fit = icons::fit(ctx, &tex, r);
                painter.image(tex.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
    }
    let mut y = at.y;
    let title_r = Rect::from_min_size(pos2(text_x, y), laid.title.size());
    style::describe_text(ctx, egui::accesskit::Role::Label, face.title, title_r, None);
    painter.galley(title_r.min, laid.title.clone(), ink.text);
    if face.failed {
        paint_stop(painter, &laid.title, title_r.min);
    }
    y += laid.title.size().y;
    // The badge and subtitle.
    if laid.badge.is_some() || laid.subtitle.is_some() {
        let h = style::pill_height(&style::small_font());
        let cy = y + 2.0 + h / 2.0;
        let mut x = text_x;
        if let (Some(w), Some(b)) = (laid.badge, face.look.badge.as_deref()) {
            let fill = ink.accent.unwrap_or(ink.dim);
            let r = style::pill(painter, x, cy, b, style::small_font(), fill, style::ink_on(fill));
            x += w.max(r.width()) + 5.0;
        }
        if let Some(g) = &laid.subtitle {
            let pos = style::text_origin(ctx, g, x, cy, style::small_font().size);
            style::describe_text(ctx, egui::accesskit::Role::Label, g.text(), Rect::from_min_size(pos, g.size()), None);
            painter.galley(pos, g.clone(), ink.dim);
        }
        y += 2.0 + h;
    }
    // The body, in a balloon for a message.
    let mut painted = Painted { body: None };
    if let Some(b) = &laid.body {
        y += m::TIGHT;
        let origin = if laid.balloon {
            let r = Rect::from_min_size(pos2(text_x, y), vec2((b.size().x + 2.0 * BALLOON_PAD.x).min(text_w), b.size().y + 2.0 * BALLOON_PAD.y));
            // Its corner by the sender's picture is the speaker's.
            let corners = CornerRadius { nw: 2, ne: 9, sw: 9, se: 9 };
            painter.rect(style::snap(ctx, r), corners, ink.balloon, egui::Stroke::new(1.0, ink.balloon_edge), egui::StrokeKind::Inside);
            y += 2.0 * BALLOON_PAD.y;
            r.min + BALLOON_PAD
        } else {
            pos2(text_x, y)
        };
        let text: String = face.body.iter().map(|s| s.text.as_str()).collect();
        style::describe_text(ctx, egui::accesskit::Role::Label, &text, Rect::from_min_size(origin, b.size()), None);
        painter.galley(origin, b.clone(), ink.soft);
        painted.body = Some((origin, b.clone()));
        y += b.size().y;
    }
    // The fields, as a little table.
    if !laid.fields.is_empty() {
        y += m::TIGHT;
        for ((l, v), (label, value)) in laid.fields.iter().zip(&face.look.fields) {
            let h = l.size().y.max(v.size().y);
            painter.galley(pos2(text_x + laid.label_w - l.size().x, y), l.clone(), ink.dim);
            let vr = Rect::from_min_size(pos2(text_x + laid.label_w + 6.0, y), v.size());
            style::describe_text(ctx, egui::accesskit::Role::Label, &format!("{label}: {value}"), vr, None);
            painter.galley(vr.min, v.clone(), ink.text);
            y += h + 1.0;
        }
    }
    if let Some(f) = face.progress {
        y += m::TIGHT;
        widgets::level_bar(painter, Rect::from_min_size(pos2(text_x, y), vec2(text_w, BAR)), f, ink.accent);
        y += BAR;
    }
    // The large picture, under it all.
    if let (Some(h), Some(path)) = (laid.image, face.look.image.as_deref()) {
        let top = (at.y + laid.picture).max(y) + GAP;
        let r = Rect::from_min_size(pos2(at.x, top), vec2(width, h));
        match icons::thumbnail(ctx, std::path::Path::new(path), width.max(h) * 2.0) {
            Some(tex) => icons::paint_cover(painter, r, &tex, 4.0),
            // Still loading (or gone): its place.
            None => {
                painter.rect_filled(style::snap(ctx, r), 4.0, ink.balloon);
            }
        }
        // Edged, so a picture as pale (or dark) as the note doesn't run into it.
        painter.rect_stroke(style::snap(ctx, r), 4.0, egui::Stroke::new(1.0, ink.balloon_edge), egui::StrokeKind::Inside);
        style::describe_text(ctx, egui::accesskit::Role::Image, &format!("Picture: {}", path.rsplit('/').next().unwrap_or(path)), r, None);
    }
    painted
}
