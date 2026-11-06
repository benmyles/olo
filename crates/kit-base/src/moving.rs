//! Moving an app, or one of its windows, to another display's app_server:
//! an agent screen's, or from one to the person's (`olo-screen take` and
//! `give`, the window menu's Move to Agent Screen). The app is asked over
//! its socket (`commands`: `{"move":MoveTo}`) and goes there itself: the
//! whole app re-executes with its state saved and the environment of the
//! display it goes to, as for an upgrade; a window alone is opened by a
//! copy of the app started there with that window's state (an app that
//! can, `olo_ui::on_move_window`), and let go here.
//!
//! The person's app on an agent screen remembers where it came from
//! (`OLO_HOME`: what its environment was), and goes back there if the
//! screen stops, or ends without a word.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Where to go, and what of the app.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MoveTo {
    /// The window to move (its `wl_surface` id on the app's connection,
    /// `WindowInfo::surface`); None, the whole app.
    #[serde(default)]
    pub surface: Option<u32>,
    /// The environment the app has there (`WAYLAND_DISPLAY`, `OLO_SOCKET`,
    /// `OLO_SESSION_PID`, an agent screen's `OLO_SCREEN`, …), and what it
    /// no longer has.
    #[serde(default)]
    pub set: Vec<(String, String)>,
    #[serde(default)]
    pub unset: Vec<String>,
}

/// How it went.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Moved {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The process now showing it there: the app's own pid (it
    /// re-executes, keeping it), or the copy's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

impl Moved {
    pub fn failed(why: impl Into<String>) -> Moved {
        Moved { ok: false, error: Some(why.into()), pid: None }
    }
}

/// What a program that answers on the kit's socket but isn't on the kit
/// says (Chrome's tree, through the Olo extension's host): it moves as
/// any other program does, through olo-wlproxy.
pub const NOT_KIT: &str = "it isn't an app on Olo's kit";

/// The variable naming where the person's app came from, while it is on
/// an agent screen: a [`MoveTo`] that brings it back.
pub const HOME: &str = "OLO_HOME";

/// Set for an app as it starts where it was moved to (taken out of the
/// environment as it starts, `claim`): a launch it is not, so it doesn't
/// hand itself to a copy already running there (`instance::forward`).
pub const MOVED: &str = "OLO_MOVED";

/// This process was moved here (once; the variable gone after).
pub fn claim() -> bool {
    static MOVED_HERE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *MOVED_HERE.get_or_init(|| {
        let here = std::env::var_os(MOVED).is_some();
        if here {
            // SAFETY: claimed at start-up (the kit's `run`, first thing, or
            // `instance::forward` before it), before the app starts threads.
            unsafe { std::env::remove_var(MOVED) };
        }
        here
    })
}

/// The environment to start with on the display `to` names, given the
/// one this process has: `to`'s changes, and `OLO_HOME` set as the app
/// goes from the person's display to a screen (what it had of everything
/// `to` changes), dropped as it goes back to theirs.
pub fn env_for(to: &MoveTo, now: impl Fn(&str) -> Option<String>) -> MoveTo {
    let mut out = MoveTo { surface: to.surface, set: to.set.clone(), unset: to.unset.clone() };
    let to_screen = to.set.iter().any(|(k, _)| k == "OLO_SCREEN");
    if !to_screen {
        out.unset.push(HOME.to_owned());
    } else if now(HOME).is_none() && now("OLO_SCREEN").is_none() {
        let mut home = MoveTo::default();
        for key in to.set.iter().map(|(k, _)| k).chain(&to.unset) {
            match now(key) {
                Some(v) => home.set.push((key.clone(), v)),
                None => home.unset.push(key.clone()),
            }
        }
        home.unset.push(HOME.to_owned());
        if let Ok(text) = serde_json::to_string(&home) {
            out.set.push((HOME.to_owned(), text));
        }
    }
    out.set.push((MOVED.to_owned(), "1".to_owned()));
    out.unset.retain(|k| !out.set.iter().any(|(s, _)| s == k));
    out
}

