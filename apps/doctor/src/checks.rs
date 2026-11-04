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
}

impl Report {
    pub fn problems(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Problem).count()
    }

    pub fn fixable(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.fix.is_some())
    }

    pub fn find(&self, id: &str) -> Option<&Finding> {
        self.findings.iter().find(|f| f.id == id)
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
    /// The system's packages (PackageKit's word): which of these names
    /// there are, newest, and whether installed. None: no PackageKit.
    pub packages: Box<dyn Fn(&[&str]) -> Option<Vec<(String, bool)>> + Send + Sync>,
    /// The names of the packages with an update waiting (None: no
    /// PackageKit).
    pub updates: Box<dyn Fn() -> Option<Vec<String>> + Send + Sync>,
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
        Roots { fs: PathBuf::from("/"), bin, uid, scope, units: Box::new(crate::fix::unit_state), airpods, packages: Box::new(real_packages), updates: Box::new(real_updates) }
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
        let mut name = String::from_utf8_lossy(first).rsplit('/').next().unwrap_or("").to_owned();
        if name.is_empty() {
            name = std::fs::read_to_string(dir.join("comm")).unwrap_or_default().trim().to_owned();
        }
        let exe = std::fs::read_link(dir.join("exe")).ok().map(|p| p.to_string_lossy().into_owned());
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
    let report = scan(&Roots::real());
    let issues: Vec<&Finding> = report.findings.iter().filter(|f| f.severity == Severity::Problem || f.fix.is_some()).collect();
    let found = olo_protocol::doctor::DoctorFound {
        issues: issues.iter().map(|f| f.title.clone()).collect(),
        problems: issues.iter().filter(|f| f.severity == Severity::Problem).count(),
        at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    };
    if let Err(e) = found.save() {
        tracing::warn!("doctor: what was found not saved: {e}");
    }
    report
}

