//! Documents to and from other formats: Markdown in (Import…) and out,
//! HTML out (Export). Each works on the text, its runs and its paragraphs
//! (a paragraph a line), so it can run on a worker with a copy of them.

use std::ops::Range;

use crate::doc::{Align, DEFAULT_SIZE, Kind, List, MONOSPACE, Para, Run, Style};

/// Links' colour, as Import sets it.
const LINK: [u8; 3] = [0x1f, 0x5f, 0xc4];

/// A Markdown document as text, runs and paragraphs.
pub fn from_markdown(source: &str) -> (String, Vec<Run>, Vec<Para>) {
    use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
    let mut b = Builder::default();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_GFM;
    // Lists open (numbered or not), quotes open, inline styles on.
    let mut lists: Vec<bool> = Vec::new();
    let mut quotes = 0;
    let (mut bold, mut italic, mut strike) = (0, 0, 0);
    let mut links: Vec<String> = Vec::new();
    let mut code_block = false;
    let mut in_cell = false;
    for e in Parser::new_ext(source, options) {
        match e {
            Event::Start(Tag::Heading { level, .. }) => {
                b.start(Para {
                    kind: match level {
                        HeadingLevel::H1 => Kind::Heading1,
                        HeadingLevel::H2 => Kind::Heading2,
                        _ => Kind::Heading3,
                    },
                    ..Para::default()
                });
            }
            Event::Start(Tag::Paragraph) => {
                // An item's first paragraph is the item itself.
                if !b.open {
                    b.start(Para { kind: if quotes > 0 { Kind::Quote } else { Kind::Body }, level: lists.len() as u8, ..Para::default() });
                }
            }
            Event::Start(Tag::Item) => {
                let numbered = lists.last().copied().unwrap_or(false);
                b.start(Para {
                    list: if numbered { List::Number } else { List::Bullet },
                    level: lists.len().saturating_sub(1) as u8,
                    kind: if quotes > 0 { Kind::Quote } else { Kind::Body },
                    ..Para::default()
                });
            }
            Event::Start(Tag::List(start)) => {
                b.end();
                lists.push(start.is_some());
            }
            Event::End(TagEnd::List(_)) => {
                b.end();
                lists.pop();
            }
            Event::Start(Tag::BlockQuote(_)) => {
                b.end();
                quotes += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                b.end();
                quotes -= 1;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                b.end();
                code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                b.end();
                code_block = false;
            }
            Event::End(TagEnd::Heading(_) | TagEnd::Paragraph | TagEnd::Item | TagEnd::TableRow | TagEnd::TableHead) => b.end(),
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                b.start(Para::default());
                in_cell = false;
            }
            Event::Start(Tag::TableCell) => {
                if in_cell {
                    b.text("\t", &Style::default());
                }
                in_cell = true;
            }
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic -= 1,
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold -= 1,
            Event::Start(Tag::Strikethrough) => strike += 1,
            Event::End(TagEnd::Strikethrough) => strike -= 1,
            Event::Start(Tag::Link { dest_url, .. }) => links.push(dest_url.to_string()),
            Event::End(TagEnd::Link) => {
                links.pop();
            }
            Event::Text(t) if code_block => {
                // A line a paragraph, set as code.
                let mut s = Style::default();
                Kind::Code.apply(&mut s);
                for line in t.strip_suffix('\n').unwrap_or(&t).split('\n') {
                    b.start(Para { kind: Kind::Code, level: lists.len() as u8, ..Para::default() });
                    b.text(line, &s);
                }
            }
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => {
                let mut s = b.base();
                s.bold |= bold > 0;
                s.italic |= italic > 0;
                s.strikeout |= strike > 0;
                if let Some(url) = links.last() {
                    s.underline = true;
                    s.color = Some(LINK);
                    s.link = Some(url.clone());
                }
                b.text(&t, &s);
            }
            Event::Code(t) => {
                let mut s = b.base();
                s.family = MONOSPACE.into();
                b.text(&t, &s);
            }
            Event::TaskListMarker(done) => b.text(if done { "☑ " } else { "☐ " }, &b.base()),
            Event::SoftBreak => b.text(" ", &b.base()),
            Event::HardBreak => {
                let p = b.para.clone();
                b.end();
                b.start(p);
            }
            Event::Rule => {
                b.start(Para { align: Some(Align::Center), ..Para::default() });
                b.text("* * *", &Style::default());
                b.end();
            }
            _ => {}
        }
    }
    b.end();
    b.finish()
}

