//! The pictures notes show, each made for the place it's drawn in: a
//! note's own (sent with it) and those in files (a history entry's kept
//! picture, a note's large picture), cut to fill their place when they
//! must (a cover) and resampled exactly to its device size on a worker,
//! so a texture pixel is a device pixel. Kept by what, how big and how
//! cut; the windows are drawn again as each arrives.
//!
//! A large picture is shown so that what it is of can be told
//! ([`shown`]: fitted whole, a few pixels as blocks, a strip from its
//! start), and is made with what a drag of it carries under the pointer
//! ([`drag_image`]), from the same pixels on the same worker, so a press
//! that drags it has it at once.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_kit::{DragImage, Painter, RenderImage, style};
use olo_protocol::notifications::FileId;
use olo_sync::Mutex;

/// Pixels in no toolkit's type: RGBA, premultiplied.
#[derive(Debug, PartialEq)]
pub struct Raw {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

impl Raw {
    /// From straight (not premultiplied) RGBA, as a note sends it.
    pub fn from_unmultiplied(width: usize, height: usize, rgba: &[u8]) -> Raw {
        Raw::from_unmultiplied_owned(width, height, rgba.to_vec())
    }

    /// The same, made from `rgba` itself (premultiplied where it is: a
    /// file's picture is never copied whole again for it).
    pub fn from_unmultiplied_owned(width: usize, height: usize, mut rgba: Vec<u8>) -> Raw {
        olo_preview::bitmap::premultiply(&mut rgba);
        Raw { size: [width, height], rgba }
    }

    fn load(path: &Path) -> Option<Raw> {
        // (Opened without waiting: a pipe named as a picture holds no
        // worker.)
        let img = decode(open_file(path, None)?)?.into_rgba8();
        Some(Raw::from_unmultiplied_owned(img.width() as usize, img.height() as usize, img.into_raw()))
    }

    /// The SVG drawing at `path` drawn `px` device pixels a side: read from
    /// the file opened as a picture is (without waiting, a file of its own),
    /// `SVG_MOST` bytes at most.
    fn drawn(path: &Path, px: u32) -> Option<Raw> {
        use std::io::Read as _;
        let mut bytes = Vec::new();
        open_file(path, None)?.take(SVG_MOST + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 > SVG_MOST {
            return None;
        }
        let img = olo_xdg::svg::render(&bytes, px.clamp(1, 1024), false)?;
        let (w, h) = (img.width as usize, img.height as usize);
        Some(if img.premultiplied { Raw { size: [w, h], rgba: img.rgba } } else { Raw::from_unmultiplied_owned(w, h, img.rgba) })
    }

    /// Its middle, as wide for its height as `w` × `h`.
    fn middle(&self, (w, h): (u32, u32)) -> Raw {
        let [sw, sh] = self.size;
        let want = w as f32 / h.max(1) as f32;
        let (cw, ch) = if sw as f32 / sh.max(1) as f32 > want { (((sh as f32 * want).round() as usize).clamp(1, sw), sh) } else { (sw, ((sw as f32 / want).round() as usize).clamp(1, sh)) };
        let (x0, y0) = ((sw - cw) / 2, (sh - ch) / 2);
        self.part(x0, y0, cw, ch)
    }

    /// `w` × `h` of it from (`x0`, `y0`).
    fn part(&self, x0: usize, y0: usize, w: usize, h: usize) -> Raw {
        let sw = self.size[0];
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in y0..y0 + h {
            let row = (y * sw + x0) * 4;
            rgba.extend_from_slice(&self.rgba[row..row + w * 4]);
        }
        Raw { size: [w, h], rgba }
    }
}

/// Where a picture comes from.
#[derive(Clone)]
pub enum Source {
    /// Sent with note `id`.
    Note(u32, Arc<Raw>),
    File(PathBuf),
    /// A note's large picture: its file, as it was when the note came
    /// (another given its name since is not what shows: a screenshot
    /// deleted and taken again has the same name).
    Large(PathBuf, Option<FileId>),
}

impl Source {
    fn key(&self) -> String {
        match self {
            Source::Note(id, _) => format!("note:{id}:{:p}", Arc::as_ptr(self.raw_ref())),
            Source::File(p) => format!("file:{}", p.display()),
            Source::Large(p, id) => format!("large:{}:{id:?}", p.display()),
        }
    }

    fn raw_ref(&self) -> &Arc<Raw> {
        match self {
            Source::Note(_, r) => r,
            Source::File(_) | Source::Large(..) => unreachable!("a file's pixels are read on the worker"),
        }
    }
}

/// Where a picture shown in part is cut short.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cut {
    None,
    Right,
    Bottom,
}

/// How a large picture is shown at a scale ([`shown`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Shown {
    /// The part of it shown, from its top left (its own pixels): all of
    /// it but for a strip.
    pub part: (u32, u32),
    /// The device pixels it is drawn in.
    pub pixels: (u32, u32),
    /// Each of its pixels a block of a whole number of device pixels
    /// (made by repeating them: never smoothed).
    pub blocks: bool,
    pub cut: Cut,
}

