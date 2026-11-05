//! `olo-fuzz sweep`: a fuzzer over many seeds at once, each in a session
//! of its own (started and stopped with `tools/dev/tsession`), each
//! fuzzer a thread of this process, as many at once as memory allows
//! (`--jobs`). Seeds start a quarter second apart
//! (sixteen sessions starting in one instant left windows not yet drawn
//! at the first look). `--confirm` runs the failures again, by
//! themselves (all at once: a few sessions are next to no load): one that
//! passes then was the load's, one that fails again is a fault.
//! Each seed's output is in /var/tmp/olo-fuzz/sweep-SEED.txt.
//!
//! It leaves nothing running and nothing in /tmp (memory): each session
//! is stopped and its folder removed however its seed ends (passed,
//! failed, panicked, interrupted), and ends with this process even if
//! it's killed (tsession's `--owner`); the programs it started are
//! waited for. `--keep` keeps a failure's logs, in
//! /var/tmp/olo-fuzz/TAG-SEED/. One sweep at a time (a lock), and one
//! starting stops what a sweep before it left.

use std::{
    os::{fd::AsRawFd as _, unix::process::CommandExt as _},
    path::Path,
    process::{Command, Stdio},
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

use crate::session::{BASE, root};

const OUT: &str = "/var/tmp/olo-fuzz";

fn seeds_of(spec: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for part in spec.split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => out.extend(a.parse::<u64>().unwrap_or(1)..=b.parse::<u64>().unwrap_or(1)),
            None => out.extend(part.parse::<u64>().ok()),
        }
    }
    out
}

/// The build sessions run: release, or debug with `--debug` (slower, and
/// every lock's order checked: olo_sync).
static PROFILE: std::sync::OnceLock<&str> = std::sync::OnceLock::new();

fn tsession(args: &[&str]) -> std::io::Result<std::process::Output> {
    // (Never the person's display: the session's own, or nothing. In a
    // process group of its own: Ctrl-C is this process's to handle, not
    // to cut a session's start or stop in half.)
    Command::new("/usr/bin/python3")
        .arg(root().join("tools/dev/tsession"))
        .args(args)
        .env("TSESSION_PROFILE", PROFILE.get().copied().unwrap_or("release"))
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("OLO_SOCKET")
        .stdin(Stdio::null())
        .process_group(0)
        .output()
}

/// The sweep's session names: `fuzz7`, and `fuzz7-again` for --confirm.
fn session_name(seed: u64, tag: &str) -> String {
    if tag == "sweep" { format!("fuzz{seed}") } else { format!("fuzz{seed}-{tag}") }
}

fn is_ours(name: &str) -> bool {
    name.strip_prefix("fuzz").is_some_and(|rest| {
        let digits = rest.split('-').next().unwrap_or("");
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && matches!(&rest[digits.len()..], "" | "-again")
    })
}

