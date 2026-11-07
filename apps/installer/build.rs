//! Olo Setup carries what Olo's kit draws with (its fonts, the icons its
//! pages show, the wordmark): none of Olo is installed when it starts.
//! Listed here as `ASSETS` (a path under Olo's data folder, and the
//! bytes), unpacked at start into a folder the kit is pointed at
//! (`OLO_DATA`).

use std::{fmt::Write as _, path::Path};

/// The icons the pages show, at every size Olo draws them.
const ICONS: &[&str] = &["installer", "olo", "document", "preferences", "archive", "warning", "info", "about", "restart", "log-out", "desktop", "software-update"];
const SIZES: &[u32] = &[16, 24, 32, 48, 64, 96, 128, 256];

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
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data").canonicalize().unwrap();
    let mut files = Vec::new();
    walk(&data.join("fonts"), &data, &mut files);
    walk(&data.join("brand"), &data, &mut files);
    for size in SIZES {
        for name in ICONS {
            let p = format!("icons/{size}/{name}.png");
            if data.join(&p).is_file() {
                files.push(p);
            }
        }
    }
    let mut code = String::from("/// Olo's data Setup carries: (path under the data folder, bytes).\npub const ASSETS: &[(&str, &[u8])] = &[\n");
    for f in &files {
        let _ = writeln!(code, "    ({f:?}, include_bytes!({:?})),", data.join(f).display().to_string());
        println!("cargo:rerun-if-changed={}", data.join(f).display());
    }
    code.push_str("];\n");
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("assets.rs"), code).unwrap();
}
