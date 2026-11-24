//! Upgrades, from a build tree and from Olo's releases on GitHub.
//!
//! An installed session (tools/install-session.sh) knows the tree it came
//! from (`<prefix>/share/olo/build-dir`, or `OLO_UPGRADE_FROM`) and
//! watches it: once a build has finished and left newer programs there,
//! it lists them in a file Deskbar reads
//! (`$XDG_RUNTIME_DIR/olo-upgrade-<display>.json`), which shows a tray
//! icon. And the person's session (Setup's install too, which has no
//! tree) asks GitHub, through `olo-release` beside it, two minutes after
//! it starts and every hour (and at once when About Olo's Check Now asks),
//! whether there's a release newer than
//! what's installed, and offers that in the same file (`release`).
//!
//! Choosing to upgrade there signals us (SIGUSR2, or `olo-session
//! upgrade [build|release]`), with which one where both are on offer
//! (`choice_path`, Deskbar asks the person). A build: we install the new
//! programs beside ours and reload in place; asked while a build is
//! running, we say so (`waiting`) and upgrade as soon as it has finished.
//! A release: `olo-release install` downloads, checks, unpacks and puts
//! it in place, saying how far it has got (`installing`, Deskbar shows
//! it); then we reload in place as for a build (`release_installed`).

use std::{
    io::BufRead as _,
    os::{
        fd::AsRawFd,
        unix::{fs::PermissionsExt, process::CommandExt as _},
    },
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};

/// How often to look.
const EVERY: Duration = Duration::from_secs(3);
/// A program modified more recently than this may still be being linked.
const SETTLE: Duration = Duration::from_secs(3);

/// Programs Olo no longer has, which an upgrade takes away from the
/// install directory (a build tree may still hold their last build:
/// cargo leaves what it no longer makes). Olo Suggestions, folded into
/// Olo Doctor (2026-11-14); Smart Tiling, removed (2026-11-17).
/// tools/install-session.sh has the same list.
pub const RETIRED: &[&str] = &["olo-suggestions", "olo-smart-tile"];

/// What Deskbar reads.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Available {
    /// The build's programs that would change, by name.
    pub programs: Vec<String>,
    /// Upgrading was asked for while a build was running: it happens
    /// once the build has finished.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waiting: bool,
    /// When the build's newest program on offer was made (seconds since
    /// the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_made: Option<u64>,
    /// A release of Olo newer than what's installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ReleaseOffer>,
    /// A release being installed: at what, and how far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installing: Option<Installing>,
    /// Why the last release's install didn't go through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// A release on offer, as `olo-release latest` says it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReleaseOffer {
    /// "2026.11.17-4e8d72a5", and its title ("Olo 2026.11.17").
    pub version: String,
    pub title: String,
    /// When what it holds was made (its commit's time).
    pub made: u64,
    /// What installing it downloads.
    pub download_bytes: u64,
}

/// How far a release's install has got.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Installing {
    pub version: String,
    /// "downloading", "unpacking" or "installing".
    pub doing: String,
    pub fraction: f32,
}

/// Which upgrade the person chose, where both are on offer; or, from
/// About Olo, to ask GitHub now, or to install a release by its tag (an
/// older one too).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Build,
    Release,
    Check,
    Tag(String),
}

impl Choice {
    /// As it is written for us to read (`choice_path`): "build",
    /// "release", "check", "release:TAG".
    pub fn said(&self) -> String {
        match self {
            Choice::Build => "build".into(),
            Choice::Release => "release".into(),
            Choice::Check => "check".into(),
            Choice::Tag(t) => format!("release:{t}"),
        }
    }

    pub fn read(said: &str) -> Option<Choice> {
        match said.trim() {
            "build" => Some(Choice::Build),
            "release" => Some(Choice::Release),
            "check" => Some(Choice::Check),
            s => s.strip_prefix("release:").filter(|t| is_tag(t)).map(|t| Choice::Tag(t.to_owned())),
        }
    }
}

/// Whether `t` could be a release's tag ("v2026.11.24.2"): what is passed
/// on to `olo-release install --tag`.
pub fn is_tag(t: &str) -> bool {
    !t.is_empty() && t.len() <= 64 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// What GitHub last said of Olo's releases, as the session heard it, for
/// About Olo ("up to date", or what is newer): kept apart from Deskbar's
/// file, which offers what it holds.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Heard {
    /// When (seconds since the epoch).
    pub at: u64,
    /// The newest release: its title, tag and version.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub version: String,
    /// Newer than what's installed.
    #[serde(default)]
    pub newer: bool,
    /// Why GitHub couldn't be asked, where it couldn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Asking now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asking: bool,
}

/// Where the session says what it last heard (`Heard`).
pub fn heard_path(display: &str) -> PathBuf {
    olo_xdg::state_runtime_dir().join(format!("upgrade-releases-{display}.json"))
}

pub fn state_path(display: &str) -> PathBuf {
    olo_xdg::state_runtime_dir().join(format!("upgrade-{display}.json"))
}

/// Where Deskbar (or `olo-session upgrade build|release`) says which,
/// before it signals us: "build" or "release".
pub fn choice_path(display: &str) -> PathBuf {
    olo_xdg::state_runtime_dir().join(format!("upgrade-choice-{display}"))
}

/// The choice said for this upgrade, taken (read once).
pub fn chosen(display: &str) -> Option<Choice> {
    let path = choice_path(display);
    let said = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Choice::read(&said)
}

/// When the session first asks GitHub, and then how often.
const RELEASES_FIRST: Duration = Duration::from_secs(120);
// (Every hour: releases come after each feature now, and GitHub lets an
// address ask 60 times an hour. It was every six.)
const RELEASES_EVERY: Duration = Duration::from_secs(3600);
/// After a question that went unanswered (offline, GitHub down).
const RELEASES_AGAIN: Duration = Duration::from_secs(1800);

