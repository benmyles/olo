//! Cards: what a tip shows when it's more than a sentence (Deskbar's
//! widgets: a graph of the last minute, each core, the processes using
//! most, a quota's accounts). A raised panel, as BeOS's are, with a
//! title and the value it's about at its top, then its parts: a
//! drawing its owner paints (live: it may ask to be drawn again, as a
//! graph scrolling does), rows (a name, a value, a level as a bar,
//! a note under the name), and notes. Its size is known before it
//! shows ([`Card::size`]), so the tip opens at it; it is painted on
//! device pixels, as the kit's controls are; and it says the same in
//! words ([`Card::text`]) for agents, who read it with the tip.
//!
//! ```ignore
//! let card = Card::new("CPU").value("13%", None).part(Part::Rows { heading: Some("Using the most".into()), rows });
//! tip::hover_card(cx, key, Rc::new(card), anchor, hovered);
//! ```

use std::rc::Rc;

use olo_interface::{Bounds, FontWeight, Hsla, Painter, Pixels, SharedString, agent, point, px, size};

use crate::{style, theme};

/// A card's width (points): room for a name and its value side by
/// side, and a minute of a graph.
pub const CARD_WIDTH: f32 = 300.0;
/// Inside its edge, all round.
pub const CARD_PAD: f32 = 12.0;
/// What a drawing has across: the card less its padding.
pub const CARD_INNER: f32 = CARD_WIDTH - 2.0 * CARD_PAD;
/// Between its parts.
const GAP: f32 = 10.0;
/// The title's line, and the caption's under it.
const HEAD: f32 = 20.0;
const CAPTION: f32 = 15.0;
/// A row's line; its level's bar under it, and its note's line.
const ROW: f32 = 19.0;
const BAR: f32 = 4.0;
const BAR_GAP: f32 = 2.0;
const ROW_NOTE: f32 = 15.0;
/// After a row with a bar or a note: room before the next.
const ROW_AFTER: f32 = 4.0;
/// A list's heading, and a note's line.
const HEADING: f32 = 18.0;
const NOTE: f32 = 16.0;
/// A row's colour swatch.
const SWATCH: f32 = 8.0;

/// What a [`Part::Drawing`] paints with: the place it has (the card's
/// inner width, its own height).
pub type CardPaint = Rc<dyn Fn(&mut Painter, Bounds<Pixels>)>;

/// One thing on a card, under its title.
#[derive(Clone)]
pub enum Part {
    /// Painted by the card's owner, `height` points tall and
    /// [`CARD_INNER`] wide (a graph, bars, a heatmap), under its
    /// heading if it has one, said to agents as `text`.
    Drawing { heading: Option<SharedString>, height: f32, paint: CardPaint, text: SharedString },
    /// Rows under a heading (in small capitals).
    Rows { heading: Option<SharedString>, rows: Vec<Row> },
    /// Lines in the dim colour, each one line (cut with "…").
    Notes(Vec<SharedString>),
}

/// A row of a [`Part::Rows`].
#[derive(Clone, Default)]
pub struct Row {
    pub name: SharedString,
    pub value: SharedString,
    /// How full, 0 to 1: a bar under it, in `color` (the accent without).
    pub level: Option<f32>,
    pub color: Option<Hsla>,
    /// Under the name, dim ("resets in 4 h 17").
    pub note: Option<SharedString>,
    /// A square of its colour before its name (a series' key).
    pub swatch: Option<Hsla>,
    /// In bold (the one answering now, the chosen).
    pub strong: bool,
}

impl Row {
    pub fn new(name: impl Into<SharedString>, value: impl Into<SharedString>) -> Row {
        Row { name: name.into(), value: value.into(), ..Default::default() }
    }

    pub fn level(mut self, level: f32, color: Option<Hsla>) -> Self {
        self.level = Some(level.clamp(0.0, 1.0));
        self.color = color;
        self
    }

    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn swatch(mut self, color: Hsla) -> Self {
        self.swatch = Some(color);
        self
    }

    pub fn strong(mut self, on: bool) -> Self {
        self.strong = on;
        self
    }

