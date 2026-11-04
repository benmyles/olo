//! olo-open: `xdg-open` and `$BROWSER` in Terminal's shells. A link or a
//! file is opened in the app Olo opens it with (Preferences ▸ Default
//! Apps, `olo_xdg::open`), started on its own, so the command that asked
//! (`gh auth login`, `git web--browse`, a build's report) carries on at
//! once. The system's xdg-open ran the browser as the command's child,
//! and `gh auth login` waited for Firefox to quit before it would finish
//! (Ben, 2026-11-04). Terminal links `xdg-open` to this in its shells'
//! bin directory, beside `sudo` (olo-sudo), and sets `BROWSER` to it.

use std::process::ExitCode;

use olo_xdg::open::Target;

/// xdg-open's exit statuses.
const SYNTAX: u8 = 1;
const MISSING: u8 = 2;
const FAILED: u8 = 4;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("xdg-open (Olo's olo-open)");
        return ExitCode::SUCCESS;
    }
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h" || a == "--manual") {
        eprintln!("usage: xdg-open {{ file | URL }}...\nOpens each in the app Olo opens it with (Preferences ▸ Default Apps), and returns at once.");
        return ExitCode::from(SYNTAX);
    }
    let here = std::env::current_dir().unwrap_or_default();
    let mut status = 0;
    for a in &args {
        let target = match Target::parse(a) {
            Target::Path(p) if p.is_relative() => Target::Path(here.join(p)),
            t => t,
        };
        if let Target::Path(p) = &target
            && !p.exists()
        {
            eprintln!("xdg-open: file '{}' does not exist", p.display());
            status = status.max(MISSING);
            continue;
        }
        if let Err(e) = olo_xdg::open::open(&target) {
            eprintln!("xdg-open: {a}: {e}");
            status = status.max(FAILED);
        }
    }
    ExitCode::from(status)
}