/// What's known of Olo's releases, and the one being installed.
struct Releases {
    /// When to ask GitHub next.
    next: Instant,
    /// `olo-release latest`'s answer, on its way.
    asking: Option<mpsc::Receiver<serde_json::Value>>,
    offer: Option<ReleaseOffer>,
    /// `olo-release install`'s words, as they come, and the last.
    installing: Option<(mpsc::Receiver<serde_json::Value>, Installing)>,
    failed: Option<String>,
    /// What an install put in place, for the session to finish
    /// (`release_installed`).
    installed: Option<Vec<String>>,
}

pub struct Watcher {
    /// The build tree, for an install made from one.
    from: Option<PathBuf>,
    to: PathBuf,
    state: PathBuf,
    pending: Vec<String>,
    build_made: Option<u64>,
    last: Option<Instant>,
    /// Asked to upgrade while a build was running.
    wanted: bool,
    /// Olo's releases, for the person's session.
    releases: Option<Releases>,
    /// Where what GitHub last said goes (`heard_path`).
    heard: PathBuf,
}

impl Watcher {
    /// Watch for programs newer than ours (in `to`): in the tree they're
    /// built in, if we know where; and, for the person's own session
    /// (`releases`), Olo's releases. None for a development session (run
    /// from the build tree itself), or with nothing to watch.
    pub fn new(to: &Path, display: &str, releases: bool) -> Option<Watcher> {
        let from = std::env::var_os("OLO_UPGRADE_FROM").map(PathBuf::from).or_else(|| {
            let file = to.parent()?.join("share/olo/build-dir");
            Some(PathBuf::from(std::fs::read_to_string(file).ok()?.trim()))
        });
        // A development session runs from the build tree itself (or one
        // the tree's own tools put in a folder of their own: no releases).
        if from.as_deref().is_some_and(|f| same_dir(f, to)) || (from.is_none() && !to.parent().is_some_and(|p| p.join("share/olo").is_dir())) {
            return None;
        }
        let releases = releases && std::env::var("OLO_UPGRADE_RELEASES").map_or(true, |v| v != "0");
        if from.is_none() && !releases {
            return None;
        }
        if let Some(from) = &from {
            crate::log(format!("watching {} for upgrades", from.display()));
        }
        if releases {
            crate::log("asking GitHub for Olo's releases (two minutes after starting, then every hour)");
        }
        let releases = releases.then(|| Releases { next: Instant::now() + RELEASES_FIRST, asking: None, offer: None, installing: None, failed: None, installed: None });
        Some(Watcher { from, to: to.to_owned(), state: state_path(display), pending: Vec::new(), build_made: None, last: None, wanted: false, releases, heard: heard_path(display) })
    }

    /// Look again (now and then), and tell Deskbar if what's on offer
    /// changed; true when an upgrade asked for during a build can go
    /// ahead now that it has finished.
    pub fn check(&mut self) -> bool {
        if self.check_releases() {
            self.publish();
        }
        if self.last.is_some_and(|t| t.elapsed() < EVERY) {
            return false;
        }
        self.last = Some(Instant::now());
        let Some(from) = self.from.clone() else { return false };
        if building(&from) {
            return false;
        }
        // (None while a program it left is too fresh to trust.)
        let Some(now) = newer(&from, &self.to) else { return false };
        if now != self.pending {
            self.build_made = made_of(&from, &now);
            self.pending = now;
            self.publish();
        }
        std::mem::take(&mut self.wanted)
    }

