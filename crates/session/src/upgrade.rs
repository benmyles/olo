//! Upgrades from a build tree. An installed session (tools/install-session.sh)
//! knows the tree it came from (`<prefix>/share/olo/build-dir`, or
//! `OLO_UPGRADE_FROM`) and watches it: once a build has finished and
//! left newer programs there, it lists them in a file Deskbar reads
//! (`$XDG_RUNTIME_DIR/olo-upgrade-<display>.json`), which shows a tray
//! icon. Choosing to upgrade there signals us (SIGUSR2, or
//! `olo-session upgrade`): we install the new programs beside ours and
//! reload in place. Asked while a build is running, we say so in the
//! same file (`waiting`) and upgrade as soon as it has finished.

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
    /// Upgrading was asked for while a build was running: it happens
    /// once the build has finished.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waiting: bool,
}

pub fn state_path(display: &str) -> PathBuf {
    crate::runtime_dir().join(format!("olo-upgrade-{display}.json"))
}

pub struct Watcher {
    from: PathBuf,
    to: PathBuf,
    state: PathBuf,
    pending: Vec<String>,
    last: Option<Instant>,
    /// Asked to upgrade while a build was running.
    wanted: bool,
}

impl Watcher {
    /// Watch for programs newer than ours (in `to`), if we know where
    /// they're built.
    pub fn new(to: &Path, display: &str) -> Option<Watcher> {
        let from = std::env::var_os("OLO_UPGRADE_FROM").map(PathBuf::from).or_else(|| {
            let file = to.parent()?.join("share/olo/build-dir");
            Some(PathBuf::from(std::fs::read_to_string(file).ok()?.trim()))
        })?;
        // A development session runs from the build tree itself.
        if same_dir(&from, to) {
            return None;
        }
        crate::log(format!("watching {} for upgrades", from.display()));
        Some(Watcher { from, to: to.to_owned(), state: state_path(display), pending: Vec::new(), last: None, wanted: false })
    }

    /// Look again (now and then), and tell Deskbar if what's on offer
    /// changed; true when an upgrade asked for during a build can go
    /// ahead now that it has finished.
    pub fn check(&mut self) -> bool {
        if self.last.is_some_and(|t| t.elapsed() < EVERY) {
            return false;
        }
        self.last = Some(Instant::now());
        if building(&self.from) {
            return false;
        }
        // (None while a program it left is too fresh to trust.)
        let Some(now) = newer(&self.from, &self.to) else { return false };
        if now != self.pending {
            self.pending = now;
            self.publish();
        }
        std::mem::take(&mut self.wanted)
    }

    fn publish(&self) {
        if self.pending.is_empty() && !self.wanted {
            let _ = std::fs::remove_file(&self.state);
            return;
        }
        if self.wanted {
            crate::log("upgrading once the build has finished");
        } else {
            crate::log(format!("upgrade available: {}", self.pending.join(", ")));
        }
        let json = serde_json::to_string(&Available { programs: self.pending.clone(), waiting: self.wanted }).unwrap_or_default();
        let tmp = self.state.with_extension("new");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.state);
        }
    }

    /// Install what's newer; the programs that were.
    pub fn install(&mut self) -> Vec<String> {
        if building(&self.from) {
            // Asked for now, done when it has finished (`check`), and
            // said so meanwhile.
            crate::log("a build is still running; upgrading as soon as it has finished");
            self.wanted = true;
            self.publish();
            return Vec::new();
        }
        self.wanted = false;
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
            match sync_icons(&self.from, &prefix.join("share/olo/icons")) {
                Ok(0) => {}
                Ok(n) => crate::log(format!("installed {n} new or changed icons")),
                Err(e) => crate::log(format!("could not install icons: {e}")),
            }
            match sync_brand(&self.from, &prefix.join("share/olo/brand")) {
                Ok(0) => {}
                Ok(n) => crate::log(format!("installed {n} new or changed brand pictures")),
                Err(e) => crate::log(format!("could not install the brand pictures: {e}")),
            }
            match sync_fonts(&self.from, &prefix.join("share/olo/fonts")) {
                Ok(0) => {}
                Ok(n) => crate::log(format!("installed {n} new or changed fonts")),
                Err(e) => crate::log(format!("could not install the fonts: {e}")),
            }
            match sync_themes(&self.from, &prefix.join("share/olo/themes")) {
                Ok(0) => {}
                Ok(n) => crate::log(format!("installed {n} new or changed theme pictures")),
                Err(e) => crate::log(format!("could not install the themes: {e}")),
            }
            match sync_wallpapers(&self.from, &prefix.join("share/olo/wallpapers")) {
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

    /// The session's programs (by `display` in their environment) still
    /// running a binary of ours that was replaced, by pid and name, but
    /// for `skip`: what an upgrade left behind (`Session::check_stale`).
    pub fn stale(&self, skip: &[i32], display: &str) -> Vec<(i32, String)> {
        let uid = unsafe { libc::getuid() };
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir("/proc") else { return out };
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            if skip.contains(&pid) || std::fs::metadata(e.path()).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).ok() != Some(uid) {
                continue;
            }
            let Ok(exe) = std::fs::read_link(e.path().join("exe")) else { continue };
            if !replaced(&exe, &self.to) {
                continue;
            }
            let Ok(env) = std::fs::read(e.path().join("environ")) else { continue };
            let wanted = format!("WAYLAND_DISPLAY={display}");
            if !env.split(|&b| b == 0).any(|v| v == wanted.as_bytes()) {
                continue;
            }
            let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or_default().trim_end_matches(" (deleted)").to_owned();
            out.push((pid, name));
        }
        out.sort();
        out
    }

    pub fn forget(&self) {
        let _ = std::fs::remove_file(&self.state);
    }
}

