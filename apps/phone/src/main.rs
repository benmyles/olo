//! olo-phone: the phone (an iPhone's or an Android phone's messages,
//! contacts and one-time codes, spoken to by Olo itself over Bluetooth).
//! `--service` is the session's service (see service.rs); the rest is the
//! command line, for people and agents alike, which asks the service on
//! its socket: how the phone stands, the conversations, sending, and
//! Olo's settings for it.

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

use olo_protocol::phone::{self, AgentCodes, Conversation, Notify, PhoneSettings, PhoneState, Reply, Request};

const USAGE: &str = "usage: olo-phone [COMMAND]
  --status [--json]           the phone: paired, connected, its battery,
                              how many conversations and unread messages
  --conversations [--json]    the conversations, the latest first
  --read WHO [--json]         one conversation's messages (WHO: its key,
                              its name, or the number)
  --send TO TEXT…             a message to a number, an address, or a
                              conversation (its key or its name)
  --mark-read WHO
  --contacts [QUERY] [--json] the phone's contacts
  --reconnect                 try the phone again now
  --code                      the latest one-time code, while it's good
  --copy-code                 the same, put on the clipboard
  --find-code                 what code a message has (read from standard
                              input; nothing is sent or kept)
  --settings [--json]         Olo's settings for the phone
  --set SETTING VALUE         notify: messages|nothing
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
        "--conversations" => conversations(|all| {
            if json {
                out!("{}", serde_json::to_string_pretty(&all).unwrap_or_default());
                return Ok(());
            }
            for t in &all {
                let unread = match t.unread() {
                    0 => String::new(),
                    n => format!(" ({n} unread)"),
                };
                out!("{}{unread}  [{}]", t.name, t.key);
                if let Some(m) = t.messages.last() {
                    out!("    {}{}", if m.outgoing { "you: " } else { "" }, one_line(&m.text, 100));
                }
            }
            Ok(())
        }),
        "--read" if !arg(1).is_empty() => conversations(|all| {
            let Some(t) = find(&all, &arg(1)) else { return Err(format!("no conversation “{}” (--conversations lists them)", arg(1))) };
            if json {
                out!("{}", serde_json::to_string_pretty(t).unwrap_or_default());
                return Ok(());
            }
            out!("{} ({})", t.name, t.address);
            for m in &t.messages {
                out!("{}  {}: {}", clock(m.at), if m.outgoing { "you" } else { t.name.as_str() }, m.text);
            }
            Ok(())
        }),
        "--send" | "--reply" if !arg(2).is_empty() => {
            // (A conversation's name stands for its key.)
            let to = match ask(&Request::Conversations) {
                Ok(r) => find(&r.conversations.unwrap_or_default(), &arg(1)).map_or_else(|| arg(1), |t| t.key.clone()),
                Err(_) => arg(1),
            };
            done(ask(&Request::Send { to, text: rest(2) }))
        }
        "--mark-read" if !arg(1).is_empty() => conversations(|all| {
            let Some(t) = find(&all, &arg(1)) else { return Err(format!("no conversation “{}”", arg(1))) };
            ask(&Request::MarkRead { key: t.key.clone() }).map(|_| ())
        }),
        "--contacts" => match ask(&Request::Contacts { query: rest(1) }) {
            Ok(r) => {
                let people = r.contacts.unwrap_or_default();
                if json {
                    out!("{}", serde_json::to_string_pretty(&people).unwrap_or_default());
                } else {
                    for p in &people {
                        out!("{}  {}", p.name, p.phones.iter().chain(&p.emails).cloned().collect::<Vec<_>>().join(", "));
                    }
                }
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e),
        },
        "--reconnect" => done(ask(&Request::Reconnect)),
        "--code" => match ask(&Request::Code) {
            Ok(r) => {
                out!("{}", r.code.unwrap_or_default());
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e),
        },
        "--copy-code" => match ask(&Request::Code) {
            Ok(r) => {
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
            Err(e) => fail(&e),
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

/// Ask the service; its answer, or why not, said plainly.
fn ask(request: &Request) -> Result<Reply, String> {
    match phone::ask(request, Duration::from_secs(50)) {
        Ok(r) if r.ok => Ok(r),
        Ok(r) => Err(r.error.unwrap_or_else(|| "the phone service said no".into())),
        Err(e) => Err(format!("the phone service isn't running ({e})")),
    }
}

fn done(answer: Result<Reply, String>) -> ExitCode {
    match answer {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

/// Do `with` the conversations.
fn conversations(with: impl FnOnce(Vec<Conversation>) -> Result<(), String>) -> ExitCode {
    match ask(&Request::Conversations).and_then(|r| with(r.conversations.unwrap_or_default())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

/// A conversation by its key, else by its name (any case), else by its
/// number however it's written.
fn find<'a>(all: &'a [Conversation], which: &str) -> Option<&'a Conversation> {
    let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
    all.iter()
        .find(|t| t.key == which)
        .or_else(|| all.iter().find(|t| t.name.eq_ignore_ascii_case(which)))
        .or_else(|| all.iter().find(|t| digits(which).len() >= 5 && digits(&t.address).ends_with(&digits(which))))
}

fn one_line(s: &str, most: usize) -> String {
    let line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > most { format!("{}…", line.chars().take(most).collect::<String>()) } else { line }
}

/// A time, as this computer's clock says it.
fn clock(at: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Local.timestamp_opt(at, 0).single().map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M").to_string())
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
    if state.paired() {
        out!("{}: {}", state.name, state.words());
    } else {
        out!("{}", state.words());
    }
    if !state.trouble.is_empty() {
        out!("  {}", state.trouble);
    }
    if state.link.connected() {
        out!("  {} conversations, {} unread{}", state.conversations, state.unread, if state.contacts { "" } else { "; its contacts haven't come" });
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
