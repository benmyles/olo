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

use std::{collections::HashMap, time::Duration};

use smithay::{
    backend::{
        allocator::{Buffer as _, Fourcc, dmabuf::Dmabuf},
        renderer::{Bind, Color32F, ExportMem, Offscreen, Renderer, Texture, TextureMapping as _, damage::OutputDamageTracker, element::RenderElement, gles::GlesTexture, sync::SyncPoint},
    },
    output::{Output, WeakOutput},
    reexports::wayland_server::protocol::wl_shm,
    utils::{Buffer as BufferCoords, Physical, Rectangle, Size, Transform},
    wayland::{
        dmabuf::get_dmabuf,
        image_copy_capture::{BufferConstraints, CaptureFailureReason, CursorSession, CursorSessionRef, DmabufConstraints, Frame, Session, SessionRef},
        shm::{BufferData, with_buffer_contents, with_buffer_contents_mut},
    },
};

/// What a capture source shows: a screen, or one window (by its id; the
/// source was made from its handle in the foreign-toplevel list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Output,
    Window(u64),
}

/// In a window source's user data: which window.
pub struct WindowSource(pub u64);

/// The windows mostly on `output`, whose frames are taken with its frame;
/// none while the screen is locked (a window's share pauses on its last
/// frame: nothing but the lock screen is shown then).
pub fn windows_on(space: &smithay::desktop::Space<crate::shell::WindowElement>, output: &Output, locked: bool) -> Vec<u64> {
    if locked {
        return Vec::new();
    }
    space.elements().filter(|w| space.outputs_for_element(w).first() == Some(output)).map(|w| w.id()).collect()
}

/// What a session shows this frame: the screen (its own elements, made by
/// `screen`: the overlays, overview, background and pointer) or a window
/// alone (None: it went).
pub fn picture<'a, R>(
    renderer: &mut R,
    output: &Output,
    space: &smithay::desktop::Space<crate::shell::WindowElement>,
    parked: &[crate::shell::WindowElement],
    what: Target,
    locked: bool,
    screen: impl FnOnce() -> (
        Vec<crate::render::CustomRenderElements<R>>,
        Option<&'a crate::overview::Overview>,
        Color32F,
        Option<(smithay::utils::Point<f64, smithay::utils::Logical>, &'a smithay::input::pointer::CursorImageStatus)>,
    ),
) -> Option<Picture<crate::render::FrameElements<R, crate::shell::WindowRenderElement<R>>>>
where
    R: Renderer + smithay::backend::renderer::ImportAll + smithay::backend::renderer::ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    match what {
        Target::Output => {
            let (overlays, overview, color, pointer) = screen();
            let (elements, clear) = crate::render::capture_elements(renderer, output, space, overlays, overview, color, locked, pointer, true);
            Some(Picture { elements, clear, size: size(output)?, scale: output.current_scale().fractional_scale() })
        }
        Target::Window(id) => {
            let (mut elements, size, scale) = crate::render::window_picture(renderer, space, parked, output, id)?;
            // A private window's share shows it blacked out (`private.rs`).
            if let Some(shade) = space.elements().chain(parked.iter()).find(|w| w.id() == id).and_then(|w| crate::private::window_blackout(w, size)) {
                let shade = crate::render::FrameElements::Normal(crate::render::OutputRenderElements::Custom(crate::render::CustomRenderElements::Warm(shade)));
                return Some(Picture { elements: vec![shade], clear: Color32F::TRANSPARENT, size, scale });
            }
            // The pointer, over the window's content (a shown window's).
            let (_, _, _, pointer) = screen();
            if let Some((at, status)) = pointer
                && let Some(origin) = crate::render::content_origin(space, id)
            {
                let area = smithay::utils::Rectangle::new(origin.to_f64(), size.to_f64().to_logical(scale));
                let drawn = crate::render::captured_pointer(renderer, area, scale, at, status);
                elements.splice(0..0, drawn.into_iter().map(|e| crate::render::FrameElements::Normal(crate::render::OutputRenderElements::Custom(e))));
            }
            // A recording's caption, low on the window's picture.
            let caption = crate::showing::caption_elements(renderer, size, scale);
            elements.splice(0..0, caption.into_iter().map(|e| crate::render::FrameElements::Normal(crate::render::OutputRenderElements::Custom(crate::render::CustomRenderElements::Overlay(e)))));
            Some(Picture { elements, clear: Color32F::TRANSPARENT, size, scale })
        }
    }
}

