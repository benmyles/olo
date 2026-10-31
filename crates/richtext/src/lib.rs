//! Formatted text documents as paragraphs of styled spans, read from and
//! written to Word (.docx), OpenDocument text (.odt) and Rich Text Format
//! (.rtf) natively: zip and XML, and RTF's own words, in process. The model is what a word processor's paragraphs and
//! character styles are; an app turns it into its own and back
//! (StyledEdit's `convert.rs`).

use std::path::Path;

mod docx;
mod odt;
mod rtf;
mod xml;

/// What a paragraph is: body text, a title or heading, a quotation or
/// code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Body,
    Title,
    Heading1,
    Heading2,
    Heading3,
    Quote,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// A paragraph's place in a list: numbered or bulleted, how deep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct List {
    pub numbered: bool,
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paragraph {
    pub kind: Kind,
    pub list: Option<List>,
    /// Indent levels outside a list (half an inch each in Word).
    pub indent: u8,
    pub align: Align,
    pub spans: Vec<Span>,
    /// A table here, in the flow of paragraphs, instead of text (its
    /// spans are empty).
    pub table: Option<Table>,
}

/// A table: rows of cells, each cell paragraphs of its own (a table in
/// a cell comes in as its paragraphs). Every row has a cell for each
/// column, and every cell a paragraph at least (`Document::tidy`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub rows: Vec<Vec<Vec<Paragraph>>>,
    /// The first row is a header (repeated at the top of each page).
    pub header: bool,
    /// Each column's width, relative to the others (any unit); none:
    /// as wide as each other.
    pub widths: Vec<f32>,
}

impl Table {
    pub fn columns(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    /// Each column's share of the table's width, adding up to 1.
    pub fn shares(&self) -> Vec<f32> {
        let n = self.columns().max(1);
        let sum: f32 = self.widths.iter().sum();
        if self.widths.len() == n && self.widths.iter().all(|w| *w > 0.0) {
            self.widths.iter().map(|w| w / sum).collect()
        } else {
            vec![1.0 / n as f32; n]
        }
    }

    /// Where each column ends across `total` (whole units, the last at
    /// `total`).
    pub fn edges(&self, total: u32) -> Vec<u32> {
        let mut at = 0.0;
        self.shares()
            .iter()
            .map(|s| {
                at += s;
                (at * total as f32).round().min(total as f32) as u32
            })
            .collect()
    }

    /// Each column's width across `total` (whole units, adding up to it).
    pub fn column_widths(&self, total: u32) -> Vec<u32> {
        let mut last = 0;
        self.edges(total)
            .into_iter()
            .map(|e| {
                let w = e - last;
                last = e;
                w
            })
            .collect()
    }

    /// A cell's text, its paragraphs a space apart.
    pub fn cell_text(cell: &[Paragraph]) -> String {
        cell.iter().map(|p| p.spans.iter().map(|s| s.text.as_str()).collect::<String>()).collect::<Vec<_>>().join(" ")
    }
}

/// Text in one style. Sizes are points; None leaves it to the
/// paragraph's kind.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub size: Option<f32>,
    pub color: Option<[u8; 3]>,
    pub font: Option<String>,
    pub monospace: bool,
    pub link: Option<String>,
}

impl Span {
    /// The same style, other text.
    pub fn with_text(&self, text: impl Into<String>) -> Span {
        Span { text: text.into(), ..self.clone() }
    }

