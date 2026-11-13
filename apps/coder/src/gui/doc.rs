//! The conversation, shown: the shared rich text (`crate::rich`, made on the
//! presentation worker) in the kit's editor, read-only, so a week-long
//! session lays out only the paragraphs that show and can be selected and
//! copied like any document. Each paragraph is set by what it is (a heading,
//! a list, code) and by its part of the conversation: what the person said
//! on a tinted ground with a bar, actions as quiet one-line rows, their
//! output in a code well under them, notes with a coloured bar.
use crate::rich::{self, Block, Class, Kind, Tone, ToolState};
use olo_kit::{
    Cx, Element, FontStyle, FontWeight, Hsla, View,
    editor::{self, Editor, EditorHost, Paragraph, Row, Span},
    metrics as m, style, theme,
};
use olo_theme::Palette;
use std::{ops::Range, sync::Arc};

/// What the conversation tells the window.
pub enum Event {
    /// A link in it was clicked (an action opened or closed, a source).
    Link(String),
}

pub struct Transcript {
    editor: Editor,
    text: Arc<rich::Text>,
    /// Kept at the end as text arrives (else left where the person scrolled).
    pub follow: bool,
}

pub enum Msg {}

impl Transcript {
    pub fn new(follow: bool) -> Transcript {
        Transcript { editor: Editor::new(), text: Arc::default(), follow }
    }

    /// The conversation as it is now.
    pub fn set(&mut self, text: Arc<rich::Text>, cx: &mut Cx<Self>) {
        if Arc::ptr_eq(&self.text, &text) {
            return;
        }
        // Only what differs is laid out again: from the first paragraph
        // whose text, kind or styles changed (an action opened, the live
        // reply growing at the end).
        self.editor.changed(&self.text.text, &text.text);
        let first_para = self.text.paragraphs.iter().zip(&text.paragraphs).find(|(a, b)| a.start != b.start || a.kind != b.kind || a.block != b.block).map(|(a, b)| a.start.min(b.start));
        let first_run = self.text.runs.iter().zip(&text.runs).find(|(a, b)| a != b).map(|(a, b)| a.range.start.min(b.range.start));
        let first = first_para.into_iter().chain(first_run).min().unwrap_or_else(|| self.text.text.len().min(text.text.len()));
        if first < text.text.len() {
            self.editor.restyled(first..text.text.len());
        }
        self.text = text;
        if self.follow {
            let end = self.text.text.len();
            self.editor.select(&self.text.text, end..end);
        }
        cx.invalidate();
    }

    pub fn copy(&mut self, cx: &mut Cx<Self>) {
        editor::edit(self, 0, editor::Edit::Copy, cx);
    }

    pub fn select_all(&mut self, cx: &mut Cx<Self>) {
        editor::edit(self, 0, editor::Edit::SelectAll, cx);
    }

    pub fn to_end(&mut self, cx: &mut Cx<Self>) {
        self.follow = true;
        let end = self.text.text.len();
        self.editor.select(&self.text.text, end..end);
        cx.invalidate();
    }
}

impl View for Transcript {
    type Message = Msg;

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let options = editor::Options { editable: false, margin: (m::WINDOW_INSET, m::SPACING), background: Some(p.document_background), ..Default::default() };
        editor::view(self, 0, options, cx)
    }
}

impl EditorHost for Transcript {
    fn editor(&mut self, _: usize, cx: &mut Cx<Self>, f: &mut dyn FnMut(&mut Editor, &mut dyn editor::Document, &mut Cx<Self>)) {
        let palette = theme::palette();
        let mut doc = Doc { content: &self.text, p: &palette };
        f(&mut self.editor, &mut doc, cx);
    }

    fn editor_shown(&self, _: usize) -> Option<(&Editor, &str)> {
        Some((&self.editor, &self.text.text))
    }

    fn open_link(&mut self, _: usize, link: String, cx: &mut Cx<Self>) {
        cx.emit(Event::Link(link));
    }
}

/// The colours the conversation's parts are set in.
pub struct Look {
    pub user_ground: Hsla,
    pub user_bar: Hsla,
    pub code_ground: Hsla,
    pub quiet: Hsla,
}

impl Look {
    pub fn of(p: &Palette) -> Look {
        Look {
            user_ground: style::c(olo_theme::mix(p.document_background, p.keyboard_navigation, 0.07)),
            user_bar: style::c(p.keyboard_navigation),
            code_ground: style::c(olo_theme::mix(p.document_background, p.document_text, 0.045)),
            quiet: style::c(olo_theme::mix(p.document_text, p.document_background, 0.42)),
        }
    }
}

pub fn tone_color(p: &Palette, tone: Tone) -> Hsla {
    style::c(match tone {
        Tone::Info => p.keyboard_navigation,
        Tone::Success => p.success,
        Tone::Warning => p.warning,
        Tone::Failure => p.failure,
    })
}

fn state_color(p: &Palette, state: ToolState) -> Option<Hsla> {
    match state {
        ToolState::Failed => Some(style::c(p.failure)),
        ToolState::Waiting => Some(style::c(olo_theme::readable(p.warning, p.document_background, 4.5))),
        ToolState::Denied | ToolState::Stopped => None,
        ToolState::Running | ToolState::Done => None,
    }
}

/// The editor's view of the conversation (made as it's asked for: the
/// palette may have changed since).
struct Doc<'a> {
    content: &'a rich::Text,
    p: &'a Palette,
}

/// Body text, a point larger than controls': the conversation is read.
const BODY: f32 = style::PLAIN_SIZE + 1.0;

