//! What was done on each screen: every action `olo-screen` takes on it
//! (as it was asked, what it matched or where it went, how it went and how
//! long it took), one JSON object a line in
//! `$XDG_STATE_HOME/olo/screens/NAME/actions.jsonl`, appended as each one
//! ends. It outlives the screen until cleared (`olo-screen history NAME
//! --clear`), and is kept under `MAX_BYTES` by dropping its oldest half.

use std::{
    collections::BTreeMap,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// The commands that act on a screen (its history). Reads (`tree`,
/// `look`, `windows`, `menus`, `wait`) change nothing and are left out.
pub const ACTIONS: &[&str] = &[
    "start", "stop", "run", "click", "move", "down", "up", "drag", "scroll", "wheel", "key", "type", "set", "select", "scroll-to", "invoke", "focus", "close", "minimize", "zoom", "resize", "place",
];

/// The most a screen's history takes on disk.
const MAX_BYTES: u64 = 8 << 20;

/// Stretches without an action longer than this don't count as the
/// screen being in use.
pub const IDLE_MS: u64 = 60_000;

/// One action on a screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// When it began (milliseconds since the epoch), and how long it took.
    pub at: u64,
    pub ms: u64,
    pub kind: String,
    /// Its arguments after the screen's name, as given.
    #[serde(default)]
    pub args: Vec<String>,
    /// Its answer: what it matched, where it clicked, the focus after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Action {
    /// A line for people: the time, what was done, and what it found.
    pub fn line(&self) -> String {
        let time = clock(self.at);
        let args = self.args.iter().map(|a| if a.contains(' ') || a.is_empty() { format!("{a:?}") } else { a.clone() }).collect::<Vec<_>>().join(" ");
        let what = if args.is_empty() { self.kind.clone() } else { format!("{} {args}", self.kind) };
        let outcome = match (&self.error, self.matched()) {
            (Some(e), _) => format!("  ✗ {e}"),
            (None, Some(m)) => format!("  → {m}"),
            (None, None) => String::new(),
        };
        format!("{time}  {what}{outcome}  ({} ms)", self.ms)
    }

    /// What it found to act on, as `Role "name" at X,Y`.
    pub fn matched(&self) -> Option<String> {
        let m = self.result.as_ref()?.get("matched")?;
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let name = m.get("name").and_then(|r| r.as_str()).unwrap_or("");
        let at = m.get("center").and_then(|c| Some((c.get(0)?.as_f64()?, c.get(1)?.as_f64()?)));
        Some(match at {
            Some((x, y)) => format!("{role} {name:?} at {x:.0},{y:.0}"),
            None => format!("{role} {name:?}"),
        })
    }

    /// Whether `text` is in it anywhere (its kind, arguments, what it
    /// matched, its error), regardless of case.
    pub fn mentions(&self, text: &str) -> bool {
        let t = text.to_lowercase();
        self.kind.to_lowercase().contains(&t)
            || self.args.iter().any(|a| a.to_lowercase().contains(&t))
            || self.matched().is_some_and(|m| m.to_lowercase().contains(&t))
            || self.error.as_ref().is_some_and(|e| e.to_lowercase().contains(&t))
    }
}

/// Where screen `name`'s history is kept.
pub fn dir(name: &str) -> PathBuf {
    olo_xdg::state_dir().join("screens").join(name)
}