/// What a session's source shows.
pub fn target(session: &SessionRef) -> Option<(Target, Option<Output>)> {
    let source = session.source();
    if let Some(WindowSource(id)) = source.user_data().get::<WindowSource>() {
        return Some((Target::Window(*id), None));
    }
    let output = source.user_data().get::<WeakOutput>()?.upgrade()?;
    Some((Target::Output, Some(output)))
}

/// The identifier window `id` has in the foreign-toplevel list (how the
/// portal finds a window Olo's IPC named).
pub fn toplevel_identifier(id: u64) -> String {
    format!("olo-{id}")
}

pub fn window_of_identifier(identifier: &str) -> Option<u64> {
    identifier.strip_prefix("olo-")?.parse().ok()
}

/// What a session is drawn from this frame: its elements and background,
/// and the picture's size and scale.
pub struct Picture<E> {
    pub elements: Vec<E>,
    pub clear: Color32F,
    pub size: Size<i32, Physical>,
    pub scale: f64,
}

/// A client's capture session, and what it last saw.
struct Cast {
    session: Session,
    /// What changed since its last frame, at the size and scale it is of.
    damage: Option<(OutputDamageTracker, Size<i32, Physical>, f64)>,
    /// Whether it has had a frame: the first comes whatever changed.
    shown: bool,
    targets: Targets,
}

/// Where a session's frames are drawn.
#[derive(Default)]
struct Targets {
    /// Its shared-memory frames, drawn here first (only where they
    /// changed) and read back.
    target: Option<(GlesTexture, OutputDamageTracker, Size<i32, Physical>, f64)>,
    /// The dmabuf it was last drawn into, which holds that frame unless the
    /// client damages it: drawn into again, only what changed is.
    drawn_into: Option<(smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer, OutputDamageTracker, Size<i32, Physical>, f64)>,
}

/// A client's cursor session on a screen: where the pointer is on it, and
/// its picture apart from the screen's (the portal's cursor sent as
/// metadata, for an app that draws it itself: a remote desktop's viewer).
struct CursorCast {
    session: CursorSession,
    damage: Option<(OutputDamageTracker, Size<i32, Physical>, f64)>,
    shown: bool,
    targets: Targets,
}

/// A cursor's picture is a square this many points wide, whatever its
/// shape (Haiku's cursors are drawn on 32; a program's own larger than
/// that is cut).
pub const CURSOR_BOX: f64 = 32.0;

/// The size of the cursor's picture on a screen at `scale`, in pixels.
pub fn cursor_size(scale: f64) -> Size<i32, Physical> {
    let side = (CURSOR_BOX * scale).ceil() as i32;
    (side, side).into()
}

/// What a client's buffers for the cursor on `output` must be (shared
/// memory: it's small).
pub fn cursor_constraints(output: &Output) -> BufferConstraints {
    let size = cursor_size(output.current_scale().fractional_scale());
    BufferConstraints { size: (size.w, size.h).into(), shm: vec![wl_shm::Format::Argb8888], dma: None }
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
    cursors: Vec<CursorCast>,
    /// Cursor frames asked for, waiting for the cursor's picture to change
    /// (the first: for its output's next frame).
    pending_cursors: Vec<(CursorSessionRef, Frame)>,
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
    size(output).map(|size| constraints_of(size, dma))
}

