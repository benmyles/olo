//! The document set on pages, for printing and PDF: each paragraph laid
//! out as the editor lays it out (its indents, spacing, alignment and
//! marker; `Doc::format`) at the page's text width, by a worker's own
//! fonts (`olo_kit::style::all_font_definitions`, the document's faces
//! included), and its rows set down the pages, a row that doesn't fit
//! starting the next. Each word is placed where the layout put it, in
//! its own font, size and colour. The document's header and footer are
//! set in the top and bottom margins (else pages are numbered at the foot
//! when there is more than one). A table's row is its cells laid out in their columns, ruled (a header
//! shaded), and kept whole on a page.

use egui::{Color32, FontFamily, epaint::text::Fonts, text::LayoutJob};
use olo_kit::egui;
use olo_print::{Font, Item, Page, Paper};

use crate::doc::{Align, Doc, MONOSPACE, Page as Margins, Para, Run, Style};

/// A table's rules, and a header row's shade.
const RULE: [u8; 3] = [0x80, 0x80, 0x80];
const SHADE: [u8; 3] = [0xee, 0xee, 0xee];

/// The page's margins, in points (an inch).
pub const MARGIN: f32 = 72.0;
/// The header's and footer's size, and colour.
const MARGIN_TEXT: f32 = 10.0;
const MARGIN_GRAY: [u8; 3] = [0x50, 0x50, 0x50];
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
    pub page: Option<Margins>,
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
        if let Some(row) = &f.row {
            // A table's row: each cell laid out in its column, the row
            // kept whole (on the next page if it doesn't fit).
            let cells = olo_kit::editor::cells(&input.text[r.clone()]);
            let columns = olo_kit::editor::cell_columns(row, cells.len(), width - f.left);
            let inset = olo_kit::editor::CELL_INSET;
            let mut laid = Vec::new();
            for ((s, len), (x, w)) in cells.into_iter().zip(columns) {
                let cell = r.start + s..r.start + s + len;
                let inner = (w - 2.0 * inset.x).max(1.0);
                let mut job = LayoutJob { text: input.text[cell.clone()].to_owned(), halign: f.align.unwrap_or(egui::Align::Min), keep_trailing_whitespace: true, ..Default::default() };
                job.wrap.max_width = inner;
                let mut styles: Vec<Style> = Vec::new();
                doc.sections(cell, Color32::BLACK, &mut |s| {
                    styles.push(s.clone());
                    family(s)
                }, &mut job);
                let section_of = sections_of(&job);
                let g = fonts.with_pixels_per_point(PPP).layout_job(job);
                let ax = match f.align.unwrap_or(egui::Align::Min) {
                    egui::Align::Min => 0.0,
                    egui::Align::Center => inner / 2.0,
                    egui::Align::Max => inner,
                };
                laid.push((x, w, ax, g, styles, section_of));
            }
            let h = laid.iter().map(|c| c.3.size().y).fold(0.0, f32::max) + 2.0 * inset.y;
            if y + h > bottom && y > MARGIN {
                pages.push(new_page());
                y = MARGIN;
            }
            let page = pages.last_mut().expect("a page");
            let left = MARGIN + f.left;
            for (x, w, ax, g, styles, section_of) in &laid {
                let (l, rr) = (left + x, left + x + w);
                if row.header {
                    page.items.push(Item::Rect { x: l, y, w: *w, h, color: SHADE });
                }
                for (from, to) in [((l, y), (rr, y)), ((l, y + h), (rr, y + h)), ((l, y), (l, y + h)), ((rr, y), (rr, y + h))] {
                    page.items.push(Item::Line { from, to, width: 0.5, color: RULE });
                }
                let mut char_at = 0;
                for grow in &g.rows {
                    place_words(page, grow, left + x + inset.x + ax, y + inset.y + grow.pos.y, styles, section_of, &mut char_at);
                }
            }
            y += h;
            continue;
        }
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
        let space = fonts.with_pixels_per_point(PPP).glyph_width(&job.sections[0].format.font_id, ' ');
        job.tabs = Some(f.galley_tabs(space));
        let first_format = job.sections[0].format.clone();
        let section_of = sections_of(&job);
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
            place_words(page, row, origin_x, top, &styles, &section_of, &mut char_at);
        }
        y = base + g.size().y + f.below;
    }
    let n = pages.len();
    if let Some(m) = &input.page {
        // The header's baseline half way down the top margin, the
        // footer's half way up the bottom one; each part where it says.
        let style = Style { size: MARGIN_TEXT, color: Some(MARGIN_GRAY), ..Style::default() };
        let family = family(&style);
        for (k, p) in pages.iter_mut().enumerate() {
            for (parts, y) in [(&m.header, MARGIN / 2.0), (&m.footer, paper.height - MARGIN / 2.0)] {
                for (slot, part) in parts.iter().enumerate() {
                    let text = Margins::fill(part, k + 1, n);
                    if text.trim().is_empty() {
                        continue;
                    }
                    let job = LayoutJob::simple_singleline(text, egui::FontId::new(MARGIN_TEXT, family.clone()), Color32::BLACK);
                    let section_of = sections_of(&job);
                    let g = fonts.with_pixels_per_point(PPP).layout_job(job);
                    let w = g.size().x;
                    let x = match slot {
                        0 => MARGIN,
                        1 => (paper.width - w) / 2.0,
                        _ => paper.width - MARGIN - w,
                    };
                    let Some(row) = g.rows.first() else { continue };
                    // (Placed so its first row's baseline is at `y`.)
                    let top = y - row.glyphs.first().map_or(0.0, |gl| gl.pos.y);
                    place_words(p, row, x - row.pos.x, top, std::slice::from_ref(&style), &section_of, &mut 0);
                }
            }
        }
    } else if n > 1 {
        // Page numbers, at the foot.
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

/// Which section of `job` each character is in (the glyphs come in the
/// characters' order).
fn sections_of(job: &LayoutJob) -> Vec<usize> {
    let ranges: Vec<std::ops::Range<usize>> = job.sections.iter().map(|s| s.byte_range.start.0..s.byte_range.end.0).collect();
    job.text.char_indices().map(|(b, _)| ranges.iter().position(|r| r.contains(&b)).unwrap_or(0)).collect()
}

/// A laid-out row's words (and not the spaces between), each where the
/// layout put it, in its section's style; `char_at` counts the galley's
/// characters on.
fn place_words(page: &mut Page, row: &egui::epaint::text::PlacedRow, origin_x: f32, top: f32, styles: &[Style], section_of: &[usize], char_at: &mut usize) {
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
        let section = section_of.get(*char_at).copied().unwrap_or(0);
        *char_at += 1;
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
        Input { text: text.into(), runs: vec![Run { len: text.len(), style: Style::default() }], paras: vec![Para::default(); text.split('\n').count()], align: Align::Left, page: None, families: Vec::new(), options }
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
    fn table_rows_are_ruled_cells() {
        let mut i = input("Name\tSize\na\t1");
        i.paras = vec![
            Para { row: Some(crate::doc::Row { header: true, ..Default::default() }), ..Para::default() },
            Para { row: Some(crate::doc::Row::default()), ..Para::default() },
        ];
        let paper = olo_print::PAPERS[0];
        let pages = pages(i, paper);
        let items = &pages[0].items;
        // The header shaded, every cell ruled, each word in its column.
        assert_eq!(items.iter().filter(|i| matches!(i, Item::Rect { .. })).count(), 2);
        assert_eq!(items.iter().filter(|i| matches!(i, Item::Line { .. })).count(), 16);
        let x_of = |w: &str| items.iter().find_map(|i| if let Item::Text { text, x, .. } = i { (text == w).then_some(*x) } else { None }).unwrap();
        let half = (paper.width - 2.0 * MARGIN) / 2.0;
        assert!((x_of("Size") - (MARGIN + half + olo_kit::editor::CELL_INSET.x)).abs() < 1.0);
        assert!((x_of("1") - x_of("Size")).abs() < 0.5 && (x_of("a") - x_of("Name")).abs() < 0.5);
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

    #[test]
    fn header_and_footer_in_the_margins() {
        let text = (0..120).map(|i| format!("Line {i}")).collect::<Vec<_>>().join("\n");
        let mut i = input(&text);
        i.page = Some(Margins { header: ["Report".into(), String::new(), "{page} of {pages}".into()], footer: [String::new(), "Draft".into(), String::new()] });
        let paper = olo_print::PAPERS[0];
        let pages = pages(i, paper);
        let n = pages.len();
        assert!(n > 1);
        // The words in the top margin, in order.
        let top = |p: &Page| -> Vec<(String, f32, f32, f32)> {
            p.items.iter().filter_map(|i| if let Item::Text { text, x, y, width, .. } = i { (*y < MARGIN).then(|| (text.clone(), *x, *y, *width)) } else { None }).collect()
        };
        let foot = |p: &Page| -> Vec<(String, f32, f32, f32)> {
            p.items.iter().filter_map(|i| if let Item::Text { text, x, y, width, .. } = i { (*y > paper.height - MARGIN).then(|| (text.clone(), *x, *y, *width)) } else { None }).collect()
        };
        for (k, p) in pages.iter().enumerate() {
            let words = top(p);
            let texts: Vec<&str> = words.iter().map(|w| w.0.as_str()).collect();
            assert_eq!(texts, ["Report", &(k + 1).to_string(), "of", &n.to_string()]);
            // Left at the margin, half way down the top one.
            let (_, x, y, _) = words[0];
            assert!((x - MARGIN).abs() < 0.5 && (y - MARGIN / 2.0).abs() < 0.5, "{x} {y}");
            // Right: its last word ends at the margin.
            let (_, x, _, w) = words[3];
            assert!((x + w - (paper.width - MARGIN)).abs() < 1.0, "{x} {w}");
            // Centred at the foot.
            let words = foot(p);
            assert_eq!(words.len(), 1);
            let (_, x, y, w) = words[0].clone();
            assert!((x + w / 2.0 - paper.width / 2.0).abs() < 1.0 && (y - (paper.height - MARGIN / 2.0)).abs() < 0.5);
        }
        // Set empty: nothing at all in the margins.
        let mut i = input(&text);
        i.page = Some(Margins::default());
        let empty = super::pages(i, paper);
        assert!(empty.iter().all(|p| top(p).is_empty() && foot(p).is_empty()));
    }
}
