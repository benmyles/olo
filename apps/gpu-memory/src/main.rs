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
//! (a new kernel's first start: three seconds, with the kernel's own
//! build system, `make` and the compiler, the one way a module can be
//! built for it; its headers are the `kernel-devel` package's, and the
//! work is done in /var/lib/olo-gpu-memory/KERNEL), installs it where
//! locally built modules go, /lib/modules/KERNEL/extra/olo (the one
//! place SELinux lets a service load a module from: from /var/lib the
//! kernel refused it, "Permission denied", on Ben's computer), and loads
//! it (`finit_module`).
//! Where it can't (no headers, a kernel without live patching, Secure
//! Boot refusing an unsigned module) it says why and ends failed, which
//! Olo Doctor shows; nothing else is touched, and the kernel is as it
//! was.
//!
//! `olo-gpu-memory --status` says what there is, as anyone, in a line of
//! JSON; `--remove` (root) takes the patch out of the running kernel
//! again.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};

/// The patch's name: its module's, and its folder's in
/// /sys/kernel/livepatch.
const PATCH: &str = "olo_gpu_memory";
/// Its source, carried in this program.
const SOURCE: &str = include_str!("../../../kernel/gpu-memory/olo_gpu_memory.c");
const KBUILD: &str = include_str!("../../../kernel/gpu-memory/Kbuild");

/// Where things are: the system's own places, or a test's tree.
#[derive(Debug, Clone)]
struct Places {
    root: PathBuf,
}

impl Places {
    fn here() -> Places {
        Places { root: "/".into() }
    }

    /// The kernel's live patches, and ours among them.
    fn live_patches(&self) -> PathBuf {
        self.root.join("sys/kernel/livepatch")
    }

    fn patch(&self) -> PathBuf {
        self.live_patches().join(PATCH)
    }

    /// The kernels installed, each with its `build` (its headers, where
    /// kernel-devel is installed for it).
    fn kernels(&self) -> PathBuf {
        self.root.join("lib/modules")
    }

    fn headers(&self, release: &str) -> PathBuf {
        self.kernels().join(release).join("build")
    }

    /// Where the patch is built, a folder a kernel (its source and the
    /// build's files).
    fn works(&self) -> PathBuf {
        self.root.join("var/lib/olo-gpu-memory")
    }

    fn work(&self, release: &str) -> PathBuf {
        self.works().join(release)
    }

    /// The patch as installed for a kernel: with its other locally
    /// built modules.
    fn module(&self, release: &str) -> PathBuf {
        self.kernels().join(release).join("extra/olo").join(format!("{PATCH}.ko"))
    }

    /// A kernel of this release is installed (its image is there; a
    /// removed one's folder may outlive it, ours in it).
    fn installed(&self, release: &str) -> bool {
        self.kernels().join(release).join("vmlinuz").is_file()
    }

    /// How the service's last try went, a line: "patched", or "failed: "
    /// and why (Olo Doctor reads it).
    fn said(&self) -> PathBuf {
        self.root.join("run/olo-gpu-memory/status")
    }
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The patch is in the running kernel, and every task runs with it.
fn patched(places: &Places) -> bool {
    let patch = places.patch();
    read(&patch.join("enabled")).is_some_and(|e| e.trim() == "1") && read(&patch.join("transition")).is_none_or(|t| t.trim() == "0")
}

/// The patch is built for `release` from this program's source, and
/// installed.
fn built(places: &Places, release: &str) -> bool {
    places.module(release).is_file() && read(&places.work(release).join(format!("{PATCH}.c"))).as_deref() == Some(SOURCE)
}

/// The kernel's part: building a module for it and loading one. (The
/// real kernel's, or a test's.)
trait Kernel {
    /// Build the module whose source is in `dir` against `headers`.
    fn build(&mut self, headers: &Path, dir: &Path) -> Result<(), String>;
    fn load(&mut self, module: &Path) -> Result<(), String>;
}

struct Running;

impl Kernel for Running {
    fn build(&mut self, headers: &Path, dir: &Path) -> Result<(), String> {
        // The kernel's own build system: the one program this runs.
        let out = std::process::Command::new("/usr/bin/make")
            .arg("-C")
            .arg(headers)
            .arg(format!("M={}", dir.display()))
            .arg("modules")
            .env_clear()
            .env("PATH", "/usr/bin:/usr/sbin:/bin:/sbin")
            .env("LANG", "C")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("make couldn't be run: {e} (the fix installs it)"))?;
        if out.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&out.stderr);
        let last: Vec<&str> = said.lines().filter(|l| !l.trim().is_empty()).rev().take(4).collect();
        Err(format!("the kernel's build failed: {}", last.into_iter().rev().collect::<Vec<_>>().join(" | ")))
    }

    fn load(&mut self, module: &Path) -> Result<(), String> {
        use std::os::fd::AsRawFd as _;
        let file = std::fs::File::open(module).map_err(|e| format!("{}: {e}", module.display()))?;
        // SAFETY: finit_module(2) with an open file and an empty,
        // NUL-terminated parameter string.
        let r = unsafe { libc::syscall(libc::SYS_finit_module, file.as_raw_fd(), c"".as_ptr(), 0) };
        if r == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EEXIST) => Ok(()),
            Some(libc::EKEYREJECTED) | Some(libc::EPERM) => Err(format!("the kernel refused the module ({e}): with Secure Boot it loads only signed ones")),
            _ => Err(format!("the kernel didn't take the module: {e}")),
        }
    }
}

