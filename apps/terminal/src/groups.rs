//! Tab groups: a Terminal window can be a named group of tabs with a
//! colour, saved (each tab's folder) and opened again later. Saved groups
//! live in `terminal-groups.json` in the config directory.

use std::path::PathBuf;

use olo_kit::egui::Color32;

/// The colours a group can have: (name, light mode, dark mode).
pub const COLORS: [(&str, [u8; 3], [u8; 3]); 8] = [
    ("Red", [214, 59, 50], [232, 92, 82]),
    ("Orange", [232, 128, 24], [240, 152, 64]),
    ("Yellow", [222, 176, 20], [236, 196, 64]),
    ("Green", [52, 160, 72], [84, 186, 98]),
    ("Teal", [24, 150, 160], [58, 180, 190]),
    ("Blue", [44, 108, 214], [84, 140, 236]),
    ("Purple", [146, 80, 196], [174, 112, 222]),
    ("Grey", [122, 124, 130], [150, 152, 158]),
];

pub fn color(i: usize, dark: bool) -> Color32 {
    let (_, light, night) = COLORS[i % COLORS.len()];
    let [r, g, b] = if dark { night } else { light };
    Color32::from_rgb(r, g, b)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Group {
    pub name: String,
    /// Into [`COLORS`].
    pub color: usize,
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
