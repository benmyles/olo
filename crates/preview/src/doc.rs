//! Formatted documents (Markdown, Word, OpenDocument text and slides,
//! PowerPoint) as blocks of styled text, and spreadsheets (Excel,
//! OpenDocument, CSV) as tables. All read natively: zip + XML, calamine,
//! pulldown-cmark.

use std::{io::Read, path::Path};

use olo_kit::style::Syntax;

use super::load::{Body, Image, Update};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: bool,
    /// Where a link goes (Markdown's; office documents' links have none).
    pub url: Option<String>,
}

/// How a table's column lines up its cells.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// A GitHub alert: a quote that starts `[!NOTE]` and the like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Callout {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl Callout {
    pub fn title(self) -> &'static str {
        match self {
            Callout::Note => "Note",
            Callout::Tip => "Tip",
            Callout::Important => "Important",
            Callout::Warning => "Warning",
            Callout::Caution => "Caution",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    /// A list item: nesting depth and its bullet, number or task box
    /// ("☐", "☑"); "" for a further paragraph of the item above.
    Item(u8, String, Vec<Span>),
    /// A block quote begins (a callout's, if it is one): what comes
    /// before its `QuoteEnd` is quoted. They nest.
    QuoteStart(Option<Callout>),
    QuoteEnd,
    /// Code: its lines, each line's token classes (byte ranges; none when
    /// its language is unknown), indented `depth` list levels.
    Code { lines: Vec<String>, tokens: Vec<Vec<(u32, u32, Syntax)>>, depth: u8 },
    /// Rows of cells, the first the header; each column's alignment.
    Table { align: Vec<Align>, rows: Vec<Row> },
    /// A picture on its own (Markdown's, from beside the document), at
    /// most `MAX_PICTURE` wide, and halved again and again.
    Picture { image: Image, smaller: Vec<Image>, alt: String },
    Rule,
}

impl Block {
    /// A table of plain cells, lined up left.
    pub fn plain_table(rows: Vec<Vec<String>>) -> Block {
        let rows = rows.into_iter().map(|r| r.into_iter().map(|text| vec![Span { text, ..Default::default() }]).collect()).collect();
        Block::Table { align: Vec::new(), rows }
    }
}

/// A table's row: its cells, each its text.
pub type Row = Vec<Vec<Span>>;

/// Rows shown per sheet.
const MAX_ROWS: usize = 5000;
const MAX_COLS: usize = 200;
/// A Markdown document's pictures: at most this many, this wide (pixels).
const MAX_PICTURES: usize = 64;
const MAX_PICTURE: u32 = 2400;

// ---- Markdown -------------------------------------------------------------

pub fn markdown(path: &Path) -> Result<Update, String> {
    let text = read_text(path)?;
    let blocks = parse_markdown_in(&text, path.parent());
    Ok(Update::Body(Body::Rich { blocks, note: "Markdown document".into() }))
}

fn read_text(path: &Path) -> Result<String, String> {
    let mut data = Vec::new();
    std::fs::File::open(path).and_then(|f| f.take(4 << 20).read_to_end(&mut data)).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}

pub fn parse_markdown(text: &str) -> Vec<Block> {
    parse_markdown_in(text, None)
}

/// What a Markdown parse is in the middle of.
#[derive(Default)]
struct Md<'a> {
    blocks: Vec<Block>,
    spans: Vec<Span>,
    style: Span,
    /// Open lists: the next number, if ordered.
    lists: Vec<Option<u64>>,
    /// The open item's mark, until its first paragraph takes it.
    mark: Option<String>,
    quotes: usize,
    code: Option<(String, String)>,
    table: Option<(Vec<Align>, Vec<Row>)>,
    cell: Vec<Span>,
    /// An image's address and its alt text so far.
    image: Option<(String, String)>,
    /// Inside an HTML comment (it may run over several events).
    comment: bool,
    /// Where the document is: its pictures are found from here.
    dir: Option<&'a Path>,
    pictures: usize,
}

