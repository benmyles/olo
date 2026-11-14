//! olo-pool (Token Pool): several Claude and ChatGPT subscriptions
//! shared by Claude Code and Codex on this computer. A gateway on
//! loopback (`olo_token_pool::gateway`) takes the clients' requests
//! and sends each with whichever account's quota resets soonest, so less
//! of it goes unused; a sign-in the provider refuses is refreshed, an
//! account out of quota waits for its reset while the next serves.
//!
//! The session runs it in the background (`--service`); the window shows
//! the gateway, which clients use it, and each account's quota, usage
//! and state, live, and adds accounts: this computer's own sign-in, or a
//! separate one made in Terminal by the client's own sign-in. Sign-ins
//! are kept sealed in `pool.vault`. `--status` (`--json`) tells agents
//! the same, without the sign-ins. Its Clients window (`clients_window`)
//! shows the pool's activity over time and what of Claude Code's and
//! Codex's is running, with Codex's service and its Restart;
//! `--clients` (`--json`) and `--restart-codex-service` are the same for
//! agents. A client turned off is paused, still pointed at the proxy,
//! which passes its requests through (`--put-back` takes it off). A new
//! olo-pool installed takes over at once in a copy of its own, while this
//! one finishes what it has under way (`ui::hand_over`,
//! `olo_token_pool::drain`), and the pid olo-session follows stays as
//! the keeper (`keeper`).

mod clients_window;
mod ui;

use std::{
    io::{Read, Write},
    time::Duration,
};

use olo_kit::instance;
use olo_token_pool::{
    clients, codex_daemon,
    gateway::{self, Shared},
    model::{Provider, now},
    running,
};
use serde_json::Value;

const APP_ID: &str = "olo.pool";
const PROVIDERS: [Provider; 2] = [Provider::Claude, Provider::Codex];

fn client_index(p: Provider) -> usize {
    PROVIDERS.iter().position(|&q| q == p).unwrap_or(0)
}

/// "14:05", or "Tue 14:05" beyond today.
fn clock(t: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_opt(t, 0).single() else { return String::new() };
    if at.date_naive() == Local::now().date_naive() { at.format("%H:%M").to_string() } else { at.format("%a %H:%M").to_string() }
}

