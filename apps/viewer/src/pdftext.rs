//! A PDF page's text, read natively: its glyphs as hayro's interpreter
//! draws them (a [`Device`] that keeps each glyph's Unicode and box
//! instead of painting: the font's ToUnicode map, else its glyph names),
//! in the order the page draws them, with where a space, a line break or
//! a paragraph's goes worked out from where each glyph sits against the
//! last (as a person reading it would put them). OCR'd scans' invisible
//! text counts. Boxes are in the page's rendered points, y down (as
//! `PageGeom` has them).

use hayro::{
    hayro_interpret::{
        BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps, InterpreterCache, InterpreterSettings, SoftMask, TransformExt as _,
        font::{Glyph, GlyphRun},
        hayro_cmap::BfString,
        interpret_page,
    },
    hayro_syntax::page::Page,
    kurbo::{Affine, BezPath, Point, Rect, Vec2},
};

/// What goes after a character when the text is copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    Nothing,
    Space,
    Line,
    Paragraph,
}

impl After {
    fn text(self) -> &'static str {
        match self {
            After::Nothing => "",
            After::Space => " ",
            After::Line => "\n",
            After::Paragraph => "\n\n",
        }
    }
}

/// A character on the page (a glyph's: a ligature's is two or three).
#[derive(Clone, Debug, PartialEq)]
pub struct TextChar {
    pub text: Box<str>,
    /// x0, y0, x1, y1 in rendered points, y down.
    pub rect: [f32; 4],
    /// Which line of the page it's on (from 0).
    pub line: u32,
    pub after: After,
}

/// A page's text: its characters, and the text they make (what's copied
/// and searched), with where each character's part of it starts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    pub chars: Vec<TextChar>,
    pub string: String,
    /// Each character's byte offset in `string` (and one past the last).
    pub offsets: Vec<usize>,
    /// Each line: its characters (as carets), its box and its text (as
    /// agents read the page).
    pub lines: Vec<TextLine>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub start: usize,
    pub end: usize,
    pub rect: [f32; 4],
    pub text: std::sync::Arc<str>,
}

/// A glyph as drawn.
struct Drawn {
    text: String,
    rect: [f32; 4],
    /// Its origin on the baseline and where the next would go.
    origin: Point,
    end: Point,
    /// The baseline's direction (unit), and the em in points.
    dir: Vec2,
    size: f64,
}

#[derive(Default)]
struct Collector {
    glyphs: Vec<Drawn>,
}

impl<'a> Device<'a> for Collector {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for g in run.glyphs() {
            let text = match g.as_unicode() {
                Some(BfString::Char(c)) => unligature(c).map_or_else(|| c.to_string(), str::to_owned),
                Some(BfString::String(s)) => s.chars().map(|c| unligature(c).map_or_else(|| c.to_string(), str::to_owned)).collect(),
                None => continue,
            };
            // Glyph space is 1000 units to the em.
            let advance = match &**g {
                Glyph::Outline(o) => o.advance_width().filter(|a| *a > 0.0).unwrap_or(500.0) as f64,
                Glyph::Type3(_) => 500.0,
            };
            let t: Affine = props.transform * g.transform();
            let corners = [Point::new(0.0, -250.0), Point::new(advance, -250.0), Point::new(advance, 850.0), Point::new(0.0, 850.0)].map(|p| t * p);
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for c in corners {
                (x0, y0, x1, y1) = (x0.min(c.x), y0.min(c.y), x1.max(c.x), y1.max(c.y));
            }
            let [a, b, c, d, ..] = t.as_coeffs();
            let along = Vec2::new(a, b) * 1000.0;
            let up = Vec2::new(c, d) * 1000.0;
            let size = up.hypot();
            if !size.is_finite() || size < 0.01 || !(x0.is_finite() && y1.is_finite()) {
                continue;
            }
            let origin = t * Point::ORIGIN;
            let dir = if along.hypot() > 0.0 { along / along.hypot() } else { Vec2::new(1.0, 0.0) };
            self.glyphs.push(Drawn { text, rect: [x0 as f32, y0 as f32, x1 as f32, y1 as f32], origin, end: t * Point::new(advance, 0.0), dir, size });
        }
    }
}

/// A ligature's letters (ﬁ: "fi"), as they're pasted and searched.
fn unligature(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{FB05}' | '\u{FB06}' => "st",
        _ => return None,
    })
}

