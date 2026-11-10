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
pub const INPUT_RULE: &str = include_str!("../../../data/udev/71-olo-input.rules");
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

const INPUT_RULE_PATH: &str = "etc/udev/rules.d/71-olo-input.rules";
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
/// the session, a program started before an upgrade) and isn't needed.
/// GNOME Keyring only while Olo's keyring answers. (After a `|`: what
/// follows its processes, as `by_program` says them.)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// Gets in Olo's way now.
    Problem,
    /// Better changed.
    Advice,
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

/// Something applied that can be undone (Olo Suggestions): the id its
/// button goes by, the line it's on (a `Fine`'s title), and how.
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
    /// counts, Fix All, the tray and the notifications; still fixable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<Finding>,
}

impl Report {
    pub fn problems(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Problem).count()
    }

    pub fn fixable(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.fix.is_some())
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
    /// Bluetooth controllers that don't answer (BlueZ's adapter powered
    /// and its power off, or the other way round), by name ("hci0").
    pub bluetooth_stuck: Vec<String>,
    /// The system's packages (PackageKit's word): which of these names
    /// there are, newest, and whether installed. None: no PackageKit.
    pub packages: Box<dyn Fn(&[&str]) -> Option<Vec<(String, bool)>> + Send + Sync>,
    /// The names of the packages with an update waiting (None: no
    /// PackageKit).
    pub updates: Box<dyn Fn() -> Option<Vec<String>> + Send + Sync>,
    /// Where the session's programs keep temporary files (`temp_files`).
    pub temp: TempFiles,
}

