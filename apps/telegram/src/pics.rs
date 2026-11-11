//! Pictures (people's and chats', messages' photos, videos' first frames,
//! stickers): asked of the backend the first time they're drawn, decoded
//! on two workers once downloaded, then kept as GPUI images (BGRA,
//! straight alpha). Files fetched whole (to open or play) are remembered
//! by path.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
};

use olo_kit::{Proxy, RenderImage};
use olo_tg::{Backend, Cmd, Pic};

/// A picture decoded on a worker (sent to the app).
pub struct Decoded(pub Pic, pub Result<Arc<RenderImage>, String>);

enum Slot {
    Fetching,
    Decoding,
    Ready(Arc<RenderImage>),
    Failed,
}

pub struct Pics {
    slots: HashMap<Pic, Slot>,
    files: HashMap<Pic, PathBuf>,
    /// Downloads under way: how far.
    pub progress: HashMap<Pic, f32>,
    work: mpsc::Sender<(Pic, PathBuf)>,
}

/// The longest side kept: avatars small, the rest as large as a bubble
/// shows them at 2×.
fn most(pic: &Pic) -> u32 {
    match pic {
        Pic::Avatar(_) => 160,
        _ => 1024,
    }
}

fn decode(pic: &Pic, path: &Path) -> Result<Arc<RenderImage>, String> {
    let (img, _) = olo_images::open(path)?;
    let m = most(pic);
    let img = if img.width().max(img.height()) > m { img.thumbnail(m, m) } else { img };
    let mut bgra = img.to_rgba8();
    for p in bgra.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new([image::Frame::new(bgra)])))
}

impl Pics {
    pub fn new(proxy: Proxy) -> Pics {
        let (work, jobs) = mpsc::channel::<(Pic, PathBuf)>();
        let jobs = Arc::new(Mutex::new(jobs));
        for i in 0..2 {
            let (jobs, proxy) = (jobs.clone(), proxy.clone());
            let _ = std::thread::Builder::new().name(format!("telegram-pics-{i}")).spawn(move || {
                loop {
                    let job = jobs.lock().unwrap().recv();
                    let Ok((pic, path)) = job else { return };
                    let r = decode(&pic, &path);
                    proxy.send(Decoded(pic, r));
                }
            });
        }
        Pics { slots: HashMap::new(), files: HashMap::new(), progress: HashMap::new(), work }
    }

    /// `pic` once it's here (asked for the first time it's wanted).
    pub fn get(&mut self, pic: &Pic, backend: &Backend) -> Option<Arc<RenderImage>> {
        if !self.slots.contains_key(pic) {
            self.slots.insert(pic.clone(), Slot::Fetching);
            backend.send(Cmd::Fetch(pic.clone()));
            return None;
        }
        match self.slots.get(pic) {
            Some(Slot::Ready(t)) => Some(t.clone()),
            _ => None,
        }
    }

    /// The backend has it, at `path`.
    pub fn fetched(&mut self, pic: Pic, path: PathBuf) {
        self.progress.remove(&pic);
        self.files.insert(pic.clone(), path.clone());
        if matches!(pic, Pic::File(..)) {
            return;
        }
        self.slots.insert(pic.clone(), Slot::Decoding);
        let _ = self.work.send((pic, path));
    }

    pub fn fetch_failed(&mut self, pic: Pic) {
        self.progress.remove(&pic);
        self.slots.insert(pic, Slot::Failed);
    }

    pub fn decoded(&mut self, d: Decoded) {
        let slot = match d.1 {
            Ok(image) => Slot::Ready(image),
            Err(e) => {
                tracing::debug!("telegram: a picture couldn’t be read: {e}");
                Slot::Failed
            }
        };
        self.slots.insert(d.0, slot);
    }

    /// A whole file the backend has fetched.
    pub fn file(&self, pic: &Pic) -> Option<&Path> {
        self.files.get(pic).map(PathBuf::as_path)
    }

    /// Ask for a whole file (to open, play or save).
    pub fn fetch_file(&mut self, pic: Pic, backend: &Backend) {
        if self.files.contains_key(&pic) || self.progress.contains_key(&pic) {
            return;
        }
        self.progress.insert(pic.clone(), 0.0);
        backend.send(Cmd::Fetch(pic));
    }

    /// Forget a chat's avatar (its picture changed).
    pub fn forget(&mut self, pic: &Pic) {
        self.slots.remove(pic);
    }
}
