//! Arranging windows from the keyboard (tabula_protocol::shortcuts): tiling
//! one into part of its display, centering it, moving it to another
//! display, tiling, cascading or fanning them all out (and undoing that),
//! showing the desktop; and putting
//! windows back on a display when theirs goes away (unplugged, or set to
//! mirror). Tiled windows keep space around them
//! (`Settings::tiling`: gap and margin) and remember how they were, so
//! Restore (or unzooming) puts them back.
//!
//! Tiled windows that meet move together: dragging the edge of one moves
//! the edges of the other tiled windows on that line (beside it across the
//! gap, or in line with it), as the split between them. They stay tiled,
//! in the parts they now have. Windows not tiled by a command are never
//! pushed, and a tiled window sized by hand with no tiled neighbours is
//! no longer tiled.

use std::cell::Cell;

use smithay::{
    desktop::{WindowSurface, space::SpaceElement},
    output::Output,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle, Size},
};
use tabula_protocol::Tile;
use tracing::info;

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

/// Where a window was before an arrangement moved it, all Undo Window
/// Arrangement needs to put it back.
#[derive(Clone, Debug)]
pub struct Placed {
    window: WindowElement,
    frame: Rectangle<i32, Logical>,
    tile: Option<Tile>,
    part: Option<Part>,
    /// Its own frame, if it was tiled or zoomed (for Restore).
    pre: Option<Rectangle<i32, Logical>>,
    tab_offset: i32,
}

/// The part a tiled window has after its edges were dragged, in place of
/// its tile's own.
#[derive(Default)]
struct TilePart(Cell<Option<Part>>);

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

