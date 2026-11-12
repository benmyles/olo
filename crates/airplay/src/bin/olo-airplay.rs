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
//!   olo-airplay play NAME FILE|ADDRESS [--from TIME] [--json] [--trace] [--notify] [--at HOST:PORT]
//!   olo-airplay play --on NAME FILE|ADDRESS …
//!       play a film (or sound) on a receiver paired with: a file here,
//!       which it fetches from this computer while it plays, or
//!       something on the web (http, https). It says how the play goes
//!       and ends when the film does; Ctrl-C stops it there. --from
//!       begins at a place (seconds, or 1:02:03); --json says it as a
//!       line of JSON for each change; --trace says each step and what
//!       the receiver answered (for finding out why one won't play);
//!       --notify tells the person in a notification if it couldn't be
//!       played (what an app's "Play On" asks for); --on names the
//!       receiver apart from the film (a name of several words)
//!   olo-airplay playing [--json]
//!       the films playing: what, where, and how far along
//!   olo-airplay stop [NAME] | pause [NAME] | resume [NAME]
//!       stop, pause or go on with what plays on a receiver (on every
//!       one, with no name; --pid N: the one play of that process,
//!       as `playing --json` numbers them)
//!   olo-airplay paired
//!       the receivers paired with
//!   olo-airplay unpair NAME
//!       forget a pairing here (the receiver keeps its side until this
//!       computer is removed there)
//!
//! NAME is a receiver's name or the start of it, in any case, or its id
//! as `list --json` has it; --at says where it is instead of asking the
//! network (one on another network).

