//! Every kit app's menus, for the command palette (apps/palette): each app
//! listens on a socket named after its process id, answering with the
//! menu bar of its last active window, flattened, and choosing a command
//! from it as if from the menu. app_server tells the palette which process
//! a window belongs to (`WindowInfo::pid`).
//!
//! Newline-delimited JSON, one request and one answer per connection:
//! `{"menus":true}` → [`Menus`]; `{"invoke":"cmd"}` → `{"ok":bool}`;
//! `{"tree":true}` → a [`WindowTree`] per window: what each shows, as
//! egui's AccessKit tree, made for one frame on demand (so agents read
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
//! window with that surface shows ([`crate::Cx::set_path`]), for its
//! tab's menu (Open in Tracker);
//! `{"lose_gpu":true}` → `{"ok":bool}`: debug builds only, destroy the
//! GPU device, as a driver reset loses it, to test that the app comes
//! back (`platform::recover_gpu`).

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::menu::{Menu, MenuItem};

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// A menu item standing for a minimized window ([`crate::menu::Action::minimized`]).
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
    /// The file or folder it shows ([`crate::Cx::set_path`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// In tree order, a node's children after it.
    pub nodes: Vec<UiNode>,
}

impl WindowTree {
    /// From one frame's AccessKit update (its bounds in logical pixels).
    pub fn from_update(title: &str, size: (u32, u32), active: bool, update: &egui::accesskit::TreeUpdate) -> WindowTree {
        use std::collections::HashMap;
        let nodes: HashMap<u64, &egui::accesskit::Node> = update.nodes.iter().map(|(id, n)| (id.0, n)).collect();
        let mut parents: HashMap<u64, u64> = HashMap::new();
        for (id, n) in &update.nodes {
            for c in n.children() {
                parents.insert(c.0, id.0);
            }
        }
        let root = update.tree.as_ref().map(|t| t.root.0).or_else(|| update.nodes.iter().map(|(id, _)| id.0).find(|id| !parents.contains_key(id)));
        let mut out = Vec::new();
        let mut stack: Vec<u64> = root.into_iter().collect();
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(n) = nodes.get(&id) else { continue };
            // The window itself has no bounds: it is all of the content.
            let r = n.bounds().filter(|_| Some(id) != root).unwrap_or(egui::accesskit::Rect { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 });
            let r = if Some(id) == root { egui::accesskit::Rect { x1: size.0 as f64, y1: size.1 as f64, ..r } } else { r };
            let text = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
            let clickable = n.supports_action(egui::accesskit::Action::Click);
            let picks = matches!(n.role(), egui::accesskit::Role::Row | egui::accesskit::Role::Tab | egui::accesskit::Role::ColumnHeader | egui::accesskit::Role::ListItem);
            let role = match n.role() {
                egui::accesskit::Role::Unknown if clickable => "Clickable".to_owned(),
                role => format!("{role:?}"),
            };
            out.push(UiNode {
                id,
                parent: parents.get(&id).copied(),
                role,
                name: text(n.label()),
                value: text(n.value()).or_else(|| n.numeric_value().map(|v| v.to_string())),
                x: r.x0 as f32,
                y: r.y0 as f32,
                width: (r.x1 - r.x0) as f32,
                height: (r.y1 - r.y0) as f32,
                enabled: !n.is_disabled(),
                clickable,
                focused: update.focus.0 == id && Some(id) != root,
                // egui says a row's, tab's or column header's selection
                // as "toggled": for those it is selected, not checked.
                checked: n.toggled().filter(|_| !picks).map(|t| t == egui::accesskit::Toggled::True),
                selected: n.is_selected().or_else(|| n.toggled().filter(|_| picks).map(|t| t == egui::accesskit::Toggled::True)),
                row: n.row_index(),
                rows: n.row_count(),
                minimized: false,
            });
            stack.extend(n.children().iter().rev().map(|c| c.0));
        }
        // Text is drawn beside the area that takes the click, not in it:
        // each text (a label, a list cell) names the smallest unnamed one
        // its middle is inside (a key, not the whole keypad behind it; a
        // row, by its cells).
        let inside = |n: &UiNode, x: f32, y: f32| (n.x..n.x + n.width).contains(&x) && (n.y..n.y + n.height).contains(&y);
        let mut names: Vec<Vec<String>> = vec![Vec::new(); out.len()];
        // (Few take clicks unnamed, and a window can have thousands of
        // texts: each text looks through those few, not everything.)
        let unnamed: Vec<usize> = (0..out.len()).filter(|&i| out[i].clickable && out[i].name.is_none()).collect();
        for t in out.iter().filter(|t| t.role == "Label" || t.role == "Cell") {
            let Some(text) = &t.name else { continue };
            let (x, y) = (t.x + t.width / 2.0, t.y + t.height / 2.0);
            let area = |i: &usize| out[*i].width * out[*i].height;
            let smallest = unnamed.iter().filter(|&&i| inside(&out[i], x, y)).min_by(|a, b| area(a).total_cmp(&area(b)));
            if let Some(&i) = smallest {
                names[i].push(text.clone());
            }
        }
        for (n, words) in out.iter_mut().zip(names) {
            if !words.is_empty() {
                n.name = Some(words.join(" "));
            }
        }
        WindowTree { title: title.to_owned(), width: size.0, height: size.1, active, layer: None, surface: None, path: None, nodes: out }
    }

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

