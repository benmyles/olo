//! Formatted documents set as a page: a
//! measure of at most `MEASURE`, centred; body text on a 20-point line;
//! headings with room above them (a rule under the first two levels);
//! lists with hanging indents, their numbers lined up on the right and
//! tasks as check boxes; quotes and GitHub's alerts beside a bar; code in
//! a box, coloured by its language; tables as wide as what they hold; the
//! document's own pictures, a texture pixel to a device pixel. Links open
//! (a `#heading` scrolls to it), underlined under the pointer.
//!
//! Blocks are laid out once for a width, scale and look (GPUI's shaped,
//! wrapped lines, painted again as they are): those in view at once, the
//! rest a few milliseconds a frame after; only those in view are drawn.

use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use olo_theme::{Hue, Palette, mix, readable};
use olo_ui::{
    agent,
    gpui::{
        self, AppContext as _, Bounds, Context, CursorStyle, Font, FontStyle, FontWeight, Hitbox, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
        MouseMoveEvent, ParentElement as _, Pixels, Point, Render, RenderImage, ScrollHandle, SharedString, StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, Task, TextAlign, TextRun,
        Window, WrappedLine, canvas, div, point, px, size,
    },
    style, theme,
    widgets::{ScrollView, image_view},
};
use smallvec::SmallVec;

use crate::{
    doc::{Align, Block, Callout, Row, Span},
    load::Image,
    text::Tok,
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
const CELL_PAD: (f32, f32) = (10.0, 6.0);
const CODE_PAD: (f32, f32) = (12.0, 10.0);
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

/// A rectangle on the page (points from its top left).
#[derive(Clone, Copy, Debug, Default)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn on(self, o: Point<Pixels>) -> Bounds<Pixels> {
        Bounds::new(point(o.x + px(self.x), o.y + px(self.y)), size(px(self.w), px(self.h)))
    }
}

#[derive(Default)]
pub struct RichView {
    blocks: Arc<Vec<Block>>,
    generation: u64,
    base: Option<PathBuf>,
    scroll: ScrollHandle,
    laid: Vec<Laid>,
    /// Quote bars, as they close.
    bars: Vec<(Rect, Hsla)>,
    at: Cursor,
    /// What it is laid out for.
    key: Option<Key>,
    /// The height of a block on average when last laid out: for those not
    /// yet laid out again.
    average: f32,
    /// Pictures made for their device size: by block and size.
    pictures: HashMap<(usize, (u32, u32)), Arc<RenderImage>>,
    making: HashMap<(usize, (u32, u32)), Task<()>>,
    /// The link under the pointer (its block and group).
    hover: Option<(usize, usize)>,
    /// Where the page was last drawn (its origin) and the view.
    origin: Point<Pixels>,
    /// The text selected: where it began, where it goes (or the whole
    /// document's, Select All).
    sel: Option<(At, At)>,
    all: bool,
    dragging: bool,
    /// Find: what's looked for (lower case), where it is in what's laid
    /// out (found again as more is), which is shown.
    query: String,
    matches: Vec<(At, At)>,
    current: Option<usize>,
    found_upto: usize,
}

/// A place in the text: a block, one of its texts (`Draw::Text`), a
/// line of that, a byte in it.
type At = (usize, usize, usize, usize);

#[derive(Clone, PartialEq)]
struct Key {
    generation: u64,
    width: u32,
    scale: u32,
    palette: Palette,
}

/// How far the laying out has gone.
#[derive(Default)]
struct Cursor {
    next: usize,
    /// The bottom of what is laid out.
    y: f32,
    prev: Option<Kind>,
    /// Open quotes: where each began, its bar's colour, whether a callout.
    quotes: Vec<(f32, Hsla, bool)>,
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
    role: &'static str,
    /// A heading's anchor (`#getting-started`).
    slug: Option<String>,
}

enum Draw {
    Text {
        at: (f32, f32),
        /// (Boxed: a wrapped line keeps its decoration runs inline, about
        /// 2.5 KB, and unboxed every draw of a document, a fill or a rule
        /// too, was that big.)
        lines: Box<SmallVec<[WrappedLine; 1]>>,
        leading: f32,
    },
    Fill(Rect, f32, Hsla),
    /// A one-point line: horizontal (from x0 to x1 at y) or vertical, its
    /// top or left edge at the place given, or (`ends`) its bottom or right.
    HLine {
        x0: f32,
        x1: f32,
        y: f32,
        ends: bool,
        color: Hsla,
    },
    VLine {
        x: f32,
        y0: f32,
        y1: f32,
        ends: bool,
        color: Hsla,
    },
    Bullet((f32, f32), usize, Hsla),
    Check((f32, f32), bool),
    /// Block `block`'s picture.
    Picture(Rect, usize, Image),
}

struct Link {
    rect: Rect,
    baseline: f32,
    url: String,
    text: String,
    /// Runs of one link share it (all underline under the pointer).
    group: usize,
}

/// A run of text marked out: code (on a tint) or a link.
enum Mark {
    Code,
    Link(String),
}

/// The page's colours.
struct Inks {
    text: Hsla,
    quote: Hsla,
    dim: Hsla,
    link: Hsla,
    code_fill: Hsla,
    head_fill: Hsla,
    stripe: Hsla,
    line: Hsla,
    rule: Hsla,
}

impl Inks {
    fn new(p: &Palette) -> Inks {
        // Toward the text from the paper: darker on light, lighter on dark.
        let tint = |t: f32| style::c(mix(p.document_background, p.document_text, t));
        Inks {
            text: style::c(p.document_text),
            quote: style::c(readable(mix(p.document_text, p.document_background, 0.3), p.document_background, 4.5)),
            dim: style::c(readable(mix(p.document_text, p.document_background, 0.45), p.document_background, 4.5)),
            link: style::c(p.syntax(Tok::Link)),
            code_fill: tint(0.055),
            head_fill: tint(0.035),
            stripe: tint(0.018),
            line: tint(0.14),
            rule: tint(0.12),
        }
    }
}

/// Text and its runs, as a paragraph is set, with where its code and
/// links are (byte ranges).
struct Job {
    text: String,
    runs: Vec<TextRun>,
    marks: Vec<(Range<usize>, Mark)>,
    size: f32,
    leading: f32,
}

