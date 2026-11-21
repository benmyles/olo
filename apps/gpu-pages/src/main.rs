//! olo-gpu-pages: no program is held for a 2 MB piece of graphics memory.
//!
//! The kernel's GPU memory manager (TTM, under Intel's xe driver) fills
//! every buffer with the largest pieces of memory it can get, and for
//! the size the driver names as worth having, 2 MB, Linux 7.3 has it try
//! hard: where no such piece is free it compacts memory and reclaims, on
//! the thread that made the buffer. Those threads draw. On a computer
//! whose free memory was in small pieces (builds running), apps' drawing
//! threads and a browser's GPU process were held a third to half of a
//! second at a time, the compositor's loop up to 8 ms (kernel/TUNING.md,
//! "Graphics memory in 2 MB pieces"; `sudo tools/dev/memory-holds`).
//! Olo's module (kernel/gpu-pages, `olo_gpu_pages`) has that one try made
//! without waiting: a free 2 MB piece is taken, the kernel's own threads
//! are woken to make more, and otherwise the buffer is filled with
//! smaller pieces, as the pool does by itself when a size is refused.
//!
//! A kernel module is built for one kernel. This program, as root at
//! every start (systemd's `olo-gpu-pages.service`, installed by Olo
//! Doctor), builds the module for the running kernel where it hasn't yet
//! (a new kernel's first start: a few seconds; `olo_kmod` has how and
//! where, shared with Olo's other kernel modules), and loads it for the
//! one device xe has (the one the computer started on, where it has
//! several).
//!
//! Only into a kernel build it was checked on. The module reads a
//! function's arguments where one build of the kernel keeps them, and
//! nothing a module can do at load says where another build does; so
//! each build is looked at and tried before it is listed
//! (kernel/gpu-pages/QUALIFIED: the kernel's build ID and its TTM
//! module's, as `tools/dev/gpu-pages-check --qualify` prints them), and
//! a kernel that isn't listed, as a new one an update brings isn't until
//! an Olo that has checked it, is left as it comes: said, not a failure.
//! Then the module checks itself as it loads and stays out of a kernel
//! whose pool isn't the one it was written for (its own words are in the
//! kernel's log, and said on here); where it can't be built or loaded
//! this says why and ends failed, which Olo Doctor shows. Nothing else
//! is touched, and the kernel is as it was. Where the computer's
//! graphics aren't on xe there is nothing for it to do, and it says so.
//!
//! `olo-gpu-pages --status` says what there is, as anyone, in a line of
//! JSON; `--remove` (root) takes the module out of the running kernel
//! again.

use olo_kmod::{Kernel, Module, Places, Running, read, release};
use std::process::ExitCode;

/// The module: its name (and its folder's in /sys/module), and its
/// source, carried in this program.
const NAME: &str = "olo_gpu_pages";
const SOURCE: &str = include_str!("../../../kernel/gpu-pages/olo_gpu_pages.c");
const KBUILD: &str = include_str!("../../../kernel/gpu-pages/Kbuild");
const MODULE: Module = Module { name: NAME, state: "olo-gpu-pages", files: &[("Kbuild", KBUILD), ("olo_gpu_pages.c", SOURCE)] };

/// One of the module's parameters as the kernel shows it: `on` (Y or
/// N), `changed`, `missed`.
fn parameter(places: &Places, name: &str) -> Option<String> {
    read(&places.loaded(&MODULE).join("parameters").join(name)).map(|v| v.trim().to_owned())
}

/// The module is in the running kernel and switched on.
fn in_force(places: &Places) -> bool {
    parameter(places, "on").as_deref() == Some("Y")
}

/// The kernel builds the module was checked on: release, the kernel's
/// build ID, its TTM module's.
const QUALIFIED: &str = include_str!("../../../kernel/gpu-pages/QUALIFIED");

/// The PCI device xe has, as the module is to be told it
/// ("0000:00:02.0"): the one the computer started on where xe has
/// several, else the first. None: no device on xe.
fn xe(places: &Places) -> Option<String> {
    let driver = places.root.join("sys/bus/pci/drivers/xe");
    let mut devices: Vec<String> = std::fs::read_dir(&driver)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        // (A device's address among the driver's own files: "bind", "module"…)
        .filter(|name| name.len() >= 12 && name.split([':', '.']).count() == 4 && name.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.'))
        .collect();
    devices.sort();
    let started_on = |name: &String| read(&driver.join(name).join("boot_vga")).is_some_and(|v| v.trim() == "1");
    devices.iter().find(|d| started_on(d)).or(devices.first()).cloned()
}

/// The list's line for the running build of the kernel `release`: what
/// was checked beside it (the live patch's build ID). None: not listed.
fn checked_with(places: &Places, release: &str) -> Option<Vec<&'static str>> {
    olo_kmod::builds::running(&places.root, "ttm").and_then(|b| olo_kmod::builds::entry(QUALIFIED, release, &b))
}

