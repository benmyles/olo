//! Tab groups: a Terminal window can be a named group of tabs with a
//! colour, saved (each tab's folder) and opened again later. Saved groups
//! live in `terminal-groups.json` in the config directory.

use std::path::PathBuf;

use olo_kit::{
    egui::Color32,
    style,
    theme::{Hue, Palette},
};

/// The colours a group can have: the look's hues (the saved index into
/// this stays the same colour from look to look).
pub const COLORS: [Hue; 8] = [Hue::Red, Hue::Orange, Hue::Yellow, Hue::Green, Hue::Teal, Hue::Blue, Hue::Purple, Hue::Gray];

pub fn color(i: usize, p: &Palette) -> Color32 {
    style::c(p.hue(COLORS[i % COLORS.len()]))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Group {
    pub name: String,
    /// Into [`COLORS`].
    pub color: usize,
    /// The git repository it's for (its main worktree), if it became its
    /// group on its own (repos.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<PathBuf>,
    /// It became its repository's group on its own (not named by the
    /// person), so it follows the tabs: gone when none is in the
    /// repository, another's when they all move to that one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Saved {
    pub group: Group,
    pub tabs: Vec<SavedTab>,
    #[serde(default)]
    pub active: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavedTab {
    pub cwd: Option<PathBuf>,
    pub title: String,
}

fn path() -> PathBuf {
    olo_kit::config_dir().join("terminal-groups.json")
}

pub fn load() -> Vec<Saved> {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Write the saved groups (off the UI thread).
pub fn store(list: &[Saved]) {
    let path = path();
    let text = serde_json::to_string_pretty(list).unwrap_or_default();
    std::thread::spawn(move || {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = path.with_extension("json.new");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    });
}

/// A name for a new group: "Group 2" if "Group 1" is taken, and so on.
pub fn new_name(taken: &[&str]) -> String {
    (1..).map(|n| format!("Group {n}")).find(|n| !taken.contains(&n.as_str())).unwrap_or_default()
}

/// The first colour no group in `used` has (else the next in turn).
pub fn new_color(used: &[usize]) -> usize {
    (0..COLORS.len()).find(|c| !used.contains(c)).unwrap_or(used.len() % COLORS.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_colors() {
        assert_eq!(new_name(&[]), "Group 1");
        assert_eq!(new_name(&["Group 1", "Group 3"]), "Group 2");
        assert_eq!(new_color(&[0, 1]), 2);
        assert_eq!(new_color(&[0, 1, 2, 3, 4, 5, 6, 7, 0]), 1);
    }
}
