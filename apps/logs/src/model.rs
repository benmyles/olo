//! Journal entries as log lines: who wrote them, how serious, the text
//! cleaned of terminal colours and of the time and level `tracing` puts
//! in front (the journal has its own), and multi-line output (a pretty
//! debug print, a backtrace) joined back into the line it belongs to.
//! Then which lines a query shows.

use std::{path::Path, sync::Arc};

use olo_kit_base::search::{self, Needle};
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
    /// One of Olo's own programs.
    pub olo: bool,
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

/// One of Olo's programs (not a crate's test binary, `olo_x-hash`), or
/// one of them under its name before the rename (Tabula) in older lines.
pub fn is_olo(program: &str) -> bool {
    program.starts_with("olo-") || program.starts_with("tabula-") || program == "app_server"
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
        if comm.len() == COMM_MAX {
            if let Some(known) = full.iter().find(|k| k.starts_with(comm)) {
                return known.clone();
            }
            // A program that came and went before journald saw its file
            // (a screenshot): one of ours, installed beside us.
            if let Some(name) = installed().iter().find(|n| n.starts_with(comm)) {
                let name: Arc<str> = Arc::from(name.as_str());
                full.push(name.clone());
                return name;
            }
        }
        return Arc::from(comm);
    }
    Arc::from(e.get(Field::Identifier).and_then(base).unwrap_or_else(|| "?".into()))
}

/// Olo's programs with names longer than a process name, from the
/// directory this one runs from.
fn installed() -> &'static [String] {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
        let mut names: Vec<String> = dir
            .and_then(|d| std::fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| (n.starts_with("olo-") || n.starts_with("tabula-")) && n.len() > COMM_MAX && !n.contains('.'))
            .collect();
        names.sort();
        names
    })
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
    Continues {
        index: usize,
        text: String,
    },
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
                // Olo's programs log warnings through tracing; what they
                // print to stderr themselves (which the session's journal
                // stream files as warnings) is news unless it says otherwise.
                Level::Warning if is_olo(&program) => {
                    if says_trouble(&text) {
                        Level::Warning
                    } else {
                        Level::Info
                    }
                }
                l => l,
            });
            (level, text.trim_end().to_owned())
        };
        let olo = is_olo(&program);
        let line = Line { time: e.realtime, program, pid, level, message, olo, boot: e.boot };
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

/// A line of a log file `program` wrote (no journal around it): its
/// level and text read as a journal line's are, its time tracing's where
/// it starts with one, else `fallback` (microseconds since the epoch).
pub fn file_line(raw: &str, program: &Arc<str>, fallback: u64, boot: [u8; 16]) -> Line {
    let text = strip_ansi(raw);
    let time = tracing_time(&text).unwrap_or(fallback);
    let (level, message) = if let Some((level, rest)) = tracing_prefix(&text) {
        (level, rest.trim_end().to_owned())
    } else if let Some((level, rest)) = chromium_prefix(&text) {
        (level, rest.trim_end().to_owned())
    } else {
        // As an Olo program's stderr in the journal: news unless it says
        // otherwise.
        let level = level_in_text(&text).unwrap_or(if says_trouble(&text) { Level::Warning } else { Level::Info });
        (level, text.trim_end().to_owned())
    };
    // (A file chosen to read is shown as Olo's own are, by default.)
    Line { time, program: program.clone(), pid: 0, level, message, olo: true, boot }
}

/// Whether a log file's line goes on with the one before it (a panic's
/// backtrace, a wrapped value).
pub fn continues_line(raw: &str) -> bool {
    continues(&strip_ansi(raw))
}

/// The time tracing's lines start with (`2026-10-27T04:45:52.225681Z`), in
/// microseconds since the epoch.
fn tracing_time(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| text.get(r).and_then(|t| t.parse::<i64>().ok());
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut micros = 0;
    if b[19] == b'.' {
        let digits: String = text[20..].chars().take_while(char::is_ascii_digit).take(6).collect();
        micros = format!("{digits:0<6}").parse::<u64>().ok()?;
    }
    // Days since 1970 of the civil date (Howard Hinnant's).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let secs = u64::try_from(days * 86_400 + h * 3600 + mi * 60 + sec).ok()?;
    Some(secs * 1_000_000 + micros)
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

/// What the Log Viewer shows, from its search's text
/// ([`Query::from_text`], the kit's query language): the filters
/// `program:` (several at once, `all` for every program, `-program:` to
/// leave one out), `level:`, `since:` and `pid:`, and the words.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub since: Since,
    pub level: Level,
    /// The programs to show (any of them; by their name's start while
    /// one is typed); none: Olo's, or every one with `all_programs`.
    pub programs: Vec<Needle>,
    pub all_programs: bool,
    /// Programs left out.
    pub left_out: Vec<Needle>,
    pub pids: Vec<u32>,
    /// Words (and phrases) each line must have, or (left out) not.
    pub words: search::Query,
}