/// Paragraphs and runs as they are made.
#[derive(Default)]
struct Builder {
    text: String,
    runs: Vec<Run>,
    paras: Vec<Para>,
    /// The paragraph being filled, and that one is.
    para: Para,
    open: bool,
}

impl Builder {
    fn start(&mut self, para: Para) {
        self.end();
        if !self.paras.is_empty() {
            // The newline ending the one before, in its style.
            let style = self.runs.last().map(|r| r.style.clone()).unwrap_or_default();
            self.push("\n", &style);
        }
        self.para = para.clone();
        self.paras.push(para);
        self.open = true;
    }

    fn end(&mut self) {
        self.open = false;
    }

    /// The style the paragraph's kind gives its text.
    fn base(&self) -> Style {
        let mut s = Style::default();
        self.para.kind.apply(&mut s);
        s
    }

    fn text(&mut self, t: &str, style: &Style) {
        if !self.open {
            let kind = if self.para.kind == Kind::Code { Kind::Body } else { self.para.kind };
            self.start(Para { kind, ..Para::default() });
        }
        self.push(t, style);
    }

    fn push(&mut self, t: &str, style: &Style) {
        if t.is_empty() {
            return;
        }
        self.text.push_str(t);
        match self.runs.last_mut() {
            Some(r) if r.style == *style => r.len += t.len(),
            _ => self.runs.push(Run { len: t.len(), style: style.clone() }),
        }
    }

    fn finish(mut self) -> (String, Vec<Run>, Vec<Para>) {
        if self.paras.is_empty() {
            self.paras.push(Para::default());
        }
        (self.text, self.runs, self.paras)
    }
}

/// Each paragraph's bytes (without its newline), with its setting.
fn paragraphs<'a>(text: &'a str, paras: &'a [Para]) -> impl Iterator<Item = (Range<usize>, Para)> + 'a {
    let mut at = 0;
    text.split('\n').enumerate().map(move |(i, line)| {
        let r = at..at + line.len();
        at = r.end + 1;
        (r, paras.get(i).cloned().unwrap_or_default())
    })
}

/// The runs over `range`, as (bytes, style), cut to it.
fn spans<'a>(runs: &'a [Run], range: Range<usize>) -> impl Iterator<Item = (Range<usize>, &'a Style)> + 'a {
    let mut at = 0;
    runs.iter().filter_map(move |r| {
        let (a, b) = (at, at + r.len);
        at = b;
        let (a, b) = (a.max(range.start), b.min(range.end));
        (b > a).then_some((a..b, &r.style))
    })
}

/// The document as Markdown: headings, lists, quotes and code blocks;
/// bold, italic, struck-out and code text. (Sizes, colours and fonts have
/// no Markdown, and are left out.)
pub fn to_markdown(text: &str, runs: &[Run], paras: &[Para]) -> String {
    let mut out = String::new();
    // Whether the last block written was a list item, or code (open fence).
    let mut last_item = false;
    let mut fence = false;
    let mut any = false;
    for (r, p) in paragraphs(text, paras) {
        if fence && p.kind != Kind::Code {
            out.push_str("```\n");
            fence = false;
        }
        if p.kind == Kind::Code {
            if !fence {
                if any {
                    out.push('\n');
                }
                out.push_str("```\n");
                fence = true;
                any = true;
            }
            out.push_str(&text[r]);
            out.push('\n');
            last_item = false;
            continue;
        }
        if r.is_empty() && p.list == List::None {
            continue;
        }
        let item = p.list != List::None;
        if any && !(item && last_item) {
            out.push('\n');
        }
        any = true;
        last_item = item;
        let indent = "    ".repeat(p.level as usize);
        let lead = match p.kind {
            Kind::Title | Kind::Heading1 => "# ",
            Kind::Heading2 => "## ",
            Kind::Heading3 => "### ",
            Kind::Quote => "> ",
            _ => "",
        };
        out.push_str(&indent);
        out.push_str(lead);
        match p.list {
            List::Bullet => out.push_str("- "),
            List::Number => out.push_str("1. "),
            List::None => {}
        }
        let heading = matches!(p.kind, Kind::Title | Kind::Heading1 | Kind::Heading2 | Kind::Heading3);
        out.push_str(&inline_markdown(text, runs, r, heading, p.kind == Kind::Quote, lead.is_empty() && !item));
        out.push('\n');
    }
    if fence {
        out.push_str("```\n");
    }
    out
}

/// One paragraph's text with Markdown's marks for its styles. A heading's
/// bold and a quote's italics are the paragraph's, not marked.
fn inline_markdown(text: &str, runs: &[Run], range: Range<usize>, heading: bool, quote: bool, line_start: bool) -> String {
    // Spans with the same marks run together.
    type Marks<'a> = (bool, bool, bool, bool, Option<&'a str>);
    let mut parts: Vec<(String, Marks)> = Vec::new();
    for (r, s) in spans(runs, range) {
        let marks = (s.family == MONOSPACE, s.bold && !heading, s.italic && !quote, s.strikeout, s.link.as_deref());
        match parts.last_mut() {
            Some((t, m)) if *m == marks => t.push_str(&text[r]),
            _ => parts.push((text[r].to_owned(), marks)),
        }
    }
    let mut out = String::new();
    for (k, (t, (code, bold, italic, strike, link))) in parts.into_iter().enumerate() {
        // Marks hug the text: spaces at either end go outside them.
        let body = t.trim();
        let (lead, tail) = (&t[..t.len() - t.trim_start().len()], &t[t.trim_end().len()..]);
        out.push_str(lead);
        if body.is_empty() {
            out.push_str(tail);
            continue;
        }
        if link.is_some() {
            out.push('[');
        }
        if code {
            let ticks = if body.contains('`') { "``" } else { "`" };
            out.push_str(ticks);
            out.push_str(body);
            out.push_str(ticks);
        } else {
            let marks = format!("{}{}{}", if strike { "~~" } else { "" }, if bold { "**" } else { "" }, if italic { "*" } else { "" });
            out.push_str(&marks);
            out.push_str(&escape_markdown(body, line_start && k == 0 && lead.is_empty()));
            out.extend(marks.chars().rev());
        }
        if let Some(url) = link {
            out.push_str("](");
            out.push_str(&url.replace(' ', "%20").replace(')', "%29"));
            out.push(')');
        }
        out.push_str(tail);
    }
    out
}