impl Md<'_> {
    /// The text so far as a paragraph (or a list item's, or the rest of
    /// one).
    fn flush(&mut self) {
        if self.spans.iter().all(|s| s.text.trim().is_empty()) {
            self.spans.clear();
            return;
        }
        // No break or space at either end.
        while self.spans.last().is_some_and(|s| s.text.trim().is_empty()) {
            self.spans.pop();
        }
        let mut s = std::mem::take(&mut self.spans);
        if let Some(first) = s.first_mut() {
            first.text = first.text.trim_start().to_owned();
        }
        if let Some(last) = s.last_mut() {
            last.text = last.text.trim_end().to_owned();
        }
        self.blocks.push(match self.lists.len() {
            0 => Block::Para(s),
            n => Block::Item(n as u8 - 1, self.mark.take().unwrap_or_default(), s),
        });
    }

    fn push(&mut self, text: &str) {
        if let Some((_, alt)) = &mut self.image {
            alt.push_str(text);
        } else if self.table.is_some() {
            self.cell.push(Span { text: text.to_owned(), ..self.style.clone() });
        } else {
            self.spans.push(Span { text: text.to_owned(), ..self.style.clone() });
        }
    }

    /// An image ends: a picture of its own if it is beside the document
    /// and can be read, else its alt text, quietly.
    fn image(&mut self, url: &str, alt: String) {
        if let Some(path) = self.dir.and_then(|d| local(d, url))
            && self.pictures < MAX_PICTURES
            && self.table.is_none()
            && let Some((image, smaller)) = picture(&path)
        {
            self.pictures += 1;
            self.flush();
            self.blocks.push(Block::Picture { image, smaller, alt });
            return;
        }
        if !alt.trim().is_empty() {
            let span = Span { text: alt, italic: true, ..self.style.clone() };
            if self.table.is_some() { self.cell.push(span) } else { self.spans.push(span) }
        }
    }

    /// Raw HTML: its text kept, tags and comments dropped; `<br>` breaks
    /// the line and `<img>` is an image.
    fn html(&mut self, html: &str) {
        let mut rest = html;
        while !rest.is_empty() {
            if self.comment {
                match rest.find("-->") {
                    Some(i) => {
                        self.comment = false;
                        rest = &rest[i + 3..];
                    }
                    None => return,
                }
                continue;
            }
            match rest.find('<') {
                Some(i) => {
                    let text = &rest[..i];
                    if !text.trim().is_empty() {
                        self.push(&entities(text));
                    }
                    rest = &rest[i..];
                    if rest.starts_with("<!--") {
                        self.comment = true;
                        rest = &rest[4..];
                        continue;
                    }
                    let end = rest.find('>').map_or(rest.len(), |e| e + 1);
                    let tag = &rest[..end];
                    let name: String = tag.trim_start_matches(['<', '/']).chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
                    match name.as_str() {
                        "br" => self.push("\n"),
                        "img" => {
                            let url = attribute(tag, "src").unwrap_or_default();
                            self.image(&url, attribute(tag, "alt").unwrap_or_default());
                        }
                        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "details" | "summary" if tag.starts_with("</") => self.flush(),
                        _ => {}
                    }
                    rest = &rest[end..];
                }
                None => {
                    if !rest.trim().is_empty() {
                        self.push(&entities(rest));
                    }
                    return;
                }
            }
        }
    }
}

/// `url` as a file beside `dir`, if it is one (never a web address).
fn local(dir: &Path, url: &str) -> Option<std::path::PathBuf> {
    let url = url.trim();
    if url.is_empty() || url.contains("://") && !url.starts_with("file://") || url.starts_with("data:") {
        return None;
    }
    let url = url.trim_start_matches("file://");
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let path = Path::new(url);
    let path = if path.is_absolute() { path.to_owned() } else { dir.join(path) };
    path.is_file().then_some(path)
}

/// A picture from `path`, at most `MAX_PICTURE` wide, and its halvings.
fn picture(path: &Path) -> Option<(Image, Vec<Image>)> {
    let (img, _) = olo_kit::images::open(path).ok()?;
    let img = if img.width() > MAX_PICTURE {
        let h = (img.height() as u64 * MAX_PICTURE as u64 / img.width() as u64).max(1) as u32;
        img.resize_exact(MAX_PICTURE, h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let image = super::load::color_image(img.to_rgba8());
    let smaller = super::load::halvings(&image);
    Some((image, smaller))
}

/// An HTML tag's attribute (quoted either way, or bare).
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before = lower[..at].chars().last();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let after = tag[from..].trim_start();
        let Some(value) = after.strip_prefix('=') else { continue };
        let value = value.trim_start();
        let v = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
            _ => value.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
        };
        return Some(entities(v));
    }
    None
}

