//! The crash reports Olo's programs leave (`olo_crash`, in
//! ~/.local/state/olo/crashes): read, named, told once each by the
//! session's watcher (notify.rs, by inotify on the folder), and listed
//! for a terminal or an agent (`olo-doctor --crashes [--json]`).

use std::path::{Path, PathBuf};

use serde::Serialize;

/// One report, as much as is shown of it.
#[derive(Debug, Clone, Serialize)]
pub struct Crash {
    /// The report's file name: what it's remembered as told by.
    pub file: String,
    pub path: PathBuf,
    pub program: String,
    pub pid: u64,
    pub at_ms: u64,
    pub started_ms: u64,
    /// "panic" or "signal".
    pub kind: String,
    pub thread: String,
    /// A panic's message, or the signal and where ("SIGSEGV at 0x10").
    pub what: String,
    /// Where a panic was (file:line:column).
    pub place: String,
    pub backtrace: String,
}

fn text(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or_default().to_owned()
}

fn number(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or_default()
}

fn read(path: &Path) -> Option<Crash> {
    let file = path.file_name()?.to_string_lossy().into_owned();
    if file.starts_with('.') || !file.ends_with(".json") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let kind = text(&v, "kind");
    let what = match (kind.as_str(), number(&v, "sent_by")) {
        ("signal", 0) => format!("{} at {}", text(&v, "signal"), text(&v, "address")),
        ("signal", by) => format!("{}, sent by process {by}", text(&v, "signal")),
        _ => text(&v, "message"),
    };
    Some(Crash {
        file,
        path: path.to_path_buf(),
        program: text(&v, "program"),
        pid: number(&v, "pid"),
        at_ms: number(&v, "at_ms"),
        started_ms: number(&v, "started_ms"),
        kind,
        thread: text(&v, "thread"),
        what,
        place: text(&v, "place"),
        backtrace: text(&v, "backtrace"),
    })
}

/// The reports in `dir`, newest first.
pub fn list_in(dir: &Path) -> Vec<Crash> {
    let mut all: Vec<Crash> = std::fs::read_dir(dir).map(|r| r.flatten().filter_map(|e| read(&e.path())).collect()).unwrap_or_default();
    all.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
    all
}

pub fn list() -> Vec<Crash> {
    list_in(&olo_crash::dir())
}

/// What a person calls the program.
pub fn name(program: &str) -> String {
    if let Some((_, name, _)) = olo_xdg::apps::OUR_APPS.iter().find(|(p, _, _)| *p == program) {
        return (*name).to_owned();
    }
    match program {
        "olo-app-server" => "Olo's window server",
        "olo-session" => "Olo's session",
        "olo-deskbar" => "Deskbar",
        "olo-notifications" => "Notifications",
        "olo-preferences" => "Preferences",
        "olo-palette" => "the command palette",
        "olo-brush" | "brush" => "brush",
        "olo-termd" => "Terminal's shells",
        "olo-lock" => "the lock screen",
        "olo-rescue" => "Olo Rescue",
        p => p,
    }
    .to_owned()
}

/// The icon a crash is told with: the program's own when it has one.
pub fn icon(program: &str) -> &'static str {
    olo_xdg::apps::OUR_APPS.iter().find(|(p, _, _)| *p == program).map_or("doctor", |(_, _, i)| i)
}

/// A crash in a line or two: what, and in which thread.
pub fn summary(c: &Crash) -> String {
    let what = c.what.lines().next().unwrap_or_default();
    let what: String = if what.chars().count() > 160 { what.chars().take(159).chain(['…']).collect() } else { what.to_owned() };
    // Its main thread: "main" in a panic's report, the program's own name
    // (as the kernel keeps it, 15 bytes at most) in a signal's.
    let main = c.thread.is_empty() || c.thread == "main" || (c.program.starts_with(&c.thread) && c.thread.len() >= c.program.len().min(15));
    if main { what } else { format!("{what} (in its {} thread)", c.thread) }
}

/// Logs' search for a crash: the program's lines from a little before
/// it to now.
pub fn logs_query(c: &Crash, now_ms: u64) -> String {
    let minutes = now_ms.saturating_sub(c.at_ms) / 60_000 + 2;
    format!("program:{} since:{minutes}m", c.program)
}

/// `olo-doctor --crashes [--json]`.
pub fn print(json: bool) {
    let all = list();
    if json {
        println!("{}", serde_json::to_string_pretty(&all).unwrap_or_default());
        return;
    }
    if all.is_empty() {
        println!("No crashes reported (in {}).", olo_crash::dir().display());
    }
    for c in &all {
        let secs = (c.at_ms / 1000) as libc::time_t;
        // SAFETY: localtime_r fills `tm` from `secs`.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&secs, &mut tm) };
        let ran = c.at_ms.saturating_sub(c.started_ms) / 1000;
        println!(
            "{:04}-{:02}-{:02} {:02}:{:02}  {} ({}, pid {}, after {}s): {}{}\n  {}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            name(&c.program),
            c.program,
            c.pid,
            ran,
            summary(c),
            if c.place.is_empty() { String::new() } else { format!(" at {}", c.place) },
            c.path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_read_and_told_as_a_person_would_say_them() {
        let dir = PathBuf::from(format!("/var/tmp/olo-doctor-crashes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("olo-viewer-9-2000.json"),
            r#"{"program":"olo-viewer","pid":9,"started_ms":1000,"at_ms":2000,"kind":"panic","thread":"video","message":"PoisonError\nmore","place":"crates/preview/src/video.rs:262:27","backtrace":"0: x"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("olo-styled-edit-7-2500.json"), r#"{"program":"olo-styled-edit","pid":7,"at_ms":2500,"kind":"signal","thread":"olo-styled-edit","signal":"SIGSEGV","sent_by":42}"#)
            .unwrap();
        std::fs::write(dir.join("olo-app-server-5-3000.json"), r#"{"program":"olo-app-server","pid":5,"at_ms":3000,"kind":"signal","thread":"dmabuf check","signal":"SIGSEGV","address":"0x10"}"#)
            .unwrap();
        // Half written (the dot), and not a report: passed over.
        std::fs::write(dir.join(".olo-tracker-1-4000.json"), "{").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let mut all = list_in(&dir);
        assert_eq!(all.iter().map(|c| c.program.as_str()).collect::<Vec<_>>(), ["olo-app-server", "olo-styled-edit", "olo-viewer"], "newest first");
        assert_eq!(summary(&all[1]), "SIGSEGV, sent by process 42", "its main thread, by the program's name");
        all.remove(1);
        assert_eq!(summary(&all[0]), "SIGSEGV at 0x10 (in its dmabuf check thread)");
        assert_eq!(summary(&all[1]), "PoisonError (in its video thread)");
        assert_eq!((name("olo-viewer").as_str(), icon("olo-viewer")), ("Viewer", "show-image"));
        assert_eq!(name("olo-app-server"), "Olo's window server");
        assert_eq!(logs_query(&all[1], 2000 + 5 * 60_000), "program:olo-viewer since:7m");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
