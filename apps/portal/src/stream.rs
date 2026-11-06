//! A screen as a PipeWire video source, for the portal's ScreenCast. One
//! thread runs PipeWire's loop, which also reads app_server (the capture's
//! connection is one of its sources): while the stream streams, a frame is
//! asked for at most at the rate the app agreed to; app_server answers
//! once the screen has changed (so a still screen sends nothing); and the
//! stream, its own driver, is then given it. An app that takes dmabufs
//! (Chrome, OBS, GStreamer) is offered BGRx with the modifiers app_server
//! draws; once it agrees, each of the stream's buffers is a dmabuf of the
//! portal's on the GPU that app_server captures straight into, so a frame
//! goes from the screen to the app without touching the CPU. Else the
//! stream's buffers are memory (memfds) the frame is copied into from the
//! capture's own buffer (BGRx rows). The stream allocates its buffers
//! itself either way (`add_buffer`). `run` returns once PipeWire has given
//! the stream a node, whose id Start hands the app.

use std::{
    cell::RefCell,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use pipewire as pw;
use pw::{properties::properties, spa};
use wayland_client::protocol::wl_buffer::WlBuffer;

use olo_capture::{Capture, Source};

/// A running share: setting `stop`, or dropping it, ends it.
pub struct Share {
    pub node: u32,
    pub size: (u32, u32),
    pub stop: Arc<AtomicBool>,
}

impl Drop for Share {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        LIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Shares running (while there are, an upgrade waits: `App::busy`).
static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// How the pointer goes with a share (the portal's cursor modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// Not at all.
    Hidden,
    /// Drawn into the picture.
    Embedded,
    /// Apart, as metadata on each buffer (where it is, and its picture
    /// when that changes): for an app that draws it itself (a remote
    /// desktop's viewer, OBS). A move with nothing else changed is a
    /// buffer with no picture.
    Metadata,
}

/// Start sharing `source` (a screen, or a window), the pointer as
/// `cursor` says. `lost` is called if the share ends by itself, not
/// stopped (app_server never came back).
pub fn run(source: Source, cursor: Cursor, lost: impl FnOnce() + Send + 'static) -> Result<Share, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<(u32, (u32, u32)), String>>();
    {
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("portal-stream".into())
            .spawn(move || {
                let ended = stream(source, cursor, stop.clone(), &tx);
                if let Err(e) = &ended {
                    tracing::warn!("portal: the share ended: {e}");
                    let _ = tx.send(Err(e.clone()));
                }
                if !stop.load(Ordering::Relaxed) {
                    lost();
                }
            })
            .map_err(|e| e.to_string())?;
    }
    let started = rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "PipeWire gave the stream no node".to_owned()).and_then(|r| r);
    let (node, size) = match started {
        Ok(s) => s,
        Err(e) => {
            stop.store(true, Ordering::Relaxed);
            return Err(e);
        }
    };
    LIVE.fetch_add(1, Ordering::Relaxed);
    Ok(Share { node, size, stop })
}

/// One of the stream's buffers, as the portal made it (`add_buffer`).
struct Slot {
    pw: *mut pw::sys::pw_buffer,
    kind: Kind,
}

enum Kind {
    /// A dmabuf app_server captures into, which the app gets as it is.
    Dma { bo: gbm::BufferObject<()>, wl: WlBuffer, _fds: Vec<OwnedFd> },
    /// Memory the frame is copied into.
    Mem { _map: memmap2::MmapMut, _fd: OwnedFd },
}

/// What the loop's sources share.
struct Shared {
    capture: Capture,
    /// The size the stream was offered.
    size: (u32, u32),
    /// A frame came that the stream hasn't had (in memory).
    fresh: bool,
    streaming: bool,
    /// How often frames may come, as agreed.
    every: Duration,
    asked: std::time::Instant,
    /// app_server went away: waiting for it to come back.
    lost: bool,
    /// The modifiers offered for dmabufs (None: memory only).
    modifiers: Option<Vec<u64>>,
    /// The modifier agreed, while frames go as dmabufs.
    dma: Option<u64>,
    slots: Vec<Slot>,
    /// The stream's buffer app_server captures into (or did, and failed:
    /// used for the next).
    filling: Option<*mut pw::sys::pw_buffer>,
    /// How frames were last said to go (a modifier: as dmabufs).
    told: Option<Option<u64>>,
    /// The frames given so far (each one's number in its header).
    seq: u64,
    /// When app_server ended the capture: what was shared went, unless
    /// app_server itself goes soon after (a reload, which the share
    /// outlives).
    stopped_at: Option<std::time::Instant>,
    /// The pointer goes as metadata (`Cursor::Metadata`, and the capture
    /// follows it).
    cursor_meta: bool,
    /// The cursor picture last sent (its serial), and whether the empty
    /// one (off the screen) was.
    cursor_sent: u64,
    cursor_gone_sent: bool,
    /// When a buffer last went with only the cursor (they go no more
    /// often than frames may).
    cursor_at: std::time::Instant,
}

