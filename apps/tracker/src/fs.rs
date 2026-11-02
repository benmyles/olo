//! Files as Tracker sees them: entries with a kind and an icon, plus the
//! operations (trash, copy, move, rename, open) behind its menus.

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

use olo_xdg::{mime, open as opener};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: u32,
    pub kind: Kind,
    /// The icon of the Olo application this program (or a link to it)
    /// is.
    pub app_icon: Option<&'static str>,
    /// A download being written (Chrome's `NAME.crdownload`): shown as
    /// `NAME`, with how far along it is when it says how big it will be
    /// (`Some`, `olo_kit_base::files::EXPECTED_SIZE`).
    pub downloading: Option<Option<u64>>,
}

impl Entry {
    /// The name shown: a download's as it will be once done.
    pub fn label(&self) -> &str {
        match self.downloading {
            Some(_) => self.name.strip_suffix(olo_kit_base::files::DOWNLOADING).filter(|n| !n.is_empty()).unwrap_or(&self.name),
            None => &self.name,
        }
    }

    /// How far along a download being written is (0–1), if it knows.
    pub fn progress(&self) -> Option<f32> {
        let total = self.downloading.flatten().filter(|&t| t > 0)?;
        Some((self.size as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }

    /// Its Size column: a download's "3.1 of 10 MB".
    pub fn size_text(&self) -> String {
        match (self.is_dir, self.downloading.flatten()) {
            (true, _) => "—".into(),
            (false, Some(total)) => olo_kit_base::files::format_progress(self.size, total),
            (false, None) => format_size(self.size),
        }
    }

    /// Its Kind column: "Downloading, 31%" for a download.
    pub fn kind_text(&self) -> String {
        match (self.downloading, self.progress()) {
            (Some(_), Some(p)) => format!("Downloading, {}%", (p * 100.0).floor() as u32),
            (Some(_), None) => "Downloading".into(),
            (None, _) => self.kind.name().into(),
        }
    }
}

pub use olo_kit_base::files::{Kind, format_size, format_time, kind_for};

pub fn entry(path: &Path) -> Option<Entry> {
    let lmeta = fs::symlink_metadata(path).ok()?;
    let is_link = lmeta.file_type().is_symlink();
    let meta = if is_link { fs::metadata(path).unwrap_or(lmeta) } else { lmeta };
    let is_dir = meta.is_dir();
    let mode = meta.permissions().mode();
    // A download being written: where it goes once done, and how big it
    // will be if it says.
    let downloading = (!is_dir)
        .then(|| path.to_str()?.strip_suffix(olo_kit_base::files::DOWNLOADING).filter(|p| !p.ends_with('/')).map(PathBuf::from))
        .flatten()
        .map(|done| (done, olo_kit_base::files::expected_size(path)));
    Some(Entry {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()),
        path: path.to_owned(),
        is_dir,
        is_link,
        size: if is_dir { 0 } else { meta.size() },
        modified: meta.modified().ok(),
        mode,
        // A download is the kind it will be.
        kind: kind_for(downloading.as_ref().map_or(path, |(done, _)| done.as_path()), is_dir, mode),
        app_icon: if is_dir { None } else { app_icon(path, is_link) },
        downloading: downloading.map(|(_, total)| total),
    })
}

/// One of our applications' icons, for its program or a link to it.
fn app_icon(path: &Path, is_link: bool) -> Option<&'static str> {
    let target = if is_link { fs::read_link(path).ok()? } else { path.to_owned() };
    let name = target.file_name()?.to_str()?;
    olo_xdg::apps::OUR_APPS.iter().find(|(bin, _, _)| *bin == name).map(|(_, _, icon)| *icon)
}

/// The app id of the running application program `e` is (or links to),
/// when it can open another window: New Window asks it for one.
pub fn running_app(e: &Entry) -> Option<String> {
    e.app_icon?;
    let target = if e.is_link { fs::read_link(&e.path).ok()? } else { e.path.clone() };
    let bin = target.file_name()?.to_str()?;
    if !olo_xdg::apps::NEW_WINDOW.contains(&bin) {
        return None;
    }
    let id = olo_xdg::apps::app_id_of(bin)?;
    olo_kit_base::instance::running(&id).then_some(id)
}

