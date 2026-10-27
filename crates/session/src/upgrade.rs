//! Upgrades from a build tree. An installed session (tools/install-session.sh)
//! knows the tree it came from (`<prefix>/share/tabula/build-dir`, or
//! `TABULA_UPGRADE_FROM`) and watches it: once a build has finished and
//! left newer programs there, it lists them in a file Deskbar reads
//! (`$XDG_RUNTIME_DIR/tabula-upgrade-<display>.json`), which shows a tray
//! icon. Choosing to upgrade there signals us (SIGUSR2, or
//! `tabula-session upgrade`): we install the new programs beside ours and
//! reload in place.

use std::{
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};

/// How often to look.
const EVERY: Duration = Duration::from_secs(3);
/// A program modified more recently than this may still be being linked.
const SETTLE: Duration = Duration::from_secs(3);

/// What Deskbar reads.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Available {
    /// The programs that would change, by name.
    pub programs: Vec<String>,
}

pub fn state_path(display: &str) -> PathBuf {
    crate::runtime_dir().join(format!("tabula-upgrade-{display}.json"))
}

pub struct Watcher {
    from: PathBuf,
    to: PathBuf,
    state: PathBuf,
    pending: Vec<String>,
    last: Option<Instant>,
}

impl Watcher {
    /// Watch for programs newer than ours (in `to`), if we know where
    /// they're built.
    pub fn new(to: &Path, display: &str) -> Option<Watcher> {
        let from = std::env::var_os("TABULA_UPGRADE_FROM").map(PathBuf::from).or_else(|| {
            let file = to.parent()?.join("share/tabula/build-dir");
            Some(PathBuf::from(std::fs::read_to_string(file).ok()?.trim()))
        })?;
        // A development session runs from the build tree itself.
        if same_dir(&from, to) {
            return None;
        }
        crate::log(format!("watching {} for upgrades", from.display()));
        Some(Watcher { from, to: to.to_owned(), state: state_path(display), pending: Vec::new(), last: None })
    }

    /// Look again (now and then), and tell Deskbar if what's on offer changed.
    pub fn check(&mut self) {
        if self.last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.last = Some(Instant::now());
        if building(&self.from) {
            return;
        }
        let Some(now) = newer(&self.from, &self.to) else { return };
        if now != self.pending {
            self.pending = now;
            self.publish();
        }
    }

    fn publish(&self) {
        if self.pending.is_empty() {
            let _ = std::fs::remove_file(&self.state);
            return;
        }
        crate::log(format!("upgrade available: {}", self.pending.join(", ")));
        let json = serde_json::to_string(&Available { programs: self.pending.clone() }).unwrap_or_default();
        let tmp = self.state.with_extension("new");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.state);
        }
    }

    /// Install what's newer; the programs that were.
    pub fn install(&mut self) -> Vec<String> {
        if building(&self.from) {
            crate::log("a build is still running; not upgrading yet");
            return Vec::new();
        }
        let programs = newer(&self.from, &self.to).unwrap_or_default();
        let mut done = Vec::new();
        for name in &programs {
            match install(&self.from.join(name), &self.to, name) {
                Ok(()) => done.push(name.clone()),
                Err(e) => crate::log(format!("could not install {name}: {e}")),
            }
        }
        crate::log(format!("installed {} of {} upgraded programs", done.len(), programs.len()));
        // New and redrawn icons come with them (a new panel's, say).
        if let Some(prefix) = self.to.parent() {
            match sync_icons(&self.from, &prefix.join("share/tabula/icons")) {
                Ok(0) => {}
                Ok(n) => crate::log(format!("installed {n} new or changed icons")),
                Err(e) => crate::log(format!("could not install icons: {e}")),
            }
            match sync_wallpapers(&self.from, &prefix.join("share/tabula/wallpapers")) {
                Ok(0) => {}
                Ok(n) => crate::log(format!("installed {n} new or changed wallpapers")),
                Err(e) => crate::log(format!("could not install wallpapers: {e}")),
            }
        }
        self.pending = newer(&self.from, &self.to).unwrap_or_default();
        self.publish();
        done
    }

    /// Tell our programs still running from files just replaced in the
    /// install directory (the kit re-executes them in place, with their
    /// windows). Leaves out `skip` (us and app_server).
    pub fn nudge(&self, skip: &[i32]) {
        // SAFETY: getuid cannot fail.
        let uid = unsafe { libc::getuid() };
        let Ok(dir) = std::fs::read_dir("/proc") else { return };
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            if skip.contains(&pid) || std::fs::metadata(e.path()).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).ok() != Some(uid) {
                continue;
            }
            if let Ok(exe) = std::fs::read_link(e.path().join("exe"))
                && replaced(&exe, &self.to)
            {
                crate::log(format!("asking {} (pid {pid}) to upgrade", exe.display()));
                // SAFETY: SIGURG (ignored unless handled) to our own program.
                unsafe { libc::kill(pid, libc::SIGURG) };
            }
        }
    }

    pub fn forget(&self) {
        let _ = std::fs::remove_file(&self.state);
    }
}

