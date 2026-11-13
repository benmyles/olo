//! A client of app_server's screen capture, ext-image-copy-capture (on
//! its direct socket, where outside programs never see it): the portal's
//! screen sharing, and Agent Screens' live view (`live`). A `Capture` is
//! a session on one output or window: it learns the buffer size and
//! formats app_server takes and keeps a buffer of that size, a dmabuf on
//! the GPU app_server names where it can (app_server draws into it on the
//! GPU, reading nothing back) else shared memory, and gives frames as
//! BGRx rows (what WebRTC wants first), into buffers of the caller's, or
//! lent out of a few of its own as they are, in the GPU's memory
//! ([`Capture::lend_frames`]: a recording's encoder on the same GPU).

pub mod live;

use std::{
    os::fd::{AsFd, AsRawFd as _, BorrowedFd, FromRawFd, OwnedFd},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool},
};
use wayland_protocols::{
    ext::{
        foreign_toplevel_list::v1::client::{ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1},
        image_capture_source::v1::client::{ext_foreign_toplevel_image_capture_source_manager_v1, ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1},
        image_copy_capture::v1::client::{
            ext_image_copy_capture_cursor_session_v1 as cursor_v1, ext_image_copy_capture_frame_v1 as frame_v1, ext_image_copy_capture_manager_v1, ext_image_copy_capture_session_v1 as session_v1,
        },
    },
    wp::linux_dmabuf::zv1::client::{zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1},
};

/// Olo's own Wayland protocol (protocols/): a workspace as a capture
/// source, and a source fitted to a size.
#[allow(non_upper_case_globals, non_camel_case_types, unused_imports, dead_code, missing_docs, clippy::all)]
pub mod protocol {
    use wayland_client::{self, protocol::*};
    use wayland_protocols::ext::image_capture_source::v1::client::*;
    pub mod __interfaces {
        use wayland_client::{backend as wayland_backend, protocol::__interfaces::*};
        use wayland_protocols::ext::image_capture_source::v1::client::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/olo-image-capture-source-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/olo-image-capture-source-v1.xml");
}

use protocol::olo_image_capture_source_manager_v1::OloImageCaptureSourceManagerV1;

/// One frame: its size and its rows (BGRx or BGRA, as app_server gave).
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: wl_shm::Format,
    pub data: Vec<u8>,
}

/// A window in the foreign-toplevel list, as its events say.
struct Toplevel {
    handle: ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    identifier: String,
    app_id: String,
    title: String,
}

#[derive(Default)]
struct State {
    outputs: Vec<(wl_output::WlOutput, String)>,
    toplevels: Vec<Toplevel>,
    size: Option<(u32, u32)>,
    formats: Vec<wl_shm::Format>,
    /// The GPU app_server draws dmabufs on, and the formats (with their
    /// modifiers) it draws.
    dma_device: Option<libc::dev_t>,
    dma_formats: Vec<(u32, Vec<u64>)>,
    constraints_done: bool,
    stopped: bool,
    frame: Option<Result<(), String>>,
    cursor: CursorState,
    /// The session whose events count (another's, one just replaced by
    /// `Capture::fit`, are let go).
    session: Option<wayland_client::backend::ObjectId>,
}

/// The cursor session's (`Capture::follow_cursor`): where the pointer is
/// on the screen, its hotspot, and its picture's session.
#[derive(Default)]
struct CursorState {
    /// Where the hotspot is, in the screen's pixels (None: off it).
    position: Option<(i32, i32)>,
    hotspot: (i32, i32),
    /// The picture's size and formats, as its session says.
    size: Option<(u32, u32)>,
    formats: Vec<wl_shm::Format>,
    constraints_done: bool,
    frame: Option<Result<(), String>>,
    /// The place or picture changed since `Capture::cursor` was last read.
    changed: bool,
}

/// Marks what belongs to the cursor's picture (its session and frames),
/// told apart from the screen's.
struct OfCursor;

/// Where the pointer is on a screen and what it looks like, apart from
/// the screen's picture (`Capture::cursor`).
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    /// Where its hotspot is, in the screen's pixels; None: off it, or
    /// hidden.
    pub position: Option<(i32, i32)>,
    /// The hotspot's place in the picture.
    pub hotspot: (i32, i32),
    /// The picture (BGRA rows, `size.0 * 4` bytes each) and its size,
    /// once one has come; `serial` grows with each new one.
    pub size: (u32, u32),
    pub serial: u64,
}

/// A cursor session and its picture.
struct Following {
    // (Kept: dropping it would end the session.)
    _session: cursor_v1::ExtImageCopyCaptureCursorSessionV1,
    pictures: session_v1::ExtImageCopyCaptureSessionV1,
    /// The buffer its pictures come into (a memfd), and its size.
    buffer: Option<(wl_buffer::WlBuffer, memmap2::MmapMut, (u32, u32))>,
    frame: Option<frame_v1::ExtImageCopyCaptureFrameV1>,
    /// The last picture that came, and how many have.
    image: Vec<u8>,
    image_size: (u32, u32),
    serial: u64,
    _pointer: wl_pointer::WlPointer,
}

/// Buffers frames are lent out of, in turn: one drawn into, one lent
/// (or waiting to be), one free.
const LENDING: usize = 3;

/// Lent frames' keys, unique in the process.
static NEXT_LENT: AtomicU64 = AtomicU64::new(1);

