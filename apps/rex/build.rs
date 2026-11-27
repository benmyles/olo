//! Olo's helper on a Rex host (`crates/rex-link`) is carried in the app
//! and put on the hosts by it (HACKING, Rex, "A helper on the host"):
//! built here for each kind of host, statically (musl: it runs on any
//! Linux of its machine, whatever is installed there), whatever computer
//! the app itself is built for.
//!
//! Each is one `cargo build` of that package for its target, in a
//! folder of its own under the target folder (shared by every profile:
//! the helper is the same for a dev build of the app and a release
//! one), linked by the toolchain's own linker. The targets are in
//! `rust-toolchain.toml`, so rustup has them; where one can't be built
//! the app is built without that host's helper, and says so here.
//!
//! The helper's own link over UDP (its `quic` feature) brings TLS,
//! whose ciphers are C, and keeping files in step (`sync`) brings zstd,
//! C too: compiled for the host's machine by zig (`zig cc`, through a
//! wrapper written here for each target). Where zig isn't installed the
//! helper is built without either (no C in it, as before), and says so
//! here.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The hosts the helper is built for: the target, and the name its
/// build is carried under (`src/link.rs`).
const HOSTS: [(&str, &str); 2] = [("x86_64-unknown-linux-musl", "x86_64"), ("aarch64-unknown-linux-musl", "aarch64")];

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_owned();
    // (The helper's own code, the engine's, and russh's (vendored, the
    // helper's own SSH connection when it moves an account).)
    for changed in [
        "crates/rex-link/src",
        "crates/rex-link/Cargo.toml",
        "crates/agent-screen/src",
        "crates/agent-screen/Cargo.toml",
        // (What the helper reads recordings with, for a host's screens:
        // a fix there alone left the helper inside Rex as it was. A
        // code review, 2026-11-27.)
        "crates/screen-codec/src",
        "crates/screen-codec/Cargo.toml",
        "crates/rex-sync/src",
        "crates/rex-sync/Cargo.toml",
        "vendor/russh/src",
        "vendor/russh/Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(changed).display());
    }
    println!("cargo:rerun-if-env-changed=OLO_REX_LINK_SKIP");
    // (OUT_DIR is under TARGET/PROFILE/build, however cargo lays that
    // folder out: the helper's own folder is beside the profiles'.)
    let profile = out.ancestors().find(|dir| dir.file_name().is_some_and(|name| name == "build")).and_then(Path::parent).unwrap_or(&out);
    let target = profile.parent().unwrap_or(profile).join("rex-link");
    let skip = std::env::var_os("OLO_REX_LINK_SKIP").is_some();
    let zig = !skip && Command::new("zig").arg("version").output().is_ok_and(|o| o.status.success());
    if !zig && !skip {
        println!("cargo:warning=zig isn't installed (`dnf install zig`): Olo's helper for hosts is built without its link over UDP");
    }
    let mut id = 0xcbf29ce484222325u64;
    // (Whether every helper carried keeps files in step: built with zig.)
    let mut syncs = true;
    for (triple, name) in HOSTS {
        let built = target.join(triple).join("release/olo-rex-link");
        let carried = out.join(format!("olo-rex-link-{name}"));
        // (`OLO_REX_LINK_SKIP`: built without the helpers, for a build
        // that only checks the app.)
        // (With the link where zig can compile its C for the host; and
        // without it where that build fails, rather than no helper.)
        let full = !skip && zig && build(&root, &target, &out, triple, true);
        let ok = full || (!skip && build(&root, &target, &out, triple, false));
        syncs &= full;
        let bytes = if ok { std::fs::read(&built).unwrap_or_default() } else { Vec::new() };
        if bytes.is_empty() {
            println!("cargo:warning=Olo's helper for {name} hosts wasn't built ({triple}: is the target installed? `rustup target add {triple}`); Rex is built without it");
        }
        for byte in &bytes {
            id = (id ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
        std::fs::write(&carried, &bytes).unwrap();
        println!("cargo:rustc-env=OLO_REX_LINK_{}={}", name.to_uppercase(), carried.display());
    }
    // The build's name: what the app looks for on a host is exactly
    // what it carries.
    println!("cargo:rustc-env=OLO_REX_LINK_ID={id:016x}");
    println!("cargo:rustc-env=OLO_REX_LINK_SYNC={}", u8::from(syncs));
}

/// zig as the C compiler and the archiver for the host's machine: cc's
/// own `--target=` is dropped (zig doesn't take the target as Rust
/// names it), and zig's checks for undefined behaviour are left out
/// (their handlers aren't linked: `__ubsan_handle_*` undefined
/// otherwise).
fn wrappers(out: &Path, triple: &str) -> Option<(PathBuf, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;
    let zig_target = triple.replace("-unknown", "");
    let (cc, ar) = (out.join(format!("zcc-{zig_target}")), out.join(format!("zar-{zig_target}")));
    let script = format!("#!/bin/sh\nfor a in \"$@\"; do shift; case \"$a\" in --target=*) ;; *) set -- \"$@\" \"$a\";; esac; done\nexec zig cc -target {zig_target} -fno-sanitize=undefined \"$@\"\n");
    std::fs::write(&cc, script).ok()?;
    std::fs::write(&ar, "#!/bin/sh\nexec zig ar \"$@\"\n").ok()?;
    for file in [&cc, &ar] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).ok()?;
    }
    Some((cc, ar))
}

