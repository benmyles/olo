//! Viewer: a window for any file Quick Look can preview (Tracker's pop-out
//! button opens one), and a PDF editor: fill in forms, add text, draw,
//! sign, then Save (appended to the original as an incremental update),
//! Save As, or Export a flattened copy. Pictures can be rotated, flipped,
//! cropped and resized (olo-preview's `edit`), then saved.

mod pdfdoc;
mod pdfview;
mod pdfwork;
mod signature;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use olo_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Align, Key, Rect, Ui, pos2, vec2},
    file_panel::{FilePanel, PANEL_SIZE, PanelResult},
    files::{self, Kind},
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, CheckBox, TextControl},
};
use olo_preview::{
    Item, Preview, Update,
    edit::{self, CropTool, EditEvent, Editor, Op},
};

use pdfview::{Fit, PdfView, Tool};
use pdfwork::Event;
use signature::{Pad, PadResult, Signature};

const APP_ID: &str = "olo.viewer";

enum Msg {
    Pdf(WindowId, Event),
    Preview(WindowId, u64, Update),
    Stat(WindowId, PathBuf, Option<Item>),
    Siblings(WindowId, PathBuf, Vec<PathBuf>),
    Edit(WindowId, EditEvent),
    /// The forms font is installed (see `style::add_document_font`).
    FontReady,
}

enum Content {
    /// Waiting for the file's details.
    Loading,
    Pdf(Box<PdfView>),
    Other(Box<Preview>),
    Failed(String),
}

/// What to do once the document is saved (or its changes discarded).
#[derive(Clone, Debug, PartialEq)]
enum After {
    Nothing,
    Close,
    Open(PathBuf),
}

struct DocWin {
    path: PathBuf,
    content: Content,
    /// The files beside it, for Previous / Next.
    siblings: Vec<PathBuf>,
    title: String,
    sized: bool,
    after_save: After,
    /// The screen is kept awake (a video is playing).
    awake: bool,
    /// A picture being edited.
    edit: Option<Box<ImageEdit>>,
}

/// Editing a picture: its steps, and the frame while cropping.
struct ImageEdit {
    editor: Editor,
    crop: Option<CropTool>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Purpose {
    Open,
    SaveAs,
    Export,
}

enum Win {
    Doc(Box<DocWin>),
    Pad { doc: WindowId, pad: Pad },
    Panel { doc: Option<WindowId>, purpose: Purpose, panel: FilePanel },
    Unsaved { doc: WindowId, after: After, message: String },
    /// Adjust Size for a picture: width and height as typed.
    Resize { doc: WindowId, w: String, h: String, keep: bool, size: (u32, u32) },
    Error { message: String },
}

struct Viewer {
    windows: HashMap<WindowId, Win>,
    signatures: Vec<Signature>,
    next_id: u64,
    generation: u64,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    files: Vec<PathBuf>,
}

fn is_pdf(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
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

impl Viewer {
    fn open_file(&mut self, cx: &mut Cx, path: PathBuf) {
        // Already open: bring it forward.
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Doc(d) if d.path == path)) {
            cx.activate(id);
            return;
        }
        let title = file_name(&path);
        let min_w = if is_pdf(&path) { pdfview::MIN_WIDTH as u32 } else { 360 };
        let id = cx.open_window(WindowOptions::new(title.clone(), 720, 560).min_size(min_w, 240));
        let doc = DocWin { path: path.clone(), content: Content::Loading, siblings: Vec::new(), title, sized: false, after_save: After::Nothing, awake: false, edit: None };
        self.windows.insert(id, Win::Doc(Box::new(doc)));
        self.load(cx, id, path);
    }

