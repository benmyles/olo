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
//! - `--set-up-drive [CLIENT_FILE | CLIENT_ID CLIENT_SECRET]`: back up to
//!   Google Drive with the person's own OAuth client (its downloaded JSON,
//!   or its ID and secret; none: the one Passwords keeps): the browser
//!   sent to Google to sign in (its address printed too), then as
//!   `--set-up-folder`. Making the client: the app's Google Drive… walks
//!   through it.
//! - `--snapshots`: every snapshot there.
//! - `--restore SNAPSHOT PATH [TO]`: put `PATH` back as it was in
//!   `SNAPSHOT` (`latest`, or the start of its id), where it was or `TO`.
//! - `--keep-key`: the key kept in Passwords (started if it isn't
//!   running; unlocked if it asks), as the app's Keep Key in Passwords
//!   does (Olo Doctor's fix).
//! - `--tidy`: now, not waiting for the service's daily one: this
//!   computer's snapshots past keeping forgotten, and the space only they
//!   used freed (with the backup to itself: not while one runs). Asked of
//!   the service if it's running (its state says how it went), else here.
//! - `--delete SNAPSHOT…`: those backups deleted (ids, or their starts, as
//!   `--snapshots` lists them; `unfinished`, the one that didn't finish),
//!   and the space only they took freed, as the window's Backups ▸
//!   Delete… does (asked of the service).
//! - `--validate [--full] SNAPSHOT…`: those backups (ids or their starts,
//!   `latest`, `all`) validated, as the window's Backups ▸ Validate does
//!   (asked of the service): fast (what they need all there, checksums
//!   compared, samples read back), or with `--full` every byte read back;
//!   what's damaged is repaired as far as it can be. Each backup is
//!   validated fast once it's made.
//! - `--erase-all --yes`: every backup there deleted, to start fresh
//!   (File ▸ Erase All Backups…).
//! - `--set KEY=VALUE…`: `enabled`, `automatic`, `battery`, `deskbar`
//!   (on/off; `automatic` off: only when asked, `--now`), `priority`
//!   (`idle`, `low` or `normal`), `every`
//!   (minutes), `upload`, `download` (KB/s, 0 for no limit), `paths`
//!   (comma-separated), `exclude` (a pattern, added), `groups`
//!   (comma-separated ids; `--groups` lists them), `hourly`, `daily`,
//!   `weekly`, `monthly` (how many of each to keep).