/// How a picture fills its place.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Shape {
    /// All of it, as large as fits (its own proportions).
    Fit,
    /// Filling it, its middle cut out.
    Cover,
    /// As [`shown`] says: exactly its pixels.
    Shown(Shown),
}

/// A large picture's short side is shown at least this tall where it
/// can be (points, inside its frame): less, and what it is of can't be
/// told.
pub const LEAST: f32 = 44.0;

/// How a picture of `w` × `h` pixels is shown, at `scale`, in a place
/// `width` × `most` points (its frame's lines, `lines` device pixels in
/// all across, taken off), so that what it is of can be told (after
/// Screenshot's own card, before its pictures were notifications).
/// Larger than its place: fitted whole (its shape kept), where that
/// leaves its short side [`LEAST`] tall. A strip, whose short side
/// would not be: at its own size (made smaller only while its short
/// side stays that tall), from its start, its far end cut. Smaller than
/// its place both ways: at its own size, a pixel to a device pixel, or,
/// a few pixels dragged out of the screen, as blocks of whole device
/// pixels until it is that tall, where that fits.
pub fn shown(w: u32, h: u32, scale: f32, width: f32, most: f32, lines: u32) -> Shown {
    let (w, h) = (w.max(1), h.max(1));
    let inside = |points: f32| (((points * scale).round() as u32).saturating_sub(lines)).max(1);
    let (col, tall, least) = (inside(width), inside(most), ((LEAST * scale).round() as u32).max(1));
    if w <= col && h <= tall {
        // (As many points at every scale as the pixels allow: the least
        // whole number of points a pixel that makes its short side tall
        // enough at 1×, then the whole device pixels a pixel that come
        // nearest under it. Counted in device pixels alone, a few pixels
        // were 60 points wide at 2× and 80 at 1× and 1.5×.)
        let points = LEAST.round() as u32;
        let short = w.min(h);
        let by = ((points.div_ceil(short).max(1) as f32 * scale).floor() as u32).max(1);
        let by = if short * by < least { by + 1 } else { by }.min(col / w).min(tall / h).max(1);
        return Shown { part: (w, h), pixels: (w * by, h * by), blocks: by > 1, cut: Cut::None };
    }
    let by = (col as f32 / w as f32).min(tall as f32 / h as f32);
    let fit = (((w as f32 * by).round() as u32).clamp(1, col), ((h as f32 * by).round() as u32).clamp(1, tall));
    if fit.0.min(fit.1) >= least {
        return Shown { part: (w, h), pixels: fit, blocks: false, cut: Cut::None };
    }
    // A strip: a pixel a point (fewer, while its short side stays LEAST
    // points tall), as the same picture is at 1× at every scale: at 2×,
    // as blocks of whole device pixels, never resampled up (counted in
    // device pixels, it showed half as tall at 2× and twice as much of
    // its length). Where a point isn't a whole number of device pixels,
    // the nearest whole number that is.
    let points = (LEAST / w.min(h) as f32).min(1.0);
    let each = points * scale;
    if each >= 1.0 {
        // (Whole blocks: as many whole pixels of it as fit, each `k`
        // device pixels on a side.)
        let k = (each.round() as u32).max(1);
        let blocks = k > 1;
        return if w >= h {
            let across = (col / k).clamp(1, w);
            Shown { part: (across, h), pixels: (across * k, (h * k).min(tall)), blocks, cut: if across < w { Cut::Right } else { Cut::None } }
        } else {
            let down = (tall / k).clamp(1, h);
            Shown { part: (w, down), pixels: ((w * k).min(col), down * k), blocks, cut: if down < h { Cut::Bottom } else { Cut::None } }
        };
    }
    let by = each;
    let whole = (((w as f32 * by).round() as u32).max(1), ((h as f32 * by).round() as u32).max(1));
    if w >= h {
        let across = whole.0.min(col);
        Shown { part: (((across as f32 / by).round() as u32).clamp(1, w), h), pixels: (across, whole.1.min(tall)), blocks: false, cut: if whole.0 > col { Cut::Right } else { Cut::None } }
    } else {
        let down = whole.1.min(tall);
        Shown { part: (w, ((down as f32 / by).round() as u32).clamp(1, h)), pixels: (whole.0.min(col), down), blocks: false, cut: if whole.1 > tall { Cut::Bottom } else { Cut::None } }
    }
}

