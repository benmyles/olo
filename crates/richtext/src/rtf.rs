//! Rich Text Format (.rtf). Read as a stream of groups and control
//! words: the font and colour tables, the style sheet's names for the
//! paragraph kinds, lists from the list table (or Word 95's `\pn`),
//! links from fields, tables flattened to a paragraph a row (cells
//! apart by tabs), text in any Windows code page or as `\u`; pictures,
//! headers, footers, notes and whatever a reader may ignore (`\*`) left
//! out. Written as Word and LibreOffice read it: a style sheet and a
//! list table, and each paragraph's formatting spelled out, since an RTF
//! reader takes a paragraph's look from its own words, not its style.

use std::{collections::HashMap, fmt::Write as _};

use crate::{Align, Document, Kind, List, Paragraph, Span, Tab, TabAlign, Table, xml::is_mono};

/// Twips (twentieths of a point) in half an inch: an indent level.
const LEVEL: i32 = 720;
/// The font code is set in.
const MONO: &str = "Courier New";
/// Twips across a page's text (US Letter less inch margins): a table's
/// width, shared out between its columns.
const TEXT_WIDTH: i32 = 9360;
/// The colour links are drawn in (Word's).
const LINK_BLUE: [u8; 3] = [0x05, 0x63, 0xC1];
/// RTF's `\levelnfc` for a bullet, and for no number at all.
const NFC_BULLET: i32 = 23;
const NFC_NONE: i32 = 255;

/// The style sheet: each kind's style number is its place here.
const KINDS: [(Kind, &str); 7] = [
    (Kind::Body, "Normal"),
    (Kind::Heading1, "heading 1"),
    (Kind::Heading2, "heading 2"),
    (Kind::Heading3, "heading 3"),
    (Kind::Title, "Title"),
    (Kind::Quote, "Quote"),
    (Kind::Code, "Source Code"),
];

/// How a kind of paragraph looks (sizes in points, spacing in twips),
/// as the DOCX writer's styles have it: what the writer spells out on
/// each paragraph and the reader takes off again.
struct Look {
    size: f32,
    bold: bool,
    italic: bool,
    mono: bool,
    before: u32,
    after: u32,
}

fn look(kind: Kind) -> Look {
    let l = |size, bold, italic, mono, before, after| Look { size, bold, italic, mono, before, after };
    match kind {
        Kind::Body => l(12.0, false, false, false, 0, 0),
        Kind::Title => l(28.0, true, false, false, 120, 200),
        Kind::Heading1 => l(22.0, true, false, false, 280, 80),
        Kind::Heading2 => l(18.0, true, false, false, 240, 80),
        Kind::Heading3 => l(14.0, true, false, false, 200, 80),
        Kind::Quote => l(12.0, false, true, false, 80, 80),
        Kind::Code => l(12.0, false, false, true, 0, 0),
    }
}

// ---- writing ----

/// Text as RTF: ASCII as it is (`\`, `{` and `}` escaped), the rest as
/// `\u` with a `?` for readers without Unicode.
fn text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut units = [0u16; 2];
    for c in s.chars() {
        match c {
            '\\' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("\\tab "),
            '\n' => out.push_str("\\line "),
            c if (c as u32) < 0x20 => {}
            c if (c as u32) < 0x80 => out.push(c),
            c => {
                for u in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{}?", *u as i16);
                }
            }
        }
    }
    out
}

/// A list's definition: bullets, or numbers counting from 1 at each
/// level.
fn list_def(id: u32, bullet: bool) -> String {
    let mut s = format!("{{\\list\\listtemplateid{id}\\listhybrid");
    for l in 0..9 {
        let (nfc, level_text, numbers) = if bullet {
            (NFC_BULLET, format!("\\'01{}", ["\\u8226?", "\\u9702?", "\\u9642?"][l % 3]), "")
        } else {
            (0, format!("\\'02\\'0{l}."), "\\'01")
        };
        let indent = LEVEL * (l as i32 + 1);
        let _ = write!(
            s,
            "{{\\listlevel\\levelnfc{nfc}\\levelnfcn{nfc}\\leveljc0\\leveljcn0\\levelfollow0\\levelstartat1\\levelspace0\\levelindent0\
             {{\\leveltext\\leveltemplateid{}{level_text};}}{{\\levelnumbers{numbers};}}\\fi-360\\li{indent}\\lin{indent}}}",
            id * 10 + l as u32
        );
    }
    let _ = write!(s, "{{\\listname ;}}\\listid{id}}}");
    s
}

/// What a kind's text looks like, spelled out (after `\plain`).
fn char_look(kind: Kind) -> String {
    let lk = look(kind);
    let mut s = format!("\\f{}\\fs{}", if lk.mono { 1 } else { 0 }, (lk.size * 2.0) as i32);
    if lk.bold {
        s.push_str("\\b");
    }
    if lk.italic {
        s.push_str("\\i");
    }
    s
}

/// What a kind's paragraphs look like (spacing, a quotation's indents).
fn para_look(kind: Kind) -> String {
    let lk = look(kind);
    let mut s = String::new();
    if lk.before > 0 {
        let _ = write!(s, "\\sb{}", lk.before);
    }
    if lk.after > 0 {
        let _ = write!(s, "\\sa{}", lk.after);
    }
    if kind == Kind::Heading1 || kind == Kind::Heading2 || kind == Kind::Heading3 {
        let level = match kind {
            Kind::Heading1 => 0,
            Kind::Heading2 => 1,
            _ => 2,
        };
        let _ = write!(s, "\\keepn\\outlinelevel{level}");
    }
    s
}