/// What a client's buffers for a picture of `size` must be.
pub fn constraints_of(size: Size<i32, Physical>, dma: Option<DmabufConstraints>) -> BufferConstraints {
    BufferConstraints { size: (size.w, size.h).into(), shm: vec![wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888], dma }
}

/// The formats app_server can draw captures into, from what its renderer
/// renders: BGRx and BGRA, with their modifiers.
pub fn dma_constraints(node: smithay::backend::drm::DrmNode, formats: &smithay::backend::allocator::format::FormatSet) -> Option<DmabufConstraints> {
    let formats: Vec<_> =
        [Fourcc::Xrgb8888, Fourcc::Argb8888].into_iter().map(|code| (code, formats.iter().filter(|f| f.code == code).map(|f| f.modifier).collect::<Vec<_>>())).filter(|(_, m)| !m.is_empty()).collect();
    (!formats.is_empty()).then_some(DmabufConstraints { node, formats })
}

impl Captures {
    /// A new session, kept until the client ends it (dropping it would
    /// stop it).
    pub fn add(&mut self, session: Session) {
        self.casts.push(Cast { session, damage: None, shown: false, targets: Targets::default() });
    }

    /// Whether any session waits for a frame (else there's nothing to
    /// look at each frame).
    pub fn pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn remove(&mut self, session: &SessionRef) {
        self.casts.retain(|c| c.session.as_ref() != *session);
        self.pending.retain(|(s, _)| s != session);
    }

    /// A frame `session` asked for, drawn with its output's next frame
    /// (a window's: that of the output it is mostly on).
    pub fn queue(&mut self, session: &SessionRef, frame: Frame) {
        if target(session).is_none() {
            frame.fail(CaptureFailureReason::Unknown);
            return;
        }
        self.pending.push((session.clone(), frame));
    }

    /// A new cursor session, kept until the client ends it.
    pub fn add_cursor(&mut self, session: CursorSession) {
        self.cursors.push(CursorCast { session, damage: None, shown: false, targets: Targets::default() });
    }

    pub fn remove_cursor(&mut self, session: &CursorSessionRef) {
        self.cursors.retain(|c| c.session.as_ref() != *session);
        self.pending_cursors.retain(|(s, _)| s != session);
    }

    /// Whether any client follows the cursor (else nothing to tell).
    pub fn has_cursors(&self) -> bool {
        !self.cursors.is_empty()
    }

    /// A cursor frame `session` asked for: drawn when the cursor's
    /// picture next changes (the first at once, with its screen's frame).
    pub fn queue_cursor(&mut self, session: &CursorSessionRef, frame: Frame) {
        self.pending_cursors.push((session.clone(), frame));
    }

