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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub enabled: bool,
    /// What it computes with: "processor (NEON)", "GPU (…)".
    pub backend: String,
    /// What was asked for, and why it isn't (a GPU that couldn't start).
    pub wanted: Accel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
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

/// What the model computes with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accel {
    /// The processor's vectors (SIMD).
    #[default]
    Simd,
    /// The graphics card.
    Gpu,
}

/// Preferences ▸ Terminal's: whether brush suggests with the model, and
/// what it computes with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub accel: Accel,
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

/// The socket: in the runtime folder (None without one).
pub fn socket_path() -> Option<PathBuf> {
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
