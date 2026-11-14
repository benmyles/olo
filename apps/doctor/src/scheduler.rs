//! The scheduler the computer runs under (Preferences ▸ Performance;
//! Ben, 2026-11-10: "add to olo the ability to pick a custom scheduler
//! in preferences -> performance and the recommendation should be
//! scx_lavd and if the user enables it then olo should handle
//! installing"), as root steps: `scx_lavd` installed as a service of
//! the system's (`olo-scheduler.service`), as the login screen's parts
//! and the fans' are, enabled and started; or that service stopped and
//! disabled, which leaves the kernel's own. `olo-doctor --scheduler
//! [kernel|lavd]` is how Preferences, a person at a terminal and an
//! agent all set it; with no word it says what there is.
//!
//! The program is the `olo-scx-lavd` beside Doctor (scx_lavd as Olo
//! carries it, built by tools/build-scheduler.sh), copied where root
//! owns it (/usr/local/libexec/olo/scx_lavd): a root service must not
//! run a file the person can write. Where there is none beside Doctor,
//! root's copy is used as it is, if it's there.

use std::path::Path;

use olo_system::scheduler::{Choice, Places, look};

use crate::{
    fix::{Step, UnitAction},
    helper,
};

pub const UNIT: &str = include_str!("../../../data/systemd/olo-scheduler.service");
pub const SERVICE: &str = "olo-scheduler.service";
/// Its program, beside ours (scx_lavd under the name Olo's programs
/// are carried by).
pub const PROGRAM: &str = "olo-scx-lavd";

fn same(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The steps that put `choice` in force, from the `olo-scx-lavd` in `bin`:
/// only what isn't so already (every one can be done again). Err: LAVD
/// is asked for and there is no program for it.
///
/// LAVD: its program and unit, where they're missing or not this
/// build's; the service enabled, and started (again, if its program or
/// unit changed). The kernel's own: the service disabled and stopped
/// (LAVD unloads itself as it ends, and the kernel's takes over).
pub fn steps(choice: Choice, bin: &Path, places: &Places) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    let enabled = std::fs::symlink_metadata(places.wanted()).is_ok();
    // (systemd's part: in a test's tree, which has none, the link it
    // would make stands for it.)
    let unit = |steps: &mut Vec<Step>, action: UnitAction| steps.push(Step::Unit { name: SERVICE.into(), action });
    if choice == Choice::Kernel {
        if places.stand_in() {
            if enabled {
                steps.push(Step::RemoveFile { path: places.wanted() });
            }
            return Ok(steps);
        }
        let state = olo_system::systemd::unit_state(SERVICE).ok();
        if enabled || state.as_ref().is_some_and(|s| s.enabled) {
            unit(&mut steps, UnitAction::Disable);
        }
        if state.as_ref().is_some_and(|s| s.active) {
            unit(&mut steps, UnitAction::Stop);
        }
        return Ok(steps);
    }
    let mut changed = false;
    let from = bin.join(PROGRAM);
    if from.is_file() {
        if !same(&from, &places.program()) {
            steps.push(Step::CopyFile { from, to: places.program(), mode: 0o755 });
            if !places.stand_in() {
                steps.push(Step::SelinuxLabels { rules: Vec::new(), restore: vec![places.program()] });
            }
            changed = true;
        }
    } else if !places.program().is_file() {
        return Err("LAVD isn't with this copy of Olo. Built from Olo's source it is: tools/build-scheduler.sh, then an upgrade.".into());
    }
    if std::fs::read_to_string(places.unit()).ok().as_deref() != Some(UNIT) {
        steps.push(Step::WriteFile { path: places.unit(), content: UNIT.to_owned(), mode: 0o644 });
        changed = true;
    }
    if places.stand_in() {
        if !enabled {
            steps.push(Step::WriteFile { path: places.wanted(), content: String::new(), mode: 0o644 });
        }
        return Ok(steps);
    }
    if changed {
        unit(&mut steps, UnitAction::Reload);
    }
    let state = olo_system::systemd::unit_state(SERVICE).ok();
    if !enabled || !state.as_ref().is_some_and(|s| s.enabled) {
        unit(&mut steps, UnitAction::Enable);
    }
    if changed || !state.as_ref().is_some_and(|s| s.active) {
        unit(&mut steps, UnitAction::Restart);
    }
    Ok(steps)
}