fn font(mono: bool, bold: bool, italic: bool) -> Font {
    let mut f = if mono { style::mono_font(if bold { FontWeight::BOLD } else { FontWeight::NORMAL }) } else { style::ui_font(if bold { FontWeight::BOLD } else { FontWeight::NORMAL }) };
    if italic && !mono {
        f.style = FontStyle::Italic;
    }
    f
}

/// `spans` set at `size` on a `leading` line (`bold`: all of it, as a
/// heading).
fn text_job(spans: &[Span], size: f32, leading: f32, bold: bool, color: Hsla, ink: &Inks) -> Job {
    let mut job = Job { text: String::new(), runs: Vec::new(), marks: Vec::new(), size, leading };
    let push = |job: &mut Job, text: &str, font: Font, color: Hsla, run_size: f32, strike: bool| {
        if text.is_empty() {
            return;
        }
        job.text.push_str(text);
        job.runs.push(TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: strike.then_some(StrikethroughStyle { thickness: px(1.0), color: Some(color) }),
            size: (run_size != size).then_some(px(run_size)),
        });
    };
    for s in spans {
        let b = bold || s.bold;
        let color = if s.link { ink.link } else { color };
        let f = font(s.code, b, s.italic);
        let run_size = if s.code { size - 1.0 } else { size };
        // Code gets a thin space either side, for its tint: a no-break
        // space set small (a line mustn't break between "(" and code).
        if s.code && !job.text.is_empty() {
            push(&mut job, "\u{a0}", font(false, false, false), color, size * 0.75, false);
        }
        let start = job.text.len();
        push(&mut job, &s.text, f, color, run_size, s.strike);
        let end = job.text.len();
        if s.code {
            job.marks.push((start..end, Mark::Code));
            push(&mut job, "\u{a0}", font(false, false, false), color, size * 0.75, false);
        } else if let Some(url) = &s.url {
            // One link split by its styles is still one.
            match job.marks.last_mut() {
                Some((r, Mark::Link(u))) if u == url && r.end == start => r.end = end,
                _ => job.marks.push((start..end, Mark::Link(url.clone()))),
            }
        }
    }
    job
}

fn shape(window: &mut Window, job: &Job, width: f32) -> SmallVec<[WrappedLine; 1]> {
    window.text_system().shape_text(job.text.clone().into(), px(job.size), &job.runs, Some(px(width.max(1.0))), None).unwrap_or_default()
}

/// The height of shaped `lines` on a `leading` line.
fn height(lines: &[WrappedLine], leading: f32) -> f32 {
    lines.iter().map(|l| f32::from(l.size(px(leading)).height)).sum::<f32>().max(if lines.is_empty() { 0.0 } else { leading })
}

/// Where each marked range of `lines` (one shaped paragraph) is: a rect
/// per row it's on (page points from the text's origin), its baseline,
/// and which mark.
fn mark_rects(lines: &[WrappedLine], marks: &[(Range<usize>, Mark)], job: &Job) -> Vec<(Rect, f32, usize)> {
    let mut out = Vec::new();
    if marks.is_empty() {
        return out;
    }
    let mut top = 0.0f32;
    let mut offset = 0usize;
    for line in lines {
        let l = &line.unwrapped_layout;
        let (ascent, descent) = (f32::from(l.ascent), f32::from(l.descent));
        let baseline = (job.leading - ascent - descent) / 2.0 + ascent;
        // Each row's first byte.
        let mut starts = vec![0usize];
        for b in line.wrap_boundaries() {
            if let Some(g) = l.runs.get(b.run_ix).and_then(|r| r.glyphs.get(b.glyph_ix)) {
                starts.push(g.index);
            }
        }
        let len = line.len();
        for (row, &s) in starts.iter().enumerate() {
            let e = starts.get(row + 1).copied().unwrap_or(len);
            let row_x = f32::from(l.x_for_index(s));
            for (m, (r, mark)) in marks.iter().enumerate() {
                let (a, b) = (r.start.max(offset + s), r.end.min(offset + e));
                if a >= b {
                    continue;
                }
                let x0 = f32::from(l.x_for_index(a - offset)) - row_x;
                let x1 = f32::from(l.x_for_index(b - offset)) - row_x;
                let y = top + row as f32 * job.leading;
                // A code span is a point smaller: its tint fits it.
                let k = if matches!(mark, Mark::Code) { (job.size - 1.0) / job.size } else { 1.0 };
                let (a_, d_) = (ascent * k, descent * k);
                out.push((Rect { x: x0, y: y + baseline - a_, w: x1 - x0, h: a_ + d_ }, y + baseline, m));
            }
        }
        top += f32::from(line.size(px(job.leading)).height);
        offset += len + 1;
    }
    out
}

impl RichView {
    /// Show `blocks` (document `generation`'s; links relative to `base`).
    pub fn show(&mut self, blocks: Arc<Vec<Block>>, generation: u64, base: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.generation != generation || !Arc::ptr_eq(&self.blocks, &blocks) {
            self.blocks = blocks;
            self.generation = generation;
            self.base = base;
            self.key = None;
            self.sel = None;
            self.all = false;
            self.scroll.set_offset(point(px(0.0), px(0.0)));
            cx.notify();
        }
    }

    /// Lay out again from the start (another width, scale or look).
    fn restart(&mut self, key: Key) {
        if !self.laid.is_empty() {
            self.average = self.at.y / self.laid.len() as f32;
        }
        if self.key.as_ref().is_none_or(|k| k.generation != key.generation) {
            self.pictures.clear();
            self.making.clear();
            self.average = 0.0;
        }
        self.laid.clear();
        self.bars.clear();
        self.at = Cursor { y: MARGIN, ..Default::default() };
        self.hover = None;
        self.key = Some(key);
    }

