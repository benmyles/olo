//! Text shaped once and painted where it goes, for drawings that lay
//! their own text out (a conversation's balloons, a list's rows): a
//! [`Line`] (one line, cut short with "…" past a width) and a [`Para`]
//! (text wrapped to a width, paragraph by paragraph, as wide as its
//! widest line; which byte is under a point, and where a range of it
//! is, for selecting and for links). Shape when what's shown changes,
//! keep it, and paint it each frame: shaping is the cost.
//! (Telegram's, moved here when Phone wanted the same.)

use crate::{Bounds, Font, Hsla, Painter, Pixels, Point, ShapedLine, SharedString, TextAlign, TextRun, WrappedLine, point, px, size, style};

/// A line of text, shaped once: painted with its top at a point.
#[derive(Clone)]
pub struct Line {
    pub shaped: ShapedLine,
    pub w: f32,
    pub h: f32,
    /// The whole text, if it's cut short: its tip (`olo_kit::cut`).
    pub full: Option<SharedString>,
}

impl Line {
    /// From the top to the baseline.
    pub fn baseline(&self) -> f32 {
        let (a, d) = (f32::from(self.shaped.ascent), f32::from(self.shaped.descent));
        (self.h - a - d) / 2.0 + a
    }

    pub fn paint(&self, gr: &mut Painter, at: Point<Pixels>) {
        let _ = self.shaped.paint(at, px(self.h), gr);
        if let Some(full) = &self.full {
            crate::cut::note(gr, full.clone(), Bounds { origin: at, size: size(px(self.w), px(self.h)) });
        }
    }
}

/// The height of a line of `size` text.
pub fn line_height(size: f32) -> f32 {
    (size * style::LINE_HEIGHT).ceil()
}

/// `text` on one line, cut short with "…" past `max_w`.
pub fn line(text: &str, size: f32, font: Font, color: Hsla, max_w: Option<f32>) -> Line {
    let whole: SharedString = text.replace('\n', " ").into();
    let text = match max_w {
        Some(w) => style::fit_with(&crate::thread_text_system(), whole.clone(), size, &font, px(w.max(10.0))),
        None => whole.clone(),
    };
    let full = (text != whole).then_some(whole);
    let run = TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None, size: None };
    let shaped = crate::thread_text_system().shape_line(text, px(size), &[run], None);
    Line { w: f32::from(shaped.width), h: line_height(size), shaped, full }
}

/// Text wrapped to a width, paragraph by paragraph.
#[derive(Clone)]
pub struct Para {
    pub lines: Vec<WrappedLine>,
    pub lh: f32,
    pub w: f32,
    pub h: f32,
    pub align: TextAlign,
}

/// Where each line of a wrapped paragraph starts (bytes in it).
fn row_starts(p: &WrappedLine) -> Vec<usize> {
    let layout = &p.unwrapped_layout;
    std::iter::once(0).chain(p.wrap_boundaries().iter().map(|b| layout.runs[b.run_ix].glyphs[b.glyph_ix].index)).collect()
}

impl Para {
    pub fn new(text: &str, size: f32, runs: &[TextRun], width: f32, lines_at_most: Option<usize>) -> Para {
        let lh = line_height(size);
        let lines: Vec<WrappedLine> =
            crate::thread_text_system().shape_text(SharedString::from(text.to_owned()), px(size), runs, Some(px(width)), lines_at_most).map(|l| l.into_vec()).unwrap_or_default();
        // As wide as its widest line (not the width it was wrapped to).
        let mut w: f32 = 0.0;
        let mut rows = 0;
        for p in &lines {
            let starts = row_starts(p);
            for (r, &s) in starts.iter().enumerate() {
                let e = starts.get(r + 1).copied().unwrap_or(p.len());
                let x0 = p.unwrapped_layout.x_for_index(s);
                // (A wrap's trailing space isn't the line's.)
                let end = if r + 1 < starts.len() { p.text[..e].trim_end().len().max(s) } else { e };
                w = w.max(f32::from(p.unwrapped_layout.x_for_index(end) - x0));
                rows += 1;
            }
        }
        Para { lines, lh, w: w.ceil(), h: lh * rows.max(1) as f32, align: TextAlign::Left }
    }

