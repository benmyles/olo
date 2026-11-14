//! Fixes that need root: Olo Doctor's own program run again as root
//! (`olo-doctor --helper`) through pkexec (polkit asks for the password
//! with Olo's agent; there is no in-process way across that boundary),
//! running the steps it is sent and answering over JSON lines, as
//! Activity Monitor's helper does. It lives as long as the app: when its
//! input closes it exits, and the password is asked once.
//!
//! Asked for only in the person's own session (`OLO_REAL_SESSION`, not
//! on an agent's screen), where polkit's dialog belongs.
//! `OLO_DOCTOR_HELPER=direct` runs it as this user without pkexec
//! (tests: the protocol, nothing elevated); `=off` never.

use olo_sync::Mutex;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use crate::fix::Step;

#[derive(Debug, Serialize, Deserialize)]
pub enum ToHelper {
    Run { id: u64, steps: Vec<Step> },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum FromHelper {
    Ready,
    /// How the fix `id` is going (a package download's progress).
    Progress {
        id: u64,
        text: String,
    },
    /// The steps' outcomes, in order; the first failure stops the rest.
    Done {
        id: u64,
        results: Vec<Result<(), String>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Way {
    Pkexec,
    Direct,
    Off,
}

pub fn way() -> Way {
    match std::env::var("OLO_DOCTOR_HELPER").as_deref() {
        Ok("direct") => Way::Direct,
        Ok("off") => Way::Off,
        _ if olo_system::env::at_the_computer() => Way::Pkexec,
        _ => Way::Off,
    }
}

/// Run `steps` in order, stopping at the first failure.
pub fn run_steps(steps: &[Step]) -> Vec<Result<(), String>> {
    run_steps_with(steps, &mut |_| {})
}

/// Run `steps` in order, stopping at the first failure, telling how each
/// goes (`progress`: the step, then what it says as it goes).
pub fn run_steps_with(steps: &[Step], progress: &mut dyn FnMut(String)) -> Vec<Result<(), String>> {
    let mut results = Vec::with_capacity(steps.len());
    for s in steps {
        let what = s.describe();
        progress(what.clone());
        let r = s.run_with(&mut |p| progress(format!("{what}: {p}")));
        let failed = r.is_err();
        results.push(r);
        if failed {
            break;
        }
    }
    results
}

/// Run `steps` as root and wait for them: here if this is root already
/// (under sudo), else through a helper of our own, asked for now and
/// gone when they're done (pkexec asks for the password, in the
/// person's own session only; as `OLO_DOCTOR_HELPER` says in a test).
/// Err: root wasn't given, or isn't asked for here.
pub fn run_as_root(steps: &[Step], progress: &mut dyn FnMut(String)) -> Result<Vec<Result<(), String>>, String> {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return Ok(run_steps_with(steps, progress));
    }
    // (Never asked for from a test session or an agent's screen: `way`.)
    let way = way();
    if way == Way::Off {
        return Err("this is set in the person's own session, where the administrator's password is asked.".into());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let helper = Helper::start(way, move |m| _ = tx.send(m)).map_err(|e| format!("can't run as the administrator: {e}"))?;
    // Ready once the password is given.
    if !matches!(rx.recv(), Ok(Some(FromHelper::Ready))) {
        return Err("an administrator's password is needed.".into());
    }
    helper.send(&ToHelper::Run { id: 1, steps: steps.to_vec() });
    loop {
        match rx.recv() {
            Ok(Some(FromHelper::Done { id: 1, results })) => return Ok(results),
            Ok(Some(FromHelper::Progress { text, .. })) => progress(text),
            Ok(Some(_)) => continue,
            _ => return Err("the administrator's helper stopped.".into()),
        }
    }
}

/// Be the helper for the app on the other end of stdin and stdout.
pub fn serve() -> ! {
    serve_on(BufReader::new(std::io::stdin()), std::io::stdout());
    std::process::exit(0)
}

/// Answer what comes in on `input` until it closes (or `out` does).
fn serve_on(input: impl BufRead, out: impl Write) {
    let out = std::cell::RefCell::new(out);
    let say = |m: &FromHelper| {
        let Ok(mut line) = serde_json::to_string(m) else { return true };
        line.push('\n');
        let mut out = out.borrow_mut();
        out.write_all(line.as_bytes()).and_then(|_| out.flush()).is_ok()
    };
    if !say(&FromHelper::Ready) {
        return;
    }
    for line in input.lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<ToHelper>(&line) else { continue };
        let answer = match msg {
            ToHelper::Run { id, steps } => FromHelper::Done { id, results: run_steps_with(&steps, &mut |text| _ = say(&FromHelper::Progress { id, text })) },
        };
        // The app went: nothing to do but go too.
        if !say(&answer) {
            return;
        }
    }
}

/// The app's end of a running helper.
pub struct Helper {
    child: Option<Child>,
    input: Arc<Mutex<ChildStdin>>,
}

impl Helper {
    /// Start it (pkexec asks for the password on polkit's schedule while
    /// this returns at once); `heard` gets what it says, then None once
    /// it has gone (refused, or quit).
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
            // (What it went by, so a wrong "no" can be found: the
            // person's own session once said this of itself.)
            Way::Off => {
                let why = match std::env::var("OLO_DOCTOR_HELPER").as_deref() {
                    Ok("off") => "it's turned off here (OLO_DOCTOR_HELPER)".to_owned(),
                    _ if std::env::var_os("OLO_SCREEN").is_some() => "this is an agent's screen, with nobody in front of it to ask".to_owned(),
                    _ => format!(
                        "this isn't the session at the computer (it's on {}, and the person's is {})",
                        olo_xdg::session::own_display().unwrap_or_else(|| "no display".into()),
                        olo_xdg::session::real_display().unwrap_or_else(|| "not said".into())
                    ),
                };
                return Err(std::io::Error::other(why));
            }
        };
        let mut child = cmd.arg("--helper").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let stdout = child.stdout.take().expect("piped");
        let input = Arc::new(Mutex::new(child.stdin.take().expect("piped")));
        std::thread::Builder::new().name("doctor-helper".into()).spawn(move || {
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

    /// What `way` is in the environment this is run in (to find out why
    /// a session says root can't be asked for): `… way_here --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn way_here() {
        println!("way: {:?}; at the computer: {}", way(), olo_system::env::at_the_computer());
    }

    #[test]
    fn the_helper_runs_steps_in_order_and_stops_at_a_failure() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-doctor-helper-test-{}", std::process::id()));
        let file = dir.join("a");
        let steps = vec![
            Step::WriteFile { path: file.clone(), content: "hi".into(), mode: 0o600 },
            Step::WriteSysfs { path: dir.join("missing/b"), value: "x".into() },
            Step::RemoveFile { path: file.clone() },
        ];
        let input = format!("not json\n{}\n", serde_json::to_string(&ToHelper::Run { id: 7, steps }).unwrap());
        let mut out = Vec::new();
        serve_on(input.as_bytes(), &mut out);
        let said: Vec<FromHelper> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(matches!(said[0], FromHelper::Ready));
        // Each step said as it begins, then the outcomes.
        let begun: Vec<&str> = said[1..said.len() - 1]
            .iter()
            .map(|m| match m {
                FromHelper::Progress { id: 7, text } => text.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(begun, [format!("Writing {}", file.display()), format!("Setting {} to x", dir.join("missing/b").display())]);
        match said.last().unwrap() {
            FromHelper::Done { id: 7, results } => {
                assert_eq!(results.len(), 2, "stopped at the failure: {results:?}");
                assert!(results[0].is_ok());
                assert!(results[1].as_ref().unwrap_err().contains("missing/b"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hi", "the removal after the failure didn't run");
        let _ = std::fs::remove_dir_all(dir);
    }
}
