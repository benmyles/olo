//! The recordings (`olo-screenshot --library`; Screenshot's Record ▸
//! Recordings…, Deskbar's recording menu): every screen recording in the
//! recordings folder ("Screen Recording …" in Videos, their edited copies
//! too), as CleanShot X's capture history. A list (the kit's
//! BColumnListView) of each one's picture, name, length, size and date,
//! newest first (a header click sorts), found by name as it's typed; and
//! Play (the Viewer), Edit (the Viewer's editor), Show in Tracker,
//! Rename… and Delete (to the Trash). A double click plays. What each is
//! (its length and pixels, `olo_preview::video::probe`) and its picture
//! (`picture_at`, a tenth in) are made on a worker as they're needed and
//! kept; the folder is followed (inotify), so a new recording, a rename
//! or a deletion elsewhere shows at once. ⌘Z undoes a Delete (put back
//! from the Trash) or a Rename…, ⇧⌘Z redoes it. `olo-screenshot --recordings`
//! prints the same list as JSON, for agents.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use olo_kit::app_kit::Messenger;
use olo_kit::{
    Bounds, Child, ClickEvent, Corners, Cx, Element, InteractiveElement as _, KeyDownEvent, ParentElement as _, RenderImage, Styled as _, View, WindowOptions, div, instance, metrics as m, point, px,
    size, style, theme,
    undo::{History, Purpose},
    widgets::{Button, Column, ColumnListView, ListModel, StringView, TextControl, TextField, TextFieldEvent, button_width, text_field},
};

const APP_ID: &str = "olo.screenshot-library";
/// A row's picture (points), and the row.
const PICTURE: (f32, f32) = (64.0, 40.0);
const ROW_H: f32 = 48.0;

/// One recording.
#[derive(Clone, Debug)]
struct Recording {
    path: PathBuf,
    name: String,
    bytes: u64,
    modified: SystemTime,
    /// Its length and pixels, once read.
    seconds: Option<f64>,
    pixels: Option<(u32, u32)>,
}

enum Msg {
    /// The folder's recordings, as listed now.
    Listed(Vec<Recording>),
    /// What one is: its length and pixels, and its picture for a scale.
    Read(PathBuf, Option<f64>, Option<(u32, u32)>, Option<(f32, Arc<RenderImage>)>),
    /// The folder changed.
    Changed,
    /// Rename… renamed one.
    Renamed { from: PathBuf, to: PathBuf },
}

/// Where recordings go (the Videos folder).
pub fn folder() -> PathBuf {
    olo_xdg::user_dir("VIDEOS").unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Videos"))
}

/// The recordings in `dir`: MP4s and GIFs Olo made (its mark in them,
/// `olo_video::made_by_olo`: renamed, they're still known), and those
/// named as it names them ("Screen Recording …", made before the mark).
fn list(dir: &Path) -> Vec<Recording> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Recording> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ext = Path::new(&name).extension().map(|x| x.to_string_lossy().to_ascii_lowercase());
            if !matches!(ext.as_deref(), Some("mp4" | "gif")) || !(name.starts_with("Screen Recording") || olo_video::made_by_olo(&e.path())) {
                return None;
            }
            let md = e.metadata().ok().filter(|m| m.is_file())?;
            Some(Recording { path: e.path(), name, bytes: md.len(), modified: md.modified().unwrap_or(SystemTime::UNIX_EPOCH), seconds: None, pixels: None })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// `m:ss` (or `h:mm:ss`).
fn length(seconds: f64) -> String {
    let s = seconds.round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / f64::from(1u32 << 30)),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / f64::from(1u32 << 20)),
        n if n >= 1 << 10 => format!("{} KB", n >> 10),
        n => format!("{n} bytes"),
    }
}

/// When, as the person reads it ("6 Nov 2026, 14:23").
fn when(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: localtime_r fills `tm` from `secs`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {}, {:02}:{:02}", tm.tm_mday, MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_year + 1900, tm.tm_hour, tm.tm_min)
}

