//! Clients' new buffers (dmabufs), checked off the event loop, on a GPU
//! device of the worker's own, which holds each buffer till the renderer
//! has let it go, so its memory is freed there too.
//!
//! The kernel takes a buffer into a GPU address space as it's bound there
//! (Mesa binds as it imports), and frees what that space let go of in its
//! next bind (`drm_gpuvm_bo_deferred_cleanup`): a window-sized buffer's
//! pages and IOMMU mappings, 1–4 ms each on Apple's GPUs. Mesa holds the
//! lock of its device's buffers through a bind, and finishing a frame
//! takes that lock (it gives back the frame's own buffers). So a check on
//! the renderer's device, which was this worker's way of taking the slow
//! import off the loop, held the renderer for as long as the kernel freed
//! the buffers let go of since the last bind: a menu's first opening held
//! app_server's frame 12–37 ms, an app's launch or resize 15–20 (found
//! 2026-11-02 with the kernel's syscall trace and kprobes: `futex` on the
//! loop while the check sat in `DRM_IOCTL_ASAHI_VM_BIND`, 25 ms of it
//! freeing twelve old buffers).
//!
//! Now the worker checks on a device of its own (the render node opened
//! again: Mesa makes a device, with its own locks, for each file), and
//! keeps what it checked till the renderer has let it go: the renderer's
//! bind then lets go of a buffer something else still holds, which is
//! nothing, and the worker frees it on its own device as no check waits.
//! The renderer imports each checked buffer itself, on the loop; with the
//! worker holding the buffer, that's a bind and nothing else. Clients may
//! draw into a buffer and show it before it is checked (Mesa's Vulkan
//! does); such a commit waits for the check (`Gate`), the window showing
//! its last frame meanwhile.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::dmabuf::{Dmabuf, WeakDmabuf},
        egl::{EGLDisplay, ffi::egl::types::EGLImage},
    },
    reexports::{
        calloop::channel,
        wayland_server::{Client, protocol::wl_surface::WlSurface},
    },
    wayland::{
        compositor::{Blocker, BlockerState},
        dmabuf::ImportNotifier,
    },
};

/// Longer than this, a check is logged.
const SLOW: Duration = Duration::from_millis(4);
/// How long after a buffer goes the renderer has let it go (its texture
/// dropped as a frame finished, the frames that drew it done on the GPU,
/// the kernel's unmapping of it waiting for the next bind).
const GRACE: Duration = Duration::from_millis(100);
/// And how many frames the renderer has finished since.
const GRACE_FRAMES: u64 = 3;
/// More than this many let go of, they're freed even while checks wait
/// (a window resized live).
const MOST_HELD: usize = 8;
/// How many are freed at a time (a bind each): a check coming in waits
/// for these and no more (1–4 ms a buffer on Apple's GPUs: a check behind
/// a dozen waited 45 ms, and a drag's round with it).
const FREE_AT_ONCE: usize = 2;
/// Buffers waiting for a bind in the renderer this long (nothing new
/// coming in), the loop is asked for one ([`Answer::Bind`]).
const BIND_WITHIN: Duration = Duration::from_millis(500);
/// After a client let go of a buffer ([`Checks::gone`]), how long and how
/// often the worker looks whether the buffer went (the surface showing it
/// may hold it a while).
const LOOK_FOR: Duration = Duration::from_secs(1);
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// A display for the checks alone, on `node` opened again, and a spare:
/// a small buffer made there and let go of at once (only its exported
/// file keeps it), which a device can take in whenever a bind is wanted
/// (the worker's, to free what it let go of; the renderer's, asked).
fn own_display(node: &std::path::Path) -> Option<(EGLDisplay, Option<Dmabuf>)> {
    use smithay::backend::allocator::{
        Fourcc,
        dmabuf::AsDmabuf as _,
        gbm::{GbmBuffer, GbmBufferFlags, GbmDevice},
    };
    let file = std::fs::OpenOptions::new().read(true).write(true).open(node).ok()?;
    let gbm = GbmDevice::new(file).ok()?;
    let spare = gbm.create_buffer_object::<()>(64, 64, Fourcc::Argb8888, GbmBufferFlags::RENDERING).ok().and_then(|bo| GbmBuffer::from_bo(bo, true).export().ok());
    // SAFETY: the gbm device outlives the display (moved into it).
    match unsafe { EGLDisplay::new(gbm) } {
        Ok(display) => Some((display, spare)),
        Err(e) => {
            tracing::warn!(?node, "buffer checks on the renderer's display: {e}");
            None
        }
    }
}

/// A bind of `spare` into `display`'s GPU address space, and its image
/// let go of: the kernel frees what that space let go of before.
fn bind(display: &EGLDisplay, spare: &Dmabuf) {
    drop(display.create_image_from_dmabuf(spare).ok().map(|image| Image { display: display.clone(), image }));
}