/// What making sure of the patch came to.
#[derive(Debug, PartialEq)]
enum Done {
    /// In the kernel already.
    Already,
    Loaded,
    BuiltAndLoaded,
}

/// The patch built for `release` from this program's source, in its
/// work folder, and installed among the kernel's modules.
fn build(places: &Places, release: &str, kernel: &mut dyn Kernel) -> Result<(), String> {
    let headers = places.headers(release);
    if !headers.join("Makefile").is_file() {
        return Err(format!("this kernel's headers aren't installed (the kernel-devel package for {release})"));
    }
    let (dir, module) = (places.work(release), places.module(release));
    let source = dir.join(format!("{PATCH}.c"));
    let at = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    // (Afresh: nothing of an older source's build is taken for this one's.)
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| at(&dir, e))?;
    std::fs::write(dir.join("Kbuild"), KBUILD).map_err(|e| at(&dir, e))?;
    std::fs::write(&source, SOURCE).map_err(|e| at(&source, e))?;
    let made = kernel.build(&headers, &dir).and_then(|()| {
        // Installed through a sibling, renamed into place: never half there.
        let made = dir.join(format!("{PATCH}.ko"));
        let to = module.parent().expect("in a folder");
        std::fs::create_dir_all(to).map_err(|e| at(to, e))?;
        let new = module.with_extension("ko.new");
        std::fs::copy(&made, &new).map_err(|e| if made.is_file() { at(&new, e) } else { "the build made no module".into() })?;
        std::fs::rename(&new, &module).map_err(|e| at(&module, e))
    });
    if made.is_err() {
        // (Not left looking built.)
        let _ = std::fs::remove_file(&source);
    }
    made
}

