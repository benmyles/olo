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
//! or a deletion elsewhere shows at once. `olo-screenshot --recordings`
//! prints the same list as JSON, for agents.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use olo_ui::{
    WindowOptions,
    gpui::{self, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, RenderImage, Styled as _, Window, div, point, px, size},
    instance, metrics as m, style, theme,
    widgets::{
        Button, StringView, TextControl, TextField, TextFieldEvent,
        list::{Column, ColumnListView, ListModel},
        text_field,
    },
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

struct Library {
    all: Vec<Recording>,
    /// The rows shown (indexes into `all`), as found and sorted.
    shown: Vec<usize>,
    pictures: HashMap<PathBuf, (f32, Arc<RenderImage>)>,
    /// Asked for (a worker reading it), by path, for a scale.
    asked: HashMap<PathBuf, f32>,
    dropped: Vec<Arc<RenderImage>>,
    list: Entity<ListModel>,
    search: Entity<TextField>,
    _watch: Option<olo_ui::watch::Watch>,
    scale: f32,
}

impl Library {
    fn message(&mut self, m: Msg, cx: &mut Context<Self>) {
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
                let here: std::collections::HashSet<&PathBuf> = self.all.iter().map(|r| &r.path).collect();
                let gone: Vec<PathBuf> = self.pictures.keys().filter(|p| !here.contains(p)).cloned().collect();
                for p in gone {
                    self.dropped.extend(self.pictures.remove(&p).map(|(_, i)| i));
                }
                self.refilter(cx);
            }
            Msg::Read(path, seconds, pixels, picture) => {
                self.asked.remove(&path);
                if let Some(r) = self.all.iter_mut().find(|r| r.path == path) {
                    r.seconds = seconds.or(r.seconds);
                    r.pixels = pixels.or(r.pixels);
                }
                if let Some(p) = picture {
                    self.dropped.extend(self.pictures.insert(path, p).map(|(_, i)| i));
                }
                self.refilter(cx);
            }
            Msg::Changed => {
                let proxy = olo_ui::proxy(cx);
                std::thread::spawn(move || proxy.send(Msg::Listed(list(&folder()))));
            }
        }
        cx.notify();
    }

    /// The rows: those whose name has what's typed, sorted as the header
    /// says (newest first until it says otherwise).
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let typed = self.search.read(cx).text().to_lowercase();
        let mut shown: Vec<usize> = (0..self.all.len()).filter(|&i| typed.is_empty() || self.all[i].name.to_lowercase().contains(&typed)).collect();
        let sort = self.list.read(cx).state.sort;
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
            self.list.update(cx, |l, _| l.state.selected.clear());
        }
        self.shown = shown;
    }

    /// Those not yet read (or read for another scale) read, a worker each
    /// few at a time.
    fn ask(&mut self, cx: &mut Context<Self>) {
        let scale = self.scale;
        let want: Vec<PathBuf> =
            self.shown.iter().map(|&i| self.all[i].path.clone()).filter(|p| self.pictures.get(p).is_none_or(|(s, _)| *s != scale) && self.asked.get(p) != Some(&scale)).take(64).collect();
        if want.is_empty() {
            return;
        }
        for p in &want {
            self.asked.insert(p.clone(), scale);
        }
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            for p in want {
                let (seconds, pixels, picture) = read(&p, scale);
                proxy.send(Msg::Read(p, seconds, pixels, picture.map(|i| (scale, i))));
            }
        });
    }

    fn chosen(&self, cx: &Context<Self>) -> Option<PathBuf> {
        let row = self.list.read(cx).state.single()?;
        self.shown.get(row).map(|&i| self.all[i].path.clone())
    }

    fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.chosen(cx) else { return };
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
                let _ = olo_ui::tracker::open(&path);
            }
            "delete" => {
                if let Err(e) = crate::after::to_trash(&path) {
                    eprintln!("olo-screenshot: couldn't move {} to the Trash: {e}", path.display());
                }
            }
            "rename" => {
                let me = cx.entity();
                cx.defer(move |cx| {
                    olo_ui::open_window(cx, WindowOptions::new("Rename", 360.0, 120.0).dialog(), move |window, cx| cx.new(|cx| Rename::new(me, path, window, cx)));
                });
            }
            _ => {}
        }
        let _ = window;
    }
}

