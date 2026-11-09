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
//!
//! A backup that doesn't finish keeps what it got through (the engine's
//! unfinished snapshot), which the next goes on from: said in the state
//! (`unfinished`) and deletable (`Delete { unfinished }`). What an older
//! one left with nothing kept of its own (sent, but in no snapshot) is
//! found once, at the start, and said the same way.
//!
//! Each backup is checked once it's made (the engine's `check`: what it
//! needs all in the store, its lists read back); Validate reads chosen
//! ones back whole. The results are kept (by snapshot), and the packs
//! found missing or damaged are repaired by the next tidy, done at once:
//! what's whole in them salvaged, the rest let go of, so the next backup
//! sends it again.

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

use std::os::unix::ffi::OsStrExt;

use olo_diskbackup::{Id, Options, Progress, Repo, Snapshot, backup, exclude::Excludes, keep, prune};
use olo_protocol::diskbackup::{Check, Doing, Keep, Last, Priority, Reply, Request, Settings, SnapshotInfo, Stage, State, Target, Tidied, Unfinished};

use crate::target::{self, Limits};

enum Event {
    Settings,
    Now,
    Tidy,
    /// Delete these backups, and the unfinished one if asked (and free
    /// what only they took).
    Delete(Vec<Id>, bool),
    /// Delete every backup there, and start again.
    Erase,
    /// Validate these (fully, if so).
    Validate(Vec<Id>, bool),
}

/// What the loop was asked meanwhile (as it waited, or backed up).
#[derive(Default)]
struct Asked {
    now: bool,
    tidy: bool,
    delete: Vec<Id>,
    delete_unfinished: bool,
    erase: bool,
    validate: Vec<Id>,
    validate_full: bool,
}

impl Asked {
    /// `e`, noted; false if the events are over (the service ends).
    fn note(&mut self, e: Result<Event, mpsc::RecvTimeoutError>) -> bool {
        match e {
            Ok(Event::Now) => self.now = true,
            Ok(Event::Tidy) => self.tidy = true,
            Ok(Event::Delete(ids, unfinished)) => {
                self.delete.extend(ids);
                self.delete_unfinished |= unfinished;
            }
            Ok(Event::Erase) => self.erase = true,
            Ok(Event::Validate(ids, full)) => {
                self.validate.extend(ids);
                self.validate_full |= full;
            }
            Ok(Event::Settings) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
        }
        true
    }
}

/// A priority asked for that this process can't take (a lower one can't
/// be raised again: only a new process can): it ends once it's not busy,
/// and olo-session starts it again, at the session's, then this one.
static RESTART: AtomicBool = AtomicBool::new(false);

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
    /// Each backup's last check, by its id (hex).
    checks: std::collections::BTreeMap<String, Check>,
    /// Packs a check found missing or damaged, for the next tidy to repair.
    repair: Vec<String>,
    /// Where the rest is of (the last place set up: disconnecting and
    /// connecting again to the same keeps it; moving elsewhere starts
    /// afresh).
    target: Option<Target>,
}

/// Each of this computer's backups' last check, by id (hex).
static CHECKS: Mutex<std::collections::BTreeMap<String, Check>> = Mutex::new(std::collections::BTreeMap::new());
/// Packs to repair at the next tidy.
static REPAIR: Mutex<Vec<Id>> = Mutex::new(Vec::new());

/// What `checked` found, as said (`full`: Validate's).
fn check_of(c: &olo_diskbackup::Checked, full: bool) -> Check {
    Check {
        time: now(),
        full,
        problems: c.problems.clone(),
        files: c.files.clone(),
        files_affected: c.files_affected,
        bytes_read: c.bytes_read,
        by_checksum: c.by_checksum,
        sampled: c.sampled,
        seconds: c.seconds,
        error: None,
    }
}

/// Backup `id`'s check kept (and its bad packs, to repair).
fn note_check(id: &Id, check: &Check, bad: &[Id]) {
    if let Ok(mut c) = CHECKS.lock() {
        c.insert(id.hex(), check.clone());
    }
    if !bad.is_empty()
        && let Ok(mut r) = REPAIR.lock()
    {
        for b in bad {
            if !r.contains(b) {
                r.push(*b);
            }
        }
    }
}