/// The module was checked on the running build of the kernel `release`
/// (`QUALIFIED`).
fn checked_on_this_build(places: &Places, release: &str) -> bool {
    checked_with(places, release).is_some()
}

/// The kernel's live patches are as they were when the build was
/// checked: none at all, or Olo's own `olo_gpu_memory` alone, settled,
/// patching the one function of TTM's it patches, and the very build of
/// it that was beside the module then (`reviewed`: its build ID, the
/// list's fourth word). Anything else has changed the kernel under the
/// list's feet, though its own notes read the same: left as it comes.
fn reviewed_patches(places: &Places, reviewed: Option<&str>) -> bool {
    let folder = places.root.join("sys/kernel/livepatch");
    let Ok(entries) = std::fs::read_dir(&folder) else { return false };
    entries.into_iter().all(|entry| {
        let Ok(entry) = entry else { return false };
        let path = entry.path();
        entry.file_name() == "olo_gpu_memory"
            && read(&path.join("transition")).is_some_and(|v| v.trim() == "0")
            && read(&path.join("enabled")).is_some_and(|v| v.trim() == "1")
            && read(&path.join("ttm/patched")).is_some_and(|v| v.trim() == "1")
            && path.join("ttm/ttm_backup_bytes_avail,1").is_dir()
            && reviewed.is_some_and(|reviewed| olo_kmod::builds::running(&places.root, "olo_gpu_memory").is_some_and(|(_, id)| id == reviewed))
    })
}

/// The kernel as the module was checked on it: this build, with the
/// live patches as they were then.
fn as_checked(places: &Places, release: &str) -> bool {
    checked_with(places, release).is_some_and(|with| reviewed_patches(places, with.first().copied()))
}

/// The name this service gave its last load of the module, kept where
/// its next run reads it.
fn owner_file(places: &Places) -> std::path::PathBuf {
    places.root.join("run/olo-gpu-pages/owner")
}

/// The module in the kernel is this service's own load (a trial's, or
/// one put in by hand, is not: left alone).
fn owned(places: &Places) -> bool {
    let Some(owner) = read(&owner_file(places)) else { return false };
    !owner.trim().is_empty() && parameter(places, "service_id").as_deref() == Some(owner.trim())
}

/// The module switched on or off, and read back.
fn switch(places: &Places, enabled: bool) -> Result<(), String> {
    let path = places.loaded(&MODULE).join("parameters/on");
    let value = if enabled { "Y" } else { "N" };
    std::fs::write(&path, value).map_err(|e| format!("{}: {e}", path.display()))?;
    if parameter(places, "on").as_deref() != Some(value) {
        return Err("the module's switch didn't read back as it was written".into());
    }
    Ok(())
}

/// The module is built for `release` from this program's source, and
/// installed.
fn built(places: &Places, release: &str) -> bool {
    olo_kmod::built(places, &MODULE, release)
}

/// The module's own last words in the kernel's log (it says why it
/// stays out of a kernel it isn't sure of): root's to read.
fn last_words(places: &Places) -> Option<String> {
    use std::{io::Read as _, os::unix::fs::OpenOptionsExt as _};
    // (Each read gives one record, "level,sequence,time,flags;text";
    // not waiting for the next once the last is read.)
    let mut log = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(places.root.join("dev/kmsg")).ok()?;
    let (mut last, mut record) = (None, vec![0u8; 8192]);
    // (A test's log is a file: all of it at once, a record a line.)
    while let Ok(n) = log.read(&mut record) {
        if n == 0 {
            break;
        }
        for line in String::from_utf8_lossy(&record[..n]).lines() {
            if let Some(words) = line.split_once(';').and_then(|(_, text)| text.strip_prefix("olo_gpu_pages: ")) {
                // (Every refusal ends by saying it isn't loading, which
                // whoever reads this already knows.)
                last = Some(words.trim().trim_end_matches(": not loading").to_owned());
            }
        }
    }
    last
}

/// What making sure of the module came to.
#[derive(Debug, PartialEq)]
enum Done {
    /// In the kernel already.
    Already,
    Loaded,
    BuiltAndLoaded,
    /// No xe driver here: nothing for it to do.
    NotNeeded,
    /// This kernel's build isn't one the module was checked on: left as
    /// it comes.
    NotChecked,
}

