//! `pbcopy`, as macOS's: what it reads (all of it) goes on the clipboard
//! as text, for Paste in any app, until something else is copied. It
//! returns once the clipboard has it; a copy of itself, on its own (a
//! session of its own, no terminal), serves it to whoever pastes
//! (`olo_kit_base::clipboard::hold`). The session puts it on PATH as
//! `pbcopy` (and `olo-pbpaste` as `pbpaste`). macOS's `-pboard` and
//! `-help` are taken and the rest ignored, as there.

use std::io::{Read as _, Write as _};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-help" || a == "--help") {
        println!("usage: pbcopy [-pboard {{general | ruler | find | font}}]\n  Copies what it reads to the clipboard, as text.");
        return;
    }
    let mut data = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut data) {
        eprintln!("pbcopy: {e}");
        std::process::exit(1);
    }
    let clip = olo_kit_base::clipboard::text(&String::from_utf8_lossy(&data));
    let (mut said, tell) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pbcopy: {e}");
            std::process::exit(1);
        }
    };
    // SAFETY: no thread has been started yet; the child only makes its
    // own connection and threads after the fork.
    match unsafe { libc::fork() } {
        -1 => {
            eprintln!("pbcopy: {}", std::io::Error::last_os_error());
            std::process::exit(1);
        }
        0 => {
            drop(said);
            // On its own: no terminal to hang on, out of the shell's
            // process group (a ^C there doesn't take the clipboard away).
            // SAFETY: setsid and dup2 onto 0..=2 have no memory effects.
            unsafe {
                libc::setsid();
                if let Ok(null) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/null") {
                    let fd = std::os::fd::AsRawFd::as_raw_fd(&null);
                    libc::dup2(fd, 0);
                    libc::dup2(fd, 1);
                    libc::dup2(fd, 2);
                }
            }
            let mut tell = Some(tell);
            let held = olo_kit_base::clipboard::hold_then(clip, || {
                if let Some(mut t) = tell.take() {
                    let _ = t.write_all(b"ok\n");
                }
            });
            if let (Err(e), Some(mut t)) = (held, tell) {
                let _ = writeln!(t, "{e}");
            }
            std::process::exit(0);
        }
        _ => {
            drop(tell);
            let mut answer = String::new();
            let _ = said.read_to_string(&mut answer);
            if answer.trim() != "ok" {
                let why = answer.trim();
                eprintln!("pbcopy: {}", if why.is_empty() { "the clipboard couldn't be set" } else { why });
                std::process::exit(1);
            }
        }
    }
}
