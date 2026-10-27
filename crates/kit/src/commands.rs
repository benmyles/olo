//! Every kit app's menus, for the command palette (apps/palette): each app
//! listens on a socket named after its process id, answering with the
//! menu bar of its last active window, flattened, and choosing a command
//! from it as if from the menu. app_server tells the palette which process
//! a window belongs to (`WindowInfo::pid`).
//!
//! Newline-delimited JSON, one request and one answer per connection:
//! `{"menus":true}` → [`Menus`]; `{"invoke":"cmd"}` → `{"ok":bool}`.

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Ask {
    Menus(bool),
    Invoke(String),
}

/// A question for the UI thread, answered through `reply`.
pub(crate) enum Query {
    Menus(mpsc::Sender<Menus>),
    Invoke(String, mpsc::Sender<bool>),
}

/// How long the app has to answer (it may be busy drawing).
const WAIT: Duration = Duration::from_millis(400);

fn socket_path(pid: u32) -> PathBuf {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    rt.join(format!("tabula-menus-{pid}.sock"))
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
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix("tabula-menus-")).and_then(|n| n.strip_suffix(".sock")) else { continue };
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

fn ask(pid: u32, ask: &Ask) -> Option<String> {
    let s = UnixStream::connect(socket_path(pid)).ok()?;
    s.set_read_timeout(Some(WAIT * 2)).ok()?;
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
        let m = flatten("tabula.test", &bar);
        assert_eq!(m.entries.len(), 3);
        assert_eq!(m.entries[0].keys.as_deref(), Some("⌘N"));
        assert_eq!(m.entries[1].path, ["File", "Recent"]);
        assert!(m.entries[2].checked && !m.entries[2].enabled);
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Menus>(&json).unwrap(), m);
        assert_eq!(serde_json::to_string(&Ask::Menus(true)).unwrap(), r#"{"menus":true}"#);
    }
}