/// A frame lent out of the capture's own dmabufs ([`Capture::lend`]):
/// one plane of BGRx (DRM's XRGB8888) in the GPU's memory, laid out as
/// `modifier` says. Its buffer isn't drawn into again till it's dropped.
pub struct Lent {
    /// The same for every frame lent out of this buffer.
    pub key: u64,
    pub width: u32,
    pub height: u32,
    pub modifier: u64,
    pub offset: u32,
    pub pitch: u32,
    /// The buffer's size, in bytes.
    pub size: u32,
    fd: Arc<OwnedFd>,
    lent: Arc<AtomicUsize>,
}

impl Lent {
    /// The buffer's dmabuf.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        self.lent.fetch_sub(1, Ordering::Release);
    }
}

/// One of the buffers frames are lent out of.
struct Lending {
    key: u64,
    width: u32,
    height: u32,
    bo: gbm::BufferObject<()>,
    wl: wl_buffer::WlBuffer,
    fd: Arc<OwnedFd>,
    size: u32,
    /// Frames of it lent out now.
    lent: Arc<AtomicUsize>,
}

/// The buffer frames are captured into.
enum Target {
    Shm(memmap2::MmapMut),
    Dma(gbm::BufferObject<()>),
}

/// A capture session on one of app_server's outputs.
pub struct Capture {
    queue: EventQueue<State>,
    state: State,
    shm: wl_shm::WlShm,
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    /// The GPU's allocator, once opened (None: shared memory only).
    gbm: Option<gbm::Device<std::fs::File>>,
    conn: Connection,
    /// The output's name (its connector).
    name: String,
    session: session_v1::ExtImageCopyCaptureSessionV1,
    buffer: Option<(wl_buffer::WlBuffer, Target, (u32, u32, u32, wl_shm::Format))>,
    /// The buffer is new: what it holds is unknown to app_server.
    fresh: bool,
    /// The buffer holds a whole frame (not while a new one, allocated for
    /// another size, waits for its first).
    filled: bool,
    /// The frame asked for, until it comes.
    frame: Option<frame_v1::ExtImageCopyCaptureFrameV1>,
    /// It was asked for into `buffer` (not one of the stream's).
    into_own: bool,
    /// Frames lent out (`lend_frames`): the layouts they may come in, the
    /// buffers, the one the frame asked for goes into, and the one the
    /// last came into.
    lend_layouts: fn(u64) -> bool,
    lending: Option<Vec<Lending>>,
    lend_drawing: Option<usize>,
    lend_last: Option<usize>,
    /// What it captures, kept for a cursor session on it.
    source: ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    copier: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
    /// Whether the pointer is drawn in.
    paint_cursors: bool,
    /// Olo's own sources (None: an app_server without them), and the
    /// fitted source frames come from (`fit`).
    olo_sources: Option<OloImageCaptureSourceManagerV1>,
    fitted: Option<ext_image_capture_source_v1::ExtImageCaptureSourceV1>,
    seat: Option<wl_seat::WlSeat>,
    /// The pointer followed apart (`follow_cursor`).
    following: Option<Following>,
}

/// What to capture.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// The output with this name (the first one if None).
    Output(Option<String>),
    /// A window, by its id in Olo (its handle's identifier in the
    /// foreign-toplevel list is `olo-ID`); if it is gone, one with the
    /// same app id and title (after an app_server reload, as windows are
    /// put back with new ids).
    Window { id: u64, app_id: String, title: String },
    /// Another display's app_server's first screen, by its Wayland
    /// display (an agent screen's `wayland-9`): shared into a call from
    /// the person's display.
    Display(String),
    /// Workspace `index` (from 0) of the output with this name (the
    /// first if None): what it shows while it shows there, and that
    /// workspace's windows as they are while it shows another.
    Workspace { output: Option<String>, index: u32 },
}

impl Capture {
    /// Capture `source` on the compositor at `WAYLAND_DISPLAY`, with the
    /// pointer drawn in if `cursor`.
    pub fn new(source: &Source, cursor: bool) -> Result<Capture, String> {
        let display = std::env::var("WAYLAND_DISPLAY").ok();
        Self::on(display.as_deref(), source, cursor)
    }