    /// What GitHub said, and how an install goes: whether what's on
    /// offer changed. Asks again when it's time (no more than reading
    /// two channels otherwise: nothing waits).
    fn check_releases(&mut self) -> bool {
        let to = self.to.clone();
        let Some(r) = self.releases.as_mut() else { return false };
        let mut changed = false;
        if let Some(rx) = &r.asking {
            match rx.try_recv() {
                Ok(v) => {
                    r.asking = None;
                    let at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    let text = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
                    let heard = Heard {
                        at,
                        title: text("title"),
                        tag: text("tag"),
                        version: text("version"),
                        newer: v["newer"].as_bool().unwrap_or(false),
                        error: v["error"].as_str().map(str::to_owned),
                        asking: false,
                    };
                    write_heard(&self.heard, &heard);
                    if let Some(e) = v["error"].as_str() {
                        crate::log(format!("couldn't ask GitHub for Olo's releases: {e}"));
                        r.next = Instant::now() + RELEASES_AGAIN;
                    } else {
                        let offer = v["newer"].as_bool().unwrap_or(false).then(|| ReleaseOffer {
                            version: v["version"].as_str().unwrap_or_default().to_owned(),
                            title: v["title"].as_str().unwrap_or_default().to_owned(),
                            made: v["made"].as_u64().unwrap_or(0),
                            download_bytes: v["download_bytes"].as_u64().unwrap_or(0),
                        });
                        if offer != r.offer {
                            match &offer {
                                Some(o) => crate::log(format!("a newer release of Olo is available: {} ({})", o.title, o.version)),
                                None => crate::log("no release of Olo newer than this one"),
                            }
                            r.offer = offer;
                            changed = true;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    r.asking = None;
                    r.next = Instant::now() + RELEASES_AGAIN;
                    let at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    write_heard(&self.heard, &Heard { at, error: Some("olo-release stopped before it had answered".into()), ..Heard::default() });
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((rx, now)) = r.installing.as_mut() {
            let mut finished: Option<Result<Vec<String>, String>> = None;
            loop {
                match rx.try_recv() {
                    Ok(v) => {
                        if let Some(e) = v["error"].as_str() {
                            finished = Some(Err(e.to_owned()));
                        } else if let Some(list) = v["installed"].as_array() {
                            finished = Some(Ok(list.iter().filter_map(|p| p.as_str().map(str::to_owned)).collect()));
                        } else if let Some(doing) = v["doing"].as_str() {
                            *now = Installing { version: now.version.clone(), doing: doing.to_owned(), fraction: v["fraction"].as_f64().unwrap_or(0.0) as f32 };
                            changed = true;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished.get_or_insert_with(|| Err("olo-release stopped before it had finished".into()));
                        break;
                    }
                }
            }
            if let Some(f) = finished {
                let version = now.version.clone();
                r.installing = None;
                changed = true;
                match f {
                    Ok(done) => {
                        crate::log(format!("installed release {version}: {} programs changed", done.len()));
                        r.offer = None;
                        r.failed = None;
                        r.installed = Some(done);
                        // (Asked again soon: what's installed now.)
                        r.next = Instant::now() + RELEASES_FIRST;
                    }
                    Err(e) => {
                        crate::log(format!("couldn't install release {version}: {e}"));
                        r.failed = Some(e);
                    }
                }
            }
        }
        if r.asking.is_none() && r.installing.is_none() && Instant::now() >= r.next {
            r.next = Instant::now() + RELEASES_EVERY;
            r.asking = run_release(&to, &["latest"]);
            // (Said, so About Olo shows it's asking.)
            let mut was: Heard = std::fs::read(&self.heard).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            was.asking = r.asking.is_some();
            write_heard(&self.heard, &was);
        }
        changed
    }

    /// What installing a release put in place, once it has: the programs
    /// that changed, for the session to switch to (as after a build's).
    pub fn release_installed(&mut self) -> Option<Vec<String>> {
        self.releases.as_mut()?.installed.take()
    }

    fn publish(&self) {
        let r = self.releases.as_ref();
        let release = r.and_then(|r| r.offer.clone());
        let installing = r.and_then(|r| r.installing.as_ref().map(|(_, i)| i.clone()));
        let failed = r.and_then(|r| r.failed.clone());
        if self.pending.is_empty() && !self.wanted && release.is_none() && installing.is_none() {
            let _ = std::fs::remove_file(&self.state);
            return;
        }
        if self.wanted {
            crate::log("upgrading once the build has finished");
        } else if !self.pending.is_empty() && installing.is_none() {
            crate::log(format!("upgrade available: {}", self.pending.join(", ")));
        }
        let available = Available { programs: self.pending.clone(), waiting: self.wanted, build_made: self.build_made, release, installing, failed };
        let json = serde_json::to_string(&available).unwrap_or_default();
        let tmp = self.state.with_extension("new");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.state);
        }
    }

    /// The build tree it installs from and the directory it installs to
    /// (none for an install without a tree).
    pub fn paths(&self) -> Option<(PathBuf, PathBuf)> {
        Some((self.from.clone()?, self.to.clone()))
    }

    /// Upgrade as `choice` says (none said: the build, if it has
    /// anything newer, else the release on offer). A build's programs are
    /// installed now, and returned; a release's install is started, and
    /// what it put in place comes later (`release_installed`).
    pub fn install(&mut self, choice: Option<Choice>) -> Vec<String> {
        let offered = self.releases.as_ref().and_then(|r| r.offer.clone());
        let release = match choice {
            Some(Choice::Release) => true,
            Some(Choice::Build) => false,
            // About Olo's Check Now: GitHub asked at the next look.
            Some(Choice::Check) => {
                match self.releases.as_mut() {
                    Some(r) => {
                        crate::log("asked to look for a newer release now");
                        r.next = Instant::now();
                    }
                    None => crate::log("not looking at releases here"),
                }
                return Vec::new();
            }
            Some(Choice::Tag(tag)) => {
                self.install_release_tagged(&tag);
                return Vec::new();
            }
            None => self.pending.is_empty() && !self.wanted && offered.is_some(),
        };
        if release {
            self.install_release(offered);
            return Vec::new();
        }
        let Some(from) = self.from.clone() else {
            crate::log("nothing to upgrade from: no build tree");
            return Vec::new();
        };
        self.install_build(&from)
    }

    /// `olo-release install --tag TAG`: a release chosen in About Olo,
    /// an older one too.
    fn install_release_tagged(&mut self, tag: &str) {
        let to = self.to.clone();
        let Some(r) = self.releases.as_mut() else {
            crate::log("not looking at releases here");
            return;
        };
        if r.installing.is_some() {
            crate::log("a release is being installed already");
            return;
        }
        let mut args = vec!["install", "--tag", tag];
        for name in RETIRED {
            args.extend(["--retire", name]);
        }
        match run_release(&to, &args) {
            Some(rx) => {
                crate::log(format!("installing release {tag}, as chosen"));
                r.failed = None;
                r.installing = Some((rx, Installing { version: tag.to_owned(), doing: "downloading".into(), fraction: 0.0 }));
            }
            None => r.failed = Some("olo-release isn't installed beside the session".into()),
        }
        self.publish();
    }

    /// `olo-release install` started for the release on offer.
    fn install_release(&mut self, offer: Option<ReleaseOffer>) {
        let to = self.to.clone();
        let Some(r) = self.releases.as_mut() else {
            crate::log("not looking at releases here");
            return;
        };
        if r.installing.is_some() {
            crate::log("a release is being installed already");
            return;
        }
        let Some(offer) = offer else {
            crate::log("no release on offer to install");
            return;
        };
        let mut args = vec!["install"];
        for name in RETIRED {
            args.extend(["--retire", name]);
        }
        match run_release(&to, &args) {
            Some(rx) => {
                crate::log(format!("installing release {} ({})", offer.title, offer.version));
                r.failed = None;
                r.installing = Some((rx, Installing { version: offer.version, doing: "downloading".into(), fraction: 0.0 }));
            }
            None => r.failed = Some("olo-release isn't installed beside the session".into()),
        }
        self.publish();
    }

    /// The build's newer programs installed; the programs that were.
    fn install_build(&mut self, from: &Path) -> Vec<String> {
        if building(from) {
            // Asked for now, done when it has finished (`check`), and
            // said so meanwhile.
            crate::log("a build is still running; upgrading as soon as it has finished");
            self.wanted = true;
            self.publish();
            return Vec::new();
        }
        self.wanted = false;
        let programs = newer(from, &self.to).unwrap_or_default();
        let mut done = Vec::new();
        for name in &programs {
            match install(&from.join(name), &self.to, name) {
                Ok(()) => done.push(name.clone()),
                Err(e) => crate::log(format!("could not install {name}: {e}")),
            }
        }
        crate::log(format!("installed {} of {} upgraded programs", done.len(), programs.len()));
        for name in RETIRED {
            match std::fs::remove_file(self.to.join(name)) {
                Ok(()) => crate::log(format!("removed {name}: Olo no longer has it")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => crate::log(format!("could not remove {name}, which Olo no longer has: {e}")),
            }
        }
        // New and redrawn icons come with them (a new panel's, say).
        sync_data(from, &self.to);
        self.pending = newer(from, &self.to).unwrap_or_default();
        self.build_made = made_of(from, &self.pending);
        self.publish();
        done
    }

    /// Whether the program at `exe` (as /proc shows it) is one of ours
    /// from the install directory, since replaced there.
    pub fn replaced_here(&self, exe: &Path) -> bool {
        replaced(exe, &self.to)
    }

    /// Screens' sessions (headless olo-sessions) still running an
    /// olo-session replaced here: those from before screens followed an
    /// upgrade by themselves (`Session::follow_install`), by pid.
    pub fn old_screens(&self) -> Vec<i32> {
        let uid = unsafe { libc::getuid() };
        let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
        let mut out = Vec::new();
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            if std::fs::metadata(e.path()).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).ok() != Some(uid) {
                continue;
            }
            let Ok(exe) = std::fs::read_link(e.path().join("exe")) else { continue };
            if !exe.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("olo-session")) || !self.replaced_here(&exe) {
                continue;
            }
            let Ok(args) = std::fs::read(e.path().join("cmdline")) else { continue };
            if args.split(|&b| b == 0).any(|a| a == b"--headless") {
                out.push(pid);
            }
        }
        out
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

/// The program that started `pid`, if it isn't one of ours: such a
/// process is left alone (the rule, and why, in olo-system, which Olo
/// Doctor goes by too).
pub use olo_system::procs::started_by_another;

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

/// What was heard, written whole (a reader never sees half of it).
fn write_heard(path: &Path, heard: &Heard) {
    let tmp = path.with_extension("new");
    if serde_json::to_vec(heard).is_ok_and(|b| std::fs::write(&tmp, b).is_ok()) {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// `olo-release` (beside the session in `to`) run with `args` and the
/// install's prefix, at a low priority, what it says read on a thread of
/// its own: each line, as JSON. (The session's `reap` reaps it.) None if
/// it isn't there or can't start.
fn run_release(to: &Path, args: &[&str]) -> Option<mpsc::Receiver<serde_json::Value>> {
    let program = to.join("olo-release");
    if !program.is_file() {
        return None;
    }
    let mut cmd = std::process::Command::new(&program);
    cmd.args(args).arg("--prefix").arg(to.parent()?).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped());
    // SAFETY: async-signal-safe calls only, between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            // No signals blocked (the session blocks its own), and in
            // the background: a download and an unpack, never ahead of
            // what the person is doing.
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            libc::setpriority(libc::PRIO_PROCESS, 0, 10);
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| crate::log(format!("couldn't start olo-release: {e}"))).ok()?;
    let out = child.stdout.take()?;
    // (Reaped by the session's waitpid, not through `Child`.)
    std::mem::forget(child);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("olo-release".into())
        .spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                    && tx.send(v).is_err()
                {
                    break;
                }
            }
        })
        .ok()?;
    Some(rx)
}

/// When the newest of `programs` in `from` was made (seconds since the
/// epoch).
fn made_of(from: &Path, programs: &[String]) -> Option<u64> {
    programs.iter().filter_map(|p| std::fs::metadata(from.join(p)).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())).max()
}

