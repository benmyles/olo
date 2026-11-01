//! A message, laid out and painted: a balloon (ours on the right in a
//! tint of the keyboard's blue, theirs on the left on the card colour),
//! with who sent it in a group, what it forwards or answers, its picture
//! or file, its text (bold, italic, code, links, spoilers; emoji in
//! colour, as Olo's GPUI draws them), its time (edited, read ticks,
//! views) and reactions under it. Stickers and a few emoji alone stand on
//! the background with no balloon; service messages are a pill in the
//! middle. Laying out is done once per message and width (`Laid`, cached
//! by the view); painting reads it.

use std::sync::Arc;

use olo_tg::{EntityKind, Media, Message, Pending};
use olo_ui::{
    gpui::{App, BorderStyle, Bounds, Corners, Font, FontStyle, FontWeight, Hsla, PathBuilder, Pixels, Point, RenderImage, ShapedLine, SharedString, StrikethroughStyle, TextAlign, TextRun, UnderlineStyle, Window, WrappedLine, hsla, point, px, quad, rgb, size, transparent_black, white},
    style::{self, Align},
};

/// Message text's size.
pub const TEXT: f32 = 13.0;
/// The time and other small print.
pub const META: f32 = 10.5;
/// Inside a balloon.
pub const PAD: (f32, f32) = (10.0, 6.0);
pub const RADIUS: f32 = 12.0;
/// A corner where the balloon joins the next of the same sender's.
pub const JOINED: f32 = 4.0;
/// The widest a balloon gets.
pub const MAX_WIDTH: f32 = 460.0;
/// A sender's picture beside their balloons in a group.
pub const AVATAR: f32 = 32.0;
/// A sticker's box.
pub const STICKER: f32 = 168.0;
const REACTION_H: f32 = 24.0;

/// A rectangle in points (laid out from a message's top left).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn at(&self, o: Point<Pixels>) -> Bounds<Pixels> {
        Bounds { origin: point(o.x + px(self.x), o.y + px(self.y)), size: size(px(self.w), px(self.h)) }
    }

    fn shift(self, dx: f32) -> R {
        R { x: self.x + dx, ..self }
    }
}

/// The colours of a conversation.
#[derive(Clone)]
pub struct Look {
    pub ground: Hsla,
    pub theirs: Hsla,
    pub theirs_edge: Hsla,
    pub ours: Hsla,
    pub ours_edge: Hsla,
    pub text: Hsla,
    pub dim: Hsla,
    pub link: Hsla,
    pub accent: Hsla,
    pub pill: Hsla,
    pub pill_text: Hsla,
    pub dark: bool,
}

impl Look {
    pub fn of(p: &olo_ui::palette::Palette) -> Look {
        let ink = style::card_ink(p);
        let dark = p.mode == olo_ui::palette::ThemeMode::Dark;
        let (_, card) = style::card_paper(p);
        let ours = olo_ui::palette::mix(card, p.keyboard_navigation, if dark { 0.30 } else { 0.15 });
        let link = olo_ui::palette::mix(p.keyboard_navigation, p.document_text, if dark { 0.0 } else { 0.15 });
        Look {
            ground: ink.gutter,
            theirs: ink.card,
            theirs_edge: ink.edge,
            ours: style::c(ours),
            ours_edge: style::c(olo_ui::palette::mix(ours, p.keyboard_navigation, 0.35)),
            text: ink.text,
            dim: ink.dim,
            link: style::c(link),
            accent: style::c(p.keyboard_navigation),
            pill: style::c(olo_ui::palette::mix(p.document_text, p.document_background, if dark { 0.75 } else { 0.62 })).opacity(0.85),
            pill_text: if dark { style::c([235, 235, 235, 255]) } else { white() },
            dark,
        }
    }
}

/// Telegram's seven colours for people and chats (names in groups,
/// pictures that are initials): bright for fills, and a readable ink.
pub fn peer_color(id: i64, dark: bool) -> (Hsla, Hsla) {
    const FILL: [u32; 7] = [0xFF845E, 0xFEBB5B, 0xB694F9, 0x9AD164, 0x5BCBE3, 0x5CAFFA, 0xFF8AAC];
    const INK_LIGHT: [u32; 7] = [0xCC5049, 0xD67722, 0x955CDB, 0x40A920, 0x309EBA, 0x368AD1, 0xC7508B];
    const INK_DARK: [u32; 7] = [0xFF8E86, 0xFFA357, 0xB18FFF, 0x7CD65F, 0x45D8E8, 0x7AC9FF, 0xFF7FD5];
    let i = (id.unsigned_abs() % 7) as usize;
    (rgb(FILL[i]).into(), rgb(if dark { INK_DARK[i] } else { INK_LIGHT[i] }).into())
}

/// A person's or chat's picture: their photo when it's here, else their
/// initials on their colour, round.
pub fn avatar(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, id: i64, name: &str, img: Option<&Arc<RenderImage>>, dark: bool) {
    let r = style::snap(window, r);
    let radius = r.size.width.min(r.size.height) / 2.0;
    let c = r.center();
    if let Some(img) = img {
        let _ = window.paint_image(r, Corners::all(radius), img.clone(), 0, false);
        let hair = style::hairline(window);
        window.paint_sector(c, radius - hair, radius, 0.0, std::f32::consts::TAU, hsla(0.0, 0.0, 0.0, 30.0 / 255.0));
        return;
    }
    let (fill, _) = peer_color(id, dark);
    window.paint_sector(c, px(0.0), radius, 0.0, std::f32::consts::TAU, fill);
    // Lit from above: a lighter top half of the same colour.
    window.paint_sector(c, px(0.0), radius, 1.5 * std::f32::consts::PI, std::f32::consts::PI, hsla(0.0, 0.0, 1.0, 28.0 / 255.0));
    let letters = initials(name);
    let h = f32::from(r.size.height);
    let size = (h * if letters.chars().count() > 1 { 0.38 } else { 0.44 }).round();
    style::label(window, cx, c.x, c.y, Align::Center, letters, size, style::ui_font(FontWeight::BOLD), white());
}

/// "MO" for "Mira Okafor", "D" for "Design".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().filter(|w| w.chars().next().is_some_and(char::is_alphanumeric)).collect();
    let first = |w: &str| w.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    match words.as_slice() {
        [] => name.chars().find(|c| !c.is_whitespace()).map(|c| c.to_string()).unwrap_or_default(),
        [one] => first(one),
        [a, .., b] => format!("{}{}", first(a), first(b)),
    }
}

/// How a message sits with its neighbours.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Place {
    /// The first of a run from one sender (shows their name).
    pub first: bool,
    /// The last of the run (shows their picture; full corners).
    pub last: bool,
    /// In a group or forum: names and pictures shown.
    pub many: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Bubble,
    /// A sticker or emoji alone: no balloon.
    Bare,
    Service,
}

/// What a part of a message does when clicked.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Reply(i32),
    Media,
    Reaction(String),
    Link(String),
    Mention(String),
    MentionId(i64),
    Hashtag(String),
    Spoiler,
    Sender(i64),
}

/// A line of text, shaped once: painted with its top at a point.
#[derive(Clone)]
pub struct Line {
    pub shaped: ShapedLine,
    pub w: f32,
    pub h: f32,
}