/// Packs waiting to be repaired.
fn to_repair() -> Vec<Id> {
    REPAIR.lock().map(|r| r.clone()).unwrap_or_default()
}

fn kept_path() -> std::path::PathBuf {
    match std::env::var_os("OLO_DISKBACKUP_DIR") {
        Some(d) => std::path::PathBuf::from(d).join("diskbackup-kept.json"),
        None => olo_xdg::state_dir().join("diskbackup.json"),
    }
}

/// What the socket answers Tracker from (`Request::Marks`, `Versions`):
/// this computer's snapshots and a reader of their trees, made again after
/// each backup and tidy (so it never waits for one to finish).
struct Lookup {
    reader: olo_diskbackup::TreeReader,
    /// This computer's, oldest first.
    snapshots: Vec<(Id, Snapshot)>,
    /// This computer's backup that didn't finish.
    unfinished: Option<Unfinished>,
}

static LOOKUP: Mutex<Option<Arc<Lookup>>> = Mutex::new(None);

/// The lookup made again from `o`.
fn publish(o: &Opened) {
    let host = host();
    let snapshots: Vec<(Id, Snapshot)> = match o.repo.snapshots() {
        Ok(all) => all.into_iter().filter(|(_, s)| s.host == host).collect(),
        Err(e) => {
            tracing::warn!("diskbackup: snapshots for lookups: {e}");
            return;
        }
    };
    // The one gone on from next (the furthest), else what an older one
    // left (found at the start).
    let unfinished = o.repo.unfinished().unwrap_or_default().into_iter().filter(|(_, s)| s.host == host).max_by_key(|(_, s)| (s.summary.files, s.time));
    let unfinished = unfinished
        .map(|(_, s)| Unfinished { time: s.time, files: s.summary.files, bytes: s.summary.bytes, new_bytes: s.summary.new_bytes, sent: s.summary.stored_bytes, seconds: s.summary.seconds })
        .or_else(|| o.leftover.clone());
    // (Checks of backups gone, forgotten.)
    if let Ok(mut c) = CHECKS.lock() {
        c.retain(|id, _| snapshots.iter().any(|(s, _)| s.hex() == *id));
    }
    if let Ok(mut l) = LOOKUP.lock() {
        *l = Some(Arc::new(Lookup { reader: o.repo.tree_reader(), snapshots, unfinished }));
    }
}

/// This computer's backup that didn't finish, as last opened.
fn unfinished() -> Option<Unfinished> {
    LOOKUP.lock().ok().and_then(|l| l.as_ref().and_then(|l| l.unfinished.clone()))
}

/// This computer's backups, newest first (as opened: the backup not
/// opened yet, none).
fn snapshot_list() -> Reply {
    let lookup = LOOKUP.lock().ok().and_then(|l| l.clone());
    let checks = CHECKS.lock().map(|c| c.clone()).unwrap_or_default();
    let snapshots = lookup
        .map(|l| {
            l.snapshots
                .iter()
                .rev()
                .map(|(id, s)| SnapshotInfo {
                    id: id.hex(),
                    time: s.time,
                    files: s.summary.files,
                    bytes: s.summary.bytes,
                    new_files: s.summary.new_files,
                    changed_files: s.summary.changed_files,
                    new_bytes: s.summary.new_bytes,
                    stored_bytes: s.summary.stored_bytes,
                    seconds: s.summary.seconds,
                    unreadable: s.summary.errors.len() as u64,
                    check: checks.get(&id.hex()).cloned(),
                })
                .collect()
        })
        .unwrap_or_default();
    Reply::Snapshots { snapshots }
}

