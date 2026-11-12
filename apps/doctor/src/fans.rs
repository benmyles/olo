//! The fan profile (Preferences ▸ Power; Ben, 2026-11-10: "on auto
//! settings the thinkpad is too slow to ramp up the fans, it gets behind
//! the thermal curve and then starts throttling cpu"), as root steps:
//! `olo-fans` (apps/fans) installed as a service of the system's, as the
//! login screen's parts are, and `/etc/olo/fans` saying which curve:
//! `auto`, the firmware's own (the service sleeps), or `cool`, the
//! service's. `olo-doctor --fans [auto|cool]` is how Preferences, a
//! person at a terminal and an agent all set it; with no word it says
//! what there is.
//!
//! Its unit is this build's own, carried in the program. The service's
//! program is a copy root owns (/usr/local/libexec/olo, as the login
//! screen's), so an upgrade replaces the person's and not it: choosing
//! Cool again brings it up to date.

use std::path::Path;

use olo_system::fans::{Places, Profile, look};

use crate::{
    fix::{Step, UnitAction},
    helper,
};

pub const UNIT: &str = include_str!("../../../data/systemd/olo-fans.service");
pub const SERVICE: &str = "olo-fans.service";
/// Its program, beside ours.
pub const PROGRAM: &str = "olo-fans";