    /// One run of `color` over all of `text`.
    pub fn plain(text: &str, size: f32, font: Font, color: Hsla, width: f32, lines_at_most: Option<usize>) -> Para {
        let run = TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None, size: None };
        Para::new(text, size, &[run], width, lines_at_most)
    }

    fn tops(&self) -> Vec<(usize, f32)> {
        let (mut start, mut y) = (0, 0.0);
        self.lines
            .iter()
            .map(|p| {
                let out = (start, y);
                start += p.len() + 1;
                y += self.lh * (p.wrap_boundaries().len() + 1) as f32;
                out
            })
            .collect()
    }

    pub fn paint(&self, gr: &mut Painter, at: Point<Pixels>) {
        let width = px(self.w);
        for (p, (_, y)) in self.lines.iter().zip(self.tops()) {
            let origin = point(at.x, at.y + px(y));
            let bounds = (self.align != TextAlign::Left).then(|| Bounds { origin, size: size(width, px(self.lh)) });
            // (Code's and a hidden spoiler's grounds, under the glyphs.)
            let _ = p.paint_background(origin, px(self.lh), self.align, bounds, gr);
            let _ = p.paint(origin, px(self.lh), self.align, bounds, gr);
        }
    }

    /// The byte of the text under `q` (from the text's top left), if any.
    pub fn index_at(&self, q: Point<Pixels>) -> Option<usize> {
        for (p, (start, y)) in self.lines.iter().zip(self.tops()) {
            let h = self.lh * (p.wrap_boundaries().len() + 1) as f32;
            let local = point(q.x, q.y - px(y));
            if f32::from(local.y) >= 0.0 && f32::from(local.y) < h {
                return p.index_for_position(local, px(self.lh)).ok().map(|i| start + i);
            }
        }
        None
    }

    /// Where the bytes `range` of it are, row by row, from the text's
    /// top left (as [`Para::index_at`] finds them: from the left).
    pub fn rects_of(&self, range: std::ops::Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut rects = Vec::new();
        for (p, (start, y)) in self.lines.iter().zip(self.tops()) {
            let starts = row_starts(p);
            for (r, &s) in starts.iter().enumerate() {
                let e = starts.get(r + 1).copied().unwrap_or(p.len());
                let (from, to) = (range.start.saturating_sub(start).max(s), range.end.saturating_sub(start).min(e));
                if from < to {
                    let left = p.unwrapped_layout.x_for_index(s);
                    let (x0, x1) = (p.unwrapped_layout.x_for_index(from) - left, p.unwrapped_layout.x_for_index(to) - left);
                    rects.push(Bounds { origin: point(x0, px(y + self.lh * r as f32)), size: size(x1 - x0, px(self.lh)) });
                }
            }
        }
        rects
    }

    /// Where its last line ends, and that line's baseline (from the top).
    pub fn last_line(&self) -> (f32, f32) {
        let Some(p) = self.lines.last() else { return (0.0, self.h * 0.8) };
        let starts = row_starts(p);
        let x0 = p.unwrapped_layout.x_for_index(*starts.last().unwrap_or(&0));
        let right = f32::from(p.unwrapped_layout.x_for_index(p.len()) - x0);
        let (a, d) = (f32::from(p.unwrapped_layout.ascent), f32::from(p.unwrapped_layout.descent));
        (right, self.h - self.lh + (self.lh - a - d) / 2.0 + a)
    }

    /// The span of lines (top and bottom) each byte range covers.
    pub fn rows_of(&self, range: std::ops::Range<usize>) -> Option<(f32, f32)> {
        let mut span: Option<(f32, f32)> = None;
        for (p, (start, y)) in self.lines.iter().zip(self.tops()) {
            let starts = row_starts(p);
            for (r, &s) in starts.iter().enumerate() {
                let e = starts.get(r + 1).copied().unwrap_or(p.len() + 1);
                if start + s < range.end && start + e > range.start {
                    let top = y + self.lh * r as f32;
                    span = Some(span.map_or((top, top + self.lh), |(a, b)| (a.min(top), b.max(top + self.lh))));
                }
            }
        }
        span
    }
}
