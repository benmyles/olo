//! Formatted documents (Markdown, and the text of Word, OpenDocument and
//! PowerPoint files) set as a page: a measure of at most `MEASURE`,
//! centred; body text on a 20-point line; headings with room above
//! them (and a rule under the first two levels); lists with hanging
//! indents, their numbers lined up on the right and tasks as check boxes;
//! quotes and GitHub's alerts beside a bar; code in a box, coloured by
//! its language; tables as wide as what they hold, lined up as the
//! document says; and the document's own pictures, a texture pixel to a
//! device pixel when they fit. Links open (a `#heading` scrolls to it).
//!
//! Blocks are laid out once for a width and scale: those in view at once,
//! the rest a few milliseconds a frame after; only those in view are
//! drawn.

use std::{
    collections::HashMap,
    ops::Range,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use olo_kit::{
    egui::{self, Color32, FontFamily, FontId, Galley, Pos2, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2, pos2, text::LayoutJob, vec2},
    style::{self, Syntax, c},
    theme::{Hue, Palette, mix, readable},
    widgets::{self, ScrollView},
};

use crate::{
    doc::{Align, Block, Callout, Row, Span},
    load::Image,
};

/// The text column's greatest width: about 75 characters.
const MEASURE: f32 = 680.0;
/// Room around the page.
const MARGIN: f32 = 24.0;
const BODY: f32 = 13.0;
/// Body text's line, a whole number of points.
const LEADING: f32 = 20.0;
const MONO: f32 = 12.0;
const CODE_LEADING: f32 = 18.0;
/// A list level's indent, and a quote's.
const INDENT: f32 = 24.0;
const QUOTE_INDENT: f32 = 16.0;
const QUOTE_BAR: f32 = 3.0;
/// Between blocks, and between a list's items.
const GAP: f32 = 12.0;
const ITEM_GAP: f32 = 4.0;
const CELL_PAD: Vec2 = vec2(10.0, 6.0);
const CODE_PAD: Vec2 = vec2(12.0, 10.0);
/// Heading sizes and the room above each, by level.
const HEADING_SIZE: [f32; 6] = [26.0, 20.0, 16.0, 14.0, 13.0, 13.0];
const HEADING_ABOVE: [f32; 6] = [28.0, 24.0, 20.0, 16.0, 14.0, 14.0];
/// Capitals' height as a part of the size (Noto Sans).
const CAP: f32 = 0.714;
/// What a frame spends laying out blocks out of view, and at most on
/// those in view.
const BUDGET: Duration = Duration::from_millis(3);
const IN_VIEW: Duration = Duration::from_millis(5);
/// Blocks shown at most.
const MAX_BLOCKS: usize = 5000;

/// What a document view keeps from frame to frame.
#[derive(Default)]
pub struct RichView {
    laid: Vec<Laid>,
    /// Quote bars, as they close.
    bars: Vec<(Rect, Color32)>,
    at: Cursor,
    /// What it is laid out for.
    key: Option<Key>,
    /// The height of a block on average when last laid out: for those not
    /// yet laid out again.
    average: f32,
    textures: HashMap<(usize, usize), TextureHandle>,
}

#[derive(Clone, PartialEq)]
struct Key {
    id: u64,
    width: f32,
    ppp: f32,
    palette: Palette,
    generation: u64,
}

/// How far the laying out has gone.
#[derive(Default)]
struct Cursor {
    next: usize,
    /// The bottom of what is laid out.
    y: f32,
    prev: Option<Kind>,
    /// Open quotes: where each began and its bar's colour.
    quotes: Vec<(f32, Color32, bool)>,
}

/// What a block is, for the room around it.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Heading(u8),
    Para,
    Item,
    ItemMore,
    Code(bool),
    Other,
    QuoteStart(bool),
    QuoteEnd,
}

/// One block, laid out: what to draw, in the page's coordinates.
struct Laid {
    top: f32,
    bottom: f32,
    draws: Vec<Draw>,
    links: Vec<Link>,
    /// What it says, and what it is, for agents.
    text: String,
    role: egui::accesskit::Role,
    /// A heading's anchor (`#getting-started`).
    slug: Option<String>,
}

