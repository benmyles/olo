//! A small mark that is on or off: what a thing has or hasn't, said at
//! a glance in a status bar (Rex's: Olo's typing helper, its bridge and
//! the skills for agents on the host typed into; Ben, 2026-11-23:
//! "little icons in the status bar that are either in an on or off
//! state not the weird popover"). The mark is one of the kit's
//! ([`Mark`]: drawn in whole device lines at every scale, as a
//! control's face carries them); on, it is drawn in the ink of the
//! bar's own words (a status line's: `style::secondary_text`); off, in
//! a quarter of the text's ink, far fainter than anything the bar says
//! (off is to be seen as off with no mark that is on beside it: the
//! first design reviews, of the ink a disabled label has). Its name and
//! how it stands are its tip, above it (it is at a window's foot), and
//! what agents read ("Typing helper", "on").

use olo_interface::{Element, IntoElement, SharedString, Styled as _, agent, canvas, px};
use olo_kit_base::metrics as m;

use super::segmented::{Mark, mark_lines, paint_mark_from};
use crate::{style, theme};

pub struct Indicator {
    name: SharedString,
    mark: Mark,
    on: bool,
    says: Option<SharedString>,
    key: SharedString,
    /// What its tip is placed over besides it ([`indicators`]).
    over: crate::tip::Reach,
    /// One of a row, which draws it.
    in_row: bool,
}

/// A mark's box, which is the indicator's own width; and the gap a row
/// of them is set with (Opus, of four points: "they read as one
/// strip").
const MARK: (f32, f32) = (13.0, 11.0);
pub const INDICATOR_GAP: f32 = 6.0;

/// The box of `mark`: the bridge's two lines wider than the others' (a
/// drawing of open lines, with a span outside each tower: in thirteen
/// its towers were its ends, and it was a letter M to three reviews).
/// Gaps are from box to box.
fn box_of(mark: Mark) -> (f32, f32) {
    if mark == Mark::Bridge { (MARK.0 + 2.0, MARK.1) } else { MARK }
}

/// How much of the text's ink a mark that is off has, where a device
/// line is a point across (1×, 2×). Where it is less (two thirds of a
/// point at 1.5×) a mark has that much more of the ink, on or off, or
/// it is a third lighter there than at the other scales and one that
/// is off nearly gone (Opus's review of the second drawings).
const OFF: f32 = 0.25;

/// A mark's ink at `scale`: on, the ink of a status line's words,
/// nearer the text's own by as much as a line is thinner (half the
/// way at 1.5×; three quarters, and it was near black beside the
/// same marks at 1× to both reviews); off,
/// [`OFF`] of the text's, more by as much as a line is thinner.
fn ink(p: &olo_theme::Palette, on: bool, scale: f32) -> olo_interface::Hsla {
    let thin = scale / style::line_pixels(scale);
    if on {
        let quiet = style::secondary_text(p);
        let full = style::c(p.panel_text);
        let more = (thin - 1.0).clamp(0.0, 1.0);
        olo_interface::Hsla { h: quiet.h, s: quiet.s + (full.s - quiet.s) * more, l: quiet.l + (full.l - quiet.l) * more, a: quiet.a }
    } else {
        style::c(olo_theme::mix(p.panel_text, p.panel_background, 1.0 - (OFF * thin).min(0.45)))
    }
}

/// A row of indicators, [`INDICATOR_GAP`] apart, as a status bar ends
/// with: the tip of each opens in one place, over the row (not begun
/// at the mark under the pointer and somewhere else for the next),
/// and clear of what stands `under_by` points over the row (a bar's
/// rule: a tip set right over the row lay on it).
pub fn indicators(marks: Vec<Indicator>, under_by: f32) -> Element {
    use olo_interface::ParentElement as _;
    // The row draws its marks itself, from its own end: each as many
    // device pixels from the next as every other, and the last ending
    // where the row does. (Each drawn in its own box, which is laid on
    // whole pixels by itself: gaps of eight pixels and seven at 1.5×,
    // and the last mark two pixels short of the bar's inset.)
    let shown: Vec<(Mark, bool)> = marks.iter().map(|mark| (mark.mark, mark.on)).collect();
    let drawn = canvas({
        let shown = shown.clone();
        move |g, b| {
            let s = g.scale_factor();
            let mut end = (f32::from(b.right()) * s).round();
            for &(mark, on) in shown.iter().rev() {
                end = paint(g, mark, on, end, b.center().y) - (INDICATOR_GAP * s).round();
            }
        }
    })
    .paint_key(style::key(&shown))
    .absolute()
    .inset_0();
    // (What is before and after each in the row, for its tip's place.)
    let widths: Vec<f32> = shown.iter().map(|&(mark, _)| box_of(mark).0 + INDICATOR_GAP).collect();
    olo_interface::div().relative().flex_shrink_0().flex().flex_row().items_center().gap(px(INDICATOR_GAP)).child(drawn).children(marks.into_iter().enumerate().map(|(i, mut mark)| {
        mark.over = crate::tip::Reach { before: widths[..i].iter().sum(), after: widths[i + 1..].iter().sum(), above: under_by };
        mark.in_row = true;
        mark.into_element()
    }))
}

/// A mark drawn to end at the device pixel `end`, about the height
/// `middle` (up, where it can't be as far from the top as from the
/// foot); where it begins.
fn paint(g: &mut olo_interface::Painter, mark: Mark, on: bool, end: f32, middle: olo_interface::Pixels) -> f32 {
    let s = g.scale_factor();
    let line = style::line_pixels(s);
    let (cols, rows) = mark_lines(mark, box_of(mark), s);
    let (wide, tall) = (cols as f32 * line, rows as f32 * line);
    let top = (f32::from(middle) * s - tall / 2.0 - 0.01).round();
    paint_mark_from(g, mark, (end - wide, top), box_of(mark), ink(&theme::palette(), on, s));
    end - wide
}

impl Indicator {
    /// `name`: what it is the state of ("Typing helper"), for agents
    /// and for its tip where [`Self::says`] gives no other.
    pub fn new(name: impl Into<SharedString>, mark: Mark) -> Indicator {
        let name = crate::words::of(name);
        Indicator { key: format!("indicator-{name}").into(), name, mark, on: false, says: None, over: Default::default(), in_row: false }
    }

    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    /// Its tip: how it stands, in a sentence ("Olo’s typing helper is in
    /// use on “far”").
    pub fn says(mut self, text: impl Into<SharedString>) -> Self {
        self.says = Some(crate::words::of(text));
        self
    }
}

impl IntoElement for Indicator {
    fn into_element(self) -> Element {
        let Indicator { name, mark, on, says, key, over, in_row } = self;
        let tip: SharedString = says.unwrap_or_else(|| if on { crate::tr!("{}: on", name).into() } else { crate::tr!("{}: off", name).into() });
        let said = name.clone();
        let mark_el = canvas(move |g, b| {
            // (One of a row is drawn by its row: `indicators`.)
            if !in_row {
                let end = (f32::from(b.right()) * g.scale_factor()).round();
                paint(g, mark, on, end, b.center().y);
            }
            g.agent(agent::Node { role: "Status", name: Some(said.clone()), value: Some(if on { "on" } else { "off" }.into()), bounds: b, enabled: true, ..Default::default() });
        })
        .paint_key(style::key((&name, mark, on, in_row)))
        .w(px(box_of(mark).0))
        .h(px(m::LABEL_ROW))
        .flex_shrink_0();
        crate::tip::on_above_of(mark_el.into_any_element(), key, tip, over)
    }
}