impl Default for Query {
    fn default() -> Self {
        Query { since: Since::Boot, level: Level::Info, programs: Vec::new(), all_programs: false, left_out: Vec::new(), pids: Vec::new(), words: search::Query::default() }
    }
}

/// The filters the Log Viewer's search knows.
pub const KEYS: &[&str] = &["program", "level", "since", "pid"];

impl Since {
    /// As typed in a query (`since:2h`).
    pub fn code(self) -> String {
        match self {
            Since::Minutes(m) if m >= 60 && m % 60 == 0 => format!("{}h", m / 60),
            Since::Minutes(m) => format!("{m}m"),
            Since::Today => "today".into(),
            Since::Boot => "boot".into(),
            Since::All => "all".into(),
        }
    }
}

/// What a query needs to know about now.
/// What a query needs to know about now.
#[derive(Clone, Copy)]
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

/// Local time at the start of `minute` (since the epoch), as the last
/// asked for is kept: a screenful of lines is of a few minutes, and
/// `localtime_r` for every one took 2 ms a frame.
fn local_minute(minute: u64) -> libc::tm {
    thread_local! {
        static LAST: std::cell::Cell<Option<(u64, libc::tm)>> = const { std::cell::Cell::new(None) };
    }
    LAST.with(|last| {
        if let Some((m, tm)) = last.get()
            && m == minute
        {
            return tm;
        }
        let t = (minute * 60) as libc::time_t;
        // SAFETY: localtime_r writes into the zeroed tm we own.
        let tm = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            tm
        };
        last.set(Some((minute, tm)));
        tm
    })
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
    /// The query a search's text asks for.
    pub fn from_text(text: &str) -> Query {
        let q = search::Query::parse(text, KEYS);
        let mut out = Query::default();
        for t in &q.terms {
            match (t.key.as_deref(), t.not) {
                (Some("program"), false) => {
                    for n in &t.any {
                        if n.is("all") || n.is("*") {
                            out.all_programs = true;
                        } else {
                            out.programs.push(n.clone());
                        }
                    }
                }
                (Some("program"), true) => out.left_out.extend(t.any.iter().cloned()),
                (Some("level"), false) => {
                    if let Some(l) = t.any.last().and_then(|n| Level::parse(n.as_str())) {
                        out.level = l;
                    }
                }
                (Some("since"), false) => {
                    if let Some(s) = t.any.last().and_then(|n| Since::parse(n.as_str())) {
                        out.since = s;
                    }
                }
                (Some("pid"), false) => out.pids.extend(t.any.iter().filter_map(|n| n.as_str().parse::<u32>().ok())),
                _ => {}
            }
        }
        out.words = search::Query { terms: q.terms.into_iter().filter(|t| t.key.is_none()).collect() };
        out
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

    pub fn matches(&self, l: &Line, now: &Now) -> bool {
        if l.level < self.level || !self.in_time(l, now) {
            return false;
        }
        if !self.programs.is_empty() {
            if !self.programs.iter().any(|n| n.starts(&l.program)) {
                return false;
            }
        } else if !self.all_programs && !l.olo {
            return false;
        }
        if self.left_out.iter().any(|n| n.is(&l.program)) || !self.pids.is_empty() && !self.pids.contains(&l.pid) {
            return false;
        }
        self.words.matches(|_, n| n.in_text(&l.message) || n.in_text(&l.program))
    }
}