/// HTML's common entities, decoded.
fn entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    s.replace("&nbsp;", "\u{a0}").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&amp;", "&")
}

/// Markdown (CommonMark with GitHub's tables, task lists, strikethrough
/// and alerts, and typographer's quotes and dashes) as blocks; pictures
/// are read from `dir`, the document's folder.
pub fn parse_markdown_in(text: &str, dir: Option<&Path>) -> Vec<Block> {
    use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
    let mut md = Md { dir, ..Default::default() };
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_GFM;
    for ev in Parser::new_ext(text, options) {
        match ev {
            Event::Start(Tag::Heading { .. }) => md.flush(),
            Event::End(TagEnd::Heading(level)) => {
                let s = std::mem::take(&mut md.spans);
                md.blocks.push(Block::Heading(level as u8, s));
            }
            Event::Start(Tag::List(start)) => {
                md.flush();
                md.lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                md.flush();
                md.lists.pop();
            }
            Event::Start(Tag::Item) => {
                md.flush();
                md.mark = Some(match md.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}.", *n - 1)
                    }
                    _ => ["•", "◦", "▪"][(md.lists.len().max(1) - 1) % 3].into(),
                });
            }
            Event::End(TagEnd::Item | TagEnd::Paragraph | TagEnd::HtmlBlock) => md.flush(),
            Event::TaskListMarker(done) => md.mark = Some(if done { "☑" } else { "☐" }.into()),
            Event::Start(Tag::BlockQuote(kind)) => {
                md.flush();
                md.quotes += 1;
                md.blocks.push(Block::QuoteStart(kind.map(|k| match k {
                    BlockQuoteKind::Note => Callout::Note,
                    BlockQuoteKind::Tip => Callout::Tip,
                    BlockQuoteKind::Important => Callout::Important,
                    BlockQuoteKind::Warning => Callout::Warning,
                    BlockQuoteKind::Caution => Callout::Caution,
                })));
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                md.flush();
                md.quotes = md.quotes.saturating_sub(1);
                md.blocks.push(Block::QuoteEnd);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                md.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split([' ', ',', '{']).next().unwrap_or("").to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                md.code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, c)) = md.code.take() {
                    let lines: Vec<String> = c.trim_end_matches('\n').lines().map(|l| l.replace('\t', "    ")).collect();
                    let tokens = if lang.is_empty() { Vec::new() } else { super::text::highlight(&lang, &lines) };
                    md.blocks.push(Block::Code { lines, tokens, depth: md.lists.len() as u8 });
                }
            }
            Event::Start(Tag::Table(align)) => {
                md.flush();
                let align = align
                    .iter()
                    .map(|a| match a {
                        pulldown_cmark::Alignment::Center => Align::Center,
                        pulldown_cmark::Alignment::Right => Align::Right,
                        _ => Align::Left,
                    })
                    .collect();
                md.table = Some((align, Vec::new()));
            }
            Event::End(TagEnd::Table) => {
                if let Some((align, rows)) = md.table.take() {
                    md.blocks.push(Block::Table { align, rows });
                }
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                if let Some((_, rows)) = &mut md.table {
                    rows.push(Vec::new());
                }
            }
            Event::End(TagEnd::TableCell) => {
                let cell = std::mem::take(&mut md.cell);
                if let Some(row) = md.table.as_mut().and_then(|t| t.1.last_mut()) {
                    row.push(cell);
                }
            }
            Event::Start(Tag::Emphasis) => md.style.italic = true,
            Event::End(TagEnd::Emphasis) => md.style.italic = false,
            Event::Start(Tag::Strong) => md.style.bold = true,
            Event::End(TagEnd::Strong) => md.style.bold = false,
            Event::Start(Tag::Strikethrough) => md.style.strike = true,
            Event::End(TagEnd::Strikethrough) => md.style.strike = false,
            Event::Start(Tag::Link { dest_url, .. }) => {
                md.style.link = true;
                md.style.url = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                md.style.link = false;
                md.style.url = None;
            }
            Event::Start(Tag::Image { dest_url, .. }) => md.image = Some((dest_url.to_string(), String::new())),
            Event::End(TagEnd::Image) => {
                if let Some((url, alt)) = md.image.take() {
                    md.image(&url, alt);
                }
            }
            Event::Text(t) => match &mut md.code {
                Some((_, c)) => c.push_str(&t),
                None => md.push(&t),
            },
            Event::Code(t) => {
                if let Some((_, alt)) = &mut md.image {
                    alt.push_str(&t);
                } else {
                    let span = Span { text: t.to_string(), code: true, ..md.style.clone() };
                    if md.table.is_some() { md.cell.push(span) } else { md.spans.push(span) }
                }
            }
            Event::InlineHtml(t) | Event::Html(t) => md.html(&t),
            Event::SoftBreak => md.push(" "),
            Event::HardBreak => md.push("\n"),
            Event::Rule => {
                md.flush();
                md.blocks.push(Block::Rule);
            }
            _ => {}
        }
    }
    md.flush();
    md.blocks
}

