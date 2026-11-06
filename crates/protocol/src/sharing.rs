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
    /// What of the computer it controls ("keyboard", "pointer", "touch
    /// screen"; the RemoteDesktop portal), or nothing (a share alone).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub control: Vec<String>,
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

    /// "Studio Display with Google Chrome": what a share is, said ("Studio
    /// Display and control with Chrome Remote Desktop", "control of your
    /// keyboard and pointer with Deskflow").
    pub fn describe(share: &Share) -> String {
        let screens = match share.screens.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            many => Some(format!("{} screens", many.len())),
        };
        let what = match (screens, share.control.is_empty()) {
            (Some(s), true) => s,
            (None, true) => "your screen".to_owned(),
            (Some(s), false) => format!("{s} and control"),
            (None, false) => format!("control of your {}", and_list(&share.control)),
        };
        if share.app.is_empty() { what } else { format!("{what} with {}", share.app) }
    }
}

impl SharingState {
    /// What a share is, as a sentence for Deskbar ("Sharing Studio Display
    /// with Google Chrome", "Deskflow is controlling your keyboard and
    /// pointer", "Zoom sees Studio Display and controls your pointer").
    pub fn said(share: &Share) -> String {
        if share.control.is_empty() {
            return format!("Sharing {}", Self::describe(share));
        }
        let who = if share.app.is_empty() { "An app" } else { share.app.as_str() };
        let control = and_list(&share.control);
        match share.screens.as_slice() {
            [] => format!("{who} is controlling your {control}"),
            [one] => format!("{who} sees {one} and controls your {control}"),
            many => format!("{who} sees {} screens and controls your {control}", many.len()),
        }
    }
}

/// "a, b and c".
pub fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_said() {
        let s = Share { session: "/s".into(), app: "Google Chrome".into(), screens: vec!["Studio Display".into()], since: 0, windows: Vec::new(), control: Vec::new() };
        assert_eq!(SharingState::describe(&s), "Studio Display with Google Chrome");
        let s = Share { app: String::new(), screens: vec!["A".into(), "B".into()], ..s };
        assert_eq!(SharingState::describe(&s), "2 screens");
        // Control too, or alone.
        let s = Share { app: "Deskflow".into(), screens: vec!["Studio Display".into()], control: vec!["keyboard".into(), "pointer".into()], ..s };
        assert_eq!(SharingState::describe(&s), "Studio Display and control with Deskflow");
        let s = Share { screens: Vec::new(), ..s };
        assert_eq!(SharingState::describe(&s), "control of your keyboard and pointer with Deskflow");
        assert_eq!(SharingState::said(&s), "Deskflow is controlling your keyboard and pointer");
        let s = Share { screens: vec!["Studio Display".into()], ..s };
        assert_eq!(SharingState::said(&s), "Deskflow sees Studio Display and controls your keyboard and pointer");
    }
}
