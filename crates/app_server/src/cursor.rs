//! The pointer: Haiku's system cursors (`olo-hvif`), drawn at each
//! output's scale and kept once drawn. Named cursors, from our own
//! decorations or from clients over cursor-shape-v1, map onto them.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

pub use olo_hvif::cursor::{Cursor, SIZE, Shape};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus},
    utils::{Logical, Point, Transform},
    wayland::compositor,
};

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
        CursorImageStatus::Surface(surface) => {
            compositor::with_states(surface, |states| states.data_map.get::<olo_sync::Mutex<CursorImageAttributes>>().map(|a| a.lock().unwrap_or_else(|e| e.into_inner()).hotspot).unwrap_or_default())
        }
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
    /// Reports in the last second (their times), for the rate.
    recent: std::collections::VecDeque<std::time::Duration>,
    /// Gaps over `GAP` within a burst of motion, in the last minute:
    /// when, and how long.
    gaps: std::collections::VecDeque<(std::time::Duration, std::time::Duration)>,
}

/// Reports older than this say nothing about now (the mouse stopped).
const STALE: std::time::Duration = std::time::Duration::from_millis(40);
/// Drawn this far behind the vblank, so the position is interpolated
/// between reports, not guessed, at any report rate of 250 Hz and up.
const LATENCY: std::time::Duration = std::time::Duration::from_millis(4);
/// At most this far past the last report, along its motion.
const AHEAD: std::time::Duration = std::time::Duration::from_millis(4);
const KEPT: usize = 16;
/// The velocity is taken over at least this span of reports: two reports
/// a millisecond apart say little (a 1 kHz mouse's noise).
const VELOCITY_SPAN: std::time::Duration = std::time::Duration::from_millis(6);
/// A gap between reports longer than this, while moving, is one worth
/// counting (a 2.4 GHz receiver dropping packets, a receiver asleep).
const GAP: std::time::Duration = std::time::Duration::from_millis(5);
/// Gaps are counted over this long.
const GAP_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

impl Track {
    /// A pointer report: at `at` the pointer was at `pos`.
    pub fn note(&mut self, at: std::time::Duration, pos: Point<f64, Logical>) {
        if let Some(&(last, _)) = self.reports.back()
            && at > last
        {
            let gap = at - last;
            // (A pause of the hand is no gap: only within a run of reports,
            // the one before this gap recent.)
            if gap > GAP && gap < STALE {
                self.gaps.push_back((at, gap));
            }
        }
        if self.reports.len() == KEPT {
            self.reports.pop_front();
        }
        self.reports.push_back((at, pos));
        self.recent.push_back(at);
        while self.recent.front().is_some_and(|&t| at.saturating_sub(t) > std::time::Duration::from_secs(1)) {
            self.recent.pop_front();
        }
        while self.gaps.front().is_some_and(|&(t, _)| at.saturating_sub(t) > GAP_WINDOW) {
            self.gaps.pop_front();
        }
    }

    /// The pointer's reports: how many in the last second, how many gaps
    /// in the last minute (over `GAP`, within a run of motion), the
    /// longest of them, and the usual interval between reports.
    pub fn health(&self) -> olo_protocol::PointerHealth {
        let n = self.reports.len();
        let interval = (1..n).map(|i| self.reports[i].0.saturating_sub(self.reports[i - 1].0)).filter(|d| *d < STALE).min().unwrap_or_default();
        olo_protocol::PointerHealth {
            reports_second: self.recent.len() as u32,
            gaps_minute: self.gaps.len() as u32,
            longest_gap_us: self.gaps.iter().map(|(_, g)| g.as_micros() as u64).max().unwrap_or(0),
            interval_us: interval.as_micros() as u64,
        }
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
            // Past the last report: on at its velocity, a little. The
            // velocity over the last `VELOCITY_SPAN` of reports, not the
            // last two (a 1 kHz mouse's two reports a millisecond apart
            // are noise; this made the drawn steps less even than the
            // real ones, 2026-11-01).
            let Some(&(t0, p0)) = self.reports.iter().rev().skip(1).find(|(t, _)| t1.saturating_sub(*t) >= VELOCITY_SPAN).or_else(|| self.reports.front()) else { return now };
            if t1 <= t0 || t1 - t0 > STALE || t1 - t0 < std::time::Duration::from_millis(2) {
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
