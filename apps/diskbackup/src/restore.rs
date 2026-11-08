//! Restore (File ▸ Restore…, Browse Backups…): the snapshots, newest
//! first; a snapshot's folders browsed, as they were then; what's chosen
//! put back where it was (Replace or Keep Both, asked if something's there
//! now) or into a folder chosen. Opening the backup, reading a folder and
//! putting back each run on a worker, the last with its progress and Stop.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
};

use olo_diskbackup::{Id, Kind, Node, Progress, Repo, Snapshot, restore::find};
use olo_protocol::diskbackup::Settings;
use olo_ui::{
    Proxy,
    file_panel::{self, FilePanel, PanelResult},
    gpui::{self, App, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    metrics as m, style, theme,
    widgets::{Button, Column, ColumnListView, ListModel, StringView, alert},
};

use crate::{app::Msg, target};

/// The backup open, for the window and its workers.
type Shared = Arc<RwLock<Repo>>;

/// What a worker tells the Restore window.
pub enum Done {
    Opened(Result<(Shared, Vec<(Id, Snapshot)>), String>),
    /// A folder's entries (for the listing numbered so).
    Listed(u64, Result<Vec<Node>, String>),
    Progress(u64, u64, String),
    Restored(Result<String, String>),
}

pub struct RestoreWindow {
    proxy: Proxy,
    /// (Its index read again if the backup was tidied since: `retrying`.)
    repo: Option<Arc<RwLock<Repo>>>,
    /// Newest first.
    snapshots: Vec<(Id, Snapshot)>,
    snap: Option<usize>,
    /// The folder shown, as it was backed up ("/home/ben").
    dir: PathBuf,
    nodes: Vec<Node>,
    listing: u64,
    snaps: Entity<ListModel>,
    files: Entity<ListModel>,
    /// What to say (and whether it's an error).
    status: (String, bool),
    /// Putting back: its Stop.
    restoring: Option<Arc<AtomicBool>>,
}

/// "Today 14:05", "Tue 14:05", "7 Nov 2026 14:05".
pub(crate) fn when(t: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_opt(t, 0).single() else { return String::new() };
    let today = Local::now().date_naive();
    let days = (today - at.date_naive()).num_days();
    match days {
        0 => format!("Today {}", at.format("%H:%M")),
        1 => format!("Yesterday {}", at.format("%H:%M")),
        2..=6 => at.format("%a %H:%M").to_string(),
        _ => at.format("%-d %b %Y %H:%M").to_string(),
    }
}

/// `when`, in a sentence: "today at 14:05", "yesterday at 14:05", "on
/// Tue at 14:05", "on 7 Nov 2026 at 14:05".
pub(crate) fn when_said(t: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_opt(t, 0).single() else { return String::new() };
    let time = at.format("%H:%M");
    match (Local::now().date_naive() - at.date_naive()).num_days() {
        0 => format!("today at {time}"),
        1 => format!("yesterday at {time}"),
        2..=6 => format!("on {} at {time}", at.format("%a")),
        _ => format!("on {} at {time}", at.format("%-d %b %Y")),
    }
}

pub(crate) fn size(b: u64) -> String {
    match b {
        0..1000 => format!("{b} bytes"),
        1000..1_000_000 => format!("{} KB", b.div_ceil(1000)),
        1_000_000..1_000_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        _ => format!("{:.2} GB", b as f64 / 1e9),
    }
}

fn name_of(n: &Node) -> String {
    n.name_str().into_owned()
}

/// `name` as "name (from 7 Nov 2026 14.05).ext", beside what's there now
/// (a date, not "Today": it stays true).
pub(crate) fn beside(name: &str, time: i64) -> String {
    use chrono::{Local, TimeZone};
    let stamp = Local.timestamp_opt(time, 0).single().map(|t| t.format("%-d %b %Y %H.%M").to_string()).unwrap_or_default();
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} (from {stamp}).{ext}"),
        _ => format!("{name} (from {stamp})"),
    }
}

impl RestoreWindow {
    pub fn new(proxy: Proxy, cx: &mut Context<Self>) -> RestoreWindow {
        let w = RestoreWindow {
            proxy: proxy.clone(),
            repo: None,
            snapshots: Vec::new(),
            snap: None,
            dir: target::home(),
            nodes: Vec::new(),
            listing: 0,
            snaps: ListModel::new(cx, false),
            files: ListModel::new(cx, true),
            status: ("Opening the backup…".into(), false),
            restoring: None,
        };
        // The backup opened, its index read and its snapshots listed.
        std::thread::spawn(move || {
            let r = (|| -> Result<_, String> {
                let s = Settings::load();
                let limits = target::Limits::new();
                limits.follow(&s);
                let store = target::store(&s.target, &limits)?;
                let repo = Repo::open(store, target::key()?, Some(target::cache())).map_err(|e| e.to_string())?;
                let mut snaps = repo.snapshots().map_err(|e| e.to_string())?;
                snaps.reverse();
                Ok((Arc::new(RwLock::new(repo)), snaps))
            })();
            proxy.send(Msg::Restore(Done::Opened(r)));
        });
        w
    }