    /// The same on the app_server of Wayland display `display` (an Agent
    /// Screen's `wayland-9`; None: `WAYLAND_DISPLAY`'s).
    pub fn on(display: Option<&str>, source: &Source, cursor: bool) -> Result<Capture, String> {
        if let Source::Display(other) = source {
            return Self::on(Some(other), &Source::Output(None), cursor);
        }
        let conn = connect(display).map_err(|e| format!("can't reach app_server: {e}"))?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let mut state = State::default();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(|_| "no wl_shm".to_owned())?;
        let dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1> = globals.bind(&qh, 3..=3, ()).ok();
        let seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
        let copier: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "app_server offers no ext-image-copy-capture".to_owned())?;
        let olo_sources: Option<OloImageCaptureSourceManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
        // The output named (the first if None), its names come with its
        // first events.
        let output_named = |state: &mut State, queue: &mut EventQueue<State>, name: &Option<String>| -> Result<(wl_output::WlOutput, String), String> {
            for g in globals.contents().clone_list().iter().filter(|g| g.interface == "wl_output") {
                let o: wl_output::WlOutput = globals.registry().bind(g.name, g.version.min(4), &qh, ());
                state.outputs.push((o, String::new()));
            }
            queue.roundtrip(state).map_err(|e| e.to_string())?;
            match name {
                Some(n) => state.outputs.iter().find(|(_, o)| o == n).cloned().ok_or_else(|| format!("no output {n}")),
                None => state.outputs.first().cloned().ok_or_else(|| "no outputs".to_owned()),
            }
        };
        let (image_source, name) = match source {
            Source::Output(name) => {
                let sources: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 =
                    globals.bind(&qh, 1..=1, ()).map_err(|_| "app_server offers no output capture (ext-image-capture-source)".to_owned())?;
                let (output, name) = output_named(&mut state, &mut queue, name)?;
                (sources.create_source(&output, &qh, ()), name)
            }
            Source::Workspace { output, index } => {
                let sources = olo_sources.as_ref().ok_or("app_server offers no workspace capture (olo-image-capture-source)")?;
                let (output, name) = output_named(&mut state, &mut queue, output)?;
                (sources.create_workspace_source(&output, *index, &qh, ()), name)
            }
            Source::Display(_) => unreachable!("taken above"),
            Source::Window { id, app_id, title } => {
                let list: ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "app_server lists no windows (ext-foreign-toplevel-list)".to_owned())?;
                let sources: ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1 =
                    globals.bind(&qh, 1..=1, ()).map_err(|_| "app_server offers no window capture".to_owned())?;
                // The windows come at once, each finished by `done`.
                queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
                queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
                let wanted = format!("olo-{id}");
                let found = state
                    .toplevels
                    .iter()
                    .find(|t| t.identifier == wanted)
                    .or_else(|| state.toplevels.iter().find(|t| t.app_id == *app_id && t.title == *title))
                    .ok_or_else(|| format!("no window {id}"))?;
                let made = sources.create_source(&found.handle, &qh, ());
                let name = found.identifier.clone();
                list.stop();
                (made, name)
            }
        };
        let session = copier.create_session(&image_source, options(cursor), &qh, ());
        state.session = Some(wayland_client::Proxy::id(&session));
        while !state.constraints_done && !state.stopped {
            queue.blocking_dispatch(&mut state).map_err(|e| e.to_string())?;
        }
        if state.stopped {
            return Err("app_server stopped the capture".into());
        }
        let gbm = if std::env::var_os("OLO_PORTAL_SHM").is_some() || dmabuf.is_none() {
            None
        } else {
            state.dma_device.and_then(|dev| match open_render_node(dev).and_then(|f| gbm::Device::new(f).map_err(|e| e.to_string())) {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::warn!("capture: no dmabufs ({e}), capturing into shared memory");
                    None
                }
            })
        };
        Ok(Capture {
            queue,
            state,
            shm,
            dmabuf,
            gbm,
            conn,
            name,
            session,
            buffer: None,
            fresh: true,
            filled: false,
            frame: None,
            into_own: false,
            lend_layouts: |_| true,
            lending: None,
            lend_drawing: None,
            lend_last: None,
            source: image_source,
            copier,
            paint_cursors: cursor,
            olo_sources,
            fitted: None,
            seat,
            following: None,
        })
    }

    /// Frames made smaller by app_server as it draws them, to fit at most
    /// `width` × `height` (its shape kept, each side even; never larger:
    /// Olo's fitted source), in place of the source's as they are: what a
    /// video sender would otherwise shrink itself, frame by frame. False,
    /// nothing changed, from an app_server that can't. Before frames are
    /// asked for or lent.
    pub fn fit(&mut self, width: u32, height: u32) -> Result<bool, String> {
        let Some(sources) = self.olo_sources.as_ref() else { return Ok(false) };
        if self.frame.is_some() || self.lending.is_some() {
            return Err("fit before frames are asked for".into());
        }
        let qh = self.queue.handle();
        let fitted = sources.create_fitted_source(&self.source, width, height, &qh, ());
        let session = self.copier.create_session(&fitted, options(self.paint_cursors), &qh, ());
        self.session.destroy();
        self.session = session;
        self.state.session = Some(wayland_client::Proxy::id(&self.session));
        self.state.constraints_done = false;
        while !self.state.constraints_done && !self.state.stopped {
            self.queue.blocking_dispatch(&mut self.state).map_err(|e| e.to_string())?;
        }
        if let Some(old) = self.fitted.replace(fitted) {
            old.destroy();
        }
        // (A buffer of the new size for the first frame.)
        if let Some((old, _, _)) = self.buffer.take() {
            old.destroy();
        }
        (self.fresh, self.filled) = (true, false);
        Ok(!self.state.stopped)
    }

    /// Follow the pointer apart from the picture (a cursor session: the
    /// portal's cursor sent as metadata): where it is (`cursor`) and its
    /// picture (`cursor_image`), each kept up to date as `poll` reads.
    /// Ask for the picture without the pointer drawn in (`cursor` false).
    pub fn follow_cursor(&mut self) -> Result<(), String> {
        if self.following.is_some() {
            return Ok(());
        }
        let qh = self.queue.handle();
        let seat = self.seat.as_ref().ok_or("app_server offers no seat")?;
        let pointer = seat.get_pointer(&qh, ());
        let session = self.copier.create_pointer_cursor_session(&self.source, &pointer, &qh, ());
        let pictures = session.get_capture_session(&qh, OfCursor);
        while !self.state.cursor.constraints_done && !self.state.stopped {
            self.queue.blocking_dispatch(&mut self.state).map_err(|e| e.to_string())?;
        }
        self.following = Some(Following { _session: session, pictures, buffer: None, frame: None, image: Vec::new(), image_size: (0, 0), serial: 0, _pointer: pointer });
        self.request_cursor()
    }

    /// Where the pointer is and its picture's serial, if it's followed
    /// (`follow_cursor`), and whether either changed since last asked.
    pub fn cursor(&mut self) -> Option<(Cursor, bool)> {
        let f = self.following.as_ref()?;
        let c = &mut self.state.cursor;
        let changed = std::mem::take(&mut c.changed);
        Some((Cursor { position: c.position, hotspot: c.hotspot, size: f.image_size, serial: f.serial }, changed))
    }

    /// Whether where the pointer is, or its picture, changed since
    /// `cursor` was last asked (without asking).
    pub fn cursor_changed(&self) -> bool {
        self.following.is_some() && self.state.cursor.changed
    }

    /// The pointer's last picture (BGRA rows), if one has come.
    pub fn cursor_image(&self) -> Option<&[u8]> {
        self.following.as_ref().filter(|f| f.serial > 0).map(|f| f.image.as_slice())
    }

    /// Ask for the cursor's next picture: at once the first time, then
    /// when it changes.
    fn request_cursor(&mut self) -> Result<(), String> {
        let Some((w, h)) = self.state.cursor.size else { return Ok(()) };
        let format = [wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888].into_iter().find(|f| self.state.cursor.formats.contains(f)).ok_or("no BGRA format for the cursor")?;
        let fresh = self.following.as_ref().is_some_and(|f| f.buffer.as_ref().is_none_or(|b| b.2 != (w, h)));
        if fresh {
            let (buffer, map) = shm(&self.shm, &self.queue.handle(), w, h, format)?;
            let f = self.following.as_mut().unwrap();
            if let Some((old, _, _)) = f.buffer.replace((buffer, map, (w, h))) {
                old.destroy();
            }
        }
        let qh = self.queue.handle();
        let Some(f) = self.following.as_mut() else { return Ok(()) };
        let (buffer, _, _) = f.buffer.as_ref().unwrap();
        self.state.cursor.frame = None;
        let frame = f.pictures.create_frame(&qh, OfCursor);
        frame.attach_buffer(buffer);
        frame.damage_buffer(0, 0, w as i32, h as i32);
        frame.capture();
        f.frame = Some(frame);
        self.queue.flush().map_err(|e| e.to_string())
    }

    /// The cursor's picture that came (or failed), and the next asked for.
    fn cursor_came(&mut self) -> Result<(), String> {
        let Some(done) = self.state.cursor.frame.take() else { return Ok(()) };
        let Some(f) = self.following.as_mut() else { return Ok(()) };
        if let Some(frame) = f.frame.take() {
            frame.destroy();
        }
        if done.is_ok()
            && let Some((_, map, size)) = f.buffer.as_ref()
        {
            f.image.clear();
            f.image.extend_from_slice(&map[..(size.0 * size.1 * 4) as usize]);
            f.image_size = *size;
            f.serial += 1;
            self.state.cursor.changed = true;
        }
        self.request_cursor()
    }

    /// Frames into shared memory, never dmabufs (asked before the first
    /// frame): for a picture read on the CPU in a process that has a GPU
    /// context of its own (the share picker's thumbnails, in olo-portal's
    /// window), where a dmabuf mapped for reading came back black.
    pub fn in_memory(mut self) -> Capture {
        self.gbm = None;
        self
    }

    /// Whether frames come through the GPU (dmabufs).
    pub fn on_gpu(&self) -> bool {
        matches!(self.buffer, Some((_, Target::Dma(_), _))) || self.lend_last.is_some()
    }

    /// The GPU frames come in, as its render node's device number, where
    /// they can come as dmabufs.
    pub fn render_device(&self) -> Option<u64> {
        self.gbm.as_ref()?;
        self.state.dma_device.map(|d| d as u64)
    }

    /// From the next `request`, frames come into a few dmabufs of the
    /// capture's own in turn, laid out as `layouts` takes (by modifier),
    /// each lent out as it comes (`lend`) and not drawn into again till
    /// it's given back (dropped); when none is free, into its own buffer
    /// as before. None: as before.
    pub fn lend_frames(&mut self, layouts: Option<fn(u64) -> bool>) {
        let Some(layouts) = layouts else {
            for b in self.lending.take().into_iter().flatten() {
                b.wl.destroy();
            }
            (self.lend_drawing, self.lend_last) = (None, None);
            return;
        };
        if self.lending.is_none() && self.gbm.is_some() {
            self.lend_layouts = layouts;
            self.lending = Some(Vec::new());
        }
    }

    /// The last frame, lent out, if it came into a buffer it can be lent
    /// out of (`lend_frames`).
    pub fn lend(&self) -> Option<Lent> {
        let b = &self.lending.as_ref()?[self.lend_last?];
        b.lent.fetch_add(1, Ordering::Acquire);
        let modifier = u64::from(b.bo.modifier());
        Some(Lent { key: b.key, width: b.width, height: b.height, modifier, offset: b.bo.offset(0), pitch: b.bo.stride_for_plane(0), size: b.size, fd: b.fd.clone(), lent: b.lent.clone() })
    }

    /// Ask for the next frame into a free buffer to lend out of (made
    /// anew at a new size): false if none is free.
    fn request_lending(&mut self, w: u32, h: u32) -> Result<bool, String> {
        let mut bufs = self.lending.take().unwrap_or_default();
        if bufs.first().is_some_and(|b| (b.width, b.height) != (w, h)) {
            // (Ones lent out keep their memory: the lent frames hold its
            // descriptor.)
            for b in bufs.drain(..) {
                b.wl.destroy();
            }
            (self.lend_drawing, self.lend_last) = (None, None);
        }
        let made = (|| {
            while bufs.len() < LENDING {
                let (wl, bo, format) = self.dma_buffer(w, h, self.lend_layouts)?;
                if format != wl_shm::Format::Xrgb8888 || bo.plane_count() != 1 {
                    wl.destroy();
                    return Err("frames to lend come as BGRx in one plane".to_owned());
                }
                let fd = bo.fd_for_plane(0).map_err(|e| e.to_string())?;
                // SAFETY: lseek on a dmabuf of ours: its size.
                let size = unsafe { libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_END) };
                if size <= 0 {
                    wl.destroy();
                    return Err("a dmabuf without a size".to_owned());
                }
                bufs.push(Lending { key: NEXT_LENT.fetch_add(1, Ordering::Relaxed), width: w, height: h, bo, wl, fd: Arc::new(fd), size: size as u32, lent: Arc::default() });
            }
            Ok(())
        })();
        let free = (0..bufs.len()).find(|&i| Some(i) != self.lend_last && bufs[i].lent.load(Ordering::Acquire) == 0);
        let wl = free.map(|i| bufs[i].wl.clone());
        self.lending = Some(bufs);
        made?;
        let (Some(i), Some(wl)) = (free, wl) else { return Ok(false) };
        self.request_into(&wl)?;
        self.lend_drawing = Some(i);
        Ok(true)
    }

    /// The modifiers app_server draws BGRx dmabufs with, where frames can
    /// come as dmabufs (None: shared memory only).
    pub fn dma_modifiers(&self) -> Option<Vec<u64>> {
        self.gbm.as_ref()?;
        self.dmabuf.as_ref()?;
        self.state.dma_formats.iter().find(|(c, _)| *c == gbm::Format::Xrgb8888 as u32).map(|(_, m)| m.clone()).filter(|m| !m.is_empty())
    }

    /// A BGRx dmabuf of `w`×`h` for a stream's buffer, with the GPU's
    /// choice of `modifiers`.
    pub fn allocate(&self, w: u32, h: u32, modifiers: &[u64]) -> Result<gbm::BufferObject<()>, String> {
        let gbm = self.gbm.as_ref().ok_or("no GPU")?;
        gbm.create_buffer_object_with_modifiers2::<()>(w, h, gbm::Format::Xrgb8888, modifiers.iter().map(|&m| gbm::Modifier::from(m)), gbm::BufferObjectFlags::RENDERING)
            .map_err(|e| format!("can't allocate a {w}×{h} dmabuf: {e}"))
    }

    /// `bo` as a buffer app_server can capture into (on this connection:
    /// again after a reconnection).
    pub fn import(&self, bo: &gbm::BufferObject<()>) -> Result<wl_buffer::WlBuffer, String> {
        let dmabuf = self.dmabuf.as_ref().ok_or("no linux-dmabuf")?;
        let qh = self.queue.handle();
        let params = dmabuf.create_params(&qh, ());
        let modifier = u64::from(bo.modifier());
        for plane in 0..bo.plane_count() as i32 {
            let fd = bo.fd_for_plane(plane).map_err(|e| e.to_string())?;
            params.add(fd.as_fd(), plane as u32, bo.offset(plane), bo.stride_for_plane(plane), (modifier >> 32) as u32, modifier as u32);
        }
        let buffer = params.create_immed(bo.width() as i32, bo.height() as i32, bo.format() as u32, zwp_linux_buffer_params_v1::Flags::empty(), &qh, ());
        params.destroy();
        Ok(buffer)
    }

    /// Ask for the next frame into `buffer` (a stream's, which holds an
    /// older frame, or none: drawn whole). `poll` says when it has come.
    pub fn request_into(&mut self, buffer: &wl_buffer::WlBuffer) -> Result<(), String> {
        let (w, h) = self.state.size.ok_or("app_server gave no size")?;
        let qh = self.queue.handle();
        self.state.frame = None;
        let frame = self.session.create_frame(&qh, ());
        frame.attach_buffer(buffer);
        frame.damage_buffer(0, 0, w as i32, h as i32);
        frame.capture();
        self.frame = Some(frame);
        self.into_own = false;
        self.queue.flush().map_err(|e| e.to_string())
    }

    /// A dmabuf for frames of `w`×`h`, in BGRx (else BGRA), as app_server
    /// draws them.
    fn dma_buffer(&self, w: u32, h: u32, layouts: fn(u64) -> bool) -> Result<(wl_buffer::WlBuffer, gbm::BufferObject<()>, wl_shm::Format), String> {
        let (gbm, dmabuf) = (self.gbm.as_ref().ok_or("no GPU")?, self.dmabuf.as_ref().ok_or("no linux-dmabuf")?);
        let pick = [(gbm::Format::Xrgb8888, wl_shm::Format::Xrgb8888), (gbm::Format::Argb8888, wl_shm::Format::Argb8888)];
        let (fourcc, shm_format, modifiers) =
            pick.into_iter().find_map(|(f, s)| self.state.dma_formats.iter().find(|(c, _)| *c == f as u32).map(|(_, m)| (f, s, m.clone()))).ok_or("app_server draws no BGRx or BGRA dmabufs")?;
        // Linear when asked (`OLO_PORTAL_LINEAR`, to compare), else what the
        // GPU likes best of what app_server draws.
        let linear = u64::from(gbm::Modifier::Linear);
        let modifiers: Vec<u64> = if std::env::var_os("OLO_PORTAL_LINEAR").is_some() { modifiers.into_iter().filter(|m| *m == linear).collect() } else { modifiers };
        let modifiers: Vec<u64> = modifiers.into_iter().filter(|&m| layouts(m)).collect();
        if modifiers.is_empty() {
            return Err("app_server draws dmabufs in no layout wanted".into());
        }
        let bo = gbm
            .create_buffer_object_with_modifiers2::<()>(w, h, fourcc, modifiers.iter().map(|&m| gbm::Modifier::from(m)), gbm::BufferObjectFlags::RENDERING)
            .map_err(|e| format!("can't allocate a {w}×{h} dmabuf: {e}"))?;
        let qh = self.queue.handle();
        let params = dmabuf.create_params(&qh, ());
        let modifier = u64::from(bo.modifier());
        for plane in 0..bo.plane_count() as i32 {
            let fd = bo.fd_for_plane(plane).map_err(|e| e.to_string())?;
            params.add(fd.as_fd(), plane as u32, bo.offset(plane), bo.stride_for_plane(plane), (modifier >> 32) as u32, modifier as u32);
        }
        let buffer = params.create_immed(w as i32, h as i32, fourcc as u32, zwp_linux_buffer_params_v1::Flags::empty(), &qh, ());
        params.destroy();
        Ok((buffer, bo, shm_format))
    }

    /// A shared-memory buffer for frames of `w`×`h`.
    fn shm_buffer(&self, w: u32, h: u32) -> Result<(wl_buffer::WlBuffer, memmap2::MmapMut, wl_shm::Format), String> {
        let format = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888].into_iter().find(|f| self.state.formats.contains(f)).ok_or("no BGRx/BGRA format offered")?;
        let stride = w * 4;
        let len = (stride * h) as usize;
        // SAFETY: a fresh memfd, owned from here on.
        let fd = unsafe {
            let raw = libc::memfd_create(c"olo-portal-frame".as_ptr(), libc::MFD_CLOEXEC);
            if raw < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            OwnedFd::from_raw_fd(raw)
        };
        let file = std::fs::File::from(fd);
        file.set_len(len as u64).map_err(|e| e.to_string())?;
        // SAFETY: the file is ours and stays this size.
        let map = unsafe { memmap2::MmapMut::map_mut(&file) }.map_err(|e| e.to_string())?;
        let qh = self.queue.handle();
        let pool = self.shm.create_pool(file.as_fd(), len as i32, &qh, ());
        let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, &qh, ());
        pool.destroy();
        Ok((buffer, map, format))
    }

    /// The output's name (its connector).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The size frames are, as app_server last said.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.state.size
    }

    /// The connection's descriptor, readable when app_server has said
    /// something (`poll`).
    pub fn fd(&self) -> Result<OwnedFd, String> {
        self.conn.backend().poll_fd().try_clone_to_owned().map_err(|e| e.to_string())
    }

    /// Ask for the next frame: at once the first time, then once the
    /// screen has changed. `poll` says when it has come.
    pub fn request(&mut self) -> Result<(), String> {
        let (w, h) = self.state.size.ok_or("app_server gave no size")?;
        if self.lending.is_some() {
            match self.request_lending(w, h) {
                Ok(true) => return Ok(()),
                // (None free: into its own.)
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!("capture: {e}; frames copied, not lent");
                    self.lend_frames(None);
                }
            }
        }
        if self.buffer.as_ref().is_none_or(|b| (b.2.0, b.2.1) != (w, h)) {
            if let Some((old, _, _)) = self.buffer.take() {
                old.destroy();
            }
            let dma = if self.gbm.is_some() {
                match self.dma_buffer(w, h, |_| true) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        tracing::warn!("capture: {e}; capturing into shared memory");
                        self.gbm = None;
                        None
                    }
                }
            } else {
                None
            };
            self.buffer = Some(match dma {
                Some((buffer, bo, format)) => {
                    let stride = bo.stride();
                    (buffer, Target::Dma(bo), (w, h, stride, format))
                }
                None => {
                    let (buffer, map, format) = self.shm_buffer(w, h)?;
                    (buffer, Target::Shm(map), (w, h, w * 4, format))
                }
            });
            self.fresh = true;
            self.filled = false;
        }
        let qh = self.queue.handle();
        let (buffer, _, _) = self.buffer.as_ref().unwrap();
        self.state.frame = None;
        let frame = self.session.create_frame(&qh, ());
        frame.attach_buffer(buffer);
        // What it holds is only unknown the first time.
        if std::mem::take(&mut self.fresh) {
            frame.damage_buffer(0, 0, w as i32, h as i32);
        }
        frame.capture();
        self.frame = Some(frame);
        self.into_own = true;
        self.queue.flush().map_err(|e| e.to_string())
    }

    /// Read what app_server said, without waiting: Some once the frame
    /// asked for has come (or failed: then the size may have changed, and
    /// the next request allocates again). An error is the connection's
    /// (app_server went away); that it ended the capture is `stopped`.
    pub fn poll(&mut self) -> Result<Option<Result<(), String>>, String> {
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        self.queue.dispatch_pending(&mut self.state).map_err(|e| e.to_string())?;
        if self.following.is_some() {
            self.cursor_came()?;
        }
        let Some(done) = self.state.frame.take() else { return Ok(None) };
        if let Some(frame) = self.frame.take() {
            frame.destroy();
        }
        let lent = self.lend_drawing.take();
        match done {
            Ok(()) if lent.is_some() => self.lend_last = lent,
            Ok(()) if self.into_own => {
                self.filled = true;
                self.lend_last = None;
            }
            Ok(()) => {}
            Err(_) if lent.is_some() => {}
            Err(_) => self.fresh = true,
        }
        Ok(Some(done))
    }

    /// Whether app_server ended the session: what it showed went (a
    /// window closed), or app_server is going away (a reload, which ends
    /// the connection soon after).
    pub fn stopped(&self) -> bool {
        self.state.stopped
    }

    /// Whether a frame asked for hasn't come yet.
    pub fn waiting(&self) -> bool {
        self.frame.is_some()
    }

    /// The size of the last frame that came (which `copy_into` copies):
    /// None before one has, or while a buffer of another size waits for
    /// its first. (`size` is the size the next will be.)
    pub fn frame_size(&self) -> Option<(u32, u32)> {
        if let Some(b) = self.lend_last.and_then(|i| self.lending.as_ref()?.get(i)) {
            return Some((b.width, b.height));
        }
        self.buffer.as_ref().filter(|_| self.filled).map(|(_, _, (w, h, _, _))| (*w, *h))
    }

    /// Copy the last frame into `out`, `stride` bytes a row (as wide as the
    /// frame, or wider).
    pub fn copy_into(&self, out: &mut [u8], stride: usize) -> Result<(), String> {
        let lent = self.lend_last.and_then(|i| self.lending.as_ref()?.get(i));
        let (target, w, h) = match (lent, self.buffer.as_ref().filter(|_| self.filled)) {
            (Some(b), _) => (None, &b.width, &b.height),
            (None, Some((_, target, (w, h, _, _)))) => (Some(target), w, h),
            (None, None) => return Err("no frame yet".into()),
        };
        let row = *w as usize * 4;
        if stride < row || out.len() < stride * (*h as usize - 1) + row {
            return Err("the buffer is too small for the frame".into());
        }
        let copy = |from: &[u8], from_stride: usize, out: &mut [u8]| {
            for (src, dst) in from.chunks(from_stride).zip(out.chunks_mut(stride)).take(*h as usize) {
                dst[..row].copy_from_slice(&src[..row]);
            }
        };
        match (target, lent) {
            (Some(Target::Shm(map)), _) => copy(map, row, out),
            (Some(Target::Dma(bo)), _) => bo.map(0, 0, *w, *h, |m| copy(m.buffer(), m.stride() as usize, out)).map_err(|e| format!("can't read the dmabuf: {e}"))?,
            (None, Some(b)) => b.bo.map(0, 0, *w, *h, |m| copy(m.buffer(), m.stride() as usize, out)).map_err(|e| format!("can't read the dmabuf: {e}"))?,
            (None, None) => unreachable!(),
        }
        Ok(())
    }

    /// The next frame, waiting for it.
    pub fn frame(&mut self) -> Result<Picture, String> {
        self.request()?;
        loop {
            if let Some(done) = self.poll()? {
                done?;
                break;
            }
            if self.state.stopped {
                return Err("app_server stopped the capture".into());
            }
            self.queue.blocking_dispatch(&mut self.state).map_err(|e| e.to_string())?;
        }
        let (_, _, (w, h, _, format)) = self.buffer.as_ref().unwrap();
        let (w, h, format) = (*w, *h, *format);
        let mut data = vec![0; (w * 4 * h) as usize];
        self.copy_into(&mut data, w as usize * 4)?;
        Ok(Picture { width: w, height: h, stride: w * 4, format, data })
    }
}