/// The fonts and colours the body uses, numbered as it goes.
struct Tables {
    fonts: Vec<String>,
    colors: Vec<[u8; 3]>,
}

impl Tables {
    fn font(&mut self, name: &str) -> usize {
        self.fonts.iter().position(|f| f == name).unwrap_or_else(|| {
            self.fonts.push(name.to_owned());
            self.fonts.len() - 1
        })
    }

    /// Its number in the colour table (0 is "auto").
    fn color(&mut self, c: [u8; 3]) -> usize {
        1 + self.colors.iter().position(|x| *x == c).unwrap_or_else(|| {
            self.colors.push(c);
            self.colors.len() - 1
        })
    }
}

pub fn write(doc: &Document) -> Result<Vec<u8>, String> {
    let mut t = Tables { fonts: vec![doc.font.clone().unwrap_or_else(|| "Times New Roman".into()), MONO.into()], colors: vec![LINK_BLUE] };
    // Each numbered list counts from 1: a list of its own.
    let mut numbered_lists = 0u32;
    let mut in_numbered = false;
    // Where each level of the current numbered list is (for readers
    // that show `\listtext` rather than number lists themselves).
    let mut counts = [0u32; 9];
    let mut body = String::new();
    for p in &doc.paragraphs {
        let numbered = p.list.is_some_and(|l| l.numbered);
        if numbered && !in_numbered {
            numbered_lists += 1;
            counts = [0; 9];
        }
        if p.list.is_none() {
            in_numbered = false;
        } else if numbered {
            in_numbered = true;
        }
        match &p.table {
            Some(table) => write_table(&mut body, table, numbered_lists, &mut counts, &mut t),
            None => paragraph(&mut body, p, numbered_lists, &mut counts, &mut t, None),
        }
    }
    let mut out = String::from("{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1\n{\\fonttbl");
    for (i, f) in t.fonts.iter().enumerate() {
        let family = if i == 1 || is_mono(f) { "\\fmodern" } else { "\\fnil" };
        let _ = write!(out, "{{\\f{i}{family}\\fcharset0 {};}}", text(f));
    }
    out.push_str("}\n{\\colortbl;");
    for [r, g, b] in &t.colors {
        let _ = write!(out, "\\red{r}\\green{g}\\blue{b};");
    }
    out.push_str("}\n{\\stylesheet");
    for (i, (kind, name)) in KINDS.iter().enumerate() {
        let based = if i == 0 { String::new() } else { "\\sbasedon0\\snext0".into() };
        let indent = if *kind == Kind::Quote { "\\li720\\ri720" } else { "" };
        let _ = write!(out, "{{\\s{i}{}{indent}{}{based} {name};}}", para_look(*kind), char_look(*kind));
    }
    out.push_str("}\n{\\*\\listtable");
    out.push_str(&list_def(1, true));
    for k in 1..=numbered_lists {
        out.push_str(&list_def(k + 1, false));
    }
    out.push_str("}\n{\\*\\listoverridetable");
    for id in 1..=numbered_lists + 1 {
        let _ = write!(out, "{{\\listoverride\\listid{id}\\listoverridecount0\\ls{id}}}");
    }
    out.push_str("}\n\\margl1440\\margr1440\\margt1440\\margb1440\\viewkind4\n");
    out.push_str(&body);
    out.push_str("}\n");
    Ok(out.into_bytes())
}

/// Points in twentieths (twips).
fn twips(points: f32) -> i32 {
    (points.max(0.0) * 20.0).round() as i32
}

/// A table, ruled: each row's cells defined (their right edges), then
/// their paragraphs, in the table (`\intbl`), each cell's last ended by
/// `\cell`, and the row by `\row`.
fn write_table(out: &mut String, table: &Table, numbered_list: u32, counts: &mut [u32; 9], t: &mut Tables) {
    let edges = table.edges(TEXT_WIDTH as u32);
    for (r, row) in table.rows.iter().enumerate() {
        out.push_str("\\trowd\\trgaph108\\trleft0");
        if r == 0 && table.header {
            out.push_str("\\trhdr");
        }
        for edge in &edges {
            let _ = write!(out, "\\clbrdrt\\brdrs\\brdrw10\\clbrdrl\\brdrs\\brdrw10\\clbrdrb\\brdrs\\brdrw10\\clbrdrr\\brdrs\\brdrw10\\cellx{edge}");
        }
        out.push('\n');
        for c in 0..edges.len() {
            match row.get(c).filter(|cell| !cell.is_empty()) {
                Some(cell) => {
                    for (k, p) in cell.iter().enumerate() {
                        paragraph(out, p, numbered_list, counts, t, Some(k + 1 == cell.len()));
                    }
                }
                None => out.push_str("\\pard\\plain\\intbl\\cell\n"),
            }
        }
        out.push_str("\\row\n");
    }
}

