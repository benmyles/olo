//! A banner: news to be seen at once, not read in a status line ("This
//! step is done"): a box across its column tinted for what it says
//! (green, red), a large round mark (a check, a cross), a bold title, a
//! line under it, and on its right what to do next (a `KeyHint`, a
//! button). For views whose outcome matters more than a mark beside a
//! sentence says: the palm trainer's finished step. Agents read it as a
//! "Status" node named by its title, its line the value.

use olo_interface::{Bounds, Element, Hsla, IntoElement, Painter, ParentElement as _, PathBuilder, Pixels, SharedString, Styled as _, agent, canvas, div, point, px};
use olo_kit_base::metrics as m;

use crate::{style, theme, widgets::StringView};

/// What a banner (or a mark) says: it went well, or it didn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tone {
    Success,
    Failure,
}

impl Tone {
    fn ink(self, p: &olo_theme::Palette) -> Hsla {
        style::c(match self {
            Tone::Success => p.success,
            Tone::Failure => p.failure,
        })
    }
}

/// A round mark `d` points across centred in `b`: a white check on green
/// for success, a cross on red for failure.
pub fn verdict_mark(g: &mut Painter, b: Bounds<Pixels>, tone: Tone, d: f32, p: &olo_theme::Palette) {
    let c = b.center();
    let mut disc = PathBuilder::fill();
    for k in 0..=48 {
        let t = k as f32 / 48.0 * std::f32::consts::TAU;
        let pt = point(c.x + px(d / 2.0 * t.cos()), c.y + px(d / 2.0 * t.sin()));
        if k == 0 { disc.move_to(pt) } else { disc.line_to(pt) }
    }
    disc.close();
    if let Ok(path) = disc.build() {
        g.paint_path(path, tone.ink(p));
    }
    let mut path = PathBuilder::stroke(px((d / 8.0).max(1.5)));
    let k = d / 2.0;
    if tone == Tone::Success {
        path.move_to(point(c.x - px(k * 0.45), c.y + px(k * 0.02)));
        path.line_to(point(c.x - px(k * 0.12), c.y + px(k * 0.36)));
        path.line_to(point(c.x + px(k * 0.48), c.y - px(k * 0.34)));
    } else {
        path.move_to(point(c.x - px(k * 0.36), c.y - px(k * 0.36)));
        path.line_to(point(c.x + px(k * 0.36), c.y + px(k * 0.36)));
        path.move_to(point(c.x + px(k * 0.36), c.y - px(k * 0.36)));
        path.line_to(point(c.x - px(k * 0.36), c.y + px(k * 0.36)));
    }
    if let Ok(path) = path.build() {
        g.paint_path(path, olo_interface::white());
    }
}

/// A banner's box in `b`: tinted for `tone`, or (None) a quiet one of the
/// panel's own colours, for what stands in the banner's place until it
/// comes (the palm trainer's line on the last touch), so nothing moves
/// when it does.
pub fn banner_back(g: &mut Painter, b: Bounds<Pixels>, tone: Option<Tone>, p: &olo_theme::Palette) {
    match tone {
        Some(tone) => {
            let ink = tone.ink(p);
            style::fill_rect(g, b, ink.opacity(0.16));
            style::frame_rect(g, b, ink);
        }
        None => {
            style::fill_rect(g, b, style::shade(p, p.panel_background, 1.04));
            style::frame_rect(g, b, style::shade(p, p.panel_background, 1.22));
        }
    }
}

/// The banner's mark, across.
const MARK: f32 = 32.0;

/// A banner's height: its title's line and its own, inset.
pub fn banner_height() -> f32 {
    style::line_row(style::TITLE_SIZE) + m::TIGHT + style::line_row(style::LEAD_SIZE) + m::SPACING * 3.0
}

pub struct Banner {
    tone: Tone,
    title: SharedString,
    detail: SharedString,
    trailing: Option<Element>,
}

impl Banner {
    pub fn new(tone: Tone, title: impl Into<SharedString>, detail: impl Into<SharedString>) -> Banner {
        Banner { tone, title: title.into(), detail: detail.into(), trailing: None }
    }

    /// What goes on its right: what to do next.
    pub fn trailing(mut self, e: impl IntoElement) -> Self {
        self.trailing = Some(e.into_element());
        self
    }
}

impl IntoElement for Banner {
    fn into_element(self) -> Element {
        let (tone, title, detail) = (self.tone, self.title, self.detail);
        let (name, value) = (title.clone(), detail.clone());
        let back = canvas(move |g, b| {
            let p = theme::palette();
            banner_back(g, b, Some(tone), &p);
            g.agent(agent::Node { role: "Status", name: Some(name.clone()), value: Some(value.clone()), bounds: b, enabled: true, ..Default::default() });
        })
        .paint_key(style::key((tone, &title, &detail)))
        .absolute()
        .inset_0();
        let mark = canvas(move |g, b| verdict_mark(g, b, tone, MARK, &theme::palette())).paint_key(style::key(tone)).w(px(MARK)).h(px(MARK)).flex_shrink_0();
        let words = div()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .flex_1()
            .min_w_0()
            .child(StringView::new(title).bold(true).size(style::TITLE_SIZE).height(style::line_row(style::TITLE_SIZE)))
            .child(StringView::new(detail).size(style::LEAD_SIZE).height(style::line_row(style::LEAD_SIZE)));
        let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING * 2.0)).px(px(m::SPACING * 2.0)).size_full().child(mark).child(words);
        if let Some(t) = self.trailing {
            row = row.child(div().flex_shrink_0().child(t));
        }
        div().relative().w_full().h(px(banner_height())).child(back).child(row)
    }
}