impl Line {
    /// From the top to the baseline.
    pub fn baseline(&self) -> f32 {
        let (a, d) = (f32::from(self.shaped.ascent), f32::from(self.shaped.descent));
        (self.h - a - d) / 2.0 + a
    }

    pub fn paint(&self, window: &mut Window, cx: &mut App, at: Point<Pixels>) {
        let _ = self.shaped.paint(at, px(self.h), window, cx);
    }
}

/// The height of a line of `size` text.
pub fn line_height(size: f32) -> f32 {
    (size * style::LINE_HEIGHT).ceil()
}

pub fn font(weight: FontWeight) -> Font {
    style::ui_font(weight)
}

pub fn italic() -> Font {
    Font { style: FontStyle::Italic, ..style::ui_font(FontWeight::NORMAL) }
}

/// `text` on one line, cut short with "…" past `max_w`.
pub fn line(window: &mut Window, text: &str, size: f32, font: Font, color: Hsla, max_w: Option<f32>) -> Line {
    let text: SharedString = text.replace('\n', " ").into();
    let text = match max_w {
        Some(w) => style::fit(window, text, size, &font, px(w.max(10.0))),
        None => text,
    };
    let run = TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None, size: None };
    let shaped = window.text_system().shape_line(text, px(size), &[run], None);
    Line { w: f32::from(shaped.width), h: line_height(size), shaped }
}

/// Text wrapped to a width, paragraph by paragraph.
#[derive(Clone)]
pub struct Para {
    pub lines: Vec<WrappedLine>,
    pub lh: f32,
    pub w: f32,
    pub h: f32,
    pub align: TextAlign,
}

/// Where each line of a wrapped paragraph starts (bytes in it).
fn row_starts(p: &WrappedLine) -> Vec<usize> {
    let layout = &p.unwrapped_layout;
    std::iter::once(0).chain(p.wrap_boundaries().iter().map(|b| layout.runs[b.run_ix].glyphs[b.glyph_ix].index)).collect()
}

impl Para {
    pub fn new(window: &mut Window, text: &str, size: f32, runs: &[TextRun], width: f32, lines_at_most: Option<usize>) -> Para {
        let lh = line_height(size);
        let lines: Vec<WrappedLine> = window.text_system().shape_text(SharedString::from(text.to_owned()), px(size), runs, Some(px(width)), lines_at_most).map(|l| l.into_vec()).unwrap_or_default();
        // As wide as its widest line (not the width it was wrapped to).
        let mut w: f32 = 0.0;
        let mut rows = 0;
        for p in &lines {
            let starts = row_starts(p);
            for (r, &s) in starts.iter().enumerate() {
                let e = starts.get(r + 1).copied().unwrap_or(p.len());
                let x0 = p.unwrapped_layout.x_for_index(s);
                // (A wrap's trailing space isn't the line's.)
                let end = if r + 1 < starts.len() { p.text[..e].trim_end().len().max(s) } else { e };
                w = w.max(f32::from(p.unwrapped_layout.x_for_index(end) - x0));
                rows += 1;
            }
        }
        Para { lines, lh, w: w.ceil(), h: lh * rows.max(1) as f32, align: TextAlign::Left }
    }

    /// One run of `color` over all of `text`.
    pub fn plain(window: &mut Window, text: &str, size: f32, font: Font, color: Hsla, width: f32, lines_at_most: Option<usize>) -> Para {
        let run = TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None, size: None };
        Para::new(window, text, size, &[run], width, lines_at_most)
    }

    fn tops(&self) -> Vec<(usize, f32)> {
        let (mut start, mut y) = (0, 0.0);
        self.lines
            .iter()
            .map(|p| {
                let out = (start, y);
                start += p.len() + 1;
                y += self.lh * (p.wrap_boundaries().len() + 1) as f32;
                out
            })
            .collect()
    }

    pub fn paint(&self, window: &mut Window, cx: &mut App, at: Point<Pixels>) {
        let width = px(self.w);
        for (p, (_, y)) in self.lines.iter().zip(self.tops()) {
            let origin = point(at.x, at.y + px(y));
            let bounds = (self.align != TextAlign::Left).then(|| Bounds { origin, size: size(width, px(self.lh)) });
            // (Code's and a hidden spoiler's grounds, under the glyphs.)
            let _ = p.paint_background(origin, px(self.lh), self.align, bounds, window, cx);
            let _ = p.paint(origin, px(self.lh), self.align, bounds, window, cx);
        }
    }

    /// The byte of the text under `q` (from the text's top left), if any.
    pub fn index_at(&self, q: Point<Pixels>) -> Option<usize> {
        for (p, (start, y)) in self.lines.iter().zip(self.tops()) {
            let h = self.lh * (p.wrap_boundaries().len() + 1) as f32;
            let local = point(q.x, q.y - px(y));
            if f32::from(local.y) >= 0.0 && f32::from(local.y) < h {
                return p.index_for_position(local, px(self.lh)).ok().map(|i| start + i);
            }
        }
        None
    }

    /// Where its last line ends, and that line's baseline (from the top).
    pub fn last_line(&self) -> (f32, f32) {
        let Some(p) = self.lines.last() else { return (0.0, self.h * 0.8) };
        let starts = row_starts(p);
        let x0 = p.unwrapped_layout.x_for_index(*starts.last().unwrap_or(&0));
        let right = f32::from(p.unwrapped_layout.x_for_index(p.len()) - x0);
        let (a, d) = (f32::from(p.unwrapped_layout.ascent), f32::from(p.unwrapped_layout.descent));
        (right, self.h - self.lh + (self.lh - a - d) / 2.0 + a)
    }

    /// The span of lines (top and bottom) each byte range covers.
    fn rows_of(&self, range: std::ops::Range<usize>) -> Option<(f32, f32)> {
        let mut span: Option<(f32, f32)> = None;
        for (p, (start, y)) in self.lines.iter().zip(self.tops()) {
            let starts = row_starts(p);
            for (r, &s) in starts.iter().enumerate() {
                let e = starts.get(r + 1).copied().unwrap_or(p.len() + 1);
                if start + s < range.end && start + e > range.start {
                    let top = y + self.lh * r as f32;
                    span = Some(span.map_or((top, top + self.lh), |(a, b)| (a.min(top), b.max(top + self.lh))));
                }
            }
        }
        span
    }
}

/// A message laid out at a width.
pub struct Laid {
    pub shape: Shape,
    /// Its whole height in the list (without the gap after it).
    pub height: f32,
    /// The balloon (or the bare thing), from the column's left.
    pub bubble: R,
    pub name: Option<(Line, (f32, f32))>,
    pub forward: Option<(Line, (f32, f32))>,
    /// The quote: its box, the name and the text.
    pub reply: Option<(R, Line, Line)>,
    pub media: Option<R>,
    /// File, voice and audio rows: their title and detail.
    pub media_text: Option<(Line, Line)>,
    /// A web page's preview: its bar and the site, title, description.
    pub page: Option<(R, Vec<Placed>)>,
    /// A poll: the question and kind, then each answer, its share and box.
    pub poll: Option<Poll>,
    pub text: Option<(Para, (f32, f32))>,
    pub meta: (Line, (f32, f32)),
    /// The time sits on the picture (no text under it).
    pub meta_over: bool,
    pub reactions: Vec<(R, Line, Line, String, bool)>,
    pub big_emoji: Option<(Line, (f32, f32))>,
    pub avatar: bool,
}