/// Whether two files hold the same bytes (neither missing).
fn same(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The steps that put `profile` in force, from the `olo-fans` in `bin`:
/// only what isn't so already (every one can be done again).
///
/// Cool: the service's program and unit, where they're missing or not
/// this build's; thinkpad_acpi told to take levels from the next start
/// of the computer, where it doesn't; the word; the service enabled, and
/// started again if its program or unit changed (running already, it
/// reads the word by itself).
/// Auto: the word alone, and only where the service is installed (the
/// firmware's curve is what a computer without it has).
pub fn steps(profile: Profile, bin: &Path, places: &Places) -> Vec<Step> {
    let mut steps = Vec::new();
    let word = |steps: &mut Vec<Step>| {
        if std::fs::read_to_string(places.profile()).ok().and_then(|w| Profile::parse(&w)) != Some(profile) {
            steps.push(Step::WriteFile { path: places.profile(), content: format!("{}\n", profile.word()), mode: 0o644 });
        }
    };
    let installed = places.program().is_file() && places.unit().is_file();
    if profile == Profile::Auto {
        if installed {
            word(&mut steps);
        }
        return steps;
    }
    let mut changed = false;
    let from = bin.join(PROGRAM);
    if !same(&from, &places.program()) {
        steps.push(Step::CopyFile { from, to: places.program(), mode: 0o755 });
        if !places.stand_in() {
            // (Its label as the folder's own says: a program systemd may run.)
            steps.push(Step::SelinuxLabels { rules: Vec::new(), restore: vec![places.program()] });
        }
        changed = true;
    }
    if std::fs::read_to_string(places.unit()).ok().as_deref() != Some(UNIT) {
        steps.push(Step::WriteFile { path: places.unit(), content: UNIT.to_owned(), mode: 0o644 });
        changed = true;
    }
    if !std::fs::read_to_string(places.fan_control()).is_ok_and(|v| matches!(v.trim(), "Y" | "1")) {
        let conf = "# Olo's fan profile (Preferences > Power): the fans' level may be set.\noptions thinkpad_acpi fan_control=1\n";
        if std::fs::read_to_string(places.modprobe()).ok().as_deref() != Some(conf) {
            steps.push(Step::WriteFile { path: places.modprobe(), content: conf.to_owned(), mode: 0o644 });
        }
    }
    word(&mut steps);
    // (systemd's part: not in a test's tree, which has none.)
    if !places.stand_in() {
        if changed {
            steps.push(Step::Unit { name: SERVICE.into(), action: UnitAction::Reload });
        }
        let state = olo_system::systemd::unit_state(SERVICE).ok();
        if changed || !state.as_ref().is_some_and(|s| s.enabled) {
            steps.push(Step::Unit { name: SERVICE.into(), action: UnitAction::Enable });
        }
        if changed || !state.as_ref().is_some_and(|s| s.active) {
            steps.push(Step::Unit { name: SERVICE.into(), action: UnitAction::Restart });
        }
    }
    steps
}

/// Put `profile` in force, saying how it goes; what was done, in a
/// line. As root where it needs to be (the helper, through pkexec: the
/// password is asked once), and not at all where nothing is to change.
pub fn apply(profile: Profile, places: &Places, progress: &mut dyn FnMut(String)) -> Result<String, String> {
    if look(places).is_none() {
        return Err("This computer's fans can't be set (a ThinkPad's can, through thinkpad_acpi).".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let bin = exe.parent().unwrap_or(Path::new("."));
    if profile == Profile::Cool && !bin.join(PROGRAM).is_file() {
        return Err(format!("{} isn't beside {}: Olo isn't whole here.", PROGRAM, exe.display()));
    }
    let steps = steps(profile, bin, places);
    if !steps.is_empty() {
        // (A test's tree is the person's to write.)
        let results = if places.stand_in() { helper::run_steps_with(&steps, progress) } else { helper::run_as_root(&steps, progress)? };
        if let Some(e) = results.into_iter().find_map(Result::err) {
            return Err(e);
        }
    }
    let waits = profile == Profile::Cool && look(places).is_some_and(|s| !s.control);
    Ok(match profile {
        Profile::Auto => "The fans are left to the computer.".to_owned(),
        Profile::Cool if waits => "Cool is chosen: it starts with the computer's next start (the fans' driver takes levels from then).".to_owned(),
        Profile::Cool => "The fans run ahead of the heat.".to_owned(),
    })
}

/// `olo-doctor --fans [auto|cool]`; its exit status.
pub fn from_the_command_line(args: &[String]) -> i32 {
    let places = Places::here();
    match args.first().map(String::as_str) {
        None => {
            match look(&places) {
                None => println!("This computer's fans can't be set (a ThinkPad's can, through thinkpad_acpi)."),
                Some(s) => {
                    println!("profile: {}", s.profile.word());
                    println!("service: {}", if s.installed { "installed" } else { "not installed" });
                    if let Some((level, why)) = &s.held {
                        println!("level: {level}{}", if why.is_empty() { String::new() } else { format!(" ({why})") });
                    }
                    if !s.control {
                        println!("thinkpad_acpi doesn't take levels (fan_control): a profile waits for the next start of the computer.");
                    }
                }
            }
            0
        }
        Some(word) => match Profile::parse(word) {
            None => {
                eprintln!("olo-doctor: --fans takes auto or cool, not {word}");
                2
            }
            Some(profile) => match apply(profile, &places, &mut |text| println!("  {text}")) {
                Ok(said) => {
                    println!("{said}");
                    0
                }
                Err(e) => {
                    eprintln!("olo-doctor: {e}");
                    1
                }
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn put(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A made-up ThinkPad under a scratch folder, and a bin with a
    /// program to install.
    fn thinkpad(name: &str, control: &str) -> (PathBuf, Places, PathBuf) {
        let root = PathBuf::from(format!("/var/tmp/olo-doctor-fans-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let places = Places::under(&root.join("root"));
        put(places.fan(), "status:\t\tenabled\nspeed:\t\t2880\nlevel:\t\tauto\n");
        put(places.fan_control(), control);
        let bin = root.join("bin");
        put(bin.join(PROGRAM), "the program, build one");
        (root, places, bin)
    }

    fn run(steps: &[Step]) {
        for r in helper::run_steps(steps) {
            r.unwrap();
        }
    }

    #[test]
    fn cool_installs_the_service_once_and_auto_only_says_so() {
        let (root, places, bin) = thinkpad("cool", "Y\n");
        // Nothing installed: the firmware's curve is what there is.
        assert!(steps(Profile::Auto, &bin, &places).is_empty());
        let first = steps(Profile::Cool, &bin, &places);
        assert_eq!(first.len(), 3, "{first:?}");
        run(&first);
        assert_eq!(std::fs::read_to_string(places.program()).unwrap(), "the program, build one");
        assert_eq!(std::fs::read_to_string(places.unit()).unwrap(), UNIT);
        assert_eq!(std::fs::read_to_string(places.profile()).unwrap(), "cool\n");
        assert!(!places.modprobe().exists(), "the driver takes levels already");
        let s = look(&places).unwrap();
        assert_eq!((s.profile, s.installed), (Profile::Cool, true));
        // Again: nothing to do.
        assert!(steps(Profile::Cool, &bin, &places).is_empty());
        // Auto: the word alone; and back.
        let auto = steps(Profile::Auto, &bin, &places);
        assert!(matches!(auto.as_slice(), [Step::WriteFile { content, .. }] if content == "auto\n"), "{auto:?}");
        run(&auto);
        assert_eq!(look(&places).unwrap().profile, Profile::Auto);
        let back = steps(Profile::Cool, &bin, &places);
        assert!(matches!(back.as_slice(), [Step::WriteFile { content, .. }] if content == "cool\n"), "{back:?}");
        // A newer build's program replaces root's copy.
        put(bin.join(PROGRAM), "the program, build two");
        let newer = steps(Profile::Cool, &bin, &places);
        assert!(newer.iter().any(|s| matches!(s, Step::CopyFile { .. })), "{newer:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_driver_that_takes_no_levels_is_told_to_from_the_next_start() {
        let (root, places, bin) = thinkpad("control", "N\n");
        let first = steps(Profile::Cool, &bin, &places);
        assert!(first.iter().any(|s| matches!(s, Step::WriteFile { path, .. } if *path == places.modprobe())), "{first:?}");
        run(&first);
        assert!(std::fs::read_to_string(places.modprobe()).unwrap().contains("options thinkpad_acpi fan_control=1"));
        assert!(steps(Profile::Cool, &bin, &places).is_empty());
        assert_eq!(look(&places).unwrap().now(), "Starts with the computer's next start.");
        let _ = std::fs::remove_dir_all(root);
    }
}