/// What a recording is, and its picture `PICTURE` big at `scale` (exact
/// device pixels, its shape kept), read on a worker.
fn read(path: &Path, scale: f32) -> (Option<f64>, Option<(u32, u32)>, Option<Arc<RenderImage>>) {
    let gif = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    let (seconds, picture) = if gif {
        (None, image::open(path).ok().map(|i| i.into_rgba8()))
    } else {
        let input = olo_preview::input::Input::file(path);
        let seconds = olo_preview::video::probe(&input).ok().and_then(|i| i.duration);
        let at = seconds.map_or(0.0, |s| (s * 0.1).min(1.0));
        let picture = olo_preview::video::picture_at(&input, at).ok().and_then(|b| image::RgbaImage::from_raw(b.size[0] as u32, b.size[1] as u32, b.rgba.clone()));
        (seconds, picture)
    };
    let pixels = picture.as_ref().map(|p| p.dimensions());
    let image = picture.and_then(|p| {
        let (pw, ph) = p.dimensions();
        let k = (PICTURE.0 / pw as f32).min(PICTURE.1 / ph as f32);
        let (w, h) = (((pw as f32 * k * scale).round() as u32).max(1), ((ph as f32 * k * scale).round() as u32).max(1));
        crate::preview_of(&p, w, h)
    });
    (seconds, pixels, image)
}

/// What Undo takes back here.
#[derive(Debug, Clone)]
enum Change {
    /// Deleted: where it was, and where it is in the Trash.
    Trashed {
        /// (Said when a change is logged.)
        #[allow(dead_code)]
        from: PathBuf,
        at: PathBuf,
    },
    /// Put back from the Trash (Redo deletes it again).
    PutBack(PathBuf),
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
}

struct Library {
    /// Undo and Redo (⌘Z, ⇧⌘Z: the window has no menu bar).
    history: History<Change>,
    all: Vec<Recording>,
    /// The rows shown (indexes into `all`), as found and sorted.
    shown: Vec<usize>,
    pictures: HashMap<PathBuf, (f32, Arc<RenderImage>)>,
    /// Asked for (a worker reading it), by path, for a scale.
    asked: HashMap<PathBuf, f32>,
    list: ListModel,
    search: Child<TextField>,
    _watch: Option<olo_kit::watch::Watch>,
    scale: f32,
}

impl Library {
    fn new(cx: &mut Cx<Self>) -> Library {
        let list = ListModel::new(false);
        list.state_mut().sort = Some((4, false));
        let search = text_field(cx, |f| f.name("Search").hint("Search"));
        cx.subscribe(&search, |l: &mut Library, _: &TextFieldEvent, cx| {
            l.refilter(cx);
            cx.invalidate();
        });
        let to = cx.messenger();
        let watch = olo_kit::watch::watch([folder()], move |_| {
            to.send(Msg::Changed);
        });
        cx.send(Msg::Changed);
        Library { history: History::default(), all: Vec::new(), shown: Vec::new(), pictures: HashMap::new(), asked: HashMap::new(), list, search, _watch: watch, scale: 0.0 }
    }

    /// The rows: those whose name has what's typed, sorted as the header
    /// says (newest first until it says otherwise).
    fn refilter(&mut self, cx: &mut Cx<Self>) {
        let typed = self.search.read(cx).text().to_lowercase();
        let mut shown: Vec<usize> = (0..self.all.len()).filter(|&i| typed.is_empty() || self.all[i].name.to_lowercase().contains(&typed)).collect();
        let sort = self.list.state().sort;
        let all = &self.all;
        if let Some((col, up)) = sort {
            shown.sort_by(|&a, &b| {
                let (a, b) = (&all[a], &all[b]);
                let o = match col {
                    1 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    2 => a.seconds.unwrap_or(0.0).total_cmp(&b.seconds.unwrap_or(0.0)),
                    3 => a.bytes.cmp(&b.bytes),
                    _ => a.modified.cmp(&b.modified),
                };
                if up { o } else { o.reverse() }
            });
        }
        if shown != self.shown {
            self.list.state_mut().selected.clear();
        }
        self.shown = shown;
    }

    /// Those not yet read (or read for another scale) read, a worker each
    /// few at a time.
    fn ask(&mut self, cx: &mut Cx<Self>) {
        let scale = self.scale;
        let want: Vec<PathBuf> =
            self.shown.iter().map(|&i| self.all[i].path.clone()).filter(|p| self.pictures.get(p).is_none_or(|(s, _)| *s != scale) && self.asked.get(p) != Some(&scale)).take(64).collect();
        if want.is_empty() {
            return;
        }
        for p in &want {
            self.asked.insert(p.clone(), scale);
        }
        let to = cx.messenger();
        std::thread::spawn(move || {
            for p in want {
                let (seconds, pixels, picture) = read(&p, scale);
                to.send(Msg::Read(p, seconds, pixels, picture.map(|i| (scale, i))));
            }
        });
    }

