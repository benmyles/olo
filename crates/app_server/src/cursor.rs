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

/// Where the pointer is drawn for a frame (`Track::at`): resampled onto
/// the frame clock, as `pace.rs` does for scrolling and Android does for
/// touch. A mouse reports at its own rate (125 Hz, 1000 Hz) against the
/// display's (60, 120 Hz): drawn at the last report's position, one
/// frame got two reports' worth of motion and the next three, and the
/// pointer moved unevenly though the hand didn't. The frame shown at a
/// vblank draws the pointer where it was `LATENCY` before that vblank,
/// interpolated between the reports either side of then; past the last
/// report it goes on at the last velocity, but at most `AHEAD` (an
/// extrapolation a frame long overshot every turn of the hand, 2026-11-01).
/// Nothing while the mouse is still (reports older than `STALE`), or
/// after something warped the pointer. OLO_POINTER_AHEAD=0 draws it at
/// its real position, as before.
#[derive(Debug, Default)]
pub struct Track {
    /// The last reports, oldest first: when (the input clock, monotonic,
    /// the vblanks' too) and where.
    reports: std::collections::VecDeque<(std::time::Duration, Point<f64, Logical>)>,
}

/// Reports older than this say nothing about now (the mouse stopped).
const STALE: std::time::Duration = std::time::Duration::from_millis(40);
/// Drawn this far behind the vblank, so the position is interpolated
/// between reports, not guessed, at any report rate of 250 Hz and up.
const LATENCY: std::time::Duration = std::time::Duration::from_millis(4);
/// At most this far past the last report, along its motion.
const AHEAD: std::time::Duration = std::time::Duration::from_millis(4);
const KEPT: usize = 8;

impl Track {
    /// A pointer report: at `at` the pointer was at `pos`.
    pub fn note(&mut self, at: std::time::Duration, pos: Point<f64, Logical>) {
        if self.reports.len() == KEPT {
            self.reports.pop_front();
        }
        self.reports.push_back((at, pos));
    }

    /// The last report's time, and the interval from the one before.
    pub fn last(&self) -> Option<(std::time::Duration, std::time::Duration)> {
        let n = self.reports.len();
        let (t1, _) = *self.reports.get(n.checked_sub(1)?)?;
        let t0 = self.reports.get(n.checked_sub(2)?).map_or(t1, |(t, _)| *t);
        Some((t1, t1 - t0))
    }

    /// Where the pointer is drawn for a frame shown at `target`, given
    /// where it is now (`now`).
    pub fn at(&self, target: std::time::Duration, now: Point<f64, Logical>) -> Point<f64, Logical> {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| std::env::var_os("OLO_POINTER_AHEAD").is_none_or(|v| v != "0")) {
            return now;
        }
        let n = self.reports.len();
        if n < 2 {
            return now;
        }
        let (t1, p1) = self.reports[n - 1];
        if p1 != now || target <= t1 && t1 - target > STALE || target > t1 && target - t1 > STALE {
            return now;
        }
        let t = target.saturating_sub(LATENCY);
        if t >= t1 {
            // Past the last report: on at its velocity, a little.
            let (t0, p0) = self.reports[n - 2];
            if t1 <= t0 || t1 - t0 > STALE {
                return now;
            }
            let lead = (t - t1).min(AHEAD).as_secs_f64();
            let dt = (t1 - t0).as_secs_f64();
            return Point::from((p1.x + (p1.x - p0.x) / dt * lead, p1.y + (p1.y - p0.y) / dt * lead));
        }
        // Between two reports: where it was then.
        for w in self.reports.iter().rev().collect::<Vec<_>>().windows(2) {
            let ((tb, pb), (ta, pa)) = (*w[0], *w[1]);
            if ta <= t && t <= tb {
                if tb == ta {
                    return pb;
                }
                let f = (t - ta).as_secs_f64() / (tb - ta).as_secs_f64();
                return Point::from((pa.x + (pb.x - pa.x) * f, pa.y + (pb.y - pa.y) * f));
            }
        }
        now
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
    fn interpolates_between_reports_a_little_behind() {
        let mut t = Track::default();
        t.note(ms(100), Point::from((0.0, 0.0)));
        t.note(ms(108), Point::from((8.0, 0.0)));
        t.note(ms(116), Point::from((16.0, 0.0)));
        // A frame at 118: drawn where it was at 114, between the last two.
        let p = t.at(ms(118), Point::from((16.0, 0.0)));
        assert_eq!((p.x, p.y), (14.0, 0.0));
    }

    #[test]
    fn goes_on_a_little_past_the_last_report() {
        let mut t = Track::default();
        t.note(ms(100), Point::from((0.0, 0.0)));
        t.note(ms(108), Point::from((8.0, 0.0)));
        // At 1 px/ms: a frame at 130 wants 126, 18 ms past the last report: capped at 4 ms.
        let p = t.at(ms(130), Point::from((8.0, 0.0)));
        assert_eq!((p.x, p.y), (12.0, 0.0));
        // A frame at 114 wants 110: 2 ms on.
        let p = t.at(ms(114), Point::from((8.0, 0.0)));
        assert_eq!((p.x, p.y), (10.0, 0.0));
    }

    #[test]
    fn stays_put_when_stopped_or_unsure() {
        let mut t = Track::default();
        let now = Point::from((18.0, 10.0));
        assert_eq!(t.at(ms(112), now), now);
        t.note(ms(100), Point::from((10.0, 10.0)));
        assert_eq!(t.at(ms(112), now), now);
        t.note(ms(108), now);
        assert_eq!(t.at(ms(200), now), now);
        assert_eq!(t.at(ms(112), Point::from((500.0, 500.0))), Point::from((500.0, 500.0)));
        assert_eq!(t.last(), Some((ms(108), ms(8))));
    }
}
