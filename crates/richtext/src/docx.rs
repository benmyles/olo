//! Word documents (Office Open XML, .docx): `word/document.xml` with its
//! styles, numbering and links.

use std::{collections::HashMap, fmt::Write as _, io::Write as _};

use crate::{
    Align, Document, Kind, List, Paragraph, Span, Tab, TabAlign, Table,
    xml::{X, attr, esc, events, hex_color, is_mono, zip_text},
};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
/// Twentieths of a point in half an inch: an indent level.
const LEVEL: u32 = 720;
/// The font code is set in.
const MONO: &str = "Courier New";
/// Twips across a page's text (US Letter less inch margins): a table's
/// width, shared out between its columns.
const TEXT_WIDTH: u32 = 9360;

fn style_id(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Body => None,
        Kind::Title => Some("Title"),
        Kind::Heading1 => Some("Heading1"),
        Kind::Heading2 => Some("Heading2"),
        Kind::Heading3 => Some("Heading3"),
        Kind::Quote => Some("Quote"),
        Kind::Code => Some("SourceCode"),
    }
}

/// A paragraph style's kind, by its name (or id).
pub(crate) fn kind_of(name: &str) -> Kind {
    let n = name.to_ascii_lowercase().replace(' ', "");
    match n.as_str() {
        "title" => Kind::Title,
        "heading1" => Kind::Heading1,
        "heading2" => Kind::Heading2,
        _ if n.starts_with("heading") && n[7..].parse::<u8>().is_ok() => Kind::Heading3,
        "quote" | "quotations" | "blockquotation" | "intensequote" | "blocktext" => Kind::Quote,
        _ if n.contains("code") || n.contains("source") || n.contains("preformatted") => Kind::Code,
        _ => Kind::Body,
    }
}

pub fn write(doc: &Document) -> Result<Vec<u8>, String> {
    let mut links: Vec<String> = Vec::new();
    // Each numbered list counts from 1: a numbering of its own.
    let mut numbered_lists = 0u32;
    let mut in_numbered = false;
    let mut body = String::new();
    for p in &doc.paragraphs {
        let numbered = p.list.is_some_and(|l| l.numbered);
        if numbered && !in_numbered {
            numbered_lists += 1;
        }
        if p.list.is_none() {
            in_numbered = false;
        } else if numbered {
            in_numbered = true;
        }
        match &p.table {
            Some(t) => table(&mut body, t, &mut links, numbered_lists),
            None => paragraph(&mut body, p, &mut links, numbered_lists),
        }
    }
    // (Word wants a paragraph last, after a table.)
    if doc.paragraphs.last().is_some_and(|p| p.table.is_some()) {
        body.push_str("<w:p/>");
    }
    // A header and footer, if there is one: parts of their own.
    let has = |p: &[String; 3]| p.iter().any(|s| !s.is_empty());
    let (header, footer) = (has(&doc.header), has(&doc.footer));
    let mut sect = String::from("<w:sectPr>");
    if header {
        sect.push_str("<w:headerReference w:type=\"default\" r:id=\"rIdHeader\"/>");
    }
    if footer {
        sect.push_str("<w:footerReference w:type=\"default\" r:id=\"rIdFooter\"/>");
    }
    sect.push_str("</w:sectPr>");
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document xmlns:w=\"{W}\" xmlns:r=\"{R}\"><w:body>{body}{sect}</w:body></w:document>"
    );
    let mut rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{PKG}\">\
         <Relationship Id=\"rIdStyles\" Type=\"{R}/styles\" Target=\"styles.xml\"/>\
         <Relationship Id=\"rIdNumbering\" Type=\"{R}/numbering\" Target=\"numbering.xml\"/>\
         <Relationship Id=\"rIdSettings\" Type=\"{R}/settings\" Target=\"settings.xml\"/>"
    );
    if header {
        let _ = write!(rels, "<Relationship Id=\"rIdHeader\" Type=\"{R}/header\" Target=\"header1.xml\"/>");
    }
    if footer {
        let _ = write!(rels, "<Relationship Id=\"rIdFooter\" Type=\"{R}/footer\" Target=\"footer1.xml\"/>");
    }
    for (i, url) in links.iter().enumerate() {
        let _ = write!(rels, "<Relationship Id=\"rIdL{}\" Type=\"{R}/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>", i + 1, esc(url));
    }
    rels.push_str("</Relationships>");
    let mut content_types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
        <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
        <Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>\
        <Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>",
    );
    if header {
        content_types.push_str("<Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/>");
    }
    if footer {
        content_types.push_str("<Override PartName=\"/word/footer1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml\"/>");
    }
    content_types.push_str("</Types>");
    let root_rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{PKG}\"><Relationship Id=\"rId1\" Type=\"{R}/officeDocument\" Target=\"word/document.xml\"/></Relationships>"
    );
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut files = vec![
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", root_rels),
        ("word/document.xml", document),
        ("word/styles.xml", styles(doc.font.as_deref())),
        ("word/numbering.xml", numbering(numbered_lists)),
        // Tabs past a paragraph's own stops go every half inch (as Word
        // has them, and Olo; LibreOffice's own is 1.25 cm).
        ("word/settings.xml", format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:settings xmlns:w=\"{W}\"><w:defaultTabStop w:val=\"720\"/></w:settings>")),
        ("word/_rels/document.xml.rels", rels),
    ];
    if header {
        files.push(("word/header1.xml", header_part("hdr", &doc.header)));
    }
    if footer {
        files.push(("word/footer1.xml", header_part("ftr", &doc.footer)));
    }
    for (name, data) in files {
        z.start_file(name, opts).map_err(|e| e.to_string())?;
        z.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(z.finish().map_err(|e| e.to_string())?.into_inner())
}

