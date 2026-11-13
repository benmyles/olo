//! Viewer: a window for any file Quick Look can preview (Tracker's pop-out
//! button opens one), and a PDF editor: fill in forms, add text, draw,
//! sign, then Save (appended to the original as an incremental update),
//! Save As, or Export a flattened copy. Pictures can be rotated, flipped,
//! cropped and resized (olo-preview's `edit`), then saved. On olo-kit:
//! each document is its window's own view (`Doc`), told what its workers
//! find; the app's looper (`Hub`) knows which files are open, follows
//! them on disk and opens the windows.

mod crop;
mod pdfdoc;
mod pdftext;
mod pdfview;
mod pdfwork;
mod signature;
mod video_edit;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use olo_kit::{
    Bounds, Child, ClickEvent, Corner, Cx, Element, ExternalPaths, FocusHandle, FontWeight, Gamut, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu, MenuBar, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, RenderImage, Styled as _, View, ViewRef, WindowCx, WindowHandle, WindowOptions, agent, canvas, div,
    file_panel::{self, FilePanel, PanelResult},
    files::{self, Kind},
    instance::{self, Launch},
    metrics as m, point, px, size,
    style::{self, Align},
    theme,
    widgets::{Button, CheckBox, TextControl, TextField, TextFieldEvent, Unsaved, ask_to_save, open_alert, text_field},
};
use olo_preview::{
    Body, Item,
    edit::{self, EditEvent, Editor, Op},
    kit::{PreviewEvent, PreviewView, VideoZoom},
};
use serde::{Deserialize, Serialize};

use crop::CropFrame;
use pdfview::{Fit, PdfView, Tool};
use pdfwork::Event;
use signature::{Pad, Signature};

const APP_ID: &str = "olo.viewer";

/// What a window asks of the app (through the proxy, to its looper).
enum ToHub {
    /// A file to show in a window of its own (or its window forward).
    Open(PathBuf),
    /// A window shows this file now (another than it opened with).
    Showing(u64, PathBuf),
    Closed(u64),
    /// Open files, or the folders they are in, changed on disk.
    OnDisk(Vec<PathBuf>),
    /// A look following a file panel (`--follow`): the file to show now.
    LookAt(PathBuf),
    /// The signatures kept changed: every window's menus say them again.
    Signatures,
    /// Quit: every window asked to close.
    Quit,
}

/// What a document's window is told: by its workers, and by the app.
pub(crate) enum DocMsg {
    Pdf(Event),
    Stat(PathBuf, Option<Item>),
    Siblings(PathBuf, Vec<PathBuf>),
    Edit(EditEvent),
    OnDisk(Vec<PathBuf>),
    /// Another file in this window (a look's panel chose it), asking
    /// about unsaved changes first.
    LookAt(PathBuf),
    /// A video trimmed (`Doc::trim_command`): where the trimmed copy
    /// went, or why not.
    Trimmed(Result<PathBuf, String>),
    /// The video editor's (`video_edit`): a picture for its timeline
    /// (which, the scale it's made for, it), the waveform's peaks, how far
    /// an export is, and what it made (with the edits it made it of).
    EditPicture(usize, f32, Arc<RenderImage>),
    EditPeaks(Option<Vec<f32>>),
    ExportProgress(f32),
    Exported(Result<PathBuf, String>, video_edit::Edits),
    /// A file panel of this window's answered.
    Chosen(Purpose, PathBuf),
    /// Asked to close (Quit), as its close box asks.
    Leave,
    /// Opened again with `--edit`: its video's editor.
    EditVideo,
    /// The signatures changed.
    Signatures,
    /// (A drag of the file out of the window ended.)
    Dragged,
}

enum Content {
    /// Waiting for the file's details.
    Loading,
    Pdf(Child<PdfView>),
    Other(Child<PreviewView>),
    Failed(String),
}

/// What to do once the document is saved (or its changes discarded).
#[derive(Clone, Debug, PartialEq)]
enum After {
    Nothing,
    Close,
    Open(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Purpose {
    Open,
    SaveAs,
    Export,
}

/// Editing a picture: its steps, and the frame while cropping.
struct ImageEdit {
    editor: Editor,
    crop: Option<CropFrame>,
}

/// What every window of the app knows.
struct Shared {
    /// Started with `--look` (a file panel's Space): a look at a file,
    /// its own process, put away with Space or Escape, as Quick Look.
    look: bool,
    /// The look follows the panel that asked for it (`--follow`): the
    /// arrows ask it for the file before or after (`step -1`, `step 1` on
    /// stdout), and it says what to show (a path a line on stdin), so the
    /// look steps through the panel's list as the panel's selection does.
    follow: bool,
    signatures: Mutex<Vec<Signature>>,
    /// Each window's "unsaved changes, or a save under way" (an upgrade
    /// waits while any says so).
    busy: Mutex<Vec<Weak<AtomicBool>>>,
}

impl Shared {
    fn signatures(&self) -> std::sync::MutexGuard<'_, Vec<Signature>> {
        self.signatures.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn sign_menu(&self, title: &str) -> Menu {
        let signatures = self.signatures();
        let mut sign = Menu::new(title);
        for (k, s) in signatures.iter().enumerate() {
            sign = sign.item(signature::label(s, k), format!("sign:{k}"));
        }
        sign = sign.separator().item("New Signature…", "new-signature");
        if !signatures.is_empty() {
            let mut del = Menu::new("Delete Signature");
            for (k, s) in signatures.iter().enumerate() {
                del = del.item(signature::label(s, k), format!("unsign:{k}"));
            }
            sign = sign.submenu(del);
        }
        sign
    }
}

/// The app's looper: the windows open and the files they show, followed
/// as they change on disk.
struct Hub {
    docs: Vec<Open>,
    shared: Arc<Shared>,
    /// The window a look opens over (`--over=HANDLE`, the file panel's,
    /// exported: xdg-foreign).
    over: Option<String>,
    /// The open files and their folders, followed as they change: a file
    /// another program changed is shown again unless it has changes here,
    /// and Previous / Next know new files.
    watch: Option<olo_kit::watch::Watch>,
    next: u64,
}

struct Open {
    id: u64,
    path: PathBuf,
    window: WindowHandle<Doc>,
}

struct Doc {
    id: u64,
    shared: Arc<Shared>,
    /// Unsaved, or being saved (as `body` last found; `Shared::busy`).
    busy_said: Arc<AtomicBool>,
    path: PathBuf,
    content: Content,
    /// The files beside it, for Previous / Next.
    siblings: Vec<PathBuf>,
    title: String,
    sized: bool,
    after_save: After,
    /// The screen is kept awake (a video is playing).
    awake: bool,
    /// The window is full screen (as last drawn): its menu bar hidden,
    /// a picture or a video all there is.
    fullscreen: bool,
    /// What the window draws in (asked for as what it shows needs).
    gamut: Gamut,
    edit: Option<ImageEdit>,
    /// The file as last opened or saved here (when it was written, how
    /// long), to tell another program's change from our own save.
    on_disk: Option<(std::time::SystemTime, u64)>,
    focus: FocusHandle,
    /// A press on the picture while cropping, and whether it has dragged.
    crop_press: Option<bool>,
    /// A video being trimmed into a copy, on a worker.
    trimming: bool,
    /// A video being edited (`video_edit`).
    video_edit: Option<video_edit::VideoEdit>,
    /// Opened to be edited (`--edit`): the editor as soon as it shows.
    pending_edit: bool,
    /// How fast its sound or video plays (View ▸ Playback Speed): kept
    /// as it goes to the file before or after, and over an upgrade.
    speed: f64,
}

/// The files open, kept for an upgrade.
#[derive(Serialize, Deserialize)]
struct Saved {
    files: Vec<PathBuf>,
    /// Each file's playback speed (1 for one that plays nothing).
    #[serde(default)]
    speeds: Vec<f64>,
}

/// What one window keeps (`View::save_state`).
#[derive(Serialize, Deserialize)]
struct SavedDoc {
    file: PathBuf,
    speed: f64,
}

fn is_pdf(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// When a file was last written and how long it is.
fn stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    std::fs::metadata(path).ok().map(|m| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len()))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Files Viewer can step through: everything but folders and hidden
/// files, by name.
fn list_siblings(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.path())
        .filter(|p| !matches!(files::kind_for(p, false, 0o644), Kind::Folder | Kind::Executable | Kind::Application))
        .collect();
    v.sort_by_key(|p| file_name(p).to_lowercase());
    v
}

/// The screen's size the window is on (points), for sizing it to what
/// it shows.
fn screen_of(cx: &WindowCx) -> Option<(f32, f32)> {
    let size = |b: &Bounds<Pixels>| (f32::from(b.size.width), f32::from(b.size.height));
    cx.display_bounds().map(|b| size(&b)).or_else(screen)
}

/// The smallest screen (a window not on one yet could open on any).
pub(crate) fn screen() -> Option<(f32, f32)> {
    olo_kit::displays().iter().map(|b| (f32::from(b.size.width), f32::from(b.size.height))).min_by(|a, b| (a.0 * a.1).total_cmp(&(b.0 * b.1)))
}

/// A number for each thing previewed (what loads for one before isn't
/// taken for the next).
fn next_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn to_hub(message: ToHub) {
    olo_kit::proxy().send(message);
}

impl Hub {
    /// `path` in a window of its own (`edit`: its video's editor;
    /// `speed`: how fast it plays, kept over an upgrade), or its window
    /// brought forward.
    fn open_file(&mut self, path: PathBuf, edit: bool, speed: Option<f64>) {
        self.docs.retain(|d| d.window.is_open());
        if let Some(doc) = self.docs.iter().find(|d| d.path == path) {
            olo_kit::raise(Some(&doc.window));
            if edit {
                doc.window.messenger().send(DocMsg::EditVideo);
            }
            return;
        }
        let title = file_name(&path);
        let min_w = if is_pdf(&path) { pdfview::MIN_WIDTH } else { 360.0 };
        let mut options = WindowOptions::new(title, 720.0, 560.0).min_size(min_w, 240.0);
        // A look floats over the panel that asked, as Quick Look does:
        // never a tile or a tab.
        if self.shared.look {
            options = options.dialog();
            if let Some(handle) = &self.over {
                options = options.foreign_parent(handle.clone());
            }
        }
        self.next += 1;
        let (id, shared, opened) = (self.next, self.shared.clone(), path.clone());
        let window = olo_kit::open_window(options, move |cx| Doc::new(id, shared, opened, edit, speed.unwrap_or(1.0), cx));
        self.docs.push(Open { id, path, window });
        self.follow_files();
    }

