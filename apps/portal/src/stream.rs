//! A screen as a PipeWire video source, for the portal's ScreenCast. One
//! thread runs PipeWire's loop, which also reads app_server (the capture's
//! connection is one of its sources): while the stream streams, a frame is
//! asked for at most at the rate the app agreed to; app_server answers
//! once the screen has changed (so a still screen sends nothing); and the
//! stream, its own driver, is then given it, copied straight from the
//! capture's buffer into PipeWire's (BGRx rows, in memory PipeWire maps
//! for the app). `run` returns once PipeWire has given the stream a node,
//! whose id Start hands the app.

use std::{
    cell::RefCell,
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

use crate::capture::{Capture, Source};

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

/// Start sharing `source` (a screen, or a window), with the
/// pointer drawn in if `cursor`. `lost` is called if the share ends
/// by itself, not stopped (app_server never came back).
pub fn run(source: Source, cursor: bool, lost: impl FnOnce() + Send + 'static) -> Result<Share, String> {
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

/// What the loop's sources share.
struct Shared {
    capture: Capture,
    /// The size the stream was offered.
    size: (u32, u32),
    /// A frame came that the stream hasn't had.
    fresh: bool,
    streaming: bool,
    /// How often frames may come, as agreed.
    every: Duration,
    asked: std::time::Instant,
    /// app_server went away: waiting for it to come back.
    lost: bool,
}

/// Offer the stream at a new size (the app renegotiates).
fn offer(stream: &pw::stream::Stream, (w, h): (u32, u32)) {
    if let Ok(bytes) = format(w, h)
        && let Some(pod) = pw::spa::pod::Pod::from_bytes(&bytes)
    {
        let _ = stream.update_params(&mut [pod]);
    }
}

/// A capture of `source` again, once app_server is back: tried every
/// quarter second for half a minute (or until the share stops).
fn reconnect(source: &Source, cursor: bool, stop: &AtomicBool) -> Option<Capture> {
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        match Capture::new(source, cursor) {
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

/// The stream's one format: BGRx at the screen's size, as often as it
/// changes up to 60 times a second.
fn format(w: u32, h: u32) -> Result<Vec<u8>, String> {
    use pw::spa::{param::format::FormatProperties as P, pod, utils};
    let object = pod::object!(
        utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pod::property!(P::MediaType, Id, pw::spa::param::format::MediaType::Video),
        pod::property!(P::MediaSubtype, Id, pw::spa::param::format::MediaSubtype::Raw),
        pod::property!(P::VideoFormat, Id, pw::spa::param::video::VideoFormat::BGRx),
        pod::property!(P::VideoSize, Rectangle, utils::Rectangle { width: w, height: h }),
        pod::property!(P::VideoFramerate, Fraction, utils::Fraction { num: 0, denom: 1 }),
        pod::property!(P::VideoMaxFramerate, Choice, Range, Fraction, utils::Fraction { num: 60, denom: 1 }, utils::Fraction { num: 1, denom: 1 }, utils::Fraction { num: 60, denom: 1 }),
    );
    pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod::Value::Object(object)).map(|r| r.0.into_inner()).map_err(|e| format!("{e:?}"))
}

/// The buffers, once the format is agreed: one plane of a frame, in
/// memory PipeWire maps for the app (a memfd, else plain memory).
fn buffers(w: u32, h: u32) -> Option<Vec<u8>> {
    use pw::spa::{
        pod::{ChoiceValue, Object, Property, Value},
        sys,
        utils::{Choice, ChoiceEnum, ChoiceFlags},
    };
    let stride = w * 4;
    let object = Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: pw::spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            Property::new(sys::SPA_PARAM_BUFFERS_buffers, Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default: 4, min: 2, max: 8 })))),
            Property::new(sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            Property::new(sys::SPA_PARAM_BUFFERS_size, Value::Int((stride * h) as i32)),
            Property::new(sys::SPA_PARAM_BUFFERS_stride, Value::Int(stride as i32)),
            // (A choice of flags, as PipeWire reads it.)
            Property::new(
                sys::SPA_PARAM_BUFFERS_dataType,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags { default: 1 << sys::SPA_DATA_MemFd, flags: vec![1 << sys::SPA_DATA_MemFd, 1 << sys::SPA_DATA_MemPtr] },
                ))),
            ),
        ],
    };
    pw::spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object)).map(|r| r.0.into_inner()).ok()
}

