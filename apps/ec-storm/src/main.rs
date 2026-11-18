//! olo-ec-storm: a flood of one embedded-controller event doesn't hold
//! up the others.
//!
//! A laptop's embedded controller tells the kernel of each thing that
//! happens to it, and the kernel queues the firmware's handler for each,
//! without limit. A controller that reports the same thing over and over
//! (a ThinkPad P1 Gen 9's: its charger coming and going 23 times a
//! second, 2026-11-17) fills that queue faster than its slow handlers
//! run, and the brightness keys' and the USB-C ports' wait behind:
//! Ben's keys were dead for two hours, 195,778 handlers in line
//! (docs/AGENT-NOTES.md, "The embedded controller's event storm"; "add
//! something to olo's supervisor to do this if an unsustainable backlog
//! builds up"). Olo's module (kernel/ec-storm, `olo_ec_storm`) puts a
//! handler of its own in front of each of the firmware's, by the
//! driver's own way for a module to: of one event arriving over and
//! over, one runs its method and the rest are counted for it to run
//! again for, a few at most. The queue stays short whatever the
//! controller does.
//!
//! A kernel module is built for one kernel. This program, as root at
//! every start (systemd's `olo-ec-storm.service`, installed by Olo
//! Doctor), builds the module for the running kernel where it hasn't yet
//! (`olo_kmod`, shared with Olo's other kernel modules) and loads it.
//! Only on a kernel series whose EC driver was read and found to take
//! such handlers as the module needs (kernel/ec-storm/CHECKED): the two
//! functions it calls are declared in the driver's own header, so
//! nothing at build time says if they changed. Where it can't (no
//! embedded controller, a series not checked, no headers, Secure Boot
//! refusing an unsigned module) it says why and ends failed, which Olo
//! Doctor shows; the kernel is then as it comes.
//!
//! `olo-ec-storm --status` says what there is, as anyone, in a line of
//! JSON (the module's own counts among it); `--remove` (root) takes the
//! module out again.

use olo_kmod::{Kernel, Module, Places, Running, read, release};
use std::process::ExitCode;

/// The module's name, and its source, carried in this program.
const NAME: &str = "olo_ec_storm";
const SOURCE: &str = include_str!("../../../kernel/ec-storm/olo_ec_storm.c");
const COUNT: &str = include_str!("../../../kernel/ec-storm/olo_ec_storm_count.h");
const KBUILD: &str = include_str!("../../../kernel/ec-storm/Kbuild");
const MODULE: Module = Module { name: NAME, state: "olo-ec-storm", files: &[("Kbuild", KBUILD), ("olo_ec_storm.c", SOURCE), ("olo_ec_storm_count.h", COUNT)] };
/// The kernel series its way in was checked on.
const CHECKED: &str = include_str!("../../../kernel/ec-storm/CHECKED");

/// A release's series: "7.2" of "7.2.15-300.fc45.x86_64".
fn series(release: &str) -> String {
    release.split('.').take(2).collect::<Vec<_>>().join(".")
}

fn checked(release: &str) -> bool {
    let series = series(release);
    CHECKED.lines().map(str::trim).any(|l| !l.starts_with('#') && l == series)
}

/// This computer has an embedded controller (ACPI's PNP0C09).
fn controller(places: &Places) -> bool {
    std::fs::read_dir(places.root.join("sys/bus/acpi/devices")).is_ok_and(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("PNP0C09:")))
}

/// The module is in the running kernel.
fn loaded(places: &Places) -> bool {
    places.root.join("sys/module").join(NAME).is_dir()
}

/// What the module says of itself ("guarding 64 ran 812 counted 0").
fn stats(places: &Places) -> Option<String> {
    read(&places.root.join("sys/module").join(NAME).join("parameters/stats")).map(|s| s.trim().to_owned())
}

/// What making sure of the guard came to.
#[derive(Debug, PartialEq)]
enum Done {
    Already,
    Loaded,
    BuiltAndLoaded,
}