/// How long after the capture ends to wait for app_server going too.
const STOPPED_WAIT: Duration = Duration::from_secs(2);

impl Shared {
    fn slot(&self, b: *mut pw::sys::pw_buffer) -> Option<&Slot> {
        self.slots.iter().find(|s| s.pw == b)
    }
}

/// Serialize a pod value.
fn serialize(value: &pw::spa::pod::Value) -> Option<Vec<u8>> {
    pw::spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), value).map(|r| r.0.into_inner()).ok()
}

/// Offer `formats` (the app renegotiates).
fn offer_formats(stream: &pw::stream::Stream, formats: &[Vec<u8>]) {
    let mut pods: Vec<&pw::spa::pod::Pod> = formats.iter().filter_map(|b| pw::spa::pod::Pod::from_bytes(b)).collect();
    let _ = stream.update_params(&mut pods);
}

/// Offer the stream at a new size (the app renegotiates).
fn offer(stream: &pw::stream::Stream, (w, h): (u32, u32), modifiers: Option<&[u64]>) {
    offer_formats(stream, &formats(w, h, modifiers));
}

/// A capture of `source` again, once app_server is back: tried every
/// quarter second for half a minute (or until the share stops).
fn reconnect(source: &Source, cursor: Cursor, stop: &AtomicBool) -> Option<Capture> {
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        match open(source, cursor) {
            Ok(c) => {
                tracing::info!("portal: sharing {} again", c.name());
                return Some(c);
            }
            Err(e) => tracing::debug!("portal: not yet: {e}"),
        }
    }
    tracing::warn!("portal: app_server didn't come back; the share of {source:?} ends");
    None
}

/// A capture of `source`, the pointer drawn in or followed apart as
/// `cursor` says (where following it fails, it's left out).
fn open(source: &Source, cursor: Cursor) -> Result<Capture, String> {
    let mut capture = Capture::new(source, cursor == Cursor::Embedded)?;
    if cursor == Cursor::Metadata
        && let Err(e) = capture.follow_cursor()
    {
        tracing::warn!("portal: the cursor can't go apart ({e}): the share has none");
    }
    Ok(capture)
}

/// The largest cursor picture a buffer's metadata holds (a side, in
/// pixels): Olo's cursors are 32 points, so 96 pixels at 3×.
const CURSOR_MAX: u32 = 256;

/// The bytes a cursor's metadata takes: where it is, then its picture.
fn cursor_meta_size() -> usize {
    use pw::spa::sys;
    std::mem::size_of::<sys::spa_meta_cursor>() + std::mem::size_of::<sys::spa_meta_bitmap>() + (CURSOR_MAX * CURSOR_MAX * 4) as usize
}

/// The cursor's metadata, asked for with the buffers (`Cursor::Metadata`).
fn cursor_param() -> Option<Vec<u8>> {
    use pw::spa::{
        pod::{Object, Property, Value},
        sys,
        utils::Id,
    };
    let object = Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: pw::spa::param::ParamType::Meta.as_raw(),
        properties: vec![Property::new(sys::SPA_PARAM_META_type, Value::Id(Id(sys::SPA_META_Cursor))), Property::new(sys::SPA_PARAM_META_size, Value::Int(cursor_meta_size() as i32))],
    };
    serialize(&Value::Object(object))
}

