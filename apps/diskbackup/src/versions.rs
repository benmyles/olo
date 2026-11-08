//! A file's versions (`olo-diskbackup --versions-window PATH`; Tracker's
//! Older Versions…): each content the backups kept of it, newest first
//! (when it was modified, its size, when it was backed up), the one there
//! now said; Restore puts the chosen one back, beside what's there (Keep
//! Both: named with its time) or in its place (Replace).

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
};

use olo_diskbackup::{Progress, Repo};
use olo_protocol::diskbackup::{Reply, Request, Settings, Version};
use olo_ui::{
    Proxy,
    gpui::{self, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    metrics as m, style, theme,
    widgets::{Button, Column, ColumnListView, ListModel, StringView, alert},
};

use crate::{app::Msg, target};

/// What a worker tells a Versions window.
pub enum Done {
    Listed(Result<Vec<Version>, String>),
    Progress(u64),
    Restored(Result<String, String>),
}

pub struct VersionsWindow {
    proxy: Proxy,
    pub path: PathBuf,
    versions: Vec<Version>,
    list: Entity<ListModel>,
    status: (String, bool),
    restoring: Option<Arc<AtomicBool>>,
}

/// The versions of `path`: the service's answer (its trees are at hand),
/// else from the backup opened here.
fn load(path: &std::path::Path) -> Result<Vec<Version>, String> {
    let at = path.display().to_string();
    match olo_protocol::diskbackup::call(&Request::Versions { path: at }) {
        Ok(Reply::Versions { versions }) => return Ok(versions),
        Ok(r) => return Err(format!("the service answered {r:?}")),
        Err(e) if !e.contains("isn't running") => return Err(e),
        Err(_) => {}
    }
    let (_, repo) = open()?;
    let host = crate::service::host();
    let mine: Vec<_> = repo.snapshots().map_err(|e| e.to_string())?.into_iter().filter(|(_, s)| s.host == host).collect();
    let vs = olo_diskbackup::versions(&repo, &mine, path).map_err(|e| e.to_string())?;
    Ok(vs
        .into_iter()
        .map(|v| Version { snapshot: v.snapshot.hex(), time: v.time, first: v.first, size: v.node.size, mtime: v.node.mtime, current: olo_diskbackup::backup::unchanged_since(&v.node, path) })
        .collect())
}

fn open() -> Result<(Settings, Repo), String> {
    let s = Settings::load();
    let limits = target::Limits::new();
    limits.follow(&s);
    let store = target::store(&s.target, &limits)?;
    let repo = Repo::open(store, target::key()?, Some(target::cache())).map_err(|e| e.to_string())?;
    Ok((s, repo))
}

impl VersionsWindow {
    pub fn new(proxy: Proxy, path: PathBuf, cx: &mut Context<Self>) -> VersionsWindow {
        let mut w = VersionsWindow { proxy, path, versions: Vec::new(), list: ListModel::new(cx, false), status: ("Looking…".into(), false), restoring: None };
        w.reload(cx);
        w
    }

    /// Its versions read again, on a worker.
    pub fn reload(&mut self, _: &mut Context<Self>) {
        let (at, proxy) = (self.path.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let r = load(&at);
            proxy.send(Msg::Versions(at, Done::Listed(r)));
        });
    }

    pub fn message(&mut self, d: Done, cx: &mut Context<Self>) {
        match d {
            Done::Listed(Ok(vs)) => {
                self.versions = vs;
                self.status = (
                    match self.versions.len() {
                        0 => "The backups have no version of it.".into(),
                        1 if self.versions[0].current => "The backups have only the version there now.".into(),
                        _ => String::new(),
                    },
                    false,
                );
                // The newest that isn't what's there now, chosen.
                if let Some(i) = self.versions.iter().position(|v| !v.current) {
                    self.list.update(cx, |m, _| {
                        m.state.selected = [i].into();
                        m.state.cursor = Some(i);
                    });
                }
            }
            Done::Listed(Err(e)) => self.status = (format!("Its versions can’t be read: {e}."), true),
            Done::Progress(bytes) => {
                if self.restoring.is_some() {
                    self.status = (format!("Restoring: {} so far…", crate::app::size(bytes)), false);
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

    fn chosen(&self, cx: &gpui::App) -> Option<Version> {
        self.list.read(cx).state.cursor.and_then(|i| self.versions.get(i)).cloned()
    }

    /// The chosen version back: asked Keep Both (beside, named with when it
    /// was modified) or Replace, if something's there now.
    fn restore(&mut self, cx: &mut Context<Self>) {
        let Some(v) = self.chosen(cx) else { return };
        if v.current || self.restoring.is_some() {
            return;
        }
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if self.path.symlink_metadata().is_err() {
            return self.run(v, self.path.clone(), cx);
        }
        let beside = self.path.with_file_name(crate::restore::beside(&name, v.mtime / 1_000_000_000));
        let message =
            format!("“{name}” is there now. Replace it with the version modified {}, or keep both (the restored one named with its time)?", crate::restore::when_said(v.mtime / 1_000_000_000));
        let (me, here) = (cx.entity(), self.path.clone());
        alert::open_alert(cx, "Restore", "warning", &message, &["Cancel", "Keep Both", "Replace"], move |choice, cx| match choice {
            Some(1) => me.update(cx, |w, cx| w.run(v.clone(), beside.clone(), cx)),
            Some(2) => me.update(cx, |w, cx| w.run(v.clone(), here.clone(), cx)),
            _ => {}
        });
    }

    /// `v` put back as `to`, on a worker (a shared lock held, the index read
    /// again first); its progress every quarter second.
    fn run(&mut self, v: Version, to: PathBuf, cx: &mut Context<Self>) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.restoring = Some(cancel.clone());
        self.status = ("Restoring…".into(), false);
        cx.notify();
        let (proxy, from) = (self.proxy.clone(), self.path.clone());
        std::thread::spawn(move || {
            let progress = Progress::default();
            let done = AtomicBool::new(false);
            let r = std::thread::scope(|s| {
                let reporter = s.spawn(|| {
                    while !done.load(Relaxed) {
                        std::thread::park_timeout(std::time::Duration::from_millis(250));
                        proxy.send(Msg::Versions(from.clone(), Done::Progress(progress.bytes.load(Relaxed))));
                    }
                });
                let r = (|| -> Result<(), String> {
                    let (_, mut repo) = open()?;
                    let _lock = target::lock_to_read(&repo, &cancel, |_| {}).map_err(|e| e.to_string())?;
                    repo.refresh().map_err(|e| e.to_string())?;
                    let snaps = repo.snapshots().map_err(|e| e.to_string())?;
                    let (_, snap) = snaps.iter().find(|(id, _)| id.hex() == v.snapshot).ok_or("that snapshot isn’t kept any more")?;
                    olo_diskbackup::restore(&repo, &snap.tree, &from, &to, &cancel, &progress).map_err(|e| e.to_string())
                })();
                done.store(true, Relaxed);
                reporter.thread().unpark();
                r
            });
            let shown = to.display().to_string().replace(&target::home().display().to_string(), "~");
            let out = match r {
                Ok(()) => Ok(format!("Restored as {shown}.")),
                Err(e) if e == olo_diskbackup::Error::Cancelled.to_string() => Err("Stopped.".into()),
                Err(e) => Err(format!("Couldn’t restore: {e}.")),
            };
            proxy.send(Msg::Versions(from, Done::Restored(out)));
        });
    }
}

impl Render for VersionsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let me = cx.entity();
        let rows = self.versions.clone();
        let when = crate::restore::when;
        let list = ColumnListView::new(
            "versions",
            &self.list,
            vec![Column::new("Modified", 210.0), Column::new("Size", 90.0).right(), Column::new("Backed Up", 230.0)],
            rows.len(),
            move |cell, window, cx| {
                let Some(v) = rows.get(cell.row) else { return };
                match cell.column {
                    0 => cell.text(window, cx, if v.current { format!("{} (there now)", when(v.mtime / 1_000_000_000)) } else { when(v.mtime / 1_000_000_000) }),
                    1 => cell.text(window, cx, crate::restore::size(v.size)),
                    _ => cell.text(window, cx, if v.first == v.time { when(v.time) } else { format!("{} to {}", when(v.first), when(v.time)) }),
                }
            },
        )
        .on_select({
            let me = me.clone();
            move |_, cx| me.update(cx, |_, cx| cx.notify())
        })
        .on_open({
            let me = me.clone();
            move |_, _, cx| me.update(cx, |w, cx| w.restore(cx))
        });
        let shown = self.path.display().to_string().replace(&target::home().display().to_string(), "~");
        let (text, error) = self.status.clone();
        let status = StringView::new(if text.is_empty() { " ".into() } else { text }).wrap(true).color(if error { style::c(p.failure) } else { style::disabled_text(&p) });
        let can = self.chosen(cx).is_some_and(|v| !v.current) && self.restoring.is_none();
        let mut buttons = div().flex().flex_row().items_center().justify_end().gap(px(m::SPACING));
        if let Some(stop) = self.restoring.clone() {
            buttons = buttons.child(Button::new("Stop").on_click(move |_, _| stop.store(true, Relaxed)));
        }
        let here = me.clone();
        buttons = buttons.child(Button::new("Restore").enabled(can).on_click(move |_, cx| here.update(cx, |w, cx| w.restore(cx))));
        div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(StringView::new(shown)).child(div().flex_1().min_h_0().child(list)).child(status).child(buttons)
    }
}

/// The window's options (titled with the file's name).
pub fn options(path: &std::path::Path) -> olo_ui::WindowOptions {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    olo_ui::WindowOptions::new(format!("Versions of {name}"), 600.0, 340.0).min_size(480.0, 240.0).keep_size("versions")
}

#[allow(unused_imports)]
use gpui::AppContext as _;
