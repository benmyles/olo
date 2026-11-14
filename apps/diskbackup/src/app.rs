//! DiskBackup's app (on olo-kit): its windows, each a view on a looper of
//! its own. The main one (`window.rs`) keeps the service's state and the
//! settings, live (their files followed by inotify here, for every
//! window: nothing ticks while nothing happens), and what's done from it;
//! changes are written to the settings, which the service follows at
//! once. What outlives the windows is a [`Hub`] on the app's looper: which
//! are open, and what a launch asks.

use std::path::PathBuf;

use olo_kit::{WindowHandle, WindowOptions};
use olo_protocol::diskbackup::{Settings, State, Target};

use crate::{
    drive_setup::DriveSetup,
    restore::RestoreWindow,
    target,
    versions::VersionsWindow,
    window::{MainWindow, WinMsg},
};

/// What the hub is told: by the files' watch, a launch, and the windows.
pub enum Msg {
    /// The state or the settings changed (the files, followed).
    Changed,
    /// The Restore window (File ▸ Restore…).
    OpenRestore,
    /// The Google Drive setup window.
    OpenDrive,
    /// Google Drive was set up (by the setup window).
    SetUp,
}

/// A settings change, for Edit ▸ Undo: the settings before it and after.
/// Undo puts back only what it changed (the service or a setup may have
/// changed others since).
#[derive(Debug, Clone)]
pub struct Changed {
    pub before: Settings,
    pub after: Settings,
}

/// What a change is called (Edit ▸ Undo *Keep*), by what it changed.
pub(crate) fn step_name(a: &Settings, b: &Settings) -> &'static str {
    if a.paths != b.paths {
        if b.paths.len() > a.paths.len() { "Add Folder" } else { "Remove Folder" }
    } else if a.exclude_groups != b.exclude_groups {
        "Leave Out"
    } else if a.excludes != b.excludes {
        "Patterns"
    } else if a.automatic != b.automatic {
        if b.automatic { "Back Up Automatically" } else { "Back Up Only When Asked" }
    } else if a.every_minutes != b.every_minutes {
        "Back Up Every"
    } else if a.on_battery != b.on_battery {
        "Also on Battery"
    } else if a.keep != b.keep {
        "Keep"
    } else if a.upload_kbs != b.upload_kbs || a.download_kbs != b.download_kbs {
        "Speed"
    } else if a.priority != b.priority {
        "Priority"
    } else if a.in_deskbar != b.in_deskbar {
        "Show in Deskbar"
    } else {
        ""
    }
}

/// `now` with what `before` → `after` changed put back to `before`.
pub(crate) fn put_back(now: &mut Settings, before: &Settings, after: &Settings) {
    macro_rules! each {
        ($($f:ident),*) => {
            $(if before.$f != after.$f {
                now.$f = before.$f.clone();
            })*
        };
    }
    each!(enabled, target, paths, exclude_groups, excludes, automatic, every_minutes, keep, upload_kbs, download_kbs, on_battery, priority, one_file_system, key_in_passwords, in_deskbar);
}

/// The main window's: a form, at the size of what it shows (it floats,
/// never zoomed in Focus or put in a tile. Ben, 2026-11-08: "diskbackup
/// looks silly in focus mode"); fixed, as Preferences' panels: as tall as
/// the tab chosen shows.
pub fn main_options() -> WindowOptions {
    WindowOptions { fixed: true, ..WindowOptions::new("DiskBackup", 720.0, 300.0).fits_content().floating() }
}

/// The app's windows, kept from start to end on the app's looper.
pub struct Hub {
    win: Option<WindowHandle<MainWindow>>,
    /// The Restore window, while it's open.
    restore: Option<WindowHandle<RestoreWindow>>,
    /// Versions windows, a file each.
    versions: Vec<(PathBuf, WindowHandle<VersionsWindow>)>,
    /// The Google Drive setup window, while it's open.
    drive: Option<WindowHandle<DriveSetup>>,
    /// When a backup last finished whole, as last read.
    last_ok: Option<i64>,
    _watch: Option<olo_kit::watch::Watch>,
}

impl Hub {
    pub fn new() -> Hub {
        let changed = olo_kit::proxy();
        let watch = olo_kit::watch::watch([State::path(), Settings::path()], move |_| changed.send(Msg::Changed));
        Hub { win: None, restore: None, versions: Vec::new(), drive: None, last_ok: State::load().last_ok, _watch: watch }
    }

