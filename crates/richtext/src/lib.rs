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
    /// Indents beyond the levels (a ruler's), in points: `left` added to
    /// `indent`'s, `first` the first line's further in from it, `right`
    /// from the text's right edge. (Outside lists: a list's indents are
    /// its levels'.)
    pub left: f32,
    pub first: f32,
    pub right: f32,
    /// Tab stops, in points from the text's left edge, left to right.
    pub tabs: Vec<Tab>,
    /// A picture here instead of text (its spans are empty), placed as
    /// the paragraph is aligned.
    pub picture: Option<Picture>,
}

/// A picture in the flow of paragraphs ([`Paragraph::picture`]): its file
/// as it is, and how big it is on the page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Picture {
    /// The image file's bytes (PNG, JPEG, GIF, WebP, …; [`Picture::kind`]).
    pub data: Vec<u8>,
    /// Points across and down the page.
    pub width: f32,
    pub height: f32,
    /// What it shows, for whoever can't see it.
    pub alt: String,
}

impl Picture {
    /// Its type, from its first bytes: the file extension and media
    /// type, or None for a kind of file this doesn't know.
    pub fn kind(&self) -> Option<(&'static str, &'static str)> {
        kind_of_image(&self.data)
    }
}

/// An image file's size in pixels, from its header (PNG, JPEG, GIF,
/// WebP, BMP).
pub fn pixels(data: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*data.get(i)?, *data.get(i + 1)?]) as u32);
    let le16 = |i: usize| Some(u16::from_le_bytes([*data.get(i)?, *data.get(i + 1)?]) as u32);
    let be32 = |i: usize| Some(u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?));
    let le32 = |i: usize| Some(u32::from_le_bytes(data.get(i..i + 4)?.try_into().ok()?));
    match kind_of_image(data)?.0 {
        "png" => Some((be32(16)?, be32(20)?)),
        "gif" => Some((le16(6)?, le16(8)?)),
        "bmp" => Some((le32(18)?, le32(22)?.cast_signed().unsigned_abs())),
        "webp" => match data.get(12..16)? {
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let b = le32(21)?;
                Some(((b & 0x3fff) + 1, ((b >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => {
                let w = le32(24)? & 0xff_ffff;
                let h = le32(27)? & 0xff_ffff;
                Some((w + 1, h + 1))
            }
            _ => None,
        },
        // JPEG: the first start-of-frame marker's.
        _ => {
            let mut i = 2;
            while i + 9 < data.len() {
                if data[i] != 0xff {
                    i += 1;
                    continue;
                }
                let marker = data[i + 1];
                if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                    return Some((be16(i + 7)?, be16(i + 5)?));
                }
                if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
                    i += 2;
                    continue;
                }
                i += 2 + be16(i + 2)? as usize;
            }
            None
        }
    }
}

/// An image file's type from its first bytes: its extension and media
/// type.
pub fn kind_of_image(data: &[u8]) -> Option<(&'static str, &'static str)> {
    Some(match data {
        [0x89, b'P', b'N', b'G', ..] => ("png", "image/png"),
        [0xff, 0xd8, 0xff, ..] => ("jpeg", "image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => ("gif", "image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ("webp", "image/webp"),
        [b'B', b'M', ..] => ("bmp", "image/bmp"),
        _ => return None,
    })
}

/// A tab stop: where, and how it lines up the text after a tab.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tab {
    pub x: f32,
    pub align: TabAlign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabAlign {
    /// The text starts at it.
    #[default]
    Left,
    /// It is centred on it.
    Center,
    /// It ends at it.
    Right,
    /// Its decimal point is at it.
    Decimal,
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
    /// What each page shows above and below its text: its left, centre
    /// and right parts, `{page}` and `{pages}` standing for the page's
    /// number and how many pages there are.
    pub header: [String; 3],
    pub footer: [String; 3],
}

/// A piece of a header's or footer's part: text, or a field.
#[derive(Debug, PartialEq)]
pub(crate) enum Bit<'a> {
    Text(&'a str),
    Page,
    Pages,
}

/// `part` as its text and fields, in order.
pub(crate) fn bits(part: &str) -> Vec<Bit<'_>> {
    let mut out = Vec::new();
    let mut rest = part;
    while let Some(at) = rest.find('{') {
        let (field, len) = if rest[at..].starts_with("{pages}") {
            (Bit::Pages, 7)
        } else if rest[at..].starts_with("{page}") {
            (Bit::Page, 6)
        } else {
            out.push(Bit::Text(&rest[..=at]));
            rest = &rest[at + 1..];
            continue;
        };
        if at > 0 {
            out.push(Bit::Text(&rest[..at]));
        }
        out.push(field);
        rest = &rest[at + len..];
    }
    if !rest.is_empty() {
        out.push(Bit::Text(rest));
    }
    out
}

/// A header's or footer's text as read (its parts a tab apart, fields as
/// `{page}` and `{pages}`) made its three parts: one part alone goes where
/// its paragraph's alignment puts it.
pub(crate) fn parts(text: &str, align: Align) -> [String; 3] {
    let text = text.trim_matches(['\n', ' ']);
    let mut v: Vec<String> = text.split('\t').map(|s| s.trim().to_owned()).collect();
    if v.len() == 1 {
        let one = v.remove(0);
        return match align {
            Align::Center => [String::new(), one, String::new()],
            Align::Right => [String::new(), String::new(), one],
            _ => [one, String::new(), String::new()],
        };
    }
    v.resize(3, String::new());
    // (More than three: the rest goes on the right.)
    let right = v[2..].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
    [v[0].clone(), v[1].clone(), right]
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
        // A picture: alone in its paragraph, which keeps its alignment.
        if let Some(picture) = p.picture.take() {
            if picture.data.is_empty() {
                continue;
            }
            out.push(Paragraph { picture: Some(picture), align: p.align, ..Paragraph::default() });
            continue;
        }
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
    /// A picture: a 16-pixel icon, set at 48 points.
    pub(crate) fn printer() -> Picture {
        Picture { data: include_bytes!("../../../data/icons/16/printer.png").to_vec(), width: 48.0, height: 48.0, alt: "A printer".into() }
    }

    #[test]
    fn pictures_kinds_and_sizes() {
        let p = printer();
        assert_eq!(p.kind(), Some(("png", "image/png")));
        assert_eq!(pixels(&p.data), Some((16, 16)));
        assert_eq!(kind_of_image(b"hello"), None);
    }

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
                    left: 18.0,
                    first: 36.0,
                    right: 9.0,
                    tabs: vec![Tab { x: 144.0, align: TabAlign::Right }, Tab { x: 216.0, align: TabAlign::Decimal }, Tab { x: 288.0, align: TabAlign::Center }, Tab { x: 360.0, align: TabAlign::Left }],
                    ..p(Kind::Body, vec![plain.with_text("Ruled\t1\t2.5\tmid\tend")])
                },
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
                Paragraph { align: Align::Center, picture: Some(printer()), ..Default::default() },
                p(Kind::Heading3, vec![plain.with_text("Last")]),
                p(Kind::Body, vec![]),
            ],
            font: Some("Noto Sans".into()),
            header: ["Olo".into(), String::new(), "{page} of {pages}".into()],
            footer: [String::new(), "Page {page}".into(), String::new()],
        }
    }

    #[test]
    fn tidy_joins_and_splits() {
        let mut d = Document {
            paragraphs: vec![Paragraph {
                spans: vec![Span::default().with_text("a"), Span::default().with_text("b\nc"), Span { bold: true, ..Span::default().with_text("") }],
                ..Default::default()
            }],
            ..Default::default()
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
