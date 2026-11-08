//! The backup service (`olo-diskbackup --service`; olo-session runs it):
//! asleep until a backup is due (`every_minutes` after the last), its
//! settings change, or it's asked (its socket: Back Up Now, Stop); then a
//! backup, gently (the whole process at the CPU's and the disks' idle
//! priority: anything else that wants them goes first), its progress in
//! the state file every 2 s while it runs and not otherwise. Speed limits
//! follow the settings at once, even mid-backup.
//!
//! After each backup, this computer's snapshots past keeping (the settings'
//! hours, days, weeks, months) are forgotten; once a day the backup is
//! tidied (`prune`): the space only they used freed. Each holds a lock in
//! the store (shared to back up, alone to tidy), so another computer
//! backing up to the same place, or a restore, is never pulled from under.

use std::{
    os::unix::net::UnixListener,
    process::ExitCode,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
        mpsc,
    },
    time::Duration,
};

use olo_diskbackup::{Id, Options, Progress, Repo, Snapshot, backup, exclude::Excludes, keep, prune};
use olo_protocol::diskbackup::{Doing, Keep, Last, Reply, Request, Settings, State, Target, Tidied};

use crate::target::{self, Limits};

enum Event {
    Settings,
    Now,
    Tidy,
}

/// What's kept between runs of the service (in the state folder).
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Kept {
    last: Option<Last>,
    last_ok: Option<i64>,
    tidied: Option<Tidied>,
    size: Option<u64>,
    snapshots: u32,
    /// Snapshots forgotten since the last tidy.
    forgotten: u32,
}

fn kept_path() -> std::path::PathBuf {
    match std::env::var_os("OLO_DISKBACKUP_DIR") {
        Some(d) => std::path::PathBuf::from(d).join("diskbackup-kept.json"),
        None => olo_xdg::state_dir().join("diskbackup.json"),
    }
}

/// The repository open, for the target it's in, and the last snapshot of
/// this computer's (what the next takes unchanged files from).
struct Opened {
    target: Target,
    repo: Repo,
    parent: Option<(Id, Snapshot)>,
}

