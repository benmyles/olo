//! olo-phone: the phone (an iPhone's messages and one-time codes, through
//! BlueFerry's backend). `--service` is the session's service (see
//! service.rs); the rest is the command line, for people and agents
//! alike: what the service knows, the conversations, sending, and Olo's
//! settings for it.

mod codes;
mod service;

/// A line on standard output; a reader gone (`| head`) isn't an error.
macro_rules! out {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($t)*);
    }};
}

use std::{process::ExitCode, time::Duration};

use olo_protocol::phone::{self, AgentCodes, Backend, Notify, PhoneSettings, PhoneState, Request};
use olo_system::blueferry::{self, Client};

const USAGE: &str = "usage: olo-phone [COMMAND]
  --status [--json]           the phone: connected, its battery, how many
                              conversations and unread messages
  --conversations [--json]    the conversations, the newest first
  --read KEY [--json]         one conversation's messages
  --send TO TEXT…             a new message to a number or an address
  --reply KEY TEXT…           an answer in a conversation
  --mark-read KEY
  --start                     start the backend (Olo's service does, and
                              keeps it running, once a phone is paired)
  --reconnect                 try the phone again now
  --code                      the latest one-time code, while it's good
  --copy-code                 the same, put on the clipboard
  --find-code                 what code a message has (read from standard
                              input; nothing is sent or kept)
  --settings [--json]         Olo's settings for the phone
  --set SETTING VALUE         notify: messages|everything|nothing
                              show-text, codes, codes-in-browser,
                              copy-codes: on|off
                              agent-screens: give|ask|never
  --service                   run the service (the session starts it)";