/// Put where the pointer is (and its picture, if it changed since it
/// was last sent) in buffer `b`'s cursor metadata, if it has some. Off
/// the screen, an empty picture is sent once.
///
/// # Safety
/// `b` is one of the stream's buffers, dequeued.
unsafe fn put_cursor(b: *mut pw::sys::pw_buffer, sh: &mut Shared) {
    use pw::spa::sys;
    let Some((c, _)) = sh.capture.cursor() else { return };
    let head = std::mem::size_of::<sys::spa_meta_cursor>();
    let bitmap = std::mem::size_of::<sys::spa_meta_bitmap>();
    // SAFETY: the buffer's metas are PipeWire's, `n_metas` of them, each
    // `size` bytes at `data`; written only within that.
    unsafe {
        let buffer = (*b).buffer;
        if (*buffer).metas.is_null() {
            return;
        }
        let Some(m) = std::slice::from_raw_parts((*buffer).metas, (*buffer).n_metas as usize).iter().find(|m| m.type_ == sys::SPA_META_Cursor && m.size as usize >= head + bitmap && !m.data.is_null())
        else {
            return;
        };
        let room = m.size as usize;
        let mc = m.data.cast::<sys::spa_meta_cursor>();
        (*mc).id = 1;
        (*mc).flags = 0;
        (*mc).bitmap_offset = 0;
        // (The picture after the cursor's head; its offset set by the caller.)
        let picture = |w: u32, h: u32, pixels: Option<&[u8]>| {
            let need = head + bitmap + (w * h * 4) as usize;
            if need > room {
                return false;
            }
            let mb = m.data.cast::<u8>().add(head).cast::<sys::spa_meta_bitmap>();
            (*mb).format = sys::SPA_VIDEO_FORMAT_BGRA;
            (*mb).size = sys::spa_rectangle { width: w, height: h };
            (*mb).stride = (w * 4) as i32;
            (*mb).offset = bitmap as u32;
            let at = std::slice::from_raw_parts_mut(m.data.cast::<u8>().add(head + bitmap), (w * h * 4) as usize);
            match pixels {
                Some(p) => at.copy_from_slice(&p[..at.len()]),
                None => at.fill(0),
            }
            true
        };
        match c.position {
            Some((x, y)) => {
                (*mc).position = sys::spa_point { x, y };
                (*mc).hotspot = sys::spa_point { x: c.hotspot.0, y: c.hotspot.1 };
                let (w, h) = c.size;
                if (c.serial != sh.cursor_sent || sh.cursor_gone_sent)
                    && let Some(pixels) = sh.capture.cursor_image()
                    && picture(w, h, Some(pixels))
                {
                    (*mc).bitmap_offset = head as u32;
                    sh.cursor_sent = c.serial;
                    sh.cursor_gone_sent = false;
                }
            }
            None => {
                (*mc).position = sys::spa_point { x: 0, y: 0 };
                (*mc).hotspot = sys::spa_point { x: 0, y: 0 };
                if !sh.cursor_gone_sent && picture(1, 1, None) {
                    (*mc).bitmap_offset = head as u32;
                    sh.cursor_gone_sent = true;
                }
            }
        }
    }
}

/// The stream's formats: BGRx at the screen's size, as often as it
/// changes up to 60 times a second; as dmabufs with one of `modifiers`
/// first (where frames can come so), then in memory.
fn formats(w: u32, h: u32, modifiers: Option<&[u64]>) -> Vec<Vec<u8>> {
    use pw::spa::{
        param::format::FormatProperties as P,
        pod::{self, ChoiceValue, Property, PropertyFlags, Value},
        utils::{self, Choice, ChoiceEnum, ChoiceFlags},
    };
    let format = |modifier: Option<Property>| {
        let mut object = pod::object!(
            utils::SpaTypes::ObjectParamFormat,
            pw::spa::param::ParamType::EnumFormat,
            pod::property!(P::MediaType, Id, pw::spa::param::format::MediaType::Video),
            pod::property!(P::MediaSubtype, Id, pw::spa::param::format::MediaSubtype::Raw),
            pod::property!(P::VideoFormat, Id, pw::spa::param::video::VideoFormat::BGRx),
            pod::property!(P::VideoSize, Rectangle, utils::Rectangle { width: w, height: h }),
            pod::property!(P::VideoFramerate, Fraction, utils::Fraction { num: 0, denom: 1 }),
            pod::property!(P::VideoMaxFramerate, Choice, Range, Fraction, utils::Fraction { num: 60, denom: 1 }, utils::Fraction { num: 1, denom: 1 }, utils::Fraction { num: 60, denom: 1 }),
        );
        object.properties.extend(modifier);
        serialize(&Value::Object(object))
    };
    let mut out = Vec::new();
    if let Some(&[first, ..]) = modifiers {
        let modifiers = modifiers.unwrap();
        // One modifier: the one chosen. Several: the app says which it
        // can take, and the portal picks of those (DONT_FIXATE).
        let (flags, value) = if modifiers.len() == 1 {
            (PropertyFlags::MANDATORY, Value::Long(first as i64))
        } else {
            (
                PropertyFlags::MANDATORY | PropertyFlags::DONT_FIXATE,
                Value::Choice(ChoiceValue::Long(Choice(ChoiceFlags::empty(), ChoiceEnum::Enum { default: first as i64, alternatives: modifiers.iter().map(|&m| m as i64).collect() }))),
            )
        };
        out.extend(format(Some(Property { key: P::VideoModifier.as_raw(), flags, value })));
    }
    out.extend(format(None));
    out
}

