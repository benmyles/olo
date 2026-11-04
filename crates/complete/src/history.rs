//! The commands the person has run: each with where (the folder, when
//! known), when, in which shell, and how it ended. Olo's own record
//! (`history.jsonl` in the state folder: what brush tells olo-complete
//! as each command runs) has all of that; brush's and bash's history
//! files (read once, for the first fine-tune) have the command and, when
//! the shell wrote them, its time.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One command run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub command: String,
    /// The folder it ran in (None: not known, as in a history file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// When, in seconds since 1970 (0: not known).
    #[serde(default)]
    pub at: i64,
    /// Which shell (its process id, or 0 for one long session).
    #[serde(default)]
    pub session: u64,
    /// How it ended, when the shell said (0: well).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
}

impl Entry {
    pub fn new(command: impl Into<String>) -> Entry {
        Entry { command: command.into(), cwd: None, at: 0, session: 0, status: None }
    }
}

/// A command line as worth learning: trimmed, one line, not empty, not
/// too long, and not one the person kept out of history (a leading
/// space, as bash's `ignorespace`).
pub fn worth_keeping(line: &str) -> Option<&str> {
    if line.starts_with(' ') || line.contains('\n') {
        return None;
    }
    let t = line.trim();
    (!t.is_empty() && t.len() <= 400).then_some(t)
}

/// A bash or brush history file's commands, oldest first: `#<seconds>`
/// lines give the next command's time; other comments are skipped.
pub fn parse_history_file(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('#') {
            at = rest.trim().parse().unwrap_or(0);
            continue;
        }
        if let Some(cmd) = worth_keeping(line) {
            out.push(Entry { at, ..Entry::new(cmd) });
        }
        at = 0;
    }
    out
}

/// The person's shell history from files, oldest first: bash's, then
/// brush's (each command as either kept it).
pub fn read_history_files(home: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    for name in [".bash_history", ".brush_history"] {
        if let Ok(bytes) = std::fs::read(home.join(name)) {
            out.extend(parse_history_file(&String::from_utf8_lossy(&bytes)));
        }
    }
    out
}

/// Olo's own record: one JSON entry a line (a line that doesn't read is
/// skipped).
pub fn parse_record(text: &str) -> Vec<Entry> {
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// The local hour (0 to 24, with minutes) and day of the week (0:
/// Sunday) at `at` seconds since 1970.
pub fn local_time(at: i64) -> (f32, u8) {
    // SAFETY: localtime_r fills the tm it's given; nothing else is kept.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t: libc::time_t = at as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    (tm.tm_hour as f32 + tm.tm_min as f32 / 60.0, tm.tm_wday.clamp(0, 6) as u8)
}

/// Seconds since 1970, now.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_files_with_times_and_without() {
        let text = "ls -la\n#1700000000\ngit status\n#not a time\n# a comment\n cd secret\n\ncargo build\n";
        let e = parse_history_file(text);
        assert_eq!(e.iter().map(|e| e.command.as_str()).collect::<Vec<_>>(), ["ls -la", "git status", "cargo build"]);
        assert_eq!((e[0].at, e[1].at, e[2].at), (0, 1_700_000_000, 0));
    }

    #[test]
    fn the_record_round_trips() {
        let e = Entry { cwd: Some("/home/me/src".into()), at: 5, session: 42, status: Some(1), ..Entry::new("make") };
        let line = serde_json::to_string(&e).unwrap();
        assert_eq!(parse_record(&format!("{line}\nnot json\n{line}\n")), [e.clone(), e]);
    }
}
