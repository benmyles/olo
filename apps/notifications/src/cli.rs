//! The command line: asks the running service over
//! `com.olodesktop.Notifications1` (the session bus, or `--bus ADDRESS`).
//!
//! ```text
//! olo-notifications --list [--json]   what is on screen and in the list,
//!                                     and what waits in apps (their
//!                                     own counts: not notifications;
//!                                     `"waiting"` in the JSON)
//! olo-notifications --cleared [--json]   what has left it (the Cleared
//!                                     list), each with why and when
//!                                     (--archived is the same)
//! olo-notifications --history         open the Notifications window
//! olo-notifications --invoke ID [ACTION]
//! olo-notifications --clear ID…       clear it, as its × does: out of the
//!                                     list, into the Cleared one; several
//!                                     at once by their numbers in one Undo
//!                                     step (Deskbar's Clear All on a stack)
//!                                     (--dismiss ID… is the same)
//! olo-notifications --read ID|all     mark read (as a click does)
//! olo-notifications --unread ID       mark unread again
//! olo-notifications --pin ID | --unpin ID   keep it put, or not
//! olo-notifications --snooze ID MINUTES     away for a while, then back
//! olo-notifications --clear           clear them all (Clear All)
//! olo-notifications --dnd on|off|MINUTES
//! ```

use std::process::ExitCode;

use crate::{server, store::Entry};

pub fn usage() -> ExitCode {
    eprintln!(
        "usage: olo-notifications [--bus ADDRESS]   (serve; only in a real session)\n       \
         olo-notifications [--bus ADDRESS] --list [--json] | --cleared [--json] | --history | --invoke ID [ACTION] | --clear [ID…] | --read ID|all | --unread ID | --pin ID | --unpin ID | --snooze ID MINUTES | --dnd on|off|MINUTES"
    );
    ExitCode::from(2)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command<'a> {
    List { json: bool },
    Archived { json: bool },
    History,
    ClearHistory,
    Dismiss(u32),
    ClearMany(Vec<u32>),
    MarkAllRead,
    SetPinned(u32, bool),
    Snooze(u32, u32),
    SetRead(u32, bool),
    Invoke(u32, &'a str),
    DoNotDisturb(bool, u32),
}

/// Parse command-line arguments (after `--bus ADDRESS` has been stripped)
/// without talking to the service. Any malformed or overflowing argument
/// returns `None` so zero service calls are ever made.
fn parse_command(args: &[String]) -> Option<Command<'_>> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    let id = || arg(1).and_then(|s| s.parse::<u32>().ok());
    let ids = || args.get(1..).filter(|rest| !rest.is_empty()).and_then(|rest| rest.iter().map(|s| s.parse::<u32>().ok()).collect::<Option<Vec<u32>>>());
    match (arg(0), arg(1)) {
        (Some("--list"), json) if (json.is_none() || json == Some("--json")) && args.len() <= 2 => Some(Command::List { json: json.is_some() }),
        (Some("--cleared" | "--archived"), json) if (json.is_none() || json == Some("--json")) && args.len() <= 2 => Some(Command::Archived { json: json.is_some() }),
        (Some("--history"), None) => Some(Command::History),
        (Some("--clear"), None) => Some(Command::ClearHistory),
        (Some("--clear" | "--dismiss"), Some(_)) => {
            let parsed = ids()?;
            match parsed.as_slice() {
                [one] => Some(Command::Dismiss(*one)),
                _ => Some(Command::ClearMany(parsed)),
            }
        }
        (Some("--read"), Some("all")) if args.len() == 2 => Some(Command::MarkAllRead),
        (Some(flag @ ("--pin" | "--unpin")), Some(_)) if args.len() == 2 => Some(Command::SetPinned(id()?, flag == "--pin")),
        (Some("--snooze"), Some(_)) if args.len() == 3 => {
            let mins = arg(2)?.parse::<u32>().ok().filter(|m| *m > 0)?;
            Some(Command::Snooze(id()?, mins))
        }
        (Some(flag @ ("--read" | "--unread")), Some(_)) if args.len() == 2 => Some(Command::SetRead(id()?, flag == "--read")),
        (Some("--invoke"), Some(_)) if args.len() <= 3 => Some(Command::Invoke(id()?, arg(2).unwrap_or("default"))),
        (Some("--dnd"), Some(v)) if args.len() == 2 => {
            let (on, minutes) = match v {
                "on" => (true, 0),
                "off" => (false, 0),
                m => (true, m.parse::<u32>().ok().filter(|m| *m > 0)?),
            };
            Some(Command::DoNotDisturb(on, minutes))
        }
        _ => None,
    }
}

