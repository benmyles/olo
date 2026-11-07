//! Viewer: a window for any file Quick Look can preview (Tracker's pop-out
//! button opens one), and a PDF editor: fill in forms, add text, draw,
//! sign, then Save (appended to the original as an incremental update),
//! Save As, or Export a flattened copy. Pictures can be rotated, flipped,
//! cropped and resized (olo-preview's `edit`), then saved. On Olo's GPUI.

mod crop;
mod pdfdoc;
mod pdftext;
mod pdfview;
mod pdfwork;
mod signature;
mod video_edit;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_preview::{
    Body, Item,
    edit::{self, EditEvent, Editor, Op},
    ui::{PreviewEvent, PreviewView, VideoZoom},
};
use olo_ui::{
    Frame, Menu, MenuBar, WindowOptions, agent,
    files::{self, Kind},
    gpui::{
        self, AnyElement, App, AppContext as _, Context, Entity, EntityId, ExternalPaths, FocusHandle, Focusable as _, FontWeight, HitboxBehavior, InteractiveElement as _, IntoElement, KeyDownEvent,
        MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Render, Styled as _, Subscription, WeakEntity, Window, WindowHandle, canvas, div, point, px, size,
    },
    instance::{self, Launch},
    metrics as m,
    style::{self, Align},
    theme,
    widgets::{Button, CheckBox, TextField, TextFieldEvent, Unsaved, ask_to_save, open_alert, text_field},
};
use serde::{Deserialize, Serialize};

use crop::CropFrame;
use pdfview::{Fit, PdfView, Tool};
use pdfwork::Event;
use signature::{Pad, Signature};

const APP_ID: &str = "olo.viewer";

/// From the workers, to the document they're for.
enum Msg {
    Pdf(EntityId, Event),
    Stat(EntityId, PathBuf, Option<Item>),
    Siblings(EntityId, PathBuf, Vec<PathBuf>),
    Edit(EntityId, EditEvent),
    /// Open files, or the folders they are in, changed on disk.
    OnDisk(Vec<PathBuf>),
    /// A look following a file panel (`--follow`): the file to show now.
    LookAt(PathBuf),
    /// A video trimmed (`Doc::trim`): where the trimmed copy went, or why
    /// not.
    Trimmed(EntityId, Result<PathBuf, String>),
    /// The video editor's (`video_edit`): a picture for its timeline
    /// (which, the scale it's made for, it), the waveform's peaks, how far
    /// an export is, and what it made (with the edits it made it of).
    EditPicture(EntityId, usize, f32, std::sync::Arc<gpui::RenderImage>),
    EditPeaks(EntityId, Option<Vec<f32>>),
    ExportProgress(EntityId, f32),
    Exported(EntityId, Result<PathBuf, String>, video_edit::Edits),
}