    pub fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        if let Some(launch) = message.downcast_ref::<olo_kit::instance::Launch>() {
            return self.launched(&launch.args, launch.cwd.as_deref());
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Changed => {
                let (settings, state) = (Settings::load(), State::load());
                // A backup finished: the Versions windows list again.
                if state.last_ok != self.last_ok {
                    self.last_ok = state.last_ok;
                    for (_, w) in &self.versions {
                        w.messenger().send(crate::versions::Done::Reload);
                    }
                }
                if let Some(w) = &self.win {
                    w.messenger().send(WinMsg::Changed(settings, state));
                }
            }
            Msg::OpenRestore => self.open_restore(),
            Msg::OpenDrive => self.open_drive(),
            Msg::SetUp => {
                if let Some(w) = &self.win {
                    w.messenger().send(WinMsg::SetUp);
                }
            }
        }
    }

    /// What a launch asks (its arguments): `--restore-window` (Deskbar's
    /// Restore Files…), `--versions-window PATH` (Tracker's Older
    /// Versions…), else the window.
    pub fn launched(&mut self, args: &[String], cwd: Option<&std::path::Path>) {
        match args.first().map(String::as_str) {
            Some("--restore-window") => self.open_restore(),
            // Agent Setup's agent set it up: said in the setup window.
            Some("--agent-setup-done") => {
                if let Target::GoogleDrive { account, .. } = Settings::load().target
                    && let Some(w) = self.drive.as_ref().filter(|w| w.is_open())
                {
                    // (Which tells the main window, as when it set it up.)
                    w.messenger().send(crate::drive_setup::Done::SetUp(Ok(account)));
                } else if let Some(w) = &self.win {
                    w.messenger().send(WinMsg::SetUp);
                }
            }
            Some("--versions-window") => match args.get(1) {
                Some(p) => {
                    let p = target::expand(p);
                    let p = if p.is_relative() { cwd.map(|c| c.join(&p)).unwrap_or(p) } else { p };
                    self.open_versions(p)
                }
                None => self.open(),
            },
            _ => self.open(),
        }
    }

    /// The main window (or brought to the front).
    pub fn open(&mut self) {
        olo_kit::open_or_raise(&mut self.win, main_options(), MainWindow::new);
    }

    /// The Google Drive setup window (or brought to the front).
    pub fn open_drive(&mut self) {
        olo_kit::open_or_raise(&mut self.drive, crate::drive_setup::options(), DriveSetup::new);
    }

    /// `path`'s Versions window (or brought to the front).
    pub fn open_versions(&mut self, path: PathBuf) {
        self.versions.retain(|(_, w)| w.is_open());
        if let Some((_, w)) = self.versions.iter().find(|(p, _)| *p == path) {
            olo_kit::raise(Some(w));
            return;
        }
        let at = path.clone();
        let w = olo_kit::open_window(crate::versions::options(&path), move |cx| VersionsWindow::new(at, cx));
        self.versions.push((path, w));
    }

    /// The Restore window (or brought to the front).
    pub fn open_restore(&mut self) {
        olo_kit::open_or_raise(&mut self.restore, crate::restore::options(), RestoreWindow::new);
    }
}

/// `path` with the home folder as `~`.
pub(crate) fn tilde(path: &std::path::Path) -> String {
    match path.strip_prefix(target::home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The key, from the keyring, kept in Passwords (started if it isn't
/// running; it asks to be unlocked if it's locked). Blocks.
pub(crate) fn keep_in_passwords() -> Result<(), String> {
    let key = target::key()?;
    let request = olo_keychain::Request::KeepSecret {
        title: target::KEY_TITLE.into(),
        secret: key.to_hex(),
        note: "The key to DiskBackup's backups: without it they can't be read. DiskBackup keeps a copy in this computer's keyring; on another computer, it reads it from here.".into(),
        replace: false,
    };
    match target::passwords(&request)? {
        olo_keychain::Response::Saved { .. } => Ok(()),
        ref other => Err(format!("Passwords answered {other:?}")),
    }
}

/// "14:05", or "Tue 14:05" beyond today, or a date further back.
pub(crate) fn clock(t: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_opt(t, 0).single() else { return String::new() };
    let today = Local::now().date_naive();
    if at.date_naive() == today {
        at.format("%H:%M").to_string()
    } else if (today - at.date_naive()).num_days().abs() < 7 {
        at.format("%a %H:%M").to_string()
    } else {
        at.format("%-d %b %H:%M").to_string()
    }
}

// (Said as Deskbar says them.)
pub(crate) use olo_protocol::diskbackup::words::{count, plural, size};

pub(crate) fn speed_label(kbs: u64) -> String {
    match kbs {
        0 => "No limit".into(),
        k if k >= 1000 && k % 1000 == 0 => format!("{} MB/s", k / 1000),
        k => format!("{k} KB/s"),
    }
}

#[cfg(test)]
mod undo_tests {
    use super::*;

    /// Undo puts back what its step changed, and leaves what changed
    /// since (the service, a setup) as it is now.
    #[test]
    fn undo_puts_back_only_what_the_step_changed() {
        let before = Settings::default();
        let mut after = before.clone();
        after.every_minutes = before.every_minutes + 15;
        assert_eq!(step_name(&before, &after), "Back Up Every");
        let mut now = after.clone();
        now.in_deskbar = !before.in_deskbar;
        now.enabled = !before.enabled;
        put_back(&mut now, &before, &after);
        assert_eq!(now.every_minutes, before.every_minutes);
        assert_eq!((now.in_deskbar, now.enabled), (!before.in_deskbar, !before.enabled), "changed since: kept");
        let mut more = before.clone();
        more.paths.push("~/Music".into());
        assert_eq!(step_name(&before, &more), "Add Folder");
        assert_eq!(step_name(&more, &before), "Remove Folder");
        more = before.clone();
        more.keep.daily += 1;
        assert_eq!(step_name(&before, &more), "Keep");
    }
}
