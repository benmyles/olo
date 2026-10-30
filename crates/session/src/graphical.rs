//! What other desktops' programs expect of a graphical session. systemd's
//! `graphical-session.target` is how services tied to one (the desktop
//! portals, which GTK 4, libadwaita, Chrome and Flatpak apps read the
//! colour scheme, file choosers and more through) know there is one: they
//! refuse to start without it. It can't be started by hand, only pulled in
//! by a session's own target, as compositors' `<name>-session.target`s do.
//! Which portal backends serve an Olo session is set in
//! `olo-portals.conf` (xdg-desktop-portal picks the file by the first
//! name in `XDG_CURRENT_DESKTOP` that has one, so `Olo:GNOME` still
//! finds Olo's); Olo's own backend (olo-portal: screen sharing) is
//! described to it by `olo.portal` in `~/.local/share/xdg-desktop-portal/
//! portals`. xdg-desktop-portal reads both when it starts, so it is
//! restarted (if running) when they change.
//!
//! Only a real session does this: it changes the user's systemd manager.

use std::path::{Path, PathBuf};

use crate::log;

const TARGET: &str = "olo-session.target";
/// What the target was called before the rename (Tabula); a session that
/// started on that build upgrades in place with it running.
const OLD_TARGET: &str = "tabula-session.target";

const TARGET_UNIT: &str = "\
# Written by olo-session: the Olo desktop is running.
[Unit]
Description=Olo Desktop session
Documentation=https://github.com/benmyles/olo
BindsTo=graphical-session.target
Before=graphical-session.target
Wants=graphical-session-pre.target
After=graphical-session-pre.target
";

/// GTK's backend for what it has (its Settings read the GSettings
/// app_server writes: colour scheme, accent, fonts); the keyring for
/// secrets, as on GNOME; ours for screen sharing, screenshots, keeping
/// awake, Open and Save panels, Open With, and the permission dialogs
/// other portals ask through.
const PORTALS: &str = "\
# Written by olo-session: the desktop portals for Olo.
[preferred]
default=gtk
org.freedesktop.impl.portal.Secret=gnome-keyring
org.freedesktop.impl.portal.ScreenCast=olo
org.freedesktop.impl.portal.Inhibit=olo
org.freedesktop.impl.portal.Screenshot=olo
org.freedesktop.impl.portal.Access=olo
org.freedesktop.impl.portal.FileChooser=olo
org.freedesktop.impl.portal.AppChooser=olo
";

/// Olo's backend (olo-portal, which olo-session runs), for
/// xdg-desktop-portal to find.
const OLO_PORTAL: &str = "\
# Written by olo-session: Olo's portal backend (olo-portal).
[portal]
DBusName=org.freedesktop.impl.portal.desktop.olo
Interfaces=org.freedesktop.impl.portal.ScreenCast;org.freedesktop.impl.portal.Inhibit;org.freedesktop.impl.portal.Screenshot;org.freedesktop.impl.portal.Access;org.freedesktop.impl.portal.FileChooser;org.freedesktop.impl.portal.AppChooser;
";

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share")))
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

/// What changed in setting up the files under `config` (`~/.config`)
/// and `data` (`~/.local/share`).
struct Written {
    /// The session's target unit is new.
    unit: bool,
    /// The portals' configuration (or our backend's description) is.
    portals: bool,
}

fn write_files(config: &Path, data: &Path) -> std::io::Result<Written> {
    // The ones from before the rename (a new session is never Tabula).
    let _ = std::fs::remove_file(config.join("xdg-desktop-portal").join("tabula-portals.conf"));
    let _ = std::fs::remove_file(config.join("systemd").join("user").join(OLD_TARGET));
    let conf = put(&config.join("xdg-desktop-portal").join("olo-portals.conf"), PORTALS)?;
    let backend = put(&data.join("xdg-desktop-portal").join("portals").join("olo.portal"), OLO_PORTAL)?;
    let unit = put(&config.join("systemd").join("user").join(TARGET), TARGET_UNIT)?;
    Ok(Written { unit, portals: conf || backend })
}

/// Begin the graphical session (blocks on systemd: call it off the main
/// thread), after the environment is exported, so the portals it lets
/// start find our display.
pub fn start() {
    let (Some(config), Some(data)) = (config_home(), data_home()) else { return };
    let written = match write_files(&config, &data) {
        Ok(w) => w,
        Err(e) => return log(format!("could not set up the graphical session: {e}")),
    };
    if written.unit && let Err(e) = olo_system::env::reload_units() {
        log(format!("could not reload systemd units: {e}"));
    }
    if let Err(e) = olo_system::env::start_unit(TARGET, true) {
        log(format!("could not start {TARGET}: {e}"));
    }
    // A portal started before (by an app) still has the old backends.
    if written.portals && let Err(e) = olo_system::env::try_restart_unit("xdg-desktop-portal.service") {
        log(format!("could not restart xdg-desktop-portal: {e}"));
    }
}

/// End it (the portals and other session services stop with it), and
/// take away the colours GTK 4 imports while Olo runs (app_server's
/// toolkits.rs).
pub fn stop() {
    let _ = std::fs::remove_file(crate::runtime_dir().join("olo").join("gtk-4.0.css"));
    let _ = std::fs::remove_file(crate::runtime_dir().join(olo_xdg::legacy::OLD).join("gtk-4.0.css"));
    if let Err(e) = olo_system::env::start_unit(TARGET, false) {
        log(format!("could not stop {TARGET}: {e}"));
    }
    // Started by this session before it was upgraded from Tabula.
    let _ = olo_system::env::start_unit(OLD_TARGET, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_written_once() {
        let dir = std::env::temp_dir().join(format!("olo-graphical-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (config, data) = (dir.join("config"), dir.join("data"));
        let first = write_files(&config, &data).unwrap();
        assert!(first.unit && first.portals);
        let again = write_files(&config, &data).unwrap();
        assert!(!again.unit && !again.portals);
        let unit = std::fs::read_to_string(config.join("systemd/user/olo-session.target")).unwrap();
        assert!(unit.contains("BindsTo=graphical-session.target"));
        let conf = std::fs::read_to_string(config.join("xdg-desktop-portal/olo-portals.conf")).unwrap();
        assert!(conf.starts_with("# Written") && conf.contains("default=gtk") && conf.contains("ScreenCast=olo"));
        let backend = std::fs::read_to_string(data.join("xdg-desktop-portal/portals/olo.portal")).unwrap();
        assert!(backend.contains("DBusName=org.freedesktop.impl.portal.desktop.olo"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