/// What the layout review ([`crate::layout_review`]) found in one
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
    pub findings: Vec<crate::layout_review::Finding>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Ask {
    Menus(bool),
    Invoke(String),
    Tree(bool),
    Hints(Vec<u32>),
    Focus(u64),
    Frames(bool),
    Review(bool),
    Path(u32),
    LoseGpu(bool),
}

/// A question for the UI thread, answered through `reply`.
pub(crate) enum Query {
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
}

/// How long a tree may take: every window draws a frame for it.
const TREE_WAIT: Duration = Duration::from_millis(1500);

/// How long the app has to answer (it may be busy drawing).
const WAIT: Duration = Duration::from_millis(400);

fn socket_path(pid: u32) -> PathBuf {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    rt.join(format!("olo-menus-{pid}.sock"))
}

/// Flatten a menu bar, skipping separators.
pub fn flatten(app_id: &str, bar: &[Menu]) -> Menus {
    fn walk(menu: &Menu, path: &mut Vec<String>, out: &mut Vec<Entry>) {
        path.push(menu.title.clone());
        for item in &menu.items {
            match item {
                MenuItem::Action(a) => out.push(Entry {
                    path: path.clone(),
                    label: a.label.clone(),
                    command: a.command.clone(),
                    keys: a.key_label(),
                    enabled: a.enabled,
                    checked: a.checked,
                }),
                MenuItem::Submenu(m) => walk(m, path, out),
                MenuItem::Separator => {}
            }
        }
        path.pop();
    }
    let mut entries = Vec::new();
    for m in bar {
        walk(m, &mut Vec::new(), &mut entries);
    }
    Menus { app_id: app_id.to_owned(), entries }
}