mod agent_setup;
mod app;
mod drive_setup;
#[cfg(test)]
mod lint;
mod restore;
mod service;
mod target;
mod versions;
mod window;

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
            Some("--set-up-drive") => set_up_drive_cli(&args[1..]),
            Some("--agent-setup-done") => agent_setup_done(&args[1..]),
            Some("--snapshots") => snapshots(),
            Some("--delete") if args.len() > 1 => delete_cli(&args[1..]),
            Some("--validate") if args.len() > 1 => validate_cli(&args[1..]),
            Some("--erase-all") => {
                if args.iter().any(|a| a == "--yes") {
                    ask(Request::EraseAll).map(|()| println!("Erasing every backup; the next one starts afresh (--status says how it goes)."))
                } else {
                    Err("--erase-all deletes every backup there, for good: add --yes".into())
                }
            }
            Some("--restore") => match (args.get(1), args.get(2)) {
                (Some(snap), Some(path)) => restore_cli(snap, path, args.get(3)),
                _ => Err("--restore SNAPSHOT PATH [TO]".into()),
            },
            Some("--tidy") => tidy(),
        Some("--keep-key") => app::keep_in_passwords().and_then(|()| {
            let mut s = Settings::load();
            s.key_in_passwords = true;
            s.save().map_err(|e| e.to_string())?;
            println!("DiskBackup's key is kept in Passwords (“{}”).", target::KEY_TITLE);
            Ok(())
        }),
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
            _ => Err("usage: olo-diskbackup --service | --status [--json] | --now | --stop | --set-up-folder PATH | --set-up-drive [CLIENT_FILE | ID SECRET] | --agent-setup-done [CLIENT_FILE] [--screen NAME] | --snapshots | --delete SNAPSHOT… | --validate [--full] SNAPSHOT… | --erase-all --yes | --restore SNAPSHOT PATH [TO] | --marks DIR | --versions PATH | --keep-key | --tidy | --set KEY=VALUE… | --groups"
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
    if let Some(u) = &state.unfinished {
        println!("  unfinished: {} — {} files, {} so far; {} sent (the next goes on from it; --delete unfinished frees it)", when(u.time), u.files, mb(u.bytes), mb(u.sent));
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
    set_up(t, store, &dir.display().to_string())
}

/// Sign in to Google with `client` (its consent page given to `open`: the
/// browser); blocks until Google answers, or `cancel`.
pub(crate) fn sign_in(client: &olo_diskbackup::oauth::Client, cancel: &AtomicBool, open: impl FnOnce(&str)) -> Result<olo_diskbackup::oauth::SignedIn, String> {
    client.check()?;
    let client = olo_diskbackup::oauth::Client { id: client.id.trim().to_owned(), secret: client.secret.trim().to_owned() };
    let s = olo_diskbackup::oauth::SignIn::start(&client, &target::endpoints()).map_err(|e| e.to_string())?;
    open(s.url());
    s.wait(&olo_diskbackup::oauth::agent(), cancel)
}

/// Back up to Google Drive, signed in: the sign-in kept in the keyring, the
/// backups' folder found or made, the backup there opened or made; the
/// client kept in Passwords too (for another computer), if it can be.
pub(crate) fn set_up_drive(client: olo_diskbackup::oauth::Client, signed: olo_diskbackup::oauth::SignedIn) -> Result<String, String> {
    let client = olo_diskbackup::oauth::Client { id: client.id.trim().to_owned(), secret: client.secret.trim().to_owned() };
    let folder = olo_diskbackup::drive::Drive::folder(&client, &signed.refresh_token, &target::endpoints())?;
    let g = target::Google { client, refresh_token: signed.refresh_token };
    target::keep_google(&g)?;
    let t = Target::GoogleDrive { account: signed.email.clone(), folder };
    let store = target::store(&t, &target::Limits::new())?;
    set_up(t, store, "Google Drive")?;
    if std::env::var_os("OLO_DISKBACKUP_GOOGLE").is_none()
        && let Err(e) = target::keep_client(&g.client)
    {
        tracing::info!("diskbackup: the Google client not kept in Passwords: {e}");
    }
    Ok(signed.email)
}

/// Back up to `t` (`store` its store; `said`, what it's called): a backup
/// there already opened with this computer's key (or one Passwords keeps),
/// else one made with it (or a new key); the key kept in the keyring, the
/// settings saying `t`, backing up on.
fn set_up(t: Target, store: std::sync::Arc<dyn olo_diskbackup::Store>, said: &str) -> Result<(), String> {
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
            let keys = target::keys_in_passwords().map_err(|e| format!("there's a backup in {said} already: its key is needed, and Passwords couldn't give it ({e})"))?;
            keys.into_iter()
                .find_map(|k| Repo::open(store.clone(), k, None).ok().map(|r| r.key().clone()))
                .ok_or_else(|| format!("there's a backup in {said} already, and none of the keys in Passwords (“{}”) opens it", target::KEY_TITLE))?
        }
        Err(e) => return Err(e.to_string()),
    };
    target::keep_key(&key)?;
    let mut s = Settings::load();
    s.target = t;
    s.enabled = true;
    s.save().map_err(|e| e.to_string())?;
    tracing::info!("backing up to {said} (its key kept in the login keyring)");
    Ok(())
}

fn set_up_drive_cli(a: &[String]) -> Result<(), String> {
    let client = match a {
        [file] => olo_diskbackup::oauth::Client::from_json(&std::fs::read_to_string(target::expand(file)).map_err(|e| format!("{file}: {e}"))?)?,
        [id, secret] => olo_diskbackup::oauth::Client { id: id.clone(), secret: secret.clone() },
        [] => target::client_in_passwords()?.ok_or("no Google client is kept in Passwords: give its file (Download JSON), or its ID and secret")?,
        _ => return Err("--set-up-drive [CLIENT_FILE | CLIENT_ID CLIENT_SECRET]".into()),
    };
    let cancel = AtomicBool::new(false);
    let signed = sign_in(&client, &cancel, |url| {
        println!("Sign in to Google in your browser (DiskBackup opened it), or open this:\n\n  {url}\n");
        // (Not with a test's own Google: no browser.)
        if std::env::var_os("OLO_DISKBACKUP_GOOGLE_API").is_none() {
            let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(url.to_owned()));
        }
    })?;
    println!("Signed in as {}; setting up…", signed.email);
    let email = set_up_drive(client, signed)?;
    println!("Backing up to Google Drive ({email}), in the folder “{}”.", olo_diskbackup::drive::FOLDER);
    Ok(())
}

