//! Pages to PDF with krilla: fonts found by fontdb (Olo's own for text
//! that names none), embedded and subset; text stays text.

use std::collections::HashMap;

use krilla::{
    Document,
    geom::{PathBuilder, Point},
    num::NormalizedF32,
    page::PageSettings,
    paint::{Fill, Stroke},
    text::{Font as KFont, TextDirection},
};

use crate::{Font, Item, Page, Rgb};

/// A face loaded: for krilla, its data (to see which characters it has
/// and how wide they are), and whether it is drawn bold by stroking it
/// (asked for bold, and lighter than that: a variable font's default).
#[derive(Clone)]
struct Face {
    font: KFont,
    data: std::sync::Arc<Vec<u8>>,
    index: u32,
    fake_bold: bool,
}

impl Face {
    fn has(&self, c: char) -> bool {
        ttf_parser::Face::parse(&self.data, self.index).is_ok_and(|f| f.glyph_index(c).is_some())
    }

    /// How far `text` runs at `size` (its characters' advances).
    fn advance(&self, text: &str, size: f32) -> f32 {
        let Ok(f) = ttf_parser::Face::parse(&self.data, self.index) else { return 0.0 };
        let em = f.units_per_em() as f32;
        text.chars().filter_map(|c| f.glyph_index(c).and_then(|g| f.glyph_hor_advance(g))).map(|a| a as f32 / em * size).sum()
    }
}

/// Fonts by what pages ask for, loaded once each, and for characters
/// they lack, a face that has them.
struct Fonts {
    db: fontdb::Database,
    loaded: HashMap<Font, Option<Face>>,
    by_id: HashMap<fontdb::ID, Option<Face>>,
    fallback: HashMap<char, Option<fontdb::ID>>,
}

impl Fonts {
    fn new() -> Fonts {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Fonts { db, loaded: HashMap::new(), by_id: HashMap::new(), fallback: HashMap::new() }
    }

    fn face(&mut self, id: fontdb::ID, bold: bool) -> Option<Face> {
        if let Some(f) = self.by_id.get(&id) {
            return f.clone().map(|f| Face { fake_bold: bold && f.fake_bold, ..f });
        }
        let weight = self.db.face(id).map_or(400, |f| f.weight.0);
        let face = self
            .db
            .with_face_data(id, |data, index| {
                let data = std::sync::Arc::new(data.to_vec());
                let font = KFont::new(data.clone().into(), index)?;
                Some(Face { font, data, index, fake_bold: weight < 600 })
            })
            .flatten();
        self.by_id.insert(id, face.clone());
        face.map(|f| Face { fake_bold: bold && f.fake_bold, ..f })
    }

    fn get(&mut self, f: &Font) -> Option<Face> {
        if let Some(k) = self.loaded.get(f) {
            return k.clone();
        }
        let names: Vec<String> = if f.mono {
            olo_theme::fonts::MONO.iter().map(|s| s.to_string()).collect()
        } else if f.family.is_empty() {
            olo_theme::fonts::UI.iter().map(|s| s.to_string()).collect()
        } else {
            std::iter::once(f.family.clone()).chain(olo_theme::fonts::UI.iter().map(|s| s.to_string())).collect()
        };
        let query = |db: &fontdb::Database, name: &str| {
            let families = [fontdb::Family::Name(name)];
            db.query(&fontdb::Query {
                families: &families,
                weight: fontdb::Weight(if f.bold { 700 } else { 400 }),
                stretch: fontdb::Stretch::Normal,
                style: if f.italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
            })
        };
        let id = names.iter().find_map(|n| query(&self.db, n)).or_else(|| {
            let families = [if f.mono { fontdb::Family::Monospace } else { fontdb::Family::SansSerif }];
            self.db.query(&fontdb::Query { families: &families, ..Default::default() })
        });
        let face = id.and_then(|id| self.face(id, f.bold));
        self.loaded.insert(f.clone(), face.clone());
        face
    }

    /// A face with `c` in it, for text whose own font lacks it.
    fn for_char(&mut self, c: char, bold: bool) -> Option<Face> {
        let id = match self.fallback.get(&c) {
            Some(id) => *id,
            None => {
                // The usual places first, then every face.
                let preferred = ["Noto Sans", "DejaVu Sans", "Noto Sans Symbols", "Noto Sans Symbols 2", "Noto Sans Math", "Symbola"];
                let covers = |db: &fontdb::Database, id: fontdb::ID| db.with_face_data(id, |d, i| ttf_parser::Face::parse(d, i).is_ok_and(|f| f.glyph_index(c).is_some())).unwrap_or(false);
                let mut found = None;
                for name in preferred {
                    let families = [fontdb::Family::Name(name)];
                    if let Some(id) = self.db.query(&fontdb::Query { families: &families, ..Default::default() })
                        && covers(&self.db, id)
                    {
                        found = Some(id);
                        break;
                    }
                }
                if found.is_none() {
                    let ids: Vec<fontdb::ID> = self.db.faces().map(|f| f.id).collect();
                    found = ids.into_iter().find(|id| covers(&self.db, *id));
                }
                self.fallback.insert(c, found);
                found
            }
        };
        id.and_then(|id| self.face(id, bold))
    }
}