    fn height(&self) -> f32 {
        let mut h = ROW;
        if self.note.is_some() {
            h += ROW_NOTE;
        }
        if self.level.is_some() {
            h += BAR_GAP + BAR;
        }
        if self.note.is_some() || self.level.is_some() {
            h += ROW_AFTER;
        }
        h
    }
}

/// A tip's card: its title, the value it's about, a line under the
/// title, and its parts.
#[derive(Clone)]
pub struct Card {
    pub title: SharedString,
    pub caption: Option<SharedString>,
    pub value: Option<SharedString>,
    pub value_color: Option<Hsla>,
    pub parts: Vec<Part>,
}

impl Card {
    pub fn new(title: impl Into<SharedString>) -> Card {
        Card { title: title.into(), caption: None, value: None, value_color: None, parts: Vec::new() }
    }

    pub fn caption(mut self, caption: impl Into<SharedString>) -> Self {
        self.caption = Some(caption.into());
        self
    }

    /// The value it's about, at its top right (in `color`: the ink's
    /// without).
    pub fn value(mut self, value: impl Into<SharedString>, color: Option<Hsla>) -> Self {
        self.value = Some(value.into());
        self.value_color = color;
        self
    }

    pub fn part(mut self, part: Part) -> Self {
        self.parts.push(part);
        self
    }

    /// Its caption's lines: wrapped at words to the card's width, two at
    /// most (the second cut with "…" if it's still longer).
    fn caption_lines(&self) -> Vec<SharedString> {
        let Some(caption) = &self.caption else { return Vec::new() };
        let font = style::ui_font(FontWeight::NORMAL);
        let fits = |t: &str| f32::from(style::text_width(t.to_owned(), style::DETAIL_SIZE, font.clone())) <= CARD_INNER;
        if fits(caption) {
            return vec![caption.clone()];
        }
        let mut first = String::new();
        let mut words = caption.split(' ').peekable();
        while let Some(w) = words.peek() {
            let tried = if first.is_empty() { (*w).to_owned() } else { format!("{first} {w}") };
            if !fits(&tried) && !first.is_empty() {
                break;
            }
            first = tried;
            words.next();
        }
        let rest: Vec<&str> = words.collect();
        if rest.is_empty() { vec![first.into()] } else { vec![first.into(), rest.join(" ").into()] }
    }

    /// Its size (points), on whole points: as wide as every card, as
    /// tall as its parts.
    pub fn size(&self) -> (f32, f32) {
        let mut h = 2.0 * CARD_PAD + HEAD;
        h += self.caption_lines().len() as f32 * CAPTION;
        for (i, part) in self.parts.iter().enumerate() {
            // (The first under an etched line, the rest a gap apart.)
            h += if i == 0 { GAP + 2.0 } else { GAP };
            h += match part {
                Part::Drawing { heading, height, .. } => heading.as_ref().map_or(0.0, |_| HEADING) + *height,
                Part::Rows { heading, rows } => heading.as_ref().map_or(0.0, |_| HEADING) + rows.iter().map(Row::height).sum::<f32>(),
                Part::Notes(lines) => lines.len() as f32 * NOTE,
            };
        }
        (CARD_WIDTH, h.ceil())
    }

    /// What it says, in words: for agents, and wherever it can't be
    /// drawn.
    pub fn text(&self) -> String {
        let mut out = match &self.value {
            Some(v) => format!("{}: {v}", self.title),
            None => self.title.to_string(),
        };
        if let Some(c) = &self.caption {
            out.push_str(&format!("\n{c}"));
        }
        for part in &self.parts {
            match part {
                Part::Drawing { heading: Some(h), text, .. } if !text.is_empty() => out.push_str(&format!("\n{h}: {text}")),
                Part::Drawing { text, .. } if !text.is_empty() => out.push_str(&format!("\n{text}")),
                Part::Drawing { .. } => {}
                Part::Rows { heading, rows } => {
                    out.push('\n');
                    if let Some(h) = heading {
                        out.push_str(&format!("\n{h}:"));
                    }
                    for r in rows {
                        let mut line = format!("\n{}: {}", r.name, r.value);
                        if let Some(n) = &r.note {
                            line.push_str(&format!(" ({n})"));
                        }
                        out.push_str(&line);
                    }
                }
                Part::Notes(lines) => {
                    for l in lines {
                        out.push_str(&format!("\n{l}"));
                    }
                }
            }
        }
        out
    }

