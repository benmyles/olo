//! Windows moved between the person's display and agent screens
//! (`olo-screen take` and `give`; the window menu's, Deskbar's and the
//! viewer's Move to Agent Screen and Move Windows to): the program keeps
//! running as it goes.
//!
//! - An app on Olo's kit is asked to go (`olo_ui::moving`): all of it
//!   re-executes there with its state, as for an upgrade; one window of
//!   several goes by a copy of the app started there with that window's
//!   state, where the app can (Tracker's folder windows; Terminal's).
//! - Any other program (Chrome, GTK, Qt, Electron) is moved by the
//!   olo-wlproxy that holds its connection (`control` there): told what a
//!   restart ends, then replayed on the other app_server, as across an
//!   upgrade. A program's connection is all its windows, so they all go.
//!
//! The person's programs on a screen go back to their display when the
//! screen stops (`send_home`, from `screens::stop`), or ends without a
//! word (the proxy and the kit each notice).

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use olo_protocol::{Client, Reply, Request, WindowInfo};
use olo_ui::moving::{self, MoveTo};

use crate::screens::Screen;

/// A display's session: the person's, or an agent screen's.
#[derive(Debug, Clone)]
pub struct Place {
    /// The screen's name; None for the person's display.
    pub screen: Option<String>,
    pub wayland_display: String,
    pub olo_socket: PathBuf,
    /// What a program there has in its environment (and doesn't).
    pub env: MoveTo,
}

impl Place {
    /// The person's own display: where this program runs, unless that's
    /// a screen.
    pub fn person() -> Result<Place, String> {
        if std::env::var_os("OLO_SCREEN").is_some() {
            return Err("this is an agent screen: windows are given to the person from their own display (olo-screen run from it)".into());
        }
        let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| "not in the person's Olo session (WAYLAND_DISPLAY isn't set)")?;
        let socket = olo_protocol::socket_path().ok_or("not in the person's Olo session (OLO_SOCKET isn't set)")?;
        let mut env = MoveTo::default();
        env.set.push(("WAYLAND_DISPLAY".into(), display.clone()));
        env.set.push(("OLO_SOCKET".into(), socket.display().to_string()));
        env.set.push(("OLO_SESSION".into(), "1".into()));
        // What a screen gives its programs, as the person has it (or not).
        for key in ["OLO_SESSION_PID", "CHROME_CONFIG_HOME", "PULSE_SERVER"] {
            match std::env::var(key) {
                Ok(v) => env.set.push((key.into(), v)),
                Err(_) => env.unset.push(key.into()),
            }
        }
        env.unset.push("OLO_SCREEN".into());
        Ok(Place { screen: None, wayland_display: display, olo_socket: socket, env })
    }

    /// An agent screen, running.
    pub fn screen(s: &Screen) -> Place {
        let env = MoveTo { surface: None, set: s.env(), unset: vec!["DISPLAY".into()] };
        Place { screen: Some(s.name.clone()), wayland_display: s.wayland_display.clone(), olo_socket: s.olo_socket.clone(), env }
    }

    fn client(&self) -> Result<Client, String> {
        Client::connect_to(&self.olo_socket).map_err(|e| format!("{} isn't answering: {e}", self.describe()))
    }

    pub fn describe(&self) -> String {
        match &self.screen {
            Some(n) => format!("screen {n}"),
            None => "the person's display".into(),
        }
    }

    pub fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        match self.client()?.request(&Request::ListWindows).map_err(|e| e.to_string())? {
            Reply::Windows { windows } => Ok(windows),
            other => Err(format!("unexpected reply {other:?}")),
        }
    }

    pub fn request(&self, r: &Request) -> Result<Reply, String> {
        self.client()?.request(r).map_err(|e| e.to_string())
    }

    /// Its app_server's socket for programs that come through a proxy.
    fn proxied(&self) -> PathBuf {
        let name = Path::new(&self.wayland_display).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        olo_xdg::legacy::runtime_file(&olo_xdg::runtime_dir(), &format!("olo-{name}-proxied"))
    }
}