impl Render for Library {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.scale_factor() != self.scale {
            self.scale = window.scale_factor();
        }
        self.ask(cx);
        let p = theme::palette(cx);
        let rows: Arc<Vec<(String, String, String, String, String, Option<Arc<RenderImage>>)>> = Arc::new(
            self.shown
                .iter()
                .map(|&i| {
                    let r = &self.all[i];
                    let picture = self.pictures.get(&r.path).filter(|(s, _)| *s == self.scale).map(|(_, img)| img.clone());
                    let pixels = r.pixels.map(|(w, h)| format!("{w} × {h}")).unwrap_or_default();
                    (
                        r.name.clone(),
                        r.seconds.map(length).unwrap_or_else(|| if r.name.to_lowercase().ends_with(".gif") { "GIF".into() } else { String::new() }),
                        bytes(r.bytes),
                        when(r.modified),
                        pixels,
                        picture,
                    )
                })
                .collect(),
        );
        let dropped = std::mem::take(&mut self.dropped);
        let columns = vec![
            Column::new("Picture", PICTURE.0 + 2.0 * m::CELL_INSET),
            Column::new("Name", 300.0),
            Column::new("Length", 70.0).right(),
            Column::new("Size", 80.0).right(),
            Column::new("Date", 150.0),
        ];
        let me = cx.entity();
        let (open_me, sort_me) = (me.clone(), me.clone());
        let cells = rows.clone();
        let first = std::cell::RefCell::new(Some(dropped));
        let list = ColumnListView::new("recordings", &self.list, columns, rows.len(), move |cell, window, cx| {
            for img in first.borrow_mut().take().into_iter().flatten() {
                let _ = window.drop_image(img);
            }
            let (name, len, size_s, date, pixels, picture) = &cells[cell.row];
            match cell.column {
                0 => {
                    let b = cell.bounds;
                    let frame = style::snap(window, gpui::Bounds::centered_at(b.center(), size(px(PICTURE.0), px(PICTURE.1))));
                    let p = theme::palette(cx);
                    style::fill_rect(window, frame, gpui::black());
                    if let Some(img) = picture {
                        let s = window.scale_factor();
                        let sz = img.size(0);
                        let (w, h) = (sz.width.0 as f32 / s, sz.height.0 as f32 / s);
                        let at = style::snap(window, gpui::Bounds::new(point(frame.center().x - px(w / 2.0), frame.center().y - px(h / 2.0)), size(px(w), px(h))));
                        let _ = window.paint_image(gpui::Bounds::new(at.origin, size(px(w), px(h))), gpui::Corners::default(), img.clone(), 0, false);
                    }
                    style::frame_rect(window, frame, style::tint(p.panel_background, 1.4));
                    cell.say(if pixels.is_empty() { "Picture".to_owned() } else { format!("Picture, {pixels}") });
                }
                1 => cell.text(window, cx, name.clone()),
                2 => cell.text(window, cx, len.clone()),
                3 => cell.text(window, cx, size_s.clone()),
                _ => cell.text(window, cx, date.clone()),
            }
        })
        .row_height(ROW_H)
        .empty_text("No screen recordings yet. Record one with ⌘⇧6.")
        .type_select({
            let rows = rows.clone();
            move |i| rows[i].0.clone().into()
        })
        .on_open(move |_, window, cx| open_me.update(cx, |l, cx| l.command("play", window, cx)))
        .on_sort(move |_, cx| {
            sort_me.update(cx, |l, cx| {
                l.refilter(cx);
                cx.notify();
            })
        })
        .on_select({
            let me = me.clone();
            move |_, cx| me.update(cx, |_, cx| cx.notify())
        });
        let chosen = self.list.read(cx).state.single().is_some();
        let cmd = |c: &'static str| {
            let me = me.clone();
            move |window: &mut Window, cx: &mut gpui::App| me.update(cx, |l, cx| l.command(c, window, cx))
        };
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
                    .child(div().w(px(240.0)).child(TextControl::new(self.search.clone())))
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
    library: Entity<Library>,
    path: PathBuf,
    field: Entity<TextField>,
    error: Option<String>,
}

impl Rename {
    fn new(library: Entity<Library>, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Rename {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let field = text_field(cx, |f| f.name("Name"));
        field.update(cx, |f, cx| f.set_text(stem, cx));
        cx.subscribe_in(&field, window, |r, _, e: &TextFieldEvent, window, cx| match e {
            TextFieldEvent::Entered => r.done(true, window, cx),
            _ => {
                r.error = None;
                cx.notify();
            }
        })
        .detach();
        field.read(cx).focus(window);
        Rename { library, path, field, error: None }
    }

    fn done(&mut self, rename: bool, window: &mut Window, cx: &mut Context<Self>) {
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
                    cx.notify();
                    return;
                }
                if let Err(e) = std::fs::rename(&self.path, &to) {
                    self.error = Some(e.to_string());
                    cx.notify();
                    return;
                }
                let _ = &self.library;
            }
        }
        window.remove_window();
    }
}

impl Render for Rename {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (ok, cancel) = (cx.entity(), cx.entity());
        let p = theme::palette(cx);
        let bw = olo_ui::widgets::buttons::button_width(window, "Rename", false).max(olo_ui::widgets::buttons::button_width(window, "Cancel", false));
        div()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::STACK))
            .on_key_down(cx.listener(|r, e: &gpui::KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    r.done(false, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(TextControl::new(self.field.clone()))
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
                    .child(Button::new("Cancel").min_width(bw).on_click(move |window, cx| cancel.update(cx, |r, cx| r.done(false, window, cx))))
                    .child(Button::new("Rename").default(true).min_width(bw).on_click(move |window, cx| ok.update(cx, |r, cx| r.done(true, window, cx)))),
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
    olo_ui::run(APP_ID, move |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let list_model = ListModel::new(cx, false);
        list_model.update(cx, |l, _| l.state.sort = Some((4, false)));
        let search = text_field(cx, |f| f.name("Search").hint("Search"));
        let proxy = olo_ui::proxy(cx);
        let watch = olo_ui::watch::watch([folder()], move |_| proxy.send(Msg::Changed));
        let library = cx.new(|cx| {
            cx.subscribe(&search, |l: &mut Library, _, _: &TextFieldEvent, cx| {
                l.refilter(cx);
                cx.notify();
            })
            .detach();
            Library { all: Vec::new(), shown: Vec::new(), pictures: HashMap::new(), asked: HashMap::new(), dropped: Vec::new(), list: list_model, search, _watch: watch, scale: 0.0 }
        });
        let messages = library.clone();
        olo_ui::message_handler(cx, move |m, cx| {
            if m.downcast_ref::<instance::Launch>().is_some() {
                olo_ui::raise(None::<gpui::AnyWindowHandle>, cx);
                return;
            }
            if let Ok(m) = m.downcast::<Msg>() {
                messages.update(cx, |l, cx| l.message(*m, cx));
            }
        });
        library.update(cx, |l, cx| l.message(Msg::Changed, cx));
        let view = library.clone();
        olo_ui::open_window(cx, WindowOptions::new("Recordings", 760.0, 520.0).min_size(560.0, 360.0).keep_size("recordings"), move |_, _| view);
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
