//! Screen capture for programs (ext-image-copy-capture): a frame a client
//! asks for of an output waits in `State::pending_captures` and is taken
//! with that output's next frame, drawn as a whole-screen screenshot is
//! (`render::capture`, offscreen and read back) and copied into the
//! client's shared-memory buffer in its format. (Dmabufs, drawn into
//! straight, come next.) Outside programs never see the protocol
//! (wlproxy hides it); Olo's screen-sharing portal reaches it on the
//! direct socket.

use smithay::{
    output::{Output, WeakOutput},
    reexports::wayland_server::protocol::wl_shm,
    utils::Transform,
    wayland::{
        image_copy_capture::{CaptureFailureReason, Frame},
        shm::{BufferData, with_buffer_contents_mut},
    },
};

/// A frame a client asked for, and the output it is of.
pub struct Pending {
    pub output: WeakOutput,
    pub frame: Frame,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending").field("output", &self.output.upgrade().map(|o| o.name())).finish_non_exhaustive()
    }
}

/// The frames waiting for `output`'s next frame (those of gone outputs
/// fail).
pub fn take(pending: &mut Vec<Pending>, output: &Output) -> Vec<Frame> {
    let (mine, rest): (Vec<Pending>, Vec<Pending>) = std::mem::take(pending).into_iter().partition(|p| p.output.upgrade().is_none_or(|o| o == *output));
    *pending = rest;
    mine.into_iter()
        .filter_map(|p| match p.output.upgrade() {
            Some(_) => Some(p.frame),
            None => {
                p.frame.fail(CaptureFailureReason::Unknown);
                None
            }
        })
        .collect()
}

/// Copy `image` (straight RGBA) into `frame`'s buffer and say it's done;
/// a buffer we can't fill (not shared memory, another size or format)
/// fails the frame, and the client asks again with what it is told.
pub fn fill(frame: Frame, image: &image::RgbaImage, presented: std::time::Duration) {
    let buffer = frame.buffer();
    let done = with_buffer_contents_mut(&buffer, |ptr, len, data: BufferData| {
        let (w, h) = (image.width() as i32, image.height() as i32);
        if data.width != w || data.height != h || !matches!(data.format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888) {
            return false;
        }
        let stride = data.stride as usize;
        let start = data.offset as usize;
        if start + stride * h as usize > len {
            return false;
        }
        // SAFETY: the pool is mapped for `len` bytes, checked above.
        let out = unsafe { std::slice::from_raw_parts_mut(ptr.add(start), stride * h as usize) };
        for (y, row) in image.rows().enumerate() {
            let line = &mut out[y * stride..y * stride + w as usize * 4];
            // (Argb8888 and Xrgb8888 are B, G, R, A in memory.)
            for (px, p) in line.chunks_exact_mut(4).zip(row) {
                px.copy_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        true
    });
    match done {
        Ok(true) => frame.success(Transform::Normal, None, presented),
        _ => frame.fail(CaptureFailureReason::BufferConstraints),
    }
}
