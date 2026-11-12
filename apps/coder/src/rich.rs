//! Read-only rich text prepared on a worker, shared by the native and terminal
//! views. No colours, fonts or scale-dependent measurements are kept here:
//! each paragraph says what it is (its Markdown kind) and which part of the
//! conversation it belongs to (its [`Block`]); each frontend styles those.
//! Fenced code is coloured with the syntax grammars; unchanged blocks keep
//! their captures.
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
/// Which part of the conversation a paragraph belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Block {
    /// The agent's own words, or a command's result.
    #[default]
    Body,
    /// What the person (or a collaborator) said.
    User,
    /// An action's one-line summary.
    Tool(ToolState),
    /// What an action showed when opened (its output, a diff).
    Output,
    /// Something that happened to the session (a pause, a checkpoint).
    Note(Tone),
    /// Reasoning in progress, and other things said quietly.
    Quiet,
}

/// How an action stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ToolState {
    Waiting,
    Running,
    Done,
    Failed,
    Stopped,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tone {
    Info,
    Success,
    Warning,
    Failure,
}

#[derive(Clone, Debug)]
pub struct Paragraph {
    pub start: usize,
    pub kind: Kind,
    pub block: Block,
    /// The first paragraph of an entry in the conversation (room above it).
    pub first: bool,
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
        self.paragraph(at).map(|p| &p.kind).unwrap_or(&Kind::Body)
    }
    /// The paragraph byte `at` is in.
    pub fn paragraph(&self, at: usize) -> Option<&Paragraph> {
        self.paragraphs.get(self.paragraphs.partition_point(|p| p.start <= at).saturating_sub(1))
    }
    /// `other` added at the end, its paragraphs in `block`, the first of
    /// them starting an entry when `first`.
    pub fn append(&mut self, other: &Text, block: Block, first: bool) {
        if other.text.is_empty() {
            return;
        }
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
            if let Some(last) = self.runs.last_mut() {
                last.range.end += 1;
            }
        }
        let at = self.text.len();
        self.text.push_str(&other.text);
        self.runs.extend(other.runs.iter().map(|r| Run { range: r.range.start + at..r.range.end + at, style: r.style.clone() }));
        self.paragraphs.extend(other.paragraphs.iter().enumerate().map(|(i, p)| Paragraph {
            start: p.start + at,
            kind: p.kind.clone(),
            block: if p.block == Block::Body { block } else { p.block },
            first: first && i == 0,
        }));
        // The text ends with a newline, so the next entry starts its own paragraph.
        if !self.text.ends_with('\n') {
            self.text.push('\n');
            if let Some(last) = self.runs.last_mut() {
                last.range.end += 1;
            }
        }
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
                self.out.paragraphs.push(Paragraph { start: self.out.text.len(), kind: kind.clone(), block: Block::Body, first: false });
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
