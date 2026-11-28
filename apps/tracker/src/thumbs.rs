//! Pictures made small: the icon view's and the desktop's icon of a
//! picture or a video (the picture itself; a frame of the video), and
//! the column view's preview of a selected picture. Made on a thread of
//! their own, the last asked for first (what has just scrolled into
//! view, not what scrolled past), kept by the file (its path and when it
//! last changed) and size, and the windows drawn again as each comes.
//!
//! Kept on disk too, where every desktop keeps them (the freedesktop
//! thumbnail cache: `~/.cache/thumbnails/normal`, `large`, `x-large` and
//! `xx-large`, 128 to 1024 pixels a side, a PNG named by the MD5 of the
//! file's URI, saying the file and its date): a folder of photographs is
//! decoded once, by whichever program shows it first, and read back in a
//! millisecond or two each after. What's drawn is made from that at the
//! exact device size asked for (a texture pixel a device pixel: never
//! resampled as it's drawn).

use olo_sync::{Condvar, Mutex};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::SystemTime,
};

use olo_kit::{Proxy, RenderImage};

/// What a file is, to make it small.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Of {
    Picture,
    /// A frame of it (the first it can show).
    Video,
}

/// (The file, the side in device pixels, the scale in thousandths, and
/// when the file last changed.)
type Key = (PathBuf, u32, u32, Option<SystemTime>);

/// A picture made small, and its size in device pixels.
#[derive(Clone)]
pub struct Thumb {
    pub image: Arc<RenderImage>,
    pub size: (u32, u32),
    /// Device pixels a pixel of it takes, drawn (nearest: a tiny picture
    /// kept as it is, `TINY`); 1 for the rest.
    pub whole: u32,
}

/// A picture this many pixels a side or fewer is tiny: kept as it is and
/// drawn whole times its size, each pixel a square of device pixels (as
/// near 32 points as that goes: 16 × 16 is 2, 3 and 4 device pixels a
/// pixel at 1, 1.5 and 2×), sharp; made smooth, it was blurred, then blown
/// up (Opus's look and a review, 2026-11-28).
pub const TINY: u32 = 32;

/// The longest side of a thumbnail, in points (the icon box is 48).
pub const THUMB: f32 = 40.0;

/// How many device pixels a pixel of a tiny picture `w` by `h` takes at
/// `scale` (`TINY`): 1 for one that isn't tiny.
pub fn whole_times((w, h): (u32, u32), scale: f32) -> u32 {
    let longest = w.max(h).max(1);
    if longest > TINY {
        return 1;
    }
    let points = (TINY / longest).max(1) as f32;
    // (Never past `THUMB`: 32 × 32 at 1.5× was two device pixels a
    // pixel, 42.7 points.)
    let most = (THUMB * scale / longest as f32).floor();
    (points * scale).round().min(most).max(1.0) as u32
}

enum Slot {
    Loading,
    /// (When it was last asked for, by the count of askings.)
    Ready(Option<Thumb>, u64),
}

/// How many are kept in memory (the icons of a few screenfuls: 96 pixels
/// a side at 2×, 37 KB each), and how many wait to be made at once
/// (what scrolled past and out of view is let go: asked for again if it
/// comes back).
const KEPT: usize = 900;
const WAITING: usize = 240;

static CACHE: Mutex<Option<HashMap<Key, Slot>>> = Mutex::new(None);
static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WAKE: OnceLock<Proxy> = OnceLock::new();

/// Who to tell when one is ready (sent [`crate::ToHub::Thumb`]).
pub fn init(proxy: Proxy) {
    let _ = WAKE.set(proxy);
}

/// How many pictures have been made: part of the key of a drawing of
/// one (it's drawn again once its picture is ready).
pub fn made() -> u64 {
    MADE.load(std::sync::atomic::Ordering::Relaxed)
}

