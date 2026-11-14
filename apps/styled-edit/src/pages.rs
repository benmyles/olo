//! The document set on pages, for printing and PDF: each paragraph laid
//! out as the editor lays it out (`olo_kit::editor`'s layout, with its
//! indents, spacing, alignment and marker: `Doc::format`) at the page's
//! text width, by the kit's text system (the app's, on a worker: the faces
//! the window draws with), and its rows set down the pages, a row that
//! doesn't fit starting the next. Each word is placed where the layout
//! put it, in its own font, size and colour. The document's header and
//! footer are set in the top and bottom margins (else pages are numbered
//! at the foot when there is more than one). A table's row is its cells
//! laid out in their columns, ruled (a header shaded), and kept whole on
//! a page.

use std::sync::Arc;

use olo_kit::{
    TextSystem,
    editor::{self, Laid, Params, Shaper, Span, TabStops},
};
use olo_print::{Font, Item, Page, Paper};

use crate::doc::{Align, Doc, MONOSPACE, Page as Margins, Para, Run, Style};

/// A table's rules, and a header row's shade.
const RULE: [u8; 3] = [0x80, 0x80, 0x80];
const SHADE: [u8; 3] = [0xee, 0xee, 0xee];

/// The page's margins, in points (an inch).
pub const MARGIN: f32 = 72.0;
/// The header's and footer's size, and colour.
pub const MARGIN_TEXT: f32 = 10.0;
const MARGIN_GRAY: [u8; 3] = [0x50, 0x50, 0x50];

/// What a worker needs to set a document: a copy of it, and the text
/// system to lay it out with.
pub struct Input {
    pub text: String,
    pub runs: Vec<Run>,
    pub paras: Vec<Para>,
    pub align: Align,
    pub page: Option<Margins>,
    pub text_system: Arc<TextSystem>,
    /// Shown as Markdown or source code: set as it looks first
    /// (`convert::styled_copy`).
    pub source: Option<crate::convert::Source>,
}

/// Bytes `range`'s spans and their styles, in order (one at least).
fn spans_of(doc: &Doc, range: std::ops::Range<usize>) -> (Vec<Span>, Vec<Style>) {
    let (mut spans, mut styles) = (Vec::new(), Vec::new());
    doc.spans(
        range,
        &mut |s| {
            styles.push(s.clone());
            crate::fonts::font(s)
        },
        &mut spans,
    );
    (spans, styles)
}