/// A picture's own size, `(w, h)` pixels, fitted within `place`.
pub fn fitted(size: [usize; 2], place: (u32, u32)) -> (u32, u32) {
    let [w, h] = size.map(|v| v.max(1) as f32);
    let k = (place.0 as f32 / w).min(place.1 as f32 / h);
    (((w * k).round() as u32).max(1), ((h * k).round() as u32).max(1))
}

type Key = (String, u32, u32, Shape);

enum Slot<T> {
    Making,
    Made(T),
    Missing,
}

/// Pictures made so far (one cache for the program: the banners and the
/// History window draw the same ones).
static MADE: Mutex<Option<HashMap<Key, Slot<Arc<RenderImage>>>>> = Mutex::new(None);

/// What a drag of a large picture carries, by its source and the whole
/// scale its buffer has.
static DRAGS: Mutex<Option<HashMap<DragKey, Slot<DragImage>>>> = Mutex::new(None);

/// A drag's picture's source (`Source::key`), the whole scale its
/// buffer has, and its edge's colour (`Carry`).
type DragKey = (String, u32, [u8; 4]);

/// At most this many kept; past it, they all go and are made again as
/// they're drawn (the History window scrolled through two hundred
/// pictures). One let go of takes its texture with it.
const KEEP: usize = 400;
/// Drags' pictures kept (a few of the newest are the ones dragged).
const KEEP_DRAGS: usize = 32;

/// `source`'s picture made for a place `place` device pixels in size
/// (`shape`: how it fills it; fitted, it is its own size within it, to
/// be centred there), for what `g` is painting: None while it's being
/// made (the drawing is made again when it is) or if it can't be read.
pub fn get(g: &mut Painter, source: &Source, place: (u32, u32), shape: Shape) -> Option<Arc<RenderImage>> {
    let place = (place.0.max(1), place.1.max(1));
    let key = (source.key(), place.0, place.1, shape);
    {
        let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
        let made = made.get_or_insert_with(HashMap::new);
        match made.get(&key) {
            Some(Slot::Made(img)) => return Some(img.clone()),
            Some(Slot::Missing) => return None,
            Some(Slot::Making) => {
                g.paint_unfinished();
                return None;
            }
            None => {}
        }
        if made.len() >= KEEP {
            made.retain(|_, slot| matches!(slot, Slot::Making));
        }
        made.insert(key.clone(), Slot::Making);
    }
    g.paint_unfinished();
    // (A large picture's drag is made with it, from the same pixels, for
    // this window's scale: there when it's pressed.)
    let drag = matches!(source, Source::Large(..)).then(|| drag_for(g.scale_factor())).filter(|&carry| want_drag(source, carry));
    let source = source.clone();
    olo_kit::app_kit::pool::run(move || {
        let made = make(&source, place, shape, drag);
        if let Some((by, edge)) = drag {
            let slot = made.as_ref().and_then(|(_, d)| d.clone()).map_or(Slot::Missing, Slot::Made);
            DRAGS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert((source.key(), by, edge), slot);
        }
        MADE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, made.and_then(|(img, _)| img).map_or(Slot::Missing, Slot::Made));
        // What was drawn without it is drawn again.
        olo_kit::all_windows(|cx| cx.repaint_unfinished());
    })
    .detach();
    None
}

/// What a drag's picture is made for: the whole scale its buffer has at
/// a window's scale (the display scales a buffer by whole numbers), and
/// its edge's colour, a large picture's frame's in a banner, in this
/// theme.
type Carry = (u32, [u8; 4]);

fn drag_for(scale: f32) -> Carry {
    (scale.ceil().max(1.0) as u32, olo_theme::tint(olo_kit::theme::palette().panel_background, 1.25))
}

/// Whether a drag's picture for `source` as `carry` says is still to be
/// made (marked as being made, if so).
fn want_drag(source: &Source, (by, edge): Carry) -> bool {
    let mut drags = DRAGS.lock().unwrap_or_else(|e| e.into_inner());
    let drags = drags.get_or_insert_with(HashMap::new);
    let key = (source.key(), by, edge);
    if drags.contains_key(&key) {
        return false;
    }
    if drags.len() >= KEEP_DRAGS {
        drags.retain(|_, slot| matches!(slot, Slot::Making));
    }
    drags.insert(key, Slot::Making);
    true
}

