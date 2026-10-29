//! Big textures, uploaded off the UI thread.
//!
//! Copying an image's pixels for the GPU costs about a millisecond for
//! every 4 MB (a 6000 × 4000 photo: 23 ms, three frames at 120 Hz). So a
//! texture of [`STAGE_BYTES`] and up (a 1024 × 1024 image), new or set
//! again, is uploaded on a worker: until it is in, frames show the old
//! pixels or, for a new texture, nothing. [`uploading`] says whether that
//! is so; show something smaller meanwhile (olo-preview shows the
//! image's next smaller level that is in). The font atlas is always
//! uploaded at once.

use std::{collections::HashSet, sync::Arc};

use egui::TextureId;

/// Textures this big and up are uploaded on a worker.
pub const STAGE_BYTES: usize = 4 << 20;

/// Whether texture `id` is waiting for its first upload from the worker:
/// a frame drawing it would show nothing.
pub fn uploading(ctx: &egui::Context, id: TextureId) -> bool {
    let big = ctx.tex_manager().read().meta(id).is_some_and(|m| m.bytes_used() >= STAGE_BYTES);
    big && !ctx.data(|d| d.get_temp::<Arc<HashSet<TextureId>>>(key()).is_some_and(|s| s.contains(&id)))
}

/// Whether the kit uploads this delta on the worker: a whole image, not
/// the font atlas, of `STAGE_BYTES` and up.
pub(crate) fn staged(id: TextureId, delta: &egui::epaint::ImageDelta) -> bool {
    id != TextureId::default() && delta.pos.is_none() && delta.image.bytes_per_pixel() * delta.image.width() * delta.image.height() >= STAGE_BYTES
}

/// Texture `id` has its pixels on the GPU (`true`), or is gone.
pub(crate) fn set_uploaded(ctx: &egui::Context, id: TextureId, uploaded: bool) {
    ctx.data_mut(|d| {
        let set = d.get_temp_mut_or_default::<Arc<HashSet<TextureId>>>(key());
        if set.contains(&id) != uploaded {
            let set = Arc::make_mut(set);
            if uploaded {
                set.insert(id);
            } else {
                set.remove(&id);
            }
        }
    });
}

fn key() -> egui::Id {
    egui::Id::unique("olo-uploaded-textures")
}
