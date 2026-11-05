//! olo-gpu-tune: each GPU's tuning (Preferences ▸ GPUs;
//! `olo_system::gpu_tuning`).
//!
//!   olo-gpu-tune show [--wake]   every GPU, what it allows and its
//!                                tuning, as JSON (NVIDIA's read only
//!                                while it's awake, or with --wake)
//!   olo-gpu-tune set             (root) the tuning on stdin, as JSON
//!                                (`Tuning`): kept in /etc/olo/gpus.json,
//!                                applied, and set again at every start
//!   olo-gpu-tune apply [CARD]    (root) the tuning kept, applied (udev
//!                                runs it as each GPU appears: "card1")
//!   olo-gpu-tune follow          (root, olo-gpu-follow.service) every
//!                                GPU on the power mode's preset as the
//!                                mode changes
//!
//! `set` installs a copy of this program where only root can change it
//! (/usr/local/libexec/olo-gpu-tune), the udev rule that runs it at
//! start and the service that follows the power mode: a rule or service
//! running a program the person's account could change would run
//! anything as root. Preferences runs `set` through polkit.

use std::io::{Read, Write};

use olo_system::{
    gpu_tuning::{self, Tuning},
    gpus, power_profile, systemd,
};

const INSTALLED: &str = "/usr/local/libexec/olo-gpu-tune";
const RULE_FILE: &str = "/etc/udev/rules.d/73-olo-gpu-tune.rules";
const RULE: &str =
    "# Olo: each GPU's tuning (Preferences ▸ GPUs), set again as it appears.\nACTION==\"add\", SUBSYSTEM==\"drm\", KERNEL==\"card[0-9]*\", RUN+=\"/usr/local/libexec/olo-gpu-tune apply %k\"\n";
const SERVICE_NAME: &str = "olo-gpu-follow.service";
const SERVICE_FILE: &str = "/etc/systemd/system/olo-gpu-follow.service";
const SERVICE: &str = "# Olo: each GPU on the power mode's preset as the mode changes (Preferences ▸ GPUs).
[Unit]
Description=Olo: the GPUs follow the power mode
After=dbus.service power-profiles-daemon.service tuned-ppd.service

[Service]
ExecStart=/usr/local/libexec/olo-gpu-tune follow
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
";

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
            serde_json::json!({
                "pci": g.pci,
                "name": g.name,
                "driver": g.driver,
                "asleep": gpu_tuning::asleep(&g),
                "caps": gpu_tuning::caps(&g, wake),
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
    let tuning = Tuning { gpus: tuning.gpus.into_iter().filter(|g| here.contains(&g.pci)).collect(), driver: Vec::new() };
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
    report(gpu_tuning::apply_kept(None, true))
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
/// daemon, every GPU (integrated and discrete) on the mode's preset now
/// and as the mode changes, wherever it's changed (Preferences ▸ Power,
/// Deskbar's battery menu, a ThinkPad's Fn keys, `powerprofilesctl`),
/// kept and applied as `set` does; the daemon sets the processors and
/// the firmware. Woken only by the daemon's signals.
fn follow() -> i32 {
    if let Err(e) = power_profile::wait_for_daemon() {
        eprintln!("olo-gpu-tune: no power profiles daemon: {e}");
        return 1;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    power_profile::watch(move |modes| {
        let _ = tx.send(modes.map(|m| m.active));
    });
    let mut last = None;
    for mode in rx.iter().flatten() {
        if last == Some(mode) {
            continue;
        }
        last = Some(mode);
        let kept = Tuning::load();
        // (What each allows: NVIDIA's asked, waking it a moment, as
        // applying its preset does anyway.)
        let gpus: Vec<(String, gpu_tuning::Caps)> = gpus::list().iter().filter_map(|g| Some((g.pci.clone(), gpu_tuning::caps(g, true)?))).collect();
        let Some(new) = gpu_tuning::following(&kept, &gpus, gpu_tuning::for_mode(mode)) else { continue };
        let new = gpu_tuning::with_driver_limits(new, &kept);
        if let Err(e) = keep(&new) {
            eprintln!("olo-gpu-tune: {}: {e}", gpu_tuning::FILE);
            continue;
        }
        println!("the power mode is {}: the GPUs on {}", mode.label(), gpu_tuning::for_mode(mode).name());
        report(gpu_tuning::apply_kept(None, true));
    }
    // (The daemon's watch ended: systemd starts this again.)
    1
}

fn apply(card: Option<&str>) -> i32 {
    report(gpu_tuning::apply_kept(card, false))
}
