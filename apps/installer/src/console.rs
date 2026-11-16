//! `olo-installer --console`: the Installer with no window, for a Fedora
//! with no desktop to show one (a minimal install; Ben, 2026-11-15: "we
//! should be 100% independent of gnome", and "just have olo installer
//! and olo doctor install the needed packagekit/dnf stuff"). The same
//! plan (`plan::install`) as the window, its stages and progress written
//! to the terminal; the licence agreed to by typing "agree" (or `--yes`).
//! The administrator's helper is started through `sudo`, which asks for
//! the password on the terminal (no polkit agent on a console, and polkit
//! itself may not be installed yet). Where there's no login screen at all
//! (no display manager), Olo's own is chosen: there'd be no way in to
//! Olo otherwise.

use std::{
    io::{BufRead as _, Write as _},
    sync::{Arc, atomic::AtomicBool, mpsc},
};

use crate::plan::{self, Choices, Event};

/// The licence, as the window shows it.
const LICENCE: &str = include_str!("licence.txt");

pub fn run(mut choices: Choices, given: Option<String>, agreed: bool) -> std::process::ExitCode {
    let c = plan::look();
    if let Some(why) = plan::unsuitable(&c) {
        eprintln!("olo-installer: {why}");
        return std::process::ExitCode::FAILURE;
    }
    println!("Olo Installer\n");
    if !agreed {
        println!("{}", LICENCE.trim_end());
        print!("\nType \"agree\" to agree to the licence and install Olo: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).is_err() || line.trim() != "agree" {
            println!("Nothing was installed.");
            return std::process::ExitCode::FAILURE;
        }
    }
    if olo_doctor::login_screen::display_manager(std::path::Path::new("/")).is_none() && !choices.login_screen {
        println!("This computer has no login screen: Olo's own is installed too, so there's a way in to Olo.");
        choices.login_screen = true;
    }
    // SAFETY: before any thread starts.
    unsafe { std::env::set_var("OLO_SETUP_HELPER", "sudo") };
    let (tx, rx) = mpsc::channel::<Event>();
    let tx = std::sync::Mutex::new(tx);
    let stop = Arc::new(AtomicBool::new(false));
    let worker = std::thread::spawn(move || plan::install(choices, given, stop, move |e| _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(e)));
    let mut last = Said::default();
    let mut code = std::process::ExitCode::FAILURE;
    for e in rx {
        match say(&e, &mut last) {
            Some(Ok(())) => code = std::process::ExitCode::SUCCESS,
            Some(Err(())) => code = std::process::ExitCode::FAILURE,
            None => {}
        }
    }
    let _ = worker.join();
    code
}

/// What's been written: the progress line last written (not written
/// again), and the step it was under ("Installing mesa-dri-drivers and 21
/// more"), said once with its details under it.
#[derive(Default)]
struct Said {
    line: String,
    step: String,
}

/// An event written to the terminal; Some at the end, whether it worked.
fn say(e: &Event, last: &mut Said) -> Option<Result<(), ()>> {
    match e {
        Event::Stage { n, of, title } => {
            println!("\n[{n}/{of}] {title}");
            last.step.clear();
        }
        Event::Progress { share, text } => {
            // (A share only where it's known: a stage's own steps say theirs.)
            let pct = share.filter(|s| *s > 0.0).map_or(String::new(), |s| format!("{:>3.0}%  ", s * 100.0));
            let (step, detail) = text.split_once(": ").map_or((text.as_str(), None), |(a, b)| (a, Some(b)));
            if step != last.step {
                println!("  {pct}{step}");
                last.step = step.to_owned();
            }
            if let Some(d) = detail {
                let line = format!("      {pct}{}", d.trim());
                if line != last.line {
                    println!("{line}");
                    last.line = line;
                }
            }
        }
        Event::Note(n) => println!("  {n}"),
        Event::Password => println!("  The administrator's password, for what needs it:"),
        Event::Done { version, restart } => {
            println!("\nOlo {version} is installed.");
            println!("{}", if *restart { "Restart the computer, then choose Olo at the login screen." } else { "Log out, then choose Olo at the login screen." });
            return Some(Ok(()));
        }
        Event::Failed { what, why, advice } => {
            eprintln!("\n{what} didn't work: {why}\n{advice}\nRun olo-installer --console again: it goes on from where it stopped.");
            return Some(Err(()));
        }
        Event::Stopped => {
            println!("\nStopped. Run it again to go on.");
            return Some(Err(()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_it_says() {
        let mut last = Said::default();
        assert_eq!(say(&Event::Stage { n: 1, of: 6, title: "This computer".into() }, &mut last), None);
        assert_eq!(say(&Event::Progress { share: Some(0.5), text: "Downloading".into() }, &mut last), None);
        assert_eq!(last.step, "Downloading");
        // A step's details under it, the step said once.
        assert_eq!(say(&Event::Progress { share: Some(0.0), text: "Installing mesa and 21 more: harfbuzz".into() }, &mut last), None);
        assert_eq!((last.step.as_str(), last.line.as_str()), ("Installing mesa and 21 more", "      harfbuzz"));
        assert_eq!(say(&Event::Done { version: "1.0".into(), restart: true }, &mut last), Some(Ok(())));
        assert_eq!(say(&Event::Failed { what: "Getting what Olo needs".into(), why: "no network".into(), advice: "Check it.".into() }, &mut last), Some(Err(())));
    }
}