enum Call<'a> {
    NoArgs(&'static str),
    U32(&'static str, u32),
    U32Bool(&'static str, u32, bool),
    U32U32(&'static str, u32, u32),
    U32Str(&'static str, u32, &'a str),
    BoolU32(&'static str, bool, u32),
    ClearMany(Vec<(u32, u64)>, bool),
}

pub fn run(address: Option<&str>, args: &[String]) -> ExitCode {
    // Piped into `head`: stop quietly when the reader goes.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    let Some(cmd) = parse_command(args) else { return usage() };
    dispatch_command(cmd, |call| match call {
        Call::NoArgs(m) => server::call(address, m, &()),
        Call::U32(m, a) => server::call(address, m, &(a,)),
        Call::U32Bool(m, a, b) => server::call(address, m, &(a, b)),
        Call::U32U32(m, a, b) => server::call(address, m, &(a, b)),
        Call::U32Str(m, a, b) => server::call(address, m, &(a, b)),
        Call::BoolU32(m, a, b) => server::call(address, m, &(a, b)),
        Call::ClearMany(targets, unread_only) => server::call(address, "ClearMany", &(targets, unread_only)),
    })
}

/// Dispatch `args` over an existing D-Bus connection (used by service-door
/// tests as well as the real CLI).
#[cfg(test)]
pub(crate) fn run_on(conn: &zbus::blocking::Connection, destination: Option<&str>, args: &[String]) -> ExitCode {
    let Some(cmd) = parse_command(args) else { return usage() };
    dispatch_command(cmd, |call| match call {
        Call::NoArgs(m) => server::call_on(conn, destination, m, &()),
        Call::U32(m, a) => server::call_on(conn, destination, m, &(a,)),
        Call::U32Bool(m, a, b) => server::call_on(conn, destination, m, &(a, b)),
        Call::U32U32(m, a, b) => server::call_on(conn, destination, m, &(a, b)),
        Call::U32Str(m, a, b) => server::call_on(conn, destination, m, &(a, b)),
        Call::BoolU32(m, a, b) => server::call_on(conn, destination, m, &(a, b)),
        Call::ClearMany(targets, unread_only) => server::call_on(conn, destination, "ClearMany", &(targets, unread_only)),
    })
}

fn dispatch_command<'a>(cmd: Command<'a>, mut call: impl FnMut(Call<'a>) -> zbus::Result<zbus::Message>) -> ExitCode {
    let result: zbus::Result<ExitCode> = match cmd {
        Command::List { json } => list_with(&mut call, json).map(|()| ExitCode::SUCCESS),
        Command::Archived { json } => archived_with(&mut call, json).map(|()| ExitCode::SUCCESS),
        Command::History => call(Call::NoArgs("ShowHistory")).map(|_| ExitCode::SUCCESS),
        Command::ClearHistory => call(Call::NoArgs("ClearHistory")).map(|_| ExitCode::SUCCESS),
        Command::Dismiss(id) => call(Call::U32("Dismiss", id)).map(|_| ExitCode::SUCCESS),
        // One atomic batch call on the service (one Undo step; CLI clears read
        // history entries too, so `unread_only = false` and `instance = 0`).
        Command::ClearMany(ids) => {
            let targets: Vec<(u32, u64)> = ids.iter().map(|&id| (id, 0)).collect();
            call(Call::ClearMany(targets, false)).and_then(|reply| {
                let (cleared, _batch): (Vec<u32>, String) = reply.body().deserialize()?;
                let (all_ok, line) = clear_many_outcome(&ids, &cleared);
                if all_ok {
                    println!("{line}");
                    Ok(ExitCode::SUCCESS)
                } else {
                    eprintln!("olo-notifications: {line}");
                    Ok(ExitCode::FAILURE)
                }
            })
        }
        Command::MarkAllRead => call(Call::NoArgs("MarkAllRead")).map(|_| ExitCode::SUCCESS),
        Command::SetPinned(id, pinned) => call(Call::U32Bool("SetPinned", id, pinned)).map(|_| ExitCode::SUCCESS),
        Command::Snooze(id, mins) => call(Call::U32U32("Snooze", id, mins)).map(|_| ExitCode::SUCCESS),
        Command::SetRead(id, read) => call(Call::U32Bool("SetRead", id, read)).map(|_| ExitCode::SUCCESS),
        Command::Invoke(id, action) => call(Call::U32Str("Invoke", id, action)).map(|_| ExitCode::SUCCESS),
        Command::DoNotDisturb(on, minutes) => call(Call::BoolU32("SetDoNotDisturb", on, minutes)).map(|_| ExitCode::SUCCESS),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("olo-notifications: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Summarize what `ClearMany` cleared and what it skipped against the unique
/// `requested` IDs. Returns `(all_cleared, message)`: `true` only when every
/// requested unique ID was cleared.
fn clear_many_outcome(requested: &[u32], cleared: &[u32]) -> (bool, String) {
    let cleared_set: std::collections::HashSet<u32> = cleared.iter().copied().collect();
    let mut seen = std::collections::HashSet::new();
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for &id in requested {
        if !seen.insert(id) {
            continue;
        }
        if cleared_set.contains(&id) {
            applied.push(id);
        } else {
            skipped.push(id);
        }
    }
    let join = |ids: &[u32]| ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
    match (applied.is_empty(), skipped.is_empty()) {
        (false, true) => (true, format!("cleared {} ({})", applied.len(), join(&applied))),
        (true, _) => (false, format!("cleared 0; skipped {} ({})", skipped.len(), join(&skipped))),
        (false, false) => (false, format!("cleared {} ({}); skipped {} ({})", applied.len(), join(&applied), skipped.len(), join(&skipped))),
    }
}

/// What has left the list, newest first: when it came, who from, what
/// it said, and why and when it went.
fn archived_with<'a>(call: &mut impl FnMut(Call<'a>) -> zbus::Result<zbus::Message>, json: bool) -> zbus::Result<()> {
    let reply = call(Call::NoArgs("ListArchived"))?;
    let text: String = reply.body().deserialize()?;
    if json {
        println!("{text}");
        return Ok(());
    }
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let entries: Vec<Entry> = serde_json::from_value(v["archived"].clone()).unwrap_or_default();
    println!("{} cleared", entries.len());
    for e in entries.iter().rev() {
        let why = e.why.map_or_else(|| "Cleared".to_owned(), |w| w.said(&e.app));
        println!("{:>5}  {}  {}: {}  [{why} {}]", e.id, crate::when(e.time), e.app, e.summary, crate::when(e.archived));
        if !e.text.is_empty() {
            println!("       {}", e.text.replace('\n', "\n       "));
        }
    }
    Ok(())
}

fn list_with<'a>(call: &mut impl FnMut(Call<'a>) -> zbus::Result<zbus::Message>, json: bool) -> zbus::Result<()> {
    let reply = call(Call::NoArgs("List"))?;
    let text: String = reply.body().deserialize()?;
    if json {
        println!("{text}");
        return Ok(());
    }
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let entries: Vec<Entry> = serde_json::from_value(v["notifications"].clone()).unwrap_or_default();
    println!("{} unread{}", v["unread"].as_u64().unwrap_or(0), if v["quiet"].as_bool() == Some(true) { ", Do Not Disturb" } else { "" });
    // What apps say is waiting in them: theirs, apart from the list.
    let waiting: Vec<crate::waiting::Waiting> = serde_json::from_value(v["waiting"].clone()).unwrap_or_default();
    for w in &waiting {
        println!("{:>5}  waiting in apps: {}{}{}", w.id, w.said(), if w.urgent { " (urgent)" } else { "" }, if w.put_off() { "  [put off]" } else { "" });
    }
    for e in entries.iter().rev() {
        let strength = v["notifications"].as_array().and_then(|a| a.iter().find(|n| n["id"] == e.id)).and_then(|n| n["strength"].as_f64()).unwrap_or(1.0);
        let faded = format!("{}% faded", ((1.0 - strength) * 100.0).round());
        let snoozed = e.snoozed_until.map(|t| format!("snoozed until {}", crate::when(t))).unwrap_or_default();
        let state = [(e.shown, "shown"), (e.live, "open"), (e.unread, "unread"), (e.pinned, "pinned"), (e.snoozed(), snoozed.as_str()), (strength < 0.995, faded.as_str())]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, s)| *s)
            .collect::<Vec<_>>()
            .join(", ");
        let actions: Vec<String> = e.actions.iter().map(|(k, l)| if l.is_empty() || l == k { k.clone() } else { format!("{k} ({l})") }).collect();
        println!("{:>5}  {}  {}: {}{}", e.id, crate::when(e.time), e.app, e.summary, if state.is_empty() { String::new() } else { format!("  [{state}]") });
        if !e.text.is_empty() {
            println!("       {}", e.text.replace('\n', "\n       "));
        }
        if !actions.is_empty() {
            println!("       actions: {}", actions.join(", "));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn clear_and_dismiss_parsing() {
        // Bare --clear still means ClearHistory; bare --dismiss is invalid (zero calls).
        assert_eq!(parse_command(&args(&["--clear"])), Some(Command::ClearHistory));
        assert_eq!(parse_command(&args(&["--dismiss"])), None);
        // Single ID uses single-card Dismiss (works on waiting cards too).
        assert_eq!(parse_command(&args(&["--clear", "17"])), Some(Command::Dismiss(17)));
        assert_eq!(parse_command(&args(&["--dismiss", "17"])), Some(Command::Dismiss(17)));
        // Multiple IDs use one atomic ClearMany call on both aliases.
        assert_eq!(parse_command(&args(&["--clear", "17", "18", "19"])), Some(Command::ClearMany(vec![17, 18, 19])));
        assert_eq!(parse_command(&args(&["--dismiss", "17", "18"])), Some(Command::ClearMany(vec![17, 18])));
        // Any malformed, empty or overflowing trailing ID rejects the whole command.
        assert_eq!(parse_command(&args(&["--clear", "17", "abc"])), None);
        assert_eq!(parse_command(&args(&["--clear", "17", ""])), None);
        assert_eq!(parse_command(&args(&["--clear", "17", "4294967296"])), None);
        assert_eq!(parse_command(&args(&["--dismiss", "17", "4294967296"])), None);
        assert_eq!(parse_command(&args(&["--dismiss", "-1"])), None);
    }

    #[test]
    fn clear_many_outcome_reports_applied_and_skipped() {
        // Complete success (deduplicating repeated requested IDs).
        let (ok, msg) = clear_many_outcome(&[17, 18, 17], &[17, 18]);
        assert!(ok && msg == "cleared 2 (17, 18)", "{msg}");
        // Partial clear: reports both cleared and skipped, returns failure.
        let (ok, msg) = clear_many_outcome(&[17, 18, 19], &[17, 19]);
        assert!(!ok && msg == "cleared 2 (17, 19); skipped 1 (18)", "{msg}");
        // Nothing cleared (e.g. pinned + missing): reports 0 cleared and skipped, returns failure.
        let (ok, msg) = clear_many_outcome(&[17, 18], &[]);
        assert!(!ok && msg == "cleared 0; skipped 2 (17, 18)", "{msg}");
    }
}