// ---- Office documents -----------------------------------------------------

/// Simplified XML events: local names only.
enum X {
    Start(String, Vec<(String, String)>),
    End(String),
    Text(String),
}

fn xml_events(xml: &str) -> Vec<X> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml);
    let mut out = Vec::new();
    let attrs = |e: &quick_xml::events::BytesStart| {
        e.attributes()
            .flatten()
            .map(|a| {
                let k = a.key.local_name().as_ref().to_owned();
                let v = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.into_owned()).unwrap_or_default();
                (k, v)
            })
            .collect::<Vec<_>>()
    };
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => out.push(X::Start(e.local_name().as_ref().to_owned(), attrs(&e))),
            Ok(Event::Empty(e)) => {
                let name = e.local_name().as_ref().to_owned();
                out.push(X::Start(name.clone(), attrs(&e)));
                out.push(X::End(name));
            }
            Ok(Event::End(e)) => out.push(X::End(e.local_name().as_ref().to_owned())),
            Ok(Event::Text(t)) => out.push(X::Text(t.xml10_content().into_owned())),
            Ok(Event::CData(t)) => out.push(X::Text(AsRef::<str>::as_ref(&t).to_owned())),
            Ok(Event::GeneralRef(r)) => {
                let s = match r.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    _ => match r.xml10_content().as_ref() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        _ => String::new(),
                    },
                };
                out.push(X::Text(s));
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn zip_text(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

pub fn office(path: &Path, ext: &str) -> Result<Update, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(file).map_err(|_| "This document can't be read.".to_string())?;
    let (blocks, kind) = match ext {
        "docx" => (docx(&zip_text(&mut z, "word/document.xml").ok_or("This document can't be read.")?), "Word document"),
        "pptx" => {
            let mut slides: Vec<(u32, String)> = (0..z.len())
                .filter_map(|i| z.by_index(i).ok().and_then(|f| f.name().ok().map(|n| n.into_owned())))
                .filter_map(|n| {
                    let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
                    Some((num, n))
                })
                .collect();
            slides.sort();
            let mut blocks = Vec::new();
            for (num, name) in slides {
                blocks.push(Block::Heading(2, vec![Span { text: format!("Slide {num}"), ..Default::default() }]));
                if let Some(x) = zip_text(&mut z, &name) {
                    blocks.extend(pptx_slide(&x));
                }
            }
            (blocks, "PowerPoint presentation")
        }
        "odp" => (odf(&zip_text(&mut z, "content.xml").ok_or("This document can't be read.")?, true), "OpenDocument presentation"),
        _ => (odf(&zip_text(&mut z, "content.xml").ok_or("This document can't be read.")?, false), "OpenDocument text"),
    };
    Ok(Update::Body(Body::Rich { blocks, note: kind.into() }))
}

fn on(attrs: &[(String, String)]) -> bool {
    !matches!(attr(attrs, "val"), Some("0" | "false" | "none"))
}

