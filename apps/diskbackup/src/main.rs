//! `olo-diskbackup`: DiskBackup's app, service and command line.
//!
//! - (none): the app's window (`app.rs`): what it's doing and did, where
//!   backups go, what, when, how fast, the key.
//! - `--restore-window`: the app's Restore window (Deskbar's Restore
//!   Files…).
//! - `--versions-window PATH`: the app's window of `PATH`'s versions, to
//!   put one back (Tracker's Older Versions…).
//! - `--marks DIR`: which of `DIR`'s files are backed up as they are now
//!   (Tracker's ticks; asked of the service).
//! - `--versions PATH`: the versions of `PATH` the backups kept.
//! - `--service`: run the service (olo-session does, in a real session).
//! - `--status [--json]`: what it's doing, has done, and its settings.
//! - `--now`, `--stop`: back up now; stop what's under way.
//! - `--set-up-folder PATH`: back up to a folder (a disk of its own): a key
//!   made (or the keyring's, for a backup there already), kept in the
//!   login keyring, and backing up turned on.
//! - `--snapshots`: every snapshot there.
//! - `--restore SNAPSHOT PATH [TO]`: put `PATH` back as it was in
//!   `SNAPSHOT` (`latest`, or the start of its id), where it was or `TO`.
//! - `--tidy`: now, not waiting for the service's daily one: this
//!   computer's snapshots past keeping forgotten, and the space only they
//!   used freed (with the backup to itself: not while one runs). Asked of
//!   the service if it's running (its state says how it went), else here.
//! - `--set KEY=VALUE…`: `enabled`, `battery`, `deskbar` (on/off), `every`
//!   (minutes), `upload`, `download` (KB/s, 0 for no limit), `paths`
//!   (comma-separated), `exclude` (a pattern, added), `groups`
//!   (comma-separated ids; `--groups` lists them), `hourly`, `daily`,
//!   `weekly`, `monthly` (how many of each to keep).

mod app;
mod restore;
mod service;
mod target;
mod versions;

const APP_ID: &str = "olo.diskbackup";

use std::{path::PathBuf, process::ExitCode, sync::atomic::AtomicBool};

use olo_diskbackup::{Key, Progress, Repo};
use olo_protocol::diskbackup::{Doing, Reply, Request, Settings, State, Target};

fn main() -> ExitCode {
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r =
        match args.first().map(String::as_str) {
            Some("--service") => return service::run(),
            Some("--status") => {
                status(args.iter().any(|a| a == "--json"));
                Ok(())
            }
            Some("--now") => ask(Request::BackUpNow),
            Some("--stop") => ask(Request::Stop),
            Some("--set-up-folder") => match args.get(1) {
                Some(p) => set_up_folder(p),
                None => Err("--set-up-folder PATH".into()),
            },
            Some("--snapshots") => snapshots(),
            Some("--restore") => match (args.get(1), args.get(2)) {
                (Some(snap), Some(path)) => restore_cli(snap, path, args.get(3)),
                _ => Err("--restore SNAPSHOT PATH [TO]".into()),
            },
            Some("--tidy") => tidy(),
            Some("--set") => set(&args[1..]),
            Some("--groups") => {
                for g in olo_diskbackup::exclude::GROUPS {
                    println!("{:<12} {}{}: {}", g.id, g.name, if g.default { "" } else { " (off unless chosen)" }, g.about);
                }
                Ok(())
            }
            None | Some("--restore-window") | Some("--versions-window") => return app_main(),
        Some("--marks") => match args.get(1) {
            Some(dir) => marks_cli(dir),
            None => Err("--marks DIR".into()),
        },
        Some("--versions") => match args.get(1) {
            Some(path) => versions_cli(path),
            None => Err("--versions PATH".into()),
        },
            _ => Err("usage: olo-diskbackup --service | --status [--json] | --now | --stop | --set-up-folder PATH | --snapshots | --restore SNAPSHOT PATH [TO] | --marks DIR | --versions PATH | --tidy | --set KEY=VALUE… | --groups"
                .into()),
        };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("olo-diskbackup: {e}");
            ExitCode::FAILURE
        }
    }
}

