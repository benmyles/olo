//! The healer's source as a fingerprint built into it (`OLO_HEAL_SOURCE`):
//! its own files and every crate it's built from, as Cargo.lock has them.
//! Olo Doctor compares the installed copy's with the new build's: their
//! bytes differ whenever what was built beside it changes a dependency's
//! features, the healer itself the same, and it was "out of date" after
//! every upgrade.

use std::{collections::BTreeSet, path::Path};

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lock = dir.join("../../Cargo.lock");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed={}", lock.display());
    let mut h = Fnv::default();
    let mut files = Vec::new();
    gather(&dir.join("src"), &mut files);
    files.push(dir.join("Cargo.toml"));
    files.sort();
    for f in &files {
        h.add(f.strip_prefix(dir).unwrap_or(f).to_string_lossy().as_bytes());
        h.add(&std::fs::read(f).unwrap_or_default());
    }
    for package in reachable(&std::fs::read_to_string(&lock).unwrap_or_default(), "olo-wifi-heal") {
        h.add(package.as_bytes());
    }
    println!("cargo:rustc-env=OLO_HEAL_SOURCE={:016x}", h.0);
}

fn gather(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            gather(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Every package `root` is built from in `lock` ("name version"), itself
/// included.
fn reachable(lock: &str, root: &str) -> BTreeSet<String> {
    // Each package: its name, version and what it depends on (entries are
    // "name" or "name version", with a source when two versions are in).
    let packages: Vec<(String, String, Vec<String>)> = lock
        .split("[[package]]")
        .skip(1)
        .map(|block| {
            let field = |k: &str| block.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().trim_matches('"').to_owned()).unwrap_or_default();
            let deps = block.split_once("dependencies = [").map_or(Vec::new(), |(_, rest)| {
                rest.split(']').next().unwrap_or("").split(',').map(|d| d.trim().trim_matches('"').to_owned()).filter(|d| !d.is_empty()).collect()
            });
            (field("name = "), field("version = "), deps)
        })
        .collect();
    let mut seen = BTreeSet::new();
    let mut todo = vec![root.to_owned()];
    while let Some(want) = todo.pop() {
        let mut parts = want.split(' ');
        let (name, version) = (parts.next().unwrap_or(""), parts.next());
        for (n, v, deps) in &packages {
            if n == name && version.is_none_or(|want| want == v) && seen.insert(format!("{n} {v}")) {
                todo.extend(deps.iter().cloned());
            }
        }
    }
    seen
}

/// FNV-1a, 64 bits: the same from build to build and Rust to Rust.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn add(&mut self, bytes: &[u8]) {
        for &b in bytes.iter().chain(&[0xff]) {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}