/// `page`'s text (its rendered points, as at a scale of 1).
pub fn read<'a>(page: &'a Page<'a>, cache: &InterpreterCache<'a>) -> PageText {
    let transform = page.initial_transform(true).to_kurbo();
    let (w, h) = page.render_dimensions();
    let mut device = Collector::default();
    let mut ctx = Context::new(transform, Rect::new(0.0, 0.0, w as f64, h as f64), cache, page.xref(), InterpreterSettings::default());
    interpret_page(page, &mut ctx, &mut device);
    arrange(device.glyphs)
}

/// The glyphs as text: spaces dropped for the gaps they leave, and a
/// space, line break or paragraph between glyphs from where each sits
/// against the one before.
fn arrange(glyphs: Vec<Drawn>) -> PageText {
    let mut chars: Vec<TextChar> = Vec::new();
    let mut last: Option<&Drawn> = None;
    let mut line = 0u32;
    for g in &glyphs {
        let blank = g.text.trim().is_empty();
        if let Some(prev) = last {
            let size = prev.size.min(g.size).max(0.01);
            // Across the baseline (the normal to it) and along it.
            let normal = Vec2::new(-prev.dir.y, prev.dir.x);
            let off = g.origin - prev.origin;
            let across = off.dot(normal).abs();
            let gap = (g.origin - prev.end).dot(prev.dir);
            let after = if across > 1.8 * size {
                After::Paragraph
            } else if across > 0.5 * size || gap < -2.0 * size {
                After::Line
            } else if gap > 0.1 * size {
                // (A word space set tight in a justified line is a fifth
                // of an em; kerning, far under a tenth.)
                After::Space
            } else {
                After::Nothing
            };
            if let Some(c) = chars.last_mut() {
                // (A drawn space says there's one even when it's narrow.)
                let stronger = match (c.after, after) {
                    (After::Nothing, a) => a,
                    (a, After::Nothing) => a,
                    (a, b) if (b as u8) > (a as u8) => b,
                    (a, _) => a,
                };
                c.after = stronger;
                if blank && c.after == After::Nothing {
                    c.after = After::Space;
                }
                if matches!(after, After::Line | After::Paragraph) && !blank {
                    line += 1;
                }
            }
        }
        last = Some(g);
        if blank {
            continue;
        }
        chars.push(TextChar { text: g.text.clone().into_boxed_str(), rect: g.rect, line, after: After::Nothing });
    }
    // Lines counted where a character starts one.
    let mut n = 0;
    for c in &mut chars {
        c.line = n;
        if matches!(c.after, After::Line | After::Paragraph) {
            n += 1;
        }
    }
    let (string, offsets) = joined(&chars);
    let mut t = PageText { chars, string, offsets, lines: Vec::new() };
    let mut start = 0;
    while start < t.chars.len() {
        let line = t.chars[start].line;
        let end = t.chars[start..].iter().position(|c| c.line != line).map_or(t.chars.len(), |n| start + n);
        let rect = t.boxes(start, end).first().copied().unwrap_or_default();
        let text = t.between(start, end).into();
        t.lines.push(TextLine { start, end, rect, text });
        start = end;
    }
    t
}

/// A line ending in a hyphen that the next goes on from in lower case:
/// one word, broken (pasted whole).
fn hyphenated(c: &TextChar, next: Option<&TextChar>) -> bool {
    matches!(c.after, After::Line) && (c.text.ends_with('-') || c.text.ends_with('\u{AD}')) && next.is_some_and(|n| n.text.chars().next().is_some_and(char::is_lowercase))
}

fn joined(chars: &[TextChar]) -> (String, Vec<usize>) {
    let mut s = String::new();
    let mut offsets = Vec::with_capacity(chars.len() + 1);
    for (i, c) in chars.iter().enumerate() {
        offsets.push(s.len());
        if hyphenated(c, chars.get(i + 1)) {
            s.push_str(c.text.trim_end_matches(['-', '\u{AD}']));
            continue;
        }
        s.push_str(&c.text);
        s.push_str(c.after.text());
    }
    offsets.push(s.len());
    (s, offsets)
}

impl PageText {
    /// The text from caret `a` to caret `b` (carets are between
    /// characters, 0 to `chars.len()`), without what follows the last.
    pub fn between(&self, a: usize, b: usize) -> String {
        let (a, b) = (a.min(self.chars.len()), b.min(self.chars.len()));
        if a >= b {
            return String::new();
        }
        let end = self.offsets[b - 1] + self.chars[b - 1].text.len();
        let end = end.min(self.offsets[b]);
        self.string[self.offsets[a]..end].to_owned()
    }

