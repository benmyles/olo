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
