//! PDF documents, rendered by hayro (pure Rust). The worker keeps the
//! document open and renders pages as the preview scrolls to them, so a
//! thousand-page PDF costs no more than the pages on screen.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use hayro::{
    PixmapSettings, RenderCache, RenderSettings, hayro_interpret::InterpreterSettings, hayro_syntax::Pdf,
    vello_cpu::color::palette::css::WHITE,
};
use tabula_kit::egui::ColorImage;

use super::load::{Body, Send, Update};

/// Pages are rendered this tall, in pixels: sharp at 2× in a preview that
/// fills most of the screen.
const PAGE_PX: f32 = 1800.0;

pub fn load(path: &Path, cancel: &Arc<AtomicBool>, send: Send) {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => return send(Update::Failed(e.to_string())),
    };
    let pdf = match Pdf::new(data) {
        Ok(p) => p,
        Err(hayro::hayro_syntax::LoadPdfError::Decryption(_)) => {
            return send(Update::Failed("This PDF is protected by a password.".into()));
        }
        Err(_) => return send(Update::Failed("This PDF can't be read.".into())),
    };
    let pages = pdf.pages();
    let n = pages.len();
    if n == 0 {
        return send(Update::Failed("This PDF has no pages.".into()));
    }
    let page_size = pages[0].render_dimensions();
    let (want, wanted) = mpsc::channel::<usize>();
    let note = format!("PDF document, {n} page{}", if n == 1 { "" } else { "s" });
    send(Update::Body(Body::Pages { pages: vec![None; n], page_size, note, want }));

    let cache = RenderCache::new();
    let settings = InterpreterSettings::default();
    let render = |i: usize| {
        let page = &pages[i];
        let (_, h) = page.render_dimensions();
        let k = (PAGE_PX / h.max(1.0)).clamp(0.25, 6.0);
        let pix = hayro::render(page, &cache, &settings, &RenderSettings::default(), &PixmapSettings {
            x_scale: k,
            y_scale: k,
            bg_color: WHITE,
        });
        let size = [pix.width() as usize, pix.height() as usize];
        Arc::new(ColorImage::from_rgba_premultiplied(size, pix.data_as_u8_slice()))
    };
    // The first page right away; then whatever the preview asks for.
    send(Update::Page(0, render(0)));
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        match wanted.recv_timeout(Duration::from_millis(200)) {
            Ok(i) if i < n => send(Update::Page(i, render(i))),
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}
