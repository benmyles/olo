//! Reading a file for Quick Look, on a worker thread. Every format is
//! decoded here, in process: no helper programs.

use std::{
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{archive, doc, media, pdf, text};
use olo_kit_base::files::{self as fs, Kind};

use crate::{Bitmap, Item};

/// Largest image side kept for display, in pixels. Bigger images are
/// scaled down as they are decoded (a 48-megapixel photo would otherwise
/// cost 200 MB of texture for no visible gain).
pub const MAX_IMAGE: u32 = 3200;
/// The same for a zoomable view (Viewer): GPUs' usual texture limit, so
/// most photos are shown pixel for pixel at 100%.
pub const MAX_ZOOMABLE: u32 = 8192;

pub type Image = Arc<Bitmap>;

/// What a preview shows.
pub enum Body {
    /// `full` is the picture's own size in pixels (the image may have
    /// been scaled down to fit a texture).
    Image {
        image: Image,
        full: [f32; 2],
        note: String,
    },
    /// A PDF: pages arrive one by one (`Update::Page`).
    /// `want` asks the worker to render a page (it answers with
    /// `Update::Page`).
    Pages {
        pages: Vec<Option<Image>>,
        page_size: (f32, f32),
        note: String,
        want: std::sync::mpsc::Sender<usize>,
        /// Every page is shown, one under another, scrolled through (a
        /// presentation's slides); else the first (a PDF's: its pages
        /// are the Viewer's).
        every: bool,
        /// Each page's words, for agents (a slide's text; empty: none
        /// said).
        words: Vec<String>,
        /// The size in device pixels a page asked for is wanted at
        /// (width in the high half, height in the low; 0: the loader's
        /// own): set by the view that shows every page before it asks,
        /// so a page is drawn for the pixels it covers, never resampled.
        px: Arc<std::sync::atomic::AtomicU64>,
    },
    Text(text::Text),
    /// Formatted documents: Markdown, Word, OpenDocument, PowerPoint.
    Rich {
        blocks: Vec<doc::Block>,
        note: String,
    },
    /// Spreadsheets and CSV: each sheet's rows.
    Table {
        sheets: Vec<(String, crate::table::Rows)>,
        note: String,
    },
    /// Folders and archives.
    Listing {
        items: Vec<archive::Item>,
        note: String,
    },
    Media(media::Media),
    Font {
        data: Arc<Vec<u8>>,
        note: String,
    },
    /// Nothing more to show than the icon and facts.
    Card {
        note: String,
    },
}

pub enum Update {
    Body(Body),
    /// PDF page `n` (0-based) is ready.
    Page(usize, Image),
    /// Syntax colouring for the text already shown.
    Highlighted(text::Text),
    Failed(String),
}

pub type Send<'a> = &'a mut dyn FnMut(Update);

/// Load `entry`, posting what is found to `send` (several times for
/// documents that stream in). Stops early once `cancel` is set.
/// What a view wants of pictures: their longest side at most (the
/// views resample them exactly to the size they're shown at).
#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub max_image: u32,
}

/// Whether `entry` is shown as a picture (`load`'s choice: a drawing by
/// its name, else a picture by its kind), without reading it: its
/// window's menus are a picture's, shown or not.
pub fn shown_as_picture(entry: &Item) -> bool {
    let ext = entry.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    !entry.is_dir && (matches!(ext.as_str(), "svg" | "svgz") || entry.kind == Kind::Image)
}

