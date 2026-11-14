//! How a window keeps up: its frames' times, and the work each did.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// How a window kept up, for agents and tools (the kit's
/// `{"frames":true}`): frames drawn since last taken, their total and
/// slowest time from starting to draw to handing the frame to the GPU,
/// and the last [`FrameStats::MARKS`] of them.
#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    /// Frames drawn.
    pub frames: u32,
    /// Their time together, in microseconds.
    pub total_us: u64,
    /// The slowest one's.
    pub slowest_us: u64,
    /// Each frame: when it started (CLOCK_MONOTONIC microseconds), how
    /// long laying out and painting took, and the whole frame.
    pub marks: VecDeque<(u64, u32, u32)>,
    /// Each mark's scrollers, as the display thread drew them in it, in
    /// the order they're drawn, then what scrolls itself
    /// (`Painter::note_scroll`): how far each was scrolled (across and
    /// down: zero at its start, up to how far it goes; past either end,
    /// its content pulled past its edge) and the furthest it goes (x,
    /// y). What `tools/dev/scroll-check` and the benchmark judge a
    /// scroll by.
    pub scrolls: VecDeque<Vec<[f32; 4]>>,
}

impl FrameStats {
    /// How many frames' marks are kept between takes.
    pub const MARKS: usize = 1200;

    pub(crate) fn record(&mut self, started: Instant, drawn: Duration, whole: Duration, scrolls: Vec<[f32; 4]>) {
        let us = whole.as_micros() as u64;
        self.frames += 1;
        self.total_us += us;
        self.slowest_us = self.slowest_us.max(us);
        // (A ring: a window nobody asks about drops its oldest mark with
        // every frame, without moving the rest.)
        if self.marks.len() == Self::MARKS {
            self.marks.pop_front();
            self.scrolls.pop_front();
        }
        let at = monotonic_us().saturating_sub(started.elapsed().as_micros() as u64);
        self.marks.push_back((at, drawn.as_micros() as u32, us as u32));
        self.scrolls.push_back(scrolls);
    }
}

pub(crate) fn monotonic_us() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid timespec for the kernel to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000
}

/// what a frame did that a frame's phases don't name (`OLO_FRAME_PHASES`):
/// lines shaped and glyphs drawn into the atlas, how many and how long.
pub(crate) struct FrameWork {
    pub shaped: [std::sync::atomic::AtomicU64; 2],
    pub rasterized: [std::sync::atomic::AtomicU64; 2],
    /// Lines' sprites found afresh (`paint_glyph_run`).
    pub runs: [std::sync::atomic::AtomicU64; 2],
}

pub(crate) static FRAME_WORK: FrameWork = FrameWork {
    shaped: [std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0)],
    rasterized: [std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0)],
    runs: [std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0)],
};

impl FrameWork {
    pub(crate) fn note(&self, what: &[std::sync::atomic::AtomicU64; 2], since: std::time::Instant) {
        use std::sync::atomic::Ordering::Relaxed;
        what[0].fetch_add(1, Relaxed);
        what[1].fetch_add(since.elapsed().as_nanos() as u64, Relaxed);
    }

    /// (how many, ms) of each, since last taken.
    pub(crate) fn take(&self) -> [(u64, f64); 3] {
        use std::sync::atomic::Ordering::Relaxed;
        let t = |w: &[std::sync::atomic::AtomicU64; 2]| (w[0].swap(0, Relaxed), w[1].swap(0, Relaxed) as f64 / 1e6);
        [t(&self.shaped), t(&self.rasterized), t(&self.runs)]
    }
}

/// Each layout's tree on stderr (`OLO_DUMP_LAYOUT=1`): where every node
/// was put and what its style asked for.
pub(crate) fn dump_layout() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_DUMP_LAYOUT").is_some())
}

/// each frame's time in its phases and what its scene holds, on
/// stderr (`OLO_FRAME_PHASES=1`).
pub(crate) fn frame_phases() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_FRAME_PHASES").is_some())
}
