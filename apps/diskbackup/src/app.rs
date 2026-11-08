//! DiskBackup's window (on olo-ui): what the service is doing and has done,
//! live (its state file, followed by inotify: nothing ticks while nothing
//! happens), where backups go and setting that up, what's backed up and
//! left out, when, how fast, and the key. Changes are written to the
//! settings, which the service follows at once.

use std::path::PathBuf;

use olo_diskbackup::exclude::GROUPS;
use olo_protocol::diskbackup::{Doing, Request, Settings, State, Target};
use olo_ui::{
    Frame, Menu, MenuBar, Proxy, WindowOptions,
    file_panel::{self, FilePanel, PanelResult},
    gpui::{self, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, WindowHandle, div, px},
    metrics as m, style, theme,
    undo::{History, Purpose},
    widgets::{BBox, Button, CheckBox, MenuField, StringView, TextControl, TextField, TextFieldEvent},
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
}

/// What a backup takes "every": minutes, and what it's called.
const EVERY: &[(u32, &str)] = &[(15, "15 minutes"), (30, "30 minutes"), (60, "Hour"), (120, "2 hours"), (360, "6 hours"), (720, "12 hours"), (1440, "Day")];
/// Speed limits offered, KB/s (0: none).
const SPEEDS: &[u64] = &[0, 256, 512, 1000, 2000, 5000, 10_000, 25_000];

/// How long each kind of snapshot is kept: the choices.
const HOURLY: &[(u32, &str)] = &[(0, "None"), (12, "12 hours"), (24, "24 hours"), (48, "48 hours"), (168, "A week")];
const DAILY: &[(u32, &str)] = &[(0, "None"), (7, "7 days"), (14, "14 days"), (30, "30 days"), (90, "90 days")];
const WEEKLY: &[(u32, &str)] = &[(0, "None"), (4, "4 weeks"), (8, "8 weeks"), (12, "12 weeks"), (26, "26 weeks"), (52, "52 weeks")];
const MONTHLY: &[(u32, &str)] = &[(0, "None"), (6, "6 months"), (12, "12 months"), (24, "24 months"), (60, "5 years"), (u32::MAX, "Forever")];
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
    } else if a.every_minutes != b.every_minutes {
        "Back Up Every"
    } else if a.on_battery != b.on_battery {
        "Also on Battery"
    } else if a.keep != b.keep {
        "Keep"
    } else if a.upload_kbs != b.upload_kbs || a.download_kbs != b.download_kbs {
        "Speed"
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
    each!(enabled, target, paths, exclude_groups, excludes, every_minutes, keep, upload_kbs, download_kbs, on_battery, one_file_system, key_in_passwords, in_deskbar);
}