fn paragraph(out: &mut String, p: &Paragraph, links: &mut Vec<String>, numbered_list: u32) {
    out.push_str("<w:p><w:pPr>");
    if let Some(id) = style_id(p.kind) {
        let _ = write!(out, "<w:pStyle w:val=\"{id}\"/>");
    }
    if let Some(l) = p.list {
        // Bullets are numbering 1; numbered list k is numbering k + 1.
        let num = if l.numbered { numbered_list + 1 } else { 1 };
        let _ = write!(out, "<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{num}\"/></w:numPr>", l.level.min(8));
    }
    // (In the schema's order: tabs, then indents, then alignment.)
    if !p.tabs.is_empty() {
        out.push_str("<w:tabs>");
        for t in &p.tabs {
            let _ = write!(out, "<w:tab w:val=\"{}\" w:pos=\"{}\"/>", tab_val(t.align), twips(t.x));
        }
        out.push_str("</w:tabs>");
    }
    if p.list.is_none() && (p.indent > 0 || p.left > 0.0 || p.first > 0.0 || p.right > 0.0) {
        out.push_str("<w:ind");
        let left = p.indent as u32 * LEVEL + twips(p.left);
        if left > 0 {
            let _ = write!(out, " w:left=\"{left}\"");
        }
        if p.right > 0.0 {
            let _ = write!(out, " w:right=\"{}\"", twips(p.right));
        }
        if p.first > 0.0 {
            let _ = write!(out, " w:firstLine=\"{}\"", twips(p.first));
        }
        out.push_str("/>");
    }
    let jc = match p.align {
        Align::Left => None,
        Align::Center => Some("center"),
        Align::Right => Some("right"),
        Align::Justify => Some("both"),
    };
    if let Some(jc) = jc {
        let _ = write!(out, "<w:jc w:val=\"{jc}\"/>");
    }
    out.push_str("</w:pPr>");
    let mut i = 0;
    while i < p.spans.len() {
        match &p.spans[i].link {
            Some(url) => {
                let n = links.iter().position(|l| l == url).unwrap_or_else(|| {
                    links.push(url.clone());
                    links.len() - 1
                });
                let _ = write!(out, "<w:hyperlink r:id=\"rIdL{}\">", n + 1);
                while i < p.spans.len() && p.spans[i].link.as_ref() == Some(url) {
                    run(out, &p.spans[i]);
                    i += 1;
                }
                out.push_str("</w:hyperlink>");
            }
            None => {
                run(out, &p.spans[i]);
                i += 1;
            }
        }
    }
    out.push_str("</w:p>");
}