    pub fn message(&mut self, d: Done, cx: &mut Context<Self>) {
        match d {
            Done::Opened(Ok((repo, snaps))) => {
                self.repo = Some(repo);
                self.snapshots = snaps;
                self.status = (if self.snapshots.is_empty() { "Nothing backed up yet.".into() } else { String::new() }, false);
                if !self.snapshots.is_empty() {
                    self.choose_snapshot(0, cx);
                }
            }
            Done::Opened(Err(e)) => self.status = (format!("The backup can’t be opened: {e}."), true),
            Done::Listed(n, r) if n == self.listing => match r {
                Ok(nodes) => {
                    self.nodes = nodes;
                    self.files.update(cx, |m, _| m.state.selected.clear());
                    if self.restoring.is_none() {
                        self.status = (String::new(), false);
                    }
                }
                Err(e) => self.status = (e, true),
            },
            Done::Listed(..) => {}
            Done::Progress(files, bytes, current) => {
                if self.restoring.is_some() {
                    let shown = current.replace(&target::home().display().to_string(), "~");
                    self.status = (format!("Restoring: {files} {}, {} so far. {shown}", if files == 1 { "file" } else { "files" }, size(bytes)), false);
                }
            }
            Done::Restored(r) => {
                self.restoring = None;
                self.status = match r {
                    Ok(s) => (s, false),
                    Err(e) => (e, true),
                };
            }
        }
        cx.notify();
    }

    fn choose_snapshot(&mut self, i: usize, cx: &mut Context<Self>) {
        self.snap = Some(i);
        self.snaps.update(cx, |m, _| {
            m.state.selected = [i].into();
            m.state.cursor = Some(i);
        });
        // Where the home folder was, if it was backed up; else the top.
        let dir = self.dir.clone();
        self.show(dir, cx);
    }