fn mb(b: u64) -> String {
    if b >= 1_000_000_000 { format!("{:.1} GB", b as f64 / 1e9) } else { format!("{:.1} MB", b as f64 / 1e6) }
}

fn when(t: i64) -> String {
    let ago = service::now() - t;
    match ago {
        i64::MIN..=-60 => format!("in {} min", -ago / 60),
        -59..=59 => "now".into(),
        60..=3599 => format!("{} min ago", ago / 60),
        3600..=86399 => format!("{} h ago", ago / 3600),
        _ => format!("{} days ago", ago / 86400),
    }
}

fn status(json: bool) {
    let (state, settings) = (State::load(), Settings::load());
    if json {
        println!("{}", serde_json::json!({ "state": state, "settings": settings }));
        return;
    }
    println!("DiskBackup: {} to {}", if settings.enabled { "backing up" } else { "off" }, settings.target.describe());
    if state.pid == 0 {
        println!("  the service isn't running");
    }
    match &state.doing {
        Doing::Waiting { why } => println!("  waiting: {why}"),
        Doing::BackingUp => {
            if let Some(p) = &state.progress {
                println!("  backing up: {} files, {} read, {} new; {}", p.files, mb(p.bytes), mb(p.new_bytes), p.current);
            }
        }
        d => println!("  {d:?}"),
    }
    if let Some(l) = &state.last {
        match &l.error {
            Some(e) => println!("  last: {} — didn't finish: {e}", when(l.time)),
            None => println!(
                "  last: {} — {} files ({} new, {} changed), {}; {} new, {} up, {:.0} s{}",
                when(l.time),
                l.files,
                l.new_files,
                l.changed_files,
                mb(l.bytes),
                mb(l.new_bytes),
                mb(l.uploaded),
                l.seconds,
                if l.unreadable > 0 { format!("; {} unreadable", l.unreadable) } else { String::new() }
            ),
        }
    }
    if let Some(n) = state.next {
        println!("  next: {}", when(n));
    }
    if let Some(t) = &state.tidied {
        match &t.error {
            Some(e) => println!("  tidied: {} — didn't finish: {e}", when(t.time)),
            None => println!("  tidied: {} — {} snapshot{} let go of, {} freed", when(t.time), t.forgotten, if t.forgotten == 1 { "" } else { "s" }, mb(t.freed)),
        }
    }
    if let Some(size) = state.size {
        println!("  takes {} there", mb(size));
    }
    let limit = |k: u64| if k == 0 { "no limit".to_string() } else { format!("{k} KB/s") };
    let k = settings.keep;
    println!(
        "  every {} min; {}; up {}, down {}; {} snapshots (keeping {} hourly, {} daily, {} weekly, {} monthly)",
        settings.every_minutes,
        if settings.on_battery { "on battery too" } else { "not on battery" },
        limit(settings.upload_kbs),
        limit(settings.download_kbs),
        state.snapshots,
        k.hourly,
        k.daily,
        k.weekly,
        k.monthly
    );
}

/// The app: one window, one copy (another launch brings it forward).
fn app_main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if olo_ui::instance::forward(APP_ID, &args) {
        return ExitCode::SUCCESS;
    }
    olo_ui::run(APP_ID, move |cx| {
        use olo_ui::gpui::AppContext as _;
        let proxy = olo_ui::proxy(cx);
        olo_ui::instance::listen(APP_ID, proxy.clone());
        let disk = cx.new(|_| app::Disk::new(proxy));
        let cwd = std::env::current_dir().ok();
        disk.update(cx, |d, cx| d.launched(&args, cwd.as_deref(), cx));
        let messages = disk.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |d, cx| d.message(message, cx)));
        // (Back as it was after an upgrade: its window.)
        olo_ui::on_upgrade(cx, |_| Some("{}".into()));
    });
    ExitCode::SUCCESS
}

pub(crate) fn ask(r: Request) -> Result<(), String> {
    olo_protocol::diskbackup::call(&r).map(|_| ())
}