fn actions_path(name: &str) -> PathBuf {
    dir(name).join("actions.jsonl")
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Add `action` to screen `name`'s history (quietly giving up if the disk
/// won't have it: the action itself was done).
pub fn record(name: &str, action: &Action) {
    let path = actions_path(name);
    if std::fs::create_dir_all(dir(name)).is_err() {
        return;
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let all = read(name);
        let keep = &all[all.len() / 2..];
        let text: String = keep.iter().filter_map(|a| serde_json::to_string(a).ok()).map(|l| l + "\n").collect();
        let tmp = path.with_extension("jsonl.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path)
        && let Ok(line) = serde_json::to_string(action)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Screen `name`'s history, oldest first (lines that don't read are
/// skipped).
pub fn read(name: &str) -> Vec<Action> {
    std::fs::read_to_string(actions_path(name)).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
}

/// Forget screen `name`'s history.
pub fn clear(name: &str) -> Result<(), String> {
    match std::fs::remove_dir_all(dir(name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("couldn't clear {name}'s history: {e}")),
    }
}

/// The screens with a history kept.
pub fn names() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(olo_xdg::state_dir().join("screens"))
        .map(|d| d.flatten().filter(|e| e.path().join("actions.jsonl").is_file()).filter_map(|e| e.file_name().to_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// A screen's use, from its history.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stats {
    pub name: String,
    /// When it was first started and last acted on (ms since the epoch).
    pub started: Option<u64>,
    pub last_used: Option<u64>,
    /// How long it has run (from each start to its stop, or to now while
    /// it runs) and been in use (the stretches between actions shorter
    /// than `IDLE_MS`), in milliseconds.
    pub ran_ms: u64,
    pub active_ms: u64,
    /// Actions of each kind.
    pub actions: BTreeMap<String, usize>,
    /// The programs run on it, each with how many times.
    pub programs: BTreeMap<String, usize>,
    /// Actions that failed.
    pub failed: usize,
}

/// `history`'s stats, `running` saying whether the screen is up now.
pub fn stats(name: &str, history: &[Action], running: bool, now: u64) -> Stats {
    let mut s = Stats { name: name.to_owned(), ..Default::default() };
    let mut since: Option<u64> = None;
    let mut last_end: Option<u64> = None;
    for a in history {
        *s.actions.entry(a.kind.clone()).or_default() += 1;
        s.failed += usize::from(a.error.is_some());
        if a.kind == "run"
            && let Some(p) = a.args.first()
        {
            let program = std::path::Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p).to_owned();
            *s.programs.entry(program).or_default() += 1;
        }
        match a.kind.as_str() {
            "start" if a.error.is_none() => {
                s.started.get_or_insert(a.at);
                since.get_or_insert(a.at);
            }
            "stop" => {
                if let Some(from) = since.take() {
                    s.ran_ms += a.at.saturating_sub(from);
                }
            }
            _ => {}
        }
        // In use: an action's own time, and the gap before it when short.
        let end = a.at + a.ms;
        let gap = last_end.map(|e| a.at.saturating_sub(e)).filter(|g| *g < IDLE_MS).unwrap_or(0);
        s.active_ms += a.ms + gap;
        last_end = Some(last_end.map_or(end, |e| e.max(end)));
        s.last_used = Some(s.last_used.map_or(end, |l| l.max(end)));
    }
    if running && let Some(from) = since {
        s.ran_ms += now.saturating_sub(from);
    }
    s
}

/// `ms` (since the epoch) as the local time of day, to the millisecond.
pub fn clock(ms: u64) -> String {
    let secs = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(at: u64, ms: u64, kind: &str, args: &[&str], error: Option<&str>) -> Action {
        Action { at, ms, kind: kind.into(), args: args.iter().map(|a| (*a).to_owned()).collect(), result: None, error: error.map(str::to_owned) }
    }

    #[test]
    fn stats_from_a_history() {
        let h = vec![
            act(1_000, 900, "start", &[], None),
            act(5_000, 100, "run", &["/usr/bin/olo-styled-edit", "notes.txt"], None),
            act(6_000, 50, "click", &["--name", "Save"], None),
            act(7_000, 30, "click", &["--name", "Nope"], Some("nothing matches")),
            // Ten minutes later: the gap isn't use.
            act(607_000, 40, "key", &["Cmd+S"], None),
            act(608_000, 10, "stop", &[], None),
        ];
        let s = stats("agent", &h, false, 700_000);
        assert_eq!((s.started, s.last_used), (Some(1_000), Some(608_010)));
        assert_eq!(s.ran_ms, 607_000);
        assert_eq!(s.actions["click"], 2);
        assert_eq!(s.programs["olo-styled-edit"], 1);
        assert_eq!(s.failed, 1);
        // The actions' own time and the short gaps between them, not the
        // ten minutes.
        assert_eq!(s.active_ms, 900 + 100 + 50 + 30 + 40 + 10 + (5_000 - 1_900) + (6_000 - 5_100) + (7_000 - 6_050) + (608_000 - 607_040));
        // Still running: up to now.
        let running = stats("agent", &h[..5], true, 700_000);
        assert_eq!(running.ran_ms, 699_000);
    }

    #[test]
    fn lines_say_what_was_done_and_found() {
        let mut a = act(0, 42, "click", &["--name", "Save As…"], None);
        a.result = Some(serde_json::json!({"matched": {"role": "Button", "name": "Save", "center": [640.0, 320.0]}}));
        assert!(a.line().ends_with("click --name \"Save As…\"  → Button \"Save\" at 640,320  (42 ms)"), "{}", a.line());
        assert!(a.mentions("button") && a.mentions("save as") && !a.mentions("tracker"));
        let failed = act(0, 5, "click", &["--name", "Nope"], Some("nothing matches"));
        assert!(failed.line().contains("✗ nothing matches"));
    }
}
