//! `olo-airplay`: AirPlay from the command line, for people and agents.
//! It prints and ends; it never opens a window.
//!
//!   olo-airplay list [--json] [--wait SECONDS]
//!       the receivers on the networks this computer is on (asked by
//!       mDNS, for SECONDS, 2 if not said): each one's name, what it is,
//!       what it takes, and whether it's paired with
//!   olo-airplay pair NAME [--code CODE] [--at HOST:PORT]
//!       pair with a receiver: it shows a code on its screen, which is
//!       typed here (or given with --code); once for each receiver
//!   olo-airplay check NAME [--at HOST:PORT]
//!       whether a receiver paired with still knows this computer
//!   olo-airplay paired
//!       the receivers paired with
//!   olo-airplay unpair NAME
//!       forget a pairing here (the receiver keeps its side until this
//!       computer is removed there)
//!
//! NAME is a receiver's name or the start of it, in any case; --at says
//! where it is instead of asking the network (one on another network).

use std::{
    io::{BufRead as _, Write as _},
    net::{SocketAddr, ToSocketAddrs as _},
    process::ExitCode,
    time::Duration,
};

use olo_airplay::{
    Kind, Receiver, kept,
    pairing::{Error, Http, Setup, verify},
};

const USAGE: &str = "usage: olo-airplay list [--json] [--wait SECONDS]
       olo-airplay pair NAME [--code CODE] [--at HOST:PORT]
       olo-airplay check NAME [--at HOST:PORT]
       olo-airplay paired
       olo-airplay unpair NAME
  list     the AirPlay receivers on the network: name, kind, what each takes
  pair     pair with one: the code it shows on its screen is typed here, once
  check    whether one paired with still knows this computer
  paired   the receivers paired with
  unpair   forget a pairing here";

/// How long a receiver is waited on to answer.
const WAIT: Duration = Duration::from_secs(8);

fn main() -> ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.first().map(String::as_str) {
        Some("list") => list(&args[1..]),
        Some("pair") => pair(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("paired") => {
            let all = kept::all();
            if all.is_empty() {
                println!("No receiver is paired with.");
            }
            for k in all {
                println!("{}  ({})", k.name, k.device_id);
            }
            Ok(())
        }
        Some("unpair") => match name_in(&args[1..]) {
            Some(which) => match kept::forget(&which) {
                Ok(0) => Err(format!("no pairing with {which}")),
                Ok(_) => {
                    println!("Forgotten here. {which} keeps its side until this computer is removed there.");
                    Ok(())
                }
                Err(e) => Err(format!("the pairings couldn't be written: {e}")),
            },
            None => Err(USAGE.into()),
        },
        _ => {
            println!("{USAGE}");
            return if args.is_empty() || args[0] == "--help" || args[0] == "-h" { ExitCode::SUCCESS } else { ExitCode::from(2) };
        }
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("olo-airplay: {why}");
            ExitCode::FAILURE
        }
    }
}

/// An option's value ("--code 1234").
fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// What isn't an option or its value: the receiver's name.
fn name_in(args: &[String]) -> Option<String> {
    let mut words = Vec::new();
    let mut skip = false;
    for a in args {
        if std::mem::take(&mut skip) {
            continue;
        }
        if a.starts_with("--") {
            skip = matches!(a.as_str(), "--code" | "--at" | "--wait");
        } else {
            words.push(a.as_str());
        }
    }
    (!words.is_empty()).then(|| words.join(" "))
}

fn list(args: &[String]) -> Result<(), String> {
    let json = args.iter().any(|a| a == "--json");
    let wait = option(args, "--wait").and_then(|s| s.parse::<f64>().ok()).filter(|s| *s > 0.0 && *s <= 30.0).unwrap_or(2.0);
    let found = olo_airplay::receivers(Duration::from_secs_f64(wait)).map_err(|e| format!("the network couldn't be asked: {e}"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&found).unwrap_or_else(|_| "[]".into()));
        return Ok(());
    }
    if found.is_empty() {
        println!("No AirPlay receiver answered.");
        return Ok(());
    }
    let width = found.iter().map(|r| r.name.chars().count()).max().unwrap_or(0);
    for r in &found {
        println!("{:width$}  {:<11} {}", r.name, kind(r), takes(r));
    }
    Ok(())
}

fn kind(r: &Receiver) -> &'static str {
    match r.kind {
        Kind::AppleTv => "Apple TV",
        Kind::Mac => "Mac",
        Kind::Speaker => "speaker",
        Kind::Television => "television",
    }
}

