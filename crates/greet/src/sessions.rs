//! The desktops a session may be: the login screen's entries
//! (`wayland-sessions`), Olo's first. Wayland only (no X11 sessions).

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The entry's name without `.desktop` ("olo").
    pub id: String,
    /// As shown ("Olo Desktop").
    pub name: String,
    /// What starts it.
    pub command: Vec<String>,
    /// XDG_CURRENT_DESKTOP for it (`DesktopNames`).
    pub desktop_names: Vec<String>,
}

/// The sessions there are under `root` (`/`; a test's tree): its
/// `usr/local/share` and `usr/share` entries, the first of each id, by
/// name, Olo's first.
pub fn all(root: &Path) -> Vec<Session> {
    let mut out: Vec<Session> = Vec::new();
    for dir in ["usr/local/share/wayland-sessions", "usr/share/wayland-sessions"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
        paths.sort();
        for path in paths {
            let Some(s) = parse(&path) else { continue };
            if !out.iter().any(|o| o.id == s.id) {
                out.push(s);
            }
        }
    }
    out.sort_by_key(|s| (s.id != "olo", s.name.to_lowercase()));
    out
}

/// A session's entry, if it's one to offer (not hidden, with an Exec).
fn parse(path: &Path) -> Option<Session> {
    let text = std::fs::read_to_string(path).ok()?;
    let (mut name, mut exec, mut names, mut hidden, mut main) = (None, None, Vec::new(), false, false);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            main = line == "[Desktop Entry]";
            continue;
        }
        let Some((k, v)) = line.split_once('=').filter(|_| main) else { continue };
        match k.trim() {
            "Name" => name = Some(v.trim().to_owned()),
            "Exec" => exec = Some(v.trim().to_owned()),
            "DesktopNames" => names = v.split(';').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            "Hidden" | "NoDisplay" => hidden |= v.trim() == "true",
            _ => {}
        }
    }
    if hidden {
        return None;
    }
    let command = split(&exec?);
    if command.is_empty() {
        return None;
    }
    let id = path.file_stem()?.to_string_lossy().into_owned();
    Some(Session { name: name.unwrap_or_else(|| id.clone()), id, command, desktop_names: names })
}

/// An Exec line's words (quotes kept together; field codes left out).
fn split(exec: &str) -> Vec<String> {
    let (mut out, mut word, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    word.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if any || !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
                any = false;
            }
            c => word.push(c),
        }
    }
    if any || !word.is_empty() {
        out.push(word);
    }
    out.retain(|w| !(w.len() == 2 && w.starts_with('%')));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktops_olo_first() {
        let root = std::path::PathBuf::from(format!("/var/tmp/olo-greet-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = |p: &str, c: &str| {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        };
        w("usr/share/wayland-sessions/gnome.desktop", "[Desktop Entry]\nName=GNOME\nExec=/usr/bin/gnome-session\nDesktopNames=GNOME\n");
        w("usr/share/wayland-sessions/olo.desktop", "[Desktop Entry]\nName=Olo Desktop\nExec=\"/home/ben/.local/bin/olo-session\" --tty-udev %f\nDesktopNames=Olo;GNOME;\n");
        w("usr/share/wayland-sessions/hidden.desktop", "[Desktop Entry]\nName=Old\nExec=old\nHidden=true\n");
        w("usr/local/share/wayland-sessions/gnome.desktop", "[Desktop Entry]\nName=GNOME (local)\nExec=gnome-session\n");
        let s = all(&root);
        let got: Vec<(&str, &str)> = s.iter().map(|s| (s.id.as_str(), s.name.as_str())).collect();
        assert_eq!(got, [("olo", "Olo Desktop"), ("gnome", "GNOME (local)")]);
        assert_eq!(s[0].command, ["/home/ben/.local/bin/olo-session", "--tty-udev"]);
        assert_eq!(s[0].desktop_names, ["Olo", "GNOME"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