    fn chosen(&self) -> Option<PathBuf> {
        let row = self.list.state().single()?;
        self.shown.get(row).map(|&i| self.all[i].path.clone())
    }

    fn act(&mut self, command: &str, cx: &mut Cx<Self>) {
        let Some(path) = self.chosen() else { return };
        let viewer = |args: &[&std::ffi::OsStr]| match olo_xdg::find_program("olo-viewer") {
            Some(v) => {
                let mut argv = vec![v.into_os_string()];
                argv.extend(args.iter().map(|a| a.to_os_string()));
                if let Err(e) = olo_xdg::open::spawn(&argv) {
                    eprintln!("olo-screenshot: couldn't open the Viewer: {e}");
                }
            }
            None => eprintln!("olo-screenshot: no Viewer here"),
        };
        match command {
            "play" => viewer(&[path.as_os_str()]),
            "edit" => viewer(&["--edit".as_ref(), path.as_os_str()]),
            "show" => {
                let _ = olo_kit::tracker::open(&path);
            }
            "delete" => match crate::after::to_trash(&path) {
                Ok(at) => self.history.done("Delete", Change::Trashed { from: path, at }),
                Err(e) => eprintln!("olo-screenshot: couldn't move {} to the Trash: {e}", path.display()),
            },
            "rename" => {
                let to = cx.messenger();
                olo_kit::open_window_here(cx, WindowOptions::new("Rename", 360.0, 120.0).dialog(), move |cx| Rename::new(to, path, cx));
            }
            _ => {}
        }
    }

    /// ⌘Z (`redo` false) or ⇧⌘Z: a Delete or a Rename… taken back, or
    /// done again. (The folder is followed: the list shows it.)
    fn undo_redo(&mut self, redo: bool, cx: &mut Cx<Self>) {
        let Some(step) = (if redo { self.history.take_redo() } else { self.history.take_undo() }) else { return };
        let purpose = if redo { Purpose::Redo } else { Purpose::Undo };
        let done = match step.change {
            Change::Trashed { at, .. } => olo_xdg::trash::put_back(&at).map(Change::PutBack),
            Change::PutBack(path) => crate::after::to_trash(&path).map(|at| Change::Trashed { from: path, at }),
            Change::Renamed { from, to } => {
                if from.exists() {
                    Err(std::io::Error::other(format!("“{}” is taken", from.file_name().unwrap_or_default().to_string_lossy())))
                } else {
                    std::fs::rename(&to, &from).map(|()| Change::Renamed { from: to, to: from })
                }
            }
        };
        match done {
            Ok(change) => self.history.record(purpose, step.name, change),
            Err(e) => eprintln!("olo-screenshot: couldn't {} {}: {e}", if redo { "redo" } else { "undo" }, step.name),
        }
        cx.invalidate();
    }
}

impl View for Library {
    type Message = Msg;

    fn message(&mut self, m: Msg, cx: &mut Cx<Self>) {
        match m {
            Msg::Listed(all) => {
                // (What was read of those still here kept.)
                let known: HashMap<PathBuf, (Option<f64>, Option<(u32, u32)>)> = self.all.iter().map(|r| (r.path.clone(), (r.seconds, r.pixels))).collect();
                self.all = all
                    .into_iter()
                    .map(|mut r| {
                        if let Some((s, p)) = known.get(&r.path) {
                            (r.seconds, r.pixels) = (*s, *p);
                        }
                        r
                    })
                    .collect();
                // (A picture let go of takes its texture with it.)
                let here: std::collections::HashSet<&PathBuf> = self.all.iter().map(|r| &r.path).collect();
                self.pictures.retain(|p, _| here.contains(p));
                self.refilter(cx);
            }
            Msg::Read(path, seconds, pixels, picture) => {
                self.asked.remove(&path);
                if let Some(r) = self.all.iter_mut().find(|r| r.path == path) {
                    r.seconds = seconds.or(r.seconds);
                    r.pixels = pixels.or(r.pixels);
                }
                if let Some(p) = picture {
                    self.pictures.insert(path, p);
                }
                self.refilter(cx);
            }
            Msg::Changed => cx.work(|| list(&folder()), Msg::Listed),
            Msg::Renamed { from, to } => self.history.done("Rename", Change::Renamed { from, to }),
        }
        cx.invalidate();
    }