enum Content {
    /// Waiting for the file's details.
    Loading,
    Pdf(Entity<PdfView>),
    Other(Entity<PreviewView>),
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
enum Purpose {
    Open,
    SaveAs,
    Export,
}

/// Editing a picture: its steps, and the frame while cropping.
struct ImageEdit {
    editor: Editor,
    crop: Option<CropFrame>,
}

struct Viewer {
    docs: Vec<Entity<Doc>>,
    signatures: Vec<Signature>,
    generation: u64,
    /// The open files and their folders, followed as they change: a file
    /// another program changed is shown again unless it has changes here,
    /// and Previous / Next know new files.
    watch: Option<olo_ui::watch::Watch>,
    /// Started with `--look` (a file panel's Space): a look at a file,
    /// its own process, put away with Space or Escape, as Quick Look.
    look: bool,
    /// The window the look opens over (`--over=HANDLE`, the file panel's,
    /// exported: xdg-foreign).
    over: Option<String>,
    /// The look follows the panel that asked for it (`--follow`): the
    /// arrows ask it for the file before or after (`step -1`, `step 1` on
    /// stdout), and it says what to show (a path a line on stdin), so the
    /// look steps through the panel's list as the panel's selection does.
    follow: bool,
    /// File panels open, and with no document (to quit when they go).
    panels: usize,
}

struct Doc {
    viewer: WeakEntity<Viewer>,
    win: Option<WindowHandle<Frame>>,
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
    _subs: Vec<Subscription>,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    files: Vec<PathBuf>,
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
fn screen(window: &Window, cx: &App) -> Option<(f32, f32)> {
    let size = |d: &std::rc::Rc<dyn gpui::PlatformDisplay>| {
        let b = d.bounds();
        (f32::from(b.size.width), f32::from(b.size.height))
    };
    // Not on one yet, and it could open on any: the smallest.
    window.display(cx).map(|d| size(&d)).or_else(|| cx.displays().iter().map(size).min_by(|a, b| (a.0 * a.1).total_cmp(&(b.0 * b.1))))
}

impl Viewer {
    fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // Already open: bring it forward.
        if let Some(doc) = self.docs.iter().find(|d| d.read(cx).path == path)
            && let Some(win) = doc.read(cx).win
        {
            olo_ui::raise(Some(win), cx);
            return;
        }
        let title = file_name(&path);
        let min_w = if is_pdf(&path) { pdfview::MIN_WIDTH } else { 360.0 };
        let me = cx.weak_entity();
        let doc = cx.new(|cx| Doc {
            viewer: me,
            win: None,
            path: path.clone(),
            content: Content::Loading,
            siblings: Vec::new(),
            title: title.clone(),
            sized: false,
            after_save: After::Nothing,
            awake: false,
            fullscreen: false,
            edit: None,
            trimming: false,
            video_edit: None,
            pending_edit: false,
            on_disk: None,
            focus: cx.focus_handle(),
            crop_press: None,
            _subs: Vec::new(),
        });
        let view = doc.clone();
        let mut options = WindowOptions::new(title, 720.0, 560.0).min_size(min_w, 240.0);
        // A look floats over the panel that asked, as Quick Look does:
        // never a tile or a tab.
        if self.look {
            options = options.dialog();
            if let Some(handle) = &self.over {
                options = options.foreign_parent(handle.clone());
            }
        }
        let win = olo_ui::open_window(cx, options, move |window, cx| {
            // The document has the keyboard (else the window's frame
            // takes it, and the arrows and Space never reach it).
            window.focus(&view.read(cx).focus);
            view
        });
        doc.update(cx, |d, cx| {
            d.win = win;
            d.set_title(cx);
        });
        if let Some(win) = win {
            // The window's close: asks about unsaved changes first.
            let closing = doc.clone();
            let _ = win.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |window, cx| {
                    closing.update(cx, |d, cx| d.leave(After::Close, window, cx));
                    false
                });
            });
        }
        self.docs.push(doc.clone());
        doc.update(cx, |d, cx| d.load(path, cx));
        self.follow_files(cx);
    }

    /// Follow the open files and their folders (as they are now).
    fn follow_files(&mut self, cx: &mut Context<Self>) {
        let mut paths: Vec<PathBuf> = Vec::new();
        for d in &self.docs {
            let p = &d.read(cx).path;
            paths.push(p.clone());
            paths.extend(p.parent().map(Path::to_path_buf));
        }
        paths.sort();
        paths.dedup();
        match &self.watch {
            Some(w) => w.set(paths),
            None => {
                let proxy = olo_ui::proxy(cx);
                self.watch = olo_ui::watch::watch(paths, move |changed| proxy.send(Msg::OnDisk(changed.to_vec())));
            }
        }
    }

    /// A document's window went.
    fn closed(&mut self, doc: EntityId, cx: &mut Context<Self>) {
        self.docs.retain(|d| d.entity_id() != doc);
        self.follow_files(cx);
        if self.docs.is_empty() && self.panels == 0 {
            cx.quit();
        }
    }

    fn show_error(&mut self, message: String, cx: &mut Context<Self>) {
        let _ = open_alert(cx, "Viewer", "warning", &message, &["OK"], |_, _| {});
    }

    fn open_panel(&mut self, doc: Option<Entity<Doc>>, purpose: Purpose, cx: &mut Context<Self>) {
        let path = doc.as_ref().map(|d| d.read(cx).path.clone());
        let dir = path.as_deref().and_then(Path::parent).map_or_else(|| std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from), Path::to_path_buf);
        let stem = path.as_deref().and_then(Path::file_stem).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        use olo_ui::file_panel::{FilePanel, PanelResult};
        let (title, panel) = match purpose {
            Purpose::Open => ("Open", FilePanel::open(dir)),
            Purpose::SaveAs if path.as_deref().is_some_and(|p| !is_pdf(p)) => {
                // A picture: its own format if we write it, else PNG.
                let ext = path.as_deref().filter(|p| edit::save_format(p).is_some()).and_then(Path::extension).map_or("png".into(), |e| e.to_string_lossy().to_lowercase());
                ("Save As", FilePanel::save(dir, format!("{stem} copy.{ext}")).types(&["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"]))
            }
            Purpose::SaveAs => ("Save As", FilePanel::save(dir, format!("{stem} copy.pdf")).types(&["pdf"])),
            Purpose::Export => ("Export Flattened PDF", FilePanel::save(dir, format!("{stem} (flattened).pdf")).types(&["pdf"])),
        };
        self.panels += 1;
        let me = cx.entity();
        olo_ui::file_panel::show(cx, title, panel, move |answer, cx| {
            me.update(cx, |v, cx| {
                v.panels -= 1;
                let PanelResult::Chosen(path) = answer.result else {
                    if v.docs.is_empty() && v.panels == 0 {
                        cx.quit();
                    }
                    return;
                };
                v.panel_chosen(doc, purpose, path, cx);
            })
        });
    }

    fn panel_chosen(&mut self, doc: Option<Entity<Doc>>, purpose: Purpose, path: PathBuf, cx: &mut Context<Self>) {
        match (purpose, doc) {
            (Purpose::Open, _) | (_, None) => self.open_file(path, cx),
            (Purpose::SaveAs, Some(doc)) if !is_pdf(&doc.read(cx).path) => {
                // A picture: written there (with any edits), then shown.
                if edit::save_format(&path).is_none() {
                    return self.show_error("Pictures can be saved as PNG, JPEG, WebP, BMP, TIFF or GIF.".into(), cx);
                }
                doc.update(cx, |d, cx| {
                    d.ensure_editor(cx);
                    if let Some(e) = &mut d.edit {
                        e.editor.save(path);
                    }
                    cx.notify();
                });
            }
            (Purpose::SaveAs | Purpose::Export, Some(doc)) => doc.update(cx, |d, cx| {
                if let Content::Pdf(v) = &d.content {
                    let flatten = purpose == Purpose::Export;
                    d.after_save = After::Nothing;
                    v.update(cx, |v, cx| v.save(path, flatten, !flatten, cx));
                }
            }),
        }
    }

    fn handle_args(&mut self, args: &[String], cwd: Option<&Path>, cx: &mut Context<Self>) {
        let mut opened = false;
        // `--edit`: a video opened in its editor (Screenshot's card's Edit).
        let edit = args.iter().any(|a| a == "--edit");
        for a in args.iter().filter(|a| !a.starts_with("--")) {
            let p = PathBuf::from(a);
            let p = match (p.is_absolute(), cwd) {
                (false, Some(cwd)) => cwd.join(p),
                _ => p,
            };
            self.open_file(p.clone(), cx);
            opened = true;
            if edit && let Some(doc) = self.docs.iter().find(|d| d.read(cx).path == p).cloned() {
                doc.update(cx, |d, cx| {
                    let shown = matches!(&d.content, Content::Other(prev) if prev.read(cx).is_video());
                    if shown {
                        d.start_video_edit(cx);
                    } else {
                        d.pending_edit = true;
                    }
                });
            }
        }
        if !opened && self.docs.is_empty() {
            self.open_panel(None, Purpose::Open, cx);
        }
    }

    fn sign_menu(&self, title: &str) -> Menu {
        let mut sign = Menu::new(title);
        for (k, s) in self.signatures.iter().enumerate() {
            sign = sign.item(signature::label(s, k), format!("sign:{k}"));
        }
        sign = sign.separator().item("New Signature…", "new-signature");
        if !self.signatures.is_empty() {
            let mut del = Menu::new("Delete Signature");
            for (k, s) in self.signatures.iter().enumerate() {
                del = del.item(signature::label(s, k), format!("unsign:{k}"));
            }
            sign = sign.submenu(del);
        }
        sign
    }

    fn doc_of(&self, id: EntityId) -> Option<Entity<Doc>> {
        self.docs.iter().find(|d| d.entity_id() == id).cloned()
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_args(&launch.args, launch.cwd.as_deref(), cx),
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            // The look's one window, on the file the panel has selected now.
            Msg::LookAt(path) => {
                if let Some(doc) = self.docs.first().cloned()
                    && let Some(win) = doc.read(cx).win
                {
                    let _ = win.update(cx, |_, window, cx| doc.update(cx, |d, cx| d.leave(After::Open(path), window, cx)));
                }
            }
            Msg::OnDisk(paths) => {
                for doc in self.docs.clone() {
                    doc.update(cx, |d, cx| d.on_disk_changed(&paths, cx));
                }
            }
            Msg::Edit(id, ev) => {
                if let Some(doc) = self.doc_of(id) {
                    doc.update(cx, |d, cx| d.edit_event(ev, cx));
                }
            }
            m @ (Msg::EditPicture(..) | Msg::EditPeaks(..) | Msg::ExportProgress(..) | Msg::Exported(..)) => {
                let id = match &m {
                    Msg::EditPicture(id, ..) | Msg::EditPeaks(id, _) | Msg::ExportProgress(id, _) | Msg::Exported(id, ..) => *id,
                    _ => return,
                };
                if let Some(doc) = self.doc_of(id) {
                    doc.update(cx, |d, cx| d.video_edit_message(m, cx));
                }
            }
            Msg::Trimmed(id, result) => {
                if let Some(doc) = self.doc_of(id) {
                    doc.update(cx, |d, cx| {
                        d.trimming = false;
                        match result {
                            // The trimmed copy shown in this window.
                            Ok(path) => d.load(path, cx),
                            Err(e) => {
                                if let Some(v) = d.viewer.upgrade() {
                                    v.update(cx, |v, cx| v.show_error(format!("The video couldn’t be trimmed.\n\n{e}"), cx));
                                }
                            }
                        }
                        cx.notify();
                    });
                }
            }
            Msg::Siblings(id, path, list) => {
                if let Some(doc) = self.doc_of(id) {
                    doc.update(cx, |d, _| {
                        if d.path == path {
                            d.siblings = list;
                        }
                    });
                }
            }
            Msg::Stat(id, path, item) => {
                self.generation += 1;
                let generation = self.generation;
                if let Some(doc) = self.doc_of(id) {
                    doc.update(cx, |d, cx| d.stat(path, item, generation, cx));
                }
            }
            Msg::Pdf(id, ev) => {
                if let Some(doc) = self.doc_of(id) {
                    doc.update(cx, |d, cx| d.pdf_event(ev, cx));
                }
            }
        }
    }
}