use std::{
    io::{BufRead as _, Write as _},
    net::{SocketAddr, ToSocketAddrs as _},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use olo_airplay::{
    Kind, Pairing, Receiver, kept, our_name,
    pairing::Error,
    play::{Now, Options, Play, State, Trace, What},
};
use olo_protocol::airplay::{self, Playing};

const USAGE: &str = "usage: olo-airplay list [--json] [--wait SECONDS]
       olo-airplay pair NAME [--code CODE] [--at HOST:PORT]
       olo-airplay check NAME [--at HOST:PORT]
       olo-airplay play NAME FILE|ADDRESS [--from TIME] [--json] [--trace] [--notify] [--at HOST:PORT]
       olo-airplay playing [--json]
       olo-airplay stop [NAME] | pause [NAME] | resume [NAME]
       olo-airplay paired
       olo-airplay unpair NAME
  list     the AirPlay receivers on the network: name, kind, what each takes
  pair     pair with one: the code it shows on its screen is typed here, once
  check    whether one paired with still knows this computer
  play     play a film on one: a file here or an address on the web; Ctrl-C stops it
  playing  the films playing; stop, pause and resume act on them
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
        Some("play") => play(&args[1..]),
        Some("playing") => playing(&args[1..]),
        Some("stop") => tell_plays(&args[1..], libc::SIGTERM),
        Some("pause") => tell_plays(&args[1..], libc::SIGUSR1),
        Some("resume") => tell_plays(&args[1..], libc::SIGUSR2),
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

/// What isn't an option or its value.
fn words_in(args: &[String]) -> Vec<&str> {
    let mut words = Vec::new();
    let mut skip = false;
    for a in args {
        if std::mem::take(&mut skip) {
            continue;
        }
        if a.starts_with("--") {
            skip = matches!(a.as_str(), "--code" | "--at" | "--wait" | "--from" | "--pid" | "--on");
        } else {
            words.push(a.as_str());
        }
    }
    words
}

/// The receiver's name: the words that aren't options.
fn name_in(args: &[String]) -> Option<String> {
    let words = words_in(args);
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
    // (By its id, which an app's menu names it by; else by its name.)
    let by_id: Vec<&Receiver> = found.iter().filter(|r| !r.device_id.is_empty() && r.device_id.to_lowercase() == wanted).collect();
    let exact: Vec<&Receiver> = found.iter().filter(|r| r.name.to_lowercase() == wanted).collect();
    let begins: Vec<&Receiver> = found.iter().filter(|r| r.name.to_lowercase().starts_with(&wanted)).collect();
    // (What it's called, for saying so: a receiver known by its id here.)
    let called = kept::all().into_iter().find(|k| k.device_id.to_lowercase() == wanted).map_or_else(|| name.to_owned(), |k| k.name);
    let r = match (by_id.as_slice(), exact.as_slice(), begins.as_slice()) {
        ([one], ..) | ([], [one], _) | ([], [], [one]) => *one,
        ([], [], []) => return Err(format!("{called} didn't answer: is it on, and on this network? (olo-airplay list says who is)")),
        _ => return Err(format!("more than one receiver is called {name}: say more of its name")),
    };
    let at = r.at().ok_or_else(|| format!("{} didn't say where it is", r.name))?;
    Ok((at, r.device_id.clone(), r.name.clone()))
}

fn said(e: Error) -> String {
    e.to_string()
}

fn pair(args: &[String]) -> Result<(), String> {
    let name = name_in(args).ok_or(USAGE)?;
    let (at, device_id, name) = find(&name, args)?;
    let pairing = Pairing::begin(at, &device_id, &name, WAIT).map_err(said)?;
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
    let signed = pairing.finish(&code)?;
    println!("Paired with {name}.");
    if !signed {
        println!("(It didn't sign for its key as HomeKit's pairing says it does. The code was proved both ways, so the pairing stands; worth telling Olo's makers which receiver this is.)");
    }
    Ok(())
}

fn check(args: &[String]) -> Result<(), String> {
    let name = name_in(args).ok_or(USAGE)?;
    let (at, device_id, name) = find(&name, args)?;
    let kept = kept::of(&device_id, &name).ok_or_else(|| format!("{name} isn't paired with (olo-airplay pair)"))?;
    olo_airplay::check(at, &kept, WAIT)?;
    println!("{name} knows this computer.");
    Ok(())
}

/// "90", "1:30" or "1:02:03" as seconds.
fn seconds_of(text: &str) -> Option<f64> {
    let parts: Vec<f64> = text.split(':').map(|p| p.parse::<f64>().ok().filter(|n| *n >= 0.0 && n.is_finite())).collect::<Option<_>>()?;
    (1..=3).contains(&parts.len()).then(|| parts.iter().fold(0.0, |all, part| all * 60.0 + part))
}

/// Seconds as a film's time: "2:03", "1:02:03".
fn time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// To the millisecond: as near as a place in a film is known.
fn thousandths(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

/// A play that couldn't be, told to the person (`--notify`: nobody is
/// reading what this prints). It waits while the notification shows, at
/// most half a minute: the note goes with the program that showed it.
fn tell(receiver: &str, title: &str, why: &str) {
    use olo_kit_base::notify::{Note, NoteEvent, Notifier};
    let (tx, rx) = std::sync::mpsc::channel();
    let notes = Notifier::with_events("AirPlay", "olo-preferences", move |e| {
        let _ = tx.send(e);
    });
    let mut why = why.to_owned();
    if let Some(first) = why.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    notes.show(None, Note::new(format!("{receiver} didn’t play {title}")).body(format!("{}.", why.trim_end_matches('.'))).icon("airplay").action("default", "AirPlay Preferences"));
    while let Ok(event) = rx.recv_timeout(Duration::from_secs(30)) {
        match event {
            NoteEvent::Action { .. } => {
                let _ = olo_xdg::spawn_detached("olo-preferences airplay");
                return;
            }
            NoteEvent::Closed { .. } => return,
            NoteEvent::Replied { .. } => {}
        }
    }
}

fn play(args: &[String]) -> Result<(), String> {
    // (What is to play, for telling the person of anything that keeps it
    // from being asked for at all.)
    let notify = args.iter().any(|a| a == "--notify");
    let asked = play_asked(args);
    if notify && let Err(why) = &asked {
        let words = words_in(args);
        let what = words.last().map_or("it", |w| w.rsplit('/').next().unwrap_or(w));
        let receiver = option(args, "--on").map(|on| kept::all().into_iter().find(|k| k.device_id.eq_ignore_ascii_case(on)).map_or_else(|| on.to_owned(), |k| k.name));
        tell(receiver.as_deref().unwrap_or("AirPlay"), what, why);
    }
    asked
}

fn play_asked(args: &[String]) -> Result<(), String> {
    let (json, trace, notify) = (args.iter().any(|a| a == "--json"), args.iter().any(|a| a == "--trace"), args.iter().any(|a| a == "--notify"));
    let from = match option(args, "--from") {
        Some(text) => seconds_of(text).ok_or_else(|| format!("{text} isn't a time (seconds, or 1:02:03)"))?,
        None => 0.0,
    };
    // The receiver by --on, the rest the film; or the last word the
    // film and the words before it the receiver's name.
    let mut words = words_in(args);
    let (receiver, what) = match option(args, "--on") {
        Some(on) if !words.is_empty() => (on.to_owned(), words.join(" ")),
        None if words.len() >= 2 => {
            let what = words.pop().unwrap_or_default().to_owned();
            (words.join(" "), what)
        }
        _ => return Err(USAGE.into()),
    };
    let what = what.as_str();
    let (at, device_id, name) = find(&receiver, args)?;
    let kept = kept::of(&device_id, &name).ok_or_else(|| format!("{name} isn't paired with (olo-airplay pair)"))?;
    let credentials = kept.credentials().ok_or("its pairing here can't be read: pair with it again")?;
    let (what, title) = if what.starts_with("http://") || what.starts_with("https://") {
        (What::Address(what.to_owned()), what.to_owned())
    } else {
        let path = std::fs::canonicalize(what).map_err(|e| format!("{what}: {e}"))?;
        let title = path.file_name().map_or_else(|| what.to_owned(), |n| n.to_string_lossy().into_owned());
        (What::File(path), title)
    };
    // Ctrl-C (and being told to end) stops the film at the receiver, and
    // `olo-airplay pause` and `resume` reach this play as signals too:
    // they are waited for by a thread of their own, kept from every other
    // (the play's are made after this).
    let signals = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGUSR1, libc::SIGUSR2] {
            libc::sigaddset(&mut set, signal);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    let trace: Option<Trace> = trace.then(|| Arc::new(|line: &str| eprintln!("  {line}")) as Trace);
    let play = Play::begin(at, &credentials, &what, &Options { from, name: our_name(), trace, ..Options::default() }).map_err(|e| e.to_string())?;
    let remote = play.remote();
    std::thread::spawn(move || {
        loop {
            let mut signal = 0;
            unsafe { libc::sigwait(&signals, &mut signal) };
            match signal {
                libc::SIGUSR1 => drop(remote.pause(true)),
                libc::SIGUSR2 => drop(remote.pause(false)),
                _ => return remote.interrupt(),
            }
        }
    });
    // How it goes, where Deskbar and `olo-airplay playing` find it:
    // written when what it's doing changes or where it is jumps, not as
    // it goes.
    let (pid, started) = (std::process::id(), airplay::now_ms() / 1000);
    let path = match &what {
        What::File(path) => Some(path.clone()),
        What::Address(_) => None,
    };
    let mut written: Option<Playing> = None;
    let mut publish = |now: &Now| {
        let state = match now.state {
            State::Loading => "loading",
            State::Playing => "playing",
            State::Paused => "paused",
            State::Ended | State::Left | State::Failed => return,
        };
        let new = Playing {
            pid,
            receiver: name.clone(),
            title: title.clone(),
            path: path.clone(),
            state: state.into(),
            position: now.position_now(),
            duration: now.duration,
            rate: now.rate,
            as_of: airplay::now_ms(),
            started,
        };
        let same = written.as_ref().is_some_and(|old| {
            let near = match (old.position_now(), new.position) {
                (Some(a), Some(b)) => (a - b).abs() < 1.5,
                (a, b) => a == b,
            };
            (&old.state, old.duration, old.rate) == (&new.state, new.duration, new.rate) && near
        });
        if !same {
            let _ = new.write();
            written = Some(new);
        }
    };
    let said = |now: &Now| {
        if json {
            let state = match now.state {
                State::Loading => "loading",
                State::Playing => "playing",
                State::Paused => "paused",
                State::Ended => "ended",
                State::Left => "left",
                State::Failed => "failed",
            };
            println!("{}", serde_json::json!({ "state": state, "position": now.position_now().map(thousandths), "duration": now.duration, "rate": now.rate, "why": now.why }));
            return;
        }
        let at = now.position_now().unwrap_or(0.0);
        match now.state {
            State::Loading => println!("{name} is fetching {title}."),
            State::Playing => println!(
                "Playing on {name}{}{}.",
                if at >= 1.0 { format!(", from {}", time(at)) } else { String::new() },
                now.duration.map(|d| format!(" (it is {} long)", time(d))).unwrap_or_default()
            ),
            State::Paused => println!("Paused at {}.", time(at)),
            State::Ended => println!("It played to its end."),
            State::Left => println!("It was left at {}: {}.", time(at), now.why.as_deref().unwrap_or("the receiver went on to something else")),
            State::Failed => {}
        }
    };
    let mut now = play.now();
    said(&now);
    publish(&now);
    while !now.state.over() && !play.interrupted() {
        let next = play.next(&now, None);
        // (Said when what it is doing changes, not each time it says where it is.)
        if next.state != now.state {
            said(&next);
        }
        publish(&next);
        now = next;
    }
    let stopped = !now.state.over();
    let _ = std::fs::remove_file(airplay::path_of(pid));
    drop(play);
    // (Left before it had played a moment of it: it didn't play, as far
    // as the person who asked can tell.)
    if notify && now.state == State::Left && now.position_now().unwrap_or(0.0) < from + 2.0 {
        tell(&name, &title, now.why.as_deref().unwrap_or("the receiver went on to something else"));
    }
    if stopped {
        if json {
            println!("{}", serde_json::json!({ "state": "stopped", "position": now.position_now().map(thousandths), "duration": now.duration, "rate": 0.0, "why": null }));
        } else {
            println!("Stopped.");
        }
    }
    match now.state {
        State::Failed => Err(now.why.unwrap_or_else(|| "it didn't play".into())),
        _ => Ok(()),
    }
}

/// How a play goes, in a line: "A film.mp4 on Front Room: playing, 2:03 of 1:32:10".
fn in_a_line(p: &Playing) -> String {
    let place = match (p.position_now(), p.duration) {
        (Some(at), Some(all)) => format!(", {} of {}", time(at), time(all)),
        (Some(at), None) => format!(", at {}", time(at)),
        _ => String::new(),
    };
    format!("{} on {}: {}{place}", p.title, p.receiver, p.state)
}

fn playing(args: &[String]) -> Result<(), String> {
    let all = airplay::all();
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&all).unwrap_or_else(|_| "[]".into()));
    } else if all.is_empty() {
        println!("Nothing is playing.");
    } else {
        for p in &all {
            println!("{}", in_a_line(p));
        }
    }
    Ok(())
}