/// The document on pages of `paper`.
pub fn pages(mut input: Input, paper: Paper) -> Vec<Page> {
    if let Some(source) = input.source.take() {
        (input.text, input.runs, input.paras) = crate::convert::styled_copy(&input.text, &source);
        input.align = Align::Left;
    }
    let sh: &dyn Shaper = &*input.text_system;
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
            let cells = editor::cells(&input.text[r.clone()]);
            let columns = editor::cell_columns(row, cells.len(), width - f.left);
            let inset = editor::CELL_INSET;
            let mut laid = Vec::new();
            for ((s, len), (x, w)) in cells.into_iter().zip(columns) {
                let cell = r.start + s..r.start + s + len;
                let inner = (w - 2.0 * inset.0).max(1.0);
                let (spans, styles) = spans_of(&doc, cell.clone());
                let params = Params { wrap: Some(inner), width: inner, first: 0.0, align: f.align.unwrap_or_default(), justify: false, tabs: TabStops::default() };
                let l = editor::lay_out(sh, &input.text[cell.clone()], &spans, &params);
                laid.push((x, w, l, cell, styles));
            }
            let h = laid.iter().map(|c| c.2.height()).fold(0.0, f32::max) + 2.0 * inset.1;
            if y + h > bottom && y > MARGIN {
                pages.push(new_page());
                y = MARGIN;
            }
            let page = pages.last_mut().expect("a page");
            let left = MARGIN + f.left;
            for (x, w, l, cell, styles) in &laid {
                let (lx, rx) = (left + x, left + x + w);
                if row.header {
                    page.items.push(Item::Rect { x: lx, y, w: *w, h, color: SHADE });
                }
                for (from, to) in [((lx, y), (rx, y)), ((lx, y + h), (rx, y + h)), ((lx, y), (lx, y + h)), ((rx, y), (rx, y + h))] {
                    page.items.push(Item::Line { from, to, width: 0.5, color: RULE });
                }
                for k in 0..l.rows.len() {
                    place_words(page, l, k, &input.text[cell.clone()], styles, lx + inset.0, y + inset.1 + l.rows[k].top);
                }
            }
            y += h;
            continue;
        }
        let wrap = (width - f.left - f.right).max(1.0);
        let (spans, styles) = spans_of(&doc, r.clone());
        let space = space_width(sh, &spans[0]);
        let params = Params { wrap: Some(wrap), width: wrap, first: f.first, align: f.align.unwrap_or_default(), justify: f.justify.unwrap_or(false), tabs: f.layout_tabs(space) };
        let text = &input.text[r.clone()];
        let l = editor::lay_out(sh, text, &spans, &params);
        // Space above isn't kept at a page's top.
        if y > MARGIN {
            y += f.above;
        }
        // The page y of the paragraph's top (moved on a page break).
        let mut base = y;
        for (k, row) in l.rows.iter().enumerate() {
            if base + row.top + row.height > bottom && base + row.top > MARGIN {
                pages.push(new_page());
                base = MARGIN - row.top;
            }
            let top = base + row.top;
            let page = pages.last_mut().expect("a page");
            // A list's marker, before the first row, on its baseline.
            if k == 0
                && let Some(m) = f.marker.as_ref().filter(|m| !m.is_empty())
            {
                let mark = editor::lay_out_line(sh, m, &spans[0]);
                let w = mark.width();
                let gap = (mark.height() * 0.35).round();
                let s = styles.first().cloned().unwrap_or_default();
                page.items.push(text_item(MARGIN + f.left - gap - w, top + row.ascent, m.clone(), &s, w));
            }
            place_words(page, &l, k, text, &styles, MARGIN + f.left, top);
        }
        y = base + l.height() + f.below;
    }
    let n = pages.len();
    if let Some(m) = &input.page {
        // The header's baseline half way down the top margin, the
        // footer's half way up the bottom one; each part where it says.
        let style = Style { size: MARGIN_TEXT, color: Some(MARGIN_GRAY), ..Style::default() };
        let span = Span::new(0, crate::fonts::font(&style), MARGIN_TEXT);
        for (k, p) in pages.iter_mut().enumerate() {
            for (parts, y) in [(&m.header, MARGIN / 2.0), (&m.footer, paper.height - MARGIN / 2.0)] {
                for (slot, part) in parts.iter().enumerate() {
                    let text = Margins::fill(part, k + 1, n);
                    if text.trim().is_empty() {
                        continue;
                    }
                    let l = editor::lay_out_line(sh, &text, &span);
                    let w = l.width();
                    let x = match slot {
                        0 => MARGIN,
                        1 => (paper.width - w) / 2.0,
                        _ => paper.width - MARGIN - w,
                    };
                    let Some(row) = l.rows.first() else { continue };
                    // (Placed so its row's baseline is at `y`.)
                    place_words(p, &l, 0, &text, std::slice::from_ref(&style), x, y - row.ascent);
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

/// A space's width in `span`'s font and size.
fn space_width(sh: &dyn Shaper, span: &Span) -> f32 {
    editor::lay_out_line(sh, " ", span).units.first().map_or(span.size * 0.25, |u| u.adv)
}

/// What page `k` (from 0) of `n` shows in its margins, as printed: the
/// header's left, centre and right parts, then the footer's, each with
/// its baseline's distance from the page's top. With no header or footer
/// set, the page's number at the foot when there's more than one page.
pub fn margin_lines(page: Option<&Margins>, k: usize, n: usize) -> Vec<(String, f32)> {
    let height = olo_print::local_paper().height;
    match page {
        Some(m) => m.header.iter().map(|p| (Margins::fill(p, k + 1, n), MARGIN / 2.0)).chain(m.footer.iter().map(|p| (Margins::fill(p, k + 1, n), height - MARGIN / 2.0))).collect(),
        None if n > 1 => vec![(String::new(), 0.0), (String::new(), 0.0), (String::new(), 0.0), (String::new(), 0.0), ((k + 1).to_string(), height - MARGIN / 2.0), (String::new(), 0.0)],
        None => Vec::new(),
    }
}

/// Row `r` of `l` (paragraph text `text`, styled `styles` span by span)
/// as words (not the spaces between), each where the layout put it,
/// the paragraph's box at `origin_x` and the row's top at `top`.
fn place_words(page: &mut Page, l: &Laid, r: usize, text: &str, styles: &[Style], origin_x: f32, top: f32) {
    let row = &l.rows[r];
    let baseline = top + row.ascent;
    // The word so far: where it starts, its text, its span, where it ends.
    let mut word: Option<(f32, String, u16, f32)> = None;
    let flush = |word: &mut Option<(f32, String, u16, f32)>, page: &mut Page| {
        if let Some((x, text, span, end)) = word.take()
            && !text.trim().is_empty()
        {
            let s = styles.get(span as usize).cloned().unwrap_or_default();
            page.items.push(text_item(x, baseline, text, &s, end - x));
        }
    };
    for u in &l.units[row.units.clone()] {
        let x = origin_x + u.x;
        let chars = &text[u.byte..u.end];
        match u.kind {
            // A picture: where its character was laid out, as big.
            editor::Kind::Picture => {
                flush(&mut word, page);
                if let Some(pic) = styles.get(u.span as usize).and_then(|s| s.picture.as_ref())
                    && let Some(data) = picture_bytes(&pic.path)
                {
                    page.items.push(Item::Image { x, y: baseline - pic.height, w: pic.width, h: pic.height, data });
                }
            }
            editor::Kind::Space | editor::Kind::Tab => flush(&mut word, page),
            // (A Markdown mark off the caret's line: not printed.)
            editor::Kind::Hidden => {}
            editor::Kind::Text => match &mut word {
                Some((_, t, span, end)) if *span == u.span => {
                    t.push_str(chars);
                    *end = x + u.adv;
                }
                _ => {
                    flush(&mut word, page);
                    word = Some((x, chars.to_owned(), u.span, x + u.adv));
                }
            },
        }
    }
    flush(&mut word, page);
}

/// A picture file's bytes as a PDF takes them (each read once a setting).
fn picture_bytes(path: &str) -> Option<std::sync::Arc<Vec<u8>>> {
    thread_local! {
        static READ: std::cell::RefCell<std::collections::HashMap<String, Option<std::sync::Arc<Vec<u8>>>>> = Default::default();
    }
    READ.with(|r| r.borrow_mut().entry(path.to_owned()).or_insert_with(|| crate::pictures::export_bytes(std::path::Path::new(path), true).map(std::sync::Arc::new)).clone())
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
        Input {
            text: text.into(),
            runs: vec![Run { len: text.len(), style: Style::default() }],
            paras: vec![Para::default(); text.split('\n').count()],
            align: Align::Left,
            page: None,
            text_system: TextSystem::standalone(),
            source: None,
        }
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
        i.paras = vec![Para { row: Some(crate::doc::Row { header: true, ..Default::default() }), ..Para::default() }, Para { row: Some(crate::doc::Row::default()), ..Para::default() }];
        let paper = olo_print::PAPERS[0];
        let pages = pages(i, paper);
        let items = &pages[0].items;
        // The header shaded, every cell ruled, each word in its column.
        assert_eq!(items.iter().filter(|i| matches!(i, Item::Rect { .. })).count(), 2);
        assert_eq!(items.iter().filter(|i| matches!(i, Item::Line { .. })).count(), 16);
        let x_of = |w: &str| items.iter().find_map(|i| if let Item::Text { text, x, .. } = i { (text == w).then_some(*x) } else { None }).unwrap();
        let half = (paper.width - 2.0 * MARGIN) / 2.0;
        assert!((x_of("Size") - (MARGIN + half + editor::CELL_INSET.0)).abs() < 1.0);
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

    #[test]
    fn pictures_on_pages() {
        let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/icons/16/printer.png");
        let pic = crate::doc::Picture { path: file.into(), width: 48.0, height: 36.0, alt: String::new() };
        let text = "A \u{FFFC} b";
        let mut i = input(text);
        i.runs = vec![Run { len: 2, style: Style::default() }, Run { len: 3, style: Style { picture: Some(pic), ..Style::default() } }, Run { len: 2, style: Style::default() }];
        let pages = pages(i, olo_print::PAPERS[0]);
        let items = &pages[0].items;
        // The picture where its character is, as big; the words either side.
        let img = items.iter().find_map(|i| if let Item::Image { x, y, w, h, .. } = i { Some((*x, *y, *w, *h)) } else { None }).expect("the picture");
        assert_eq!((img.2, img.3), (48.0, 36.0));
        let x_of = |w: &str| items.iter().find_map(|i| if let Item::Text { text, x, .. } = i { (text == w).then_some(*x) } else { None }).unwrap();
        assert!(x_of("A") < img.0 && img.0 + 48.0 <= x_of("b") + 0.5, "{img:?}");
        assert!(!items.iter().any(|i| matches!(i, Item::Text { text, .. } if text.contains('\u{FFFC}'))));
    }
}