/// A paragraph and where it goes in the bubble.
pub type Placed = (Para, (f32, f32));

/// A poll laid out: the question, its kind and votes, where it heads the
/// bubble, then each answer (its text, its share as text, the share, its
/// row).
pub type Poll = (Para, Line, R, Vec<(Line, Line, f32, R)>);

/// The text with its formatting, as runs over it.
pub fn text_runs(m: &Message, look: &Look, revealed: bool) -> Vec<TextRun> {
    let text = &m.text;
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for e in &m.entities {
        cuts.push(e.start.min(text.len()));
        cuts.push(e.end.min(text.len()));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            continue;
        }
        let mut run = TextRun { len: b - a, font: font(FontWeight::NORMAL), color: look.text, background_color: None, underline: None, strikethrough: None, size: None };
        let mut bold = false;
        for e in m.entities.iter().filter(|e| e.start <= a && e.end >= b) {
            match &e.kind {
                EntityKind::Bold => bold = true,
                EntityKind::Italic | EntityKind::Quote => run.font = italic(),
                EntityKind::Underline => run.underline = Some(UnderlineStyle { color: Some(look.text), thickness: px(1.0), wavy: false }),
                EntityKind::Strike => run.strikethrough = Some(StrikethroughStyle { color: Some(look.text), thickness: px(1.0) }),
                EntityKind::Code => {
                    run.font = style::mono_font(FontWeight::NORMAL);
                    run.background_color = Some(look.text.opacity(0.07));
                }
                // A block: one box behind its rows (`pre_boxes`).
                EntityKind::Pre(_) => run.font = style::mono_font(FontWeight::NORMAL),
                EntityKind::Spoiler if !revealed => {
                    run.color = transparent_black();
                    run.background_color = Some(look.dim);
                }
                k if k.is_link() => run.color = look.link,
                _ => {}
            }
        }
        if bold && run.font.family == font(FontWeight::NORMAL).family {
            run.font.weight = FontWeight::BOLD;
        }
        runs.push(run);
    }
    // (Runs cover every byte, joined where nothing changes.)
    let covered: usize = runs.iter().map(|r| r.len).sum();
    if covered < text.len() {
        runs.push(TextRun { len: text.len() - covered, font: font(FontWeight::NORMAL), color: look.text, background_color: None, underline: None, strikethrough: None, size: None });
    }
    runs
}

/// The time, "edited", views: as it reads at the bottom of a balloon.
fn meta_text(m: &Message, time: &str) -> String {
    let mut s = String::new();
    if let Some(v) = m.views {
        s.push_str(&short_count(v));
        s.push_str("  ");
    }
    if m.edited.is_some() {
        s.push_str("edited ");
    }
    s.push_str(time);
    s
}

/// "9.3K", "1.2M".
pub fn short_count(n: u32) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..10_000 => format!("{:.1}K", n as f32 / 1000.0).replace(".0K", "K"),
        10_000..1_000_000 => format!("{}K", n / 1000),
        _ => format!("{:.1}M", n as f32 / 1e6).replace(".0M", "M"),
    }
}

/// Room after the time for the read ticks (ours).
const TICKS: f32 = 16.0;

/// A message alone of one to three emoji, shown large.
fn big_emoji(m: &Message) -> bool {
    let t = m.text.trim();
    if m.media.is_some() || t.is_empty() || t.chars().count() > 24 {
        return false;
    }
    let found = olo_emoji::clusters(t);
    let covered: usize = found.iter().map(|r| r.len()).sum();
    !found.is_empty() && found.len() <= 3 && covered == t.chars().filter(|c| !c.is_whitespace()).map(char::len_utf8).sum::<usize>()
}