    /// Lay out the next block.
    fn lay(&mut self, window: &mut Window, p: &Palette, ink: &Inks, width: f32) {
        let blocks = self.blocks.clone();
        let i = self.at.next;
        self.at.next += 1;
        let s = window.scale_factor();
        let snap = |v: f32| (v * s).round() / s;
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
        let role = match &blocks[i] {
            Block::Heading(..) => "Heading",
            Block::Item(..) => "ListItem",
            Block::Code { .. } => "Code",
            Block::Table { .. } => "Table",
            Block::Picture { .. } => "Image",
            _ => "Paragraph",
        };
        let mut laid = Laid { top, bottom: top, draws: Vec::new(), links: Vec::new(), text: String::new(), role, slug: None };
        let mut y = top;
        match &blocks[i] {
            Block::Heading(level, spans) => {
                let l = (*level).clamp(1, 6) as usize;
                let size = HEADING_SIZE[l - 1];
                let color = if l == 6 { ink.dim } else { text_ink };
                let job = text_job(spans, size, (size * 1.3).round(), true, color, ink);
                y += place(window, &mut laid, &job, (x0, y), w, ink);
                if l <= 2 {
                    y += 6.0;
                    laid.draws.push(Draw::HLine { x0, x1: x0 + w, y: snap(y), ends: false, color: ink.rule });
                    y += 1.0;
                }
                laid.slug = Some(slug(&plain(spans)));
            }
            Block::Para(spans) => {
                let job = text_job(spans, BODY, LEADING, false, text_ink, ink);
                y += place(window, &mut laid, &job, (x0, y), w, ink);
            }
            Block::Item(depth, mark, spans) => {
                let text_x = x0 + (*depth as f32 + 1.0) * INDENT;
                let job = text_job(spans, BODY, LEADING, false, text_ink, ink);
                // The mark sits on the first line: centred on its capitals.
                let lines = shape(window, &job, x0 + w - text_x);
                let first = lines.first().map(|l| {
                    let u = &l.unwrapped_layout;
                    (LEADING - f32::from(u.ascent) - f32::from(u.descent)) / 2.0 + f32::from(u.ascent)
                });
                let middle = y + first.unwrap_or(LEADING * 0.7) - BODY * CAP / 2.0;
                match mark.as_str() {
                    "" => {}
                    "•" | "◦" | "▪" => {
                        let level = ["•", "◦", "▪"].iter().position(|b| b == mark).unwrap_or(0);
                        laid.draws.push(Draw::Bullet((text_x - 12.0, middle), level, text_ink));
                    }
                    "☐" | "☑" => laid.draws.push(Draw::Check((text_x - 14.0, middle), mark == "☑")),
                    number => {
                        let run = TextRun { len: number.len(), font: font(false, false, false), color: text_ink, background_color: None, underline: None, strikethrough: None, size: None };
                        let n = window.text_system().shape_text(number.to_owned().into(), px(BODY), &[run], None, None).unwrap_or_default();
                        let nw = n.first().map_or(0.0, |l| f32::from(l.width()));
                        laid.draws.push(Draw::Text { at: (snap(text_x - 7.0 - nw), y), lines: Box::new(n), leading: LEADING });
                    }
                }
                y += place_lines(&mut laid, lines, &job, (text_x, y), ink);
            }
            Block::Code { lines, tokens, depth } => {
                let x = x0 + *depth as f32 * INDENT;
                let w = x0 + w - x;
                let mono = font(true, false, false);
                let mut job = Job { text: String::new(), runs: Vec::new(), marks: Vec::new(), size: MONO, leading: CODE_LEADING };
                let push = |job: &mut Job, t: &str, color: Hsla| {
                    if !t.is_empty() {
                        job.text.push_str(t);
                        job.runs.push(TextRun { len: t.len(), font: mono.clone(), color, background_color: None, underline: None, strikethrough: None, size: None });
                    }
                };
                for (n, line) in lines.iter().enumerate() {
                    let mut at = 0;
                    for &(a, b, tok) in tokens.get(n).map_or(&[][..], Vec::as_slice) {
                        let (a, b) = (a as usize, (b as usize).min(line.len()));
                        if a > at && line.is_char_boundary(at) && line.is_char_boundary(a) {
                            push(&mut job, &line[at..a], ink.text);
                        }
                        if line.is_char_boundary(a) && line.is_char_boundary(b) && b > a && a >= at {
                            push(&mut job, &line[a..b], style::c(p.syntax(tok)));
                            at = b;
                        }
                    }
                    push(&mut job, &line[at.min(line.len())..], ink.text);
                    if n + 1 < lines.len() {
                        push(&mut job, "\n", ink.text);
                    }
                }
                let shaped = shape(window, &job, w - 2.0 * CODE_PAD.0);
                let h = height(&shaped, CODE_LEADING).max(CODE_LEADING) + 2.0 * CODE_PAD.1;
                laid.draws.push(Draw::Fill(Rect { x, y, w, h: snap(h) }, 6.0, ink.code_fill));
                laid.text = lines.join("\n");
                laid.draws.push(Draw::Text { at: (x + CODE_PAD.0, y + CODE_PAD.1), lines: Box::new(shaped), leading: CODE_LEADING });
                y += snap(h);
            }
            Block::Table { align, rows } => y += table(window, &mut laid, align, rows, (x0, y), w, ink, s),
            Block::Picture { image, alt, .. } => {
                // A texture pixel to a device pixel, unless too wide.
                let (nw, nh) = (image.size[0] as f32 / s, image.size[1] as f32 / s);
                let (pw, ph) = if nw > w { (w, nh * w / nw) } else { (nw, nh) };
                let rect = Rect { x: x0, y, w: snap(pw), h: snap(ph) };
                laid.draws.push(Draw::Picture(rect, i, image.clone()));
                laid.text = alt.clone();
                y += rect.h;
            }
            Block::Rule => {
                laid.draws.push(Draw::HLine { x0, x1: x0 + w, y: snap(y + 12.0), ends: false, color: ink.rule });
                y += 25.0;
            }
            Block::QuoteStart(callout) => {
                let color = match callout {
                    Some(k) => style::c(p.hue(callout_hue(*k))),
                    None => ink.line,
                };
                self.at.quotes.push((top, color, callout.is_some()));
                if let Some(k) = callout {
                    let title = k.title();
                    let run = TextRun {
                        len: title.len(),
                        font: font(false, true, false),
                        color: style::c(p.hue_text(callout_hue(*k))),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                        size: None,
                    };
                    let lines = window.text_system().shape_text(title.into(), px(BODY), &[run], None, None).unwrap_or_default();
                    laid.text = title.into();
                    laid.draws.push(Draw::Text { at: (x0 + QUOTE_INDENT, y), lines: Box::new(lines), leading: LEADING });
                    y += LEADING;
                }
            }
            Block::QuoteEnd => {
                if let Some((from, color, _)) = self.at.quotes.pop() {
                    let x = left + self.at.quotes.len() as f32 * QUOTE_INDENT;
                    self.bars.push((Rect { x, y: from, w: QUOTE_BAR, h: snap(self.at.y) - from }, color));
                }
                laid.top = self.at.y;
                y = self.at.y;
            }
        }
        laid.bottom = y;
        if laid.text.is_empty()
            && let Block::Heading(_, sp) | Block::Para(sp) | Block::Item(_, _, sp) = &blocks[i]
        {
            laid.text = plain(sp);
        }
        self.at.y = y;
        self.at.prev = Some(kind);
        self.laid.push(laid);
    }

