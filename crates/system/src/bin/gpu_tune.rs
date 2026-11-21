//! olo-gpu-tune: the processors' and each GPU's power (Preferences ▸
//! Power; `olo_system::gpu_tuning`, `olo_system::cpu_tuning`).
//!
//!   olo-gpu-tune show [--wake]   every GPU, what it allows and its
//!                                tuning, as JSON (NVIDIA's read only
//!                                while it's awake, or with --wake)
//!   olo-gpu-tune set             (root) the tuning on stdin, as JSON
//!                                (`Tuning`: its plan, each GPU's Custom
//!                                knobs): kept in /etc/olo/gpus.json,
//!                                what's in force now applied, and set
//!                                again at every start
//!   olo-gpu-tune apply [CARD]    (root) apply the active graphics policy
//!   olo-gpu-tune follow          (root, olo-gpu-follow.service) the
//!                                processors and every GPU on the plan's
//!                                presets as the power mode or the power
//!                                supply changes
//!
//! `set` installs a copy of this program where only root can change it
//! (/usr/local/libexec/olo-gpu-tune), the udev rule that requests the
//! follower and the service that follows the power mode: a rule or service
//! running a program the person's account could change would run
//! anything as root. Restoring a hardware backup runs `set` through polkit.

#[path = "gpu_tune/events.rs"]
mod events;

use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

use olo_system::{
    battery, cpu_tuning,
    gpu_tuning::{self, Tuning},
    gpus, nvme_power, power_profile, sata_power, systemd,
};

use gpu_tuning::{
    FOLLOWER_PROGRAM as INSTALLED, FOLLOWER_RULE as RULE, FOLLOWER_RULE_PATH as RULE_FILE, FOLLOWER_SERVICE as SERVICE_NAME, FOLLOWER_UNIT as SERVICE, FOLLOWER_UNIT_PATH as SERVICE_FILE,
};
use olo_system::battery::Supply;

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("show") => show(args.iter().any(|a| a == "--wake")),
        Some("set") => set(),
        Some("apply") => apply(args.get(1).map(String::as_str)),
        Some("follow") => follow(),
        _ => {
            eprintln!("usage: olo-gpu-tune show [--wake] | set (root; JSON on stdin) | apply [CARD] (root) | follow (root)");
            2
        }
    };
    std::process::exit(code);
}

