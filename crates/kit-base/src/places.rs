//! The folders a person keeps at hand: the standard ones (Home, Desktop,
//! Documents, …), favourites they add, and the ones gone to lately. Every
//! file panel's folder pop-up and Tracker's Go menu show them, from one
//! file, `~/.config/olo/places.json` (agents can read and change it too),
//! as BeOS's file panel kept Favorites and Recent Folders. It also keeps
//! the folder a panel was last in for each app that asked through the
//! portal, for opening and for saving, so the next opens there.

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
    /// The folder a panel was last in for each app (by its id), opening
    /// and saving.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub apps: std::collections::BTreeMap<String, AppFolders>,
}

/// Where an app's panels were last.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppFolders {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save: Option<PathBuf>,
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

/// The folder `app`'s panel was last in (`saving`: a Save panel's), if
/// it's still there.
pub fn last_for(app: &str, saving: bool) -> Option<PathBuf> {
    let p = load();
    let f = p.apps.get(app)?;
    if saving { f.save.clone() } else { f.open.clone() }.filter(|d| d.is_dir())
}

/// `app`'s panel was left in `dir` (`saving`: a Save panel).
pub fn left_in(app: &str, saving: bool, dir: &Path) {
    if app.is_empty() {
        return;
    }
    let mut p = load();
    let f = p.apps.entry(app.to_owned()).or_default();
    let slot = if saving { &mut f.save } else { &mut f.open };
    if slot.as_deref() == Some(dir) {
        return;
    }
    *slot = Some(dir.to_path_buf());
    save(&p);
}

/// The standard folders that are there: Home, then Desktop, Documents,
/// Downloads, Pictures, Music and Videos (as `user-dirs.dirs` names them).
pub fn standard() -> Vec<(String, PathBuf)> {
    // (Asked for whenever a window's menus are said again, which is
    // whenever the window is: every step of a resize. Looked up afresh
    // at most every two seconds: it reads the environment and
    // user-dirs.dirs and looks at each folder on the disk.)
    static KEPT: std::sync::Mutex<Option<(std::time::Instant, Vec<(String, PathBuf)>)>> = std::sync::Mutex::new(None);
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, list)) = kept.as_ref()
        && at.elapsed() < std::time::Duration::from_secs(2)
    {
        return list.clone();
    }
    let list = standard_now();
    *kept = Some((std::time::Instant::now(), list.clone()));
    list
}

fn standard_now() -> Vec<(String, PathBuf)> {
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

    /// A file from before apps' folders were kept still reads; they're
    /// kept by app, opening and saving apart, and written only if any.
    #[test]
    fn apps_folders_kept() {
        let old: Places = serde_json::from_str(r#"{"favorites":["/a"],"recent":["/b"]}"#).unwrap();
        assert!(old.apps.is_empty() && old.favorites == [PathBuf::from("/a")]);
        assert!(!serde_json::to_string(&old).unwrap().contains("apps"));
        let mut p = old;
        p.apps.insert("com.slack.Slack".into(), AppFolders { open: Some("/home/x/Pictures".into()), save: None });
        let back: Places = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.apps["com.slack.Slack"].save, None);
    }
}
