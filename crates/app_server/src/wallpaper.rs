//! The desktop picture: Backgrounds' wallpaper, scaled to cover each output
//! and drawn behind everything (Tracker's desktop layer is transparent, and
//! the workspace colour shows until the picture is ready or if it can't be
//! read). Decoding and scaling a camera-sized JPEG takes a while, so it is
//! done on a worker thread, one picture at a time and only the latest asked
//! for each output (flipping the theme quickly would otherwise decode a
//! dozen at once); the finished buffer is kept on the output, where
//! `render::output_elements` finds it. A picture from
//! the pack (`olo_xdg::wallpapers`) comes in the version for each
//! screen's shape and the current mode. On a display in HDR a picture
//! with a gain map beside it (`olo_images::gain`: the pack's all have
//! one) is shown in HDR: the map scaled and cropped as the picture is
//! and multiplied in, into half floats of linear light (1.0 SDR's white),
//! so its light sources and glints shine above SDR's white.

use olo_sync::Mutex;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

use crate::render::Exact;
use fast_image_resize::{
    self as fir, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use image::{DynamicImage, RgbaImage};
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

/// What an output should show: this file at this physical size, in HDR
/// with this gain map (a display in HDR, a picture that has one).
#[derive(Debug, Clone, PartialEq)]
struct Key {
    path: PathBuf,
    size: (u32, u32),
    gain: Option<PathBuf>,
}

/// Kept in each output's user data.
#[derive(Default)]
struct Slot {
    /// The picture asked for (being decoded, shown, or failed).
    wanted: Option<Key>,
    shown: Option<Strips>,
    /// A new picture on its way to the GPU, and how many of its strips are
    /// there: shown once they all are.
    coming: Option<(Strips, usize)>,
}

/// A picture cut across into strips of about `STRIP_BYTES`, each a texture
/// of its own: a frame uploads one of a new picture's (a whole screen's
/// is 30 MB, which held a frame up 20 ms), the old picture showing till
/// they're all there. Each strip: its top, its height, its pixels; then
/// the picture's width, and whether it's HDR's linear light.
#[derive(Clone)]
struct Strips(Vec<(i32, i32, MemoryRenderBuffer)>, i32, bool);

/// What the worker makes: 8-bit sRGB, or HDR's half floats.
enum Picture {
    Sdr(RgbaImage),
    Hdr { w: usize, h: usize, pixels: Vec<u16> },
}

/// What an HDR wallpaper's texture holds: linear light, sRGB's primaries,
/// 1.0 SDR's white.
const HDR_COLOR: smithay::backend::renderer::ColorSource = smithay::backend::renderer::ColorSource { primaries: olo_color::Primaries::SRGB, transfer: olo_color::Transfer::Linear };

const STRIP_BYTES: usize = 4 << 20;

/// A worker's picture for an output, ready to draw.
type Done = (String, Key, Result<Strips, String>);

/// What the worker is asked: a picture for an output, or to give back
/// the memory pictures took (once a new one has replaced the old).
enum Job {
    Load(String, Key),
    Trim,
}

/// The way to the worker.
#[derive(Debug, Default)]
pub struct Loader {
    jobs: Option<mpsc::Sender<Job>>,
}

/// The worker: the latest picture asked for each output, one at a time,
/// each sent back to the loop through `done` ready to draw.
fn work(jobs: mpsc::Receiver<Job>, done: channel::Sender<Done>) {
    while let Ok(job) = jobs.recv() {
        let mut todo: Vec<(String, Key)> = Vec::new();
        let mut trim = false;
        for job in std::iter::once(job).chain(jobs.try_iter()) {
            match job {
                Job::Load(name, key) => {
                    todo.retain(|(n, _)| *n != name);
                    todo.push((name, key));
                }
                Job::Trim => trim = true,
            }
        }
        for (name, key) in todo {
            // Ready to draw: copied into its strips' buffers here, not on
            // the loop.
            let started = std::time::Instant::now();
            let result = load_picture(&key).map(|picture| {
                let (w, h, bytes, format, hdr, opaque) = match &picture {
                    Picture::Sdr(img) => (img.width() as usize, img.height() as usize, std::borrow::Cow::Borrowed(img.as_raw().as_slice()), Fourcc::Abgr8888, false, img.pixels().all(|p| p[3] == 255)),
                    Picture::Hdr { w, h, pixels } => {
                        let one = half::f16::ONE.to_bits();
                        let opaque = pixels.chunks_exact(4).all(|p| p[3] == one);
                        (*w, *h, std::borrow::Cow::Owned(pixels.iter().flat_map(|v| v.to_ne_bytes()).collect::<Vec<u8>>()), Fourcc::Abgr16161616f, true, opaque)
                    }
                };
                let bpp = if hdr { 8 } else { 4 };
                let rows = (STRIP_BYTES / (w * bpp).max(1)).clamp(1, h.max(1));
                let strips = (0..h)
                    .step_by(rows)
                    .map(|top| {
                        let n = rows.min(h - top);
                        let size: Size<i32, Buffer> = (w as i32, n as i32).into();
                        let pixels = &bytes[top * w * bpp..(top + n) * w * bpp];
                        (top as i32, n as i32, MemoryRenderBuffer::from_slice(pixels, format, size, 1, Transform::Normal, opaque.then(|| vec![Rectangle::from_size(size)])))
                    })
                    .collect();
                if hdr {
                    info!(path = %key.path.display(), ms = started.elapsed().as_millis() as u64, mb = (w * h * bpp) >> 20, "HDR wallpaper made");
                }
                Strips(strips, w as i32, hdr)
            });
            if done.send((name, key, result)).is_err() {
                return;
            }
        }
        // What reading took, and the pictures replaced, go back to the
        // system rather than staying with the allocator for good.
        #[cfg(target_env = "gnu")]
        if trim {
            unsafe {
                libc::malloc_trim(0);
            }
        }
    }
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
    olo_images::open(path).map(|(img, _)| img)
}

/// Read `path` the right way up and scale it to cover `size`, cropping the
/// overhang evenly (a centred "fill", as most desktops default to). One
/// copy of the picture at a time: cropped as it is scaled, in 8 bits
/// (an 8K picture would otherwise pass through several full-size copies
/// and a float one, hundreds of megabytes, for each display at once).
fn load(path: &Path, (w, h): (u32, u32)) -> Result<RgbaImage, String> {
    let img = decode(path)?.into_rgba8();
    let (sw, sh) = (img.width().max(1), img.height().max(1));
    let scale = (w as f64 / sw as f64).max(h as f64 / sh as f64);
    let (cw, ch) = (((w as f64 / scale).round() as u32).clamp(1, sw), ((h as f64 / scale).round() as u32).clamp(1, sh));
    let src = ImageRef::new(img.width(), img.height(), img.as_raw(), PixelType::U8x4).map_err(|e| e.to_string())?;
    let mut scaled = Image::new(w, h, PixelType::U8x4);
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(fir::FilterType::CatmullRom)).crop(((sw - cw) / 2) as f64, ((sh - ch) / 2) as f64, cw as f64, ch as f64);
    Resizer::new().resize(&src, &mut scaled, &options).map_err(|e| e.to_string())?;
    drop(img);
    let mut out = RgbaImage::from_raw(w, h, scaled.into_vec()).ok_or("scaled picture of the wrong size")?;
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

/// The picture `key` asks for: in HDR when it has a gain map (a picture
/// whose map can't be read is shown in SDR).
fn load_picture(key: &Key) -> Result<Picture, String> {
    let img = load(&key.path, key.size)?;
    let Some(gain) = &key.gain else { return Ok(Picture::Sdr(img)) };
    let source = image::image_dimensions(&key.path).map_err(|e| e.to_string())?;
    match load_gain(gain, source, key.size) {
        Ok(map) => Ok(Picture::Hdr { w: img.width() as usize, h: img.height() as usize, pixels: olo_images::gain::apply(img.as_raw(), map.as_raw()) }),
        Err(e) => {
            warn!(path = %gain.display(), "can't read the wallpaper's gain map, shown in SDR: {e}");
            Ok(Picture::Sdr(img))
        }
    }
}

/// Gain map `path` for a picture `source` pixels big, scaled and cropped
/// to `size` as `load` does the picture.
fn load_gain(path: &Path, (sw, sh): (u32, u32), (w, h): (u32, u32)) -> Result<image::GrayImage, String> {
    let map = image::open(path).map_err(|e| e.to_string())?.into_luma8();
    let (gw, gh) = (map.width().max(1), map.height().max(1));
    let (sw, sh) = (sw.max(1), sh.max(1));
    let scale = (w as f64 / sw as f64).max(h as f64 / sh as f64);
    let (cw, ch) = ((w as f64 / scale).min(sw as f64), (h as f64 / scale).min(sh as f64));
    // The picture's crop, in the map's pixels.
    let (kx, ky) = (gw as f64 / sw as f64, gh as f64 / sh as f64);
    let src = ImageRef::new(gw, gh, map.as_raw(), PixelType::U8).map_err(|e| e.to_string())?;
    let mut scaled = Image::new(w, h, PixelType::U8);
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(fir::FilterType::Bilinear)).crop((sw as f64 - cw) / 2.0 * kx, (sh as f64 - ch) / 2.0 * ky, cw * kx, ch * ky);
    Resizer::new().resize(&src, &mut scaled, &options).map_err(|e| e.to_string())?;
    image::GrayImage::from_raw(w, h, scaled.into_vec()).ok_or_else(|| "scaled gain map of the wrong size".into())
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Listen for finished pictures. Called once at start-up.
    pub fn start_wallpaper(&mut self) {
        let (done, rx) = channel::channel::<Done>();
        let (jobs, todo) = mpsc::channel();
        let _ = std::thread::Builder::new().name("wallpaper".into()).spawn(move || work(todo, done));
        let _ = self.handle.insert_source(rx, |event, _, state| {
            let ChannelEvent::Msg((name, key, result)) = event else { return };
            let start = std::time::Instant::now();
            state.wallpaper_arrived(name, key, result);
            state.backend_data.wake(None);
            state.timings.handled(|| "wallpaper ready".into(), start);
        });
        self.wallpaper.jobs = Some(jobs);
    }

    fn wallpaper_arrived(&mut self, name: String, key: Key, result: Result<Strips, String>) {
        let Some(output) = self.space.outputs().find(|o| o.name() == name).cloned() else { return };
        let mut slot = slot(&output).lock().unwrap();
        if slot.wanted.as_ref() != Some(&key) {
            return; // Superseded while it was being read.
        }
        match result {
            Ok(strips) => {
                // Shown once on the GPU (`element`).
                slot.coming = Some((strips, 0));
                info!(output = name, path = %key.path.display(), "wallpaper ready");
                // The picture replaced is freed: the worker gives the memory
                // back (off the loop, which that would hold up a moment).
                if let Some(jobs) = &self.wallpaper.jobs {
                    let _ = jobs.send(Job::Trim);
                }
            }
            Err(e) => {
                warn!(path = %key.path.display(), "can't show the wallpaper: {e}");
                (slot.shown, slot.coming) = (None, None);
            }
        }
    }

    /// Make sure `output` has (or is getting) the current wallpaper at its
    /// current size. Cheap when nothing changed; called before each frame.
    pub fn update_wallpaper(&mut self, output: &Output) {
        // A pack picture: the version for the screen's shape and the mode
        // (so it changes with dark mode, Auto's schedule too).
        let dark = self.settings.appearance.effective_theme == olo_theme::ThemeMode::Dark;
        let hdr = matches!(crate::color::drawing(output), crate::color::Drawing::Hdr { .. });
        let want = self.settings.wallpaper.as_deref().zip(physical_size(output)).and_then(|(setting, size)| {
            let path = olo_xdg::wallpapers::resolve(setting, size, dark)?;
            let gain = if hdr { olo_images::gain::find(&path) } else { None };
            Some(Key { path, size, gain })
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
        if let Some(jobs) = &self.wallpaper.jobs {
            let _ = jobs.send(Job::Load(output.name(), key));
        }
    }
}

/// A new picture is going up to the GPU for `output`, a strip a frame:
/// the display doesn't rest till it's shown.
pub fn coming(output: &Output) -> bool {
    slot(output).lock().unwrap().coming.is_some()
}

/// The wallpaper's elements for `output`, its strips covering it pixel
/// for pixel (it is made at the screen's size), if one is ready. A new
/// one is uploaded a strip each frame meanwhile.
pub fn element<R>(renderer: &mut R, output: &Output) -> Vec<Exact<MemoryRenderBufferRenderElement<R>>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let strip = |renderer: &mut R, w: i32, hdr: bool, (top, h, buffer): &(i32, i32, MemoryRenderBuffer)| {
        let e = crate::render::picture(renderer, (0.0, *top as f64).into(), buffer, (w, *h), None, Kind::Unspecified)?;
        Some(if hdr { e.with_color(HDR_COLOR) } else { e })
    };
    let mut slot = slot(output).lock().unwrap();
    if let Some((strips, done)) = &mut slot.coming {
        // (Made now, the element uploads its texture; it is kept with the
        // buffer for when it is shown.)
        if let Some(s) = strips.0.get(*done) {
            let _ = strip(renderer, strips.1, strips.2, s);
            *done += 1;
        }
        if *done >= strips.0.len() {
            slot.shown = slot.coming.take().map(|(s, _)| s);
        }
    }
    let Some(shown) = slot.shown.clone() else { return Vec::new() };
    drop(slot);
    shown.0.iter().filter_map(|s| strip(renderer, shown.1, shown.2, s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_and_crops() {
        let dir = std::env::temp_dir().join(format!("olo-wallpaper-{}", std::process::id()));
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

    /// What an HDR wallpaper costs against SDR, at the ThinkPad's
    /// 3200 × 2000: `cargo test --release -p app_server wallpaper_cost --
    /// --ignored --nocapture`.
    #[test]
    #[ignore]
    fn wallpaper_cost() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/wallpapers/4k");
        let path = dir.join("bebox-dark.png");
        let size = (3200, 2000);
        for gain in [None, olo_images::gain::find(&path)] {
            let key = Key { path: path.clone(), size, gain: gain.clone() };
            let mut times = Vec::new();
            let mut bytes = 0;
            for _ in 0..5 {
                let t = std::time::Instant::now();
                let p = load_picture(&key).unwrap();
                times.push(t.elapsed().as_secs_f64() * 1000.0);
                bytes = match p {
                    Picture::Sdr(img) => img.as_raw().len(),
                    Picture::Hdr { pixels, .. } => pixels.len() * 2,
                };
            }
            times.sort_by(f64::total_cmp);
            println!("{}: {:.0} ms (median of 5; {:.0}–{:.0}), {} MB", if gain.is_some() { "HDR" } else { "SDR" }, times[2], times[0], times[4], bytes >> 20);
        }
    }

    #[test]
    fn preferences_and_app_server_find_the_same_gain_maps() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/wallpapers/4k");
        for name in ["bebox.png", "bebox-dark.png", "aqua.png"] {
            let p = dir.join(name);
            assert_eq!(olo_xdg::wallpapers::has_hdr(&p), olo_images::gain::find(&p).is_some(), "{name}");
            assert!(olo_xdg::wallpapers::has_hdr(&p), "{name} has its gain map");
        }
    }

    #[test]
    fn jpeg_xl() {
        // Fedora ships its backgrounds as JPEG XL; use one when present.
        let Some(path) = std::fs::read_dir("/usr/share/backgrounds/gnome").into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "jxl")) else {
            return;
        };
        let out = load(&path, (64, 40)).unwrap();
        assert_eq!(out.dimensions(), (64, 40));
    }
}
