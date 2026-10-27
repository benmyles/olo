//! The PDF model behind Viewer's markup tools: interactive form fields,
//! the annotations Viewer adds (text boxes, ink and signatures), and
//! writing them back, either as an incremental update appended to the
//! original file (so earlier revisions and digital signatures survive) or
//! as a flattened copy with every annotation burnt into the page.
//!
//! Everything here is plain data plus pure functions over lopdf documents,
//! so it runs on the document's worker thread and is unit tested without
//! a display.

use std::collections::{BTreeMap, HashMap, HashSet};

use lopdf::{Dictionary, Document, IncrementalDocument, Object, ObjectId, Stream, StringFormat};

/// A rectangle in PDF user space: [x0, y0, x1, y1] with x0 < x1, y0 < y1.
pub type PdfRect = [f32; 4];

/// Annotations Viewer wrote carry this prefix in /NM, so they can be
/// edited again when the file is reopened (`tabula-` before the rename).
const OUR_NM: &str = "olo-";
const OLD_NM: &str = "tabula-";

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text { multiline: bool, password: bool, comb: bool, max_len: Option<usize> },
    Check,
    Radio,
    Choice { options: Vec<(String, String)>, combo: bool, editable: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    /// A button's state: the on-state name, or None for Off.
    State(Option<String>),
}

/// One place a field appears (a widget annotation).
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    pub id: ObjectId,
    pub page: usize,
    pub rect: PdfRect,
    /// For check boxes and radio buttons: this widget's on-state name.
    pub on: Option<String>,
    pub border: Option<[f32; 3]>,
    pub background: Option<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The terminal field dictionary (holds /V).
    pub id: ObjectId,
    pub name: String,
    /// The tool tip (/TU), shown as a hint.
    pub tip: Option<String>,
    pub kind: FieldKind,
    pub value: Value,
    pub read_only: bool,
    /// From /DA; 0 means "fit".
    pub font_size: f32,
    pub color: [f32; 3],
    /// /Q: 0 left, 1 centre, 2 right.
    pub align: u8,
    pub widgets: Vec<Widget>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarkKind {
    Text { text: String, size: f32 },
    /// Strokes with points normalised to the mark's rect (0..1, y up).
    Ink { strokes: Vec<Vec<[f32; 2]>>, width: f32, signature: bool },
}

/// An annotation Viewer manages: added in this session, or written by
/// Viewer earlier.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// The annotation object, once it is in the file.
    pub id: Option<ObjectId>,
    pub page: usize,
    pub rect: PdfRect,
    pub color: [f32; 3],
    pub kind: MarkKind,
}

/// Size and placement of one page, for drawing over its rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeom {
    /// Rendered size in points (after crop box and rotation).
    pub size: (f32, f32),
    /// User space → rendered points, y down: [a b c d e f].
    pub transform: [f64; 6],
}

/// What the editor shows: fields and marks, plus the annotations it draws
/// itself (to hide from the page rendering).
#[derive(Debug, Clone, Default)]
pub struct Model {
    pub fields: Vec<Field>,
    pub marks: Vec<Mark>,
}

impl Model {
    /// Annotation objects Viewer draws itself (field widgets and marks);
    /// the page rendering must leave these out.
    pub fn drawn_by_us(&self) -> HashSet<ObjectId> {
        let mut s: HashSet<ObjectId> = self.fields.iter().flat_map(|f| f.widgets.iter().map(|w| w.id)).collect();
        s.extend(self.marks.iter().filter_map(|m| m.id));
        s
    }
}

// ---- reading ------------------------------------------------------------

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

fn get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

fn get_dict<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Dictionary> {
    get(doc, d, key)?.as_dict().ok()
}

