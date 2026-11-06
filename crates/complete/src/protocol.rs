//! What brush and Olo's apps say to olo-complete, and how they reach it:
//! JSON lines over `$XDG_RUNTIME_DIR/olo-complete.sock`, one request and
//! one reply a connection (a Unix socket's connect is microseconds).

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::train::Tuned;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    /// What `line` (typed so far, in `cwd`, in shell `session`) likely
    /// becomes.
    Suggest { line: String, cwd: Option<String>, session: u64 },
    /// A command was entered.
    Ran { command: String, cwd: Option<String>, session: u64 },
    /// How the shell's last command ended (told at its next prompt).
    Ended { session: u64, status: i32 },
    /// The suggestion shown was taken: whole, or a word of it.
    Accepted { session: u64, whole: bool },
    /// Shell `session` carries on shell `from` (a Terminal tab restored
    /// after a restart: its commands so far are the old one's).
    Continues { session: u64, from: u64 },
    /// How it's doing (Activity Monitor ▸ AI).
    Stats,
    /// Learn from the history files (as the option is turned on): the
    /// reply when it's done.
    Learn,
    /// The settings changed (Preferences ▸ Terminal).
    Reload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    /// What to show after the line (empty: nothing), and the likeliest
    /// commands whole, best first.
    Suggestion {
        rest: String,
        candidates: Vec<String>,
        micros: u64,
    },
    Stats(Stats),
    Learned(Tuned),
    Ok,
    Error {
        message: String,
    },
}

/// How the model is doing, for Activity Monitor.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub enabled: bool,
    /// What it computes with: "processor (NEON)", "processor (AVX2)".
    pub backend: String,
    /// Suggestions shown, and taken (whole, or a word).
    pub offered: u64,
    pub accepted: u64,
    pub accepted_words: u64,
    /// How long a suggestion took (in the daemon), the median, the 95th
    /// percentile and the worst of the last 512, and the last ones in
    /// order (for a graph).
    pub latency_p50_us: u32,
    pub latency_p95_us: u32,
    pub latency_worst_us: u32,
    /// How many those are of (512 at most; 0: none asked for yet).
    #[serde(default)]
    pub latency_count: usize,
    #[serde(default)]
    pub latencies_us: Vec<u32>,
    /// Commands known (different ones) and seen.
    pub known: usize,
    pub seen: u64,
    /// Fine-tunes since it started, the last's report and when (seconds
    /// since 1970), and whether one waits for the minute to pass.
    pub fine_tunes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tune: Option<Tuned>,
    pub last_tune_at: i64,
    pub pending: usize,
    /// Whether the weights are the person's (fine-tuned) or the
    /// hand-written ones.
    pub tuned: bool,
}

/// Made-up figures as long as real ones get, a model a while in use:
/// for the layout lints and for looking at headless (Preferences ▸
/// Terminal, Activity Monitor's AI under `OLO_ACTIVITY_DEMO_AI`).
pub fn demo_stats() -> Stats {
    use crate::train::{Score, Tuned};
    let latencies_us: Vec<u32> = (0..120u32).map(|i| 1 + (i * 7919) % 5 + if i % 37 == 0 { 9 } else { 0 }).collect();
    Stats {
        enabled: true,
        backend: "processor (NEON)".into(),
        offered: 18_204,
        accepted: 4_311,
        accepted_words: 1_208,
        latency_p50_us: 2,
        latency_p95_us: 5,
        latency_worst_us: 41,
        latency_count: 512,
        latencies_us,
        known: 12_408,
        seen: 248_311,
        fine_tunes: 1_206,
        last_tune: Some(Tuned {
            before: Score { top1: 0.8412, mrr: 0.9012, examples: 300 },
            after: Score { top1: 0.8587, mrr: 0.9133, examples: 300 },
            baseline: Score { top1: 0.7311, mrr: 0.8245, examples: 300 },
            examples: 2_700,
            steps: 255,
            loss: 0.4821,
            micros: 11_480,
            backend: "processor (NEON)".into(),
            kept: true,
        }),
        last_tune_at: 1_793_777_040,
        pending: 18,
        tuned: true,
    }
}

/// In Olo's brush (`black_box`ed in its `main`, so it's in the binary):
/// Preferences finds it in the login shell's file to tell a brush that
/// asks olo-complete from one built before it did.
pub const HINTER_MARK: &[u8] = b"olo-complete: this brush asks for hints (protocol 1)";

/// Whether the program at `path` is a brush that asks olo-complete (its
/// file holds [`HINTER_MARK`]). Reads the whole file: off the UI thread.
pub fn asks_for_hints(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let finder = memchr::memmem::Finder::new(HINTER_MARK);
    let keep = HINTER_MARK.len() - 1;
    let mut buf = vec![0u8; 1 << 20];
    let mut have = 0;
    loop {
        let Ok(n) = f.read(&mut buf[have..]) else { return false };
        if n == 0 {
            return false;
        }
        have += n;
        if finder.find(&buf[..have]).is_some() {
            return true;
        }
        // (The mark may straddle two reads: its length less one kept.)
        let tail = have.min(keep);
        buf.copy_within(have - tail..have, 0);
        have = tail;
    }
}

