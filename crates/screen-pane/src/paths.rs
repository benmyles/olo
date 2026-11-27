//! Shared runtime paths for Rex's screens service (state file + Unix socket),
//! following the same per-display naming and test-override convention as
//! `olo_kit_base::tracker::rex_files_socket`.

use std::path::PathBuf;

fn safe_display() -> String {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    display.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect()
}

/// Directory where Rex publishes its connected hosts' screen state and socket
/// (`$XDG_RUNTIME_DIR/olo-rex-screens`, or overridden by `OLO_REX_SCREENS_DIR`).
pub fn rex_screens_dir() -> PathBuf {
    if let Ok(path) = std::env::var("OLO_REX_SCREENS_DIR")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    olo_xdg::runtime_dir().join("olo-rex-screens")
}

/// Atomic state snapshot written by Rex for this Wayland display:
/// `$XDG_RUNTIME_DIR/olo-rex-screens/state-<WAYLAND_DISPLAY>.json`.
pub fn rex_screens_state_path() -> PathBuf {
    rex_screens_dir().join(format!("state-{}.json", safe_display()))
}

/// Mode `0600` Unix socket where Rex serves live streams, input, history, and
/// lifecycle actions for remote hosts' screens to `olo-screen`:
/// `$XDG_RUNTIME_DIR/olo-rex-screens/rex-<WAYLAND_DISPLAY>.sock`
/// (or overridden by `OLO_REX_SCREENS_SOCKET`).
pub fn rex_screens_socket() -> PathBuf {
    if let Ok(path) = std::env::var("OLO_REX_SCREENS_SOCKET")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    rex_screens_dir().join(format!("rex-{}.sock", safe_display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_live_under_rex_screens_dir() {
        let dir = rex_screens_dir();
        let state = rex_screens_state_path();
        let sock = rex_screens_socket();
        assert_eq!(state.parent(), Some(dir.as_path()));
        assert!(state.file_name().unwrap().to_str().unwrap().starts_with("state-"));
        assert!(state.extension().is_some_and(|e| e == "json"));
        if std::env::var_os("OLO_REX_SCREENS_SOCKET").is_none() {
            assert_eq!(sock.parent(), Some(dir.as_path()));
            assert!(sock.file_name().unwrap().to_str().unwrap().starts_with("rex-"));
            assert!(sock.extension().is_some_and(|e| e == "sock"));
        }
    }
}
