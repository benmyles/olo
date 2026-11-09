//! AppImages in `~/AppImages`, in the Olo menu (Ben, 2026-11-09: "if the
//! user creates a ~/AppImages dir any app images they put in there should
//! show up automatically in olo deskbar menu under a new AppImages menu
//! item (under Applications)", "the apps in there should update in real
//! time", "if there is a way to extract an icon from an app image make
//! sure it does that and uses that icon").
//!
//! An AppImage (type 2) is an ELF program, its runtime, with a SquashFS
//! image after it (where the ELF's section headers end), and in that image
//! the app's desktop entry and its icon (`.DirIcon`, often a link to the
//! icon by its name). Both are read here, in process (`backhand`), for its
//! name (the entry's `Name`, else the file's name, versions trimmed) and
//! its icon, kept in `~/.cache/olo/appimages` (PNG or SVG, as given) with
//! what was read of each, by its path, size and time: an AppImage is read
//! once, not at every scan. A type 1 AppImage (ISO 9660), one being
//! downloaded or one that can't be read is listed by its file name with the
//! generic icon. Deskbar follows the folder (inotify, `olo_ui::watch`:
//! there or not, made later), and lists it again as it changes.

use std::{
    collections::HashMap,
    io::{BufReader, Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// One AppImage, as the menu shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppImage {
    pub path: PathBuf,
    pub name: String,
    /// Its own icon, taken out of it (a file in the cache), if it has one.
    pub icon: Option<PathBuf>,
}

/// The folder, `~/AppImages`.
pub fn dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("AppImages")
}

fn cache_dir() -> PathBuf {
    olo_xdg::cache_dir().join("appimages")
}

/// What was read of each AppImage, by path: its size and time then.
#[derive(Default, Serialize, Deserialize)]
struct Index(HashMap<PathBuf, (u64, i64, AppImage)>);

/// The AppImages in the folder (none if it isn't there), by name; read
/// as they're new or changed, else as kept.
pub fn scan() -> Option<Vec<AppImage>> {
    let dir = dir();
    let entries = std::fs::read_dir(&dir).ok()?;
    let index_path = cache_dir().join("index.json");
    let kept: Index = std::fs::read(&index_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut now = Index::default();
    for e in entries.flatten() {
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() || !is_appimage(&path) {
            continue;
        }
        let (size, time) = (meta.len(), meta.mtime());
        let app = match kept.0.get(&path) {
            Some((s, t, a)) if *s == size && *t == time && a.icon.as_ref().is_none_or(|i| i.is_file()) => a.clone(),
            _ => read(&path, size, time),
        };
        now.0.insert(path, (size, time, app));
    }
    if now.0.len() != kept.0.len() || now.0.iter().any(|(p, v)| kept.0.get(p).is_none_or(|k| k.0 != v.0 || k.1 != v.1)) {
        let _ = std::fs::create_dir_all(cache_dir());
        let _ = std::fs::write(&index_path, serde_json::to_vec(&now).unwrap_or_default());
        // (Icons of AppImages gone or changed: theirs no more.)
        let wanted: Vec<&PathBuf> = now.0.values().filter_map(|(_, _, a)| a.icon.as_ref()).collect();
        if let Ok(rd) = std::fs::read_dir(cache_dir()) {
            for f in rd.flatten().map(|f| f.path()).filter(|f| f.file_name().is_some_and(|n| n != "index.json")) {
                if !wanted.contains(&&f) {
                    let _ = std::fs::remove_file(f);
                }
            }
        }
    }
    let mut out: Vec<AppImage> = now.0.into_values().map(|(_, _, a)| a).collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Some(out)
}

/// An AppImage by its name or its insides: `.AppImage`, or an ELF program
/// marked as one ("AI" and its type after the ELF's magic).
fn is_appimage(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("appimage")) {
        return true;
    }
    let mut head = [0u8; 11];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head[..4] == *b"\x7fELF" && head[8..10] == *b"AI"
}

/// Its name from its file's: the extension, and a version and machine
/// after it, gone ("Obsidian-1.5.12.AppImage": "Obsidian").
fn name_of_file(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("appimage")) { stem } else { path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() };
    let b = stem.as_bytes();
    let cut = (1..b.len()).find(|&i| {
        matches!(b[i - 1], b'-' | b'_' | b' ' | b'.')
            && (b[i].is_ascii_digit()
                || (b[i] == b'v' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
                || stem[i..].starts_with("x86")
                || stem[i..].starts_with("aarch64")
                || stem[i..].starts_with("amd64"))
    });
    let name = cut.map_or(stem.as_str(), |i| &stem[..i - 1]).replace(['_'], " ");
    if name.trim().is_empty() { stem } else { name.trim().to_owned() }
}

/// Where its SquashFS image starts: after the ELF's section headers.
fn squashfs_offset(f: &mut std::fs::File) -> Option<u64> {
    let mut h = [0u8; 64];
    f.read_exact(&mut h).ok()?;
    if h[..4] != *b"\x7fELF" || h[5] != 1 {
        return None;
    }
    let u16_at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]) as u64;
    let offset = match h[4] {
        // 64-bit: e_shoff at 0x28, e_shentsize 0x3A, e_shnum 0x3C.
        2 => u64::from_le_bytes(h[0x28..0x30].try_into().ok()?) + u16_at(0x3A) * u16_at(0x3C),
        // 32-bit: at 0x20, 0x2E, 0x30.
        1 => u32::from_le_bytes(h[0x20..0x24].try_into().ok()?) as u64 + u16_at(0x2E) * u16_at(0x30),
        _ => return None,
    };
    let mut magic = [0u8; 4];
    f.seek(SeekFrom::Start(offset)).ok()?;
    f.read_exact(&mut magic).ok()?;
    (magic == *b"hsqs").then_some(offset)
}