/// Lay out `m` for a column `col` wide.
#[allow(clippy::too_many_arguments)]
pub fn lay(window: &mut Window, m: &Message, place: Place, look: &Look, col: f32, time: &str, sender_name: Option<&str>, revealed: bool) -> Laid {
    let normal = font(FontWeight::NORMAL);
    let bold = font(FontWeight::BOLD);
    let ours = m.out;
    let left_room = if place.many && !ours { AVATAR + 8.0 } else { 0.0 };
    let max_w = (col - left_room - 48.0).clamp(120.0, MAX_WIDTH);
    let meta_g = line(window, &meta_text(m, time), META, normal.clone(), look.dim, None);
    let meta_w = meta_g.w + if ours { TICKS + 2.0 } else { 0.0 };
    let empty = |meta_g: Line| Laid {
        shape: Shape::Service,
        height: 0.0,
        bubble: R::default(),
        name: None,
        forward: None,
        reply: None,
        media: None,
        media_text: None,
        page: None,
        poll: None,
        text: None,
        meta: (meta_g, (0.0, 0.0)),
        meta_over: false,
        reactions: Vec::new(),
        big_emoji: None,
        avatar: false,
    };

    // A service message: a pill in the middle.
    if let Some(s) = &m.service {
        let mut g = Para::plain(window, s, META + 0.5, normal.clone(), look.pill_text, (col - 80.0).max(100.0), None);
        g.align = TextAlign::Center;
        let (w, h) = (g.w + 20.0, g.h + 8.0);
        let r = R::new(col / 2.0 - w / 2.0, 0.0, w, h);
        return Laid { shape: Shape::Service, height: h, bubble: r, text: Some((g, (r.x + 10.0, 4.0))), ..empty(meta_g) };
    }

    let bare_sticker = matches!(m.media, Some(Media::Sticker { .. }) | Some(Media::Dice { .. })) && m.text.is_empty() && m.reply_to.is_none() && m.forwarded.is_none();
    if bare_sticker || big_emoji(m) {
        let (content, big) = if bare_sticker {
            let (w, h) = match m.media {
                Some(Media::Sticker { w, h, .. }) => (w.max(1) as f32, h.max(1) as f32),
                _ => (1.0, 1.0),
            };
            let k = (STICKER / w).min(STICKER / h);
            ((w * k, h * k), None)
        } else {
            let n = olo_emoji::clusters(m.text.trim()).len();
            let size = [48.0, 40.0, 32.0][n.clamp(1, 3) - 1];
            let g = line(window, m.text.trim(), size, normal.clone(), look.text, None);
            ((g.w, g.h), Some(g))
        };
        let meta_size = (meta_g.w + 12.0 + if ours { TICKS } else { 0.0 }, meta_g.h + 4.0);
        let h = content.1 + 4.0 + meta_size.1;
        let bw = content.0.max(meta_size.0);
        let x = if ours { col - bw } else { left_room };
        let bubble = R::new(x, 0.0, bw, h);
        let media = R::new(if ours { bubble.right() - content.0 } else { x }, 0.0, content.0, content.1);
        let meta_pos = (if ours { bubble.right() - meta_size.0 } else { x }, content.1 + 4.0);
        let reactions = lay_reactions(window, m, look, bubble.x, h + 4.0, MAX_WIDTH);
        let height = reactions.last().map_or(h, |r| r.0.bottom());
        return Laid {
            shape: Shape::Bare,
            height,
            bubble,
            media: big.is_none().then_some(media),
            meta: (meta_g, meta_pos),
            meta_over: true,
            reactions,
            big_emoji: big.map(|g| (g, (media.x, media.y))),
            avatar: place.many && !ours && place.last,
            ..empty(line(window, "", META, normal.clone(), look.dim, None))
        };
    }

    // A balloon: its parts top to bottom, at widths up to `inner`.
    let (pad_x, pad_y) = PAD;
    let inner = max_w - 2.0 * pad_x;
    let mut y = pad_y;
    let mut width: f32 = 0.0;
    let name = (place.many && !ours && place.first)
        .then(|| {
            sender_name.map(|n| {
                let ink = peer_color(m.from.as_ref().map_or(0, |f| f.id), look.dark).1;
                line(window, n, META + 1.0, bold.clone(), ink, Some(inner))
            })
        })
        .flatten()
        .map(|g| {
            let p = (pad_x, y);
            y += g.h + 2.0;
            width = width.max(g.w);
            (g, p)
        });
    let forward = m.forwarded.as_ref().map(|f| {
        let g = line(window, &format!("Forwarded from {f}"), META + 0.5, bold.clone(), look.link, Some(inner));
        let p = (pad_x, y);
        y += g.h + 3.0;
        width = width.max(g.w);
        (g, p)
    });
    let reply = m.reply_to.as_ref().map(|r| {
        let who = r.from.clone().unwrap_or_else(|| "Message".into());
        let ng = line(window, &who, META + 0.5, bold.clone(), look.link, Some(inner - 12.0));
        let tg = line(window, r.text.as_deref().unwrap_or("…"), META + 1.0, normal.clone(), look.text, Some(inner - 12.0));
        let w = ng.w.max(tg.w) + 14.0;
        let h = ng.h + tg.h + 8.0;
        let rect = R::new(pad_x, y, w, h);
        y += h + 5.0;
        width = width.max(w);
        (rect, ng, tg)
    });

    // Media.
    let mut media = None;
    let mut media_text = None;
    let mut page = None;
    let mut poll = None;
    let mut photo_top = false;
    match &m.media {
        Some(Media::Photo { w, h, .. }) | Some(Media::Video { w, h, .. }) => {
            let (w, h) = ((*w).max(1) as f32, (*h).max(1) as f32);
            let round = matches!(m.media, Some(Media::Video { round: true, .. }));
            let most = if round { (220.0, 220.0) } else { (max_w.min(400.0), 360.0) };
            let k = (most.0 / w).min(most.1 / h);
            let sz = ((w * k).max(140.0).min(most.0), (h * k).max(80.0).min(most.1));
            // A picture opens the balloon edge to edge when it leads.
            if name.is_none() && forward.is_none() && reply.is_none() {
                photo_top = true;
                media = Some(R::new(0.0, 0.0, sz.0, sz.1));
                y = sz.1 + pad_y;
            } else {
                media = Some(R::new(pad_x, y, sz.0, sz.1));
                y += sz.1 + pad_y;
            }
            width = width.max(sz.0 - if photo_top { 2.0 * pad_x } else { 0.0 });
        }
        Some(Media::Voice { duration, .. }) => {
            let r = R::new(pad_x, y, inner.min(240.0), 40.0);
            media_text = Some((line(window, "", META, normal.clone(), look.text, None), line(window, &olo_tg::clock(*duration), META, normal.clone(), look.dim, None)));
            media = Some(r);
            y += r.h + 4.0;
            width = width.max(r.w);
        }
        Some(Media::Audio { size: bytes, .. }) | Some(Media::File { size: bytes, .. }) => {
            let label = m.media.as_ref().map(Media::label).unwrap_or_default();
            let detail = match &m.media {
                Some(Media::Audio { duration, .. }) => format!("{} · {}", olo_tg::clock(*duration), olo_tg::bytes(*bytes)),
                _ => olo_tg::bytes(*bytes),
            };
            let tg = line(window, &label, TEXT - 0.5, bold.clone(), look.text, Some(inner - 52.0));
            let dg = line(window, &detail, META, normal.clone(), look.dim, Some(inner - 52.0));
            let w = 52.0 + tg.w.max(dg.w);
            let r = R::new(pad_x, y, w.min(inner), 42.0);
            media_text = Some((tg, dg));
            media = Some(r);
            y += r.h + 4.0;
            width = width.max(r.w);
        }
        Some(Media::WebPage { site, title, description, .. }) => {
            // After the text: laid out below, once the text is.
            page = Some((site.clone(), title.clone(), description.clone()));
        }
        Some(Media::Poll { question, answers, voters, closed, quiz }) => {
            let qg = Para::plain(window, question, TEXT, bold.clone(), look.text, inner, None);
            let total: u32 = answers.iter().map(|a| a.1).sum::<u32>().max(1);
            let kind = format!("{}{} · {} votes", if *closed { "Final results · " } else { "" }, if *quiz { "Quiz" } else { "Poll" }, voters);
            let kg = line(window, &kind, META, normal.clone(), look.dim, Some(inner));
            let head = R::new(pad_x, y, inner, qg.h);
            let mut py = y + qg.h + 4.0 + kg.h + 6.0;
            let mut rows = Vec::new();
            for (text, votes) in answers {
                let share = *votes as f32 / total as f32;
                let tg = line(window, text, TEXT - 0.5, normal.clone(), look.text, Some(inner - 44.0));
                let pg = line(window, &format!("{:.0}%", share * 100.0), META + 0.5, bold.clone(), look.text, Some(40.0));
                let row = R::new(pad_x, py, inner.min(300.0), tg.h + 10.0);
                py = row.bottom() + 4.0;
                rows.push((tg, pg, share, row));
            }
            y = py + 2.0;
            width = width.max(inner.min(300.0));
            poll = Some((qg, kg, head, rows));
        }
        Some(other) if !matches!(other, Media::Sticker { .. }) => {
            let (label, detail) = match other {
                Media::Location { title, address, lat, lon } => (format!("📍 {}", title.clone().unwrap_or_else(|| "Location".into())), address.clone().unwrap_or_else(|| format!("{lat:.5}, {lon:.5}"))),
                Media::Contact { name, phone } => (format!("👤 {name}"), phone.clone()),
                Media::Dice { emoji, value } => (emoji.clone(), format!("rolled {value}")),
                other => (other.label(), String::new()),
            };
            let tg = line(window, &label, TEXT - 0.5, bold.clone(), look.text, Some(inner));
            let dg = line(window, &detail, META, normal.clone(), look.dim, Some(inner));
            let r = R::new(pad_x, y, tg.w.max(dg.w).min(inner), tg.h + dg.h + 2.0);
            media_text = Some((tg, dg));
            media = Some(r);
            y += r.h + 4.0;
            width = width.max(r.w);
        }
        Some(Media::Sticker { w, h, .. }) => {
            let (w, h) = ((*w).max(1) as f32, (*h).max(1) as f32);
            let k = (120.0 / w).min(120.0 / h);
            let r = R::new(pad_x, y, w * k, h * k);
            media = Some(r);
            y += r.h + 4.0;
            width = width.max(r.w);
        }
        _ => {}
    }

    // The text, with the time after its last line if there's room.
    let mut text = None;
    let mut meta_y;
    let mut meta_over = false;
    if !m.text.is_empty() {
        let g = Para::new(window, &m.text, TEXT, &text_runs(m, look, revealed), inner, None);
        let (last_w, last_base) = g.last_line();
        let fits_inline = last_w + 10.0 + meta_w <= inner;
        width = width.max(g.w);
        let top = y;
        y += g.h;
        if fits_inline {
            width = width.max(last_w + 10.0 + meta_w);
            // On the last line's baseline.
            meta_y = top + last_base - meta_g.baseline();
        } else {
            meta_y = y + 2.0;
            y += meta_g.h + 2.0;
        }
        text = Some((g, (pad_x, top)));
    } else if photo_top && page.is_none() && poll.is_none() {
        // The time on the picture itself.
        meta_over = true;
        meta_y = y - pad_y - meta_g.h - 8.0;
        y -= pad_y;
    } else {
        width = width.max(meta_w);
        meta_y = y;
        y += meta_g.h;
    }

    // A web page's preview, under the text.
    let page = page.map(|(site, title, description)| {
        y += 6.0;
        let top = y;
        let mut parts = Vec::new();
        let x = pad_x + 10.0;
        let w = inner - 10.0;
        for (t, f, ink, rows, sz) in [(site, bold.clone(), look.link, 1, META + 0.5), (title, bold.clone(), look.text, 2, TEXT - 0.5), (description, normal.clone(), look.text, 4, TEXT - 1.0)] {
            let Some(t) = t else { continue };
            let g = Para::plain(window, &t, sz, f, ink, w, Some(rows));
            let h = g.h;
            width = width.max(g.w + 10.0);
            parts.push((g, (x, y)));
            y += h + 1.0;
        }
        let r = R::new(pad_x, top, 3.0, y - top);
        y += 4.0;
        meta_y = y;
        y += meta_g.h;
        (r, parts)
    });

    let w = (width + 2.0 * pad_x).max(meta_w + 2.0 * pad_x + 8.0).min(max_w);
    let w = match media {
        Some(r) if photo_top => w.max(r.w),
        _ => w,
    };
    let h = y + if meta_over { 0.0 } else { pad_y };
    let x = if ours { col - w } else { left_room };
    let bubble = R::new(x, 0.0, w, h);
    let meta_x = bubble.right() - pad_x - meta_w + if meta_over { -6.0 } else { 0.0 };
    let reactions = lay_reactions(window, m, look, bubble.x, bubble.bottom() + 4.0, max_w);
    let height = reactions.last().map_or(bubble.bottom(), |r| r.0.bottom());
    let at = |p: (f32, f32)| (p.0 + x, p.1);
    Laid {
        shape: Shape::Bubble,
        height,
        bubble,
        name: name.map(|(g, p)| (g, at(p))),
        forward: forward.map(|(g, p)| (g, at(p))),
        reply: reply.map(|(r, a, b)| (r.shift(x), a, b)),
        media: media.map(|r| if photo_top { R::new(r.x + x, r.y, w, r.h) } else { r.shift(x) }),
        media_text,
        page: page.map(|(r, parts)| (r.shift(x), parts.into_iter().map(|(g, p)| (g, at(p))).collect())),
        poll: poll.map(|(q, k, head, rows)| (q, k, head.shift(x), rows.into_iter().map(|(a, b, s, r)| (a, b, s, r.shift(x))).collect())),
        text: text.map(|(g, p)| (g, at(p))),
        meta: (meta_g, (meta_x, meta_y)),
        meta_over,
        reactions,
        big_emoji: None,
        avatar: place.many && !ours && place.last,
    }
}

