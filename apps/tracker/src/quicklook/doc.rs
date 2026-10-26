//! Formatted documents (Markdown, Word, OpenDocument text and slides,
//! PowerPoint) as blocks of styled text, and spreadsheets (Excel,
//! OpenDocument, CSV) as tables. All read natively: zip + XML, calamine,
//! pulldown-cmark.

use std::{io::Read, path::Path};

use super::load::{Body, Update};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    /// A list item: nesting depth and its bullet or number.
    Item(u8, String, Vec<Span>),
    Quote(Vec<Span>),
    Code(Vec<String>),
    Table(Vec<Vec<String>>),
    Rule,
}

/// Rows shown per sheet.
const MAX_ROWS: usize = 5000;
const MAX_COLS: usize = 200;

// ---- Markdown -------------------------------------------------------------

pub fn markdown(path: &Path) -> Result<Update, String> {
    let text = read_text(path)?;
    let blocks = parse_markdown(&text);
    Ok(Update::Body(Body::Rich { blocks, note: "Markdown document".into() }))
}

fn read_text(path: &Path) -> Result<String, String> {
    let mut data = Vec::new();
    std::fs::File::open(path).and_then(|f| f.take(4 << 20).read_to_end(&mut data)).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}

pub fn parse_markdown(text: &str) -> Vec<Block> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut blocks = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Span::default();
    // Open lists: the next number, if ordered.
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item: Option<String> = None;
    let mut quote = 0;
    let mut code: Option<String> = None;
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut cell = String::new();

    let flush = |spans: &mut Vec<Span>, blocks: &mut Vec<Block>, item: &mut Option<String>, depth: usize, quote: i32| {
        if spans.iter().all(|s| s.text.trim().is_empty()) {
            spans.clear();
            return;
        }
        let s = std::mem::take(spans);
        blocks.push(match item.take() {
            Some(mark) => Block::Item(depth.saturating_sub(1) as u8, mark, s),
            None if quote > 0 => Block::Quote(s),
            None => Block::Para(s),
        });
    };

    for ev in Parser::new_ext(text, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS) {
        let is_code = matches!(ev, Event::Code(_));
        match ev {
            Event::Start(Tag::Heading { .. }) => spans.clear(),
            Event::End(TagEnd::Heading(level)) => blocks.push(Block::Heading(level as u8, std::mem::take(&mut spans))),
            Event::Start(Tag::List(start)) => {
                flush(&mut spans, &mut blocks, &mut item, lists.len(), quote);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut spans, &mut blocks, &mut item, lists.len(), quote);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut spans, &mut blocks, &mut item, lists.len(), quote);
                item = Some(match lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}.", *n - 1)
                    }
                    _ => if lists.len() > 1 { "◦" } else { "•" }.into(),
                });
            }
            Event::End(TagEnd::Item) | Event::End(TagEnd::Paragraph) => {
                flush(&mut spans, &mut blocks, &mut item, lists.len(), quote)
            }
            Event::TaskListMarker(done) => item = Some(if done { "☑" } else { "☐" }.into()),
            Event::Start(Tag::BlockQuote(_)) => quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote -= 1,
            Event::Start(Tag::CodeBlock(_)) => code = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(c) = code.take() {
                    blocks.push(Block::Code(c.trim_end_matches('\n').lines().map(|l| l.replace('\t', "    ")).collect()));
                }
            }
            Event::Start(Tag::Table(_)) => table = Some(Vec::new()),
            Event::End(TagEnd::Table) => {
                if let Some(t) = table.take() {
                    blocks.push(Block::Table(t));
                }
            }
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                if let Some(t) = &mut table {
                    t.push(Vec::new());
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(row) = table.as_mut().and_then(|t| t.last_mut()) {
                    row.push(std::mem::take(&mut cell));
                }
            }
            Event::Start(Tag::Emphasis) => style.italic = true,
            Event::End(TagEnd::Emphasis) => style.italic = false,
            Event::Start(Tag::Strong) => style.bold = true,
            Event::End(TagEnd::Strong) => style.bold = false,
            Event::Start(Tag::Strikethrough) => style.strike = true,
            Event::End(TagEnd::Strikethrough) => style.strike = false,
            Event::Start(Tag::Link { .. }) => style.link = true,
            Event::End(TagEnd::Link) => style.link = false,
            Event::Text(t) | Event::Code(t) | Event::InlineHtml(t) | Event::Html(t) => {
                if let Some(c) = &mut code {
                    c.push_str(&t);
                } else if table.is_some() {
                    cell.push_str(&t);
                } else {
                    spans.push(Span { text: t.to_string(), code: is_code, ..style.clone() });
                }
            }
            Event::SoftBreak => spans.push(Span { text: " ".into(), ..style.clone() }),
            Event::HardBreak => spans.push(Span { text: "\n".into(), ..style.clone() }),
            Event::Rule => blocks.push(Block::Rule),
            _ => {}
        }
    }
    flush(&mut spans, &mut blocks, &mut item, lists.len(), quote);
    blocks
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
                        blocks.push(Block::Table(t));
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
                        blocks.push(Block::Table(t));
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
        assert!(b.iter().any(|x| matches!(x, Block::Code(l) if l == &vec!["code".to_string()])));
        assert!(b.iter().any(|x| matches!(x, Block::Table(t) if t.len() == 2 && t[1] == vec!["1", "2"])));
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