/// Stop, pause or go on with the plays on a receiver (on every one, for
/// no name): each play is a process of this program, told by a signal.
fn tell_plays(args: &[String], signal: i32) -> Result<(), String> {
    let name = name_in(args);
    let mut plays = name.as_deref().map_or_else(airplay::all, airplay::on);
    // (One play exactly, by its process: Deskbar's menu.)
    if let Some(pid) = option(args, "--pid") {
        let pid = pid.parse::<u32>().map_err(|_| format!("{pid} isn't a play's number"))?;
        plays.retain(|p| p.pid == pid);
    }
    if plays.is_empty() {
        return Err(name.map_or_else(|| "nothing is playing".to_owned(), |name| format!("nothing is playing on {name}")));
    }
    for p in plays {
        // (Only a play: never whatever has its number by now.)
        if airplay::alive(p.pid) && unsafe { libc::kill(p.pid as i32, signal) } == 0 {
            let done = match signal {
                libc::SIGUSR1 => "Paused",
                libc::SIGUSR2 => "Going on with",
                _ => "Stopped",
            };
            println!("{done} {} on {}.", p.title, p.receiver);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_play_in_a_line() {
        let mut p = Playing {
            pid: 1,
            receiver: "Front Room".into(),
            title: "A film.mp4".into(),
            path: None,
            state: "paused".into(),
            position: Some(123.0),
            duration: Some(5530.0),
            rate: 0.0,
            as_of: 1,
            started: 0,
        };
        assert_eq!(in_a_line(&p), "A film.mp4 on Front Room: paused, 2:03 of 1:32:10");
        p.duration = None;
        assert_eq!(in_a_line(&p), "A film.mp4 on Front Room: paused, at 2:03");
        (p.position, p.state) = (None, "loading".into());
        assert_eq!(in_a_line(&p), "A film.mp4 on Front Room: loading");
    }

    #[test]
    fn times_typed_and_said() {
        assert_eq!(seconds_of("90"), Some(90.0));
        assert_eq!(seconds_of("1:30"), Some(90.0));
        assert_eq!(seconds_of("1:02:03.5"), Some(3723.5));
        for not in ["", "soon", "1:2:3:4", "-5", "1::2"] {
            assert_eq!(seconds_of(not), None, "{not}");
        }
        assert_eq!((time(0.4), time(123.9), time(3723.0)), ("0:00".to_owned(), "2:03".to_owned(), "1:02:03".to_owned()));
    }

    #[test]
    fn the_words_that_arent_options() {
        let args: Vec<String> = ["Front", "Room", "--from", "1:30", "film.mp4", "--json", "--at", "127.0.0.1:7000"].map(String::from).to_vec();
        assert_eq!(words_in(&args), ["Front", "Room", "film.mp4"]);
        assert_eq!(option(&args, "--from"), Some("1:30"));
        assert_eq!(name_in(&args[..2]).as_deref(), Some("Front Room"));
        assert_eq!(name_in(&args[5..]), None);
    }
}