fn docx(xml: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Span::default();
    let (mut heading, mut list, mut in_text, mut in_ppr) = (0u8, None::<u8>, false, false);
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut cell: Option<String> = None;
    for ev in xml_events(xml) {
        match ev {
            X::Start(n, a) => match n.as_str() {
                "p" => {
                    spans.clear();
                    heading = 0;
                    list = None;
                }
                "pPr" => in_ppr = true,
                "pStyle" => {
                    let v = attr(&a, "val").unwrap_or("").to_lowercase();
                    if let Some(l) = v.strip_prefix("heading").and_then(|l| l.parse::<u8>().ok()) {
                        heading = l.clamp(1, 6);
                    } else if v == "title" {
                        heading = 1;
                    } else if v.contains("list") {
                        list.get_or_insert(0);
                    }
                }
                "ilvl" if in_ppr => list = Some(attr(&a, "val").and_then(|v| v.parse().ok()).unwrap_or(0)),
                "numPr" => {
                    list.get_or_insert(0);
                }
                "r" => style = Span::default(),
                "b" if !in_ppr => style.bold = on(&a),
                "i" if !in_ppr => style.italic = on(&a),
                "strike" if !in_ppr => style.strike = on(&a),
                "t" => in_text = true,
                "tab" if !in_ppr => spans.push(Span { text: "    ".into(), ..style.clone() }),
                "br" => spans.push(Span { text: "\n".into(), ..style.clone() }),
                "tbl" => table = Some(Vec::new()),
                "tr" => {
                    if let Some(t) = &mut table {
                        t.push(Vec::new());
                    }
                }
                "tc" => cell = Some(String::new()),
                _ => {}
            },
            X::End(n) => match n.as_str() {
                "pPr" => in_ppr = false,
                "t" => in_text = false,
                "p" => {
                    let s = std::mem::take(&mut spans);
                    if let Some(c) = &mut cell {
                        if !c.is_empty() {
                            c.push(' ');
                        }
                        c.push_str(&s.iter().map(|s| s.text.as_str()).collect::<String>());
                    } else if heading > 0 {
                        blocks.push(Block::Heading(heading, s));
                    } else if let Some(d) = list {
                        blocks.push(Block::Item(d, "•".into(), s));
                    } else {
                        blocks.push(Block::Para(s));
                    }
                }
                "tc" => {
                    if let (Some(c), Some(row)) = (cell.take(), table.as_mut().and_then(|t| t.last_mut())) {
                        row.push(c);
                    }
                }
                "tbl" => {
                    if let Some(t) = table.take() {
                        blocks.push(Block::plain_table(t));
                    }
                }
                _ => {}
            },
            X::Text(t) if in_text => spans.push(Span { text: t, ..style.clone() }),
            X::Text(_) => {}
        }
    }
    blocks
}

fn pptx_slide(xml: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Span::default();
    let mut in_text = false;
    for ev in xml_events(xml) {
        match ev {
            X::Start(n, a) => match n.as_str() {
                "p" => spans.clear(),
                "rPr" => {
                    style.bold = attr(&a, "b") == Some("1");
                    style.italic = attr(&a, "i") == Some("1");
                }
                "r" => style = Span::default(),
                "t" => in_text = true,
                "br" => spans.push(Span { text: "\n".into(), ..Default::default() }),
                _ => {}
            },
            X::End(n) => match n.as_str() {
                "t" => in_text = false,
                "p" if !spans.is_empty() => blocks.push(Block::Para(std::mem::take(&mut spans))),
                _ => {}
            },
            X::Text(t) if in_text => spans.push(Span { text: t, ..style.clone() }),
            X::Text(_) => {}
        }
    }
    blocks
}

