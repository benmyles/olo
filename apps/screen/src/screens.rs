//! Agent screens: each one a headless Tabula session of its own (its own
//! app_server, Deskbar and desktop, windows, pointer and keyboard focus),
//! so what an agent does there never moves the person's pointer or takes
//! their focus. The person watches (and steps in) through the viewer.
//!
//! A screen is `tabula-session --headless WxH`, started in its own process
//! group with settings of its own (`TABULA_APP_SERVER_SETTINGS`: the
//! person's, at 1× unless asked, never locking, blanking or sleeping) and
//! silent unless asked. It is listed in `$XDG_RUNTIME_DIR/tabula-screens/
//! NAME.json`; everything else talks to its app_server's IPC socket.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tabula_protocol::{Chord, Client, ImageFormat, Reply, Request, Settings, SyntheticEvent, WindowInfo};

/// evdev button codes.
pub const LEFT: u32 = 0x110;
pub const RIGHT: u32 = 0x111;
pub const MIDDLE: u32 = 0x112;

/// One agent screen, as listed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Screen {
    pub name: String,
    pub wayland_display: String,
    pub tabula_socket: PathBuf,
    /// tabula-session's pid (the leader of the screen's process group).
    pub pid: i32,
    /// Logical size.
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub sound: bool,
    /// Seconds since the epoch.
    pub started: u64,
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// Where screens are listed (and their logs and settings kept).
pub fn dir() -> PathBuf {
    runtime_dir().join("tabula-screens")
}

fn entry(name: &str) -> PathBuf {
    dir().join(format!("{name}.json"))
}

/// A name a screen can have: letters, digits, `-` and `_`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Screen {
    /// Still running: its session is alive and is the one listed.
    pub fn alive(&self) -> bool {
        let cmdline = std::fs::read(format!("/proc/{}/cmdline", self.pid)).unwrap_or_default();
        let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
        let session = args.first().is_some_and(|a| a.ends_with(b"tabula-session"));
        session && args.iter().any(|a| *a == b"--headless") && self.tabula_socket.exists()
    }

    fn client(&self) -> Result<Client, String> {
        Client::connect_to(&self.tabula_socket).map_err(|e| format!("screen {} isn't answering: {e}", self.name))
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
        let request = Request::Screenshot { path: path.display().to_string(), output: None, window, border: true, pointer, format };
        self.request(&request).map(drop)
    }

    /// The environment a program on this screen gets: its display and
    /// app_server, no X display (X programs would show on the person's
    /// screen), and silence unless the screen has sound.
    pub fn env(&self) -> Vec<(String, String)> {
        let mut env = vec![
            ("WAYLAND_DISPLAY".to_owned(), self.wayland_display.clone()),
            ("TABULA_SOCKET".to_owned(), self.tabula_socket.display().to_string()),
            ("TABULA_SESSION".to_owned(), "1".to_owned()),
            ("TABULA_SESSION_PID".to_owned(), self.pid.to_string()),
            ("TABULA_SCREEN".to_owned(), self.name.clone()),
        ];
        if !self.sound {
            env.push(("PULSE_SERVER".to_owned(), format!("unix:{}", dir().join("no-sound").display())));
        }
        env
    }

    /// Start `argv` on this screen (a bare name is looked for as Tabula
    /// finds programs); its pid.
    pub fn run(&self, argv: &[String]) -> Result<u32, String> {
        let program = argv.first().ok_or("no program to run")?;
        let path = if program.contains('/') {
            PathBuf::from(program)
        } else {
            tabula_xdg::find_program(program).ok_or_else(|| format!("{program} isn't installed"))?
        };
        let mut cmd = Command::new(path);
        cmd.args(&argv[1..]).env_remove("DISPLAY").stdin(Stdio::null());
        for (k, v) in self.env() {
            cmd.env(k, v);
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
        if screen.alive() {
            screens.push(screen);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    screens.sort_by(|a, b| a.name.cmp(&b.name));
    screens
}

pub fn get(name: &str) -> Result<Screen, String> {
    list().into_iter().find(|s| s.name == name).ok_or_else(|| format!("there's no screen {name:?} (tabula-screen list)"))
}

/// The first free "agent", "agent-2", ….
pub fn free_name() -> String {
    let taken: Vec<String> = list().into_iter().map(|s| s.name).collect();
    (1..).map(|n| if n == 1 { "agent".to_owned() } else { format!("agent-{n}") }).find(|n| !taken.contains(n)).unwrap_or_default()
}

/// What to start a screen with.
#[derive(Debug, Clone)]
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

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("tabula").join("app_server.json")
}

/// The person's settings, made right for a screen nobody sits at: its
/// own scale, one display, and nothing that locks, blanks or sleeps.
pub fn screen_settings(person: Option<Settings>, scale: f64) -> Settings {
    let mut s = person.unwrap_or_default();
    s.display = tabula_protocol::DisplaySettings { scale, ..Default::default() };
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

    let session = tabula_xdg::find_program("tabula-session").ok_or("tabula-session isn't installed")?;
    let log_path = dir.join(format!("{name}.log"));
    let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
    // Pixels, as the backend takes them.
    let (pw, ph) = ((opts.width as f64 * opts.scale).round() as i32, (opts.height as f64 * opts.scale).round() as i32);
    let mut cmd = Command::new(session);
    cmd.arg("--headless")
        .arg(format!("{pw}x{ph}"))
        .env("TABULA_APP_SERVER_SETTINGS", &settings_path)
        .env("TABULA_SCREEN", name)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("TABULA_SOCKET")
        .env_remove("TABULA_SESSION")
        .env_remove("TABULA_SESSION_PID")
        .env_remove("TABULA_REAL_SESSION")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    if !opts.sound {
        cmd.env("PULSE_SERVER", format!("unix:{}", dir.join("no-sound").display()));
    }
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
        tabula_socket: socket,
        pid,
        width: opts.width,
        height: opts.height,
        scale: opts.scale,
        sound: opts.sound,
        started: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
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
    let tmp = entry(name).with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&screen).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, entry(name)).map_err(|e| e.to_string())?;
    Ok(screen)
}

