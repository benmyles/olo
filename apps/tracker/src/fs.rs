//! Files as Tracker sees them: entries with a kind and an icon, plus the
//! operations (trash, copy, move, rename, open) behind its menus.

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

use tabula_kit::{mime, open as opener};

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

pub use tabula_kit::files::{Kind, format_size, format_time, kind_for};

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

pub fn trash_dir() -> PathBuf {
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

pub fn unique(dir: &Path, name: &str) -> PathBuf {
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

/// Reserve a place in the freedesktop Trash for `path`: a unique name in
/// Trash/files and its .trashinfo (written now, so other desktops can
/// restore it). Returns both; remove the info if the move then fails.
pub fn trash_slot(path: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
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
    Ok((target, info_file))
}

/// Where an item in the Trash came from, by its .trashinfo (beside
/// Trash/files, in Trash/info), and that info file.
pub fn trash_origin(item: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    use std::os::unix::ffi::OsStringExt;
    let name = item.file_name().ok_or_else(|| std::io::Error::other("not in the Trash"))?;
    let trash = item.parent().and_then(Path::parent).ok_or_else(|| std::io::Error::other("not in the Trash"))?;
    let mut info_name = name.to_os_string();
    info_name.push(".trashinfo");
    let info = trash.join("info").join(info_name);
    let text = fs::read_to_string(&info)
        .map_err(|_| std::io::Error::other(format!("there’s no record of where “{}” came from", name.to_string_lossy())))?;
    let path = text
        .lines()
        .find_map(|l| l.strip_prefix("Path="))
        .ok_or_else(|| std::io::Error::other(format!("the record of where “{}” came from is damaged", name.to_string_lossy())))?;
    let path = PathBuf::from(std::ffi::OsString::from_vec(percent_decode(path)));
    // Relative paths are relative to the Trash's top directory (the
    // file system's, for a $topdir/.Trash-uid); ours is the home one.
    let path = if path.is_absolute() { path } else { trash.parent().and_then(Path::parent).unwrap_or(Path::new("/")).join(path) };
    Ok((path, info))
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// Where a copy of `from` dropped into `dir` goes: its own name, or
/// "name copy" when copying into the folder it is already in.
pub fn copy_target(from: &Path, dir: &Path) -> PathBuf {
    let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if from.parent() == Some(dir) { unique(dir, &copy_name(&name)) } else { unique(dir, &name) }
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

/// The name of the application `open` would use, when it is one of ours.
pub fn opener_name(entry: &Entry) -> Option<String> {
    match entry.kind {
        Kind::Folder => Some("Tracker".into()),
        Kind::Executable | Kind::Application => None,
        _ => opener::handler_cached(&mime::for_path(&entry.path)).map(|h| h.name()),
    }
}

/// Show any file in its own Viewer window.
/// A new Terminal window (a tab, once Terminal has them) in `dir`.
pub fn open_in_terminal(dir: &Path) {
    let terminal = tabula_kit::find_program("tabula-terminal").map_or_else(|| "tabula-terminal".into(), PathBuf::into_os_string);
    if let Err(e) = opener::spawn(&[terminal, "-w".into(), dir.as_os_str().to_owned()]) {
        tracing::warn!("terminal in {}: {e}", dir.display());
    }
}

pub fn open_in_viewer(path: &Path) {
    let viewer = tabula_kit::find_program("tabula-viewer").map_or_else(|| "tabula-viewer".into(), PathBuf::into_os_string);
    if let Err(e) = opener::spawn(&[viewer, path.as_os_str().to_owned()]) {
        tracing::warn!("viewer {}: {e}", path.display());
    }
}

/// Open a file with the right application: ours where we have one,
/// otherwise the user's default (see `tabula_xdg::open`). Programs run.
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
        assert_eq!(percent_decode("/home/a%20b%26c.txt"), b"/home/a b&c.txt");
        assert_eq!(percent_decode("/tmp/%C3%A9%"), "/tmp/é%".as_bytes());
    }
}
