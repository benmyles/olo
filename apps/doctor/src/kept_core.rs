//! The system's part of the kept core (`olo_system::cores`; Preferences ▸
//! Performance ▸ Scheduler): the system's services, root's, kept off the
//! core app_server's loop and input thread have to themselves, as the
//! person's own programs are (app_server does theirs, with no
//! privilege). systemd's `CPUAffinity=` in a drop-in of its own
//! (`cores::SYSTEM_CONF`), which what it starts inherits from the next
//! start of the computer; what runs now, swept as root
//! ([`Step::KeepOffCores`]). Or both undone. `olo-doctor --keep-core
//! [on|off]` is how Preferences, a person at a terminal and an agent all
//! set it; with no word it says what there is.

use std::path::{Path, PathBuf};

use olo_system::cores;

use crate::{fix::Step, helper};

/// The drop-in under `fs` (the file system's root, or a test's tree).
pub fn conf_path(fs: &Path) -> PathBuf {
    fs.join(cores::SYSTEM_CONF.trim_start_matches('/'))
}

/// The steps that keep the system's services off `kept` (`on`), or let
/// them back, from `fs`; only what isn't so already.
pub fn steps(on: bool, kept: &[usize], online: &[usize], fs: &Path) -> Vec<Step> {
    let path = conf_path(fs);
    let now = std::fs::read_to_string(&path).ok();
    let mut steps = Vec::new();
    if on {
        let want = cores::system_conf(online, kept);
        if now.as_deref() != Some(want.as_str()) {
            steps.push(Step::WriteFile { path: PathBuf::from(cores::SYSTEM_CONF), content: want, mode: 0o644 });
            steps.push(Step::KeepOffCores { kept: kept.to_vec(), on: true });
        }
    } else if now.is_some() {
        steps.push(Step::RemoveFile { path: PathBuf::from(cores::SYSTEM_CONF) });
        steps.push(Step::KeepOffCores { kept: kept.to_vec(), on: false });
    }
    steps
}

/// Every process's threads (the kernel's own and realtime ones aside)
/// kept off `kept`, or let back: the root step.
pub fn sweep_all(kept: &[usize], on: bool) -> cores::Swept {
    let pids: Vec<i32> = std::fs::read_dir("/proc").map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default();
    let (online, plain) = (cores::online(), cores::plain());
    let how = if on { cores::Sweep::KeepOff { kept, plain: &plain, online: &online } } else { cores::Sweep::GiveBack { kept, plain: &plain, online: &online } };
    cores::sweep(&pids, how, &[])
}

/// `olo-doctor --keep-core [on|off]`; its exit status.
pub fn from_the_command_line(args: &[String]) -> i32 {
    let Some(kept) = cores::kept() else {
        println!("This computer has fewer than {} processors online: none is kept.", cores::FEWEST);
        return 0;
    };
    let list = cores::format_list(&kept);
    let on = match args.first().map(String::as_str) {
        None => {
            println!("kept: processor {list}");
            match cores::system_kept() {
                Some(k) => println!("the system's services: kept off processor {}", cores::format_list(&k)),
                None => println!("the system's services: may run anywhere"),
            }
            return 0;
        }
        Some("on") => true,
        Some("off") => false,
        Some(word) => {
            eprintln!("olo-doctor: --keep-core takes on or off, not {word}");
            return 2;
        }
    };
    let steps = steps(on, &kept, &cores::online(), Path::new("/"));
    if !steps.is_empty() {
        match helper::run_as_root(&steps, &mut |text| println!("  {text}")) {
            Err(e) => {
                eprintln!("olo-doctor: {e}");
                return 1;
            }
            Ok(results) => {
                if let Some(e) = results.into_iter().find_map(Result::err) {
                    eprintln!("olo-doctor: {e}");
                    return 1;
                }
            }
        }
    }
    if on {
        println!("The system's services are kept off processor {list}, now and at every start.");
    } else {
        println!("The system's services may run on processor {list} again.");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drop_in_is_written_once_and_removed_to_undo() {
        let fs = PathBuf::from(format!("/var/tmp/olo-doctor-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fs);
        let online: Vec<usize> = (0..16).collect();
        let first = steps(true, &[2], &online, &fs);
        assert!(matches!(&first[..], [Step::WriteFile { .. }, Step::KeepOffCores { on: true, .. }]), "{first:?}");
        assert!(steps(false, &[2], &online, &fs).is_empty(), "nothing to undo");
        // Written (in the test's tree): nothing more to do; undone, both.
        let path = conf_path(&fs);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, cores::system_conf(&online, &[2])).unwrap();
        assert!(steps(true, &[2], &online, &fs).is_empty());
        let undo = steps(false, &[2], &online, &fs);
        assert!(matches!(&undo[..], [Step::RemoveFile { .. }, Step::KeepOffCores { on: false, .. }]), "{undo:?}");
        // Another core kept (a new computer's): written again.
        assert_eq!(steps(true, &[3], &online, &fs).len(), 2);
        let _ = std::fs::remove_dir_all(&fs);
    }
}