fn get_f32(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<f32> {
    get(doc, d, key)?.as_float().ok()
}

fn get_name(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<String> {
    get(doc, d, key)?.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned())
}

fn get_text(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<String> {
    match get(doc, d, key)? {
        Object::String(b, _) => Some(decode_text(b)),
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

fn get_rect(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<PdfRect> {
    let a = get(doc, d, key)?.as_array().ok()?;
    if a.len() != 4 {
        return None;
    }
    let v: Vec<f32> = a.iter().filter_map(|o| deref(doc, o).as_float().ok()).collect();
    (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

fn get_color(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<[f32; 3]> {
    let a = get(doc, d, key)?.as_array().ok()?;
    let v: Vec<f32> = a.iter().filter_map(|o| deref(doc, o).as_float().ok()).collect();
    match v.len() {
        1 => Some([v[0]; 3]),
        3 => Some([v[0], v[1], v[2]]),
        4 => Some([(1.0 - v[0]) * (1.0 - v[3]), (1.0 - v[1]) * (1.0 - v[3]), (1.0 - v[2]) * (1.0 - v[3])]),
        _ => None,
    }
}

/// A PDF text string: UTF-16BE with a byte-order mark, UTF-8 with one
/// (PDF 2.0), or PDFDocEncoding (Latin-1 for our purposes).
pub fn decode_text(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = b[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&b[3..]).into_owned()
    } else {
        b.iter().map(|&c| pdfdoc_char(c)).collect()
    }
}

fn pdfdoc_char(c: u8) -> char {
    match c {
        0x80 => '•',
        0x81 => '†',
        0x82 => '‡',
        0x83 => '…',
        0x84 => '—',
        0x85 => '–',
        0x8D => '“',
        0x8E => '”',
        0x8F => '‘',
        0x90 => '’',
        0x92 => '™',
        0xA0 => '€',
        _ => c as char,
    }
}

/// A text string object: plain when ASCII, UTF-16BE otherwise.
pub fn text_object(s: &str) -> Object {
    if s.is_ascii() {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, StringFormat::Hexadecimal)
    }
}

/// The page index of every annotation, from the pages' /Annots arrays.
fn annotation_pages(doc: &Document, pages: &[ObjectId]) -> HashMap<ObjectId, usize> {
    let mut out = HashMap::new();
    for (i, &pid) in pages.iter().enumerate() {
        let Ok(page) = doc.get_dictionary(pid) else { continue };
        if let Some(a) = get(doc, page, b"Annots").and_then(|o| o.as_array().ok()) {
            for o in a {
                if let Ok(id) = o.as_reference() {
                    out.insert(id, i);
                }
            }
        }
    }
    out
}

/// Inheritable field attributes, collected walking down the field tree.
#[derive(Clone, Default)]
struct Inherited {
    name: String,
    ft: Option<String>,
    ff: i64,
    v: Option<Object>,
    da: Option<String>,
    q: Option<i64>,
}

/// Parse /DA ("/Helv 12 Tf 0 g") into a font size and colour.
fn parse_da(da: &str) -> (f32, [f32; 3]) {
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut size = 0.0;
    let mut color = [0.0; 3];
    for (i, t) in toks.iter().enumerate() {
        let num = |k: usize| toks.get(i.wrapping_sub(k)).and_then(|s| s.parse::<f32>().ok());
        match *t {
            "Tf" => size = num(1).unwrap_or(0.0),
            "g" => color = [num(1).unwrap_or(0.0); 3],
            "rg" => color = [num(3).unwrap_or(0.0), num(2).unwrap_or(0.0), num(1).unwrap_or(0.0)],
            "k" => {
                let (cc, m, y, k) = (num(4).unwrap_or(0.0), num(3).unwrap_or(0.0), num(2).unwrap_or(0.0), num(1).unwrap_or(0.0));
                color = [(1.0 - cc) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
            }
            _ => {}
        }
    }
    (size, color)
}

/// The on-state name of a button widget: the key of /AP /N that isn't Off.
fn on_state(doc: &Document, w: &Dictionary) -> Option<String> {
    let n = get_dict(doc, w, b"AP").and_then(|ap| get_dict(doc, ap, b"N"))?;
    n.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).find(|k| k != "Off")
}

/// Read the document's form fields and the marks Viewer wrote earlier.
pub fn read_model(doc: &Document) -> Model {
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let annot_page = annotation_pages(doc, &pages);
    let mut model = Model::default();
    let form = doc.catalog().ok().and_then(|c| get_dict(doc, c, b"AcroForm"));
    if let Some(form) = form {
        let default_da = get_text(doc, form, b"DA");
        let top = Inherited { da: default_da, q: get(doc, form, b"Q").and_then(|o| o.as_i64().ok()), ..Default::default() };
        if let Some(fields) = get(doc, form, b"Fields").and_then(|o| o.as_array().ok()) {
            let mut seen = HashSet::new();
            for f in fields {
                if let Ok(id) = f.as_reference() {
                    walk_field(doc, id, &top, &annot_page, &pages, &mut seen, &mut model.fields, 0);
                }
            }
        }
    }
    for (i, &pid) in pages.iter().enumerate() {
        let Ok(page) = doc.get_dictionary(pid) else { continue };
        let Some(a) = get(doc, page, b"Annots").and_then(|o| o.as_array().ok()) else { continue };
        for o in a {
            let Ok(id) = o.as_reference() else { continue };
            if let Some(m) = doc.get_dictionary(id).ok().and_then(|d| read_mark(doc, d, i)) {
                model.marks.push(Mark { id: Some(id), ..m });
            }
        }
    }
    model
}

#[allow(clippy::too_many_arguments)]
fn walk_field(
    doc: &Document,
    id: ObjectId,
    parent: &Inherited,
    annot_page: &HashMap<ObjectId, usize>,
    pages: &[ObjectId],
    seen: &mut HashSet<ObjectId>,
    out: &mut Vec<Field>,
    depth: usize,
) {
    if depth > 32 || !seen.insert(id) {
        return;
    }
    let Ok(d) = doc.get_dictionary(id) else { return };
    let mut inh = parent.clone();
    if let Some(t) = get_text(doc, d, b"T") {
        inh.name = if inh.name.is_empty() { t } else { format!("{}.{t}", inh.name) };
    }
    if let Some(ft) = get_name(doc, d, b"FT") {
        inh.ft = Some(ft);
    }
    if let Some(ff) = get(doc, d, b"Ff").and_then(|o| o.as_i64().ok()) {
        inh.ff = ff;
    }
    if let Some(v) = get(doc, d, b"V") {
        inh.v = Some(v.clone());
    }
    if let Some(da) = get_text(doc, d, b"DA") {
        inh.da = Some(da);
    }
    if let Some(q) = get(doc, d, b"Q").and_then(|o| o.as_i64().ok()) {
        inh.q = Some(q);
    }
    // Kids with /T are child fields; kids without are this field's widgets.
    let kids: Vec<ObjectId> = get(doc, d, b"Kids")
        .and_then(|o| o.as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    let child_fields: Vec<ObjectId> =
        kids.iter().copied().filter(|k| doc.get_dictionary(*k).is_ok_and(|kd| kd.has(b"T"))).collect();
    if !child_fields.is_empty() {
        for k in child_fields {
            walk_field(doc, k, &inh, annot_page, pages, seen, out, depth + 1);
        }
        return;
    }
    let mut widget_ids: Vec<ObjectId> = kids;
    if d.has(b"Rect") || widget_ids.is_empty() {
        widget_ids.insert(0, id);
    }
    let ff = inh.ff;
    let kind = match inh.ft.as_deref() {
        Some("Tx") => FieldKind::Text {
            multiline: ff & (1 << 12) != 0,
            password: ff & (1 << 13) != 0,
            comb: ff & (1 << 24) != 0,
            max_len: get(doc, d, b"MaxLen").and_then(|o| o.as_i64().ok()).map(|n| n.max(0) as usize),
        },
        Some("Btn") if ff & (1 << 16) != 0 => return, // push button
        Some("Btn") if ff & (1 << 15) != 0 => FieldKind::Radio,
        Some("Btn") => FieldKind::Check,
        Some("Ch") => {
            let options = get(doc, d, b"Opt")
                .and_then(|o| o.as_array().ok())
                .map(|a| {
                    a.iter()
                        .filter_map(|o| match deref(doc, o) {
                            Object::Array(p) if p.len() == 2 => {
                                let e = deref(doc, &p[0]).as_str().ok().map(decode_text)?;
                                let s = deref(doc, &p[1]).as_str().ok().map(decode_text)?;
                                Some((e, s))
                            }
                            Object::String(b, _) => Some((decode_text(b), decode_text(b))),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            FieldKind::Choice { options, combo: ff & (1 << 17) != 0, editable: ff & (1 << 18) != 0 }
        }
        _ => return, // signature fields and unknown types are left alone
    };
    let value = match (&kind, &inh.v) {
        (FieldKind::Check | FieldKind::Radio, Some(Object::Name(n))) if n != b"Off" => {
            Value::State(Some(String::from_utf8_lossy(n).into_owned()))
        }
        (FieldKind::Check | FieldKind::Radio, _) => Value::State(None),
        (_, Some(Object::String(b, _))) => Value::Text(decode_text(b)),
        (_, Some(Object::Array(a))) => {
            Value::Text(a.first().and_then(|o| deref(doc, o).as_str().ok()).map(decode_text).unwrap_or_default())
        }
        (_, Some(Object::Name(n))) => Value::Text(String::from_utf8_lossy(n).into_owned()),
        _ => Value::Text(String::new()),
    };
    let (font_size, color) = parse_da(inh.da.as_deref().unwrap_or("/Helv 0 Tf 0 g"));
    let mut widgets = Vec::new();
    for wid in widget_ids {
        let Ok(w) = doc.get_dictionary(wid) else { continue };
        let Some(rect) = get_rect(doc, w, b"Rect") else { continue };
        let page = annot_page.get(&wid).copied().or_else(|| {
            let p = get(doc, w, b"P").and(w.get(b"P").ok()).and_then(|o| o.as_reference().ok())?;
            pages.iter().position(|&x| x == p)
        });
        let Some(page) = page else { continue };
        // Hidden widgets stay hidden.
        let flags = get(doc, w, b"F").and_then(|o| o.as_i64().ok()).unwrap_or(0);
        if flags & 0b10 != 0 {
            continue;
        }
        let mk = get_dict(doc, w, b"MK");
        widgets.push(Widget {
            id: wid,
            page,
            rect,
            on: matches!(kind, FieldKind::Check | FieldKind::Radio).then(|| on_state(doc, w)).flatten(),
            border: mk.and_then(|mk| get_color(doc, mk, b"BC")),
            background: mk.and_then(|mk| get_color(doc, mk, b"BG")),
        });
    }
    if widgets.is_empty() {
        return;
    }
    // Check boxes with no appearance yet get the conventional "Yes".
    if matches!(kind, FieldKind::Check) {
        for w in &mut widgets {
            w.on.get_or_insert_with(|| "Yes".into());
        }
    }
    out.push(Field {
        id,
        name: inh.name.clone(),
        tip: get_text(doc, d, b"TU"),
        kind,
        value,
        read_only: ff & 1 != 0,
        font_size,
        color,
        align: inh.q.unwrap_or(0).clamp(0, 2) as u8,
        widgets,
    });
}

/// One of our annotations, if `d` is one.
fn read_mark(doc: &Document, d: &Dictionary, page: usize) -> Option<Mark> {
    let nm = get_text(doc, d, b"NM")?;
    let nm = nm.strip_prefix(OUR_NM).or_else(|| nm.strip_prefix(OLD_NM))?;
    let rect = get_rect(doc, d, b"Rect")?;
    let color = get_color(doc, d, b"C").unwrap_or([0.0; 3]);
    match get_name(doc, d, b"Subtype")?.as_str() {
        "FreeText" => {
            let text = get_text(doc, d, b"Contents").unwrap_or_default();
            let (size, color) = parse_da(&get_text(doc, d, b"DA").unwrap_or_default());
            Some(Mark { id: None, page, rect, color, kind: MarkKind::Text { text, size: if size > 0.0 { size } else { 12.0 } } })
        }
        "Ink" => {
            let width = get_dict(doc, d, b"BS").and_then(|bs| get_f32(doc, bs, b"W")).unwrap_or(1.0);
            let list = get(doc, d, b"InkList")?.as_array().ok()?;
            let (w, h) = ((rect[2] - rect[0]).max(1e-3), (rect[3] - rect[1]).max(1e-3));
            let strokes = list
                .iter()
                .filter_map(|s| deref(doc, s).as_array().ok())
                .map(|s| {
                    let v: Vec<f32> = s.iter().filter_map(|o| deref(doc, o).as_float().ok()).collect();
                    v.as_chunks::<2>().0.iter().map(|p| [(p[0] - rect[0]) / w, (p[1] - rect[1]) / h]).collect()
                })
                .collect();
            Some(Mark { id: None, page, rect, color, kind: MarkKind::Ink { strokes, width, signature: nm.starts_with("sig") } })
        }
        _ => None,
    }
}

// ---- appearance streams ---------------------------------------------------

/// Helvetica advance widths (per 1000 em) for WinAnsi codes 32..=126.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // space../
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, // 0..?
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, // @..O
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, // P.._
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, // `..o
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, // p..~
];

/// A character as a WinAnsi byte ('?' when it has none).
fn win_ansi(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\u{A0}'..='\u{FF}' => c as u32 as u8,
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        _ => b'?',
    }
}

/// Width of a WinAnsi byte in Helvetica, per 1000 em. Accented letters
/// share their base letter's width.
fn byte_width(b: u8) -> u16 {
    const BASE: &[u8; 64] = b"AAAAAA?CEEEEIIIIDNOOOOOxOUUUUYPsaaaaaa?ceeeeiiiidnooooo/ouuuuypy";
    match b {
        32..=126 => HELV[(b - 32) as usize],
        0xC6 => 1000,
        0xE6 => 889,
        0xC0..=0xFF => match BASE[(b - 0xC0) as usize] {
            b'?' | b'x' | b'/' => 584,
            base => HELV[(base - 32) as usize],
        },
        0x85 | 0x89 | 0x97 => 1000,
        0x96 | 0x95 => 556,
        _ => 556,
    }
}

/// Width of `s` in Helvetica at `size`.
pub fn text_width(s: &str, size: f32) -> f32 {
    s.chars().map(|c| byte_width(win_ansi(c)) as f32).sum::<f32>() * size / 1000.0
}

/// A literal string for a content stream, WinAnsi encoded.
fn pdf_string(s: &str) -> Vec<u8> {
    let mut out = vec![b'('];
    for c in s.chars() {
        let b = win_ansi(c);
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
    out
}

/// [`pdf_string`] with each byte as the char of the same value, for
/// assembling content; [`latin1_bytes`] turns it back.
fn pdf_str(s: &str) -> String {
    pdf_string(s).into_iter().map(char::from).collect()
}

/// Break `text` into lines no wider than `width` (at `size`), at spaces
/// where possible and honouring newlines.
pub fn wrap(text: &str, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if text_width(&candidate, size) <= width || line.is_empty() && text_width(word, size) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word longer than the line is broken by characters.
            let mut piece = String::new();
            for ch in word.chars() {
                piece.push(ch);
                if text_width(&piece, size) > width && piece.chars().count() > 1 {
                    piece.pop();
                    lines.push(std::mem::take(&mut piece));
                    piece.push(ch);
                }
            }
            line = piece;
        }
        lines.push(line);
    }
    lines
}

fn fmt(v: f32) -> String {
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn color_op(c: [f32; 3], stroke: bool) -> String {
    format!("{} {} {} {}", fmt(c[0]), fmt(c[1]), fmt(c[2]), if stroke { "RG" } else { "rg" })
}

fn helvetica() -> Dictionary {
    let mut f = Dictionary::new();
    f.set("Type", "Font");
    f.set("Subtype", "Type1");
    f.set("BaseFont", "Helvetica");
    f.set("Encoding", "WinAnsiEncoding");
    f
}

/// A form XObject of size w × h drawing `ops`, with Helvetica as /TbHelv.
fn form_xobject(w: f32, h: f32, ops: String, font: bool) -> Stream {
    let mut d = Dictionary::new();
    d.set("Type", "XObject");
    d.set("Subtype", "Form");
    d.set("BBox", vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)]);
    let mut res = Dictionary::new();
    if font {
        let mut fonts = Dictionary::new();
        fonts.set("TbHelv", helvetica());
        res.set("Font", fonts);
    }
    d.set("Resources", res);
    Stream::new(d, ops.into_bytes())
}

/// The font size a field is drawn at: its own, or one that fits.
pub fn field_font_size(field: &Field, rect: PdfRect, text: &str) -> f32 {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    if field.font_size > 0.0 {
        return field.font_size;
    }
    match field.kind {
        FieldKind::Text { multiline: true, .. } => {
            let mut s = 12.0f32;
            while s > 4.0 && wrap(text, s, w - 4.0).len() as f32 * s * 1.15 > h - 4.0 {
                s -= 0.5;
            }
            s
        }
        FieldKind::Text { comb: true, .. } => ((h - 4.0) / 1.15).clamp(4.0, 12.0),
        _ => {
            let s = ((h - 4.0) / 1.15).clamp(4.0, 12.0);
            let tw = text_width(text, s);
            if tw > w - 4.0 && tw > 0.0 { (s * (w - 4.0) / tw).max(4.0) } else { s }
        }
    }
}

/// What a field's text shows (passwords as bullets, choices by label).
pub fn display_text(field: &Field) -> String {
    match (&field.kind, &field.value) {
        (FieldKind::Text { password: true, .. }, Value::Text(t)) => "*".repeat(t.chars().count()),
        (FieldKind::Choice { options, .. }, Value::Text(t)) => {
            options.iter().find(|(e, _)| e == t).map_or_else(|| t.clone(), |(_, d)| d.clone())
        }
        (_, Value::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

/// The box and background a widget draws behind its content.
fn widget_frame(w: &Widget, width: f32, height: f32) -> String {
    let mut ops = String::new();
    if let Some(bg) = w.background {
        ops += &format!("{} 0 0 {} {} re f\n", color_op(bg, false), fmt(width), fmt(height));
    }
    if let Some(bc) = w.border {
        ops += &format!("{} 1 w 0.5 0.5 {} {} re S\n", color_op(bc, true), fmt(width - 1.0), fmt(height - 1.0));
    }
    ops
}

/// The normal appearance of a text or choice field's widget.
pub fn text_appearance(field: &Field, w: &Widget) -> Stream {
    let (width, height) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
    let text = display_text(field);
    let size = field_font_size(field, w.rect, &text);
    let mut ops = widget_frame(w, width, height);
    ops += "/Tx BMC\nq\n";
    ops += &format!("1 1 {} {} re W n\n", fmt(width - 2.0), fmt(height - 2.0));
    ops += &format!("BT\n/TbHelv {} Tf\n{}\n", fmt(size), color_op(field.color, false));
    let x_for = |line: &str| match field.align {
        1 => (width - text_width(line, size)) / 2.0,
        2 => width - 2.0 - text_width(line, size),
        _ => 2.0,
    };
    match &field.kind {
        FieldKind::Choice { combo: false, options, .. } => {
            // A list box: every option, the chosen one highlighted.
            let lead = size * 1.15;
            let current = match &field.value {
                Value::Text(t) => t.as_str(),
                _ => "",
            };
            ops += "ET\n";
            let mut top = height - 1.0;
            let mut rows = String::new();
            for (export, label) in options {
                if top - lead < 0.0 {
                    break;
                }
                if export == current {
                    ops += &format!("0.6 0.757 0.855 rg 1 {} {} {} re f\n", fmt(top - lead), fmt(width - 2.0), fmt(lead));
                }
                rows += &format!("1 0 0 1 2 {} Tm ", fmt(top - lead + (lead - size * 0.718) / 2.0));
                rows.push_str(&pdf_str(label));
                rows += " Tj\n";
                top -= lead;
            }
            ops += &format!("BT\n/TbHelv {} Tf\n{}\n{rows}", fmt(size), color_op(field.color, false));
        }
        FieldKind::Text { multiline: true, .. } => {
            let lead = size * 1.15;
            let mut y = height - 2.0 - size * 0.93;
            for line in wrap(&text, size, width - 4.0) {
                ops += &format!("1 0 0 1 {} {} Tm ", fmt(x_for(&line)), fmt(y));
                ops.push_str(&pdf_str(&line));
                ops += " Tj\n";
                y -= lead;
            }
        }
        FieldKind::Text { comb: true, max_len: Some(n), .. } if *n > 0 => {
            let n = *n;
            let cell = width / n as f32;
            let y = (height - size * 0.718) / 2.0;
            for (i, ch) in text.chars().take(n).enumerate() {
                let s = ch.to_string();
                let x = cell * i as f32 + (cell - text_width(&s, size)) / 2.0;
                ops += &format!("1 0 0 1 {} {} Tm ", fmt(x), fmt(y));
                ops.push_str(&pdf_str(&s));
                ops += " Tj\n";
            }
        }
        _ => {
            let y = (height - size * 0.718) / 2.0;
            ops += &format!("1 0 0 1 {} {} Tm ", fmt(x_for(&text)), fmt(y));
            ops.push_str(&pdf_str(&text));
            ops += " Tj\n";
        }
    }
    ops += "ET\nQ\nEMC\n";
    // The content may hold WinAnsi bytes above 127; build it bytewise.
    let mut st = form_xobject(width, height, String::new(), true);
    st.set_content(latin1_bytes(&ops));
    st
}

/// `ops` was assembled with each WinAnsi byte as the char of the same
/// value; turn it back into bytes.
fn latin1_bytes(ops: &str) -> Vec<u8> {
    ops.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect()
}

/// A tick for a check box (drawn as a path so no font is needed).
fn check_appearance(w: &Widget, color: [f32; 3], on: bool) -> Stream {
    let (width, height) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
    let mut ops = widget_frame(w, width, height);
    if on {
        let s = width.min(height);
        let (ox, oy) = ((width - s) / 2.0, (height - s) / 2.0);
        let p = |x: f32, y: f32| format!("{} {}", fmt(ox + x * s), fmt(oy + y * s));
        ops += &format!(
            "q {} {} w 1 J 1 j {} m {} l {} l S Q\n",
            color_op(color, true),
            fmt((s * 0.12).max(1.0)),
            p(0.2, 0.52),
            p(0.42, 0.28),
            p(0.8, 0.76)
        );
    }
    form_xobject(width, height, ops, false)
}

/// A dot for a radio button.
fn radio_appearance(w: &Widget, color: [f32; 3], on: bool) -> Stream {
    let (width, height) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
    let mut ops = widget_frame(w, width, height);
    if on {
        let r = width.min(height) * 0.25;
        ops += &format!("q {} {} f Q\n", color_op(color, false), circle(width / 2.0, height / 2.0, r));
    }
    form_xobject(width, height, ops, false)
}

/// A closed circle path (four Béziers).
fn circle(cx: f32, cy: f32, r: f32) -> String {
    let k = 0.5523 * r;
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c",
        fmt(cx + r), fmt(cy),
        fmt(cx + r), fmt(cy + k), fmt(cx + k), fmt(cy + r), fmt(cx), fmt(cy + r),
        fmt(cx - k), fmt(cy + r), fmt(cx - r), fmt(cy + k), fmt(cx - r), fmt(cy),
        fmt(cx - r), fmt(cy - k), fmt(cx - k), fmt(cy - r), fmt(cx), fmt(cy - r),
        fmt(cx + k), fmt(cy - r), fmt(cx + r), fmt(cy - k), fmt(cx + r), fmt(cy),
    )
}

/// The appearance of a mark: its text, or its strokes.
fn mark_appearance(m: &Mark) -> Stream {
    let (width, height) = (m.rect[2] - m.rect[0], m.rect[3] - m.rect[1]);
    match &m.kind {
        MarkKind::Text { text, size } => {
            let mut ops = format!("q BT /TbHelv {} Tf {}\n", fmt(*size), color_op(m.color, false));
            let mut y = height - TEXT_PAD - size * 0.93;
            for line in wrap(text, *size, width - 2.0 * TEXT_PAD) {
                ops += &format!("1 0 0 1 {} {} Tm ", fmt(TEXT_PAD), fmt(y));
                ops.push_str(&pdf_str(&line));
                ops += " Tj\n";
                y -= size * 1.15;
            }
            ops += "ET Q\n";
            let mut st = form_xobject(width, height, String::new(), true);
            st.set_content(latin1_bytes(&ops));
            st
        }
        MarkKind::Ink { strokes, width: lw, .. } => {
            let mut ops = format!("q {} {} w 1 J 1 j\n", color_op(m.color, true), fmt(*lw));
            for s in strokes {
                let Some(first) = s.first() else { continue };
                ops += &format!("{} {} m\n", fmt(first[0] * width), fmt(first[1] * height));
                if s.len() == 1 {
                    ops += &format!("{} {} l\n", fmt(first[0] * width + 0.01), fmt(first[1] * height));
                }
                for p in &s[1..] {
                    ops += &format!("{} {} l\n", fmt(p[0] * width), fmt(p[1] * height));
                }
                ops += "S\n";
            }
            ops += "Q\n";
            form_xobject(width, height, ops, false)
        }
    }
}

/// Inner margin of a text mark, in points.
pub const TEXT_PAD: f32 = 2.0;

/// The height a text mark needs for its text at its width.
pub fn text_mark_height(text: &str, size: f32, width: f32) -> f32 {
    let lines = wrap(text, size, width - 2.0 * TEXT_PAD).len().max(1);
    lines as f32 * size * 1.15 + 2.0 * TEXT_PAD
}

// ---- writing --------------------------------------------------------------

/// Changes to a base document: replaced and new objects.
struct Edit<'a> {
    base: &'a Document,
    out: BTreeMap<ObjectId, Object>,
    max_id: u32,
}

impl<'a> Edit<'a> {
    fn new(base: &'a Document) -> Self {
        Edit { base, out: BTreeMap::new(), max_id: base.max_id }
    }

    fn get(&self, id: ObjectId) -> Option<&Object> {
        self.out.get(&id).or_else(|| self.base.objects.get(&id))
    }

    fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        if !self.out.contains_key(&id) {
            let o = self.base.objects.get(&id)?.clone();
            self.out.insert(id, o);
        }
        self.out.get_mut(&id)
    }

    fn dict_mut(&mut self, id: ObjectId) -> Option<&mut Dictionary> {
        self.get_mut(id)?.as_dict_mut().ok()
    }

    fn add(&mut self, o: impl Into<Object>) -> ObjectId {
        self.max_id += 1;
        let id = (self.max_id, 0);
        self.out.insert(id, o.into());
        id
    }

    /// The page's /Annots array (made direct on the page if it was a
    /// shared or missing array).
    fn annots_mut(&mut self, page: ObjectId) -> Option<&mut Vec<Object>> {
        let current = self.get(page)?.as_dict().ok()?.get(b"Annots").ok().cloned();
        let list = match current {
            Some(Object::Reference(r)) => self.get(r).and_then(|o| o.as_array().ok()).cloned().unwrap_or_default(),
            Some(Object::Array(a)) => a,
            _ => Vec::new(),
        };
        let d = self.dict_mut(page)?;
        d.set("Annots", list);
        d.get_mut(b"Annots").ok()?.as_array_mut().ok()
    }
}

/// PDF date for now ("D:20261025143000Z").
fn pdf_date() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// The user's full name, for annotation authorship.
fn author() -> String {
    let user = std::env::var("USER").unwrap_or_default();
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|p| {
            p.lines().find(|l| l.split(':').next() == Some(user.as_str())).and_then(|l| {
                let gecos = l.split(':').nth(4)?.split(',').next()?.trim().to_string();
                (!gecos.is_empty()).then_some(gecos)
            })
        })
        .unwrap_or(user)
}

fn rect_object(r: PdfRect) -> Object {
    Object::Array(r.iter().map(|v| Object::Real(*v)).collect())
}

/// Apply the model to `base`, giving the changed and new objects.
fn apply<'a>(base: &'a Document, original: &Model, model: &Model) -> Edit<'a> {
    let mut e = Edit::new(base);
    let pages: Vec<ObjectId> = base.get_pages().into_values().collect();
    // Fields whose value changed.
    let before: HashMap<ObjectId, &Field> = original.fields.iter().map(|f| (f.id, f)).collect();
    for f in &model.fields {
        if before.get(&f.id).is_some_and(|b| b.value == f.value) {
            continue;
        }
        if let Some(d) = e.dict_mut(f.id) {
            match &f.value {
                Value::Text(t) => d.set("V", text_object(t)),
                Value::State(Some(s)) => d.set("V", Object::Name(s.as_bytes().to_vec())),
                Value::State(None) => d.set("V", Object::Name(b"Off".to_vec())),
            }
        }
        for w in &f.widgets {
            match (&f.kind, &f.value) {
                (FieldKind::Check | FieldKind::Radio, Value::State(state)) => {
                    let on = w.on.clone().unwrap_or_else(|| "Yes".into());
                    let is_on = state.as_deref() == Some(on.as_str());
                    let has_ap = e
                        .get(w.id)
                        .and_then(|o| o.as_dict().ok())
                        .and_then(|d| d.get(b"AP").ok())
                        .is_some_and(|ap| match base.dereference(ap) {
                            Ok((_, Object::Dictionary(ap))) => ap.has(b"N"),
                            _ => false,
                        });
                    if !has_ap {
                        let radio = matches!(f.kind, FieldKind::Radio);
                        let mk = |is_on| if radio { radio_appearance(w, f.color, is_on) } else { check_appearance(w, f.color, is_on) };
                        let on_id = e.add(mk(true));
                        let off_id = e.add(mk(false));
                        let mut n = Dictionary::new();
                        n.set(on.as_bytes().to_vec(), on_id);
                        n.set("Off", off_id);
                        let mut ap = Dictionary::new();
                        ap.set("N", n);
                        if let Some(d) = e.dict_mut(w.id) {
                            d.set("AP", ap);
                        }
                    }
                    if let Some(d) = e.dict_mut(w.id) {
                        d.set("AS", Object::Name(if is_on { on.into_bytes() } else { b"Off".to_vec() }));
                    }
                }
                _ => {
                    let ap_id = e.add(text_appearance(f, w));
                    if let Some(d) = e.dict_mut(w.id) {
                        let mut ap = Dictionary::new();
                        ap.set("N", ap_id);
                        d.set("AP", ap);
                    }
                }
            }
        }
    }
    // Marks: removed, changed and new.
    let kept: HashSet<ObjectId> = model.marks.iter().filter_map(|m| m.id).collect();
    for old in &original.marks {
        let Some(id) = old.id else { continue };
        if !kept.contains(&id)
            && let Some(&pid) = pages.get(old.page)
            && let Some(a) = e.annots_mut(pid)
        {
            a.retain(|o| o.as_reference().ok() != Some(id));
        }
    }
    let previous: HashMap<ObjectId, &Mark> = original.marks.iter().filter_map(|m| Some((m.id?, m))).collect();
    let (date, who) = (pdf_date(), author());
    for (n, m) in model.marks.iter().enumerate() {
        if m.id.is_some_and(|id| previous.get(&id).is_some_and(|p| *p == m)) {
            continue;
        }
        let Some(&pid) = pages.get(m.page) else { continue };
        let ap_id = e.add(mark_appearance(m));
        let mut d = Dictionary::new();
        d.set("Type", "Annot");
        d.set("Rect", rect_object(m.rect));
        d.set("F", 4);
        d.set("P", pid);
        d.set("M", text_object(&date));
        d.set("T", text_object(&who));
        d.set("C", Object::Array(m.color.iter().map(|v| Object::Real(*v)).collect()));
        let mut ap = Dictionary::new();
        ap.set("N", ap_id);
        d.set("AP", ap);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros());
        match &m.kind {
            MarkKind::Text { text, size } => {
                d.set("Subtype", "FreeText");
                d.set("NM", text_object(&format!("{OUR_NM}text-{stamp}-{n}")));
                d.set("Contents", text_object(text));
                d.set("DA", text_object(&format!("/Helv {} Tf {}", fmt(*size), color_op(m.color, false))));
                let mut bs = Dictionary::new();
                bs.set("W", 0);
                d.set("BS", bs);
            }
            MarkKind::Ink { strokes, width, signature } => {
                d.set("Subtype", "Ink");
                let kind = if *signature { "sig" } else { "ink" };
                d.set("NM", text_object(&format!("{OUR_NM}{kind}-{stamp}-{n}")));
                let (w, h) = (m.rect[2] - m.rect[0], m.rect[3] - m.rect[1]);
                let list = strokes
                    .iter()
                    .map(|s| {
                        Object::Array(
                            s.iter()
                                .flat_map(|p| [Object::Real(m.rect[0] + p[0] * w), Object::Real(m.rect[1] + p[1] * h)])
                                .collect(),
                        )
                    })
                    .collect::<Vec<_>>();
                d.set("InkList", list);
                let mut bs = Dictionary::new();
                bs.set("W", Object::Real(*width));
                d.set("BS", bs);
            }
        }
        match m.id {
            Some(id) => {
                e.out.insert(id, Object::Dictionary(d));
            }
            None => {
                let id = e.add(d);
                if let Some(a) = e.annots_mut(pid) {
                    a.push(Object::Reference(id));
                }
            }
        }
    }
    e
}

/// Whether the file can take an incremental update (its cross-reference
/// data was read as written).
fn incremental_ok(doc: &Document) -> bool {
    doc.xref_start != 0 && doc.xref_recovery.is_none()
}

/// The file with `model`'s changes: appended as an incremental update
/// when possible, otherwise rewritten.
pub fn save(bytes: &[u8], base: &Document, original: &Model, model: &Model) -> Result<Vec<u8>, String> {
    let e = apply(base, original, model);
    write(bytes, base, e.out, e.max_id)
}

fn write(bytes: &[u8], base: &Document, out: BTreeMap<ObjectId, Object>, max_id: u32) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    if out.is_empty() {
        return Ok(bytes.to_vec());
    }
    if incremental_ok(base) {
        let mut inc = IncrementalDocument::create_from(bytes.to_vec(), base.clone());
        inc.new_document.max_id = max_id;
        for (id, o) in out {
            inc.new_document.set_object(id, o);
        }
        inc.save_to(&mut buf).map_err(|e| e.to_string())?;
    } else {
        let mut doc = base.clone();
        doc.max_id = max_id;
        doc.objects.extend(out);
        doc.save_to(&mut buf).map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

/// The bytes the pages are rendered from: the file without the
/// annotations Viewer draws itself.
pub fn render_bytes(bytes: &[u8], base: &Document, model: &Model) -> Result<Vec<u8>, String> {
    let ours = model.drawn_by_us();
    if ours.is_empty() {
        return Ok(bytes.to_vec());
    }
    let mut e = Edit::new(base);
    let pages: Vec<ObjectId> = base.get_pages().into_values().collect();
    let on: HashSet<usize> = model
        .fields
        .iter()
        .flat_map(|f| f.widgets.iter().map(|w| w.page))
        .chain(model.marks.iter().map(|m| m.page))
        .collect();
    for i in on {
        if let Some(&pid) = pages.get(i)
            && let Some(a) = e.annots_mut(pid)
        {
            a.retain(|o| o.as_reference().map_or(true, |id| !ours.contains(&id)));
        }
    }
    write(bytes, base, e.out, e.max_id)
}

/// A copy with every visible annotation (form fields, marks, stamps)
/// burnt into its page and the form removed: what you see, fixed.
pub fn flatten(bytes: &[u8], base: &Document, original: &Model, model: &Model) -> Result<Vec<u8>, String> {
    let e = apply(base, original, model);
    let mut doc = base.clone();
    doc.max_id = e.max_id;
    doc.objects.extend(e.out);
    let _ = bytes;
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    for pid in pages {
        flatten_page(&mut doc, pid);
    }
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"AcroForm");
    }
    doc.prune_objects();
    doc.compress();
    let mut buf = Vec::new();
    doc.save_to(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn flatten_page(doc: &mut Document, pid: ObjectId) {
    let Ok(page) = doc.get_dictionary(pid) else { return };
    let annots: Vec<Object> = match page.get(b"Annots") {
        Ok(Object::Reference(r)) => doc.get_object(*r).and_then(|o| o.as_array()).cloned().unwrap_or_default(),
        Ok(Object::Array(a)) => a.clone(),
        _ => return,
    };
    let mut ops = String::new();
    let mut xobjects: Vec<(String, ObjectId)> = Vec::new();
    let mut keep = Vec::new();
    for (k, a) in annots.into_iter().enumerate() {
        let d = match &a {
            Object::Reference(r) => doc.get_dictionary(*r).ok(),
            Object::Dictionary(d) => Some(d),
            _ => None,
        };
        let Some(d) = d else { continue };
        let subtype = get_name(doc, d, b"Subtype").unwrap_or_default();
        if subtype == "Link" {
            keep.push(a);
            continue;
        }
        if subtype == "Popup" {
            continue;
        }
        let flags = get(doc, d, b"F").and_then(|o| o.as_i64().ok()).unwrap_or(0);
        if flags & 0b10_0010 != 0 {
            continue; // hidden or no-view
        }
        let Some(rect) = get_rect(doc, d, b"Rect") else { continue };
        // The normal appearance (a stream, or the state picked by /AS).
        let n = d.get(b"AP").ok().and_then(|ap| deref(doc, ap).as_dict().ok()).and_then(|ap| ap.get(b"N").ok());
        let stream_id = match n {
            Some(Object::Reference(r)) => match doc.get_object(*r) {
                Ok(Object::Stream(_)) => Some(*r),
                Ok(Object::Dictionary(states)) => get_name(doc, d, b"AS")
                    .and_then(|s| states.get(s.as_bytes()).ok())
                    .and_then(|o| o.as_reference().ok()),
                _ => None,
            },
            Some(Object::Dictionary(states)) => get_name(doc, d, b"AS")
                .and_then(|s| states.get(s.as_bytes()).ok())
                .and_then(|o| o.as_reference().ok()),
            _ => None,
        };
        let Some(sid) = stream_id else { continue };
        let Ok(Object::Stream(st)) = doc.get_object(sid) else { continue };
        let Some(bbox) = get_rect(doc, &st.dict, b"BBox") else { continue };
        let m: Vec<f32> = get(doc, &st.dict, b"Matrix")
            .and_then(|o| o.as_array().ok())
            .map(|a| a.iter().filter_map(|o| o.as_float().ok()).collect())
            .filter(|v: &Vec<f32>| v.len() == 6)
            .unwrap_or_else(|| vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        // The bbox through /Matrix, then mapped onto the annotation rect.
        let corners = [[bbox[0], bbox[1]], [bbox[2], bbox[1]], [bbox[0], bbox[3]], [bbox[2], bbox[3]]]
            .map(|[x, y]| [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]);
        let (bx0, by0) = (corners.iter().map(|p| p[0]).fold(f32::MAX, f32::min), corners.iter().map(|p| p[1]).fold(f32::MAX, f32::min));
        let (bx1, by1) = (corners.iter().map(|p| p[0]).fold(f32::MIN, f32::max), corners.iter().map(|p| p[1]).fold(f32::MIN, f32::max));
        if bx1 - bx0 <= 0.0 || by1 - by0 <= 0.0 {
            continue;
        }
        let (sx, sy) = ((rect[2] - rect[0]) / (bx1 - bx0), (rect[3] - rect[1]) / (by1 - by0));
        let name = format!("TbFlat{k}");
        ops += &format!(
            "q {} 0 0 {} {} {} cm /{name} Do Q\n",
            fmt(sx),
            fmt(sy),
            fmt(rect[0] - bx0 * sx),
            fmt(rect[1] - by0 * sy)
        );
        xobjects.push((name, sid));
    }
    for (_, sid) in &xobjects {
        if let Ok(Object::Stream(st)) = doc.get_object_mut(*sid) {
            st.dict.set("Type", "XObject");
            st.dict.set("Subtype", "Form");
        }
    }
    // Resources: the page's own (made direct), or inherited ones copied.
    let resources = {
        let page = doc.get_dictionary(pid).ok();
        let mut res = None;
        let mut node = page.cloned();
        for _ in 0..32 {
            let Some(n) = node else { break };
            if let Some(r) = get(doc, &n, b"Resources").and_then(|o| o.as_dict().ok()) {
                res = Some(r.clone());
                break;
            }
            node = n.get(b"Parent").ok().and_then(|p| p.as_reference().ok()).and_then(|p| doc.get_dictionary(p).ok()).cloned();
        }
        res.unwrap_or_default()
    };
    let mut resources = resources;
    let mut xo = resources.get(b"XObject").ok().and_then(|o| deref(doc, o).as_dict().ok()).cloned().unwrap_or_default();
    for (name, sid) in &xobjects {
        xo.set(name.as_bytes().to_vec(), *sid);
    }
    resources.set("XObject", xo);
    // Wrap the page content in q/Q so its graphics state can't leak into
    // the appended appearances.
    let contents: Vec<Object> = match doc.get_dictionary(pid).ok().and_then(|p| p.get(b"Contents").ok()) {
        Some(Object::Array(a)) => a.clone(),
        Some(Object::Reference(r)) => match doc.get_object(*r) {
            Ok(Object::Array(a)) => a.clone(),
            _ => vec![Object::Reference(*r)],
        },
        _ => Vec::new(),
    };
    let q = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let tail = doc.add_object(Stream::new(Dictionary::new(), format!("\nQ\n{ops}").into_bytes()));
    let mut new_contents = vec![Object::Reference(q)];
    new_contents.extend(contents);
    new_contents.push(Object::Reference(tail));
    if let Ok(page) = doc.get_dictionary_mut(pid) {
        page.set("Resources", resources);
        page.set("Contents", new_contents);
        if keep.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", keep);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with a text field, a check box and radio buttons.
    pub fn sample_form() -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let page_id = doc.new_object_id();
        let content = doc.add_object(Stream::new(Dictionary::new(), b"BT /F1 12 Tf 72 720 Td (Name:) Tj ET".to_vec()));
        let mut font = helvetica();
        font.remove(b"Encoding");
        let font_id = doc.add_object(font);
        let mk_widget = |doc: &mut Document, rect: [i64; 4], extra: Dictionary| -> ObjectId {
            let mut d = Dictionary::new();
            d.set("Type", "Annot");
            d.set("Subtype", "Widget");
            d.set("Rect", rect.iter().map(|v| Object::Integer(*v)).collect::<Vec<_>>());
            d.set("P", page_id);
            d.extend(&extra);
            doc.add_object(d)
        };
        let mut t = Dictionary::new();
        t.set("FT", "Tx");
        t.set("T", Object::string_literal("name"));
        t.set("DA", Object::string_literal("/Helv 0 Tf 0 g"));
        let name = mk_widget(&mut doc, [120, 710, 320, 732], t);
        let mut c = Dictionary::new();
        c.set("FT", "Btn");
        c.set("T", Object::string_literal("agree"));
        c.set("V", "Off");
        let agree = mk_widget(&mut doc, [72, 680, 86, 694], c);
        // A radio group with two kids.
        let group = doc.new_object_id();
        let mut kids = Vec::new();
        for (i, x) in [72i64, 120].iter().enumerate() {
            let mut k = Dictionary::new();
            k.set("Parent", group);
            let mut on = Dictionary::new();
            let on_st = doc.add_object(Stream::new(Dictionary::new(), Vec::new()));
            let off_st = doc.add_object(Stream::new(Dictionary::new(), Vec::new()));
            on.set(format!("opt{i}").into_bytes(), on_st);
            on.set("Off", off_st);
            let mut ap = Dictionary::new();
            ap.set("N", on);
            k.set("AP", ap);
            k.set("AS", "Off");
            kids.push(Object::Reference(mk_widget(&mut doc, [*x, 640, x + 14, 654], k)));
        }
        let mut g = Dictionary::new();
        g.set("FT", "Btn");
        g.set("Ff", 1 << 15);
        g.set("T", Object::string_literal("size"));
        g.set("Kids", kids.clone());
        doc.objects.insert(group, Object::Dictionary(g));
        let mut annots = vec![Object::Reference(name), Object::Reference(agree)];
        annots.extend(kids);
        let mut fonts = Dictionary::new();
        fonts.set("F1", font_id);
        let mut res = Dictionary::new();
        res.set("Font", fonts);
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", pages_id);
        page.set("MediaBox", vec![0.into(), 0.into(), 612.into(), 792.into()]);
        page.set("Contents", content);
        page.set("Resources", res);
        page.set("Annots", annots);
        doc.objects.insert(page_id, Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", "Pages");
        pages.set("Kids", vec![Object::Reference(page_id)]);
        pages.set("Count", 1);
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let mut form = Dictionary::new();
        form.set("Fields", vec![Object::Reference(name), Object::Reference(agree), Object::Reference(group)]);
        let mut cat = Dictionary::new();
        cat.set("Type", "Catalog");
        cat.set("Pages", pages_id);
        cat.set("AcroForm", form);
        let cat_id = doc.add_object(cat);
        doc.trailer.set("Root", cat_id);
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        buf
    }

    fn load(b: &[u8]) -> Document {
        Document::load_mem(b).unwrap()
    }

    #[test]
    fn reads_fields() {
        let doc = load(&sample_form());
        let m = read_model(&doc);
        let names: Vec<_> = m.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["name", "agree", "size"]);
        assert!(matches!(m.fields[0].kind, FieldKind::Text { multiline: false, .. }));
        assert_eq!(m.fields[1].kind, FieldKind::Check);
        assert_eq!(m.fields[1].widgets[0].on.as_deref(), Some("Yes"));
        assert_eq!(m.fields[2].kind, FieldKind::Radio);
        let ons: Vec<_> = m.fields[2].widgets.iter().map(|w| w.on.clone().unwrap()).collect();
        assert_eq!(ons, ["opt0", "opt1"]);
        assert_eq!(m.fields[0].widgets[0].rect, [120.0, 710.0, 320.0, 732.0]);
    }

    #[test]
    fn fills_signs_and_reads_back() {
        let bytes = sample_form();
        let doc = load(&bytes);
        let original = read_model(&doc);
        let mut m = original.clone();
        m.fields[0].value = Value::Text("Zoë O’Brien".into());
        m.fields[1].value = Value::State(Some("Yes".into()));
        m.fields[2].value = Value::State(Some("opt1".into()));
        m.marks.push(Mark {
            id: None,
            page: 0,
            rect: [300.0, 100.0, 420.0, 140.0],
            color: [0.0, 0.0, 0.5],
            kind: MarkKind::Ink { strokes: vec![vec![[0.0, 0.0], [0.5, 1.0], [1.0, 0.2]]], width: 1.5, signature: true },
        });
        m.marks.push(Mark {
            id: None,
            page: 0,
            rect: [72.0, 500.0, 272.0, 520.0],
            color: [0.0; 3],
            kind: MarkKind::Text { text: "Approved (see note)".into(), size: 12.0 },
        });
        let saved = save(&bytes, &doc, &original, &m).unwrap();
        // An incremental update: the original bytes are untouched.
        assert!(saved.starts_with(&bytes));
        let doc2 = load(&saved);
        let back = read_model(&doc2);
        assert_eq!(back.fields[0].value, Value::Text("Zoë O’Brien".into()));
        assert_eq!(back.fields[1].value, Value::State(Some("Yes".into())));
        assert_eq!(back.fields[2].value, Value::State(Some("opt1".into())));
        assert_eq!(back.marks.len(), 2);
        assert!(matches!(&back.marks[0].kind, MarkKind::Ink { signature: true, strokes, .. } if strokes[0].len() == 3));
        assert!(matches!(&back.marks[1].kind, MarkKind::Text { text, .. } if text == "Approved (see note)"));
        // Every changed widget has an appearance.
        let w = doc2.get_dictionary(back.fields[2].widgets[1].id).unwrap();
        assert_eq!(w.get(b"AS").unwrap().as_name().unwrap(), b"opt1");
        let w = doc2.get_dictionary(back.fields[0].widgets[0].id).unwrap();
        assert!(w.get(b"AP").is_ok());
        // hayro renders the result.
        let pdf = hayro::hayro_syntax::Pdf::new(saved.clone()).unwrap();
        assert_eq!(pdf.pages().len(), 1);

        // Edit again: move the text, delete the signature.
        let mut m2 = back.clone();
        m2.marks.remove(0);
        m2.marks[0].rect[1] -= 10.0;
        m2.marks[0].rect[3] -= 10.0;
        let saved2 = save(&saved, &doc2, &back, &m2).unwrap();
        let back2 = read_model(&load(&saved2));
        assert_eq!(back2.marks.len(), 1);
        assert_eq!(back2.marks[0].rect[1], 490.0);
    }

    #[test]
    fn render_bytes_hide_our_annotations() {
        let bytes = sample_form();
        let doc = load(&bytes);
        let m = read_model(&doc);
        let r = render_bytes(&bytes, &doc, &m).unwrap();
        let d = load(&r);
        let page = d.get_pages()[&1];
        let annots = d.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap();
        assert!(annots.is_empty());
    }

    #[test]
    fn marks_saved_before_the_rename_are_ours() {
        let bytes = sample_form();
        let doc = load(&bytes);
        let original = read_model(&doc);
        let mut m = original.clone();
        m.marks.push(Mark {
            id: None,
            page: 0,
            rect: [300.0, 100.0, 420.0, 140.0],
            color: [0.0; 3],
            kind: MarkKind::Ink { strokes: vec![vec![[0.0, 0.0], [1.0, 1.0]]], width: 1.0, signature: true },
        });
        let mut doc2 = load(&save(&bytes, &doc, &original, &m).unwrap());
        let id = read_model(&doc2).marks[0].id.unwrap();
        let d = doc2.get_dictionary_mut(id).unwrap();
        let nm = get_text(&Document::new(), d, b"NM").unwrap().replacen(OUR_NM, OLD_NM, 1);
        d.set("NM", text_object(&nm));
        let back = read_model(&doc2);
        assert_eq!(back.marks.len(), 1);
        assert!(matches!(&back.marks[0].kind, MarkKind::Ink { signature: true, .. }));
    }

    #[test]
    fn flattens() {
        let bytes = sample_form();
        let doc = load(&bytes);
        let original = read_model(&doc);
        let mut m = original.clone();
        m.fields[0].value = Value::Text("Ben".into());
        m.fields[1].value = Value::State(Some("Yes".into()));
        let flat = flatten(&bytes, &doc, &original, &m).unwrap();
        let d = load(&flat);
        assert!(d.catalog().unwrap().get(b"AcroForm").is_err());
        let page = d.get_pages()[&1];
        assert!(d.get_dictionary(page).unwrap().get(b"Annots").is_err());
        let content = String::from_utf8_lossy(&d.get_page_content(page)).into_owned();
        assert!(content.contains("/TbFlat0 Do"), "{content}");
        assert!(read_model(&d).fields.is_empty());
    }

    /// A Letter-size application form with every kind of field, for
    /// trying Viewer by hand:
    /// `OUT=form.pdf cargo test -p olo-viewer write_fixture -- --ignored`
    #[test]
    #[ignore]
    fn write_fixture() {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let page_id = doc.new_object_id();
        let mut content = String::from("BT /F2 20 Tf 72 730 Td (Volunteer Application) Tj ET\n");
        content += "0.6 G 0.5 w 72 718 m 540 718 l S\n";
        let mut fields = Vec::new();
        let mut annots = Vec::new();
        let label = |content: &mut String, y: f32, text: &str| {
            *content += &format!("BT /F1 11 Tf 72 {} Td ({text}) Tj ET\n", y);
        };
        let add = |doc: &mut Document, d: Dictionary, fields: &mut Vec<Object>, annots: &mut Vec<Object>| {
            let id = doc.add_object(d);
            fields.push(Object::Reference(id));
            annots.push(Object::Reference(id));
            id
        };
        let widget = |rect: [f32; 4], name: &str, ft: &str| {
            let mut d = Dictionary::new();
            d.set("Type", "Annot");
            d.set("Subtype", "Widget");
            d.set("Rect", rect_object(rect));
            d.set("P", page_id);
            d.set("FT", Object::Name(ft.as_bytes().to_vec()));
            d.set("T", text_object(name));
            d.set("DA", Object::string_literal("/Helv 0 Tf 0 g"));
            let mut mk = Dictionary::new();
            mk.set("BC", vec![Object::Real(0.55), Object::Real(0.55), Object::Real(0.6)]);
            d.set("MK", mk);
            d
        };
        let rows = [("Full name", 690.0), ("Email", 660.0), ("Phone", 630.0)];
        for (name, y) in rows {
            label(&mut content, y + 6.0, &format!("{name}:"));
            add(&mut doc, widget([170.0, y, 540.0, y + 20.0], name, "Tx"), &mut fields, &mut annots);
        }
        label(&mut content, 606.0, "Postcode:");
        let mut comb = widget([170.0, 600.0, 290.0, 620.0], "Postcode", "Tx");
        comb.set("Ff", 1 << 24);
        comb.set("MaxLen", 6);
        add(&mut doc, comb, &mut fields, &mut annots);
        label(&mut content, 576.0, "Availability:");
        let mut combo = widget([170.0, 570.0, 330.0, 590.0], "Availability", "Ch");
        combo.set("Ff", 1 << 17);
        combo.set(
            "Opt",
            ["Weekdays", "Weekends", "Evenings", "Any time"].iter().map(|o| Object::string_literal(*o)).collect::<Vec<_>>(),
        );
        add(&mut doc, combo, &mut fields, &mut annots);
        label(&mut content, 546.0, "Area:");
        let mut list = widget([170.0, 480.0, 330.0, 560.0], "Area", "Ch");
        list.set(
            "Opt",
            ["Kitchen", "Front desk", "Garden", "Driving"].iter().map(|o| Object::string_literal(*o)).collect::<Vec<_>>(),
        );
        list.set("V", Object::string_literal("Garden"));
        add(&mut doc, list, &mut fields, &mut annots);
        label(&mut content, 456.0, "T-shirt size:");
        let group = doc.new_object_id();
        let mut kids = Vec::new();
        for (i, size) in ["S", "M", "L", "XL"].iter().enumerate() {
            let x = 170.0 + i as f32 * 60.0;
            content += &format!("BT /F1 11 Tf {} 456 Td ({size}) Tj ET\n", x + 18.0);
            let mut k = Dictionary::new();
            k.set("Type", "Annot");
            k.set("Subtype", "Widget");
            k.set("Rect", rect_object([x, 452.0, x + 14.0, 466.0]));
            k.set("P", page_id);
            k.set("Parent", group);
            let mut mk = Dictionary::new();
            mk.set("BC", vec![Object::Real(0.55), Object::Real(0.55), Object::Real(0.6)]);
            k.set("MK", mk);
            let w = Widget { id: (0, 0), page: 0, rect: [x, 452.0, x + 14.0, 466.0], on: None, border: Some([0.55, 0.55, 0.6]), background: None };
            let on = doc.add_object(radio_appearance(&w, [0.0; 3], true));
            let off = doc.add_object(radio_appearance(&w, [0.0; 3], false));
            let mut n = Dictionary::new();
            n.set(size.as_bytes().to_vec(), on);
            n.set("Off", off);
            let mut ap = Dictionary::new();
            ap.set("N", n);
            k.set("AP", ap);
            k.set("AS", "Off");
            let kid = doc.add_object(k);
            kids.push(Object::Reference(kid));
            annots.push(Object::Reference(kid));
        }
        let mut g = Dictionary::new();
        g.set("FT", "Btn");
        g.set("Ff", (1 << 15) | (1 << 14));
        g.set("T", Object::string_literal("Size"));
        g.set("Kids", kids);
        doc.objects.insert(group, Object::Dictionary(g));
        fields.push(Object::Reference(group));
        label(&mut content, 420.0, "Tell us about yourself:");
        let mut about = widget([72.0, 300.0, 540.0, 410.0], "About", "Tx");
        about.set("Ff", 1 << 12);
        add(&mut doc, about, &mut fields, &mut annots);
        let mut agree = widget([72.0, 262.0, 86.0, 276.0], "Agree", "Btn");
        agree.set("V", "Off");
        add(&mut doc, agree, &mut fields, &mut annots);
        content += "BT /F1 11 Tf 94 265 Td (I agree to the volunteer code of conduct.) Tj ET\n";
        label(&mut content, 200.0, "Signature:");
        content += "0 G 0.5 w 150 196 m 360 196 l S\n";
        label(&mut content, 200.0 - 0.0, "");
        content += "BT /F1 11 Tf 380 200 Td (Date:) Tj ET 0 G 0.5 w 415 196 m 540 196 l S\n";
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let mut fonts = Dictionary::new();
        fonts.set("F1", doc.add_object(helvetica()));
        let mut bold = helvetica();
        bold.set("BaseFont", "Helvetica-Bold");
        fonts.set("F2", doc.add_object(bold));
        let mut res = Dictionary::new();
        res.set("Font", fonts);
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", pages_id);
        page.set("MediaBox", vec![0.into(), 0.into(), 612.into(), 792.into()]);
        page.set("Contents", content_id);
        page.set("Resources", res);
        page.set("Annots", annots);
        doc.objects.insert(page_id, Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", "Pages");
        pages.set("Kids", vec![Object::Reference(page_id)]);
        pages.set("Count", 1);
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let mut form = Dictionary::new();
        form.set("Fields", fields);
        form.set("DA", Object::string_literal("/Helv 0 Tf 0 g"));
        let mut cat = Dictionary::new();
        cat.set("Type", "Catalog");
        cat.set("Pages", pages_id);
        cat.set("AcroForm", form);
        let cat_id = doc.add_object(cat);
        doc.trailer.set("Root", cat_id);
        doc.save(std::env::var("OUT").unwrap()).unwrap();
    }

    /// Render page 1 of $PDF to $OUT with hayro alone (what other viewers
    /// see of the saved appearances).
    #[test]
    #[ignore]
    fn render_png() {
        use hayro::{PixmapSettings, RenderCache, RenderSettings, hayro_interpret::InterpreterSettings, vello_cpu::color::palette::css::WHITE};
        let bytes = std::fs::read(std::env::var("PDF").unwrap()).unwrap();
        let pdf = hayro::hayro_syntax::Pdf::new(bytes).unwrap();
        let pages = pdf.pages();
        let pix = hayro::render(&pages[0], &RenderCache::new(), &InterpreterSettings::default(), &RenderSettings::default(), &PixmapSettings {
            x_scale: 2.0,
            y_scale: 2.0,
            bg_color: WHITE,
        });
        image::save_buffer(std::env::var("OUT").unwrap(), pix.data_as_u8_slice(), pix.width() as u32, pix.height() as u32, image::ColorType::Rgba8).unwrap();
    }

    /// `PDF=file cargo test -p olo-viewer dump_fields -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_fields() {
        let bytes = std::fs::read(std::env::var("PDF").unwrap()).unwrap();
        let doc = load(&bytes);
        let m = read_model(&doc);
        for f in &m.fields {
            println!("{:?} {:?} {:?} size={} widgets={:?}", f.name, f.kind, f.value, f.font_size, f.widgets);
        }
        println!("{} marks; incremental: {}", m.marks.len(), incremental_ok(&doc));
    }

    #[test]
    fn text_helpers() {
        assert_eq!(decode_text(&[0xFE, 0xFF, 0, b'H', 0, b'i']), "Hi");
        assert_eq!(wrap("hello world", 12.0, text_width("world", 12.0) + 1.0), ["hello", "world"]);
        assert_eq!(wrap("a\nb", 12.0, 100.0), ["a", "b"]);
        assert_eq!(pdf_string("a(b)"), b"(a\\(b\\))");
        assert!((text_width("Hello", 10.0) - 22.78).abs() < 0.01);
        assert_eq!(parse_da("/Helv 9 Tf 1 0 0 rg"), (9.0, [1.0, 0.0, 0.0]));
        assert!(pdf_date().starts_with("D:20"));
    }
}