/// Apply `env` to a command about to start the app there.
pub fn apply(env: &MoveTo, cmd: &mut std::process::Command) {
    for k in &env.unset {
        cmd.env_remove(k);
    }
    for (k, v) in &env.set {
        cmd.env(k, v);
    }
}

/// Where this app came from, if it is the person's on an agent screen.
pub fn home() -> Option<MoveTo> {
    serde_json::from_str(&std::env::var(HOME).ok()?).ok()
}

/// The agent screen this app is on has ended: its session gone, or the
/// app_server socket it was given removed (a session removes its sockets
/// as it ends). An app_server restarting in place keeps both.
pub fn screen_gone() -> bool {
    let socket = std::env::var_os("OLO_SOCKET");
    let session = std::env::var("OLO_SESSION_PID").ok().and_then(|p| p.parse::<u32>().ok());
    socket.is_some_and(|s| !Path::new(&s).exists()) || session.is_some_and(|pid| !Path::new(&format!("/proc/{pid}")).exists())
}

/// Ask the kit app with process id `pid` to move (blocks for up to a
/// few seconds: call it off the UI thread).
pub fn ask(pid: u32, to: &MoveTo) -> Moved {
    match crate::commands::ask_move(pid, to) {
        Some(m) => m,
        None => Moved::failed("it didn't answer (not an app on Olo's kit, or one from before it could move)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> MoveTo {
        MoveTo {
            surface: None,
            set: vec![("WAYLAND_DISPLAY".into(), "wayland-3".into()), ("OLO_SCREEN".into(), "agent".into()), ("PULSE_SERVER".into(), "unix:/nowhere".into())],
            unset: vec!["DISPLAY".into()],
        }
    }

    #[test]
    fn going_to_a_screen_remembers_home() {
        let person = |k: &str| match k {
            "WAYLAND_DISPLAY" => Some("wayland-1".to_owned()),
            _ => None,
        };
        let env = env_for(&screen(), person);
        let home: MoveTo = serde_json::from_str(&env.set.iter().find(|(k, _)| k == HOME).unwrap().1).unwrap();
        assert_eq!(home.set, vec![("WAYLAND_DISPLAY".to_owned(), "wayland-1".to_owned())]);
        assert!(home.unset.contains(&"OLO_SCREEN".to_owned()) && home.unset.contains(&"PULSE_SERVER".to_owned()) && home.unset.contains(&HOME.to_owned()));
        // Applied, home takes the screen's away again.
        let back = env_for(&home, |k| env.set.iter().find(|(s, _)| s == k).map(|(_, v)| v.clone()));
        assert!(back.unset.contains(&"OLO_SCREEN".to_owned()) && back.unset.contains(&HOME.to_owned()));
        assert!(!back.set.iter().any(|(k, _)| k == HOME));
    }

    #[test]
    fn from_screen_to_screen_keeps_the_first_home() {
        let on_screen = |k: &str| match k {
            "OLO_SCREEN" => Some("other".to_owned()),
            HOME => Some("{}".to_owned()),
            _ => None,
        };
        let env = env_for(&screen(), on_screen);
        assert!(!env.set.iter().any(|(k, _)| k == HOME));
        assert!(!env.unset.contains(&HOME.to_owned()));
    }

    #[test]
    fn started_on_a_screen_given_to_the_person_has_no_home() {
        let person = MoveTo { surface: None, set: vec![("WAYLAND_DISPLAY".into(), "wayland-1".into())], unset: vec!["OLO_SCREEN".into()] };
        let env = env_for(&person, |k| (k == "OLO_SCREEN").then(|| "agent".to_owned()));
        assert!(env.unset.contains(&HOME.to_owned()) && env.unset.contains(&"OLO_SCREEN".to_owned()));
    }
}

// ---- For menus ---------------------------------------------------------------

/// An agent screen running now, as menus list it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunningScreen {
    pub name: String,
    /// The name the person gave it, else its own.
    pub label: String,
    pub wayland_display: String,
    pub olo_socket: std::path::PathBuf,
    /// Its session's pid.
    pub pid: i32,
    pub sound: bool,
}

