//! Files as Tracker sees them: entries with a kind and an icon, plus the
//! operations (trash, copy, move, rename, open) behind its menus.

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Text,
    Image,
    Audio,
    Video,
    Archive,
    Pdf,
    Application,
    Executable,
    Document,
}

impl Kind {
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Folder => "folder",
            Kind::Text => "text",
            Kind::Image => "image",
            Kind::Audio => "audio",
            Kind::Video => "video",
            Kind::Archive => "archive",
            Kind::Pdf => "document",
            Kind::Application => "application",
            Kind::Executable => "executable",
            Kind::Document => "document",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::Text => "Text",
            Kind::Image => "Image",
            Kind::Audio => "Audio",
            Kind::Video => "Video",
            Kind::Archive => "Archive",
            Kind::Pdf => "PDF Document",
            Kind::Application => "Application",
            Kind::Executable => "Executable",
            Kind::Document => "Document",
        }
    }
}

fn kind_for(path: &Path, is_dir: bool, mode: u32) -> Kind {
    if is_dir {
        return Kind::Folder;
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    // Executable scripts run when opened, so show them as executables.
    if mode & 0o111 != 0 && matches!(ext.as_str(), "sh" | "py" | "pl" | "rb" | "lua" | "") {
        return Kind::Executable;
    }
    match ext.as_str() {
        "txt" | "md" | "rs" | "toml" | "json" | "yaml" | "yml" | "c" | "h" | "cpp" | "hpp" | "py" | "sh" | "js"
        | "ts" | "html" | "css" | "xml" | "conf" | "ini" | "log" | "csv" | "go" | "java" | "lua" | "rb" | "desktop" => {
            Kind::Text
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "svg" | "heic" | "ico" => Kind::Image,
        "mp3" | "flac" | "ogg" | "wav" | "m4a" | "aac" | "opus" => Kind::Audio,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" => Kind::Video,
        "zip" | "tar" | "gz" | "xz" | "bz2" | "zst" | "7z" | "rar" | "tgz" | "rpm" | "deb" => Kind::Archive,
        "pdf" => Kind::Pdf,
        "appimage" => Kind::Application,
        _ if mode & 0o111 != 0 => Kind::Executable,
        "" => sniff_text(path).then_some(Kind::Text).unwrap_or(Kind::Document),
        _ => Kind::Document,
    }
}

/// Extension-less files that look like text (README, Makefile, …).
fn sniff_text(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 512];
    let Ok(mut f) = fs::File::open(path) else { return false };
    let n = f.read(&mut buf).unwrap_or(0);
    n > 0 && std::str::from_utf8(&buf[..n]).is_ok() && !buf[..n].contains(&0)
}

pub fn entry(path: &Path) -> Option<Entry> {
    let lmeta = fs::symlink_metadata(path).ok()?;
    let is_link = lmeta.file_type().is_symlink();
    let meta = if is_link { fs::metadata(path).unwrap_or(lmeta) } else { lmeta };
    let is_dir = meta.is_dir();
    let mode = meta.permissions().mode();
    Some(Entry {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()),
        path: path.to_owned(),
        is_dir,
        is_link,
        size: if is_dir { 0 } else { meta.size() },
        modified: meta.modified().ok(),
        mode,
        kind: kind_for(path, is_dir, mode),
    })
}

pub fn list(dir: &Path, hidden: bool) -> std::io::Result<Vec<Entry>> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir)?.flatten() {
        let name = e.file_name();
        if !hidden && name.to_string_lossy().starts_with('.') {
            continue;
        }
        if let Some(en) = entry(&e.path()) {
            v.push(en);
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

pub fn format_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} bytes");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 { format!("{v:.0} {}", UNITS[u]) } else { format!("{v:.1} {}", UNITS[u]) }
}

pub fn format_time(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return String::new() };
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // SAFETY: localtime_r fills the zeroed tm we own.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t = secs as libc::time_t;
        libc::localtime_r(&t, &mut tm);
        tm
    };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let h = tm.tm_hour % 12;
    format!(
        "{} {}, {}, {}:{:02} {}",
        MONTHS[tm.tm_mon as usize % 12],
        tm.tm_mday,
        tm.tm_year + 1900,
        if h == 0 { 12 } else { h },
        tm.tm_min,
        if tm.tm_hour < 12 { "AM" } else { "PM" }
    )
}

// ---- operations ---------------------------------------------------------