    /// Open `url` (a web address, or a file beside the document); a
    /// `#heading` gives where it is on the page.
    fn follow(&self, url: &str) -> Option<f32> {
        if let Some(anchor) = url.strip_prefix('#') {
            let anchor = anchor.to_lowercase();
            return self.laid.iter().find(|l| l.slug.as_deref() == Some(anchor.as_str())).map(|l| l.top);
        }
        let target = if url.contains("://") || url.starts_with("mailto:") {
            olo_xdg::open::Target::parse(url)
        } else {
            let path = url.split('#').next().unwrap_or(url);
            match &self.base {
                Some(dir) if !Path::new(path).is_absolute() => olo_xdg::open::Target::Path(dir.join(path)),
                _ => olo_xdg::open::Target::parse(path),
            }
        };
        std::thread::spawn(move || {
            if let Err(e) = olo_xdg::open::open(&target) {
                tracing::warn!("a document's link: {e}");
            }
        });
        None
    }

    /// The link (block, link) at page point `at`.
    fn link_at(&self, at: (f32, f32)) -> Option<(usize, usize)> {
        let first = self.laid.partition_point(|l| l.bottom < at.1);
        for (k, laid) in self.laid[first..].iter().enumerate() {
            if laid.top > at.1 {
                break;
            }
            for (n, link) in laid.links.iter().enumerate() {
                let r = link.rect;
                if at.0 >= r.x && at.0 < r.x + r.w && at.1 >= r.y && at.1 < r.y + r.h {
                    return Some((first + k, n));
                }
            }
        }
        None
    }

    /// Lays out what the frame has time for (what's in view first), in
    /// prepaint: the hitboxes for the links in view.
    fn prepaint(&mut self, b: Bounds<Pixels>, p: &Palette, window: &mut Window) -> Vec<Hitbox> {
        let s = window.scale_factor();
        let width = f32::from(b.size.width);
        let key = Key { generation: self.generation, width: (width * 8.0).round() as u32, scale: (s * 120.0).round() as u32, palette: (*p).clone() };
        if self.key.as_ref() != Some(&key) {
            self.restart(key);
        }
        // The page's origin on the device grid, so what was laid out on
        // it stays there.
        let o = point(px((f32::from(b.origin.x) * s).round() / s), px((f32::from(b.origin.y) * s).round() / s));
        self.origin = o;
        let view = window.content_mask().bounds;
        let view_bottom = f32::from(view.bottom() - o.y);
        let n = self.blocks.len().min(MAX_BLOCKS);
        let start = Instant::now();
        let ink = Inks::new(p);
        while self.at.next < n && ((self.at.y < view_bottom && start.elapsed() < IN_VIEW) || start.elapsed() < BUDGET) {
            self.lay(window, p, &ink, width);
        }
        if self.at.next < n {
            window.request_animation_frame();
        }
        // The links in view take the pointer.
        let (top, bottom) = (f32::from(view.top() - o.y), view_bottom);
        let first = self.laid.partition_point(|l| l.bottom < top);
        let mut hits = Vec::new();
        for laid in &self.laid[first..] {
            if laid.top > bottom {
                break;
            }
            for link in &laid.links {
                hits.push(window.insert_hitbox(link.rect.on(o), HitboxBehavior::Normal));
            }
        }
        hits
    }

