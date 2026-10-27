//! The PDF worker: one thread per open PDF that owns the parsed document,
//! renders pages at the zoom the window asks for, and saves. The window
//! talks to it with [`Request`]s and hears back through [`Event`]s, so
//! nothing slow ever runs on the UI thread.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
};

use hayro::{
    PixmapSettings, RenderCache, RenderSettings,
    hayro_interpret::InterpreterSettings,
    hayro_syntax::{LoadPdfError, Pdf},
    vello_cpu::color::palette::css::WHITE,
};
use lopdf::Document;
use olo_kit::egui::ColorImage;

use crate::pdfdoc::{self, Model, PageGeom};

/// The largest page image we render, in pixels on its longer side.
pub const MAX_PX: f32 = 6000.0;

pub enum Request {
    /// Render page `page` at `scale` pixels per point.
    Render { page: usize, scale: f32 },
    /// Write the document with `model` to `to`. `flatten` burns everything
    /// in; otherwise, when `to` becomes the window's file (`adopt`), the
    /// worker reopens from what it wrote.
    Save { model: Model, to: PathBuf, flatten: bool, adopt: bool },
}

pub enum Event {
    Opened { geoms: Vec<PageGeom>, model: Model, editable: bool },
    Page { page: usize, scale: f32, image: Arc<ColorImage> },
    Saved { to: PathBuf, adopted: Option<Model> },
    SaveFailed(String),
    Failed(String),
}

struct Doc {
    bytes: Vec<u8>,
    /// None when lopdf can't parse it: viewable, not editable.
    doc: Option<Document>,
    model: Model,
    pdf: Pdf,
}

fn open(bytes: Vec<u8>) -> Result<Doc, String> {
    let doc = Document::load_mem(&bytes).ok().filter(|d| !d.is_encrypted() || d.encryption_state.is_some());
    let model = doc.as_ref().map(pdfdoc::read_model).unwrap_or_default();
    let render = match &doc {
        Some(d) => pdfdoc::render_bytes(&bytes, d, &model).unwrap_or_else(|e| {
            tracing::warn!("can't hide form widgets: {e}");
            bytes.clone()
        }),
        None => bytes.clone(),
    };
    let pdf = Pdf::new(render).map_err(|e| match e {
        LoadPdfError::Decryption(_) => "This PDF is protected by a password.".to_string(),
        _ => "This PDF can't be read.".to_string(),
    })?;
    if pdf.pages().is_empty() {
        return Err("This PDF has no pages.".into());
    }
    Ok(Doc { bytes, doc, model, pdf })
}

fn geoms(pdf: &Pdf) -> Vec<PageGeom> {
    pdf.pages()
        .iter()
        .map(|p| {
            let (w, h) = p.render_dimensions();
            PageGeom { size: (w, h), transform: p.initial_transform(true).as_coeffs() }
        })
        .collect()
}

/// Write `data` to `to` atomically: a temporary file beside it, synced,
/// then renamed over it (keeping the old file's permissions).
pub fn write_atomic(to: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = to.parent().unwrap_or(Path::new("."));
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.olo-{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        if let Ok(meta) = std::fs::metadata(to) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, to)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Start the worker for `path`; events go to `send` until the returned
/// sender is dropped.
pub fn start(path: PathBuf, send: impl Fn(Event) + Send + 'static) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::Builder::new()
        .name("viewer-pdf".into())
        .spawn(move || {
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => return send(Event::Failed(e.to_string())),
            };
            let mut d = match open(bytes) {
                Ok(d) => d,
                Err(e) => return send(Event::Failed(e)),
            };
            send(Event::Opened { geoms: geoms(&d.pdf), model: d.model.clone(), editable: d.doc.is_some() });
            let mut cache = RenderCache::new();
            let settings = InterpreterSettings::default();
            let mut queue: Vec<Request> = Vec::new();
            loop {
                if queue.is_empty() {
                    match rx.recv() {
                        Ok(r) => queue.push(r),
                        Err(_) => return,
                    }
                }
                queue.extend(rx.try_iter());
                // Saves first; then the newest render requests, skipping
                // repeats and stale scales of the same page.
                let next = match queue.iter().position(|r| matches!(r, Request::Save { .. })) {
                    Some(i) => queue.remove(i),
                    None => queue.pop().expect("queue is not empty"),
                };
                match next {
                    Request::Render { page, scale } => {
                        queue.retain(|r| !matches!(r, Request::Render { page: p, .. } if *p == page));
                        let pages = d.pdf.pages();
                        let Some(p) = pages.get(page) else { continue };
                        let (w, h) = p.render_dimensions();
                        let k = scale.min(MAX_PX / w.max(h).max(1.0)).max(0.05);
                        let pix = hayro::render(p, &cache, &settings, &RenderSettings::default(), &PixmapSettings {
                            x_scale: k,
                            y_scale: k,
                            bg_color: WHITE,
                        });
                        let size = [pix.width() as usize, pix.height() as usize];
                        let image = Arc::new(ColorImage::from_rgba_premultiplied(size, pix.data_as_u8_slice()));
                        send(Event::Page { page, scale, image });
                    }
                    Request::Save { model, to, flatten, adopt } => {
                        let Some(doc) = &d.doc else {
                            send(Event::SaveFailed("This PDF can't be edited.".into()));
                            continue;
                        };
                        let out = if flatten {
                            pdfdoc::flatten(&d.bytes, doc, &d.model, &model)
                        } else {
                            pdfdoc::save(&d.bytes, doc, &d.model, &model)
                        };
                        let out = match out {
                            Ok(o) => o,
                            Err(e) => {
                                send(Event::SaveFailed(e));
                                continue;
                            }
                        };
                        if let Err(e) = write_atomic(&to, &out) {
                            send(Event::SaveFailed(e.to_string()));
                            continue;
                        }
                        let adopted = if adopt && !flatten {
                            match open(out) {
                                Ok(nd) => {
                                    d = nd;
                                    cache = RenderCache::new();
                                    Some(d.model.clone())
                                }
                                Err(e) => {
                                    send(Event::SaveFailed(format!("Saved, but it can't be reopened: {e}")));
                                    None
                                }
                            }
                        } else {
                            None
                        };
                        send(Event::Saved { to, adopted });
                    }
                }
            }
        })
        .expect("spawn PDF worker");
    tx
}