static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Draw the windows again: a picture is ready (a thumbnail, an
/// AppImage's icon).
pub fn wake() {
    MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Some(p) = WAKE.get() {
        p.send(crate::ToHub::Thumb);
    }
}

/// What waits to be made: the last asked for is made first.
fn waiting() -> &'static (Mutex<Vec<(Key, Of)>>, Condvar) {
    static WAITING_FOR: OnceLock<(Mutex<Vec<(Key, Of)>>, Condvar)> = OnceLock::new();
    WAITING_FOR.get_or_init(|| {
        let _ = std::thread::Builder::new().name("thumbnails".into()).spawn(|| {
            let (queue, more) = waiting();
            let mut busy = false;
            loop {
                let (key, of) = {
                    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        match q.pop() {
                            Some(next) => break next,
                            None => {
                                // (All made: what decoding them took goes
                                // back to the system, not kept by the
                                // allocator for good.)
                                if std::mem::take(&mut busy) {
                                    drop(q);
                                    // SAFETY: glibc's own call, nothing of ours passed.
                                    #[cfg(target_env = "gnu")]
                                    unsafe {
                                        libc::malloc_trim(0);
                                    }
                                    q = queue.lock().unwrap_or_else(|e| e.into_inner());
                                    continue;
                                }
                                q = more.wait(q).unwrap_or_else(|e| e.into_inner());
                            }
                        }
                    }
                };
                busy = true;
                let made = make(&key.0, of, key.1, key.2 as f32 / 1000.0, key.3);
                if let Ok(mut c) = CACHE.lock() {
                    c.get_or_insert_with(HashMap::new).insert(key, Slot::Ready(made, ASKED.load(std::sync::atomic::Ordering::Relaxed)));
                }
                wake();
            }
        });
        (Mutex::new(Vec::new()), Condvar::new())
    })
}

/// The file's URI as the thumbnail cache names it: `file://` and its
/// path, every byte but a letter, a digit and `-._~/` as `%XX`.
pub fn uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The cache's folder and side for a picture `px` pixels a side: the
/// smallest of its four that's big enough.
pub fn standard(px: u32) -> (&'static str, u32) {
    match px {
        0..=128 => ("normal", 128),
        129..=256 => ("large", 256),
        257..=512 => ("x-large", 512),
        _ => ("xx-large", 1024),
    }
}

/// Where the desktop keeps its thumbnails.
fn cache_dir() -> PathBuf {
    // (Beside Olo's own cache folder: `$XDG_CACHE_HOME/thumbnails`.)
    let olo = olo_xdg::cache_dir();
    olo.parent().unwrap_or(&olo).join("thumbnails")
}

/// Where `path`'s thumbnail of that size is kept.
pub fn kept_at(path: &Path, px: u32) -> PathBuf {
    cache_dir().join(standard(px).0).join(format!("{:x}.png", md5::compute(uri(path).as_bytes())))
}

fn seconds(at: Option<SystemTime>) -> Option<u64> {
    at.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs())
}

/// The thumbnail kept on disk for `path` as it was last changed `at`,
/// if there is one and it's that file's as it is now.
fn kept(path: &Path, px: u32, at: Option<SystemTime>) -> Option<image::RgbaImage> {
    let file = std::fs::File::open(kept_at(path, px)).ok()?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().ok()?;
    let said = |key: &str| reader.info().uncompressed_latin1_text.iter().find(|t| t.keyword == key).map(|t| t.text.clone());
    if said("Thumb::URI")? != uri(path) || said("Thumb::MTime")?.parse::<u64>().ok() != seconds(at) {
        return None;
    }
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => image::RgbaImage::from_raw(w, h, pixels),
        (png::ColorType::Rgb, png::BitDepth::Eight) => image::RgbImage::from_raw(w, h, pixels).map(|i| image::DynamicImage::ImageRgb8(i).to_rgba8()),
        _ => None,
    }
}