    fn paint(&mut self, hits: Vec<Hitbox>, window: &mut Window, cx: &mut Context<Self>) {
        let p = theme::palette(cx);
        let ink = Inks::new(&p);
        let o = self.origin;
        let view = window.content_mask().bounds;
        let (top, bottom) = (f32::from(view.top() - o.y), f32::from(view.bottom() - o.y));
        for hit in &hits {
            window.set_cursor_style(CursorStyle::PointingHand, hit);
        }
        for (r, color) in &self.bars {
            if r.y + r.h >= top && r.y <= bottom {
                window.paint_quad(gpui::fill(style::snap(window, r.on(o)), *color));
            }
        }
        let s = window.scale_factor();
        let first = self.laid.partition_point(|l| l.bottom < top);
        let mut want = Vec::new();
        let selected = style::selection_behind(&p, window.is_window_active());
        if !self.query.is_empty() && self.found_upto != self.laid.len() {
            self.refind();
        }
        let found = style::c(p.hue(Hue::Yellow)).opacity(0.45);
        // (Nothing selected: nothing more a text.)
        let marking = self.all || self.sel.is_some_and(|(a, b)| a != b);
        for (k, laid) in self.laid[first..].iter().enumerate() {
            if laid.top > bottom {
                break;
            }
            for (di, d) in laid.draws.iter().enumerate() {
                match d {
                    Draw::Text { at, lines, leading } => {
                        // Behind it (over what's under it: a code
                        // block's ground), what's found and selected.
                        for (n, m) in self.matches.iter().enumerate().skip_while(|(_, m)| m.0.0 < first + k).take_while(|(_, m)| m.0.0 == first + k).filter(|(_, m)| m.0.1 == di) {
                            if self.current != Some(n) {
                                for r in self.range_rects_in(first + k, Some(di), m.0, m.1) {
                                    window.paint_quad(gpui::fill(r.on(o), found));
                                }
                            }
                        }
                        if marking {
                            for r in self.selection_rects(first + k, di) {
                                window.paint_quad(gpui::fill(r.on(o), selected));
                            }
                        }
                        let mut y = o.y + px(at.1);
                        for line in lines.iter() {
                            let _ = line.paint(point(o.x + px(at.0), y), px(*leading), TextAlign::Left, None, window, cx);
                            y += line.size(px(*leading)).height;
                        }
                    }
                    Draw::Fill(r, radius, color) => {
                        let b = style::snap(window, r.on(o));
                        window.paint_quad(gpui::fill(b, *color).corner_radii(px(*radius)));
                    }
                    Draw::HLine { x0, x1, y, ends, color } => {
                        let line = if *ends { style::hline_above } else { style::hline };
                        line(window, o.x + px(*x0), o.x + px(*x1), o.y + px(*y), *color);
                    }
                    Draw::VLine { x, y0, y1, ends, color } => {
                        let line = if *ends { style::vline_left_of } else { style::vline };
                        line(window, o.x + px(*x), o.y + px(*y0), o.y + px(*y1), *color);
                    }
                    Draw::Bullet(at, level, color) => bullet(window, point(o.x + px(at.0), o.y + px(at.1)), *level, *color),
                    Draw::Check(at, done) => {
                        let well = style::snap_centred(window, point(o.x + px(at.0), o.y + px(at.1)), olo_kit_base::metrics::MARK_SIZE, olo_kit_base::metrics::MARK_SIZE);
                        olo_ui::widgets::check_box_look(window, well, &p, *done);
                    }
                    Draw::Picture(r, block, image) => {
                        let b = r.on(o);
                        let dev = ((r.w * s).round() as u32, (r.h * s).round() as u32);
                        match self.pictures.get(&(*block, dev)) {
                            Some(img) => {
                                let _ = window.paint_image(style::snap(window, b), gpui::Corners::default(), img.clone(), 0, false);
                            }
                            None => want.push((*block, dev, image.clone())),
                        }
                    }
                }
            }
            let r = Bounds::from_corners(point(view.left(), o.y + px(laid.top)), point(view.right(), o.y + px(laid.bottom))).intersect(&view);
            agent::record(window, cx, agent::Node { role: laid.role, name: Some(SharedString::from(laid.text.clone())), bounds: r, enabled: true, ..Default::default() });
            for (n, link) in laid.links.iter().enumerate() {
                agent::record(window, cx, agent::Node { role: "Link", name: Some(link.text.clone().into()), bounds: link.rect.on(o), enabled: true, clickable: true, ..Default::default() });
                let _ = (k, n);
            }
        }
        // The link under the pointer is underlined, all of it.
        if let Some((b, group)) = self.hover
            && let Some(laid) = self.laid.get(b)
        {
            for link in laid.links.iter().filter(|l| l.group == group) {
                let y = px(((f32::from(o.y) + link.baseline) * s).round() / s + 1.0 / s);
                style::hline(window, o.x + px(link.rect.x), o.x + px(link.rect.x + link.rect.w), y, ink.link);
            }
        }
        // What isn't laid out yet, in view: it's coming.
        if self.at.next < self.blocks.len().min(MAX_BLOCKS) && self.at.y < bottom {
            let pending = Bounds::from_corners(point(view.left(), (o.y + px(self.at.y)).max(view.top())), view.bottom_right());
            style::spinner(window, pending.center(), 9.0, style::disabled_text(&p));
        }
        for (block, dev, image) in want {
            self.want_picture(block, dev, image, cx);
        }
    }

    fn want_picture(&mut self, block: usize, dev: (u32, u32), image: Image, cx: &mut Context<Self>) {
        if self.making.contains_key(&(block, dev)) {
            return;
        }
        let generation = self.generation;
        let task = cx.spawn(async move |view, cx| {
            let made = cx.background_spawn(async move { image_view::resampled(&*image, dev) }).await;
            let _ = view.update(cx, |v, cx| {
                v.making.remove(&(block, dev));
                if v.generation == generation
                    && let Some(img) = made
                {
                    v.pictures.insert((block, dev), img);
                    cx.notify();
                }
            });
        });
        self.making.insert((block, dev), task);
    }

    // ---- selecting -----------------------------------------------------------

    pub fn has_selection(&self) -> bool {
        self.all || self.sel.is_some_and(|(a, b)| a != b)
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.all = true;
        self.sel = None;
        cx.notify();
    }

