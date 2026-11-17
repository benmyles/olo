//! olo-bbr3: BBRv3 for TCP.
//!
//! How fast TCP sends is decided by its congestion control. Linux's
//! default, CUBIC, fills the buffers along the path until a packet is
//! dropped, and its own "bbr" is BBRv1 (2016). BBRv3 (Google, 2023)
//! models the path instead (its bandwidth and its round-trip time) and
//! keeps the queues short: on a 50 Mbit/s link with a 40 ms round trip
//! it sends as much as CUBIC with the round trip at 43 ms instead of 68
//! (kernel/bbr3/README.md has the measurements). It isn't in Linux:
//! Google's is a patched kernel. Olo carries it as a module of its own
//! for the kernel as it comes (kernel/bbr3, `tcp_bbr3`, the congestion
//! control "bbr3").
//!
//! A kernel module is built for one kernel. This program, as root at
//! every start (systemd's `olo-bbr3.service`, installed by Olo Doctor),
//! builds the module for the running kernel where it hasn't yet (a new
//! kernel's first start: a few seconds; `olo_kmod` has how and where),
//! loads it, and makes it the congestion control new connections get
//! (`net.ipv4.tcp_congestion_control`; what it was before is kept, for
//! `--remove`). Where the kernel's own "bbr" is BBRv3 already (a kernel
//! built with Google's patches), that one is chosen and no module is
//! built. Where it can't (no headers, Secure Boot refusing an unsigned
//! module, a kernel whose TCP has changed under the module) it says why
//! and ends failed, which Olo Doctor shows; connections go on with the
//! congestion control there was.
//!
//! `olo-bbr3 --status` says what there is, as anyone, in a line of JSON;
//! `--remove` (root) gives new connections back what they had before,
//! and unloads the module once no connection runs it.

use olo_kmod::{Kernel, Module, Places, Running, read, release};
use std::{path::PathBuf, process::ExitCode};

/// The congestion control's name, and the kernel's own BBR's.
const NAME: &str = "bbr3";
const KERNELS: &str = "bbr";
const MODULE: Module = Module {
    name: "tcp_bbr3",
    state: "olo-bbr3",
    files: &[
        ("Kbuild", include_str!("../../../kernel/bbr3/Kbuild")),
        ("tcp_bbr3.c", include_str!("../../../kernel/bbr3/tcp_bbr3.c")),
        ("bbr3_olo.h", include_str!("../../../kernel/bbr3/bbr3_olo.h")),
        ("bbr3_selfcheck.h", include_str!("../../../kernel/bbr3/bbr3_selfcheck.h")),
    ],
};

/// The congestion control new connections get, and those there are.
fn default_file(places: &Places) -> PathBuf {
    places.root.join("proc/sys/net/ipv4/tcp_congestion_control")
}