/// Programs of ours left from a session that has gone (Ben, 2026-11-05:
/// "make the upgrade process identify any zombies that have been leftover
/// from past upgrades and kill them"): the user's processes running one
/// of our programs whose display (`WAYLAND_DISPLAY`, its socket in
/// `runtime`) no longer exists, a session's or a test session's that
/// ended without them. Not one younger than 30 s (a session starting),
/// nor one with no display in its environment (a coding agent's MCP
/// server, a service), nor `skip`. Each with its start time (so it's
/// killed later only if it's still the same process) and why.
pub fn leftovers(runtime: &Path, skip: &[i32]) -> Vec<(i32, u64, String)> {
    leftovers_aged(runtime, skip, 30.0)
}

/// [`leftovers`] at least `min_age` seconds old.
fn leftovers_aged(runtime: &Path, skip: &[i32], min_age: f64) -> Vec<(i32, u64, String)> {
    let uid = unsafe { libc::getuid() };
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else { return out };
    let uptime = std::fs::read_to_string("/proc/uptime").ok().and_then(|u| u.split_whitespace().next()?.parse::<f64>().ok()).unwrap_or(0.0);
    // SAFETY: sysconf can't fail for this.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        if skip.contains(&pid) || std::fs::metadata(e.path()).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).ok() != Some(uid) {
            continue;
        }
        let Ok(exe) = std::fs::read_link(e.path().join("exe")) else { continue };
        let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or_default().trim_end_matches(" (deleted)").to_owned();
        if !(name.starts_with("olo-") || name.starts_with("tabula-")) {
            continue;
        }
        let Some(started) = start_time(pid) else { continue };
        if uptime - started as f64 / hz < min_age {
            continue;
        }
        let Ok(env) = std::fs::read(e.path().join("environ")) else { continue };
        let Some(display) = env.split(|&b| b == 0).find_map(|v| v.strip_prefix(b"WAYLAND_DISPLAY=")).map(|d| String::from_utf8_lossy(d).into_owned()).filter(|d| !d.is_empty()) else { continue };
        let socket = if display.starts_with('/') { PathBuf::from(&display) } else { runtime.join(&display) };
        if socket.exists() {
            continue;
        }
        out.push((pid, started, format!("{name} (pid {pid}): its display {display} has gone")));
    }
    out.sort();
    out
}

