//! The watchdog's judgements, each from several signals rather than one
//! number, with the evidence it rests on and how sure it is:
//!
//! - **Spin**: busy (`spin_cpu` of a core) steadily for `spin_minutes`
//!   (every part of the time, not on average), and, adding to it, it
//!   never waits on anything (few voluntary context switches), does no
//!   I/O, its memory doesn't move, and it has gone on long. Background
//!   work (a compiler is busy and quiet by nature) isn't judged so.
//! - **Leak**: its memory (anonymous resident and swapped) grows over
//!   `leak_minutes` by `leak_growth_mb` at `leak_rate_mb` a minute, a
//!   straight line fitting it (R² ≥ `leak_fit`, not one jump), mostly
//!   rising step by step; urgent if at that rate it takes what memory is
//!   left within half an hour.
//! - **Memory**: it holds `memory_share` of RAM while the system is short
//!   (less than `memory_available` free, or tasks stalling on memory
//!   `memory_stall` of the time, or swapping), more so as these agree;
//!   urgent at half that free, or twice the stalls (or all tasks
//!   stalling).
//! - **Zombies**: a parent leaving at least `zombie_least` exited
//!   children unreaped for `zombie_minutes` (fewer: one that reaps
//!   lazily, left alone), surer the more and the longer; urgent past
//!   `zombie_count` of them.
//!
//! Pure functions of samples, for tests; `sample.rs` gathers them.

use std::collections::VecDeque;

use olo_protocol::performance::{FindingKind, Thresholds};

/// One look at a process.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sample {
    /// When (seconds, any clock).
    pub t: f64,
    /// CPU time used so far (seconds).
    pub cpu: f64,
    /// Voluntary context switches so far (its threads').
    pub switches: u64,
    /// Bytes read and written so far (`rchar` + `wchar`).
    pub io: u64,
    /// Anonymous memory, resident and swapped (bytes).
    pub memory: u64,
}

/// The system's memory, as last looked at.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct System {
    pub total: u64,
    pub available: u64,
    /// Tasks stalled on memory (% of the last 10 s), some and all.
    pub stall_some: f32,
    pub stall_full: f32,
    /// Pages swapped out a second.
    pub swapping: f32,
}

impl System {
    pub fn available_pct(&self) -> f32 {
        if self.total == 0 { 100.0 } else { self.available as f32 * 100.0 / self.total as f32 }
    }
}

/// A judgement: how sure, whether urgent, a line, and its evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Judged {
    pub kind: FindingKind,
    pub confidence: f32,
    pub urgent: bool,
    pub summary: String,
    pub evidence: Vec<String>,
}

const MB: f64 = 1024.0 * 1024.0;

fn mb(bytes: f64) -> String {
    if bytes >= 1024.0 * MB { format!("{:.1} GB", bytes / 1024.0 / MB) } else { format!("{:.0} MB", bytes / MB) }
}

fn minutes(secs: f64) -> String {
    if secs >= 3600.0 { format!("{:.0} h {:.0} min", (secs / 3600.0).floor(), (secs % 3600.0) / 60.0) } else { format!("{:.0} min", (secs / 60.0).max(1.0)) }
}

/// The samples covering the last `span` seconds (from one at or before
/// its start), if they reach back that far.
fn window(samples: &VecDeque<Sample>, span: f64) -> Option<Vec<Sample>> {
    let last = samples.back()?;
    let from = last.t - span;
    let start = samples.iter().rposition(|s| s.t <= from)?;
    Some(samples.iter().skip(start).copied().collect())
}

