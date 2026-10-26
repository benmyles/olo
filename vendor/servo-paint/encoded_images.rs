/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Patched in for Tabula: still pictures that pages hand over encoded
//! ([`SerializableImageData::Encoded`]) rather than as pixels.
//!
//! WebRender would keep a CPU copy of every picture's pixels for as long
//! as its page lives, so a page of a hundred photos would hold a hundred
//! bitmaps here (and as many again in the page's process). Instead each
//! one is an external buffer image: we keep its file, and decode it only
//! when WebRender asks for its pixels, which it does only to upload the
//! picture to its texture cache for drawing. Decoded pixels stay in a
//! budget shared by every page and the least recently drawn go first; a
//! picture scrolled back into view is decoded again.
//!
//! Decoding never holds up drawing: a picture WebRender asks for before
//! it is decoded is drawn clear, decoded on a worker, then updated (as
//! browsers do with pictures that come into view).

use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use euclid::default::Size2D;
use log::warn;
use paint_api::{
    ExternalImageSource, ImageUpdate, PaintMessage, PaintProxy, SerializableImageData,
    WebRenderExternalImageApi, WebRenderExternalImageIdManager, WebRenderImageHandlerType,
};
use rustc_hash::FxHashMap;
use servo_base::id::PainterId;
use smallvec::smallvec;
use webrender_api::{
    ExternalImageData, ExternalImageId, ExternalImageType, ImageDescriptor, ImageKey,
};

/// Decoded pixels kept for every page together.
const BUDGET: usize = 128 << 20;

/// Pictures decoded at once.
const DECODERS: usize = 2;

#[derive(Clone, Copy, PartialEq)]
enum Decoding {
    No,
    Busy,
    Failed,
}

struct Picture {
    painter: PainterId,
    key: ImageKey,
    descriptor: ImageDescriptor,
    encoded: Arc<Vec<u8>>,
    pixels: Option<Arc<Vec<u8>>>,
    decoding: Decoding,
    /// When WebRender last asked for it (a tick of `Store::clock`).
    used: u64,
}

#[derive(Default)]
struct Store {
    pictures: FxHashMap<ExternalImageId, Picture>,
    by_key: FxHashMap<ImageKey, ExternalImageId>,
    /// Bytes of decoded pixels held.
    decoded: usize,
    clock: u64,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(Default::default);

static DECODER_POOL: LazyLock<Option<rayon::ThreadPool>> = LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(DECODERS)
        .thread_name(|i| format!("ImageDecode#{i}"))
        .build()
        .inspect_err(|error| warn!("No picture decoders: {error}"))
        .ok()
});

fn store() -> MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn external(id: ExternalImageId) -> ExternalImageData {
    ExternalImageData {
        id,
        channel_index: 0,
        image_type: ExternalImageType::Buffer,
        normalized_uvs: false,
    }
}

fn byte_len(descriptor: &ImageDescriptor) -> usize {
    descriptor.compute_total_size().max(0) as usize
}

/// Take a page's encoded picture: the image data to give WebRender for it.
pub(crate) fn add(
    painter: PainterId,
    proxy: &PaintProxy,
    ids: &mut WebRenderExternalImageIdManager,
    key: ImageKey,
    descriptor: ImageDescriptor,
    encoded: Arc<Vec<u8>>,
) -> ExternalImageData {
    let id = ids.next_id(WebRenderImageHandlerType::Image);
    let mut store = store();
    if let Some(old) = store.by_key.insert(key, id) {
        forget(&mut store, ids, old);
    }
    // The first screenful is usually what was just added: decode while
    // there's room, so it's ready when first drawn.
    let prefetch = store.decoded + byte_len(&descriptor) <= BUDGET / 2;
    store.pictures.insert(
        id,
        Picture {
            painter,
            key,
            descriptor,
            encoded,
            pixels: None,
            decoding: if prefetch { Decoding::Busy } else { Decoding::No },
            used: 0,
        },
    );
    drop(store);
    if prefetch {
        decode(id, proxy.clone());
    }
    external(id)
}

/// The page let go of `key`.
pub(crate) fn remove(ids: &mut WebRenderExternalImageIdManager, key: ImageKey) {
    let mut store = store();
    if let Some(id) = store.by_key.remove(&key) {
        forget(&mut store, ids, id);
    }
}

fn forget(store: &mut Store, ids: &mut WebRenderExternalImageIdManager, id: ExternalImageId) {
    if let Some(picture) = store.pictures.remove(&id) {
        if picture.pixels.is_some() {
            store.decoded -= byte_len(&picture.descriptor);
        }
    }
    ids.remove(&id);
}

