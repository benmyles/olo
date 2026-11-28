//! DiskBackup's main window (Ben, 2026-11-08: "improve the ux in
//! diskbackup … it's complicated and not intuitive"; "where are the
//! settings for limiting up/down speed and resource usage and knowing
//! exactly what's being backed up now what the plan is what's still
//! remaining"; "i should also be able to choose if i even want auto
//! backups … i should be able to clear out / remove old backups to start
//! fresh"). One question first, where backups go, until that's answered;
//! then what matters most at the top: whether your files are safe (when
//! they were last backed up, where to, when next), what's wrong with the
//! fix beside it, and while a backup runs, the whole of it (how far, of
//! how much, what's left, how fast, the file now, each folder's turn).
//! Under that, the settings by what they're about, in tabs: Schedule,
//! Files, Backups (each one kept: delete some, or erase them all),
//! Speed & Resources, Destination.
//!
//! A backup that didn't finish is said (what it got through, that the next
//! goes on from it) and listed first in Backups, to delete if it isn't
//! wanted; each backup's check is listed too (after it was made, or
//! Validate's, which reads one back whole), and damage to the latest is
//! said at the top. Destination says what DiskBackup is connected to (whose
//! Google Drive, its space; a folder's disk), with Disconnect (Ben,
//! 2026-11-08: "when i'm connected it should show the details of that
//! connection and a button to disconnect it").

use std::path::PathBuf;

use olo_diskbackup::exclude::GROUPS;
use olo_kit::{
    Child, ClickEvent, Cx, Element, IntoElement as _, Menu, MenuBar, ParentElement as _, SharedString, Styled as _, View, WindowCx, canvas, div,
    file_panel::{self, FilePanel, PanelResult},
    icons, metrics as m, px, style, theme, tip,
    undo::{History, Purpose},
    widgets::{
        self, BBox, Button, CheckBox, Column, ColumnListView, ListModel, MenuField, RadioButton, StatusBar, StringView, TabView, TextControl, TextField, TextFieldEvent, Tone, open_alert, text_field,
    },
};
use olo_protocol::diskbackup::{Check, Doing, Priority, Progress, Request, Settings, SnapshotInfo, Stage, State, Target};

use crate::{
    app::{Changed, Msg, clock, count, keep_in_passwords, put_back, size, speed_label, step_name, tilde},
    target::{self, Where},
};

/// The tabs, in order.
const TABS: [&str; 5] = ["Schedule", "Files", "Backups", "Speed & Resources", "Destination"];
/// Narrower than this, the welcome's two places for backups go one over
/// the other: each box's button ("Connect Google Drive…") needs about
/// 200 points.
const CHOICES_SIDE_BY_SIDE: f32 = 480.0;
const DESTINATION: usize = 4;

/// Busy with something that can be stopped (a backup, a tidy, erasing,
/// validating).
fn busy_with(st: &State) -> bool {
    matches!(st.doing, Doing::BackingUp | Doing::Pruning | Doing::Erasing | Doing::Validating | Doing::Deleting { .. })
}

/// What a backup takes "every": minutes, and what it's called.
// (Whole phrases after "Automatically,": "every hour" is said its own
// way in each language, "jede Stunde", "toutes les heures".)
const EVERY: &[(u32, &str)] = &[(15, "every 15 minutes"), (30, "every 30 minutes"), (60, "every hour"), (120, "every 2 hours"), (360, "every 6 hours"), (720, "every 12 hours"), (1440, "every day")];
/// Speed limits offered, KB/s (0: none).
const SPEEDS: &[u64] = &[0, 256, 512, 1000, 2000, 5000, 10_000, 25_000];
/// How long each kind of backup is kept: the choices.
const HOURLY: &[(u32, &str)] = &[(0, "None"), (12, "12 hours"), (24, "24 hours"), (48, "48 hours"), (168, "A week")];
const DAILY: &[(u32, &str)] = &[(0, "None"), (7, "7 days"), (14, "14 days"), (30, "30 days"), (90, "90 days")];
const WEEKLY: &[(u32, &str)] = &[(0, "None"), (4, "4 weeks"), (8, "8 weeks"), (12, "12 weeks"), (26, "26 weeks"), (52, "52 weeks")];
const MONTHLY: &[(u32, &str)] = &[(0, "None"), (6, "6 months"), (12, "12 months"), (24, "24 months"), (60, "5 years"), (u32::MAX, "Forever")];

/// The header's picture, points.
const ICON: f32 = 48.0;
/// From the welcome's picture to its words: they start where the boxes'
/// words do under them (a box's inset, its 32-point picture, a spacing).
const HERO_GAP: f32 = m::BOX_INSET + 32.0 + m::SPACING - ICON;
// (Room after the picture: a label's from its field at least.)
const _: () = assert!(HERO_GAP >= m::LABEL_GAP);
/// The welcome's heading where the title's size doesn't fit its line.
const HEADING_NARROW: f32 = 20.0;

/// The welcome's heading's size: the title's (`style::TITLE_SIZE`)
/// where it's `wide` at that size fits the `room`, else smaller.
fn welcome_heading_size(wide: f32, room: f32) -> f32 {
    if wide <= room { style::TITLE_SIZE } else { HEADING_NARROW }
}
/// A label column, points.
const DIVIDER: f32 = 140.0;

/// What the main window is told: by the hub, its workers, its alerts and
/// its panels.
pub enum WinMsg {
    /// The state or the settings changed (the files, followed): as read.
    Changed(Settings, State),
    /// Something done in the background: what to say (an error if Err).
    Note(Result<String, String>),
    /// This computer's backups, from the service (or why not).
    /// The backups listed (None: the service hasn't read them yet).
    Snapshots(Option<Result<Vec<SnapshotInfo>, String>>),
    /// Where backups go, looked at (its disk's space, or Google's answer).
    Where(Target, Result<Where, String>),
    /// Asked of the service, once the person said so.
    Ask(Request),
    /// Disconnect Google Drive: the person said so.
    Disconnect,
    /// Back up to this folder (chosen).
    SetUpFolder(PathBuf),
    /// Back this folder up too (chosen).
    AddFolder(PathBuf),
    /// Google Drive was set up (the setup window, or its agent).
    SetUp,
}

pub struct MainWindow {
    pub settings: Settings,
    /// Edit ▸ Undo and Redo of the settings changed here.
    history: History<Changed>,
    pub state: State,
    /// What was last said (and whether it's an error).
    note: Option<(String, bool)>,
    /// Setting up, or keeping the key: a worker is on it.
    busy: bool,
    /// This computer's backups (None: being asked).
    snapshots: Option<Result<Vec<SnapshotInfo>, String>>,
    /// Where backups go, as last looked at (None inside: looking).
    where_: Option<(Target, Option<Result<Where, String>>)>,
    tab: usize,
    patterns: Child<TextField>,
    /// The Backups tab's list (several may be chosen, to delete).
    list: ListModel,
    /// Drawn for the layout lint: nothing asked of the service or looked
    /// at.
    lint: bool,
}

/// What the patterns' field says, as the settings keep it.
fn patterns_of(text: &str) -> Vec<String> {
    text.split([',', '\n']).map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect()
}

impl MainWindow {
    pub fn new(cx: &mut Cx<Self>) -> MainWindow {
        let w = MainWindow::with(Settings::load(), State::load(), false, cx);
        w.load_snapshots(cx);
        w
    }

    /// For the layout lint: `settings` and `state` as given, no backups
    /// listed, nothing watched or asked.
    #[cfg(test)]
    pub fn for_lint(settings: Settings, state: State, cx: &mut Cx<Self>) -> MainWindow {
        let mut w = MainWindow::with(settings, state, true, cx);
        w.snapshots = Some(Ok(Vec::new()));
        w
    }

