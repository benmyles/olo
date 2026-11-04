//! What Olo Doctor last found, for Deskbar's tray (a doctor's bag there
//! while there is something to fix; a click opens Olo Doctor). Written by
//! every look Olo Doctor takes (at login and every 15 minutes after, its
//! window's, its command line's), in the runtime directory; Deskbar
//! follows it by inotify.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DoctorFound {
    /// What gets in Olo's way, or can be fixed with a button: each
    /// finding's title.
    pub issues: Vec<String>,
    /// How many of them are problems (the rest are fixes suggested).
    pub problems: usize,
    /// When it looked (seconds since the epoch).
    pub at: u64,
}

impl DoctorFound {
    pub fn path() -> std::path::PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join("olo-doctor-found.json")
    }

    pub fn load() -> DoctorFound {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Written whole (a sibling renamed into place), so a reader never sees
    /// half of it.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
}