/// Agent Setup's last step, run by the agent (`agent_setup`): the client
/// (its file, else the one Passwords keeps) kept in Passwords, DiskBackup
/// signed in with it (Google's consent page on the agent's screen, for it
/// to allow; with no screen, the browser), set up, and DiskBackup's
/// window told. Says how it goes, for the agent to read; gives up after
/// 20 minutes.
fn agent_setup_done(a: &[String]) -> Result<(), String> {
    let mut file = None;
    let mut screen = None;
    let mut it = a.iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--screen" => screen = it.next().cloned(),
            f if !f.starts_with('-') && file.is_none() => file = Some(f.to_owned()),
            other => return Err(format!("{other}? --agent-setup-done [CLIENT_FILE] [--screen NAME]")),
        }
    }
    let client = match &file {
        Some(f) => olo_diskbackup::oauth::Client::from_json(&std::fs::read_to_string(target::expand(f)).map_err(|e| format!("{f}: {e}"))?)?,
        None => target::client_in_passwords()?.ok_or("no client file given, and Passwords keeps none")?,
    };
    client.check()?;
    if file.is_some() {
        println!("Keeping the client in Olo Passwords (the person may be asked to unlock it)…");
        target::keep_client(&client)?;
        println!("Kept in Passwords as “{}”.", target::CLIENT_TITLE);
    }
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(20 * 60));
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }
    let signed = sign_in(&client, &cancel, |url| match &screen {
        Some(s) => {
            println!(
                "Google's consent page is open on screen “{s}”: choose the account (sign in if asked), then Continue or Allow. DiskBackup waits for it.

  {url}
"
            );
            let opened = std::process::Command::new(olo_xdg::find_program("olo-screen").map(|p| p.into_os_string()).unwrap_or_else(|| "olo-screen".into()))
                .args(["run", s, "olo-chrome", url])
                .stdout(std::process::Stdio::null())
                .status();
            if !opened.is_ok_and(|st| st.success()) {
                println!("(It didn't open there: open the address above on the screen.)");
            }
        }
        None => {
            println!(
                "Sign in to Google in the browser (DiskBackup opened it), or open this:

  {url}
"
            );
            if std::env::var_os("OLO_DISKBACKUP_GOOGLE_API").is_none() {
                let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(url.to_owned()));
            }
        }
    })?;
    // (A test of the agent against the real Google: signed in, nothing
    // made in its Drive, nothing set up here.)
    if std::env::var_os("OLO_DISKBACKUP_AGENT_CHECK_ONLY").is_some() {
        println!("Signed in as {}. (Checking only, for a test: DiskBackup isn't set up.)", signed.email);
        return Ok(());
    }
    println!("Signed in as {}; setting up…", signed.email);
    let email = set_up_drive(client, signed)?;
    println!("Backing up to Google Drive ({email}), in the folder “{}”. DiskBackup is set up.", olo_diskbackup::drive::FOLDER);
    // DiskBackup's window, if it's open, says so (not started for it).
    if olo_ui::instance::running(APP_ID) {
        olo_ui::instance::forward(APP_ID, &["--agent-setup-done".to_owned()]);
    }
    Ok(())
}

/// This computer's backups by id (or an id's start; `latest`, `all`), as
/// the service lists them.
fn chosen(wanted: &[String]) -> Result<Vec<String>, String> {
    let all = match olo_protocol::diskbackup::call(&Request::Snapshots)? {
        olo_protocol::diskbackup::Reply::Snapshots { snapshots } => snapshots,
        other => return Err(format!("the service answered {other:?}")),
    };
    let mut ids = Vec::new();
    for w in wanted {
        let found: Vec<&olo_protocol::diskbackup::SnapshotInfo> = match w.as_str() {
            "all" => all.iter().collect(),
            "latest" => all.first().into_iter().collect(),
            _ => all.iter().filter(|s| s.id.starts_with(w.as_str())).collect(),
        };
        match found.as_slice() {
            [] => return Err(format!("no backup {w}")),
            [_, _, ..] if w != "all" => return Err(format!("{w} is the start of more than one backup's id")),
            f => ids.extend(f.iter().map(|s| s.id.clone())),
        }
    }
    Ok(ids)
}

