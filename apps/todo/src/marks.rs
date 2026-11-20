//! The window's small drawings, each in one colour, in a box 12 points
//! across: the marks a to-do's row and card show for what it has (notes,
//! attachments, a reminder), and the glyphs of its small buttons (add,
//! take away, more). A row of full-colour icons at every to-do was
//! noise, and blue link words for every action made a page of a window
//! (Ben, 2026-11-19: "so much is ugly"; Opus's and Astra's reviews).

use olo_kit::{
    Bounds, Element, Hsla, InteractiveElement as _, MouseButton, MouseDownEvent, Painter, PathBuilder, Pixels, SharedString, Styled as _, WindowCx, agent, control_canvas, point, px, size, style,
    theme,
};

/// What a mark says a to-do has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mark {
    Notes,
    Attachments,
    Reminder,
}

/// A mark's width.
pub const MARK: f32 = 12.0;

impl Mark {
    /// What agents read of it.
    pub fn word(self) -> &'static str {
        match self {
            Mark::Notes => "notes",
            Mark::Attachments => "attachments",
            Mark::Reminder => "reminder",
        }
    }
}

fn stroke(g: &mut Painter, width: f32, ink: Hsla, points: &[olo_kit::Point<Pixels>]) {
    let mut path = PathBuilder::stroke(px(width));
    let Some((first, rest)) = points.split_first() else { return };
    path.move_to(*first);
    for p in rest {
        path.line_to(*p);
    }
    if let Ok(path) = path.build() {
        g.paint_path(path, ink);
    }
}

/// `mark` drawn in `ink`, in the 12 points square at `b`'s middle: set
/// on whole device pixels, its lines a whole number of them wide, so at
/// 1× each is a clear small drawing and not a smudge.
pub fn paint(g: &mut Painter, mark: Mark, b: Bounds<Pixels>, ink: Hsla) {
    let s = g.scale_factor();
    let snap = |v: Pixels| px((f32::from(v) * s).round() / s);
    let (x0, y0) = (snap(b.center().x - px(MARK / 2.0)), snap(b.center().y - px(MARK / 2.0)));
    // (A line's width: a point, in whole device pixels; its middle on a
    // pixel's middle where that's an odd number of them.)
    let w = (s).round().max(1.0) / s;
    let half = w / 2.0;
    let at = |x: f32, y: f32| point(x0 + px(x + half), y0 + px(y + half));
    match mark {
        // A page, its corner folded, two lines of writing on it.
        Mark::Notes => {
            stroke(g, w, ink, &[at(6.0, 1.0), at(2.0, 1.0), at(2.0, 10.0), at(9.0, 10.0), at(9.0, 4.0), at(6.0, 1.0), at(6.0, 4.0), at(9.0, 4.0)]);
            stroke(g, w, ink, &[at(4.0, 6.0), at(7.0, 6.0)]);
            stroke(g, w, ink, &[at(4.0, 8.0), at(7.0, 8.0)]);
        }
        // A paper clip, upright, as one is bent: down its right side, round
        // its foot, up its left, over its head, down inside, round a small
        // foot and a short way back up. Four wires, each on a pixel: its
        // inner loop a clear pixel wide, two clear at its left between
        // that and its outer wire (with three wires it read as a 0, then
        // as a U; with four a pixel apart, as a narrow U still: Astra's
        // and Opus's reviews, 2026-11-19).
        Mark::Attachments => {
            let mut clip = PathBuilder::stroke(px(w));
            clip.move_to(at(8.0, 3.0));
            clip.line_to(at(8.0, 7.5));
            clip.arc_to(point(px(3.5), px(3.5)), px(0.0), false, true, at(1.0, 7.5));
            clip.line_to(at(1.0, 3.0));
            clip.arc_to(point(px(2.5), px(2.5)), px(0.0), false, true, at(6.0, 3.0));
            clip.line_to(at(6.0, 6.5));
            clip.arc_to(point(px(1.0), px(1.0)), px(0.0), false, true, at(4.0, 6.5));
            clip.line_to(at(4.0, 4.0));
            if let Ok(path) = clip.build() {
                g.paint_path(path, ink);
            }
        }
        // A bell: its dome in outline, its rim a line, its clapper under it.
        Mark::Reminder => {
            let mut dome = PathBuilder::stroke(px(w));
            dome.move_to(at(2.0, 8.5));
            dome.cubic_bezier_to(at(3.0, 5.0), at(3.0, 7.5), at(3.0, 6.5));
            dome.cubic_bezier_to(at(8.0, 5.0), at(3.0, 0.8), at(8.0, 0.8));
            dome.cubic_bezier_to(at(9.0, 8.5), at(8.0, 6.5), at(8.0, 7.5));
            if let Ok(path) = dome.build() {
                g.paint_path(path, ink);
            }
            stroke(g, w, ink, &[at(1.0, 8.5), at(10.0, 8.5)]);
            let clapper = Bounds { origin: point(x0 + px(4.5), y0 + px(9.5)), size: size(px(2.0), px(1.5)) };
            g.paint_quad(olo_kit::fill(clapper, ink).corner_radii(px(0.75)));
        }
    }
}

