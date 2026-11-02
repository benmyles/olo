//! Backup: every Olo setting and piece of saved state, in one file and
//! back. A backup is a zstd-compressed tar of `config/` (the settings
//! directory, `~/.config/olo`) and `state/` (`~/.local/state/olo`),
//! with `manifest.json` first saying what it holds. Secrets stay as they
//! are on disk (Passwords' vault is encrypted). Importing shows what a
//! backup holds and asks before replacing anything; the settings as they
//! were are saved first (`state/backups`, the last few kept), so an import
//! can be undone by importing that. Files the backup doesn't have are left
//! alone. Export and import run on a worker thread with progress.
//!
//! Agents: `olo-preferences --export FILE`, `--inspect FILE` and
//! `--import FILE` do the same without a window (the summary on stdout).

use std::{
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, channel},
    },
    time::Duration,
};

use olo_protocol::Settings;
use olo_ui::{
    file_panel::{self, FilePanel, PanelResult},
    gpui::{Context, FontWeight, ParentElement as _, ScrollHandle, Styled as _, canvas, div, px},
    metrics as m,
    style::{self, c},
    theme,
    widgets::{Button, ScrollView, StatusBar, StringView},
};
use serde::{Deserialize, Serialize};

use crate::{Msg, PanelView, form::Ui, panel};

pub const EXTENSION: &str = "olo-backup";
/// Backups made before the rename (Tabula), which still import.
pub const OLD_EXTENSION: &str = "tabula-backup";
const FORMAT: &str = "olo-backup";
const OLD_FORMAT: &str = "tabula-backup";
const MANIFEST: &str = "manifest.json";
/// Automatic backups made before an import that are kept.
const KEEP: usize = 5;

/// What a backup holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    /// When it was made (RFC 3339, local time).
    pub created: String,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    /// `config/…` or `state/…`.
    pub path: String,
    pub size: u64,
}

