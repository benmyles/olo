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

/// Every portal interface named, so none falls to another desktop's
/// backends: Olo's portal for what it serves (screen sharing and remote
/// control, screenshots, keeping awake, Open and Save panels, Open With,
/// the permission dialogs other portals ask through, the desktop's look
/// as Settings, notifications), the keyring (olo-secrets, or GNOME
/// Keyring when Olo's is turned off: `OLO_SECRETS_PORTAL`) for programs'
/// secrets, and none for the rest (no backend: the portal says the
/// interface isn't there).
const PORTALS: &str = "\
# Written by olo-session: the desktop portals for Olo.
[preferred]
default=none
org.freedesktop.impl.portal.Secret=olo-secrets
org.freedesktop.impl.portal.Settings=olo
org.freedesktop.impl.portal.Notification=olo
org.freedesktop.impl.portal.ScreenCast=olo
org.freedesktop.impl.portal.RemoteDesktop=olo
org.freedesktop.impl.portal.Inhibit=olo
org.freedesktop.impl.portal.Screenshot=olo
org.freedesktop.impl.portal.Access=olo
org.freedesktop.impl.portal.Account=olo
org.freedesktop.impl.portal.FileChooser=olo
org.freedesktop.impl.portal.AppChooser=olo
org.freedesktop.impl.portal.Background=olo
org.freedesktop.impl.portal.Email=olo
org.freedesktop.impl.portal.Wallpaper=olo
org.freedesktop.impl.portal.GlobalShortcuts=olo
org.freedesktop.impl.portal.Print=olo
org.freedesktop.impl.portal.Lockdown=none
org.freedesktop.impl.portal.Clipboard=none
org.freedesktop.impl.portal.InputCapture=none
org.freedesktop.impl.portal.Usb=none
org.freedesktop.impl.portal.DynamicLauncher=none
";

/// xdg-desktop-portal reads the configuration of every desktop
/// `XDG_CURRENT_DESKTOP` names, and Olo's says `Olo:GNOME` (for Qt,
/// `olo_xdg::CURRENT_DESKTOP`): it gathered Settings from GNOME's and
/// GTK's backends as well as Olo's, starting GNOME's portal (and
/// gnome-control-center under it). So while an Olo session runs, the
/// portal service alone is told the desktop is Olo: a drop-in in the
/// user's runtime unit folder (`$XDG_RUNTIME_DIR/systemd/user`), gone
/// with the session (`stop`) or the login.
const PORTAL_DROP_IN: &str = "\
# Written by olo-session while an Olo session runs: the portals read
# Olo's configuration alone (olo-portals.conf), not GNOME's too.
[Service]
Environment=XDG_CURRENT_DESKTOP=Olo
";

/// Where [`PORTAL_DROP_IN`] goes, in the runtime directory `runtime`.
fn portal_drop_in(runtime: &Path) -> PathBuf {
    runtime.join("systemd/user/xdg-desktop-portal.service.d/olo.conf")
}

/// Olo's backend (olo-portal, which olo-session runs), for
/// xdg-desktop-portal to find.
const OLO_PORTAL: &str = "\
# Written by olo-session: Olo's portal backend (olo-portal).
[portal]
DBusName=org.freedesktop.impl.portal.desktop.olo
Interfaces=org.freedesktop.impl.portal.ScreenCast;org.freedesktop.impl.portal.RemoteDesktop;org.freedesktop.impl.portal.Inhibit;org.freedesktop.impl.portal.Screenshot;org.freedesktop.impl.portal.Access;org.freedesktop.impl.portal.Account;org.freedesktop.impl.portal.FileChooser;org.freedesktop.impl.portal.AppChooser;org.freedesktop.impl.portal.Background;org.freedesktop.impl.portal.Email;org.freedesktop.impl.portal.Wallpaper;org.freedesktop.impl.portal.GlobalShortcuts;org.freedesktop.impl.portal.Print;org.freedesktop.impl.portal.Settings;org.freedesktop.impl.portal.Notification;
";

/// The keyring's portal backend: whoever answers for the keyring
/// (`org.freedesktop.secrets`), olo-secrets or, turned off, GNOME
/// Keyring, which serves the same interface at the same path.
const OLO_SECRETS_PORTAL: &str = "\
# Written by olo-session: the keyring's portal backend (olo-secrets).
[portal]
DBusName=org.freedesktop.secrets
Interfaces=org.freedesktop.impl.portal.Secret;
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