/// What a receiver takes, in words.
fn takes(r: &Receiver) -> String {
    let mut what: Vec<&str> = [(r.plays_video, "video"), (r.shows_screens, "screens"), (r.plays_sound, "sound")].into_iter().filter(|(yes, _)| *yes).map(|(_, w)| w).collect();
    if what.is_empty() {
        what.push("nothing it says");
    }
    let pairing = match (kept::of(&r.device_id, &r.name).is_some(), r.asks_to_pair) {
        (true, _) => "; paired",
        (false, true) => "; pairs first",
        (false, false) => "",
    };
    format!("{}{pairing}", what.join(", "))
}

/// The receiver a command is for: where it is, its id and its name. By
/// --at, as given (its name the one typed); else the one on the network
/// whose name is `name` or alone begins with it.
fn find(name: &str, args: &[String]) -> Result<(SocketAddr, String, String), String> {
    if let Some(at) = option(args, "--at") {
        let addr = at.to_socket_addrs().ok().and_then(|mut a| a.next()).ok_or_else(|| format!("{at} isn't an address (HOST:PORT)"))?;
        let id = kept::all().into_iter().find(|k| k.name.eq_ignore_ascii_case(name)).map(|k| k.device_id).unwrap_or_default();
        return Ok((addr, id, name.to_owned()));
    }
    let found = olo_airplay::receivers(Duration::from_secs(2)).map_err(|e| format!("the network couldn't be asked: {e}"))?;
    let wanted = name.to_lowercase();
    let exact: Vec<&Receiver> = found.iter().filter(|r| r.name.to_lowercase() == wanted).collect();
    let begins: Vec<&Receiver> = found.iter().filter(|r| r.name.to_lowercase().starts_with(&wanted)).collect();
    let r = match (exact.as_slice(), begins.as_slice()) {
        ([one], _) | ([], [one]) => *one,
        ([], []) => return Err(format!("no receiver called {name} answered (olo-airplay list says who did)")),
        _ => return Err(format!("more than one receiver is called {name}: say more of its name")),
    };
    let addr = r.addrs.first().ok_or_else(|| format!("{} didn't say where it is", r.name))?;
    Ok((SocketAddr::new(*addr, r.port), r.device_id.clone(), r.name.clone()))
}

fn said(e: Error) -> String {
    e.to_string()
}

fn pair(args: &[String]) -> Result<(), String> {
    let name = name_in(args).ok_or(USAGE)?;
    let (at, device_id, name) = find(&name, args)?;
    let mut http = Http::connect(at, WAIT).map_err(said)?;
    let setup = Setup::begin(&mut http).map_err(said)?;
    let code = match option(args, "--code") {
        Some(code) => code.to_owned(),
        None => {
            print!("{name} is showing a code on its screen. The code: ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
            line.trim().to_owned()
        }
    };
    if code.is_empty() {
        return Err("no code given: nothing paired".into());
    }
    let paired = setup.finish(&mut http, &code, &our_name()).map_err(said)?;
    // (A receiver found by --at announced no id: the one it gave in pairing.)
    let device_id = if device_id.is_empty() { String::from_utf8_lossy(&paired.credentials.receiver_id).into_owned() } else { device_id };
    kept::keep(kept::Kept { device_id, name: name.clone(), credentials: paired.credentials.to_line() }).map_err(|e| format!("paired, but the pairing couldn't be kept: {e}"))?;
    println!("Paired with {name}.");
    if !paired.signed {
        println!("(It didn't sign for its key as HomeKit's pairing says it does. The code was proved both ways, so the pairing stands; worth telling Olo's makers which receiver this is.)");
    }
    Ok(())
}

fn check(args: &[String]) -> Result<(), String> {
    let name = name_in(args).ok_or(USAGE)?;
    let (at, device_id, name) = find(&name, args)?;
    let kept = kept::of(&device_id, &name).ok_or_else(|| format!("{name} isn't paired with (olo-airplay pair)"))?;
    let credentials = kept.credentials().ok_or("its pairing here can't be read: pair with it again")?;
    let mut http = Http::connect(at, WAIT).map_err(said)?;
    verify(&mut http, &credentials).map_err(said)?;
    println!("{name} knows this computer.");
    Ok(())
}

/// What a receiver lists this computer as.
fn our_name() -> String {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_owned()).unwrap_or_default();
    if host.is_empty() { "Olo".into() } else { format!("Olo on {host}") }
}
