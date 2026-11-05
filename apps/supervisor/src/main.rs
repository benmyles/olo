//! olo-supervisor: keeps the desktop responsive (Preferences ▸
//! Performance; off by default). The session starts it and it waits,
//! following `performance.json`, until turned on (in Preferences, or
//! Deskbar's item). Then, every two seconds, it looks at the person's
//! processes and the system's memory (`sample`):
//!
//! - **Priorities**: builds and other background work (by name, and a
//!   long CPU-bound run that isn't the active window's) lowered
//!   (`priority`); the active window's app and what it started, and Olo
//!   itself, never. Followed as the active window changes (app_server's
//!   events).
//! - **Watchdog** (`detect`): stuck processes, leaks, memory the system
//!   is short of, zombies, each from several signals with evidence and a
//!   confidence; an urgent one asks in an alert (Stop Process, Show in
//!   Activity Monitor, Ignore This Process), once a process and kind.
//!
//! What it does is published in `olo-supervisor.json` (the runtime
//! folder) for Deskbar, Activity Monitor and agents; `--status [--json]`
//! prints it. Turned off, or ending, it puts back what it changed (the
//! scheduling and I/O priorities; a raised nice stays until its process
//! ends).

mod detect;
mod priority;
mod sample;

use olo_sync::Mutex;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_protocol::performance::{Finding, FindingKind, LogEntry, Lowered, PerformanceSettings, Raised, SupervisorState};
use olo_ui::{gpui::App, instance, widgets};

const APP_ID: &str = "olo.supervisor";
const EVERY: Duration = Duration::from_secs(2);
/// How much of each process's past is kept: recent samples for spins,
/// one a minute for leaks.
const FINE: f64 = 15.0 * 60.0;
const COARSE: f64 = 3.0 * 3600.0;
const COARSE_EVERY: f64 = 60.0;
/// A run with no window, this busy (% of a core) this long (s), is
/// background work too.
const BUSY_CPU: f64 = 80.0;
const BUSY_FOR: f64 = 60.0;
const LOG_KEPT: usize = 50;

const USAGE: &str = "usage: olo-supervisor [--status [--json] | --on | --off]
  (none)         run (the session starts it; it waits until turned on)
  --status       what it is doing: the processes lowered, its findings
                 and their evidence (--json: as it publishes them)
  --on, --off    turn it on or off (Preferences ▸ Performance)";

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// What the app hears.
enum Msg {
    /// A finding to ask the person about.
    Urgent(Finding),
}

/// One process followed.
struct Followed {
    name: String,
    /// Its command line, read when first seen (it doesn't change).
    command: String,
    ppid: i32,
    start: u64,
    fine: VecDeque<detect::Sample>,
    coarse: VecDeque<(f64, u64)>,
}

/// The supervisor's work, on its own thread.
struct Supervisor {
    settings: Arc<Mutex<PerformanceSettings>>,
    /// The active window's app: its pid and name.
    focus: Arc<Mutex<Option<(i32, String)>>>,
    proxy: olo_ui::Proxy,
    uid: u32,
    clock: Instant,
    followed: HashMap<i32, Followed>,
    lowered: HashMap<i32, Lowered>,
    /// Olo's own programs found above their tier and raised (or that
    /// couldn't be, noted once).
    raised: HashMap<i32, Raised>,
    /// When to look again at one above its tier: a minute after rtkit
    /// said no; never for another session's.
    raise_again: HashMap<i32, f64>,
    /// The voice services' sound threads looked at, (pid, tid), and when
    /// to look again: never once realtime; a minute after rtkit said no
    /// (it may have room by then: Olo Doctor's fix gives it more).
    realtime: HashMap<(i32, u32), f64>,
    /// When each zombie was first seen.
    zombies: HashMap<i32, f64>,
    /// Findings by process and kind, and those already asked about.
    findings: HashMap<(i32, FindingKind), Finding>,
    asked: HashSet<(i32, u64, FindingKind)>,
    /// What the supervisor before this one already asked about (the same
    /// process by its pid and name): not asked again after an upgrade.
    asked_before: Vec<(i32, String, FindingKind)>,
    log: VecDeque<LogEntry>,
    swapped: (u64, f64),
    published: Option<SupervisorState>,
    running: bool,
    /// Programs that are Olo's own (beside this one): never lowered.
    ours: Option<std::path::PathBuf>,
    /// Tests: only this process and what it started are looked at
    /// (`OLO_SUPERVISOR_ONLY_UNDER`), never the person's own.
    only_under: Option<i32>,
}