/// Holds back commits of a buffer until its check is done: a blocker
/// (`compositor::add_blocker`) for each, and the clients to tell
/// (`blocker_cleared`) when it opens; and the drag rounds
/// (`shell::rounds`) a commit of it is in, to tell they may show.
#[derive(Clone, Default)]
pub struct Gate(Arc<GateState>);

#[derive(Default)]
pub struct GateState {
    open: AtomicBool,
    clients: Mutex<Vec<Client>>,
    rounds: Mutex<Vec<(WlSurface, u64)>>,
}

impl Blocker for Gate {
    fn state(&self) -> BlockerState {
        if self.0.open.load(Ordering::Acquire) { BlockerState::Released } else { BlockerState::Pending }
    }
}

impl Gate {
    /// A commit of `client`'s waits for this.
    pub fn hold(&self, client: Client) {
        self.0.clients.lock().unwrap().push(client);
    }

    /// The commit of `surface` that waits is round `n`'s: it shows only
    /// once this opens (`AnvilState::gate_opened` tells it).
    pub fn in_round(&self, surface: WlSurface, n: u64) {
        self.0.rounds.lock().unwrap().push((surface, n));
    }

    /// Done: the clients whose commits were waiting.
    pub fn open(&self) -> Vec<Client> {
        self.0.open.store(true, Ordering::Release);
        std::mem::take(&mut *self.0.clients.lock().unwrap())
    }
}

impl<B: crate::state::Backend + 'static> crate::state::AnvilState<B> {
    /// A buffer's check is done: the commits waiting for it go on, and
    /// the round any of them is in may show.
    pub fn gate_opened(&mut self, gate: &Gate) {
        let dh = self.display_handle.clone();
        for client in gate.open() {
            smithay::wayland::compositor::CompositorHandler::client_compositor_state(self, &client).blocker_cleared(self, &dh);
        }
        let rounds = std::mem::take(&mut *gate.0.rounds.lock().unwrap());
        for (surface, n) in rounds {
            self.round_ready(&surface, n);
        }
    }
}

/// An EGL image of a buffer: while it lives, its device holds the
/// buffer's memory.
struct Image {
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

/// What the worker tells the loop.
pub enum Answer {
    /// Buffer `id` was checked: whether the driver took it.
    Checked { id: u64, ok: bool },
    /// Buffers wait to be freed, the renderer having let them go, but no
    /// buffer came in since: a bind in the renderer is wanted
    /// ([`Checks::bind_renderer`]).
    Bind,
}

/// What the worker is asked.
enum Job {
    /// Check a new buffer.
    Check(u64, Dmabuf),
    /// The renderer bound a buffer (it took one in): it has freed what it
    /// let go of before; and, asked ([`Checks::bind_renderer`]), since
    /// when it has let go of every buffer gone then.
    Bound(Option<Instant>),
    /// A client let go of a buffer.
    Gone,
    /// app_server is ending ([`Checks::stop`]).
    Quit,
}

/// A buffer the worker holds.
struct Kept {
    buffer: WeakDmabuf,
    _image: Image,
    /// When it went, and how many frames the renderer had finished then.
    gone: Option<(Instant, u64)>,
    /// The renderer has let it go: a bind there came after it was long
    /// enough gone and frames had finished since.
    released: bool,
}

/// The worker: its device, its spare, and the buffers it holds.
struct Worker {
    display: EGLDisplay,
    spare: Option<Dmabuf>,
    kept: Vec<Kept>,
    /// Since when buffers have waited for a bind in the renderer, and
    /// whether the loop has been asked for one.
    waiting: Option<(Instant, bool)>,
    /// Till when to look whether buffers went ([`LOOK_FOR`]).
    look_until: Option<Instant>,
}

impl Worker {
    fn check(&mut self, dmabuf: &Dmabuf) -> bool {
        let t = Instant::now();
        let image = self.display.create_image_from_dmabuf(dmabuf).ok().map(|image| Image { display: self.display.clone(), image });
        if t.elapsed() > SLOW {
            use smithay::backend::allocator::Buffer as _;
            tracing::info!(w = dmabuf.size().w, h = dmabuf.size().h, took_ms = t.elapsed().as_secs_f64() * 1000.0, "slow to check a buffer");
        }
        let ok = image.is_some();
        if let Some(image) = image {
            self.kept.push(Kept { buffer: dmabuf.weak(), _image: image, gone: None, released: false });
        }
        ok
    }

