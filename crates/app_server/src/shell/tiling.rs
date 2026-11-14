//! Arranging windows from the keyboard (olo_protocol::shortcuts): tiling
//! one into part of its display, centering it, moving it to another
//! display, tiling, cascading or fanning them all out (and undoing that),
//! showing the desktop; zooming one along a single axis (top to bottom
//! or side to side, the other as it is); and putting
//! windows back on a display when theirs goes away (unplugged, or set to
//! mirror). Arranged and zoomed windows keep space around them
//! (`Settings::tiling`: gap and margin, Preferences ▸ Windows), are
//! refitted when it changes (`tiling_space_changed`), and remember how
//! they were, so Restore (or unzooming) puts them back.
//!
//! Tiled windows that meet move together: dragging the edge of one moves
//! the edges of the other tiled windows on that line (beside it across the
//! gap, or in line with it), as the split between them. They stay tiled,
//! in the parts they now have. Windows not tiled by a command are never
//! pushed, and a tiled window sized by hand with no tiled neighbours is
//! no longer tiled. Neighbours are paced like the dragged window: a new
//! size only once they have answered the last, and moved only when they
//! draw it (`Follower`), so nothing queues up or jumps ahead of its
//! picture.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use olo_protocol::Tile;
use smithay::{
    desktop::{WindowSurface, space::SpaceElement},
    output::Output,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{IsAlive, Logical, Point, Rectangle, Size},
};
use tracing::{debug, info};

use super::{
    WindowElement,
    wm::{PreZoom, ZoomedFrom},
};
use crate::state::{AnvilState, Backend};

/// The part of the display a window was tiled into (re-tiled when its
/// display changes).
#[derive(Default)]
struct Tiled(Cell<Option<Tile>>);

/// A part of the tile area, as fractions (x, y, width, height), as
/// `Tile::fraction` gives.
pub type Part = (f32, f32, f32, f32);

/// A window's own arrangement, kept while it wears its group's (or
/// another display's group's, on a trip there and back).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrangement {
    tile: Option<Tile>,
    part: Option<Part>,
    along: Option<ZoomedAlong>,
    /// Zoomed, and the frames Restore and unzooming go back to.
    zoomed: bool,
    pre: Option<Rectangle<i32, Logical>>,
    zoomed_from: Option<Rectangle<i32, Logical>>,
}

/// Where a window was before an arrangement moved it, all Undo Window
/// Arrangement needs to put it back.
/// The last window arrangement, for Undo and Redo Window Arrangement: its
/// name (Edit-menu style: Undo Cascade), what kind it was, and whether it
/// has been undone (Redo puts it again).
#[derive(Clone, Debug)]
pub struct Arranged {
    pub name: String,
    pub kind: ArrangedKind,
    pub undone: bool,
}

#[derive(Clone, Debug)]
pub enum ArrangedKind {
    /// Windows placed (Tile All, Cascade, Fan Out, Show All, an
    /// arranger's): `State::arranged` holds how they were.
    Placed,
    /// Auto-tiling's tiles changed (its own undo holds the tree).
    Auto,
    /// Auto-tiling turned off by undoing turning it on: Redo turns it on
    /// again on that workspace's displays.
    AutoOn(u32, Vec<String>),
    /// Windows moved to another workspace or display (Deskbar's Move To,
    /// the keys): where each was (display, workspace).
    Moved(Vec<(WindowElement, Option<String>, u32)>),
}

#[derive(Clone, Debug)]
pub struct Placed {
    window: WindowElement,
    frame: Rectangle<i32, Logical>,
    tile: Option<Tile>,
    part: Option<Part>,
    /// Its own frame, if it was tiled or zoomed (for Restore).
    pre: Option<Rectangle<i32, Logical>>,
    /// Zoomed, or zoomed along one axis, and where unzooming goes back
    /// to: it is zoomed again, to unzoom as before.
    zoomed: bool,
    along: Option<ZoomedAlong>,
    zoomed_from: Option<Rectangle<i32, Logical>>,
    tab_offset: i32,
}

/// The part a tiled window has after its edges were dragged, in place of
/// its tile's own.
#[derive(Default)]
struct TilePart(Cell<Option<Part>>);

/// The way Zoom Vertically and Zoom Horizontally zoom a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Top to bottom.
    Vertical,
    /// Side to side.
    Horizontal,
}

impl Axis {
    fn other(self) -> Axis {
        match self {
            Axis::Vertical => Axis::Horizontal,
            Axis::Horizontal => Axis::Vertical,
        }
    }
}

/// A window zoomed along one axis, and what unzooming it that way puts
/// back: the frame it was zoomed from, whether its own frame was kept
/// already (tiled, arranged or zoomed: Restore still goes back there),
/// and the tile it was in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ZoomedAlong {
    axis: Axis,
    from: Rectangle<i32, Logical>,
    kept: bool,
    tile: Option<(Tile, Option<Part>)>,
}

#[derive(Default)]
struct Along(Cell<Option<ZoomedAlong>>);

/// What goes along when a window moves to another display: its frame,
/// unless it follows the display (tiled or zoomed; zoomed along one axis,
/// the other is its own), the frame Restore puts back, and the frames
/// unzooming it puts back.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Carried {
    frame: Option<Rectangle<i32, Logical>>,
    pre: Option<Rectangle<i32, Logical>>,
    along_from: Option<Rectangle<i32, Logical>>,
    zoomed_from: Option<Rectangle<i32, Logical>>,
}

/// A window's last move to another display: the display it left and
/// its room there, what it had there, and what it was given on arrival.
/// Sent straight back, untouched, to that display as it was, it has what
/// it had again, rather than placed afresh (which would keep it shrunk to
/// a smaller display, and drift by rounding).
#[derive(Debug, Clone, PartialEq)]
struct Trip {
    home: String,
    home_area: Rectangle<i32, Logical>,
    left: Carried,
    /// How it was arranged when it left (its tile is its own, not
    /// carried: joining a group there hands it the group's).
    left_arrangement: Arrangement,
    arrived: Carried,
    /// It joined a group there by itself on arriving, at this frame
    /// (its app's, `sync_groups`): still there, untouched, it is as it
    /// arrived.
    joined: Option<Rectangle<i32, Logical>>,
}

/// `w`, just arrived from another display, joined a group there at
/// `frame` by itself: going back is still the round trip.
pub(crate) fn note_joined(w: &WindowElement, frame: Rectangle<i32, Logical>) {
    if let Some(t) = w.user_data().get::<Travelled>()
        && let Some(trip) = t.0.borrow_mut().as_mut()
    {
        trip.joined = Some(frame);
    }
}

#[derive(Default)]
struct Travelled(RefCell<Option<Trip>>);

/// `frame` zoomed along `axis` of `area`: all of it that way, and as it
/// is the other (moved in only as far as it must to be on it).
pub fn zoom_along(frame: Rectangle<i32, Logical>, area: Rectangle<i32, Logical>, axis: Axis) -> Rectangle<i32, Logical> {
    let fit = |at: i32, len: i32, start: i32, room: i32| {
        let len = len.min(room);
        (at.clamp(start, start + room - len), len)
    };
    match axis {
        Axis::Vertical => {
            let (x, w) = fit(frame.loc.x, frame.size.w, area.loc.x, area.size.w);
            Rectangle::new(Point::from((x, area.loc.y)), Size::from((w, area.size.h)))
        }
        Axis::Horizontal => {
            let (y, h) = fit(frame.loc.y, frame.size.h, area.loc.y, area.size.h);
            Rectangle::new(Point::from((area.loc.x, y)), Size::from((area.size.w, h)))
        }
    }
}

/// `frame` unzoomed along `axis`: that way as `from` was, the other as
/// it is.
pub fn unzoom_along(frame: Rectangle<i32, Logical>, from: Rectangle<i32, Logical>, axis: Axis) -> Rectangle<i32, Logical> {
    match axis {
        Axis::Vertical => Rectangle::new(Point::from((frame.loc.x, from.loc.y)), Size::from((frame.size.w, from.size.h))),
        Axis::Horizontal => Rectangle::new(Point::from((from.loc.x, frame.loc.y)), Size::from((from.size.w, frame.size.h))),
    }
}

/// A window zoomed along `axis` of `area` from `from`, now at `frame`,
/// unzoomed: all of it as `from` was if zooming only moved it in or
/// narrowed it the other way to fit (and it hasn't moved since), else
/// that way as `from` was and the other as it is (`unzoom_along`).
pub fn unzoomed_along(frame: Rectangle<i32, Logical>, from: Rectangle<i32, Logical>, area: Rectangle<i32, Logical>, axis: Axis) -> Rectangle<i32, Logical> {
    if frame == zoom_along(from, area, axis) { from } else { unzoom_along(frame, from, axis) }
}

/// `frame` covers all of `area` along `axis`.
fn spans(frame: Rectangle<i32, Logical>, area: Rectangle<i32, Logical>, axis: Axis) -> bool {
    match axis {
        Axis::Vertical => frame.loc.y == area.loc.y && frame.size.h == area.size.h,
        Axis::Horizontal => frame.loc.x == area.loc.x && frame.size.w == area.size.w,
    }
}

/// The narrowest (and shortest) a tiled neighbour is pushed to.
const MIN_TILE: i32 = 160;

/// A side of a window's frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// A tiled window whose `side` follows the edge being dragged.
#[derive(Debug, Clone)]
struct Link {
    window: WindowElement,
    /// Its frame when the drag began.
    start: Rectangle<i32, Logical>,
    side: Side,
    /// The dragged window's side it follows.
    follows: Side,
    /// The smallest frame it takes (its app's minimum, or `MIN_TILE`).
    min: Size<i32, Logical>,
}

/// A tiled neighbour following a drag (and catching up after it): its
/// frame when the drag began and the one asked of it now (`asked` then).
#[derive(Debug)]
pub struct Follower {
    window: WindowElement,
    start: Rectangle<i32, Logical>,
    target: Rectangle<i32, Logical>,
    asked: Instant,
    /// The asks before this one (start and target), newest first, a few:
    /// a frame drawn for one of them (a round on its way as the drag was
    /// let go, and the tiles laid out again) goes where that one put it,
    /// not where the newest would put a frame of another size.
    before: Vec<(Rectangle<i32, Logical>, Rectangle<i32, Logical>)>,
}

/// Asks a follower remembers ([`Follower::before`]).
const ASKS_KEPT: usize = 3;

/// The longest a window is followed, waiting for it to draw the size it
/// was asked for, after it last was: an app that never does is left where
/// it is.
const FOLLOW_LIMIT: Duration = Duration::from_secs(2);

/// Whether `window`'s app lets it be `frame` big: within its least and
/// most sizes.
fn can_be(window: &WindowElement, frame: Size<i32, Logical>) -> bool {
    use smithay::wayland::{compositor::with_states, shell::xdg::SurfaceCachedState};
    let content = window.inner_size(frame);
    let Some(surface) = window.wl_surface() else { return true };
    let (min, max) = with_states(&surface, |st| {
        let s = *st.cached_state.get::<SurfaceCachedState>().current();
        (s.min_size, s.max_size)
    });
    let fits = |n: i32, lo: i32, hi: i32| n >= lo && (hi <= 0 || n <= hi);
    fits(content.w, min.w, max.w) && fits(content.h, min.h, max.h)
}

/// Each linked neighbour once, with its frame when the drag began and the
/// one it takes when the dragged window's size has changed by `d` (every
/// side of it that follows moved).
fn targets(links: &[Link], d: (i32, i32)) -> Vec<(WindowElement, Rectangle<i32, Logical>, Rectangle<i32, Logical>)> {
    fold_sides(links.iter().map(|l| (l.window.clone(), l.start, l.side, l.follows)), d)
}

/// `targets` for any key: (key, start, its side, the side it follows).
fn fold_sides<K: PartialEq>(links: impl Iterator<Item = (K, Rectangle<i32, Logical>, Side, Side)>, d: (i32, i32)) -> Vec<(K, Rectangle<i32, Logical>, Rectangle<i32, Logical>)> {
    let mut out: Vec<(K, Rectangle<i32, Logical>, Rectangle<i32, Logical>)> = Vec::new();
    for (k, start, side, follows) in links {
        match out.iter_mut().find(|(w, ..)| *w == k) {
            Some((_, _, r)) => *r = with_side(*r, side, moved(follows, d)),
            None => out.push((k, start, with_side(start, side, moved(follows, d)))),
        }
    }
    out
}

/// A tiled window's edges being dragged, and the neighbours that follow.
#[derive(Debug)]
pub struct TileDrag {
    window: WindowElement,
    /// Its frame when the drag began, and the sides being dragged.
    start: Rectangle<i32, Logical>,
    sides: Vec<Side>,
    links: Vec<Link>,
    /// The smallest frame it takes (its group's: a tab behind may take
    /// more than the front one's app).
    min: Size<i32, Logical>,
    /// Auto-tiled: each window's least size in its tree's terms
    /// (`auto_mins`), for `auto_keeps`.
    mins: HashMap<u64, (f32, f32)>,
    /// The size change last applied to the neighbours, and the one the
    /// drag has got to (which the next round applies: `rounds`).
    applied: (i32, i32),
    wanted: (i32, i32),
}

