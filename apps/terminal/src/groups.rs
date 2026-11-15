//! Tab groups: a Terminal window can be a named group of tabs with a
//! colour, saved (each tab's folder) and opened again later. Saved groups
//! live in `terminal-groups.json` in the config directory.

use std::path::PathBuf;

use olo_kit::Hsla;
use olo_theme::Palette;

/// How many colours a group can have: the look's tab group colours (the
/// saved index stays the same colour from look to look, in each look's
/// own way).
pub const COLORS: usize = olo_theme::GROUP_COLORS;

/// Colour `i` in the look `p` (Ben, 2026-11-14: "any app with tab groups
/// should use this theme based palette for the tab group color
/// choices"): the look's own (`Palette::group_colors`).
pub fn color(i: usize, p: &Palette) -> Hsla {
    olo_kit::style::c(p.group_color(i).fill)
}

/// Its name in the look ("Red"; Platinum's "Cyan", "Magenta", "Brown").
pub fn name(i: usize, p: &Palette) -> &'static str {
    p.group_color(i).name
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
    olo_xdg::config_dir().join("terminal-groups.json")
}

pub fn load() -> Vec<Saved> {
    olo_kit::store::load(&path()).unwrap_or_default()
}

/// Write the saved groups (off the UI thread).
pub fn store(list: &[Saved]) {
    olo_kit::store::save(path(), list);
}

/// A name for a new group: "Group 2" if "Group 1" is taken, and so on.
pub fn new_name(taken: &[&str]) -> String {
    (1..).map(|n| format!("Group {n}")).find(|n| !taken.contains(&n.as_str())).unwrap_or_default()
}

/// The first colour no group in `used` has (else the next in turn).
pub fn new_color(used: &[usize]) -> usize {
    (0..COLORS).find(|c| !used.contains(c)).unwrap_or(used.len() % COLORS)
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