    /// The text selected, as it's copied: a document's whole (each
    /// block's own text, laid out or not) or what's between the ends.
    pub fn selected_text(&self) -> String {
        if self.all {
            let mut out = String::new();
            let mut last_item = false;
            for b in self.blocks.iter() {
                let Some(t) = block_plain(b) else { continue };
                let item = matches!(b, Block::Item(..));
                if !out.is_empty() {
                    out.push_str(if item && last_item { "\n" } else { "\n\n" });
                }
                out.push_str(&t);
                last_item = item;
            }
            return out;
        }
        let Some(sel) = self.sel else { return String::new() };
        let (a, z) = if sel.0 <= sel.1 { sel } else { (sel.1, sel.0) };
        let mut out = String::new();
        let mut prev: Option<(usize, f32, &'static str)> = None;
        for bi in a.0..=z.0.min(self.laid.len().saturating_sub(1)) {
            let laid = &self.laid[bi];
            for (di, d) in laid.draws.iter().enumerate() {
                let Draw::Text { at, lines, .. } = d else { continue };
                if (bi, di) < (a.0, a.1) || (bi, di) > (z.0, z.1) {
                    continue;
                }
                let mut piece = String::new();
                for (li, line) in lines.iter().enumerate() {
                    let here = (bi, di, li);
                    if here < (a.0, a.1, a.2) || here > (z.0, z.1, z.2) {
                        continue;
                    }
                    let from = if here == (a.0, a.1, a.2) { a.3 } else { 0 };
                    let to = if here == (z.0, z.1, z.2) { z.3 } else { line.text.len() };
                    if !piece.is_empty() {
                        piece.push('\n');
                    }
                    piece.push_str(line.text.get(from.min(to)..to.min(line.text.len())).unwrap_or(""));
                }
                // Between: a paragraph, a list's next item, a table's
                // next cell (a tab) or row (a line).
                if let Some((pb, py, role)) = prev {
                    out.push_str(if pb != bi {
                        if role == "ListItem" && laid.role == "ListItem" { "\n" } else { "\n\n" }
                    } else if at.1 > py {
                        "\n"
                    } else {
                        "\t"
                    });
                }
                out.push_str(&piece);
                prev = Some((bi, at.1, laid.role));
            }
        }
        out
    }

    /// The place in the text nearest page point `at`: in the block there
    /// (or the nearest), the line of its texts nearest, the byte under it.
    fn text_at(&self, at: (f32, f32)) -> Option<At> {
        let n = self.laid.len();
        if n == 0 {
            return None;
        }
        let mut best: Option<(f32, At)> = None;
        let first = self.laid.partition_point(|l| l.bottom < at.1).min(n - 1).saturating_sub(1);
        for (bi, laid) in self.laid.iter().enumerate().skip(first).take(3) {
            for (di, d) in laid.draws.iter().enumerate() {
                let Draw::Text { at: o, lines, leading } = d else { continue };
                let mut y = o.1;
                for (li, line) in lines.iter().enumerate() {
                    let size = line.size(px(*leading));
                    let (h, w) = (f32::from(size.height), f32::from(size.width));
                    let dy = if at.1 < y {
                        y - at.1
                    } else if at.1 > y + h {
                        at.1 - (y + h)
                    } else {
                        0.0
                    };
                    let dx = if at.0 < o.0 {
                        o.0 - at.0
                    } else if at.0 > o.0 + w {
                        at.0 - (o.0 + w)
                    } else {
                        0.0
                    };
                    let d = dy * 1000.0 + dx;
                    if best.is_none_or(|(b, _)| d < b) {
                        let p = point(px((at.0 - o.0).max(0.0)), px((at.1 - y).clamp(0.0, (h - 1.0).max(0.0))));
                        let i = match line.closest_index_for_position(p, px(*leading)) {
                            Ok(i) | Err(i) => i,
                        };
                        let i = if at.1 > y + h {
                            line.text.len()
                        } else if at.1 < y {
                            0
                        } else {
                            i
                        };
                        best = Some((d, (bi, di, li, i)));
                    }
                    y += h;
                }
            }
        }
        best.map(|(_, a)| a)
    }

    fn press(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let at = (f32::from(e.position.x - self.origin.x), f32::from(e.position.y - self.origin.y));
        let Some(here) = self.text_at(at) else { return };
        self.all = false;
        let text = self.line_text(here);
        match e.click_count {
            2 => {
                let word = |c: char| c.is_alphanumeric() || c == '_';
                let i = here.3.min(text.len());
                let a = text[..i].char_indices().rev().take_while(|(_, c)| word(*c)).last().map_or(i, |(k, _)| k);
                let b = text[i..].char_indices().find(|(_, c)| !word(*c)).map_or(text.len(), |(k, _)| i + k);
                self.sel = Some(((here.0, here.1, here.2, a), (here.0, here.1, here.2, b)));
            }
            n if n >= 3 => self.sel = Some(((here.0, here.1, here.2, 0), (here.0, here.1, here.2, text.len()))),
            _ => {
                self.sel = match self.sel {
                    Some((anchor, _)) if e.modifiers.shift => Some((anchor, here)),
                    _ => Some((here, here)),
                };
                self.dragging = true;
            }
        }
        cx.notify();
    }

    fn line_text(&self, at: At) -> SharedString {
        match self.laid.get(at.0).and_then(|l| l.draws.get(at.1)) {
            Some(Draw::Text { lines, .. }) => lines.get(at.2).map(|l| l.text.clone()).unwrap_or_default(),
            _ => SharedString::default(),
        }
    }

    /// The selection's boxes over block `bi`'s text `di` (page points).
    fn selection_rects(&self, bi: usize, di: usize) -> Vec<Rect> {
        match (self.all, self.sel) {
            (true, _) => self.range_rects_in(bi, Some(di), (0, 0, 0, 0), (usize::MAX, 0, 0, 0)),
            (false, Some(s)) if s.0 != s.1 => {
                let (a, z) = if s.0 <= s.1 { s } else { (s.1, s.0) };
                self.range_rects_in(bi, Some(di), a, z)
            }
            _ => Vec::new(),
        }
    }

    // ---- find ----------------------------------------------------------------

    /// Look for `query` (lower case) in what's laid out; the first match
    /// shown.
    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.refind();
        self.current = (!self.matches.is_empty()).then_some(0);
        self.show_match();
        cx.notify();
    }

    pub fn close_find(&mut self, cx: &mut Context<Self>) {
        self.query.clear();
        self.matches.clear();
        self.current = None;
        cx.notify();
    }

    /// How many are found, which is shown, and whether more may be (the
    /// document is still being laid out).
    pub fn found(&self) -> (usize, Option<usize>, bool) {
        (self.matches.len(), self.current, self.at.next < self.blocks.len().min(MAX_BLOCKS))
    }

