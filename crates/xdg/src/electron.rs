//! Electron programs (Slack, VS Code, Discord) started on an agent screen
//! get a config folder of the screen's own (`XDG_CONFIG_HOME`, beside its
//! history, as Chrome's profile is): their single-instance lock is in
//! their config, so one started there found the person's running copy
//! and had it open the window on their display. (The screen's own bus
//! does that for GTK's programs; Electron's lock isn't on the bus.) So
//! on a screen they start as the screen's: signed out, their own.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// `program` (a path, or a name on PATH) is an Electron app: its
/// `resources/app.asar` (or `resources/app`) beside it once links are
/// followed, or a launcher script that names one (VS Code's `code`).
pub fn is_electron(program: &OsStr) -> bool {
    let Some(path) = resolve(program) else { return false };
    let Ok(real) = std::fs::canonicalize(&path) else { return false };
    if let Some(dir) = real.parent()
        && (dir.join("resources/app.asar").exists() || dir.join("resources/app").is_dir())
    {
        return true;
    }
    // A script: what it runs.
    let mut head = vec![0u8; 8192];
    let n = std::fs::File::open(&real).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    let head = &head[..n];
    head.starts_with(b"#!") && (contains(head, b"ELECTRON_RUN_AS_NODE") || contains(head, b"resources/app"))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn resolve(program: &OsStr) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.components().count() > 1 {
        return Some(p.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(p)).find(|c| c.is_file())
}

/// Where Electron programs keep their config on the agent screen this
/// process is on (None: not on one).
pub fn screen_config() -> Option<PathBuf> {
    let screen = std::env::var_os("OLO_SCREEN").filter(|s| !s.is_empty())?;
    Some(crate::state_dir().join("screens").join(screen).join("config"))
}

/// Give `cmd` (starting `program`) the screen's config folder, if it's an
/// Electron app started on a screen.
pub fn on_screen(cmd: &mut std::process::Command, program: &OsStr) {
    if let Some(config) = screen_config()
        && is_electron(program)
    {
        let _ = std::fs::create_dir_all(&config);
        cmd.env("XDG_CONFIG_HOME", config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn electron_apps_known_by_their_files() {
        let dir = std::env::temp_dir().join(format!("olo-electron-test-{}", std::process::id()));
        let app = dir.join("slack");
        std::fs::create_dir_all(dir.join("resources")).unwrap();
        std::fs::write(&app, b"\x7fELF").unwrap();
        assert!(!is_electron(app.as_os_str()));
        std::fs::write(dir.join("resources/app.asar"), b"").unwrap();
        assert!(is_electron(app.as_os_str()));
        let script = dir.join("code");
        std::fs::write(&script, b"#!/usr/bin/env sh\nELECTRON_RUN_AS_NODE=1 \"$VSCODE_PATH/code\" \"$CLI\" \"$@\"\n").unwrap();
        assert!(is_electron(script.as_os_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
