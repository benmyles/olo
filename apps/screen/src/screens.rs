//! Agent screens: each one a headless Olo session of its own (its own
//! app_server, Deskbar and desktop, windows, pointer and keyboard focus),
//! so what an agent does there never moves the person's pointer or takes
//! their focus. The person watches (and steps in) through the viewer.
//!
//! A screen is `olo-session --headless WxH`, started in its own process
//! group with settings of its own (`OLO_APP_SERVER_SETTINGS`: the
//! person's, at 1× unless asked, never locking, blanking or sleeping) and
//! silent unless asked. It is listed in `$XDG_RUNTIME_DIR/olo-screens/
//! NAME.json`; everything else talks to its app_server's IPC socket.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_protocol::{Chord, Client, ImageFormat, Reply, Request, Settings, SyntheticEvent, WindowInfo};
use serde::{Deserialize, Serialize};

/// evdev button codes.
pub const LEFT: u32 = 0x110;
pub const RIGHT: u32 = 0x111;
pub const MIDDLE: u32 = 0x112;

/// One agent screen, as listed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Screen {
    pub name: String,
    pub wayland_display: String,
    #[serde(alias = "tabula_socket")]
    pub olo_socket: PathBuf,
    /// olo-session's pid (the leader of the screen's process group).
    pub pid: i32,
    /// Logical size.
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub sound: bool,
    /// Seconds since the epoch.
    pub started: u64,
    /// Its own session bus (`olo-bus`, in its session), and the person's
    /// accessibility bus its programs use; None for a screen from before
    /// (on the person's bus).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a11y: Option<String>,
}

/// Where Chrome keeps its profile on screen `name` (`CHROME_CONFIG_HOME`,
/// which Chrome, Chromium and olo-chrome all follow): beside the screen's
/// history. With the person's own, a Chrome started there was handed to
/// the person's running Chrome, which opened the window on their display.
pub fn chrome_home(name: &str) -> PathBuf {
    crate::history::dir(name)
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// Where screens are listed (and their logs and settings kept).
pub fn dir() -> PathBuf {
    olo_xdg::legacy::runtime_file(&runtime_dir(), "olo-screens")
}

fn entry(name: &str) -> PathBuf {
    dir().join(format!("{name}.json"))
}

/// Marks that the person has screen `name`'s pointer and keys (Agent
/// Screens' Take Control), in `dir`: the viewer's pid, while they have
/// them.
fn control_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.control"))
}

/// The person takes screen `name` over (`true`) or hands it back: said
/// for `olo-screen`, which does nothing to the screen meanwhile.
pub fn set_controlled(name: &str, on: bool) {
    set_controlled_in(&dir(), name, on);
}

fn set_controlled_in(dir: &Path, name: &str, on: bool) {
    let file = control_file(dir, name);
    if on {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&file, std::process::id().to_string());
    } else {
        let _ = std::fs::remove_file(&file);
    }
}

/// Whether the person is controlling screen `name` now (a viewer that
/// took it over and has since gone doesn't count).
pub fn controlled(name: &str) -> bool {
    controlled_in(&dir(), name)
}

fn controlled_in(dir: &Path, name: &str) -> bool {
    let Some(pid) = std::fs::read_to_string(control_file(dir, name)).ok().and_then(|s| s.trim().parse::<u32>().ok()) else { return false };
    // (The viewer is this program, in its window.)
    let ours = |exe: PathBuf| exe.file_name().is_some_and(|n| std::env::current_exe().ok().and_then(|me| me.file_name().map(|m| m == n)).unwrap_or(false));
    std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(ours)
}

/// Forget the screens this process said the person controls (a viewer
/// starting: an upgrade re-executes it with the same pid).
pub fn clear_controlled_by_me() {
    clear_controlled_in(&dir());
}

fn clear_controlled_in(dir: &Path) {
    let me = std::process::id().to_string();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "control") && std::fs::read_to_string(&path).is_ok_and(|s| s.trim() == me) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A name a screen can have: letters, digits, `-` and `_`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether a process with this command line and environment is screen
/// `name`'s session: an olo-session (`tabula-session` before the rename)
/// started for it, as its environment says (`OLO_SCREEN`, given at
/// `start` and kept across an upgrade's re-execution). Its command line
/// isn't: a session re-executed itself bare after an upgrade, so a
/// screen known by `--headless` there looked ended, `list` dropped it,
/// and it ran on unknown (ten on 2026-11-07, each with its own display
/// server).
fn is_session_of(cmdline: &[u8], environ: &[u8], name: &str) -> bool {
    let first = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    let session = first.ends_with(b"olo-session") || first.ends_with(b"tabula-session");
    let wanted = format!("OLO_SCREEN={name}");
    session && environ.split(|b| *b == 0).any(|v| v == wanted.as_bytes())
}