impl RunningScreen {
    /// What a program on it has in its environment.
    pub fn env(&self) -> Vec<(String, String)> {
        screen_env(&self.name, &self.wayland_display, &self.olo_socket, self.pid, self.sound)
    }
}

/// The folder agent screens are listed in.
fn screens_dir() -> std::path::PathBuf {
    olo_xdg::legacy::runtime_file(&olo_xdg::runtime_dir(), "olo-screens")
}

/// The environment a program on agent screen `name` gets: its display and
/// app_server, Chrome's profile of its own (beside its history), and
/// silence unless the screen has sound. (`olo-screen run` and moves
/// alike.)
pub fn screen_env(name: &str, wayland_display: &str, olo_socket: &Path, pid: i32, sound: bool) -> Vec<(String, String)> {
    let mut env = vec![
        ("WAYLAND_DISPLAY".to_owned(), wayland_display.to_owned()),
        ("OLO_SOCKET".to_owned(), olo_socket.display().to_string()),
        ("OLO_SESSION".to_owned(), "1".to_owned()),
        ("OLO_SESSION_PID".to_owned(), pid.to_string()),
        ("OLO_SCREEN".to_owned(), name.to_owned()),
        ("CHROME_CONFIG_HOME".to_owned(), olo_xdg::state_dir().join("screens").join(name).display().to_string()),
    ];
    if !sound {
        env.push(("PULSE_SERVER".to_owned(), format!("unix:{}", screens_dir().join("no-sound").display())));
    }
    env
}