fn lay_reactions(window: &mut Window, m: &Message, look: &Look, x: f32, y: f32, max_w: f32) -> Vec<(R, Line, Line, String, bool)> {
    let mut out = Vec::new();
    let (mut cx, mut cy) = (x, y);
    for r in &m.reactions {
        let g = line(window, &short_count(r.count), META + 0.5, font(FontWeight::BOLD), if r.mine { white() } else { look.link }, None);
        let e = line(window, &r.emoji, 13.0, font(FontWeight::NORMAL), look.text, None);
        // (Its emoji in from the chip's edge as the bubble's text is from
        // the bubble's, so they line up.)
        let w = PAD.0 + 18.0 + 4.0 + g.w + PAD.0;
        if cx + w > x + max_w && cx > x {
            cx = x;
            cy += REACTION_H + 4.0;
        }
        out.push((R::new(cx, cy, w, REACTION_H), e, g, r.emoji.clone(), r.mine));
        cx += w + 4.0;
    }
    out
}

/// The balloon's corners: rounded fully, but a little where it joins
/// the next or previous of the same sender's.
fn corners(place: Place, ours: bool) -> Corners<Pixels> {
    let (r, j) = (px(RADIUS), px(JOINED));
    let (top, bottom) = (if place.first { r } else { j }, if place.last { r } else { j });
    if ours { Corners { top_left: r, top_right: top, bottom_left: r, bottom_right: bottom } } else { Corners { top_left: top, top_right: r, bottom_left: bottom, bottom_right: r } }
}

/// What the painter needs besides the layout.
pub struct Paint<'a> {
    pub look: &'a Look,
    pub place: Place,
    /// The message's picture, when it's here.
    pub picture: Option<Arc<RenderImage>>,
    /// The sender's picture (groups), when it's here.
    pub sender_pic: Option<Arc<RenderImage>>,
    /// How far a download or upload has got.
    pub progress: Option<f32>,
    /// Ours, read by them.
    pub read: bool,
    /// A voice message playing: how far.
    pub playing: Option<f32>,
    pub selected: bool,
}

fn fill(window: &mut Window, b: Bounds<Pixels>, radius: impl Into<Corners<Pixels>>, color: Hsla) {
    window.paint_quad(quad(b, radius.into(), color, px(0.0), transparent_black(), BorderStyle::Solid));
}

fn disc(window: &mut Window, c: Point<Pixels>, r: f32, color: Hsla) {
    window.paint_sector(c, px(0.0), px(r), 0.0, std::f32::consts::TAU, color);
}

fn pt(o: Point<Pixels>, p: (f32, f32)) -> Point<Pixels> {
    point(o.x + px(p.0), o.y + px(p.1))
}