/// A session's options: the pointer drawn in, or not.
fn options(cursor: bool) -> ext_image_copy_capture_manager_v1::Options {
    if cursor { ext_image_copy_capture_manager_v1::Options::PaintCursors } else { ext_image_copy_capture_manager_v1::Options::empty() }
}

/// A shared-memory buffer of `w`×`h` in `format`, and its memory.
fn shm(shm: &wl_shm::WlShm, qh: &QueueHandle<State>, w: u32, h: u32, format: wl_shm::Format) -> Result<(wl_buffer::WlBuffer, memmap2::MmapMut), String> {
    let stride = w * 4;
    let len = (stride * h) as usize;
    // SAFETY: a fresh memfd, owned from here on.
    let fd = unsafe {
        let raw = libc::memfd_create(c"olo-portal-cursor".as_ptr(), libc::MFD_CLOEXEC);
        if raw < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        OwnedFd::from_raw_fd(raw)
    };
    let file = std::fs::File::from(fd);
    file.set_len(len as u64).map_err(|e| e.to_string())?;
    // SAFETY: the file is ours and stays this size.
    let map = unsafe { memmap2::MmapMut::map_mut(&file) }.map_err(|e| e.to_string())?;
    let pool = shm.create_pool(file.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, qh, ());
    pool.destroy();
    Ok((buffer, map))
}

