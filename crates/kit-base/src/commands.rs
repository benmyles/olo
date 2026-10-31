//! Every kit app's menus, for the command palette (apps/palette): each app
//! listens on a socket named after its process id, answering with the
//! menu bar of its last active window, flattened, and choosing a command
//! from it as if from the menu. app_server tells the palette which process
//! a window belongs to (`WindowInfo::pid`).
//!
//! Newline-delimited JSON, one request and one answer per connection:
//! `{"menus":true}` → [`Menus`]; `{"invoke":"cmd"}` → `{"ok":bool}`;
//! `{"tree":true}` → a [`WindowTree`] per window: what each shows, as
//! an accessibility tree, made for one frame on demand (so agents read
//! the widgets, their text and places rather than pixels);
//! `{"hints":[ID…]}` → a [`WindowTree`] for each of the surfaces named
//! (their `wl_surface` ids), only what can be picked and what holds it,
//! as soon as those have drawn (app_server's keyboard hints, which must
//! not wait for windows nobody can see);
//! `{"focus":ID}` → `{"ok":true}`: give the keyboard to the widget with
//! that tree id (a slider or text field, which a click would change);
//! `{"frames":true}` → a [`FrameTimes`] per window (and open menu; a
//! closed menu's go to its window), since the last ask
//! (answered by the UI thread, so how long the answer takes is how long
//! the app keeps people waiting; tools/dev/perf-fuzz);
//! `{"review":true}` → a [`WindowReview`] per window: the layout review
//! (`layout_review`) of the frame each draws for it, at its scale;
//! `{"path":ID}` → `{"path":"/…"}` (or null): the file or folder the
//! window with that surface shows (`Cx::set_path`), for its
//! tab's menu (Open in Tracker);
//! `{"lose_gpu":true}` → `{"ok":bool}`: debug builds only, destroy the
//! GPU device, as a driver reset loses it, to test that the app comes
//! back (the kit's `platform::recover_gpu`);
//! `{"window_over":{"window":ID,"onto":ID|null,"x":X,"y":Y}}` and
//! `{"window_dropped":{…}}` → `{"ok":true}`: from app_server, a window of
//! ours moved over another of ours (surface ids, `onto`'s coordinates),
//! and let go there (`App::window_dropped`).

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

use serde::{Deserialize, Serialize};

/// One menu item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The menus it is in ("File", "Open Recent").
    pub path: Vec<String>,
    pub label: String,
    pub command: String,
    /// Its keys as the menu shows them ("⌘⇧N"), if any.
    #[serde(default)]
    pub keys: Option<String>,
    pub enabled: bool,
    #[serde(default)]
    pub checked: bool,
}

/// An app's menu bar.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Menus {
    pub app_id: String,
    pub entries: Vec<Entry>,
}

/// One thing in a window: a widget, or text drawn there.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiNode {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    /// AccessKit's role ("Button", "CheckBox", "TextInput", "Label", …);
    /// "Clickable" for an area that takes clicks but says nothing more
    /// (an icon, a list row, a menu title), named by the text inside it.
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Where it is in the window's content, in logical pixels.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub enabled: bool,
    /// It takes clicks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clickable: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    /// Checked (check boxes, radio buttons), if it can be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// A table row's index (from 0), and how many rows the table has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
    /// A menu item standing for a minimized window (`menu::Action::minimized`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub minimized: bool,
}

/// What one window shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowTree {
    pub title: String,
    /// The content's logical size.
    pub width: u32,
    pub height: u32,
    pub active: bool,
    /// A layer surface's namespace ("deskbar"); None for a window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// Its `wl_surface`'s protocol id, when asked for by it (`hints`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<u32>,
    /// The file or folder it shows (`Cx::set_path`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// In tree order, a node's children after it.
    pub nodes: Vec<UiNode>,
}

impl WindowTree {
    /// Only what takes clicks, and what holds it (the boxes hints group
    /// by): a window's text, which can be thousands of nodes, goes.
    pub fn pickable(mut self) -> WindowTree {
        use std::collections::HashMap;
        let index: HashMap<u64, usize> = self.nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        let mut keep: Vec<bool> = self.nodes.iter().map(|n| n.clickable).collect();
        // Children come after their parents: from the end, each kept one
        // keeps its parent.
        for i in (0..self.nodes.len()).rev() {
            if keep[i]
                && let Some(&p) = self.nodes[i].parent.and_then(|p| index.get(&p))
            {
                keep[p] = true;
            }
        }
        let mut keep = keep.into_iter();
        self.nodes.retain(|_| keep.next().unwrap_or(false));
        self
    }
}

