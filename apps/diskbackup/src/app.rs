//! DiskBackup's app (on olo-ui): the service's state and settings, live
//! (its state file, followed by inotify: nothing ticks while nothing
//! happens), its windows (the main one is `window.rs`), and what's done
//! from them. Changes are written to the settings, which the service
//! follows at once.

use std::path::PathBuf;

use olo_protocol::diskbackup::{Request, Settings, SnapshotInfo, State, Target};
use olo_ui::{
    Frame, Menu, Proxy, WindowOptions,
    file_panel::{self, FilePanel, PanelResult},
    gpui::{AppContext as _, Context, Entity, WindowHandle},
    undo::{History, Purpose},
};

use crate::target;

pub enum Msg {
    /// The state or the settings changed (the files, followed).
    Changed,
    /// Something done in the background: what to say (an error if Err).
    Note(Result<String, String>),
    /// For the Restore window.
    Restore(crate::restore::Done),
    /// For the Versions window of that file.
    Versions(PathBuf, crate::versions::Done),
    /// For the Google Drive setup window.
    Drive(crate::drive_setup::Done),
    /// This computer's backups, from the service (or why not).
    Snapshots(Result<Vec<SnapshotInfo>, String>),
    /// Where backups go, looked at (its disk's space, or Google's answer).
    Where(Target, Result<target::Where, String>),
}

/// A settings change, for Edit ▸ Undo: the settings before it and after.
/// Undo puts back only what it changed (the service or a setup may have
/// changed others since).
#[derive(Debug, Clone)]
pub struct Changed {
    before: Settings,
    after: Settings,
}

