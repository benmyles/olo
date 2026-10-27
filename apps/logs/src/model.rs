//! Journal entries as log lines: who wrote them, how serious, the text
//! cleaned of terminal colours and of the time and level `tracing` puts
//! in front (the journal has its own), and multi-line output (a pretty
//! debug print, a backtrace) joined back into the line it belongs to.
//! Then which lines a query shows.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::journal::{Entry, Field};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Debug,
    Info,
    Warning,
    Error,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Debug => "Debug",
            Level::Info => "Info",
            Level::Warning => "Warning",
            Level::Error => "Error",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        Some(match s.to_ascii_lowercase().as_str() {
            "debug" | "trace" => Level::Debug,
            "info" => Level::Info,
            "warn" | "warning" | "warnings" => Level::Warning,
            "error" | "errors" | "err" => Level::Error,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Line {
    /// Microseconds since the epoch.
    pub time: u64,
    pub program: Arc<str>,
    pub pid: u32,
    pub level: Level,
    pub message: String,
    /// One of Tabula's own programs.
    pub tabula: bool,
    pub boot: [u8; 16],
}

impl Line {
    pub fn first_line(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

/// `text` without terminal escape sequences.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: parameters, then one final letter-ish byte.
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ST.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.peek() == Some(&'\\')) {
                        chars.next_if_eq(&'\\');
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    out
}

/// `2026-10-27T04:45:52.225681Z  WARN target: message` (tracing's format)
/// as its level and `target: message`.
fn tracing_prefix(text: &str) -> Option<(Level, &str)> {
    let b = text.as_bytes();
    if b.len() < 21 || !b[..4].iter().all(u8::is_ascii_digit) || b[4] != b'-' || b[10] != b'T' {
        return None;
    }
    let (_, rest) = text.split_once('Z')?;
    let rest = rest.trim_start();
    let (word, rest) = rest.split_once(char::is_whitespace)?;
    let level = match word {
        "TRACE" | "DEBUG" => Level::Debug,
        "INFO" => Level::Info,
        "WARN" => Level::Warning,
        "ERROR" => Level::Error,
        _ => return None,
    };
    Some((level, rest.trim_start()))
}

/// Chromium's `[pid:tid:MMDD/HHMMSS.micros:LEVEL:file.cc(line)] message`
/// as its level and `file.cc(line): message`.
fn chromium_prefix(text: &str) -> Option<(Level, String)> {
    let inner = text.strip_prefix('[')?;
    let (head, rest) = inner.split_once("] ")?;
    let mut parts = head.splitn(5, ':');
    let (pid, tid, time, level, file) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if !pid.bytes().all(|b| b.is_ascii_digit()) || !tid.bytes().all(|b| b.is_ascii_digit()) || !time.contains('/') {
        return None;
    }
    let level = match level {
        "FATAL" | "ERROR" => Level::Error,
        "WARNING" => Level::Warning,
        "INFO" => Level::Info,
        l if l.starts_with("VERBOSE") => Level::Debug,
        _ => return None,
    };
    Some((level, format!("{file}: {rest}")))
}

/// The level a line's own words give it (GLib's, Rust's panics).
fn level_in_text(text: &str) -> Option<Level> {
    if text.contains("-CRITICAL **") || text.contains("-ERROR **") || text.contains("panicked at") || text.starts_with("Error:") {
        Some(Level::Error)
    } else if text.contains("-WARNING **") {
        Some(Level::Warning)
    } else {
        None
    }
}

fn says_trouble(text: &str) -> bool {
    let t = text.to_lowercase();
    ["couldn't", "couldn’t", "could not", "can't", "cannot", "failed", "error", "crashed", "not found", "denied", "panic"].iter().any(|w| t.contains(w))
}

/// The level the journal's PRIORITY gives (syslog's: 0–3 errors, 4
/// warnings, 7 debug).
fn level_of_priority(p: Option<&str>) -> Level {
    match p.and_then(|p| p.parse::<u8>().ok()) {
        Some(0..=3) => Level::Error,
        Some(4) => Level::Warning,
        Some(7) => Level::Debug,
        _ => Level::Info,
    }
}

/// Whether `text` goes on the line before it (from the same process): the
/// inside of a pretty debug print, a backtrace's frames.
fn continues(text: &str) -> bool {
    let t = text.trim_start();
    text.starts_with([' ', '\t']) || t.starts_with([')', '}', ']']) || (t.starts_with('#') && t[1..].starts_with(|c: char| c.is_ascii_digit()))
}

/// One of Tabula's programs (not a crate's test binary, `tabula_x-hash`).
pub fn is_tabula(program: &str) -> bool {
    program.starts_with("tabula-") || program == "app_server"
}

/// The kernel cuts a process's name (`_COMM`) to 15 bytes.
const COMM_MAX: usize = 15;

/// Who wrote `e`: its program's file name, else its process name (made
/// whole again from a longer one seen, when cut), else its identifier.
fn program_of(e: &Entry, full: &mut Vec<Arc<str>>) -> Arc<str> {
    let base = |x: &str| x.rsplit('/').next().filter(|b| !b.is_empty()).map(str::to_owned);
    if let Some(exe) = e.get(Field::Exe).and_then(base) {
        if let Some(known) = full.iter().find(|k| ***k == *exe) {
            return known.clone();
        }
        let exe: Arc<str> = Arc::from(exe);
        if exe.len() > COMM_MAX {
            full.push(exe.clone());
        }
        return exe;
    }
    if let Some(comm) = e.get(Field::Comm) {
        if comm.len() == COMM_MAX
            && let Some(known) = full.iter().find(|k| k.starts_with(comm))
        {
            return known.clone();
        }
        return Arc::from(comm);
    }
    Arc::from(e.get(Field::Identifier).and_then(base).unwrap_or_else(|| "?".into()))
}

/// Turns entries into lines, joining continuations.
#[derive(Default)]
pub struct Builder {
    /// The last line each process wrote: its index in all lines so far,
    /// and when.
    last: std::collections::HashMap<u32, (usize, u64)>,
    /// Program names longer than a process name can be.
    full: Vec<Arc<str>>,
    /// Lines made so far.
    pub count: usize,
}

/// What the builder makes of one entry.
pub enum Built {
    New(Line),
    /// More text for line `index`.
    Continues { index: usize, text: String },
}

impl Builder {
    pub fn add(&mut self, e: &Entry) -> Option<Built> {
        let raw = e.get(Field::Message)?;
        let text = strip_ansi(raw);
        let pid = e.get(Field::Pid).and_then(|p| p.parse().ok()).unwrap_or(0);
        if pid != 0
            && continues(&text)
            && let Some(&(index, at)) = self.last.get(&pid)
            && e.realtime.saturating_sub(at) < 2_000_000
        {
            self.last.insert(pid, (index, e.realtime));
            return Some(Built::Continues { index, text: text.trim_end().to_owned() });
        }
        let mut program = program_of(e, &mut self.full);
        let (level, message) = if let Some((level, rest)) = tracing_prefix(&text) {
            (level, rest.trim_end().to_owned())
        } else if let Some((level, rest)) = chromium_prefix(&text) {
            // Google's launcher passes Chrome's output through cat.
            if &*program == "cat" {
                program = Arc::from("chrome");
            }
            (level, rest.trim_end().to_owned())
        } else {
            let level = level_in_text(&text).unwrap_or_else(|| match level_of_priority(e.get(Field::Priority)) {
                // Tabula's programs log warnings through tracing; what they
                // print to stderr themselves (which the session's journal
                // stream files as warnings) is news unless it says otherwise.
                Level::Warning if is_tabula(&program) => if says_trouble(&text) { Level::Warning } else { Level::Info },
                l => l,
            });
            (level, text.trim_end().to_owned())
        };
        let tabula = is_tabula(&program);
        let line = Line { time: e.realtime, program, pid, level, message, tabula, boot: e.boot };
        let index = self.count;
        self.count += 1;
        if pid != 0 {
            if self.last.len() > 4096 {
                self.last.clear();
            }
            self.last.insert(pid, (index, e.realtime));
        }
        Some(Built::New(line))
    }
}

/// How far back a query looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Since {
    Minutes(u64),
    Today,
    Boot,
    All,
}

impl Since {
    /// `10m`, `2h`, `1d`, `today`, `boot`, `all`.
    pub fn parse(s: &str) -> Option<Since> {
        Some(match s {
            "today" => Since::Today,
            "boot" => Since::Boot,
            "all" => Since::All,
            _ => {
                let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
                let n: u64 = n.parse().ok()?;
                Since::Minutes(match unit {
                    "m" | "min" => n,
                    "h" => n * 60,
                    "d" => n * 60 * 24,
                    _ => return None,
                })
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    pub since: Since,
    pub level: Level,
    /// One program, or every Tabula program (`None` with `all_programs`
    /// false), or everything.
    pub program: Option<String>,
    pub all_programs: bool,
    pub text: String,
}

impl Default for Query {
    fn default() -> Self {
        Query { since: Since::Boot, level: Level::Info, program: None, all_programs: false, text: String::new() }
    }
}

/// What a query needs to know about now.
pub struct Now {
    /// Microseconds since the epoch.
    pub time: u64,
    pub midnight: u64,
    pub boot: Option<[u8; 16]>,
}

impl Now {
    pub fn get() -> Now {
        let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64);
        Now { time, midnight: local_midnight(time), boot: current_boot() }
    }
}

/// This boot's id (`/proc/sys/kernel/random/boot_id`, as the journal keeps it).
pub fn current_boot() -> Option<[u8; 16]> {
    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let hex: String = text.trim().chars().filter(|c| *c != '-').collect();
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn local_midnight(micros: u64) -> u64 {
    let t = (micros / 1_000_000) as libc::time_t;
    // SAFETY: localtime_r and mktime work on the zeroed tm we own.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        let m = libc::mktime(&mut tm);
        (m.max(0) as u64) * 1_000_000
    }
}

impl Query {
    /// `text` lowercased once, for [`Query::matches`].
    pub fn needle(&self) -> String {
        self.text.trim().to_lowercase()
    }

    /// Whether `l` is in the time the query looks at.
    pub fn in_time(&self, l: &Line, now: &Now) -> bool {
        match self.since {
            Since::Minutes(m) => l.time + m * 60_000_000 >= now.time,
            Since::Today => l.time >= now.midnight,
            Since::Boot => now.boot.is_none_or(|b| b == l.boot),
            Since::All => true,
        }
    }

    pub fn matches(&self, l: &Line, now: &Now, needle: &str) -> bool {
        if l.level < self.level || !self.in_time(l, now) {
            return false;
        }
        match &self.program {
            Some(p) if *l.program != **p => return false,
            None if !self.all_programs && !l.tabula => return false,
            _ => {}
        }
        needle.is_empty() || l.message.to_lowercase().contains(needle) || l.program.to_lowercase().contains(needle)
    }
}

/// `micros` as local time: `HH:MM:SS.mmm`, with the date in front unless
/// it is today.
pub fn format_time(micros: u64, midnight: u64) -> String {
    let t = (micros / 1_000_000) as libc::time_t;
    // SAFETY: localtime_r writes into the zeroed tm we own.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    let ms = (micros / 1000) % 1000;
    let clock = format!("{:02}:{:02}:{:02}.{ms:03}", tm.tm_hour, tm.tm_min, tm.tm_sec);
    if micros >= midnight && micros < midnight + 86_400_000_000 {
        clock
    } else {
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{} {} {clock}", MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_mday)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(t: u64, pid: &str, comm: &str, msg: &str) -> Entry {
        Entry {
            realtime: t,
            fields: vec![(Field::Message, msg.into()), (Field::Pid, pid.into()), (Field::Comm, comm.into()), (Field::Priority, "6".into())],
            ..Default::default()
        }
    }

    #[test]
    fn tracing_lines_are_cleaned() {
        let raw = "\u{1b}[2m2026-10-27T04:45:52.225681Z\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m \u{1b}[2mapp_server::udev\u{1b}[0m\u{1b}[2m:\u{1b}[0m Error during rendering: TemporaryFailure(";
        let mut b = Builder::default();
        let Some(Built::New(l)) = b.add(&entry(1, "7", "tabula-app-serv", raw)) else { panic!() };
        assert_eq!(l.level, Level::Warning);
        assert_eq!(l.message, "app_server::udev: Error during rendering: TemporaryFailure(");
        assert!(l.tabula);
        // The debug print's inside and its end join it.
        let Some(Built::Continues { index: 0, text }) = b.add(&entry(2, "7", "tabula-app-serv", "    Access(")) else { panic!() };
        assert_eq!(text, "    Access(");
        assert!(matches!(b.add(&entry(3, "7", "tabula-app-serv", ")")), Some(Built::Continues { index: 0, .. })));
        // Another process's line doesn't.
        assert!(matches!(b.add(&entry(4, "8", "chrome", "    indented")), Some(Built::New(_))));
    }

    #[test]
    fn levels_from_glib_and_priority() {
        let mut b = Builder::default();
        let Some(Built::New(l)) = b.add(&entry(1, "9", "gnome-calculator", "(gnome-calculator:9): Adwaita-WARNING **: deprecated")) else { panic!() };
        assert_eq!(l.level, Level::Warning);
        assert!(!l.tabula);
        let mut e = entry(2, "10", "x", "bad");
        e.fields[3].1 = "3".into();
        let Some(Built::New(l)) = b.add(&e) else { panic!() };
        assert_eq!(l.level, Level::Error);
    }

    #[test]
    fn queries() {
        let now = Now { time: 100 * 60_000_000, midnight: 0, boot: Some([1; 16]) };
        let line = |level, program: &str, tabula, time, boot| Line { time, program: program.into(), pid: 1, level, message: "Hello World".into(), tabula, boot };
        let q = Query::default();
        assert!(q.matches(&line(Level::Info, "tabula-tracker", true, 0, [1; 16]), &now, ""));
        assert!(!q.matches(&line(Level::Info, "chrome", false, 0, [1; 16]), &now, ""));
        assert!(!q.matches(&line(Level::Info, "tabula-tracker", true, 0, [2; 16]), &now, ""));
        let q = Query { since: Since::Minutes(10), level: Level::Warning, all_programs: true, ..Query::default() };
        assert!(!q.matches(&line(Level::Info, "chrome", false, 99 * 60_000_000, [1; 16]), &now, ""));
        assert!(q.matches(&line(Level::Error, "chrome", false, 99 * 60_000_000, [1; 16]), &now, "world"));
        assert!(!q.matches(&line(Level::Error, "chrome", false, 80 * 60_000_000, [1; 16]), &now, ""));
        assert!(!q.matches(&line(Level::Error, "chrome", false, 99 * 60_000_000, [1; 16]), &now, "nope"));
        assert_eq!(Since::parse("10m"), Some(Since::Minutes(10)));
        assert_eq!(Since::parse("2h"), Some(Since::Minutes(120)));
        assert_eq!(Since::parse("boot"), Some(Since::Boot));
    }

    #[test]
    fn tabula_stderr_is_news() {
        let mut b = Builder::default();
        let mut e = entry(1, "3", "tabula-session", "tabula-session: reloading");
        e.fields[3].1 = "4".into();
        let Some(Built::New(l)) = b.add(&e) else { panic!() };
        assert_eq!(l.level, Level::Info);
        let mut e = entry(2, "3", "tabula-session", "tabula-session: could not export the environment");
        e.fields[3].1 = "4".into();
        let Some(Built::New(l)) = b.add(&e) else { panic!() };
        assert_eq!(l.level, Level::Warning);
    }

    #[test]
    fn chromium_lines() {
        let mut b = Builder::default();
        let raw = "[594078:594111:1027/003530.250883:ERROR:google_apis/gcm/engine/registration_request.cc:291] Registration response error";
        let Some(Built::New(l)) = b.add(&entry(1, "5", "cat", raw)) else { panic!() };
        assert_eq!((l.level, &*l.program), (Level::Error, "chrome"));
        assert_eq!(l.message, "google_apis/gcm/engine/registration_request.cc:291: Registration response error");
    }

    #[test]
    fn ansi_is_stripped() {
        assert_eq!(strip_ansi("\u{1b}[1;31mred\u{1b}[0m \u{1b}]0;title\u{7}x"), "red x");
    }
}