/// The patch in the kernel `release`: built where it isn't (or was from
/// another source), loaded, and every task running with it.
fn ensure(places: &Places, release: &str, kernel: &mut dyn Kernel, patience: Duration) -> Result<Done, String> {
    if patched(places) {
        return Ok(Done::Already);
    }
    if !places.live_patches().is_dir() {
        return Err("this kernel can't be patched while it runs (it has no live patching)".into());
    }
    let had = built(places, release);
    if !had {
        build(places, release, kernel)?;
    }
    if let Err(e) = kernel.load(&places.module(release)) {
        // One built for a kernel of the same name that has since changed
        // (the package rebuilt): once more, from scratch.
        if !had {
            return Err(e);
        }
        build(places, release, kernel)?;
        kernel.load(&places.module(release))?;
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
    tidy(places, release);
    Ok(if had { Done::Loaded } else { Done::BuiltAndLoaded })
}

/// What was built for kernels no longer installed, removed: the work
/// folder, the module, and the kernel's own folders it alone kept.
fn tidy(places: &Places, release: &str) {
    for e in std::fs::read_dir(places.works()).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == release || places.installed(&name) {
            continue;
        }
        let _ = std::fs::remove_dir_all(e.path());
        let ours = places.module(&name).parent().expect("in a folder").to_owned();
        let _ = std::fs::remove_dir_all(&ours);
        // (Each only if nothing else is left in it.)
        for dir in ours.ancestors().skip(1).take(2) {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// How it went, written where Olo Doctor reads it.
fn say(places: &Places, went: &Result<Done, String>) {
    let line = match went {
        Ok(_) => "patched\n".to_owned(),
        Err(e) => format!("failed: {e}\n"),
    };
    let said = places.said();
    if let Some(dir) = said.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(said, line);
}

/// The running kernel's release ("7.2.15-300.fc45.x86_64").
fn release() -> String {
    // SAFETY: uname(2) fills the struct it's given.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        libc::uname(&mut u);
        std::ffi::CStr::from_ptr(u.release.as_ptr()).to_string_lossy().into_owned()
    }
}

/// What there is, in a line of JSON.
fn status(places: &Places, release: &str) -> String {
    let swap = read(&places.root.join("proc/swaps")).is_some_and(|t| t.lines().count() > 1);
    format!(
        "{{\"kernel\":\"{release}\",\"patched\":{},\"built\":{},\"headers\":{},\"live_patching\":{},\"swap\":{swap}}}",
        patched(places),
        built(places, release),
        places.headers(release).join("Makefile").is_file(),
        places.live_patches().is_dir(),
    )
}

/// The patch taken out of the running kernel: switched off, and its
/// module unloaded once no task runs with it.
fn remove(places: &Places) -> Result<(), String> {
    let enabled = places.patch().join("enabled");
    if !enabled.exists() {
        return Ok(());
    }
    std::fs::write(&enabled, "0").map_err(|e| format!("{}: {e}", enabled.display()))?;
    let until = Instant::now() + Duration::from_secs(10);
    while places.patch().exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    let name = std::ffi::CString::new(PATCH).expect("no NUL");
    // SAFETY: delete_module(2) with a NUL-terminated name; O_NONBLOCK: refused at once if in use.
    if unsafe { libc::syscall(libc::SYS_delete_module, name.as_ptr(), libc::O_NONBLOCK) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ENOENT) {
            return Err(format!("the patch is off, its module still loaded: {e}"));
        }
    }
    Ok(())
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
        ["--remove"] => match remove(&places) {
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
            let patch = self.places.patch();
            std::fs::create_dir_all(&patch).unwrap();
            std::fs::write(patch.join("enabled"), "1\n").unwrap();
            std::fs::write(patch.join("transition"), "0\n").unwrap();
            Ok(())
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
        std::fs::remove_dir_all(tree.0.patch()).unwrap();
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
        std::fs::create_dir_all(tree.0.module(K).parent().unwrap()).unwrap();
        std::fs::create_dir_all(tree.0.work(K)).unwrap();
        std::fs::write(tree.0.module(K), "old").unwrap();
        std::fs::write(tree.0.work(K).join(format!("{PATCH}.c")), "an older source").unwrap();
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Ok(Done::BuiltAndLoaded));
        assert_eq!((kernel.built, read(&tree.0.module(K)).as_deref()), (1, Some("built")));
        // Refused once though built from this source: built again, taken.
        std::fs::remove_dir_all(tree.0.patch()).unwrap();
        kernel.refuses = 1;
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Ok(Done::Loaded));
        assert_eq!((kernel.built, kernel.loaded), (2, 2));
        // Refused however built: said, and nothing left looking loaded.
        std::fs::remove_dir_all(tree.0.patch()).unwrap();
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
        assert_eq!(read(&tree.0.said()).as_deref(), Some(format!("failed: this kernel's headers aren't installed (the kernel-devel package for {K})\n").as_str()));
        say(&tree.0, &Ok(Done::Loaded));
        assert_eq!(read(&tree.0.said()).as_deref(), Some("patched\n"));
        // A build that fails leaves nothing that looks built.
        struct Broken;
        impl Kernel for Broken {
            fn build(&mut self, _: &Path, _: &Path) -> Result<(), String> {
                Err("the kernel's build failed: no compiler".into())
            }
            fn load(&mut self, _: &Path) -> Result<(), String> {
                unreachable!("nothing to load")
            }
        }
        tree.headers(K);
        assert_eq!(ensure(&tree.0, K, &mut Broken, SOON), Err("the kernel's build failed: no compiler".into()));
        assert!(!built(&tree.0, K));
        // No live patching in this kernel.
        std::fs::remove_dir_all(tree.0.live_patches()).unwrap();
        assert!(ensure(&tree.0, K, &mut kernel, SOON).unwrap_err().contains("no live patching"));
        assert!(status(&tree.0, K).contains("\"live_patching\":false"));
    }

    /// What was built for kernels since removed goes, with the folders
    /// only it kept (a removed kernel's, left behind for our module); an
    /// installed kernel's stays, and so does another's module beside ours.
    #[test]
    fn old_kernels_builds_are_removed() {
        let tree = Tree::new("tidy");
        tree.headers(K);
        tree.headers("7.2.14-300.fc45.x86_64");
        let (kept, gone, shared) = ("7.2.14-300.fc45.x86_64", "7.1.9-200.fc44.x86_64", "7.1.8-200.fc44.x86_64");
        for old in [kept, gone, shared] {
            std::fs::create_dir_all(tree.0.work(old)).unwrap();
            std::fs::create_dir_all(tree.0.module(old).parent().unwrap()).unwrap();
            std::fs::write(tree.0.module(old), "built").unwrap();
        }
        // (Another maker's module left in a removed kernel's folder.)
        std::fs::create_dir_all(tree.0.kernels().join(shared).join("extra/nvidia")).unwrap();
        let mut kernel = Fake { places: tree.0.clone(), built: 0, loaded: 0, refuses: 0 };
        assert_eq!(ensure(&tree.0, K, &mut kernel, SOON), Ok(Done::BuiltAndLoaded));
        assert!(tree.0.module(kept).is_file() && tree.0.work(kept).is_dir(), "installed still: kept");
        assert!(!tree.0.work(gone).exists() && !tree.0.kernels().join(gone).exists(), "its kernel gone: removed, folders and all");
        assert!(!tree.0.module(shared).exists() && tree.0.kernels().join(shared).join("extra/nvidia").is_dir(), "ours gone, another's left");
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
        build(&tree.0, &release, &mut Running).unwrap();
        assert!(tree.0.module(&release).ends_with(format!("lib/modules/{release}/extra/olo/{PATCH}.ko")));
        let module = std::fs::read(tree.0.module(&release)).unwrap();
        let has = |what: &str| module.windows(what.len()).any(|w| w == what.as_bytes());
        assert!(has("livepatch=Y") && has(&format!("vermagic={release} ")) && has("ttm_backup_bytes_avail"));
        assert!(built(&tree.0, &release));
    }
}
