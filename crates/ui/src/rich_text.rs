//! Read-only rich text prepared on a worker, shared by native and terminal views.
//! No colours, fonts or scale-dependent measurements are stored here. Fenced
//! code uses the kit's syntax grammars; unchanged blocks retain their captures.
pub use olo_syntax::Class;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::{collections::VecDeque, ops::Range, sync::Arc};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub class: Option<Class>,
    pub link: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub range: Range<usize>,
    pub style: Style,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Kind {
    #[default]
    Body,
    Heading(u8),
    Quote,
    List,
    Code(String),
    Table {
        columns: usize,
        header: bool,
    },
    Rule,
}
#[derive(Clone, Debug)]
pub struct Paragraph {
    pub start: usize,
    pub kind: Kind,
}
#[derive(Clone, Debug, Default)]
pub struct Text {
    pub text: String,
    pub runs: Vec<Run>,
    pub paragraphs: Vec<Paragraph>,
}
impl Text {
    /// Captures touching a visible byte range, with logarithmic lookup.
    pub fn runs(&self, range: Range<usize>) -> impl Iterator<Item = &Run> {
        let at = self.runs.partition_point(|r| r.range.end <= range.start);
        self.runs[at..].iter().take_while(move |r| r.range.start < range.end)
    }
    pub fn kind(&self, at: usize) -> &Kind {
        self.paragraphs.get(self.paragraphs.partition_point(|p| p.start <= at).saturating_sub(1)).map(|p| &p.kind).unwrap_or(&Kind::Body)
    }
}
type Captures = Arc<Vec<(Range<usize>, Class)>>;
#[derive(Default)]
pub struct Renderer {
    blocks: VecDeque<(String, String, Captures)>,
}
impl Renderer {
    fn captures(&mut self, language: &str, code: &str) -> Captures {
        if let Some((_, _, v)) = self.blocks.iter().find(|(l, t, _)| l == language && t == code) {
            return v.clone();
        }
        let mut captures = Vec::new();
        if let Some(lang) = olo_syntax::by_name(language) {
            let mut hl = olo_syntax::Highlighter::new(lang);
            if let Some(job) = hl.job(code) {
                hl.done(job.run());
            }
            hl.classes(code, 0..code.len(), &mut captures);
        }
        let captures = Arc::new(captures);
        if code.len() <= 64 * 1024 {
            self.blocks.push_back((language.into(), code.into(), captures.clone()));
            while self.blocks.len() > 32 {
                self.blocks.pop_front();
            }
        }
        captures
    }
    /// Must run on a worker: a language's first grammar query can be expensive.
    pub fn render(&mut self, markdown: &str) -> Text {
        let mut b = Builder::default();
        let mut styles = vec![Style::default()];
        let mut kinds = vec![Kind::Body];
        let mut list_numbers: Vec<Option<u64>> = Vec::new();
        let mut code: Option<(String, usize, usize)> = None;
        let mut columns = 0;
        let mut cell = 0;
        for event in Parser::new_ext(markdown, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS) {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. } => {
                        let mut s = styles.last().cloned().unwrap_or_default();
                        match tag {
                            Tag::Strong => s.bold = true,
                            Tag::Emphasis => s.italic = true,
                            Tag::Strikethrough => s.strike = true,
                            Tag::Link { dest_url, .. } => s.link = Some(dest_url.into_string()),
                            _ => {}
                        }
                        styles.push(s);
                    }
                    Tag::Heading { level, .. } => {
                        b.line();
                        kinds.push(Kind::Heading(level as u8));
                    }
                    Tag::BlockQuote(_) => {
                        b.line();
                        kinds.push(Kind::Quote);
                    }
                    Tag::Paragraph => {
                        if !matches!(kinds.last(), Some(Kind::List)) {
                            b.line();
                        }
                    }
                    Tag::List(start) => {
                        b.line();
                        list_numbers.push(start);
                    }
                    Tag::Item => {
                        b.line();
                        kinds.push(Kind::List);
                        let marker = match list_numbers.last_mut() {
                            Some(Some(n)) => {
                                let s = format!("{n}. ");
                                *n += 1;
                                s
                            }
                            _ => "• ".into(),
                        };
                        b.append(&format!("{}{marker}", "  ".repeat(list_numbers.len().saturating_sub(1))), styles.last().unwrap(), kinds.last().unwrap());
                    }
                    Tag::CodeBlock(kind) => {
                        b.line();
                        let lang = match kind {
                            CodeBlockKind::Fenced(s) => s.split_whitespace().next().unwrap_or("").to_owned(),
                            _ => String::new(),
                        };
                        kinds.push(Kind::Code(lang.clone()));
                        code = Some((lang, b.out.text.len(), b.out.runs.len()));
                    }
                    Tag::Table(aligns) => {
                        b.line();
                        columns = aligns.len();
                    }
                    Tag::TableHead | Tag::TableRow => {
                        b.line();
                        cell = 0;
                        kinds.push(Kind::Table { columns, header: matches!(tag, Tag::TableHead) });
                    }
                    Tag::TableCell => {
                        if cell > 0 {
                            b.append("\t", styles.last().unwrap(), kinds.last().unwrap());
                        }
                        cell += 1;
                    }
                    _ => {}
                },
                Event::End(tag) => match tag {
                    TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => {
                        if styles.len() > 1 {
                            styles.pop();
                        }
                    }
                    TagEnd::CodeBlock => {
                        if let Some((lang, start, run)) = code.take() {
                            let end = b.out.text.len();
                            let captures = self.captures(&lang, &b.out.text[start..end]);
                            b.out.runs.truncate(run);
                            let mut at = start;
                            for (range, class) in captures.iter() {
                                let r = start + range.start..start + range.end;
                                if r.start > at {
                                    b.out.runs.push(Run { range: at..r.start, style: Style { code: true, ..Default::default() } });
                                }
                                b.out.runs.push(Run { range: r.clone(), style: Style { code: true, class: Some(*class), ..Default::default() } });
                                at = r.end;
                            }
                            if at < end {
                                b.out.runs.push(Run { range: at..end, style: Style { code: true, ..Default::default() } });
                            }
                        }
                        b.line();
                        kinds.pop();
                    }
                    TagEnd::Heading(_) | TagEnd::BlockQuote(_) | TagEnd::Item | TagEnd::TableHead | TagEnd::TableRow => {
                        b.line();
                        if kinds.len() > 1 {
                            kinds.pop();
                        }
                    }
                    TagEnd::List(_) => {
                        list_numbers.pop();
                        b.line();
                    }
                    TagEnd::Paragraph | TagEnd::Table => b.line(),
                    _ => {}
                },
                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                    let mut s = styles.last().cloned().unwrap_or_default();
                    s.code |= code.is_some();
                    if matches!(kinds.last(), Some(Kind::Heading(_) | Kind::Table { header: true, .. })) {
                        s.bold = true;
                    }
                    b.append(&text, &s, kinds.last().unwrap());
                }
                Event::Code(text) => {
                    let mut s = styles.last().cloned().unwrap_or_default();
                    s.code = true;
                    b.append(&text, &s, kinds.last().unwrap());
                }
                Event::SoftBreak => b.append(" ", styles.last().unwrap(), kinds.last().unwrap()),
                Event::HardBreak => b.line(),
                Event::Rule => {
                    b.line();
                    b.append("—", styles.last().unwrap(), &Kind::Rule);
                    b.line();
                }
                Event::TaskListMarker(done) => b.append(if done { "☑ " } else { "☐ " }, styles.last().unwrap(), kinds.last().unwrap()),
                _ => {}
            }
        }
        b.out
    }
}
#[derive(Default)]
struct Builder {
    out: Text,
}
impl Builder {
    fn line(&mut self) {
        if !self.out.text.is_empty() && !self.out.text.ends_with('\n') {
            self.append("\n", &Style::default(), &Kind::Body);
        }
    }
    fn append(&mut self, text: &str, style: &Style, kind: &Kind) {
        let safe: std::borrow::Cow<'_, str> = if text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
            text.chars().map(|c| if c.is_control() && c != '\n' && c != '\t' { '�' } else { c }).collect::<String>().into()
        } else {
            text.into()
        };
        for piece in safe.split_inclusive('\n') {
            if self.out.text.is_empty() || self.out.text.ends_with('\n') {
                self.out.paragraphs.push(Paragraph { start: self.out.text.len(), kind: kind.clone() });
            }
            let start = self.out.text.len();
            self.out.text.push_str(piece);
            let end = self.out.text.len();
            if start == end {
                continue;
            }
            if let Some(last) = self.out.runs.last_mut().filter(|r| r.range.end == start && r.style == *style) {
                last.range.end = end;
            } else {
                self.out.runs.push(Run { range: start..end, style: style.clone() });
            }
        }
    }
}

