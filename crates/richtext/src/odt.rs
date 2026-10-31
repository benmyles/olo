//! OpenDocument text (.odt): `content.xml`, its automatic styles and
//! lists, with the named paragraph styles in `styles.xml`.

use std::{collections::HashMap, fmt::Write as _, io::Write as _};

use crate::{
    Align, Document, Kind, List, Paragraph, Span, Tab, TabAlign, Table,
    xml::{X, attr, esc, events, hex_color, is_mono, zip_text},
};

const NS: &str = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
    xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" \
    xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" \
    xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" \
    xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
    xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\" \
    xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" \
    office:version=\"1.3\"";
const MONO: &str = "Liberation Mono";

fn style_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Body => "Text_20_body",
        Kind::Title => "Title",
        Kind::Heading1 => "Heading_20_1",
        Kind::Heading2 => "Heading_20_2",
        Kind::Heading3 => "Heading_20_3",
        Kind::Quote => "Quotations",
        Kind::Code => "Preformatted_20_Text",
    }
}

fn kind_of(name: &str) -> Option<Kind> {
    let n = name.to_ascii_lowercase().replace("_20_", " ").replace('_', " ");
    Some(match n.as_str() {
        "title" => Kind::Title,
        "heading 1" => Kind::Heading1,
        "heading 2" => Kind::Heading2,
        _ if n.starts_with("heading ") => Kind::Heading3,
        "quotations" | "quote" => Kind::Quote,
        _ if n.contains("preformatted") || n.contains("code") => Kind::Code,
        "text body" | "standard" | "body text" | "default paragraph style" => Kind::Body,
        _ => return None,
    })
}

