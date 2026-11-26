//! olo-release: Olo's releases on GitHub, for an installed Olo's session
//! (crates/session's upgrade.rs asks it, now and then and when the person
//! chooses a release in Deskbar). It changes nothing but what it's asked
//! to install.
//!
//! ```text
//! olo-release latest --prefix PREFIX
//!     the newest release, and whether it's newer than what PREFIX (~/.local)
//!     has, as JSON
//! olo-release list --prefix PREFIX
//!     Olo's releases, the newest first, and which is installed, as JSON
//!     ({"releases": [{"tag", "title", "version", "published", "installed"}],
//!     "installed": VERSION})
//! olo-release install --prefix PREFIX [--retire NAME]… [--without PART]… [--from ARCHIVE | --tag TAG]
//!     the newest release (or the one tagged TAG, an older one too; or
//!     ARCHIVE, a file or a URL, its .sha256 beside it; without the
//!     parts whose files' names begin PART: `olo-wallpapers-`) downloaded,
//!     checked, unpacked and put in place under PREFIX: one
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
olo-release list --prefix PREFIX
olo-release install --prefix PREFIX [--retire NAME]... [--without PART]... [--from ARCHIVE | --tag TAG]";

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
        Some("list") => list(&prefix),
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

/// `list`: the releases, and which is installed (About Olo's other
/// releases).
fn list(prefix: &std::path::Path) -> Result<(), String> {
    let all = olo_release::list()?;
    let installed = olo_release::installed_in(prefix);
    let releases: Vec<serde_json::Value> =
        all.iter().map(|r| json!({ "tag": r.tag, "title": r.title, "version": r.version, "published": r.published, "installed": installed.version.as_deref() == Some(r.version.as_str()) })).collect();
    say(json!({ "releases": releases, "installed": installed.version }));
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

/// `install`: the release fetched, checked, unpacked and put in place
/// (`olo_release::install`), said as it goes.
fn install(prefix: &std::path::Path, args: &[String]) -> Result<(), String> {
    let ask = olo_release::Install {
        from: option(args, "--from"),
        tag: option(args, "--tag"),
        retire: args.windows(2).filter(|p| p[0] == "--retire").map(|p| p[1].clone()).collect(),
        // (A host's screens: `--without olo-wallpapers-`.)
        without: args.windows(2).filter(|p| p[0] == "--without").map(|p| p[1].clone()).collect(),
    };
    let mut p = Progress { doing: "", last: None };
    let (changed, version) = olo_release::install(prefix, &ask, &AtomicBool::new(false), &mut |doing, done, all| p.tell(doing, done, all))?;
    say(json!({ "installed": changed, "version": version }));
    Ok(())
}
