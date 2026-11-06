//! Source code in the kit's [`editor`](crate::editor): coloured by syntax
//! as it's typed ([`olo_syntax`]: tree-sitter's grammars, in process), in
//! the monospaced face, with line numbers (`editor::Options::
//! line_numbers`). Any app with code to show or edit uses it: StyledEdit's
//! Source mode, a Markdown document's code blocks.
//!
//! The app keeps a [`Code`] beside its text: its document's `spans` come
//! from [`Code::spans`], every change to the text goes to [`Code::edit`]
//! (or [`Code::reset`] when it's all new), and [`parse`] after each runs
//! the parse on a worker and draws the view again when the colours come.
//! Until the first parse is back (a second for 100,000 lines) the text
//! shows in plain ink; after an edit, the colours stay on their
//! characters until the new parse comes. A line's spans are made once,
//! when it first shows, and kept until the text or its tree changes.

use std::{
    collections::HashMap,
    ops::Range,
    time::{Duration, Instant},
};

use gpui::{Context, Font, FontWeight, Hsla};
pub use olo_syntax::{self as syntax, Changed, Class, Edit, Language, by_name, detect};

use crate::{editor::Span, style};

/// A text's syntax and the spans it colours it with.
pub struct Code {
    hl: syntax::Highlighter,
    size: f32,
    font: Font,
    /// Each class's colour, from the palette they were made with (its
    /// text and ground).
    colors: Option<([u8; 8], [Hsla; 12])>,
    /// The spans made for each line shown, by its start and length.
    lines: HashMap<(usize, usize), Vec<Span>>,
    found: Vec<(Range<usize>, Class)>,
    /// When it was last edited, and whether a parse waits for the typing
    /// to pause (a big text's: [`parse`]).
    edited: Option<Instant>,
    waiting: bool,
}

/// A text this big is parsed once typing pauses for `PAUSE` (each parse
/// is a copy of it and 16 ms on a worker at 100,000 lines); its colours
/// follow the edits meanwhile. Smaller, at each edit.
pub const BIG: usize = 256 * 1024;
pub const PAUSE: Duration = Duration::from_millis(40);

impl Code {
    /// For a text in `lang`, at `size` points.
    pub fn new(lang: &'static Language, size: f32) -> Code {
        Code { hl: syntax::Highlighter::new(lang), size, font: style::mono_font(FontWeight::NORMAL), colors: None, lines: HashMap::new(), found: Vec::new(), edited: None, waiting: false }
    }