/// A span's character properties, as an automatic style's.
fn text_props(s: &Span) -> String {
    let mut p = String::new();
    if s.bold {
        p.push_str(" fo:font-weight=\"bold\" style:font-weight-asian=\"bold\" style:font-weight-complex=\"bold\"");
    }
    if s.italic {
        p.push_str(" fo:font-style=\"italic\" style:font-style-asian=\"italic\" style:font-style-complex=\"italic\"");
    }
    if s.underline {
        p.push_str(" style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"");
    }
    if s.strike {
        p.push_str(" style:text-line-through-style=\"solid\"");
    }
    if let Some([r, g, b]) = s.color {
        let _ = write!(p, " fo:color=\"#{r:02x}{g:02x}{b:02x}\"");
    }
    if let Some(size) = s.size {
        let _ = write!(p, " fo:font-size=\"{size}pt\"");
    }
    if let Some(f) = s.font.as_deref().or(s.monospace.then_some(MONO)) {
        let _ = write!(p, " style:font-name=\"{0}\" fo:font-family=\"'{0}'\"", esc(f));
    }
    p
}

/// The automatic styles the body uses, gathered as it is written: text
/// ones by properties, paragraph ones by (kind, alignment, indent); and
/// the fonts named.
#[derive(Default)]
struct Auto {
    text: Vec<String>,
    para: Vec<ParaStyle>,
    fonts: Vec<String>,
    tables: usize,
    /// Styles for the columns of tables with widths of their own.
    columns: String,
}

/// An automatic paragraph style: what of a paragraph isn't its kind's.
#[derive(Clone, PartialEq)]
struct ParaStyle {
    kind: Kind,
    align: Align,
    /// Points from the text's left edge (levels and the ruler's), the
    /// first line's further in, from the right edge.
    left: f32,
    first: f32,
    right: f32,
    tabs: Vec<Tab>,
}

pub fn write(doc: &Document) -> Result<Vec<u8>, String> {
    let mut auto_styles = Auto::default();
    let mut body = String::new();
    // Lists open: numbered or not.
    let mut lists: Vec<bool> = Vec::new();
    let close = |body: &mut String, lists: &mut Vec<bool>, to: usize| {
        while lists.len() > to {
            lists.pop();
            body.push_str("</text:list-item></text:list>");
        }
    };
    for p in &doc.paragraphs {
        match p.list {
            Some(l) => {
                let depth = l.level as usize + 1;
                close(&mut body, &mut lists, depth);
                // A list of the other kind at this depth: a new one.
                if lists.len() == depth && lists.last() != Some(&l.numbered) {
                    close(&mut body, &mut lists, depth - 1);
                }
                if lists.len() == depth {
                    body.push_str("</text:list-item>");
                }
                while lists.len() < depth {
                    let style = if l.numbered { "LNumbers" } else { "LBullets" };
                    let _ = write!(body, "<text:list text:style-name=\"{style}\">");
                    lists.push(l.numbered);
                    if lists.len() < depth {
                        body.push_str("<text:list-item>");
                    }
                }
                body.push_str("<text:list-item>");
            }
            None => close(&mut body, &mut lists, 0),
        }
        match &p.table {
            Some(t) => table(&mut body, t, &mut auto_styles),
            None => paragraph(&mut body, p, &mut auto_styles),
        }
    }
    close(&mut body, &mut lists, 0);

    let Auto { text: text_styles, para: para_styles, mut fonts, tables, columns } = auto_styles;
    let mut auto = String::new();
    if tables > 0 {
        auto.push_str(
            "<style:style style:name=\"Tbl\" style:family=\"table\"><style:table-properties style:width=\"6.5in\" table:align=\"margins\" table:border-model=\"collapsing\"/></style:style>\
             <style:style style:name=\"TblCell\" style:family=\"table-cell\"><style:table-cell-properties fo:padding=\"0.04in\" fo:border=\"0.5pt solid #000000\"/></style:style>",
        );
        auto.push_str(&columns);
    }
    for (i, props) in text_styles.iter().enumerate() {
        let _ = write!(auto, "<style:style style:name=\"T{}\" style:family=\"text\"><style:text-properties{props}/></style:style>", i + 1);
    }
    for (i, ps) in para_styles.iter().enumerate() {
        let a = match ps.align {
            Align::Left => "start",
            Align::Center => "center",
            Align::Right => "end",
            Align::Justify => "justify",
        };
        let _ = write!(
            auto,
            "<style:style style:name=\"P{}\" style:family=\"paragraph\" style:parent-style-name=\"{}\"><style:paragraph-properties fo:text-align=\"{a}\" fo:margin-left=\"{}pt\"",
            i + 1,
            style_name(ps.kind),
            ps.left
        );
        if ps.right > 0.0 {
            let _ = write!(auto, " fo:margin-right=\"{}pt\"", ps.right);
        }
        if ps.first > 0.0 {
            let _ = write!(auto, " fo:text-indent=\"{}pt\"", ps.first);
        }
        if ps.tabs.is_empty() {
            auto.push_str("/>");
        } else {
            // (Measured from the paragraph's indent, as LibreOffice has
            // them.)
            auto.push_str("><style:tab-stops>");
            for t in ps.tabs.iter().filter(|t| t.x > ps.left) {
                let kind = match t.align {
                    TabAlign::Left => "left\"",
                    TabAlign::Center => "center\"",
                    TabAlign::Right => "right\"",
                    TabAlign::Decimal => "char\" style:char=\".\"",
                };
                let _ = write!(auto, "<style:tab-stop style:position=\"{}pt\" style:type=\"{kind}/>", t.x - ps.left);
            }
            auto.push_str("</style:tab-stops></style:paragraph-properties>");
        }
        auto.push_str("</style:style>");
    }
    let level_styles = |bullet: bool| {
        (1..=10)
            .map(|l| {
                let ind = format!(
                    "<style:list-level-properties text:list-level-position-and-space-mode=\"label-alignment\"><style:list-level-label-alignment text:label-followed-by=\"listtab\" text:list-tab-stop-position=\"{0}in\" fo:text-indent=\"-0.25in\" fo:margin-left=\"{0}in\"/></style:list-level-properties>",
                    l as f32 * 0.5
                );
                if bullet {
                    format!("<text:list-level-style-bullet text:level=\"{l}\" text:bullet-char=\"{}\">{ind}</text:list-level-style-bullet>", ["•", "◦", "▪"][(l - 1) % 3])
                } else {
                    format!("<text:list-level-style-number text:level=\"{l}\" style:num-suffix=\".\" style:num-format=\"1\">{ind}</text:list-level-style-number>")
                }
            })
            .collect::<String>()
    };
    let _ = write!(
        auto,
        "<text:list-style style:name=\"LBullets\">{}</text:list-style><text:list-style style:name=\"LNumbers\">{}</text:list-style>",
        level_styles(true),
        level_styles(false)
    );
    if let Some(f) = &doc.font
        && !fonts.contains(f)
    {
        fonts.push(f.clone());
    }
    let font_decls: String = fonts.iter().map(|f| format!("<style:font-face style:name=\"{0}\" svg:font-family=\"'{0}'\"/>", esc(f))).collect();
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-content {NS}><office:font-face-decls>{font_decls}</office:font-face-decls><office:automatic-styles>{auto}</office:automatic-styles><office:body><office:text>{body}</office:text></office:body></office:document-content>"
    );
    let named = |name: &str, display: &str, props: &str, ppr: &str| {
        format!(
            "<style:style style:name=\"{name}\" style:display-name=\"{display}\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"text\"><style:paragraph-properties{ppr}/><style:text-properties{props}/></style:style>"
        )
    };
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-styles {NS}><office:font-face-decls>{font_decls}</office:font-face-decls><office:styles>\
         <style:default-style style:family=\"paragraph\"><style:text-properties fo:font-size=\"12pt\"{0}/></style:default-style>\
         <style:style style:name=\"Standard\" style:family=\"paragraph\" style:class=\"text\"><style:text-properties fo:font-size=\"12pt\"{0}/></style:style>\
         {1}{2}{3}{4}{5}{6}{7}</office:styles></office:document-styles>",
        doc.font.as_deref().map(|f| format!(" style:font-name=\"{0}\" fo:font-family=\"'{0}'\"", esc(f))).unwrap_or_default(),
        named("Text_20_body", "Text body", "", ""),
        named("Title", "Title", " fo:font-size=\"28pt\" fo:font-weight=\"bold\"", " fo:margin-top=\"0.08in\" fo:margin-bottom=\"0.14in\""),
        named("Heading_20_1", "Heading 1", " fo:font-size=\"22pt\" fo:font-weight=\"bold\"", " fo:margin-top=\"0.19in\" fo:margin-bottom=\"0.06in\""),
        named("Heading_20_2", "Heading 2", " fo:font-size=\"18pt\" fo:font-weight=\"bold\"", " fo:margin-top=\"0.17in\" fo:margin-bottom=\"0.06in\""),
        named("Heading_20_3", "Heading 3", " fo:font-size=\"14pt\" fo:font-weight=\"bold\"", " fo:margin-top=\"0.14in\" fo:margin-bottom=\"0.03in\""),
        named("Quotations", "Quotations", " fo:font-style=\"italic\"", " fo:margin-left=\"0.5in\" fo:margin-right=\"0.5in\""),
        named("Preformatted_20_Text", "Preformatted Text", &format!(" style:font-name=\"{MONO}\" fo:font-family=\"'{MONO}'\""), ""),
    );
    let manifest = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.3\">\
        <manifest:file-entry manifest:full-path=\"/\" manifest:media-type=\"application/vnd.oasis.opendocument.text\"/>\
        <manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/>\
        <manifest:file-entry manifest:full-path=\"styles.xml\" manifest:media-type=\"text/xml\"/>\
        </manifest:manifest>";
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    // The type first, uncompressed, as the format asks.
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file("mimetype", stored).map_err(|e| e.to_string())?;
    z.write_all(b"application/vnd.oasis.opendocument.text").map_err(|e| e.to_string())?;
    for (name, data) in [("content.xml", content.as_str()), ("styles.xml", styles.as_str()), ("META-INF/manifest.xml", manifest)] {
        z.start_file(name, deflated).map_err(|e| e.to_string())?;
        z.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(z.finish().map_err(|e| e.to_string())?.into_inner())
}

