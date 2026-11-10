//! AppImages, shown as themselves wherever Olo shows an application (Ben,
//! 2026-11-09: "if an appimage has an icon in it for the app show that
//! icon in the tracker, when running in the deskbar apps, and anywhere
//! else an icon is shown for the app").
//!
//! An AppImage (type 2) is an ELF program, its runtime, with a SquashFS
//! image after it (where the ELF's section headers end), and in that image
//! the app's desktop entry and its icon (`.DirIcon`, often a link to the
//! icon by its name). Both are read here, in process (`backhand`), for its
//! name (the entry's `Name`, else the file's name, versions trimmed), its
//! icon and the app id its windows take (the entry's file name and
//! `StartupWMClass`). What was read is kept in `~/.cache/olo/appimages`
//! (the icon, PNG or SVG as given, and an index by path, size and time),
//! so each version of an AppImage is read once, by whichever of Deskbar
//! (`~/AppImages`, [`scan`]) and Tracker (an AppImage it shows, [`info`])
//! comes to it first. A type 1 AppImage (ISO 9660), one being downloaded or
//! one that can't be read is known by its file name, without an icon.
//!
//! For each one known, a desktop entry is kept beside them
//! (`olo_xdg::desktop_entry::appimages_dir`, among the folders every
//! lookup of an app id reads, last): `NoDisplay` (they aren't installed,
//! and the menus list `~/AppImages` on their own), naming the AppImage to
//! run and the icon taken out of it. So a running AppImage's windows are
//! its app in Deskbar, the Twitcher, the window menu and notifications,
//! with its name and icon, through the same lookup as installed apps. A
//! window whose app id its entry doesn't say is matched by its process
//! ([`of_process`]: the runtime's `APPIMAGE` and `APPDIR`), and that id
//! kept for it ([`learn`]).

use std::{
    collections::HashMap,
    io::{BufReader, Read, Seek, SeekFrom, Write as _},
    os::unix::{
        ffi::OsStrExt as _,
        fs::{MetadataExt as _, PermissionsExt as _},
    },
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// One AppImage, as read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppImage {
    pub path: PathBuf,
    pub name: String,
    /// Its own icon, taken out of it (a file in the cache), if it has one.
    pub icon: Option<PathBuf>,
    /// Its desktop entry's id (its file name, "dbdelve"): the app id its
    /// windows usually take.
    #[serde(default)]
    pub id: Option<String>,
    /// Its entry's `StartupWMClass`.
    #[serde(default)]
    pub wm_class: Option<String>,
    /// How its entry's `Exec` takes files (`%f`, `%U`), if it does.
    #[serde(default)]
    pub takes: Option<String>,
    /// App ids its windows were seen with that its entry doesn't say
    /// ([`learn`]).
    #[serde(default)]
    pub seen: Vec<String>,
}

/// The folder Deskbar lists, `~/AppImages`.
pub fn dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("AppImages")
}

fn cache_dir() -> PathBuf {
    olo_xdg::cache_dir().join("appimages")
}

/// What was read of each AppImage, by path: its size and time then.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Index(HashMap<PathBuf, (u64, i64, AppImage)>);

const INDEX: &str = "apps.json";

fn load() -> Index {
    std::fs::read(cache_dir().join(INDEX)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// `path`'s, if what was read of it is of this version of it.
fn current(index: &Index, path: &Path, size: u64, time: i64) -> Option<AppImage> {
    match index.0.get(path) {
        Some((s, t, a)) if *s == size && *t == time && a.icon.as_ref().is_none_or(|i| i.is_file()) => Some(a.clone()),
        _ => None,
    }
}

/// The AppImage at `path` (or a link to one), as read: once for each
/// version of it, by whichever app comes to it first. None if it isn't
/// an AppImage. Reading one takes milliseconds: call it off the UI
/// thread.
pub fn info(path: &Path) -> Option<AppImage> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || !is_appimage(path) {
        return None;
    }
    let (size, time) = (meta.len(), meta.mtime());
    if let Some(a) = current(&load(), path, size, time) {
        return Some(a);
    }
    let app = read(path);
    update(|index| keep(index, path, size, time, app.clone()));
    Some(app)
}

/// Keep `app` as what `path` is now (the app ids seen of it before kept).
fn keep(index: &mut Index, path: &Path, size: u64, time: i64, mut app: AppImage) {
    if let Some((_, _, old)) = index.0.get(path) {
        app.seen.clone_from(&old.seen);
    }
    index.0.insert(path.to_owned(), (size, time, app));
}

/// The AppImages in `~/AppImages` (none if it isn't there), by name; read
/// as they're new or changed, else as kept.
pub fn scan() -> Option<Vec<AppImage>> {
    let entries = std::fs::read_dir(dir()).ok()?;
    let kept = load();
    let mut found = Vec::new();
    let mut fresh = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if !meta.is_file() || !is_appimage(&path) {
            continue;
        }
        let (size, time) = (meta.len(), meta.mtime());
        let app = current(&kept, &path, size, time).unwrap_or_else(|| {
            let app = read(&path);
            fresh.push((path.clone(), size, time, app.clone()));
            app
        });
        found.push(app);
    }
    // (Always: what's gone is let go of, and the entries made again if
    // the cache was cleared.)
    update(|index| {
        for (path, size, time, app) in fresh {
            keep(index, &path, size, time, app);
        }
    });
    found.sort_by_key(|a| a.name.to_lowercase());
    Some(found)
}