/// What a change is called (Edit ▸ Undo *Keep*), by what it changed.
fn step_name(a: &Settings, b: &Settings) -> &'static str {
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
fn put_back(now: &mut Settings, before: &Settings, after: &Settings) {
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

pub struct Disk {
    pub settings: Settings,
    /// Edit ▸ Undo and Redo of the settings changed here.
    history: History<Changed>,
    pub state: State,
    /// What was last said (and whether it's an error).
    pub note: Option<(String, bool)>,
    /// Setting up, or keeping the key: a worker is on it.
    pub busy: bool,
    /// This computer's backups (None: being asked).
    pub snapshots: Option<Result<Vec<SnapshotInfo>, String>>,
    /// Where backups go, as last looked at (None inside: looking).
    pub where_: Option<(Target, Option<Result<target::Where, String>>)>,
    win: Option<WindowHandle<Frame>>,
    /// The Restore window, while it's open.
    restore: Option<(WindowHandle<Frame>, Entity<crate::restore::RestoreWindow>)>,
    /// Versions windows, a file each.
    versions: Vec<(WindowHandle<Frame>, Entity<crate::versions::VersionsWindow>)>,
    /// The Google Drive setup window, while it's open.
    drive: Option<(WindowHandle<Frame>, Entity<crate::drive_setup::DriveSetup>)>,
    proxy: Proxy,
    _watch: Option<olo_ui::watch::Watch>,
}

impl Disk {
    pub fn new(proxy: Proxy) -> Disk {
        let changed = proxy.clone();
        let watch = olo_ui::watch::watch([State::path(), Settings::path()], move |_| changed.send(Msg::Changed));
        Disk {
            settings: Settings::load(),
            history: History::default(),
            state: State::load(),
            note: None,
            busy: false,
            snapshots: None,
            where_: None,
            win: None,
            restore: None,
            versions: Vec::new(),
            drive: None,
            proxy,
            _watch: watch,
        }
    }

    /// For the layout lint: `settings` and `state` as given, no backups
    /// listed, nothing watched or asked.
    #[cfg(test)]
    pub fn for_lint(settings: Settings, state: State) -> Disk {
        Disk {
            settings,
            history: History::default(),
            state,
            note: None,
            busy: false,
            snapshots: Some(Ok(Vec::new())),
            where_: None,
            win: None,
            restore: None,
            versions: Vec::new(),
            drive: None,
            proxy: Proxy::new(|_| {}),
            _watch: None,
        }
    }

    pub fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        if let Some(launch) = message.downcast_ref::<olo_ui::instance::Launch>() {
            return self.launched(&launch.args, launch.cwd.as_deref(), cx);
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Changed => {
                self.settings = Settings::load();
                let was = (self.state.last_ok, self.state.snapshots, self.state.doing.clone());
                self.state = State::load();
                // A backup finished: the Versions windows list again.
                if self.state.last_ok != was.0 {
                    for (_, w) in &self.versions {
                        w.update(cx, |w, cx| w.reload(cx));
                    }
                }
                // Backups made, deleted, erased or validated: listed again.
                if (self.state.last_ok, self.state.snapshots, self.state.doing.clone()) != was {
                    self.load_snapshots();
                }
            }
            Msg::Snapshots(r) => self.snapshots = Some(r),
            Msg::Where(t, r) => match &mut self.where_ {
                Some((now, looked)) if *now == t => *looked = Some(r),
                _ => return,
            },
            Msg::Restore(d) => {
                if let Some((_, w)) = &self.restore {
                    w.update(cx, |w, cx| w.message(d, cx));
                }
                return;
            }
            Msg::Drive(d) => {
                let set_up = matches!(&d, crate::drive_setup::Done::SetUp(Ok(_)));
                if let Some((_, w)) = &self.drive {
                    w.clone().update(cx, |w, cx| w.message(d, cx));
                }
                if set_up {
                    self.settings = Settings::load();
                    self.note = None;
                    cx.notify();
                }
                return;
            }
            Msg::Versions(path, d) => {
                if let Some((_, w)) = self.versions.iter().find(|(_, w)| w.read(cx).path == path) {
                    w.clone().update(cx, |w, cx| w.message(d, cx));
                }
                return;
            }
            Msg::Note(r) => {
                self.busy = false;
                self.settings = Settings::load();
                // (Nothing said when the window shows it already.)
                self.note = match r {
                    Ok(s) if s.is_empty() => None,
                    Ok(s) => Some((s, false)),
                    Err(e) => Some((e, true)),
                };
            }
        }
        cx.notify();
    }

    /// What a launch asks (its arguments): `--restore-window` (Deskbar's
    /// Restore Files…), `--versions-window PATH` (Tracker's Older
    /// Versions…), else the window.
    pub fn launched(&mut self, args: &[String], cwd: Option<&std::path::Path>, cx: &mut Context<Self>) {
        match args.first().map(String::as_str) {
            Some("--restore-window") => self.open_restore(cx),
            // Agent Setup's agent set it up: said in the setup window.
            Some("--agent-setup-done") => {
                self.settings = Settings::load();
                self.note = None;
                if let olo_protocol::diskbackup::Target::GoogleDrive { account, .. } = &self.settings.target
                    && let Some((_, w)) = &self.drive
                {
                    let done = crate::drive_setup::Done::SetUp(Ok(account.clone()));
                    w.clone().update(cx, |w, cx| w.message(done, cx));
                }
                cx.notify();
            }
            Some("--versions-window") => match args.get(1) {
                Some(p) => {
                    let p = target::expand(p);
                    let p = if p.is_relative() { cwd.map(|c| c.join(&p)).unwrap_or(p) } else { p };
                    self.open_versions(p, cx)
                }
                None => self.open(cx),
            },
            _ => self.open(cx),
        }
    }

    /// The Google Drive setup window (or brought to the front).
    pub fn open_drive(&mut self, cx: &mut Context<Self>) {
        if let Some((w, _)) = &self.drive
            && olo_ui::raise(Some(*w), cx)
        {
            return;
        }
        let (proxy, me) = (self.proxy.clone(), cx.entity());
        cx.defer(move |cx| {
            let view = cx.new(|cx| crate::drive_setup::DriveSetup::new(proxy, cx));
            let shown = view.clone();
            if let Some(h) = olo_ui::open_window(cx, crate::drive_setup::options(), move |_, _| shown) {
                me.update(cx, |d, _| d.drive = Some((h, view)));
            }
        });
    }

    /// `path`'s Versions window (or brought to the front).
    pub fn open_versions(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.versions.retain(|(h, _)| h.update(cx, |_, _, _| ()).is_ok());
        if let Some((h, _)) = self.versions.iter().find(|(_, w)| w.read(cx).path == path)
            && olo_ui::raise(Some(*h), cx)
        {
            return;
        }
        let (proxy, me) = (self.proxy.clone(), cx.entity());
        cx.defer(move |cx| {
            let options = crate::versions::options(&path);
            let view = cx.new(|cx| crate::versions::VersionsWindow::new(proxy, path, cx));
            let shown = view.clone();
            if let Some(h) = olo_ui::open_window(cx, options, move |_, _| shown) {
                me.update(cx, |d, _| d.versions.push((h, view)));
            }
        });
    }

    pub fn open(&mut self, cx: &mut Context<Self>) {
        let app = cx.entity();
        self.load_snapshots();
        // (A form, at the size of what it shows: it floats, never zoomed
        // in Focus or put in a tile. Ben, 2026-11-08: "diskbackup looks
        // silly in focus mode".)
        // (Fixed, as Preferences' panels: as tall as the tab chosen shows.)
        olo_ui::open_or_raise(self.win, cx, main_options(), move |window, cx| cx.new(|cx| crate::window::MainWindow::new(app, window, cx)), |a, w| a.win = w);
    }

    /// This computer's backups, asked of the service (on a worker).
    pub fn load_snapshots(&self) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let r = match olo_protocol::diskbackup::call(&Request::Snapshots) {
                Ok(olo_protocol::diskbackup::Reply::Snapshots { snapshots }) => Ok(snapshots),
                Ok(other) => Err(format!("the service answered {other:?}")),
                Err(e) => Err(e),
            };
            proxy.send(Msg::Snapshots(r));
        });
    }

    /// Delete backups `ids` (hex), and the unfinished one if `unfinished`,
    /// once the person says so.
    pub fn delete(&mut self, ids: Vec<String>, unfinished: bool, cx: &mut Context<Self>) {
        let n = ids.len() + usize::from(unfinished);
        if n == 0 {
            return;
        }
        let me = cx.entity();
        let sent = self.state.unfinished.as_ref().map_or(0, |u| u.sent);
        let message = match (ids.len(), unfinished) {
            (0, true) => format!("Delete the unfinished backup? What it sent ({}) is freed, and the next backup reads and sends again what it had done. It can’t be undone.", size(sent)),
            (1, false) => "Delete this backup? The space only it took is freed; files other backups hold too stay in those. It can’t be undone.".to_owned(),
            (_, false) => format!("Delete these {n} backups? The space only they took is freed; files other backups hold too stay in those. It can’t be undone."),
            (_, true) => format!(
                "Delete these {n} backups, the unfinished one too? The space only they took is freed (what the unfinished one sent, {}); files other backups hold too stay in those. It can’t be undone.",
                size(sent)
            ),
        };
        cx.defer(move |cx| {
            olo_ui::widgets::open_alert(cx, "Delete Backups", "warning", &message, &["Cancel", "Delete"], move |choice, cx| {
                if choice == Some(1) {
                    let ids = ids.clone();
                    me.update(cx, |d, cx| {
                        d.ask(Request::Delete { snapshots: ids, unfinished }, "");
                        cx.notify();
                    });
                }
            });
        });
    }

    /// Validate backups `ids` (hex), by the service (the header says how
    /// it goes; the list, how it went): fast at once; `full`y (every byte
    /// read back) once the person says so.
    pub fn validate(&mut self, ids: Vec<String>, full: bool, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        if !full {
            self.ask(Request::Validate { snapshots: ids, full }, "");
            cx.notify();
            return;
        }
        let me = cx.entity();
        let what = if ids.len() == 1 { "this backup needs".to_owned() } else { format!("these {} backups need", ids.len()) };
        let most = self.state.size.map(|b| format!(" (up to {})", size(b))).unwrap_or_default();
        let message = format!(
            "Validate fully? Every byte {what}{most} is read back from {} and checked, as a restore would: it can take a while, at the download limit. The usual Validate compares checksums and reads only a sample back.",
            self.settings.target.describe()
        );
        cx.defer(move |cx| {
            olo_ui::widgets::open_alert(cx, "Validate Fully", "info", &message, &["Cancel", "Validate Fully"], move |choice, cx| {
                if choice == Some(1) {
                    let ids = ids.clone();
                    me.update(cx, |d, cx| {
                        d.ask(Request::Validate { snapshots: ids, full: true }, "");
                        cx.notify();
                    });
                }
            });
        });
    }

    /// Where backups go looked at (on a worker): unless it was for this
    /// target already, or `again`.
    pub fn look_at_target(&mut self, again: bool) {
        let t = self.settings.target.clone();
        if t == Target::None || (!again && self.where_.as_ref().is_some_and(|(w, _)| *w == t)) {
            return;
        }
        self.where_ = Some((t.clone(), None));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let r = target::look_at(&t);
            proxy.send(Msg::Where(t, r));
        });
    }

    /// Disconnect Google Drive, once the person says so: backing up there
    /// stopped, and DiskBackup's sign-in forgotten on this computer (its
    /// client kept, to connect again); the backups there stay.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        let Target::GoogleDrive { account, .. } = &self.settings.target else { return };
        let message = format!(
            "Disconnect Google Drive ({account})? DiskBackup stops backing up there, and forgets its sign-in on this computer. The backups in your Drive stay as they are: connect again to go on from them, or to restore from them."
        );
        let me = cx.entity();
        cx.defer(move |cx| {
            olo_ui::widgets::open_alert(cx, "Disconnect Google Drive", "warning", &message, &["Cancel", "Disconnect"], move |choice, cx| {
                if choice == Some(1) {
                    me.update(cx, |d, cx| d.disconnect_now(cx));
                }
            });
        });
    }

    fn disconnect_now(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.note = Some(("Disconnecting from Google Drive…".into(), false));
        cx.notify();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            // (What's under way stops first: it's using the sign-in.)
            let _ = crate::ask(Request::Stop);
            let out = target::forget_google().and_then(|()| {
                let mut s = Settings::load();
                s.target = Target::None;
                s.save().map_err(|e| e.to_string())?;
                Ok("Disconnected from Google Drive. Your backups there stay as they are.".to_owned())
            });
            proxy.send(Msg::Note(out));
        });
    }

    /// Every backup deleted, to start fresh, once the person says so.
    pub fn erase_all(&mut self, cx: &mut Context<Self>) {
        let me = cx.entity();
        let at = self.settings.target.describe();
        let then = if self.settings.automatic { "A fresh backup starts right after, copying everything again." } else { "The next backup (when you click Back Up Now) copies everything again." };
        let message = format!("Erase every backup in {at}? Every version of every file kept there goes (another computer’s backups there too), and the space is freed. {then} It can’t be undone.");
        cx.defer(move |cx| {
            olo_ui::widgets::open_alert(cx, "Erase All Backups", "warning", &message, &["Cancel", "Erase All Backups"], move |choice, cx| {
                if choice == Some(1) {
                    me.update(cx, |d, cx| {
                        d.ask(Request::EraseAll, "");
                        cx.notify();
                    });
                }
            });
        });
    }

    /// The settings changed with `f` (the service follows them at once):
    /// a step for Edit ▸ Undo (typing the patterns, one a run).
    pub fn change(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let mut s = self.settings.clone();
        f(&mut s);
        if s != self.settings {
            let name = step_name(&self.settings, &s);
            let step = Changed { before: self.settings.clone(), after: s.clone() };
            if name == "Patterns" {
                self.history.done_merging("patterns", name, step, |was, now| was.after = now.after);
            } else {
                self.history.done(name, step);
            }
            self.save(s, cx);
        }
    }

    fn save(&mut self, s: Settings, cx: &mut Context<Self>) {
        if let Err(e) = s.save() {
            self.note = Some((format!("Couldn’t save the settings: {e}"), true));
        }
        self.settings = s;
        cx.notify();
    }

    /// Edit ▸ Undo (`redo` false) or Redo: what the step changed put
    /// back, the rest left as it is now.
    pub fn undo_redo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let Some(step) = (if redo { self.history.take_redo() } else { self.history.take_undo() }) else { return };
        let mut s = self.settings.clone();
        put_back(&mut s, &step.change.before, &step.change.after);
        let purpose = if redo { Purpose::Redo } else { Purpose::Undo };
        self.history.record(purpose, step.name, Changed { before: step.change.after, after: step.change.before });
        self.save(s, cx);
    }

    /// Edit ▸ Undo and Redo, named for their steps.
    pub fn edit_menu(&self) -> Menu {
        self.history.edit_items(Menu::new("Edit"))
    }

    /// A menu's or a button's command.
    pub fn command(&mut self, cmd: &str, cx: &mut Context<Self>) {
        match cmd {
            "back-up-now" => self.ask(Request::BackUpNow, ""),
            "stop" => {
                // (Said at once: keeping what's done can take a few seconds.)
                if self.state.doing == olo_protocol::diskbackup::Doing::BackingUp
                    && let Some(p) = self.state.progress.as_mut()
                {
                    p.stage = olo_protocol::diskbackup::Stage::Stopping;
                    cx.notify();
                }
                self.ask(Request::Stop, "");
            }
            "choose-folder" => {
                let me = cx.entity();
                cx.defer(move |cx| {
                    file_panel::show(cx, "Back Up to a Folder", FilePanel::folder(target::home()), move |a, cx| {
                        if let PanelResult::Chosen(path) = a.result {
                            me.update(cx, |d, cx| d.set_up_folder(path, cx));
                        }
                    });
                });
            }
            "google-drive" => self.open_drive(cx),
            "disconnect" => self.disconnect(cx),
            // Where backups go, in Google Drive's own page, or in Tracker.
            "show-target" => {
                let at = match &self.settings.target {
                    Target::GoogleDrive { folder, .. } => Some(olo_xdg::open::Target::Url(format!("https://drive.google.com/drive/folders/{folder}"))),
                    Target::Folder { path } => Some(olo_xdg::open::Target::Path(target::expand(path))),
                    Target::None => None,
                };
                if let Some(at) = at
                    && let Err(e) = olo_xdg::open::open(&at)
                {
                    self.note = Some((format!("It didn’t open: {e}."), true));
                    cx.notify();
                }
            }
            "add-folder" => {
                let me = cx.entity();
                cx.defer(move |cx| {
                    file_panel::show(cx, "Back Up a Folder", FilePanel::folder(target::home()), move |a, cx| {
                        if let PanelResult::Chosen(path) = a.result {
                            me.update(cx, |d, cx| d.change(|s| s.paths.push(tilde(&path)), cx));
                        }
                    });
                });
            }
            "keep-key" => self.keep_key(cx),
            "erase-all" => self.erase_all(cx),
            "undo" => self.undo_redo(false, cx),
            "redo" => self.undo_redo(true, cx),
            "restore" => self.open_restore(cx),
            "quit" => cx.quit(),
            _ => {}
        }
    }

    /// The Restore window (or brought to the front).
    pub fn open_restore(&mut self, cx: &mut Context<Self>) {
        if let Some((w, _)) = &self.restore
            && olo_ui::raise(Some(*w), cx)
        {
            return;
        }
        let (proxy, me) = (self.proxy.clone(), cx.entity());
        cx.defer(move |cx| {
            let view = cx.new(|cx| crate::restore::RestoreWindow::new(proxy, cx));
            let shown = view.clone();
            if let Some(h) = olo_ui::open_window(cx, crate::restore::options(), move |_, _| shown) {
                me.update(cx, |d, _| d.restore = Some((h, view)));
            }
        });
    }

    /// Ask the service `r` (on a worker: it's quick, but nothing waits);
    /// `said` once it's asked ("": nothing, the header shows it), or why
    /// it couldn't be.
    fn ask(&self, r: Request, said: &'static str) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let out = crate::ask(r).map(|()| said.to_owned());
            proxy.send(Msg::Note(out));
        });
    }

    /// Back up to `dir`: a backup there already opened with this
    /// computer's key, else a new one made (and a key, if there's none);
    /// the key kept in the keyring and in Passwords. On a worker.
    fn set_up_folder(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.busy = true;
        self.note = Some((format!("Setting up backups to {}…", dir.display()), false));
        cx.notify();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let out = crate::set_up_folder(&dir.display().to_string()).map(|()| {
                let mut s = Settings::load();
                match keep_in_passwords() {
                    Ok(()) => {
                        s.key_in_passwords = true;
                        let _ = s.save();
                        format!("Backing up to {}. Its key is kept in Passwords.", dir.display())
                    }
                    Err(e) => format!("Backing up to {}. Keep its key in Passwords when you can ({e}).", dir.display()),
                }
            });
            proxy.send(Msg::Note(out));
        });
    }

    fn keep_key(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.note = Some(("Keeping the key in Passwords (unlock it if it asks)…".into(), false));
        cx.notify();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let out = keep_in_passwords().map(|()| {
                let mut s = Settings::load();
                s.key_in_passwords = true;
                let _ = s.save();
                "The key is kept in Passwords as “DiskBackup Key”.".to_owned()
            });
            proxy.send(Msg::Note(out));
        });
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