/// Put `choice` in force, saying how it goes; what was done, in a line.
/// As root where it needs to be (the helper, through pkexec: the
/// password is asked once), and not at all where nothing is to change.
pub fn apply(choice: Choice, places: &Places, progress: &mut dyn FnMut(String)) -> Result<String, String> {
    if look(places).is_none() {
        return Err("This computer's kernel can't load another scheduler (it has no sched_ext).".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let steps = steps(choice, exe.parent().unwrap_or(Path::new(".")), places)?;
    if !steps.is_empty() {
        // (A test's tree is the person's to write.)
        let results = if places.stand_in() { helper::run_steps_with(&steps, progress) } else { helper::run_as_root(&steps, progress)? };
        if let Some(e) = results.into_iter().find_map(Result::err) {
            return Err(e);
        }
    }
    Ok(match choice {
        Choice::Kernel => "The kernel's own scheduler is chosen.".to_owned(),
        Choice::Lavd => "LAVD is chosen, from now and at every start of the computer.".to_owned(),
    })
}

/// `olo-doctor --scheduler [kernel|lavd]`; its exit status.
pub fn from_the_command_line(args: &[String]) -> i32 {
    let places = Places::here();
    match args.first().map(String::as_str) {
        None => {
            match look(&places) {
                None => println!("This computer's kernel can't load another scheduler (it has no sched_ext)."),
                Some(s) => {
                    println!("chosen: {}", s.chosen.word());
                    println!("service: {}", if s.installed { "installed" } else { "not installed" });
                    println!("loaded: {}", s.loaded.as_deref().unwrap_or("none (the kernel's own)"));
                }
            }
            0
        }
        Some(word) => match Choice::parse(word) {
            None => {
                eprintln!("olo-doctor: --scheduler takes kernel or lavd, not {word}");
                2
            }
            Some(choice) => match apply(choice, &places, &mut |text| println!("  {text}")) {
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

    fn run(steps: &[Step]) {
        for r in helper::run_steps(steps) {
            r.unwrap();
        }
    }

    #[test]
    fn lavd_is_installed_once_and_the_kernels_own_only_unchooses_it() {
        let root = PathBuf::from(format!("/var/tmp/olo-doctor-scheduler-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let places = Places::under(&root.join("root"));
        put(places.sched_ext().join("state"), "disabled\n");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // The kernel's own, with nothing installed: nothing to do.
        assert!(steps(Choice::Kernel, &bin, &places).unwrap().is_empty());
        // No program anywhere: LAVD can't be chosen.
        assert!(steps(Choice::Lavd, &bin, &places).is_err());
        put(bin.join(PROGRAM), "lavd, build one");
        let first = steps(Choice::Lavd, &bin, &places).unwrap();
        assert_eq!(first.len(), 3, "{first:?}");
        run(&first);
        assert_eq!(std::fs::read_to_string(places.program()).unwrap(), "lavd, build one");
        assert_eq!(std::fs::read_to_string(places.unit()).unwrap(), UNIT);
        let s = look(&places).unwrap();
        assert_eq!((s.chosen, s.installed), (Choice::Lavd, true));
        assert!(steps(Choice::Lavd, &bin, &places).unwrap().is_empty(), "again: nothing to do");
        // Back to the kernel's: unchosen, its parts left for the next time.
        let back = steps(Choice::Kernel, &bin, &places).unwrap();
        assert!(matches!(back.as_slice(), [Step::RemoveFile { .. }]), "{back:?}");
        run(&back);
        let s = look(&places).unwrap();
        assert_eq!((s.chosen, s.installed), (Choice::Kernel, true));
        // Chosen again: only that.
        assert_eq!(steps(Choice::Lavd, &bin, &places).unwrap().len(), 1);
        // With no program beside Doctor, root's copy serves.
        std::fs::remove_file(bin.join(PROGRAM)).unwrap();
        assert_eq!(steps(Choice::Lavd, &bin, &places).unwrap().len(), 1);
        // A newer build's replaces it.
        put(bin.join(PROGRAM), "lavd, build two");
        assert!(steps(Choice::Lavd, &bin, &places).unwrap().iter().any(|s| matches!(s, Step::CopyFile { .. })));
        let _ = std::fs::remove_dir_all(root);
    }
}