/// What clicking `q` does in `laid`, painted with its top-left at `o`:
/// a link or mention in the text, else the topmost area under it.
pub fn hit_at(o: Point<Pixels>, m: &Message, laid: &Laid, q: Point<Pixels>) -> Option<Hit> {
    let mut areas: Vec<(Bounds<Pixels>, Hit)> = Vec::new();
    if laid.shape == Shape::Service {
        return None;
    }
    // (A sticker or a picture alone has only itself and its reactions.)
    let whole = laid.shape == Shape::Bubble;
    if whole
        && laid.avatar
        && let Some(f) = &m.from
    {
        let bottom = laid.bubble.at(o).bottom();
        areas.push((Bounds { origin: point(o.x, bottom - px(AVATAR)), size: size(px(AVATAR), px(AVATAR)) }, Hit::Sender(f.id)));
    }
    if whole
        && let Some((g, p)) = &laid.name
        && let Some(f) = &m.from
    {
        areas.push((Bounds { origin: pt(o, *p), size: size(px(g.w), px(g.h)) }, Hit::Sender(f.id)));
    }
    if whole
        && let Some((r, ..)) = &laid.reply
        && let Some(rt) = &m.reply_to
    {
        areas.push((r.at(o), Hit::Reply(rt.id)));
    }
    if let Some(r) = laid.media {
        areas.push((r.at(o), Hit::Media));
    }
    if let Some((r, parts)) = &laid.page
        && let Some(Media::WebPage { url, .. }) = &m.media
    {
        let whole = parts.iter().fold(r.at(o), |w, (g, p)| w.union(&Bounds { origin: pt(o, *p), size: size(px(g.w), px(g.h)) }));
        areas.push((whole, Hit::Link(url.clone())));
    }
    for (r, _, _, emoji, _) in &laid.reactions {
        areas.push((r.at(o), Hit::Reaction(emoji.clone())));
    }
    if whole
        && let Some((g, p)) = &laid.text
    {
        let at = pt(o, *p);
        if let Some(byte) = g.index_at(point(q.x - at.x, q.y - at.y))
            && let Some(e) = m.entities.iter().find(|e| e.start <= byte && byte < e.end)
        {
            let word = m.text.get(e.start..e.end).unwrap_or("").to_owned();
            let link = match &e.kind {
                EntityKind::Url => Some(Hit::Link(word)),
                EntityKind::Email => Some(Hit::Link(format!("mailto:{word}"))),
                EntityKind::Phone => Some(Hit::Link(format!("tel:{word}"))),
                EntityKind::TextUrl(u) => Some(Hit::Link(u.clone())),
                EntityKind::Mention => Some(Hit::Mention(word)),
                EntityKind::MentionName(id) => Some(Hit::MentionId(*id)),
                EntityKind::Hashtag | EntityKind::Cashtag | EntityKind::BotCommand => Some(Hit::Hashtag(word)),
                EntityKind::Spoiler => Some(Hit::Spoiler),
                _ => None,
            };
            if link.is_some() {
                return link;
            }
        }
    }
    areas.iter().rev().find(|(r, _)| r.contains(&q)).map(|(_, h)| h.clone())
}

/// Paint `laid` with its top-left at `o`.
pub fn paint(window: &mut Window, cx: &mut App, o: Point<Pixels>, m: &Message, laid: &Laid, pp: &Paint) {
    let look = pp.look;
    let bubble = laid.bubble.at(o);
    match laid.shape {
        Shape::Service => {
            let b = style::snap(window, bubble);
            fill(window, b, b.size.height / 2.0, look.pill);
            if let Some((g, p)) = &laid.text {
                g.paint(window, cx, pt(o, *p));
            }
            return;
        }
        Shape::Bare => {
            if let Some(r) = laid.media {
                let r = r.at(o);
                match &pp.picture {
                    Some(img) => {
                        let _ = window.paint_image(style::snap(window, r), Corners::default(), img.clone(), 0, false);
                    }
                    None => {
                        let label = match &m.media {
                            Some(Media::Sticker { emoji, .. }) | Some(Media::Dice { emoji, .. }) => emoji.clone(),
                            _ => String::new(),
                        };
                        if label.is_empty() {
                            fill(window, r, px(12.0), look.theirs.opacity(0.6));
                        } else {
                            let size = f32::from(r.size.width) * 0.5;
                            style::label(window, cx, r.center().x, r.center().y, Align::Center, label, size, font(FontWeight::NORMAL), look.text);
                        }
                    }
                }
            }
            if let Some((g, p)) = &laid.big_emoji {
                g.paint(window, cx, pt(o, *p));
            }
            let (g, mp) = &laid.meta;
            let mr = Bounds { origin: pt(o, *mp), size: size(px(g.w + 12.0 + if m.out { TICKS } else { 0.0 }), px(g.h + 4.0)) };
            let mr = style::snap(window, mr);
            fill(window, mr, mr.size.height / 2.0, look.pill);
            g.paint(window, cx, point(mr.origin.x + px(6.0), mr.origin.y + px(2.0)));
            if m.out {
                ticks(window, cx, point(mr.right() - px(6.0 + TICKS - 2.0), mr.center().y), pp.read, m.pending, look.pill_text);
            }
            paint_reactions(window, cx, o, laid, look);
            return;
        }
        Shape::Bubble => {}
    }

    // The sender's picture beside the last of their run.
    if laid.avatar
        && let Some(f) = &m.from
    {
        let r = Bounds { origin: point(o.x, bubble.bottom() - px(AVATAR)), size: size(px(AVATAR), px(AVATAR)) };
        avatar(window, cx, r, f.id, &f.name, pp.sender_pic.as_ref(), look.dark);
    }
    let (fill_c, edge) = if m.out { (look.ours, look.ours_edge) } else { (look.theirs, look.theirs_edge) };
    let corners = corners(pp.place, m.out);
    let br = style::snap(window, bubble);
    // A soft lift, as a card's: a whole device pixel or two down.
    let s = window.scale_factor();
    let lift = Bounds { origin: point(br.origin.x, br.origin.y + px(s.round().max(1.0) / s)), size: br.size };
    fill(window, lift, corners, hsla(0.0, 0.0, 0.0, if look.dark { 50.0 } else { 14.0 } / 255.0));
    let face = if pp.selected { blend(fill_c, look.accent, 0.25) } else { fill_c };
    window.paint_quad(quad(br, corners, face, style::hairline(window), edge, BorderStyle::Solid));

    if let Some((g, p)) = &laid.name {
        g.paint(window, cx, pt(o, *p));
    }
    if let Some((g, p)) = &laid.forward {
        g.paint(window, cx, pt(o, *p));
    }
    if let Some((r, ng, tg)) = &laid.reply {
        let r = style::snap(window, r.at(o));
        fill(window, r, px(4.0), look.link.opacity(0.10));
        let bar = style::snap(window, Bounds { origin: r.origin, size: size(px(3.0), r.size.height) });
        fill(window, bar, Corners { top_left: px(4.0), bottom_left: px(4.0), top_right: px(0.0), bottom_right: px(0.0) }, look.link);
        ng.paint(window, cx, point(r.origin.x + px(9.0), r.origin.y + px(3.0)));
        tg.paint(window, cx, point(r.origin.x + px(9.0), r.origin.y + px(4.0 + ng.h)));
    }
    if let Some(r) = laid.media {
        let r = r.at(o);
        paint_media(window, cx, r, m, laid, pp, corners);
    }
    if let Some((r, parts)) = &laid.page {
        let rr = style::snap(window, r.at(o));
        fill(window, rr, px(1.5), look.link);
        for (g, p) in parts {
            g.paint(window, cx, pt(o, *p));
        }
    }
    if let Some((qg, kg, head, rows)) = &laid.poll {
        let h = head.at(o);
        qg.paint(window, cx, h.origin);
        kg.paint(window, cx, point(h.origin.x, h.origin.y + px(qg.h + 2.0)));
        for (a, b, share, r) in rows {
            let r = r.at(o);
            // The answer and its share on one baseline.
            let baseline = style::snap_coord(window, r.origin.y + px(a.baseline()));
            a.paint(window, cx, point(r.origin.x + px(40.0), baseline - px(a.baseline())));
            b.paint(window, cx, point(r.origin.x + px(34.0 - b.w), baseline - px(b.baseline())));
            let bar = Bounds { origin: point(r.origin.x + px(40.0), r.bottom() - px(5.0)), size: size((r.size.width - px(40.0)) * share.max(0.02), px(3.0)) };
            fill(window, style::snap(window, bar), px(1.5), look.link);
        }
    }
    if let Some((g, p)) = &laid.text {
        let at = pt(o, *p);
        for e in m.entities.iter().filter(|e| matches!(e.kind, EntityKind::Pre(_))) {
            if let Some((top, bottom)) = g.rows_of(e.start..e.end) {
                let r = Bounds::from_corners(point(at.x - px(4.0), at.y + px(top - 2.0)), point(at.x + px(g.w + 4.0), at.y + px(bottom + 2.0)));
                fill(window, style::snap(window, r), px(6.0), look.text.opacity(0.06));
            }
        }
        g.paint(window, cx, at);
    }
    let (g, mp) = &laid.meta;
    let mp = pt(o, *mp);
    let mp = point(style::snap_coord(window, mp.x), style::snap_coord(window, mp.y));
    let ink = if laid.meta_over {
        white()
    } else if m.pending == Some((0, Pending::Failed)) {
        rgb(0xFF0000).into()
    } else {
        look.dim
    };
    if laid.meta_over {
        let mr = Bounds { origin: point(mp.x - px(6.0), mp.y - px(2.0)), size: size(px(g.w + 12.0 + if m.out { TICKS } else { 0.0 }), px(g.h + 4.0)) };
        let mr = style::snap(window, mr);
        fill(window, mr, mr.size.height / 2.0, hsla(0.0, 0.0, 0.0, 110.0 / 255.0));
    }
    if m.views.is_some() {
        eye(window, point(mp.x - px(9.0), mp.y + px(g.h / 2.0)), ink);
    }
    g.paint(window, cx, mp);
    if m.out {
        let tick_ink = if laid.meta_over { white() } else if pp.read { look.link } else { look.dim };
        ticks(window, cx, point(mp.x + px(g.w + 3.0), mp.y + px(g.h / 2.0)), pp.read, m.pending, tick_ink);
    }
    paint_reactions(window, cx, o, laid, look);
}

