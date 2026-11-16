//! What Kache shows, read natively (Ben, 2026-11-15: "an olo native kache
//! frontend app ... never its CLI"): kache's daemon asked over its socket
//! (`{"stats":{…}}`, a line of JSON each way), its build events
//! (`events.jsonl`), its last clean-up (`gc_stats.json`), its settings
//! (`~/.config/kache/config.toml`, edited keeping the file's comments),
//! and its user service (`kache.service`, over systemd's D-Bus). Each on
//! a worker. `OLO_KACHE_DEMO` makes it all up (tests, the lint): nothing
//! of the person's kache is touched then.

use std::{
    io::{BufRead as _, Write as _},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use olo_system::systemd::UserUnit;
use serde_json::Value;

/// kache's user service.
pub const UNIT: &str = "kache.service";

/// Whether it's the demo (made up: tests, the lint).
pub fn demo() -> bool {
    std::env::var("OLO_KACHE_DEMO").is_ok_and(|v| !v.is_empty() && v != "0")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// kache's settings file.
pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("OLO_KACHE_CONFIG") {
        return PathBuf::from(p);
    }
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".config")).join("kache/config.toml")
}

/// kache's store and its daemon's socket and records.
pub fn cache_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("OLO_KACHE_DIR") {
        return PathBuf::from(p);
    }
    let set = std::fs::read_to_string(config_path()).ok().and_then(|t| t.parse::<toml_edit::DocumentMut>().ok()).and_then(|d| d.get("cache")?.get("cache_dir")?.as_str().map(PathBuf::from));
    set.unwrap_or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".cache")).join("kache"))
}

/// Now, in ms since 1970.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// What the daemon says of itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Daemon {
    pub version: String,
    /// Its store: what it holds (as builds see it, before files shared
    /// are counted once), its limit, how many entries.
    pub size: u64,
    pub disk: u64,
    pub limit: u64,
    pub entries: u64,
    /// Compiles going on now: each crate's name.
    pub in_flight: Vec<String>,
}

/// The last clean-up (`gc_stats.json`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cleanup {
    pub at_ms: u64,
    pub freed: u64,
    /// What it couldn't take: files builds' target folders still share.
    pub held: u64,
    pub unreclaimable: u64,
}

/// One build event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Event {
    pub at_ms: u64,
    pub crate_name: String,
    /// The project's folder's name.
    pub project: String,
    /// "local_hit", "remote_hit", "miss", "passthrough", "dup", "error".
    pub result: String,
    pub elapsed_ms: u64,
    /// What compiling it took when it was made (a hit's saving is this
    /// less what the hit took).
    pub compile_ms: u64,
    pub size: u64,
}

impl Event {
    pub fn hit(&self) -> bool {
        self.result.ends_with("hit")
    }
}

/// The last 24 hours of builds: each hour's hits, misses and others
/// (oldest first), and the totals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Day {
    pub hours: Vec<[u32; 3]>,
    pub hits: u64,
    pub misses: u64,
    pub others: u64,
    /// The hour the first bar is (ms since 1970).
    pub from_ms: u64,
    /// The projects built most, and the crates whose compiles (misses)
    /// took longest.
    pub projects: Vec<Tally>,
    pub slowest: Vec<(String, u64)>,
    /// Time the hits saved (ms): what their compiles took less what they did.
    pub saved_ms: u64,
}

/// A project's builds over the day.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tally {
    pub name: String,
    pub hits: u64,
    pub misses: u64,
    pub others: u64,
}

/// The most of each listed.
pub const TOP: usize = 5;

impl Day {
    /// Hits of what could be cached (hits and misses), 0 to 1.
    pub fn rate(&self) -> Option<f64> {
        let n = self.hits + self.misses;
        (n > 0).then(|| self.hits as f64 / n as f64)
    }
}