/// When process `pid` started (clock ticks after boot), to know it again.
pub fn start_time(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

/// The program that started `pid`, if it isn't one of ours (nor the
/// session's service manager, nor init, which adopt what's started
/// detached): a coding agent running an MCP server of ours, a browser
/// its messaging host, a shell a command. Stopping such a process breaks
/// what that program had open with it, and it won't start it again (an
/// agent's tool was gone for the rest of its session): left alone.
pub fn started_by_another(pid: i32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let ppid: i32 = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
    if ppid <= 1 {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{ppid}/comm")).ok()?.trim().to_owned();
    let exe = std::fs::read_link(format!("/proc/{ppid}/exe")).ok()?;
    let name = exe.file_name()?.to_str()?.trim_end_matches(" (deleted)");
    let ours = name.starts_with("olo-") || name.starts_with("tabula-") || name == "app_server";
    (!ours && comm != "systemd").then_some(comm)
}

/// Was the program at `exe` (as /proc shows it) one of ours in `dir`
/// whose file has since been replaced?
fn replaced(exe: &Path, dir: &Path) -> bool {
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else { return false };
    let Some(name) = name.strip_suffix(" (deleted)") else { return false };
    // A program from before the rename is still running as `tabula-…`.
    (name.starts_with("olo-") || name.starts_with("tabula-")) && exe.parent().is_some_and(|p| same_dir(p, dir))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Where `dir` has an old program (`tabula-X`) and its new one
/// (`olo-X`), make the old name a link to the new (so whatever runs it by
/// the old name, a login screen's entry or a program re-executing itself,
/// gets the new program). The links replace the files in one step; what
/// was running from them carries on until it re-executes.
pub fn link_old_names(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(old) = olo_xdg::legacy::old_program(&name) else { continue };
        if name.contains('.') || !e.path().is_file() {
            continue;
        }
        let old_path = dir.join(&old);
        let Ok(m) = std::fs::symlink_metadata(&old_path) else { continue };
        if m.file_type().is_symlink() && std::fs::read_link(&old_path).is_ok_and(|t| t == Path::new(&name)) {
            continue;
        }
        let tmp = dir.join(format!(".{old}.link"));
        let _ = std::fs::remove_file(&tmp);
        let made = std::os::unix::fs::symlink(&name, &tmp).and_then(|()| std::fs::rename(&tmp, &old_path));
        match made {
            Ok(()) => crate::log(format!("{old} is now a link to {name}")),
            Err(e) => crate::log(format!("could not link {old} to {name}: {e}")),
        }
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
        if !name.starts_with("olo-") || name.contains('.') {
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
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    if is_newer(&src.join("manifest.txt"), &to.join("manifest.txt")) {
        std::fs::copy(src.join("manifest.txt"), to.join("manifest.txt"))?;
        n += 1;
    }
    Ok(n + sync_sizes(&src, to)?)
}

/// Whether `a` was changed after `b`, or `b` is missing.
fn is_newer(a: &Path, b: &Path) -> bool {
    let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    m(b).is_none_or(|b| m(a).is_some_and(|a| a > b))
}

/// Copy an icon set's pre-scaled sizes (`src/16`, `src/24`, …; not its
/// raw sources) into `to` where new or newer; how many.
fn sync_sizes(src: &Path, to: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    for e in std::fs::read_dir(src)?.flatten() {
        let name = e.file_name();
        if name.to_str().is_some_and(|n| n.parse::<u32>().is_ok()) && e.path().is_dir() {
            n += sync_pngs(&e.path(), &to.join(&name))?;
        }
    }
    Ok(n)
}

/// Copy the PNGs in `src` into `to` where new or newer, each put in place
/// whole (a clone where the file system can); how many.
fn sync_pngs(src: &Path, to: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(src)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".png") && is_newer(&e.path(), &to.join(&name)) {
            install(&e.path(), to, &name)?;
            std::fs::set_permissions(to.join(&name), std::fs::Permissions::from_mode(0o644))?;
            n += 1;
        }
    }
    Ok(n)
}

/// Copy the build tree's brand pictures (`data/brand`: the wordmark) into
/// `to` where new or newer; how many.
fn sync_brand(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/brand")).find(|d| d.is_dir()) else {
        return Ok(0);
    };
    sync_pngs(&src, to)
}

/// Copy the build tree's fonts (`data/fonts`: the faces and their
/// licence) into `to` where new or newer; how many files.
fn sync_fonts(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/fonts")).find(|d| d.is_dir()) else {
        return Ok(0);
    };
    let mut n = 0;
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(&src)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if [".ttf", ".otf", ".txt", ".md"].iter().any(|x| name.ends_with(x)) && is_newer(&e.path(), &to.join(&name)) {
            install(&e.path(), to, &name)?;
            std::fs::set_permissions(to.join(&name), std::fs::Permissions::from_mode(0o644))?;
            n += 1;
        }
    }
    Ok(n)
}