/// Keep `small` as `path`'s thumbnail of that size (the person's alone;
/// written whole beside it and moved over).
fn keep(path: &Path, px: u32, at: Option<SystemTime>, small: &image::RgbaImage) -> Option<()> {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    let to = kept_at(path, px);
    // (Not the cache's own pictures, which would never end.)
    if path.starts_with(cache_dir()) {
        return None;
    }
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(to.parent()?).ok()?;
    let beside = to.with_extension(format!("png.{}", std::process::id()));
    let file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&beside).ok()?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), small.width(), small.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.add_text_chunk("Thumb::URI".into(), uri(path)).ok()?;
    encoder.add_text_chunk("Thumb::MTime".into(), seconds(at)?.to_string()).ok()?;
    encoder.add_text_chunk("Software".into(), "Olo Tracker".into()).ok()?;
    let done = encoder.write_header().and_then(|mut w| w.write_image_data(small.as_raw())).is_ok();
    if done { std::fs::rename(&beside, &to).ok() } else { std::fs::remove_file(&beside).ok().and(None) }
}

/// The picture, or a frame of the video, at `path`, whole.
fn whole(path: &Path, of: Of) -> Option<image::DynamicImage> {
    match of {
        Of::Picture => olo_images::open(path).ok().map(|(img, _)| img),
        Of::Video => {
            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let Ok(olo_preview::Update::Body(olo_preview::Body::Media(m))) = olo_preview::media::video(path, &cancel) else { return None };
            let frame = m.picture?;
            // (As the preview holds it, its colours times their alpha:
            // a video's frame has none.)
            image::RgbaImage::from_raw(frame.size[0] as u32, frame.size[1] as u32, frame.rgba.clone()).map(image::DynamicImage::ImageRgba8)
        }
    }
}

/// `img` no bigger than `px` a side, its shape kept (never made bigger).
fn fitted(img: &image::RgbaImage, px: u32) -> image::RgbaImage {
    if img.width() <= px && img.height() <= px {
        return img.clone();
    }
    image::DynamicImage::ImageRgba8(img.clone()).thumbnail(px, px).to_rgba8()
}

/// The least a small picture's narrower side may be, in points, for it
/// to stand for its file: under it, its kind's icon does. A 1200 × 40
/// strip made 48 points wide is a sliver 1.6 points tall, next to nothing
/// on a dark ground (QA, 2026-11-28).
pub const LEAST_SIDE: f32 = 4.0;

/// Whether a small picture `size` device pixels across, drawn `whole`
/// device pixels a pixel, is big enough to show at `scale`
/// (`LEAST_SIDE`): judged as drawn (a tiny square one was taken for a
/// sliver before it was made bigger).
pub fn shows((w, h): (u32, u32), whole: u32, scale: f32) -> bool {
    (w.min(h) * whole.max(1)) as f32 / scale.max(0.1) >= LEAST_SIDE
}

/// The size in device pixels a picture `w` by `h` is drawn at in a box
/// `px` device pixels a side at `scale`: fitted to the box, its shape
/// kept; one smaller than the box in points is drawn a point a pixel
/// (the same size on every display, not half of it at 2×).
pub fn drawn((w, h): (u32, u32), px: u32, scale: f32) -> (u32, u32) {
    let scale = scale.max(0.1);
    let fit = (px as f32 / scale / w.max(h).max(1) as f32).min(1.0);
    let side = |v: u32| ((v as f32 * fit * scale).round() as u32).clamp(1, px.max(1));
    (side(w), side(h))
}

/// The file made small: from the cache on disk where it has it, else
/// from the file itself (kept there then), for a box `px` device pixels
/// a side at `scale`, as the kit draws it (BGRA).
fn make(path: &Path, of: Of, px: u32, scale: f32, at: Option<SystemTime>) -> Option<Thumb> {
    let side = standard(px).1;
    let small = match kept(path, px, at) {
        Some(small) => small,
        None => {
            let small = fitted(&whole(path, of)?.to_rgba8(), side);
            let _ = keep(path, px, at, &small);
            small
        }
    };
    let whole = whole_times((small.width(), small.height()), scale);
    // (A tiny one as it is: its pixels drawn whole times, at paint.)
    let (w, h) = if whole > 1 || small.width().max(small.height()) <= TINY { (small.width(), small.height()) } else { drawn((small.width(), small.height()), px, scale) };
    let mut img = if (w, h) == (small.width(), small.height()) {
        small
    } else if w <= small.width() {
        image::DynamicImage::ImageRgba8(small).thumbnail_exact(w, h).to_rgba8()
    } else {
        // (A little picture on a dense display: each of its pixels a
        // point, smoothly.)
        image::imageops::resize(&small, w, h, image::imageops::FilterType::CatmullRom)
    };
    let size = (img.width(), img.height());
    for p in img.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Some(Thumb { image: Arc::new(RenderImage::new([image::Frame::new(img)])), size, whole })
}