/// The AppImage the process `pid` is running from, if it is: its
/// runtime sets `APPIMAGE` (its path) and `APPDIR` (where its image is
/// mounted), and the process's program is in that image (not one an
/// AppImage started, which inherits them).
pub fn of_process(pid: i32) -> Option<PathBuf> {
    let env = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let var = |name: &[u8]| env.split(|b| *b == 0).find_map(|v| v.strip_prefix(name)).map(|v| PathBuf::from(std::ffi::OsStr::from_bytes(v)));
    let (image, appdir) = (var(b"APPIMAGE=")?, var(b"APPDIR=")?);
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    (image.is_absolute() && appdir.is_absolute() && appdir.components().count() > 1 && exe.starts_with(&appdir)).then_some(image)
}

/// Windows of the AppImage at `path` take app id `app_id`: its entry
/// says so from now on, if it didn't (a window with an id its own entry
/// doesn't give). Returns whether that's new.
pub fn learn(path: &Path, app_id: &str) -> bool {
    let Some(app) = info(path) else { return false };
    if !valid_id(app_id) || app.id.iter().chain(&app.wm_class).chain(&app.seen).any(|k| k.eq_ignore_ascii_case(app_id)) {
        return false;
    }
    tracing::info!("appimages: {} runs as {app_id}", path.display());
    update(|index| {
        if let Some((_, _, a)) = index.0.get_mut(path)
            && !a.seen.iter().any(|s| s == app_id)
        {
            a.seen.push(app_id.to_owned());
        }
    });
    true
}

/// A name a desktop entry's file can have.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && !id.starts_with('.') && !id.contains(['/', '\0', '\n'])
}

/// Change what's kept (under a lock, as Deskbar and Tracker both do),
/// let go of AppImages that are gone, and bring the entries and icons in
/// line with it.
fn update(change: impl FnOnce(&mut Index)) {
    let dir = cache_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("appimages: {}: {e}", dir.display());
        return;
    }
    let Ok(lock) = std::fs::File::options().create(true).truncate(false).write(true).open(dir.join("apps.lock")) else { return };
    // SAFETY: flock on a descriptor we own; released when `lock` closes.
    unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&lock), libc::LOCK_EX) };
    let before = load();
    let mut index = before.clone();
    change(&mut index);
    index.0.retain(|p, _| p.is_file());
    if index != before {
        let tmp = dir.join(format!("{INDEX}.{}", std::process::id()));
        if std::fs::write(&tmp, serde_json::to_vec(&index).unwrap_or_default()).and_then(|()| std::fs::rename(&tmp, dir.join(INDEX))).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    integrate(&index);
    drop(lock);
}