/// Busy without waiting on anything: `fine` are recent samples, a few
/// seconds apart.
pub fn spin(fine: &VecDeque<Sample>, th: &Thresholds) -> Option<Judged> {
    let span = f64::from(th.spin_minutes.max(0.5)) * 60.0;
    let w = window(fine, span)?;
    let (a, b) = (w.first()?, w.last()?);
    let dt = b.t - a.t;
    if dt <= 0.0 {
        return None;
    }
    let cpu = (b.cpu - a.cpu) / dt * 100.0;
    let need = f64::from(th.spin_cpu);
    if cpu < need {
        return None;
    }
    // Every part of the time, not on average: six slices, each busy.
    let slices = 6;
    for k in 0..slices {
        let (t0, t1) = (a.t + dt * k as f64 / slices as f64, a.t + dt * (k + 1) as f64 / slices as f64);
        let s0 = w.iter().rev().find(|s| s.t <= t0).unwrap_or(a);
        let s1 = w.iter().find(|s| s.t >= t1).unwrap_or(b);
        if s1.t > s0.t && (s1.cpu - s0.cpu) / (s1.t - s0.t) * 100.0 < need * 0.8 {
            return None;
        }
    }
    let switches = (b.switches.saturating_sub(a.switches)) as f64 / dt;
    let io_kb = (b.io.saturating_sub(a.io)) as f64 / dt / 1024.0;
    let moved = (b.memory as f64 - a.memory as f64).abs();
    let (sw, iw) = (f64::from(th.spin_switches), f64::from(th.spin_io_kb));
    let s_switch = if switches <= sw { 1.0 } else if switches <= sw * 5.0 { 0.4 } else { 0.0 };
    let s_io = if io_kb <= iw { 1.0 } else if io_kb <= iw * 10.0 { 0.4 } else { 0.0 };
    let s_mem = if moved < 2.0 * MB { 1.0 } else { 0.4 };
    // How long it has been so (from the whole history kept).
    let longest = fine.back().map_or(dt, |l| {
        let mut since = l.t;
        for (x, y) in fine.iter().rev().skip(1).zip(fine.iter().rev()) {
            if y.t > x.t && (y.cpu - x.cpu) / (y.t - x.t) * 100.0 >= need * 0.8 {
                since = x.t;
            } else {
                break;
            }
        }
        l.t - since
    });
    let s_long = (longest / (2.0 * span)).min(1.0);
    let confidence = (0.3 + 0.25 * s_switch + 0.25 * s_io + 0.1 * s_mem + 0.1 * s_long) as f32;
    if confidence < 0.6 {
        return None;
    }
    let evidence = vec![
        format!("Busy: {cpu:.0}% of a core, steadily, for {}", minutes(longest)),
        format!("Waits on nothing: {switches:.1} voluntary switches a second (a working program waits often)"),
        format!("No input or output: {io_kb:.1} KB a second read or written"),
        if moved < 2.0 * MB { format!("Memory unchanged at {}", mb(b.memory as f64)) } else { format!("Memory moved by {}", mb(moved)) },
    ];
    Some(Judged {
        kind: FindingKind::Spin,
        confidence,
        urgent: confidence >= 0.8,
        summary: format!("Busy for {} without waiting on anything or doing any input or output: it may be stuck in a loop.", minutes(longest)),
        evidence,
    })
}

/// A straight line through `(x, y)`: its slope and R².
fn fit(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let n = points.len() as f64;
    if n < 3.0 {
        return None;
    }
    let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
    let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let syy: f64 = points.iter().map(|p| (p.1 - my).powi(2)).sum();
    if sxx == 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let r2 = if syy == 0.0 { 0.0 } else { (sxy * sxy) / (sxx * syy) };
    Some((slope, r2))
}