    /// Paints it filling `b` (at [`Card::size`]).
    pub fn paint(&self, g: &mut Painter, b: Bounds<Pixels>) {
        let p = theme::palette();
        let panel = p.panel_background;
        let ink = style::c(p.panel_text);
        // (Dim: the ink toward the panel, readable on it still.)
        let dim = style::c(olo_theme::readable(olo_theme::mix(p.panel_text, panel, 0.42), panel, 4.5));
        // A raised panel: its outline, then light along the top and
        // left, shadow along the bottom and right.
        style::fill_rect(g, b, style::c(panel));
        style::frame_rect(g, b, style::shade(&p, panel, 1.45));
        let inner = style::inset(g, b, 1.0);
        style::bevel(g, inner, style::c(p.shine), style::shade(&p, panel, 1.15));
        let (l, r) = (b.left() + px(CARD_PAD), b.right() - px(CARD_PAD));
        let plain = style::ui_font(FontWeight::NORMAL);
        let bold = style::ui_font(FontWeight::BOLD);
        let mut y = b.top() + px(CARD_PAD);
        // Its title, and its value across from it, on one baseline (the
        // value's larger: its capitals centred in the line).
        let baseline = style::baseline_at(g, y + px(HEAD / 2.0), style::LEAD_SIZE);
        let value_w = match &self.value {
            Some(v) => {
                let at = style::label_on_baseline(g, r, baseline, style::Align::Right, v.clone(), style::LEAD_SIZE, bold.clone(), self.value_color.unwrap_or(ink));
                f32::from(at.size.width) + olo_kit_base::metrics::TIGHT
            }
            None => 0.0,
        };
        style::label_on_baseline_fit(g, l, baseline, style::Align::Left, self.title.clone(), style::PLAIN_SIZE, bold.clone(), ink, px(CARD_INNER - value_w));
        g.agent(agent::Node {
            role: "Heading",
            name: Some(self.title.clone()),
            value: self.value.clone(),
            bounds: Bounds { origin: point(l, y), size: size(px(CARD_INNER), px(HEAD)) },
            enabled: true,
            ..Default::default()
        });
        y += px(HEAD);
        if let Some(c) = &self.caption {
            g.agent(agent::Node::label(c.clone(), Bounds { origin: point(l, y), size: size(px(CARD_INNER), px(CAPTION)) }));
        }
        for c in self.caption_lines() {
            style::label_fit(g, l, y + px(CAPTION / 2.0 - 1.0), style::Align::Left, c, style::DETAIL_SIZE, plain.clone(), dim, px(CARD_INNER));
            y += px(CAPTION);
        }
        for (i, part) in self.parts.iter().enumerate() {
            if i == 0 {
                // An etched line under the title.
                let at = y + px(GAP / 2.0);
                style::hline(g, b.left() + px(CARD_PAD / 2.0), b.right() - px(CARD_PAD / 2.0), at, style::shade(&p, panel, 1.22));
                style::hline(g, b.left() + px(CARD_PAD / 2.0), b.right() - px(CARD_PAD / 2.0), at + style::hairline(g), style::c(p.shine));
                y += px(GAP + 2.0);
            } else {
                y += px(GAP);
            }
            match part {
                Part::Drawing { heading, height, paint, text } => {
                    if let Some(h) = heading {
                        style::label(g, l, y + px(HEADING / 2.0), style::Align::Left, h.to_uppercase(), style::SMALL_SIZE, bold.clone(), dim);
                        g.agent(agent::Node {
                            role: "Heading",
                            name: Some(h.clone()),
                            bounds: Bounds { origin: point(l, y), size: size(px(CARD_INNER), px(HEADING)) },
                            enabled: true,
                            ..Default::default()
                        });
                        y += px(HEADING);
                    }
                    let at = Bounds { origin: point(l, y), size: size(px(CARD_INNER), px(*height)) };
                    paint(g, at);
                    if !text.is_empty() {
                        g.agent(agent::Node { role: "Figure", name: Some(self.title.clone()), value: Some(text.clone()), bounds: at, enabled: true, ..Default::default() });
                    }
                    y += px(*height);
                }
                Part::Rows { heading, rows } => {
                    if let Some(h) = heading {
                        style::label(g, l, y + px(HEADING / 2.0), style::Align::Left, h.to_uppercase(), style::SMALL_SIZE, bold.clone(), dim);
                        g.agent(agent::Node {
                            role: "Heading",
                            name: Some(h.clone()),
                            bounds: Bounds { origin: point(l, y), size: size(px(CARD_INNER), px(HEADING)) },
                            enabled: true,
                            ..Default::default()
                        });
                        y += px(HEADING);
                    }
                    for row in rows {
                        let font = if row.strong { bold.clone() } else { plain.clone() };
                        let mid = y + px(ROW / 2.0);
                        let mut nx = l;
                        if let Some(sw) = row.swatch {
                            let s = style::snap_centred(g, point(l + px(SWATCH / 2.0), mid), SWATCH, SWATCH);
                            style::fill_rect(g, s, sw);
                            style::frame_rect(g, s, style::shade(&p, panel, 1.5));
                            nx += px(SWATCH + 6.0);
                        }
                        let vw = if row.value.is_empty() {
                            0.0
                        } else {
                            let at = style::label(g, r, mid, style::Align::Right, row.value.clone(), style::PLAIN_SIZE, font.clone(), ink);
                            f32::from(at.size.width) + olo_kit_base::metrics::TIGHT
                        };
                        style::label_fit(g, nx, mid, style::Align::Left, row.name.clone(), style::PLAIN_SIZE, font.clone(), ink, r - nx - px(vw));
                        let said = match &row.note {
                            Some(n) => format!("{}: {} ({n})", row.name, row.value),
                            None => format!("{}: {}", row.name, row.value),
                        };
                        g.agent(agent::Node::label(said, Bounds { origin: point(l, y), size: size(px(CARD_INNER), px(ROW)) }));
                        y += px(ROW);
                        if let Some(note) = &row.note {
                            style::label_fit(g, nx, y + px(ROW_NOTE / 2.0 - 2.0), style::Align::Left, note.clone(), style::DETAIL_SIZE, plain.clone(), dim, r - nx);
                            y += px(ROW_NOTE);
                        }
                        if let Some(level) = row.level {
                            y += px(BAR_GAP);
                            let track = Bounds { origin: point(nx, y), size: size(r - nx, px(BAR)) };
                            let track = style::snap(g, track);
                            style::fill_rect(g, track, style::shade(&p, panel, if p.is_dark() { 0.8 } else { 1.12 }));
                            // (Its end on a device pixel; any level above
                            // none shows at least one.)
                            let device = g.scale_factor();
                            let w = (f32::from(track.size.width) * device * level).round().max(if level > 0.0 { 1.0 } else { 0.0 }) / device;
                            let filled = Bounds { origin: track.origin, size: size(px(w), track.size.height) };
                            if w > 0.0 {
                                style::fill_rect(g, filled, row.color.unwrap_or(style::c(p.control_highlight)));
                            }
                            y += px(BAR);
                        }
                        if row.note.is_some() || row.level.is_some() {
                            y += px(ROW_AFTER);
                        }
                    }
                }
                Part::Notes(lines) => {
                    for line in lines {
                        style::label_fit(g, l, y + px(NOTE / 2.0), style::Align::Left, line.clone(), style::DETAIL_SIZE, plain.clone(), dim, px(CARD_INNER));
                        g.agent(agent::Node::label(line.clone(), Bounds { origin: point(l, y), size: size(px(CARD_INNER), px(NOTE)) }));
                        y += px(NOTE);
                    }
                }
            }
        }
    }
}