/// The modifier the app agreed to in `format`: None for memory, else the
/// one chosen, or the ones it can take (for the portal to choose from).
fn modifier_of(format: &pw::spa::pod::Pod) -> Option<Result<u64, Vec<u64>>> {
    use pw::spa::{
        param::format::FormatProperties as P,
        pod::{ChoiceValue, Value, deserialize::PodDeserializer},
        utils::{Choice, ChoiceEnum},
    };
    let Ok((_, Value::Object(object))) = PodDeserializer::deserialize_any_from(format.as_bytes()) else { return None };
    let value = object.properties.into_iter().find(|p| p.key == P::VideoModifier.as_raw())?.value;
    Some(match value {
        Value::Long(m) => Ok(m as u64),
        Value::Choice(ChoiceValue::Long(Choice(_, ChoiceEnum::None(m)))) => Ok(m as u64),
        Value::Choice(ChoiceValue::Long(Choice(_, ChoiceEnum::Enum { default, alternatives }))) => {
            let mut all: Vec<u64> = std::iter::once(default).chain(alternatives).map(|m| m as u64).collect();
            all.dedup();
            Err(all)
        }
        _ => return None,
    })
}

/// The buffers, once the format is agreed: dmabufs of `planes` planes,
/// else one plane of a frame in memory (a memfd); made by the portal.
fn buffers(w: u32, h: u32, planes: Option<u32>) -> Option<Vec<u8>> {
    use pw::spa::{
        pod::{ChoiceValue, Object, Property, Value},
        sys,
        utils::{Choice, ChoiceEnum, ChoiceFlags},
    };
    let stride = w * 4;
    let mut properties = vec![
        Property::new(sys::SPA_PARAM_BUFFERS_buffers, Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default: 4, min: 2, max: 8 })))),
        Property::new(sys::SPA_PARAM_BUFFERS_blocks, Value::Int(planes.unwrap_or(1) as i32)),
    ];
    if planes.is_none() {
        properties.push(Property::new(sys::SPA_PARAM_BUFFERS_size, Value::Int((stride * h) as i32)));
        properties.push(Property::new(sys::SPA_PARAM_BUFFERS_stride, Value::Int(stride as i32)));
    }
    let kind = if planes.is_some() { sys::SPA_DATA_DmaBuf } else { sys::SPA_DATA_MemFd };
    // (A choice of flags, as PipeWire reads it.)
    properties.push(Property::new(sys::SPA_PARAM_BUFFERS_dataType, Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Flags { default: 1 << kind, flags: vec![1 << kind] })))));
    let object = Object { type_: pw::spa::utils::SpaTypes::ObjectParamBuffers.as_raw(), id: pw::spa::param::ParamType::Buffers.as_raw(), properties };
    serialize(&Value::Object(object))
}

/// The header each frame carries (`stamp`), asked for with the buffers.
fn header_meta() -> Option<Vec<u8>> {
    use pw::spa::{
        pod::{Object, Property, Value},
        sys,
        utils::Id,
    };
    let object = Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: pw::spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(sys::SPA_PARAM_META_type, Value::Id(Id(sys::SPA_META_Header))),
            Property::new(sys::SPA_PARAM_META_size, Value::Int(std::mem::size_of::<sys::spa_meta_header>() as i32)),
        ],
    };
    serialize(&Value::Object(object))
}

/// Stamp buffer `b`'s header, if it has one, with now (the monotonic
/// clock, as PipeWire's) and its frame number, so apps know when each
/// frame was taken and that it is a new one.
///
/// # Safety
/// `b` is one of the stream's buffers, dequeued.
unsafe fn stamp(b: *mut pw::sys::pw_buffer, seq: u64) {
    use pw::spa::sys;
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: plain clock_gettime into our own timespec; the buffer's
    // metas are PipeWire's, `n_metas` of them, each `size` bytes.
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now);
        let buffer = (*b).buffer;
        if (*buffer).metas.is_null() {
            return;
        }
        for m in std::slice::from_raw_parts((*buffer).metas, (*buffer).n_metas as usize) {
            if m.type_ == sys::SPA_META_Header && m.size as usize >= std::mem::size_of::<sys::spa_meta_header>() && !m.data.is_null() {
                let h = m.data.cast::<sys::spa_meta_header>();
                (*h).flags = 0;
                (*h).offset = 0;
                (*h).pts = now.tv_sec * 1_000_000_000 + now.tv_nsec;
                (*h).dts_offset = 0;
                (*h).seq = seq;
            }
        }
    }
}

