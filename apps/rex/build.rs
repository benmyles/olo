//! Olo's helper on a Rex host (`crates/rex-link`) is carried in the app
//! and put on the hosts by it (HACKING, Rex, "A helper on the host"):
//! built here for each kind of host, statically (musl: it runs on any
//! Linux of its machine, whatever is installed there), whatever computer
//! the app itself is built for.
//!
//! Each is one `cargo build` of that package for its target, in a
//! folder of its own under the target folder (shared by every profile:
//! the helper is the same for a dev build of the app and a release
//! one), linked by the toolchain's own linker (no C compiler for
//! another machine is needed). The targets are in `rust-toolchain.toml`,
//! so rustup has them; where one can't be built the app is built
//! without that host's helper, and says so here.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The hosts the helper is built for: the target, and the name its
/// build is carried under (`src/link.rs`).
const HOSTS: [(&str, &str); 2] = [("x86_64-unknown-linux-musl", "x86_64"), ("aarch64-unknown-linux-musl", "aarch64")];

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_owned();
    for changed in ["crates/rex-link/src", "crates/rex-link/Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
        println!("cargo:rerun-if-changed={}", root.join(changed).display());
    }
    println!("cargo:rerun-if-env-changed=OLO_REX_LINK_SKIP");
    // (OUT_DIR is under TARGET/PROFILE/build, however cargo lays that
    // folder out: the helper's own folder is beside the profiles'.)
    let profile = out.ancestors().find(|dir| dir.file_name().is_some_and(|name| name == "build")).and_then(Path::parent).unwrap_or(&out);
    let target = profile.parent().unwrap_or(profile).join("rex-link");
    let mut id = 0xcbf29ce484222325u64;
    for (triple, name) in HOSTS {
        let built = target.join(triple).join("release/olo-rex-link");
        let carried = out.join(format!("olo-rex-link-{name}"));
        // (`OLO_REX_LINK_SKIP`: built without the helpers, for a build
        // that only checks the app.)
        let ok = std::env::var_os("OLO_REX_LINK_SKIP").is_none() && build(&root, &target, triple);
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
}

fn build(root: &Path, target: &Path, triple: &str) -> bool {
    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.current_dir(root).args(["build", "--release", "--locked", "-p", "olo-rex-link", "--bin", "olo-rex-link", "--target", triple, "--target-dir"]).arg(target);
    // (What this build was told is not for that one: its flags, its
    // target folder, the profile it is built in.)
    for (name, _) in std::env::vars() {
        if name == "RUSTFLAGS"
            || name == "CARGO_TARGET_DIR"
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
    cargo
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