enum Draw {
    Text(Pos2, Arc<Galley>),
    Fill(Rect, f32, Color32),
    /// A one-point line: horizontal (from x0 to x1 at y) or vertical, its
    /// top or left edge at the place given, or (`ends`) its bottom or right.
    HLine { x0: f32, x1: f32, y: f32, ends: bool, color: Color32 },
    VLine { x: f32, y0: f32, y1: f32, ends: bool, color: Color32 },
    Bullet(Pos2, usize, Color32),
    Check(Pos2, bool),
    /// Block `block`'s picture: the image and its halvings.
    Picture(Rect, usize, Image, Vec<Image>),
}

struct Link {
    rect: Rect,
    baseline: f32,
    url: String,
    text: String,
    /// Runs of one link share it (all underline under the pointer).
    group: usize,
}

/// A run of text marked out in a job: code (on a tint) or a link.
enum Mark {
    Code,
    Link(String),
}

/// The page's colours.
struct Inks {
    text: Color32,
    quote: Color32,
    dim: Color32,
    link: Color32,
    code_fill: Color32,
    head_fill: Color32,
    stripe: Color32,
    line: Color32,
    rule: Color32,
}

impl Inks {
    fn new(p: &Palette) -> Inks {
        // Toward the text from the paper: darker on light, lighter on dark.
        let tint = |t: f32| c(mix(p.document_background, p.document_text, t));
        Inks {
            text: c(p.document_text),
            quote: c(readable(mix(p.document_text, p.document_background, 0.3), p.document_background, 4.5)),
            dim: c(readable(mix(p.document_text, p.document_background, 0.45), p.document_background, 4.5)),
            link: style::syntax_color(p, Syntax::Link),
            code_fill: tint(0.055),
            head_fill: tint(0.035),
            stripe: tint(0.018),
            line: tint(0.14),
            rule: tint(0.12),
        }
    }
}