/// Give the stream's new buffer `b` its memory: a dmabuf if frames go so,
/// else a memfd, mapped.
fn fill_buffer(sh: &mut Shared, b: *mut pw::sys::pw_buffer) -> Result<(), String> {
    use pw::spa::sys;
    let (w, h) = sh.size;
    // SAFETY: PipeWire's buffer, handed to `add_buffer`, with `n_datas`
    // datas, each with its chunk.
    let datas = unsafe {
        let buffer = (*b).buffer;
        std::slice::from_raw_parts_mut((*buffer).datas, (*buffer).n_datas as usize)
    };
    let first = datas.first().ok_or("a buffer with no data")?;
    let kind = match sh.dma {
        Some(modifier) if first.type_ & (1 << sys::SPA_DATA_DmaBuf) != 0 => {
            let bo = sh.capture.allocate(w, h, &[modifier])?;
            let wl = sh.capture.import(&bo)?;
            let mut fds = Vec::new();
            for (plane, d) in datas.iter_mut().enumerate().take(bo.plane_count() as usize) {
                let fd = bo.fd_for_plane(plane as i32).map_err(|e| e.to_string())?;
                let stride = bo.stride_for_plane(plane as i32);
                d.type_ = sys::SPA_DATA_DmaBuf;
                d.flags = sys::SPA_DATA_FLAG_READABLE;
                d.fd = fd.as_raw_fd() as i64;
                d.mapoffset = 0;
                d.maxsize = stride * h;
                d.data = std::ptr::null_mut();
                // SAFETY: the data's chunk, PipeWire's.
                unsafe {
                    (*d.chunk).offset = bo.offset(plane as i32);
                    (*d.chunk).stride = stride as i32;
                    (*d.chunk).size = stride * h;
                }
                fds.push(fd);
            }
            Kind::Dma { bo, wl, _fds: fds }
        }
        _ => {
            let len = (w * 4 * h) as usize;
            // SAFETY: a fresh memfd, owned from here on.
            let fd = unsafe {
                let raw = libc::memfd_create(c"olo-portal-stream".as_ptr(), libc::MFD_CLOEXEC);
                if raw < 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                OwnedFd::from_raw_fd(raw)
            };
            let file = std::fs::File::from(fd);
            file.set_len(len as u64).map_err(|e| e.to_string())?;
            // SAFETY: the file is ours and stays this size.
            let mut map = unsafe { memmap2::MmapMut::map_mut(&file) }.map_err(|e| e.to_string())?;
            let fd = OwnedFd::from(file);
            let d = &mut datas[0];
            d.type_ = sys::SPA_DATA_MemFd;
            d.flags = sys::SPA_DATA_FLAG_READABLE;
            d.fd = fd.as_raw_fd() as i64;
            d.mapoffset = 0;
            d.maxsize = len as u32;
            d.data = map.as_mut_ptr().cast();
            // SAFETY: the data's chunk, PipeWire's.
            unsafe {
                (*d.chunk).offset = 0;
                (*d.chunk).stride = (w * 4) as i32;
                (*d.chunk).size = len as u32;
            }
            Kind::Mem { _map: map, _fd: fd }
        }
    };
    sh.slots.push(Slot { pw: b, kind });
    Ok(())
}