/// Copy the build tree's themes (`data/themes/<id>`: each one's sizes and
/// wordmark, not its sources or references) into `to` where new or
/// newer; how many.
fn sync_themes(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/themes")).find(|d| d.is_dir()) else {
        return Ok(0);
    };
    let mut n = 0;
    for e in std::fs::read_dir(&src)?.flatten() {
        let (theme, dest) = (e.path(), to.join(e.file_name()));
        if theme.join("icons").is_dir() {
            n += sync_sizes(&theme.join("icons"), &dest.join("icons"))?;
        }
        if theme.join("brand").is_dir() {
            n += sync_pngs(&theme.join("brand"), &dest.join("brand"))?;
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
    let mut n = 0;
    for sub in ["", "4k", "ultrawide", "thumbs"] {
        let (from_dir, to_dir) = (src.join(sub), to.join(sub));
        let Ok(entries) = std::fs::read_dir(&from_dir) else { continue };
        std::fs::create_dir_all(&to_dir)?;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_file() && (name.ends_with(".png") || name == "pack.txt") && is_newer(&e.path(), &to_dir.join(&name)) {
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

    #[test]
    fn a_program_whose_display_has_gone_is_left_over() {
        // One of ours (by name) three times: its display gone, there, none.
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-leftovers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ours = dir.join("olo-test-sleep");
        std::fs::copy("/usr/bin/sleep", &ours).unwrap();
        std::fs::write(dir.join("wayland-here"), b"").unwrap();
        let run = |display: Option<&str>| {
            let mut c = std::process::Command::new(&ours);
            c.arg("5").env_remove("WAYLAND_DISPLAY");
            if let Some(d) = display {
                c.env("WAYLAND_DISPLAY", d);
            }
            c.spawn().unwrap()
        };
        let mut gone = run(Some("wayland-gone"));
        let mut here = run(Some("wayland-here"));
        let mut none = run(None);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let found: Vec<i32> = leftovers_aged(&dir, &[], 0.0).into_iter().map(|l| l.0).collect();
        // Too young for the real thing (a session starting).
        let young: Vec<i32> = leftovers(&dir, &[]).into_iter().map(|l| l.0).collect();
        for c in [&mut gone, &mut here, &mut none] {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(found.contains(&(gone.id() as i32)), "{found:?}");
        assert!(!found.contains(&(here.id() as i32)) && !found.contains(&(none.id() as i32)), "{found:?}");
        assert!(!young.contains(&(gone.id() as i32)));
    }

    #[test]
    fn a_program_another_started_is_known_so() {
        // This test (not one of ours) starting a program: it says who.
        let mut child = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let by = started_by_another(child.id() as i32);
        let _ = child.kill();
        let _ = child.wait();
        let own = std::fs::read_to_string("/proc/self/comm").unwrap().trim().to_owned();
        assert_eq!(by.as_deref(), Some(own.as_str()));
        // Gone: nothing to say.
        assert_eq!(started_by_another(i32::MAX), None);
    }

    fn program(dir: &Path, name: &str, age: u64) {
        let p = dir.join(name);
        std::fs::write(&p, name).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn an_upgrade_asked_for_during_a_build_waits_for_it() {
        let base = std::env::temp_dir().join(format!("olo-waiting-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("build"), base.join("bin"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        program(&from, "olo-deskbar", 60);
        let state = base.join("olo-upgrade-test.json");
        let mut w = Watcher { from: from.clone(), to, state: state.clone(), pending: Vec::new(), last: None, wanted: false };
        // Cargo holds its lock while it builds.
        let lock = std::fs::File::create(from.join(".cargo-lock")).unwrap();
        // SAFETY: flock on a file we own.
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        assert!(w.install().is_empty());
        let said: Available = serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
        assert!(said.waiting);
        assert!(!w.check());
        // The build done: once.
        drop(lock);
        w.last = None;
        assert!(w.check());
        w.last = None;
        assert!(!w.check());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fonts_come_along() {
        let base = std::env::temp_dir().join(format!("olo-fonts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let build = base.join("repo/target/release");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::create_dir_all(base.join("repo/data/fonts")).unwrap();
        for name in ["NotoSans[wght].ttf", "OFL.txt", ".DS_Store"] {
            std::fs::write(base.join("repo/data/fonts").join(name), name).unwrap();
        }
        let to = base.join("prefix/share/olo/fonts");
        assert_eq!(sync_fonts(&build, &to).unwrap(), 2);
        assert!(to.join("NotoSans[wght].ttf").is_file() && to.join("OFL.txt").is_file() && !to.join(".DS_Store").exists());
        assert_eq!(std::fs::metadata(to.join("OFL.txt")).unwrap().permissions().mode() & 0o777, 0o644);
        // Nothing new: nothing copied.
        assert_eq!(sync_fonts(&build, &to).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn icons_come_along() {
        let base = std::env::temp_dir().join(format!("olo-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let build = base.join("repo/target/release");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::create_dir_all(base.join("repo/data/icons/16")).unwrap();
        std::fs::create_dir_all(base.join("repo/data/icons/src")).unwrap();
        std::fs::write(base.join("repo/data/icons/manifest.txt"), "lock|a padlock\n").unwrap();
        std::fs::write(base.join("repo/data/icons/16/lock.png"), "png").unwrap();
        std::fs::write(base.join("repo/data/icons/src/lock.png"), "raw").unwrap();
        let to = base.join("prefix/share/olo/icons");
        assert_eq!(sync_icons(&build, &to).unwrap(), 2);
        assert!(to.join("16/lock.png").is_file() && !to.join("src").exists());
        // Nothing new: nothing copied.
        assert_eq!(sync_icons(&build, &to).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn themes_come_along() {
        let base = std::env::temp_dir().join(format!("olo-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let build = base.join("repo/target/release");
        let theme = base.join("repo/data/themes/soft-be");
        std::fs::create_dir_all(&build).unwrap();
        for dir in ["icons/16", "brand", "src", "reference"] {
            std::fs::create_dir_all(theme.join(dir)).unwrap();
        }
        std::fs::write(theme.join("icons/16/lock.png"), "png").unwrap();
        std::fs::write(theme.join("brand/olo-wordmark.png"), "png").unwrap();
        std::fs::write(theme.join("src/lock.png"), "raw").unwrap();
        std::fs::write(theme.join("reference/lock.png"), "ref").unwrap();
        std::fs::write(theme.join("style.txt"), "clay").unwrap();
        let to = base.join("prefix/share/olo/themes");
        assert_eq!(sync_themes(&build, &to).unwrap(), 2);
        let ours = to.join("soft-be");
        assert!(ours.join("icons/16/lock.png").is_file() && ours.join("brand/olo-wordmark.png").is_file());
        assert!(!ours.join("src").exists() && !ours.join("reference").exists() && !ours.join("style.txt").exists());
        assert_eq!(sync_themes(&build, &to).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn finds_and_installs_newer_programs() {
        let root = std::env::temp_dir().join(format!("olo-upgrade-test-{}", std::process::id()));
        let (from, to) = (root.join("build"), root.join("bin"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        program(&from, "olo-a", 100);
        program(&from, "olo-b", 100);
        program(&from, "olo-b.d", 100);
        std::fs::write(from.join("olo-c"), "not a program").unwrap();
        program(&to, "olo-a", 50);
        program(&to, "olo-b", 200);
        assert_eq!(newer(&from, &to), Some(vec!["olo-b".to_string()]));
        install(&from.join("olo-b"), &to, "olo-b").unwrap();
        assert_eq!(std::fs::read_to_string(to.join("olo-b")).unwrap(), "olo-b");
        assert_eq!(newer(&from, &to), Some(vec![]));
        // Fresh from the linker: wait.
        program(&from, "olo-a", 0);
        assert_eq!(newer(&from, &to), None);
        assert!(replaced(&to.join("olo-b (deleted)"), &to));
        assert!(!replaced(&to.join("olo-b"), &to));
        assert!(!replaced(&from.join("olo-b (deleted)"), &to));
        assert!(!replaced(&to.join("ptyxis (deleted)"), &to));
        assert!(replaced(&to.join("tabula-b (deleted)"), &to));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn old_programs_become_links_to_new_ones() {
        let dir = std::env::temp_dir().join(format!("olo-links-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        program(&dir, "olo-deskbar", 10);
        program(&dir, "tabula-deskbar", 100);
        program(&dir, "olo-tracker", 10);
        program(&dir, "tabula-gone", 100);
        link_old_names(&dir);
        assert_eq!(std::fs::read_link(dir.join("tabula-deskbar")).unwrap(), Path::new("olo-deskbar"));
        assert_eq!(std::fs::read_to_string(dir.join("tabula-deskbar")).unwrap(), "olo-deskbar");
        // Only old names that were there; and an old program with no new
        // one stays.
        assert!(std::fs::symlink_metadata(dir.join("tabula-tracker")).is_err());
        assert!(std::fs::symlink_metadata(dir.join("tabula-gone")).unwrap().is_file());
        link_old_names(&dir);
        assert_eq!(std::fs::read_link(dir.join("tabula-deskbar")).unwrap(), Path::new("olo-deskbar"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
