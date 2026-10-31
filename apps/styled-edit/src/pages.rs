//! The document set on pages, for printing and PDF: each paragraph laid
//! out as the editor lays it out (its indents, spacing, alignment and
//! marker; `Doc::format`) at the page's text width, by a worker's own
//! fonts (`olo_kit::style::all_font_definitions`, the document's faces
//! included), and its rows set down the pages, a row that doesn't fit
//! starting the next. Each word is placed where the layout put it, in
//! its own font, size and colour; pages are numbered at the foot.

use egui::{Color32, FontFamily, epaint::text::Fonts, text::LayoutJob};
use olo_kit::egui;
use olo_print::{Font, Item, Page, Paper};

use crate::doc::{Align, Doc, MONOSPACE, Para, Run, Style};

/// The page's margins, in points (an inch).
pub const MARGIN: f32 = 72.0;
/// Laid out this finely (as at 4×), so places are a quarter point at
/// worst.
const PPP: f32 = 4.0;

/// What a worker needs to set a document: a copy of it, and the egui
/// family each of its styles draws with (as the window has them).
pub struct Input {
    pub text: String,
    pub runs: Vec<Run>,
    pub paras: Vec<Para>,
    pub align: Align,
    pub families: Vec<(Style, FontFamily)>,
    pub options: egui::epaint::text::TextOptions,
}

/// The document on pages of `paper`.
pub fn pages(input: Input, paper: Paper) -> Vec<Page> {
    let mut fonts = Fonts::new(input.options, olo_kit::style::all_font_definitions());
    fonts.begin_pass(input.options, Default::default());
    let family = |s: &Style| input.families.iter().find(|(k, _)| k == s).map_or(FontFamily::Proportional, |(_, f)| f.clone());
    let doc = Doc::with_paras(input.text.clone(), Some(input.runs.clone()), Some(input.paras.clone()));
    let width = paper.width - 2.0 * MARGIN;
    let bottom = paper.height - MARGIN;
    let new_page = || Page { width: paper.width, height: paper.height, items: Vec::new() };
    let mut pages = vec![new_page()];
    let mut y = MARGIN;
    for i in 0..doc.paras().len() {
        let r = doc.para_range(i);
        let f = doc.format(i, input.align);
        let wrap = (width - f.left - f.right).max(1.0);
        let mut job = LayoutJob {
            text: input.text[r.clone()].to_owned(),
            halign: f.align.unwrap_or(egui::Align::Min),
            justify: f.justify.unwrap_or(false),
            keep_trailing_whitespace: true,
            ..Default::default()
        };
        job.wrap.max_width = wrap;
        // Each section's style, in order (the family asked once each).
        let mut styles: Vec<Style> = Vec::new();
        doc.sections(r.clone(), Color32::BLACK, &mut |s| {
            styles.push(s.clone());
            family(s)
        }, &mut job);
        if job.sections.is_empty() {
            continue;
        }
        job.sections[0].leading_space += f.first;
        let first_format = job.sections[0].format.clone();
        // Which section each character is in (the glyphs come in the
        // characters' order).
        let ranges: Vec<std::ops::Range<usize>> = job.sections.iter().map(|s| s.byte_range.start.0..s.byte_range.end.0).collect();
        let section_of: Vec<usize> = job.text.char_indices().map(|(b, _)| ranges.iter().position(|r| r.contains(&b)).unwrap_or(0)).collect();
        let mut char_at = 0usize;
        let g = fonts.with_pixels_per_point(PPP).layout_job(job);
        // Where the galley's x = 0 is (alignment puts rows around it).
        let origin_x = MARGIN
            + f.left
            + match f.align.unwrap_or(egui::Align::Min) {
                egui::Align::Min => 0.0,
                egui::Align::Center => wrap / 2.0,
                egui::Align::Max => wrap,
            };
        // Space above isn't kept at a page's top.
        if y > MARGIN {
            y += f.above;
        }
        // The page y of the galley's top (moved on a page break).
        let mut base = y;
        let mut marked = f.marker.is_none();
        for row in &g.rows {
            if base + row.pos.y + row.size.y > bottom && base + row.pos.y > MARGIN {
                pages.push(new_page());
                base = MARGIN - row.pos.y;
            }
            let top = base + row.pos.y;
            let page = pages.last_mut().expect("a page");
            // A list's marker, before the first row.
            if !marked {
                marked = true;
                if let Some(m) = &f.marker {
                    let mark = fonts.with_pixels_per_point(PPP).layout_job(LayoutJob::single_section(m.clone(), first_format.clone()));
                    let w = mark.size().x;
                    let baseline = top + row.glyphs.first().map_or(row.size.y * 0.8, |gl| gl.pos.y);
                    let gap = (first_format.font_id.size * 0.6).round();
                    let s = styles.first().cloned().unwrap_or_default();
                    page.items.push(text_item(MARGIN + f.left - gap - w, baseline, m.clone(), &s, w));
                }
            }
            // Words (and the spaces between), each where it was laid.
            let mut word: Option<(f32, f32, String, usize, f32)> = None;
            let flush = |word: &mut Option<(f32, f32, String, usize, f32)>, page: &mut Page| {
                if let Some((x, baseline, text, section, w)) = word.take()
                    && !text.trim().is_empty()
                {
                    let s = styles.get(section).cloned().unwrap_or_default();
                    page.items.push(text_item(x, baseline, text, &s, w));
                }
            };
            for gl in &row.glyphs {
                let x = origin_x + row.pos.x + gl.pos.x;
                let baseline = top + gl.pos.y;
                let section = section_of.get(char_at).copied().unwrap_or(0);
                char_at += 1;
                if gl.chr.is_whitespace() {
                    flush(&mut word, page);
                    continue;
                }
                match &mut word {
                    Some((x0, _, text, sec, w)) if *sec == section => {
                        text.push(gl.chr);
                        *w = x + gl.advance_width - *x0;
                    }
                    _ => {
                        flush(&mut word, page);
                        word = Some((x, baseline, gl.chr.to_string(), section, gl.advance_width));
                    }
                }
            }
            flush(&mut word, page);
        }
        y = base + g.size().y + f.below;
    }
    // Page numbers, at the foot, when there is more than one.
    let n = pages.len();
    if n > 1 {
        for (k, p) in pages.iter_mut().enumerate() {
            let label = format!("{}", k + 1);
            let size = 9.0;
            let w = label.len() as f32 * size * 0.55;
            p.items.push(Item::Text {
                x: (paper.width - w) / 2.0,
                y: paper.height - MARGIN / 2.0,
                text: label,
                font: Font::default(),
                size,
                color: [0x70, 0x70, 0x70],
                width: w,
                underline: false,
                strike: false,
            });
        }
    }
    pages
}