/// Which of `dir`'s files are backed up as they are now.
fn marks(dir: &str) -> Reply {
    let Some(l) = LOOKUP.lock().ok().and_then(|l| l.clone()) else { return Reply::Marks { covered: false, backed_up: Vec::new(), time: 0 } };
    let Some((_, newest)) = l.snapshots.last() else { return Reply::Marks { covered: false, backed_up: Vec::new(), time: 0 } };
    let dir = std::path::Path::new(dir);
    let node = match olo_diskbackup::restore::find(&l.reader, &newest.tree, dir) {
        Ok(n) => n,
        Err(e) => return Reply::Error { message: e.to_string() },
    };
    let Some(sub) = node.filter(|n| n.kind == olo_diskbackup::Kind::Dir).and_then(|n| n.subtree) else {
        return Reply::Marks { covered: false, backed_up: Vec::new(), time: newest.time };
    };
    let tree = match l.reader.get(&sub) {
        Ok(t) => t,
        Err(e) => return Reply::Error { message: e.to_string() },
    };
    let backed_up = tree
        .nodes
        .iter()
        .filter(|n| n.kind != olo_diskbackup::Kind::Dir)
        .filter(|n| olo_diskbackup::backup::unchanged_since(n, &dir.join(std::ffi::OsStr::from_bytes(&n.name))))
        .map(|n| n.name_str().into_owned())
        .collect();
    Reply::Marks { covered: true, backed_up, time: newest.time }
}

/// The versions of `path` this computer's snapshots kept.
fn versions(path: &str) -> Reply {
    let Some(l) = LOOKUP.lock().ok().and_then(|l| l.clone()) else { return Reply::Versions { versions: Vec::new() } };
    let path = std::path::Path::new(path);
    match olo_diskbackup::versions(&l.reader, &l.snapshots, path) {
        Ok(vs) => Reply::Versions {
            versions: vs
                .into_iter()
                .map(|v| olo_protocol::diskbackup::Version {
                    snapshot: v.snapshot.hex(),
                    time: v.time,
                    first: v.first,
                    size: v.node.size,
                    mtime: v.node.mtime,
                    current: olo_diskbackup::backup::unchanged_since(&v.node, path),
                })
                .collect(),
        },
        Err(e) => Reply::Error { message: e.to_string() },
    }
}

/// The repository open, for the target it's in, and the last snapshot of
/// this computer's (what the next takes unchanged files from).
struct Opened {
    target: Target,
    repo: Repo,
    parent: Option<(Id, Snapshot)>,
    /// What an older backup that stopped left with nothing kept of its
    /// own: sent, and in no snapshot (the next goes on from it all the
    /// same; tidying frees it).
    leftover: Option<Unfinished>,
}

