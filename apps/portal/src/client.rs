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

pub fn share_test(out: &str, bus: Option<&str>, hold: u64) -> Result<(), String> {
    let conn = match bus {
        Some(a) => zbus::blocking::connection::Builder::address(a).and_then(|b| b.build()),
        None => zbus::blocking::Connection::session(),
    }
    .map_err(|e| format!("no D-Bus: {e}"))?;
    let results = call(&conn, "CreateSession", "olo1", &(opts(&[("handle_token", "olo1".into()), ("session_handle_token", "olos".into())]),))?;
    let session: String = results.get("session_handle").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).ok_or("no session")?;
    let session = OwnedObjectPath::try_from(session).map_err(|e| e.to_string())?;
    println!("session {}", session.as_str());
    call(&conn, "SelectSources", "olo2", &(&session, opts(&[("handle_token", "olo2".into()), ("types", 1u32.into()), ("cursor_mode", 2u32.into())])))?;
    let results = call(&conn, "Start", "olo3", &(&session, "", opts(&[("handle_token", "olo3".into())])))?;
    let streams = results.get("streams").ok_or("no streams")?;
    let streams: Vec<(u32, HashMap<String, OwnedValue>)> = streams.try_clone().map_err(|e| e.to_string())?.try_into().map_err(|e: zbus::zvariant::Error| e.to_string())?;
    let (node, props) = streams.first().ok_or("no streams")?;
    println!("stream: node {node}, {props:?}");
    let portal = zbus::blocking::Proxy::new(&conn, DESKTOP, PATH, SCREENCAST).map_err(|e| e.to_string())?;
    let fd: zbus::zvariant::OwnedFd = portal.call("OpenPipeWireRemote", &(&session, HashMap::<String, OwnedValue>::new())).map_err(|e| format!("OpenPipeWireRemote: {e}"))?;
    let frame = read_frame(OwnedFd::from(fd), *node)?;
    write_png(out, frame.0, frame.1, frame.2, &frame.3)?;
    println!("wrote {out} ({}×{})", frame.0, frame.1);
    if hold > 0 {
        // Kept open (as an app sharing is) until then, or until the portal
        // closes it (Stop Sharing).
        let sess = zbus::blocking::Proxy::new(&conn, DESKTOP, session.as_str(), "org.freedesktop.portal.Session").map_err(|e| e.to_string())?;
        let mut closed = sess.receive_signal("Closed").map_err(|e| e.to_string())?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if closed.next().is_some() {
                let _ = tx.send(());
            }
        });
        match rx.recv_timeout(std::time::Duration::from_secs(hold)) {
            Ok(()) => println!("the portal closed the session"),
            Err(_) => println!("held for {hold} s"),
        }
    }
    Ok(())
}

/// The first frame of `node`, through the PipeWire connection `fd`.
fn read_frame(fd: OwnedFd, node: u32) -> Result<(u32, u32, usize, Vec<u8>), String> {
    use pw::spa::{param::format::FormatProperties as P, pod, utils};
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_fd_rc(fd, None).map_err(|e| format!("can't reach PipeWire: {e}"))?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "olo-portal share-test",
        pw::properties::properties! { *pw::keys::MEDIA_TYPE => "Video", *pw::keys::MEDIA_CATEGORY => "Capture", *pw::keys::MEDIA_ROLE => "Screen" },
    )
    .map_err(|e| e.to_string())?;
    let size = std::rc::Rc::new(std::cell::Cell::new((0u32, 0u32)));
    let got: std::rc::Rc<std::cell::RefCell<Option<(u32, u32, usize, Vec<u8>)>>> = Default::default();
    let weak = mainloop.downgrade();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed({
            let size = size.clone();
            move |_, _, id, param| {
                let Some(param) = param.filter(|_| id == pw::spa::param::ParamType::Format.as_raw()) else { return };
                let mut info = pw::spa::param::video::VideoInfoRaw::new();
                if info.parse(param).is_ok() {
                    size.set((info.size().width, info.size().height));
                }
            }
        })
        .process({
            let (size, got) = (size.clone(), got.clone());
            move |s, _| {
                let Some(mut buffer) = s.dequeue_buffer() else { return };
                let datas = buffer.datas_mut();
                let Some(d) = datas.first_mut() else { return };
                let (stride, len) = (d.chunk().stride().max(0) as usize, d.chunk().size() as usize);
                if len == 0 || got.borrow().is_some() {
                    return;
                }
                if let Some(bytes) = d.data() {
                    let (w, h) = size.get();
                    *got.borrow_mut() = Some((w, h, stride, bytes[..len.min(bytes.len())].to_vec()));
                    if let Some(l) = weak.upgrade() {
                        l.quit();
                    }
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
        pod::property!(P::VideoSize, Choice, Range, Rectangle, utils::Rectangle { width: 1920, height: 1080 }, utils::Rectangle { width: 1, height: 1 }, utils::Rectangle { width: 16384, height: 16384 }),
        pod::property!(P::VideoFramerate, Choice, Range, Fraction, utils::Fraction { num: 30, denom: 1 }, utils::Fraction { num: 0, denom: 1 }, utils::Fraction { num: 240, denom: 1 }),
    );
    let bytes = pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod::Value::Object(format)).map_err(|e| format!("{e:?}"))?.0.into_inner();
    let mut params = [pod::Pod::from_bytes(&bytes).ok_or("bad format")?];
    stream
        .connect(pw::spa::utils::Direction::Input, Some(node), pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)
        .map_err(|e| e.to_string())?;
    // Give up after a while.
    let timer = mainloop.loop_().add_timer({
        let weak = mainloop.downgrade();
        move |_| {
            if let Some(l) = weak.upgrade() {
                l.quit();
            }
        }
    });
    let _ = timer.update_timer(Some(std::time::Duration::from_secs(20)), None);
    mainloop.run();
    let _ = stream.disconnect();
    got.borrow_mut().take().ok_or_else(|| "no frame came".into())
}
