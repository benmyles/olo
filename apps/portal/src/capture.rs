//! The portal's side of screen capture: a Wayland client of app_server's
//! ext-image-copy-capture (on the direct socket, where outside programs
//! never see it). A `Capture` is a session on one output: it learns the
//! buffer size and formats app_server takes and keeps a buffer of that
//! size, a dmabuf on the GPU app_server names where it can (app_server
//! draws into it on the GPU, reading nothing back) else shared memory, and
//! gives frames as BGRx rows (what WebRTC wants first).

use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool},
};
use wayland_protocols::{
    ext::{
        foreign_toplevel_list::v1::client::{ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1},
        image_capture_source::v1::client::{ext_foreign_toplevel_image_capture_source_manager_v1, ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1},
        image_copy_capture::v1::client::{ext_image_copy_capture_frame_v1 as frame_v1, ext_image_copy_capture_manager_v1, ext_image_copy_capture_session_v1 as session_v1},
    },
    wp::linux_dmabuf::zv1::client::{zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1},
};

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
}

impl Capture {
    /// Capture `source` on the compositor at `WAYLAND_DISPLAY`, with the
    /// pointer drawn in if `cursor`.
    pub fn new(source: &Source, cursor: bool) -> Result<Capture, String> {
        let conn = connect().map_err(|e| format!("can't reach app_server: {e}"))?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let mut state = State::default();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(|_| "no wl_shm".to_owned())?;
        let dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1> = globals.bind(&qh, 3..=3, ()).ok();
        let copier: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "app_server offers no ext-image-copy-capture".to_owned())?;
        let (image_source, name) = match source {
            Source::Output(name) => {
                let sources: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "app_server offers no output capture (ext-image-capture-source)".to_owned())?;
                for g in globals.contents().clone_list().iter().filter(|g| g.interface == "wl_output") {
                    let o: wl_output::WlOutput = globals.registry().bind(g.name, g.version.min(4), &qh, ());
                    state.outputs.push((o, String::new()));
                }
                // The outputs' names come with their first events.
                queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
                let (output, name) = match name {
                    Some(n) => state.outputs.iter().find(|(_, o)| o == n).cloned().ok_or_else(|| format!("no output {n}"))?,
                    None => state.outputs.first().cloned().ok_or("no outputs")?,
                };
                (sources.create_source(&output, &qh, ()), name)
            }
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
        let session = copier.create_session(&image_source, if cursor { ext_image_copy_capture_manager_v1::Options::PaintCursors } else { ext_image_copy_capture_manager_v1::Options::empty() }, &qh, ());
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
                    tracing::warn!("portal: no dmabufs ({e}), capturing into shared memory");
                    None
                }
            })
        };
        Ok(Capture { queue, state, shm, dmabuf, gbm, conn, name, session, buffer: None, fresh: true, filled: false, frame: None, into_own: false })
    }

    /// Whether frames come through the GPU (dmabufs).
    pub fn on_gpu(&self) -> bool {
        matches!(self.buffer, Some((_, Target::Dma(_), _)))
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
    fn dma_buffer(&self, w: u32, h: u32) -> Result<(wl_buffer::WlBuffer, gbm::BufferObject<()>, wl_shm::Format), String> {
        let (gbm, dmabuf) = (self.gbm.as_ref().ok_or("no GPU")?, self.dmabuf.as_ref().ok_or("no linux-dmabuf")?);
        let pick = [(gbm::Format::Xrgb8888, wl_shm::Format::Xrgb8888), (gbm::Format::Argb8888, wl_shm::Format::Argb8888)];
        let (fourcc, shm_format, modifiers) = pick
            .into_iter()
            .find_map(|(f, s)| self.state.dma_formats.iter().find(|(c, _)| *c == f as u32).map(|(_, m)| (f, s, m.clone())))
            .ok_or("app_server draws no BGRx or BGRA dmabufs")?;
        // Linear when asked (`OLO_PORTAL_LINEAR`, to compare), else what the
        // GPU likes best of what app_server draws.
        let linear = u64::from(gbm::Modifier::Linear);
        let modifiers: Vec<u64> = if std::env::var_os("OLO_PORTAL_LINEAR").is_some() { modifiers.into_iter().filter(|m| *m == linear).collect() } else { modifiers };
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
        if self.buffer.as_ref().is_none_or(|b| (b.2.0, b.2.1) != (w, h)) {
            if let Some((old, _, _)) = self.buffer.take() {
                old.destroy();
            }
            let dma = if self.gbm.is_some() {
                match self.dma_buffer(w, h) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        tracing::warn!("portal: {e}; capturing into shared memory");
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
        let Some(done) = self.state.frame.take() else { return Ok(None) };
        if let Some(frame) = self.frame.take() {
            frame.destroy();
        }
        match done {
            Ok(()) if self.into_own => self.filled = true,
            Ok(()) => {}
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
        self.buffer.as_ref().filter(|_| self.filled).map(|(_, _, (w, h, _, _))| (*w, *h))
    }

    /// Copy the last frame into `out`, `stride` bytes a row (as wide as the
    /// frame, or wider).
    pub fn copy_into(&self, out: &mut [u8], stride: usize) -> Result<(), String> {
        let Some((_, target, (w, h, _, _))) = self.buffer.as_ref().filter(|_| self.filled) else { return Err("no frame yet".into()) };
        let row = *w as usize * 4;
        if stride < row || out.len() < stride * (*h as usize - 1) + row {
            return Err("the buffer is too small for the frame".into());
        }
        let copy = |from: &[u8], from_stride: usize, out: &mut [u8]| {
            for (src, dst) in from.chunks(from_stride).zip(out.chunks_mut(stride)).take(*h as usize) {
                dst[..row].copy_from_slice(&src[..row]);
            }
        };
        match target {
            Target::Shm(map) => copy(map, row, out),
            Target::Dma(bo) => bo.map(0, 0, *w, *h, |m| copy(m.buffer(), m.stride() as usize, out)).map_err(|e| format!("can't read the dmabuf: {e}"))?,
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

/// In an Olo session, straight to app_server (as the kit does): the
/// session's socket is olo-wlproxy's, which hides capture from outside
/// programs.
fn connect() -> Result<Connection, wayland_client::ConnectError> {
    if std::env::var_os("WAYLAND_SOCKET").is_none()
        && let (Some(dir), Ok(display)) = (std::env::var_os("XDG_RUNTIME_DIR"), std::env::var("WAYLAND_DISPLAY"))
        && !display.contains('/')
        && let Ok(stream) = std::os::unix::net::UnixStream::connect(olo_xdg::legacy::runtime_file(std::path::Path::new(&dir), &format!("olo-{display}-direct")))
    {
        return Connection::from_socket(stream);
    }
    Connection::connect_to_env()
}

/// The render node whose device number is `dev`.
fn open_render_node(dev: libc::dev_t) -> Result<std::fs::File, String> {
    use std::os::unix::fs::MetadataExt;
    let dir = std::fs::read_dir("/dev/dri").map_err(|e| format!("/dev/dri: {e}"))?;
    for entry in dir.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD"))
            && std::fs::metadata(&path).is_ok_and(|m| m.rdev() == dev)
        {
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
    fn event(state: &mut Self, _: &session_v1::ExtImageCopyCaptureSessionV1, event: session_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
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
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1
);

