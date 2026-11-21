//! Live pictures of screens and windows, shrunk for a picker's
//! thumbnails (Share Screen's, and Open in Which Window?'s): a capture of
//! each on a worker of its own, a new picture whenever it changed and its
//! item's time has come (a screen twice a second, a window every two:
//! app_server answers a frame only once something changed), shrunk to the
//! size its thumbnail is drawn at in device pixels, until stopped. Frames
//! are read into shared memory (`Capture::in_memory`: a dmabuf mapped
//! for reading in a GPU-drawing app's own process came back black) and
//! only ever from a whole frame (`Capture::frame_size`).
//!
//! Live while someone looks ([`Live::set_live`]: the picker says whether
//! its window is the active one): a picker left open behind other
//! windows asks for nothing, and its pictures stay as they were until
//! it is looked at again (six choosers nobody had answered each read
//! two browser windows whole once a second, for hours: 2026-11-21). And
//! the worker sleeps until app_server answers one of its captures, the
//! next one's time comes, or it is told something: it looked 25 times a
//! second whether or not anything was due.

use std::{
    collections::HashMap,
    os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
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
    live: Arc<AtomicBool>,
    sizes: Arc<Mutex<HashMap<String, (u32, u32)>>>,
    /// What wakes the worker when it's told something (an eventfd; None
    /// where the kernel gave none: it then looks every 40 ms, as it did).
    wake: Option<Arc<OwnedFd>>,
}

impl Live {
    /// Keep pictures of `items`, each handed to `send` as it's made (on
    /// the worker's thread).
    pub fn start(items: Vec<Item>, send: impl FnMut(Picture) + Send + 'static) -> Live {
        let stop = Arc::new(AtomicBool::new(false));
        let live = Arc::new(AtomicBool::new(true));
        let sizes = Arc::new(Mutex::new(items.iter().map(|i| (i.key.clone(), i.size)).collect::<HashMap<_, _>>()));
        // SAFETY: a new descriptor of this process's own, owned from here.
        let wake = match unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) } {
            fd if fd >= 0 => Some(Arc::new(unsafe { OwnedFd::from_raw_fd(fd) })),
            _ => None,
        };
        let (s, l, z, w) = (stop.clone(), live.clone(), sizes.clone(), wake.clone());
        let _ = std::thread::Builder::new().name("thumbnails".into()).spawn(move || run(items, &s, &l, &z, w.as_deref(), send));
        Live { stop, live, sizes, wake }
    }

    fn wake(&self) {
        if let Some(fd) = &self.wake {
            // SAFETY: eight bytes of ours written to a descriptor of ours.
            unsafe { libc::write(fd.as_raw_fd(), 1u64.to_ne_bytes().as_ptr().cast(), 8) };
        }
    }

    /// `key`'s pictures are made to fit `size` from now on (the scale
    /// changed): one comes at once, from its last frame.
    pub fn set_size(&self, key: &str, size: (u32, u32)) {
        if let Some(s) = self.sizes.lock().unwrap_or_else(|e| e.into_inner()).get_mut(key) {
            *s = size;
        }
        self.wake();
    }

    /// Whether anyone is looking (the picker's window is the active
    /// one): while not, no new picture is asked for, and the ones there
    /// stay; looked at again, each whose time has come is asked for at
    /// once. Live to begin with.
    pub fn set_live(&self, live: bool) {
        if self.live.swap(live, Ordering::Relaxed) != live {
            self.wake();
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake();
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(items: Vec<Item>, stop: &AtomicBool, live: &AtomicBool, sizes: &Mutex<HashMap<String, (u32, u32)>>, wake: Option<&OwnedFd>, mut send: impl FnMut(Picture)) {
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
    // Each capture's connection, to wait on (None: one that gave none).
    let waits: Vec<Option<OwnedFd>> = captures.iter().map(|(_, c, _)| c.as_ref().and_then(|c| c.fd().ok())).collect();
    // The size each was last shown at.
    let mut shown: HashMap<String, (u32, u32)> = HashMap::new();
    // A frame's pixels, read into the same buffer each time.
    let mut frame = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let looking = live.load(Ordering::Relaxed);
        // When the next one's time comes, of those that wait for it.
        let mut due: Option<Instant> = None;
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
                && looking
            {
                if Instant::now() >= *next {
                    let _ = cap.request();
                } else {
                    due = Some(due.map_or(*next, |d| d.min(*next)));
                }
            }
        }
        // Asleep until app_server says something to one of them, the
        // next one's time comes, or it is told (to stop, a size, that
        // someone is looking again): not a look every 40 ms.
        let mut fds: Vec<libc::pollfd> = captures
            .iter()
            .zip(&waits)
            .filter(|((_, capture, _), _)| capture.is_some())
            .filter_map(|(_, fd)| fd.as_ref())
            .chain(wake)
            .map(|fd| libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 })
            .collect();
        let waited_on = captures.iter().zip(&waits).filter(|((_, c, _), fd)| c.is_some() && fd.is_some()).count();
        let most = match due {
            // (A capture with nothing to wait on, or no way to be woken:
            // looked at as before.)
            _ if wake.is_none() || waited_on < captures.iter().filter(|(_, c, _)| c.is_some()).count() => 40,
            Some(at) => at.saturating_duration_since(Instant::now()).as_millis().min(60_000) as libc::c_int + 1,
            None => -1,
        };
        // SAFETY: an array of this thread's own, its length given.
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, most) };
        if let Some(fd) = wake {
            let mut count = [0u8; 8];
            // SAFETY: eight bytes of ours read from a descriptor of ours
            // (non-blocking: nothing there is no wait).
            unsafe { libc::read(fd.as_raw_fd(), count.as_mut_ptr().cast(), 8) };
        }
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
