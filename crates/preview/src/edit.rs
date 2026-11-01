//! Basic picture editing for the Viewer and Quick Look: rotate, flip,
//! crop and resize, undoable, then saved in the picture's own format (or
//! another with Save As). The full-resolution picture lives on a worker
//! thread; each change goes to it as the whole list of steps (applied
//! from the last result when they extend it), and the result comes back
//! as an image for [`Preview::show_edited`](crate::Preview::show_edited),
//! so a big resize never holds up a frame.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

use image::{DynamicImage, ImageFormat, imageops::FilterType};

use crate::load::{self, Image};

/// One editing step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    RotateLeft,
    RotateRight,
    FlipHorizontal,
    FlipVertical,
    /// In the picture's pixels as it was before this step.
    Crop { x: u32, y: u32, w: u32, h: u32 },
    Resize { w: u32, h: u32 },
}

impl Op {
    pub fn apply(&self, img: &DynamicImage) -> DynamicImage {
        match *self {
            Op::RotateLeft => img.rotate270(),
            Op::RotateRight => img.rotate90(),
            Op::FlipHorizontal => img.fliph(),
            Op::FlipVertical => img.flipv(),
            Op::Crop { x, y, w, h } => img.crop_imm(x, y, w.max(1), h.max(1)),
            Op::Resize { w, h } => img.resize_exact(w.max(1), h.max(1), FilterType::Lanczos3),
        }
    }

    /// The size after this step, from `(w, h)`.
    pub fn size(&self, (w, h): (u32, u32)) -> (u32, u32) {
        match *self {
            Op::RotateLeft | Op::RotateRight => (h, w),
            Op::FlipHorizontal | Op::FlipVertical => (w, h),
            Op::Crop { w, h, .. } | Op::Resize { w, h } => (w.max(1), h.max(1)),
        }
    }
}

/// Formats a picture can be saved in: (extension, format).
pub const SAVE_FORMATS: &[(&str, ImageFormat)] = &[
    ("png", ImageFormat::Png),
    ("jpg", ImageFormat::Jpeg),
    ("jpeg", ImageFormat::Jpeg),
    ("webp", ImageFormat::WebP),
    ("bmp", ImageFormat::Bmp),
    ("tif", ImageFormat::Tiff),
    ("tiff", ImageFormat::Tiff),
    ("gif", ImageFormat::Gif),
];

/// The format a path's extension saves as, if we can write it.
pub fn save_format(path: &Path) -> Option<ImageFormat> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    SAVE_FORMATS.iter().find(|(e, _)| *e == ext).map(|(_, f)| *f)
}

/// Whether a file is a picture that can be edited (decoded at full
/// resolution as pixels; drawings like SVG are not).
pub fn editable(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    !matches!(ext.as_str(), "svg" | "svgz" | "pdf" | "") && crate::Item::stat(path).is_some_and(|i| i.kind == olo_kit_base::files::Kind::Image)
}

/// What the worker sends back.
#[derive(Debug)]
pub enum EditEvent {
    /// The picture after the steps so far, ready to show.
    Shown { image: Image, smaller: Vec<Image>, full: [f32; 2], note: String, steps: usize },
    Saved(PathBuf),
    Failed(String),
}

enum Job {
    Show(Vec<Op>),
    Save(Vec<Op>, PathBuf),
}

/// The editing state of one picture.
pub struct Editor {
    pub path: PathBuf,
    ops: Vec<Op>,
    undone: Vec<Op>,
    /// The steps as last saved (or none, as opened).
    saved: Vec<Op>,
    /// The picture's size before any step.
    pub original: (u32, u32),
    to: mpsc::Sender<Job>,
    /// Steps sent and not yet shown.
    pub busy: bool,
    pub saving: bool,
}

impl Editor {
    /// Edit the picture at `path` (`size` as shown); `send` gets the
    /// worker's events on its thread.
    pub fn new(path: PathBuf, size: (u32, u32), send: impl Fn(EditEvent) + Send + 'static) -> Editor {
        Editor::new_with(path, size, true, send)
    }

