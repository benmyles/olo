//! The portal's side of screen capture: a Wayland client of app_server's
//! ext-image-copy-capture (on the direct socket, where outside programs
//! never see it). A `Capture` is a session on one output: it learns the
//! buffer size and formats app_server takes, keeps a shared-memory buffer
//! of that size, and gives frames as BGRx rows (what WebRTC wants first).

use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool},
};
use wayland_protocols::ext::{
    image_capture_source::v1::client::{ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1},
    image_copy_capture::v1::client::{ext_image_copy_capture_frame_v1 as frame_v1, ext_image_copy_capture_manager_v1, ext_image_copy_capture_session_v1 as session_v1},
};

/// One frame: its size and its rows (BGRx or BGRA, as app_server gave).
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: wl_shm::Format,
    pub data: Vec<u8>,
}

#[derive(Default)]
struct State {
    outputs: Vec<(wl_output::WlOutput, String)>,
    size: Option<(u32, u32)>,
    formats: Vec<wl_shm::Format>,
    constraints_done: bool,
    stopped: bool,
    frame: Option<Result<(), String>>,
}

/// A capture session on one of app_server's outputs.
pub struct Capture {
    queue: EventQueue<State>,
    state: State,
    shm: wl_shm::WlShm,
    session: session_v1::ExtImageCopyCaptureSessionV1,
    buffer: Option<(wl_buffer::WlBuffer, memmap2::MmapMut, (u32, u32, u32, wl_shm::Format))>,
}

impl Capture {
    /// Capture the output named `name` (the first one if None) of the
    /// compositor at `WAYLAND_DISPLAY`.
    pub fn output(name: Option<&str>) -> Result<Capture, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("can't reach app_server: {e}"))?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let mut state = State::default();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(|_| "no wl_shm".to_owned())?;
        let sources: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "app_server offers no output capture (ext-image-capture-source)".to_owned())?;
        let copier: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "app_server offers no ext-image-copy-capture".to_owned())?;
        for g in globals.contents().clone_list().iter().filter(|g| g.interface == "wl_output") {
            let o: wl_output::WlOutput = globals.registry().bind(g.name, g.version.min(4), &qh, ());
            state.outputs.push((o, String::new()));
        }
        // The outputs' names come with their first events.
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        let output = match name {
            Some(n) => state.outputs.iter().find(|(_, o)| o == n).map(|(o, _)| o.clone()).ok_or_else(|| format!("no output {n}"))?,
            None => state.outputs.first().map(|(o, _)| o.clone()).ok_or("no outputs")?,
        };
        let source = sources.create_source(&output, &qh, ());
        let session = copier.create_session(&source, ext_image_copy_capture_manager_v1::Options::empty(), &qh, ());
        while !state.constraints_done && !state.stopped {
            queue.blocking_dispatch(&mut state).map_err(|e| e.to_string())?;
        }
        if state.stopped {
            return Err("app_server stopped the capture".into());
        }
        Ok(Capture { queue, state, shm, session, buffer: None })
    }

    /// The next frame.
    pub fn frame(&mut self) -> Result<Picture, String> {
        let (w, h) = self.state.size.ok_or("app_server gave no size")?;
        let format = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888].into_iter().find(|f| self.state.formats.contains(f)).ok_or("no BGRx/BGRA format offered")?;
        let stride = w * 4;
        let qh = self.queue.handle();
        if self.buffer.as_ref().is_none_or(|b| b.2 != (w, h, stride, format)) {
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
            let pool = self.shm.create_pool(file.as_fd(), len as i32, &qh, ());
            let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, &qh, ());
            pool.destroy();
            if let Some((old, _, _)) = self.buffer.take() {
                old.destroy();
            }
            self.buffer = Some((buffer, map, (w, h, stride, format)));
        }
        let (buffer, _, _) = self.buffer.as_ref().unwrap();
        self.state.frame = None;
        let frame = self.session.create_frame(&qh, ());
        frame.attach_buffer(buffer);
        frame.damage_buffer(0, 0, w as i32, h as i32);
        frame.capture();
        while self.state.frame.is_none() && !self.state.stopped {
            self.queue.blocking_dispatch(&mut self.state).map_err(|e| e.to_string())?;
        }
        frame.destroy();
        match self.state.frame.take() {
            Some(Ok(())) => {
                let (_, map, _) = self.buffer.as_ref().unwrap();
                Ok(Picture { width: w, height: h, stride, format, data: map.to_vec() })
            }
            Some(Err(e)) => Err(e),
            None => Err("app_server stopped the capture".into()),
        }
    }
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

impl Dispatch<session_v1::ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(state: &mut Self, _: &session_v1::ExtImageCopyCaptureSessionV1, event: session_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            session_v1::Event::BufferSize { width, height } => state.size = Some((width, height)),
            session_v1::Event::ShmFormat { format } => {
                if let wayland_client::WEnum::Value(f) = format {
                    state.formats.push(f);
                }
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
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1
);
