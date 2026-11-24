//! The marks on a terminal's icon (a tab's, a pane's, an Activity
//! row's): what the coding agent in it is doing at its top corner (a
//! turning ring at work, an orange "!" waiting for an answer, a green
//! dot done and not yet seen, a red "×" stopped on an error, a grey dot
//! at rest), a blue dot for a bell not yet seen, and at its bottom
//! corners what it has on: Keep It Going (a blue disc with a "go on"
//! triangle) and Auto Approve (a green disc with a check). Terminal's
//! tabs and Rex's tabs, panes and Activity draw them alike.

use std::rc::Rc;

use olo_interface::{Bounds, Hsla, Painter, Pixels, Point, Rgba, fill, point, px, size};

use crate::{icons, style, theme, widgets::strip};

/// What the agent in a terminal is doing, as its icon's top corner
/// shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AgentMark {
    /// Nothing to show: no agent, or none worth a mark.
    #[default]
    None,
    /// At work: a turning ring (drawn again ten times a second, it alone).
    Working,
    /// Waiting for an answer: an orange disc with "!".
    Asking,
    /// Finished, and not yet looked at: a green dot.
    Done,
    /// Stopped on an error: a red disc with "×".
    Failed,
    /// Running, with nothing to do: a grey dot (where every row is an
    /// agent, as in Rex's Activity, so one at rest still reads as there).
    AtRest,
    /// No agent's news, but a bell not yet seen: a blue dot.
    Rang,
}

/// A terminal's marks: what its agent is doing, and what it has on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Marks {
    pub agent: AgentMark,
    pub keep_going: bool,
    pub auto_approve: bool,
}

/// The painter of a terminal's icon (`icon`: its name, "terminal") with
/// its marks, for a tab strip's or a list's row.
pub fn icon(name: &'static str, marks: Marks) -> strip::PaintFn {
    Rc::new(move |g: &mut Painter, r: Bounds<Pixels>| paint(g, r, name, marks))
}

/// A terminal's icon (`name`) in `r`, with its marks, all inside its
/// box: what its agent is doing at the top right, a 7-point disc (the
/// working ring 8); Auto Approve and Keep It Going along the bottom,
/// 5-point discs, quieter than what needs an answer (they stay on for
/// hours). Each is set off by a 1-point rim of what it is drawn on.
/// (They were 9-point discs half outside the box, and three of them hid
/// the icon: Opus's review, 2026-11-24.)
pub fn paint(g: &mut Painter, r: Bounds<Pixels>, name: &str, marks: Marks) {
    icons::paint(g, r, name);
    let p = theme::palette();
    // (The rim: the content's colour on the chosen tab, the panel's elsewhere.)
    let rim = strip::drawn_over().unwrap_or_else(|| style::c(p.panel_background));
    let disc = |g: &mut Painter, c: Point<Pixels>, radius: f32, color: olo_theme::Rgba, glyph: Glyph| {
        mark(g, c, radius, style::c(color), rim, glyph, style::ink_on(color));
    };
    if marks.auto_approve {
        disc(g, point(r.left() + px(2.5), r.bottom() - px(2.5)), 2.5, p.success, Glyph::Check);
    }
    if marks.keep_going {
        triangle(g, point(r.right() - px(2.5), r.bottom() - px(2.5)), 5.0, style::c(p.keyboard_navigation), rim);
    }
    let at = point(r.right() - px(3.5), r.top() + px(3.5));
    match marks.agent {
        AgentMark::Working => {
            // By the clock, so each time it's drawn shows where it is
            // now; and drawn again ten times a second while it turns
            // (it alone: what it's drawn in).
            let ring = point(r.right() - px(4.0), r.top() + px(4.0));
            let turn = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f32() % 1000.0) * 5.0;
            mark(g, ring, 4.0, rim, rim, Glyph::None, rim);
            working_ring(g, ring, turn, rim, style::c(p.keyboard_navigation), p.mode == olo_theme::ThemeMode::Dark);
            g.paint_again_within(0.1);
        }
        AgentMark::Asking => disc(g, at, 3.5, p.hue(olo_theme::Hue::Orange), Glyph::Bang),
        AgentMark::Failed => disc(g, at, 3.5, p.failure, Glyph::Cross),
        AgentMark::Done => disc(g, at, 3.0, p.success, Glyph::None),
        AgentMark::AtRest => mark(g, at, 3.0, style::disabled_text(&p), rim, Glyph::None, rim),
        AgentMark::Rang => disc(g, at, 2.5, p.keyboard_navigation, Glyph::None),
        AgentMark::None => {}
    }
}

