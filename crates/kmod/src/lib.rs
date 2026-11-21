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

/// A kernel build known by more than its name: its build ID, and one of
/// its modules'. A module of Olo's that reads what a function's
/// arguments are where one build keeps them (kernel/gpu-pages) is loaded
/// only into builds it was checked on, listed in a file of its own: a
/// line a build, "RELEASE KERNEL-BUILD-ID MODULE-BUILD-ID", `#` for
/// comments.
pub mod builds {
    use std::path::Path;

    /// The GNU build ID in a file of ELF notes, as hex: what the kernel
    /// shows of itself (/sys/kernel/notes) and of each module
    /// (/sys/module/NAME/notes/.note.gnu.build-id).
    pub fn build_id(notes: &[u8]) -> Option<String> {
        fn field(notes: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
            let padded = n.checked_next_multiple_of(4)?;
            Some((notes.get(..n)?, notes.get(padded..)?))
        }
        let mut rest = notes;
        let mut id = None;
        while !rest.is_empty() {
            let word = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().expect("four bytes"));
            let header = rest.get(..12)?;
            let (owner, tail) = field(&rest[12..], word(&header[..4]) as usize)?;
            let (value, tail) = field(tail, word(&header[4..8]) as usize)?;
            if word(&header[8..]) == 3 && owner == b"GNU\0" {
                if id.is_some() || value.is_empty() {
                    return None;
                }
                id = Some(value.iter().map(|b| format!("{b:02x}")).collect());
            }
            rest = tail;
        }
        id
    }

    /// The running kernel's build ID and its module `module`'s, where
    /// both say (anyone may read them). `root`: "/", or a test's tree.
    pub fn running(root: &Path, module: &str) -> Option<(String, String)> {
        let id = |path: &Path| std::fs::read(path).ok().and_then(|notes| build_id(&notes));
        Some((id(&root.join("sys/kernel/notes"))?, id(&root.join("sys/module").join(module).join("notes/.note.gnu.build-id"))?))
    }

    /// `list`'s line for this very build of the kernel `release`: the
    /// words after the three that name the build (what was checked
    /// beside it: a live patch's build ID). None: not listed.
    pub fn entry<'a>(list: &'a str, release: &str, builds: &(String, String)) -> Option<Vec<&'a str>> {
        list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).find_map(|l| {
            let mut words = l.split_whitespace();
            ((words.next(), words.next(), words.next()) == (Some(release), Some(builds.0.as_str()), Some(builds.1.as_str()))).then(|| words.collect())
        })
    }

    /// `list` has this very build of the kernel `release`.
    pub fn listed(list: &str, release: &str, builds: &(String, String)) -> bool {
        entry(list, release, builds).is_some()
    }

    /// A file of ELF notes as the kernel shows them, for tests: another
    /// note first, then the GNU build ID `build` (hex).
    pub fn notes_for_tests(build: &str) -> Vec<u8> {
        let id: Vec<u8> = (0..build.len() / 2).filter_map(|i| u8::from_str_radix(&build[2 * i..2 * i + 2], 16).ok()).collect();
        let mut out = Vec::new();
        for (owner, kind, value) in [(&b"Linux\0"[..], 0x101u32, &[1u8, 2, 3, 4, 5][..]), (&b"GNU\0"[..], 3, &id[..])] {
            for n in [owner.len() as u32, value.len() as u32, kind] {
                out.extend(n.to_le_bytes());
            }
            for part in [owner, value] {
                out.extend(part);
                out.resize(out.len().next_multiple_of(4), 0);
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A build ID is read out of the kernel's notes wherever it
        /// stands among them, none where there is none; and a list has a
        /// build only by all three of its words.
        #[test]
        fn a_build_is_known_by_its_ids_and_listed_by_all_three_words() {
            const KERNEL: &str = "7f111d5ff77cd8b0de0b41422fe4d4f7d375f54d";
            const TTM: &str = "e0fdead3891ad14b87586bbe3bef4fe33c1d65cc";
            assert_eq!(build_id(&notes_for_tests(KERNEL)).as_deref(), Some(KERNEL));
            assert_eq!(build_id(&notes_for_tests(KERNEL)[..20]), None);
            assert_eq!(build_id(b""), None);
            // Notes that aren't whole, or that say more than one thing of
            // the build, name no build (the third review's cases): bytes
            // left over, a length past the end, two build IDs, an empty
            // one.
            let whole = notes_for_tests(KERNEL);
            assert_eq!(build_id(&[&whole[..], &[1, 2, 3][..]].concat()), None);
            let mut long = whole.clone();
            let at = long.len() - 20 - 4 - 12 + 4;
            long[at..at + 4].copy_from_slice(&40u32.to_le_bytes());
            assert_eq!(build_id(&long), None, "a build ID said to be twice as long as what is there");
            let gnu = &whole[whole.len() - 36..];
            assert_eq!(build_id(gnu).as_deref(), Some(KERNEL));
            assert_eq!(build_id(&[gnu, gnu].concat()), None);
            assert_eq!(build_id(&notes_for_tests("")), None);
            let root = std::path::PathBuf::from(format!("/var/tmp/olo-kmod-builds-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            assert_eq!(running(&root, "ttm"), None);
            std::fs::create_dir_all(root.join("sys/kernel")).unwrap();
            std::fs::create_dir_all(root.join("sys/module/ttm/notes")).unwrap();
            std::fs::write(root.join("sys/kernel/notes"), notes_for_tests(KERNEL)).unwrap();
            assert_eq!(running(&root, "ttm"), None, "the module doesn't say");
            std::fs::write(root.join("sys/module/ttm/notes/.note.gnu.build-id"), notes_for_tests(TTM)).unwrap();
            let builds = running(&root, "ttm").unwrap();
            assert_eq!(builds, (KERNEL.to_owned(), TTM.to_owned()));
            let (k, next) = ("7.3.4-300.fc45.x86_64", "7.3.5-300.fc45.x86_64");
            let list = format!("# {k} {KERNEL} {TTM}\n\n  {next} {KERNEL} {TTM}  \n{k} {KERNEL}\n");
            assert!(listed(&list, next, &builds) && !listed(&list, k, &builds) && !listed("", k, &builds));
            assert!(!listed(&list, next, &(KERNEL.to_owned(), KERNEL.to_owned())));
            // What else was checked with a build: the words after its three.
            assert_eq!(entry(&list, next, &builds), Some(Vec::new()));
            let list = format!("{k} {KERNEL} {TTM} 5cb978 more\n");
            assert_eq!(entry(&list, k, &builds), Some(vec!["5cb978", "more"]));
            assert_eq!(entry(&list, next, &builds), None);
            let _ = std::fs::remove_dir_all(root);
        }
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
    /// [`Kernel::load`] with the module's parameters ("name=value …").
    /// A kernel that takes none (a test's) takes a module without any.
    fn load_with(&mut self, module: &Path, parameters: &str) -> Result<(), String> {
        if parameters.is_empty() { self.load(module) } else { Err(format!("this kernel takes no parameters ({parameters})")) }
    }
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
        self.load_with(module, "")
    }

    fn load_with(&mut self, module: &Path, parameters: &str) -> Result<(), String> {
        use std::os::fd::AsRawFd as _;
        let file = std::fs::File::open(module).map_err(|e| format!("{}: {e}", module.display()))?;
        let parameters = std::ffi::CString::new(parameters).map_err(|e| e.to_string())?;
        // SAFETY: finit_module(2) with an open file and a NUL-terminated
        // parameter string.
        let r = unsafe { libc::syscall(libc::SYS_finit_module, file.as_raw_fd(), parameters.as_ptr(), 0) };
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
