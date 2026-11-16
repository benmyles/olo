//! One of Olo's kernel modules, built for the running kernel and loaded.
//!
//! A kernel module is built for one kernel: a new kernel means a new
//! build. Each of Olo's modules (kernel/README.md) has a root service
//! that, at every start, builds the module for the running kernel where
//! it hasn't yet and loads it (apps/gpu-memory, apps/bbr3); this is what
//! those services share.
//!
//! The module's source is carried in its service's program
//! ([`Module::files`]). It's built with the kernel's own build system
//! (`make` and the compiler, the one way a module can be built for a
//! kernel; the kernel's headers are the `kernel-devel` package's), in a
//! work folder of its own, a kernel a folder
//! (/var/lib/STATE/KERNEL: the source as built, and the build's files),
//! and installed where locally built modules go,
//! /lib/modules/KERNEL/extra/olo (the one place SELinux lets a service
//! load a module from: from /var/lib the kernel refused it, "Permission
//! denied", on Ben's computer). A module built from another source (Olo
//! upgraded) is built again; what was built for kernels since removed is
//! removed ([`tidy`]).

use std::path::{Path, PathBuf};

/// One of Olo's kernel modules.
#[derive(Debug, Clone, Copy)]
pub struct Module {
    /// Its name: its file's (`NAME.ko`), and what the kernel lists it as.
    pub name: &'static str,
    /// Its service's folder's name, in /var/lib (where it's built) and
    /// in /run (where the service says how it went).
    pub state: &'static str,
    /// Its source: each file's name and contents, `Kbuild` among them.
    pub files: &'static [(&'static str, &'static str)],
}

/// Where things are: the system's own places, or a test's tree.
#[derive(Debug, Clone)]
pub struct Places {
    pub root: PathBuf,
}

impl Places {
    pub fn here() -> Places {
        Places { root: "/".into() }
    }

    /// The kernels installed, each with its `build` (its headers, where
    /// kernel-devel is installed for it).
    pub fn kernels(&self) -> PathBuf {
        self.root.join("lib/modules")
    }

    pub fn headers(&self, release: &str) -> PathBuf {
        self.kernels().join(release).join("build")
    }

    /// A kernel of this release is installed (its image is there; a
    /// removed one's folder may outlive it, ours in it).
    pub fn installed(&self, release: &str) -> bool {
        self.kernels().join(release).join("vmlinuz").is_file()
    }

    /// Where a module is built, a folder a kernel (its source and the
    /// build's files).
    pub fn works(&self, module: &Module) -> PathBuf {
        self.root.join("var/lib").join(module.state)
    }

    pub fn work(&self, module: &Module, release: &str) -> PathBuf {
        self.works(module).join(release)
    }

    /// A module as installed for a kernel: with its other locally built
    /// modules.
    pub fn module(&self, module: &Module, release: &str) -> PathBuf {
        self.kernels().join(release).join("extra/olo").join(format!("{}.ko", module.name))
    }

    /// The kernel's own entry for a module that is loaded.
    pub fn loaded(&self, module: &Module) -> PathBuf {
        self.root.join("sys/module").join(module.name)
    }

    /// How the service's last try went, a line (Olo Doctor reads it).
    pub fn said(&self, module: &Module) -> PathBuf {
        self.root.join("run").join(module.state).join("status")
    }
}

pub fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The kernel's part: building a module for it, loading one and
/// unloading one. (The real kernel's, or a test's.)
pub trait Kernel {
    /// Build the module whose source is in `dir` against `headers`.
    fn build(&mut self, headers: &Path, dir: &Path) -> Result<(), String>;
    fn load(&mut self, module: &Path) -> Result<(), String>;
    /// Unload the module named; false if it's in use still (or the
    /// kernel says no), true if it's out, or wasn't in.
    fn unload(&mut self, name: &str) -> Result<bool, String>;
}