fn write_files(config: &Path, data: &Path, runtime: &Path) -> std::io::Result<Written> {
    // The ones from before the rename (a new session is never Tabula).
    let _ = std::fs::remove_file(config.join("xdg-desktop-portal").join("tabula-portals.conf"));
    let _ = std::fs::remove_file(config.join("systemd").join("user").join(OLD_TARGET));
    let conf = put(&config.join("xdg-desktop-portal").join("olo-portals.conf"), PORTALS)?;
    let backend = put(&data.join("xdg-desktop-portal").join("portals").join("olo.portal"), OLO_PORTAL)?;
    let backend = put(&data.join("xdg-desktop-portal").join("portals").join("olo-secrets.portal"), OLO_SECRETS_PORTAL)? || backend;
    let drop_in = put(&portal_drop_in(runtime), PORTAL_DROP_IN)?;
    let unit = put(&config.join("systemd").join("user").join(TARGET), TARGET_UNIT)? || drop_in;
    if let Some(bin_dir) = crate::exe().parent() {
        open_for_others(bin_dir, config, &data.join("applications"))?;
        command_names(bin_dir);
    }
    Ok(Written { unit, portals: conf || backend || drop_in })
}

/// `pbcopy` and `pbpaste` on PATH, as macOS has them: links to Olo's
/// (`olo-pbcopy`, `olo-pbpaste`) beside them in `bin_dir` (first on the
/// session's PATH), never over a program of that name already there.
fn command_names(bin_dir: &Path) {
    for (name, ours) in [("pbcopy", "olo-pbcopy"), ("pbpaste", "olo-pbpaste")] {
        let link = bin_dir.join(name);
        if bin_dir.join(ours).is_file() && std::fs::symlink_metadata(&link).is_err() {
            let _ = std::os::unix::fs::symlink(ours, &link);
        }
    }
}

/// Let other programs open files in Olo's apps, as Olo does: the hidden
/// desktop entries and `olo-mimeapps.list` `olo_xdg::open::for_others`
/// gives, in `apps` (`~/.local/share/applications`, whose
/// `mimeinfo.cache` is made again when they change, as
/// update-desktop-database would) and `config`; entries written before
/// and no longer wanted go. Olo's Chrome, once installed, is offered as
/// the default browser (`olo_xdg::open::browser_offer`).
fn open_for_others(bin_dir: &Path, config: &Path, apps: &Path) -> std::io::Result<()> {
    let (entries, defaults) = olo_xdg::open::for_others(bin_dir);
    let mut changed = false;
    for (name, text) in &entries {
        changed |= put(&apps.join(name), text)?;
    }
    if let Ok(rd) = std::fs::read_dir(apps) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("olo.")
                && name.ends_with(".desktop")
                && !entries.iter().any(|(n, _)| *n == name)
                && std::fs::read_to_string(e.path()).is_ok_and(|t| t.lines().any(|l| l == olo_xdg::open::WRITTEN))
            {
                std::fs::remove_file(e.path())?;
                changed = true;
            }
        }
    }
    if changed {
        put(&apps.join("mimeinfo.cache"), &olo_xdg::open::mime_cache(apps))?;
    }
    put(&config.join("olo-mimeapps.list"), &defaults)?;
    olo_xdg::open::offer_browser(apps)?;
    Ok(())
}

/// Begin the graphical session (blocks on systemd: call it off the main
/// thread), after the environment is exported, so the portals it lets
/// start find our display.
pub fn start() {
    let (Some(config), Some(data)) = (config_home(), data_home()) else { return };
    let written = match write_files(&config, &data, &crate::runtime_dir()) {
        Ok(w) => w,
        Err(e) => return log(format!("could not set up the graphical session: {e}")),
    };
    if written.unit
        && let Err(e) = olo_system::env::reload_units()
    {
        log(format!("could not reload systemd units: {e}"));
    }
    if let Err(e) = olo_system::env::start_unit(TARGET, true) {
        log(format!("could not start {TARGET}: {e}"));
    }
    // A portal started before (by an app) still has the old backends.
    if written.portals
        && let Err(e) = olo_system::env::try_restart_unit("xdg-desktop-portal.service")
    {
        log(format!("could not restart xdg-desktop-portal: {e}"));
    }
    if let Some(bin) = crate::exe().parent() {
        take_dconf(bin, &crate::runtime_dir());
    }
}