/// A header (`hdr`) or footer (`ftr`) part: one paragraph, its three
/// parts a tab apart at a centre and a right stop, `{page}` and `{pages}`
/// as Word's PAGE and NUMPAGES fields.
fn header_part(tag: &str, parts: &[String; 3]) -> String {
    let mut p = format!(
        "<w:p><w:pPr><w:tabs><w:tab w:val=\"center\" w:pos=\"{}\"/><w:tab w:val=\"right\" w:pos=\"{TEXT_WIDTH}\"/></w:tabs></w:pPr>",
        TEXT_WIDTH / 2
    );
    for (k, part) in parts.iter().enumerate() {
        if k > 0 {
            p.push_str("<w:r><w:tab/></w:r>");
        }
        for bit in crate::bits(part) {
            match bit {
                crate::Bit::Text(t) => {
                    let _ = write!(p, "<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(t));
                }
                crate::Bit::Page => p.push_str("<w:fldSimple w:instr=\" PAGE \"><w:r><w:t>1</w:t></w:r></w:fldSimple>"),
                crate::Bit::Pages => p.push_str("<w:fldSimple w:instr=\" NUMPAGES \"><w:r><w:t>1</w:t></w:r></w:fldSimple>"),
            }
        }
    }
    p.push_str("</w:p>");
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:{tag} xmlns:w=\"{W}\" xmlns:r=\"{R}\">{p}</w:{tag}>")
}

/// A header's or footer's part read: its text (paragraphs a space apart,
/// tabs as tabs, PAGE and NUMPAGES fields, simple or complex, as `{page}`
/// and `{pages}`) and its first paragraph's alignment.
fn header_text(xml: &str) -> (String, Align) {
    let token = |instr: &str| match instr.split_whitespace().next().map(str::to_ascii_uppercase).as_deref() {
        Some("PAGE") => "{page}",
        Some("NUMPAGES" | "SECTIONPAGES") => "{pages}",
        _ => "",
    };
    let (mut out, mut align) = (String::new(), None);
    let (mut in_ppr, mut in_text, mut in_instr, mut simple) = (false, false, false, false);
    // A complex field: 1 its instruction, 2 its result (left out).
    let (mut field, mut instr) = (0, String::new());
    for e in events(xml) {
        match e {
            X::Start(n, a) => match n.as_str() {
                "p" if !out.is_empty() => out.push(' '),
                "pPr" => in_ppr = true,
                "jc" if in_ppr && align.is_none() => {
                    align = Some(match attr(&a, "val") {
                        Some("center") => Align::Center,
                        Some("right" | "end") => Align::Right,
                        _ => Align::Left,
                    })
                }
                "tab" if !in_ppr && field != 2 && !simple => out.push('\t'),
                "t" => in_text = true,
                "instrText" => in_instr = true,
                "fldSimple" => {
                    out.push_str(token(attr(&a, "instr").unwrap_or("")));
                    simple = true;
                }
                "fldChar" => match attr(&a, "fldCharType") {
                    Some("begin") => (field, instr) = (1, String::new()),
                    Some("separate") => {
                        out.push_str(token(&instr));
                        field = 2;
                    }
                    Some("end") => {
                        if field == 1 {
                            out.push_str(token(&instr));
                        }
                        field = 0;
                    }
                    _ => {}
                },
                _ => {}
            },
            X::End(n) => match n.as_str() {
                "pPr" => in_ppr = false,
                "t" => in_text = false,
                "instrText" => in_instr = false,
                "fldSimple" => simple = false,
                _ => {}
            },
            X::Text(t) if in_instr => instr.push_str(&t),
            X::Text(t) if in_text && field == 0 && !simple => out.push_str(&t),
            X::Text(_) => {}
        }
    }
    (out, align.unwrap_or(Align::Left))
}

/// Points in twentieths (twips).
fn twips(points: f32) -> u32 {
    (points.max(0.0) * 20.0).round() as u32
}

fn tab_val(a: TabAlign) -> &'static str {
    match a {
        TabAlign::Left => "left",
        TabAlign::Center => "center",
        TabAlign::Right => "right",
        TabAlign::Decimal => "decimal",
    }
}