/// Was the program at `exe` (as /proc shows it) one of ours in `dir`
/// whose file has since been replaced?
fn replaced(exe: &Path, dir: &Path) -> bool {
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else { return false };
    let Some(name) = name.strip_suffix(" (deleted)") else { return false };
    name.starts_with("tabula-") && exe.parent().is_some_and(|p| same_dir(p, dir))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Is cargo building there? (It holds one of its locks while it does:
/// `.cargo-lock`, or in newer cargo `.cargo-build-lock` and
/// `.cargo-artifact-lock`.)
fn building(from: &Path) -> bool {
    [".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"].iter().any(|name| {
        let Ok(lock) = std::fs::File::open(from.join(name)) else { return false };
        // SAFETY: flock on a file we own; released at once.
        unsafe {
            if libc::flock(lock.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 {
                return true;
            }
            libc::flock(lock.as_raw_fd(), libc::LOCK_UN);
        }
        false
    })
}

/// Our programs in `from` that are newer than (or missing from) `to`;
/// None while one is too fresh to trust.
fn newer(from: &Path, to: &Path) -> Option<Vec<String>> {
    let now = SystemTime::now();
    let mut out = Vec::new();
    for e in std::fs::read_dir(from).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("tabula-") || name.contains('.') {
            continue;
        }
        let Ok(m) = e.metadata() else { continue };
        if !m.is_file() || m.permissions().mode() & 0o111 == 0 {
            continue;
        }
        let Ok(built) = m.modified() else { continue };
        if now.duration_since(built).unwrap_or_default() < SETTLE {
            return None;
        }
        let installed = std::fs::metadata(to.join(&name)).and_then(|m| m.modified()).ok();
        if installed.is_none_or(|i| built > i) {
            out.push(name);
        }
    }
    out.sort();
    Some(out)
}

/// Copy the build tree's icons (`data/icons`: the sizes and the manifest,
/// found above `from`, the build's output directory) into `to` where
/// they're new or newer; how many.
fn sync_icons(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/icons")).find(|d| d.join("manifest.txt").is_file()) else {
        return Ok(0);
    };
    let newer = |a: &Path, b: &Path| {
        let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        m(b).is_none_or(|b| m(a).is_some_and(|a| a > b))
    };
    let mut n = 0;
    std::fs::create_dir_all(to)?;
    if newer(&src.join("manifest.txt"), &to.join("manifest.txt")) {
        std::fs::copy(src.join("manifest.txt"), to.join("manifest.txt"))?;
        n += 1;
    }
    for e in std::fs::read_dir(&src)?.flatten() {
        let name = e.file_name();
        // The pre-scaled sizes (16, 24, …); not the raw sources.
        if !name.to_str().is_some_and(|n| n.parse::<u32>().is_ok()) || !e.path().is_dir() {
            continue;
        }
        let dir = to.join(&name);
        std::fs::create_dir_all(&dir)?;
        for f in std::fs::read_dir(e.path())?.flatten() {
            let (a, b) = (f.path(), dir.join(f.file_name()));
            if a.extension().is_some_and(|x| x == "png") && newer(&a, &b) {
                let tmp = dir.join(format!(".{}.new", f.file_name().to_string_lossy()));
                std::fs::copy(&a, &tmp)?;
                std::fs::rename(&tmp, &b)?;
                n += 1;
            }
        }
    }
    Ok(n)
}

/// Copy the build tree's wallpaper pack (`data/wallpapers`: the manifest,
/// both shapes and the previews) into `to` where new or newer, by clone
/// where the file system can (the pack is ~90 MB); how many files.
fn sync_wallpapers(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/wallpapers")).find(|d| d.join("pack.txt").is_file()) else {
        return Ok(0);
    };
    let newer = |a: &Path, b: &Path| {
        let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        m(b).is_none_or(|b| m(a).is_some_and(|a| a > b))
    };
    let mut n = 0;
    for sub in ["", "4k", "ultrawide", "thumbs"] {
        let (from_dir, to_dir) = (src.join(sub), to.join(sub));
        let Ok(entries) = std::fs::read_dir(&from_dir) else { continue };
        std::fs::create_dir_all(&to_dir)?;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_file() && (name.ends_with(".png") || name == "pack.txt") && newer(&e.path(), &to_dir.join(&name)) {
                install(&e.path(), &to_dir, &name)?;
                std::fs::set_permissions(to_dir.join(&name), std::fs::Permissions::from_mode(0o644))?;
                n += 1;
            }
        }
    }
    Ok(n)
}