fn paragraph(body: &mut String, p: &Paragraph, auto: &mut Auto) {
    let list = p.list.is_some();
    let key = ParaStyle {
        kind: p.kind,
        align: p.align,
        left: if list { 0.0 } else { p.indent as f32 * 36.0 + p.left.max(0.0) },
        first: if list { 0.0 } else { p.first.max(0.0) },
        right: if list { 0.0 } else { p.right.max(0.0) },
        tabs: p.tabs.clone(),
    };
    let style = if p.align == Align::Left && key.left == 0.0 && key.first == 0.0 && key.right == 0.0 && key.tabs.is_empty() {
        style_name(p.kind).to_owned()
    } else {
        let n = auto.para.iter().position(|k| *k == key).unwrap_or_else(|| {
            auto.para.push(key);
            auto.para.len() - 1
        });
        format!("P{}", n + 1)
    };
    let (tag, level) = match p.kind {
        Kind::Heading1 => ("text:h", Some(1)),
        Kind::Heading2 => ("text:h", Some(2)),
        Kind::Heading3 => ("text:h", Some(3)),
        _ => ("text:p", None),
    };
    let _ = write!(body, "<{tag} text:style-name=\"{style}\"");
    if let Some(l) = level {
        let _ = write!(body, " text:outline-level=\"{l}\"");
    }
    body.push('>');
    let mut i = 0;
    while i < p.spans.len() {
        let url = p.spans[i].link.clone();
        if let Some(u) = &url {
            let _ = write!(body, "<text:a xlink:type=\"simple\" xlink:href=\"{}\">", esc(u));
        }
        while i < p.spans.len() && p.spans[i].link == url {
            let s = &p.spans[i];
            if let Some(f) = s.font.as_deref().or(s.monospace.then_some(MONO))
                && !auto.fonts.iter().any(|x| x == f)
            {
                auto.fonts.push(f.to_owned());
            }
            let props = text_props(s);
            if props.is_empty() {
                body.push_str(&text(&s.text));
            } else {
                let n = auto.text.iter().position(|x| *x == props).unwrap_or_else(|| {
                    auto.text.push(props);
                    auto.text.len() - 1
                });
                let _ = write!(body, "<text:span text:style-name=\"T{}\">{}</text:span>", n + 1, text(&s.text));
            }
            i += 1;
        }
        if url.is_some() {
            body.push_str("</text:a>");
        }
    }
    let _ = write!(body, "</{tag}>");
}