fn table(out: &mut String, t: &Table, links: &mut Vec<String>, numbered_list: u32) {
    let widths = t.column_widths(TEXT_WIDTH);
    out.push_str("<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr><w:tblGrid>");
    for w in &widths {
        let _ = write!(out, "<w:gridCol w:w=\"{w}\"/>");
    }
    out.push_str("</w:tblGrid>");
    for (r, row) in t.rows.iter().enumerate() {
        out.push_str("<w:tr>");
        if r == 0 && t.header {
            out.push_str("<w:trPr><w:tblHeader/></w:trPr>");
        }
        for (c, w) in widths.iter().enumerate() {
            let _ = write!(out, "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/></w:tcPr>");
            match row.get(c).filter(|cell| !cell.is_empty()) {
                Some(cell) => cell.iter().for_each(|p| paragraph(out, p, links, numbered_list)),
                // (A cell holds a paragraph at least.)
                None => out.push_str("<w:p/>"),
            }
            out.push_str("</w:tc>");
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
}

fn run(out: &mut String, s: &Span) {
    out.push_str("<w:r><w:rPr>");
    // (In the order Word's schema has them.)
    if s.link.is_some() {
        out.push_str("<w:rStyle w:val=\"Hyperlink\"/>");
    }
    let font = s.font.as_deref().or(s.monospace.then_some(MONO));
    if let Some(f) = font {
        let f = esc(f);
        let _ = write!(out, "<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\" w:eastAsia=\"{f}\"/>");
    }
    if s.bold {
        out.push_str("<w:b/>");
    }
    if s.italic {
        out.push_str("<w:i/>");
    }
    if s.strike {
        out.push_str("<w:strike/>");
    }
    if let Some([r, g, b]) = s.color {
        let _ = write!(out, "<w:color w:val=\"{r:02X}{g:02X}{b:02X}\"/>");
    }
    if let Some(size) = s.size {
        let _ = write!(out, "<w:sz w:val=\"{}\"/>", (size * 2.0).round() as u32);
    }
    if s.underline {
        out.push_str("<w:u w:val=\"single\"/>");
    }
    out.push_str("</w:rPr>");
    for (k, piece) in s.text.split('\t').enumerate() {
        if k > 0 {
            out.push_str("<w:tab/>");
        }
        if !piece.is_empty() {
            let _ = write!(out, "<w:t xml:space=\"preserve\">{}</w:t>", esc(piece));
        }
    }
    out.push_str("</w:r>");
}

fn styles(font: Option<&str>) -> String {
    let style = |id: &str, name: &str, ppr: &str, rpr: &str| {
        format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr>{ppr}</w:pPr><w:rPr>{rpr}</w:rPr></w:style>"
        )
    };
    let heading = |id: &str, name: &str, level: u8, size: u32, before: u32| {
        style(id, name, &format!("<w:keepNext/><w:spacing w:before=\"{before}\" w:after=\"80\"/><w:outlineLvl w:val=\"{level}\"/>"), &format!("<w:b/><w:sz w:val=\"{size}\"/>"))
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"{W}\">\
         <w:docDefaults><w:rPrDefault><w:rPr>{}<w:sz w:val=\"24\"/></w:rPr></w:rPrDefault></w:docDefaults>\
         <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>\
         <w:style w:type=\"character\" w:styleId=\"Hyperlink\"><w:name w:val=\"Hyperlink\"/><w:rPr><w:color w:val=\"0563C1\"/><w:u w:val=\"single\"/></w:rPr></w:style>\
         <w:style w:type=\"table\" w:styleId=\"TableGrid\"><w:name w:val=\"Table Grid\"/><w:tblPr><w:tblBorders>\
         <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
         <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
         <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
         </w:tblBorders><w:tblCellMar><w:left w:w=\"108\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>\
         {}{}{}{}{}{}</w:styles>",
        font.map(|f| {
            let f = esc(f);
            format!("<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\" w:eastAsia=\"{f}\"/>")
        })
        .unwrap_or_default(),
        style("Title", "Title", "<w:spacing w:before=\"120\" w:after=\"200\"/>", "<w:b/><w:sz w:val=\"56\"/>"),
        heading("Heading1", "heading 1", 0, 44, 280),
        heading("Heading2", "heading 2", 1, 36, 240),
        heading("Heading3", "heading 3", 2, 28, 200),
        style("Quote", "Quote", "<w:spacing w:before=\"80\" w:after=\"80\"/><w:ind w:left=\"720\" w:right=\"720\"/>", "<w:i/>"),
        style("SourceCode", "Source Code", "", &format!("<w:rFonts w:ascii=\"{MONO}\" w:hAnsi=\"{MONO}\" w:cs=\"{MONO}\"/>")),
    )
}

/// Bullets (numbering 1), and `lists` numbered lists each counting from 1.
fn numbering(lists: u32) -> String {
    let levels = |bullet: bool| {
        (0..9)
            .map(|l| {
                let (fmt, text) = if bullet { ("bullet", ["•", "◦", "▪"][l % 3].to_owned()) } else { ("decimal", format!("%{}.", l + 1)) };
                format!(
                    "<w:lvl w:ilvl=\"{l}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/><w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
                    LEVEL * (l as u32 + 1)
                )
            })
            .collect::<String>()
    };
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering xmlns:w=\"{W}\">\
         <w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"hybridMultilevel\"/>{}</w:abstractNum>\
         <w:abstractNum w:abstractNumId=\"1\"><w:multiLevelType w:val=\"hybridMultilevel\"/>{}</w:abstractNum>\
         <w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>",
        levels(true),
        levels(false)
    );
    for k in 1..=lists {
        let restart: String = (0..9).map(|l| format!("<w:lvlOverride w:ilvl=\"{l}\"><w:startOverride w:val=\"1\"/></w:lvlOverride>")).collect();
        let _ = write!(out, "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"1\"/>{restart}</w:num>", k + 1);
    }
    out.push_str("</w:numbering>");
    out
}