/// The repository as the settings say, opened.
fn open() -> Result<(Settings, Repo), String> {
    let settings = Settings::load();
    let limits = target::Limits::new();
    limits.follow(&settings);
    let store = target::store(&settings.target, &limits)?;
    let repo = Repo::open(store, target::key()?, Some(target::cache())).map_err(|e| e.to_string())?;
    Ok((settings, repo))
}

pub(crate) fn set_up_folder(path: &str) -> Result<(), String> {
    let dir = PathBuf::from(path);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dir = dir.canonicalize().map_err(|e| e.to_string())?;
    let limits = target::Limits::new();
    let t = Target::Folder { path: dir.display().to_string() };
    let store = target::store(&t, &limits)?;
    // A backup there already: the keyring's key must open it.
    let key = match Repo::open(store.clone(), target::key().unwrap_or_else(|_| Key::generate()), None) {
        Ok(r) => r.key().clone(),
        Err(olo_diskbackup::Error::NoRepository) => {
            let key = target::key().unwrap_or_else(|_| Key::generate());
            Repo::init(store, key.clone(), Some(target::cache())).map_err(|e| e.to_string())?;
            key
        }
        // Another computer's (or this one's, set up again): its key from
        // Passwords, which keeps every one DiskBackup made.
        Err(olo_diskbackup::Error::WrongKey) => {
            let keys = target::keys_in_passwords().map_err(|e| format!("there's a backup in {} already: its key is needed, and Passwords couldn't give it ({e})", dir.display()))?;
            keys.into_iter()
                .find_map(|k| Repo::open(store.clone(), k, None).ok().map(|r| r.key().clone()))
                .ok_or_else(|| format!("there's a backup in {} already, and none of the keys in Passwords (“{}”) opens it", dir.display(), target::KEY_TITLE))?
        }
        Err(e) => return Err(e.to_string()),
    };
    target::keep_key(&key)?;
    let mut s = Settings::load();
    s.target = t;
    s.enabled = true;
    s.save().map_err(|e| e.to_string())?;
    tracing::info!("backing up to {} (its key kept in the login keyring)", dir.display());
    Ok(())
}

fn snapshots() -> Result<(), String> {
    let (_, repo) = open()?;
    for (id, s) in repo.snapshots().map_err(|e| e.to_string())? {
        println!("{}  {}  {}  {} files, {}, {} new", &id.hex()[..12], when(s.time), s.host, s.summary.files, mb(s.summary.bytes), mb(s.summary.new_bytes));
    }
    Ok(())
}

fn restore_cli(snap: &str, path: &str, to: Option<&String>) -> Result<(), String> {
    let (_, repo) = open()?;
    let all = repo.snapshots().map_err(|e| e.to_string())?;
    let found = if snap == "latest" { all.last() } else { all.iter().find(|(id, _)| id.hex().starts_with(snap)) };
    let (_, s) = found.ok_or_else(|| format!("no snapshot {snap}"))?;
    let path = target::expand(path).canonicalize().unwrap_or_else(|_| target::expand(path));
    let to = to.map(|t| target::expand(t)).unwrap_or_else(|| path.clone());
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let said = std::sync::Once::new();
    let _lock = target::lock_to_read(&repo, &cancel, |why| said.call_once(|| eprintln!("Waiting: {why}…"))).map_err(|e| e.to_string())?;
    olo_diskbackup::restore(&repo, &s.tree, &path, &to, &cancel, &progress).map_err(|e| e.to_string())?;
    println!("Put back {} as {} ({} files).", path.display(), to.display(), progress.files.load(std::sync::atomic::Ordering::Relaxed));
    Ok(())
}

/// An absolute path for `p` (`~` the home folder; relative: here).
fn absolute(p: &str) -> String {
    let p = target::expand(p);
    std::path::absolute(&p).unwrap_or(p).display().to_string()
}

fn marks_cli(dir: &str) -> Result<(), String> {
    match olo_protocol::diskbackup::call(&Request::Marks { dir: absolute(dir) })? {
        Reply::Marks { covered: false, .. } => println!("Not in the backups."),
        Reply::Marks { backed_up, time, .. } => {
            println!("Backed up as they are now (snapshot {}):", when(time));
            for name in backed_up {
                println!("  {name}");
            }
        }
        r => return Err(format!("the service answered {r:?}")),
    }
    Ok(())
}

