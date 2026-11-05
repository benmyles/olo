//! Every Olo program installs the crash report first thing: each file
//! with a program's `main` in apps/ and crates/ (not tests, benches,
//! examples or build scripts) calls `olo_crash::install()` before
//! anything else.

use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !matches!(name.as_str(), "target" | "tests" | "benches" | "examples" | "vendor" | "node_modules") && !name.starts_with('.') {
                walk(&p, out);
            }
        } else if name.ends_with(".rs") && name != "build.rs" {
            out.push(p);
        }
    }
}

#[test]
fn every_program_installs_it_first() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    walk(&root.join("apps"), &mut files);
    walk(&root.join("crates"), &mut files);
    let mut mains = 0;
    let mut missing = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        // A program's main: at the start of a line, not in a test module.
        let Some(at) = text.lines().position(|l| l.starts_with("fn main()")) else { continue };
        mains += 1;
        let first = text.lines().skip(at + 1).map(str::trim).find(|l| !l.is_empty());
        if first != Some("olo_crash::install();") {
            missing.push(f.strip_prefix(&root).unwrap_or(&f).display().to_string());
        }
    }
    assert!(mains >= 49, "only {mains} programs found: the walk is wrong");
    assert!(missing.is_empty(), "these programs' main doesn't start with olo_crash::install(): {missing:?}");
}
