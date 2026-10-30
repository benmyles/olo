//! Screen capture for programs (ext-image-copy-capture). Each session a
//! client keeps is a `Cast`; a frame it asks for waits in `Captures` until
//! its output's next frame (`take`) and is drawn then only if the screen
//! changed since the session's last one (its own damage tracker; the first
//! frame comes at once), so a still screen costs nothing. A frame in a
//! dmabuf (what Olo's portal gives) is drawn straight into it on the GPU
//! and told ready once the GPU is done, with nothing read back on the
//! loop. A shared-memory frame is drawn into the session's own texture
//! (only where it changed) and read back as BGRA rows, copied as they are:
//! the fallback, as it waits on the GPU and copies on the loop. Outside
//! programs never see the protocol (wlproxy hides it); Olo's portal reaches
//! it on the direct socket.

use std::time::Duration;

use smithay::{
    backend::{
        allocator::{Buffer as _, Fourcc, dmabuf::Dmabuf},
        renderer::{
            Bind, Color32F, ExportMem, Offscreen, Renderer, Texture, TextureMapping as _,
            damage::OutputDamageTracker,
            element::RenderElement,
            gles::GlesTexture,
            sync::SyncPoint,
        },
    },
    output::{Output, WeakOutput},
    reexports::wayland_server::protocol::wl_shm,
    utils::{Buffer as BufferCoords, Physical, Rectangle, Size, Transform},
    wayland::{
        dmabuf::get_dmabuf,
        image_copy_capture::{BufferConstraints, CaptureFailureReason, DmabufConstraints, Frame, Session, SessionRef},
        shm::{BufferData, with_buffer_contents, with_buffer_contents_mut},
    },
};

/// A client's capture session, and what it last saw.
struct Cast {
    session: Session,
    /// What changed since its last frame, at the size and scale it is of.
    damage: Option<(OutputDamageTracker, Size<i32, Physical>, f64)>,
    /// Whether it has had a frame: the first comes whatever changed.
    shown: bool,
    /// Its shared-memory frames, drawn here first (only where they
    /// changed) and read back.
    target: Option<(GlesTexture, OutputDamageTracker, Size<i32, Physical>, f64)>,
    /// The dmabuf it was last drawn into, which holds that frame unless the
    /// client damages it: drawn into again, only what changed is.
    drawn_into: Option<(smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer, OutputDamageTracker, Size<i32, Physical>, f64)>,
}

/// A frame drawn into a client's dmabuf, told once the GPU is done.
struct Drawn {
    frame: Frame,
    sync: SyncPoint,
    damage: Option<Vec<Rectangle<i32, BufferCoords>>>,
    presented: Duration,
}

#[derive(Default)]
pub struct Captures {
    casts: Vec<Cast>,
    /// Frames asked for, by session, waiting for their output's frame.
    pending: Vec<(SessionRef, Frame)>,
    drawn: Vec<Drawn>,
    /// The dmabufs app_server draws into: the render node and the formats
    /// it renders (None: shared memory only).
    pub dma: Option<DmabufConstraints>,
}

impl std::fmt::Debug for Captures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Captures").field("casts", &self.casts.len()).field("pending", &self.pending.len()).field("drawn", &self.drawn.len()).finish()
    }
}

/// The size a picture of `output` is, in pixels (its mode, turned as the
/// screen is).
pub fn size(output: &Output) -> Option<Size<i32, Physical>> {
    let mode = output.current_mode()?;
    Some(output.current_transform().transform_size(mode.size))
}

/// What a client's buffers for `output` must be.
pub fn constraints(output: &Output, dma: Option<DmabufConstraints>) -> Option<BufferConstraints> {
    let size = size(output)?;
    Some(BufferConstraints {
        size: (size.w, size.h).into(),
        shm: vec![wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888],
        dma,
    })
}

/// The formats app_server can draw captures into, from what its renderer
/// renders: BGRx and BGRA, with their modifiers.
pub fn dma_constraints(node: smithay::backend::drm::DrmNode, formats: &smithay::backend::allocator::format::FormatSet) -> Option<DmabufConstraints> {
    let formats: Vec<_> = [Fourcc::Xrgb8888, Fourcc::Argb8888]
        .into_iter()
        .map(|code| (code, formats.iter().filter(|f| f.code == code).map(|f| f.modifier).collect::<Vec<_>>()))
        .filter(|(_, m)| !m.is_empty())
        .collect();
    (!formats.is_empty()).then_some(DmabufConstraints { node, formats })
}

impl Captures {
    /// A new session, kept until the client ends it (dropping it would
    /// stop it).
    pub fn add(&mut self, session: Session) {
        self.casts.push(Cast { session, damage: None, shown: false, target: None, drawn_into: None });
    }

    pub fn remove(&mut self, session: &SessionRef) {
        self.casts.retain(|c| c.session.as_ref() != *session);
        self.pending.retain(|(s, _)| s != session);
    }

