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

/// kache's daemon's socket: in its runtime folder, its settings'
/// `runtime_dir` (`~` its home), else its store's folder, as kache has it.
pub fn socket_path() -> PathBuf {
    let set = std::fs::read_to_string(config_path()).ok().and_then(|t| t.parse::<toml_edit::DocumentMut>().ok()).and_then(|d| d.get("cache")?.get("runtime_dir")?.as_str().map(str::to_owned));
    let dir = match set {
        Some(d) if d == "~" || d.starts_with("~/") => home().join(d.trim_start_matches('~').trim_start_matches('/')),
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => cache_dir(),
    };
    dir.join("daemon.sock")
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
    /// None: it looked at no entries (a sweep of locks after a restart,
    /// 2026-11-15: "0 bytes" held said of a store 600 GB over its limit).
    pub held: Option<u64>,
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
    /// What its cache key was made of (kache's `key_fields`: each part's
    /// hash), by [`KEY_FIELDS`]' index; and the crate it's of in its
    /// project (its folder, name and kind together, hashed).
    pub keys: Vec<(u8, u64)>,
    pub ident: u64,
    /// kache's own word for a miss ("unsupported"; "other" for most).
    pub miss_reason: Option<Box<str>>,
    /// Why a miss missed, worked out ([`causes`]).
    pub cause: Option<&'static str>,
}

impl Event {
    pub fn hit(&self) -> bool {
        self.result.ends_with("hit")
    }
}

/// The parts of kache's cache key it records for each build.
pub const KEY_FIELDS: &[&str] = &["args", "compiler", "crate", "diagnostics", "outcome_lints", "remap", "sources", "externs", "link", "native_bundle_audit", "env_deps"];

/// Why each miss missed (Ben's Kache request: "misses and what caused
/// them (a changed input, feature unification, a new toolchain)"): its key
/// against the last one of the same crate in the same project (`events`
/// oldest first), the first part that changed, in the order that explains
/// most (a new compiler changes everything after it); none before it,
/// "first build here"; nothing changed, "evicted" (it was there, and the
/// store let it go).
pub fn causes(events: &mut [Event]) {
    // (The last build of each crate, and of each crate with the same
    // flags: a crate built as a library and as a test alternates between
    // two, each cached, and a miss of one is explained by its own last.)
    let args = KEY_FIELDS.iter().position(|f| *f == "args").unwrap_or(0) as u8;
    let flags = |e: &Event| e.keys.iter().find(|(k, _)| *k == args).map(|(_, v)| *v);
    let mut last: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut last_alike: std::collections::HashMap<(u64, Option<u64>), usize> = std::collections::HashMap::new();
    for i in 0..events.len() {
        if events[i].result == "miss" {
            let e = &events[i];
            let alike = last_alike.get(&(e.ident, flags(e))).copied();
            events[i].cause = Some(match (e.miss_reason.as_deref(), alike, last.get(&e.ident)) {
                (Some(r), ..) if r != "other" => match r {
                    "unsupported" => "not supported by kache",
                    "not-a-compile" => "not a compile",
                    _ => "kache's own reason",
                },
                _ if e.keys.is_empty() => "not recorded",
                (_, Some(j), _) => changed(&events[j].keys, &e.keys),
                (_, None, Some(_)) => "flags or features",
                (_, None, None) => "first build here",
            });
        }
        if !events[i].keys.is_empty() {
            last.insert(events[i].ident, i);
            last_alike.insert((events[i].ident, flags(&events[i])), i);
        }
    }
}