    /// Show folder `dir` of the chosen snapshot (its entries read on a
    /// worker; the top if it isn't there).
    fn show(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let (Some(repo), Some((_, snap))) = (self.repo.clone(), self.snap.and_then(|i| self.snapshots.get(i)).cloned()) else { return };
        self.listing += 1;
        let n = self.listing;
        self.dir = dir.clone();
        self.status = ("Reading…".into(), false);
        cx.notify();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let r = target::retrying(&repo, |repo| {
                let mut at = dir.clone();
                let node = loop {
                    match find(repo, &snap.tree, &at)? {
                        Some(n) if n.kind == Kind::Dir => break n,
                        _ if at.parent().is_some() => at = at.parent().map(Path::to_path_buf).unwrap_or_default(),
                        _ => return Ok(Vec::new()),
                    }
                };
                let mut nodes = match node.subtree {
                    Some(id) => repo.tree(&id)?.nodes,
                    None => Vec::new(),
                };
                nodes.sort_by(|a, b| (a.kind != Kind::Dir).cmp(&(b.kind != Kind::Dir)).then_with(|| name_of(a).to_lowercase().cmp(&name_of(b).to_lowercase())));
                Ok(nodes)
            });
            proxy.send(Msg::Restore(Done::Listed(n, r)));
        });
    }

    fn open_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(n) = self.nodes.get(row).filter(|n| n.kind == Kind::Dir) {
            let dir = self.dir.join(name_of(n));
            self.show(dir, cx);
        }
    }

    fn up(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            self.show(parent, cx);
        }
    }

    /// The entries chosen (all of them if none is).
    fn chosen(&self, cx: &App) -> Vec<Node> {
        let sel = &self.files.read(cx).state.selected;
        if sel.is_empty() { Vec::new() } else { sel.iter().filter_map(|&i| self.nodes.get(i).cloned()).collect() }
    }

    /// Put the chosen back where they were: asked first if anything is
    /// there now (Replace, or Keep Both: beside it, named with its time).
    fn restore_here(&mut self, cx: &mut Context<Self>) {
        let chosen = self.chosen(cx);
        let Some((_, snap)) = self.snap.and_then(|i| self.snapshots.get(i)).cloned() else { return };
        if chosen.is_empty() {
            return;
        }
        let there: Vec<&Node> = chosen.iter().filter(|n| self.dir.join(name_of(n)).symlink_metadata().is_ok()).collect();
        let pairs = |both: bool| -> Vec<(PathBuf, PathBuf)> {
            chosen
                .iter()
                .map(|n| {
                    let from = self.dir.join(name_of(n));
                    let to = if both && from.symlink_metadata().is_ok() { self.dir.join(beside(&name_of(n), snap.time)) } else { from.clone() };
                    (from, to)
                })
                .collect()
        };
        if there.is_empty() {
            return self.run(pairs(false), cx);
        }
        let (replace, both) = (pairs(false), pairs(true));
        let what = match there.as_slice() {
            [one] => format!("“{}” is there now.", name_of(one)),
            many => format!("{} of these are there now.", many.len()),
        };
        let message = format!("{what} Replace with the version backed up {}, or keep both (the restored one named with its time)?", when_said(snap.time));
        let me = cx.entity();
        alert::open_alert(cx, "Restore", "warning", &message, &["Cancel", "Keep Both", "Replace"], move |choice, cx| match choice {
            Some(1) => me.update(cx, |w, cx| w.run(both.clone(), cx)),
            Some(2) => me.update(cx, |w, cx| w.run(replace.clone(), cx)),
            _ => {}
        });
    }

    /// Put the chosen into a folder chosen.
    fn restore_to(&mut self, cx: &mut Context<Self>) {
        let chosen = self.chosen(cx);
        if chosen.is_empty() {
            return;
        }
        let dir = self.dir.clone();
        let me = cx.entity();
        cx.defer(move |cx| {
            file_panel::show(cx, "Restore To", FilePanel::folder(target::home()), move |a, cx| {
                if let PanelResult::Chosen(to) = a.result {
                    let pairs = chosen.iter().map(|n| (dir.join(name_of(n)), to.join(name_of(n)))).collect();
                    me.update(cx, |w, cx| w.run(pairs, cx));
                }
            });
        });
    }

    /// Put each (where it was in the snapshot, where it goes) back, on a
    /// worker; its progress said every quarter second.
    fn run(&mut self, pairs: Vec<(PathBuf, PathBuf)>, cx: &mut Context<Self>) {
        let (Some(repo), Some((_, snap))) = (self.repo.clone(), self.snap.and_then(|i| self.snapshots.get(i)).cloned()) else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        self.restoring = Some(cancel.clone());
        self.status = ("Restoring…".into(), false);
        cx.notify();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let progress = Progress::default();
            let done = AtomicBool::new(false);
            let r = std::thread::scope(|s| {
                s.spawn(|| {
                    while !done.load(Relaxed) {
                        std::thread::park_timeout(std::time::Duration::from_millis(250));
                        let current = progress.current.lock().map(|c| c.clone()).unwrap_or_default();
                        proxy.send(Msg::Restore(Done::Progress(progress.files.load(Relaxed), progress.bytes.load(Relaxed), current)));
                    }
                });
                // Held while it restores: nobody tidies meanwhile (one
                // under way is waited for), and the index as it is now.
                let r = (|| {
                    let lock = {
                        let repo = repo.read().map_err(|_| olo_diskbackup::Error::Cancelled)?;
                        target::lock_to_read(&repo, &cancel, |why| {
                            if let Ok(mut c) = progress.current.lock() {
                                *c = format!("Waiting: {why}.");
                            }
                        })?
                    };
                    repo.write().map_err(|_| olo_diskbackup::Error::Cancelled)?.refresh()?;
                    let repo = repo.read().map_err(|_| olo_diskbackup::Error::Cancelled)?;
                    let r = pairs.iter().try_for_each(|(from, to)| olo_diskbackup::restore(&repo, &snap.tree, from, to, &cancel, &progress));
                    drop(lock);
                    r
                })();
                done.store(true, Relaxed);
                r
            });
            let out = match r {
                Ok(()) => {
                    let place = match pairs.as_slice() {
                        [(_, to)] => format!("as {}", to.display()),
                        _ => format!("into {}", pairs.first().and_then(|(_, t)| t.parent()).map(|p| p.display().to_string()).unwrap_or_default()),
                    };
                    let n = progress.files.load(Relaxed);
                    let files = if n == 1 { "1 file".to_owned() } else { format!("{n} files") };
                    Ok(format!("Restored {files} ({}) from {}, {place}.", size(progress.bytes.load(Relaxed)), when(snap.time)).replace(&target::home().display().to_string(), "~"))
                }
                Err(olo_diskbackup::Error::Cancelled) => Err("Stopped: what was restored so far is there.".into()),
                Err(e) => Err(format!("Couldn’t restore: {e}.")),
            };
            proxy.send(Msg::Restore(Done::Restored(out)));
        });
    }
}