    /// (Re)load `path` into document window `id`.
    fn load(&mut self, cx: &mut Cx, id: WindowId, path: PathBuf) {
        let Some(Win::Doc(d)) = self.windows.get_mut(&id) else { return };
        d.path = path.clone();
        d.sized = false;
        d.edit = None;
        d.title = file_name(&path);
        cx.set_title(id, d.title.clone());
        let proxy = cx.proxy();
        if is_pdf(&path) {
            self.next_id += 1;
            let tx = pdfwork::start(path.clone(), move |ev| proxy.send(Msg::Pdf(id, ev)));
            d.content = Content::Pdf(Box::new(PdfView::new(self.next_id, tx)));
        } else {
            d.content = Content::Loading;
            let p = path.clone();
            std::thread::spawn(move || {
                let item = Item::stat(&p);
                proxy.send(Msg::Stat(id, p, item));
            });
        }
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let s = list_siblings(&path);
            proxy.send(Msg::Siblings(id, path, s));
        });
        cx.redraw(id);
    }

    fn close(&mut self, cx: &mut Cx, win: WindowId) {
        self.windows.remove(&win);
        cx.close_window(win);
        // Dialogs belonging to a closed document go with it.
        let orphans: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, w)| match w {
                Win::Pad { doc, .. } | Win::Unsaved { doc, .. } => *doc == win,
                Win::Panel { doc: Some(doc), .. } => *doc == win,
                _ => false,
            })
            .map(|(id, _)| *id)
            .collect();
        for o in orphans {
            self.windows.remove(&o);
            cx.close_window(o);
        }
        if !self.windows.values().any(|w| matches!(w, Win::Doc(_) | Win::Panel { .. })) {
            cx.quit();
        }
    }

    fn show_error(&mut self, cx: &mut Cx, message: String) {
        let id = cx.open_window(WindowOptions::alert("Viewer", &message));
        self.windows.insert(id, Win::Error { message });
    }

    fn open_panel(&mut self, cx: &mut Cx, doc: Option<WindowId>, purpose: Purpose) {
        let path = doc.and_then(|d| match self.windows.get(&d) {
            Some(Win::Doc(d)) => Some(d.path.clone()),
            _ => None,
        });
        let dir = path.as_deref().and_then(Path::parent).map_or_else(
            || std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
            Path::to_path_buf,
        );
        let stem = path.as_deref().and_then(Path::file_stem).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
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
        let id = cx.open_window(WindowOptions::new(title, PANEL_SIZE.0, PANEL_SIZE.1).min_size(420, 300));
        self.windows.insert(id, Win::Panel { doc, purpose, panel });
    }

    /// Save the document in place; `after` runs once it's written.
    fn save(&mut self, cx: &mut Cx, win: WindowId, after: After) {
        let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
        if let Some(e) = &mut d.edit {
            // A format we can't write (HEIC, AVIF, …): Save As instead.
            if edit::save_format(&d.path).is_none() {
                return self.open_panel(cx, Some(win), Purpose::SaveAs);
            }
            d.after_save = after;
            e.editor.save(d.path.clone());
            cx.redraw(win);
            return;
        }
        let Content::Pdf(v) = &mut d.content else { return };
        if !v.editable {
            return self.show_error(cx, "This PDF can't be edited, so it can't be saved.".into());
        }
        d.after_save = after;
        v.save(d.path.clone(), false, true);
        cx.redraw(win);
    }

    /// Close or move on from a document, asking about unsaved changes.
    fn leave(&mut self, cx: &mut Cx, win: WindowId, after: After) {
        let Some(Win::Doc(d)) = self.windows.get(&win) else { return };
        let dirty = matches!(&d.content, Content::Pdf(v) if v.dirty()) || d.edit.as_ref().is_some_and(|e| e.editor.dirty());
        if !dirty {
            return self.finish(cx, win, after);
        }
        if self.windows.values().any(|w| matches!(w, Win::Unsaved { doc, .. } if *doc == win)) {
            return;
        }
        let message = format!("Save changes to “{}” before {}?", file_name(&d.path), if after == After::Close { "closing" } else { "moving on" });
        let id = cx.open_window(WindowOptions::alert("Viewer", &message));
        self.windows.insert(id, Win::Unsaved { doc: win, after, message });
    }

    fn finish(&mut self, cx: &mut Cx, win: WindowId, after: After) {
        match after {
            After::Nothing => {}
            After::Close => self.close(cx, win),
            After::Open(p) => self.load(cx, win, p),
        }
    }

    /// Previous (-1) or next (+1) file in the folder.
    fn step(&mut self, cx: &mut Cx, win: WindowId, d: isize) {
        let Some(Win::Doc(doc)) = self.windows.get(&win) else { return };
        let Some(i) = doc.siblings.iter().position(|p| *p == doc.path) else { return };
        let j = i as isize + d;
        if j < 0 || j as usize >= doc.siblings.len() {
            return;
        }
        let next = doc.siblings[j as usize].clone();
        self.leave(cx, win, After::Open(next));
    }

    fn doc_menus(&self, d: &DocWin) -> Vec<Menu> {
        let pdf = match &d.content {
            Content::Pdf(v) => Some(v),
            _ => None,
        };
        let editable = pdf.is_some_and(|v| v.editable && v.loaded());
        let pos = d.siblings.iter().position(|p| *p == d.path);
        let mut file = Menu::new("File")
            .item("Open…", "open", Some(Key::O))
            .separator()
            .item("Save", "save", Some(Key::S))
            .enabled(editable && pdf.is_some_and(|v| v.dirty() && !v.saving))
            .item_shift("Save As…", "save-as", Key::S)
            .enabled(editable)
            .item("Export Flattened PDF…", "export", None)
            .enabled(editable)
            .separator()
            .item("Show in Tracker", "show-in-tracker", None)
            .separator()
            .item("Close", "close", Some(Key::W))
            .item("Quit", "quit", Some(Key::Q));
        let picture = matches!(&d.content, Content::Other(prev) if prev.is_image()) && edit::editable(&d.path);
        let editor = d.edit.as_ref().map(|e| &e.editor);
        if pdf.is_none() {
            file = Menu::new("File").item("Open…", "open", Some(Key::O)).separator();
            if picture {
                let dirty = editor.is_some_and(|e| e.dirty() && !e.saving);
                file = file
                    .item("Save", "save", Some(Key::S))
                    .enabled(dirty)
                    .item_shift("Save As…", "save-as", Key::S)
                    .item("Revert to Saved", "revert", None)
                    .enabled(dirty)
                    .separator();
            }
            file = file.item("Show in Tracker", "show-in-tracker", None).separator().item("Close", "close", Some(Key::W)).item("Quit", "quit", Some(Key::Q));
        }
        let mut menus = vec![file];
        if picture {
            menus.push(
                Menu::new("Edit")
                    .item("Undo", "undo", Some(Key::Z))
                    .enabled(editor.is_some_and(|e| e.can_undo()))
                    .item_shift("Redo", "redo", Key::Z)
                    .enabled(editor.is_some_and(|e| e.can_redo())),
            );
        }
        if let Some(v) = pdf {
            menus.push(
                Menu::new("Edit")
                    .item("Undo", "undo", Some(Key::Z))
                    .enabled(v.can_undo())
                    .item_shift("Redo", "redo", Key::Z)
                    .enabled(v.can_redo())
                    .separator()
                    .item("Delete", "delete", None)
                    .enabled(v.has_selection()),
            );
            menus.push(
                Menu::new("View")
                    .item("Zoom In", "zoom-in", Some(Key::Equals))
                    .item("Zoom Out", "zoom-out", Some(Key::Minus))
                    .item("Actual Size", "zoom-100", Some(Key::Num0))
                    .separator()
                    .item("Fit Width", "fit-width", None)
                    .checked(v.fit == Some(Fit::Width))
                    .item("Fit Page", "fit-page", None)
                    .checked(v.fit == Some(Fit::Page)),
            );
        }
        if let Content::Other(prev) = &d.content
            && prev.is_image()
        {
            menus.push(
                Menu::new("View")
                    .item("Zoom In", "zoom-in", Some(Key::Equals))
                    .item("Zoom Out", "zoom-out", Some(Key::Minus))
                    .item("Actual Size", "zoom-100", Some(Key::Num0))
                    .checked(!prev.image_fitted() && (prev.image_scale() - 1.0).abs() < 1e-3)
                    .item("Zoom to Fit", "zoom-fit", Some(Key::Num9))
                    .checked(prev.image_fitted()),
            );
        }
        if picture {
            let cropping = d.edit.as_ref().is_some_and(|e| e.crop.is_some());
            menus.push(
                Menu::new("Image")
                    .item("Rotate Left", "rotate-left", Some(Key::L))
                    .item("Rotate Right", "rotate-right", Some(Key::R))
                    .item("Flip Horizontal", "flip-h", None)
                    .item("Flip Vertical", "flip-v", None)
                    .separator()
                    .item(if cropping { "Crop to Selection" } else { "Crop…" }, "crop", Some(Key::K))
                    .enabled(!cropping || d.edit.as_ref().and_then(|e| e.crop.as_ref()).is_some_and(|c| c.sel.is_some()))
                    .item("Adjust Size…", "resize", None),
            );
        }
        let mut go = Menu::new("Go")
            .item("Previous File", "prev-file", Some(Key::OpenBracket))
            .enabled(pos.is_some_and(|i| i > 0))
            .item("Next File", "next-file", Some(Key::CloseBracket))
            .enabled(pos.is_some_and(|i| i + 1 < d.siblings.len()));
        if let Some(v) = pdf {
            let n = v.geoms.len();
            go = go
                .separator()
                .item("Previous Page", "prev-page", None)
                .enabled(v.current_page > 0)
                .item("Next Page", "next-page", None)
                .enabled(v.current_page + 1 < n)
                .item("First Page", "first-page", None)
                .item("Last Page", "last-page", None);
        }
        menus.push(go);
        if let Some(v) = pdf
            && editable
        {
            let mut ink = Menu::new("Ink Colour");
            for (k, (name, rgb)) in pdfview::INKS.iter().enumerate() {
                ink = ink.item(*name, format!("ink:{k}"), None).checked(v.ink == *rgb);
            }
            menus.push(
                Menu::new("Tools")
                    .item("Select", "tool-select", Some(Key::Num1))
                    .checked(v.tool == Tool::Select)
                    .item("Add Text", "tool-text", Some(Key::Num2))
                    .checked(v.tool == Tool::Text)
                    .item("Draw", "tool-draw", Some(Key::Num3))
                    .checked(v.tool == Tool::Draw)
                    .separator()
                    .submenu(ink)
                    .separator()
                    .submenu(self.sign_menu("Sign")),
            );
        }
        menus
    }

    fn sign_menu(&self, title: &str) -> Menu {
        let mut sign = Menu::new(title);
        for (k, s) in self.signatures.iter().enumerate() {
            sign = sign.item(signature::label(s, k), format!("sign:{k}"), None);
        }
        if !self.signatures.is_empty() {
            sign = sign.separator();
        }
        sign = sign.item("New Signature…", "new-signature", None);
        if !self.signatures.is_empty() {
            let mut del = Menu::new("Delete Signature");
            for (k, s) in self.signatures.iter().enumerate() {
                del = del.item(signature::label(s, k), format!("unsign:{k}"), None);
            }
            sign = sign.submenu(del);
        }
        sign
    }

    fn doc_command(&mut self, cx: &mut Cx, win: WindowId, command: &str) {
        match command {
            "open" => return self.open_panel(cx, Some(win), Purpose::Open),
            "save" => return self.save(cx, win, After::Nothing),
            "save-as" => return self.open_panel(cx, Some(win), Purpose::SaveAs),
            "export" => return self.open_panel(cx, Some(win), Purpose::Export),
            "close" => return self.leave(cx, win, After::Close),
            "quit" => {
                let docs: Vec<WindowId> = self.windows.iter().filter(|(_, w)| matches!(w, Win::Doc(_))).map(|(id, _)| *id).collect();
                for d in docs {
                    self.leave(cx, d, After::Close);
                }
                return;
            }
            "prev-file" => return self.step(cx, win, -1),
            "next-file" => return self.step(cx, win, 1),
            "new-signature" => {
                let id = cx.open_window(WindowOptions::new("New Signature", signature::PAD_SIZE.0, signature::PAD_SIZE.1).min_size(360, 200));
                self.windows.insert(id, Win::Pad { doc: win, pad: Pad::default() });
                return;
            }
            _ => {}
        }
        if let Some(k) = command.strip_prefix("unsign:").and_then(|k| k.parse::<usize>().ok()) {
            if k < self.signatures.len() {
                self.signatures.remove(k);
                signature::store(&self.signatures);
            }
            return;
        }
        let sig = command.strip_prefix("sign:").and_then(|k| k.parse::<usize>().ok()).and_then(|k| self.signatures.get(k)).cloned();
        let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
        if command == "show-in-tracker" {
            let q = format!("'{}'", d.path.parent().unwrap_or(Path::new("/")).display().to_string().replace('\'', "'\\''"));
            if let Err(e) = olo_kit::spawn_detached(&format!("olo-tracker {q}")) {
                tracing::warn!("tracker: {e}");
            }
            return;
        }
        if matches!(command, "rotate-left" | "rotate-right" | "flip-h" | "flip-v" | "crop" | "crop-cancel" | "resize" | "revert")
            || (d.edit.is_some() && matches!(command, "undo" | "redo"))
        {
            return self.picture_command(cx, win, command);
        }
        if let Content::Other(prev) = &mut d.content {
            match command {
                "zoom-in" => prev.zoom(Some(true)),
                "zoom-out" => prev.zoom(Some(false)),
                "zoom-100" => prev.set_image_scale(1.0),
                "zoom-fit" => prev.zoom(None),
                _ => return,
            }
            cx.redraw(win);
            return;
        }
        let Content::Pdf(v) = &mut d.content else { return };
        match command {
            "undo" => v.undo(),
            "redo" => v.redo(),
            "delete" => v.delete_selection(),
            "zoom-in" => v.zoom_step(true),
            "zoom-out" => v.zoom_step(false),
            "zoom-100" => v.set_zoom(1.0, None),
            "fit-width" => v.fit = Some(Fit::Width),
            "fit-page" => v.fit = Some(Fit::Page),
            "prev-page" => v.go_to_page(v.current_page.saturating_sub(1)),
            "next-page" => v.go_to_page(v.current_page + 1),
            "first-page" => v.go_to_page(0),
            "last-page" => v.go_to_page(v.geoms.len().saturating_sub(1)),
            "tool-select" => v.set_tool(Tool::Select),
            "tool-text" => v.set_tool(Tool::Text),
            "tool-draw" => v.set_tool(Tool::Draw),
            c if c.starts_with("ink:") => {
                if let Some((_, rgb)) = c[4..].parse::<usize>().ok().and_then(|k| pdfview::INKS.get(k)) {
                    v.ink = *rgb;
                }
            }
            _ => {
                if let Some(s) = sig {
                    v.place_signature(s.strokes, s.aspect);
                }
            }
        }
        cx.redraw(win);
    }

    /// Editing a picture (an Image menu or Edit menu command).
    fn picture_command(&mut self, cx: &mut Cx, win: WindowId, command: &str) {
        let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
        let Content::Other(prev) = &mut d.content else { return };
        if !prev.is_image() || !edit::editable(&d.path) {
            return;
        }
        if d.edit.is_none() {
            let Some(olo_preview::Body::Image { full, .. }) = prev.body() else { return };
            let proxy = cx.proxy();
            let editor = Editor::new(d.path.clone(), (full[0] as u32, full[1] as u32), move |ev| proxy.send(Msg::Edit(win, ev)));
            d.edit = Some(Box::new(ImageEdit { editor, crop: None }));
        }
        let e = d.edit.as_mut().expect("made");
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
                    prev.set_still(false);
                }
                None => {
                    e.crop = Some(CropTool::default());
                    prev.set_still(true);
                    prev.zoom(None);
                }
            },
            "crop-cancel" => {
                e.crop = None;
                prev.set_still(false);
            }
            "resize" => {
                let size = e.editor.size();
                let id = cx.open_window(WindowOptions::new("Adjust Size", 300, 168).min_size(300, 168));
                self.windows.insert(id, Win::Resize { doc: win, w: size.0.to_string(), h: size.1.to_string(), keep: true, size });
                return;
            }
            _ => {}
        }
        // Rotating or flipping mid-crop would leave the frame on the old picture.
        if command != "crop"
            && e.crop.take().is_some()
        {
            prev.set_still(false);
        }
        cx.redraw(win);
    }

    fn resize_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut Ui) {
        let Some(Win::Resize { doc, w, h, keep, size }) = self.windows.get_mut(&win) else { return };
        let (doc, size) = (*doc, *size);
        let (before_w, before_h) = (w.clone(), h.clone());
        let mut done: Option<bool> = None;
        widgets::window_content(ui, |ui| {
            ui.spacing_mut().item_spacing.y = m::SPACING;
            ui.add(widgets::StringView::new(&format!("Now {} × {} pixels.", size.0, size.1)).dim(true));
            ui.add(TextControl::new(w).label("Width:").divider(60.0).width(170.0).autofocus(true));
            ui.add(TextControl::new(h).label("Height:").divider(60.0).width(170.0));
            ui.add(CheckBox::new(keep, "Keep proportions"));
            widgets::button_row(ui, |ui| {
                let ok = matches!((w.trim().parse::<u32>(), h.trim().parse::<u32>()), (Ok(a), Ok(b)) if (1..=65_535).contains(&a) && (1..=65_535).contains(&b));
                if ui.add_enabled(ok, Button::new("Resize").default(true)).clicked() {
                    done = Some(true);
                }
                if ui.add(Button::new("Cancel")).clicked() {
                    done = Some(false);
                }
            });
        });
        // Keep the other side in proportion as one is typed.
        if *keep {
            let ratio = size.1 as f64 / size.0.max(1) as f64;
            if *w != before_w
                && let Ok(v) = w.trim().parse::<u32>()
            {
                *h = ((v as f64 * ratio).round() as u32).max(1).to_string();
            } else if *h != before_h
                && let Ok(v) = h.trim().parse::<u32>()
            {
                *w = ((v as f64 / ratio).round() as u32).max(1).to_string();
            }
        }
        ui.input(|i| {
            if i.key_pressed(Key::Escape) {
                done = Some(false);
            }
        });
        let Some(apply) = done else { return };
        let op = match (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
            (Ok(w), Ok(h)) if apply => Some(Op::Resize { w, h }),
            _ => None,
        };
        self.windows.remove(&win);
        cx.close_window(win);
        if let Some(op) = op
            && let Some(Win::Doc(d)) = self.windows.get_mut(&doc)
            && let Some(e) = &mut d.edit
        {
            e.editor.push(op);
            cx.redraw(doc);
        }
    }

    fn doc_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut Ui) {
        let sign_menu = self.sign_menu("");
        let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
        let p = style::palette(ui.ctx());
        let full = ui.max_rect();
        let mut command: Option<String> = None;
        match &mut d.content {
            Content::Loading => {
                ui.painter().rect_filled(full, 0.0, c(p.panel_background));
            }
            Content::Failed(e) => {
                ui.painter().rect_filled(full, 0.0, c(p.panel_background));
                style::label(ui.painter(), full.center().x, full.center().y, Align::Center, e.as_str(), style::plain_font(), style::disabled_text(&p));
            }
            Content::Other(prev) => {
                prev.ui(ui, full);
                if prev.take_drag_out() {
                    cx.start_drag(win, file_drag(ui.ctx(), &d.path));
                }
                if prev.playing_video() != d.awake {
                    d.awake = !d.awake;
                    cx.keep_awake(win, d.awake);
                }
                let cropping = d.edit.as_ref().is_some_and(|e| e.crop.is_some());
                if let Some(e) = &mut d.edit {
                    let size = e.editor.size();
                    if let (Some(crop), Some((shown, area))) = (&mut e.crop, prev.image_rect()) {
                        crop.ui(ui, area, shown, size);
                        // Crop and Cancel, floating at the bottom.
                        let bw = m::BUTTON_MIN_WIDTH;
                        let y = area.bottom() - m::CONTROL_HEIGHT - m::SPACING;
                        let x = area.center().x;
                        let crop_r = Rect::from_min_size(pos2(x + m::SPACING / 2.0, y), vec2(bw, m::CONTROL_HEIGHT));
                        let cancel_r = Rect::from_min_size(pos2(x - m::SPACING / 2.0 - bw, y), vec2(bw, m::CONTROL_HEIGHT));
                        let back = cancel_r.union(crop_r).expand(6.0);
                        ui.painter().rect_filled(back, 6.0, style::c(style::palette(ui.ctx()).scrim));
                        if ui.put(cancel_r, Button::new("Cancel")).clicked() {
                            command = Some("crop-cancel".into());
                        }
                        if ui.add_enabled_ui(crop.sel.is_some(), |ui| ui.put(crop_r, Button::new("Crop").default(true))).inner.clicked() {
                            command = Some("crop".into());
                        }
                        ui.input(|i| {
                            if i.key_pressed(Key::Escape) {
                                command = Some("crop-cancel".into());
                            } else if i.key_pressed(Key::Enter) && crop.sel.is_some() {
                                command = Some("crop".into());
                            }
                        });
                    }
                    if e.editor.busy || e.editor.saving {
                        let r = Rect::from_min_size(pos2(full.right() - 30.0, full.top() + 10.0), vec2(20.0, 20.0));
                        ui.put(r, egui::Spinner::new().size(18.0).color(style::disabled_text(&p)));
                    }
                    let title = format!("{}{}", file_name(&d.path), if e.editor.dirty() { " — Edited" } else { "" });
                    if title != d.title {
                        d.title = title.clone();
                        cx.set_title(win, title);
                    }
                }
                if !cropping && !ui.ctx().egui_wants_keyboard_input() && prev.arrows_free() {
                    ui.input(|i| {
                        if i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp) {
                            command = Some("prev-file".into());
                        } else if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown) {
                            command = Some("next-file".into());
                        } else if i.key_pressed(Key::Space) {
                            prev.toggle_playback();
                        }
                    });
                }
            }
            Content::Pdf(v) => {
                let bar = Rect::from_min_size(full.min, vec2(full.width(), m::TOOLBAR_HEIGHT));
                let body = Rect::from_min_max(pos2(full.left(), bar.bottom() + style::hairline(ui.ctx())), full.max);
                command = toolbar(ui, bar, v, sign_menu);
                if v.loaded() {
                    v.ui(ui, body);
                } else {
                    ui.painter().rect_filled(body, 0.0, c(p.panel_background));
                }
                if !ui.ctx().egui_wants_keyboard_input() {
                    ui.input(|i| {
                        if i.key_pressed(Key::PageUp) {
                            command = Some("prev-page".into());
                        } else if i.key_pressed(Key::PageDown) {
                            command = Some("next-page".into());
                        }
                    });
                }
                // Keep the title in step with unsaved changes.
                let title = format!("{}{}", file_name(&d.path), if v.dirty() { " — Edited" } else { "" });
                if title != d.title {
                    d.title = title.clone();
                    cx.set_title(win, title);
                }
            }
        }
        if let Some(cmd) = command {
            self.doc_command(cx, win, &cmd);
        }
    }

    fn handle_args(&mut self, cx: &mut Cx, args: &[String], cwd: Option<&Path>) {
        let mut opened = false;
        for a in args.iter().filter(|a| !a.starts_with("--")) {
            let p = PathBuf::from(a);
            let p = match (p.is_absolute(), cwd) {
                (false, Some(cwd)) => cwd.join(p),
                _ => p,
            };
            self.open_file(cx, p);
            opened = true;
        }
        if !opened && !self.windows.values().any(|w| matches!(w, Win::Doc(_))) {
            self.open_panel(cx, None, Purpose::Open);
        }
    }
}