impl RichView {
    /// Show `blocks` (the document `id`'s; links relative to `base`) in `r`.
    pub fn show(&mut self, ui: &mut Ui, r: Rect, blocks: &[Block], id: u64, base: Option<&Path>) {
        let p = style::palette(ui.ctx());
        let ppp = ui.ctx().pixels_per_point();
        let scroll_id = ("ql-rich", id);
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(r));
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let blocks = &blocks[..blocks.len().min(MAX_BLOCKS)];
        let mut go_to = None;
        ScrollView::new(scroll_id).border(false).background(p.document_background).show_in(&mut ui, r, |ui, view| {
            let width = ui.available_width();
            let key = Key { id, width, ppp, palette: (*p).clone(), generation: ui.ctx().fonts(|f| f.galley_generation()) };
            if self.key.as_ref() != Some(&key) {
                self.restart(key);
            }
            // Down to the bottom of the view (if the frame has time: one
            // dragged far into a long document waits a few frames), then
            // more while it has.
            let start = Instant::now();
            let ink = Inks::new(&p);
            while self.at.next < blocks.len() && (self.at.y < view.bottom() && start.elapsed() < IN_VIEW || start.elapsed() < BUDGET) {
                self.lay(ui, &p, &ink, blocks, width);
            }
            if self.at.next < blocks.len() {
                ui.ctx().request_repaint();
            }
            let rest = (blocks.len() - self.at.next) as f32 * self.average;
            let (area, _) = ui.allocate_exact_size(vec2(width, self.at.y + rest + MARGIN), Sense::hover());
            // The page's origin on the device grid, so what was laid out
            // on it stays there.
            let o = (area.min.to_vec2() * ppp).round() / ppp;
            go_to = self.paint(ui, &p, o, view, base);
            // What isn't laid out yet, in view: it's coming.
            if self.at.next < blocks.len() && self.at.y < view.bottom() {
                let pending = Rect::from_min_max(pos2(view.min.x, self.at.y.max(view.min.y)), view.max).translate(o);
                ui.put(Rect::from_center_size(pending.center(), vec2(20.0, 20.0)), egui::Spinner::new().size(18.0).color(style::disabled_text(&p)));
            }
        });
        if let Some(y) = go_to {
            ScrollView::scroll_to(ui.ctx(), scroll_id, vec2(0.0, (y - GAP).max(0.0)));
        }
    }

    /// Lay out again from the start (for another width, scale or look).
    fn restart(&mut self, key: Key) {
        if !self.laid.is_empty() {
            self.average = self.at.y / self.laid.len() as f32;
        }
        if self.key.as_ref().is_none_or(|k| k.id != key.id) {
            self.textures.clear();
            self.average = 0.0;
        }
        self.laid.clear();
        self.bars.clear();
        self.at = Cursor { y: MARGIN, ..Default::default() };
        self.key = Some(key);
    }

    /// Lay out the next block.
    fn lay(&mut self, ui: &Ui, p: &Palette, ink: &Inks, blocks: &[Block], width: f32) {
        let i = self.at.next;
        self.at.next += 1;
        let ppp = ui.ctx().pixels_per_point();
        let snap = |v: f32| (v * ppp).round() / ppp;
        let column = (width - 2.0 * MARGIN).clamp(120.0, MEASURE);
        let left = snap(((width - column) / 2.0).max(MARGIN));
        let q = self.at.quotes.len() as f32;
        let x0 = left + q * QUOTE_INDENT;
        let w = column - q * QUOTE_INDENT;
        let quoted = self.at.quotes.iter().any(|(_, _, callout)| !callout);
        let text_ink = if quoted { ink.quote } else { ink.text };
        let kind = match &blocks[i] {
            Block::Heading(l, _) => Kind::Heading((*l).clamp(1, 6)),
            Block::Para(_) => Kind::Para,
            Block::Item(_, mark, _) if mark.is_empty() => Kind::ItemMore,
            Block::Item(..) => Kind::Item,
            Block::Code { depth, .. } => Kind::Code(*depth > 0),
            Block::QuoteStart(callout) => Kind::QuoteStart(callout.is_some()),
            Block::QuoteEnd => Kind::QuoteEnd,
            _ => Kind::Other,
        };
        let top = snap(self.at.y + space(self.at.prev, kind));
        use egui::accesskit::Role;
        let role = match &blocks[i] {
            Block::Heading(..) => Role::Heading,
            Block::Item(..) => Role::ListItem,
            Block::Code { .. } => Role::Code,
            Block::Table { .. } => Role::Table,
            Block::Picture { .. } => Role::Image,
            _ => Role::Paragraph,
        };
        let mut laid = Laid { top, bottom: top, draws: Vec::new(), links: Vec::new(), text: String::new(), role, slug: None };
        let mut y = top;
        match &blocks[i] {
            Block::Heading(level, spans) => {
                let l = (*level).clamp(1, 6) as usize;
                let size = HEADING_SIZE[l - 1];
                let color = if l == 6 { ink.dim } else { text_ink };
                let (job, marks) = text_job(spans, size, (size * 1.3).round(), true, color, ink, w);
                y += place(ui, &mut laid, job, marks, pos2(x0, y), ink);
                if l <= 2 {
                    y += 6.0;
                    laid.draws.push(Draw::HLine { x0, x1: x0 + w, y: snap(y), ends: false, color: ink.rule });
                    y += 1.0;
                }
                laid.slug = Some(slug(&plain(spans)));
            }
            Block::Para(spans) => {
                let (job, marks) = text_job(spans, BODY, LEADING, false, text_ink, ink, w);
                y += place(ui, &mut laid, job, marks, pos2(x0, y), ink);
            }
            Block::Item(depth, mark, spans) => {
                let text_x = x0 + (*depth as f32 + 1.0) * INDENT;
                let (job, marks) = text_job(spans, BODY, LEADING, false, text_ink, ink, x0 + w - text_x);
                let galley = ui.painter().layout_job(job);
                // The mark sits on the first line: centred on its capitals.
                let first = galley.rows.first().and_then(|r| r.glyphs.first().map(|g| r.pos.y + g.pos.y)).unwrap_or(LEADING * 0.7);
                let middle = y + first - BODY * CAP / 2.0;
                match mark.as_str() {
                    "" => {}
                    "•" | "◦" | "▪" => {
                        let level = ["•", "◦", "▪"].iter().position(|b| b == mark).unwrap_or(0);
                        laid.draws.push(Draw::Bullet(pos2(text_x - 12.0, middle), level, text_ink));
                    }
                    "☐" | "☑" => laid.draws.push(Draw::Check(pos2(text_x - 14.0, middle), mark == "☑")),
                    number => {
                        let mut job = LayoutJob::default();
                        job.append(number, 0.0, format(FontFamily::Proportional, BODY, LEADING, text_ink));
                        let g = ui.painter().layout_job(job);
                        laid.draws.push(Draw::Text(pos2(snap(text_x - 7.0 - g.size().x), y), g));
                    }
                }
                y += place_galley(&mut laid, galley, &marks, pos2(text_x, y), ink);
            }
            Block::Code { lines, tokens, depth } => {
                let x = x0 + *depth as f32 * INDENT;
                let w = x0 + w - x;
                let mut job = LayoutJob::default();
                for (n, line) in lines.iter().enumerate() {
                    let mut at = 0;
                    for &(a, b, tok) in tokens.get(n).map_or(&[][..], Vec::as_slice) {
                        let (a, b) = (a as usize, b as usize);
                        if a > at && line.is_char_boundary(a) {
                            job.append(&line[at..a], 0.0, format(FontFamily::Monospace, MONO, CODE_LEADING, ink.text));
                        }
                        if line.is_char_boundary(a) && line.is_char_boundary(b) && b > a {
                            job.append(&line[a..b], 0.0, format(FontFamily::Monospace, MONO, CODE_LEADING, style::syntax_color(p, tok)));
                            at = b;
                        }
                    }
                    let tail = if n + 1 < lines.len() { format!("{}\n", &line[at.min(line.len())..]) } else { line[at.min(line.len())..].to_owned() };
                    job.append(&tail, 0.0, format(FontFamily::Monospace, MONO, CODE_LEADING, ink.text));
                }
                job.wrap.max_width = w - 2.0 * CODE_PAD.x;
                let g = ui.painter().layout_job(job);
                let h = g.size().y.max(CODE_LEADING) + 2.0 * CODE_PAD.y;
                laid.draws.push(Draw::Fill(Rect::from_min_size(pos2(x, y), vec2(w, snap(h))), 6.0, ink.code_fill));
                laid.text = lines.join("\n");
                laid.draws.push(Draw::Text(pos2(x + CODE_PAD.x, y + CODE_PAD.y), g));
                y += snap(h);
            }
            Block::Table { align, rows } => y += table(ui, &mut laid, align, rows, pos2(x0, y), w, ink, ppp),
            Block::Picture { image, smaller, alt } => {
                // A texture pixel to a device pixel, unless too wide.
                let natural = vec2(image.size[0] as f32, image.size[1] as f32) / ppp;
                let size = if natural.x > w { natural * (w / natural.x) } else { natural };
                let rect = Rect::from_min_size(pos2(x0, y), vec2(snap(size.x), snap(size.y)));
                laid.draws.push(Draw::Picture(rect, i, image.clone(), smaller.clone()));
                laid.text = alt.clone();
                y += rect.height();
            }
            Block::Rule => {
                laid.draws.push(Draw::HLine { x0, x1: x0 + w, y: snap(y + 12.0), ends: false, color: ink.rule });
                y += 25.0;
            }
            Block::QuoteStart(callout) => {
                let color = match callout {
                    Some(k) => c(p.hue(callout_hue(*k))),
                    None => ink.line,
                };
                self.at.quotes.push((top, color, callout.is_some()));
                if let Some(k) = callout {
                    let mut job = LayoutJob::default();
                    job.append(k.title(), 0.0, format(style::bold(), BODY, LEADING, c(p.hue_text(callout_hue(*k)))));
                    let g = ui.painter().layout_job(job);
                    laid.text = k.title().into();
                    laid.draws.push(Draw::Text(pos2(x0 + QUOTE_INDENT, y), g));
                    y += LEADING;
                }
            }
            Block::QuoteEnd => {
                if let Some((from, color, _)) = self.at.quotes.pop() {
                    let x = left + self.at.quotes.len() as f32 * QUOTE_INDENT;
                    self.bars.push((Rect::from_min_max(pos2(x, from), pos2(x + QUOTE_BAR, snap(self.at.y))), color));
                }
                laid.top = self.at.y;
                y = self.at.y;
            }
        }
        laid.bottom = y;
        if laid.text.is_empty()
            && let Block::Heading(_, s) | Block::Para(s) | Block::Item(_, _, s) = &blocks[i]
        {
            laid.text = plain(s);
        }
        self.at.y = y;
        self.at.prev = Some(kind);
        self.laid.push(laid);
    }

    /// Draw what is in `view` (page coordinates) with the page's origin
    /// at `o`; where to scroll to, if a `#link` was clicked.
    fn paint(&mut self, ui: &mut Ui, p: &Palette, o: Vec2, view: Rect, base: Option<&Path>) -> Option<f32> {
        let ink = Inks::new(p);
        let painter = ui.painter().clone();
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        for (r, color) in &self.bars {
            if r.max.y >= view.min.y && r.min.y <= view.max.y {
                painter.rect_filled(style::snap(&ctx, r.translate(o)), 0.0, *color);
            }
        }
        // Blocks in view: found by their tops (in order).
        let first = self.laid.partition_point(|l| l.bottom < view.min.y);
        let mut go_to = None;
        let mut hovered = None;
        for (k, laid) in self.laid[first..].iter().enumerate() {
            if laid.top > view.max.y {
                break;
            }
            for d in &laid.draws {
                match d {
                    Draw::Text(at, g) => painter.galley(*at + o, g.clone(), ink.text),
                    Draw::Fill(r, radius, color) => {
                        painter.rect_filled(style::snap(&ctx, r.translate(o)), *radius, *color);
                    }
                    Draw::HLine { x0, x1, y, ends, color } => {
                        let line = if *ends { style::hline_above } else { style::hline };
                        line(&painter, x0 + o.x, x1 + o.x, y + o.y, *color);
                    }
                    Draw::VLine { x, y0, y1, ends, color } => {
                        let line = if *ends { style::vline_left_of } else { style::vline };
                        line(&painter, x + o.x, y0 + o.y, y1 + o.y, *color);
                    }
                    Draw::Bullet(at, level, color) => bullet(&painter, *at + o, *level, *color, ppp),
                    Draw::Check(at, done) => {
                        let well = style::snap_centred(&ctx, *at + o, vec2(olo_kit::metrics::MARK_SIZE, olo_kit::metrics::MARK_SIZE));
                        widgets::check_box_look(&painter, well, p, *done);
                    }
                    Draw::Picture(r, block, image, smaller) => picture(ui, &mut self.textures, (*block, image, smaller), r.translate(o)),
                }
            }
            let r = Rect::from_min_max(pos2(view.min.x, laid.top), pos2(view.max.x, laid.bottom)).translate(o);
            style::describe_text(&ctx, laid.role, &laid.text, r.intersect(ui.clip_rect()), None);
            // Links take clicks (and agents see them as links).
            for (n, link) in laid.links.iter().enumerate() {
                let resp = ui.interact(link.rect.translate(o), ui.scope_id().with(("link", first + k, n)), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &link.text));
                if resp.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    hovered = Some((first + k, link.group));
                }
                if resp.clicked() {
                    go_to = go_to.or(self.follow(&link.url, base));
                }
            }
        }
        // The link under the pointer is underlined, all of it.
        if let Some((b, group)) = hovered {
            for link in self.laid[b].links.iter().filter(|l| l.group == group) {
                let y = (((link.baseline + o.y) * ppp).round() + 1.0) / ppp;
                style::hline(&painter, link.rect.min.x + o.x, link.rect.max.x + o.x, y, ink.link);
            }
        }
        go_to
    }

    /// Open `url` (a web address, or a file beside the document); a
    /// `#heading` gives where it is on the page.
    fn follow(&self, url: &str, base: Option<&Path>) -> Option<f32> {
        if let Some(anchor) = url.strip_prefix('#') {
            let anchor = anchor.to_lowercase();
            return self.laid.iter().find(|l| l.slug.as_deref() == Some(anchor.as_str())).map(|l| l.top);
        }
        let target = if url.contains("://") || url.starts_with("mailto:") {
            olo_kit::open::Target::parse(url)
        } else {
            let path = url.split('#').next().unwrap_or(url);
            match base {
                Some(dir) if !Path::new(path).is_absolute() => olo_kit::open::Target::Path(dir.join(path)),
                _ => olo_kit::open::Target::parse(path),
            }
        };
        std::thread::spawn(move || {
            if let Err(e) = olo_kit::open::open(&target) {
                tracing::warn!("quick look: can't open a link: {e}");
            }
        });
        None
    }

}