/// Apps whose windows aren't grouped, as app_server's settings last said
/// (kept by the settings watch, so menus never wait for app_server).
pub static UNGROUPED: olo_sync::Mutex<Vec<String>> = olo_sync::Mutex::new(Vec::new());

/// The app id of the application `e` is: one of our programs (or a link
/// to one), or a `.desktop` entry (its id, which Wayland apps use).
pub fn app_id(e: &Entry) -> Option<String> {
    if e.is_dir {
        return None;
    }
    if e.app_icon.is_some() {
        let target = if e.is_link { fs::read_link(&e.path).ok()? } else { e.path.clone() };
        return olo_xdg::apps::app_id_of(target.file_name()?.to_str()?);
    }
    e.name.strip_suffix(".desktop").filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Group Windows for `e`'s app: shown when it is an application, checked
/// while its windows are grouped.
pub fn grouping_item(menu: olo_ui::Menu, e: Option<&Entry>) -> olo_ui::Menu {
    let Some(app) = e.and_then(app_id) else { return menu };
    let grouped = !UNGROUPED.lock().is_ok_and(|u| u.contains(&app));
    menu.item("Group Windows", "group-windows").checked(grouped)
}

/// Group or ungroup `e`'s app's windows (saved in app_server's settings).
pub fn toggle_grouping(e: Option<&Entry>) {
    if let Some(app) = e.and_then(app_id) {
        let grouped = !UNGROUPED.lock().is_ok_and(|u| u.contains(&app));
        olo_kit_base::settings::set_groups_windows(&app, !grouped);
    }
}

/// Ask the running app `app_id` for another window, off the UI thread.
pub fn new_window(app_id: String) {
    std::thread::spawn(move || olo_kit_base::instance::forward(&app_id, &["--new-window".to_owned()]));
}

pub fn list(dir: &Path, hidden: bool) -> std::io::Result<Vec<Entry>> {
    list_counting(dir, hidden, &std::sync::atomic::AtomicU64::new(0))
}

/// [`list`], counting items into `count` as it goes (for progress).
pub fn list_counting(dir: &Path, hidden: bool, count: &std::sync::atomic::AtomicU64) -> std::io::Result<Vec<Entry>> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir)?.flatten() {
        let name = e.file_name();
        if !hidden && name.to_string_lossy().starts_with('.') {
            continue;
        }
        if let Some(en) = entry(&e.path()) {
            v.push(en);
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    Ok(v)
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

/// A path for display, with the home folder shortened to `~`.
pub fn display_path(p: &Path) -> String {
    match p.strip_prefix(home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// A display name for a folder: "Home" for $HOME, the volume name for "/".
pub fn display_name(path: &Path) -> String {
    if path == home() {
        return "Home".into();
    }
    if path == Path::new("/") {
        return volume_label();
    }
    if path == trash_files() {
        return "Trash".into();
    }
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// The boot volume's name, as BeOS showed it on the desktop.
pub fn volume_label() -> String {
    fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && s != "localhost")
        .unwrap_or_else(|| "System".into())
}

// ---- operations ---------------------------------------------------------

pub use olo_xdg::trash::{trash_dir, trash_files, trash_is_empty, trash_origin, trash_slot, unique};

/// Where a copy of `from` dropped into `dir` goes: its own name, or
/// "name copy" when copying into the folder it is already in.
pub fn copy_target(from: &Path, dir: &Path) -> PathBuf {
    let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if from.parent() == Some(dir) { unique(dir, &copy_name(&name)) } else { unique(dir, &name) }
}






/// "notes copy.txt" for "notes.txt"; copying a copy gives "notes copy.txt"
/// again (made unique as "notes copy 2.txt"), never "notes copy copy.txt".
fn copy_name(name: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    let base = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    let base = base.strip_suffix(" copy ").or_else(|| stem.strip_suffix(" copy")).unwrap_or(stem);
    format!("{base} copy{ext}")
}

pub fn new_folder(dir: &Path) -> std::io::Result<PathBuf> {
    let p = unique(dir, "New Folder");
    fs::create_dir(&p)?;
    Ok(p)
}

pub fn rename(path: &Path, name: &str) -> std::io::Result<PathBuf> {
    if name.is_empty() || name.contains('/') {
        return Err(std::io::Error::other("invalid name"));
    }
    let to = path.with_file_name(name);
    if to.exists() && to != path {
        return Err(std::io::Error::other("a file with that name already exists"));
    }
    fs::rename(path, &to)?;
    Ok(to)
}

pub fn make_link(path: &Path) -> std::io::Result<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let name = format!("{} link", path.file_name().unwrap_or_default().to_string_lossy());
    let to = unique(dir, &name);
    std::os::unix::fs::symlink(path, &to)?;
    Ok(to)
}

/// A link to `path` in `dir` ("name link" if `dir` is where it is).
pub fn link_into(path: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let to = if path.parent() == Some(dir) { unique(dir, &format!("{name} link")) } else { unique(dir, &name) };
    std::os::unix::fs::symlink(path, &to)?;
    Ok(to)
}

/// A link to the program `path` in `dir`, named as the application is when
/// it is one of ours ("Terminal", not "olo-terminal").
pub fn app_link_into(path: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let name = olo_xdg::apps::OUR_APPS.iter().find(|(bin, _, _)| *bin == file).map_or(file.as_str(), |(_, name, _)| *name);
    let to = unique(dir, name);
    std::os::unix::fs::symlink(path, &to)?;
    Ok(to)
}

/// Text dropped on a folder, saved there as a clipping file named after
/// its first words (BeOS's Tracker made the same).
pub fn clipping(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    let first: String = text.split_whitespace().take(6).collect::<Vec<_>>().join(" ");
    let mut name: String = first.chars().filter(|c| *c != '/' && !c.is_control()).take(40).collect();
    name = name.trim_matches(['.', ' ']).to_owned();
    if name.is_empty() {
        name = "Clipping".into();
    }
    let to = unique(dir, &format!("{name} clipping.txt"));
    fs::write(&to, text)?;
    Ok(to)
}

/// A file dropped by its contents (`name`, `data`) into `dir`: under its
/// name, or a free one beside it.
pub fn save_new(dir: &Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    let to = unique(dir, name);
    fs::write(&to, data)?;
    Ok(to)
}

/// A link dropped on a folder: a clipping holding its address, named by
/// its text (or its site).
pub fn link_clipping(dir: &Path, url: &str, title: &str) -> std::io::Result<PathBuf> {
    let site = url.split_once("://").map_or(url, |(_, r)| r).split(['/', '?', '#']).next().unwrap_or("").trim_start_matches("www.");
    let words = if title.trim().is_empty() || title.trim() == url { site } else { title };
    let mut name: String = words.chars().filter(|c| *c != '/' && !c.is_control()).take(40).collect();
    name = name.trim_matches(['.', ' ']).to_owned();
    if name.is_empty() {
        name = "Link".into();
    }
    let to = unique(dir, &format!("{name} link clipping.txt"));
    fs::write(&to, format!("{url}\n"))?;
    Ok(to)
}

/// A picture pasted into a folder (`image/KIND` data from the
/// clipboard), saved there as "Pasted Picture" of its kind.
pub fn pasted_picture(dir: &Path, kind: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    let ext = match kind.trim_start_matches("x-") {
        "jpeg" | "jpg" | "pjpeg" => "jpg",
        "svg+xml" => "svg",
        "vnd.microsoft.icon" | "icon" => "ico",
        other => other,
    };
    let ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    let to = unique(dir, &format!("Pasted Picture.{}", if ext.is_empty() { "img" } else { &ext }));
    fs::write(&to, data)?;
    Ok(to)
}

/// The name of the application `open` would use, when it is one of ours.
pub fn opener_name(entry: &Entry) -> Option<String> {
    match entry.kind {
        Kind::Folder => Some("Tracker".into()),
        Kind::Executable | Kind::Application => None,
        _ => opener::handler_cached(&mime::for_path(&entry.path)).map(|h| h.name()),
    }
}

/// Show any file in its own Viewer window.
/// A new Terminal tab in `dir` (in the Terminal window last in front;
/// a window if there is none).
pub fn open_in_terminal(dir: &Path) {
    let terminal = olo_xdg::find_program("olo-terminal").map_or_else(|| "olo-terminal".into(), PathBuf::into_os_string);
    if let Err(e) = opener::spawn(&[terminal, "--tab".into(), "-w".into(), dir.as_os_str().to_owned()]) {
        tracing::warn!("terminal in {}: {e}", dir.display());
    }
}

pub fn open_in_viewer(path: &Path) {
    let viewer = olo_xdg::find_program("olo-viewer").map_or_else(|| "olo-viewer".into(), PathBuf::into_os_string);
    if let Err(e) = opener::spawn(&[viewer, path.as_os_str().to_owned()]) {
        tracing::warn!("viewer {}: {e}", path.display());
    }
}

/// Open a file with the right application: ours where we have one,
/// otherwise the user's default (see `olo_xdg::open`). Programs run.
pub fn open(entry: &Entry) {
    let (path, program) = (entry.path.clone(), matches!(entry.kind, Kind::Executable | Kind::Application));
    // Finding the handler reads files: not on the UI thread.
    std::thread::spawn(move || {
        let r = if program { opener::spawn(&[path.clone().into_os_string()]) } else { opener::open(&opener::Target::Path(path.clone())) };
        if let Err(e) = r {
            tracing::warn!("open {}: {e}", path.display());
        }
    });
}

/// Recursive size of a folder and the number of items (files and folders)
/// inside it, for Get Info; capped by `limit` entries.
pub fn dir_size(path: &Path, limit: &mut u32) -> (u64, u64) {
    let (mut bytes, mut files) = (0, 0);
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            if *limit == 0 {
                break;
            }
            *limit -= 1;
            if let Ok(m) = e.metadata() {
                if m.is_dir() {
                    let (b, f) = dir_size(&e.path(), limit);
                    bytes += b;
                    files += f + 1; // the folder is an item too
                } else {
                    bytes += m.len();
                    files += 1;
                }
            }
        }
    }
    (bytes, files)
}

/// Mounted volumes worth showing on the desktop: the root and anything
/// under /run/media or /media.
pub fn volumes() -> Vec<(String, PathBuf, &'static str)> {
    let mut v = vec![(volume_label(), PathBuf::from("/"), "disk")];
    if let Ok(m) = fs::read_to_string("/proc/mounts") {
        for line in m.lines() {
            let mut f = line.split_whitespace();
            let (_dev, mnt) = (f.next().unwrap_or(""), f.next().unwrap_or(""));
            let mnt = mnt.replace("\\040", " ");
            if mnt.starts_with("/run/media/") || mnt.starts_with("/media/") {
                let p = PathBuf::from(&mnt);
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(mnt.clone());
                v.push((name, p, "usb"));
            }
        }
    }
    v
}

/// Free / total bytes of the file system holding `path`.
pub fn fs_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the zeroed struct we own.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
        Some((st.f_bavail as u64 * st.f_frsize as u64, st.f_blocks as u64 * st.f_frsize as u64))
    }
}