/// Characters Markdown would take as marks, escaped; at a line's start
/// also those that would make it a heading, quote or list.
fn escape_markdown(s: &str, line_start: bool) -> String {
    // "1." at a line's start would number a list: its dot escaped.
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    let number = (line_start && digits > 0 && s[digits..].starts_with(['.', ')'])).then_some(digits);
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        let starts = line_start && i == 0;
        if matches!(c, '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '~') || (starts && matches!(c, '#' | '>' | '-' | '+')) || Some(i) == number {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The document as a web page: its paragraph kinds, lists, alignment and
/// every style (fonts, sizes, colours too).
pub fn to_html(text: &str, runs: &[Run], paras: &[Para], title: &str, align: Align) -> String {
    let mut out = format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n</head>\n<body>\n",
        escape_html(title)
    );
    // The lists open (numbered or not), innermost last; a quote open; code open.
    let mut lists: Vec<bool> = Vec::new();
    let mut quote = false;
    let mut code = false;
    for (r, p) in paragraphs(text, paras) {
        if code && p.kind != Kind::Code {
            out.push_str("</code></pre>\n");
            code = false;
        }
        let depth = if p.list == List::None { 0 } else { p.level as usize + 1 };
        while lists.len() > depth || (lists.len() == depth && depth > 0 && lists.last() != Some(&(p.list == List::Number))) {
            out.push_str(if lists.pop() == Some(true) { "</ol>\n" } else { "</ul>\n" });
        }
        if quote && p.kind != Kind::Quote {
            out.push_str("</blockquote>\n");
            quote = false;
        }
        if p.kind == Kind::Code {
            out.push_str(if code { "\n" } else { "<pre><code>" });
            code = true;
            out.push_str(&escape_html(&text[r]));
            continue;
        }
        if p.kind == Kind::Quote && !quote {
            out.push_str("<blockquote>\n");
            quote = true;
        }
        while lists.len() < depth {
            let numbered = p.list == List::Number;
            out.push_str(if numbered { "<ol>\n" } else { "<ul>\n" });
            lists.push(numbered);
        }
        let tag = match (p.list != List::None, p.kind) {
            (true, _) => "li",
            (_, Kind::Title | Kind::Heading1) => "h1",
            (_, Kind::Heading2) => "h2",
            (_, Kind::Heading3) => "h3",
            _ => "p",
        };
        let style = match p.align {
            Some(Align::Center) => " style=\"text-align: center\"",
            Some(Align::Right) => " style=\"text-align: right\"",
            Some(Align::Justify) => " style=\"text-align: justify\"",
            _ => match align {
                Align::Center => " style=\"text-align: center\"",
                Align::Right => " style=\"text-align: right\"",
                Align::Justify => " style=\"text-align: justify\"",
                Align::Left => "",
            },
        };
        out.push_str(&format!("<{tag}{style}>"));
        let heading = tag.starts_with('h');
        for (r, s) in spans(runs, r.clone()) {
            out.push_str(&inline_html(&text[r], s, heading, p.kind));
        }
        if r.is_empty() {
            out.push_str("<br>");
        }
        out.push_str(&format!("</{tag}>\n"));
    }
    if code {
        out.push_str("</code></pre>\n");
    }
    while let Some(numbered) = lists.pop() {
        out.push_str(if numbered { "</ol>\n" } else { "</ul>\n" });
    }
    if quote {
        out.push_str("</blockquote>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

fn inline_html(t: &str, s: &Style, heading: bool, kind: Kind) -> String {
    let mut css = Vec::new();
    // What the paragraph's kind gives it isn't said again.
    let mut base = Style::default();
    kind.apply(&mut base);
    if !s.family.is_empty() && s.family != MONOSPACE {
        css.push(format!("font-family: '{}'", s.family.replace('\'', "")));
    }
    if s.size != base.size && !(heading && s.size == DEFAULT_SIZE) {
        css.push(format!("font-size: {}pt", s.size));
    }
    if let Some([r, g, b]) = s.color.filter(|c| s.link.is_none() || *c != LINK) {
        css.push(format!("color: #{r:02x}{g:02x}{b:02x}"));
    }
    let mut out = escape_html(t);
    if s.family == MONOSPACE && kind != Kind::Code {
        out = format!("<code>{out}</code>");
    }
    if s.underline && s.link.is_none() {
        out = format!("<u>{out}</u>");
    }
    if s.strikeout {
        out = format!("<s>{out}</s>");
    }
    if s.italic && !base.italic {
        out = format!("<em>{out}</em>");
    }
    if s.bold && !base.bold {
        out = format!("<strong>{out}</strong>");
    }
    if let Some(url) = &s.link {
        out = format!("<a href=\"{}\">{out}</a>", escape_html(url));
    }
    if !css.is_empty() {
        out = format!("<span style=\"{}\">{out}</span>", css.join("; "));
    }
    out
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;

    #[test]
    fn markdown_in() {
        let (text, runs, paras) = from_markdown("# Plan\n\nSome **bold** and `code`.\n\n- one\n- two\n  1. sub\n\n> quoted\n\n```\nfn x() {}\nlet y;\n```\n");
        assert_eq!(text, "Plan\nSome bold and code.\none\ntwo\nsub\nquoted\nfn x() {}\nlet y;");
        let kinds: Vec<(Kind, List, u8)> = paras.iter().map(|p| (p.kind, p.list, p.level)).collect();
        assert_eq!(
            kinds,
            [
                (Kind::Heading1, List::None, 0),
                (Kind::Body, List::None, 0),
                (Kind::Body, List::Bullet, 0),
                (Kind::Body, List::Bullet, 0),
                (Kind::Body, List::Number, 1),
                (Kind::Quote, List::None, 0),
                (Kind::Code, List::None, 0),
                (Kind::Code, List::None, 0),
            ]
        );
        let d = Doc::with_paras(text, Some(runs), Some(paras));
        assert!(d.style_at(d.text().find("bold").unwrap() + 1).bold);
        assert_eq!(d.style_at(d.text().find("code").unwrap() + 1).family, MONOSPACE);
        assert_eq!(d.paras().len(), 8);
    }

    #[test]
    fn markdown_round_trip() {
        let md = "# Plan\n\nSome **bold** and *italic* and `code`, [a link](https://example.com/a).\n\n- one\n- two\n    1. sub\n\n> quoted\n\n```\nfn x() {}\n```\n";
        let (text, runs, paras) = from_markdown(md);
        assert_eq!(to_markdown(&text, &runs, &paras), md);
    }

    #[test]
    fn markdown_escapes() {
        let (text, runs, paras) = from_markdown("1\\. not a list and a \\*star\\*\n");
        assert_eq!(text, "1. not a list and a *star*");
        assert_eq!(to_markdown(&text, &runs, &paras), "1\\. not a list and a \\*star\\*\n");
    }

    #[test]
    fn html_out() {
        let (text, runs, paras) = from_markdown("# T\n\n- a\n    - b\n\nend **x**\n");
        let html = to_html(&text, &runs, &paras, "T & co", Align::Left);
        assert!(html.contains("<title>T &amp; co</title>"));
        assert!(html.contains("<h1>T</h1>\n<ul>\n<li>a</li>\n<ul>\n<li>b</li>\n</ul>\n</ul>\n<p>end <strong>x</strong></p>"), "{html}");
    }
}