/// Whether `data` is one of ours (an update after decoding, which needs a
/// frame drawn to show it).
pub(crate) fn is_ours(ids: &WebRenderExternalImageIdManager, data: &SerializableImageData) -> bool {
    matches!(data, SerializableImageData::External(e)
        if matches!(ids.get(&e.id), Some(WebRenderImageHandlerType::Image)))
}

/// Decode picture `id` on a worker, then have its painter show it.
fn decode(id: ExternalImageId, proxy: PaintProxy) {
    let job = move || {
        let Some(encoded) = store().pictures.get(&id).map(|p| p.encoded.clone()) else {
            return;
        };
        let decoded = pixels::decode_deferred_for_webrender(&encoded);
        let mut store = store();
        store.clock += 1;
        let clock = store.clock;
        let Some(picture) = store.pictures.get_mut(&id) else {
            return;
        };
        let size = picture.descriptor.size;
        let len = byte_len(&picture.descriptor);
        match decoded {
            Some((w, h, pixels))
                if w as i32 == size.width && h as i32 == size.height && pixels.len() >= len =>
            {
                picture.pixels = Some(pixels);
                picture.decoding = Decoding::No;
                picture.used = picture.used.max(clock);
            },
            _ => {
                warn!("Couldn't decode a {}×{} picture", size.width, size.height);
                picture.decoding = Decoding::Failed;
                return;
            },
        }
        let update = ImageUpdate::UpdateImage(
            picture.key,
            picture.descriptor,
            SerializableImageData::External(external(id)),
            None,
        );
        let painter = picture.painter;
        store.decoded += len;
        evict(&mut store, id);
        drop(store);
        proxy.send(PaintMessage::UpdateImages(painter, smallvec![update]));
    };
    match &*DECODER_POOL {
        Some(pool) => pool.spawn(job),
        None => job(),
    }
}

/// Drop the least recently drawn pixels until within the budget (never
/// `keep`'s, just decoded).
fn evict(store: &mut Store, keep: ExternalImageId) {
    while store.decoded > BUDGET {
        let oldest = store
            .pictures
            .iter()
            .filter(|(id, p)| **id != keep && p.pixels.is_some())
            .min_by_key(|(_, p)| p.used)
            .map(|(id, _)| *id);
        let Some(oldest) = oldest else { break };
        let Some(picture) = store.pictures.get_mut(&oldest) else { break };
        picture.pixels = None;
        store.decoded -= byte_len(&picture.descriptor);
    }
}

/// WebRender's way in: pixels for the pictures it uploads.
pub(crate) struct EncodedImages {
    proxy: PaintProxy,
    /// Pixels handed out and not yet given back, with how many times.
    locked: FxHashMap<u64, (Arc<Vec<u8>>, usize)>,
    /// Clear pixels for pictures not decoded yet.
    blank: Arc<Vec<u8>>,
}

impl EncodedImages {
    pub(crate) fn new(proxy: PaintProxy) -> Self {
        Self {
            proxy,
            locked: Default::default(),
            blank: Default::default(),
        }
    }
}

impl WebRenderExternalImageApi for EncodedImages {
    fn lock(&mut self, id: u64) -> (ExternalImageSource<'_>, Size2D<i32>) {
        let mut store = store();
        store.clock += 1;
        let clock = store.clock;
        let Some(picture) = store.pictures.get_mut(&ExternalImageId(id)) else {
            return (ExternalImageSource::Invalid, Size2D::zero());
        };
        picture.used = clock;
        let size = Size2D::new(picture.descriptor.size.width, picture.descriptor.size.height);
        let len = byte_len(&picture.descriptor);
        let pixels = match &picture.pixels {
            Some(pixels) => pixels.clone(),
            None => {
                if picture.decoding == Decoding::No {
                    picture.decoding = Decoding::Busy;
                    decode(ExternalImageId(id), self.proxy.clone());
                }
                if self.blank.len() < len {
                    self.blank = Arc::new(vec![0; len]);
                }
                self.blank.clone()
            },
        };
        drop(store);
        let entry = self.locked.entry(id).or_insert((pixels.clone(), 0));
        entry.1 += 1;
        if entry.1 == 1 {
            entry.0 = pixels;
        }
        let data = &entry.0[..len.min(entry.0.len())];
        (ExternalImageSource::RawData(data), size)
    }

    fn unlock(&mut self, id: u64) {
        if let Some(entry) = self.locked.get_mut(&id) {
            entry.1 -= 1;
            if entry.1 == 0 {
                self.locked.remove(&id);
            }
        }
    }
}