/// A seed's session, stopped however its seed ends (a panic unwinding
/// too): what it started waited for, its logs kept in /var/tmp if asked,
/// its folder in /tmp removed.
struct Running {
    name: String,
    keep_as: Option<std::path::PathBuf>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let dir = Path::new(BASE).join(&self.name);
        let _ = tsession(&["stop", &self.name]);
        crate::session::end_children(&dir);
        if let Some(to) = &self.keep_as {
            let _ = std::fs::remove_dir_all(to);
            let _ = copy_files(&dir, to);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// `from`'s files (logs, settings, what the apps kept), not its sockets.
fn copy_files(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let t = e.file_type()?;
        if t.is_dir() {
            copy_files(&e.path(), &to.join(e.file_name()))?;
        } else if t.is_file() {
            std::fs::copy(e.path(), to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// One seed in a session of its own: whether it passed, and its result
/// line.
fn one(kind: &str, seed: u64, steps: u32, extra: &[String], keep: bool, tag: &str) -> (bool, String) {
    if crate::stopping() {
        return (false, "interrupted before it started".into());
    }
    let name = session_name(seed, tag);
    let _ = tsession(&["stop", &name]);
    let (size, scale) = match kind {
        "wm" => ("2560x1600,1920x1080", "1"),
        "mode" => ("1920x1200,1440x900", "1"),
        _ => ("3024x1890", "1.75"),
    };
    let me = std::process::id().to_string();
    let mut running = Running { name: name.clone(), keep_as: None };
    match tsession(&["start", &name, size, "--scale", scale, "--session", "--shell", "--owner", &me]) {
        Ok(o) if o.status.success() => {}
        Ok(o) => return (false, format!("the session didn't start: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => return (false, format!("tsession: {e}")),
    }
    let path = Path::new(OUT).join(format!("{tag}-{seed}.txt"));
    let t0 = Instant::now();
    let mut args: Vec<String> = vec!["--seed".into(), seed.to_string(), "--steps".into(), steps.to_string(), "--quiet".into()];
    args.extend(extra.iter().cloned());
    let file = match std::fs::File::create(&path) {
        Ok(file) => file,
        Err(e) => return (false, format!("{}: {e}", path.display())),
    };
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::run(kind, &name, &args, Box::new(file))));
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let (passed, line) = match ran {
        Ok(code) => (code == 0, text.lines().find(|l| l.starts_with("FAIL") || l.starts_with("ok:") || l.starts_with("interrupted")).unwrap_or("no result").to_owned()),
        Err(panic) => (false, format!("FAIL the fuzzer panicked: {}", panic.downcast_ref::<String>().map(String::as_str).or_else(|| panic.downcast_ref::<&str>().copied()).unwrap_or("?"))),
    };
    if !passed && keep {
        running.keep_as = Some(Path::new(OUT).join(format!("{tag}-{seed}")));
    }
    drop(running);
    let short: String = line.chars().take(300).collect();
    (passed, format!("{short} ({:.0} s)", t0.elapsed().as_secs_f64()))
}

/// The sweep's lock (/var/tmp/olo-fuzz/sweep.lock), held while it runs;
/// None if another sweep holds it.
fn lock() -> Option<std::fs::File> {
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(Path::new(OUT).join("sweep.lock")).ok()?;
    // SAFETY: flock on a file of ours, released when it closes.
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(file)
}

/// What a sweep before this one left (killed before it stopped them, its
/// watchers too): its sessions stopped, their folders removed.
fn clear_left() {
    let Ok(dirs) = std::fs::read_dir(BASE) else { return };
    for d in dirs.flatten() {
        let name = d.file_name().to_string_lossy().into_owned();
        if is_ours(&name) {
            let _ = tsession(&["stop", &name]);
            let _ = std::fs::remove_dir_all(d.path());
        }
    }
}

/// MemAvailable, in MB.
fn available_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| m.lines().find(|l| l.starts_with("MemAvailable:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|k| k.parse::<u64>().ok()))
        .map_or(8192, |kb| kb / 1024)
}

pub fn main(args: &[String]) -> i32 {
    let (ours, theirs) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    let opt = |name: &str| ours.iter().position(|a| a == name).and_then(|i| ours.get(i + 1)).cloned();
    let Some(kind) = theirs.first().cloned().filter(|k| matches!(k.as_str(), "wm" | "auto" | "mode")) else {
        eprintln!("olo-fuzz sweep: which fuzzer? … -- wm|auto|mode [ARGS…]");
        return 2;
    };
    let extra: Vec<String> = theirs[1..].to_vec();
    // What a session costs, measured (auto, 120 steps, 24 at once): 11 GB
    // of MemAvailable at their peak, about 450 MB each, with the
    // processors 80 % idle (the fuzzers mostly wait for windows to
    // settle). So memory sets how many: 500 MB each, out of what's
    // available less the person's share (a quarter of it, 8 GB at least);
    // and by default a sweep is one wave of that many seeds.
    let jobs: usize = opt("--jobs").and_then(|j| j.parse().ok()).unwrap_or_else(|| {
        let mb = available_mb();
        (mb.saturating_sub((mb / 4).max(8192)) / 500).clamp(1, 96) as usize
    });
    let seeds = seeds_of(&opt("--seeds").unwrap_or_else(|| format!("1-{jobs}")));
    let steps: u32 = opt("--steps").and_then(|s| s.parse().ok()).unwrap_or(120);
    let keep = ours.iter().any(|a| a == "--keep");
    let _ = PROFILE.set(if ours.iter().any(|a| a == "--debug") { "debug" } else { "release" });
    let _ = std::fs::create_dir_all(OUT);
    let Some(_lock) = lock() else {
        eprintln!("olo-fuzz sweep: another sweep is running (it holds {OUT}/sweep.lock)");
        return 2;
    };
    clear_left();
    println!(
        "olo-fuzz sweep: {kind} {}, seeds {}–{} ({}), {steps} steps, {jobs} at once, {} builds",
        extra.join(" "),
        seeds.first().unwrap_or(&0),
        seeds.last().unwrap_or(&0),
        seeds.len(),
        PROFILE.get().copied().unwrap_or("release")
    );
    let run = Run { kind: &kind, steps, extra: &extra, keep };
    let t0 = Instant::now();
    let mut failed = run.all(&seeds, jobs, "sweep", |seed, _, line| println!("  {seed:3}: {line}"));
    println!("{} of {} passed in {:.0} s", seeds.len() - failed.len(), seeds.len(), t0.elapsed().as_secs_f64());
    if crate::stopping() {
        println!("interrupted: every session it started is stopped");
        return 130;
    }
    if !failed.is_empty() && ours.iter().any(|a| a == "--confirm") {
        // (A few sessions at once are next to no load: all at once.)
        failed.sort_unstable();
        println!("again, the failures by themselves: {failed:?}");
        let t1 = Instant::now();
        failed = run.all(&failed, failed.len(), "again", |seed, passed, line| {
            println!("  {seed:3}: {}: {line}", if passed { "the load's (passes by itself)" } else { "a fault" });
        });
        println!("{} faults, confirmed in {:.0} s", failed.len(), t1.elapsed().as_secs_f64());
        if crate::stopping() {
            return 130;
        }
    }
    i32::from(!failed.is_empty())
}

/// A sweep's fuzzer and how it runs each seed.
struct Run<'a> {
    kind: &'a str,
    steps: u32,
    extra: &'a [String],
    keep: bool,
}

impl Run<'_> {
    /// `seeds`, `jobs` at once (each a thread, starting a quarter second
    /// after the one before), `each` told of every result as it comes;
    /// the seeds that failed.
    fn all(&self, seeds: &[u64], jobs: usize, tag: &str, mut each: impl FnMut(u64, bool, &str)) -> Vec<u64> {
        let queue = Mutex::new(seeds.iter().copied().enumerate());
        let (tx, rx) = mpsc::channel();
        let mut failed = Vec::new();
        std::thread::scope(|scope| {
            for _ in 0..jobs.min(seeds.len()) {
                let (queue, tx) = (&queue, tx.clone());
                scope.spawn(move || {
                    loop {
                        let Some((i, seed)) = queue.lock().unwrap_or_else(|e| e.into_inner()).next() else { break };
                        let start = Instant::now() + Duration::from_millis(250 * (i % jobs) as u64);
                        while Instant::now() < start && !crate::stopping() {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        if crate::stopping() {
                            break;
                        }
                        let _ = tx.send((seed, one(self.kind, seed, self.steps, self.extra, self.keep, tag)));
                    }
                });
            }
            drop(tx);
            for (seed, (passed, line)) in rx {
                each(seed, passed, &line);
                if !passed {
                    failed.push(seed);
                }
            }
        });
        failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clears_its_own_sessions_and_no_others() {
        assert_eq!(session_name(7, "sweep"), "fuzz7");
        assert_eq!(session_name(7, "again"), "fuzz7-again");
        for name in ["fuzz7", "fuzz12-again", &session_name(3, "again")] {
            assert!(is_ours(name), "{name}");
        }
        for name in ["fuzz", "fuzzy", "fuzz7-other", "fuzz-again", "sweep7", "fz", "chrome"] {
            assert!(!is_ours(name), "{name}");
        }
    }
}