/// Olo's dconf service (olo-dconf) for `ca.desrt.dconf`, in GNOME's
/// dconf-service's place: D-Bus starts it on demand, as it would GNOME's.
fn dconf_service(bin: &Path) -> String {
    format!("[D-BUS Service]\nName=ca.desrt.dconf\nExec={}\n", bin.join("olo-dconf").display())
}

/// Where it's described while an Olo session runs: the runtime folder's
/// services, which the session bus reads before `~/.local/share`'s and
/// the system's (tried with dbus-broker 37: the same name in both, the
/// runtime folder's started), and which go with the login, so a GNOME
/// session later has GNOME's again.
fn dconf_service_path(runtime: &Path) -> PathBuf {
    runtime.join("dbus-1/services/ca.desrt.dconf.service")
}

fn reload_bus() {
    if let Err(e) = olo_system::env::reload_bus_services() {
        log(format!("could not reload the session bus's services: {e}"));
    }
}

/// Settings kept in dconf go through Olo's service from now on: it's
/// named for `ca.desrt.dconf` (the bus told to read its services again),
/// and GNOME's, if it answered before (an app wrote a setting before the
/// session got this far), is stopped, so the next change starts Olo's.
fn take_dconf(bin: &Path, runtime: &Path) {
    if !bin.join("olo-dconf").is_file() {
        return;
    }
    match put(&dconf_service_path(runtime), &dconf_service(bin)) {
        Ok(true) => reload_bus(),
        Ok(false) => {}
        Err(e) => return log(format!("could not name Olo's dconf service: {e}")),
    }
    let gnome = olo_system::env::bus_name_pid("ca.desrt.dconf").filter(|pid| std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| exe.file_name().is_some_and(|n| n == "dconf-service")));
    if let Some(pid) = gnome {
        log(format!("GNOME's dconf-service (pid {pid}) was answering for dconf: stopped, Olo's answers from the next change"));
        if let Err(e) = olo_system::env::start_unit("dconf.service", false) {
            log(format!("could not stop dconf.service: {e}"));
        }
    }
}