    /// A frame `session` asked for, drawn with its output's next frame.
    pub fn queue(&mut self, session: &SessionRef, frame: Frame) {
        if session.source().user_data().get::<WeakOutput>().is_none() {
            frame.fail(CaptureFailureReason::Unknown);
            return;
        }
        self.pending.push((session.clone(), frame));
    }

    /// Tell the frames whose drawing the GPU has finished.
    pub fn finish(&mut self) {
        let (done, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.drawn).into_iter().partition(|d| d.sync.is_reached());
        self.drawn = rest;
        for d in done {
            d.frame.success(Transform::Normal, d.damage, d.presented);
        }
    }

    /// Whether a frame of `output` is waiting (so the caller can skip
    /// building what it draws).
    pub fn wanted(&self, output: &Output) -> bool {
        self.pending.iter().any(|(s, _)| s.source().user_data().get::<WeakOutput>().and_then(|o| o.upgrade()).is_none_or(|o| o == *output))
    }

    /// Draw the frames waiting for `output` whose picture changed since
    /// their session's last. `elements(renderer, cursor)` is what a picture
    /// of it shows, with the pointer if `cursor`, and its background.
    pub fn take<R, E>(&mut self, renderer: &mut R, output: &Output, presented: Duration, mut elements: impl FnMut(&mut R, bool) -> (Vec<E>, Color32F))
    where
        R: Renderer + Bind<Dmabuf> + Offscreen<GlesTexture> + ExportMem,
        R::Error: std::fmt::Debug,
        R::TextureId: Texture,
        E: RenderElement<R>,
    {
        self.finish();
        if !self.wanted(output) {
            return;
        }
        let Some(size) = size(output) else { return };
        let scale = output.current_scale().fractional_scale();
        let mut built: [Option<(Vec<E>, Color32F)>; 2] = [None, None];
        let pending = std::mem::take(&mut self.pending);
        for (session, frame) in pending {
            let Some(target) = session.source().user_data().get::<WeakOutput>().cloned() else { continue };
            match target.upgrade() {
                None => {
                    frame.fail(CaptureFailureReason::Unknown);
                    continue;
                }
                Some(o) if o != *output => {
                    self.pending.push((session, frame));
                    continue;
                }
                Some(_) => {}
            }
            if frame.has_failed() {
                continue;
            }
            let Some(cast) = self.casts.iter_mut().find(|c| c.session.as_ref() == session) else {
                frame.fail(CaptureFailureReason::Stopped);
                continue;
            };
            // The screen changed size: the client must allocate again.
            if session.current_constraints().is_some_and(|c| c.size != (size.w, size.h).into()) {
                if let Some(c) = constraints(output, self.dma.clone()) {
                    session.update_constraints(c);
                }
                frame.fail(CaptureFailureReason::BufferConstraints);
                continue;
            }
            let cursor = session.draw_cursor();
            let (els, clear) = built[cursor as usize].get_or_insert_with(|| elements(renderer, cursor));
            // Only when something changed since its last frame.
            if cast.damage.as_ref().is_none_or(|(_, s, sc)| *s != size || *sc != scale) {
                cast.damage = Some((OutputDamageTracker::new(size, scale, Transform::Normal), size, scale));
            }
            let tracker = &mut cast.damage.as_mut().unwrap().0;
            let damage = match tracker.damage_output(1, els) {
                Ok((d, _)) => d.map(|d| d.iter().map(|r| Rectangle::new((r.loc.x, r.loc.y).into(), (r.size.w, r.size.h).into())).collect::<Vec<_>>()),
                Err(_) => {
                    frame.fail(CaptureFailureReason::Unknown);
                    continue;
                }
            };
            if cast.shown && damage.is_none() {
                self.pending.push((session, frame));
                continue;
            }
            let damage = if cast.shown { damage } else { None };
            cast.shown = true;
            match draw(renderer, cast, &frame, els, *clear, size, scale) {
                Ok(Some(sync)) => self.drawn.push(Drawn { frame, sync, damage, presented }),
                Ok(None) => frame.success(Transform::Normal, damage, presented),
                Err(e) => {
                    tracing::warn!("capture of {}: {e}", output.name());
                    frame.fail(e.reason());
                }
            }
        }
    }
}

enum DrawError {
    Constraints,
    Other(String),
}

impl DrawError {
    fn reason(&self) -> CaptureFailureReason {
        match self {
            DrawError::Constraints => CaptureFailureReason::BufferConstraints,
            DrawError::Other(_) => CaptureFailureReason::Unknown,
        }
    }
}

impl std::fmt::Display for DrawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DrawError::Constraints => f.write_str("the buffer isn't one we can draw into"),
            DrawError::Other(e) => f.write_str(e),
        }
    }
}

