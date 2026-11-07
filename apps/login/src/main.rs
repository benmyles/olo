//! olo-login: Olo's login screen's daemon, in GDM's place (see
//! `olo_greet`).
//!
//! ```text
//! olo-login --daemon [--programs DIR] [--vt N]   the display manager (root)
//! olo-login --daemon --test DIR                  the same, unprivileged, for tests
//! olo-login --worker WAY USER [--vt N] [--test DIR]   one way in (the daemon starts it)
//! ```
//!
//! A test (`--test DIR`) needs no root, seat or terminal: PAM's stack is
//! `DIR/pam`'s (a stand-in; never the real one), the people and sessions
//! `DIR/root`'s, the greeter connects to `DIR/greeter.sock`, and a
//! session runs as whoever runs the test (`DIR/started` and `DIR/ended`
//! say when).

mod daemon;
mod vt;
mod worker;

use std::path::PathBuf;

const USAGE: &str = "usage: olo-login --daemon [--programs DIR] [--vt N] [--test DIR]
       olo-login --worker password|fingerprint|face|greeter USER [--vt N] [--test DIR]";

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let test = value("--test").map(PathBuf::from);
    let vt = value("--vt").and_then(|v| v.parse().ok()).unwrap_or(1);
    let code = match args.first().map(String::as_str) {
        Some("--daemon") => {
            // SAFETY: getuid has no failure.
            if test.is_none() && unsafe { libc::getuid() } != 0 {
                eprintln!("olo-login: the daemon runs as root (or with --test DIR)");
                std::process::exit(1);
            }
            let programs = value("--programs").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/local/libexec/olo"));
            daemon::run(daemon::Config { test, programs, vt })
        }
        Some("--worker") => {
            let (Some(way), Some(user)) = (args.get(1), args.get(2)) else {
                eprintln!("{USAGE}");
                std::process::exit(2);
            };
            let (service, class) = match way.as_str() {
                "password" => (olo_greet::Way::Password.service(), "user"),
                "fingerprint" => (olo_greet::Way::Fingerprint.service(), "user"),
                "face" => (olo_greet::Way::Face.service(), "user"),
                "greeter" => ("olo-login-greeter", "greeter"),
                _ => {
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                }
            };
            worker::run(worker::Setup { service: service.into(), user: user.clone(), class, vt, test })
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}
