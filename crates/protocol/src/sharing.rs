//! What olo-portal is sharing: a small JSON file in the runtime directory
//! (per display), rewritten on every change and gone when nothing is
//! shared, which Deskbar's tray shows with Stop (`olo-portal --stop
//! SESSION`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharingState {
    /// olo-portal's process: a file it left behind when it died (its
    /// shares with it) is no one's.
    pub pid: u32,
    pub shares: Vec<Share>,
}

/// One app's share.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Share {
    /// The portal session (its object path), which Stop names.
    pub session: String,
    /// Who is sharing ("" when the portal couldn't tell).
    pub app: String,
    /// What is shared, as people call it ("Studio Display").
    pub screens: Vec<String>,
    /// Since when, seconds since the epoch.
    pub since: u64,
    /// The windows of it that are windows (app_server's ids).
    pub windows: Vec<u64>,
}

impl SharingState {
    pub fn path() -> std::path::PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join(format!("olo-sharing-{}.json", olo_xdg::display_name()))
    }

    /// What is shared now (nothing, if the portal that wrote it is gone).
    pub fn load() -> SharingState {
        let state: SharingState = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if state.pid != 0 && !std::path::Path::new(&format!("/proc/{}", state.pid)).exists() {
            return SharingState::default();
        }
        state
    }

    /// Write it (atomically), or remove the file when nothing is shared.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if self.shares.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }

    /// "Studio Display with Google Chrome": what a share is, said.
    pub fn describe(share: &Share) -> String {
        let what = match share.screens.as_slice() {
            [] => "your screen".to_owned(),
            [one] => one.clone(),
            many => format!("{} screens", many.len()),
        };
        if share.app.is_empty() { what } else { format!("{what} with {}", share.app) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_said() {
        let s = Share { session: "/s".into(), app: "Google Chrome".into(), screens: vec!["Studio Display".into()], since: 0, windows: Vec::new() };
        assert_eq!(SharingState::describe(&s), "Studio Display with Google Chrome");
        let s = Share { app: String::new(), screens: vec!["A".into(), "B".into()], ..s };
        assert_eq!(SharingState::describe(&s), "2 screens");
    }
}
