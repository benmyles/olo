//! `olo-diskbackup`: DiskBackup's app, service and command line.
//!
//! - (none): the app's window (`app.rs`): what it's doing and did, where
//!   backups go, what, when, how fast, the key.
//! - `--service`: run the service (olo-session does, in a real session).
//! - `--status [--json]`: what it's doing, has done, and its settings.
//! - `--now`, `--stop`: back up now; stop what's under way.
//! - `--set-up-folder PATH`: back up to a folder (a disk of its own): a key
//!   made (or the keyring's, for a backup there already), kept in the
//!   login keyring, and backing up turned on.
//! - `--snapshots`: every snapshot there.
//! - `--restore SNAPSHOT PATH [TO]`: put `PATH` back as it was in
//!   `SNAPSHOT` (`latest`, or the start of its id), where it was or `TO`.
//! - `--set KEY=VALUE…`: `enabled`, `battery` (on/off), `every`
//!   (minutes), `upload`, `download` (KB/s, 0 for no limit), `paths`
//!   (comma-separated), `exclude` (a pattern, added), `groups`
//!   (comma-separated ids; `--groups` lists them).

mod app;
mod service;
mod target;

const APP_ID: &str = "olo.diskbackup";

use std::{path::PathBuf, process::ExitCode, sync::atomic::AtomicBool};

use olo_diskbackup::{Key, Progress, Repo, restore};
use olo_protocol::diskbackup::{Doing, Reply, Request, Settings, State, Target};

fn main() -> ExitCode {
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
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
        Some("--set") => set(&args[1..]),
        Some("--groups") => {
            for g in olo_diskbackup::exclude::GROUPS {
                println!("{:<12} {}{}: {}", g.id, g.name, if g.default { "" } else { " (off unless chosen)" }, g.about);
            }
            Ok(())
        }
        None => return app_main(),
        _ => Err("usage: olo-diskbackup --service | --status [--json] | --now | --stop | --set-up-folder PATH | --snapshots | --restore SNAPSHOT PATH [TO] | --set KEY=VALUE… | --groups".into()),
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
    let limit = |k: u64| if k == 0 { "no limit".to_string() } else { format!("{k} KB/s") };
    println!(
        "  every {} min; {}; up {}, down {}; {} snapshots",
        settings.every_minutes,
        if settings.on_battery { "on battery too" } else { "not on battery" },
        limit(settings.upload_kbs),
        limit(settings.download_kbs),
        state.snapshots
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
        disk.update(cx, |d, cx| d.open(cx));
        let messages = disk.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |d, cx| d.message(message, cx)));
        // (Back as it was after an upgrade: its window.)
        olo_ui::on_upgrade(cx, |_| Some("{}".into()));
    });
    ExitCode::SUCCESS
}

pub(crate) fn ask(r: Request) -> Result<(), String> {
    use std::io::{BufRead, Write};
    let mut s = std::os::unix::net::UnixStream::connect(olo_protocol::diskbackup::socket()).map_err(|_| "the service isn't running".to_string())?;
    s.write_all(format!("{}\n", serde_json::to_string(&r).unwrap_or_default()).as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(&s).read_line(&mut line).map_err(|e| e.to_string())?;
    match serde_json::from_str::<Reply>(&line) {
        Ok(Reply::Ok) => Ok(()),
        Ok(Reply::Error { message }) => Err(message),
        Err(e) => Err(e.to_string()),
    }
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
        Err(olo_diskbackup::Error::WrongKey) => return Err(format!("there's a backup in {} already, and this computer's key doesn't open it", dir.display())),
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
    restore(&repo, &s.tree, &path, &to, &cancel, &progress).map_err(|e| e.to_string())?;
    println!("Put back {} as {} ({} files).", path.display(), to.display(), progress.files.load(std::sync::atomic::Ordering::Relaxed));
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
            "groups" => s.exclude_groups = Some(v.split(',').map(|g| g.trim().to_owned()).filter(|g| !g.is_empty()).collect()),
            _ => return Err(format!("{k}: enabled, battery, every, upload, download, paths, exclude or groups")),
        }
    }
    s.save().map_err(|e| e.to_string())
}