/// The module in the kernel `release` and in force: built where it
/// isn't (or was from another source, or for another build of the
/// kernel), loaded switched off under a name of this service's own, and
/// switched on once it is seen to be that load, for the device, in a
/// kernel that is still as it was checked. Each step from the third
/// review with Astra (2026-11-21): the service had switched on a module
/// it never loaded, on a build that wasn't listed.
fn ensure(places: &Places, release: &str, kernel: &mut dyn Kernel) -> Result<Done, String> {
    let loaded = places.loaded(&MODULE).is_dir();
    // (In already, and not this service's load: a trial's, or put in by
    // hand. Not ours to switch, to count as in force, or to remove.)
    if loaded && !owned(places) {
        return Err("the module is in the kernel already and isn't this service's load (a trial's, or put in by hand): left alone".into());
    }
    let device = xe(places);
    let ids = olo_kmod::builds::running(&places.root, "ttm");
    let with = ids.as_ref().and_then(|ids| olo_kmod::builds::entry(QUALIFIED, release, ids));
    let reviewed = with.as_ref().and_then(|with| with.first().copied());
    let qualified = with.is_some() && reviewed_patches(places, reviewed);
    // (No device on xe, or a kernel that isn't as it was checked: what
    // this service loaded before, into another, comes out.)
    let (Some(device), Some((k, t))) = (device.filter(|_| qualified), ids) else {
        if loaded {
            remove(places, kernel)?;
        }
        return Ok(if xe(places).is_some() { Done::NotChecked } else { Done::NotNeeded });
    };
    // (Built for this very build: a kernel of the same name rebuilt
    // doesn't take the last one's module on its release's word.)
    let stamp = places.work(&MODULE, release).join("qualified-build");
    let identity = format!("{k} {t}\n");
    let had = built(places, release) && read(&stamp).as_deref() == Some(identity.as_str());
    if loaded {
        if had && parameter(places, "bdf").as_deref() == Some(device.as_str()) && in_force(places) {
            return Ok(Done::Already);
        }
        // (Ours, and for another device, from another build, or switched
        // off since: out, and in afresh.)
        remove(places, kernel)?;
    }
    let make = |kernel: &mut dyn Kernel| {
        olo_kmod::build(places, &MODULE, release, kernel)?;
        std::fs::write(&stamp, &identity).map_err(|e| e.to_string())
    };
    if !had {
        make(kernel)?;
    }
    // A name for this load, kept before it is made: what is in the
    // kernel after is ours only if it carries it.
    const NO_NAME: &str = "the kernel gave no name for this load (/proc/sys/kernel/random/uuid)";
    let owner = read(&places.root.join("proc/sys/kernel/random/uuid")).ok_or(NO_NAME)?;
    let owner = owner.trim();
    if owner.is_empty() {
        return Err(NO_NAME.into());
    }
    let record = owner_file(places);
    std::fs::create_dir_all(record.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&record, owner).map_err(|e| e.to_string())?;
    // (Loaded switched off, and switched on only once it is seen to be
    // ours, for this device, in a kernel still as it was checked.)
    let parameters = format!("bdf={device} on=0 service_id={owner}");
    let same_target = || {
        checked_on_this_build(places, release)
            && olo_kmod::builds::running(&places.root, "ttm") == Some((k.clone(), t.clone()))
            && reviewed_patches(places, reviewed)
            && xe(places).as_deref() == Some(device.as_str())
    };
    if !same_target() {
        return Ok(Done::NotChecked);
    }
    // (What the module says of a kernel it stays out of is the reason;
    // the kernel's own word for it, "Operation not supported", isn't.)
    let refused = |e: String| match last_words(places) {
        Some(words) => format!("{e}; its own words: {words}"),
        None => e,
    };
    if let Err(e) = kernel.load_with(&places.module(&MODULE, release), &parameters) {
        // One built for a kernel of the same name that has since changed
        // (the package rebuilt): once more, from scratch.
        if !had {
            return Err(refused(e));
        }
        make(kernel)?;
        if !same_target() {
            return Ok(Done::NotChecked);
        }
        kernel.load_with(&places.module(&MODULE, release), &parameters).map_err(refused)?;
    }
    // (The kernel says a module of the name is in whoever loaded it.)
    if !owned(places) || parameter(places, "service_id").as_deref() != Some(owner) {
        return Err("another load of the module got into the kernel first: left alone".into());
    }
    if !same_target() || parameter(places, "bdf").as_deref() != Some(device.as_str()) || parameter(places, "on").as_deref() != Some("N") {
        remove(places, kernel)?;
        return Err("the kernel's build, its live patches or the device changed as the module loaded: taken out again".into());
    }
    if let Err(e) = switch(places, true) {
        remove(places, kernel)?;
        return Err(e);
    }
    olo_kmod::tidy(places, &MODULE, release);
    Ok(if had { Done::Loaded } else { Done::BuiltAndLoaded })
}

/// How it went, written where Olo Doctor reads it: "in force", "not
/// needed", "not checked" (this kernel's build), or "failed: " and why.
fn say(places: &Places, went: &Result<Done, String>) {
    olo_kmod::say(
        places,
        &MODULE,
        &match went {
            Ok(Done::NotNeeded) => "not needed".to_owned(),
            Ok(Done::NotChecked) => "not checked".to_owned(),
            Ok(_) => "in force".to_owned(),
            Err(e) => format!("failed: {e}"),
        },
    );
}