/// Our programs in `from` that are newer than (or missing from) `to`;
/// None while one is too fresh to trust.
fn newer(from: &Path, to: &Path) -> Option<Vec<String>> {
    let now = SystemTime::now();
    let mut out = Vec::new();
    for e in std::fs::read_dir(from).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("olo-") || name.contains('.') || RETIRED.contains(&name.as_str()) {
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

/// Copy the build tree's icons (`data/icons/svg`: the drawings, found
/// above `from`, the build's output directory) into `to` where they're
/// new or newer, and take away what an older Olo kept there (its icons
/// as pictures at each size, and their list); how many changed.
fn sync_icons(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/icons")).find(|d| d.join("svg").is_dir()) else {
        return Ok(0);
    };
    std::fs::create_dir_all(to)?;
    Ok(sync_drawings(&src, to)? + drop_pictures(to)?)
}

/// Whether `a` was changed after `b`, or `b` is missing.
fn is_newer(a: &Path, b: &Path) -> bool {
    let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    m(b).is_none_or(|b| m(a).is_some_and(|a| a > b))
}

/// Copy an icon set's drawings (`src/svg/*.svg`) and, a theme's, its
/// rules for the classic set's (`src/recolour.txt`) into `to` where new
/// or newer, and take away the drawings and rules it no longer has; how
/// many changed. (Not its sources or anything else beside them.)
fn sync_drawings(src: &Path, to: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    let (drawings, dest) = (src.join("svg"), to.join("svg"));
    if drawings.is_dir() {
        std::fs::create_dir_all(&dest)?;
        for e in std::fs::read_dir(&drawings)?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".svg") && is_newer(&e.path(), &dest.join(&name)) {
                install(&e.path(), &dest, &name)?;
                std::fs::set_permissions(dest.join(&name), std::fs::Permissions::from_mode(0o644))?;
                n += 1;
            }
        }
    }
    for e in std::fs::read_dir(&dest).into_iter().flatten().flatten() {
        if !drawings.join(e.file_name()).is_file() {
            std::fs::remove_file(e.path())?;
            n += 1;
        }
    }
    let rules = src.join("recolour.txt");
    if rules.is_file() {
        if is_newer(&rules, &to.join("recolour.txt")) {
            std::fs::create_dir_all(to)?;
            install(&rules, to, "recolour.txt")?;
            std::fs::set_permissions(to.join("recolour.txt"), std::fs::Permissions::from_mode(0o644))?;
            n += 1;
        }
    } else if to.join("recolour.txt").exists() {
        std::fs::remove_file(to.join("recolour.txt"))?;
        n += 1;
    }
    Ok(n)
}