/// A table, ruled, across the page; its header row repeated on each.
fn table(body: &mut String, t: &Table, auto: &mut Auto) {
    auto.tables += 1;
    let columns = t.columns().max(1);
    let _ = write!(body, "<table:table table:name=\"Table{}\" table:style-name=\"Tbl\">", auto.tables);
    if t.widths.is_empty() {
        let _ = write!(body, "<table:table-column table:number-columns-repeated=\"{columns}\"/>");
    } else {
        // Each column a style of its own: its width, of the page's 6.5in
        // and as a share (out of 10000) of the table's.
        for (c, w) in t.column_widths(10000).iter().enumerate() {
            let name = format!("Tbl{}.C{}", auto.tables, c + 1);
            let _ = write!(
                auto.columns,
                "<style:style style:name=\"{name}\" style:family=\"table-column\"><style:table-column-properties style:column-width=\"{}in\" style:rel-column-width=\"{w}*\"/></style:style>",
                *w as f32 * 6.5 / 10000.0
            );
            let _ = write!(body, "<table:table-column table:style-name=\"{name}\"/>");
        }
    }
    for (r, row) in t.rows.iter().enumerate() {
        let header = r == 0 && t.header;
        if header {
            body.push_str("<table:table-header-rows>");
        }
        body.push_str("<table:table-row>");
        for c in 0..columns {
            body.push_str("<table:table-cell table:style-name=\"TblCell\" office:value-type=\"string\">");
            match row.get(c).filter(|cell| !cell.is_empty()) {
                Some(cell) => cell.iter().for_each(|p| paragraph(body, p, auto)),
                None => body.push_str("<text:p/>"),
            }
            body.push_str("</table:table-cell>");
        }
        body.push_str("</table:table-row>");
        if header {
            body.push_str("</table:table-header-rows>");
        }
    }
    body.push_str("</table:table>");
}

