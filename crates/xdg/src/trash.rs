//! The freedesktop Trash (`$XDG_DATA_HOME/Trash`): reserving a place for
//! an item with its .trashinfo, and finding where one came from. Shared
//! by Tracker and DiskUsage; the moving itself is theirs (Tracker copies
//! across volumes with progress).

use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn trash_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())).join(".local/share")).join("Trash")
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
    let text = fs::read_to_string(&info).map_err(|_| std::io::Error::other(format!("there’s no record of where “{}” came from", name.to_string_lossy())))?;
    let path = text.lines().find_map(|l| l.strip_prefix("Path=")).ok_or_else(|| std::io::Error::other(format!("the record of where “{}” came from is damaged", name.to_string_lossy())))?;
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
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// Move `path` into the Trash when it is on the same volume as the Trash
/// (a rename, so instant); an error otherwise.
pub fn move_to_trash(path: &Path) -> std::io::Result<PathBuf> {
    let (target, info) = trash_slot(path)?;
    match fs::rename(path, &target) {
        Ok(()) => Ok(target),
        Err(e) => {
            let _ = fs::remove_file(&info);
            Err(e)
        }
    }
}

/// Put `item` (in the Trash) back where it came from, by its .trashinfo:
/// its folder made again if it has gone, and a number added to its name
/// if something new has it now. Where it went.
pub fn put_back(item: &Path) -> std::io::Result<PathBuf> {
    if fs::symlink_metadata(item).is_err() {
        return Err(std::io::Error::other(format!("“{}” isn't in the Trash any more", item.file_name().unwrap_or_default().to_string_lossy())));
    }
    let (origin, info) = trash_origin(item)?;
    let dir = origin.parent().ok_or_else(|| std::io::Error::other("no folder to put it back in"))?;
    fs::create_dir_all(dir)?;
    let target = unique(dir, &origin.file_name().unwrap_or_default().to_string_lossy());
    fs::rename(item, &target)?;
    let _ = fs::remove_file(&info);
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trashinfo_paths_are_escaped() {
        assert_eq!(percent_encode(Path::new("/home/a b&c.txt")), "/home/a%20b%26c.txt");
        assert_eq!(percent_encode(Path::new("/tmp/é")), "/tmp/%C3%A9");
        assert_eq!(percent_decode("/home/a%20b%26c.txt"), b"/home/a b&c.txt");
        assert_eq!(percent_decode("/tmp/%C3%A9%"), "/tmp/é%".as_bytes());
    }
}