pub fn run() -> ExitCode {
    // One at a time (Doctor's Start It, olo-session's, by hand).
    let st = State::load();
    if st.pid != 0 && st.pid != std::process::id() && std::fs::read_to_string(format!("/proc/{}/comm", st.pid)).is_ok_and(|c| c.trim() == "olo-diskbackup") {
        tracing::info!("diskbackup: the service is running already (process {})", st.pid);
        return ExitCode::SUCCESS;
    }
    let mut priority = Settings::load().priority;
    if !set_priority(priority) {
        tracing::info!("diskbackup: can't take the priority {priority:?} (only lower ones): from the session's");
    }
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let limits = Arc::new(Limits::new());
    listen(tx.clone(), cancel.clone());
    let _watch = olo_kit_base::watch::watch([Settings::path()], {
        let (tx, limits) = (Mutex::new(tx.clone()), limits.clone());
        move |_| {
            // (Speed limits, and the priority, at once: a backup running or
            // not.)
            let s = Settings::load();
            limits.follow(&s);
            if !set_priority(s.priority) {
                RESTART.store(true, Relaxed);
            }
            let _ = tx.lock().map(|t| t.send(Event::Settings));
        }
    });
    let k: Kept = std::fs::read_to_string(kept_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut forgotten = k.forgotten;
    if let Ok(mut c) = CHECKS.lock() {
        *c = k.checks.clone();
    }
    if let Ok(mut r) = REPAIR.lock() {
        *r = k.repair.iter().filter_map(|h| Id::from_hex(h)).collect();
    }
    let mut st = State { pid: std::process::id(), last: k.last, last_ok: k.last_ok, tidied: k.tidied, size: k.size, snapshots: k.snapshots, ..State::default() };
    let mut kept_target = k.target.clone();
    let mut open: Option<Opened> = None;
    let mut asked = Asked::default();
    let mut looked_for_leftovers = false;
    let save = |st: &State, forgotten: u32, target: &Option<Target>| {
        let k = Kept {
            last: st.last.clone(),
            last_ok: st.last_ok,
            tidied: st.tidied.clone(),
            size: st.size,
            snapshots: st.snapshots,
            forgotten,
            checks: CHECKS.lock().map(|c| c.clone()).unwrap_or_default(),
            repair: to_repair().iter().map(Id::hex).collect(),
            target: target.clone(),
        };
        if let Some(dir) = kept_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(kept_path(), serde_json::to_vec(&k).unwrap_or_default());
    };
    loop {
        let settings = Settings::load();
        limits.follow(&settings);
        // A priority this process can't take: ended, to be started again
        // at it (olo-session restarts its services).
        if RESTART.load(Relaxed) {
            tracing::info!("diskbackup: starting again for the priority {:?}", settings.priority);
            break;
        }
        if settings.priority != priority {
            priority = settings.priority;
        }
        st.target = settings.target.describe();
        // Moved elsewhere: what's said of the last place (its last backup,
        // its size, its checks) isn't of this one.
        if settings.target != Target::None && kept_target.as_ref() != Some(&settings.target) {
            if kept_target.is_some() {
                tracing::info!("diskbackup: backing up to {} now: starting afresh there", st.target);
                (st.last, st.last_ok, st.size, st.snapshots, st.tidied, st.unfinished) = (None, None, None, 0, None, None);
                forgotten = 0;
                looked_for_leftovers = true;
                if let Ok(mut c) = CHECKS.lock() {
                    c.clear();
                }
                if let Ok(mut r) = REPAIR.lock() {
                    r.clear();
                }
            }
            kept_target = Some(settings.target.clone());
            save(&st, forgotten, &kept_target);
        }
        // Not backing up anywhere (disconnected): nothing of it kept open.
        if settings.target == Target::None && open.is_some() {
            open = None;
            if let Ok(mut l) = LOOKUP.lock() {
                *l = None;
            }
        }
        // Deleting, and starting fresh: asked, whatever the schedule.
        if settings.target != Target::None && (asked.erase || !asked.delete.is_empty() || asked.delete_unfinished) {
            cancel.store(false, Relaxed);
            if std::mem::take(&mut asked.erase) {
                asked.delete.clear();
                asked.delete_unfinished = false;
                erase_all(&settings, &mut st, &mut open, &limits, &cancel);
            } else {
                let ids = std::mem::take(&mut asked.delete);
                let unfinished = std::mem::take(&mut asked.delete_unfinished);
                delete(&settings, &mut st, &mut open, &limits, &cancel, &ids, unfinished, &mut forgotten);
            }
            st.unfinished = unfinished();
            save(&st, forgotten, &kept_target);
            continue;
        }
        if let Err(why) = ready(&settings) {
            st.doing = Doing::Waiting { why: why.clone() };
            st.next = None;
            let _ = st.save();
            // On battery: look again in a while (power coming back has no
            // event of ours yet); else when something changes.
            let wait = if why.contains("battery") { Duration::from_secs(300) } else { Duration::from_secs(u64::MAX / 4) };
            if !asked.note(rx.recv_timeout(wait)) {
                break;
            }
            continue;
        }
        // Open (for Tracker's lookups) before the first backup is due.
        if open.is_none()
            && let Err(e) = opened(&settings, &mut open, &limits)
        {
            tracing::info!("diskbackup: can't open the backup yet: {e}");
        }
        // Once: what an older backup that didn't finish left with nothing
        // kept of its own (since the last tidy).
        if !looked_for_leftovers && let Some(o) = open.as_mut() {
            looked_for_leftovers = true;
            let stopped = st.last.as_ref().is_some_and(|l| l.error.is_some() && st.tidied.as_ref().is_none_or(|t| t.time < l.time));
            if stopped && unfinished().is_none() {
                match olo_diskbackup::unused(&o.repo, &cancel) {
                    Ok(sent) if sent > 0 => {
                        let time = st.last.as_ref().map_or(0, |l| l.time);
                        tracing::info!("diskbackup: a backup that stopped left {sent} bytes, in no snapshot");
                        o.leftover = Some(Unfinished { time, new_bytes: sent, sent, ..Default::default() });
                        publish(o);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::info!("diskbackup: what a stopped backup left: {e}"),
                }
            }
        }
        st.unfinished = unfinished();
        // Validate: chosen backups read back; what's damaged repaired.
        if !asked.validate.is_empty() {
            cancel.store(false, Relaxed);
            let ids = std::mem::take(&mut asked.validate);
            let full = std::mem::take(&mut asked.validate_full);
            validate(&settings, &mut st, &mut open, &limits, &cancel, &ids, full);
            if !to_repair().is_empty() {
                asked.tidy = true;
            }
            save(&st, forgotten, &kept_target);
            continue;
        }
        if std::mem::take(&mut asked.tidy) {
            cancel.store(false, Relaxed);
            match opened(&settings, &mut open, &limits) {
                Ok(_) => tidy(&settings, &mut st, &mut open, &cancel, &mut forgotten),
                Err(e) => tracing::warn!("diskbackup: can't tidy: {e}"),
            }
            save(&st, forgotten, &kept_target);
            continue;
        }
        let now = now();
        // Only when asked (Back Up Now): no next one, until it is.
        let due = if asked.now {
            Some(now)
        } else if settings.automatic {
            Some(st.last.as_ref().map_or(now, |l| l.time + settings.every_minutes.max(1) as i64 * 60))
        } else {
            None
        };
        st.doing = Doing::Idle;
        st.next = due;
        let _ = st.save();
        let Some(due) = due else {
            if !asked.note(rx.recv_timeout(Duration::from_secs(u64::MAX / 4))) {
                break;
            }
            continue;
        };
        if due > now {
            if !asked.note(rx.recv_timeout(Duration::from_secs((due - now) as u64))) {
                break;
            }
            continue;
        }
        asked.now = false;
        cancel.store(false, Relaxed);
        if once(&settings, &mut st, &mut open, &limits, &cancel, &mut forgotten)
            && (tidy_due(st.tidied.as_ref(), open.as_ref().is_some_and(|o| o.repo.damaged_index_files() > 0)) || !to_repair().is_empty())
        {
            cancel.store(false, Relaxed);
            tidy(&settings, &mut st, &mut open, &cancel, &mut forgotten);
        }
        save(&st, forgotten, &kept_target);
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
                    p.uploaded = progress.uploaded.load(Relaxed);
                    p.current = progress.current.lock().map(|c| c.clone()).unwrap_or_default();
                    p.total_files = progress.total_files.load(Relaxed);
                    p.total_bytes = progress.total_bytes.load(Relaxed);
                    p.estimated = progress.estimated.load(Relaxed);
                    p.resuming = progress.resuming.load(Relaxed);
                    p.stage = match progress.stage.load(Relaxed) {
                        olo_diskbackup::backup::COUNTING => Stage::Counting,
                        olo_diskbackup::backup::SAVING => Stage::Saving,
                        olo_diskbackup::backup::CHECKING => Stage::Checking,
                        olo_diskbackup::backup::STOPPING => Stage::Stopping,
                        _ => Stage::Reading,
                    };
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
        // What it got through, said (kept as an unfinished snapshot); then
        // opened again next time: the index as the store has it.
        if let Some(o) = open.as_ref() {
            publish(o);
        }
        *open = None;
    }
    st.unfinished = unfinished();
    let time = now();
    let ok = result.is_ok();
    st.last = Some(match result {
        Ok((snapshots, summary, gone, check)) => {
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
                check,
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

/// Tidied a day ago or more (an hour, if it didn't finish or there are
/// `damaged` index files to let go of), or never.
fn tidy_due(t: Option<&Tidied>, damaged: bool) -> bool {
    t.is_none_or(|t| now() - t.time >= if t.error.is_some() || damaged { 3600 } else { 24 * 3600 })
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
        // (Packs a check found missing or damaged: repaired.)
        let damaged: std::collections::HashSet<Id> = to_repair().into_iter().collect();
        let p = if damaged.is_empty() { prune(&mut o.repo, cancel, &progress) } else { olo_diskbackup::repair(&mut o.repo, &damaged, cancel, &progress) }?;
        if !damaged.is_empty() {
            tracing::info!("diskbackup: {} packs repaired (what's whole in them kept; the next backup sends the rest again)", damaged.len());
            if let Ok(mut r) = REPAIR.lock() {
                r.retain(|id| !damaged.contains(id));
            }
        }
        Ok::<_, olo_diskbackup::Error>((p, all.len() - forget.len()))
    });
    drop(lock);
    if result.is_ok() {
        o.leftover = None;
    }
    publish(o);
    st.unfinished = unfinished();
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
        if repo.damaged_index_files() > 0 {
            tracing::warn!("diskbackup: {} index files are damaged: their packs read back from their own lists, and the files let go of at the next tidying", repo.damaged_index_files());
        }
        let host = host();
        let parent = repo.snapshots().map_err(|e| e.to_string())?.into_iter().rfind(|(_, s)| s.host == host);
        *open = Some(Opened { target: settings.target.clone(), repo, parent, leftover: None });
        publish(open.as_ref().expect("opened"));
    }
    Ok(open.as_mut().expect("opened"))
}

/// Open and back up; how many snapshots there are then, what this one did,
/// and how many were forgotten after it.
fn back_up(settings: &Settings, open: &mut Option<Opened>, limits: &Limits, cancel: &AtomicBool, progress: &Progress) -> Result<(u32, olo_diskbackup::Summary, u32, Option<Check>), String> {
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
    // The plan: the last backup's size to go by (it changes little), else
    // counted first.
    match parent {
        Some((_, s)) => {
            progress.total_files.store(s.summary.files, Relaxed);
            progress.total_bytes.store(s.summary.bytes, Relaxed);
            progress.estimated.store(true, Relaxed);
        }
        None => olo_diskbackup::backup::count(&opts, cancel, progress).map_err(|e| e.to_string())?,
    }
    let host = opts.host.clone();
    let (id, snapshot) = backup(&mut o.repo, &opts, parent, cancel, progress).map_err(|e| e.to_string())?;
    let summary = snapshot.summary.clone();
    // Validated fast, now it's made: everything it needs in the store, its
    // lists read back from there, checksums compared, a sample read back.
    // (Stopped meanwhile: not checked; it's made.)
    progress.stage.store(olo_diskbackup::backup::CHECKING, Relaxed);
    progress.total_files.store(0, Relaxed);
    progress.total_bytes.store(0, Relaxed);
    progress.bytes.store(0, Relaxed);
    progress.files.store(0, Relaxed);
    let check = match olo_diskbackup::check(&o.repo, &snapshot, false, cancel, progress) {
        Ok(c) => {
            if !c.problems.is_empty() {
                tracing::warn!("diskbackup: the backup just made isn't whole: {:?}", c.problems);
            }
            let check = check_of(&c, false);
            note_check(&id, &check, &c.bad_packs);
            Some(check)
        }
        Err(olo_diskbackup::Error::Cancelled) => None,
        Err(e) => {
            let check = Check { time: now(), error: Some(e.to_string()), ..Default::default() };
            note_check(&id, &check, &[]);
            Some(check)
        }
    };
    o.parent = Some((id, snapshot));
    // (What an older one left is this one's now, or tidying's.)
    o.leftover = None;
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
    publish(o);
    Ok(((all.len() - forget.len()) as u32, summary, forget.len() as u32, check))
}

/// This computer's name (its snapshots are told apart by it).
pub fn host() -> String {
    olo_diskbackup::lock::host()
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Every thread of ours at `p`'s priority, the CPU's and the disks'
/// (threads made after inherit it). False if one couldn't be raised: a
/// process can lower its priority but not raise it again (the session's
/// RLIMIT_NICE is 0), so then it's started anew.
fn set_priority(p: Priority) -> bool {
    let (policy, nice, io) = match p {
        // The CPU's idle class; the disks' idle class.
        Priority::Idle => (libc::SCHED_IDLE, None, 3 << 13),
        // Batch, nice 19; the disks' best-effort class, its lowest level.
        Priority::Low => (libc::SCHED_BATCH, Some(19), (2 << 13) | 7),
        // As any program.
        Priority::Normal => (libc::SCHED_OTHER, Some(0), (2 << 13) | 4),
    };
    let tids: Vec<i32> = std::fs::read_dir("/proc/self/task").map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default();
    let mut ok = true;
    for tid in tids {
        // SAFETY: plain syscalls on our own threads.
        unsafe {
            let param = libc::sched_param { sched_priority: 0 };
            if libc::sched_setscheduler(tid, policy, &param) != 0 {
                ok = false;
            }
            if let Some(n) = nice
                && libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, n) != 0
            {
                ok = false;
            }
            // ioprio_set(IOPRIO_WHO_PROCESS, tid, class << 13 | level).
            if libc::syscall(libc::SYS_ioprio_set, 1, tid, io) != 0 {
                ok = false;
            }
        }
    }
    ok
}

/// Validate: backups `ids` (this computer's) checked fast, or `full`ly
/// (read back whole), one after another, said as it goes; each one's
/// result kept (and its bad packs, for the tidy after to repair).
fn validate(settings: &Settings, st: &mut State, open: &mut Option<Opened>, limits: &Limits, cancel: &AtomicBool, ids: &[Id], full: bool) {
    let progress = Progress::default();
    let mut newest = None;
    let result = reporting(st, Doing::Validating, &progress, || -> Result<usize, String> {
        let o = opened(settings, open, limits)?;
        // (Shared, as a backup's: nobody tidies meanwhile.)
        let _lock = target::lock_to_read(&o.repo, cancel, |_| {}).map_err(|e| e.to_string())?;
        o.repo.refresh().map_err(|e| e.to_string())?;
        let host = host();
        let all = o.repo.snapshots().map_err(|e| e.to_string())?;
        let mut done = 0;
        for (id, s) in all.iter().filter(|(id, s)| s.host == host && ids.contains(id)) {
            match olo_diskbackup::check(&o.repo, s, full, cancel, &progress) {
                Ok(c) => {
                    tracing::info!("diskbackup: validated {} ({}): {} read back, {} by checksum, {:?}", id.hex(), if full { "fully" } else { "fast" }, c.bytes_read, c.by_checksum, c.problems);
                    let check = check_of(&c, full);
                    note_check(id, &check, &c.bad_packs);
                    // (The newest: the header says it too.)
                    if o.parent.as_ref().is_some_and(|(p, _)| p == id) {
                        newest = Some(check);
                    }
                    done += 1;
                }
                Err(olo_diskbackup::Error::Cancelled) => return Err(olo_protocol::diskbackup::STOPPED.into()),
                Err(e) => {
                    note_check(id, &Check { time: now(), full, error: Some(e.to_string()), ..Default::default() }, &[]);
                    return Err(e.to_string());
                }
            }
            progress.bytes.store(0, Relaxed);
            progress.files.store(0, Relaxed);
        }
        Ok(done)
    });
    if let (Some(c), Some(l)) = (newest, st.last.as_mut().filter(|l| l.error.is_none())) {
        l.check = Some(c);
    }
    match result {
        Ok(n) => tracing::info!("diskbackup: {n} backups validated"),
        Err(e) => {
            tracing::warn!("diskbackup: validating: {e}");
            *open = None;
        }
    }
    st.doing = Doing::Idle;
    st.progress = None;
    let _ = st.save();
}

/// Delete backups `ids` (asked for in the window), and the unfinished one
/// if `unfinished`, and free what only they took (a tidy, as the daily
/// one).
#[allow(clippy::too_many_arguments)]
fn delete(settings: &Settings, st: &mut State, open: &mut Option<Opened>, limits: &Limits, cancel: &AtomicBool, ids: &[Id], unfinished: bool, forgotten: &mut u32) {
    let o = match opened(settings, open, limits) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("diskbackup: can't delete backups: {e}");
            return;
        }
    };
    match o.repo.lock(true) {
        Ok(lock) => {
            let mut gone = 0;
            for id in ids {
                match o.repo.forget(id) {
                    Ok(()) => gone += 1,
                    Err(e) => tracing::warn!("diskbackup: deleting backup {}: {e}", id.hex()),
                }
            }
            // The unfinished one: this computer's (all of them), and what an
            // older one left (tidying frees it).
            if unfinished {
                let host = host();
                for (id, _) in o.repo.unfinished().unwrap_or_default().into_iter().filter(|(_, s)| s.host == host) {
                    match o.repo.forget_unfinished(&id) {
                        Ok(()) => gone += 1,
                        Err(e) => tracing::warn!("diskbackup: deleting the unfinished backup {}: {e}", id.hex()),
                    }
                }
                o.leftover = None;
            }
            tracing::info!("diskbackup: {gone} backups deleted, as asked");
            // (The next backup goes by the newest left, not one gone.)
            if o.parent.as_ref().is_some_and(|(p, _)| ids.contains(p)) {
                let host = host();
                o.parent = o.repo.snapshots().ok().and_then(|all| all.into_iter().rfind(|(_, s)| s.host == host));
            }
            drop(lock);
            publish(o);
        }
        Err(e) => {
            tracing::warn!("diskbackup: can't delete backups now: {e}");
            return;
        }
    }
    tidy(settings, st, open, cancel, forgotten);
}

/// Every backup there deleted (this computer's and any other's: they share
/// it), and a new, empty one made with the same key, to start fresh. Said
/// as it goes (Erasing: so many of so many).
fn erase_all(settings: &Settings, st: &mut State, open: &mut Option<Opened>, limits: &Limits, cancel: &AtomicBool) {
    let progress = Progress::default();
    let result = reporting(st, Doing::Erasing, &progress, || -> Result<(), String> {
        let o = opened(settings, open, limits)?;
        let store = o.repo.store().clone();
        let lock = o.repo.lock(true).map_err(|e| e.to_string())?;
        let names: Vec<String> = store.list("").map_err(|e| e.to_string())?.into_iter().map(|(n, _)| n).filter(|n| !n.starts_with("locks/")).collect();
        progress.total_files.store(names.len() as u64, Relaxed);
        // The config last: until it goes, it's still a backup (a stop half
        // way leaves one with fewer snapshots, not a broken one).
        let (config, rest): (Vec<String>, Vec<String>) = names.into_iter().partition(|n| n == "config");
        for name in rest.iter().chain(&config) {
            if cancel.load(Relaxed) {
                return Err(olo_protocol::diskbackup::STOPPED.into());
            }
            if let Ok(mut c) = progress.current.lock() {
                *c = name.clone();
            }
            store.delete(name).map_err(|e| format!("{name}: {e}"))?;
            progress.files.fetch_add(1, Relaxed);
        }
        drop(lock);
        *open = None;
        if let Ok(mut l) = LOOKUP.lock() {
            *l = None;
        }
        // Its index's copy in the cache is of the backup gone.
        let _ = std::fs::remove_dir_all(target::cache());
        Repo::init(store, target::key()?, Some(target::cache())).map_err(|e| e.to_string())?;
        Ok(())
    });
    match result {
        Ok(()) => {
            tracing::info!("diskbackup: every backup erased; starting fresh");
            st.snapshots = 0;
            st.size = Some(0);
            st.last = None;
            st.last_ok = None;
            st.tidied = None;
            st.unfinished = None;
        }
        Err(e) => {
            tracing::warn!("diskbackup: erasing: {e}");
            *open = None;
            st.last = Some(Last { time: now(), error: Some(format!("erasing every backup didn't finish: {e}")), ..Default::default() });
        }
    }
    st.doing = Doing::Idle;
    st.progress = None;
    let _ = st.save();
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
                Ok(Request::Marks { dir }) => marks(&dir),
                Ok(Request::Versions { path }) => versions(&path),
                Ok(Request::Snapshots) => snapshot_list(),
                Ok(Request::Delete { snapshots, unfinished }) => {
                    let ids: Vec<Id> = snapshots.iter().filter_map(|h| Id::from_hex(h)).collect();
                    let _ = tx.send(Event::Delete(ids, unfinished));
                    Reply::Ok
                }
                Ok(Request::Validate { snapshots, full }) => {
                    let ids: Vec<Id> = snapshots.iter().filter_map(|h| Id::from_hex(h)).collect();
                    let _ = tx.send(Event::Validate(ids, full));
                    Reply::Ok
                }
                Ok(Request::EraseAll) => {
                    // (A backup under way stops first.)
                    cancel.store(true, Relaxed);
                    let _ = tx.send(Event::Erase);
                    Reply::Ok
                }
                Err(e) => Reply::Error { message: e.to_string() },
            };
            let _ = (&conn).write_all(format!("{}\n", serde_json::to_string(&reply).unwrap_or_default()).as_bytes());
        }
    });
}
