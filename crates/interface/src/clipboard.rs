//! The clipboard.
//!
//! What's copied is offered to other programs by the display thread;
//! what another program copied is read on a worker, when it's asked for
//! (that program writes it when it gets round to it, or never, if it's
//! stuck), and handed back in a message. Nothing here blocks: whether
//! there's text to paste ([`has_text`]) is as the display thread last
//! heard, kept where any thread reads it.

use olo_sync::RwLock;

use crate::{
    ClipboardItem, ClipboardReceipt, ImageFormat, LinuxClient,
    display::{OnDisplay, display},
};

/// The types text is offered as.
const TEXT_TYPES: [&str; 2] = ["text/plain;charset=utf-8", "UTF8_STRING"];

/// What's on the clipboard, as the display thread last heard.
#[derive(Default)]
struct Offer {
    types: Vec<String>,
    /// This program's own copy, if it's what's there.
    own: Option<ClipboardItem>,
}

static OFFER: RwLock<Offer> = RwLock::new(Offer { types: Vec::new(), own: None });

/// What's on the clipboard is looked at again (on the display thread:
/// it changed, or this program put something there).
pub(crate) fn refresh(on: &OnDisplay) {
    let (types, own) = match on.client.wayland() {
        Some(client) => (client.clipboard_types(), client.own_clipboard()),
        // (No display: this program's own copy is all there is.)
        None => {
            let own = OFFER.read().unwrap_or_else(|e| e.into_inner()).own.clone();
            (if own.as_ref().is_some_and(|o| o.text().is_some()) { TEXT_TYPES.iter().map(|t| (*t).to_owned()).collect() } else { Vec::new() }, own)
        }
    };
    *OFFER.write().unwrap_or_else(|e| e.into_inner()) = Offer { types, own };
}

/// Puts `item` on the clipboard.
pub fn write(item: ClipboardItem) {
    display().post(move |on| {
        match on.client.wayland() {
            Some(client) => client.write_to_clipboard(item),
            None => OFFER.write().unwrap_or_else(|e| e.into_inner()).own = Some(item),
        }
        refresh(on);
    });
}

/// Takes `item` off the clipboard if it's still there and this
/// program's (nothing else copied since).
pub fn withdraw(item: ClipboardItem) {
    display().post(move |on| {
        if let Some(client) = on.client.wayland() {
            client.withdraw_clipboard(&item);
        }
        refresh(on);
    });
}

/// The MIME types what's on the clipboard is offered as (to enable a
/// Paste of files, a picture or text).
pub fn types() -> Vec<String> {
    OFFER.read().unwrap_or_else(|e| e.into_inner()).types.clone()
}

/// Whether there's text to paste (without reading it).
pub fn has_text() -> bool {
    let offer = OFFER.read().unwrap_or_else(|e| e.into_inner());
    match &offer.own {
        Some(own) => own.text().is_some(),
        None => offer.types.iter().any(|t| TEXT_TYPES.contains(&t.as_str())),
    }
}

/// What's on the clipboard (its text, else a picture; this program's own
/// copy whole, as it was copied), handed to `then` when it has come:
/// another program's is read on a worker, so one slow to give it, or
/// that never does, holds nothing up. `then` runs on some other thread:
/// have it send a message.
pub fn paste_item(then: impl FnOnce(Option<ClipboardItem>) + Send + 'static) {
    display().post(move |on| {
        let Some(client) = on.client.wayland() else {
            then(OFFER.read().unwrap_or_else(|e| e.into_inner()).own.clone());
            return;
        };
        if let Some(own) = client.own_clipboard() {
            then(Some(own));
            return;
        }
        let types = client.clipboard_types();
        let offered = |mime: &str| types.iter().any(|t| t == mime);
        let (mime, image) = if let Some(text) = TEXT_TYPES.into_iter().find(|t| offered(t)) {
            (text.to_owned(), None)
        } else if let Some(format) = <ImageFormat as strum::IntoEnumIterator>::iter().find(|f| offered(f.mime_type())) {
            (format.mime_type().to_owned(), Some(format))
        } else {
            then(None);
            return;
        };
        let make = move |bytes: Vec<u8>| match image {
            // (Windows' line ends made Unix's.)
            None => String::from_utf8(bytes).ok().map(|t| ClipboardItem::new_string(t.replace("\r\n", "\n"))),
            Some(format) => Some(ClipboardItem::new_image(&crate::Image::from_bytes(format, bytes))),
        };
        match client.receive_clipboard(&mime) {
            None => then(None),
            Some(ClipboardReceipt::Ready(bytes)) => then(make(bytes)),
            Some(ClipboardReceipt::Read(read)) => olo_app::pool::run(move || then(read().and_then(make))).detach(),
        }
    });
}

/// What's on the clipboard as the first of `wanted` it's offered as (one
/// ending in `/`, as `image/`, stands for any of that kind): its type
/// and data, handed to `then` when it has come (as [`paste_item`]). None
/// if it has none of them.
pub fn paste_as(wanted: Vec<String>, then: impl FnOnce(Option<(String, Vec<u8>)>) + Send + 'static) {
    display().post(move |on| {
        let Some(client) = on.client.wayland() else {
            then(None);
            return;
        };
        let types = client.clipboard_types();
        let found = wanted.iter().find_map(|w| types.iter().find(|t| if w.ends_with('/') { t.starts_with(w.as_str()) } else { *t == w }));
        let Some(mime) = found.cloned() else {
            then(None);
            return;
        };
        match client.receive_clipboard(&mime) {
            None => then(None),
            Some(ClipboardReceipt::Ready(bytes)) => then(Some((mime, bytes))),
            Some(ClipboardReceipt::Read(read)) => olo_app::pool::run(move || then(read().map(|bytes| (mime, bytes)))).detach(),
        }
    });
}
