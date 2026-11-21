//! What a picker's live thumbnails cost, for measuring (`thumbnails::Live`):
//!
//!   live SECONDS [--still] WINDOW…
//!
//! Pictures of the windows named (app_server's ids, as `list_windows`
//! gives them), as a chooser shows them (400 by 250 pixels, each at most
//! once a second), for SECONDS; with `--still`, told halfway that nobody
//! looks any more. Says, for each half: the pictures made, and what the
//! worker's thread took of a processor and how often it woke. Run it in
//! a test desktop (`tsession run NAME …/examples/live 20 --still 3`)
//! with a window whose content keeps changing.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use olo_capture::{
    Source,
    thumbnails::{Item, Live},
};

/// The worker's thread so far: its time on the processors (ms) and how
/// often it woke of its own accord.
fn worker() -> Option<(f64, u64)> {
    for task in std::fs::read_dir("/proc/self/task").ok()?.flatten() {
        let dir = task.path();
        if std::fs::read_to_string(dir.join("comm")).is_ok_and(|c| c.trim() == "thumbnails") {
            let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
            let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_ascii_whitespace().collect();
            let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
            // SAFETY: sysconf with a name of the C library's own.
            let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
            let status = std::fs::read_to_string(dir.join("status")).ok()?;
            let woke = status.lines().find_map(|l| l.strip_prefix("voluntary_ctxt_switches:"))?.trim().parse().ok()?;
            return Some((ticks as f64 * 1000.0 / hz, woke));
        }
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: f64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let still = args.iter().any(|a| a == "--still");
    let items: Vec<Item> = args
        .iter()
        .skip(1)
        .filter_map(|a| a.parse::<u64>().ok())
        .map(|id| Item { key: id.to_string(), source: Source::Window { id, app_id: String::new(), title: String::new() }, every: Duration::from_secs(1), size: (400, 250) })
        .collect();
    if items.is_empty() {
        return eprintln!("live SECONDS [--still] WINDOW…");
    }
    let (made, count) = (Arc::new(AtomicUsize::new(0)), items.len());
    let live = {
        let made = made.clone();
        Live::start(items, move |_| {
            made.fetch_add(1, Ordering::Relaxed);
        })
    };
    // (The first pictures, and the worker's own start, aside.)
    std::thread::sleep(Duration::from_secs(2));
    let halves: &[&str] = if still { &["live", "nobody looking"] } else { &["live"] };
    for (i, what) in halves.iter().enumerate() {
        if i == 1 {
            live.set_live(false);
            std::thread::sleep(Duration::from_millis(500));
        }
        let (began, before, pictures) = (Instant::now(), worker(), made.load(Ordering::Relaxed));
        std::thread::sleep(Duration::from_secs_f64(seconds / halves.len() as f64));
        let (took, after) = (began.elapsed().as_secs_f64(), worker());
        let pictures = made.load(Ordering::Relaxed) - pictures;
        match (before, after) {
            (Some((ms0, woke0)), Some((ms1, woke1))) => println!(
                "{what}, {count} windows, {took:.1} s: {:.1} pictures a second; the worker {:.2}% of a processor, {:.1} wake-ups a second",
                pictures as f64 / took,
                (ms1 - ms0) / 10.0 / took,
                (woke1 - woke0) as f64 / took
            ),
            _ => println!("{what}: {pictures} pictures in {took:.1} s (the worker's thread wasn't found)"),
        }
    }
    live.stop();
}