/// kache's scheduler now: the pool's slots held (each admitted compile
/// holds a lease file under `scheduler/permits/` locked; a test, its
/// markers under `scheduler/tests/`), read from the kernel's list of
/// locks (`/proc/locks`), never by taking one; and builds waiting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pool {
    /// Slots in the pool (`scheduler_permits`, else one a processor).
    pub size: u32,
    pub compiles: u32,
    pub tests: u32,
    pub waiting: u32,
}

/// The pool as it is (None: kache's scheduler isn't there).
pub fn pool(dir: &std::path::Path, permits: Option<u32>) -> Option<Pool> {
    use std::os::unix::fs::MetadataExt as _;
    let dir = dir.join("scheduler");
    if !dir.is_dir() {
        return None;
    }
    let locks = std::fs::read_to_string("/proc/locks").unwrap_or_default();
    // (By inode alone: the kernel names a lock's file by its filesystem's
    // device, which on btrfs isn't the subvolume's that `stat` gives.)
    let held: std::collections::HashSet<u64> = locks.lines().filter_map(|l| l.split_whitespace().nth(5)?.rsplit(':').next()?.parse().ok()).collect();
    let locked = |sub: &str| -> u32 { std::fs::read_dir(dir.join(sub)).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()).filter(|m| held.contains(&m.ino())).count() as u32 };
    let count = |sub: &str| std::fs::read_dir(dir.join(sub)).map_or(0, |d| d.flatten().count() as u32);
    let size = permits.filter(|n| *n > 0).unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() as u32));
    Some(Pool { size, compiles: locked("permits"), tests: locked("tests"), waiting: count("waiters").max(count("queue")) })
}

/// Everything shown, read at once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// The service (None: the user manager can't be asked).
    pub unit: Option<UserUnit>,
    /// The daemon (None: not answering).
    pub daemon: Option<Daemon>,
    pub cleanup: Option<Cleanup>,
    pub day: Day,
    /// The latest builds, newest first.
    pub recent: Vec<Event>,
    pub pool: Option<Pool>,
}

/// The builds kept to list.
pub const RECENT: usize = 300;