    /// Edit's Undo and Redo (⌘Z, ⇧⌘Z: the window has no menu bar).
    fn command(&mut self, name: &str, cx: &mut Cx<Self>) -> bool {
        match name {
            "undo" => self.undo_redo(false, cx),
            "redo" => self.undo_redo(true, cx),
            _ => return false,
        }
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        self.scale = cx.scale_factor();
        self.ask(cx);
        let p = theme::palette();
        let columns = vec![
            Column::new("Picture", PICTURE.0 + 2.0 * m::CELL_INSET),
            Column::new("Name", 300.0),
            Column::new("Length", 70.0).right(),
            Column::new("Size", 80.0).right(),
            Column::new("Date", 150.0),
        ];
        let list = ColumnListView::new(cx, "recordings", &self.list, columns, self.shown.len(), |l: &Library, cell, g| {
            let Some(r) = l.shown.get(cell.row).map(|&i| &l.all[i]) else { return };
            match cell.column {
                0 => {
                    let b = cell.bounds;
                    let frame = style::snap(g, Bounds::centered_at(b.center(), size(px(PICTURE.0), px(PICTURE.1))));
                    let p = theme::palette();
                    style::fill_rect(g, frame, olo_kit::black());
                    let s = g.scale_factor();
                    if let Some((_, img)) = l.pictures.get(&r.path).filter(|(made, _)| *made == s) {
                        let sz = img.size(0);
                        let (w, h) = (sz.width.0 as f32 / s, sz.height.0 as f32 / s);
                        let at = style::snap(g, Bounds::new(point(frame.center().x - px(w / 2.0), frame.center().y - px(h / 2.0)), size(px(w), px(h))));
                        let _ = g.paint_image(Bounds::new(at.origin, size(px(w), px(h))), Corners::default(), img.clone(), 0, false);
                    }
                    style::frame_rect(g, frame, style::tint(p.panel_background, 1.4));
                    cell.say(match r.pixels {
                        Some((w, h)) => format!("Picture, {w} × {h}"),
                        None => "Picture".to_owned(),
                    });
                }
                1 => cell.text(g, r.name.clone()),
                2 => cell.text(g, r.seconds.map(length).unwrap_or_else(|| if r.name.to_lowercase().ends_with(".gif") { "GIF".into() } else { String::new() })),
                3 => cell.text(g, bytes(r.bytes)),
                _ => cell.text(g, when(r.modified)),
            }
        })
        .row_height(ROW_H)
        .empty_text("No screen recordings yet. Record one with ⌘⇧6.")
        .type_select(|l: &Library, i| l.shown.get(i).map(|&i| l.all[i].name.clone()).unwrap_or_default().into())
        .on_open(cx.listener(|l: &mut Library, _: &usize, cx| l.act("play", cx)))
        .on_sort(cx.listener(|l: &mut Library, _: &(), cx| {
            l.refilter(cx);
            cx.invalidate();
        }))
        .on_select(cx.listener(|_: &mut Library, _: &(), cx| cx.invalidate()));
        let chosen = self.list.state().single().is_some();
        let cmd = |c: &'static str| cx.listener(move |l: &mut Library, _: &ClickEvent, cx| l.act(c, cx));
        let count = match (self.shown.len(), self.all.len()) {
            (n, t) if n == t => format!("{n} recording{}", if n == 1 { "" } else { "s" }),
            (n, t) => format!("{n} of {t} recordings"),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(m::WINDOW_INSET))
            .gap(px(m::SPACING))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(div().w(px(240.0)).child(TextControl::new(cx, &self.search)))
                    .child(div().flex_1().child(StringView::new(count).color(style::disabled_text(&p)))),
            )
            .child(div().flex_1().min_h_0().child(list))
            .child(
                div()
                    .flex()
                    .gap(px(m::TIGHT))
                    .child(Button::new("Play").enabled(chosen).on_click(cmd("play")))
                    .child(Button::new("Edit").enabled(chosen).on_click(cmd("edit")))
                    .child(Button::new("Show in Tracker").enabled(chosen).on_click(cmd("show")))
                    .child(div().flex_1())
                    .child(Button::new("Rename…").enabled(chosen).on_click(cmd("rename")))
                    .child(Button::new("Delete").enabled(chosen).on_click(cmd("delete"))),
            )
    }
}