fn fill(c: Rgb) -> Fill {
    Fill { paint: krilla::color::rgb::Color::new(c[0], c[1], c[2]).into(), opacity: NormalizedF32::ONE, rule: Default::default() }
}

/// The pages as a PDF document titled `title`.
pub fn pdf(pages: &[Page], title: &str) -> Result<Vec<u8>, String> {
    let mut fonts = Fonts::new();
    let mut doc = Document::new();
    doc.set_metadata(krilla::metadata::Metadata::new().title(title.to_owned()).creator("Olo".to_owned()));
    for p in pages {
        let settings = PageSettings::new(p.width.max(1.0), p.height.max(1.0));
        let mut page = doc.start_page_with(settings);
        let mut s = page.surface();
        for item in &p.items {
            match item {
                Item::Text { x, y, text, font, size, color, width, underline, strike } => {
                    let Some(primary) = fonts.get(font) else { continue };
                    // Runs of characters by the face that has them.
                    let mut runs: Vec<(Face, String)> = Vec::new();
                    for c in text.chars() {
                        let face = if primary.has(c) { primary.clone() } else { fonts.for_char(c, font.bold).unwrap_or_else(|| primary.clone()) };
                        match runs.last_mut() {
                            Some((f, t)) if f.data.as_ptr() == face.data.as_ptr() && f.index == face.index => t.push(c),
                            _ => runs.push((face, c.to_string())),
                        }
                    }
                    let mut at = *x;
                    for (face, t) in &runs {
                        s.set_fill(Some(fill(*color)));
                        // Bold from a face without a bold weight: stroked
                        // too (still text).
                        if face.fake_bold {
                            s.set_stroke(Some(Stroke { paint: krilla::color::rgb::Color::new(color[0], color[1], color[2]).into(), width: size * 0.035, ..Default::default() }));
                        } else {
                            s.set_stroke(None);
                        }
                        s.draw_text(Point::from_xy(at, *y), face.font.clone(), *size, t, false, TextDirection::Auto);
                        at += face.advance(t, *size);
                    }
                    s.set_stroke(None);
                    // Lines under and through, as wide as the text runs.
                    if *underline || *strike {
                        let w = *width;
                        let t = (*size / 14.0).max(0.5);
                        for (on, dy) in [(*underline, size * 0.12), (*strike, -size * 0.28)] {
                            if on {
                                rect(&mut s, *x, y + dy, w, t, *color);
                            }
                        }
                    }
                }
                Item::Rect { x, y, w, h, color } => rect(&mut s, *x, *y, *w, *h, *color),
                Item::Line { from, to, width, color } => {
                    let mut b = PathBuilder::new();
                    b.move_to(from.0, from.1);
                    b.line_to(to.0, to.1);
                    if let Some(path) = b.finish() {
                        s.set_fill(None);
                        s.set_stroke(Some(Stroke { paint: krilla::color::rgb::Color::new(color[0], color[1], color[2]).into(), width: *width, ..Default::default() }));
                        s.draw_path(&path);
                    }
                }
            }
        }
        s.finish();
        page.finish();
    }
    doc.finish().map_err(|e| format!("The PDF couldn't be made ({e:?})."))
}

fn rect(s: &mut krilla::surface::Surface, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
    let mut b = PathBuilder::new();
    if let Some(r) = krilla::geom::Rect::from_xywh(x, y, w.max(0.01), h.max(0.01)) {
        b.push_rect(r);
    }
    if let Some(path) = b.finish() {
        s.set_stroke(None);
        s.set_fill(Some(fill(color)));
        s.draw_path(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_a_pdf() {
        let page = Page {
            width: 612.0,
            height: 792.0,
            items: vec![
                Item::Text { x: 72.0, y: 100.0, text: "Hello — Olo".into(), font: Font::default(), size: 24.0, color: [0, 0, 0], width: 150.0, underline: true, strike: false },
                Item::Text { x: 72.0, y: 140.0, text: "code()".into(), font: Font { mono: true, ..Font::default() }, size: 12.0, color: [0, 0, 0], width: 40.0, underline: false, strike: false },
                Item::Rect { x: 72.0, y: 150.0, w: 100.0, h: 1.0, color: [200, 0, 0] },
            ],
        };
        let out = pdf(&[page], "Test").unwrap();
        assert!(out.starts_with(b"%PDF-"));
        if let Some(dir) = std::env::var_os("OLO_PRINT_OUT") {
            std::fs::write(std::path::Path::new(&dir).join("test.pdf"), &out).unwrap();
        }
    }
}