    fn same_style(&self, o: &Span) -> bool {
        Span { text: String::new(), ..self.clone() } == Span { text: String::new(), ..o.clone() }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub paragraphs: Vec<Paragraph>,
    /// The font text without one of its own is set in (else the other
    /// program's default).
    pub font: Option<String>,
}

impl Document {
    /// Neighbouring spans of the same style run together, empty ones go,
    /// and a paragraph holding a line break becomes two; tables' cells
    /// likewise, every row as wide as the widest, every cell with a
    /// paragraph, column widths shares of the width (or none), and a
    /// table with no rows gone.
    pub fn tidy(&mut self) {
        self.paragraphs = tidy(std::mem::take(&mut self.paragraphs));
    }
}

fn tidy(paragraphs: Vec<Paragraph>) -> Vec<Paragraph> {
    let mut out = Vec::with_capacity(paragraphs.len());
    for mut p in paragraphs {
        if let Some(mut t) = p.table.take() {
            t.rows.retain(|r| !r.is_empty());
            if t.rows.is_empty() {
                continue;
            }
            let columns = t.columns();
            if t.widths.len() != columns || t.widths.iter().any(|w| w.is_nan() || *w <= 0.0) {
                t.widths.clear();
            }
            t.widths = if t.widths.is_empty() { Vec::new() } else { t.shares() };
            for row in &mut t.rows {
                row.resize_with(columns, Vec::new);
                for cell in row.iter_mut() {
                    *cell = tidy(std::mem::take(cell));
                    if cell.is_empty() {
                        cell.push(Paragraph::default());
                    }
                }
            }
            out.push(Paragraph { table: Some(t), ..Paragraph::default() });
            continue;
        }
        let spans = std::mem::take(&mut p.spans);
        let mut cur = p.clone();
        for s in spans {
            for (k, piece) in s.text.split('\n').enumerate() {
                if k > 0 {
                    out.push(std::mem::replace(&mut cur, Paragraph { spans: Vec::new(), ..p.clone() }));
                }
                if piece.is_empty() {
                    continue;
                }
                match cur.spans.last_mut() {
                    Some(last) if last.same_style(&s) => last.text.push_str(piece),
                    _ => cur.spans.push(s.with_text(piece)),
                }
            }
        }
        out.push(cur);
    }
    out
}

/// What kind of file this is, by its name: "docx", "odt", "rtf" or None.
pub fn format_of(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "docx" => Some("docx"),
        "odt" => Some("odt"),
        "rtf" => Some("rtf"),
        _ => None,
    }
}

/// Read a Word, OpenDocument text or RTF file.
pub fn read(path: &Path) -> Result<Document, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    match format_of(path) {
        Some("docx") => docx::read(&bytes),
        Some("odt") => odt::read(&bytes),
        Some("rtf") => rtf::read(&bytes),
        _ => Err("This kind of document can't be read.".into()),
    }
}