    /// [`Editor::new`], with halved copies of what it shows or not (the
    /// egui views want them, having no mipmaps; the GPUI ones resample).
    pub fn new_with(path: PathBuf, size: (u32, u32), halvings: bool, send: impl Fn(EditEvent) + Send + 'static) -> Editor {
        let (to, from) = mpsc::channel();
        let p = path.clone();
        std::thread::Builder::new().name("image-edit".into()).spawn(move || work(p, from, halvings, send)).ok();
        Editor { path, ops: Vec::new(), undone: Vec::new(), saved: Vec::new(), original: size, to, busy: false, saving: false }
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// The size the picture has now.
    pub fn size(&self) -> (u32, u32) {
        self.ops.iter().fold(self.original, |s, op| op.size(s))
    }

    pub fn dirty(&self) -> bool {
        self.ops != self.saved
    }

    pub fn can_undo(&self) -> bool {
        !self.ops.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    pub fn push(&mut self, op: Op) {
        // A crop of everything, or a resize to the same size, does nothing.
        let size = self.size();
        match op {
            Op::Crop { x: 0, y: 0, w, h } | Op::Resize { w, h } if (w, h) == size => return,
            _ => {}
        }
        self.ops.push(op);
        self.undone.clear();
        self.show();
    }

    pub fn undo(&mut self) {
        if let Some(op) = self.ops.pop() {
            self.undone.push(op);
            self.show();
        }
    }

    pub fn redo(&mut self) {
        if let Some(op) = self.undone.pop() {
            self.ops.push(op);
            self.show();
        }
    }

    /// Back to the picture as it was opened.
    pub fn revert(&mut self) {
        if !self.ops.is_empty() {
            self.undone.clear();
            self.ops.clear();
            self.show();
        }
    }

    fn show(&mut self) {
        self.busy = true;
        let _ = self.to.send(Job::Show(self.ops.clone()));
    }

    /// Write the picture to `path` (its format from the extension).
    pub fn save(&mut self, path: PathBuf) {
        self.saving = true;
        let _ = self.to.send(Job::Save(self.ops.clone(), path));
    }

    /// The worker reports `event`: keep up.
    pub fn handle(&mut self, event: &EditEvent) {
        match event {
            EditEvent::Shown { steps, .. } => {
                if *steps == self.ops.len() {
                    self.busy = false;
                }
            }
            EditEvent::Saved(_) => {
                self.saving = false;
                self.saved = self.ops.clone();
            }
            EditEvent::Failed(_) => {
                self.saving = false;
                self.busy = false;
            }
        }
    }
}

fn work(path: PathBuf, from: mpsc::Receiver<Job>, halve: bool, send: impl Fn(EditEvent)) {
    let mut source: Option<DynamicImage> = None;
    // The last result and the steps that made it.
    let mut last: Option<(Vec<Op>, DynamicImage)> = None;
    // A save that came in while views were being skipped.
    let mut pending: Option<Job> = None;
    while let Some(mut job) = pending.take().or_else(|| from.recv().ok()) {
        // Only the newest view matters; saves are all kept (and done
        // after the view asked for before them).
        while let Job::Show(_) = job {
            match from.try_recv() {
                Ok(save @ Job::Save(..)) => {
                    pending = Some(save);
                    break;
                }
                Ok(next) => job = next,
                Err(_) => break,
            }
        }
        if source.is_none() {
            match olo_images::open(&path) {
                Ok((img, _)) => source = Some(img),
                Err(e) => {
                    send(EditEvent::Failed(format!("This picture can't be edited ({e}).")));
                    continue;
                }
            }
        }
        let src = source.as_ref().expect("loaded");
        let ops = match &job {
            Job::Show(ops) | Job::Save(ops, _) => ops.clone(),
        };
        let img = match &last {
            Some((done, img)) if ops.starts_with(done) => ops[done.len()..].iter().fold(img.clone(), |i, op| op.apply(&i)),
            _ => ops.iter().fold(src.clone(), |i, op| op.apply(&i)),
        };
        last = Some((ops.clone(), img.clone()));
        match job {
            Job::Show(ops) => {
                let (w, h) = (img.width(), img.height());
                let max = load::MAX_ZOOMABLE;
                let shown = if w.max(h) > max { img.resize(max, max, FilterType::Triangle) } else { img };
                let image = load::color_image(shown.to_rgba8());
                let smaller = if halve { load::halvings(&image) } else { Vec::new() };
                send(EditEvent::Shown { image, smaller, full: [w as f32, h as f32], note: format!("Edited image, {w} × {h}"), steps: ops.len() });
            }
            Job::Save(_, to) => match write(&img, &to) {
                Ok(()) => send(EditEvent::Saved(to)),
                Err(e) => send(EditEvent::Failed(format!("“{}” couldn't be saved: {e}", to.file_name().unwrap_or_default().to_string_lossy()))),
            },
        }
    }
}

/// Encode `img` for `path`'s format and write it atomically.
pub fn write(img: &DynamicImage, path: &Path) -> Result<(), String> {
    let format = save_format(path).ok_or("Pictures can be saved as PNG, JPEG, WebP, BMP, TIFF or GIF.")?;
    let mut data = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut data);
    let r = match format {
        ImageFormat::Jpeg => {
            let rgb = img.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 92).encode_image(&rgb)
        }
        // These keep no alpha (BMP keeps it, but readers often don't).
        ImageFormat::Bmp => DynamicImage::ImageRgb8(img.to_rgb8()).write_to(&mut cursor, format),
        // WebP and GIF are written from 8-bit RGBA.
        ImageFormat::WebP | ImageFormat::Gif => DynamicImage::ImageRgba8(img.to_rgba8()).write_to(&mut cursor, format),
        _ => img.write_to(&mut cursor, format),
    };
    r.map_err(|e| e.to_string())?;
    let tmp = path.with_file_name(format!(".{}.saving", path.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

#[cfg(feature = "egui")]
pub use crop::CropTool;

/// The egui kit's crop frame (Tracker's Quick Look, until it moves).
#[cfg(feature = "egui")]
mod crop {
    use olo_kit::{
        egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2},
        style::{self, c},
    };

    use super::Op;

    /// Choosing what to crop to, drawn over the picture: drag out a frame,
    /// move it from inside, or drag its corners and edges.
    #[derive(Debug, Default, Clone)]
    pub struct CropTool {
        /// In picture pixels.
        pub sel: Option<Rect>,
        drag: Option<Grab>,
    }

    #[derive(Debug, Clone, Copy)]
    enum Grab {
        /// A new frame from this corner (picture pixels).
        New(Pos2),
        Move(Pos2),
        /// Which edges follow the pointer: left, right, top, bottom.
        Edges(bool, bool, bool, bool),
    }

    const HANDLE: f32 = 7.0;

    impl CropTool {
        /// The crop step for the frame chosen, if there is one.
        pub fn op(&self) -> Option<Op> {
            let r = self.sel?;
            let (w, h) = (r.width().round() as u32, r.height().round() as u32);
            (w >= 1 && h >= 1).then(|| Op::Crop { x: r.left().round() as u32, y: r.top().round() as u32, w, h })
        }

        /// Draw and handle it over `shown` (the picture's rect on screen),
        /// for a picture `full` pixels big. Returns true when the frame changed.
        pub fn ui(&mut self, ui: &mut Ui, view: Rect, shown: Rect, full: (u32, u32)) -> bool {
            let p = style::palette(ui.ctx());
            let (fw, fh) = (full.0 as f32, full.1 as f32);
            let k = shown.width() / fw.max(1.0);
            let to_px = |s: Pos2| pos2(((s.x - shown.left()) / k).clamp(0.0, fw), ((s.y - shown.top()) / k).clamp(0.0, fh));
            let to_screen = |r: Rect| Rect::from_min_max(shown.min + r.min.to_vec2() * k, shown.min + r.max.to_vec2() * k);
            let resp = ui.interact(view, ui.scope_id().with("crop-tool"), Sense::click_and_drag());
            let mut changed = false;
            let pointer = resp.hover_pos().or(resp.interact_pointer_pos());
            // What is under the pointer.
            let grab_at = |pos: Pos2, sel: Option<Rect>| -> Option<Grab> {
                let s = to_screen(sel?);
                let near = |a: f32, b: f32| (a - b).abs() <= HANDLE;
                let inside_y = pos.y >= s.top() - HANDLE && pos.y <= s.bottom() + HANDLE;
                let inside_x = pos.x >= s.left() - HANDLE && pos.x <= s.right() + HANDLE;
                let (l, r, t, b) = (near(pos.x, s.left()) && inside_y, near(pos.x, s.right()) && inside_y, near(pos.y, s.top()) && inside_x, near(pos.y, s.bottom()) && inside_x);
                if l || r || t || b {
                    Some(Grab::Edges(l, r && !l, t, b && !t))
                } else if s.contains(pos) {
                    Some(Grab::Move(to_px(pos)))
                } else {
                    None
                }
            };
            if let Some(pos) = pointer {
                let icon = match self.drag.or_else(|| grab_at(pos, self.sel)) {
                    Some(Grab::Move(_)) => CursorIcon::Grab,
                    Some(Grab::Edges(l, r, t, b)) => match (l || r, t || b) {
                        (true, true) => {
                            if (l && t) || (r && b) {
                                CursorIcon::ResizeNwSe
                            } else {
                                CursorIcon::ResizeNeSw
                            }
                        }
                        (true, false) => CursorIcon::ResizeHorizontal,
                        _ => CursorIcon::ResizeVertical,
                    },
                    _ => CursorIcon::Crosshair,
                };
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(icon);
                }
            }
            if resp.drag_started()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                self.drag = Some(grab_at(pos, self.sel).unwrap_or(Grab::New(to_px(pos))));
            }
            if resp.dragged()
                && let (Some(pos), Some(grab)) = (resp.interact_pointer_pos(), self.drag)
            {
                let at = to_px(pos);
                let bounds = Rect::from_min_max(Pos2::ZERO, pos2(fw, fh));
                let sel = match grab {
                    Grab::New(from) => Some(Rect::from_two_pos(from, at)),
                    Grab::Move(from) => self.sel.map(|s| {
                        let d = at - from;
                        let moved = s.translate(d);
                        // Keep it inside the picture.
                        let dx = (bounds.left() - moved.left()).max(0.0) - (moved.right() - bounds.right()).max(0.0);
                        let dy = (bounds.top() - moved.top()).max(0.0) - (moved.bottom() - bounds.bottom()).max(0.0);
                        self.drag = Some(Grab::Move(at));
                        moved.translate(vec2(dx, dy))
                    }),
                    Grab::Edges(l, r, t, b) => self.sel.map(|mut s| {
                        if l {
                            s.min.x = at.x.min(s.max.x - 1.0);
                        }
                        if r {
                            s.max.x = at.x.max(s.min.x + 1.0);
                        }
                        if t {
                            s.min.y = at.y.min(s.max.y - 1.0);
                        }
                        if b {
                            s.max.y = at.y.max(s.min.y + 1.0);
                        }
                        s
                    }),
                };
                let sel = sel.map(|s| s.intersect(bounds)).map(|s| Rect::from_min_max(s.min.round(), s.max.round())).filter(|s| s.width() >= 1.0 && s.height() >= 1.0);
                if sel != self.sel {
                    self.sel = sel;
                    changed = true;
                }
            }
            if resp.drag_stopped() {
                self.drag = None;
            }
            // A click outside the frame drops it.
            if resp.clicked()
                && let Some(pos) = resp.interact_pointer_pos()
                && grab_at(pos, self.sel).is_none()
            {
                self.sel = None;
                changed = true;
            }
            // Dim what will go, outline what stays.
            let painter = ui.painter_at(view);
            let shade = c(p.scrim);
            match self.sel.map(to_screen) {
                Some(s) => {
                    let s = style::snap(ui.ctx(), s);
                    for r in [
                        Rect::from_min_max(shown.min, pos2(shown.right(), s.top())),
                        Rect::from_min_max(pos2(shown.left(), s.bottom()), shown.max),
                        Rect::from_min_max(pos2(shown.left(), s.top()), pos2(s.left(), s.bottom())),
                        Rect::from_min_max(pos2(s.right(), s.top()), pos2(shown.right(), s.bottom())),
                    ] {
                        painter.rect_filled(r, 0.0, shade);
                    }
                    painter.rect_stroke(s, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);
                    painter.rect_stroke(s.expand(1.0), 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), egui::StrokeKind::Inside);
                    // Thirds, while it is being dragged.
                    if self.drag.is_some() {
                        let guide = Stroke::new(1.0, Color32::from_white_alpha(110));
                        for i in 1..3 {
                            let x = s.left() + s.width() * i as f32 / 3.0;
                            let y = s.top() + s.height() * i as f32 / 3.0;
                            painter.line_segment([pos2(x, s.top()), pos2(x, s.bottom())], guide);
                            painter.line_segment([pos2(s.left(), y), pos2(s.right(), y)], guide);
                        }
                    }
                    for corner in [s.left_top(), s.right_top(), s.left_bottom(), s.right_bottom()] {
                        let h = Rect::from_center_size(corner, vec2(HANDLE, HANDLE));
                        painter.rect_filled(h, 0.0, Color32::WHITE);
                        painter.rect_stroke(h, 0.0, Stroke::new(1.0, c(p.keyboard_navigation)), egui::StrokeKind::Inside);
                    }
                    // The size it will be.
                    let r = self.sel.expect("some");
                    let text = format!("{} × {}", r.width().round(), r.height().round());
                    let font = style::small_font();
                    let g = painter.layout_no_wrap(text, font, c(p.tooltip_text));
                    let at = pos2(s.center().x - g.size().x / 2.0, (s.bottom() + 6.0).min(view.bottom() - g.size().y - 6.0));
                    let tip = Rect::from_min_size(at - vec2(5.0, 2.0), g.size() + vec2(10.0, 4.0));
                    painter.rect_filled(tip, 4.0, c(p.tooltip_background));
                    painter.rect_stroke(tip, 4.0, Stroke::new(1.0, c(p.control_border)), egui::StrokeKind::Inside);
                    painter.galley(at, g, c(p.tooltip_text));
                }
                None => {
                    painter.rect_filled(shown, 0.0, c(p.scrim).gamma_multiply(0.4));
                }
            }
            changed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 0, 255])))
    }

    #[test]
    fn steps_and_sizes() {
        let img = picture(40, 20);
        let r = Op::RotateRight.apply(&img);
        assert_eq!((r.width(), r.height()), (20, 40));
        // Rotating right puts the old bottom-left at the top-left.
        assert_eq!(r.to_rgba8().get_pixel(0, 0).0, [0, 19, 0, 255]);
        let l = Op::RotateLeft.apply(&img);
        assert_eq!(l.to_rgba8().get_pixel(0, 0).0, [39, 0, 0, 255]);
        assert_eq!(Op::FlipHorizontal.apply(&img).to_rgba8().get_pixel(0, 0).0, [39, 0, 0, 255]);
        assert_eq!(Op::FlipVertical.apply(&img).to_rgba8().get_pixel(0, 0).0, [0, 19, 0, 255]);
        let c = Op::Crop { x: 10, y: 5, w: 8, h: 4 }.apply(&img);
        assert_eq!((c.width(), c.height(), c.to_rgba8().get_pixel(0, 0).0), (8, 4, [10, 5, 0, 255]));
        let s = Op::Resize { w: 10, h: 5 }.apply(&img);
        assert_eq!((s.width(), s.height()), (10, 5));
        let ops = [Op::RotateLeft, Op::Crop { x: 0, y: 0, w: 10, h: 30 }, Op::Resize { w: 5, h: 15 }];
        assert_eq!(ops.iter().fold((40, 20), |s, o| o.size(s)), (5, 15));
    }

    #[test]
    fn saves_each_format() {
        let dir = std::env::temp_dir().join(format!("olo-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = picture(12, 7);
        for ext in ["png", "jpg", "webp", "bmp", "tiff", "gif"] {
            let p = dir.join(format!("x.{ext}"));
            write(&img, &p).unwrap();
            let back = image::open(&p).unwrap();
            assert_eq!((back.width(), back.height()), (12, 7), "{ext}");
        }
        assert!(write(&img, &dir.join("x.heic")).is_err());
        assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".saving")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editor_undo_and_dirty() {
        let (tx, rx) = mpsc::channel();
        let dir = std::env::temp_dir().join(format!("olo-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        picture(30, 10).save(&path).unwrap();
        let mut e = Editor::new(path.clone(), (30, 10), move |ev| tx.send(ev).unwrap());
        assert!(!e.dirty());
        e.push(Op::RotateRight);
        e.push(Op::Crop { x: 0, y: 0, w: 10, h: 30 });
        assert_eq!(e.ops().len(), 1, "a crop of everything does nothing");
        assert_eq!(e.size(), (10, 30));
        let shown = loop {
            let ev = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            e.handle(&ev);
            if let EditEvent::Shown { full, .. } = ev {
                break full;
            }
        };
        assert_eq!(shown, [10.0, 30.0]);
        assert!(e.dirty() && !e.busy);
        e.undo();
        assert!(!e.dirty() && e.can_redo());
        e.redo();
        e.save(path.clone());
        loop {
            let ev = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            e.handle(&ev);
            if matches!(ev, EditEvent::Saved(_)) {
                break;
            }
        }
        assert!(!e.dirty());
        assert_eq!(image::image_dimensions(&path).unwrap(), (10, 30));
        // A step saved straight away (Quick Look's rotate) is still shown.
        e.push(Op::RotateLeft);
        e.save(path.clone());
        let mut got = (false, false);
        while got != (true, true) {
            let ev = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            e.handle(&ev);
            match ev {
                EditEvent::Shown { full, .. } => got.0 = full == [30.0, 10.0],
                EditEvent::Saved(_) => got.1 = true,
                EditEvent::Failed(why) => panic!("{why}"),
            }
        }
        assert!(!e.busy && !e.saving && !e.dirty());
        assert_eq!(image::image_dimensions(&path).unwrap(), (30, 10));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