/// Read everything (on a worker).
pub fn read() -> Snapshot {
    if demo() {
        return made_up(now_ms());
    }
    let dir = cache_dir();
    let unit = olo_system::systemd::user_unit(UNIT).ok();
    let daemon = ask_stats(&dir).ok();
    let cleanup = std::fs::read(dir.join("gc_stats.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| cleanup_of(&v));
    let now = now_ms();
    let since = now.saturating_sub(24 * 3_600_000);
    let mut events = archived_since(&dir.join("events.jsonl.archive"), since);
    events.extend(LIVE.lock().unwrap_or_else(|e| e.into_inner()).read(&dir.join("events.jsonl"), since).iter().cloned());
    let day = day_of(&events, now);
    let mut recent: Vec<Event> = events.into_iter().rev().take(RECENT).collect();
    recent.shrink_to_fit();
    let permits = settings_at(&config_path(), "scheduler_permits").and_then(|v| v.parse().ok());
    let pool = pool(&dir, permits);
    Snapshot { unit, daemon, cleanup, day, recent, pool }
}

/// The live build log, read as it grows: what was read is kept, and each
/// look reads only the lines added since (kache appends a line a compile:
/// the file is megabytes by the end of a day). Rotated (another file at
/// the path) or cut short, it's read again from the start.
#[derive(Default)]
pub struct Live {
    ino: u64,
    read_to: u64,
    events: Vec<Event>,
}

static LIVE: std::sync::Mutex<Live> = std::sync::Mutex::new(Live { ino: 0, read_to: 0, events: Vec::new() });

impl Live {
    /// The log's events from `since` on (and the last `RECENT` before it).
    pub fn read(&mut self, path: &std::path::Path, since: u64) -> &[Event] {
        use std::{io::Seek as _, os::unix::fs::MetadataExt as _};
        let Ok(mut f) = std::fs::File::open(path) else {
            *self = Live::default();
            return &[];
        };
        let Ok(meta) = f.metadata() else { return &self.events };
        if meta.ino() != self.ino || meta.len() < self.read_to {
            *self = Live { ino: meta.ino(), ..Live::default() };
        }
        if meta.len() > self.read_to && f.seek(std::io::SeekFrom::Start(self.read_to)).is_ok() {
            let mut added = Vec::new();
            if std::io::Read::read_to_end(&mut f, &mut added).is_ok() {
                // (Up to the last whole line: one being written is read next time.)
                let whole = added.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
                self.events.extend(parse_events(&String::from_utf8_lossy(&added[..whole])));
                self.read_to += whole as u64;
            }
        }
        // (Kept: the day's and the last few hundred.)
        let old = self.events.iter().take_while(|e| e.at_ms < since).count().min(self.events.len().saturating_sub(RECENT));
        self.events.drain(..old);
        &self.events
    }
}

/// A line of JSON to the daemon's socket, its line back.
fn ask(dir: &std::path::Path, request: &Value, timeout: Duration) -> Result<Value, String> {
    let socket = dir.join("daemon.sock");
    let s = std::os::unix::net::UnixStream::connect(&socket).map_err(|e| format!("kache's daemon isn't answering ({e})"))?;
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    let mut w = &s;
    w.write_all(format!("{request}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(&s).read_line(&mut line).map_err(|e| format!("kache's daemon didn't answer ({e})"))?;
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("kache's daemon answered something unreadable ({e})"))?;
    if v["ok"].as_bool() == Some(false) {
        return Err(v["error"].as_str().unwrap_or("kache's daemon refused").to_owned());
    }
    Ok(v)
}

fn ask_stats(dir: &std::path::Path) -> Result<Daemon, String> {
    // (No hours of events: counting them costs the daemon 0.17 s an ask,
    // without them 0.035 s; the day's figures are counted here, from the
    // events read anyway.)
    let request = serde_json::json!({"stats": {"include_entries": false, "include_summaries": false, "sort_by": null, "event_hours": 0, "event_secs": 0, "client_epoch": 0}});
    let v = ask(dir, &request, Duration::from_secs(5))?;
    Ok(daemon_of(&v["stats"]))
}

/// The daemon's stats as shown.
pub fn daemon_of(s: &Value) -> Daemon {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let in_flight = s["in_flight"].as_array().map(|a| a.iter().filter_map(|f| f["crate_name"].as_str().or(f["crate"].as_str()).or(f.as_str()).map(str::to_owned)).collect()).unwrap_or_default();
    Daemon {
        version: s["version"].as_str().unwrap_or_default().to_owned(),
        size: n(&s["total_size"]),
        disk: n(&s["blob_stats"]["total_blob_size"]),
        limit: n(&s["max_size"]),
        entries: n(&s["entry_count"]),
        in_flight,
    }
}

fn cleanup_of(v: &Value) -> Cleanup {
    let at_ms = v["last_run"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map_or(0, |t| t.timestamp_millis().max(0) as u64);
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    Cleanup { at_ms, freed: n("disk_bytes_reclaimed").max(n("bytes_freed")), held: n("bytes_held"), unreclaimable: n("entries_unreclaimable") }
}

/// The archived build events from `since` (ms since 1970) on: kache
/// rotates `events.jsonl` into `events.jsonl.archive/`, each file
/// zstd-compressed and named by its last and first events' times
/// ("01794795310-01794707744-…": seconds), so only those reaching into the
/// time are read. Oldest first.
/// (Read once and kept while the archive's files are the same: it changes
/// only as kache rotates its log, and a day of it is 30,000 events.)
pub fn archived_since(dir: &std::path::Path, since: u64) -> Vec<Event> {
    static KEPT: std::sync::Mutex<Option<(Vec<PathBuf>, u64, Vec<Event>)>> = std::sync::Mutex::new(None);
    let mut files: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let last: u64 = name.split('-').next()?.parse().ok()?;
            (last * 1000 >= since && name.ends_with(".jsonl.zst")).then(|| (last, e.path()))
        })
        .collect();
    files.sort();
    let names: Vec<PathBuf> = files.iter().map(|(_, p)| p.clone()).collect();
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((was, from, events)) = kept.as_ref()
        && *was == names
        && *from <= since
    {
        return events.iter().filter(|e| e.at_ms >= since).cloned().collect();
    }
    let events: Vec<Event> = names
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|b| zstd::decode_all(&b[..]).ok())
        .flat_map(|t| parse_events(&String::from_utf8_lossy(&t)))
        .filter(|e| e.at_ms >= since)
        .collect();
    *kept = Some((names, since, events.clone()));
    events
}

/// The build events in `text`, a line each (lines that aren't one skipped).
/// (Read as kache writes them, borrowed where they can be: a day of the
/// archive is tens of thousands.)
pub fn parse_events(text: &str) -> Vec<Event> {
    use std::borrow::Cow;
    #[derive(serde::Deserialize)]
    struct Line<'a> {
        #[serde(borrow)]
        ts: Cow<'a, str>,
        #[serde(borrow, default)]
        crate_name: Option<Cow<'a, str>>,
        #[serde(borrow, default)]
        root: Option<Cow<'a, str>>,
        #[serde(borrow)]
        result: Cow<'a, str>,
        #[serde(default)]
        elapsed_ms: Option<u64>,
        #[serde(default)]
        compile_time_ms: Option<u64>,
        #[serde(default)]
        size: Option<u64>,
    }
    text.lines()
        .filter_map(|l| serde_json::from_str::<Line>(l).ok())
        .filter_map(|v| {
            let at_ms = chrono::DateTime::parse_from_rfc3339(&v.ts).ok()?.timestamp_millis().max(0) as u64;
            // (A build's root is its project's folder or its target folder.)
            let project = v.root.as_deref().and_then(|r| r.rsplit('/').find(|p| !p.is_empty() && *p != "target")).unwrap_or_default().to_owned();
            Some(Event {
                at_ms,
                crate_name: v.crate_name.map(Cow::into_owned).unwrap_or_default(),
                project,
                result: v.result.into_owned(),
                elapsed_ms: v.elapsed_ms.unwrap_or(0),
                compile_ms: v.compile_time_ms.unwrap_or(0),
                size: v.size.unwrap_or(0),
            })
        })
        .collect()
}

