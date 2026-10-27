//! The command line: asks the running service over
//! `org.tabula.Notifications1` (the session bus, or `--bus ADDRESS`).
//!
//! ```text
//! tabula-notifications --list [--json]   what is on screen and in the history
//! tabula-notifications --history         open the History window
//! tabula-notifications --invoke ID [ACTION]
//! tabula-notifications --dismiss ID
//! tabula-notifications --clear           forget the history
//! tabula-notifications --dnd on|off|MINUTES
//! ```

use std::process::ExitCode;

use crate::{server, store::Entry};

pub fn usage() -> ExitCode {
    eprintln!(
        "usage: tabula-notifications [--bus ADDRESS]   (serve; only in a real session)\n       \
         tabula-notifications [--bus ADDRESS] --list [--json] | --history | --invoke ID [ACTION] | --dismiss ID | --clear | --dnd on|off|MINUTES"
    );
    ExitCode::from(2)
}

pub fn run(address: Option<&str>, args: &[String]) -> ExitCode {
    // Piped into `head`: stop quietly when the reader goes.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    let arg = |i: usize| args.get(i).map(String::as_str);
    let id = || arg(1).and_then(|s| s.parse::<u32>().ok());
    let result = match (arg(0), arg(1)) {
        (Some("--list"), json) if json.is_none() || json == Some("--json") => list(address, json.is_some()),
        (Some("--history"), None) => server::call(address, "ShowHistory", &()).map(|_| ()),
        (Some("--clear"), None) => server::call(address, "ClearHistory", &()).map(|_| ()),
        (Some("--dismiss"), Some(_)) if id().is_some() => server::call(address, "Dismiss", &(id().unwrap())).map(|_| ()),
        (Some("--invoke"), Some(_)) if id().is_some() => {
            server::call(address, "Invoke", &(id().unwrap(), arg(2).unwrap_or("default"))).map(|_| ())
        }
        (Some("--dnd"), Some(v)) => {
            let (on, minutes) = match v {
                "on" => (true, 0),
                "off" => (false, 0),
                m => match m.parse::<u32>() {
                    Ok(m) if m > 0 => (true, m),
                    _ => return usage(),
                },
            };
            server::call(address, "SetDoNotDisturb", &(on, minutes)).map(|_| ())
        }
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tabula-notifications: {e}");
            ExitCode::FAILURE
        }
    }
}

fn list(address: Option<&str>, json: bool) -> zbus::Result<()> {
    let reply = server::call(address, "List", &())?;
    let text: String = reply.body().deserialize()?;
    if json {
        println!("{text}");
        return Ok(());
    }
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let entries: Vec<Entry> = serde_json::from_value(v["notifications"].clone()).unwrap_or_default();
    println!("{} unread{}", v["unread"].as_u64().unwrap_or(0), if v["quiet"].as_bool() == Some(true) { ", Do Not Disturb" } else { "" });
    for e in entries.iter().rev() {
        let state = [(e.shown, "shown"), (e.live, "open"), (e.unread, "unread")].iter().filter(|(on, _)| *on).map(|(_, s)| *s).collect::<Vec<_>>().join(", ");
        let actions: Vec<&str> = e.actions.iter().map(|(k, _)| k.as_str()).collect();
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