impl TileDrag {
    pub fn window(&self) -> WindowElement {
        self.window.clone()
    }

    /// `w` is the window dragged, or a neighbour following it.
    pub fn moves(&self, w: &WindowElement) -> bool {
        self.window == *w || self.links.iter().any(|l| l.window == *w)
    }
}

/// How far `side` of the dragged window moves when its size grows by
/// (dw, dh).
fn moved(side: Side, (dw, dh): (i32, i32)) -> i32 {
    match side {
        Side::Left => -dw,
        Side::Right => dw,
        Side::Top => -dh,
        Side::Bottom => dh,
    }
}

/// `r` with `side` moved by `m`.
fn with_side(r: Rectangle<i32, Logical>, side: Side, m: i32) -> Rectangle<i32, Logical> {
    let (mut l, mut t, mut rr, mut b) = (r.loc.x, r.loc.y, r.loc.x + r.size.w, r.loc.y + r.size.h);
    match side {
        Side::Left => l += m,
        Side::Right => rr += m,
        Side::Top => t += m,
        Side::Bottom => b += m,
    }
    Rectangle::new((l, t).into(), ((rr - l).max(1), (b - t).max(1)).into())
}

/// The fractions of `area` that `part` would turn into `r` (its inverse).
pub fn fraction_of(area: Rectangle<i32, Logical>, r: Rectangle<i32, Logical>, gap: i32) -> Part {
    let half = gap / 2;
    let (aw, ah) = (area.size.w.max(1), area.size.h.max(1));
    let (mut l, mut rr) = (r.loc.x - area.loc.x, r.loc.x + r.size.w - area.loc.x);
    let (mut t, mut b) = (r.loc.y - area.loc.y, r.loc.y + r.size.h - area.loc.y);
    if l > 0 {
        l -= gap - half;
    }
    if rr < aw {
        rr += half;
    }
    if t > 0 {
        t -= gap - half;
    }
    if b < ah {
        b += half;
    }
    let f = |v: i32, len: i32| v.clamp(0, len) as f32 / len as f32;
    (f(l, aw), f(t, ah), f(rr, aw) - f(l, aw), f(b, ah) - f(t, ah))
}

/// The sides a resize drags.
pub fn sides_of(left: bool, right: bool, top: bool, bottom: bool) -> Vec<Side> {
    [(left, Side::Left), (right, Side::Right), (top, Side::Top), (bottom, Side::Bottom)].into_iter().filter(|(on, _)| *on).map(|(_, s)| s).collect()
}

/// The other tiled windows' sides on the line of `dragged`'s `side`: those
/// meeting it across the gap, and those in line with it. `others` are
/// their frames.
fn links_for(dragged: Rectangle<i32, Logical>, sides: &[Side], others: &[(WindowElement, Rectangle<i32, Logical>)], gap: i32) -> Vec<Link> {
    const NEAR: i32 = 2;
    let mut out = Vec::new();
    let right = |r: &Rectangle<i32, Logical>| r.loc.x + r.size.w;
    let bottom = |r: &Rectangle<i32, Logical>| r.loc.y + r.size.h;
    for &follows in sides {
        for (w, r) in others {
            let side = match follows {
                Side::Left if (right(r) + gap - dragged.loc.x).abs() <= NEAR => Some(Side::Right),
                Side::Left if (r.loc.x - dragged.loc.x).abs() <= NEAR => Some(Side::Left),
                Side::Right if (r.loc.x - gap - right(&dragged)).abs() <= NEAR => Some(Side::Left),
                Side::Right if (right(r) - right(&dragged)).abs() <= NEAR => Some(Side::Right),
                Side::Top if (bottom(r) + gap - dragged.loc.y).abs() <= NEAR => Some(Side::Bottom),
                Side::Top if (r.loc.y - dragged.loc.y).abs() <= NEAR => Some(Side::Top),
                Side::Bottom if (r.loc.y - gap - bottom(&dragged)).abs() <= NEAR => Some(Side::Top),
                Side::Bottom if (bottom(r) - bottom(&dragged)).abs() <= NEAR => Some(Side::Bottom),
                _ => None,
            };
            if let Some(side) = side {
                out.push(Link { window: w.clone(), start: *r, side, follows, min: Size::from((MIN_TILE, MIN_TILE)) });
            }
        }
    }
    out
}

/// The frames a drag that changed the dragged window's size by `d` asks:
/// its neighbours' (`targets`) and its own.
fn drag_frames(t: &TileDrag, d: (i32, i32)) -> Vec<(WindowElement, Rectangle<i32, Logical>)> {
    let mut frames: Vec<(WindowElement, Rectangle<i32, Logical>)> = targets(&t.links, d).into_iter().map(|(w, _, r)| (w, r)).collect();
    frames.push((t.window.clone(), t.sides.iter().fold(t.start, |r, &side| with_side(r, side, moved(side, d)))));
    frames
}

/// The furthest from 0 towards `to` that `ok` takes, given it takes 0
/// and what it takes is all of a piece (a least size holds a split at
/// one end of what it may be).
fn furthest(to: i32, ok: impl Fn(i32) -> bool) -> i32 {
    if ok(to) {
        return to;
    }
    let (mut lo, mut hi) = (0, to.abs());
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if ok(mid * to.signum()) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo * to.signum()
}

/// The size change (dw, dh) cut down so no linked neighbour gets smaller
/// than `MIN_TILE`.
fn limit(links: &[Link], (mut dw, mut dh): (i32, i32)) -> (i32, i32) {
    for l in links {
        let (len, min) = if matches!(l.side, Side::Left | Side::Right) { (l.start.size.w, l.min.w) } else { (l.start.size.h, l.min.h) };
        // How far its side may move: out freely, in down to the minimum.
        let (lo, hi) = match l.side {
            Side::Left | Side::Top => (i32::MIN / 2, (len - min).max(0)),
            Side::Right | Side::Bottom => ((min - len).min(0), i32::MAX / 2),
        };
        let m = moved(l.follows, (dw, dh)).clamp(lo, hi);
        match l.follows {
            Side::Left => dw = -m,
            Side::Right => dw = m,
            Side::Top => dh = -m,
            Side::Bottom => dh = m,
        }
    }
    (dw, dh)
}

/// The diagonal step between cascaded windows, as when they open.
pub(super) const CASCADE: i32 = 24;

/// `area` less `margin` all round.
pub(super) fn inset(area: Rectangle<i32, Logical>, margin: i32) -> Rectangle<i32, Logical> {
    let m = margin.min(area.size.w / 4).min(area.size.h / 4).max(0);
    Rectangle::new(area.loc + Point::from((m, m)), area.size - Size::from((2 * m, 2 * m)))
}

/// The part (x, y, w, h fractions) of `area`, with `gap` between it and
/// whatever is beside it (edges inside the area give up half each).
pub fn part(area: Rectangle<i32, Logical>, (x, y, w, h): (f32, f32, f32, f32), gap: i32) -> Rectangle<i32, Logical> {
    let at = |f: f32, len: i32| (f * len as f32).round() as i32;
    let (mut l, mut r) = (at(x, area.size.w), at(x + w, area.size.w));
    let (mut t, mut b) = (at(y, area.size.h), at(y + h, area.size.h));
    let half = gap / 2;
    if l > 0 {
        l += gap - half;
    }
    if r < area.size.w {
        r -= half;
    }
    if t > 0 {
        t += gap - half;
    }
    if b < area.size.h {
        b -= half;
    }
    Rectangle::new(area.loc + Point::from((l, t)), Size::from(((r - l).max(1), (b - t).max(1))))
}

/// `n` windows in a grid over `area`: as square as the area allows, the
/// last row's windows wider if it has fewer.
pub fn grid(area: Rectangle<i32, Logical>, n: usize, gap: i32) -> Vec<Rectangle<i32, Logical>> {
    if n == 0 {
        return Vec::new();
    }
    // The columns whose worst cell (the last row's are wider) is closest
    // to a window's usual shape, 4:3.
    let (w, h) = (area.size.w.max(1) as f32, area.size.h.max(1) as f32);
    let cost = |cols: usize| {
        let rows = n.div_ceil(cols);
        let last = n - (rows - 1) * cols;
        let off = |a: f32| (a / (4.0 / 3.0)).ln().abs();
        off((w / cols as f32) / (h / rows as f32)).max(off((w / last as f32) / (h / rows as f32)))
    };
    let cols = (1..=n).min_by(|a, b| cost(*a).total_cmp(&cost(*b))).unwrap_or(1);
    let rows = n.div_ceil(cols);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (row, col) = (i / cols, i % cols);
        let in_row = if row == rows - 1 { n - row * cols } else { cols };
        let f = (col as f32 / in_row as f32, row as f32 / rows as f32, 1.0 / in_row as f32, 1.0 / rows as f32);
        out.push(part(area, f, gap));
    }
    out
}

/// Rolled-up windows sharing `cell`, fanned out in it: each takes the
/// whole cell (so it fills it when unrolled), their tabs (`tabs` wide)
/// side by side along its top as Fan Out puts them, a row of tabs lower
/// (`tab_h`) when a row is full. Each one's frame and tab offset.
pub fn fan_in(cell: Rectangle<i32, Logical>, tabs: &[i32], tab_h: i32) -> Vec<(Rectangle<i32, Logical>, i32)> {
    let (mut x, mut row) = (0, 0);
    tabs.iter()
        .map(|&tab| {
            if x > 0 && x + tab > cell.size.w {
                (x, row) = (0, row + 1);
            }
            let down = (row * tab_h).min((cell.size.h - tab_h).max(0));
            let frame = Rectangle::new(cell.loc + Point::from((0, down)), Size::from((cell.size.w, (cell.size.h - down).max(1))));
            let offset = x;
            x += tab;
            (frame, offset)
        })
        .collect()
}

/// How much of a window's top edge must be in the area for it to be in
/// reach: enough of its tab or frame to take hold of.
const REACH: (i32, i32) = (48, olo_theme::deco::TAB_HEIGHT);
/// The least of a window's top edge on a display that still takes hold of
/// it (a window left hanging off an edge is put back so, not pulled in).
const GRAB: i32 = 32;

/// Whether a window at `frame` can be taken hold of in `area`: its top
/// edge is inside the area top to bottom, and enough of it across.
pub fn in_reach(frame: Rectangle<i32, Logical>, area: Rectangle<i32, Logical>) -> bool {
    let (w, h) = REACH;
    let top = Rectangle::new(frame.loc, Size::from((frame.size.w, h.min(frame.size.h))));
    frame.loc.y >= area.loc.y && frame.loc.y + h <= area.loc.y + area.size.h && top.intersection(area).is_some_and(|r| r.size.w >= w.min(frame.size.w))
}

/// A window moved by `delta` along with its display: the frames it
/// remembers (Restore's, and what unzooming puts back) move with it.
pub fn shift_remembered(window: &WindowElement, delta: Point<i32, Logical>) {
    if delta == Point::default() {
        return;
    }
    let shift = |r: Rectangle<i32, Logical>| Rectangle::new(r.loc + delta, r.size);
    if let Some(p) = window.user_data().get::<PreZoom>() {
        let moved = p.0.borrow().map(shift);
        *p.0.borrow_mut() = moved;
    }
    if let Some(f) = window.user_data().get::<ZoomedFrom>() {
        let moved = f.0.borrow().map(shift);
        *f.0.borrow_mut() = moved;
    }
    if let Some(a) = window.user_data().get::<Along>() {
        a.0.set(a.0.get().map(|z| ZoomedAlong { from: shift(z.from), ..z }));
    }
}

/// `frame` moved as little as it takes to be in reach in `area`: it may
/// still hang off any edge but the top.
pub fn into_reach(frame: Rectangle<i32, Logical>, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let (w, h) = REACH;
    let w = w.min(frame.size.w);
    let x = frame.loc.x.clamp(area.loc.x + w - frame.size.w, area.loc.x + area.size.w - w);
    let y = frame.loc.y.clamp(area.loc.y, (area.loc.y + area.size.h - h).max(area.loc.y));
    Rectangle::new(Point::from((x, y)), frame.size)
}

/// Not a fixed-size window (an alert, a panel such as DeskCalc's).
pub(crate) fn resizable(w: &WindowElement) -> bool {
    use smithay::wayland::{compositor::with_states, shell::xdg::SurfaceCachedState};
    w.wl_surface().is_none_or(|s| {
        with_states(&s, |st| {
            let mut cached = st.cached_state.get::<SurfaceCachedState>();
            let c = cached.current();
            (c.max_size.w == 0 || c.max_size.w > c.min_size.w) && (c.max_size.h == 0 || c.max_size.h > c.min_size.h)
        })
    })
}

impl<BackendData: Backend> AnvilState<BackendData> {
    pub(super) fn tile_gap(&self) -> i32 {
        self.settings.tiling.gap.min(200) as i32
    }

    /// Where tiled and zoomed windows go on a display: its usable area
    /// inside the margins.
    pub(super) fn tile_area(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        Some(inset(self.usable_area(output)?, self.settings.tiling.margin.min(400) as i32))
    }