impl Supervisor {
    fn note(&mut self, text: String) {
        tracing::info!("supervisor: {text}");
        self.log.push_back(LogEntry { at: now_secs(), text });
        while self.log.len() > LOG_KEPT {
            self.log.pop_front();
        }
    }

    /// Put back what was lowered and forget it all (turned off).
    fn stand_down(&mut self) {
        let lowered: Vec<Lowered> = self.lowered.drain().map(|(_, l)| l).collect();
        for l in &lowered {
            priority::restore(l.pid);
        }
        if !lowered.is_empty() {
            let kept: Vec<String> = lowered.iter().filter(|l| l.nice != l.nice_before).map(|l| l.name.clone()).collect();
            let mut text = format!("Turned off: {} put back to normal scheduling and I/O", lowered.len());
            if !kept.is_empty() {
                text.push_str(&format!(" ({} keep their lower nice until they end)", kept.join(", ")));
            }
            self.note(text);
        }
        self.findings.clear();
        self.followed.clear();
        self.zombies.clear();
        self.running = false;
        self.publish(None);
    }

    /// Whether `p` is Olo's own program (never lowered, never stopped).
    fn olo(&self, p: &sample::Proc) -> bool {
        if p.name.starts_with("olo-") || p.pid == std::process::id() as i32 {
            return true;
        }
        let exe = std::fs::read_link(format!("/proc/{}/exe", p.pid)).ok();
        exe.zip(self.ours.as_ref()).is_some_and(|(e, d)| e.starts_with(d))
    }