/// A small button's glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Glyph {
    /// Add.
    Plus,
    /// Take away.
    Cross,
    /// A day to pick: the set's small calendar.
    Calendar,
}

/// `glyph` drawn in `ink` at `b`'s middle.
pub fn paint_glyph(g: &mut Painter, glyph: Glyph, b: Bounds<Pixels>, ink: Hsla) {
    let c = b.center();
    let at = |x: f32, y: f32| point(c.x + px(x), c.y + px(y));
    match glyph {
        Glyph::Plus => {
            stroke(g, 1.5, ink, &[at(-4.0, 0.0), at(4.0, 0.0)]);
            stroke(g, 1.5, ink, &[at(0.0, -4.0), at(0.0, 4.0)]);
        }
        // (Finer than the +: a quiet mark beside what it takes away, a
        // point wide in whole device pixels, as the marks' lines are. A
        // point and a half, it was the loudest thing on a card's foot.)
        Glyph::Cross => {
            let s = g.scale_factor();
            let w = s.round().max(1.0) / s;
            stroke(g, w, ink, &[at(-3.5, -3.5), at(3.5, 3.5)]);
            stroke(g, w, ink, &[at(3.5, -3.5), at(-3.5, 3.5)]);
        }
        Glyph::Calendar => {
            let at = Bounds { origin: point(c.x - px(8.0), c.y - px(8.0)), size: size(px(16.0), px(16.0)) };
            olo_kit::icons::paint(g, at, "calendar");
        }
    }
}

/// How a small button stands when the pointer isn't on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Look {
    /// Its glyph alone and quiet (a row's ×: there for every row).
    Quiet,
    /// Raised, as a button is: what opens a menu beside a field.
    Raised,
    /// In force: pressed in, on the selection's ground (what it opened
    /// is showing).
    On,
}

/// A small button of a glyph (`side` points square): raised under the
/// pointer (and always, where `look` says), sunken while pressed.
/// `name` is what agents read, and its tip.
pub fn glyph_button(id: impl Into<SharedString>, glyph: Glyph, name: impl Into<SharedString>, side: f32, look: Look, on_press: impl Fn(&MouseDownEvent, &mut WindowCx) + 'static) -> Element {
    let name: SharedString = olo_kit::words::of(name.into());
    let said = name.clone();
    let el = control_canvas(move |g, b, state| {
        let p = theme::palette();
        let b = style::snap(g, b);
        if state.pressed {
            style::fill_rect(g, b, style::shade(&p, p.panel_background, 0.9));
            style::sunken(g, b, &p, false);
        } else if look == Look::On {
            // (As the half in force of the window's List/Board switch.)
            let inner = style::inset(g, b, 1.0);
            // (Its edge the selection's too: its face alone, most of it
            // under its glyph, read as the button at rest by day.)
            style::fill_rect(g, inner, style::c(p.list_selected_background));
            style::frame_rect(g, b, style::c(p.control_highlight));
            style::bevel(g, inner, style::shade(&p, p.list_selected_background, 1.35), style::c(p.shine));
        } else if look == Look::Raised {
            // (As a pop-up field's face beside it: one manner for what
            // opens a menu.)
            let base = p.menu_background;
            let inner = style::inset(g, b, 1.0);
            style::vgradient(g, inner, style::tint(base, 0.82), style::shade(&p, base, 1.05));
            style::frame_rect(g, b, style::shade(&p, p.panel_background, 1.55));
            style::bevel(g, inner, style::c(p.shine), style::shade(&p, base, 1.2));
        } else if state.hovered {
            style::raised(g, b, &p);
        }
        let ink = if look == Look::Quiet && !state.hovered { style::secondary_text(&p) } else { style::c(p.panel_text) };
        paint_glyph(g, glyph, b, ink);
        g.agent(agent::Node::button(said.clone(), b));
    })
    .paint_key(style::key((glyph, look)))
    .w(px(side))
    .h(px(side))
    .flex_shrink_0()
    .on_mouse_down(MouseButton::Left, on_press);
    olo_kit::tip::on(el, id.into(), name)
}
