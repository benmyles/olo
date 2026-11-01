//! What Olo Doctor looks for: programs that shouldn't be running beside
//! Olo (input remappers that grab the mice and keyboards, another
//! desktop's daemons and portals), settings that aren't what Olo wants
//! (USB input devices allowed to fall asleep, Olo's udev rules not
//! installed, rtkit not running for the Performance supervisor), and
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
/// The login screen's entry (its Exec made absolute on install).
const SESSION_ENTRY: &str = include_str!("../../../data/session/olo.desktop");

const INPUT_RULE_PATH: &str = "etc/udev/rules.d/71-olo-input.rules";
const DISPLAY_RULE_PATH: &str = "etc/udev/rules.d/70-olo-displays.rules";
const SESSION_ENTRY_PATH: &str = "usr/local/share/wayland-sessions/olo.desktop";

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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub fine: Vec<Fine>,
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
}

impl Roots {
    pub fn real() -> Roots {
        // Where tools/install-session.sh puts them (and the upgrade).
        let bin = PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/bin");
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let scope = scope_of(&std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default());
        Roots { fs: PathBuf::from("/"), bin, uid, scope, units: Box::new(crate::fix::unit_state) }
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
    std::fs::read_to_string(fs.join("proc/bus/input/devices"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("N: Name=\""))
        .map(|n| n.trim_end_matches('"').to_owned())
        .collect()
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
pub fn scan(roots: &Roots) -> Report {
    let mut r = Report::default();
    let procs = processes(&roots.fs);
    let input_names = input_device_names(&roots.fs);
    remappers(roots, &procs, &input_names, &mut r);
    foreign(roots, &procs, &mut r);
    let rule_current = input_rule(roots, &mut r);
    autosuspend(roots, rule_current, &mut r);
    display_rule(roots, &mut r);
    rtkit(roots, &mut r);
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
    let path = roots.fs.join(SESSION_ENTRY_PATH);
    let session = roots.bin.join("olo-session");
    let wanted: String = SESSION_ENTRY.lines().filter(|l| !l.starts_with("TryExec=")).map(|l| if l.starts_with("Exec=") { format!("Exec={}", session.display()) } else { l.to_owned() }).collect::<Vec<_>>().join("\n") + "\n";
    let installed = std::fs::read_to_string(&path).ok();
    let exec = installed.as_deref().and_then(|t| t.lines().find_map(|l| l.strip_prefix("Exec=")).map(str::to_owned));
    let runs = exec.as_deref().is_some_and(|e| Path::new(e).exists() || !e.contains('/'));
    if installed.is_some() && runs {
        r.fine.push(Fine { title: "Olo is on the login screen".into(), detail: format!("/{SESSION_ENTRY_PATH} runs {}.", exec.unwrap_or_default()) });
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
        detail: format!("{} should start {}. The fix writes it.", path.display(), session.display()),
        fix: Some(Fix { label: "Write the Entry".into(), root: true, steps: vec![Step::WriteFile { path: PathBuf::from("/").join(SESSION_ENTRY_PATH), content: wanted, mode: 0o644 }] }),
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
        let bin = fs.join("home/me/.local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("olo-session"), "").unwrap();
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
        let roots = Roots { fs: fs.clone(), bin, uid, scope: Some("session-2.scope".into()), units };
        (fs, roots)
    }

    #[test]
    fn finds_what_is_wrong_and_what_is_fine() {
        let (fs, roots) = machine("wrong");
        let r = scan(&roots);
        let ids: Vec<&str> = r.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(r.find("foreign").unwrap().title, "A program of another desktop runs in this session");
        assert_eq!(ids, ["remapper:keyd", "foreign", "rule:input", "autosuspend:3-1.3", "rule:displays", "rtkit", "stale", "login"], "{r:#?}");
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
        assert!(matches!(&login.fix.as_ref().unwrap().steps[0], Step::WriteFile { content, .. } if content.contains(&format!("Exec={}/olo-session\n", roots.bin.display())) && !content.contains("TryExec")));
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
        w(INPUT_RULE_PATH, INPUT_RULE);
        w(DISPLAY_RULE_PATH, DISPLAY_RULE);
        w(SESSION_ENTRY_PATH, &format!("[Desktop Entry]\nExec={}/olo-session\n", roots.bin.display()));
        roots.units = Box::new(|name: &str| Ok(if name == "rtkit-daemon.service" { UnitState { loaded: true, active: true, enabled: true, path: None } } else { UnitState::default() }));
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
                "rtkit is running",
                "Every Olo program runs the installed version",
                "Olo is on the login screen",
            ]
        );
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

    #[test]
    fn rule_ids_are_read_from_the_rule() {
        let ids = rule_ids(DISPLAY_RULE);
        assert!(ids.contains(&("05ac".into(), "1114".into())));
        assert_eq!(ids.len(), 5);
    }
}
