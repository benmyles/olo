//! olo-fuzz: Olo's window-management fuzzers, against headless sessions
//! (`tools/dev/tsession`), and a sweep running many seeds at once. See
//! docs/HACKING.md, "The fuzzers".
//!
//! ```text
//! olo-fuzz wm SESSION   [--seed N] [--steps N] [--only OPS] [--keep-going] [--quiet] [--tiles] [--trace-tiles] [--trail STEPS]
//! olo-fuzz auto SESSION [--seed N] [--steps N] [--only OPS] [--keep-going] [--quiet] [--reloads]
//! olo-fuzz mode SESSION [--seed N] [--steps N] [--only OPS] [--keep-going] [--quiet]
//! olo-fuzz mode SESSION --matrix [--chrome] [--only WORDS]
//! olo-fuzz sweep [--seeds A-B,C] [--jobs N] [--steps N] [--confirm] [--keep] [--debug] -- wm|auto|mode [ARGS…]
//! ```
//!
//! `wm` needs a session with two or more screens and the shell
//! (`tsession start fz 2560x1600,1920x1080 --session --shell`); `auto`
//! one with the shell (`tsession start af 3024x1890 --scale 1.75
//! --session --shell`); `mode` one with the shell, two displays finding
//! more (`tsession start mz 1920x1200,1440x900 --session --shell`). A run is reproducible from its seed (printed
//! first) as long as the session starts the same way. `sweep` starts a
//! session for each seed itself, runs them at once (as many as memory
//! allows), and with `--confirm` runs each failure again alone; it stops
//! every session it started, also when interrupted or killed.

mod auto;
mod fuzz;
mod mode;
mod rng;
mod session;
mod sweep;
mod wm;

use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
};

/// Interrupted (Ctrl-C, SIGTERM, SIGHUP): fuzzers stop after their step,
/// a sweep starts no more seeds and stops the sessions it started. A
/// second one ends this process at once (the sessions of a sweep are
/// still stopped: tsession's `--owner`).
static STOPPING: AtomicBool = AtomicBool::new(false);

pub fn stopping() -> bool {
    STOPPING.load(Ordering::Relaxed)
}

extern "C" fn on_signal(_: libc::c_int) {
    STOPPING.store(true, Ordering::Relaxed);
}

fn catch_signals() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: a handler that only stores to an atomic; SA_RESETHAND
        // gives the next one its default (ending the process).
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESETHAND | libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// Run fuzzer `kind` on `session`; its exit code. `out`: where it says
/// what it does.
pub fn run(kind: &str, session: &str, args: &[String], out: Box<dyn std::io::Write + Send>) -> i32 {
    let seed: u64 = opt(args, "--seed").and_then(|s| s.parse().ok()).unwrap_or_else(|| {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        (t.as_nanos() as u64 ^ std::process::id() as u64) % (1 << 30)
    });
    let steps: u32 = opt(args, "--steps").and_then(|s| s.parse().ok()).unwrap_or(if kind == "auto" { 120 } else { 200 });
    let mut f = match fuzz::Fuzz::new(session, seed, args.iter().any(|a| a == "--quiet"), out) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("olo-fuzz: {e}");
            return 2;
        }
    };
    f.keep_going = args.iter().any(|a| a == "--keep-going");
    f.tiles_fail = args.iter().any(|a| a == "--tiles");
    f.trace_tiles = args.iter().any(|a| a == "--trace-tiles").then(Default::default);
    f.reloads = args.iter().any(|a| a == "--reloads");
    if let Some(only) = opt(args, "--only") {
        f.only = only.split(',').map(str::to_owned).collect();
        let known: HashSet<&str> = match kind {
            "auto" => auto::OPS.into_iter().collect(),
            "mode" => mode::OPS.iter().map(|(n, _, _)| *n).collect(),
            _ => wm::OPS.iter().map(|(n, _, _)| *n).collect(),
        };
        let unknown: Vec<&String> = f.only.iter().filter(|o| !known.contains(o.as_str())).collect();
        // (The matrix's --only is words of its cases' names.)
        if !unknown.is_empty() && !args.iter().any(|a| a == "--matrix") {
            eprintln!("olo-fuzz: no such step: {unknown:?}");
            return 2;
        }
    }
    let trails: HashSet<u32> = opt(args, "--trail").map(|t| t.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
    match kind {
        "auto" => {
            if let Err(e) = auto::start(&mut f) {
                eprintln!("olo-fuzz: {}", e.0);
                return 2;
            }
            f.run(steps, "auto-fuzz", &trails, auto::step)
        }
        "mode" if args.iter().any(|a| a == "--matrix") => mode::matrix(&mut f, args.iter().any(|a| a == "--chrome")),
        "mode" => f.run(steps, "mode-fuzz", &trails, mode::step),
        _ => f.run(steps, "wm-fuzz", &trails, wm::step),
    }
}

fn main() {
    olo_crash::install();
    catch_signals();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("wm" | "auto" | "mode") if args.len() >= 2 && !args[1].starts_with('-') => run(&args[0], &args[1], &args[2..], Box::new(std::io::stdout())),
        Some("sweep") => sweep::main(&args[1..]),
        _ => {
            eprintln!(
                "usage: olo-fuzz wm|auto|mode SESSION [--seed N] [--steps N] [--only OPS] [--keep-going] [--quiet] [--reloads] [--tiles] [--trace-tiles] [--trail STEPS]\n       olo-fuzz mode SESSION --matrix [--chrome] [--only WORDS]\n       olo-fuzz sweep [--seeds A-B,C] [--jobs N] [--steps N] [--confirm] [--keep] [--debug] -- wm|auto|mode [ARGS…]"
            );
            2
        }
    };
    session::reap();
    std::process::exit(if stopping() && code != 2 { 130 } else { code });
}
