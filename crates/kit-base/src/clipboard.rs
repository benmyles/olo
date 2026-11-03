//! The clipboard for a program with no window (a screenshot taken by a
//! shortcut, an app that has closed its windows): [`hold`] puts something
//! on it and serves it until something else is copied. An app with
//! windows copies through its kit.
//!
//! The usual way to set the clipboard (`wl_data_device`) is only for the
//! client with the keyboard, and a program with no window never has it,
//! so this goes through `ext_data_control_v1`, which app_server offers
//! every client (as clipboard tools use it).
//!
//! ```ignore
//! println!("{}", path.display());
//! olo_kit_base::clipboard::hold(olo_kit_base::clipboard::text(&path.display().to_string()))?;
//! ```

use std::{
    io::{Read as _, Write},
    os::fd::{AsFd as _, AsRawFd as _},
    sync::Arc,
    thread::JoinHandle,
};

use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::ExtDataControlOfferV1,
    ext_data_control_source_v1::{self, ExtDataControlSourceV1},
};

/// What goes on the clipboard: each form's MIME type and data.
pub type Clip = Vec<(String, Arc<[u8]>)>;

/// `text` in the forms text is pasted as (what [`Cx::copy`](crate::Cx::copy) offers).
pub fn text(text: &str) -> Clip {
    let data: Arc<[u8]> = text.as_bytes().into();
    ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"].iter().map(|t| (t.to_string(), data.clone())).collect()
}

/// Files on the clipboard, in the forms file managers paste first:
/// GNOME's (and Chrome's, Electron's, Qt's) `x-special/gnome-copied-files`,
/// then `text/uri-list` (`Cx::paste_data` takes the first it has).
pub const FILES: [&str; 2] = ["x-special/gnome-copied-files", "text/uri-list"];

/// `paths`, copied (or `cut`, to move on paste) as file managers do: the
/// forms of [`FILES`], KDE's cut mark, and the paths as text (for a
/// terminal or a text field).
pub fn files(paths: &[std::path::PathBuf], cut: bool) -> Clip {
    let uris: Vec<String> = paths.iter().map(|p| crate::files::file_uri(p)).collect();
    let gnome = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
    let list: String = uris.iter().map(|u| format!("{u}\r\n")).collect();
    let text = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
    let mut clip: Clip = vec![(FILES[0].into(), gnome.into_bytes().into()), (FILES[1].into(), list.into_bytes().into())];
    if cut {
        clip.push(("application/x-kde-cutselection".into(), Arc::from(&b"1"[..])));
    }
    let text: Arc<[u8]> = text.into_bytes().into();
    clip.extend(["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"].iter().map(|t| (t.to_string(), text.clone())));
    clip
}

/// The files in clipboard data of one of [`FILES`] (`mime`): their local
/// paths, and whether they were cut. None if it names none here.
pub fn copied_files(mime: &str, data: &[u8]) -> Option<(Vec<std::path::PathBuf>, bool)> {
    let text = String::from_utf8_lossy(data);
    let (cut, list) = match mime {
        "x-special/gnome-copied-files" => {
            let (action, rest) = text.split_once('\n').unwrap_or((&text, ""));
            (action.trim() == "cut", rest.to_owned())
        }
        "text/uri-list" => (false, text.into_owned()),
        _ => return None,
    };
    let paths = crate::files::uri_list_paths(&list);
    (!paths.is_empty()).then_some((paths, cut))
}

/// Put `clip` on the clipboard and serve it to whoever pastes it, until
/// something else is copied (or the session ends): this returns then.
pub fn hold(clip: Clip) -> Result<(), String> {
    hold_then(clip, || {})
}

/// [`hold`], `ready` called once it's on the clipboard (the display has
/// taken it), before serving: `pbcopy`'s copy tells the command it's done.
pub fn hold_then(clip: Clip, ready: impl FnOnce()) -> Result<(), String> {
    let conn = connect(None)?;
    let (globals, mut queue) = registry_queue_init::<Holder>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ExtDataControlManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no clipboard for programs without a window".to_string())?;
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no seat".to_string())?;
    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    for (mime, _) in &clip {
        source.offer(mime.clone());
    }
    device.set_selection(Some(&source));
    let mut holder = Holder { clip, done: false, writers: Vec::new() };
    queue.roundtrip(&mut holder).map_err(|e| e.to_string())?;
    ready();
    while !holder.done {
        queue.blocking_dispatch(&mut holder).map_err(|e| e.to_string())?;
    }
    source.destroy();
    device.destroy();
    let _ = conn.flush();
    // What is being pasted still gets there.
    for w in holder.writers {
        let _ = w.join();
    }
    Ok(())
}