/// In an Olo session, straight to app_server (as the kit does): the
/// session's socket is olo-wlproxy's, which hides capture from outside
/// programs.
/// app_server's direct socket for `display` (where its capture is), else
/// the display's own socket.
fn connect(display: Option<&str>) -> Result<Connection, wayland_client::ConnectError> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR");
    if let (Some(dir), Some(display)) = (&dir, display)
        && !display.contains('/')
        && let Ok(stream) = std::os::unix::net::UnixStream::connect(olo_xdg::legacy::runtime_file(std::path::Path::new(dir), &format!("olo-{display}-direct")))
    {
        return Connection::from_socket(stream);
    }
    match (display, dir) {
        (Some(display), Some(dir)) => {
            let path = if display.starts_with('/') { std::path::PathBuf::from(display) } else { std::path::Path::new(&dir).join(display) };
            let stream = std::os::unix::net::UnixStream::connect(path).map_err(|_| wayland_client::ConnectError::NoCompositor)?;
            Connection::from_socket(stream)
        }
        _ => Connection::connect_to_env(),
    }
}

/// The render node whose device number is `dev`.
fn open_render_node(dev: libc::dev_t) -> Result<std::fs::File, String> {
    use std::os::unix::fs::MetadataExt;
    let dir = std::fs::read_dir("/dev/dri").map_err(|e| format!("/dev/dri: {e}"))?;
    for entry in dir.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD")) && std::fs::metadata(&path).is_ok_and(|m| m.rdev() == dev) {
            return std::fs::OpenOptions::new().read(true).write(true).open(&path).map_err(|e| format!("{}: {e}", path.display()));
        }
    }
    Err(format!("no render node for device {dev:#x}"))
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(state: &mut Self, output: &wl_output::WlOutput, event: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = event
            && let Some(o) = state.outputs.iter_mut().find(|(o, _)| o == output)
        {
            o.1 = name;
        }
    }
}