    pub fn language(&self) -> &'static Language {
        self.hl.language()
    }

    pub fn size(&self) -> f32 {
        self.size
    }

    /// Set at another size.
    pub fn set_size(&mut self, size: f32) {
        if size != self.size {
            self.size = size;
            self.lines.clear();
        }
    }

    /// The text changed by `edit` (each change, in order). The lines it
    /// touched are made again; those after it keep their spans, moved.
    pub fn edit(&mut self, edit: &Edit) {
        self.hl.edit(edit);
        self.edited = Some(Instant::now());
        let (r, n) = (edit.removed(), edit.inserted().len());
        let delta = n as isize - r.len() as isize;
        let was = std::mem::take(&mut self.lines);
        self.lines = was
            .into_iter()
            .filter_map(|((start, len), spans)| match () {
                _ if start + len < r.start => Some(((start, len), spans)),
                _ if start > r.end => Some((((start as isize + delta) as usize, len), spans)),
                _ => None,
            })
            .collect();
    }

    /// The text was replaced, all of it.
    pub fn reset(&mut self) {
        self.hl.reset();
        self.lines.clear();
    }

    /// Whether it has been parsed (else its text shows plain).
    pub fn is_parsed(&self) -> bool {
        self.hl.has_tree()
    }

    /// The spans of bytes `range` of `text` (a paragraph, a line of code),
    /// in `palette`'s colours.
    pub fn spans(&mut self, text: &str, range: Range<usize>, palette: &olo_theme::Palette, out: &mut Vec<Span>) {
        let key = palette_key(palette);
        if self.colors.as_ref().map(|c| c.0) != Some(key) {
            let mut colors = [Hsla::default(); 12];
            for (k, c) in CLASSES.iter().enumerate() {
                colors[k] = style::c(palette.syntax(*c));
            }
            self.colors = Some((key, colors));
            self.lines.clear();
        }
        if let Some(spans) = self.lines.get(&(range.start, range.len())) {
            out.extend(spans.iter().cloned());
            return;
        }
        let colors = self.colors.as_ref().map(|c| c.1).unwrap_or_default();
        self.hl.classes(text, range.clone(), &mut self.found);
        let mut spans = Vec::with_capacity(self.found.len() * 2 + 1);
        let plain = |len: usize| Span::new(len, self.font.clone(), self.size);
        let mut at = 0;
        for (r, class) in &self.found {
            if r.start > at {
                spans.push(plain(r.start - at));
            }
            let k = CLASSES.iter().position(|c| c == class).unwrap_or(0);
            spans.push(Span { color: Some(colors[k]), ..plain(r.len()) });
            at = r.end;
        }
        if at < range.len() || spans.is_empty() {
            spans.push(plain(range.len() - at));
        }
        // (Kept while they show: a big file scrolled end to end mustn't
        // keep every line it showed.)
        if self.lines.len() > 4096 {
            self.lines.clear();
        }
        out.extend(spans.iter().cloned());
        self.lines.insert((range.start, range.len()), spans);
    }

    /// The parse it wants run (the text changed since the last began, and
    /// none is out), for [`parse`].
    pub fn job(&mut self, text: &str) -> Option<syntax::Job> {
        self.hl.job(text)
    }

    /// A parse came back: its colours from now (the lines it changed made
    /// again).
    pub fn done(&mut self, parsed: syntax::Parsed) -> Changed {
        let changed = self.hl.done(parsed);
        match &changed {
            Changed::Nothing => {}
            Changed::All => self.lines.clear(),
            Changed::Ranges(rs) => self.lines.retain(|(start, len), _| !rs.iter().any(|r| r.start <= start + len && r.end >= *start)),
        }
        changed
    }
}

const CLASSES: [Class; 12] =
    [Class::Plain, Class::Comment, Class::String, Class::Keyword, Class::Number, Class::Type, Class::Function, Class::Constant, Class::Tag, Class::Attribute, Class::Heading, Class::Link];

fn palette_key(p: &olo_theme::Palette) -> [u8; 8] {
    let (a, b) = (p.document_text, p.document_background);
    [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]]
}

/// Runs the parse `code` wants (if any) on a worker; when it's back,
/// `with` finds the code and its text in the view again (None: gone), the
/// colours are taken and the view drawn again, and if the text changed
/// meanwhile it is parsed again.
pub fn parse<V: 'static>(cx: &mut Context<V>, code: &mut Code, text: &str, with: fn(&mut V) -> Option<(&mut Code, &str)>) {
    // A big text, being typed in: once the typing pauses.
    if text.len() > BIG
        && let Some(t) = code.edited
        && t.elapsed() < PAUSE
    {
        if !code.waiting {
            code.waiting = true;
            let wait = PAUSE - t.elapsed();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |v, cx| {
                    let Some((code, text)) = with(v) else { return };
                    code.waiting = false;
                    parse(cx, code, text, with);
                });
            })
            .detach();
        }
        return;
    }
    let Some(job) = code.job(text) else { return };
    let task = cx.background_executor().spawn(async move { job.run() });
    cx.spawn(async move |this, cx| {
        let parsed = task.await;
        let _ = this.update(cx, |v, cx| {
            let Some((code, text)) = with(v) else { return };
            if code.done(parsed) != Changed::Nothing {
                cx.notify();
            }
            parse(cx, code, text, with);
        });
    })
    .detach();
}