/// A paragraph, ended by `\par`; in a table (`cell`: whether it is its
/// cell's last), the last of its cell's ends the cell instead.
fn paragraph(out: &mut String, p: &Paragraph, numbered_list: u32, counts: &mut [u32; 9], t: &mut Tables, cell: Option<bool>) {
    let style = KINDS.iter().position(|(k, _)| *k == p.kind).unwrap_or(0);
    let _ = write!(out, "\\pard\\plain{}\\s{style}{}", if cell.is_some() { "\\intbl" } else { "" }, para_look(p.kind));
    let mut marker = None;
    match p.list {
        Some(l) => {
            let level = l.level.min(8) as usize;
            let ls = if l.numbered { numbered_list + 1 } else { 1 };
            let _ = write!(out, "\\ls{ls}\\ilvl{level}\\li{}\\fi-360", LEVEL * (level as i32 + 1));
            marker = Some(if l.numbered {
                counts[level] += 1;
                counts[level + 1..].iter_mut().for_each(|c| *c = 0);
                format!("{}.", counts[level])
            } else {
                ["\u{2022}", "\u{25E6}", "\u{25AA}"][level % 3].to_owned()
            });
        }
        None => {
            let quote = p.kind == Kind::Quote;
            let left = p.indent as i32 * LEVEL + if quote { LEVEL } else { 0 } + twips(p.left);
            if left > 0 {
                let _ = write!(out, "\\li{left}");
            }
            let right = if quote { LEVEL } else { 0 } + twips(p.right);
            if right > 0 {
                let _ = write!(out, "\\ri{right}");
            }
            if p.first > 0.0 {
                let _ = write!(out, "\\fi{}", twips(p.first));
            }
        }
    }
    for tab in &p.tabs {
        let kind = match tab.align {
            TabAlign::Left => "",
            TabAlign::Center => "\\tqc",
            TabAlign::Right => "\\tqr",
            TabAlign::Decimal => "\\tqdec",
        };
        let _ = write!(out, "{kind}\\tx{}", twips(tab.x));
    }
    match p.align {
        Align::Left => {}
        Align::Center => out.push_str("\\qc"),
        Align::Right => out.push_str("\\qr"),
        Align::Justify => out.push_str("\\qj"),
    }
    out.push_str(&char_look(p.kind));
    if let Some(m) = marker {
        let _ = write!(out, "{{\\listtext\\pard\\plain{} {}\\tab}}", char_look(Kind::Body), text(&m));
    }
    let mut i = 0;
    while i < p.spans.len() {
        match &p.spans[i].link {
            Some(url) => {
                // (A quotation mark would end the field's argument.)
                let _ = write!(out, "{{\\field{{\\*\\fldinst{{HYPERLINK \"{}\"}}}}{{\\fldrslt", text(&url.replace('"', "%22")));
                while i < p.spans.len() && p.spans[i].link.as_ref() == Some(url) {
                    run(out, &p.spans[i], true, t);
                    i += 1;
                }
                out.push_str("}}");
            }
            None => {
                run(out, &p.spans[i], false, t);
                i += 1;
            }
        }
    }
    out.push_str(if cell == Some(true) { "\\cell\n" } else { "\\par\n" });
}

fn run(out: &mut String, s: &Span, link: bool, t: &mut Tables) {
    let mut words = String::new();
    if let Some(f) = &s.font {
        let _ = write!(words, "\\f{}", t.font(f));
    } else if s.monospace {
        words.push_str("\\f1");
    }
    if s.bold {
        words.push_str("\\b");
    }
    if s.italic {
        words.push_str("\\i");
    }
    if s.underline || link {
        words.push_str("\\ul");
    }
    if s.strike {
        words.push_str("\\strike");
    }
    if let Some(size) = s.size {
        let _ = write!(words, "\\fs{}", (size * 2.0).round() as i32);
    }
    if let Some(c) = s.color.or(link.then_some(LINK_BLUE)) {
        let _ = write!(words, "\\cf{}", t.color(c));
    }
    out.push('{');
    if !words.is_empty() {
        out.push_str(&words);
        out.push(' ');
    }
    out.push_str(&text(&s.text));
    out.push('}');
}

// ---- reading ----

/// What a group's text is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Dest {
    Text,
    /// Left out: pictures, headers, notes, `\*` destinations not known.
    Skip,
    Fonts,
    Colors,
    Styles,
    Lists,
    Overrides,
    /// A field's instruction (a link's address).
    FieldInst,
    /// Word 95's list numbering for the paragraph.
    Numbering,
}

/// What holds within a group (each `{` starts with a copy of it).
#[derive(Debug, Clone)]
struct State {
    dest: Dest,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    hidden: bool,
    /// Half-points.
    size: i32,
    font: i32,
    color: usize,
    link: Option<String>,
    /// How many characters stand in for each `\u` (to skip).
    uc: i32,
    style: i32,
    /// Twips.
    left: i32,
    align: Align,
    ls: i32,
    ilvl: i32,
    numbering: Option<List>,
    outline: Option<i32>,
    /// The paragraph is in a table.
    intbl: bool,
    /// Twips: the first line's indent, the right one.
    first: i32,
    right: i32,
    /// Tab stops so far, and how the next lines text up.
    tabs: Vec<Tab>,
    tab_align: TabAlign,
}

impl State {
    fn new() -> State {
        State {
            dest: Dest::Text,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            hidden: false,
            size: 24,
            font: 0,
            color: 0,
            link: None,
            uc: 1,
            style: 0,
            left: 0,
            align: Align::Left,
            ls: 0,
            ilvl: 0,
            numbering: None,
            outline: None,
            intbl: false,
            first: 0,
            right: 0,
            tabs: Vec::new(),
            tab_align: TabAlign::Left,
        }
    }

    fn plain(&mut self, deff: i32) {
        (self.bold, self.italic, self.underline, self.strike, self.hidden) = (false, false, false, false, false);
        (self.size, self.font, self.color) = (24, deff, 0);
    }

    fn pard(&mut self) {
        (self.style, self.left, self.align, self.ls, self.ilvl, self.numbering, self.outline, self.intbl) = (0, 0, Align::Left, 0, 0, None, None, false);
        (self.first, self.right, self.tab_align) = (0, 0, TabAlign::Left);
        self.tabs.clear();
    }
}