/// The 24 hours to `now`, by the hour.
pub fn day_of(events: &[Event], now: u64) -> Day {
    const HOUR: u64 = 3_600_000;
    let from = (now / HOUR + 1) * HOUR - 24 * HOUR;
    let mut d = Day { hours: vec![[0; 3]; 24], from_ms: from, ..Day::default() };
    let mut projects: std::collections::HashMap<String, Tally> = std::collections::HashMap::new();
    let mut slowest: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for e in events.iter().filter(|e| e.at_ms >= from && e.at_ms <= now) {
        let h = ((e.at_ms - from) / HOUR).min(23) as usize;
        let k = if e.hit() {
            d.hits += 1;
            d.saved_ms += e.compile_ms.saturating_sub(e.elapsed_ms);
            0
        } else if e.result == "miss" {
            d.misses += 1;
            1
        } else {
            d.others += 1;
            2
        };
        d.hours[h][k] += 1;
        let name = if e.project.is_empty() { "(no project)" } else { e.project.as_str() };
        let t = match projects.get_mut(name) {
            Some(t) => t,
            None => projects.entry(name.to_owned()).or_insert_with(|| Tally { name: name.to_owned(), ..Tally::default() }),
        };
        *[&mut t.hits, &mut t.misses, &mut t.others][k] += 1;
        if k == 1 {
            let ms = slowest.entry(e.crate_name.as_str()).or_insert(0);
            *ms = (*ms).max(e.elapsed_ms);
        }
    }
    let mut projects: Vec<Tally> = projects.into_values().collect();
    projects.sort_by(|a, b| (b.hits + b.misses + b.others).cmp(&(a.hits + a.misses + a.others)).then_with(|| a.name.cmp(&b.name)));
    projects.truncate(TOP);
    d.projects = projects;
    let mut slowest: Vec<(String, u64)> = slowest.into_iter().map(|(c, ms)| (c.to_owned(), ms)).collect();
    slowest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    slowest.truncate(TOP);
    d.slowest = slowest;
    d
}