fn build(root: &Path, target: &Path, out: &Path, triple: &str, quic: bool) -> bool {
    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.current_dir(root).args(["build", "--release", "--locked", "-p", "olo-rex-link", "--bin", "olo-rex-link", "--target", triple, "--target-dir"]).arg(target);
    // (And keeping files in step, `sync`: its engine's zstd is C too; and
    // moving an account, `move`: the engine, and russh's ciphers, C as
    // the link's are.)
    // (The agents service, `agents`, is in every build: all Rust, it
    // needs no C compiler for the host's machine. `move` has it.)
    cargo.args(["--features", if quic { "quic,sync,move,screens" } else { "agents" }]);
    // (What this build was told is not for that one: its flags, its
    // target folder, the profile it is built in.)
    for (name, _) in std::env::vars() {
        if name == "RUSTFLAGS"
            || name == "CARGO_TARGET_DIR"
            || name == "CC"
            || name == "CFLAGS"
            || name == "AR"
            || name == "CARGO_BUILD_TARGET"
            || name.starts_with("CARGO_PROFILE_")
            || name.starts_with("CARGO_CFG_")
            || name.starts_with("CARGO_FEATURE_")
        {
            cargo.env_remove(name);
        }
    }
    // (No path of this computer goes to a host in it: the folders its
    // sources are read from are named as anybody's would be, which
    // also makes the same helper of the same sources wherever they
    // are checked out.)
    let mut flags = vec!["-Clinker=rust-lld".to_owned(), format!("--remap-path-prefix={}=/olo", root.display())];
    for (var, named) in [("HOME", "/home"), ("CARGO_HOME", "/cargo"), ("RUSTUP_HOME", "/rustup")] {
        if let Some(dir) = std::env::var_os(var).filter(|dir| !dir.is_empty()) {
            flags.push(format!("--remap-path-prefix={}={named}", Path::new(&dir).display()));
        }
    }
    if quic {
        let Some((cc, ar)) = wrappers(out, triple) else { return false };
        let under = triple.replace('-', "_");
        // (And no path of this computer in what the C compiler writes.)
        cargo.env(format!("CC_{under}"), cc).env(format!("AR_{under}"), ar).env(format!("CFLAGS_{under}"), format!("-ffile-prefix-map={}=/olo", root.display()));
    }
    cargo
        // (This workspace unifies features across all its packages:
        // the helper takes only its own.)
        .env("CARGO_RESOLVER_FEATURE_UNIFICATION", "selected")
        .env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f"))
        .env("CARGO_PROFILE_RELEASE_DEBUG", "0")
        .env("CARGO_PROFILE_RELEASE_STRIP", "symbols")
        .env("CARGO_PROFILE_RELEASE_PANIC", "abort")
        .env("CARGO_PROFILE_RELEASE_LTO", "fat")
        .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1");
    match cargo.output() {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            for line in String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.starts_with("error")).take(4) {
                println!("cargo:warning={line}");
            }
            false
        }
        Err(e) => {
            println!("cargo:warning=cargo couldn't be run for Olo's helper: {e}");
            false
        }
    }
}