pub fn read(bytes: &[u8]) -> Result<Document, String> {
    let bad = || "This document can't be read.".to_string();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| bad())?;
    let main = zip_text(&mut z, "word/document.xml").ok_or_else(bad)?;
    // Links by relationship id, and every part's target.
    let mut links: HashMap<String, String> = HashMap::new();
    let mut targets: HashMap<String, String> = HashMap::new();
    for e in events(&zip_text(&mut z, "word/_rels/document.xml.rels").unwrap_or_default()) {
        if let X::Start(n, a) = e
            && n == "Relationship"
            && let (Some(id), Some(target)) = (attr(&a, "Id"), attr(&a, "Target"))
        {
            if attr(&a, "Type").is_some_and(|t| t.ends_with("/hyperlink")) {
                links.insert(id.to_owned(), target.to_owned());
            }
            targets.insert(id.to_owned(), target.to_owned());
        }
    }
    // The section's own header and footer (the default ones).
    let mut header_footer = [None, None];
    for e in events(&main) {
        if let X::Start(n, a) = e
            && (n == "headerReference" || n == "footerReference")
            && attr(&a, "type").is_none_or(|t| t == "default")
            && let Some(target) = attr(&a, "id").and_then(|id| targets.get(id))
        {
            let path = format!("word/{}", target.trim_start_matches('/').trim_start_matches("word/"));
            header_footer[usize::from(n == "footerReference")] = zip_text(&mut z, &path).map(|xml| header_text(&xml));
        }
    }
    // Paragraph styles' names by id, and the document's font.
    let mut names: HashMap<String, String> = HashMap::new();
    let mut style: Option<String> = None;
    let mut defaults = false;
    let mut font: Option<String> = None;
    for e in events(&zip_text(&mut z, "word/styles.xml").unwrap_or_default()) {
        match e {
            X::Start(n, _) if n == "docDefaults" => defaults = true,
            X::End(n) if n == "docDefaults" => defaults = false,
            X::Start(n, a) if n == "rFonts" && defaults => font = attr(&a, "ascii").map(str::to_owned),
            X::Start(n, a) if n == "style" => style = attr(&a, "styleId").map(str::to_owned),
            X::Start(n, a) if n == "name" => {
                if let (Some(id), Some(name)) = (style.as_ref(), attr(&a, "val")) {
                    names.insert(id.clone(), name.to_owned());
                }
            }
            _ => {}
        }
    }
    // Which (numbering, level) is a bulleted one.
    let mut abstract_bullets: HashMap<(String, u8), bool> = HashMap::new();
    let mut nums: HashMap<String, String> = HashMap::new();
    let (mut abs, mut lvl, mut num): (Option<String>, u8, Option<String>) = (None, 0, None);
    for e in events(&zip_text(&mut z, "word/numbering.xml").unwrap_or_default()) {
        match e {
            X::Start(n, a) => match n.as_str() {
                "abstractNum" => abs = attr(&a, "abstractNumId").map(str::to_owned),
                "lvl" => lvl = attr(&a, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0),
                "numFmt" => {
                    if let Some(id) = &abs {
                        abstract_bullets.insert((id.clone(), lvl), matches!(attr(&a, "val"), Some("bullet" | "none")));
                    }
                }
                "num" => num = attr(&a, "numId").map(str::to_owned),
                "abstractNumId" => {
                    if let (Some(n), Some(v)) = (&num, attr(&a, "val")) {
                        nums.insert(n.clone(), v.to_owned());
                    }
                }
                _ => {}
            },
            X::End(n) if n == "abstractNum" => abs = None,
            X::End(n) if n == "num" => num = None,
            _ => {}
        }
    }
    let bulleted = |num: &str, level: u8| nums.get(num).and_then(|a| abstract_bullets.get(&(a.clone(), level))).copied().unwrap_or(true);

    let mut doc = Document { font, ..Document::default() };
    let mut para = Paragraph::default();
    let mut span = Span::default();
    let (mut in_ppr, mut in_rpr, mut in_run, mut in_text) = (false, false, false, false);
    let (mut num_id, mut ilvl): (Option<String>, u8) = (None, 0);
    let mut link: Option<String> = None;
    // The table being read (one in a cell comes in as its paragraphs).
    let mut table_depth = 0;
    let mut table = Table::default();
    for e in events(&main) {
        match e {
            X::Start(n, a) => match n.as_str() {
                "p" => {
                    para = Paragraph::default();
                    (num_id, ilvl) = (None, 0);
                }
                "pPr" => in_ppr = true,
                "rPr" if in_run => in_rpr = true,
                "pStyle" if in_ppr => {
                    let id = attr(&a, "val").unwrap_or("");
                    let name = names.get(id).map_or(id, String::as_str);
                    para.kind = kind_of(name);
                    let lower = name.to_ascii_lowercase();
                    if lower.starts_with("list bullet") {
                        para.list = Some(List { numbered: false, level: 0 });
                    } else if lower.starts_with("list number") {
                        para.list = Some(List { numbered: true, level: 0 });
                    }
                }
                "numId" if in_ppr => num_id = attr(&a, "val").filter(|v| *v != "0").map(str::to_owned),
                "ilvl" if in_ppr => ilvl = attr(&a, "val").and_then(|v| v.parse().ok()).unwrap_or(0),
                "ind" if in_ppr => {
                    // Whole levels, and the rest in points.
                    let twip = |name: &str| attr(&a, name).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0).max(0.0) as u32;
                    let left = twip("left").max(twip("start"));
                    para.indent = (left / LEVEL).min(8) as u8;
                    para.left = (left - para.indent as u32 * LEVEL) as f32 / 20.0;
                    para.right = twip("right").max(twip("end")) as f32 / 20.0;
                    para.first = twip("firstLine") as f32 / 20.0;
                }
                "tab" if in_ppr => {
                    let align = match attr(&a, "val") {
                        Some("center") => Some(TabAlign::Center),
                        Some("right" | "end") => Some(TabAlign::Right),
                        Some("decimal") => Some(TabAlign::Decimal),
                        Some("left" | "start") => Some(TabAlign::Left),
                        // (A cleared one, a bar, a list's.)
                        _ => None,
                    };
                    if let (Some(align), Some(pos)) = (align, attr(&a, "pos").and_then(|v| v.parse::<f64>().ok())) {
                        para.tabs.push(Tab { x: (pos.max(0.0) / 20.0) as f32, align });
                        para.tabs.sort_by(|a, b| a.x.total_cmp(&b.x));
                    }
                }
                "jc" if in_ppr => {
                    para.align = match attr(&a, "val") {
                        Some("center") => Align::Center,
                        Some("right" | "end") => Align::Right,
                        Some("both" | "distribute") => Align::Justify,
                        _ => Align::Left,
                    }
                }
                "hyperlink" => link = attr(&a, "id").and_then(|id| links.get(id)).cloned(),
                "r" => {
                    in_run = true;
                    span = Span { link: link.clone(), ..Span::default() };
                }
                "b" if in_rpr => span.bold = on(&a),
                "i" if in_rpr => span.italic = on(&a),
                "strike" | "dstrike" if in_rpr => span.strike = on(&a),
                "u" if in_rpr => span.underline = !matches!(attr(&a, "val"), Some("none")),
                "color" if in_rpr => span.color = attr(&a, "val").and_then(hex_color),
                "sz" if in_rpr => span.size = attr(&a, "val").and_then(|v| v.parse::<f32>().ok()).map(|h| h / 2.0),
                "rFonts" if in_rpr => {
                    if let Some(f) = attr(&a, "ascii").or(attr(&a, "hAnsi")) {
                        if is_mono(f) {
                            span.monospace = true;
                        } else {
                            span.font = Some(f.to_owned());
                        }
                    }
                }
                "t" if in_run => in_text = true,
                "tab" if in_run && !in_ppr => para.spans.push(span.with_text("\t")),
                "br" | "cr" if in_run => para.spans.push(span.with_text("\n")),
                "noBreakHyphen" if in_run => para.spans.push(span.with_text("-")),
                "tbl" => {
                    table_depth += 1;
                    if table_depth == 1 {
                        table = Table::default();
                    }
                }
                "tr" if table_depth == 1 => table.rows.push(Vec::new()),
                "tc" if table_depth == 1 => {
                    if let Some(row) = table.rows.last_mut() {
                        row.push(Vec::new());
                    }
                }
                "tblHeader" if table_depth == 1 && table.rows.len() == 1 => table.header = on(&a),
                "gridCol" if table_depth == 1 => table.widths.push(attr(&a, "w").and_then(|v| v.parse().ok()).unwrap_or(0.0)),
                _ => {}
            },
            X::End(n) => match n.as_str() {
                "pPr" => {
                    in_ppr = false;
                    if let Some(id) = &num_id {
                        para.list = Some(List { numbered: !bulleted(id, ilvl), level: ilvl });
                    }
                    if para.list.is_some() {
                        (para.indent, para.left, para.first, para.right) = (0, 0.0, 0.0, 0.0);
                    }
                }
                "rPr" => in_rpr = false,
                "r" => in_run = false,
                "t" => in_text = false,
                "hyperlink" => link = None,
                "p" => {
                    let p = std::mem::take(&mut para);
                    match table.rows.last_mut().and_then(|r| r.last_mut()) {
                        Some(cell) if table_depth > 0 => cell.push(p),
                        _ => doc.paragraphs.push(p),
                    }
                }
                "tbl" => {
                    if table_depth == 1 {
                        doc.paragraphs.push(Paragraph { table: Some(std::mem::take(&mut table)), ..Paragraph::default() });
                    }
                    table_depth -= 1;
                }
                _ => {}
            },
            X::Text(t) if in_text => para.spans.push(span.with_text(t)),
            X::Text(_) => {}
        }
    }
    if let Some((text, align)) = &header_footer[0] {
        doc.header = crate::parts(text, *align);
    }
    if let Some((text, align)) = &header_footer[1] {
        doc.footer = crate::parts(text, *align);
    }
    doc.tidy();
    Ok(doc)
}

/// A toggle property's value: on unless it says off.
fn on(a: &[(String, String)]) -> bool {
    !matches!(attr(a, "val"), Some("0" | "false" | "off" | "none"))
}