fn odf(xml: &str, slides: bool) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let (mut depth, mut in_par, mut heading, mut slide) = (0u8, 0, 0u8, 0);
    let mut in_body = false;
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut cell: Option<String> = None;
    for ev in xml_events(xml) {
        match ev {
            X::Start(n, a) => match n.as_str() {
                "body" => in_body = true,
                "page" if slides => {
                    slide += 1;
                    let name = attr(&a, "name").filter(|n| !n.starts_with("page")).map(str::to_owned);
                    blocks.push(Block::Heading(2, vec![Span { text: name.unwrap_or(format!("Slide {slide}")), ..Default::default() }]));
                }
                "list" => depth += 1,
                "p" => {
                    in_par += 1;
                    heading = 0;
                    spans.clear();
                }
                "h" => {
                    in_par += 1;
                    heading = attr(&a, "outline-level").and_then(|l| l.parse().ok()).unwrap_or(1u8).clamp(1, 6);
                    spans.clear();
                }
                "s" => spans.push(Span { text: " ".repeat(attr(&a, "c").and_then(|c| c.parse().ok()).unwrap_or(1)), ..Default::default() }),
                "tab" => spans.push(Span { text: "    ".into(), ..Default::default() }),
                "line-break" => spans.push(Span { text: "\n".into(), ..Default::default() }),
                "table" => table = Some(Vec::new()),
                "table-row" => {
                    if let Some(t) = &mut table {
                        t.push(Vec::new());
                    }
                }
                "table-cell" => cell = Some(String::new()),
                _ => {}
            },
            X::End(n) => match n.as_str() {
                "list" => depth = depth.saturating_sub(1),
                "p" | "h" => {
                    in_par -= 1;
                    let s = std::mem::take(&mut spans);
                    if let Some(c) = &mut cell {
                        if !c.is_empty() {
                            c.push(' ');
                        }
                        c.push_str(&s.iter().map(|s| s.text.as_str()).collect::<String>());
                    } else if s.iter().all(|s| s.text.trim().is_empty()) && n == "p" {
                        // Empty paragraphs are spacing; skip.
                    } else if n == "h" {
                        blocks.push(Block::Heading(heading, s));
                    } else if depth > 0 {
                        blocks.push(Block::Item(depth - 1, "•".into(), s));
                    } else {
                        blocks.push(Block::Para(s));
                    }
                }
                "table-cell" => {
                    if let (Some(c), Some(row)) = (cell.take(), table.as_mut().and_then(|t| t.last_mut())) {
                        row.push(c);
                    }
                }
                "table" => {
                    if let Some(t) = table.take() {
                        blocks.push(Block::plain_table(t));
                    }
                }
                _ => {}
            },
            X::Text(t) if in_body && in_par > 0 => spans.push(Span { text: t, ..Default::default() }),
            X::Text(_) => {}
        }
    }
    blocks
}

// ---- Spreadsheets ---------------------------------------------------------