/// The PDF toolbar: tools, Sign, zoom and where you are.
fn toolbar(ui: &mut Ui, bar: Rect, v: &mut PdfView, sign_menu: Menu) -> Option<String> {
    let p = style::palette(ui.ctx());
    let painter = ui.painter().clone();
    painter.rect_filled(bar, 0.0, c(p.panel_background));
    style::hline(&painter, bar.left(), bar.right(), bar.bottom(), style::shade(&p, p.panel_background, 1.3));
    let y = bar.top() + m::TIGHT;
    let mut x = bar.left() + m::SPACING;
    let mut command = None;
    let button = |ui: &mut Ui, x: &mut f32, label: &str, w: f32, pushed: bool, enabled: bool| {
        let r = Rect::from_min_size(pos2(*x, y), vec2(w, m::CONTROL_HEIGHT));
        *x += w + m::TIGHT;
        ui.add_enabled_ui(enabled, |ui| ui.put(r, Button::new(label).min_width(w).pushed(pushed))).inner
    };
    let editable = v.editable && v.loaded();
    for (label, tool, cmd) in [("Select", Tool::Select, "tool-select"), ("Text", Tool::Text, "tool-text"), ("Draw", Tool::Draw, "tool-draw")] {
        if button(ui, &mut x, label, 64.0, editable && v.tool == tool, editable).clicked() {
            command = Some(cmd.to_string());
        }
    }
    x += m::SPACING;
    let sign = button(ui, &mut x, "Sign…", 72.0, false, editable);
    let sign_id = ui.make_persistent_id(("sign-popup", v as *const PdfView as usize));
    if sign.clicked() {
        widgets::popup_menu_at(ui.ctx(), sign_id, sign.rect.left_bottom(), sign_menu);
    }
    if let Some(cmd) = widgets::popup_result(ui.ctx(), sign_id) {
        command = Some(cmd);
    }
    // Right side: page, then zoom.
    let mut rx = bar.right() - m::SPACING;
    let rbutton = |ui: &mut Ui, rx: &mut f32, label: &str| {
        let w = 28.0;
        *rx -= w;
        let r = Rect::from_min_size(pos2(*rx, y), vec2(w, m::CONTROL_HEIGHT));
        *rx -= m::TIGHT;
        ui.put(r, Button::new(label).min_width(w)).clicked()
    };
    if rbutton(ui, &mut rx, "+") {
        command = Some("zoom-in".into());
    }
    let pct = format!("{}%", (v.zoom * 100.0).round());
    let pw = 44.0;
    style::label(&painter, rx - pw / 2.0, bar.center().y, Align::Center, &pct, style::plain_font(), c(p.panel_text));
    rx -= pw + m::TIGHT;
    if rbutton(ui, &mut rx, "−") {
        command = Some("zoom-out".into());
    }
    if v.loaded() {
        let page = format!("Page {} of {}", v.current_page + 1, v.geoms.len());
        let lw = style::label_width(ui.ctx(), &page, style::plain_font());
        let px = rx - m::SPACING - lw;
        if px > x + m::SPACING {
            style::label(&painter, px, bar.center().y, Align::Min, &page, style::plain_font(), style::disabled_text(&p));
        }
        if !v.editable {
            let note = "Read only";
            let nx = px - m::SPACING * 2.0 - style::label_width(ui.ctx(), note, style::plain_font());
            if nx > x {
                style::label(&painter, nx, bar.center().y, Align::Min, note, style::plain_font(), style::disabled_text(&p));
            }
        }
    }
    command
}