    /// The display shortcuts act on: the focused window's, else the one a
    /// workspace was last switched on or the pointer last pressed on,
    /// else the pointer's.
    pub fn active_output(&self) -> Option<Output> {
        if let Some(w) = self.focused_window() {
            return self.window_output(&w);
        }
        // Where a workspace was last switched, or a press last landed.
        if let Some(o) = self.last_display.as_ref().and_then(|name| self.space.outputs().find(|o| o.name() == *name)) {
            return Some(o.clone());
        }
        let pointer = self.pointer.current_location();
        self.space.output_under(pointer).next().cloned().or_else(|| self.main_output())
    }

    /// Put a window's frame (tab and border included) at `frame`, no longer
    /// zoomed. `remember`: keep where it was, for Restore.
    pub fn set_frame(&mut self, window: &WindowElement, frame: Rectangle<i32, Logical>, remember: bool) {
        let Some(location) = self.space.element_location(window) else { return };
        debug!(window = window.id(), ?frame, remember, "set frame");
        if remember {
            window.user_data().insert_if_missing(PreZoom::default);
            let pre = window.user_data().get::<PreZoom>().unwrap();
            pre.0.borrow_mut().get_or_insert(Rectangle::new(location, window.geometry().size));
        }
        // Put here now: no longer on its way to where a drag or a layout
        // asked before (that would put it back by its old edges when it
        // draws this size).
        self.unfollow(window);
        // (Before taking the surface's lock: inner_size takes it too.)
        let content = window.inner_size(frame.size);
        match window.0.underlying_surface() {
            WindowSurface::Wayland(t) => {
                t.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                    state.size = Some(content);
                });
                if t.is_initial_configure_sent() {
                    t.send_pending_configure();
                }
            }
        }
        self.space.relocate_element(window, frame.loc);
    }

    pub(super) fn tiled(window: &WindowElement) -> Option<Tile> {
        window.user_data().get::<Tiled>().and_then(|t| t.0.get())
    }

    /// Tiled into `tile` (or not), in its tile's own part. Either way
    /// no longer zoomed along one axis: every arrangement, and any hand
    /// move or resize, comes through here.
    fn set_tiled(window: &WindowElement, tile: Option<Tile>) {
        window.user_data().insert_if_missing(Tiled::default);
        window.user_data().get::<Tiled>().unwrap().0.set(tile);
        Self::set_part(window, None);
        Self::set_zoomed_along(window, None);
    }

    fn zoomed_along(window: &WindowElement) -> Option<ZoomedAlong> {
        window.user_data().get::<Along>().and_then(|a| a.0.get())
    }

    fn set_zoomed_along(window: &WindowElement, z: Option<ZoomedAlong>) {
        window.user_data().insert_if_missing(Along::default);
        window.user_data().get::<Along>().unwrap().0.set(z);
    }

    /// The axis a window is zoomed along alone, if it is.
    pub fn zoomed_axis(window: &WindowElement) -> Option<Axis> {
        Self::zoomed_along(window).map(|z| z.axis)
    }

    /// Which ways (width, height) Olo holds a window to the size it gave
    /// it: both, zoomed or tiled; the one way, zoomed along one axis.
    pub fn held_size(&self, window: &WindowElement) -> (bool, bool) {
        if self.is_zoomed(window) || Self::tiled(window).is_some() {
            return (true, true);
        }
        match Self::zoomed_axis(window) {
            Some(Axis::Vertical) => (false, true),
            Some(Axis::Horizontal) => (true, false),
            None => (false, false),
        }
    }

    /// Tell each window which of its sides are held (xdg's tiled states):
    /// all four in a tile (tiling's or auto-tiling's), top and bottom
    /// zoomed vertically, left and right horizontally; so an app sizing
    /// itself (Terminal keeping its columns) asks only for what it can
    /// have, and doesn't flicker to a size that is put back.
    pub fn sync_held_sides(&mut self) {
        use xdg_toplevel::State;
        let windows: Vec<WindowElement> = self.space.elements().cloned().collect();
        for w in windows {
            let (across, down) = if self.auto_tiled(&w) { (true, true) } else { self.held_size(&w) };
            let WindowSurface::Wayland(t) = w.0.underlying_surface();
            let changed = t.with_pending_state(|s| {
                let mut changed = false;
                for (state, on) in [(State::TiledLeft, across), (State::TiledRight, across), (State::TiledTop, down), (State::TiledBottom, down)] {
                    if s.states.contains(state) != on {
                        changed = true;
                        if on {
                            s.states.set(state);
                        } else {
                            s.states.unset(state);
                        }
                    }
                }
                changed
            });
            if changed && t.is_initial_configure_sent() {
                t.send_pending_configure();
            }
        }
    }

    /// Zoom a window along `axis` (Zoom Vertically, Zoom Horizontally),
    /// or unzoom it that way, the other way left as it is. Zoomed along
    /// both, it is zoomed; a zoomed window zoomed along one is unzoomed
    /// the other way. Unzoomed, it goes back that way to where it was
    /// (into its tile, if it was in one).
    pub fn toggle_zoom_along(&mut self, window: &WindowElement, axis: Axis) {
        let (Some(area), Some(here)) = (self.zoom_area(window), self.space.element_geometry(window)) else { return };
        let pre = window.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow());
        let along = Self::zoomed_along(window);
        let tile = Self::tiled(window).map(|t| (t, Self::custom_part(window)));
        if self.is_zoomed(window) {
            // Zoomed both ways: just the other way now.
            let zoomed_from = window.user_data().get::<ZoomedFrom>().and_then(|z| z.0.borrow_mut().take());
            let stays = axis.other();
            let z = match along {
                Some(z) => ZoomedAlong { axis: stays, ..z },
                None => ZoomedAlong { axis: stays, from: zoomed_from.or(pre).unwrap_or(here), kept: zoomed_from.is_some(), tile },
            };
            Self::set_tiled(window, None);
            // Back to the frame it had zoomed that way, as it was when it
            // zoomed both ways (`ZoomedFrom`), not as that way's zoom began:
            // sized along the other axis since, or taken from its group's,
            // it came back at the old size.
            let frame = match (along, zoomed_from) {
                (Some(a), Some(f)) if a.axis == stays => f,
                _ => zoom_along(z.from, area, stays),
            };
            self.set_frame(window, frame, true);
            Self::set_zoomed_along(window, Some(z));
        } else if let Some(z) = along.filter(|z| z.axis == axis) {
            // Back that way.
            Self::set_zoomed_along(window, None);
            match z.tile {
                Some((tile, part)) => {
                    Self::set_tile_state(window, Some(tile), part);
                    self.retile(window, tile);
                }
                None => self.set_frame(window, unzoomed_along(here, z.from, area, axis), false),
            }
            // (After: retiling keeps where it is.)
            if !z.kept
                && let Some(p) = window.user_data().get::<PreZoom>()
            {
                p.0.borrow_mut().take();
            }
        } else if along.is_some() {
            // Zoomed the other way already: both ways is zoomed, and
            // unzooms back to just that way, as it is now. (Not as the
            // way's zoom began: sized by hand along the other axis since,
            // or taken from its group's, it came back at that width.)
            self.zoom_window(window);
            if let Some(f) = window.user_data().get::<ZoomedFrom>() {
                *f.0.borrow_mut() = Some(here);
            }
        } else if let Some(pre) = pre.filter(|_| tile.is_none() && spans(here, area, axis)) {
            // Zoomed this way, then sized or moved by hand the other way
            // (which forgot it): back that way all the same.
            if let Some(p) = window.user_data().get::<PreZoom>() {
                p.0.borrow_mut().take();
            }
            self.set_frame(window, unzoom_along(here, pre, axis), false);
        } else {
            let z = ZoomedAlong { axis, from: here, kept: pre.is_some(), tile };
            Self::set_tiled(window, None);
            self.set_frame(window, zoom_along(here, area, axis), true);
            Self::set_zoomed_along(window, Some(z));
        }
        info!(window = window.id(), ?axis, along = ?Self::zoomed_axis(window), zoomed = self.is_zoomed(window), "zoomed along");
    }

    fn set_part(window: &WindowElement, part: Option<Part>) {
        window.user_data().insert_if_missing(TilePart::default);
        window.user_data().get::<TilePart>().unwrap().0.set(part);
    }

    pub(super) fn custom_part(window: &WindowElement) -> Option<Part> {
        window.user_data().get::<TilePart>().and_then(|p| p.0.get())
    }

    /// How a window is tiled (its tile, and the part its edges were
    /// dragged to), to keep across a restart.
    pub fn tile_state(window: &WindowElement) -> (Option<Tile>, Option<Part>) {
        (Self::tiled(window), Self::tiled(window).and(Self::custom_part(window)))
    }

    /// A window back from a restart, tiled as it was.
    pub fn set_tile_state(window: &WindowElement, tile: Option<Tile>, part: Option<Part>) {
        Self::set_tiled(window, tile);
        Self::set_part(window, tile.and(part));
    }

    /// `w`'s own arrangement (its tile, the part its edges were dragged
    /// to, its zoom along one axis), to give back later
    /// ([`Self::rearrange`]).
    pub(crate) fn arrangement_of(w: &WindowElement) -> Arrangement {
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        Arrangement {
            tile: Self::tiled(w),
            part: Self::custom_part(w),
            along: Self::zoomed_along(w),
            zoomed: t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Maximized)),
            pre: w.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow()),
            zoomed_from: w.user_data().get::<ZoomedFrom>().and_then(|z| *z.0.borrow()),
        }
    }

    /// `w` arranged as it was ([`Self::arrangement_of`]), zoomed again if
    /// it was (told so now; its frame is put back by whoever called,
    /// `refit` say).
    pub(crate) fn rearrange(w: &WindowElement, a: Arrangement) {
        Self::set_tile_state(w, a.tile, a.part);
        Self::set_zoomed_along(w, a.along);
        w.user_data().insert_if_missing(PreZoom::default);
        w.user_data().insert_if_missing(ZoomedFrom::default);
        *w.user_data().get::<PreZoom>().unwrap().0.borrow_mut() = a.pre;
        *w.user_data().get::<ZoomedFrom>().unwrap().0.borrow_mut() = a.zoomed_from;
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        t.with_pending_state(|s| {
            if a.zoomed {
                s.states.set(xdg_toplevel::State::Maximized);
            } else {
                s.states.unset(xdg_toplevel::State::Maximized);
            }
        });
        if t.is_initial_configure_sent() {
            t.send_pending_configure();
        }
    }

    /// Give `w` the arrangement `leader` has: its tile and part, its zoom
    /// (along an axis too), and the frames Restore and unzooming go back
    /// to. A group is one window to arranging, so whichever of its
    /// windows is in front later (a tab clicked, a window back from
    /// another workspace), the group stays where it is.
    pub(super) fn share_arrangement(&mut self, leader: &WindowElement, w: &WindowElement) {
        Self::set_tile_state(w, Self::tiled(leader), Self::custom_part(leader));
        Self::set_zoomed_along(w, Self::zoomed_along(leader));
        let pre = leader.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow());
        let from = leader.user_data().get::<ZoomedFrom>().and_then(|z| *z.0.borrow());
        w.user_data().insert_if_missing(PreZoom::default);
        w.user_data().insert_if_missing(ZoomedFrom::default);
        *w.user_data().get::<PreZoom>().unwrap().0.borrow_mut() = pre;
        *w.user_data().get::<ZoomedFrom>().unwrap().0.borrow_mut() = from;
        let zoomed = self.is_zoomed(leader);
        if self.is_zoomed(w) != zoomed {
            let WindowSurface::Wayland(t) = w.0.underlying_surface();
            t.with_pending_state(|s| {
                if zoomed {
                    s.states.set(xdg_toplevel::State::Maximized);
                } else {
                    s.states.unset(xdg_toplevel::State::Maximized);
                }
            });
            if t.is_initial_configure_sent() {
                t.send_pending_configure();
            }
        }
    }

    /// Tile a window into part of its display (a tiling command: its
    /// tile's own part).
    pub fn tile_window(&mut self, window: &WindowElement, tile: Tile) {
        let Some(area) = self.window_output(window).and_then(|o| self.tile_area(&o)) else { return };
        let frame = self.given_way(window, part(area, tile.fraction(), self.tile_gap()), area);
        self.set_frame(window, frame, true);
        Self::set_tiled(window, Some(tile));
    }

    /// `window` was arranged into `cell` of `area`: tiled there, so
    /// dragging an edge resizes its neighbours, and re-tiled into the
    /// same part when its display changes.
    fn set_cell(&mut self, window: &WindowElement, area: Rectangle<i32, Logical>, cell: Rectangle<i32, Logical>) {
        Self::set_tiled(window, Some(Tile::Cell));
        Self::set_part(window, Some(fraction_of(area, cell, self.tile_gap())));
    }

    /// Put a tiled window back in its part of its display: its tile's, or
    /// the one its edges were dragged to.
    pub fn retile(&mut self, window: &WindowElement, tile: Tile) {
        let Some(custom) = Self::custom_part(window) else { return self.tile_window(window, tile) };
        let Some(area) = self.window_output(window).and_then(|o| self.tile_area(&o)) else { return };
        let frame = self.given_way(window, part(area, custom, self.tile_gap()), area);
        self.set_frame(window, frame, true);
    }

    /// Where a tiled window belongs now (what `retile` would make it): its
    /// part of the tiling area, given way to what it and its group can be
    /// at the least.
    pub(super) fn tile_frame(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        let tile = Self::tiled(window)?;
        let area = self.window_output(window).and_then(|o| self.tile_area(&o))?;
        let fraction = Self::custom_part(window).unwrap_or_else(|| tile.fraction());
        Some(self.given_way(window, part(area, fraction, self.tile_gap()), area))
    }

    /// An edge drag of `window` (`sides`) is starting or going on: the
    /// first time, find its tiled neighbours. A tiled window with none is
    /// being sized away from its tile by hand: no longer tiled.
    pub fn tile_drag_begin(&mut self, window: &WindowElement, sides: &[Side]) {
        if self.tile_drag.as_ref().is_some_and(|d| d.window == *window) {
            return;
        }
        // Dragged, it goes where the drag puts it: not still after a
        // target from before (a drag let go a moment ago makes the window
        // dragged a follower, `tile_drag_end`), which would pull each frame
        // it draws back there, against the drag.
        self.unfollow(window);
        let mut links = Vec::new();
        let start = self.space.element_geometry(window).unwrap_or_default();
        if Self::tiled(window).is_some()
            && !self.is_zoomed(window)
            && let (Some(output), Some(frame)) = (self.window_output(window), self.space.element_geometry(window))
        {
            let others: Vec<_> = self
                .windows_on(&output)
                .into_iter()
                .filter(|w| w != window && Self::tiled(w).is_some() && !self.is_zoomed(w))
                .filter_map(|w| Some((w.clone(), self.space.element_geometry(&w)?)))
                .collect();
            links = links_for(frame, sides, &others, self.tile_gap());
            for l in &mut links {
                l.min = self.min_frame(&l.window);
            }
        }
        // (An auto-tiled window stays a tile: its edges at the side of the
        // display just don't move.)
        if links.is_empty() && !self.auto_tiled(window) {
            self.forget_tile(window);
        }
        let min = self.min_frame(window);
        let mins = match self.window_output(window).and_then(|o| self.tile_area(&o)) {
            Some(area) if self.auto_tiled(window) && !links.is_empty() => self.auto_mins(&self.all_windows(), area),
            _ => HashMap::new(),
        };
        self.tile_drag = Some(TileDrag { window: window.clone(), start, sides: sides.to_vec(), links, min, mins, applied: (0, 0), wanted: (0, 0) });
    }

    /// The smallest frame a window takes: its app's minimum content size
    /// (if it gives one) with the tab and border, and never under
    /// `MIN_TILE`. In a group, the group's: they share one frame, as big
    /// as the biggest least size among them (windows of different apps
    /// put together by hand).
    pub(super) fn min_frame(&self, window: &WindowElement) -> Size<i32, Logical> {
        let least = self.least_frame(window);
        Size::from((least.w.max(MIN_TILE), least.h.max(MIN_TILE)))
    }

    /// The smallest frame `window`'s app draws it at (its group's), with
    /// no floor of Olo's.
    fn least_frame(&self, window: &WindowElement) -> Size<i32, Logical> {
        use smithay::wayland::{compositor::with_states, shell::xdg::SurfaceCachedState};
        let least = |w: &WindowElement| w.wl_surface().map(|s| with_states(&s, |st| st.cached_state.get::<SurfaceCachedState>().current().min_size)).unwrap_or_default();
        // (Its group's as it is about to be: windows coming back from
        // another workspace together are placed before the groups are
        // made again, and each placed by its own least size would be put
        // back by the group's a frame later.)
        let content = self.mates_to_be(window).iter().map(least).fold(least(window), |a, b| Size::from((a.w.max(b.w), a.h.max(b.h))));
        // Its frame round the content: what Olo's decoration adds, not
        // its frame less its content now (before it first draws, that is
        // something else, and the tile beside it would move as it does).
        let extra = if window.decorated() || window.will_be_decorated() { super::deco::extra_size() } else { Size::default() };
        if content.w <= 0 && content.h <= 0 {
            return Size::default();
        }
        Size::from((content.w + extra.w, content.h + extra.h))
    }

    /// `frame`, a part of `area`, for `window`: no smaller than its app
    /// draws it, and given way inwards to stay in `area` as
    /// `keep_in_tile_area` would push it once drawn so. A tile then asks
    /// for where the window ends up, rather than passing through its part
    /// on the way (a flicker, each time it is put back: from another
    /// workspace, from being hidden).
    fn given_way(&self, window: &WindowElement, frame: Rectangle<i32, Logical>, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        give_way(frame, self.least_frame(window), area)
    }

    /// The size change the drag may make: neighbours keep their least
    /// sizes, and so does the window dragged (its group's); auto-tiled,
    /// only as far as its tree lays the tiles out where the drag has them
    /// (`auto_keeps`: what it shows is what stays as it's let go, not an
    /// edge held off by a least size the drag didn't count, the tiles
    /// jumping there).
    pub fn tile_drag_limit(&self, d: (i32, i32)) -> (i32, i32) {
        let Some(t) = self.tile_drag.as_ref() else { return d };
        let (mut dw, mut dh) = limit(&t.links, d);
        if !t.links.is_empty() {
            dw = dw.max((t.min.w - t.start.size.w).min(0));
            dh = dh.max((t.min.h - t.start.size.h).min(0));
        }
        // An auto-tiled window's sides at the edge of the display stay.
        if self.auto_tiled(&t.window) {
            let moves = |s: &Side| self.auto_side_moves(&t.window, *s);
            if !t.sides.iter().filter(|s| matches!(s, Side::Left | Side::Right)).any(moves) {
                dw = 0;
            }
            if !t.sides.iter().filter(|s| matches!(s, Side::Top | Side::Bottom)).any(moves) {
                dh = 0;
            }
            if !t.links.is_empty() {
                let keeps = |d: (i32, i32)| self.auto_keeps(&t.window, &drag_frames(t, d), &t.mins);
                // (Where it began isn't as its tree lays it out: something
                // else moved it, and it's put back as it's let go whatever
                // the drag does.)
                if !keeps((dw, dh)) && keeps((0, 0)) {
                    dw = furthest(dw, |x| keeps((x, 0)));
                    dh = furthest(dh, |y| keeps((dw, y)));
                }
            }
        }
        (dw, dh)
    }

    /// The dragged window's size changed by `d` from where it began: its
    /// neighbours' sides move with it, in the next round (`rounds`): true
    /// if it has neighbours that follow (its own size then goes with
    /// theirs, `tile_drag_next`).
    pub fn tile_drag_want(&mut self, d: (i32, i32)) -> bool {
        let Some(t) = self.tile_drag.as_mut() else { return false };
        t.wanted = d;
        !t.links.is_empty()
    }

    /// A drag whose neighbours follow, its steps shown in rounds.
    pub fn drag_in_rounds(&self) -> bool {
        self.tile_drag.as_ref().is_some_and(|t| !t.links.is_empty())
    }

    /// The next round of a drag, unless one is on its way: the dragged
    /// window's size (its pending state, the grab's) and its neighbours',
    /// for where the drag has got to, asked at once.
    pub fn tile_drag_next(&mut self) {
        if self.in_round() {
            return;
        }
        let Some(t) = self.tile_drag.as_mut() else { return };
        if t.links.is_empty() || !t.window.alive() {
            return;
        }
        let d = t.wanted;
        let mut windows = vec![t.window.clone()];
        if t.applied != d {
            t.applied = d;
            for (w, start, target) in targets(&t.links, d) {
                self.ask_to_follow(&w, start, target);
                windows.push(w);
            }
        }
        self.round_send(&windows);
    }

    /// Ask a neighbour that began at `start` for the frame `target`: now
    /// if it has answered the last size asked, else when it does. It moves
    /// when it draws it (`tile_follower_drew`).
    pub(super) fn follow(&mut self, window: &WindowElement, start: Rectangle<i32, Logical>, target: Rectangle<i32, Logical>) {
        self.ask_to_follow(window, start, target);
        // (A drag's last round being made: it goes with that.)
        if let Some(collecting) = self.round_collecting.as_mut() {
            collecting.push(window.clone());
            return;
        }
        let WindowSurface::Wayland(t) = window.0.underlying_surface();
        // (One in a drag's round has its size sent with the round.)
        if t.is_initial_configure_sent() && !self.round_has(window) {
            super::grabs::send_when_caught_up(t);
        }
    }

    /// `follow`, the size left to send (a round sends it).
    fn ask_to_follow(&mut self, window: &WindowElement, start: Rectangle<i32, Logical>, target: Rectangle<i32, Logical>) {
        let asked = Instant::now();
        match self.tile_followers.iter_mut().find(|f| f.window == *window) {
            Some(f) => {
                if f.target != target {
                    f.before.insert(0, (f.start, f.target));
                    f.before.truncate(ASKS_KEPT);
                }
                (f.start, f.target, f.asked) = (start, target, asked);
            }
            None => {
                // Its ask before this one: where it is, at the size last
                // asked of it (the window dragged, whose last round may be
                // on its way as the drag is let go and it is laid out
                // again: a frame of that comes where the drag put it).
                let WindowSurface::Wayland(t) = window.0.underlying_surface();
                let pending = t.with_pending_state(|s| s.size);
                let before = match (self.space.element_geometry(window), pending) {
                    (Some(geo), Some(size)) => {
                        // (Outside the surface's lock: `outer_size` takes it too.)
                        let frame = window.outer_size(size);
                        if frame != target.size { vec![(geo, Rectangle::new(geo.loc, frame))] } else { Vec::new() }
                    }
                    _ => Vec::new(),
                };
                self.tile_followers.push(Follower { window: window.clone(), start, target, asked, before });
            }
        }
        let content = window.inner_size(target.size);
        // (Following a drag, it's being resized by hand as much as the
        // window dragged: so told, an app keeps room to grow, as the kit's
        // does, rather than making new buffers at every step, a new
        // swapchain of GPU memory each, and each new buffer checked and
        // freed here too, which a round then waits for.)
        let resizing = self.tile_drag.is_some();
        let WindowSurface::Wayland(t) = window.0.underlying_surface();
        t.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            if resizing {
                state.states.set(xdg_toplevel::State::Resizing);
            } else {
                state.states.unset(xdg_toplevel::State::Resizing);
            }
            state.size = Some(content);
        });
    }

    /// Where `window` began, if it is still on its way to a frame it was
    /// asked for (`follow`).
    pub(super) fn following_from(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        self.tile_followers.iter().find(|f| f.window == *window).map(|f| f.start)
    }

    /// `window` is where it was going (put there by other means): it
    /// follows no more.
    pub(super) fn unfollow(&mut self, window: &WindowElement) {
        self.tile_followers.retain(|f| f.window != *window);
    }

    /// A window committed: if it is following a drag, put it where the
    /// size it drew belongs (the sides that don't move where they were)
    /// and ask for the frame the drag has got to since. Once it has
    /// caught up with a drag that's over, it's done: it has drawn the
    /// last size asked (or one it can't be). Having answered isn't
    /// enough: an app may answer a size and draw it a frame later, and
    /// that frame would land unplaced, its far side out past its tile.
    pub fn tile_follower_drew(&mut self, window: &WindowElement) {
        let Some(i) = self.tile_followers.iter().position(|f| f.window == *window) else { return };
        let (start, target, asked) = (self.tile_followers[i].start, self.tile_followers[i].target, self.tile_followers[i].asked);
        let (Some(loc), Some(geo)) = (self.space.element_location(window), self.space.element_geometry(window)) else {
            self.tile_followers.remove(i);
            return;
        };
        // Drawn for an ask before the newest: placed by that one (its size
        // and edges together, as the round it came in showed them).
        let (from, to) = match geo.size == target.size {
            true => (start, target),
            false => self.tile_followers[i].before.iter().find(|(_, t)| t.size == geo.size).copied().unwrap_or((start, target)),
        };
        // Held by the side that moved less (the one not dragged): a side
        // a pixel off by rounding as the other went 125 px had a frame of
        // the size before hung from the dragged side, 125 px out over its
        // neighbour (auto-fuzz seed 37).
        let moved = |a0: i32, a1: i32, b0: i32, b1: i32| ((b0 - a0).abs(), ((b0 + b1) - (a0 + a1)).abs());
        let (dl, dr) = moved(from.loc.x, from.size.w, to.loc.x, to.size.w);
        let (dt, db) = moved(from.loc.y, from.size.h, to.loc.y, to.size.h);
        let x = if dl > dr { to.loc.x + to.size.w - geo.size.w } else { to.loc.x };
        let y = if dt > db { to.loc.y + to.size.h - geo.size.h } else { to.loc.y };
        let at = Point::from((x, y)) - (geo.loc - loc);
        debug!(target: "olo::rounds", window = window.id(), drew = ?(geo.size.w, geo.size.h), target = ?(target.loc.x, target.loc.y, target.size.w, target.size.h), in_round = self.round_has(window), "follower drew");
        if at != loc {
            self.space.relocate_element(window, at);
        }
        match window.0.underlying_surface() {
            WindowSurface::Wayland(t) => {
                if t.has_pending_changes() {
                    if !self.drag_in_rounds() && !self.round_has(window) {
                        super::grabs::send_when_caught_up(t);
                    }
                } else if self.tile_drag.is_none() && !super::grabs::awaiting_ack(t) && (geo.size == target.size || !can_be(window, target.size) || asked.elapsed() > FOLLOW_LIMIT) {
                    self.tile_followers.remove(i);
                }
            }
        }
    }

    /// The drag ended: the dragged window and its neighbours keep the
    /// parts they now have.
    pub fn tile_drag_end(&mut self) {
        let Some(mut t) = self.tile_drag.take() else { return };
        if t.links.is_empty() {
            return;
        }
        // The dragged window's frame in the round on its way, before the
        // last one changes what's applied.
        let on_its_way = drag_frames(&t, t.applied).pop().map(|(_, r)| r);
        // Where it was let go, the last round (after the one on its way):
        // its size (`end_resize` left it to send) and its neighbours'.
        let mut windows = vec![t.window.clone()];
        if t.applied != t.wanted {
            t.applied = t.wanted;
            for (w, start, target) in targets(&t.links, t.wanted) {
                self.ask_to_follow(&w, start, target);
                windows.push(w);
            }
        }
        // (The neighbours resized no longer, `ask_to_follow`: told in it
        // too, so they fit their buffers to their size again.)
        for l in &t.links {
            let WindowSurface::Wayland(x) = l.window.0.underlying_surface();
            x.with_pending_state(|state| state.states.unset(xdg_toplevel::State::Resizing));
            if !windows.contains(&l.window) {
                windows.push(l.window.clone());
            }
        }
        let window = t.window.clone();
        let gap = self.tile_gap();
        if let Some(area) = self.window_output(&t.window).and_then(|o| self.tile_area(&o)) {
            self.tile_followers.retain(|f| f.window.alive());
            // The frames the drag asked for (they may not have drawn them yet).
            for (w, r) in drag_frames(&t, t.applied) {
                Self::set_part(&w, Some(fraction_of(area, r, gap)));
            }
            // The dragged window follows now too, from where the drag put
            // it: laid out again below, a frame it draws for a round on
            // its way still goes there (`Follower::before`), whichever
            // edge was dragged.
            if let Some(last) = drag_frames(&t, t.applied).pop().map(|(_, r)| r)
                && !self.tile_followers.iter().any(|f| f.window == window)
            {
                let before = on_its_way.filter(|r| *r != last).map(|r| vec![(t.start, r)]).unwrap_or_default();
                self.tile_followers.push(Follower { window: window.clone(), start: t.start, target: last, asked: Instant::now(), before });
            }
            // Auto-tiled: the splits move just there, and what that lays out
            // otherwise than the drag left it (a least size holding an edge
            // off where it went) goes in the same round.
            self.round_collecting = Some(Vec::new());
            self.auto_drag_ended(&window);
            for w in self.round_collecting.take().unwrap_or_default() {
                if !windows.contains(&w) {
                    windows.push(w);
                }
            }
        }
        self.round_last(windows);
    }

    /// In a tile or zoomed (Restore would change it).
    pub fn arranged(&self, window: &WindowElement) -> bool {
        Self::tiled(window).is_some() || self.is_zoomed(window) || self.auto_tiled(window)
    }

    /// Back to how it was before it was tiled or zoomed.
    pub fn restore_frame(&mut self, window: &WindowElement) {
        self.forget_tile(window);
        self.unzoom_window(window);
    }

    /// It was moved or resized by hand (or Restored): no longer in a tile,
    /// so unzooming it or moving it to another display won't put it back.
    pub fn forget_tile(&mut self, window: &WindowElement) {
        Self::set_tiled(window, None);
        if self.tile_drag.as_ref().is_some_and(|d| d.window == *window) {
            self.tile_drag = None;
        }
        if let Some(z) = window.user_data().get::<ZoomedFrom>() {
            z.0.borrow_mut().take();
        }
    }

    /// Put somewhere on purpose at a size of its own (`Request::Place`):
    /// that is its own frame now, so Restore leaves it there rather than
    /// taking it back to the one it had before it was last tiled or
    /// zoomed.
    pub fn forget_own_frame(&mut self, window: &WindowElement) {
        if let Some(p) = window.user_data().get::<PreZoom>() {
            p.0.borrow_mut().take();
        }
    }

    /// A window's own size (not tiled or zoomed), as it is now.
    fn own_frame(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        let pre = window.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow());
        pre.or_else(|| self.space.element_geometry(window))
    }

    pub fn center_window(&mut self, window: &WindowElement) {
        let (Some(area), Some(frame)) = (self.window_output(window).and_then(|o| self.tile_area(&o)), self.own_frame(window)) else {
            return;
        };
        let size = Size::from((frame.size.w.min(area.size.w), frame.size.h.min(area.size.h)));
        let loc = area.loc + Point::from(((area.size.w - size.w) / 2, (area.size.h - size.h) / 2));
        // Centering is the window's own place now.
        Self::set_tiled(window, None);
        if let Some(p) = window.user_data().get::<PreZoom>() {
            p.0.borrow_mut().take();
        }
        self.set_frame(window, Rectangle::new(loc, size), false);
    }

    /// The displays left to right (then top to bottom).
    fn outputs_in_order(&self) -> Vec<Output> {
        let mut outputs: Vec<Output> = self.space.outputs().cloned().collect();
        outputs.sort_by_key(|o| self.space.output_geometry(o).map(|g| (g.loc.x, g.loc.y)).unwrap_or_default());
        outputs
    }

    /// Move a window to the next (or previous) display, to the same place
    /// on it: tiled or zoomed as it was, else at the same relative spot.
    pub fn window_to_next_output(&mut self, window: &WindowElement, back: bool) {
        let outputs = self.outputs_in_order();
        let Some(from) = self.window_output(window) else { return };
        let Some(i) = outputs.iter().position(|o| *o == from) else { return };
        if outputs.len() < 2 {
            return;
        }
        let to = outputs[if back { (i + outputs.len() - 1) % outputs.len() } else { (i + 1) % outputs.len() }].clone();
        self.move_to_output(window, &from, &to, false);
        self.focus_window(window);
    }

    /// Put a window on `to`, where it was on `from` (a display that may be
    /// gone: then just a free cascade spot). It counts as arriving there:
    /// if it joins a group by itself, its own frame is kept for when it
    /// is alone again, and going back is still the round trip. `home`:
    /// back to the display it was taken from without the person moving it
    /// (it was unplugged, and is back): exactly as it left, whatever Olo
    /// did with it meanwhile.
    pub(crate) fn move_to_output(&mut self, window: &WindowElement, from: &Output, to: &Output, home: bool) {
        self.carry_to_output(window, from, to, home);
        window.meta().arrived.set(self.space.element_geometry(window));
    }

    fn carry_to_output(&mut self, window: &WindowElement, from: &Output, to: &Output, home: bool) {
        let (Some(dest), Some(zoom_dest)) = (self.usable_area(to), self.tile_area(to)) else { return };
        let zoomed = self.is_zoomed(window);
        let along = Self::zoomed_along(window);
        let src = self.usable_area(from);
        let trip = window.user_data().get::<Travelled>().and_then(|t| t.0.borrow_mut().take());
        let left_arrangement = Self::arrangement_of(window);
        if let (Some(here), Some(src)) = (self.carried(window), src) {
            // Each frame at the same relative place on the new display.
            let rel = |v: i32, origin: i32, len: i32, dlen: i32, size: i32| {
                let f = (v - origin) as f32 / (len - size).max(1) as f32;
                (f.clamp(0.0, 1.0) * (dlen - size).max(0) as f32).round() as i32
            };
            let place = |r: Rectangle<i32, Logical>| {
                let size = Size::from((r.size.w.min(dest.size.w), r.size.h.min(dest.size.h)));
                let loc = dest.loc + Point::from((rel(r.loc.x, src.loc.x, src.size.w, dest.size.w, size.w), rel(r.loc.y, src.loc.y, src.size.h, dest.size.h, size.h)));
                Rectangle::new(loc, size)
            };
            // Back where it came from, as it arrived (or in the group it
            // joined by itself there, untouched since).
            let geometry = self.space.element_geometry(window);
            let arranged = Self::arrangement_of(window);
            let joined_only = |t: &Trip| t.joined.is_some_and(|g| window.meta().joined.get().is_some_and(|j| j.group == g && j.untouched(geometry, arranged)));
            let back = trip.filter(|t| t.home == to.name() && t.home_area == dest && (home || t.arrived == here || joined_only(t)));
            if let Some(t) = &back {
                // Its own frame and arrangement are the ones it goes back
                // to now.
                window.meta().joined.set(None);
                Self::rearrange(window, t.left_arrangement);
            }
            let there = match &back {
                Some(t) => t.left,
                None => {
                    // What unzooming it puts back goes along too.
                    let along_from = here.along_from.map(place);
                    let zoomed_from = here.zoomed_from.map(|r| match (along, along_from) {
                        (Some(z), Some(f)) => zoom_along(f, zoom_dest, z.axis),
                        _ => place(r),
                    });
                    // Zoomed along an axis: placed, then along it there.
                    let frame = here.frame.map(place).map(|f| match along {
                        Some(z) if !zoomed => zoom_along(f, zoom_dest, z.axis),
                        _ => f,
                    });
                    Carried { frame, pre: here.pre.map(place), along_from, zoomed_from }
                }
            };
            info!(window = window.id(), from = from.name(), to = to.name(), back = back.is_some(), ?here, ?there, zoomed, ?along, "to another display");
            window.user_data().insert_if_missing(Travelled::default);
            *window.user_data().get::<Travelled>().unwrap().0.borrow_mut() = Some(Trip { home: from.name(), home_area: src, left: here, left_arrangement, arrived: there, joined: None });
            if let Some(p) = window.user_data().get::<PreZoom>() {
                *p.0.borrow_mut() = there.pre;
            }
            if let (Some(z), Some(from), None) = (along, there.along_from, &back) {
                Self::set_zoomed_along(window, Some(ZoomedAlong { from, ..z }));
            }
            if let Some(f) = window.user_data().get::<ZoomedFrom>() {
                *f.0.borrow_mut() = there.zoomed_from;
            }
            match there.frame {
                Some(frame) => {
                    self.set_frame(window, frame, false);
                    if back.is_some() || along.is_some() {
                        // As it was, on a display as it was, or zoomed
                        // along its axis there already: nothing to fit.
                        return;
                    }
                }
                // Tiled or zoomed: somewhere on the new display first, so
                // it is "on" it, for `refit` to put it in its place there.
                None => self.space.relocate_element(window, there.along_from.map_or(dest.loc, |f| f.loc)),
            }
        } else {
            let slot = self.free_cascade_slot(to, window);
            self.space.relocate_element(window, slot);
        }
        self.refit(window);
    }

    /// What goes along with a window to another display (`Carried`).
    fn carried(&self, window: &WindowElement) -> Option<Carried> {
        let frame = self.space.element_geometry(window)?;
        let along = Self::zoomed_along(window);
        let follows = self.is_zoomed(window) || Self::tiled(window).is_some();
        Some(Carried {
            frame: (!follows).then_some(frame),
            pre: window.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow()),
            along_from: along.map(|z| z.from),
            zoomed_from: window.user_data().get::<ZoomedFrom>().and_then(|f| *f.0.borrow()),
        })
    }

    /// How far the display `frame` was on, among `then` (by name), has
    /// moved since: what was remembered there moves as much.
    pub fn display_moved(&self, frame: Rectangle<i32, Logical>, then: &[(String, Rectangle<i32, Logical>)]) -> Point<i32, Logical> {
        let middle = frame.loc + frame.size.downscale(2).to_point();
        then.iter()
            .find(|(_, r)| r.contains(middle))
            .and_then(|(name, was)| {
                let now = self.space.outputs().find(|o| o.name() == *name)?;
                Some(self.space.output_geometry(now)?.loc - was.loc)
            })
            .unwrap_or_default()
    }

    /// `frame` as it is if it is in reach on any display; else brought
    /// into reach on the one it is (mostly) on, or the nearest
    /// (`into_reach`): where a window put somewhere by number goes.
    pub fn reachable(&self, frame: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        let areas: Vec<(Rectangle<i32, Logical>, Rectangle<i32, Logical>)> = self.space.outputs().filter_map(|o| Some((self.space.output_geometry(o)?, self.usable_area(o)?))).collect();
        if areas.iter().any(|(_, area)| in_reach(frame, *area)) {
            return frame;
        }
        let overlap = |g: &Rectangle<i32, Logical>| frame.intersection(*g).map_or(0, |r| r.size.w as i64 * r.size.h as i64);
        let far = |g: &Rectangle<i32, Logical>| {
            let c = frame.loc + frame.size.downscale(2).to_point();
            let dx = (g.loc.x - c.x).max(c.x - (g.loc.x + g.size.w)).max(0) as i64;
            let dy = (g.loc.y - c.y).max(c.y - (g.loc.y + g.size.h)).max(0) as i64;
            dx * dx + dy * dy
        };
        let best = areas.iter().filter(|(g, _)| overlap(g) > 0).max_by_key(|(g, _)| overlap(g)).or_else(|| areas.iter().min_by_key(|(g, _)| far(g)));
        best.map_or(frame, |(_, area)| into_reach(frame, *area))
    }

    /// The first diagonal slot on a display no other window sits on.
    fn free_cascade_slot(&self, output: &Output, window: &WindowElement) -> Point<i32, Logical> {
        let area = self.usable_area(output).unwrap_or_default();
        let origin = area.loc + Point::from((48, 40));
        let taken: Vec<Point<i32, Logical>> = self.space.elements().filter(|w| *w != window).filter_map(|w| self.space.element_location(w)).collect();
        let slots = ((area.size.h / 3) / CASCADE).max(1);
        let n = (0..slots).find(|i| !taken.contains(&(origin + Point::from((i * CASCADE, i * CASCADE))))).unwrap_or(0);
        origin + Point::from((n * CASCADE, n * CASCADE))
    }

    /// `window`, its frame `frame` as drawn, drew another size than it was
    /// asked, one it can be.
    fn drawn_otherwise(window: &WindowElement, frame: Size<i32, Logical>) -> bool {
        use smithay::wayland::{compositor::with_states, shell::xdg::SurfaceCachedState};
        let WindowSurface::Wayland(t) = window.0.underlying_surface();
        let Some(asked) = t.with_pending_state(|s| s.size) else { return false };
        let least = with_states(t.wl_surface(), |st| st.cached_state.get::<SurfaceCachedState>().current().min_size);
        // (Outside the surface's lock: `inner_size` takes it too.)
        let content = window.inner_size(frame);
        content != asked && asked.w >= least.w && asked.h >= least.h
    }

    /// A tiled or zoomed window its app keeps bigger than its tile (it
    /// won't be smaller than its least size: a third of a narrow display
    /// is less than StyledEdit's) gives way inwards, so it stays in the
    /// tiling area rather than hanging out past its edge.
    pub fn keep_in_tile_area(&mut self, window: &WindowElement) {
        // (Zoomed along one axis, it keeps its own place along the other.)
        let held = Self::tiled(window).is_some() || self.is_zoomed(window);
        if !held || self.tile_drag.as_ref().is_some_and(|t| t.window == *window) {
            return;
        }
        // Still drawing the size it had (an arrangement has just asked
        // for its tile's): not kept bigger, only not there yet. Pushed
        // in now, it would stay where that size fitted.
        if super::groups::size_on_the_way(window).is_some() {
            return;
        }
        let (Some(loc), Some(geo)) = (self.space.element_location(window), self.space.element_geometry(window)) else { return };
        let Some(area) = self.window_output(window).and_then(|o| self.tile_area(&o)) else { return };
        let inside = |pos: i32, len: i32, lo: i32, span: i32| if len <= span { pos.clamp(lo, lo + span - len) } else { lo };
        let x = inside(geo.loc.x, geo.size.w, area.loc.x, area.size.w);
        let y = inside(geo.loc.y, geo.size.h, area.loc.y, area.size.h);
        // Nor when what it drew isn't the size it acked, and could be (its
        // least size fits): a frame drawn before, the size asked to come
        // (the kit's apps answer a new window's first configure with a
        // frame at their own size, then their tile's). Pushed in by that
        // frame, a new Terminal stayed over its neighbour, at its tile's
        // size but where its first frame had fitted (auto-fuzz, seed 7).
        // (Asked only of one that would move: nothing more on every
        // commit.)
        if (x, y) != (geo.loc.x, geo.loc.y) && !Self::drawn_otherwise(window, geo.size) {
            self.space.relocate_element(window, loc + Point::from((x - geo.loc.x, y - geo.loc.y)));
        }
    }

    /// Neither in a tile nor zoomed (wholly or along an axis): a window of
    /// its own size, where it was put.
    pub(crate) fn own_size(&self, window: &WindowElement) -> bool {
        Self::tiled(window).is_none() && !self.is_zoomed(window) && Self::zoomed_along(window).is_none()
    }

    /// Fit a window to the display it is on now: re-tile or re-zoom it
    /// there (along its axis, if it is zoomed along one), else shrink it
    /// to fit and bring its tab into view. A window that fits and can be
    /// reached stays exactly where it is, even hanging off an edge, as
    /// people leave windows: this runs whenever a window comes back
    /// (from another workspace, from being hidden) and when displays
    /// change, and nothing should move then for no reason.
    pub fn refit(&mut self, window: &WindowElement) {
        // (Zoomed first: a tiled window can be zoomed, and stays so.)
        if self.is_zoomed(window) {
            if let Some(area) = self.zoom_area(window)
                && self.space.element_geometry(window) != Some(area)
            {
                self.zoom_window(window);
            }
            return;
        }
        if let Some(tile) = Self::tiled(window) {
            self.retile(window, tile);
            return;
        }
        if let Some(z) = Self::zoomed_along(window) {
            let (Some(area), Some(frame)) = (self.zoom_area(window), self.space.element_geometry(window)) else { return };
            let fitted = zoom_along(frame, area, z.axis);
            if fitted != frame {
                self.set_frame(window, fitted, false);
            }
            return;
        }
        // A window of its own size only has to be on the display (the
        // margins are for arranging).
        let area = self.window_output(window).and_then(|o| self.usable_area(&o));
        let (Some(area), Some(frame)) = (area, self.space.element_geometry(window)) else { return };
        let size = Size::from((frame.size.w.min(area.size.w), frame.size.h.min(area.size.h)));
        if size == frame.size && in_reach(frame, area) {
            return;
        }
        let loc = Point::from((frame.loc.x.clamp(area.loc.x, area.loc.x + area.size.w - size.w), frame.loc.y.clamp(area.loc.y, area.loc.y + area.size.h - size.h)));
        if size != frame.size {
            self.set_frame(window, Rectangle::new(loc, size), false);
        } else if loc != frame.loc {
            self.space.relocate_element(window, loc);
        }
    }

    /// Windows on a display on this workspace, back to front; a group
    /// counts as its front window (the rest follow it).
    pub(super) fn windows_on(&self, output: &Output) -> Vec<WindowElement> {
        let all: Vec<WindowElement> = self.space.elements().cloned().collect();
        all.into_iter().filter(|w| self.window_output(w).as_ref() == Some(output) && !self.behind_in_group(w)).collect()
    }

    fn placed(&self, windows: &[WindowElement]) -> Vec<Placed> {
        windows
            .iter()
            .filter_map(|w| {
                Some(Placed {
                    window: w.clone(),
                    frame: self.space.element_geometry(w)?,
                    tile: Self::tiled(w),
                    part: Self::custom_part(w),
                    pre: w.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow()),
                    zoomed: self.is_zoomed(w),
                    along: Self::zoomed_along(w),
                    zoomed_from: w.user_data().get::<ZoomedFrom>().and_then(|f| *f.0.borrow()),
                    tab_offset: w.decoration().tab_offset,
                })
            })
            .collect()
    }

    /// Keep how `windows` are now, for Undo Window Arrangement, the step
    /// called `name` (Undo Cascade).
    pub(super) fn remember_arrangement(&mut self, windows: &[WindowElement], name: &str) {
        self.auto_forget_undo();
        self.arranged = self.placed(windows);
        self.arranged_among = self.display_frames();
        self.arrangement_step(name, ArrangedKind::Placed);
    }

    /// The last arrangement is now `name` (of `kind`), not undone: said to
    /// those who show it (Deskbar's menu).
    pub fn arrangement_step(&mut self, name: &str, kind: ArrangedKind) {
        self.arrangement = Some(Arranged { name: name.to_owned(), kind, undone: false });
        self.tell_arrangement();
    }

    /// What Undo and Redo Window Arrangement would do, by name.
    pub fn arrangement_names(&self) -> (Option<String>, Option<String>) {
        match &self.arrangement {
            Some(a) if a.undone => (None, Some(a.name.clone())),
            Some(a) => (Some(a.name.clone()), None),
            None => (None, None),
        }
    }

    fn tell_arrangement(&mut self) {
        let (undo, redo) = self.arrangement_names();
        self.ipc.broadcast(olo_protocol::Event::ArrangementUndoChanged { undo, redo });
    }

    /// `windows` about to move to another workspace or display: where each
    /// is now kept as the step `name` (Undo Move to Workspace 2).
    pub fn remember_moves(&mut self, windows: &[WindowElement], name: &str) {
        let was = windows.iter().map(|w| (w.clone(), self.display_of(w).map(|o| o.name()), self.workspace_of(w))).collect();
        self.auto_forget_undo();
        self.arrangement_step(name, ArrangedKind::Moved(was));
    }

    /// Undo Window Arrangement (⌃⌘Z): the last arrangement taken back
    /// (nothing if it's undone already: Redo puts it again).
    pub fn undo_window_arrangement(&mut self) {
        if self.arrangement.as_ref().is_some_and(|a| !a.undone) {
            self.arrangement_back(true);
        }
    }

    /// Redo Window Arrangement (⇧⌃⌘Z): the arrangement undone put again.
    pub fn redo_window_arrangement(&mut self) {
        if self.arrangement.as_ref().is_some_and(|a| a.undone) {
            self.arrangement_back(false);
        }
    }

    /// The last arrangement swapped with how things are (each kind keeps
    /// what it takes back, and takes back to the other way after), now
    /// `undone` or not.
    fn arrangement_back(&mut self, undone: bool) {
        let Some(mut a) = self.arrangement.take() else { return };
        match std::mem::replace(&mut a.kind, ArrangedKind::Placed) {
            ArrangedKind::Placed => {
                self.undo_arrangement();
                a.kind = ArrangedKind::Placed;
            }
            ArrangedKind::Auto => {
                let turned_on = self.auto_turned_on_undo();
                if !self.auto_undo() {
                    return;
                }
                a.kind = match turned_on {
                    Some((ws, outputs)) => ArrangedKind::AutoOn(ws, outputs),
                    None => ArrangedKind::Auto,
                };
            }
            // Redo of turning auto-tiling on: turning it on again (which is
            // a step of its own, as it was).
            ArrangedKind::AutoOn(ws, outputs) => {
                for o in self.space.outputs().filter(|o| outputs.contains(&o.name())).cloned().collect::<Vec<_>>() {
                    self.set_windowing_mode(olo_protocol::WindowingMode::AutoTiling, Some(ws), Some(o));
                }
                return;
            }
            ArrangedKind::Moved(was) => {
                let now: Vec<(WindowElement, Option<String>, u32)> = was
                    .iter()
                    .filter(|(w, _, _)| self.space.elements().any(|e| e == w) || self.parked.0.borrow().contains(w))
                    .map(|(w, _, _)| (w.clone(), self.display_of(w).map(|o| o.name()), self.workspace_of(w)))
                    .collect();
                for (w, output, ws) in &was {
                    let to = output.as_ref().and_then(|n| self.space.outputs().find(|o| &o.name() == n).cloned());
                    match to {
                        Some(to) => self.move_to_display_workspace(w, &to, *ws),
                        None => self.move_to_workspace(w, *ws),
                    }
                }
                a.kind = ArrangedKind::Moved(now);
            }
        }
        a.undone = undone;
        info!(name = a.name, undone, "window arrangement taken back");
        self.arrangement = Some(a);
        self.tell_arrangement();
    }

    /// Every window the last arrangement moved back where and how big it
    /// was (those still open); again, as the arrangement had them.
    pub fn undo_arrangement(&mut self) {
        let before = std::mem::take(&mut self.arranged);
        let among = std::mem::take(&mut self.arranged_among);
        let windows: Vec<WindowElement> = before.iter().map(|p| p.window.clone()).filter(|w| self.space.element_location(w).is_some()).collect();
        let now = self.placed(&windows);
        // The displays as they were: each back exactly where it was (out
        // past an edge too, as it was left); else where it can be reached.
        let same_displays = among == self.display_frames();
        for p in &before {
            if !windows.contains(&p.window) {
                continue;
            }
            // Where its display is now, if that moved since.
            let delta = self.display_moved(p.frame, &among);
            let shift = |r: Rectangle<i32, Logical>| Rectangle::new(r.loc + delta, r.size);
            let p = &Placed { frame: shift(p.frame), pre: p.pre.map(shift), zoomed_from: p.zoomed_from.map(shift), along: p.along.map(|z| ZoomedAlong { from: shift(z.from), ..z }), ..p.clone() };
            if p.zoomed {
                self.zoom_window(&p.window);
            } else {
                let frame = if same_displays { p.frame } else { self.put_back_at(p.frame) };
                self.set_frame(&p.window, frame, false);
            }
            Self::set_tiled(&p.window, p.tile);
            Self::set_part(&p.window, p.tile.and(p.part));
            Self::set_zoomed_along(&p.window, p.along);
            p.window.user_data().insert_if_missing(PreZoom::default);
            p.window.user_data().get::<PreZoom>().unwrap().0.replace(p.pre);
            p.window.user_data().insert_if_missing(ZoomedFrom::default);
            p.window.user_data().get::<ZoomedFrom>().unwrap().0.replace(p.zoomed_from);
            p.window.decoration().tab_offset = p.tab_offset;
        }
        info!(windows = windows.len(), "undid an arrangement");
        self.arranged = now;
        self.arranged_among = self.display_frames();
    }

    /// Every window on the active display at the top left of it, stacked
    /// with their tabs side by side along the top (as BeOS people slid
    /// them by hand), in the order their tabs are now from the left. Each
    /// starts where the tab before it ends and is narrowed so it stops
    /// beside Deskbar, but never below a quarter of the display (so four
    /// always fit, narrower ones more); past that the rest go as far
    /// right as they fit, their tabs slid along them to keep stepping
    /// right, and overlap only when there's no room left for them.
    /// `keep`: all but that window, left where it is.
    pub fn fan_out(&mut self, keep: Option<&WindowElement>) {
        let Some(output) = self.arranged_output(keep) else { return };
        let (Some(area), Some(screen)) = (self.tile_area(&output), self.space.output_geometry(&output)) else { return };
        let (windows, kept) = self.windows_to_arrange(&output, keep);
        // (Remembered even if there is nothing to arrange: Undo then has
        // nothing to undo, rather than redoing the arrangement before.)
        self.remember_arrangement(&windows, "Fan Out");
        if windows.is_empty() {
            return;
        }
        let tab_x = |s: &Self, w: &WindowElement| {
            let at = s.space.element_location(w).unwrap_or_default();
            let (title, size) = (w.title(), w.geometry().size);
            at.x + w.decoration().tab_layout(&title, size).x
        };
        let mut order: Vec<(i32, usize, WindowElement)> = windows.iter().enumerate().map(|(z, w)| (tab_x(self, w), windows.len() - z, w.clone())).collect();
        order.sort_by_key(|(x, z, _)| (*x, *z));
        let narrowest = screen.size.w / 4;
        let right = area.loc.x + area.size.w;
        let mut x = area.loc.x;
        for (_, _, w) in &order {
            let own = self.own_frame(w).unwrap_or_default().size;
            // Its tab as wide as its title wants (as on a wide window).
            let tab = {
                let title = w.title();
                let mut d = w.decoration();
                d.tab_offset = 0;
                d.tab_layout(&title, Size::from((area.size.w, 1))).width
            };
            // No narrower than it can be (or it would grow past Deskbar).
            let min = self.min_frame(w);
            let width = own.w.min((right - x).max(narrowest)).min(area.size.w).max(min.w).max(1);
            let at = x.min(right - width).max(area.loc.x);
            let height = own.h.min(area.size.h).max(min.h).max(1);
            // Pushed left of where its tab should go: its tab slides along
            // to there, as far as the window is wide.
            let offset = (x - at).clamp(0, (width - tab).max(0));
            Self::set_tiled(w, None);
            if let Some(p) = w.user_data().get::<PreZoom>() {
                p.0.borrow_mut().take();
            }
            self.set_frame(w, Rectangle::new(Point::from((at, area.loc.y)), Size::from((width, height))), false);
            w.decoration().tab_offset = offset;
            x = at + offset + tab;
        }
        self.bring_back(kept);
        info!(windows = order.len(), "fanned out");
    }

    /// The display an arranging command works on: the kept window's, else
    /// the active one.
    fn arranged_output(&self, keep: Option<&WindowElement>) -> Option<Output> {
        keep.and_then(|w| self.window_output(w)).or_else(|| self.active_output())
    }

    /// The windows on `output` an arranging command moves: all of them,
    /// or all but `keep`, which it hands back if it is there.
    fn windows_to_arrange(&self, output: &Output, keep: Option<&WindowElement>) -> (Vec<WindowElement>, Option<WindowElement>) {
        let mut windows = self.windows_on(output);
        let kept = keep.filter(|k| windows.contains(k)).cloned();
        windows.retain(|w| Some(w) != kept.as_ref());
        (windows, kept)
    }

    /// The window an arrangement left alone, in front and focused again.
    fn bring_back(&mut self, kept: Option<WindowElement>) {
        if let Some(w) = kept {
            self.space.raise_element(&w, false);
            self.focus_window(&w);
        }
    }

    /// Every window on the active display in a grid, in reading order of
    /// where they are now; the rolled-up ones fanned out in one cell.
    pub fn tile_all(&mut self) {
        let Some(output) = self.active_output() else { return };
        let Some(area) = self.tile_area(&output) else { return };
        let mut windows = self.windows_on(&output);
        self.remember_arrangement(&windows, "Tile All");
        windows.sort_by_key(|w| {
            let g = self.space.element_geometry(w).unwrap_or_default();
            (g.loc.y + g.size.h / 2, g.loc.x + g.size.w / 2)
        });
        self.tile_grid(area, &windows);
        info!(windows = windows.len(), "tiled all");
    }

    /// `windows` (in reading order) tiled in a grid over `area`, a cell
    /// each, except the rolled-up ones: those share the last cell, fanned
    /// out in it (`fan_in`), rather than each leaving a cell nearly empty.
    fn tile_grid(&mut self, area: Rectangle<i32, Logical>, windows: &[WindowElement]) {
        let (rolled, open): (Vec<WindowElement>, Vec<WindowElement>) = windows.iter().cloned().partition(|w| w.rolled());
        let mut cells = grid(area, open.len() + usize::from(!rolled.is_empty()), self.tile_gap());
        let shared = if rolled.is_empty() { None } else { cells.pop() };
        for (w, cell) in open.iter().zip(cells) {
            self.set_frame(w, cell, true);
            self.set_cell(w, area, cell);
        }
        let Some(cell) = shared else { return };
        let tabs: Vec<i32> = rolled
            .iter()
            .map(|w| {
                let (title, content) = (w.title(), w.inner_size(cell.size));
                let mut d = w.decoration();
                d.tab_offset = 0;
                d.tab_layout(&title, content).width
            })
            .collect();
        for (w, (frame, offset)) in rolled.iter().zip(fan_in(cell, &tabs, olo_theme::deco::TAB_HEIGHT)) {
            let min = self.min_frame(w);
            let size = Size::from((frame.size.w.max(min.w), frame.size.h.max(min.h)));
            // Wider than the cell (an app's least width): as far left as
            // it must go to stay in the area, its tab kept where the fan
            // put it (moved with its frame, it lay over the others' tabs).
            let x = frame.loc.x.min(area.loc.x + area.size.w - size.w).max(area.loc.x);
            let frame = Rectangle::new(Point::from((x, frame.loc.y)), size);
            self.set_frame(w, frame, true);
            self.set_cell(w, area, frame);
            w.decoration().tab_offset = offset + cell.loc.x - x;
        }
    }

    /// Every resizable window on the active display zoomed; if they all
    /// are, every one unzoomed (back into its tile or cell, if it had
    /// one, as Zoom does).
    pub fn zoom_all(&mut self) {
        let Some(output) = self.active_output() else { return };
        let windows: Vec<WindowElement> = self.windows_on(&output).into_iter().filter(resizable).collect();
        self.remember_arrangement(&windows, "Zoom All");
        if windows.is_empty() {
            return;
        }
        let unzoom = windows.iter().all(|w| self.is_zoomed(w));
        for w in &windows {
            match (unzoom, self.is_zoomed(w)) {
                (true, _) => self.unzoom_window(w),
                (false, false) => self.zoom_window(w),
                _ => {}
            }
        }
        info!(windows = windows.len(), unzoom, "zoomed all");
    }

    /// The active display, the area tiling uses on it, and its windows,
    /// front first: what an arranger (Smart Tiling) works with.
    pub fn arrangement(&self) -> Option<(Output, Rectangle<i32, Logical>, Vec<WindowElement>)> {
        let output = self.active_output()?;
        let area = self.tile_area(&output)?;
        let mut windows = self.windows_on(&output);
        windows.reverse();
        Some((output, area, windows))
    }

    /// Windows of the active display put in parts of its tiling area (an
    /// arranger's answer), as Tile All puts them in cells: undoable, and
    /// tiled so their edges move together. How many it placed.
    pub fn arrange(&mut self, placements: &[olo_protocol::Placement]) -> usize {
        let Some((_, area, windows)) = self.arrangement() else { return 0 };
        let placed: Vec<(WindowElement, (f32, f32, f32, f32))> = placements
            .iter()
            .filter_map(|p| {
                let w = windows.iter().find(|w| w.meta().id == p.id)?;
                let clamp = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
                let (x, y) = (clamp(p.x), clamp(p.y));
                let (w2, h) = (clamp(p.w).min(1.0 - x).max(0.05), clamp(p.h).min(1.0 - y).max(0.05));
                Some((w.clone(), (x, y, w2, h)))
            })
            .collect();
        self.remember_arrangement(&windows, "Arrangement");
        if placed.is_empty() {
            return 0;
        }
        for (w, f) in &placed {
            let cell = part(area, *f, self.tile_gap());
            self.set_frame(w, cell, true);
            self.set_cell(w, area, cell);
        }
        info!(windows = placed.len(), "arranged");
        placed.len()
    }

    /// The resizable windows of app `app_id` on this workspace (not its
    /// alerts or other fixed-size panels) in a grid on the active display,
    /// in front, in reading order of where they are now: an app showing
    /// all its documents at once (Terminal's groups). False if it has none.
    pub fn tile_app(&mut self, app_id: &str) -> bool {
        let Some(output) = self.active_output() else { return false };
        let Some(area) = self.tile_area(&output) else { return false };
        let mut windows: Vec<WindowElement> = self.space.elements().filter(|w| w.app_id() == app_id && resizable(w) && !self.behind_in_group(w)).cloned().collect();
        if windows.is_empty() {
            return false;
        }
        self.remember_arrangement(&windows, "Show All");
        windows.sort_by_key(|w| {
            let g = self.space.element_geometry(w).unwrap_or_default();
            (g.loc.y + g.size.h / 2, g.loc.x + g.size.w / 2)
        });
        self.tile_grid(area, &windows);
        for w in &windows {
            self.space.raise_element(w, false);
        }
        if let Some(front) = windows.last() {
            self.focus_window(front);
        }
        info!(app_id, windows = windows.len(), "tiled an app's windows");
        true
    }

    /// Every window on the active display stacked diagonally from the top
    /// left, as they open, front-most last; each its own size where it
    /// fits. `keep`: all but that window, left where it is, in front.
    pub fn cascade_all(&mut self, keep: Option<&WindowElement>) {
        let Some(output) = self.arranged_output(keep) else { return };
        let Some(area) = self.tile_area(&output) else { return };
        let (windows, kept) = self.windows_to_arrange(&output, keep);
        self.remember_arrangement(&windows, "Cascade");
        let n = windows.len() as i32;
        for (i, w) in windows.iter().enumerate() {
            let step = if n > 1 { CASCADE.min((area.size.h / 3) / (n - 1)).max(4) } else { 0 };
            let at = area.loc + Point::from((i as i32 * step, i as i32 * step));
            let own = self.own_frame(w).unwrap_or_default();
            let room: Size<i32, Logical> = Size::from((area.size.w - (at.x - area.loc.x), area.size.h - (at.y - area.loc.y)));
            let size = Size::from((own.size.w.min(room.w).max(1), own.size.h.min(room.h).max(1)));
            Self::set_tiled(w, None);
            if let Some(p) = w.user_data().get::<PreZoom>() {
                p.0.borrow_mut().take();
            }
            self.set_frame(w, Rectangle::new(at, size), false);
            self.space.raise_element(w, false);
        }
        if kept.is_some() {
            self.bring_back(kept);
        } else if let Some(front) = windows.last() {
            self.focus_window(front);
        }
    }

    /// Hide every window on this workspace; again, bring them back.
    pub fn show_desktop(&mut self) {
        let hidden: Vec<WindowElement> = std::mem::take(&mut self.desktop_shown);
        let back: Vec<WindowElement> = hidden.into_iter().filter(|w| w.meta().minimized.get() && self.parked.0.borrow().contains(w)).collect();
        if !back.is_empty() {
            for w in &back {
                self.restore(w);
            }
            return;
        }
        let visible: Vec<WindowElement> = self.space.elements().cloned().collect();
        for w in visible.iter().rev() {
            self.minimize(w);
        }
        self.desktop_shown = visible;
    }

    /// After displays came or went: windows left on none go to the main
    /// display, cascaded; the rest are fitted to the display they are on
    /// (re-tiled, re-zoomed, or shrunk to fit).
    pub fn rehome_windows(&mut self) {
        let all: Vec<WindowElement> = self.space.elements().cloned().collect();
        for w in &all {
            self.rehome_window(w);
        }
        // The pointer too, if it was on a display that's gone.
        let pointer = self.pointer.current_location();
        if self.space.output_under(pointer).next().is_none()
            && let Some(g) = self.main_output().and_then(|o| self.space.output_geometry(&o))
        {
            self.warp_pointer((g.loc + g.size.downscale(2).to_point()).to_f64());
        }
    }

    fn on_a_display(&self, r: Rectangle<i32, Logical>) -> bool {
        self.space.outputs().any(|o| self.space.output_geometry(o).is_some_and(|g| g.overlaps(r)))
    }

    /// Where a remembered frame goes back to (Restore, unzooming, undoing
    /// an arrangement): just there while some of its top edge is on a
    /// display to take hold of, as the person may have left it hanging
    /// off one; else brought into reach ([`Self::reachable`]).
    pub(crate) fn put_back_at(&self, frame: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        let top = Rectangle::new(frame.loc, Size::from((frame.size.w, olo_theme::deco::TAB_HEIGHT.min(frame.size.h))));
        let held = self.space.outputs().filter_map(|o| self.space.output_geometry(o)).any(|g| top.intersection(g).is_some_and(|r| r.size.w >= GRAB.min(frame.size.w) && r.size.h >= top.size.h));
        if held { frame } else { self.reachable(frame) }
    }

    /// A window back from another workspace or from being hidden:
    /// rehomed if the displays changed while it was away, else just as
    /// it was (only re-tiled or re-zoomed, if the room for that changed:
    /// the Deskbar may have moved). A window someone made bigger than its
    /// display, across two, stays so.
    pub fn welcome_back(&mut self, w: &WindowElement, displays_changed: bool) {
        if displays_changed {
            self.rehome_window(w);
        } else if Self::tiled(w).is_some() || self.is_zoomed(w) || Self::zoomed_along(w).is_some() {
            self.refit(w);
        }
    }

    /// The space around arranged windows changed (`Settings::tiling`):
    /// tiled and zoomed windows on this workspace take their places again
    /// with it, at once; the rest when they come back (`welcome_back`).
    pub fn tiling_space_changed(&mut self) {
        self.auto.changed();
        let all: Vec<WindowElement> = self.space.elements().filter(|w| !self.behind_in_group(w)).cloned().collect();
        for w in &all {
            self.welcome_back(w, false);
        }
    }

    /// One window (mapped): onto the main display if it is on none, then
    /// fitted to the display it is on.
    pub fn rehome_window(&mut self, w: &WindowElement) {
        let Some(frame) = self.space.element_geometry(w) else { return };
        if !self.on_a_display(frame) {
            let Some(main) = self.main_output() else { return };
            let slot = self.free_cascade_slot(&main, w);
            info!(window = w.id(), "window to the main display");
            // Smaller if it would run off the display from there.
            let area = self.usable_area(&main).unwrap_or_default();
            let room: Size<i32, Logical> = Size::from((area.loc.x + area.size.w - slot.x, area.loc.y + area.size.h - slot.y));
            self.space.relocate_element(w, slot);
            if (frame.size.w > room.w || frame.size.h > room.h) && room.w > area.size.w / 2 && room.h > area.size.h / 2 {
                self.set_frame(w, Rectangle::new(slot, Size::from((frame.size.w.min(room.w), frame.size.h.min(room.h)))), false);
            }
        }
        // Its own frame (for Restore) goes along if it was elsewhere.
        let pre = w.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow());
        if let Some(r) = pre
            && !self.on_a_display(r)
            && let Some(at) = self.space.element_location(w)
        {
            w.user_data().get::<PreZoom>().unwrap().0.replace(Some(Rectangle::new(at, r.size)));
        }
        if let Some(z) = Self::zoomed_along(w)
            && !self.on_a_display(z.from)
            && let Some(at) = self.space.element_location(w)
        {
            Self::set_zoomed_along(w, Some(ZoomedAlong { from: Rectangle::new(at, z.from.size), ..z }));
        }
        self.refit(w);
    }
}