/// The guard in the kernel `release`: built where it isn't (or was from
/// another source), and loaded.
fn ensure(places: &Places, release: &str, kernel: &mut dyn Kernel) -> Result<Done, String> {
    if loaded(places) {
        return Ok(Done::Already);
    }
    if !controller(places) {
        return Err("this computer has no embedded controller to guard".into());
    }
    if !checked(release) {
        return Err(format!("Olo hasn't checked the embedded controller's driver in this kernel's series ({}): the guard isn't loaded on it", series(release)));
    }
    let had = olo_kmod::built(places, &MODULE, release);
    if !had {
        olo_kmod::build(places, &MODULE, release, kernel)?;
    }
    if let Err(e) = kernel.load(&places.module(&MODULE, release)) {
        // One built for a kernel of the same name that has since changed
        // (the package rebuilt): once more, from scratch.
        if !had {
            return Err(e);
        }
        olo_kmod::build(places, &MODULE, release, kernel)?;
        kernel.load(&places.module(&MODULE, release))?;
    }
    if !loaded(places) {
        return Err("the kernel took the module but doesn't list it".into());
    }
    olo_kmod::tidy(places, &MODULE, release);
    Ok(if had { Done::Loaded } else { Done::BuiltAndLoaded })
}

/// How it went, written where Olo Doctor reads it: "guarding", or
/// "failed: " and why.
fn say(places: &Places, went: &Result<Done, String>) {
    olo_kmod::say(
        places,
        &MODULE,
        &match went {
            Ok(_) => "guarding".to_owned(),
            Err(e) => format!("failed: {e}"),
        },
    );
}

/// What there is, in a line of JSON.
fn status(places: &Places, release: &str) -> String {
    let stats = stats(places).map_or("null".to_owned(), |s| format!("\"{}\"", s.replace(['"', '\\'], "")));
    format!(
        "{{\"kernel\":\"{release}\",\"loaded\":{},\"built\":{},\"headers\":{},\"controller\":{},\"checked\":{},\"stats\":{stats}}}",
        loaded(places),
        olo_kmod::built(places, &MODULE, release),
        places.headers(release).join("Makefile").is_file(),
        controller(places),
        checked(release),
    )
}

/// The guard taken out of the running kernel: every event is the
/// firmware's handlers' alone again.
fn remove(places: &Places, kernel: &mut dyn Kernel) -> Result<(), String> {
    if !loaded(places) {
        return Ok(());
    }
    match kernel.unload(NAME) {
        Ok(true) => Ok(()),
        Ok(false) => Err("the module is in use still".into()),
        Err(e) => Err(e),
    }
}