/// Preferences ▸ Terminal's checkbox (`olo-preferences terminal --show
/// OPTION` opens there with it lit).
pub const OPTION: &str = "Suggest commands from a model of your own";

/// Preferences ▸ Terminal's: whether brush suggests with the model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub enabled: bool,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

/// `~/.config/olo/complete.json`.
pub fn settings_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("olo/complete.json")
}

/// Where its record and weights are kept: `~/.local/state/olo/complete`.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("olo/complete")
}

pub fn home_dir() -> PathBuf {
    home()
}

/// A Terminal shell's id (`OLO_SHELL_ID`, hex), set by olo-termd: the
/// shell's session for olo-complete, the same for its tab's whole life;
/// and the shell a restored one stands in for (`OLO_SHELL_RESTORED`).
pub const SHELL_ID_ENV: &str = "OLO_SHELL_ID";
pub const SHELL_RESTORED_ENV: &str = "OLO_SHELL_RESTORED";

/// A shell id from the environment variable `name`.
pub fn shell_id(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| u64::from_str_radix(v.trim(), 16).ok())
}

/// Where olo-termd keeps the end of shell `id`'s screen as text (the
/// last lines, scrollback before them), for olo-complete to read what
/// the commands printed: in the runtime folder, the user's alone.
pub fn tail_path(id: u64) -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()).map(|d| d.join(format!("olo-shell-{id:016x}.tail")))
}

/// The socket: in the runtime folder (None without one), or
/// `OLO_COMPLETE_SOCKET` (tests: a test session shares the runtime
/// folder with the person's own, so its olo-complete must not).
pub fn socket_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OLO_COMPLETE_SOCKET").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return Some(p);
    }
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()).map(|d| d.join("olo-complete.sock"))
}

impl Settings {
    pub fn load() -> Settings {
        std::fs::read(settings_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Written whole (a temporary name, then renamed).
    pub fn save(&self) -> std::io::Result<()> {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
}

/// Ask olo-complete, waiting at most `timeout` for the reply (None: it
/// isn't running, or didn't answer in time).
pub fn ask(req: &Request, timeout: Duration) -> Option<Reply> {
    let mut s = UnixStream::connect(socket_path()?).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    let mut line = serde_json::to_vec(req).ok()?;
    line.push(b'\n');
    s.write_all(&line).ok()?;
    let mut reply = String::new();
    BufReader::new(s).read_line(&mut reply).ok()?;
    serde_json::from_str(&reply).ok()
}

/// Tell olo-complete something, not waiting for its answer beyond the
/// write (what was run, what was taken).
pub fn tell(req: &Request) {
    let _ = ask(req, Duration::from_millis(50));
}

/// Whether olo-complete is listening (a socket left by one that's gone
/// refuses the connect).
pub fn running() -> bool {
    socket_path().is_some_and(|p| UnixStream::connect(p).is_ok())
}

/// Start olo-complete if it isn't running: the one beside this program
/// (an install's ~/.local/bin, a build's target), else on the path. Its
/// own group, so a shell's job control leaves it alone.
pub fn start() -> bool {
    let here = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("olo-complete")));
    let program = here.filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("olo-complete"));
    use std::os::unix::process::CommandExt;
    let child = std::process::Command::new(program).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).process_group(0).spawn();
    match child {
        Ok(mut c) => {
            // Reaped when it ends (it's a daemon: usually never, while
            // the shell lasts).
            let _ = std::thread::Builder::new().name("olo-complete-wait".into()).spawn(move || c.wait());
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A brush that asks is told from one that doesn't, the mark found
    /// wherever it lies (across two reads too), a missing file asking
    /// nothing.
    #[test]
    fn a_brush_that_asks_is_found_by_its_mark() {
        let dir = std::env::temp_dir().join(format!("olo-complete-mark-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("brush");
        for at in [0usize, 4096, (1 << 20) - 10, (1 << 20) + 3, 3 << 20] {
            let mut bytes = vec![7u8; (3 << 20) + HINTER_MARK.len() + 5];
            bytes[at..at + HINTER_MARK.len()].copy_from_slice(HINTER_MARK);
            std::fs::write(&file, &bytes).unwrap();
            assert!(asks_for_hints(&file), "the mark at {at}");
        }
        let mut almost = HINTER_MARK.to_vec();
        almost.pop();
        std::fs::write(&file, [vec![0u8; 1 << 20], almost].concat()).unwrap();
        assert!(!asks_for_hints(&file));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!asks_for_hints(&file));
    }
}