    pub fn step(&mut self, back: bool, cx: &mut Context<Self>) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        self.current = Some(match self.current {
            Some(c) if back => (c + n - 1) % n,
            Some(c) => (c + 1) % n,
            None => 0,
        });
        self.show_match();
        cx.notify();
    }

    fn refind(&mut self) {
        self.found_upto = self.laid.len();
        let keep = self.current.and_then(|c| self.matches.get(c)).copied();
        self.matches.clear();
        if self.query.is_empty() {
            return;
        }
        for (bi, laid) in self.laid.iter().enumerate() {
            for (di, d) in laid.draws.iter().enumerate() {
                let Draw::Text { lines, .. } = d else { continue };
                for (li, line) in lines.iter().enumerate() {
                    let low = line.text.to_lowercase();
                    // (Where lower case keeps every byte where it was.)
                    if low.len() != line.text.len() {
                        continue;
                    }
                    for (i, m) in low.match_indices(&self.query) {
                        self.matches.push(((bi, di, li, i), (bi, di, li, i + m.len())));
                    }
                }
            }
        }
        if let Some(k) = keep {
            self.current = self.matches.iter().position(|m| *m == k);
        }
    }

    /// The match shown: selected, and scrolled to the middle of the view.
    fn show_match(&mut self) {
        let Some(m) = self.current.and_then(|c| self.matches.get(c)).copied() else { return };
        self.all = false;
        self.sel = Some(m);
        if let Some(r) = self.range_rects(m.0.0, m.0, m.1).first() {
            let view = f32::from(self.scroll.bounds().size.height);
            self.scroll.set_offset(point(px(0.0), px(-(r.y + r.h / 2.0 - view / 2.0).max(0.0))));
        }
    }

    /// Boxes over block `bi`'s text from `a` to `z` (page points).
    fn range_rects(&self, bi: usize, a: At, z: At) -> Vec<Rect> {
        self.range_rects_in(bi, None, a, z)
    }

    /// The same, over one of its texts only (`only`: its draw).
    fn range_rects_in(&self, bi: usize, only: Option<usize>, a: At, z: At) -> Vec<Rect> {
        let mut out = Vec::new();
        let Some(laid) = self.laid.get(bi) else { return out };
        if bi < a.0 || bi > z.0 {
            return out;
        }
        for (di, d) in laid.draws.iter().enumerate() {
            let Draw::Text { at, lines, leading } = d else { continue };
            if only.is_some_and(|o| o != di) {
                continue;
            }
            let lh = px(*leading);
            let mut y = at.1;
            for (li, line) in lines.iter().enumerate() {
                let here = (bi, di, li);
                let h = f32::from(line.size(lh).height);
                let w = f32::from(line.size(lh).width);
                if here >= (a.0, a.1, a.2) && here <= (z.0, z.1, z.2) {
                    let from = if here == (a.0, a.1, a.2) { a.3 } else { 0 };
                    let to = if here == (z.0, z.1, z.2) { z.3 } else { line.text.len() };
                    let p0 = line.position_for_index(from.min(line.text.len()), lh).unwrap_or(point(px(0.0), px(0.0)));
                    let p1 = line.position_for_index(to.min(line.text.len()), lh).unwrap_or(point(px(w), px((h - *leading).max(0.0))));
                    let (x0, y0, x1, y1) = (f32::from(p0.x), f32::from(p0.y), f32::from(p1.x), f32::from(p1.y));
                    // Row by row of the wrapped line: the first from where
                    // it starts, the last to where it ends.
                    let mut row = y0;
                    while row <= y1 + 0.5 {
                        let left = if (row - y0).abs() < 0.5 { x0 } else { 0.0 };
                        let right = if (row - y1).abs() < 0.5 { x1 } else { w };
                        if right > left {
                            out.push(Rect { x: at.0 + left, y: y + row, w: right - left, h: *leading });
                        }
                        row += *leading;
                    }
                }
                y += h;
            }
        }
        out
    }

    fn content_height(&self) -> f32 {
        let n = self.blocks.len().min(MAX_BLOCKS);
        let rest = n.saturating_sub(self.at.next) as f32 * self.average.max(LEADING);
        self.at.y + rest + MARGIN
    }
}

impl Render for RichView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let me = cx.entity();
        let (pre, paint) = (me.clone(), me.clone());
        let page = canvas(
            move |b, window, cx| {
                let p = theme::palette(cx);
                pre.update(cx, |v, _| v.prepaint(b, &p, window))
            },
            move |_, hits, window, cx| paint.update(cx, |v, cx| v.paint(hits, window, cx)),
        )
        .size_full();
        let content = div()
            .id("ql-rich-page")
            .w_full()
            .h(px(self.content_height()))
            .child(page)
            .cursor(CursorStyle::IBeam)
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, _, cx| {
                let at = (f32::from(e.position.x - v.origin.x), f32::from(e.position.y - v.origin.y));
                if v.dragging {
                    if e.pressed_button != Some(MouseButton::Left) {
                        v.dragging = false;
                    } else if let (Some(here), Some((anchor, old))) = (v.text_at(at), v.sel)
                        && here != old
                    {
                        v.sel = Some((anchor, here));
                        cx.notify();
                    }
                    return;
                }
                let now = v.link_at(at).map(|(b, n)| (b, v.laid[b].links[n].group));
                if now != v.hover {
                    v.hover = now;
                    cx.notify();
                }
            }))
            // (Its moves come only while the pointer is over the page:
            // gone, from the view or the window, its link is no longer
            // underlined.)
            .on_hover(cx.listener(|v, inside: &bool, _, cx| {
                if !*inside && v.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|v, _: &gpui::MouseUpEvent, _, _| v.dragging = false))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, e: &MouseDownEvent, _, cx| {
                    let at = (f32::from(e.position.x - v.origin.x), f32::from(e.position.y - v.origin.y));
                    if let Some((b, n)) = v.link_at(at) {
                        let url = v.laid[b].links[n].url.clone();
                        if let Some(y) = v.follow(&url) {
                            v.scroll.set_offset(point(px(0.0), px(-(y - GAP).max(0.0))));
                            cx.notify();
                        }
                        return;
                    }
                    // Not on a link: the text, selected.
                    v.press(e, cx);
                }),
            );
        div().size_full().child(ScrollView::new("ql-rich", &self.scroll).border(false).background(p.document_background).child(content))
    }
}

/// Lay `job` out `width` wide at `at`; its height.
fn place(window: &mut Window, laid: &mut Laid, job: &Job, at: (f32, f32), width: f32, ink: &Inks) -> f32 {
    let lines = shape(window, job, width);
    place_lines(laid, lines, job, at, ink)
}

/// `lines` (shaped from `job`) at `at`, their code on a tint and their
/// links taking clicks; their height.
fn place_lines(laid: &mut Laid, lines: SmallVec<[WrappedLine; 1]>, job: &Job, at: (f32, f32), ink: &Inks) -> f32 {
    let h = height(&lines, job.leading);
    let groups = laid.links.last().map_or(0, |l| l.group + 1);
    for (rect, baseline, m) in mark_rects(&lines, &job.marks, job) {
        let rect = Rect { x: rect.x + at.0, y: rect.y + at.1, ..rect };
        match &job.marks[m].1 {
            Mark::Code => laid.draws.push(Draw::Fill(Rect { x: rect.x - 2.0, y: rect.y - 0.5, w: rect.w + 4.0, h: rect.h + 1.0 }, 3.0, ink.code_fill)),
            Mark::Link(url) => {
                let text = job.text.get(job.marks[m].0.clone()).unwrap_or("").to_owned();
                laid.links.push(Link { rect, baseline: baseline + at.1, url: url.clone(), text, group: groups + m });
            }
        }
    }
    laid.draws.push(Draw::Text { at, lines: Box::new(lines), leading: job.leading });
    h
}