    fn with(settings: Settings, state: State, lint: bool, cx: &mut Cx<Self>) -> MainWindow {
        let now = settings.excludes.join(", ");
        let patterns = text_field(cx, |f| f.hint("~/Videos/raw, **/*.iso").with_text(now));
        cx.subscribe(&patterns, |w: &mut MainWindow, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Changed) {
                let typed = patterns_of(w.patterns.read(cx).text());
                w.change(|s| s.excludes = typed, cx);
            }
        });
        MainWindow { settings, history: History::default(), state, note: None, busy: false, snapshots: None, where_: None, tab: 0, patterns, list: ListModel::new(true), lint }
    }

    /// This computer's backups, asked of the service (on a worker).
    fn load_snapshots(&self, cx: &mut Cx<Self>) {
        if self.lint {
            return;
        }
        cx.work(
            || match olo_protocol::diskbackup::call(&Request::Snapshots) {
                // (Not read yet: the service says when it has, `State::read`.)
                Ok(olo_protocol::diskbackup::Reply::Snapshots { read: false, .. }) => None,
                Ok(olo_protocol::diskbackup::Reply::Snapshots { snapshots, .. }) => Some(Ok(snapshots)),
                Ok(other) => Some(Err(olo_kit::tr!("the service answered {}", format!("{:?}", other)))),
                Err(e) => Some(Err(e)),
            },
            WinMsg::Snapshots,
        );
    }

    /// The settings as they are now: the Destination tab looks where
    /// backups go once that changes, and the patterns' field says what
    /// Undo put back (what's being typed stays as typed while it means the
    /// same).
    fn settings_changed(&mut self, cx: &mut Cx<Self>) {
        if self.tab == DESTINATION {
            self.look_at_target(false, cx);
        }
        let now = self.settings.excludes.clone();
        if patterns_of(self.patterns.read(cx).text()) != now {
            self.patterns.update(cx, |f, cx| f.set_text(now.join(", "), cx));
        }
        cx.invalidate();
    }

    /// Delete backups `ids` (hex), and the unfinished one if `unfinished`,
    /// once the person says so.
    fn delete(&mut self, ids: Vec<String>, unfinished: bool, cx: &mut Cx<Self>) {
        let n = ids.len() + usize::from(unfinished);
        if n == 0 {
            return;
        }
        let sent = self.state.unfinished.as_ref().map_or(0, |u| u.sent);
        let message = match (ids.len(), unfinished) {
            (0, true) => olo_kit::tr!("Delete the unfinished backup? What it sent ({}) is freed, and the next backup reads and sends again what it had done. It can’t be undone.", size(sent)),
            (1, false) => olo_kit::words::tr("Delete this backup? The space only it took is freed; files other backups hold too stay in those. It can’t be undone.").to_owned(),
            (_, false) => olo_kit::tr!("Delete these {} backups? The space only they took is freed; files other backups hold too stay in those. It can’t be undone.", n),
            (_, true) => olo_kit::tr!(
                "Delete these {} backups, the unfinished one too? The space only they took is freed (what the unfinished one sent, {}); files other backups hold too stay in those. It can’t be undone.",
                n,
                size(sent)
            ),
        };
        self.confirm("Delete Backups", "warning", &message, "Delete", Request::Delete { snapshots: ids, unfinished }, cx);
    }

    /// An alert asking `message`: `request` asked of the service if the
    /// person presses `button`.
    fn confirm(&mut self, title: &str, icon: &str, message: &str, button: &str, request: Request, cx: &mut Cx<Self>) {
        let to = cx.messenger();
        open_alert(cx, title, icon, message, &["Cancel", button], move |choice, _| {
            if *choice == Some(1) {
                to.send(WinMsg::Ask(request.clone()));
            }
        });
    }

    /// Validate backups `ids` (hex), by the service (the header says how
    /// it goes; the list, how it went): fast at once; `full`y (every byte
    /// read back) once the person says so.
    fn validate(&mut self, ids: Vec<String>, full: bool, cx: &mut Cx<Self>) {
        if ids.is_empty() {
            return;
        }
        if !full {
            return self.ask(Request::Validate { snapshots: ids, full }, "", cx);
        }
        let what = olo_kit::words::count(ids.len() as u64, "this backup needs", "these {n} backups need");
        let most = self.state.size.map(|b| format!(" (up to {})", size(b))).unwrap_or_default();
        let message = olo_kit::tr!(
            "Validate fully? Every byte {}{} is read back from {} and checked, as a restore would: it can take a while, at the download limit. The usual Validate compares checksums and reads only a sample back.",
            what,
            most,
            self.settings.target.describe()
        );
        self.confirm("Validate Fully", "info", &message, "Validate Fully", Request::Validate { snapshots: ids, full: true }, cx);
    }

    /// Where backups go looked at (on a worker): unless it was for this
    /// target already, or `again`.
    fn look_at_target(&mut self, again: bool, cx: &mut Cx<Self>) {
        let t = self.settings.target.clone();
        if self.lint || t == Target::None || (!again && self.where_.as_ref().is_some_and(|(w, _)| *w == t)) {
            return;
        }
        self.where_ = Some((t.clone(), None));
        cx.work(
            move || {
                let r = target::look_at(&t);
                (t, r)
            },
            |(t, r)| WinMsg::Where(t, r),
        );
    }

    /// Disconnect Google Drive, once the person says so: backing up there
    /// stopped, and DiskBackup's sign-in forgotten on this computer (its
    /// client kept, to connect again); the backups there stay.
    fn disconnect(&mut self, cx: &mut Cx<Self>) {
        let Target::GoogleDrive { account, .. } = &self.settings.target else { return };
        let message = olo_kit::tr!(
            "Disconnect Google Drive ({})? DiskBackup stops backing up there, and forgets its sign-in on this computer. The backups in your Drive stay as they are: connect again to go on from them, or to restore from them.",
            account
        );
        let to = cx.messenger();
        open_alert(cx, "Disconnect Google Drive", "warning", &message, &["Cancel", "Disconnect"], move |choice, _| {
            if *choice == Some(1) {
                to.send(WinMsg::Disconnect);
            }
        });
    }

    fn disconnect_now(&mut self, cx: &mut Cx<Self>) {
        self.busy = true;
        self.note = Some((olo_kit::words::tr("Disconnecting from Google Drive…").into(), false));
        cx.work(|| crate::disconnect_drive().map(|said| format!("{said}.")), WinMsg::Note);
    }

    /// Every backup deleted, to start fresh, once the person says so.
    fn erase_all(&mut self, cx: &mut Cx<Self>) {
        let at = self.settings.target.describe();
        let then = if self.settings.automatic { "A fresh backup starts right after, copying everything again." } else { "The next backup (when you click Back Up Now) copies everything again." };
        let message =
            olo_kit::tr!("Erase every backup in {}? Every version of every file kept there goes (another computer’s backups there too), and the space is freed. {} It can’t be undone.", at, then);
        self.confirm("Erase All Backups", "warning", &message, "Erase All Backups", Request::EraseAll, cx);
    }

    /// The settings changed with `f` (the service follows them at once):
    /// a step for Edit ▸ Undo (typing the patterns, one a run).
    fn change(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Cx<Self>) {
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

    fn save(&mut self, s: Settings, cx: &mut Cx<Self>) {
        if let Err(e) = s.save() {
            self.note = Some((olo_kit::tr!("Couldn’t save the settings: {}", olo_kit::words::of_said(&e.to_string())), true));
        }
        self.settings = s;
        self.settings_changed(cx);
    }

    /// Edit ▸ Undo (`redo` false) or Redo: what the step changed put
    /// back, the rest left as it is now.
    fn undo_redo(&mut self, redo: bool, cx: &mut Cx<Self>) {
        let Some(step) = (if redo { self.history.take_redo() } else { self.history.take_undo() }) else { return };
        let mut s = self.settings.clone();
        put_back(&mut s, &step.change.before, &step.change.after);
        let purpose = if redo { Purpose::Redo } else { Purpose::Undo };
        self.history.record(purpose, step.name, Changed { before: step.change.after, after: step.change.before });
        self.save(s, cx);
    }

    /// A menu's or a button's command.
    fn do_command(&mut self, cmd: &str, cx: &mut Cx<Self>) {
        match cmd {
            "back-up-now" => self.ask(Request::BackUpNow, "", cx),
            // Paused: nothing backed up or sent (the backups kept are still
            // listed and restorable); resumed, it goes on as it was set.
            "pause" => self.change(|s| s.enabled = false, cx),
            "resume" => self.change(|s| s.enabled = true, cx),
            "stop" => {
                // (Said at once: keeping what's done can take a few seconds.)
                if self.state.doing == Doing::BackingUp
                    && let Some(p) = self.state.progress.as_mut()
                {
                    p.stage = Stage::Stopping;
                }
                self.ask(Request::Stop, "", cx);
            }
            // (A panel answers on its own looper: it tells this window.)
            "choose-folder" => {
                let to = cx.messenger();
                file_panel::show("Back Up to a Folder", FilePanel::folder(target::home()), move |a| {
                    if let PanelResult::Chosen(path) = a.result {
                        to.send(WinMsg::SetUpFolder(path));
                    }
                });
            }
            "google-drive" => olo_kit::proxy().send(Msg::OpenDrive),
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
                    self.note = Some((olo_kit::tr!("It didn’t open: {}.", olo_kit::words::of_said(&e.to_string())), true));
                }
            }
            "add-folder" => {
                let to = cx.messenger();
                file_panel::show("Back Up a Folder", FilePanel::folder(target::home()), move |a| {
                    if let PanelResult::Chosen(path) = a.result {
                        to.send(WinMsg::AddFolder(path));
                    }
                });
            }
            "keep-key" => self.keep_key(cx),
            "erase-all" => self.erase_all(cx),
            "undo" => self.undo_redo(false, cx),
            "redo" => self.undo_redo(true, cx),
            "restore" => olo_kit::proxy().send(Msg::OpenRestore),
            "close" => cx.close_window(),
            "quit" => olo_kit::quit(),
            _ => {}
        }
        cx.invalidate();
    }

    /// Ask the service `r` (on a worker: it's quick, but nothing waits);
    /// `said` once it's asked ("": nothing, the header shows it), or why
    /// it couldn't be.
    fn ask(&mut self, r: Request, said: &'static str, cx: &mut Cx<Self>) {
        cx.work(move || crate::ask(r).map(|()| said.to_owned()), WinMsg::Note);
        cx.invalidate();
    }

    /// Back up to `dir`: a backup there already opened with this
    /// computer's key, else a new one made (and a key, if there's none);
    /// the key kept in the keyring and in Passwords. On a worker.
    fn set_up_folder(&mut self, dir: PathBuf, cx: &mut Cx<Self>) {
        self.busy = true;
        self.note = Some((olo_kit::tr!("Setting up backups to {}…", dir.display()), false));
        cx.work(
            move || {
                crate::set_up_folder(&dir.display().to_string()).map(|()| {
                    let mut s = Settings::load();
                    match keep_in_passwords() {
                        Ok(()) => {
                            s.key_in_passwords = true;
                            let _ = s.save();
                            olo_kit::tr!("Backing up to {}. Its key is kept in Passwords.", dir.display())
                        }
                        Err(e) => olo_kit::tr!("Backing up to {}. Keep its key in Passwords when you can ({}).", dir.display(), e),
                    }
                })
            },
            WinMsg::Note,
        );
    }

    fn keep_key(&mut self, cx: &mut Cx<Self>) {
        self.busy = true;
        self.note = Some((olo_kit::words::tr("Keeping the key in Passwords (unlock it if it asks)…").into(), false));
        cx.work(
            || {
                keep_in_passwords().map(|()| {
                    let mut s = Settings::load();
                    s.key_in_passwords = true;
                    let _ = s.save();
                    olo_kit::words::tr("The key is kept in Passwords as “DiskBackup Key”.").to_owned()
                })
            },
            WinMsg::Note,
        );
    }
}

// ---- words for times, sizes and amounts ---------------------------------

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// "just now", "12 minutes ago", "3 hours ago", "yesterday at 14:05",
/// "on Tue at 14:05", "on 3 Nov": said again as it changes
/// (`olo_kit::ago`), in a view's body.
pub fn ago(t: i64) -> String {
    olo_kit::ago::ago_secs(t, move |age| ago_at(t, t + age.as_secs() as i64))
}

/// [`ago`] as said at `now`.
fn ago_at(t: i64, now: i64) -> String {
    use chrono::{Local, TimeZone};
    let d = (now - t).max(0);
    match d {
        0..60 => olo_kit::words::tr("just now").into(),
        60..3600 => olo_kit::words::count((d / 60) as u64, "a minute ago", "{n} minutes ago"),
        3600..43_200 => olo_kit::words::count((d / 3600) as u64, "an hour ago", "{n} hours ago"),
        _ => {
            let Some(at) = Local.timestamp_opt(t, 0).single() else { return String::new() };
            let Some(then) = Local.timestamp_opt(now, 0).single() else { return String::new() };
            let days = (then.date_naive() - at.date_naive()).num_days();
            match days {
                0 => olo_kit::tr!("today at {}", at.format("%H:%M")),
                1 => olo_kit::tr!("yesterday at {}", at.format("%H:%M")),
                // (The day's and month's names are English until Olo's
                // formats follow the language.)
                2..7 => olo_kit::tr!("on {} at {}", at.format("%a"), at.format("%H:%M")),
                _ => olo_kit::tr!("on {}", at.format("%-d %b %Y")),
            }
        }
    }
}