    /// `output`'s frame: its cursor sessions told where the pointer is
    /// on it (`at`, in the desktop's points; None off it, or hidden) and
    /// its hotspot, and the cursor frames waiting drawn if its picture
    /// (`status`) changed since their session's last.
    pub fn cursors<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        area: Rectangle<f64, smithay::utils::Logical>,
        presented: Duration,
        at: smithay::utils::Point<f64, smithay::utils::Logical>,
        status: &smithay::input::pointer::CursorImageStatus,
    ) where
        R: Renderer + Bind<Dmabuf> + Offscreen<GlesTexture> + ExportMem + smithay::backend::renderer::ImportAll + smithay::backend::renderer::ImportMem,
        R::Error: std::fmt::Debug,
        R::TextureId: Texture + Clone + Send + 'static,
    {
        let on = |s: &CursorSessionRef| s.source().user_data().get::<WeakOutput>().and_then(|w| w.upgrade()).is_some_and(|o| o == *output);
        if !self.cursors.iter().any(|c| on(&c.session)) {
            return;
        }
        let scale = output.current_scale().fractional_scale();
        let hidden = matches!(status, smithay::input::pointer::CursorImageStatus::Hidden);
        let position = (!hidden && area.contains(at)).then(|| {
            let p = (at - area.loc).to_physical(scale).to_i32_round::<i32>();
            smithay::utils::Point::<i32, BufferCoords>::from((p.x, p.y))
        });
        let hotspot = match crate::cursor::named(status) {
            Some(shape) => {
                let (x, y) = crate::cursor::image(shape, scale).hotspot;
                (x as i32, y as i32)
            }
            None => {
                let h = crate::cursor::hotspot(status).to_f64().to_physical(scale).to_i32_round::<i32>();
                (h.x, h.y)
            }
        };
        for c in &self.cursors {
            if on(&c.session) {
                c.session.set_cursor_hotspot(hotspot);
                c.session.set_cursor_pos(position);
            }
        }
        let size = cursor_size(scale);
        let mut elements: Option<Vec<crate::render::CustomRenderElements<R>>> = None;
        for (session, frame) in std::mem::take(&mut self.pending_cursors) {
            if !on(&session) || hidden {
                self.pending_cursors.push((session, frame));
                continue;
            }
            if frame.has_failed() {
                continue;
            }
            let Some(cast) = self.cursors.iter_mut().find(|c| c.session.as_ref() == session) else {
                frame.fail(CaptureFailureReason::Stopped);
                continue;
            };
            if session.current_constraints().is_some_and(|c| c.size != (size.w, size.h).into()) {
                session.update_constraints(cursor_constraints(output));
                frame.fail(CaptureFailureReason::BufferConstraints);
                continue;
            }
            let elements = elements.get_or_insert_with(|| crate::render::cursor_picture(renderer, scale, status));
            if cast.damage.as_ref().is_none_or(|(_, s, sc)| *s != size || *sc != scale) {
                cast.damage = Some((OutputDamageTracker::new(size, scale, Transform::Normal), size, scale));
            }
            let tracker = &mut cast.damage.as_mut().unwrap().0;
            let damage = match tracker.damage_output(1, elements) {
                Ok((d, _)) => d.map(|d| d.iter().map(|r| Rectangle::new((r.loc.x, r.loc.y).into(), (r.size.w, r.size.h).into())).collect::<Vec<_>>()),
                Err(_) => {
                    frame.fail(CaptureFailureReason::Unknown);
                    continue;
                }
            };
            if cast.shown && damage.is_none() {
                self.pending_cursors.push((session, frame));
                continue;
            }
            let damage = if cast.shown { damage } else { None };
            cast.shown = true;
            match draw(renderer, &mut cast.targets, &frame, elements, Color32F::TRANSPARENT, size, scale) {
                Ok(Some(sync)) => self.drawn.push(Drawn { frame, sync, damage, presented }),
                Ok(None) => frame.success(Transform::Normal, damage, presented),
                Err(e) => {
                    tracing::warn!("cursor capture for {}: {e}", output.name());
                    frame.fail(e.reason());
                }
            }
        }
    }

    /// Stop the sessions of window `id` (it went).
    pub fn window_gone(&mut self, id: u64) {
        let gone = |s: &SessionRef| matches!(target(s), Some((Target::Window(w), _)) if w == id);
        self.pending.retain(|(s, _)| !gone(s));
        let (stopped, kept): (Vec<Cast>, Vec<Cast>) = std::mem::take(&mut self.casts).into_iter().partition(|c| gone(&c.session.as_ref()));
        self.casts = kept;
        for c in stopped {
            c.session.stop();
        }
    }

    /// Tell the frames whose drawing the GPU has finished.
    pub fn finish(&mut self) {
        let (done, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.drawn).into_iter().partition(|d| d.sync.is_reached());
        self.drawn = rest;
        for d in done {
            d.frame.success(Transform::Normal, d.damage, d.presented);
        }
    }

    /// Whether a frame for `output`'s frame is waiting (so the caller can
    /// skip building what it draws): of it, or of a window `on_output`
    /// says is mostly on it.
    pub fn wanted(&self, output: &Output, on_output: impl Fn(u64) -> bool) -> bool {
        self.pending.iter().any(|(s, _)| match target(s) {
            Some((Target::Output, o)) => o.is_none_or(|o| o == *output),
            Some((Target::Window(id), _)) => on_output(id),
            None => true,
        })
    }

    /// Draw the frames waiting for `output`'s frame (of it, or of a window
    /// `on_output` says is mostly on it) whose picture changed since their
    /// session's last. `picture(renderer, target, cursor)` is what one
    /// shows (with the pointer if `cursor`); None: it's gone.
    pub fn take<R, E>(&mut self, renderer: &mut R, output: &Output, presented: Duration, on_output: impl Fn(u64) -> bool, mut picture: impl FnMut(&mut R, Target, bool) -> Option<Picture<E>>)
    where
        R: Renderer + Bind<Dmabuf> + Offscreen<GlesTexture> + ExportMem,
        R::Error: std::fmt::Debug,
        R::TextureId: Texture,
        E: RenderElement<R>,
    {
        self.finish();
        if !self.wanted(output, &on_output) {
            return;
        }
        let mut built: HashMap<(Target, bool), Option<Picture<E>>> = HashMap::new();
        let mut gone = Vec::new();
        let pending = std::mem::take(&mut self.pending);
        for (session, frame) in pending {
            let mine = match target(&session) {
                None => {
                    frame.fail(CaptureFailureReason::Unknown);
                    continue;
                }
                Some((Target::Output, o)) => match o {
                    None => {
                        frame.fail(CaptureFailureReason::Unknown);
                        continue;
                    }
                    Some(o) if o == *output => Some(Target::Output),
                    Some(_) => None,
                },
                Some((Target::Window(id), _)) => on_output(id).then_some(Target::Window(id)),
            };
            let Some(what) = mine else {
                self.pending.push((session, frame));
                continue;
            };
            if frame.has_failed() {
                continue;
            }
            let Some(cast) = self.casts.iter_mut().find(|c| c.session.as_ref() == session) else {
                frame.fail(CaptureFailureReason::Stopped);
                continue;
            };
            let cursor = session.draw_cursor();
            let Some(pic) = built.entry((what, cursor)).or_insert_with(|| picture(renderer, what, cursor)) else {
                if let Target::Window(id) = what {
                    gone.push(id);
                }
                frame.fail(CaptureFailureReason::Stopped);
                continue;
            };
            let (size, scale) = (pic.size, pic.scale);
            // The picture changed size (the screen's mode, the window): the
            // client must allocate again.
            if session.current_constraints().is_some_and(|c| c.size != (size.w, size.h).into()) {
                session.update_constraints(constraints_of(size, self.dma.clone()));
                frame.fail(CaptureFailureReason::BufferConstraints);
                continue;
            }
            // Only when something changed since its last frame.
            if cast.damage.as_ref().is_none_or(|(_, s, sc)| *s != size || *sc != scale) {
                cast.damage = Some((OutputDamageTracker::new(size, scale, Transform::Normal), size, scale));
            }
            let tracker = &mut cast.damage.as_mut().unwrap().0;
            let damage = match tracker.damage_output(1, &pic.elements) {
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
            match draw(renderer, &mut cast.targets, &frame, &pic.elements, pic.clear, size, scale) {
                Ok(Some(sync)) => self.drawn.push(Drawn { frame, sync, damage, presented }),
                Ok(None) => frame.success(Transform::Normal, damage, presented),
                Err(e) => {
                    tracing::warn!("capture for {}: {e}", output.name());
                    frame.fail(e.reason());
                }
            }
        }
        for id in gone {
            self.window_gone(id);
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
fn draw<R, E>(renderer: &mut R, cast: &mut Targets, frame: &Frame, elements: &[E], clear: Color32F, size: Size<i32, Physical>, scale: f64) -> Result<Option<SyncPoint>, DrawError>
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