/// Text as ODF keeps it: runs of spaces and tabs spelled out (XML would
/// fold them into one).
fn text(s: &str) -> String {
    let mut out = String::new();
    let mut spaces = 0;
    let flush = |out: &mut String, spaces: &mut usize, start: bool| {
        if *spaces == 0 {
            return;
        }
        // One plain space, unless it begins the text; the rest counted.
        let plain = if start { 0 } else { 1 };
        if plain == 1 {
            out.push(' ');
        }
        let rest = *spaces - plain;
        if rest == 1 {
            out.push_str("<text:s/>");
        } else if rest > 1 {
            let _ = write!(out, "<text:s text:c=\"{rest}\"/>");
        }
        *spaces = 0;
    };
    for c in s.chars() {
        match c {
            ' ' => spaces += 1,
            '\t' => {
                let start = out.is_empty();
                flush(&mut out, &mut spaces, start);
                out.push_str("<text:tab/>");
            }
            c => {
                let start = out.is_empty();
                flush(&mut out, &mut spaces, start);
                out.push_str(&esc(&c.to_string()));
            }
        }
    }
    let start = out.is_empty();
    // Trailing spaces: all spelled out (a plain one at the end would go).
    if spaces > 0 {
        if start {
            flush(&mut out, &mut spaces, true);
        } else {
            let _ = write!(out, "<text:s text:c=\"{spaces}\"/>");
        }
    }
    out
}

/// What a style says (only what it sets), and its parent.
#[derive(Default, Clone)]
struct Props {
    parent: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<[u8; 3]>,
    size: Option<f32>,
    font: Option<String>,
    align: Option<Align>,
    margin: Option<f32>,
    /// Inches: the first line's indent, the right margin.
    text_indent: Option<f32>,
    margin_right: Option<f32>,
    /// Tab stops (points from the indent).
    tabs: Option<Vec<Tab>>,
    /// A table column's width: relative (`rel-column-width`), else in
    /// inches.
    column: Option<f32>,
}

