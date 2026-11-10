//! Freedesktop `.desktop` entries: the installed applications Deskbar
//! lists and launches, and how Wayland app ids map back to names/icons.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DesktopEntry {
    /// File stem, e.g. `org.gnome.Nautilus`.
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub exec: Option<String>,
    pub icon: Option<String>,
    pub categories: Vec<String>,
    pub terminal: bool,
    pub no_display: bool,
    pub startup_wm_class: Option<String>,
    /// MIME types it opens (`MimeType=`).
    pub mime_types: Vec<String>,
}

/// Where applications' desktop entries are, most personal first.
pub fn app_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        v.push(Path::new(&home).join(".local/share/applications"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(|s| Path::new(s).join("applications")));
    v.push("/var/lib/flatpak/exports/share/applications".into());
    // AppImages' (last: one installed wins).
    v.push(appimages_dir());
    v
}

/// Where the entries made for AppImages are (`olo_appimage`): each
/// `NoDisplay`, naming its AppImage and the icon taken out of it, so a
/// running one is found by its app id as an installed app is.
pub fn appimages_dir() -> PathBuf {
    crate::cache_dir().join("appimages/applications")
}

/// The entry in the file at `path` (an autostart entry, one dropped on
/// Deskbar), if it is an application's.
pub fn parse_file(path: &Path) -> Option<DesktopEntry> {
    parse(path)
}

pub(crate) fn parse(path: &Path) -> Option<DesktopEntry> {
    let text = std::fs::read_to_string(path).ok()?;
    let lang = std::env::var("LANG").ok().and_then(|l| l.split(['.', '_']).next().map(str::to_owned));
    let mut e = DesktopEntry { id: path.file_stem()?.to_string_lossy().into_owned(), path: path.to_owned(), ..Default::default() };
    let mut in_main = false;
    let mut localized_name = None;
    let mut is_app = false;
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim().to_owned());
        match k {
            "Type" => is_app = v == "Application",
            "Name" => e.name = v,
            "GenericName" => e.generic_name = Some(v),
            "Comment" => e.comment = Some(v),
            "Exec" => e.exec = Some(v),
            "Icon" => e.icon = Some(v),
            "Categories" => e.categories = v.split(';').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            "Terminal" => e.terminal = v == "true",
            "NoDisplay" => e.no_display = v == "true",
            "Hidden" => hidden = v == "true",
            "StartupWMClass" => e.startup_wm_class = Some(v),
            "MimeType" => e.mime_types = v.split(';').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            _ => {
                if let Some(l) = &lang
                    && k == format!("Name[{l}]")
                {
                    localized_name = Some(v);
                }
            }
        }
    }
    if let Some(n) = localized_name {
        e.name = n;
    }
    // Olo's own icon for a program it has one for (Chrome, Slack), over
    // whatever the program ships.
    if is_app && crate::has_own_icon(&e.id) {
        e.icon = Some(e.id.clone());
    }
    (is_app && !hidden && !e.name.is_empty()).then_some(e)
}

/// Every installed application, first definition of each id winning
/// (user entries override system ones).
/// [`all`], kept: read again only when an applications folder changed
/// (one installed or removed); a look costs a few `stat`s.
pub fn all_cached() -> std::sync::Arc<Vec<DesktopEntry>> {
    type Kept = olo_sync::Mutex<Option<(Vec<Option<std::time::SystemTime>>, std::sync::Arc<Vec<DesktopEntry>>)>>;
    static KEPT: std::sync::OnceLock<Kept> = std::sync::OnceLock::new();
    let stamps = stamps();
    let kept = KEPT.get_or_init(Default::default);
    if let Some((s, all)) = kept.lock().unwrap().as_ref()
        && *s == stamps
    {
        return all.clone();
    }
    let fresh = std::sync::Arc::new(all());
    *kept.lock().unwrap() = Some((stamps, fresh.clone()));
    fresh
}

/// When each applications folder last changed (an entry added, removed
/// or replaced): what was read of them is good while these are the same.
pub fn stamps() -> Vec<Option<std::time::SystemTime>> {
    app_dirs().iter().map(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok()).collect()
}

pub fn all() -> Vec<DesktopEntry> {
    let mut seen: HashMap<String, DesktopEntry> = HashMap::new();
    for dir in app_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for f in rd.flatten() {
            let p = f.path();
            if p.extension().is_some_and(|x| x == "desktop") {
                let id = p.file_stem().unwrap().to_string_lossy().into_owned();
                if seen.contains_key(&id) {
                    continue;
                }
                if let Some(e) = parse(&p) {
                    seen.insert(id, e);
                }
            }
        }
    }
    let mut v: Vec<_> = seen.into_values().collect();
    v.sort_by_key(|e| e.name.to_lowercase());
    v
}