/// "in a minute", "in 48 minutes", "at 15:00", "tomorrow at 03:00".
pub fn until(t: i64) -> String {
    use chrono::{Local, TimeZone};
    let d = t - now();
    match d {
        ..=60 => olo_kit::words::tr("in a minute").into(),
        61..3600 => olo_kit::words::count(((d + 59) / 60) as u64, "in a minute", "in {n} minutes"),
        _ => {
            let Some(at) = Local.timestamp_opt(t, 0).single() else { return String::new() };
            if at.date_naive() == Local::now().date_naive() { olo_kit::tr!("at {}", at.format("%H:%M")) } else { olo_kit::tr!("tomorrow at {}", at.format("%H:%M")) }
        }
    }
}

/// A count said in the person's language, its number grouped as Olo
/// groups it ("312,004 files").
fn counted(n: u64, one: &'static str, other: &'static str) -> String {
    olo_kit::words::count(n, one, other).replacen(&n.to_string(), &count(n), 1)
}

fn minutes(n: i64) -> String {
    olo_kit::words::count(n.max(0) as u64, "a minute", "{n} minutes")
}

/// "12 seconds", "4 minutes", "1 hour 5 minutes".
pub fn took(secs: f64) -> String {
    let s = secs.round() as i64;
    match s {
        ..1 => olo_kit::words::tr("under a second").into(),
        1..60 => olo_kit::words::count(s as u64, "1 second", "{n} seconds"),
        60..3600 => minutes((s + 30) / 60),
        _ => {
            let (h, m) = (s / 3600, (s % 3600 + 30) / 60);
            let h = olo_kit::words::count(h as u64, "1 hour", "{n} hours");
            if m == 0 { h } else { format!("{h} {}", minutes(m)) }
        }
    }
}

/// What's kept, said plainly: "Every backup from the last hour, then one
/// an hour for 24 hours, one a day for 14 days, one a week for 8 weeks and
/// one a month for 24 months."
pub fn keep_words(k: &olo_protocol::diskbackup::Keep) -> String {
    let mut parts = Vec::new();
    // (Each said whole, counted: "one a day for 14 days"; every literal
    // in its call, where the word gatherer finds it.)
    let mut add = |n: u32, ever: &str, counted: String| {
        if n == u32::MAX {
            parts.push(ever.to_owned());
        } else if n > 0 {
            parts.push(counted);
        }
    };
    use olo_kit::words::count;
    add(k.hourly, olo_kit::words::tr("one an hour for ever"), count(u64::from(k.hourly), "one an hour for 1 hour", "one an hour for {n} hours"));
    add(k.daily, olo_kit::words::tr("one a day for ever"), count(u64::from(k.daily), "one a day for 1 day", "one a day for {n} days"));
    add(k.weekly, olo_kit::words::tr("one a week for ever"), count(u64::from(k.weekly), "one a week for 1 week", "one a week for {n} weeks"));
    add(k.monthly, olo_kit::words::tr("one a month for ever"), count(u64::from(k.monthly), "one a month for 1 month", "one a month for {n} months"));
    match parts.len() {
        0 => olo_kit::words::tr("Every backup from the last hour; older ones go.").into(),
        1 => olo_kit::tr!("Every backup from the last hour, then {}.", parts[0]),
        n => olo_kit::tr!("Every backup from the last hour, then {} and {}.", parts[..n - 1].join(", "), parts[n - 1]),
    }
}

/// `t` with its first letter a capital.
fn capital(t: &str) -> String {
    let mut c = t.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

/// Folder `p` (as the settings keep it) as the window says it.
fn folder_name(p: &str) -> String {
    if p == "~" { olo_kit::words::tr("Your home folder").into() } else { p.to_owned() }
}

/// What a backup that didn't finish got through, in a line.
pub fn unfinished_words(u: &olo_protocol::diskbackup::Unfinished) -> String {
    if u.files == 0 {
        olo_kit::tr!("Unfinished: {} sent before it stopped; the next backup goes on from there.", size(u.sent))
    } else {
        olo_kit::tr!("Unfinished: {} ({}) done and {} sent; the next backup goes on from there.", counted(u.files, "1 file", "{n} files"), size(u.bytes), size(u.sent))
    }
}

/// A backup's check, in a list's cell.
pub fn check_cell(c: Option<&Check>) -> &'static str {
    match c {
        None => "—",
        Some(c) if c.error.is_some() => "Not checked",
        Some(c) if !c.problems.is_empty() => "✗ Damaged",
        Some(c) if c.full => "✓ Full",
        Some(_) => "✓ Fast",
    }
}

/// A backup's check, said in a line.
pub fn check_words(c: Option<&Check>) -> String {
    let Some(c) = c else { return olo_kit::words::tr("Not checked yet: Validate reads it back and checks it’s whole.").into() };
    if let Some(e) = &c.error {
        return olo_kit::tr!("Couldn’t be checked {}: {}.", ago(c.time), olo_kit::words::of_said(e));
    }
    if !c.problems.is_empty() {
        let mut t = format!("Damaged (found {}): {}.", ago(c.time), c.problems.join("; "));
        if !c.files.is_empty() {
            let more = c.files_affected.saturating_sub(c.files.len() as u64);
            let home = target::home().display().to_string();
            let named: Vec<String> = c.files.iter().map(|f| f.strip_prefix(&home).map_or(f.clone(), |r| format!("~{r}"))).collect();
            let named = named.join(", ");
            t.push_str(&format!(" {}", if more > 0 { olo_kit::tr!("Can’t come back as it was: {} and {} more.", named, more) } else { olo_kit::tr!("Can’t come back as it was: {}.", named) }));
        }
        t.push_str(&format!(" {}", olo_kit::words::tr("The next backup sends again what it can.")));
        return t;
    }
    if c.full {
        return olo_kit::tr!("Validated fully {}: all it needs ({}) read back, and whole.", ago(c.time), size(c.bytes_read));
    }
    let mut how = Vec::new();
    if c.by_checksum > 0 {
        how.push(olo_kit::words::count(c.by_checksum as u64, "1 pack as sent, by its checksums", "{n} packs as sent, by their checksums"));
    }
    if c.sampled > 0 {
        how.push(olo_kit::tr!("{} read back", counted(c.sampled, "1 sample", "{n} samples")));
    }
    let how = if how.is_empty() { String::new() } else { format!(" ({})", how.join("; ")) };
    olo_kit::tr!("Validated {}: everything it needs is there{}. Validate Fully reads it all back.", ago(c.time), how)
}

// ---- the header: whether your files are safe -----------------------------

/// The headline, what's under it, and whether it's bad news.
pub fn headline(st: &State, s: &Settings) -> (String, String, bool) {
    let to = olo_kit::tr!("To {}", s.target.describe());
    if st.pid == 0 {
        return (olo_kit::words::tr("DiskBackup isn’t running").into(), olo_kit::words::tr("Its service starts with your session: log out and in again, or ask Olo Doctor to start it.").into(), true);
    }
    let progress = st.progress.clone().unwrap_or_default();
    // (Under way, with the network gone: it waits, and goes on.)
    // (Each said whole: what goes on is the sentence's subject.)
    let goes_on = match &st.doing {
        Doing::BackingUp => Some(olo_kit::words::tr("The backup goes on by itself once it’s back, from where it got to.")),
        Doing::Validating => Some(olo_kit::words::tr("Validating goes on by itself once it’s back, from where it got to.")),
        Doing::Pruning | Doing::Deleting { .. } => Some(olo_kit::words::tr("Tidying goes on by itself once it’s back, from where it got to.")),
        Doing::Erasing => Some(olo_kit::words::tr("Erasing goes on by itself once it’s back, from where it got to.")),
        _ => None,
    };
    if let Some(goes_on) = goes_on.filter(|_| progress.offline) {
        // (Where it goes is in Destination: the line has room for this.)
        return (olo_kit::words::tr("Waiting for the network").into(), goes_on.into(), false);
    }
    match &st.doing {
        Doing::BackingUp => {
            let title = match progress.stage {
                // (Titles fit beside the buttons: the lint draws each.)
                Stage::Counting => olo_kit::words::tr("Counting what to back up"),
                Stage::Reading if progress.resuming => olo_kit::words::tr("Resuming the backup"),
                // ("Backing up your home folder": what, said.)
                Stage::Reading => return reading_headline(&progress, s),
                Stage::Saving => olo_kit::words::tr("Finishing the backup"),
                Stage::Checking => return (olo_kit::words::tr("Checking the backup").into(), olo_kit::tr!("Everything it needs looked for, in {}.", s.target.describe()), false),
                Stage::Stopping => return (olo_kit::words::tr("Stopping the backup").into(), olo_kit::words::tr("Finishing what’s being sent, then keeping what’s done.").into(), false),
            };
            // (Reading: the file it's at, under the title; where it goes
            // is in Destination.)
            if progress.stage == Stage::Reading && !progress.current.is_empty() {
                let home = target::home().display().to_string();
                let current = match progress.current.strip_prefix(&home) {
                    Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
                    _ => progress.current.clone(),
                };
                return (title.into(), olo_kit::tr!("Now: {}", current), false);
            }
            return (title.into(), format!("{to}."), false);
        }
        Doing::Pruning => return (olo_kit::words::tr("Tidying old backups").into(), olo_kit::tr!("Freeing the space only backups past keeping took, in {}.", s.target.describe()), false),
        Doing::Deleting { what } => {
            let one = what == "the unfinished backup" || what == "a backup";
            // (`what` is the service's, in English: compared, not shown.)
            let title = if what == "the unfinished backup" {
                olo_kit::words::tr("Deleting the unfinished backup")
            } else if one {
                olo_kit::words::tr("Deleting a backup")
            } else {
                olo_kit::words::tr("Deleting backups")
            };
            let freed = if one {
                olo_kit::tr!("Then the space only it took is freed, in {}.", s.target.describe())
            } else {
                olo_kit::tr!("Then the space only they took is freed, in {}.", s.target.describe())
            };
            return (title.into(), freed, false);
        }
        Doing::Restoring => return (olo_kit::words::tr("Restoring").into(), to, false),
        Doing::Erasing => return (olo_kit::words::tr("Erasing every backup").into(), olo_kit::tr!("{}. The next backup starts afresh.", to), false),
        Doing::Validating => return (olo_kit::words::tr("Validating backups").into(), olo_kit::tr!("Every byte read back and checked, from {}.", s.target.describe()), false),
        // Not backing up, and why, in its own words (Ben, 2026-11-18: "make
        // sure the backup app itself also shows the status as to whether
        // it is running or paused or whatnot"), with when the last one
        // was: the backups kept are still listed and restorable.
        Doing::Waiting { why } => {
            let last = st.last_ok.map_or(olo_kit::words::tr("No backup has finished yet").to_owned(), |t| olo_kit::tr!("The last backup was {}", ago(t)));
            let (title, detail) = match why.as_str() {
                "turned off" => (olo_kit::words::tr("Paused").to_owned(), olo_kit::tr!("Nothing is backed up or sent until you resume. {}. {}.", last, to)),
                w if w.starts_with("on battery") => (olo_kit::words::tr("Waiting for power").to_owned(), olo_kit::tr!("It backs up once the computer is plugged in. {}. {}.", last, to)),
                w if w.starts_with("no wired network") => {
                    (olo_kit::words::tr("Waiting for Ethernet").to_owned(), olo_kit::tr!("It backs up once the computer is connected by a cable. {}. {}.", last, to))
                }
                w if w.starts_with("no network") => (olo_kit::words::tr("Waiting for the network").to_owned(), olo_kit::tr!("It goes on once there’s one. {}. {}.", last, to)),
                // (The service's reason, said in English: in the person's
                // language here.)
                w => {
                    let mut w = olo_kit::words::of_said(w);
                    if let Some(c) = w.chars().next() {
                        w.replace_range(..c.len_utf8(), &c.to_uppercase().to_string());
                    }
                    (olo_kit::words::tr("Waiting").to_owned(), format!("{w}. {to}."))
                }
            };
            return (title, detail, false);
        }
        Doing::Idle => {}
    }
    let next = match st.next {
        Some(n) if s.automatic => olo_kit::tr!("Next {}.", until(n)),
        _ => olo_kit::words::tr("Next when you click Back Up Now.").into(),
    };
    match (&st.last, st.last_ok) {
        (Some(l), _) if l.error.is_none() => (olo_kit::tr!("Backed up {}", ago(l.time)), format!("{to}. {next}"), now() - l.time > 3 * 24 * 3600),
        // (Stopped: the person's doing, not trouble, unless nothing whole
        // is recent.)
        (Some(l), ok) if l.stopped() => {
            let whole = ok.map_or(olo_kit::words::tr("No backup has finished yet").to_owned(), |t| olo_kit::tr!("The last whole one was {}", ago(t)));
            (olo_kit::words::tr("Backup stopped").into(), olo_kit::tr!("You stopped it {}. {}. {}", ago(l.time), whole, next), ok.is_some_and(|t| now() - t > 3 * 24 * 3600))
        }
        (Some(l), ok) => {
            let whole = ok.map_or(olo_kit::words::tr("No backup has finished yet").to_owned(), |t| olo_kit::tr!("The last whole one was {}", ago(t)));
            let when = ago(l.time);
            let when = when.get(..1).map_or(when.clone(), |c| format!("{}{}", c.to_uppercase(), &when[1..]));
            (olo_kit::words::tr("Backup didn’t finish").into(), olo_kit::tr!("{}: {}. {}. {}", when, olo_kit::words::of_said(l.error.as_deref().unwrap_or_default()), whole, next), true)
        }
        (None, _) => (
            olo_kit::words::tr("Not backed up yet").into(),
            format!("{to}. {}", if s.automatic { olo_kit::words::tr("The first backup starts in a moment.") } else { olo_kit::words::tr("Click Back Up Now to make the first.") }),
            false,
        ),
    }
}