impl Render for RestoreWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let me = cx.entity();
        // ---- the snapshots.
        let rows: Vec<(String, String)> = self.snapshots.iter().map(|(_, s)| (when(s.time), format!("{} files, {} new", s.summary.files, size(s.summary.new_bytes)))).collect();
        let pick = me.clone();
        let snaps_model = self.snaps.clone();
        let list_rows = rows.clone();
        let snaps = ColumnListView::new("snapshots", &self.snaps, vec![Column::new("Backed Up", 130.0), Column::new("What", 150.0)], rows.len(), move |cell, window, cx| {
            let Some((t, what)) = list_rows.get(cell.row) else { return };
            match cell.column {
                0 => cell.text(window, cx, t.clone()),
                _ => cell.text(window, cx, what.clone()),
            }
        })
        .on_select(move |_, cx| {
            let i = snaps_model.read(cx).state.cursor;
            if let Some(i) = i {
                pick.update(cx, |w, cx| {
                    if w.snap != Some(i) {
                        w.choose_snapshot(i, cx);
                    }
                });
            }
        });
        // ---- the snapshot's folder.
        let nodes = self.nodes.clone();
        let open = me.clone();
        let files =
            ColumnListView::new("files", &self.files, vec![Column::new("Name", 260.0), Column::new("Size", 90.0).right(), Column::new("Modified", 150.0)], nodes.len(), move |cell, window, cx| {
                let Some(n) = nodes.get(cell.row) else { return };
                match cell.column {
                    0 => cell.icon_text(window, cx, if n.kind == Kind::Dir { "folder" } else { "document" }, "document", name_of(n)),
                    1 => cell.text(window, cx, if n.kind == Kind::File { size(n.size) } else { String::new() }),
                    _ => cell.text(window, cx, when(n.mtime / 1_000_000_000)),
                }
            })
            .on_open(move |row, _, cx| open.update(cx, |w, cx| w.open_row(row, cx)));
        let path = self.dir.display().to_string().replace(&target::home().display().to_string(), "~");
        let up = me.clone();
        let bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(Button::new("Up").enabled(self.dir.parent().is_some() && self.repo.is_some()).on_click(move |_, cx| up.update(cx, |w, cx| w.up(cx))))
            .child(div().flex_1().min_w_0().child(StringView::new(path)));
        let any = !self.files.read(cx).state.selected.is_empty() && self.restoring.is_none();
        let (here, to, stop) = (me.clone(), me.clone(), self.restoring.clone());
        // What's going on, a line of its own (it can be long), then the
        // buttons at the right.
        let (text, error) = self.status.clone();
        let status = StringView::new(if text.is_empty() { " ".into() } else { text }).wrap(true).color(if error { style::c(p.failure) } else { style::disabled_text(&p) });
        let mut buttons = div().flex().flex_row().items_center().justify_end().gap(px(m::SPACING));
        if let Some(stop) = stop {
            buttons = buttons.child(Button::new("Stop").on_click(move |_, _| stop.store(true, Relaxed)));
        }
        buttons = buttons
            .child(Button::new("Restore To…").enabled(any).on_click(move |_, cx| to.update(cx, |w, cx| w.restore_to(cx))))
            .child(Button::new("Restore").enabled(any).on_click(move |_, cx| here.update(cx, |w, cx| w.restore_here(cx))));
        // Both lists start level: the snapshots' title on the path's row.
        let top = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .h(px(m::CONTROL_HEIGHT))
            .child(div().w(px(300.0)).child(StringView::new("Snapshots").bold(true)))
            .child(div().flex_1().min_w_0().child(bar));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(top)
            .child(div().flex_1().min_h_0().flex().flex_row().gap(px(m::SPACING)).child(div().w(px(300.0)).h_full().child(snaps)).child(div().flex_1().min_w_0().h_full().child(files)))
            .child(status)
            .child(buttons)
    }
}

/// The window's options.
pub fn options() -> olo_ui::WindowOptions {
    olo_ui::WindowOptions::new("Restore", 920.0, 560.0).min_size(720.0, 380.0).keep_size("restore")
}

/// (`gpui` re-exported names this file uses.)
#[allow(unused_imports)]
use gpui::AppContext as _;
