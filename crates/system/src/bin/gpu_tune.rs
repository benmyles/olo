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
//!   olo-gpu-tune apply [CARD]    (root) the tuning kept, applied (udev
//!                                runs it as each GPU appears: "card1")
//!   olo-gpu-tune follow          (root, olo-gpu-follow.service) the
//!                                processors and every GPU on the plan's
//!                                presets as the power mode or the power
//!                                supply changes
//!
//! `set` installs a copy of this program where only root can change it
//! (/usr/local/libexec/olo-gpu-tune), the udev rule that runs it at
//! start and the service that follows the power mode: a rule or service
//! running a program the person's account could change would run
//! anything as root. Preferences runs `set` through polkit.

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
                "tuning": tuning.of(&g.pci),
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
    match install() {
        Ok(new) => {
            if let Err(e) = start_following(new) {
                println!("error: following the power mode: {e}");
            }
        }
        Err(e) => println!("error: setting it at every start: {e}"),
    }
    // What's in force now (the mode and the power supply as they are).
    match power_profile::modes() {
        Ok(Some(modes)) => report(follow_now(modes.active, !battery::on_ac())),
        _ => report(gpu_tuning::apply_kept(None, true)),
    }
}

/// The plan's presets for `mode` and the power supply: every GPU's (kept
/// and applied as `set` does; NVIDIA's asked what it allows, waking it a
/// moment, as applying its preset does anyway) and the processors'
/// (and what was done said in `/run/olo-gpu-follow.json`, for Olo Doctor
/// and Preferences).
fn follow_now(mode: power_profile::Mode, on_battery: bool) -> Vec<(String, gpu_tuning::Applied)> {
    let mut out = follow_plan(mode, on_battery);
    let kept = Tuning::load();
    let before = gpu_tuning::Followed::load(std::path::Path::new("/")).unwrap_or_default();
    let (done, drives, limited, sata_found) = follow_drives(&kept.plan, mode, on_battery, &before);
    out.extend(done);
    let followed = gpu_tuning::Followed {
        version: gpu_tuning::FOLLOWER_VERSION,
        mode: mode.id().into(),
        on_battery,
        cpu: kept.plan.cpu_now(mode, on_battery).id().into(),
        top: kept.plan.cpu_top_now(mode, on_battery),
        failed: failures(&out),
        cpu_after: cpu_tuning::caps(),
        drives,
        limited,
        sata_found,
    };
    if let Err(e) = followed.save() {
        eprintln!("olo-gpu-tune: couldn't say what was done: {e}");
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

fn follow_plan(mode: power_profile::Mode, on_battery: bool) -> Vec<(String, gpu_tuning::Applied)> {
    let kept = Tuning::load();
    let gpus: Vec<gpu_tuning::PlanGpu> = gpus::list().iter().filter_map(|g| Some(gpu_tuning::PlanGpu { pci: g.pci.clone(), caps: gpu_tuning::caps(g, true)?, nvidia: g.vendor == 0x10de })).collect();
    if let Some(new) = gpu_tuning::scenario(&kept, &gpus, mode, on_battery) {
        let new = gpu_tuning::with_driver_limits(new, &kept);
        if let Err(e) = keep(&new) {
            return vec![("gpus".into(), vec![Err(format!("{}: {e}", gpu_tuning::FILE))])];
        }
    }
    let mut out = gpu_tuning::apply_kept(None, true);
    let (cpu, top) = (kept.plan.cpu_now(mode, on_battery), kept.plan.cpu_top_now(mode, on_battery));
    out.push((format!("processors ({})", cpu.name()), cpu_tuning::apply(cpu, top, on_battery)));
    out
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

/// The power supply as the follower acts on it: a change counts once it
/// has held for `SETTLE` (a plug put in twice, a cable moved to another
/// port), and a supply that keeps changing (`UNSTEADY` times within
/// `WINDOW`) is taken as the battery, the careful plan, set once, until it
/// has been steady for `WINDOW`. A ThinkPad P1's controller reported its
/// adapter coming and going 23 times a second for two hours, charger in
/// or out (2026-11-17): every change UPower told of had the whole plan
/// set again, 610 times in the first hour, each waking NVIDIA's GPU and
/// calling the firmware for its profile.
struct Supply {
    /// What the plan in force was set for.
    battery: bool,
    /// The last reading.
    seen: bool,
    /// When the reading changed, lately.
    changes: std::collections::VecDeque<Instant>,
    unsteady: bool,
}

impl Supply {
    const SETTLE: Duration = Duration::from_secs(2);
    const WINDOW: Duration = Duration::from_secs(30);
    const UNSTEADY: usize = 4;

    fn new(battery: bool) -> Self {
        Supply { battery, seen: battery, changes: Default::default(), unsteady: false }
    }

    /// A reading of the supply.
    fn observe(&mut self, battery: bool, now: Instant) {
        if battery != self.seen {
            self.seen = battery;
            self.changes.push_back(now);
        }
        while self.changes.len() > Self::UNSTEADY {
            self.changes.pop_front();
        }
        if self.changes.len() == Self::UNSTEADY && self.changes.front().is_some_and(|t| now.duration_since(*t) <= Self::WINDOW) {
            self.unsteady = true;
        }
    }

    /// The supply to plan for at `now`.
    fn battery(&mut self, now: Instant) -> bool {
        let since = self.changes.back().map(|t| now.saturating_duration_since(*t));
        if self.unsteady {
            if since.is_some_and(|d| d < Self::WINDOW) {
                self.battery = true;
                return true;
            }
            self.unsteady = false;
            self.changes.clear();
        }
        if self.seen != self.battery && since.is_none_or(|d| d >= Self::SETTLE) {
            self.battery = self.seen;
        }
        self.battery
    }

    /// When to ask again without a reading: a change settling, or an
    /// unsteady supply come to rest.
    fn deadline(&self) -> Option<Instant> {
        let last = *self.changes.back()?;
        if self.unsteady {
            Some(last + Self::WINDOW)
        } else if self.seen != self.battery {
            Some(last + Self::SETTLE)
        } else {
            None
        }
    }
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
        Mode(Option<power_profile::Mode>),
        Supply,
        Settled,
        /// Back from sleep: a drive's controller reset on the way puts it
        /// back on its highest state.
        Woke,
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let modes = tx.clone();
    power_profile::watch(move |m| {
        let _ = modes.send(Change::Mode(m.map(|m| m.active)));
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
    let mut mode = None;
    let mut supply = Supply::new(!battery::on_ac());
    let mut last = None;
    let mut unsteady = false;
    loop {
        let change = match supply.deadline() {
            Some(end) => match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(change) => change,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Change::Settled,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(change) => change,
                Err(_) => break,
            },
        };
        match change {
            Change::Mode(Some(m)) => mode = Some(m),
            Change::Mode(None) => continue,
            Change::Supply | Change::Settled => supply.observe(!battery::on_ac(), Instant::now()),
            Change::Woke => {
                // Only the drives: the rest keeps across a sleep (and
                // NVIDIA's GPU isn't woken for it).
                if let Some(m) = mode {
                    let mut followed = gpu_tuning::Followed::load(std::path::Path::new("/")).unwrap_or_default();
                    let (done, drives, limited, sata_found) = follow_drives(&Tuning::load().plan, m, supply.battery(Instant::now()), &followed);
                    if !done.is_empty() {
                        println!("woke: the drives set again");
                        followed.failed.retain(|f| !f.contains("(nvme") && !f.contains("(sd"));
                        followed.failed.extend(failures(&done));
                        (followed.drives, followed.limited, followed.sata_found) = (drives, limited, sata_found);
                        let _ = followed.save();
                        report(done);
                    }
                }
                continue;
            }
        }
        let Some(m) = mode else { continue };
        let on_battery = supply.battery(Instant::now());
        if supply.unsteady != unsteady {
            unsteady = supply.unsteady;
            if unsteady {
                println!("the power supply keeps changing ({} times in {} s): planned for as the battery until it's steady", Supply::UNSTEADY, Supply::WINDOW.as_secs());
            } else {
                println!("the power supply is steady again");
            }
        }
        // (The mode said again is set again: the daemon set its own first.)
        if last == Some((m, on_battery)) && !matches!(change, Change::Mode(_)) {
            continue;
        }
        last = Some((m, on_battery));
        println!("the power mode is {}{}", m.label(), if on_battery { ", on battery" } else { "" });
        report(follow_now(m, on_battery));
    }
    // (The daemon's watch ended: systemd starts this again.)
    1
}

#[cfg(test)]
mod tests {
    use super::*;

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
    report(gpu_tuning::apply_kept(card, false))
}
