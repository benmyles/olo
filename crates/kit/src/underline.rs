//! An underline that leaves letters' tails alone: for text set cell by
//! cell (a terminal's), where a line under the baseline would run
//! through every g, p and y. A link under the pointer in Terminal's way
//! was so underlined, and both design reviews of Rex's links found the
//! same of it (2026-11-23): "the tails turn into blue and black knots",
//! and the line "looks like it was laid over the text instead of drawn
//! for it". Under the tails there is no room in a terminal's rows: a
//! line there reads as one over the next row.
//!
//! So the line stays close under the baseline and is broken where a
//! letter's ink crosses it, a little clear of the ink on each side
//! ([`paint`]). It is a point thick in whole device pixels (as Olo's
//! lines are: one at 1× and 1.5×, two at 2×), its top edge a point and a
//! half under the baseline, on a device pixel; a point clear of ink;
//! and no piece of it shorter than most of a cell is drawn.
//!
//! The ink is the text system's own drawing of each letter
//! (`TextSystem::line_mask`), kept by the caller between frames
//! ([`Inks`]): a letter is drawn once for a face, a size and a scale.

use std::{collections::HashMap, rc::Rc};

use olo_interface::{Bounds, Font, FontId, Hsla, Painter, fill, point, px};

use crate::style;

/// A letter as drawn: its coverage, row by row, and its baseline's row.
struct Ink {
    width: i32,
    height: i32,
    baseline: i32,
    mask: Vec<u8>,
}

/// The letters drawn so far, to be kept by what paints the underline.
#[derive(Default)]
pub struct Inks(HashMap<(char, FontId, u32, u32), Option<Rc<Ink>>>);

/// How much of a pixel a letter must cover to count as its ink (the
/// faint edge of a curve too: the line is kept clear of what is seen).
const INK: u8 = 24;

/// The shortest piece of the line that is drawn: three points, or
/// most of a cell where that is more. What would be left shorter
/// (between two tails side by side, as of "pg"; after the last
/// letter's tail, as of a final "y"; in a letter's bowl) is a speck,
/// not a line: what is left of a cell beside a tail in it, and what
/// lies between the tails of two letters side by side, is always less
/// than this. Gemini's second and third reviews of Rex's links (at 1×
/// a piece of exactly three pixels was still drawn after a last "y",
/// and one of five between the tails of "pg").
const LEAST: f32 = 3.0;
const LEAST_OF_CELL: f32 = 0.8;

impl Inks {
    fn of(&mut self, ch: char, font: &Font, size: f32, scale: f32) -> Option<Rc<Ink>> {
        let system = olo_interface::text_system();
        let key = (ch, system.resolve_font(font), size.to_bits(), scale.to_bits());
        if self.0.len() > 1024 && !self.0.contains_key(&key) {
            self.0.clear();
        }
        self.0
            .entry(key)
            .or_insert_with(|| {
                let mut text = [0u8; 4];
                let (drawn, mask, baseline) = system.line_mask(ch.encode_utf8(&mut text), font, px(size), scale)?;
                Some(Rc::new(Ink { width: drawn.width.0, height: drawn.height.0, baseline, mask }))
            })
            .clone()
    }
}

/// Where the line is broken, for letters at whole device pixels: the
/// spans (from, to) of device columns to leave out, in order and
/// apart. `cells`: each letter's left edge (a device column) and its
/// ink; `rows`: the line's device rows from each letter's baseline
/// (from, to); `clear`: how far from ink the line stays; `least`: the
/// shortest piece of line left between two breaks.
fn breaks(cells: &[(i32, Rc<Ink>)], rows: (i32, i32), clear: i32, least: i32) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    for (left, ink) in cells {
        let (from, to) = ((ink.baseline + rows.0).max(0), (ink.baseline + rows.1).min(ink.height));
        let mut run: Option<(i32, i32)> = None;
        for x in 0..=ink.width {
            let inked = x < ink.width && (from..to).any(|y| ink.mask[(y * ink.width + x) as usize] >= INK);
            match (inked, run) {
                (true, None) => run = Some((x, x + 1)),
                (true, Some((a, _))) => run = Some((a, x + 1)),
                (false, Some((a, b))) => {
                    let (a, b) = (left + a - clear, left + b + clear);
                    match out.last_mut() {
                        // (What is left between two breaks this near is
                        // no line: a speck.)
                        Some(last) if a < last.1 + least => last.1 = last.1.max(b),
                        _ => out.push((a, b)),
                    }
                    run = None;
                }
                (false, None) => {}
            }
        }
    }
    out
}