/// Memory growing steadily: `coarse` are its memory a minute or so apart.
pub fn leak(coarse: &VecDeque<(f64, u64)>, system: &System, th: &Thresholds) -> Option<Judged> {
    let span = f64::from(th.leak_minutes.max(5.0)) * 60.0;
    let last = coarse.back()?;
    let start = coarse.iter().rposition(|s| s.0 <= last.0 - span)?;
    let pts: Vec<(f64, f64)> = coarse.iter().skip(start).map(|(t, m)| ((t - last.0) / 60.0, *m as f64 / MB)).collect();
    let (slope, r2) = fit(&pts)?;
    let growth = pts.last()?.1 - pts.first()?.1;
    if slope < f64::from(th.leak_rate_mb) || growth < f64::from(th.leak_growth_mb) || r2 < f64::from(th.leak_fit) {
        return None;
    }
    let steps = pts.windows(2).filter(|w| w[1].1 >= w[0].1 - 1.0).count() as f64 / (pts.len() - 1) as f64;
    if steps < 0.6 {
        return None;
    }
    let fitness = ((r2 - f64::from(th.leak_fit)) / (1.0 - f64::from(th.leak_fit)).max(0.01)).clamp(0.0, 1.0);
    let size = (growth / (2.0 * f64::from(th.leak_growth_mb)) - 0.5).clamp(0.0, 1.0);
    let confidence = (0.4 + 0.3 * fitness + 0.2 * size + 0.1 * steps) as f32;
    // At this rate, how long until what's left is gone.
    let left_mb = system.available as f64 / MB;
    let until = if slope > 0.0 { left_mb / slope } else { f64::INFINITY };
    let mut evidence = vec![
        format!("Grew {} in {}, {slope:.1} MB a minute", mb(growth * MB), minutes((pts.last()?.0 - pts.first()?.0) * 60.0)),
        format!("Steadily: a straight line fits it (R² {r2:.2}), rising at {:.0}% of the minutes", steps * 100.0),
        format!("Now {}", mb(last.1 as f64)),
    ];
    if until.is_finite() {
        evidence.push(format!("At this rate the memory left ({}) is gone in {}", mb(system.available as f64), minutes(until * 60.0)));
    }
    Some(Judged {
        kind: FindingKind::Leak,
        confidence,
        urgent: until < 30.0 && confidence >= 0.7,
        summary: format!("Its memory has grown steadily by {} over {}: it may be leaking.", mb(growth * MB), minutes(span)),
        evidence,
    })
}

/// So much memory the system is short of it: `memory` is the process's,
/// `growing` whether it rises lately.
pub fn memory(memory: u64, growing: bool, system: &System, th: &Thresholds) -> Option<Judged> {
    if system.total == 0 {
        return None;
    }
    let share = memory as f32 * 100.0 / system.total as f32;
    if share < th.memory_share {
        return None;
    }
    let avail = system.available_pct();
    let short = avail <= th.memory_available;
    let stalling = system.stall_some >= th.memory_stall;
    let swapping = system.swapping >= 256.0;
    if !(short || stalling || swapping) {
        return None;
    }
    let s = |b: bool| if b { 1.0 } else { 0.0 };
    let confidence = 0.4 + 0.25 * s(short) + 0.2 * s(stalling) + 0.1 * s(swapping) + 0.05 * s(growing);
    let mut evidence = vec![format!("Holds {share:.0}% of the memory ({})", mb(memory as f64))];
    evidence.push(format!("{avail:.0}% of the memory is available{}", if short { " (short)" } else { "" }));
    if stalling {
        evidence.push(format!("Tasks waited on memory {:.0}% of the last 10 s", system.stall_some));
    }
    if swapping {
        evidence.push(format!("Swapping out {:.0} pages a second", system.swapping));
    }
    if growing {
        evidence.push("And it's still growing".into());
    }
    Some(Judged {
        kind: FindingKind::Memory,
        confidence,
        urgent: avail <= th.memory_available / 2.0 || system.stall_full >= th.memory_stall || system.stall_some >= 2.0 * th.memory_stall,
        summary: format!("Holds {share:.0}% of the memory while the system is short of it: everything slows down, and the system may stop programs to free some."),
        evidence,
    })
}