/// Block `i`'s picture in `r`: from the smallest halving still as big as
/// it shows (none smaller, so it never comes out soft).
fn picture(ui: &Ui, textures: &mut HashMap<(usize, usize), TextureHandle>, (i, image, smaller): (usize, &Image, &[Image]), r: Rect) {
    let want = r.width() * ui.ctx().pixels_per_point();
    let level = smaller.iter().rposition(|s| s.size[0] as f32 >= want - 0.5).map_or(0, |n| n + 1);
    let img = if level == 0 { image } else { &smaller[level - 1] };
    let tex = textures
        .entry((i, level))
        .or_insert_with(|| ui.ctx().load_texture(format!("ql-picture-{i}-{level}"), crate::view::color_image(img), TextureOptions::LINEAR));
    ui.painter().image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

/// The room above a block of `kind` after one of `prev`.
fn space(prev: Option<Kind>, kind: Kind) -> f32 {
    match (prev, kind) {
        (None, _) | (_, Kind::QuoteEnd) | (Some(Kind::QuoteStart(false)), _) => 0.0,
        (Some(Kind::QuoteStart(true)), _) => 4.0,
        (_, Kind::Heading(l)) => HEADING_ABOVE[l as usize - 1],
        (Some(Kind::Heading(l)), _) => if l <= 2 { 14.0 } else { 8.0 },
        (Some(Kind::Item | Kind::ItemMore), Kind::Item) => ITEM_GAP,
        (Some(Kind::Item | Kind::ItemMore), Kind::ItemMore | Kind::Code(true)) => 8.0,
        _ => GAP,
    }
}

fn format(family: FontFamily, size: f32, leading: f32, color: Color32) -> egui::TextFormat {
    let mut f = egui::TextFormat::simple(FontId::new(size, family), color);
    f.line_height = Some(leading);
    f
}

/// `spans` as text `width` wide (`bold`: all of it, as a heading), and
/// where its code and links are (character ranges).
fn text_job(spans: &[Span], size: f32, leading: f32, bold: bool, color: Color32, ink: &Inks, width: f32) -> (LayoutJob, Vec<(Range<usize>, Mark)>) {
    let mut job = LayoutJob::default();
    let mut marks = Vec::new();
    let mut chars = 0;
    for s in spans {
        let b = bold || s.bold;
        let color = if s.link { ink.link } else { color };
        let (family, size) = match (s.code, b, s.italic) {
            (true, true, _) => (style::mono_bold(), size - 1.0),
            (true, false, _) => (FontFamily::Monospace, size - 1.0),
            (false, true, _) => (style::bold(), size),
            (false, false, true) => (style::italic(), size),
            _ => (FontFamily::Proportional, size),
        };
        // Code gets a thin space either side, for its tint: a no-break
        // space set small (a line mustn't break between "(" and code).
        let pad = |job: &mut LayoutJob, chars: &mut usize| {
            if s.code && *chars > 0 {
                job.append("\u{a0}", 0.0, format(FontFamily::Proportional, size * 0.75, leading, color));
                *chars += 1;
            }
        };
        pad(&mut job, &mut chars);
        let mut f = format(family, size, leading, color);
        if s.strike {
            f.strikethrough = egui::Stroke::new(1.0, color);
        }
        job.append(&s.text, 0.0, f);
        let n = s.text.chars().count();
        if s.code {
            marks.push((chars..chars + n, Mark::Code));
        } else if let Some(url) = &s.url {
            // One link split by its styles is still one.
            match marks.last_mut() {
                Some((r, Mark::Link(u))) if u == url && r.end == chars => r.end += n,
                _ => marks.push((chars..chars + n, Mark::Link(url.clone()))),
            }
        }
        chars += n;
        if s.code {
            job.append("\u{a0}", 0.0, format(FontFamily::Proportional, size * 0.75, leading, color));
            chars += 1;
        }
    }
    job.wrap.max_width = width;
    (job, marks)
}

/// Lay `job` out at `at`; its height.
fn place(ui: &Ui, laid: &mut Laid, job: LayoutJob, marks: Vec<(Range<usize>, Mark)>, at: Pos2, ink: &Inks) -> f32 {
    let g = ui.painter().layout_job(job);
    place_galley(laid, g, &marks, at, ink)
}

/// Draw `g` at `at`, its code on a tint and its links taking clicks; its
/// height.
fn place_galley(laid: &mut Laid, g: Arc<Galley>, marks: &[(Range<usize>, Mark)], at: Pos2, ink: &Inks) -> f32 {
    let h = g.size().y;
    let groups = laid.links.last().map_or(0, |l| l.group + 1);
    for (rect, baseline, m) in runs(&g, marks) {
        let (rect, baseline) = (rect.translate(at.to_vec2()), baseline + at.y);
        match &marks[m].1 {
            Mark::Code => laid.draws.push(Draw::Fill(rect.expand2(vec2(2.0, 0.5)), 3.0, ink.code_fill)),
            Mark::Link(url) => {
                let text: String = g.text().chars().skip(marks[m].0.start).take(marks[m].0.len()).collect();
                laid.links.push(Link { rect, baseline, url: url.clone(), text, group: groups + m });
            }
        }
    }
    laid.draws.push(Draw::Text(at, g));
    h
}

/// Where in `g` each marked range is: a rect per row it is on, its
/// baseline and which mark.
fn runs(g: &Galley, marks: &[(Range<usize>, Mark)]) -> Vec<(Rect, f32, usize)> {
    let mut out: Vec<(Rect, f32, usize)> = Vec::new();
    if marks.is_empty() {
        return out;
    }
    let mut at = 0;
    for (row_n, row) in g.rows.iter().enumerate() {
        let mut open = false;
        for glyph in &row.glyphs {
            let m = marks.iter().position(|(r, _)| r.contains(&at));
            let x = row.pos.x + glyph.pos.x;
            match (m, out.last_mut()) {
                (Some(m), Some((r, _, k))) if open && *k == m => r.max.x = x + glyph.advance_width,
                (Some(m), _) => {
                    let top = row.pos.y + glyph.pos.y - glyph.font_ascent;
                    out.push((Rect::from_min_size(pos2(x, top), vec2(glyph.advance_width, glyph.font_height)), row.pos.y + glyph.pos.y, m));
                    open = true;
                }
                (None, _) => open = false,
            }
            at += 1;
        }
        if row.ends_with_newline {
            at += 1;
        }
        let _ = row_n;
    }
    out
}

/// A table `w` wide at most at `at`; its height. Columns are as wide as
/// their widest cell, or share the width by it when that's too much.
#[allow(clippy::too_many_arguments)]
fn table(ui: &Ui, laid: &mut Laid, align: &[Align], rows: &[Row], at: Pos2, w: f32, ink: &Inks, ppp: f32) -> f32 {
    let snap = |v: f32| (v * ppp).round() / ppp;
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return 0.0;
    }
    let natural: Vec<f32> = (0..cols)
        .map(|k| {
            rows.iter()
                .enumerate()
                .filter_map(|(n, r)| r.get(k).map(|cell| (n, cell)))
                .map(|(n, cell)| ui.painter().layout_job(text_job(cell, BODY, LEADING, n == 0, ink.text, ink, f32::INFINITY).0).size().x)
                .fold(0.0, f32::max)
                .ceil()
                + 2.0 * CELL_PAD.x
        })
        .collect();
    let total: f32 = natural.iter().sum();
    let widths: Vec<f32> = if total <= w { natural } else { natural.iter().map(|n| (n * w / total).max(48.0).floor()).collect() };
    let mut xs = vec![at.x];
    for cw in &widths {
        xs.push(snap(xs.last().unwrap() + cw));
    }
    let right = *xs.last().unwrap();
    let mut y = at.y;
    let mut text = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        let mut cells = Vec::new();
        let mut h: f32 = LEADING;
        for k in 0..cols {
            let spans = row.get(k).map_or(&[][..], Vec::as_slice);
            let (job, marks) = text_job(spans, BODY, LEADING, n == 0, ink.text, ink, xs[k + 1] - xs[k] - 2.0 * CELL_PAD.x);
            let g = ui.painter().layout_job(job);
            h = h.max(g.size().y);
            cells.push((g, marks));
        }
        let row_h = snap(h + 2.0 * CELL_PAD.y);
        let fill = if n == 0 { Some(ink.head_fill) } else if n % 2 == 0 { Some(ink.stripe) } else { None };
        if let Some(fill) = fill {
            laid.draws.push(Draw::Fill(Rect::from_min_max(pos2(at.x, y), pos2(right, y + row_h)), 0.0, fill));
        }
        for (k, (g, marks)) in cells.into_iter().enumerate() {
            let room = xs[k + 1] - xs[k] - 2.0 * CELL_PAD.x;
            let x = match align.get(k).copied().unwrap_or_default() {
                Align::Left => xs[k] + CELL_PAD.x,
                Align::Center => xs[k] + CELL_PAD.x + ((room - g.size().x) / 2.0).max(0.0),
                Align::Right => xs[k + 1] - CELL_PAD.x - g.size().x.min(room),
            };
            place_galley(laid, g, &marks, pos2(snap(x), y + CELL_PAD.y), ink);
        }
        text.push(row.iter().map(|c| plain(c)).collect::<Vec<_>>().join("\t"));
        laid.draws.push(Draw::HLine { x0: at.x, x1: right, y, ends: false, color: ink.line });
        y += row_h;
    }
    laid.draws.push(Draw::HLine { x0: at.x, x1: right, y, ends: true, color: ink.line });
    for (k, &x) in xs.iter().enumerate() {
        laid.draws.push(Draw::VLine { x, y0: at.y, y1: y, ends: k == cols, color: ink.line });
    }
    laid.text = text.join("\n");
    y - at.y
}

