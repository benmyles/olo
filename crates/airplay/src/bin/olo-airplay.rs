//! `olo-airplay`: AirPlay from the command line, for people and agents.
//! It prints and ends; it never opens a window.
//!
//!   olo-airplay list [--json] [--wait SECONDS]
//!       the receivers on the networks this computer is on (asked by
//!       mDNS, for SECONDS, 2 if not said): each one's name, what it is,
//!       what it takes and whether it asks to be paired with first

use std::{process::ExitCode, time::Duration};

use olo_airplay::{Kind, Receiver};

const USAGE: &str = "usage: olo-airplay list [--json] [--wait SECONDS]\n  list   the AirPlay receivers on the network: name, kind, what each takes";

fn main() -> ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => list(&args[1..]),
        _ => {
            println!("{USAGE}");
            if args.is_empty() || args[0] == "--help" || args[0] == "-h" { ExitCode::SUCCESS } else { ExitCode::from(2) }
        }
    }
}

fn list(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let wait = args.iter().position(|a| a == "--wait").and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<f64>().ok()).filter(|s| *s > 0.0 && *s <= 30.0).unwrap_or(2.0);
    let found = match olo_airplay::receivers(Duration::from_secs_f64(wait)) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("olo-airplay: the network couldn't be asked: {e}");
            return ExitCode::FAILURE;
        }
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&found).unwrap_or_else(|_| "[]".into()));
        return ExitCode::SUCCESS;
    }
    if found.is_empty() {
        println!("No AirPlay receiver answered.");
        return ExitCode::SUCCESS;
    }
    let width = found.iter().map(|r| r.name.chars().count()).max().unwrap_or(0);
    for r in &found {
        println!("{:width$}  {:<11} {}", r.name, kind(r), takes(r));
    }
    ExitCode::SUCCESS
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
    format!("{}{}", what.join(", "), if r.asks_to_pair { "; pairs first" } else { "" })
}