    /// Follow the open files and their folders (as they are now).
    fn follow_files(&mut self) {
        let mut paths: Vec<PathBuf> = Vec::new();
        for d in &self.docs {
            paths.push(d.path.clone());
            paths.extend(d.path.parent().map(Path::to_path_buf));
        }
        paths.sort();
        paths.dedup();
        match &self.watch {
            Some(w) => w.set(paths),
            None => self.watch = olo_kit::watch::watch(paths, |changed| to_hub(ToHub::OnDisk(changed.to_vec()))),
        }
    }

    /// Open…, with no document to ask from (started with no file): the
    /// app ends with the panel if nothing is chosen.
    fn open_panel(&self) {
        let dir = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        file_panel::show("Open", FilePanel::open(dir), |answer| {
            if let PanelResult::Chosen(path) = answer.result {
                to_hub(ToHub::Open(path));
            }
        });
    }

    fn handle_args(&mut self, args: &[String], cwd: Option<&Path>) {
        let mut opened = false;
        // `--edit`: a video opened in its editor (Screenshot's card's Edit).
        let edit = args.iter().any(|a| a == "--edit");
        for a in args.iter().filter(|a| !a.starts_with("--")) {
            let p = PathBuf::from(a);
            let p = match (p.is_absolute(), cwd) {
                (false, Some(cwd)) => cwd.join(p),
                _ => p,
            };
            self.open_file(p, edit, None);
            opened = true;
        }
        self.docs.retain(|d| d.window.is_open());
        if !opened && self.docs.is_empty() {
            self.open_panel();
        }
    }

    fn tell_all(&mut self, message: impl Fn() -> DocMsg) {
        self.docs.retain(|d| d.window.is_open());
        for d in &self.docs {
            d.window.messenger().send(message());
        }
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_args(&launch.args, launch.cwd.as_deref()),
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<ToHub>() else { return };
        match *msg {
            ToHub::Open(path) => self.open_file(path, false, None),
            ToHub::Showing(id, path) => {
                if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
                    d.path = path;
                }
                self.follow_files();
            }
            ToHub::Closed(id) => {
                self.docs.retain(|d| d.id != id);
                self.follow_files();
            }
            ToHub::OnDisk(paths) => self.tell_all(|| DocMsg::OnDisk(paths.clone())),
            // The look's one window, on the file the panel has selected now.
            ToHub::LookAt(path) => {
                if let Some(d) = self.docs.first() {
                    d.window.messenger().send(DocMsg::LookAt(path));
                }
            }
            ToHub::Signatures => self.tell_all(|| DocMsg::Signatures),
            ToHub::Quit => self.tell_all(|| DocMsg::Leave),
        }
    }
}

impl Doc {
    fn new(id: u64, shared: Arc<Shared>, path: PathBuf, edit: bool, speed: f64, cx: &mut Cx<Self>) -> Doc {
        let busy_said = Arc::new(AtomicBool::new(false));
        {
            let mut busy = shared.busy.lock().unwrap_or_else(|e| e.into_inner());
            busy.retain(|b| b.strong_count() > 0);
            busy.push(Arc::downgrade(&busy_said));
        }
        let mut doc = Doc {
            id,
            shared,
            busy_said,
            path: path.clone(),
            content: Content::Loading,
            siblings: Vec::new(),
            title: String::new(),
            sized: false,
            after_save: After::Nothing,
            awake: false,
            fullscreen: false,
            gamut: Gamut::Srgb,
            edit: None,
            trimming: false,
            video_edit: None,
            pending_edit: edit,
            speed,
            on_disk: None,
            focus: FocusHandle::new(),
            crop_press: None,
        };
        doc.load(path, cx);
        doc
    }

    /// (Re)load `path` into this window.
    fn load(&mut self, path: PathBuf, cx: &mut Cx<Self>) {
        // (The next file plays at the speed this one did.)
        if let Content::Other(p) = &self.content {
            self.speed = p.read(cx).chosen_rate();
        }
        self.on_disk = stamp(&path);
        self.path = path.clone();
        self.sized = false;
        self.edit = None;
        self.title = file_name(&path);
        self.set_title(cx);
        // (sRGB again till a wide photo shows.)
        self.draw_in(Gamut::Srgb, false, cx);
        let to = cx.messenger();
        if is_pdf(&path) {
            let sending = to.clone();
            let tx = pdfwork::start(path.clone(), move |ev| {
                sending.send(DocMsg::Pdf(ev));
            });
            let pdf = cx.new_view(|_| PdfView::new(tx));
            // (The tool bar says its page and zoom.)
            cx.subscribe(&pdf, |_: &mut Doc, _: &pdfview::Changed, cx| cx.invalidate());
            self.content = Content::Pdf(pdf);
        } else {
            self.content = Content::Loading;
            let (p, sending) = (path.clone(), to.clone());
            std::thread::spawn(move || {
                let item = Item::stat(&p);
                sending.send(DocMsg::Stat(p, item));
            });
        }
        std::thread::spawn(move || {
            let s = list_siblings(&path);
            to.send(DocMsg::Siblings(path, s));
        });
        cx.invalidate();
    }

    /// Draw the window in `gamut` from now on (Display P3 while it shows
    /// a photo wider than sRGB, else sRGB; `only_on_hdr`: only on a
    /// display whose HDR is on). Whether it does.
    fn draw_in(&mut self, gamut: Gamut, only_on_hdr: bool, cx: &mut Cx<Self>) -> bool {
        if self.gamut == gamut {
            return true;
        }
        let drawn = cx.set_gamut(gamut, only_on_hdr);
        if drawn {
            self.gamut = gamut;
        }
        drawn
    }

    /// The window drawn for what its preview shows: for a picture in
    /// `gamut`; and with `hdr_video` (a preview playing HDR video), in
    /// HDR10 when the window is on a display whose HDR is on and can draw
    /// in it, the video's pictures then HDR10's; else as the picture is
    /// (P3 for a wide one), the video's mapped for SDR.
    fn set_video_gamut(&mut self, gamut: Gamut, hdr_video: bool, cx: &mut Cx<Self>) {
        let hdr = hdr_video && self.draw_in(Gamut::Hdr10, true, cx);
        if hdr_video && let Content::Other(p) = &self.content {
            p.read(cx).draw_video_hdr(hdr);
        }
        if !hdr {
            // (Wide still: an HDR video's pictures are BT.2020's.)
            let gamut = if hdr_video || gamut == Gamut::Hdr10 { Gamut::DisplayP3 } else { gamut };
            self.draw_in(gamut, false, cx);
        }
    }

    /// The window's title, and the file it shows (its tab's Open in
    /// Tracker).
    fn set_title(&self, cx: &mut Cx<Self>) {
        cx.set_window_title(self.title.clone());
        agent::set_path(cx, Some(self.path.clone()));
    }

    fn stat(&mut self, path: PathBuf, item: Option<Item>, cx: &mut Cx<Self>) {
        if self.path != path {
            return;
        }
        match item {
            Some(item) => {
                let (speed, generation) = (self.speed, next_generation());
                let preview = cx.new_view(|cx| {
                    let mut p = PreviewView::new(item, generation, true, cx);
                    p.set_playback_rate(speed, cx);
                    p
                });
                cx.subscribe(&preview, |d: &mut Doc, e: &PreviewEvent, cx| d.preview_event(e, cx));
                // Dragging the picture out where it doesn't pan: the file.
                let (doc, image) = (cx.view_ref(), preview.read(cx).image_view().downgrade());
                image.update(cx, move |v, _| v.on_drag_out(move |cx| doc.update(cx, |d, cx| d.drag_out(cx))));
                self.content = Content::Other(preview);
            }
            None => self.content = Content::Failed(format!("“{}” can't be found.", file_name(&path))),
        }
        cx.invalidate();
    }

    /// What the preview told.
    fn preview_event(&mut self, e: &PreviewEvent, cx: &mut Cx<Self>) {
        // Each picture of a video: only the editor follows (its playhead,
        // and what's cut passed over).
        if *e == PreviewEvent::Played {
            return self.skip_cuts(cx);
        }
        let Content::Other(prev) = &self.content else { return };
        // A photo wider than sRGB shown whole: the window draws in
        // Display P3 while it shows one (`Gamut`). A video wider than
        // sRGB (BT.2020, P3) too, from its first picture; an HDR one in
        // HDR10 on a display whose HDR is on.
        if matches!(*e, PreviewEvent::Shown | PreviewEvent::Gamut) {
            let (gamut, hdr) = {
                let p = prev.read(cx);
                (p.gamut(cx), p.video_is_hdr())
            };
            self.set_video_gamut(gamut, hdr, cx);
        }
        if *e == PreviewEvent::Shown && !self.sized {
            self.sized = true;
            self.resize_to(cx);
        }
        // Opened to be edited (`--edit`): the editor, once the video
        // shows.
        let Content::Other(prev) = &self.content else { return };
        if *e == PreviewEvent::Shown && self.pending_edit && prev.read(cx).is_video() {
            self.pending_edit = false;
            self.start_video_edit(cx);
        }
        cx.invalidate();
    }