/// Backing up, reading: what (your home folder), and the file it's at.
fn reading_headline(p: &Progress, s: &Settings) -> (String, String, bool) {
    // (What's backed up, said whole in each language.)
    let title = match s.paths.as_slice() {
        [p] if p == "~" => olo_kit::words::tr("Backing up your home folder").to_owned(),
        [p] => olo_kit::tr!("Backing up {}", p),
        ps => olo_kit::words::count(ps.len() as u64, "Backing up 1 folder", "Backing up {n} folders"),
    };
    if p.current.is_empty() {
        return (title, olo_kit::tr!("To {}.", s.target.describe()), false);
    }
    let home = target::home().display().to_string();
    let current = match p.current.strip_prefix(&home) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => p.current.clone(),
    };
    (title, olo_kit::tr!("Now: {}", current), false)
}

/// Something to see to, in a line: a small mark, what's wrong, and the
/// button that sees to it, on the banners' tint (a `Banner` is for news;
/// this is a reminder).
fn notice(text: impl Into<SharedString>, button: Button) -> Element {
    const MARK: f32 = 16.0;
    let back = canvas(|g, b| widgets::banner_back(g, b, Some(Tone::Failure), &theme::palette())).paint_key(style::key("notice")).absolute().left(px(0.0)).top(px(0.0)).size_full();
    let mark = canvas(|g, b| widgets::verdict_mark(g, b, Tone::Failure, MARK, &theme::palette())).paint_key(style::key("notice-mark")).w(px(MARK)).h(px(MARK)).flex_shrink_0();
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .px(px(m::SPACING))
        .py(px(m::TIGHT + 2.0))
        .child(back)
        .child(mark)
        .child(div().flex_1().min_w_0().child(StringView::new(text.into()).wrap(true)))
        .child(button)
}

/// A picture `name`, `size` points square, crisp.
fn picture(name: &'static str, size: f32) -> Element {
    canvas(move |g, b| {
        let at = style::snap_centred(g, b.center(), size, size);
        icons::paint(g, at, name);
    })
    .paint_key(style::key((name, size.to_bits())))
    .w(px(size))
    .h(px(size))
    .flex_shrink_0()
}

impl MainWindow {
    fn menus(&self) -> Vec<Menu> {
        let set_up = self.settings.target != Target::None;
        let running = busy_with(&self.state);
        vec![
            Menu::new("File")
                .app_items(&["backup"])
                .item("Back Up Now", "back-up-now")
                .key("b")
                .enabled(set_up && !running && self.settings.enabled)
                .item("Stop", "stop")
                .enabled(running)
                .item(if self.settings.enabled { "Pause Backups" } else { "Resume Backups" }, if self.settings.enabled { "pause" } else { "resume" })
                .enabled(set_up)
                .item("Restore…", "restore")
                .key("r")
                .enabled(set_up)
                .separator()
                .item("Erase All Backups…", "erase-all")
                .enabled(set_up && !running)
                .close_quit(),
            self.history.edit_items(Menu::new("Edit")),
        ]
    }
}

impl View for MainWindow {
    type Message = WinMsg;

    fn message(&mut self, message: WinMsg, cx: &mut Cx<Self>) {
        match message {
            WinMsg::Changed(settings, state) => {
                let was = (self.state.last_ok, self.state.snapshots, self.state.doing.clone(), self.state.read);
                self.settings = settings;
                self.state = state;
                // Backups made, deleted, erased or validated, or read by
                // the service as it started: listed again.
                if (self.state.last_ok, self.state.snapshots, self.state.doing.clone(), self.state.read) != was {
                    self.load_snapshots(cx);
                }
            }
            WinMsg::Snapshots(r) => self.snapshots = r,
            WinMsg::Where(t, r) => match &mut self.where_ {
                Some((now, looked)) if *now == t => *looked = Some(r),
                _ => return,
            },
            WinMsg::Note(r) => {
                self.busy = false;
                self.settings = Settings::load();
                // (Nothing said when the window shows it already.)
                self.note = match r {
                    Ok(s) if s.is_empty() => None,
                    Ok(s) => Some((s, false)),
                    Err(e) => Some((e, true)),
                };
            }
            WinMsg::Ask(r) => return self.ask(r, "", cx),
            WinMsg::Disconnect => self.disconnect_now(cx),
            WinMsg::SetUpFolder(dir) => self.set_up_folder(dir, cx),
            WinMsg::AddFolder(path) => return self.change(|s| s.paths.push(tilde(&path)), cx),
            WinMsg::SetUp => {
                self.settings = Settings::load();
                self.note = None;
            }
        }
        self.settings_changed(cx);
    }

    fn command(&mut self, name: &str, cx: &mut Cx<Self>) -> bool {
        self.do_command(name, cx);
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let (s, st) = (self.settings.clone(), self.state.clone());
        let set_up = s.target != Target::None;
        let running = busy_with(&st);
        let bar = MenuBar::of(self, cx, |w, _| w.menus());
        let p = theme::palette();
        let mut body = div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET));
        if !set_up {
            body = body.child(self.welcome(cx));
        } else {
            body = body.child(self.header(&s, &st, cx));
            if !s.key_in_passwords {
                body = body.child(notice(
                    "The key is only on this computer: keep it in Passwords, to restore anywhere else.",
                    Button::new("Keep Key in Passwords").enabled(!self.busy).on_click(self.cmd(cx, "keep-key")),
                ));
            }
            // The latest backup found damaged when checked (not while one
            // runs: that's what sends again what's lost).
            if !running && let Some(c) = st.last.as_ref().and_then(|l| l.check.as_ref()).filter(|c| !c.problems.is_empty()) {
                body = body.child(notice(
                    olo_kit::tr!("The last backup isn’t whole: {}. The next backup sends again what it can.", c.problems.join("; ")),
                    Button::new("Back Up Now").id("damage-back-up").enabled(!running).on_click(self.cmd(cx, "back-up-now")),
                ));
            }
            let items: Vec<Element> = match self.tab {
                0 => self.schedule(&s, cx),
                1 => self.files(&s, &st, cx),
                2 => self.backups(&s, &st, cx),
                3 => self.speed(&s, cx),
                _ => self.destination(&s, &st, cx),
            };
            let tabs: Vec<SharedString> = TABS.into_iter().map(SharedString::from).collect();
            // (In a column of its own that takes the rest: a growing tab
            // view is at least as tall as what holds it.)
            body = body.child(
                div().flex_1().flex().flex_col().child(
                    TabView::new("diskbackup-tabs", tabs, self.tab)
                        .grows()
                        .on_select(cx.listener(|w: &mut MainWindow, i: &usize, cx| {
                            w.tab = *i;
                            // (Looked at again each time it's chosen.)
                            if *i == DESTINATION {
                                w.look_at_target(true, cx);
                            }
                            cx.invalidate();
                        }))
                        .child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items)),
                ),
            );
        }
        if let Some((text, error)) = self.note.clone() {
            body = body.child(StringView::new(text).wrap(true).color(if error { style::c(p.failure) } else { style::disabled_text(&p) }));
        }
        div().size_full().flex().flex_col().child(bar).child(body)
    }
}