impl Doc {
    fn me(&self, cx: &Context<Self>) -> EntityId {
        cx.entity_id()
    }

    /// (Re)load `path` into this window.
    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.on_disk = stamp(&path);
        self.path = path.clone();
        self.sized = false;
        self.edit = None;
        self.title = file_name(&path);
        self.set_title(cx);
        let (proxy, id) = (olo_ui::proxy(cx), self.me(cx));
        // (sRGB again till a wide photo shows.)
        self.set_gamut(olo_ui::gpui::Gamut::Srgb, cx);
        if is_pdf(&path) {
            let sending = proxy.clone();
            let tx = pdfwork::start(path.clone(), move |ev| sending.send(Msg::Pdf(id, ev)));
            self.content = Content::Pdf(cx.new(|cx| PdfView::new(tx, cx)));
        } else {
            self.content = Content::Loading;
            let (p, sending) = (path.clone(), proxy.clone());
            std::thread::spawn(move || {
                let item = Item::stat(&p);
                sending.send(Msg::Stat(id, p, item));
            });
        }
        std::thread::spawn(move || {
            let s = list_siblings(&path);
            proxy.send(Msg::Siblings(id, path, s));
        });
        cx.notify();
    }

    /// Draw the window in `gamut` (Display P3 while it shows a photo
    /// wider than sRGB, else sRGB).
    fn set_gamut(&mut self, gamut: olo_ui::gpui::Gamut, cx: &mut Context<Self>) {
        if let Some(win) = self.win {
            cx.defer(move |cx| {
                let _ = win.update(cx, |_, window, _| window.set_gamut(gamut));
            });
        }
    }

    /// As `set_gamut`, for what `gamut` its picture is in; and with
    /// `hdr_video` (a preview playing HDR video), in HDR10 when the window
    /// is on a display whose HDR is on and can draw in it, the video's
    /// pictures then HDR10's; else as the picture is (P3 for a wide one),
    /// the video's mapped for SDR.
    fn set_video_gamut(&mut self, gamut: olo_ui::gpui::Gamut, hdr_video: Option<Entity<PreviewView>>, cx: &mut Context<Self>) {
        use olo_ui::gpui::Gamut;
        let Some(win) = self.win else { return };
        cx.defer(move |cx| {
            let _ = win.update(cx, |_, window, cx| {
                let hdr = hdr_video.is_some() && window.on_hdr_display() && window.set_gamut(Gamut::Hdr10);
                if let Some(p) = &hdr_video {
                    p.read(cx).draw_video_hdr(hdr);
                }
                if !hdr {
                    // (Wide still: an HDR video's pictures are BT.2020's.)
                    let gamut = if hdr_video.is_some() {
                        Gamut::DisplayP3
                    } else if gamut == Gamut::Hdr10 {
                        Gamut::DisplayP3
                    } else {
                        gamut
                    };
                    window.set_gamut(gamut);
                }
            });
        });
    }

    /// The window's title, and the file it shows (its tab's Open in
    /// Tracker).
    fn set_title(&self, cx: &mut Context<Self>) {
        if let Some(win) = self.win {
            let (title, path) = (self.title.clone(), self.path.clone());
            cx.defer(move |cx| {
                let _ = win.update(cx, |f, window, cx| {
                    f.set_title(title, window);
                    agent::set_path(window, cx, Some(path));
                });
            });
        }
    }

    fn stat(&mut self, path: PathBuf, item: Option<Item>, generation: u64, cx: &mut Context<Self>) {
        if self.path != path {
            return;
        }
        match item {
            Some(item) => {
                let preview = cx.new(|cx| PreviewView::new(item, generation, true, cx));
                let sub = cx.subscribe(&preview, |d: &mut Doc, prev, e: &PreviewEvent, cx| {
                    // A photo wider than sRGB shown whole: the window draws
                    // in Display P3 while it shows one (`Gamut`).
                    // A video wider than sRGB (BT.2020, P3) too, from its
                    // first picture; an HDR one in HDR10 on a display
                    // whose HDR is on.
                    if matches!(*e, PreviewEvent::Shown | PreviewEvent::Gamut) {
                        let (gamut, hdr) = {
                            let p = prev.read(cx);
                            (p.gamut(cx), p.video_is_hdr())
                        };
                        d.set_video_gamut(gamut, hdr.then(|| prev.clone()), cx);
                    }
                    if *e == PreviewEvent::Shown && !d.sized {
                        d.sized = true;
                        d.resize_to(prev.clone(), cx);
                    }
                    // Opened to be edited (`--edit`): the editor, once the
                    // video shows.
                    if *e == PreviewEvent::Shown && d.pending_edit && prev.read(cx).is_video() {
                        d.pending_edit = false;
                        d.start_video_edit(cx);
                    }
                    cx.notify();
                });
                // Dragging the picture out where it doesn't pan: the file.
                let (path, doc) = (self.path.clone(), cx.entity());
                preview.read(cx).image_view().clone().update(cx, |v, _| {
                    v.on_drag_out(move |window, cx| {
                        let cropping = doc.read(cx).edit.as_ref().is_some_and(|e| e.crop.is_some());
                        if !cropping {
                            olo_ui::dnd::start(window, cx, file_drag(&path), |_, _| {});
                        }
                    })
                });
                self._subs = vec![sub];
                self.content = Content::Other(preview);
            }
            None => self.content = Content::Failed(format!("“{}” can't be found.", file_name(&path))),
        }
        cx.notify();
    }

    /// The window's size for what the preview shows, within its screen.
    fn resize_to(&self, prev: Entity<PreviewView>, cx: &mut Context<Self>) {
        let Some(win) = self.win else { return };
        cx.defer(move |cx| {
            let _ = win.update(cx, |_, window, cx| {
                let s = screen(window, cx).map(|(w, h)| (w as u32, h as u32));
                let (w, h) = prev.read(cx).ideal_size(s, olo_ui::menu::BAR_HEIGHT);
                window.resize(size(px(w as f32), px(h as f32)));
            });
        });
    }

    fn pdf_event(&mut self, ev: Event, cx: &mut Context<Self>) {
        let Content::Pdf(v) = &self.content else { return };
        let v = v.clone();
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
                if adopted.is_some() {
                    self.path = to.clone();
                }
                after = Some(std::mem::replace(&mut self.after_save, After::Nothing));
            }
            _ => {}
        }
        let opened = v.update(cx, |v, cx| v.event(ev, cx));
        if opened && !self.sized {
            self.sized = true;
            if let Some(win) = self.win {
                let pdf = v.clone();
                cx.defer(move |cx| {
                    let _ = win.update(cx, |_, window, cx| {
                        let s = screen(window, cx);
                        let (w, h) = pdf.update(cx, |p, _| p.ideal_size(s, olo_ui::menu::BAR_HEIGHT + m::TOOLBAR_HEIGHT + 1.0));
                        window.resize(size(px(w), px(h)));
                    });
                });
            }
        }
        if let Some(e) = &error
            && !v.read(cx).loaded()
        {
            self.content = Content::Failed(e.clone());
            error = None;
        }
        if let Some(e) = error
            && let Some(viewer) = self.viewer.upgrade()
        {
            cx.defer(move |cx| viewer.update(cx, |v, cx| v.show_error(e, cx)));
        }
        if let Some(a) = after {
            self.finish(a, cx);
        }
        cx.notify();
    }

    fn edit_event(&mut self, ev: EditEvent, cx: &mut Context<Self>) {
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
                if let Some(viewer) = self.viewer.upgrade() {
                    cx.defer(move |cx| viewer.update(cx, |v, cx| v.show_error(m, cx)));
                }
            }
        }
        self.update_title(cx);
        cx.notify();
    }

    fn on_disk_changed(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        // A file beside it came or went: Previous / Next know.
        if self.path.parent().is_some_and(|dir| paths.iter().any(|p| p == dir)) {
            let (proxy, path, id) = (olo_ui::proxy(cx), self.path.clone(), self.me(cx));
            std::thread::spawn(move || {
                let s = list_siblings(&path);
                proxy.send(Msg::Siblings(id, path, s));
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
    fn busy(&self, cx: &App) -> bool {
        matches!(&self.content, Content::Pdf(v) if v.read(cx).dirty() || v.read(cx).saving) || self.edit.as_ref().is_some_and(|e| e.editor.dirty() || e.editor.saving)
    }

    fn dirty(&self, cx: &App) -> bool {
        matches!(&self.content, Content::Pdf(v) if v.read(cx).dirty()) || self.edit.as_ref().is_some_and(|e| e.editor.dirty())
    }

    fn update_title(&mut self, cx: &mut Context<Self>) {
        let title = format!("{}{}", file_name(&self.path), if self.dirty(cx) { " — Edited" } else { "" });
        if title != self.title {
            self.title = title;
            self.set_title(cx);
        }
    }

    /// Save the document in place; `after` runs once it's written.
    fn save(&mut self, after: After, cx: &mut Context<Self>) {
        if let Some(e) = &mut self.edit {
            // A format we can't write (HEIC, AVIF, …): Save As instead.
            if edit::save_format(&self.path).is_none() {
                return self.ask_panel(Purpose::SaveAs, cx);
            }
            self.after_save = after;
            e.editor.save(self.path.clone());
            cx.notify();
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

    fn error(&self, message: String, cx: &mut Context<Self>) {
        if let Some(viewer) = self.viewer.upgrade() {
            cx.defer(move |cx| viewer.update(cx, |v, cx| v.show_error(message, cx)));
        }
    }

    fn ask_panel(&self, purpose: Purpose, cx: &mut Context<Self>) {
        let (viewer, doc) = (self.viewer.clone(), cx.entity());
        cx.defer(move |cx| {
            if let Some(viewer) = viewer.upgrade() {
                viewer.update(cx, |v, cx| v.open_panel(Some(doc), purpose, cx));
            }
        });
    }

    /// Close or move on, asking about unsaved changes.
    fn leave(&mut self, after: After, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dirty(cx) {
            return self.finish(after, cx);
        }
        let _ = window;
        let (name, doing) = (file_name(&self.path), if after == After::Close { "closing" } else { "moving on" });
        let doc = cx.entity();
        cx.defer(move |cx| {
            let _ = ask_to_save(cx, "Viewer", &name, doing, move |choice, cx| {
                let after = after.clone();
                doc.update(cx, |d, cx| match choice {
                    // Don't Save: closing or loading another file drops the edits.
                    Some(Unsaved::DontSave) => d.finish(after, cx),
                    Some(Unsaved::Save) => d.save(after, cx),
                    None => {}
                })
            });
        });
    }

    fn finish(&mut self, after: After, cx: &mut Context<Self>) {
        match after {
            After::Nothing => {}
            After::Close => {
                self.awake = false;
                let (win, viewer, id) = (self.win, self.viewer.clone(), self.me(cx));
                cx.defer(move |cx| {
                    if let Some(win) = win {
                        let _ = win.update(cx, |_, window, _| window.remove_window());
                    }
                    if let Some(viewer) = viewer.upgrade() {
                        viewer.update(cx, |v, cx| v.closed(id, cx));
                    }
                });
            }
            After::Open(p) => {
                self.load(p, cx);
                if let Some(viewer) = self.viewer.upgrade() {
                    cx.defer(move |cx| viewer.update(cx, |v, cx| v.follow_files(cx)));
                }
            }
        }
    }

    /// Previous (-1) or next (+1) file in the folder.
    fn step(&mut self, d: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.siblings.iter().position(|p| *p == self.path) else { return };
        let j = i as isize + d;
        if j < 0 || j as usize >= self.siblings.len() {
            return;
        }
        let next = self.siblings[j as usize].clone();
        self.leave(After::Open(next), window, cx);
    }

    /// A picture being edited: its editor, made as the first step is.
    fn ensure_editor(&mut self, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            return;
        }
        let Content::Other(prev) = &self.content else { return };
        let Some(Body::Image { full, .. }) = prev.read(cx).body() else { return };
        let (proxy, id) = (olo_ui::proxy(cx), self.me(cx));
        let editor = Editor::new(self.path.clone(), (full[0] as u32, full[1] as u32), move |ev| proxy.send(Msg::Edit(id, ev)));
        self.edit = Some(ImageEdit { editor, crop: None });
    }

    fn picture(&self, cx: &App) -> bool {
        matches!(&self.content, Content::Other(prev) if prev.read(cx).is_image()) && edit::editable(&self.path)
    }

    fn menus(&self, cx: &App) -> Vec<Menu> {
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
        file = file.item(olo_ui::tracker::LABEL, olo_ui::tracker::COMMAND).separator().item("Close", "close").key("w").item("Quit", "quit").key("q");
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
                    Menu::new("Edit")
                        .item("Undo", "edit-undo")
                        .key("z")
                        .enabled(e.can_undo() && !busy)
                        .item("Redo", "edit-redo")
                        .shift_key("z")
                        .enabled(e.can_redo() && !busy)
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
            menus.push(Menu::new("Edit").item("Undo", "undo").key("z").enabled(editor.is_some_and(|e| e.can_undo())).item("Redo", "redo").shift_key("z").enabled(editor.is_some_and(|e| e.can_redo())));
        }
        if let Some(v) = pdf {
            menus.push(
                Menu::new("Edit")
                    .item("Undo", "undo")
                    .key("z")
                    .enabled(v.can_undo())
                    .item("Redo", "redo")
                    .shift_key("z")
                    .enabled(v.can_redo())
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
            // What decodes it: the default (Preferences ▸ GPUs), or one
            // chosen for it; and what does now.
            let (with, now) = (prev.read(cx).decode_with(), prev.read(cx).decoding());
            use olo_preview::video::Device;
            let nvidia = olo_video::nvdec::available();
            let decode = Menu::new("Decode With")
                .item(format!("Decoding on {}", now.unwrap_or("…")), "")
                .enabled(false)
                .separator()
                .item("Default (Preferences ▸ GPUs)", "decode-default")
                .checked(with == Device::Default)
                .item("Automatic", "decode-auto")
                .checked(with == Device::Automatic)
                .item("NVIDIA GPU (NVDEC)", "decode-nvidia")
                .checked(with == Device::Nvidia)
                .enabled(nvidia)
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
                    .submenu(decode)
                    .separator()
                    .item(full_screen, "fullscreen")
                    .keys("F"),
            );
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
            let sign = self.viewer.upgrade().map_or_else(|| Menu::new("Sign"), |viewer| viewer.read(cx).sign_menu("Sign"));
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

    fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            "open" => return self.ask_panel(Purpose::Open, cx),
            "save" => return self.save(After::Nothing, cx),
            "save-as" => return self.ask_panel(Purpose::SaveAs, cx),
            "export" => return self.ask_panel(Purpose::Export, cx),
            "close" => return self.leave(After::Close, window, cx),
            "quit" => {
                if let Some(viewer) = self.viewer.upgrade() {
                    cx.defer(move |cx| {
                        let docs = viewer.read(cx).docs.clone();
                        for d in docs {
                            let win = d.read(cx).win;
                            if let Some(win) = win {
                                let _ = win.update(cx, |_, window, cx| d.update(cx, |d, cx| d.leave(After::Close, window, cx)));
                            }
                        }
                    });
                }
                return;
            }
            "prev-file" => return self.step(-1, window, cx),
            "next-file" => return self.step(1, window, cx),
            "new-signature" => return self.new_signature(cx),
            c if c == olo_ui::tracker::COMMAND => {
                if let Err(e) = olo_ui::tracker::open(&self.path) {
                    tracing::warn!("couldn't show {} in Tracker: {e}", self.path.display());
                }
                return;
            }
            _ => {}
        }
        if let Some(k) = command.strip_prefix("unsign:").and_then(|k| k.parse::<usize>().ok()) {
            if let Some(viewer) = self.viewer.upgrade() {
                viewer.update(cx, |v, _| {
                    if k < v.signatures.len() {
                        v.signatures.remove(k);
                        signature::store(&v.signatures);
                    }
                });
            }
            return;
        }
        if matches!(command, "trim" | "trim-cancel" | "trim-save") {
            return self.trim_command(command, cx);
        }
        if command.starts_with("edit-") {
            return self.video_edit_command(command, window, cx);
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
            cx.notify();
            return;
        }
        // A chapter of the video: where it starts, the one after the
        // position, or before it (within a second of a chapter's start:
        // the one before that, as players do).
        if (command.starts_with("chapter-") || command == "next-chapter" || command == "prev-chapter")
            && let Content::Other(prev) = &self.content
        {
            let prev = prev.clone();
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
            return self.picture_command(command, window, cx);
        }
        // A text or a formatted document: its text copied, all of it
        // selected, found.
        if let Content::Other(prev) = &self.content
            && prev.read(cx).has_text()
            && matches!(command, "copy" | "select-all" | "find" | "find-next" | "find-previous")
        {
            let prev = prev.clone();
            prev.update(cx, |p, cx| match command {
                "copy" => p.copy(cx),
                "select-all" => p.select_all(cx),
                "find" => p.open_find(window, cx),
                "find-next" => p.find_step(false, cx),
                _ => p.find_step(true, cx),
            });
            return;
        }
        if command == "fullscreen" {
            window.toggle_fullscreen();
            cx.notify();
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
            cx.notify();
            return;
        }
        if let Content::Other(prev) = &self.content {
            let image = prev.read(cx).image_view().clone();
            image.update(cx, |v, cx| match command {
                "zoom-in" => v.zoom(Some(true), cx),
                "zoom-out" => v.zoom(Some(false), cx),
                "zoom-100" => v.set_scale(1.0, cx),
                "zoom-fit" => v.zoom(None, cx),
                _ => {}
            });
            cx.notify();
            return;
        }
        let sig = command.strip_prefix("sign:").and_then(|k| k.parse::<usize>().ok()).and_then(|k| self.viewer.upgrade().and_then(|v| v.read(cx).signatures.get(k).cloned()));
        let Content::Pdf(v) = &self.content else { return };
        v.update(cx, |v, cx| match command {
            "undo" => v.undo(cx),
            "redo" => v.redo(cx),
            "delete" => v.delete_selection(cx),
            "copy" => v.copy(cx),
            "select-all" => v.select_all(cx),
            "find" => v.open_find(window, cx),
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
                    cx.notify();
                }
            }
            _ => {
                if let Some(s) = sig {
                    v.place_signature(s.strokes, s.aspect, cx);
                }
            }
        });
        self.update_title(cx);
        cx.notify();
    }

    fn new_signature(&mut self, cx: &mut Context<Self>) {
        let (doc, viewer) = (cx.entity(), self.viewer.clone());
        cx.defer(move |cx| {
            let pad_done = move |sig: Option<Signature>, window: &mut Window, cx: &mut App| {
                window.remove_window();
                let Some(sig) = sig else { return };
                if let Some(viewer) = viewer.upgrade() {
                    viewer.update(cx, |v, _| {
                        v.signatures.push(sig.clone());
                        signature::store(&v.signatures);
                    });
                }
                doc.update(cx, |d, cx| {
                    if let Content::Pdf(v) = &d.content {
                        v.update(cx, |v, cx| v.place_signature(sig.strokes, sig.aspect, cx));
                    }
                    if let Some(win) = d.win {
                        cx.defer(move |cx| {
                            olo_ui::raise(Some(win), cx);
                        });
                    }
                });
            };
            let (w, h) = signature::PAD_SIZE;
            olo_ui::open_window(cx, WindowOptions::new("New Signature", w, h).min_size(360.0, 200.0).child(), move |_, cx| cx.new(|cx| Pad::new(cx, pad_done)));
        });
    }

    /// Editing a picture (an Image menu or Edit menu command).
    fn picture_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.picture(cx) {
            return;
        }
        self.ensure_editor(cx);
        let Content::Other(prev) = &self.content else { return };
        let image = prev.read(cx).image_view().clone();
        let e = self.edit.as_mut().expect("made");
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
                let size = e.editor.size();
                let doc = cx.entity();
                cx.defer(move |cx| {
                    olo_ui::open_window(cx, WindowOptions::new("Adjust Size", 300.0, 168.0).min_size(300.0, 168.0).child(), move |window, cx| cx.new(|cx| Resize::new(doc, size, window, cx)));
                });
                return;
            }
            _ => {}
        }
        // Rotating or flipping mid-crop would leave the frame on the old picture.
        if command != "crop" && command != "crop-cancel" && e.crop.take().is_some() {
            image.update(cx, |v, _| v.set_still(false));
        }
        let _ = window;
        self.update_title(cx);
        cx.notify();
    }

    /// Following a panel (`--follow`): the arrows step through the
    /// panel's list (it moves its selection, and says what to show), as
    /// Quick Look's do.
    fn follow_keys(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = e.keystroke.key.as_str();
        let follow = self.viewer.upgrade().is_some_and(|v| v.read(cx).follow);
        if !follow || e.keystroke.modifiers.modified() || !matches!(key, "up" | "down" | "left" | "right") {
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

    fn keys(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = e.keystroke.key.as_str();
        let plain = !e.keystroke.modifiers.modified();
        let look = self.viewer.upgrade().is_some_and(|v| v.read(cx).look);
        // A look: Space or Escape puts it away.
        if look && plain && matches!(key, "space" | "escape") {
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
                    self.video_edit_command("edit-cut", window, cx);
                    true
                }
                "enter" if e.cropping.is_some() => {
                    self.video_edit_command("edit-crop", window, cx);
                    true
                }
                "escape" if e.cropping.is_some() => {
                    e.cropping = None;
                    cx.notify();
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
                "escape" => self.picture_command("crop-cancel", window, cx),
                "enter" if has_sel => self.picture_command("crop", window, cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        // A picture or a video: F full screen and back, Escape back.
        if plain && matches!(&self.content, Content::Other(p) if p.read(cx).is_image() || p.read(cx).is_video()) {
            match key {
                "f" => window.toggle_fullscreen(),
                "escape" if window.is_fullscreen() => window.toggle_fullscreen(),
                _ => {}
            }
            if key == "f" || (key == "escape" && self.fullscreen) {
                cx.notify();
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
                        prev.update(cx, |p, cx| p.skip(-olo_preview::ui::PreviewView::SKIP_SECS, cx));
                    }
                    "right" if media => {
                        prev.update(cx, |p, cx| p.skip(olo_preview::ui::PreviewView::SKIP_SECS, cx));
                    }
                    "left" | "up" => self.step(-1, window, cx),
                    "right" | "down" => self.step(1, window, cx),
                    "space" | "p" => {
                        prev.update(cx, |p, cx| p.toggle_playback(cx));
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }
            Content::Pdf(_) if plain => {
                match key {
                    "pageup" => self.command("prev-page", window, cx),
                    "pagedown" => self.command("next-page", window, cx),
                    "delete" | "backspace" => self.command("delete", window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// The PDF toolbar: tools, Sign, zoom and where you are.
    fn toolbar(&self, v: &Entity<PdfView>, cx: &mut Context<Self>) -> AnyElement {
        let pv = v.read(cx);
        let editable = pv.editable && pv.loaded();
        let doc = cx.entity();
        let mut left = div().flex().items_center().gap(px(m::TIGHT));
        for (label, tool, cmd) in [("Select", Tool::Select, "tool-select"), ("Text", Tool::Text, "tool-text"), ("Draw", Tool::Draw, "tool-draw")] {
            let d = doc.clone();
            left = left.child(Button::new(label).min_width(64.0).pushed(editable && pv.tool == tool).enabled(editable).on_click(move |window, cx| d.update(cx, |d, cx| d.command(cmd, window, cx))));
        }
        let sign = self.viewer.upgrade().map_or_else(|| Menu::new(""), |viewer| viewer.read(cx).sign_menu(""));
        let d = doc.clone();
        left = left.child(div().w(px(m::SPACING))).child(Button::new("Sign…").min_width(72.0).enabled(editable).on_click_at(move |b, window, cx| {
            let (d, sign) = (d.clone(), sign.clone());
            olo_ui::menu::popup(window, cx, sign, b, gpui::Corner::BottomLeft, move |cmd, window, cx| d.update(cx, |d, cx| d.command(cmd, window, cx)));
        }));
        let (zin, zout) = (doc.clone(), doc.clone());
        let read_only = pv.loaded() && !pv.editable;
        // The page and zoom as the pages settle them in this frame (a
        // scroll asked for lands, a fit's zoom is worked out, as they're
        // laid out), read as they're painted.
        let (pages, zooms) = (v.clone(), v.clone());
        let info = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let dim = style::disabled_text(&p);
                let mut x = b.right();
                let pv = pages.read(cx);
                let page = pv.loaded().then(|| format!("Page {} of {}", pv.current_page + 1, pv.geoms.len()));
                if let Some(page) = &page {
                    let at = style::label(window, cx, x, b.center().y, Align::Right, page.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                    agent::record(window, cx, agent::Node::label(page.clone(), at));
                    x = at.left() - px(m::SPACING * 2.0);
                }
                if read_only && x > b.left() {
                    let at = style::label(window, cx, x, b.center().y, Align::Right, "Read only", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
                    agent::record(window, cx, agent::Node::label("Read only", at));
                }
            },
        )
        .flex_1()
        .h(px(m::CONTROL_HEIGHT));
        let zoom_label = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let pct = format!("{}%", (zooms.read(cx).zoom * 100.0).round());
                let at = style::label(window, cx, b.center().x, b.center().y, Align::Center, pct.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
                agent::record(window, cx, agent::Node::label(pct.clone(), at));
            },
        )
        .w(px(44.0))
        .h(px(m::CONTROL_HEIGHT));
        let right = div()
            .flex()
            .items_center()
            .gap(px(m::TIGHT))
            .child(Button::new("−").min_width(28.0).on_click(move |window, cx| zout.update(cx, |d, cx| d.command("zoom-out", window, cx))))
            .child(zoom_label)
            .child(Button::new("+").min_width(28.0).on_click(move |window, cx| zin.update(cx, |d, cx| d.command("zoom-in", window, cx))));
        let ground = canvas(
            |_, _, _| {},
            |b, _, window, cx| {
                let p = theme::palette(cx);
                window.paint_quad(gpui::fill(b, style::c(p.panel_background)));
                style::hline_above(window, b.left(), b.right(), b.bottom(), style::shade(&p, p.panel_background, 1.3));
            },
        )
        .absolute()
        .size_full();
        div().relative().h(px(m::TOOLBAR_HEIGHT)).w_full().px(px(m::SPACING)).flex().items_center().gap(px(m::SPACING)).child(ground).child(left).child(info).child(right).into_any_element()
    }

    /// Trim… (a video): handles on its timeline (the player's own,
    /// `PreviewView::set_trimming`), Cancel and Trim over it; Trim copies
    /// the stretch kept into "NAME (trimmed).mp4" beside it on a worker
    /// (`olo_video::trim`: nothing encoded again) and shows that.
    fn trim_command(&mut self, command: &str, cx: &mut Context<Self>) {
        let Content::Other(prev) = &self.content else { return };
        let prev = prev.clone();
        match command {
            "trim" => prev.update(cx, |p, cx| p.set_trimming(true, cx)),
            "trim-cancel" => prev.update(cx, |p, cx| p.set_trimming(false, cx)),
            _ => {
                let Some((from, to)) = prev.read(cx).trimming() else { return };
                prev.update(cx, |p, cx| {
                    p.stop();
                    p.set_trimming(false, cx);
                });
                self.trimming = true;
                let (src, id, proxy) = (self.path.clone(), cx.entity_id(), olo_ui::proxy(cx));
                std::thread::spawn(move || {
                    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let dst = src.with_file_name(format!("{stem} (trimmed).mp4"));
                    let part = dst.with_extension("mp4.part");
                    let r = olo_video::trim::trim(&src, &part, from, to).and_then(|_| std::fs::rename(&part, &dst).map_err(|e| e.to_string()));
                    if r.is_err() {
                        let _ = std::fs::remove_file(&part);
                    }
                    proxy.send(Msg::Trimmed(id, r.map(|()| dst)));
                });
            }
        }
        cx.notify();
    }

    /// While trimming: Cancel and Trim, floating over the bottom of the
    /// picture, above the timeline.
    fn trim_bar(&self, prev: &Entity<PreviewView>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (from, to) = prev.read(cx).trimming()?;
        let (trim_doc, cancel_doc) = (cx.entity(), cx.entity());
        let bw = m::BUTTON_MIN_WIDTH;
        let kept = format!("{} kept", olo_preview::media::format_duration(to - from));
        let p = theme::palette(cx);
        Some(
            div()
                .absolute()
                .bottom(px(olo_preview::ui::UNDER_VIDEO + m::SPACING))
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
                        .child(olo_ui::widgets::StringView::new(kept).color(style::c(p.desktop_text)))
                        .child(Button::new("Cancel").min_width(bw).on_click(move |_, cx| cancel_doc.update(cx, |d, cx| d.trim_command("trim-cancel", cx))))
                        .child(Button::new("Trim").default(true).min_width(bw).enabled(to - from >= 0.1).on_click(move |_, cx| trim_doc.update(cx, |d, cx| d.trim_command("trim-save", cx)))),
                )
                .into_any_element(),
        )
    }

    /// The picture's crop frame, over the preview, and its Crop and
    /// Cancel buttons.
    fn crop_overlay(&self, prev: &Entity<PreviewView>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let e = self.edit.as_ref()?;
        let crop = e.crop.clone()?;
        let full = e.editor.size();
        let image = prev.read(cx).image_view().clone();
        let (shown, view) = image.read(cx).picture_bounds()?;
        let frame = crop.clone();
        let paint = canvas(
            |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |_, hitbox, window, cx| {
                window.set_cursor_style(frame.shape(), &hitbox);
                frame.paint(window, cx, view, shown, full);
            },
        )
        .absolute()
        .inset_0();
        let doc = cx.entity();
        let (d_move, d_up) = (doc.clone(), doc.clone());
        let pointer = div()
            .id("crop")
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, cx| {
                doc.update(cx, |d, cx| {
                    if let Some(c) = d.edit.as_mut().and_then(|e| e.crop.as_mut()) {
                        c.press(e.position, shown, full);
                        d.crop_press = Some(false);
                        cx.notify();
                    }
                });
                cx.stop_propagation();
            })
            .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
                d_move.update(cx, |d, cx| {
                    let Some(c) = d.edit.as_mut().and_then(|e| e.crop.as_mut()) else { return };
                    let mut changed = c.hover(e.position, shown, full);
                    if e.pressed_button == Some(MouseButton::Left) && d.crop_press.is_some() {
                        d.crop_press = Some(true);
                        changed |= c.drag_to(e.position, shown, full);
                    }
                    if changed {
                        cx.notify();
                    }
                })
            })
            .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
                d_up.update(cx, |d, cx| {
                    let moved = d.crop_press.take().unwrap_or(false);
                    if let Some(c) = d.edit.as_mut().and_then(|e| e.crop.as_mut()) {
                        c.release(moved);
                        cx.notify();
                    }
                })
            });
        // Crop and Cancel, floating at the bottom.
        let has_sel = crop.sel.is_some();
        let (crop_doc, cancel_doc) = (cx.entity(), cx.entity());
        let bw = m::BUTTON_MIN_WIDTH;
        let buttons = div().absolute().bottom(px(m::SPACING)).left_0().right_0().flex().justify_center().child(
            div()
                .flex()
                .gap(px(m::SPACING))
                .p(px(6.0))
                .rounded(px(6.0))
                .bg(style::c(theme::palette(cx).scrim))
                .child(Button::new("Cancel").min_width(bw).on_click(move |window, cx| cancel_doc.update(cx, |d, cx| d.picture_command("crop-cancel", window, cx))))
                .child(Button::new("Crop").default(true).min_width(bw).enabled(has_sel).on_click(move |window, cx| crop_doc.update(cx, |d, cx| d.picture_command("crop", window, cx)))),
        );
        // Over the preview, not after it.
        Some(div().absolute().inset_0().child(paint).child(pointer).child(buttons).into_any_element())
    }
}

impl Render for Doc {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            window.focus(&self.focus);
        }
        self.fullscreen = window.is_fullscreen();
        // A playing video keeps the screen awake.
        let playing = matches!(&self.content, Content::Other(p) if p.read(cx).playing_video());
        if playing != self.awake {
            self.awake = playing;
            window.set_keep_awake(playing);
        }
        let me = cx.entity();
        // (Built as the document changes: not for each picture of a video.)
        let bar = MenuBar::of(&me, |d: &Doc, cx| d.menus(cx), {
            let me = me.clone();
            move |cmd, window, cx| me.update(cx, |d, cx| d.command(cmd, window, cx))
        });
        let p = theme::palette(cx);
        let body: AnyElement = match &self.content {
            Content::Loading => div().size_full().bg(style::c(p.panel_background)).into_any_element(),
            Content::Failed(e) => {
                let e = e.clone();
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        let p = theme::palette(cx);
                        window.paint_quad(gpui::fill(b, style::c(p.panel_background)));
                        let at = style::label_fit(
                            window,
                            cx,
                            b.center().x,
                            b.center().y,
                            Align::Center,
                            e.clone(),
                            style::PLAIN_SIZE,
                            style::ui_font(FontWeight::NORMAL),
                            style::disabled_text(&p),
                            b.size.width - px(2.0 * m::WINDOW_INSET),
                        );
                        agent::record(window, cx, agent::Node::label(e.clone(), at));
                    },
                )
                .size_full()
                .into_any_element()
            }
            Content::Other(prev) => {
                let prev = prev.clone();
                let editing = self.video_edit_area(&prev, window, cx);
                let mut area = div().relative().size_full().child(prev.clone());
                if let Some(overlay) = self.crop_overlay(&prev, cx) {
                    area = area.child(overlay);
                }
                // Editing a video: the editor's timeline and bar under it.
                if let Some((timeline, bar, crop)) = editing {
                    let mut video = div().relative().flex_1().min_h_0().child(prev.clone());
                    if let Some(crop) = crop {
                        video = video.child(crop);
                    }
                    area = div().size_full().flex().flex_col().child(video).child(timeline).child(bar);
                }
                if let Some(bar) = self.trim_bar(&prev, cx) {
                    area = area.child(bar);
                }
                if self.edit.as_ref().is_some_and(|e| e.editor.busy || e.editor.saving) {
                    let spin = canvas(
                        |_, _, _| {},
                        |b, _, window, cx| {
                            let p = theme::palette(cx);
                            style::spinner(window, point(b.right() - px(20.0), b.top() + px(20.0)), 9.0, style::disabled_text(&p));
                            window.request_animation_frame();
                        },
                    )
                    .absolute()
                    .inset_0();
                    area = area.child(spin);
                }
                area.into_any_element()
            }
            Content::Pdf(v) => {
                let v = v.clone();
                let toolbar = self.toolbar(&v, cx);
                let loaded = v.read(cx).loaded();
                let pages: AnyElement = if loaded { v.clone().into_any_element() } else { div().size_full().bg(style::c(p.panel_background)).into_any_element() };
                self.update_title(cx);
                div().size_full().flex().flex_col().child(toolbar).child(div().flex_1().min_h_0().child(pages)).into_any_element()
            }
        };
        let me = cx.entity();
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|d, e: &KeyDownEvent, window, cx| d.keys(e, window, cx)))
            // A look following its panel: its arrows are the panel's, before
            // what it shows (a text, a PDF) takes them to scroll.
            .capture_key_down(cx.listener(|d, e: &KeyDownEvent, _, cx| d.follow_keys(e, cx)))
            .on_drop(move |paths: &ExternalPaths, _, cx| {
                let viewer = me.read(cx).viewer.clone();
                let paths: Vec<PathBuf> = paths.paths().to_vec();
                if let Some(viewer) = viewer.upgrade() {
                    viewer.update(cx, |v, cx| {
                        for p in paths {
                            v.open_file(p, cx);
                        }
                    });
                }
            })
            // (Full screen, the bar's still there for its keys, but none
            // of it shows.)
            .child(if self.fullscreen { div().h(px(0.0)).overflow_hidden().child(bar) } else { div().child(bar) })
            .child(div().flex_1().min_h_0().child(body))
    }
}