    fn tick(&mut self) {
        let settings = self.settings.lock().unwrap().clone();
        if !settings.enabled {
            if self.running {
                self.stand_down();
            }
            return;
        }
        if !self.running {
            self.running = true;
            self.note("Turned on".into());
        }
        let t = self.clock.elapsed().as_secs_f64();
        let mut procs = sample::processes(self.uid, t);
        if let Some(root) = self.only_under {
            let parents: HashMap<i32, i32> = procs.iter().map(|p| (p.pid, p.ppid)).collect();
            let under = |mut pid: i32| {
                for _ in 0..64 {
                    if pid == root {
                        return true;
                    }
                    match parents.get(&pid) {
                        Some(&pp) if pp > 1 => pid = pp,
                        _ => return false,
                    }
                }
                false
            };
            procs.retain(|p| under(p.pid));
        }
        // The system, its swapping a rate since last time.
        let out = sample::swapped_out();
        let rate = if self.swapped.1 > 0.0 && t > self.swapped.1 { (out.saturating_sub(self.swapped.0)) as f32 / (t - self.swapped.1) as f32 } else { 0.0 };
        self.swapped = (out, t);
        let system = sample::system(rate);

        // Who's who: children by parent, the active window's app's tree.
        let by_pid: HashMap<i32, &sample::Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
        for p in &procs {
            children.entry(p.ppid).or_default().push(p.pid);
        }
        let focus = self.focus.lock().unwrap().clone();
        let mut favoured = HashSet::new();
        if let Some((pid, _)) = &focus {
            let mut stack = vec![*pid];
            while let Some(x) = stack.pop() {
                if favoured.insert(x) {
                    stack.extend(children.get(&x).into_iter().flatten());
                }
            }
        }

        // Follow each one (a pid used again is another process).
        let alive: HashSet<i32> = procs.iter().map(|p| p.pid).collect();
        self.followed.retain(|pid, f| alive.contains(pid) && by_pid.get(pid).is_some_and(|p| p.start == f.start));
        for p in &procs {
            let f = self.followed.entry(p.pid).or_insert_with(|| Followed {
                name: p.name.clone(),
                command: sample::command(p.pid),
                ppid: p.ppid,
                start: p.start,
                fine: VecDeque::new(),
                coarse: VecDeque::new(),
            });
            f.ppid = p.ppid;
            let mut s = p.sample;
            // A busy one's switches, all its threads'.
            if let Some(last) = f.fine.back()
                && s.t > last.t
                && (s.cpu - last.cpu) / (s.t - last.t) * 100.0 >= 50.0
                && let Some(all) = sample::all_switches(p.pid)
            {
                s.switches = all;
            }
            f.fine.push_back(s);
            while f.fine.front().is_some_and(|x| x.t < t - FINE) {
                f.fine.pop_front();
            }
            if f.coarse.back().is_none_or(|(ct, _)| t - ct >= COARSE_EVERY) {
                f.coarse.push_back((t, s.memory));
                while f.coarse.front().is_some_and(|x| x.0 < t - COARSE) {
                    f.coarse.pop_front();
                }
            }
        }

        // Olo's own tiers first (olo_protocol::performance::tier): each
        // asks for itself as it starts, so one found above its tier is
        // one rtkit refused then, or one lowered since (reniced); raised
        // here, its main thread (the one that matters: every input event
        // and frame, and in app_server the touchpad's palm model, judged
        // as each frame comes; and rtkit allows a handful of actions a
        // burst, which raising every thread of every program blew
        // through, "Device or resource busy"). Looked at every pass, so
        // it stays raised; one rtkit refused, a minute later. Only this
        // session's programs: a headless test session's are the person's
        // too, and not for this supervisor (2026-11-01).
        self.raised.retain(|pid, _| alive.contains(pid));
        self.raise_again.retain(|pid, _| alive.contains(pid));
        for p in &procs {
            let Some(nice) = olo_protocol::performance::tier(&p.name) else { continue };
            if p.state == 'Z' || self.raise_again.get(&p.pid).is_some_and(|at| t < *at) {
                continue;
            }
            let Some(before) = olo_system::priority::nice_of(p.pid as u32).filter(|n| *n > nice) else { continue };
            if !same_session(p.pid) {
                self.raise_again.insert(p.pid, f64::INFINITY);
                continue;
            }
            // (Raised before and found above its tier again: lowered.)
            let again = self.raised.get(&p.pid).is_some_and(|r| r.nice < r.nice_before);
            let refused = self.raised.get(&p.pid).is_some_and(|r| r.nice == r.nice_before);
            match olo_system::priority::raise_thread(p.pid as u32, p.pid as u32, nice) {
                Ok(now) => {
                    self.raised.insert(p.pid, Raised { pid: p.pid, name: p.name.clone(), nice_before: before, nice: now });
                    self.raise_again.remove(&p.pid);
                    let how = if again { format!("again (it was lowered to nice {before})") } else { format!("to nice {now}") };
                    self.note(format!("Raised {} ({}) {how}: Olo's own, above everything else", p.name, p.pid));
                }
                Err(e) => {
                    // (Asked again a minute later; said once.)
                    self.raised.insert(p.pid, Raised { pid: p.pid, name: p.name.clone(), nice_before: before, nice: before });
                    self.raise_again.insert(p.pid, t + 60.0);
                    if !refused {
                        self.note(format!("Couldn't raise {} ({}): {e}", p.name, p.pid));
                    }
                }
            }
        }

        // The voice services' sound (olo-voice, olo-airpods): PipeWire's
        // thread for it should be realtime (libpipewire asks rtkit as the
        // stream starts); one that isn't (rtkit said no then, or no
        // limit on realtime time was set), asked for again, once.
        self.realtime.retain(|(pid, _), _| alive.contains(pid));
        for p in &procs {
            if olo_protocol::performance::tier(&p.name) != Some(olo_protocol::performance::AUDIO_NICE) || p.state == 'Z' || !same_session(p.pid) {
                continue;
            }
            let Ok(tasks) = std::fs::read_dir(format!("/proc/{}/task", p.pid)) else { continue };
            for task in tasks.flatten() {
                let Ok(tid) = task.file_name().to_string_lossy().parse::<u32>() else { continue };
                if self.realtime.get(&(p.pid, tid)).is_some_and(|next| t < *next) {
                    continue;
                }
                let comm = std::fs::read_to_string(task.path().join("comm")).unwrap_or_default();
                if !comm.starts_with(olo_protocol::performance::AUDIO_THREAD) || olo_system::priority::is_realtime(tid) {
                    self.realtime.insert((p.pid, tid), f64::INFINITY);
                    continue;
                }
                let first = !self.realtime.contains_key(&(p.pid, tid));
                match olo_system::priority::realtime(p.pid as u32, tid, olo_protocol::performance::AUDIO_RT_PRIORITY) {
                    Ok(()) => {
                        self.realtime.insert((p.pid, tid), f64::INFINITY);
                        self.note(format!("Made {}'s sound ({}, thread {tid}) realtime: Olo's voice, above everything", p.name, p.pid));
                    }
                    Err(e) => {
                        self.realtime.insert((p.pid, tid), t + 60.0);
                        // (Noted the first time, not every minute.)
                        if first {
                            self.note(format!("Couldn't make {}'s sound ({}, thread {tid}) realtime: {e}", p.name, p.pid));
                        }
                    }
                }
            }
        }

        // Priorities: background work lowered, as it starts.
        self.lowered.retain(|pid, _| alive.contains(pid));
        if settings.priorities {
            let background: HashSet<&str> = settings.background.iter().map(String::as_str).collect();
            for p in &procs {
                if self.lowered.contains_key(&p.pid) || favoured.contains(&p.pid) || self.olo(p) || p.state == 'Z' {
                    continue;
                }
                let by_name = background.contains(p.name.as_str());
                let busy = !by_name && self.followed.get(&p.pid).is_some_and(|f| busy_job(f, t));
                if !(by_name || busy) {
                    continue;
                }
                let nice = settings.background_nice.clamp(1, 19);
                if let Some(before) = priority::lower(p.pid, nice) {
                    let why = if by_name { "background work (by its name)".to_owned() } else { "a terminal's job, busy for a minute and more".to_owned() };
                    self.lowered.insert(p.pid, Lowered { pid: p.pid, name: p.name.clone(), why: why.clone(), nice_before: before, nice: before.max(nice), policy: "batch".into(), io: "idle".into() });
                    // (Builds start many: noted once a name a minute.)
                    let recent = self.log.iter().rev().take(10).any(|e| e.text.starts_with(&format!("Lowered {}", p.name)) && now_secs().saturating_sub(e.at) < 60);
                    if !recent {
                        self.note(format!("Lowered {} ({}): {why}", p.name, p.pid));
                    }
                }
            }
        } else if !self.lowered.is_empty() {
            for l in self.lowered.values() {
                priority::restore(l.pid);
            }
            self.lowered.clear();
            self.note("Priorities off: background work put back".into());
        }

        // The watchdog.
        let mut found: HashMap<(i32, FindingKind), Finding> = HashMap::new();
        if settings.watchdog {
            let th = &settings.thresholds;
            let background: HashSet<&str> = settings.background.iter().map(String::as_str).collect();
            for p in &procs {
                let Some(f) = self.followed.get(&p.pid) else { continue };
                if p.state == 'Z' || settings.ignores(&p.name, &f.command) {
                    continue;
                }
                let mut judged = Vec::new();
                if !background.contains(p.name.as_str()) {
                    judged.extend(detect::spin(&f.fine, th));
                }
                judged.extend(detect::leak(&f.coarse, &system, th));
                let growing = f.coarse.len() >= 3 && f.coarse.back().map(|x| x.1) > f.coarse.get(f.coarse.len() - 3).map(|x| x.1);
                judged.extend(detect::memory(p.sample.memory, growing, &system, th));
                for j in judged {
                    found.insert((p.pid, j.kind), self.finding(p, j));
                }
            }
            // Zombies, by their parents.
            let zombies: Vec<&sample::Proc> = procs.iter().filter(|p| p.state == 'Z').collect();
            self.zombies.retain(|pid, _| zombies.iter().any(|z| z.pid == *pid));
            for z in &zombies {
                self.zombies.entry(z.pid).or_insert(t);
            }
            let mut by_parent: HashMap<i32, (usize, f64)> = HashMap::new();
            for z in &zombies {
                let e = by_parent.entry(z.ppid).or_insert((0, 0.0));
                e.0 += 1;
                e.1 = e.1.max(t - self.zombies.get(&z.pid).copied().unwrap_or(t));
            }
            for (parent, (count, oldest)) in by_parent {
                let Some(p) = by_pid.get(&parent) else { continue };
                if settings.ignores(&p.name, &self.command_of(parent)) {
                    continue;
                }
                if let Some(j) = detect::zombies(count, oldest, th) {
                    found.insert((parent, j.kind), self.finding(p, j));
                }
            }
        }
        // New ones noted; urgent ones asked about, once.
        for (key, f) in &found {
            if !self.findings.contains_key(key) {
                self.note(format!("{}: {} ({}), {:.0}% sure", f.kind.title(), f.name, f.pid, f.confidence * 100.0));
            }
            let start = self.followed.get(&f.pid).map_or(0, |x| x.start);
            let before = self.asked_before.iter().any(|(pid, name, kind)| *pid == f.pid && *name == f.name && *kind == f.kind);
            if f.urgent && settings.alerts && !self.olo_pid(f.pid) && self.asked.insert((f.pid, start, f.kind)) && !before {
                self.proxy.send(Msg::Urgent(f.clone()));
            }
        }
        self.findings = found;
        let favoured = focus.filter(|(pid, _)| by_pid.contains_key(pid));
        self.publish(Some((favoured, system)));
    }