/// Where temporary files go: /tmp's file system, and the session's
/// `TMPDIR` (systemd's user manager's, else ours) and whether it's in
/// memory too; the user's config folder (for the fix's file) and data
/// folder (for D-Bus activation files: `file_manager`), and the
/// session's runtime folder (the activation files an Olo session writes).
#[derive(Debug, Clone, Default)]
pub struct TempFiles {
    pub tmp_in_memory: bool,
    pub tmpdir: Option<String>,
    pub tmpdir_in_memory: bool,
    pub config: PathBuf,
    pub data: PathBuf,
    pub runtime: PathBuf,
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
        let tmpdir = session_tmpdir(manager.ok(), olo_system::env::tmpdir_in_file(&config), std::env::var("TMPDIR").ok());
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".local/share"));
        let runtime = olo_xdg::runtime_dir();
        TempFiles {
            tmp_in_memory: olo_system::env::in_memory(Path::new("/tmp")),
            tmpdir_in_memory: tmpdir.as_deref().is_some_and(|d| olo_system::env::in_memory(Path::new(d))),
            tmpdir,
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
        let bluetooth_stuck =
            olo_system::bluetooth::status().ok().and_then(|s| s.adapter).filter(|a| a.not_answering()).map(|a| a.path.rsplit('/').next().unwrap_or_default().to_owned()).into_iter().collect();
        Roots {
            fs: PathBuf::from("/"),
            bin,
            uid,
            scope,
            units: Box::new(crate::fix::unit_state),
            airpods,
            bluetooth_stuck,
            packages: Box::new(real_packages),
            updates: Box::new(real_updates),
            temp: TempFiles::real(),
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

/// Look at everything.
/// Look this computer over, and say what was found for Deskbar's tray
/// (`olo_protocol::doctor::DoctorFound`).
pub fn look() -> Report {
    let mut report = scan(&Roots::real());
    report.set_aside(&crate::ignored::of(crate::app::Kind::Doctor));
    tell_tray(&report);
    report
}

/// What Deskbar's tray shows (its bag) from `report`: what's ignored is
/// left out.
pub fn tell_tray(report: &Report) {
    let issues: Vec<&Finding> = report.findings.iter().filter(|f| f.severity == Severity::Problem || f.fix.is_some()).collect();
    let found = olo_protocol::doctor::DoctorFound {
        issues: issues.iter().map(|f| f.title.clone()).collect(),
        problems: issues.iter().filter(|f| f.severity == Severity::Problem).count(),
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
    voice_realtime(roots, &procs, &mut r);
    airpods_listening(roots, &procs, &mut r);
    util_clamp(roots, &mut r);
    let gpus = pci_devices(&roots.fs);
    idle_gpu(roots, &gpus, &mut r);
    gpu_stuck(roots, &gpus, &procs, &mut r);
    gpu_driver(roots, &gpus, &mut r);
    intel_video(roots, &gpus, &mut r);
    builtin_sound(roots, &gpus, &mut r);
    thunderbolt(roots, &mut r);
    thunderbolt_usb2(&gpus, &mut r);
    login_keyring(roots, &mut r);
    secrets_store(roots, &procs, &mut r);
    presence_rule(roots, &mut r);
    power_now(roots, &mut r);
    security_keys(roots, &mut r);
    panel_hdr(roots, &mut r);
    stale(roots, &procs, &mut r);
    login_entry(roots, &mut r);
    login_screen_handed_over(roots, &mut r);
    own_login_screen(roots, &mut r);
    temp_files(roots, &mut r);
    file_manager(roots, &mut r);
    diskbackup(roots, &mut r);
    r
}

/// DiskBackup, as its settings and its service's state say: not set up
/// (said, no button: it's the person's to choose where backups go),
/// its service not running (Start It), paused (Resume Backups), nothing
/// finished for days (Back Up Now), its key only on this computer (Keep
/// Key in Passwords: without it, the backups can't be read elsewhere);
/// else when it last backed up, as fine.
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
            severity: Severity::Problem,
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
    let keyring_ours = secrets_status(roots).is_some_and(|v| v["serving"].as_bool() == Some(true));
    let found: Vec<(&Proc, &str)> = procs
        .iter()
        .filter(|p| ours(p) && (p.scope.is_none() || p.scope == roots.scope))
        .filter(|p| p.name != "gnome-keyring-daemon" || keyring_ours)
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
    if installed.as_deref() == Some(INPUT_RULE) {
        r.fine.push(Fine { title: "Olo's input rule is installed".into(), detail: "USB mice, keyboards and receivers are never put to sleep (71-olo-input.rules).".into() });
        return true;
    }
    let mut steps = vec![Step::WriteFile { path: PathBuf::from("/").join(INPUT_RULE_PATH), content: INPUT_RULE.into(), mode: 0o644 }];
    // Applied to what is plugged in now (udev reads the rule itself for
    // what comes later).
    for d in usb_inputs(&roots.fs) {
        if d.control != "on" {
            steps.push(Step::WriteSysfs { path: PathBuf::from("/sys/bus/usb/devices").join(&d.port).join("power/control"), value: "on".into() });
        }
    }
    let (title, verb) = if installed.is_some() { ("Olo's input rule is out of date", "updates") } else { ("Olo's input rule isn't installed", "installs") };
    r.findings.push(Finding {
        id: "rule:input".into(),
        severity: Severity::Problem,
        title: title.into(),
        detail: format!("Without it, a USB mouse, keyboard or wireless receiver may be put to sleep after a pause and lose the first reports when the hand moves again (the pointer sits still for a moment). The fix {verb} /etc/udev/rules.d/71-olo-input.rules and applies it to what is plugged in now."),
        fix: Some(Fix { label: "Install the Rule".into(), root: true, steps }),
    });
    false
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
        let mut steps = Vec::new();
        if !rule_current {
            steps.push(Step::WriteFile { path: PathBuf::from("/").join(INPUT_RULE_PATH), content: INPUT_RULE.into(), mode: 0o644 });
        }
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
            Step::WriteFile {
                path: "/etc/systemd/system/bluetooth.service.d/olo.conf".into(),
                content: format!("# Olo: BlueZ's advertisement monitor, for AirPods (Olo Doctor).\n[Service]\nExecStart=\nExecStart={program} --experimental\n"),
                mode: 0o644,
            },
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

/// The voice services' sound (olo-voice, olo-airpods): PipeWire's thread
/// for it ("data-loop") is made realtime through rtkit, which gives that
/// to so many processes a user (15 by default) and threads (25), and a
/// desktop's programs use them up: then a frame of someone's voice can
/// wait behind other work, a click in their call. The fix lets rtkit give
/// more, and restarts it (the supervisor asks again).
fn voice_realtime(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let threads = audio_threads(roots, procs);
    if threads.is_empty() {
        return;
    }
    let mut not: Vec<String> = Vec::new();
    for t in threads.iter().filter(|t| !t.realtime) {
        if !not.contains(&t.program) {
            not.push(t.program.clone());
        }
    }
    if not.is_empty() {
        r.fine.push(Fine { title: "Voice processing's sound is realtime".into(), detail: "rtkit made the voice services' sound threads realtime.".into() });
        return;
    }
    let daemon = ["usr/libexec/rtkit-daemon", "usr/lib/rtkit/rtkit-daemon"].into_iter().find(|p| roots.fs.join(p).exists());
    let fix = daemon.map(|d| Fix {
        label: "Let rtkit Give More".into(),
        root: true,
        steps: vec![
            Step::WriteFile {
                path: "/etc/systemd/system/rtkit-daemon.service.d/olo.conf".into(),
                content: format!(
                    "# Olo: room for the voice services' sound to be realtime (Olo Doctor).
[Service]
ExecStart=
ExecStart=/{d} --processes-per-user-max=64 --threads-per-user-max=256
"
                ),
                mode: 0o644,
            },
            Step::Unit { name: "rtkit-daemon.service".into(), action: UnitAction::Reload },
            Step::Unit { name: "rtkit-daemon.service".into(), action: UnitAction::Restart },
        ],
    });
    r.findings.push(Finding {
        id: "voice-realtime".into(),
        severity: Severity::Advice,
        title: "Voice processing's sound isn't realtime".into(),
        detail: format!(
            "{} asked rtkit for realtime and didn't get it: rtkit gives it to only so many processes a user (15 by default), and this desktop's programs have used them up. Under load, a frame of a voice can then wait behind other work, a click in a call. The fix lets rtkit give it to 64 processes and 256 threads, restarts it, and asks it again for their sound.",
            not.join(" and ")
        ),
        fix,
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

/// NVIDIA's kernel modules, in the order they're unloaded (each before
/// what it uses): its DRM driver first, `nvidia` itself last.
const NVIDIA_MODULES: [&str; 5] = ["nvidia_drm", "nvidia_modeset", "nvidia_uvm", "nvidia_peermem", "nvidia"];
/// The services that keep NVIDIA's GPUs open: NVIDIA's own and Olo's GPU
/// follower (`olo-gpu-tune follow`, which holds NVML), stopped while its
/// driver starts again and started after (those running or meant to:
/// one a failed try left stopped is started again too).
const NVIDIA_SERVICES: [&str; 3] = ["nvidia-powerd.service", "nvidia-persistenced.service", "olo-gpu-follow.service"];
/// Olo's own that can't be quit to free a card: the session's.
const SESSION_PROGRAMS: [&str; 3] = ["olo-app-server", "olo-session", "olo-wlproxy"];

/// A graphics card whose power management failed: the kernel has it in
/// an error state (`runtime_status` "error") and its driver doesn't
/// answer until it starts again, as NVIDIA's after memory ran out
/// (2026-11-07: NVML said "an internal driver error occurred"). NVIDIA's
/// is started again without restarting the computer (Ben: "i don't want
/// you to restart the machine just fix the card/driver or whatever"):
/// what of the person's uses it quit, NVIDIA's services stopped, its
/// modules unloaded (refused at once while anything still uses them),
/// the card reset and found on the bus again (`Step::RestartPciDevice`:
/// fresh power management), its driver loaded by udev as at start (the
/// card's PCI alias names nvidia_drm), the services started again.
fn gpu_stuck(roots: &Roots, pcis: &[Pci], procs: &[Proc], r: &mut Report) {
    for p in pcis.iter().filter(|p| p.display() && read(&p.dir.join("power/runtime_status")).as_deref() == Some("error")) {
        let maker = p.maker();
        let id = format!("gpu:stuck:{}", p.address);
        let title = format!("The {maker} graphics card stopped answering");
        let broken = "Its power management failed (the kernel has it in an error state) and its driver doesn't answer, so nothing can use it: video it would decode, screen recordings it would encode, a display on its ports.";
        if p.vendor != 0x10de || p.driver.as_deref() != Some("nvidia") {
            r.findings.push(Finding { id, severity: Severity::Problem, title, detail: format!("{broken} Restarting the computer starts it again."), fix: None });
            continue;
        }
        // Its device nodes: its DRM card and render node, and NVIDIA's own
        // (/dev/nvidia0, nvidiactl, nvidia-uvm…).
        let mut nodes: Vec<String> = std::fs::read_dir(p.dir.join("drm"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("card") || n.starts_with("renderD"))
            .map(|n| format!("/dev/dri/{n}"))
            .collect();
        nodes.sort();
        let uses = |link: &str| link.starts_with("/dev/nvidia") || nodes.iter().any(|n| n == link);
        let holds = |pid: i32| {
            std::fs::read_dir(roots.fs.join("proc").join(pid.to_string()).join("fd"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|f| std::fs::read_link(f.path()).ok())
                .any(|l| uses(&l.to_string_lossy()))
        };
        let users: Vec<&Proc> = procs.iter().filter(|q| q.uid == roots.uid && holds(q.pid)).collect();
        if users.iter().any(|q| SESSION_PROGRAMS.contains(&q.name.as_str())) {
            r.findings.push(Finding {
                id,
                severity: Severity::Problem,
                title,
                detail: format!(
                    "{broken} Olo can start it again without restarting the computer, but the session's display server has it open for a display on its ports: unplug that display, then Check Again."
                ),
                fix: None,
            });
            continue;
        }
        let services: Vec<&str> = NVIDIA_SERVICES.into_iter().filter(|u| (roots.units)(u).is_ok_and(|s| s.active || s.enabled)).collect();
        let mut steps: Vec<Step> = users.iter().map(|q| Step::Signal { pid: q.pid, signal: libc::SIGTERM }).collect();
        steps.push(Step::RestartGpu {
            address: p.address.clone(),
            modules: NVIDIA_MODULES.into_iter().filter(|m| roots.fs.join("sys/module").join(m).exists()).map(String::from).collect(),
            services: services.iter().map(|u| (*u).to_owned()).collect(),
            in_use_by: std::iter::once("/dev/nvidia*".to_owned()).chain(nodes.iter().cloned()).collect(),
        });
        // What it quits, each program once: "chrome (processes 1 and 2)".
        let mut programs: Vec<(&str, Vec<i32>)> = Vec::new();
        for q in &users {
            match programs.iter_mut().find(|(n, _)| *n == q.name) {
                Some((_, pids)) => pids.push(q.pid),
                None => programs.push((&q.name, vec![q.pid])),
            }
        }
        let named: Vec<String> = programs
            .iter()
            .map(|(n, pids)| match pids.as_slice() {
                [pid] => format!("{n}, process {pid}"),
                [rest @ .., last] => format!("{n}, processes {} and {last}", rest.iter().map(i32::to_string).collect::<Vec<_>>().join(", ")),
                [] => String::new(),
            })
            .collect();
        let quits = if named.is_empty() { String::new() } else { format!("it quits what uses it ({}: open them again after), ", named.join("; ")) };
        let stops = if services.is_empty() { String::new() } else { format!(" ({} stopped meanwhile)", services.iter().map(|u| u.trim_end_matches(".service")).collect::<Vec<_>>().join(" and ")) };
        r.findings.push(Finding {
            id,
            severity: Severity::Problem,
            title,
            detail: format!(
                "{broken} The fix starts it again without restarting the computer: {quits}NVIDIA's driver is unloaded{stops}, the card reset and found on the bus again, and the driver loads as at start. If it still doesn't answer, restarting the computer does."
            ),
            fix: Some(Fix { label: "Start It Again".into(), root: true, steps }),
        });
    }
}

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

/// The login keyring opened at login: gnome-keyring keeps the secrets
/// (Passwords' key, Chrome's, Wi-Fi's), and only its PAM module, given
/// the password as GDM logs one in, opens it (makes it, the first time).
/// Without the module (Fedora's `gnome-keyring-pam`, not installed with
/// gnome-keyring) the keyring stays locked and asks for its password, or
/// is never made: Passwords couldn't keep its key (the ThinkPad P1
/// Gen 9, 2026-11-04).
fn login_keyring(roots: &Roots, r: &mut Report) {
    // Olo's keyring answering, its key sealed by the login password
    // (pam_olo_keyring opens it at Olo's login screen): nothing of GNOME's
    // is needed.
    let sealed = roots.bin.parent().is_some_and(|local| local.parent().is_some_and(|home| home.join(".config/olo/secrets.login").exists()));
    if sealed && secrets_status(roots).is_some_and(|v| v["serving"].as_bool() == Some(true)) {
        r.fine.push(Fine {
            title: "Olo's keyring opens when you log in".into(),
            detail: "Programs' secrets are in Olo's keyring, which your login password opens (at Olo's login screen; another asks for it once).".into(),
        });
        return;
    }
    if !roots.fs.join("usr/bin/gnome-keyring-daemon").exists() {
        return;
    }
    let module = ["usr/lib64/security", "usr/lib/security", "lib64/security", "usr/lib/x86_64-linux-gnu/security", "usr/lib/aarch64-linux-gnu/security"]
        .iter()
        .any(|d| roots.fs.join(d).join("pam_gnome_keyring.so").exists());
    if module {
        r.fine.push(Fine { title: "The login keyring opens when you log in".into(), detail: "gnome-keyring's PAM module is installed: logging in opens it with your password.".into() });
        return;
    }
    r.findings.push(Finding {
        id: "keyring:pam".into(),
        severity: Severity::Problem,
        title: "The login keyring doesn't open when you log in".into(),
        detail: format!(
            "gnome-keyring keeps the secrets apps keep (Passwords' key, Chrome's, Wi-Fi passwords), and logging in opens it only through its PAM module, which isn't installed ({KEYRING_PAM}). So it stays locked and asks for its password, or was never made. The fix installs it; the keyring then opens at your next login, if its password is your login password."
        ),
        fix: Some(Fix { label: "Install the Keyring's Login Module".into(), root: true, steps: vec![Step::InstallPackages { names: vec![KEYRING_PAM.into()] }] }),
    });
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
/// that; found if it still has two (the fix starts it again). With GNOME
/// Keyring answering, its own two are said (olo-secrets merges them when
/// it takes over). Two, gh (go-keyring, which reads `login`) lost its
/// token to the other (Ben, 2026-11-06).
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
    // GNOME Keyring answering: its own two, if it has them.
    let Some(dir) = roots.bin.parent().map(|local| local.join("share/keyrings")) else { return };
    if !dir.join("login.keyring").exists() {
        return;
    }
    let others: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".keyring").map(str::to_owned))
        .filter(|stem| stem != "login" && stem.eq_ignore_ascii_case("login"))
        .collect();
    if others.is_empty() {
        return;
    }
    r.findings.push(Finding {
        id: "secrets:gnome-two-logins".into(),
        severity: Severity::Advice,
        title: "GNOME Keyring has two login keyrings".into(),
        detail: format!(
            "Besides “login” it has “{}”, the default, so programs that keep secrets in the default and those that read “login” (gh, through go-keyring) don't see each other's. Olo's own keyring (Preferences ▸ Security ▸ Programs' Secrets) merges them into one when it takes over.",
            others.join("”, “")
        ),
        fix: None,
    });
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
        fix: Some(Fix { label: "Turn On HDR at Start".into(), root: true, steps: vec![Step::AddKernelArgument { arg }] }),
    });
}

/// gnome-keyring's PAM module, as Fedora packages it.
const KEYRING_PAM: &str = "gnome-keyring-pam";

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
    let steps = waiting.iter().map(|(n, _)| Step::WriteSysfs { path: PathBuf::from("/sys/bus/thunderbolt/devices").join(n).join("authorized"), value: "1".into() }).collect();
    r.findings.push(Finding {
        id: format!("thunderbolt:{}", waiting.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(",")),
        severity: Severity::Problem,
        title: format!("{} is plugged in but not allowed", names.join(", ")),
        detail: "This computer's Thunderbolt lets a device in only once it's allowed, and it hasn't been: until then a dock, a display or a drive on it does nothing. Allow it if you know it.".into(),
        fix: Some(Fix { label: "Allow It".into(), root: true, steps }),
    });
}

/// Intel's Thunderbolt 5 controller (Barlow Ridge) has a USB controller
/// of its own (8086:5782) for its USB-C ports. A USB 2 device in such a
/// port (Ben's YubiKey, 2026-11-04) wasn't seen at all, the kernel hearing
/// nothing, while USB 3 drives were and the key worked at once in another
/// port: so said, with what to do meanwhile. Whether the controller,
/// asleep, misses a USB 2 plug is still to find; the fix keeps it awake
/// until the next restart, so plugging the device in again tells.
fn thunderbolt_usb2(pcis: &[Pci], r: &mut Report) {
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
    if xhci.power_control.as_deref() == Some("on") {
        r.fine.push(Fine {
            title: "The Thunderbolt 5 ports' USB is kept awake".into(),
            detail: "Plug a USB 2 device (a security key, a keyboard) into a Thunderbolt 5 port: if it's seen now, its controller was missing the plug while asleep. Until the next restart.".into(),
        });
        return;
    }
    let mut steps = vec![Step::WriteSysfs { path: sys.join("power/control"), value: "on".into() }];
    steps.extend(hubs.iter().map(|h| Step::WriteSysfs { path: sys.join(h).join("power/control"), value: "on".into() }));
    r.findings.push(Finding {
        id: format!("usb2:thunderbolt5:{}", xhci.address),
        severity: Severity::Advice,
        title: "A USB 2 device in a Thunderbolt 5 port may go unseen".into(),
        detail: "This computer's Thunderbolt 5 ports have a USB controller of their own, and a USB 2 device in one (a security key, a keyboard, a mouse's receiver) wasn't seen at all here, though USB 3 drives were and the same key worked at once in another port. Meanwhile, plug such devices into another USB-C port. To find out whether the controller misses the plug while it sleeps, the fix keeps it awake until the next restart (a little power): then plug the device in again.".into(),
        fix: Some(Fix { label: "Keep It Awake for Now".into(), root: true, steps }),
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
        severity: Severity::Problem,
        title: "The built-in speakers and microphones have no driver".into(),
        detail: format!(
            "Linux finds {chips} but doesn't know how this computer wires them, so it makes no sound card of them: no speakers, microphones or headphone jack. Sound plays through Bluetooth, USB and HDMI meanwhile. {then}"
        ),
        fix,
    });
}

fn stale(roots: &Roots, procs: &[Proc], r: &mut Report) {
    let me = std::process::id() as i32;
    let bin = roots.bin.to_string_lossy();
    let old: Vec<&Proc> = procs
        .iter()
        .filter(|p| p.uid == roots.uid && p.pid != me)
        .filter(|p| p.exe.as_deref().is_some_and(|e| e.ends_with(" (deleted)") && e.starts_with(&*bin)))
        .filter(|p| !NEVER_STOPPED.contains(&p.name.as_str()))
        .filter(|p| !on_a_screen(&roots.fs, p.pid))
        .collect();
    if old.is_empty() {
        r.fine.push(Fine { title: "Every Olo program runs the installed version".into(), detail: String::new() });
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
            r.findings.push(Finding {
                id: "login:own-older".into(),
                severity: Severity::Advice,
                title: "Olo's login screen runs an older version".into(),
                detail: format!(
                    "It runs copies of Olo's programs of its own ({}), which an upgrade doesn't replace. The fix copies the installed ones over them, for the next time it shows.",
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
            bluetooth_stuck: Vec::new(),
            packages: Box::new(|_| None),
            updates: Box::new(|| None),
            temp: TempFiles::default(),
        };
        (fs, roots)
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
    /// found (its fix starts it again), its mending said, and GNOME
    /// Keyring's own two said when it answers.
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
        // GNOME Keyring answering, with its own two.
        w(&status, r#"{"serving":false,"collections":[]}"#);
        w("home/me/.local/share/keyrings/login.keyring", "");
        w("home/me/.local/share/keyrings/Login.keyring", "");
        let r = scan(&roots);
        let f = r.find("secrets:gnome-two-logins").expect("GNOME's two");
        assert!(f.fix.is_none() && f.detail.contains("“Login”"), "{}", f.detail);
        std::fs::remove_file(fs.join("home/me/.local/share/keyrings/Login.keyring")).unwrap();
        assert!(scan(&roots).find("secrets:gnome-two-logins").is_none());
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
        assert_eq!(paths, ["/sys/bus/pci/devices/0000:a9:00.0/power/control=on", "/sys/bus/pci/devices/0000:a9:00.0/usb5/power/control=on", "/sys/bus/pci/devices/0000:a9:00.0/usb6/power/control=on"]);
        w(&format!("{dev}/power/control"), "on");
        let r = scan(&roots);
        assert!(r.find("usb2:thunderbolt5:0000:a9:00.0").is_none());
        assert!(r.fine.iter().any(|f| f.title == "The Thunderbolt 5 ports' USB is kept awake"));
    }

    /// What's ignored is out of the counts and Fix All, still found and
    /// fixable by its id, and back among the rest when no longer ignored.
    #[test]
    fn ignored_findings_are_set_aside() {
        let (_fs, roots) = machine("ignored");
        let mut r = scan(&roots);
        let before = (r.problems(), r.fixable().count(), r.findings.len());
        let id = r.fixable().next().expect("a finding with a fix").id.clone();
        let problem = r.find(&id).unwrap().severity == Severity::Problem;
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
        // (GNOME Keyring only while Olo's keyring answers.)
        assert_eq!(f.title, "2 of GNOME's services run", "{}", f.detail);
        // (A program running twice is said once, with both processes.)
        assert!(f.detail.contains("(dconf-service, process 950)") && f.detail.contains("(gvfsd, processes 951 and 954)") && f.detail.contains("network folders"), "{}", f.detail);
        assert!(!f.detail.contains("xdg-desktop-portal-gtk"), "{}", f.detail);
        assert_eq!(f.fix.unwrap().steps, [950, 951, 954].map(|pid| Step::Signal { pid, signal: libc::SIGTERM }));
        w(&format!("run/user/{}/olo-secrets.json", roots.uid), "{\"serving\": true}");
        assert_eq!(scan(&roots).find("gnome:services").unwrap().title, "3 of GNOME's services run");
    }

    #[test]
    fn finds_what_is_wrong_and_what_is_fine() {
        let (fs, mut roots) = machine("wrong");
        // Programs' temporary files in memory too.
        roots.temp = TempFiles { tmp_in_memory: true, config: fs.join("home/me/.config"), data: fs.join("home/me/.local/share"), ..TempFiles::default() };
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
        assert!(voice.detail.starts_with("olo-voice asked rtkit"), "{}", voice.detail);
        assert!(matches!(&voice.fix.as_ref().unwrap().steps[0], Step::WriteFile { content, .. } if content.contains("ExecStart=/usr/libexec/rtkit-daemon --processes-per-user-max=64")));
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
        assert!(matches!(&steps[0], Step::WriteFile { path, content, .. } if path == Path::new("/etc/udev/rules.d/71-olo-input.rules") && content == INPUT_RULE));
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
        let foreign = r.find("foreign").unwrap();
        assert!(foreign.detail.contains("GNOME's power daemon (gsd-power, process 901)"));
        let login = r.find("login").unwrap();
        assert!(
            matches!(&login.fix.as_ref().unwrap().steps[0], Step::WriteFile { content, .. } if content.contains(&format!("Exec={}/olo-session\n", roots.bin.display())) && !content.contains("TryExec"))
        );
        // Written where GDM always looks (the old one, in /usr/local, read
        // only when GDM's XDG_DATA_DIRS names it).
        assert!(matches!(&login.fix.as_ref().unwrap().steps[0], Step::WriteFile { path, .. } if path == Path::new("/usr/share/wayland-sessions/olo.desktop")));
        assert!(r.fine.is_empty(), "{:?}", r.fine);
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

    /// A ThinkPad P1 Gen 9: Intel graphics it started on, an NVIDIA card
    /// nouveau couldn't start, kept awake, and SoundWire sound chips with
    /// no card made of them.
    #[test]
    fn the_login_keyring_opens_at_login() {
        let (fs, roots) = machine("keyring");
        let w = |p: &str| {
            let path = fs.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        };
        // gnome-keyring, without its PAM module (pam_oo7 beside it).
        w("usr/bin/gnome-keyring-daemon");
        w("usr/lib64/security/pam_oo7.so");
        let mut r = Report::default();
        login_keyring(&roots, &mut r);
        let f = r.find("keyring:pam").expect("the module missing");
        assert_eq!(f.fix.as_ref().unwrap().steps, vec![Step::InstallPackages { names: vec!["gnome-keyring-pam".into()] }]);
        w("usr/lib64/security/pam_gnome_keyring.so");
        let mut r = Report::default();
        login_keyring(&roots, &mut r);
        assert!(r.findings.is_empty());
        assert_eq!(r.fine[0].title, "The login keyring opens when you log in");
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
        assert_eq!(fix.steps, vec![Step::AddKernelArgument { arg: "xe.enable_dpcd_backlight=3".into() }]);
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
        w(&format!("{tb}/0-1/vendor_name"), "Lenovo\n");
        w(&format!("{tb}/0-1/device_name"), "ThinkPad Thunderbolt 4 Dock\n");
        let mut r = Report::default();
        thunderbolt(&roots, &mut r);
        let f = r.find("thunderbolt:0-1").expect("the dock");
        assert_eq!(f.title, "Lenovo ThinkPad Thunderbolt 4 Dock is plugged in but not allowed");
        let fix = f.fix.as_ref().unwrap();
        assert!(fix.root);
        assert_eq!(fix.steps, vec![Step::WriteSysfs { path: "/sys/bus/thunderbolt/devices/0-1/authorized".into(), value: "1".into() }]);
        // Allowed: fine.
        w(&format!("{tb}/0-1/authorized"), "1\n");
        let mut r = Report::default();
        thunderbolt(&roots, &mut r);
        assert!(r.findings.is_empty());
        assert_eq!(r.fine[0].title, "Thunderbolt devices are let in");
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

    /// NVIDIA's card with its power management failed: started again
    /// without a restart, what uses it quit first, nvidia-powerd stopped
    /// and started; none offered while the session's display server has
    /// it open; nothing said once it answers again.
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
    fn a_stuck_nvidia_card_is_started_again() {
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
        std::fs::create_dir_all(fs.join(format!("{card}/drm/card1"))).unwrap();
        std::fs::create_dir_all(fs.join(format!("{card}/drm/renderD129"))).unwrap();
        std::fs::create_dir_all(fs.join("sys/bus/pci/drivers/nvidia")).unwrap();
        std::os::unix::fs::symlink(fs.join("sys/bus/pci/drivers/nvidia"), fs.join(format!("{card}/driver"))).unwrap();
        for m in ["nvidia", "nvidia_drm", "nvidia_modeset", "nvidia_uvm"] {
            std::fs::create_dir_all(fs.join("sys/module").join(m)).unwrap();
        }
        // Two of Chrome's processes on its render node, Activity Monitor
        // on NVIDIA's own node, Deskbar on neither.
        let open = |pid: u32, cmd: &str, node: &str| {
            w(&format!("proc/{pid}/cmdline"), &format!("{cmd}\0"));
            std::fs::create_dir_all(fs.join(format!("proc/{pid}/fd"))).unwrap();
            std::os::unix::fs::symlink(node, fs.join(format!("proc/{pid}/fd/9"))).unwrap();
        };
        open(950, "/opt/chrome/chrome", "/dev/dri/renderD129");
        // (Its command line rewritten into one word, as Chrome does.)
        open(951, "/opt/chrome/chrome --type=gpu-process --user-data-dir=/tmp/x/profile", "/dev/dri/renderD129");
        open(952, "/home/me/.local/bin/olo-activity", "/dev/nvidia0");
        open(953, "/home/me/.local/bin/olo-tracker", "/dev/dri/renderD128");
        // nvidia-powerd running; Olo's follower installed but stopped (a
        // failed try): both stopped meanwhile, and started after.
        let roots = Roots {
            units: Box::new(|name: &str| -> Result<UnitState, String> {
                Ok(UnitState { loaded: true, active: name == "nvidia-powerd.service", enabled: name != "nvidia-persistenced.service", path: None })
            }),
            ..roots
        };
        let f = scan(&roots).find("gpu:stuck:0000:01:00.0").cloned().expect("the stuck card");
        assert_eq!((f.title.as_str(), f.severity), ("The NVIDIA graphics card stopped answering", Severity::Problem));
        assert!(f.detail.contains("it quits what uses it (chrome, processes 950 and 951; olo-activity, process 952: open them again after)"), "{}", f.detail);
        assert!(f.detail.contains("(nvidia-powerd and olo-gpu-follow stopped meanwhile)"), "{}", f.detail);
        let fix = f.fix.expect("a fix");
        assert_eq!((fix.label.as_str(), fix.root), ("Start It Again", true));
        let quit = |pid| Step::Signal { pid, signal: libc::SIGTERM };
        assert_eq!(
            fix.steps,
            vec![
                quit(950),
                quit(951),
                quit(952),
                Step::RestartGpu {
                    address: "0000:01:00.0".into(),
                    modules: ["nvidia_drm", "nvidia_modeset", "nvidia_uvm", "nvidia"].map(String::from).to_vec(),
                    services: ["nvidia-powerd.service", "olo-gpu-follow.service"].map(String::from).to_vec(),
                    in_use_by: ["/dev/nvidia*", "/dev/dri/card1", "/dev/dri/renderD129"].map(String::from).to_vec(),
                },
            ]
        );
        // The display server has it open (a display on its port): it
        // can't be freed, so no fix, and what to do.
        open(954, "/home/me/.local/bin/olo-app-server", "/dev/dri/card1");
        let f = scan(&roots).find("gpu:stuck:0000:01:00.0").cloned().unwrap();
        assert!(f.fix.is_none() && f.detail.contains("unplug that display"), "{}", f.detail);
        // Answering again: nothing to say.
        w(&format!("{card}/power/runtime_status"), "active\n");
        assert!(scan(&roots).find("gpu:stuck:0000:01:00.0").is_none());
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
                bluetooth_stuck: Vec::new(),
                packages: Box::new(|_| None),
                updates: Box::new(|| None),
                temp: TempFiles {
                    tmp_in_memory,
                    tmpdir: tmpdir.map(str::to_owned),
                    tmpdir_in_memory,
                    config: PathBuf::from("/home/me/.config"),
                    data: PathBuf::from("/home/me/.local/share"),
                    runtime: PathBuf::new(),
                },
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