/// "2h 10m", "3d 4h", "45s".
fn span(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

/// "340k", "1.2M".
fn tokens(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{}k", n / 1000),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn plan_name(plan: &str) -> String {
    let mut c = plan.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
}

/// "850ms", "3.2s", "2m 5s".
fn duration(ms: u64) -> String {
    match ms {
        0..1000 => format!("{ms}ms"),
        1000..60_000 => format!("{:.1}s", ms as f64 / 1000.0),
        _ => format!("{}m {}s", ms / 60_000, ms % 60_000 / 1000),
    }
}

// ---- the command line -------------------------------------------------

const USAGE: &str = "usage: olo-pool [--service | --status [--json] | --clients [--json] | --restart-codex-service | --put-back claude|codex]
  (none)            open Token Pool (starting the gateway if it isn't running)
  --service         run the gateway in the background (the session does)
  --status [--json] the pool's accounts, quota and state (no sign-ins), and
                    each minute's requests by client over the last day
  --clients [--json]
                    the Claude Code and Codex running, where each one's
                    connections go, and Codex's service (its daemon)
  --restart-codex-service
                    restart Codex's service onto its settings now (a turn
                    going on ends; Codex's windows reconnect)
  --put-back claude|codex
                    take the client off the pool's proxy altogether, its
                    settings as before the pool (turned off in Token Pool,
                    it only passes through, needing no restart)";

/// The running gateway's status, over loopback.
fn fetch_status(port: u16) -> Result<Value, String> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|_| "Token Pool isn’t running.".to_owned())?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    write!(s, "GET /pool/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    serde_json::from_str(body).map_err(|_| "Token Pool answered in a way this version doesn’t understand.".to_owned())
}

fn status(port: u16, as_json: bool) -> std::process::ExitCode {
    let v = match fetch_status(port) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("olo-pool: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(e) = v["error"].as_str() {
        println!("! {e}");
    }
    let t = now();
    let accounts = v["accounts"].as_array().cloned().unwrap_or_default();
    if accounts.is_empty() {
        println!("No accounts in the pool.");
    }
    for a in accounts {
        let state = if let Some(why) = a["signed_out"].as_str() {
            format!("signed out ({why})")
        } else if a["enabled"] == false {
            "paused".into()
        } else if let Some(u) = a["blocked_until"].as_i64() {
            format!("out of quota until {}", clock(u))
        } else {
            "ready".into()
        };
        let credits: u64 = a["reset_credits"].as_array().into_iter().flatten().filter_map(|c| c["remaining"].as_u64()).sum();
        let state = if credits > 0 { format!("{state}, {credits} reset credit{}", if credits == 1 { "" } else { "s" }) } else { state };
        println!("{} [{}] {}", a["name"].as_str().unwrap_or(""), a["provider"].as_str().unwrap_or(""), state);
        for w in a["windows"].as_array().into_iter().flatten() {
            let reset = w["resets_at"].as_i64().filter(|&r| r > t).map(|r| format!(", resets {} ({})", clock(r), span(r - t))).unwrap_or_default();
            println!("  {}: {:.0}% used{reset}", w["label"].as_str().unwrap_or(""), w["used"].as_f64().unwrap_or(0.0));
        }
        let d = &a["today"];
        println!(
            "  today: {} request{}, {} in, {} out",
            d["requests"].as_u64().unwrap_or(0),
            if d["requests"] == 1 { "" } else { "s" },
            tokens(d["input_tokens"].as_u64().unwrap_or(0)),
            tokens(d["output_tokens"].as_u64().unwrap_or(0))
        );
    }
    std::process::ExitCode::SUCCESS
}

/// What's running of the clients', as the Clients window shows it.
fn clients_now(as_json: bool) -> std::process::ExitCode {
    let port = olo_token_pool::intercept::port();
    let r = running::scan(port);
    if as_json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
        return std::process::ExitCode::SUCCESS;
    }
    let t = now();
    match &r.daemon {
        Some(d) => {
            let version = d.version.as_deref().unwrap_or("?");
            let state = if d.stale { "started before the pool’s settings last changed".to_owned() } else { d.connections.describe(Provider::Codex) };
            println!("Codex’s service: Codex {version}, pid {}, since {} ({}): {state}", d.pid, clock(d.started), span(t - d.started));
            match &d.threads {
                Ok(threads) => {
                    for th in threads {
                        let state = match (th.working, th.waiting) {
                            (true, true) => "waiting for you",
                            (true, false) => "working",
                            _ => "idle",
                        };
                        println!("  {} {state}{}: {}", th.id, th.cwd.as_deref().map(|c| format!(" in {}", c.display())).unwrap_or_default(), th.title);
                    }
                }
                Err(e) => println!("  its conversations: it didn’t say ({e})"),
            }
        }
        None => println!("Codex’s service: not running"),
    }
    for (prov, list) in [(Provider::Claude, &r.claude), (Provider::Codex, &r.codex)] {
        println!("{} ({}):", prov.client(), list.len());
        for p in list {
            let place = p.cwd.as_deref().map(|c| c.display().to_string()).unwrap_or_default();
            println!("  pid {} in {place}, since {} ({}): {}", p.pid, clock(p.started), span(t - p.started), p.connections.describe(prov));
        }
    }
    std::process::ExitCode::SUCCESS
}

