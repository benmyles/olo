//! olo-complete: brush's command suggestions (Preferences ▸ Terminal ▸
//! "Suggest commands with Olo's model"). One per person, on
//! `$XDG_RUNTIME_DIR/olo-complete.sock` (olo_complete::protocol), started
//! by brush or Preferences when the option is on, gone when it's turned
//! off.
//!
//! It keeps Olo's record of what ran (`~/.local/state/olo/complete/
//! history.jsonl`: each command with its folder, time and shell, as brush
//! tells it; made from the history files the first time), the index and
//! the examples made from it, and the weights (`model.json`: the
//! hand-written ones until the first fine-tune). A suggestion is
//! answered at once, computed with the processor's SIMD (about a
//! microsecond). What's typed makes examples; a minute after the first
//! new one, a quick fine-tune on a thread of its own (the model taken
//! only to read its weights and put the new ones in, so suggestions
//! never wait on it); with nothing new it sleeps, costing nothing.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use olo_complete::{
    history::{self, Entry},
    index::{Context, Index},
    kernels::{Cpu, Weights},
    protocol::{self, Reply, Request, Settings, Stats},
    train::{self, Backend, CpuBackend, Example, Plan, Tuned},
};
use serde::{Deserialize, Serialize};

/// Examples kept to learn from (the newest).
const KEEP: usize = 8000;
/// A quick fine-tune's: the newest examples it learns from.
const QUICK_WINDOW: usize = 3000;
/// How long after the first new command a quick fine-tune waits, so a
/// minute's typing is learned in one go.
const QUICK_AFTER: Duration = Duration::from_secs(60);
/// Latencies kept, for the percentiles.
const LATENCIES: usize = 512;

/// The weights on disk, and how they came.
#[derive(Serialize, Deserialize)]
struct Saved {
    weights: Weights,
    #[serde(default)]
    last: Option<Tuned>,
    #[serde(default)]
    at: i64,
}

struct Model {
    ix: Index,
    weights: Weights,
    tuned: bool,
    examples: Vec<Example>,
    /// New examples since the last fine-tune, and when the first came.
    pending: usize,
    since: Option<Instant>,
}

#[derive(Default)]
struct Counters {
    offered: u64,
    accepted: u64,
    accepted_words: u64,
    latencies: Vec<u32>,
    fine_tunes: u64,
    last_tune: Option<Tuned>,
    last_tune_at: i64,
}

struct Daemon {
    model: Mutex<Model>,
    /// Woken when there's something new to learn.
    learn: Condvar,
    settings: Mutex<Settings>,
    counters: Mutex<Counters>,
}

fn record_path() -> PathBuf {
    protocol::state_dir().join("history.jsonl")
}

fn model_path() -> PathBuf {
    protocol::state_dir().join("model.json")
}

/// Olo's record; made from the history files the first time.
fn load_record() -> Vec<Entry> {
    match std::fs::read(record_path()) {
        Ok(bytes) => history::parse_record(&String::from_utf8_lossy(&bytes)),
        Err(_) => {
            let seeded = history::read_history_files(&protocol::home_dir());
            let _ = std::fs::create_dir_all(protocol::state_dir());
            let text: String = seeded.iter().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect();
            let _ = std::fs::write(record_path(), text);
            seeded
        }
    }
}

fn append_record(e: &Entry) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(record_path())
        && let Ok(line) = serde_json::to_string(e)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn save_weights(w: &Weights, last: Option<&Tuned>) {
    let saved = Saved { weights: w.clone(), last: last.cloned(), at: history::now() };
    let _ = std::fs::create_dir_all(protocol::state_dir());
    let tmp = model_path().with_extension("json.new");
    if std::fs::write(&tmp, serde_json::to_vec(&saved).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, model_path());
    }
}