/// Write a Word, OpenDocument text or RTF file (beside it, then over it).
pub fn write(path: &Path, doc: &Document) -> Result<(), String> {
    let bytes = match format_of(path) {
        Some("docx") => docx::write(doc)?,
        Some("odt") => odt::write(doc)?,
        Some("rtf") => rtf::write(doc)?,
        _ => return Err("This kind of document can't be written.".into()),
    };
    let dir = path.parent().ok_or("no folder")?;
    let name = path.file_name().ok_or("no name")?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.saving"));
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

pub use docx::{read as read_docx, write as write_docx};
pub use odt::{read as read_odt, write as write_odt};
pub use rtf::{read as read_rtf, write as write_rtf};

#[cfg(test)]
mod tests {
    use super::*;

    /// A document using everything the model has.
    pub(crate) fn sample() -> Document {
        let plain = Span::default();
        let p = |kind, spans: Vec<Span>| Paragraph { kind, spans, ..Default::default() };
        Document {
            paragraphs: vec![
                p(Kind::Title, vec![plain.with_text("A Title")]),
                p(Kind::Heading1, vec![plain.with_text("First & <best>")]),
                p(
                    Kind::Body,
                    vec![
                        plain.with_text("Some "),
                        Span { bold: true, ..plain.with_text("bold") },
                        plain.with_text(", "),
                        Span { italic: true, underline: true, ..plain.with_text("italic underlined") },
                        plain.with_text(" and "),
                        Span { strike: true, color: Some([0xc0, 0x10, 0x20]), size: Some(18.0), ..plain.with_text("red struck") },
                        plain.with_text("  two  spaces\tand a tab, "),
                        Span { link: Some("https://example.com/?a=1&b=2".into()), ..plain.with_text("a link") },
                        plain.with_text(" in "),
                        Span { font: Some("Noto Serif".into()), ..plain.with_text("serif") },
                        plain.with_text("."),
                    ],
                ),
                Paragraph { list: Some(List { numbered: false, level: 0 }), ..p(Kind::Body, vec![plain.with_text("bullet")]) },
                Paragraph { list: Some(List { numbered: true, level: 1 }), ..p(Kind::Body, vec![plain.with_text("numbered, deeper")]) },
                Paragraph { list: Some(List { numbered: false, level: 0 }), ..p(Kind::Body, vec![plain.with_text("bullet again")]) },
                p(Kind::Quote, vec![plain.with_text("A quotation.")]),
                p(Kind::Code, vec![Span { monospace: true, ..plain.with_text("let x = 1;") }]),
                Paragraph { align: Align::Center, ..p(Kind::Heading2, vec![plain.with_text("Centred")]) },
                Paragraph { indent: 2, align: Align::Justify, ..p(Kind::Body, vec![plain.with_text("Indented twice.")]) },
                Paragraph {
                    table: Some(Table {
                        header: true,
                        widths: vec![0.75, 0.25],
                        rows: vec![
                            vec![vec![p(Kind::Body, vec![Span { bold: true, ..plain.with_text("Name") }])], vec![p(Kind::Body, vec![Span { bold: true, ..plain.with_text("Size") }])]],
                            vec![
                                vec![p(Kind::Body, vec![plain.with_text("first")]), p(Kind::Body, vec![plain.with_text("two paragraphs")])],
                                vec![Paragraph { align: Align::Right, ..p(Kind::Body, vec![plain.with_text("12")]) }],
                            ],
                            vec![vec![p(Kind::Body, vec![Span { italic: true, ..plain.with_text("second") }])], vec![p(Kind::Body, vec![])]],
                        ],
                    }),
                    ..Default::default()
                },
                p(Kind::Heading3, vec![plain.with_text("Last")]),
                p(Kind::Body, vec![]),
            ],
            font: Some("Noto Sans".into()),
        }
    }

    #[test]
    fn tidy_joins_and_splits() {
        let mut d = Document {
            paragraphs: vec![Paragraph {
                spans: vec![Span::default().with_text("a"), Span::default().with_text("b\nc"), Span { bold: true, ..Span::default().with_text("") }],
                ..Default::default()
            }],
            font: None,
        };
        d.tidy();
        assert_eq!(d.paragraphs.len(), 2);
        assert_eq!(d.paragraphs[0].spans, vec![Span::default().with_text("ab")]);
        assert_eq!(d.paragraphs[1].spans, vec![Span::default().with_text("c")]);
    }

    /// The sample written to `$OLO_RICHTEXT_OUT/sample.docx`, `.odt` and `.rtf`,
    /// for opening in other programs (set to check them by hand).
    #[test]
    fn writes_samples() {
        let Some(dir) = std::env::var_os("OLO_RICHTEXT_OUT") else { return };
        let dir = std::path::PathBuf::from(dir);
        write(&dir.join("sample.docx"), &sample()).unwrap();
        write(&dir.join("sample.odt"), &sample()).unwrap();
        write(&dir.join("sample.rtf"), &sample()).unwrap();
    }

    /// The document at `$OLO_RICHTEXT_READ` as read, printed (set to see
    /// what another program's file comes in as).
    #[test]
    fn reads_named_file() {
        let Some(path) = std::env::var_os("OLO_RICHTEXT_READ") else { return };
        let doc = read(std::path::Path::new(&path)).unwrap();
        println!("font {:?}", doc.font);
        for p in &doc.paragraphs {
            println!("{:?} {:?} indent {} {:?}: {:?}", p.kind, p.list, p.indent, p.align, p.spans);
        }
    }

    #[test]
    fn docx_round_trip() {
        let doc = sample();
        let back = read_docx(&write_docx(&doc).unwrap()).unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn odt_round_trip() {
        let doc = sample();
        let back = read_odt(&write_odt(&doc).unwrap()).unwrap();
        assert_eq!(back, doc);
    }
}