fn default(places: &Places) -> Option<String> {
    read(&default_file(places)).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn available(places: &Places) -> Vec<String> {
    read(&places.root.join("proc/sys/net/ipv4/tcp_available_congestion_control")).map(|s| s.split_whitespace().map(str::to_owned).collect()).unwrap_or_default()
}

/// What new connections had before Olo chose for them, kept across
/// starts.
fn was_file(places: &Places) -> PathBuf {
    places.works(&MODULE).join("was")
}

/// Make `name` what new connections get, remembering what they had.
fn choose(places: &Places, name: &str) -> Result<(), String> {
    let now = default(places);
    if now.as_deref() == Some(name) {
        return Ok(());
    }
    if let Some(now) = now.filter(|n| n != NAME) {
        let was = was_file(places);
        if let Some(dir) = was.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(was, format!("{now}\n"));
    }
    let file = default_file(places);
    std::fs::write(&file, name).map_err(|e| format!("the kernel didn't take {name} as its congestion control ({}: {e})", file.display()))
}

/// What making sure of BBRv3 came to.
#[derive(Debug, PartialEq)]
enum Done {
    /// Chosen already, its module loaded.
    Already,
    /// The module was loaded; chosen.
    Chosen,
    Loaded,
    BuiltAndLoaded,
    /// The kernel's own "bbr" is BBRv3: chosen, no module.
    Kernels,
}

/// BBRv3 as what new connections get, in the kernel `release`: the
/// kernel's own if it is one (`kernels`: its "bbr"'s version, where it
/// has one loaded), else Olo's module, built where it isn't (or was from
/// another source) and loaded.
fn ensure(places: &Places, release: &str, kernel: &mut dyn Kernel, kernels: Option<u8>) -> Result<Done, String> {
    if kernels.is_some_and(|v| v >= 3) {
        choose(places, KERNELS)?;
        return Ok(Done::Kernels);
    }
    let loaded = places.loaded(&MODULE).is_dir();
    let had = olo_kmod::built(places, &MODULE, release);
    if !had {
        // (Built from another source and loaded: the new one is built
        // now, and is the one loaded from the next start. A module can't
        // be changed under the connections that run it.)
        olo_kmod::build(places, &MODULE, release, kernel)?;
    }
    if !loaded && let Err(e) = kernel.load(&places.module(&MODULE, release)) {
        // One built for a kernel of the same name that has since changed
        // (the package rebuilt): once more, from scratch.
        if !had {
            return Err(e);
        }
        olo_kmod::build(places, &MODULE, release, kernel)?;
        kernel.load(&places.module(&MODULE, release))?;
    }
    let chosen = default(places).as_deref() == Some(NAME);
    choose(places, NAME)?;
    olo_kmod::tidy(places, &MODULE, release);
    Ok(match (loaded, had, chosen) {
        (true, _, true) => Done::Already,
        (true, _, false) => Done::Chosen,
        (false, true, _) => Done::Loaded,
        (false, false, _) => Done::BuiltAndLoaded,
    })
}

/// How it went, written where Olo Doctor reads it: the congestion
/// control chosen ("bbr3", or "bbr" where the kernel's own is BBRv3), or
/// "failed: " and why.
fn say(places: &Places, went: &Result<Done, String>) {
    olo_kmod::say(
        places,
        &MODULE,
        &match went {
            Ok(Done::Kernels) => KERNELS.to_owned(),
            Ok(_) => NAME.to_owned(),
            Err(e) => format!("failed: {e}"),
        },
    );
}

/// What there is, in a line of JSON.
fn status(places: &Places, release: &str, kernels: Option<u8>) -> String {
    format!(
        "{{\"kernel\":\"{release}\",\"congestion_control\":\"{}\",\"available\":[{}],\"loaded\":{},\"built\":{},\"headers\":{},\"kernels_bbr\":{}}}",
        default(places).unwrap_or_default(),
        available(places).iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(","),
        places.loaded(&MODULE).is_dir(),
        olo_kmod::built(places, &MODULE, release),
        places.headers(release).join("Makefile").is_file(),
        kernels.map_or("null".to_owned(), |v| v.to_string()),
    )
}

/// New connections given back what they had before (CUBIC if that was
/// never noted), and the module unloaded if no connection runs it:
/// whether it is out.
fn remove(places: &Places, kernel: &mut dyn Kernel) -> Result<bool, String> {
    if default(places).as_deref() == Some(NAME) {
        let was = read(&was_file(places)).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty() && s != NAME).unwrap_or_else(|| "cubic".into());
        let file = default_file(places);
        std::fs::write(&file, &was).map_err(|e| format!("the kernel didn't take {was} back as its congestion control ({}: {e})", file.display()))?;
    }
    if !places.loaded(&MODULE).is_dir() {
        return Ok(true);
    }
    kernel.unload(MODULE.name)
}