/// The screen a process is the session of, if it's a screen's session
/// (`OLO_SCREEN`'s name).
fn screen_of(pid: i32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let name = environ.split(|b| *b == 0).find_map(|v| v.strip_prefix(b"OLO_SCREEN=")).map(|n| String::from_utf8_lossy(n).into_owned())?;
    is_session_of(&cmdline, &environ, &name).then_some(name)
}

/// When a process started, in clock ticks since boot (`/proc/PID/stat`'s
/// 22nd field, after its name in parentheses, which may hold anything).
fn started_ticks(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

/// How long ago process `pid` started.
fn age(pid: i32) -> Option<Duration> {
    let ticks = started_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    Some(Duration::from_secs_f64((uptime - ticks as f64 / hz).max(0.0)))
}

fn parent(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// Screens' sessions no listed screen is (`(pid, screen)`): forgotten
/// by an olo-screen from before (every upgrade did), or left by one
/// that ended while starting a screen. Those of screen `name` whatever
/// their age; others once two minutes old, so a screen another agent is
/// starting now isn't one. Never a session inside a screen's (a test
/// session run there inherits its `OLO_SCREEN`).
fn strays(name: Option<&str>) -> Vec<(i32, String)> {
    let listed: Vec<i32> = list().iter().map(|s| s.pid).collect();
    let in_a_screen = |pid: i32| {
        let mut p = parent(pid);
        for _ in 0..64 {
            match p {
                Some(q) if q > 1 => {
                    if screen_of(q).is_some() {
                        return true;
                    }
                    p = parent(q);
                }
                _ => return false,
            }
        }
        false
    };
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        if listed.contains(&pid) || std::fs::read_to_string(format!("/proc/{pid}/comm")).map_or(true, |c| !c.trim().ends_with("-session")) {
            continue;
        }
        let Some(screen) = screen_of(pid) else { continue };
        let old = age(pid).is_some_and(|a| a > Duration::from_secs(120));
        if (Some(screen.as_str()) == name || old) && !in_a_screen(pid) {
            out.push((pid, screen));
        }
    }
    out
}

/// End sessions `pids` (each a screen's, in a process group of its own):
/// asked first, then, any still there after 5 s, their whole process
/// group ended (app_server and what the session started with it), so
/// none is left running unknown.
fn end_sessions(pids: &[i32]) {
    for &pid in pids {
        stop_pid(pid);
    }
    let gone = |pid: i32| !std::path::Path::new(&format!("/proc/{pid}")).exists() || screen_of(pid).is_none();
    let until = Instant::now() + Duration::from_secs(5);
    while pids.iter().any(|&p| !gone(p)) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    for &pid in pids.iter().filter(|&&p| !gone(p)) {
        tracing::warn!(pid, "a screen's session didn't end when asked; ending its process group");
        // SAFETY: plain kill(2), on the process group `start` made for it.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    let until = Instant::now() + Duration::from_secs(2);
    while pids.iter().any(|&p| !gone(p)) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// End the screens' sessions nobody lists any more (`strays`).
fn end_strays(name: Option<&str>) {
    let found = strays(name);
    for (pid, screen) in &found {
        tracing::info!(pid, screen, "a screen's session nothing lists; ending it");
    }
    end_sessions(&found.iter().map(|(p, _)| *p).collect::<Vec<_>>());
}

impl Screen {
    /// Its session is still running: the process listed is this
    /// screen's session (`is_session_of`), whatever its command line
    /// says now.
    pub fn running(&self) -> bool {
        screen_of(self.pid).as_deref() == Some(self.name.as_str())
    }

    /// Still running and reachable: its session alive and its socket
    /// there.
    pub fn alive(&self) -> bool {
        self.running() && self.olo_socket.exists()
    }

    fn client(&self) -> Result<Client, String> {
        Client::connect_to(&self.olo_socket).map_err(|e| format!("screen {} isn't answering: {e}", self.name))
    }

    pub fn request(&self, request: &Request) -> Result<Reply, String> {
        self.client()?.request(request).map_err(|e| e.to_string())
    }

    /// Its windows, front ones with where they are.
    pub fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        match self.request(&Request::ListWindows)? {
            Reply::Windows { windows } => Ok(windows),
            other => Err(format!("unexpected reply {other:?}")),
        }
    }

    pub fn input(&self, events: Vec<SyntheticEvent>) -> Result<(), String> {
        self.request(&Request::Synthetic { events }).map(drop)
    }

    /// Type `text`: what a US keyboard types as keys, anything else (é,
    /// emoji, CJK) put in whole as an input method would
    /// (`Request::InsertText`), in order.
    pub fn type_text(&self, text: &str) -> Result<(), String> {
        for (keys, run) in runs(text) {
            if keys {
                self.input(type_text(run)?)?;
            } else {
                if let Reply::Error { message } = self.request(&Request::InsertText { text: run.to_owned() })? {
                    return Err(format!("{run:?} couldn't be put in: {message}"));
                }
            }
        }
        Ok(())
    }

    /// Send each batch of events in turn, waiting after each as long as
    /// it says: as a hand would, so what reacts to movement over time (a
    /// drag, a touchpad's lift) sees it as from a person.
    pub fn input_paced(&self, steps: Vec<(Vec<SyntheticEvent>, Duration)>) -> Result<(), String> {
        let mut client = self.client()?;
        for (events, wait) in steps {
            client.request(&Request::Synthetic { events }).map_err(|e| e.to_string())?;
            std::thread::sleep(wait);
        }
        Ok(())
    }

    /// A picture of the whole screen (or one window) at `path`, once it
    /// is written.
    pub fn picture(&self, path: &Path, window: Option<u64>, pointer: bool, format: ImageFormat) -> Result<(), String> {
        let request = Request::Screenshot { path: path.display().to_string(), output: None, window, border: true, pointer, format, as_shown: false };
        self.request(&request).map(drop)
    }

    /// The environment a program on this screen gets: its display and
    /// app_server, no X display (X programs would show on the person's
    /// screen), Chrome's profile of its own (`chrome_home`), and silence
    /// unless the screen has sound.
    pub fn env(&self) -> Vec<(String, String)> {
        // (The kit's, which moves to a screen use too.)
        let mut env = olo_ui::moving::screen_env(&self.name, &self.wayland_display, &self.olo_socket, self.pid, self.sound);
        env.extend(olo_ui::moving::bus_env(self.bus.as_deref(), self.a11y.as_deref()));
        env
    }

    /// Start `argv` on this screen (a bare name is looked for as Olo
    /// finds programs); its pid.
    pub fn run(&self, argv: &[String]) -> Result<u32, String> {
        let program = argv.first().ok_or("no program to run")?;
        let path = if program.contains('/') { PathBuf::from(program) } else { olo_xdg::find_program(program).ok_or_else(|| format!("{program} isn't installed"))? };
        let mut cmd = Command::new(&path);
        cmd.args(&argv[1..]).env_remove("DISPLAY").stdin(Stdio::null());
        for (k, v) in self.env() {
            cmd.env(k, v);
        }
        // An Electron app's single-instance lock is in its config: the
        // screen's own, or it hands itself to the person's copy.
        if olo_xdg::electron::is_electron(path.as_os_str()) {
            let config = crate::history::dir(&self.name).join("config");
            let _ = std::fs::create_dir_all(&config);
            cmd.env("XDG_CONFIG_HOME", config);
        }
        let log = std::fs::File::create(dir().join(format!("{}-{}.log", self.name, sanitize(program))));
        if let Ok(log) = log {
            cmd.stdout(log.try_clone().map(Stdio::from).unwrap_or_else(|_| Stdio::null())).stderr(Stdio::from(log));
        }
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|e| format!("{program} couldn't be started: {e}"))?;
        let pid = child.id();
        // Reaped when it ends (a CLI that exits first leaves that to init).
        std::thread::spawn(move || child.wait());
        Ok(pid)
    }
}

fn sanitize(program: &str) -> String {
    Path::new(program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The running screens, by name. Entries whose session is gone are
/// removed.
pub fn list() -> Vec<Screen> {
    let mut screens = Vec::new();
    let Ok(read) = std::fs::read_dir(dir()) else { return screens };
    for e in read.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "json") || path.to_string_lossy().ends_with("-settings.json") {
            continue;
        }
        let Some(screen) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Screen>(&b).ok()) else { continue };
        // (Forgotten only once its session has ended: a socket missing a
        // moment, or a command line that changed, isn't that.)
        if screen.running() {
            screens.push(screen);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    screens.sort_by(|a, b| a.name.cmp(&b.name));
    screens
}

pub fn get(name: &str) -> Result<Screen, String> {
    list().into_iter().find(|s| s.name == name).ok_or_else(|| format!("there's no screen {name:?} (olo-screen list)"))
}

/// The first free "agent", "agent-2", ….
pub fn free_name() -> String {
    let taken: Vec<String> = list().into_iter().map(|s| s.name).collect();
    (1..).map(|n| if n == 1 { "agent".to_owned() } else { format!("agent-{n}") }).find(|n| !taken.contains(n)).unwrap_or_default()
}

/// What to start a screen with (kept beside its history: `kept`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Options {
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub sound: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { width: 1440, height: 900, scale: 1.0, sound: false }
    }
}

