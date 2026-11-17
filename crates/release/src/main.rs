//! olo-release: Olo's releases on GitHub, for an installed Olo's session
//! (crates/session's upgrade.rs asks it, now and then and when the person
//! chooses a release in Deskbar). It changes nothing but what it's asked
//! to install.
//!
//! ```text
//! olo-release latest --prefix PREFIX
//!     the newest release, and whether it's newer than what PREFIX (~/.local)
//!     has, as JSON
//! olo-release install --prefix PREFIX [--retire NAME]… [--from ARCHIVE]
//!     the newest release (or ARCHIVE, a file or a URL, its .sha256 beside
//!     it) downloaded, checked, unpacked and put in place under PREFIX: one
//!     JSON object a line as it goes ({"doing": "downloading", "fraction":
//!     0.42}), then what it installed ({"installed": [programs], "version"})
//!     or why not ({"error"})
//! ```
//!
//! `OLO_RELEASES` is GitHub's API for the repository to ask instead (a
//! test's own server).

use std::{
    io::Write as _,
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use serde_json::json;

const USAGE: &str = "olo-release latest --prefix PREFIX
olo-release install --prefix PREFIX [--retire NAME]... [--from ARCHIVE]";

fn option(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

/// One line of what it says, at once (the session reads it as it comes).
fn say(v: serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(prefix) = option(&args, "--prefix").map(PathBuf::from) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let result = match args.first().map(String::as_str) {
        Some("latest") => latest(&prefix),
        Some("install") => install(&prefix, &args),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        say(json!({ "error": e }));
        std::process::exit(1);
    }
}

/// `latest`: the newest release, and whether to offer it.
fn latest(prefix: &std::path::Path) -> Result<(), String> {
    let r = olo_release::latest()?;
    let installed = olo_release::installed_in(prefix);
    say(json!({
        "version": r.version,
        "tag": r.tag,
        "title": r.title,
        "published": r.published,
        "committed": r.committed,
        "made": r.made(),
        "newer": olo_release::is_newer(&r, &installed),
        "download_bytes": r.download_bytes(&installed),
    }));
    Ok(())
}

/// Says how far it has got, at most every quarter of a second (and each
/// step's start and end).
struct Progress {
    doing: &'static str,
    last: Option<Instant>,
}

impl Progress {
    fn tell(&mut self, doing: &'static str, done: u64, total: u64) {
        let now = Instant::now();
        let fresh = self.doing != doing;
        if !fresh && done < total && self.last.is_some_and(|t| now.duration_since(t) < Duration::from_millis(250)) {
            return;
        }
        self.doing = doing;
        self.last = Some(now);
        let fraction = if total > 0 { (done as f64 / total as f64).min(1.0) } else { 0.0 };
        say(json!({ "doing": doing, "fraction": (fraction * 1000.0).round() / 1000.0 }));
    }
}

/// `install`: the release fetched, checked, unpacked and put in place.
fn install(prefix: &std::path::Path, args: &[String]) -> Result<(), String> {
    let retire: Vec<String> = args.windows(2).filter(|p| p[0] == "--retire").map(|p| p[1].clone()).collect();
    let installed = olo_release::installed_in(prefix);
    // The newest release (its checksum file and commit known: recorded
    // as it's installed), or the archive given.
    let (source, release) = match option(args, "--from") {
        Some(given) => (olo_release::find(Some(&given))?, None),
        None => {
            let r = olo_release::latest()?;
            (r.source.clone(), Some(r))
        }
    };
    let stop = AtomicBool::new(false);
    let mut p = Progress { doing: "", last: None };
    // (Bytes across all the files: the archive, and the parts it hasn't.)
    let total: u64 = release.as_ref().map_or(0, |r| r.download_bytes(&installed));
    let mut before = 0u64;
    let mut current = (0usize, 0u64);
    let archives = olo_release::fetch(&source, &installed.sums, &stop, &mut |file, _, got, of| {
        if file != current.0 {
            before += current.1;
            current = (file, 0);
        }
        current.1 = of;
        let (done, all) = if total > 0 { (before + got, total) } else { (got, of) };
        p.tell("downloading", done, all);
    })?;
    let stage = olo_release::cache_dir().join("upgrade");
    let mut unpacking = Progress { doing: "", last: None };
    let olo = olo_release::unpack(&archives, &stage, &stop, Box::new(move |done, all| unpacking.tell("unpacking", done, all)))?;
    let version = olo_release::version_of(&olo);
    let changed = olo_release::place_prefix(&olo, prefix, &retire, release.as_ref(), &mut |done, all| p.tell("installing", done, all))?;
    // What was downloaded, only kept until it's in place: 2 GB in the
    // cache otherwise.
    let _ = std::fs::remove_dir_all(&stage);
    for a in archives.iter().filter(|a| a.starts_with(olo_release::cache_dir())) {
        let _ = std::fs::remove_file(a);
    }
    say(json!({ "installed": changed, "version": version }));
    Ok(())
}