/// What stays of the olo-pool olo-session started, once an upgrade has
/// handed over to a copy of the new version (`ui::hand_over`), or once it
/// found another copy serving the port as it started (`main`): its pid,
/// which olo-session follows. It follows the copies that serve: its
/// children running olo-pool (as a subreaper, a later copy is given to it
/// as the one before goes) and whichever olo-pool holds the gateway's
/// listening socket (`drain::serving`: a copy that isn't its own, one the
/// session's copy before it left behind), each on a pidfd. It passes
/// olo-session's signals on to them and ends once none is left, with how
/// the last of its own ended (so olo-session starts it again as before if
/// it crashes). An upgrade makes it the new version's keeper (in place:
/// olo-session never sees an old program). Nothing else: about 8 MB
/// resident, asleep in poll.
fn keeper() -> std::process::ExitCode {
    use std::os::unix::process::CommandExt as _;
    // (Its pid waited for the copy before it handed over: not any more.)
    olo_kit::restore::mark_waiting(false);
    let port = gateway::port();
    // SAFETY: prctl, sigprocmask, signalfd, pidfd_open, poll and read with
    // valid arguments; waitpid on our own children; kill on the copies
    // we follow, which are ours (the same user's olo-pool).
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGURG] {
            libc::sigaddset(&mut set, s);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        let signals = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        // Our children running olo-pool (others given to us, a client's
        // daemon, are only reaped).
        let children = || -> Vec<i32> {
            let me = std::process::id();
            let list = std::fs::read_to_string(format!("/proc/{me}/task/{me}/children")).unwrap_or_default();
            list.split_whitespace().filter_map(|p| p.parse::<i32>().ok()).filter(|&p| olo_system::listening::program(std::path::Path::new("/"), p).as_deref() == Some("olo-pool")).collect()
        };
        let mut last = 0;
        loop {
            // Each child that ended, reaped; the last one's way of ending kept.
            loop {
                let mut status = 0;
                let pid = libc::waitpid(-1, &mut status, libc::WNOHANG);
                if pid <= 0 {
                    break;
                }
                last = status;
            }
            let mut follow = children();
            for p in olo_token_pool::drain::serving(port) {
                if !follow.contains(&p) {
                    follow.push(p);
                }
            }
            if follow.is_empty() {
                break;
            }
            // Asleep till a signal comes or one of them ends.
            let pidfds: Vec<i32> = follow.iter().map(|&p| libc::syscall(libc::SYS_pidfd_open, p, 0) as i32).filter(|&fd| fd >= 0).collect();
            let mut polled: Vec<libc::pollfd> = std::iter::once(signals).chain(pidfds.iter().copied()).filter(|&fd| fd >= 0).map(|fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
            while libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, -1) < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {}
            for fd in pidfds {
                libc::close(fd);
            }
            loop {
                let mut info: libc::signalfd_siginfo = std::mem::zeroed();
                let size = std::mem::size_of::<libc::signalfd_siginfo>();
                if signals < 0 || libc::read(signals, (&raw mut info).cast(), size) != size as isize {
                    break;
                }
                match info.ssi_signo as i32 {
                    libc::SIGURG => {
                        // A new version installed: its keeper, in place (the
                        // mask, and what's pending, carry across).
                        if let Ok(exe) = std::env::current_exe() {
                            let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
                            let e = std::process::Command::new(exe).env(ui::KEEPER_ENV, "1").exec();
                            eprintln!("olo-pool: the keeper couldn’t become the new version: {e}");
                        }
                    }
                    s @ (libc::SIGTERM | libc::SIGINT | libc::SIGHUP) => {
                        for &p in &follow {
                            libc::kill(p, s);
                        }
                    }
                    _ => {}
                }
            }
        }
        if libc::WIFEXITED(last) {
            std::process::ExitCode::from(libc::WEXITSTATUS(last) as u8)
        } else if libc::WIFSIGNALED(last) {
            std::process::ExitCode::from((128 + libc::WTERMSIG(last)) as u8)
        } else {
            std::process::ExitCode::SUCCESS
        }
    }
}

/// Become the keeper (`keeper`), as a program of its own (its environment
/// says so, for Olo Doctor to know it by); returns only if it couldn't.
fn become_keeper() -> std::io::Error {
    use std::os::unix::process::CommandExt as _;
    let exe = match std::env::current_exe() {
        Ok(e) => std::path::PathBuf::from(e.to_string_lossy().trim_end_matches(" (deleted)")),
        Err(e) => return e,
    };
    std::process::Command::new(exe).env(ui::KEEPER_ENV, "1").env_remove(ui::KEPT_ENV).env_remove(olo_kit::restore::ENV).exec()
}

/// Whether the version before this one (re-executing in place) had its
/// window open; its file read and removed.
fn window_was_open() -> bool {
    let Some(path) = std::env::var_os(olo_kit::restore::ENV) else { return false };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    serde_json::from_str::<Value>(&text).is_ok_and(|v| v["open"] == true)
}

