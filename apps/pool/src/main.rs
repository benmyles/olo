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
//! olo-pool installed waits for what is under way, holding new
//! connections for it (`olo_token_pool::drain`).

mod clients_window;
mod ui;

use std::{
    io::{Read, Write},
    time::Duration,
};

use olo_token_pool::{
    clients, codex_daemon,
    gateway::{self, Shared},
    model::{Provider, now},
    running,
};
use olo_ui::instance;
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

fn main() -> std::process::ExitCode {
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
    if service && std::env::var_os("OLO_REAL_SESSION").is_none() && std::env::var_os("OLO_POOL_PORT").is_none() {
        eprintln!("olo-pool: --service is for the real session (set OLO_POOL_PORT to try it)");
        return std::process::ExitCode::FAILURE;
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_ui::run(APP_ID, move |cx| {
        olo_ui::stay_resident(cx);
        let proxy = olo_ui::proxy(cx);
        instance::listen(APP_ID, proxy.clone());
        let to_ui = proxy.clone();
        let shared = Shared::launch(port, move || to_ui.send(ui::Msg::Changed));
        // Clients pointed at the pool the old way (an API endpoint of its
        // own) go through its proxy instead, as on their own subscription.
        let proxy_port = olo_token_pool::intercept::port();
        std::thread::spawn(move || clients::migrate(proxy_port));
        // Codex's daemon, which outlives its windows, kept on the settings.
        codex_daemon::keep_following(Duration::from_secs(10));
        // (The app's state lives in the handlers it gives olo-ui.)
        ui::start(cx, ui::PoolApp::new(shared, proxy_port, proxy), service);
    });
    std::process::ExitCode::SUCCESS
}