    /// The picture dragged out of the window: the file.
    fn drag_out(&mut self, cx: &mut Cx<Self>) {
        let cropping = self.edit.as_ref().is_some_and(|e| e.crop.is_some());
        if !cropping {
            olo_kit::dnd::start(cx, file_drag(&self.path), |_| DocMsg::Dragged);
        }
    }

    /// The window's size for what the preview shows, within its screen.
    fn resize_to(&self, cx: &mut Cx<Self>) {
        let Content::Other(prev) = &self.content else { return };
        let s = screen_of(cx).map(|(w, h)| (w as u32, h as u32));
        let (w, h) = prev.read(cx).ideal_size(s, olo_kit::menu::BAR_HEIGHT);
        cx.resize_window(size(px(w as f32), px(h as f32)));
    }

    fn pdf_event(&mut self, ev: Event, cx: &mut Cx<Self>) {
        if !matches!(self.content, Content::Pdf(_)) {
            return;
        }
        let mut error = None;
        let mut after = None;
        match &ev {
            Event::Failed(e) => error = Some(e.clone()),
            Event::SaveFailed(e) => {
                error = Some(format!("“{}” couldn't be saved. {e}", file_name(&self.path)));
                self.after_save = After::Nothing;
            }
            Event::Saved { to, adopted } => {
                if adopted.is_some() || *to == self.path {
                    self.on_disk = stamp(to);
                }
                if adopted.is_some() && self.path != *to {
                    self.path = to.clone();
                    to_hub(ToHub::Showing(self.id, to.clone()));
                }
                after = Some(std::mem::replace(&mut self.after_save, After::Nothing));
            }
            _ => {}
        }
        let Content::Pdf(v) = &self.content else { return };
        let opened = v.update(cx, |v, cx| v.event(ev, cx));
        if opened && !self.sized {
            self.sized = true;
            let s = screen_of(cx);
            let (w, h) = v.update(cx, |p, _| p.ideal_size(s, olo_kit::menu::BAR_HEIGHT + m::TOOLBAR_HEIGHT + 1.0));
            cx.resize_window(size(px(w), px(h)));
        }
        let loaded = v.read(cx).loaded();
        if let Some(e) = &error
            && !loaded
        {
            self.content = Content::Failed(e.clone());
            error = None;
        }
        if let Some(e) = error {
            self.error(e, cx);
        }
        if let Some(a) = after {
            self.finish(a, cx);
        }
        cx.invalidate();
    }

    fn edit_event(&mut self, ev: EditEvent, cx: &mut Cx<Self>) {
        let Some(e) = &mut self.edit else { return };
        e.editor.handle(&ev);
        match ev {
            EditEvent::Shown { image, full, note, steps, .. } => {
                if steps == e.editor.ops().len()
                    && let Content::Other(prev) = &self.content
                {
                    prev.update(cx, |p, cx| p.show_edited(image, full, note, cx));
                }
            }
            EditEvent::Saved(to) => {
                if to == self.path {
                    self.on_disk = stamp(&to);
                }
                let after = std::mem::replace(&mut self.after_save, After::Nothing);
                let next = if to != self.path { After::Open(to) } else { after };
                self.finish(next, cx);
            }
            EditEvent::Failed(m) => {
                self.after_save = After::Nothing;
                self.error(m, cx);
            }
        }
        self.update_title(cx);
        cx.invalidate();
    }

    fn on_disk_changed(&mut self, paths: &[PathBuf], cx: &mut Cx<Self>) {
        // A file beside it came or went: Previous / Next know.
        if self.path.parent().is_some_and(|dir| paths.iter().any(|p| p == dir)) {
            let (to, path) = (cx.messenger(), self.path.clone());
            std::thread::spawn(move || {
                let s = list_siblings(&path);
                to.send(DocMsg::Siblings(path, s));
            });
        }
        // Written by another program (not our own save), and nothing here
        // unsaved: shown again as it is now (the window keeps its size).
        if !paths.contains(&self.path) {
            return;
        }
        let now = stamp(&self.path);
        if now.is_some() && now != self.on_disk && !self.busy(cx) {
            self.load(self.path.clone(), cx);
            self.sized = true;
        }
    }

    /// Unsaved, or being saved.
    fn busy(&self, cx: &WindowCx) -> bool {
        matches!(&self.content, Content::Pdf(v) if v.read(cx).dirty() || v.read(cx).saving) || self.edit.as_ref().is_some_and(|e| e.editor.dirty() || e.editor.saving)
    }

    fn dirty(&self, cx: &WindowCx) -> bool {
        matches!(&self.content, Content::Pdf(v) if v.read(cx).dirty()) || self.edit.as_ref().is_some_and(|e| e.editor.dirty())
    }

    fn update_title(&mut self, cx: &mut Cx<Self>) {
        let title = format!("{}{}", file_name(&self.path), if self.dirty(cx) { " — Edited" } else { "" });
        if title != self.title {
            self.title = title;
            self.set_title(cx);
        }
    }

    /// Save the document in place; `after` runs once it's written.
    fn save(&mut self, after: After, cx: &mut Cx<Self>) {
        if let Some(e) = &mut self.edit {
            // A format we can't write (HEIC, AVIF, …): Save As instead.
            if edit::save_format(&self.path).is_none() {
                return self.ask_panel(Purpose::SaveAs, cx);
            }
            self.after_save = after;
            e.editor.save(self.path.clone());
            cx.invalidate();
            return;
        }
        let Content::Pdf(v) = &self.content else { return };
        if !v.read(cx).editable {
            return self.error("This PDF can't be edited, so it can't be saved.".into(), cx);
        }
        self.after_save = after;
        let path = self.path.clone();
        v.update(cx, |v, cx| v.save(path, false, true, cx));
    }

    pub(crate) fn error(&self, message: String, cx: &mut Cx<Self>) {
        let _ = open_alert(cx, "Viewer", "warning", &message, &["OK"], |_, _| {});
    }

