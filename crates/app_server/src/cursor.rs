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
/// A gap between reports, while moving, is one worth counting (a 2.4
/// GHz receiver dropping packets, a receiver asleep) when it is longer
/// than this and half as long again as the device's own usual interval
/// (`Track::usual`): a 1 kHz mouse's missing 5 ms, a touchpad's (about
/// 7 ms between its reports, give or take one) a report missed. 5 ms alone counted every report of the P1's
/// touchpad as a gap, 1,100 a minute, 2026-11-17.
const GAP: std::time::Duration = std::time::Duration::from_millis(5);
/// Intervals needed before the usual one is known (a run's first few
/// reports judge nothing).
const USUAL_FROM: usize = 4;
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
            // (The usual interval only asked for past `GAP`: a mouse's
            // every report isn't.)
            if gap < STALE
                && gap > GAP
                && let Some(usual) = self.usual()
                && gap > usual * 3 / 2
            {
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

    /// The usual interval between the reports kept, those of a run of
    /// motion (under `STALE`): their median, once there are `USUAL_FROM`.
    /// Whichever device is moving it sets it (one at a time, as hands go).
    fn usual(&self) -> Option<std::time::Duration> {
        let mut iv = [std::time::Duration::ZERO; KEPT];
        let mut n = 0;
        for (a, b) in self.reports.iter().zip(self.reports.iter().skip(1)) {
            let d = b.0.saturating_sub(a.0);
            if d < STALE {
                iv[n] = d;
                n += 1;
            }
        }
        if n < USUAL_FROM {
            return None;
        }
        let iv = &mut iv[..n];
        iv.sort_unstable();
        Some(iv[n / 2])
    }

    /// The pointer's reports: how many in the last second, how many gaps
    /// in the last minute (reports gone missing, `GAP`), the longest of
    /// them, and the usual interval between reports.
    pub fn health(&self) -> olo_protocol::PointerHealth {
        let interval = self.usual().unwrap_or_default();
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

    /// Reports gone missing are counted against the device's own pace:
    /// a touchpad's every report 7 ms apart is none, a mouse's 1 ms
    /// reports gone for 6 ms is one, the touchpad's missing one is one,
    /// and a hand's pause (past `STALE`) is none.
    #[test]
    fn gaps_are_reports_gone_missing_at_the_device_s_own_pace() {
        let us = Duration::from_micros;
        let mut pad = Track::default();
        for i in 0..200u64 {
            pad.note(us(7_000 * i + (i % 3) * 400), Point::from((i as f64, 0.0)));
        }
        assert_eq!(pad.health().gaps_minute, 0, "a touchpad's own pace");
        // (7.4, 7.4 and 6.2 ms apart, over and over.)
        assert_eq!(pad.health().interval_us, 7_400, "its usual interval");
        pad.note(us(7_000 * 199 + 14_000), Point::from((0.0, 0.0)));
        assert_eq!(pad.health().gaps_minute, 1, "one report missed");
        pad.note(us(7_000 * 199 + 14_000 + 300_000), Point::from((0.0, 0.0)));
        assert_eq!(pad.health().gaps_minute, 1, "a pause");

        let mut mouse = Track::default();
        for i in 0..100u64 {
            mouse.note(us(1_000 * i), Point::from((i as f64, 0.0)));
        }
        mouse.note(us(1_000 * 99 + 4_000), Point::from((0.0, 0.0)));
        assert_eq!(mouse.health().gaps_minute, 0, "4 ms is under GAP");
        mouse.note(us(1_000 * 99 + 4_000 + 6_000), Point::from((0.0, 0.0)));
        assert_eq!(mouse.health().gaps_minute, 1, "a 1 kHz mouse's 6 ms");
        // A run's first reports judge nothing.
        let mut new = Track::default();
        new.note(us(0), Point::from((0.0, 0.0)));
        new.note(us(20_000), Point::from((0.0, 0.0)));
        assert_eq!(new.health().gaps_minute, 0);
    }

    /// What a report costs to note (release: `cargo test --release -p
    /// olo-app-server note_costs -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn note_costs() {
        let mut t = Track::default();
        let n = 2_000_000u64;
        let start = std::time::Instant::now();
        for i in 0..n {
            // A 1 kHz mouse, a report missing every 64.
            let at = Duration::from_micros(1_000 * i + if i % 64 == 0 { 3_000 } else { 0 });
            t.note(at, Point::from((i as f64, 0.0)));
        }
        let each = start.elapsed().as_nanos() as f64 / n as f64;
        println!("note: {each:.1} ns a mouse's report; {} gaps", t.health().gaps_minute);
        // A touchpad: 7 ms apart, each past `GAP`, so each asks the usual.
        let mut t = Track::default();
        let start = std::time::Instant::now();
        for i in 0..n {
            t.note(Duration::from_micros(7_000 * i + (i % 3) * 400), Point::from((i as f64, 0.0)));
        }
        let each = start.elapsed().as_nanos() as f64 / n as f64;
        println!("note: {each:.1} ns a touchpad's report; {} gaps", t.health().gaps_minute);
    }
}