impl editor::Document for Doc<'_> {
    fn text(&self) -> &str {
        &self.content.text
    }

    fn spans(&mut self, range: Range<usize>, out: &mut Vec<Span>) {
        let look = Look::of(self.p);
        let para = self.content.paragraph(range.start);
        let kind = para.map(|p| &p.kind).unwrap_or(&Kind::Body);
        let block = para.map(|p| p.block).unwrap_or_default();
        let heading = match kind {
            Kind::Heading(n) => Some(*n),
            _ => None,
        };
        let in_code = matches!(kind, Kind::Code(_));
        for r in self.content.runs(range.clone()) {
            let len = r.range.end.min(range.end) - r.range.start.max(range.start);
            if len == 0 {
                continue;
            }
            let bold = r.style.bold;
            let weight = if bold { FontWeight::BOLD } else { FontWeight::NORMAL };
            let mono = r.style.code || in_code;
            let mut font = if mono { style::mono_font(weight) } else { style::ui_font(weight) };
            if r.style.italic {
                font.style = FontStyle::Italic;
            }
            let size = match (mono, block, heading) {
                (true, Block::Tool(_), _) => style::MONO_SIZE - 0.5,
                (true, Block::Output, _) => style::MONO_SIZE - 0.5,
                (true, _, _) => style::MONO_SIZE,
                (false, Block::Tool(_) | Block::Quiet, _) => style::PLAIN_SIZE,
                (false, _, Some(1)) => BODY + 7.0,
                (false, _, Some(2)) => BODY + 4.0,
                (false, _, Some(3)) => BODY + 2.0,
                (false, _, _) => BODY,
            };
            let mut span = Span::new(len, font, size);
            span.strike = r.style.strike;
            // Links: a toggle in an action's row is the row itself (no
            // underline); others read as links.
            let toggle = r.style.link.as_deref().is_some_and(|l| l.starts_with("olo-coder:toggle/"));
            span.underline = r.style.link.is_some() && !toggle;
            span.color = match block {
                Block::Tool(state) => match r.style.class {
                    _ if mono => state_color(self.p, state).or(Some(style::c(self.p.document_text))),
                    _ => state_color(self.p, state).or(Some(look.quiet)),
                },
                Block::Quiet => Some(look.quiet),
                _ => r.style.class.or_else(|| (r.style.link.is_some() && !toggle).then_some(Class::Link)).map(|c| style::c(self.p.syntax(c))),
            };
            if r.style.code && !in_code && !matches!(block, Block::Tool(_)) {
                span.background = Some(look.code_ground);
            }
            out.push(span);
        }
        if out.is_empty() {
            out.push(Span::new(0, style::ui_font(FontWeight::NORMAL), BODY));
        }
    }

    fn paragraph(&mut self, range: Range<usize>) -> Paragraph {
        let look = Look::of(self.p);
        let Some(para) = self.content.paragraph(range.start) else { return Paragraph::default() };
        let mut p = Paragraph::default();
        match &para.kind {
            Kind::Heading(n) => {
                p.above = if *n <= 2 { m::SPACING * 1.5 } else { m::SPACING };
                p.below = m::TIGHT;
            }
            Kind::Body => p.below = m::TIGHT,
            Kind::Quote => {
                p.left = m::WINDOW_INSET;
                p.bar = Some(style::c(self.p.control_border));
                p.below = m::TIGHT;
            }
            Kind::List => {
                p.left = m::SPACING;
                p.below = 2.0;
            }
            Kind::Code(lang) => {
                p.left = m::SPACING;
                p.right = m::SPACING;
                p.verbatim = true;
                p.shade = Some(look.code_ground);
                if lang == "diff" {
                    let line = &self.content.text[range.clone()];
                    if line.starts_with('+') && !line.starts_with("+++") {
                        p.shade = Some(style::c(olo_theme::mix(self.p.document_background, self.p.success, 0.14)));
                    } else if line.starts_with('-') && !line.starts_with("---") {
                        p.shade = Some(style::c(olo_theme::mix(self.p.document_background, self.p.failure, 0.12)));
                    }
                }
            }
            Kind::Table { columns, header } => {
                p.row = Some(Row { columns: vec![1.0; *columns], header: *header, ..Default::default() });
                p.verbatim = true;
            }
            Kind::Rule => {
                p.rule = Some(style::c(self.p.control_border));
                p.above = m::SPACING;
                p.below = m::SPACING;
            }
        }
        match para.block {
            Block::User => {
                // What the person said: one tinted box with a bar, the
                // paragraphs of a message making one ground.
                p.left += m::SPACING;
                p.right += m::SPACING;
                p.shade = Some(look.user_ground);
                p.bar = Some(look.user_bar);
                p.below = m::TIGHT;
            }
            Block::Note(tone) => {
                p.left += m::SPACING;
                p.bar = Some(tone_color(self.p, tone));
            }
            Block::Tool(_) => {
                // An action's row: tight under the one before, so a run of
                // them reads as a list.
                p.above = 0.0;
                p.below = 2.0;
                p.verbatim = true;
            }
            Block::Output => {
                p.left += m::WINDOW_INSET;
            }
            Block::Body | Block::Quiet => {}
        }
        if para.first {
            p.above += match para.block {
                Block::Tool(_) => 2.0,
                _ => m::SPACING,
            };
        }
        p
    }

    fn link_at(&mut self, at: usize) -> Option<String> {
        self.content.runs(at..at + 1).find_map(|r| r.style.link.clone())
    }

    fn replace(&mut self, _: Range<usize>, _: &str, _: Range<usize>) {}
}