/// Clean up now: the daemon's own clean-up, as its limit asks.
pub fn clean_up() -> Result<String, String> {
    if demo() {
        return Ok("Cleaned up (made up).".into());
    }
    // (As kache's own CLI asks: its automatic policy, the daemon's limit.)
    ask(&cache_dir(), &serde_json::json!({"gc_v2": {"max_age_hours": null, "mode": "automatic"}}), Duration::from_secs(300))?;
    Ok("Cleaned up.".into())
}

/// Restart, start or stop the service (on a worker).
pub fn service(method: &str) -> Result<UserUnit, String> {
    if demo() {
        return Ok(made_up(now_ms()).unit.unwrap_or_default());
    }
    olo_system::systemd::user_job(UNIT, method).map_err(|e| format!("kache's service: {e}"))
}

/// A setting of kache's, as shown and edited (its `[cache]` key).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setting {
    pub key: &'static str,
    pub label: &'static str,
    pub about: &'static str,
    pub kind: Kind,
    /// What kache does with it unset (its source, 2.1.1): a switch's
    /// "true" or "false"; a field's words, shown in it while it's empty.
    pub default: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// A size as kache writes one ("95GiB").
    Size,
    Number,
    Switch,
}

/// The settings Kache offers, in order.
pub const SETTINGS: &[Setting] = &[
    Setting { key: "local_max_size", label: "Store limit:", about: "Before what's used least goes.", kind: Kind::Size, default: "5% of its disk, up to 100GiB" },
    Setting { key: "count_held_in_max_size", label: "", about: "Count what builds' target folders share toward the limit", kind: Kind::Switch, default: "false" },
    Setting { key: "auto_clean_idle_targets_days", label: "Idle targets:", about: "Days unused before a target folder is cleaned.", kind: Kind::Number, default: "never" },
    Setting { key: "auto_clean_orphaned_targets", label: "", about: "Clean target folders whose project is gone", kind: Kind::Switch, default: "true" },
    Setting { key: "scheduler_permits", label: "At once:", about: "Compiles and tests run together.", kind: Kind::Number, default: "one a processor" },
    Setting { key: "scheduler_test_slots", label: "Test slots:", about: "The most of those one test binary takes.", kind: Kind::Number, default: "all" },
    Setting { key: "scheduler_test_wait_secs", label: "Test wait:", about: "Seconds a test waits for its slots.", kind: Kind::Number, default: "1800" },
    Setting { key: "scheduler_dynamic", label: "", about: "Let more in while the machine has room to spare", kind: Kind::Switch, default: "true" },
    Setting { key: "scheduler_cpu_percent", label: "Room to:", about: "Percent of the processors kept busy.", kind: Kind::Number, default: "85" },
];

/// One setting's value in the file at `path`.
fn settings_at(path: &std::path::Path, key: &str) -> Option<String> {
    let doc = std::fs::read_to_string(path).ok()?.parse::<toml_edit::DocumentMut>().ok()?;
    let item = doc.get("cache")?.get(key)?;
    item.as_str().map(str::to_owned).or_else(|| item.as_integer().map(|i| i.to_string()))
}

/// Each setting's value in the file (None: not set, kache's own).
pub fn settings() -> Vec<Option<String>> {
    let doc = std::fs::read_to_string(config_path()).ok().and_then(|t| t.parse::<toml_edit::DocumentMut>().ok());
    SETTINGS
        .iter()
        .map(|s| {
            let item = doc.as_ref()?.get("cache")?.get(s.key)?;
            item.as_str().map(str::to_owned).or_else(|| item.as_integer().map(|i| i.to_string())).or_else(|| item.as_bool().map(|b| b.to_string()))
        })
        .collect()
}

/// Whether a switch is on: as set, else kache's own.
pub fn switched(s: &Setting, value: Option<&str>) -> bool {
    value.unwrap_or(s.default) == "true"
}

