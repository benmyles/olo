//! The pointer: Haiku's system cursors (`olo-hvif`), drawn at each
//! output's scale and kept once drawn. Named cursors, from our own
//! decorations or from clients over cursor-shape-v1, map onto them.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus},
    utils::{Logical, Point, Transform},
    wayland::compositor,
};
pub use olo_hvif::cursor::{Cursor, SIZE, Shape};

/// The Haiku cursor for a CSS / cursor-shape-v1 name.
pub fn shape_for(icon: CursorIcon) -> Shape {
    match icon {
        CursorIcon::ContextMenu => Shape::ContextMenu,
        CursorIcon::Help => Shape::Help,
        CursorIcon::Pointer => Shape::FollowLink,
        CursorIcon::Progress | CursorIcon::Wait => Shape::Progress,
        CursorIcon::Cell | CursorIcon::Crosshair => Shape::CrossHair,
        CursorIcon::Text => Shape::IBeam,
        CursorIcon::VerticalText => Shape::IBeamHorizontal,
        CursorIcon::Alias => Shape::CreateLink,
        CursorIcon::Copy => Shape::Copy,
        CursorIcon::Move | CursorIcon::AllScroll => Shape::Move,
        CursorIcon::NoDrop | CursorIcon::NotAllowed => Shape::NotAllowed,
        CursorIcon::Grab => Shape::Grab,
        CursorIcon::Grabbing => Shape::Grabbing,
        CursorIcon::EResize => Shape::ResizeEast,
        CursorIcon::NResize => Shape::ResizeNorth,
        CursorIcon::NeResize => Shape::ResizeNorthEast,
        CursorIcon::NwResize => Shape::ResizeNorthWest,
        CursorIcon::SResize => Shape::ResizeSouth,
        CursorIcon::SeResize => Shape::ResizeSouthEast,
        CursorIcon::SwResize => Shape::ResizeSouthWest,
        CursorIcon::WResize => Shape::ResizeWest,
        CursorIcon::EwResize | CursorIcon::ColResize => Shape::ResizeEastWest,
        CursorIcon::NsResize | CursorIcon::RowResize => Shape::ResizeNorthSouth,
        CursorIcon::NeswResize => Shape::ResizeNorthEastSouthWest,
        CursorIcon::NwseResize => Shape::ResizeNorthWestSouthEast,
        CursorIcon::ZoomIn => Shape::ZoomIn,
        CursorIcon::ZoomOut => Shape::ZoomOut,
        _ => Shape::SystemDefault,
    }
}

/// The built-in cursor `status` asks for, if any (none when hidden or
/// when the client draws its own).
pub fn named(status: &CursorImageStatus) -> Option<Shape> {
    match status {
        CursorImageStatus::Named(icon) => Some(shape_for(*icon)),
        _ => None,
    }
}

/// What to draw for the pointer in a screenshot: the built-in cursor,
/// standing in for one a client draws itself; nothing when hidden.
pub fn pictured(status: &CursorImageStatus) -> Option<Shape> {
    match status {
        CursorImageStatus::Hidden => None,
        _ => Some(named(status).unwrap_or(Shape::SystemDefault)),
    }
}

/// Where the pointer's hot pixel is in its image, for either kind.
pub fn hotspot(status: &CursorImageStatus) -> Point<i32, Logical> {
    match status {
        CursorImageStatus::Surface(surface) => compositor::with_states(surface, |states| {
            states.data_map.get::<std::sync::Mutex<CursorImageAttributes>>().map(|a| a.lock().unwrap_or_else(|e| e.into_inner()).hotspot).unwrap_or_default()
        }),
        CursorImageStatus::Named(icon) => {
            let (x, y) = shape_for(*icon).hotspot();
            (x as i32, y as i32).into()
        }
        CursorImageStatus::Hidden => (0, 0).into(),
    }
}

/// `shape` drawn for a screen of `scale`; drawn once, then shared.
pub fn image(shape: Shape, scale: f64) -> Arc<Cursor> {
    type Drawn = HashMap<(Shape, u64), Arc<Cursor>>;
    static CACHE: OnceLock<Mutex<Drawn>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache.entry((shape, scale.to_bits())).or_insert_with(|| Arc::new(olo_hvif::cursor::render(shape, scale))).clone()
}

thread_local! {
    static BUFFERS: std::cell::RefCell<HashMap<(Shape, u64), MemoryRenderBuffer>> = Default::default();
}