fn read_styles(xml: &str, into: &mut HashMap<String, Props>, lists: &mut HashMap<String, Vec<bool>>) {
    let mut cur: Option<String> = None;
    let mut list: Option<String> = None;
    for e in events(xml) {
        match e {
            X::Start(n, a) => match n.as_str() {
                "style" => {
                    let name = attr(&a, "name").unwrap_or("").to_owned();
                    into.insert(name.clone(), Props { parent: attr(&a, "parent-style-name").map(str::to_owned), ..Props::default() });
                    cur = Some(name);
                }
                "text-properties" => {
                    if let Some(p) = cur.as_ref().and_then(|c| into.get_mut(c)) {
                        if let Some(w) = attr(&a, "font-weight") {
                            p.bold = Some(w == "bold" || w.parse::<u32>().is_ok_and(|w| w >= 600));
                        }
                        if let Some(s) = attr(&a, "font-style") {
                            p.italic = Some(s == "italic" || s == "oblique");
                        }
                        if let Some(u) = attr(&a, "text-underline-style") {
                            p.underline = Some(u != "none");
                        }
                        if let Some(s) = attr(&a, "text-line-through-style") {
                            p.strike = Some(s != "none");
                        }
                        if let Some(c) = attr(&a, "color").and_then(hex_color) {
                            p.color = Some(c);
                        }
                        if let Some(s) = attr(&a, "font-size").and_then(|s| s.strip_suffix("pt")).and_then(|s| s.parse().ok()) {
                            p.size = Some(s);
                        }
                        if let Some(f) = attr(&a, "font-name").or(attr(&a, "font-family")) {
                            p.font = Some(f.trim_matches('\'').to_owned());
                        }
                    }
                }
                "paragraph-properties" => {
                    if let Some(p) = cur.as_ref().and_then(|c| into.get_mut(c)) {
                        p.align = attr(&a, "text-align").map(|v| match v {
                            "center" => Align::Center,
                            "end" | "right" => Align::Right,
                            "justify" => Align::Justify,
                            _ => Align::Left,
                        });
                        p.margin = attr(&a, "margin-left").and_then(inches);
                        p.text_indent = attr(&a, "text-indent").and_then(inches);
                        p.margin_right = attr(&a, "margin-right").and_then(inches);
                    }
                }
                "tab-stops" => {
                    if let Some(p) = cur.as_ref().and_then(|c| into.get_mut(c)) {
                        p.tabs = Some(Vec::new());
                    }
                }
                "tab-stop" => {
                    if let Some(tabs) = cur.as_ref().and_then(|c| into.get_mut(c)).and_then(|p| p.tabs.as_mut())
                        && let Some(at) = attr(&a, "position").and_then(inches)
                    {
                        let align = match attr(&a, "type") {
                            Some("center") => TabAlign::Center,
                            Some("right") => TabAlign::Right,
                            Some("char") => TabAlign::Decimal,
                            _ => TabAlign::Left,
                        };
                        tabs.push(Tab { x: at * 72.0, align });
                    }
                }
                "table-column-properties" => {
                    if let Some(p) = cur.as_ref().and_then(|c| into.get_mut(c)) {
                        p.column = attr(&a, "rel-column-width").and_then(|v| v.trim_end_matches('*').parse().ok()).or_else(|| attr(&a, "column-width").and_then(inches));
                    }
                }
                "list-style" => {
                    let name = attr(&a, "name").unwrap_or("").to_owned();
                    lists.insert(name.clone(), vec![false; 10]);
                    list = Some(name);
                }
                "list-level-style-number" | "list-level-style-bullet" => {
                    if let (Some(l), Some(level)) = (list.as_ref().and_then(|l| lists.get_mut(l)), attr(&a, "level").and_then(|v| v.parse::<usize>().ok()))
                        && (1..=10).contains(&level)
                    {
                        l[level - 1] = n == "list-level-style-number";
                    }
                }
                _ => {}
            },
            X::End(n) if n == "style" => cur = None,
            X::End(n) if n == "list-style" => list = None,
            _ => {}
        }
    }
}

/// A length in inches ("0.5in", "1.27cm", "36pt").
fn inches(v: &str) -> Option<f32> {
    let (num, unit) = v.split_at(v.find(|c: char| c.is_ascii_alphabetic())?);
    let n: f32 = num.parse().ok()?;
    Some(match unit {
        "in" => n,
        "cm" => n / 2.54,
        "mm" => n / 25.4,
        "pt" => n / 72.0,
        _ => return None,
    })
}

