//! The sound choices Olo keeps itself: None as the output (sound goes
//! nowhere: `olo_system::audio::set_no_output`), made again at login
//! (Deskbar), since the sound server forgets the silent output it is.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundChoice {
    /// None was chosen as the output (and nothing since).
    pub no_output: bool,
}

impl SoundChoice {
    pub fn path() -> PathBuf {
        olo_xdg::config_dir().join("sound.json")
    }

    pub fn load() -> SoundChoice {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
}