/// `shape` at `scale` as a buffer for the renderer (premultiplied, its
/// texture kept once uploaded).
pub fn buffer(shape: Shape, scale: f64) -> MemoryRenderBuffer {
    BUFFERS.with_borrow_mut(|b| {
        b.entry((shape, scale.to_bits()))
            .or_insert_with(|| {
                let c = image(shape, scale);
                MemoryRenderBuffer::from_slice(&c.rgba, Fourcc::Abgr8888, (c.size as i32, c.size as i32), 1, Transform::Normal, None)
            })
            .clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_haiku_cursors() {
        assert_eq!(shape_for(CursorIcon::Default), Shape::SystemDefault);
        assert_eq!(shape_for(CursorIcon::Text), Shape::IBeam);
        assert_eq!(shape_for(CursorIcon::WResize), Shape::ResizeWest);
        let a = image(Shape::IBeam, 2.0);
        assert!(Arc::ptr_eq(&a, &image(Shape::IBeam, 2.0)));
        assert_eq!(a.size, 44);
    }
}

/// Where the pointer will be when the frame shows (`Track::at`). A
/// mouse reports at its own rate (125 Hz, 1000 Hz) against the display's
/// (60, 120 Hz), so a frame drawn with the last position got two
/// reports' worth of motion and the next three: the pointer moved
/// unevenly though the hand didn't. As `pace.rs` does for scrolling,
/// the frame draws the pointer where the last reports say it will be at
/// the frame's vblank, from the velocity of the last two (extrapolated
/// at most a frame ahead, and only while reports keep coming): motion
/// even on any display, and a frame's worth less behind the hand.
#[derive(Debug, Default)]
pub struct Track {
    /// The last reports: when (the input clock, monotonic) and where.
    last: Option<(std::time::Duration, Point<f64, Logical>)>,
    before: Option<(std::time::Duration, Point<f64, Logical>)>,
}

/// Reports older than this say nothing about now (the mouse stopped).
const STALE: std::time::Duration = std::time::Duration::from_millis(40);
/// At most this far ahead of the last report.
const AHEAD: std::time::Duration = std::time::Duration::from_millis(17);

impl Track {
    /// A pointer report: at `at` the pointer was at `pos`.
    pub fn note(&mut self, at: std::time::Duration, pos: Point<f64, Logical>) {
        self.before = self.last.take();
        self.last = Some((at, pos));
    }

    /// Where the pointer is drawn for a frame shown at `target`, given
    /// where it is now (`now`): ahead along its last motion, or just
    /// `now` when it isn't moving or the reports are too few or old.
    pub fn at(&self, target: std::time::Duration, now: Point<f64, Logical>) -> Point<f64, Logical> {
        // Off unless asked for (OLO_POINTER_AHEAD=1): tried on Ben's
        // session (2026-11-01) it made the pointer feel worse, not
        // better; it wants a measured trace of reports against vblanks
        // before another try.
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| std::env::var_os("OLO_POINTER_AHEAD").is_some_and(|v| v == "1")) {
            return now;
        }
        let (Some((t1, p1)), Some((t0, p0))) = (self.last, self.before) else { return now };
        if target <= t1 || target - t1 > STALE || t1 <= t0 || t1 - t0 > STALE || p1 != now {
            return now;
        }
        let ahead = (target - t1).min(AHEAD).as_secs_f64();
        let dt = (t1 - t0).as_secs_f64();
        Point::from((p1.x + (p1.x - p0.x) / dt * ahead, p1.y + (p1.y - p0.y) / dt * ahead))
    }
}

#[cfg(test)]
mod track_tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn moves_ahead_along_the_last_motion() {
        // SAFETY: a test's own environment, before the switch is read.
        unsafe { std::env::set_var("OLO_POINTER_AHEAD", "1") };
        let mut t = Track::default();
        t.note(ms(100), Point::from((10.0, 10.0)));
        t.note(ms(108), Point::from((18.0, 10.0)));
        // 1 px/ms: a frame 4 ms after the last report draws it 4 px on.
        let p = t.at(ms(112), Point::from((18.0, 10.0)));
        assert_eq!((p.x, p.y), (22.0, 10.0));
    }

    #[test]
    fn stays_put_when_stopped_or_unsure() {
        let mut t = Track::default();
        let now = Point::from((18.0, 10.0));
        assert_eq!(t.at(ms(112), now), now);
        t.note(ms(100), Point::from((10.0, 10.0)));
        assert_eq!(t.at(ms(112), now), now);
        t.note(ms(108), now);
        // Old reports: the mouse stopped.
        assert_eq!(t.at(ms(200), now), now);
        // The pointer was moved by something else since (warped).
        assert_eq!(t.at(ms(112), Point::from((500.0, 500.0))), Point::from((500.0, 500.0)));
    }

    #[test]
    fn at_most_a_frame_ahead() {
        // SAFETY: a test's own environment, before the switch is read.
        unsafe { std::env::set_var("OLO_POINTER_AHEAD", "1") };
        let mut t = Track::default();
        t.note(ms(100), Point::from((0.0, 0.0)));
        t.note(ms(101), Point::from((10.0, 0.0)));
        let p = t.at(ms(130), Point::from((10.0, 0.0)));
        assert!(p.x <= 10.0 + 10.0 * 17.0 + 0.001, "{}", p.x);
    }
}
