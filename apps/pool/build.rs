//! The Token Pool's program for Rex hosts (`olo-pool-host`,
//! `crates/pool/src/bin`) is carried in the app and put on the hosts by
//! it (HACKING, Token Pool, "A pool of its own on a Rex host"): built
//! here for each kind of host, statically (musl: it runs on any Linux
//! of its machine), whatever computer the app itself is built for.
//!
//! Each is one `cargo build` of that program for its target, without
//! the crate's `desktop` feature, in a folder of its own under the
//! target folder (shared by every profile). It has C in it (aws-lc,
//! zstd), compiled for the host's machine by zig (`zig cc`, through a
//! wrapper written here for each target) and linked by the toolchain's
//! own linker. Where zig isn't installed, or a target can't be built,
//! the app is built without that host's pool, and says so here.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The hosts it is built for: the target, zig's name for it, and the
/// name its build is carried under (`src/host_builds.rs`).
const HOSTS: [(&str, &str, &str); 2] = [("x86_64-unknown-linux-musl", "x86_64-linux-musl", "x86_64"), ("aarch64-unknown-linux-musl", "aarch64-linux-musl", "aarch64")];

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_owned();
    // (The program is the pool's crate and what it stands on.)
    for changed in ["crates/pool/src", "crates/pool/Cargo.toml", "crates/xdg/src", "crates/net/src", "crates/sync/src", "crates/protocol/src", "crates/app/src", "Cargo.lock", "rust-toolchain.toml"] {
        println!("cargo:rerun-if-changed={}", root.join(changed).display());
    }
    println!("cargo:rerun-if-env-changed=OLO_POOL_HOST_SKIP");
    let profile = out.ancestors().find(|dir| dir.file_name().is_some_and(|name| name == "build")).and_then(Path::parent).unwrap_or(&out);
    let target = profile.parent().unwrap_or(profile).join("pool-host");
    let zig = std::env::var_os("OLO_POOL_HOST_SKIP").is_none() && Command::new("zig").arg("version").output().is_ok_and(|o| o.status.success());
    if !zig && std::env::var_os("OLO_POOL_HOST_SKIP").is_none() {
        println!("cargo:warning=zig isn't installed (`dnf install zig`): Token Pool is built without its program for Rex hosts");
    }
    let mut id = 0xcbf29ce484222325u64;
    for (triple, zig_target, name) in HOSTS {
        let built = target.join(triple).join("release/olo-pool-host");
        let carried = out.join(format!("olo-pool-host-{name}"));
        let ok = zig && build(&root, &target, &out, triple, zig_target);
        let bytes = if ok { std::fs::read(&built).unwrap_or_default() } else { Vec::new() };
        if zig && bytes.is_empty() {
            println!("cargo:warning=Token Pool's program for {name} hosts wasn't built ({triple}); the app is built without it");
        }
        for byte in &bytes {
            id = (id ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
        std::fs::write(&carried, &bytes).unwrap();
        println!("cargo:rustc-env=OLO_POOL_HOST_{}={}", name.to_uppercase(), carried.display());
    }
    println!("cargo:rustc-env=OLO_POOL_HOST_ID={id:016x}");
}

/// zig as the C compiler and the archiver for `zig_target`: cc's own
/// `--target=` is dropped (zig doesn't take the target as Rust names
/// it), and zig's checks for undefined behaviour are left out (their
/// handlers aren't linked: `__ubsan_handle_*` undefined otherwise).
fn wrappers(out: &Path, zig_target: &str) -> Option<(PathBuf, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;
    let (cc, ar) = (out.join(format!("zcc-{zig_target}")), out.join(format!("zar-{zig_target}")));
    let script = format!("#!/bin/sh\nfor a in \"$@\"; do shift; case \"$a\" in --target=*) ;; *) set -- \"$@\" \"$a\";; esac; done\nexec zig cc -target {zig_target} -fno-sanitize=undefined \"$@\"\n");
    std::fs::write(&cc, script).ok()?;
    std::fs::write(&ar, "#!/bin/sh\nexec zig ar \"$@\"\n").ok()?;
    for file in [&cc, &ar] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).ok()?;
    }
    Some((cc, ar))
}

fn build(root: &Path, target: &Path, out: &Path, triple: &str, zig_target: &str) -> bool {
    let Some((cc, ar)) = wrappers(out, zig_target) else { return false };
    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.current_dir(root).args(["build", "--release", "--locked", "-p", "olo-token-pool", "--no-default-features", "--bin", "olo-pool-host", "--target", triple, "--target-dir"]).arg(target);
    // (What this build was told is not for that one: its flags, its
    // target folder, the profile it is built in, the C compiler.)
    for (name, _) in std::env::vars() {
        if name == "RUSTFLAGS"
            || name == "CARGO_TARGET_DIR"
            || name == "CARGO_BUILD_TARGET"
            || name == "CC"
            || name == "CFLAGS"
            || name == "AR"
            || name.starts_with("CARGO_PROFILE_")
            || name.starts_with("CARGO_CFG_")
            || name.starts_with("CARGO_FEATURE_")
        {
            cargo.env_remove(name);
        }
    }
    // (No path of this computer goes to a host in it.)
    let mut flags = vec!["-Clinker=rust-lld".to_owned(), format!("--remap-path-prefix={}=/olo", root.display())];
    for (var, named) in [("HOME", "/home"), ("CARGO_HOME", "/cargo"), ("RUSTUP_HOME", "/rustup")] {
        if let Some(dir) = std::env::var_os(var).filter(|dir| !dir.is_empty()) {
            flags.push(format!("--remap-path-prefix={}={named}", Path::new(&dir).display()));
        }
    }
    let under = triple.replace('-', "_");
    cargo
        .env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f"))
        // (This workspace unifies features across all its packages:
        // the host's program takes only its own.)
        .env("CARGO_RESOLVER_FEATURE_UNIFICATION", "selected")
        .env(format!("CC_{under}"), &cc)
        .env(format!("AR_{under}"), &ar)
        // (And no path of this computer in what the C compiler writes.)
        .env(format!("CFLAGS_{under}"), format!("-ffile-prefix-map={}=/olo", root.display()))
        .env("CARGO_PROFILE_RELEASE_DEBUG", "0")
        .env("CARGO_PROFILE_RELEASE_STRIP", "symbols");
    match cargo.output() {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            for line in String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.starts_with("error")).take(4) {
                println!("cargo:warning={line}");
            }
            false
        }
        Err(e) => {
            println!("cargo:warning=cargo couldn't be run for Token Pool's host program: {e}");
            false
        }
    }
}