fn main() -> ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (places, release) = (Places::here(), release());
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {
            let went = ensure(&places, &release, &mut Running, olo_net::tcp_info::kernels_bbr_version());
            say(&places, &went);
            let how = match &went {
                Ok(Done::Already) => "the module was loaded and chosen already".to_owned(),
                Ok(Done::Chosen) => "the module was loaded; chosen".to_owned(),
                Ok(Done::Loaded) => format!("the module loaded for {release}"),
                Ok(Done::BuiltAndLoaded) => format!("the module built for {release} and loaded"),
                Ok(Done::Kernels) => "this kernel's own bbr is BBRv3; chosen".to_owned(),
                Err(e) => {
                    eprintln!("olo-bbr3: TCP goes on with {}: {e}", default(&places).unwrap_or_else(|| "its congestion control".into()));
                    return ExitCode::FAILURE;
                }
            };
            println!("olo-bbr3: new TCP connections use BBRv3 ({how})");
            ExitCode::SUCCESS
        }
        ["--status"] => {
            println!("{}", status(&places, &release, olo_net::tcp_info::kernels_bbr_version()));
            ExitCode::SUCCESS
        }
        ["--remove"] => match remove(&places, &mut Running) {
            Ok(out) => {
                println!(
                    "olo-bbr3: new TCP connections use {} again; the module is {}",
                    default(&places).unwrap_or_default(),
                    if out { "out of the running kernel" } else { "loaded until the connections that run it end (or the next start)" }
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-bbr3: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            println!(
                "usage: olo-bbr3 [--status | --remove]\n  (none)    build Olo's BBRv3 module for the running kernel where it isn't yet, load it, and make it what new TCP connections use (root; systemd's olo-bbr3.service)\n  --status  what there is, a line of JSON (anyone)\n  --remove  give new connections back what they had, and unload the module (root)"
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
            let root = PathBuf::from(format!("/var/tmp/olo-bbr3-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let net = root.join("proc/sys/net/ipv4");
            std::fs::create_dir_all(&net).unwrap();
            std::fs::write(net.join("tcp_congestion_control"), "cubic\n").unwrap();
            std::fs::write(net.join("tcp_available_congestion_control"), "reno cubic\n").unwrap();
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
    /// by listing it and its congestion control.
    struct Fake {
        places: Places,
        built: usize,
        loaded: usize,
        refuses: usize,
        in_use: bool,
    }

    impl Fake {
        fn new(tree: &Tree) -> Fake {
            Fake { places: tree.0.clone(), built: 0, loaded: 0, refuses: 0, in_use: false }
        }
    }

    impl Kernel for Fake {
        fn build(&mut self, headers: &Path, dir: &Path) -> Result<(), String> {
            assert!(headers.join("Makefile").is_file());
            for (name, contents) in MODULE.files {
                assert_eq!(read(&dir.join(name)).as_deref(), Some(*contents));
            }
            self.built += 1;
            std::fs::write(dir.join("tcp_bbr3.ko"), "built").map_err(|e| e.to_string())
        }

        fn load(&mut self, module: &Path) -> Result<(), String> {
            assert!(module.is_file());
            if self.refuses > 0 {
                self.refuses -= 1;
                return Err("refused".into());
            }
            self.loaded += 1;
            std::fs::create_dir_all(self.places.loaded(&MODULE)).unwrap();
            std::fs::write(self.places.root.join("proc/sys/net/ipv4/tcp_available_congestion_control"), "reno cubic bbr3\n").unwrap();
            Ok(())
        }

        fn unload(&mut self, name: &str) -> Result<bool, String> {
            assert_eq!(name, "tcp_bbr3");
            if self.in_use {
                return Ok(false);
            }
            std::fs::remove_dir_all(self.places.loaded(&MODULE)).unwrap();
            Ok(true)
        }
    }

    const K: &str = "7.2.15-300.fc45.x86_64";

    /// A kernel's first start with Olo: the module built for it, loaded
    /// and chosen; at its next, loaded and chosen; chosen already,
    /// nothing done. What new connections had is kept, and given back.
    #[test]
    fn built_once_a_kernel_loaded_and_chosen_at_each_start() {
        let tree = Tree::new("built-once");
        let places = &tree.0;
        tree.headers(K);
        let mut kernel = Fake::new(&tree);
        assert_eq!(ensure(places, K, &mut kernel, None), Ok(Done::BuiltAndLoaded));
        assert_eq!((kernel.built, kernel.loaded, default(places).as_deref()), (1, 1, Some("bbr3")));
        assert_eq!(read(&was_file(places)).as_deref(), Some("cubic\n"));
        assert_eq!(ensure(places, K, &mut kernel, None), Ok(Done::Already));
        assert_eq!((kernel.built, kernel.loaded), (1, 1));
        assert_eq!(
            status(places, K, Some(1)),
            format!("{{\"kernel\":\"{K}\",\"congestion_control\":\"bbr3\",\"available\":[\"reno\",\"cubic\",\"bbr3\"],\"loaded\":true,\"built\":true,\"headers\":true,\"kernels_bbr\":1}}")
        );
        // Something set it back (a sysctl file read again): chosen again.
        std::fs::write(default_file(places), "reno\n").unwrap();
        assert_eq!(ensure(places, K, &mut kernel, None), Ok(Done::Chosen));
        assert_eq!((default(places).as_deref(), read(&was_file(places)).as_deref()), (Some("bbr3"), Some("reno\n")));
        // The computer's next start: the kernel as it was, the build kept.
        std::fs::remove_dir_all(places.loaded(&MODULE)).unwrap();
        std::fs::write(default_file(places), "cubic\n").unwrap();
        assert_eq!(ensure(places, K, &mut kernel, None), Ok(Done::Loaded));
        assert_eq!((kernel.built, kernel.loaded, default(places).as_deref()), (1, 2, Some("bbr3")));
        // Taken out: what there was before, and the module gone; unless
        // a connection runs it still.
        kernel.in_use = true;
        assert_eq!(remove(places, &mut kernel), Ok(false));
        assert_eq!(default(places).as_deref(), Some("cubic"));
        assert!(places.loaded(&MODULE).is_dir());
        kernel.in_use = false;
        assert_eq!(remove(places, &mut kernel), Ok(true));
        assert!(!places.loaded(&MODULE).is_dir());
        assert_eq!(remove(places, &mut kernel), Ok(true));
    }

    /// An update brings a new kernel (Ben, 2026-11-16: "make sure all
    /// these kernel tunings/mods will continue to work properly after
    /// doing os updates that update the kernel"): at its first start the
    /// module is built for it and loaded, as at the very first. The old
    /// kernel's build is kept while that kernel is (it can still be
    /// started) and goes with it. A kernel that came without its headers
    /// is said, with TCP left as it comes (Olo Doctor's fix installs the
    /// package that brings them with every kernel).
    #[test]
    fn built_for_the_kernel_an_update_brings_at_its_first_start() {
        const NEXT: &str = "7.3.3-300.fc45.x86_64";
        let tree = Tree::new("update");
        let places = &tree.0;
        tree.headers(K);
        let mut kernel = Fake::new(&tree);
        assert_eq!(ensure(places, K, &mut kernel, None), Ok(Done::BuiltAndLoaded));
        // The update: the new kernel and its headers beside the old one.
        // Then the computer restarted (nothing loaded, TCP as it comes).
        tree.headers(NEXT);
        let restart = |places: &Places| {
            let _ = std::fs::remove_dir_all(places.loaded(&MODULE));
            std::fs::write(default_file(places), "cubic\n").unwrap();
        };
        restart(places);
        assert!(!olo_kmod::built(places, &MODULE, NEXT));
        assert_eq!(ensure(places, NEXT, &mut kernel, None), Ok(Done::BuiltAndLoaded));
        assert_eq!((kernel.built, kernel.loaded, default(places).as_deref()), (2, 2, Some("bbr3")));
        assert!(places.module(&MODULE, K).is_file() && places.module(&MODULE, NEXT).is_file());
        // The old kernel started once more: its own build, as it was.
        restart(places);
        assert_eq!(ensure(places, K, &mut kernel, None), Ok(Done::Loaded));
        assert_eq!((kernel.built, default(places).as_deref()), (2, Some("bbr3")));
        // The old kernel removed (a later update): its build goes at the
        // next start, the new one's stays.
        std::fs::remove_file(places.kernels().join(K).join("vmlinuz")).unwrap();
        restart(places);
        assert_eq!(ensure(places, NEXT, &mut kernel, None), Ok(Done::Loaded));
        assert!(!places.module(&MODULE, K).exists() && !places.work(&MODULE, K).exists() && places.module(&MODULE, NEXT).is_file());
        // A kernel without its headers: why is said, nothing is chosen.
        const BARE: &str = "7.4.0-100.fc45.x86_64";
        std::fs::create_dir_all(places.kernels().join(BARE)).unwrap();
        std::fs::write(places.kernels().join(BARE).join("vmlinuz"), "").unwrap();
        restart(places);
        let went = ensure(places, BARE, &mut kernel, None);
        assert_eq!(went, Err(format!("this kernel's headers aren't installed (the kernel-devel package for {BARE})")));
        say(places, &went);
        assert_eq!(
            (default(places).as_deref(), read(&places.said(&MODULE)).as_deref().map(str::trim)),
            (Some("cubic"), Some(format!("failed: this kernel's headers aren't installed (the kernel-devel package for {BARE})").as_str()))
        );
    }

    /// A build from another source (Olo upgraded) is made again: loaded
    /// at once where the old one isn't in, from the next start where it
    /// is. One the kernel refuses (the same name, rebuilt) is built again
    /// once.
    #[test]
    fn built_again_for_a_new_source_or_a_changed_kernel() {
        let tree = Tree::new("built-again");
        let places = &tree.0;
        tree.headers(K);
        let mut kernel = Fake::new(&tree);
        std::fs::create_dir_all(places.module(&MODULE, K).parent().unwrap()).unwrap();
        std::fs::create_dir_all(places.work(&MODULE, K)).unwrap();
        std::fs::write(places.module(&MODULE, K), "old").unwrap();
        std::fs::write(places.work(&MODULE, K).join("tcp_bbr3.c"), "an older source").unwrap();
        assert_eq!(ensure(places, K, &mut kernel, None), Ok(Done::BuiltAndLoaded));
        assert_eq!((kernel.built, read(&places.module(&MODULE, K)).as_deref()), (1, Some("built")));
        // The old one in the kernel, a newer source here: built for the
        // next start, the old one left to the connections that run it.
        std::fs::write(places.work(&MODULE, K).join("bbr3_olo.h"), "an older header").unwrap();
        assert_eq!(ensure(places, K, &mut kernel, None), Ok(Done::Already));
        assert_eq!((kernel.built, kernel.loaded), (2, 1));
        // Refused once though built from this source: built again, taken.
        std::fs::remove_dir_all(places.loaded(&MODULE)).unwrap();
        kernel.refuses = 1;
        assert_eq!(ensure(places, K, &mut kernel, None), Ok(Done::Loaded));
        assert_eq!((kernel.built, kernel.loaded), (3, 2));
        // Refused however built: said, and nothing chosen that isn't there.
        std::fs::remove_dir_all(places.loaded(&MODULE)).unwrap();
        std::fs::write(default_file(places), "cubic\n").unwrap();
        kernel.refuses = 2;
        assert_eq!(ensure(places, K, &mut kernel, None), Err("refused".into()));
        assert_eq!(default(places).as_deref(), Some("cubic"));
    }

    /// A kernel whose own "bbr" is BBRv3 has that chosen, and nothing
    /// built; one whose "bbr" is BBRv1 gets Olo's.
    #[test]
    fn the_kernels_own_bbrv3_is_chosen() {
        let tree = Tree::new("kernels");
        let places = &tree.0;
        let mut kernel = Fake::new(&tree);
        assert_eq!(ensure(places, K, &mut kernel, Some(3)), Ok(Done::Kernels));
        assert_eq!((kernel.built, kernel.loaded, default(places).as_deref()), (0, 0, Some("bbr")));
        say(places, &Ok(Done::Kernels));
        assert_eq!(read(&places.said(&MODULE)).as_deref(), Some("bbr\n"));
        tree.headers(K);
        assert_eq!(ensure(places, K, &mut kernel, Some(1)), Ok(Done::BuiltAndLoaded));
        assert_eq!(default(places).as_deref(), Some("bbr3"));
    }

    /// What it can't do, said where Olo Doctor reads it; and "bbr3" once
    /// it's in.
    #[test]
    fn what_it_cant_do_is_said() {
        let tree = Tree::new("cant");
        let places = &tree.0;
        let mut kernel = Fake::new(&tree);
        let e = ensure(places, K, &mut kernel, None).unwrap_err();
        assert_eq!(e, format!("this kernel's headers aren't installed (the kernel-devel package for {K})"));
        assert_eq!((kernel.built, default(places).as_deref()), (0, Some("cubic")));
        say(places, &Err(e));
        assert_eq!(read(&places.said(&MODULE)).as_deref(), Some(format!("failed: this kernel's headers aren't installed (the kernel-devel package for {K})\n").as_str()));
        assert!(places.said(&MODULE).ends_with("run/olo-bbr3/status"));
        say(places, &Ok(Done::Loaded));
        assert_eq!(read(&places.said(&MODULE)).as_deref(), Some("bbr3\n"));
    }

    /// The module's source is what it's said to be: Google's BBRv3,
    /// registered as "bbr3", under the kernel's licence, and never the
    /// self-checking build.
    #[test]
    fn the_module_is_bbr3() {
        let source = MODULE.files.iter().find(|(n, _)| *n == "tcp_bbr3.c").unwrap().1;
        assert!(source.starts_with("// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause\n"));
        assert!(source.contains(".name\t\t= \"bbr3\"") && source.contains("#define BBR_VERSION\t\t3") && source.contains("MODULE_LICENSE(\"Dual BSD/GPL\")"));
        assert!(source.contains("\"tcp_mark_skb_lost\"") && source.contains("\"__tcp_retransmit_skb\""));
        let kbuild = MODULE.files.iter().find(|(n, _)| *n == "Kbuild").unwrap().1;
        assert!(kbuild.starts_with("obj-m := tcp_bbr3.o\n") && kbuild.contains("ccflags-$(BBR3_SELFCHECK)"));
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
        let module = std::fs::read(tree.0.module(&MODULE, &release)).unwrap();
        let has = |what: &str| module.windows(what.len()).any(|w| w == what.as_bytes());
        assert!(has(&format!("vermagic={release} ")) && has("bbr3") && has("version=3"));
        // (Not the self-checking build: no probe on every packet sent.)
        assert!(!has("__tcp_transmit_skb"));
        assert!(olo_kmod::built(&tree.0, &MODULE, &release));
    }

    /// The real build for another kernel than the running one: the one
    /// an update brings, before it's installed (`--ignored`, with
    /// `OLO_NEXT_KERNEL=RELEASE=HEADERS`: that kernel's kernel-devel
    /// unpacked somewhere, as tools/dev/bbr3-check's `--on` has it).
    /// Whether the module is right there is that tool's to say, in a
    /// guest that runs the kernel.
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
        assert!(has(&format!("vermagic={release} ")) && has("bbr3") && has("version=3"));
    }
}