    /// The caret nearest `p` (rendered points): before or after the
    /// character under it, else the nearest on its line, else the
    /// nearest of all.
    pub fn caret_at(&self, p: [f32; 2]) -> usize {
        if self.chars.is_empty() {
            return 0;
        }
        let side = |i: usize| {
            let r = self.chars[i].rect;
            if p[0] > (r[0] + r[2]) / 2.0 { i + 1 } else { i }
        };
        let mut best: Option<(f32, usize)> = None;
        for (i, c) in self.chars.iter().enumerate() {
            let r = c.rect;
            let dy = if p[1] < r[1] {
                r[1] - p[1]
            } else if p[1] > r[3] {
                p[1] - r[3]
            } else {
                0.0
            };
            let dx = if p[0] < r[0] {
                r[0] - p[0]
            } else if p[0] > r[2] {
                p[0] - r[2]
            } else {
                0.0
            };
            if dx == 0.0 && dy == 0.0 {
                return side(i);
            }
            // On its line first: across matters far more than along.
            let d = dy * 1000.0 + dx;
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, i));
            }
        }
        let i = best.map_or(0, |(_, i)| i);
        let r = self.chars[i].rect;
        if p[1] > r[3] && p[0] > r[0] {
            // Below the nearest line's end: after it.
            i + 1
        } else if p[1] < r[1] {
            i
        } else {
            side(i)
        }
    }

    /// The character under `p` (rendered points), if one is.
    pub fn char_at(&self, p: [f32; 2]) -> Option<usize> {
        self.chars.iter().position(|c| p[0] >= c.rect[0] && p[0] < c.rect[2] && p[1] >= c.rect[1] && p[1] < c.rect[3])
    }

    /// The word around character `at` (letters and digits), as carets.
    pub fn word_at(&self, at: usize) -> (usize, usize) {
        let word = |i: usize| self.chars.get(i).is_some_and(|c| c.text.chars().all(char::is_alphanumeric));
        let at = at.min(self.chars.len().saturating_sub(1));
        if !word(at) {
            return (at, (at + 1).min(self.chars.len()));
        }
        let mut a = at;
        while a > 0 && word(a - 1) && self.chars[a - 1].after == After::Nothing {
            a -= 1;
        }
        let mut b = at;
        while b + 1 < self.chars.len() && word(b + 1) && self.chars[b].after == After::Nothing {
            b += 1;
        }
        (a, b + 1)
    }

    /// The line around caret `at`, as carets.
    pub fn line_at(&self, at: usize) -> (usize, usize) {
        let at = at.min(self.chars.len().saturating_sub(1));
        let Some(line) = self.chars.get(at).map(|c| c.line) else { return (0, 0) };
        let a = self.chars.iter().position(|c| c.line == line).unwrap_or(at);
        let b = self.chars.iter().rposition(|c| c.line == line).map_or(at + 1, |i| i + 1);
        (a, b)
    }

    /// The boxes carets `a` to `b` cover, one a line (rendered points).
    pub fn boxes(&self, a: usize, b: usize) -> Vec<[f32; 4]> {
        let mut out: Vec<(u32, [f32; 4])> = Vec::new();
        for c in self.chars.iter().take(b.min(self.chars.len())).skip(a) {
            match out.last_mut() {
                Some((line, r)) if *line == c.line => {
                    *r = [r[0].min(c.rect[0]), r[1].min(c.rect[1]), r[2].max(c.rect[2]), r[3].max(c.rect[3])];
                }
                _ => out.push((c.line, c.rect)),
            }
        }
        out.into_iter().map(|(_, r)| r).collect()
    }

    /// Where `needle` (already lower case) is in the text, case left
    /// aside, as carets.
    pub fn find(&self, needle: &str) -> Vec<(usize, usize)> {
        if needle.is_empty() {
            return Vec::new();
        }
        let hay = self.string.to_lowercase();
        // (Lower case keeps every byte offset for the text pages have,
        // ASCII and most else; where it doesn't, matches are skipped.)
        if hay.len() != self.string.len() {
            return Vec::new();
        }
        let caret = |byte: usize| self.offsets.partition_point(|o| *o <= byte).saturating_sub(1);
        hay.match_indices(needle).map(|(at, m)| (caret(at), caret(at + m.len() - 1) + 1)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(text: &str, x: f64, y: f64, w: f64) -> Drawn {
        // 10-point type, y down: the baseline at `y`.
        Drawn { text: text.into(), rect: [x as f32, (y - 8.5) as f32, (x + w) as f32, (y + 2.5) as f32], origin: Point::new(x, y), end: Point::new(x + w, y), dir: Vec2::new(1.0, 0.0), size: 10.0 }
    }

    fn line(text: &str, x: f64, y: f64) -> Vec<Drawn> {
        let mut out = Vec::new();
        let mut at = x;
        for c in text.chars() {
            let w = if c == ' ' { 2.5 } else { 5.0 };
            out.push(g(&c.to_string(), at, y, w));
            at += w;
        }
        out
    }

    #[test]
    fn spaces_lines_paragraphs_and_hyphens() {
        let mut glyphs = line("Hello world, a long hyphen-", 72.0, 100.0);
        glyphs.extend(line("ated word.", 72.0, 112.0));
        // A gap without a drawn space is one too.
        glyphs.push(g("A", 200.0, 112.0, 5.0));
        glyphs.extend(line("Next paragraph", 72.0, 150.0));
        let t = arrange(glyphs);
        assert_eq!(t.string, "Hello world, a long hyphenated word. A\n\nNext paragraph");
        assert_eq!(t.chars.iter().map(|c| c.line).max(), Some(2));
        // Selections copy what's between their carets ("Hello" 0..5,
        // "world," 5..11).
        assert_eq!(t.between(0, 5), "Hello");
        assert_eq!(t.between(0, 10), "Hello world");
        assert_eq!(t.word_at(7), (5, 10));
        let (a, b) = t.line_at(0);
        assert_eq!(t.between(a, b), "Hello world, a long hyphen");
        assert_eq!(t.lines.len(), 3);
        assert_eq!(&*t.lines[1].text, "ated word. A");
        assert_eq!(t.boxes(0, 12).len(), 1);
        assert_eq!(t.boxes(0, t.chars.len()).len(), 3);
        // Found, case aside, across the line broken in a word.
        let found = t.find("hyphenated");
        assert_eq!(found.len(), 1);
        let (a, b) = found[0];
        assert_eq!(t.between(a, b), "hyphenated");
        assert_eq!(t.find("WORLD".to_lowercase().as_str()).len(), 1);
    }

    #[test]
    fn carets_from_points() {
        let t = arrange(line("one two", 0.0, 10.0));
        // On a character's right half: after it.
        assert_eq!(t.caret_at([4.0, 6.0]), 1);
        assert_eq!(t.caret_at([1.0, 6.0]), 0);
        // Past the line's end and below it: after its last.
        assert_eq!(t.caret_at([200.0, 6.0]), t.chars.len());
        assert_eq!(t.caret_at([200.0, 300.0]), t.chars.len());
        // Above it all: its start.
        assert_eq!(t.caret_at([0.0, -50.0]), 0);
    }
}

#[cfg(test)]
mod live {
    /// Prints the text of the PDF at `OLO_PDF` (its first pages):
    /// `OLO_PDF=… cargo test -p olo-viewer pdftext::live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_a_real_pdf() {
        let path = std::env::var("OLO_PDF").expect("OLO_PDF");
        let pdf = hayro::hayro_syntax::Pdf::new(std::fs::read(path).unwrap()).unwrap();
        let cache = hayro::hayro_interpret::InterpreterCache::new();
        let pages = pdf.pages();
        for (i, page) in pages.iter().enumerate().take(std::env::var("OLO_PAGES").ok().and_then(|n| n.parse().ok()).unwrap_or(2)) {
            let t = std::time::Instant::now();
            let text = super::read(page, &cache);
            println!("---- page {} ({} characters, {:.1} ms)\n{}", i + 1, text.chars.len(), t.elapsed().as_secs_f64() * 1e3, text.string);
            if let Ok(word) = std::env::var("OLO_PDF_AT")
                && let Some(at) = text.string.find(&word)
            {
                let c = text.offsets.partition_point(|o| *o <= at) - 1;
                for ch in text.chars.iter().skip(c.saturating_sub(2)).take(word.len() + 16) {
                    println!("{:?} {:?} {:?}", ch.text, ch.rect, ch.after);
                }
            }
        }
    }
}