pub fn scan(roots: &Roots) -> Report {
    let mut r = Report::default();
    let procs = processes(&roots.fs);
    let input_names = input_device_names(&roots.fs);
    remappers(roots, &procs, &input_names, &mut r);
    foreign(roots, &procs, &mut r);
    let rule_current = input_rule(roots, &mut r);
    autosuspend(roots, rule_current, &mut r);
    display_rule(roots, &mut r);
    wifi(roots, &mut r);
    rtkit(roots, &mut r);
    voice_realtime(roots, &procs, &mut r);
    airpods_listening(roots, &procs, &mut r);
    util_clamp(roots, &mut r);
    let gpus = pci_devices(&roots.fs);
    idle_gpu(roots, &gpus, &mut r);
    gpu_driver(roots, &gpus, &mut r);
    builtin_sound(roots, &gpus, &mut r);
    thunderbolt(roots, &mut r);
    login_keyring(roots, &mut r);
    stale(roots, &procs, &mut r);
    login_entry(roots, &mut r);
    r
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
        procs.iter().filter(|p| ours(p) && (p.scope.is_none() || p.scope == roots.scope)).filter_map(|p| FOREIGN.iter().find(|(n, _)| *n == p.name).map(|(_, what)| (p, *what))).collect()
    };
    if mine.is_empty() {
        r.fine.push(Fine { title: "No other desktop's daemons run in this session".into(), detail: "Nothing fights Olo for the keys or the power settings.".into() });
        return;
    }
    let names: Vec<String> = mine.iter().map(|(p, what)| format!("{what} ({}, process {})", p.name, p.pid)).collect();
    let steps = mine.iter().map(|(p, _)| Step::Signal { pid: p.pid, signal: libc::SIGTERM }).collect();
    r.findings.push(Finding {
        id: "foreign".into(),
        severity: Severity::Problem,
        title: if mine.len() == 1 { "A program of another desktop runs in this session".into() } else { format!("{} programs of another desktop run in this session", mine.len()) },
        detail: format!("{}. They answer to keys and power events meant for Olo, and some grab devices. Olo does all of this itself; they aren't needed here and are quit.", names.join("; ")),
        fix: Some(Fix { label: "Quit Them".into(), root: false, steps }),
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

/// Olo's Wi-Fi healer, on a chip whose firmware can stop joining.
fn wifi(roots: &Roots, r: &mut Report) {
    let Some((iface, driver)) = wifi_chips(&roots.fs).into_iter().find(|(_, d)| HEALED_DRIVERS.contains(&d.as_str())) else { return };
    let source = roots.bin.join("olo-wifi-heal");
    if !source.exists() {
        // Not installed (a build tree): nothing to install it from.
        return;
    }
    let state = (roots.units)("olo-wifi-heal.service").unwrap_or_default();
    let installed = roots.fs.join(HEAL_BIN_PATH);
    let current = same_healer(&source, &installed)
        && read(&roots.fs.join(HEAL_UNIT_PATH)).as_deref() == Some(HEAL_UNIT.trim())
        && read(&roots.fs.join(BRCMFMAC_CONF_PATH)).as_deref() == Some(BRCMFMAC_CONF.trim());
    if current && state.active && state.enabled {
        r.fine.push(Fine { title: format!("Olo's Wi-Fi healer watches {iface}"), detail: "If the chip's firmware stops joining, it is started again and the network joined again.".into() });
        return;
    }
    let (title, label) = if !installed.exists() {
        ("Olo's Wi-Fi healer isn't installed", "Install the Healer")
    } else if !current {
        ("Olo's Wi-Fi healer is out of date", "Update the Healer")
    } else {
        ("Olo's Wi-Fi healer isn't running", "Start the Healer")
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
        detail: format!(
            "This Mac's Wi-Fi chip ({driver}, {iface}) can get stuck in its firmware after failed joins: every join then fails, as if the password were wrong, until the chip is started again. Olo's healer, a small system service, notices, starts the chip again, joins the network again and tells you. The fix installs it as a program only the administrator can change ({}), and has NetworkManager keep the chip's own address while it scans.",
            Path::new("/").join(HEAL_BIN_PATH).display()
        ),
        fix: Some(Fix { label: label.into(), root: true, steps }),
    });
}

fn rtkit(roots: &Roots, r: &mut Report) {
    let unit = "rtkit-daemon.service";
    match (roots.units)(unit) {
        Err(e) => r.findings.push(Finding { id: "rtkit".into(), severity: Severity::Advice, title: "systemd couldn't be asked about rtkit".into(), detail: format!("{e}. The Performance supervisor raises Olo's threads through rtkit."), fix: None }),
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
    let audio = |name: &str| olo_protocol::performance::tier(name) == Some(olo_protocol::performance::AUDIO_NICE);
    let mut found = false;
    let mut not = Vec::new();
    for p in procs.iter().filter(|p| p.uid == roots.uid && audio(&p.name)) {
        let tasks = roots.fs.join(format!("proc/{}/task", p.pid));
        for t in std::fs::read_dir(&tasks).into_iter().flatten().flatten() {
            let comm = read(&t.path().join("comm")).unwrap_or_default();
            if !comm.trim().starts_with(olo_protocol::performance::AUDIO_THREAD) {
                continue;
            }
            found = true;
            // The scheduling policy: field 41 of stat (0 normal, 1 FIFO, 2 RR).
            let stat = read(&t.path().join("stat")).unwrap_or_default();
            let policy = stat.rfind(')').and_then(|i| stat[i + 1..].split_whitespace().nth(38)).and_then(|f| f.parse::<u32>().ok());
            if !matches!(policy, Some(1 | 2)) && !not.contains(&p.name) {
                not.push(p.name.clone());
            }
        }
    }
    if !found {
        return;
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
            "{} asked rtkit for realtime and didn't get it: rtkit gives it to only so many processes a user (15 by default), and this desktop's programs have used them up. Under load, a frame of a voice can then wait behind other work, a click in a call. The fix lets rtkit give it to 64 processes and 256 threads, and restarts it.",
            not.join(" and ")
        ),
        fix,
    });
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

    /// A machine in a folder: a keyd running from /usr/local, a mouse
    /// receiver allowed to sleep, a Studio Display, no rules, a stale
    /// Deskbar.
    fn machine(name: &str) -> (PathBuf, Roots) {
        let fs = PathBuf::from(format!("/var/tmp/olo-doctor-test-{}-{name}", std::process::id()));
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
        let roots = Roots { fs: fs.clone(), bin, uid, scope: Some("session-2.scope".into()), units, airpods: true, packages: Box::new(|_| None), updates: Box::new(|| None) };
        (fs, roots)
    }

    #[test]
    fn finds_what_is_wrong_and_what_is_fine() {
        let (fs, roots) = machine("wrong");
        let r = scan(&roots);
        let ids: Vec<&str> = r.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(r.find("foreign").unwrap().title, "A program of another desktop runs in this session");
        assert_eq!(
            ids,
            ["remapper:keyd", "foreign", "rule:input", "autosuspend:3-1.3", "rule:displays", "wifi-heal", "rtkit", "voice-realtime", "airpods-listening", "uclamp", "stale", "login"],
            "{r:#?}"
        );
        let voice = r.find("voice-realtime").unwrap();
        assert!(voice.detail.starts_with("olo-voice asked rtkit"), "{}", voice.detail);
        assert!(matches!(&voice.fix.as_ref().unwrap().steps[0], Step::WriteFile { content, .. } if content.contains("ExecStart=/usr/libexec/rtkit-daemon --processes-per-user-max=64")));
        let uclamp = r.find("uclamp").unwrap();
        assert!(uclamp.detail.starts_with("kernel.sched_util_clamp_min is 512"));
        assert_eq!(uclamp.fix.as_ref().unwrap().steps[1], Step::WriteSysfs { path: "/proc/sys/kernel/sched_util_clamp_min".into(), value: "1024".into() });
        let heal = r.find("wifi-heal").unwrap();
        assert_eq!(heal.title, "Olo's Wi-Fi healer isn't installed");
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
                "Olo's input rule is installed",
                "1 USB input device stays awake",
                "Olo's display rule is installed",
                "Olo's Wi-Fi healer watches wld0",
                "rtkit is running",
                "Voice processing's sound is realtime",
                "Bluetooth listens for the AirPods",
                "Frames run on the fast cores",
                "Every Olo program runs the installed version",
                "Olo is on the login screen",
            ]
        );
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
    fn rule_ids_are_read_from_the_rule() {
        let ids = rule_ids(DISPLAY_RULE);
        assert!(ids.contains(&("05ac".into(), "1114".into())));
        assert_eq!(ids.len(), 5);
    }
}