pub struct Running;

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

    fn unload(&mut self, name: &str) -> Result<bool, String> {
        let name = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
        // SAFETY: delete_module(2) with a NUL-terminated name; O_NONBLOCK: refused at once if in use.
        if unsafe { libc::syscall(libc::SYS_delete_module, name.as_ptr(), libc::O_NONBLOCK) } == 0 {
            return Ok(true);
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::ENOENT) => Ok(true),
            Some(libc::EWOULDBLOCK) | Some(libc::EBUSY) => Ok(false),
            _ => Err(format!("the kernel didn't let the module go: {e}")),
        }
    }
}

/// The module is built for `release` from this program's source, and
/// installed.
pub fn built(places: &Places, module: &Module, release: &str) -> bool {
    let dir = places.work(module, release);
    places.module(module, release).is_file() && module.files.iter().all(|(name, contents)| read(&dir.join(name)).as_deref() == Some(*contents))
}

/// The module built for `release` from this program's source, in its
/// work folder, and installed among the kernel's modules.
pub fn build(places: &Places, module: &Module, release: &str, kernel: &mut dyn Kernel) -> Result<(), String> {
    let headers = places.headers(release);
    if !headers.join("Makefile").is_file() {
        return Err(format!("this kernel's headers aren't installed (the kernel-devel package for {release})"));
    }
    let (dir, installed) = (places.work(module, release), places.module(module, release));
    let at = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    // (Afresh: nothing of an older source's build is taken for this one's.)
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| at(&dir, e))?;
    for (name, contents) in module.files {
        let file = dir.join(name);
        std::fs::write(&file, contents).map_err(|e| at(&file, e))?;
    }
    let made = kernel.build(&headers, &dir).and_then(|()| {
        // Installed through a sibling, renamed into place: never half there.
        let made = dir.join(format!("{}.ko", module.name));
        let to = installed.parent().expect("in a folder");
        std::fs::create_dir_all(to).map_err(|e| at(to, e))?;
        let new = installed.with_extension("ko.new");
        std::fs::copy(&made, &new).map_err(|e| if made.is_file() { at(&new, e) } else { "the build made no module".into() })?;
        std::fs::rename(&new, &installed).map_err(|e| at(&installed, e))
    });
    if made.is_err() {
        // (Not left looking built.)
        for (name, _) in module.files {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
    made
}

/// What was built for kernels no longer installed, removed: the work
/// folder, the module, and the kernel's own folders it alone kept.
pub fn tidy(places: &Places, module: &Module, release: &str) {
    for e in std::fs::read_dir(places.works(module)).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == release || places.installed(&name) {
            continue;
        }
        let _ = std::fs::remove_dir_all(e.path());
        let ours = places.module(module, &name);
        let _ = std::fs::remove_file(&ours);
        // (Each folder only if nothing else is left in it: another of
        // Olo's modules, another maker's.)
        for dir in ours.ancestors().skip(1).take(3) {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// How it went, written where Olo Doctor reads it: `line`, as it is.
pub fn say(places: &Places, module: &Module, line: &str) {
    let said = places.said(module);
    if let Some(dir) = said.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(said, format!("{line}\n"));
}

/// The running kernel's release ("7.2.15-300.fc45.x86_64").
pub fn release() -> String {
    // SAFETY: uname(2) fills the struct it's given.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        libc::uname(&mut u);
        std::ffi::CStr::from_ptr(u.release.as_ptr()).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODULE: Module = Module { name: "olo_test", state: "olo-test", files: &[("Kbuild", "obj-m := olo_test.o\n"), ("olo_test.c", "the source"), ("olo_test.h", "its header")] };
    const K: &str = "7.2.15-300.fc45.x86_64";

    struct Tree(Places);

    impl Tree {
        fn new(name: &str) -> Tree {
            let root = PathBuf::from(format!("/var/tmp/olo-kmod-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Tree(Places { root })
        }

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

    /// A kernel whose build writes the module's file, or fails.
    struct Fake {
        fails: bool,
        built: usize,
    }

    impl Kernel for Fake {
        fn build(&mut self, headers: &Path, dir: &Path) -> Result<(), String> {
            assert!(headers.join("Makefile").is_file());
            for (name, contents) in MODULE.files {
                assert_eq!(read(&dir.join(name)).as_deref(), Some(*contents));
            }
            if self.fails {
                return Err("the kernel's build failed: no compiler".into());
            }
            self.built += 1;
            std::fs::write(dir.join("olo_test.ko"), "built").map_err(|e| e.to_string())
        }

        fn load(&mut self, _: &Path) -> Result<(), String> {
            unreachable!()
        }

        fn unload(&mut self, _: &str) -> Result<bool, String> {
            unreachable!()
        }
    }

    /// Built with every file of its source, installed where locally
    /// built modules go; built again only for another source.
    #[test]
    fn built_from_its_source_and_installed() {
        let tree = Tree::new("built");
        let places = &tree.0;
        let mut kernel = Fake { fails: false, built: 0 };
        assert_eq!(build(places, &MODULE, K, &mut kernel), Err(format!("this kernel's headers aren't installed (the kernel-devel package for {K})")));
        tree.headers(K);
        assert!(!built(places, &MODULE, K));
        build(places, &MODULE, K, &mut kernel).unwrap();
        assert!(built(places, &MODULE, K));
        assert!(places.module(&MODULE, K).ends_with(format!("lib/modules/{K}/extra/olo/olo_test.ko")));
        assert_eq!(read(&places.module(&MODULE, K)).as_deref(), Some("built"));
        // Another source (any one file of it): not this one's build.
        let newer = Module { files: &[("Kbuild", "obj-m := olo_test.o\n"), ("olo_test.c", "the source"), ("olo_test.h", "a newer header")], ..MODULE };
        assert!(!built(places, &newer, K));
        // A build that fails leaves nothing that looks built.
        assert_eq!(build(places, &MODULE, K, &mut Fake { fails: true, built: 0 }), Err("the kernel's build failed: no compiler".into()));
        assert!(!built(places, &MODULE, K));
    }

    /// What was built for kernels since removed goes, with the folders
    /// only it kept; an installed kernel's stays, and so does another
    /// module beside ours.
    #[test]
    fn old_kernels_builds_are_removed() {
        let tree = Tree::new("tidy");
        let places = &tree.0;
        tree.headers(K);
        tree.headers("7.2.14-300.fc45.x86_64");
        let (kept, gone, shared, ours_too) = ("7.2.14-300.fc45.x86_64", "7.1.9-200.fc44.x86_64", "7.1.8-200.fc44.x86_64", "7.1.7-200.fc44.x86_64");
        for old in [kept, gone, shared, ours_too] {
            std::fs::create_dir_all(places.work(&MODULE, old)).unwrap();
            std::fs::create_dir_all(places.module(&MODULE, old).parent().unwrap()).unwrap();
            std::fs::write(places.module(&MODULE, old), "built").unwrap();
        }
        // (Another maker's module left in a removed kernel's folder, and
        // another of Olo's beside this one.)
        std::fs::create_dir_all(places.kernels().join(shared).join("extra/nvidia")).unwrap();
        std::fs::write(places.kernels().join(ours_too).join("extra/olo/olo_other.ko"), "another").unwrap();
        tidy(places, &MODULE, K);
        assert!(places.module(&MODULE, kept).is_file() && places.work(&MODULE, kept).is_dir(), "installed still: kept");
        assert!(!places.work(&MODULE, gone).exists() && !places.kernels().join(gone).exists(), "its kernel gone: removed, folders and all");
        assert!(!places.module(&MODULE, shared).exists() && places.kernels().join(shared).join("extra/nvidia").is_dir(), "ours gone, another's left");
        assert!(!places.module(&MODULE, ours_too).exists() && places.kernels().join(ours_too).join("extra/olo/olo_other.ko").is_file(), "ours gone, Olo's other left");
    }

    #[test]
    fn how_it_went_is_said() {
        let tree = Tree::new("said");
        say(&tree.0, &MODULE, "failed: no compiler");
        assert_eq!(read(&tree.0.said(&MODULE)).as_deref(), Some("failed: no compiler\n"));
        assert!(tree.0.said(&MODULE).ends_with("run/olo-test/status"));
        assert!(!release().is_empty());
    }
}