fn main() -> ExitCode {
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|a| a == "--service") {
        upgrade_on_signal();
        service::run();
    }
    let json = args.iter().any(|a| a == "--json");
    args.retain(|a| a != "--json");
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    let rest = |i: usize| args.get(i..).map(|a| a.join(" ")).unwrap_or_default();
    match arg(0).as_str() {
        "" | "--status" => status(json),
        "--conversations" => backend(|c| {
            let threads = c.threads(200)?;
            if json {
                let list: Vec<_> = threads.iter().map(thread_json).collect();
                out!("{}", serde_json::to_string_pretty(&list).unwrap_or_default());
                return Ok(());
            }
            for t in &threads {
                let last = t.messages.last();
                let unread = match t.unread() {
                    0 => String::new(),
                    n => format!(" ({n} unread)"),
                };
                out!("{}{unread}  [{}]", t.name, t.key);
                if let Some(m) = last {
                    out!("    {}{}", if m.outgoing { "you: " } else { "" }, one_line(&m.body, 100));
                }
            }
            Ok(())
        }),
        "--read" if !arg(1).is_empty() => backend(|c| {
            let threads = c.threads(200)?;
            let Some(t) = find(&threads, &arg(1)) else { return Err(not_found(&arg(1))) };
            if json {
                out!("{}", serde_json::to_string_pretty(&thread_json(t)).unwrap_or_default());
                return Ok(());
            }
            out!("{}{}", t.name, if t.is_group { format!(" ({})", t.recipients.join(", ")) } else { String::new() });
            for m in &t.messages {
                let who = if m.outgoing {
                    "you"
                } else if t.is_group && !m.sender.is_empty() {
                    m.sender.as_str()
                } else {
                    t.name.as_str()
                };
                out!("{}  {who}: {}", m.at, m.body);
            }
            Ok(())
        }),
        "--send" if !arg(2).is_empty() => backend(|c| c.send(&arg(1), &rest(2))),
        "--reply" if !arg(2).is_empty() => backend(|c| {
            let threads = c.threads(200)?;
            let Some(t) = find(&threads, &arg(1)) else { return Err(not_found(&arg(1))) };
            c.reply(t, &rest(2))
        }),
        "--mark-read" if !arg(1).is_empty() => backend(|c| {
            let threads = c.threads(200)?;
            let Some(t) = find(&threads, &arg(1)) else { return Err(not_found(&arg(1))) };
            c.mark_read(&t.key).map(|_| ())
        }),
        "--start" => {
            if !blueferry::installed() {
                return fail("the phone's backend (BlueFerry) isn't installed: Olo Doctor offers it");
            }
            if !blueferry::configured() {
                return fail("BlueFerry starts once an iPhone is paired: Preferences ▸ Phone pairs it");
            }
            let _ = olo_system::env::reload_session_bus();
            match Client::connect().and_then(|c| c.status()) {
                Ok(s) => {
                    out!("BlueFerry {} is running.", s.release);
                    ExitCode::SUCCESS
                }
                Err(e) => fail(&blueferry::said(&e)),
            }
        }
        "--reconnect" => backend(|c| c.reconnect().map(|how| out!("{how}"))),
        "--code" => match phone::ask(&Request::Code, Duration::from_secs(5)) {
            Ok(r) if r.ok => {
                out!("{}", r.code.unwrap_or_default());
                ExitCode::SUCCESS
            }
            Ok(r) => fail(&r.error.unwrap_or_default()),
            Err(e) => fail(&format!("the phone service isn't running ({e})")),
        },
        "--copy-code" => match phone::ask(&Request::Code, Duration::from_secs(5)) {
            Ok(r) if r.ok => {
                // (Held until something else is copied.)
                use zeroize::Zeroize as _;
                let mut code = r.code.unwrap_or_default();
                let held = olo_kit_base::clipboard::hold(olo_kit_base::clipboard::text(&code));
                code.zeroize();
                match held {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => fail(&format!("can't copy: {e}")),
                }
            }
            Ok(r) => fail(&r.error.unwrap_or_default()),
            Err(e) => fail(&format!("the phone service isn't running ({e})")),
        },
        "--find-code" => {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text);
            match codes::find(&text) {
                Some(c) if json => out!("{}", serde_json::json!({"code": c.code, "site": c.domain, "from": c.service})),
                Some(c) => {
                    out!("{}", c.code);
                    if let Some(s) = c.service {
                        out!("from: {s}");
                    }
                    if let Some(d) = c.domain {
                        out!("for the site: {d}");
                    }
                }
                None if json => out!("null"),
                None => {
                    out!("no code in it");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        "--settings" => {
            let s = PhoneSettings::load();
            if json {
                out!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
            } else {
                let on = |b: bool| if b { "on" } else { "off" };
                out!("notify: {}", s.notify.label().to_lowercase());
                out!("show-text: {}", on(s.show_text));
                out!("codes: {}", on(s.codes));
                out!("codes-in-browser: {}", on(s.codes_in_browser));
                out!("agent-screens: {}", s.agent_screens.word());
                out!("copy-codes: {}", on(s.copy_codes));
            }
            ExitCode::SUCCESS
        }
        "--set" => {
            let mut s = PhoneSettings::load();
            let on = on_off(&arg(2));
            let set = match (arg(1).as_str(), arg(2).as_str()) {
                ("notify", "messages") => Some(s.notify = Notify::Messages),
                ("notify", "everything") => Some(s.notify = Notify::Everything),
                ("notify", "nothing") => Some(s.notify = Notify::Nothing),
                ("show-text", _) => on.map(|v| s.show_text = v),
                ("codes", _) => on.map(|v| s.codes = v),
                ("codes-in-browser", _) => on.map(|v| s.codes_in_browser = v),
                ("copy-codes", _) => on.map(|v| s.copy_codes = v),
                ("agent-screens", w) => AgentCodes::from_word(w).map(|v| s.agent_screens = v),
                _ => None,
            };
            match set.map(|()| s.save()) {
                Some(Ok(())) => ExitCode::SUCCESS,
                Some(Err(e)) => fail(&format!("can't write {}: {e}", PhoneSettings::path().display())),
                None => usage(),
            }
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn fail(why: &str) -> ExitCode {
    eprintln!("olo-phone: {why}");
    ExitCode::FAILURE
}

fn on_off(v: &str) -> Option<bool> {
    match v {
        "on" | "yes" | "true" | "1" => Some(true),
        "off" | "no" | "false" | "0" => Some(false),
        _ => None,
    }
}

fn not_found(key: &str) -> zbus_error::Error {
    zbus_error::Error::Failure(format!("no conversation “{key}” (--conversations lists them)"))
}

/// What the backend's errors are here: `olo_system`'s (zbus's).
mod zbus_error {
    pub use olo_system::Error;
}

/// A conversation by its key, else by its name (any case).
fn find<'a>(threads: &'a [blueferry::Thread], which: &str) -> Option<&'a blueferry::Thread> {
    threads.iter().find(|t| t.key == which).or_else(|| threads.iter().find(|t| t.name.eq_ignore_ascii_case(which)))
}

fn one_line(s: &str, most: usize) -> String {
    let line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > most { format!("{}…", line.chars().take(most).collect::<String>()) } else { line }
}

fn thread_json(t: &blueferry::Thread) -> serde_json::Value {
    serde_json::json!({
        "key": t.key, "name": t.name, "group": t.is_group, "recipients": t.recipients, "can_reply": t.reply_ready, "unread": t.unread(), "starred": t.starred,
        "messages": t.messages.iter().map(|m| serde_json::json!({"at": m.at, "from_me": m.outgoing, "read": m.read, "sender": m.sender, "text": m.body})).collect::<Vec<_>>(),
    })
}

/// Ask the backend, saying plainly why not if it can't be.
fn backend(ask: impl FnOnce(&Client) -> olo_system::Result<()>) -> ExitCode {
    let client = match Client::connect() {
        Ok(c) => c,
        Err(e) => return fail(&format!("no session bus: {e}")),
    };
    if !client.running() {
        return fail(match (blueferry::installed(), blueferry::configured()) {
            (false, _) => "the phone's backend (BlueFerry) isn't installed: Olo Doctor offers it",
            (true, false) => "no iPhone is paired yet: Preferences ▸ Phone pairs it",
            (true, true) => "the phone's backend isn't running (olo-phone --start says why)",
        });
    }
    match ask(&client) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&blueferry::said(&e)),
    }
}