    /// Notes which buffers went and, after a bind in the renderer, which
    /// it has let go of: those gone long enough with frames finished since,
    /// or, the renderer settled first, gone before then.
    fn note(&mut self, frames: u64, bound: Option<Option<Instant>>) {
        let now = Instant::now();
        let mut waiting = false;
        for k in &mut self.kept {
            let was = k.gone;
            if k.gone.is_none() && k.buffer.upgrade().is_none() {
                k.gone = Some((now, frames));
            }
            if let Some((at, then)) = k.gone
                && !k.released
            {
                let settled = matches!((bound, was), (Some(Some(s)), Some((w, _))) if w < s);
                let let_go = settled || (now - at >= GRACE && frames >= then + GRACE_FRAMES);
                k.released = bound.is_some() && let_go;
                waiting |= !k.released;
            }
        }
        // (A bind in the renderer starts the wait for the next afresh.)
        self.waiting = match (waiting, self.waiting) {
            (false, _) => None,
            (true, Some(w)) if bound.is_none() => Some(w),
            (true, _) => Some((now, false)),
        };
    }

    fn released(&self) -> usize {
        self.kept.iter().filter(|k| k.released).count()
    }

    /// Frees what the renderer has let go of, `most` buffers of it: the
    /// images let go of, then a bind of the spare, so the kernel frees
    /// them now rather than in the next check.
    fn free(&mut self, most: usize) {
        let t = Instant::now();
        let before = self.kept.len();
        let mut n = 0;
        self.kept.retain(|k| {
            let go = k.released && n < most;
            n += go as usize;
            !go
        });
        if self.kept.len() == before {
            return;
        }
        if let Some(spare) = &self.spare {
            bind(&self.display, spare);
        }
        if t.elapsed() > SLOW {
            tracing::debug!(freed = before - self.kept.len(), took_ms = t.elapsed().as_secs_f64() * 1000.0, "buffers freed");
        }
    }

    /// How long to wait for the next job: till it's time to look again
    /// whether buffers went, or for buffers waiting for a bind in the
    /// renderer to ask for one; else for ever.
    fn wait(&mut self) -> Option<Duration> {
        let now = Instant::now();
        if self.look_until.is_some_and(|t| now >= t) {
            self.look_until = None;
        }
        let look = self.look_until.map(|_| LOOK_EVERY);
        let ask = match self.waiting {
            Some((since, false)) => Some(BIND_WITHIN.saturating_sub(since.elapsed())),
            _ => None,
        };
        look.into_iter().chain(ask).min()
    }

    /// Time to ask the loop for a bind in the renderer.
    fn ask(&mut self) -> bool {
        match &mut self.waiting {
            Some((since, asked @ false)) if since.elapsed() >= BIND_WITHIN => {
                *asked = true;
                true
            }
            _ => false,
        }
    }
}

/// Buffers being checked, and the worker checking them.
pub struct Checks {
    tx: mpsc::Sender<Job>,
    /// The worker's thread, waited for as app_server ends.
    worker: Option<std::thread::JoinHandle<()>>,
    pending: HashMap<u64, (Dmabuf, ImportNotifier, Gate)>,
    next: u64,
    /// Frames the renderer has finished ([`Checks::frame_done`]).
    frames: Arc<AtomicU64>,
    /// The renderer's display, and the worker's spare ([`Checks::bind_renderer`]).
    renderer: EGLDisplay,
    spare: Arc<Mutex<Option<Dmabuf>>>,
}

impl Checks {
    /// A worker answering through `done` (a channel in the event loop,
    /// which hands each answer to [`Checks::finish`]), on `node` opened
    /// again ([`own_display`]), or, if that can't be, on the renderer's
    /// `display` (as it was: holding its buffers then frees nothing
    /// elsewhere).
    pub fn new(display: EGLDisplay, node: Option<PathBuf>, done: channel::Sender<Answer>) -> Checks {
        let (tx, rx) = mpsc::channel::<Job>();
        let frames = Arc::new(AtomicU64::new(0));
        let spare = Arc::new(Mutex::new(None));
        let (seen, shared, renderer) = (frames.clone(), spare.clone(), display.clone());
        let worker = std::thread::Builder::new().name("dmabuf check".into()).spawn(move || {
            let (display, spare) = node.as_deref().and_then(own_display).unwrap_or((display, None));
            *shared.lock().unwrap() = spare.clone();
            let mut worker = Worker { display, spare, kept: Vec::new(), waiting: None, look_until: None };
            let mut next = None;
            loop {
                let job = match (next.take(), worker.wait()) {
                    (Some(job), _) => Ok(job),
                    (None, None) => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                    (None, Some(d)) => rx.recv_timeout(d),
                };
                let mut job = match job {
                    Ok(Job::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // app_server is ending: what it holds is left for
                        // the process's end to free (a destroy now would
                        // race EGL's own teardown at exit: the cores of
                        // 2026-11-04, `dri_destroy` on a freed heap and a
                        // dead mutex in NVIDIA's thread-exit destructor).
                        std::mem::forget(worker);
                        return;
                    }
                    Ok(job) => Some(job),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                };
                if job.is_none() {
                    // (Nothing came in: buffers that went noted, and, if
                    // they've waited long enough, the loop asked for a bind.)
                    worker.note(seen.load(Ordering::Relaxed), None);
                    if worker.ask() && worker.spare.is_some() && done.send(Answer::Bind).is_err() {
                        return;
                    }
                    continue;
                }
                while let Some(j) = job.take() {
                    match j {
                        Job::Check(id, dmabuf) => {
                            tracing::debug!(target: "olo::rounds", id, "checking a buffer");
                            let ok = worker.check(&dmabuf);
                            tracing::debug!(target: "olo::rounds", id, "checked a buffer");
                            if done.send(Answer::Checked { id, ok }).is_err() {
                                return;
                            }
                            worker.note(seen.load(Ordering::Relaxed), None);
                        }
                        Job::Bound(settled) => worker.note(seen.load(Ordering::Relaxed), Some(settled)),
                        Job::Gone => {
                            worker.look_until = Some(Instant::now() + LOOK_FOR);
                            worker.note(seen.load(Ordering::Relaxed), None);
                        }
                        Job::Quit => {
                            std::mem::forget(worker);
                            return;
                        }
                    }
                    // Checks first; the freeing once none waits (or too
                    // much waits to be freed).
                    job = rx.try_recv().ok();
                    if job.is_some() && worker.released() > MOST_HELD {
                        worker.free(FREE_AT_ONCE);
                    }
                }
                // (A few at a time, a check that comes in going first.)
                while worker.released() > 0 {
                    worker.free(FREE_AT_ONCE);
                    if let Ok(job) = rx.try_recv() {
                        next = Some(job);
                        break;
                    }
                }
            }
        });
        Checks { tx, worker: worker.ok(), pending: HashMap::new(), next: 0, frames, renderer, spare }
    }