struct Holder {
    clip: Clip,
    done: bool,
    writers: Vec<JoinHandle<()>>,
}

/// What's on the clipboard, in the first of `types` it's offered as
/// (None: nothing, or none of them), read as a program with no window
/// can (`ext_data_control_v1`).
pub fn read(types: &[&str]) -> Result<Option<Vec<u8>>, String> {
    let conn = connect(None)?;
    let (globals, mut queue) = registry_queue_init::<Reader>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ExtDataControlManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no clipboard for programs without a window".to_string())?;
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no seat".to_string())?;
    let device = manager.get_data_device(&seat, &qh, ());
    // The device tells what's on the clipboard at once: its offer, the
    // offer's types, then the selection.
    let mut reader = Reader::default();
    queue.roundtrip(&mut reader).map_err(|e| e.to_string())?;
    let found = reader.selection.as_ref().and_then(|o| {
        let offered = reader.types.get(&wayland_client::Proxy::id(o)).cloned().unwrap_or_default();
        types.iter().find(|t| offered.iter().any(|o| o == *t)).map(|t| (o.clone(), t.to_string()))
    });
    let Some((offer, mime)) = found else {
        device.destroy();
        return Ok(None);
    };
    let (mut r, w) = std::io::pipe().map_err(|e| e.to_string())?;
    offer.receive(mime, std::os::fd::AsFd::as_fd(&w));
    conn.flush().map_err(|e| e.to_string())?;
    drop(w);
    let mut data = Vec::new();
    r.read_to_end(&mut data).map_err(|e| e.to_string())?;
    offer.destroy();
    device.destroy();
    let _ = conn.flush();
    Ok(Some(data))
}

/// Display `display`'s connection: a Wayland socket's name (in
/// `XDG_RUNTIME_DIR`) or its path; None, the environment's.
fn connect(display: Option<&str>) -> Result<Connection, String> {
    let Some(d) = display else { return Connection::connect_to_env().map_err(|e| format!("no display: {e}")) };
    let path = if d.starts_with('/') { std::path::PathBuf::from(d) } else { std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("no XDG_RUNTIME_DIR")?).join(d) };
    let stream = std::os::unix::net::UnixStream::connect(&path).map_err(|e| format!("no display at {}: {e}", path.display()))?;
    Connection::from_socket(stream).map_err(|e| e.to_string())
}

/// The most of one form of what's copied [`read_all`] and [`watch`] take
/// (a photo's PNG is a few MB).
const MOST: u64 = 64 << 20;

/// Every form `offer` is in (`types`), read whole.
fn receive_all(conn: &Connection, offer: &ExtDataControlOfferV1, types: &[String]) -> Clip {
    let mut clip = Clip::new();
    for mime in types {
        let Ok((r, w)) = std::io::pipe() else { continue };
        offer.receive(mime.clone(), w.as_fd());
        if conn.flush().is_err() {
            break;
        }
        drop(w);
        let mut data = Vec::new();
        if r.take(MOST).read_to_end(&mut data).is_ok() {
            clip.push((mime.clone(), data.into()));
        }
    }
    clip
}

/// What's on display `display`'s clipboard, every form of it (None:
/// nothing is): to copy it as it is somewhere else.
pub fn read_all(display: Option<&str>) -> Result<Option<Clip>, String> {
    let conn = connect(display)?;
    let (globals, mut queue) = registry_queue_init::<Reader>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ExtDataControlManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no clipboard for programs without a window".to_string())?;
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no seat".to_string())?;
    let device = manager.get_data_device(&seat, &qh, ());
    let mut reader = Reader::default();
    queue.roundtrip(&mut reader).map_err(|e| e.to_string())?;
    let clip = reader.selection.take().map(|offer| {
        let types = reader.types.get(&wayland_client::Proxy::id(&offer)).cloned().unwrap_or_default();
        let clip = receive_all(&conn, &offer, &types);
        offer.destroy();
        clip
    });
    device.destroy();
    let _ = conn.flush();
    Ok(clip.filter(|c| !c.is_empty()))
}

