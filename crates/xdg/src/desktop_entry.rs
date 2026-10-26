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

pub(crate) fn app_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        v.push(Path::new(&home).join(".local/share/applications"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(|s| Path::new(s).join("applications")));
    v.push("/var/lib/flatpak/exports/share/applications".into());
    v
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
    (is_app && !hidden && !e.name.is_empty()).then_some(e)
}

/// Every installed application, first definition of each id winning
/// (user entries override system ones).
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
    let lower = app_id.to_lowercase();
    let last = lower.rsplit('.').next().unwrap_or(&lower).to_owned();
    let entries = all();
    entries
        .iter()
        .find(|e| e.startup_wm_class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(app_id)))
        .or_else(|| entries.iter().find(|e| e.id.to_lowercase() == lower))
        .or_else(|| entries.iter().find(|e| e.id.to_lowercase().rsplit('.').next() == Some(last.as_str())))
        .cloned()
}

impl DesktopEntry {
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

    /// Start the application detached from us.
    pub fn launch(&self) -> std::io::Result<()> {
        let Some(cmd) = self.command() else {
            return Err(std::io::Error::other("no Exec line"));
        };
        let cmd = if self.terminal { format!("tabula-terminal -e {cmd}") } else { cmd };
        crate::spawn_detached(&cmd)
    }
}