/// `micros` as local time: `HH:MM:SS.mmm`, with the date in front unless
/// it is today.
pub fn format_time(micros: u64, midnight: u64) -> String {
    let secs = micros / 1_000_000;
    let tm = local_minute(secs / 60);
    let ms = (micros / 1000) % 1000;
    let clock = format!("{:02}:{:02}:{:02}.{ms:03}", tm.tm_hour, tm.tm_min, secs % 60);
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
        Entry { realtime: t, fields: vec![(Field::Message, msg.into()), (Field::Pid, pid.into()), (Field::Comm, comm.into()), (Field::Priority, "6".into())], ..Default::default() }
    }

    #[test]
    fn tracing_lines_are_cleaned() {
        let raw = "\u{1b}[2m2026-10-27T04:45:52.225681Z\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m \u{1b}[2mapp_server::udev\u{1b}[0m\u{1b}[2m:\u{1b}[0m Error during rendering: TemporaryFailure(";
        let mut b = Builder::default();
        let Some(Built::New(l)) = b.add(&entry(1, "7", "olo-app-serv", raw)) else { panic!() };
        assert_eq!(l.level, Level::Warning);
        assert_eq!(l.message, "app_server::udev: Error during rendering: TemporaryFailure(");
        assert!(l.olo);
        // The debug print's inside and its end join it.
        let Some(Built::Continues { index: 0, text }) = b.add(&entry(2, "7", "olo-app-serv", "    Access(")) else { panic!() };
        assert_eq!(text, "    Access(");
        assert!(matches!(b.add(&entry(3, "7", "olo-app-serv", ")")), Some(Built::Continues { index: 0, .. })));
        // Another process's line doesn't.
        assert!(matches!(b.add(&entry(4, "8", "chrome", "    indented")), Some(Built::New(_))));
    }

    #[test]
    fn levels_from_glib_and_priority() {
        let mut b = Builder::default();
        let Some(Built::New(l)) = b.add(&entry(1, "9", "gnome-calculator", "(gnome-calculator:9): Adwaita-WARNING **: deprecated")) else { panic!() };
        assert_eq!(l.level, Level::Warning);
        assert!(!l.olo);
        let mut e = entry(2, "10", "x", "bad");
        e.fields[3].1 = "3".into();
        let Some(Built::New(l)) = b.add(&e) else { panic!() };
        assert_eq!(l.level, Level::Error);
    }

    #[test]
    fn queries() {
        let now = Now { time: 100 * 60_000_000, midnight: 0, boot: Some([1; 16]) };
        let line = |level, program: &str, olo, time, boot| Line { time, program: program.into(), pid: 1, level, message: "Hello World".into(), olo, boot };
        let q = Query::default();
        assert_eq!(Query::from_text(""), q);
        assert!(q.matches(&line(Level::Info, "olo-tracker", true, 0, [1; 16]), &now));
        assert!(!q.matches(&line(Level::Info, "chrome", false, 0, [1; 16]), &now));
        assert!(!q.matches(&line(Level::Info, "olo-tracker", true, 0, [2; 16]), &now));
        let q = Query::from_text("since:10m level:warning program:all");
        assert_eq!((q.since, q.level, q.all_programs), (Since::Minutes(10), Level::Warning, true));
        assert!(!q.matches(&line(Level::Info, "chrome", false, 99 * 60_000_000, [1; 16]), &now));
        assert!(q.matches(&line(Level::Error, "chrome", false, 99 * 60_000_000, [1; 16]), &now));
        assert!(Query::from_text("since:10m level:error program:all \"World\"").matches(&line(Level::Error, "chrome", false, 99 * 60_000_000, [1; 16]), &now));
        assert!(!q.matches(&line(Level::Error, "chrome", false, 80 * 60_000_000, [1; 16]), &now));
        assert!(!Query::from_text("program:all nope").matches(&line(Level::Error, "chrome", false, 0, [1; 16]), &now));
        // Programs: several, by their start while typed, or left out.
        let some = Query::from_text("program:chrome,olo-p");
        assert!(some.matches(&line(Level::Info, "chrome", false, 0, [1; 16]), &now));
        assert!(some.matches(&line(Level::Info, "olo-pool", true, 0, [1; 16]), &now));
        assert!(!some.matches(&line(Level::Info, "olo-tracker", true, 0, [1; 16]), &now));
        let not = Query::from_text("-program:olo-pool hello -world");
        assert!(!not.matches(&line(Level::Info, "olo-pool", true, 0, [1; 16]), &now));
        assert!(!not.matches(&line(Level::Info, "olo-tracker", true, 0, [1; 16]), &now), "-world leaves it out");
        assert_eq!(Query::from_text("pid:1").pids, vec![1]);
        assert_eq!(Since::parse("2h").map(Since::code).as_deref(), Some("2h"));
        assert_eq!(Since::parse("10m"), Some(Since::Minutes(10)));
        assert_eq!(Since::parse("2h"), Some(Since::Minutes(120)));
        assert_eq!(Since::parse("boot"), Some(Since::Boot));
    }

    #[test]
    fn olo_stderr_is_news() {
        let mut b = Builder::default();
        let mut e = entry(1, "3", "olo-session", "olo-session: reloading");
        e.fields[3].1 = "4".into();
        let Some(Built::New(l)) = b.add(&e) else { panic!() };
        assert_eq!(l.level, Level::Info);
        let mut e = entry(2, "3", "olo-session", "olo-session: could not export the environment");
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

    #[test]
    fn file_lines() {
        let p: Arc<str> = Arc::from("agent");
        let l = file_line("2026-10-30T19:46:53.401234Z  WARN app_server::udev: no gamma", &p, 7, [0; 16]);
        assert_eq!((l.level, l.message.as_str()), (Level::Warning, "app_server::udev: no gamma"));
        assert_eq!(l.time, 1_793_389_613_401_234);
        assert_eq!(tracing_time("1970-01-01T00:00:01Z"), Some(1_000_000));
        let plain = file_line("starting up", &p, 7, [0; 16]);
        assert_eq!((plain.time, plain.level), (7, Level::Info));
        assert_eq!(file_line("couldn't open the socket", &p, 7, [0; 16]).level, Level::Warning);
        assert!(continues_line("    at main.rs:3") && !continues_line("next"));
    }
}
