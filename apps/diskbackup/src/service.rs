//! The backup service (`olo-diskbackup --service`; olo-session runs it):
//! asleep until a backup is due (`every_minutes` after the last), its
//! settings change, or it's asked (its socket: Back Up Now, Stop); then a
//! backup, gently (the whole process at the CPU's and the disks' idle
//! priority: anything else that wants them goes first), its progress in
//! the state file every 2 s while it runs and not otherwise. Speed limits
//! follow the settings at once, even mid-backup.

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

use olo_diskbackup::{Id, Options, Progress, Repo, Snapshot, backup, exclude::Excludes};
use olo_protocol::diskbackup::{Doing, Last, Reply, Request, Settings, State, Target};

use crate::target::{self, Limits};

enum Event {
    Settings,
    Now,
}

/// What's kept between runs of the service (in the state folder).
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Kept {
    last: Option<Last>,
    last_ok: Option<i64>,
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
    let mut st = State { pid: std::process::id(), last: k.last, last_ok: k.last_ok, ..State::default() };
    let mut open: Option<Opened> = None;
    let mut now_asked = false;
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
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                _ => {}
            }
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
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                _ => {}
            }
            continue;
        }
        now_asked = false;
        cancel.store(false, Relaxed);
        once(&settings, &mut st, &mut open, &limits, &cancel);
        let k = Kept { last: st.last.clone(), last_ok: st.last_ok };
        if let Some(dir) = kept_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(kept_path(), serde_json::to_vec(&k).unwrap_or_default());
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

/// One backup, start to end, the state said throughout.
fn once(settings: &Settings, st: &mut State, open: &mut Option<Opened>, limits: &Limits, cancel: &AtomicBool) {
    st.doing = Doing::BackingUp;
    st.next = None;
    st.progress = Some(olo_protocol::diskbackup::Progress { started: now(), ..Default::default() });
    let _ = st.save();
    let progress = Progress::default();
    let done = AtomicBool::new(false);
    let base = st.clone();
    let result = std::thread::scope(|s| {
        // The state file as it goes, every 2 s while it runs.
        s.spawn(|| {
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
        let r = back_up(settings, open, limits, cancel, &progress);
        done.store(true, Relaxed);
        r
    });
    let time = now();
    st.last = Some(match result {
        Ok((snapshots, summary)) => {
            st.snapshots = snapshots;
            st.last_ok = Some(time);
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
}

/// Open (once, or for a new target) and back up; how many snapshots there
/// are then, and what this one did.
fn back_up(settings: &Settings, open: &mut Option<Opened>, limits: &Limits, cancel: &AtomicBool, progress: &Progress) -> Result<(u32, olo_diskbackup::Summary), String> {
    let host = host();
    if open.as_ref().is_none_or(|o| o.target != settings.target) {
        *open = None;
        let store = target::store(&settings.target, limits)?;
        let key = target::key()?;
        let repo = Repo::open(store, key, Some(target::cache())).map_err(|e| e.to_string())?;
        let parent = repo.snapshots().map_err(|e| e.to_string())?.into_iter().rfind(|(_, s)| s.host == host);
        *open = Some(Opened { target: settings.target.clone(), repo, parent });
    }
    let o = open.as_mut().expect("opened");
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
    let (id, snapshot) = backup(&mut o.repo, &opts, parent, cancel, progress).map_err(|e| e.to_string())?;
    let summary = snapshot.summary.clone();
    o.parent = Some((id, snapshot));
    let count = o.repo.snapshots().map(|s| s.len() as u32).unwrap_or(0);
    Ok((count, summary))
}

/// This computer's name (its snapshots are told apart by it).
pub fn host() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: a buffer of its length; the name is NUL-terminated in it.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    let name = if ok { String::from_utf8_lossy(&buf[..buf.iter().position(|b| *b == 0).unwrap_or(0)]).into_owned() } else { String::new() };
    if name.is_empty() { "this computer".into() } else { name }
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
                Err(e) => Reply::Error { message: e.to_string() },
            };
            let _ = (&conn).write_all(format!("{}\n", serde_json::to_string(&reply).unwrap_or_default()).as_bytes());
        }
    });
}