fn text_item(x: f32, y: f32, text: String, s: &Style, width: f32) -> Item {
    Item::Text {
        x,
        y,
        text,
        font: Font { family: if s.family == MONOSPACE { String::new() } else { s.family.clone() }, bold: s.bold, italic: s.italic, mono: s.family == MONOSPACE },
        size: s.size,
        color: s.color.unwrap_or([0, 0, 0]),
        width,
        underline: s.underline,
        strike: s.strikeout,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(text: &str) -> Input {
        let options = egui::epaint::text::TextOptions::default();
        Input { text: text.into(), runs: vec![Run { len: text.len(), style: Style::default() }], paras: vec![Para::default(); text.split('\n').count()], align: Align::Left, families: Vec::new(), options }
    }

    #[test]
    fn words_on_pages() {
        let paper = olo_print::PAPERS[0];
        let pages = pages(input("Hello world\n\nSecond paragraph"), paper);
        assert_eq!(pages.len(), 1);
        let words: Vec<&str> = pages[0].items.iter().filter_map(|i| if let Item::Text { text, .. } = i { Some(text.as_str()) } else { None }).collect();
        assert_eq!(words, ["Hello", "world", "Second", "paragraph"]);
        // Starting at the margin, on a baseline below the top margin.
        if let Item::Text { x, y, .. } = &pages[0].items[0] {
            assert!((x - MARGIN).abs() < 0.5 && *y > MARGIN && *y < MARGIN + 20.0, "{x} {y}");
        }
    }

    #[test]
    fn long_text_breaks_pages_and_numbers_them() {
        let text = (0..400).map(|i| format!("Line {i} of a long document")).collect::<Vec<_>>().join("\n");
        let paper = olo_print::PAPERS[0];
        let pages = pages(input(&text), paper);
        assert!(pages.len() > 3, "{}", pages.len());
        for p in &pages {
            for i in &p.items {
                if let Item::Text { y, .. } = i {
                    assert!(*y < paper.height - MARGIN / 2.0 + 1.0);
                }
            }
        }
        // The last item of each page is its number.
        assert!(matches!(pages[1].items.last(), Some(Item::Text { text, .. }) if text == "2"));
    }
}
