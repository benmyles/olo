//! What Olo Doctor looks for: programs that shouldn't be running beside
//! Olo (input remappers that grab the mice and keyboards, another
//! desktop's daemons and portals), settings that aren't what Olo wants
//! (USB input devices allowed to fall asleep, Olo's udev rules not
//! installed, rtkit not running for the Performance supervisor, frames'
//! threads kept off the fast cores), and
//! Olo programs still running an older version. Each finding carries
//! its fix as steps (fix.rs), run as this user or as root through the
//! helper (helper.rs).
//!
//! Everything is read from the file system and systemd, through
//! `Roots`, so a test can point it at a tree of its own.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::fix::{Step, UnitAction, UnitState};

/// Olo's udev rules, as the repository has them (installed by
/// tools/install-session.sh; here so the Doctor can install or refresh
/// them without the repository).
pub const INPUT_RULE: &str = include_str!("../../../data/udev/95-olo-input.rules");
/// The Thunderbolt 5 ports' USB controller kept awake (`thunderbolt_usb2`).
pub const THUNDERBOLT_USB_RULE: &str = include_str!("../../../data/udev/75-olo-thunderbolt-usb.rules");
pub const DISPLAY_RULE: &str = include_str!("../../../data/udev/70-olo-displays.rules");
pub const IDLE_GPU_RULE: &str = include_str!("../../../data/udev/72-olo-idle-gpu.rules");
/// The login screen's entry (its Exec made absolute on install).
const SESSION_ENTRY: &str = include_str!("../../../data/session/olo.desktop");

/// Olo's Wi-Fi healer (apps/wifi-heal): its unit, and NetworkManager's
/// setting for the chips it heals.
pub const HEAL_UNIT: &str = include_str!("../../../data/systemd/olo-wifi-heal.service");
pub const BRCMFMAC_CONF: &str = include_str!("../../../data/NetworkManager/80-olo-brcmfmac.conf");
const HEAL_BIN_PATH: &str = "usr/local/libexec/olo-wifi-heal";
const HEAL_UNIT_PATH: &str = "etc/systemd/system/olo-wifi-heal.service";
const BRCMFMAC_CONF_PATH: &str = "etc/NetworkManager/conf.d/80-olo-brcmfmac.conf";
/// Wi-Fi drivers whose firmware the healer resets.
const HEALED_DRIVERS: &[&str] = &["brcmfmac"];

const INPUT_RULE_PATH: &str = "etc/udev/rules.d/95-olo-input.rules";
/// Where it was until 2026-11-12: before the smart-card readers' rule,
/// which undid it for a security key at every start (taken away as the
/// rule is installed in its new place).
const INPUT_RULE_OLD_PATH: &str = "etc/udev/rules.d/71-olo-input.rules";
const THUNDERBOLT_USB_RULE_PATH: &str = "etc/udev/rules.d/75-olo-thunderbolt-usb.rules";
const DISPLAY_RULE_PATH: &str = "etc/udev/rules.d/70-olo-displays.rules";
const IDLE_GPU_RULE_PATH: &str = "etc/udev/rules.d/72-olo-idle-gpu.rules";
/// A FIDO security key's node for the person at the seat (`security_keys`).
pub const SECURITY_KEY_RULE: &str = include_str!("../../../data/udev/74-olo-security-keys.rules");
const SECURITY_KEY_RULE_PATH: &str = "etc/udev/rules.d/74-olo-security-keys.rules";

/// The presence sensor's buffer for the session (`presence_rule`).
pub const PRESENCE_RULE: &str = include_str!("../../../data/udev/73-olo-presence.rules");
/// Olo's helper for the power button's "Immediately" (Preferences ▸
/// Keyboard): its unit and the polkit rule that lets the person at the
/// computer start it.
pub const POWER_NOW_UNIT: &str = include_str!("../../../data/systemd/olo-power-now@.service");
pub const POWER_NOW_RULES: &str = include_str!("../../../data/polkit/49-olo-power-now.rules");
const PRESENCE_RULE_PATH: &str = "etc/udev/rules.d/73-olo-presence.rules";
/// Where the login screen's entry goes (tools/install-session.sh): where
/// GDM and SDDM always look, or on an atomic system (its /usr read-only)
/// /usr/local/share's, which GDM reads only when its XDG_DATA_DIRS names
/// it.
const SESSION_ENTRY_PATH: &str = "usr/share/wayland-sessions/olo.desktop";
const SESSION_ENTRY_LOCAL_PATH: &str = "usr/local/share/wayland-sessions/olo.desktop";
/// The most any thread may ask the scheduler to take it as busy
/// (uclamp.min, of 1024), and Olo's setting of it kept at boot.
const UTIL_CLAMP_PATH: &str = "proc/sys/kernel/sched_util_clamp_min";
const UTIL_CLAMP_CONF_PATH: &str = "etc/sysctl.d/80-olo-uclamp.conf";
const UTIL_CLAMP_CONF: &str = "# Olo: frames' threads may ask for a fast core (olo_protocol::performance::CORE_UTIL_MIN).\nkernel.sched_util_clamp_min = 1024\n";

/// Input remappers: they grab the real keyboards (and any mouse whose
/// motion node has keys) and type through a virtual one, so Olo's own
/// key mapping and its per-mouse settings don't reach them. Program
/// name, system unit, what it is.
const REMAPPERS: &[(&str, &str, &str)] = &[
    ("keyd", "keyd.service", "a key remapping daemon"),
    ("xremap", "xremap.service", "a key remapper"),
    ("input-remapper-service", "input-remapper.service", "Input Remapper's daemon"),
    ("input-remapper-reader-service", "input-remapper.service", "Input Remapper's reader"),
    ("evremap", "evremap.service", "a key remapper"),
    ("kmonad", "kmonad.service", "a keyboard remapper"),
    ("udevmon", "udevmon.service", "Interception Tools' daemon"),
];

/// Another desktop's parts that fight Olo for the session when they run
/// in it: GNOME's settings daemons (keys, power, the pointer), its shell,
/// KDE's compositor, and their portals (Olo has its own).
const FOREIGN: &[(&str, &str)] = &[
    ("gnome-shell", "GNOME Shell"),
    ("mutter", "Mutter"),
    ("kwin_wayland", "KWin"),
    ("gsd-keyboard", "GNOME's keyboard settings daemon"),
    ("gsd-media-keys", "GNOME's media keys daemon"),
    ("gsd-power", "GNOME's power daemon"),
    ("gsd-xsettings", "GNOME's XSettings daemon"),
    ("gsd-color", "GNOME's colour daemon"),
];

/// GNOME's services Olo has its own in place of: one running in an Olo
/// session was started by something before Olo's took over (the start of
/// the session, a program started before an upgrade) and isn't needed;
/// GNOME Keyring too (Olo's keyring always answers, and nothing of
/// GNOME's is needed). (After a `|`: what follows its processes, as
/// `by_program` says them.)
const GNOME_SERVICES: &[(&str, &str)] = &[
    ("dconf-service", "GNOME's dconf service|, where olo-dconf answers"),
    ("xdg-desktop-portal-gnome", "GNOME's portal backend|, where olo-portal is the backend"),
    ("xdg-desktop-portal-gtk", "GTK's portal backend|, where olo-portal is the backend"),
    ("gnome-keyring-daemon", "GNOME Keyring|, where Olo's keyring answers"),
    ("gvfsd", "gvfs|, where programs Olo starts use GIO's own files"),
    ("gvfsd-fuse", "gvfs's FUSE folder"),
    ("gvfsd-metadata", "gvfs's metadata"),
    ("gvfs-udisks2-volume-monitor", "gvfs's disk monitor"),
    ("gvfs-mtp-volume-monitor", "gvfs's phone monitor"),
    ("gvfs-gphoto2-volume-monitor", "gvfs's camera monitor"),
    ("gvfs-afc-volume-monitor", "gvfs's iPhone monitor"),
    ("gvfs-goa-volume-monitor", "gvfs's online accounts monitor"),
    ("goa-daemon", "GNOME Online Accounts"),
];

/// The programs that are another desktop's session itself: one running
/// in another login (another session's scope) means that desktop is
/// logged in there, and the daemons are its own.
const DESKTOPS: &[&str] = &["gnome-shell", "mutter", "kwin_wayland"];

/// Olo's programs the Doctor never stops for being old: the session
/// (stopping it logs out), the compositor (every display resets) and
/// the proxies (the session's stale check leaves them too).
const NEVER_STOPPED: &[&str] = &["olo-session", "olo-app_server", "olo-app-server", "olo-wlproxy", "olo-doctor"];

/// What kind a finding is, as the window tags it and lists it (most
/// pressing first): Ben, 2026-11-13, "each item has a type showing
/// whether it's e.g. a suggestion or something critical".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    /// Something you rely on doesn't work or is at risk now (your
    /// backups, the computer's sound).
    Critical,
    /// Gets in Olo's way now.
    Problem,
    /// Better changed ("Recommended").
    Advice,
    /// Olo suggests it, none of it needed: Olo works as well without
    /// (suggest.rs; what Olo Suggestions had).
    Suggestion,
}

impl Severity {
    /// Its tag, as the window shows it.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Critical => "Critical",
            Severity::Problem => "Problem",
            Severity::Advice => "Recommended",
            Severity::Suggestion => "Suggestion",
        }
    }

    /// Something wrong (told in a notification, counted as a problem by
    /// the tray), not something better.
    pub fn is_problem(self) -> bool {
        matches!(self, Severity::Critical | Severity::Problem)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fix {
    /// The button: "Stop and Uninstall keyd".
    pub label: String,
    /// Needs root (the helper, through pkexec).
    pub root: bool,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable across scans ("remapper:keyd", "autosuspend:3-1.3").
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    /// None: something to do by hand (the detail says what).
    pub fix: Option<Fix>,
}

/// Something checked and found as it should be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fine {
    pub title: String,
    pub detail: String,
}

/// Something applied that can be undone (a suggestion taken): the id
/// its button goes by, the line it's on (a `Fine`'s title), and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Undo {
    pub id: String,
    pub title: String,
    pub fix: Fix,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub fine: Vec<Fine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undo: Vec<Undo>,
    /// Found, but what the person said to ignore (ignored.rs): out of the
    /// counts, what the window checks, the tray and the notifications;
    /// still fixable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<Finding>,
}

impl Report {
    /// How many findings are problems, critical ones too.
    pub fn problems(&self) -> usize {
        self.findings.iter().filter(|f| f.severity.is_problem()).count()
    }

    /// How many findings are of kind `s`.
    pub fn count(&self, s: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == s).count()
    }

    /// What's fixed (`--fix all`, and checked in the window to begin
    /// with): each finding with a fix but the suggestions (checked by
    /// the person, or `--fix suggestions`).
    pub fn fixable(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.fix.is_some() && f.severity != Severity::Suggestion)
    }

    /// The suggestions that can be applied with a button.
    pub fn applicable(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.fix.is_some() && f.severity == Severity::Suggestion)
    }

    /// `other`'s findings, what's fine and what can be undone added to
    /// these (the checks' and the suggestions', one report).
    pub fn merge(&mut self, other: Report) {
        self.findings.extend(other.findings);
        self.fine.extend(other.fine);
        self.undo.extend(other.undo);
        self.ignored.extend(other.ignored);
    }

    /// The findings most pressing first (critical, problems, what's
    /// recommended, suggestions), each kind in the order it was found.
    pub fn sort(&mut self) {
        self.findings.sort_by_key(|f| f.severity);
        self.ignored.sort_by_key(|f| f.severity);
    }

    /// A finding, ignored or not.
    pub fn find(&self, id: &str) -> Option<&Finding> {
        self.findings.iter().chain(&self.ignored).find(|f| f.id == id)
    }

    /// The findings whose ids are in `ids` moved to `ignored`.
    pub fn set_aside(&mut self, ids: &std::collections::BTreeSet<String>) {
        let (ignored, kept) = std::mem::take(&mut self.findings).into_iter().partition(|f| ids.contains(&f.id));
        self.findings = kept;
        self.ignored = ignored;
    }

    /// Finding `id` ignored (`on`) or back among the rest, as the window
    /// shows it at once (ignored.rs keeps it).
    pub fn move_ignored(&mut self, id: &str, on: bool) {
        let (from, to) = if on { (&mut self.findings, &mut self.ignored) } else { (&mut self.ignored, &mut self.findings) };
        if let Some(i) = from.iter().position(|f| f.id == id) {
            to.push(from.remove(i));
        }
    }

    /// What the button `id` does: a finding's fix, or an undo.
    pub fn action(&self, id: &str) -> Option<&Fix> {
        self.find(id).and_then(|f| f.fix.as_ref()).or_else(|| self.undo.iter().find(|u| u.id == id).map(|u| &u.fix))
    }
}

/// Where the system is looked at.
pub struct Roots {
    /// The file system's root ("/"): /sys, /proc, /etc, /usr under it.
    pub fs: PathBuf,
    /// Where Olo's programs are installed (~/.local/bin).
    pub bin: PathBuf,
    /// This user's id (processes of others are left alone).
    pub uid: u32,
    /// This login session ("session-2.scope"): another's processes are
    /// left alone.
    pub scope: Option<String>,
    /// What systemd says about a system unit.
    pub units: Box<dyn Fn(&str) -> Result<UnitState, String> + Send + Sync>,
    /// AirPods are paired here (the AirPods service's state).
    pub airpods: bool,
    /// A phone is paired here (BlueZ's: one whose class is a phone's).
    pub phone: bool,
    /// An iPhone among them (Apple's vendor id).
    pub iphone: bool,
    /// A paired phone is connected now (BlueZ's word).
    pub phone_connected: bool,
    /// The class of device the Bluetooth adapter says now (None: no
    /// adapter, or BlueZ not answering).
    pub bluetooth_class: Option<u32>,
    /// Bluetooth controllers that don't answer (BlueZ's adapter powered
    /// and its power off, or the other way round), by name ("hci0").
    pub bluetooth_stuck: Vec<String>,
    /// The system's packages (PackageKit's word): which of these names
    /// there are, newest, and whether installed. None: no PackageKit.
    pub packages: Box<dyn Fn(&[&str]) -> Option<Vec<(String, bool)>> + Send + Sync>,
    /// The names of the packages with an update waiting (None: no
    /// PackageKit).
    pub updates: Box<dyn Fn() -> Option<Vec<String>> + Send + Sync>,
    /// The services software is installed through that aren't here
    /// (`olo_system::packages::missing_services`: PackageKit, dnf's), by
    /// package.
    pub package_services: Box<dyn Fn() -> Vec<&'static str> + Send + Sync>,
    /// Where the session's programs keep temporary files (`temp_files`).
    pub temp: TempFiles,
    /// The port Token Pool's gateway listens on (`token_pool`).
    pub pool_port: u16,
    /// Which BBR this kernel's own "bbr" is, where it has one loaded
    /// (`olo_net::tcp_info::kernels_bbr_version`: 1 as mainline's, 3 in a
    /// kernel built with Google's patches).
    pub kernels_bbr: Option<u8>,
}

/// Token Pool's gateway's port, unless `OLO_POOL_PORT` says another
/// (olo_token_pool's `gateway::PORT`).
const POOL_PORT: u16 = 17891;

/// Where temporary files go: /tmp's file system, and the session's
/// `TMPDIR` (systemd's user manager's, else ours) and whether it's in
/// memory too; the user's config folder (for the fix's file) and data
/// folder (for D-Bus activation files: `file_manager`), and the
/// session's runtime folder (the activation files an Olo session writes).
#[derive(Debug, Clone, Default)]
pub struct TempFiles {
    pub tmp_in_memory: bool,
    pub tmpdir: Option<String>,
    /// The environment of what's started on demand in this session
    /// (systemd's user manager's; None: it didn't say): `language`.
    pub session_env: Option<Vec<(String, String)>>,
    pub tmpdir_in_memory: bool,
    pub config: PathBuf,
    pub data: PathBuf,
    pub runtime: PathBuf,
    /// Olo's cache folder (~/.cache/olo): where the boot splash is drawn
    /// before it's installed (`start_up_screen`).
    pub cache: PathBuf,
}

/// The session's `TMPDIR`: what systemd's user manager has (`manager`,
/// what everything started from now on gets); if it couldn't be asked
/// (busy past its 10 s), what Olo's file sets for the session (`file`),
/// and only then this program's own (`own`). Doctor's watcher starts at
/// login, before any fix, so its own has none: whenever the manager was
/// slow to answer, the tray said "Temporary files take memory" while the
/// window, started after the fix, said all was well (Ben, 2026-11-07:
/// "seems like some of the checks are flaky").
fn session_tmpdir(manager: Option<Vec<(String, String)>>, file: Option<String>, own: Option<String>) -> Option<String> {
    let found = match manager {
        Some(env) => env.into_iter().find(|(k, _)| k == "TMPDIR").map(|(_, v)| v),
        None => file.or(own),
    };
    found.filter(|d| !d.is_empty())
}

impl TempFiles {
    fn real() -> TempFiles {
        let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".config"));
        let manager = olo_system::env::manager_environment();
        if let Err(e) = &manager {
            tracing::info!("doctor: systemd's user manager didn't say its environment ({e}): going by Olo's file");
        }
        let session_env = manager.as_ref().ok().cloned();
        let tmpdir = session_tmpdir(manager.ok(), olo_system::env::tmpdir_in_file(&config), std::env::var("TMPDIR").ok());
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".local/share"));
        let runtime = olo_xdg::runtime_dir();
        TempFiles {
            cache: olo_xdg::cache_dir(),
            tmp_in_memory: olo_system::env::in_memory(Path::new("/tmp")),
            tmpdir_in_memory: tmpdir.as_deref().is_some_and(|d| olo_system::env::in_memory(Path::new(d))),
            tmpdir,
            session_env,
            config,
            data,
            runtime,
        }
    }
}

/// PackageKit's answers, for [`Roots::real`].
fn real_packages(names: &[&str]) -> Option<Vec<(String, bool)>> {
    olo_system::packages::resolve(names).ok().map(|ps| ps.into_iter().map(|p| (p.name().to_owned(), p.installed)).collect())
}

fn real_updates() -> Option<Vec<String>> {
    olo_system::packages::updates().ok().map(|ps| ps.into_iter().map(|p| p.name().to_owned()).collect())
}

impl Roots {
    pub fn real() -> Roots {
        // Under sudo, the person who ran it (their programs, their
        // session's files), not root.
        let sudo_uid = std::env::var("SUDO_UID").ok().and_then(|u| u.parse::<u32>().ok()).filter(|_| unsafe { libc::geteuid() } == 0);
        if let Some(u) = sudo_uid {
            // SAFETY: setting our own environment before any thread reads it.
            unsafe { std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{u}")) };
            if let Some(home) = std::env::var_os("SUDO_HOME").or_else(|| home_of(u)) {
                unsafe { std::env::set_var("HOME", home) };
            }
        }
        // Where tools/install-session.sh puts them (and the upgrade).
        let bin = PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/bin");
        // SAFETY: getuid has no preconditions.
        let uid = sudo_uid.unwrap_or_else(|| unsafe { libc::getuid() });
        let scope = scope_of(&std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default());
        // AirPods paired, as BlueZ says (the service needn't be running).
        let airpods = olo_system::bluetooth::status().map(|s| s.devices.iter().any(|d| d.paired && d.modalias.as_deref().is_some_and(olo_system::airpods::is_airpods))).unwrap_or(false);
        let phones: Vec<bool> = olo_system::bluetooth::status()
            .map(|s| s.devices.iter().filter(|d| d.paired && d.icon == "phone").map(|d| d.modalias.as_deref().is_some_and(|m| m.to_ascii_lowercase().starts_with("bluetooth:v004c"))).collect())
            .unwrap_or_default();
        let (phone, iphone) = (!phones.is_empty(), phones.contains(&true));
        let phone_connected = olo_system::bluetooth::status().map(|s| s.devices.iter().any(|d| d.paired && d.connected && d.icon == "phone")).unwrap_or(false);
        let bluetooth_class = olo_system::bluetooth::status().ok().and_then(|s| s.adapter).map(|a| a.class);
        let bluetooth_stuck =
            olo_system::bluetooth::status().ok().and_then(|s| s.adapter).filter(|a| a.not_answering()).map(|a| a.path.rsplit('/').next().unwrap_or_default().to_owned()).into_iter().collect();
        Roots {
            fs: PathBuf::from("/"),
            bin,
            uid,
            scope,
            units: Box::new(crate::fix::unit_state),
            airpods,
            phone,
            iphone,
            phone_connected,
            bluetooth_class,
            bluetooth_stuck,
            packages: Box::new(real_packages),
            updates: Box::new(real_updates),
            package_services: Box::new(olo_system::packages::missing_services),
            temp: TempFiles::real(),
            pool_port: std::env::var("OLO_POOL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(POOL_PORT),
            kernels_bbr: olo_net::tcp_info::kernels_bbr_version(),
        }
    }
}

/// A process of this machine, as much of it as the checks need.
struct Proc {
    pid: i32,
    uid: u32,
    /// Its program's name (the command line's first word, by its file
    /// name; else the comm).
    name: String,
    /// Its executable, as the kernel names it ("… (deleted)" once
    /// replaced).
    exe: Option<String>,
    /// The login session it belongs to ("session-2.scope"); None for
    /// the user's services, which every session shares.
    scope: Option<String>,
}

/// A user's home, from /etc/passwd.
fn home_of(uid: u32) -> Option<std::ffi::OsString> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 5 && f[2] == uid.to_string()).then(|| f[5].into())
    })
}

/// Whether a process is an agent screen's (`OLO_SCREEN` in its
/// environment): the screen's own session, on a bus of its own, which
/// starts its services again as they exit and brings its programs up to
/// date as it upgrades, so it isn't the person's session's to quit things
/// in. (Three screens from before their portals were Olo's alone ran
/// GTK's portal backend, started again at once: Ben, 2026-11-07, "i press
/// fix all or quit them and it just comes back".)
fn on_a_screen(fs: &Path, pid: i32) -> bool {
    std::fs::read(fs.join("proc").join(pid.to_string()).join("environ")).is_ok_and(|e| e.split(|b| *b == 0).any(|v| v.starts_with(b"OLO_SCREEN=")))
}

/// A process's login session, from its cgroup.
fn scope_of(cgroup: &str) -> Option<String> {
    cgroup.lines().find_map(|l| l.rsplit('/').find(|part| part.starts_with("session-") && part.ends_with(".scope")).map(str::to_owned))
}

fn processes(fs: &Path) -> Vec<Proc> {
    use std::os::unix::fs::MetadataExt as _;
    let mut v = Vec::new();
    for e in std::fs::read_dir(fs.join("proc")).into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        let dir = e.path();
        let Ok(meta) = dir.metadata() else { continue };
        let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
        let first = cmdline.split(|b| *b == 0).next().unwrap_or(&[]);
        let exe = std::fs::read_link(dir.join("exe")).ok().map(|p| p.to_string_lossy().into_owned());
        let first = String::from_utf8_lossy(first);
        // (Chrome writes its whole command line into its first word, with
        // spaces: "…/chrome --type=gpu-process …". Its program is then its
        // executable's, or the first word's up to a space.)
        let first = if first.contains(' ') {
            exe.as_deref().map(|e| e.trim_end_matches(" (deleted)").to_owned()).unwrap_or_else(|| first.split(' ').next().unwrap_or("").to_owned())
        } else {
            first.into_owned()
        };
        let mut name = first.rsplit('/').next().unwrap_or("").to_owned();
        if name.is_empty() {
            name = std::fs::read_to_string(dir.join("comm")).unwrap_or_default().trim().to_owned();
        }
        let scope = scope_of(&std::fs::read_to_string(dir.join("cgroup")).unwrap_or_default());
        v.push(Proc { pid, uid: meta.uid(), name, exe, scope });
    }
    v
}

/// The input devices' names (/proc/bus/input/devices).
fn input_device_names(fs: &Path) -> Vec<String> {
    std::fs::read_to_string(fs.join("proc/bus/input/devices")).unwrap_or_default().lines().filter_map(|l| l.strip_prefix("N: Name=\"")).map(|n| n.trim_end_matches('"').to_owned()).collect()
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

/// A USB device with an input interface (a mouse, keyboard or receiver).
struct UsbInput {
    /// The bus path ("3-1.3").
    port: String,
    dir: PathBuf,
    manufacturer: String,
    product: String,
    /// What the kernel made of it ("Logitech G309").
    inputs: Vec<String>,
    /// power/control: "on" or "auto".
    control: String,
}

fn usb_inputs(fs: &Path) -> Vec<UsbInput> {
    let devices = fs.join("sys/bus/usb/devices");
    let mut by_port: BTreeMap<String, UsbInput> = BTreeMap::new();
    for e in std::fs::read_dir(&devices).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // Interfaces are "PORT:CONFIG.INTERFACE".
        let Some((port, _)) = name.split_once(':') else { continue };
        let iface = e.path();
        if read(&iface.join("bInterfaceClass")).as_deref() != Some("03") {
            continue;
        }
        // An input node under it (a HID device without one, such as a
        // display's brightness control, isn't an input device).
        let mut inputs = Vec::new();
        input_names_under(&iface, 3, &mut inputs);
        if inputs.is_empty() {
            continue;
        }
        let dir = devices.join(port);
        let entry = by_port.entry(port.to_owned()).or_insert_with(|| UsbInput {
            port: port.to_owned(),
            manufacturer: read(&dir.join("manufacturer")).unwrap_or_default(),
            product: read(&dir.join("product")).unwrap_or_default(),
            inputs: Vec::new(),
            control: read(&dir.join("power/control")).unwrap_or_default(),
            dir,
        });
        for n in inputs {
            if !entry.inputs.contains(&n) {
                entry.inputs.push(n);
            }
        }
    }
    by_port.into_values().collect()
}

/// The names of the input nodes under a USB interface: its HID
/// device's, or (a receiver's) those of the devices paired with it,
/// a level further down.
fn input_names_under(dir: &Path, depth: u32, names: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == "input" {
            for input in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                if let Some(n) = read(&input.path().join("name"))
                    && !names.contains(&n)
                {
                    names.push(n);
                }
            }
        } else if depth > 0 && name.starts_with("000") && e.file_type().is_ok_and(|t| t.is_dir()) {
            // A HID device ("0003:046D:C547.0016").
            input_names_under(&e.path(), depth - 1, names);
        }
    }
}

impl UsbInput {
    /// "Logitech G309 (Logitech USB Receiver)".
    fn label(&self) -> String {
        let device = format!("{} {}", self.manufacturer, self.product).trim().to_owned();
        match self.inputs.first() {
            Some(first) if !device.is_empty() && !first.eq_ignore_ascii_case(&device) => format!("{first} ({device})"),
            Some(first) => first.clone(),
            None => device,
        }
    }
}

/// The vendor:product pairs a hidraw rule names.
fn rule_ids(rule: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for line in rule.lines().filter(|l| !l.starts_with('#')) {
        let take = |key: &str| line.split(key).nth(1).and_then(|r| r.split('"').nth(1)).map(str::to_owned);
        if let (Some(vendor), Some(product)) = (take("{idVendor}=="), take("{idProduct}==")) {
            v.push((vendor, product));
        }
    }
    v
}

/// The USB devices present with one of `ids`.
fn usb_with_ids(fs: &Path, ids: &[(String, String)]) -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(fs.join("sys/bus/usb/devices")).into_iter().flatten().flatten() {
        let dir = e.path();
        let (Some(vendor), Some(product)) = (read(&dir.join("idVendor")), read(&dir.join("idProduct"))) else { continue };
        if ids.iter().any(|(v, p)| *v == vendor && *p == product) {
            v.push((read(&dir.join("product")).unwrap_or_else(|| e.file_name().to_string_lossy().into_owned()), dir));
        }
    }
    v
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("{n} {one}") } else { format!("{n} {many}") }
}

/// Look this computer over (the checks and the suggestions, one report,
/// most pressing first), set aside what's ignored, and say what was
/// found for Deskbar's tray (`olo_protocol::doctor::DoctorFound`).
/// (`OLO_DOCTOR_DEMO`: a made-up report, one of everything, to look at
/// headless; the tray isn't told.)
pub fn look() -> Report {
    if crate::demo::on() {
        return crate::demo::report();
    }
    let mut report = scan(&Roots::real());
    report.merge(crate::suggest::scan());
    report.set_aside(&crate::ignored::all());
    // What was applied and can be undone (applied.rs).
    crate::applied::into_report(&mut report);
    report.sort();
    tell_tray(&report);
    report
}

/// What Deskbar's tray shows (its bag) from `report`: the problems, and
/// what's recommended that a button fixes; not the suggestions (told
/// once in a notification, then left in the window), nor what's ignored.
pub fn tell_tray(report: &Report) {
    if crate::demo::on() {
        return;
    }
    let issues: Vec<&Finding> = report.findings.iter().filter(|f| f.severity.is_problem() || (f.fix.is_some() && f.severity == Severity::Advice)).collect();
    let found = olo_protocol::doctor::DoctorFound {
        issues: issues.iter().map(|f| f.title.clone()).collect(),
        problems: issues.iter().filter(|f| f.severity.is_problem()).count(),
        at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    };
    if let Err(e) = found.save() {
        tracing::warn!("doctor: what was found not saved: {e}");
    }
}

pub fn scan(roots: &Roots) -> Report {
    let mut r = Report::default();
    let procs = processes(&roots.fs);
    let input_names = input_device_names(&roots.fs);
    remappers(roots, &procs, &input_names, &mut r);
    foreign(roots, &procs, &mut r);
    gnome_services(roots, &procs, &mut r);
    let rule_current = input_rule(roots, &mut r);
    autosuspend(roots, rule_current, &mut r);
    display_rule(roots, &mut r);
    wifi(roots, &mut r);
    bluetooth(roots, &mut r);
    rtkit(roots, &mut r);
    let realtime_leave = realtime(roots, &mut r);
    voice_realtime(roots, &procs, realtime_leave, &mut r);
    airpods_listening(roots, &procs, &mut r);
    phone(roots, &mut r);
    phone_link(roots, &mut r);
    phone_class(roots, &procs, &mut r);
    util_clamp(roots, &mut r);
    kept_core(roots, &mut r);
    session_weight(roots, &mut r);
    session_groups(roots, &mut r);
    let gpus = pci_devices(&roots.fs);
    idle_gpu(roots, &gpus, &mut r);
    gpu_stuck(roots, &gpus, &mut r);
    nvidia_console(roots, &gpus, &mut r);
    gpu_driver(roots, &gpus, &mut r);
    graphics_swap(roots, &gpus, &mut r);
    memory_kept(roots, &procs, &mut r);
    open_files(roots, &procs, &mut r);
    inotify(roots, &procs, &mut r);
    language(roots, &procs, &mut r);
    swappiness(roots, &mut r);
    swap_size(roots, &gpus, &mut r);
    tcp_bbr3(roots, &mut r);
    ec_storm(roots, &mut r);
    ec_guard(roots, &mut r);
    kernel_settings(roots, &mut r);
    intel_video(roots, &gpus, &mut r);
    builtin_sound(roots, &gpus, &mut r);
    thunderbolt(roots, &mut r);
    thunderbolt_usb2(roots, &gpus, &mut r);
    login_keyring(roots, &mut r);
    secrets_store(roots, &procs, &mut r);
    presence_rule(roots, &mut r);
    power_now(roots, &mut r);
    power_modes(roots, &mut r);
    security_keys(roots, &mut r);
    panel_hdr(roots, &mut r);
    stale(roots, &procs, &mut r);
    token_pool(roots, &procs, &mut r);
    kache(roots, &procs, &mut r);
    package_services(roots, &mut r);
    needed_packages(roots, &mut r);
    login_entry(roots, &mut r);
    login_screen_handed_over(roots, &mut r);
    own_login_screen(roots, &mut r);
    fan_profile(roots, &mut r);
    temp_files(roots, &mut r);
    file_manager(roots, &mut r);
    diskbackup(roots, &mut r);
    start_up_screen(roots, &mut r);
    start_up_chime(roots, &mut r);
    r
}

/// The finding that offers Olo's start-up screen, and the one that
/// brings an installed one up to date: both draw it first (`prepare`).
pub const START_UP_SCREEN: &str = "start-up-screen";
pub const START_UP_SCREEN_OLD: &str = "start-up-screen:old";

/// Where the start-up screen is drawn, as the person, before it's
/// installed (`cache`: Olo's cache folder).
pub fn splash_drawn(cache: &Path) -> PathBuf {
    cache.join("plymouth-theme")
}

/// The root steps that install Olo's boot splash drawn into `theme` on
/// the system under `fs` (Olo Doctor's and Olo Setup's): its plugin if
/// it's missing, the theme, Plymouth's settings (what was there kept the
/// first time, for the way back) and the start-up images made again.
pub fn splash_steps(fs: &Path, theme: &Path, packages: &dyn Fn(&[&str]) -> Option<Vec<(String, bool)>>) -> Vec<Step> {
    use olo_system::splash;
    let mut steps = Vec::new();
    if !fs.join(splash::PLUGIN).exists() && packages(&[splash::PLUGIN_PACKAGE]).is_some_and(|p| p.iter().any(|(n, _)| n == splash::PLUGIN_PACKAGE)) {
        steps.push(Step::InstallPackages { names: vec![splash::PLUGIN_PACKAGE.into()] });
    }
    steps.push(Step::CopyTree { from: theme.to_owned(), to: PathBuf::from(splash::THEME_DIR), mode: 0o644 });
    let conf = fs.join(splash::CONF.trim_start_matches('/'));
    if conf.exists() && splash::theme(fs).as_deref() != Some("olo") && !fs.join(splash::CONF_BEFORE.trim_start_matches('/')).exists() {
        steps.push(Step::CopyFile { from: PathBuf::from(splash::CONF), to: PathBuf::from(splash::CONF_BEFORE), mode: 0o644 });
    }
    steps.push(Step::WriteFile { path: PathBuf::from(splash::CONF), content: splash::OLO_CONF.into(), mode: 0o644 });
    steps.push(Step::RebuildInitramfs);
    steps
}

/// Olo's start-up screen as the computer starts (Ben, 2026-11-13: "olo
/// doctor should offer to install the boot stuff"): its boot splash, a
/// Plymouth theme of the start-up screen's picture
/// (`olo_system::splash`; app_server draws it). Not installed: a
/// suggestion (Use Olo's Start-Up Screen); drawn by an older version or
/// at another scale than the computer's own screen is at now: Update It;
/// installed but changed since the start-up image was made: Make It
/// Again (the image alone); else fine, with the way back. Nothing on an
/// atomic system (/usr is its image's), without Plymouth, or without
/// Olo's compositor installed (it draws it).
fn start_up_screen(roots: &Roots, r: &mut Report) {
    use olo_system::splash::{self, Installed};
    if !roots.bin.join("olo-app-server").is_file() || roots.fs.join("run/ostree-booted").exists() || !roots.fs.join("usr/sbin/plymouthd").exists() {
        return;
    }
    let scale = splash::boot_scale(&splash::display_settings(&roots.temp.config), &roots.fs.join("sys"));
    let release = read(&roots.fs.join("proc/sys/kernel/osrelease")).map(|s| s.trim().to_owned()).unwrap_or_default();
    let theme = splash_drawn(&roots.temp.cache);
    let install = || splash_steps(&roots.fs, &theme, &*roots.packages);
    let pct = |s: f64| format!("{}%", (s * 100.0).round());
    match splash::installed(&roots.fs, scale, &release) {
        Installed::Not(_) => r.findings.push(Finding {
            id: START_UP_SCREEN.into(),
            severity: Severity::Suggestion,
            title: "Show Olo's start-up screen as the computer starts".into(),
            detail: format!(
                "From the moment the computer starts until you log in, it shows the system's own logos. Olo's start-up screen (its wordmark and the row of icons lit as it starts) can show instead, at your screen's scale ({}), and asks a disk's password in Olo's field. It's made into the start-up image, which takes a minute or two; Olo Doctor can go back to the one before.",
                pct(scale)
            ),
            fix: Some(Fix { label: "Use Olo's Start-Up Screen".into(), root: true, steps: install() }),
        }),
        Installed::Old(made) => r.findings.push(Finding {
            id: START_UP_SCREEN_OLD.into(),
            severity: Severity::Advice,
            title: "Olo's start-up screen is out of date".into(),
            detail: match made {
                Some(m) if m.format == splash::FORMAT => format!("It was drawn at {}, and the computer's own screen is at {} in Olo now: drawn again, it's sharp at that scale.", pct(m.scale), pct(scale)),
                _ => "It was drawn by an older version of Olo: drawn again as this one draws it.".into(),
            },
            fix: Some(Fix { label: "Update It".into(), root: true, steps: install() }),
        }),
        Installed::NotInImage => r.findings.push(Finding {
            id: "start-up-screen:image".into(),
            severity: Severity::Advice,
            title: "Olo's start-up screen isn't in the start-up image yet".into(),
            detail: "It's installed, but the start-up image (initramfs) the computer starts from was made before it, so the computer starts with what was there. Making the image again takes a minute or two.".into(),
            fix: Some(Fix { label: "Make It Again".into(), root: true, steps: vec![Step::RebuildInitramfs] }),
        }),
        Installed::Current => {
            let title = "Olo's start-up screen shows as the computer starts";
            r.fine.push(Fine { title: title.into(), detail: format!("Drawn at {}, in the start-up image.", pct(scale)) });
            // The way back: Plymouth's settings as they were (or its
            // defaults), the image made again.
            let before = roots.fs.join(splash::CONF_BEFORE.trim_start_matches('/'));
            let mut steps = if before.exists() {
                vec![Step::CopyFile { from: PathBuf::from(splash::CONF_BEFORE), to: PathBuf::from(splash::CONF), mode: 0o644 }, Step::RemoveFile { path: PathBuf::from(splash::CONF_BEFORE) }]
            } else {
                vec![Step::RemoveFile { path: PathBuf::from(splash::CONF) }]
            };
            steps.push(Step::RebuildInitramfs);
            r.undo.push(Undo { id: "start-up-screen:undo".into(), title: title.into(), fix: Fix { label: "Use the One Before".into(), root: true, steps } });
        }
    }
}

/// Olo's start-up chime as the computer starts (apps/chime, its service
/// data/systemd/olo-chime.service): from the computer's own speakers
/// while the start-up screen shows, as a Mac plays its own (Ben,
/// 2026-11-13), not as the person logs in. The program runs from a copy
/// of its own, as root, before anyone logs in.
pub const CHIME_SERVICE: &str = "olo-chime.service";
pub const CHIME_UNIT: &str = include_str!("../../../data/systemd/olo-chime.service");
const CHIME_UNIT_PATH: &str = "/etc/systemd/system/olo-chime.service";
/// Its `WantedBy=sound.target`, as enabling it makes it.
const CHIME_WANTED: &str = "/etc/systemd/system/sound.target.wants/olo-chime.service";

fn chime_program() -> PathBuf {
    Path::new(crate::login_screen::LIBEXEC).join("olo-chime")
}

/// The chime: not installed, a suggestion (none where the person turned
/// the start-up sound off, nor without a built-in sound card with
/// speakers: `olo_chime::cards`); installed from an older Olo (the
/// program newer than its copy, as the login screen's are told, or its
/// unit not this one's): Update It; else fine, with the way back to the
/// chime as the session starts. Nothing without olo-chime installed.
fn start_up_chime(roots: &Roots, r: &mut Report) {
    let bin = roots.bin.join("olo-chime");
    if !bin.is_file() {
        return;
    }
    let under = |p: &str| roots.fs.join(p.trim_start_matches('/'));
    let program = chime_program();
    let copy = under(&program.to_string_lossy());
    let written = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let old_copy = match (written(&bin), written(&copy)) {
        (Some(new), Some(old)) => new > old,
        _ => true,
    };
    let unit = std::fs::read_to_string(under(CHIME_UNIT_PATH)).ok();
    let old_unit = unit.as_deref() != Some(CHIME_UNIT);
    let enabled = under(CHIME_WANTED).exists();
    let installed = unit.is_some() && enabled;
    let mut steps = Vec::new();
    if old_copy {
        steps.push(Step::CopyFile { from: bin.clone(), to: program.clone(), mode: 0o755 });
        // (Labelled as its folder's programs are: one systemd may run.)
        steps.push(Step::SelinuxLabels { rules: Vec::new(), restore: vec![program.clone()] });
    }
    if old_unit {
        steps.push(Step::WriteFile { path: PathBuf::from(CHIME_UNIT_PATH), content: CHIME_UNIT.into(), mode: 0o644 });
        steps.push(Step::Unit { name: CHIME_SERVICE.into(), action: UnitAction::Reload });
    }
    if old_unit || !enabled {
        steps.push(Step::Unit { name: CHIME_SERVICE.into(), action: UnitAction::Enable });
    }
    if !installed {
        let settings: olo_protocol::Settings = read(&roots.temp.config.join("olo/app_server.json")).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if !settings.startup_sound || olo_chime::cards::built_in(&olo_chime::cards::cards(&roots.fs)).is_none() {
            return;
        }
        r.findings.push(Finding {
            id: "start-up-chime".into(),
            severity: Severity::Suggestion,
            title: "Play Olo's chime as the computer starts".into(),
            detail: "Olo's chime plays as you log in. It can play as the computer starts instead, as a Mac's does: from its own speakers while the start-up screen shows, at the volume they were last set to (none while they're muted). It plays the chime chosen in Preferences ▸ Sound, and none if anyone on this computer turned the start-up sound off.".into(),
            fix: Some(Fix { label: "Play It as the Computer Starts".into(), root: true, steps }),
        });
        return;
    }
    if !steps.is_empty() {
        r.findings.push(Finding {
            id: "start-up-chime:older".into(),
            severity: Severity::Advice,
            title: "Olo's start-up chime is an older version".into(),
            detail: format!(
                "It plays as the computer starts from a copy of its own ({}) that an upgrade doesn't replace, {}. The fix brings it up to date.",
                program.display(),
                if old_copy { "and this one's is newer" } else { "and its service isn't set up as this version sets it up" }
            ),
            fix: Some(Fix { label: "Update It".into(), root: true, steps }),
        });
        return;
    }
    let title = "Olo's chime plays as the computer starts";
    r.fine.push(Fine { title: title.into(), detail: "From the computer's own speakers, while the start-up screen shows.".into() });
    r.undo.push(Undo {
        id: "start-up-chime:undo".into(),
        title: title.into(),
        fix: Fix {
            label: "Play It as You Log In".into(),
            root: true,
            steps: vec![
                Step::Unit { name: CHIME_SERVICE.into(), action: UnitAction::Disable },
                Step::RemoveFile { path: PathBuf::from(CHIME_UNIT_PATH) },
                Step::RemoveFile { path: program },
                Step::Unit { name: CHIME_SERVICE.into(), action: UnitAction::Reload },
            ],
        },
    });
}

/// What a fix needs done as the person before its steps run, on a worker
/// (they may take a moment): a suggestion's state kept for its undo
/// (`suggest::before`); the start-up screen drawn where its steps copy
/// it from (`olo-app-server --plymouth-theme`: Olo's compositor draws
/// it, as the person, at their screen's scale).
pub fn prepare(id: &str) -> Result<(), String> {
    crate::suggest::before(id);
    if id == START_UP_SCREEN || id == START_UP_SCREEN_OLD {
        // (Not `Roots::real`: that asks systemd's manager, up to 10 s.)
        let bin = PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/bin");
        let to = splash_drawn(&olo_xdg::cache_dir());
        let _ = std::fs::remove_dir_all(&to);
        let out = std::process::Command::new(bin.join("olo-app-server")).arg("--plymouth-theme").arg(&to).stdin(std::process::Stdio::null()).output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => return Err(format!("Olo's start-up screen couldn't be drawn: {}", String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("olo-app-server: "))),
            Err(e) => return Err(format!("Olo's start-up screen couldn't be drawn: {e}")),
        }
    }
    Ok(())
}

/// DiskBackup, as its settings and its service's state say: not set up
/// (said, no button: it's the person's to choose where backups go),
/// its service not running (Start It), paused (Resume Backups), nothing
/// finished for days (Back Up Now), its key only on this computer (Keep
/// Key in Passwords: without it, the backups can't be read elsewhere);
/// else when it last backed up, as fine.
/// What Olo needs from Fedora's repositories to run (`crate::needs`),
/// where it's missing: on Fedora, Olo installed (its session there), and
/// PackageKit answering.
fn needed_packages(roots: &Roots, r: &mut Report) {
    let fedora = read(&roots.fs.join("etc/os-release")).is_some_and(|t| t.lines().any(|l| l.trim() == "ID=fedora"));
    if !fedora || !roots.bin.join("olo-session").exists() {
        return;
    }
    let Some(found) = (roots.packages)(crate::needs::PACKAGES) else { return };
    let missing: Vec<String> = crate::needs::PACKAGES.iter().filter(|n| !found.iter().any(|(f, installed)| f == *n && *installed)).map(|n| (*n).to_owned()).collect();
    if missing.is_empty() {
        return;
    }
    r.findings.push(Finding {
        id: "packages:needed".into(),
        severity: Severity::Problem,
        title: if missing.len() == 1 { format!("Olo needs {}", missing[0]) } else { format!("Olo needs {} packages that aren't installed", missing.len()) },
        detail: format!(
            "{} {} from Fedora's repositories, which Olo's programs need to run (a computer set up without a desktop comes without {}).",
            missing.join(", "),
            if missing.len() == 1 { "isn't installed" } else { "aren't installed" },
            if missing.len() == 1 { "it" } else { "them" }
        ),
        fix: Some(Fix { label: "Install Them".into(), root: true, steps: vec![Step::InstallPackages { names: missing }] }),
    });
}

/// The services software is installed through (PackageKit, dnf's): a
/// minimal Fedora has neither, so Olo's fixes that install packages, OS
/// Updates and the Installer couldn't (Ben, 2026-11-15: "just have olo
/// installer and olo doctor install the needed packagekit/dnf stuff").
/// Only on Fedora, where they'd be.
fn package_services(roots: &Roots, r: &mut Report) {
    let fedora = read(&roots.fs.join("etc/os-release")).is_some_and(|t| t.lines().any(|l| l.trim() == "ID=fedora"));
    let missing = (roots.package_services)();
    if !fedora || missing.is_empty() {
        return;
    }
    let names = missing.join(" and ");
    let (is, them, they_are) = if missing.len() == 1 { ("isn't", "it", "it is") } else { ("aren't", "them", "they are") };
    r.findings.push(Finding {
        id: "packages:services".into(),
        severity: Severity::Problem,
        title: "Software can't be installed here".into(),
        detail: format!(
            "{names} {is} installed (a minimal Fedora comes without {them}): Olo installs software and updates through {them}, so its fixes that install something, and OS Updates, can't until {they_are}. Installing {them} uses Fedora's own dnf, this once."
        ),
        fix: Some(Fix { label: if missing.len() == 1 { "Install It" } else { "Install Them" }.into(), root: true, steps: vec![Step::InstallPackageServices] }),
    });
}

/// kache, the build cache (Olo's Kache app, `apps/kache`), where it's
/// used (its store in the cache folder): its daemon running, else builds
/// compile everything again and nothing keeps the store under its limit;
/// and the store able to get under its limit: its last clean-up
/// (`gc_stats.json`) couldn't take what builds' target folders still share
/// with it, and when that alone is more than the limit, no clean-up gets
/// it under.
fn kache(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let Some(store) = roots.temp.cache.parent().filter(|c| !c.as_os_str().is_empty()).map(|c| c.join("kache")) else { return };
    if !store.is_dir() {
        return;
    }
    let args = |pid: i32| -> Vec<String> {
        std::fs::read(roots.fs.join(format!("proc/{pid}/cmdline"))).unwrap_or_default().split(|b| *b == 0).skip(1).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
    };
    // (`kache-engine daemon run`, as its service runs it; `kache daemon …`
    // before.)
    let running = procs.iter().any(|p| p.uid == roots.uid && p.name.starts_with("kache") && args(p.pid).iter().any(|a| a == "daemon"));
    let open = || Some(Fix { label: "Open Kache".into(), root: false, steps: vec![Step::StartProgram { argv: vec!["olo-kache".into()] }] });
    if !running {
        if roots.temp.config.join("systemd/user/kache.service").exists() {
            r.findings.push(Finding {
                id: "kache:not-running".into(),
                severity: Severity::Advice,
                title: "The build cache isn't running".into(),
                detail: "kache's daemon (kache.service) isn't running: builds go on, but compile everything again, and nothing keeps kache's store under its limit until it starts.".into(),
                fix: Some(Fix { label: "Start It".into(), root: false, steps: vec![Step::StartProgram { argv: vec!["olo-kache".into(), "--start".into()] }] }),
            });
        }
        return;
    }
    let gc: serde_json::Value = read(&store.join("gc_stats.json")).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let n = |k: &str| gc[k].as_u64().unwrap_or(0);
    let held = n("bytes_held");
    let limit = kache_limit(roots, &store);
    // (A clean-up that looked at no entries, as the sweep of locks after a
    // restart, says nothing of what's held.)
    let looked = n("entries_evicted") + n("entries_unreclaimable") + n("entries_pinned") + held > 0;
    if !looked {
        r.fine.push(Fine { title: "The build cache is running".into(), detail: "kache's daemon is running.".into() });
    } else if held > limit {
        let n = gc["entries_unreclaimable"].as_u64().unwrap_or(0);
        r.findings.push(Finding {
            id: "kache:over-limit".into(),
            severity: Severity::Advice,
            title: "The build cache can't keep under its limit".into(),
            detail: format!(
                "Builds' target folders still share {} with kache's store ({n} entries), more than its {} limit: no clean-up can take them while those folders are there. They go as the folders do: have kache clean idle target folders sooner (Kache ▸ Settings ▸ Idle targets), or remove old projects' target folders.",
                gb(held),
                gb(limit)
            ),
            fix: open(),
        });
    } else {
        r.fine.push(Fine { title: "The build cache is running".into(), detail: format!("kache's daemon is running, and its store can keep under its {} limit.", gb(limit)) });
    }
}

/// kache's store's limit: its settings' `local_max_size`, else as kache
/// sets it, 5% of the store's disk rounded to a GiB, from 5 to 100 GiB.
fn kache_limit(roots: &Roots, store: &Path) -> u64 {
    const GIB: u64 = 1 << 30;
    let set = read(&roots.temp.config.join("kache/config.toml"))
        .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|d| d.get("cache")?.get("local_max_size").and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_integer().map(|i| i.to_string()))));
    if let Some(bytes) = set.as_deref().and_then(size_of) {
        return bytes;
    }
    let disk = std::ffi::CString::new(store.as_os_str().as_encoded_bytes()).ok().and_then(|c| {
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: statvfs fills `s` from a NUL-terminated path.
        (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_blocks as u64 * s.f_frsize as u64)
    });
    disk.map_or(50 * GIB, |d| ((d / 20 + GIB / 2) / GIB * GIB).clamp(5 * GIB, 100 * GIB))
}

/// A size as kache reads one ("95GiB", "1.5 TB", bytes).
fn size_of(v: &str) -> Option<u64> {
    let v = v.trim();
    if let Ok(n) = v.parse::<u64>() {
        return Some(n);
    }
    let end = v.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(v.len());
    let (n, unit) = v.split_at(end);
    let n: f64 = n.parse().ok()?;
    let factor: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        "t" | "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024f64.powi(2),
        "gib" => 1024f64.powi(3),
        "tib" => 1024f64.powi(4),
        _ => return None,
    };
    Some((n * factor) as u64)
}

/// Bytes, as people read them ("450 GB").
fn gb(n: u64) -> String {
    let g = n as f64 / 1e9;
    if g >= 10.0 { format!("{g:.0} GB") } else { format!("{g:.1} GB") }
}

fn diskbackup(roots: &Roots, r: &mut Report) {
    use olo_protocol::diskbackup::{Settings, State, Target};
    if roots.temp.config.as_os_str().is_empty() {
        return;
    }
    let settings: Settings = read(&roots.temp.config.join("olo/diskbackup.json")).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if settings.target == Target::None {
        r.findings.push(Finding {
            id: "diskbackup:not-set-up".into(),
            severity: Severity::Advice,
            title: "Nothing is backed up".into(),
            detail: "DiskBackup isn't set up. Open DiskBackup and choose where backups go (a disk of its own, or your Google Drive): from then on your home folder is backed up every hour, at the computer's idle priority, encrypted, its key kept in Passwords.".into(),
            fix: None,
        });
        return;
    }
    let state: State = read(&roots.temp.runtime.join("olo-diskbackup.json")).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let running = state.pid != 0 && roots.fs.join(format!("proc/{}", state.pid)).exists();
    let start = |label: &str, argv: &[&str]| Some(Fix { label: label.into(), root: false, steps: vec![Step::StartProgram { argv: argv.iter().map(|a| a.to_string()).collect() }] });
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    if !running {
        r.findings.push(Finding {
            id: "diskbackup:not-running".into(),
            severity: Severity::Problem,
            title: "DiskBackup isn't running".into(),
            detail: format!("Its service (olo-diskbackup --service) starts with your session and isn't running: nothing is backed up to {} until it does.", settings.target.describe()),
            fix: start("Start It", &["olo-diskbackup", "--service"]),
        });
    } else if !settings.enabled {
        r.findings.push(Finding {
            id: "diskbackup:paused".into(),
            severity: Severity::Advice,
            title: "Backups are paused".into(),
            detail: format!("DiskBackup is set up to back up to {}, but turned off: what changes now isn't backed up.", settings.target.describe()),
            fix: start("Resume Backups", &["olo-diskbackup", "--set", "enabled=on"]),
        });
    } else {
        // Nothing whole for three days, or three of its intervals; backing
        // up only when asked, a week (advice: it's the person's to do).
        let manual = !settings.automatic;
        let overdue = if manual { 7 * 86_400 } else { (3 * 86_400).max(3 * 60 * settings.every_minutes as i64) };
        let since = state.last_ok.map(|t| now - t);
        if manual && since.is_some_and(|s| s > overdue) && state.doing == olo_protocol::diskbackup::Doing::Idle {
            r.findings.push(Finding {
                id: "diskbackup:overdue".into(),
                severity: Severity::Advice,
                title: format!("Not backed up for {} days", since.unwrap_or_default() / 86_400),
                detail: format!("DiskBackup backs up to {} when you ask it to, and hasn't for a while.", settings.target.describe()),
                fix: start("Back Up Now", &["olo-diskbackup", "--now"]),
            });
        } else if !manual && since.is_none_or(|s| s > overdue) && state.doing == olo_protocol::diskbackup::Doing::Idle {
            let when = match since {
                Some(s) => format!("for {} days", s / 86_400),
                None => "yet".into(),
            };
            let why = state.last.as_ref().filter(|l| !l.stopped()).and_then(|l| l.error.clone()).map(|e| format!(" The last didn't finish: {e}.")).unwrap_or_default();
            r.findings.push(Finding {
                id: "diskbackup:overdue".into(),
                severity: Severity::Problem,
                title: format!("Nothing backed up {when}"),
                detail: format!("DiskBackup backs up to {} every {} minutes, and nothing has finished {when}.{why}", settings.target.describe(), settings.every_minutes),
                fix: start("Back Up Now", &["olo-diskbackup", "--now"]),
            });
        } else if let Some(s) = since {
            let ago = match s {
                ..120 => "a minute ago".to_owned(),
                120..3600 => format!("{} minutes ago", s / 60),
                3600..7200 => "an hour ago".to_owned(),
                7200..86_400 => format!("{} hours ago", s / 3600),
                86_400..172_800 => "a day ago".to_owned(),
                _ => format!("{} days ago", s / 86_400),
            };
            r.fine.push(Fine { title: "Your files are backed up".into(), detail: format!("DiskBackup last backed up to {} {ago}.", settings.target.describe()) });
        }
    }
    // The last backup, checked once it was made, found not whole.
    if let Some(c) = state.last.as_ref().and_then(|l| l.check.as_ref()).filter(|c| !c.problems.is_empty()) {
        r.findings.push(Finding {
            id: "diskbackup:damaged".into(),
            // (What's backed up may not come back: critical.)
            severity: Severity::Critical,
            title: "The last backup isn't whole".into(),
            detail: format!(
                "Checked once it was made, it was missing what some files need ({}): those can't be restored as they were. The damaged parts are let go of, and the next backup sends again what it can.",
                c.problems.join("; ")
            ),
            fix: start("Back Up Now", &["olo-diskbackup", "--now"]),
        });
    }
    if !settings.key_in_passwords {
        r.findings.push(Finding {
            id: "diskbackup:key".into(),
            severity: Severity::Advice,
            title: "DiskBackup's key is only on this computer".into(),
            detail: "The backups are encrypted with a key kept in this computer's keyring alone: if this computer is lost, they can't be read. Keep it in Passwords too (and so in 1Password), and any computer can restore them.".into(),
            fix: start("Keep Key in Passwords", &["olo-diskbackup", "--keep-key"]),
        });
    }
}

/// The D-Bus name other programs ask to show a file in its folder
/// (Chrome's Show in folder, an app's Open Containing Folder).
const FILE_MANAGER: &str = "org.freedesktop.FileManager1";

/// The user's D-Bus activation file for [`FILE_MANAGER`] that starts
/// Tracker (`olo-tracker --file-manager`, at `tracker`).
fn file_manager_service(tracker: &Path) -> String {
    format!(
        "# Olo Doctor's \"Always Use Tracker\": D-Bus starts Tracker when a program asks for\n# the file manager and no Tracker has the name yet. Removing this file undoes it.\n[D-BUS Service]\nName={FILE_MANAGER}\nExec={} --file-manager\n",
        tracker.display()
    )
}

/// Whether programs' "show in folder" always reaches Tracker (Ben,
/// 2026-11-05: "doctor should offer to install the change that makes it
/// always use tracker"). The desktop's Tracker takes the name from
/// whoever has it, but while none has it (as Olo restarts after an
/// upgrade, or before Tracker starts) D-Bus starts the system's activation
/// file's program for it: GNOME's Files (Nautilus) on Fedora, which then
/// keeps the name till Tracker waits it out. A user's own activation file
/// wins over the system's: the fix writes one that starts Tracker
/// (`--file-manager`), and has the session bus read it at once. It is the
/// user's, so it applies in another desktop's session on this account
/// too. An Olo session now names Tracker for it itself, in the runtime
/// folder's services (which the bus reads first, gone with the login:
/// olo-session's graphical.rs), so there it's fine with nothing to do.
fn file_manager(roots: &Roots, r: &mut Report) {
    let tracker = roots.bin.join("olo-tracker");
    if !tracker.is_file() {
        return;
    }
    let mine = roots.temp.data.join("dbus-1/services").join(format!("{FILE_MANAGER}.service"));
    let session = roots.temp.runtime.join("dbus-1/services").join(format!("{FILE_MANAGER}.service"));
    let starts_tracker =
        |text: &str| text.lines().any(|l| l.trim().strip_prefix("Exec=").and_then(|e| e.split_whitespace().next()).is_some_and(|p| Path::new(p).file_name().is_some_and(|n| n == "olo-tracker")));
    if read(&session).is_some_and(|t| starts_tracker(&t)) {
        r.fine.push(Fine {
            title: "Programs' “Show in folder” opens Tracker".into(),
            detail: format!("While Olo runs, D-Bus starts Tracker when a program asks for the file manager and none is running yet (Olo's session names it, in {}).", session.display()),
        });
        return;
    }
    let content = file_manager_service(&tracker);
    // (Read whole: `read` trims.)
    if std::fs::read_to_string(&mine).is_ok_and(|t| t == content) {
        r.fine.push(Fine {
            title: "Programs' “Show in folder” opens Tracker".into(),
            detail: format!("D-Bus starts Tracker when a program asks for the file manager and none is running yet ({}).", mine.display()),
        });
        return;
    }
    // What D-Bus would start now: the user's own file, else the system's.
    let exec = |text: String| text.lines().find_map(|l| l.trim().strip_prefix("Exec=").map(|e| e.split_whitespace().next().unwrap_or("").to_owned()));
    let theirs = read(&mine)
        .and_then(exec)
        .or_else(|| ["usr/local/share/dbus-1/services", "usr/share/dbus-1/services"].iter().find_map(|d| read(&roots.fs.join(d).join(format!("{FILE_MANAGER}.service"))).and_then(exec)));
    let (title, now) = match theirs.as_deref().map(|p| Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p)) {
        Some("nautilus") => {
            ("Programs may open GNOME's Files instead of Tracker".to_owned(), "D-Bus starts GNOME's Files (Nautilus) for it, which then keeps the name till Tracker waits it out".to_owned())
        }
        Some(program) => (format!("Programs may open {program} instead of Tracker"), format!("D-Bus starts {program} for it")),
        None => ("Programs' “Show in folder” can go nowhere".to_owned(), "nothing answers it".to_owned()),
    };
    r.findings.push(Finding {
        id: "dbus:file-manager".into(),
        severity: Severity::Advice,
        title,
        detail: format!(
            "When a program asks the file manager to show something (Chrome's Show in folder, an app's Open Containing Folder) while no Tracker is answering yet (as Olo restarts after an upgrade), {now}. The fix installs {} so D-Bus always starts Tracker instead. It's yours alone, and also applies in another desktop's session on this account; removing it undoes it.",
            mine.display()
        ),
        fix: Some(Fix { label: "Always Use Tracker".into(), root: false, steps: vec![Step::WriteFile { path: mine, content, mode: 0o644 }, Step::SessionBusReload] }),
    });
}

/// Programs' temporary files on disk, not in memory (Ben, 2026-11-05:
/// "have olo doctor check what the env vars are for telling processes what
/// temp dir path to use and if they're not present or not set to /var/tmp
/// then have it provide that as a fix"): with /tmp a tmpfs (Fedora's), a
/// session whose `TMPDIR` is unset (so /tmp) or in memory too has every
/// build directory, download and copy a program makes there take RAM.
fn temp_files(roots: &Roots, r: &mut Report) {
    let t = &roots.temp;
    match &t.tmpdir {
        Some(d) if !t.tmpdir_in_memory => {
            r.fine.push(Fine { title: "Temporary files are kept on disk".into(), detail: format!("Programs keep them in {d} (TMPDIR), not in memory.") });
            return;
        }
        None if !t.tmp_in_memory => {
            r.fine.push(Fine { title: "Temporary files are kept on disk".into(), detail: "/tmp is on disk here.".into() });
            return;
        }
        _ => {}
    }
    let file = t.config.join(olo_system::env::TMPDIR_FILE);
    let vars: Vec<(String, String)> = olo_system::env::TMP_VARS.iter().map(|k| ((*k).to_owned(), olo_system::env::DISK_TMP.to_owned())).collect();
    let content = format!(
        "# Olo Doctor: programs' temporary files on disk ({}), not in memory.\n# Remove this file to undo (from the next login).\n{}",
        olo_system::env::DISK_TMP,
        vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect::<String>()
    );
    let now = match &t.tmpdir {
        Some(d) => format!("TMPDIR is {d}, in memory too"),
        None => "TMPDIR isn't set, so they go to /tmp".into(),
    };
    r.findings.push(Finding {
        id: "tmp:memory".into(),
        severity: Severity::Advice,
        title: "Temporary files take memory".into(),
        detail: format!(
            "/tmp is held in memory here, and programs keep their temporary files there ({now}): a build, a download, a copy of something big all take RAM until they're removed. The fix tells programs to use {disk} instead (TMPDIR, TMP and TEMP), on disk: for everything started from now on (programs already running keep theirs until they start again), and from each login after, through {file}. Removing that file undoes it.",
            disk = olo_system::env::DISK_TMP,
            file = file.display()
        ),
        fix: Some(Fix {
            label: "Keep Temporary Files on Disk".into(),
            root: false,
            steps: vec![Step::WriteFile { path: file, content, mode: 0o644 }, Step::SetSessionEnvironment { vars }],
        }),
    });
}

fn remappers(roots: &Roots, procs: &[Proc], input_names: &[String], r: &mut Report) {
    let mut any = false;
    for (program, unit, what) in REMAPPERS {
        let running: Vec<&Proc> = procs.iter().filter(|p| p.name == *program).collect();
        let state = (roots.units)(unit).unwrap_or_default();
        if running.is_empty() && !state.enabled {
            continue;
        }
        any = true;
        let virtuals: Vec<&String> = input_names.iter().filter(|n| n.to_lowercase().contains(program.split('-').next().unwrap_or(program))).collect();
        let mut steps = Vec::new();
        let mut title = String::new();
        let mut detail = format!("{program} is {what}: it grabs the keyboards (and any mouse with keys on its motion node, such as a Logitech G309) and types through a virtual one");
        if !virtuals.is_empty() {
            detail.push_str(&format!(" ({})", virtuals.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
        }
        detail.push_str(". Olo maps the Mac keys and Caps Lock itself (Preferences ▸ Keyboard), and a mouse it can't reach ignores Olo's speed and acceleration settings.");
        if !running.is_empty() {
            title.push_str(&format!("{program} is running"));
            if state.active {
                steps.push(Step::Unit { name: unit.to_string(), action: UnitAction::Stop });
            } else {
                // Started by hand: stopped as a process.
                steps.extend(running.iter().map(|p| Step::Signal { pid: p.pid, signal: libc::SIGTERM }));
            }
        }
        if state.enabled {
            if title.is_empty() {
                title.push_str(&format!("{program} starts at boot"));
            } else {
                title.push_str(" and starts at boot");
            }
            steps.push(Step::Unit { name: unit.to_string(), action: UnitAction::Disable });
        }
        // A copy installed by hand under /usr/local (not a package's):
        // taken away too, so nothing brings it back.
        let local_bin = roots.fs.join("usr/local/bin").join(program);
        let hand_installed = local_bin.exists() && state.path.as_deref().is_some_and(|p| p.starts_with("/usr/local/") || p.starts_with("/etc/systemd/"));
        let label = if hand_installed {
            steps.push(Step::RemoveFile { path: local_bin });
            if let Some(p) = &state.path {
                steps.push(Step::RemoveFile { path: roots.fs.join(p.trim_start_matches('/')) });
            }
            steps.push(Step::Unit { name: unit.to_string(), action: UnitAction::Reload });
            detail.push_str(&format!(" It was installed by hand (/usr/local/bin/{program}): the fix stops it, takes it off the boot and removes it; its configuration under /etc is left for you."));
            format!("Stop and Uninstall {program}")
        } else {
            detail.push_str(" The fix stops it and takes it off the boot.");
            format!("Stop {program}")
        };
        r.findings.push(Finding { id: format!("remapper:{program}"), severity: Severity::Problem, title, detail, fix: Some(Fix { label, root: true, steps }) });
    }
    if !any {
        r.fine.push(Fine { title: "No input remapper is running".into(), detail: "Olo reads the keyboards and mice itself.".into() });
    }
}

fn foreign(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let ours = |p: &Proc| p.uid == roots.uid;
    // Another login's desktop: everything of it is its own.
    let elsewhere = procs.iter().filter(|p| ours(p) && DESKTOPS.contains(&p.name.as_str())).any(|p| p.scope.is_some() && p.scope != roots.scope);
    let mine: Vec<(&Proc, &str)> = if elsewhere {
        Vec::new()
    } else {
        procs
            .iter()
            .filter(|p| ours(p) && (p.scope.is_none() || p.scope == roots.scope))
            .filter_map(|p| FOREIGN.iter().find(|(n, _)| *n == p.name).map(|(_, what)| (p, *what)))
            .filter(|(p, _)| !on_a_screen(&roots.fs, p.pid))
            .collect()
    };
    if mine.is_empty() {
        r.fine.push(Fine { title: "No other desktop's daemons run in this session".into(), detail: "Nothing fights Olo for the keys or the power settings.".into() });
        return;
    }
    let names = by_program(&mine);
    let steps = mine.iter().map(|(p, _)| Step::Signal { pid: p.pid, signal: libc::SIGTERM }).collect();
    r.findings.push(Finding {
        id: "foreign".into(),
        severity: Severity::Problem,
        title: if names.len() == 1 { "A program of another desktop runs in this session".into() } else { format!("{} programs of another desktop run in this session", names.len()) },
        detail: format!("{}. They answer to keys and power events meant for Olo, and some grab devices. Olo does all of this itself; they aren't needed here and are quit.", names.join("; ")),
        fix: Some(Fix { label: "Quit Them".into(), root: false, steps }),
    });
}

/// Programs found running, each said once with its processes (a portal
/// backend D-Bus started six times is one program, not six): "GTK's
/// portal backend (xdg-desktop-portal-gtk, processes 1, 2 and 3)", and
/// what follows a `|` in its description after them (", where olo-portal
/// is the backend").
fn by_program(found: &[(&Proc, &str)]) -> Vec<String> {
    let mut programs: Vec<(&str, &str, Vec<i32>)> = Vec::new();
    for (p, what) in found {
        match programs.iter_mut().find(|(name, ..)| *name == p.name) {
            Some((.., pids)) => pids.push(p.pid),
            None => programs.push((&p.name, what, vec![p.pid])),
        }
    }
    programs
        .into_iter()
        .map(|(name, what, pids)| {
            let (what, then) = what.split_once('|').unwrap_or((what, ""));
            match pids.as_slice() {
                [pid] => format!("{what} ({name}, process {pid}){then}"),
                [rest @ .., last] => format!("{what} ({name}, processes {} and {last}){then}", rest.iter().map(i32::to_string).collect::<Vec<_>>().join(", ")),
                [] => String::new(),
            }
        })
        .collect()
}

/// Nothing of GNOME's needed: its services Olo has its own in place of
/// (`GNOME_SERVICES`), if any runs in this session (not when GNOME is
/// logged in elsewhere: they're its own), quit by the fix.
fn gnome_services(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let ours = |p: &Proc| p.uid == roots.uid;
    if procs.iter().filter(|p| ours(p) && DESKTOPS.contains(&p.name.as_str())).any(|p| p.scope.is_some() && p.scope != roots.scope) {
        return;
    }
    let found: Vec<(&Proc, &str)> = procs
        .iter()
        .filter(|p| ours(p) && (p.scope.is_none() || p.scope == roots.scope))
        .filter_map(|p| GNOME_SERVICES.iter().find(|(n, _)| *n == p.name).map(|(_, what)| (p, *what)))
        .filter(|(p, _)| !on_a_screen(&roots.fs, p.pid))
        .collect();
    if found.is_empty() {
        // (GNOME's settings daemons are another desktop's: `foreign` says so.)
        if procs.iter().any(|p| ours(p) && (p.scope.is_none() || p.scope == roots.scope) && FOREIGN.iter().any(|(n, _)| *n == p.name)) {
            return;
        }
        r.fine.push(Fine { title: "Nothing of GNOME's runs".into(), detail: "Olo answers for dconf, the portals and the keyring itself, and programs it starts use GIO's own files.".into() });
        return;
    }
    let names = by_program(&found);
    let gvfs = found.iter().any(|(p, _)| p.name.starts_with("gvfs"));
    r.findings.push(Finding {
        id: "gnome:services".into(),
        severity: Severity::Advice,
        title: if names.len() == 1 { "One of GNOME's services runs".into() } else { format!("{} of GNOME's services run", names.len()) },
        detail: format!(
            "{}. Olo has its own in their place; something started these before Olo's took over (the start of the session, or a program started before an upgrade), and they aren't needed. Quitting them is safe: what asks for dconf, a portal or the keyring reaches Olo's.{}",
            names.join("; "),
            if gvfs { " Programs started before keep asking gvfs for network folders until they're started again." } else { "" }
        ),
        fix: Some(Fix { label: "Quit Them".into(), root: false, steps: found.iter().map(|(p, _)| Step::Signal { pid: p.pid, signal: libc::SIGTERM }).collect() }),
    });
}

/// Is Olo's input rule installed as it should be? Adds the finding (or
/// the fine) and says whether it is.
fn input_rule(roots: &Roots, r: &mut Report) -> bool {
    let path = roots.fs.join(INPUT_RULE_PATH);
    let installed = std::fs::read_to_string(&path).ok();
    let old = roots.fs.join(INPUT_RULE_OLD_PATH).exists();
    if installed.as_deref() == Some(INPUT_RULE) && !old {
        r.fine.push(Fine { title: "Olo's input rule is installed".into(), detail: "USB mice, keyboards, receivers and security keys are never put to sleep (95-olo-input.rules).".into() });
        return true;
    }
    let mut steps = input_rule_steps(roots);
    // Applied to what is plugged in now (udev reads the rule itself for
    // what comes later).
    for d in usb_inputs(&roots.fs) {
        if d.control != "on" {
            steps.push(Step::WriteSysfs { path: PathBuf::from("/sys/bus/usb/devices").join(&d.port).join("power/control"), value: "on".into() });
        }
    }
    let (title, verb) = if installed.is_some() || old { ("Olo's input rule is out of date", "updates") } else { ("Olo's input rule isn't installed", "installs") };
    r.findings.push(Finding {
        id: "rule:input".into(),
        severity: Severity::Problem,
        title: title.into(),
        detail: format!("Without it, a USB mouse, keyboard or wireless receiver may be put to sleep after a pause and lose the first reports when the hand moves again (the pointer sits still for a moment). A security key that is a smart card too is put to sleep by the smart-card readers' own rule unless Olo's comes after it. The fix {verb} /etc/udev/rules.d/95-olo-input.rules and applies it to what is plugged in now."),
        fix: Some(Fix { label: "Install the Rule".into(), root: true, steps }),
    });
    false
}

/// The steps that put Olo's input rule in place: the file, and its old
/// one taken away where it's still there.
fn input_rule_steps(roots: &Roots) -> Vec<Step> {
    let mut steps = vec![Step::WriteFile { path: PathBuf::from("/").join(INPUT_RULE_PATH), content: INPUT_RULE.into(), mode: 0o644 }];
    if roots.fs.join(INPUT_RULE_OLD_PATH).exists() {
        steps.push(Step::RemoveFile { path: PathBuf::from("/").join(INPUT_RULE_OLD_PATH) });
    }
    steps
}

fn autosuspend(roots: &Roots, rule_current: bool, r: &mut Report) {
    let devices = usb_inputs(&roots.fs);
    let asleep: Vec<&UsbInput> = devices.iter().filter(|d| d.control != "on").collect();
    if asleep.is_empty() {
        if !devices.is_empty() {
            r.fine.push(Fine {
                title: format!("{} awake", plural(devices.len(), "USB input device stays", "USB input devices stay")),
                detail: devices.iter().map(UsbInput::label).collect::<Vec<_>>().join(", "),
            });
        }
        return;
    }
    for d in asleep {
        // (The rule is what keeps it so from its next plugging in and
        // the computer's next start; the value is for now.)
        let mut steps = if rule_current { Vec::new() } else { input_rule_steps(roots) };
        steps.push(Step::WriteSysfs { path: PathBuf::from("/sys/bus/usb/devices").join(&d.port).join("power/control"), value: "on".into() });
        let _ = &d.dir;
        r.findings.push(Finding {
            id: format!("autosuspend:{}", d.port),
            severity: Severity::Problem,
            title: format!("{} may be put to sleep", d.label()),
            detail: format!(
                "USB power management autosuspends it (power/control is “{}” at USB port {}): a receiver asleep loses the first reports when the hand moves again, and the pointer sits still for a moment. The fix keeps it on{}.",
                d.control,
                d.port,
                if rule_current { "" } else { " and installs Olo's input rule so it stays on" }
            ),
            fix: Some(Fix { label: "Keep It Awake".into(), root: true, steps }),
        });
    }
}

fn display_rule(roots: &Roots, r: &mut Report) {
    let path = roots.fs.join(DISPLAY_RULE_PATH);
    let installed = std::fs::read_to_string(&path).ok();
    let displays = usb_with_ids(&roots.fs, &rule_ids(DISPLAY_RULE));
    if installed.as_deref() == Some(DISPLAY_RULE) {
        r.fine.push(Fine { title: "Olo's display rule is installed".into(), detail: "The brightness of USB-controlled monitors can be set from the seat (70-olo-displays.rules).".into() });
        return;
    }
    let mut steps = vec![Step::WriteFile { path: PathBuf::from("/").join(DISPLAY_RULE_PATH), content: DISPLAY_RULE.into(), mode: 0o644 }];
    // A "change" event re-applies the rule to a monitor already there.
    for (_, dir) in &displays {
        steps.push(Step::WriteSysfs { path: PathBuf::from("/").join(dir.strip_prefix(&roots.fs).unwrap_or(dir)).join("uevent"), value: "change".into() });
    }
    let names = displays.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>().join(", ");
    r.findings.push(Finding {
        id: "rule:displays".into(),
        severity: if displays.is_empty() { Severity::Advice } else { Severity::Problem },
        title: if installed.is_some() { "Olo's display rule is out of date".into() } else { "Olo's display rule isn't installed".into() },
        detail: format!(
            "It lets the person at the seat set the brightness of monitors that take it over USB (Apple Studio Display, Pro Display XDR, LG UltraFine){}. The fix installs /etc/udev/rules.d/70-olo-displays.rules.",
            if displays.is_empty() { "; none is connected now".to_owned() } else { format!(": {names}") }
        ),
        fix: Some(Fix { label: "Install the Rule".into(), root: true, steps }),
    });
}

/// The Wi-Fi interfaces, and their drivers.
fn wifi_chips(fs: &Path) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(fs.join("sys/class/net")).into_iter().flatten().flatten() {
        let dir = e.path();
        if !dir.join("phy80211").exists() && !dir.join("wireless").exists() {
            continue;
        }
        let driver = std::fs::read_link(dir.join("device/driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_default();
        v.push((e.file_name().to_string_lossy().into_owned(), driver));
    }
    v.sort();
    v
}

/// What the healer built into itself to say which source it is
/// (apps/wifi-heal/build.rs): a fingerprint, 16 hex digits, after this.
const HEAL_SOURCE_MARK: &[u8] = b"olo-wifi-heal source ";

/// The healer at `path`'s source fingerprint, if it says.
fn heal_source(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let at = memchr::memmem::find(&bytes, HEAL_SOURCE_MARK)? + HEAL_SOURCE_MARK.len();
    let digits = bytes.get(at..at + 16)?;
    (digits.iter().all(u8::is_ascii_hexdigit) && bytes.get(at + 16) == Some(&b';')).then(|| String::from_utf8_lossy(digits).into_owned())
}

/// The healer installed is the one built: the same source (its bytes
/// differ with what was built beside it, the healer the same), or, for
/// a build from before it said, the same bytes.
fn same_healer(built: &Path, installed: &Path) -> bool {
    match (heal_source(built), heal_source(installed)) {
        (Some(a), Some(b)) => a == b,
        (Some(_), None) => false,
        (None, _) => same_contents(built, installed),
    }
}

/// Two files hold the same bytes.
fn same_contents(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    ma.len() == mb.len() && std::fs::read(a).ok() == std::fs::read(b).ok()
}

/// The Bluetooth controllers here, by name ("hci0").
fn bluetooth_controllers(fs: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(fs.join("sys/class/bluetooth"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("hci") && !n.contains(':'))
        .collect();
    v.sort();
    v
}

/// A Bluetooth controller that doesn't answer: nothing connects (AirPods
/// sit at Connecting…, Ben, 2026-11-09) until its hardware starts again.
/// The fix does what Olo's healer does by itself once it's installed.
fn bluetooth(roots: &Roots, r: &mut Report) {
    for hci in &roots.bluetooth_stuck {
        let base = roots.fs.join("sys/class/bluetooth").join(hci).join("device");
        let (Ok(device), Ok(driver)) = (std::fs::canonicalize(&base), std::fs::canonicalize(base.join("driver"))) else { continue };
        let name = driver.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let healer = (roots.units)("olo-wifi-heal.service").is_ok_and(|s| s.active);
        r.findings.push(Finding {
            id: format!("bluetooth-stuck-{hci}"),
            severity: Severity::Problem,
            title: "Bluetooth's hardware isn't answering".into(),
            detail: format!(
                "Its controller ({hci}, {name}) fails everything it's asked, so nothing connects: AirPods and other devices stay at Connecting…. Turning Bluetooth off and on doesn't reach it; starting its hardware again does (its driver lets go of it and takes it up again, the device reset where it can be). {}",
                if healer { "Olo's healer does this by itself; this one it couldn't, or not yet." } else { "Olo's healer, once installed, does it by itself the moment it happens." }
            ),
            fix: Some(Fix { label: "Restart Bluetooth".into(), root: true, steps: vec![Step::RestartBluetooth { device, driver }] }),
        });
    }
}

/// Olo's Wi-Fi and Bluetooth healer, on a machine with Bluetooth or a
/// Wi-Fi chip whose firmware can stop joining.
fn wifi(roots: &Roots, r: &mut Report) {
    let chip = wifi_chips(&roots.fs).into_iter().find(|(_, d)| HEALED_DRIVERS.contains(&d.as_str()));
    let bluetooth = !bluetooth_controllers(&roots.fs).is_empty();
    if chip.is_none() && !bluetooth {
        return;
    }
    let source = roots.bin.join("olo-wifi-heal");
    if !source.exists() {
        // Not installed (a build tree): nothing to install it from.
        return;
    }
    // (systemd not answering, busy: nothing said, not "stopped": what
    // isn't known isn't a problem, and it flickered in the tray.)
    let Ok(state) = (roots.units)("olo-wifi-heal.service") else { return };
    let installed = roots.fs.join(HEAL_BIN_PATH);
    let current = same_healer(&source, &installed)
        && read(&roots.fs.join(HEAL_UNIT_PATH)).as_deref() == Some(HEAL_UNIT.trim())
        && read(&roots.fs.join(BRCMFMAC_CONF_PATH)).as_deref() == Some(BRCMFMAC_CONF.trim());
    if current && state.active && state.enabled {
        let watches = match &chip {
            Some((iface, _)) if bluetooth => format!("{iface} and Bluetooth"),
            Some((iface, _)) => iface.clone(),
            None => "Bluetooth".into(),
        };
        r.fine.push(Fine {
            title: format!("Olo's healer watches {watches}"),
            detail: "A Wi-Fi chip whose firmware stops joining, or a Bluetooth controller that stops answering, is started again by itself.".into(),
        });
        return;
    }
    let (title, label) = if !installed.exists() {
        ("Olo's Wi-Fi and Bluetooth healer isn't installed", "Install the Healer")
    } else if !current {
        ("Olo's Wi-Fi and Bluetooth healer is out of date", "Update the Healer")
    } else {
        ("Olo's Wi-Fi and Bluetooth healer isn't running", "Start the Healer")
    };
    let unit = "olo-wifi-heal.service".to_owned();
    let steps = vec![
        Step::CopyFile { from: source, to: PathBuf::from("/").join(HEAL_BIN_PATH), mode: 0o755 },
        Step::WriteFile { path: PathBuf::from("/").join(HEAL_UNIT_PATH), content: HEAL_UNIT.into(), mode: 0o644 },
        Step::WriteFile { path: PathBuf::from("/").join(BRCMFMAC_CONF_PATH), content: BRCMFMAC_CONF.into(), mode: 0o644 },
        Step::Unit { name: unit.clone(), action: UnitAction::Reload },
        Step::Unit { name: unit.clone(), action: UnitAction::Enable },
        Step::Unit { name: unit, action: UnitAction::Restart },
        Step::NetworkManagerReload,
    ];
    r.findings.push(Finding {
        id: "wifi-heal".into(),
        severity: Severity::Problem,
        title: title.into(),
        detail: {
            let mut d = String::new();
            if let Some((iface, driver)) = &chip {
                d.push_str(&format!("This Mac's Wi-Fi chip ({driver}, {iface}) can get stuck in its firmware after failed joins: every join then fails, as if the password were wrong, until the chip is started again. "));
            }
            if bluetooth {
                d.push_str("A Bluetooth controller can stop answering (its driver failing everything it's asked), and nothing connects until its hardware starts again. ");
            }
            d.push_str(&format!(
                "Olo's healer, a small system service, notices, starts it again (and joins the network again) and tells you. The fix installs it as a program only the administrator can change ({}){}.",
                Path::new("/").join(HEAL_BIN_PATH).display(),
                if chip.is_some() { ", and has NetworkManager keep the chip's own address while it scans" } else { "" }
            ));
            d
        },
        fix: Some(Fix { label: label.into(), root: true, steps }),
    });
}

fn rtkit(roots: &Roots, r: &mut Report) {
    let unit = "rtkit-daemon.service";
    match (roots.units)(unit) {
        // (Not answering, busy: nothing said this look; it said "systemd
        // couldn't be asked" and was gone at the next.)
        Err(e) => tracing::info!("doctor: systemd didn't say how rtkit is: {e}"),
        Ok(s) if !s.loaded => r.findings.push(Finding {
            id: "rtkit".into(),
            severity: Severity::Advice,
            title: "rtkit isn't installed".into(),
            detail: "The Performance supervisor raises app_server, the session and Deskbar above other programs through rtkit (the package is rtkit on Fedora, rtkit on Debian and Ubuntu). Without it they run at the normal priority.".into(),
            fix: None,
        }),
        Ok(s) if !s.active => {
            let mut steps = vec![Step::Unit { name: unit.into(), action: UnitAction::Start }];
            if !s.enabled {
                steps.push(Step::Unit { name: unit.into(), action: UnitAction::Enable });
            }
            r.findings.push(Finding {
                id: "rtkit".into(),
                severity: Severity::Problem,
                title: "rtkit isn't running".into(),
                detail: "The Performance supervisor raises app_server, the session and Deskbar above other programs through rtkit; while it's down they run at the normal priority. The fix starts it and has it start at boot.".into(),
                fix: Some(Fix { label: "Start rtkit".into(), root: true, steps }),
            });
        }
        Ok(_) => r.fine.push(Fine { title: "rtkit is running".into(), detail: "The Performance supervisor can raise Olo's threads.".into() }),
    }
}

/// Where Olo Doctor's Allow Realtime gives the account its leave to make
/// threads realtime (the login's limits, pam_limits).
pub const REALTIME_LIMITS_CONF: &str = "etc/security/limits.d/80-olo-realtime.conf";

/// Where an account's hard limit on open files is raised, when it's
/// under what Olo needs: the login's (PAM) and systemd's own, for the
/// system's services and each account's.
pub const OPEN_FILES_LIMITS_CONF: &str = "etc/security/limits.d/80-olo-open-files.conf";
pub const OPEN_FILES_SYSTEMD_CONFS: [&str; 2] = ["etc/systemd/system.conf.d/80-olo-open-files.conf", "etc/systemd/user.conf.d/80-olo-open-files.conf"];
/// The hard limit given (what systemd and Fedora give: a program asks
/// for it, none starts with it).
pub const OPEN_FILES_HARD: u64 = 524_288;

fn open_files_limits_conf(uid: u32) -> String {
    format!(
        "# Olo: account {uid}'s programs may keep this many files open, when they ask (Olo's do as they start: a window\n# server's every window, buffer and connection is one).\n{uid}:{uid} hard nofile {OPEN_FILES_HARD}\n"
    )
}

fn open_files_systemd_conf() -> String {
    format!("# Olo: programs may keep this many files open, when they ask (they start with 1024).\n[Manager]\nDefaultLimitNOFILE=1024:{OPEN_FILES_HARD}\n")
}

/// The session's programs know the system's language (Ben, 2026-11-18:
/// "maybe we should have olo doctor offer to set the LANG properly?").
/// A session from Olo's own login had no `LANG` at all (a display
/// manager gives it; Olo's didn't), so its programs ran in the "C"
/// locale: C's sorting and dates, and ASCII for programs that take
/// their text's encoding from it. The session sets it itself now
/// (`olo_xdg::locale`, from `/etc/locale.conf` and the person's
/// `~/.config/locale.conf`).
///
/// What's judged is what a program started now would get, two ways:
/// from Deskbar, which starts the person's programs (what Deskbar was
/// started with, and where that's nothing, what it gives each as it
/// starts it, `olo_xdg::locale::give`: a Deskbar of this build does, one
/// from before an upgrade, replaced on disk, doesn't); and on demand
/// (D-Bus activation, systemd's user services: the user manager's
/// environment). Each fix changes one of the two, so it is gone once
/// fixed: `/etc/locale.conf` written where the computer has no language
/// (US English where its locale is installed, else `C.UTF-8`: UTF-8
/// text at least), which Deskbar gives from then on; and the session's
/// environment set (`Step::SetSessionEnvironment`). (Ben, the same day:
/// "even after i press fix the language one isn't going away". It was
/// judged by Deskbar's own environment, which no fix can change: a
/// running program keeps what it was started with.)
fn language(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let fs = &roots.fs;
    let system_text = read(&fs.join(olo_xdg::locale::SYSTEM_CONF)).unwrap_or_default();
    let own_text = Some(&roots.temp.config).filter(|c| c.is_absolute()).and_then(|c| read(&c.join("locale.conf"))).unwrap_or_default();
    let system = olo_xdg::locale::read(&system_text);
    let set = system.iter().find(|(name, _)| *name == "LANG").map(|(_, v)| v.clone());
    // The program that starts the person's programs here.
    let Some(deskbar) = procs.iter().find(|p| p.uid == roots.uid && p.name == "olo-deskbar" && p.scope == roots.scope && roots.scope.is_some()) else { return };
    let Ok(environ) = std::fs::read(fs.join(format!("proc/{}/environ", deskbar.pid))) else { return };
    let known = |name: &str| {
        environ.split(|b| *b == 0).find_map(|kv| kv.strip_prefix(name.as_bytes()).and_then(|rest| rest.strip_prefix(b"=")).filter(|v| !v.is_empty()).map(|v| String::from_utf8_lossy(v).into_owned()))
    };
    // What it starts gets what it has; and where it has none, what it
    // gives as it starts them (not a Deskbar from before an upgrade).
    let replaced = deskbar.exe.as_deref().is_some_and(|e| e.ends_with(" (deleted)"));
    let given = if replaced { Vec::new() } else { olo_xdg::locale::missing(&system_text, &own_text, |name| known(name).is_some()) };
    let started = known("LC_ALL").or_else(|| known("LANG")).or_else(|| given.iter().find(|(name, _)| *name == "LANG").map(|(_, v)| v.clone()));
    // What's started on demand: the user manager's (not known: not judged).
    let on_demand = roots.temp.session_env.as_ref().map(|env| env.iter().any(|(k, v)| (k == "LC_ALL" || k == "LANG") && !v.is_empty()));
    let what_c_does = "they sort, write dates and numbers as the bare \"C\" locale does, and those that take their text's encoding from it take ASCII";
    let to_session = |lang: &str| -> Vec<(String, String)> {
        let mut vars: Vec<(String, String)> = olo_xdg::locale::missing(&system_text, &own_text, |_| false).into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        if !vars.iter().any(|(k, _)| k == "LANG") {
            vars.insert(0, ("LANG".to_owned(), lang.to_owned()));
        }
        vars
    };
    let (title, detail, fix) = match (&started, set, on_demand) {
        (Some(which), _, Some(true) | None) => {
            r.fine.push(Fine { title: "Programs know their language".into(), detail: format!("They start with {which}.") });
            return;
        }
        // The person's programs have it; what's started on demand doesn't.
        (Some(which), _, Some(false)) => (
            "Programs started on demand don't know their language",
            format!(
                "The programs you open here start with {which}, but what the session starts on demand (services other programs ask for) has no LANG: {what_c_does}. The fix gives it to them from now on, until you log out; from the next login the session sets it itself."
            ),
            Some(Fix { label: "Set the Language".into(), root: false, steps: vec![Step::SetSessionEnvironment { vars: to_session(which) }] }),
        ),
        // The computer has a language; this Deskbar, from before an
        // upgrade, hands none down. Nothing to press: the upgrade does it.
        (None, Some(lang), _) => (
            "Programs here don't know their language",
            format!(
                "Programs started in this session have no LANG: {what_c_does}. This computer's is set ({lang}, /etc/locale.conf), but this session is from before Olo took it, and the Deskbar running is from before an upgrade: upgrade Olo from Deskbar's tray (or log out and in), and what's started from then on has it."
            ),
            None,
        ),
        // No language set on this computer at all.
        (None, None, _) => {
            // US English where the computer has it; UTF-8 at the least.
            let installed = ["usr/lib/locale/en_US.utf8", "usr/lib/locale/en_US.UTF-8"].iter().any(|p| fs.join(p).exists())
                || std::fs::read(fs.join("usr/lib/locale/locale-archive")).is_ok_and(|a| a.windows(10).any(|w| w == b"en_US.utf8"));
            let lang = if installed { "en_US.UTF-8" } else { "C.UTF-8" }.to_owned();
            let content = format!("# Olo Doctor: this computer's language and formats (locale.conf(5)).\nLANG={lang}\n");
            let steps = vec![
                Step::WriteFile { path: PathBuf::from("/").join(olo_xdg::locale::SYSTEM_CONF), content, mode: 0o644 },
                Step::SetSessionEnvironment { vars: vec![("LANG".to_owned(), lang.clone())] },
            ];
            let after = if replaced { " The Deskbar running is from before an upgrade: what it starts has it once Olo is upgraded from its tray." } else { "" };
            (
                "Programs here don't know their language",
                format!(
                    "Programs started in this session have no LANG: {what_c_does}. This computer has none set (no LANG in /etc/locale.conf). The fix sets {lang} for the computer: what you open from now on has it, and so does what's started on demand.{after}"
                ),
                Some(Fix { label: "Set the Language".into(), root: true, steps }),
            )
        }
    };
    r.findings.push(Finding { id: "language".into(), severity: Severity::Advice, title: title.into(), detail, fix });
}

/// A program's limits on open files (soft, hard) and how many it has
/// open (None: not ours to count), from `/proc/PID`.
fn open_files_of(fs: &Path, pid: i32) -> Option<(u64, u64, Option<usize>)> {
    let limits = read(&fs.join(format!("proc/{pid}/limits")))?;
    let line = limits.lines().find(|l| l.starts_with("Max open files"))?;
    let mut words = line["Max open files".len()..].split_whitespace();
    let number = |w: Option<&str>| match w? {
        "unlimited" => Some(u64::MAX),
        n => n.parse::<u64>().ok(),
    };
    let (soft, hard) = (number(words.next())?, number(words.next())?);
    let open = std::fs::read_dir(fs.join(format!("proc/{pid}/fd"))).ok().map(|d| d.count());
    Some((soft, hard, open))
}

/// Olo's programs may keep the files they need open (Ben, 2026-11-18:
/// "why is there a 1024 soft limit? the file limits should be huuuuge or
/// even unlimited … if that's something olo doctor needs to be able to
/// fix for me then add that to olo doctor"). A program starts with the
/// login's soft limit, 1,024, and may raise it to the hard one
/// (524,288 on Fedora); every Olo program does as it starts
/// (`olo_crash::raise_open_files`), since app_server ran out with
/// three displays' windows (696 open of 1,024, and Deskbar's folding
/// making every window draw anew: Terminal and Tracker lost their
/// windows). This finds those that haven't: started by a build from
/// before, or on a system whose hard limit is itself low. The fix
/// raises the running ones in place (`Step::LetOloOpenFiles`, no
/// password where the hard limit allows it), and, for a low hard
/// limit, writes the login's and systemd's limits for the next login
/// and raises the running programs' as root.
fn open_files(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let fs = &roots.fs;
    let enough = olo_crash::OPEN_FILES_ENOUGH;
    let ours: Vec<(&Proc, (u64, u64, Option<usize>))> = procs.iter().filter(|p| p.uid == roots.uid && p.name.starts_with("olo-")).filter_map(|p| Some((p, open_files_of(fs, p.pid)?))).collect();
    let low: Vec<&(&Proc, (u64, u64, Option<usize>))> = ours.iter().filter(|(_, (soft, _, _))| *soft < enough).collect();
    if low.is_empty() {
        if let Some((_, (soft, _, _))) = ours.iter().find(|(p, _)| p.name == "olo-app-server") {
            r.fine.push(Fine {
                title: "Olo's programs may keep the files they need open".into(),
                detail: format!("The window server may keep {} open, and has {}.", if *soft == u64::MAX { "any number".to_owned() } else { thousands(*soft) }, open_now(&ours)),
            });
        }
        return;
    }
    // The hard limit: what a program may raise itself to.
    let hard_low = low.iter().any(|(_, (_, hard, _))| *hard < enough);
    // The one that matters most, and how near it is: the window server
    // (this session's first), else whichever is nearest its limit.
    let server = low.iter().filter(|(p, _)| p.name == "olo-app-server").max_by_key(|(p, (_, _, open))| (p.scope == roots.scope && roots.scope.is_some(), open.unwrap_or(0)));
    let nearest = server.or_else(|| low.iter().max_by_key(|(_, (soft, _, open))| open.unwrap_or(0) as u64 * 1000 / (*soft).max(1)));
    let Some((who, (soft, _, open))) = nearest.map(|x| **x) else { return };
    let name = if who.name == "olo-app-server" { "Olo's window server".to_owned() } else { who.name.clone() };
    let near = open.is_some_and(|n| n as u64 * 2 >= soft);
    let mut steps = Vec::new();
    if hard_low {
        steps.push(Step::WriteFile { path: PathBuf::from("/").join(OPEN_FILES_LIMITS_CONF), content: open_files_limits_conf(roots.uid), mode: 0o644 });
        steps.extend(OPEN_FILES_SYSTEMD_CONFS.iter().map(|p| Step::WriteFile { path: PathBuf::from("/").join(p), content: open_files_systemd_conf(), mode: 0o644 }));
    }
    steps.push(Step::LetOloOpenFiles { uid: roots.uid, hard: OPEN_FILES_HARD });
    let has = open.map_or(String::new(), |n| format!(" It has {} open now.", thousands(n as u64)));
    let others = low.len() - 1;
    let more = match others {
        0 => String::new(),
        1 => " One other Olo program is held the same.".to_owned(),
        n => format!(" {n} other Olo programs are held the same."),
    };
    r.findings.push(Finding {
        id: "open-files".into(),
        severity: if near { Severity::Problem } else { Severity::Advice },
        title: format!("{name} may keep only {} files open", thousands(soft)),
        detail: format!(
            "Every window's pictures, every program's connection and every buffer shared with the screen is an open file to the window server, and a terminal's shells and a folder's files are to their programs.{has} Past the limit, programs lose their windows (what they send is dropped on the way), or can't open what they're asked to.{more} Olo's programs raise this limit themselves as they start, from the build that does; the fix raises it now for those already running{}.",
            if hard_low { ", and for the account from its next login (this system's own limit is low)" } else { "" }
        ),
        fix: Some(Fix { label: "Raise the Limit".into(), root: hard_low, steps }),
    });
}

/// Where the kernel keeps how many inotify instances one account may
/// hold, Olo's setting of it, and enough.
const INOTIFY_INSTANCES_PATH: &str = "proc/sys/fs/inotify/max_user_instances";
pub const INOTIFY_SYSCTL_PATH: &str = "etc/sysctl.d/90-olo-inotify.conf";
pub const INOTIFY_SYSCTL: &str = "# Olo: room for every program that follows files as they change (Olo Doctor, Raise the Limit;\n# the kernel's default, 128, ran out with a few agent screens open).\nfs.inotify.max_user_instances = 8192\n";
const INOTIFY_ENOUGH: u64 = 8192;

/// How many inotify instances process `pid` holds (its open files that
/// are one).
fn inotify_of(fs: &Path, pid: i32) -> u64 {
    let Ok(rd) = std::fs::read_dir(fs.join(format!("proc/{pid}/fd"))) else { return 0 };
    rd.flatten().filter(|e| std::fs::read_link(e.path()).is_ok_and(|t| t.as_os_str() == "anon_inode:inotify")).count() as u64
}

/// Programs may follow the files they need as they change (Ben,
/// 2026-11-18: "why am i limited to 128 instances??? shouldn't that be
/// thousands?? can olo doctor fix that for us?"). Every program that
/// follows files holds one or more of the account's inotify instances
/// (an Olo program one, for all its watches: `olo_kit::watch`), which
/// the kernel caps at 128 unless told otherwise, and Fedora doesn't;
/// each agent screen runs its own Deskbar, Tracker, Notifications,
/// Weather and portal, and each Chrome two, so with ten screens open
/// 125 were in use, and a program starting then couldn't follow its
/// files. The fix keeps 8,192 in /etc/sysctl.d and sets it at once
/// (an instance costs a little kernel memory, and only while it's
/// used). The programs' open files are read only while it's low.
fn inotify(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let fs = &roots.fs;
    let Some(limit) = read(&fs.join(INOTIFY_INSTANCES_PATH)).and_then(|v| v.trim().parse::<u64>().ok()) else { return };
    if limit >= INOTIFY_ENOUGH {
        r.fine.push(Fine { title: "Programs may follow the files they need".into(), detail: format!("Up to {} programs at once may follow files as they change.", thousands(limit)) });
        return;
    }
    let used: u64 = procs.iter().filter(|p| p.uid == roots.uid).map(|p| inotify_of(fs, p.pid)).sum();
    let near = used * 4 >= limit * 3;
    r.findings.push(Finding {
        id: "inotify-instances".into(),
        severity: if near { Severity::Problem } else { Severity::Advice },
        title: format!("Only {} programs may follow files at once", thousands(limit)),
        detail: format!(
            "Every program that follows files as they change (Tracker, Preferences, Calendar, DiskBackup, Chrome, and each agent screen's own) takes one of this account's {}, and {} are taken. With none left, a program that starts can't follow its files and doesn't see them change. Raise the Limit makes it 8,192, now and at every start.",
            thousands(limit),
            thousands(used)
        ),
        fix: Some(Fix {
            label: "Raise the Limit".into(),
            root: true,
            steps: vec![
                Step::WriteFile { path: PathBuf::from("/").join(INOTIFY_SYSCTL_PATH), content: INOTIFY_SYSCTL.into(), mode: 0o644 },
                Step::WriteSysfs { path: PathBuf::from("/").join(INOTIFY_INSTANCES_PATH), value: INOTIFY_ENOUGH.to_string() },
            ],
        }),
    });
}

/// How many files the window server has open, in words.
fn open_now(ours: &[(&Proc, (u64, u64, Option<usize>))]) -> String {
    ours.iter().find(|(p, _)| p.name == "olo-app-server").and_then(|(_, (_, _, open))| *open).map_or("some".to_owned(), |n| thousands(n as u64))
}

/// 1024 → "1,024".
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The limits file for account `uid`: realtime up to Olo's most, and no
/// limit on how long a realtime thread runs (Olo's own guard ends one
/// stuck, 30 s).
fn realtime_limits_conf(uid: u32) -> String {
    let most = olo_system::realtime::MOST_PRIORITY;
    format!(
        "# Olo: account {uid}'s programs may make their threads realtime themselves (app_server's loop and input, the window\n# with the keyboard, sound, PipeWire's in Olo's programs), with no limit on how long one runs: Olo ends a thread\n# stuck 30 s itself, with a report. Without this, realtime comes only through rtkit, which ends a program whose\n# realtime thread runs 200 ms without sleeping (app_server was, 2026-11-15). Written by Olo Doctor (Allow Realtime).\n{uid}:{uid} - rtprio {most}\n{uid}:{uid} - rttime unlimited\n# ...and keep their memory in RAM: the desktop's core locks its own (app_server, the session, Deskbar, sound), so\n# no page of it is moved to swap, or dropped and read back from disk, while someone waits. Without a limit: with\n# one, what a program mapped past it would fail to map. (Olo Doctor, Keep the Core in Memory.)\n{uid}:{uid} - memlock unlimited\n"
    )
}

/// The sound server kept out of swap: its services (the person's own
/// manager's: PipeWire, its PulseAudio side, WirePlumber) may use none
/// (`MemorySwapMax=0`, a drop-in each, for every account). Not their own
/// `mem.mlock-all`: with it each read in everything it had mapped, 40 MB
/// become 260 to 310 (measured in a private sound server, 2026-11-16),
/// where this costs nothing. (On Fedora their memory is shielded from
/// being reclaimed at all besides: uresourced gives `session.slice`,
/// where they run, 250 MB that is never taken; PipeWire's group had no
/// page reclaimed in 22 hours.)
pub const SOUND_MEMORY_CONFS: &[&str] =
    &["etc/systemd/user/pipewire.service.d/50-olo-memory.conf", "etc/systemd/user/pipewire-pulse.service.d/50-olo-memory.conf", "etc/systemd/user/wireplumber.service.d/50-olo-memory.conf"];
pub const SOUND_MEMORY_CONF: &str = "# Olo: the sound server's memory is never moved to swap (Olo Doctor, Keep the Core in Memory).\n[Service]\nMemorySwapMax=0\n";

/// The desktop's core kept in memory. A page of app_server's moved to
/// swap, or dropped and read back from disk, is a wait for everything on
/// screen (it had taken 7,669 major page faults in 2 h 20 min on Ben's
/// computer, 2026-11-16: "keep all those core "responsiveness" processes
/// out of swap always … any realtime/rtkit type processes e.g.
/// libinput/pipewire/etc"). Olo's programs that are realtime for as long
/// as they run lock their own memory as they start
/// (`olo_app::realtime::lock_memory`: app_server with its input thread,
/// the session, the supervisor, Deskbar, the proxy, the voice service),
/// where the account may lock without limit. The fix gives it that (the
/// limits file Allow Realtime writes, with its memlock line; for the
/// programs running now, their limit raised in place, so what starts
/// from them has it without a new login), and keeps the sound server
/// out of swap (`SOUND_MEMORY_CONFS`; in force once the person's
/// manager has read them, which `after_fix` has it do). Olo's programs
/// lock from their next start: an upgrade or a login.
fn memory_kept(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let fs = &roots.fs;
    let limits = read(&fs.join("proc/self/limits")).unwrap_or_default();
    let Some(line) = limits.lines().find(|l| l.starts_with("Max locked memory")) else { return };
    let unlimited = line["Max locked memory".len()..].split_whitespace().nth(1) == Some("unlimited");
    let uid = roots.uid;
    let conf = std::fs::read_to_string(fs.join(REALTIME_LIMITS_CONF)).unwrap_or_default();
    let allowed = conf.lines().any(|l| l.trim() == format!("{uid}:{uid} - memlock unlimited"));
    let sound = SOUND_MEMORY_CONFS.iter().all(|p| std::fs::read_to_string(fs.join(p)).ok().as_deref() == Some(SOUND_MEMORY_CONF));
    let mut steps = Vec::new();
    if !allowed {
        steps.push(Step::WriteFile { path: PathBuf::from("/").join(REALTIME_LIMITS_CONF), content: realtime_limits_conf(uid), mode: 0o644 });
    }
    if !unlimited {
        steps.push(Step::LetSessionLockMemory { uid });
    }
    if !sound {
        steps.extend(SOUND_MEMORY_CONFS.iter().map(|p| Step::WriteFile { path: PathBuf::from("/").join(p), content: SOUND_MEMORY_CONF.into(), mode: 0o644 }));
    }
    if !steps.is_empty() {
        r.findings.push(Finding {
            id: "memory-kept".into(),
            severity: Severity::Advice,
            title: "The desktop's core can be moved out of memory".into(),
            detail: "When memory fills (a build's files are enough), the kernel may move part of any program to swap, or drop its code to read it back from disk later: app_server's own, Deskbar's, the sound server's. Each such page is a wait for whoever is typing, watching or listening. Olo's core programs keep their memory in RAM themselves where the account may lock memory without limit; a login's limit is 8 MB. The fix lets it (now, and at every login), and keeps the sound server (PipeWire, WirePlumber) out of swap. Olo's core keeps its memory from its next start: an upgrade from Deskbar's tray, or a login.".into(),
            fix: Some(Fix { label: "Keep the Core in Memory".into(), root: true, steps }),
        });
        return;
    }
    // Allowed: whether the person's app_server has locked its own yet.
    let server = procs.iter().find(|p| p.uid == uid && p.name == "olo-app-server" && p.scope == roots.scope && roots.scope.is_some());
    let locked = |pid: i32| {
        read(&fs.join(format!("proc/{pid}/status"))).and_then(|s| s.lines().find_map(|l| l.strip_prefix("VmLck:")).and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())).unwrap_or(0)
    };
    match server {
        Some(p) if locked(p.pid) == 0 => r.findings.push(Finding {
            id: "memory-kept".into(),
            severity: Severity::Advice,
            title: "The desktop's core keeps its memory from Olo's next start".into(),
            detail: "The account may lock memory without limit now, but app_server was started before (or is an Olo from before it locked its own): upgrade Olo from Deskbar's tray, or log out and in, and it, Deskbar and the session keep their memory in RAM.".into(),
            fix: None,
        }),
        _ => r.fine.push(Fine {
            title: "The desktop's core stays in memory".into(),
            detail: "app_server, the session and Deskbar keep their own memory in RAM (no page of theirs is moved to swap, or read back from disk, while someone waits), and the sound server is never swapped.".into(),
        }),
    }
}

/// Where the kernel's readiness to swap is kept, and Olo's setting of it.
const SWAPPINESS_PATH: &str = "proc/sys/vm/swappiness";
pub const MEMORY_SYSCTL_PATH: &str = "etc/sysctl.d/80-olo-memory.conf";
pub const MEMORY_SYSCTL: &str = "# Olo: when memory fills, files' cache goes before programs' own memory is moved to swap (Olo Doctor,\n# Keep Programs in Memory). Swap is still there for when memory is truly short.\nvm.swappiness = 1\n";

/// Programs' own memory moved to swap while there is file cache to drop
/// (`vm.swappiness`, 60 as the kernel has it: how readily it takes
/// programs' memory rather than cache). A build fills memory with cache;
/// at 60 the kernel moves part of what idle programs hold to swap to keep
/// more of it, and they wait for it when next used. At 1 cache goes
/// first, and swap is used when memory is truly short (Ben, 2026-11-16:
/// "could we just make it set swapiness to 1 … without losing the
/// benefits when it's really needed?"). It doesn't govern the graphics
/// driver's own shrinker (`graphics_swap`).
fn swappiness(roots: &Roots, r: &mut Report) {
    let fs = &roots.fs;
    if read(&fs.join(SWAPS_PATH)).map(|t| crate::fix::swap_areas(&t)).unwrap_or_default().is_empty() {
        return;
    }
    let Some(now) = read(&fs.join(SWAPPINESS_PATH)).and_then(|v| v.parse::<u32>().ok()) else { return };
    if now <= 1 {
        r.fine.push(Fine {
            title: "Programs stay in memory while there's cache to drop".into(),
            detail: format!("vm.swappiness is {now}: files' cache goes first, and swap is used when memory is truly short."),
        });
        return;
    }
    r.findings.push(Finding {
        id: "swappiness".into(),
        severity: Severity::Advice,
        title: "Programs' memory is moved to swap while files' cache could go".into(),
        detail: format!(
            "vm.swappiness is {now}: when memory fills with files' cache (a build, a big copy), the kernel moves part of programs' own memory to swap to keep more of the cache, and a program waits for it when it's next used. The fix sets it to 1, now and at every start: cache goes first, and swap is used when memory is truly short."
        ),
        fix: Some(Fix {
            label: "Keep Programs in Memory".into(),
            root: true,
            steps: vec![
                Step::WriteFile { path: PathBuf::from("/").join(MEMORY_SYSCTL_PATH), content: MEMORY_SYSCTL.into(), mode: 0o644 },
                Step::WriteSysfs { path: PathBuf::from("/").join(SWAPPINESS_PATH), value: "1".into() },
            ],
        }),
    });
}

/// zram-generator's own settings (Fedora's: a swap in compressed memory
/// of 8 GB at most), and the one that makes it as large as memory.
const ZRAM_DEFAULTS_PATH: &str = "usr/lib/systemd/zram-generator.conf";
pub const ZRAM_FULL_CONF: &str = "# Olo: swap in compressed memory, as large as the computer's memory (Olo Doctor's suggestion, More Swap):\n# room for what should be swapped when memory is truly short. Empty, it takes a third of a percent of its\n# size (the kernel's tables for it); beyond that, what's in it, compressed.\n# This file replaces the system's own (/usr/lib/systemd/zram-generator.conf); remove it to have that back.\n[zram0]\nzram-size = ram\n";

/// Swap as large as memory, suggested (Ben, 2026-11-16: "have olo doctor
/// suggest a swap size equal to the ram size, e.g. 64gb for this
/// machine, and have it offer to set it for the user if it isn't already
/// >= that target. that way stuff that really should be swapped out can
/// be and has plenty of swap available for it"). Only where it's the
/// generator's swap in compressed memory (its size is a line of its
/// settings; a disk's would be a partition's), and only where the
/// graphics driver can't take the room for windows' buffers
/// (`graphics_swap`: Olo's patch in, or another driver): more swap
/// without it is more of them moved out. Not free: the kernel's tables
/// for a swap area are there whether it's used or not, 222 MB for
/// 62 GiB of zram on Ben's computer (`VmallocUsed` before and after), a
/// third of a percent of its size, which the suggestion says.
fn swap_size(roots: &Roots, pcis: &[Pci], r: &mut Report) {
    let fs = &roots.fs;
    let areas = read(&fs.join(SWAPS_PATH)).map(|t| crate::fix::swap_areas(&t)).unwrap_or_default();
    let Some(memory) =
        read(&fs.join("proc/meminfo")).and_then(|m| m.lines().find_map(|l| l.strip_prefix("MemTotal:")).and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())).map(|kb| kb * 1024)
    else {
        return;
    };
    let total: u64 = areas.iter().map(|(_, size)| size).sum();
    // (As large already, to within what the kernel keeps of memory for
    // itself: 64 GB of memory reads 62.)
    if areas.is_empty() || total >= memory / 20 * 19 {
        return;
    }
    let generated = areas.len() == 1 && areas[0].0 == "/dev/zram0" && fs.join(ZRAM_DEFAULTS_PATH).is_file();
    let xe = pcis.iter().any(|p| p.display() && p.driver.as_deref() == Some("xe"));
    if !generated || (xe && !graphics_memory_patched(fs)) {
        return;
    }
    let words = |n: u64| olo_system::dnf::size_words(n).replace(' ', "\u{a0}");
    let steps = vec![
        Step::WriteFile { path: PathBuf::from("/").join(ZRAM_CONF_PATH), content: ZRAM_FULL_CONF.into(), mode: 0o644 },
        Step::Unit { name: "swap".into(), action: UnitAction::Reload },
        // (Made again at its new size: what's in it comes back into
        // memory first.)
        Step::RemakeZramSwap { device: "zram0".into() },
    ];
    r.findings.push(Finding {
        id: "swap-size".into(),
        severity: Severity::Suggestion,
        title: "More swap, for what should be swapped".into(),
        detail: format!(
            "Swap is {} in compressed memory, and the computer has {} of memory. With swap as large as memory, what truly should be moved out when memory is short has room, where now the largest program is stopped once {} are full. Empty, it takes about {} for the kernel's own tables; beyond that, only what's in it, compressed. The fix sets its size to the computer's memory, now and at every start (what's in swap comes back into memory while it's made again).",
            words(total),
            words(memory),
            words(total),
            words(memory / 290)
        ),
        fix: Some(Fix { label: "Make Swap as Large as Memory".into(), root: true, steps }),
    });
}

/// The account's leave to make its threads realtime itself (this
/// session's limits: RLIMIT_RTPRIO at least Olo's most, and no hard limit
/// on realtime time short of the guard's). Whether it has it.
fn realtime(roots: &Roots, r: &mut Report) -> bool {
    let limits = read(&roots.fs.join("proc/self/limits")).unwrap_or_default();
    let field = |name: &str| -> Vec<String> { limits.lines().find(|l| l.starts_with(name)).map(|l| l[name.len()..].split_whitespace().map(str::to_owned).collect()).unwrap_or_default() };
    let rtprio = field("Max realtime priority").first().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let time_hard = field("Max realtime timeout").get(1).cloned().unwrap_or_default();
    let long_enough = time_hard == "unlimited" || time_hard.parse::<u64>().is_ok_and(|us| us >= olo_system::realtime::HARD_LIMIT_US);
    let most = olo_system::realtime::MOST_PRIORITY;
    if rtprio >= most && long_enough {
        r.fine.push(Fine {
            title: "Olo's threads are realtime, with no time limit".into(),
            detail: "app_server's loop and input, the window with the keyboard and sound run before anything else wanting the processor, for as long as they have work: only a thread stuck for 30 s ends its program, with a report.".into(),
        });
        return true;
    }
    if roots.fs.join(REALTIME_LIMITS_CONF).exists() {
        r.findings.push(Finding {
            id: "realtime".into(),
            severity: Severity::Advice,
            title: "Realtime for Olo's threads starts at your next login".into(),
            detail: "The account may make its threads realtime now, but this session was started before: log out and in, and app_server, the window with the keyboard and sound run realtime, with no time limit.".into(),
            fix: None,
        });
        return false;
    }
    r.findings.push(Finding {
        id: "realtime".into(),
        severity: Severity::Problem,
        title: "Olo's threads can't be realtime without being ended at 200 ms".into(),
        detail: format!(
            "Under load, app_server's loop and input, the window with the keyboard and sound need to run before other programs' work. This account may not make threads realtime itself (its limit is {rtprio}), so the only way is rtkit, which ends a whole program whose realtime thread runs 200 ms without a pause, even when it's only busy (app_server was ended so, inside a graphics driver's wait). So Olo's threads stay at a raised priority, not realtime. The fix lets the account make them realtime itself, up to {most}, with no limit on how long they run: Olo ends a thread only if it's stuck for 30 s, with a report. It applies at your next login."
        ),
        fix: Some(Fix {
            label: "Allow Realtime".into(),
            root: true,
            steps: vec![Step::WriteFile { path: PathBuf::from("/").join(REALTIME_LIMITS_CONF), content: realtime_limits_conf(roots.uid), mode: 0o644 }],
        }),
    });
    false
}

/// AirPods paired: connecting and switching by themselves needs their
/// broadcasts heard, through BlueZ's advertisement monitor, which BlueZ
/// (5.87) keeps among its experimental interfaces (`bluetoothd
/// --experimental`). The fix: a drop-in that starts it so, and Bluetooth
/// restarted.
fn airpods_listening(roots: &Roots, procs: &[Proc], r: &mut Report) {
    if !roots.airpods {
        return;
    }
    let Some(bluez) = procs.iter().find(|p| p.name == "bluetoothd") else { return };
    let cmdline = std::fs::read(roots.fs.join(format!("proc/{}/cmdline", bluez.pid))).unwrap_or_default();
    let args: Vec<String> = cmdline.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    if args.iter().any(|a| a == "-E" || a == "--experimental") {
        r.fine.push(Fine { title: "Bluetooth listens for the AirPods".into(), detail: "BlueZ's advertisement monitor is on: they can connect and switch by themselves.".into() });
        return;
    }
    // Its program: the exe link is root's to read; the command line's
    // first word is anyone's.
    let program = bluez.exe.clone().map(|e| e.trim_end_matches(" (deleted)").to_owned()).or_else(|| args.first().cloned()).filter(|e| e.starts_with('/'));
    let fix = program.map(|program| Fix {
        label: "Let Bluetooth Listen".into(),
        root: true,
        steps: vec![
            Step::WriteFile { path: format!("/{BLUETOOTH_DROP_IN}").into(), content: bluetooth_drop_in(&program, drop_in_hands_free(roots)), mode: 0o644 },
            Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Reload },
            Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Restart },
        ],
    });
    r.findings.push(Finding {
        id: "airpods-listening".into(),
        severity: Severity::Advice,
        title: "Bluetooth doesn't listen for the AirPods".into(),
        detail: "Connecting automatically when you put them in, and switching between this computer and a phone, need their broadcasts heard: BlueZ's advertisement monitor, among its experimental interfaces. The fix starts Bluetooth with them; restarting it disconnects Bluetooth devices for a moment.".into(),
        fix,
    });
}

/// Olo's own way of starting Bluetooth (a drop-in).
const BLUETOOTH_DROP_IN: &str = "etc/systemd/system/bluetooth.service.d/olo.conf";
/// What says BlueFerry's backend is installed.
const PHONE_BACKEND: &[&str] = &["usr/share/dbus-1/services/io.weirdware.BlueFerry.service", "usr/bin/blueferry"];

/// Olo's drop-in for Bluetooth: BlueZ's experimental interfaces on (its
/// advertisement monitor, for AirPods' broadcasts).
fn bluetooth_drop_in(program: &str, hands_free: bool) -> String {
    if hands_free {
        return format!(
            "# Olo: BlueZ's advertisement monitor, for AirPods, and without its hostname plugin, which would say\n# this computer is a laptop over the hands-free class an iPhone wants (Olo Doctor).\n[Service]\nExecStart=\nExecStart={program} --experimental -P hostname\n"
        );
    }
    format!("# Olo: BlueZ's advertisement monitor, for AirPods (Olo Doctor).\n[Service]\nExecStart=\nExecStart={program} --experimental\n")
}

/// Olo's drop-in starts Bluetooth without its hostname plugin (the
/// hands-free class kept: `phone_class`).
fn drop_in_hands_free(roots: &Roots) -> bool {
    read(&roots.fs.join(BLUETOOTH_DROP_IN)).is_some_and(|d| d.lines().any(|l| l.trim().starts_with("ExecStart=") && (l.contains("-P hostname") || l.contains("--noplugin=hostname"))))
}

/// BlueFerry, which Olo no longer uses (it speaks to the phone itself:
/// `olo_system::phone`), where an earlier Olo Doctor installed it (or
/// the person did): its backend would hold the phone's message and
/// contact profiles through obexd, which Olo's own then can't have, and
/// its drop-in starts Bluetooth its way. Offered for removal: the
/// package (and what was installed only for it), and Olo's own Bluetooth
/// drop-in back as AirPods alone need it. What it kept (its history in
/// ~/.local/state/blueferry, its settings in ~/.config/blueferry) is the
/// person's to keep or delete; said.
fn phone(roots: &Roots, r: &mut Report) {
    if !PHONE_BACKEND.iter().any(|p| roots.fs.join(p).exists()) {
        return;
    }
    let mut steps = vec![Step::RemovePackages { names: vec!["blueferry-backend".into(), "blueferry-gtk".into(), "blueferry-qt".into(), "blueferry-quickshell".into()] }];
    // Olo's drop-in, written for BlueFerry's needs (the hands-free plugin
    // left out), back as it is without it; Bluetooth started so.
    if let Some(ours) = read(&roots.fs.join(BLUETOOTH_DROP_IN))
        && let Some(line) = ours.lines().rev().find_map(|l| l.trim().strip_prefix("ExecStart=").filter(|v| !v.trim().is_empty()))
        && let Some(program) = line.split_whitespace().next()
        && line.contains("hfp")
    {
        steps.push(Step::WriteFile { path: format!("/{BLUETOOTH_DROP_IN}").into(), content: bluetooth_drop_in(program, drop_in_hands_free(roots)), mode: 0o644 });
        steps.push(Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Reload });
        steps.push(Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Restart });
    }
    r.findings.push(Finding {
        id: "phone-blueferry".into(),
        severity: Severity::Advice,
        title: "BlueFerry is installed, and Olo no longer uses it".into(),
        detail: "Olo now speaks to your phone itself (Preferences ▸ Phone). BlueFerry’s backend would hold the phone’s messages and contacts where Olo’s own can’t reach them. The fix removes BlueFerry and what was installed only for it, and starts Bluetooth as it was before (what’s connected comes back by itself). What BlueFerry kept (its history in ~/.local/state/blueferry and its settings in ~/.config/blueferry) is left for you to keep or delete.".into(),
        fix: Some(Fix { label: "Remove BlueFerry".into(), root: true, steps }),
    });
}

/// How long (seconds) a connected phone's messages may fail to open
/// before Olo Doctor says so.
const PHONE_STALLED: u64 = 180;

/// A phone paired: its messages, contacts and codes reach Olo only
/// through the phone service (`olo-phone --service`, which the session
/// starts) and the profiles it registers with BlueZ. It registers them
/// again by itself with a BlueZ started since (an update, a fix here);
/// what it can't, it says in its state (`PhoneState::profiles`,
/// `trouble`), and this says it where the person will see it. A service
/// that isn't running (the session leaves one stopped that ended again
/// and again) is said too.
fn phone_link(roots: &Roots, r: &mut Report) {
    if !roots.phone {
        return;
    }
    let state: Option<olo_protocol::phone::PhoneState> = read(&roots.temp.runtime.join("olo-phone.json")).and_then(|t| serde_json::from_str(&t).ok());
    let running = state.as_ref().is_some_and(|s| s.pid > 0 && roots.fs.join(format!("proc/{}", s.pid)).exists());
    let Some(state) = state.filter(|_| running) else {
        r.findings.push(Finding {
            id: "phone-service".into(),
            severity: Severity::Problem,
            title: "The phone service isn’t running".into(),
            detail: "A phone is paired, but Olo’s phone service, which its messages, contacts and one-time codes come through, isn’t running: it ended again and again, and the session left it stopped. Logging out and in starts it again; Log Viewer (olo-phone) says why it ended.".into(),
            fix: None,
        });
        return;
    };
    if !state.profiles {
        let trouble = if state.trouble.is_empty() { "Bluetooth isn’t answering.".to_owned() } else { state.trouble.clone() };
        r.findings.push(Finding {
            id: "phone-profiles".into(),
            severity: Severity::Problem,
            title: "The phone’s messages can’t reach Olo".into(),
            detail: format!("Olo’s phone service couldn’t register its profiles (the phone’s messages, its contacts, and the server it tells of new messages) with Bluetooth: {trouble} It tries again every minute, and as soon as Bluetooth starts again."),
            fix: None,
        });
        return;
    }
    let name = if state.name.is_empty() { "The phone".to_owned() } else { format!("“{}”", state.name) };
    // The phone is there (BlueZ has it connected) and its messages have
    // not reached Olo for minutes. The service asks again by itself, and
    // every wait of its has an end, so this long is something it can't
    // get past: said, with why, and the service started afresh if the
    // person wants (ended here; the session starts it again).
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let waiting = matches!(state.link, olo_protocol::phone::Link::Away | olo_protocol::phone::Link::Connecting);
    if roots.phone_connected && waiting && state.since > 0 && now.saturating_sub(state.since) > PHONE_STALLED {
        let why = if state.trouble.is_empty() { String::new() } else { format!(" {}", state.trouble) };
        r.findings.push(Finding {
            id: "phone-messages".into(),
            severity: Severity::Problem,
            title: "The phone is connected, but its messages aren’t reaching Olo".into(),
            detail: format!(
                "{name} is connected over Bluetooth, and for {} minutes Olo’s phone service hasn’t been able to open its messages, though it keeps asking.{why} The fix starts the phone service afresh. If it’s the same after: on the phone, turn its switches for this computer off and on (its Bluetooth settings), or pair again from Preferences ▸ Phone.",
                now.saturating_sub(state.since) / 60
            ),
            fix: Some(Fix { label: "Start the Phone Service Again".into(), root: false, steps: vec![Step::Signal { pid: state.pid as i32, signal: libc::SIGTERM }] }),
        });
        return;
    }
    r.fine.push(Fine {
        title: "The phone’s messages can reach Olo".into(),
        detail: format!("Olo’s phone service is running, its profiles registered with Bluetooth. {name}: {}.", state.link.words().trim_end_matches('.')),
    });
}

/// BlueZ's settings file.
const BLUEZ_CONF: &str = "etc/bluetooth/main.conf";
/// The class this computer presents for an iPhone: Audio/Video (major
/// 4), Hands-Free (minor 2, the byte 0x08); BlueZ adds the service bits.
const HANDS_FREE: u32 = 0x000408;

/// BlueZ's settings with `key` set to `value` in [General]: the line
/// there replaced, else put after its commented example, else last in
/// the section (a section made if there's none). The rest as it was.
fn with_bluez(conf: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let is = |t: &str| t.split_once('=').is_some_and(|(k, _)| k.trim() == key);
    let mut out: Vec<String> = Vec::new();
    let (mut general, mut done, mut seen_general) = (false, false, false);
    for l in conf.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            if general && !done {
                out.push(line.clone());
                done = true;
            }
            general = t == "[General]";
            seen_general |= general;
            out.push(l.to_owned());
            continue;
        }
        if general && !done && is(t) {
            out.push(line.clone());
            done = true;
            continue;
        }
        out.push(l.to_owned());
        if general && !done && t.starts_with('#') && is(t.trim_start_matches('#')) {
            out.push(line.clone());
            done = true;
        }
    }
    if general && !done {
        out.push(line.clone());
        done = true;
    }
    if !seen_general && !done {
        out.insert(0, "[General]".into());
        out.insert(1, line);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// A [General] setting of BlueZ's (commented out: none).
fn bluez_setting(conf: &str, key: &str) -> Option<String> {
    let mut general = false;
    for line in conf.lines().map(str::trim) {
        if line.starts_with('[') {
            general = line == "[General]";
        } else if general
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim().to_owned());
        }
    }
    None
}

/// The `Class` set in BlueZ's settings.
fn bluez_class(conf: &str) -> Option<u32> {
    let v = bluez_setting(conf, "Class")?;
    u32::from_str_radix(v.trim_start_matches("0x").trim_start_matches("0X"), 16).ok()
}

/// This computer's name, as BlueZ's hostname plugin names the adapter:
/// the pretty hostname, else the static, else the transient.
fn host_name(roots: &Roots) -> Option<String> {
    let pretty = read(&roots.fs.join("etc/machine-info")).and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_HOSTNAME=").map(|v| v.trim().trim_matches('"').trim_matches('\'').to_owned())));
    [pretty, read(&roots.fs.join("etc/hostname")), read(&roots.fs.join("proc/sys/kernel/hostname"))].into_iter().flatten().map(|n| n.trim().to_owned()).find(|n| !n.is_empty())
}

/// An iPhone offers its messages and contacts to a computer (the
/// switches under it in its Bluetooth settings) by what the computer said
/// it was as they paired, and BlueFerry's notes found them offered to one
/// that says it's a hands-free device (Ben, 2026-11-17: "have it identify
/// as blueferry suggests"). BlueZ says so at every start from its own
/// settings (`Class`), so nothing has to put it back; the fix writes it
/// there and starts Bluetooth again. Said wherever there's Bluetooth,
/// since it must be so *before* an iPhone pairs: a problem with one
/// paired, a suggestion otherwise. (At first only with an iPhone paired:
/// Ben, who had unpaired to pair again, "olo doctor didn't say anything
/// about it".)
fn phone_class(roots: &Roots, procs: &[Proc], r: &mut Report) {
    if !roots.iphone && bluetooth_controllers(&roots.fs).is_empty() {
        return;
    }
    // (Whole: written back with only what's needed changed.)
    let conf = std::fs::read_to_string(roots.fs.join(BLUEZ_CONF)).unwrap_or_default();
    let set = bluez_class(&conf).is_some_and(|c| c & 0x1ffc == HANDS_FREE);
    // What the adapter says now is what counts (only the major and
    // minor classes: BlueZ takes no more); without it, the settings.
    let says = roots.bluetooth_class.map(|c| c & 0x1ffc == HANDS_FREE);
    if says.unwrap_or(set && drop_in_hands_free(roots)) {
        r.fine.push(Fine { title: "Bluetooth presents this computer as an iPhone expects".into(), detail: "As a hands-free device: an iPhone offers its messages and contacts to it.".into() });
        return;
    }
    let (severity, title) = if roots.iphone {
        (Severity::Problem, "Bluetooth doesn’t present this computer as an iPhone expects")
    } else {
        (Severity::Advice, "For an iPhone: have Bluetooth present this computer as one expects")
    };
    // The class, in BlueZ's settings; and, since BlueZ's hostname plugin
    // puts its own over it (a laptop's, from the chassis: Ben's, after
    // the class alone was set), Bluetooth started without that plugin,
    // and the name it gave the adapter kept in BlueZ's settings (else
    // it's "BlueZ 5.87"), unless one is set there already.
    let mut conf2 = with_bluez(&conf, "Class", &format!("0x{HANDS_FREE:06x}"));
    if bluez_setting(&conf, "Name").is_none()
        && let Some(name) = host_name(roots)
    {
        conf2 = with_bluez(&conf2, "Name", &name);
    }
    let mut steps = vec![Step::WriteFile { path: format!("/{BLUEZ_CONF}").into(), content: conf2, mode: 0o644 }];
    let bluez = procs.iter().find(|p| p.name == "bluetoothd");
    let program = bluez
        .and_then(|b| {
            let cmdline = std::fs::read(roots.fs.join(format!("proc/{}/cmdline", b.pid))).unwrap_or_default();
            b.exe.clone().map(|e| e.trim_end_matches(" (deleted)").to_owned()).or_else(|| cmdline.split(|c| *c == 0).next().map(|a| String::from_utf8_lossy(a).into_owned()))
        })
        .filter(|e| e.starts_with('/'))
        .unwrap_or_else(|| "/usr/libexec/bluetooth/bluetoothd".to_owned());
    if !drop_in_hands_free(roots) {
        steps.push(Step::WriteFile { path: format!("/{BLUETOOTH_DROP_IN}").into(), content: bluetooth_drop_in(&program, true), mode: 0o644 });
        steps.push(Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Reload });
    }
    steps.push(Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Restart });
    r.findings.push(Finding {
        id: "phone-class".into(),
        severity,
        title: title.into(),
        detail: "An iPhone offers its messages and contacts to a computer (Show Notifications and Sync Contacts, under it in the iPhone’s Bluetooth settings) by what the computer says it is as they pair, and it offers its contacts to one that says it’s a hands-free device; this one says it’s a laptop. The fix has Bluetooth say so from now on (Class in /etc/bluetooth/main.conf, and Bluetooth started without its hostname plugin, which would put a laptop’s class over it; the computer’s name kept) and starts it again. An iPhone paired before: forget this computer on the iPhone, remove the iPhone in Preferences ▸ Bluetooth, and pair again (Preferences ▸ Phone). The iPhone may then also offer this computer for calls’ sound, and other devices see it as a headset.".into(),
        fix: Some(Fix { label: "Present as Hands-Free".into(), root: true, steps }),
    });
}

/// The voice services' sound (olo-voice, olo-airpods): PipeWire's thread
/// for it ("data-loop") is made realtime through rtkit, which gives that
/// to so many processes a user (15 by default) and threads (25), and a
/// desktop's programs use them up: then a frame of someone's voice can
/// wait behind other work, a click in their call. The fix lets rtkit give
/// more, and restarts it (the supervisor asks again).
fn voice_realtime(roots: &Roots, procs: &[Proc], leave: bool, r: &mut Report) {
    let threads = audio_threads(roots, procs);
    // (Without the account's leave, Allow Realtime's finding says it.)
    if threads.is_empty() || !leave {
        return;
    }
    let mut not: Vec<String> = Vec::new();
    for t in threads.iter().filter(|t| !t.realtime) {
        if !not.contains(&t.program) {
            not.push(t.program.clone());
        }
    }
    if not.is_empty() {
        r.fine.push(Fine { title: "Voice processing's sound is realtime".into(), detail: "The voice services' sound threads are realtime.".into() });
        return;
    }
    r.findings.push(Finding {
        id: "voice-realtime".into(),
        severity: Severity::Advice,
        title: "Voice processing's sound isn't realtime".into(),
        detail: format!(
            "{} started before the account could make threads realtime itself, and their sound runs at its nice: under load, a frame of a voice can wait behind other work, a click in a call. The fix makes their sound threads realtime now, with no time limit.",
            not.join(" and ")
        ),
        fix: Some(Fix { label: "Make It Realtime".into(), root: false, steps: Vec::new() }),
    });
}

/// One of the voice services' sound threads (PipeWire's "data-loop", in
/// a program of the audio tier), the user's.
struct AudioThread {
    pid: i32,
    program: String,
    tid: u32,
    realtime: bool,
}

fn audio_threads(roots: &Roots, procs: &[Proc]) -> Vec<AudioThread> {
    let audio = |name: &str| olo_protocol::performance::tier(name) == Some(olo_protocol::performance::AUDIO_NICE);
    let mut out = Vec::new();
    for p in procs.iter().filter(|p| p.uid == roots.uid && audio(&p.name)) {
        let tasks = roots.fs.join(format!("proc/{}/task", p.pid));
        for t in std::fs::read_dir(&tasks).into_iter().flatten().flatten() {
            let Some(tid) = t.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let comm = read(&t.path().join("comm")).unwrap_or_default();
            if !comm.trim().starts_with(olo_protocol::performance::AUDIO_THREAD) {
                continue;
            }
            // The scheduling policy: field 41 of stat (0 normal, 1 FIFO, 2 RR).
            let stat = read(&t.path().join("stat")).unwrap_or_default();
            let policy = stat.rfind(')').and_then(|i| stat[i + 1..].split_whitespace().nth(38)).and_then(|f| f.parse::<u32>().ok());
            out.push(AudioThread { pid: p.pid, program: p.name.clone(), tid, realtime: matches!(policy, Some(1 | 2)) });
        }
    }
    out
}

/// What a fix of finding `id` leaves to do as the person, once its steps
/// (as root) have run; before the look that checks it. The voice
/// services' sound: rtkit, restarted with room, asked now for their
/// threads (it gives only to their own user, so not by the helper), not
/// at the supervisor's next try a minute on: else that look still found
/// them not realtime, and the finding came back after "Fixed." (Ben,
/// 2026-11-06: "i press fix it, it says it's fixed, but then it just
/// comes back").
pub fn after_fix(id: &str) -> Option<fn()> {
    match id {
        "voice-realtime" => Some(ask_realtime_again as fn()),
        // (The sound server's drop-ins, written for every account: the
        // person's own manager reads them now, and its running services
        // take the setting without starting again.)
        "memory-kept" => Some(|| {
            if let Err(e) = olo_system::systemd::user_reload() {
                tracing::warn!("the sound server's settings wait for the next login: {e}");
            }
        }),
        // (olo-secrets started again by the session, its status written
        // once it has merged: waited for, five seconds at most.)
        "secrets:two-logins" => Some(|| {
            let roots = Roots::real();
            for _ in 0..50 {
                if secrets_status(&roots).is_some_and(|v| v["serving"].as_bool() == Some(true) && login_keyrings(&v) <= 1) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }),
        _ => None,
    }
}

fn ask_realtime_again() {
    let roots = Roots::real();
    let procs = processes(&roots.fs);
    for t in audio_threads(&roots, &procs).into_iter().filter(|t| !t.realtime) {
        match olo_system::priority::realtime(t.pid as u32, t.tid, olo_protocol::performance::AUDIO_RT_PRIORITY) {
            Ok(()) => tracing::info!("made {}'s sound (thread {}) realtime", t.program, t.tid),
            Err(e) => tracing::warn!("{}'s sound (thread {}) still not realtime: {e}", t.program, t.tid),
        }
    }
}

/// app_server's and every window's thread ask to be taken as busy while
/// they run, for a fast core at a high clock (`CORE_UTIL_MIN`,
/// `UI_UTIL_MIN`); the kernel caps what anyone may ask at
/// `sched_util_clamp_min`.
fn util_clamp(roots: &Roots, r: &mut Report) {
    let Some(cap) = read(&roots.fs.join(UTIL_CLAMP_PATH)) else {
        r.findings.push(Finding {
            id: "uclamp".into(),
            severity: Severity::Advice,
            title: "This kernel can't keep frames on the fast cores".into(),
            detail: "It's built without utilization clamping (CONFIG_UCLAMP_TASK): on a computer of fast and slow cores, a frame after a pause may run on a slow core at a low clock, three or four times as long.".into(),
            fix: None,
        });
        return;
    };
    match cap.trim().parse::<u32>() {
        Ok(cap) if cap < 1024 => r.findings.push(Finding {
            id: "uclamp".into(),
            severity: Severity::Problem,
            title: "Frames' threads can't ask for the fast cores".into(),
            detail: format!(
                "kernel.sched_util_clamp_min is {cap} (of 1024): app_server and every window ask the scheduler for a fast core at a high clock while they draw, and get no more than that, so a frame after a pause can run on a slow core at a low clock, three or four times as long. The fix sets it to 1024, now and at every boot."
            ),
            fix: Some(Fix {
                label: "Allow Fast Cores".into(),
                root: true,
                steps: vec![
                    Step::WriteFile { path: PathBuf::from("/").join(UTIL_CLAMP_CONF_PATH), content: UTIL_CLAMP_CONF.into(), mode: 0o644 },
                    Step::WriteSysfs { path: PathBuf::from("/").join(UTIL_CLAMP_PATH), value: "1024".into() },
                ],
            }),
        }),
        _ => r.fine.push(Fine { title: "Frames run on the fast cores".into(), detail: "app_server and every window can ask the scheduler for a fast core at a high clock while they draw.".into() }),
    }
}

/// Where every login session's scope gets its weight (Olo Doctor's).
pub const SESSION_WEIGHT_CONF: &str = "etc/systemd/system/session-.scope.d/50-olo-weight.conf";

/// The drop-in giving every login session's scope `weight`.
fn session_weight_conf(weight: u64) -> String {
    format!(
        "# Olo: a login session (where Olo runs: app_server, every app, the terminal's jobs) weighs as much as the account's own manager,\n# which uresourced raises for whoever is at the computer (it's where GNOME's shell runs, and Olo's doesn't).\n[Scope]\nCPUWeight={weight}\nIOWeight={weight}\n"
    )
}

/// The login session's weight beside the person's own manager's. The
/// processor is shared out between sibling cgroups by weight before any
/// nice counts: Fedora's uresourced raises the active user's manager
/// (`user@UID.service`, where GNOME's shell runs) to 500, and Olo runs in
/// logind's scope for the session, at 100. Whatever runs as a service of
/// the person's (a browser's scope, a build service, a cache daemon)
/// then had five times the share of the whole desktop under load, and no
/// nice could reach it. The fix gives every session's scope the
/// manager's weight, from now (this session's cgroup) and for every
/// later login (a drop-in for `session-*.scope`).
fn session_weight(roots: &Roots, r: &mut Report) {
    let Some(scope) = roots.scope.as_deref() else { return };
    let slice = roots.fs.join(format!("sys/fs/cgroup/user.slice/user-{}.slice", roots.uid));
    let weight = |unit: &str, file: &str| read(&slice.join(unit).join(file)).and_then(|t| t.trim().parse::<u64>().ok());
    let (Some(ours), Some(theirs)) = (weight(scope, "cpu.weight"), weight(&format!("user@{}.service", roots.uid), "cpu.weight")) else { return };
    if ours >= theirs {
        r.fine.push(Fine {
            title: "The desktop's share of the processor is the account's services'".into(),
            detail: format!("Olo's session weighs {ours} beside your own services' {theirs}: under load, a browser or a build run as a service can't take more of the processor than the desktop has."),
        });
        return;
    }
    let mut steps = vec![Step::WriteFile { path: PathBuf::from("/").join(SESSION_WEIGHT_CONF), content: session_weight_conf(theirs), mode: 0o644 }];
    for file in ["cpu.weight", "io.weight"] {
        if roots.fs.join(format!("sys/fs/cgroup/user.slice/user-{}.slice/{scope}/{file}", roots.uid)).exists() {
            steps.push(Step::WriteSysfs {
                path: PathBuf::from(format!("/sys/fs/cgroup/user.slice/user-{}.slice/{scope}/{file}", roots.uid)),
                value: if file == "io.weight" { format!("default {theirs}") } else { theirs.to_string() },
            });
        }
    }
    r.findings.push(Finding {
        id: "session-weight".into(),
        severity: Severity::Problem,
        title: "Your own services get five times the desktop's share of the processor".into(),
        detail: format!(
            "The processor is shared between groups of programs before any program's priority counts. uresourced gives your account's services (a browser started from the desktop, a build or cache service) a weight of {theirs}, made for desktops whose shell runs among them; Olo's session, where app_server, every app and the terminal's jobs run, has {ours}. Under load the desktop then gets {:.0}% of what those services get, and lowering a build's priority doesn't help. The fix gives login sessions the same weight, now and at every login.",
            ours as f64 / theirs as f64 * 100.0
        ),
        fix: Some(Fix { label: "Give the Desktop Its Share".into(), root: true, steps }),
    });
}

/// The login session divided into the desktop and background work
/// (`olo_system::session_groups`): Olo's login makes the two groups in a
/// session's scope only where systemd has delegated the scope, which a
/// drop-in for every login session says. The fix writes it; it is in
/// force for sessions started after it (and Olo's login must be this
/// version's, which its own finding sees to).
fn session_groups(roots: &Roots, r: &mut Report) {
    use olo_system::session_groups::{DELEGATE_CONF, DELEGATE_CONF_TEXT};
    let Some(scope) = roots.scope.as_deref() else { return };
    // Divided: the two groups are there and the session's programs are
    // in the desktop's (the groups alone, with the session running
    // elsewhere in the scope, is a division that didn't take).
    let olo = roots.fs.join(format!("sys/fs/cgroup/user.slice/user-{}.slice/{scope}/olo", roots.uid));
    let divided = olo.join("bg/memory.high").exists() && read(&olo.join("desk/cgroup.procs")).is_some_and(|list| !list.trim().is_empty());
    if divided {
        r.fine.push(Fine {
            title: "The desktop and background work are kept apart".into(),
            detail: "This session is divided in two: compilers and linkers can be given only the processors and the memory the desktop isn't using (Preferences ▸ Performance, Give compilers and linkers only spare memory).".into(),
        });
        return;
    }
    if std::fs::read_to_string(roots.fs.join(DELEGATE_CONF)).is_ok_and(|text| text == DELEGATE_CONF_TEXT) {
        r.fine.push(Fine {
            title: "The session delegation setting is installed".into(),
            detail: "This login has no usable desktop and background groups. The login worker records why it skipped setup.".into(),
        });
        return;
    }
    r.findings.push(Finding {
        id: "session-groups".into(),
        severity: Severity::Advice,
        title: "Builds can't be kept apart from the desktop".into(),
        detail: "Olo's login screen can divide your session in two, the desktop and background work, so that compilers and linkers run when the desktop leaves the processors idle and are the first to give up memory when it runs short (Preferences ▸ Performance, Give compilers and linkers only spare memory). systemd leaves such a division alone only where a setting for login sessions says so. This writes that setting. It applies to sessions started afterwards: log out and in again, or restart.".into(),
        fix: Some(Fix { label: "Let Olo Divide Sessions".into(), root: true, steps: vec![Step::WriteFile { path: PathBuf::from("/").join(DELEGATE_CONF), content: DELEGATE_CONF_TEXT.into(), mode: 0o644 }] }),
    });
}

/// The kept core (`olo_system::cores`, `Settings::keep_core`, on by
/// default): the person's programs are kept off it by app_server; the
/// system's services by systemd's drop-in, root's (`kept_core`).
fn kept_core(roots: &Roots, r: &mut Report) {
    let cpus = roots.fs.join(olo_system::cores::CPU_DIR.trim_start_matches('/'));
    let Some(kept) = olo_system::cores::kept_in(&cpus) else { return };
    let online = olo_system::cores::online_in(&cpus);
    let settings: olo_protocol::Settings = read(&roots.temp.config.join("olo/app_server.json")).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let list = olo_system::cores::format_list(&kept);
    let steps = crate::kept_core::steps(settings.keep_core, &kept, &online, &roots.fs);
    if steps.is_empty() {
        if settings.keep_core {
            r.fine.push(Fine {
                title: format!("Processor {list} is kept for the window server"),
                detail: "Every key press and every frame go through app_server's loop and input thread, which have it to themselves: neither your programs nor the system's services run there.".into(),
            });
        }
        return;
    }
    r.findings.push(if settings.keep_core {
        Finding {
            id: "kept-core".into(),
            severity: Severity::Advice,
            title: "The system's services can run on the window server's core".into(),
            detail: format!(
                "Processor {list} is kept for app_server's loop and input thread (Preferences ▸ Performance ▸ Scheduler), and your programs run on the others, but the system's (an update, an indexer, a service's build) can still run there. The fix keeps them off it too: systemd starts them on the other {} from now on, and those running now are moved.",
                online.len() - kept.len()
            ),
            fix: Some(Fix { label: "Keep Them Off".into(), root: true, steps }),
        }
    } else {
        Finding {
            id: "kept-core".into(),
            severity: Severity::Advice,
            title: format!("The system's services are kept off processor {list}"),
            detail: "No core is kept for the window server now (Preferences ▸ Performance ▸ Scheduler), but systemd still starts the system's services on the other processors. The fix lets them use every one.".into(),
            fix: Some(Fix { label: "Let Them Back".into(), root: true, steps }),
        }
    });
}

/// A PCI device, as sysfs has it.
struct Pci {
    /// Its address ("0000:01:00.0") and folder.
    address: String,
    dir: PathBuf,
    class: u32,
    vendor: u16,
    /// Its driver's name, if one has it.
    driver: Option<String>,
    /// The card the computer started on.
    boot_vga: bool,
    power_control: Option<String>,
}

impl Pci {
    fn display(&self) -> bool {
        self.class >> 16 == 0x03
    }

    fn maker(&self) -> &'static str {
        match self.vendor {
            0x10de => "NVIDIA",
            0x1002 => "AMD",
            0x8086 => "Intel",
            _ => "other",
        }
    }
}

fn pci_devices(fs: &Path) -> Vec<Pci> {
    let hex = |p: &Path| read(p).and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok());
    let mut out: Vec<Pci> = std::fs::read_dir(fs.join("sys/bus/pci/devices"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            Some(Pci {
                address: e.file_name().to_string_lossy().into_owned(),
                class: hex(&dir.join("class"))?,
                vendor: hex(&dir.join("vendor"))? as u16,
                driver: std::fs::read_link(dir.join("driver")).ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())),
                boot_vga: read(&dir.join("boot_vga")).is_some_and(|s| s.trim() == "1"),
                power_control: read(&dir.join("power/control")).map(|s| s.trim().to_owned()),
                dir,
            })
        })
        .collect();
    out.sort_by(|a, b| a.address.cmp(&b.address));
    out
}

/// A second graphics card (NVIDIA's, AMD's) kept powered while nothing
/// uses it: runtime power management left off ("on"), as it is unless a
/// driver or a rule turns it on. With no driver bound (nouveau failing on
/// a new card) nothing ever does. (It wakes for a display wired to it,
/// which it composes for: app_server draws each display with its own GPU.)
fn idle_gpu(roots: &Roots, pcis: &[Pci], r: &mut Report) {
    let second: Vec<&Pci> = pcis.iter().filter(|p| p.display() && !p.boot_vga && matches!(p.vendor, 0x10de | 0x1002)).collect();
    if second.is_empty() {
        return;
    }
    let installed = std::fs::read_to_string(roots.fs.join(IDLE_GPU_RULE_PATH)).ok();
    let awake: Vec<&&Pci> = second.iter().filter(|p| p.power_control.as_deref() == Some("on")).collect();
    let maker = second[0].maker();
    if awake.is_empty() && installed.as_deref() == Some(IDLE_GPU_RULE) {
        r.fine.push(Fine { title: format!("The {maker} graphics card sleeps when unused"), detail: "Runtime power management is on for it (72-olo-idle-gpu.rules).".into() });
        return;
    }
    let mut steps = vec![Step::WriteFile { path: PathBuf::from("/").join(IDLE_GPU_RULE_PATH), content: IDLE_GPU_RULE.into(), mode: 0o644 }];
    for p in &second {
        let control = p.dir.join("power/control");
        steps.push(Step::WriteSysfs { path: PathBuf::from("/").join(control.strip_prefix(&roots.fs).unwrap_or(&control)), value: "auto".into() });
    }
    let unused = awake.iter().any(|p| p.driver.is_none());
    r.findings.push(Finding {
        id: "gpu:idle".into(),
        severity: if awake.is_empty() { Severity::Advice } else { Severity::Problem },
        title: if awake.is_empty() { format!("The {maker} graphics card's sleep isn't kept at boot") } else { format!("The {maker} graphics card never sleeps") },
        detail: format!(
            "{} so it draws power the whole time, warming the computer and running the battery down. Olo draws with the {} graphics. The fix lets it sleep while nothing uses it, now and at every start (/etc/udev/rules.d/72-olo-idle-gpu.rules); an app that wants it, or a display plugged into a port it drives, wakes it.",
            if unused { "No driver has it, and its runtime power management is off," } else if awake.is_empty() { "It sleeps now, but nothing keeps that after a restart," } else { "Its runtime power management is off," },
            pcis.iter().find(|p| p.display() && p.boot_vga).map_or("built-in", |p| p.maker()),
        ),
        fix: Some(Fix { label: "Let It Sleep".into(), root: true, steps }),
    });
}

/// NVIDIA's kernel modules.
const NVIDIA_MODULES: [&str; 5] = ["nvidia_drm", "nvidia_modeset", "nvidia_uvm", "nvidia_peermem", "nvidia"];

/// A graphics card whose power management failed: the kernel has it in
/// an error state (`runtime_status` "error": a suspend or a resume its
/// driver couldn't finish), as NVIDIA's on 2026-11-07 and 2026-11-20.
/// Said as what the kernel says, with what brings the card back, a
/// restart of the computer. **Olo doesn't start such a card again in
/// place.** It did (unloading NVIDIA's modules, resetting the card,
/// loading them again), it worked once, and the second time the unload
/// never came back from the kernel: NVIDIA's display module waits for a
/// card that doesn't answer with no way out, and nothing short of a
/// restart ends that (HACKING, "A graphics card that stopped
/// answering"). No order of unloading, unbinding and resetting is safe
/// with that driver, and nothing readable beforehand tells the state
/// that hangs from the one that doesn't.
///
/// And a module of NVIDIA's that stays on its way out (`initstate`
/// "going" now and still after `STUCK_FOR`: every unload passes through
/// "going", for a moment) is said as stuck, by its name, with what that
/// costs until the restart.
fn gpu_stuck(roots: &Roots, pcis: &[Pci], r: &mut Report) {
    let going = |m: &&str| read(&roots.fs.join("sys/module").join(m).join("initstate")).as_deref().map(str::trim) == Some("going");
    let mut stuck = NVIDIA_MODULES.into_iter().find(|m| going(m));
    if let Some(module) = stuck {
        // (Looked at again for a while: an ordinary unload is over in
        // well under that.)
        let until = std::time::Instant::now() + STUCK_FOR;
        while going(&module) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        if !going(&module) {
            stuck = None;
        }
    }
    let nvidia = pcis.iter().find(|p| p.display() && p.vendor == 0x10de);
    if let (Some(module), Some(card)) = (stuck, nvidia) {
        r.findings.push(Finding { id: format!("gpu:stuck:{}", card.address), severity: Severity::Problem, title: STUCK_TITLE.into(), detail: stuck_detail(module), fix: None });
    }
    for p in pcis.iter().filter(|p| p.display() && read(&p.dir.join("power/runtime_status")).as_deref().map(str::trim) == Some("error")) {
        // (NVIDIA's with its driver stuck is said above, once.)
        if stuck.is_some() && nvidia.is_some_and(|n| n.address == p.address) {
            continue;
        }
        let why = if p.vendor == 0x10de { " Olo doesn't try to start it again in place: unloading its driver after that can hang in the kernel." } else { "" };
        r.findings.push(Finding {
            id: format!("gpu:stuck:{}", p.address),
            severity: Severity::Problem,
            title: format!("The {} graphics card's power management failed", p.maker()),
            detail: format!("The kernel has the card in an error state after a power change it couldn't finish, and its driver may not answer: video it would decode, screen recordings it would encode, a display on its ports. Restarting the computer is what starts it again: save your work first.{why}"),
            fix: None,
        });
    }
}

/// How long a module of NVIDIA's is watched on its way out before it is
/// called stuck (none of it in a test).
const STUCK_FOR: std::time::Duration = if cfg!(test) { std::time::Duration::ZERO } else { std::time::Duration::from_secs(3) };

/// What is said of NVIDIA's driver stuck unloading: its title, and its
/// words with the module's name (the demo's too).
pub(crate) const STUCK_TITLE: &str = "NVIDIA's graphics driver is stuck";
pub(crate) fn stuck_detail(module: &str) -> String {
    format!(
        "Linux began shutting down NVIDIA's graphics driver and hasn't finished. Until the computer restarts, the graphics card can't be used, one of the processors may stay busy with it, and putting the computer to sleep may fail or hang. Save your work, then restart. (The part that is stuck is called {module}.)"
    )
}

/// The option that keeps NVIDIA's display module from taking a console:
/// read as the module loads, so in force from the next start.
const NVIDIA_CONSOLE_PATH: &str = "etc/modprobe.d/olo-nvidia-console.conf";
const NVIDIA_CONSOLE: &str = "# Olo Doctor: NVIDIA's display module takes no framebuffer console on a card that has no display.\n# (Giving that console up as the module unloads is where it can wait for ever on a card that\n# stopped answering; nothing shows on a card with no display anyway.)\noptions nvidia_drm fbdev=0\n";

/// NVIDIA's display module holding a framebuffer console on cards that
/// have no display (`nvidia_drm`'s `fbdev`, on as it comes): a laptop's
/// second card, wired to no screen and no port, there to compute. The
/// console shows nothing there, and giving it up is one of the things
/// the module does as it unloads, by programming the card's display
/// engine and waiting for it with no time limit: on a card that had
/// stopped answering that wait never ended (2026-11-20; `gpu_stuck`).
/// Without the console that step isn't taken. (Not the only one that
/// can wait, and nothing to do with why the card stopped answering:
/// the finding says what it is, no more.)
///
/// Offered only on what is known for certain, since the option is the
/// module's, for every card of NVIDIA's at once: the module loaded and
/// running with modesetting (one on its way out has already taken its
/// cards' DRM devices away: nothing can be told of them); every display
/// device of NVIDIA's its driver's, not the one the computer started
/// on, and with a DRM card of its own that has no connector at all
/// (read: one that can't be read isn't known, and nothing is offered);
/// and another display device the computer did start on. Then what the
/// running module says decides the rest: no console (N), fine; the
/// option kept since this start, fine, from the next; kept from before
/// this start and still not in force, said, with where to look.
fn nvidia_console(roots: &Roots, pcis: &[Pci], r: &mut Report) {
    let module = roots.fs.join("sys/module/nvidia_drm");
    let says = |name: &str| read(&module.join(name)).map(|s| s.trim().to_owned());
    if says("initstate").as_deref() != Some("live") || says("parameters/modeset").as_deref() != Some("Y") {
        return;
    }
    let file = roots.fs.join(NVIDIA_CONSOLE_PATH);
    let kept = std::fs::read_to_string(&file).is_ok_and(|kept| kept == NVIDIA_CONSOLE);
    match says("parameters/fbdev").as_deref() {
        Some("N") => {
            if kept {
                r.fine.push(Fine { title: "NVIDIA's driver takes no console".into(), detail: "The running driver says so (fbdev is off), and the option is kept (olo-nvidia-console.conf).".into() });
            }
            return;
        }
        Some("Y") => {}
        _ => return,
    }
    if kept {
        // (Written since the computer started: the running module read
        // its options before. From before this start and not in force:
        // something else decides.)
        let started = read(&roots.fs.join("proc/stat")).and_then(|t| t.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse::<u64>().ok()));
        let written = std::fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
        if started.zip(written).is_none_or(|(started, written)| written >= started) {
            r.fine.push(Fine {
                title: "NVIDIA's driver takes no console from the next start".into(),
                detail: "The option is kept (olo-nvidia-console.conf); the driver read its options before it was, and reads them again as the computer starts.".into(),
            });
        } else {
            r.findings.push(Finding {
                id: "gpu:console:kept".into(),
                severity: Severity::Advice,
                title: "NVIDIA's console option isn't in force".into(),
                detail: "The option that turns the console off is kept (olo-nvidia-console.conf) and the computer has started since, yet the running driver still has its console. Another setting says otherwise (the kernel's command line, another file in /etc/modprobe.d), or the driver is loaded from the start-up image, which was made before the option was there.".into(),
                fix: None,
            });
        }
        return;
    }
    // A DRM card of its own with no connector ("card1-DP-3" in "card1"):
    // Err where that can't be read, Ok(false) with no card or with a
    // connector.
    fn no_connectors(p: &Pci) -> std::io::Result<bool> {
        let mut card = false;
        for entry in std::fs::read_dir(p.dir.join("drm"))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.strip_prefix("card").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
                continue;
            }
            card = true;
            for inside in std::fs::read_dir(entry.path())? {
                if inside?.file_name().to_string_lossy().starts_with(&format!("{name}-")) {
                    return Ok(false);
                }
            }
        }
        Ok(card)
    }
    let nvidias: Vec<&Pci> = pcis.iter().filter(|p| p.display() && p.vendor == 0x10de).collect();
    let started_on_another = pcis.iter().any(|p| p.display() && p.vendor != 0x10de && p.boot_vga);
    let all_without_display = nvidias.iter().all(|p| !p.boot_vga && p.driver.as_deref() == Some("nvidia") && no_connectors(p).unwrap_or(false));
    let Some(card) = nvidias.first().filter(|_| started_on_another && all_without_display) else { return };
    r.findings.push(Finding {
        id: format!("gpu:console:{}", card.address),
        severity: Severity::Advice,
        title: CONSOLE_KEPT.0.into(),
        detail: CONSOLE_KEPT.1.into(),
        fix: Some(Fix { label: CONSOLE_KEPT.2.into(), root: true, steps: vec![Step::WriteFile { path: PathBuf::from("/").join(NVIDIA_CONSOLE_PATH), content: NVIDIA_CONSOLE.into(), mode: 0o644 }] }),
    });
}

/// What is said of it: its title, its words and its fix's name (the
/// demo's too).
pub(crate) const CONSOLE_KEPT: (&str, &str, &str) = (
    "NVIDIA's driver keeps a console no one can see",
    "NVIDIA's driver keeps a text console on its graphics card, though no screen is connected to the card to show it. Handing that console back is one of the steps where the driver can get stuck when it is shut down. This turns the console off on every NVIDIA card; drawing and computing on a card don't need it. It takes effect the next time the computer starts.",
    "Turn Off the Console",
);

/// A graphics card no driver has, when one tried (nouveau loaded and
/// gone): a card newer than the kernel's driver knows.
fn gpu_driver(roots: &Roots, pcis: &[Pci], r: &mut Report) {
    let nvidia: Vec<&Pci> = pcis.iter().filter(|p| p.display() && p.driver.is_none() && p.vendor == 0x10de).collect();
    if nvidia.is_empty() {
        return;
    }
    // NVIDIA's driver, built for the running kernel (akmod: again for
    // each new one), from the system's repositories (Fedora's NVIDIA
    // repository, RPM Fusion's).
    let found = (roots.packages)(&[NVIDIA_DRIVER]);
    let package = found.as_ref().and_then(|ps| ps.iter().find(|(n, _)| n == NVIDIA_DRIVER));
    let secure_boot = read_bytes(&roots.fs.join(SECURE_BOOT_VAR)).is_some_and(|b| b.last() == Some(&1));
    for p in nvidia {
        let tried = roots.fs.join("sys/module/nouveau").exists();
        let why = if tried { "Linux's own driver for it (nouveau) couldn't start it: the card is newer than the driver knows." } else { "No driver is installed for it." };
        let drawn = pcis.iter().find(|x| x.display() && x.boot_vga).map_or("built-in", |x| x.maker());
        let uses = format!(
            "Olo and its apps run on the {drawn} graphics, so nothing is missing day to day; an app asking for the NVIDIA card (a game, CUDA) won't find it, nor will a display plugged into a port it drives."
        );
        let (detail, fix) = match package {
            // Installed, not working yet: built at the next start.
            Some((_, true)) => (format!("{why} NVIDIA's driver is installed: restart, and it's built for this kernel and started (the first start takes a few minutes longer)."), None),
            Some((_, false)) => (
                format!(
                    "{why} {uses} The fix installs NVIDIA's driver ({NVIDIA_DRIVER}, from the system's NVIDIA repository), built for this kernel; restart when it's done{}.",
                    if secure_boot { ", and enrol its signing key when the computer asks (Secure Boot)" } else { "" }
                ),
                Some(Fix { label: "Install NVIDIA's Driver".into(), root: true, steps: vec![Step::InstallPackages { names: vec![NVIDIA_DRIVER.into()] }] }),
            ),
            None => (
                format!(
                    "{why} {uses} NVIDIA's driver isn't in any repository this computer uses: on Fedora, turn on the third-party NVIDIA repository (or add RPM Fusion's), and Olo Doctor can install it."
                ),
                None,
            ),
        };
        r.findings.push(Finding { id: format!("gpu:driver:{}", p.address), severity: Severity::Advice, title: "The NVIDIA graphics card has no working driver".into(), detail, fix });
    }
}

/// NVIDIA's driver, as Fedora's repositories name it.
const NVIDIA_DRIVER: &str = "akmod-nvidia";

/// The swap areas in use, as the kernel lists them.
const SWAPS_PATH: &str = "proc/swaps";
/// zram-generator's settings for this computer, which replace the
/// system's own whole (Fedora's: a swap in compressed memory, up to
/// 8 GB): one naming no device makes none.
pub const ZRAM_CONF_PATH: &str = "etc/systemd/zram-generator.conf";
pub const ZRAM_OFF_CONF: &str = "# Olo: no swap in compressed memory (Olo Doctor, \"Turn Swap Off\").\n# Intel's graphics driver (xe) moves windows' buffers out to swap whenever\n# the kernel tidies memory, and the desktop stalls while they come back.\n# This file, naming no device, replaces the system's own\n# (/usr/lib/systemd/zram-generator.conf); remove it to have that back.\n";

/// Olo's live patch that keeps graphics memory out of swap
/// (kernel/gpu-memory, `olo_gpu_memory`): the service that builds it for
/// the running kernel and loads it at each start (apps/gpu-memory), its
/// program beside ours and where root's copy and its unit go, where the
/// kernel lists the patch once it's in, and where the service says how
/// it went.
pub const GPU_MEMORY_UNIT: &str = include_str!("../../../data/systemd/olo-gpu-memory.service");
pub const GPU_MEMORY_SERVICE: &str = "olo-gpu-memory.service";
pub const GPU_MEMORY_PROGRAM: &str = "olo-gpu-memory";
const GPU_MEMORY_PROGRAM_PATH: &str = "usr/local/libexec/olo/olo-gpu-memory";
const GPU_MEMORY_UNIT_PATH: &str = "etc/systemd/system/olo-gpu-memory.service";
const GPU_MEMORY_PATCH: &str = "sys/kernel/livepatch/olo_gpu_memory";
const LIVE_PATCHES: &str = "sys/kernel/livepatch";
/// What building the patch for a kernel takes: its headers (the package
/// that follows each new kernel), the compiler and make.
const KERNEL_BUILD_PACKAGES: &[&str] = &["kernel-devel-matched", "gcc", "make"];

/// Olo's patch is in the running kernel: GPU buffers aren't backed up to
/// swap.
fn graphics_memory_patched(fs: &Path) -> bool {
    read(&fs.join(GPU_MEMORY_PATCH).join("enabled")).is_some_and(|e| e.trim() == "1")
}

/// The steps that turn swap off and keep it off: what turns each area on
/// at every start first; then systemd told, so the units it made for the
/// areas are gone before they're turned off (turned off alone, an area
/// is back within minutes: swap's target wants its unit, and the next
/// system service that starts brings the target's wants up again; Ben's
/// computer, 2026-11-16: off at 11:47, on again at 11:58 as PackageKit
/// started); then zram's devices given back (their set-up services,
/// which systemd no longer stops with the swap once their units are
/// gone).
fn swap_off_steps(roots: &Roots, areas: &[(String, u64)]) -> Vec<Step> {
    let mut steps = Vec::new();
    if areas.iter().any(|(name, _)| name.starts_with("/dev/zram")) {
        steps.push(Step::WriteFile { path: PathBuf::from("/").join(ZRAM_CONF_PATH), content: ZRAM_OFF_CONF.into(), mode: 0o644 });
    }
    if areas.iter().any(|(name, _)| !name.starts_with("/dev/zram")) {
        steps.push(Step::FstabSwapOff);
    }
    steps.push(Step::Unit { name: "swap".into(), action: UnitAction::Reload });
    steps.push(Step::SwapOff);
    for (name, _) in areas {
        let Some(device) = name.strip_prefix("/dev/").filter(|d| d.starts_with("zram")) else { continue };
        let service = format!("systemd-zram-setup@{device}.service");
        if (roots.units)(&service).is_ok_and(|u| u.active) {
            steps.push(Step::Unit { name: service, action: UnitAction::Stop });
        }
    }
    steps
}

/// The steps that put Olo's patch service in place, from the
/// `olo-gpu-memory` beside ours: only what isn't so already (its program
/// and unit where they're missing or not this build's, enabled, and
/// started (again, if either changed: it takes an older patch out and
/// builds this one), with what the kernel's build needs installed first
/// where it's missing).
pub(crate) fn graphics_memory_steps(roots: &Roots) -> Vec<Step> {
    let fs = &roots.fs;
    let mut steps = Vec::new();
    let release = read(&fs.join("proc/sys/kernel/osrelease")).map(|r| r.trim().to_owned()).unwrap_or_default();
    let can_build = fs.join("lib/modules").join(&release).join("build/Makefile").is_file() && fs.join("usr/bin/make").is_file() && fs.join("usr/bin/gcc").is_file();
    if !can_build {
        steps.push(Step::InstallPackages { names: KERNEL_BUILD_PACKAGES.iter().map(|n| (*n).to_owned()).collect() });
    }
    let (from, program, unit) = (roots.bin.join(GPU_MEMORY_PROGRAM), PathBuf::from("/").join(GPU_MEMORY_PROGRAM_PATH), PathBuf::from("/").join(GPU_MEMORY_UNIT_PATH));
    let mut changed = false;
    if !same_contents(&from, &fs.join(GPU_MEMORY_PROGRAM_PATH)) {
        steps.push(Step::CopyFile { from, to: program.clone(), mode: 0o755 });
        // (Its label as the folder's own says: a program systemd may run.)
        steps.push(Step::SelinuxLabels { rules: Vec::new(), restore: vec![program] });
        changed = true;
    }
    if std::fs::read_to_string(fs.join(GPU_MEMORY_UNIT_PATH)).ok().as_deref() != Some(GPU_MEMORY_UNIT) {
        steps.push(Step::WriteFile { path: unit, content: GPU_MEMORY_UNIT.into(), mode: 0o644 });
        changed = true;
    }
    if changed {
        steps.push(Step::Unit { name: GPU_MEMORY_SERVICE.into(), action: UnitAction::Reload });
    }
    let state = (roots.units)(GPU_MEMORY_SERVICE).ok();
    if changed || !state.as_ref().is_some_and(|s| s.enabled) {
        steps.push(Step::Unit { name: GPU_MEMORY_SERVICE.into(), action: UnitAction::Enable });
    }
    if changed || !can_build || !graphics_memory_patched(fs) || !state.as_ref().is_some_and(|s| s.active) {
        steps.push(Step::Unit { name: GPU_MEMORY_SERVICE.into(), action: UnitAction::Restart });
    }
    steps
}

/// Windows' graphics memory moved out to swap. Intel's xe driver backs
/// its buffers up to swap (TTM's shrinker: `xe_shrinker_scan`) whenever
/// the kernel is short of memory, were it only file cache (a build's
/// output), and only while there is swap space to take them. A buffer
/// comes back inside its program's next GPU submission, under its
/// window buffers' locks, which app_server's every submission takes
/// too: its loop stood still while any window's came back (windows not
/// drawn, keys late, one pressed before the wait typed twice). Ben's
/// computer, 2026-11-16, building three apps: 47 frames held (the worst
/// 951 ms) and three keys late with 8 GB of zram swap, 1 GiB of buffers
/// out; none with swap off, and none with it on and Olo's patch in
/// (HACKING, "Windows' buffers stay in memory").
///
/// The fix is Olo's live patch (`graphics_memory_steps`): the driver is
/// told there is no swap for its buffers, and the computer keeps its
/// swap for the rest. Where the kernel can't take it (Secure Boot loads
/// only signed modules; a kernel without live patching), or its service
/// couldn't build or load it, swap is turned off instead
/// (`swap_off_steps`).
fn graphics_swap(roots: &Roots, pcis: &[Pci], r: &mut Report) {
    if !pcis.iter().any(|p| p.display() && p.driver.as_deref() == Some("xe")) {
        return;
    }
    let fs = &roots.fs;
    let areas = read(&fs.join(SWAPS_PATH)).map(|t| crate::fix::swap_areas(&t)).unwrap_or_default();
    if areas.is_empty() {
        r.fine.push(Fine { title: "Windows' graphics memory stays in memory".into(), detail: "Swap is off, so Intel's graphics driver (xe) has nowhere to move windows' buffers out to.".into() });
        return;
    }
    // (Without the service's program beside ours, an Olo from before it
    // or a build tree's, there's nothing to install it from.)
    let ours = roots.bin.join(GPU_MEMORY_PROGRAM).is_file();
    let patched = graphics_memory_patched(fs);
    let steps = if ours { graphics_memory_steps(roots) } else { Vec::new() };
    let kept = fs.join(GPU_MEMORY_UNIT_PATH).is_file() && (roots.units)(GPU_MEMORY_SERVICE).is_ok_and(|s| s.enabled);
    if patched && steps.is_empty() && (ours || kept) {
        r.fine.push(Fine {
            title: "Windows' graphics memory stays in memory".into(),
            detail: "Olo's kernel patch (olo_gpu_memory) is in: Intel's graphics driver (xe) is told there's no swap for windows' buffers, and the computer keeps its swap for everything else.".into(),
        });
        return;
    }
    if patched {
        r.findings.push(Finding {
            id: "graphics-swap:service".into(),
            severity: Severity::Advice,
            title: "Windows' graphics memory is kept out of swap only until the next start".into(),
            detail: format!(
                "Olo's kernel patch (olo_gpu_memory) is in the running kernel, but the service that builds it for each kernel and loads it at every start isn't installed, or isn't this build's. {}",
                if ours { "The fix puts it in place." } else { "This Olo has no olo-gpu-memory beside it to install it from." }
            ),
            fix: ours.then(|| Fix { label: "Keep It at Every Start".into(), root: true, steps }),
        });
        return;
    }
    let total: u64 = areas.iter().map(|(_, size)| size).sum();
    let size = olo_system::dnf::size_words(total).replace(' ', "\u{a0}");
    let what = match (areas.iter().any(|(n, _)| n.starts_with("/dev/zram")), areas.iter().any(|(n, _)| !n.starts_with("/dev/zram"))) {
        (true, false) => "in compressed memory",
        (false, _) => "on disk",
        (true, true) => "in compressed memory and on disk",
    };
    let trouble = format!(
        "Swap is on ({size} {what}), and Intel's graphics driver (xe) moves windows' buffers out to it whenever the kernel tidies memory, even when it's only a build's or a copy's files filling it. A window's next frame then waits in the kernel while its buffers come back, and everything on screen waits with it: windows stop drawing and keys arrive late (a letter typed twice), for up to seconds."
    );
    let swap_off = "Without swap, a computer that runs out of memory has its largest program stopped rather than slowing down.";
    let secure_boot = read_bytes(&fs.join(SECURE_BOOT_VAR)).is_some_and(|b| b.last() == Some(&1));
    // How the service's last try went, where it has tried.
    let said = read(&fs.join("run/olo-gpu-memory/status")).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let failed = said.as_deref().and_then(|s| s.strip_prefix("failed: "));
    let (detail, label, steps) = if !ours || secure_boot || !fs.join(LIVE_PATCHES).is_dir() {
        let why = if secure_boot {
            "Olo's kernel patch would keep them in memory and leave swap on, but with Secure Boot this kernel loads only signed modules. "
        } else if ours {
            "Olo's kernel patch would keep them in memory and leave swap on, but this kernel can't be patched while it runs. "
        } else {
            ""
        };
        (format!("{trouble} {why}The fix turns swap off, now and at every start; the driver then keeps every buffer in memory. {swap_off}"), "Turn Swap Off", swap_off_steps(roots, &areas))
    } else if let Some(why) = failed.filter(|why| !why.contains("headers aren't installed") && !why.contains("make couldn't be run")) {
        (
            format!("{trouble} Olo's kernel patch, which keeps them in memory and leaves swap on, couldn't be put in: {why}. The fix turns swap off instead, now and at every start. {swap_off}"),
            "Turn Swap Off",
            swap_off_steps(roots, &areas),
        )
    } else {
        (
            format!(
                "{trouble} The fix puts Olo's kernel patch in (olo_gpu_memory: one function of the running kernel replaced, no restart): the driver is told there's no swap for its buffers, and the computer keeps its swap for everything else. A service builds the patch for each new kernel at its first start (a few seconds) and loads it at every start{}.",
                if steps.iter().any(|s| matches!(s, Step::InstallPackages { .. })) { "; what building it needs (the kernel's headers, the compiler, make) is installed first" } else { "" }
            ),
            "Keep Graphics Memory in Memory",
            steps,
        )
    };
    r.findings.push(Finding {
        id: "graphics-swap".into(),
        severity: Severity::Problem,
        title: "Windows' graphics memory is moved out to swap".into(),
        detail,
        fix: Some(Fix { label: label.into(), root: true, steps }),
    });
}

/// BBRv3 for TCP (kernel/bbr3, `tcp_bbr3`: the congestion control
/// "bbr3"): the service that builds Olo's module for the running kernel,
/// loads it and makes it what new connections use, at each start
/// (apps/bbr3), its program beside ours and where root's copy and its
/// unit go, where the kernel lists the module once it's loaded, and
/// where the service says how it went.
pub const BBR3_UNIT: &str = include_str!("../../../data/systemd/olo-bbr3.service");
pub const BBR3_SERVICE: &str = "olo-bbr3.service";
pub const BBR3_PROGRAM: &str = "olo-bbr3";
const BBR3_PROGRAM_PATH: &str = "usr/local/libexec/olo/olo-bbr3";
const BBR3_UNIT_PATH: &str = "etc/systemd/system/olo-bbr3.service";
const BBR3_MODULE: &str = "sys/module/tcp_bbr3";
const BBR3_SAID: &str = "run/olo-bbr3/status";
const TCP_CONGESTION_PATH: &str = "proc/sys/net/ipv4/tcp_congestion_control";

/// New connections use BBRv3: Olo's module's, loaded; or the kernel's
/// own "bbr" where that is BBRv3.
fn bbr3_in_use(roots: &Roots) -> bool {
    let now = read(&roots.fs.join(TCP_CONGESTION_PATH));
    match now.as_deref() {
        Some("bbr3") => roots.fs.join(BBR3_MODULE).is_dir(),
        Some("bbr") => roots.kernels_bbr.is_some_and(|v| v >= 3),
        _ => false,
    }
}

/// The steps that put Olo's BBRv3 service in place, from the `olo-bbr3`
/// beside ours: only what isn't so already (its program and unit where
/// they're missing or not this build's, enabled, and started (again, if
/// either changed), with what the kernel's build needs installed first
/// where it's missing; nothing to build where the kernel's own "bbr" is
/// BBRv3, which the service then chooses).
pub(crate) fn bbr3_steps(roots: &Roots) -> Vec<Step> {
    let fs = &roots.fs;
    let mut steps = Vec::new();
    let release = read(&fs.join("proc/sys/kernel/osrelease")).unwrap_or_default();
    let can_build =
        roots.kernels_bbr.is_some_and(|v| v >= 3) || (fs.join("lib/modules").join(&release).join("build/Makefile").is_file() && fs.join("usr/bin/make").is_file() && fs.join("usr/bin/gcc").is_file());
    if !can_build {
        steps.push(Step::InstallPackages { names: KERNEL_BUILD_PACKAGES.iter().map(|n| (*n).to_owned()).collect() });
    }
    let (from, program, unit) = (roots.bin.join(BBR3_PROGRAM), PathBuf::from("/").join(BBR3_PROGRAM_PATH), PathBuf::from("/").join(BBR3_UNIT_PATH));
    let mut changed = false;
    if !same_contents(&from, &fs.join(BBR3_PROGRAM_PATH)) {
        steps.push(Step::CopyFile { from, to: program.clone(), mode: 0o755 });
        // (Its label as the folder's own says: a program systemd may run.)
        steps.push(Step::SelinuxLabels { rules: Vec::new(), restore: vec![program] });
        changed = true;
    }
    if std::fs::read_to_string(fs.join(BBR3_UNIT_PATH)).ok().as_deref() != Some(BBR3_UNIT) {
        steps.push(Step::WriteFile { path: unit, content: BBR3_UNIT.into(), mode: 0o644 });
        changed = true;
    }
    if changed {
        steps.push(Step::Unit { name: BBR3_SERVICE.into(), action: UnitAction::Reload });
    }
    let state = (roots.units)(BBR3_SERVICE).ok();
    if changed || !state.as_ref().is_some_and(|s| s.enabled) {
        steps.push(Step::Unit { name: BBR3_SERVICE.into(), action: UnitAction::Enable });
    }
    if changed || !can_build || !bbr3_in_use(roots) || !state.as_ref().is_some_and(|s| s.active) {
        steps.push(Step::Unit { name: BBR3_SERVICE.into(), action: UnitAction::Restart });
    }
    steps
}

/// Olo's guard against a flood of the embedded controller's events
/// (kernel/ec-storm, `olo_ec_storm`): the service that builds it for the
/// running kernel and loads it at each start (apps/ec-storm), its
/// program beside ours and where root's copy and its unit go, where the
/// kernel lists the module once it's in, and where the service says how
/// it went.
pub const EC_STORM_UNIT: &str = include_str!("../../../data/systemd/olo-ec-storm.service");
pub const EC_STORM_SERVICE: &str = "olo-ec-storm.service";
pub const EC_STORM_PROGRAM: &str = "olo-ec-storm";
const EC_STORM_PROGRAM_PATH: &str = "usr/local/libexec/olo/olo-ec-storm";
const EC_STORM_UNIT_PATH: &str = "etc/systemd/system/olo-ec-storm.service";
const EC_STORM_MODULE: &str = "sys/module/olo_ec_storm";
/// The kernel series its way into the driver was checked on.
const EC_STORM_CHECKED: &str = include_str!("../../../kernel/ec-storm/CHECKED");

/// The steps that put the guard's service in place, from the
/// `olo-ec-storm` beside ours: only what isn't so already (as
/// `graphics_memory_steps`).
pub(crate) fn ec_guard_steps(roots: &Roots) -> Vec<Step> {
    let fs = &roots.fs;
    let mut steps = Vec::new();
    let release = read(&fs.join("proc/sys/kernel/osrelease")).map(|r| r.trim().to_owned()).unwrap_or_default();
    let can_build = fs.join("lib/modules").join(&release).join("build/Makefile").is_file() && fs.join("usr/bin/make").is_file() && fs.join("usr/bin/gcc").is_file();
    if !can_build {
        steps.push(Step::InstallPackages { names: KERNEL_BUILD_PACKAGES.iter().map(|n| (*n).to_owned()).collect() });
    }
    let (from, program, unit) = (roots.bin.join(EC_STORM_PROGRAM), PathBuf::from("/").join(EC_STORM_PROGRAM_PATH), PathBuf::from("/").join(EC_STORM_UNIT_PATH));
    let mut changed = false;
    if !same_contents(&from, &fs.join(EC_STORM_PROGRAM_PATH)) {
        steps.push(Step::CopyFile { from, to: program.clone(), mode: 0o755 });
        steps.push(Step::SelinuxLabels { rules: Vec::new(), restore: vec![program] });
        changed = true;
    }
    if std::fs::read_to_string(fs.join(EC_STORM_UNIT_PATH)).ok().as_deref() != Some(EC_STORM_UNIT) {
        steps.push(Step::WriteFile { path: unit, content: EC_STORM_UNIT.into(), mode: 0o644 });
        changed = true;
    }
    if changed {
        steps.push(Step::Unit { name: EC_STORM_SERVICE.into(), action: UnitAction::Reload });
    }
    let state = (roots.units)(EC_STORM_SERVICE).ok();
    if changed || !state.as_ref().is_some_and(|s| s.enabled) {
        steps.push(Step::Unit { name: EC_STORM_SERVICE.into(), action: UnitAction::Enable });
    }
    if changed || !can_build || !fs.join(EC_STORM_MODULE).is_dir() || !state.as_ref().is_some_and(|s| s.active) {
        steps.push(Step::Unit { name: EC_STORM_SERVICE.into(), action: UnitAction::Restart });
    }
    steps
}

/// A laptop's embedded controller can flood the kernel with one event
/// over and over, and the kernel queues the firmware's slow handler for
/// each without limit: the brightness keys' and the ports' wait behind
/// (a ThinkPad P1 Gen 9, 2026-11-17: two hours, 195,778 in line). Olo's
/// guard (`ec_guard_steps`) keeps the queue short whatever the
/// controller does. Suggested where there is a controller, the kernel
/// can take the module (no Secure Boot) and its series is one the
/// driver was checked in; said fine once it's in and kept.
fn ec_guard(roots: &Roots, r: &mut Report) {
    let fs = &roots.fs;
    let controller = std::fs::read_dir(fs.join("sys/bus/acpi/devices")).is_ok_and(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("PNP0C09:")));
    if !controller {
        return;
    }
    let title = "A flood of controller events can’t hold up the keys";
    let ours = roots.bin.join(EC_STORM_PROGRAM).is_file();
    let loaded = fs.join(EC_STORM_MODULE).is_dir();
    let steps = if ours { ec_guard_steps(roots) } else { Vec::new() };
    if loaded && steps.is_empty() {
        let stats = read(&fs.join(EC_STORM_MODULE).join("parameters/stats")).map(|s| format!(" ({})", s.trim())).unwrap_or_default();
        r.fine.push(Fine {
            title: title.into(),
            detail: format!("Olo's guard (olo_ec_storm) is in the kernel: of one event of the embedded controller's arriving over and over, one runs its handler and the rest are counted{stats}."),
        });
        return;
    }
    let release = read(&fs.join("proc/sys/kernel/osrelease")).map(|r| r.trim().to_owned()).unwrap_or_default();
    let series = release.split('.').take(2).collect::<Vec<_>>().join(".");
    let checked = EC_STORM_CHECKED.lines().map(str::trim).any(|l| !l.starts_with('#') && l == series);
    let secure_boot = read_bytes(&fs.join(SECURE_BOOT_VAR)).is_some_and(|b| b.last() == Some(&1));
    // (Nothing to offer: no program to install it from, a kernel that
    // loads only signed modules, or a series the driver wasn't checked in.)
    if !ours || secure_boot || !checked {
        return;
    }
    // How the service's last try went, where it has tried.
    let failed = read(&fs.join("run/olo-ec-storm/status")).and_then(|s| s.trim().strip_prefix("failed: ").map(str::to_owned));
    let tried = failed.filter(|why| !why.contains("headers aren't installed") && !why.contains("make couldn't be run")).map(|why| format!(" Its service's last try: {why}.")).unwrap_or_default();
    r.findings.push(Finding {
        id: "ec-guard".into(),
        severity: Severity::Suggestion,
        title: "Guard the keys against a flood of controller events".into(),
        detail: format!(
            "A laptop's embedded controller tells the system of each thing that happens to it (a key of the top row, the lid, the charger), and the kernel lines up a handler for each without limit. A controller that repeats itself, as a ThinkPad P1's did with its charger 23 times a second, fills that line faster than it empties, and the brightness keys and news of the charger and the USB-C ports wait behind for as long as it lasts. The fix puts Olo's guard in the kernel (olo_ec_storm, a module built for this kernel{} and loaded at every start): of one event arriving over and over, one runs its handler and the rest are only counted, so the others run as they come. It changes nothing while the controller behaves.{tried}",
            if steps.iter().any(|s| matches!(s, Step::InstallPackages { .. })) { "; what building it needs (the kernel's headers, the compiler, make) is installed first" } else { "" }
        ),
        fix: Some(Fix { label: "Guard the Keys".into(), root: true, steps }),
    });
}

/// The laptop's embedded controller is flooding the kernel with events,
/// as the supervisor measures it (`olo_system::ec_storm`; its file,
/// there while it lasts): said, with what it holds up and what clears
/// it. Nothing here mends it: only a full power-off does.
fn ec_storm(roots: &Roots, r: &mut Report) {
    use olo_protocol::performance::EcStorm;
    let Some(storm) = EcStorm::load(&EcStorm::path_under(&roots.temp.runtime)) else { return };
    r.findings.push(Finding { id: "ec-storm".into(), severity: Severity::Critical, title: EcStorm::title().into(), detail: storm.detail(), fix: None });
}

/// BBRv3 as TCP's congestion control, offered (Ben, 2026-11-16: "i also
/// want olo doctor to offer to set the tcp congestion control algo to
/// BBRv3 if it's not. if BBRv3 is not shipped with fedora then vendor
/// the kernel module/patches and adapt if needed and then have olo
/// doctor take care of doing everything needed to switch to BBRv3").
///
/// How fast TCP sends is its congestion control's to decide. Linux's
/// default, CUBIC, sends faster until a buffer along the way overflows;
/// its "bbr" is BBRv1. BBRv3 isn't in Linux (Google's is a patched
/// kernel), so Olo carries it as a module for the kernel as it comes
/// (kernel/bbr3/README.md: what it took, how it was shown to be BBRv3,
/// and the measurements quoted here: a 50 Mbit/s link with 40 ms of
/// round trip and a queue of 170 packets, 12 s of sending, in a KVM
/// guest; `tools/dev/bbr3-check --compare` makes them again). Where the
/// kernel's own "bbr" is BBRv3 already, the service chooses that one.
/// Where the kernel can't take Olo's module (Secure Boot loads only
/// signed ones) nothing is offered: there's nothing here to do about
/// it.
fn tcp_bbr3(roots: &Roots, r: &mut Report) {
    let fs = &roots.fs;
    let Some(now) = read(&fs.join(TCP_CONGESTION_PATH)).filter(|n| !n.is_empty()) else { return };
    let kernels = roots.kernels_bbr.is_some_and(|v| v >= 3);
    // (Without the service's program beside ours, an Olo from before it
    // or a build tree's, there's nothing to install it from.)
    let ours = roots.bin.join(BBR3_PROGRAM).is_file();
    let steps = if ours { bbr3_steps(roots) } else { Vec::new() };
    let kept = fs.join(BBR3_UNIT_PATH).is_file() && (roots.units)(BBR3_SERVICE).is_ok_and(|s| s.enabled);
    let in_use = bbr3_in_use(roots);
    if in_use && steps.is_empty() && (ours || kept) {
        r.fine.push(Fine {
            title: "TCP sends with BBRv3".into(),
            detail: if now == "bbr" {
                "New connections use this kernel's own bbr, which is BBRv3.".into()
            } else {
                // (Built for whatever kernel an update brings: said where
                // that kernel's series is one it hasn't been checked on.)
                let series = olo_system::kernel_tuning::series(&read(&fs.join("proc/sys/kernel/osrelease")).unwrap_or_default());
                format!(
                    "New connections use bbr3: Olo's module (Google's BBRv3, made for the kernel as it comes), built for this kernel and loaded at every start.{}",
                    if series.is_empty() || olo_system::kernel_tuning::bbr3_checked_on(&series) {
                        String::new()
                    } else {
                        format!(" It was built for this kernel as for any, but hasn't yet passed its self-check on this kernel's series ({series}): an Olo update brings that.")
                    }
                )
            },
        });
        return;
    }
    if in_use {
        r.findings.push(Finding {
            id: "tcp-bbr3:service".into(),
            severity: Severity::Advice,
            title: "TCP sends with BBRv3 only until the next start".into(),
            detail: format!(
                "New connections use BBRv3 ({now}), but the service that sees to it at every start (and builds Olo's module for each new kernel) isn't installed, or isn't this build's. {}",
                if ours { "The fix puts it in place." } else { "This Olo has no olo-bbr3 beside it to install it from." }
            ),
            fix: ours.then(|| Fix { label: "Keep It at Every Start".into(), root: true, steps }),
        });
        return;
    }
    let secure_boot = read_bytes(&fs.join(SECURE_BOOT_VAR)).is_some_and(|b| b.last() == Some(&1));
    if !ours || (secure_boot && !kernels) {
        return;
    }
    let does = match now.as_str() {
        "cubic" => "CUBIC, Linux's default: it sends faster and faster until a buffer along the way overflows and a packet is dropped, so an upload keeps the slowest link's queue full, and everything else waits behind it".to_owned(),
        "bbr" => "the kernel's bbr, which is BBRv1: it goes by a model of the path too, but loses many more packets where the path's buffers are small (0.6% of what it sent was sent again, against BBRv3's 0.03%, in the test below), and takes more than its share beside other connections (40 of the 50 Mbit/s beside a CUBIC flow, where BBRv3 took 25)".to_owned(),
        other => other.to_owned(),
    };
    // How the service's last try went, where it has tried.
    let failed = read(&fs.join(BBR3_SAID)).and_then(|s| s.strip_prefix("failed: ").map(str::to_owned));
    let how = if kernels {
        "This kernel has it as its own bbr; the fix has new connections use it, now and at every start.".to_owned()
    } else {
        format!(
            "Linux doesn't have it, so the fix puts in Olo's module (tcp_bbr3: Google's BBRv3, made for the kernel as it comes) and a service that builds it for each new kernel at its first start (a few seconds), loads it at every start and has new connections use it{}.{}",
            if steps.iter().any(|s| matches!(s, Step::InstallPackages { .. })) { "; what building it needs (the kernel's headers, the compiler, make) is installed first" } else { "" },
            failed.map(|why| format!(" At its last try it couldn't: {why}.")).unwrap_or_default()
        )
    };
    r.findings.push(Finding {
        id: "tcp-bbr3".into(),
        severity: Severity::Suggestion,
        title: "TCP could send with BBRv3".into(),
        detail: format!(
            "New TCP connections here use {does}. BBRv3 (Google's) keeps a model of the path, its bandwidth and its round-trip time, and sends at that: on a test link of 50 Mbit/s with 40 ms of round trip it sent 45.8 Mbit/s to CUBIC's 47.1, with the round trip at 43 ms where CUBIC's was 69. It governs what this computer sends (uploads, calls, what it serves to others); what it receives is paced by whoever sends it. {how}"
        ),
        fix: Some(Fix { label: "Use BBRv3".into(), root: true, steps }),
    });
}

/// The kernel's settings Olo suggests for a computer someone sits at
/// (`olo_system::kernel_tuning`, apps/doctor/src/kernel.rs; Ben,
/// 2026-11-16: "research what other linux kernel patches are recommended
/// for high performance, responsive desktop operating systems and if you
/// find any that are win-win over the fedora 45 kernel and would be a
/// win for olo users then also integrate them into olo like bbr3 so that
/// olo doctor can offer them"). What the desktop kernels patch in is
/// nearly all in Fedora's kernel already, or can't be had without
/// replacing it; what was left to gain, and measured to be a gain with
/// nothing given up for it, is two settings: full preemption, and a
/// limit on what waits in memory for a removable disk (kernel/TUNING.md
/// has each one's measurements, made in a KVM guest on this kernel by
/// tools/dev/kernel-check, and what was measured and left alone: the
/// multi-generational LRU's thrash protection among it, which ended a
/// program that fitted in three runs of nine). Each is offered where the
/// kernel has it and nobody
/// chose otherwise; set, it's said to be fine. Preferences ▸ Performance
/// ▸ Kernel shows and switches the same.
fn kernel_settings(roots: &Roots, r: &mut Report) {
    use olo_system::kernel_tuning::{self as tuning, Setting};
    let places = crate::kernel::places(&roots.fs);
    let size = |bytes: u64| if bytes >= 1 << 30 { format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64) } else { format!("{:.0} MB", bytes as f64 / (1u64 << 20) as f64) };

    // Preemption.
    if let Some(p) = tuning::preemption(&places) {
        let steps = crate::kernel::steps(Setting::Preemption, true, &places);
        if p.on() {
            r.fine.push(Fine { title: "The kernel switches to a woken program at once".into(), detail: "Full preemption, now and at every start.".into() });
        } else if steps.is_empty() {
            // (Kept for the next start, and this kernel can't be switched
            // while it runs: nothing to do but restart.)
            r.fine.push(Fine {
                title: "The kernel switches to a woken program at once from the next restart".into(),
                detail: format!("Full preemption is set for every start; the running kernel is on {} until then (it's locked down, so it can't be switched while it runs).", p.now()),
            });
        } else if p.lost {
            // Kept for every start, and this start was made without it.
            r.findings.push(Finding {
                id: "kernel:preemption".into(),
                severity: Severity::Advice,
                title: "The kernel was started without full preemption".into(),
                detail: format!(
                    "Full preemption is kept for every start, but this start's boot entry doesn't have preempt=full (an entry made some other way than from the kept command line, as a new kernel's can be), so the kernel is on {}. The fix puts it in every boot entry again{}.",
                    p.now(),
                    if p.live { ", and switches the running kernel now" } else { ", from the next restart" }
                ),
                fix: Some(Fix { label: "Preempt at Once".into(), root: true, steps }),
            });
        } else if !p.chosen_otherwise() && !scheduled_by_another(&roots.fs) {
            r.findings.push(Finding {
                id: "kernel:preemption".into(),
                severity: Severity::Suggestion,
                title: "A program woken while the processors are busy waits for the kernel's tick".into(),
                detail: format!(
                    "Linux here preempts lazily ({}): a program woken while every processor is busy (a build, an export) isn't switched to until the kernel's next tick, up to a millisecond later, at each step of what it does for you. Told to preempt fully, the kernel switches at once: with every processor computing at low priority, 99 in 100 wakes ran within 4 µs where they took 950, and the work itself got as much done (within 1% computing, writing and passing messages). Olo's window server is realtime and runs at once either way; this is for the programs it shows. The fix starts the kernel with preempt=full from the next restart{}.",
                    p.now(),
                    if p.live { ", and switches the running kernel now" } else { " (the running kernel is locked down, so it can't be switched until then)" }
                ),
                fix: Some(Fix { label: "Preempt at Once".into(), root: true, steps }),
            });
        }
    }

    // Short turns on the processors.
    if let Some(t) = tuning::turns(&places) {
        let steps = crate::kernel::steps(Setting::ShortTurns, true, &places);
        let ms = |ns: u64| format!("{} ms", ns as f64 / 1e6);
        if t.on() {
            r.fine.push(Fine {
                title: "A busy program's turn on a processor is short".into(),
                detail: format!("{}, now and at every start (the kernel comes with {} here).", ms(t.now_ns()), ms(t.built_ns)),
            });
        } else if steps.is_empty() {
            r.fine.push(Fine {
                title: "A busy program's turn on a processor is short from the next restart".into(),
                detail: format!("{} is set for every start; the running kernel is on {} until then (it's locked down, so it can't be set while it runs).", ms(tuning::SHORT_TURN_NS), ms(t.now_ns())),
            });
        } else if !scheduled_by_another(&roots.fs) {
            r.findings.push(Finding {
                id: "kernel:short-turns".into(),
                severity: Severity::Advice,
                title: "Under load, what you're waiting for waits its turn behind busy programs".into(),
                detail: format!(
                    "A busy program keeps a processor for {} here before one that is waiting gets it: Linux lengthens that turn with the number of processors, from {}. Whatever is woken while every processor is busy waits out a turn, at each step of what it does: a window drawing, the Terminal passing a key on, and the kernel's own workers that hand every frame to the GPU. With turns of {} (measured on a computer of 16 processors under a build), such waits of over a millisecond were six times rarer and the longest a third as long. The fix sets it for every start{}.",
                    ms(t.now_ns()),
                    ms(tuning::SHORT_TURN_NS),
                    ms(tuning::SHORT_TURN_NS),
                    if t.live { ", and now" } else { " (the running kernel is locked down, so it can't be set until then)" }
                ),
                fix: Some(Fix { label: "Keep Turns Short".into(), root: true, steps }),
            });
        }
    }

    // Removable disks.
    let s = tuning::slow_disks(&places);
    if s.can {
        if s.on() {
            r.fine.push(Fine {
                title: "Removable disks' writes are limited".into(),
                detail: format!("{} may wait in memory for each: those here now, and each as it's plugged in.", size(tuning::SLOW_DISK_BYTES)),
            });
        } else {
            r.findings.push(Finding {
                id: "kernel:slow-disks".into(),
                severity: Severity::Suggestion,
                title: "A removable disk's writes pile up in memory".into(),
                detail: format!(
                    "{} written to a USB stick or a card may wait in memory here, as for any disk: a copy seems done at once, and the disk is then written for minutes while Eject waits, or is pulled out with the files not on it. With {} at most waiting for each removable disk, a copy goes at the disk's own pace and what it wrote is on the disk a moment later: of 600 MB copied to a stick that takes 10 MB a second, 280 MB were still in memory when the copy ended and 28 s went by before it could be taken out; limited, 14 MB and 1.5 s. The whole took as long either way. The fix {}installs a rule that limits each as it's plugged in.",
                    if s.may_wait > 0 { format!("Up to {}", size(s.may_wait)) } else { "Gigabytes".to_owned() },
                    size(tuning::SLOW_DISK_BYTES),
                    if s.disks.is_empty() { "" } else { "limits those here now and " }
                ),
                fix: Some(Fix { label: "Limit Removable Disks".into(), root: true, steps: crate::kernel::steps(Setting::SlowDisks, true, &places) }),
            });
        }
    }
}

/// A scheduler loaded over the kernel's own is scheduling, or LAVD is
/// chosen to: it asks the kernel to switch at once whenever it means to,
/// so the kernel's preemption setting changes nothing there (measured
/// with LAVD in the guest: the same wakes lazy and full; kernel/TUNING.md)
/// and isn't suggested.
fn scheduled_by_another(fs: &Path) -> bool {
    use olo_system::scheduler;
    scheduler::look(&scheduler::Places::under(fs)).is_some_and(|s| s.loaded.is_some() || s.chosen == scheduler::Choice::Lavd)
}

/// Intel's full video driver (iHD with every codec), as RPM Fusion's
/// nonfree repository names it, and where it and its kind put theirs
/// (libva looks there before its own folder).
const INTEL_MEDIA: &str = "intel-media-driver";
const FULL_VIDEO_DRIVERS: &[&str] = &["usr/lib64/dri-nonfree/iHD_drv_video.so", "usr/lib64/dri-freeworld/iHD_drv_video.so"];

/// The system's name and version (`/etc/os-release`'s ID, and ID_LIKE's
/// names, and VERSION_ID).
fn os_release(fs: &Path) -> (Vec<String>, Option<String>) {
    let text = read(&fs.join("etc/os-release")).or_else(|| read(&fs.join("usr/lib/os-release"))).unwrap_or_default();
    let value = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(|v| v.trim().trim_matches('"').to_owned());
    let mut ids: Vec<String> = value("ID").into_iter().collect();
    ids.extend(value("ID_LIKE").unwrap_or_default().split_whitespace().map(str::to_owned));
    (ids, value("VERSION_ID"))
}

/// Intel graphics on Fedora: its video driver is Fedora's, which leaves
/// out H.264 and HEVC (their patents), so those decode on the processor
/// or NVIDIA's card rather than the Intel GPU (Olo's player,
/// `olo_vaapi::decode`). RPM Fusion's full driver decodes them: adding
/// its repositories (their own release packages, as RPM Fusion says to)
/// and installing it.
fn intel_video(roots: &Roots, pcis: &[Pci], r: &mut Report) {
    if !pcis.iter().any(|p| p.display() && p.vendor == 0x8086) {
        return;
    }
    let (ids, version) = os_release(&roots.fs);
    if !ids.iter().any(|i| i == "fedora") {
        return;
    }
    if FULL_VIDEO_DRIVERS.iter().any(|p| roots.fs.join(p).exists()) {
        r.fine.push(Fine { title: "Intel graphics decodes every video".into(), detail: "RPM Fusion's full driver for it is installed: H.264, HEVC, VP9 and AV1 play on the Intel GPU.".into() });
        return;
    }
    let what = "Fedora's driver for the Intel graphics leaves out H.264 and HEVC (for their patents), so those videos are decoded by the processor (a whole core for 4K at 60 frames a second) or by waking the NVIDIA card. RPM Fusion's full driver decodes them on the Intel GPU, with a quarter of a core.";
    let atomic = roots.fs.join("run/ostree-booted").exists();
    let found = (roots.packages)(&[INTEL_MEDIA, "rpmfusion-free-release", "rpmfusion-nonfree-release"]);
    let (detail, fix) = match (&found, version, atomic) {
        (_, _, true) | (None, _, _) | (_, None, _) => (format!("{what} Add RPM Fusion's free and nonfree repositories and install {INTEL_MEDIA}, as rpmfusion.org says."), None),
        (Some(packages), Some(version), false) => {
            let mut steps = Vec::new();
            let there = packages.iter().any(|(n, _)| n == INTEL_MEDIA);
            if !there {
                // (A repository added already, as a fix stopped half way
                // leaves it, isn't added again.)
                let added = |kind: &str| packages.iter().any(|(n, installed)| *installed && *n == format!("rpmfusion-{kind}-release"));
                for kind in ["free", "nonfree"].into_iter().filter(|k| !added(k)) {
                    let file = format!("{kind}/fedora/rpmfusion-{kind}-release-{version}.noarch.rpm");
                    steps.push(Step::InstallRepository {
                        name: format!("RPM Fusion ({kind})"),
                        // RPM Fusion's own server, then a mirror (its
                        // redirector's choice: one may be out of date).
                        urls: vec![format!("https://download1.rpmfusion.org/{file}"), format!("https://mirrors.rpmfusion.org/{file}")],
                    });
                }
            }
            steps.push(Step::InstallPackages { names: vec![INTEL_MEDIA.into()] });
            let how = if there {
                format!("The fix installs it ({INTEL_MEDIA}, from RPM Fusion)")
            } else {
                format!("The fix adds RPM Fusion's repositories (their release packages, as rpmfusion.org says to) and installs it ({INTEL_MEDIA})")
            };
            (format!("{what} {how}; videos opened afterwards use it."), Some(Fix { label: "Install the Full Video Driver".into(), root: true, steps }))
        }
    };
    r.findings.push(Finding { id: "video:intel".into(), severity: Severity::Advice, title: "Intel graphics can't decode H.264 and HEVC".into(), detail, fix });
}
/// The firmware's Secure Boot variable (its last byte 1: on).
const SECURE_BOOT_VAR: &str = "sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";

fn read_bytes(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// The packages that bring drivers and firmware: a new kernel (its sound
/// and graphics drivers, its quirks for new laptops), and the firmware
/// they load.
const DRIVER_PACKAGES: &[&str] = &[
    "kernel",
    "kernel-core",
    "kernel-modules",
    "kernel-modules-core",
    "kernel-modules-extra",
    "linux-firmware",
    "linux-firmware-whence",
    "alsa-sof-firmware",
    "cirrus-audio-firmware",
    "intel-audio-firmware",
    "intel-gpu-firmware",
    "intel-vsc-firmware",
    "nvidia-gpu-firmware",
    "amd-gpu-firmware",
    "iwlwifi-mld-firmware",
    "iwlwifi-mvm-firmware",
    "iwlbluetooth-firmware",
];

/// SoundWire parts by maker and part number (`sdw:LINK:UNIQUE:MFG:PART`).
fn soundwire_part(mfg: &str, part: &str) -> Option<&'static str> {
    Some(match (mfg, part) {
        ("01fa", "4243") => "Cirrus Logic CS42L43",
        ("01fa", "3556") => "Cirrus Logic CS35L56",
        ("01fa", "3557") => "Cirrus Logic CS35L57",
        ("01fa", "3563") => "Cirrus Logic CS35L63",
        ("025d", "0711") => "Realtek RT711",
        ("025d", "0712") => "Realtek RT712",
        ("025d", "0722") => "Realtek RT722",
        ("025d", "1316") => "Realtek RT1316",
        ("025d", "1318") => "Realtek RT1318",
        _ => return None,
    })
}

/// Olo's keyring opened at login: programs' secrets (Passwords' key,
/// Chrome's, Wi-Fi's) in olo-secrets' store, its key sealed by the login
/// password (`secrets.login`), which pam_olo_keyring gives at Olo's
/// login screen. Locked is a problem (programs can't read their secrets:
/// unlock it from the notification or Preferences ▸ Security). Nothing of
/// GNOME's: its keyring's PAM module was once what this offered (Ben,
/// 2026-11-15: "we should be 100% independent of gnome").
fn login_keyring(roots: &Roots, r: &mut Report) {
    let Some(status) = secrets_status(roots) else { return };
    let sealed = roots.bin.parent().is_some_and(|local| local.parent().is_some_and(|home| home.join(".config/olo/secrets.login").exists()));
    if status["locked"].as_bool() == Some(true) {
        let why = status["error"].as_str().unwrap_or("the login password wasn't given");
        r.findings.push(Finding {
            id: "keyring:locked".into(),
            severity: Severity::Problem,
            title: "Programs' secrets are locked".into(),
            detail: format!("Programs can't read their sign-ins, keys and Wi-Fi passwords: {why}. Unlock them with your login password from the notification, or Preferences ▸ Security ▸ Passwords ▸ Programs' Secrets."),
            fix: None,
        });
        return;
    }
    if status["serving"].as_bool() != Some(true) {
        return;
    }
    if sealed {
        r.fine.push(Fine {
            title: "Olo's keyring opens when you log in".into(),
            detail: "Programs' secrets are in Olo's keyring, which your login password opens (at Olo's login screen; another asks for it once).".into(),
        });
    } else {
        r.findings.push(Finding {
            id: "keyring:unsealed".into(),
            severity: Severity::Advice,
            title: "Olo's keyring isn't opened by your login yet".into(),
            detail:
                "It's open now, but its key isn't sealed by your login password, so the next login asks for it. olo-secrets asks once to seal it when it starts again (an upgrade, or the next login)."
                    .into(),
            fix: None,
        });
    }
}

/// olo-secrets' status file (`$XDG_RUNTIME_DIR/olo-secrets.json`).
fn secrets_status(roots: &Roots) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(roots.fs.join(format!("run/user/{}/olo-secrets.json", roots.uid))).ok()?).ok()
}

/// How many login keyrings a status lists (`login`, and a second "Login").
fn login_keyrings(status: &serde_json::Value) -> usize {
    status["collections"].as_array().map_or(0, |cs| cs.iter().filter(|c| c["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case("login"))).count())
}

/// Programs' secrets kept in one login keyring. Olo's own (olo-secrets,
/// answering for GNOME Keyring) mends itself each time it starts, as an
/// upgrade starts it: a second login keyring ("Login", which Olo's own
/// client made beside `login` on a computer with no default alias,
/// 2026-11-04) merged in, an item there twice kept once. Said when it did
/// that; found if it still has two (the fix starts it again). Two, gh
/// (go-keyring, which reads `login`) lost its token to the other (Ben,
/// 2026-11-06).
fn secrets_store(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let status = secrets_status(roots);
    if let Some(v) = status.as_ref().filter(|v| v["serving"].as_bool() == Some(true)) {
        if login_keyrings(v) > 1 {
            let pid = procs.iter().find(|p| p.uid == roots.uid && p.name == "olo-secrets").map(|p| p.pid);
            r.findings.push(Finding {
                id: "secrets:two-logins".into(),
                severity: Severity::Problem,
                title: "Olo's keyring has two login keyrings".into(),
                detail: "Programs that keep their secrets in the login keyring don't all look in the same one: gh (through go-keyring) reads “login”, others the default, “Login”, so gh can lose its token. olo-secrets merges them, each secret kept once (the newest), as it starts; the fix starts it again (its programs carry on).".into(),
                fix: pid.map(|pid| Fix { label: "Merge Them".into(), root: false, steps: vec![Step::Signal { pid, signal: libc::SIGTERM }] }),
            });
            return;
        }
        let detail = match v.get("healed") {
            Some(h) => {
                let merged = h["merged"].as_array().map_or(0, Vec::len);
                let twice = h["duplicates"].as_u64().unwrap_or(0);
                let mut said = Vec::new();
                if merged > 0 {
                    said.push("a second login keyring merged in".to_owned());
                }
                if twice > 0 {
                    said.push(format!("{twice} secret{} kept twice now kept once, the newest", if twice == 1 { "" } else { "s" }));
                }
                if said.is_empty() {
                    said.push("its items given GNOME's schema".into());
                }
                format!("Olo's keyring mended itself as it started: {}.", said.join("; "))
            }
            None => "Olo's keyring keeps programs' secrets in one login keyring, as GNOME Keyring would.".into(),
        };
        r.fine.push(Fine { title: "Programs' secrets are in one login keyring".into(), detail });
        return;
    }
}

/// A human presence sensor (a ThinkPad's "prox"), and Olo's rule that
/// gives the session its buffer (Preferences ▸ Power ▸ Presence).
/// The power button's "Restart Immediately" and "Shut Down Immediately"
/// (a hold, by default ten seconds' shuts down at once): done by Olo's
/// helper unit, which only root can install. Its unit is what's checked:
/// polkit's rules folder isn't readable by the person, and the fix writes
/// both.
fn power_now(roots: &Roots, r: &mut Report) {
    let input = roots.fs.join("sys/class/input");
    let button = std::fs::read_dir(&input).into_iter().flatten().flatten().any(|e| read(&e.path().join("name")).is_some_and(|n| n.trim() == "Power Button"));
    if !button {
        return;
    }
    let unit = roots.fs.join(olo_protocol::power_button::HELPER_UNIT_PATH.trim_start_matches('/'));
    let installed = std::fs::read_to_string(&unit).ok();
    if installed.as_deref() == Some(POWER_NOW_UNIT) {
        r.fine.push(Fine { title: "The power button can restart and shut down at once".into(), detail: "Olo's helper is installed (olo-power-now@.service).".into() });
        return;
    }
    r.findings.push(Finding {
        id: "power-now".into(),
        severity: Severity::Advice,
        title: if installed.is_some() { "Olo's power button helper is out of date".into() } else { "The power button can't restart or shut down at once".into() },
        detail: "Held, the power button can restart or shut the computer down immediately (Preferences ▸ Keyboard: by default, held ten seconds): every program ended and the disks written out, without waiting for the orderly stop. That takes a small system helper, which only the administrator can install; without it, Immediately does what Restart and Shut Down do. The fix installs olo-power-now@.service and a polkit rule that lets the person at the computer start it, nothing else.".into(),
        fix: Some(Fix {
            label: if installed.is_some() { "Update the Helper".into() } else { "Install the Helper".into() },
            root: true,
            steps: vec![
                Step::WriteFile { path: PathBuf::from(olo_protocol::power_button::HELPER_UNIT_PATH), content: POWER_NOW_UNIT.into(), mode: 0o644 },
                Step::WriteFile { path: PathBuf::from(olo_protocol::power_button::HELPER_RULES_PATH), content: POWER_NOW_RULES.into(), mode: 0o644 },
                Step::Unit { name: "olo-power-now@.service".into(), action: UnitAction::Reload },
            ],
        }),
    });
}

/// What Olo's power mode follower does (`olo-gpu-tune follow`), said in
/// each finding about it.
const FOLLOWER_DOES: &str = "Olo applies Battery Saver, Adaptive Performance and Low Latency to the processors, graphics and cooling. It restores the CPU floor and lets an unused discrete GPU sleep.";

/// The power modes as Olo plans them (Preferences ▸ Power ▸ Power
/// Modes; Ben, 2026-11-13: "olo doctor should make sure those profiles
/// get set/customized properly"): the follower (`olo-gpu-tune follow`,
/// root) installed, of this version (`gpu_tuning::FOLLOWER_VERSION`, as
/// what it last did says: not every build, which would ask for an
/// administrator's password at every upgrade), running, everything it
/// set set, and the processors still as it set them. Only with a power
/// profiles daemon (tuned-ppd, power-profiles-daemon) and processors
/// that can be set.
fn power_modes(roots: &Roots, r: &mut Report) {
    use olo_system::{
        cpu_tuning::{CpuCaps, CpuPreset, Sys},
        gpu_tuning::{self as g, Followed},
        power_profile::Mode,
    };
    let cpu = roots.fs.join("sys/devices/system/cpu");
    if !cpu.join("cpufreq").is_dir() {
        return;
    }
    if !["tuned-ppd.service", "power-profiles-daemon.service"].iter().any(|u| (roots.units)(u).is_ok_and(|s| s.active)) {
        return;
    }
    // (A build tree's: nothing to install from.)
    let from = roots.bin.join("olo-gpu-tune");
    if !from.is_file() {
        return;
    }
    let at = |p: &str| roots.fs.join(p.trim_start_matches('/'));
    let installed = at(g::FOLLOWER_UNIT_PATH).is_file() && at(g::FOLLOWER_PROGRAM).is_file();
    let running = (roots.units)(g::FOLLOWER_SERVICE).is_ok_and(|s| s.active);
    let followed = Followed::load(&roots.fs);
    let current = followed.as_ref().is_some_and(|f| f.version >= g::FOLLOWER_VERSION);
    let same_program = std::fs::read(&from).ok().is_some_and(|a| std::fs::read(at(g::FOLLOWER_PROGRAM)).ok().is_some_and(|b| a == b));
    // Installing or updating: the program (where only root can change it),
    // the unit and rule where they differ, then systemd told and the
    // follower started on it (which sets everything for the mode now).
    let install = || {
        let mut steps = Vec::new();
        if !same_program {
            steps.push(Step::CopyFile { from: from.clone(), to: PathBuf::from(g::FOLLOWER_PROGRAM), mode: 0o755 });
            steps.push(Step::SelinuxLabels { rules: Vec::new(), restore: vec![PathBuf::from(g::FOLLOWER_PROGRAM)] });
        }
        if std::fs::read_to_string(at(g::FOLLOWER_UNIT_PATH)).ok().as_deref() != Some(g::FOLLOWER_UNIT) {
            steps.push(Step::WriteFile { path: PathBuf::from(g::FOLLOWER_UNIT_PATH), content: g::FOLLOWER_UNIT.into(), mode: 0o644 });
        }
        if std::fs::read_to_string(at(g::FOLLOWER_RULE_PATH)).ok().as_deref() != Some(g::FOLLOWER_RULE) {
            steps.push(Step::WriteFile { path: PathBuf::from(g::FOLLOWER_RULE_PATH), content: g::FOLLOWER_RULE.into(), mode: 0o644 });
        }
        if let Ok(conf) = std::fs::read_to_string(at(g::TUNED_PPD)) {
            let mapping = g::laptop_mapping(&conf);
            if (mapping.is_some() || conf.lines().any(|l| l.trim() == "performance=olo-low-latency")) && std::fs::read_to_string(at(g::TUNED_LATENCY)).ok().as_deref() != Some(g::LATENCY_PROFILE) {
                steps.push(Step::WriteFile { path: g::TUNED_LATENCY.into(), content: g::LATENCY_PROFILE.into(), mode: 0o644 });
            }
            if let Some(content) = mapping {
                steps.push(Step::WriteFile { path: g::TUNED_PPD.into(), content, mode: 0o644 });
                steps.push(Step::Unit { name: "tuned-ppd.service".into(), action: UnitAction::Restart });
            }
        }
        let places = if roots.fs == std::path::Path::new("/") { olo_system::fans::Places::here() } else { olo_system::fans::Places::under(&roots.fs) };
        if places.fan().is_file() && roots.bin.join(crate::fans::PROGRAM).is_file() {
            steps.extend(crate::fans::steps_with(olo_system::fans::Profile::Cool, &roots.bin, &places, &*roots.units));
        }
        for action in [UnitAction::Reload, UnitAction::Enable, UnitAction::Restart] {
            steps.push(Step::Unit { name: g::FOLLOWER_SERVICE.into(), action });
        }
        steps
    };
    let again = || vec![Step::Unit { name: g::FOLLOWER_SERVICE.into(), action: UnitAction::Restart }];
    let fix = |label: &str, steps: Vec<Step>| Some(Fix { label: label.into(), root: true, steps });
    if !installed {
        r.findings.push(Finding {
            id: "power-modes:missing".into(),
            severity: Severity::Advice,
            title: "The power modes aren't set as Olo plans them".into(),
            detail: format!(
                "{FOLLOWER_DOES} It isn't installed, so the system power profiles daemon is managing the hardware. The fix installs it: olo-gpu-follow.service, running {} as root.",
                g::FOLLOWER_PROGRAM
            ),
            fix: fix("Install It", install()),
        });
        return;
    }
    if !running {
        r.findings.push(Finding {
            id: "power-modes:stopped".into(),
            severity: Severity::Problem,
            title: "Olo's power mode follower isn't running".into(),
            detail: format!(
                "{FOLLOWER_DOES} It's installed but not running, so the mode's settings are the daemon's own. The fix starts it{}.",
                if same_program { "" } else { ", from the installed version of Olo" }
            ),
            fix: fix("Start It", if same_program { again() } else { install() }),
        });
        return;
    }
    let Some(f) = followed.filter(|_| current) else {
        r.findings.push(Finding {
            id: "power-modes:older".into(),
            severity: Severity::Advice,
            title: "Olo's power mode follower is out of date".into(),
            detail: format!("{FOLLOWER_DOES} The running version predates adaptive power modes. The fix brings it up to date and starts it again."),
            fix: fix("Update It", install()),
        });
        return;
    };
    let mode = Mode::from_id(&f.mode).map_or(f.mode.clone(), |m| m.label().to_owned());
    let when = format!("{mode}{}", if f.on_battery { " on battery" } else { "" });
    if !f.failed.is_empty() {
        r.findings.push(Finding {
            id: "power-modes:failed".into(),
            severity: Severity::Advice,
            title: "Some of the power mode's settings couldn't be set".into(),
            detail: format!("Setting what {when} plans, Olo's power mode follower was refused: {}. The fix tries again.", f.failed.join("; ")),
            fix: fix("Try Again", again()),
        });
        return;
    }
    // The processors as they are against as it left them (a moment after
    // a change, the follower may still be on its way).
    let settled = std::fs::metadata(Followed::path(&roots.fs)).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e.as_secs() >= 10));
    let now = Sys { cpu, firmware: roots.fs.join("sys/firmware/acpi") }.caps();
    if settled && let (Some(was), Some(now)) = (&f.cpu_after, &now) {
        let said = |c: &CpuCaps| {
            [
                ("governor", c.governor.clone()),
                ("energy preference", c.preference.clone()),
                ("turbo", if c.turbo { "on" } else { "off" }.to_owned()),
                ("platform profile", c.platform.clone()),
                ("minimum performance", c.floor.map(|t| format!("{t}%")).unwrap_or_default()),
                ("top speed", c.top.map(|t| format!("{t}%")).unwrap_or_default()),
            ]
        };
        let changed: Vec<String> = said(now).into_iter().zip(said(was)).filter(|(n, w)| n.1 != w.1).map(|((what, n), (_, w))| format!("the {what} {n} (Olo set {w})")).collect();
        if !changed.is_empty() {
            r.findings.push(Finding {
                id: "power-modes:changed".into(),
                severity: Severity::Advice,
                title: "Something changed the processors' settings since Olo set them".into(),
                detail: format!(
                    "For {when}, the processors now have {}: another program (a tuning tool, a script) changed them, so they aren't as Preferences ▸ Power plans. The fix sets them again.",
                    changed.join(", ")
                ),
                fix: fix("Set Them Again", again()),
            });
            return;
        }
    }
    let preset = CpuPreset::parse(&f.cpu).map_or(f.cpu.clone(), |p| p.name().to_owned());
    r.fine.push(Fine { title: "The power modes are set as Olo plans them".into(), detail: format!("Now {when}: the processors {preset}, at up to {}% of their top speed (olo-gpu-follow).", f.top) });
}

fn presence_rule(roots: &Roots, r: &mut Report) {
    let iio = roots.fs.join("sys/bus/iio/devices");
    let Some(sensor) = std::fs::read_dir(&iio).into_iter().flatten().flatten().map(|e| e.path()).find(|p| read(&p.join("name")).is_some_and(|n| n.trim() == "prox")) else { return };
    let installed = std::fs::read_to_string(roots.fs.join(PRESENCE_RULE_PATH)).ok();
    if installed.as_deref() == Some(PRESENCE_RULE) {
        r.fine.push(Fine { title: "The presence sensor can be read".into(), detail: "Olo's rule gives the session its buffer (73-olo-presence.rules).".into() });
        return;
    }
    let name = sensor.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    r.findings.push(Finding {
        id: "rule:presence".into(),
        severity: Severity::Advice,
        title: if installed.is_some() { "Olo's presence sensor rule is out of date".into() } else { "The presence sensor can't be read".into() },
        detail: "This computer can tell when someone is in front of it (its human presence sensor), so Olo can lock the screen when you walk away and wake it when you come back (Preferences ▸ Power). The fix installs /etc/udev/rules.d/73-olo-presence.rules, which gives the session the sensor.".into(),
        fix: Some(Fix {
            label: "Install the Rule".into(),
            root: true,
            steps: vec![
                Step::WriteFile { path: PathBuf::from("/").join(PRESENCE_RULE_PATH), content: PRESENCE_RULE.into(), mode: 0o644 },
                // (A "change" applies it to the sensor already there.)
                Step::WriteSysfs { path: PathBuf::from("/sys/bus/iio/devices").join(&name).join("uevent"), value: "change".into() },
            ],
        }),
    });
}

/// Each FIDO security key plugged in (its hidraw node, found by its
/// report descriptor): one the person can't open can't approve anything
/// (olo-fido, Preferences ▸ Security). systemd's rules give it to the
/// person at the seat (fido_id, uaccess); Olo's rule does where they
/// don't.
fn security_keys(roots: &Roots, r: &mut Report) {
    let hidraw = roots.fs.join("sys/class/hidraw");
    let keys: Vec<(String, String)> = std::fs::read_dir(&hidraw)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| std::fs::read(e.path().join("device/report_descriptor")).is_ok_and(|rd| olo_fido::hid::is_fido(&rd)))
        .map(|e| {
            let node = e.file_name().to_string_lossy().into_owned();
            let name = read(&e.path().join("device/uevent")).and_then(|u| u.lines().find_map(|l| l.strip_prefix("HID_NAME=").map(str::to_owned))).unwrap_or_else(|| node.clone());
            (node, name)
        })
        .collect();
    if keys.is_empty() {
        return;
    }
    let shut: Vec<&(String, String)> = keys.iter().filter(|(node, _)| !can_read_write(&roots.fs.join("dev").join(node))).collect();
    if shut.is_empty() {
        let n = keys.len();
        r.fine.push(Fine {
            title: if n == 1 { "The security key plugged in can be used".into() } else { format!("The {n} security keys plugged in can be used") },
            detail: format!(
                "Its FIDO interface is yours to open, so a touch can approve sudo, Passwords and the lock screen ({}).",
                keys.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ")
            ),
        });
        return;
    }
    let names = shut.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ");
    r.findings.push(Finding {
        id: "rule:security-keys".into(),
        severity: Severity::Advice,
        title: "A security key plugged in can't be used".into(),
        detail: format!(
            "Its FIDO interface isn't yours to open ({names}), so it can't be set up in Preferences ▸ Security or approve anything with a touch. The fix installs /etc/udev/rules.d/74-olo-security-keys.rules, which gives a security key to the person at the computer, as systemd's own rules do elsewhere."
        ),
        fix: Some(Fix {
            label: "Install the Rule".into(),
            root: true,
            steps: std::iter::once(Step::WriteFile { path: PathBuf::from("/").join(SECURITY_KEY_RULE_PATH), content: SECURITY_KEY_RULE.into(), mode: 0o644 })
                // (A "change" applies it to the keys already there.)
                .chain(shut.iter().map(|(node, _)| Step::WriteSysfs { path: PathBuf::from("/sys/class/hidraw").join(node).join("uevent"), value: "change".into() }))
                .collect(),
        }),
    });
}

/// Whether this process may read and write `path`.
fn can_read_write(path: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return false };
    // SAFETY: a valid C string.
    unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
}

/// A built-in HDR panel whose HDR the kernel doesn't drive: Intel's driver
/// uses an eDP panel's own HDR interface (its PQ decoding, its brightness
/// in nits) only if the firmware asks, the EDID has HDR static metadata,
/// or it's started with `enable_dpcd_backlight=3`. The ThinkPad P1 Gen 9's
/// OLED gives its HDR luminance in DisplayID only: HDR on it, without,
/// was a PQ signal it read as SDR. (app_server offers HDR on it only with
/// the argument: `color::kernel_drives_panel_hdr`.)
fn panel_hdr(roots: &Roots, r: &mut Report) {
    let drm = roots.fs.join("sys/class/drm");
    let Some(edp) = std::fs::read_dir(&drm)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains("-eDP-")) && read(&p.join("status")).as_deref() == Some("connected"))
    else {
        return;
    };
    let Some(edid) = std::fs::read(edp.join("edid")).ok().filter(|b| b.len() >= 128) else { return };
    let color = olo_system::edid::color(&edid);
    // An HDR panel (DisplayID's luminance) without HDR in its EDID.
    if color.pq || !color.peak_nits.is_some_and(|n| n >= 400.0) {
        return;
    }
    let card = edp.file_name().map(|n| n.to_string_lossy().split('-').next().unwrap_or_default().to_owned()).unwrap_or_default();
    let driver = std::fs::read_link(drm.join(&card).join("device/driver")).ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    if driver != "xe" && driver != "i915" {
        return;
    }
    let arg = format!("{driver}.enable_dpcd_backlight=3");
    let started_with = read(&roots.fs.join("proc/cmdline")).is_some_and(|c| c.split_whitespace().any(|a| a == arg));
    let peak = color.peak_nits.unwrap_or_default();
    if started_with {
        r.fine.push(Fine {
            title: "The built-in display's HDR is the kernel's to switch".into(),
            detail: format!("Started with {arg}: Preferences ▸ Display ▸ Colour offers HDR, up to {peak:.0} cd/m²."),
        });
        return;
    }
    r.findings.push(Finding {
        id: "boot:panel-hdr".into(),
        severity: Severity::Advice,
        title: "The built-in display's HDR is off".into(),
        detail: format!(
            "This display can show HDR, up to {peak:.0} cd/m², through its own HDR interface, which Linux's Intel driver uses only when started with {arg}. The fix starts it so from the next restart (every boot entry and new kernels too). The driver then sets the display's brightness through that interface as well, in nits; if brightness stops working, remove {arg} from /etc/kernel/cmdline and the entries in /boot/loader/entries."
        ),
        fix: Some(Fix { label: "Turn On HDR at Start".into(), root: true, steps: vec![Step::AddKernelArgument { arg, root: "/".into() }] }),
    });
}

/// Thunderbolt devices plugged in but not let in: on a computer whose
/// Thunderbolt asks first (security "user" or "secure"), a dock or a
/// drive does nothing until it's allowed. boltd allows those it knows
/// (and, with the IOMMU guarding memory, new ones), so one waiting is one
/// it didn't: allowed here (the kernel's `authorized`, root).
fn thunderbolt(roots: &Roots, r: &mut Report) {
    let dir = roots.fs.join("sys/bus/thunderbolt/devices");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut waiting = Vec::new();
    let mut allowed = 0;
    for e in entries.flatten() {
        let d = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        // (Devices, not domains, the hosts ("0-0") or their ports.)
        if name.starts_with("domain") || name.ends_with("-0") || name.contains(':') || !d.join("authorized").exists() {
            continue;
        }
        let what = [read(&d.join("vendor_name")), read(&d.join("device_name"))].into_iter().flatten().map(|s| s.trim().to_owned()).collect::<Vec<_>>().join(" ");
        match read(&d.join("authorized")).as_deref().map(str::trim) {
            Some("0") => waiting.push((name, if what.is_empty() { "A Thunderbolt device".into() } else { what })),
            _ => allowed += 1,
        }
    }
    if waiting.is_empty() {
        if allowed > 0 {
            r.fine.push(Fine { title: "Thunderbolt devices are let in".into(), detail: format!("{allowed} connected, each allowed.") });
        }
        return;
    }
    let names: Vec<String> = waiting.iter().map(|(_, w)| w.clone()).collect();
    // (Remembered, so it's let in whenever it comes: allowed by the
    // kernel's `authorized` alone it waited again at its next plugging
    // in and at every start.)
    let steps = waiting.iter().map(|(n, _)| Step::AllowThunderbolt { device: n.clone(), uid: read(&dir.join(n).join("unique_id")).map(|u| u.trim().to_owned()).unwrap_or_default() }).collect();
    r.findings.push(Finding {
        id: format!("thunderbolt:{}", waiting.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(",")),
        severity: Severity::Problem,
        title: format!("{} is plugged in but not allowed", names.join(", ")),
        detail: "This computer's Thunderbolt lets a device in only once it's allowed, and it hasn't been: until then a dock, a display or a drive on it does nothing. Allow it if you know it: it's remembered, and let in whenever it's plugged in from then on.".into(),
        fix: Some(Fix { label: "Allow It".into(), root: true, steps }),
    });
}

/// Intel's Thunderbolt 5 controller (Barlow Ridge) has a USB controller
/// of its own (8086:5782) for its USB-C ports. A USB 2 device in such a
/// port (Ben's YubiKey, 2026-11-04) wasn't seen at all, the kernel hearing
/// nothing, while USB 3 drives were and the key worked at once in another
/// port: so said, with what to do meanwhile. Whether the controller,
/// asleep, misses a USB 2 plug is still to find; the fix keeps it awake,
/// by a rule so it's awake at every start (it was a value written for
/// the day, and the finding came back with each restart: Ben,
/// 2026-11-10, "olo should just set ephemeral stuff like that"), and
/// the way back takes the rule away.
fn thunderbolt_usb2(roots: &Roots, pcis: &[Pci], r: &mut Report) {
    let Some(xhci) = pcis.iter().find(|p| p.vendor == 0x8086 && read(&p.dir.join("device")).is_some_and(|d| d.trim().trim_start_matches("0x").eq_ignore_ascii_case("5782"))) else { return };
    // Its root hubs (usbN), kept awake with it.
    let mut hubs: Vec<String> = std::fs::read_dir(&xhci.dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().filter(|n| n.starts_with("usb") && n[3..].parse::<u32>().is_ok()).map(str::to_owned))
        .collect();
    hubs.sort();
    let sys = PathBuf::from("/sys/bus/pci/devices").join(&xhci.address);
    let rule = std::fs::read_to_string(roots.fs.join(THUNDERBOLT_USB_RULE_PATH)).ok().as_deref() == Some(THUNDERBOLT_USB_RULE);
    let power = |value: &str| -> Vec<Step> {
        std::iter::once(sys.join("power/control")).chain(hubs.iter().map(|h| sys.join(h).join("power/control"))).map(|path| Step::WriteSysfs { path, value: value.into() }).collect()
    };
    if rule && xhci.power_control.as_deref() == Some("on") {
        let title = "The Thunderbolt 5 ports' USB is kept awake";
        r.fine.push(Fine {
            title: title.into(),
            detail: "At every start (75-olo-thunderbolt-usb.rules). Plug a USB 2 device (a security key, a keyboard) into a Thunderbolt 5 port: if it's seen now, its controller was missing the plug while asleep.".into(),
        });
        // The way back: the rule away, and asleep when idle again.
        let mut steps = vec![Step::RemoveFile { path: PathBuf::from("/").join(THUNDERBOLT_USB_RULE_PATH) }];
        steps.extend(power("auto"));
        r.undo.push(Undo { id: format!("usb2:thunderbolt5:{}:undo", xhci.address), title: title.into(), fix: Fix { label: "Let It Sleep".into(), root: true, steps } });
        return;
    }
    // The rule, for every start from now; and awake now.
    let mut steps = Vec::new();
    if !rule {
        steps.push(Step::WriteFile { path: PathBuf::from("/").join(THUNDERBOLT_USB_RULE_PATH), content: THUNDERBOLT_USB_RULE.into(), mode: 0o644 });
    }
    steps.extend(power("on"));
    r.findings.push(Finding {
        id: format!("usb2:thunderbolt5:{}", xhci.address),
        severity: Severity::Advice,
        title: "A USB 2 device in a Thunderbolt 5 port may go unseen".into(),
        detail: "This computer's Thunderbolt 5 ports have a USB controller of their own, and a USB 2 device in one (a security key, a keyboard, a mouse's receiver) wasn't seen at all here, though USB 3 drives were and the same key worked at once in another port. Meanwhile, plug such devices into another USB-C port. To find out whether the controller misses the plug while it sleeps, the fix keeps it awake, now and at every start (a little power; Olo Doctor can let it sleep again): then plug the device in again.".into(),
        fix: Some(Fix { label: "Keep It Awake".into(), root: true, steps }),
    });
}

/// The built-in sound chips found, but no sound card made of them: the
/// kernel's driver (Intel's SOF) doesn't know how this computer wires
/// them (a new laptop's SoundWire codecs).
fn builtin_sound(roots: &Roots, pcis: &[Pci], r: &mut Report) {
    // The computer's own audio controller (not a graphics card's HDMI).
    let Some(dsp) = pcis.iter().find(|p| p.class >> 8 == 0x0401 || (p.class >> 8 == 0x0403 && p.driver.as_deref().is_some_and(|d| d.starts_with("sof-audio")))) else { return };
    if !dsp.driver.as_deref().is_some_and(|d| d.starts_with("sof-audio")) {
        return;
    }
    // A sound card made from it?
    let has_card = std::fs::read_dir(roots.fs.join("sys/class/sound")).into_iter().flatten().flatten().any(|e| {
        e.file_name().to_string_lossy().starts_with("card")
            && std::fs::canonicalize(e.path().join("device")).is_ok_and(|d| d.to_string_lossy().contains(&dsp.address) || d.to_string_lossy().contains("sof"))
    });
    if has_card {
        return;
    }
    let mut parts: Vec<&str> = std::fs::read_dir(roots.fs.join("sys/bus/soundwire/devices"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let f: Vec<&str> = name.split(':').collect();
            (f.len() >= 5).then(|| soundwire_part(f[3], f[4])).flatten()
        })
        .collect();
    let amps = parts.iter().filter(|p| p.contains("CS35") || p.contains("RT13")).count();
    parts.sort();
    parts.dedup();
    let chips =
        if parts.is_empty() { "its sound chips".to_string() } else { format!("its sound chips ({}{})", parts.join(", "), if amps > 1 { format!("; {amps} amplifiers") } else { String::new() }) };
    // A newer kernel and firmware waiting: they may know it.
    let waiting: Vec<String> = (roots.updates)().unwrap_or_default().into_iter().filter(|n| DRIVER_PACKAGES.contains(&n.as_str())).collect();
    let kernel = waiting.iter().any(|n| n.starts_with("kernel"));
    let (then, fix) = if waiting.is_empty() {
        ("It needs a newer kernel (and linux-firmware with this computer's speaker tuning) than there is yet: Olo Doctor offers it as soon as one comes.".to_string(), None)
    } else {
        (
            format!(
                "A newer {} waiting, which may know it: the fix installs {}{}.",
                if kernel { "kernel and firmware are" } else { "firmware is" },
                if waiting.len() > 3 { format!("{} and {} more", waiting[..3].join(", "), waiting.len() - 3) } else { waiting.join(", ") },
                if kernel { "; restart afterwards to use the new kernel" } else { "" }
            ),
            Some(Fix { label: "Install Driver Updates".into(), root: true, steps: vec![Step::UpdatePackages { names: waiting.clone() }] }),
        )
    };
    r.findings.push(Finding {
        id: "sound:builtin".into(),
        // (No sound at all, nor calls: critical.)
        severity: Severity::Critical,
        title: "The built-in speakers and microphones have no driver".into(),
        detail: format!(
            "Linux finds {chips} but doesn't know how this computer wires them, so it makes no sound card of them: no speakers, microphones or headphone jack. Sound plays through Bluetooth, USB and HDMI meanwhile. {then}"
        ),
        fix,
    });
}

/// Whether process `pid` is finishing what it has under way before it
/// becomes the new version (olo_kit_base::restore's `mark_waiting`: Token
/// Pool, handed over to a copy of the new version, streaming the answers
/// it has), which olo-session leaves to it too.
/// (A marker older than the process is another's, left by one that
/// ended before it could take it away, whose pid this one has again.)
fn finishing(roots: &Roots, pid: i32) -> bool {
    let Ok(marked) = std::fs::metadata(roots.temp.runtime.join("olo-upgrade-waits").join(pid.to_string())) else { return false };
    let marked_ago = marked.modified().ok().and_then(|t| t.elapsed().ok()).map_or(0.0, |d| d.as_secs_f64());
    age(&roots.fs, pid).is_none_or(|running| marked_ago <= running + 1.0)
}

/// Olo's programs still on a version an upgrade replaced, but not one
/// finishing what it has before it goes (`finishing`): Quit Them stopped
/// Token Pool's copy the session follows while it streamed its last
/// answers, its successor left to run on alone, and the session, starting
/// it again, ended up with a second Token Pool with no gateway (Ben,
/// 2026-11-07). Nor one another program started
/// (`olo_system::procs::started_by_another`: a command in a shell, an
/// agent's tool), which isn't the session's to start again and ends with
/// what it's doing: an upgrade leaves those too, and Quit Them was
/// offered for a script an agent ran through olo-sudo and five
/// `olo-screen` commands another agent had waiting (Ben, 2026-11-18: "i
/// want to make sure that … it's not offering to terminate legit
/// processes that should continue post upgrade"). Those are said among
/// what's fine, so it's plain why they're still the old version.
fn stale(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let me = std::process::id() as i32;
    let bin = roots.bin.to_string_lossy();
    let (theirs, old): (Vec<(&Proc, Option<String>)>, Vec<(&Proc, Option<String>)>) = procs
        .iter()
        .filter(|p| p.uid == roots.uid && p.pid != me)
        .filter(|p| p.exe.as_deref().is_some_and(|e| e.ends_with(" (deleted)") && e.starts_with(&*bin)))
        .filter(|p| !NEVER_STOPPED.contains(&p.name.as_str()))
        .filter(|p| !on_a_screen(&roots.fs, p.pid))
        .filter(|p| !finishing(roots, p.pid))
        .map(|p| (p, olo_system::procs::started_by_another_in(&roots.fs, p.pid)))
        .partition(|(_, by)| by.is_some());
    let old: Vec<&Proc> = old.into_iter().map(|(p, _)| p).collect();
    if old.is_empty() {
        let detail = if theirs.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = theirs.iter().map(|(p, by)| format!("{} ({}), started by {}", p.name, p.pid, by.as_deref().unwrap_or_default())).collect();
            let what = if theirs.len() == 1 { "a command from before the upgrade, which ends with what it's doing" } else { "commands from before the upgrade, which end with what they're doing" };
            format!("{}: {what}; the next one started is the current version.", names.join("; "))
        };
        r.fine.push(Fine { title: "Every Olo program runs the installed version".into(), detail });
        return;
    }
    let names: Vec<String> = old.iter().map(|p| format!("{} ({})", p.name, p.pid)).collect();
    r.findings.push(Finding {
        id: "stale".into(),
        severity: Severity::Advice,
        title: format!("{} still running an older version", plural(old.len(), "Olo program is", "Olo programs are")),
        detail: format!("{}: each runs a program file an upgrade has since replaced. Quitting them lets the session start the current version.", names.join(", ")),
        fix: Some(Fix { label: "Quit Them".into(), root: false, steps: old.iter().map(|p| Step::Signal { pid: p.pid, signal: libc::SIGTERM }).collect() }),
    });
}

/// How long ago process `pid` started, in seconds, as /proc says (None
/// if it doesn't).
fn age(fs: &Path, pid: i32) -> Option<f64> {
    let stat = std::fs::read_to_string(fs.join("proc").join(pid.to_string()).join("stat")).ok()?;
    let started: f64 = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()?;
    let uptime: f64 = std::fs::read_to_string(fs.join("proc/uptime")).ok()?.split_whitespace().next()?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    Some(uptime - started / hz)
}

/// Token Pool, as its processes and its gateway's port say. A copy of it
/// serving nothing beside the one that serves: the session's, started
/// while another held the port (which ran on with no gateway, its window
/// showing no requests, and took Token Pool's launches: Ben, 2026-11-09)
/// is found and quit (the session starts it again, and it keeps the one
/// serving: olo-pool's `keeper`). The gateway's port held by no Token
/// Pool: by another program (said), or by none (Token Pool started
/// again). Else, which serves, as fine. The keeper, a copy finishing
/// what it has after handing over, a launch on its way (younger than 15
/// s) and the command line's own (`--status`) are as they should be.
fn token_pool(roots: &Roots, procs: &[Proc], r: &mut Report) {
    const COMMANDS: &[&str] = &["--status", "--clients", "--put-back", "--restart-codex-service", "-h", "--help"];
    let port = roots.pool_port;
    let dir = |pid: i32| roots.fs.join("proc").join(pid.to_string());
    let args = |pid: i32| -> Vec<String> {
        std::fs::read(dir(pid).join("cmdline")).unwrap_or_default().split(|b| *b == 0).skip(1).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
    };
    let environ = |pid: i32| -> Vec<Vec<u8>> { std::fs::read(dir(pid).join("environ")).unwrap_or_default().split(|b| *b == 0).map(<[u8]>::to_vec).collect() };
    let keeper = |pid: i32| environ(pid).iter().any(|v| v == b"OLO_POOL_KEEPER=1");
    // (One on a port of its own is a test's or a nested session's.)
    let its_port = |pid: i32| environ(pid).iter().find_map(|v| std::str::from_utf8(v.strip_prefix(b"OLO_POOL_PORT=")?).ok()?.parse::<u16>().ok()).unwrap_or(POOL_PORT);
    let pools: Vec<&Proc> = procs
        .iter()
        .filter(|p| p.uid == roots.uid && p.name == "olo-pool" && (p.scope.is_none() || p.scope == roots.scope))
        .filter(|p| !on_a_screen(&roots.fs, p.pid) && !args(p.pid).iter().any(|a| COMMANDS.contains(&a.as_str())) && its_port(p.pid) == port)
        .collect();
    if pools.is_empty() {
        // (Not running: turned off, or not this session's to run.)
        return;
    }
    // Token Pool's own first (a few processes' descriptors read); every
    // other program's only when none of them holds the port (a thousand
    // processes: 30 ms).
    let ids: Vec<i32> = pools.iter().map(|p| p.pid).collect();
    let serving = olo_system::listening::holders(&roots.fs, port, |pid| ids.contains(&pid));
    let idle: Vec<i32> = ids.iter().copied().filter(|&pid| !serving.contains(&pid) && !keeper(pid) && !finishing(roots, pid) && age(&roots.fs, pid).is_none_or(|a| a > 15.0)).collect();
    let processes = |pids: &[i32]| -> String {
        match pids {
            [one] => format!("process {one}"),
            [rest @ .., last] => format!("processes {} and {last}", rest.iter().map(i32::to_string).collect::<Vec<_>>().join(", ")),
            [] => String::new(),
        }
    };
    let quit = |label: &str| Fix { label: label.into(), root: false, steps: idle.iter().map(|&pid| Step::Signal { pid, signal: libc::SIGTERM }).collect() };
    if serving.is_empty() {
        let others = olo_system::listening::holders(&roots.fs, port, |pid| !ids.contains(&pid) && procs.iter().any(|p| p.pid == pid && p.uid == roots.uid));
        if let Some(other) = others.iter().find_map(|h| procs.iter().find(|p| p.pid == *h)) {
            r.findings.push(Finding {
                id: "token-pool:port".into(),
                severity: Severity::Problem,
                title: "Another program has Token Pool's port".into(),
                detail: format!(
                    "{} (process {}) listens on port {port}, where Token Pool's gateway does, so Token Pool can't, and Claude Code and Codex can't reach the pool. Quit it, then quit Token Pool from Activity Monitor: the session starts it again on its port.",
                    other.name, other.pid
                ),
                fix: None,
            });
        } else if !idle.is_empty() {
            r.findings.push(Finding {
                id: "token-pool:no-gateway".into(),
                severity: Severity::Problem,
                title: "Token Pool's gateway isn't running".into(),
                detail: format!("Token Pool runs ({}), but nothing listens on port {port}, so Claude Code and Codex can't reach the pool. Starting it again starts its gateway.", processes(&idle)),
                fix: Some(quit("Start It Again")),
            });
        }
        return;
    }
    if !idle.is_empty() {
        r.findings.push(Finding {
            id: "token-pool:twice".into(),
            severity: Severity::Problem,
            title: "Token Pool is running twice".into(),
            detail: format!(
                "Token Pool ({}) serves Claude Code and Codex; another ({}) started beside it and couldn't take its port, so it serves nothing: its window shows no requests, and opening Token Pool may show it. Quitting it fixes this; the session starts it again to look after the one serving.",
                processes(&serving),
                processes(&idle)
            ),
            fix: Some(quit(if idle.len() == 1 { "Quit It" } else { "Quit Them" })),
        });
        return;
    }
    r.fine.push(Fine { title: "Token Pool serves Claude Code and Codex".into(), detail: format!("Its gateway listens on port {port} ({}).", processes(&serving)) });
}

/// Olo's login screen gave the seat to another this boot (olo-login's
/// `hand_over`: its greeter never showed, time after time), which leaves
/// a note in /run/olo-login: said, with what usually causes it.
fn login_screen_handed_over(roots: &Roots, r: &mut Report) {
    let Ok(note) = std::fs::read_to_string(roots.fs.join("run/olo-login/handed-over")) else { return };
    r.findings.push(Finding {
        id: "login:handed-over".into(),
        severity: Severity::Problem,
        title: "Olo's login screen couldn't show this time".into(),
        detail: format!(
            "{} Olo draws with the GPU: a computer (or virtual machine) without one it can render on, or a graphics driver that failed to load, keeps its login screen from showing. It tries again at the next start; tools/install-login.sh --revert makes GDM the login screen for good.",
            note.trim()
        ),
        fix: None,
    });
}

/// Olo's own login screen (login_screen.rs): offered while another is
/// the login screen (Ben, 2026-11-07: "the login screen should come up in
/// olo doctor as something it can do for me"), its copies brought up to
/// date once an upgrade has left them older, and, in use, a way back to
/// the one it took the place of.
fn own_login_screen(roots: &Roots, r: &mut Report) {
    use crate::login_screen as ls;
    // (Not installed: a build tree's, or Olo without its login screen.)
    if !roots.bin.join("olo-login").is_file() || !roots.bin.join("olo-greeter").is_file() {
        return;
    }
    // (An atomic Fedora's /usr is its image's: tools/setup.sh's way there.)
    if roots.fs.join("run/ostree-booted").exists() {
        return;
    }
    let share = roots.bin.parent().map(|p| p.join("share/olo")).unwrap_or_default();
    let dm = ls::display_manager(&roots.fs);
    if dm.as_deref() == Some(ls::SERVICE) {
        let old = ls::stale(&roots.fs, &roots.bin);
        if !old.is_empty() {
            // (Without the module, the password typed at login opens no
            // keyring: programs' secrets stay locked till it's asked for
            // again.)
            let keyring = old.contains(&"pam_olo_keyring.so");
            r.findings.push(Finding {
                id: "login:own-older".into(),
                severity: if keyring { Severity::Problem } else { Severity::Advice },
                title: if keyring { "Logging in doesn't open your keyring".into() } else { "Olo's login screen runs an older version".into() },
                detail: format!(
                    "{}It runs copies of Olo's programs of its own ({}), which an upgrade doesn't replace. The fix copies the installed ones over them, for the next time it shows.",
                    if keyring {
                        "The part of the login screen that opens the secrets programs keep with your login password isn't installed (or is older), so they're asked for again after you log in. "
                    } else {
                        ""
                    },
                    old.join(", ")
                ),
                fix: Some(Fix { label: "Update the Login Screen".into(), root: true, steps: ls::steps(&roots.bin, &share, None) }),
            });
            return;
        }
        let title = "Olo's own login screen greets you";
        r.fine.push(Fine { title: title.into(), detail: "In GDM's place, running the installed version of Olo.".into() });
        // The way back, to the login screen it took the place of.
        let units = |u: &str| ["usr/lib/systemd/system", "etc/systemd/system"].iter().any(|d| roots.fs.join(d).join(u).exists());
        if let Some((other, name)) = [("gdm.service", "GDM"), ("sddm.service", "SDDM")].into_iter().find(|(u, _)| units(u)) {
            r.undo.push(Undo {
                id: "login:own-undo".into(),
                title: title.into(),
                fix: Fix {
                    label: format!("Go Back to {name}"),
                    root: true,
                    steps: vec![Step::Unit { name: ls::SERVICE.into(), action: crate::fix::UnitAction::Disable }, Step::Unit { name: other.into(), action: crate::fix::UnitAction::Enable }],
                },
            });
        }
        return;
    }
    let now = match dm.as_deref() {
        Some("gdm.service") => "GNOME's (GDM)".to_owned(),
        Some(u) => u.trim_end_matches(".service").to_owned(),
        None => "none of Olo's".to_owned(),
    };
    r.findings.push(Finding {
        id: "login:own".into(),
        severity: Severity::Advice,
        title: format!("The login screen is {now}, not Olo's own"),
        detail: "Olo has a login screen of its own: the people on this computer and their pictures, the password, fingerprint and face unlock, in Olo's look; your password there also opens Olo's keyring. The fix makes it the login screen from the next start. The one it replaces stays installed, and Olo Doctor offers to go back to it."
            .into(),
        fix: Some(Fix { label: "Use Olo's Login Screen".into(), root: true, steps: ls::steps(&roots.bin, &share, dm.as_deref()) }),
    });
}

/// The fan profile (fans.rs): Cool, chosen, that isn't what the fans are
/// doing, and a service an upgrade has left older (Ben, 2026-11-12: "why
/// does it say the fan service isn't started in activity monitor?": it
/// was running, and the fans' driver had taken no level since that
/// morning's restart, having been told to only by hand). Nothing where
/// the firmware's own curve is chosen, or the fans can't be set at all.
fn fan_profile(roots: &Roots, r: &mut Report) {
    use olo_system::fans::{Amiss, Places, look};
    let places = if roots.fs == Path::new("/") { Places::here() } else { Places::under(&roots.fs) };
    let Some(state) = look(&places) else { return };
    // (Any of Olo's own curves: Cool, Cooler, Coolest. Only Cool was
    // asked after the profiles became three, and "cool" as kept before
    // reads as Cooler, so the service went unchecked: a test found it,
    // 2026-11-19.)
    if !state.installed || !state.profile.ours() {
        return;
    }
    let chosen = state.profile.label();
    // (A build tree's: nothing to bring it up to date from.)
    if !roots.bin.join(crate::fans::PROGRAM).is_file() {
        return;
    }
    let mut steps = crate::fans::steps_with(state.profile, &roots.bin, &places, &*roots.units);
    let amiss = state.amiss();
    let restarts = |steps: &[Step]| steps.iter().any(|s| matches!(s, Step::Unit { action: UnitAction::Restart, .. }));
    if amiss == Some(Amiss::Stopped) && !restarts(&steps) {
        steps.push(Step::Unit { name: crate::fans::SERVICE.into(), action: UnitAction::Restart });
    }
    let older = steps.iter().any(|s| matches!(s, Step::CopyFile { .. }));
    let firmware = "Until then the fans are the firmware's own, which wait for the heat and fall behind it: the processor is slowed for it under load.";
    let fix = |label: &str, steps: Vec<Step>| Some(Fix { label: label.into(), root: true, steps });
    match amiss {
        Some(Amiss::NextStart) => r.findings.push(Finding {
            id: "fans:next-start".into(),
            severity: Severity::Advice,
            title: format!("The {chosen} fan profile starts with the computer's next start"),
            detail: format!(
                "The fans' driver (thinkpad_acpi) lets them be set only if it's told to as it's loaded, and this time it wasn't. It is told to from the next start on: nothing more is needed. {firmware}{}",
                if steps.is_empty() { "" } else { " The fix brings Olo's fan service up to date meanwhile." }
            ),
            fix: if steps.is_empty() { None } else { fix("Update the Fan Service", steps) },
        }),
        Some(Amiss::Driver) => r.findings.push(Finding {
            id: "fans:driver".into(),
            severity: Severity::Problem,
            title: format!("The {chosen} fan profile isn't in force"),
            detail: format!(
                "The fans' driver (thinkpad_acpi) doesn't let them be set, and nothing tells it to when it's loaded (it may have been told by hand before the computer last started). The fix tells it to at every start (/etc/modprobe.d/olo-fans.conf): {chosen} is in force from the next one. {firmware}"
            ),
            fix: fix("Repair the Fan Profile", steps),
        }),
        Some(Amiss::Stopped) => r.findings.push(Finding {
            id: "fans:stopped".into(),
            severity: Severity::Problem,
            title: format!("The {chosen} fan profile isn't in force"),
            detail: format!("Olo's fan service (olo-fans) isn't running. The fix starts it{}. {firmware}", if older { ", from the installed version of Olo" } else { "" }),
            fix: fix("Start the Fan Service", steps),
        }),
        None if !steps.is_empty() => r.findings.push(Finding {
            id: "fans:older".into(),
            severity: Severity::Advice,
            title: if older { "Olo's fan service runs an older version".into() } else { "Olo's fan service isn't set up as this version sets it up".into() },
            detail: "It runs as the system's, from a copy of its own (/usr/local/libexec/olo/olo-fans) that an upgrade doesn't replace. The fix brings the copy and its settings up to date and starts it again; the fans are the firmware's for the moment that takes.".into(),
            fix: fix("Update the Fan Service", steps),
        }),
        None => r.fine.push(Fine { title: "The fans run ahead of the heat".into(), detail: format!("The {chosen} fan profile is in force: Olo's fan service is running, the installed version.") }),
    }
}

fn login_entry(roots: &Roots, r: &mut Report) {
    let atomic = roots.fs.join("run/ostree-booted").exists();
    let home = if atomic { SESSION_ENTRY_LOCAL_PATH } else { SESSION_ENTRY_PATH };
    // (Found in either place; written in its own.)
    let found = [SESSION_ENTRY_PATH, SESSION_ENTRY_LOCAL_PATH].into_iter().find(|p| roots.fs.join(p).exists()).unwrap_or(home);
    let path = roots.fs.join(found);
    let session = roots.bin.join("olo-session");
    let wanted: String =
        SESSION_ENTRY.lines().filter(|l| !l.starts_with("TryExec=")).map(|l| if l.starts_with("Exec=") { format!("Exec={}", session.display()) } else { l.to_owned() }).collect::<Vec<_>>().join("\n")
            + "\n";
    let installed = std::fs::read_to_string(&path).ok();
    let exec = installed.as_deref().and_then(|t| t.lines().find_map(|l| l.strip_prefix("Exec=")).map(str::to_owned));
    let runs = exec.as_deref().is_some_and(|e| Path::new(e).exists() || !e.contains('/'));
    if installed.is_some() && runs {
        r.fine.push(Fine { title: "Olo is on the login screen".into(), detail: format!("/{found} runs {}.", exec.unwrap_or_default()) });
        return;
    }
    if !session.exists() {
        // Not installed (a build tree): nothing to point the entry at.
        return;
    }
    r.findings.push(Finding {
        id: "login".into(),
        severity: Severity::Advice,
        title: if installed.is_some() { "The login screen's Olo entry runs a program that isn't there".into() } else { "Olo isn't on the login screen".into() },
        detail: format!("/{home} should start {}. The fix writes it.", session.display()),
        fix: Some(Fix { label: "Write the Entry".into(), root: true, steps: vec![Step::WriteFile { path: PathBuf::from("/").join(home), content: wanted, mode: 0o644 }] }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test's folder in /var/tmp, removed when it's dropped (the test
    /// over, passed or not).
    struct Scratch(PathBuf);

    impl std::ops::Deref for Scratch {
        type Target = PathBuf;
        fn deref(&self) -> &PathBuf {
            &self.0
        }
    }

    impl AsRef<Path> for Scratch {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A Bluetooth controller that doesn't answer: a finding whose fix
    /// starts its hardware again (its device and driver, as sysfs has
    /// them); one that answers, none.
    #[test]
    fn a_bluetooth_controller_that_doesnt_answer() {
        let (fs, mut roots) = machine("bluetooth");
        let device = fs.join("sys/devices/pci0000:00/0000:00:14.7");
        let driver = fs.join("sys/bus/pci/drivers/btintel_pcie");
        std::fs::create_dir_all(&device).unwrap();
        std::fs::create_dir_all(&driver).unwrap();
        std::os::unix::fs::symlink(&driver, device.join("driver")).unwrap();
        std::fs::create_dir_all(fs.join("sys/class/bluetooth/hci0")).unwrap();
        std::os::unix::fs::symlink(&device, fs.join("sys/class/bluetooth/hci0/device")).unwrap();
        assert!(scan(&roots).findings.iter().all(|f| !f.id.starts_with("bluetooth-stuck")));
        roots.bluetooth_stuck = vec!["hci0".into()];
        let r = scan(&roots);
        let f = r.findings.iter().find(|f| f.id == "bluetooth-stuck-hci0").expect("found");
        assert_eq!(f.title, "Bluetooth's hardware isn't answering");
        assert!(f.detail.contains("btintel_pcie"), "{}", f.detail);
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root);
        assert_eq!(fix.steps, [Step::RestartBluetooth { device: device.canonicalize().unwrap(), driver: driver.canonicalize().unwrap() }]);
        // (Bluetooth here: the healer is for it too, Wi-Fi chip or not.)
        assert!(r.findings.iter().any(|f| f.id == "wifi-heal") || r.fine.iter().any(|f| f.title.starts_with("Olo's healer watches")), "{:?}", r.findings.iter().map(|f| &f.title).collect::<Vec<_>>());
    }

    /// A machine in a folder: a keyd running from /usr/local, a mouse
    /// receiver allowed to sleep, a Studio Display, no rules, a stale
    /// Deskbar.
    fn machine(name: &str) -> (Scratch, Roots) {
        let fs = Scratch(PathBuf::from(format!("/var/tmp/olo-doctor-test-{}-{name}", std::process::id())));
        let _ = std::fs::remove_dir_all(&fs);
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w("proc/772/cmdline", "/usr/local/bin/keyd\0");
        w("proc/900/cmdline", "/home/me/.local/bin/olo-deskbar\0");
        w("proc/901/cmdline", "gsd-power\0");
        w("proc/901/cgroup", "0::/user.slice/user-1000.slice/session-2.scope\n");
        w("proc/bus/input/devices", "N: Name=\"keyd virtual keyboard\"\nN: Name=\"keyd virtual pointer\"\nN: Name=\"Logitech G309\"\n");
        // BlueZ, started without its experimental interfaces.
        w("proc/903/cmdline", "/usr/libexec/bluetooth/bluetoothd\0");
        // The voice service, its sound thread not realtime (rtkit full).
        w("proc/902/cmdline", "olo-voice\0--service\0");
        w("proc/902/cgroup", "0::/user.slice/user-1000.slice/session-2.scope\n");
        w("proc/902/task/905/comm", "data-loop.0\n");
        w("proc/902/task/905/stat", &format!("905 (data-loop.0) S{}\n", " 0".repeat(40)));
        w("usr/libexec/rtkit-daemon", "");
        w("usr/local/bin/keyd", "");
        w("usr/local/lib/systemd/system/keyd.service", "[Unit]\n");
        // The receiver: device 3-1.3 with an input interface.
        w("sys/bus/usb/devices/3-1.3/manufacturer", "Logitech\n");
        w("sys/bus/usb/devices/3-1.3/product", "USB Receiver\n");
        w("sys/bus/usb/devices/3-1.3/idVendor", "046d\n");
        w("sys/bus/usb/devices/3-1.3/idProduct", "c548\n");
        w("sys/bus/usb/devices/3-1.3/power/control", "auto\n");
        w("sys/bus/usb/devices/3-1.3:1.0/bInterfaceClass", "03\n");
        // A receiver: the mouse paired with it a level down.
        w("sys/bus/usb/devices/3-1.3:1.0/0003:046D:C548.0001/0003:046D:40B1.0002/input/input8/name", "Logitech G309\n");
        // The display: HID interfaces without input nodes, asleep (fine).
        w("sys/bus/usb/devices/3-1.4/manufacturer", "Apple Inc.\n");
        w("sys/bus/usb/devices/3-1.4/product", "Studio Display\n");
        w("sys/bus/usb/devices/3-1.4/idVendor", "05ac\n");
        w("sys/bus/usb/devices/3-1.4/idProduct", "1114\n");
        w("sys/bus/usb/devices/3-1.4/power/control", "auto\n");
        w("sys/bus/usb/devices/3-1.4:1.5/bInterfaceClass", "03\n");
        w("usr/local/share/wayland-sessions/olo.desktop", "[Desktop Entry]\nExec=/nowhere/olo-session\n");
        // A tuning profile's cap on what a thread may ask the scheduler.
        w("proc/sys/kernel/sched_util_clamp_min", "512\n");
        // A Broadcom Wi-Fi chip, and the healer's program installed for
        // the person.
        w("sys/class/net/wld0/phy80211/name", "phy0\n");
        w("sys/bus/pci/drivers/brcmfmac/bind", "");
        std::fs::create_dir_all(fs.join("sys/class/net/wld0/device")).unwrap();
        std::os::unix::fs::symlink(fs.join("sys/bus/pci/drivers/brcmfmac"), fs.join("sys/class/net/wld0/device/driver")).unwrap();
        let bin = fs.join("home/me/.local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("olo-session"), "").unwrap();
        std::fs::write(bin.join("olo-wifi-heal"), "the healer").unwrap();
        // The stale Deskbar: its exe link says deleted.
        std::os::unix::fs::symlink(format!("{}/olo-deskbar (deleted)", bin.display()), fs.join("proc/900/exe")).unwrap();
        std::os::unix::fs::symlink(bin.join("olo-doctor"), fs.join("proc/901/exe")).unwrap();
        let units = Box::new(|name: &str| -> Result<UnitState, String> {
            Ok(match name {
                "keyd.service" => UnitState { loaded: true, active: true, enabled: true, path: Some("/usr/local/lib/systemd/system/keyd.service".into()) },
                "rtkit-daemon.service" => UnitState { loaded: true, active: false, enabled: true, path: Some("/usr/lib/systemd/system/rtkit-daemon.service".into()) },
                _ => UnitState::default(),
            })
        });
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let roots = Roots {
            fs: fs.clone(),
            bin,
            uid,
            scope: Some("session-2.scope".into()),
            units,
            airpods: true,
            phone: false,
            iphone: false,
            phone_connected: false,
            bluetooth_class: None,
            bluetooth_stuck: Vec::new(),
            packages: Box::new(|_| None),
            updates: Box::new(|| None),
            package_services: Box::new(Vec::new),
            temp: TempFiles::default(),
            pool_port: POOL_PORT,
            kernels_bbr: None,
        };
        (fs, roots)
    }

    /// The session's division: a finding with the drop-in to write where
    /// it isn't written; fine, and said to be for the next login, once
    /// it is; fine in a session that is divided.
    #[test]
    fn a_session_is_divided_once_the_drop_in_is_there_and_it_is_started_again() {
        use olo_system::session_groups::{DELEGATE_CONF, DELEGATE_CONF_TEXT};
        let (fs, roots) = machine("session-groups");
        let mut r = Report::default();
        session_groups(&roots, &mut r);
        let f = r.findings.iter().find(|f| f.id == "session-groups").unwrap();
        let fix = f.fix.as_ref().unwrap();
        assert!(
            fix.root
                && matches!(fix.steps.as_slice(), [Step::WriteFile { path, content, mode: 0o644 }] if path == Path::new("/etc/systemd/system/session-.scope.d/50-olo-groups.conf") && content == DELEGATE_CONF_TEXT)
        );
        assert!(DELEGATE_CONF_TEXT.contains("[Scope]\nDelegate=cpu memory\n"));
        // Written: nothing to fix, and said to be for sessions started after.
        std::fs::create_dir_all(fs.join(DELEGATE_CONF).parent().unwrap()).unwrap();
        std::fs::write(fs.join(DELEGATE_CONF), DELEGATE_CONF_TEXT).unwrap();
        let mut r = Report::default();
        session_groups(&roots, &mut r);
        assert!(r.findings.is_empty());
        assert!(r.fine.iter().any(|f| f.title == "The session delegation setting is installed"));
        // A session Olo's login divided.
        let bg = fs.join(format!("sys/fs/cgroup/user.slice/user-{}.slice/session-2.scope/olo/bg", roots.uid));
        std::fs::create_dir_all(&bg).unwrap();
        std::fs::write(bg.join("memory.high"), "max\n").unwrap();
        // (The groups there but the session's programs not in the
        // desktop's: not divided.)
        let mut r = Report::default();
        session_groups(&roots, &mut r);
        assert!(!r.fine.iter().any(|f| f.title == "The desktop and background work are kept apart"));
        std::fs::create_dir_all(bg.with_file_name("desk")).unwrap();
        std::fs::write(bg.with_file_name("desk").join("cgroup.procs"), "3872\n3918\n").unwrap();
        let mut r = Report::default();
        session_groups(&roots, &mut r);
        assert!(r.findings.is_empty());
        assert!(r.fine.iter().any(|f| f.title == "The desktop and background work are kept apart"));
    }

    /// Olo's guard for the embedded controller's events is suggested where
    /// there is a controller and the kernel can take it (what building
    /// it needs installed first, where missing); fine once it's in and
    /// its service kept; and not offered with no controller, on a kernel
    /// series the driver wasn't checked in, or without its program.
    #[test]
    fn the_ec_guard_is_suggested_then_fine() {
        let (fs, roots) = machine("ec-guard");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w("proc/sys/kernel/osrelease", "7.2.15-300.fc45.x86_64\n");
        std::fs::write(roots.bin.join("olo-ec-storm"), "the service").unwrap();
        // No embedded controller (a desktop): nothing said.
        assert!(scan(&roots).find("ec-guard").is_none());
        std::fs::create_dir_all(fs.join("sys/bus/acpi/devices/PNP0C09:00")).unwrap();
        let f = scan(&roots).find("ec-guard").unwrap().clone();
        assert_eq!((f.severity, f.fix.as_ref().unwrap().label.as_str(), f.fix.as_ref().unwrap().root), (Severity::Suggestion, "Guard the Keys", true));
        let said: Vec<String> = f.fix.as_ref().unwrap().steps.iter().map(Step::describe).collect();
        assert_eq!(
            said,
            [
                "Installing kernel-devel-matched, gcc, make",
                "Installing /usr/local/libexec/olo/olo-ec-storm",
                "Labelling /usr/local/libexec/olo/olo-ec-storm for SELinux",
                "Writing /etc/systemd/system/olo-ec-storm.service",
                "Reloading systemd for olo-ec-storm.service",
                "Enabling olo-ec-storm.service",
                "Restarting olo-ec-storm.service",
            ]
        );
        // A kernel series the driver wasn't checked in: not offered.
        w("proc/sys/kernel/osrelease", "9.9.1-100.fc50.x86_64\n");
        assert!(scan(&roots).find("ec-guard").is_none());
        w("proc/sys/kernel/osrelease", "7.2.15-300.fc45.x86_64\n");
        // In, its service this build's, enabled and run: fine.
        w("lib/modules/7.2.15-300.fc45.x86_64/build/Makefile", "");
        w("usr/bin/make", "");
        w("usr/bin/gcc", "");
        w("sys/module/olo_ec_storm/parameters/stats", "guarding 64 ran 12 counted 0\n");
        w("usr/local/libexec/olo/olo-ec-storm", "the service");
        w("etc/systemd/system/olo-ec-storm.service", EC_STORM_UNIT);
        let kept = |name: &str| -> Result<UnitState, String> { if name == EC_STORM_SERVICE { Ok(UnitState { loaded: true, active: true, enabled: true, path: None }) } else { Err("none".into()) } };
        let roots = Roots { units: Box::new(kept), ..roots };
        let r = scan(&roots);
        assert!(r.find("ec-guard").is_none());
        assert!(
            r.fine.iter().any(|f| f.title.starts_with("A flood of controller events") && f.detail.contains("guarding 64 ran 12 counted 0")),
            "{:?}",
            r.fine.iter().map(|f| &f.title).collect::<Vec<_>>()
        );
        // Without its program beside ours (an Olo from before it): nothing to offer.
        std::fs::remove_dir_all(fs.join("sys/module/olo_ec_storm")).unwrap();
        std::fs::remove_file(roots.bin.join("olo-ec-storm")).unwrap();
        assert!(scan(&roots).find("ec-guard").is_none());
    }

    /// The embedded controller flooding the kernel is said while the
    /// supervisor's file says it lasts, with what it holds up and what
    /// clears it; and not otherwise.
    #[test]
    fn an_ec_storm_is_said_while_it_lasts() {
        use olo_protocol::performance::EcStorm;
        let (fs, mut roots) = machine("ec-storm");
        roots.temp.runtime = fs.join("run/user/1000");
        let found = |roots: &Roots| scan(roots).findings.into_iter().find(|f| f.id == "ec-storm");
        assert!(found(&roots).is_none());
        let file = EcStorm::path_under(&roots.temp.runtime);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, serde_json::to_vec(&EcStorm { since: 1, events_per_second: 46.2, waiting: 352 }).unwrap()).unwrap();
        let f = found(&roots).expect("said");
        assert_eq!((f.severity, f.fix.is_none()), (Severity::Critical, true));
        assert!(f.detail.contains("46 power events a second") && f.detail.contains("352 are waiting") && f.detail.contains("Shutting down fully"), "{}", f.detail);
        std::fs::remove_file(&file).unwrap();
        assert!(found(&roots).is_none(), "over");
        std::fs::write(&file, "not json").unwrap();
        assert!(found(&roots).is_none());
    }

    /// Realtime: without the account's leave, Allow Realtime (the account
    /// by its uid, up to Olo's most, no time limit); written but this
    /// session from before it, the next login; in force, fine.
    #[test]
    fn realtime_leave_is_offered_then_fine() {
        let (fs, roots) = machine("realtime");
        let limits = |prio: u32, soft: &str, hard: &str| {
            let p = fs.join("proc/self/limits");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, format!("Limit                     Soft Limit           Hard Limit           Units     \nMax realtime priority     {prio}                    {prio}                    \nMax realtime timeout      {soft}            {hard}            us        \n")).unwrap();
        };
        limits(0, "unlimited", "unlimited");
        let mut r = Report::default();
        assert!(!realtime(&roots, &mut r));
        let f = r.findings.iter().find(|f| f.id == "realtime").unwrap();
        assert_eq!(f.severity, Severity::Problem);
        let Some(Fix { root: true, steps, .. }) = &f.fix else { panic!("no root fix") };
        let Step::WriteFile { path, content, .. } = &steps[0] else { panic!("not a file") };
        assert_eq!(path, &PathBuf::from("/").join(REALTIME_LIMITS_CONF));
        let u = roots.uid;
        assert!(content.contains(&format!("{u}:{u} - rtprio 70")) && content.contains(&format!("{u}:{u} - rttime unlimited")), "{content}");
        // Written; this session started before it.
        std::fs::create_dir_all(fs.join(REALTIME_LIMITS_CONF).parent().unwrap()).unwrap();
        std::fs::write(fs.join(REALTIME_LIMITS_CONF), content).unwrap();
        let mut r = Report::default();
        assert!(!realtime(&roots, &mut r));
        let f = r.findings.iter().find(|f| f.id == "realtime").unwrap();
        assert!(f.severity == Severity::Advice && f.fix.is_none());
        // In force (the guard's own limits in the process, 30 s and 31 s).
        limits(70, "30000000", "31000000");
        let mut r = Report::default();
        assert!(realtime(&roots, &mut r));
        assert!(r.findings.is_empty() && r.fine.iter().any(|f| f.title.contains("realtime")));
        // A hard limit of rtkit's (200 ms) isn't leave: ended at that.
        limits(70, "200000", "200000");
        let mut r = Report::default();
        assert!(!realtime(&roots, &mut r));
    }

    /// The desktop's core kept in memory: where the account may lock only
    /// its 8 MB, the fix lets it lock all (the limits file with its line,
    /// the running programs' limit raised in place) and writes the sound
    /// server's setting; let, app_server locks from its next start; and
    /// locked, fine.
    #[test]
    fn the_core_is_kept_in_memory() {
        let (fs, roots) = machine("memory-kept");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let limits = |locked: &str| {
            w(
                "proc/self/limits",
                &format!(
                    "Limit                     Soft Limit           Hard Limit           Units     \nMax locked memory         {locked}              {locked}              bytes     \nMax realtime priority     70                   70                   \n"
                ),
            )
        };
        let u = roots.uid;
        let look = |roots: &Roots| {
            let (mut r, procs) = (Report::default(), processes(&roots.fs));
            memory_kept(roots, &procs, &mut r);
            r
        };
        // Nothing said where the limit can't be read.
        assert!(look(&roots).findings.is_empty() && look(&roots).fine.is_empty());
        // A login's 8 MB: everything the fix does.
        limits("8388608");
        let r = look(&roots);
        let f = r.find("memory-kept").unwrap();
        assert_eq!((f.severity, f.title.as_str()), (Severity::Advice, "The desktop's core can be moved out of memory"));
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root && fix.label == "Keep the Core in Memory");
        assert_eq!(
            fix.steps.iter().map(Step::describe).collect::<Vec<_>>(),
            [
                "Writing /etc/security/limits.d/80-olo-realtime.conf",
                "Letting the programs running now keep their memory in RAM",
                "Writing /etc/systemd/user/pipewire.service.d/50-olo-memory.conf",
                "Writing /etc/systemd/user/pipewire-pulse.service.d/50-olo-memory.conf",
                "Writing /etc/systemd/user/wireplumber.service.d/50-olo-memory.conf",
            ]
        );
        let Step::WriteFile { content, .. } = &fix.steps[0] else { panic!("not a file") };
        // (The realtime leave kept in the same file.)
        assert!(
            content.contains(&format!("{u}:{u} - memlock unlimited")) && content.contains(&format!("{u}:{u} - rtprio 70")) && content.contains(&format!("{u}:{u} - rttime unlimited")),
            "{content}"
        );
        assert_eq!(fix.steps[1], Step::LetSessionLockMemory { uid: u });
        assert!(SOUND_MEMORY_CONF.ends_with("[Service]\nMemorySwapMax=0\n") && after_fix("memory-kept").is_some());
        // Allow Realtime's file from before the line (as Ben's was): the
        // file again, with it.
        w(REALTIME_LIMITS_CONF, &format!("{u}:{u} - rtprio 70\n{u}:{u} - rttime unlimited\n"));
        assert!(matches!(&look(&roots).find("memory-kept").unwrap().fix.as_ref().unwrap().steps[0], Step::WriteFile { path, .. } if path.ends_with("80-olo-realtime.conf")));
        // The file as it should be, this session from before it: the
        // running programs' limit, and sound's setting.
        w(REALTIME_LIMITS_CONF, &realtime_limits_conf(u));
        assert_eq!(look(&roots).find("memory-kept").unwrap().fix.as_ref().unwrap().steps.len(), 4);
        for p in SOUND_MEMORY_CONFS {
            w(p, SOUND_MEMORY_CONF);
        }
        assert_eq!(look(&roots).find("memory-kept").unwrap().fix.as_ref().unwrap().steps, [Step::LetSessionLockMemory { uid: u }]);
        // Let: app_server, started before, hasn't locked its own yet.
        limits("unlimited");
        w("proc/950/cmdline", "/home/me/.local/bin/olo-app-server\0");
        w("proc/950/cgroup", "0::/user.slice/user-1000.slice/session-2.scope\n");
        w("proc/950/status", "Name:\tolo-app-server\nVmLck:\t       0 kB\nVmRSS:\t  295820 kB\n");
        // (The test's files are the test's user's: the account's.)
        let roots = Roots { uid: std::fs::metadata(fs.join("proc/950")).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap(), ..roots };
        w(REALTIME_LIMITS_CONF, &realtime_limits_conf(roots.uid));
        let r = look(&roots);
        let f = r.find("memory-kept").unwrap();
        assert!(f.fix.is_none() && f.title == "The desktop's core keeps its memory from Olo's next start", "{f:?}");
        // Locked: fine.
        w("proc/950/status", "Name:\tolo-app-server\nVmLck:\t  295820 kB\nVmRSS:\t  295820 kB\n");
        let r = look(&roots);
        assert!(r.findings.is_empty() && r.fine.iter().any(|f| f.title == "The desktop's core stays in memory"));
    }

    /// Open files: an Olo program held to a login's 1,024 is found, the
    /// window server first, a Problem once it's half way there; the fix
    /// raises the running ones (no password), and where the account's
    /// own limit is low writes the limits too (with one). Raised: fine.
    #[test]
    fn olos_programs_may_keep_their_files_open() {
        let (fs, roots) = machine("open-files");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let program = |pid: u32, name: &str, soft: &str, hard: &str, open: usize| {
            w(&format!("proc/{pid}/cmdline"), &format!("/home/me/.local/bin/{name}\0"));
            w(&format!("proc/{pid}/cgroup"), "0::/user.slice/user-1000.slice/session-2.scope\n");
            w(&format!("proc/{pid}/status"), &format!("Name:\t{name}\n"));
            w(
                &format!("proc/{pid}/limits"),
                &format!("Limit                     Soft Limit           Hard Limit           Units     \nMax open files            {soft}                 {hard}               files     \n"),
            );
            let _ = std::fs::remove_dir_all(fs.join(format!("proc/{pid}/fd")));
            for n in 0..open {
                w(&format!("proc/{pid}/fd/{n}"), "");
            }
        };
        let look = |roots: &Roots| {
            let (mut r, procs) = (Report::default(), processes(&roots.fs));
            open_files(roots, &procs, &mut r);
            r
        };
        // No Olo program to be seen: nothing said.
        assert!(look(&roots).findings.is_empty() && look(&roots).fine.is_empty());
        program(950, "olo-app-server", "1024", "524288", 40);
        program(951, "olo-tracker", "1024", "524288", 30);
        // (Someone else's program, held the same: not Olo's to raise.)
        program(952, "bash", "1024", "524288", 4);
        // (The test's files are the test's user's: the account's.)
        let roots = Roots { uid: std::fs::metadata(fs.join("proc/950")).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap(), ..roots };
        let r = look(&roots);
        let f = r.find("open-files").unwrap();
        assert_eq!((f.severity, f.title.as_str()), (Severity::Advice, "Olo's window server may keep only 1,024 files open"));
        assert!(f.detail.contains("It has 40 open now.") && f.detail.contains("One other Olo program is held the same."), "{}", f.detail);
        let fix = f.fix.as_ref().unwrap();
        assert!(!fix.root && fix.label == "Raise the Limit");
        assert_eq!(fix.steps, [Step::LetOloOpenFiles { uid: roots.uid, hard: 524_288 }]);
        // Half way to the limit (three displays' windows): a problem.
        program(950, "olo-app-server", "1024", "524288", 696);
        let f = look(&roots).find("open-files").cloned().unwrap();
        assert!(f.severity == Severity::Problem && f.detail.contains("It has 696 open now."), "{f:?}");
        // The account's own limit low: its limits written too, as root.
        program(950, "olo-app-server", "1024", "4096", 40);
        let f = look(&roots).find("open-files").cloned().unwrap();
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root && f.detail.contains("this system's own limit is low"));
        assert_eq!(
            fix.steps.iter().map(Step::describe).collect::<Vec<_>>(),
            [
                "Writing /etc/security/limits.d/80-olo-open-files.conf",
                "Writing /etc/systemd/system.conf.d/80-olo-open-files.conf",
                "Writing /etc/systemd/user.conf.d/80-olo-open-files.conf",
                "Letting Olo's programs running now keep more files open",
            ]
        );
        let Step::WriteFile { content, .. } = &fix.steps[0] else { panic!("not a file") };
        assert!(content.ends_with(&format!("{u}:{u} hard nofile 524288\n", u = roots.uid)), "{content}");
        assert!(open_files_systemd_conf().ends_with("[Manager]\nDefaultLimitNOFILE=1024:524288\n"));
        // Raised (what each does as it starts, and what the fix does): fine.
        program(950, "olo-app-server", "524288", "524288", 696);
        program(951, "olo-tracker", "524288", "524288", 30);
        let r = look(&roots);
        assert!(r.findings.is_empty(), "{:?}", r.findings);
        assert!(r.fine.iter().any(|f| f.title == "Olo's programs may keep the files they need open" && f.detail == "The window server may keep 524,288 open, and has 696."), "{:?}", r.fine);
        assert_eq!((thousands(7), thousands(1024), thousands(1_048_576)), ("7".into(), "1,024".into(), "1,048,576".into()));
    }

    /// Language: judged by what a program started now gets. Deskbar
    /// without a language gives the computer's as it starts programs
    /// (fine), unless it's from before an upgrade (said, with nothing to
    /// press); what's started on demand is the user manager's, and its
    /// fix sets that; a computer with none gets /etc/locale.conf written.
    /// Every fix changes what's judged: fixed, it's gone.
    #[test]
    fn programs_know_their_language() {
        let (fs, roots) = machine("language");
        let w = |p: &str, c: &[u8]| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let look = |roots: &Roots| {
            let (mut r, procs) = (Report::default(), processes(&roots.fs));
            language(roots, &procs, &mut r);
            r
        };
        let fine = |r: &Report, with: &str| r.findings.is_empty() && r.fine.iter().any(|f| f.title == "Programs know their language" && f.detail == format!("They start with {with}."));
        // No Deskbar of this session to judge by: nothing said.
        assert!(look(&roots).findings.is_empty() && look(&roots).fine.is_empty());
        w("proc/960/cmdline", b"/home/me/.local/bin/olo-deskbar\0");
        w("proc/960/cgroup", b"0::/user.slice/user-1000.slice/session-2.scope\n");
        w("proc/960/status", b"Name:\tolo-deskbar\n");
        w("proc/960/environ", b"HOME=/home/me\0WAYLAND_DISPLAY=wayland-1\0LANGUAGE=en\0");
        let uid = std::fs::metadata(fs.join("proc/960")).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap();
        let with_session = |env: Option<&[(&str, &str)]>| Roots {
            uid,
            temp: TempFiles { session_env: env.map(|e| e.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()), ..TempFiles::default() },
            ..machine("language-roots").1
        };
        let at = |roots: Roots| Roots { fs: fs.0.clone(), ..roots };
        // The computer's is set; Deskbar was started without, and gives
        // it to what it starts: fine (what's on demand not known).
        w("etc/locale.conf", b"LANG=\"en_GB.UTF-8\"\nLC_TIME=nl_NL.UTF-8\n");
        assert!(fine(&look(&at(with_session(None))), "en_GB.UTF-8"));
        // What's started on demand has none: found, and its fix sets the
        // session's (no password); set, it's gone.
        let f = look(&at(with_session(Some(&[("PATH", "/usr/bin")])))).find("language").cloned().unwrap();
        assert_eq!((f.severity, f.title.as_str()), (Severity::Advice, "Programs started on demand don't know their language"));
        let fix = f.fix.as_ref().unwrap();
        assert!(!fix.root && fix.label == "Set the Language");
        assert_eq!(fix.steps, [Step::SetSessionEnvironment { vars: vec![("LANG".into(), "en_GB.UTF-8".into()), ("LC_TIME".into(), "nl_NL.UTF-8".into())] }]);
        assert!(fine(&look(&at(with_session(Some(&[("LANG", "en_GB.UTF-8"), ("LC_TIME", "nl_NL.UTF-8")])))), "en_GB.UTF-8"));
        // A Deskbar from before an upgrade hands none down: said, with
        // nothing to press (the upgrade does it).
        std::os::unix::fs::symlink("/home/me/.local/bin/olo-deskbar (deleted)", fs.join("proc/960/exe")).unwrap();
        let f = look(&at(with_session(Some(&[("LANG", "en_GB.UTF-8")])))).find("language").cloned().unwrap();
        assert!(f.fix.is_none() && f.title == "Programs here don't know their language" && f.detail.contains("upgrade Olo from Deskbar's tray"), "{f:?}");
        std::fs::remove_file(fs.join("proc/960/exe")).unwrap();
        // None set on the computer: written, US English where it's
        // installed; and then it's what Deskbar gives.
        std::fs::remove_file(fs.join("etc/locale.conf")).unwrap();
        let f = look(&at(with_session(Some(&[])))).find("language").cloned().unwrap();
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root && f.detail.contains("The fix sets C.UTF-8"), "{f:?}");
        assert!(matches!(&fix.steps[0], Step::WriteFile { path, content, .. } if path.ends_with("etc/locale.conf") && content.ends_with("LANG=C.UTF-8\n")));
        std::fs::create_dir_all(fs.join("usr/lib/locale/en_US.utf8")).unwrap();
        let f = look(&at(with_session(Some(&[])))).find("language").cloned().unwrap();
        let Step::WriteFile { content, .. } = &f.fix.as_ref().unwrap().steps[0] else { panic!("{f:?}") };
        assert!(content.ends_with("LANG=en_US.UTF-8\n"));
        assert_eq!(f.fix.as_ref().unwrap().steps[1], Step::SetSessionEnvironment { vars: vec![("LANG".into(), "en_US.UTF-8".into())] });
        // (The fix done: the file as it wrote it, the session's set.)
        w("etc/locale.conf", content.as_bytes());
        assert!(fine(&look(&at(with_session(Some(&[("LANG", "en_US.UTF-8")])))), "en_US.UTF-8"));
        // The session has it itself: fine.
        w("proc/960/environ", b"HOME=/home/me\0LANG=nl_NL.UTF-8\0");
        assert!(fine(&look(&at(with_session(Some(&[("LANG", "nl_NL.UTF-8")])))), "nl_NL.UTF-8"));
    }

    /// The inotify limit: low with most of it taken, a problem; with
    /// few taken, advice; raised to 8,192 now and at every start; fine
    /// at 8,192.
    #[test]
    fn programs_may_follow_their_files() {
        let (fs, roots) = machine("inotify");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        // Two of this account's programs holding three instances between
        // them, and another account's one (not counted).
        let procs: Vec<Proc> = [(101, roots.uid), (102, roots.uid), (103, roots.uid + 1)].iter().map(|&(pid, uid)| Proc { pid, uid, name: "olo-tracker".into(), exe: None, scope: None }).collect();
        for (pid, n) in [(101, 2), (102, 1), (103, 1)] {
            let dir = fs.join(format!("proc/{pid}/fd"));
            std::fs::create_dir_all(&dir).unwrap();
            for i in 0..n {
                std::os::unix::fs::symlink("anon_inode:inotify", dir.join(format!("{}", 10 + i))).unwrap();
            }
            std::os::unix::fs::symlink("/dev/null", dir.join("0")).unwrap();
        }
        let look = |roots: &Roots| {
            let mut r = Report::default();
            inotify(roots, &procs, &mut r);
            r
        };
        w("proc/sys/fs/inotify/max_user_instances", "4\n");
        let r = look(&roots);
        let f = r.find("inotify-instances").unwrap();
        assert_eq!((f.severity, f.title.as_str()), (Severity::Problem, "Only 4 programs may follow files at once"));
        assert!(f.detail.contains("this account's 4, and 3 are taken"), "{}", f.detail);
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root && fix.label == "Raise the Limit");
        assert_eq!(
            fix.steps,
            [
                Step::WriteFile { path: "/etc/sysctl.d/90-olo-inotify.conf".into(), content: INOTIFY_SYSCTL.into(), mode: 0o644 },
                Step::WriteSysfs { path: "/proc/sys/fs/inotify/max_user_instances".into(), value: "8192".into() }
            ]
        );
        assert!(INOTIFY_SYSCTL.lines().any(|l| l == "fs.inotify.max_user_instances = 8192"));
        w("proc/sys/fs/inotify/max_user_instances", "128\n");
        assert_eq!(look(&roots).find("inotify-instances").unwrap().severity, Severity::Advice, "few taken: advice");
        w("proc/sys/fs/inotify/max_user_instances", "8192\n");
        let r = look(&roots);
        assert!(r.findings.is_empty() && r.fine[0].detail.starts_with("Up to 8,192 programs"), "{:?}", r.fine);
    }

    /// Swappiness: with swap on and the kernel's 60, set to 1 now and at
    /// every start; fine at 1; nothing to say with no swap.
    #[test]
    fn swappiness_is_set_to_one() {
        let (fs, roots) = machine("swappiness");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let heading = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n";
        let look = |roots: &Roots| {
            let mut r = Report::default();
            swappiness(roots, &mut r);
            r
        };
        w("proc/sys/vm/swappiness", "60\n");
        w("proc/swaps", heading);
        assert!(look(&roots).findings.is_empty() && look(&roots).fine.is_empty());
        w("proc/swaps", &format!("{heading}/dev/zram0 partition 8388604 0 100\n"));
        let r = look(&roots);
        let f = r.find("swappiness").unwrap();
        assert_eq!((f.severity, f.title.as_str()), (Severity::Advice, "Programs' memory is moved to swap while files' cache could go"));
        assert!(f.detail.starts_with("vm.swappiness is 60: "), "{}", f.detail);
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root && fix.label == "Keep Programs in Memory");
        assert_eq!(
            fix.steps,
            [
                Step::WriteFile { path: "/etc/sysctl.d/80-olo-memory.conf".into(), content: MEMORY_SYSCTL.into(), mode: 0o644 },
                Step::WriteSysfs { path: "/proc/sys/vm/swappiness".into(), value: "1".into() }
            ]
        );
        assert!(MEMORY_SYSCTL.lines().any(|l| l == "vm.swappiness = 1"));
        w("proc/sys/vm/swappiness", "1\n");
        let r = look(&roots);
        assert!(r.findings.is_empty() && r.fine[0].detail.starts_with("vm.swappiness is 1: "));
    }

    /// Swap as large as memory is suggested where it's the generator's
    /// compressed-memory swap and smaller, and the graphics driver can't
    /// take the room for windows' buffers; not otherwise.
    #[test]
    fn swap_as_large_as_memory_is_suggested() {
        let (fs, roots) = machine("swap-size");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let heading = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n";
        let driver = |name: &str| {
            let link = fs.join("sys/bus/pci/devices/0000:00:02.0/driver");
            let _ = std::fs::remove_file(&link);
            std::fs::create_dir_all(fs.join("sys/bus/pci/drivers").join(name)).unwrap();
            std::os::unix::fs::symlink(fs.join("sys/bus/pci/drivers").join(name), link).unwrap();
        };
        let look = |roots: &Roots| {
            let mut r = Report::default();
            swap_size(roots, &pci_devices(&roots.fs), &mut r);
            r
        };
        w("sys/bus/pci/devices/0000:00:02.0/class", "0x030000\n");
        w("sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
        driver("xe");
        w("proc/meminfo", "MemTotal:       65263084 kB\nMemFree:         2114000 kB\n");
        w("proc/swaps", &format!("{heading}/dev/zram0 partition 8388604 0 100\n"));
        w("usr/lib/systemd/zram-generator.conf", "[zram0]\nzram-size = min(ram, 8192)\n");
        // xe's without Olo's patch: more swap would be more buffers out.
        assert!(look(&roots).findings.is_empty());
        w("sys/kernel/livepatch/olo_gpu_memory/enabled", "1\n");
        let r = look(&roots);
        let f = r.find("swap-size").unwrap();
        assert_eq!((f.severity, f.title.as_str()), (Severity::Suggestion, "More swap, for what should be swapped"));
        assert!(f.detail.starts_with("Swap is 8.59\u{a0}GB in compressed memory, and the computer has 66.83\u{a0}GB of memory."), "{}", f.detail);
        assert!(f.detail.contains("Empty, it takes about 230.4\u{a0}MB for the kernel's own tables"), "{}", f.detail);
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root && fix.label == "Make Swap as Large as Memory");
        assert_eq!(
            fix.steps,
            [
                Step::WriteFile { path: "/etc/systemd/zram-generator.conf".into(), content: ZRAM_FULL_CONF.into(), mode: 0o644 },
                Step::Unit { name: "swap".into(), action: UnitAction::Reload },
                Step::RemakeZramSwap { device: "zram0".into() },
            ]
        );
        assert!(ZRAM_FULL_CONF.ends_with("[zram0]\nzram-size = ram\n"));
        // As large as memory already (what the kernel shows of 64 GB).
        w("proc/swaps", &format!("{heading}/dev/zram0 partition 65263080 0 100\n"));
        assert!(look(&roots).findings.is_empty());
        // A disk's swap: its size isn't a setting's.
        w("proc/swaps", &format!("{heading}/dev/nvme0n1p3 partition 8388604 0 -2\n"));
        assert!(look(&roots).findings.is_empty());
        // None: nothing to enlarge (and that's the graphics fix's doing).
        w("proc/swaps", heading);
        assert!(look(&roots).findings.is_empty());
        // Another driver's graphics: suggested without the patch.
        w("proc/swaps", &format!("{heading}/dev/zram0 partition 8388604 0 100\n"));
        std::fs::remove_dir_all(fs.join("sys/kernel/livepatch")).unwrap();
        driver("i915");
        assert!(look(&roots).find("swap-size").is_some());
    }

    /// The power modes as planned: nothing said without a power profiles
    /// daemon; the follower installed, started, brought up to date, tried
    /// again, its settings set again when something changed them; else
    /// fine, saying what's in force.
    #[test]
    fn power_modes_as_planned() {
        use olo_system::{
            cpu_tuning::{CpuPreset, Sys},
            gpu_tuning::{self as g, Followed},
        };
        let (fs, mut roots) = machine("power-modes");
        let w = |p: &str, c: &str| {
            let path = fs.join(p.trim_start_matches('/'));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        for k in 0..2 {
            let d = format!("sys/devices/system/cpu/cpufreq/policy{k}");
            w(&format!("{d}/scaling_driver"), "intel_pstate\n");
            w(&format!("{d}/scaling_governor"), "powersave\n");
            w(&format!("{d}/scaling_available_governors"), "performance powersave\n");
            w(&format!("{d}/energy_performance_preference"), "balance_performance\n");
            w(&format!("{d}/energy_performance_available_preferences"), "default performance balance_performance balance_power power\n");
        }
        w("sys/devices/system/cpu/intel_pstate/no_turbo", "0\n");
        w("sys/devices/system/cpu/intel_pstate/max_perf_pct", "75\n");
        w("sys/firmware/acpi/platform_profile", "balanced\n");
        w("sys/firmware/acpi/platform_profile_choices", "low-power balanced performance\n");
        std::fs::write(roots.bin.join("olo-gpu-tune"), "the follower").unwrap();
        let units = |daemon: bool, follower: bool| -> Box<dyn Fn(&str) -> Result<UnitState, String> + Send + Sync> {
            Box::new(move |name: &str| {
                let active = match name {
                    "tuned-ppd.service" => daemon,
                    g::FOLLOWER_SERVICE => follower,
                    _ => false,
                };
                Ok(UnitState { loaded: active, active, enabled: active, path: None })
            })
        };
        // No daemon: nothing to follow.
        roots.units = units(false, false);
        let r = scan(&roots);
        assert!(r.findings.iter().all(|f| !f.id.starts_with("power-modes")) && r.fine.iter().all(|f| !f.title.starts_with("The power modes")));
        // Not installed: Install It, everything.
        roots.units = units(true, false);
        let f = scan(&roots).find("power-modes:missing").cloned().expect("not installed");
        let steps = f.fix.unwrap().steps;
        assert_eq!(steps[0], Step::CopyFile { from: roots.bin.join("olo-gpu-tune"), to: g::FOLLOWER_PROGRAM.into(), mode: 0o755 });
        assert!(steps.contains(&Step::WriteFile { path: g::FOLLOWER_UNIT_PATH.into(), content: g::FOLLOWER_UNIT.into(), mode: 0o644 }));
        assert_eq!(steps.last(), Some(&Step::Unit { name: g::FOLLOWER_SERVICE.into(), action: UnitAction::Restart }));
        // Installed, this version's program, stopped: Start It.
        w(g::FOLLOWER_PROGRAM, "the follower");
        w(g::FOLLOWER_UNIT_PATH, g::FOLLOWER_UNIT);
        w(g::FOLLOWER_RULE_PATH, g::FOLLOWER_RULE);
        let f = scan(&roots).find("power-modes:stopped").cloned().expect("stopped");
        assert_eq!(f.fix.unwrap().steps, [Step::Unit { name: g::FOLLOWER_SERVICE.into(), action: UnitAction::Restart }]);
        // Running, saying nothing (an older one): Update It, which
        // restarts it on the program installed.
        roots.units = units(true, true);
        let f = scan(&roots).find("power-modes:older").cloned().expect("older");
        assert_eq!(f.fix.unwrap().steps.last(), Some(&Step::Unit { name: g::FOLLOWER_SERVICE.into(), action: UnitAction::Restart }));
        // Refused something: Try Again.
        let sys = Sys { cpu: fs.join("sys/devices/system/cpu"), firmware: fs.join("sys/firmware/acpi") };
        let mut followed = Followed {
            version: g::FOLLOWER_VERSION,
            mode: "balanced".into(),
            on_battery: true,
            cpu: "balanced".into(),
            top: 75,
            failed: vec!["turbo: Permission denied".into()],
            cpu_after: sys.caps(),
            ..Default::default()
        };
        let save = |f: &Followed, ago: u64| {
            let path = Followed::path(&fs);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_vec(f).unwrap()).unwrap();
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(ago)).unwrap();
        };
        save(&followed, 60);
        let f = scan(&roots).find("power-modes:failed").cloned().expect("refused");
        assert!(f.detail.contains("Adaptive Performance on battery") && f.detail.contains("turbo: Permission denied"), "{}", f.detail);
        // As set: fine, saying so.
        followed.failed.clear();
        save(&followed, 60);
        let r = scan(&roots);
        let fine = r.fine.iter().find(|f| f.title == "The power modes are set as Olo plans them").expect("fine");
        assert_eq!(fine.detail, format!("Now Adaptive Performance on battery: the processors {}, at up to 75% of their top speed (olo-gpu-follow).", CpuPreset::Balanced.name()));
        // Changed since (the top speed lifted, the preference raised): set
        // them again; but not a moment after the follower wrote.
        w("sys/devices/system/cpu/intel_pstate/max_perf_pct", "100\n");
        w("sys/devices/system/cpu/cpufreq/policy0/energy_performance_preference", "performance\n");
        let f = scan(&roots).find("power-modes:changed").cloned().expect("changed");
        assert!(f.detail.contains("the energy preference performance (Olo set balance_performance), the top speed 100% (Olo set 75%)"), "{}", f.detail);
        assert_eq!(f.fix.unwrap().label, "Set Them Again");
        save(&followed, 1);
        assert!(scan(&roots).find("power-modes:changed").is_none(), "just written: the follower may be on its way");
    }

    /// What Olo needs to run, missing: said with the packages, the fix
    /// installs them; nothing said where Olo isn't installed or
    /// PackageKit doesn't answer.
    #[test]
    fn needed_packages_missing() {
        let (fs, mut roots) = machine("needed-packages");
        std::fs::create_dir_all(fs.join("etc")).unwrap();
        std::fs::write(fs.join("etc/os-release"), "ID=fedora\n").unwrap();
        roots.packages = Box::new(|names| Some(names.iter().map(|n| ((*n).to_owned(), *n != "libwayland-server")).collect()));
        let _ = std::fs::remove_file(roots.bin.join("olo-session"));
        assert!(scan(&roots).find("packages:needed").is_none(), "Olo not installed here");
        std::fs::create_dir_all(&roots.bin).unwrap();
        std::fs::write(roots.bin.join("olo-session"), "").unwrap();
        let r = scan(&roots);
        let f = r.find("packages:needed").expect("said");
        assert_eq!(f.title, "Olo needs libwayland-server");
        assert_eq!(f.fix.as_ref().unwrap().steps, [Step::InstallPackages { names: vec!["libwayland-server".into()] }]);
        roots.packages = Box::new(|_| None);
        assert!(scan(&roots).find("packages:needed").is_none(), "PackageKit not answering: nothing said");
    }

    /// No PackageKit or dnf's service (a minimal Fedora): said, and the fix
    /// installs them; on Fedora only.
    #[test]
    fn no_package_services() {
        let (fs, mut roots) = machine("package-services");
        std::fs::create_dir_all(fs.join("etc")).unwrap();
        std::fs::write(fs.join("etc/os-release"), "NAME=\"Fedora Linux\"\nID=fedora\nVERSION_ID=45\n").unwrap();
        assert!(scan(&roots).find("packages:services").is_none(), "both there");
        roots.package_services = Box::new(|| vec!["PackageKit", "dnf5daemon-server"]);
        let r = scan(&roots);
        let f = r.find("packages:services").expect("said");
        assert!(f.detail.starts_with("PackageKit and dnf5daemon-server aren't installed"), "{}", f.detail);
        assert_eq!(f.fix.as_ref().map(|x| (x.root, x.steps.clone())), Some((true, vec![Step::InstallPackageServices])));
        roots.package_services = Box::new(|| vec!["dnf5daemon-server"]);
        let f = scan(&roots).find("packages:services").cloned().expect("said");
        assert!(f.detail.starts_with("dnf5daemon-server isn't installed"), "{}", f.detail);
        std::fs::write(fs.join("etc/os-release"), "ID=debian\n").unwrap();
        assert!(scan(&roots).find("packages:services").is_none(), "Fedora only");
    }

    /// The build cache: nothing said where kache isn't used; its daemon
    /// not running (its service there), started by the fix; running, its
    /// store held past its limit by builds' target folders, else fine.
    #[test]
    fn the_build_cache() {
        let (fs, mut roots) = machine("kache");
        roots.temp.cache = fs.join("home/me/.cache/olo");
        roots.temp.config = fs.join("home/me/.config");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        // Not used here: nothing said.
        let r = scan(&roots);
        assert!(r.find("kache:not-running").is_none() && r.find("kache:over-limit").is_none());
        // Its store and service, its daemon not running.
        w("home/me/.cache/kache/gc_stats.json", r#"{"bytes_held": 450481390286, "entries_unreclaimable": 4548}"#);
        w("home/me/.config/systemd/user/kache.service", "[Service]\n");
        w("home/me/.config/kache/config.toml", "[cache]\nlocal_max_size = \"95GiB\"\n");
        let r = scan(&roots);
        let f = r.find("kache:not-running").expect("not running");
        assert_eq!(f.fix.as_ref().unwrap().steps, [Step::StartProgram { argv: vec!["olo-kache".into(), "--start".into()] }]);
        assert!(r.find("kache:over-limit").is_none(), "nothing more said of one not running");
        // Running (as its service runs it): held past its limit.
        w("proc/960/cmdline", "/home/me/.cargo/bin/kache-engine\0daemon\0run\0");
        let r = scan(&roots);
        assert!(r.find("kache:not-running").is_none());
        let f = r.find("kache:over-limit").expect("over");
        assert!(f.detail.contains("share 450 GB") && f.detail.contains("4548 entries") && f.detail.contains("102 GB limit"), "{}", f.detail);
        assert_eq!(f.fix.as_ref().unwrap().label, "Open Kache");
        // Held under it: fine.
        w("home/me/.cache/kache/gc_stats.json", r#"{"bytes_held": 1000}"#);
        let r = scan(&roots);
        assert!(r.find("kache:over-limit").is_none() && r.fine.iter().any(|f| f.title == "The build cache is running" && f.detail.contains("under its 102 GB limit")));
        // A clean-up that looked at no entries (locks swept after a
        // restart): nothing said of the limit.
        w("home/me/.cache/kache/gc_stats.json", r#"{"entries_evicted": 0, "bytes_held": 0, "entries_unreclaimable": 0, "key_locks_removed": 1314}"#);
        let r = scan(&roots);
        assert!(r.find("kache:over-limit").is_none() && r.fine.iter().any(|f| f.title == "The build cache is running" && !f.detail.contains("limit")));
        assert_eq!((size_of("95GiB"), size_of("1.5 TB"), size_of("12"), size_of("lots")), (Some(102_005_473_280), Some(1_500_000_000_000), Some(12), None));
    }

    /// Token Pool running twice (Ben, 2026-11-09): the copy serving holds
    /// the gateway's listening socket, the session's beside it doesn't, and
    /// is quit; the keeper, the command line's own and a launch on its way
    /// aren't. With no copy serving, its gateway started again; with
    /// another program on the port, said; one copy serving, fine. A copy
    /// finishing what it has after an upgrade isn't quit as old.
    #[test]
    fn token_pool_twice_or_without_its_gateway() {
        let (fs, mut roots) = machine("token-pool");
        roots.temp.runtime = fs.join("run/user/1000");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let link = |to: &str, at: &str| {
            let at = fs.join(at);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            let _ = std::fs::remove_file(&at);
            std::os::unix::fs::symlink(to, at).unwrap();
        };
        let pool = format!("{}/olo-pool", roots.bin.display());
        w(
            "proc/net/tcp",
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:45E3 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 555 1 0 100 0 0 10 0\n",
        );
        w("proc/uptime", "100000.00 1.00\n");
        // SAFETY: sysconf has no preconditions.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        let stat = |pid: i32, ago: f64| w(&format!("proc/{pid}/stat"), &format!("{pid} (olo-pool) S 1{} {} 0 0\n", " 0".repeat(17), ((100000.0 - ago) * hz) as u64));
        // Serving; the session's, beside it, an hour old; the keeper; the
        // command line's.
        for (pid, args) in [(950, "--service\0"), (951, "--service\0"), (952, ""), (953, "--status\0")] {
            w(&format!("proc/{pid}/cmdline"), &format!("{pool}\0{args}"));
            link(&pool, &format!("proc/{pid}/exe"));
            stat(pid, 3600.0);
        }
        link("socket:[555]", "proc/950/fd/7");
        w("proc/952/environ", "HOME=/home/me\0OLO_POOL_KEEPER=1\0");
        let r = scan(&roots);
        let f = r.find("token-pool:twice").expect("twice");
        assert_eq!(f.title, "Token Pool is running twice");
        assert!(f.detail.starts_with("Token Pool (process 950) serves") && f.detail.contains("another (process 951)"), "{}", f.detail);
        assert_eq!(f.fix.as_ref().unwrap().label, "Quit It");
        assert_eq!(f.fix.as_ref().unwrap().steps, [Step::Signal { pid: 951, signal: libc::SIGTERM }]);
        // A launch on its way (5 s old) isn't a second Token Pool.
        stat(951, 5.0);
        let r = scan(&roots);
        assert!(r.find("token-pool:twice").is_none());
        assert!(r.fine.iter().any(|f| f.title == "Token Pool serves Claude Code and Codex" && f.detail == "Its gateway listens on port 17891 (process 950)."), "{:?}", r.fine);
        // Nothing serves: the session's started again (not the keeper).
        stat(951, 3600.0);
        std::fs::remove_file(fs.join("proc/950/fd/7")).unwrap();
        std::fs::remove_dir_all(fs.join("proc/950")).unwrap();
        let r = scan(&roots);
        let f = r.find("token-pool:no-gateway").expect("no gateway");
        assert_eq!((f.fix.as_ref().unwrap().label.as_str(), f.fix.as_ref().unwrap().steps.clone()), ("Start It Again", vec![Step::Signal { pid: 951, signal: libc::SIGTERM }]));
        // Another program has the port: said, nothing to press.
        w("proc/960/cmdline", "python3\0-m\0http.server\0");
        link("socket:[555]", "proc/960/fd/3");
        let r = scan(&roots);
        let f = r.find("token-pool:port").expect("port taken");
        assert!(f.detail.starts_with("python3 (process 960) listens on port 17891"), "{}", f.detail);
        assert!(f.fix.is_none() && r.find("token-pool:no-gateway").is_none());
        // A test's Token Pool, on a port of its own, isn't this one's.
        std::fs::remove_dir_all(fs.join("proc/960")).unwrap();
        w("proc/951/environ", "OLO_POOL_PORT=27891\0");
        assert!(scan(&roots).find("token-pool:no-gateway").is_none());
        std::fs::remove_file(fs.join("proc/951/environ")).unwrap();
        // A marker older than the process (another's, its pid again)
        // doesn't make it one finishing.
        w("run/user/1000/olo-upgrade-waits/951", "");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options().write(true).open(fs.join("run/user/1000/olo-upgrade-waits/951")).unwrap().set_modified(old).unwrap();
        assert!(scan(&roots).find("token-pool:no-gateway").is_some());
        // Finishing what it has after handing over: neither idle here nor
        // old to Quit Them (the stale Deskbar, 900, finishing too).
        link("/home/me/.local/bin/olo-pool (deleted)", "proc/951/exe");
        w("run/user/1000/olo-upgrade-waits/951", "");
        w("run/user/1000/olo-upgrade-waits/900", "");
        let r = scan(&roots);
        assert!(r.find("token-pool:no-gateway").is_none() && r.find("stale").is_none(), "{:?}", r.findings.iter().map(|f| &f.id).collect::<Vec<_>>());
    }

    /// Olo's start-up screen: offered (Fedora's in place), old (from
    /// before the stamp, then at another scale), not in the image yet,
    /// and fine with the way back; the steps exactly.
    #[test]
    fn the_start_up_screen() {
        use olo_system::splash;
        let fs = Scratch(PathBuf::from(format!("/var/tmp/olo-doctor-test-{}-splash", std::process::id())));
        let _ = std::fs::remove_dir_all(&fs);
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w("home/me/.local/bin/olo-app-server", "");
        w("usr/sbin/plymouthd", "");
        w("usr/lib64/plymouth/two-step.so", "");
        w("proc/sys/kernel/osrelease", "7.2.15\n");
        w("etc/plymouth/plymouthd.conf", "# Administrator customizations go in this file\n#[Daemon]\n#Theme=fade-in\n");
        w("home/me/.config/olo/app_server.json", r#"{"display":{"scale":1.5}}"#);
        let roots = Roots {
            fs: fs.0.clone(),
            bin: fs.join("home/me/.local/bin"),
            uid: 1000,
            scope: None,
            units: Box::new(|_| Err("none".into())),
            airpods: false,
            phone: false,
            iphone: false,
            phone_connected: false,
            bluetooth_class: None,
            bluetooth_stuck: Vec::new(),
            packages: Box::new(|_| None),
            updates: Box::new(|| None),
            package_services: Box::new(Vec::new),
            temp: TempFiles { config: fs.join("home/me/.config"), cache: fs.join("home/me/.cache/olo"), ..TempFiles::default() },
            pool_port: POOL_PORT,
            kernels_bbr: None,
        };
        let look = || {
            let mut r = Report::default();
            start_up_screen(&roots, &mut r);
            r
        };
        let drawn = fs.join("home/me/.cache/olo/plymouth-theme");
        let install = |backup: bool| {
            let mut steps = vec![Step::CopyTree { from: drawn.clone(), to: splash::THEME_DIR.into(), mode: 0o644 }];
            if backup {
                steps.push(Step::CopyFile { from: splash::CONF.into(), to: splash::CONF_BEFORE.into(), mode: 0o644 });
            }
            steps.extend([Step::WriteFile { path: splash::CONF.into(), content: splash::OLO_CONF.into(), mode: 0o644 }, Step::RebuildInitramfs]);
            steps
        };
        // Fedora's: offered, as a suggestion, at the screen's scale.
        let r = look();
        let f = r.find(START_UP_SCREEN).expect("offered");
        assert_eq!(f.severity, Severity::Suggestion);
        assert!(f.detail.contains("(150%)"), "{}", f.detail);
        assert_eq!(f.fix.as_ref().unwrap().steps, install(true));
        // Olo's, from before the stamp: drawn again (what was there kept
        // already).
        w("etc/plymouth/plymouthd.conf", splash::OLO_CONF);
        w("etc/plymouth/plymouthd.conf.before-olo", "#[Daemon]\n");
        w("usr/share/plymouth/themes/olo/olo.plymouth", "");
        let r = look();
        let f = r.find(START_UP_SCREEN_OLD).expect("old");
        assert!(f.detail.contains("older version"), "{}", f.detail);
        assert_eq!(f.fix.as_ref().unwrap().steps, install(false));
        // Drawn at 200%, the screen at 150% now.
        w("usr/share/plymouth/themes/olo/olo-made.json", &format!(r#"{{"format":{},"scale":2.0}}"#, splash::FORMAT));
        let f = look().findings.into_iter().find(|f| f.id == START_UP_SCREEN_OLD).expect("old");
        assert!(f.detail.contains("drawn at 200%") && f.detail.contains("150%"), "{}", f.detail);
        // Current, but the image from before it.
        w("usr/share/plymouth/themes/olo/olo-made.json", &format!(r#"{{"format":{},"scale":1.5}}"#, splash::FORMAT));
        w("boot/initramfs-7.2.15.img", "");
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(fs.join("boot/initramfs-7.2.15.img")).unwrap().set_modified(past).unwrap();
        let f = look().findings.into_iter().find(|f| f.id == "start-up-screen:image").expect("not in the image");
        assert_eq!(f.fix.unwrap().steps, [Step::RebuildInitramfs]);
        // In the image: fine, and the way back to what was there.
        w("boot/initramfs-7.2.15.img", "made again");
        let r = look();
        assert!(r.findings.is_empty() && r.fine.len() == 1);
        assert_eq!(
            r.action("start-up-screen:undo").unwrap().steps,
            [Step::CopyFile { from: splash::CONF_BEFORE.into(), to: splash::CONF.into(), mode: 0o644 }, Step::RemoveFile { path: splash::CONF_BEFORE.into() }, Step::RebuildInitramfs]
        );
        // No Plymouth: nothing said.
        std::fs::remove_file(fs.join("usr/sbin/plymouthd")).unwrap();
        let r = look();
        assert!(r.findings.is_empty() && r.fine.is_empty());
    }

    /// The start-up chime: nothing without a built-in card, or with the
    /// start-up sound off; offered; fine, with the way back; older when
    /// an upgrade brings a newer program, or another unit.
    #[test]
    fn the_start_up_chime() {
        let fs = Scratch(PathBuf::from(format!("/var/tmp/olo-doctor-test-{}-chime", std::process::id())));
        let _ = std::fs::remove_dir_all(&fs);
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w("home/me/.local/bin/olo-chime", "the chime");
        w("proc/asound/cards", " 0 [NVidia         ]: HDA-Intel - HDA NVidia\n");
        w("proc/asound/card0/pcm3p/info", "id: HDMI 0\n");
        let roots = Roots {
            fs: fs.0.clone(),
            bin: fs.join("home/me/.local/bin"),
            uid: 1000,
            scope: None,
            units: Box::new(|_| Err("none".into())),
            airpods: false,
            phone: false,
            iphone: false,
            phone_connected: false,
            bluetooth_class: None,
            bluetooth_stuck: Vec::new(),
            packages: Box::new(|_| None),
            updates: Box::new(|| None),
            package_services: Box::new(Vec::new),
            temp: TempFiles { config: fs.join("home/me/.config"), ..TempFiles::default() },
            pool_port: POOL_PORT,
            kernels_bbr: None,
        };
        let look = || {
            let mut r = Report::default();
            start_up_chime(&roots, &mut r);
            r
        };
        let program = PathBuf::from("/usr/local/libexec/olo/olo-chime");
        let unit = |action| Step::Unit { name: CHIME_SERVICE.into(), action };
        let copy = [Step::CopyFile { from: fs.join("home/me/.local/bin/olo-chime"), to: program.clone(), mode: 0o755 }, Step::SelinuxLabels { rules: Vec::new(), restore: vec![program.clone()] }];
        // Only HDMI: nowhere to play it.
        let r = look();
        assert!(r.findings.is_empty() && r.fine.is_empty());
        // The built-in card's speakers: offered, all of it.
        w("proc/asound/cards", " 0 [NVidia         ]: HDA-Intel - HDA NVidia\n 1 [sofsoundwire   ]: sof-soundwire - sof-soundwire\n");
        w("proc/asound/card1/pcm2p/info", "id: Speaker (*)\n");
        let f = look().findings.into_iter().next().expect("offered");
        assert_eq!((f.id.as_str(), f.severity), ("start-up-chime", Severity::Suggestion));
        let mut all = copy.to_vec();
        all.extend([Step::WriteFile { path: CHIME_UNIT_PATH.into(), content: CHIME_UNIT.into(), mode: 0o644 }, unit(UnitAction::Reload), unit(UnitAction::Enable)]);
        assert_eq!(f.fix.unwrap().steps, all);
        // The start-up sound turned off: not offered.
        w("home/me/.config/olo/app_server.json", r#"{"startup_sound":false}"#);
        assert!(look().findings.is_empty());
        w("home/me/.config/olo/app_server.json", "{}");
        // Installed, the copy newer than the program: fine, and the way back.
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(fs.join("home/me/.local/bin/olo-chime")).unwrap().set_modified(past).unwrap();
        w("usr/local/libexec/olo/olo-chime", "the chime");
        // (Its time said too, between the program's and now: two files
        // written within one tick of the kernel's clock have the same
        // time, and the newer program below was then not newer, once in
        // a great many runs.)
        std::fs::File::options().write(true).open(fs.join("usr/local/libexec/olo/olo-chime")).unwrap().set_modified(past + std::time::Duration::from_secs(30)).unwrap();
        w("etc/systemd/system/olo-chime.service", CHIME_UNIT);
        w("etc/systemd/system/sound.target.wants/olo-chime.service", CHIME_UNIT);
        let r = look();
        assert!(r.findings.is_empty() && r.fine.len() == 1, "{:?}", r.findings);
        assert_eq!(
            r.action("start-up-chime:undo").unwrap().steps,
            [unit(UnitAction::Disable), Step::RemoveFile { path: CHIME_UNIT_PATH.into() }, Step::RemoveFile { path: program.clone() }, unit(UnitAction::Reload)]
        );
        // An upgrade's newer program: the copy brought up to date alone.
        w("home/me/.local/bin/olo-chime", "the chime, newer");
        let f = look().findings.into_iter().next().expect("older");
        assert_eq!((f.id.as_str(), f.severity), ("start-up-chime:older", Severity::Advice));
        assert_eq!(f.fix.unwrap().steps, copy);
        // Installed whatever the sound setting, and whatever the cards.
        w("usr/local/libexec/olo/olo-chime", "the chime, newer");
        w("etc/systemd/system/olo-chime.service", "[Unit]\n");
        std::fs::remove_dir_all(fs.join("proc/asound/card1")).unwrap();
        let f = look().findings.into_iter().next().expect("another unit");
        assert!(f.detail.contains("isn't set up as this version"), "{}", f.detail);
        assert_eq!(f.fix.unwrap().steps, [Step::WriteFile { path: CHIME_UNIT_PATH.into(), content: CHIME_UNIT.into(), mode: 0o644 }, unit(UnitAction::Reload), unit(UnitAction::Enable)]);
    }

    /// DiskBackup: not set up, said without a button; set up, its service
    /// gone (Start It); running but paused (Resume); nothing for days
    /// (Back Up Now, the reason said); fine, and its key kept elsewhere.
    #[test]
    fn diskbackup_checks() {
        let (fs, mut roots) = machine("diskbackup");
        roots.temp.config = fs.join("home/me/.config");
        roots.temp.runtime = fs.join("run/user/1000");
        let w = |p: &Path, c: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        };
        let (settings, state) = (roots.temp.config.join("olo/diskbackup.json"), roots.temp.runtime.join("olo-diskbackup.json"));
        let ids = |r: &Report| -> Vec<String> { r.findings.iter().chain(&r.ignored).filter(|f| f.id.starts_with("diskbackup:")).map(|f| f.id.clone()).collect() };
        assert_eq!(ids(&scan(&roots)), ["diskbackup:not-set-up"]);
        assert!(scan(&roots).find("diskbackup:not-set-up").unwrap().fix.is_none());
        w(&settings, r#"{"enabled":true,"target":{"kind":"folder","path":"/mnt/backup"},"every_minutes":60}"#);
        let r = scan(&roots);
        assert_eq!(ids(&r), ["diskbackup:not-running", "diskbackup:key"]);
        assert_eq!(r.find("diskbackup:not-running").unwrap().fix.as_ref().unwrap().steps, [Step::StartProgram { argv: vec!["olo-diskbackup".into(), "--service".into()] }]);
        // Running (process 902 is), nothing whole for 5 days, the last failed.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        w(&state, &format!(r#"{{"pid":902,"doing":"idle","last":{{"time":{},"error":"/mnt/backup isn't there"}},"last_ok":{}}}"#, now - 600, now - 5 * 86_400));
        let r = scan(&roots);
        let f = r.find("diskbackup:overdue").expect("overdue");
        assert_eq!(f.title, "Nothing backed up for 5 days");
        assert!(f.detail.contains("isn't there"), "{}", f.detail);
        // Paused: said, not overdue.
        w(&settings, r#"{"enabled":false,"target":{"kind":"folder","path":"/mnt/backup"},"key_in_passwords":true}"#);
        assert_eq!(ids(&scan(&roots)), ["diskbackup:paused"]);
        // Backed up an hour ago, the key kept: fine.
        w(&settings, r#"{"enabled":true,"target":{"kind":"folder","path":"/mnt/backup"},"key_in_passwords":true}"#);
        w(&state, &format!(r#"{{"pid":902,"doing":"idle","last_ok":{}}}"#, now - 3700));
        let r = scan(&roots);
        assert!(ids(&r).is_empty(), "{:?}", ids(&r));
        assert!(r.fine.iter().any(|f| f.title == "Your files are backed up" && f.detail.ends_with("/mnt/backup an hour ago.")), "{:?}", r.fine);
    }

    /// Programs' secrets in one login keyring: olo-secrets still having two
    /// found (its fix starts it again), and its mending said.
    #[test]
    fn one_login_keyring() {
        let (fs, roots) = machine("secrets");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w("proc/960/cmdline", "/home/me/.local/bin/olo-secrets\0");
        let status = format!("run/user/{}/olo-secrets.json", roots.uid);
        w(&status, r#"{"serving":true,"collections":[{"id":"login","label":"Login","items":3},{"id":"session","label":"Session","items":0},{"id":"Login","label":"Login","items":12}]}"#);
        let r = scan(&roots);
        let f = r.find("secrets:two-logins").expect("two found");
        assert_eq!(f.fix.as_ref().unwrap().steps, [Step::Signal { pid: 960, signal: libc::SIGTERM }]);
        assert!(after_fix("secrets:two-logins").is_some());
        // Mended as it started: said, nothing found.
        w(&status, r#"{"serving":true,"collections":[{"id":"login","label":"Login","items":13}],"healed":{"merged":["Login"],"duplicates":2,"schemas":2}}"#);
        let r = scan(&roots);
        assert!(r.find("secrets:two-logins").is_none());
        let fine = r.fine.iter().find(|f| f.title == "Programs' secrets are in one login keyring").expect("said");
        assert!(fine.detail.contains("a second login keyring merged in") && fine.detail.contains("2 secrets kept twice"), "{}", fine.detail);
    }

    /// Barlow Ridge's USB controller found: said, with a fix keeping it and
    /// its root hubs awake; once awake, fine.
    #[test]
    fn thunderbolt5_usb2() {
        let (fs, roots) = machine("tb5");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let dev = "sys/bus/pci/devices/0000:a9:00.0";
        for (f, v) in [("vendor", "0x8086"), ("device", "0x5782"), ("class", "0x0c0330"), ("power/control", "auto")] {
            w(&format!("{dev}/{f}"), v);
        }
        w(&format!("{dev}/usb5/speed"), "480");
        w(&format!("{dev}/usb6/speed"), "20000");
        let r = scan(&roots);
        let f = r.find("usb2:thunderbolt5:0000:a9:00.0").expect("said");
        let paths: Vec<String> = f
            .fix
            .as_ref()
            .unwrap()
            .steps
            .iter()
            .map(|s| match s {
                Step::WriteSysfs { path, value } => format!("{}={value}", path.display()),
                other => format!("{other:?}"),
            })
            .collect();
        // The rule that keeps it so at every start, and the value now.
        assert!(paths[0].contains("75-olo-thunderbolt-usb.rules"), "{paths:?}");
        assert_eq!(
            paths[1..],
            ["/sys/bus/pci/devices/0000:a9:00.0/power/control=on", "/sys/bus/pci/devices/0000:a9:00.0/usb5/power/control=on", "/sys/bus/pci/devices/0000:a9:00.0/usb6/power/control=on"]
        );
        // Awake for the day, by the value alone (as the fix once left
        // it): still said, for the rule.
        w(&format!("{dev}/power/control"), "on");
        let r = scan(&roots);
        let f = r.find("usb2:thunderbolt5:0000:a9:00.0").expect("awake only until the next start");
        assert!(matches!(&f.fix.as_ref().unwrap().steps[0], Step::WriteFile { content, .. } if content == THUNDERBOLT_USB_RULE));
        // With the rule: as it should be, and a way back.
        w("etc/udev/rules.d/75-olo-thunderbolt-usb.rules", THUNDERBOLT_USB_RULE);
        let r = scan(&roots);
        assert!(r.find("usb2:thunderbolt5:0000:a9:00.0").is_none());
        assert!(r.fine.iter().any(|f| f.title == "The Thunderbolt 5 ports' USB is kept awake"));
        let back = r.undo.iter().find(|u| u.id == "usb2:thunderbolt5:0000:a9:00.0:undo").expect("a way back");
        assert!(matches!(&back.fix.steps[0], Step::RemoveFile { path } if path == Path::new("/etc/udev/rules.d/75-olo-thunderbolt-usb.rules")));
        assert!(matches!(&back.fix.steps[1], Step::WriteSysfs { value, .. } if value == "auto"));
    }

    /// What's ignored is out of the counts and what's checked, still found
    /// and fixable by its id, and back among the rest when no longer
    /// ignored.
    #[test]
    fn ignored_findings_are_set_aside() {
        let (_fs, roots) = machine("ignored");
        let mut r = scan(&roots);
        let before = (r.problems(), r.fixable().count(), r.findings.len());
        let id = r.fixable().next().expect("a finding with a fix").id.clone();
        let problem = r.find(&id).unwrap().severity.is_problem();
        r.set_aside(&[id.clone()].into_iter().collect());
        assert!(r.findings.iter().all(|f| f.id != id) && r.ignored.iter().any(|f| f.id == id));
        assert_eq!((r.problems(), r.fixable().count(), r.findings.len()), (before.0 - usize::from(problem), before.1 - 1, before.2 - 1));
        assert!(r.find(&id).is_some() && r.action(&id).is_some(), "still found, still fixable");
        r.move_ignored(&id, false);
        assert_eq!((r.problems(), r.fixable().count(), r.findings.len()), before);
        assert!(r.ignored.is_empty());
    }

    /// Olo's login screen offered over GDM; in use and current, fine with
    /// a way back; in use with an older copy, its update offered.
    #[test]
    fn olo_s_login_screen_is_offered() {
        let (fs, roots) = machine("own-login");
        let w = |p: &Path, c: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        };
        // Not installed: nothing said.
        assert!(scan(&roots).find("login:own").is_none());
        for name in crate::login_screen::PROGRAMS {
            w(&roots.bin.join(name), "program");
        }
        w(&fs.join("usr/lib/systemd/system/gdm.service"), "[Unit]\n");
        std::fs::create_dir_all(fs.join("etc/systemd/system")).unwrap();
        std::os::unix::fs::symlink("/usr/lib/systemd/system/gdm.service", fs.join("etc/systemd/system/display-manager.service")).unwrap();
        let r = scan(&roots);
        let f = r.find("login:own").expect("offered");
        assert_eq!(f.title, "The login screen is GNOME's (GDM), not Olo's own");
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root);
        assert!(fix.steps.iter().any(|s| matches!(s, Step::WriteFile { path, .. } if path == Path::new("/etc/systemd/system/olo-login.service"))));
        assert!(fix.steps.iter().any(|s| matches!(s, Step::Unit { name, action: crate::fix::UnitAction::Disable } if name == "gdm.service")));
        assert!(matches!(fix.steps.last(), Some(Step::Unit { name, action: crate::fix::UnitAction::Enable }) if name == "olo-login.service"));
        assert_eq!(fix.steps.iter().filter(|s| matches!(s, Step::CopyFile { to, .. } if to.starts_with(crate::login_screen::LIBEXEC))).count(), 5);
        // Olo's in use, its copies made after the programs: fine, and the
        // way back to GDM.
        std::fs::remove_file(fs.join("etc/systemd/system/display-manager.service")).unwrap();
        std::os::unix::fs::symlink("/etc/systemd/system/olo-login.service", fs.join("etc/systemd/system/display-manager.service")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        for name in crate::login_screen::PROGRAMS {
            w(&fs.join("usr/local/libexec/olo").join(name), "copy");
        }
        let r = scan(&roots);
        assert!(r.find("login:own").is_none() && r.find("login:own-older").is_none());
        assert!(r.fine.iter().any(|f| f.title == "Olo's own login screen greets you"));
        let back = r.undo.iter().find(|u| u.id == "login:own-undo").expect("a way back");
        assert_eq!(back.fix.label, "Go Back to GDM");
        // An upgrade writes the programs again: the copies are older.
        std::thread::sleep(std::time::Duration::from_millis(20));
        w(&roots.bin.join("olo-greeter"), "newer");
        let r = scan(&roots);
        let f = r.find("login:own-older").expect("its update offered");
        assert!(f.detail.contains("olo-greeter") && !f.detail.contains("olo-session"), "{}", f.detail);
        assert_eq!(f.fix.as_ref().unwrap().label, "Update the Login Screen");
    }

    /// Cool, chosen: in force and said so; and each way it isn't, with
    /// what puts it right.
    #[test]
    fn a_cool_fan_profile_not_in_force_is_found() {
        use olo_system::fans::{CONF, Places};
        let (fs, roots) = machine("fans");
        let places = Places::under(&fs);
        let w = |path: PathBuf, c: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let ids = |r: &Report| r.findings.iter().filter(|f| f.id.starts_with("fans:")).map(|f| f.id.clone()).collect::<Vec<_>>();
        // Not a ThinkPad, then one with the firmware's curve: nothing said.
        assert!(ids(&scan(&roots)).is_empty());
        w(places.fan(), "status:\t\tenabled\nspeed:\t\t2880\nlevel:\t\tauto\n");
        w(places.fan_control(), "Y\n");
        w(roots.bin.join(crate::fans::PROGRAM), "the program");
        assert!(ids(&scan(&roots)).is_empty());
        // Cool, as Preferences puts it there, the service running.
        w(places.program(), "the program");
        w(places.unit(), crate::fans::UNIT);
        w(places.profile(), "cool\n");
        w(places.modprobe(), CONF);
        w(places.status(), "level=auto why=\n");
        let r = scan(&roots);
        assert!(ids(&r).is_empty(), "{:?}", ids(&r));
        assert!(r.fine.iter().any(|f| f.title == "The fans run ahead of the heat"));
        // The computer started again with a driver that takes no levels
        // and nothing telling it to (it had been told by hand).
        w(places.fan_control(), "N\n");
        std::fs::remove_file(places.modprobe()).unwrap();
        let r = scan(&roots);
        assert_eq!(ids(&r), ["fans:driver"]);
        let f = r.find("fans:driver").unwrap();
        assert_eq!((f.severity, f.fix.as_ref().unwrap().label.as_str()), (Severity::Problem, "Repair the Fan Profile"));
        assert!(matches!(f.fix.as_ref().unwrap().steps.as_slice(), [Step::WriteFile { path, content, .. }] if *path == places.modprobe() && content == CONF), "{:?}", f.fix);
        // Told to: the next start has it, and nothing is left to fix.
        w(places.modprobe(), CONF);
        let r = scan(&roots);
        assert_eq!(ids(&r), ["fans:next-start"]);
        assert!(r.find("fans:next-start").unwrap().fix.is_none());
        // The driver takes levels and the service isn't running.
        w(places.fan_control(), "Y\n");
        std::fs::remove_dir_all(places.running()).unwrap();
        let r = scan(&roots);
        assert_eq!(ids(&r), ["fans:stopped"]);
        assert!(matches!(r.find("fans:stopped").unwrap().fix.as_ref().unwrap().steps.as_slice(), [Step::Unit { action: UnitAction::Restart, .. }]));
        // Running, and older than the installed Olo.
        w(places.status(), "level=4 why=19 W\n");
        w(roots.bin.join(crate::fans::PROGRAM), "the program, a newer build's");
        let r = scan(&roots);
        assert_eq!(ids(&r), ["fans:older"]);
        let f = r.find("fans:older").unwrap();
        assert_eq!((f.severity, f.title.as_str(), f.fix.as_ref().unwrap().label.as_str()), (Severity::Advice, "Olo's fan service runs an older version", "Update the Fan Service"));
    }

    /// Olo's input rule has the last word on a USB input device's
    /// power: after the smart-card readers' rule, which turns
    /// autosuspend on for a security key as its driver binds. Where it
    /// was is out of date, and taken away as the new one is installed.
    #[test]
    fn the_input_rule_comes_after_the_smart_card_rule() {
        // (udev reads rule files by name, whatever folder each is in.)
        assert!("95-olo-input.rules" > "92_pcscd_ccid.rules");
        assert!(INPUT_RULE_PATH.ends_with("/95-olo-input.rules"));
        // The device itself, at each of its events (its driver's
        // binding, which follows its interfaces, among them).
        assert!(INPUT_RULE.lines().any(|l| l.starts_with("ACTION!=\"remove\"") && l.contains("usb_device") && l.contains("ID_USB_INTERFACES") && l.contains("ATTR{power/control}=\"on\"")));
        let (fs, roots) = machine("input-rule");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        // As an Olo from before installed it.
        let _ = std::fs::remove_file(fs.join(INPUT_RULE_PATH));
        w(INPUT_RULE_OLD_PATH, "ACTION==\"add|change\", SUBSYSTEM==\"usb\", ENV{DEVTYPE}==\"usb_interface\", ATTR{bInterfaceClass}==\"03\", ATTR{../power/control}=\"on\"\n");
        let r = scan(&roots);
        let f = r.find("rule:input").expect("out of date");
        assert_eq!(f.title, "Olo's input rule is out of date");
        let steps = &f.fix.as_ref().unwrap().steps;
        assert!(matches!(&steps[0], Step::WriteFile { path, content, .. } if path == Path::new("/etc/udev/rules.d/95-olo-input.rules") && content == INPUT_RULE));
        assert_eq!(steps[1], Step::RemoveFile { path: "/etc/udev/rules.d/71-olo-input.rules".into() });
        // Both there (the new one written, the old not yet gone): still
        // to do.
        w(INPUT_RULE_PATH, INPUT_RULE);
        assert!(scan(&roots).find("rule:input").is_some());
        std::fs::remove_file(fs.join(INPUT_RULE_OLD_PATH)).unwrap();
        let r = scan(&roots);
        assert!(r.find("rule:input").is_none());
        assert!(r.fine.iter().any(|f| f.title == "Olo's input rule is installed"));
    }

    #[test]
    fn a_login_screen_handed_over_is_said() {
        let (fs, roots) = machine("handed");
        assert!(scan(&roots).find("login:handed-over").is_none());
        std::fs::create_dir_all(fs.join("run/olo-login")).unwrap();
        std::fs::write(fs.join("run/olo-login/handed-over"), "Olo's login screen didn't show 3 times in a row.\n").unwrap();
        let f = scan(&roots).find("login:handed-over").cloned().unwrap();
        assert!(f.detail.starts_with("Olo's login screen didn't show 3 times in a row. Olo draws with the GPU"), "{}", f.detail);
    }

    #[test]
    fn gnome_s_services_found_and_quit() {
        let (fs, roots) = machine("gnome");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        // The user's services (no login's scope): dconf, gvfs, GNOME Keyring.
        w("proc/950/cmdline", "/usr/libexec/dconf-service\0");
        w("proc/951/cmdline", "/usr/libexec/gvfsd\0");
        w("proc/954/cmdline", "/usr/libexec/gvfsd\0");
        w("proc/952/cmdline", "/usr/bin/gnome-keyring-daemon\0--start\0");
        w("proc/953/cmdline", "/usr/libexec/xdg-desktop-portal\0");
        // An agent screen's GTK portal backend: its own session's, left.
        w("proc/955/cmdline", "/usr/libexec/xdg-desktop-portal-gtk\0");
        w("proc/955/environ", "OLO_SCREEN=claude-browser\0DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/olo-screens/claude-browser.bus\0");
        let f = scan(&roots).find("gnome:services").cloned().unwrap();
        // (GNOME Keyring too, whatever Olo's keyring is doing: nothing of
        // GNOME's is needed.)
        assert_eq!(f.title, "3 of GNOME's services run", "{}", f.detail);
        // (A program running twice is said once, with both processes.)
        assert!(f.detail.contains("(dconf-service, process 950)") && f.detail.contains("(gvfsd, processes 951 and 954)") && f.detail.contains("network folders"), "{}", f.detail);
        assert!(!f.detail.contains("xdg-desktop-portal-gtk"), "{}", f.detail);
        assert_eq!(f.fix.unwrap().steps, [950, 951, 954, 952].map(|pid| Step::Signal { pid, signal: libc::SIGTERM }));
    }

    #[test]
    fn finds_what_is_wrong_and_what_is_fine() {
        let (fs, mut roots) = machine("wrong");
        // Programs' temporary files in memory too.
        roots.temp = TempFiles { tmp_in_memory: true, config: fs.join("home/me/.config"), data: fs.join("home/me/.local/share"), ..TempFiles::default() };
        // (The account may make threads realtime: the voice services'
        // aren't, started before.)
        let limits = fs.join("proc/self/limits");
        std::fs::create_dir_all(limits.parent().unwrap()).unwrap();
        std::fs::write(&limits, "Max realtime priority     70                   70                   \nMax realtime timeout      unlimited            unlimited            us        \n").unwrap();
        let r = scan(&roots);
        let ids: Vec<&str> = r.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(r.find("foreign").unwrap().title, "A program of another desktop runs in this session");
        assert_eq!(
            ids,
            [
                "remapper:keyd",
                "foreign",
                "rule:input",
                "autosuspend:3-1.3",
                "rule:displays",
                "wifi-heal",
                "rtkit",
                "voice-realtime",
                "airpods-listening",
                "uclamp",
                "stale",
                "login",
                "tmp:memory",
                "diskbackup:not-set-up"
            ],
            "{r:#?}"
        );
        let voice = r.find("voice-realtime").unwrap();
        assert!(voice.detail.starts_with("olo-voice started before"), "{}", voice.detail);
        let fix = voice.fix.as_ref().unwrap();
        assert!(fix.label == "Make It Realtime" && !fix.root && fix.steps.is_empty());
        let uclamp = r.find("uclamp").unwrap();
        assert!(uclamp.detail.starts_with("kernel.sched_util_clamp_min is 512"));
        assert_eq!(uclamp.fix.as_ref().unwrap().steps[1], Step::WriteSysfs { path: "/proc/sys/kernel/sched_util_clamp_min".into(), value: "1024".into() });
        let heal = r.find("wifi-heal").unwrap();
        assert_eq!(heal.title, "Olo's Wi-Fi and Bluetooth healer isn't installed");
        assert!(heal.detail.contains("brcmfmac, wld0"));
        let steps = &heal.fix.as_ref().unwrap().steps;
        assert_eq!(steps[0], Step::CopyFile { from: roots.bin.join("olo-wifi-heal"), to: "/usr/local/libexec/olo-wifi-heal".into(), mode: 0o755 });
        assert!(matches!(steps.last(), Some(Step::NetworkManagerReload)));
        let keyd = r.find("remapper:keyd").unwrap();
        assert_eq!(keyd.title, "keyd is running and starts at boot");
        assert!(keyd.detail.contains("keyd virtual keyboard, keyd virtual pointer"));
        let fix = keyd.fix.as_ref().unwrap();
        assert_eq!(fix.label, "Stop and Uninstall keyd");
        assert!(fix.root);
        assert_eq!(
            fix.steps,
            vec![
                Step::Unit { name: "keyd.service".into(), action: UnitAction::Stop },
                Step::Unit { name: "keyd.service".into(), action: UnitAction::Disable },
                Step::RemoveFile { path: fs.join("usr/local/bin/keyd") },
                Step::RemoveFile { path: fs.join("usr/local/lib/systemd/system/keyd.service") },
                Step::Unit { name: "keyd.service".into(), action: UnitAction::Reload },
            ]
        );
        let mouse = r.find("autosuspend:3-1.3").unwrap();
        assert_eq!(mouse.title, "Logitech G309 (Logitech USB Receiver) may be put to sleep");
        let steps = &mouse.fix.as_ref().unwrap().steps;
        assert!(matches!(&steps[0], Step::WriteFile { path, content, .. } if path == Path::new("/etc/udev/rules.d/95-olo-input.rules") && content == INPUT_RULE));
        assert_eq!(steps[1], Step::WriteSysfs { path: "/sys/bus/usb/devices/3-1.3/power/control".into(), value: "on".into() });
        // The display is a HID device without input nodes: not a finding
        // of its own, but the display rule applies to it.
        let displays = r.find("rule:displays").unwrap();
        assert_eq!(displays.severity, Severity::Problem);
        assert!(displays.detail.contains("Studio Display"));
        assert!(matches!(&displays.fix.as_ref().unwrap().steps[1], Step::WriteSysfs { path, value } if path == Path::new("/sys/bus/usb/devices/3-1.4/uevent") && value == "change"));
        let rtkit = r.find("rtkit").unwrap();
        assert_eq!(rtkit.fix.as_ref().unwrap().steps, vec![Step::Unit { name: "rtkit-daemon.service".into(), action: UnitAction::Start }]);
        let stale = r.find("stale").unwrap();
        assert!(stale.detail.starts_with("olo-deskbar (900)"));
        assert!(!stale.fix.as_ref().unwrap().root);
        // A command a shell started (a script run through olo-sudo before
        // the upgrade) isn't the session's to quit: not among them. The
        // same program started by the session is.
        let command = |pid: i32, parent: i32, parent_exe: &str, parent_comm: &str| {
            for (path, text) in [
                (format!("proc/{pid}/cmdline"), "sudo\0-n\0/var/tmp/job.sh\0".to_owned()),
                (format!("proc/{pid}/stat"), format!("{pid} (sudo) S {parent} 1 1 0\n")),
                (format!("proc/{parent}/comm"), format!("{parent_comm}\n")),
            ] {
                let path = fs.join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            let _ = std::fs::remove_file(fs.join(format!("proc/{pid}/exe")));
            let _ = std::fs::remove_file(fs.join(format!("proc/{parent}/exe")));
            std::os::unix::fs::symlink(format!("{}/olo-sudo (deleted)", roots.bin.display()), fs.join(format!("proc/{pid}/exe"))).unwrap();
            std::os::unix::fs::symlink(parent_exe, fs.join(format!("proc/{parent}/exe"))).unwrap();
        };
        command(930, 931, "/usr/bin/bash", "bash");
        let r = scan(&roots);
        let stale = r.find("stale").unwrap();
        assert!(!stale.detail.contains("930") && stale.fix.as_ref().unwrap().steps == vec![Step::Signal { pid: 900, signal: libc::SIGTERM }], "{}", stale.detail);
        command(932, 933, &format!("{}/olo-session", roots.bin.display()), "olo-session");
        let stale = scan(&roots).find("stale").cloned().unwrap();
        assert!(stale.detail.contains("(932)") && !stale.detail.contains("(930)"), "{}", stale.detail);
        // With nothing else old, it's fine, and says whose the command is.
        std::fs::remove_dir_all(fs.join("proc/932")).unwrap();
        std::fs::remove_file(fs.join("proc/900/exe")).unwrap();
        let r = scan(&roots);
        assert!(r.find("stale").is_none());
        let fine = r.fine.iter().find(|f| f.title == "Every Olo program runs the installed version").expect("fine");
        assert!(fine.detail.starts_with("sudo (930), started by bash: a command from before the upgrade, which ends with what it's doing"), "{}", fine.detail);
        std::os::unix::fs::symlink(format!("{}/olo-deskbar (deleted)", roots.bin.display()), fs.join("proc/900/exe")).unwrap();
        for pid in [930, 931, 933] {
            std::fs::remove_dir_all(fs.join(format!("proc/{pid}"))).unwrap();
        }
        let r = scan(&roots);
        let foreign = r.find("foreign").unwrap();
        assert!(foreign.detail.contains("GNOME's power daemon (gsd-power, process 901)"));
        let login = r.find("login").unwrap();
        assert!(
            matches!(&login.fix.as_ref().unwrap().steps[0], Step::WriteFile { content, .. } if content.contains(&format!("Exec={}/olo-session\n", roots.bin.display())) && !content.contains("TryExec"))
        );
        // Written where GDM always looks (the old one, in /usr/local, read
        // only when GDM's XDG_DATA_DIRS names it).
        assert!(matches!(&login.fix.as_ref().unwrap().steps[0], Step::WriteFile { path, .. } if path == Path::new("/usr/share/wayland-sessions/olo.desktop")));
        // (Only the account's leave for realtime, given above, is fine.)
        assert!(r.fine.iter().all(|f| f.title.starts_with("Olo's threads are realtime")), "{:?}", r.fine);
        let _ = std::fs::remove_dir_all(&fs);
    }

    /// BlueFerry, where it's installed: offered for removal, with Olo's
    /// own Bluetooth drop-in back as it is without it.
    #[test]
    fn blueferry_installed_is_offered_for_removal() {
        let (fs, roots) = machine("phone");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let found = |roots: &Roots| scan(roots).findings.into_iter().filter(|f| f.id.starts_with("phone-")).collect::<Vec<_>>();
        assert_eq!(found(&roots).len(), 0, "not installed: nothing said (and never suggested)");
        w("usr/share/dbus-1/services/io.weirdware.BlueFerry.service", "[D-BUS Service]\n");
        let f = found(&roots);
        assert_eq!((f.len(), f[0].id.as_str(), f[0].severity), (1, "phone-blueferry", Severity::Advice));
        let fix = f[0].fix.as_ref().unwrap();
        assert!(fix.root);
        assert_eq!(fix.steps, [Step::RemovePackages { names: vec!["blueferry-backend".into(), "blueferry-gtk".into(), "blueferry-qt".into(), "blueferry-quickshell".into()] }]);
        // Olo's drop-in as an earlier Doctor wrote it for BlueFerry:
        // back to what AirPods alone need, and Bluetooth started so.
        w(BLUETOOTH_DROP_IN, "# Olo\n[Service]\nExecStart=\nExecStart=/usr/libexec/bluetooth/bluetoothd --experimental -P hfp\n");
        let f = found(&roots);
        let steps = &f[0].fix.as_ref().unwrap().steps;
        assert!(
            matches!(&steps[1], Step::WriteFile { path, content, .. } if path == Path::new("/etc/systemd/system/bluetooth.service.d/olo.conf") && content.ends_with("ExecStart=/usr/libexec/bluetooth/bluetoothd --experimental\n")),
            "{steps:?}"
        );
        assert_eq!(steps[2..], [Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Reload }, Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Restart }]);
        let _ = std::fs::remove_dir_all(&fs);
    }

    /// A phone paired: the phone service running with its profiles
    /// registered is fine; not running, or its profiles not registered
    /// (another program has them; Bluetooth not answering), a problem.
    #[test]
    fn a_paired_phone_needs_the_service_and_its_profiles() {
        let (fs, mut roots) = machine("phone-link");
        roots.temp.runtime = fs.join("run/user/1000");
        std::fs::create_dir_all(&roots.temp.runtime).unwrap();
        let found = |roots: &Roots| scan(roots).findings.into_iter().filter(|f| f.id.starts_with("phone-")).map(|f| f.id).collect::<Vec<_>>();
        let state = |pid: u32, profiles: bool| {
            let s = olo_protocol::phone::PhoneState {
                pid,
                name: "Mira’s iPhone".into(),
                profiles,
                trouble: if profiles { String::new() } else { "Another program has the phone’s message profiles.".into() },
                ..Default::default()
            };
            std::fs::write(roots.temp.runtime.join("olo-phone.json"), serde_json::to_string(&s).unwrap()).unwrap();
        };
        assert!(found(&roots).is_empty(), "no phone paired: nothing said");
        roots.phone = true;
        assert_eq!(found(&roots), ["phone-service"], "no state: not running");
        // (A process that's there: this test's own; one that isn't.)
        std::fs::create_dir_all(fs.join("proc/4242")).unwrap();
        state(4243, true);
        assert_eq!(found(&roots), ["phone-service"], "its process gone");
        state(4242, false);
        let f = scan(&roots).findings.into_iter().find(|f| f.id == "phone-profiles").unwrap();
        assert_eq!(f.severity, Severity::Problem);
        assert!(f.detail.contains("Another program has the phone’s message profiles."), "{}", f.detail);
        state(4242, true);
        let report = scan(&roots);
        assert!(report.findings.iter().all(|f| !f.id.starts_with("phone-")));
        assert!(report.fine.iter().any(|f| f.title == "The phone’s messages can reach Olo"));
        // The phone connected and its messages not opened for minutes:
        // said, with why, and the service started afresh by the fix.
        let mut stalled = |since: u64, connected: bool| {
            let s = olo_protocol::phone::PhoneState {
                pid: 4242,
                name: "Mira’s iPhone".into(),
                profiles: true,
                link: olo_protocol::phone::Link::Connecting,
                since,
                trouble: "Bluetooth couldn’t open the phone’s messages (br-connection-busy).".into(),
                ..Default::default()
            };
            std::fs::write(roots.temp.runtime.join("olo-phone.json"), serde_json::to_string(&s).unwrap()).unwrap();
            roots.phone_connected = connected;
            scan(&roots).findings.into_iter().find(|f| f.id == "phone-messages")
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(stalled(now - 30, true).is_none(), "half a minute: it's still trying");
        assert!(stalled(now - 600, false).is_none(), "the phone not connected: out of reach, nothing wrong");
        let f = stalled(now - 600, true).expect("said");
        assert!(f.detail.contains("for 10 minutes") && f.detail.contains("br-connection-busy"), "{}", f.detail);
        assert_eq!(f.fix.unwrap().steps, [Step::Signal { pid: 4242, signal: libc::SIGTERM }]);
        let _ = std::fs::remove_dir_all(&fs);
    }

    /// An iPhone paired: BlueZ set to present the computer as hands-free,
    /// its settings otherwise as they were.
    #[test]
    fn an_iphone_wants_a_hands_free_computer() {
        let (fs, mut roots) = machine("phone-class");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let found = |roots: &Roots| scan(roots).findings.into_iter().find(|f| f.id == "phone-class");
        assert!(found(&roots).is_none(), "no Bluetooth: nothing said");
        // Bluetooth, no iPhone paired yet: suggested (it must be so before
        // one pairs); one paired: a problem.
        std::fs::create_dir_all(fs.join("sys/class/bluetooth/hci0")).unwrap();
        roots.bluetooth_class = Some(0x6c010c);
        assert_eq!(found(&roots).map(|f| f.severity), Some(Severity::Advice));
        roots.iphone = true;
        let conf = "[General]\n# Default device class.\n#Class = 0x000100\n#DiscoverableTimeout = 0\n\n[BR]\n#PageScanType = \n";
        w(BLUEZ_CONF, conf);
        w("proc/sys/kernel/hostname", "fedora\n");
        let f = found(&roots).expect("said");
        assert_eq!(f.severity, Severity::Problem);
        let fix = f.fix.unwrap();
        assert!(fix.root);
        let Step::WriteFile { content, .. } = &fix.steps[0] else { panic!("{:?}", fix.steps) };
        // The class after its commented example; the name the hostname
        // plugin gave the adapter at the section's end.
        assert_eq!(content, "[General]\n# Default device class.\n#Class = 0x000100\nClass = 0x000408\n#DiscoverableTimeout = 0\n\nName = fedora\n[BR]\n#PageScanType = \n");
        // Bluetooth without its hostname plugin (which puts a laptop's
        // class over it), started so.
        let Step::WriteFile { path, content: drop_in, .. } = &fix.steps[1] else { panic!("{:?}", fix.steps) };
        assert_eq!(path, Path::new("/etc/systemd/system/bluetooth.service.d/olo.conf"));
        assert!(drop_in.ends_with("ExecStart=/usr/libexec/bluetooth/bluetoothd --experimental -P hostname\n"), "{drop_in}");
        assert_eq!(fix.steps[2..], [Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Reload }, Step::Unit { name: "bluetooth.service".into(), action: UnitAction::Restart }]);
        // The settings set and the adapter still a laptop (Ben's: the
        // hostname plugin's class over it): still said, the drop-in
        // still needed.
        w(BLUEZ_CONF, content);
        assert_eq!(found(&roots).unwrap().fix.unwrap().steps.len(), 4);
        // The adapter says hands-free (service bits with it): fine.
        roots.bluetooth_class = Some(0x240408);
        assert!(found(&roots).is_none());
        // Not known (BlueZ not answering): by the settings and the drop-in.
        roots.bluetooth_class = None;
        assert!(found(&roots).is_some(), "the drop-in isn't there yet");
        w(BLUETOOTH_DROP_IN, &bluetooth_drop_in("/usr/libexec/bluetooth/bluetoothd", true));
        assert!(found(&roots).is_none());
        // A name already set is kept; another class replaced, in place.
        assert_eq!(with_bluez("[General]\nName = x\nClass = 0x00010c\n[LE]\n", "Class", "0x000408"), "[General]\nName = x\nClass = 0x000408\n[LE]\n");
        assert_eq!(with_bluez("[General]\nName = x\n[LE]\nClass = 1\n", "Class", "0x000408"), "[General]\nName = x\nClass = 0x000408\n[LE]\nClass = 1\n");
        assert_eq!(with_bluez("", "Class", "0x000408"), "[General]\nClass = 0x000408\n");
        assert_eq!(bluez_class("[General]\nClass = 0x240408\n"), Some(0x240408));
        assert_eq!(bluez_class("[LE]\nClass = 0x000408\n"), None);
        let _ = std::fs::remove_dir_all(&fs);
    }

    #[test]
    fn a_well_set_up_machine_is_all_fine() {
        let (fs, mut roots) = machine("fine");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w("proc/self/limits", "Max realtime priority     70                   70                   \nMax realtime timeout      30000000             31000000             us        \n");
        std::fs::remove_dir_all(fs.join("proc/772")).unwrap();
        std::fs::remove_dir_all(fs.join("proc/900")).unwrap();
        std::fs::remove_dir_all(fs.join("proc/901")).unwrap();
        w("sys/bus/usb/devices/3-1.3/power/control", "on\n");
        w("proc/903/cmdline", "/usr/libexec/bluetooth/bluetoothd\0--experimental\0");
        // The voice service's sound thread realtime (SCHED_RR, field 41).
        w("proc/902/task/905/stat", &format!("905 (data-loop.0) S{} 2\n", " 0".repeat(37)));
        w(INPUT_RULE_PATH, INPUT_RULE);
        w(DISPLAY_RULE_PATH, DISPLAY_RULE);
        w(SESSION_ENTRY_PATH, &format!("[Desktop Entry]\nExec={}/olo-session\n", roots.bin.display()));
        w(HEAL_BIN_PATH, "the healer");
        w(HEAL_UNIT_PATH, HEAL_UNIT);
        w(BRCMFMAC_CONF_PATH, BRCMFMAC_CONF);
        w(UTIL_CLAMP_PATH, "1024\n");
        roots.units = Box::new(|name: &str| {
            Ok(if matches!(name, "rtkit-daemon.service" | "olo-wifi-heal.service") { UnitState { loaded: true, active: true, enabled: true, path: None } } else { UnitState::default() })
        });
        let r = scan(&roots);
        assert!(r.findings.is_empty(), "{:#?}", r.findings);
        let titles: Vec<&str> = r.fine.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "No input remapper is running",
                "No other desktop's daemons run in this session",
                "Nothing of GNOME's runs",
                "Olo's input rule is installed",
                "1 USB input device stays awake",
                "Olo's display rule is installed",
                "Olo's healer watches wld0",
                "rtkit is running",
                "Olo's threads are realtime, with no time limit",
                "Voice processing's sound is realtime",
                "Bluetooth listens for the AirPods",
                "Frames run on the fast cores",
                "Every Olo program runs the installed version",
                "Olo is on the login screen",
                "Temporary files are kept on disk",
            ]
        );
        let _ = std::fs::remove_dir_all(&fs);
    }

    #[test]
    fn a_security_key_the_person_cant_open() {
        let (fs, roots) = machine("fido");
        let w = |p: &str, c: &[u8]| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        // A YubiKey's FIDO interface, and its OTP keyboard (not a key's).
        w("sys/class/hidraw/hidraw4/device/report_descriptor", &[0x06, 0xD0, 0xF1, 0x09, 0x01, 0xA1, 0x01]);
        w("sys/class/hidraw/hidraw4/device/uevent", b"HID_ID=0003:00001050:00000407\nHID_NAME=Yubico YubiKey OTP+FIDO+CCID\n");
        w("sys/class/hidraw/hidraw3/device/report_descriptor", &[0x05, 0x01, 0x09, 0x06, 0xA1, 0x01]);
        w("dev/hidraw4", b"");
        w("dev/hidraw3", b"");
        let ours = |r: &Report| r.findings.iter().any(|f| f.id == "rule:security-keys");
        // Its node the person's: fine.
        let r = scan(&roots);
        assert!(!ours(&r) && r.fine.iter().any(|f| f.title == "The security key plugged in can be used"), "{:#?}", r.fine);
        // Root's alone (as a test, not root): the rule, applied to it.
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(fs.join("dev/hidraw4"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = scan(&roots);
        if unsafe { libc::geteuid() } != 0 {
            let f = r.findings.iter().find(|f| f.id == "rule:security-keys").expect("the rule offered");
            let steps = &f.fix.as_ref().unwrap().steps;
            assert!(matches!(&steps[0], Step::WriteFile { content, .. } if content == SECURITY_KEY_RULE));
            assert!(matches!(&steps[1], Step::WriteSysfs { path, .. } if path == Path::new("/sys/class/hidraw/hidraw4/uevent")));
            assert_eq!(steps.len(), 2);
        }
        let _ = std::fs::remove_dir_all(&fs);
    }

    /// The power button's helper: offered where there's a power button,
    /// its unit and rule written and systemd reloaded; fine once its unit
    /// is this build's; an older unit, updated.
    #[test]
    fn the_power_buttons_helper() {
        let (fs, roots) = machine("power-now");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let ours = |r: &Report| r.findings.iter().find(|f| f.id == "power-now").cloned();
        // No power button (an agent's screen, a server): nothing said.
        assert!(ours(&scan(&roots)).is_none());
        w("sys/class/input/input2/name", "Power Button\n");
        let f = ours(&scan(&roots)).expect("offered");
        assert_eq!(f.title, "The power button can't restart or shut down at once");
        let steps = &f.fix.as_ref().unwrap().steps;
        assert!(matches!(&steps[0], Step::WriteFile { path, content, .. } if path == Path::new("/etc/systemd/system/olo-power-now@.service") && content == POWER_NOW_UNIT));
        assert!(matches!(&steps[1], Step::WriteFile { path, content, .. } if path == Path::new("/etc/polkit-1/rules.d/49-olo-power-now.rules") && content == POWER_NOW_RULES));
        assert!(matches!(&steps[2], Step::Unit { action: UnitAction::Reload, .. }));
        w("etc/systemd/system/olo-power-now@.service", "an older one");
        assert_eq!(ours(&scan(&roots)).unwrap().title, "Olo's power button helper is out of date");
        w("etc/systemd/system/olo-power-now@.service", POWER_NOW_UNIT);
        let r = scan(&roots);
        assert!(ours(&r).is_none() && r.fine.iter().any(|f| f.title == "The power button can restart and shut down at once"));
        let _ = std::fs::remove_dir_all(&fs);
    }

    /// Programs' "show in folder" while no Tracker answers: Nautilus on
    /// Fedora, unless the user's own activation file starts Tracker.
    #[test]
    fn programs_show_in_folder_with_tracker() {
        let (fs, mut roots) = machine("filemanager");
        roots.temp.data = fs.join("home/me/.local/share");
        std::fs::write(roots.bin.join("olo-tracker"), "").unwrap();
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w("usr/share/dbus-1/services/org.freedesktop.FileManager1.service", "[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec=/usr/bin/nautilus --gapplication-service\n");
        let r = scan(&roots);
        let f = r.findings.iter().find(|f| f.id == "dbus:file-manager").expect("offered");
        assert_eq!(f.title, "Programs may open GNOME's Files instead of Tracker");
        let fix = f.fix.as_ref().unwrap();
        assert!(!fix.root, "the user's own file");
        let Step::WriteFile { path, content, .. } = &fix.steps[0] else { panic!("{:?}", fix.steps) };
        assert_eq!(*path, fs.join("home/me/.local/share/dbus-1/services/org.freedesktop.FileManager1.service"));
        assert!(content.contains(&format!("\nName=org.freedesktop.FileManager1\nExec={} --file-manager\n", roots.bin.join("olo-tracker").display())), "{content}");
        assert_eq!(fix.steps[1], Step::SessionBusReload);
        // Done (the bus left alone: a test), it's fine.
        fix.steps[0].run().unwrap();
        let r = scan(&roots);
        assert!(!r.findings.iter().any(|f| f.id == "dbus:file-manager"));
        assert!(r.fine.iter().any(|f| f.title == "Programs' “Show in folder” opens Tracker"));
        // Another file manager in the user's own file: named, and replaced.
        w("home/me/.local/share/dbus-1/services/org.freedesktop.FileManager1.service", "[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec=/usr/bin/dolphin\n");
        let r = scan(&roots);
        assert_eq!(r.findings.iter().find(|f| f.id == "dbus:file-manager").map(|f| f.title.as_str()), Some("Programs may open dolphin instead of Tracker"));
        // In an Olo session, which names Tracker for it in the runtime
        // folder (read before the user's own): fine, whatever the user's.
        roots.temp.runtime = fs.join("run/user/1000");
        w("run/user/1000/dbus-1/services/org.freedesktop.FileManager1.service", "[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec=/home/me/.local/bin/olo-tracker --file-manager\n");
        let r = scan(&roots);
        assert!(!r.findings.iter().any(|f| f.id == "dbus:file-manager"));
        assert!(r.fine.iter().any(|f| f.title == "Programs' “Show in folder” opens Tracker" && f.detail.contains("While Olo runs")));
        let _ = std::fs::remove_dir_all(&fs);
    }

    /// Olo's keyring: locked said (with how to unlock), open but not sealed
    /// by the login password advised, sealed fine; nothing of GNOME's
    /// offered.
    #[test]
    fn the_login_keyring_opens_at_login() {
        let (fs, roots) = machine("keyring");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let status = format!("run/user/{}/olo-secrets.json", roots.uid);
        w(&status, r#"{"serving":false,"locked":true,"error":"the secrets stay locked: the login password wasn't given"}"#);
        let mut r = Report::default();
        login_keyring(&roots, &mut r);
        let f = r.find("keyring:locked").expect("locked said");
        assert!(f.detail.contains("Preferences ▸ Security") && f.fix.is_none(), "{}", f.detail);
        w(&status, r#"{"serving":true}"#);
        let mut r = Report::default();
        login_keyring(&roots, &mut r);
        assert!(r.find("keyring:unsealed").is_some());
        w("home/me/.config/olo/secrets.login", "sealed");
        let mut r = Report::default();
        login_keyring(&roots, &mut r);
        assert!(r.findings.is_empty());
        assert_eq!(r.fine[0].title, "Olo's keyring opens when you log in");
    }

    #[test]
    fn the_built_in_panels_hdr_at_start() {
        let (fs, roots) = machine("panel-hdr");
        let w = |p: &str, c: &[u8]| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        // The ThinkPad's OLED, on Intel's xe.
        let oled = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/system/tests/edid/thinkpad-p1-gen9-oled.bin")).unwrap();
        w("sys/class/drm/card0-eDP-1/status", b"connected\n");
        w("sys/class/drm/card0-eDP-1/edid", &oled);
        std::fs::create_dir_all(fs.join("sys/bus/pci/drivers/xe")).unwrap();
        std::fs::create_dir_all(fs.join("sys/class/drm/card0/device")).unwrap();
        std::os::unix::fs::symlink(fs.join("sys/bus/pci/drivers/xe"), fs.join("sys/class/drm/card0/device/driver")).unwrap();
        w("proc/cmdline", b"root=UUID=1 ro rhgb quiet\n");
        let mut r = Report::default();
        panel_hdr(&roots, &mut r);
        let f = r.find("boot:panel-hdr").expect("HDR off at start");
        assert!(f.detail.contains("1600 cd/m²"), "{}", f.detail);
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root);
        assert_eq!(fix.steps, vec![Step::AddKernelArgument { arg: "xe.enable_dpcd_backlight=3".into(), root: "/".into() }]);
        // Started so: fine.
        w("proc/cmdline", b"root=UUID=1 ro xe.enable_dpcd_backlight=3\n");
        let mut r = Report::default();
        panel_hdr(&roots, &mut r);
        assert!(r.findings.is_empty());
        assert_eq!(r.fine[0].title, "The built-in display's HDR is the kernel's to switch");
    }

    #[test]
    fn a_thunderbolt_dock_waiting_is_allowed() {
        let (fs, roots) = machine("thunderbolt");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let tb = "sys/bus/thunderbolt/devices";
        // The host (allowed, not a device to allow), a dock waiting.
        w(&format!("{tb}/domain0/security"), "user\n");
        w(&format!("{tb}/0-0/authorized"), "1\n");
        w(&format!("{tb}/0-1/authorized"), "0\n");
        w(&format!("{tb}/0-1/unique_id"), "c4010000-0080-7e18-23c0-5a0c2c111122\n");
        w(&format!("{tb}/0-1/vendor_name"), "Lenovo\n");
        w(&format!("{tb}/0-1/device_name"), "ThinkPad Thunderbolt 4 Dock\n");
        let mut r = Report::default();
        thunderbolt(&roots, &mut r);
        let f = r.find("thunderbolt:0-1").expect("the dock");
        assert_eq!(f.title, "Lenovo ThinkPad Thunderbolt 4 Dock is plugged in but not allowed");
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root);
        // (Remembered by boltd, by its unique id: let in from now on.)
        assert_eq!(fix.steps, vec![Step::AllowThunderbolt { device: "0-1".into(), uid: "c4010000-0080-7e18-23c0-5a0c2c111122".into() }]);
        // Allowed: fine.
        w(&format!("{tb}/0-1/authorized"), "1\n");
        let mut r = Report::default();
        thunderbolt(&roots, &mut r);
        assert!(r.findings.is_empty());
        assert_eq!(r.fine[0].title, "Thunderbolt devices are let in");
    }

    /// Intel's xe driver with swap on: its windows' buffers can be moved
    /// out, so swap is turned off (what turns it on at each start first:
    /// zram-generator's file, /etc/fstab's lines); fine with none; and
    /// nothing said of another driver's.
    #[test]
    fn xe_and_swap() {
        let (fs, roots) = machine("graphics-swap");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let heading = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n";
        w("sys/bus/pci/devices/0000:00:02.0/class", "0x030000\n");
        w("sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
        w("proc/swaps", &format!("{heading}/dev/zram0                              partition\t8388604\t\t6403060\t\t100\n"));
        // Another driver's (i915): nothing said.
        let driver = |name: &str| {
            let link = fs.join("sys/bus/pci/devices/0000:00:02.0/driver");
            let _ = std::fs::remove_file(&link);
            std::fs::create_dir_all(fs.join("sys/bus/pci/drivers").join(name)).unwrap();
            std::os::unix::fs::symlink(fs.join("sys/bus/pci/drivers").join(name), link).unwrap();
        };
        driver("i915");
        let r = scan(&roots);
        assert!(r.find("graphics-swap").is_none() && r.fine.iter().all(|f| !f.title.starts_with("Windows' graphics memory")));
        // xe's, Fedora's zram swap on: off, and kept off.
        driver("xe");
        let r = scan(&roots);
        let f = r.find("graphics-swap").expect("found");
        assert_eq!((f.severity, f.title.as_str()), (Severity::Problem, "Windows' graphics memory is moved out to swap"));
        assert!(f.detail.starts_with("Swap is on (8.59\u{a0}GB in compressed memory), and Intel's graphics driver (xe) moves windows' buffers out to it"), "{}", f.detail);
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root && fix.label == "Turn Swap Off");
        assert_eq!(
            fix.steps,
            [
                Step::WriteFile { path: "/etc/systemd/zram-generator.conf".into(), content: ZRAM_OFF_CONF.into(), mode: 0o644 },
                Step::Unit { name: "swap".into(), action: UnitAction::Reload },
                Step::SwapOff
            ]
        );
        // (The file names no device: zram-generator makes none.)
        assert!(ZRAM_OFF_CONF.lines().all(|l| l.starts_with('#')));
        // A partition's beside it: /etc/fstab's line too.
        w("proc/swaps", &format!("{heading}/dev/zram0 partition 8388604 0 100\n/dev/nvme0n1p3 partition 16777212 0 -2\n"));
        let f = scan(&roots).find("graphics-swap").unwrap().clone();
        assert!(f.detail.contains("in compressed memory and on disk"), "{}", f.detail);
        assert_eq!(
            f.fix.unwrap().steps.iter().map(Step::describe).collect::<Vec<_>>(),
            ["Writing /etc/systemd/zram-generator.conf", "Taking swap out of /etc/fstab", "Reloading systemd for swap", "Turning swap off"]
        );
        // zram's set-up service running (as it is while its swap is on):
        // stopped last, its device given back.
        let roots = Roots { units: Box::new(|name: &str| if name == "systemd-zram-setup@zram0.service" { Ok(UnitState { active: true, ..Default::default() }) } else { Err("none".into()) }), ..roots };
        assert_eq!(scan(&roots).find("graphics-swap").unwrap().fix.as_ref().unwrap().steps.last().map(Step::describe).as_deref(), Some("Stopping systemd-zram-setup@zram0.service"));
        // None on: fine, and said.
        w("proc/swaps", heading);
        let r = scan(&roots);
        assert!(r.find("graphics-swap").is_none());
        assert!(r.fine.iter().any(|f| f.title == "Windows' graphics memory stays in memory"));
    }

    /// TCP's congestion control: BBRv3 offered where it isn't (Olo's
    /// module and its service, what building needs first where it's
    /// missing); fine once in use and kept; kept at each start where it
    /// was only loaded by hand; the kernel's own chosen where that is
    /// BBRv3; nothing offered where the kernel can't take the module.
    #[test]
    fn the_kernels_settings_are_offered_and_set() {
        use olo_system::kernel_tuning as tuning;
        let (fs, roots) = machine("kernel");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let ids = |roots: &Roots| scan(roots).findings.iter().filter(|f| f.id.starts_with("kernel:")).map(|f| f.id.clone()).collect::<Vec<_>>();
        // A kernel that says nothing of any of it: nothing offered.
        assert!(ids(&roots).is_empty());
        // Fedora's, as it comes, with a stick plugged in.
        w("proc/sys/kernel/osrelease", "7.2.15-300.fc45.x86_64\n");
        w("lib/modules/7.2.15-300.fc45.x86_64/config", "CONFIG_PREEMPT_LAZY=y\nCONFIG_PREEMPT_DYNAMIC=y\n");
        w("proc/cmdline", "BOOT_IMAGE=(hd0,gpt2)/vmlinuz ro rhgb quiet\n");
        w("etc/kernel/cmdline", "root=UUID=1 ro rhgb quiet\n");
        w("sys/kernel/debug/sched/preempt", "none voluntary full (lazy)\n");
        w("proc/vmstat", "nr_dirty 100\nnr_writeback 0\nnr_dirty_threshold 3748981\noom_kill 0\n");
        // Sixteen processors.
        w("sys/devices/system/cpu/online", "0-15\n");
        w("sys/kernel/debug/sched/base_slice_ns", "2800000\n");
        for (name, removable) in [("nvme0n1", "0"), ("sdb", "1")] {
            w(&format!("sys/block/{name}/removable"), removable);
            w(&format!("sys/block/{name}/size"), "60000");
            w(&format!("sys/block/{name}/bdi/strict_limit"), "0\n");
            w(&format!("sys/block/{name}/bdi/max_bytes"), "15355826176\n");
            w(&format!("sys/block/{name}/bdi/max_ratio"), "100\n");
        }
        assert_eq!(ids(&roots), ["kernel:preemption", "kernel:short-turns", "kernel:slow-disks"]);
        let r = scan(&roots);
        for (id, label, severity) in [
            ("kernel:preemption", "Preempt at Once", Severity::Suggestion),
            ("kernel:short-turns", "Keep Turns Short", Severity::Advice),
            ("kernel:slow-disks", "Limit Removable Disks", Severity::Suggestion),
        ] {
            let f = r.find(id).unwrap();
            assert_eq!((f.severity, f.fix.as_ref().unwrap().label.as_str(), f.fix.as_ref().unwrap().root), (severity, label, true), "{id}");
        }
        let turns = &r.find("kernel:short-turns").unwrap().detail;
        assert!(turns.starts_with("A busy program keeps a processor for 2.8 ms here") && turns.ends_with("The fix sets it for every start, and now."), "{turns}");
        assert!(r.find("kernel:preemption").unwrap().detail.ends_with("from the next restart, and switches the running kernel now."));
        assert!(r.find("kernel:slow-disks").unwrap().detail.starts_with("Up to 14.3 GB written to a USB stick"), "{}", r.find("kernel:slow-disks").unwrap().detail);
        assert!(r.find("kernel:slow-disks").unwrap().detail.ends_with("The fix limits those here now and installs a rule that limits each as it's plugged in."));
        // Each fix, run (the tree is ours to write), leaves it fine.
        for id in ["kernel:preemption", "kernel:short-turns", "kernel:slow-disks"] {
            for done in crate::helper::run_steps(&scan(&roots).find(id).unwrap().fix.as_ref().unwrap().steps) {
                done.unwrap();
            }
            assert!(scan(&roots).find(id).is_none(), "{id} is offered still");
        }
        let fine: Vec<String> = scan(&roots).fine.iter().map(|f| f.title.clone()).collect();
        for title in ["The kernel switches to a woken program at once", "A busy program's turn on a processor is short", "Removable disks' writes are limited"] {
            assert!(fine.iter().any(|t| t == title), "{title}: {fine:?}");
        }
        assert_eq!(std::fs::read_to_string(fs.join("etc/kernel/cmdline")).unwrap(), "root=UUID=1 ro rhgb quiet preempt=full\n");
        assert_eq!(std::fs::read_to_string(fs.join("etc/udev/rules.d/61-olo-slow-disks.rules")).unwrap(), tuning::SLOW_DISK_RULE);
        // A kernel an update brought, started from an entry without the
        // argument though it's kept: said, and the fix puts it back.
        std::fs::remove_file(fs.join("run/olo-kernel/preemption")).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        w("proc/stat", &format!("btime {}\n", now + 3600));
        let f = scan(&roots).find("kernel:preemption").unwrap().clone();
        assert_eq!((f.severity, f.title.as_str()), (Severity::Advice, "The kernel was started without full preemption"));
        assert!(matches!(f.fix.unwrap().steps.as_slice(), [Step::AddKernelArgument { .. }, Step::WriteSysfs { .. }, Step::WriteFile { .. }]));
        w("proc/stat", "btime 1\n");
        w("run/olo-kernel/preemption", "full\n");
        // With LAVD scheduling the setting changes nothing: not offered.
        std::fs::remove_file(fs.join("run/olo-kernel/preemption")).unwrap();
        w("etc/kernel/cmdline", "root=UUID=1 ro rhgb quiet\n");
        assert!(scan(&roots).find("kernel:preemption").is_some());
        w("sys/kernel/sched_ext/state", "enabled\n");
        w("sys/kernel/sched_ext/root/ops", "lavd_1.1.4_x86_64_unknown_linux_gnu\n");
        assert!(scan(&roots).find("kernel:preemption").is_none());
        w("sys/kernel/sched_ext/state", "disabled\n");
        w("run/olo-kernel/preemption", "full\n");
        // Someone's own choice of preemption is theirs: not offered.
        std::fs::remove_file(fs.join("run/olo-kernel/preemption")).unwrap();
        w("etc/kernel/cmdline", "root=UUID=1 ro preempt=none\n");
        assert!(scan(&roots).find("kernel:preemption").is_none());
        // Locked down with the argument kept: fine, from the next restart.
        w("etc/kernel/cmdline", "root=UUID=1 ro preempt=full\n");
        w("sys/kernel/security/lockdown", "none [integrity] confidentiality\n");
        let r = scan(&roots);
        assert!(r.find("kernel:preemption").is_none());
        assert!(r.fine.iter().any(|f| f.title == "The kernel switches to a woken program at once from the next restart"));
        // With no removable disk here the fix is the rule alone, and says so.
        std::fs::rename(fs.join("sys/block/sdb"), fs.join("sys/block-sdb-away")).unwrap();
        std::fs::remove_file(fs.join("etc/udev/rules.d/61-olo-slow-disks.rules")).unwrap();
        let f = scan(&roots).find("kernel:slow-disks").unwrap().clone();
        assert!(f.detail.ends_with("The whole took as long either way. The fix installs a rule that limits each as it's plugged in."), "{}", f.detail);
        assert_eq!(f.fix.unwrap().steps.len(), 1);
        std::fs::rename(fs.join("sys/block-sdb-away"), fs.join("sys/block/sdb")).unwrap();
        w("etc/udev/rules.d/61-olo-slow-disks.rules", tuning::SLOW_DISK_RULE);
        // A stick plugged in before the rule's time (not limited): offered again, for it.
        w("sys/block/sdb/bdi/strict_limit", "0\n");
        assert_eq!(scan(&roots).find("kernel:slow-disks").unwrap().fix.as_ref().unwrap().steps.len(), 2);
    }

    #[test]
    fn tcp_is_offered_bbr3() {
        let (fs, roots) = machine("bbr3");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        // (No TCP to ask about, or no service program beside ours: nothing.)
        assert!(scan(&roots).find("tcp-bbr3").is_none());
        w("proc/sys/net/ipv4/tcp_congestion_control", "cubic\n");
        w("proc/sys/kernel/osrelease", "7.2.15-300.fc45.x86_64\n");
        assert!(scan(&roots).find("tcp-bbr3").is_none());
        std::fs::write(roots.bin.join("olo-bbr3"), "the service").unwrap();
        let said = |roots: &Roots| scan(roots).find("tcp-bbr3").map(|f| f.fix.as_ref().unwrap().steps.iter().map(Step::describe).collect::<Vec<_>>());
        let install = [
            "Installing /usr/local/libexec/olo/olo-bbr3",
            "Labelling /usr/local/libexec/olo/olo-bbr3 for SELinux",
            "Writing /etc/systemd/system/olo-bbr3.service",
            "Reloading systemd for olo-bbr3.service",
            "Enabling olo-bbr3.service",
            "Restarting olo-bbr3.service",
        ];
        // Nothing to build with: installed first.
        let f = scan(&roots).find("tcp-bbr3").unwrap().clone();
        assert_eq!((f.severity, f.fix.as_ref().unwrap().label.as_str(), f.fix.as_ref().unwrap().root), (Severity::Suggestion, "Use BBRv3", true));
        assert!(f.detail.starts_with("New TCP connections here use CUBIC, Linux's default") && f.detail.ends_with("the compiler, make) is installed first."), "{}", f.detail);
        assert_eq!(said(&roots).unwrap(), [&["Installing kernel-devel-matched, gcc, make"][..], &install[..]].concat());
        // The kernel's headers, make and the compiler there: the service alone.
        w("lib/modules/7.2.15-300.fc45.x86_64/build/Makefile", "");
        w("usr/bin/make", "");
        w("usr/bin/gcc", "");
        assert_eq!(said(&roots).unwrap(), install);
        assert!(scan(&roots).find("tcp-bbr3").unwrap().detail.ends_with("has new connections use it."));
        // The kernel's BBRv1 in use: said for what it is.
        w("proc/sys/net/ipv4/tcp_congestion_control", "bbr\n");
        let roots = Roots { kernels_bbr: Some(1), ..roots };
        assert!(scan(&roots).find("tcp-bbr3").unwrap().detail.starts_with("New TCP connections here use the kernel's bbr, which is BBRv1"));
        // Its service tried and couldn't: why is said, and the fix stays.
        w("run/olo-bbr3/status", "failed: the kernel's build failed: no rule\n");
        let f = scan(&roots).find("tcp-bbr3").unwrap().clone();
        assert!(f.detail.ends_with("At its last try it couldn't: the kernel's build failed: no rule.") && f.fix.is_some(), "{}", f.detail);
        // In use, its service this build's, enabled and run: fine.
        w("run/olo-bbr3/status", "bbr3\n");
        w("proc/sys/net/ipv4/tcp_congestion_control", "bbr3\n");
        std::fs::create_dir_all(fs.join("sys/module/tcp_bbr3")).unwrap();
        w("usr/local/libexec/olo/olo-bbr3", "the service");
        w("etc/systemd/system/olo-bbr3.service", BBR3_UNIT);
        let kept = |name: &str| -> Result<UnitState, String> { if name == BBR3_SERVICE { Ok(UnitState { loaded: true, active: true, enabled: true, path: None }) } else { Err("none".into()) } };
        let roots = Roots { units: Box::new(kept), ..roots };
        let r = scan(&roots);
        assert!(r.find("tcp-bbr3").is_none() && r.find("tcp-bbr3:service").is_none(), "{:?}", r.find("tcp-bbr3").or(r.find("tcp-bbr3:service")).map(|f| f.detail.clone()));
        assert!(r.fine.iter().any(|f| f.title == "TCP sends with BBRv3" && f.detail.ends_with("built for this kernel and loaded at every start.")));
        // A kernel an update brought, of a series the module hasn't been
        // checked on: built and in use all the same, and said.
        w("proc/sys/kernel/osrelease", "7.9.1-200.fc46.x86_64\n");
        w("lib/modules/7.9.1-200.fc46.x86_64/build/Makefile", "");
        assert!(
            scan(&roots).fine.iter().any(|f| f.title == "TCP sends with BBRv3" && f.detail.ends_with("hasn't yet passed its self-check on this kernel's series (7.9): an Olo update brings that."))
        );
        w("proc/sys/kernel/osrelease", "7.2.15-300.fc45.x86_64\n");
        // An older build's program there: brought up to date.
        w("usr/local/libexec/olo/olo-bbr3", "an older service");
        let f = scan(&roots).find("tcp-bbr3:service").unwrap().clone();
        assert_eq!((f.severity, f.fix.as_ref().unwrap().label.as_str()), (Severity::Advice, "Keep It at Every Start"));
        assert_eq!(
            f.fix.as_ref().unwrap().steps.iter().map(Step::describe).collect::<Vec<_>>(),
            [
                "Installing /usr/local/libexec/olo/olo-bbr3",
                "Labelling /usr/local/libexec/olo/olo-bbr3 for SELinux",
                "Reloading systemd for olo-bbr3.service",
                "Enabling olo-bbr3.service",
                "Restarting olo-bbr3.service"
            ]
        );
        w("usr/local/libexec/olo/olo-bbr3", "the service");
        // Named but its module not in the kernel (the name left in a
        // namespace's settings, say): not in use.
        std::fs::remove_dir_all(fs.join("sys/module/tcp_bbr3")).unwrap();
        assert_eq!(said(&roots).unwrap(), ["Restarting olo-bbr3.service"]);
        // A kernel whose own bbr is BBRv3: that one, nothing built.
        w("proc/sys/net/ipv4/tcp_congestion_control", "cubic\n");
        std::fs::remove_file(fs.join("usr/bin/gcc")).unwrap();
        let roots = Roots { kernels_bbr: Some(3), ..roots };
        let f = scan(&roots).find("tcp-bbr3").unwrap().clone();
        assert!(f.detail.ends_with("This kernel has it as its own bbr; the fix has new connections use it, now and at every start."), "{}", f.detail);
        assert_eq!(said(&roots).unwrap(), ["Restarting olo-bbr3.service"]);
        w("proc/sys/net/ipv4/tcp_congestion_control", "bbr\n");
        let r = scan(&roots);
        assert!(r.find("tcp-bbr3").is_none() && r.fine.iter().any(|f| f.detail == "New connections use this kernel's own bbr, which is BBRv3."));
        // Secure Boot, and no BBRv3 of the kernel's own: nothing to offer.
        w("proc/sys/net/ipv4/tcp_congestion_control", "cubic\n");
        let roots = Roots { kernels_bbr: None, ..roots };
        assert!(scan(&roots).find("tcp-bbr3").is_some());
        std::fs::create_dir_all(fs.join(SECURE_BOOT_VAR).parent().unwrap()).unwrap();
        std::fs::write(fs.join(SECURE_BOOT_VAR), [6u8, 0, 0, 0, 1]).unwrap();
        assert!(scan(&roots).find("tcp-bbr3").is_none());
    }

    /// Intel's xe driver with swap on, where Olo's kernel patch can go in:
    /// its service installed (what building the patch needs first, where
    /// it's missing), swap left on; fine once it's in and kept; kept at
    /// each start where it was only loaded by hand; and swap turned off
    /// instead where the kernel can't take it or its service couldn't.
    #[test]
    fn xe_swap_and_olos_patch() {
        let (fs, roots) = machine("graphics-patch");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w("sys/bus/pci/devices/0000:00:02.0/class", "0x030000\n");
        w("sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
        std::fs::create_dir_all(fs.join("sys/bus/pci/drivers/xe")).unwrap();
        std::os::unix::fs::symlink(fs.join("sys/bus/pci/drivers/xe"), fs.join("sys/bus/pci/devices/0000:00:02.0/driver")).unwrap();
        w("proc/swaps", "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n/dev/zram0 partition 8388604 0 100\n");
        w("proc/sys/kernel/osrelease", "7.2.15-300.fc45.x86_64\n");
        std::fs::create_dir_all(fs.join("sys/kernel/livepatch")).unwrap();
        std::fs::write(roots.bin.join("olo-gpu-memory"), "the service").unwrap();
        let said = |roots: &Roots| scan(roots).find("graphics-swap").map(|f| f.fix.as_ref().unwrap().steps.iter().map(Step::describe).collect::<Vec<_>>());
        let install = [
            "Installing /usr/local/libexec/olo/olo-gpu-memory",
            "Labelling /usr/local/libexec/olo/olo-gpu-memory for SELinux",
            "Writing /etc/systemd/system/olo-gpu-memory.service",
            "Reloading systemd for olo-gpu-memory.service",
            "Enabling olo-gpu-memory.service",
            "Restarting olo-gpu-memory.service",
        ];
        // Nothing to build with: installed first.
        let f = scan(&roots).find("graphics-swap").unwrap().clone();
        assert_eq!((f.severity, f.fix.as_ref().unwrap().label.as_str()), (Severity::Problem, "Keep Graphics Memory in Memory"));
        assert!(f.detail.contains("The fix puts Olo's kernel patch in (olo_gpu_memory") && f.detail.ends_with("the compiler, make) is installed first."), "{}", f.detail);
        assert_eq!(said(&roots).unwrap(), [&["Installing kernel-devel-matched, gcc, make"][..], &install[..]].concat());
        // The kernel's headers, make and the compiler there: the service alone.
        w("lib/modules/7.2.15-300.fc45.x86_64/build/Makefile", "");
        w("usr/bin/make", "");
        w("usr/bin/gcc", "");
        assert_eq!(said(&roots).unwrap(), install);
        assert!(scan(&roots).find("graphics-swap").unwrap().detail.ends_with("loads it at every start."));
        // In, its service this build's, enabled and run: fine, swap on.
        w("sys/kernel/livepatch/olo_gpu_memory/enabled", "1\n");
        w("usr/local/libexec/olo/olo-gpu-memory", "the service");
        w("etc/systemd/system/olo-gpu-memory.service", GPU_MEMORY_UNIT);
        let kept = |name: &str| -> Result<UnitState, String> { if name == GPU_MEMORY_SERVICE { Ok(UnitState { loaded: true, active: true, enabled: true, path: None }) } else { Err("none".into()) } };
        let roots = Roots { units: Box::new(kept), ..roots };
        let r = scan(&roots);
        assert!(
            r.find("graphics-swap").is_none() && r.find("graphics-swap:service").is_none(),
            "{:?}",
            r.find("graphics-swap").or(r.find("graphics-swap:service")).map(|f| f.fix.as_ref().unwrap().steps.iter().map(Step::describe).collect::<Vec<_>>())
        );
        assert!(r.fine.iter().any(|f| f.title == "Windows' graphics memory stays in memory" && f.detail.starts_with("Olo's kernel patch (olo_gpu_memory) is in")));
        // In by hand, from an Olo with no service program beside it: said,
        // nothing to offer.
        std::fs::remove_file(roots.bin.join("olo-gpu-memory")).unwrap();
        std::fs::remove_file(fs.join("etc/systemd/system/olo-gpu-memory.service")).unwrap();
        let f = scan(&roots).find("graphics-swap:service").unwrap().clone();
        assert!(f.fix.is_none() && f.detail.ends_with("no olo-gpu-memory beside it to install it from."), "{}", f.detail);
        std::fs::write(roots.bin.join("olo-gpu-memory"), "the service").unwrap();
        w("etc/systemd/system/olo-gpu-memory.service", GPU_MEMORY_UNIT);
        // An older build's program there: brought up to date (taken out
        // and built again as its service starts again).
        w("usr/local/libexec/olo/olo-gpu-memory", "an older service");
        let f = scan(&roots).find("graphics-swap:service").unwrap().clone();
        assert_eq!((f.severity, f.fix.as_ref().unwrap().label.as_str()), (Severity::Advice, "Keep It at Every Start"));
        assert_eq!(f.fix.unwrap().steps.iter().map(Step::describe).collect::<Vec<_>>(), [install[0], install[1], install[3], install[4], install[5]]);
        w("usr/local/libexec/olo/olo-gpu-memory", "the service");
        // Secure Boot: the kernel takes no unsigned module, so swap off.
        std::fs::remove_dir_all(fs.join("sys/kernel/livepatch/olo_gpu_memory")).unwrap();
        w("sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c", "\u{6}\0\0\0\u{1}");
        let f = scan(&roots).find("graphics-swap").unwrap().clone();
        assert_eq!(f.fix.as_ref().unwrap().label, "Turn Swap Off");
        assert!(f.detail.contains("with Secure Boot this kernel loads only signed modules"), "{}", f.detail);
        assert_eq!(f.fix.unwrap().steps.last(), Some(&Step::SwapOff));
        std::fs::remove_file(fs.join("sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c")).unwrap();
        // Its service tried and couldn't: why, and swap off instead.
        w("run/olo-gpu-memory/status", "failed: the kernel's build failed: no rule to make target\n");
        let f = scan(&roots).find("graphics-swap").unwrap().clone();
        assert_eq!(f.fix.as_ref().unwrap().label, "Turn Swap Off");
        assert!(f.detail.contains("couldn't be put in: the kernel's build failed: no rule to make target. The fix turns swap off instead"), "{}", f.detail);
        // Only this kernel's headers missing: they're installed, and it's tried again.
        w("run/olo-gpu-memory/status", "failed: this kernel's headers aren't installed (the kernel-devel package for 7.2.15-300.fc45.x86_64)\n");
        std::fs::remove_file(fs.join("lib/modules/7.2.15-300.fc45.x86_64/build/Makefile")).unwrap();
        assert_eq!(said(&roots).unwrap(), ["Installing kernel-devel-matched, gcc, make", "Restarting olo-gpu-memory.service"]);
    }

    /// Intel graphics on Fedora with its own driver: RPM Fusion's full
    /// one, its repositories added first where they aren't; nothing to
    /// do once it's there, or on another system.
    #[test]
    fn intel_graphics_decode_every_video() {
        let (fs, roots) = machine("intel-video");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w("sys/bus/pci/devices/0000:00:02.0/class", "0x030000\n");
        w("sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
        w("etc/os-release", "NAME=\"Fedora Linux\"\nID=fedora\nVERSION_ID=45\n");
        // No PackageKit: what to do, said.
        let r = scan(&roots);
        let f = r.find("video:intel").expect("the driver to install");
        assert!(f.fix.is_none() && f.detail.contains("rpmfusion.org"), "{}", f.detail);
        // RPM Fusion not added: added, then the driver.
        let roots = Roots { packages: Box::new(|_| Some(Vec::new())), ..roots };
        let fix = scan(&roots).find("video:intel").unwrap().fix.clone().expect("a fix");
        assert!(fix.root);
        assert_eq!(
            fix.steps,
            vec![
                Step::InstallRepository {
                    name: "RPM Fusion (free)".into(),
                    urls: vec![
                        "https://download1.rpmfusion.org/free/fedora/rpmfusion-free-release-45.noarch.rpm".into(),
                        "https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-45.noarch.rpm".into()
                    ]
                },
                Step::InstallRepository {
                    name: "RPM Fusion (nonfree)".into(),
                    urls: vec![
                        "https://download1.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-45.noarch.rpm".into(),
                        "https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-45.noarch.rpm".into()
                    ]
                },
                Step::InstallPackages { names: vec!["intel-media-driver".into()] },
            ]
        );
        // Half done (the free repository added, the nonfree one not, as a
        // mirror's 404 left it, 2026-11-06): the rest.
        let roots = Roots { packages: Box::new(|_| Some(vec![("rpmfusion-free-release".into(), true)])), ..roots };
        let fix = scan(&roots).find("video:intel").unwrap().fix.clone().unwrap();
        assert_eq!(fix.steps.iter().map(Step::describe).collect::<Vec<_>>(), ["Adding the RPM Fusion (nonfree) repository", "Installing intel-media-driver"]);
        // RPM Fusion there: only the driver.
        let roots = Roots { packages: Box::new(|_| Some(vec![("intel-media-driver".into(), false)])), ..roots };
        let fix = scan(&roots).find("video:intel").unwrap().fix.clone().unwrap();
        assert_eq!(fix.steps, vec![Step::InstallPackages { names: vec!["intel-media-driver".into()] }]);
        // Installed: fine.
        w("usr/lib64/dri-nonfree/iHD_drv_video.so", "");
        let r = scan(&roots);
        assert!(r.find("video:intel").is_none());
        assert!(r.fine.iter().any(|f| f.title == "Intel graphics decodes every video"));
        // An atomic Fedora: by hand.
        std::fs::remove_file(fs.join("usr/lib64/dri-nonfree/iHD_drv_video.so")).unwrap();
        w("run/ostree-booted", "");
        assert!(scan(&roots).find("video:intel").unwrap().fix.is_none());
        // Not Fedora: nothing.
        std::fs::remove_file(fs.join("run/ostree-booted")).unwrap();
        w("etc/os-release", "ID=arch\n");
        assert!(scan(&roots).find("video:intel").is_none());
    }

    #[test]
    fn thinkpad_graphics_and_sound() {
        let (fs, roots) = machine("thinkpad");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let pci = "sys/bus/pci/devices";
        w(&format!("{pci}/0000:00:02.0/class"), "0x030000\n");
        w(&format!("{pci}/0000:00:02.0/vendor"), "0x8086\n");
        w(&format!("{pci}/0000:00:02.0/boot_vga"), "1\n");
        w(&format!("{pci}/0000:01:00.0/class"), "0x030000\n");
        w(&format!("{pci}/0000:01:00.0/vendor"), "0x10de\n");
        w(&format!("{pci}/0000:01:00.0/boot_vga"), "0\n");
        w(&format!("{pci}/0000:01:00.0/power/control"), "on\n");
        std::fs::create_dir_all(fs.join("sys/module/nouveau")).unwrap();
        w(&format!("{pci}/0000:00:1f.3/class"), "0x040100\n");
        w(&format!("{pci}/0000:00:1f.3/vendor"), "0x8086\n");
        std::fs::create_dir_all(fs.join("sys/bus/pci/drivers/sof-audio-pci-intel-ptl")).unwrap();
        std::os::unix::fs::symlink(fs.join("sys/bus/pci/drivers/sof-audio-pci-intel-ptl"), fs.join(format!("{pci}/0000:00:1f.3/driver"))).unwrap();
        for d in ["sdw:0:2:01fa:4243:01", "sdw:0:2:01fa:3557:01:0", "sdw:0:2:01fa:3557:01:4"] {
            std::fs::create_dir_all(fs.join("sys/bus/soundwire/devices").join(d)).unwrap();
        }
        let r = scan(&roots);
        let idle = r.find("gpu:idle").expect("the card kept awake");
        assert_eq!(idle.title, "The NVIDIA graphics card never sleeps");
        assert!(idle.detail.starts_with("No driver has it"), "{}", idle.detail);
        assert!(idle.detail.contains("Olo draws with the Intel graphics"), "{}", idle.detail);
        let fix = idle.fix.as_ref().unwrap();
        assert!(fix.root);
        assert_eq!(
            fix.steps,
            vec![
                Step::WriteFile { path: "/etc/udev/rules.d/72-olo-idle-gpu.rules".into(), content: IDLE_GPU_RULE.into(), mode: 0o644 },
                Step::WriteSysfs { path: "/sys/bus/pci/devices/0000:01:00.0/power/control".into(), value: "auto".into() },
            ]
        );
        // No PackageKit: what to do, said.
        let driver = r.find("gpu:driver:0000:01:00.0").expect("no driver");
        assert!(driver.detail.contains("nouveau") && driver.fix.is_none());
        let sound = r.find("sound:builtin").expect("no sound card");
        assert!(sound.detail.contains("Cirrus Logic CS35L57, Cirrus Logic CS42L43; 2 amplifiers"), "{}", sound.detail);
        assert!(sound.fix.is_none());
        // With PackageKit: NVIDIA's driver to install, a kernel and
        // firmware to update.
        let roots = Roots {
            packages: Box::new(|names| Some(names.iter().filter(|n| **n == "akmod-nvidia").map(|n| (n.to_string(), false)).collect())),
            updates: Box::new(|| Some(vec!["kernel".into(), "firefox".into(), "cirrus-audio-firmware".into()])),
            ..roots
        };
        let r = scan(&roots);
        let fix = r.find("gpu:driver:0000:01:00.0").unwrap().fix.clone().expect("an install");
        assert_eq!((fix.label.as_str(), fix.steps), ("Install NVIDIA's Driver", vec![Step::InstallPackages { names: vec!["akmod-nvidia".into()] }]));
        let sound = r.find("sound:builtin").unwrap();
        assert!(sound.detail.contains("installs kernel, cirrus-audio-firmware; restart afterwards"), "{}", sound.detail);
        assert_eq!(sound.fix.as_ref().unwrap().steps, vec![Step::UpdatePackages { names: vec!["kernel".into(), "cirrus-audio-firmware".into()] }]);
        // Fixed: the rule there and the card allowed to sleep.
        w("etc/udev/rules.d/72-olo-idle-gpu.rules", IDLE_GPU_RULE);
        w(&format!("{pci}/0000:01:00.0/power/control"), "auto\n");
        // And a sound card made of the chips.
        std::fs::create_dir_all(fs.join("sys/class/sound/card1")).unwrap();
        std::os::unix::fs::symlink(fs.join(format!("{pci}/0000:00:1f.3")), fs.join("sys/class/sound/card1/device")).unwrap();
        let r = scan(&roots);
        assert!(r.find("gpu:idle").is_none() && r.find("sound:builtin").is_none());
        assert!(r.fine.iter().any(|f| f.title == "The NVIDIA graphics card sleeps when unused"));
        let _ = std::fs::remove_dir_all(&fs);
    }

    /// The session's TMPDIR: systemd's when it answers; else Olo's file,
    /// never Doctor's own (the watcher's, from before the fix) over it.
    #[test]
    fn the_sessions_tmpdir() {
        let set = || Some(vec![("TMPDIR".to_owned(), "/var/tmp".to_owned())]);
        assert_eq!(session_tmpdir(set(), None, None).as_deref(), Some("/var/tmp"));
        // (The manager says none: none, whatever this program has.)
        assert_eq!(session_tmpdir(Some(vec![]), Some("/var/tmp".into()), Some("/var/tmp".into())), None);
        // Not answering: the file's, then this program's own.
        assert_eq!(session_tmpdir(None, Some("/var/tmp".into()), None).as_deref(), Some("/var/tmp"));
        assert_eq!(session_tmpdir(None, None, Some("/srv/tmp".into())).as_deref(), Some("/srv/tmp"));
        assert_eq!(session_tmpdir(None, None, None), None);
        // The file as Doctor's fix writes it.
        let dir = PathBuf::from(format!("/var/tmp/olo-tmpdir-file-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("environment.d"));
        std::fs::write(dir.join(olo_system::env::TMPDIR_FILE), "# Olo Doctor: …\nTMPDIR=/var/tmp\nTMP=/var/tmp\nTEMP=/var/tmp\n").unwrap();
        assert_eq!(olo_system::env::tmpdir_in_file(&dir).as_deref(), Some("/var/tmp"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stuck_card_is_said_and_never_started_again_in_place() {
        let (fs, roots) = machine("stuck-gpu");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let card = "sys/bus/pci/devices/0000:01:00.0";
        w(&format!("{card}/class"), "0x030000\n");
        w(&format!("{card}/vendor"), "0x10de\n");
        w(&format!("{card}/power/runtime_status"), "error\n");
        std::fs::create_dir_all(fs.join("sys/bus/pci/drivers/nvidia")).unwrap();
        std::os::unix::fs::symlink(fs.join("sys/bus/pci/drivers/nvidia"), fs.join(format!("{card}/driver"))).unwrap();
        for m in ["nvidia", "nvidia_drm", "nvidia_modeset", "nvidia_uvm"] {
            w(&format!("sys/module/{m}/initstate"), "live\n");
        }
        // A program with its node open, as there was when the fix that
        // is gone would quit it.
        w("proc/952/cmdline", "/home/me/.local/bin/olo-activity\0");
        std::fs::create_dir_all(fs.join("proc/952/fd")).unwrap();
        std::os::unix::fs::symlink("/dev/nvidia0", fs.join("proc/952/fd/9")).unwrap();
        // Its power management failed: said, a restart its only fix,
        // and why Olo offers none.
        let id = "gpu:stuck:0000:01:00.0";
        let f = scan(&roots).find(id).cloned().expect("the stuck card");
        assert_eq!((f.title.as_str(), f.severity), ("The NVIDIA graphics card's power management failed", Severity::Problem));
        assert!(f.fix.is_none(), "no fix that unloads a driver whose card doesn't answer: {:?}", f.fix);
        assert!(f.detail.contains("Restarting the computer is what starts it again") && f.detail.contains("can hang in the kernel"), "{}", f.detail);
        // Its driver's unload begun and never finished (the fix that is
        // gone, or anyone's `modprobe -r`): said as that, whatever the
        // card's own state reads.
        w("sys/module/nvidia_drm/initstate", "going\n");
        let f = scan(&roots).find(id).cloned().unwrap();
        assert_eq!(f.title, STUCK_TITLE);
        assert!(f.fix.is_none() && f.detail.contains("may fail or hang") && f.detail.contains("Save your work, then restart.") && f.detail.ends_with("is called nvidia_drm.)"), "{}", f.detail);
        w(&format!("{card}/power/runtime_status"), "active\n");
        assert_eq!(scan(&roots).find(id).map(|f| f.title.clone()).as_deref(), Some(STUCK_TITLE));
        // Answering, and nothing unloading: nothing to say.
        w("sys/module/nvidia_drm/initstate", "live\n");
        assert!(scan(&roots).find(id).is_none());
        // Another maker's card: said, without NVIDIA's reason.
        w(&format!("{card}/vendor"), "0x1002\n");
        w(&format!("{card}/power/runtime_status"), "error\n");
        let f = scan(&roots).find(id).cloned().unwrap();
        assert!(f.fix.is_none() && f.detail.ends_with("save your work first."), "{}", f.detail);
    }

    #[test]
    fn nvidias_console_on_a_card_with_no_display_is_given_up() {
        let (fs, roots) = machine("nvidia-console");
        let w = |p: &str, c: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let pci = "sys/bus/pci/devices";
        let card = |address: &str, vendor: &str, boot: &str, driver: &str, drm: &str| {
            w(&format!("{pci}/{address}/class"), "0x030000\n");
            w(&format!("{pci}/{address}/vendor"), &format!("{vendor}\n"));
            w(&format!("{pci}/{address}/boot_vga"), &format!("{boot}\n"));
            std::fs::create_dir_all(fs.join("sys/bus/pci/drivers").join(driver)).unwrap();
            let link = fs.join(format!("{pci}/{address}/driver"));
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(fs.join("sys/bus/pci/drivers").join(driver), link).unwrap();
            std::fs::create_dir_all(fs.join(format!("{pci}/{address}/drm/{drm}"))).unwrap();
        };
        card("0000:00:02.0", "0x8086", "1", "xe", "card0");
        card("0000:01:00.0", "0x10de", "0", "nvidia", "card1");
        let id = "gpu:console:0000:01:00.0";
        // The module isn't loaded: nothing to say.
        assert!(scan(&roots).find(id).is_none());
        // Loaded, running, with modesetting, and no console: nothing.
        w("sys/module/nvidia_drm/initstate", "live\n");
        w("sys/module/nvidia_drm/parameters/modeset", "Y\n");
        w("sys/module/nvidia_drm/parameters/fbdev", "N\n");
        assert!(scan(&roots).find(id).is_none());
        // It takes one, on a card with no connector, beside the card
        // the computer started on: offered.
        w("sys/module/nvidia_drm/parameters/fbdev", "Y\n");
        let f = scan(&roots).find(id).cloned().expect("the console");
        assert_eq!((f.severity, f.title.as_str()), (Severity::Advice, CONSOLE_KEPT.0));
        assert!(!f.detail.contains("as before") && !f.detail.contains("again"), "no more is claimed than is so: {}", f.detail);
        let fix = f.fix.expect("a fix");
        assert_eq!((fix.label.as_str(), fix.root), ("Turn Off the Console", true));
        assert_eq!(fix.steps, vec![Step::WriteFile { path: "/etc/modprobe.d/olo-nvidia-console.conf".into(), content: NVIDIA_CONSOLE.into(), mode: 0o644 }]);
        assert!(NVIDIA_CONSOLE.lines().any(|l| l == "options nvidia_drm fbdev=0"));
        // Not on what isn't known for certain. The module on its way
        // out (its cards' DRM devices go first: as on 2026-11-20):
        w("sys/module/nvidia_drm/initstate", "going\n");
        std::fs::remove_dir_all(fs.join(format!("{pci}/0000:01:00.0/drm"))).unwrap();
        let r = scan(&roots);
        assert!(r.find(id).is_none(), "offered from a card whose DRM device is gone");
        assert!(r.find("gpu:stuck:0000:01:00.0").is_some_and(|f| f.detail.contains("then restart.")));
        // Running, but its DRM device can't be read:
        w("sys/module/nvidia_drm/initstate", "live\n");
        assert!(scan(&roots).find(id).is_none());
        std::fs::create_dir_all(fs.join(format!("{pci}/0000:01:00.0/drm/card1"))).unwrap();
        assert!(scan(&roots).find(id).is_some());
        // Without modesetting:
        w("sys/module/nvidia_drm/parameters/modeset", "N\n");
        assert!(scan(&roots).find(id).is_none());
        w("sys/module/nvidia_drm/parameters/modeset", "Y\n");
        // A card with a port: its console is the text before the
        // desktop.
        std::fs::create_dir_all(fs.join(format!("{pci}/0000:01:00.0/drm/card1/card1-DP-3"))).unwrap();
        assert!(scan(&roots).find(id).is_none());
        std::fs::remove_dir_all(fs.join(format!("{pci}/0000:01:00.0/drm/card1/card1-DP-3"))).unwrap();
        // A second card of NVIDIA's that has one: the option is for
        // both, so neither.
        card("0000:02:00.0", "0x10de", "0", "nvidia", "card2");
        std::fs::create_dir_all(fs.join(format!("{pci}/0000:02:00.0/drm/card2/card2-HDMI-A-1"))).unwrap();
        assert!(scan(&roots).find(id).is_none());
        std::fs::remove_dir_all(fs.join(format!("{pci}/0000:02:00.0"))).unwrap();
        // The computer started on NVIDIA's own card: its console is the
        // person's.
        w(&format!("{pci}/0000:00:02.0/boot_vga"), "0\n");
        assert!(scan(&roots).find(id).is_none());
        w(&format!("{pci}/0000:00:02.0/boot_vga"), "1\n");
        assert!(scan(&roots).find(id).is_some());
        // The option kept since this start: fine, from the next (the
        // running module still says Y).
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        w("proc/stat", &format!("cpu  1 2 3\nbtime {}\n", now - 3600));
        w("etc/modprobe.d/olo-nvidia-console.conf", NVIDIA_CONSOLE);
        let r = scan(&roots);
        assert!(r.find(id).is_none() && r.find("gpu:console:kept").is_none());
        assert!(r.fine.iter().any(|f| f.title == "NVIDIA's driver takes no console from the next start"));
        // Kept from before this start, and still not in force: said.
        w("proc/stat", &format!("cpu  1 2 3\nbtime {}\n", now + 3600));
        let r = scan(&roots);
        assert!(r.find("gpu:console:kept").is_some_and(|f| f.fix.is_none() && f.severity == Severity::Advice));
        // In force.
        w("sys/module/nvidia_drm/parameters/fbdev", "N\n");
        let r = scan(&roots);
        assert!(r.find("gpu:console:kept").is_none() && r.fine.iter().any(|f| f.title == "NVIDIA's driver takes no console"));
        let _ = std::fs::remove_dir_all(&fs);
    }

    #[test]
    fn another_logins_desktop_is_left_alone() {
        let (fs, roots) = machine("elsewhere");
        std::fs::create_dir_all(fs.join("proc/950")).unwrap();
        std::fs::write(fs.join("proc/950/cmdline"), "/usr/bin/gnome-shell\0").unwrap();
        std::fs::write(fs.join("proc/950/cgroup"), "0::/user.slice/user-1000.slice/session-5.scope\n").unwrap();
        assert!(scan(&roots).find("foreign").is_none());
        let _ = std::fs::remove_dir_all(&fs);
    }

    /// Two builds of the same healer, their bytes different (what was
    /// built beside them): the installed one is current. Another source,
    /// or one installed from before builds said: out of date.
    #[test]
    fn the_healer_is_current_by_its_source() {
        let dir = PathBuf::from(format!("/var/tmp/olo-doctor-test-{}-heal", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        };
        let built = file("built", b"\x7fELF one build olo-wifi-heal source 0123456789abcdef; more");
        let same = file("same", b"\x7fELF another build, longer olo-wifi-heal source 0123456789abcdef; and more");
        let other = file("other", b"\x7fELF olo-wifi-heal source fedcba9876543210;");
        let before = file("before", b"\x7fELF from before");
        assert_eq!(heal_source(&built).as_deref(), Some("0123456789abcdef"));
        assert!(same_healer(&built, &same));
        assert!(!same_healer(&built, &other));
        assert!(!same_healer(&built, &before));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporary_files_in_memory_are_moved_to_disk() {
        let run = |tmp_in_memory: bool, tmpdir: Option<&str>, tmpdir_in_memory: bool| {
            let mut r = Report::default();
            let roots = Roots {
                fs: PathBuf::from("/nonexistent"),
                bin: PathBuf::new(),
                uid: 1000,
                scope: None,
                units: Box::new(|_| Err("none".into())),
                airpods: false,
                phone: false,
                iphone: false,
                phone_connected: false,
                bluetooth_class: None,
                bluetooth_stuck: Vec::new(),
                packages: Box::new(|_| None),
                updates: Box::new(|| None),
                package_services: Box::new(Vec::new),
                temp: TempFiles {
                    tmp_in_memory,
                    tmpdir: tmpdir.map(str::to_owned),
                    tmpdir_in_memory,
                    session_env: None,
                    config: PathBuf::from("/home/me/.config"),
                    data: PathBuf::from("/home/me/.local/share"),
                    runtime: PathBuf::new(),
                    cache: PathBuf::new(),
                },
                pool_port: POOL_PORT,
                kernels_bbr: None,
            };
            temp_files(&roots, &mut r);
            r
        };
        // Fedora's /tmp in memory and no TMPDIR: the fix.
        let r = run(true, None, false);
        let f = r.find("tmp:memory").expect("found");
        let fix = f.fix.as_ref().unwrap();
        assert!(!fix.root);
        let Step::WriteFile { path, content, .. } = &fix.steps[0] else { panic!("{:?}", fix.steps) };
        assert_eq!(path, &PathBuf::from("/home/me/.config/environment.d/60-olo-tmpdir.conf"));
        assert!(content.contains("TMPDIR=/var/tmp\nTMP=/var/tmp\nTEMP=/var/tmp\n"), "{content}");
        assert_eq!(fix.steps[1], Step::SetSessionEnvironment { vars: ["TMPDIR", "TMP", "TEMP"].map(|k| (k.to_owned(), "/var/tmp".to_owned())).to_vec() });
        // TMPDIR in memory too (the runtime folder): the same.
        assert!(run(true, Some("/run/user/1000"), true).find("tmp:memory").is_some());
        // On disk: fine.
        let r = run(true, Some("/var/tmp"), false);
        assert!(r.find("tmp:memory").is_none() && r.fine.iter().any(|f| f.detail.contains("/var/tmp")));
        assert!(run(false, None, false).find("tmp:memory").is_none());
    }

    #[test]
    fn rule_ids_are_read_from_the_rule() {
        let ids = rule_ids(DISPLAY_RULE);
        assert!(ids.contains(&("05ac".into(), "1114".into())));
        assert_eq!(ids.len(), 5);
    }
}
