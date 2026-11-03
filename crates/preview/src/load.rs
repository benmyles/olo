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
            (_, "md" | "markdown" | "mdown") => doc::markdown(&entry.path),
            (_, "docx" | "odt" | "rtf" | "pptx" | "odp") => doc::office(&entry.path, &ext),
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

fn image(path: &Path, max: u32) -> Result<Update, String> {
    let (img, kind) = olo_images::open(path).map_err(|e| format!("This image can't be read ({e})."))?;
    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > max { img.resize(max, max, image::imageops::FilterType::Triangle) } else { img };
    let image = color_image(img.to_rgba8());
    Ok(Update::Body(Body::Image { image, full: [w as f32, h as f32], note: format!("{kind} image, {w} × {h}") }))
}

fn svg(path: &Path, max: u32) -> Result<Update, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let data = if data.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&data[..]).read_to_end(&mut out).map_err(|e| e.to_string())?;
        out
    } else {
        data
    };
    let mut opt = resvg::usvg::Options { resources_dir: path.parent().map(Path::to_owned), ..Default::default() };
    opt.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_data(&data, &opt).map_err(|e| format!("This drawing can't be read ({e})."))?;
    let size = tree.size();
    // Render crisp at up to `max` px on the long side.
    let k = (max as f32 / size.width().max(size.height())).min(8.0);
    let (w, h) = ((size.width() * k).ceil() as u32, (size.height() * k).ceil() as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or("This drawing is empty.")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    let image = Arc::new(Bitmap::from_rgba_premultiplied([pixmap.width() as usize, pixmap.height() as usize], pixmap.take()));
    Ok(Update::Body(Body::Image { image, full: [size.width(), size.height()], note: format!("SVG drawing, {} × {}", size.width().round(), size.height().round()) }))
}

fn font(path: &Path) -> Result<Update, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let face = ttf_parser::Face::parse(&data, 0).map_err(|e| format!("This font can't be read ({e})."))?;
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