pub fn load(entry: &Item, want: Want, cancel: &Arc<AtomicBool>, send: Send) {
    let Want { max_image } = want;
    let ext = entry.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let name = entry.name.to_lowercase();
    let result = if entry.is_dir {
        archive::folder(&entry.path, cancel).map(Update::Body)
    } else {
        match (entry.kind, ext.as_str()) {
            (_, "svg" | "svgz") => svg(&entry.path, max_image / 2),
            (_, "pdf") => return pdf::load(&entry.path, cancel, send),
            (_, "pptx" | "ppsx" | "potx") => return crate::pptx::load(&entry.path, cancel, send),
            (_, "md" | "markdown" | "mdown") => doc::markdown(&entry.path),
            (_, "docx" | "odt" | "rtf" | "odp") => doc::office(&entry.path, &ext),
            (_, "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" | "csv" | "tsv") => doc::table(&entry.path, &ext),
            (_, "ttf" | "otf" | "ttc" | "otc") => font(&entry.path),
            (Kind::Image, _) => image(&entry.path, max_image),
            (Kind::Audio, _) | (_, "opus" | "aiff" | "aif" | "caf" | "alac" | "wma" | "mka") => media::audio(&entry.path),
            (Kind::Video, _) => media::video(&entry.path, cancel),
            (Kind::Archive, _) | (_, "jar" | "apk" | "epub" | "whl" | "crate") => archive::archive(&entry.path, &name, cancel),
            _ if looks_like_text(&entry.path) => return text::load(&entry.path, cancel, send),
            _ => Ok(Update::Body(Body::Card { note: entry.kind.name().into() })),
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    send(result.unwrap_or_else(Update::Failed));
}

/// Text if the first few kilobytes are UTF-8 (allowing a character cut
/// off at the end) with no NULs.
fn looks_like_text(path: &Path) -> bool {
    let mut buf = vec![0u8; 8192];
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let n = f.read(&mut buf).unwrap_or(0);
    let b = &buf[..n];
    if b.contains(&0) {
        return false;
    }
    match std::str::from_utf8(b) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && n == buf.len(),
    }
}

pub fn color_image(rgba: image::RgbaImage) -> Image {
    let size = [rgba.width() as usize, rgba.height() as usize];
    Arc::new(Bitmap::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// Opening without waiting (`O_NONBLOCK`, Linux's on every Olo machine):
/// a FIFO put where a picture was opens at once, and is refused for not
/// being a file, rather than waiting for a writer.
const O_NONBLOCK: i32 = 0o4000;

/// The most of a drawing's XML (an SVGZ's as it unpacks too) or a font
/// read to show it: past this it's too big to show, said so, rather than
/// held whole by the window showing it (review, 2026-11-28).
const DRAWING_LIMIT: u64 = 64 << 20;
const FONT_LIMIT: u64 = 64 << 20;

/// The file at `path`, opened once and looked at on that handle (not by
/// its name again: what's at the name may be another by the time it's
/// read): one with something in it, else why it can't be shown, in plain
/// words.
fn open_file(path: &Path) -> Result<std::fs::File, String> {
    use olo_kit_base::words::tr;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(O_NONBLOCK).open(path).map_err(|e| io_failure(path, &e))?;
    let m = file.metadata().map_err(|e| io_failure(path, &e))?;
    if !m.is_file() {
        return Err(tr("This isn’t a file that can be shown.").to_owned());
    }
    if m.len() == 0 {
        return Err(tr("This file is empty.").to_owned());
    }
    Ok(file)
}

/// What `from` holds, `limit` bytes at most: None past that (the first
/// byte over it is as far as it's read).
fn read_capped(from: impl Read, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    from.take(limit + 1).read_to_end(&mut out)?;
    Ok((out.len() as u64 <= limit).then_some(out))
}

/// Whether the picture decoder tells `path`'s format by its name's
/// extension only (a TGA has no mark of its own; a Telegram sticker is
/// gzipped Lottie): decoded by name, the rest by the file opened.
fn told_by_name(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("tga") || e.eq_ignore_ascii_case("tgs"))
}

/// What's said of a file that couldn't be read, by why, in plain words:
/// gone, not the person's to read, or else unreadable (the error itself
/// to the log).
fn io_failure(path: &Path, e: &std::io::Error) -> String {
    use olo_kit_base::words::tr;
    use std::io::ErrorKind;
    tracing::info!("preview {}: {e}", path.display());
    match e.kind() {
        ErrorKind::NotFound => tr("This file is no longer there."),
        ErrorKind::PermissionDenied => tr("You don’t have permission to open this file."),
        _ => tr("This file can’t be read."),
    }
    .to_owned()
}

/// What's said of an image the decoder couldn't show, from what it
/// said (`error`, to the log), in plain words, never its own (which can
/// be its structs: "This image can't be read (Format error decoding Png:
/// ChunkType {...", cut off there: QA, 2026-11-28): the file gone or not
/// the person's to read as it was opened (the decoder's own open said
/// so: the file isn't opened again to ask, which could wait on a FIFO put
/// there since, or say what another file is), a kind of image that can't
/// be read, one too big, or (whatever else stopped it) a damaged file.
fn image_failure(path: &Path, error: &str) -> String {
    use olo_kit_base::words::tr;
    tracing::info!("image {}: {error}", path.display());
    let e = error.to_lowercase();
    let said = if e.contains("no such file") {
        tr("This file is no longer there.")
    } else if e.contains("permission denied") {
        tr("You don’t have permission to open this file.")
    // (The image crate's words for a format it has no decoder for, or
    // can't tell.)
    } else if ["not supported", "unsupported", "could not be determined", "not recognized"].iter().any(|w| e.contains(w)) {
        tr("This kind of image can’t be shown.")
    } else if e.contains("limit") || e.contains("too large") || e.contains("memory") {
        tr("This image is too big to show.")
    } else {
        tr("This image file is damaged and can’t be opened.")
    };
    said.to_owned()
}

fn image(path: &Path, max: u32) -> Result<Update, String> {
    use std::os::fd::AsRawFd;
    let file = open_file(path)?;
    // Decoded from the file opened (its /proc name is it, whatever is at
    // `path` now), but for the formats told by name.
    let by = if told_by_name(path) { path.to_owned() } else { std::path::PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())) };
    // (A photo wider than sRGB kept in Display P3: shown whole in a window
    // drawing in P3, Viewer's; converted as drawn in any other.)
    let (img, kind, p3) = olo_images::open_wide(&by).map_err(|e| image_failure(path, &e))?;
    drop(file);
    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > max { img.resize(max, max, image::imageops::FilterType::Triangle) } else { img };
    let size = [img.width() as usize, img.height() as usize];
    let image = Arc::new(Bitmap::from_rgba_unmultiplied(size, img.to_rgba8().as_raw()).in_p3(p3));
    Ok(Update::Body(Body::Image { image, full: [w as f32, h as f32], note: format!("{kind} image, {w} × {h}") }))
}