/// A list's bullet at `at`: a disc, then a ring, then a square, by level.
fn bullet(painter: &egui::Painter, at: Pos2, level: usize, color: Color32, ppp: f32) {
    let grid = |v: f32| (v * ppp).round() / ppp;
    let at = pos2(grid(at.x), grid(at.y));
    match level % 3 {
        0 => painter.circle_filled(at, 2.5, color),
        1 => painter.circle_stroke(at, 2.25, egui::Stroke::new(1.0, color)),
        _ => painter.rect_filled(style::snap_centred(painter.ctx(), at, vec2(4.5, 4.5)), 0.0, color),
    };
}

/// An alert's colour.
fn callout_hue(k: Callout) -> Hue {
    match k {
        Callout::Note => Hue::Blue,
        Callout::Tip => Hue::Green,
        Callout::Important => Hue::Purple,
        Callout::Warning => Hue::Orange,
        Callout::Caution => Hue::Red,
    }
}

fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// A heading's anchor, as GitHub makes them: lower case, spaces to
/// hyphens, punctuation gone.
fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|ch| match ch {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Getting started"), "getting-started");
        assert_eq!(slug("What's new in 2.0?"), "whats-new-in-20");
    }

    #[test]
    fn room_between() {
        assert_eq!(space(None, Kind::Heading(1)), 0.0);
        assert_eq!(space(Some(Kind::Para), Kind::Heading(2)), 24.0);
        assert_eq!(space(Some(Kind::Item), Kind::Item), ITEM_GAP);
        assert_eq!(space(Some(Kind::QuoteStart(false)), Kind::Para), 0.0);
    }
}