/// Draw `elements` into `frame`'s buffer: a dmabuf on the GPU (the sync
/// point says when it is done), shared memory through `cast`'s texture
/// (done on return).
fn draw<R, E>(renderer: &mut R, cast: &mut Cast, frame: &Frame, elements: &[E], clear: Color32F, size: Size<i32, Physical>, scale: f64) -> Result<Option<SyncPoint>, DrawError>
where
    R: Renderer + Bind<Dmabuf> + Offscreen<GlesTexture> + ExportMem,
    R::Error: std::fmt::Debug,
    R::TextureId: Texture,
    E: RenderElement<R>,
{
    let other = |e: &dyn std::fmt::Debug| DrawError::Other(format!("{e:?}"));
    let buffer = frame.buffer();
    if let Ok(dmabuf) = get_dmabuf(&buffer) {
        let mut dmabuf = dmabuf.clone();
        if dmabuf.width() != size.w as u32 || dmabuf.height() != size.h as u32 {
            return Err(DrawError::Constraints);
        }
        // The buffer it was last drawn into, untouched since: only what
        // changed is drawn. Another (buffers taking turns): all of it.
        let same = cast.drawn_into.as_ref().is_some_and(|(b, _, s, sc)| *b == buffer && *s == size && *sc == scale) && frame.damage().is_empty();
        if !same {
            cast.drawn_into = Some((buffer.clone(), OutputDamageTracker::new(size, scale, Transform::Normal), size, scale));
        }
        let tracker = &mut cast.drawn_into.as_mut().unwrap().1;
        let mut fb = renderer.bind(&mut dmabuf).map_err(|e| other(&e))?;
        let result = tracker.render_output(renderer, &mut fb, if same { 1 } else { 0 }, elements, clear).map_err(|e| other(&e))?;
        return Ok(Some(result.sync));
    }
    let data = with_buffer_contents(&buffer, |_, _, data: BufferData| data).map_err(|_| DrawError::Constraints)?;
    if data.width != size.w || data.height != size.h || !matches!(data.format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888) {
        return Err(DrawError::Constraints);
    }
    if cast.target.as_ref().is_none_or(|(_, _, s, sc)| *s != size || *sc != scale) {
        let texture = renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into()).map_err(|e| other(&e))?;
        cast.target = Some((texture, OutputDamageTracker::new(size, scale, Transform::Normal), size, scale));
    }
    let (texture, tracker, _, _) = cast.target.as_mut().unwrap();
    let region = Rectangle::from_size((size.w, size.h).into());
    let mapping = {
        let mut fb = renderer.bind(texture).map_err(|e| other(&e))?;
        // The texture holds its last frame: only what changed is drawn.
        tracker.render_output(renderer, &mut fb, 1, elements, clear).map_err(|e| other(&e))?;
        // BGRA as it is in memory, where the GPU can give it (no swizzle).
        match renderer.copy_framebuffer(&fb, region, Fourcc::Argb8888) {
            Ok(m) => (m, false),
            Err(_) => (renderer.copy_framebuffer(&fb, region, Fourcc::Abgr8888).map_err(|e| other(&e))?, true),
        }
    };
    let (mapping, swizzle) = mapping;
    let flipped = mapping.flipped();
    let pixels = renderer.map_texture(&mapping).map_err(|e| other(&e))?;
    let (w, h) = (size.w as usize, size.h as usize);
    let done = with_buffer_contents_mut(&buffer, |ptr, len, data: BufferData| {
        let (stride, start) = (data.stride as usize, data.offset as usize);
        if stride < w * 4 || start + stride * h > len || pixels.len() < w * h * 4 {
            return false;
        }
        // SAFETY: the pool is mapped for `len` bytes, checked above.
        let out = unsafe { std::slice::from_raw_parts_mut(ptr.add(start), stride * h) };
        for y in 0..h {
            // GL reads bottom-up unless the mapping says otherwise.
            let from = if flipped { y } else { h - 1 - y };
            let row = &pixels[from * w * 4..(from + 1) * w * 4];
            let line = &mut out[y * stride..y * stride + w * 4];
            if swizzle {
                for (px, p) in line.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                    px.copy_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            } else {
                line.copy_from_slice(row);
            }
        }
        true
    });
    match done {
        Ok(true) => Ok(None),
        _ => Err(DrawError::Constraints),
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::allocator::{Format, Fourcc, Modifier, format::FormatSet};

    #[test]
    fn dmabufs_of_bgrx_and_bgra_only() {
        let formats = FormatSet::from_iter([
            Format { code: Fourcc::Xrgb8888, modifier: Modifier::Linear },
            Format { code: Fourcc::Xrgb8888, modifier: Modifier::from(0x0300_0000_0000_0001) },
            Format { code: Fourcc::Abgr8888, modifier: Modifier::Linear },
        ]);
        let node = match smithay::backend::drm::DrmNode::from_dev_id(libc::makedev(226, 128)) {
            Ok(n) => n,
            // (No such device here: nothing to check against.)
            Err(_) => return,
        };
        let c = super::dma_constraints(node, &formats).unwrap();
        assert_eq!(c.formats.len(), 1);
        assert_eq!(c.formats[0].0, Fourcc::Xrgb8888);
        assert_eq!(c.formats[0].1.len(), 2);
        assert!(super::dma_constraints(node, &FormatSet::from_iter([Format { code: Fourcc::Abgr8888, modifier: Modifier::Linear }])).is_none());
    }
}