pub fn read(bytes: &[u8]) -> Result<Document, String> {
    let bad = || "This document can't be read.".to_string();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| bad())?;
    let content = zip_text(&mut z, "content.xml").ok_or_else(bad)?;
    let mut styles: HashMap<String, Props> = HashMap::new();
    let mut list_styles: HashMap<String, Vec<bool>> = HashMap::new();
    read_styles(&zip_text(&mut z, "styles.xml").unwrap_or_default(), &mut styles, &mut list_styles);
    read_styles(&content, &mut styles, &mut list_styles);
    // A style's property, looking up its parents.
    fn find<T>(styles: &HashMap<String, Props>, name: &str, f: &dyn Fn(&Props) -> Option<T>) -> Option<T> {
        let mut n = Some(name.to_owned());
        for _ in 0..8 {
            let p = styles.get(n.as_deref()?)?;
            if let Some(v) = f(p) {
                return Some(v);
            }
            n = p.parent.clone();
        }
        None
    }
    // A paragraph style's kind: its own name's, else its parent's.
    let kind = |name: &str| {
        let mut n = Some(name.to_owned());
        for _ in 0..8 {
            let Some(cur) = n else { break };
            if let Some(k) = kind_of(&cur) {
                return k;
            }
            n = styles.get(&cur).and_then(|p| p.parent.clone());
        }
        Kind::Body
    };

    let mut doc = Document { font: styles.get("Standard").and_then(|p| p.font.clone()), ..Document::default() };
    let mut para: Option<Paragraph> = None;
    // Spans' styles open (text:span nests), the link open.
    let mut spans: Vec<Span> = Vec::new();
    let mut link: Option<String> = None;
    // Lists open: each one's style (inherited by nested lists).
    let mut lists: Vec<Option<String>> = Vec::new();
    // The table being read (one in a cell comes in as its paragraphs).
    let mut table_depth = 0;
    let mut table = Table::default();
    let span_for = |name: &str, base: &Span| {
        let mut s = base.clone();
        if let Some(v) = find(&styles, name, &|p| p.bold) {
            s.bold = v;
        }
        if let Some(v) = find(&styles, name, &|p| p.italic) {
            s.italic = v;
        }
        if let Some(v) = find(&styles, name, &|p| p.underline) {
            s.underline = v;
        }
        if let Some(v) = find(&styles, name, &|p| p.strike) {
            s.strike = v;
        }
        if let Some(v) = find(&styles, name, &|p| p.color) {
            s.color = Some(v);
        }
        if let Some(v) = find(&styles, name, &|p| p.size) {
            s.size = Some(v);
        }
        if let Some(f) = find(&styles, name, &|p| p.font.clone()) {
            if is_mono(&f) {
                s.monospace = true;
            } else {
                s.font = Some(f);
            }
        }
        s
    };
    for e in events(&content) {
        match e {
            X::Start(n, a) => match n.as_str() {
                "list" => {
                    let style = attr(&a, "style-name").map(str::to_owned).or_else(|| lists.last().cloned().flatten());
                    lists.push(style);
                }
                "p" | "h" => {
                    let style = attr(&a, "style-name").unwrap_or("");
                    let mut p = Paragraph { kind: kind(style), ..Paragraph::default() };
                    if n == "h" {
                        p.kind = match attr(&a, "outline-level").and_then(|v| v.parse::<u8>().ok()).unwrap_or(1) {
                            1 => Kind::Heading1,
                            2 => Kind::Heading2,
                            _ => Kind::Heading3,
                        };
                    }
                    p.align = find(&styles, style, &|p| p.align).unwrap_or_default();
                    if let Some(depth) = lists.len().checked_sub(1) {
                        let numbered = lists.last().cloned().flatten().and_then(|s| list_styles.get(&s).and_then(|l| l.get(depth).copied())).unwrap_or(false);
                        p.list = Some(List { numbered, level: depth.min(8) as u8 });
                    } else if p.kind != Kind::Quote {
                        // Whole levels and the rest in points. (Margins
                        // the kind gives itself aren't indents.)
                        let pt = |f: &dyn Fn(&Props) -> Option<f32>| find(&styles, style, f).map_or(0.0, |v| (v * 72.0 * 100.0).round() / 100.0).max(0.0);
                        let m = pt(&|p| p.margin);
                        p.indent = ((m + 0.01) / 36.0).floor().min(8.0) as u8;
                        p.left = m - p.indent as f32 * 36.0;
                        p.first = pt(&|p| p.text_indent);
                        p.right = pt(&|p| p.margin_right);
                    }
                    // Tab stops, from the indent to from the text's edge.
                    if let Some(tabs) = find(&styles, style, &|p| p.tabs.clone()) {
                        let m = find(&styles, style, &|p| p.margin).unwrap_or(0.0) * 72.0;
                        p.tabs = tabs.into_iter().map(|t| Tab { x: ((t.x + m) * 100.0).round() / 100.0, ..t }).collect();
                        p.tabs.sort_by(|a, b| a.x.total_cmp(&b.x));
                    }
                    // The paragraph style's own character properties are
                    // its kind's, not spans'.
                    spans = vec![Span { link: link.clone(), ..Span::default() }];
                    para = Some(p);
                }
                "span" => {
                    let base = spans.last().cloned().unwrap_or_default();
                    spans.push(span_for(attr(&a, "style-name").unwrap_or(""), &base));
                }
                "a" => {
                    link = attr(&a, "href").map(str::to_owned);
                    let mut s = spans.last().cloned().unwrap_or_default();
                    s.link = link.clone();
                    spans.push(s);
                }
                "s" => {
                    let c: usize = attr(&a, "c").and_then(|v| v.parse().ok()).unwrap_or(1);
                    push(&mut para, &spans, &" ".repeat(c));
                }
                "tab" => push(&mut para, &spans, "\t"),
                "line-break" => push(&mut para, &spans, "\n"),
                "table" => {
                    table_depth += 1;
                    if table_depth == 1 {
                        table = Table::default();
                    }
                }
                "table-header-rows" if table_depth == 1 && table.rows.is_empty() => table.header = true,
                "table-column" if table_depth == 1 => {
                    let width = attr(&a, "style-name").and_then(|s| styles.get(s)).and_then(|p| p.column).unwrap_or(0.0);
                    let repeated: usize = attr(&a, "number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1);
                    table.widths.extend(std::iter::repeat_n(width, repeated.min(1024)));
                }
                "table-row" if table_depth == 1 => table.rows.push(Vec::new()),
                "table-cell" | "covered-table-cell" if table_depth == 1 => {
                    if let Some(row) = table.rows.last_mut() {
                        row.push(Vec::new());
                    }
                }
                _ => {}
            },
            X::End(n) => match n.as_str() {
                "list" => {
                    lists.pop();
                }
                "p" | "h" => {
                    if let Some(p) = para.take() {
                        match table.rows.last_mut().and_then(|r| r.last_mut()) {
                            Some(cell) if table_depth > 0 => cell.push(p),
                            _ => doc.paragraphs.push(p),
                        }
                    }
                    spans.clear();
                }
                "span" => {
                    spans.pop();
                }
                "a" => {
                    spans.pop();
                    link = None;
                }
                "table" => {
                    if table_depth == 1 {
                        doc.paragraphs.push(Paragraph { table: Some(std::mem::take(&mut table)), ..Paragraph::default() });
                    }
                    table_depth -= 1;
                }
                _ => {}
            },
            // Spacing in the text folds into one space, as ODF has it
            // (runs of spaces are `text:s`).
            X::Text(t) => {
                if para.is_some() {
                    let mut folded = String::with_capacity(t.len());
                    for c in t.chars() {
                        if c.is_whitespace() {
                            if !folded.ends_with(' ') {
                                folded.push(' ');
                            }
                        } else {
                            folded.push(c);
                        }
                    }
                    push(&mut para, &spans, &folded);
                }
            }
        }
    }
    doc.tidy();
    Ok(doc)
}

fn push(para: &mut Option<Paragraph>, spans: &[Span], text: &str) {
    if let Some(p) = para.as_mut() {
        p.spans.push(spans.last().cloned().unwrap_or_default().with_text(text));
    }
}