/// Where a screen's own settings are kept, beside its history: every
/// screen, however started (the viewer's New Screen leaves no history
/// till an agent acts on it), is listed once stopped, and starts again as
/// it was.
fn kept_path(name: &str) -> PathBuf {
    crate::history::dir(name).join("screen.json")
}

/// What screen `name` was started with, if it ever was.
pub fn kept(name: &str) -> Option<Options> {
    std::fs::read(kept_path(name)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn keep(name: &str, opts: &Options) {
    let path = kept_path(name);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(opts) {
        let _ = std::fs::write(path, json);
    }
}

/// How a screen is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Stopped,
    Archived,
}

/// Every screen there is (running, or stopped or archived with what it
/// kept), by name, and how it is.
pub fn all() -> Vec<(String, State)> {
    let running: Vec<String> = list().into_iter().map(|s| s.name).collect();
    let mut all: Vec<(String, State)> = running.iter().map(|n| (n.clone(), State::Running)).collect();
    for n in crate::history::names() {
        if !running.contains(&n) {
            let state = if crate::history::archived(&n) { State::Archived } else { State::Stopped };
            all.push((n, state));
        }
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all
}

/// Stop screen `name` if it runs, archive it.
pub fn archive(name: &str, on: bool) -> Result<(), String> {
    if !all().iter().any(|(n, _)| n == name) {
        return Err(format!("there's no screen {name:?} (olo-screen list)"));
    }
    if on && list().iter().any(|s| s.name == name) {
        return Err(format!("screen {name} is running: stop it first (olo-screen stop {name})"));
    }
    crate::history::set_archived(name, on)
}

/// Delete screen `name` and everything it kept (its history,
/// recordings, stats and Chrome profile), to the Trash; stopped first if
/// it runs.
pub fn delete(name: &str) -> Result<(), String> {
    if !all().iter().any(|(n, _)| n == name) {
        return Err(format!("there's no screen {name:?} (olo-screen list)"));
    }
    if list().iter().any(|s| s.name == name) {
        stop(name)?;
    }
    crate::history::trash(name)
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("olo").join("app_server.json")
}

/// The person's settings, made right for a screen nobody sits at: its
/// own scale, one display, and nothing that locks, blanks or sleeps.
pub fn screen_settings(person: Option<Settings>, scale: f64) -> Settings {
    let mut s = person.unwrap_or_default();
    s.display = olo_protocol::DisplaySettings { scale, ..Default::default() };
    s.power.display_off_minutes = 0;
    s.power.sleep_minutes = 0;
    s.power.lock_on_display_off = false;
    s.screen_saver.enabled = false;
    s.screen_saver.lock = false;
    s
}

/// Start a screen called `name`, and wait until it answers.
pub fn start(name: &str, opts: &Options) -> Result<Screen, String> {
    if !valid_name(name) {
        return Err(format!("{name:?} can't name a screen (letters, digits, - and _)"));
    }
    if list().iter().any(|s| s.name == name) {
        return Err(format!("there's already a screen {name:?}"));
    }
    // (One of this name nothing lists would be a second, unreachable.)
    end_strays(Some(name));
    if !(320..=7680).contains(&opts.width) || !(240..=4320).contains(&opts.height) {
        return Err(format!("{}x{} isn't a size a screen can have", opts.width, opts.height));
    }
    if !(1.0..=3.0).contains(&opts.scale) {
        return Err(format!("{} isn't a scale a screen can have (1 to 3)", opts.scale));
    }
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = std::fs::File::create(dir.join("no-sound"));
    let person: Option<Settings> = std::fs::read(config_path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let settings_path = dir.join(format!("{name}-settings.json"));
    let settings = serde_json::to_vec_pretty(&screen_settings(person, opts.scale)).map_err(|e| e.to_string())?;
    std::fs::write(&settings_path, settings).map_err(|e| e.to_string())?;

    let session = olo_xdg::find_program("olo-session").ok_or("olo-session isn't installed")?;
    let log_path = dir.join(format!("{name}.log"));
    let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
    // Pixels, as the backend takes them.
    let (pw, ph) = ((opts.width as f64 * opts.scale).round() as i32, (opts.height as f64 * opts.scale).round() as i32);
    // A session bus of its own, so a program there never finds the
    // person's copy of itself (GTK's), and dialogs it asks for show there;
    // the person's accessibility bus still, for `tree`.
    let bus = format!("unix:path={}", dir.join(format!("{name}.bus")).display());
    let a11y = person_a11y_bus();
    let mut cmd = Command::new(session);
    cmd.arg("--headless")
        .arg(format!("{pw}x{ph}"))
        .arg("--own-bus")
        .arg(&bus)
        .env("OLO_APP_SERVER_SETTINGS", &settings_path)
        .env("OLO_SCREEN", name)
        .env("CHROME_CONFIG_HOME", chrome_home(name))
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("OLO_SOCKET")
        .env_remove("OLO_SESSION")
        .env_remove("OLO_SESSION_PID")
        .env_remove("OLO_REAL_SESSION")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    if !opts.sound {
        cmd.env("PULSE_SERVER", format!("unix:{}", dir.join("no-sound").display()));
    }
    match &a11y {
        Some(a) => cmd.env("OLO_A11Y_BUS", a),
        None => cmd.env_remove("OLO_A11Y_BUS"),
    };
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("the screen couldn't be started: {e}"))?;
    let pid = child.id() as i32;
    std::thread::spawn(move || child.wait());

    // The session names its sockets on its first line, then app_server
    // answers on the IPC socket once it is up.
    let until = Instant::now() + Duration::from_secs(20);
    let (display, socket) = loop {
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        if let Some(found) = text.lines().find_map(sockets) {
            break found;
        }
        if Instant::now() > until || !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return Err(format!("the screen didn't start; see {}", log_path.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let screen = Screen {
        name: name.to_owned(),
        wayland_display: display,
        olo_socket: socket,
        pid,
        width: opts.width,
        height: opts.height,
        scale: opts.scale,
        sound: opts.sound,
        started: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        // (Its session says if it couldn't have its own.)
        bus: (!std::fs::read_to_string(&log_path).unwrap_or_default().contains("shares the person's")).then_some(bus),
        a11y,
    };
    loop {
        if matches!(screen.request(&Request::Ping), Ok(Reply::Pong { .. })) {
            break;
        }
        if Instant::now() > until {
            stop_pid(pid);
            return Err(format!("the screen started but its app_server doesn't answer; see {}", log_path.display()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    fastest(&screen);
    // Kept, to be listed once stopped and started again as it was; out
    // of the archive if it was there.
    keep(name, opts);
    let _ = crate::history::set_archived(name, false);
    let tmp = entry(name).with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&screen).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, entry(name)).map_err(|e| e.to_string())?;
    // Its frames, kept beside its history (`record`) for as long as it
    // runs: in its process group, so it ends with it.
    if let Ok(exe) = std::env::current_exe() {
        let mut rec = Command::new(exe);
        rec.args(["record", name]).process_group(pid).stdin(std::process::Stdio::null());
        if let Ok(log) = std::fs::File::create(dir.join(format!("{name}-record.log"))) {
            if let Ok(out) = log.try_clone() {
                rec.stdout(out);
            }
            rec.stderr(log);
        }
        if let Err(e) = rec.spawn() {
            tracing::warn!("screen {name}'s frames won't be kept: {e}");
        }
    }
    Ok(screen)
}

/// The person's accessibility bus (at-spi's), asked of their session bus
/// (starting it if need be), for a screen's programs to use: where
/// `olo-screen tree` reads GTK and Qt programs.
fn person_a11y_bus() -> Option<String> {
    // (A bus that leaves a call to a name nobody has unanswered must not
    // hold the screen up.)
    let conn = zbus::blocking::connection::Builder::session().ok()?.method_timeout(Duration::from_secs(2)).build().ok()?;
    let reply = conn.call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &()).ok()?;
    reply.body().deserialize::<String>().ok().filter(|a| !a.is_empty())
}

/// "session: WAYLAND_DISPLAY=wayland-4 OLO_SOCKET=/run/…" → its parts.
fn sockets(line: &str) -> Option<(String, PathBuf)> {
    let rest = line.strip_prefix("session: WAYLAND_DISPLAY=")?;
    let (display, socket) = rest.split_once(" OLO_SOCKET=")?;
    Some((display.to_owned(), PathBuf::from(socket.trim())))
}

/// The screen at the fastest refresh its size has (120 Hz): watched live
/// (`olo_capture::live`), it moves as smoothly as the person's own
/// screen. At rest it draws nothing either way; busy, it costs about the
/// same (a window dragged for five seconds: 4.2% of a core at 60 Hz,
/// 4.4% at 120).
fn fastest(screen: &Screen) {
    let Ok(Reply::Outputs { outputs }) = screen.request(&Request::Outputs) else { return };
    for o in outputs {
        let Some(now) = o.current_mode else { continue };
        let best = o.modes.iter().filter(|m| (m.width, m.height) == (now.width, now.height)).max_by_key(|m| m.refresh);
        if let Some(best) = best.filter(|b| b.refresh > now.refresh) {
            let config = olo_protocol::OutputConfig { mode: Some(best.clone()), ..Default::default() };
            if let Err(e) = screen.request(&Request::ConfigureOutput { name: o.name.clone(), config }) {
                tracing::warn!("screen {}: its display stays at {} Hz: {e}", screen.name, now.refresh / 1000);
            }
        }
    }
}

fn stop_pid(pid: i32) {
    // The session ends app_server and the programs it started.
    // SAFETY: plain kill(2).
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

/// End a screen and everything started on it.
pub fn stop(name: &str) -> Result<(), String> {
    let screen = get(name)?;
    // The person's programs on it back to their display first.
    crate::moving::send_home(&screen);
    end_sessions(&[screen.pid]);
    end_strays(Some(name));
    // Programs `run` started are in process groups of their own, tied to
    // the screen's display: they end when it goes (Wayland clients exit
    // when their compositor does).
    if screen.running() {
        // (Listed still, so it isn't forgotten while it runs: stopped again,
        // or by the next start of its name.)
        return Err(format!("screen {name} is still ending"));
    }
    let _ = std::fs::remove_file(entry(name));
    let _ = std::fs::remove_file(dir().join(format!("{name}-settings.json")));
    let _ = std::fs::remove_file(control_file(&dir(), name));
    Ok(())
}

// ---- Input -----------------------------------------------------------------

pub fn click(x: f64, y: f64, button: u32, count: u32) -> Vec<SyntheticEvent> {
    let mut events = vec![SyntheticEvent::Move { x, y }];
    for _ in 0..count.max(1) {
        events.push(SyntheticEvent::Button { button, pressed: true });
        events.push(SyntheticEvent::Button { button, pressed: false });
    }
    events
}

/// A drag from `from` to `to` in `steps` moves over `duration`, pausing
/// a little after the press and before the release, as a hand does (a
/// scroll bar's knob or a window's corner needs to see the press, then
/// movement over time).
pub fn drag(from: (f64, f64), to: (f64, f64), steps: u32, duration: Duration) -> Vec<(Vec<SyntheticEvent>, Duration)> {
    const SETTLE: Duration = Duration::from_millis(50);
    let steps = steps.max(1);
    let each = duration / steps;
    let mut out = vec![(vec![SyntheticEvent::Move { x: from.0, y: from.1 }, SyntheticEvent::Button { button: LEFT, pressed: true }], SETTLE)];
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        out.push((vec![SyntheticEvent::Move { x: from.0 + (to.0 - from.0) * t, y: from.1 + (to.1 - from.1) * t }], if i == steps { SETTLE } else { each }));
    }
    out.push((vec![SyntheticEvent::Button { button: LEFT, pressed: false }], Duration::ZERO));
    out
}

/// A touchpad scroll by (dx, dy) pixels: moves of at most 20 pixels every
/// 8 ms (as fingers report), then, unless `lift` is false, the fingers
/// resting a moment and lifting. Resting first means nothing glides on
/// (app_server glides a lift that comes while the fingers move fast), and
/// the lift ends the gesture, so a list pulled past its end springs back.
pub fn scroll(dx: f64, dy: f64, lift: bool) -> Vec<(Vec<SyntheticEvent>, Duration)> {
    const STEP: Duration = Duration::from_millis(8);
    let n = (dx.abs().max(dy.abs()) / 20.0).ceil().max(1.0) as u32;
    let mut out: Vec<_> = (0..n).map(|_| (vec![SyntheticEvent::Scroll { dx: dx / n as f64, dy: dy / n as f64, at_us: None }], STEP)).collect();
    if lift {
        if let Some(last) = out.last_mut() {
            last.1 = Duration::from_millis(80);
        }
        out.push((vec![SyntheticEvent::Scroll { dx: 0.0, dy: 0.0, at_us: None }], Duration::ZERO));
    }
    out
}

/// Mouse-wheel notches, one at a time (a toolkit may take only so much of
/// one event), a little apart.
pub fn wheel(dx: i32, dy: i32) -> Vec<(Vec<SyntheticEvent>, Duration)> {
    let n = dx.abs().max(dy.abs());
    (0..n)
        .map(|i| {
            let step = |d: i32| if i < d.abs() { d.signum() } else { 0 };
            (vec![SyntheticEvent::Wheel { dx: step(dx), dy: step(dy) }], Duration::from_millis(10))
        })
        .collect()
}

/// evdev codes of the modifiers.
const CTRL: u32 = 29;
const SHIFT: u32 = 42;
const ALT: u32 = 56;
const CMD: u32 = 125;

/// A chord as people write it: "Cmd+S", "ctrl+shift+t", "Enter", "Esc".
pub fn parse_chord(text: &str) -> Result<Chord, String> {
    let mut parts: Vec<String> = text.split('+').map(|p| p.trim().to_owned()).collect();
    if let Some(key) = parts.last_mut() {
        let alias = match key.to_ascii_lowercase().as_str() {
            "enter" | "return" => Some("Return"),
            "esc" => Some("Escape"),
            "del" => Some("Delete"),
            "pgup" => Some("PageUp"),
            "pgdn" | "pgdown" => Some("PageDown"),
            "space" | " " => Some("Space"),
            "-" => Some("Minus"),
            "=" => Some("Equal"),
            "," => Some("Comma"),
            "." => Some("Period"),
            "/" => Some("Slash"),
            ";" => Some("Semicolon"),
            "'" => Some("Apostrophe"),
            "`" => Some("Grave"),
            "[" => Some("BracketLeft"),
            "]" => Some("BracketRight"),
            "\\" => Some("Backslash"),
            _ => None,
        };
        if let Some(a) = alias {
            *key = a.to_owned();
        }
    }
    parts.join("+").parse()
}

/// Press a chord's modifiers, then its key, then let all go.
pub fn chord(c: &Chord) -> Vec<SyntheticEvent> {
    let mods: Vec<u32> = [(c.ctrl, CTRL), (c.alt, ALT), (c.shift, SHIFT), (c.cmd, CMD)].iter().filter(|(on, _)| *on).map(|(_, k)| *k).collect();
    let mut events: Vec<SyntheticEvent> = mods.iter().map(|&key| SyntheticEvent::Key { key, pressed: true }).collect();
    events.push(SyntheticEvent::Key { key: c.key, pressed: true });
    events.push(SyntheticEvent::Key { key: c.key, pressed: false });
    events.extend(mods.iter().rev().map(|&key| SyntheticEvent::Key { key, pressed: false }));
    events
}

/// The key (and whether Shift) that types `c` on a US keyboard.
pub fn key_for(c: char) -> Option<(u32, bool)> {
    const ROWS: [(&str, &str, u32); 4] = [("`1234567890-=", "~!@#$%^&*()_+", 41), ("qwertyuiop[]\\", "QWERTYUIOP{}|", 16), ("asdfghjkl;'", "ASDFGHJKL:\"", 30), ("zxcvbnm,./", "ZXCVBNM<>?", 44)];
    match c {
        ' ' => return Some((57, false)),
        '\n' => return Some((28, false)),
        '\t' => return Some((15, false)),
        _ => {}
    }
    for (plain, shifted, first) in ROWS {
        // The number row starts with ` (41), then 1 (2) onward.
        let code = |i: usize| {
            if first == 41 {
                if i == 0 { 41 } else { 1 + i as u32 }
            } else if first == 16 && i == 12 {
                43
            } else {
                first + i as u32
            }
        };
        if let Some(i) = plain.chars().position(|p| p == c) {
            return Some((code(i), false));
        }
        if let Some(i) = shifted.chars().position(|p| p == c) {
            return Some((code(i), true));
        }
    }
    None
}

/// `text` in runs: what a US keyboard types (true), and what it doesn't.
pub fn runs(text: &str) -> Vec<(bool, &str)> {
    let mut out: Vec<(bool, &str)> = Vec::new();
    let mut start = 0;
    let mut kind = None;
    for (i, c) in text.char_indices() {
        let keys = key_for(c).is_some();
        if kind.is_some_and(|k| k != keys) {
            out.push((kind.unwrap_or(true), &text[start..i]));
            start = i;
        }
        kind = Some(keys);
    }
    if let Some(k) = kind {
        out.push((k, &text[start..]));
    }
    out
}

/// Type `text` (what a US keyboard can type).
pub fn type_text(text: &str) -> Result<Vec<SyntheticEvent>, String> {
    let mut events = Vec::new();
    for c in text.chars() {
        let (key, shift) = key_for(c).ok_or_else(|| format!("{c:?} can't be typed (only what a US keyboard types)"))?;
        if shift {
            events.push(SyntheticEvent::Key { key: SHIFT, pressed: true });
        }
        events.push(SyntheticEvent::Key { key, pressed: true });
        events.push(SyntheticEvent::Key { key, pressed: false });
        if shift {
            events.push(SyntheticEvent::Key { key: SHIFT, pressed: false });
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {

    /// A screen's session is known by the `OLO_SCREEN` it was started
    /// with, not its command line, which an upgrade's re-execution left
    /// bare (every screen then looked ended and was forgotten).
    #[test]
    fn a_screens_session_is_known_by_its_environment() {
        let env = b"HOME=/home/me\0OLO_SCREEN=claude-browser\0WAYLAND_DISPLAY=\0";
        // Started, and re-executed bare after an upgrade: both its.
        assert!(is_session_of(b"/home/me/.local/bin/olo-session\0--headless\01440x900\0", env, "claude-browser"));
        assert!(is_session_of(b"/home/me/.local/bin/olo-session\0", env, "claude-browser"));
        // Another screen's, the person's own session, another program.
        assert!(!is_session_of(b"/home/me/.local/bin/olo-session\0", env, "claude"));
        assert!(!is_session_of(b"/home/me/.local/bin/olo-session\0", b"HOME=/home/me\0", "claude-browser"));
        assert!(!is_session_of(b"/usr/bin/bash\0", env, "claude-browser"));
    }

    /// When a process started, past a name with spaces and parentheses.
    #[test]
    fn a_process_start_from_its_stat() {
        let stat = "4102 (olo (x) session) S 4061 4102 4102 0 -1 4194560 1 2 3 4 5 6 7 8 20 0 12 0 987654 1000 200 18446744073709551615";
        assert_eq!(started_ticks(stat), Some(987654));
    }

    use super::*;

    /// Chrome on a screen keeps a profile of the screen's own, so it
    /// runs there rather than handing its window to the person's Chrome.
    #[test]
    fn an_agent_waits_while_the_person_controls() {
        let dir = std::env::temp_dir().join(format!("olo-screen-control-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!controlled_in(&dir, "agent"));
        set_controlled_in(&dir, "agent", true);
        assert!(controlled_in(&dir, "agent") && !controlled_in(&dir, "agent-2"));
        set_controlled_in(&dir, "agent", false);
        assert!(!controlled_in(&dir, "agent"));
        // A viewer that has gone (another program on that pid, or none)
        // controls nothing.
        std::fs::write(control_file(&dir, "agent"), "1").unwrap();
        assert!(!controlled_in(&dir, "agent"));
        // Started again (an upgrade's exec: the same pid), it forgets
        // what it said before.
        set_controlled_in(&dir, "agent", true);
        clear_controlled_in(&dir);
        assert!(!controlled_in(&dir, "agent"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chrome_has_a_profile_of_its_own() {
        let s = Screen { name: "work".into(), wayland_display: "wayland-9".into(), olo_socket: "/x".into(), pid: 1, width: 1, height: 1, scale: 1.0, sound: false, started: 0, bus: None, a11y: None };
        let env: std::collections::HashMap<String, String> = s.env().into_iter().collect();
        let home = &env["CHROME_CONFIG_HOME"];
        assert!(home.ends_with("/screens/work"), "{home}");
        assert_ne!(Some(home.as_str()), std::env::var("CHROME_CONFIG_HOME").ok().as_deref());
    }

    #[test]
    fn keys_for_text() {
        assert_eq!(key_for('a'), Some((30, false)));
        assert_eq!(key_for('A'), Some((30, true)));
        assert_eq!(key_for('1'), Some((2, false)));
        assert_eq!(key_for('0'), Some((11, false)));
        assert_eq!(key_for('='), Some((13, false)));
        assert_eq!(key_for('`'), Some((41, false)));
        assert_eq!(key_for('~'), Some((41, true)));
        assert_eq!(key_for('@'), Some((3, true)));
        assert_eq!(key_for('\\'), Some((43, false)));
        assert_eq!(key_for('|'), Some((43, true)));
        assert_eq!(key_for('/'), Some((53, false)));
        assert_eq!(key_for('"'), Some((40, true)));
        assert_eq!(key_for('é'), None);
        assert!(type_text("Hi!").unwrap().len() == 10);
        assert_eq!(runs("café 👍🏽 ok\n"), vec![(true, "caf"), (false, "é"), (true, " "), (false, "👍🏽"), (true, " ok\n")]);
        assert!(runs("").is_empty());
    }

    #[test]
    fn chords() {
        let c = parse_chord("cmd+shift+t").unwrap();
        assert!(c.cmd && c.shift && !c.ctrl && c.key == 20);
        assert_eq!(parse_chord("Enter").unwrap().key, 28);
        assert_eq!(parse_chord("esc").unwrap().key, 1);
        assert_eq!(parse_chord("ctrl+/").unwrap().key, 53);
        assert!(parse_chord("hyper+x").is_err());
        let events = chord(&parse_chord("Cmd+W").unwrap());
        assert_eq!(events.first(), Some(&SyntheticEvent::Key { key: CMD, pressed: true }));
        assert_eq!(events.last(), Some(&SyntheticEvent::Key { key: CMD, pressed: false }));
    }

    #[test]
    fn paced_input() {
        // A scroll in finger-sized steps, resting before the lift.
        let s = scroll(0.0, -300.0, true);
        assert_eq!(s.len(), 16);
        assert!(s[..15].iter().all(|(e, _)| e == &vec![SyntheticEvent::Scroll { dx: 0.0, dy: -20.0, at_us: None }]));
        assert_eq!(s[14].1, Duration::from_millis(80));
        assert_eq!(s[15].0, vec![SyntheticEvent::Scroll { dx: 0.0, dy: 0.0, at_us: None }]);
        assert_eq!(scroll(0.0, 30.0, false).len(), 2);
        // A notch at a time, both ways at once.
        let w = wheel(1, -3);
        assert_eq!(
            w.iter().map(|(e, _)| e[0].clone()).collect::<Vec<_>>(),
            vec![SyntheticEvent::Wheel { dx: 1, dy: -1 }, SyntheticEvent::Wheel { dx: 0, dy: -1 }, SyntheticEvent::Wheel { dx: 0, dy: -1 }]
        );
        // Press, the moves, ending where asked, then let go.
        let d = drag((0.0, 0.0), (100.0, 50.0), 4, Duration::from_millis(200));
        assert_eq!(d.len(), 6);
        assert_eq!(d[4].0, vec![SyntheticEvent::Move { x: 100.0, y: 50.0 }]);
        assert_eq!(d[5].0, vec![SyntheticEvent::Button { button: LEFT, pressed: false }]);
    }

    #[test]
    fn names_and_sockets() {
        assert!(valid_name("agent-2") && valid_name("claude_1"));
        assert!(!valid_name("") && !valid_name("a/b") && !valid_name("x y"));
        assert_eq!(sockets("session: WAYLAND_DISPLAY=wayland-4 OLO_SOCKET=/run/user/1000/olo-wayland-4.sock"), Some(("wayland-4".into(), "/run/user/1000/olo-wayland-4.sock".into())));
        assert_eq!(sockets("olo-session: hello"), None);
    }

    #[test]
    fn settings_never_lock_or_sleep() {
        let mut person = Settings::default();
        person.power.sleep_minutes = 15;
        person.screen_saver.enabled = true;
        person.display.scale = 2.0;
        let s = screen_settings(Some(person), 1.0);
        assert_eq!((s.power.sleep_minutes, s.power.display_off_minutes, s.screen_saver.enabled, s.display.scale), (0, 0, false, 1.0));
    }
}
