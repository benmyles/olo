//! The folders a person keeps at hand: the standard ones (Home, Desktop,
//! Documents, …), favourites they add, and the ones gone to lately. Every
//! file panel's folder pop-up and Tracker's Go menu show them, from one
//! file, `~/.config/olo/places.json` (agents can read and change it too),
//! as BeOS's file panel kept Favorites and Recent Folders.

use std::path::{Path, PathBuf};

/// How many recent folders are kept.
const RECENT: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Places {
    #[serde(default)]
    pub favorites: Vec<PathBuf>,
    /// Most recent first.
    #[serde(default)]
    pub recent: Vec<PathBuf>,
}

fn path() -> PathBuf {
    crate::config_dir().join("places.json")
}

/// What is kept now (nothing, if the file can't be read).
pub fn load() -> Places {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(p: &Places) {
    let _ = std::fs::create_dir_all(crate::config_dir());
    if let Ok(json) = serde_json::to_vec_pretty(p) {
        let tmp = path().with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, path());
        }
    }
}

/// `dir` a favourite, or (`on` false) not.
pub fn set_favorite(dir: &Path, on: bool) {
    let mut p = load();
    p.favorites.retain(|f| f != dir);
    if on {
        p.favorites.push(dir.to_path_buf());
    }
    save(&p);
}

/// `dir` was gone to: the most recent folder.
pub fn visited(dir: &Path) {
    let mut p = load();
    if p.recent.first().is_some_and(|r| r == dir) {
        return;
    }
    p.recent.retain(|r| r != dir);
    p.recent.insert(0, dir.to_path_buf());
    p.recent.truncate(RECENT);
    save(&p);
}

/// The standard folders that are there: Home, then Desktop, Documents,
/// Downloads, Pictures, Music and Videos (as `user-dirs.dirs` names them).
pub fn standard() -> Vec<(String, PathBuf)> {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    let mut out = vec![("Home".to_owned(), home.clone())];
    for (key, label, fallback) in [
        ("DESKTOP", "Desktop", "Desktop"),
        ("DOCUMENTS", "Documents", "Documents"),
        ("DOWNLOAD", "Downloads", "Downloads"),
        ("PICTURES", "Pictures", "Pictures"),
        ("MUSIC", "Music", "Music"),
        ("VIDEOS", "Videos", "Videos"),
    ] {
        let dir = olo_xdg::user_dir(key).unwrap_or_else(|| home.join(fallback));
        if dir != home && dir.is_dir() && !out.iter().any(|(_, d)| *d == dir) {
            out.push((label.to_owned(), dir));
        }
    }
    out
}

/// A folder's name to show: its own, or "/" for the root.
pub fn label(dir: &Path) -> String {
    dir.file_name().map_or_else(|| "/".to_owned(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_folders_start_at_home() {
        let s = standard();
        assert_eq!(s[0].0, "Home");
        assert!(s.iter().all(|(_, d)| d.is_dir()));
        assert_eq!(label(Path::new("/")), "/");
        assert_eq!(label(Path::new("/a/bc")), "bc");
    }
}