    /// A file panel for this document: what's chosen comes back to it
    /// (`DocMsg::Chosen`).
    fn ask_panel(&self, purpose: Purpose, cx: &mut Cx<Self>) {
        let path = &self.path;
        let dir = path.parent().map_or_else(|| std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from), Path::to_path_buf);
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let (title, panel) = match purpose {
            Purpose::Open => ("Open", FilePanel::open(dir)),
            Purpose::SaveAs if !is_pdf(path) => {
                // A picture: its own format if we write it, else PNG.
                let ext = Some(path.as_path()).filter(|p| edit::save_format(p).is_some()).and_then(Path::extension).map_or("png".into(), |e| e.to_string_lossy().to_lowercase());
                ("Save As", FilePanel::save(dir, format!("{stem} copy.{ext}")).types(&["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"]))
            }
            Purpose::SaveAs => ("Save As", FilePanel::save(dir, format!("{stem} copy.pdf")).types(&["pdf"])),
            Purpose::Export => ("Export Flattened PDF", FilePanel::save(dir, format!("{stem} (flattened).pdf")).types(&["pdf"])),
        };
        let to = cx.messenger();
        file_panel::show(title, panel, move |answer| {
            if let PanelResult::Chosen(path) = answer.result {
                to.send(DocMsg::Chosen(purpose, path));
            }
        });
    }

    /// What a file panel of this document's chose.
    fn chosen(&mut self, purpose: Purpose, path: PathBuf, cx: &mut Cx<Self>) {
        match purpose {
            Purpose::Open => to_hub(ToHub::Open(path)),
            Purpose::SaveAs if !is_pdf(&self.path) => {
                // A picture: written there (with any edits), then shown.
                if edit::save_format(&path).is_none() {
                    return self.error("Pictures can be saved as PNG, JPEG, WebP, BMP, TIFF or GIF.".into(), cx);
                }
                self.ensure_editor(cx);
                if let Some(e) = &mut self.edit {
                    e.editor.save(path);
                }
                cx.invalidate();
            }
            Purpose::SaveAs | Purpose::Export => {
                if let Content::Pdf(v) = &self.content {
                    let flatten = purpose == Purpose::Export;
                    self.after_save = After::Nothing;
                    v.update(cx, |v, cx| v.save(path, flatten, !flatten, cx));
                }
            }
        }
    }

    /// Close or move on, asking about unsaved changes.
    fn leave(&mut self, after: After, cx: &mut Cx<Self>) {
        if !self.dirty(cx) {
            return self.finish(after, cx);
        }
        let (name, doing) = (file_name(&self.path), if after == After::Close { "closing" } else { "moving on" });
        let doc = cx.view_ref();
        let _ = ask_to_save(cx, "Viewer", &name, doing, move |choice, cx| {
            let (choice, after) = (*choice, after.clone());
            doc.update(cx, move |d, cx| match choice {
                // Don't Save: closing or loading another file drops the edits.
                Some(Unsaved::DontSave) => d.finish(after, cx),
                Some(Unsaved::Save) => d.save(after, cx),
                None => {}
            })
        });
    }

    fn finish(&mut self, after: After, cx: &mut Cx<Self>) {
        match after {
            After::Nothing => {}
            // (The app hears as the window goes: `window_closing`.)
            After::Close => cx.close_window(),
            After::Open(p) => {
                to_hub(ToHub::Showing(self.id, p.clone()));
                self.load(p, cx);
            }
        }
    }

    /// Previous (-1) or next (+1) file in the folder.
    fn step(&mut self, d: isize, cx: &mut Cx<Self>) {
        let Some(i) = self.siblings.iter().position(|p| *p == self.path) else { return };
        let j = i as isize + d;
        if j < 0 || j as usize >= self.siblings.len() {
            return;
        }
        let next = self.siblings[j as usize].clone();
        self.leave(After::Open(next), cx);
    }

    /// A picture being edited: its editor, made as the first step is.
    fn ensure_editor(&mut self, cx: &mut Cx<Self>) {
        if self.edit.is_some() {
            return;
        }
        let Content::Other(prev) = &self.content else { return };
        let Some(Body::Image { full, .. }) = prev.read(cx).body() else { return };
        let to = cx.messenger();
        let editor = Editor::new(self.path.clone(), (full[0] as u32, full[1] as u32), move |ev| {
            to.send(DocMsg::Edit(ev));
        });
        self.edit = Some(ImageEdit { editor, crop: None });
    }

    fn picture(&self, cx: &WindowCx) -> bool {
        matches!(&self.content, Content::Other(prev) if prev.read(cx).is_image()) && edit::editable(&self.path)
    }

    fn menus(&self, cx: &WindowCx) -> Vec<Menu> {
        let pdf = match &self.content {
            Content::Pdf(v) => Some(v.read(cx)),
            _ => None,
        };
        let editable = pdf.is_some_and(|v| v.editable && v.loaded());
        let pos = self.siblings.iter().position(|p| *p == self.path);
        let picture = self.picture(cx);
        let editor = self.edit.as_ref().map(|e| &e.editor);
        let mut file = Menu::new("File").item("Open…", "open").key("o").separator();
        if let Some(v) = pdf {
            file = file
                .item("Save", "save")
                .key("s")
                .enabled(editable && v.dirty() && !v.saving)
                .item("Save As…", "save-as")
                .shift_key("s")
                .enabled(editable)
                .item("Export Flattened PDF…", "export")
                .enabled(editable)
                .separator();
        } else if picture {
            let dirty = editor.is_some_and(|e| e.dirty() && !e.saving);
            file = file.item("Save", "save").key("s").enabled(dirty).item("Save As…", "save-as").shift_key("s").item("Revert to Saved", "revert").enabled(dirty).separator();
        }
        file = file.item(olo_kit::tracker::LABEL, olo_kit::tracker::COMMAND);
        // A film, on a television (Ben, 2026-11-12: "integrated airplay
        // from relevant app menus"): the receiver fetches and plays the
        // file itself (`olo-airplay play`).
        if matches!(&self.content, Content::Other(prev) if prev.read(cx).is_video()) {
            file = file.submenu(Menu::airplay("Play On", &olo_kit::airplay::paired(), ""));
        }
        file = file.separator().item("Close", "close").key("w").item("Quit", "quit").key("q");
        let mut menus = vec![file];
        // A video: Trim… (and while trimming, Cancel and Trim there too).
        if let Content::Other(prev) = &self.content
            && prev.read(cx).is_video()
        {
            let trimming = prev.read(cx).trimming().is_some();
            let menu = match &self.video_edit {
                // Editing: its steps, undone and done again, and its tools.
                Some(e) => {
                    let busy = e.exporting.is_some();
                    olo_kit::undo::edit_items_as(Menu::new("Edit"), e.undo_name().filter(|_| !busy), e.redo_name().filter(|_| !busy), ("edit-undo", "edit-redo"))
                        .separator()
                        .item("Cut", "edit-cut")
                        .key("x")
                        .enabled(!busy && (e.selection.is_some() || e.mark.is_some()))
                        .item("Add Mark", "edit-add-mark")
                        .key("k")
                        .enabled(!busy)
                        .item("Delete Mark", "edit-delete-mark")
                        .enabled(!busy && e.mark.is_some())
                        .separator()
                        .item(if e.cropping.is_some() { "Done Cropping" } else { "Crop…" }, "edit-crop")
                        .enabled(!busy)
                        .item("Reset Crop", "edit-crop-reset")
                        .enabled(!busy && e.now.crop.is_some())
                        .item(if e.now.mute { "Unmute" } else { "Mute" }, "edit-mute")
                        .enabled(!busy)
                        .separator()
                        .item("Export…", "edit-export")
                        .shift_key("e")
                        .enabled(!busy)
                        .item("Done Editing", "edit-done")
                        .key("e")
                }
                None => Menu::new("Edit")
                    .item("Edit Video…", "edit-video")
                    .key("e")
                    .enabled(!trimming && !self.trimming)
                    .separator()
                    .item("Trim…", "trim")
                    .key("t")
                    .enabled(!trimming && !self.trimming)
                    .item("Trim to Selection", "trim-save")
                    .enabled(trimming)
                    .item("Cancel Trimming", "trim-cancel")
                    .enabled(trimming),
            };
            menus.push(menu);
        }
        if let Content::Other(prev) = &self.content
            && prev.read(cx).has_text()
        {
            let prev = prev.read(cx);
            menus.push(
                Menu::new("Edit")
                    .item("Copy", "copy")
                    .key("c")
                    .enabled(prev.has_text_selection(cx))
                    .item("Select All", "select-all")
                    .key("a")
                    .separator()
                    .item("Find…", "find")
                    .key("f")
                    .item("Find Next", "find-next")
                    .key("g")
                    .enabled(prev.finding(cx))
                    .item("Find Previous", "find-previous")
                    .shift_key("g")
                    .enabled(prev.finding(cx)),
            );
        }
        if picture {
            menus.push(olo_kit::undo::edit_items_named(Menu::new("Edit"), editor.and_then(|e| e.undo_name()), editor.and_then(|e| e.redo_name())));
        }
        if let Some(v) = pdf {
            menus.push(
                olo_kit::undo::edit_items_named(Menu::new("Edit"), v.undo_name(), v.redo_name())
                    .separator()
                    .item("Copy", "copy")
                    .key("c")
                    .enabled(v.has_text_selection())
                    .item("Select All", "select-all")
                    .key("a")
                    .item("Delete", "delete")
                    .enabled(v.has_selection())
                    .separator()
                    .item("Find…", "find")
                    .key("f")
                    .item("Find Next", "find-next")
                    .key("g")
                    .enabled(v.finding())
                    .item("Find Previous", "find-previous")
                    .shift_key("g")
                    .enabled(v.finding()),
            );
            menus.push(
                Menu::new("View")
                    .item("Zoom In", "zoom-in")
                    .key("=")
                    .item("Zoom Out", "zoom-out")
                    .key("-")
                    .item("Actual Size", "zoom-100")
                    .key("0")
                    .separator()
                    .item("Fit Width", "fit-width")
                    .checked(v.fit == Some(Fit::Width))
                    .item("Fit Page", "fit-page")
                    .checked(v.fit == Some(Fit::Page)),
            );
        }
        let full_screen = if self.fullscreen { "Exit Full Screen" } else { "Enter Full Screen" };
        if let Content::Other(prev) = &self.content
            && prev.read(cx).is_image()
        {
            let image = prev.read(cx).image_view().read(cx);
            menus.push(
                Menu::new("View")
                    .item("Zoom In", "zoom-in")
                    .key("=")
                    .item("Zoom Out", "zoom-out")
                    .key("-")
                    .item("Actual Size", "zoom-100")
                    .key("0")
                    .checked(!image.fitted() && (image.scale() - 1.0).abs() < 1e-3)
                    .item("Zoom to Fit", "zoom-fit")
                    .key("9")
                    .checked(image.fitted())
                    .separator()
                    .item(full_screen, "fullscreen")
                    .keys("F"),
            );
        }
        // A video: at most its own size, as big as fits or filling the
        // window, its proportions kept; full screen the same ways (Actual
        // Size there is as big as fits).
        if let Content::Other(prev) = &self.content
            && prev.read(cx).is_video()
        {
            let zoom = prev.read(cx).video_zoom();
            // What decodes it: the default (Preferences ▸ Power ▸ Graphics), or one
            // chosen for it; and what does now.
            let (with, now, gpus) = (prev.read(cx).decode_with(), prev.read(cx).decoding(), prev.read(cx).decoders());
            use olo_preview::video::Device;
            let decode = Menu::new("Decode With")
                .item(format!("Decoding on {}", now.unwrap_or("…")), "")
                .enabled(false)
                .separator()
                .item("Default (Preferences ▸ Power)", "decode-default")
                .checked(with == Device::Default)
                .item("Automatic", "decode-auto")
                .checked(with == Device::Automatic)
                .item("NVIDIA GPU (NVDEC)", "decode-nvidia")
                .checked(with == Device::Nvidia)
                .enabled(gpus.nvidia)
                .item("Intel GPU (VA-API)", "decode-intel")
                .checked(with == Device::Intel)
                .enabled(gpus.intel)
                .item("Processor", "decode-cpu")
                .checked(with == Device::Processor);
            menus.push(
                Menu::new("View")
                    .item("Actual Size", "video-actual")
                    .key("0")
                    .checked(zoom == VideoZoom::Actual)
                    .item("Zoom to Fit", "video-fit")
                    .key("9")
                    .checked(zoom == VideoZoom::Fit)
                    .item("Zoom to Fill", "video-fill")
                    .shift_key("9")
                    .checked(zoom == VideoZoom::Fill)
                    .separator()
                    .submenu(speed_menu(prev.read(cx).playback_rate().unwrap_or(1.0)))
                    .submenu(decode)
                    .separator()
                    .item(full_screen, "fullscreen")
                    .keys("F"),
            );
        }
        // A sound: its speed.
        if let Content::Other(prev) = &self.content
            && !prev.read(cx).is_video()
            && let Some(rate) = prev.read(cx).playback_rate()
        {
            menus.push(Menu::new("View").submenu(speed_menu(rate)));
        }
        if picture {
            let crop = self.edit.as_ref().and_then(|e| e.crop.as_ref());
            menus.push(
                Menu::new("Image")
                    .item("Rotate Left", "rotate-left")
                    .key("l")
                    .item("Rotate Right", "rotate-right")
                    .key("r")
                    .item("Flip Horizontal", "flip-h")
                    .item("Flip Vertical", "flip-v")
                    .separator()
                    .item(if crop.is_some() { "Crop to Selection" } else { "Crop…" }, "crop")
                    .key("k")
                    .enabled(crop.is_none_or(|c| c.sel.is_some()))
                    .item("Adjust Size…", "resize"),
            );
        }
        let mut go = Menu::new("Go")
            .item("Previous File", "prev-file")
            .key("[")
            .enabled(pos.is_some_and(|i| i > 0))
            .item("Next File", "next-file")
            .key("]")
            .enabled(pos.is_some_and(|i| i + 1 < self.siblings.len()));
        // A video's chapters (an Olo recording's marks).
        if let Content::Other(prev) = &self.content {
            let prev = prev.read(cx);
            let chapters = prev.chapters();
            if !chapters.is_empty() {
                let now = prev.position().unwrap_or(0.0);
                go = go
                    .separator()
                    .item("Previous Chapter", "prev-chapter")
                    .enabled(chapters.iter().any(|(at, _)| *at < now - 1.0))
                    .item("Next Chapter", "next-chapter")
                    .enabled(chapters.iter().any(|(at, _)| *at > now + 0.05))
                    .separator();
                for (k, (at, title)) in chapters.iter().enumerate() {
                    let name = if title.is_empty() { format!("Chapter {}", k + 1) } else { title.clone() };
                    go = go.item(format!("{name}  ({})", olo_preview::media::format_duration(*at)), format!("chapter-{k}"));
                }
            }
        }
        if let Some(v) = pdf {
            let n = v.geoms.len();
            go = go
                .separator()
                .item("Previous Page", "prev-page")
                .enabled(v.current_page > 0)
                .item("Next Page", "next-page")
                .enabled(v.current_page + 1 < n)
                .item("First Page", "first-page")
                .item("Last Page", "last-page");
        }
        menus.push(go);
        if let Some(v) = pdf
            && editable
        {
            let mut ink = Menu::new("Ink Colour");
            for (k, (name, rgb)) in pdfview::INKS.iter().enumerate() {
                ink = ink.item(*name, format!("ink:{k}")).checked(v.ink == *rgb);
            }
            let sign = self.shared.sign_menu("Sign");
            menus.push(
                Menu::new("Tools")
                    .item("Select", "tool-select")
                    .key("1")
                    .checked(v.tool == Tool::Select)
                    .item("Add Text", "tool-text")
                    .key("2")
                    .checked(v.tool == Tool::Text)
                    .item("Draw", "tool-draw")
                    .key("3")
                    .checked(v.tool == Tool::Draw)
                    .separator()
                    .submenu(ink)
                    .separator()
                    .submenu(sign),
            );
        }
        menus
    }

    fn run(&mut self, command: &str, cx: &mut Cx<Self>) {
        if olo_kit::airplay::play_from_menu(command, &self.path.to_string_lossy()) {
            return;
        }
        match command {
            "open" => return self.ask_panel(Purpose::Open, cx),
            "save" => return self.save(After::Nothing, cx),
            "save-as" => return self.ask_panel(Purpose::SaveAs, cx),
            "export" => return self.ask_panel(Purpose::Export, cx),
            "close" => return self.leave(After::Close, cx),
            "quit" => return to_hub(ToHub::Quit),
            "prev-file" => return self.step(-1, cx),
            "next-file" => return self.step(1, cx),
            "new-signature" => return self.new_signature(cx),
            c if c == olo_kit::tracker::COMMAND => {
                if let Err(e) = olo_kit::tracker::open(&self.path) {
                    tracing::warn!("couldn't show {} in Tracker: {e}", self.path.display());
                }
                return;
            }
            _ => {}
        }
        if let Some(k) = command.strip_prefix("unsign:").and_then(|k| k.parse::<usize>().ok()) {
            {
                let mut signatures = self.shared.signatures();
                if k < signatures.len() {
                    signatures.remove(k);
                    signature::store(&signatures);
                }
            }
            return to_hub(ToHub::Signatures);
        }
        if matches!(command, "trim" | "trim-cancel" | "trim-save") {
            return self.trim_command(command, cx);
        }
        if command.starts_with("edit-") {
            return self.video_edit_command(command, cx);
        }
        // View ▸ Playback Speed: a speed, or the next one up or down.
        if command.starts_with("speed-")
            && let Content::Other(prev) = &self.content
        {
            prev.update(cx, |p, cx| match command {
                "speed-faster" => {
                    p.step_playback_rate(1, cx);
                }
                "speed-slower" => {
                    p.step_playback_rate(-1, cx);
                }
                c => {
                    if let Some(rate) = olo_preview::kit::speed_command(c) {
                        p.set_playback_rate(rate, cx);
                    }
                }
            });
            cx.invalidate();
            return;
        }
        // View ▸ Decode With: the video decoded there from now on.
        if let Some(d) = command.strip_prefix("decode-")
            && let Content::Other(prev) = &self.content
        {
            let device = match d {
                "default" => olo_preview::video::Device::Default,
                d => olo_preview::video::Device::parse(d).unwrap_or_default(),
            };
            prev.update(cx, |p, cx| p.set_decode_with(device, cx));
            cx.invalidate();
            return;
        }
        // A chapter of the video: where it starts, the one after the
        // position, or before it (within a second of a chapter's start:
        // the one before that, as players do).
        if (command.starts_with("chapter-") || command == "next-chapter" || command == "prev-chapter")
            && let Content::Other(prev) = &self.content
        {
            prev.update(cx, |p, cx| {
                let (chapters, now) = (p.chapters(), p.position().unwrap_or(0.0));
                let to = match command {
                    "next-chapter" => chapters.iter().map(|c| c.0).find(|&at| at > now + 0.05),
                    "prev-chapter" => chapters.iter().map(|c| c.0).rev().find(|&at| at < now - 1.0),
                    _ => command.strip_prefix("chapter-").and_then(|k| k.parse::<usize>().ok()).and_then(|k| chapters.get(k)).map(|c| c.0),
                };
                if let Some(at) = to {
                    p.seek_to(at, cx);
                }
            });
            return;
        }
        if matches!(command, "rotate-left" | "rotate-right" | "flip-h" | "flip-v" | "crop" | "crop-cancel" | "resize" | "revert") || (self.edit.is_some() && matches!(command, "undo" | "redo")) {
            return self.picture_command(command, cx);
        }
        // A text or a formatted document: its text copied, all of it
        // selected, found.
        if let Content::Other(prev) = &self.content
            && prev.read(cx).has_text()
            && matches!(command, "copy" | "select-all" | "find" | "find-next" | "find-previous")
        {
            prev.update(cx, |p, cx| match command {
                "copy" => p.copy(cx),
                "select-all" => p.select_all(cx),
                "find" => p.open_find(cx),
                "find-next" => p.find_step(false, cx),
                _ => p.find_step(true, cx),
            });
            // (Its menu says what's found, and whether there's a selection.)
            cx.invalidate();
            return;
        }
        if command == "fullscreen" {
            cx.toggle_fullscreen();
            cx.invalidate();
            return;
        }
        if let Content::Other(prev) = &self.content
            && let Some(zoom) = match command {
                "video-actual" => Some(VideoZoom::Actual),
                "video-fit" => Some(VideoZoom::Fit),
                "video-fill" => Some(VideoZoom::Fill),
                _ => None,
            }
        {
            prev.update(cx, |p, cx| p.set_video_zoom(zoom, cx));
            cx.invalidate();
            return;
        }
        if let Content::Other(prev) = &self.content {
            prev.update(cx, |p, cx| {
                p.image_view().update(cx, |v, cx| match command {
                    "zoom-in" => v.zoom(Some(true), cx),
                    "zoom-out" => v.zoom(Some(false), cx),
                    "zoom-100" => v.set_scale(1.0, cx),
                    "zoom-fit" => v.zoom(None, cx),
                    _ => {}
                })
            });
            cx.invalidate();
            return;
        }
        let sig = command.strip_prefix("sign:").and_then(|k| k.parse::<usize>().ok()).and_then(|k| self.shared.signatures().get(k).cloned());
        let Content::Pdf(v) = &self.content else { return };
        v.update(cx, |v, cx| match command {
            "undo" => v.undo(cx),
            "redo" => v.redo(cx),
            "delete" => v.delete_selection(cx),
            "copy" => v.copy(cx),
            "select-all" => v.select_all(cx),
            "find" => v.open_find(cx),
            "find-next" => v.find_step(false, cx),
            "find-previous" => v.find_step(true, cx),
            "zoom-in" => v.zoom_step(true, cx),
            "zoom-out" => v.zoom_step(false, cx),
            "zoom-100" => v.set_zoom(1.0, None, cx),
            "fit-width" => v.set_fit(Fit::Width, cx),
            "fit-page" => v.set_fit(Fit::Page, cx),
            "prev-page" => v.go_to_page(v.current_page.saturating_sub(1), cx),
            "next-page" => v.go_to_page(v.current_page + 1, cx),
            "first-page" => v.go_to_page(0, cx),
            "last-page" => v.go_to_page(v.geoms.len().saturating_sub(1), cx),
            "tool-select" => v.set_tool(Tool::Select, cx),
            "tool-text" => v.set_tool(Tool::Text, cx),
            "tool-draw" => v.set_tool(Tool::Draw, cx),
            c if c.starts_with("ink:") => {
                if let Some((_, rgb)) = c[4..].parse::<usize>().ok().and_then(|k| pdfview::INKS.get(k)) {
                    v.ink = *rgb;
                    cx.invalidate();
                }
            }
            _ => {
                if let Some(s) = sig {
                    v.place_signature(s.strokes, s.aspect, cx);
                }
            }
        });
        self.update_title(cx);
        cx.invalidate();
    }

    /// New Signature…: the pad, a window of this one's; what's signed is
    /// kept, and placed on this document.
    fn new_signature(&mut self, cx: &mut Cx<Self>) {
        let (doc, shared) = (cx.view_ref(), self.shared.clone());
        let pad_done = move |sig: Option<Signature>, cx: &mut WindowCx| {
            let Some(sig) = sig else { return };
            {
                let mut signatures = shared.signatures();
                signatures.push(sig.clone());
                signature::store(&signatures);
            }
            to_hub(ToHub::Signatures);
            doc.update(cx, move |d, cx| {
                if let Content::Pdf(v) = &d.content {
                    v.update(cx, |v, cx| v.place_signature(sig.strokes, sig.aspect, cx));
                }
                cx.activate_window();
                cx.invalidate();
            });
        };
        let (w, h) = signature::PAD_SIZE;
        olo_kit::open_window_here(cx, WindowOptions::new("New Signature", w, h).min_size(360.0, 200.0).child(), move |_| Pad::new(pad_done));
    }

    /// Editing a picture (an Image menu or Edit menu command).
    fn picture_command(&mut self, command: &str, cx: &mut Cx<Self>) {
        if !self.picture(cx) {
            return;
        }
        self.ensure_editor(cx);
        let Content::Other(prev) = &self.content else { return };
        let image = prev.read(cx).image_view().downgrade();
        let Some(e) = self.edit.as_mut() else { return };
        match command {
            "rotate-left" => e.editor.push(Op::RotateLeft),
            "rotate-right" => e.editor.push(Op::RotateRight),
            "flip-h" => e.editor.push(Op::FlipHorizontal),
            "flip-v" => e.editor.push(Op::FlipVertical),
            "undo" => e.editor.undo(),
            "redo" => e.editor.redo(),
            "revert" => e.editor.revert(),
            "crop" => match e.crop.take() {
                Some(crop) => {
                    if let Some(op) = crop.op() {
                        e.editor.push(op);
                    }
                    image.update(cx, |v, _| v.set_still(false));
                }
                None => {
                    e.crop = Some(CropFrame::default());
                    image.update(cx, |v, cx| {
                        v.set_still(true);
                        v.zoom(None, cx);
                    });
                }
            },
            "crop-cancel" => {
                e.crop = None;
                image.update(cx, |v, _| v.set_still(false));
            }
            "resize" => {
                let (size, doc) = (e.editor.size(), cx.view_ref());
                olo_kit::open_window_here(cx, WindowOptions::new("Adjust Size", 300.0, 168.0).min_size(300.0, 168.0).child(), move |cx| Resize::new(doc, size, cx));
                return;
            }
            _ => {}
        }
        // Rotating or flipping mid-crop would leave the frame on the old picture.
        if command != "crop" && command != "crop-cancel" && e.crop.take().is_some() {
            image.update(cx, |v, _| v.set_still(false));
        }
        self.update_title(cx);
        cx.invalidate();
    }

    /// Following a panel (`--follow`): the arrows step through the
    /// panel's list (it moves its selection, and says what to show), as
    /// Quick Look's do.
    fn follow_keys(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let key = e.keystroke.key.as_str();
        if !self.shared.follow || e.keystroke.modifiers.modified() || !matches!(key, "up" | "down" | "left" | "right") {
            return;
        }
        // (← and → move a video's or a sound's time: `keys`.)
        if matches!(key, "left" | "right") && matches!(&self.content, Content::Other(p) if p.read(cx).has_media_time()) {
            return;
        }
        use std::io::Write as _;
        let by = if matches!(key, "up" | "left") { -1 } else { 1 };
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "step {by}").and_then(|_| out.flush());
        cx.stop_propagation();
    }

    fn keys(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let key = e.keystroke.key.as_str();
        let plain = !e.keystroke.modifiers.modified();
        // A video or a sound: `>` a speed faster, `<` slower.
        if let Some(by) = olo_preview::kit::speed_key(&e.keystroke)
            && let Content::Other(prev) = &self.content
            && prev.read(cx).playback_rate().is_some()
        {
            prev.update(cx, |p, cx| p.step_playback_rate(by, cx));
            cx.invalidate();
            cx.stop_propagation();
            return;
        }
        // A look: Space or Escape puts it away.
        if self.shared.look && plain && matches!(key, "space" | "escape") {
            self.finish(After::Close, cx);
            cx.stop_propagation();
            return;
        }

        // Editing a video: Delete cuts what's chosen; while cropping,
        // Return takes the frame and Escape puts it away.
        if let Some(e) = self.video_edit.as_mut()
            && plain
        {
            let handled = match key {
                "backspace" | "delete" if e.selection.is_some() || e.mark.is_some() => {
                    self.video_edit_command("edit-cut", cx);
                    true
                }
                "enter" if e.cropping.is_some() => {
                    self.video_edit_command("edit-crop", cx);
                    true
                }
                "escape" if e.cropping.is_some() => {
                    e.cropping = None;
                    cx.invalidate();
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
                return;
            }
        }
        let cropping = self.edit.as_ref().is_some_and(|e| e.crop.is_some());
        if cropping && plain {
            let has_sel = self.edit.as_ref().and_then(|e| e.crop.as_ref()).is_some_and(|c| c.sel.is_some());
            match key {
                "escape" => self.picture_command("crop-cancel", cx),
                "enter" if has_sel => self.picture_command("crop", cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        // A picture or a video: F full screen and back, Escape back.
        if plain && matches!(&self.content, Content::Other(p) if p.read(cx).is_image() || p.read(cx).is_video()) {
            let full = cx.is_fullscreen();
            if key == "f" || (key == "escape" && full) {
                cx.toggle_fullscreen();
                cx.invalidate();
                cx.stop_propagation();
                return;
            }
        }
        match &self.content {
            Content::Other(prev) if plain && prev.read(cx).arrows_free() => {
                // A video or a sound: ← and → move its time by 10 s, ↑ and ↓
                // go to the next file.
                let media = prev.read(cx).has_media_time();
                match key {
                    "left" if media => {
                        prev.update(cx, |p, cx| p.skip(-PreviewView::SKIP_SECS, cx));
                    }
                    "right" if media => {
                        prev.update(cx, |p, cx| p.skip(PreviewView::SKIP_SECS, cx));
                    }
                    "left" | "up" => self.step(-1, cx),
                    "right" | "down" => self.step(1, cx),
                    "space" | "p" => {
                        prev.update(cx, |p, cx| p.toggle_playback(cx));
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }
            Content::Pdf(_) if plain => {
                match key {
                    "pageup" => self.run("prev-page", cx),
                    "pagedown" => self.run("next-page", cx),
                    "delete" | "backspace" => self.run("delete", cx),
                    _ => return,
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// The PDF toolbar: tools, Sign, zoom and where you are.
    fn toolbar(&self, v: &Child<PdfView>, cx: &Cx<Self>) -> Element {
        let pv = v.read(cx);
        let editable = pv.editable && pv.loaded();
        let cmd = |c: &'static str| cx.listener(move |d: &mut Doc, _: &ClickEvent, cx| d.run(c, cx));
        let mut left = div().flex().items_center().gap(px(m::TIGHT));
        for (label, tool, c) in [("Select", Tool::Select, "tool-select"), ("Text", Tool::Text, "tool-text"), ("Draw", Tool::Draw, "tool-draw")] {
            left = left.child(Button::new(label).min_width(64.0).pushed(editable && pv.tool == tool).enabled(editable).on_click(cmd(c)));
        }
        // Sign…: its menu under the button.
        left = left.child(div().w(px(m::SPACING))).child(Button::new("Sign…").min_width(72.0).enabled(editable).on_click(cx.listener(|d: &mut Doc, _: &ClickEvent, cx| {
            let (sign, at, me) = (d.shared.sign_menu(""), cx.listener_bounds(), cx.view_ref());
            olo_kit::menu::popup(cx, sign, at, Corner::BottomLeft, move |cmd, cx| {
                let cmd = cmd.to_owned();
                me.update(cx, move |d, cx| d.run(&cmd, cx));
            });
        })));
        let read_only = pv.loaded() && !pv.editable;
        // The page and zoom as the pages have them (they tell when
        // either changes: `pdfview::Changed`).
        let page = pv.loaded().then(|| format!("Page {} of {}", pv.current_page + 1, pv.geoms.len()));
        let pct = format!("{}%", (pv.zoom * 100.0).round());
        let info = canvas(move |g, b| {
            let p = theme::palette();
            let dim = style::disabled_text(&p);
            let mut x = b.right();
            if let Some(page) = &page {
                let at = style::label(g, x, b.center().y, Align::Right, page.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                agent::record(g, agent::Node::label(page.clone(), at));
                x = at.left() - px(m::SPACING * 2.0);
            }
            if read_only && x > b.left() {
                let at = style::label(g, x, b.center().y, Align::Right, "Read only", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                agent::record(g, agent::Node::label("Read only", at));
            }
        })
        .flex_1()
        .h(px(m::CONTROL_HEIGHT));
        let zoom_label = canvas(move |g, b| {
            let p = theme::palette();
            let at = style::label(g, b.center().x, b.center().y, Align::Center, pct.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
            agent::record(g, agent::Node::label(pct.clone(), at));
        })
        .w(px(44.0))
        .h(px(m::CONTROL_HEIGHT));
        let right = div()
            .flex()
            .items_center()
            .gap(px(m::TIGHT))
            .child(Button::new("−").min_width(28.0).on_click(cmd("zoom-out")))
            .child(zoom_label)
            .child(Button::new("+").min_width(28.0).on_click(cmd("zoom-in")));
        let ground = canvas(|g, b| {
            let p = theme::palette();
            g.paint_quad(olo_kit::fill(b, style::c(p.panel_background)));
            style::hline_above(g, b.left(), b.right(), b.bottom(), style::shade(&p, p.panel_background, 1.3));
        })
        .absolute()
        .size_full();
        div().relative().h(px(m::TOOLBAR_HEIGHT)).w_full().px(px(m::SPACING)).flex().items_center().gap(px(m::SPACING)).child(ground).child(left).child(info).child(right).into_any_element()
    }

    /// Trim… (a video): handles on its timeline (the player's own,
    /// `PreviewView::set_trimming`), Cancel and Trim over it; Trim copies
    /// the stretch kept into "NAME (trimmed).mp4" beside it on a worker
    /// (`olo_video::trim`: nothing encoded again) and shows that.
    fn trim_command(&mut self, command: &str, cx: &mut Cx<Self>) {
        let Content::Other(prev) = &self.content else { return };
        match command {
            "trim" => prev.update(cx, |p, cx| p.set_trimming(true, cx)),
            "trim-cancel" => prev.update(cx, |p, cx| p.set_trimming(false, cx)),
            _ => {
                let Some((from, to)) = prev.read(cx).trimming() else { return };
                prev.update(cx, |p, cx| {
                    p.stop(cx);
                    p.set_trimming(false, cx);
                });
                self.trimming = true;
                let (src, done) = (self.path.clone(), cx.messenger());
                std::thread::spawn(move || {
                    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let dst = src.with_file_name(format!("{stem} (trimmed).mp4"));
                    let part = dst.with_extension("mp4.part");
                    let r = olo_video::trim::trim(&src, &part, from, to).and_then(|_| std::fs::rename(&part, &dst).map_err(|e| e.to_string()));
                    if r.is_err() {
                        let _ = std::fs::remove_file(&part);
                    }
                    done.send(DocMsg::Trimmed(r.map(|()| dst)));
                });
            }
        }
        cx.invalidate();
    }

    /// While trimming: Cancel and Trim, floating over the bottom of the
    /// picture, above the timeline.
    fn trim_bar(&self, prev: &Child<PreviewView>, cx: &Cx<Self>) -> Option<Element> {
        let (from, to) = prev.read(cx).trimming()?;
        let bw = m::BUTTON_MIN_WIDTH;
        let kept = format!("{} kept", olo_preview::media::format_duration(to - from));
        let p = theme::palette();
        Some(
            div()
                .absolute()
                .bottom(px(olo_preview::kit::UNDER_VIDEO + m::SPACING))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(m::SPACING))
                        .p(px(6.0))
                        .rounded(px(6.0))
                        .bg(style::c(p.scrim))
                        .child(olo_kit::widgets::StringView::new(kept).color(style::c(p.desktop_text)))
                        .child(Button::new("Cancel").min_width(bw).on_click(cx.listener(|d: &mut Doc, _: &ClickEvent, cx| d.trim_command("trim-cancel", cx))))
                        .child(Button::new("Trim").default(true).min_width(bw).enabled(to - from >= 0.1).on_click(cx.listener(|d: &mut Doc, _: &ClickEvent, cx| d.trim_command("trim-save", cx)))),
                )
                .into_any_element(),
        )
    }

    /// The picture's crop frame, over the preview, and its Crop and
    /// Cancel buttons.
    fn crop_overlay(&self, prev: &Child<PreviewView>, cx: &Cx<Self>) -> Option<Element> {
        let e = self.edit.as_ref()?;
        let crop = e.crop.clone()?;
        let full = e.editor.size();
        let (shown, view) = prev.read(cx).image_view().read(cx).picture_bounds()?;
        let frame = crop.clone();
        let paint = canvas(move |g, b| {
            g.set_cursor_style(frame.shape(), b);
            frame.paint(g, view, shown, full);
        })
        .absolute()
        .inset_0();
        let pointer = div()
            .id("crop")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |d: &mut Doc, e: &MouseDownEvent, cx| {
                    if let Some(c) = d.edit.as_mut().and_then(|e| e.crop.as_mut()) {
                        c.press(e.position, shown, full);
                        d.crop_press = Some(false);
                        cx.invalidate();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(move |d: &mut Doc, e: &MouseMoveEvent, cx| {
                let Some(c) = d.edit.as_mut().and_then(|e| e.crop.as_mut()) else { return };
                let mut changed = c.hover(e.position, shown, full);
                if e.pressed_button == Some(MouseButton::Left) && d.crop_press.is_some() {
                    d.crop_press = Some(true);
                    changed |= c.drag_to(e.position, shown, full);
                }
                if changed {
                    cx.invalidate();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|d: &mut Doc, _: &MouseUpEvent, cx| {
                    let moved = d.crop_press.take().unwrap_or(false);
                    if let Some(c) = d.edit.as_mut().and_then(|e| e.crop.as_mut()) {
                        c.release(moved);
                        cx.invalidate();
                    }
                }),
            );
        // Crop and Cancel, floating at the bottom.
        let has_sel = crop.sel.is_some();
        let bw = m::BUTTON_MIN_WIDTH;
        let buttons = div().absolute().bottom(px(m::SPACING)).left_0().right_0().flex().justify_center().child(
            div()
                .flex()
                .gap(px(m::SPACING))
                .p(px(6.0))
                .rounded(px(6.0))
                .bg(style::c(theme::palette().scrim))
                .child(Button::new("Cancel").min_width(bw).on_click(cx.listener(|d: &mut Doc, _: &ClickEvent, cx| d.picture_command("crop-cancel", cx))))
                .child(Button::new("Crop").default(true).min_width(bw).enabled(has_sel).on_click(cx.listener(|d: &mut Doc, _: &ClickEvent, cx| d.picture_command("crop", cx)))),
        );
        // Over the preview, not after it.
        Some(div().absolute().inset_0().child(paint).child(pointer).child(buttons).into_any_element())
    }
}

impl View for Doc {
    type Message = DocMsg;

    fn message(&mut self, message: DocMsg, cx: &mut Cx<Self>) {
        match message {
            DocMsg::Pdf(ev) => self.pdf_event(ev, cx),
            DocMsg::Stat(path, item) => self.stat(path, item, cx),
            DocMsg::Siblings(path, list) => {
                if self.path == path {
                    self.siblings = list;
                    // (Go ▸ Previous and Next File follow.)
                    cx.invalidate();
                }
            }
            DocMsg::Edit(ev) => self.edit_event(ev, cx),
            DocMsg::OnDisk(paths) => self.on_disk_changed(&paths, cx),
            DocMsg::LookAt(path) => self.leave(After::Open(path), cx),
            DocMsg::Trimmed(result) => {
                self.trimming = false;
                match result {
                    // The trimmed copy shown in this window.
                    Ok(path) => {
                        to_hub(ToHub::Showing(self.id, path.clone()));
                        self.load(path, cx);
                    }
                    Err(e) => self.error(format!("The video couldn’t be trimmed.\n\n{e}"), cx),
                }
                cx.invalidate();
            }
            m @ (DocMsg::EditPicture(..) | DocMsg::EditPeaks(..) | DocMsg::ExportProgress(..) | DocMsg::Exported(..)) => self.video_edit_message(m, cx),
            DocMsg::Chosen(purpose, path) => self.chosen(purpose, path, cx),
            DocMsg::Leave => self.leave(After::Close, cx),
            DocMsg::EditVideo => {
                if matches!(&self.content, Content::Other(prev) if prev.read(cx).is_video()) {
                    self.start_video_edit(cx);
                } else {
                    self.pending_edit = true;
                }
            }
            DocMsg::Signatures => cx.invalidate(),
            DocMsg::Dragged => {}
        }
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run(command, cx);
        true
    }

    /// The window's close: asks about unsaved changes first.
    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool {
        if self.dirty(cx) {
            self.leave(After::Close, cx);
            return false;
        }
        true
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.busy_said.store(false, Ordering::Relaxed);
        to_hub(ToHub::Closed(self.id));
    }

    /// The file it shows and how fast it plays, kept for an upgrade (a
    /// look isn't kept).
    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        if self.shared.look {
            return None;
        }
        let speed = match &self.content {
            Content::Other(p) => p.read(cx).chosen_rate(),
            _ => self.speed,
        };
        serde_json::to_string(&SavedDoc { file: self.path.clone(), speed }).ok()
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // The document has the keyboard (else the window's frame takes
        // it, and the arrows and Space never reach it).
        self.focus.focus_if_none(cx);
        self.fullscreen = cx.is_fullscreen();
        self.busy_said.store(self.busy(cx), Ordering::Relaxed);
        // A playing video keeps the screen awake.
        let playing = matches!(&self.content, Content::Other(p) if p.read(cx).playing_video());
        if playing != self.awake {
            self.awake = playing;
            cx.set_keep_awake(playing);
        }
        if matches!(self.content, Content::Pdf(_)) {
            self.update_title(cx);
        }
        let menus = self.menus(cx);
        let bar = MenuBar::new(cx, menus);
        let p = theme::palette();
        let editing = self.video_edit_area(cx);
        let body: Element = match &self.content {
            Content::Loading => div().size_full().bg(style::c(p.panel_background)).into_any_element(),
            Content::Failed(e) => {
                let e = e.clone();
                canvas(move |g, b| {
                    let p = theme::palette();
                    g.paint_quad(olo_kit::fill(b, style::c(p.panel_background)));
                    let at = style::label_fit(
                        g,
                        b.center().x,
                        b.center().y,
                        Align::Center,
                        e.clone(),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        style::disabled_text(&p),
                        b.size.width - px(2.0 * m::WINDOW_INSET),
                    );
                    agent::record(g, agent::Node::label(e.clone(), at));
                })
                .size_full()
                .into_any_element()
            }
            Content::Other(prev) => {
                let mut area = div().relative().size_full();
                // Editing a video: the editor's timeline and bar under it.
                area = match editing {
                    Some((timeline, bar, crop)) => {
                        let mut video = div().relative().flex_1().min_h_0().child(prev);
                        if let Some(crop) = crop {
                            video = video.child(crop);
                        }
                        area.flex().flex_col().child(video).child(timeline).child(bar)
                    }
                    None => {
                        let mut area = area.child(prev);
                        if let Some(overlay) = self.crop_overlay(prev, cx) {
                            area = area.child(overlay);
                        }
                        area
                    }
                };
                if let Some(bar) = self.trim_bar(prev, cx) {
                    area = area.child(bar);
                }
                if self.edit.as_ref().is_some_and(|e| e.editor.busy || e.editor.saving) {
                    let spin = canvas(|g, b| {
                        let p = theme::palette();
                        style::spinner(g, point(b.right() - px(20.0), b.top() + px(20.0)), 9.0, style::disabled_text(&p));
                        g.paint_again_within(1.0 / 60.0);
                    })
                    .absolute()
                    .inset_0();
                    area = area.child(spin);
                }
                area.into_any_element()
            }
            Content::Pdf(v) => {
                let toolbar = self.toolbar(v, cx);
                let pages: Element = if v.read(cx).loaded() { v.element() } else { div().size_full().bg(style::c(p.panel_background)).into_any_element() };
                div().size_full().flex().flex_col().child(toolbar).child(div().flex_1().min_h_0().child(pages)).into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|d: &mut Doc, e: &KeyDownEvent, cx| d.keys(e, cx)))
            // A look following its panel: its arrows are the panel's, before
            // what it shows (a text, a PDF) takes them to scroll.
            .capture_key_down(cx.listener(|d: &mut Doc, e: &KeyDownEvent, cx| d.follow_keys(e, cx)))
            .on_drop(|paths: &ExternalPaths, _| {
                for p in paths.paths() {
                    to_hub(ToHub::Open(p.to_path_buf()));
                }
            })
            // (Full screen, the bar's still there for its keys, but none
            // of it shows.)
            .child(if self.fullscreen { div().h(px(0.0)).overflow_hidden().child(bar) } else { div().child(bar) })
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

/// Adjust Size for a picture: width and height as typed, kept in
/// proportion if asked.
struct Resize {
    doc: ViewRef<Doc>,
    size: (u32, u32),
    w: Child<TextField>,
    h: Child<TextField>,
    keep: bool,
}

impl Resize {
    fn new(doc: ViewRef<Doc>, size: (u32, u32), cx: &mut Cx<Self>) -> Resize {
        let w = text_field(cx, |f| f.name("Width"));
        let h = text_field(cx, |f| f.name("Height"));
        w.update(cx, |f, cx| f.set_text(size.0.to_string(), cx));
        h.update(cx, |f, cx| f.set_text(size.1.to_string(), cx));
        w.focus(cx);
        // Keep the other side in proportion as one is typed.
        cx.subscribe(&w, |r: &mut Resize, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Typed(_))
                && r.keep
                && let Ok(v) = r.w.read(cx).text().trim().parse::<u32>()
            {
                let ratio = r.size.1 as f64 / r.size.0.max(1) as f64;
                let h = ((v as f64 * ratio).round() as u32).max(1).to_string();
                r.h.update(cx, |f, cx| f.set_text(h, cx));
            }
            cx.invalidate();
        });
        cx.subscribe(&h, |r: &mut Resize, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Typed(_))
                && r.keep
                && let Ok(v) = r.h.read(cx).text().trim().parse::<u32>()
            {
                let ratio = r.size.1 as f64 / r.size.0.max(1) as f64;
                let w = ((v as f64 / ratio).round() as u32).max(1).to_string();
                r.w.update(cx, |f, cx| f.set_text(w, cx));
            }
            cx.invalidate();
        });
        Resize { doc, size, w, h, keep: true }
    }

    fn values(&self, cx: &WindowCx) -> Option<(u32, u32)> {
        match (self.w.read(cx).text().trim().parse::<u32>(), self.h.read(cx).text().trim().parse::<u32>()) {
            (Ok(a), Ok(b)) if (1..=65_535).contains(&a) && (1..=65_535).contains(&b) => Some((a, b)),
            _ => None,
        }
    }

    fn done(&mut self, apply: bool, cx: &mut Cx<Self>) {
        if apply && let Some((w, h)) = self.values(cx) {
            self.doc.update(cx, move |d, cx| {
                if let Some(e) = &mut d.edit {
                    e.editor.push(Op::Resize { w, h });
                    d.update_title(cx);
                    cx.invalidate();
                }
            });
        }
        cx.close_window();
    }
}

impl View for Resize {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let ok = self.values(cx).is_some();
        let now = format!("Now {} × {} pixels.", self.size.0, self.size.1);
        let field = |label: &'static str, f: &Child<TextField>| {
            div().flex().items_center().h(px(m::CONTROL_HEIGHT)).child(div().w(px(60.0)).child(label)).child(div().w(px(110.0)).child(TextControl::new(cx, f)))
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .on_key_down(cx.listener(|r: &mut Resize, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    r.done(false, cx);
                    cx.stop_propagation();
                }
            }))
            .child(div().text_color(style::disabled_text(&theme::palette())).child(now))
            .child(field("Width:", &self.w))
            .child(field("Height:", &self.h))
            .child(CheckBox::new("Keep proportions", self.keep).on_toggle(cx.listener(|r: &mut Resize, on: &bool, cx| {
                r.keep = *on;
                cx.invalidate();
            })))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Cancel").on_click(cx.listener(|r: &mut Resize, _: &ClickEvent, cx| r.done(false, cx))))
                    .child(Button::new("Resize").default(true).enabled(ok).on_click(cx.listener(|r: &mut Resize, _: &ClickEvent, cx| r.done(true, cx)))),
            )
            .into_any_element()
    }
}

/// Dragging the document out (from a picture that fits the window): the
/// file, and for a common picture format its bytes too, for programs that
/// take an image rather than a file.
fn file_drag(path: &Path) -> olo_kit::OutgoingDrag {
    let mut drag = olo_kit::dnd::files(&[path.to_path_buf()], true, false);
    let mime = olo_xdg::mime::for_path(path);
    let small = std::fs::metadata(path).is_ok_and(|m| m.len() <= 16 << 20);
    if matches!(mime.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp")
        && small
        && let Ok(bytes) = std::fs::read(path)
    {
        // After the file: a folder takes the file, an editor the picture.
        drag.types.push((mime, Arc::from(bytes)));
    }
    drag
}

/// View ▸ Playback Speed: slower and faster (`<` and `>`), then each
/// speed, the one playing checked.
fn speed_menu(rate: f64) -> Menu {
    let speeds = PreviewView::SPEEDS;
    let menu = Menu::new("Playback Speed")
        .item("Slower", "speed-slower")
        .keys("<")
        .enabled(rate > speeds[0] + 1e-6)
        .item("Faster", "speed-faster")
        .keys(">")
        .enabled(rate < speeds[speeds.len() - 1] - 1e-6)
        .separator();
    olo_preview::kit::speed_items(menu, rate)
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // (A look is its own process, which the panel that asked for it can
    // put away.)
    let look = args.iter().any(|a| a == "--look");
    let follow = look && args.iter().any(|a| a == "--follow");
    let over = args.iter().find_map(|a| a.strip_prefix("--over=")).map(str::to_owned);
    if !look && instance::forward(APP_ID, &args) {
        return;
    }
    olo_preview::audio::set_identity("Viewer", "olo-viewer");
    olo_kit::run(APP_ID, move |app| {
        if !look {
            instance::listen(APP_ID, olo_kit::proxy());
        }
        let shared = Arc::new(Shared { look, follow, signatures: Mutex::new(signature::load()), busy: Mutex::default() });
        let mut hub = Hub { docs: Vec::new(), shared: shared.clone(), over, watch: None, next: 0 };
        // What the panel says to show, a line at a time, on a thread.
        if follow {
            let read = std::thread::Builder::new().name("look follows".into()).spawn(move || {
                use std::io::BufRead as _;
                for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                    let path = PathBuf::from(line.trim_end());
                    if path.is_file() {
                        to_hub(ToHub::LookAt(path));
                    }
                }
            });
            if let Err(e) = read {
                tracing::warn!("a look can't follow its panel: {e}");
            }
        }
        // The files open, kept for an upgrade (a look isn't kept): what
        // each window keeps, as one list.
        olo_kit::on_upgrade(move |windows| {
            if look {
                return Some("{}".into());
            }
            let docs: Vec<SavedDoc> = windows.iter().filter_map(|s| serde_json::from_str(s).ok()).collect();
            serde_json::to_string(&Saved { files: docs.iter().map(|d| d.file.clone()).collect(), speeds: docs.iter().map(|d| d.speed).collect() }).ok()
        });
        // Unsaved changes, or a save under way: the new version waits.
        let busy = shared.clone();
        olo_kit::upgrade_waits_while(move || busy.busy.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|b| b.upgrade().is_some_and(|b| b.load(Ordering::Relaxed))));
        let restored = olo_kit::restored_state().filter(|_| !look).and_then(|s| serde_json::from_str::<Saved>(&s).ok());
        match restored {
            Some(saved) if !saved.files.is_empty() => {
                for (k, f) in saved.files.into_iter().enumerate() {
                    hub.open_file(f, false, saved.speeds.get(k).copied());
                }
            }
            _ => hub.handle_args(&args, None),
        }
        olo_kit::message_handler(app, move |message, _| hub.message(message));
    });
}