/// Backups read back whole and checked, by the service.
fn validate_cli(args: &[String]) -> Result<(), String> {
    let full = args.iter().any(|a| a == "--full");
    let wanted: Vec<String> = args.iter().filter(|a| *a != "--full").cloned().collect();
    let ids = chosen(&wanted)?;
    let n = ids.len();
    ask(Request::Validate { snapshots: ids, full })?;
    println!(
        "Validating {} {}: --status says how it goes, --snapshots how each was last checked.",
        if n == 1 { "1 backup".to_owned() } else { format!("{n} backups") },
        if full { "fully (every byte read back)" } else { "fast (checksums, and samples read back)" }
    );
    Ok(())
}

/// Backups deleted by id (or an id's start), by the service.
fn delete_cli(wanted: &[String]) -> Result<(), String> {
    let all = match olo_protocol::diskbackup::call(&Request::Snapshots)? {
        olo_protocol::diskbackup::Reply::Snapshots { snapshots } => snapshots,
        other => return Err(format!("the service answered {other:?}")),
    };
    let mut ids = Vec::new();
    // ("unfinished": the backup that didn't finish.)
    let unfinished = wanted.iter().any(|w| w == "unfinished");
    if unfinished && State::load().unfinished.is_none() {
        return Err("no backup is unfinished".into());
    }
    for w in wanted.iter().filter(|w| *w != "unfinished") {
        let found: Vec<&olo_protocol::diskbackup::SnapshotInfo> = all.iter().filter(|s| s.id.starts_with(w.as_str())).collect();
        match found.as_slice() {
            [one] => ids.push(one.id.clone()),
            [] => return Err(format!("no backup {w}")),
            _ => return Err(format!("{w} is the start of more than one backup's id")),
        }
    }
    let n = ids.len() + usize::from(unfinished);
    ask(Request::Delete { snapshots: ids, unfinished })?;
    println!("{} deleted; the space only they took is being freed.", if n == 1 { "1 backup".to_owned() } else { format!("{n} backups") });
    Ok(())
}

fn snapshots() -> Result<(), String> {
    let (_, repo) = open()?;
    // (How each was last checked: the service keeps that.)
    let checks: std::collections::HashMap<String, olo_protocol::diskbackup::Check> = match olo_protocol::diskbackup::call(&Request::Snapshots) {
        Ok(Reply::Snapshots { snapshots }) => snapshots.into_iter().filter_map(|s| s.check.map(|c| (s.id, c))).collect(),
        _ => Default::default(),
    };
    for (id, s) in repo.snapshots().map_err(|e| e.to_string())? {
        let check = match checks.get(&id.hex()) {
            None => String::new(),
            Some(c) if c.error.is_some() => format!("; not checked {}", when(c.time)),
            Some(c) if !c.problems.is_empty() => format!("; DAMAGED ({}): {}", when(c.time), c.problems.join("; ")),
            Some(c) => format!("; validated {} {}", if c.full { "fully" } else { "fast" }, when(c.time)),
        };
        println!("{}  {}  {}  {} files, {}, {} new{check}", &id.hex()[..12], when(s.time), s.host, s.summary.files, mb(s.summary.bytes), mb(s.summary.new_bytes));
    }
    for (id, s) in repo.unfinished().map_err(|e| e.to_string())? {
        println!("{}  {}  {}  UNFINISHED: {} files, {} so far, {} sent", &id.hex()[..12], when(s.time), s.host, s.summary.files, mb(s.summary.bytes), mb(s.summary.stored_bytes));
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
            "automatic" => s.automatic = flag()?,
            "priority" => {
                s.priority = match v {
                    "idle" => olo_protocol::diskbackup::Priority::Idle,
                    "low" => olo_protocol::diskbackup::Priority::Low,
                    "normal" => olo_protocol::diskbackup::Priority::Normal,
                    _ => return Err(format!("priority={v}? idle, low or normal")),
                }
            }
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
