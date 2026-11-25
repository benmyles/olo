//! `olo-rex-link sync watch ROOT [--shallow] [--exclude PATTERN]… [--settle MS]`:
//! thin CLI wrapper around [`olo_rex_sync::watch::watch_dir`]. Says `watching`
//! once watches are installed, `changed` once changes settle, `gone` if ROOT
//! disappears, or `error: …`, and exits cleanly when stdin closes.

use std::io::Write;
use std::path::PathBuf;

use olo_rex_sync::index::Filter;
use olo_rex_sync::watch::{DEFAULT_SETTLE_MS, WatchEvent, watch_dir};

const USAGE: &str = "usage: olo-rex-link sync watch ROOT [--shallow] [--exclude PATTERN]… [--settle MS]";

fn say(word: &str) -> bool {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{word}").and_then(|_| out.flush()).is_ok()
}

pub fn run(args: Vec<String>) -> ! {
    let fail = |why: &str| -> ! {
        eprintln!("olo-rex-link sync watch: {why}\n{USAGE}");
        std::process::exit(2)
    };
    let mut it = args.into_iter();
    let Some(root) = it.next() else { fail("say which folder") };
    let root = PathBuf::from(root);
    let root = match std::env::var_os("HOME") {
        Some(home) if root.is_relative() => PathBuf::from(home).join(root),
        _ => root,
    };
    let (mut filter, mut settle) = (Filter::default(), DEFAULT_SETTLE_MS);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--shallow" => filter.recursive = false,
            "--exclude" => filter.exclude.push(it.next().unwrap_or_else(|| fail("--exclude takes a pattern"))),
            "--settle" => settle = it.next().and_then(|m| m.parse().ok()).unwrap_or_else(|| fail("--settle takes milliseconds")),
            other => fail(&format!("{other} isn't one of its options")),
        }
    }
    let mut exit_code = 0;
    watch_dir(&root, filter, settle, 0, |ev| match ev {
        WatchEvent::Watching => say("watching"),
        WatchEvent::Changed => say("changed"),
        WatchEvent::Gone => {
            let _ = say("gone");
            exit_code = 1;
            false
        }
        WatchEvent::Error(why) => {
            let _ = say(&format!("error: {why}"));
            exit_code = 1;
            false
        }
    });
    std::process::exit(exit_code);
}
