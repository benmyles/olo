//! PowerPoint presentations, each slide drawn as it looks: its
//! background, pictures, shapes, lines, tables and text, with what a
//! slide takes from its layout, its master and the theme (where
//! placeholders sit, how their text is set, the colours and fonts).
//! Natively: the file's own DrawingML is turned into an SVG drawing a
//! slide at a time and resvg draws it (as it draws SVG files), so the
//! preview shows slides as pages, as a PDF's (`Body::Pages`): the first
//! at once, the rest as the preview scrolls to them.
//!
//! What it knows: solid, gradient and picture fills; outlines and their
//! dashes; the usual shapes by name and any drawn by its own path
//! (`custGeom`); groups, rotation and flips; pictures (cropped as the
//! slide crops them); text with its sizes, colours, weights, fonts,
//! alignment, bullets and numbers, spacing, insets and anchoring, wrapped
//! by the fonts' own widths, shrunk as the file says it was; tables.
//! Charts and diagrams (drawn by PowerPoint from data) show as their
//! fallback picture when the file has one, else not at all; text
//! effects, shadows and 3-D are left out.

use std::{
    collections::HashMap,
    fmt::Write as _,
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

use base64::Engine as _;
use resvg::usvg::fontdb;

use super::load::{Body, Send, Update};
use crate::Bitmap;

/// Slides are rendered this tall, in pixels (as a PDF's pages).
const SLIDE_PX: f32 = 1080.0;
/// The most pixels a slide is drawn at, either way.
const MOST_PX: u32 = 8192;
/// EMUs in a point.
const EMU: f64 = 12700.0;

type Zip = zip::ZipArchive<std::fs::File>;

// ---- the file's XML --------------------------------------------------------

/// An element: its name without its prefix, its attributes as written
/// (`r:embed` keeps its prefix: `id` and `r:id` are two things), its
/// children in order and its own text.
#[derive(Debug, Default, Clone)]
pub(crate) struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Node>,
    text: String,
}

impl Node {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn num(&self, key: &str) -> Option<f64> {
        self.attr(key)?.parse().ok()
    }

    fn flag(&self, key: &str) -> Option<bool> {
        self.attr(key).map(|v| matches!(v, "1" | "true" | "on"))
    }

    fn kid(&self, name: &str) -> Option<&Node> {
        self.kids.iter().find(|k| k.name == name)
    }

    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.kids.iter().filter(move |k| k.name == name)
    }

    /// The element down `path` from here, a child at each step.
    fn at(&self, path: &[&str]) -> Option<&Node> {
        path.iter().try_fold(self, |n, name| n.kid(name))
    }

    /// Its children as drawn: what an `AlternateContent` stands in for
    /// (its fallback, a picture of what this can't draw, else its
    /// choice) in its place.
    fn drawn(&self) -> Vec<&Node> {
        let mut out = Vec::new();
        for k in &self.kids {
            if k.name == "AlternateContent" {
                let stand_in = k.kid("Fallback").filter(|f| !f.kids.is_empty()).or_else(|| k.kid("Choice"));
                out.extend(stand_in.into_iter().flat_map(|s| s.drawn()));
            } else {
                out.push(k);
            }
        }
        out
    }
}

/// The document's root element, as a tree.
pub(crate) fn tree(xml: &str) -> Node {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml);
    let mut stack: Vec<Node> = vec![Node::default()];
    let start = |e: &quick_xml::events::BytesStart| Node {
        name: e.local_name().as_ref().to_owned(),
        attrs: e.attributes().flatten().map(|a| (a.key.as_ref().to_owned(), a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.into_owned()).unwrap_or_default())).collect(),
        ..Default::default()
    };
    let close = |stack: &mut Vec<Node>| {
        if stack.len() > 1 {
            let done = stack.pop().unwrap();
            stack.last_mut().unwrap().kids.push(done);
        }
    };
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => stack.push(start(&e)),
            Ok(Event::Empty(e)) => {
                stack.push(start(&e));
                close(&mut stack);
            }
            Ok(Event::End(_)) => close(&mut stack),
            Ok(Event::Text(t)) => stack.last_mut().unwrap().text.push_str(&t.xml10_content()),
            Ok(Event::CData(t)) => stack.last_mut().unwrap().text.push_str(AsRef::<str>::as_ref(&t)),
            Ok(Event::GeneralRef(g)) => {
                let s = match g.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    _ => match g.xml10_content().as_ref() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        _ => String::new(),
                    },
                };
                stack.last_mut().unwrap().text.push_str(&s);
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.pop().and_then(|root| root.kids.into_iter().next()).unwrap_or_default()
}

fn bytes(z: &mut Zip, name: &str) -> Option<Vec<u8>> {
    let mut f = z.by_name(name).ok()?;
    let mut out = Vec::new();
    f.read_to_end(&mut out).ok()?;
    Some(out)
}

/// `target` (a relationship's) from the part at `from`, as a path in the
/// file: relative to the part's folder, `..` taken up, or from the top.
fn resolve(from: &str, target: &str) -> String {
    if let Some(top) = target.strip_prefix('/') {
        return top.to_owned();
    }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for step in target.split('/') {
        match step {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// One part of the file (a slide, a layout, a master): its XML, and what
/// it refers to by id: each one's kind (the last word of its type) and
/// its path in the file.
#[derive(Default)]
struct Part {
    root: Node,
    rels: HashMap<String, (String, String)>,
}

impl Part {
    fn open(z: &mut Zip, path: &str) -> Option<Part> {
        let root = tree(&String::from_utf8_lossy(&bytes(z, path)?));
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
        let rels_path = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
        let mut rels = HashMap::new();
        if let Some(text) = bytes(z, &rels_path) {
            for r in tree(&String::from_utf8_lossy(&text)).all("Relationship") {
                if let (Some(id), Some(target)) = (r.attr("Id"), r.attr("Target")) {
                    let kind = r.attr("Type").and_then(|t| t.rsplit('/').next()).unwrap_or_default().to_owned();
                    let external = r.attr("TargetMode") == Some("External");
                    rels.insert(id.to_owned(), (kind, if external { String::new() } else { resolve(path, target) }));
                }
            }
        }
        Some(Part { root, rels })
    }

    /// The path of the first part of `kind` this one refers to.
    fn linked(&self, kind: &str) -> Option<&str> {
        self.rels.values().find(|(k, p)| k == kind && !p.is_empty()).map(|(_, p)| p.as_str())
    }
}

// ---- colours ---------------------------------------------------------------

/// A colour and how much of it shows (0–1).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Color {
    rgb: [u8; 3],
    alpha: f32,
}

impl Color {
    fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.rgb[0], self.rgb[1], self.rgb[2])
    }
}