impl App for Viewer {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        if matches!(self.windows.get(&window), Some(Win::Doc(_))) {
            olo_kit::dnd::open_files(ui, "open");
        }
        match self.windows.get_mut(&window) {
            Some(Win::Doc(_)) => self.doc_ui(cx, window, ui),
            Some(Win::Resize { .. }) => self.resize_ui(cx, window, ui),
            Some(Win::Pad { doc, pad }) => {
                let doc = *doc;
                match pad.ui(ui) {
                    Some(PadResult::Done(sig)) => {
                        self.signatures.push(sig.clone());
                        signature::store(&self.signatures);
                        self.windows.remove(&window);
                        cx.close_window(window);
                        if let Some(Win::Doc(d)) = self.windows.get_mut(&doc)
                            && let Content::Pdf(v) = &mut d.content
                        {
                            v.place_signature(sig.strokes, sig.aspect);
                            cx.activate(doc);
                            cx.redraw(doc);
                        }
                    }
                    Some(PadResult::Cancel) => {
                        self.windows.remove(&window);
                        cx.close_window(window);
                    }
                    None => {}
                }
            }
            Some(Win::Panel { doc, purpose, panel }) => {
                let (doc, purpose) = (*doc, *purpose);
                let Some(result) = panel.ui(ui) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                let PanelResult::Chosen(path) = result else {
                    if !self.windows.values().any(|w| matches!(w, Win::Doc(_) | Win::Panel { .. })) {
                        cx.quit();
                    }
                    return;
                };
                match purpose {
                    Purpose::Open => self.open_file(cx, path),
                    Purpose::SaveAs if doc.is_some_and(|d| matches!(self.windows.get(&d), Some(Win::Doc(d)) if !is_pdf(&d.path))) => {
                        let doc = doc.expect("checked");
                        // A picture: write it there (with any edits), then show that file.
                        if edit::save_format(&path).is_none() {
                            return self.show_error(cx, "Pictures can be saved as PNG, JPEG, WebP, BMP, TIFF or GIF.".into());
                        }
                        let proxy = cx.proxy();
                        let Some(Win::Doc(d)) = self.windows.get_mut(&doc) else { return };
                        if d.edit.is_none()
                            && let Content::Other(prev) = &d.content
                            && let Some(olo_preview::Body::Image { full, .. }) = prev.body()
                        {
                            let editor = Editor::new(d.path.clone(), (full[0] as u32, full[1] as u32), move |ev| proxy.send(Msg::Edit(doc, ev)));
                            d.edit = Some(Box::new(ImageEdit { editor, crop: None }));
                        }
                        if let Some(e) = &mut d.edit {
                            e.editor.save(path);
                        }
                        cx.redraw(doc);
                    }
                    Purpose::SaveAs | Purpose::Export => {
                        if let Some(doc) = doc
                            && let Some(Win::Doc(d)) = self.windows.get_mut(&doc)
                            && let Content::Pdf(v) = &mut d.content
                        {
                            let flatten = purpose == Purpose::Export;
                            d.after_save = After::Nothing;
                            v.save(path, flatten, !flatten);
                            cx.redraw(doc);
                        }
                    }
                }
            }
            Some(Win::Unsaved { doc, after, message }) => {
                let (doc, after, message) = (*doc, after.clone(), message.clone());
                let Some(choice) = widgets::alert(ui, "warning", &message, &["Cancel", "Don't Save", "Save"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                match choice {
                    // Don't Save: closing or loading another file drops the edits.
                    1 => self.finish(cx, doc, after),
                    2 => self.save(cx, doc, after),
                    _ => {}
                }
            }
            Some(Win::Error { message }) => {
                let message = message.clone();
                if widgets::alert(ui, "warning", &message, &["OK"]).is_some() {
                    self.windows.remove(&window);
                    cx.close_window(window);
                }
            }
            None => {}
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        match self.windows.get(&window) {
            Some(Win::Doc(d)) => Some(self.doc_menus(d)),
            _ => None,
        }
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        if matches!(self.windows.get(&window), Some(Win::Doc(_))) {
            self.doc_command(cx, window, command);
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        match self.windows.get(&window) {
            Some(Win::Doc(_)) => self.leave(cx, window, After::Close),
            Some(Win::Panel { .. }) => self.close(cx, window),
            _ => {
                self.windows.remove(&window);
                cx.close_window(window);
            }
        }
    }

    fn dropped(&mut self, cx: &mut Cx, _window: WindowId, drop: olo_kit::dnd::Drop) {
        for p in drop.paths() {
            self.open_file(cx, p);
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_args(cx, &launch.args, launch.cwd.as_deref()),
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::FontReady => cx.redraw_all(),
            Msg::Edit(win, ev) => {
                let mut error = None;
                let mut next = None;
                {
                    let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
                    let Some(e) = &mut d.edit else { return };
                    e.editor.handle(&ev);
                    match ev {
                        EditEvent::Shown { image, smaller, full, note, steps } => {
                            if steps == e.editor.ops().len()
                                && let Content::Other(prev) = &mut d.content
                            {
                                prev.show_edited(image, smaller, full, note);
                            }
                        }
                        EditEvent::Saved(to) => {
                            let after = std::mem::replace(&mut d.after_save, After::Nothing);
                            next = Some((to != d.path).then_some(to.clone()).map_or(after, After::Open));
                        }
                        EditEvent::Failed(m) => {
                            d.after_save = After::Nothing;
                            error = Some(m);
                        }
                    }
                }
                if let Some(m) = error {
                    self.show_error(cx, m);
                }
                if let Some(a) = next {
                    self.finish(cx, win, a);
                }
                cx.redraw(win);
            }
            Msg::Siblings(win, path, list) => {
                if let Some(Win::Doc(d)) = self.windows.get_mut(&win)
                    && d.path == path
                {
                    d.siblings = list;
                }
            }
            Msg::Stat(win, path, item) => {
                let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
                if d.path != path {
                    return;
                }
                match item {
                    Some(item) => {
                        self.generation += 1;
                        let proxy = cx.proxy();
                        let preview = Preview::new_zoomable(item, self.generation, move |g, u| proxy.send(Msg::Preview(win, g, u)));
                        d.content = Content::Other(Box::new(preview));
                    }
                    None => d.content = Content::Failed(format!("“{}” can't be found.", file_name(&path))),
                }
                cx.redraw(win);
            }
            Msg::Preview(win, generation, update) => {
                let screen = cx.screen_size();
                let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
                let Content::Other(prev) = &mut d.content else { return };
                if prev.generation != generation {
                    return;
                }
                if prev.update(update) && !d.sized {
                    d.sized = true;
                    let size = prev.ideal_size(screen, olo_kit::menu::BAR_HEIGHT);
                    cx.resize(win, size);
                }
                cx.redraw(win);
            }
            Msg::Pdf(win, ev) => {
                let screen = cx.screen_size();
                let mut error = None;
                let mut after = None;
                {
                    let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
                    let Content::Pdf(v) = &mut d.content else { return };
                    match &ev {
                        Event::Failed(e) => error = Some(e.clone()),
                        Event::SaveFailed(e) => {
                            error = Some(format!("“{}” couldn't be saved. {e}", file_name(&d.path)));
                            d.after_save = After::Nothing;
                        }
                        Event::Saved { to, adopted } => {
                            if adopted.is_some() {
                                d.path = to.clone();
                            }
                            after = Some(std::mem::replace(&mut d.after_save, After::Nothing));
                        }
                        _ => {}
                    }
                    let opened = v.event(ev);
                    if opened && !d.sized {
                        d.sized = true;
                        let size = v.ideal_size(screen, olo_kit::menu::BAR_HEIGHT + m::TOOLBAR_HEIGHT + 1.0);
                        cx.resize(win, size);
                    }
                    if let (Some(e), false) = (&error, v.loaded()) {
                        d.content = Content::Failed(e.clone());
                        error = None;
                    }
                }
                if let Some(e) = error {
                    self.show_error(cx, e);
                }
                if let Some(a) = after {
                    self.finish(cx, win, a);
                }
                cx.redraw(win);
            }
        }
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        let files = self
            .windows
            .values()
            .filter_map(|w| match w {
                Win::Doc(d) => Some(d.path.clone()),
                _ => None,
            })
            .collect();
        serde_json::to_string(&Saved { files }).ok()
    }
}

/// Dragging the document out (from a picture that fits the window): the
/// file, and for a common picture format its bytes too, for programs that
/// take an image rather than a file.
fn file_drag(ctx: &egui::Context, path: &Path) -> olo_kit::dnd::DragData {
    use olo_kit::dnd::{Actions, DragData};
    let mut data = DragData::files(&[path.to_path_buf()], Actions::COPY);
    if let Some(dir) = path.parent() {
        data = data.from_folder(dir);
    }
    let mime = olo_kit::mime::for_path(path);
    let small = std::fs::metadata(path).is_ok_and(|m| m.len() <= 16 << 20);
    if matches!(mime.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp")
        && small
        && let Ok(bytes) = std::fs::read(path)
    {
        // After the file: a folder takes the file, an editor the picture.
        data.types.push((mime, bytes.into()));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let icon = Item::stat(path).map_or_else(|| "document".to_owned(), |i| i.icon);
    match olo_kit::icons::drag_icon(ctx, &icon, 32.0, 1, vec2(16.0, 16.0), olo_kit::icons::DragLabel::Below(&name)) {
        Some(i) => data.icon(i),
        None => data,
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_preview::audio::set_identity("Viewer", "olo-viewer");
    olo_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            // Helvetica's metrics, for form fields and text boxes.
            if let Some((data, index)) = style::find_font(&["Liberation Sans", "Arimo", "Nimbus Sans", "Helvetica"], 400) {
                style::add_document_font(pdfview::PDF_FONT, data, index);
                proxy.send(Msg::FontReady);
            }
        });
        let mut v = Viewer {
            windows: HashMap::new(),
            signatures: signature::load(),
            next_id: 0,
            generation: 0,
        };
        match cx.restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok()) {
            Some(saved) if !saved.files.is_empty() => {
                for f in saved.files {
                    v.open_file(cx, f);
                }
            }
            _ => v.handle_args(cx, &args, None),
        }
        v
    });
}