impl Manifest {
    /// The files by app: (app, files, bytes), apps by name.
    pub fn groups(&self) -> Vec<(String, usize, u64)> {
        let mut groups: Vec<(String, usize, u64)> = Vec::new();
        for f in &self.files {
            let app = app_of(&f.path);
            match groups.iter_mut().find(|g| g.0 == app) {
                Some(g) => {
                    g.1 += 1;
                    g.2 += f.size;
                }
                None => groups.push((app, 1, f.size)),
            }
        }
        groups.sort_by_key(|g| g.0.to_lowercase());
        groups
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// "Oct 27, 2026, 2:20 AM" (or as written, if it can't be read).
    pub fn created_text(&self) -> String {
        self.created.parse::<jiff::Timestamp>().map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%b %-d, %Y, %-I:%M\u{a0}%p").to_string()).unwrap_or_else(|_| self.created.clone())
    }
}

/// Which app a backed-up file belongs to, for the summary.
fn app_of(path: &str) -> String {
    let rel = path.split_once('/').map_or(path, |(_, r)| r);
    for part in rel.split('/') {
        let stem = part.split('.').next().unwrap_or(part);
        match stem {
            "olo" | "tabula" | "" => continue,
            "app_server" => return "Desktop (Preferences)".into(),
            "signatures" => return "Viewer".into(),
            "desktop" => return "Tracker".into(),
            "palette" => return "Palette".into(),
            _ => {}
        }
        if let Some((name, _)) = olo_xdg::apps::our_app(stem) {
            return name.into();
        }
        let mut name = stem.replace(['-', '_'], " ");
        if let Some(first) = name.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        return name;
    }
    "Other".into()
}

/// The directories a backup holds, by their name in it.
pub fn roots() -> Vec<(&'static str, PathBuf)> {
    vec![("config", olo_ui::config_dir()), ("state", olo_ui::state_dir())]
}

/// Where automatic backups go (left out of backups themselves).
fn backups_dir(roots: &[(&'static str, PathBuf)]) -> Option<PathBuf> {
    roots.iter().find(|(n, _)| *n == "state").map(|(_, d)| d.join("backups"))
}

/// Every regular file under the roots: (name in the backup, path).
fn collect(roots: &[(&'static str, PathBuf)]) -> Vec<(String, PathBuf)> {
    let skip = backups_dir(roots);
    let mut out = Vec::new();
    for (name, dir) in roots {
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                let Ok(t) = e.file_type() else { continue };
                if t.is_dir() {
                    if Some(&path) != skip.as_ref() {
                        stack.push(path);
                    }
                    continue;
                }
                let file = e.file_name().to_string_lossy().into_owned();
                // Half-written files (atomic saves in progress).
                if !t.is_file() || file.ends_with(".saving") || file.ends_with(".tmp") || file.ends_with(".partial") {
                    continue;
                }
                let Ok(rel) = path.strip_prefix(dir) else { continue };
                out.push((format!("{name}/{}", rel.to_string_lossy()), path));
            }
        }
    }
    out.sort();
    out
}

fn now() -> String {
    jiff::Zoned::now().timestamp().to_string()
}

/// Progress: (bytes done, bytes in all).
pub type Progress<'a> = &'a dyn Fn(u64, u64);

/// Write a backup of `roots` to `dest` (atomically).
pub fn export(roots: &[(&'static str, PathBuf)], dest: &Path, progress: Progress, cancel: &AtomicBool) -> Result<Manifest, String> {
    let mut files = Vec::new();
    let mut data = Vec::new();
    for (name, path) in collect(roots) {
        // Read now, so the manifest and the archive agree.
        match std::fs::read(&path) {
            Ok(bytes) => {
                let mode = std::fs::metadata(&path).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777).unwrap_or(0o644);
                files.push(FileEntry { path: name.clone(), size: bytes.len() as u64 });
                data.push((name, bytes, mode));
            }
            Err(e) => tracing::info!("backup: skipping {}: {e}", path.display()),
        }
    }
    let manifest = Manifest { format: FORMAT.into(), version: 1, created: now(), files };
    let total = manifest.bytes();
    let partial = dest.with_file_name(format!(".{}.partial", dest.file_name().unwrap_or_default().to_string_lossy()));
    let write = || -> Result<(), String> {
        let file = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
        let enc = zstd::Encoder::new(file, 9).map_err(|e| e.to_string())?;
        let mut tar = tar::Builder::new(enc);
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        append(&mut tar, MANIFEST, &json, 0o644)?;
        let mut done = 0;
        for (name, bytes, mode) in &data {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled.".into());
            }
            append(&mut tar, name, bytes, *mode)?;
            done += bytes.len() as u64;
            progress(done, total);
        }
        let enc = tar.into_inner().map_err(|e| e.to_string())?;
        enc.finish().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())
    };
    match write() {
        Ok(()) => std::fs::rename(&partial, dest).map_err(|e| e.to_string()).map(|()| manifest),
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

fn append<W: Write>(tar: &mut tar::Builder<W>, name: &str, bytes: &[u8], mode: u32) -> Result<(), String> {
    let mut h = tar::Header::new_gnu();
    h.set_size(bytes.len() as u64);
    h.set_mode(mode);
    h.set_mtime(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
    h.set_entry_type(tar::EntryType::Regular);
    tar.append_data(&mut h, name, bytes).map_err(|e| e.to_string())
}

fn open(path: &Path) -> Result<tar::Archive<zstd::Decoder<'static, std::io::BufReader<std::fs::File>>>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let dec = zstd::Decoder::new(file).map_err(|_| "This isn't an Olo backup.".to_string())?;
    Ok(tar::Archive::new(dec))
}

fn not_backup() -> String {
    "This isn't an Olo backup.".into()
}

/// What the backup at `path` holds (its manifest, checked).
pub fn inspect(path: &Path) -> Result<Manifest, String> {
    let mut archive = open(path)?;
    let mut entries = archive.entries().map_err(|_| not_backup())?;
    let mut first = entries.next().ok_or_else(not_backup)?.map_err(|_| not_backup())?;
    if first.path().ok().as_deref() != Some(Path::new(MANIFEST)) {
        return Err(not_backup());
    }
    let mut json = String::new();
    first.read_to_string(&mut json).map_err(|_| not_backup())?;
    let manifest: Manifest = serde_json::from_str(&json).map_err(|_| not_backup())?;
    if manifest.format != FORMAT && manifest.format != OLD_FORMAT {
        return Err(not_backup());
    }
    if manifest.version > 1 {
        return Err("This backup was made by a newer Olo.".into());
    }
    Ok(manifest)
}

/// Where a file named `name` in a backup goes, if it is one of ours.
fn target(roots: &[(&'static str, PathBuf)], name: &Path) -> Option<PathBuf> {
    let mut parts = name.components();
    let Some(Component::Normal(root)) = parts.next() else { return None };
    let (_, dir) = roots.iter().find(|(n, _)| std::ffi::OsStr::new(n) == root)?;
    let rest: PathBuf = parts.clone().collect();
    if rest.as_os_str().is_empty() || !parts.all(|p| matches!(p, Component::Normal(_))) {
        return None;
    }
    Some(dir.join(rest))
}

/// What an import did.
#[derive(Debug)]
pub struct Imported {
    pub manifest: Manifest,
    pub files: usize,
    /// The settings as they were, saved first.
    pub before: Option<PathBuf>,
    /// app_server's settings from the backup, to hand to it (it keeps its
    /// own copy and would write the old one back).
    pub settings: Option<Settings>,
}

/// Replace the files under `roots` with the backup's, saving them first.
pub fn import(roots: &[(&'static str, PathBuf)], src: &Path, progress: Progress, cancel: &AtomicBool) -> Result<Imported, String> {
    let manifest = inspect(src)?;
    // The settings as they are, so this can be undone.
    let before = match backups_dir(roots) {
        Some(dir) if !collect(roots).is_empty() => {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let stamp = jiff::Zoned::now().strftime("%Y-%m-%d %H.%M.%S").to_string();
            let path = dir.join(format!("Before import {stamp}.{EXTENSION}"));
            export(roots, &path, &|_, _| {}, cancel)?;
            prune(&dir);
            Some(path)
        }
        _ => None,
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled.".into());
    }
    // From here on it runs to the end: each file is replaced whole.
    let total = manifest.bytes();
    let mut archive = open(src)?;
    let (mut done, mut files, mut settings) = (0, 0, None);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let name = entry.path().map_err(|e| e.to_string())?.into_owned();
        if name == Path::new(MANIFEST) || entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let Some(dest) = target(roots, &name) else {
            tracing::info!("backup: ignoring {}", name.display());
            continue;
        };
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if name == Path::new("config/app_server.json") {
            settings = serde_json::from_slice(&bytes).ok();
        }
        write_atomic(&dest, &bytes, mode).map_err(|e| format!("“{}” couldn't be written: {e}", dest.display()))?;
        files += 1;
        done += bytes.len() as u64;
        progress(done, total);
    }
    Ok(Imported { manifest, files, before, settings })
}

fn write_atomic(dest: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = dest.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.partial", dest.file_name().unwrap_or_default().to_string_lossy()));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(mode))?;
    std::fs::rename(&tmp, dest)
}

/// Keep only the newest automatic backups.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut all: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(EXTENSION) || e.file_name().to_string_lossy().ends_with(OLD_EXTENSION))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    all.sort();
    let extra = all.len().saturating_sub(KEEP);
    for (_, p) in all.into_iter().take(extra) {
        let _ = std::fs::remove_file(p);
    }
}

/// A size as people read it.
pub fn size_text(bytes: u64) -> String {
    match bytes {
        b if b < 1000 => format!("{b} bytes"),
        b if b < 1_000_000 => format!("{:.1} KB", b as f64 / 1e3),
        b => format!("{:.1} MB", b as f64 / 1e6),
    }
}

// ---- The panel ----

/// What the worker reports.
enum Report {
    Progress(u64, u64),
    Exported(PathBuf, Result<Manifest, String>),
    Inspected(PathBuf, Result<Manifest, String>),
    Imported(Result<Box<Imported>, String>),
}

enum Phase {
    Idle,
    Working { what: &'static str, done: u64, total: u64, cancel: Arc<AtomicBool>, cancellable: bool },
    /// A backup chosen for import: what it holds, before replacing.
    Confirm(PathBuf, Manifest),
}

pub struct State {
    phase: Phase,
    rx: Option<Receiver<Report>>,
    /// The outcome of the last export or import.
    message: Option<(bool, String)>,
    /// The settings as they were before the last import.
    before: Option<PathBuf>,
    proxy: olo_ui::Proxy,
    scroll: ScrollHandle,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        State { phase: Phase::Idle, rx: None, message: None, before: None, proxy: olo_ui::proxy(cx), scroll: ScrollHandle::new() }
    }

    fn work(&mut self, what: &'static str, cancellable: bool, job: impl FnOnce(&dyn Fn(Report), &AtomicBool) + Send + 'static) {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let proxy = self.proxy.clone();
        std::thread::Builder::new()
            .name("backup".into())
            .spawn(move || {
                // Progress drawn at most twenty times a second.
                let last = std::cell::Cell::new(None::<std::time::Instant>);
                let send = move |r: Report| {
                    let progress = matches!(r, Report::Progress(..));
                    let _ = tx.send(r);
                    if !progress || last.get().is_none_or(|t| t.elapsed() > Duration::from_millis(50)) {
                        last.set(Some(std::time::Instant::now()));
                        proxy.send(Msg::Redraw);
                    }
                };
                job(&send, &flag);
            })
            .expect("thread");
        self.rx = Some(rx);
        self.message = None;
        self.before = None;
        self.phase = Phase::Working { what, done: 0, total: 0, cancel, cancellable };
    }

    /// The file panel chose where to export to.
    pub fn export_to(&mut self, dest: PathBuf) {
        self.work("Exporting…", true, move |send, cancel| {
            let r = export(&roots(), &dest, &|d, t| send(Report::Progress(d, t)), cancel);
            send(Report::Exported(dest, r));
        });
    }

    /// The file panel chose a backup to import: read what it holds.
    pub fn inspect(&mut self, src: PathBuf) {
        if matches!(self.phase, Phase::Working { .. }) {
            return;
        }
        self.work("Reading the backup…", false, move |send, _| {
            let r = inspect(&src);
            send(Report::Inspected(src, r));
        });
    }

    fn import(&mut self, src: PathBuf) {
        self.work("Importing…", true, move |send, cancel| {
            let r = import(&roots(), &src, &|d, t| send(Report::Progress(d, t)), cancel);
            send(Report::Imported(r.map(Box::new)));
        });
    }

    /// Take in what the worker reported; app_server's settings from an
    /// import, when there were some.
    fn poll(&mut self) -> Option<Settings> {
        let mut settings = None;
        while let Some(r) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match r {
                Report::Progress(d, t) => {
                    if let Phase::Working { done, total, .. } = &mut self.phase {
                        (*done, *total) = (d, t);
                    }
                }
                Report::Exported(dest, r) => {
                    self.phase = Phase::Idle;
                    self.message = Some(match r {
                        Ok(m) => (true, format!("Exported {} files ({}) to “{}”.", m.files.len(), size_text(m.bytes()), name_of(&dest))),
                        Err(e) => (false, format!("The backup couldn't be written: {e}")),
                    });
                }
                Report::Inspected(src, r) => match r {
                    Ok(m) => self.phase = Phase::Confirm(src, m),
                    Err(e) => {
                        self.phase = Phase::Idle;
                        self.message = Some((false, format!("“{}” can't be imported. {e}", name_of(&src))));
                    }
                },
                Report::Imported(r) => {
                    self.phase = Phase::Idle;
                    self.message = Some(match r {
                        Ok(i) => {
                            settings = i.settings;
                            self.before = i.before.clone();
                            let undo = match &i.before {
                                Some(p) => format!(" Your settings as they were are in “{}”.", name_of(p)),
                                None => String::new(),
                            };
                            (true, format!("Imported {} files. Apps that are open take some settings when they next start.{undo}", i.files))
                        }
                        Err(e) => (false, format!("The backup couldn't be imported: {e}")),
                    });
                }
            }
        }
        settings
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Backup(st) => Some(st),
        _ => None,
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The name an export is offered under.
pub fn suggested_name() -> String {
    format!("Olo Settings {}.{EXTENSION}", jiff::Zoned::now().strftime("%Y-%m-%d"))
}

/// A file panel for the backup (to export to, or to import), its answer
/// to this panel's window.
fn ask(ui: &Ui, export: bool) -> impl Fn(&mut olo_ui::gpui::Window, &mut olo_ui::gpui::App) + 'static {
    let view = ui.cx.entity().downgrade();
    move |_, cx| {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let (title, panel) = if export {
            ("Export Settings", FilePanel::save(home, suggested_name()).types(&[EXTENSION]))
        } else {
            ("Import Settings", FilePanel::open(home).types(&[EXTENSION, OLD_EXTENSION]))
        };
        let view = view.clone();
        file_panel::show(cx, title, panel, move |a, cx| {
            let PanelResult::Chosen(path) = a.result else { return };
            let _ = view.update(cx, |v, cx| {
                if let Some(st) = state_of(v) {
                    if export {
                        st.export_to(path);
                    } else {
                        st.inspect(path);
                    }
                }
                cx.notify();
            });
        });
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    if let Some(s) = st.poll() {
        // app_server keeps its own copy: hand it the backup's.
        let prefs = ui.prefs();
        ui.cx.defer(move |cx| {
            let _ = prefs.update(cx, |p, cx| p.edit(cx, |now| *now = s));
        });
    }
    let p = theme::palette(ui.cx);
    let idle = matches!(st.phase, Phase::Idle);
    let (export_it, import_it) = (ask(ui, true), ask(ui, false));
    ui.bbox("Export", |ui| {
        ui.note("Saves every Olo setting and what apps keep between runs (windows, histories, Passwords' encrypted vault…) in one file.");
        ui.enabled_if(idle, |ui| ui.button("Export…", export_it));
    });
    ui.bbox("Import", |ui| {
        ui.note("Puts the settings from a backup in place of these. What you have now is saved first.");
        ui.enabled_if(idle, |ui| ui.button("Import…", import_it));
    });
    match &st.phase {
        Phase::Idle => {
            if let Some((ok, text)) = &st.message {
                let color = if *ok { c(p.panel_text) } else { c(p.failure) };
                ui.add(StringView::new(text.clone()).wrap(true).color(color));
                if *ok && let Some(before) = st.before.clone() {
                    ui.button(olo_ui::tracker::LABEL, move |_, _| {
                        if let Err(e) = olo_ui::tracker::open(&before) {
                            tracing::warn!("couldn't open Tracker: {e}");
                        }
                    });
                }
            }
        }
        Phase::Working { what, done, total, cancel, cancellable } => {
            ui.label(*what);
            let fraction = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
            let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().child(StatusBar::new(fraction)));
            if *cancellable {
                let cancel = cancel.clone();
                let stopping = cancel.load(Ordering::Relaxed);
                row = row.child(Button::new("Cancel").enabled(!stopping).on_click(move |_, _| cancel.store(true, Ordering::Relaxed)));
            }
            ui.add(row);
        }
        Phase::Confirm(src, manifest) => {
            ui.add(StringView::new(format!("“{}”, made {}, holds:", name_of(src), manifest.created_text())).wrap(true));
            let groups = manifest.groups();
            let rows = groups.len().min(5).max(1) as f32;
            let list = groups.into_iter().map(|(app, files, bytes)| {
                let what = format!("{} {}, {}", files, if files == 1 { "file" } else { "files" }, size_text(bytes));
                canvas(|_, _, _| {}, move |r, _, window, cx| {
                    let p = theme::palette(cx);
                    style::label(window, cx, r.left() + px(6.0), r.center().y, style::Align::Left, app.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), c(p.document_text));
                    style::label(window, cx, r.right() - px(6.0), r.center().y, style::Align::Right, what.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                })
                .w_full()
                .h(px(m::LIST_ROW))
            });
            ui.add(div().h(px(rows * m::LIST_ROW + 4.0)).child(ScrollView::new("backup-groups", &st.scroll).background(p.document_background).auto_hide(true).child(div().flex().flex_col().children(list.collect::<Vec<_>>()))));
            let src = src.clone();
            let go = ui.on_click_view(move |v, _, cx| {
                if let Some(st) = state_of(v) {
                    st.import(src.clone());
                }
                cx.notify();
            });
            let back = ui.on_click_view(|v, _, cx| {
                if let Some(st) = state_of(v) {
                    st.phase = Phase::Idle;
                }
                cx.notify();
            });
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(div().flex_1().child(StringView::new("Replace your settings with these?")))
                    .child(Button::new("Cancel").on_click(back))
                    .child(Button::new("Import").default(true).on_click(go)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(tag: &str) -> (PathBuf, Vec<(&'static str, PathBuf)>) {
        let base = std::env::temp_dir().join(format!("olo-backup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        (base.clone(), vec![("config", base.join("config")), ("state", base.join("state"))])
    }

    #[test]
    fn export_and_import_round_trip() {
        let (base, roots) = dirs("trip");
        let (cfg, state) = (roots[0].1.clone(), roots[1].1.clone());
        std::fs::create_dir_all(cfg.join("olo")).unwrap();
        std::fs::create_dir_all(state.join("notifications")).unwrap();
        std::fs::write(cfg.join("terminal.json"), b"{\"font\":12}").unwrap();
        std::fs::write(cfg.join("olo/palette.json"), b"[]").unwrap();
        write_atomic(&cfg.join("passwords.vault"), b"sealed", 0o600).unwrap();
        std::fs::write(cfg.join(".x.json.saving"), b"half").unwrap();
        std::fs::write(state.join("notifications/history.json"), b"[1,2]").unwrap();
        let file = base.join("b.olo-backup");
        let no = AtomicBool::new(false);
        let m = export(&roots, &file, &|_, _| {}, &no).unwrap();
        let names: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["config/olo/palette.json", "config/passwords.vault", "config/terminal.json", "state/notifications/history.json"]);
        assert_eq!(inspect(&file).unwrap(), m);
        let groups: Vec<String> = m.groups().into_iter().map(|g| g.0).collect();
        assert_eq!(groups, ["Notifications", "Palette", "Passwords", "Terminal"]);

        // Change things, then bring the backup back.
        std::fs::write(cfg.join("terminal.json"), b"{\"font\":20}").unwrap();
        std::fs::remove_file(cfg.join("passwords.vault")).unwrap();
        std::fs::write(cfg.join("new.json"), b"{}").unwrap();
        let i = import(&roots, &file, &|_, _| {}, &no).unwrap();
        assert_eq!(i.files, 4);
        assert_eq!(std::fs::read(cfg.join("terminal.json")).unwrap(), b"{\"font\":12}");
        assert_eq!(std::fs::read(cfg.join("passwords.vault")).unwrap(), b"sealed");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(cfg.join("passwords.vault")).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(cfg.join("new.json").exists(), "files the backup doesn't have are left alone");
        // What was there before was saved, and isn't itself backed up.
        let before = inspect(i.before.as_ref().unwrap()).unwrap();
        assert!(before.files.iter().any(|f| f.path == "config/new.json"));
        assert!(collect(&roots).iter().all(|(n, _)| !n.starts_with("state/backups")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn refuses_what_isnt_a_backup() {
        let (base, roots) = dirs("bad");
        std::fs::create_dir_all(&base).unwrap();
        let junk = base.join("junk.olo-backup");
        std::fs::write(&junk, b"not a backup").unwrap();
        assert!(inspect(&junk).is_err());
        // Names that would land outside the roots are not ours.
        assert_eq!(target(&roots, Path::new("config/../../etc/passwd")), None);
        assert_eq!(target(&roots, Path::new("/etc/passwd")), None);
        assert_eq!(target(&roots, Path::new("other/x.json")), None);
        assert_eq!(target(&roots, Path::new("config")), None);
        assert_eq!(target(&roots, Path::new("config/a/b.json")), Some(roots[0].1.join("a/b.json")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn names_apps() {
        assert_eq!(app_of("config/app_server.json"), "Desktop (Preferences)");
        assert_eq!(app_of("config/terminal.json"), "Terminal");
        assert_eq!(app_of("state/notifications/history.json"), "Notifications");
        assert_eq!(app_of("config/deskcalc.json"), "DeskCalc");
        assert_eq!(app_of("config/some-thing.json"), "Some thing");
    }
}