/// A note's icon named by a file's path (an `app_icon`, a GTK note's
/// file icon, an `image-path` that wasn't a picture), drawn in `r` as a
/// picture is read: by the bounded decoder (`decode`), on a worker, and
/// only a file of its own (opened without waiting: a pipe holds nothing
/// up), at its own size within `r`, a texture pixel to a device pixel.
/// Never by the icon theme's loader, which reads whatever it's given,
/// whole, however large, and waits on a pipe. True if it's drawn or
/// still being read; false if it can't be (the next mark is drawn).
pub fn paint_file_icon(g: &mut Painter, r: olo_kit::Bounds<olo_kit::Pixels>, path: &str) -> bool {
    let s = g.scale_factor();
    let place = ((f32::from(r.size.width) * s).round().max(1.0) as u32, (f32::from(r.size.height) * s).round().max(1.0) as u32);
    let source = Source::File(PathBuf::from(path));
    {
        let made = MADE.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(made.as_ref().and_then(|m| m.get(&(source.key(), place.0, place.1, Shape::Fit))), Some(Slot::Missing)) {
            return false;
        }
    }
    if let Some(img) = get(g, &source, place, Shape::Fit) {
        let d = img.size(0);
        let at = style::snap_centred(g, r.center(), d.width.0 as f32 / s, d.height.0 as f32 / s);
        let _ = g.paint_image(at, olo_kit::Corners::default(), img, 0, false);
    }
    true
}

/// Files found lately to be what their notes came with, files of their
/// own (`banners::draggable`), by path and identity: whether, and when.
type CheckKey = (String, FileId);
#[derive(Default)]
struct Checked {
    found: HashMap<CheckKey, (std::time::Instant, bool)>,
    /// Being looked at now, on a worker: asked again meanwhile, not again.
    looking: std::collections::HashSet<CheckKey>,
}
static CHECKED: Mutex<Option<Checked>> = Mutex::new(None);
/// How long a file's check stands before it's looked at again, and how
/// many are kept (past that, the oldest go).
const CHECK_KEPT: std::time::Duration = std::time::Duration::from_secs(10);
const CHECKS_KEPT: usize = 64;

/// `key` found to be (`ok`) or not the file its note came with, now.
fn note_checked(checked: &mut Checked, key: CheckKey, ok: bool) {
    if checked.found.len() >= CHECKS_KEPT
        && !checked.found.contains_key(&key)
        && let Some(oldest) = checked.found.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone())
    {
        checked.found.remove(&oldest);
    }
    checked.found.insert(key, (std::time::Instant::now(), ok));
}

/// File `path` found to be the file its note came with (`id`), a file of
/// its own, by what has just read it (the service as the note came, the
/// worker that made its picture): known, so that the first drag finds it.
pub fn seed_checked(path: &str, id: FileId) {
    let mut checked = CHECKED.lock().unwrap_or_else(|e| e.into_inner());
    note_checked(checked.get_or_insert_with(Checked::default), (path.to_owned(), id), true);
}

/// The file a note's picture drags out as, if it was found to be the file
/// the note came with, a file of its own; else None, and it's looked at
/// on a worker. Never a look at the file system on the thread that takes
/// the pointer (a file on a slow disk held the banners up as the drag
/// began): the note's coming, its picture made, the pointer coming onto
/// it and the press on it look first, and the drag finds the answer. A
/// good answer past its time still stands while it's looked at again (a
/// drag begun after a long hover, or a quick first one, went nowhere and
/// the press became a click); a bad one stands until it's found good.
pub fn checked_file(look: &olo_protocol::NoteLook) -> Option<PathBuf> {
    let (path, id) = look.dragged()?;
    let key = (path.to_owned(), id);
    let answer = {
        let mut checked = CHECKED.lock().unwrap_or_else(|e| e.into_inner());
        let checked = checked.get_or_insert_with(Checked::default);
        let found = checked.found.get(&key).copied();
        let fresh = found.is_some_and(|(at, _)| at.elapsed() < CHECK_KEPT);
        if !fresh && !checked.looking.insert(key.clone()) {
            // (Being looked at already.)
            return found.is_some_and(|(_, ok)| ok).then(|| PathBuf::from(path));
        }
        if fresh {
            return found.is_some_and(|(_, ok)| ok).then(|| PathBuf::from(path));
        }
        found.map(|(_, ok)| ok)
    };
    let look = look.clone();
    let looked = key.clone();
    olo_kit::app_kit::pool::run(move || {
        let ok = crate::banners::draggable(&look).is_some();
        let mut checked = CHECKED.lock().unwrap_or_else(|e| e.into_inner());
        let checked = checked.get_or_insert_with(Checked::default);
        checked.looking.remove(&looked);
        note_checked(checked, looked, ok);
    })
    .detach();
    answer.filter(|ok| *ok).map(|_| PathBuf::from(&key.0))
}