/// Read what `path` says of itself (its entry's name, its icon, kept in
/// the cache); its file's name and no icon if it can't be read.
fn read(path: &Path, size: u64, time: i64) -> AppImage {
    let fallback = AppImage { path: path.to_owned(), name: name_of_file(path), icon: None };
    match inside(path, size, time) {
        Ok((name, icon)) => AppImage { name: name.unwrap_or(fallback.name), icon, ..fallback },
        Err(e) => {
            tracing::debug!("appimages: {}: {e}", path.display());
            fallback
        }
    }
}

fn inside(path: &Path, size: u64, time: i64) -> Result<(Option<String>, Option<PathBuf>), String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let offset = squashfs_offset(&mut f).ok_or("no SquashFS image (a type 1 AppImage, or one not all here yet)")?;
    let fs = backhand::FilesystemReader::from_reader_with_offset(BufReader::new(f), offset).map_err(|e| e.to_string())?;
    // Each node by its path in the image ("usr/share/…", no leading "/").
    let nodes: HashMap<PathBuf, &backhand::Node<backhand::SquashfsFileReader>> = fs.files().map(|n| (n.fullpath.strip_prefix("/").unwrap_or(&n.fullpath).to_owned(), n)).collect();
    // A file's bytes, following links (in the image) a few deep.
    let bytes = |p: &Path| -> Option<Vec<u8>> {
        let mut at = p.to_owned();
        for _ in 0..8 {
            match &nodes.get(&at)?.inner {
                backhand::InnerNode::File(file) => {
                    let mut out = Vec::new();
                    fs.file(file).reader().take(16 << 20).read_to_end(&mut out).ok()?;
                    return Some(out);
                }
                backhand::InnerNode::Symlink(s) => {
                    let to = if s.link.is_absolute() { s.link.strip_prefix("/").ok()?.to_owned() } else { at.parent().unwrap_or(Path::new("")).join(&s.link) };
                    at = normalize(&to);
                }
                _ => return None,
            }
        }
        None
    };
    // Its desktop entry: at the top of the image.
    let entry = nodes.keys().filter(|p| p.components().count() == 1 && p.extension().is_some_and(|e| e == "desktop")).min().and_then(|p| bytes(p)).map(|b| String::from_utf8_lossy(&b).into_owned());
    let field = |key: &str| {
        entry.as_deref().and_then(|t| {
            t.lines()
                .skip_while(|l| l.trim() != "[Desktop Entry]")
                .skip(1)
                .take_while(|l| !l.starts_with('['))
                .find_map(|l| l.strip_prefix(key)?.trim_start().strip_prefix('=').map(|v| v.trim().to_owned()))
        })
    };
    let name = field("Name").filter(|n| !n.is_empty());
    // Its icon: `.DirIcon`; else the entry's by its name, at the top or in
    // the image's hicolor theme, the largest.
    let icon_name = field("Icon").unwrap_or_default();
    let mut icon = bytes(Path::new(".DirIcon"));
    if icon.is_none() && !icon_name.is_empty() {
        let mut found: Vec<(u32, PathBuf)> = nodes
            .keys()
            .filter(|p| p.file_stem().is_some_and(|s| s == icon_name.as_str()) && p.extension().is_some_and(|e| e == "png" || e == "svg"))
            .map(|p| {
                let px = if p.extension().is_some_and(|e| e == "svg") { u32::MAX } else { p.components().find_map(|c| c.as_os_str().to_str()?.split('x').next()?.parse().ok()).unwrap_or(0) };
                (px, p.clone())
            })
            .collect();
        found.sort();
        icon = found.last().and_then(|(_, p)| bytes(p));
    }
    let icon = icon.and_then(|b| {
        let ext = if b.starts_with(b"\x89PNG") {
            "png"
        } else if b.windows(4).take(512).any(|w| w == b"<svg") {
            "svg"
        } else {
            return None;
        };
        let key = {
            use std::hash::{Hash as _, Hasher as _};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (path, size, time).hash(&mut h);
            h.finish()
        };
        let out = cache_dir().join(format!("{key:016x}.{ext}"));
        std::fs::create_dir_all(cache_dir()).ok()?;
        std::fs::write(&out, &b).ok()?;
        Some(out)
    });
    Ok((name, icon))
}

/// `a/./b/../c` as `a/c` (a link's target within the image).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(n) => out.push(n),
            _ => {}
        }
    }
    out
}

/// Start it, made executable first if it wasn't (one just downloaded):
/// it's in the AppImages folder to be run.
pub fn launch(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(path)?;
    let mode = meta.permissions().mode();
    if mode & 0o100 == 0 {
        tracing::info!("appimages: {} made executable, to run", path.display());
        // (Executable by whoever may read it.)
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | ((mode & 0o444) >> 2)))?;
    }
    use std::os::unix::process::CommandExt as _;
    std::process::Command::new(path).current_dir(std::env::var_os("HOME").unwrap_or_else(|| "/".into())).stdin(std::process::Stdio::null()).process_group(0).spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_files() {
        assert_eq!(name_of_file(Path::new("/x/Obsidian-1.5.12.AppImage")), "Obsidian");
        assert_eq!(name_of_file(Path::new("/x/LM_Studio-0.3.5-x86_64.AppImage")), "LM Studio");
        assert_eq!(name_of_file(Path::new("/x/cursor-x86_64.AppImage")), "cursor");
        assert_eq!(name_of_file(Path::new("/x/Joplin-v3.0.15.AppImage")), "Joplin");
        assert_eq!(name_of_file(Path::new("/x/Krita.appimage")), "Krita");
        assert_eq!(normalize(Path::new("usr/share/../share/./icons/a.png")), PathBuf::from("usr/share/icons/a.png"));
    }
}