fn main() -> ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (places, release) = (Places::here(), release());
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {
            let went = ensure(&places, &release, &mut Running);
            say(&places, &went);
            match went {
                Ok(done) => {
                    let how = match done {
                        Done::Already => "the guard was in already".to_owned(),
                        Done::Loaded => format!("the guard loaded for {release}"),
                        Done::BuiltAndLoaded => format!("the guard built for {release} and loaded"),
                    };
                    println!("olo-ec-storm: a flood of one of the embedded controller's events won't hold up the others ({how}; {})", stats(&places).unwrap_or_default());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("olo-ec-storm: the embedded controller's events aren't guarded: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        ["--status"] => {
            println!("{}", status(&places, &release));
            ExitCode::SUCCESS
        }
        ["--remove"] => match remove(&places, &mut Running) {
            Ok(()) => {
                println!("olo-ec-storm: the guard is out of the running kernel");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-ec-storm: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            println!(
                "usage: olo-ec-storm [--status | --remove]\n  (none)    build Olo's guard for the running kernel where it isn't yet, and load it (root; systemd's olo-ec-storm.service)\n  --status  what there is, in a line of JSON\n  --remove  take the guard out of the running kernel (root)"
            );
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A computer in a folder, removed when the test is over.
    struct Tree(Places);

    impl Tree {
        fn new(name: &str, controller: bool) -> Tree {
            let root = PathBuf::from(format!("/var/tmp/olo-ec-storm-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("sys/module")).unwrap();
            std::fs::create_dir_all(root.join("sys/bus/acpi/devices/LNXSYSTM:00")).unwrap();
            if controller {
                std::fs::create_dir_all(root.join("sys/bus/acpi/devices/PNP0C09:00")).unwrap();
            }
            Tree(Places { root })
        }

        /// A kernel installed, with its headers.
        fn headers(&self, release: &str) {
            let dir = self.0.headers(release);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("Makefile"), "").unwrap();
            std::fs::write(self.0.kernels().join(release).join("vmlinuz"), "").unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.root);
        }
    }

    /// A kernel that builds a module by writing its file, and takes one
    /// by listing it as its modules are.
    struct Fake {
        places: Places,
        built: usize,
        loaded: usize,
        refuses: usize,
    }

    impl Kernel for Fake {
        fn build(&mut self, headers: &Path, dir: &Path) -> Result<(), String> {
            assert!(headers.join("Makefile").is_file());
            assert_eq!(read(&dir.join("Kbuild")).as_deref(), Some(KBUILD));
            assert_eq!(read(&dir.join(format!("{NAME}.c"))).as_deref(), Some(SOURCE));
            assert_eq!(read(&dir.join("olo_ec_storm_count.h")).as_deref(), Some(COUNT), "the header the module includes goes with it");
            self.built += 1;
            std::fs::write(dir.join(format!("{NAME}.ko")), "built").map_err(|e| e.to_string())
        }

        fn load(&mut self, module: &Path) -> Result<(), String> {
            assert!(module.is_file());
            if self.refuses > 0 {
                self.refuses -= 1;
                return Err("refused".into());
            }
            self.loaded += 1;
            let dir = self.places.root.join("sys/module").join(NAME).join("parameters");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("stats"), "guarding 64 ran 0 counted 0\n").unwrap();
            Ok(())
        }

        fn unload(&mut self, name: &str) -> Result<bool, String> {
            assert_eq!(name, NAME);
            std::fs::remove_dir_all(self.places.root.join("sys/module").join(NAME)).unwrap();
            Ok(true)
        }
    }

    const K: &str = "7.2.15-300.fc45.x86_64";

    /// A kernel's first start with Olo: the guard built for it, kept and
    /// loaded; at its next, only loaded; in already, nothing done; taken
    /// out when asked.
    #[test]
    fn built_once_a_kernel_and_loaded_at_each_start() {
        let tree = Tree::new("built-once", true);
        tree.headers(K);
        let mut kernel = Fake { places: tree.0.clone(), built: 0, loaded: 0, refuses: 0 };
        assert!(!loaded(&tree.0));
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::BuiltAndLoaded));
        assert!(loaded(&tree.0));
        assert_eq!((kernel.built, kernel.loaded), (1, 1));
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::Already));
        assert_eq!(
            status(&tree.0, K),
            format!("{{\"kernel\":\"{K}\",\"loaded\":true,\"built\":true,\"headers\":true,\"controller\":true,\"checked\":true,\"stats\":\"guarding 64 ran 0 counted 0\"}}")
        );
        // The computer's next start: the build kept.
        assert_eq!(remove(&tree.0, &mut kernel), Ok(()));
        assert!(!loaded(&tree.0));
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::Loaded));
        assert_eq!((kernel.built, kernel.loaded), (1, 2));
        // Refused once (built for a kernel since rebuilt): built again, then in.
        assert_eq!(remove(&tree.0, &mut kernel), Ok(()));
        kernel.refuses = 1;
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::Loaded));
        assert_eq!((kernel.built, kernel.loaded), (2, 3));
    }

    /// Nothing is built or loaded where there is no embedded controller,
    /// or on a kernel series the driver wasn't checked in: said, and the
    /// kernel left as it comes.
    #[test]
    fn only_where_there_is_a_controller_and_the_series_was_checked() {
        let none = Tree::new("no-controller", false);
        none.headers(K);
        let mut kernel = Fake { places: none.0.clone(), built: 0, loaded: 0, refuses: 0 };
        assert!(ensure(&none.0, K, &mut kernel).unwrap_err().contains("no embedded controller"));
        let tree = Tree::new("unchecked", true);
        let next = "9.9.1-100.fc50.x86_64";
        tree.headers(next);
        let mut kernel2 = Fake { places: tree.0.clone(), built: 0, loaded: 0, refuses: 0 };
        let why = ensure(&tree.0, next, &mut kernel2).unwrap_err();
        assert!(why.contains("9.9") && why.contains("hasn't checked"), "{why}");
        assert_eq!((kernel.built, kernel.loaded, kernel2.built, kernel2.loaded), (0, 0, 0, 0));
        assert!(checked(K) && checked("7.3.3-300.fc45.x86_64") && !checked(next));
        assert_eq!(series(K), "7.2");
    }
}