/// The encoding of a Windows code page (or a Mac one, 10000).
fn encoding(cp: u16) -> &'static encoding_rs::Encoding {
    let label = match cp {
        932 => "shift_jis".to_owned(),
        936 => "gbk".into(),
        949 => "euc-kr".into(),
        950 => "big5".into(),
        10000 => "macintosh".into(),
        437 | 850 => "ibm866".into(),
        cp => format!("windows-{cp}"),
    };
    encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252)
}

/// The code page a font's `\fcharset` means (None: the document's).
fn charset_page(charset: i32) -> Option<u16> {
    Some(match charset {
        77 => 10000,
        128 => 932,
        129 => 949,
        134 => 936,
        136 => 950,
        161 => 1253,
        162 => 1254,
        163 => 1258,
        177 => 1255,
        178 => 1256,
        186 => 1257,
        204 => 1251,
        222 => 874,
        238 => 1250,
        _ => return None,
    })
}

struct Reader {
    st: State,
    stack: Vec<State>,
    deff: i32,
    page: u16,
    /// Fonts by number: name and code page (if its own).
    fonts: HashMap<i32, (String, Option<u16>)>,
    font_now: (i32, String, Option<u16>),
    colors: Vec<Option<[u8; 3]>>,
    color_now: [Option<u8>; 3],
    /// Paragraph styles' names by number.
    styles: HashMap<i32, String>,
    style_now: (i32, String, bool),
    /// Lists by id: each level's number format.
    lists: HashMap<i32, Vec<i32>>,
    list_now: Option<(i32, Vec<i32>)>,
    /// `\ls` numbers to list ids.
    overrides: HashMap<i32, i32>,
    override_id: i32,
    numbering_now: Option<List>,
    field: String,
    /// Bytes in the code page, waiting to be decoded together.
    bytes: Vec<u8>,
    /// Characters still to skip after a `\u`.
    skip: i32,
    high: Option<u16>,
    ignorable: bool,
    spans: Vec<Span>,
    paragraphs: Vec<Paragraph>,
    /// The table being read, its row, and the row's cell.
    table: Option<Table>,
    row: Vec<Vec<Paragraph>>,
    cell: Vec<Paragraph>,
    /// The row being defined repeats at the top of each page.
    row_header: bool,
    /// Its cells' right edges (twips).
    row_edges: Vec<i32>,
}