/// "session: WAYLAND_DISPLAY=wayland-4 TABULA_SOCKET=/run/…" → its parts.
fn sockets(line: &str) -> Option<(String, PathBuf)> {
    let rest = line.strip_prefix("session: WAYLAND_DISPLAY=")?;
    let (display, socket) = rest.split_once(" TABULA_SOCKET=")?;
    Some((display.to_owned(), PathBuf::from(socket.trim())))
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
    stop_pid(screen.pid);
    let until = Instant::now() + Duration::from_secs(5);
    while screen.alive() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    // Programs `run` started are in process groups of their own, tied to
    // the screen's display: they end when it goes (Wayland clients exit
    // when their compositor does).
    let _ = std::fs::remove_file(entry(name));
    let _ = std::fs::remove_file(dir().join(format!("{name}-settings.json")));
    if screen.alive() { Err(format!("screen {name} is still ending")) } else { Ok(()) }
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
    let mut out: Vec<_> = (0..n).map(|_| (vec![SyntheticEvent::Scroll { dx: dx / n as f64, dy: dy / n as f64 }], STEP)).collect();
    if lift {
        if let Some(last) = out.last_mut() {
            last.1 = Duration::from_millis(80);
        }
        out.push((vec![SyntheticEvent::Scroll { dx: 0.0, dy: 0.0 }], Duration::ZERO));
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
    const ROWS: [(&str, &str, u32); 4] = [
        ("`1234567890-=", "~!@#$%^&*()_+", 41),
        ("qwertyuiop[]\\", "QWERTYUIOP{}|", 16),
        ("asdfghjkl;'", "ASDFGHJKL:\"", 30),
        ("zxcvbnm,./", "ZXCVBNM<>?", 44),
    ];
    match c {
        ' ' => return Some((57, false)),
        '\n' => return Some((28, false)),
        '\t' => return Some((15, false)),
        _ => {}
    }
    for (plain, shifted, first) in ROWS {
        // The number row starts with ` (41), then 1 (2) onward.
        let code = |i: usize| if first == 41 { if i == 0 { 41 } else { 1 + i as u32 } } else if first == 16 && i == 12 { 43 } else { first + i as u32 };
        if let Some(i) = plain.chars().position(|p| p == c) {
            return Some((code(i), false));
        }
        if let Some(i) = shifted.chars().position(|p| p == c) {
            return Some((code(i), true));
        }
    }
    None
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
    use super::*;

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
        assert!(s[..15].iter().all(|(e, _)| e == &vec![SyntheticEvent::Scroll { dx: 0.0, dy: -20.0 }]));
        assert_eq!(s[14].1, Duration::from_millis(80));
        assert_eq!(s[15].0, vec![SyntheticEvent::Scroll { dx: 0.0, dy: 0.0 }]);
        assert_eq!(scroll(0.0, 30.0, false).len(), 2);
        // A notch at a time, both ways at once.
        let w = wheel(1, -3);
        assert_eq!(w.iter().map(|(e, _)| e[0].clone()).collect::<Vec<_>>(), vec![SyntheticEvent::Wheel { dx: 1, dy: -1 }, SyntheticEvent::Wheel { dx: 0, dy: -1 }, SyntheticEvent::Wheel { dx: 0, dy: -1 }]);
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
        assert_eq!(sockets("session: WAYLAND_DISPLAY=wayland-4 TABULA_SOCKET=/run/user/1000/tabula-wayland-4.sock"), Some(("wayland-4".into(), "/run/user/1000/tabula-wayland-4.sock".into())));
        assert_eq!(sockets("tabula-session: hello"), None);
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
