//! Live pictures of screens and windows, shrunk for a picker's
//! thumbnails (Share Screen's, and Open in Which Window?'s): a capture of
//! each on a worker of its own, a new picture whenever it changed and its
//! item's time has come (a screen twice a second, a window every two:
//! app_server answers a frame only once something changed), shrunk to the
//! size its thumbnail is drawn at in device pixels, until stopped. Frames
//! are read into shared memory (`Capture::in_memory`: a dmabuf mapped
//! for reading in a GPU-drawing app's own process came back black) and
//! only ever from a whole frame (`Capture::frame_size`).

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{Capture, Source};

/// Something to keep a picture of.
#[derive(Clone)]
pub struct Item {
    /// What its pictures are named by, when they come.
    pub key: String,
    pub source: Source,
    /// The least time between two of its pictures.
    pub every: Duration,
    /// The size its picture is made to fit, in device pixels.
    pub size: (u32, u32),
}

/// A picture made: its item's key, its size, and its pixels (BGRA,
/// opaque: the frame's own order).
pub struct Picture {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// The worker, running until this is dropped (or `stop`).
pub struct Live {
    stop: Arc<AtomicBool>,
    sizes: Arc<Mutex<HashMap<String, (u32, u32)>>>,
}

impl Live {
    /// Keep pictures of `items`, each handed to `send` as it's made (on
    /// the worker's thread).
    pub fn start(items: Vec<Item>, send: impl FnMut(Picture) + Send + 'static) -> Live {
        let stop = Arc::new(AtomicBool::new(false));
        let sizes = Arc::new(Mutex::new(items.iter().map(|i| (i.key.clone(), i.size)).collect::<HashMap<_, _>>()));
        let (s, z) = (stop.clone(), sizes.clone());
        let _ = std::thread::Builder::new().name("thumbnails".into()).spawn(move || run(items, &s, &z, send));
        Live { stop, sizes }
    }

    /// `key`'s pictures are made to fit `size` from now on (the scale
    /// changed): one comes at once, from its last frame.
    pub fn set_size(&self, key: &str, size: (u32, u32)) {
        if let Some(s) = self.sizes.lock().unwrap_or_else(|e| e.into_inner()).get_mut(key) {
            *s = size;
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(items: Vec<Item>, stop: &AtomicBool, sizes: &Mutex<HashMap<String, (u32, u32)>>, mut send: impl FnMut(Picture)) {
    let mut captures: Vec<(Item, Option<Capture>, Instant)> = items
        .into_iter()
        .map(|item| {
            let mut c = Capture::new(&item.source, false).ok().map(Capture::in_memory);
            if let Some(cap) = c.as_mut()
                && cap.request().is_err()
            {
                c = None;
            }
            (item, c, Instant::now())
        })
        .collect();
    // The size each was last shown at.
    let mut shown: HashMap<String, (u32, u32)> = HashMap::new();
    // A frame's pixels, read into the same buffer each time.
    let mut frame = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        for (item, capture, next) in captures.iter_mut() {
            let want = sizes.lock().unwrap_or_else(|e| e.into_inner()).get(&item.key).copied().unwrap_or(item.size);
            let Some(cap) = capture.as_mut() else { continue };
            if cap.stopped() {
                *capture = None;
                continue;
            }
            match cap.poll() {
                Ok(Some(Ok(()))) => {
                    if let Some(p) = shrink(cap, want, &mut frame) {
                        shown.insert(item.key.clone(), want);
                        send(Picture { key: item.key.clone(), width: p.0, height: p.1, bgra: p.2 });
                    }
                    *next = Instant::now() + item.every;
                }
                Ok(Some(Err(_))) => *next = Instant::now() + item.every,
                Ok(None) => {}
                Err(_) => *capture = None,
            }
            // The scale changed: this size again, from the last frame.
            if shown.get(&item.key).is_some_and(|s| *s != want)
                && let Some(cap) = capture.as_ref()
                && let Some(p) = shrink(cap, want, &mut frame)
            {
                shown.insert(item.key.clone(), want);
                send(Picture { key: item.key.clone(), width: p.0, height: p.1, bgra: p.2 });
            }
            if let Some(cap) = capture.as_mut()
                && !cap.waiting()
                && Instant::now() >= *next
            {
                let _ = cap.request();
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    // A screen's frames (16 MB each at 2560×1600) were this thread's: its
    // heap goes back, not kept by the allocator for a thread that's ending.
    drop((captures, frame));
    // SAFETY: glibc's malloc_trim takes no pointers.
    unsafe { libc::malloc_trim(0) };
}

/// The last frame of `cap`, shrunk to fit `(w, h)` device pixels in its
/// own proportions: its size and its pixels (BGRA, opaque).
fn shrink(cap: &Capture, (w, h): (u32, u32), frame: &mut Vec<u8>) -> Option<(u32, u32, Vec<u8>)> {
    use fast_image_resize::{
        PixelType, ResizeAlg, ResizeOptions, Resizer,
        images::{Image, ImageRef},
    };
    let (sw, sh) = cap.frame_size()?;
    frame.resize((sw * sh * 4) as usize, 0);
    cap.copy_into(frame, sw as usize * 4).ok()?;
    let fit = (w as f64 / sw as f64).min(h as f64 / sh as f64);
    let (tw, th) = (((sw as f64 * fit).round() as u32).clamp(1, w.max(1)), ((sh as f64 * fit).round() as u32).clamp(1, h.max(1)));
    let src = ImageRef::new(sw, sh, frame, PixelType::U8x4).ok()?;
    let mut small = Image::new(tw, th, PixelType::U8x4);
    Resizer::new().resize(&src, &mut small, &ResizeOptions::new().resize_alg(ResizeAlg::SuperSampling(fast_image_resize::FilterType::Bilinear, 2))).ok()?;
    // BGRx: opaque.
    let mut out = small.into_vec();
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    Some((tw, th, out))
}