impl Daemon {
    fn start(settings: Settings) -> Daemon {
        let entries = load_record();
        let (mut examples, ix) = train::replay(&entries);
        if examples.len() > KEEP {
            examples.drain(..examples.len() - KEEP);
        }
        let saved: Option<Saved> = std::fs::read(model_path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let saved = saved.filter(|s| s.weights.sound());
        let tuned = saved.is_some();
        let mut counters = Counters::default();
        if let Some(s) = &saved {
            counters.last_tune = s.last.clone();
            counters.last_tune_at = s.at;
        }
        Daemon {
            model: Mutex::new(Model { ix, weights: saved.map_or_else(Weights::baseline, |s| s.weights), tuned, examples, pending: 0, since: None }),
            learn: Condvar::new(),
            settings: Mutex::new(settings),
            counters: Mutex::new(counters),
        }
    }

    fn suggest(&self, line: &str, cwd: Option<&str>, session: u64) -> Reply {
        let t0 = Instant::now();
        let m = self.model.lock().unwrap();
        let cx = Context { line, cwd, session, at: history::now() };
        let candidates = olo_complete::suggest(&m.ix, &mut CpuBackend(Cpu::Simd), &m.weights, &cx, 5);
        drop(m);
        let rest = candidates.first().and_then(|c| c.strip_prefix(line)).unwrap_or("").to_owned();
        if !rest.is_empty() {
            self.counters.lock().unwrap().offered += 1;
        }
        Reply::Suggestion { rest, candidates, micros: t0.elapsed().as_micros() as u64 }
    }

    /// How long a suggestion took, from its question read to its answer
    /// sent (what Activity Monitor shows).
    fn took(&self, t: Duration) {
        let mut c = self.counters.lock().unwrap();
        c.latencies.push(t.as_micros().min(u32::MAX as u128) as u32);
        if c.latencies.len() > LATENCIES {
            c.latencies.remove(0);
        }
    }

    /// A command entered: recorded, its examples made (from what was
    /// known before it), then known.
    fn ran(&self, command: &str, cwd: Option<String>, session: u64) {
        let Some(command) = history::worth_keeping(command) else { return };
        let e = Entry { cwd, at: history::now(), session, ..Entry::new(command) };
        append_record(&e);
        let mut m = self.model.lock().unwrap();
        let mut made = 0;
        if m.ix.id(&e.command).is_some() {
            for k in train::typing_points(&e.command) {
                if let Some(ex) = train::example_at(&m.ix, &e, k) {
                    m.examples.push(ex);
                    made += 1;
                }
            }
        }
        if m.examples.len() > KEEP {
            let over = m.examples.len() - KEEP;
            m.examples.drain(..over);
        }
        m.ix.add(&e);
        if made > 0 {
            m.pending += made;
            m.since.get_or_insert_with(Instant::now);
            self.learn.notify_all();
        }
    }

    /// Fine-tune: from the hand-written weights on everything (`first`),
    /// or from the current ones on the newest.
    fn fine_tune(&self, first: bool) -> Tuned {
        let (start, train_set, held) = {
            let mut m = self.model.lock().unwrap();
            m.pending = 0;
            m.since = None;
            let from = if first { 0 } else { m.examples.len().saturating_sub(QUICK_WINDOW) };
            let (train_set, held) = train::split(m.examples[from..].to_vec());
            (if first { Weights::baseline() } else { m.weights.clone() }, train_set, held)
        };
        let plan = if first { Plan::FIRST } else { Plan::QUICK };
        let (w, report) = train::fine_tune(&mut CpuBackend(Cpu::Simd), &start, &train_set, &held, plan);
        {
            let mut m = self.model.lock().unwrap();
            if report.kept || first {
                m.weights = w.clone();
                m.tuned = true;
            }
        }
        save_weights(&self.model.lock().unwrap().weights, Some(&report));
        let mut c = self.counters.lock().unwrap();
        c.fine_tunes += 1;
        c.last_tune = Some(report.clone());
        c.last_tune_at = history::now();
        report
    }

    /// The trainer: asleep until something new is typed, then a fine-tune
    /// a minute after the first of it.
    fn trainer(self: Arc<Self>) {
        loop {
            let m = self.model.lock().unwrap();
            let m = self.learn.wait_while(m, |m| m.pending == 0).unwrap();
            let due = m.since.unwrap_or_else(Instant::now) + QUICK_AFTER;
            let wait = due.saturating_duration_since(Instant::now());
            let (m, _) = self.learn.wait_timeout_while(m, wait, |m| m.pending > 0 && Instant::now() < due).unwrap();
            let go = m.pending > 0;
            drop(m);
            if go {
                self.fine_tune(false);
            }
        }
    }

    fn stats(&self) -> Stats {
        let m = self.model.lock().unwrap();
        let c = self.counters.lock().unwrap();
        let settings = *self.settings.lock().unwrap();
        let mut sorted = c.latencies.clone();
        sorted.sort_unstable();
        let pct = |p: f32| sorted.get(((sorted.len() as f32 - 1.0) * p).round() as usize).copied().unwrap_or(0);
        Stats {
            enabled: settings.enabled,
            backend: CpuBackend(Cpu::Simd).name(),
            offered: c.offered,
            accepted: c.accepted,
            accepted_words: c.accepted_words,
            latency_p50_us: pct(0.5),
            latency_p95_us: pct(0.95),
            latency_worst_us: sorted.last().copied().unwrap_or(0),
            latency_count: c.latencies.len(),
            latencies_us: c.latencies.iter().rev().take(120).rev().copied().collect(),
            known: m.ix.len(),
            seen: m.ix.seen(),
            fine_tunes: c.fine_tunes,
            last_tune: c.last_tune.clone(),
            last_tune_at: c.last_tune_at,
            pending: m.pending,
            tuned: m.tuned,
        }
    }

    /// The settings read again: off, it goes (the reply first).
    fn reload(&self) -> bool {
        let new = Settings::load();
        *self.settings.lock().unwrap() = new;
        new.enabled
    }
}

/// One connection: a request, its reply. (Learn's reply comes when the
/// fine-tune is done, on a thread of its own.)
fn serve(d: &Arc<Daemon>, stream: UnixStream) -> bool {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return true;
    }
    let t0 = Instant::now();
    let Ok(req) = serde_json::from_str::<Request>(&line) else {
        reply(&stream, &Reply::Error { message: "not a request".into() });
        return true;
    };
    let mut keep_going = true;
    let r = match req {
        Request::Suggest { line, cwd, session } => d.suggest(&line, cwd.as_deref(), session),
        Request::Ran { command, cwd, session } => {
            d.ran(&command, cwd, session);
            Reply::Ok
        }
        Request::Ended { session, status } => {
            d.model.lock().unwrap().ix.ended(session, status);
            Reply::Ok
        }
        Request::Accepted { whole, .. } => {
            let mut c = d.counters.lock().unwrap();
            if whole {
                c.accepted += 1;
            } else {
                c.accepted_words += 1;
            }
            Reply::Ok
        }
        Request::Stats => Reply::Stats(d.stats()),
        Request::Learn => {
            let d = d.clone();
            let _ = std::thread::Builder::new().name("olo-complete-learn".into()).spawn(move || {
                let report = d.fine_tune(true);
                reply(&stream, &Reply::Learned(report));
            });
            return true;
        }
        Request::Reload => {
            keep_going = d.reload();
            Reply::Ok
        }
    };
    reply(&stream, &r);
    if matches!(r, Reply::Suggestion { .. }) {
        d.took(t0.elapsed());
    }
    keep_going
}