/// `frame` grown to `least` where it is smaller, kept inside `area` as
/// far as it fits (else at its start).
fn give_way(frame: Rectangle<i32, Logical>, least: Size<i32, Logical>, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let size = Size::from((frame.size.w.max(least.w), frame.size.h.max(least.h)));
    if size == frame.size {
        return frame;
    }
    let inside = |pos: i32, len: i32, lo: i32, span: i32| if len <= span { pos.clamp(lo, lo + span - len) } else { lo };
    Rectangle::new(Point::from((inside(frame.loc.x, size.w, area.loc.x, area.size.w), inside(frame.loc.y, size.h, area.loc.y, area.size.h))), size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// A drag held by its tree goes as far as it may, either way.
    #[test]
    fn a_drag_goes_as_far_as_it_may() {
        assert_eq!(furthest(300, |x| x <= 137), 137);
        assert_eq!(furthest(-300, |x| x >= -42), -42);
        assert_eq!(furthest(80, |_| true), 80);
        assert_eq!(furthest(80, |x| x == 0), 0);
    }

    #[test]
    fn zooming_along_one_axis_keeps_the_other() {
        let area = r(0, 30, 1280, 770);
        let w = r(100, 200, 500, 300);
        assert_eq!(zoom_along(w, area, Axis::Vertical), r(100, 30, 500, 770));
        assert_eq!(zoom_along(w, area, Axis::Horizontal), r(0, 200, 1280, 300));
        // Both ways is the whole area.
        assert_eq!(zoom_along(zoom_along(w, area, Axis::Vertical), area, Axis::Horizontal), area);
        // Partly off the area: pulled in, and no wider than it.
        assert_eq!(zoom_along(r(1000, 0, 500, 300), area, Axis::Vertical), r(780, 30, 500, 770));
        assert_eq!(zoom_along(r(-50, 0, 1500, 300), area, Axis::Vertical), r(0, 30, 1280, 770));
    }

    #[test]
    fn unzooming_along_one_axis_leaves_the_other() {
        let area = r(0, 30, 1280, 770);
        let from = r(100, 200, 500, 300);
        let zoomed = zoom_along(from, area, Axis::Vertical);
        assert!(spans(zoomed, area, Axis::Vertical) && !spans(zoomed, area, Axis::Horizontal));
        assert_eq!(unzoom_along(zoomed, from, Axis::Vertical), from);
        // Moved across while zoomed: it stays where it was moved to.
        let moved = r(400, 30, 500, 770);
        assert_eq!(unzoom_along(moved, from, Axis::Vertical), r(400, 200, 500, 300));
        assert_eq!(unzoom_along(r(0, 50, 1280, 300), from, Axis::Horizontal), r(100, 50, 500, 300));
    }

    #[test]
    fn into_reach_moves_as_little_as_it_takes() {
        let area = r(1280, 0, 960, 540);
        // Above the top: down to it, still hanging off the right.
        assert_eq!(into_reach(r(1907, -32, 937, 434), area), r(1907, 0, 937, 434));
        // Off the right: back to 48 px in.
        assert_eq!(into_reach(r(2300, 100, 400, 300), area), r(2192, 100, 400, 300));
        // Off the left, and below the bottom: its tab comes up.
        assert_eq!(into_reach(r(500, 600, 400, 300), area), r(1280 + 48 - 400, 540 - olo_theme::deco::TAB_HEIGHT, 400, 300));
        // Already in reach, hanging off: left alone.
        let hanging = r(2000, 300, 900, 600);
        assert!(in_reach(hanging, area));
        assert_eq!(into_reach(hanging, area), hanging);
        // Narrower than the reach: all of it across.
        assert_eq!(into_reach(r(2230, 10, 30, 30), area), r(2210, 10, 30, 30));
    }

    #[test]
    fn unzooming_along_puts_back_what_fitting_changed() {
        let area = r(8, 8, 944, 524);
        // Hanging off the left and wider than the area: zooming pulled it
        // in and narrowed it; unzooming puts both back.
        let from = r(-153, 81, 1061, 451);
        let zoomed = zoom_along(from, area, Axis::Vertical);
        assert_eq!(zoomed, r(8, 8, 944, 524));
        assert_eq!(unzoomed_along(zoomed, from, area, Axis::Vertical), from);
        let zoomed = zoom_along(from, area, Axis::Horizontal);
        assert_eq!(unzoomed_along(zoomed, from, area, Axis::Horizontal), from);
        // Moved across since: it stays where it was moved to.
        assert_eq!(unzoomed_along(r(40, 8, 944, 524), from, area, Axis::Vertical), r(40, 81, 944, 451));
    }

    #[test]
    fn halves_leave_the_gap_between_them() {
        let area = r(0, 0, 1000, 600);
        assert_eq!(part(area, Tile::Left.fraction(), 10), r(0, 0, 495, 600));
        assert_eq!(part(area, Tile::Right.fraction(), 10), r(505, 0, 495, 600));
        assert_eq!(part(area, Tile::BottomRight.fraction(), 10), r(505, 305, 495, 295));
        assert_eq!(part(area, Tile::CenterThird.fraction(), 0), r(333, 0, 334, 600));
        assert_eq!(inset(area, 8), r(8, 8, 984, 584));
    }

    #[test]
    fn dragged_parts_come_back_as_they_were() {
        // A part's frame turned back into fractions makes the same frame.
        let area = r(8, 8, 1088, 784);
        for tile in [Tile::Left, Tile::TopRight, Tile::BottomRight, Tile::CenterThird] {
            let frame = part(area, tile.fraction(), 8);
            assert_eq!(part(area, fraction_of(area, frame, 8), 8), frame, "{tile:?}");
        }
        // The left half dragged 130 px narrower stays that size.
        let narrower = with_side(part(area, Tile::Left.fraction(), 8), Side::Right, moved(Side::Left, (130, 0)));
        assert_eq!(narrower.size.w, 540 - 130);
        assert_eq!(part(area, fraction_of(area, narrower, 8), 8), narrower);
        // Growing by 130 to the left moves that side left; the right stays.
        let grown = with_side(r(556, 404, 540, 388), Side::Left, moved(Side::Left, (130, 0)));
        assert_eq!(grown, r(426, 404, 670, 388));
    }

    #[test]
    fn a_neighbour_on_two_sides_follows_both() {
        // Dragging the bottom-right corner of the top-left window: the
        // window to its lower right follows across the bottom (its top)
        // and in line on the right (its right), both at once.
        let links = [(1, r(500, 400, 500, 400), Side::Top, Side::Bottom), (1, r(500, 400, 500, 400), Side::Right, Side::Right), (2, r(0, 400, 492, 400), Side::Top, Side::Bottom)];
        let t = fold_sides(links.into_iter(), (30, -20));
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], (1, r(500, 400, 500, 400), r(500, 380, 530, 420)));
        assert_eq!(t[1], (2, r(0, 400, 492, 400), r(0, 380, 492, 420)));
    }

    #[test]
    fn grids() {
        let area = r(0, 0, 1600, 900);
        assert_eq!(grid(area, 1, 0), [area]);
        assert_eq!(grid(area, 2, 0), [r(0, 0, 800, 900), r(800, 0, 800, 900)]);
        // Three on a wide display: side by side, or two and one wide.
        let three = grid(area, 3, 0);
        assert_eq!(three.len(), 3);
        let covered: i32 = three.iter().map(|c| c.size.w * c.size.h).sum();
        assert_eq!(covered, 1600 * 900);
        // Five: three over two, the bottom two wider.
        let five = grid(area, 5, 0);
        assert_eq!(five[0], r(0, 0, 533, 450));
        assert_eq!(five[3], r(0, 450, 800, 450));
        assert!(grid(area, 0, 8).is_empty());
    }

    #[test]
    fn rolled_windows_fan_out_in_one_cell() {
        let cell = r(800, 450, 300, 400);
        let fanned = fan_in(cell, &[120, 100, 90, 140], 21);
        // Side by side along the top, each the whole cell.
        assert_eq!(fanned[0], (cell, 0));
        assert_eq!(fanned[1], (cell, 120));
        // The third's tab won't fit beside them: a row lower.
        assert_eq!(fanned[2], (r(800, 471, 300, 379), 0));
        assert_eq!(fanned[3], (r(800, 471, 300, 379), 90));
        // However many, the tabs stay in the cell.
        let many = fan_in(r(0, 0, 100, 50), &[100; 6], 21);
        assert!(many.iter().all(|(f, _)| f.loc.y + 21 <= 50 && f.size.h >= 21));
        assert!(fan_in(cell, &[], 21).is_empty());
    }

    #[test]
    fn a_window_too_big_for_its_tile_gives_way_inwards() {
        let area = r(8, 8, 2048, 524);
        // The right third, for a window that won't be under 290 wide: in
        // from the right edge, as keep_in_tile_area pushes it once drawn.
        let third = r(1800, 8, 256, 524);
        assert_eq!(give_way(third, Size::from((290, 100)), area), r(1766, 8, 290, 524));
        // The left third grows rightwards; one that fits stays as it is.
        assert_eq!(give_way(r(8, 8, 256, 524), Size::from((290, 100)), area), r(8, 8, 290, 524));
        assert_eq!(give_way(third, Size::from((200, 100)), area), third);
        // Bigger than the whole area: at its start.
        assert_eq!(give_way(third, Size::from((3000, 100)), area), r(8, 8, 3000, 524));
    }
}