fn versions_cli(path: &str) -> Result<(), String> {
    match olo_protocol::diskbackup::call(&Request::Versions { path: absolute(path) })? {
        Reply::Versions { versions } if versions.is_empty() => println!("No versions kept."),
        Reply::Versions { versions } => {
            for v in versions {
                println!(
                    "{}  {}  modified {}, kept {} to {}{}",
                    &v.snapshot[..12],
                    mb(v.size),
                    when(v.mtime / 1_000_000_000),
                    when(v.first),
                    when(v.time),
                    if v.current { "  (what's there now)" } else { "" }
                );
            }
        }
        r => return Err(format!("the service answered {r:?}")),
    }
    Ok(())
}

fn tidy() -> Result<(), String> {
    if State::load().pid != 0 {
        ask(Request::Tidy)?;
        println!("Asked the service to tidy: `olo-diskbackup --status` says how it went.");
        return Ok(());
    }
    let (settings, mut repo) = open()?;
    let _lock = repo.lock(true).map_err(|e| e.to_string())?;
    let host = service::host();
    let mine: Vec<_> = repo.snapshots().map_err(|e| e.to_string())?.into_iter().filter(|(_, s)| s.host == host).collect();
    let forget = olo_diskbackup::keep::to_forget(&mine, service::policy(settings.keep));
    for id in &forget {
        repo.forget(id).map_err(|e| e.to_string())?;
    }
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let p = olo_diskbackup::prune(&mut repo, &cancel, &progress).map_err(|e| e.to_string())?;
    println!(
        "Let go of {} snapshot{}; {} packs deleted, {} copied from into {}; {} freed ({} fetched, {} written); takes {} now; {:.1} s.",
        forget.len(),
        if forget.len() == 1 { "" } else { "s" },
        p.packs_deleted,
        p.packs_repacked,
        p.packs_written,
        mb(p.bytes_deleted.saturating_sub(p.bytes_written)),
        mb(p.bytes_read),
        mb(p.bytes_written),
        mb(p.size),
        p.seconds
    );
    if p.missing > 0 {
        println!("{} pieces the snapshots use aren't there: the next backup puts back what this computer still has.", p.missing);
    }
    Ok(())
}

fn set(pairs: &[String]) -> Result<(), String> {
    let mut s = Settings::load();
    for pair in pairs {
        let (k, v) = pair.split_once('=').ok_or_else(|| format!("{pair}: KEY=VALUE"))?;
        let flag = || match v {
            "on" | "true" | "yes" | "1" => Ok(true),
            "off" | "false" | "no" | "0" => Ok(false),
            _ => Err(format!("{k}: on or off")),
        };
        let number = || v.parse::<u64>().map_err(|_| format!("{k}: a number"));
        match k {
            "enabled" => s.enabled = flag()?,
            "battery" => s.on_battery = flag()?,
            "every" => s.every_minutes = number()?.clamp(5, 7 * 24 * 60) as u32,
            "upload" => s.upload_kbs = number()?,
            "download" => s.download_kbs = number()?,
            "paths" => s.paths = v.split(',').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect(),
            "exclude" => s.excludes.push(v.to_owned()),
            "deskbar" => s.in_deskbar = flag()?,
            "groups" => s.exclude_groups = Some(v.split(',').map(|g| g.trim().to_owned()).filter(|g| !g.is_empty()).collect()),
            "hourly" => s.keep.hourly = number()?.min(u32::MAX as u64) as u32,
            "daily" => s.keep.daily = number()?.min(u32::MAX as u64) as u32,
            "weekly" => s.keep.weekly = number()?.min(u32::MAX as u64) as u32,
            "monthly" => s.keep.monthly = number()?.min(u32::MAX as u64) as u32,
            _ => return Err(format!("{k}: enabled, battery, deskbar, every, upload, download, paths, exclude, groups, hourly, daily, weekly or monthly")),
        }
    }
    s.save().map_err(|e| e.to_string())
}