/// A parent leaving `count` zombies, the oldest unreaped for `oldest`
/// seconds.
pub fn zombies(count: usize, oldest: f64, th: &Thresholds) -> Option<Judged> {
    let wait = f64::from(th.zombie_minutes.max(0.5)) * 60.0;
    let least = (th.zombie_least as usize).max(1);
    if count < least || oldest < wait {
        return None;
    }
    // Surer the more of them past the least, and the longer.
    let more = ((count - least) as f64 / least as f64).min(1.0);
    let longer = ((oldest - wait) / (2.0 * wait)).min(1.0);
    let confidence = (0.55 + 0.25 * more + 0.15 * longer).min(0.95);
    Some(Judged {
        kind: FindingKind::Zombies,
        confidence: confidence as f32,
        urgent: count >= th.zombie_count as usize,
        summary: format!("Leaves {count} exited children unreaped (zombies), the oldest for {}.", minutes(oldest)),
        evidence: vec![format!("{count} zombie children"), format!("The oldest unreaped for {}", minutes(oldest)), "A zombie holds only its process slot, but many fill the table".into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(f: impl Fn(f64) -> Sample, secs: u32, every: f64) -> VecDeque<Sample> {
        (0..=(f64::from(secs) / every) as u32).map(|i| f(f64::from(i) * every)).collect()
    }

    #[test]
    fn a_spin_from_several_signals() {
        let th = Thresholds::default();
        // A loop: a core's worth, no switches, no I/O, memory flat.
        let stuck = run(|t| Sample { t, cpu: t * 0.99, switches: 3, io: 100, memory: 50 << 20 }, 400, 2.0);
        let j = spin(&stuck, &th).expect("not found");
        assert!(j.urgent && j.confidence >= 0.8, "{j:?}");
        assert!(j.evidence.iter().any(|e| e.contains("Waits on nothing")));
        // As busy, but waiting and reading all the time: working.
        let working = run(|t| Sample { t, cpu: t * 0.99, switches: (t * 400.0) as u64, io: (t * 2e6) as u64, memory: 50 << 20 }, 400, 2.0);
        assert!(spin(&working, &th).is_none());
        // Busy only half the time: no.
        let bursty = run(|t| Sample { t, cpu: if t < 200.0 { t } else { 200.0 }, switches: 0, io: 0, memory: 0 }, 400, 2.0);
        assert!(spin(&bursty, &th).is_none());
        // Not yet for long enough.
        let short = run(|t| Sample { t, cpu: t, switches: 0, io: 0, memory: 0 }, 60, 2.0);
        assert!(spin(&short, &th).is_none());
    }

    #[test]
    fn a_leak_is_a_steady_climb_not_a_jump() {
        let th = Thresholds::default();
        let sys = System { total: 16 << 30, available: 8 << 30, ..Default::default() };
        let climb: VecDeque<(f64, u64)> = (0..=40).map(|m| (f64::from(m) * 60.0, ((200 + m * 12) as u64) << 20)).collect();
        let j = leak(&climb, &sys, &th).expect("not found");
        assert!(j.confidence >= 0.7 && !j.urgent, "{j:?}");
        // One jump, then flat: not a leak.
        let jump: VecDeque<(f64, u64)> = (0..=40).map(|m| (f64::from(m) * 60.0, if m < 20 { 200u64 << 20 } else { 700u64 << 20 })).collect();
        assert!(leak(&jump, &sys, &th).is_none());
        // Up and down (a cache): no.
        let saw: VecDeque<(f64, u64)> = (0..=40).map(|m| (f64::from(m) * 60.0, ((200 + (m % 10) * 60) as u64) << 20)).collect();
        assert!(leak(&saw, &sys, &th).is_none());
        // Climbing into what's left within half an hour: urgent.
        let low = System { total: 16 << 30, available: 200 << 20, ..Default::default() };
        assert!(leak(&climb, &low, &th).unwrap().urgent);
    }

    #[test]
    fn memory_only_when_the_system_is_short() {
        let th = Thresholds::default();
        let plenty = System { total: 16 << 30, available: 8 << 30, ..Default::default() };
        assert!(memory(6 << 30, false, &plenty, &th).is_none());
        let short = System { total: 16 << 30, available: 1 << 30, stall_some: 20.0, ..Default::default() };
        let j = memory(6 << 30, true, &short, &th).unwrap();
        assert!(j.confidence > 0.8 && j.urgent, "{j:?}");
        assert!(memory(1 << 30, true, &short, &th).is_none(), "a small one isn't the cause");
    }

    #[test]
    fn zombies_left_long() {
        let th = Thresholds::default();
        assert!(zombies(3, 60.0, &th).is_none());
        assert!(!zombies(3, 600.0, &th).unwrap().urgent);
        assert!(zombies(25, 600.0, &th).unwrap().urgent);
        // One or two, however long: a parent reaping lazily (Chrome's).
        assert!(zombies(1, 3600.0, &th).is_none() && zombies(2, 3600.0, &th).is_none());
        // Surer the more and the longer.
        let (few, many) = (zombies(3, 301.0, &th).unwrap(), zombies(8, 1800.0, &th).unwrap());
        assert!(few.confidence < 0.6 && many.confidence > 0.9, "{few:?} {many:?}");
    }
}
