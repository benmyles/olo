//! What other desktops' programs expect of a graphical session. systemd's
//! `graphical-session.target` is how services tied to one (the desktop
//! portals, which GTK 4, libadwaita, Chrome and Flatpak apps read the
//! colour scheme, file choosers and more through) know there is one: they
//! refuse to start without it. It can't be started by hand, only pulled in
//! by a session's own target, as compositors' `<name>-session.target`s do.
//! Which portal backends serve a Tabula session is set in
//! `tabula-portals.conf` (xdg-desktop-portal picks the file by
//! `XDG_CURRENT_DESKTOP`).
//!
//! Only a real session does this: it changes the user's systemd manager.

use std::path::{Path, PathBuf};

use crate::log;

const TARGET: &str = "tabula-session.target";

const TARGET_UNIT: &str = "\
# Written by tabula-session: the Tabula desktop is running.
[Unit]
Description=Tabula session
Documentation=https://github.com/benmyles/tabula
BindsTo=graphical-session.target
Before=graphical-session.target
Wants=graphical-session-pre.target
After=graphical-session-pre.target
";

/// GTK's backend for what it has (its Settings read the GSettings
/// app_server writes: colour scheme, accent, fonts); the keyring for
/// secrets, as on GNOME.
const PORTALS: &str = "\
# Written by tabula-session: the desktop portals for Tabula.
[preferred]
default=gtk
org.freedesktop.impl.portal.Secret=gnome-keyring
";

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))
}

/// Write `text` to `path` unless it is there already; whether it changed.
fn put(path: &Path, text: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|t| t == text) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// Set up the files under `config` (`~/.config`); whether the unit is new.
fn write_files(config: &Path) -> std::io::Result<bool> {
    put(&config.join("xdg-desktop-portal").join("tabula-portals.conf"), PORTALS)?;
    put(&config.join("systemd").join("user").join(TARGET), TARGET_UNIT)
}

/// Begin the graphical session (blocks on systemd: call it off the main
/// thread), after the environment is exported, so the portals it lets
/// start find our display.
pub fn start() {
    let Some(config) = config_home() else { return };
    let new = match write_files(&config) {
        Ok(new) => new,
        Err(e) => return log(format!("could not set up the graphical session: {e}")),
    };
    if new && let Err(e) = tabula_system::env::reload_units() {
        log(format!("could not reload systemd units: {e}"));
    }
    if let Err(e) = tabula_system::env::start_unit(TARGET, true) {
        log(format!("could not start {TARGET}: {e}"));
    }
}

/// End it (the portals and other session services stop with it), and
/// take away the colours GTK 4 imports while Tabula runs (app_server's
/// toolkits.rs).
pub fn stop() {
    let _ = std::fs::remove_file(crate::runtime_dir().join("tabula").join("gtk-4.0.css"));
    if let Err(e) = tabula_system::env::start_unit(TARGET, false) {
        log(format!("could not stop {TARGET}: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_written_once() {
        let dir = std::env::temp_dir().join(format!("tabula-graphical-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(write_files(&dir).unwrap());
        assert!(!write_files(&dir).unwrap());
        let unit = std::fs::read_to_string(dir.join("systemd/user/tabula-session.target")).unwrap();
        assert!(unit.contains("BindsTo=graphical-session.target"));
        let conf = std::fs::read_to_string(dir.join("xdg-desktop-portal/tabula-portals.conf")).unwrap();
        assert!(conf.starts_with("# Written") && conf.contains("default=gtk"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