/// Underlines letters set a `cell` apart from `left` on a line whose
/// baseline is at `baseline` (points, in the window), in `color`,
/// leaving their tails alone. `text`: each cell's letter and the face
/// it is drawn in (a space, or `'\0'`, where there is none); `size`:
/// the text's size in points.
pub fn paint(g: &mut Painter, inks: &mut Inks, left: f32, baseline: f32, cell: f32, text: &[(char, Font)], size: f32, color: Hsla) {
    let scale = g.scale_factor();
    let thick = style::line_pixels(scale) as i32;
    // (A point clear of a letter's ink, and no piece under three.)
    let (clear, least) = ((scale.round() as i32).max(1), (LEAST.max(LEAST_OF_CELL * cell) * scale).round() as i32);
    let at = |points: f32| (points * scale).round() as i32;
    // (Its top edge a point and a half under the baseline.)
    let under = (1.5 * scale).round() as i32;
    let top = at(baseline) + under;
    let (from, to) = (at(left), at(left + cell * text.len() as f32));
    let cells: Vec<(i32, Rc<Ink>)> =
        text.iter().enumerate().filter(|(_, (ch, _))| !ch.is_whitespace() && *ch != '\0').filter_map(|(i, (ch, font))| Some((at(left + cell * i as f32), inks.of(*ch, font, size, scale)?))).collect();
    let mut x = from;
    let line = |g: &mut Painter, a: i32, b: i32| {
        if b - a >= least {
            let corner = |x: i32, y: i32| point(px(x as f32 / scale), px(y as f32 / scale));
            g.paint_quad(fill(Bounds::from_corners(corner(a, top), corner(b, top + thick)), color));
        }
    };
    for (a, b) in breaks(&cells, (under, under + thick), clear, least) {
        line(g, x, a.min(to));
        x = x.max(b);
    }
    line(g, x, to);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A letter eight columns wide whose ink, in the rows the line
    /// takes, is the columns `inked`.
    fn letter(inked: &[i32]) -> Rc<Ink> {
        let (width, height, baseline) = (8, 12, 8);
        let mut mask = vec![0u8; (width * height) as usize];
        for y in baseline + 2..baseline + 3 {
            for &x in inked {
                mask[(y * width + x) as usize] = 255;
            }
        }
        // (Ink over the baseline is none of the line's.)
        mask[(3 * width + 1) as usize] = 255;
        Rc::new(Ink { width, height, baseline, mask })
    }

    #[test]
    fn the_line_is_broken_where_ink_crosses_it_and_nowhere_else() {
        // An "a" (nothing under its baseline), a "p" (a stem at column
        // 1), a "g" (a bowl: columns 1 and 5 to 6), an "a".
        let cells = [(0, letter(&[])), (8, letter(&[1])), (16, letter(&[1, 5, 6])), (24, letter(&[]))];
        // The p's stem with a pixel clear each side; the g's bowl as
        // one break, the speck inside it not left.
        assert_eq!(breaks(&cells, (2, 3), 1, 3), [(8, 11), (16, 24)]);
        // Rows the ink isn't in: no break.
        assert_eq!(breaks(&cells, (4, 5), 1, 3), []);
        // Further clear at a finer scale.
        assert_eq!(breaks(&cells[1..2], (2, 4), 2, 6), [(7, 12)]);
        // Two tails far apart in one letter are two breaks.
        assert_eq!(breaks(&[(0, letter(&[0, 7]))], (2, 3), 1, 3), [(-1, 2), (6, 9)]);
        // Two tails side by side ("pg"): what would be left between
        // them, shorter than the least, isn't; a longer piece is.
        let side_by_side = [(0, letter(&[6])), (8, letter(&[1]))];
        assert_eq!(breaks(&side_by_side, (2, 3), 1, 3), [(5, 11)]);
        let apart = [(0, letter(&[2])), (8, letter(&[5]))];
        assert_eq!(breaks(&apart, (2, 3), 1, 3), [(1, 4), (12, 15)]);
        assert_eq!(breaks(&apart, (2, 3), 1, 9), [(1, 15)]);
    }
}
