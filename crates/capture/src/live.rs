//! Frames to show live, never copied (Agent Screens' Live tab): captured
//! into a few dmabufs in turn, each handed on as it comes to be shown by
//! the person's compositor (GPUI's `Window::set_external_buffer`), and
//! captured into again once that compositor lets it go. app_server draws
//! a frame on the GPU only when the screen changed, and nothing reads it
//! back: a screen at rest costs nothing, a busy one a copy on the GPU a
//! frame.

use std::{
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use wayland_client::protocol::wl_buffer;

use crate::{Capture, Source};

/// BGRx, DRM's `XR24`: what frames come as.
pub const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");

/// Buffers in turn: one shown, one on its way to be, one being drawn.
const BUFFERS: usize = 3;

/// Frames failing in a row before the capture is given up.
const FAILURES: u32 = 10;

/// Buffer ids, unique in the process: a viewer showing another screen in
/// the same place never takes one's buffer for another's.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// One frame: a dmabuf, as its planes (each a descriptor, the plane's
/// offset and its bytes a row).
pub struct LiveFrame {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<(Arc<OwnedFd>, u32, u32)>,
}

struct Buf {
    id: u64,
    width: u32,
    height: u32,
    modifier: u64,
    /// Kept for its memory (the planes' descriptors name it too).
    _bo: gbm::BufferObject<()>,
    wl: wl_buffer::WlBuffer,
    planes: Vec<(Arc<OwnedFd>, u32, u32)>,
    free: bool,
}

/// Wakes [`Live::next`]: buffers let go, or the end.
#[derive(Clone)]
pub struct Waker(Arc<Wake>);

struct Wake {
    fd: OwnedFd,
    released: Mutex<Vec<u64>>,
    stop: AtomicBool,
}

impl Waker {
    /// Buffer `id` is no longer shown: it can be drawn into again.
    pub fn released(&self, id: u64) {
        self.0.released.lock().unwrap_or_else(|e| e.into_inner()).push(id);
        self.poke();
    }

    /// [`Live::next`] returns None from now on.
    pub fn stop(&self) {
        self.0.stop.store(true, Ordering::Relaxed);
        self.poke();
    }

    fn poke(&self) {
        let one = 1u64.to_ne_bytes();
        // SAFETY: an eventfd of ours, written eight bytes.
        unsafe { libc::write(self.0.fd.as_raw_fd(), one.as_ptr().cast(), 8) };
    }
}

/// A capture of one of app_server's outputs or windows into dmabufs to
/// show.
pub struct Live {
    capture: Capture,
    /// The modifiers both app_servers take, the GPU's choice of them.
    modifiers: Vec<u64>,
    bufs: Vec<Buf>,
    /// The buffer the asked-for frame goes in.
    drawing: Option<usize>,
    wake: Waker,
    failures: u32,
    /// The connection's descriptor, to wait on.
    conn: OwnedFd,
}

impl Live {
    /// Capture `source` on the app_server of Wayland display `display`
    /// (None: `WAYLAND_DISPLAY`'s), with the pointer drawn in if `cursor`,
    /// to show on a compositor that takes BGRx dmabufs with the
    /// `shown` modifiers. An error if frames can't come as dmabufs it
    /// takes (then take pictures another way).
    pub fn start(display: Option<&str>, source: &Source, cursor: bool, shown: &[u64]) -> Result<Live, String> {
        let capture = Capture::on(display, source, cursor)?;
        let drawn = capture.dma_modifiers().ok_or("frames can't come as dmabufs here")?;
        let modifiers: Vec<u64> = drawn.into_iter().filter(|m| shown.contains(m)).collect();
        if modifiers.is_empty() {
            return Err("no dmabuf layout both app_servers take".into());
        }
        // SAFETY: eventfd(2); the descriptor is ours from here on.
        let fd = unsafe {
            let raw = libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK);
            if raw < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            OwnedFd::from_raw_fd(raw)
        };
        let wake = Waker(Arc::new(Wake { fd, released: Mutex::new(Vec::new()), stop: AtomicBool::new(false) }));
        let conn = capture.fd()?;
        Ok(Live { capture, modifiers, bufs: Vec::new(), drawing: None, wake, failures: 0, conn })
    }

    /// What tells [`Live::next`] a buffer is free again, or to end.
    pub fn waker(&self) -> Waker {
        self.wake.clone()
    }

    /// The next frame, once the screen has changed (and a buffer is free
    /// to draw it in); None once stopped. An error ends the capture (the
    /// screen went, or app_server did).
    pub fn next(&mut self) -> Result<Option<LiveFrame>, String> {
        loop {
            if self.wake.0.stop.load(Ordering::Relaxed) {
                return Ok(None);
            }
            for id in std::mem::take(&mut *self.wake.0.released.lock().unwrap_or_else(|e| e.into_inner())) {
                if let Some(b) = self.bufs.iter_mut().find(|b| b.id == id) {
                    b.free = true;
                }
            }
            if self.capture.stopped() {
                return Err("app_server ended the capture".into());
            }
            if self.drawing.is_none() {
                self.ask()?;
            }
            match self.capture.poll()? {
                Some(Ok(())) => {
                    self.failures = 0;
                    if let Some(i) = self.drawing.take() {
                        let b = &self.bufs[i];
                        return Ok(Some(LiveFrame { id: b.id, width: b.width, height: b.height, fourcc: XRGB8888, modifier: b.modifier, planes: b.planes.clone() }));
                    }
                }
                Some(Err(e)) => {
                    // (Its size changed, most likely: the next is asked
                    // for at the new one.)
                    if let Some(i) = self.drawing.take() {
                        self.bufs[i].free = true;
                    }
                    self.failures += 1;
                    if self.failures >= FAILURES {
                        return Err(e);
                    }
                    continue;
                }
                None => {}
            }
            self.wait()?;
        }
    }

    /// Ask for the next frame into a free buffer, if there is one (made
    /// anew at a new size).
    fn ask(&mut self) -> Result<(), String> {
        let (w, h) = self.capture.size().ok_or("app_server gave no size")?;
        if self.bufs.first().is_some_and(|b| (b.width, b.height) != (w, h)) {
            // (Ones still shown stay alive with the compositor that shows
            // them; their releases name ids gone.)
            for b in self.bufs.drain(..) {
                b.wl.destroy();
            }
        }
        while self.bufs.len() < BUFFERS {
            let bo = self.capture.allocate(w, h, &self.modifiers)?;
            let wl = self.capture.import(&bo)?;
            let mut planes = Vec::new();
            for plane in 0..bo.plane_count() as i32 {
                let fd = bo.fd_for_plane(plane).map_err(|e| e.to_string())?;
                planes.push((Arc::new(fd), bo.offset(plane), bo.stride_for_plane(plane)));
            }
            let modifier = u64::from(bo.modifier());
            self.bufs.push(Buf { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), width: w, height: h, modifier, _bo: bo, wl, planes, free: true });
        }
        if let Some(i) = self.bufs.iter().position(|b| b.free) {
            self.capture.request_into(&self.bufs[i].wl)?;
            self.bufs[i].free = false;
            self.drawing = Some(i);
        }
        Ok(())
    }

    /// Until app_server says something, or the waker pokes.
    fn wait(&mut self) -> Result<(), String> {
        let mut fds = [libc::pollfd { fd: self.conn.as_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: self.wake.0.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: two valid pollfds.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e.to_string());
            }
        }
        if fds[1].revents != 0 {
            let mut count = [0u8; 8];
            // SAFETY: reading the eventfd's count into eight bytes.
            unsafe { libc::read(self.wake.0.fd.as_raw_fd(), count.as_mut_ptr().cast(), 8) };
        }
        Ok(())
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        for b in self.bufs.drain(..) {
            b.wl.destroy();
        }
    }
}
