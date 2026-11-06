//! `olo-portal share-test`: ask xdg-desktop-portal for a screen as an app
//! does (CreateSession, SelectSources, Start, each answered by a Response
//! signal on its request; then OpenPipeWireRemote), and read the stream
//! it hands back through the PipeWire connection it gives (which sees
//! only that stream), writing the first frame. It checks the whole path
//! an app's share takes: the portal, the picker, the stream, PipeWire's
//! permissions.

use std::{collections::HashMap, os::fd::OwnedFd};

use pipewire as pw;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const DESKTOP: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST: &str = "org.freedesktop.portal.ScreenCast";

/// Write BGRx rows as a PNG.
pub fn write_png(out: &str, w: u32, h: u32, stride: usize, bgrx: &[u8]) -> Result<(), String> {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in bgrx.chunks(stride).take(h as usize) {
        for px in row.chunks_exact(4).take(w as usize) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    let file = std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut wr| wr.write_image_data(&rgba)).map_err(|e| e.to_string())
}

/// Call `method` and wait for its request's Response: the results, or why
/// not.
fn call(conn: &zbus::blocking::Connection, method: &str, token: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> Result<HashMap<String, OwnedValue>, String> {
    // The request's path is known before the call: listen first.
    let sender = conn.unique_name().ok_or("no bus name")?.trim_start_matches(':').replace('.', "_");
    let request = format!("{PATH}/request/{sender}/{token}");
    let proxy = zbus::blocking::Proxy::new(conn, DESKTOP, request.as_str(), "org.freedesktop.portal.Request").map_err(|e| e.to_string())?;
    let mut responses = proxy.receive_signal("Response").map_err(|e| e.to_string())?;
    let portal = zbus::blocking::Proxy::new(conn, DESKTOP, PATH, SCREENCAST).map_err(|e| e.to_string())?;
    let _: OwnedObjectPath = portal.call(method, body).map_err(|e| format!("{method}: {e}"))?;
    let message = responses.next().ok_or("no response")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize().map_err(|e| e.to_string())?;
    match code {
        0 => Ok(results),
        1 => Err(format!("{method}: cancelled")),
        _ => Err(format!("{method}: failed")),
    }
}

fn opts(pairs: &[(&str, Value<'static>)]) -> HashMap<String, OwnedValue> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), OwnedValue::try_from(v.clone()).unwrap())).collect()
}

pub fn share_test(out: &str, bus: Option<&str>, hold: u64, types: u32, modifiers: Option<Vec<u64>>) -> Result<(), String> {
    let conn = match bus {
        Some(a) => zbus::blocking::connection::Builder::address(a).and_then(|b| b.build()),
        None => zbus::blocking::Connection::session(),
    }
    .map_err(|e| format!("no D-Bus: {e}"))?;
    let results = call(&conn, "CreateSession", "olo1", &(opts(&[("handle_token", "olo1".into()), ("session_handle_token", "olos".into())]),))?;
    let session: String = results.get("session_handle").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).ok_or("no session")?;
    let session = OwnedObjectPath::try_from(session).map_err(|e| e.to_string())?;
    println!("session {}", session.as_str());
    call(&conn, "SelectSources", "olo2", &(&session, opts(&[("handle_token", "olo2".into()), ("types", types.into()), ("cursor_mode", 2u32.into())])))?;
    let results = call(&conn, "Start", "olo3", &(&session, "", opts(&[("handle_token", "olo3".into())])))?;
    let streams = results.get("streams").ok_or("no streams")?;
    let streams: Vec<(u32, HashMap<String, OwnedValue>)> = streams.try_clone().map_err(|e| e.to_string())?.try_into().map_err(|e: zbus::zvariant::Error| e.to_string())?;
    let (node, props) = streams.first().ok_or("no streams")?;
    println!("stream: node {node}, {props:?}");
    let portal = zbus::blocking::Proxy::new(&conn, DESKTOP, PATH, SCREENCAST).map_err(|e| e.to_string())?;
    let fd: zbus::zvariant::OwnedFd = portal.call("OpenPipeWireRemote", &(&session, HashMap::<String, OwnedValue>::new())).map_err(|e| format!("OpenPipeWireRemote: {e}"))?;
    // Kept open (as an app sharing is) for `hold` seconds after the first
    // frame, or until the portal closes it (Stop Sharing).
    let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if hold > 0 {
        let sess = zbus::blocking::Proxy::new(&conn, DESKTOP, session.as_str(), "org.freedesktop.portal.Session").map_err(|e| e.to_string())?;
        let mut signals = sess.receive_signal("Closed").map_err(|e| e.to_string())?;
        let closed = closed.clone();
        std::thread::spawn(move || {
            if signals.next().is_some() {
                closed.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }
    let frame = read_frames(Some(OwnedFd::from(fd)), *node, std::time::Duration::from_secs(hold), closed.clone(), modifiers.as_deref())?;
    write_png(out, frame.0, frame.1, frame.2, &frame.3)?;
    println!("wrote {out} ({}×{})", frame.0, frame.1);
    if hold > 0 {
        if closed.load(std::sync::atomic::Ordering::Relaxed) {
            println!("the portal closed the session");
        } else {
            println!("held for {hold} s");
        }
    }
    Ok(())
}

/// Read `node`'s frames from this computer's PipeWire, as an app does:
/// the first written to `out`. With `modifiers`, it takes dmabufs with
/// one of them (as Chrome and OBS do), read back through the GPU.
pub fn read_test(node: u32, out: &str, modifiers: Option<Vec<u64>>, hold: u64) -> Result<(), String> {
    let frame = read_frames(None, node, std::time::Duration::from_secs(hold), Default::default(), modifiers.as_deref())?;
    write_png(out, frame.0, frame.1, frame.2, &frame.3)?;
    println!("wrote {out} ({}×{})", frame.0, frame.1);
    Ok(())
}

/// Read `node`'s stream for `secs` seconds asking for the cursor apart
/// (its metadata), printing each place and picture that comes and how
/// many buffers came with only the cursor: for trying the share's cursor
/// mode `metadata` (`stream-test - SECS --cursor metadata`).
pub fn cursor_test(node: u32, secs: u64) -> Result<(), String> {
    use pw::spa::{param::format::FormatProperties as P, pod, sys, utils};
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| format!("can't reach PipeWire: {e}"))?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "olo-portal cursor-test",
        pw::properties::properties! { *pw::keys::MEDIA_TYPE => "Video", *pw::keys::MEDIA_CATEGORY => "Capture", *pw::keys::MEDIA_ROLE => "Screen" },
    )
    .map_err(|e| e.to_string())?;
    #[derive(Default)]
    struct Seen {
        frames: u32,
        cursor_only: u32,
        last: Option<(i32, i32)>,
        pictures: u32,
    }
    let seen: std::rc::Rc<std::cell::RefCell<Seen>> = Default::default();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |s, _, id, param| {
            if param.is_none() || id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            // The cursor's metadata, as an app asks for it.
            let size = std::mem::size_of::<sys::spa_meta_cursor>() + std::mem::size_of::<sys::spa_meta_bitmap>() + 256 * 256 * 4;
            let meta = pod::Value::Object(pod::Object {
                type_: utils::SpaTypes::ObjectParamMeta.as_raw(),
                id: pw::spa::param::ParamType::Meta.as_raw(),
                properties: vec![
                    pod::Property::new(sys::SPA_PARAM_META_type, pod::Value::Id(utils::Id(sys::SPA_META_Cursor))),
                    pod::Property::new(sys::SPA_PARAM_META_size, pod::Value::Int(size as i32)),
                ],
            });
            let Ok(bytes) = pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &meta).map(|b| b.0.into_inner()) else { return };
            let mut params: Vec<&pod::Pod> = pod::Pod::from_bytes(&bytes).into_iter().collect();
            let _ = s.update_params(&mut params);
        })
        .process({
            let seen = seen.clone();
            move |s, _| {
                // SAFETY: a dequeued buffer, given back below; its metas and
                // first data's chunk are PipeWire's.
                unsafe {
                    let b = s.dequeue_raw_buffer();
                    if b.is_null() {
                        return;
                    }
                    let buffer = (*b).buffer;
                    let mut seen = seen.borrow_mut();
                    let d = &*(*buffer).datas;
                    if (*d.chunk).size == 0 {
                        seen.cursor_only += 1;
                    } else {
                        seen.frames += 1;
                    }
                    if !(*buffer).metas.is_null() {
                        for m in std::slice::from_raw_parts((*buffer).metas, (*buffer).n_metas as usize) {
                            if m.type_ != sys::SPA_META_Cursor || m.data.is_null() {
                                continue;
                            }
                            let c = &*m.data.cast::<sys::spa_meta_cursor>();
                            if c.id == 0 {
                                continue;
                            }
                            let at = (c.position.x, c.position.y);
                            if seen.last != Some(at) {
                                println!("cursor at {at:?}, hotspot ({}, {})", c.hotspot.x, c.hotspot.y);
                                seen.last = Some(at);
                            }
                            if c.bitmap_offset != 0 {
                                let bm = &*m.data.cast::<u8>().add(c.bitmap_offset as usize).cast::<sys::spa_meta_bitmap>();
                                let px = std::slice::from_raw_parts(m.data.cast::<u8>().add(c.bitmap_offset as usize + bm.offset as usize), (bm.size.width * bm.size.height * 4) as usize);
                                let opaque = px.chunks_exact(4).filter(|p| p[3] > 0).count();
                                println!("cursor picture {}×{} ({opaque} pixels drawn)", bm.size.width, bm.size.height);
                                seen.pictures += 1;
                            }
                        }
                    }
                    drop(seen);
                    s.queue_raw_buffer(b);
                }
            }
        })
        .register()
        .map_err(|e| e.to_string())?;
    let format = pod::object!(
        utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pod::property!(P::MediaType, Id, pw::spa::param::format::MediaType::Video),
        pod::property!(P::MediaSubtype, Id, pw::spa::param::format::MediaSubtype::Raw),
        pod::property!(P::VideoFormat, Choice, Enum, Id, pw::spa::param::video::VideoFormat::BGRx, pw::spa::param::video::VideoFormat::BGRx, pw::spa::param::video::VideoFormat::BGRA),
        pod::property!(
            P::VideoSize,
            Choice,
            Range,
            Rectangle,
            utils::Rectangle { width: 1920, height: 1080 },
            utils::Rectangle { width: 1, height: 1 },
            utils::Rectangle { width: 16384, height: 16384 }
        ),
        pod::property!(P::VideoFramerate, Choice, Range, Fraction, utils::Fraction { num: 30, denom: 1 }, utils::Fraction { num: 0, denom: 1 }, utils::Fraction { num: 240, denom: 1 }),
    );
    let offered = pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod::Value::Object(format)).map_err(|e| format!("{e:?}"))?.0.into_inner();
    let mut params: Vec<&pod::Pod> = pod::Pod::from_bytes(&offered).into_iter().collect();
    stream.connect(pw::spa::utils::Direction::Input, Some(node), pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params).map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    let timer = mainloop.loop_().add_timer({
        let weak = mainloop.downgrade();
        move |_| {
            if started.elapsed() >= std::time::Duration::from_secs(secs)
                && let Some(l) = weak.upgrade()
            {
                l.quit();
            }
        }
    });
    let _ = timer.update_timer(Some(std::time::Duration::from_millis(100)), Some(std::time::Duration::from_millis(100)));
    mainloop.run();
    let seen = seen.borrow();
    println!("{} frames, {} buffers with only the cursor, {} cursor pictures", seen.frames, seen.cursor_only, seen.pictures);
    Ok(())
}