/// `values` (None: kache's own) written into the file's `[cache]`, the rest
/// of it, comments too, as it was. A value that doesn't read as its kind
/// is refused, saying which.
pub fn save_settings(values: &[Option<String>]) -> Result<(), String> {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let doc = set_in(&text, values)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("toml.new");
    std::fs::write(&tmp, doc).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// `values` set in config `text`: the new text.
pub fn set_in(text: &str, values: &[Option<String>]) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("kache's settings file can't be read: {e}"))?;
    if !doc.contains_key("cache") {
        doc["cache"] = toml_edit::table();
    }
    for (s, v) in SETTINGS.iter().zip(values) {
        let cache = doc["cache"].as_table_mut().ok_or("kache's settings file has a [cache] that isn't a table")?;
        let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
            cache.remove(s.key);
            continue;
        };
        let name = s.label.trim_end_matches(':');
        let name = if name.is_empty() { s.about } else { name };
        cache[s.key] = match s.kind {
            Kind::Number => toml_edit::value(v.parse::<i64>().map_err(|_| format!("{name}: a whole number, please"))?),
            Kind::Switch => toml_edit::value(v == "true"),
            Kind::Size => {
                if !size_reads(v) {
                    return Err(format!("{name}: a size such as 95GiB, please"));
                }
                toml_edit::value(v)
            }
        };
    }
    Ok(doc.to_string())
}

/// Whether `v` reads as a size kache takes: a number and a unit.
fn size_reads(v: &str) -> bool {
    let i = v.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(v.len());
    let (n, unit) = v.split_at(i);
    !n.is_empty() && n.parse::<f64>().is_ok() && ["", "B", "KB", "MB", "GB", "TB", "KiB", "MiB", "GiB", "TiB", "K", "M", "G", "T"].contains(&unit.trim())
}

/// Bytes as people read them ("688 GB").
pub fn bytes(n: u64) -> String {
    let b = n as f64;
    let (v, unit) = match b {
        b if b >= 1e12 => (b / 1e12, "TB"),
        b if b >= 1e9 => (b / 1e9, "GB"),
        b if b >= 1e6 => (b / 1e6, "MB"),
        b if b >= 1e3 => (b / 1e3, "KB"),
        _ => return format!("{n} bytes"),
    };
    if v >= 100.0 { format!("{v:.0} {unit}") } else { format!("{v:.1} {unit}") }
}

/// A span as people say it: "3.2 s", "4 min", "2 h 10 min".
pub fn span(ms: u64) -> String {
    match ms {
        0..1000 => format!("{ms} ms"),
        1000..60_000 => format!("{:.1} s", ms as f64 / 1000.0),
        60_000..3_600_000 => format!("{} min", ms / 60_000),
        _ => format!("{} h {} min", ms / 3_600_000, ms % 3_600_000 / 60_000),
    }
}