/// Native document adapter. Construct with the current palette when the editor
/// asks for text; the prepared document itself is reusable across themes/scales.
pub struct Document<'a> {
    pub content: &'a Text,
    pub palette: &'a olo_theme::Palette,
}
impl crate::editor::Document for Document<'_> {
    fn text(&self) -> &str {
        &self.content.text
    }
    fn spans(&mut self, range: Range<usize>, out: &mut Vec<crate::editor::Span>) {
        use crate::{editor::Span, style};
        use gpui::{FontStyle, FontWeight};
        let heading = match self.content.kind(range.start) {
            Kind::Heading(n) => Some(*n),
            _ => None,
        };
        for r in self.content.runs(range.clone()) {
            let weight = if r.style.bold { FontWeight::BOLD } else { FontWeight::NORMAL };
            let mut font = if r.style.code { style::mono_font(weight) } else { style::ui_font(weight) };
            if r.style.italic {
                font.style = FontStyle::Italic;
            }
            let size = if r.style.code {
                style::MONO_SIZE
            } else {
                style::PLAIN_SIZE
                    + match heading {
                        Some(1) => 8.0,
                        Some(2) => 5.0,
                        Some(3) => 2.0,
                        _ => 0.0,
                    }
            };
            let mut span = Span::new(r.range.end.min(range.end) - r.range.start.max(range.start), font, size);
            span.strike = r.style.strike;
            span.underline = r.style.link.is_some();
            span.color = r.style.class.or_else(|| r.style.link.as_ref().map(|_| Class::Link)).map(|c| style::c(self.palette.syntax(c)));
            if r.style.code && !matches!(self.content.kind(range.start), Kind::Code(_)) {
                span.background = Some(style::c(olo_theme::mix(self.palette.document_background, self.palette.document_text, 0.06)));
            }
            out.push(span);
        }
        if out.is_empty() {
            out.push(Span::new(0, style::ui_font(FontWeight::NORMAL), style::PLAIN_SIZE));
        }
    }
    fn paragraph(&mut self, range: Range<usize>) -> crate::editor::Paragraph {
        use crate::{
            editor::{Paragraph, Row},
            metrics as m, style,
        };
        let mut p = Paragraph::default();
        match self.content.kind(range.start) {
            Kind::Heading(n) => {
                p.above = if *n <= 3 { m::SPACING * 2.0 } else { m::SPACING };
                p.below = m::TIGHT;
            }
            Kind::Body => p.below = m::TIGHT,
            Kind::Quote => {
                p.left = m::WINDOW_INSET;
                p.bar = Some(style::c(self.palette.control_highlight));
                p.below = m::TIGHT;
            }
            Kind::List => {
                p.left = m::SPACING;
                p.below = m::TIGHT;
            }
            Kind::Code(lang) => {
                p.left = m::SPACING;
                p.right = m::SPACING;
                p.verbatim = true;
                p.shade = Some(style::c(olo_theme::mix(self.palette.document_background, self.palette.document_text, 0.04)));
                if lang == "diff" {
                    let line = &self.content.text[range.clone()];
                    if line.starts_with('+') {
                        p.shade = Some(style::c(olo_theme::mix(self.palette.document_background, self.palette.success, 0.12)));
                    } else if line.starts_with('-') {
                        p.shade = Some(style::c(olo_theme::mix(self.palette.document_background, self.palette.failure, 0.10)));
                    }
                }
            }
            Kind::Table { columns, header } => {
                p.row = Some(Row { columns: vec![1.0; *columns], header: *header, ..Default::default() });
                p.verbatim = true;
            }
            Kind::Rule => {
                p.rule = Some(style::c(self.palette.control_border));
                p.above = m::SPACING;
                p.below = m::SPACING;
            }
        }
        p
    }
    fn link_at(&mut self, at: usize) -> Option<String> {
        self.content.runs(at..at + 1).find_map(|r| r.style.link.clone())
    }
    fn replace(&mut self, _: Range<usize>, _: &str, _: Range<usize>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rich_text_preserves_unicode_styles_tables_and_syntax() {
        let mut renderer = Renderer::default();
        let doc =
            renderer.render("# Result\n\n**Café** and [code](https://example.com)\n\n| name | value |\n|---|---|\n| α | 42 |\n\n```rust\nfn main() { println!(\"你好\"); }\n```\n\n- [x] verified");
        assert!(doc.text.contains("Café and code"));
        assert!(doc.text.contains("α\t42"));
        assert!(doc.runs.iter().any(|r| r.style.class == Some(Class::Keyword)));
        assert!(doc.runs.iter().any(|r| r.style.link.is_some()));
        assert!(doc.paragraphs.iter().any(|p| matches!(p.kind, Kind::Table { header: true, .. })));
        let mut at = 0;
        for r in &doc.runs {
            assert_eq!(r.range.start, at);
            assert!(doc.text.is_char_boundary(r.range.end));
            at = r.range.end;
        }
        assert_eq!(at, doc.text.len());
        let again = renderer.render("```rust\nfn main() { println!(\"你好\"); }\n```");
        assert!(!again.runs.is_empty());
        assert_eq!(renderer.blocks.len(), 1);
    }
}