    /// What the supervisor before this one (the version before an
    /// upgrade, or one that crashed) lowered: kept lowered if this one is
    /// to lower background work, else put back.
    fn adopt(&mut self) {
        let before = SupervisorState::read();
        let settings = self.settings.lock().unwrap().clone();
        let keep = settings.enabled && settings.priorities;
        self.log = before.log.into_iter().rev().take(LOG_KEPT).rev().collect();
        self.asked_before = before.findings.iter().filter(|f| f.urgent).map(|f| (f.pid, f.name.clone(), f.kind)).collect();
        let mut put_back = 0;
        for l in before.lowered {
            // The same process still (pids are used again): its name.
            let comm = std::fs::read_to_string(format!("/proc/{}/comm", l.pid)).unwrap_or_default();
            if comm.trim() != l.name {
                continue;
            }
            if keep {
                self.lowered.insert(l.pid, l);
            } else {
                priority::restore(l.pid);
                put_back += 1;
            }
        }
        if put_back > 0 {
            self.note(format!("Off: {put_back} lowered before put back to normal scheduling and I/O"));
        }
    }

    fn command_of(&self, pid: i32) -> String {
        self.followed.get(&pid).map_or_else(|| sample::command(pid), |f| f.command.clone())
    }

    fn olo_pid(&self, pid: i32) -> bool {
        self.followed.get(&pid).is_some_and(|f| f.name.starts_with("olo-"))
    }