/// How one window kept up since the last ask: frames drawn, and the
/// slowest and total time drawing them (microseconds, from taking its
/// input to handing the frame to the GPU), and what draws them: the
/// adapter's name, API and kind ("Apple M2 (Vulkan, IntegratedGpu)"; a
/// `Cpu` one is software rendering, which should never happen).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameTimes {
    pub title: String,
    pub frames: u32,
    pub slowest_us: u64,
    pub total_us: u64,
    #[serde(default)]
    pub adapter: String,
    /// Each frame drawn, the last [`MARKS`] of them: when, and where its
    /// scroll views were, so a scroll's smoothness can be measured
    /// (tools/dev/scroll-check).
    #[serde(default)]
    pub marks: Vec<FrameMark>,
}

/// How many [`FrameMark`]s a window keeps between asks.
pub const MARKS: usize = 1200;

/// One frame a window drew.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameMark {
    /// When it took its input, in CLOCK_MONOTONIC microseconds (the
    /// clock app_server and input events go by).
    pub at_us: u64,
    /// How long it took to draw, to handing it to the GPU.
    pub us: u32,
    /// Of that, egui and the app's own code.
    #[serde(default)]
    pub ui_us: u32,
    /// Setting the GPU surface up (made, or its swapchain configured
    /// for a new size).
    #[serde(default)]
    pub setup_us: u32,
    /// Turning egui's shapes into triangles.
    #[serde(default)]
    pub tessellate_us: u32,
    /// Waiting for a buffer to draw in (the compositor still showing
    /// the others).
    #[serde(default)]
    pub acquire_us: u32,
    /// Uploading textures and triangles, recording, submitting and
    /// presenting.
    #[serde(default)]
    pub submit_us: u32,
    /// The offset each scroll view in it showed (content x, y at its
    /// top left) and the furthest it scrolls to (x, y), in the order
    /// they were drawn: an offset past either end is the content pulled
    /// past its edge.
    pub scrolls: Vec<[f32; 4]>,
}

/// What the layout review (the kit's `layout_review`) found in one
/// window's frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowReview {
    pub title: String,
    /// The content's logical size, and the scale it was drawn at.
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// A layer surface's namespace ("deskbar"); None for a window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    pub findings: Vec<crate::review::Finding>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ask {
    Menus(bool),
    Invoke(String),
    Tree(bool),
    Hints(Vec<u32>),
    Focus(u64),
    Frames(bool),
    Review(bool),
    Path(u32),
    LoseGpu(bool),
    WindowOver(WindowAt),
    WindowDropped(WindowAt),
}

/// app_server's word that a window of ours being moved is over another
/// of ours (`onto`, its surface; None: no longer), at (`x`, `y`) in it, or
/// was let go there (`window_dropped`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowAt {
    pub window: u32,
    pub onto: Option<u32>,
    pub x: f64,
    pub y: f64,
}

/// A question for the UI thread, answered through `reply`.
pub enum Query {
    Menus(mpsc::Sender<Menus>),
    Invoke(String, mpsc::Sender<bool>),
    Tree(mpsc::Sender<Vec<WindowTree>>),
    /// The trees of the surfaces with these protocol ids, pickable only.
    Hints(Vec<u32>, mpsc::Sender<Vec<WindowTree>>),
    Focus(u64),
    Frames(mpsc::Sender<Vec<FrameTimes>>),
    Review(mpsc::Sender<Vec<WindowReview>>),
    /// The path of the window with the surface with this protocol id.
    Path(u32, mpsc::Sender<Option<PathBuf>>),
    /// Destroy the GPU device (debug builds; a test of losing it).
    LoseGpu(mpsc::Sender<bool>),
    WindowOver(WindowAt),
    WindowDropped(WindowAt),
}

/// How long a tree may take: every window draws a frame for it.
const TREE_WAIT: Duration = Duration::from_millis(1500);

/// How long the app has to answer (it may be busy drawing).
const WAIT: Duration = Duration::from_millis(400);

fn socket_path(pid: u32) -> PathBuf {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    rt.join(format!("olo-menus-{pid}.sock"))
}

/// The socket's owner: removes it when the app ends.
pub struct Listening(PathBuf);