/// The desktop entries and icons kept for `index`: one entry for each
/// app id (its own, and those seen), the AppImage in `~/AppImages` first,
/// then the newest; others' taken away. Only what changed is written (the
/// apps following the folder read it again).
fn integrate(index: &Index) {
    let mut apps: Vec<&(u64, i64, AppImage)> = index.0.values().collect();
    let home = dir();
    apps.sort_by(|a, b| (!a.2.path.starts_with(&home), std::cmp::Reverse(a.1), &a.2.path).cmp(&(!b.2.path.starts_with(&home), std::cmp::Reverse(b.1), &b.2.path)));
    let mut wanted: HashMap<String, String> = HashMap::new();
    for (_, _, a) in apps {
        for id in a.id.iter().chain(&a.seen).filter(|id| valid_id(id)) {
            wanted.entry(id.clone()).or_insert_with(|| entry_text(a));
        }
    }
    let out = olo_xdg::desktop_entry::appimages_dir();
    if let Ok(rd) = std::fs::read_dir(&out) {
        for f in rd.flatten() {
            let keep = f.path().extension().is_some_and(|e| e == "desktop") && f.path().file_stem().and_then(|s| s.to_str()).is_some_and(|s| wanted.contains_key(s));
            if !keep {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
    if !wanted.is_empty() && std::fs::create_dir_all(&out).is_ok() {
        for (id, text) in &wanted {
            let path = out.join(format!("{id}.desktop"));
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                // (Whole or not at all: read as it's written.)
                let tmp = out.join(format!(".{id}.{}", std::process::id()));
                let wrote = std::fs::File::create(&tmp).and_then(|mut f| f.write_all(text.as_bytes())).and_then(|()| std::fs::rename(&tmp, &path));
                if let Err(e) = wrote {
                    tracing::warn!("appimages: {}: {e}", path.display());
                    let _ = std::fs::remove_file(&tmp);
                }
            }
        }
    }
    // Icons no AppImage has now (gone or changed), half-written ones, and
    // what was kept before the entries were (`index.json`), let go of;
    // not one just written (another app reading an AppImage, about to
    // keep it).
    let icons: Vec<&PathBuf> = index.0.values().filter_map(|(_, _, a)| a.icon.as_ref()).collect();
    if let Ok(rd) = std::fs::read_dir(cache_dir()) {
        for f in rd.flatten() {
            let (path, name) = (f.path(), f.file_name());
            let name = name.to_string_lossy();
            let ours = name.ends_with(".png") || name.ends_with(".svg") || name == "index.json" || (name.starts_with('.') && f.file_type().is_ok_and(|t| t.is_file()));
            let old = f.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() >= 60);
            if ours && old && !icons.contains(&&path) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// `app`'s desktop entry: not listed, it names the AppImage to run (as
/// the shell reads it: Olo runs `Exec` with `sh -c`) and its icon.
fn entry_text(app: &AppImage) -> String {
    let one_line = |s: &str| s.replace(['\n', '\r'], " ");
    let path = app.path.to_string_lossy();
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    let mut t = format!("[Desktop Entry]\nType=Application\nName={}\nExec={}", one_line(&app.name), one_line(&quoted));
    if let Some(code) = app.takes.as_deref() {
        t.push(' ');
        t.push_str(code);
    }
    t.push('\n');
    if let Some(icon) = &app.icon {
        t.push_str(&format!("Icon={}\n", one_line(&icon.to_string_lossy())));
    }
    if let Some(class) = &app.wm_class {
        t.push_str(&format!("StartupWMClass={}\n", one_line(class)));
    }
    t.push_str(&format!("NoDisplay=true\nX-Olo-AppImage={}\n", one_line(&path)));
    t
}

/// An AppImage by its name or its insides: `.AppImage`, or an ELF program
/// marked as one ("AI" and its type after the ELF's magic).
pub fn is_appimage(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("appimage")) {
        return true;
    }
    let mut head = [0u8; 11];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head[..4] == *b"\x7fELF" && head[8..10] == *b"AI"
}

/// Its name from its file's: the extension, and a version and machine
/// after it, gone ("Obsidian-1.5.12.AppImage": "Obsidian").
pub fn name_of_file(path: &Path) -> String {
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

/// Read what `path` says of itself (its entry, its icon, kept in the
/// cache); its file's name and no icon if it can't be read.
fn read(path: &Path) -> AppImage {
    let fallback = AppImage { path: path.to_owned(), name: name_of_file(path), icon: None, id: None, wm_class: None, takes: None, seen: Vec::new() };
    match inside(path) {
        Ok(found) => AppImage { name: found.name.unwrap_or(fallback.name), icon: found.icon, id: found.id, wm_class: found.wm_class, takes: found.takes, ..fallback },
        Err(e) => {
            tracing::debug!("appimages: {}: {e}", path.display());
            fallback
        }
    }
}

struct Inside {
    name: Option<String>,
    icon: Option<PathBuf>,
    id: Option<String>,
    wm_class: Option<String>,
    takes: Option<String>,
}

fn inside(path: &Path) -> Result<Inside, String> {
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
    let entry_path = nodes.keys().filter(|p| p.components().count() == 1 && p.extension().is_some_and(|e| e == "desktop")).min().cloned();
    let entry = entry_path.as_deref().and_then(bytes).map(|b| String::from_utf8_lossy(&b).into_owned());
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
    let id = entry.is_some().then(|| entry_path.as_deref().and_then(Path::file_stem).map(|s| s.to_string_lossy().into_owned())).flatten().filter(|s| valid_id(s));
    let wm_class = field("StartupWMClass").filter(|c| !c.is_empty());
    let takes = field("Exec").and_then(|e| e.split_whitespace().map(|p| p.trim_matches(['"', '\''])).find(|p| matches!(*p, "%f" | "%F" | "%u" | "%U")).map(str::to_owned));
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
        // (By what it is: copies of an AppImage share one.)
        let key = {
            use std::hash::{Hash as _, Hasher as _};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            b.hash(&mut h);
            h.finish()
        };
        let out = cache_dir().join(format!("{key:016x}.{ext}"));
        std::fs::create_dir_all(cache_dir()).ok()?;
        // (Whole or not at all: another app may be drawing it.)
        let tmp = cache_dir().join(format!(".{key:016x}.{}", std::process::id()));
        std::fs::write(&tmp, &b).and_then(|()| std::fs::rename(&tmp, &out)).ok()?;
        Some(out)
    });
    Ok(Inside { name, icon, id, wm_class, takes })
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

    /// The entry made for one names it to run (quoted as the shell reads
    /// it), its icon and its class, and isn't listed.
    #[test]
    fn entries_made() {
        let app = AppImage {
            path: "/home/x/App Images/My \"$App\".AppImage".into(),
            name: "My App".into(),
            icon: Some("/c/1.png".into()),
            id: Some("myapp".into()),
            wm_class: Some("MyApp".into()),
            takes: Some("%U".into()),
            seen: vec![],
        };
        let t = entry_text(&app);
        assert!(t.contains("Exec=\"/home/x/App Images/My \\\"\\$App\\\".AppImage\" %U\n"), "{t}");
        assert!(t.contains("Icon=/c/1.png\n") && t.contains("StartupWMClass=MyApp\n") && t.contains("NoDisplay=true\n"), "{t}");
        assert!(valid_id("dbdelve") && !valid_id("../x") && !valid_id(".hidden") && !valid_id(""));
    }

    /// An entry for each app id (its own and those seen), the one in
    /// `~/AppImages` first; let go of with its AppImage.
    #[test]
    fn entries_kept() {
        let root = PathBuf::from(format!("/var/tmp/olo-appimage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // SAFETY: the only test here that reads or sets them.
        unsafe {
            std::env::set_var("HOME", root.join("home"));
            std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        }
        let (a, b) = (root.join("home/AppImages/a.AppImage"), root.join("other/b.AppImage"));
        for p in [&a, &b] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let app = |path: &Path, seen: &[&str]| AppImage {
            path: path.to_owned(),
            name: "App".into(),
            icon: None,
            id: Some("app".into()),
            wm_class: None,
            takes: None,
            seen: seen.iter().map(|s| s.to_string()).collect(),
        };
        // (b the newer, but not in ~/AppImages.)
        update(|i| {
            i.0.insert(a.clone(), (1, 100, app(&a, &["app-window"])));
            i.0.insert(b.clone(), (1, 200, app(&b, &["b-only"])));
        });
        let dir = olo_xdg::desktop_entry::appimages_dir();
        let names = |id: &str| std::fs::read_to_string(dir.join(format!("{id}.desktop"))).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("X-Olo-AppImage=").map(PathBuf::from)));
        assert_eq!(names("app"), Some(a.clone()));
        assert_eq!(names("app-window"), Some(a.clone()));
        assert_eq!(names("b-only"), Some(b.clone()));
        // Found as an installed app is, by its id.
        assert_eq!(olo_xdg::desktop_entry::find("b-only").map(|e| e.no_display), Some(true));
        std::fs::remove_file(&a).unwrap();
        update(|_| {});
        assert_eq!(names("app"), Some(b.clone()));
        assert_eq!(names("app-window"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