/// The socket's owner: removes it when the app ends.
pub(crate) struct Listening(PathBuf);

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
pub(crate) fn listen(to: smithay_client_toolkit::reexports::calloop::channel::Sender<Query>) -> Option<Listening> {
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
                        let _ = to.send(Query::Menus(tx));
                        serde_json::to_string(&rx.recv_timeout(WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Invoke(cmd)) => {
                        // It acts as if chosen from the menu: say who chose it.
                        tracing::info!(command = %cmd, by = %peer(&s), "asked to choose a menu command");
                        let (tx, rx) = mpsc::channel();
                        let _ = to.send(Query::Invoke(cmd, tx));
                        serde_json::to_string(&serde_json::json!({ "ok": rx.recv_timeout(WAIT).unwrap_or(false) }))
                    }
                    Ok(Ask::Focus(id)) => {
                        let _ = to.send(Query::Focus(id));
                        serde_json::to_string(&serde_json::json!({ "ok": true }))
                    }
                    Ok(Ask::Tree(_)) => {
                        let (tx, rx) = mpsc::channel();
                        let _ = to.send(Query::Tree(tx));
                        serde_json::to_string(&rx.recv_timeout(TREE_WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Hints(surfaces)) => {
                        let (tx, rx) = mpsc::channel();
                        let _ = to.send(Query::Hints(surfaces, tx));
                        serde_json::to_string(&rx.recv_timeout(TREE_WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Frames(_)) => {
                        let (tx, rx) = mpsc::channel();
                        let _ = to.send(Query::Frames(tx));
                        serde_json::to_string(&rx.recv_timeout(TREE_WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Review(_)) => {
                        let (tx, rx) = mpsc::channel();
                        let _ = to.send(Query::Review(tx));
                        serde_json::to_string(&rx.recv_timeout(TREE_WAIT).unwrap_or_default())
                    }
                    Ok(Ask::Path(surface)) => {
                        let (tx, rx) = mpsc::channel();
                        let _ = to.send(Query::Path(surface, tx));
                        serde_json::to_string(&serde_json::json!({ "path": rx.recv_timeout(WAIT).ok().flatten() }))
                    }
                    Ok(Ask::LoseGpu(_)) => {
                        tracing::warn!(by = %peer(&s), "asked to lose the GPU device");
                        let (tx, rx) = mpsc::channel();
                        let _ = to.send(Query::LoseGpu(tx));
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
                Ok(Ask::Invoke(_) | Ask::Focus(_) | Ask::LoseGpu(_)) => serde_json::to_string(&serde_json::json!({ "ok": false })),
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
/// id `pid` shows, if it says ([`crate::Cx::set_path`]). Blocks briefly:
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Menu;

    #[test]
    fn flattens_submenus() {
        let bar = vec![
            Menu::new("File").item("New", "new", Some(egui::Key::N)).separator().submenu(Menu::new("Recent").item("a.txt", "open:a", None)),
            Menu::new("View").item("Zoom", "zoom", None).checked(true).enabled(false),
        ];
        let m = flatten("olo.test", &bar);
        assert_eq!(m.entries.len(), 3);
        assert_eq!(m.entries[0].keys.as_deref(), Some("⌘N"));
        assert_eq!(m.entries[1].path, ["File", "Recent"]);
        assert!(m.entries[2].checked && !m.entries[2].enabled);
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Menus>(&json).unwrap(), m);
        assert_eq!(serde_json::to_string(&Ask::Menus(true)).unwrap(), r#"{"menus":true}"#);
    }

    #[test]
    fn names_what_takes_clicks() {
        use egui::accesskit::{Action, Node, NodeId, Rect, Role, TreeId, TreeInfo, TreeUpdate};
        let rect = |x0, y0, x1, y1| Rect { x0, y0, x1, y1 };
        let mut root = Node::new(Role::Window);
        root.set_children(vec![NodeId(5), NodeId(2), NodeId(3), NodeId(4)]);
        // Behind it all, a clickable background: the text names the smaller area.
        let mut back = Node::new(Role::Unknown);
        back.add_action(Action::Click);
        back.set_bounds(rect(0.0, 0.0, 300.0, 200.0));
        let mut area = Node::new(Role::Unknown);
        area.add_action(Action::Click);
        area.set_bounds(rect(10.0, 10.0, 110.0, 74.0));
        let mut label = Node::new(Role::Label);
        label.set_label("Appearance");
        label.set_bounds(rect(20.0, 50.0, 90.0, 64.0));
        let mut field = Node::new(Role::TextInput);
        field.set_value("hi");
        field.add_action(Action::Click);
        field.set_bounds(rect(0.0, 80.0, 200.0, 100.0));
        let update = TreeUpdate {
            nodes: vec![(NodeId(1), root), (NodeId(5), back), (NodeId(2), area), (NodeId(3), label), (NodeId(4), field)],
            tree: Some(TreeInfo::new(NodeId(1))),
            tree_id: TreeId::ROOT,
            focus: NodeId(4),
        };
        let t = WindowTree::from_update("Preferences", (300, 200), true, &update);
        let roles: Vec<&str> = t.nodes.iter().map(|n| n.role.as_str()).collect();
        assert_eq!(roles, ["Window", "Clickable", "Clickable", "Label", "TextInput"]);
        assert_eq!((t.nodes[0].width, t.nodes[0].height), (300.0, 200.0));
        assert_eq!(t.nodes[1].name, None);
        assert_eq!(t.nodes[2].name.as_deref(), Some("Appearance"));
        assert_eq!(t.nodes[2].parent, Some(1));
        assert!(t.nodes[4].focused && t.nodes[4].clickable && t.nodes[4].name.is_none());
        assert_eq!(t.nodes[4].value.as_deref(), Some("hi"));
        assert!(!t.nodes[0].focused);
    }
}