// ---- olo-wlproxy's control sockets ----------------------------------------

/// Every running olo-wlproxy's control socket (its pid's process alive).
fn proxies() -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(olo_xdg::runtime_dir()) else { return Vec::new() };
    let mut out: Vec<PathBuf> = dir
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix("olo-wlproxy-").and_then(|n| n.strip_suffix(".sock")).and_then(|p| p.parse::<u32>().ok()).is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists())
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// A request to a proxy, and its answer's lines (to the empty line that
/// ends a list, or the one line of anything else).
fn tell(proxy: &Path, line: &str) -> Result<Vec<String>, String> {
    let s = UnixStream::connect(proxy).map_err(|e| format!("olo-wlproxy at {}: {e}", proxy.display()))?;
    s.set_read_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
    (&s).write_all(format!("{line}\n").as_bytes()).map_err(|e| e.to_string())?;
    let list = line == "list";
    let mut out = Vec::new();
    for l in BufReader::new(&s).lines() {
        let l = l.map_err(|e| e.to_string())?;
        if list && l.is_empty() {
            break;
        }
        out.push(l);
        if !list {
            break;
        }
    }
    Ok(out)
}

/// The proxy that has program `pid`'s connection.
fn proxy_of(pid: u32) -> Option<PathBuf> {
    proxies().into_iter().find(|p| tell(p, "list").is_ok_and(|lines| lines.iter().any(|l| l.split(' ').next() == Some(&pid.to_string()))))
}

fn proxy_move(pid: u32, to: &Place) -> Result<(), String> {
    let proxy = proxy_of(pid).ok_or("it connected to app_server itself rather than through olo-wlproxy, so it can't move (start it again from Olo)")?;
    let answer = tell(&proxy, &format!("move {pid} {}", to.proxied().display()))?;
    match answer.first().map(String::as_str) {
        Some("ok") => Ok(()),
        Some(e) => Err(e.strip_prefix("error: ").unwrap_or(e).to_owned()),
        None => Err("olo-wlproxy didn't answer".into()),
    }
}

// ---- Moving ----------------------------------------------------------------

/// A program's windows that went, and where they are now.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Went {
    /// The program, and the process showing it there (the same, or a
    /// copy of a kit app holding one window).
    pub pid: u32,
    pub now: u32,
    pub app_id: String,
    /// Its windows there, by their ids on that display.
    pub windows: Vec<u64>,
    /// How: "kit" (re-executed there), "copy" (a window by a copy of the
    /// app), "proxy" (its connection replayed there).
    pub how: &'static str,
    /// Its other windows, which went with it (a program's connection is
    /// all of them).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What didn't go, and why.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Stayed {
    pub window: u64,
    pub app_id: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Outcome {
    pub went: Vec<Went>,
    pub stayed: Vec<Stayed>,
}

/// How long the windows have to come up on the other display.
const ARRIVE: Duration = Duration::from_secs(15);