fn trash_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
        .join("Trash")
}

pub fn trash_files() -> PathBuf {
    trash_dir().join("files")
}

pub fn trash_is_empty() -> bool {
    fs::read_dir(trash_files()).map(|mut d| d.next().is_none()).unwrap_or(true)
}

fn unique(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() && fs::symlink_metadata(&p).is_err() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.to_owned(), String::new()),
    };
    // Numbering starts at 2: "New Folder", "New Folder 2", …
    for i in 2.. {
        let p = dir.join(format!("{stem} {i}{ext}"));
        if fs::symlink_metadata(&p).is_err() {
            return p;
        }
    }
    unreachable!()
}

/// Move to the freedesktop trash (with a .trashinfo so other desktops can
/// restore it).
pub fn move_to_trash(path: &Path) -> std::io::Result<()> {
    let files = trash_files();
    let info = trash_dir().join("info");
    fs::create_dir_all(&files)?;
    fs::create_dir_all(&info)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let target = unique(&files, &name);
    let tname = target.file_name().unwrap().to_string_lossy().into_owned();
    let now = format_iso_now();
    let info_file = info.join(format!("{tname}.trashinfo"));
    let abs = std::path::absolute(path)?;
    fs::write(&info_file, format!("[Trash Info]\nPath={}\nDeletionDate={now}\n", percent_encode(&abs)))?;
    move_path(path, &target).inspect_err(|_| {
        let _ = fs::remove_file(&info_file);
    })
}

/// URL-style escaping for .trashinfo paths, as the trash spec requires.
fn percent_encode(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn empty_trash() -> std::io::Result<()> {
    for sub in ["files", "info"] {
        let d = trash_dir().join(sub);
        if let Ok(rd) = fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if fs::symlink_metadata(&p).map(|m| m.is_dir()).unwrap_or(false) {
                    fs::remove_dir_all(&p)?;
                } else {
                    fs::remove_file(&p)?;
                }
            }
        }
    }
    Ok(())
}

fn format_iso_now() -> String {
    // SAFETY: as in format_time.
    let tm = unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

pub fn move_path(from: &Path, to: &Path) -> std::io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        // Across file systems: copy, then remove.
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            copy_path(from, to)?;
            if fs::symlink_metadata(from)?.is_dir() { fs::remove_dir_all(from) } else { fs::remove_file(from) }
        }
        Err(e) => Err(e),
    }
}

pub fn copy_path(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)
    } else if meta.is_dir() {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)?.flatten() {
            copy_path(&e.path(), &to.join(e.file_name()))?;
        }
        fs::set_permissions(to, meta.permissions())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// Copy `from` into `dir`, renaming on collision ("name copy").
pub fn copy_into(from: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let target = if from.parent() == Some(dir) {
        unique(dir, &copy_name(&name))
    } else {
        unique(dir, &name)
    };
    copy_path(from, &target)?;
    Ok(target)
}

pub fn move_into(from: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let target = unique(dir, &name);
    move_path(from, &target)?;
    Ok(target)
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

fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

/// Open a file with the right application: ours where we have one,
/// otherwise the freedesktop default.
pub fn open(entry: &Entry) {
    let q = shell_quote(&entry.path);
    let cmd = match entry.kind {
        Kind::Text => format!("tabula-styled-edit {q}"),
        Kind::Image => format!("tabula-show-image {q}"),
        Kind::Executable | Kind::Application => q,
        _ => format!("xdg-open {q}"),
    };
    if let Err(e) = tabula_kit::spawn_detached(&cmd) {
        tracing::warn!("open {}: {e}", entry.path.display());
    }
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
    fn copy_names() {
        assert_eq!(copy_name("notes.txt"), "notes copy.txt");
        assert_eq!(copy_name("notes copy.txt"), "notes copy.txt");
        assert_eq!(copy_name("notes copy 12.txt"), "notes copy.txt");
        assert_eq!(copy_name("photo 3.png"), "photo 3 copy.png");
        assert_eq!(copy_name("2024"), "2024 copy");
        assert_eq!(copy_name(".bashrc"), ".bashrc copy");
    }

    #[test]
    fn trashinfo_paths_are_escaped() {
        assert_eq!(percent_encode(Path::new("/home/a b&c.txt")), "/home/a%20b%26c.txt");
        assert_eq!(percent_encode(Path::new("/tmp/é")), "/tmp/%C3%A9");
    }
}