/// The demo: a day of builds, a daemon with work in flight, a clean-up.
pub fn made_up(now: u64) -> Snapshot {
    let mut events = Vec::new();
    let names = ["serde", "tokio", "olo-kit", "olo-weather", "regex", "syn", "app_server", "olo-deskbar"];
    for i in 0..400u64 {
        let at = now - (i * 211_000) % (24 * 3_600_000);
        let result = match i % 7 {
            0 | 1 | 2 => "local_hit",
            3 => "miss",
            4 => "passthrough",
            5 => "remote_hit",
            _ => "miss",
        };
        events.push(Event {
            at_ms: at,
            crate_name: names[(i % 8) as usize].into(),
            project: if i % 3 == 0 { "olo".into() } else { "shiftup".into() },
            result: result.into(),
            elapsed_ms: 200 + (i * 37) % 9000,
            compile_ms: 20_000 + (i * 53) % 9000,
            size: (i % 5) * 4_000_000,
        });
    }
    events.sort_by_key(|e| e.at_ms);
    let day = day_of(&events, now);
    let unit = UserUnit { loaded: true, active: "active".into(), sub: "running".into(), since_us: (now - 3 * 3_600_000) * 1000, pid: 4242, memory: Some(380_000_000) };
    let daemon = Daemon { version: "2.1.1".into(), size: 88_000_000_000, disk: 61_000_000_000, limit: 102_005_473_280, entries: 9_412, in_flight: vec!["olo-kit".into(), "app_server".into()] };
    let cleanup = Cleanup { at_ms: now - 40 * 60_000, freed: 19_702_859_847, held: 41_000_000_000, unreclaimable: 512 };
    let pool = Pool { size: 16, compiles: 5, tests: 4, waiting: 2 };
    Snapshot { unit: Some(unit), daemon: Some(daemon), cleanup: Some(cleanup), day, recent: events.into_iter().rev().take(RECENT).collect(), pool: Some(pool) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon's answer as it came (2026-11-15, trimmed).
    #[test]
    fn the_daemons_stats_are_read() {
        let v = serde_json::json!({"version": "2.1.1", "total_size": 688_151_103_802u64, "max_size": 102_005_473_280u64, "entry_count": 28_893,
            "blob_stats": {"total_blob_size": 542_895_485_334u64}, "in_flight": [{"crate_name": "serde"}]});
        let d = daemon_of(&v);
        assert_eq!((d.size, d.disk, d.limit, d.entries), (688_151_103_802, 542_895_485_334, 102_005_473_280, 28_893));
        assert_eq!(d.in_flight, ["serde".to_owned()]);
    }

    /// Build events by the hour; a line that isn't one skipped.
    #[test]
    fn a_day_of_builds() {
        let now = 1_794_800_000_000u64;
        let iso = |ms: u64| chrono::DateTime::from_timestamp_millis(ms as i64).unwrap().to_rfc3339();
        let text = format!(
            "{}\n{}\nnot json\n{}\n{}\n",
            serde_json::json!({"ts": iso(now - 1000), "crate_name": "serde", "root": "/home/ben/src/olo", "result": "local_hit", "elapsed_ms": 30, "compile_time_ms": 2030}),
            serde_json::json!({"ts": iso(now - 2 * 3_600_000), "crate_name": "syn", "root": "/home/ben/src/olo/target", "result": "miss", "elapsed_ms": 4000}),
            serde_json::json!({"ts": iso(now - 3000), "crate_name": "olo", "result": "passthrough"}),
            serde_json::json!({"ts": iso(now - 30 * 3_600_000), "crate_name": "old", "result": "miss"}),
        );
        let events = parse_events(&text);
        assert_eq!(events.len(), 4);
        assert_eq!(events[1].project, "olo");
        let d = day_of(&events, now);
        assert_eq!((d.hits, d.misses, d.others), (1, 1, 1), "the day's only");
        assert_eq!(d.hours.len(), 24);
        assert_eq!(d.hours[23][0] + d.hours[23][2], 2, "this hour: the hit and the passthrough");
        assert_eq!(d.rate(), Some(0.5));
        assert_eq!(d.projects.iter().map(|t| (t.name.as_str(), t.hits, t.misses, t.others)).collect::<Vec<_>>(), [("olo", 1, 1, 0), ("(no project)", 0, 0, 1)]);
        assert_eq!(d.slowest, [("syn".to_owned(), 4000)]);
        assert_eq!(d.saved_ms, 2000, "the hit's compile less the hit");
    }

    /// Settings written into the file as kache reads them, its comments
    /// and other keys kept; one emptied goes; a wrong one is refused.
    #[test]
    fn settings_keep_the_file() {
        let text = "# tuned by hand\n[cache]\n# the pool\nscheduler_permits = 16\nauto_recover_targets = true\n";
        let mut values = vec![None; SETTINGS.len()];
        let at = |k: &str| SETTINGS.iter().position(|s| s.key == k).unwrap();
        values[at("local_max_size")] = Some("120GiB".into());
        values[at("scheduler_permits")] = Some("12".into());
        values[at("scheduler_dynamic")] = Some("true".into());
        let out = set_in(text, &values).unwrap();
        assert!(out.contains("# tuned by hand") && out.contains("# the pool") && out.contains("auto_recover_targets = true"), "{out}");
        assert!(out.contains("local_max_size = \"120GiB\"") && out.contains("scheduler_permits = 12") && out.contains("scheduler_dynamic = true"), "{out}");
        values[at("scheduler_permits")] = None;
        assert!(!set_in(text, &values).unwrap().contains("scheduler_permits"), "emptied: kache's own");
        values[at("local_max_size")] = Some("lots".into());
        assert!(set_in(text, &values).unwrap_err().contains("Store limit"));
        values[at("local_max_size")] = None;
        values[at("scheduler_test_slots")] = Some("eight".into());
        assert!(set_in(text, &values).unwrap_err().contains("Test slots"));
    }

    /// The live log read as it grows; read again when rotated.
    #[test]
    fn the_live_log_is_read_as_it_grows() {
        use std::io::Write as _;
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-kache-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        let now = now_ms();
        let line = |name: &str, at: u64| format!("{}\n", serde_json::json!({"ts": chrono::DateTime::from_timestamp_millis(at as i64).unwrap().to_rfc3339(), "crate_name": name, "result": "miss"}));
        std::fs::write(&path, line("a", now - 1000) + &line("b", now - 500)).unwrap();
        let mut live = Live::default();
        assert_eq!(live.read(&path, 0).len(), 2);
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        // A line half written: not read until it's whole.
        let c = line("c", now);
        f.write_all(&c.as_bytes()[..10]).unwrap();
        assert_eq!(live.read(&path, 0).len(), 2);
        f.write_all(&c.as_bytes()[10..]).unwrap();
        let names: Vec<_> = live.read(&path, 0).iter().map(|e| e.crate_name.clone()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        // Rotated: another file there, read from its start.
        std::fs::write(dir.join("new"), line("d", now)).unwrap();
        std::fs::rename(dir.join("new"), &path).unwrap();
        let names: Vec<_> = live.read(&path, 0).iter().map(|e| e.crate_name.clone()).collect();
        assert_eq!(names, ["d"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A switch is as kache has it unset until set.
    #[test]
    fn switches_follow_kaches_own() {
        let s = |k: &str| SETTINGS.iter().find(|s| s.key == k).unwrap();
        assert!(switched(s("scheduler_dynamic"), None) && !switched(s("scheduler_dynamic"), Some("false")));
        assert!(!switched(s("count_held_in_max_size"), None) && switched(s("count_held_in_max_size"), Some("true")));
    }

    /// Slots held are read from the kernel's locks, never taken.
    #[test]
    fn the_pools_slots_held() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-kache-pool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["scheduler/permits", "scheduler/tests", "scheduler/waiters"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let files: Vec<std::fs::File> = (0..3).map(|i| std::fs::File::create(dir.join(format!("scheduler/permits/{i}"))).unwrap()).collect();
        std::fs::write(dir.join("scheduler/waiters/a"), "").unwrap();
        assert_eq!(pool(&dir, Some(16)).unwrap(), Pool { size: 16, compiles: 0, tests: 0, waiting: 1 });
        // Two leases held, as kache holds them (flock).
        for f in &files[..2] {
            assert_eq!(unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(f), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        }
        assert_eq!(pool(&dir, Some(16)).unwrap().compiles, 2);
        drop(files);
        assert_eq!(pool(&dir, Some(16)).unwrap().compiles, 0, "a lease let go");
        assert!(pool(&dir.join("nowhere"), None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words() {
        assert_eq!((bytes(688_151_103_802), bytes(19_702_859_847), bytes(512)), ("688 GB".into(), "19.7 GB".into(), "512 bytes".into()));
        assert_eq!((span(450), span(3200), span(240_000), span(7_800_000)), ("450 ms".into(), "3.2 s".into(), "4 min".into(), "2 h 10 min".into()));
        assert!(size_reads("95GiB") && size_reads("1.5 TB") && !size_reads("lots") && !size_reads("GiB"));
    }
}