pub fn run() -> ExitCode {
    gentle();
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let limits = Arc::new(Limits::new());
    listen(tx.clone(), cancel.clone());
    let _watch = olo_kit_base::watch::watch([Settings::path()], {
        let (tx, limits) = (Mutex::new(tx.clone()), limits.clone());
        move |_| {
            // (Speed limits at once, a backup running or not.)
            limits.follow(&Settings::load());
            let _ = tx.lock().map(|t| t.send(Event::Settings));
        }
    });
    let k: Kept = std::fs::read_to_string(kept_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut forgotten = k.forgotten;
    let mut st = State { pid: std::process::id(), last: k.last, last_ok: k.last_ok, tidied: k.tidied, size: k.size, snapshots: k.snapshots, ..State::default() };
    let mut open: Option<Opened> = None;
    let mut now_asked = false;
    let mut tidy_asked = false;
    let save = |st: &State, forgotten: u32| {
        let k = Kept { last: st.last.clone(), last_ok: st.last_ok, tidied: st.tidied.clone(), size: st.size, snapshots: st.snapshots, forgotten };
        if let Some(dir) = kept_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(kept_path(), serde_json::to_vec(&k).unwrap_or_default());
    };
    loop {
        let settings = Settings::load();
        limits.follow(&settings);
        st.target = settings.target.describe();
        if let Err(why) = ready(&settings) {
            st.doing = Doing::Waiting { why: why.clone() };
            st.next = None;
            let _ = st.save();
            // On battery: look again in a while (power coming back has no
            // event of ours yet); else when something changes.
            let wait = if why.contains("battery") { Duration::from_secs(300) } else { Duration::from_secs(u64::MAX / 4) };
            match rx.recv_timeout(wait) {
                Ok(Event::Now) => now_asked = true,
                Ok(Event::Tidy) => tidy_asked = true,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                _ => {}
            }
            continue;
        }
        if std::mem::take(&mut tidy_asked) {
            cancel.store(false, Relaxed);
            match opened(&settings, &mut open, &limits) {
                Ok(_) => tidy(&settings, &mut st, &mut open, &cancel, &mut forgotten),
                Err(e) => tracing::warn!("diskbackup: can't tidy: {e}"),
            }
            save(&st, forgotten);
            continue;
        }
        let now = now();
        let due = if now_asked { now } else { st.last.as_ref().map_or(now, |l| l.time + settings.every_minutes.max(1) as i64 * 60) };
        st.doing = Doing::Idle;
        st.next = Some(due);
        let _ = st.save();
        if due > now {
            match rx.recv_timeout(Duration::from_secs((due - now) as u64)) {
                Ok(Event::Now) => now_asked = true,
                Ok(Event::Tidy) => tidy_asked = true,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                _ => {}
            }
            continue;
        }
        now_asked = false;
        cancel.store(false, Relaxed);
        if once(&settings, &mut st, &mut open, &limits, &cancel, &mut forgotten) && tidy_due(st.tidied.as_ref()) {
            cancel.store(false, Relaxed);
            tidy(&settings, &mut st, &mut open, &cancel, &mut forgotten);
        }
        save(&st, forgotten);
    }
    ExitCode::SUCCESS
}

/// Why not to back up now, if not.
fn ready(s: &Settings) -> Result<(), String> {
    if s.target == Target::None {
        return Err("not set up yet".into());
    }
    if !s.enabled {
        return Err("turned off".into());
    }
    if !s.on_battery && on_battery() {
        return Err("on battery: it waits for power".into());
    }
    Ok(())
}

/// Running on battery (no power supply online, and a battery there).
fn on_battery() -> bool {
    let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") else { return false };
    let (mut battery, mut mains) = (false, false);
    for e in rd.flatten() {
        let kind = std::fs::read_to_string(e.path().join("type")).unwrap_or_default();
        match kind.trim() {
            "Battery" => battery = true,
            "Mains" | "USB" => mains |= std::fs::read_to_string(e.path().join("online")).is_ok_and(|o| o.trim() == "1"),
            _ => {}
        }
    }
    battery && !mains
}

/// `f`, with its progress in the state file every 2 s while it runs.
fn reporting<T>(st: &mut State, doing: Doing, progress: &Progress, f: impl FnOnce() -> T) -> T {
    st.doing = doing;
    st.next = None;
    st.progress = Some(olo_protocol::diskbackup::Progress { started: now(), ..Default::default() });
    let _ = st.save();
    let done = AtomicBool::new(false);
    let base = st.clone();
    std::thread::scope(|s| {
        let reporter = s.spawn(|| {
            while !done.load(Relaxed) {
                std::thread::park_timeout(Duration::from_secs(2));
                if done.load(Relaxed) {
                    break;
                }
                let mut now = base.clone();
                if let Some(p) = now.progress.as_mut() {
                    p.files = progress.files.load(Relaxed);
                    p.bytes = progress.bytes.load(Relaxed);
                    p.new_bytes = progress.new_bytes.load(Relaxed);
                    p.current = progress.current.lock().map(|c| c.clone()).unwrap_or_default();
                }
                let _ = now.save();
            }
        });
        let r = f();
        done.store(true, Relaxed);
        reporter.thread().unpark();
        r
    })
}

/// One backup, start to end, the state said throughout; whether it
/// finished.
fn once(settings: &Settings, st: &mut State, open: &mut Option<Opened>, limits: &Limits, cancel: &AtomicBool, forgotten: &mut u32) -> bool {
    let progress = Progress::default();
    let result = reporting(st, Doing::BackingUp, &progress, || back_up(settings, open, limits, cancel, &progress));
    if result.is_err() {
        // (Opened again next time: the index as the store has it.)
        *open = None;
    }
    let time = now();
    let ok = result.is_ok();
    st.last = Some(match result {
        Ok((snapshots, summary, gone)) => {
            st.snapshots = snapshots;
            st.last_ok = Some(time);
            st.size = st.size.map(|s| s + summary.stored_bytes);
            *forgotten += gone;
            Last {
                time,
                error: None,
                files: summary.files,
                bytes: summary.bytes,
                new_files: summary.new_files,
                changed_files: summary.changed_files,
                new_bytes: summary.new_bytes,
                uploaded: summary.stored_bytes,
                seconds: summary.seconds,
                unreadable: summary.errors.len() as u64,
            }
        }
        Err(e) => {
            tracing::warn!("diskbackup: {e}");
            Last { time, error: Some(e), ..Default::default() }
        }
    });
    st.doing = Doing::Idle;
    st.progress = None;
    let _ = st.save();
    ok
}

/// Tidied a day ago or more (an hour, if it didn't finish), or never.
fn tidy_due(t: Option<&Tidied>) -> bool {
    t.is_none_or(|t| now() - t.time >= if t.error.is_some() { 3600 } else { 24 * 3600 })
}

/// Tidy: with the backup to itself (another computer using it: next
/// time), this computer's snapshots past keeping forgotten (the settings
/// may have changed since the last backup), then what only they used freed.
fn tidy(settings: &Settings, st: &mut State, open: &mut Option<Opened>, cancel: &AtomicBool, forgotten: &mut u32) {
    let Some(o) = open.as_mut() else { return };
    let lock = match o.repo.lock(true) {
        Ok(l) => l,
        Err(e) => {
            tracing::info!("diskbackup: not tidying now: {e}");
            return;
        }
    };
    let progress = Progress::default();
    let result = reporting(st, Doing::Pruning, &progress, || {
        o.repo.refresh()?;
        let all = o.repo.snapshots()?;
        let host = host();
        let mine: Vec<(Id, Snapshot)> = all.iter().filter(|(_, s)| s.host == host).cloned().collect();
        let forget = keep::to_forget(&mine, policy(settings.keep));
        for f in &forget {
            o.repo.forget(f)?;
        }
        *forgotten += forget.len() as u32;
        prune(&mut o.repo, cancel, &progress).map(|p| (p, all.len() - forget.len()))
    });
    drop(lock);
    st.tidied = Some(match result {
        Ok((p, snapshots)) => {
            st.snapshots = snapshots as u32;
            tracing::info!("diskbackup: tidied: {p:?}");
            let freed = p.bytes_deleted.saturating_sub(p.bytes_written);
            st.size = Some(p.size);
            let t = Tidied { time: now(), error: None, forgotten: *forgotten, freed };
            *forgotten = 0;
            t
        }
        Err(e) => {
            tracing::warn!("diskbackup: tidying: {e}");
            *open = None;
            Tidied { time: now(), error: Some(e.to_string()), forgotten: *forgotten, freed: 0 }
        }
    });
    st.doing = Doing::Idle;
    st.progress = None;
    let _ = st.save();
}

/// The engine's keeping rule, from the settings'.
pub fn policy(k: Keep) -> keep::Policy {
    keep::Policy { hourly: k.hourly, daily: k.daily, weekly: k.weekly, monthly: k.monthly }
}

/// The repository open (once, or for a new target).
fn opened<'a>(settings: &Settings, open: &'a mut Option<Opened>, limits: &Limits) -> Result<&'a mut Opened, String> {
    if open.as_ref().is_none_or(|o| o.target != settings.target) {
        *open = None;
        let store = target::store(&settings.target, limits)?;
        let key = target::key()?;
        let repo = Repo::open(store, key, Some(target::cache())).map_err(|e| e.to_string())?;
        let host = host();
        let parent = repo.snapshots().map_err(|e| e.to_string())?.into_iter().rfind(|(_, s)| s.host == host);
        *open = Some(Opened { target: settings.target.clone(), repo, parent });
    }
    Ok(open.as_mut().expect("opened"))
}