/// The room above a block of `kind` after one of `prev`.
fn space(prev: Option<Kind>, kind: Kind) -> f32 {
    match (prev, kind) {
        (None, _) | (_, Kind::QuoteEnd) | (Some(Kind::QuoteStart(false)), _) => 0.0,
        (Some(Kind::QuoteStart(true)), _) => 4.0,
        (_, Kind::Heading(l)) => HEADING_ABOVE[l as usize - 1],
        (Some(Kind::Heading(l)), _) => {
            if l <= 2 {
                14.0
            } else {
                8.0
            }
        }
        (Some(Kind::Item | Kind::ItemMore), Kind::Item) => ITEM_GAP,
        (Some(Kind::Item | Kind::ItemMore), Kind::ItemMore | Kind::Code(true)) => 8.0,
        _ => GAP,
    }
}

/// A table `w` wide at most at `at`; its height. Columns are as wide as
/// their widest cell, or share the width by it when that's too much.
#[allow(clippy::too_many_arguments)]
fn table(window: &mut Window, laid: &mut Laid, align: &[Align], rows: &[Row], at: (f32, f32), w: f32, ink: &Inks, s: f32) -> f32 {
    let snap = |v: f32| (v * s).round() / s;
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return 0.0;
    }
    let natural: Vec<f32> = (0..cols)
        .map(|k| {
            rows.iter()
                .enumerate()
                .filter_map(|(n, r)| r.get(k).map(|cell| (n, cell)))
                .map(|(n, cell)| {
                    let job = text_job(cell, BODY, LEADING, n == 0, ink.text, ink);
                    window.text_system().shape_text(job.text.clone().into(), px(BODY), &job.runs, None, None).ok().and_then(|l| l.first().map(|l| f32::from(l.width()))).unwrap_or(0.0)
                })
                .fold(0.0, f32::max)
                .ceil()
                + 2.0 * CELL_PAD.0
        })
        .collect();
    let total: f32 = natural.iter().sum();
    let widths: Vec<f32> = if total <= w { natural } else { natural.iter().map(|n| (n * w / total).max(48.0).floor()).collect() };
    let mut xs = vec![at.0];
    for cw in &widths {
        xs.push(snap(xs.last().expect("one") + cw));
    }
    let right = *xs.last().expect("one");
    let mut y = at.1;
    let mut text = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        let mut cells = Vec::new();
        let mut h: f32 = LEADING;
        for k in 0..cols {
            let spans = row.get(k).map_or(&[][..], Vec::as_slice);
            let job = text_job(spans, BODY, LEADING, n == 0, ink.text, ink);
            let lines = shape(window, &job, xs[k + 1] - xs[k] - 2.0 * CELL_PAD.0);
            h = h.max(height(&lines, LEADING));
            cells.push((lines, job));
        }
        let row_h = snap(h + 2.0 * CELL_PAD.1);
        let fill = if n == 0 {
            Some(ink.head_fill)
        } else if n % 2 == 0 {
            Some(ink.stripe)
        } else {
            None
        };
        if let Some(fill) = fill {
            laid.draws.push(Draw::Fill(Rect { x: at.0, y, w: right - at.0, h: row_h }, 0.0, fill));
        }
        for (k, (lines, job)) in cells.into_iter().enumerate() {
            let room = xs[k + 1] - xs[k] - 2.0 * CELL_PAD.0;
            let lw = lines.iter().map(|l| f32::from(l.width())).fold(0.0, f32::max);
            let x = match align.get(k).copied().unwrap_or_default() {
                Align::Left => xs[k] + CELL_PAD.0,
                Align::Center => xs[k] + CELL_PAD.0 + ((room - lw) / 2.0).max(0.0),
                Align::Right => xs[k + 1] - CELL_PAD.0 - lw.min(room),
            };
            place_lines(laid, lines, &job, (snap(x), y + CELL_PAD.1), ink);
        }
        text.push(row.iter().map(|c| plain(c)).collect::<Vec<_>>().join("\t"));
        laid.draws.push(Draw::HLine { x0: at.0, x1: right, y, ends: false, color: ink.line });
        y += row_h;
    }
    laid.draws.push(Draw::HLine { x0: at.0, x1: right, y, ends: true, color: ink.line });
    for (k, &x) in xs.iter().enumerate() {
        laid.draws.push(Draw::VLine { x, y0: at.1, y1: y, ends: k == cols, color: ink.line });
    }
    laid.text = text.join("\n");
    y - at.1
}

/// A list's bullet at `at`: a disc, then a ring, then a square, by level.
fn bullet(window: &mut Window, at: Point<Pixels>, level: usize, color: Hsla) {
    let s = window.scale_factor();
    let grid = |v: Pixels| px((f32::from(v) * s).round() / s);
    let at = point(grid(at.x), grid(at.y));
    match level % 3 {
        0 => window.paint_sector(at, px(0.0), px(2.5), 0.0, std::f32::consts::TAU, color),
        1 => window.paint_sector(at, px(1.75), px(2.75), 0.0, std::f32::consts::TAU, color),
        _ => window.paint_quad(gpui::fill(style::snap_centred(window, at, 4.5, 4.5), color)),
    }
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

/// A block's own text, as the whole document's is copied (pictures,
/// rules and quotes' edges say nothing).
fn block_plain(b: &Block) -> Option<String> {
    match b {
        Block::Heading(_, sp) | Block::Para(sp) => Some(plain(sp)),
        Block::Item(_, mark, sp) if mark.is_empty() => Some(plain(sp)),
        Block::Item(_, mark, sp) => Some(format!("{mark} {}", plain(sp))),
        Block::Code { lines, .. } => Some(lines.join("\n")),
        Block::Table { rows, .. } => Some(rows.iter().map(|r| r.iter().map(|c| plain(c)).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n")),
        _ => None,
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