/// The first render node's allocator, to read dmabufs back with.
fn render_node() -> Result<gbm::Device<std::fs::File>, String> {
    let mut nodes: Vec<_> =
        std::fs::read_dir("/dev/dri").map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD"))).collect();
    nodes.sort();
    let file = std::fs::OpenOptions::new().read(true).write(true).open(nodes.first().ok_or("no render node")?).map_err(|e| e.to_string())?;
    gbm::Device::new(file).map_err(|e| e.to_string())
}

/// A frame that came as a dmabuf, read back: its rows, `w`×4 bytes each.
fn read_dmabuf(gbm: &gbm::Device<std::fs::File>, d: &pw::spa::buffer::Data, (w, h): (u32, u32), modifier: u64) -> Result<Vec<u8>, String> {
    // SAFETY: the buffer's descriptor, PipeWire's, open while it's dequeued.
    let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(d.fd()) };
    let (stride, offset) = (d.chunk().stride(), d.chunk().offset() as i32);
    let bo = gbm
        .import_buffer_object_from_dma_buf_with_modifiers::<()>(
            1,
            [Some(fd), None, None, None],
            w,
            h,
            gbm::Format::Xrgb8888,
            gbm::BufferObjectFlags::empty(),
            [stride, 0, 0, 0],
            [offset, 0, 0, 0],
            gbm::Modifier::from(modifier),
        )
        .map_err(|e| format!("can't import the dmabuf: {e}"))?;
    let row = w as usize * 4;
    let mut out = vec![0u8; row * h as usize];
    bo.map(0, 0, w, h, |m| {
        for (src, dst) in m.buffer().chunks(m.stride() as usize).zip(out.chunks_mut(row)) {
            dst.copy_from_slice(&src[..row]);
        }
    })
    .map_err(|e| format!("can't read the dmabuf: {e}"))?;
    Ok(out)
}

