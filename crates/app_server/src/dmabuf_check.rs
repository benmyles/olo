//! Clients' new buffers (dmabufs), checked off the event loop. The first
//! import of a buffer, when the driver takes in its memory, can take
//! milliseconds (3–8 ms for a window-sized buffer on Apple's GPUs), and a
//! window being resized has new buffers every frame: checked on the loop,
//! a resize held up everything else for tens of milliseconds at a time
//! (tools/dev/perf-fuzz). So a worker thread makes an EGL image of each
//! new buffer, the slow part; while that image lives, the renderer's own
//! import back on the loop finds the memory already taken in and takes
//! microseconds. Then the client is told the buffer is good (or not), and
//! the worker's image goes. Clients may draw into a buffer and show it
//! before it is checked (Mesa's Vulkan does); such a commit waits for the
//! check (`Gate`), the window showing its last frame meanwhile, rather
//! than the renderer importing the buffer the slow way.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        egl::{EGLDisplay, ffi::egl::types::EGLImage},
    },
    reexports::{calloop::channel, wayland_server::Client},
    wayland::{
        compositor::{Blocker, BlockerState},
        dmabuf::ImportNotifier,
    },
};

/// Holds back commits of a buffer until its check is done: a blocker
/// (`compositor::add_blocker`) for each, and the clients to tell
/// (`blocker_cleared`) when it opens.
#[derive(Clone, Default)]
pub struct Gate(Arc<(AtomicBool, Mutex<Vec<Client>>)>);

impl Blocker for Gate {
    fn state(&self) -> BlockerState {
        if self.0.0.load(Ordering::Acquire) { BlockerState::Released } else { BlockerState::Pending }
    }
}

impl Gate {
    /// A commit of `client`'s waits for this.
    pub fn hold(&self, client: Client) {
        self.0.1.lock().unwrap().push(client);
    }

    /// Done: the clients whose commits were waiting.
    pub fn open(&self) -> Vec<Client> {
        self.0.0.store(true, Ordering::Release);
        std::mem::take(&mut *self.0.1.lock().unwrap())
    }
}

/// The worker's EGL image of a buffer, destroyed when dropped (on the loop,
/// after the renderer's own import).
pub struct Image {
    display: EGLDisplay,
    image: EGLImage,
}

// An EGL image is a handle to the display's, which any thread may use.
unsafe impl Send for Image {}

impl Drop for Image {
    fn drop(&mut self) {
        unsafe {
            smithay::backend::egl::ffi::egl::DestroyImageKHR(**self.display.get_display_handle(), self.image);
        }
    }
}

/// The worker's answer for buffer `id`: its image, if the driver took the
/// buffer at all.
pub struct Prepared {
    id: u64,
    pub image: Option<Image>,
}

/// Buffers being checked, and the worker checking them.
pub struct Checks {
    tx: mpsc::Sender<(u64, Dmabuf)>,
    pending: HashMap<u64, (Dmabuf, ImportNotifier, Gate)>,
    next: u64,
}

impl Checks {
    /// A worker for `display`, answering through `done` (a channel in the
    /// event loop, which hands each answer to [`Checks::finish`]).
    pub fn new(display: EGLDisplay, done: channel::Sender<Prepared>) -> Checks {
        let (tx, rx) = mpsc::channel::<(u64, Dmabuf)>();
        let _ = std::thread::Builder::new().name("dmabuf check".into()).spawn(move || {
            for (id, dmabuf) in rx {
                let image = display.create_image_from_dmabuf(&dmabuf).ok().map(|image| Image { display: display.clone(), image });
                if done.send(Prepared { id, image }).is_err() {
                    break;
                }
            }
        });
        Checks { tx, pending: HashMap::new(), next: 0 }
    }

    /// Check `dmabuf` on the worker, telling `notifier` later; or, if the
    /// worker is gone, hand them back to be checked here.
    pub fn start(&mut self, dmabuf: Dmabuf, notifier: ImportNotifier) -> Option<(Dmabuf, ImportNotifier)> {
        let id = self.next;
        self.next += 1;
        if self.tx.send((id, dmabuf.clone())).is_err() {
            return Some((dmabuf, notifier));
        }
        self.pending.insert(id, (dmabuf, notifier, Gate::default()));
        None
    }

    /// What a commit of `dmabuf` must wait for, while it is being checked.
    pub fn gate(&self, dmabuf: &Dmabuf) -> Option<Gate> {
        self.pending.values().find(|(d, _, _)| d == dmabuf).map(|(_, _, g)| g.clone())
    }

    /// The buffer `p` answers for, who to tell, and the gate to open once
    /// the renderer has it.
    pub fn finish(&mut self, p: &Prepared) -> Option<(Dmabuf, ImportNotifier, Gate)> {
        self.pending.remove(&p.id)
    }
}