/// A tiled window's edges being dragged, and the neighbours that follow.
#[derive(Debug)]
pub struct TileDrag {
    window: WindowElement,
    /// Its frame when the drag began, and the sides being dragged.
    start: Rectangle<i32, Logical>,
    sides: Vec<Side>,
    links: Vec<Link>,
    /// The size change last applied to the neighbours.
    applied: (i32, i32),
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
const CASCADE: i32 = 24;

/// `area` less `margin` all round.
fn inset(area: Rectangle<i32, Logical>, margin: i32) -> Rectangle<i32, Logical> {
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

impl<BackendData: Backend> AnvilState<BackendData> {
    fn tile_gap(&self) -> i32 {
        self.settings.tiling.gap.min(200) as i32
    }

    /// Where tiled windows go on a display.
    fn tile_area(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        Some(inset(self.usable_area(output)?, self.settings.tiling.margin.min(400) as i32))
    }

    /// The display shortcuts act on: the focused window's, else the
    /// pointer's.
    fn active_output(&self) -> Option<Output> {
        if let Some(w) = self.focused_window() {
            return self.window_output(&w);
        }
        let pointer = self.pointer.current_location();
        self.space.output_under(pointer).next().cloned().or_else(|| self.main_output())
    }

    /// Put a window's frame (tab and border included) at `frame`, no longer
    /// zoomed. `remember`: keep where it was, for Restore.
    pub fn set_frame(&mut self, window: &WindowElement, frame: Rectangle<i32, Logical>, remember: bool) {
        let Some(location) = self.space.element_location(window) else { return };
        if remember {
            window.user_data().insert_if_missing(PreZoom::default);
            let pre = window.user_data().get::<PreZoom>().unwrap();
            pre.0.borrow_mut().get_or_insert(Rectangle::new(location, window.geometry().size));
        }
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

    fn set_tiled(window: &WindowElement, tile: Option<Tile>) {
        window.user_data().insert_if_missing(Tiled::default);
        window.user_data().get::<Tiled>().unwrap().0.set(tile);
        Self::set_part(window, None);
    }

    fn set_part(window: &WindowElement, part: Option<Part>) {
        window.user_data().insert_if_missing(TilePart::default);
        window.user_data().get::<TilePart>().unwrap().0.set(part);
    }

    fn custom_part(window: &WindowElement) -> Option<Part> {
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

    /// Tile a window into part of its display (a tiling command: its
    /// tile's own part).
    pub fn tile_window(&mut self, window: &WindowElement, tile: Tile) {
        let Some(area) = self.window_output(window).and_then(|o| self.tile_area(&o)) else { return };
        self.set_frame(window, part(area, tile.fraction(), self.tile_gap()), true);
        Self::set_tiled(window, Some(tile));
    }

    /// Put a tiled window back in its part of its display: its tile's, or
    /// the one its edges were dragged to.
    pub fn retile(&mut self, window: &WindowElement, tile: Tile) {
        let Some(custom) = Self::custom_part(window) else { return self.tile_window(window, tile) };
        let Some(area) = self.window_output(window).and_then(|o| self.tile_area(&o)) else { return };
        self.set_frame(window, part(area, custom, self.tile_gap()), true);
    }

    /// An edge drag of `window` (`sides`) is starting or going on: the
    /// first time, find its tiled neighbours. A tiled window with none is
    /// being sized away from its tile by hand: no longer tiled.
    pub fn tile_drag_begin(&mut self, window: &WindowElement, sides: &[Side]) {
        if self.tile_drag.as_ref().is_some_and(|d| d.window == *window) {
            return;
        }
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
        if links.is_empty() {
            self.forget_tile(window);
        }
        self.tile_drag = Some(TileDrag { window: window.clone(), start, sides: sides.to_vec(), links, applied: (0, 0) });
    }

    /// The smallest frame a window takes: its app's minimum content size
    /// (if it gives one) with the tab and border, and never under
    /// `MIN_TILE`.
    fn min_frame(&self, window: &WindowElement) -> Size<i32, Logical> {
        use smithay::wayland::{compositor::with_states, shell::xdg::SurfaceCachedState};
        let content = window.wl_surface().map(|s| with_states(&s, |st| st.cached_state.get::<SurfaceCachedState>().current().min_size)).unwrap_or_default();
        let frame = self.space.element_geometry(window).map(|g| g.size).unwrap_or_default();
        let extra = frame - window.content_size();
        Size::from(((content.w + extra.w).max(MIN_TILE), (content.h + extra.h).max(MIN_TILE)))
    }

    /// The size change the drag may make (neighbours keep a minimum).
    pub fn tile_drag_limit(&self, d: (i32, i32)) -> (i32, i32) {
        self.tile_drag.as_ref().map_or(d, |t| limit(&t.links, d))
    }

    /// The dragged window's size changed by `d` from where it began: move
    /// its neighbours' sides with it.
    pub fn tile_drag_apply(&mut self, d: (i32, i32)) {
        let Some(t) = self.tile_drag.as_mut() else { return };
        if t.applied == d || t.links.is_empty() {
            return;
        }
        t.applied = d;
        let moves: Vec<_> = t.links.iter().map(|l| (l.window.clone(), with_side(l.start, l.side, moved(l.follows, d)))).collect();
        for (w, r) in moves {
            if self.space.element_geometry(&w) != Some(r) {
                self.set_frame(&w, r, false);
            }
        }
    }

    /// The drag ended: the dragged window and its neighbours keep the
    /// parts they now have.
    pub fn tile_drag_end(&mut self) {
        let Some(t) = self.tile_drag.take() else { return };
        if t.links.is_empty() {
            return;
        }
        let gap = self.tile_gap();
        let Some(area) = self.window_output(&t.window).and_then(|o| self.tile_area(&o)) else { return };
        let mut frames: Vec<(WindowElement, Rectangle<i32, Logical>)> = t.links.iter().filter_map(|l| Some((l.window.clone(), self.space.element_geometry(&l.window)?))).collect();
        // Its own frame as the drag left it (it may not have drawn it yet).
        let own = t.sides.iter().fold(t.start, |r, &side| with_side(r, side, moved(side, t.applied)));
        frames.push((t.window.clone(), own));
        for (w, r) in frames {
            Self::set_part(&w, Some(fraction_of(area, r, gap)));
        }
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

    /// A window's own size (not tiled or zoomed), as it is now.
    fn own_frame(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        let pre = window.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow());
        pre.or_else(|| self.space.element_geometry(window))
    }

    pub fn center_window(&mut self, window: &WindowElement) {
        let (Some(area), Some(frame)) = (self.window_output(window).and_then(|o| self.usable_area(&o)), self.own_frame(window)) else {
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
        self.move_to_output(window, &from, &to);
        self.focus_window(window);
    }

    /// Put a window on `to`, where it was on `from` (a display that may be
    /// gone: then just a free cascade spot).
    fn move_to_output(&mut self, window: &WindowElement, from: &Output, to: &Output) {
        let Some(dest) = self.usable_area(to) else { return };
        let zoomed = self.is_zoomed(window);
        let own = self.own_frame(window);
        let src = self.usable_area(from);
        // Its own frame, at the same relative place on the new display.
        if let (Some(own), Some(src)) = (own, src) {
            let rel = |v: i32, origin: i32, len: i32, dlen: i32, size: i32| {
                let f = (v - origin) as f32 / (len - size).max(1) as f32;
                (f.clamp(0.0, 1.0) * (dlen - size).max(0) as f32).round() as i32
            };
            let size = Size::from((own.size.w.min(dest.size.w), own.size.h.min(dest.size.h)));
            let loc = dest.loc
                + Point::from((
                    rel(own.loc.x, src.loc.x, src.size.w, dest.size.w, size.w),
                    rel(own.loc.y, src.loc.y, src.size.h, dest.size.h, size.h),
                ));
            if let Some(p) = window.user_data().get::<PreZoom>()
                && p.0.borrow().is_some()
            {
                *p.0.borrow_mut() = Some(Rectangle::new(loc, size));
            }
            if !zoomed && Self::tiled(window).is_none() {
                self.set_frame(window, Rectangle::new(loc, size), false);
            } else {
                // Somewhere on the new display first, so it is "on" it.
                self.space.relocate_element(window, loc);
            }
        } else {
            let slot = self.free_cascade_slot(to, window);
            self.space.relocate_element(window, slot);
        }
        self.refit(window);
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

    /// Fit a window to the display it is on now: re-tile or re-zoom it
    /// there, else shrink it to fit and bring its tab into view.
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
        let (Some(area), Some(frame)) = (self.zoom_area(window), self.space.element_geometry(window)) else { return };
        let size = Size::from((frame.size.w.min(area.size.w), frame.size.h.min(area.size.h)));
        let loc = Point::from((
            frame.loc.x.clamp(area.loc.x, area.loc.x + area.size.w - size.w),
            frame.loc.y.clamp(area.loc.y, area.loc.y + area.size.h - size.h),
        ));
        if size != frame.size {
            self.set_frame(window, Rectangle::new(loc, size), false);
        } else if loc != frame.loc {
            self.space.relocate_element(window, loc);
        }
    }

    /// Windows on a display on this workspace, back to front.
    fn windows_on(&self, output: &Output) -> Vec<WindowElement> {
        let all: Vec<WindowElement> = self.space.elements().cloned().collect();
        all.into_iter().filter(|w| self.window_output(w).as_ref() == Some(output)).collect()
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
                    tab_offset: w.decoration().tab_offset,
                })
            })
            .collect()
    }

    /// Keep how `windows` are now, for Undo Window Arrangement.
    fn remember_arrangement(&mut self, windows: &[WindowElement]) {
        self.arranged = self.placed(windows);
    }

    /// Every window the last arrangement moved back where and how big it
    /// was (those still open); again, as the arrangement had them.
    pub fn undo_arrangement(&mut self) {
        let before = std::mem::take(&mut self.arranged);
        let windows: Vec<WindowElement> = before.iter().map(|p| p.window.clone()).filter(|w| self.space.element_location(w).is_some()).collect();
        let now = self.placed(&windows);
        for p in &before {
            if !windows.contains(&p.window) {
                continue;
            }
            self.set_frame(&p.window, p.frame, false);
            Self::set_tiled(&p.window, p.tile);
            Self::set_part(&p.window, p.tile.and(p.part));
            p.window.user_data().insert_if_missing(PreZoom::default);
            p.window.user_data().get::<PreZoom>().unwrap().0.replace(p.pre);
            p.window.decoration().tab_offset = p.tab_offset;
        }
        info!(windows = windows.len(), "undid an arrangement");
        self.arranged = now;
    }

    /// Every window on the active display at the top left of it, stacked
    /// with their tabs side by side along the top (as BeOS people slid
    /// them by hand), in the order their tabs are now from the left. Each
    /// starts where the tab before it ends and is narrowed so it stops
    /// beside Deskbar, but never below a quarter of the display (so four
    /// always fit, narrower ones more); past that the rest go as far
    /// right as they fit, their tabs slid along them to keep stepping
    /// right, and overlap only when there's no room left for them.
    pub fn fan_out(&mut self) {
        let Some(output) = self.active_output() else { return };
        let (Some(area), Some(screen)) = (self.tile_area(&output), self.space.output_geometry(&output)) else { return };
        let windows = self.windows_on(&output);
        if windows.is_empty() {
            return;
        }
        self.remember_arrangement(&windows);
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
        info!(windows = order.len(), "fanned out");
    }

    /// Every window on the active display in a grid, in reading order of
    /// where they are now.
    pub fn tile_all(&mut self) {
        let Some(output) = self.active_output() else { return };
        let Some(area) = self.tile_area(&output) else { return };
        let mut windows = self.windows_on(&output);
        self.remember_arrangement(&windows);
        windows.sort_by_key(|w| {
            let g = self.space.element_geometry(w).unwrap_or_default();
            (g.loc.y + g.size.h / 2, g.loc.x + g.size.w / 2)
        });
        let cells = grid(area, windows.len(), self.tile_gap());
        for (w, cell) in windows.iter().zip(cells) {
            Self::set_tiled(w, None);
            self.set_frame(w, cell, true);
        }
        info!(windows = windows.len(), "tiled all");
    }

    /// The resizable windows of app `app_id` on this workspace (not its
    /// alerts or other fixed-size panels) in a grid on the active display,
    /// in front, in reading order of where they are now: an app showing
    /// all its documents at once (Terminal's groups). False if it has none.
    pub fn tile_app(&mut self, app_id: &str) -> bool {
        let Some(output) = self.active_output() else { return false };
        let Some(area) = self.tile_area(&output) else { return false };
        use smithay::wayland::{compositor::with_states, shell::xdg::SurfaceCachedState};
        let resizable = |w: &WindowElement| {
            w.wl_surface().is_none_or(|s| {
                with_states(&s, |st| {
                    let mut cached = st.cached_state.get::<SurfaceCachedState>();
                    let c = cached.current();
                    (c.max_size.w == 0 || c.max_size.w > c.min_size.w) && (c.max_size.h == 0 || c.max_size.h > c.min_size.h)
                })
            })
        };
        let mut windows: Vec<WindowElement> = self.space.elements().filter(|w| w.app_id() == app_id && resizable(w)).cloned().collect();
        if windows.is_empty() {
            return false;
        }
        self.remember_arrangement(&windows);
        windows.sort_by_key(|w| {
            let g = self.space.element_geometry(w).unwrap_or_default();
            (g.loc.y + g.size.h / 2, g.loc.x + g.size.w / 2)
        });
        let cells = grid(area, windows.len(), self.tile_gap());
        for (w, cell) in windows.iter().zip(cells) {
            Self::set_tiled(w, None);
            self.set_frame(w, cell, true);
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
    /// fits.
    pub fn cascade_all(&mut self) {
        let Some(output) = self.active_output() else { return };
        let Some(area) = self.tile_area(&output) else { return };
        let windows = self.windows_on(&output);
        self.remember_arrangement(&windows);
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
        if let Some(front) = windows.last() {
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
        self.refit(w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
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
}