pub fn permissions_string(mode: u32, is_dir: bool) -> String {
    let mut s = String::with_capacity(10);
    s.push(if is_dir { 'd' } else { '-' });
    for shift in [6, 3, 0] {
        let m = (mode >> shift) & 7;
        s.push(if m & 4 != 0 { 'r' } else { '-' });
        s.push(if m & 2 != 0 { 'w' } else { '-' });
        s.push(if m & 1 != 0 { 'x' } else { '-' });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_show_as_they_will_be() {
        let dir = std::env::temp_dir().join(format!("olo-tracker-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Report.pdf.crdownload");
        std::fs::write(&p, vec![0u8; 3 * 1024 * 1024 + 100_000]).unwrap();
        let e = entry(&p).unwrap();
        assert_eq!((e.label(), e.kind, e.downloading), ("Report.pdf", Kind::Pdf, Some(None)));
        assert_eq!(e.kind_text(), "Downloading");
        if olo_kit_base::files::set_expected_size(&p, 10 * 1024 * 1024) {
            let e = entry(&p).unwrap();
            assert_eq!(e.kind_text(), "Downloading, 30%");
            assert_eq!(e.size_text(), "3.1 of 10 MB");
        }
        // Only a name with something before the suffix.
        let odd = dir.join(".crdownload");
        std::fs::write(&odd, b"x").unwrap();
        assert_eq!(entry(&odd).unwrap().downloading, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_names() {
        assert_eq!(copy_name("notes.txt"), "notes copy.txt");
        assert_eq!(copy_name("notes copy.txt"), "notes copy.txt");
        assert_eq!(copy_name("notes copy 12.txt"), "notes copy.txt");
        assert_eq!(copy_name("photo 3.png"), "photo 3 copy.png");
        assert_eq!(copy_name("2024"), "2024 copy");
        assert_eq!(copy_name(".bashrc"), ".bashrc copy");
    }
}
