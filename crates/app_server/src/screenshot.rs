//! Screenshots: `Request::Screenshot` (the Screenshot app, tsession) and
//! app_server's own (the screen saver's picture of the desktop).
//!
//! A request becomes a [`Shot`] for one output; the backend takes it with
//! that output's next frame ([`take`]: rendered offscreen and read back,
//! the only part that needs the GPU) and a thread draws the pointer in,
//! encodes the PNG and writes it (as `NAME.part`, then renamed, so readers
//! never see half a file). The requester is answered once it is on disk.

use std::{
    path::PathBuf,
    rc::Rc,
};

use smithay::{
    backend::renderer::{Color32F, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, element::AsRenderElements},
    desktop::{Space, space::SpaceElement},
    output::Output,
    utils::{Logical, Point, Rectangle},
};
use tabula_protocol::{ImageFormat, Reply};
use tracing::warn;

use crate::{
    ipc::{Conn, IpcState},
    render::CustomRenderElements,
    shell::{WindowElement, WindowRenderElement},
    state::{AnvilState, Backend},
};

pub struct Shot {
    pub path: PathBuf,
    /// Who to answer (none for app_server's own).
    pub conn: Option<Rc<Conn>>,
    /// The output whose frame takes it (the window's, for a window).
    pub output: Option<String>,
    pub window: Option<u64>,
    pub border: bool,
    pub pointer: bool,
    pub format: ImageFormat,
}

impl Shot {
    /// The whole of whichever output draws next.
    pub fn screen(path: PathBuf) -> Shot {
        Shot { path, conn: None, output: None, window: None, border: true, pointer: false, format: ImageFormat::Png }
    }
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Queue a screenshot for the right output's next frame.
    pub fn request_screenshot(&mut self, mut shot: Shot) -> Result<(), String> {
        if let Some(id) = shot.window {
            let window = self.space.elements().find(|w| w.id() == id).ok_or("no such window on screen")?;
            let geo = self.space.element_geometry(window).ok_or("no such window on screen")?;
            // The output that shows most of it.
            shot.output = self
                .space
                .outputs()
                .filter_map(|o| Some((o, self.space.output_geometry(o)?.intersection(geo)?)))
                .max_by_key(|(_, r)| r.size.w * r.size.h)
                .map(|(o, _)| o.name());
            if shot.output.is_none() {
                return Err("the window is off screen".into());
            }
        } else if let Some(name) = &shot.output {
            if !self.space.outputs().chain(self.mirrors.iter()).any(|o| &o.name() == name) {
                return Err(format!("no output {name}"));
            }
        } else {
            let at = self.pointer.current_location();
            shot.output = self.space.output_under(at).next().or_else(|| self.space.outputs().next()).map(|o| o.name());
        }
        self.ipc.pending_screenshots.push(shot);
        Ok(())
    }
}

impl IpcState {
    /// The shots `output`'s frame should take.
    pub fn shots_for(&mut self, output: &Output) -> Vec<Shot> {
        let name = output.name();
        let (mine, rest) = std::mem::take(&mut self.pending_screenshots)
            .into_iter()
            .partition(|s| s.output.as_ref().is_none_or(|o| *o == name));
        self.pending_screenshots = rest;
        mine
    }
}

/// Take `shot` with `output`'s renderer and hand it to a thread to save.
/// `overlays` are what the backend draws over the windows (the lock screen,
/// the twitcher), for whole-screen shots.
#[allow(clippy::too_many_arguments)]
pub fn take<R, T>(
    ipc: &mut IpcState,
    shot: Shot,
    renderer: &mut R,
    output: &Output,
    space: &Space<WindowElement>,
    overlays: Vec<CustomRenderElements<R>>,
    clear_color: Color32F,
    locked: bool,
    (pointer, shape): (Point<f64, Logical>, Option<crate::cursor::Shape>),
) where
    R: Renderer + ImportAll + ImportMem + Offscreen<T> + ExportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: std::fmt::Display,
{
    let scale = output.current_scale().fractional_scale();
    let captured = match shot.window {
        _ if shot.window.is_some() && locked => Err("the screen is locked".to_owned()),
        Some(id) => capture_window(renderer, space, id, scale, shot.border),
        None => crate::render::capture(renderer, output, space, overlays, clear_color, locked)
            .map(|img| (img, space.output_geometry(output).unwrap_or_default())),
    };
    let captured = captured.map(|(img, rect)| {
        // Where the pointer is in the picture, in pixels (if it's in it).
        let at = ((pointer - rect.loc.to_f64()).to_physical(scale), scale);
        let cursor = shape.filter(|_| shot.pointer && !locked && rect.to_f64().contains(pointer)).map(|shape| (at.0, at.1, shape));
        (img, cursor)
    });
    save(ipc, shot, captured);
}

