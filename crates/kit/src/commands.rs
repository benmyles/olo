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
//! the widgets, their text and places rather than pixels).

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
}

/// What one window shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowTree {
    pub title: String,
    /// The content's logical size.
    pub width: u32,
    pub height: u32,
    pub active: bool,
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
            });
            stack.extend(n.children().iter().rev().map(|c| c.0));
        }
        // Text is drawn beside the area that takes the click, not in it:
        // each text (a label, a list cell) names the smallest unnamed one
        // its middle is inside (a key, not the whole keypad behind it; a
        // row, by its cells).
        let inside = |n: &UiNode, x: f32, y: f32| (n.x..n.x + n.width).contains(&x) && (n.y..n.y + n.height).contains(&y);
        let mut names: Vec<Vec<String>> = vec![Vec::new(); out.len()];
        for t in out.iter().filter(|t| t.role == "Label" || t.role == "Cell") {
            let Some(text) = &t.name else { continue };
            let (x, y) = (t.x + t.width / 2.0, t.y + t.height / 2.0);
            let smallest = out
                .iter()
                .enumerate()
                .filter(|(_, n)| n.clickable && n.name.is_none() && inside(n, x, y))
                .min_by(|(_, a), (_, b)| (a.width * a.height).total_cmp(&(b.width * b.height)));
            if let Some((i, _)) = smallest {
                names[i].push(text.clone());
            }
        }
        for (n, words) in out.iter_mut().zip(names) {
            if !words.is_empty() {
                n.name = Some(words.join(" "));
            }
        }
        WindowTree { title: title.to_owned(), width: size.0, height: size.1, active, nodes: out }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Ask {
    Menus(bool),
    Invoke(String),
    Tree(bool),
}

/// A question for the UI thread, answered through `reply`.
pub(crate) enum Query {
    Menus(mpsc::Sender<Menus>),
    Invoke(String, mpsc::Sender<bool>),
    Tree(mpsc::Sender<Vec<WindowTree>>),
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
                    keys: a.shortcut.map(|s| format!("⌘{}", s.label())),
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
                        let (tx, rx) = mpsc::channel();
                        let _ = to.send(Query::Invoke(cmd, tx));
                        serde_json::to_string(&serde_json::json!({ "ok": rx.recv_timeout(WAIT).unwrap_or(false) }))
                    }
                    Ok(Ask::Tree(_)) => {
                        let (tx, rx) = mpsc::channel();
                        let _ = to.send(Query::Tree(tx));
                        serde_json::to_string(&rx.recv_timeout(TREE_WAIT).unwrap_or_default())
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
                Ok(Ask::Invoke(_)) => serde_json::to_string(&serde_json::json!({ "ok": false })),
                Ok(Ask::Tree(_)) => serde_json::to_string(&trees()),
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
    s.set_read_timeout(Some(if matches!(ask, Ask::Tree(_)) { TREE_WAIT + WAIT } else { WAIT * 2 })).ok()?;
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

/// What the windows of the kit app with process id `pid` show (None if
/// it isn't one, or doesn't answer in time). Blocks for up to two
/// seconds: call it off the UI thread.
pub fn tree(pid: u32) -> Option<Vec<WindowTree>> {
    serde_json::from_str(&ask(pid, &Ask::Tree(true))?).ok()
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
