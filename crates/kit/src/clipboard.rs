//! The clipboard for a program with no window (a screenshot taken by a
//! shortcut): [`hold`] puts something on it and serves it until something
//! else is copied. An app with windows copies with [`Cx::copy`](crate::Cx::copy).
//!
//! The usual way to set the clipboard (`wl_data_device`) is only for the
//! client with the keyboard, and a program with no window never has it,
//! so this goes through `ext_data_control_v1`, which app_server offers
//! every client (as clipboard tools use it).
//!
//! ```ignore
//! println!("{}", path.display());
//! olo_kit::clipboard::hold(olo_kit::clipboard::text(&path.display().to_string()))?;
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

use crate::Clip;

/// `text` in the forms text is pasted as (what [`Cx::copy`](crate::Cx::copy) offers).
pub fn text(text: &str) -> Clip {
    let data: Arc<[u8]> = text.as_bytes().into();
    ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"].iter().map(|t| (t.to_string(), data.clone())).collect()
}

/// Put `clip` on the clipboard and serve it to whoever pastes it, until
/// something else is copied (or the session ends): this returns then.
pub fn hold(clip: Clip) -> Result<(), String> {
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