/// Open and back up; how many snapshots there are then, what this one did,
/// and how many were forgotten after it.
fn back_up(settings: &Settings, open: &mut Option<Opened>, limits: &Limits, cancel: &AtomicBool, progress: &Progress) -> Result<(u32, olo_diskbackup::Summary, u32), String> {
    let host = host();
    let o = opened(settings, open, limits)?;
    // Held while it backs up (and forgets): nobody tidies meanwhile.
    let lock = o.repo.lock(false).map_err(|e| e.to_string())?;
    // (The index as it is now: another computer may have tidied.)
    o.repo.refresh().map_err(|e| e.to_string())?;
    let home = target::home();
    let groups = settings.exclude_groups.clone().unwrap_or_else(olo_diskbackup::exclude::default_groups);
    // Never the backup's own: its cache, and a folder target.
    let mut own = settings.excludes.clone();
    own.push(target::cache().display().to_string());
    if let Target::Folder { path } = &settings.target {
        own.push(target::expand(path).display().to_string());
    }
    let excludes = Excludes::new(&groups, &own, &home);
    let paths = settings.paths.iter().map(|p| target::expand(p)).filter(|p| p.exists()).collect();
    let opts = Options { paths, excludes: &excludes, host, one_file_system: settings.one_file_system };
    let parent = o.parent.as_ref().map(|(id, s)| (*id, s));
    let host = opts.host.clone();
    let (id, snapshot) = backup(&mut o.repo, &opts, parent, cancel, progress).map_err(|e| e.to_string())?;
    let summary = snapshot.summary.clone();
    o.parent = Some((id, snapshot));
    // Past keeping: this computer's snapshots the settings don't keep
    // (another computer's are its own to keep), forgotten; the space only
    // they used is freed when it's next tidied.
    let all = o.repo.snapshots().map_err(|e| e.to_string())?;
    let mine: Vec<(Id, Snapshot)> = all.iter().filter(|(_, s)| s.host == host).cloned().collect();
    let forget = keep::to_forget(&mine, policy(settings.keep));
    for f in &forget {
        o.repo.forget(f).map_err(|e| e.to_string())?;
    }
    drop(lock);
    Ok(((all.len() - forget.len()) as u32, summary, forget.len() as u32))
}