/// The file at `path` made small for a box `px` device pixels a side at
/// `scale` ([`drawn`]): None while it's made (the windows are drawn
/// again then), or if it can't be read as what it's said to be.
pub fn of(path: &Path, of: Of, px: u32, scale: f32) -> Option<Thumb> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let key: Key = (path.to_owned(), px, (scale * 1000.0).round() as u32, mtime);
    let asked = ASKED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut c = CACHE.lock().ok()?;
    let map = c.get_or_insert_with(HashMap::new);
    match map.get_mut(&key) {
        Some(Slot::Ready(thumb, last)) => {
            *last = asked;
            return thumb.clone();
        }
        Some(Slot::Loading) => return None,
        None => {}
    }
    // Too many kept: those asked for longest ago go (a quarter of them).
    if map.len() > KEPT {
        let mut ages: Vec<u64> = map.values().filter_map(|s| if let Slot::Ready(_, last) = s { Some(*last) } else { None }).collect();
        ages.sort_unstable();
        let cut = ages.get(ages.len() / 4).copied().unwrap_or(0);
        map.retain(|_, s| !matches!(s, Slot::Ready(_, last) if *last <= cut));
    }
    map.insert(key.clone(), Slot::Loading);
    let (queue, more) = waiting();
    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
    // (Too many waiting: the oldest askings are let go, to be asked for
    // again if they're still in view.)
    if q.len() >= WAITING {
        for (old, _) in q.drain(..WAITING / 2) {
            map.remove(&old);
        }
    }
    q.push((key, of));
    more.notify_one();
    None
}

