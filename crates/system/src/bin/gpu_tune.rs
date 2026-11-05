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
//!
//! `set` installs a copy of this program where only root can change it
//! (/usr/local/libexec/olo-gpu-tune) and the udev rule that runs it at
//! start: a rule running a program the person's account could change
//! would run anything as root. Preferences runs `set` through polkit.

use std::io::{Read, Write};

use olo_system::{
    gpu_tuning::{self, Tuning},
    gpus,
};

const INSTALLED: &str = "/usr/local/libexec/olo-gpu-tune";
const RULE_FILE: &str = "/etc/udev/rules.d/73-olo-gpu-tune.rules";
const RULE: &str =
    "# Olo: each GPU's tuning (Preferences ▸ GPUs), set again as it appears.\nACTION==\"add\", SUBSYSTEM==\"drm\", KERNEL==\"card[0-9]*\", RUN+=\"/usr/local/libexec/olo-gpu-tune apply %k\"\n";

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("show") => show(args.iter().any(|a| a == "--wake")),
        Some("set") => set(),
        Some("apply") => apply(args.get(1).map(String::as_str)),
        _ => {
            eprintln!("usage: olo-gpu-tune show [--wake] | set (root; JSON on stdin) | apply [CARD] (root)");
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
    let file = std::path::Path::new(gpu_tuning::FILE);
    let kept = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(file.with_extension("json.new"), serde_json::to_vec_pretty(&tuning).unwrap_or_default()))
        .and_then(|()| std::fs::rename(file.with_extension("json.new"), file));
    if let Err(e) = kept {
        println!("error: {}: {e}", file.display());
        return 1;
    }
    if let Err(e) = install() {
        println!("error: setting it at every start: {e}");
    }
    report(gpu_tuning::apply_kept(None, true))
}

/// This program where only root can change it, and the rule that runs
/// it at start.
fn install() -> std::io::Result<()> {
    let me = std::fs::read("/proc/self/exe")?;
    if std::fs::read(INSTALLED).ok().as_deref() != Some(&me[..]) {
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
    Ok(())
}

fn apply(card: Option<&str>) -> i32 {
    report(gpu_tuning::apply_kept(card, false))
}
