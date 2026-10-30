//! Full access: Activity Monitor's own program run again as root
//! (`olo-activity --helper`), which measures everything (every process's
//! I/O, open files and connections, other users' details) and does what
//! this account may not (signal or renice any process, set tunables and
//! governors), speaking JSON lines over its standard input and output.
//!
//! Root is reached through pkexec, polkit's way to run a program as
//! another user: the one boundary there is no in-process way across (a
//! process can't raise its own privileges; polkit asks for the password
//! through Olo's agent). Not systemd's transient units: with SELinux
//! enforcing, a service may not run a program from a home folder. The
//! helper lives as long as the app: when its input closes it exits.
//!
//! It is asked for only in the person's own session (`OLO_REAL_SESSION`):
//! in a test session or on an agent's screen, polkit's password dialog
//! would come up on the person's screen for something they didn't start.
//! `OLO_ACTIVITY_HELPER=direct` runs the helper without pkexec, as this
//! user (tests: the whole protocol, nothing elevated); `=off` never asks.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::{
    actions::{Action, Refusal},
    details::Details,
    sample::{self, Control, Sample, Source, Wants},
};

#[derive(Debug, Serialize, Deserialize)]
pub enum ToHelper {
    Wants(Wants),
    Inspect { pid: i32 },
    Act { id: u64, action: Action },
}

#[derive(Serialize, Deserialize)]
pub enum FromHelper {
    Ready,
    Sources(Vec<Source>),
    Sample(Box<Sample>),
    Details { pid: i32, details: Result<Box<Details>, String> },
    Done { id: u64, result: Result<(), Refusal> },
}

/// How the helper is reached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Way {
    Pkexec,
    /// As this user (tests).
    Direct,
    /// Not at all.
    Off,
}

pub fn way() -> Way {
    match std::env::var("OLO_ACTIVITY_HELPER").as_deref() {
        Ok("direct") => Way::Direct,
        Ok("off") => Way::Off,
        _ if std::env::var_os("OLO_REAL_SESSION").is_some() && std::env::var_os("OLO_SCREEN").is_none() => Way::Pkexec,
        _ => Way::Off,
    }
}

/// Be the helper: measure and act for the app on the other end of stdin
/// and stdout, until it goes.
pub fn serve() -> ! {
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let say = {
        let out = out.clone();
        move |m: &FromHelper| {
            let Ok(mut line) = serde_json::to_string(m) else { return };
            line.push('\n');
            let mut o = out.lock().unwrap();
            // The app went: nothing to do but go too.
            if o.write_all(line.as_bytes()).and_then(|_| o.flush()).is_err() {
                std::process::exit(0);
            }
        }
    };
    say(&FromHelper::Ready);
    let control = Control::new(1000);
    let (s1, s2) = (say.clone(), say.clone());
    sample::start(control.clone(), move |sources| s1(&FromHelper::Sources(sources)), move |sample| s2(&FromHelper::Sample(Box::new(sample))));
    for line in BufReader::new(std::io::stdin()).lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<ToHelper>(&line) else { continue };
        match msg {
            ToHelper::Wants(w) => control.set(w),
            ToHelper::Inspect { pid } => {
                let say = say.clone();
                std::thread::spawn(move || say(&FromHelper::Details { pid, details: crate::details::read_details(pid).map(Box::new) }));
            }
            ToHelper::Act { id, action } => say(&FromHelper::Done { id, result: action.run() }),
        }
    }
    std::process::exit(0)
}

/// The app's end of a running helper.
pub struct Helper {
    child: Option<Child>,
    input: Arc<Mutex<ChildStdin>>,
}

impl Helper {
    /// Start the helper (pkexec asks for the password, on polkit's
    /// schedule, while this returns at once); `heard` gets what it says,
    /// then None once it has gone (refused, or quit).
    pub fn start(way: Way, heard: impl Fn(Option<FromHelper>) + Send + 'static) -> std::io::Result<Helper> {
        let exe = std::env::current_exe()?;
        // An upgrade replaced the program under us: its new self is at
        // the same path.
        let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
        let mut cmd = match way {
            Way::Pkexec => {
                let mut c = Command::new("pkexec");
                c.arg("--disable-internal-agent").arg(&exe);
                c
            }
            Way::Direct => Command::new(&exe),
            Way::Off => return Err(std::io::Error::other("full access is turned off")),
        };
        let mut child = cmd.arg("--helper").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let stdout = child.stdout.take().expect("piped");
        let input = Arc::new(Mutex::new(child.stdin.take().expect("piped")));
        std::thread::Builder::new().name("activity-helper".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(m) = serde_json::from_str::<FromHelper>(&line) {
                    heard(Some(m));
                }
            }
            heard(None);
        })?;
        Ok(Helper { child: Some(child), input })
    }

    pub fn send(&self, m: &ToHelper) {
        let Ok(mut line) = serde_json::to_string(m) else { return };
        line.push('\n');
        let input = self.input.clone();
        // Off the UI thread: a pipe can be full.
        std::thread::spawn(move || {
            let mut i = input.lock().unwrap();
            let _ = i.write_all(line.as_bytes()).and_then(|_| i.flush());
        });
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        // Closing its input ends it; reap it off the UI thread.
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let m = ToHelper::Act { id: 3, action: Action::Signal { pid: 5, signal: 15 } };
        let back: ToHelper = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert!(matches!(back, ToHelper::Act { id: 3, action: Action::Signal { pid: 5, signal: 15 } }));
        let s = FromHelper::Done { id: 1, result: Err(Refusal { message: "x".into(), needs_access: true }) };
        let back: FromHelper = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(matches!(back, FromHelper::Done { id: 1, result: Err(_) }));
    }
}