/// Render one window, alone over transparency: its tab and border too
/// with `border` (the tab's shape is kept), or only what the client drew.
fn capture_window<R, T>(
    renderer: &mut R,
    space: &Space<WindowElement>,
    id: u64,
    scale: f64,
    border: bool,
) -> Result<(image::RgbaImage, Rectangle<i32, Logical>), String>
where
    R: Renderer + ImportAll + ImportMem + Offscreen<T> + ExportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: std::fmt::Display,
{
    let window = space.elements().find(|w| w.id() == id).ok_or("no such window on screen")?;
    let at = space.element_location(window).ok_or("no such window on screen")?;
    let geo = SpaceElement::geometry(window);
    // Where the element draws from, as Space does it.
    let origin = at - geo.loc;
    let rect = if border || !window.decorated() {
        Rectangle::new(at, geo.size)
    } else {
        Rectangle::new(origin + crate::shell::deco::content_offset(), window.content_size())
    };
    let location = (origin - rect.loc).to_f64().to_physical(scale).to_i32_round();
    let elements: Vec<WindowRenderElement<R>> = window.render_elements(renderer, location, scale.into(), 1.0);
    let size = rect.size.to_f64().to_physical(scale).to_i32_round();
    let img = crate::render::offscreen(renderer, size, scale, &elements, Color32F::TRANSPARENT)?;
    Ok((img, rect))
}

/// A captured image, and where to draw the pointer (in pixels, at a scale).
type Captured = (image::RgbaImage, Option<(Point<f64, smithay::utils::Physical>, f64, crate::cursor::Shape)>);

/// Draw the pointer in, encode and write on a thread; answer when done.
fn save(ipc: &mut IpcState, shot: Shot, captured: Result<Captured, String>) {
    let (mut img, cursor) = match captured {
        Ok(c) => c,
        Err(message) => {
            warn!(%message, "screenshot failed");
            if let Some(conn) = shot.conn {
                conn.send(&Reply::Error { message });
            }
            return;
        }
    };
    let (id, done) = ipc.saving(shot.conn);
    let (path, format) = (shot.path, shot.format);
    std::thread::Builder::new()
        .name("screenshot".into())
        .spawn(move || {
            if let Some((at, scale, shape)) = cursor {
                draw_cursor(&mut img, at, scale, shape);
            }
            unpremultiply(&mut img);
            let mut part = path.clone().into_os_string();
            part.push(".part");
            let part = PathBuf::from(part);
            let res = match format {
                ImageFormat::Png => image::DynamicImage::ImageRgba8(img)
                    .save_with_format(&part, image::ImageFormat::Png)
                    .and_then(|()| std::fs::rename(&part, &path).map_err(Into::into))
                    .map_err(|e| e.to_string()),
                ImageFormat::Rgba => {
                    let mut bytes = Vec::with_capacity(8 + img.as_raw().len());
                    bytes.extend_from_slice(&img.width().to_le_bytes());
                    bytes.extend_from_slice(&img.height().to_le_bytes());
                    bytes.extend_from_slice(img.as_raw());
                    std::fs::write(&part, bytes).and_then(|()| std::fs::rename(&part, &path)).map_err(|e| e.to_string())
                }
            };
            if let Some(tx) = done {
                let _ = tx.send((id, res));
            }
        })
        .expect("thread");
}

/// GL hands back premultiplied pixels; PNG wants them straight.
fn unpremultiply(img: &mut image::RgbaImage) {
    for p in img.pixels_mut() {
        let a = p.0[3] as u32;
        if a != 0 && a != 255 {
            for k in 0..3 {
                p.0[k] = (p.0[k] as u32 * 255 / a).min(255) as u8;
            }
        }
    }
}

/// The pointer (premultiplied, as `img` is) over `img`.
fn draw_cursor(img: &mut image::RgbaImage, at: Point<f64, smithay::utils::Physical>, scale: f64, shape: crate::cursor::Shape) {
    let c = crate::cursor::image(shape, scale);
    let (hx, hy) = shape.hotspot();
    let (x0, y0) = (at.x.round() as i64 - (hx as f64 * scale).round() as i64, at.y.round() as i64 - (hy as f64 * scale).round() as i64);
    for y in 0..c.size as i64 {
        for x in 0..c.size as i64 {
            let (dx, dy) = (x0 + x, y0 + y);
            if dx < 0 || dy < 0 || dx >= img.width() as i64 || dy >= img.height() as i64 {
                continue;
            }
            let i = ((y * c.size as i64 + x) * 4) as usize;
            let src = &c.rgba[i..i + 4];
            let a = src[3] as u32;
            if a == 0 {
                continue;
            }
            let dst = img.get_pixel_mut(dx as u32, dy as u32);
            for (d, &s) in dst.0.iter_mut().zip(src).take(3) {
                *d = (s as u32 + *d as u32 * (255 - a) / 255).min(255) as u8;
            }
            dst.0[3] = (a + dst.0[3] as u32 * (255 - a) / 255).min(255) as u8;
        }
    }
}
