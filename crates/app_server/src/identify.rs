//! Identify: each display's name in a BeOS panel in its top-left corner
//! for a few seconds (Preferences ▸ Display ▸ Identify). The badge is
//! drawn once when asked for and kept on the output; `render` shows it
//! until it expires.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    output::Output,
    utils::{Buffer, Rectangle, Size, Transform},
};
use tiny_skia::Pixmap;

use crate::{shell::fill, text, theme};

/// How long badges stay up.
const SHOWN: Duration = Duration::from_secs(4);
/// Logical metrics.
const MARGIN: i32 = 24;
const PAD: i32 = 16;
const TITLE: f32 = 28.0;
const DETAIL: f32 = 14.0;

/// Until when, the drawn panel, and its size in device pixels.
type Shown = (Instant, MemoryRenderBuffer, (i32, i32));

#[derive(Default)]
struct Badge(Mutex<Option<Shown>>);

fn badge(output: &Output) -> &Badge {
    output.user_data().insert_if_missing_threadsafe(Badge::default);
    output.user_data().get::<Badge>().unwrap()
}

/// Draw `title` over `detail` in a raised panel at `s` device pixels per
/// logical pixel.
fn draw(title: &str, detail: &str, s: f64) -> Pixmap {
    let p = theme::palette();
    let px = |l: f32| (l as f64 * s).round() as i32;
    let u = px(1.0).max(1);
    let (ts, ds) = (TITLE * s as f32, DETAIL * s as f32);
    let tw = text::measure(title, ts, true).max(text::measure(detail, ds, false));
    let w = tw.ceil() as i32 + 2 * px(PAD as f32);
    let h = px(PAD as f32) * 2 + (ts * 1.25).ceil() as i32 + (ds * 1.25).ceil() as i32 + px(4.0);
    let mut pm = Pixmap::new(w as u32, h as u32).expect("identify pixmap");
    fill(&mut pm, 0, 0, w, h, p.frame_focused[0]);
    fill(&mut pm, u, u, w - 2 * u, h - 2 * u, p.panel_background);
    fill(&mut pm, u, u, w - 2 * u, u, p.shine);
    fill(&mut pm, u, u, u, h - 2 * u, p.shine);
    fill(&mut pm, u, h - 2 * u, w - 2 * u, u, p.shadow);
    fill(&mut pm, w - 2 * u, u, u, h - 2 * u, p.shadow);
    let x = px(PAD as f32) as f32;
    let y = px(PAD as f32) as f32;
    text::draw(&mut pm, title, x, y, ts, true, p.panel_text);
    text::draw(&mut pm, detail, x, y + (ts * 1.25).ceil() + px(4.0) as f32, ds, false, p.panel_text);
    pm
}

/// Put up `output`'s badge.
pub fn show(output: &Output, title: &str, detail: &str) {
    let s = output.current_scale().fractional_scale();
    let pm = draw(title, detail, s);
    let size: Size<i32, Buffer> = (pm.width() as i32, pm.height() as i32).into();
    let buffer = MemoryRenderBuffer::from_slice(
        pm.data(),
        Fourcc::Abgr8888,
        size,
        1,
        Transform::Normal,
        Some(vec![Rectangle::from_size(size)]),
    );
    *badge(output).0.lock().unwrap() = Some((Instant::now() + SHOWN, buffer, (size.w, size.h)));
}

/// The badge for `output`, while it is up.
pub fn element<R>(renderer: &mut R, output: &Output) -> Option<MemoryRenderBufferRenderElement<R>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let mut slot = badge(output).0.lock().unwrap();
    if slot.as_ref().is_some_and(|(until, _, _)| Instant::now() >= *until) {
        *slot = None;
    }
    let (_, buffer, (w, h)) = slot.as_ref()?;
    let s = output.current_scale().fractional_scale();
    let at = (MARGIN as f64 * s).round();
    let logical = ((*w as f64 / s).round() as i32, (*h as f64 / s).round() as i32);
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (at, at),
        buffer,
        None,
        Some(Rectangle::from_size((*w as f64, *h as f64).into())),
        Some(logical.into()),
        Kind::Unspecified,
    )
    .ok()
}