/// `a` taken `t` of the way to `b`.
pub fn blend(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    olo_ui::gpui::Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }.into()
}

fn paint_reactions(window: &mut Window, cx: &mut App, o: Point<Pixels>, laid: &Laid, look: &Look) {
    for (r, e, g, _, mine) in &laid.reactions {
        let r = style::snap(window, r.at(o));
        let face = if *mine { look.accent } else { look.link.opacity(0.14) };
        fill(window, r, r.size.height / 2.0, face);
        // The count centred in the chip by its figures, the emoji on
        // their middle (as a mark beside text is).
        let size_ = META + 0.5;
        let baseline = style::baseline_at(window, r.center().y, size_);
        let caps = baseline - px(size_ * style::CAP_HEIGHT / 2.0);
        let ex = r.origin.x + px(PAD.0);
        e.paint(window, cx, point(ex, caps - px(e.h / 2.0)));
        g.paint(window, cx, point(ex + px(18.0 + 4.0), baseline - px(g.baseline())));
    }
}

fn triangle(window: &mut Window, pts: [(f32, f32); 3], c: Point<Pixels>, color: Hsla) {
    let mut path = PathBuilder::fill();
    path.move_to(point(c.x + px(pts[0].0), c.y + px(pts[0].1)));
    path.line_to(point(c.x + px(pts[1].0), c.y + px(pts[1].1)));
    path.line_to(point(c.x + px(pts[2].0), c.y + px(pts[2].1)));
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn paint_media(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, m: &Message, laid: &Laid, pp: &Paint, corners: Corners<Pixels>) {
    let look = pp.look;
    match &m.media {
        Some(Media::Photo { .. }) | Some(Media::Video { .. }) => {
            let round = matches!(m.media, Some(Media::Video { round: true, .. }));
            let leads = laid.name.is_none() && laid.forward.is_none() && laid.reply.is_none();
            let mut c = if leads { Corners { bottom_left: px(0.0), bottom_right: px(0.0), ..corners } } else { Corners::all(px(6.0)) };
            if leads && laid.text.is_none() && laid.page.is_none() {
                c = corners;
            }
            let rr = style::snap(window, r);
            match &pp.picture {
                Some(img) if round => {
                    let _ = window.paint_image(rr, Corners::all(rr.size.width / 2.0), img.clone(), 0, false);
                }
                Some(img) => {
                    // Filling the box, cut to it where its shape differs.
                    let s = img.size(0);
                    let (iw, ih) = (s.width.0.max(1) as f32, s.height.0.max(1) as f32);
                    let (bw, bh) = (f32::from(rr.size.width), f32::from(rr.size.height).max(1.0));
                    let k = (bw / bh) / (iw / ih);
                    if (k - 1.0).abs() < 0.02 {
                        let _ = window.paint_image(rr, c, img.clone(), 0, false);
                    } else {
                        let (w, h) = if k > 1.0 { (bw, bw * ih / iw) } else { (bh * iw / ih, bh) };
                        let full = Bounds { origin: point(rr.center().x - px(w / 2.0), rr.center().y - px(h / 2.0)), size: size(px(w), px(h)) };
                        window.with_content_mask(Some(olo_ui::gpui::ContentMask { bounds: rr }), |window| {
                            let _ = window.paint_image(full, Corners::default(), img.clone(), 0, false);
                        });
                    }
                }
                None => fill(window, rr, if round { Corners::all(rr.size.width / 2.0) } else { c }, look.text.opacity(0.08)),
            }
            if let Some(Media::Video { duration, .. }) = &m.media {
                // A play button and the length.
                let cc = rr.center();
                disc(window, cc, 22.0, hsla(0.0, 0.0, 0.0, 120.0 / 255.0));
                triangle(window, [(-6.0, -9.0), (-6.0, 9.0), (10.0, 0.0)], cc, white());
                let g = line(window, &olo_tg::clock(*duration), META, font(FontWeight::NORMAL), white(), None);
                let tr = Bounds { origin: point(rr.origin.x + px(6.0), rr.origin.y + px(6.0)), size: size(px(g.w + 10.0), px(g.h + 4.0)) };
                fill(window, tr, tr.size.height / 2.0, hsla(0.0, 0.0, 0.0, 120.0 / 255.0));
                g.paint(window, cx, point(tr.origin.x + px(5.0), tr.origin.y + px(2.0)));
            }
            if let Some(f) = pp.progress {
                ring(window, rr.center(), 20.0, f, white());
            }
        }
        Some(Media::Voice { waveform, duration, .. }) => {
            let c = point(r.origin.x + px(20.0), r.center().y);
            disc(window, c, 18.0, look.accent);
            if pp.playing.is_some() {
                for dx in [-4.0, 4.0] {
                    fill(window, Bounds { origin: point(c.x + px(dx - 2.0), c.y - px(6.5)), size: size(px(4.0), px(13.0)) }, px(1.0), white());
                }
            } else {
                triangle(window, [(-5.0, -8.0), (-5.0, 8.0), (9.0, 0.0)], c, white());
            }
            if let Some(f) = pp.progress {
                ring(window, c, 18.0, f, white());
            }
            // The waveform: bars of its samples, the part played in the
            // accent; each on device pixels, two points wide, its height
            // whole pixels standing on the same line.
            let (left, top, right) = (r.origin.x + px(46.0), r.origin.y + px(4.0), r.right());
            let wh = 20.0;
            let n = ((f32::from(right - left) / 3.0) as usize).max(1);
            let played = pp.playing.unwrap_or(0.0);
            let s = window.scale_factor();
            let bottom = style::snap_coord(window, top + px(wh));
            let (w, step) = ((2.0 * s).round() / s, (3.0 * s).round() / s);
            let x0 = style::snap_coord(window, left);
            for i in 0..n {
                let v = if waveform.is_empty() { 4 } else { waveform[i * waveform.len() / n] } as f32;
                let h = ((2.0 + v / 31.0 * wh).min(wh) * s).round().max(1.0) / s;
                let x = x0 + px(i as f32 * step);
                let ink = if (i as f32 / n as f32) < played { look.accent } else { look.accent.opacity(0.35) };
                fill(window, Bounds::from_corners(point(x, bottom - px(h)), point(x + px(w), bottom)), px(1.0), ink);
            }
            let t = match pp.playing {
                Some(f) => olo_tg::clock(f * duration),
                None => olo_tg::clock(*duration),
            };
            let g = line(window, &t, META, font(FontWeight::NORMAL), look.dim, None);
            g.paint(window, cx, point(left, bottom + px(3.0)));
        }
        Some(Media::Audio { .. }) | Some(Media::File { .. }) => {
            let ic = point(r.origin.x + px(21.0), r.origin.y + px(21.0));
            disc(window, ic, 20.0, look.accent);
            match pp.progress {
                Some(f) => ring(window, ic, 15.0, f, white()),
                None if matches!(m.media, Some(Media::Audio { .. })) => triangle(window, [(-5.0, -8.0), (-5.0, 8.0), (9.0, 0.0)], ic, white()),
                None => document(window, ic, white()),
            }
            if let Some((tg, dg)) = &laid.media_text {
                tg.paint(window, cx, point(r.origin.x + px(52.0), r.origin.y + px(3.0)));
                dg.paint(window, cx, point(r.origin.x + px(52.0), r.origin.y + px(5.0 + tg.h)));
            }
        }
        Some(Media::Sticker { .. }) => match &pp.picture {
            Some(img) => {
                let _ = window.paint_image(style::snap(window, r), Corners::default(), img.clone(), 0, false);
            }
            None => fill(window, r, px(8.0), look.text.opacity(0.06)),
        },
        _ => {
            if let Some((tg, dg)) = &laid.media_text {
                tg.paint(window, cx, r.origin);
                dg.paint(window, cx, point(r.origin.x, r.origin.y + px(tg.h + 2.0)));
            }
        }
    }
}

/// One tick (sent) or two (read); a clock while sending, "!" if it failed.
fn ticks(window: &mut Window, cx: &mut App, left_mid: Point<Pixels>, read: bool, pending: Option<(u64, Pending)>, ink: Hsla) {
    let at = |x: f32, y: f32| point(left_mid.x + px(x), left_mid.y + px(y));
    let w = px(1.3);
    match pending {
        Some((_, Pending::Sending)) => {
            let c = at(6.0, 0.0);
            window.paint_sector(c, px(4.5 - 0.65), px(4.5 + 0.65), 0.0, std::f32::consts::TAU, ink);
            window.paint_segment(c, at(6.0, -3.0), w, false, ink);
            window.paint_segment(c, at(8.0, 0.0), w, false, ink);
        }
        Some((_, Pending::Failed)) => {
            let c = at(6.0, 0.0);
            disc(window, c, 5.5, rgb(0xE53E3E).into());
            style::label(window, cx, c.x, c.y, Align::Center, "!", 9.0, font(FontWeight::BOLD), white());
        }
        None => {
            let mut path = PathBuilder::stroke(w);
            path.move_to(at(0.0, 0.0));
            path.line_to(at(3.0, 3.0));
            path.line_to(at(9.0, -3.5));
            if read {
                path.move_to(at(7.0, 3.0));
                path.line_to(at(13.0, -3.5));
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, ink);
            }
        }
    }
}