fn reply(mut stream: &UnixStream, r: &Reply) {
    if let Ok(mut b) = serde_json::to_vec(r) {
        b.push(b'\n');
        let _ = stream.write_all(&b);
    }
}

fn main() {
    let settings = Settings::load();
    if !settings.enabled {
        return;
    }
    let Some(path) = protocol::socket_path() else {
        eprintln!("olo-complete: no runtime folder (XDG_RUNTIME_DIR) for its socket");
        std::process::exit(1);
    };
    // One a person: another already answering, this one goes.
    if protocol::running() {
        return;
    }
    let _ = std::fs::remove_file(&path);
    let d = Arc::new(Daemon::start(settings));
    let Ok(listener) = UnixListener::bind(&path) else {
        eprintln!("olo-complete: can't listen on {}", path.display());
        std::process::exit(1);
    };
    {
        let d = d.clone();
        let _ = std::thread::Builder::new().name("olo-complete-train".into()).spawn(move || d.trainer());
    }
    let gone = SocketWatch::new(&path);
    loop {
        match gone.as_ref().map_or(Ok(true), |g| g.wait(&listener)) {
            // Its socket removed (the runtime folder goes at logout, or
            // it was taken over): nobody can ask any more.
            Err(()) => return,
            Ok(false) => continue,
            Ok(true) => {}
        }
        let Ok((stream, _)) = listener.accept() else { continue };
        if !serve(&d, stream) {
            break;
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// Told (inotify) when the socket's name goes from its folder, so it
/// doesn't linger after logout; asleep in the meantime, costing nothing.
struct SocketWatch {
    fd: std::os::fd::OwnedFd,
    name: std::ffi::OsString,
}

impl SocketWatch {
    fn new(path: &std::path::Path) -> Option<SocketWatch> {
        use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};
        let dir = std::ffi::CString::new(path.parent()?.as_os_str().as_bytes()).ok()?;
        // SAFETY: plain syscalls; the fd is owned from here.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        let mask = libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF;
        // SAFETY: a valid fd and a NUL-terminated path.
        if unsafe { libc::inotify_add_watch(std::os::fd::AsRawFd::as_raw_fd(&fd), dir.as_ptr(), mask) } < 0 {
            return None;
        }
        Some(SocketWatch { fd, name: path.file_name()?.to_owned() })
    }

    /// Asleep until someone connects (Ok(true)) or the socket goes
    /// (Err); Ok(false): woken for something else.
    fn wait(&self, listener: &UnixListener) -> Result<bool, ()> {
        use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
        let mut fds = [libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: two valid pollfds.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            return Ok(false);
        }
        if fds[1].revents != 0 {
            let mut buf = [0u8; 4096];
            // SAFETY: reading into a buffer of its size.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            let head = std::mem::size_of::<libc::inotify_event>();
            let mut at = 0;
            while n > 0 && at + head <= n as usize {
                // SAFETY: the kernel wrote whole events; read unaligned.
                let ev: libc::inotify_event = unsafe { std::ptr::read_unaligned(buf[at..].as_ptr().cast()) };
                let name = &buf[at + head..(at + head + ev.len as usize).min(n as usize)];
                let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
                if ev.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 || name == self.name.as_bytes() {
                    return Err(());
                }
                at += head + ev.len as usize;
            }
        }
        Ok(fds[0].revents != 0)
    }
}