    /// As app_server ends, before its renderer and EGL go: the worker
    /// told to quit, and waited for (its thread's end runs the driver's
    /// destructors, which need EGL still there), half a second at most,
    /// so a driver stuck in a check can't hold an upgrade's restart up.
    pub fn stop(&mut self) {
        let Some(worker) = self.worker.take() else { return };
        let asked = Instant::now();
        let _ = self.tx.send(Job::Quit);
        let until = Instant::now() + std::time::Duration::from_millis(500);
        while !worker.is_finished() && Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
        if worker.is_finished() {
            let _ = worker.join();
            tracing::info!(took_ms = asked.elapsed().as_secs_f64() * 1000.0, "the dmabuf check worker stopped");
        } else {
            tracing::warn!("the dmabuf check worker didn't stop in time; ending without it");
        }
    }

    /// Check `dmabuf` on the worker, telling `notifier` later; or, if the
    /// worker is gone, hand them back to be checked here.
    pub fn start(&mut self, dmabuf: Dmabuf, notifier: ImportNotifier) -> Option<(Dmabuf, ImportNotifier)> {
        let id = self.next;
        self.next += 1;
        if self.tx.send(Job::Check(id, dmabuf.clone())).is_err() {
            return Some((dmabuf, notifier));
        }
        self.pending.insert(id, (dmabuf, notifier, Gate::default()));
        None
    }

    /// The renderer imported a buffer (a bind in its GPU address space).
    pub fn bound(&self) {
        let _ = self.tx.send(Job::Bound(None));
    }

    /// A client let go of a buffer (it may go once its surface does).
    pub fn gone(&self) {
        let _ = self.tx.send(Job::Gone);
    }

    /// A bind in the renderer, asked for ([`Answer::Bind`]), after
    /// `settle` (the renderer's textures of buffers gone dropped, and the
    /// GPU waited for, so the driver lets go of them): cheap, what it lets
    /// go of held by the worker.
    pub fn bind_renderer(&self, settle: impl FnOnce()) {
        let Some(spare) = self.spare.lock().unwrap().clone() else { return };
        let at = Instant::now();
        settle();
        bind(&self.renderer, &spare);
        let _ = self.tx.send(Job::Bound(Some(at)));
    }

    /// The renderer finished a frame (for when it has let a buffer go).
    pub fn frame_done(&self) {
        self.frames.fetch_add(1, Ordering::Relaxed);
    }

    /// What a commit of `dmabuf` must wait for, while it is being checked.
    pub fn gate(&self, dmabuf: &Dmabuf) -> Option<Gate> {
        self.pending.values().find(|(d, _, _)| d == dmabuf).map(|(_, _, g)| g.clone())
    }

    /// The buffer checked `id` is, who to tell, and the gate to open once
    /// the renderer has it.
    pub fn finish(&mut self, id: u64) -> Option<(Dmabuf, ImportNotifier, Gate)> {
        self.pending.remove(&id)
    }
}

impl Drop for Checks {
    fn drop(&mut self) {
        self.stop();
    }
}