/// A small eye (a channel post's views), centred at `c`.
fn eye(window: &mut Window, c: Point<Pixels>, ink: Hsla) {
    let mut path = PathBuilder::stroke(px(1.0));
    for side in [-1.0f32, 1.0] {
        for i in 0..=12 {
            let t = i as f32 / 12.0 * std::f32::consts::PI;
            let q = point(c.x + px(-4.5 * t.cos()), c.y + px(-2.6 * t.sin() * side));
            if i == 0 { path.move_to(q) } else { path.line_to(q) }
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, ink);
    }
    disc(window, c, 1.4, ink);
}

/// A page with a folded corner (a file), centred at `c`: its edges whole
/// device pixels, each down the middle of its pixels so it's crisp.
fn document(window: &mut Window, c: Point<Pixels>, ink: Hsla) {
    let s = window.scale_factor();
    let dev = (1.5 * s).round().max(1.0);
    let at = |v: f32| ((v * s - dev / 2.0).round() + dev / 2.0) / s;
    let (cx_, cy_) = (f32::from(c.x), f32::from(c.y));
    let (l, t, r, b) = (at(cx_ - 6.0), at(cy_ - 7.5), at(cx_ + 6.0), at(cy_ + 7.5));
    let fold = (4.0 * s).round() / s;
    let mut path = PathBuilder::stroke(px(dev / s));
    let p = |x: f32, y: f32| point(px(x), px(y));
    path.move_to(p(l, t));
    path.line_to(p(r - fold, t));
    path.line_to(p(r, t + fold));
    path.line_to(p(r, b));
    path.line_to(p(l, b));
    path.close();
    path.move_to(p(r - fold, t));
    path.line_to(p(r - fold, t + fold));
    path.line_to(p(r, t + fold));
    if let Ok(path) = path.build() {
        window.paint_path(path, ink);
    }
}

/// A ring of progress round `c`, from 12 o'clock.
pub fn ring(window: &mut Window, c: Point<Pixels>, radius: f32, fraction: f32, ink: Hsla) {
    window.paint_sector(c, px(radius - 1.0), px(radius + 1.0), 0.0, fraction.clamp(0.02, 1.0) * std::f32::consts::TAU, ink);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_to_initials() {
        assert_eq!(initials("Mira Okafor"), "MO");
        assert_eq!(initials("Design Crew Team"), "DT");
        assert_eq!(initials("jun"), "J");
        assert_eq!(initials("🎉 Party"), "P");
        assert_eq!(short_count(18_204), "18K");
        assert_eq!(short_count(9_311), "9.3K");
        assert_eq!(short_count(2_000), "2K");
    }

    #[test]
    fn emoji_alone_is_big() {
        let m = |t: &str| Message { text: t.into(), ..Message::default() };
        assert!(big_emoji(&m("🎉")));
        assert!(big_emoji(&m("👍🏽 ❤\u{FE0F}")));
        assert!(!big_emoji(&m("hi 🎉")));
        assert!(!big_emoji(&m("🎉🎉🎉🎉")));
    }
}