impl MainWindow {
    /// A button's click: command `c`.
    fn cmd(&self, cx: &Cx<Self>, c: &'static str) -> impl Fn(&ClickEvent, &mut WindowCx) + 'static {
        cx.listener(move |w: &mut MainWindow, _: &ClickEvent, cx| w.do_command(c, cx))
    }

    /// A control's change: the settings changed with `f` and what it says.
    fn sets<E: Copy + 'static>(&self, cx: &Cx<Self>, f: impl Fn(&mut Settings, E) + 'static) -> impl Fn(&E, &mut WindowCx) + 'static {
        cx.listener(move |w: &mut MainWindow, e: &E, cx| {
            let e = *e;
            w.change(|s| f(s, e), cx)
        })
    }

    /// Not set up: the one question, where backups go.
    fn welcome(&self, cx: &mut Cx<Self>) -> Element {
        let busy = self.busy;
        let width = f32::from(cx.viewport_size().width);
        let side_by_side = width >= CHOICES_SIDE_BY_SIDE;
        // (Its heading on one line: smaller where the title's size would
        // wrap it, "computer" alone on a line of its own in a third of a
        // screen; Opus's and Neon's reviews, 2026-11-28.)
        let room = width - 2.0 * m::WINDOW_INSET - ICON - HERO_GAP;
        let heading = welcome_heading_size(f32::from(style::text_width(olo_kit::words::tr("Back up this computer").to_owned(), style::TITLE_SIZE, style::ui_font(olo_kit::FontWeight::BOLD))), room);
        let dim = |t: &'static str| StringView::new(t).wrap(true).dim(true);
        // (Side by side, as tall as the taller, each button at its box's
        // foot: their buttons level whatever their words' lengths. They
        // were level only while the two said as much, and French's
        // Google Drive ran a line longer.)
        let choice = |icon: &'static str, title: &'static str, about: &'static str, button: Button| {
            BBox::new(title)
                .grow()
                .child(div().flex().flex_row().items_start().gap(px(m::SPACING)).child(picture(icon, 32.0)).child(div().flex_1().min_w_0().child(dim(about))))
                .child(div().flex_1().flex().flex_row().justify_end().items_end().child(button))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(
                // (The picture level with the heading's top, its words where
                // the boxes' are under it: centred on the whole, it floated
                // low beside the paragraph, the words 14 points right of the
                // boxes'; Opus's and Neon's reviews, 2026-11-28.)
                div().flex().flex_row().items_start().gap(px(HERO_GAP)).child(picture("diskbackup", ICON)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(m::TIGHT))
                        // (Wrapped beside the picture where even that's too narrow.)
                        .child(StringView::new("Back up this computer").bold(true).size(heading).wrap(true))
                        .child(
                            StringView::new("DiskBackup copies your files, encrypted, every hour, and keeps older copies too, so any file can be brought back as it was. Where should the backups go?")
                                .wrap(true),
                        ),
                ),
            )
            .child(
                // (One over the other where two side by side would be too
                // narrow for their buttons: a third of a screen, held so by
                // tiling, ran Connect Google Drive… out of its box.)
                // (A tight spacing more over them: a box's name stands over
                // its frame, and the paragraph's last line sat nearer it
                // than the boxes are to each other.)
                if side_by_side { div().flex().flex_row() } else { div().flex().flex_col() }
                    .pt(px(m::TIGHT))
                    .gap(px(m::SPACING))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(choice(
                        "disk",
                        "A Disk or Folder",
                        "A disk of its own, or a folder on one: your backups kept apart from this computer.",
                        Button::new("Choose a Folder…").enabled(!busy).on_click(self.cmd(cx, "choose-folder")),
                    )))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(choice(
                        "diskbackup",
                        "Google Drive",
                        "Your Google Drive keeps them anywhere: sign in once, or let an agent set it up.",
                        Button::new("Connect Google Drive…").enabled(!busy).on_click(self.cmd(cx, "google-drive")),
                    ))),
            )
            .child(dim("Your home folder is backed up every hour, leaving out what can be downloaded or built again. Once it’s set up, you can change any of it."))
    }

    /// Set up: how it stands, and Back Up Now (or Stop).
    fn header(&self, s: &Settings, st: &State, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let (title, detail, bad) = headline(st, s);
        let running = busy_with(st);
        let mut buttons = div().flex().flex_row().gap(px(m::SPACING)).flex_shrink_0();
        buttons = buttons.child(Button::new("Restore…").on_click(self.cmd(cx, "restore")));
        let stopping = st.progress.as_ref().is_some_and(|p| p.stage == Stage::Stopping);
        // (Paused, backing up now does nothing: Resume is what's next.)
        buttons = buttons.child(if running {
            Button::new("Stop").enabled(!stopping).on_click(self.cmd(cx, "stop"))
        } else if !s.enabled {
            Button::new("Resume").default(true).enabled(st.pid != 0).on_click(self.cmd(cx, "resume"))
        } else {
            Button::new("Back Up Now").default(true).enabled(st.pid != 0).on_click(self.cmd(cx, "back-up-now"))
        });
        let mut words = div().flex_1().min_w_0().flex().flex_col().gap(px(m::TIGHT));
        // (One line, never past the buttons: a folder named by its path
        // can be any length, cut short then, whole in its tip.)
        let mut t = StringView::new(title).bold(true).size(style::TITLE_SIZE).height(style::line_row(style::TITLE_SIZE)).cut(true);
        if bad {
            t = t.color(style::c(p.failure));
        }
        // (While it runs, the file it's at: one line, whatever its length.)
        let detail = if running { StringView::new(detail).cut(true).height(style::line_row(style::PLAIN_SIZE)) } else { StringView::new(detail).wrap(true) };
        // (The title beside the buttons; what's said under it has the
        // whole width, under the buttons too: in a language longer than
        // English, the paragraphs wrapped beside the buttons ran a line or
        // two longer and past the window's foot.)
        // (The buttons centred on the title's line.)
        let head = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(t)).child(buttons);
        words = words.child(head).child(detail);
        // While it runs, the whole of it (under, the window's width); else
        // the last one, in a line.
        let progress = if running && let Some(pr) = &st.progress { Some(self.progress(s, st, pr)) } else { None };
        if st.doing == Doing::Idle
            && let Some(l) = st.last.as_ref().filter(|l| l.error.is_none())
        {
            let mut said =
                olo_kit::tr!("Last: {}, {} new or changed, {} new, took {}.", counted(l.files, "1 file", "{n} files"), count(l.new_files + l.changed_files), size(l.new_bytes), took(l.seconds));
            if l.unreadable > 0 {
                said.push_str(&format!(" {}", olo_kit::words::count(l.unreadable, "1 file couldn’t be read.", "{n} files couldn’t be read.")));
            }
            match &l.check {
                Some(c) if c.whole() => said.push_str(&format!(" {}", olo_kit::words::tr("Validated: all there."))),
                Some(Check { error: Some(e), .. }) => said.push_str(&format!(" {}", olo_kit::tr!("Not checked ({}).", olo_kit::words::of_said(e)))),
                _ => {}
            }
            words = words.child(StringView::new(said).wrap(true).dim(true));
        }
        // What a backup that didn't finish got through.
        if st.doing == Doing::Idle
            && let Some(u) = &st.unfinished
        {
            words = words.child(StringView::new(unfinished_words(u)).wrap(true).dim(true));
        }
        let row = div().flex().flex_row().items_start().gap(px(m::SPACING * 2.0)).child(picture("diskbackup", ICON)).child(words);
        div().flex().flex_col().gap(px(m::TIGHT)).child(row).children(progress)
    }

    /// A backup (or a tidy, or erasing) under way: the whole of it.
    fn progress(&self, s: &Settings, st: &State, pr: &Progress) -> Element {
        let dim = |t: String| StringView::new(t).wrap(true).dim(true);
        let line = |t: String| StringView::new(t).wrap(true);
        let mut col = div().flex().flex_col().gap(px(m::TIGHT));
        match (&st.doing, pr.stage) {
            (Doing::BackingUp, Stage::Counting) => {
                col = col.child(line(olo_kit::tr!("Counting what there is to back up: {}, {} so far.", counted(pr.total_files, "1 file", "{n} files"), size(pr.total_bytes))));
                col = col.child(dim(olo_kit::words::tr("The first backup reads everything; later ones read only what changed, in a fraction of the time.").into()));
            }
            (Doing::BackingUp | Doing::Validating, Stage::Checking) | (Doing::Validating, _) => {
                // (The bar throughout, so nothing moves: empty till it knows
                // how much there is to read back.)
                let done = if pr.total_bytes == 0 { 0.0 } else { (pr.bytes as f32 / pr.total_bytes as f32).min(1.0) };
                let label = if pr.total_bytes == 0 { olo_kit::words::tr("Looking at what’s there").to_owned() } else { olo_kit::tr!("{} of {} read back", size(pr.bytes), size(pr.total_bytes)) };
                col = col.child(StatusBar::new(done).label(label).trailing(format!("{:.0}%", done * 100.0)));
                col = col.child(line(if pr.current.is_empty() { olo_kit::words::tr("Looking at what’s there…").into() } else { format!("{}…", olo_kit::words::of_said(&pr.current)) }));
            }
            // Stopping: the pack being sent, finished first (then what's
            // done is kept).
            (Doing::BackingUp, Stage::Stopping) => {
                let (bar, label, then) = match pr.sending_left() {
                    Some((done, left)) => (
                        done as f32 / pr.sending as f32,
                        olo_kit::tr!("Finishing sending a pack: {} of {}", size(done), size(pr.sending)),
                        format!("Then what’s done is kept, so the next backup goes on from there{}.", left.map(|l| format!(" ({})", olo_protocol::diskbackup::words::left(l))).unwrap_or_default()),
                    ),
                    None => (1.0, olo_kit::words::tr("Keeping what’s done…").to_owned(), olo_kit::words::tr("So the next backup goes on from there.").to_owned()),
                };
                col = col.child(StatusBar::new(bar.min(1.0)).label(label).trailing(format!("{:.0}%", bar.min(1.0) * 100.0)));
                col = col.child(line(olo_kit::tr!("{}, {} new to the backup, {} sent so far.", pr.of_files(), size(pr.new_bytes), size(pr.uploaded))));
                col = col.child(dim(then));
            }
            (Doing::BackingUp, stage) => {
                let (done, left) = pr.how_far(now());
                let saving = stage == Stage::Saving;
                // How far through all of it (what's unchanged since the
                // last backup is gone through without being read again).
                let label = if saving { olo_kit::words::tr("Writing the backup’s lists…").to_owned() } else { pr.gone_through() };
                col = col.child(StatusBar::new(if saving { 1.0 } else { done }).label(label).trailing(format!("{:.0}%", if saving { 100.0 } else { done * 100.0 })));
                let mut words = pr.of_files();
                if let Some(l) = left.filter(|_| !saving) {
                    words.push_str(&format!(" · {}", olo_protocol::diskbackup::words::left(l)));
                }
                col = col.child(line(words));
                // What the backup didn't have yet, and that sent.
                let spent = (now() - pr.started).max(1) as f64;
                let rate = pr.uploaded as f64 / spent;
                let limit = if s.upload_kbs > 0 { format!(", {}", olo_kit::tr!("at most {}", speed_label(s.upload_kbs))) } else { String::new() };
                col = col.child(dim(olo_kit::tr!("{} new to the backup · {} sent, compressed and encrypted ({}/s{})", size(pr.new_bytes), size(pr.uploaded), size(rate as u64), limit)));
                // Each folder's turn (they're gone through in order).
                if s.paths.len() > 1 {
                    let mut paths: Vec<(String, std::path::PathBuf)> = s.paths.iter().map(|p| (folder_name(p), target::expand(p))).collect();
                    paths.sort_by(|a, b| a.1.cmp(&b.1));
                    let now_at = paths.iter().rposition(|(_, p)| std::path::Path::new(&pr.current).starts_with(p));
                    let marks: Vec<String> = paths
                        .iter()
                        .enumerate()
                        .map(|(i, (name, _))| match now_at {
                            Some(n) if i < n => format!("✓ {name}"),
                            Some(n) if i == n => format!("▸ {name}"),
                            _ => format!("○ {name}"),
                        })
                        .collect();
                    col = col.child(dim(format!("Folders: {}", marks.join("   "))));
                }
            }
            (Doing::Erasing, _) => {
                let done = if pr.total_files == 0 { 0.0 } else { pr.files as f32 / pr.total_files as f32 };
                // (Google Drive: its folder deleted whole, one step.)
                let label = if pr.total_files == 0 {
                    format!("{}…", if pr.current.is_empty() { "Getting ready" } else { &pr.current })
                } else {
                    olo_kit::tr!("Deleting {} of {}", count(pr.files), count(pr.total_files))
                };
                col = col.child(StatusBar::new(done).label(label).trailing(format!("{:.0}%", done * 100.0)));
            }
            // Tidying, or deleting (a tidy after): the step it's on, how
            // many of how many, and (deleting what isn't used) what's freed.
            (Doing::Pruning | Doing::Deleting { .. }, _) => {
                let what = if pr.current.is_empty() { olo_kit::words::tr("Getting ready").to_owned() } else { pr.current.clone() };
                let done = if pr.total_files == 0 { 0.0 } else { (pr.files as f32 / pr.total_files as f32).min(1.0) };
                let label = match (pr.total_files, pr.files) {
                    (0, 0) => format!("{what}…"),
                    (0, n) => format!("{what}: {}", count(n)),
                    (t, n) => format!("{what}: {} of {}", count(n), count(t)),
                };
                col = col.child(StatusBar::new(done).label(label).trailing(if pr.total_files == 0 { String::new() } else { format!("{:.0}%", done * 100.0) }));
                let step = if pr.steps > 0 { olo_kit::tr!("Step {} of {}", pr.step.max(1), pr.steps) } else { "Starting".into() };
                let freed = if pr.total_bytes > 0 { format!(" · {}", olo_kit::tr!("{} of {} freed", size(pr.bytes), size(pr.total_bytes))) } else { String::new() };
                let spent = took((now() - pr.started).max(0) as f64);
                col = col.child(dim(olo_kit::tr!("{}{} · {} so far", step, freed, spent)));
            }
            _ => {}
        }
        col.pt(px(m::TIGHT))
    }

    // ---- the tabs -------------------------------------------------------

    fn schedule(&self, s: &Settings, cx: &mut Cx<Self>) -> Vec<Element> {
        let every_at = EVERY.iter().position(|(m, _)| *m == s.every_minutes);
        let mut names: Vec<SharedString> = EVERY.iter().map(|(_, n)| SharedString::from(*n)).collect();
        if every_at.is_none() {
            names.push(olo_kit::words::count(u64::from(s.every_minutes), "every minute", "every {n} minutes").into());
        }
        let every = MenuField::new("every", names, every_at.unwrap_or(EVERY.len())).enabled(s.automatic).on_change(self.sets(cx, |s, i: usize| {
            if let Some((m, _)) = EVERY.get(i) {
                s.every_minutes = *m;
            }
        }));
        let when = BBox::new("When to Back Up")
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    // (As tall as the menu beside it: placed alike, their words land
                    // on the same device pixels at 1.25× and 1.75× too.)
                    .child(RadioButton::new("Automatically,", s.automatic).height(m::CONTROL_HEIGHT).on_select(self.sets(cx, |s, (): ()| s.automatic = true)))
                    .child(every),
            )
            .child(RadioButton::new("Only when I click Back Up Now", !s.automatic).on_select(self.sets(cx, |s, (): ()| s.automatic = false)))
            .child(div().pt(px(m::TIGHT)).child(CheckBox::new("Also when running on battery", s.on_battery).on_toggle(self.sets(cx, |s, on: bool| s.on_battery = on))))
            .child(CheckBox::new("Only when connected to Ethernet", s.wired_only).on_toggle(self.sets(cx, |s, on: bool| s.wired_only = on)));
        // (Each column's fields begin together, after its wider label.)
        let (left, right) = (DIVIDER.max(widgets::divider_for(&["One an hour for:", "One a day for:"])), DIVIDER.max(widgets::divider_for(&["One a week for:", "One a month for:"])));
        let keep_menu = |id: &'static str, label: &'static str, choices: &'static [(u32, &'static str)], now: u32, divider: f32, set: fn(&mut Settings, u32)| {
            let at = choices.iter().position(|(n, _)| *n == now);
            let mut names: Vec<SharedString> = choices.iter().map(|(_, name)| SharedString::from(*name)).collect();
            if at.is_none() {
                names.push(now.to_string().into());
            }
            MenuField::new(id, names, at.unwrap_or(choices.len())).label(label).divider(divider).on_change(self.sets(cx, move |s, i: usize| {
                if let Some((n, _)) = choices.get(i) {
                    set(s, *n);
                }
            }))
        };
        let k = s.keep;
        let keep = BBox::new("Backups Kept")
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING * 2.0))
                    .child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).child(keep_menu("hourly", "One an hour for:", HOURLY, k.hourly, left, |s, n| s.keep.hourly = n)).child(keep_menu(
                        "daily",
                        "One a day for:",
                        DAILY,
                        k.daily,
                        left,
                        |s, n| s.keep.daily = n,
                    )))
                    .child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).child(keep_menu("weekly", "One a week for:", WEEKLY, k.weekly, right, |s, n| s.keep.weekly = n)).child(keep_menu(
                        "monthly",
                        "One a month for:",
                        MONTHLY,
                        k.monthly,
                        right,
                        |s, n| s.keep.monthly = n,
                    ))),
            )
            .child(StringView::new(keep_words(&k)).wrap(true).dim(true))
            .child(StringView::new("Older ones go, and the space only they took is freed, once a day. Delete any yourself under Backups.").wrap(true).dim(true));
        vec![when.into_element(), keep.into_element()]
    }

    fn files(&self, s: &Settings, st: &State, cx: &mut Cx<Self>) -> Vec<Element> {
        let mut what = BBox::new("Back Up These Folders");
        let n = s.paths.len();
        for (i, path) in s.paths.iter().enumerate() {
            let shown = folder_name(path);
            let sub = if path == "~" { target::home().display().to_string() } else { String::new() };
            what = what.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(picture("folder", 16.0))
                    .child(div().flex_1().min_w_0().flex().flex_row().gap(px(m::SPACING)).child(StringView::new(shown)).child(StringView::new(sub).dim(true)))
                    .child(Button::new("Remove").id(format!("remove-{i}")).enabled(n > 1).on_click(cx.listener(move |w: &mut MainWindow, _: &ClickEvent, cx| {
                        w.change(
                            |s| {
                                if i < s.paths.len() {
                                    s.paths.remove(i);
                                }
                            },
                            cx,
                        )
                    }))),
            );
        }
        what = what.child(
            div().flex().flex_row().items_center().gap(px(m::SPACING)).pt(px(m::TIGHT)).child(Button::new("Add a Folder…").on_click(self.cmd(cx, "add-folder"))).child(
                StringView::new(match &st.last {
                    Some(l) if l.error.is_none() => olo_kit::tr!("{} in {}, by the last backup.", size(l.bytes), counted(l.files, "1 file", "{n} files")),
                    _ => String::new(),
                })
                .dim(true),
            ),
        );
        // What can be made again: two columns, what each means in its tip.
        let chosen = s.exclude_groups.clone().unwrap_or_else(olo_diskbackup::exclude::default_groups);
        let check = |g: &'static olo_diskbackup::exclude::Group| {
            let on = chosen.iter().any(|c| c == g.id);
            let id = g.id;
            let check = CheckBox::new(g.name, on).on_toggle(self.sets(cx, move |s, on: bool| {
                let mut now = s.exclude_groups.clone().unwrap_or_else(olo_diskbackup::exclude::default_groups);
                now.retain(|g| g != id);
                if on {
                    now.push(id.to_owned());
                }
                s.exclude_groups = Some(now);
            }));
            tip::on(div().child(check), format!("leave-{id}"), g.about)
        };
        let half = GROUPS.len().div_ceil(2);
        let left = div().flex_1().min_w_0().flex().flex_col().gap(px(m::TIGHT)).children(GROUPS[..half].iter().map(&check));
        let right = div().flex_1().min_w_0().flex().flex_col().gap(px(m::TIGHT)).children(GROUPS[half..].iter().map(&check));
        // (What each covers: in its tip; how patterns go, in the field's
        // (and by its example, the placeholder).)
        let leave = BBox::new("Leave Out What Can Be Downloaded or Built Again").child(div().flex().flex_row().gap(px(m::SPACING * 2.0)).child(left).child(right)).child(tip::on(
            div().pt(px(m::SPACING)).child(TextControl::new(cx, &self.patterns).label("Also leave out:").divider(DIVIDER)),
            "leave-patterns",
            "Folders and files, separated by commas: ~/ is your home folder, **/ anywhere, * any name.",
        ));
        vec![what.into_element(), leave.into_element()]
    }

    /// The unfinished backup (listed first) and the backups kept.
    fn rows(&self) -> (Option<&olo_protocol::diskbackup::Unfinished>, &[SnapshotInfo]) {
        let snaps = match &self.snapshots {
            Some(Ok(l)) => l.as_slice(),
            _ => &[],
        };
        (self.state.unfinished.as_ref(), snaps)
    }

    fn backups(&self, s: &Settings, st: &State, cx: &mut Cx<Self>) -> Vec<Element> {
        let p = theme::palette();
        let (unfinished, snaps) = self.rows();
        // The unfinished one first (the next goes on from it), then each
        // kept, newest first.
        let first = usize::from(unfinished.is_some());
        let said = match &self.snapshots {
            None => olo_kit::words::tr("Reading the backups…").to_owned(),
            Some(Err(e)) => olo_kit::tr!("The backups can’t be listed: {}.", olo_kit::words::of_said(&e.to_string())),
            Some(Ok(l)) => {
                let mut t = if l.is_empty() {
                    olo_kit::words::tr("No backup has finished yet.").to_owned()
                } else {
                    let taken = st.size.map(|b| format!(", {}", olo_kit::tr!("{} in all", size(b)))).unwrap_or_default();
                    olo_kit::tr!("{} kept in {}{}.", counted(l.len() as u64, "1 backup", "{n} backups"), s.target.describe(), taken)
                };
                if unfinished.is_some() {
                    t.push_str(" The unfinished one is where the next goes on from: delete it to start over.");
                }
                t.push_str(" Choose any to validate or delete them.");
                t
            }
        };
        let running = busy_with(st);
        let list = ColumnListView::new(
            cx,
            "backups",
            &self.list,
            vec![Column::new("When", 160.0), Column::new("Files", 70.0), Column::new("Size", 75.0), Column::new("New", 75.0), Column::new("Took", 90.0), Column::new("Validated", 100.0)],
            snaps.len() + first,
            |w: &MainWindow, cell, g| {
                let (u, rows) = w.rows();
                let first = usize::from(u.is_some());
                let text = match (u, cell.row.checked_sub(first)) {
                    (Some(u), None) => match cell.column {
                        0 => format!("Unfinished ({})", clock(u.time)),
                        1 if u.files == 0 => "—".into(),
                        1 => count(u.files),
                        2 if u.bytes == 0 => "—".into(),
                        2 => size(u.bytes),
                        3 => size(u.new_bytes),
                        4 if u.seconds == 0.0 => "—".into(),
                        4 => took(u.seconds),
                        _ => "—".into(),
                    },
                    (_, Some(i)) => {
                        let Some(b) = rows.get(i) else { return };
                        match cell.column {
                            // (How long ago, while that's the shorter.)
                            0 if now() - b.time < 12 * 3600 => format!("{} ({})", clock(b.time), ago(b.time)),
                            0 => clock(b.time),
                            1 => count(b.files),
                            2 => size(b.bytes),
                            3 => size(b.new_bytes),
                            4 => took(b.seconds),
                            _ => check_cell(b.check.as_ref()).into(),
                        }
                    }
                    _ => return,
                };
                cell.text(g, text);
            },
        )
        .on_select(cx.listener(|_: &mut MainWindow, _: &(), cx| cx.invalidate()));
        let selected: Vec<usize> = self.list.state().selected.iter().copied().collect();
        let unfinished_chosen = first == 1 && selected.contains(&0);
        let chosen: Vec<String> = selected.iter().filter_map(|&i| i.checked_sub(first).and_then(|i| snaps.get(i)).map(|b| b.id.clone())).collect();
        let n = chosen.len() + usize::from(unfinished_chosen);
        // What the one chosen is, in a line (all of it in its tip).
        let (detail, bad) = match (selected.as_slice(), unfinished) {
            ([0], Some(u)) => (unfinished_words(u), false),
            ([i], _) => match i.checked_sub(first).and_then(|i| snaps.get(i)) {
                Some(b) => (check_words(b.check.as_ref()), b.check.as_ref().is_some_and(|c| !c.problems.is_empty())),
                None => (String::new(), false),
            },
            ([], _) => (olo_kit::words::tr("Choose a backup to see how it was last checked.").into(), false),
            _ => (counted(n as u64, "1 backup chosen.", "{n} backups chosen."), false),
        };
        let mut detail = StringView::new(detail).cut(true).height(style::line_row(style::PLAIN_SIZE));
        detail = if bad { detail.color(style::c(p.failure)) } else { detail.dim(true) };
        let (to_delete, to_check, to_check_fully) = (chosen.clone(), chosen.clone(), chosen.clone());
        let delete = Button::new(if n > 1 { olo_kit::tr!("Delete {} Backups…", n) } else { "Delete…".into() })
            .enabled(n > 0 && !running)
            .on_click(cx.listener(move |w: &mut MainWindow, _: &ClickEvent, cx| w.delete(to_delete.clone(), unfinished_chosen, cx)));
        let can_check = !chosen.is_empty() && !running;
        let validate = tip::on(
            div().child(
                Button::new(if chosen.len() > 1 { format!("Validate {}", chosen.len()) } else { "Validate".into() })
                    .enabled(can_check)
                    .on_click(cx.listener(move |w: &mut MainWindow, _: &ClickEvent, cx| w.validate(to_check.clone(), false, cx))),
            ),
            "backups-validate",
            "Check the backups chosen are whole, quickly: everything they need there, the store’s checksums compared with what was sent, and a few samples read back (as after each backup). What’s damaged is repaired as far as it can be.",
        );
        let validate_fully = tip::on(
            div().child(Button::new("Validate Fully…").enabled(can_check).on_click(cx.listener(move |w: &mut MainWindow, _: &ClickEvent, cx| w.validate(to_check_fully.clone(), true, cx)))),
            "backups-validate-fully",
            "Read every byte the backups chosen need back from where they’re kept, and check each piece, as a restore would",
        );
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(Button::new("Restore…").id("backups-restore").on_click(self.cmd(cx, "restore")))
            .child(validate)
            .child(validate_fully)
            .child(delete)
            .child(div().flex_1())
            .child(Button::new("Erase All Backups…").enabled(!running && !(snaps.is_empty() && unfinished.is_none())).on_click(self.cmd(cx, "erase-all")));
        let kept = BBox::new("Backups Kept").child(StringView::new(said).wrap(true).dim(true)).child(div().h(px(180.0)).child(list)).child(detail).child(row);
        vec![kept.into_element()]
    }

    fn speed(&self, s: &Settings, cx: &mut Cx<Self>) -> Vec<Element> {
        let speed_menu = |id: &'static str, label: &'static str, now: u64, set: fn(&mut Settings, u64)| {
            let at = SPEEDS.iter().position(|k| *k == now);
            let mut names: Vec<SharedString> = SPEEDS.iter().map(|k| SharedString::from(speed_label(*k))).collect();
            if at.is_none() {
                names.push(speed_label(now).into());
            }
            MenuField::new(id, names, at.unwrap_or(SPEEDS.len())).label(label).divider(DIVIDER).on_change(self.sets(cx, move |s, i: usize| {
                if let Some(k) = SPEEDS.get(i) {
                    set(s, *k);
                }
            }))
        };
        let network = BBox::new("Network")
            .child(speed_menu("upload", "Upload at most:", s.upload_kbs, |s, k| s.upload_kbs = k))
            .child(speed_menu("download", "Download at most:", s.download_kbs, |s, k| s.download_kbs = k))
            .child(StringView::new("Uploading is backing up; downloading is restoring, and tidying. A limit holds at once, for a backup under way too.").wrap(true).dim(true));
        let names: Vec<SharedString> = Priority::ALL.iter().map(|p| SharedString::from(p.label())).collect();
        let at = Priority::ALL.iter().position(|p| *p == s.priority).unwrap_or(0);
        let about = match s.priority {
            Priority::Idle => "It takes only what nothing else wants, the processor’s time and the disk’s: never in your way, though slower while you work.",
            Priority::Low => "Below anything of yours, but steadily: you won’t notice it, and it finishes sooner than only when idle.",
            Priority::Normal => "As much as any program: done soonest, and you may notice it while it runs.",
        };
        let computer = BBox::new("This Computer")
            .child(MenuField::new("priority", names, at).label("Back up:").divider(DIVIDER).on_change(self.sets(cx, |s, i: usize| s.priority = Priority::ALL[i])))
            .child(StringView::new(about).wrap(true).dim(true))
            .child(StringView::new("A backup reads one file at a time, and leaves what it read out of memory: it never takes much of either.").wrap(true).dim(true));
        vec![network.into_element(), computer.into_element()]
    }

    fn destination(&self, s: &Settings, st: &State, cx: &mut Cx<Self>) -> Vec<Element> {
        let p = theme::palette();
        let busy = self.busy;
        // As last looked at (None: looking).
        let looked = self.where_.as_ref().filter(|(t, _)| *t == s.target).and_then(|(_, r)| r.clone());
        let taken = st.size.map(|b| olo_kit::tr!("The backups take {} there", size(b)));
        let space = |free: u64, total: Option<u64>| match total {
            Some(t) if t > 0 => olo_kit::tr!("{} of {} free", size(free), size(t)),
            _ => olo_kit::words::tr("no limit to its space").to_owned(),
        };
        let and = |first: Option<String>, then: String| match first {
            Some(f) => format!("{f}; {then}."),
            None => format!("{}.", capital(&then)),
        };
        let (icon, title, detail, status, buttons): (&'static str, String, String, (String, bool), Vec<Element>) = match &s.target {
            Target::GoogleDrive { account, .. } => {
                let status = match &looked {
                    None => (olo_kit::words::tr("Checking the connection…").to_owned(), false),
                    Some(Ok(Where::Drive(a))) => (format!("Connected. {}", and(taken.clone(), format!("your Drive has {}", space(a.limit.map_or(0, |l| l.saturating_sub(a.used)), a.limit)))), false),
                    Some(Ok(_)) => (String::new(), false),
                    Some(Err(e)) => (olo_kit::tr!("Not connected: {}.", olo_kit::words::of_said(&e.to_string())), true),
                };
                let mut buttons = Vec::new();
                // (Signed out by Google: sign in again, with the client from before.)
                if matches!(looked, Some(Err(_))) {
                    buttons.push(Button::new("Sign In Again…").enabled(!busy).on_click(self.cmd(cx, "google-drive")).into_element());
                }
                buttons.push(Button::new("Disconnect…").enabled(!busy).on_click(self.cmd(cx, "disconnect")).into_element());
                buttons.push(tip::on(
                    div().child(Button::new("Use a Folder Instead…").enabled(!busy).on_click(self.cmd(cx, "choose-folder"))),
                    "use-folder",
                    "Back up to a disk or folder from now on; the backups in Google Drive stay as they are",
                ));
                ("diskbackup", olo_kit::words::tr("Google Drive").to_owned(), olo_kit::tr!("{}, in the folder “{}”", account, olo_diskbackup::drive::FOLDER), status, buttons)
            }
            Target::Folder { path } => {
                let status = match &looked {
                    None => (olo_kit::words::tr("Looking at its disk…").to_owned(), false),
                    Some(Ok(Where::Disk { free, total })) => (and(taken.clone(), olo_kit::tr!("its disk has {}", space(*free, Some(*total)))), false),
                    Some(Ok(_)) => (String::new(), false),
                    Some(Err(e)) => (format!("{}.", capital(e)), true),
                };
                let name = std::path::Path::new(path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
                let buttons = vec![
                    Button::new("Choose Another Folder…").enabled(!busy).on_click(self.cmd(cx, "choose-folder")).into_element(),
                    tip::on(
                        div().child(Button::new("Use Google Drive Instead…").enabled(!busy).on_click(self.cmd(cx, "google-drive"))),
                        "use-drive",
                        "Back up to your Google Drive from now on; the backups in this folder stay as they are",
                    ),
                ];
                ("disk", name, tilde(std::path::Path::new(path)), status, buttons)
            }
            Target::None => ("disk", olo_kit::words::tr("Not set up").into(), String::new(), (String::new(), false), Vec::new()),
        };
        let open_label = if matches!(s.target, Target::GoogleDrive { .. }) { "Open in Google Drive" } else { "Show in Tracker" };
        let mut status_view = StringView::new(status.0).cut(true).height(style::line_row(style::PLAIN_SIZE));
        status_view = if status.1 { status_view.color(style::c(p.failure)) } else { status_view.dim(true) };
        let where_ = BBox::new("Where Backups Go")
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(picture(icon, 32.0))
                    // (Three lines as tall as each other: the picture centred
                    // on the middle one.)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(StringView::new(title).bold(true).height(style::line_row(style::PLAIN_SIZE)))
                            .child(StringView::new(detail).cut(true).height(style::line_row(style::PLAIN_SIZE)))
                            .child(status_view),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .pt(px(m::TIGHT))
                    .children(buttons)
                    .child(div().flex_1())
                    .child(Button::new(open_label).id("show-target").on_click(self.cmd(cx, "show-target"))),
            );
        let mut key = BBox::new("Key");
        if s.key_in_passwords {
            key = key.child(
                StringView::new("Your backups are encrypted before they leave this computer. Their key is kept in Passwords (“DiskBackup Key”): with it, they can be restored on any computer.")
                    .wrap(true)
                    .dim(true),
            );
        } else {
            key = key
                .child(
                    StringView::new("Your backups are encrypted before they leave this computer, with a key only this computer has. Keep it in Passwords too, or they can’t be read anywhere else.")
                        .wrap(true),
                )
                .child(div().pt(px(m::TIGHT)).child(Button::new("Keep Key in Passwords").id("key-keep").enabled(!busy).on_click(self.cmd(cx, "keep-key"))));
        }
        let deskbar = BBox::new("Deskbar").child(CheckBox::new("Show DiskBackup in Deskbar", s.in_deskbar).on_toggle(self.sets(cx, |s, on: bool| s.in_deskbar = on)));
        vec![where_.into_element(), key.into_element(), deskbar.into_element()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_protocol::diskbackup::{Keep, Last};

    /// The welcome's words start where the boxes' do (a box's inset, its
    /// picture and a spacing), and its heading keeps to one line: the
    /// title's size where it fits, smaller where it doesn't.
    #[test]
    fn the_welcome_lines_up_with_its_boxes() {
        assert_eq!(ICON + HERO_GAP, m::BOX_INSET + 32.0 + m::SPACING);
        assert_eq!(welcome_heading_size(250.0, 400.0), style::TITLE_SIZE);
        assert_eq!(welcome_heading_size(270.0, 259.0), HEADING_NARROW);
    }

    #[test]
    fn words_for_times() {
        let t = now();
        assert_eq!(ago(t - 20), "just now");
        assert_eq!(ago(t - 60), "a minute ago");
        assert_eq!(ago(t - 12 * 60), "12 minutes ago");
        assert_eq!(ago(t - 3600), "an hour ago");
        assert_eq!(until(t + 30), "in a minute");
        assert_eq!(until(t + 48 * 60), "in 48 minutes");
        assert_eq!(took(0.3), "under a second");
        assert_eq!(took(12.0), "12 seconds");
        assert_eq!(took(240.0), "4 minutes");
        assert_eq!(took(3900.0), "1 hour 5 minutes");
    }

    #[test]
    fn what_is_kept_said_plainly() {
        assert_eq!(keep_words(&Keep::default()), "Every backup from the last hour, then one an hour for 24 hours, one a day for 14 days, one a week for 8 weeks and one a month for 24 months.");
        assert_eq!(keep_words(&Keep { hourly: 0, daily: 7, weekly: 0, monthly: u32::MAX }), "Every backup from the last hour, then one a day for 7 days and one a month for ever.");
        assert_eq!(keep_words(&Keep { hourly: 0, daily: 0, weekly: 0, monthly: 0 }), "Every backup from the last hour; older ones go.");
    }

    #[test]
    fn how_far_and_how_long() {
        let t = now();
        let p = Progress { started: t - 100, bytes: 25, total_bytes: 100, ..Default::default() };
        let (done, left) = p.how_far(t);
        assert!((done - 0.25).abs() < 1e-6);
        assert_eq!(left, Some(300), "a quarter in 100 s: 300 s to go");
        // Past the plan (it grew): never done while it runs.
        let p = Progress { started: t - 100, bytes: 150, total_bytes: 100, ..Default::default() };
        assert!(p.how_far(t).0 <= 0.99);
        // Too soon to say.
        assert_eq!(Progress { started: t - 3, bytes: 1, total_bytes: 100, ..Default::default() }.how_far(t).1, None);
    }

    #[test]
    fn the_headline_says_how_it_stands() {
        let s = Settings { target: Target::Folder { path: "/backups".into() }, ..Settings::default() };
        let mut st = State { pid: 1, ..State::default() };
        assert_eq!(headline(&st, &s).0, "Not backed up yet");
        st.last = Some(Last { time: now() - 12 * 60, ..Default::default() });
        st.next = Some(now() + 48 * 60);
        let (title, detail, bad) = headline(&st, &s);
        assert_eq!(title, "Backed up 12 minutes ago");
        assert!(detail.contains("To /backups") && detail.contains("Next in 48 minutes"), "{detail}");
        assert!(!bad);
        let manual = Settings { automatic: false, ..s.clone() };
        assert!(headline(&st, &manual).1.contains("Next when you click Back Up Now"));
        st.last = Some(Last { time: now() - 60, error: Some("the disk is full".into()), ..Default::default() });
        let (title, detail, bad) = headline(&st, &s);
        assert!(bad && title.contains("didn’t finish") && detail.contains("the disk is full"), "{title} {detail}");
        // Stopped: calmly, unless nothing whole is recent.
        st.last = Some(Last { time: now() - 60, error: Some(olo_protocol::diskbackup::STOPPED.into()), ..Default::default() });
        st.last_ok = Some(now() - 3600);
        let (title, detail, bad) = headline(&st, &s);
        assert!(!bad && title == "Backup stopped" && detail.starts_with("You stopped it a minute ago. The last whole one was an hour ago."), "{title} {detail}");
        st.last_ok = Some(now() - 5 * 86_400);
        assert!(headline(&st, &s).2);
        // Not backing up, and why: paused, waiting for power or the
        // network; with when the last one was.
        st.doing = Doing::Waiting { why: "turned off".into() };
        let (title, detail, bad) = headline(&st, &s);
        assert!(!bad && title == "Paused" && detail.starts_with("Nothing is backed up or sent until you resume. The last backup was ") && detail.ends_with(". To /backups."), "{title} {detail}");
        st.doing = Doing::Waiting { why: "on battery: it waits for power".into() };
        assert_eq!(headline(&st, &s).0, "Waiting for power");
        st.doing = Doing::Waiting { why: "no network: it goes on once there’s one".into() };
        assert_eq!(headline(&st, &s).0, "Waiting for the network");
        st.doing = Doing::Idle;
        // Under way: the file it's at, the home folder as ~.
        st.last_ok = None;
        st.doing = Doing::BackingUp;
        st.progress = Some(Progress { stage: Stage::Reading, current: format!("{}/Music/a.flac", target::home().display()), ..Default::default() });
        assert_eq!(headline(&st, &s), ("Backing up your home folder".into(), "Now: ~/Music/a.flac".into(), false));
        st.progress = Some(Progress { stage: Stage::Reading, resuming: true, ..Default::default() });
        assert_eq!(headline(&st, &s).0, "Resuming the backup");
        st.doing = Doing::Validating;
        assert_eq!(headline(&st, &s).0, "Validating backups");
        st.doing = Doing::Deleting { what: "the unfinished backup".into() };
        assert_eq!(headline(&st, &s), ("Deleting the unfinished backup".into(), "Then the space only it took is freed, in /backups.".into(), false));
        st.doing = Doing::Deleting { what: "2 backups and the unfinished one".into() };
        assert_eq!(headline(&st, &s).0, "Deleting backups");
        st.pid = 0;
        assert!(headline(&st, &s).2);
    }

    /// An unfinished backup, and each backup's check, said plainly.
    #[test]
    fn unfinished_and_checks_said() {
        use olo_protocol::diskbackup::Unfinished;
        let u = Unfinished { time: now(), files: 1234, bytes: 2_100_000_000, sent: 1_200_000_000, ..Default::default() };
        assert_eq!(unfinished_words(&u), "Unfinished: 1,234 files (2.10 GB) done and 1.20 GB sent; the next backup goes on from there.");
        let left = Unfinished { files: 0, sent: 5_000_000, ..u };
        assert!(unfinished_words(&left).starts_with("Unfinished: 5.0 MB sent before it stopped"));
        assert_eq!(check_cell(None), "—");
        let whole = Check { time: now(), full: true, bytes_read: 3_000_000, ..Default::default() };
        assert_eq!(check_cell(Some(&whole)), "✓ Full");
        assert!(check_words(Some(&whole)).starts_with("Validated fully just now: all it needs (3.0 MB) read back"));
        let fast = Check { full: false, by_checksum: 12, sampled: 1, ..whole.clone() };
        assert_eq!(check_cell(Some(&fast)), "✓ Fast");
        assert_eq!(check_words(Some(&fast)), "Validated just now: everything it needs is there (12 packs as sent, by their checksums; 1 sample read back). Validate Fully reads it all back.");
        let home = target::home().display().to_string();
        let bad = Check { problems: vec!["2 of its packs are missing".into()], files: vec![format!("{home}/a.bin")], files_affected: 3, ..whole.clone() };
        assert_eq!(check_cell(Some(&bad)), "✗ Damaged");
        let said = check_words(Some(&bad));
        assert!(said.contains("2 of its packs are missing") && said.contains("~/a.bin and 2 more") && said.ends_with("sends again what it can."), "{said}");
        let failed = Check { error: Some("Google Drive didn't answer".into()), ..whole };
        assert_eq!(check_cell(Some(&failed)), "Not checked");
    }
}