fn stream(source: Source, cursor: bool, stop: Arc<AtomicBool>, tx: &mpsc::Sender<Result<(u32, (u32, u32)), String>>) -> Result<(), String> {
    let capture = Capture::new(&source, cursor)?;
    // A screen by its name from now on (the first, if none was named).
    let source = match source {
        Source::Output(_) => Source::Output(Some(capture.name().to_owned())),
        window => window,
    };
    let size = capture.size().ok_or("app_server gave no size")?;
    let fd = capture.fd()?;
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
    let shared = Rc::new(RefCell::new(Shared { capture, size, fresh: false, streaming: false, every: Duration::from_millis(16), asked: std::time::Instant::now(), lost: false }));
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
                shared.borrow_mut().streaming = matches!(new, pw::stream::StreamState::Streaming);
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
                if rate.num > 0 {
                    shared.borrow_mut().every = Duration::from_secs_f64(rate.denom as f64 / rate.num as f64);
                }
                if let Some(bytes) = buffers(size.width, size.height)
                    && let Some(pod) = pw::spa::pod::Pod::from_bytes(&bytes)
                {
                    let _ = s.update_params(&mut [pod]);
                }
            }
        })
        .process({
            let shared = shared.clone();
            move |s, _| {
                let mut shared = shared.borrow_mut();
                if !std::mem::take(&mut shared.fresh) {
                    return;
                }
                // (A frame of another size than the stream's, while it is
                // offered anew, isn't given.)
                if shared.capture.frame_size() != Some(shared.size) {
                    return;
                }
                let Some(mut buffer) = s.dequeue_buffer() else { return };
                let (w, h) = shared.size;
                let datas = buffer.datas_mut();
                let Some(d) = datas.first_mut() else { return };
                let stride = w as usize * 4;
                let copied = d.data().map(|out| shared.capture.copy_into(out, stride));
                let chunk = d.chunk_mut();
                match copied {
                    Some(Ok(())) => {
                        *chunk.offset_mut() = 0;
                        *chunk.stride_mut() = stride as i32;
                        *chunk.size_mut() = (stride * h as usize) as u32;
                    }
                    other => {
                        if let Some(Err(e)) = other {
                            tracing::warn!("portal: {e}");
                        }
                        *chunk.size_mut() = 0;
                    }
                }
            }
        })
        .register()
        .map_err(|e| e.to_string())?;
    let bytes = format(size.0, size.1)?;
    let mut params = [pw::spa::pod::Pod::from_bytes(&bytes).ok_or("bad format")?];
    stream
        .connect(spa::utils::Direction::Output, None, pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)
        .map_err(|e| e.to_string())?;
    // (Addresses: the loop's callbacks must be 'static, and the stream
    // outlives the loop that runs them.)
    let stream_at = &*stream as *const pw::stream::Stream as usize;
    // SAFETY: as above.
    let stream_of = move || unsafe { &*(stream_at as *const pw::stream::Stream) };
    // While streaming, the next frame is asked for once the last has gone
    // and its time has come; and the share ends when told.
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
            if !sh.lost && sh.streaming && !sh.fresh && !sh.capture.waiting() && sh.asked.elapsed() >= sh.every.saturating_sub(Duration::from_millis(2)) {
                sh.asked = std::time::Instant::now();
                if let Err(e) = sh.capture.request() {
                    tracing::warn!("portal: {e}");
                }
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
                match polled {
                    Ok(None) => {}
                    Ok(Some(Ok(()))) => {
                        shared.borrow_mut().fresh = true;
                        // (With no borrow held: this may process at once.)
                        let _ = stream_of().trigger_process();
                    }
                    Ok(Some(Err(e))) => {
                        let mut sh = shared.borrow_mut();
                        if let Some(new) = sh.capture.size().filter(|&n| n != sh.size) {
                            sh.size = new;
                            drop(sh);
                            offer(stream_of(), new);
                        } else {
                            tracing::debug!("portal: {e}");
                        }
                    }
                    // What it showed went (a window closed): the share ends.
                    Err(_) if shared.borrow().capture.stopped() => {
                        tracing::info!("portal: what was shared is gone");
                        if let Some(l) = weak.upgrade() {
                            l.quit();
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
        sh.capture = capture;
        sh.lost = false;
        if let Some(new) = new.filter(|&n| n != sh.size) {
            sh.size = new;
            drop(sh);
            offer(&stream, new);
        }
    }
    let _ = stream.disconnect();
    Ok(())
}