/// What there is, in a line of JSON.
fn status(places: &Places, release: &str) -> String {
    let number = |name: &str| parameter(places, name).and_then(|v| v.parse::<u64>().ok()).map_or("null".to_owned(), |n| n.to_string());
    let quoted = |text: Option<String>| text.map_or("null".to_owned(), |t| format!("\"{t}\""));
    let builds = olo_kmod::builds::running(&places.root, "ttm");
    format!(
        "{{\"kernel\":\"{release}\",\"loaded\":{},\"ours\":{},\"in_force\":{},\"for_device\":{},\"changed\":{},\"missed\":{},\"built\":{},\"headers\":{},\"device\":{},\"checked_on_this_build\":{},\"live_patches_as_checked\":{},\"kernel_build\":{},\"ttm_build\":{}}}",
        places.loaded(&MODULE).is_dir(),
        owned(places),
        in_force(places),
        quoted(parameter(places, "bdf")),
        number("changed"),
        number("missed"),
        built(places, release),
        places.headers(release).join("Makefile").is_file(),
        quoted(xe(places)),
        checked_on_this_build(places, release),
        as_checked(places, release),
        quoted(builds.as_ref().map(|b| b.0.clone())),
        quoted(builds.map(|b| b.1)),
    )
}

/// The module taken out of the running kernel: this service's own load
/// only, switched off first.
fn remove(places: &Places, kernel: &mut dyn Kernel) -> Result<(), String> {
    if !places.loaded(&MODULE).is_dir() {
        return Ok(());
    }
    if !owned(places) {
        return Err("the module in the kernel isn't this service's load (a trial's, or put in by hand): left alone".into());
    }
    // (Off first, and read back: should the kernel then not let it go,
    // what stays changes nothing.)
    switch(places, false)?;
    match kernel.unload(NAME) {
        Ok(true) => Ok(()),
        Ok(false) => Err("the module is switched off, and still loaded: in use".into()),
        Err(e) => Err(e),
    }
}