/// End it (the portals and other session services stop with it), and
/// take away the colours GTK 4 imports while Olo runs (app_server's
/// toolkits.rs).
pub fn stop() {
    let _ = std::fs::remove_file(crate::runtime_dir().join("olo").join("gtk-4.0.css"));
    let _ = std::fs::remove_file(portal_drop_in(&crate::runtime_dir()));
    if std::fs::remove_file(dconf_service_path(&crate::runtime_dir())).is_ok() {
        reload_bus();
    }
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
        let (config, data, runtime) = (dir.join("config"), dir.join("data"), dir.join("run"));
        let first = write_files(&config, &data, &runtime).unwrap();
        assert!(first.unit && first.portals);
        let again = write_files(&config, &data, &runtime).unwrap();
        assert!(!again.unit && !again.portals);
        let unit = std::fs::read_to_string(config.join("systemd/user/olo-session.target")).unwrap();
        assert!(unit.contains("BindsTo=graphical-session.target"));
        let conf = std::fs::read_to_string(config.join("xdg-desktop-portal/olo-portals.conf")).unwrap();
        assert!(conf.starts_with("# Written") && conf.contains("default=none") && conf.contains("ScreenCast=olo") && conf.contains("Settings=olo"));
        // Every interface the portal has is named (none falls to GNOME's).
        for iface in [
            "Access",
            "Account",
            "AppChooser",
            "Background",
            "Clipboard",
            "DynamicLauncher",
            "Email",
            "FileChooser",
            "GlobalShortcuts",
            "Inhibit",
            "InputCapture",
            "Lockdown",
            "Notification",
            "Print",
            "RemoteDesktop",
            "ScreenCast",
            "Screenshot",
            "Secret",
            "Settings",
            "Usb",
            "Wallpaper",
        ] {
            assert!(conf.contains(&format!("\norg.freedesktop.impl.portal.{iface}=")), "{iface}");
        }
        let secrets = std::fs::read_to_string(data.join("xdg-desktop-portal/portals/olo-secrets.portal")).unwrap();
        assert!(conf.contains("\norg.freedesktop.impl.portal.Secret=olo-secrets\n") && secrets.contains("DBusName=org.freedesktop.secrets\nInterfaces=org.freedesktop.impl.portal.Secret;"));
        let backend = std::fs::read_to_string(data.join("xdg-desktop-portal/portals/olo.portal")).unwrap();
        assert!(backend.contains("DBusName=org.freedesktop.impl.portal.desktop.olo"));
        // What the config sends to Olo's backend, it says it has.
        for line in conf.lines().filter(|l| l.ends_with("=olo")) {
            let iface = line.trim_end_matches("=olo");
            assert!(backend.contains(&format!("{iface};")), "{iface}");
        }
        let drop_in = std::fs::read_to_string(portal_drop_in(&runtime)).unwrap();
        assert!(drop_in.contains("[Service]\nEnvironment=XDG_CURRENT_DESKTOP=Olo\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dconf_is_olo_s_while_it_runs() {
        let text = dconf_service(Path::new("/home/u/.local/bin"));
        assert_eq!(text, "[D-BUS Service]\nName=ca.desrt.dconf\nExec=/home/u/.local/bin/olo-dconf\n");
        assert_eq!(dconf_service_path(Path::new("/run/user/1000")), Path::new("/run/user/1000/dbus-1/services/ca.desrt.dconf.service"));
    }

    #[test]
    fn others_open_files_in_olo() {
        let dir = std::env::temp_dir().join(format!("olo-graphical-others-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (bin, config, apps) = (dir.join("bin"), dir.join("config"), dir.join("applications"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(bin.join("olo-viewer"), "").unwrap();
        // Another app's entry, one we wrote for an app no longer there, and
        // the person's own that happens to start "olo.".
        std::fs::write(apps.join("okular.desktop"), "[Desktop Entry]\nType=Application\nName=Okular\nMimeType=application/pdf;\n").unwrap();
        std::fs::write(apps.join("olo.tracker.desktop"), format!("[Desktop Entry]\nType=Application\nName=Tracker\nMimeType=inode/directory;\n{}\n", olo_xdg::open::WRITTEN)).unwrap();
        std::fs::write(apps.join("olo.mine.desktop"), "[Desktop Entry]\nType=Application\nName=Mine\n").unwrap();
        open_for_others(&bin, &config, &apps).unwrap();
        assert!(apps.join("olo.viewer.desktop").is_file() && apps.join("olo.mine.desktop").is_file());
        assert!(!apps.join("olo.tracker.desktop").exists());
        let cache = std::fs::read_to_string(apps.join("mimeinfo.cache")).unwrap();
        let defaults = std::fs::read_to_string(config.join("olo-mimeapps.list")).unwrap();
        if !olo_xdg::mime::known().is_empty() {
            assert!(cache.contains("\napplication/pdf=okular.desktop;olo.viewer.desktop;\n"), "{cache}");
            assert!(!cache.contains("inode/directory"));
            assert!(defaults.contains("\napplication/pdf=olo.viewer.desktop;\n"));
        }
        // No Chrome, no browser offered; once it is installed, it is.
        assert!(!apps.join("olo-mimeapps.list").exists());
        std::fs::write(apps.join(olo_xdg::open::CHROME_ENTRY), "[Desktop Entry]\nType=Application\nName=Google Chrome\n").unwrap();
        open_for_others(&bin, &config, &apps).unwrap();
        let offer = std::fs::read_to_string(apps.join("olo-mimeapps.list")).unwrap();
        assert!(offer.contains("\nx-scheme-handler/https=google-chrome.desktop;\n") && !defaults.contains("x-scheme-handler"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// pbcopy and pbpaste are Olo's, beside them, unless the person has
    /// their own.
    #[test]
    fn pbcopy_and_pbpaste_named() {
        let bin = std::env::temp_dir().join(format!("olo-graphical-pb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bin);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("olo-pbcopy"), "").unwrap();
        std::fs::write(bin.join("olo-pbpaste"), "").unwrap();
        std::fs::write(bin.join("pbpaste"), "mine").unwrap();
        command_names(&bin);
        command_names(&bin);
        assert_eq!(std::fs::read_link(bin.join("pbcopy")).unwrap(), Path::new("olo-pbcopy"));
        assert_eq!(std::fs::read_to_string(bin.join("pbpaste")).unwrap(), "mine");
        let _ = std::fs::remove_dir_all(&bin);
    }
}