/// Waits for the connection to have something to read, or for `stop`
/// (a byte written to it: true; its other end closed: its fd taken out of
/// the wait, `stop` None). An error is the connection's.
fn wait<D>(conn: &Connection, queue: &mut wayland_client::EventQueue<D>, stop: &mut Option<std::io::PipeReader>) -> Result<bool, String> {
    conn.flush().map_err(|e| e.to_string())?;
    let Some(guard) = queue.prepare_read() else { return Ok(false) };
    let mut fds = vec![libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 }];
    if let Some(s) = stop {
        fds.push(libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLIN, revents: 0 });
    }
    // SAFETY: `fds` is a valid array of `fds.len()` pollfds.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
    if n < 0 {
        let e = std::io::Error::last_os_error();
        return if e.kind() == std::io::ErrorKind::Interrupted { Ok(false) } else { Err(e.to_string()) };
    }
    if let (Some(s), Some(f)) = (stop.as_mut(), fds.get(1))
        && f.revents != 0
    {
        let mut byte = [0u8];
        if matches!(s.read(&mut byte), Ok(1)) {
            return Ok(true);
        }
        *stop = None;
    }
    if fds[0].revents != 0 {
        match guard.read() {
            Ok(_) => {}
            Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(false)
}

/// Watching a display's clipboard ([`watch`]) until dropped.
pub struct Watch {
    _stop: std::io::PipeWriter,
}

/// Hand each thing copied on display `display` (None: the
/// environment's) to `copied`, every form of it, on a thread of its own,
/// as it's copied; what's there already too if `now`. Nothing is asked of
/// the display meanwhile: it says when something is copied.
pub fn watch(display: Option<&str>, now: bool, mut copied: impl FnMut(Clip) + Send + 'static) -> Result<Watch, String> {
    let conn = connect(display)?;
    let (globals, queue) = registry_queue_init::<Reader>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ExtDataControlManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no clipboard for programs without a window".to_string())?;
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no seat".to_string())?;
    let (stop_r, stop_w) = std::io::pipe().map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("clipboard-watch".into())
        .spawn(move || {
            let mut queue = queue;
            let device = manager.get_data_device(&seat, &qh, ());
            let mut reader = Reader::default();
            // (What's on it now comes first.)
            if queue.roundtrip(&mut reader).is_err() {
                return;
            }
            let mut first = true;
            let mut stop = Some(stop_r);
            loop {
                if let Some(offer) = reader.selection.take() {
                    if now || !first {
                        let types = reader.types.get(&wayland_client::Proxy::id(&offer)).cloned().unwrap_or_default();
                        let clip = receive_all(&conn, &offer, &types);
                        if !clip.is_empty() {
                            copied(clip);
                        }
                    }
                    reader.types.remove(&wayland_client::Proxy::id(&offer));
                    offer.destroy();
                }
                first = false;
                match wait(&conn, &mut queue, &mut stop) {
                    Ok(true) | Err(_) => break,
                    Ok(false) if stop.is_none() => break,
                    Ok(false) => {}
                }
                if queue.dispatch_pending(&mut reader).is_err() {
                    break;
                }
            }
            device.destroy();
            let _ = conn.flush();
        })
        .map_err(|e| e.to_string())?;
    Ok(Watch { _stop: stop_w })
}

/// Something put on a display's clipboard by [`hold_on`], served until
/// something else is copied there; dropped, it's taken off again (if
/// it's still what's copied), unless [`Held::keep`] said to leave it.
pub struct Held {
    stop: Option<std::io::PipeWriter>,
}

impl Held {
    /// Leave it on the clipboard, served until something else is copied.
    pub fn keep(mut self) {
        // (Closed without a byte: the holder carries on.)
        drop(self.stop.take());
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(mut s) = self.stop.take() {
            let _ = s.write_all(b"x");
        }
    }
}

/// [`hold`] on display `display` (None: the environment's), from a
/// thread of its own; returns once the display has it.
pub fn hold_on(display: Option<&str>, clip: Clip) -> Result<Held, String> {
    let conn = connect(display)?;
    let (globals, queue) = registry_queue_init::<Holder>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ExtDataControlManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no clipboard for programs without a window".to_string())?;
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|_| "the display has no seat".to_string())?;
    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    for (mime, _) in &clip {
        source.offer(mime.clone());
    }
    device.set_selection(Some(&source));
    let mut holder = Holder { clip, done: false, writers: Vec::new() };
    let mut queue = queue;
    queue.roundtrip(&mut holder).map_err(|e| e.to_string())?;
    let (stop_r, stop_w) = std::io::pipe().map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("clipboard-hold".into())
        .spawn(move || {
            let mut stop = Some(stop_r);
            let mut take_off = false;
            while !holder.done {
                match wait(&conn, &mut queue, &mut stop) {
                    Ok(true) => {
                        take_off = true;
                        break;
                    }
                    Err(_) => break,
                    Ok(false) => {}
                }
                if queue.dispatch_pending(&mut holder).is_err() {
                    break;
                }
            }
            // (Still ours: what was copied over it would have cancelled it.)
            if take_off && !holder.done {
                device.set_selection(None);
            }
            source.destroy();
            device.destroy();
            let _ = conn.flush();
            for w in holder.writers {
                let _ = w.join();
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Held { stop: Some(stop_w) })
}

#[derive(Default)]
struct Reader {
    types: std::collections::HashMap<wayland_client::backend::ObjectId, Vec<String>>,
    selection: Option<ExtDataControlOfferV1>,
}

impl Dispatch<ExtDataControlDeviceV1, ()> for Reader {
    fn event(reader: &mut Self, _: &ExtDataControlDeviceV1, event: ext_data_control_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // (An offer copied over before it was read, or the primary
        // selection's, isn't wanted: let go of it.)
        let gone = |reader: &mut Self, offer: ExtDataControlOfferV1| {
            reader.types.remove(&wayland_client::Proxy::id(&offer));
            offer.destroy();
        };
        match event {
            ext_data_control_device_v1::Event::Selection { id } => {
                if let Some(old) = std::mem::replace(&mut reader.selection, id) {
                    gone(reader, old);
                }
            }
            ext_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => gone(reader, offer),
            _ => {}
        }
    }

    wayland_client::event_created_child!(Reader, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for Reader {
    fn event(reader: &mut Self, offer: &ExtDataControlOfferV1, event: <ExtDataControlOfferV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            reader.types.entry(wayland_client::Proxy::id(offer)).or_default().push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlManagerV1, ()> for Reader {
    fn event(_: &mut Self, _: &ExtDataControlManagerV1, _: <ExtDataControlManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlSeat, ()> for Reader {
    fn event(_: &mut Self, _: &WlSeat, _: <WlSeat as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlRegistry, GlobalListContents> for Reader {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as wayland_client::Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtDataControlSourceV1, ()> for Holder {
    fn event(holder: &mut Self, _: &ExtDataControlSourceV1, event: ext_data_control_source_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                let Some((_, data)) = holder.clip.iter().find(|(t, _)| *t == mime_type).or(holder.clip.first()) else { return };
                let data = data.clone();
                // On a thread: the reader may be slow.
                holder.writers.retain(|w| !w.is_finished());
                holder.writers.push(std::thread::spawn(move || {
                    let _ = std::fs::File::from(fd).write_all(&data);
                }));
            }
            ext_data_control_source_v1::Event::Cancelled => holder.done = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for Holder {
    fn event(holder: &mut Self, _: &ExtDataControlDeviceV1, event: ext_data_control_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            // What is on the clipboard now (ours, at first): not wanted.
            ext_data_control_device_v1::Event::Selection { id: Some(offer) } | ext_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => offer.destroy(),
            ext_data_control_device_v1::Event::Finished => holder.done = true,
            _ => {}
        }
    }

    wayland_client::event_created_child!(Holder, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for Holder {
    fn event(_: &mut Self, _: &ExtDataControlOfferV1, _: <ExtDataControlOfferV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtDataControlManagerV1, ()> for Holder {
    fn event(_: &mut Self, _: &ExtDataControlManagerV1, _: <ExtDataControlManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlSeat, ()> for Holder {
    fn event(_: &mut Self, _: &WlSeat, _: <WlSeat as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlRegistry, GlobalListContents> for Holder {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as wayland_client::Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