    /// A finding about `p`, keeping when it was first made.
    fn finding(&self, p: &sample::Proc, j: detect::Judged) -> Finding {
        let since = self.findings.get(&(p.pid, j.kind)).map_or_else(now_secs, |f| f.since);
        Finding { pid: p.pid, name: p.name.clone(), command: self.command_of(p.pid), kind: j.kind, confidence: j.confidence, urgent: j.urgent, summary: j.summary, evidence: j.evidence, since }
    }

    /// Write what it's doing, if that changed.
    fn publish(&mut self, now: Option<(Option<(i32, String)>, detect::System)>) {
        let mut findings: Vec<Finding> = self.findings.values().cloned().collect();
        findings.sort_by(|a, b| b.urgent.cmp(&a.urgent).then(b.confidence.total_cmp(&a.confidence)).then(a.pid.cmp(&b.pid)));
        let mut lowered: Vec<Lowered> = self.lowered.values().cloned().collect();
        lowered.sort_by_key(|l| l.pid);
        // (Only those it raised; one it couldn't is noted in the log.)
        let mut raised: Vec<Raised> = self.raised.values().filter(|r| r.nice < r.nice_before).cloned().collect();
        raised.sort_by_key(|r| r.pid);
        let (favoured, system) = now.unwrap_or_default();
        let state = SupervisorState {
            running: self.running,
            pid: std::process::id(),
            favoured,
            findings,
            lowered,
            raised,
            log: self.log.iter().cloned().collect(),
            memory_available: system.available_pct(),
            memory_stall: system.stall_some,
        };
        // (Memory moves every time: compared without it.)
        let same = self
            .published
            .as_ref()
            .is_some_and(|p| SupervisorState { memory_available: 0.0, memory_stall: 0.0, ..p.clone() } == SupervisorState { memory_available: 0.0, memory_stall: 0.0, ..state.clone() });
        if !same || self.published.is_none() {
            if let Err(e) = state.save() {
                tracing::warn!("supervisor: can't publish its state: {e}");
            }
            self.published = Some(state);
        }
    }
}

