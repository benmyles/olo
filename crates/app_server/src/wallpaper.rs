//! The desktop picture: Backgrounds' wallpaper, scaled to cover each output
//! and drawn behind everything (Tracker's desktop layer is transparent, and
//! the workspace colour shows until the picture is ready or if it can't be
//! read). Decoding and scaling a camera-sized JPEG takes a while, so it is
//! done on a worker thread per output size; the finished buffer is kept on
//! the output, where `render::output_elements` finds it. A picture from
//! the pack (`tabula_xdg::wallpapers`) comes in the version for each
//! screen's shape and the current mode.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use image::{DynamicImage, RgbaImage, imageops::FilterType};
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
    reexports::calloop::channel::{self, Event as ChannelEvent},
    utils::{Buffer, Rectangle, Size, Transform},
};
use tracing::{info, warn};

use crate::state::{AnvilState, Backend};

/// What an output should show: this file at this physical size.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    path: PathBuf,
    size: (u32, u32),
}

/// Kept in each output's user data.
#[derive(Default)]
struct Slot {
    /// The picture asked for (being decoded, shown, or failed).
    wanted: Option<Key>,
    shown: Option<MemoryRenderBuffer>,
}

type Done = (String, Key, Result<RgbaImage, String>);

/// The worker threads' way back to the event loop.
#[derive(Debug, Default)]
pub struct Loader {
    tx: Option<channel::Sender<Done>>,
}

fn slot(output: &Output) -> &Mutex<Slot> {
    output.user_data().insert_if_missing_threadsafe(|| Mutex::new(Slot::default()));
    output.user_data().get::<Mutex<Slot>>().unwrap()
}

/// The size an output's framebuffer is, after rotation.
fn physical_size(output: &Output) -> Option<(u32, u32)> {
    let mode = output.current_mode()?;
    let size = output.current_transform().transform_size(mode.size);
    (size.w > 0 && size.h > 0).then_some((size.w as u32, size.h as u32))
}

/// JPEG XL (what Fedora's own backgrounds are) by its signature: a bare
/// codestream or the ISO container.
/// Decode `path`, the right way up.
fn decode(path: &Path) -> Result<DynamicImage, String> {
    tabula_images::open(path).map(|(img, _)| img)
}