fn stream(source: Source, cursor: Cursor, stop: Arc<AtomicBool>, tx: &mpsc::Sender<Result<(u32, (u32, u32)), String>>) -> Result<(), String> {
    let mut capture = open(&source, cursor)?;
    let cursor_meta = capture.cursor().is_some();
    // A screen by its name from now on (the first, if none was named).
    let source = match source {
        Source::Output(_) => Source::Output(Some(capture.name().to_owned())),
        window => window,
    };
    let size = capture.size().ok_or("app_server gave no size")?;
    let fd = capture.fd()?;
    let modifiers = capture.dma_modifiers();
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| format!("can't reach PipeWire: {e}"))?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "Olo screen",
        properties! {
            *pw::keys::MEDIA_CLASS => "Video/Source",
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| e.to_string())?;
    let shared = Rc::new(RefCell::new(Shared {
        capture,
        size,
        fresh: false,
        streaming: false,
        every: Duration::from_millis(16),
        asked: std::time::Instant::now(),
        lost: false,
        modifiers,
        dma: None,
        slots: Vec::new(),
        filling: None,
        told: None,
        seq: 0,
        stopped_at: None,
        cursor_meta,
        cursor_sent: 0,
        cursor_gone_sent: false,
        cursor_at: std::time::Instant::now(),
    }));
    let told = std::cell::Cell::new(false);
    let tx = tx.clone();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed({
            let shared = shared.clone();
            move |s, _, _, new| {
                if let pw::stream::StreamState::Error(e) = &new {
                    tracing::warn!("portal: the stream failed: {e}");
                }
                let streaming = matches!(new, pw::stream::StreamState::Streaming);
                let waiting = {
                    let mut sh = shared.borrow_mut();
                    sh.streaming = streaming;
                    sh.fresh
                };
                // A frame that came as the last app left goes to the next
                // (no other comes until it has gone).
                if streaming && waiting {
                    let _ = s.trigger_process();
                }
                if matches!(new, pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming) && !told.replace(true) {
                    let size = shared.borrow().size;
                    let _ = tx.send(Ok((s.node_id(), size)));
                }
            }
        })
        .param_changed({
            let shared = shared.clone();
            move |s, _, id, param| {
                let Some(param) = param.filter(|_| id == pw::spa::param::ParamType::Format.as_raw()) else { return };
                let mut info = pw::spa::param::video::VideoInfoRaw::new();
                if info.parse(param).is_err() {
                    return;
                }
                let (size, rate) = (info.size(), info.max_framerate());
                let (w, h) = (size.width, size.height);
                // What to tell the stream, worked out first: telling it may
                // call back here, or `add_buffer`, at once.
                let next = {
                    let mut sh = shared.borrow_mut();
                    if rate.num > 0 {
                        sh.every = Duration::from_secs_f64(rate.denom as f64 / rate.num as f64);
                    }
                    match modifier_of(param) {
                        // The ones it can take: the GPU's pick of those, offered
                        // alone (else memory only).
                        Some(Err(choices)) => {
                            let picked = sh.capture.allocate(w, h, &choices).map(|bo| u64::from(bo.modifier()));
                            if let Err(e) = &picked {
                                tracing::warn!("portal: none of the app's modifiers will do ({e}); frames go in memory");
                                sh.modifiers = None;
                            }
                            Err(formats(w, h, picked.ok().as_ref().map(std::slice::from_ref)))
                        }
                        Some(Ok(modifier)) => match sh.capture.allocate(w, h, &[modifier]) {
                            Ok(bo) => {
                                if sh.told != Some(Some(modifier)) {
                                    tracing::info!("portal: frames go to the app as dmabufs (modifier {modifier:#x})");
                                    sh.told = Some(Some(modifier));
                                }
                                sh.dma = Some(modifier);
                                Ok(buffers(w, h, Some(bo.plane_count())))
                            }
                            Err(e) => {
                                tracing::warn!("portal: {e}; frames go in memory");
                                sh.modifiers = None;
                                sh.dma = None;
                                Err(formats(w, h, None))
                            }
                        },
                        None => {
                            sh.dma = None;
                            if sh.told != Some(None) {
                                tracing::info!("portal: frames go to the app in memory");
                                sh.told = Some(None);
                            }
                            Ok(buffers(w, h, None))
                        }
                    }
                };
                match next {
                    Ok(Some(bytes)) => {
                        let meta = header_meta().unwrap_or_default();
                        let cursor = if shared.borrow().cursor_meta { cursor_param().unwrap_or_default() } else { Vec::new() };
                        let mut pods: Vec<&pw::spa::pod::Pod> = [&bytes, &meta, &cursor].into_iter().filter_map(|b| pw::spa::pod::Pod::from_bytes(b)).collect();
                        let _ = s.update_params(&mut pods);
                    }
                    Ok(None) => {}
                    Err(formats) => offer_formats(s, &formats),
                }
            }
        })
        .add_buffer({
            let shared = shared.clone();
            move |_, _, b| {
                if let Err(e) = fill_buffer(&mut shared.borrow_mut(), b) {
                    tracing::warn!("portal: a buffer for the stream: {e}");
                }
            }
        })
        .remove_buffer({
            let shared = shared.clone();
            move |_, _, b| {
                let mut sh = shared.borrow_mut();
                if sh.filling == Some(b) {
                    sh.filling = None;
                }
                if let Some(at) = sh.slots.iter().position(|s| s.pw == b) {
                    let slot = sh.slots.remove(at);
                    if let Kind::Dma { wl, .. } = slot.kind {
                        wl.destroy();
                    }
                }
            }
        })
        .process({
            let shared = shared.clone();
            move |s, _| {
                let mut shared = shared.borrow_mut();
                // (Dmabufs go to the stream as they're filled.)
                if shared.dma.is_some() || !std::mem::take(&mut shared.fresh) {
                    return;
                }
                // (A frame of another size than the stream's, while it is
                // offered anew, isn't given.)
                if shared.capture.frame_size() != Some(shared.size) {
                    return;
                }
                // SAFETY: the stream's own buffer, given back below.
                let b = unsafe { s.dequeue_raw_buffer() };
                if b.is_null() {
                    return;
                }
                let (w, h) = shared.size;
                let stride = w as usize * 4;
                // SAFETY: a dequeued buffer's first data, its memory (mapped
                // by `fill_buffer`) `maxsize` bytes, and its chunk.
                unsafe {
                    let d = &mut *(*(*b).buffer).datas;
                    let copied = (!d.data.is_null()).then(|| shared.capture.copy_into(std::slice::from_raw_parts_mut(d.data.cast::<u8>(), d.maxsize as usize), stride));
                    let chunk = &mut *d.chunk;
                    match copied {
                        Some(Ok(())) => {
                            chunk.offset = 0;
                            chunk.stride = stride as i32;
                            chunk.size = (stride * h as usize) as u32;
                        }
                        other => {
                            if let Some(Err(e)) = other {
                                tracing::warn!("portal: {e}");
                            }
                            chunk.size = 0;
                        }
                    }
                    shared.seq += 1;
                    stamp(b, shared.seq);
                    if shared.cursor_meta {
                        put_cursor(b, &mut shared);
                    }
                    s.queue_raw_buffer(b);
                }
            }
        })
        .register()
        .map_err(|e| e.to_string())?;
    let offered = formats(size.0, size.1, shared.borrow().modifiers.as_deref());
    let mut params: Vec<&pw::spa::pod::Pod> = offered.iter().filter_map(|b| pw::spa::pod::Pod::from_bytes(b)).collect();
    stream.connect(spa::utils::Direction::Output, None, pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::ALLOC_BUFFERS, &mut params).map_err(|e| e.to_string())?;
    // (Addresses: the loop's callbacks must be 'static, and the stream
    // outlives the loop that runs them.)
    let stream_at = &*stream as *const pw::stream::Stream as usize;
    // SAFETY: as above.
    let stream_of = move || unsafe { &*(stream_at as *const pw::stream::Stream) };
    // While streaming, the next frame is asked for once the last has gone
    // and its time has come (into a buffer of the stream's, when they're
    // dmabufs); and the share ends when told.
    let timer = mainloop.loop_().add_timer({
        let shared = shared.clone();
        let weak = mainloop.downgrade();
        let stop = stop.clone();
        move |_| {
            if stop.load(Ordering::Relaxed) {
                if let Some(l) = weak.upgrade() {
                    l.quit();
                }
                return;
            }
            let mut sh = shared.borrow_mut();
            // The capture ended and app_server is still there: what was
            // shared is gone (a window closed), and so is the share.
            if !sh.lost && sh.stopped_at.is_some_and(|t| t.elapsed() >= STOPPED_WAIT) {
                tracing::info!("portal: what was shared is gone");
                if let Some(l) = weak.upgrade() {
                    l.quit();
                }
                return;
            }
            // The pointer moved (or changed) with nothing else: a buffer
            // with only it, no more often than frames may come.
            if sh.cursor_meta && !sh.lost && sh.stopped_at.is_none() && sh.streaming && !sh.fresh && sh.capture.cursor_changed() && sh.cursor_at.elapsed() >= sh.every {
                // SAFETY: the stream's own buffer, given back at once.
                let b = unsafe { stream_of().dequeue_raw_buffer() };
                if !b.is_null() {
                    sh.cursor_at = std::time::Instant::now();
                    sh.seq += 1;
                    let seq = sh.seq;
                    // SAFETY: dequeued above; its first data's chunk.
                    unsafe {
                        let d = &mut *(*(*b).buffer).datas;
                        let chunk = &mut *d.chunk;
                        chunk.size = 0;
                        stamp(b, seq);
                        put_cursor(b, &mut sh);
                        stream_of().queue_raw_buffer(b);
                    }
                }
            }
            if sh.lost || sh.stopped_at.is_some() || !sh.streaming || sh.fresh || sh.capture.waiting() || sh.asked.elapsed() < sh.every.saturating_sub(Duration::from_millis(2)) {
                return;
            }
            let asked = if sh.dma.is_some() {
                // SAFETY: the stream's own buffer, given back once filled.
                let b = sh.filling.take().unwrap_or_else(|| unsafe { stream_of().dequeue_raw_buffer() });
                if b.is_null() {
                    // (The app holds them all: later.)
                    return;
                }
                sh.filling = Some(b);
                match sh.slot(b).map(|s| &s.kind) {
                    Some(Kind::Dma { wl, .. }) => {
                        let wl = wl.clone();
                        sh.capture.request_into(&wl)
                    }
                    _ => Err("a buffer of the stream's that isn't a dmabuf".to_owned()),
                }
            } else {
                sh.capture.request()
            };
            sh.asked = std::time::Instant::now();
            if let Err(e) = asked {
                tracing::warn!("portal: {e}");
            }
        }
    });
    let _ = timer.update_timer(Some(Duration::from_millis(1)), Some(Duration::from_millis(4)));
    let mut fd = Some(fd);
    loop {
        // What app_server says: a frame has come (the stream is given it),
        // or the screen changed size (the stream is offered the new one).
        let Some(io) = fd.take() else { break };
        let app_server = mainloop.loop_().add_io(io, pw::spa::support::system::IoFlags::IN, {
            let shared = shared.clone();
            let weak = mainloop.downgrade();
            move |_| {
                let polled = shared.borrow_mut().capture.poll();
                // Ended: wait to see whether app_server goes too (the timer
                // ends the share if not).
                if polled.is_ok() && shared.borrow().capture.stopped() {
                    let mut sh = shared.borrow_mut();
                    if sh.stopped_at.is_none() {
                        tracing::debug!("portal: app_server ended the capture");
                        sh.stopped_at = Some(std::time::Instant::now());
                    }
                    return;
                }
                match polled {
                    Ok(None) => {}
                    Ok(Some(Ok(()))) => {
                        let mut sh = shared.borrow_mut();
                        if sh.dma.is_some() {
                            // Filled: to the app as it is.
                            if let Some(b) = sh.filling.take() {
                                sh.seq += 1;
                                let seq = sh.seq;
                                // SAFETY: dequeued from this stream, not given back yet.
                                unsafe {
                                    stamp(b, seq);
                                    if sh.cursor_meta {
                                        put_cursor(b, &mut sh);
                                    }
                                }
                                drop(sh);
                                // SAFETY: as above.
                                unsafe { stream_of().queue_raw_buffer(b) };
                            }
                        } else {
                            sh.fresh = true;
                            drop(sh);
                        }
                        // (With no borrow held: this may process at once.)
                        let _ = stream_of().trigger_process();
                    }
                    Ok(Some(Err(e))) => {
                        let mut sh = shared.borrow_mut();
                        if let Some(new) = sh.capture.size().filter(|&n| n != sh.size) {
                            sh.size = new;
                            let modifiers = sh.modifiers.clone();
                            drop(sh);
                            offer(stream_of(), new, modifiers.as_deref());
                        } else {
                            tracing::debug!("portal: {e}");
                        }
                    }
                    // app_server went (a reload): the share waits for it.
                    Err(e) => {
                        tracing::info!("portal: lost app_server ({e}); reconnecting");
                        shared.borrow_mut().lost = true;
                        if let Some(l) = weak.upgrade() {
                            l.quit();
                        }
                    }
                }
            }
        });
        mainloop.run();
        drop(app_server);
        if stop.load(Ordering::Relaxed) || !shared.borrow().lost {
            break;
        }
        // Back as it was once app_server is (a reload takes a second or
        // two), at its new size if that changed; the app sees a pause.
        let Some(capture) = reconnect(&source, cursor, &stop) else { break };
        let new = capture.size();
        fd = capture.fd().ok();
        let mut sh = shared.borrow_mut();
        // (A new capture follows the cursor anew: its picture is sent again.)
        sh.cursor_sent = 0;
        // The stream's dmabufs, known to the new connection.
        for slot in &mut sh.slots {
            if let Kind::Dma { bo, wl, .. } = &mut slot.kind {
                match capture.import(bo) {
                    Ok(new) => *wl = new,
                    Err(e) => tracing::warn!("portal: {e}"),
                }
            }
        }
        sh.capture = capture;
        sh.lost = false;
        sh.stopped_at = None;
        if let Some(new) = new.filter(|&n| n != sh.size) {
            sh.size = new;
            let modifiers = sh.modifiers.clone();
            drop(sh);
            offer(&stream, new, modifiers.as_deref());
        }
    }
    let _ = stream.disconnect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_dmabufs_then_memory() {
        let pod = |b: &Vec<u8>| pw::spa::pod::Pod::from_bytes(b).unwrap().as_bytes().to_vec();
        let read = |b: &Vec<u8>| modifier_of(pw::spa::pod::Pod::from_bytes(&pod(b)).unwrap());
        // Several modifiers: for the app to say which it takes, then memory.
        let offered = formats(640, 400, Some(&[0x0c00_0000_0000_0002, 0]));
        assert_eq!(offered.len(), 2);
        assert_eq!(read(&offered[0]), Some(Err(vec![0x0c00_0000_0000_0002, 0])));
        assert_eq!(read(&offered[1]), None);
        // One, chosen.
        let chosen = formats(640, 400, Some(&[0x0c00_0000_0000_0002]));
        assert_eq!(read(&chosen[0]), Some(Ok(0x0c00_0000_0000_0002)));
        // No GPU: memory only.
        let memory = formats(640, 400, None);
        assert_eq!(memory.len(), 1);
        assert_eq!(read(&memory[0]), None);
        // The buffers each way, and the header.
        assert!(buffers(640, 400, Some(1)).is_some() && buffers(640, 400, None).is_some() && header_meta().is_some() && cursor_param().is_some());
    }
}
