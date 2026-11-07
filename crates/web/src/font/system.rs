//! The system's installed fonts, mapped into a catalogue, and the family
//! policy Chrome uses on Linux: its standard font is Times New Roman, its
//! sans-serif Arial and its fixed font monospace, which fontconfig answers
//! with Liberation Serif, Liberation Sans and Noto Sans Mono on a stock
//! Fedora; `system-ui` is fontconfig's sans (Noto Sans). Metric-compatible
//! aliases (Arial → Liberation Sans, …) follow fontconfig's
//! 30-metric-aliases.conf, so a page naming Arial renders as in Chrome.
use super::{Builder, GenericPolicy, Result};
use crate::css::properties::GenericFamily;
use std::path::{Path, PathBuf};
/// Where fonts are installed (the XDG base directories, as fontconfig
/// looks): `$XDG_DATA_DIRS` (`/usr/local/share:/usr/share` without it)
/// and `$XDG_DATA_HOME` (`~/.local/share`), each with `fonts`, and
/// `~/.fonts`.
pub fn directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let data_dirs = std::env::var_os("XDG_DATA_DIRS").filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for dir in std::env::split_paths(&data_dirs) {
        if !dir.as_os_str().is_empty() {
            dirs.push(dir.join("fonts"));
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    if let Some(data_home) = data_home {
        dirs.push(data_home.join("fonts"));
    }
    if let Some(home) = home {
        dirs.push(home.join(".fonts"));
    }
    dirs.retain(|d| d.is_dir());
    dirs.dedup();
    dirs
}
/// Lease every font file under the directories (recursively, bounded) to
/// the builder through `open` (which maps or reads it); files that fail to
/// open or parse are skipped. Returns how many files were taken.
pub fn map_directories(builder: &mut Builder, dirs: &[PathBuf], mut open: impl FnMut(&Path) -> Option<std::sync::Arc<dyn AsRef<[u8]> + Send + Sync>>) -> usize {
    let mut taken = 0;
    let mut stack: Vec<(PathBuf, usize)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            visited += 1;
            if visited > 20_000 {
                return taken;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() || (kind.is_symlink() && path.is_dir()) {
                if depth < 8 {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let font = path.extension().and_then(|e| e.to_str()).is_some_and(|e| ["ttf", "otf", "ttc", "otc"].iter().any(|k| e.eq_ignore_ascii_case(k)));
            if font
                && let Some(lease) = open(&path)
                && builder.add_leased(lease, None).is_ok()
            {
                taken += 1;
            }
        }
    }
    taken
}
/// Chrome's family policy on Linux, as above, over the installed fonts;
/// Olo's own Noto faces answer whatever is missing.
pub fn chrome_policy() -> Result<GenericPolicy> {
    let mut policy = GenericPolicy::new(&["Liberation Serif", "Noto Serif", "Noto Sans"])?;
    policy.set(GenericFamily::Serif, "", None, &["Liberation Serif", "Noto Serif", "DejaVu Serif"])?;
    policy.set(GenericFamily::SansSerif, "", None, &["Liberation Sans", "Noto Sans", "DejaVu Sans"])?;
    policy.set(GenericFamily::Monospace, "", None, &["Noto Sans Mono", "Liberation Mono", "DejaVu Sans Mono"])?;
    policy.set(GenericFamily::SystemUi, "", None, &["Noto Sans", "Adwaita Sans", "Cantarell", "DejaVu Sans"])?;
    policy.set(GenericFamily::UiSansSerif, "", None, &["Noto Sans", "Adwaita Sans", "DejaVu Sans"])?;
    policy.set(GenericFamily::UiSerif, "", None, &["Noto Serif", "Liberation Serif"])?;
    policy.set(GenericFamily::UiMonospace, "", None, &["Adwaita Mono", "Noto Sans Mono", "DejaVu Sans Mono"])?;
    policy.set(GenericFamily::Cursive, "", None, &["Z003", "Comic Neue", "Noto Sans"])?;
    policy.set(GenericFamily::Fantasy, "", None, &["Noto Sans"])?;
    policy.set(GenericFamily::Math, "", None, &["Noto Sans Math", "Noto Sans"])?;
    for (name, families) in [
        ("Arial", &["Liberation Sans", "Arimo", "Albany", "Albany AMT"][..]),
        ("Helvetica", &["Liberation Sans", "Arimo", "Nimbus Sans"][..]),
        ("Helvetica Neue", &["Liberation Sans", "Arimo", "Nimbus Sans"][..]),
        ("Times New Roman", &["Liberation Serif", "Tinos", "Thorndale", "Thorndale AMT"][..]),
        ("Times", &["Liberation Serif", "Tinos", "Nimbus Roman"][..]),
        ("Courier New", &["Liberation Mono", "Cousine", "Cumberland", "Cumberland AMT"][..]),
        ("Courier", &["Liberation Mono", "Cousine", "Nimbus Mono PS"][..]),
        ("Calibri", &["Carlito"][..]),
        ("Cambria", &["Caladea"][..]),
        ("Georgia", &["Gelasio"][..]),
        ("Symbol", &["SymbolNeu", "Standard Symbols PS"][..]),
    ] {
        policy.alias(name, families)?;
    }
    Ok(policy)
}