/// Rename…: a recording's new name (its extension kept).
struct Rename {
    /// The library, told what was renamed (for Undo).
    library: Messenger<Msg>,
    path: PathBuf,
    field: Child<TextField>,
    error: Option<String>,
}

impl Rename {
    fn new(library: Messenger<Msg>, path: PathBuf, cx: &mut Cx<Self>) -> Rename {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let field = text_field(cx, |f| f.name("Name").with_text(stem));
        cx.subscribe(&field, |r: &mut Rename, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => r.done(true, cx),
            _ => {
                r.error = None;
                cx.invalidate();
            }
        });
        field.focus(cx);
        Rename { library, path, field, error: None }
    }

    fn done(&mut self, rename: bool, cx: &mut Cx<Self>) {
        if rename {
            let name = self.field.read(cx).text().trim().replace('/', "-");
            if name.is_empty() {
                return;
            }
            let ext = self.path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let to = self.path.with_file_name(format!("{name}{ext}"));
            if to != self.path {
                if to.exists() {
                    self.error = Some(format!("“{name}{ext}” is taken."));
                    return cx.invalidate();
                }
                if let Err(e) = std::fs::rename(&self.path, &to) {
                    self.error = Some(e.to_string());
                    return cx.invalidate();
                }
                self.library.send(Msg::Renamed { from: self.path.clone(), to });
            }
        }
        cx.close_window();
    }
}

impl View for Rename {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let bw = button_width("Rename", false).max(button_width("Cancel", false));
        div()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::STACK))
            .on_key_down(cx.listener(|r: &mut Rename, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    r.done(false, cx);
                }
            }))
            .child(TextControl::new(cx, &self.field))
            .child(StringView::new(self.error.clone().unwrap_or_else(|| "Its file’s name, in its folder.".into())).color(if self.error.is_some() {
                style::c(p.failure)
            } else {
                style::disabled_text(&p)
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Cancel").min_width(bw).on_click(cx.listener(|r: &mut Rename, _: &ClickEvent, cx| r.done(false, cx))))
                    .child(Button::new("Rename").default(true).min_width(bw).on_click(cx.listener(|r: &mut Rename, _: &ClickEvent, cx| r.done(true, cx)))),
            )
    }
}

/// `--recordings`: the recordings as JSON (path, name, seconds, width,
/// height, bytes, modified as seconds since the epoch), newest first.
pub fn print() -> i32 {
    let all: Vec<serde_json::Value> = list(&folder())
        .into_iter()
        .map(|r| {
            let info = olo_preview::video::probe(&olo_preview::input::Input::file(&r.path)).ok();
            serde_json::json!({
                "path": r.path, "name": r.name, "seconds": info.as_ref().and_then(|i| i.duration),
                "width": info.as_ref().map(|i| i.width), "height": info.as_ref().map(|i| i.height), "bytes": r.bytes,
                "modified": r.modified.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&all).unwrap_or_default());
    0
}

/// `--library`: the window (one: a second `--library` brings it forward).
pub fn run(args: &[String]) -> std::process::ExitCode {
    if instance::forward(APP_ID, args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        let window = olo_kit::open_window(WindowOptions::new("Recordings", 760.0, 520.0).min_size(560.0, 360.0).keep_size("recordings"), Library::new);
        olo_kit::message_handler(app, move |m, _| {
            if m.downcast_ref::<instance::Launch>().is_some() {
                olo_kit::raise(Some(&window));
            }
        });
    });
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_screen_recordings_newest_first() {
        let dir = PathBuf::from(format!("/var/tmp/olo-library-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        for (name, age) in [("Screen Recording a.mp4", 30), ("Screen Recording b (edited).mp4", 10), ("Screen Recording c.gif", 20), ("holiday.mp4", 5), ("Screen Recording d.txt", 1)] {
            let p = dir.join(name);
            std::fs::write(&p, b"x").unwrap();
            let t = SystemTime::now() - std::time::Duration::from_secs(age);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        }
        let names: Vec<String> = list(&dir).into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["Screen Recording b (edited).mp4", "Screen Recording c.gif", "Screen Recording a.mp4"]);
        assert_eq!(length(61.4), "1:01");
        assert_eq!(bytes(3 << 20), "3.0 MB");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