fn show(wake: bool) -> i32 {
    let tuning = Tuning::load();
    let followed = gpu_tuning::Followed::load(std::path::Path::new("/")).filter(|s| s.version >= gpu_tuning::FOLLOWER_VERSION);
    let out: Vec<serde_json::Value> = gpus::list()
        .into_iter()
        .map(|g| {
            let (caps, why) = gpu_tuning::caps_or_why(&g, wake);
            serde_json::json!({
                "pci": g.pci,
                "name": g.name,
                "driver": g.driver,
                "asleep": gpu_tuning::asleep(&g),
                "power_failed": gpu_tuning::power_failed(&g),
                "caps": caps,
                "unanswered": why,
                "tuning": followed.as_ref().map_or_else(|| Some(tuning.of(&g.pci)), |s| s.gpus.iter().find(|p| p.pci == g.pci).cloned()),
                "pending": followed.as_ref().is_some_and(|s| s.deferred.contains(&g.pci)),
                "mode": followed.as_ref().map(|s| s.mode.as_str()),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    0
}

/// What applying said, a line each: "ok: …" or "error: …".
fn report(results: Vec<(String, gpu_tuning::Applied)>) -> i32 {
    let mut failed = false;
    let mut out = std::io::stdout().lock();
    for (gpu, lines) in results {
        for line in lines {
            match line {
                Ok(l) => {
                    let _ = writeln!(out, "ok: {gpu}: {l}");
                }
                Err(e) => {
                    failed = true;
                    let _ = writeln!(out, "error: {gpu}: {e}");
                }
            }
        }
    }
    i32::from(failed)
}

fn set() -> i32 {
    let mut json = String::new();
    if std::io::stdin().read_to_string(&mut json).is_err() {
        println!("error: no tuning given");
        return 2;
    }
    let tuning: Tuning = match serde_json::from_str(&json) {
        Ok(t) => t,
        Err(e) => {
            println!("error: the tuning given isn't one: {e}");
            return 2;
        }
    };
    // (Only GPUs this computer has.)
    let here: Vec<String> = gpus::list().into_iter().map(|g| g.pci).collect();
    let tuning = Tuning {
        gpus: tuning.gpus.into_iter().filter(|g| here.contains(&g.pci)).collect(),
        driver: Vec::new(),
        custom: tuning.custom.into_iter().filter(|g| here.contains(&g.pci)).collect(),
        plan: tuning.plan,
    };
    // (The driver's own limits noted before anything is written over
    // them: what Balanced puts back.)
    let tuning = gpu_tuning::with_driver_limits(tuning, &Tuning::load());
    if let Err(e) = keep(&tuning) {
        println!("error: {}: {e}", gpu_tuning::FILE);
        return 1;
    }
    if let Err(e) = install().map_err(|e| e.to_string()).and_then(|_| start_following(true).map_err(|e| e.to_string())) {
        eprintln!("error: applying the restored power settings: {e}");
        return 1;
    }
    0
}

fn ceiling(mode: power_profile::Mode) -> u8 {
    if mode != power_profile::Mode::PowerSaver && std::fs::read_to_string("/run/olo-fans/pressure").is_ok_and(|s| s.trim() == "hot") { 80 } else { 100 }
}

/// Only write a GPU when its effective request changed. Keep pending
/// requests for sleeping GPUs in the follower until a real client uses one.
fn follow_gpus(mode: power_profile::Mode, battery: bool, state: &mut gpu_tuning::Followed, pending_only: bool) -> Vec<(String, gpu_tuning::Applied)> {
    use gpu_tuning::Caps;
    let mut out = Vec::new();
    // (What the person chose for this mode, where they did.)
    let plan = Tuning::load().plan;
    for g in gpus::list() {
        if !matches!(g.driver.as_str(), "xe" | "nvidia") || (pending_only && !state.deferred.contains(&g.pci)) {
            continue;
        }
        // (An NVIDIA card that stopped answering, or whose driver is
        // being unloaded, is left wholly alone: nothing of it written,
        // nothing asked, and no setting counted as refused. Olo Doctor
        // says its state, `gpu_stuck`.)
        if g.vendor == 0x10de && (olo_system::gpu_load::failed(&g.sysfs) || olo_system::gpu_load::unloading(std::path::Path::new("/sys"))) {
            state.deferred.retain(|id| id != &g.pci);
            continue;
        }
        let preset = plan.mode_gpu(mode, &g.pci, g.vendor == 0x10de, battery);
        let previous = state.autosuspend_ms.get(&g.pci).copied();
        match gpu_tuning::runtime_tail(&g.sysfs, plan.mode_gpu_ready(mode), previous) {
            Ok(Some(ms)) => {
                state.autosuspend_ms.insert(g.pci.clone(), ms);
            }
            Ok(None) => {
                state.autosuspend_ms.remove(&g.pci);
            }
            Err(e) => out.push((g.name.clone(), vec![Err(format!("GPU idle delay: {e}"))])),
        }
        // (Asleep, or on its way to sleep or out of it: set at its next
        // open, once it says it's awake.)
        if g.vendor == 0x10de && !olo_system::gpu_load::askable(&g.sysfs) {
            if !state.gpus.iter().any(|t| t.pci == g.pci && t.preset == preset) {
                if !state.deferred.contains(&g.pci) {
                    state.deferred.push(g.pci.clone());
                }
            } else {
                state.deferred.retain(|id| id != &g.pci);
            }
            continue;
        }
        let (caps, why) = gpu_tuning::caps_or_why(&g, false);
        let Some(caps) = caps else {
            out.push((g.name, vec![Err(why.unwrap_or_else(|| "driver capabilities unavailable".into()))]));
            continue;
        };
        let tune = gpu_tuning::with_top(gpu_tuning::preset(&g.pci, preset, &caps), &caps, plan.mode_gpu_top(mode, &g.pci));
        // The kernel's RPe follows operating conditions. Reasserting the
        // same range at every supply event overrides its autonomous policy.
        if !state.gpus.contains(&tune) {
            let done = gpu_tuning::apply(&g, &tune, None);
            if done.iter().all(Result::is_ok) {
                state.gpus.retain(|t| t.pci != g.pci);
                if !matches!(caps, Caps::None) {
                    state.gpus.push(tune);
                }
                state.deferred.retain(|id| id != &g.pci);
            }
            out.push((g.name, done));
        } else {
            state.deferred.retain(|id| id != &g.pci);
        }
    }
    out
}

fn follow_now(mode: power_profile::Mode, on_battery: bool, state: &mut gpu_tuning::Followed) -> Vec<(String, gpu_tuning::Applied)> {
    let mut out = follow_gpus(mode, on_battery, state, false);
    let plan = Tuning::load().plan;
    let cpu = plan.mode_cpu(mode);
    // (The person's top speed for the mode, and under it while the
    // computer runs hot.)
    let top = ceiling(mode).min(plan.mode_cpu_top(mode));
    out.push((format!("processors ({})", cpu.name()), cpu_tuning::apply(cpu, top, on_battery)));
    use olo_system::fans::Places;
    let places = Places::here();
    if let Some(f) = olo_system::fans::look(&places)
        && f.installed
    {
        out.push(("fans".into(), vec![follow_fans(plan.mode_fans(mode), &places)]));
    }
    let (done, drives, limited, sata_found) = follow_drives(&plan, mode, on_battery, state);
    out.extend(done);
    state.version = gpu_tuning::FOLLOWER_VERSION;
    state.mode = mode.id().into();
    state.on_battery = on_battery;
    state.cpu = cpu.id().into();
    state.top = top;
    state.cpu_after = cpu_tuning::caps();
    state.failed = failures(&out);
    (state.drives, state.limited, state.sata_found) = (drives, limited, sata_found);
    if let Err(e) = state.save() {
        eprintln!("couldn't report power settings: {e}");
    }
    out
}

/// What failed in `out`, each as "what: why".
fn failures(out: &[(String, gpu_tuning::Applied)]) -> Vec<String> {
    out.iter().flat_map(|(what, done)| done.iter().filter_map(move |d| d.as_ref().err().map(|e| format!("{what}: {e}")))).collect()
}

/// Each NVMe drive on the plan's preset for `mode` and the supply: a
/// lower working state set, or, Auto, put back on PS0 only if the
/// follower had lowered it (`limited`, by serial: else it's left quite
/// alone). Each SATA SSD likewise, its link's policy: Auto puts back
/// the policy found before the follower first changed it
/// (`sata_found`). What was done, each drive as it is now, those
/// lowered, and the SATA policies found.
type Drives = (Vec<(String, gpu_tuning::Applied)>, Vec<nvme_power::DriveState>, Vec<String>, std::collections::BTreeMap<String, String>);
fn follow_drives(plan: &gpu_tuning::Plan, mode: power_profile::Mode, on_battery: bool, before: &gpu_tuning::Followed) -> Drives {
    let limited = &before.limited;
    let (mut out, mut drives, mut lowered) = (Vec::new(), Vec::new(), Vec::new());
    for d in nvme_power::drives() {
        let what = format!("{} ({})", d.label(), d.name);
        let states = match nvme_power::power_states(&d.name) {
            Ok(s) => s,
            Err(e) => {
                out.push((what, vec![Err(format!("its power states couldn't be read: {e}"))]));
                drives.push(nvme_power::DriveState { serial: d.serial, model: d.model, name: d.name, ..Default::default() });
                continue;
            }
        };
        let preset = plan.nvme_now(mode, on_battery, &d.serial);
        let target = preset.state(&states).or_else(|| limited.contains(&d.serial).then_some(0));
        if let Some(ps) = target {
            let watts = states.iter().find(|s| s.index == ps).map_or(0.0, |s| s.max_watts);
            out.push((what, vec![nvme_power::set(&d.name, ps).map(|()| format!("power state {ps} ({watts:.1} W at most)")).map_err(|e| e.to_string())]));
        }
        if preset.state(&states).is_some() {
            lowered.push(d.serial.clone());
        }
        let now = nvme_power::current(&d.name).ok();
        drives.push(nvme_power::DriveState { serial: d.serial, model: d.model, name: d.name, states, now, ..Default::default() });
    }
    let mut found = before.sata_found.clone();
    for s in sata_power::drives() {
        let d = s.drive;
        let what = format!("{} ({})", d.label(), d.name);
        let preset = plan.nvme_now(mode, on_battery, &d.serial);
        let now = sata_power::current(&s.host).ok();
        let target = match sata_power::policy(preset) {
            Some(p) => {
                // (What it was before the follower first changed it.)
                if !found.contains_key(&d.serial)
                    && let Some(n) = &now
                {
                    found.insert(d.serial.clone(), n.clone());
                }
                lowered.push(d.serial.clone());
                Some(p.to_owned())
            }
            None => limited.contains(&d.serial).then(|| found.remove(&d.serial)).flatten(),
        };
        if let Some(t) = target.filter(|t| now.as_deref() != Some(t.as_str())) {
            out.push((what, vec![sata_power::set(&s.host, &t).map(|()| format!("its link on {t} ({})", sata_power::policy_words(&t))).map_err(|e| e.to_string())]));
        }
        let link = sata_power::current(&s.host).ok();
        drives.push(nvme_power::DriveState { serial: d.serial, model: d.model, name: d.name, link, sata: true, ..Default::default() });
    }
    (out, drives, lowered, found)
}

/// The fans' profile made `profile`: its word written where `olo-fans`
/// watches for it (it takes it within the moment). Nothing written
/// where it's that already. One of Olo's own with the service not set
/// up is said, not written: Preferences sets it up as it's first chosen
/// (Olo Doctor's steps: the program, the unit, the driver told to take
/// levels).
fn follow_fans(profile: olo_system::fans::Profile, places: &olo_system::fans::Places) -> Result<String, String> {
    let Some(state) = olo_system::fans::look(places) else { return Err("this computer's fans can't be set".into()) };
    if profile.ours() && !state.installed {
        return Err("the fan service isn't set up yet: Preferences sets it up when one of its profiles is first chosen".into());
    }
    let file = places.profile();
    if std::fs::read_to_string(&file).ok().and_then(|w| olo_system::fans::Profile::parse(&w)) == Some(profile) {
        return Ok(format!("{} already", profile.label()));
    }
    // (Nothing there and Auto wanted: nothing to say to anyone.)
    if !file.exists() && !profile.ours() {
        return Ok("the computer's own".into());
    }
    file.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, format!("{}\n", profile.word())))
        .map(|()| profile.label().to_owned())
        .map_err(|e| format!("{}: {e}", file.display()))
}

/// `tuning` kept (written whole, then put in place).
fn keep(tuning: &Tuning) -> std::io::Result<()> {
    let file = std::path::Path::new(gpu_tuning::FILE);
    file.parent().map_or(Ok(()), std::fs::create_dir_all)?;
    std::fs::write(file.with_extension("json.new"), serde_json::to_vec_pretty(tuning).unwrap_or_default())?;
    std::fs::rename(file.with_extension("json.new"), file)
}

/// This program where only root can change it, the rule that runs it at
/// start, and the service that follows the power mode, enabled. Whether
/// any of it is new (the service then started again, on it).
fn install() -> std::io::Result<bool> {
    let me = std::fs::read("/proc/self/exe")?;
    let mut new = false;
    if std::fs::read(INSTALLED).ok().as_deref() != Some(&me[..]) {
        new = true;
        let dir = std::path::Path::new(INSTALLED).parent().expect("a folder");
        std::fs::create_dir_all(dir)?;
        let new = format!("{INSTALLED}.new");
        std::fs::write(&new, &me)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&new, INSTALLED)?;
    }
    if std::fs::read_to_string(RULE_FILE).ok().as_deref() != Some(RULE) {
        std::fs::write(RULE_FILE, RULE)?;
    }
    if std::fs::read_to_string(SERVICE_FILE).ok().as_deref() != Some(SERVICE) {
        new = true;
        std::fs::write(SERVICE_FILE, SERVICE)?;
    }
    Ok(new)
}

/// The follower started at every boot and running on what's installed
/// (systemd's own API: told of a new unit file, then started, or started
/// again on a new build).
fn start_following(new: bool) -> olo_system::Result<()> {
    if new {
        systemd::reload()?;
    }
    systemd::enable(SERVICE_NAME, true)?;
    if new { systemd::restart(SERVICE_NAME) } else { systemd::start_stop(SERVICE_NAME, true) }
}

/// `follow` (root, olo-gpu-follow.service): once there's a power profiles
/// daemon, the processors and every GPU (integrated and discrete) on the
/// plan's presets for the mode and the power supply (Preferences ▸ Power:
/// each mode's, and on battery's), now and as either changes: the mode
/// wherever it's changed (Preferences ▸ Power, Deskbar's battery menu, a
/// ThinkPad's Fn keys, `powerprofilesctl`), the supply as UPower says.
/// The daemon sets the processors and the firmware for its mode first;
/// what the plan says is set after it. Woken only by the daemon's and
/// UPower's signals.
fn follow() -> i32 {
    if let Err(e) = power_profile::wait_for_daemon() {
        eprintln!("olo-gpu-tune: no power profiles daemon: {e}");
        return 1;
    }
    enum Change {
        Mode(Option<power_profile::Modes>),
        Supply,
        Settled,
        /// Back from sleep: a drive's controller reset on the way puts it
        /// back on its highest state.
        Woke,
        Applied(String),
        Device,
        Topology,
        Cooling,
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let device = tx.clone();
    if let Err(e) = events::watch(move |e| {
        device
            .send(match e {
                events::Event::Device => Change::Device,
                events::Event::Topology => Change::Topology,
                events::Event::Cooling => Change::Cooling,
            })
            .is_ok()
    }) {
        eprintln!("couldn't watch GPU opens and cooling: {e}");
    }
    let applied = tx.clone();
    if let Err(e) = power_profile::watch_tuned_applied(move |name| {
        let _ = applied.send(Change::Applied(name));
    }) {
        eprintln!("couldn't follow TuneD: {e}");
    }
    let mut state = gpu_tuning::Followed::load(std::path::Path::new("/")).unwrap_or_default();
    // A new process has no NVML handles or verified driver requests yet.
    state.gpus.clear();
    state.deferred.clear();
    let modes = tx.clone();
    power_profile::watch(move |m| {
        let _ = modes.send(Change::Mode(m));
    });
    let woke = tx.clone();
    if let Err(e) = olo_system::power::watch_wake(move || {
        let _ = woke.send(Change::Woke);
    }) {
        eprintln!("olo-gpu-tune: not told of resumes: {e}");
    }
    battery::watch(move |_| {
        let _ = tx.send(Change::Supply);
    });
    // (Its first change opens NVML, held from then on, which keeps NVIDIA's
    // GPU's lock: `gpu_tuning::nvml`.)
    let mut last_tuned = None;
    let mut uses_tuned = false;
    let mut mode = None;
    let mut supply = Supply::new(!battery::on_ac());
    let mut last = None;
    let mut unsteady = false;
    // An open can precede the driver's first runtime resume. Recheck the
    // passive runtime status briefly, only in response to that open.
    let mut probe = None;
    let mut probe_end = None;
    loop {
        let change = match supply.deadline().into_iter().chain(probe).min() {
            Some(end) => match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(change) => change,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if probe.is_some_and(|t| Instant::now() >= t) => Change::Device,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Change::Settled,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(change) => change,
                Err(_) => break,
            },
        };
        match change {
            Change::Mode(Some(ref m)) => {
                uses_tuned = m.drivers.iter().any(|d| d == "tuned");
                if mode.is_none() && uses_tuned {
                    last_tuned = power_profile::tuned_profile();
                }
                mode = Some(m.active);
            }
            Change::Mode(None) => continue,
            Change::Supply | Change::Settled => supply.observe(!battery::on_ac(), Instant::now()),
            Change::Applied(ref name) => {
                if !uses_tuned {
                    continue;
                }
                last_tuned = Some(name.clone());
                mode = power_profile::modes().ok().flatten().map(|m| m.active);
            }
            Change::Device => {
                let now = Instant::now();
                if probe.is_none() {
                    probe_end = Some(now + Duration::from_secs(2));
                }
                probe = None;
                if !state.deferred.is_empty()
                    && let Some(m) = mode
                {
                    let prev = state.deferred.clone();
                    let done = follow_gpus(m, supply.battery(Instant::now()), &mut state, true);
                    if !done.is_empty() || prev != state.deferred {
                        state.failed.extend(failures(&done));
                        let _ = state.save();
                        report(done);
                    }
                    if !state.deferred.is_empty() && probe_end.is_some_and(|end| now < end) {
                        probe = Some(now + Duration::from_millis(200));
                    }
                }
                continue;
            }
            Change::Cooling => {
                if let Some(m) = mode {
                    let top = ceiling(m);
                    if top != state.top {
                        let done = cpu_tuning::Sys::default().limit(top);
                        if let Err(e) = done {
                            eprintln!("thermal power ceiling: {e}");
                        } else {
                            state.top = top;
                            state.cpu_after = cpu_tuning::caps();
                            let _ = state.save();
                        }
                    }
                }
                continue;
            }
            Change::Topology => {
                if let Some(m) = mode {
                    let done = follow_gpus(m, supply.battery(Instant::now()), &mut state, false);
                    state.failed.extend(failures(&done));
                    let _ = state.save();
                    report(done);
                }
                continue;
            }
            Change::Woke => state.gpus.clear(),
        }
        // (The supply's settling is taken up whether or not there is a
        // mode to plan for yet: left for after, a deadline already past
        // stayed past, and the wait above came back at once, over and
        // over, until a mode was known. Found in review, 2026-11-20.)
        let on_battery = supply.battery(Instant::now());
        let Some(m) = mode else { continue };
        // Switching profiles returns before TuneD has finished writing.
        // Its completion signal is the next event for this transition.
        if uses_tuned && power_profile::tuned_profile() != last_tuned {
            continue;
        }
        if supply.unsteady != unsteady {
            unsteady = supply.unsteady;
            if unsteady {
                println!("the power supply keeps changing ({} times in {} s): planned for as the battery until it's steady", Supply::UNSTEADY, Supply::WINDOW.as_secs());
            } else {
                println!("the power supply is steady again");
            }
        }
        // (The mode said again is set again: the daemon set its own first.)
        if last == Some((m, on_battery)) && !matches!(change, Change::Applied(_) | Change::Woke) {
            continue;
        }
        last = Some((m, on_battery));
        println!("the power mode is {}{}", m.label(), if on_battery { ", on battery" } else { "" });
        report(follow_now(m, on_battery, &mut state));
    }
    // (The daemon's watch ended: systemd starts this again.)
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fans follow the plan: the profile's word written where the
    /// fan service watches for it, once; one of Olo's own with the
    /// service not set up is said, not written; where the fans can't be
    /// set at all, that's said. The word there was before (`cool`) is
    /// Cooler already.
    #[test]
    fn the_fans_follow_the_plan() {
        use olo_system::fans::Places;
        let root = std::path::PathBuf::from(format!("/var/tmp/olo-gpu-tune-fans-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let places = Places::under(&root);
        let put = |path: std::path::PathBuf, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        assert!(follow_fans(Profile::Cooler, &places).unwrap_err().contains("can't be set"));
        put(places.fan(), "status:\t\tenabled\n");
        assert!(follow_fans(Profile::Cooler, &places).unwrap_err().contains("isn't set up"));
        assert_eq!(follow_fans(Profile::Auto, &places), Ok("the computer's own".to_owned()));
        assert!(!places.profile().exists());
        put(places.program(), "");
        put(places.unit(), "");
        assert_eq!(follow_fans(Profile::Cooler, &places), Ok("Cooler".to_owned()));
        assert_eq!(std::fs::read_to_string(places.profile()).unwrap(), "cool-2\n");
        assert_eq!(follow_fans(Profile::Cooler, &places), Ok("Cooler already".to_owned()));
        put(places.profile(), "cool\n");
        assert_eq!(follow_fans(Profile::Cooler, &places), Ok("Cooler already".to_owned()));
        assert_eq!(std::fs::read_to_string(places.profile()).unwrap(), "cool\n", "left as it was written");
        assert_eq!(follow_fans(Profile::Coolest, &places), Ok("Coolest".to_owned()));
        assert_eq!(follow_fans(Profile::Auto, &places), Ok("Auto".to_owned()));
        assert_eq!(std::fs::read_to_string(places.profile()).unwrap(), "auto\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_change_of_supply_counts_once_it_has_held() {
        let t = Instant::now();
        let mut s = Supply::new(false);
        assert_eq!((s.battery(t), s.deadline()), (false, None));
        // Unplugged: the battery's plan two seconds on, not before.
        s.observe(true, t);
        assert_eq!((s.battery(t + ms(100)), s.deadline()), (false, Some(t + Supply::SETTLE)));
        assert!(s.battery(t + Supply::SETTLE));
        assert_eq!(s.deadline(), None);
        // A cable moved to another port, out and in within the two seconds: nothing.
        let t = t + Duration::from_secs(100);
        s.observe(false, t);
        assert!(!s.battery(t + Supply::SETTLE));
        let t = t + Duration::from_secs(100);
        s.observe(true, t);
        s.observe(false, t + ms(900));
        assert_eq!((s.battery(t + ms(950)), s.deadline()), (false, None));
        assert!(!s.battery(t + Duration::from_secs(5)) && !s.unsteady);
    }

    /// The P1's storm: on 30 ms, off 13, for minutes, then at rest.
    #[test]
    fn a_supply_that_keeps_changing_is_the_battery_until_it_is_steady() {
        let start = Instant::now();
        let mut s = Supply::new(false);
        let (mut plans, mut last) = (0, false);
        let mut t = start;
        for i in 0..4000u64 {
            t = start + ms(43 * i);
            s.observe(true, t);
            s.observe(false, t + ms(13));
            let now = s.battery(t + ms(13));
            plans += usize::from(now != last);
            last = now;
        }
        assert!(s.unsteady && last, "planned for as the battery");
        assert_eq!(plans, 1, "set once in three minutes of it");
        // At rest on the adapter: the adapter's plan when it has been steady a while.
        let rest = t + ms(13);
        assert_eq!(s.deadline(), Some(rest + Supply::WINDOW));
        assert!(s.battery(rest + Supply::WINDOW - ms(1)));
        assert!(!s.battery(rest + Supply::WINDOW) && !s.unsteady);
        assert_eq!(s.deadline(), None);
    }
}

fn apply(card: Option<&str>) -> i32 {
    let Some(m) = power_profile::modes().ok().flatten() else { return 0 };
    let mut state = gpu_tuning::Followed::default();
    // Device add is also reported to the long-lived follower. Never open a sleeping card.
    if let Some(g) = gpus::list().into_iter().find(|g| g.card.as_ref().and_then(|p| p.file_name()).and_then(|s| s.to_str()) == card) {
        state.deferred.push(g.pci);
    }
    report(follow_gpus(m.active, !battery::on_ac(), &mut state, card.is_some()))
}