fn main() -> ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (places, release) = (Places::here(), release());
    // One at a time on the module: a start, a removal, a trial
    // (tools/dev/gpu-pages-try takes the same lock), run by systemd or
    // by hand.
    let _lock = if args.is_empty() || args.as_slice() == ["--remove"] {
        let path = places.root.join("run/olo-gpu-pages.lifecycle.lock");
        match std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(path).and_then(|f| {
            f.try_lock().map_err(std::io::Error::from)?;
            Ok(f)
        }) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("olo-gpu-pages: something else is at work on the module now (another olo-gpu-pages, or a trial): {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {
            let went = ensure(&places, &release, &mut Running);
            say(&places, &went);
            match went {
                Ok(Done::NotNeeded) => {
                    println!("olo-gpu-pages: this computer's graphics aren't on Intel's xe driver: nothing to do");
                    ExitCode::SUCCESS
                }
                Ok(Done::NotChecked) => {
                    println!(
                        "olo-gpu-pages: Olo's module hasn't been checked on this kernel's build ({release}) with the live patches now in it: the kernel is left as it comes, and programs can be held for large pieces of graphics memory"
                    );
                    ExitCode::SUCCESS
                }
                Ok(done) => {
                    let how = match done {
                        Done::Already => "the module was in already".to_owned(),
                        Done::Loaded => format!("the module loaded for {release}"),
                        _ => format!("the module built for {release} and loaded"),
                    };
                    println!("olo-gpu-pages: no program waits for a large piece of graphics memory ({how})");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("olo-gpu-pages: programs can still be held for large pieces of graphics memory: {e}");
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
                println!("olo-gpu-pages: the module is out of the running kernel");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-gpu-pages: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            println!(
                "usage: olo-gpu-pages [--status | --remove]\n  (none)    build Olo's module for the running kernel where it isn't yet, and load it (root; systemd's olo-gpu-pages.service)\n  --status  what there is, in a line of JSON (anyone)\n  --remove  take the module out of the running kernel (root)"
            );
            ExitCode::SUCCESS
        }
    }
}

/// Where a test's tree has the module's entry once it's "loaded".
#[cfg(test)]
fn entry(places: &Places) -> std::path::PathBuf {
    places.loaded(&MODULE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// The first build the module was checked on (`QUALIFIED`'s).
    const K: &str = "7.3.4-300.fc45.x86_64";
    const KERNEL_BUILD: &str = "7f111d5ff77cd8b0de0b41422fe4d4f7d375f54d";
    const TTM_BUILD: &str = "e0fdead3891ad14b87586bbe3bef4fe33c1d65cc";

    use olo_kmod::builds::notes_for_tests as notes;

    /// A computer in a folder, removed when the test is over: one
    /// device on xe, and the kernel build the module was checked on.
    struct Tree(Places);

    impl Tree {
        fn new(name: &str) -> Tree {
            let root = PathBuf::from(format!("/var/tmp/olo-gpu-pages-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let tree = Tree(Places { root });
            std::fs::create_dir_all(tree.0.root.join("dev")).unwrap();
            std::fs::create_dir_all(tree.0.root.join("sys/kernel/livepatch")).unwrap();
            tree.put("proc/sys/kernel/random/uuid", b"fixture-owner");
            tree.put("sys/bus/pci/drivers/xe/0000:00:02.0/boot_vga", b"1\n");
            tree.put("sys/bus/pci/drivers/xe/bind", b"");
            tree.put("sys/bus/pci/drivers/xe/module/refcnt", b"3\n");
            tree.put("sys/kernel/notes", &notes(KERNEL_BUILD));
            tree.put("sys/module/ttm/notes/.note.gnu.build-id", &notes(TTM_BUILD));
            tree
        }

        fn put(&self, path: &str, contents: &[u8]) {
            let path = self.0.root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
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
    /// by listing it as its modules are, switched on; or refuses it,
    /// with the module's words in its log.
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
            self.built += 1;
            std::fs::write(dir.join(format!("{NAME}.ko")), "built").map_err(|e| e.to_string())
        }

        fn load(&mut self, _: &Path) -> Result<(), String> {
            unreachable!("it is always told its device")
        }

        fn load_with(&mut self, module: &Path, parameters: &str) -> Result<(), String> {
            assert!(module.is_file());
            assert_eq!(parameters, "bdf=0000:00:02.0 on=0 service_id=fixture-owner");
            if self.refuses > 0 {
                self.refuses -= 1;
                return Err("the kernel didn't take the module: Operation not supported (os error 95)".into());
            }
            self.loaded += 1;
            let parameters = entry(&self.places).join("parameters");
            std::fs::create_dir_all(&parameters).unwrap();
            for (name, value) in [("on", "N\n"), ("bdf", "0000:00:02.0"), ("service_id", "fixture-owner"), ("changed", "0\n"), ("missed", "0\n")] {
                std::fs::write(parameters.join(name), value).unwrap();
            }
            Ok(())
        }

        fn unload(&mut self, name: &str) -> Result<bool, String> {
            assert_eq!(name, NAME);
            std::fs::remove_dir_all(entry(&self.places)).unwrap();
            Ok(true)
        }
    }

    fn fake(tree: &Tree) -> Fake {
        Fake { places: tree.0.clone(), built: 0, loaded: 0, refuses: 0 }
    }

    /// A kernel's first start with Olo: the module built for it, kept
    /// and loaded; at its next, only loaded; in already, nothing done
    /// (but switched on again if someone had switched it off).
    #[test]
    fn built_once_a_kernel_and_loaded_at_each_start() {
        let tree = Tree::new("built-once");
        tree.headers(K);
        let mut kernel = fake(&tree);
        assert!(!in_force(&tree.0) && !built(&tree.0, K));
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::BuiltAndLoaded));
        assert!(in_force(&tree.0) && built(&tree.0, K));
        assert_eq!((kernel.built, kernel.loaded), (1, 1));
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::Already));
        assert_eq!((kernel.built, kernel.loaded), (1, 1));
        std::fs::write(entry(&tree.0).join("parameters/on"), "N\n").unwrap();
        assert!(!in_force(&tree.0));
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::Loaded));
        assert_eq!(read(&entry(&tree.0).join("parameters/on")).as_deref(), Some("Y"), "read back after a fresh load");
        // The computer's next start: the kernel as it was, the build kept.
        std::fs::remove_dir_all(entry(&tree.0)).unwrap();
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::Loaded));
        assert_eq!((kernel.built, kernel.loaded), (1, 3));
        std::fs::write(entry(&tree.0).join("parameters/changed"), "1234\n").unwrap();
        assert_eq!(
            status(&tree.0, K),
            format!(
                "{{\"kernel\":\"{K}\",\"loaded\":true,\"ours\":true,\"in_force\":true,\"for_device\":\"0000:00:02.0\",\"changed\":1234,\"missed\":0,\"built\":true,\"headers\":true,\"device\":\"0000:00:02.0\",\"checked_on_this_build\":true,\"live_patches_as_checked\":true,\"kernel_build\":\"{KERNEL_BUILD}\",\"ttm_build\":\"{TTM_BUILD}\"}}"
            )
        );
        say(&tree.0, &Ok(Done::Loaded));
        assert_eq!(read(&tree.0.said(&MODULE)).as_deref(), Some("in force\n"));
        assert!(tree.0.said(&MODULE).ends_with("run/olo-gpu-pages/status"));
    }

    /// Graphics on another driver: nothing built, nothing loaded, and
    /// said so.
    #[test]
    fn nothing_to_do_without_xe() {
        let tree = Tree::new("no-xe");
        tree.headers(K);
        std::fs::remove_dir_all(tree.0.root.join("sys/bus/pci/drivers/xe")).unwrap();
        let mut kernel = fake(&tree);
        let went = ensure(&tree.0, K, &mut kernel);
        assert_eq!(went, Ok(Done::NotNeeded));
        assert_eq!((kernel.built, kernel.loaded), (0, 0));
        say(&tree.0, &went);
        assert_eq!(read(&tree.0.said(&MODULE)).as_deref(), Some("not needed\n"));
        assert!(status(&tree.0, K).contains("\"loaded\":false,\"ours\":false,\"in_force\":false,\"for_device\":null,\"changed\":null,\"missed\":null,\"built\":false,\"headers\":true,\"device\":null,\"checked_on_this_build\":true,\"live_patches_as_checked\":true"));
    }

    /// A kernel build the module wasn't checked on (the release of one
    /// that was, rebuilt; another release; a kernel that doesn't say
    /// what build it is): nothing built, nothing loaded, said, and no
    /// failure. The list's own lines are read as they are written.
    #[test]
    fn a_build_it_wasnt_checked_on_is_left_as_it_comes() {
        let tree = Tree::new("not-checked");
        tree.headers(K);
        const NEXT: &str = "7.3.5-300.fc45.x86_64";
        tree.headers(NEXT);
        let mut kernel = fake(&tree);
        let went = ensure(&tree.0, NEXT, &mut kernel);
        assert_eq!(went, Ok(Done::NotChecked));
        say(&tree.0, &went);
        assert_eq!(read(&tree.0.said(&MODULE)).as_deref(), Some("not checked\n"));
        tree.put("sys/module/ttm/notes/.note.gnu.build-id", &notes("00fdead3891ad14b87586bbe3bef4fe33c1d65cc"));
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::NotChecked));
        assert!(
            status(&tree.0, K).contains("\"checked_on_this_build\":false,\"live_patches_as_checked\":false,\"kernel_build\":\"7f111d5ff77cd8b0de0b41422fe4d4f7d375f54d\",\"ttm_build\":\"00fdead3")
        );
        std::fs::remove_file(tree.0.root.join("sys/module/ttm/notes/.note.gnu.build-id")).unwrap();
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::NotChecked));
        assert!(status(&tree.0, K).contains("\"kernel_build\":null,\"ttm_build\":null"));
        assert_eq!((kernel.built, kernel.loaded), (0, 0));
        // The list itself: a release and two build IDs a line, and at most the live patch's after them; and this build is on it.
        assert!(QUALIFIED.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).all(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (3..=4).contains(&w.len()) && w[1..].iter().all(|id| id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        }));
        assert!(olo_kmod::builds::listed(QUALIFIED, K, &(KERNEL_BUILD.to_owned(), TTM_BUILD.to_owned())));
    }

    /// A module in the kernel that this service didn't load (a trial's,
    /// one put in by hand) is not its to switch, to call its own, or to
    /// remove: the third review's finding, on an unlisted build with no
    /// device, a module switched off was switched on. And its own load,
    /// found in a kernel that isn't as it was checked, comes out.
    #[test]
    fn a_load_that_isnt_this_services_is_left_alone() {
        let tree = Tree::new("not-ours");
        tree.headers(K);
        let mut kernel = fake(&tree);
        for (name, value) in [("on", "N\n"), ("bdf", "0000:00:02.0"), ("trial_id", "a-trial"), ("service_id", "")] {
            tree.put(&format!("sys/module/olo_gpu_pages/parameters/{name}"), value.as_bytes());
        }
        // On a listed build, with the device: left alone.
        let e = ensure(&tree.0, K, &mut kernel).unwrap_err();
        assert!(e.contains("isn't this service's load") && e.ends_with("left alone"), "{e}");
        // On an unlisted build with no device: the same, and not NotChecked with a switch thrown.
        std::fs::remove_dir_all(tree.0.root.join("sys/bus/pci/drivers/xe")).unwrap();
        assert!(ensure(&tree.0, "7.3.5-300.fc45.x86_64", &mut kernel).is_err());
        assert!(remove(&tree.0, &mut kernel).unwrap_err().ends_with("left alone"));
        assert_eq!(parameter(&tree.0, "on").as_deref(), Some("N"), "never switched");
        assert!(entry(&tree.0).is_dir() && (kernel.built, kernel.loaded) == (0, 0));
        assert!(status(&tree.0, K).contains("\"loaded\":true,\"ours\":false,\"in_force\":false"));
        // An owner's name on file that isn't the module's: still not ours.
        tree.put("run/olo-gpu-pages/owner", b"fixture-owner");
        assert!(ensure(&tree.0, K, &mut kernel).is_err());
        // Its own load, the kernel no longer as checked (another build of its ttm): out.
        let tree = Tree::new("ours-unlisted");
        tree.headers(K);
        let mut kernel = fake(&tree);
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::BuiltAndLoaded));
        assert!(in_force(&tree.0) && owned(&tree.0));
        tree.put("sys/module/ttm/notes/.note.gnu.build-id", &notes("00fdead3891ad14b87586bbe3bef4fe33c1d65cc"));
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::NotChecked));
        assert!(!entry(&tree.0).exists(), "taken out");
        // Built for the other build of the same name, the module isn't taken on its release's word: built again.
        tree.put("sys/module/ttm/notes/.note.gnu.build-id", &notes(TTM_BUILD));
        std::fs::write(tree.0.work(&MODULE, K).join("qualified-build"), "another build\n").unwrap();
        let before = kernel.built;
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::BuiltAndLoaded));
        assert_eq!(kernel.built, before + 1);
    }

    /// The kernel's live patches as they were when the build was
    /// checked: none, or Olo's own alone, settled, of the very build
    /// the list names. Any other, another build of it, or one still
    /// settling: the kernel is left as it comes.
    #[test]
    fn the_live_patches_are_as_they_were_checked_or_it_stays_out() {
        const REVIEWED: &str = "5cb978ba9f9e8d27330f5f94950eed9fcb90efb7";
        assert!(QUALIFIED.lines().any(|l| l.split_whitespace().collect::<Vec<_>>() == [K, KERNEL_BUILD, TTM_BUILD, REVIEWED]), "the list's fourth word");
        let tree = Tree::new("live-patches");
        tree.headers(K);
        let mut kernel = fake(&tree);
        let patch = |name: &str, build: &str, transition: &str| {
            tree.put(&format!("sys/kernel/livepatch/{name}/transition"), transition.as_bytes());
            tree.put(&format!("sys/kernel/livepatch/{name}/enabled"), b"1\n");
            tree.put(&format!("sys/kernel/livepatch/{name}/ttm/patched"), b"1\n");
            tree.put(&format!("sys/kernel/livepatch/{name}/ttm/ttm_backup_bytes_avail,1/.keep"), b"");
            tree.put(&format!("sys/module/{name}/notes/.note.gnu.build-id"), &notes(build));
        };
        // Olo's own, as checked: in.
        patch("olo_gpu_memory", REVIEWED, "0\n");
        assert!(as_checked(&tree.0, K));
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::BuiltAndLoaded));
        // Still settling: out again (and what was loaded comes out).
        patch("olo_gpu_memory", REVIEWED, "1\n");
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::NotChecked));
        assert!(!entry(&tree.0).exists() && !as_checked(&tree.0, K));
        assert!(status(&tree.0, K).contains("\"checked_on_this_build\":true,\"live_patches_as_checked\":false"));
        // Another build of it.
        patch("olo_gpu_memory", "00b978ba9f9e8d27330f5f94950eed9fcb90efb7", "0\n");
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::NotChecked));
        // Another patch beside it.
        patch("olo_gpu_memory", REVIEWED, "0\n");
        patch("someone_elses", REVIEWED, "0\n");
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::NotChecked));
        std::fs::remove_dir_all(tree.0.root.join("sys/kernel/livepatch/someone_elses")).unwrap();
        // A line that names no live patch allows none.
        assert!(reviewed_patches(&tree.0, Some(REVIEWED)) && !reviewed_patches(&tree.0, None));
        std::fs::remove_dir_all(tree.0.root.join("sys/kernel/livepatch/olo_gpu_memory")).unwrap();
        assert!(reviewed_patches(&tree.0, None) && reviewed_patches(&tree.0, Some(REVIEWED)));
        // A kernel without live patching at all (no such folder) isn't one it was checked on.
        std::fs::remove_dir_all(tree.0.root.join("sys/kernel/livepatch")).unwrap();
        assert!(!reviewed_patches(&tree.0, None));
    }

    /// The device the module is told: the one the computer started on
    /// where xe has several, the first where none says so; the driver's
    /// own files are not devices.
    #[test]
    fn the_device_is_the_one_the_computer_started_on() {
        let tree = Tree::new("device");
        assert_eq!(xe(&tree.0).as_deref(), Some("0000:00:02.0"));
        tree.put("sys/bus/pci/drivers/xe/0000:03:00.0/boot_vga", b"0\n");
        tree.put("sys/bus/pci/drivers/xe/0000:00:01.0/class", b"0x038000\n");
        assert_eq!(xe(&tree.0).as_deref(), Some("0000:00:02.0"));
        tree.put("sys/bus/pci/drivers/xe/0000:00:02.0/boot_vga", b"0\n");
        assert_eq!(xe(&tree.0).as_deref(), Some("0000:00:01.0"));
        tree.put("sys/bus/pci/drivers/xe/0000:03:00.0/boot_vga", b"1\n");
        assert_eq!(xe(&tree.0).as_deref(), Some("0000:03:00.0"));
    }

    /// A kernel the module stays out of (its own check): said with the
    /// module's own words from the kernel's log, after one more build;
    /// and what else it can't do.
    #[test]
    fn what_it_cant_do_is_said_in_the_modules_own_words() {
        let tree = Tree::new("cant");
        let mut kernel = fake(&tree);
        let e = ensure(&tree.0, K, &mut kernel).unwrap_err();
        assert_eq!(e, format!("this kernel's headers aren't installed (the kernel-devel package for {K})"));
        tree.headers(K);
        std::fs::write(
            tree.0.root.join("dev/kmsg"),
            "6,812,5112233,-;xe 0000:00:02.0: [drm] something else\n3,813,5112300,-;olo_gpu_pages: an older line\n6,814,5112400,-;olo_gpu_pages: this kernel's pool doesn't wait for 2 MB pieces (flags 0x3868c2): nothing to change, not loading\n6,815,5112500,-;audit: more\n",
        )
        .unwrap();
        kernel.refuses = 1;
        let e = ensure(&tree.0, K, &mut kernel).unwrap_err();
        assert_eq!(
            e,
            "the kernel didn't take the module: Operation not supported (os error 95); its own words: this kernel's pool doesn't wait for 2 MB pieces (flags 0x3868c2): nothing to change, not loading"
        );
        assert!(!in_force(&tree.0));
        say(&tree.0, &Err(e));
        assert!(read(&tree.0.said(&MODULE)).unwrap().starts_with("failed: the kernel didn't take the module: Operation not supported"));
        // Built from this source and refused: built once more, then said.
        kernel.refuses = 2;
        let before = kernel.built;
        assert!(ensure(&tree.0, K, &mut kernel).is_err());
        assert_eq!(kernel.built, before + 1);
        // No log to read (not root): the kernel's word alone.
        std::fs::remove_file(tree.0.root.join("dev/kmsg")).unwrap();
        kernel.refuses = 2;
        assert_eq!(ensure(&tree.0, K, &mut kernel).unwrap_err(), "the kernel didn't take the module: Operation not supported (os error 95)");
    }

    /// Taken out again.
    #[test]
    fn taken_out_again() {
        let tree = Tree::new("remove");
        tree.headers(K);
        let mut kernel = fake(&tree);
        assert_eq!(remove(&tree.0, &mut kernel), Ok(()), "not in: nothing to do");
        assert_eq!(ensure(&tree.0, K, &mut kernel), Ok(Done::BuiltAndLoaded));
        assert_eq!(remove(&tree.0, &mut kernel), Ok(()));
        assert!(!in_force(&tree.0) && !entry(&tree.0).exists());
    }

    /// The module's source is what this program takes it to be: a probe
    /// at the pool's one function, a switch and two counts, under the
    /// kernel's licence, and nothing of the kernel's replaced.
    #[test]
    fn the_module_is_a_probe_at_the_pools_function() {
        assert!(SOURCE.contains(".symbol_name = \"ttm:ttm_pool_alloc_page\"") && SOURCE.contains("module_param_cb(on, &on_ops, &on, 0644)") && SOURCE.contains("module_param(bdf, charp, 0444)"));
        assert!(SOURCE.contains("module_param_cb(changed,") && SOURCE.contains("module_param_cb(missed,"));
        assert!(!SOURCE.contains("livepatch") || !SOURCE.contains("klp_enable_patch"));
        assert!(SOURCE.contains("MODULE_LICENSE(\"GPL\")") && SOURCE.starts_with("// SPDX-License-Identifier: GPL-2.0\n"));
        assert_eq!(KBUILD, format!("obj-m := {NAME}.o\n"));
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
        assert!(tree.0.module(&MODULE, &release).ends_with(format!("lib/modules/{release}/extra/olo/{NAME}.ko")));
        let module = std::fs::read(tree.0.module(&MODULE, &release)).unwrap();
        let has = |what: &str| module.windows(what.len()).any(|w| w == what.as_bytes());
        assert!(has(&format!("vermagic={release} ")) && has("ttm_pool_alloc_page") && has("depends=ttm"));
        assert!(built(&tree.0, &release));
    }

    /// The real build for another kernel than the running one: the one
    /// an update brings, before it's installed (`--ignored`, with
    /// `OLO_NEXT_KERNEL=RELEASE=HEADERS`: that kernel's kernel-devel
    /// unpacked somewhere). Whether it then loads is the module's own
    /// check's to say, in that kernel (`tools/dev/gpu-pages-check`).
    #[test]
    #[ignore]
    fn builds_for_the_kernel_an_update_brings() {
        let next = std::env::var("OLO_NEXT_KERNEL").expect("OLO_NEXT_KERNEL=RELEASE=HEADERS");
        let (release, headers) = next.split_once('=').expect("RELEASE=HEADERS");
        assert_ne!(release, olo_kmod::release(), "another kernel than the running one");
        let tree = Tree::new("next-build");
        std::fs::create_dir_all(tree.0.kernels().join(release)).unwrap();
        std::os::unix::fs::symlink(std::fs::canonicalize(headers).unwrap(), tree.0.headers(release)).unwrap();
        olo_kmod::build(&tree.0, &MODULE, release, &mut Running).unwrap();
        let module = std::fs::read(tree.0.module(&MODULE, release)).unwrap();
        let has = |what: &str| module.windows(what.len()).any(|w| w == what.as_bytes());
        assert!(has(&format!("vermagic={release} ")) && has("ttm_pool_alloc_page"));
    }
}