fn hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim_start_matches('#');
    (s.len() == 6).then(|| u32::from_str_radix(s, 16).ok()).flatten().map(|v| [(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

fn hsl([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn rgb_of((h, s, l): (f32, f32, f32)) -> [u8; 3] {
    let hue = |p: f32, q: f32, mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    if s == 0.0 {
        return [byte(l); 3];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    [byte(hue(p, q, h + 1.0 / 3.0)), byte(hue(p, q, h)), byte(hue(p, q, h - 1.0 / 3.0))]
}

/// A colour by its name in the files (`prstClr`): the common ones.
fn named(name: &str) -> Option<[u8; 3]> {
    Some(match name {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "gray" | "grey" => [128, 128, 128],
        "ltGray" | "lightGray" | "lightGrey" => [211, 211, 211],
        "dkGray" | "darkGray" | "darkGrey" => [169, 169, 169],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        "cyan" | "aqua" => [0, 255, 255],
        "magenta" | "fuchsia" => [255, 0, 255],
        "navy" => [0, 0, 128],
        "teal" => [0, 128, 128],
        "maroon" => [128, 0, 0],
        "olive" => [128, 128, 0],
        "silver" => [192, 192, 192],
        "lime" => [0, 255, 0],
        _ => return None,
    })
}

/// The theme's colours and fonts, and the master's names for them.
#[derive(Default, Clone)]
struct Theme {
    colors: HashMap<String, [u8; 3]>,
    /// The master's map: `bg1` → `lt1`, `tx1` → `dk1`, …
    map: HashMap<String, String>,
    major: String,
    minor: String,
}

impl Theme {
    fn read(&mut self, theme: &Node) {
        if let Some(scheme) = theme.at(&["themeElements", "clrScheme"]) {
            for c in &scheme.kids {
                let value = c.kids.first().and_then(|v| v.attr("lastClr").or(v.attr("val"))).and_then(hex);
                if let Some(v) = value {
                    self.colors.insert(c.name.clone(), v);
                }
            }
        }
        let latin = |which: &str| theme.at(&["themeElements", "fontScheme", which, "latin"]).and_then(|l| l.attr("typeface")).unwrap_or_default().to_owned();
        self.major = latin("majorFont");
        self.minor = latin("minorFont");
    }

    fn scheme(&self, name: &str) -> Option<[u8; 3]> {
        let name = self.map.get(name).map_or(name, String::as_str);
        let name = match name {
            "bg1" => "lt1",
            "tx1" => "dk1",
            "bg2" => "lt2",
            "tx2" => "dk2",
            n => n,
        };
        self.colors.get(name).copied()
    }

    /// The colour a fill's colour element says (`srgbClr`, `schemeClr`,
    /// `sysClr`, `prstClr`), with its lightening, darkening and
    /// transparency; `ph` is what `phClr` stands for (a style's).
    fn color(&self, c: &Node, ph: Option<Color>) -> Option<Color> {
        let mut color = match c.name.as_str() {
            "srgbClr" => Color { rgb: hex(c.attr("val")?)?, alpha: 1.0 },
            "sysClr" => Color { rgb: c.attr("lastClr").and_then(hex).or_else(|| named(if c.attr("val")? == "window" { "white" } else { "black" }))?, alpha: 1.0 },
            "prstClr" => Color { rgb: named(c.attr("val")?)?, alpha: 1.0 },
            "schemeClr" => match c.attr("val")? {
                "phClr" => ph?,
                name => Color { rgb: self.scheme(name)?, alpha: 1.0 },
            },
            "scrgbClr" => {
                let part = |k: &str| (c.num(k).unwrap_or(0.0) / 100_000.0).clamp(0.0, 1.0).powf(1.0 / 2.2);
                Color { rgb: [(part("r") * 255.0) as u8, (part("g") * 255.0) as u8, (part("b") * 255.0) as u8], alpha: 1.0 }
            }
            _ => return None,
        };
        for m in &c.kids {
            let v = (m.num("val").unwrap_or(100_000.0) / 100_000.0) as f32;
            match m.name.as_str() {
                "alpha" => color.alpha *= v,
                "lumMod" => {
                    let (h, s, l) = hsl(color.rgb);
                    color.rgb = rgb_of((h, s, l * v));
                }
                "lumOff" => {
                    let (h, s, l) = hsl(color.rgb);
                    color.rgb = rgb_of((h, s, (l + v).clamp(0.0, 1.0)));
                }
                "tint" => color.rgb = color.rgb.map(|x| (255.0 - (255.0 - f32::from(x)) * v).round() as u8),
                "shade" => color.rgb = color.rgb.map(|x| (f32::from(x) * v).round() as u8),
                _ => {}
            }
        }
        Some(color)
    }

    /// The colour of the first colour element in `holder` (a
    /// `solidFill`, a gradient's stop, a reference).
    fn color_in(&self, holder: &Node, ph: Option<Color>) -> Option<Color> {
        holder.kids.iter().find_map(|c| self.color(c, ph))
    }

    /// A typeface as written: the theme's for `+mj-lt` and `+mn-lt`.
    fn font(&self, face: &str) -> String {
        match face {
            f if f.starts_with("+mj") => self.major.clone(),
            f if f.starts_with("+mn") => self.minor.clone(),
            f => f.to_owned(),
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ---- fonts -----------------------------------------------------------------

/// The fonts text is measured with (the system's, as resvg draws with).
struct Fonts {
    db: Arc<fontdb::Database>,
    found: std::cell::RefCell<HashMap<(String, bool, bool), Option<fontdb::ID>>>,
}

impl Fonts {
    fn new() -> Fonts {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Fonts { db: Arc::new(db), found: Default::default() }
    }

    fn face(&self, family: &str, bold: bool, italic: bool) -> Option<fontdb::ID> {
        *self.found.borrow_mut().entry((family.to_owned(), bold, italic)).or_insert_with(|| {
            let families = [fontdb::Family::Name(family), fontdb::Family::SansSerif];
            let query = fontdb::Query {
                families: if family.is_empty() { &families[1..] } else { &families[..] },
                weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
                stretch: fontdb::Stretch::Normal,
                style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
            };
            self.db.query(&query)
        })
    }

    /// Whether bold text in `family` has to be thickened by hand: the
    /// face found for it isn't a bold one (the family has none, or it is
    /// one variable font, whose weights resvg doesn't set).
    fn thicken(&self, family: &str, italic: bool) -> bool {
        self.face(family, true, italic).and_then(|id| self.db.face(id)).is_none_or(|f| f.weight.0 < 600)
    }

    /// How wide `text` is in that face at `size` points (its glyphs'
    /// advances; half its size a letter for a font that can't be read).
    fn width(&self, family: &str, bold: bool, italic: bool, size: f32, text: &str) -> f32 {
        let measured = self.face(family, bold, italic).and_then(|id| {
            self.db
                .with_face_data(id, |data, index| {
                    let face = ttf_parser::Face::parse(data, index).ok()?;
                    let em = f32::from(face.units_per_em());
                    let width: f32 = text.chars().map(|c| face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g)).map_or(em * 0.5, f32::from)).sum();
                    Some(width / em * size)
                })
                .flatten()
        });
        measured.unwrap_or(text.chars().count() as f32 * size * 0.5)
    }
}

// ---- text ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct RunStyle {
    size: f32,
    bold: bool,
    italic: bool,
    underline: bool,
    color: Color,
    font: String,
    /// Bold, in a face that isn't: drawn with an outline of its colour.
    thicken: bool,
}

#[derive(Debug, Clone)]
struct Piece {
    text: String,
    style: RunStyle,
    width: f32,
}

#[derive(Debug, Clone, Default)]
struct Line {
    pieces: Vec<Piece>,
    /// Where its text starts and how much room it has, in the box.
    start: f32,
    room: f32,
    /// Its bullet, drawn before it, and where.
    bullet: Option<(Piece, f32)>,
    align: Align,
    height: f32,
    /// Its largest text.
    size: f32,
    /// Space over it (a paragraph's first line).
    before: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// The first of `sources` that has attribute `key`.
fn first_attr<'a>(sources: &[&'a Node], key: &str) -> Option<&'a str> {
    sources.iter().find_map(|s| s.attr(key))
}

/// A text body laid out in a box `width` wide (None: lines as long as
/// they are): its lines, top to bottom.
struct Layout<'a> {
    theme: &'a Theme,
    fonts: &'a Fonts,
    /// Where text properties come from, nearest first: each a list of
    /// levels (`lvl1pPr`…): the shape's own, its placeholder's in the
    /// layout and the master, the master's style for its kind, the
    /// presentation's defaults.
    styles: Vec<&'a Node>,
    /// What `phClr` stands for, and the colour text has when nothing
    /// says (the shape style's font colour, else the theme's text).
    font_color: Option<Color>,
    /// The sizes' scale and the line spacing taken off, as the file says
    /// the text was shrunk to fit.
    scale: f32,
    tighter: f32,
}

impl<'a> Layout<'a> {
    fn run_style(&self, own: Option<&Node>, para: &[&Node]) -> RunStyle {
        let mut sources: Vec<&Node> = own.into_iter().collect();
        sources.extend(para.iter().filter_map(|p| p.kid("defRPr")));
        let color = sources
            .iter()
            .find_map(|s| s.kid("solidFill"))
            .and_then(|f| self.theme.color_in(f, self.font_color))
            .or(self.font_color)
            .unwrap_or(Color { rgb: self.theme.scheme("tx1").unwrap_or([0, 0, 0]), alpha: 1.0 });
        let face = sources.iter().find_map(|s| s.kid("latin").and_then(|l| l.attr("typeface"))).map(|f| self.theme.font(f)).unwrap_or_else(|| self.theme.minor.clone());
        let bold = first_attr(&sources, "b").is_some_and(|b| b == "1" || b == "true");
        let italic = first_attr(&sources, "i").is_some_and(|b| b == "1" || b == "true");
        RunStyle {
            size: first_attr(&sources, "sz").and_then(|s| s.parse::<f32>().ok()).map_or(18.0, |s| s / 100.0) * self.scale,
            bold,
            italic,
            underline: first_attr(&sources, "u").is_some_and(|u| u != "none"),
            color,
            thicken: bold && self.fonts.thicken(&face, italic),
            font: face,
        }
    }

    fn piece(&self, text: &str, style: &RunStyle) -> Piece {
        Piece { width: self.fonts.width(&style.font, style.bold, style.italic, style.size, text), text: text.to_owned(), style: style.clone() }
    }

    fn lines(&self, body: &Node, width: Option<f32>) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut number = 0u32;
        for (n, p) in body.all("p").enumerate() {
            let ppr = p.kid("pPr");
            let level = ppr.and_then(|p| p.num("lvl")).unwrap_or(0.0) as usize;
            let name = format!("lvl{}pPr", level + 1);
            let mut para: Vec<&Node> = ppr.into_iter().collect();
            para.extend(self.styles.iter().filter_map(|s| s.kid(&name)));
            let pt = |emu: &str| emu.parse::<f32>().ok().map(|v| v / EMU as f32);
            let margin = first_attr(&para, "marL").and_then(pt).unwrap_or(0.0);
            let indent = first_attr(&para, "indent").and_then(pt).unwrap_or(0.0);
            let align = match first_attr(&para, "algn") {
                Some("ctr") => Align::Center,
                Some("r") => Align::Right,
                _ => Align::Left,
            };
            let base = self.run_style(p.kid("endParaRPr"), &para);
            // (Spacing: so many points, or a share of the text's size.)
            let spacing = |name: &str, of: f32| -> Option<f32> {
                let s = para.iter().find_map(|s| s.kid(name))?;
                s.kid("spcPts").and_then(|v| v.num("val")).map(|v| v as f32 / 100.0).or_else(|| s.kid("spcPct").and_then(|v| v.num("val")).map(|v| v as f32 / 100_000.0 * of))
            };
            // Its runs as words, spaces and breaks.
            let mut pieces: Vec<Option<Piece>> = Vec::new();
            for r in &p.kids {
                match r.name.as_str() {
                    "r" | "fld" => {
                        let style = self.run_style(r.kid("rPr"), &para);
                        let text = r.kid("t").map(|t| t.text.as_str()).unwrap_or_default();
                        let mut word = String::new();
                        for c in text.chars() {
                            if c == ' ' || c == '\t' {
                                if !word.is_empty() {
                                    pieces.push(Some(self.piece(&std::mem::take(&mut word), &style)));
                                }
                                pieces.push(Some(self.piece(" ", &style)));
                            } else if c == '\n' || c == '\u{b}' {
                                if !word.is_empty() {
                                    pieces.push(Some(self.piece(&std::mem::take(&mut word), &style)));
                                }
                                pieces.push(None);
                            } else {
                                word.push(c);
                            }
                        }
                        if !word.is_empty() {
                            pieces.push(Some(self.piece(&word, &style)));
                        }
                    }
                    "br" => pieces.push(None),
                    _ => {}
                }
            }
            let has_text = pieces.iter().flatten().any(|p| !p.text.trim().is_empty());
            // Its bullet: what the nearest source that says anything of
            // bullets says; only a paragraph with text has one.
            let bullet = para.iter().find_map(|s| s.kids.iter().find(|k| matches!(k.name.as_str(), "buNone" | "buChar" | "buAutoNum" | "buBlip"))).filter(|_| has_text);
            let first_style = pieces.iter().flatten().next().map_or(base.clone(), |p| p.style.clone());
            let mark = match bullet.map(|b| (b.name.as_str(), b)) {
                Some(("buChar", b)) => {
                    number = 0;
                    b.attr("char").map(str::to_owned)
                }
                Some(("buAutoNum", b)) => {
                    number += 1;
                    let n = number + b.num("startAt").map_or(0, |s| s as u32 - 1);
                    Some(match b.attr("type").unwrap_or("arabicPeriod") {
                        t if t.starts_with("alphaLc") => format!("{}.", (b'a' + ((n - 1) % 26) as u8) as char),
                        t if t.starts_with("alphaUc") => format!("{}.", (b'A' + ((n - 1) % 26) as u8) as char),
                        t if t.ends_with("ParenR") => format!("{n})"),
                        _ => format!("{n}."),
                    })
                }
                Some(("buBlip", _)) => Some("•".to_owned()),
                _ => {
                    number = 0;
                    None
                }
            };
            let mark = mark.map(|m| {
                let mut style = first_style.clone();
                (style.bold, style.italic, style.underline, style.thicken) = (false, false, false, false);
                if let Some(c) = para.iter().find_map(|s| s.kid("buClr")).and_then(|c| self.theme.color_in(c, self.font_color)) {
                    style.color = c;
                }
                self.piece(&m, &style)
            });
            // Where its first line and the rest start: a bullet hangs in
            // the margin's indent, the text at the margin.
            let (first, rest) = match &mark {
                Some(m) if indent < 0.0 => (margin, margin),
                Some(m) => (margin + indent + m.width + m.style.size * 0.3, margin),
                None => ((margin + indent).max(0.0), margin),
            };
            let bullet_at = (margin + indent).max(0.0);
            let spacing_of = |size: f32| -> f32 {
                let natural = size * 1.2;
                let set = para.iter().find_map(|s| s.kid("lnSpc"));
                let h = match set.and_then(|s| s.kid("spcPts")).and_then(|v| v.num("val")) {
                    Some(pts) => pts as f32 / 100.0,
                    None => natural * set.and_then(|s| s.kid("spcPct")).and_then(|v| v.num("val")).map_or(1.0, |v| v as f32 / 100_000.0),
                };
                h * (1.0 - self.tighter)
            };
            let before = if n == 0 { 0.0 } else { spacing("spcBef", first_style.size).unwrap_or(0.0) };
            let after = spacing("spcAft", first_style.size).unwrap_or(0.0);
            let from = lines.len();
            let mut line = Line { start: first, room: width.map_or(f32::MAX, |w| (w - first).max(1.0)), bullet: mark.map(|m| (m, bullet_at)), align, before, ..Default::default() };
            let mut used = 0.0f32;
            let close = |line: &mut Line, lines: &mut Vec<Line>, base: &RunStyle| {
                while line.pieces.last().is_some_and(|p| p.text == " ") {
                    line.pieces.pop();
                }
                line.size = line.pieces.iter().map(|p| p.style.size).fold(0.0, f32::max);
                if line.size == 0.0 {
                    line.size = base.size;
                }
                line.height = spacing_of(line.size);
                lines.push(std::mem::take(line));
            };
            for piece in pieces {
                match piece {
                    None => {
                        close(&mut line, &mut lines, &base);
                        line = Line { start: rest, room: width.map_or(f32::MAX, |w| (w - rest).max(1.0)), align, ..Default::default() };
                        used = 0.0;
                    }
                    Some(p) => {
                        if used + p.width > line.room && used > 0.0 && p.text != " " {
                            close(&mut line, &mut lines, &base);
                            line = Line { start: rest, room: width.map_or(f32::MAX, |w| (w - rest).max(1.0)), align, ..Default::default() };
                            used = 0.0;
                        }
                        if p.text == " " && line.pieces.is_empty() && used == 0.0 && lines.len() > from {
                            continue;
                        }
                        used += p.width;
                        line.pieces.push(p);
                    }
                }
            }
            close(&mut line, &mut lines, &base);
            if let Some(last) = lines.last_mut() {
                last.height += after;
            }
        }
        lines
    }
}

/// The lines as SVG text, the box's top left at (`x`, `y`).
fn text_svg(lines: &[Line], x: f32, y: f32, out: &mut String) {
    let mut top = y;
    let span = |p: &Piece, out: &mut String| {
        let s = &p.style;
        let _ = write!(
            out,
            r#"<tspan font-family="{}, sans-serif" font-size="{:.2}" fill="{}"{}{}{}{}{}>{}</tspan>"#,
            esc(&s.font),
            s.size,
            s.color.hex(),
            if s.color.alpha < 1.0 { format!(r#" fill-opacity="{:.3}""#, s.color.alpha) } else { String::new() },
            if s.thicken { format!(r#" stroke="{}" stroke-width="{:.2}" stroke-linejoin="round""#, s.color.hex(), s.size * 0.035) } else { String::new() },
            if s.bold { r#" font-weight="bold""# } else { "" },
            if s.italic { r#" font-style="italic""# } else { "" },
            if s.underline { r#" text-decoration="underline""# } else { "" },
            esc(&p.text)
        );
    };
    for line in lines {
        top += line.before;
        let text_height = line.height.min(line.size * 1.2);
        let baseline = top + (line.height - text_height).max(0.0) + line.size * 0.93;
        if let Some((mark, at)) = &line.bullet {
            let _ = write!(out, r#"<text x="{:.2}" y="{:.2}" xml:space="preserve">"#, x + at, baseline);
            span(mark, out);
            out.push_str("</text>");
        }
        if !line.pieces.is_empty() {
            let (at, anchor) = match line.align {
                Align::Left => (x + line.start, "start"),
                Align::Center => (x + line.start + line.room / 2.0, "middle"),
                Align::Right => (x + line.start + line.room, "end"),
            };
            let _ = write!(out, r#"<text x="{at:.2}" y="{baseline:.2}" text-anchor="{anchor}" xml:space="preserve">"#);
            for p in &line.pieces {
                span(p, out);
            }
            out.push_str("</text>");
        }
        top += line.height;
    }
}

fn height_of(lines: &[Line]) -> f32 {
    lines.iter().map(|l| l.before + l.height).sum()
}

// ---- shapes ----------------------------------------------------------------

/// A shape's outline by its name (`prstGeom`), `w` by `h`, as an SVG
/// path. `adj`: its adjust values by name (thousandths of a hundred).
/// One it doesn't know is its box.
fn outline(name: &str, w: f32, h: f32, adj: &HashMap<String, f32>) -> String {
    let a = |key: &str, default: f32| adj.get(key).copied().unwrap_or(default) / 100_000.0;
    let m = w.min(h);
    let poly = |points: &[(f32, f32)]| {
        let mut d = String::new();
        for (i, (x, y)) in points.iter().enumerate() {
            let _ = write!(d, "{}{:.2} {:.2} ", if i == 0 { "M" } else { "L" }, x, y);
        }
        d + "Z"
    };
    let rounded = |r: f32| {
        let r = r.min(w / 2.0).min(h / 2.0);
        format!(
            "M{r:.2} 0 H{:.2} A{r:.2} {r:.2} 0 0 1 {w:.2} {r:.2} V{:.2} A{r:.2} {r:.2} 0 0 1 {:.2} {h:.2} H{r:.2} A{r:.2} {r:.2} 0 0 1 0 {:.2} V{r:.2} A{r:.2} {r:.2} 0 0 1 {r:.2} 0 Z",
            w - r,
            h - r,
            w - r,
            h - r
        )
    };
    match name {
        "ellipse" | "flowChartConnector" => format!("M0 {:.2} A{:.2} {:.2} 0 1 0 {w:.2} {:.2} A{:.2} {:.2} 0 1 0 0 {:.2} Z", h / 2.0, w / 2.0, h / 2.0, h / 2.0, w / 2.0, h / 2.0, h / 2.0),
        "roundRect" | "flowChartAlternateProcess" => rounded(m * a("adj", 16_667.0)),
        "round2SameRect" => {
            let r = (m * a("adj1", 16_667.0)).min(w / 2.0).min(h);
            format!("M{r:.2} 0 H{:.2} A{r:.2} {r:.2} 0 0 1 {w:.2} {r:.2} V{h:.2} H0 V{r:.2} A{r:.2} {r:.2} 0 0 1 {r:.2} 0 Z", w - r)
        }
        "flowChartTerminator" => rounded(h / 2.0),
        "triangle" | "flowChartExtract" => poly(&[(w * a("adj", 50_000.0), 0.0), (w, h), (0.0, h)]),
        "rtTriangle" => poly(&[(0.0, 0.0), (w, h), (0.0, h)]),
        "diamond" | "flowChartDecision" => poly(&[(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)]),
        "parallelogram" | "flowChartInputOutput" => {
            let s = m * a("adj", 25_000.0);
            poly(&[(s, 0.0), (w, 0.0), (w - s, h), (0.0, h)])
        }
        "trapezoid" => {
            let s = m * a("adj", 25_000.0);
            poly(&[(s, 0.0), (w - s, 0.0), (w, h), (0.0, h)])
        }
        "pentagon" => poly(&[(w / 2.0, 0.0), (w, h * 0.38), (w * 0.81, h), (w * 0.19, h), (0.0, h * 0.38)]),
        "hexagon" => {
            let s = m * a("adj", 25_000.0);
            poly(&[(s, 0.0), (w - s, 0.0), (w, h / 2.0), (w - s, h), (s, h), (0.0, h / 2.0)])
        }
        "octagon" => {
            let s = m * a("adj", 29_289.0);
            poly(&[(s, 0.0), (w - s, 0.0), (w, s), (w, h - s), (w - s, h), (s, h), (0.0, h - s), (0.0, s)])
        }
        "homePlate" => {
            let s = (m * a("adj", 50_000.0)).min(w);
            poly(&[(0.0, 0.0), (w - s, 0.0), (w, h / 2.0), (w - s, h), (0.0, h)])
        }
        "chevron" => {
            let s = (m * a("adj", 50_000.0)).min(w);
            poly(&[(0.0, 0.0), (w - s, 0.0), (w, h / 2.0), (w - s, h), (0.0, h), (s, h / 2.0)])
        }
        "rightArrow" | "leftArrow" | "upArrow" | "downArrow" => {
            // (Drawn pointing right in a box of its length by its
            // breadth, then turned.)
            let across = matches!(name, "rightArrow" | "leftArrow");
            let (len, breadth) = if across { (w, h) } else { (h, w) };
            let shaft = breadth * a("adj1", 50_000.0);
            let head = (breadth.min(len) * a("adj2", 50_000.0)).min(len);
            let (y1, y2) = ((breadth - shaft) / 2.0, (breadth + shaft) / 2.0);
            let pts = [(0.0, y1), (len - head, y1), (len - head, 0.0), (len, breadth / 2.0), (len - head, breadth), (len - head, y2), (0.0, y2)];
            let turned: Vec<(f32, f32)> = pts
                .iter()
                .map(|&(x, y)| match name {
                    "rightArrow" => (x, y),
                    "leftArrow" => (w - x, y),
                    "downArrow" => (y, x),
                    _ => (y, h - x),
                })
                .collect();
            poly(&turned)
        }
        "plus" => {
            let s = m * a("adj", 25_000.0);
            poly(&[(s, 0.0), (w - s, 0.0), (w - s, s), (w, s), (w, h - s), (w - s, h - s), (w - s, h), (s, h), (s, h - s), (0.0, h - s), (0.0, s), (s, s)])
        }
        "star5" => {
            let (cx, cy) = (w / 2.0, h / 2.0);
            let pts: Vec<(f32, f32)> = (0..10)
                .map(|i| {
                    let r = if i % 2 == 0 { 1.0 } else { 0.4 };
                    let t = std::f32::consts::PI * (i as f32 / 5.0 - 0.5);
                    (cx + cx * r * t.cos(), cy + cy * r * t.sin())
                })
                .collect();
            poly(&pts)
        }
        "line" | "straightConnector1" => format!("M0 0 L{w:.2} {h:.2}"),
        // Connectors: an elbow (across, then down), two (across to
        // where `adj1` says, half way unless said, down, across), and
        // the same as curves.
        "bentConnector2" => format!("M0 0 L{w:.2} 0 L{w:.2} {h:.2}"),
        "bentConnector3" | "bentConnector4" | "bentConnector5" => {
            let x = w * adj.get("adj1").copied().unwrap_or(50000.0) / 100000.0;
            format!("M0 0 L{x:.2} 0 L{x:.2} {h:.2} L{w:.2} {h:.2}")
        }
        "curvedConnector2" => format!("M0 0 Q{w:.2} 0 {w:.2} {h:.2}"),
        "curvedConnector3" | "curvedConnector4" | "curvedConnector5" => {
            let x = w * adj.get("adj1").copied().unwrap_or(50000.0) / 100000.0;
            format!("M0 0 C{x:.2} 0 {x:.2} {h:.2} {w:.2} {h:.2}")
        }
        _ => poly(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]),
    }
}

/// The points of a path made of moves, lines and curves only, in order
/// (a curve's control points too: the way it leaves and arrives), and
/// whether it is lines only. `None` for anything else (arcs, a closed
/// path): its ends aren't found.
fn path_points(d: &str) -> Option<(Vec<(f32, f32)>, bool)> {
    let (mut points, mut straight) = (Vec::new(), true);
    for part in d.split(|c: char| c == 'M' || c == 'L' || c == 'Q' || c == 'C').skip(1) {
        let nums: Vec<f32> = part.split_whitespace().map(|n| n.parse().ok()).collect::<Option<_>>()?;
        if nums.len() % 2 != 0 {
            return None;
        }
        straight &= nums.len() == 2;
        points.extend(nums.chunks(2).map(|p| (p[0], p[1])));
    }
    (points.len() >= 2 && !d.contains(['A', 'Z'])).then_some((points, straight))
}

/// A line's arrowheads (`ln`'s `headEnd`, at the path's start, and
/// `tailEnd`, at its end), as shapes in `color`, and the path to draw
/// with them: a path of lines stops inside a filled head, so a thick
/// line's square end doesn't stand out past its point.
fn arrowheads(ln: &Node, d: &str, color: Color, width: f32) -> (String, String) {
    let Some((mut points, straight)) = path_points(d) else { return (d.to_owned(), String::new()) };
    let mut heads = String::new();
    let n = points.len();
    for (end, at, from) in [("headEnd", 0, 1), ("tailEnd", n - 1, n - 2)] {
        let Some(e) = ln.kid(end) else { continue };
        let kind = e.attr("type").unwrap_or("none");
        if kind == "none" {
            continue;
        }
        let (p, q) = (points[at], points[from]);
        let (dx, dy) = (p.0 - q.0, p.1 - q.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.01 {
            continue;
        }
        // (Along the line into its end, and across it.)
        let (u, v) = ((dx / len, dy / len), (-dy / len, dx / len));
        let factor = |a: &str| match e.attr(a) {
            Some("sm") => 2.0,
            Some("lg") => 5.0,
            _ => 3.0,
        };
        let base = width.max(2.0);
        let (long, half) = ((base * factor("len")).min(len), base * factor("w") / 2.0);
        let pt = |along: f32, across: f32| format!("{:.2},{:.2}", p.0 - u.0 * along + v.0 * across, p.1 - u.1 * along + v.1 * across);
        let fill = format!(r#"fill="{}" fill-opacity="{:.3}" stroke="none""#, color.hex(), color.alpha);
        let back = match kind {
            "arrow" => {
                let _ = write!(
                    heads,
                    r#"<polyline points="{} {} {}" fill="none" stroke="{}" stroke-opacity="{:.3}" stroke-width="{width:.2}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                    pt(long, half),
                    pt(0.0, 0.0),
                    pt(long, -half),
                    color.hex(),
                    color.alpha
                );
                0.0
            }
            "diamond" => {
                let _ = write!(heads, r#"<polygon points="{} {} {} {}" {fill}/>"#, pt(-long / 2.0, 0.0), pt(0.0, half), pt(long / 2.0, 0.0), pt(0.0, -half));
                0.0
            }
            "oval" => {
                let _ = write!(
                    heads,
                    r#"<ellipse cx="{:.2}" cy="{:.2}" rx="{:.2}" ry="{:.2}" transform="rotate({:.2} {:.2} {:.2})" {fill}/>"#,
                    p.0,
                    p.1,
                    long / 2.0,
                    half,
                    u.1.atan2(u.0).to_degrees(),
                    p.0,
                    p.1
                );
                0.0
            }
            "stealth" => {
                let _ = write!(heads, r#"<polygon points="{} {} {} {}" {fill}/>"#, pt(0.0, 0.0), pt(long, half), pt(long * 0.6, 0.0), pt(long, -half));
                long * 0.5
            }
            _ => {
                let _ = write!(heads, r#"<polygon points="{} {} {}" {fill}/>"#, pt(0.0, 0.0), pt(long, half), pt(long, -half));
                long * 0.9
            }
        };
        if straight && back > 0.0 {
            points[at] = (p.0 - u.0 * back, p.1 - u.1 * back);
        }
    }
    if heads.is_empty() || !straight {
        return (d.to_owned(), heads);
    }
    let mut d = String::new();
    for (i, p) in points.iter().enumerate() {
        let _ = write!(d, "{}{:.2} {:.2} ", if i == 0 { 'M' } else { 'L' }, p.0, p.1);
    }
    (d.trim_end().to_owned(), heads)
}

/// A shape's own path (`custGeom`), scaled to `w` by `h`.
fn custom(geom: &Node, w: f32, h: f32) -> Option<String> {
    let mut d = String::new();
    for path in geom.kid("pathLst")?.all("path") {
        let (pw, ph) = (path.num("w").unwrap_or(0.0) as f32, path.num("h").unwrap_or(0.0) as f32);
        let (sx, sy) = (if pw > 0.0 { w / pw } else { 1.0 / EMU as f32 }, if ph > 0.0 { h / ph } else { 1.0 / EMU as f32 });
        let pt = |n: &Node| -> (f32, f32) { (n.num("x").unwrap_or(0.0) as f32 * sx, n.num("y").unwrap_or(0.0) as f32 * sy) };
        let mut at = (0.0f32, 0.0f32);
        for c in &path.kids {
            let pts: Vec<(f32, f32)> = c.all("pt").map(pt).collect();
            match (c.name.as_str(), pts.as_slice()) {
                ("moveTo", [p]) => {
                    let _ = write!(d, "M{:.2} {:.2} ", p.0, p.1);
                    at = *p;
                }
                ("lnTo", [p]) => {
                    let _ = write!(d, "L{:.2} {:.2} ", p.0, p.1);
                    at = *p;
                }
                ("cubicBezTo", [a, b, p]) => {
                    let _ = write!(d, "C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} ", a.0, a.1, b.0, b.1, p.0, p.1);
                    at = *p;
                }
                ("quadBezTo", [a, p]) => {
                    let _ = write!(d, "Q{:.2} {:.2} {:.2} {:.2} ", a.0, a.1, p.0, p.1);
                    at = *p;
                }
                ("arcTo", _) => {
                    // From where it is, along an ellipse of those radii,
                    // from one angle through another (60,000ths of a degree).
                    let (rx, ry) = (c.num("wR").unwrap_or(0.0) as f32 * sx, c.num("hR").unwrap_or(0.0) as f32 * sy);
                    let (start, swing) = ((c.num("stAng").unwrap_or(0.0) / 60_000.0).to_radians() as f32, (c.num("swAng").unwrap_or(0.0) / 60_000.0).to_radians() as f32);
                    let centre = (at.0 - rx * start.cos(), at.1 - ry * start.sin());
                    let end = (centre.0 + rx * (start + swing).cos(), centre.1 + ry * (start + swing).sin());
                    let _ = write!(d, "A{rx:.2} {ry:.2} 0 {} {} {:.2} {:.2} ", u8::from(swing.abs() > std::f32::consts::PI), u8::from(swing > 0.0), end.0, end.1);
                    at = end;
                }
                ("close", _) => d.push_str("Z "),
                _ => {}
            }
        }
    }
    (!d.is_empty()).then_some(d)
}

/// What a slide is drawn with: the theme, the fonts, and what it takes
/// its placeholders and styles from.
struct Draw<'a> {
    theme: &'a Theme,
    fonts: &'a Fonts,
    layout: Option<&'a Part>,
    master: Option<&'a Part>,
    /// The presentation's `defaultTextStyle`.
    defaults: Option<&'a Node>,
    /// Gradients, clips and the like (`<defs>`), and their count.
    defs: String,
    ids: usize,
}

/// A placeholder's kind and number.
fn placeholder(shape: &Node) -> Option<(&str, Option<&str>)> {
    let ph = ["nvSpPr", "nvPicPr", "nvGraphicFramePr"].iter().find_map(|n| shape.at(&[n, "nvPr", "ph"]))?;
    Some((ph.attr("type").unwrap_or("body"), ph.attr("idx")))
}

/// The kinds a master tells apart: a title, the body's, or itself.
fn kind_class(kind: &str) -> &str {
    match kind {
        "title" | "ctrTitle" => "title",
        "body" | "subTitle" | "obj" | "tbl" | "chart" | "pic" | "media" | "clipArt" | "dgm" => "body",
        other => other,
    }
}

impl<'a> Draw<'a> {
    /// The placeholder in `tree` a slide's (`kind`, `idx`) takes after:
    /// the one of its number, else of its kind.
    fn like(tree: &'a Node, kind: &str, idx: Option<&str>, by_class: bool) -> Option<&'a Node> {
        let all: Vec<(&Node, (&str, Option<&str>))> = tree.kids.iter().filter_map(|s| Some((s, placeholder(s)?))).collect();
        let by_idx = idx.and_then(|i| all.iter().find(|(_, (_, n))| *n == Some(i)));
        let by_kind = || all.iter().find(|(_, (k, _))| if by_class { kind_class(k) == kind_class(kind) } else { *k == kind || kind_class(k) == kind_class(kind) });
        if by_class { by_kind().map(|(s, _)| *s) } else { by_idx.or_else(by_kind).map(|(s, _)| *s) }
    }

    /// The shapes a slide's placeholder takes after: the layout's, then
    /// the master's.
    fn ancestors(&self, shape: &Node) -> Vec<&'a Node> {
        let Some((kind, idx)) = placeholder(shape) else { return Vec::new() };
        let layout = self.layout.and_then(|l| l.root.at(&["cSld", "spTree"])).and_then(|t| Self::like(t, kind, idx, false));
        let master = self.master.and_then(|m| m.root.at(&["cSld", "spTree"])).and_then(|t| Self::like(t, kind, None, true));
        layout.into_iter().chain(master).collect()
    }

    fn id(&mut self) -> String {
        self.ids += 1;
        format!("d{}", self.ids)
    }

    /// A fill (`solidFill`, `gradFill`, `noFill`, `pattFill`…) as the
    /// attributes that paint it: None if `holder` says nothing of its
    /// fill. A picture's fill is drawn by the caller.
    fn fill(&mut self, holder: &Node, ph: Option<Color>) -> Option<String> {
        for f in &holder.kids {
            match f.name.as_str() {
                "noFill" => return Some(r#"fill="none""#.into()),
                "solidFill" => {
                    let c = self.theme.color_in(f, ph)?;
                    return Some(format!(r#"fill="{}" fill-opacity="{:.3}""#, c.hex(), c.alpha));
                }
                "pattFill" => {
                    let c = f.kid("fgClr").and_then(|c| self.theme.color_in(c, ph))?;
                    return Some(format!(r#"fill="{}" fill-opacity="{:.3}""#, c.hex(), c.alpha));
                }
                "gradFill" => {
                    let stops: Vec<(f32, Color)> = f.kid("gsLst")?.all("gs").filter_map(|g| Some((g.num("pos")? as f32 / 1000.0, self.theme.color_in(g, ph)?))).collect();
                    if stops.is_empty() {
                        return None;
                    }
                    let id = self.id();
                    let angle = f.kid("lin").and_then(|l| l.num("ang")).map_or(0.0, |a| a / 60_000.0).to_radians();
                    let (dx, dy) = (angle.cos() as f32, angle.sin() as f32);
                    let radial = f.kid("path").is_some();
                    if radial {
                        let _ = write!(self.defs, r#"<radialGradient id="{id}" cx="0.5" cy="0.5" r="0.75">"#);
                    } else {
                        let _ = write!(self.defs, r#"<linearGradient id="{id}" x1="{:.3}" y1="{:.3}" x2="{:.3}" y2="{:.3}">"#, 0.5 - dx / 2.0, 0.5 - dy / 2.0, 0.5 + dx / 2.0, 0.5 + dy / 2.0);
                    }
                    let mut stops = stops;
                    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                    for (at, c) in stops {
                        let _ = write!(self.defs, r#"<stop offset="{at:.1}%" stop-color="{}" stop-opacity="{:.3}"/>"#, c.hex(), c.alpha);
                    }
                    self.defs.push_str(if radial { "</radialGradient>" } else { "</linearGradient>" });
                    return Some(format!(r#"fill="url(#{id})""#));
                }
                _ => {}
            }
        }
        None
    }

    /// An outline (`a:ln`) as the attributes that stroke it; None if it
    /// says nothing of how it's drawn.
    /// A line's colour and width (`None`: it says no colour, or none at
    /// all; a line with a width but no colour takes `ph`, its style's).
    fn line_look(&self, ln: &Node, ph: Option<Color>) -> Option<(Color, f32)> {
        if ln.kid("noFill").is_some() {
            return None;
        }
        let color = match (ln.kid("solidFill"), ln.kid("gradFill")) {
            (Some(f), _) => self.theme.color_in(f, ph)?,
            (None, Some(g)) => g.kid("gsLst").and_then(|l| l.kid("gs")).and_then(|g| self.theme.color_in(g, ph))?,
            (None, None) => ph?,
        };
        Some((color, ln.num("w").map_or(0.75, |w| (w / EMU) as f32).max(0.25)))
    }

    fn stroke(&self, ln: &Node, ph: Option<Color>) -> Option<String> {
        if ln.kid("noFill").is_some() {
            return Some(r#"stroke="none""#.into());
        }
        let (color, width) = self.line_look(ln, ph)?;
        let dash = match ln.kid("prstDash").and_then(|d| d.attr("val")) {
            Some("dash" | "sysDash") => format!(r#" stroke-dasharray="{:.2} {:.2}""#, width * 4.0, width * 3.0),
            Some("dot" | "sysDot") => format!(r#" stroke-dasharray="{:.2} {:.2}""#, width, width * 2.0),
            Some("lgDash") => format!(r#" stroke-dasharray="{:.2} {:.2}""#, width * 8.0, width * 3.0),
            Some("dashDot" | "lgDashDot" | "sysDashDot") => format!(r#" stroke-dasharray="{:.2} {:.2} {:.2} {:.2}""#, width * 4.0, width * 3.0, width, width * 3.0),
            _ => String::new(),
        };
        Some(format!(r#"stroke="{}" stroke-opacity="{:.3}" stroke-width="{width:.2}"{dash} stroke-linejoin="round""#, color.hex(), color.alpha))
    }

    /// A picture (`blip`, by its link in `part`) drawn in a box `w` by
    /// `h`, cropped as `src` says (`srcRect`: thousandths of a hundred
    /// off each side).
    fn picture(&mut self, z: &mut Zip, part: &Part, blip_fill: &Node, w: f32, h: f32, out: &mut String) {
        let Some(path) = blip_fill.kid("blip").and_then(|b| b.attr("r:embed")).and_then(|id| part.rels.get(id)).map(|(_, p)| p.clone()).filter(|p| !p.is_empty()) else { return };
        let Some(data) = bytes(z, &path) else { return };
        let ext = path.rsplit('.').next().unwrap_or_default().to_lowercase();
        // What resvg reads goes in as it is; anything else the image
        // crate can read, as a PNG made of it.
        let (mime, data) = match ext.as_str() {
            "png" => ("image/png", data),
            "jpg" | "jpeg" => ("image/jpeg", data),
            "gif" => ("image/gif", data),
            "webp" => ("image/webp", data),
            "svg" => ("image/svg+xml", data),
            _ => match image::load_from_memory(&data) {
                Ok(img) => {
                    let mut png = Vec::new();
                    if img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_err() {
                        return;
                    }
                    ("image/png", png)
                }
                Err(_) => return,
            },
        };
        let crop = |side: &str| blip_fill.kid("srcRect").and_then(|r| r.num(side)).map_or(0.0, |v| v as f32 / 100_000.0);
        let (l, t, r, b) = (crop("l"), crop("t"), crop("r"), crop("b"));
        let (fw, fh) = (w / (1.0 - l - r).max(0.01), h / (1.0 - t - b).max(0.01));
        let href = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&data));
        if l == 0.0 && t == 0.0 && r == 0.0 && b == 0.0 {
            let _ = write!(out, r#"<image x="0" y="0" width="{w:.2}" height="{h:.2}" preserveAspectRatio="none" href="{href}"/>"#);
        } else {
            let id = self.id();
            let _ = write!(self.defs, r#"<clipPath id="{id}"><rect x="0" y="0" width="{w:.2}" height="{h:.2}"/></clipPath>"#);
            let _ = write!(out, r#"<g clip-path="url(#{id})"><image x="{:.2}" y="{:.2}" width="{fw:.2}" height="{fh:.2}" preserveAspectRatio="none" href="{href}"/></g>"#, -l * fw, -t * fh);
        }
    }

    /// A shape's text, in its box `w` by `h`.
    fn text(&mut self, shape: &Node, body: &Node, w: f32, h: f32, font_color: Option<Color>, out: &mut String) {
        let ancestors = self.ancestors(shape);
        let kind = placeholder(shape).map(|(k, _)| kind_class(k));
        // Where its text's settings come from, nearest first.
        let mut styles: Vec<&Node> = body.kid("lstStyle").into_iter().collect();
        styles.extend(ancestors.iter().filter_map(|a| a.at(&["txBody", "lstStyle"])));
        let of_master = self.master.and_then(|m| m.root.kid("txStyles")).and_then(|t| {
            t.kid(match kind {
                Some("title") => "titleStyle",
                Some("body") => "bodyStyle",
                _ => "otherStyle",
            })
        });
        styles.extend(of_master);
        styles.extend(self.defaults);
        // The body's own settings, its placeholder's behind them.
        let mut props: Vec<&Node> = body.kid("bodyPr").into_iter().collect();
        props.extend(ancestors.iter().filter_map(|a| a.at(&["txBody", "bodyPr"])));
        let inset = |key: &str, default: f32| first_attr(&props, key).and_then(|v| v.parse::<f32>().ok()).map_or(default, |v| v / EMU as f32);
        let (l, t, r, b) = (inset("lIns", 7.2), inset("tIns", 3.6), inset("rIns", 7.2), inset("bIns", 3.6));
        let fit = props.iter().find_map(|p| p.kid("normAutofit"));
        let layout = Layout {
            theme: self.theme,
            fonts: self.fonts,
            styles,
            font_color,
            scale: fit.and_then(|f| f.num("fontScale")).map_or(1.0, |s| s as f32 / 100_000.0),
            tighter: fit.and_then(|f| f.num("lnSpcReduction")).map_or(0.0, |s| s as f32 / 100_000.0),
        };
        let wraps = first_attr(&props, "wrap") != Some("none");
        let lines = layout.lines(body, wraps.then_some((w - l - r).max(1.0)));
        if lines.iter().all(|l| l.pieces.is_empty()) {
            return;
        }
        let tall = height_of(&lines);
        let top = match first_attr(&props, "anchor") {
            Some("ctr") => t + (h - t - b - tall) / 2.0,
            Some("b") => h - b - tall,
            _ => t,
        };
        // (Unwrapped text is set about its box's side or middle.)
        let mut lines = lines;
        if !wraps {
            for line in &mut lines {
                line.room = (w - l - r - line.start).max(0.0);
            }
        }
        text_svg(&lines, l, top, out);
    }

    /// Where a shape is and how it's turned (`xfrm`): its place, its
    /// size, and the transform that puts its box there.
    fn placed(xfrm: &Node) -> Option<(f32, f32, String)> {
        let (off, ext) = (xfrm.kid("off")?, xfrm.kid("ext")?);
        let pt = |n: &Node, k: &str| (n.num(k).unwrap_or(0.0) / EMU) as f32;
        let (x, y, w, h) = (pt(off, "x"), pt(off, "y"), pt(ext, "cx"), pt(ext, "cy"));
        let mut t = format!("translate({x:.2} {y:.2})");
        if let Some(rot) = xfrm.num("rot").filter(|r| *r != 0.0) {
            let _ = write!(t, " rotate({:.3} {:.2} {:.2})", rot / 60_000.0, w / 2.0, h / 2.0);
        }
        let (fh, fv) = (xfrm.flag("flipH").unwrap_or(false), xfrm.flag("flipV").unwrap_or(false));
        if fh || fv {
            let _ = write!(t, " translate({:.2} {:.2}) scale({} {})", if fh { w } else { 0.0 }, if fv { h } else { 0.0 }, if fh { -1 } else { 1 }, if fv { -1 } else { 1 });
        }
        Some((w, h, t))
    }

    /// Every shape of a tree (`spTree`, a group), in order. `own`: the
    /// slide's (a layout's or master's placeholders aren't drawn: they
    /// say where a slide's go).
    fn shapes(&mut self, z: &mut Zip, part: &Part, tree: &Node, own: bool, out: &mut String) {
        for shape in tree.drawn() {
            if !own && placeholder(shape).is_some() {
                continue;
            }
            match shape.name.as_str() {
                "sp" | "cxnSp" => self.shape(z, part, shape, out),
                "pic" => {
                    let Some(pr) = shape.kid("spPr") else { continue };
                    let xfrm = pr.kid("xfrm").or_else(|| self.ancestors(shape).into_iter().find_map(|a| a.at(&["spPr", "xfrm"])));
                    let Some((w, h, transform)) = xfrm.and_then(Self::placed) else { continue };
                    let _ = write!(out, r#"<g transform="{transform}">"#);
                    if let Some(fill) = shape.kid("blipFill") {
                        self.picture(z, part, fill, w, h, out);
                    }
                    if let Some(stroke) = pr.kid("ln").and_then(|ln| self.stroke(ln, None)) {
                        let _ = write!(out, r#"<rect x="0" y="0" width="{w:.2}" height="{h:.2}" fill="none" {stroke}/>"#);
                    }
                    out.push_str("</g>");
                }
                "grpSp" => {
                    let Some(xfrm) = shape.at(&["grpSpPr", "xfrm"]) else {
                        self.shapes(z, part, shape, own, out);
                        continue;
                    };
                    let Some((w, h, transform)) = Self::placed(xfrm) else { continue };
                    // Its children are placed in a space of their own,
                    // which is its box.
                    let pt = |n: Option<&Node>, k: &str, default: f32| n.and_then(|n| n.num(k)).map_or(default, |v| (v / EMU) as f32);
                    let (cx, cy) = (pt(xfrm.kid("chOff"), "x", 0.0), pt(xfrm.kid("chOff"), "y", 0.0));
                    let (cw, ch) = (pt(xfrm.kid("chExt"), "cx", w), pt(xfrm.kid("chExt"), "cy", h));
                    let _ = write!(out, r#"<g transform="{transform} scale({:.5} {:.5}) translate({:.2} {:.2})">"#, if cw > 0.0 { w / cw } else { 1.0 }, if ch > 0.0 { h / ch } else { 1.0 }, -cx, -cy);
                    self.shapes(z, part, shape, own, out);
                    out.push_str("</g>");
                }
                "graphicFrame" => self.frame(shape, out),
                _ => {}
            }
        }
    }

    fn shape(&mut self, z: &mut Zip, part: &Part, shape: &Node, out: &mut String) {
        let ancestors = self.ancestors(shape);
        let pr = shape.kid("spPr");
        let prs: Vec<&Node> = pr.into_iter().chain(ancestors.iter().filter_map(|a| a.kid("spPr"))).collect();
        let Some((w, h, transform)) = prs.iter().find_map(|p| p.kid("xfrm")).and_then(Self::placed) else { return };
        let style = shape.kid("style");
        let style_color = |name: &str| style.and_then(|s| s.kid(name)).filter(|r| r.num("idx").is_none_or(|i| i > 0.0)).and_then(|r| self.theme.color_in(r, None));
        let (fill_ref, line_ref, font_ref) = (style_color("fillRef"), style_color("lnRef"), style.and_then(|s| s.kid("fontRef")).and_then(|r| self.theme.color_in(r, None)));
        let geom = prs.iter().find_map(|p| {
            p.kid("custGeom").and_then(|g| custom(g, w, h)).or_else(|| {
                let g = p.kid("prstGeom")?;
                let adj: HashMap<String, f32> =
                    g.kid("avLst").into_iter().flat_map(|l| l.all("gd")).filter_map(|gd| Some((gd.attr("name")?.to_owned(), gd.attr("fmla")?.strip_prefix("val ")?.trim().parse().ok()?))).collect();
                Some(outline(g.attr("prst").unwrap_or("rect"), w, h, &adj))
            })
        });
        let connector = shape.name == "cxnSp";
        let d = geom.unwrap_or_else(|| outline(if connector { "line" } else { "rect" }, w, h, &HashMap::new()));
        let open = !d.trim_end().ends_with('Z');
        let _ = write!(out, r#"<g transform="{transform}">"#);
        // Its fill: its own, its placeholder's, its style's; a picture's
        // is the picture, clipped to its outline.
        let picture_fill = prs.iter().find_map(|p| p.kid("blipFill"));
        let said = prs.iter().find(|p| p.kids.iter().any(|k| matches!(k.name.as_str(), "noFill" | "solidFill" | "gradFill" | "pattFill" | "blipFill" | "grpFill")));
        let fill = match said {
            _ if open => Some(r#"fill="none""#.to_owned()),
            Some(p) if picture_fill.is_some_and(|b| p.kid("blipFill").is_some_and(|x| std::ptr::eq(x, b))) => None,
            Some(p) => self.fill(p, fill_ref),
            None => fill_ref.map(|c| format!(r#"fill="{}" fill-opacity="{:.3}""#, c.hex(), c.alpha)),
        };
        let line = prs.iter().find_map(|p| p.kid("ln")).and_then(|ln| self.stroke(ln, line_ref)).or_else(|| {
            let c = line_ref?;
            let width = style.and_then(|s| s.kid("lnRef")).and_then(|r| r.num("idx")).map_or(0.75, |i| (i as f32 * 0.5).max(0.5));
            Some(format!(r#"stroke="{}" stroke-opacity="{:.3}" stroke-width="{width:.2}" stroke-linejoin="round""#, c.hex(), c.alpha))
        });
        if let (Some(b), None) = (picture_fill, &fill) {
            let id = self.id();
            let _ = write!(self.defs, r#"<clipPath id="{id}"><path d="{d}"/></clipPath>"#);
            let _ = write!(out, r#"<g clip-path="url(#{id})">"#);
            self.picture(z, part, b, w, h, out);
            out.push_str("</g>");
        }
        let (fill, line) = (fill.unwrap_or_else(|| r#"fill="none""#.into()), line.unwrap_or_else(|| r#"stroke="none""#.into()));
        // A line's arrowheads, at its ends.
        let (d, heads) = match prs.iter().find_map(|p| p.kid("ln")).filter(|_| open).and_then(|ln| Some((ln, self.line_look(ln, line_ref)?))) {
            Some((ln, (color, width))) => arrowheads(ln, &d, color, width),
            None => (d, String::new()),
        };
        if fill != r#"fill="none""# || line != r#"stroke="none""# {
            let _ = write!(out, r#"<path d="{d}" {fill} {line}/>"#);
        }
        out.push_str(&heads);
        if let Some(body) = shape.kid("txBody") {
            self.text(shape, body, w, h, font_ref, out);
        }
        out.push_str("</g>");
    }

    /// A table (other frames, charts and diagrams, are PowerPoint's to
    /// draw from their data: left out unless the file gave a picture in
    /// their place, which `drawn` takes).
    fn frame(&mut self, frame: &Node, out: &mut String) {
        let Some(table) = frame.at(&["graphic", "graphicData", "tbl"]) else { return };
        let Some((_, _, transform)) = frame.kid("xfrm").and_then(Self::placed) else { return };
        let columns: Vec<f32> = table.kid("tblGrid").into_iter().flat_map(|g| g.all("gridCol")).map(|c| (c.num("w").unwrap_or(0.0) / EMU) as f32).collect();
        let _ = write!(out, r#"<g transform="{transform}">"#);
        let mut y = 0.0;
        for row in table.all("tr") {
            let height = (row.num("h").unwrap_or(0.0) / EMU) as f32;
            let mut x = 0.0;
            let mut column = 0;
            for cell in row.all("tc") {
                let span = cell.num("gridSpan").map_or(1, |s| s as usize).max(1);
                let width: f32 = columns.iter().skip(column).take(span).sum();
                column += span;
                if cell.flag("hMerge") != Some(true) && cell.flag("vMerge") != Some(true) {
                    let pr = cell.kid("tcPr");
                    let fill = pr.and_then(|p| self.fill(p, None)).unwrap_or_else(|| r#"fill="none""#.into());
                    let line = pr
                        .and_then(|p| ["lnB", "lnT", "lnL", "lnR"].iter().find_map(|n| p.kid(n)))
                        .and_then(|ln| self.stroke(ln, None))
                        .unwrap_or_else(|| r##"stroke="#a0a0a0" stroke-width="0.5""##.into());
                    let _ = write!(out, r#"<g transform="translate({x:.2} {y:.2})"><rect x="0" y="0" width="{width:.2}" height="{height:.2}" {fill} {line}/>"#);
                    if let Some(body) = cell.kid("txBody") {
                        self.text(cell, body, width, height, None, out);
                    }
                    out.push_str("</g>");
                }
                x += width;
            }
            y += height;
        }
        out.push_str("</g>");
    }
}

// ---- the presentation -------------------------------------------------------

/// A presentation, open: its slides in order, and what every slide
/// shares.
pub(crate) struct Deck {
    zip: Zip,
    /// A slide's size, in points.
    pub size: (f32, f32),
    slides: Vec<String>,
    defaults: Option<Node>,
    fonts: Fonts,
    /// Layouts and masters read (each with its theme), by path.
    parts: HashMap<String, Arc<Part>>,
    themes: HashMap<String, Theme>,
}

impl Deck {
    pub fn open(path: &Path) -> Result<Deck, String> {
        let unreadable = || "This presentation can't be read.".to_string();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).map_err(|e| e.to_string())?).map_err(|_| unreadable())?;
        let top = Part::open(&mut zip, "ppt/presentation.xml").ok_or_else(unreadable)?;
        let size = top.root.kid("sldSz").map_or((960.0, 540.0), |s| ((s.num("cx").unwrap_or(12_192_000.0) / EMU) as f32, (s.num("cy").unwrap_or(6_858_000.0) / EMU) as f32));
        let slides: Vec<String> =
            top.root.kid("sldIdLst").into_iter().flat_map(|l| l.all("sldId")).filter_map(|s| top.rels.get(s.attr("r:id")?).map(|(_, p)| p.clone())).filter(|p| !p.is_empty()).collect();
        Ok(Deck { zip, size, slides, defaults: top.root.kid("defaultTextStyle").cloned(), fonts: Fonts::new(), parts: HashMap::new(), themes: HashMap::new() })
    }

    pub fn len(&self) -> usize {
        self.slides.len()
    }

    fn part(&mut self, path: &str) -> Option<Arc<Part>> {
        if let Some(p) = self.parts.get(path) {
            return Some(p.clone());
        }
        let part = Arc::new(Part::open(&mut self.zip, path)?);
        self.parts.insert(path.to_owned(), part.clone());
        Some(part)
    }

    /// The theme a master draws with: its theme's colours and fonts,
    /// named as the master's colour map names them.
    fn theme(&mut self, master: Option<&Part>, master_path: &str) -> Theme {
        if let Some(t) = self.themes.get(master_path) {
            return t.clone();
        }
        let mut theme = Theme::default();
        if let Some(text) = master.and_then(|m| m.linked("theme")).map(str::to_owned).and_then(|p| bytes(&mut self.zip, &p)) {
            theme.read(&tree(&String::from_utf8_lossy(&text)));
        }
        if let Some(map) = master.and_then(|m| m.root.kid("clrMap")) {
            theme.map = map.attrs.iter().cloned().collect();
        }
        self.themes.insert(master_path.to_owned(), theme.clone());
        theme
    }

    /// Slide `i` as an SVG drawing, its size the slide's in points.
    pub fn svg(&mut self, i: usize) -> Option<String> {
        let path = self.slides.get(i)?.clone();
        let slide = Part::open(&mut self.zip, &path)?;
        let layout = slide.linked("slideLayout").map(str::to_owned).and_then(|p| self.part(&p));
        let master_path = layout.as_ref().and_then(|l| l.linked("slideMaster")).map(str::to_owned).unwrap_or_default();
        let master = self.part(&master_path);
        let theme = self.theme(master.as_deref(), &master_path);
        let (w, h) = self.size;
        let defaults = self.defaults.clone();
        let mut draw = Draw { theme: &theme, fonts: &self.fonts, layout: layout.as_deref(), master: master.as_deref(), defaults: defaults.as_ref(), defs: String::new(), ids: 0 };
        let mut body = String::new();
        // Its background: its own, else its layout's, else its master's.
        let own_parts: [Option<&Part>; 3] = [Some(&slide), layout.as_deref(), master.as_deref()];
        let background = own_parts.iter().flatten().find_map(|p| Some((*p, p.root.at(&["cSld", "bg"])?)));
        let mut painted = false;
        if let Some((part, bg)) = background {
            if let Some(pr) = bg.kid("bgPr") {
                if let Some(picture) = pr.kid("blipFill") {
                    draw.picture(&mut self.zip, part, picture, w, h, &mut body);
                    painted = true;
                } else if let Some(fill) = draw.fill(pr, None) {
                    let _ = write!(body, r#"<rect x="0" y="0" width="{w:.2}" height="{h:.2}" {fill}/>"#);
                    painted = true;
                }
            } else if let Some(c) = bg.kid("bgRef").and_then(|r| theme.color_in(r, None)) {
                let _ = write!(body, r#"<rect x="0" y="0" width="{w:.2}" height="{h:.2}" fill="{}"/>"#, c.hex());
                painted = true;
            }
        }
        if !painted {
            let _ = write!(body, r##"<rect x="0" y="0" width="{w:.2}" height="{h:.2}" fill="#ffffff"/>"##);
        }
        // The master's and the layout's own shapes behind the slide's
        // (a layout or a slide may say the master's aren't shown).
        let shows_master = slide.root.flag("showMasterSp").unwrap_or(true) && layout.as_ref().is_none_or(|l| l.root.flag("showMasterSp").unwrap_or(true));
        if let (true, Some(m)) = (shows_master, master.as_deref())
            && let Some(tree) = m.root.at(&["cSld", "spTree"])
        {
            draw.shapes(&mut self.zip, m, tree, false, &mut body);
        }
        if let (true, Some(l)) = (slide.root.flag("showMasterSp").unwrap_or(true), layout.as_deref())
            && let Some(tree) = l.root.at(&["cSld", "spTree"])
        {
            draw.shapes(&mut self.zip, l, tree, false, &mut body);
        }
        if let Some(tree) = slide.root.at(&["cSld", "spTree"]) {
            draw.shapes(&mut self.zip, &slide, tree, true, &mut body);
        }
        Some(format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}" height="{h:.2}" viewBox="0 0 {w:.2} {h:.2}"><defs>{}</defs><clipPath id="page"><rect x="0" y="0" width="{w:.2}" height="{h:.2}"/></clipPath><g clip-path="url(#page)">{body}</g></svg>"#,
            draw.defs
        ))
    }

    /// Slide `i`'s words, its paragraphs one after another (for agents,
    /// who read a slide's text where people see its picture).
    pub fn words(&mut self, i: usize) -> String {
        fn gather(n: &Node, out: &mut Vec<String>) {
            if n.name == "p" {
                let line: String = n.kids.iter().filter(|r| matches!(r.name.as_str(), "r" | "fld")).filter_map(|r| r.kid("t")).map(|t| t.text.as_str()).collect();
                if !line.trim().is_empty() {
                    out.push(line.trim().to_owned());
                }
            } else {
                n.kids.iter().for_each(|k| gather(k, out));
            }
        }
        let mut out = Vec::new();
        if let Some(slide) = self.slides.get(i).cloned().and_then(|p| Part::open(&mut self.zip, &p))
            && let Some(tree) = slide.root.at(&["cSld", "spTree"])
        {
            gather(tree, &mut out);
        }
        out.join("\n")
    }

    /// Slide `i` drawn `tall` pixels high.
    #[cfg(test)]
    pub fn render(&mut self, i: usize, tall: f32) -> Option<Bitmap> {
        let k = (tall / self.size.1.max(1.0)).clamp(0.25, 8.0);
        self.render_to(i, (self.size.0 * k).ceil() as u32, (self.size.1 * k).ceil() as u32)
    }

    /// Slide `i` drawn to fill `w` × `h` pixels exactly (the pixels a
    /// view gives it: nothing resampled after).
    pub fn render_to(&mut self, i: usize, w: u32, h: u32) -> Option<Bitmap> {
        let svg = self.svg(i)?;
        let opt = resvg::usvg::Options { fontdb: self.fonts.db.clone(), ..Default::default() };
        let tree = resvg::usvg::Tree::from_str(&svg, &opt).ok()?;
        let (w, h) = (w.clamp(1, MOST_PX), h.clamp(1, MOST_PX));
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(w as f32 / self.size.0.max(1.0), h as f32 / self.size.1.max(1.0)), &mut pixmap.as_mut());
        Some(Bitmap::from_rgba_premultiplied([w as usize, h as usize], pixmap.take()))
    }
}

/// A presentation's slides as pages: the first at once, then whichever
/// the preview asks for.
pub fn load(path: &Path, cancel: &Arc<AtomicBool>, send: Send) {
    let mut deck = match Deck::open(path) {
        Ok(d) => d,
        Err(e) => return send(Update::Failed(e)),
    };
    let n = deck.len();
    if n == 0 {
        return send(Update::Failed("This presentation has no slides.".into()));
    }
    let (want, wanted) = mpsc::channel::<usize>();
    let note = format!("PowerPoint presentation, {n} slide{}", if n == 1 { "" } else { "s" });
    let words = (0..n).map(|i| deck.words(i)).collect();
    let px = Arc::new(AtomicU64::new(0));
    send(Update::Body(Body::Pages { pages: vec![None; n], page_size: deck.size, note, want, every: true, words, px: px.clone() }));
    // (Nothing drawn until the view asks: it says at what size.)
    let page = |deck: &mut Deck, i: usize| {
        let at = px.load(Ordering::Relaxed);
        let (w, h) = match ((at >> 32) as u32, at as u32) {
            (0, _) | (_, 0) => ((deck.size.0 * SLIDE_PX / deck.size.1.max(1.0)).ceil() as u32, SLIDE_PX as u32),
            size => size,
        };
        Arc::new(deck.render_to(i, w, h).unwrap_or_else(|| {
            let (w, h) = (w.clamp(1, MOST_PX) as usize, h.clamp(1, MOST_PX) as usize);
            Bitmap::from_rgba_premultiplied([w, h], vec![255; w * h * 4])
        }))
    };
    loop {
        // Waiting on the channel costs nothing; it ends when the view
        // that asks is gone.
        let Ok(first) = wanted.recv() else { return };
        // Everything asked for by now, each page once (a window being
        // resized asks for the same pages over and over).
        let mut asked = vec![first];
        asked.extend(wanted.try_iter());
        let mut seen = std::collections::HashSet::new();
        for i in asked.into_iter().rev().filter(|i| *i < n && seen.insert(*i)).collect::<Vec<_>>().into_iter().rev() {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            send(Update::Page(i, page(&mut deck, i)));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write as _;

    use super::*;

    const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

    fn rels(items: &[(&str, &str, &str)]) -> String {
        let mut s = String::from(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
        for (id, kind, target) in items {
            let _ = write!(s, r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#);
        }
        s + "</Relationships>"
    }

    /// A picture for the deck: a red square with a green quarter at its
    /// top left (a PNG).
    fn picture() -> Vec<u8> {
        let img = image::RgbImage::from_fn(64, 64, |x, y| if x < 32 && y < 32 { image::Rgb([0, 160, 0]) } else { image::Rgb([200, 0, 0]) });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    /// A small presentation, written at `path`: a theme, a master (a blue
    /// bar of its own, a title's and a body's placeholders and styles), a
    /// layout, and three slides: a title and bullets in the layout's
    /// placeholders; shapes, a line, a rotated box, a cropped picture and
    /// a table; and a slide that is one picture.
    pub(crate) fn deck(path: &Path) {
        let sp =
            |id: u32, ph: &str, xfrm: &str, rest: &str| format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="s{id}"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{xfrm}</p:spPr>{rest}</p:sp>"#);
        let at = |x: i64, y: i64, w: i64, h: i64| format!(r#"<a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm>"#, x * 12700, y * 12700, w * 12700, h * 12700);
        let tree = |shapes: &str| format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{shapes}</p:spTree></p:cSld>"#);
        let theme = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="T"><a:themeElements><a:clrScheme name="T"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F3864"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="2E75B6"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="T"><a:majorFont><a:latin typeface="Noto Sans"/></a:majorFont><a:minorFont><a:latin typeface="Noto Sans"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;
        let master = format!(
            r#"<p:sldMaster {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{}{}{}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:txStyles><p:titleStyle><a:lvl1pPr algn="l"><a:defRPr sz="3600" b="1"><a:solidFill><a:schemeClr val="tx2"/></a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL="228600" indent="-228600"><a:buChar char="&#8226;"/><a:defRPr sz="2400"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:defRPr></a:lvl1pPr><a:lvl2pPr marL="571500" indent="-228600"><a:buChar char="&#8211;"/><a:defRPr sz="2000"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:defRPr></a:lvl2pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#,
            sp(2, "", &at(0, 520, 960, 20), r#"<p:spPr/>"#)
                .replace("<p:spPr/></p:sp>", "</p:sp>")
                .replace("</p:spPr></p:sp>", r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></p:spPr></p:sp>"#),
            sp(3, r#"<p:ph type="title"/>"#, &at(48, 30, 864, 80), r#"<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:r><a:t>Master title</a:t></a:r></a:p></p:txBody>"#),
            sp(4, r#"<p:ph type="body" idx="1"/>"#, &at(48, 130, 864, 360), r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Master text</a:t></a:r></a:p></p:txBody>"#),
        );
        let layout = format!(
            r#"<p:sldLayout {NS}>{}</p:sldLayout>"#,
            tree(&format!(
                "{}{}",
                sp(2, r#"<p:ph type="title"/>"#, "", r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Layout title</a:t></a:r></a:p></p:txBody>"#),
                sp(3, r#"<p:ph idx="1"/>"#, "", r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Layout text</a:t></a:r></a:p></p:txBody>"#)
            ))
        );
        let slide1 = format!(
            r#"<p:sld {NS}>{}</p:sld>"#,
            tree(&format!(
                "{}{}",
                sp(2, r#"<p:ph type="title"/>"#, "", r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Quarterly review</a:t></a:r></a:p></p:txBody>"#),
                sp(
                    3,
                    r#"<p:ph idx="1"/>"#,
                    "",
                    r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Revenue grew in every region, with the north ahead of its plan for the third quarter running</a:t></a:r></a:p><a:p><a:pPr lvl="1"/><a:r><a:t>North: </a:t></a:r><a:r><a:rPr b="1"><a:solidFill><a:srgbClr val="C00000"/></a:solidFill></a:rPr><a:t>up 12%</a:t></a:r></a:p><a:p><a:r><a:t>Two new offices</a:t></a:r></a:p></p:txBody>"#
                )
            ))
        );
        let shape = |id: u32, xfrm: &str, geom: &str, fill: &str, text: &str| {
            format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="s{id}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{xfrm}<a:prstGeom prst="{geom}"><a:avLst/></a:prstGeom>{fill}</p:spPr>{text}</p:sp>"#)
        };
        let label = |words: &str| {
            format!(
                r#"<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr sz="1400"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></a:rPr><a:t>{words}</a:t></a:r></a:p></p:txBody>"#
            )
        };
        let slide2 = format!(
            r#"<p:sld {NS}>{}</p:sld>"#,
            tree(&format!(
                "{}{}{}{}{}{}{}{}{}",
                shape(2, &at(40, 40, 200, 100), "rect", r#"<a:solidFill><a:srgbClr val="C00000"/></a:solidFill>"#, &label("A box")),
                shape(
                    3,
                    &at(280, 40, 200, 100),
                    "roundRect",
                    r#"<a:gradFill><a:gsLst><a:gs pos="0"><a:schemeClr val="accent1"/></a:gs><a:gs pos="100000"><a:schemeClr val="accent6"/></a:gs></a:gsLst><a:lin ang="0"/></a:gradFill>"#,
                    &label("Rounded")
                ),
                shape(
                    4,
                    &at(520, 40, 160, 100),
                    "ellipse",
                    r#"<a:solidFill><a:schemeClr val="accent2"><a:alpha val="60000"/></a:schemeClr></a:solidFill><a:ln w="38100"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln>"#,
                    ""
                ),
                shape(5, &at(720, 60, 180, 60), "rightArrow", r#"<a:solidFill><a:schemeClr val="accent4"/></a:solidFill>"#, ""),
                r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="6" name="l"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="508000" y="2286000"/><a:ext cx="5080000" cy="0"/></a:xfrm><a:prstGeom prst="line"><a:avLst/></a:prstGeom><a:ln w="25400"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill><a:prstDash val="dash"/></a:ln></p:spPr></p:cxnSp>"#,
                // A connector with an elbow and an arrowhead at its end.
                r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="10" name="c"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="7620000" y="3810000"/><a:ext cx="2540000" cy="1270000"/></a:xfrm><a:prstGeom prst="bentConnector3"><a:avLst/></a:prstGeom><a:ln w="38100"><a:solidFill><a:srgbClr val="7030A0"/></a:solidFill><a:tailEnd type="triangle"/></a:ln></p:spPr></p:cxnSp>"#,
                r#"<p:sp><p:nvSpPr><p:cNvPr id="7" name="t"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="-900000"><a:off x="6096000" y="2540000"/><a:ext cx="2540000" cy="508000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="none"/><a:lstStyle/><a:p><a:r><a:rPr sz="2000" i="1"/><a:t>Turned a little</a:t></a:r></a:p></p:txBody></p:sp>"#,
                r#"<p:pic><p:nvPicPr><p:cNvPr id="8" name="p"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:srcRect l="50000"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="508000" y="2794000"/><a:ext cx="1270000" cy="2540000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#,
                r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="9" name="tb"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="2540000" y="3048000"/><a:ext cx="3048000" cy="762000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid><a:gridCol w="1524000"/><a:gridCol w="1524000"/></a:tblGrid><a:tr h="381000"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr sz="1400" b="1"/><a:t>Region</a:t></a:r></a:p></a:txBody><a:tcPr><a:solidFill><a:schemeClr val="lt2"/></a:solidFill></a:tcPr></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr sz="1400" b="1"/><a:t>Growth</a:t></a:r></a:p></a:txBody><a:tcPr><a:solidFill><a:schemeClr val="lt2"/></a:solidFill></a:tcPr></a:tc></a:tr><a:tr h="381000"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr sz="1400"/><a:t>North</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr sz="1400"/><a:t>12%</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
            ))
        );
        let slide3 = format!(
            r#"<p:sld {NS}>{}</p:sld>"#,
            tree(
                r#"<p:pic><p:nvPicPr><p:cNvPr id="2" name="p"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="12192000" cy="6858000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#
            )
        );
        let presentation = format!(
            r#"<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/><p:sldId id="258" r:id="rId4"/></p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/></p:presentation>"#
        );
        let slide_rels =
            |picture: bool| rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")].iter().copied().chain(picture.then_some(("rId2", "image", "../media/image1.png"))).collect::<Vec<_>>());
        let files: Vec<(&str, Vec<u8>)> = vec![
            ("ppt/presentation.xml", presentation.into_bytes()),
            (
                "ppt/_rels/presentation.xml.rels",
                rels(&[
                    ("rId1", "slideMaster", "slideMasters/slideMaster1.xml"),
                    ("rId2", "slide", "slides/slide1.xml"),
                    ("rId3", "slide", "slides/slide2.xml"),
                    ("rId4", "slide", "slides/slide3.xml"),
                ])
                .into_bytes(),
            ),
            ("ppt/theme/theme1.xml", theme.as_bytes().to_vec()),
            ("ppt/slideMasters/slideMaster1.xml", master.into_bytes()),
            ("ppt/slideMasters/_rels/slideMaster1.xml.rels", rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"), ("rId2", "theme", "../theme/theme1.xml")]).into_bytes()),
            ("ppt/slideLayouts/slideLayout1.xml", layout.into_bytes()),
            ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", rels(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]).into_bytes()),
            ("ppt/slides/slide1.xml", slide1.into_bytes()),
            ("ppt/slides/_rels/slide1.xml.rels", slide_rels(false).into_bytes()),
            ("ppt/slides/slide2.xml", slide2.into_bytes()),
            ("ppt/slides/_rels/slide2.xml.rels", slide_rels(true).into_bytes()),
            ("ppt/slides/slide3.xml", slide3.into_bytes()),
            ("ppt/slides/_rels/slide3.xml.rels", slide_rels(true).into_bytes()),
            ("ppt/media/image1.png", picture()),
        ];
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in files {
            z.start_file(name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
            z.write_all(&data).unwrap();
        }
        z.finish().unwrap();
    }

    fn made() -> (Deck, std::path::PathBuf) {
        // (On disk: /tmp is memory here.)
        let dir = std::path::Path::new("/var/tmp").join(format!("olo-pptx-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        deck(&dir.join("deck.pptx"));
        (Deck::open(&dir.join("deck.pptx")).unwrap(), dir)
    }

    /// A slide's text comes out as the file sets it: a title and a body
    /// in the layout's placeholders, placed and styled by the master
    /// (the layout's and master's own sample words never shown), bullets
    /// by level, a run's own colour and weight, long lines wrapped.
    #[test]
    fn a_slides_text_as_its_master_sets_it() {
        let (mut d, dir) = made();
        assert_eq!((d.len(), d.size), (3, (960.0, 540.0)));
        let svg = d.svg(0).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        for gone in ["Master title", "Master text", "Layout title", "Layout text"] {
            assert!(!svg.contains(gone), "{gone} is a placeholder's sample");
        }
        // The title: the master's place (48, 30), size, weight and colour.
        assert!(svg.contains(r##"<g transform="translate(48.00 30.00)">"##), "{svg}");
        let title = svg.split("<text").find(|t| t.contains("Quarterly")).unwrap();
        assert!(title.contains(r##"font-size="36.00""##) && title.contains(r##"fill="#1f3864""##) && title.contains("font-weight=\"bold\""), "{title}");
        // The body: bullets of each level, and the long line in two.
        let texts: Vec<&str> = svg.split("<text").skip(1).collect();
        assert_eq!(texts.iter().filter(|t| t.contains(">•</tspan>")).count(), 2);
        assert_eq!(texts.iter().filter(|t| t.contains(">–</tspan>")).count(), 1);
        let first = texts.iter().position(|t| t.contains(">Revenue</tspan>")).unwrap();
        assert!(!texts[first].contains("running") && texts[first + 1].contains(">running</tspan>"), "the long line wraps: {}", texts[first]);
        assert!(texts[first].contains(r##"font-size="24.00""##));
        let run = texts.iter().find(|t| t.contains(">12%</tspan>")).unwrap();
        assert!(run.contains(r##"font-size="20.00" fill="#c00000""##) && run.contains(">North:</tspan>"), "{run}");
    }

    /// Shapes, a cropped picture and a slide that is one picture, drawn:
    /// the colours where they are.
    #[test]
    fn shapes_and_pictures_drawn() {
        let (mut d, dir) = made();
        let slide = d.render(1, 540.0).unwrap();
        let whole = d.render(2, 540.0).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(slide.size, [960, 540]);
        let at = |b: &Bitmap, x: usize, y: usize| -> [u8; 3] { b.rgba[(y * b.size[0] + x) * 4..][..3].try_into().unwrap() };
        let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 6);
        // The box, away from its words; the page beside it; the master's bar.
        assert!(near(at(&slide, 60, 60), [0xc0, 0, 0]), "{:?}", at(&slide, 60, 60));
        assert!(near(at(&slide, 20, 20), [255, 255, 255]));
        assert!(near(at(&slide, 480, 530), [0x2e, 0x75, 0xb6]), "{:?}", at(&slide, 480, 530));
        // The gradient: the theme's blue at its left going to its green.
        let (left, right) = (at(&slide, 290, 90), at(&slide, 470, 90));
        assert!(left[2] > left[1] && right[1] > right[2], "{left:?} {right:?}");
        // The ellipse's middle (60% of the orange over white) and its outline.
        assert!(near(at(&slide, 600, 90), [0xf4, 0xb1, 0x83]), "{:?}", at(&slide, 600, 90));
        // The arrow's shaft; the dashed line's colour somewhere along it.
        assert!(near(at(&slide, 760, 90), [0xff, 0xc0, 0]));
        assert!((40..440).any(|x| near(at(&slide, x, 180), [0, 0, 255])));
        // The connector (600, 300 to 800, 400): its elbow half way, its
        // arrowhead at its end and nothing past the point.
        let purple = [0x70, 0x30, 0xa0];
        assert!(near(at(&slide, 650, 300), purple) && near(at(&slide, 700, 350), purple) && near(at(&slide, 750, 400), purple), "{:?}", at(&slide, 700, 350));
        assert!(near(at(&slide, 794, 400), purple) && near(at(&slide, 792, 402), purple), "its head: {:?}", at(&slide, 792, 402));
        assert!(near(at(&slide, 760, 403), [255, 255, 255]) && near(at(&slide, 803, 400), [255, 255, 255]));
        // The picture, its left half cropped away: no green in it.
        for (x, y) in [(45, 225), (90, 300), (135, 415)] {
            assert!(near(at(&slide, x, y), [200, 0, 0]), "{x},{y}: {:?}", at(&slide, x, y));
        }
        // The table's first row is filled; its words are dark.
        assert!(near(at(&slide, 310, 268), [0xe7, 0xe6, 0xe6]), "{:?}", at(&slide, 310, 268));
        // The slide that is one picture: green in its top left quarter, red elsewhere, over the master's bar.
        assert!(near(at(&whole, 200, 120), [0, 160, 0]) && near(at(&whole, 700, 120), [200, 0, 0]) && near(at(&whole, 480, 530), [200, 0, 0]));
    }

    /// Colours as the files say them: the theme's by the master's names,
    /// lightened, darkened and seen through.
    #[test]
    fn colours() {
        let mut theme = Theme::default();
        theme.colors.insert("dk1".into(), [0, 0, 0]);
        theme.colors.insert("accent1".into(), [0x2e, 0x75, 0xb6]);
        theme.map.insert("tx1".into(), "dk1".into());
        let c = |xml: &str| theme.color(&tree(xml), None).map(|c| (c.hex(), (c.alpha * 100.0).round() as u32));
        assert_eq!(c(r#"<srgbClr val="FF8000"/>"#), Some(("#ff8000".into(), 100)));
        assert_eq!(c(r#"<schemeClr val="tx1"/>"#), Some(("#000000".into(), 100)));
        assert_eq!(c(r#"<schemeClr val="accent1"><alpha val="50000"/></schemeClr>"#), Some(("#2e75b6".into(), 50)));
        assert_eq!(c(r#"<schemeClr val="tx1"><lumMod val="50000"/><lumOff val="50000"/></schemeClr>"#), Some(("#808080".into(), 100)));
        assert_eq!(c(r#"<srgbClr val="808080"><shade val="50000"/></srgbClr>"#), Some(("#404040".into(), 100)));
        assert_eq!(c(r#"<prstClr val="white"/>"#), Some(("#ffffff".into(), 100)));
        assert_eq!(c(r#"<schemeClr val="phClr"/>"#), None);
        // A line's ends: a filled head, and the line stopping inside it.
        let ln = tree(r#"<ln><tailEnd type="triangle"/><headEnd type="oval" w="sm" len="sm"/></ln>"#);
        let (d, heads) = arrowheads(&ln, "M0 0 L100.00 0.00", Color { rgb: [0, 0, 0], alpha: 1.0 }, 2.0);
        assert_eq!(d, "M0.00 0.00 L94.60 0.00");
        assert!(heads.contains(r#"<polygon points="100.00,0.00 94.00,3.00 94.00,-3.00""#) && heads.contains(r#"<ellipse cx="0.00" cy="0.00" rx="2.00" ry="2.00""#), "{heads}");
        // (A curve keeps its path; a closed shape has no ends.)
        assert_eq!(arrowheads(&ln, "M0 0 C50 0 50 40 100 40", Color { rgb: [0, 0, 0], alpha: 1.0 }, 2.0).0, "M0 0 C50 0 50 40 100 40");
        assert_eq!(arrowheads(&ln, "M0 0 L10 0 L10 10 Z", Color { rgb: [0, 0, 0], alpha: 1.0 }, 2.0).1, "");
        assert_eq!(resolve("ppt/slides/slide1.xml", "../media/image1.png"), "ppt/media/image1.png");
        assert_eq!(resolve("ppt/presentation.xml", "slides/slide2.xml"), "ppt/slides/slide2.xml");
        assert_eq!(resolve("ppt/slides/slide1.xml", "/ppt/media/a.png"), "ppt/media/a.png");
    }

    /// The deck written and its slides drawn as pictures beside it, to
    /// look at (`OLO_FIXTURE_PPTX`: where; or `OLO_PPTX`: a presentation
    /// of one's own to draw, its pictures in `OLO_PPTX_OUT`). `cargo
    /// test --release -p olo-preview pptx_fixture -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn pptx_fixture() {
        let (file, out) = match (std::env::var_os("OLO_PPTX"), std::env::var_os("OLO_FIXTURE_PPTX")) {
            (Some(own), _) => (std::path::PathBuf::from(own), std::path::PathBuf::from(std::env::var_os("OLO_PPTX_OUT").expect("OLO_PPTX_OUT"))),
            (None, Some(made)) => {
                let made = std::path::PathBuf::from(made);
                deck(&made);
                (made.clone(), made.parent().unwrap().to_owned())
            }
            _ => return,
        };
        let mut d = Deck::open(&file).unwrap();
        for i in 0..d.len() {
            let t = std::time::Instant::now();
            // (`OLO_PPTX_SIZE=WxH`: at that size exactly, as a view asks.)
            let size = std::env::var("OLO_PPTX_SIZE").ok().and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))));
            let b = match size {
                Some((w, h)) => d.render_to(i, w, h).unwrap(),
                None => d.render(i, 900.0).unwrap(),
            };
            let all = t.elapsed().as_secs_f64() * 1e3;
            // (Where the time goes: the slide written as SVG, and that read.)
            let t = std::time::Instant::now();
            let svg = d.svg(i).unwrap();
            let written = t.elapsed().as_secs_f64() * 1e3;
            let t = std::time::Instant::now();
            let opt = resvg::usvg::Options { fontdb: d.fonts.db.clone(), ..Default::default() };
            let _ = resvg::usvg::Tree::from_str(&svg, &opt).unwrap();
            let read = t.elapsed().as_secs_f64() * 1e3;
            println!("slide {}: {}×{} in {all:.0} ms (its SVG, {} KB, written in {written:.0} ms and read in {read:.0} ms)", i + 1, b.size[0], b.size[1], svg.len() / 1024);
            image::RgbaImage::from_raw(b.size[0] as u32, b.size[1] as u32, b.rgba.clone()).unwrap().save(out.join(format!("slide-{}.png", i + 1))).unwrap();
        }
    }
}