pub struct Disk {
    pub settings: Settings,
    /// Edit ▸ Undo and Redo of the settings changed here.
    history: History<Changed>,
    pub state: State,
    /// What was last said (and whether it's an error).
    note: Option<(String, bool)>,
    /// Setting up, or keeping the key: a worker is on it.
    busy: bool,
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
            win: None,
            restore: None,
            versions: Vec::new(),
            drive: None,
            proxy,
            _watch: watch,
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
                let was = self.state.last_ok;
                self.state = State::load();
                // A backup finished: the Versions windows list again.
                if self.state.last_ok != was {
                    for (_, w) in &self.versions {
                        w.update(cx, |w, cx| w.reload(cx));
                    }
                }
            }
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
                self.note = Some(match r {
                    Ok(s) => (s, false),
                    Err(e) => (e, true),
                });
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
        // (A form, at the size of what it shows: it floats, never zoomed
        // in Focus or put in a tile. Ben, 2026-11-08: "diskbackup looks
        // silly in focus mode".)
        let options = WindowOptions::new("DiskBackup", 1060.0, 560.0).fits_content().floating();
        olo_ui::open_or_raise(self.win, cx, options, move |window, cx| cx.new(|cx| MainWindow::new(app, window, cx)), |a, w| a.win = w);
    }

    /// The settings changed with `f` (the service follows them at once):
    /// a step for Edit ▸ Undo (typing the patterns, one a run).
    fn change(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
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
            "back-up-now" => self.ask(Request::BackUpNow, "Backing up now."),
            "stop" => self.ask(Request::Stop, "Stopping."),
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

    /// Ask the service (on a worker: it's quick, but nothing waits).
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
fn tilde(path: &std::path::Path) -> String {
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
fn clock(t: i64) -> String {
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

pub(crate) fn size(b: u64) -> String {
    match b {
        0..1_000_000 => format!("{} KB", b.div_ceil(1000)),
        1_000_000..1_000_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        _ => format!("{:.2} GB", b as f64 / 1e9),
    }
}

fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "1 file", "12 files".
fn plural(n: u64, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{} {what}s", count(n)) }
}

fn speed_label(kbs: u64) -> String {
    match kbs {
        0 => "No limit".into(),
        k if k >= 1000 && k % 1000 == 0 => format!("{} MB/s", k / 1000),
        k => format!("{k} KB/s"),
    }
}

/// What the service is doing and did, in a few lines: the first the
/// headline, an error if it's one.
pub fn status_lines(st: &State, s: &Settings) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    if s.target == Target::None {
        out.push(("Not set up yet: choose where your backups go.".into(), false));
        return out;
    }
    if st.pid == 0 {
        out.push(("The DiskBackup service isn’t running (it starts with your session).".into(), true));
    }
    match &st.doing {
        Doing::BackingUp => {
            let p = st.progress.clone().unwrap_or_default();
            out.push((format!("Backing up: {}, {} read, {} new.", plural(p.files, "file"), size(p.bytes), size(p.new_bytes)), false));
            if !p.current.is_empty() {
                out.push((p.current.replace(&target::home().display().to_string(), "~"), false));
            }
        }
        Doing::Waiting { why } => out.push((format!("Waiting: {why}."), false)),
        Doing::Restoring => out.push(("Restoring…".into(), false)),
        Doing::Pruning => {
            let p = st.progress.clone().unwrap_or_default();
            out.push((if p.current.is_empty() { "Tidying…".into() } else { format!("Tidying: {}…", p.current) }, false));
        }
        Doing::Idle => match (&st.last, st.last_ok) {
            (Some(l), _) if l.error.is_none() => out.push((format!("Backed up at {}: {}, {} new.", clock(l.time), plural(l.files, "file"), size(l.new_bytes)), false)),
            (Some(l), ok) => {
                out.push((format!("The backup at {} didn’t finish: {}.", clock(l.time), l.error.clone().unwrap_or_default()), true));
                if let Some(t) = ok {
                    out.push((format!("Last backed up whole at {}.", clock(t)), false));
                }
            }
            (None, _) => out.push(("Not backed up yet.".into(), false)),
        },
    }
    if let Some(n) = st.next.filter(|_| st.doing == Doing::Idle) {
        let takes = st.size.map(|b| format!(", {}", size(b))).unwrap_or_default();
        out.push((format!("Next at {}. {} kept{takes}.", clock(n), plural(st.snapshots as u64, "snapshot")), false));
    }
    // The last tidy, if it did anything (or didn't finish).
    if let Some(t) = st.tidied.as_ref().filter(|_| st.doing == Doing::Idle) {
        match &t.error {
            Some(e) => out.push((format!("Tidying at {} didn’t finish: {e}.", clock(t.time)), true)),
            None if t.forgotten > 0 || t.freed > 0 => out.push((format!("Tidied at {}: {} let go of, {} freed.", clock(t.time), plural(t.forgotten as u64, "old snapshot"), size(t.freed)), false)),
            None => {}
        }
    }
    out
}

pub struct MainWindow {
    app: Entity<Disk>,
    patterns: Entity<TextField>,
    _sub: gpui::Subscription,
    _observe: gpui::Subscription,
}

impl MainWindow {
    fn new(app: Entity<Disk>, _window: &mut Window, cx: &mut Context<Self>) -> MainWindow {
        let now = app.read(cx).settings.excludes.join(", ");
        let patterns = cx.new(|cx| {
            let mut f = TextField::new(cx).hint("~/Videos/raw, **/*.iso");
            f.set_text(now, cx);
            f
        });
        let to = app.clone();
        let sub = cx.subscribe(&patterns, move |_, field, e: &TextFieldEvent, cx| {
            if !matches!(e, TextFieldEvent::Changed) {
                return;
            }
            let text = field.read(cx).text().to_owned();
            to.update(cx, |d, cx| d.change(|s| s.excludes = text.split([',', '\n']).map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect(), cx));
        });
        let observe = cx.observe(&app, |w: &mut MainWindow, app, cx| {
            // (Put back by Undo: the field says so. What's being typed
            // stays as typed while it means the same.)
            let now = app.read(cx).settings.excludes.clone();
            let typed: Vec<String> = w.patterns.read(cx).text().split([',', '\n']).map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect();
            if typed != now {
                w.patterns.update(cx, |f, cx| f.set_text(now.join(", "), cx));
            }
            cx.notify();
        });
        MainWindow { app, patterns, _sub: sub, _observe: observe }
    }
}

impl Render for MainWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let d = self.app.read(cx);
        let (s, st) = (d.settings.clone(), d.state.clone());
        let set_up = s.target != Target::None;
        let app = self.app.clone();
        let cmd = |c: &'static str| {
            let app = app.clone();
            move |_: &mut Window, cx: &mut gpui::App| app.update(cx, |d, cx| d.command(c, cx))
        };
        let line = |text: String, error: bool| StringView::new(text).wrap(true).color(if error { style::c(p.failure) } else { style::c(p.panel_text) });
        let dim = |text: String| StringView::new(text).wrap(true).dim(true);

        // ---- the backup: what it's doing, and Back Up Now or Stop.
        let mut status = BBox::new("Backup");
        for (i, (text, error)) in status_lines(&st, &s).into_iter().enumerate() {
            status = status.child(if i == 0 || error { line(text, error).into_any_element() } else { dim(text).into_any_element() });
        }
        let running = matches!(st.doing, Doing::BackingUp | Doing::Pruning);
        let buttons = div().flex().flex_row().gap(px(m::SPACING)).pt(px(m::TIGHT)).child(if running {
            Button::new("Stop").on_click(cmd("stop"))
        } else {
            Button::new("Back Up Now").enabled(set_up && st.pid != 0).on_click(cmd("back-up-now"))
        });
        let buttons = buttons.child(Button::new("Restore…").enabled(set_up).on_click(cmd("restore")));
        status = status.child(buttons);
        // (What Deskbar's tray shows: this, in a glance.)
        status = status.child(div().pt(px(m::TIGHT)).child({
            let app = app.clone();
            CheckBox::new("Show in Deskbar", s.in_deskbar).on_toggle(move |on, _, cx| app.update(cx, |d, cx| d.change(|s| s.in_deskbar = on, cx)))
        }));

        // ---- where backups go.
        let mut target = BBox::new("Back Up To");
        target = target.child(line(if set_up { s.target.describe() } else { "Nowhere yet.".into() }, false));
        target = target.child(dim("A disk of its own (or a folder on one) keeps your backups apart from this one; your Google Drive keeps them anywhere.".into()));
        target = target.child(
            div()
                .flex()
                .flex_row()
                .gap(px(m::SPACING))
                .pt(px(m::TIGHT))
                .child(Button::new(if set_up { "Choose Another Folder…" } else { "Choose a Folder…" }).enabled(!d.busy).on_click(cmd("choose-folder")))
                .child(Button::new("Google Drive…").enabled(!d.busy).on_click(cmd("google-drive"))),
        );

        // ---- when, and how fast.
        let every_at = EVERY.iter().position(|(m, _)| *m == s.every_minutes);
        let mut every_names: Vec<SharedString> = EVERY.iter().map(|(_, n)| SharedString::from(*n)).collect();
        if every_at.is_none() {
            every_names.push(format!("{} minutes", s.every_minutes).into());
        }
        let app_every = app.clone();
        let when = BBox::new("When")
            .child(MenuField::new("every", every_names, every_at.unwrap_or(EVERY.len())).label("Back up every:").divider(120.0).on_change(move |i, _, cx| {
                if let Some((m, _)) = EVERY.get(i) {
                    let m = *m;
                    app_every.update(cx, |d, cx| d.change(|s| s.every_minutes = m, cx));
                }
            }))
            .child({
                let app = app.clone();
                CheckBox::new("Also on battery", s.on_battery).on_toggle(move |on, _, cx| app.update(cx, |d, cx| d.change(|s| s.on_battery = on, cx)))
            });
        let speed_menu = |id: &'static str, label: &'static str, now: u64, set: fn(&mut Settings, u64)| {
            let at = SPEEDS.iter().position(|k| *k == now);
            let mut names: Vec<SharedString> = SPEEDS.iter().map(|k| SharedString::from(speed_label(*k))).collect();
            if at.is_none() {
                names.push(speed_label(now).into());
            }
            let app = app.clone();
            MenuField::new(id, names, at.unwrap_or(SPEEDS.len())).label(label).divider(120.0).on_change(move |i, _, cx| {
                if let Some(k) = SPEEDS.get(i) {
                    let k = *k;
                    app.update(cx, |d, cx| d.change(|s| set(s, k), cx));
                }
            })
        };
        let speed = BBox::new("Speed")
            .child(speed_menu("upload", "Upload at most:", s.upload_kbs, |s, k| s.upload_kbs = k))
            .child(speed_menu("download", "Download at most:", s.download_kbs, |s, k| s.download_kbs = k))
            .child(dim("Backups always go at the computer’s idle priority: anything else you do comes first.".into()));

        // ---- how long snapshots are kept.
        let keep_menu = |id: &'static str, label: &'static str, choices: &'static [(u32, &'static str)], now: u32, set: fn(&mut Settings, u32)| {
            let at = choices.iter().position(|(n, _)| *n == now);
            let mut names: Vec<SharedString> = choices.iter().map(|(_, name)| SharedString::from(*name)).collect();
            if at.is_none() {
                names.push(now.to_string().into());
            }
            let app = app.clone();
            MenuField::new(id, names, at.unwrap_or(choices.len())).label(label).divider(120.0).on_change(move |i, _, cx| {
                if let Some((n, _)) = choices.get(i) {
                    let n = *n;
                    app.update(cx, |d, cx| d.change(|s| set(s, n), cx));
                }
            })
        };
        let k = s.keep;
        let keep = BBox::new("Keep")
            .child(keep_menu("hourly", "Hourly for:", HOURLY, k.hourly, |s, n| s.keep.hourly = n))
            .child(keep_menu("daily", "Daily for:", DAILY, k.daily, |s, n| s.keep.daily = n))
            .child(keep_menu("weekly", "Weekly for:", WEEKLY, k.weekly, |s, n| s.keep.weekly = n))
            .child(keep_menu("monthly", "Monthly for:", MONTHLY, k.monthly, |s, n| s.keep.monthly = n))
            .child(dim("Every backup from the last hour is kept, then the last of each hour, day, week and month that long; the rest go, and the space only they took is freed once a day.".into()));

        // ---- the key.
        let mut key = BBox::new("Key");
        if s.key_in_passwords {
            key = key.child(dim("The key to these backups is kept in Passwords (“DiskBackup Key”), and so in 1Password: with it, they can be restored on any computer.".into()));
        } else {
            key = key.child(line("Keep the key in Passwords too: without it, these backups can’t be read on another computer.".into(), set_up));
            key = key.child(div().pt(px(m::TIGHT)).child(Button::new("Keep Key in Passwords").enabled(set_up && !d.busy).on_click(cmd("keep-key"))));
        }

        // ---- what's backed up, and what's left out.
        let mut what = BBox::new("Back Up");
        let n = s.paths.len();
        for (i, path) in s.paths.iter().enumerate() {
            let app = app.clone();
            let shown = if path == "~" { "Home folder".to_owned() } else { path.clone() };
            what = what.child(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(StringView::new(shown))).child(
                Button::new("Remove").enabled(n > 1).on_click(move |_, cx| {
                    app.update(cx, |d, cx| {
                        d.change(
                            |s| {
                                if i < s.paths.len() {
                                    s.paths.remove(i);
                                }
                            },
                            cx,
                        )
                    })
                }),
            ));
        }
        what = what.child(div().pt(px(m::TIGHT)).child(Button::new("Add a Folder…").on_click(cmd("add-folder"))));
        let chosen = s.exclude_groups.clone().unwrap_or_else(olo_diskbackup::exclude::default_groups);
        let mut leave = BBox::new("Leave Out").child(dim("Your own files and code are always backed up. These can be downloaded or built again, so they needn’t take space or time:".into()));
        for g in GROUPS {
            let app = app.clone();
            let on = chosen.iter().any(|c| c == g.id);
            let id = g.id;
            // (What it means, under it: the folders and files.)
            let check = CheckBox::new(g.name, on).on_toggle(move |on, _, cx| {
                app.update(cx, |d, cx| {
                    d.change(
                        |s| {
                            let mut now = s.exclude_groups.clone().unwrap_or_else(olo_diskbackup::exclude::default_groups);
                            now.retain(|g| g != id);
                            if on {
                                now.push(id.to_owned());
                            }
                            s.exclude_groups = Some(now);
                        },
                        cx,
                    )
                })
            });
            let about = div().pl(px(m::MARK_SIZE + m::MARK_GAP)).text_size(px(style::DETAIL_SIZE)).child(StringView::new(g.about).wrap(true).dim(true));
            leave = leave.child(div().flex().flex_col().child(check).child(about));
        }
        // (Its own box in the middle column: Leave Out's alone is tall.)
        let also = BBox::new("Also Leave Out")
            .child(TextControl::new(self.patterns.clone()).label("Patterns:").divider(72.0))
            .child(dim("Folders and files, comma-separated: ~/ is your home folder, **/ anywhere. Folders with a CACHEDIR.TAG (Cargo’s target/, many caches) are always left out.".into()));

        let column = |items: Vec<gpui::AnyElement>| div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(m::SPACING)).children(items);
        // (Three columns: two would be taller than a 1280 × 800 screen.)
        // (Where and how fast with the status; when and how long kept with
        // what's backed up.)
        let first = column(vec![status.into_any_element(), target.into_any_element(), speed.into_any_element(), key.into_any_element()]);
        let second = column(vec![what.into_any_element(), also.into_any_element(), when.into_any_element(), keep.into_any_element()]);
        let third = column(vec![leave.into_any_element()]);
        let mut body = div().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(div().flex().flex_row().items_start().gap(px(m::SPACING)).child(first).child(second).child(third));
        if let Some((text, error)) = d.note.clone() {
            body = body.child(line(text, error));
        }
        let bar_app = self.app.clone();
        let bar = MenuBar::new(
            vec![
                Menu::new("File")
                    .item("Back Up Now", "back-up-now")
                    .key("b")
                    .enabled(set_up && !running)
                    .item("Stop", "stop")
                    .enabled(running)
                    .item("Restore…", "restore")
                    .key("r")
                    .enabled(set_up)
                    .separator()
                    .item("Close", "close")
                    .key("w")
                    .item("Quit", "quit")
                    .key("q"),
                d.edit_menu(),
            ],
            move |c, window, cx| match c {
                "close" => window.remove_window(),
                _ => bar_app.update(cx, |d, cx| d.command(c, cx)),
            },
        );
        div().size_full().flex().flex_col().child(bar).child(body)
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