/// Put `src` in `dir` as `name`: a clone of the file where the file system
/// can (btrfs, XFS: instant, no space), else a copy, under a temporary
/// name renamed into place, so running programs keep their old file.
fn install(src: &Path, dir: &Path, name: &str) -> std::io::Result<()> {
    let tmp = dir.join(format!(".{name}.new"));
    let _ = std::fs::remove_file(&tmp);
    let from = std::fs::File::open(src)?;
    let to = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    // SAFETY: FICLONE between two files we hold open.
    let cloned = unsafe { libc::ioctl(to.as_raw_fd(), libc::FICLONE, from.as_raw_fd()) } == 0;
    drop(to);
    if !cloned {
        std::fs::copy(src, &tmp)?;
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, dir.join(name)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(dir: &Path, name: &str, age: u64) {
        let p = dir.join(name);
        std::fs::write(&p, name).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn icons_come_along() {
        let base = std::env::temp_dir().join(format!("tabula-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let build = base.join("repo/target/release");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::create_dir_all(base.join("repo/data/icons/16")).unwrap();
        std::fs::create_dir_all(base.join("repo/data/icons/src")).unwrap();
        std::fs::write(base.join("repo/data/icons/manifest.txt"), "lock|a padlock\n").unwrap();
        std::fs::write(base.join("repo/data/icons/16/lock.png"), "png").unwrap();
        std::fs::write(base.join("repo/data/icons/src/lock.png"), "raw").unwrap();
        let to = base.join("prefix/share/tabula/icons");
        assert_eq!(sync_icons(&build, &to).unwrap(), 2);
        assert!(to.join("16/lock.png").is_file() && !to.join("src").exists());
        // Nothing new: nothing copied.
        assert_eq!(sync_icons(&build, &to).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn finds_and_installs_newer_programs() {
        let root = std::env::temp_dir().join(format!("tabula-upgrade-test-{}", std::process::id()));
        let (from, to) = (root.join("build"), root.join("bin"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        program(&from, "tabula-a", 100);
        program(&from, "tabula-b", 100);
        program(&from, "tabula-b.d", 100);
        std::fs::write(from.join("tabula-c"), "not a program").unwrap();
        program(&to, "tabula-a", 50);
        program(&to, "tabula-b", 200);
        assert_eq!(newer(&from, &to), Some(vec!["tabula-b".to_string()]));
        install(&from.join("tabula-b"), &to, "tabula-b").unwrap();
        assert_eq!(std::fs::read_to_string(to.join("tabula-b")).unwrap(), "tabula-b");
        assert_eq!(newer(&from, &to), Some(vec![]));
        // Fresh from the linker: wait.
        program(&from, "tabula-a", 0);
        assert_eq!(newer(&from, &to), None);
        assert!(replaced(&to.join("tabula-b (deleted)"), &to));
        assert!(!replaced(&to.join("tabula-b"), &to));
        assert!(!replaced(&from.join("tabula-b (deleted)"), &to));
        assert!(!replaced(&to.join("ptyxis (deleted)"), &to));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