/// The agent screens running (as `olo-screen` lists them in
/// `$XDG_RUNTIME_DIR/olo-screens`, their sessions alive), by name, but
/// the one this program is on.
pub fn screens() -> Vec<RunningScreen> {
    let dir = screens_dir();
    let own = olo_xdg::display_name();
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<RunningScreen> = read
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".json") && !name.ends_with("-settings.json")
        })
        .filter_map(|e| std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()))
        .filter(|v| v["wayland_display"].as_str().is_some_and(|d| d != own))
        .filter(|v| {
            let cmdline = v["pid"].as_i64().and_then(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).ok()).unwrap_or_default();
            let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
            // (`tabula-…` for a screen started before the rename.)
            let session = args.first().is_some_and(|a| a.ends_with(b"olo-session") || a.ends_with(b"tabula-session"));
            let socket = v["olo_socket"].as_str().or(v["tabula_socket"].as_str());
            session && args.iter().any(|a| *a == b"--headless") && socket.is_some_and(|s| Path::new(s).exists())
        })
        .filter_map(|v| {
            let name = v["name"].as_str()?.to_owned();
            let title = std::fs::read_to_string(olo_xdg::state_dir().join("screens").join(&name).join("title")).ok().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
            Some(RunningScreen {
                label: title.unwrap_or_else(|| name.clone()),
                name,
                wayland_display: v["wayland_display"].as_str()?.to_owned(),
                olo_socket: v["olo_socket"].as_str().or(v["tabula_socket"].as_str())?.into(),
                pid: v["pid"].as_i64()? as i32,
                sound: v["sound"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    out.sort_by_key(|s| s.label.to_lowercase());
    out
}

/// A menu item's command for moving to agent screen NAME (after it), and
/// for a new screen.
pub const SCREEN_COMMAND: &str = "agent-screen:";
pub const NEW_SCREEN_COMMAND: &str = "agent-screen-new";

/// The screen a menu's command moves to: Some(Some(NAME)), Some(None) for
/// a new one; None if it isn't such a command.
pub fn screen_command(command: &str) -> Option<Option<String>> {
    if command == NEW_SCREEN_COMMAND {
        return Some(None);
    }
    command.strip_prefix(SCREEN_COMMAND).map(|n| Some(n.to_owned()))
}

/// Windows can be moved to agent screens from here: the person's own
/// display (not a screen's).
pub fn menus_offer() -> bool {
    std::env::var_os("OLO_SCREEN").is_none()
}

/// Move `windows` (ids on this display) to agent screen `screen` (None:
/// a new one) as a menu does: `olo-screen take`, on a thread of its own;
/// if nothing moved, a notification says why.
pub fn take_to_screen(screen: Option<String>, windows: Vec<u64>) {
    let _ = std::thread::Builder::new().name("move-to-screen".into()).spawn(move || {
        let Some(program) = olo_xdg::find_program("olo-screen") else {
            failed("Agent Screens isn't installed.");
            return;
        };
        let mut cmd = std::process::Command::new(program);
        cmd.arg("take");
        match &screen {
            Some(name) => cmd.arg(name),
            None => cmd.arg("--new"),
        };
        for w in &windows {
            cmd.arg("--window").arg(w.to_string());
        }
        cmd.stdin(std::process::Stdio::null());
        match cmd.output() {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let why = err.lines().last().unwrap_or("").trim_start_matches("olo-screen: ").to_owned();
                failed(&why);
            }
            Err(e) => failed(&e.to_string()),
        }
    });
}

fn failed(why: &str) {
    static NOTES: std::sync::OnceLock<crate::notify::Notifier> = std::sync::OnceLock::new();
    let notes = NOTES.get_or_init(|| crate::notify::Notifier::with_events("Agent Screens", "olo-screen", |_| {}));
    let why = why.strip_prefix("nothing moved: ").unwrap_or(why);
    notes.show(None, crate::notify::Note::new("Couldn't move to an agent screen").body(why));
    tracing::warn!("moving to an agent screen: {why}");
}

/// Agent screen `name`, running; None: a new one, started (`olo-screen
/// start`). Blocks while a screen starts: call it off the UI thread.
pub fn ensure_screen(name: Option<&str>) -> Result<RunningScreen, String> {
    let name = match name {
        Some(n) => n.to_owned(),
        None => {
            let program = olo_xdg::find_program("olo-screen").ok_or("Agent Screens isn't installed")?;
            let out = std::process::Command::new(program).arg("start").stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("olo-screen: ").to_owned());
            }
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
            v["name"].as_str().ok_or("the new screen didn't say its name")?.to_owned()
        }
    };
    screens().into_iter().find(|s| s.name == name).ok_or_else(|| format!("screen {name} isn't running"))
}

/// Start a copy of this program with `args`, handing it `state` (what
/// `restore::take` gives it), in the environment `env` says: in a session
/// of its own. Its pid.
pub fn start_copy(args: &[String], state: Option<&str>, env: &MoveTo) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt as _;
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let exe = std::env::current_exe()?;
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    apply(env, &mut cmd);
    if let Some(state) = state.filter(|s| !s.is_empty()) {
        let file = olo_xdg::runtime_dir().join(format!("olo-restore-{}-copy{}.json", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&file, state)?;
        cmd.env(crate::restore::ENV, &file);
    }
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();
    std::thread::spawn(move || child.wait());
    Ok(pid)
}

/// A copy of this program on agent screen `screen` (None: a new one),
/// started with `args` and `state` (a window's or a tab's), on a thread of
/// its own; `then` hears it started (its pid) or not; if not, a
/// notification says why.
pub fn copy_to_screen(screen: Option<String>, args: Vec<String>, state: Option<String>, then: impl FnOnce(Result<u32, String>) + Send + 'static) {
    let _ = std::thread::Builder::new().name("copy-to-screen".into()).spawn(move || {
        let r = ensure_screen(screen.as_deref()).and_then(|s| {
            let to = MoveTo { surface: None, set: s.env(), unset: vec!["DISPLAY".into()] };
            let env = env_for(&to, |k| std::env::var(k).ok());
            start_copy(&args, state.as_deref(), &env).map_err(|e| e.to_string())
        });
        if let Err(e) = &r {
            failed(e);
        }
        then(r);
    });
}