/// Auto Approve's countdown over a terminal's screen, as Terminal and
/// Rex show it (`widgets::countdown_box`): its check, as on the tab's
/// icon, so the one explains the other; "Auto Approve answers Yes in N
/// s", the count two figures wide (a figure space before one figure,
/// so the box doesn't move as it goes from 10 to 9); and Stop.
pub fn auto_approve_countdown(
    left: u64,
    short: bool,
    id: impl Into<olo_interface::SharedString>,
    on_stop: impl Fn(&olo_interface::ClickEvent, &mut olo_interface::WindowCx) + 'static,
) -> olo_interface::Element {
    use olo_interface::{IntoElement as _, Styled as _};
    let check = olo_interface::canvas(|g, b| {
        let p = theme::palette();
        mark(g, b.center(), 5.0, style::c(p.success), style::c(p.panel_background), Glyph::Check, style::ink_on(p.success));
    })
    .paint_key(1)
    .w(px(12.0))
    .h(px(12.0))
    .into_element();
    let count = format!("{left:\u{2007}>2}");
    // (Over a narrow pane, `short`: the check says whose Yes it is.)
    let text = if short { crate::tr!("Yes in {} s", count) } else { crate::tr!("Auto Approve answers Yes in {} s", count) };
    crate::widgets::countdown_box(Some(check), text.into(), id.into(), on_stop)
}

/// What a mark's disc carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Glyph {
    None,
    /// "!": waiting for an answer.
    Bang,
    /// A check: Auto Approve.
    Check,
    /// "×": failed.
    Cross,
}

/// A disc `radius` points round `centre`, of `ground`, with a 1-point rim
/// of `rim` round it and `glyph` on it in `ink`: drawn as the icons are,
/// in whole device pixels each of one colour, no smoothed edge (a
/// smoothed disc of five pixels, a check stroked on it, was a smudge at
/// 1×). Its width in device pixels is whole and of the parity of the
/// glyph's stroke, so the stroke sits in its middle.
fn mark(g: &mut Painter, centre: Point<Pixels>, radius: f32, ground: Hsla, rim: Hsla, glyph: Glyph, ink: Hsla) {
    let s = g.scale_factor();
    let edge = s.round().max(1.0);
    let mut stroke = (1.25 * s).round().max(1.0);
    let mut width = (2.0 * radius * s).round().max(3.0);
    // (Under seven pixels a glyph in a disc is a smudge: at 1× and 1.25×
    // Auto Approve's mark is a plain disc of five pixels and Keep It
    // Going's a solid triangle, the two told apart by their shapes:
    // Opus's review, 2026-11-24.)
    let small = width < 7.0 && glyph == Glyph::Check;
    if small {
        width = 5.0;
        stroke = 1.0;
    }
    if (width + 2.0 * edge) as i32 % 2 != stroke as i32 % 2 {
        // (Of the stroke's parity: the disc a pixel narrower, never the
        // stroke thinner than one.)
        if width > 4.0 { width -= 1.0 } else { stroke += 1.0 }
    }
    // (A slanting stroke needs no parity: a point, a thinner line than
    // the "!"'s bar, which at 1.25 points made the check fill its disc.)
    let line = if matches!(glyph, Glyph::Bang) { stroke } else { s.max(1.0) };
    let whole = width + 2.0 * edge;
    let (x0, y0) = ((f32::from(centre.x) * s - whole / 2.0).round(), (f32::from(centre.y) * s - whole / 2.0).round());
    let (inner, outer) = (width / 2.0, whole / 2.0);
    let n = whole as i32;
    let colours = [ground, ink, rim];
    // Which colour a pixel is (by its offset from the centre, in device
    // pixels): the glyph where most of the pixel is under it.
    let which = |dx: f32, dy: f32| -> Option<usize> {
        let d = (dx * dx + dy * dy).sqrt();
        if d > outer {
            return None;
        }
        if d > inner {
            return Some(2);
        }
        if small {
            return Some(0);
        }
        // (A device pixel of the disc kept round the glyph: where it
        // met the rim, of its colour, the disc read as bitten.)
        if d > inner - 1.0 {
            return Some(0);
        }
        let mut under = 0;
        for i in 0..4 {
            for j in 0..4 {
                if on_glyph(glyph, dx - 0.375 + i as f32 * 0.25, dy - 0.375 + j as f32 * 0.25, inner, line) {
                    under += 1;
                }
            }
        }
        Some(if under >= 7 { 1 } else { 0 })
    };
    for row in 0..n {
        let dy = row as f32 + 0.5 - outer;
        // (A row's pixels of one colour side by side are one piece.)
        let mut run: Option<(i32, usize)> = None;
        for col in 0..=n {
            let here = if col < n { which(col as f32 + 0.5 - outer, dy) } else { None };
            if let Some((from, of)) = run
                && here != Some(of)
            {
                let piece = Bounds { origin: point(px((x0 + from as f32) / s), px((y0 + row as f32) / s)), size: size(px((col - from) as f32 / s), px(1.0 / s)) };
                g.paint_quad(fill(piece, colours[of]));
                run = None;
            }
            if run.is_none() {
                run = here.map(|of| (col, of));
            }
        }
    }
}