/// What a drag of the large picture `source` carries for a window at
/// `scale` (made with the picture shown: None until then, or if it
/// can't be read). Asked for again, alone, where it was let go of.
pub fn drag_image(source: &Source, scale: f32) -> Option<DragImage> {
    let (by, edge) = drag_for(scale);
    {
        let drags = DRAGS.lock().unwrap_or_else(|e| e.into_inner());
        match drags.as_ref().and_then(|d| d.get(&(source.key(), by, edge))) {
            Some(Slot::Made(d)) => return Some(d.clone()),
            Some(Slot::Making | Slot::Missing) => return None,
            None => {}
        }
    }
    if matches!(source, Source::Large(..)) && want_drag(source, (by, edge)) {
        let source = source.clone();
        olo_kit::app_kit::pool::run(move || {
            let made = file_pixels(&source).and_then(|img| olo_kit::dnd::picture_of(&img, by as f32, edge));
            DRAGS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert((source.key(), by, edge), made.map_or(Slot::Missing, Slot::Made));
        })
        .detach();
    }
    None
}

/// The file at `path`, opened to be read, if it's a file of its own (not
/// a pipe or a device, which could keep whoever opens it waiting for
/// ever: it's opened without waiting, and looked at as opened) and,
/// where `id` says, the very file a note came with (whatever has its
/// name since). What's read from it is that file's.
pub fn open_file(path: impl AsRef<Path>, id: Option<FileId>) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).ok()?;
    let m = file.metadata().ok()?;
    (m.is_file() && id.is_none_or(|id| FileId::of_meta(&m) == id)).then_some(file)
}

/// The most a picture read from a file takes, decoded (bytes): a 64
/// megapixel one.
const DECODED_MOST: u64 = 256 << 20;

/// The most of an SVG icon's file read (bytes): an icon is a few KB.
const SVG_MOST: u64 = 1 << 20;

/// Whether `path` names an SVG drawing (by its name: what a theme's icon
/// loader goes by too).
fn is_svg(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

/// The picture in `file`, by what it holds (not its name), if it takes
/// no more than `DECODED_MOST` decoded.
pub fn decode(file: std::fs::File) -> Option<image::DynamicImage> {
    decode_within(std::io::BufReader::new(file), DECODED_MOST)
}

/// The picture in `bytes` (a GTK note's icon sent as its file's bytes),
/// as [`decode`] has it.
pub fn decode_bytes(bytes: &[u8]) -> Option<image::DynamicImage> {
    decode_within(std::io::Cursor::new(bytes), DECODED_MOST)
}

/// The picture `r` holds if it takes no more than `most` bytes as it's
/// decoded and as straight RGBA too (what's made of it: four bytes a
/// pixel, whatever it holds, so a grey one takes four times what it did
/// decoded): both told by its header, before any of it is decoded.
fn decode_within(r: impl std::io::BufRead + std::io::Seek, most: u64) -> Option<image::DynamicImage> {
    use image::ImageDecoder as _;
    let mut r = image::ImageReader::new(r).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(most);
    r.limits(limits);
    let decoder = r.into_decoder().ok()?;
    let (w, h) = decoder.dimensions();
    let rgba = u64::from(w).checked_mul(u64::from(h)).and_then(|n| n.checked_mul(4));
    if decoder.total_bytes() > most || rgba.is_none_or(|b| b > most) {
        return None;
    }
    image::DynamicImage::from_decoder(decoder).ok()
}

/// A picture file's size in pixels, from its header.
pub fn file_size(file: std::fs::File) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::BufReader::new(file)).with_guessed_format().ok()?.into_dimensions().ok()
}

/// A large picture's file, straight RGBA, if it is still the file the
/// note came with (looked at as it's opened, then read from what was
/// opened). One kept from before a note's file was known is shown as
/// its name has it.
fn file_pixels(source: &Source) -> Option<image::RgbaImage> {
    let Source::Large(path, id) = source else { return None };
    let img = decode(open_file(path, *id)?)?.into_rgba8();
    (img.width() > 0 && img.height() > 0).then_some(img)
}

/// `source`'s picture for `place` as `shape` says, and (`drag`: for that
/// whole scale) what a drag of it carries.
fn make(source: &Source, place: (u32, u32), shape: Shape, drag: Option<Carry>) -> Option<(Option<Arc<RenderImage>>, Option<DragImage>)> {
    let raw = match source {
        Source::Note(_, raw) => raw.clone(),
        // (A drawing, an app's icon given as an SVG file's path: drawn for
        // the place it fills, its few kilobytes read as a picture's are.)
        Source::File(path) if is_svg(path) => Arc::new(Raw::drawn(path, place.0.max(place.1))?),
        Source::File(path) => Arc::new(Raw::load(path)?),
        Source::Large(path, id) => {
            let img = file_pixels(source)?;
            // (Read as the very file the note came with: its drag needn't
            // look again, `checked_file`.)
            if let Some(id) = id {
                seed_checked(&path.to_string_lossy(), *id);
            }
            let carried = drag.and_then(|(by, edge)| olo_kit::dnd::picture_of(&img, by as f32, edge));
            // (Its pixels themselves, once the drag's are made of them.)
            let raw = Raw::from_unmultiplied_owned(img.width() as usize, img.height() as usize, img.into_raw());
            return Some((shaped(&raw, place, shape), carried));
        }
    };
    Some((shaped(&raw, place, shape), None))
}