pub fn read(bytes: &[u8]) -> Result<Document, String> {
    if !bytes.starts_with(b"{\\rtf") {
        return Err("This isn't an RTF document.".into());
    }
    let mut r = Reader {
        st: State::new(),
        stack: Vec::new(),
        deff: 0,
        page: 1252,
        fonts: HashMap::new(),
        font_now: (0, String::new(), None),
        colors: Vec::new(),
        color_now: [None; 3],
        styles: HashMap::new(),
        style_now: (0, String::new(), true),
        lists: HashMap::new(),
        list_now: None,
        overrides: HashMap::new(),
        override_id: 0,
        numbering_now: None,
        field: String::new(),
        bytes: Vec::new(),
        skip: 0,
        high: None,
        ignorable: false,
        spans: Vec::new(),
        paragraphs: Vec::new(),
        table: None,
        row: Vec::new(),
        cell: Vec::new(),
        row_header: false,
        row_edges: Vec::new(),
    };
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        match b {
            b'{' => {
                r.flush();
                r.skip = 0;
                r.stack.push(r.st.clone());
            }
            b'}' => {
                r.flush();
                r.skip = 0;
                let Some(outer) = r.stack.pop() else { break };
                let inner = std::mem::replace(&mut r.st, outer);
                if inner.dest != r.st.dest {
                    r.ended(inner.dest);
                }
                if r.stack.is_empty() {
                    break;
                }
            }
            b'\\' => {
                let Some(&c) = bytes.get(i) else { break };
                if c.is_ascii_alphabetic() {
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                    let num_start = i;
                    if i < bytes.len() && bytes[i] == b'-' {
                        i += 1;
                    }
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    let param = std::str::from_utf8(&bytes[num_start..i]).ok().and_then(|s| s.parse::<i64>().ok()).map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
                    if i < bytes.len() && bytes[i] == b' ' {
                        i += 1;
                    }
                    if word == "bin" {
                        // Binary data: its bytes, unread.
                        r.flush();
                        i += param.unwrap_or(0).max(0) as usize;
                        continue;
                    }
                    if r.skip > 0 {
                        r.skip -= 1;
                        continue;
                    }
                    r.flush();
                    r.word(word, param);
                } else if c == b'\'' {
                    let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                    i += 3;
                    if r.skip > 0 {
                        r.skip -= 1;
                        continue;
                    }
                    if let Some(v) = hex {
                        r.bytes.push(v);
                    }
                } else {
                    i += 1;
                    if r.skip > 0 {
                        r.skip -= 1;
                        continue;
                    }
                    r.flush();
                    match c {
                        b'\\' | b'{' | b'}' => r.char(c as char),
                        b'~' => r.char('\u{a0}'),
                        b'_' => r.char('\u{2011}'),
                        b'*' => r.ignorable = true,
                        b'\r' | b'\n' => r.word("par", None),
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' => {}
            _ => {
                if r.skip > 0 {
                    r.skip -= 1;
                } else if b < 0x80 {
                    r.flush();
                    r.char(b as char);
                } else {
                    r.bytes.push(b);
                }
            }
        }
    }
    r.flush();
    if !r.spans.is_empty() {
        r.end_paragraph();
    }
    if !r.cell.is_empty() || !r.row.is_empty() {
        r.end_row();
    }
    r.end_table();
    let font = r.fonts.get(&r.deff).map(|(n, _)| n.clone()).filter(|n| !n.is_empty());
    let mut doc = Document { paragraphs: r.paragraphs, font };
    doc.tidy();
    Ok(doc)
}

impl Reader {
    /// Decode the code page's bytes waiting (in the font's code page, if
    /// it has one).
    fn flush(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let page = self.fonts.get(&self.st.font).and_then(|(_, p)| *p).unwrap_or(self.page);
        let bytes = std::mem::take(&mut self.bytes);
        let (s, _, _) = encoding(page).decode(&bytes);
        for c in s.chars() {
            self.char(c);
        }
    }

    /// A character of the current destination.
    fn char(&mut self, c: char) {
        match self.st.dest {
            Dest::Text => self.text(c),
            Dest::Fonts => {
                if c == ';' {
                    let (n, name, page) = std::mem::take(&mut self.font_now);
                    self.fonts.insert(n, (name.trim().to_owned(), page));
                } else {
                    self.font_now.1.push(c);
                }
            }
            Dest::Colors => {
                if c == ';' {
                    let [r, g, b] = std::mem::take(&mut self.color_now);
                    let any = r.is_some() || g.is_some() || b.is_some();
                    self.colors.push(any.then(|| [r.unwrap_or(0), g.unwrap_or(0), b.unwrap_or(0)]));
                }
            }
            Dest::Styles => {
                if c == ';' {
                    let (n, name, para) = std::mem::replace(&mut self.style_now, (0, String::new(), true));
                    if para {
                        self.styles.insert(n, name.trim().to_owned());
                    }
                } else {
                    self.style_now.1.push(c);
                }
            }
            Dest::FieldInst => self.field.push(c),
            _ => {}
        }
    }

    /// A character of the document's text, in the style that holds.
    fn text(&mut self, c: char) {
        if self.st.hidden {
            return;
        }
        let st = &self.st;
        let (font, monospace) = match self.fonts.get(&st.font) {
            Some((name, _)) if st.font != self.deff => {
                if is_mono(name) {
                    (None, true)
                } else {
                    (Some(name.as_str()), false)
                }
            }
            _ => (None, false),
        };
        let color = self.colors.get(st.color).copied().flatten();
        let size = Some(st.size as f32 / 2.0);
        if let Some(last) = self.spans.last_mut()
            && (last.bold, last.italic, last.underline, last.strike, last.size, last.color, last.font.as_deref(), last.monospace, last.link.as_ref())
                == (st.bold, st.italic, st.underline, st.strike, size, color, font, monospace, st.link.as_ref())
        {
            last.text.push(c);
            return;
        }
        self.spans.push(Span {
            text: c.to_string(),
            bold: st.bold,
            italic: st.italic,
            underline: st.underline,
            strike: st.strike,
            size,
            color,
            font: font.map(str::to_owned),
            monospace,
            link: st.link.clone(),
        });
    }

    /// A destination's group closed.
    fn ended(&mut self, dest: Dest) {
        match dest {
            Dest::Lists => {
                if let Some((id, levels)) = self.list_now.take() {
                    self.lists.insert(id, levels);
                }
            }
            Dest::FieldInst => {
                // HYPERLINK "address" (switches aside; `\l` is a place in
                // the document, not a link out).
                let inst = std::mem::take(&mut self.field);
                let mut words = inst.split_whitespace();
                if words.next().is_some_and(|w| w.eq_ignore_ascii_case("HYPERLINK")) && !inst.contains("\\l ") {
                    let url = match (inst.find('"'), inst.rfind('"')) {
                        (Some(a), Some(b)) if b > a => inst[a + 1..b].to_owned(),
                        _ => words.find(|w| !w.starts_with('\\')).unwrap_or("").to_owned(),
                    };
                    if !url.is_empty() {
                        self.st.link = Some(url);
                    }
                }
            }
            Dest::Numbering => self.st.numbering = self.numbering_now.take(),
            _ => {}
        }
    }

    fn word(&mut self, word: &str, param: Option<i32>) {
        let ignorable = std::mem::take(&mut self.ignorable);
        let n = param.unwrap_or(0);
        let on = param != Some(0);
        let dest = self.st.dest;
        // Destinations, known and not.
        match word {
            "fonttbl" => return self.st.dest = Dest::Fonts,
            "colortbl" => return self.st.dest = Dest::Colors,
            "stylesheet" => return self.st.dest = Dest::Styles,
            "listtable" => return self.st.dest = Dest::Lists,
            "listoverridetable" => return self.st.dest = Dest::Overrides,
            "fldinst" => {
                self.field.clear();
                return self.st.dest = Dest::FieldInst;
            }
            "fldrslt" => return self.st.dest = if dest == Dest::Skip { Dest::Skip } else { Dest::Text },
            "pn" => {
                self.numbering_now = Some(List { numbered: true, level: 0 });
                return self.st.dest = Dest::Numbering;
            }
            // The Unicode text of `\upr`'s pair (the other is skipped).
            "ud" => return self.st.dest = Dest::Text,
            "upr" => return self.st.dest = Dest::Skip,
            "info" | "pict" | "header" | "headerl" | "headerr" | "headerf" | "footer" | "footerl" | "footerr" | "footerf" | "footnote"
            | "object" | "xe" | "tc" | "txe" | "rxe" | "annotation" | "atnid" | "atnauthor" | "listtext" | "pntext" | "nonshppict"
            | "shp" | "template" | "revtbl" | "filetbl" | "comment" | "doccomm" => return self.st.dest = Dest::Skip,
            _ if ignorable && !matches!(dest, Dest::Lists | Dest::Overrides) => return self.st.dest = Dest::Skip,
            _ => {}
        }
        match dest {
            Dest::Skip | Dest::FieldInst => {}
            Dest::Fonts => match word {
                "f" => self.font_now.0 = n,
                "fcharset" => self.font_now.2 = charset_page(n),
                "cpg" => self.font_now.2 = u16::try_from(n).ok().filter(|p| *p > 0),
                _ => {}
            },
            Dest::Colors => match word {
                "red" => self.color_now[0] = Some(n.clamp(0, 255) as u8),
                "green" => self.color_now[1] = Some(n.clamp(0, 255) as u8),
                "blue" => self.color_now[2] = Some(n.clamp(0, 255) as u8),
                _ => {}
            },
            Dest::Styles => match word {
                "s" => self.style_now = (n, String::new(), true),
                "cs" | "ds" | "ts" => self.style_now = (n, String::new(), false),
                _ => {}
            },
            Dest::Lists => match word {
                "list" => {
                    if let Some((id, levels)) = self.list_now.replace((0, Vec::new())) {
                        self.lists.insert(id, levels);
                    }
                }
                "listlevel" => {
                    if let Some((_, levels)) = &mut self.list_now {
                        levels.push(0);
                    }
                }
                "levelnfc" | "levelnfcn" => {
                    if let Some(l) = self.list_now.as_mut().and_then(|(_, levels)| levels.last_mut()) {
                        *l = n;
                    }
                }
                "listid" => {
                    if let Some((id, _)) = &mut self.list_now {
                        *id = n;
                    }
                }
                _ => {}
            },
            Dest::Overrides => match word {
                "listid" => self.override_id = n,
                "ls" => {
                    self.overrides.insert(n, self.override_id);
                }
                _ => {}
            },
            Dest::Numbering => {
                let now = self.numbering_now.get_or_insert(List { numbered: true, level: 0 });
                match word {
                    "pnlvlblt" => now.numbered = false,
                    "pnlvlbody" | "pnlvlcont" => now.level = 0,
                    "pnlvl" => now.level = (n - 1).clamp(0, 8) as u8,
                    _ => {}
                }
            }
            Dest::Text => self.text_word(word, n, on, param),
        }
    }

    fn text_word(&mut self, word: &str, n: i32, on: bool, param: Option<i32>) {
        let st = &mut self.st;
        match word {
            // The document.
            "ansicpg" => self.page = u16::try_from(n).ok().filter(|p| *p > 0).unwrap_or(1252),
            "mac" => self.page = 10000,
            "pc" => self.page = 437,
            "pca" => self.page = 850,
            "deff" => {
                self.deff = n;
                st.font = n;
            }
            "uc" => st.uc = n.max(0),
            "u" => {
                let unit = (if n < 0 { n + 0x10000 } else { n }) as u16;
                self.skip = st.uc;
                match (self.high.take(), unit) {
                    (_, 0xD800..=0xDBFF) => self.high = Some(unit),
                    (Some(h), 0xDC00..=0xDFFF) => {
                        if let Some(c) = char::decode_utf16([h, unit]).next().and_then(Result::ok) {
                            self.text(c);
                        }
                    }
                    (_, u) => {
                        if let Some(c) = char::from_u32(u as u32) {
                            self.text(c);
                        }
                    }
                }
            }
            // Characters.
            "plain" => st.plain(self.deff),
            "b" => st.bold = on,
            "i" => st.italic = on,
            "ul" | "uld" | "uldb" | "ulw" | "ulth" | "uldash" | "uldashd" | "uldashdd" | "ulwave" | "ulhwave" | "ululdbwave" | "ulthd" | "ulthdash" => st.underline = on,
            "ulnone" => st.underline = false,
            "strike" | "striked" => st.strike = on,
            "v" | "deleted" => st.hidden = on,
            "fs" => st.size = if param.is_some() { n.clamp(2, 3276) } else { 24 },
            "f" => st.font = n,
            "cf" => st.color = n.max(0) as usize,
            // Paragraphs.
            "pard" => st.pard(),
            "s" => st.style = n,
            "ql" => st.align = Align::Left,
            "qc" => st.align = Align::Center,
            "qr" => st.align = Align::Right,
            "qj" | "qd" => st.align = Align::Justify,
            "li" | "lin" => st.left = n,
            "fi" => st.first = n,
            "ri" | "rin" => st.right = n,
            "tqc" => st.tab_align = TabAlign::Center,
            "tqr" => st.tab_align = TabAlign::Right,
            "tqdec" => st.tab_align = TabAlign::Decimal,
            "tx" => {
                let align = std::mem::take(&mut st.tab_align);
                st.tabs.push(Tab { x: n.max(0) as f32 / 20.0, align });
            }
            // (A bar, not a stop.)
            "tb" => st.tab_align = TabAlign::Left,
            "ls" => st.ls = n,
            "ilvl" => st.ilvl = n,
            "outlinelevel" => st.outline = Some(n),
            "intbl" => st.intbl = true,
            "itap" => st.intbl = n > 0,
            "par" => self.end_paragraph(),
            "line" => self.text('\n'),
            // (A table in a cell: its cells a tab apart.)
            "tab" | "nestcell" => self.text('\t'),
            "trowd" => {
                self.row_header = false;
                self.row_edges.clear();
            }
            "cellx" => self.row_edges.push(n),
            "trhdr" => self.row_header = true,
            "cell" => {
                let p = self.take_paragraph();
                self.cell.push(p);
                self.row.push(std::mem::take(&mut self.cell));
            }
            "row" => self.end_row(),
            "emdash" => self.text('\u{2014}'),
            "endash" => self.text('\u{2013}'),
            "bullet" => self.text('\u{2022}'),
            "lquote" => self.text('\u{2018}'),
            "rquote" => self.text('\u{2019}'),
            "ldblquote" => self.text('\u{201C}'),
            "rdblquote" => self.text('\u{201D}'),
            "emspace" => self.text('\u{2003}'),
            "enspace" => self.text('\u{2002}'),
            "qmspace" => self.text('\u{2005}'),
            _ => {}
        }
    }

    /// `\par`: the paragraph ends; in a table, in its cell (a table ends
    /// at the first paragraph after it).
    fn end_paragraph(&mut self) {
        let p = self.take_paragraph();
        if self.st.intbl {
            self.cell.push(p);
        } else {
            self.end_table();
            self.paragraphs.push(p);
        }
    }

    /// `\row`: its cells (and any paragraphs not yet in one) a row of the
    /// table.
    fn end_row(&mut self) {
        if !self.cell.is_empty() {
            self.row.push(std::mem::take(&mut self.cell));
        }
        let row = std::mem::take(&mut self.row);
        let table = self.table.get_or_insert_with(Table::default);
        if table.rows.is_empty() {
            table.header = self.row_header;
            // The columns as wide as the first row's cells.
            let mut last = 0;
            table.widths = self.row_edges.iter().map(|&e| std::mem::replace(&mut last, e).abs_diff(e) as f32).collect();
        }
        table.rows.push(row);
    }

    fn end_table(&mut self) {
        if let Some(t) = self.table.take() {
            self.paragraphs.push(Paragraph { table: Some(t), ..Paragraph::default() });
        }
    }

    /// The paragraph read so far: its kind (by its style's name, or its
    /// outline level), list, indent and alignment, and its text with what
    /// its kind looks like taken off.
    fn take_paragraph(&mut self) -> Paragraph {
        let st = &self.st;
        let name = self.styles.get(&st.style);
        let mut kind = name.map(|n| crate::docx::kind_of(n)).unwrap_or_default();
        if kind == Kind::Body
            && let Some(level) = st.outline.filter(|l| (0..9).contains(l))
        {
            kind = [Kind::Heading1, Kind::Heading2][..].get(level as usize).copied().unwrap_or(Kind::Heading3);
        }
        let list = if st.ls > 0 {
            let level = st.ilvl.clamp(0, 8);
            match self.overrides.get(&st.ls).and_then(|id| self.lists.get(id)) {
                Some(levels) => match levels.get(level as usize).copied().unwrap_or(0) {
                    NFC_NONE => None,
                    nfc => Some(List { numbered: nfc != NFC_BULLET, level: level as u8 }),
                },
                None => Some(List { numbered: false, level: level as u8 }),
            }
        } else {
            st.numbering
        };
        // Outside a list: whole levels, and the rest in points.
        let quote = if kind == Kind::Quote { LEVEL } else { 0 };
        let (indent, left, first, right) = if list.is_some() {
            (0, 0.0, 0.0, 0.0)
        } else {
            let left = (st.left - quote).max(0);
            let levels = (left / LEVEL).min(8);
            (levels as u8, (left - levels * LEVEL) as f32 / 20.0, st.first.max(0) as f32 / 20.0, (st.right - quote).max(0) as f32 / 20.0)
        };
        let mut tabs = st.tabs.clone();
        tabs.sort_by(|a, b| a.x.total_cmp(&b.x));
        let lk = look(kind);
        let mut spans = std::mem::take(&mut self.spans);
        for s in &mut spans {
            if lk.bold {
                s.bold = false;
            }
            if lk.italic {
                s.italic = false;
            }
            if s.size == Some(lk.size) {
                s.size = None;
            }
            if lk.mono {
                s.monospace = true;
            }
            if s.link.is_some() {
                s.underline = false;
                if matches!(s.color, Some(LINK_BLUE) | Some([0, 0, 0xff])) {
                    s.color = None;
                }
            }
        }
        Paragraph { kind, list, indent, align: st.align, spans, table: None, left, first, right, tabs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let doc = crate::tests::sample();
        let back = read(&write(&doc).unwrap()).unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn writes_ascii_only() {
        let doc = Document { paragraphs: vec![Paragraph { spans: vec![Span::default().with_text("naïve 😀 {x}\\")], ..Default::default() }], font: None };
        let out = write(&doc).unwrap();
        assert!(out.is_ascii());
        let back = read(&out).unwrap();
        assert_eq!(back.paragraphs[0].spans[0].text, "naïve 😀 {x}\\");
    }

    /// As Word writes it: code pages, a style sheet, a list table, a
    /// link, a table, a picture and a header to leave out.
    #[test]
    fn reads_what_word_writes() {
        let rtf = br#"{\rtf1\ansi\ansicpg1252\deff0\deflang1033{\fonttbl{\f0\froman\fcharset0 Times New Roman;}{\f1\fswiss\fcharset204{\*\falt Arial} Arial Cyr;}{\f2\fmodern\fcharset0 Consolas;}}
{\colortbl;\red0\green0\blue255;\red255\green0\blue0;}
{\stylesheet{\ql \li0\widctlpar\f0\fs24 \snext0 Normal;}{\s1\ql\outlinelevel0\b\fs48 \sbasedon0 \snext0 heading 1;}{\*\cs10 \additive Default Paragraph Font;}}
{\*\listtable{\list\listtemplateid1{\listlevel\levelnfc23\levelnfcn23{\leveltext\'01\u-3913 ?;}{\levelnumbers;}\fi-360\li720}{\listlevel\levelnfc0{\leveltext\'03\'01.;}{\levelnumbers\'01;}}\listid100}
{\list\listtemplateid2{\listlevel\levelnfc0\levelnfcn0{\leveltext\'02\'00.;}{\levelnumbers\'01;}}\listid200}}
{\*\listoverridetable{\listoverride\listid100\listoverridecount0\ls1}{\listoverride\listid200\listoverridecount0\ls2}}
{\info{\title Secret}{\author Someone}}
{\header \pard\plain Page header\par}
\pard\plain\s1\outlinelevel0\b\fs48 Caf\'e9 na\u239?ve\par
\pard\plain\fs24 Plain {\b bold} and {\cf2 red} {\f1 \'cf\'f0\'e8\'e2\'e5\'f2} {\f2 code}\par
{\listtext\'b7\tab}\pard\ls1\ilvl0\fi-360\li720 first\par
{\listtext 1.\tab}\pard\ls2\ilvl0\fi-360\li720 one\par
\pard\qc See {\field{\*\fldinst{HYPERLINK "https://example.com/"}}{\fldrslt{\ul\cf1 the site}}}.\par
{\*\shppict{\pict\pngblip\picw10\pich10 89504e470d0a}}{\nonshppict{\pict\wmetafile8 0100}}
\trowd\cellx1000\cellx2000\pard\intbl A1\cell B1\cell\row
\pard\li1440 \ldblquote quoted\rdblquote  \emdash  done\par
}"#;
        let doc = read(rtf).unwrap();
        assert_eq!(texts(&doc), ["Café naïve", "Plain bold and red Привет code", "first", "one", "See the site.", "A1\tB1", "\u{201C}quoted\u{201D} \u{2014} done"]);
        assert!(doc.paragraphs[5].table.is_some());
        assert_eq!(doc.font.as_deref(), Some("Times New Roman"));
        let p = &doc.paragraphs;
        assert_eq!(p[0].kind, Kind::Heading1);
        assert!(!p[0].spans[0].bold && p[0].spans[0].size == Some(24.0), "{:?}", p[0].spans[0]);
        assert!(p[1].spans.iter().any(|s| s.bold && s.text == "bold"));
        assert!(p[1].spans.iter().any(|s| s.color == Some([255, 0, 0]) && s.text == "red"));
        assert!(p[1].spans.iter().any(|s| s.font.as_deref() == Some("Arial Cyr")));
        assert!(p[1].spans.iter().any(|s| s.monospace && s.text == "code"));
        assert_eq!(p[2].list, Some(List { numbered: false, level: 0 }));
        assert_eq!(p[3].list, Some(List { numbered: true, level: 0 }));
        assert_eq!(p[4].align, Align::Center);
        let link = p[4].spans.iter().find(|s| s.text == "the site").unwrap();
        assert_eq!(link.link.as_deref(), Some("https://example.com/"));
        assert!(!link.underline && link.color.is_none());
        assert_eq!(p[6].indent, 2);
    }

    /// Each paragraph's text; a table's rows a line each, cells a tab
    /// apart.
    fn texts(doc: &Document) -> Vec<String> {
        doc.paragraphs
            .iter()
            .map(|p| match &p.table {
                Some(t) => t.rows.iter().map(|r| r.iter().map(|c| Table::cell_text(c)).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n"),
                None => p.spans.iter().map(|s| s.text.as_str()).collect(),
            })
            .collect()
    }

    #[test]
    fn reads_tables() {
        let rtf = br"{\rtf1\ansi\deff0{\fonttbl{\f0 Arial;}}
Before\par
\trowd\trhdr\cellx2000\cellx4000\pard\intbl {\b Name}\cell {\b Size}\cell\row
\trowd\cellx2000\cellx4000\pard\intbl one\par two\cell\pard\intbl\qr 12\cell\row
\pard After\par}";
        let doc = read(rtf).unwrap();
        assert_eq!(texts(&doc), ["Before", "Name\tSize\none two\t12", "After"]);
        let t = doc.paragraphs[1].table.as_ref().unwrap();
        assert!(t.header);
        assert_eq!(t.rows[1][0].len(), 2);
        assert_eq!(t.rows[1][1][0].align, Align::Right);
        assert!(t.rows[0][0][0].spans[0].bold);
    }

    #[test]
    fn reads_word_95_lists_and_surrogates() {
        let rtf = br"{\rtf1\ansi\deff0{\fonttbl{\f0 Arial;}}
{\pntext\f0\'b7\tab}{\*\pn\pnlvlblt\pnf0\pnindent0{\pntxtb\'b7}}\fi-720\li720 dot\par
\pard{\pntext 1.\tab}{\*\pn\pnlvlbody\pndec}\fi-720\li720 num\par
\pard\uc2 smile \u-10179\'3f\'3f\u-8704\'3f\'3f!\par}";
        let doc = read(rtf).unwrap();
        assert_eq!(doc.paragraphs[0].list, Some(List { numbered: false, level: 0 }));
        assert_eq!(doc.paragraphs[1].list, Some(List { numbered: true, level: 0 }));
        assert_eq!(doc.paragraphs[2].spans[0].text, "smile \u{1F600}!");
        assert_eq!(doc.paragraphs[2].list, None);
    }

    #[test]
    fn not_rtf() {
        assert!(read(b"hello").is_err());
    }
}