/// Asks the copy serving the gateway on `port` to open its window
/// (`/pool/open`); true if it will.
fn ask_to_open(port: u16) -> bool {
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else { return false };
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    if write!(s, "POST /pool/open HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").is_err() {
        return false;
    }
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    out.starts_with(b"HTTP/1.1 204")
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    if std::env::var_os(ui::KEEPER_ENV).is_some() {
        return keeper();
    }
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port = gateway::port();
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("-h") || has("--help") {
        println!("{USAGE}");
        return std::process::ExitCode::SUCCESS;
    }
    if has("--status") {
        return status(port, has("--json"));
    }
    if has("--clients") {
        return clients_now(has("--json"));
    }
    if let Some(i) = args.iter().position(|a| a == "--put-back") {
        let Some(p) = args.get(i + 1).and_then(|a| Provider::parse(a)) else {
            eprintln!("olo-pool: --put-back takes claude or codex");
            return std::process::ExitCode::FAILURE;
        };
        return match clients::put_back(p) {
            Ok(()) => {
                println!("{} is off the pool’s proxy (from its next start).", p.client());
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-pool: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if has("--restart-codex-service") {
        return match codex_daemon::restart_now() {
            Ok(pid) => {
                println!("Codex’s service restarted (pid {pid}).");
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-pool: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    let service = has("--service");
    // One gateway per computer: a test or nested session runs its own
    // only on a port of its own.
    if service && !olo_xdg::session::real() && std::env::var_os("OLO_POOL_PORT").is_none() {
        eprintln!("olo-pool: --service is for the real session (set OLO_POOL_PORT to try it)");
        return std::process::ExitCode::FAILURE;
    }
    // Handed the listening sockets by the copy before: this one is the
    // pool (`drain::listen`).
    let handed = std::env::var_os(olo_token_pool::drain::LISTENERS_ENV).is_some();
    // The session's copy, started while another serves the port already
    // (one that outlived the copy the session started before: Olo Doctor
    // quit that one while it finished handing over, Ben, 2026-11-07): it
    // keeps that one, rather than running beside it with no gateway (whose
    // window showed nothing, and took the launches), or ending (the session
    // started it again and again, and gave up).
    if service && !handed {
        let serving = olo_token_pool::drain::serving(port);
        if !serving.is_empty() {
            eprintln!("olo-pool: Token Pool (process {}) serves port {port} already; keeping it", serving.iter().map(i32::to_string).collect::<Vec<_>>().join(", "));
            if window_was_open() {
                ask_to_open(port);
            }
            let e = become_keeper();
            eprintln!("olo-pool: couldn’t keep it ({e})");
            return std::process::ExitCode::FAILURE;
        }
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    // Opened while the copy serving takes no launches (it is starting, as
    // an upgrade hands over to it): it is asked over the gateway, not
    // given a copy beside it with no gateway. (One from before
    // `/pool/open`: its launches, once it takes them.)
    if !service && !handed && std::env::var_os(olo_kit::restore::ENV).is_none() && !olo_token_pool::drain::serving(port).is_empty() {
        if ask_to_open(port) {
            return std::process::ExitCode::SUCCESS;
        }
        for _ in 0..25 {
            std::thread::sleep(Duration::from_millis(200));
            if instance::forward(APP_ID, &args) {
                return std::process::ExitCode::SUCCESS;
            }
        }
    }
    olo_kit::run(APP_ID, move |app| {
        olo_kit::stay_resident();
        let proxy = olo_kit::proxy();
        instance::listen(APP_ID, proxy.clone());
        let to_ui = proxy.clone();
        let shared = Shared::launch(port, move || to_ui.send(ui::Msg::Changed));
        let to_ui = proxy.clone();
        shared.on_open(move || to_ui.send(ui::Msg::Open));
        // Clients pointed at the pool the old way (an API endpoint of its
        // own) go through its proxy instead, as on their own subscription.
        let proxy_port = olo_token_pool::intercept::port();
        std::thread::spawn(move || clients::migrate(proxy_port));
        // Codex's daemon, which outlives its windows, kept on the settings.
        codex_daemon::keep_following(Duration::from_secs(10));
        // (The app's state lives in the hub the app's looper keeps.)
        ui::start(app, ui::Hub::new(shared, proxy_port, proxy), service);
    });
    std::process::ExitCode::SUCCESS
}