/// `raw` made for `place` as `shape` says.
fn shaped(raw: &Raw, place: (u32, u32), shape: Shape) -> Option<Arc<RenderImage>> {
    let size = |r: &Raw| (r.size[0] as u32, r.size[1] as u32);
    match shape {
        Shape::Cover => {
            let middle = raw.middle(place);
            olo_kit::pictures::resampled(&middle.rgba, size(&middle), place)
        }
        Shape::Fit => olo_kit::pictures::resampled(&raw.rgba, size(raw), fitted(raw.size, place)),
        Shape::Shown(how) => made(raw, how),
    }
}

/// The picture as [`shown`] says, a pixel to a device pixel: its part
/// (a strip's start), resampled to its pixels, or repeated into blocks.
pub fn made(raw: &Raw, how: Shown) -> Option<Arc<RenderImage>> {
    let [w, h] = raw.size;
    if w == 0 || h == 0 {
        return None;
    }
    let part = ((how.part.0 as usize).clamp(1, w), (how.part.1 as usize).clamp(1, h));
    let from = if part == (w, h) { None } else { Some(raw.part(0, 0, part.0, part.1)) };
    let from = from.as_ref().unwrap_or(raw);
    let (pw, ph) = (how.pixels.0.max(1), how.pixels.1.max(1));
    if !how.blocks {
        return olo_kit::pictures::resampled(&from.rgba, (from.size[0] as u32, from.size[1] as u32), (pw, ph));
    }
    // Blocks: each pixel repeated, never smoothed (straight BGRA, as the
    // Interface Kit draws it).
    let (fw, fh) = (from.size[0], from.size[1]);
    let mut out = Vec::with_capacity(pw as usize * ph as usize * 4);
    for y in 0..ph as usize {
        let sy = (y * fh / ph as usize).min(fh - 1);
        for x in 0..pw as usize {
            let sx = (x * fw / pw as usize).min(fw - 1);
            let p = &from.rgba[(sy * fw + sx) * 4..][..4];
            let a = p[3];
            let un = |c: u8| if a == 0 || a == 255 { c } else { ((f32::from(c) * 255.0 / f32::from(a)).round()).min(255.0) as u8 };
            out.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a]);
        }
    }
    olo_kit::pictures::from_bgra(pw, ph, out)
}