fn status(json: bool) -> ExitCode {
    let state = match phone::ask(&Request::Status, Duration::from_secs(5)) {
        Ok(r) => r.state.unwrap_or_default(),
        // Not running: what it last published, if anything.
        Err(_) => PhoneState::load(),
    };
    if json {
        out!("{}", serde_json::to_string_pretty(&state).unwrap_or_default());
        return ExitCode::SUCCESS;
    }
    out!("{}", state.words());
    if !state.trouble.is_empty() {
        out!("  {}", state.trouble);
    }
    if state.backend == Backend::Running {
        out!("  BlueFerry {}", state.release);
        let up = |on: bool| if on { "yes" } else { "no" };
        out!("  messages: {}, contacts: {}, the phone's notifications: {}", up(state.messages), up(state.contacts), up(state.notifications));
        out!("  {} conversations, {} unread", state.conversations, state.unread);
        if state.storage != "ready" && !state.storage.is_empty() {
            out!("  its store of messages is {}", state.storage);
        }
    }
    ExitCode::SUCCESS
}

/// After an upgrade olo-session sends SIGURG to the programs it replaced:
/// the service starts again as the new version (the browsers' hosts
/// connect again; a code waiting is let go).
fn upgrade_on_signal() {
    static FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    extern "C" fn on_signal(_: libc::c_int) {
        let fd = FD.load(std::sync::atomic::Ordering::Relaxed);
        if fd >= 0 {
            // SAFETY: write(2) is async-signal-safe.
            unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
        }
    }
    let mut fds = [0; 2];
    // SAFETY: a pipe and a signal handler that only writes to it.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return;
        }
        FD.store(fds[1], std::sync::atomic::Ordering::Relaxed);
        libc::signal(libc::SIGURG, on_signal as *const () as libc::sighandler_t);
    }
    let read = fds[0];
    std::thread::Builder::new()
        .name("upgrade".into())
        .spawn(move || {
            loop {
                let mut b = [0u8; 1];
                // SAFETY: reading one byte into our buffer.
                if unsafe { libc::read(read, b.as_mut_ptr().cast(), 1) } != 1 {
                    return;
                }
                let exe = std::fs::read_link("/proc/self/exe").unwrap_or_default();
                let Some(path) = exe.to_str().and_then(|p| p.strip_suffix(" (deleted)")) else { continue };
                tracing::info!("phone: upgraded: starting again as the new version");
                let _ = std::fs::remove_file(PhoneState::socket());
                use std::os::unix::process::CommandExt;
                let e = std::process::Command::new(path).args(std::env::args_os().skip(1)).exec();
                tracing::warn!("phone: couldn't start the new version: {e}");
            }
        })
        .expect("thread");
}