/// This computer's name (its snapshots are told apart by it).
pub fn host() -> String {
    olo_diskbackup::lock::host()
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The whole process at the idle priority, the CPU's and the disks':
/// anything else that wants them goes first (threads made after inherit).
fn gentle() {
    // SAFETY: plain syscalls on this thread and process.
    unsafe {
        let p = libc::sched_param { sched_priority: 0 };
        if libc::sched_setscheduler(0, libc::SCHED_IDLE, &p) != 0 {
            tracing::info!("diskbackup: can't take the idle CPU priority: {}", std::io::Error::last_os_error());
        }
        // ioprio_set(IOPRIO_WHO_PROCESS, this, IOPRIO_CLASS_IDLE << 13).
        if libc::syscall(libc::SYS_ioprio_set, 1, 0, 3 << 13) != 0 {
            tracing::info!("diskbackup: can't take the idle disk priority: {}", std::io::Error::last_os_error());
        }
    }
}

/// The socket: Back Up Now and Stop.
fn listen(tx: mpsc::Sender<Event>, cancel: Arc<AtomicBool>) {
    let path = olo_protocol::diskbackup::socket();
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        tracing::warn!("diskbackup: can't listen at {}", path.display());
        return;
    };
    let _ = std::thread::Builder::new().name("socket".into()).spawn(move || {
        use std::io::{BufRead, Write};
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            let _ = std::io::BufReader::new(&conn).read_line(&mut line);
            let reply = match serde_json::from_str::<Request>(&line) {
                Ok(Request::BackUpNow) => {
                    let _ = tx.send(Event::Now);
                    Reply::Ok
                }
                Ok(Request::Stop) => {
                    cancel.store(true, Relaxed);
                    Reply::Ok
                }
                Ok(Request::Tidy) => {
                    let _ = tx.send(Event::Tidy);
                    Reply::Ok
                }
                Err(e) => Reply::Error { message: e.to_string() },
            };
            let _ = (&conn).write_all(format!("{}\n", serde_json::to_string(&reply).unwrap_or_default()).as_bytes());
        }
    });
}
