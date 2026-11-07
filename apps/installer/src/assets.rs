//! What Setup carries of Olo's data (build.rs: the fonts, the icons its
//! pages show, the wordmark), unpacked where the kit is pointed at it
//! (`OLO_DATA`): none of Olo is installed when Setup starts.

use std::path::PathBuf;

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// Unpack it (what's there already, the same size, kept); where it is.
pub fn unpack() -> Option<PathBuf> {
    let dir = olo_xdg::runtime_dir().join("olo-installer-data");
    for (path, bytes) in ASSETS {
        let to = dir.join(path);
        if std::fs::metadata(&to).is_ok_and(|m| m.len() == bytes.len() as u64) {
            continue;
        }
        std::fs::create_dir_all(to.parent()?).ok()?;
        std::fs::write(&to, bytes).ok()?;
    }
    Some(dir)
}