/// Adjust Size for a picture: width and height as typed, kept in
/// proportion if asked.
struct Resize {
    doc: Entity<Doc>,
    size: (u32, u32),
    w: Entity<TextField>,
    h: Entity<TextField>,
    keep: bool,
    _subs: Vec<Subscription>,
}

impl Resize {
    fn new(doc: Entity<Doc>, size: (u32, u32), window: &mut Window, cx: &mut Context<Self>) -> Resize {
        let w = text_field(cx, |f| f.name("Width"));
        let h = text_field(cx, |f| f.name("Height"));
        w.update(cx, |f, cx| f.set_text(size.0.to_string(), cx));
        h.update(cx, |f, cx| f.set_text(size.1.to_string(), cx));
        let focus = w.read(cx).focus_handle(cx);
        window.focus(&focus);
        // Keep the other side in proportion as one is typed.
        let sw = cx.subscribe(&w, |r: &mut Resize, w, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Typed(_))
                && r.keep
                && let Ok(v) = w.read(cx).text().trim().parse::<u32>()
            {
                let ratio = r.size.1 as f64 / r.size.0.max(1) as f64;
                let h = ((v as f64 * ratio).round() as u32).max(1).to_string();
                r.h.update(cx, |f, cx| f.set_text(h, cx));
            }
            cx.notify();
        });
        let sh = cx.subscribe(&h, |r: &mut Resize, h, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Typed(_))
                && r.keep
                && let Ok(v) = h.read(cx).text().trim().parse::<u32>()
            {
                let ratio = r.size.1 as f64 / r.size.0.max(1) as f64;
                let w = ((v as f64 / ratio).round() as u32).max(1).to_string();
                r.w.update(cx, |f, cx| f.set_text(w, cx));
            }
            cx.notify();
        });
        Resize { doc, size, w, h, keep: true, _subs: vec![sw, sh] }
    }

    fn values(&self, cx: &App) -> Option<(u32, u32)> {
        match (self.w.read(cx).text().trim().parse::<u32>(), self.h.read(cx).text().trim().parse::<u32>()) {
            (Ok(a), Ok(b)) if (1..=65_535).contains(&a) && (1..=65_535).contains(&b) => Some((a, b)),
            _ => None,
        }
    }

    fn done(&mut self, apply: bool, window: &mut Window, cx: &mut Context<Self>) {
        if apply && let Some((w, h)) = self.values(cx) {
            self.doc.update(cx, |d, cx| {
                if let Some(e) = &mut d.edit {
                    e.editor.push(Op::Resize { w, h });
                    d.update_title(cx);
                    cx.notify();
                }
            });
        }
        window.remove_window();
    }
}