/// The first part of a key that differs between `before` and `now`, said.
fn changed(before: &[(u8, u64)], now: &[(u8, u64)]) -> &'static str {
    let get = |keys: &[(u8, u64)], field: &str| KEY_FIELDS.iter().position(|f| *f == field).and_then(|i| keys.iter().find(|(k, _)| *k as usize == i).map(|(_, v)| *v));
    let differs = |field: &str| get(before, field) != get(now, field);
    if differs("compiler") {
        "a new compiler"
    } else if differs("args") {
        "flags or features"
    } else if differs("env_deps") {
        "the environment"
    } else if differs("externs") {
        "a dependency changed"
    } else if differs("sources") {
        "its source changed"
    } else if before != now {
        "build settings"
    } else {
        "evicted from the store"
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
    /// Why the misses missed, each cause's count, most first.
    pub causes: Vec<(&'static str, u64)>,
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
    /// Tests holding the pool's slots.
    pub held: Vec<crate::held::Held>,
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
    let daemon = ask_stats(&socket_path()).ok();
    let cleanup = std::fs::read(dir.join("gc_stats.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| cleanup_of(&v));
    let now = now_ms();
    let since = now.saturating_sub(24 * 3_600_000);
    let mut events = archived_since(&dir.join("events.jsonl.archive"), since);
    events.extend(LIVE.lock().unwrap_or_else(|e| e.into_inner()).read(&dir.join("events.jsonl"), since).iter().cloned());
    causes(&mut events);
    let day = day_of(&events, now);
    let mut recent: Vec<Event> = events.into_iter().rev().take(RECENT).collect();
    recent.shrink_to_fit();
    let permits = settings_at(&config_path(), "scheduler_permits").and_then(|v| v.parse().ok());
    let pool = pool(&dir, permits);
    let held = crate::held::held(&dir);
    Snapshot { unit, daemon, cleanup, day, recent, pool, held }
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
fn ask(socket: &std::path::Path, request: &Value, timeout: Duration) -> Result<Value, String> {
    let s = std::os::unix::net::UnixStream::connect(socket).map_err(|e| olo_kit::tr!("kache's daemon isn't answering ({})", e))?;
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    let mut w = &s;
    w.write_all(format!("{request}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(&s).read_line(&mut line).map_err(|e| olo_kit::tr!("kache's daemon didn't answer ({})", e))?;
    let v: Value = serde_json::from_str(&line).map_err(|e| olo_kit::tr!("kache's daemon answered something unreadable ({})", e))?;
    if v["ok"].as_bool() == Some(false) {
        return Err(v["error"].as_str().unwrap_or("kache's daemon refused").to_owned());
    }
    Ok(v)
}

fn ask_stats(socket: &std::path::Path) -> Result<Daemon, String> {
    // (No hours of events: counting them costs the daemon 0.17 s an ask,
    // without them 0.035 s; the day's figures are counted here, from the
    // events read anyway.)
    let request = serde_json::json!({"stats": {"include_entries": false, "include_summaries": false, "sort_by": null, "event_hours": 0, "event_secs": 0, "client_epoch": 0}});
    let v = ask(socket, &request, Duration::from_secs(5))?;
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
    let looked = n("entries_evicted") + n("entries_unreclaimable") + n("entries_pinned") + n("bytes_held") > 0;
    Cleanup { at_ms, freed: n("disk_bytes_reclaimed").max(n("bytes_freed")), held: looked.then(|| n("bytes_held")), unreclaimable: n("entries_unreclaimable") }
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
        #[serde(borrow, default)]
        key_fields: Option<std::collections::BTreeMap<Cow<'a, str>, Cow<'a, str>>>,
        #[serde(borrow, default)]
        miss_reason: Option<Cow<'a, str>>,
    }
    text.lines()
        .filter_map(|l| serde_json::from_str::<Line>(l).ok())
        .filter_map(|v| {
            let at_ms = chrono::DateTime::parse_from_rfc3339(&v.ts).ok()?.timestamp_millis().max(0) as u64;
            // (A build's root is its project's folder or its target folder.)
            let project = v.root.as_deref().and_then(|r| r.rsplit('/').find(|p| !p.is_empty() && *p != "target")).unwrap_or_default().to_owned();
            let keys: Vec<(u8, u64)> = v.key_fields.iter().flatten().filter_map(|(k, h)| Some((KEY_FIELDS.iter().position(|f| *f == k.as_ref())? as u8, u64::from_str_radix(h, 16).ok()?))).collect();
            let crate_name = v.crate_name.map(Cow::into_owned).unwrap_or_default();
            let ident = {
                use std::hash::{Hash as _, Hasher as _};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (&project, &crate_name, keys.iter().find(|(k, _)| KEY_FIELDS[*k as usize] == "crate").map(|(_, v)| *v)).hash(&mut h);
                h.finish()
            };
            Some(Event {
                at_ms,
                crate_name,
                project,
                result: v.result.into_owned(),
                elapsed_ms: v.elapsed_ms.unwrap_or(0),
                compile_ms: v.compile_time_ms.unwrap_or(0),
                size: v.size.unwrap_or(0),
                keys,
                ident,
                miss_reason: v.miss_reason.map(|r| r.into()),
                cause: None,
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
    let mut causes: std::collections::BTreeMap<&'static str, u64> = std::collections::BTreeMap::new();
    for e in events.iter().filter(|e| e.at_ms >= from && e.at_ms <= now) {
        if let Some(c) = e.cause {
            *causes.entry(c).or_default() += 1;
        }
    }
    let mut causes: Vec<(&'static str, u64)> = causes.into_iter().collect();
    causes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    d.causes = causes;
    d
}

/// Clean up now: the daemon's own clean-up, as its limit asks.
pub fn clean_up() -> Result<String, String> {
    if demo() {
        return Ok(olo_kit::words::tr("Cleaned up (made up).").into());
    }
    let v = ask(&socket_path(), &gc_request(&config_path()), Duration::from_secs(300))?;
    Ok(cleaned_up(&v))
}

/// The clean-up kache's own CLI asks for: its automatic policy (the
/// daemon's limit), with the age limit from its settings
/// (`gc_max_age_hours`, 0: none), which the daemon needs said (2026-11-15:
/// "invalid GC request: automatic GC request is missing
/// effective_max_age_hours").
pub fn gc_request(config: &std::path::Path) -> Value {
    let hours: u64 = settings_at(config, "gc_max_age_hours").and_then(|v| v.parse().ok()).unwrap_or(0);
    serde_json::json!({"gc_v2": {"max_age_hours": null, "mode": "automatic", "effective_max_age_hours": hours}})
}

/// What a clean-up did, from the daemon's answer (its passes: copies,
/// age, size).
pub fn cleaned_up(v: &Value) -> String {
    let gc = &v["gc"];
    let sum = |k: &str| ["duplicate", "age", "size"].iter().map(|p| gc[p][k].as_u64().unwrap_or(0)).sum::<u64>();
    let evicted = v["evicted"].as_u64().unwrap_or_else(|| sum("entries_evicted"));
    let freed = sum("disk_bytes_reclaimed").max(sum("bytes_freed"));
    let held = sum("entries_unreclaimable");
    let mut said = match evicted {
        0 => olo_kit::words::tr("Cleaned up: nothing to remove").to_owned(),
        n => olo_kit::words::fill(&olo_kit::words::count(n as u64, "Cleaned up: 1 entry removed, {} freed", "Cleaned up: {n} entries removed, {} freed"), &[&bytes(freed)]),
    };
    if held > 0 {
        said.push_str(&format!("; {}", olo_kit::tr!("{} kept, still shared with builds' target folders", held)));
    }
    said.push('.');
    said
}

/// Restart, start or stop the service (on a worker).
pub fn service(method: &str) -> Result<UserUnit, String> {
    if demo() {
        return Ok(made_up(now_ms()).unit.unwrap_or_default());
    }
    olo_system::systemd::user_job(UNIT, method).map_err(|e| olo_kit::tr!("kache's service: {}", e))
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
    Setting {
        key: "local_max_size",
        label: "Store limit",
        about: "The most the store keeps: past it, what's used least goes. kache's own is 5% of its disk, up to 100 GiB.",
        kind: Kind::Size,
        default: "5% of the disk",
    },
    Setting { key: "count_held_in_max_size", label: "", about: "Count what builds' target folders share toward the limit", kind: Kind::Switch, default: "false" },
    Setting { key: "gc_max_age_hours", label: "Age limit", about: "How long an entry may go unused before a clean-up takes it.", kind: Kind::Number, default: "none" },
    Setting { key: "auto_clean_idle_targets_days", label: "Idle target folders", about: "How long a build's target folder may go unused before it's cleaned.", kind: Kind::Number, default: "never" },
    Setting { key: "auto_clean_orphaned_targets", label: "", about: "Clean target folders whose project is gone", kind: Kind::Switch, default: "true" },
    Setting {
        key: "scheduler_permits",
        label: "Compiles at once",
        about: "Compiles and tests run together: the pool's slots. kache's own is one a processor.",
        kind: Kind::Number,
        default: "one a processor",
    },
    Setting { key: "scheduler_test_slots", label: "Slots a test takes", about: "The most of the pool one test binary holds.", kind: Kind::Number, default: "all" },
    Setting { key: "scheduler_test_wait_secs", label: "Test wait", about: "How long a test waits for its slots before it runs without them.", kind: Kind::Number, default: "30 minutes" },
    Setting { key: "scheduler_dynamic", label: "", about: "Let more in while the machine has room to spare", kind: Kind::Switch, default: "true" },
    Setting { key: "scheduler_cpu_percent", label: "Room to spare, up to", about: "How busy letting more in keeps the processors.", kind: Kind::Number, default: "85%" },
];

/// A setting's choices, the only ones a slider offers (Ben, 2026-11-15:
/// "what is store limit supposed to be? mb? percent? make it a slider with
/// only valid values possible"): what's written for each (None: not set,
/// kache's own, always first) and its name with its unit. A value the
/// file holds that isn't among them (set by hand) is one too, in its
/// place. `processors`: this computer's, for the pool's steps.
pub fn steps(s: &Setting, current: Option<&str>, processors: u32) -> Vec<(Option<String>, String)> {
    let own = (None, olo_kit::tr!("kache's own: {}", s.default));
    let n = |v: u64, name: String| (Some(v.to_string()), name);
    let plural = |v: u64, one: &str, many: &str| if v == 1 { format!("1 {one}") } else { format!("{v} {many}") };
    let mut steps: Vec<(Option<String>, String)> = match s.key {
        "local_max_size" => [10u64, 25, 50, 75, 100, 150, 200, 300, 500].into_iter().map(|g| (Some(format!("{g}GiB")), format!("{g} GiB"))).collect(),
        "gc_max_age_hours" => [(24u64, "1 day"), (72, "3 days"), (168, "1 week"), (336, "2 weeks"), (720, "30 days"), (2160, "90 days")].into_iter().map(|(h, name)| n(h, name.into())).collect(),
        "auto_clean_idle_targets_days" => [1u64, 2, 3, 7, 14, 30, 90].into_iter().map(|d| n(d, plural(d, "day", "days"))).collect(),
        "scheduler_permits" | "scheduler_test_slots" => (1..=u64::from(processors.max(1)) * 2).map(|k| n(k, plural(k, "slot", "slots"))).collect(),
        "scheduler_test_wait_secs" => [(10u64, "10 seconds"), (30, "30 seconds"), (60, "1 minute"), (120, "2 minutes"), (300, "5 minutes"), (600, "10 minutes"), (3600, "1 hour")]
            .into_iter()
            .map(|(v, name)| n(v, name.into()))
            .collect(),
        "scheduler_cpu_percent" => (10..=20u64).map(|k| k * 5).filter(|p| *p != 85).map(|p| n(p, format!("{p}%"))).collect(),
        _ => Vec::new(),
    };
    // (A value set by hand: kept, in its place by its size.)
    let measure = |v: &str| if s.kind == Kind::Size { size_bytes(v) } else { v.parse::<u64>().ok() };
    if let Some(cur) = current.map(str::trim).filter(|c| !c.is_empty())
        && !steps.iter().any(|(v, _)| v.as_deref() == Some(cur) || (measure(cur).is_some() && v.as_deref().and_then(measure) == measure(cur)))
    {
        // (As written: "120GiB" → "120 GiB".)
        let name = match s.kind {
            Kind::Size => {
                let i = cur.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(cur.len());
                let (n, unit) = cur.split_at(i);
                if unit.trim().is_empty() { bytes(n.parse().unwrap_or(0)) } else { format!("{n} {}", unit.trim()) }
            }
            _ => cur.to_owned(),
        };
        let at = steps.iter().position(|(v, _)| v.as_deref().and_then(measure).zip(measure(cur)).is_some_and(|(a, b)| a > b)).unwrap_or(steps.len());
        steps.insert(at, (Some(cur.to_owned()), name));
    }
    steps.insert(0, own);
    steps
}

/// Which of `steps` the file's `current` value is (0: kache's own).
pub fn step_of(steps: &[(Option<String>, String)], current: Option<&str>, size: bool) -> usize {
    let measure = |v: &str| if size { size_bytes(v) } else { v.trim().parse::<u64>().ok() };
    let Some(cur) = current.map(str::trim).filter(|c| !c.is_empty()) else { return 0 };
    steps.iter().position(|(v, _)| v.as_deref() == Some(cur) || (measure(cur).is_some() && v.as_deref().and_then(measure) == measure(cur))).unwrap_or(0)
}

/// A size as kache reads one, in bytes: "95GiB", "1.5 TB", "1000".
pub fn size_bytes(v: &str) -> Option<u64> {
    let v = v.trim();
    if let Ok(b) = v.parse::<u64>() {
        return Some(b);
    }
    let i = v.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(v.len());
    let (n, unit) = v.split_at(i);
    let n: f64 = n.parse().ok()?;
    let f: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        "t" | "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024f64.powi(2),
        "gib" => 1024f64.powi(3),
        "tib" => 1024f64.powi(4),
        _ => return None,
    };
    Some((n * f) as u64)
}

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
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| olo_kit::tr!("kache's settings file can't be read: {}", e))?;
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
            Kind::Number => toml_edit::value(v.parse::<i64>().map_err(|_| olo_kit::tr!("{}: a whole number, please", name))?),
            Kind::Switch => toml_edit::value(v == "true"),
            Kind::Size => {
                if !size_reads(v) {
                    return Err(olo_kit::tr!("{}: a size such as 95GiB, please", name));
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
        _ => olo_kit::tr!("{} h {} min", ms / 3_600_000, ms % 3_600_000 / 60_000),
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
            project: if i % 3 == 0 { "olo".into() } else { "garden".into() },
            result: result.into(),
            elapsed_ms: 200 + (i * 37) % 9000,
            compile_ms: 20_000 + (i * 53) % 9000,
            size: (i % 5) * 4_000_000,
            cause: (result == "miss")
                .then(|| ["its source changed", "a dependency changed", "its source changed", "first build here", "flags or features", "evicted from the store"][(i % 6) as usize]),
            ..Event::default()
        });
    }
    events.sort_by_key(|e| e.at_ms);
    let day = day_of(&events, now);
    let unit = UserUnit { loaded: true, active: "active".into(), sub: "running".into(), since_us: (now - 3 * 3_600_000) * 1000, pid: 4242, memory: Some(380_000_000) };
    let daemon = Daemon { version: "2.1.1".into(), size: 88_000_000_000, disk: 61_000_000_000, limit: 102_005_473_280, entries: 9_412, in_flight: vec!["olo-kit".into(), "app_server".into()] };
    let cleanup = Cleanup { at_ms: now - 40 * 60_000, freed: 19_702_859_847, held: Some(41_000_000_000), unreclaimable: 512 };
    let pool = Pool { size: 16, compiles: 5, tests: 4, waiting: 2 };
    let held = vec![crate::held::Held { pid: 41_002, name: "olo_kit".into(), project: "olo".into(), slots: 4, since_ms: now - 3 * 60_000 }];
    Snapshot { unit: Some(unit), daemon: Some(daemon), cleanup: Some(cleanup), day, recent: events.into_iter().rev().take(RECENT).collect(), pool: Some(pool), held }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test's folder on disk, removed however the test ends (a failed
    /// one left `/var/tmp/olo-kache-pool-PID` behind, 2026-11-15).
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = PathBuf::from(format!("/var/tmp/olo-kache-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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
        assert!(set_in(text, &values).unwrap_err().contains("Slots a test takes"));
    }

    /// The live log read as it grows; read again when rotated.
    #[test]
    fn the_live_log_is_read_as_it_grows() {
        use std::io::Write as _;
        let scratch = Scratch::new("live");
        let dir = scratch.0.clone();
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
    }

    /// A switch is as kache has it unset until set.
    #[test]
    fn switches_follow_kaches_own() {
        let s = |k: &str| SETTINGS.iter().find(|s| s.key == k).unwrap();
        assert!(switched(s("scheduler_dynamic"), None) && !switched(s("scheduler_dynamic"), Some("false")));
        assert!(!switched(s("count_held_in_max_size"), None) && switched(s("count_held_in_max_size"), Some("true")));
    }

    /// Each miss's cause from its key against the same crate's last.
    #[test]
    fn why_misses_missed() {
        let now = 1_794_800_000_000u64;
        let line = |ts: u64, result: &str, keys: &str, extra: &str| {
            format!(
                r#"{{"ts":"{}","crate_name":"serde","root":"/home/ben/src/olo","result":"{result}","key_fields":{{{keys}}}{extra}}}"#,
                chrono::DateTime::from_timestamp_millis((now - 10_000 + ts) as i64).unwrap().to_rfc3339()
            )
        };
        let k = |compiler: &str, args: &str, sources: &str, externs: &str| format!(r#""crate":"aa","compiler":"{compiler}","args":"{args}","sources":"{sources}","externs":"{externs}""#);
        let text = [
            line(1000, "miss", &k("01", "02", "03", "04"), ""),
            line(2000, "miss", &k("01", "02", "05", "04"), ""),
            line(3000, "local_hit", &k("01", "02", "05", "04"), ""),
            line(4000, "miss", &k("01", "02", "05", "06"), ""),
            line(5000, "miss", &k("01", "07", "08", "06"), ""),
            line(6000, "miss", &k("09", "07", "08", "06"), ""),
            line(7000, "miss", &k("09", "07", "08", "06"), ""),
            line(8000, "miss", &k("09", "07", "08", "06"), r#","miss_reason":"unsupported""#),
        ]
        .join("\n");
        let mut events = parse_events(&text);
        causes(&mut events);
        let said: Vec<_> = events.iter().map(|e| e.cause.unwrap_or("-")).collect();
        assert_eq!(said, ["first build here", "its source changed", "-", "a dependency changed", "flags or features", "a new compiler", "evicted from the store", "not supported by kache"]);
        let d = day_of(&events, now);
        assert_eq!(d.causes.iter().map(|(_, n)| *n).sum::<u64>(), 7, "every miss, counted once");
        assert!(d.causes.iter().all(|(_, n)| *n == 1));
        // A library and its test alternating: each against its own last.
        let text = [
            line(1000, "miss", &k("01", "0a", "03", "04"), ""),
            line(2000, "miss", &k("01", "0b", "03", "04"), ""),
            line(3000, "miss", &k("01", "0a", "05", "04"), ""),
            line(4000, "miss", &k("01", "0b", "05", "04"), ""),
        ]
        .join("\n");
        let mut events = parse_events(&text);
        causes(&mut events);
        let said: Vec<_> = events.iter().map(|e| e.cause.unwrap_or("-")).collect();
        assert_eq!(said, ["first build here", "flags or features", "its source changed", "its source changed"]);
    }

    /// Slots held are read from the kernel's locks, never taken.
    #[test]
    fn the_pools_slots_held() {
        let scratch = Scratch::new("pool");
        let dir = scratch.0.clone();
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
    }

    /// The clean-up asked for as kache's CLI asks (its age limit said),
    /// and what it did said; a record that looked at no entries holds
    /// nothing known.
    #[test]
    fn a_clean_up() {
        let dir = Scratch::new("gc");
        let config = dir.0.join("config.toml");
        assert_eq!(gc_request(&config)["gc_v2"]["effective_max_age_hours"], 0, "no settings: none");
        std::fs::write(&config, "[cache]\ngc_max_age_hours = 336\n").unwrap();
        let r = gc_request(&config);
        assert_eq!((r["gc_v2"]["mode"].as_str(), r["gc_v2"]["effective_max_age_hours"].as_u64()), (Some("automatic"), Some(336)));
        let answer = serde_json::json!({"ok": true, "evicted": 309, "gc": {"mode": "automatic",
            "duplicate": {"entries_evicted": 0, "bytes_freed": 0},
            "age": {"entries_evicted": 9, "bytes_freed": 702_859_847u64, "disk_bytes_reclaimed": 702_859_847u64},
            "size": {"entries_evicted": 300, "bytes_freed": 19_000_000_000u64, "disk_bytes_reclaimed": 19_000_000_000u64, "entries_unreclaimable": 4548}}});
        assert_eq!(cleaned_up(&answer), "Cleaned up: 309 entries removed, 19.7 GB freed; 4548 kept, still shared with builds' target folders.");
        assert_eq!(cleaned_up(&serde_json::json!({"ok": true, "evicted": 0, "gc": {}})), "Cleaned up: nothing to remove.");
        // The record after a restart (locks swept, no entries looked at).
        let swept = serde_json::json!({"last_run": "2026-11-16T04:04:53.998076260+00:00", "entries_evicted": 0, "bytes_held": 0, "entries_unreclaimable": 0, "key_locks_removed": 1314});
        assert_eq!(cleanup_of(&swept).held, None);
        let sized = serde_json::json!({"last_run": "2026-11-16T02:45:11.437513609+00:00", "entries_evicted": 309, "bytes_held": 450_481_390_286u64, "entries_unreclaimable": 4548});
        assert_eq!(cleanup_of(&sized).held, Some(450_481_390_286));
    }

    /// Only valid steps, kache's own first; a value set by hand kept in
    /// its place.
    #[test]
    fn settings_steps() {
        let s = |k: &str| SETTINGS.iter().find(|s| s.key == k).unwrap();
        let store = steps(s("local_max_size"), None, 16);
        assert_eq!(store[0], (None, "kache's own: 5% of the disk".into()));
        assert_eq!(store[1], (Some("10GiB".into()), "10 GiB".into()));
        assert_eq!(step_of(&store, Some("100GiB"), true), 5);
        assert_eq!(step_of(&store, None, true), 0);
        // "120GiB" by hand: between 100 and 150.
        let hand = steps(s("local_max_size"), Some("120GiB"), 16);
        let at = step_of(&hand, Some("120GiB"), true);
        assert_eq!((hand[at - 1].1.as_str(), hand[at].1.as_str(), hand[at + 1].1.as_str()), ("100 GiB", "120 GiB", "150 GiB"));
        let permits = steps(s("scheduler_permits"), Some("16"), 16);
        assert_eq!((permits.len(), step_of(&permits, Some("16"), false), permits[16].1.as_str()), (33, 16, "16 slots"));
        let wait = steps(s("scheduler_test_wait_secs"), Some("120"), 16);
        assert_eq!(wait[step_of(&wait, Some("120"), false)].1, "2 minutes");
        let room = steps(s("scheduler_cpu_percent"), None, 16);
        assert!(room.iter().skip(1).all(|(v, _)| v.as_deref().is_some_and(|v| v.parse::<u32>().is_ok_and(|p| (50..=100).contains(&p) && p % 5 == 0))));
        let idle = steps(s("auto_clean_idle_targets_days"), Some("1"), 16);
        assert_eq!(idle[step_of(&idle, Some("1"), false)].1, "1 day");
    }

    #[test]
    fn words() {
        assert_eq!((bytes(688_151_103_802), bytes(19_702_859_847), bytes(512)), ("688 GB".into(), "19.7 GB".into(), "512 bytes".into()));
        assert_eq!((span(450), span(3200), span(240_000), span(7_800_000)), ("450 ms".into(), "3.2 s".into(), "4 min".into(), "2 h 10 min".into()));
        assert!(size_reads("95GiB") && size_reads("1.5 TB") && !size_reads("lots") && !size_reads("GiB"));
    }
}