/// Read `path` the right way up and scale it to cover `size`, cropping the
/// overhang evenly (a centred "fill", as most desktops default to).
fn load(path: &Path, (w, h): (u32, u32)) -> Result<RgbaImage, String> {
    let img = decode(path)?;
    let (sw, sh) = (img.width().max(1), img.height().max(1));
    let scale = (w as f64 / sw as f64).max(h as f64 / sh as f64);
    let (cw, ch) = (((w as f64 / scale).round() as u32).clamp(1, sw), ((h as f64 / scale).round() as u32).clamp(1, sh));
    let cropped = img.crop_imm((sw - cw) / 2, (sh - ch) / 2, cw, ch);
    let mut out = image::imageops::resize(&cropped.to_rgba8(), w, h, FilterType::CatmullRom);
    // The renderer wants premultiplied alpha.
    for p in out.pixels_mut() {
        let a = p[3] as u32;
        if a < 255 {
            for c in &mut p.0[..3] {
                *c = (*c as u32 * a / 255) as u8;
            }
        }
    }
    Ok(out)
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Listen for finished pictures. Called once at start-up.
    pub fn start_wallpaper(&mut self) {
        let (tx, rx) = channel::channel::<Done>();
        let _ = self.handle.insert_source(rx, |event, _, state| {
            let ChannelEvent::Msg((name, key, result)) = event else { return };
            let Some(output) = state.space.outputs().find(|o| o.name() == name).cloned() else { return };
            let mut slot = slot(&output).lock().unwrap();
            if slot.wanted.as_ref() != Some(&key) {
                return; // Superseded while it was being read.
            }
            match result {
                Ok(img) => {
                    let size: Size<i32, Buffer> = (img.width() as i32, img.height() as i32).into();
                    let opaque = img.pixels().all(|p| p[3] == 255);
                    slot.shown = Some(MemoryRenderBuffer::from_slice(
                        img.as_raw(),
                        Fourcc::Abgr8888,
                        size,
                        1,
                        Transform::Normal,
                        opaque.then(|| vec![Rectangle::from_size(size)]),
                    ));
                    info!(output = name, path = %key.path.display(), "wallpaper ready");
                }
                Err(e) => {
                    warn!(path = %key.path.display(), "can't show the wallpaper: {e}");
                    slot.shown = None;
                }
            }
        });
        self.wallpaper.tx = Some(tx);
    }

    /// Make sure `output` has (or is getting) the current wallpaper at its
    /// current size. Cheap when nothing changed; called before each frame.
    pub fn update_wallpaper(&mut self, output: &Output) {
        // A pack picture: the version for the screen's shape and the mode
        // (so it changes with dark mode, Auto's schedule too).
        let dark = self.settings.appearance.effective_theme == tabula_theme::ThemeMode::Dark;
        let want = self.settings.wallpaper.as_deref().zip(physical_size(output)).and_then(|(setting, size)| {
            let path = tabula_xdg::wallpapers::resolve(setting, size, dark)?;
            Some(Key { path, size })
        });
        let mut slot = slot(output).lock().unwrap();
        if slot.wanted == want {
            return;
        }
        let Some(key) = want else {
            *slot = Slot::default();
            return;
        };
        // A new size of the same picture keeps the old one up meanwhile;
        // a different picture too, so the change is a single swap.
        slot.wanted = Some(key.clone());
        let Some(tx) = self.wallpaper.tx.clone() else { return };
        let name = output.name();
        std::thread::spawn(move || {
            let result = load(&key.path, key.size);
            let _ = tx.send((name, key, result));
        });
    }
}

/// The wallpaper element for `output`, covering it, if one is ready.
pub fn element<R>(renderer: &mut R, output: &Output) -> Option<MemoryRenderBufferRenderElement<R>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let buffer = slot(output).lock().unwrap().shown.clone()?;
    let (w, h) = physical_size(output)?;
    let scale = output.current_scale().fractional_scale();
    let logical = ((w as f64 / scale).round() as i32, (h as f64 / scale).round() as i32);
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, 0.0),
        &buffer,
        None,
        Some(Rectangle::from_size((w as f64, h as f64).into())),
        Some(logical.into()),
        Kind::Unspecified,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_and_crops() {
        let dir = std::env::temp_dir().join(format!("tabula-wallpaper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A wide picture: left half red, right half blue, a green stripe in the middle.
        let img = RgbaImage::from_fn(400, 100, |x, _| match x {
            0..190 => image::Rgba([255, 0, 0, 255]),
            190..210 => image::Rgba([0, 255, 0, 255]),
            _ => image::Rgba([0, 0, 255, 255]),
        });
        let path = dir.join("wide.png");
        img.save(&path).unwrap();
        // Into a square: the middle 100×100 survives, scaled to 50×50.
        let out = load(&path, (50, 50)).unwrap();
        assert_eq!(out.dimensions(), (50, 50));
        assert_eq!(out.get_pixel(25, 25)[1], 255);
        assert!(out.get_pixel(0, 25)[0] > 200);
        assert!(out.get_pixel(49, 25)[2] > 200);
        assert!(load(&dir.join("missing.png"), (10, 10)).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn jpeg_xl() {
        // Fedora ships its backgrounds as JPEG XL; use one when present.
        let Some(path) = std::fs::read_dir("/usr/share/backgrounds/gnome")
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "jxl"))
        else {
            return;
        };
        let out = load(&path, (64, 40)).unwrap();
        assert_eq!(out.dimensions(), (64, 40));
    }
}