/// The entry for a Wayland app id: by file name, then by StartupWMClass,
/// then case-insensitively by the id's last component.
pub fn find(app_id: &str) -> Option<DesktopEntry> {
    if app_id.is_empty() {
        return None;
    }
    for dir in app_dirs() {
        if let Some(e) = parse(&dir.join(format!("{app_id}.desktop"))) {
            return Some(e);
        }
    }
    find_by_class(&all(), app_id).cloned()
}

/// As [`find`], among `entries` (from [`all`], kept: reading them all
/// takes milliseconds) rather than on disk.
pub fn find_among<'a>(entries: &'a [DesktopEntry], app_id: &str) -> Option<&'a DesktopEntry> {
    if app_id.is_empty() {
        return None;
    }
    entries.iter().find(|e| e.id == app_id).or_else(|| find_by_class(entries, app_id))
}

/// The entry for `app_id` by StartupWMClass, then case-insensitively by
/// the id's last component.
fn find_by_class<'a>(entries: &'a [DesktopEntry], app_id: &str) -> Option<&'a DesktopEntry> {
    let lower = app_id.to_lowercase();
    let last = lower.rsplit('.').next().unwrap_or(&lower).to_owned();
    entries
        .iter()
        .find(|e| e.startup_wm_class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(app_id)))
        .or_else(|| entries.iter().find(|e| e.id.to_lowercase() == lower))
        .or_else(|| entries.iter().find(|e| e.id.to_lowercase().rsplit('.').next() == Some(last.as_str())))
}

impl DesktopEntry {
    /// Its command line takes files or links (`%f`, `%F`, `%u`, `%U`): it
    /// can open one.
    pub fn takes_files(&self) -> bool {
        self.exec.as_deref().is_some_and(|e| e.split_whitespace().any(|p| matches!(p.trim_matches(['"', '\'']), "%f" | "%F" | "%u" | "%U")))
    }

    /// The command line with field codes removed.
    pub fn command(&self) -> Option<String> {
        let exec = self.exec.as_ref()?;
        let mut out = Vec::new();
        for part in exec.split_whitespace() {
            if part.starts_with('%') && part.len() == 2 {
                continue;
            }
            out.push(part);
        }
        Some(out.join(" "))
    }

    /// Start the application detached from us, in a scope of its own
    /// (`scope`), unless it runs in Terminal.
    pub fn launch(&self) -> std::io::Result<()> {
        let Some(cmd) = self.command() else {
            return Err(std::io::Error::other("no Exec line"));
        };
        if self.terminal {
            return crate::spawn_detached(&format!("olo-terminal -e {cmd}"));
        }
        crate::spawn_detached_app(&cmd, &self.id)
    }

    /// Start it as a copy of its own, beside one running (Deskbar's
    /// ⌘-click): Olo's apps then don't hand the launch to the copy running
    /// (`NEW_INSTANCE`); another program does as it does (Chrome opens a
    /// window in the one it has).
    pub fn launch_new_instance(&self) -> std::io::Result<()> {
        let Some(cmd) = self.command() else {
            return Err(std::io::Error::other("no Exec line"));
        };
        if self.terminal {
            return crate::spawn_detached(&format!("olo-terminal -e {cmd}"));
        }
        crate::spawn_detached_app_with(&cmd, &self.id, &[(crate::NEW_INSTANCE, "1")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Olo's own icon wins over the one a program's entry names (Slack's
    /// is a path into its install).
    #[test]
    fn own_icons_win() {
        let dir = std::env::temp_dir().join(format!("olo-entry-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("slack.desktop");
        std::fs::write(&p, "[Desktop Entry]\nType=Application\nName=Slack\nIcon=/opt/slack/slack.png\nStartupWMClass=Slack\n").unwrap();
        assert_eq!(parse(&p).unwrap().icon.as_deref(), Some("slack"));
        let q = dir.join("someapp.desktop");
        std::fs::write(&q, "[Desktop Entry]\nType=Application\nName=Some App\nIcon=someapp\n").unwrap();
        assert_eq!(parse(&q).unwrap().icon.as_deref(), Some("someapp"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only a command that takes files can open one.
    #[test]
    fn takes_files() {
        let entry = |exec: &str| DesktopEntry { exec: Some(exec.into()), ..Default::default() };
        assert!(entry("firefox %u").takes_files() && entry("sh -c 'x \"%F\"'").takes_files() && entry("eog \"%U\"").takes_files());
        assert!(!entry("btop").takes_files() && !entry("app --icon %i").takes_files() && !DesktopEntry::default().takes_files());
    }
}