/// Keep It Going's mark: a solid triangle pointing right, `height`
/// points tall round `centre`, with a 1-point rim of `rim`; in whole
/// device pixels, its rows one pixel wider each from its tips to its
/// middle (at 1× one, two, three, two and one wide). A shape of its own
/// beside Auto Approve's disc at every size: a triangle drawn in a disc
/// of eight or nine pixels read as a blob in a ring (Opus's review,
/// 2026-11-24).
fn triangle(g: &mut Painter, centre: Point<Pixels>, height: f32, ground: Hsla, rim: Hsla) {
    let s = g.scale_factor();
    let edge = s.round().max(1.0) as i32;
    // (Odd, so it has a middle row.)
    let tall = {
        let h = (height * s).round().max(5.0) as i32;
        if h % 2 == 0 { h - 1 } else { h }
    };
    let half = tall / 2;
    let wide = half + 1;
    let (x0, y0) = ((f32::from(centre.x) * s - wide as f32 / 2.0).round() as i32, (f32::from(centre.y) * s - tall as f32 / 2.0).round() as i32);
    let inside = |col: i32, row: i32| (0..tall).contains(&row) && col >= 0 && col <= half - (row - half).abs();
    for row in -edge..tall + edge {
        // (A row's pixels of one colour side by side are one piece.)
        let mut run: Option<(i32, bool)> = None;
        for col in -edge..=wide + edge {
            let here = if col < wide + edge {
                if inside(col, row) {
                    Some(true)
                } else if (-edge..=edge).any(|i| (-edge..=edge).any(|j| inside(col + i, row + j))) {
                    Some(false)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some((from, filled)) = run
                && here != Some(filled)
            {
                let piece = Bounds { origin: point(px((x0 + from) as f32 / s), px((y0 + row) as f32 / s)), size: size(px((col - from) as f32 / s), px(1.0 / s)) };
                g.paint_quad(fill(piece, if filled { ground } else { rim }));
                run = None;
            }
            if run.is_none() {
                run = here.map(|filled| (col, filled));
            }
        }
    }
}

/// Whether a point (device pixels from the disc's centre; `r` its
/// radius, `t` the stroke) is under the glyph.
fn on_glyph(glyph: Glyph, x: f32, y: f32, r: f32, t: f32) -> bool {
    let near = |a: (f32, f32), b: (f32, f32)| -> bool {
        let (ax, ay, bx, by) = (a.0 * r, a.1 * r, b.0 * r, b.1 * r);
        let (vx, vy) = (bx - ax, by - ay);
        let k = (((x - ax) * vx + (y - ay) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
        let (qx, qy) = (ax + k * vx - x, ay + k * vy - y);
        (qx * qx + qy * qy).sqrt() <= t / 2.0
    };
    match glyph {
        Glyph::None => false,
        Glyph::Bang => x.abs() <= t / 2.0 && ((-0.62 * r..=0.12 * r).contains(&y) || (0.36 * r..=0.36 * r + t).contains(&y)),
        Glyph::Check => near((-0.4, 0.0), (-0.12, 0.28)) || near((-0.12, 0.28), (0.4, -0.26)),
        Glyph::Cross => near((-0.42, -0.42), (0.42, 0.42)) || near((0.42, -0.42), (-0.42, 0.42)),
    }
}

/// `a` with `t` of `b` in it.
fn blend(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rgba { r: l(a.r, b.r), g: l(a.g, b.g), b: l(a.b, b.b), a: l(a.a, b.a) }.into()
}

/// The mark of an agent at work, centred near `centre`: a ring, faint all
/// the way round, with a strong head and a tail fading behind it, turned
/// to `turn` (radians, clockwise). Busy at whatever moment it is seen,
/// and which way it goes, in a still picture too: a three-quarter arc
/// read as a letter there, and a strong third with no tail as a cap
/// sitting on top (the reviewers' looks, 2026-11-21).
///
/// Drawn as the icons are: whole device pixels, each one colour, no
/// smoothed edge (a smoothed ring 7.5 pixels across was a blur beside a
/// drawn icon at 1×, and soft at 1.5×). Its width is even and its centre
/// on a pixel's corner, its stroke a quarter of its width: at 1× eight
/// across with a hole of four, the same shape at every scale. The faint ring is the strong colour in
/// the rim's (`rim`: what the tab is filled with), more of it in dark,
/// where less of it was lost in the tab.
fn working_ring(g: &mut Painter, centre: Point<Pixels>, turn: f32, rim: Hsla, strong: Hsla, dark: bool) {
    use std::f32::consts::{PI, TAU};
    let s = g.scale_factor();
    let outer = ((3.75 * s).round() * 2.0).max(8.0);
    let stroke = (outer / 4.0).round();
    let (cx, cy) = ((f32::from(centre.x) * s).round(), (f32::from(centre.y) * s).round());
    let (r1, r0) = (outer / 2.0, outer / 2.0 - stroke);
    // The faint ring: the accent in the rim's colour, less of it in dark
    // (a strong ring there shaded like a raised knob lit from above, and
    // outweighed the title of a tab behind). The head: the accent, in
    // dark lightened and as blue as it was (near white, it ran into the
    // icon's bezel). The tail's two steps: the larger drop right behind
    // the head, so the leading edge is crisp and the trailing one fades.
    let faint = blend(rim, strong, if dark { 0.33 } else { 0.38 });
    let head = if dark { Hsla { l: strong.l.max(0.76), ..strong } } else { strong };
    let shades = [head, blend(faint, head, if dark { 0.6 } else { 0.72 }), blend(faint, head, if dark { 0.3 } else { 0.36 }), faint];
    // Which shade a pixel has, by how far behind the head it is.
    let shade = |x: f32, y: f32| -> Option<usize> {
        let d = x * x + y * y;
        if d > r1 * r1 || d < r0 * r0 {
            return None;
        }
        // (A sixth of a turn of head, then two steps of tail of a ninth
        // each: at 1× six pixels and four and four of the ring's 36.
        // Shorter, the tail was a pixel or two and the head read as "a
        // quarter done".)
        let behind = (turn - y.atan2(x)).rem_euclid(TAU);
        Some(if behind < PI / 3.0 {
            0
        } else if behind < PI / 3.0 + TAU / 9.0 {
            1
        } else if behind < PI / 3.0 + 2.0 * TAU / 9.0 {
            2
        } else {
            3
        })
    };
    let n = outer as i32;
    for row in 0..n {
        let y = row as f32 - r1 + 0.5;
        // (A row's pixels of one shade side by side are one piece.)
        let mut run: Option<(i32, usize)> = None;
        for col in 0..=n {
            let here = if col < n { shade(col as f32 - r1 + 0.5, y) } else { None };
            if let Some((from, of)) = run
                && here != Some(of)
            {
                let piece = Bounds { origin: point(px((cx - r1 + from as f32) / s), px((cy - r1 + row as f32) / s)), size: size(px((col - from) as f32 / s), px(1.0 / s)) };
                g.paint_quad(fill(piece, shades[of]));
                run = None;
            }
            if run.is_none() {
                run = here.map(|of| (col, of));
            }
        }
    }
}