/// The first frame of `node`, through the PipeWire connection `fd` (else
/// this computer's); then frames go on being read for `hold` (or until
/// `closed`), saying when the format changes and how many came each
/// second. With `modifiers`, dmabufs are asked for first.
fn read_frames(
    fd: Option<OwnedFd>,
    node: u32,
    hold: std::time::Duration,
    closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    modifiers: Option<&[u64]>,
) -> Result<(u32, u32, usize, Vec<u8>), String> {
    use pw::spa::{param::format::FormatProperties as P, pod, utils};
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = match fd {
        Some(fd) => context.connect_fd_rc(fd, None),
        None => context.connect_rc(None),
    }
    .map_err(|e| format!("can't reach PipeWire: {e}"))?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "olo-portal share-test",
        pw::properties::properties! { *pw::keys::MEDIA_TYPE => "Video", *pw::keys::MEDIA_CATEGORY => "Capture", *pw::keys::MEDIA_ROLE => "Screen" },
    )
    .map_err(|e| e.to_string())?;
    let size = std::rc::Rc::new(std::cell::Cell::new((0u32, 0u32)));
    let modifier = std::rc::Rc::new(std::cell::Cell::new(None::<u64>));
    let frames = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let got: std::rc::Rc<std::cell::RefCell<Option<(u32, u32, usize, Vec<u8>)>>> = Default::default();
    let gbm = if modifiers.is_some() { Some(render_node()?) } else { None };
    let weak = mainloop.downgrade();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed({
            let (size, modifier) = (size.clone(), modifier.clone());
            move |_, _, id, param| {
                let Some(param) = param.filter(|_| id == pw::spa::param::ParamType::Format.as_raw()) else { return };
                let mut info = pw::spa::param::video::VideoInfoRaw::new();
                if info.parse(param).is_ok() {
                    size.set((info.size().width, info.size().height));
                    let dma = info.flags().bits() & pw::spa::sys::SPA_VIDEO_FLAG_MODIFIER != 0;
                    modifier.set(dma.then(|| info.modifier()));
                    match modifier.get() {
                        Some(m) => println!("format {}×{} as dmabufs (modifier {m:#x})", info.size().width, info.size().height),
                        None => println!("format {}×{}", info.size().width, info.size().height),
                    }
                }
            }
        })
        .process({
            let (size, got, frames, modifier) = (size.clone(), got.clone(), frames.clone(), modifier.clone());
            move |s, _| {
                let Some(mut buffer) = s.dequeue_buffer() else { return };
                let datas = buffer.datas_mut();
                let Some(d) = datas.first_mut() else { return };
                let (stride, len) = (d.chunk().stride().max(0) as usize, d.chunk().size() as usize);
                if len == 0 {
                    return;
                }
                frames.set(frames.get() + 1);
                if got.borrow().is_some() {
                    return;
                }
                let (w, h) = size.get();
                if d.type_() == pw::spa::buffer::DataType::DmaBuf {
                    let read = match (&gbm, modifier.get()) {
                        (Some(gbm), Some(m)) => read_dmabuf(gbm, d, (w, h), m),
                        _ => Err("a dmabuf not asked for".into()),
                    };
                    match read {
                        Ok(rows) => {
                            println!("the first frame came as a dmabuf");
                            *got.borrow_mut() = Some((w, h, w as usize * 4, rows));
                        }
                        Err(e) => println!("{e}"),
                    }
                } else if let Some(bytes) = d.data() {
                    println!("the first frame came in memory");
                    *got.borrow_mut() = Some((w, h, stride, bytes[..len.min(bytes.len())].to_vec()));
                }
                if got.borrow().is_some()
                    && hold.is_zero()
                    && let Some(l) = weak.upgrade()
                {
                    l.quit();
                }
            }
        })
        .register()
        .map_err(|e| e.to_string())?;
    let mut offered = Vec::new();
    if let Some(modifiers) = modifiers.filter(|m| !m.is_empty()) {
        use pw::spa::{
            pod::{ChoiceValue, Property, PropertyFlags, Value},
            utils::{Choice, ChoiceEnum, ChoiceFlags},
        };
        let mut format = pod::object!(
            utils::SpaTypes::ObjectParamFormat,
            pw::spa::param::ParamType::EnumFormat,
            pod::property!(P::MediaType, Id, pw::spa::param::format::MediaType::Video),
            pod::property!(P::MediaSubtype, Id, pw::spa::param::format::MediaSubtype::Raw),
            pod::property!(P::VideoFormat, Id, pw::spa::param::video::VideoFormat::BGRx),
            pod::property!(
                P::VideoSize,
                Choice,
                Range,
                Rectangle,
                utils::Rectangle { width: 1920, height: 1080 },
                utils::Rectangle { width: 1, height: 1 },
                utils::Rectangle { width: 16384, height: 16384 }
            ),
            pod::property!(P::VideoFramerate, Choice, Range, Fraction, utils::Fraction { num: 30, denom: 1 }, utils::Fraction { num: 0, denom: 1 }, utils::Fraction { num: 240, denom: 1 }),
        );
        format.properties.push(Property {
            key: P::VideoModifier.as_raw(),
            flags: PropertyFlags::MANDATORY | PropertyFlags::DONT_FIXATE,
            value: Value::Choice(ChoiceValue::Long(Choice(ChoiceFlags::empty(), ChoiceEnum::Enum { default: modifiers[0] as i64, alternatives: modifiers.iter().map(|&m| m as i64).collect() }))),
        });
        offered.push(pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod::Value::Object(format)).map_err(|e| format!("{e:?}"))?.0.into_inner());
    }
    let format = pod::object!(
        utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pod::property!(P::MediaType, Id, pw::spa::param::format::MediaType::Video),
        pod::property!(P::MediaSubtype, Id, pw::spa::param::format::MediaSubtype::Raw),
        pod::property!(P::VideoFormat, Choice, Enum, Id, pw::spa::param::video::VideoFormat::BGRx, pw::spa::param::video::VideoFormat::BGRx, pw::spa::param::video::VideoFormat::BGRA),
        pod::property!(
            P::VideoSize,
            Choice,
            Range,
            Rectangle,
            utils::Rectangle { width: 1920, height: 1080 },
            utils::Rectangle { width: 1, height: 1 },
            utils::Rectangle { width: 16384, height: 16384 }
        ),
        pod::property!(P::VideoFramerate, Choice, Range, Fraction, utils::Fraction { num: 30, denom: 1 }, utils::Fraction { num: 0, denom: 1 }, utils::Fraction { num: 240, denom: 1 }),
    );
    offered.push(pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod::Value::Object(format)).map_err(|e| format!("{e:?}"))?.0.into_inner());
    let mut params: Vec<&pod::Pod> = offered.iter().filter_map(|b| pod::Pod::from_bytes(b)).collect();
    stream.connect(pw::spa::utils::Direction::Input, Some(node), pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params).map_err(|e| e.to_string())?;
    // Each second: how many came; the end once held long enough (or
    // closed), or given up on after 20 s with nothing.
    let started = std::time::Instant::now();
    let first: std::rc::Rc<std::cell::Cell<Option<std::time::Instant>>> = Default::default();
    let timer = mainloop.loop_().add_timer({
        let weak = mainloop.downgrade();
        let (got, frames, size, first) = (got.clone(), frames.clone(), size.clone(), first.clone());
        move |_| {
            if got.borrow().is_some() && first.get().is_none() {
                first.set(Some(std::time::Instant::now()));
            }
            let n = frames.replace(0);
            if n > 0 && !hold.is_zero() {
                println!("{n} frames ({}×{})", size.get().0, size.get().1);
            }
            let done = match first.get() {
                Some(t) => t.elapsed() >= hold || closed.load(std::sync::atomic::Ordering::Relaxed),
                None => started.elapsed() > std::time::Duration::from_secs(20),
            };
            if done && let Some(l) = weak.upgrade() {
                l.quit();
            }
        }
    });
    let _ = timer.update_timer(Some(std::time::Duration::from_secs(1)), Some(std::time::Duration::from_secs(1)));
    mainloop.run();
    let _ = stream.disconnect();
    got.borrow_mut().take().ok_or_else(|| "no frame came".into())
}
