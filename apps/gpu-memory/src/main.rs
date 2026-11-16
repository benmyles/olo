//! olo-gpu-memory: graphics memory stays in memory.
//!
//! The kernel's GPU memory manager (TTM, under Intel's xe driver) backs
//! windows' buffers up to swap whenever the kernel is short of memory,
//! were it only of file cache (a build's output), and a buffer comes back
//! inside its program's next GPU submission, under locks app_server's
//! every submission takes too: the desktop stood still for up to seconds
//! (Ben, 2026-11-16: "i just had a responsiveness stall when i was typing
//! and it actually even repeated a letter"; HACKING, "Windows' buffers
//! stay in memory"). TTM's one gate is whether there is swap space free
//! (`ttm_backup_bytes_avail`). Olo's live patch (kernel/gpu-memory,
//! `olo_gpu_memory`) replaces that one function in the running kernel
//! with one that says none: GPU buffers are never backed up, and the
//! computer keeps its swap for ordinary memory (Ben: "is there a way to
//! just prevent stuff that is going to be used with the gpu from going
//! into swap?").
//!
//! A kernel module is built for one kernel. This program, as root at
//! every start (systemd's `olo-gpu-memory.service`, installed by Olo
//! Doctor), builds the patch for the running kernel where it hasn't yet
//! (a new kernel's first start: three seconds; `olo_kmod` has how and
//! where, shared with Olo's other kernel modules), and loads it
//! (`finit_module`).
//! Where it can't (no headers, a kernel without live patching, Secure
//! Boot refusing an unsigned module) it says why and ends failed, which
//! Olo Doctor shows; nothing else is touched, and the kernel is as it
//! was.
//!
//! `olo-gpu-memory --status` says what there is, as anyone, in a line of
//! JSON; `--remove` (root) takes the patch out of the running kernel
//! again.

