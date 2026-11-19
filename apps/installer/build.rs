//! Olo Setup carries what Olo's kit draws with (its fonts, the icons its
//! pages show, the wordmark): none of Olo is installed when it starts.
//! Listed here as `ASSETS` (a path under Olo's data folder, and the
//! bytes), unpacked at start into a folder the kit is pointed at
//! (`OLO_DATA`).

use std::{fmt::Write as _, path::Path};

/// The icons the pages show: their drawings, rendered for whatever size
/// and scale Setup runs at.
const ICONS: &[&str] = &["installer", "olo", "document", "preferences", "archive", "warning", "info", "about", "restart", "log-out", "desktop", "software-update"];

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out);
        } else {
            out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
        }
    }
}

fn main() {
    // (Its folder as Cargo runs it, not as it was compiled: kache gave a
    // build of the main checkout this script compiled in a worktree since
    // removed, its path in it, and it found no data folder, 2026-11-19.)
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo runs a build script with CARGO_MANIFEST_DIR");
    let data = Path::new(&manifest).join("../../data").canonicalize().unwrap();
    let mut files = Vec::new();
    walk(&data.join("fonts"), &data, &mut files);
    walk(&data.join("brand"), &data, &mut files);
    for name in ICONS {
        let p = format!("icons/svg/{name}.svg");
        assert!(data.join(&p).is_file(), "Olo has no drawing of {name}");
        files.push(p);
    }
    let mut code = String::from("/// Olo's data Setup carries: (path under the data folder, bytes).\npub const ASSETS: &[(&str, &[u8])] = &[\n");
    for f in &files {
        let _ = writeln!(code, "    ({f:?}, include_bytes!({:?})),", data.join(f).display().to_string());
        println!("cargo:rerun-if-changed={}", data.join(f).display());
    }
    code.push_str("];\n");
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("assets.rs"), code).unwrap();
}