pub fn table(path: &Path, ext: &str) -> Result<Update, String> {
    if ext == "csv" || ext == "tsv" {
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(if ext == "tsv" { b'\t' } else { sniff_delimiter(path) })
            .from_path(path)
            .map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        let mut total = 0;
        for r in rdr.records() {
            let Ok(r) = r else { break };
            total += 1;
            if rows.len() < MAX_ROWS {
                rows.push(r.iter().take(MAX_COLS).map(str::to_owned).collect());
            }
        }
        let note = format!("{} table, {total} row{}", if ext == "tsv" { "TSV" } else { "CSV" }, if total == 1 { "" } else { "s" });
        let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        return Ok(Update::Body(Body::Table { sheets: vec![(name, rows)], note }));
    }
    use calamine::{Data, Reader, open_workbook_auto};
    let mut wb = open_workbook_auto(path).map_err(|e| format!("This spreadsheet can't be read ({e})."))?;
    let mut sheets = Vec::new();
    for name in wb.sheet_names() {
        let Ok(range) = wb.worksheet_range(&name) else { continue };
        let rows: Vec<Vec<String>> = range
            .rows()
            .take(MAX_ROWS)
            .map(|r| {
                r.iter()
                    .take(MAX_COLS)
                    .map(|c| match c {
                        Data::Empty => String::new(),
                        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .collect();
        sheets.push((name, rows));
    }
    let n = sheets.len();
    let kind = match ext {
        "ods" => "OpenDocument spreadsheet",
        _ => "Excel spreadsheet",
    };
    Ok(Update::Body(Body::Table { sheets, note: format!("{kind}, {n} sheet{}", if n == 1 { "" } else { "s" }) }))
}

/// Comma unless the first line has more semicolons or tabs.
fn sniff_delimiter(path: &Path) -> u8 {
    let mut buf = [0u8; 4096];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    let line = buf[..n].split(|&b| b == b'\n').next().unwrap_or(&[]);
    let count = |c: u8| line.iter().filter(|&&b| b == c).count();
    b",;\t".iter().copied().max_by_key(|&c| count(c)).filter(|&c| count(c) > 0).unwrap_or(b',')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &[Span]) -> String {
        s.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn markdown_blocks() {
        let b = parse_markdown("# Title\n\nSome **bold** text.\n\n- one\n- two\n  1. nested\n\n```\ncode\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(matches!(&b[0], Block::Heading(1, s) if plain(s) == "Title"));
        assert!(matches!(&b[1], Block::Para(s) if s.iter().any(|s| s.bold && s.text == "bold")));
        assert!(matches!(&b[2], Block::Item(0, m, s) if m == "•" && plain(s) == "one"));
        assert!(b.iter().any(|x| matches!(x, Block::Item(1, m, s) if m == "1." && plain(s) == "nested")));
        assert!(b.iter().any(|x| matches!(x, Block::Code { lines, .. } if lines == &vec!["code".to_string()])));
        assert!(b.iter().any(|x| matches!(x, Block::Table { rows, .. } if rows.len() == 2 && plain(&rows[1][1]) == "2")));
    }

    #[test]
    fn markdown_details() {
        let b = parse_markdown(
            "1. First\n\n   More of it.\n\n   ```rust\n   fn x() {}\n   ```\n\n> [!WARNING]\n> Careful\n\n| l | c | r |\n|:--|:-:|--:|\n| *a* | `b` | [c](https://c.example) |\n\n<!-- hidden\nstill -->\n<p>Hi<br>there &amp; you</p>\n\n- [x] done\n\n![Logo](https://example.com/logo.png)\n",
        );
        assert!(matches!(&b[0], Block::Item(0, m, s) if m == "1." && plain(s) == "First"));
        // A further paragraph of the item, and its code, stay in it.
        assert!(matches!(&b[1], Block::Item(0, m, s) if m.is_empty() && plain(s) == "More of it."));
        assert!(matches!(&b[2], Block::Code { depth: 1, tokens, .. } if !tokens.is_empty()));
        assert!(matches!(b[3], Block::QuoteStart(Some(Callout::Warning))));
        assert!(matches!(&b[4], Block::Para(s) if plain(s) == "Careful"));
        assert!(matches!(b[5], Block::QuoteEnd));
        let Block::Table { align, rows } = &b[6] else { panic!("{:?}", b[6]) };
        assert_eq!(align, &[Align::Left, Align::Center, Align::Right]);
        assert!(rows[1][0][0].italic && rows[1][1][0].code);
        assert_eq!(rows[1][2][0].url.as_deref(), Some("https://c.example"));
        // The comment goes; the paragraph's text stays, broken where it said.
        assert!(matches!(&b[7], Block::Para(s) if plain(s) == "Hi\nthere & you"), "{:?}", b[7]);
        assert!(matches!(&b[8], Block::Item(0, m, s) if m == "☑" && plain(s) == "done"));
        // A picture from the web isn't fetched: its alt text shows.
        assert!(matches!(&b[9], Block::Para(s) if s[0].italic && plain(s) == "Logo"));
        // Typographer's quotes.
        assert!(matches!(&parse_markdown("\"Quoted\" -- it's")[0], Block::Para(s) if plain(s) == "“Quoted” – it’s"));
    }

    #[test]
    fn markdown_pictures() {
        let dir = std::env::temp_dir().join(format!("olo-md-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbaImage::from_pixel(4, 3, image::Rgba([200, 10, 10, 255])).save(dir.join("a.png")).unwrap();
        let b = parse_markdown_in("Before ![A](a.png) after\n\n<img src=\"./a.png\" alt='B'>\n", Some(&dir));
        assert!(matches!(&b[0], Block::Para(s) if plain(s) == "Before"));
        assert!(matches!(&b[1], Block::Picture { image, alt, .. } if image.size == [4, 3] && alt == "A"));
        assert!(matches!(&b[2], Block::Para(s) if plain(s) == "after"));
        assert!(matches!(&b[3], Block::Picture { alt, .. } if alt == "B"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn word_xml() {
        let xml = r#"<w:document xmlns:w="w"><w:body>
            <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r><w:r><w:t xml:space="preserve"> &amp; plain</w:t></w:r></w:p>
            </w:body></w:document>"#;
        let b = docx(xml);
        assert!(matches!(&b[0], Block::Heading(1, s) if plain(s) == "Report"));
        assert!(matches!(&b[1], Block::Para(s) if s[0].bold && plain(s) == "Bold & plain"));
    }
}