/// The device pixels a large picture's frame takes across, both sides
/// together: a line each side.
pub fn frame_lines(scale: f32) -> u32 {
    (2.0 * style::line_pixels(scale)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How a picture is shown, so that it can be told what it is of (in a banner's room: 156 points, at most
    /// 160 tall, a line round it).
    #[test]
    fn a_picture_is_shown_so_that_it_can_be_seen() {
        let at = |w, h, scale: f32| shown(w, h, scale, 156.0, 160.0, frame_lines(scale));
        let plain = |part, pixels| Shown { part, pixels, blocks: false, cut: Cut::None };
        // A 16:10 screen's picture fills the room's width (154 × 96 points inside its frame), at each scale.
        assert_eq!(at(1280, 800, 1.0), plain((1280, 800), (154, 96)));
        assert_eq!(at(2560, 1600, 2.0), plain((2560, 1600), (308, 193)));
        assert_eq!(at(2240, 1400, 1.75), plain((2240, 1400), (271, 169)));
        // A tall window's: as tall as the room is, as wide as its shape says.
        assert_eq!(at(426, 720, 1.0), plain((426, 720), (93, 158)));
        // A picture as large as the room or smaller, and tall enough: a pixel to a device pixel (a point at
        // least, a picture of a part of a 2× screen at its own size there).
        assert_eq!(at(150, 100, 1.0), plain((150, 100), (150, 100)));
        assert_eq!(at(300, 200, 2.0), plain((300, 200), (300, 200)));
        assert_eq!(at(100, 90, 2.0), Shown { part: (100, 90), pixels: (200, 180), blocks: true, cut: Cut::None });
        // A few pixels dragged out of the screen: blocks of whole device pixels.
        let blocks = |pixels| Shown { part: (40, 30), pixels, blocks: true, cut: Cut::None };
        assert_eq!(at(40, 30, 1.0), blocks((80, 60)));
        assert_eq!(at(40, 30, 1.5), blocks((120, 90)));
        assert_eq!(at(40, 30, 2.0), blocks((160, 120)));
        // (By its short side, whichever that is.)
        assert_eq!(at(4, 8, 1.0).pixels, (44, 88));
        assert_eq!(at(8, 4, 1.0).pixels, (88, 44));
        // A strip: at its own size from its start, its far end cut (never fitted whole into a line).
        assert_eq!(at(1280, 20, 1.0), Shown { part: (154, 20), pixels: (154, 20), blocks: false, cut: Cut::Right });
        assert_eq!(at(20, 1200, 1.0), Shown { part: (20, 158), pixels: (20, 158), blocks: false, cut: Cut::Bottom });
        // (A pixel a point at every scale: at 2× the same part of it, as tall, in blocks of two; at 1.5×,
        // whole blocks too, the nearest.)
        assert_eq!(at(1280, 20, 2.0), Shown { part: (154, 20), pixels: (308, 40), blocks: true, cut: Cut::Right });
        assert_eq!(at(20, 1200, 2.0), Shown { part: (20, 158), pixels: (40, 316), blocks: true, cut: Cut::Bottom });
        assert_eq!(at(1280, 20, 1.5), Shown { part: (116, 20), pixels: (232, 40), blocks: true, cut: Cut::Right });
        assert_eq!(at(1280, 20, 1.25).pixels, (193, 20));
        // (A long one that is thick: made smaller only until its short side is as short as may be.)
        assert_eq!(at(4000, 100, 1.0), Shown { part: (350, 100), pixels: (154, 44), blocks: false, cut: Cut::Right });
        // Nothing: no division by nothing.
        assert_eq!(at(0, 0, 1.0).pixels, (44, 44));
        // Never wider or taller than the room, at every scale: its frame is the room's width at most.
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 3.0] {
            for (w, h) in [(1280, 800), (3840, 2160), (426, 720), (1280, 20), (20, 1200), (40, 30), (1, 1)] {
                let s = at(w, h, scale);
                let lines = frame_lines(scale);
                assert!(s.pixels.0 + lines <= (156.0 * scale).round() as u32 && s.pixels.1 + lines <= (160.0 * scale).round() as u32, "{w} × {h} at {scale}: {s:?}");
            }
        }
    }

    /// The picture made is exactly the pixels asked for: blocks repeated, a strip's start, never smoothed.
    #[test]
    fn the_picture_made_is_the_pixels_shown() {
        let tiny = Raw::from_unmultiplied(2, 1, &[10, 20, 30, 255, 200, 100, 50, 255]);
        let blocks = made(&tiny, Shown { part: (2, 1), pixels: (4, 2), blocks: true, cut: Cut::None }).unwrap();
        // (BGRA: each of its two pixels a block of two by two, nothing between them.)
        assert_eq!(blocks.as_bytes(0).unwrap(), [30, 20, 10, 255, 30, 20, 10, 255, 50, 100, 200, 255, 50, 100, 200, 255].repeat(2));
        let strip = made(&tiny, Shown { part: (1, 1), pixels: (1, 1), blocks: false, cut: Cut::Right }).unwrap();
        assert_eq!(strip.as_bytes(0).unwrap(), [30, 20, 10, 255]);
        // A picture with nothing in it makes none.
        assert!(made(&Raw { size: [0, 50], rgba: Vec::new() }, shown(0, 50, 1.0, 156.0, 160.0, 2)).is_none());
        // A long strip's start is made from its few pixels alone: the part it shows.
        let long = Raw::from_unmultiplied(4000, 100, &[1, 2, 3, 255].repeat(4000 * 100));
        let how = shown(4000, 100, 1.0, 156.0, 160.0, 2);
        assert_eq!(made(&long, how).unwrap().size(0), olo_kit::size(olo_kit::DevicePixels(154), olo_kit::DevicePixels(44)));
    }

    /// A picture is decoded only if it fits as decoded and as the straight RGBA made of it, both told by its
    /// header: a grey one takes four times as much as RGBA (a 100-megapixel grey PNG fit the bound decoded,
    /// then took 400 MB more). Its pixels are made premultiplied in place, never copied whole again.
    /// A drag finds its file known: seeded as the note came or its picture was made, it answers at once; a
    /// good answer past its time still stands while it's looked at again (a drag after a long hover went
    /// nowhere, and the press became a click); past the most kept, the oldest go, not all of them.
    /// An app's icon given as an SVG file's path is drawn for the place it fills (the picture decoder takes
    /// no SVG, and the next mark was drawn instead), read as a picture is: a file of its own, a little of it.
    #[test]
    fn an_svg_icon_file_is_drawn() {
        let dir = PathBuf::from(format!("/var/tmp/olo-notes-svg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("app.svg");
        std::fs::write(&svg, r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" fill="#e5534b"/></svg>"##).unwrap();
        let (place, _) = make(&Source::File(svg.clone()), (32, 32), Shape::Fit, None).unwrap();
        let img = place.unwrap();
        assert_eq!(img.size(0), olo_kit::size(olo_kit::DevicePixels(32), olo_kit::DevicePixels(32)));
        // Too much to be an icon: not read.
        let big = dir.join("big.svg");
        std::fs::write(&big, vec![b' '; (SVG_MOST + 10) as usize]).unwrap();
        assert!(make(&Source::File(big), (32, 32), Shape::Fit, None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_drag_finds_its_file_known() {
        let dir = PathBuf::from(format!("/var/tmp/olo-notes-checked-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        image::RgbaImage::from_pixel(4, 3, image::Rgba([1, 2, 3, 255])).save(&path).unwrap();
        let id = FileId::of(&path).unwrap();
        let p = path.to_string_lossy().into_owned();
        let look = olo_protocol::NoteLook { image: Some(p.clone()), image_id: Some(id), file: Some(p.clone()), file_id: Some(id), ..Default::default() };
        assert!(look.dragged().is_some());
        seed_checked(&p, id);
        assert_eq!(checked_file(&look), Some(path.clone()), "seeded: known at once");
        // Past its time: still good while it's looked at again.
        {
            let mut checked = CHECKED.lock().unwrap();
            let c = checked.as_mut().unwrap();
            c.found.insert((p.clone(), id), (std::time::Instant::now() - 2 * CHECK_KEPT, true));
        }
        assert_eq!(checked_file(&look), Some(path.clone()), "a good answer past its time stands");
        // The oldest go first.
        let mut c = Checked::default();
        let old = (String::from("/old"), id);
        c.found.insert(old.clone(), (std::time::Instant::now() - CHECK_KEPT, true));
        for n in 0..CHECKS_KEPT {
            note_checked(&mut c, (format!("/n{n}"), id), true);
        }
        assert_eq!(c.found.len(), CHECKS_KEPT);
        assert!(!c.found.contains_key(&old) && c.found.contains_key(&(String::from("/n0"), id)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picture_is_read_within_its_bound() {
        let mut png = Vec::new();
        image::GrayImage::from_pixel(100, 100, image::Luma([128])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        // 10 000 bytes decoded, 40 000 as RGBA.
        assert!(decode_within(std::io::Cursor::new(&png), 40_000).is_some());
        assert!(decode_within(std::io::Cursor::new(&png), 20_000).is_none(), "its RGBA is past the bound");
        assert!(decode_within(std::io::Cursor::new(&png), 5_000).is_none());
        assert!(decode_bytes(&png).is_some() && decode_bytes(b"not a picture").is_none());
        let raw = Raw::from_unmultiplied_owned(2, 1, vec![200, 100, 50, 128, 1, 2, 3, 255]);
        assert_eq!(raw.rgba, Raw::from_unmultiplied(2, 1, &[200, 100, 50, 128, 1, 2, 3, 255]).rgba);
        assert_eq!(&raw.rgba[4..], [1, 2, 3, 255]);
        assert!(raw.rgba[0] < 200 && raw.rgba[3] == 128, "premultiplied: {:?}", raw.rgba);
    }

    /// A large picture is its file as the note came: another file given its name since shows nothing (and
    /// isn't dragged out), and a picture is made with what a drag of it carries.
    #[test]
    fn a_large_picture_is_the_file_it_came_with() {
        let dir = PathBuf::from(format!("/var/tmp/olo-notes-large-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("screenshot1.png");
        image::RgbaImage::from_pixel(64, 40, image::Rgba([9, 8, 7, 255])).save(&path).unwrap();
        let id = FileId::of(&path);
        let source = Source::Large(path.clone(), id);
        let how = shown(64, 40, 2.0, 156.0, 160.0, frame_lines(2.0));
        let (picture, carried) = make(&source, how.pixels, Shape::Shown(how), Some((2, [1, 2, 3, 255]))).unwrap();
        assert_eq!(picture.unwrap().size(0), olo_kit::size(olo_kit::DevicePixels(how.pixels.0 as i32), olo_kit::DevicePixels(how.pixels.1 as i32)));
        let carried = carried.unwrap();
        assert!(carried.scale == 2 && carried.width % 2 == 0 && carried.height % 2 == 0);
        // Deleted, and taken again under its name: not the note's file.
        std::fs::rename(&path, dir.join("trashed.png")).unwrap();
        image::RgbaImage::from_pixel(64, 40, image::Rgba([200, 0, 0, 255])).save(&path).unwrap();
        assert!(make(&source, how.pixels, Shape::Shown(how), Some((2, [1, 2, 3, 255]))).is_none());
        assert!(make(&Source::Large(path.clone(), FileId::of(&path)), how.pixels, Shape::Shown(how), None).is_some());
        // (Different keys: what was made for the one is never the other's.)
        assert_ne!(source.key(), Source::Large(path.clone(), FileId::of(&path)).key());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
