//! Login items (the freedesktop Autostart spec): what the person asked to
//! start when they log in (`~/.config/autostart`: Slack's and Discord's
//! "Launch on Login", what the Background portal writes for a Flatpak),
//! started once a real session's graphical side is up. A person's entry
//! overrides the system's of the same name (`Hidden=true` turns one off).
//! Entries hidden, turned off (`X-GNOME-Autostart-enabled=false`), not for
//! Olo (`OnlyShowIn` without it, `NotShowIn` with it) or whose `TryExec`
//! isn't installed don't start. The system's (`/etc/xdg/autostart`) start
//! only when they name Olo in `OnlyShowIn`: Olo says it is GNOME too, and
//! GNOME's settings daemons and the rest would fight Olo over power,
//! keys and displays.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// What an autostart entry says about itself.
#[derive(Debug, Default)]
struct Login {
    hidden: bool,
    enabled: bool,
    only: Option<Vec<String>>,
    not: Vec<String>,
    try_exec: Option<String>,
}

fn read(path: &Path) -> Option<Login> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut l = Login { enabled: true, ..Default::default() };
    let mut main = false;
    let list = |v: &str| v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase).collect::<Vec<_>>();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            main = line == "[Desktop Entry]";
            continue;
        }
        let Some((k, v)) = line.split_once('=').filter(|_| main) else { continue };
        match k.trim() {
            "Hidden" => l.hidden = v.trim() == "true",
            "X-GNOME-Autostart-enabled" => l.enabled = v.trim() != "false",
            "OnlyShowIn" => l.only = Some(list(v)),
            "NotShowIn" => l.not = list(v),
            "TryExec" => l.try_exec = Some(v.trim().to_owned()).filter(|s| !s.is_empty()),
            _ => {}
        }
    }
    Some(l)
}

/// The entries to start, most personal first: `personal` (the person's
/// folder) and `system` folders, in that order of precedence.
pub fn chosen(personal: &Path, system: &[PathBuf]) -> Vec<PathBuf> {
    // Each name once: the first folder that has it decides.
    let mut by_name: HashMap<String, (PathBuf, bool)> = HashMap::new();
    let mut order = Vec::new();
    for (dir, mine) in std::iter::once((personal, true)).chain(system.iter().map(|d| (d.as_path(), false))) {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "desktop")).collect();
        files.sort();
        for f in files {
            let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if let std::collections::hash_map::Entry::Vacant(e) = by_name.entry(name.clone()) {
                order.push(name);
                e.insert((f, mine));
            }
        }
    }
    order
        .into_iter()
        .filter_map(|name| {
            let (path, mine) = by_name.remove(&name)?;
            let l = read(&path)?;
            let for_olo = match &l.only {
                Some(only) => only.iter().any(|d| d == "olo"),
                // The system's must name Olo; the person's needn't.
                None => mine,
            };
            let installed = l.try_exec.as_deref().is_none_or(|t| if t.starts_with('/') { Path::new(t).is_file() } else { olo_xdg::find_program(t).is_some() });
            (!l.hidden && l.enabled && for_olo && !l.not.iter().any(|d| d == "olo") && installed).then_some(path)
        })
        .collect()
}

/// Start the person's login items (call once, at login, off the main
/// thread).
pub fn start() {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")));
    let Some(config) = config else { return };
    let dirs = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into());
    let system: Vec<PathBuf> = dirs.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("autostart")).collect();
    for path in chosen(&config.join("autostart"), &system) {
        let Some(entry) = olo_xdg::desktop_entry::parse_file(&path) else { continue };
        match olo_xdg::open::launch_line(&entry) {
            Some(argv) => match olo_xdg::open::spawn_app(&argv, &entry.id) {
                Ok(()) => crate::log(format!("started login item {}", path.display())),
                Err(e) => crate::log(format!("could not start login item {}: {e}", path.display())),
            },
            None => crate::log(format!("login item {} has no command", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_start() {
        let dir = std::env::temp_dir().join(format!("olo-autostart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mine, sys) = (dir.join("mine"), dir.join("sys"));
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::create_dir_all(&sys).unwrap();
        let entry = |extra: &str| format!("[Desktop Entry]\nType=Application\nName=X\nExec=/usr/bin/true\n{extra}\n");
        std::fs::write(mine.join("slack.desktop"), entry("")).unwrap();
        std::fs::write(mine.join("off.desktop"), entry("X-GNOME-Autostart-enabled=false")).unwrap();
        std::fs::write(mine.join("gnome-only.desktop"), entry("OnlyShowIn=GNOME;")).unwrap();
        std::fs::write(mine.join("missing.desktop"), entry("TryExec=/nonexistent/program")).unwrap();
        // The person turns off a system one by hiding it.
        std::fs::write(mine.join("ibus.desktop"), entry("Hidden=true")).unwrap();
        std::fs::write(sys.join("ibus.desktop"), entry("OnlyShowIn=Olo;")).unwrap();
        std::fs::write(sys.join("gsd-power.desktop"), entry("OnlyShowIn=GNOME;")).unwrap();
        std::fs::write(sys.join("xdg-user-dirs.desktop"), entry("")).unwrap();
        std::fs::write(sys.join("ours.desktop"), entry("OnlyShowIn=GNOME;Olo;")).unwrap();
        std::fs::write(sys.join("not-olo.desktop"), entry("OnlyShowIn=Olo;\nNotShowIn=Olo;")).unwrap();
        let names: Vec<String> = chosen(&mine, &[sys]).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["slack.desktop", "ours.desktop"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
