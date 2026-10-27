//! Arranging windows from the keyboard (tabula_protocol::shortcuts): tiling
//! one into part of its display, centering it, moving it to another
//! display, tiling or cascading them all, showing the desktop; and putting
//! windows back on a display when theirs goes away (unplugged, or set to
//! mirror). Tiled windows keep space around them
//! (`Settings::tiling`: gap and margin) and remember how they were, so
//! Restore (or unzooming) puts them back.

use std::cell::Cell;

use smithay::{
    desktop::{WindowSurface, space::SpaceElement},
    output::Output,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle, Size},
};
use tabula_protocol::Tile;
use tracing::info;

use super::{WindowElement, wm::PreZoom};
use crate::state::{AnvilState, Backend};

/// The part of the display a window was tiled into (re-tiled when its
/// display changes).
#[derive(Default)]
struct Tiled(Cell<Option<Tile>>);

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
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => {
                let _ = x.set_maximized(false);
                let _ = x.configure(Rectangle::new(frame.loc + window.content_offset(), content));
            }
        }
        self.space.relocate_element(window, frame.loc);
    }

    fn tiled(window: &WindowElement) -> Option<Tile> {
        window.user_data().get::<Tiled>().and_then(|t| t.0.get())
    }

    fn set_tiled(window: &WindowElement, tile: Option<Tile>) {
        window.user_data().insert_if_missing(Tiled::default);
        window.user_data().get::<Tiled>().unwrap().0.set(tile);
    }

    /// Tile a window into part of its display.
    pub fn tile_window(&mut self, window: &WindowElement, tile: Tile) {
        let Some(area) = self.window_output(window).and_then(|o| self.tile_area(&o)) else { return };
        self.set_frame(window, part(area, tile.fraction(), self.tile_gap()), true);
        Self::set_tiled(window, Some(tile));
    }

    /// Back to how it was before it was tiled or zoomed.
    pub fn restore_frame(&mut self, window: &WindowElement) {
        Self::set_tiled(window, None);
        self.unzoom_window(window);
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
        if let Some(tile) = Self::tiled(window) {
            self.tile_window(window, tile);
            return;
        }
        if self.is_zoomed(window) {
            if let Some(area) = self.zoom_area(window)
                && self.space.element_geometry(window) != Some(area)
            {
                self.zoom_window(window);
            }
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

    /// Every window on the active display in a grid, in reading order of
    /// where they are now.
    pub fn tile_all(&mut self) {
        let Some(output) = self.active_output() else { return };
        let Some(area) = self.tile_area(&output) else { return };
        let mut windows = self.windows_on(&output);
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

    /// Every window on the active display stacked diagonally from the top
    /// left, as they open, front-most last; each its own size where it
    /// fits.
    pub fn cascade_all(&mut self) {
        let Some(output) = self.active_output() else { return };
        let Some(area) = self.tile_area(&output) else { return };
        let windows = self.windows_on(&output);
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