/// Take away an icon set's pictures at each size (`16/`, `24/`, …) and
/// their list, from when icons were pictures: nothing reads them, and
/// they were most of what an install held; how many folders and files
/// went.
fn drop_pictures(set: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    for e in std::fs::read_dir(set).into_iter().flatten().flatten() {
        if e.file_name().to_str().is_some_and(|name| name.parse::<u32>().is_ok()) && e.path().is_dir() {
            std::fs::remove_dir_all(e.path())?;
            n += 1;
        }
    }
    if set.join("manifest.txt").is_file() {
        std::fs::remove_file(set.join("manifest.txt"))?;
        n += 1;
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

/// Copy the build tree's brand pictures (`data/brand`: the wordmark, and
/// in `sizes/` the logo pre-scaled for the veil and the boot splash) into
/// `to` where new or newer; how many.
fn sync_brand(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/brand")).find(|d| d.is_dir()) else {
        return Ok(0);
    };
    let sizes = if src.join("sizes").is_dir() { sync_pngs(&src.join("sizes"), &to.join("sizes"))? } else { 0 };
    Ok(sync_pngs(&src, to)? + sizes)
}

/// Copy the build tree's catalogs (`data/words`: Olo's word list and each
/// language's words, which every program reads at its start:
/// `olo_kit_base::words`) into `to` where new or newer; how many. Each
/// written beside and moved into place, so a program starting meanwhile
/// reads a whole one.
fn sync_words(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/words")).find(|d| d.is_dir()) else {
        return Ok(0);
    };
    let mut n = 0;
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(&src)?.flatten() {
        let (path, dest) = (e.path(), to.join(e.file_name()));
        if path.extension().is_some_and(|x| x == "txt") && is_newer(&path, &dest) {
            let new = dest.with_extension("txt.new");
            std::fs::copy(&path, &new)?;
            std::fs::rename(&new, &dest)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Copy the build tree's models that read the text in pictures
/// (`data/ocr`: the two models, their licence and what they are) into
/// `to` where new or newer; how many files.
fn sync_ocr(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/ocr")).find(|d| d.is_dir()) else {
        return Ok(0);
    };
    let mut n = 0;
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(&src)?.flatten() {
        let (path, dest) = (e.path(), to.join(e.file_name()));
        if path.is_file() && is_newer(&path, &dest) {
            std::fs::copy(&path, &dest)?;
            n += 1;
        }
    }
    Ok(n)
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

/// Copy the build tree's themes (`data/themes/<id>`: each one's own
/// drawings and its colours for the classic set's, `sync_drawings`) into
/// `to` where new or newer, and take
/// away the ones it no longer has (the themes there were before Classic
/// and Platinum) and a theme's own wordmark (`brand/`: the wordmark is the
/// brand's since November 2026, the same in every theme); how many
/// changed.
fn sync_themes(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/themes")).find(|d| d.is_dir()) else {
        return Ok(0);
    };
    let mut n = 0;
    for e in std::fs::read_dir(&src)?.flatten() {
        let (theme, dest) = (e.path(), to.join(e.file_name()));
        if theme.join("icons").is_dir() {
            n += sync_drawings(&theme.join("icons"), &dest.join("icons"))? + drop_pictures(&dest.join("icons"))?;
        }
        if dest.join("brand").is_dir() {
            std::fs::remove_dir_all(dest.join("brand"))?;
            n += 1;
        }
    }
    for e in std::fs::read_dir(to).into_iter().flatten().flatten() {
        if e.path().is_dir() && !src.join(e.file_name()).is_dir() {
            std::fs::remove_dir_all(e.path())?;
            n += 1;
        }
    }
    Ok(n)
}

/// What comes with the programs (icons, the brand's pictures, fonts,
/// themes, the wallpaper pack), copied from the build tree `from` to the
/// install beside `to` where new or newer. An upgrade does it as it
/// installs, and the version it installed does it again once running
/// (`Session::resume`): what an older version didn't know to copy (the
/// pack's JPEG pictures and tiles, 2026-11-15) comes then, not at the
/// upgrade after.
pub fn sync_data(from: &Path, to: &Path) {
    let Some(prefix) = to.parent() else { return };
    match sync_icons(from, &prefix.join("share/olo/icons")) {
        Ok(0) => {}
        Ok(n) => crate::log(format!("installed {n} new or changed icons")),
        Err(e) => crate::log(format!("could not install icons: {e}")),
    }
    // (Before the programs start again in them.)
    match sync_words(from, &prefix.join("share/olo/words")) {
        Ok(0) => {}
        Ok(n) => crate::log(format!("installed {n} new or changed catalogs of words")),
        Err(e) => crate::log(format!("could not install the catalogs of words: {e}")),
    }
    match sync_brand(from, &prefix.join("share/olo/brand")) {
        Ok(0) => {}
        Ok(n) => crate::log(format!("installed {n} new or changed brand pictures")),
        Err(e) => crate::log(format!("could not install the brand pictures: {e}")),
    }
    match sync_fonts(from, &prefix.join("share/olo/fonts")) {
        Ok(0) => {}
        Ok(n) => crate::log(format!("installed {n} new or changed fonts")),
        Err(e) => crate::log(format!("could not install the fonts: {e}")),
    }
    match sync_ocr(from, &prefix.join("share/olo/ocr")) {
        Ok(0) => {}
        Ok(n) => crate::log(format!("installed {n} new or changed files of the models that read text in pictures")),
        Err(e) => crate::log(format!("could not install the models that read text in pictures: {e}")),
    }
    match sync_themes(from, &prefix.join("share/olo/themes")) {
        Ok(0) => {}
        Ok(n) => crate::log(format!("installed {n} new or changed theme pictures")),
        Err(e) => crate::log(format!("could not install the themes: {e}")),
    }
    match sync_wallpapers(from, &prefix.join("share/olo/wallpapers")) {
        Ok(0) => {}
        Ok(n) => crate::log(format!("installed {n} new or changed wallpapers")),
        Err(e) => crate::log(format!("could not install wallpapers: {e}")),
    }
    match sync_pam_module(from, &prefix.join("lib/olo")) {
        Ok(false) => {}
        Ok(true) => crate::log("installed a new pam_olo_keyring.so (Olo Doctor puts it in the login screen's place)"),
        Err(e) => crate::log(format!("could not install pam_olo_keyring.so: {e}")),
    }
}

/// The PAM module that opens Olo's keyring with the login password
/// (`libpam_olo_keyring.so`, built beside the programs) copied into `to`
/// as `pam_olo_keyring.so` where new or newer: not a program, so not in
/// `bin`. Only root can put it where PAM loads it (Olo Doctor's login
/// screen fix does, from here); before, a person's install never had
/// it, so their typed password opened nothing (2026-11-15). Whether it
/// was copied.
fn sync_pam_module(from: &Path, to: &Path) -> std::io::Result<bool> {
    let built = from.join("libpam_olo_keyring.so");
    if !built.is_file() || !is_newer(&built, &to.join("pam_olo_keyring.so")) {
        return Ok(false);
    }
    std::fs::create_dir_all(to)?;
    install(&built, to, "pam_olo_keyring.so")?;
    std::fs::set_permissions(to.join("pam_olo_keyring.so"), std::fs::Permissions::from_mode(0o755))?;
    Ok(true)
}

/// Copy the build tree's wallpaper pack (`data/wallpapers`: the manifest,
/// both shapes and the previews) into `to` where new or newer, by clone
/// where the file system can (the pack is 1.6 GB, lossless), and take away the
/// pictures it no longer has; how many files changed.
fn sync_wallpapers(from: &Path, to: &Path) -> std::io::Result<usize> {
    let Some(src) = from.ancestors().skip(1).take(4).map(|d| d.join("data/wallpapers")).find(|d| d.join("pack.txt").is_file()) else {
        return Ok(0);
    };
    let mut n = 0;
    for sub in ["", "4k", "ultrawide", "thumbs", "tiles"] {
        let (from_dir, to_dir) = (src.join(sub), to.join(sub));
        let Ok(entries) = std::fs::read_dir(&from_dir) else { continue };
        std::fs::create_dir_all(&to_dir)?;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // (The pictures in any of the pack's formats, their gain maps and previews.)
            let wanted = olo_xdg::wallpapers::is_picture(&name) || name.ends_with(".png") || name == "pack.txt";
            if e.path().is_file() && wanted && is_newer(&e.path(), &to_dir.join(&name)) {
                install(&e.path(), &to_dir, &name)?;
                std::fs::set_permissions(to_dir.join(&name), std::fs::Permissions::from_mode(0o644))?;
                n += 1;
            }
        }
        for e in std::fs::read_dir(&to_dir)?.flatten() {
            let name = e.file_name();
            // (A picture converted to WebP leaves its PNG or JPEG here: gone.)
            let picture = olo_xdg::wallpapers::is_picture(&name.to_string_lossy()) || name.to_string_lossy().ends_with(".png");
            if e.path().is_file() && picture && !from_dir.join(&name).is_file() {
                std::fs::remove_file(e.path())?;
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

    /// Starts `c`, a program just copied: another test forking while the
    /// copy was open for writing keeps it busy (ETXTBSY) for a moment.
    fn spawn_just_copied(c: &mut std::process::Command) -> std::process::Child {
        for _ in 0..50 {
            match c.spawn() {
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => std::thread::sleep(std::time::Duration::from_millis(20)),
                r => return r.unwrap(),
            }
        }
        panic!("{c:?} stayed busy");
    }

    /// The person's session's watcher and `olo-release` (a stand-in,
    /// saying what the real one says): a newer release asked for and
    /// offered in Deskbar's file, then, chosen, installed, how far said
    /// as it goes, and what it put in place handed to the session.
    #[test]
    fn a_release_offered_then_installed() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-release-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(dir.join("share/olo")).unwrap();
        let stand_in = bin.join("olo-release");
        std::fs::write(
            &stand_in,
            "#!/bin/sh\ncase \"$1\" in\nlatest) echo '{\"version\":\"2\",\"title\":\"Olo 2\",\"made\":123,\"newer\":true,\"download_bytes\":10}' ;;\ninstall) echo '{\"doing\":\"downloading\",\"fraction\":0.5}'; echo '{\"installed\":[\"olo-a\"],\"version\":\"2\"}' ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755)).unwrap();
        let display = format!("test-release-watch-{}", std::process::id());
        // (A copy of the session's: no build tree here.)
        let mut w = Watcher::new(&bin, &display, true).expect("an install to watch for releases");
        assert!(w.from.is_none());
        w.releases.as_mut().unwrap().next = Instant::now();
        let until = Instant::now() + Duration::from_secs(10);
        let state = state_path(&display);
        let read = || -> serde_json::Value { std::fs::read(&state).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default() };
        while read()["release"]["version"] != "2" {
            assert!(Instant::now() < until, "never offered");
            w.check();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(read()["release"]["title"], "Olo 2");
        // Chosen: installed, and handed over.
        assert!(w.install(Some(Choice::Release)).is_empty(), "a release's install goes on in the background");
        assert_eq!(read()["installing"]["version"], "2");
        let done = loop {
            assert!(Instant::now() < until, "never installed");
            w.check();
            if let Some(done) = w.release_installed() {
                break done;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(done, ["olo-a"]);
        assert!(!state.exists(), "nothing on offer any more");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Which upgrade: as said in its file, read once.
    #[test]
    fn the_choice_is_read_once() {
        let display = format!("test-choice-{}", std::process::id());
        std::fs::create_dir_all(olo_xdg::state_runtime_dir()).unwrap();
        std::fs::write(choice_path(&display), "release\n").unwrap();
        assert_eq!(chosen(&display), Some(Choice::Release));
        for c in [Choice::Check, Choice::Tag("v2026.11.24.2".into()), Choice::Build] {
            std::fs::write(choice_path(&display), c.said()).unwrap();
            assert_eq!(chosen(&display), Some(c));
        }
        // (A tag that isn't one is no choice: it would be passed on.)
        std::fs::write(choice_path(&display), "release:v1; rm -rf ~").unwrap();
        assert_eq!(chosen(&display), None);
        assert_eq!(chosen(&display), None);
        std::fs::write(choice_path(&display), "nonsense").unwrap();
        assert_eq!(chosen(&display), None);
        assert!(!choice_path(&display).exists());
    }

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
            spawn_just_copied(&mut c)
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
    fn a_screen_on_a_replaced_session_is_old() {
        // Sessions (bash under its name, to have a process to look at):
        // a screen's and a real one's, then the installed program replaced.
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-old-screens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("olo-session");
        std::fs::copy("/usr/bin/bash", &session).unwrap();
        let run = |backend: &str| spawn_just_copied(std::process::Command::new(&session).args(["-c", "sleep 5; :", backend]));
        let (mut screen, mut real) = (run("--headless"), run("--tty-udev"));
        let w = Watcher {
            from: Some(dir.join("build")),
            to: dir.clone(),
            state: dir.join("state.json"),
            pending: Vec::new(),
            build_made: None,
            last: None,
            wanted: false,
            releases: None,
            heard: PathBuf::from("/nonexistent/heard.json"),
        };
        std::thread::sleep(std::time::Duration::from_millis(100));
        // Not replaced yet: nothing old.
        assert!(!w.old_screens().contains(&(screen.id() as i32)));
        std::fs::copy("/usr/bin/bash", dir.join(".new")).unwrap();
        std::fs::rename(dir.join(".new"), &session).unwrap();
        let old = w.old_screens();
        for c in [&mut screen, &mut real] {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(old.contains(&(screen.id() as i32)), "{old:?}");
        assert!(!old.contains(&(real.id() as i32)), "{old:?}");
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

    /// A program Olo no longer has is never installed from the build
    /// tree (its last build still there), and an upgrade takes it away.
    #[test]
    fn a_retired_program_is_taken_away() {
        let base = PathBuf::from(format!("/var/tmp/olo-retired-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("build"), base.join("bin"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        program(&from, "olo-deskbar", 60);
        program(&from, RETIRED[0], 60);
        program(&to, RETIRED[0], 600);
        assert_eq!(newer(&from, &to).unwrap(), ["olo-deskbar"]);
        let mut w = Watcher {
            from: Some(from),
            to: to.clone(),
            state: base.join("olo-upgrade-test.json"),
            pending: Vec::new(),
            build_made: None,
            last: None,
            wanted: false,
            releases: None,
            heard: PathBuf::from("/nonexistent/heard.json"),
        };
        assert_eq!(w.install(None), ["olo-deskbar"]);
        assert!(to.join("olo-deskbar").exists() && !to.join(RETIRED[0]).exists());
        let _ = std::fs::remove_dir_all(&base);
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
        let mut w = Watcher {
            from: Some(from.clone()),
            to,
            state: state.clone(),
            pending: Vec::new(),
            build_made: None,
            last: None,
            wanted: false,
            releases: None,
            heard: PathBuf::from("/nonexistent/heard.json"),
        };
        // Cargo holds its lock while it builds.
        let lock = std::fs::File::create(from.join(".cargo-lock")).unwrap();
        // SAFETY: flock on a file we own.
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        assert!(w.install(None).is_empty());
        let said: Available = serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
        assert!(said.waiting);
        assert!(!w.check());
        // The build done: once. (Another test forking just then holds the
        // lock with it until its child starts: a moment.)
        drop(lock);
        let mut done = false;
        for _ in 0..50 {
            w.last = None;
            if w.check() {
                done = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(done);
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
    fn words_come_along() {
        let base = std::env::temp_dir().join(format!("olo-words-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let build = base.join("repo/target/release");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::create_dir_all(base.join("repo/data/words")).unwrap();
        for name in ["en.txt", "de.txt", "notes.md"] {
            std::fs::write(base.join("repo/data/words").join(name), name).unwrap();
        }
        let to = base.join("prefix/share/olo/words");
        assert_eq!(sync_words(&build, &to).unwrap(), 2);
        assert!(to.join("en.txt").is_file() && to.join("de.txt").is_file() && !to.join("notes.md").exists() && !to.join("de.txt.new").exists());
        // Nothing new: nothing copied.
        assert_eq!(sync_words(&build, &to).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn icons_come_along() {
        let base = std::env::temp_dir().join(format!("olo-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let build = base.join("repo/target/release");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::create_dir_all(base.join("repo/data/icons/svg")).unwrap();
        std::fs::create_dir_all(base.join("repo/data/icons/src")).unwrap();
        std::fs::write(base.join("repo/data/icons/svg/lock.svg"), "<svg/>").unwrap();
        std::fs::write(base.join("repo/data/icons/src/lock.png"), "raw").unwrap();
        // What an older Olo installed: pictures at each size, their list,
        // and a drawing there no longer is.
        let to = base.join("prefix/share/olo/icons");
        std::fs::create_dir_all(to.join("16")).unwrap();
        std::fs::create_dir_all(to.join("svg")).unwrap();
        std::fs::write(to.join("16/lock.png"), "png").unwrap();
        std::fs::write(to.join("manifest.txt"), "lock|a padlock\n").unwrap();
        std::fs::write(to.join("svg/gone.svg"), "<svg/>").unwrap();
        assert_eq!(sync_icons(&build, &to).unwrap(), 4);
        assert_eq!(std::fs::read_to_string(to.join("svg/lock.svg")).unwrap(), "<svg/>");
        assert!(!to.join("src").exists() && !to.join("16").exists() && !to.join("manifest.txt").exists() && !to.join("svg/gone.svg").exists());
        // Nothing new: nothing copied.
        assert_eq!(sync_icons(&build, &to).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn themes_come_along() {
        let base = std::env::temp_dir().join(format!("olo-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let build = base.join("repo/target/release");
        let theme = base.join("repo/data/themes/platinum");
        std::fs::create_dir_all(&build).unwrap();
        for dir in ["icons/svg", "src"] {
            std::fs::create_dir_all(theme.join(dir)).unwrap();
        }
        std::fs::write(theme.join("icons/svg/lock.svg"), "<svg/>").unwrap();
        std::fs::write(theme.join("icons/recolour.txt"), "ink 000000\n").unwrap();
        std::fs::write(theme.join("src/lock.png"), "raw").unwrap();
        let to = base.join("prefix/share/olo/themes");
        // A theme there was once, installed before.
        std::fs::create_dir_all(to.join("soft-be/icons/16")).unwrap();
        std::fs::write(to.join("soft-be/icons/16/lock.png"), "png").unwrap();
        // The theme's own wordmark, from before the brand's was everyone's,
        // and its icons as pictures.
        std::fs::create_dir_all(to.join("platinum/brand")).unwrap();
        std::fs::write(to.join("platinum/brand/olo-wordmark.png"), "png").unwrap();
        std::fs::create_dir_all(to.join("platinum/icons/16")).unwrap();
        std::fs::write(to.join("platinum/icons/16/lock.png"), "png").unwrap();
        assert_eq!(sync_themes(&build, &to).unwrap(), 5);
        let ours = to.join("platinum");
        assert!(ours.join("icons/svg/lock.svg").is_file() && !ours.join("brand").exists() && !ours.join("icons/16").exists());
        assert_eq!(std::fs::read_to_string(ours.join("icons/recolour.txt")).unwrap(), "ink 000000\n");
        assert!(!ours.join("src").exists());
        assert!(!to.join("soft-be").exists(), "a theme that went stayed");
        assert_eq!(sync_themes(&build, &to).unwrap(), 0);
        // The theme's rules gone from the tree: gone from the install.
        std::fs::remove_file(theme.join("icons/recolour.txt")).unwrap();
        assert_eq!(sync_themes(&build, &to).unwrap(), 1);
        assert!(!ours.join("icons/recolour.txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn wallpapers_come_along_and_old_ones_go() {
        let base = std::env::temp_dir().join(format!("olo-wallpapers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let build = base.join("repo/target/release");
        let pack = base.join("repo/data/wallpapers");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::create_dir_all(pack.join("4k")).unwrap();
        std::fs::write(pack.join("pack.txt"), "classic|Classic\n").unwrap();
        std::fs::write(pack.join("4k/classic.png"), "png").unwrap();
        // The retro collections' pictures are JPEGs, their patterns tiles.
        std::fs::write(pack.join("4k/blueberry-union.jpg"), "jpg").unwrap();
        std::fs::create_dir_all(pack.join("tiles")).unwrap();
        std::fs::write(pack.join("tiles/weave.png"), "png").unwrap();
        let to = base.join("prefix/share/olo/wallpapers");
        std::fs::create_dir_all(to.join("4k")).unwrap();
        std::fs::write(to.join("4k/bebox.png"), "png").unwrap();
        assert_eq!(sync_wallpapers(&build, &to).unwrap(), 5);
        assert!(to.join("4k/classic.png").is_file() && to.join("pack.txt").is_file() && !to.join("4k/bebox.png").exists());
        assert!(to.join("4k/blueberry-union.jpg").is_file() && to.join("tiles/weave.png").is_file());
        assert_eq!(sync_wallpapers(&build, &to).unwrap(), 0);
        // Converted to WebP: the WebP comes, the JPEG goes.
        std::fs::remove_file(pack.join("4k/blueberry-union.jpg")).unwrap();
        std::fs::write(pack.join("4k/blueberry-union.webp"), "webp").unwrap();
        assert_eq!(sync_wallpapers(&build, &to).unwrap(), 2);
        assert!(to.join("4k/blueberry-union.webp").is_file() && !to.join("4k/blueberry-union.jpg").exists());
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