/// Move the windows `ids` (on `from`) to `to`. An app all of whose
/// windows are going goes whole; a kit app keeping some sends the others
/// by a copy, where it can; another program goes with all its windows.
pub fn move_windows(from: &Place, ids: &[u64], to: &Place) -> Result<Outcome, String> {
    let all = from.windows()?;
    let mut by_pid: BTreeMap<u32, Vec<&WindowInfo>> = BTreeMap::new();
    let mut out = Outcome::default();
    for id in ids {
        let Some(w) = all.iter().find(|w| w.id == *id) else { return Err(format!("{} has no window {id}", from.describe())) };
        match w.pid {
            Some(pid) if w.kept => by_pid.entry(pid as u32).or_default().push(w),
            Some(_) => out.stayed.push(Stayed { window: w.id, app_id: w.app_id.clone(), why: "its program can't be moved: it connected to app_server itself, not through Olo".into() }),
            None => out.stayed.push(Stayed { window: w.id, app_id: w.app_id.clone(), why: "its program is unknown".into() }),
        }
    }
    // What each program's windows were before, to know they've come.
    let mut waiting: Vec<(Went, usize)> = Vec::new();
    for (pid, going) in by_pid {
        let theirs: Vec<&WindowInfo> = all.iter().filter(|w| w.pid == Some(pid as i32)).collect();
        let whole = theirs.iter().all(|t| going.iter().any(|g| g.id == t.id));
        let app_id = going[0].app_id.clone();
        // The whole app if all of it goes (unless it's part of the desktop
        // there, as Tracker is), else window by window.
        let asks: Vec<(Option<&WindowInfo>, MoveTo)> =
            whole.then(|| (None, MoveTo { surface: None, ..to.env.clone() })).into_iter().chain(going.iter().map(|g| (Some(*g), MoveTo { surface: g.surface, ..to.env.clone() }))).collect();
        let mut by_proxy = false;
        for (k, (window, ask)) in asks.iter().enumerate() {
            let answer = moving::ask(pid, ask);
            if answer.ok {
                let now = answer.pid.unwrap_or(pid);
                let how = if now == pid { "kit" } else { "copy" };
                let expect = if now == pid { theirs.len() } else { 1 };
                waiting.push((Went { pid, now, app_id: app_id.clone(), windows: Vec::new(), how, note: None }, expect));
                if now == pid {
                    break;
                }
                continue;
            }
            // (No socket, or Chrome's tree server's: not on the kit.)
            let kit = olo_xdg::runtime_dir().join(format!("olo-menus-{pid}.sock")).exists()
                && answer.error.as_deref() != Some(moving::NOT_KIT)
                && !answer.error.as_deref().is_some_and(|e| e.starts_with("it didn't answer"));
            if !kit {
                by_proxy = k == 0;
                break;
            }
            // The whole app refused: one window at a time, next.
            if let Some(w) = window {
                out.stayed.push(Stayed { window: w.id, app_id: w.app_id.clone(), why: answer.error.unwrap_or_else(|| "it couldn't move".into()) });
            }
        }
        if by_proxy {
            match proxy_move(pid, to) {
                Ok(()) => {
                    let note = (!whole).then(|| format!("its {} other window(s) went too: a program's windows go together", theirs.len() - going.len()));
                    waiting.push((Went { pid, now: pid, app_id, windows: Vec::new(), how: "proxy", note }, theirs.len()));
                }
                Err(why) => out.stayed.extend(going.iter().map(|w| Stayed { window: w.id, app_id: w.app_id.clone(), why: why.clone() })),
            }
        }
    }
    // Until each has its windows there.
    let until = Instant::now() + ARRIVE;
    loop {
        let there = to.windows().unwrap_or_default();
        let mut done = true;
        for (went, expect) in &mut waiting {
            went.windows = there.iter().filter(|w| w.pid == Some(went.now as i32)).map(|w| w.id).collect();
            done &= went.windows.len() >= *expect;
        }
        if done || Instant::now() > until {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for (went, _) in waiting {
        if went.windows.is_empty() {
            out.stayed.push(Stayed { window: 0, app_id: went.app_id.clone(), why: format!("it was sent to {} but hasn't shown a window there yet", to.describe()) });
        }
        out.went.push(went);
    }
    Ok(out)
}

/// Screen `s`'s windows (`ids`; None, all it can give) to the person's
/// display, onto its workspace `index` (from 0) of `output` (the active
/// display when None), or wherever they come (`index` None).
pub fn give(s: &Screen, ids: Option<Vec<u64>>, index: Option<u32>, output: Option<String>) -> Result<Outcome, String> {
    let person = Place::person()?;
    let from = Place::screen(s);
    let ids = match ids.filter(|i| !i.is_empty()) {
        Some(ids) => ids,
        None => movable(&from.windows()?),
    };
    if ids.is_empty() {
        return Err(format!("screen {} has no windows to give", s.name));
    }
    let outcome = move_windows(&from, &ids, &person)?;
    if let Some(index) = index {
        for id in outcome.went.iter().flat_map(|w| &w.windows) {
            person.request(&Request::MoveToWorkspace { id: *id, index, output: output.clone() })?;
        }
    }
    Ok(outcome)
}

/// The person's display-workspaces, as Deskbar names them ("D1 Workspace
/// 2" among several displays), each with its display (connector) and
/// index: where windows can be given.
pub fn person_workspaces() -> Result<Vec<(String, u32, String)>, String> {
    let person = Place::person()?;
    let count = match person.request(&Request::Workspaces)? {
        Reply::Workspaces { count, .. } => count,
        _ => 1,
    };
    let outputs = match person.request(&Request::Outputs)? {
        Reply::Outputs { outputs } => outputs,
        _ => Vec::new(),
    };
    let mut displays: Vec<&olo_protocol::OutputInfo> = outputs.iter().filter(|o| o.mirror_of.is_none()).collect();
    displays.sort_by_key(|o| o.position);
    let mut out = Vec::new();
    for d in displays {
        let number = olo_protocol::display_number(&outputs, &d.name);
        for n in 0..count.clamp(1, 9) {
            out.push((d.name.clone(), n, olo_protocol::workspace_name(number, n)));
        }
    }
    Ok(out)
}

/// The windows on a screen the person can be given: every window there
/// but the screen's own (Deskbar and the desktop are layers, not listed).
pub fn movable(windows: &[WindowInfo]) -> Vec<u64> {
    windows.iter().filter(|w| w.kept && w.pid.is_some()).map(|w| w.id).collect()
}

/// Before screen `s` stops: the person's programs on it back to their
/// display (each proxy's that came from it; each kit app that remembers
/// a home), and a moment for them to go.
pub fn send_home(s: &Screen) {
    let place = Place::screen(s);
    let from = place.proxied();
    for p in proxies() {
        let _ = tell(&p, &format!("home {}", from.display()));
    }
    let windows = place.windows().unwrap_or_default();
    let mut pids: Vec<u32> = windows.iter().filter_map(|w| w.pid).map(|p| p as u32).collect();
    pids.sort_unstable();
    pids.dedup();
    let homes: Vec<(u32, MoveTo)> = pids.into_iter().filter_map(|pid| Some((pid, home_of(pid)?))).collect();
    for (pid, home) in &homes {
        let answer = moving::ask(*pid, home);
        if !answer.ok {
            tracing::warn!("pid {pid} didn't go back to the person's display: {}", answer.error.unwrap_or_default());
        }
    }
    // (Gone from the screen, or a few seconds.)
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        let left = place.windows().unwrap_or_default();
        if !left.iter().any(|w| w.pid.is_some_and(|p| homes.iter().any(|(h, _)| *h == p as u32))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Where kit app `pid` came from, if it's the person's on a screen (its
/// environment's `OLO_HOME`).
fn home_of(pid: u32) -> Option<MoveTo> {
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let value = environ.split(|b| *b == 0).find_map(|kv| kv.strip_prefix(format!("{}=", moving::HOME).as_bytes()))?;
    serde_json::from_slice(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movable_leaves_out_what_cannot_move() {
        let w = |id: u64, pid: Option<i32>, kept: bool| -> WindowInfo {
            serde_json::from_value(serde_json::json!({ "id": id, "app_id": "a", "title": "t", "pid": pid, "workspace": 0, "minimized": false, "focused": false, "kept": kept })).unwrap()
        };
        assert_eq!(movable(&[w(1, Some(10), true), w(2, None, true), w(3, Some(11), false)]), vec![1]);
    }
}
