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

use std::{io::Write, sync::Arc, thread::JoinHandle};

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
    let mut clip: Clip = vec![
        (FILES[0].into(), gnome.into_bytes().into()),
        (FILES[1].into(), list.into_bytes().into()),
    ];
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
    let conn = Connection::connect_to_env().map_err(|e| format!("no display: {e}"))?;
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
    use std::io::Read as _;
    let conn = Connection::connect_to_env().map_err(|e| format!("no display: {e}"))?;
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

#[derive(Default)]
struct Reader {
    types: std::collections::HashMap<wayland_client::backend::ObjectId, Vec<String>>,
    selection: Option<ExtDataControlOfferV1>,
}

impl Dispatch<ExtDataControlDeviceV1, ()> for Reader {
    fn event(reader: &mut Self, _: &ExtDataControlDeviceV1, event: ext_data_control_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_data_control_device_v1::Event::Selection { id } = event {
            reader.selection = id;
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
            ext_data_control_device_v1::Event::Selection { id: Some(offer) } | ext_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => {
                offer.destroy()
            }
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