impl Dispatch<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, ()> for State {
    fn event(state: &mut Self, _: &ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, event: ext_foreign_toplevel_list_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.push(Toplevel { handle: toplevel, identifier: String::new(), app_id: String::new(), title: String::new() });
        }
    }

    wayland_client::event_created_child!(State, ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()> for State {
    fn event(state: &mut Self, handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, event: ext_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some(t) = state.toplevels.iter_mut().find(|t| t.handle == *handle) else { return };
        match event {
            ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } => t.identifier = identifier,
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => t.app_id = app_id,
            ext_foreign_toplevel_handle_v1::Event::Title { title } => t.title = title,
            ext_foreign_toplevel_handle_v1::Event::Closed => state.toplevels.retain(|t| t.handle != *handle),
            _ => {}
        }
    }
}

impl Dispatch<session_v1::ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(state: &mut Self, session: &session_v1::ExtImageCopyCaptureSessionV1, event: session_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if state.session.as_ref().is_some_and(|s| *s != wayland_client::Proxy::id(session)) {
            return;
        }
        match event {
            // (A new set of constraints starts with the size.)
            session_v1::Event::BufferSize { width, height } => {
                state.size = Some((width, height));
                state.formats.clear();
                state.dma_formats.clear();
            }
            session_v1::Event::ShmFormat { format } => {
                if let wayland_client::WEnum::Value(f) = format {
                    state.formats.push(f);
                }
            }
            session_v1::Event::DmabufDevice { device } => {
                state.dma_device = device.get(..std::mem::size_of::<libc::dev_t>()).map(|b| libc::dev_t::from_ne_bytes(b.try_into().unwrap()));
            }
            session_v1::Event::DmabufFormat { format, modifiers } => {
                let modifiers = modifiers.chunks_exact(8).map(|b| u64::from_ne_bytes(b.try_into().unwrap())).collect();
                state.dma_formats.push((format, modifiers));
            }
            session_v1::Event::Done => state.constraints_done = true,
            session_v1::Event::Stopped => state.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<frame_v1::ExtImageCopyCaptureFrameV1, ()> for State {
    fn event(state: &mut Self, _: &frame_v1::ExtImageCopyCaptureFrameV1, event: frame_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            frame_v1::Event::Ready => state.frame = Some(Ok(())),
            frame_v1::Event::Failed { reason } => state.frame = Some(Err(format!("the frame failed: {reason:?}"))),
            _ => {}
        }
    }
}

impl Dispatch<cursor_v1::ExtImageCopyCaptureCursorSessionV1, ()> for State {
    fn event(state: &mut Self, _: &cursor_v1::ExtImageCopyCaptureCursorSessionV1, event: cursor_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let c = &mut state.cursor;
        match event {
            cursor_v1::Event::Enter => {}
            cursor_v1::Event::Leave => c.position = None,
            cursor_v1::Event::Position { x, y } => c.position = Some((x, y)),
            cursor_v1::Event::Hotspot { x, y } => c.hotspot = (x, y),
            _ => {}
        }
        c.changed = true;
    }
}

impl Dispatch<session_v1::ExtImageCopyCaptureSessionV1, OfCursor> for State {
    fn event(state: &mut Self, _: &session_v1::ExtImageCopyCaptureSessionV1, event: session_v1::Event, _: &OfCursor, _: &Connection, _: &QueueHandle<Self>) {
        let c = &mut state.cursor;
        match event {
            session_v1::Event::BufferSize { width, height } => {
                c.size = Some((width, height));
                c.formats.clear();
            }
            session_v1::Event::ShmFormat { format } => {
                if let wayland_client::WEnum::Value(f) = format {
                    c.formats.push(f);
                }
            }
            session_v1::Event::Done => c.constraints_done = true,
            _ => {}
        }
    }
}

impl Dispatch<frame_v1::ExtImageCopyCaptureFrameV1, OfCursor> for State {
    fn event(state: &mut Self, _: &frame_v1::ExtImageCopyCaptureFrameV1, event: frame_v1::Event, _: &OfCursor, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            frame_v1::Event::Ready => state.cursor.frame = Some(Ok(())),
            frame_v1::Event::Failed { reason } => state.cursor.frame = Some(Err(format!("the cursor's picture failed: {reason:?}"))),
            _ => {}
        }
    }
}

macro_rules! ignore {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore!(
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    OloImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
    wl_seat::WlSeat,
    wl_pointer::WlPointer
);