impl Render for Resize {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ok = self.values(cx).is_some();
        let now = format!("Now {} × {} pixels.", self.size.0, self.size.1);
        let (me_ok, me_cancel, me_keep) = (cx.entity(), cx.entity(), cx.entity());
        let field = |label: &'static str, f: Entity<TextField>| {
            div().flex().items_center().h(px(m::CONTROL_HEIGHT)).child(div().w(px(60.0)).child(label)).child(div().w(px(110.0)).child(olo_ui::widgets::TextControl::new(f)))
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .on_key_down(cx.listener(|r, e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    r.done(false, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(div().text_color(style::disabled_text(&theme::palette(cx))).child(now))
            .child(field("Width:", self.w.clone()))
            .child(field("Height:", self.h.clone()))
            .child(CheckBox::new("Keep proportions", self.keep).on_toggle(move |on, _, cx| {
                me_keep.update(cx, |r, cx| {
                    r.keep = on;
                    cx.notify();
                })
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Cancel").on_click(move |window, cx| me_cancel.update(cx, |r, cx| r.done(false, window, cx))))
                    .child(Button::new("Resize").default(true).enabled(ok).on_click(move |window, cx| me_ok.update(cx, |r, cx| r.done(true, window, cx)))),
            )
    }
}

/// Dragging the document out (from a picture that fits the window): the
/// file, and for a common picture format its bytes too, for programs that
/// take an image rather than a file.
fn file_drag(path: &Path) -> gpui::OutgoingDrag {
    let mut drag = olo_ui::dnd::files(&[path.to_path_buf()], true, false);
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
    olo_ui::run(APP_ID, move |cx| {
        if !look {
            instance::listen(APP_ID, olo_ui::proxy(cx));
        }
        pdfview::bind_keys(cx);
        let viewer = cx.new(|_| Viewer { docs: Vec::new(), signatures: signature::load(), generation: 0, watch: None, look, over, follow, panels: 0 });
        // What the panel says to show, a line at a time, on a thread.
        if follow {
            let proxy = olo_ui::proxy(cx);
            let read = std::thread::Builder::new().name("look follows".into()).spawn(move || {
                use std::io::BufRead as _;
                for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                    let path = PathBuf::from(line.trim_end());
                    if path.is_file() {
                        proxy.send(Msg::LookAt(path));
                    }
                }
            });
            if let Err(e) = read {
                tracing::warn!("a look can't follow its panel: {e}");
            }
        }
        let messages = viewer.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |v, cx| v.message(message, cx)));
        // The files open, kept for an upgrade (a look isn't kept).
        let saving = viewer.clone();
        olo_ui::on_upgrade(cx, move |cx| {
            let v = saving.read(cx);
            if v.look {
                return Some("{}".into());
            }
            let files = v.docs.iter().map(|d| d.read(cx).path.clone()).collect();
            serde_json::to_string(&Saved { files }).ok()
        });
        // Unsaved changes, or a save under way: the new version waits.
        let docs = viewer.clone();
        olo_ui::upgrade_waits_while(cx, move |cx| docs.read(cx).docs.iter().any(|d| d.read(cx).busy(cx)));
        let restored = olo_ui::restored_state().filter(|_| !look).and_then(|s| serde_json::from_str::<Saved>(&s).ok());
        viewer.update(cx, |v, cx| match restored {
            Some(saved) if !saved.files.is_empty() => {
                for f in saved.files {
                    v.open_file(f, cx);
                }
            }
            _ => v.handle_args(&args, None, cx),
        });
    });
}