fn svg(path: &Path, max: u32) -> Result<Update, String> {
    use olo_kit_base::words::tr;
    let too_big = || tr("This drawing is too big to show.").to_owned();
    let data = read_capped(open_file(path)?, DRAWING_LIMIT).map_err(|e| io_failure(path, &e))?.ok_or_else(too_big)?;
    // (Said plainly, as the rest; the decompressor's and the parser's own
    // words to the log. Unpacked no further than the limit too.)
    let data = if data.starts_with(&[0x1f, 0x8b]) {
        read_capped(flate2::read::GzDecoder::new(&data[..]), DRAWING_LIMIT)
            .map_err(|e| {
                tracing::info!("drawing {}: {e}", path.display());
                tr("This drawing is damaged and can’t be opened.").to_owned()
            })?
            .ok_or_else(too_big)?
    } else {
        data
    };
    let mut opt = resvg::usvg::Options { resources_dir: path.parent().map(Path::to_owned), ..Default::default() };
    opt.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_data(&data, &opt).map_err(|e| {
        tracing::info!("drawing {}: {e}", path.display());
        tr("This drawing can’t be read.").to_owned()
    })?;
    let size = tree.size();
    // Render crisp at up to `max` px on the long side.
    let k = (max as f32 / size.width().max(size.height())).min(8.0);
    let (w, h) = ((size.width() * k).ceil() as u32, (size.height() * k).ceil() as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or_else(|| tr("This drawing is empty.").to_owned())?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    let image = Arc::new(Bitmap::from_rgba_premultiplied([pixmap.width() as usize, pixmap.height() as usize], pixmap.take()));
    Ok(Update::Body(Body::Image { image, full: [size.width(), size.height()], note: format!("SVG drawing, {} × {}", size.width().round(), size.height().round()) }))
}

fn font(path: &Path) -> Result<Update, String> {
    let data = read_capped(open_file(path)?, FONT_LIMIT).map_err(|e| io_failure(path, &e))?.ok_or_else(|| olo_kit_base::words::tr("This font is too big to show.").to_owned())?;
    let face = ttf_parser::Face::parse(&data, 0).map_err(|e| {
        tracing::info!("font {}: {e}", path.display());
        olo_kit_base::words::tr("This font can’t be read.").to_owned()
    })?;
    let name = face
        .names()
        .into_iter()
        .filter(|n| n.name_id == ttf_parser::name_id::FULL_NAME && n.is_unicode())
        .find_map(|n| n.to_string())
        .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
    let note = format!("{name}, {} glyphs", face.number_of_glyphs());
    Ok(Update::Body(Body::Font { data: Arc::new(data), note }))
}

/// Facts about an item for its footer when nothing else says more.
pub fn summary(e: &Item) -> String {
    if e.is_dir { e.kind.name().into() } else { format!("{}, {}", e.kind.name(), fs::format_size(e.size)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of the test's own under /var/tmp, made new (never one
    /// that was there, which another test may own); the test deletes it.
    fn fresh_test_dir(prefix: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        loop {
            let dir = std::path::PathBuf::from(format!("/var/tmp/{prefix}-{}-{now}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&dir) {
                Ok(()) => return dir,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("a test folder: {e}"),
            }
        }
    }

    /// What's read to show a drawing or a font stops at its limit: an
    /// SVGZ unpacking past it is too big, as is a file past it; under
    /// it, whole. A picture is decoded from the file opened, a TGA by
    /// its name.
    #[test]
    fn reads_stop_at_their_limit() {
        use std::io::Write as _;
        assert_eq!(read_capped(&[7u8; 100][..], 100).unwrap().map(|v| v.len()), Some(100));
        assert_eq!(read_capped(&[7u8; 101][..], 100).unwrap(), None);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(&[b' '; 10_000]).unwrap();
        let packed = gz.finish().unwrap();
        assert!(packed.len() < 200);
        assert_eq!(read_capped(flate2::read::GzDecoder::new(&packed[..]), 1000).unwrap(), None);
        assert_eq!(read_capped(flate2::read::GzDecoder::new(&packed[..]), 10_000).unwrap().map(|v| v.len()), Some(10_000));
        assert!(told_by_name(std::path::Path::new("/f/old.TGA")) && !told_by_name(std::path::Path::new("/f/a.png")));
    }

    /// An image that can't be shown is said to be so in plain words, by
    /// why: never the decoder's own (its structs and all); a drawing and
    /// a font too; what isn't a file with something in it is said so
    /// before anything opens it.
    #[test]
    fn images_that_cant_be_shown_say_why_plainly() {
        let dir = fresh_test_dir("olo-preview-image-test");
        let write = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        };
        let failed = |r: Result<Update, String>| match r {
            Err(e) => e,
            Ok(_) => panic!("it was shown"),
        };
        // The QA's corrupt.png: a PNG's signature, then nothing a PNG has.
        let damaged = failed(image(&write("corrupt.png", b"\x89PNG\r\n\x1a\nthis is not a real png file at all"), 512));
        assert_eq!(damaged, "This image file is damaged and can’t be opened.");
        assert_eq!(failed(image(&write("zero.png", b""), 512)), "This file is empty.");
        // (No picture's signature, a name no decoder knows.)
        assert_eq!(failed(image(&write("words.xyz", b"plain words, not a picture"), 512)), "This kind of image can’t be shown.");
        assert_eq!(failed(image(&dir.join("gone.png"), 512)), "This file is no longer there.");
        // Not a file (a folder, a FIFO, a device): said so, not opened.
        std::fs::create_dir(dir.join("folder.png")).unwrap();
        assert_eq!(failed(image(&dir.join("folder.png"), 512)), "This isn’t a file that can be shown.");
        // (A device: as a FIFO would be, whose open would wait.)
        assert_eq!(failed(image(Path::new("/dev/null"), 512)), "This isn’t a file that can be shown.");
        // Not the person's to read (as root, it is).
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let shut = write("shut.png", b"\x89PNG\r\n\x1a\nx");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::metadata(&shut).unwrap().uid() != 0 {
            assert_eq!(failed(image(&shut, 512)), "You don’t have permission to open this file.");
        }
        // A damaged drawing, compressed or not, and a font.
        assert_eq!(failed(svg(&write("broken.svgz", b"\x1f\x8b\x08\x00 not gzip at all"), 512)), "This drawing is damaged and can’t be opened.");
        assert_eq!(failed(svg(&write("broken.svg", b"<svg"), 512)), "This drawing can’t be read.");
        assert_eq!(failed(font(&write("broken.ttf", b"not a font"))), "This font can’t be read.");
        for said in [damaged, failed(image(&write("half.jpg", b"\xff\xd8\xff\xe0 cut short"), 512))] {
            assert!(!said.contains('{') && !said.contains("ChunkType") && !said.contains("Format error") && !said.contains('\''), "{said}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