/// The thumbnail of the picture at `path`, `px` device pixels at most on
/// a side (the column view's preview): None while it's made (the
/// windows are drawn again then), or if it isn't a picture that can be
/// read.
pub fn get(path: &Path, px: u32) -> Option<Arc<RenderImage>> {
    // (Its own pixels, never made bigger: a scale of one.)
    of(path, Of::Picture, px, 1.0).map(|t| t.image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture whose small picture would be too thin to make out shows
    /// as its kind's icon; others as themselves, at every scale.
    #[test]
    fn thin_pictures_show_as_their_kind() {
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            let px = (48.0 * scale).round() as u32;
            // A 1200 × 40 strip: 48 × 1.6 points.
            assert!(!shows(drawn((1200, 40), px, scale), 1, scale), "at {scale}×");
            // 16 × 16: tiny, drawn about 32 points.
            assert!(shows((16, 16), whole_times((16, 16), scale), scale), "at {scale}×");
            // 3 × 3: tiny, not a sliver as drawn.
            assert!(shows((3, 3), whole_times((3, 3), scale), scale), "at {scale}×");
            // A photo, and 1000 × 100 (48 × 4.8 points).
            assert!(shows(drawn((4032, 3024), px, scale), 1, scale), "at {scale}×");
            assert!(shows(drawn((1000, 100), px, scale), 1, scale), "at {scale}×");
        }
    }

    /// The cache's names, as every desktop makes them.
    #[test]
    fn where_a_thumbnail_is_kept() {
        // (The freedesktop specification's own example.)
        assert_eq!(format!("{:x}", md5::compute(b"file:///home/jens/photos/me.png")), "c6ee772d9e49320e97ec29a7eb5b1697");
        assert_eq!(uri(Path::new("/home/mira/Pictures/a b/ünï.jpg")), "file:///home/mira/Pictures/a%20b/%C3%BCn%C3%AF.jpg");
        assert_eq!(uri(Path::new("/tmp/a-b_c.d~e")), "file:///tmp/a-b_c.d~e");
        assert_eq!([48, 128, 129, 256, 300, 512, 600].map(standard), [("normal", 128), ("normal", 128), ("large", 256), ("large", 256), ("x-large", 512), ("x-large", 512), ("xx-large", 1024)]);
    }

    /// A picture made small is kept on disk, saying whose it is and from
    /// when, and read back; one of a file changed since isn't.
    #[test]
    fn made_kept_and_read_back() {
        let dir = PathBuf::from(format!("/var/tmp/olo-tracker-thumbs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: the only test that sets it; read as the cache's folder.
        unsafe { std::env::set_var("XDG_CACHE_HOME", dir.join("cache")) };
        let file = dir.join("wide.png");
        image::RgbaImage::from_fn(400, 200, |x, _| image::Rgba([(x % 256) as u8, 40, 200, 255])).save(&file).unwrap();
        let at = std::fs::metadata(&file).unwrap().modified().ok();
        let thumb = make(&file, Of::Picture, 96, 2.0, at).expect("made");
        assert_eq!(thumb.size, (96, 48), "its shape kept, in the box asked for");
        let on_disk = kept(&file, 96, at).expect("kept");
        assert_eq!((on_disk.width(), on_disk.height()), (128, 64), "the cache's own size");
        assert!(kept_at(&file, 96).starts_with(dir.join("cache/thumbnails/normal")));
        // Another date: not that file's as it is now.
        assert!(kept(&file, 96, at.map(|t| t + std::time::Duration::from_secs(5))).is_none());
        // A tiny picture is kept as it is, drawn whole times its size
        // (nearest: sharp); a small one a point a pixel, smoothly.
        let small = dir.join("small.png");
        image::RgbaImage::from_pixel(20, 10, image::Rgba([9, 9, 9, 255])).save(&small).unwrap();
        assert_eq!(make(&small, Of::Picture, 48, 1.0, None).map(|t| (t.size, t.whole)), Some(((20, 10), 1)));
        assert_eq!(make(&small, Of::Picture, 96, 2.0, None).map(|t| (t.size, t.whole)), Some(((20, 10), 2)));
        let tiny = dir.join("tiny.png");
        image::RgbaImage::from_pixel(16, 16, image::Rgba([9, 9, 9, 255])).save(&tiny).unwrap();
        for (scale, whole) in [(1.0, 2), (1.5, 3), (2.0, 4)] {
            let t = make(&tiny, Of::Picture, (THUMB * scale) as u32, scale, None).unwrap();
            assert_eq!((t.size, t.whole), ((16, 16), whole), "at {scale}×");
            assert_eq!(16.0 * whole as f32 / scale, 32.0, "at {scale}×: 32 points");
        }
        // Never past `THUMB` on its long side, whatever its size and the
        // scale (32 × 32 at 1.5× was 42.7 points).
        for side in 1..=TINY {
            for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
                let k = whole_times((side, side), scale);
                assert!(k >= 1 && (side * k) as f32 / scale <= THUMB + 0.01, "{side} at {scale}×: {k}");
            }
        }
        let middling = dir.join("middling.png");
        image::RgbaImage::from_pixel(36, 30, image::Rgba([9, 9, 9, 255])).save(&middling).unwrap();
        assert_eq!(make(&middling, Of::Picture, 80, 2.0, None).map(|t| (t.size, t.whole)), Some(((72, 60), 1)));
        // The sizes drawn: fitted to the box; small ones a point a pixel.
        assert_eq!(drawn((400, 200), 48, 1.0), (48, 24));
        assert_eq!(drawn((400, 200), 72, 1.5), (72, 36));
        assert_eq!(drawn((128, 64), 96, 2.0), (96, 48));
        assert_eq!(drawn((60, 40), 96, 2.0), (96, 64), "bigger than the box in points: fitted");
        assert_eq!(drawn((60, 40), 48, 1.0), (48, 32));
        assert_eq!(drawn((30, 40), 72, 1.5), (45, 60));
        // Not a picture.
        std::fs::write(dir.join("not.png"), b"no").unwrap();
        assert!(make(&dir.join("not.png"), Of::Picture, 96, 2.0, None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