impl Drop for Listening {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The program at the other end of `s`, as "name (pid)".
fn peer(s: &UnixStream) -> String {
    use std::os::fd::AsRawFd;
    // SAFETY: an all-zero ucred is valid, and getsockopt writes at most
    // `len` bytes into it.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let got = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len) };
    if got != 0 {
        return "unknown".into();
    }
    let name = std::fs::read_to_string(format!("/proc/{}/comm", cred.pid)).unwrap_or_default();
    format!("{} ({})", name.trim(), cred.pid)
}

/// Remove the sockets of apps that died without cleaning up.
fn sweep(ours: &std::path::Path) {
    let Some(dir) = ours.parent() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix("olo-menus-")).and_then(|n| n.strip_suffix(".sock")) else { continue };
        if pid.parse::<u32>().is_ok() && !std::path::Path::new("/proc").join(pid).exists() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Answer on our socket; questions reach the UI thread through `to`.
pub fn listen(to: impl Fn(Query) + Send + 'static) -> Option<Listening> {
    let path = socket_path(std::process::id());
    let _ = std::fs::remove_file(&path);
    sweep(&path);
    let listener = UnixListener::bind(&path).ok()?;
    std::thread::Builder::new()
        .name("menus".into())
        .spawn(move || {
            for s in listener.incoming().flatten() {
                let _ = s.set_read_timeout(Some(WAIT));
                let mut line = String::new();
                let mut reader = BufReader::new(&s);
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let answer = match serde_json::from_str::<Ask>(&line) {
                    Ok(Ask::Menus(_)) => {
                        let (tx, rx) = mpsc::channel();
                        to(Query::Menus(tx));
                        serde_json::to_string(&rx.recv_timeout(WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Invoke(cmd)) => {
                        // It acts as if chosen from the menu: say who chose it.
                        tracing::info!(command = %cmd, by = %peer(&s), "asked to choose a menu command");
                        let (tx, rx) = mpsc::channel();
                        to(Query::Invoke(cmd, tx));
                        serde_json::to_string(&serde_json::json!({ "ok": rx.recv_timeout(WAIT).unwrap_or(false) }))
                    }
                    Ok(Ask::Focus(id)) => {
                        to(Query::Focus(id));
                        serde_json::to_string(&serde_json::json!({ "ok": true }))
                    }
                    Ok(Ask::Tree(_)) => {
                        let (tx, rx) = mpsc::channel();
                        to(Query::Tree(tx));
                        serde_json::to_string(&rx.recv_timeout(TREE_WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Hints(surfaces)) => {
                        let (tx, rx) = mpsc::channel();
                        to(Query::Hints(surfaces, tx));
                        serde_json::to_string(&rx.recv_timeout(TREE_WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Frames(_)) => {
                        let (tx, rx) = mpsc::channel();
                        to(Query::Frames(tx));
                        serde_json::to_string(&rx.recv_timeout(TREE_WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Review(_)) => {
                        let (tx, rx) = mpsc::channel();
                        to(Query::Review(tx));
                        serde_json::to_string(&rx.recv_timeout(TREE_WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Path(surface)) => {
                        let (tx, rx) = mpsc::channel();
                        to(Query::Path(surface, tx));
                        serde_json::to_string(&serde_json::json!({ "path": rx.recv_timeout(WAIT).ok().flatten() }))
                    }
                    Ok(Ask::WindowOver(at)) => {
                        to(Query::WindowOver(at));
                        serde_json::to_string(&serde_json::json!({ "ok": true }))
                    }
                    Ok(Ask::WindowDropped(at)) => {
                        to(Query::WindowDropped(at));
                        serde_json::to_string(&serde_json::json!({ "ok": true }))
                    }
                    Ok(Ask::LoseGpu(_)) => {
                        tracing::warn!(by = %peer(&s), "asked to lose the GPU device");
                        let (tx, rx) = mpsc::channel();
                        to(Query::LoseGpu(tx));
                        serde_json::to_string(&serde_json::json!({ "ok": rx.recv_timeout(WAIT).unwrap_or(false) }))
                    }
                    Err(_) => continue,
                };
                if let Ok(answer) = answer {
                    let _ = (&s).write_all(answer.as_bytes());
                    let _ = (&s).write_all(b"\n");
                }
            }
        })
        .ok()?;
    Some(Listening(path))
}

/// Answer `tree` for process `pid`, a program that isn't a kit app (the
/// Olo extension's host does it for Chrome): `trees` is called on the
/// listening thread for each question and may take up to `TREE_WAIT`.
/// Its menus are empty and nothing can be invoked. Runs until the
/// process ends; the socket is left for the next sweep.
pub fn serve_trees(pid: u32, app_id: &str, trees: impl Fn() -> Vec<WindowTree> + Send + 'static) -> std::io::Result<()> {
    let path = socket_path(pid);
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    let app_id = app_id.to_owned();
    std::thread::Builder::new().name("trees".into()).spawn(move || {
        for s in listener.incoming().flatten() {
            let _ = s.set_read_timeout(Some(WAIT));
            let mut line = String::new();
            if BufReader::new(&s).read_line(&mut line).is_err() {
                continue;
            }
            let answer = match serde_json::from_str::<Ask>(&line) {
                Ok(Ask::Menus(_)) => serde_json::to_string(&Menus { app_id: app_id.clone(), entries: Vec::new() }),
                Ok(Ask::Invoke(_) | Ask::Focus(_) | Ask::LoseGpu(_) | Ask::WindowOver(_) | Ask::WindowDropped(_)) => serde_json::to_string(&serde_json::json!({ "ok": false })),
                Ok(Ask::Frames(_)) => serde_json::to_string(&Vec::<FrameTimes>::new()),
                Ok(Ask::Review(_)) => serde_json::to_string(&Vec::<WindowReview>::new()),
                Ok(Ask::Path(_)) => serde_json::to_string(&serde_json::json!({ "path": null })),
                // It can't tell its surfaces apart: all of them, matched
                // by title and size.
                Ok(Ask::Tree(_) | Ask::Hints(_)) => serde_json::to_string(&trees()),
                Err(_) => continue,
            };
            if let Ok(answer) = answer {
                let _ = (&s).write_all(answer.as_bytes());
                let _ = (&s).write_all(b"\n");
            }
        }
    })?;
    Ok(())
}

/// How long [`serve_trees`]' `trees` may take.
pub const TREE_WAIT_FOR: Duration = TREE_WAIT;

fn ask(pid: u32, ask: &Ask) -> Option<String> {
    let s = UnixStream::connect(socket_path(pid)).ok()?;
    s.set_read_timeout(Some(if matches!(ask, Ask::Tree(_) | Ask::Review(_)) { TREE_WAIT + WAIT } else { WAIT * 2 })).ok()?;
    s.set_write_timeout(Some(WAIT)).ok()?;
    let mut line = serde_json::to_string(ask).ok()?;
    line.push('\n');
    (&s).write_all(line.as_bytes()).ok()?;
    let mut answer = String::new();
    BufReader::new(&s).read_line(&mut answer).ok()?;
    Some(answer)
}

/// The menus of the kit app with process id `pid` (None if it isn't one,
/// or doesn't answer in time). Blocks briefly: call it off the UI thread.
pub fn menus(pid: u32) -> Option<Menus> {
    serde_json::from_str(&ask(pid, &Ask::Menus(true))?).ok()
}

/// Choose `command` from the menus of the app with process id `pid`, in
/// its last active window. Blocks briefly: call it off the UI thread.
pub fn invoke(pid: u32, command: &str) -> bool {
    ask(pid, &Ask::Invoke(command.to_owned()))
        .and_then(|a| serde_json::from_str::<serde_json::Value>(&a).ok())
        .is_some_and(|v| v["ok"] == true)
}

/// Give the keyboard to the widget with tree id `node` in the kit app
/// with process id `pid`. Blocks briefly: call it off the UI thread.
pub fn focus(pid: u32, node: u64) -> bool {
    ask(pid, &Ask::Focus(node))
        .and_then(|a| serde_json::from_str::<serde_json::Value>(&a).ok())
        .is_some_and(|v| v["ok"] == true)
}

/// What the windows of the kit app with process id `pid` show (None if
/// it isn't one, or doesn't answer in time). Blocks for up to two
/// seconds: call it off the UI thread.
pub fn tree(pid: u32) -> Option<Vec<WindowTree>> {
    serde_json::from_str(&ask(pid, &Ask::Tree(true))?).ok()
}

/// The file or folder the window with surface `surface` (its protocol
/// id, [`olo_protocol::WindowInfo::surface`]) of the kit app with process
/// id `pid` shows, if it says (`Cx::set_path`). Blocks briefly:
/// call it off the UI thread.
pub fn path(pid: u32, surface: u32) -> Option<PathBuf> {
    let answer: serde_json::Value = serde_json::from_str(&ask(pid, &Ask::Path(surface))?).ok()?;
    answer["path"].as_str().map(PathBuf::from)
}

/// The layout review of each window of the kit app with process id
/// `pid` (None if it isn't one, or doesn't answer in time). Blocks for
/// up to two seconds: call it off the UI thread.
pub fn review(pid: u32) -> Option<Vec<WindowReview>> {
    serde_json::from_str(&ask(pid, &Ask::Review(true))?).ok()
}