/// A run that isn't background by name but behaves so: a terminal's job
/// (a shell, or a build tool, above it: not an app the session or Deskbar
/// started), busy for a while (`BUSY_CPU` of a core for `BUSY_FOR`).
fn busy_job(f: &Followed, t: f64) -> bool {
    let Some(start) = f.fine.iter().rev().find(|s| s.t <= t - BUSY_FOR) else { return false };
    let Some(last) = f.fine.back() else { return false };
    let busy = last.t > start.t && (last.cpu - start.cpu) / (last.t - start.t) * 100.0 >= BUSY_CPU;
    busy && {
        let parent = std::fs::read_to_string(format!("/proc/{}/comm", f.ppid)).unwrap_or_default();
        ["bash", "zsh", "fish", "sh", "dash", "olo-termd", "make", "cargo", "ninja", "nohup", "xargs"].contains(&parent.trim())
    }
}

/// Follow which app has the active window, from app_server's events.
fn follow_focus(focus: Arc<Mutex<Option<(i32, String)>>>) {
    std::thread::Builder::new()
        .name("supervisor-focus".into())
        .spawn(move || {
            loop {
                if let Ok(client) = olo_protocol::Client::connect()
                    && let Ok(events) = client.into_events_of(&[olo_protocol::EventKind::Focus])
                {
                    for e in events {
                        let Ok(e) = e else { break };
                        if let olo_protocol::Event::WindowsChanged { windows } = e {
                            let active = windows.iter().find(|w| w.focused).and_then(|w| Some((w.pid?, w.app_id.clone())));
                            *focus.lock().unwrap() = active;
                        }
                    }
                }
                // app_server restarting: again shortly.
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("focus thread");
}

/// Follow `performance.json` as Preferences and Deskbar change it.
fn follow_settings(settings: Arc<Mutex<PerformanceSettings>>, wake: Arc<(Mutex<bool>, olo_sync::Condvar)>) {
    let path = PerformanceSettings::path();
    let watch = olo_ui::watch::watch([path], move |_| {
        *settings.lock().unwrap() = PerformanceSettings::load();
        let (lock, cv) = &*wake;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    });
    // For as long as the app runs.
    std::mem::forget(watch);
}

/// The alert's choices: the last (Enter) stops it.
const BUTTONS: &[&str] = &["Ignore This Process", "Show in Activity Monitor", "Stop Process"];

/// The alert's text: the process, what was found and why.
fn alert_text(f: &Finding) -> String {
    let evidence: Vec<String> = f.evidence.iter().take(3).map(|e| format!("• {e}")).collect();
    format!("“{}” ({}): {}\n{}\n({:.0}% sure.)", f.name, f.pid, f.summary, evidence.join("\n"), f.confidence * 100.0)
}

/// What the person chose for a finding.
fn chose(f: &Finding, choice: usize, settings: &Mutex<PerformanceSettings>) {
    match choice {
        0 => {
            let mut s = settings.lock().unwrap().clone();
            if !s.ignored.contains(&f.name) {
                s.ignored.push(f.name.clone());
            }
            if let Err(e) = s.save() {
                tracing::warn!("supervisor: can't save the ignore list: {e}");
            }
        }
        1 => show_in_activity_monitor(f.pid),
        _ => stop(f.pid, f.name.clone()),
    }
}

/// Asks the person about an urgent finding (an alert; closing it from
/// its tab is no choice).
fn ask(cx: &mut App, f: Finding, settings: Arc<Mutex<PerformanceSettings>>) {
    let icon = if f.kind == FindingKind::Zombies { "warning" } else { "stop" };
    let text = alert_text(&f);
    widgets::open_alert(cx, f.kind.title(), icon, &text, BUTTONS, move |choice, _| {
        if let Some(choice) = choice {
            chose(&f, choice, &settings);
        }
    });
}

/// Activity Monitor, at the process (its inspector).
fn show_in_activity_monitor(pid: i32) {
    let args = ["--inspect".to_owned(), pid.to_string()];
    if instance::forward("olo.activity", &args) {
        return;
    }
    if let Some(exe) = olo_xdg::find_program("olo-activity") {
        let mut argv = vec![exe.into_os_string()];
        argv.extend(args.iter().map(Into::into));
        if let Err(e) = olo_xdg::open::spawn(&argv) {
            tracing::warn!("supervisor: can't open Activity Monitor: {e}");
        }
    }
}

/// Ask `pid` to end, and make it if it hasn't in a few seconds (one of
/// the person's own: it is theirs to stop).
fn stop(pid: i32, name: String) {
    std::thread::spawn(move || {
        // SAFETY: plain kill on a pid the person chose to stop.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(100));
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return;
            }
        }
        tracing::info!("supervisor: {name} ({pid}) didn't end when asked; killing it");
        // SAFETY: as above.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    });
}

/// `--status`: what the running supervisor published.
fn status(json: bool) -> std::process::ExitCode {
    let s = SupervisorState::load();
    if json {
        println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
        return std::process::ExitCode::SUCCESS;
    }
    if !s.running {
        println!("The supervisor is off (Preferences ▸ Performance, or olo-supervisor --on).");
        return std::process::ExitCode::SUCCESS;
    }
    println!("On. Memory: {:.0}% available, stalls {:.0}%.", s.memory_available, s.memory_stall);
    if let Some((pid, app)) = &s.favoured {
        println!("Favoured: {app} ({pid}).");
    }
    println!("Lowered ({}):", s.lowered.len());
    for l in &s.lowered {
        println!("  {} ({}): nice {} → {}, {} scheduling, {} I/O: {}", l.name, l.pid, l.nice_before, l.nice, l.policy, l.io, l.why);
    }
    println!("Findings ({}):", s.findings.len());
    for f in &s.findings {
        println!("  {}{}: {} ({}), {:.0}% sure: {}", if f.urgent { "URGENT " } else { "" }, f.kind.title(), f.name, f.pid, f.confidence * 100.0, f.summary);
        for e in &f.evidence {
            println!("    {e}");
        }
    }
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("-h") || has("--help") {
        println!("{USAGE}");
        return std::process::ExitCode::SUCCESS;
    }
    if has("--status") {
        return status(has("--json"));
    }
    if has("--on") || has("--off") {
        let mut s = PerformanceSettings::load();
        s.enabled = has("--on");
        return match s.save() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("olo-supervisor: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_ui::run(APP_ID, move |cx| {
        olo_ui::stay_resident(cx);
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let settings = Arc::new(Mutex::new(PerformanceSettings::load()));
        let focus = Arc::new(Mutex::new(None));
        let wake = Arc::new((Mutex::new(false), olo_sync::Condvar::new()));
        follow_focus(focus.clone());
        follow_settings(settings.clone(), wake.clone());
        let mut sup = Supervisor {
            settings: settings.clone(),
            focus,
            proxy: olo_ui::proxy(cx),
            // SAFETY: plain getuid.
            uid: unsafe { libc::getuid() },
            clock: Instant::now(),
            followed: HashMap::new(),
            lowered: HashMap::new(),
            raised: HashMap::new(),
            raise_again: HashMap::new(),
            realtime: HashMap::new(),
            zombies: HashMap::new(),
            findings: HashMap::new(),
            asked: HashSet::new(),
            asked_before: Vec::new(),
            log: VecDeque::new(),
            swapped: (0, 0.0),
            published: None,
            running: false,
            ours: std::env::current_exe().ok().and_then(|e| e.parent().map(std::path::Path::to_path_buf)),
            only_under: std::env::var("OLO_SUPERVISOR_ONLY_UNDER").ok().and_then(|p| p.parse().ok()),
        };
        sup.adopt();
        sup.publish(None);
        let _ = std::thread::Builder::new().name("supervisor".into()).spawn(move || {
            loop {
                let began = Instant::now();
                sup.tick();
                tracing::debug!("supervisor: looked in {:.1} ms", began.elapsed().as_secs_f64() * 1000.0);
                // Every two seconds, or at once when the settings change.
                let (lock, cv) = &*wake;
                let mut woke = lock.lock().unwrap();
                if !*woke {
                    woke = cv.wait_timeout(woke, EVERY).unwrap().0;
                }
                *woke = false;
            }
        });
        olo_ui::message_handler(cx, move |message, cx| {
            if let Ok(msg) = message.downcast::<Msg>() {
                let Msg::Urgent(f) = *msg;
                ask(cx, f, settings.clone());
            }
        });
    });
    std::process::ExitCode::SUCCESS
}

/// Whether `pid` runs in this supervisor's session (its WAYLAND_DISPLAY),
/// as read from its environment; a process whose environment can't be
/// read (gone, or not ours) isn't.
fn same_session(pid: i32) -> bool {
    let Some(mine) = std::env::var_os("WAYLAND_DISPLAY") else { return true };
    let Ok(env) = std::fs::read(format!("/proc/{pid}/environ")) else { return false };
    let key = b"WAYLAND_DISPLAY=";
    env.split(|&b| b == 0).any(|e| e.strip_prefix(key).is_some_and(|v| v == mine.as_encoded_bytes()))
}