use olo_kmod::{Kernel, Module, Places, Running, read, release};
use std::{
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

/// The patch: its module's name (and its folder's in
/// /sys/kernel/livepatch), and its source, carried in this program.
const PATCH: &str = "olo_gpu_memory";
const SOURCE: &str = include_str!("../../../kernel/gpu-memory/olo_gpu_memory.c");
const KBUILD: &str = include_str!("../../../kernel/gpu-memory/Kbuild");
const MODULE: Module = Module { name: PATCH, state: "olo-gpu-memory", files: &[("Kbuild", KBUILD), ("olo_gpu_memory.c", SOURCE)] };

/// The kernel's live patches, and ours among them.
fn live_patches(places: &Places) -> PathBuf {
    places.root.join("sys/kernel/livepatch")
}

fn patch(places: &Places) -> PathBuf {
    live_patches(places).join(PATCH)
}

/// The patch is in the running kernel, and every task runs with it.
fn patched(places: &Places) -> bool {
    let patch = patch(places);
    read(&patch.join("enabled")).is_some_and(|e| e.trim() == "1") && read(&patch.join("transition")).is_none_or(|t| t.trim() == "0")
}

/// The patch is built for `release` from this program's source, and
/// installed.
fn built(places: &Places, release: &str) -> bool {
    olo_kmod::built(places, &MODULE, release)
}

/// What making sure of the patch came to.
#[derive(Debug, PartialEq)]
enum Done {
    /// In the kernel already.
    Already,
    Loaded,
    BuiltAndLoaded,
}

/// The patch in the kernel `release`: built where it isn't (or was from
/// another source), loaded, and every task running with it.
fn ensure(places: &Places, release: &str, kernel: &mut dyn Kernel, patience: Duration) -> Result<Done, String> {
    if patched(places) {
        return Ok(Done::Already);
    }
    if !live_patches(places).is_dir() {
        return Err("this kernel can't be patched while it runs (it has no live patching)".into());
    }
    let had = built(places, release);
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
    // In, once every task has left the old function (at once: nothing
    // sleeps in it).
    let until = Instant::now() + patience;
    while !patched(places) {
        if Instant::now() >= until {
            return Err("the kernel took the patch but hasn't switched every task to it".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    olo_kmod::tidy(places, &MODULE, release);
    Ok(if had { Done::Loaded } else { Done::BuiltAndLoaded })
}

/// How it went, written where Olo Doctor reads it: "patched", or
/// "failed: " and why.
fn say(places: &Places, went: &Result<Done, String>) {
    olo_kmod::say(
        places,
        &MODULE,
        &match went {
            Ok(_) => "patched".to_owned(),
            Err(e) => format!("failed: {e}"),
        },
    );
}

/// What there is, in a line of JSON.
fn status(places: &Places, release: &str) -> String {
    let swap = read(&places.root.join("proc/swaps")).is_some_and(|t| t.lines().count() > 1);
    format!(
        "{{\"kernel\":\"{release}\",\"patched\":{},\"built\":{},\"headers\":{},\"live_patching\":{},\"swap\":{swap}}}",
        patched(places),
        built(places, release),
        places.headers(release).join("Makefile").is_file(),
        live_patches(places).is_dir(),
    )
}

/// The patch taken out of the running kernel: switched off, and its
/// module unloaded once no task runs with it.
fn remove(places: &Places, kernel: &mut dyn Kernel) -> Result<(), String> {
    let enabled = patch(places).join("enabled");
    if !enabled.exists() {
        return Ok(());
    }
    std::fs::write(&enabled, "0").map_err(|e| format!("{}: {e}", enabled.display()))?;
    let until = Instant::now() + Duration::from_secs(10);
    while patch(places).exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    match kernel.unload(PATCH) {
        Ok(true) => Ok(()),
        Ok(false) => Err("the patch is off, its module still loaded: in use".into()),
        Err(e) => Err(format!("the patch is off, its module still loaded: {e}")),
    }
}

fn main() -> ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (places, release) = (Places::here(), release());
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {
            let went = ensure(&places, &release, &mut Running, Duration::from_secs(10));
            say(&places, &went);
            match went {
                Ok(Done::Already) => {
                    println!("olo-gpu-memory: graphics memory stays out of swap (the patch was in already)");
                    ExitCode::SUCCESS
                }
                Ok(Done::Loaded) => {
                    println!("olo-gpu-memory: graphics memory stays out of swap (the patch loaded for {release})");
                    ExitCode::SUCCESS
                }
                Ok(Done::BuiltAndLoaded) => {
                    println!("olo-gpu-memory: graphics memory stays out of swap (the patch built for {release} and loaded)");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("olo-gpu-memory: graphics memory can still go to swap: {e}");
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
                println!("olo-gpu-memory: the patch is out of the running kernel");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-gpu-memory: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            println!(
                "usage: olo-gpu-memory [--status | --remove]\n  (none)    build Olo's live patch for the running kernel where it isn't yet, and load it (root; systemd's olo-gpu-memory.service)\n  --status  what there is, a line of JSON (anyone)\n  --remove  take the patch out of the running kernel (root)"
            );
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A computer in a folder, removed when the test is over.
    struct Tree(Places);

    impl Tree {
        fn new(name: &str) -> Tree {
            let root = PathBuf::from(format!("/var/tmp/olo-gpu-memory-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("sys/kernel/livepatch")).unwrap();
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
    /// by listing the patch as its live patches are.
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
            assert_eq!(read(&dir.join(format!("{PATCH}.c"))).as_deref(), Some(SOURCE));
            self.built += 1;
            std::fs::write(dir.join(format!("{PATCH}.ko")), "built").map_err(|e| e.to_string())
        }

        fn load(&mut self, module: &Path) -> Result<(), String> {
            assert!(module.is_file());
            if self.refuses > 0 {
                self.refuses -= 1;
                return Err("refused".into());
            }
            self.loaded += 1;
            let patch = patch(&self.places);
            std::fs::create_dir_all(&patch).unwrap();
            std::fs::write(patch.join("enabled"), "1\n").unwrap();
            std::fs::write(patch.join("transition"), "0\n").unwrap();
            Ok(())
        }

        fn unload(&mut self, name: &str) -> Result<bool, String> {
            assert_eq!(name, PATCH);
            Ok(true)
        }
    }

    const K: &str = "7.2.15-300.fc45.x86_64";
    const SOON: Duration = Duration::from_millis(200);

    /// A kernel's first start with Olo: the patch built for it, kept and
    /// loaded; at its next, only loaded; in already, nothing done.
    #[test]
    fn built_once_a_kernel_and_loaded_at_each_start() {
        let tree = Tree::new("built-once");
        tree.headers(K);
        let mut kernel = Fake { places: tree.0.clone(), built: 0, loaded: 0, refuses: 0 };
        assert!(!patched(&tree.0) && !built(&tree.0, K));
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Ok(Done::BuiltAndLoaded));
        assert!(patched(&tree.0) && built(&tree.0, K));
        assert_eq!((kernel.built, kernel.loaded), (1, 1));
        // In already (the service started again).
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Ok(Done::Already));
        assert_eq!((kernel.built, kernel.loaded), (1, 1));
        // The computer's next start: the kernel as it was, the build kept.
        std::fs::remove_dir_all(patch(&tree.0)).unwrap();
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Ok(Done::Loaded));
        assert_eq!((kernel.built, kernel.loaded), (1, 2));
        assert_eq!(status(&tree.0, K), format!("{{\"kernel\":\"{K}\",\"patched\":true,\"built\":true,\"headers\":true,\"live_patching\":true,\"swap\":false}}"));
    }

    /// A build from another source (Olo upgraded) is made again; so is
    /// one the kernel refuses (the same name, rebuilt), once.
    #[test]
    fn built_again_for_a_new_source_or_a_changed_kernel() {
        let tree = Tree::new("built-again");
        tree.headers(K);
        let mut kernel = Fake { places: tree.0.clone(), built: 0, loaded: 0, refuses: 0 };
        std::fs::create_dir_all(tree.0.module(&MODULE, K).parent().unwrap()).unwrap();
        std::fs::create_dir_all(tree.0.work(&MODULE, K)).unwrap();
        std::fs::write(tree.0.module(&MODULE, K), "old").unwrap();
        std::fs::write(tree.0.work(&MODULE, K).join("Kbuild"), KBUILD).unwrap();
        std::fs::write(tree.0.work(&MODULE, K).join(format!("{PATCH}.c")), "an older source").unwrap();
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Ok(Done::BuiltAndLoaded));
        assert_eq!((kernel.built, read(&tree.0.module(&MODULE, K)).as_deref()), (1, Some("built")));
        // Refused once though built from this source: built again, taken.
        std::fs::remove_dir_all(patch(&tree.0)).unwrap();
        kernel.refuses = 1;
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Ok(Done::Loaded));
        assert_eq!((kernel.built, kernel.loaded), (2, 2));
        // Refused however built: said, and nothing left looking loaded.
        std::fs::remove_dir_all(patch(&tree.0)).unwrap();
        kernel.refuses = 2;
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Err("refused".into()));
        assert!(!patched(&tree.0));
    }

    /// What it can't do, said: no headers for this kernel (and nothing
    /// left looking built), a build that fails, a kernel without live
    /// patching.
    #[test]
    fn what_it_cant_do_is_said() {
        let tree = Tree::new("cant");
        let mut kernel = Fake { places: tree.0.clone(), built: 0, loaded: 0, refuses: 0 };
        let e = ensure(&tree.0, K, &mut kernel, SOON).unwrap_err();
        assert_eq!(e, format!("this kernel's headers aren't installed (the kernel-devel package for {K})"));
        assert!(!built(&tree.0, K) && kernel.built == 0);
        // (Said where Olo Doctor reads it; and "patched" once it's in.)
        say(&tree.0, &Err(e));
        assert_eq!(read(&tree.0.said(&MODULE)).as_deref(), Some(format!("failed: this kernel's headers aren't installed (the kernel-devel package for {K})\n").as_str()));
        assert!(tree.0.said(&MODULE).ends_with("run/olo-gpu-memory/status"));
        say(&tree.0, &Ok(Done::Loaded));
        assert_eq!(read(&tree.0.said(&MODULE)).as_deref(), Some("patched\n"));
        // A build that fails leaves nothing that looks built.
        struct Broken;
        impl Kernel for Broken {
            fn build(&mut self, _: &Path, _: &Path) -> Result<(), String> {
                Err("the kernel's build failed: no compiler".into())
            }
            fn load(&mut self, _: &Path) -> Result<(), String> {
                unreachable!("nothing to load")
            }
            fn unload(&mut self, _: &str) -> Result<bool, String> {
                unreachable!("nothing to unload")
            }
        }
        tree.headers(K);
        assert_eq!(ensure(&tree.0, K, &mut Broken, SOON), Err("the kernel's build failed: no compiler".into()));
        assert!(!built(&tree.0, K));
        // No live patching in this kernel.
        std::fs::remove_dir_all(live_patches(&tree.0)).unwrap();
        assert!(ensure(&tree.0, K, &mut kernel, SOON).unwrap_err().contains("no live patching"));
        assert!(status(&tree.0, K).contains("\"live_patching\":false"));
    }

    /// Taken out again: switched off, and its module unloaded.
    #[test]
    fn taken_out_again() {
        let tree = Tree::new("remove");
        tree.headers(K);
        let mut kernel = Fake { places: tree.0.clone(), built: 0, loaded: 0, refuses: 0 };
        // (Not in: nothing to do.)
        assert_eq!(remove(&tree.0, &mut kernel), Ok(()));
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Ok(Done::BuiltAndLoaded));
        // (The kernel takes the patch's folder away once it's off: here, a thread.)
        let folder = patch(&tree.0);
        let gone = std::thread::spawn(move || {
            while read(&folder.join("enabled")).is_some_and(|e| e.trim() == "1") {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::fs::remove_dir_all(&folder).unwrap();
        });
        assert_eq!(remove(&tree.0, &mut kernel), Ok(()));
        gone.join().unwrap();
        assert!(!patched(&tree.0));
    }

    /// The patch's source is what the kernel's build needs it to be: a
    /// live patch of TTM's one gate, under the kernel's licence.
    #[test]
    fn the_patch_is_ttms_gate() {
        assert!(SOURCE.contains(".old_name = \"ttm_backup_bytes_avail\"") && SOURCE.contains(".name = \"ttm\""));
        assert!(SOURCE.contains("MODULE_INFO(livepatch, \"Y\")") && SOURCE.contains("MODULE_LICENSE(\"GPL\")"));
        assert!(SOURCE.starts_with("// SPDX-License-Identifier: GPL-2.0\n"));
        assert_eq!(KBUILD, format!("obj-m := {PATCH}.o\n"));
    }

    /// The real build, against this computer's kernel headers
    /// (`--ignored`: needs kernel-devel, make and the compiler; loads
    /// nothing).
    #[test]
    #[ignore]
    fn builds_for_this_kernel() {
        let tree = Tree::new("real-build");
        let release = release();
        let real = Places::here().headers(&release);
        assert!(real.join("Makefile").is_file(), "kernel-devel for {release} isn't installed");
        std::fs::create_dir_all(tree.0.kernels().join(&release)).unwrap();
        std::os::unix::fs::symlink(std::fs::canonicalize(&real).unwrap(), tree.0.headers(&release)).unwrap();
        olo_kmod::build(&tree.0, &MODULE, &release, &mut Running).unwrap();
        assert!(tree.0.module(&MODULE, &release).ends_with(format!("lib/modules/{release}/extra/olo/{PATCH}.ko")));
        let module = std::fs::read(tree.0.module(&MODULE, &release)).unwrap();
        let has = |what: &str| module.windows(what.len()).any(|w| w == what.as_bytes());
        assert!(has("livepatch=Y") && has(&format!("vermagic={release} ")) && has("ttm_backup_bytes_avail"));
        assert!(built(&tree.0, &release));
    }
}
