use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
        Color32F, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, TextureMapping,
        damage::{Error as OutputDamageTrackerError, OutputDamageTracker, RenderOutputResult},
        utils::{CommitCounter, DamageSet, OpaqueRegions},
        element::{
            AsRenderElements, Element, Id, Kind, RenderElement, UnderlyingStorage, Wrap,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::{
                CropRenderElement, Relocate, RelocateRenderElement,
                RescaleRenderElement,
            },
        },
    },
    },
    desktop::{
        LayerSurface, layer_map_for_output,
        space::{Space, SpaceElement, SpaceRenderElements},
    },
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform, user_data::UserDataMap},
    wayland::shell::wlr_layer::Layer,
};

#[cfg(feature = "debug")]
use crate::drawing::FpsElement;
use crate::{
    drawing::{CLEAR_COLOR_FULLSCREEN, PointerRenderElement},
    shell::{FullscreenSurface, WindowElement, WindowRenderElement},
};

smithay::backend::renderer::element::render_elements! {
    pub CustomRenderElements<R> where
        R: ImportAll + ImportMem;
    Pointer=PointerRenderElement<R>,
    Surface=WaylandSurfaceRenderElement<R>,
    Overlay=Exact<MemoryRenderBufferRenderElement<R>>,
    Warm=smithay::backend::renderer::element::solid::SolidColorRenderElement,
    #[cfg(feature = "debug")]
    // Note: We would like to borrow this element instead, but that would introduce
    // a feature-dependent lifetime, which introduces a lot more feature bounds
    // as the whole type changes and we can't have an unused lifetime (for when "debug" is disabled)
    // in the declaration.
    Fps=FpsElement<R::TextureId>,
}

impl<R: Renderer> std::fmt::Debug for CustomRenderElements<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pointer(arg0) => f.debug_tuple("Pointer").field(arg0).finish(),
            Self::Surface(arg0) => f.debug_tuple("Surface").field(arg0).finish(),
            Self::Overlay(arg0) => f.debug_tuple("Overlay").field(arg0).finish(),
            Self::Warm(arg0) => f.debug_tuple("Warm").field(arg0).finish(),
            #[cfg(feature = "debug")]
            Self::Fps(arg0) => f.debug_tuple("Fps").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

smithay::backend::renderer::element::render_elements! {
    pub OutputRenderElements<R, E> where R: ImportAll + ImportMem;
    Space=SpaceRenderElements<R, E>,
    Window=Wrap<E>,
    Custom=CustomRenderElements<R>,
    Preview=CropRenderElement<RelocateRenderElement<RescaleRenderElement<WindowRenderElement<R>>>>,
}

impl<R: Renderer + ImportAll + ImportMem, E: RenderElement<R> + std::fmt::Debug> std::fmt::Debug
    for OutputRenderElements<R, E>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Space(arg0) => f.debug_tuple("Space").field(arg0).finish(),
            Self::Window(arg0) => f.debug_tuple("Window").field(arg0).finish(),
            Self::Custom(arg0) => f.debug_tuple("Custom").field(arg0).finish(),
            Self::Preview(arg0) => f.debug_tuple("Preview").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

// ---- pixel for pixel --------------------------------------------------------

/// A picture drawn pixel for pixel: exactly `rect`, in device pixels.
/// smithay sizes a memory element in whole logical points, so at a
/// fractional scale a picture drawn at its device size `w` comes out at
/// `round(round(w / scale) × scale)`, often a pixel off, and is resampled
/// (soft); this keeps the rectangle it was drawn for.
#[derive(Debug)]
pub struct Exact<E> {
    inner: E,
    rect: Rectangle<i32, Physical>,
}

impl<E: Element> Element for Exact<E> {
    fn id(&self) -> &Id {
        self.inner.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }
    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        self.rect.loc
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }
    fn transform(&self) -> Transform {
        self.inner.transform()
    }
    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.rect
    }
    // The inner element is built a point per device pixel (`exact`), so
    // at scale 1 its damage and opaque regions are in device pixels.
    fn damage_since(&self, _scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        self.inner.damage_since(Scale::from(1.0), commit)
    }
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(Scale::from(1.0))
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for Exact<E> {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        self.inner.draw(frame, src, dst, damage, opaque_regions, cache)
    }
    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }
}

/// `buffer` (buffer scale 1) drawn into `rect` (device pixels): its
/// `src` pixels, all of it for a picture drawn at that size, or one
/// pixel stretched for a line.
pub fn exact<R>(
    renderer: &mut R,
    rect: Rectangle<i32, Physical>,
    buffer: &MemoryRenderBuffer,
    src: (i32, i32),
    alpha: Option<f32>,
    kind: Kind,
) -> Option<Exact<MemoryRenderBufferRenderElement<R>>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let from = Rectangle::from_size((src.0 as f64, src.1 as f64).into());
    let size: Size<i32, Logical> = (rect.size.w, rect.size.h).into();
    let inner =
        MemoryRenderBufferRenderElement::from_buffer(renderer, rect.loc.to_f64(), buffer, alpha, Some(from), Some(size), kind)
            .ok()?;
    Some(Exact { inner, rect })
}

/// [`exact`] for a picture drawn at its own size, `size` device pixels,
/// its top left at `at` (to the nearest device pixel).
pub fn picture<R>(
    renderer: &mut R,
    at: Point<f64, Physical>,
    buffer: &MemoryRenderBuffer,
    size: (i32, i32),
    alpha: Option<f32>,
    kind: Kind,
) -> Option<Exact<MemoryRenderBufferRenderElement<R>>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    exact(renderer, Rectangle::new(at.to_i32_round(), size.into()), buffer, size, alpha, kind)
}

// ---- mirroring ------------------------------------------------------------

/// What a mirrored screen shows (kept in its user data). A mirror is not
/// in the Space: it draws its source's frame, scaled to fit and centred.
#[derive(Default)]
struct MirrorOf(std::sync::Mutex<Option<Output>>);

pub fn mirror_source(output: &Output) -> Option<Output> {
    output.user_data().get::<MirrorOf>()?.0.lock().unwrap().clone()
}

pub fn set_mirror_source(output: &Output, source: Option<Output>) {
    output.user_data().insert_if_missing_threadsafe(MirrorOf::default);
    *output.user_data().get::<MirrorOf>().unwrap().0.lock().unwrap() = source;
}

/// How a mirror shows its source: scaled by `factor`, then moved by
/// `offset` (device pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub factor: f64,
    pub offset: Point<i32, Physical>,
}

/// Fit a `source` picture (device pixels) into a `mirror`'s, keeping its
/// shape.
pub fn fit(source: Size<i32, Physical>, mirror: Size<i32, Physical>) -> Fit {
    let (sw, sh) = (source.w.max(1) as f64, source.h.max(1) as f64);
    let (mw, mh) = (mirror.w as f64, mirror.h as f64);
    let factor = (mw / sw).min(mh / sh);
    let offset = (((mw - sw * factor) / 2.0).round() as i32, ((mh - sh * factor) / 2.0).round() as i32);
    Fit { factor, offset: offset.into() }
}

/// [`fit`] for a mirror output and its source.
pub fn fit_outputs(mirror: &Output, source: &Output, space: &Space<WindowElement>) -> Fit {
    let size = |o: &Output| o.current_mode().map(|m| o.current_transform().transform_size(m.size)).unwrap_or_default();
    let from = space
        .output_geometry(source)
        .map(|g| g.size.to_f64().to_physical(source.current_scale().fractional_scale()).to_i32_round())
        .unwrap_or_else(|| size(source));
    fit(from, size(mirror))
}

smithay::backend::renderer::element::render_elements! {
    /// A frame: a screen's own elements, or its source's when it mirrors.
    pub FrameElements<R, E> where R: ImportAll + ImportMem;
    Normal=OutputRenderElements<R, E>,
    Mirror=RelocateRenderElement<RescaleRenderElement<OutputRenderElements<R, E>>>,
}

impl<R: Renderer + ImportAll + ImportMem, E: RenderElement<R> + std::fmt::Debug> std::fmt::Debug for FrameElements<R, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal(e) => f.debug_tuple("Normal").field(e).finish(),
            Self::Mirror(e) => f.debug_tuple("Mirror").field(e).finish(),
            Self::_GenericCatcher(e) => f.debug_tuple("_GenericCatcher").field(e).finish(),
        }
    }
}

/// A screen's elements as a frame, fitted when it mirrors another.
pub fn frame<R, E>(elements: Vec<OutputRenderElements<R, E>>, fit: Option<Fit>) -> Vec<FrameElements<R, E>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: 'static,
    E: RenderElement<R>,
{
    match fit {
        None => elements.into_iter().map(FrameElements::Normal).collect(),
        Some(fit) => elements
            .into_iter()
            .map(|e| {
                let scaled = RescaleRenderElement::from_element(e, (0, 0).into(), fit.factor);
                FrameElements::Mirror(RelocateRenderElement::from_element(scaled, fit.offset, Relocate::Relative))
            })
            .collect(),
    }
}

#[profiling::function]
/// Put the level indicator just under the pointer, which comes first.
fn add_indicator<R, E>(elements: &mut Vec<OutputRenderElements<R, E>>, indicator: Option<OutputRenderElements<R, E>>)
where
    R: Renderer + ImportAll + ImportMem,
    E: RenderElement<R>,
{
    if let Some(i) = indicator {
        let at = elements.iter().position(|e| !matches!(e, OutputRenderElements::Custom(CustomRenderElements::Pointer(_)))).unwrap_or(elements.len());
        elements.insert(at, i);
    }
}

/// The layer surfaces and windows, front first, as smithay's
/// `space_render_elements` draws them but for the menus of the Background
/// and Bottom layers (`shell::lower_layer_menus`): over the windows and
/// the Top layer rather than under the windows with their surface. With
/// the overview out on `output`, it stands in for the windows.
fn space_elements<R>(
    renderer: &mut R,
    space: &Space<WindowElement>,
    output: &Output,
    overview: Option<&crate::overview::Overview>,
) -> Vec<OutputRenderElements<R, WindowRenderElement<R>>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let scale = output.current_scale().fractional_scale();
    let layers: Vec<(LayerSurface, Point<i32, Logical>)> = {
        let map = layer_map_for_output(output);
        map.layers().rev().filter_map(|l| map.layer_geometry(l).map(|g| (l.clone(), g.loc))).collect()
    };
    let mut out = Vec::new();
    // A layer surface with its menus, or (`tree`) a surface alone.
    let whole = |renderer: &mut R, out: &mut Vec<_>, layer: Layer| {
        for (l, at) in layers.iter().filter(|(l, _)| l.layer() == layer) {
            let start = std::time::Instant::now();
            let elements: Vec<WaylandSurfaceRenderElement<R>> =
                AsRenderElements::<R>::render_elements(l, renderer, at.to_physical_precise_round(scale), scale.into(), 1.0);
            slow_elements(start, || format!("layer {}", l.namespace()));
            out.extend(elements.into_iter().map(|e| OutputRenderElements::Space(SpaceRenderElements::Surface(e))));
        }
    };
    let tree = |renderer: &mut R, out: &mut Vec<_>, surface: &WlSurface, at: Point<i32, Logical>| {
        let elements: Vec<WaylandSurfaceRenderElement<R>> = render_elements_from_surface_tree(
            renderer,
            surface,
            at.to_physical_precise_round(scale),
            scale,
            1.0,
            Kind::Unspecified,
        );
        out.extend(elements.into_iter().map(|e| OutputRenderElements::Space(SpaceRenderElements::Surface(e))));
    };
    let t = std::time::Instant::now();
    whole(renderer, &mut out, Layer::Overlay);
    for (menu, at) in crate::shell::lower_layer_menus(output) {
        tree(renderer, &mut out, &menu, at);
    }
    whole(renderer, &mut out, Layer::Top);
    part("gather: upper layers", t);
    let t = std::time::Instant::now();
    let window = |e| OutputRenderElements::Space(SpaceRenderElements::Element(Wrap::from(e)));
    if let (Some(region), Some(overview)) = (space.output_geometry(output), overview.filter(|o| o.shows(&output.name()))) {
        out.extend(overview.elements(renderer, output, region.loc, region.size));
    } else if let Some(region) = space.output_geometry(output) {
        // As `Space::render_elements_for_region` draws them, but for the
        // spotlight while switching: its dim under the windows it keeps
        // bright (the front ones), over the rest.
        let area = Rectangle::from_size(region.size.to_f64().to_physical(scale).to_i32_ceil());
        let mut dim = crate::spotlight::dim(&output.name(), area);
        for w in space.elements().rev() {
            let (Some(bbox), Some(at)) = (space.element_bbox(w), space.element_location(w)) else { continue };
            // Behind the window Focus shows (`shell::modes`), or filling
            // a display (drawn by that one).
            if !region.overlaps(bbox) || w.meta().covered.get() || w.meta().fullscreen.get() {
                continue;
            }
            if dim.as_ref().is_some_and(|(bright, _)| !bright.contains(&w.id())) {
                out.push(window(WindowRenderElement::Decoration(dim.take().unwrap().1)));
            }
            let location = (at - SpaceElement::geometry(w).loc - region.loc).to_physical_precise_round(scale);
            let start = std::time::Instant::now();
            let elements: Vec<WindowRenderElement<R>> = w.render_elements(renderer, location, scale.into(), 1.0);
            slow_elements(start, || format!("window {} {:?}", w.id(), SpaceElement::geometry(w).size));
            out.extend(elements.into_iter().map(window));
        }
        if let Some((_, d)) = dim {
            out.push(window(WindowRenderElement::Decoration(d)));
        }
    }
    part("gather: windows", t);
    // The lower layers themselves, without their menus.
    let t = std::time::Instant::now();
    for (l, at) in layers.iter().filter(|(l, _)| matches!(l.layer(), Layer::Bottom | Layer::Background)) {
        tree(renderer, &mut out, l.wl_surface(), *at);
    }
    part("gather: lower layers", t);
    out
}

thread_local! {
    /// The parts of the look being drawn and what each took, for the
    /// timings (`part`, `take_parts`).
    static PARTS: std::cell::RefCell<Vec<(&'static str, std::time::Duration)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Part `name` of a look done, since `start` (`Timings::phases`).
pub fn part(name: &'static str, start: std::time::Instant) {
    PARTS.with(|p| p.borrow_mut().push((name, start.elapsed())));
}

/// Part `name` of a look done, in `took` (Smithay's compositor phases,
/// `PHASE_HOOK`).
pub fn add_part(name: &'static str, took: std::time::Duration) {
    PARTS.with(|p| p.borrow_mut().push((name, took)));
}

/// The parts of the look just done (`part`).
pub fn take_parts() -> Vec<(&'static str, std::time::Duration)> {
    PARTS.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// What took over 4 ms to gather (most likely importing a buffer it had
/// just committed), named by `what`, in the log.
fn slow_elements(start: std::time::Instant, what: impl FnOnce() -> String) {
    let took = start.elapsed();
    if took > std::time::Duration::from_millis(4) {
        tracing::info!("slow elements: {} took {:.1} ms", what(), took.as_secs_f64() * 1e3);
    }
}

pub fn output_elements<R>(
    output: &Output,
    space: &Space<WindowElement>,
    custom_elements: impl IntoIterator<Item = CustomRenderElements<R>>,
    renderer: &mut R,
    overview: Option<&crate::overview::Overview>,
    desktop_color: Color32F,
    locked: bool,
) -> (Vec<OutputRenderElements<R, WindowRenderElement<R>>>, Color32F)
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let warm = keep_warm(output).map(|e| OutputRenderElements::Custom(CustomRenderElements::Warm(e)));
    if warm_uploads() {
        crate::shell::deco::warm_tab_upload(renderer);
    }
    let (mut elements, clear) = gather_elements(output, space, custom_elements, renderer, overview, desktop_color, locked);
    if let Some(warm) = warm {
        elements.insert(0, warm);
    }
    (elements, clear)
}

/// How long the GPU driver keeps memory for a frame's commands that no
/// frame used (Mesa's, at least; Asahi's drops it after about 2 s).
const WARM_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// Kept warm this long after the last frame that drew something.
const WARM_FOR: std::time::Duration = std::time::Duration::from_secs(120);

/// The frames' memory kept warm (`keep_warm`): each screen's pixel, the
/// commit they share, when it last changed, and when a frame last drew
/// anything else.
struct Warm {
    ids: std::collections::HashMap<String, Id>,
    commit: CommitCounter,
    bumped: std::time::Instant,
    drawn: std::time::Instant,
    /// When the uploads that keep the driver's staging memory warm were
    /// last done (`warm_uploads`).
    uploaded: std::time::Instant,
    /// What each screen last had of the pixel (its commit, or none).
    shown: std::collections::HashMap<String, Option<CommitCounter>>,
    /// The screen whose frame being gathered changed its pixel.
    changed: Option<String>,
}

thread_local! {
    static WARM: std::cell::RefCell<Warm> = std::cell::RefCell::new(Warm {
        ids: Default::default(),
        commit: CommitCounter::default(),
        bumped: std::time::Instant::now(),
        drawn: std::time::Instant::now(),
        uploaded: std::time::Instant::now(),
        shown: Default::default(),
        changed: None,
    });
}

/// A pixel of nothing in the screen's top left corner, there for a frame
/// or two every second while the desktop is in use, so a frame is drawn
/// (and one more as it goes; only then, so a full-screen window still
/// goes straight to the display between): the GPU
/// driver keeps memory for a frame's commands only while frames use it,
/// and getting more maps it into the GPU's address space, which can hold
/// the CPU for tens of milliseconds while the GPU is busy (Apple's: the
/// first frames after a pause took 10–30 ms). Drawing a pixel costs
/// nothing to see.
fn keep_warm(output: &Output) -> Option<smithay::backend::renderer::element::solid::SolidColorRenderElement> {
    WARM.with(|w| {
        let mut w = w.borrow_mut();
        let now = std::time::Instant::now();
        if now - w.bumped >= WARM_EVERY && now - w.drawn <= WARM_FOR {
            w.commit.increment();
            w.bumped = now;
        }
        // (Every screen's next frame, whatever its rate.)
        let showing = (now - w.bumped <= std::time::Duration::from_millis(30)).then_some(w.commit);
        let name = output.name();
        let changed = w.shown.insert(name.clone(), showing) != Some(showing);
        w.changed = changed.then(|| name.clone());
        showing?;
        let id = w.ids.entry(name).or_insert_with(Id::new).clone();
        Some(smithay::backend::renderer::element::solid::SolidColorRenderElement::new(
            id,
            Rectangle::from_size((1, 1).into()),
            w.commit,
            [0.0, 0.0, 0.0, 0.0],
            Kind::Unspecified,
        ))
    })
}

/// Whether this frame does the uploads that keep the driver's memory for
/// writing textures warm (once each time `keep_warm`'s pixel changes).
fn warm_uploads() -> bool {
    WARM.with(|w| {
        let mut w = w.borrow_mut();
        let due = w.uploaded < w.bumped;
        if due {
            w.uploaded = w.bumped;
        }
        due
    })
}

/// Whether the frame just gathered for `output` changed `keep_warm`'s
/// pixel: a resting display's look that found only that stays resting.
pub fn warm_changed(output: &Output) -> bool {
    WARM.with(|w| w.borrow().changed.as_deref() == Some(output.name().as_str()))
}

/// Whether displays rest (`OLO_REST=off`, or `Request::SetResting` at
/// run time, stops it). The first time on real displays it made the
/// desktop stutter: every pointer motion woke every resting display for a
/// look, each with a DRM test commit, up to four a frame, and a scroll's
/// glide went on at ten frames a second once nothing changed. Now pointer
/// motion and scrolling wake only the displays under the pointer, a woken
/// look comes a frame after the last at the soonest (for the next vblank),
/// and nothing rests while something of app_server's own moves
/// (`keeps_awake`): headless, scrolling as smooth as without (glides
/// smoother), a storm of pointer motion no dearer, an idle display ten
/// looks a second.
pub fn rests(_real: bool) -> bool {
    match REST.load(std::sync::atomic::Ordering::Relaxed) {
        ON => true,
        OFF => false,
        _ => {
            let from_env = match std::env::var("OLO_REST").as_deref() {
                Ok("on") => ON,
                Ok("off") => OFF,
                _ => DEFAULT,
            };
            let _ = REST.compare_exchange(UNSET, from_env, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed);
            REST.load(std::sync::atomic::Ordering::Relaxed) != OFF
        }
    }
}

/// Resting on or off for every display from now (`Request::SetResting`),
/// or (None) as the backend has it.
pub fn set_rests(on: Option<bool>) {
    REST.store(match on { Some(true) => ON, Some(false) => OFF, None => DEFAULT }, std::sync::atomic::Ordering::Relaxed);
}

static REST: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(UNSET);
const UNSET: u8 = 0;
const DEFAULT: u8 = 1;
const ON: u8 = 2;
const OFF: u8 = 3;

/// Frames in a row with nothing new after which a display rests: it
/// looks again only every `IDLE_POLL`, or at once when woken
/// (`Backend::wake`: a commit on it, input, a request, the windows
/// changing). Looking costs gathering every element on it and comparing
/// them: done 120 times a second for each display, it kept an idle
/// app_server at a fifth of a core. Few, as what comes next wakes it
/// anyway: a blinking cursor's frame was followed by twelve looks.
pub const IDLE_FRAMES: u32 = 3;
/// How often a resting display looks anyway, for what changed without
/// waking it (`keep_warm`'s pixel, a timer of ours not hooked up).
pub const IDLE_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// A frame was drawn (`drew`: anything at all): unless it was the
/// screens' frames for `keep_warm`'s pixel (any in the few milliseconds
/// after it changed), the desktop is in use.
pub fn frame_done(drew: bool) {
    WARM.with(|w| {
        let mut w = w.borrow_mut();
        let now = std::time::Instant::now();
        if drew && now - w.bumped > std::time::Duration::from_millis(60) {
            w.drawn = now;
        }
    });
}

fn gather_elements<R>(
    output: &Output,
    space: &Space<WindowElement>,
    custom_elements: impl IntoIterator<Item = CustomRenderElements<R>>,
    renderer: &mut R,
    overview: Option<&crate::overview::Overview>,
    desktop_color: Color32F,
    locked: bool,
) -> (Vec<OutputRenderElements<R, WindowRenderElement<R>>>, Color32F)
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    // The brightness / volume indicator, over everything but the pointer.
    let screen = space.output_geometry(output).map(|g| g.size).unwrap_or_default();
    let indicator = crate::osd::element(renderer, output, screen)
        .map(|e| OutputRenderElements::Custom(CustomRenderElements::Overlay(e)));
    // Locked: nothing but what the caller passed (the pointer and the lock
    // screen) over an opaque background.
    if locked {
        let [r, g, b, a] = crate::lock::BACKGROUND;
        let mut elements: Vec<_> = custom_elements.into_iter().map(OutputRenderElements::from).collect();
        add_indicator(&mut elements, indicator);
        return (elements, Color32F::new(r, g, b, a));
    }
    if let Some(window) = output
        .user_data()
        .get::<FullscreenSurface>()
        .and_then(|f| f.get())
    {
        let scale = output.current_scale().fractional_scale().into();
        let window_render_elements: Vec<WindowRenderElement<R>> =
            AsRenderElements::<R>::render_elements(&window, renderer, (0, 0).into(), scale, 1.0);

        let elements = custom_elements
            .into_iter()
            .map(OutputRenderElements::from)
            .chain(
                window_render_elements
                    .into_iter()
                    .map(|e| OutputRenderElements::Window(Wrap::from(e))),
            )
            .collect::<Vec<_>>();
        let mut elements = elements;
        add_indicator(&mut elements, indicator);
        (elements, CLEAR_COLOR_FULLSCREEN)
    } else {
        let mut output_render_elements = custom_elements
            .into_iter()
            .map(OutputRenderElements::from)
            .collect::<Vec<_>>();
        // Preferences ▸ Display ▸ Identify, over the windows.
        output_render_elements.extend(
            crate::identify::element(renderer, output)
                .map(|e| OutputRenderElements::Custom(CustomRenderElements::Overlay(e))),
        );
        add_indicator(&mut output_render_elements, indicator);

        crate::shell::groups::mark_behind(space);
        crate::shell::modes::mark_covered(space);
        output_render_elements.extend(space_elements(renderer, space, output, overview));
        // The wallpaper goes under everything, over the workspace colour.
        let start = std::time::Instant::now();
        output_render_elements.extend(
            crate::wallpaper::element(renderer, output)
                .into_iter()
                .map(|e| OutputRenderElements::Custom(CustomRenderElements::Overlay(e))),
        );
        slow_elements(start, || "the wallpaper".into());
        part("gather: wallpaper", start);

        (output_render_elements, desktop_color)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_output<'a, 'd, R>(
    output: &'a Output,
    space: &'a Space<WindowElement>,
    custom_elements: impl IntoIterator<Item = CustomRenderElements<R>>,
    renderer: &'a mut R,
    framebuffer: &'a mut R::Framebuffer<'_>,
    damage_tracker: &'d mut OutputDamageTracker,
    age: usize,
    overview: Option<&crate::overview::Overview>,
    desktop_color: Color32F,
    locked: bool,
    fit: Option<Fit>,
) -> Result<RenderOutputResult<'d>, OutputDamageTrackerError<R::Error>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let start = std::time::Instant::now();
    let cpu = ThreadUse::now();
    let (elements, clear_color) =
        output_elements(output, space, custom_elements, renderer, overview, desktop_color, locked);
    let elements = frame(elements, fit);
    note_content(output, &elements);
    let gathered = start.elapsed();
    part("gather (all)", start);
    let t = std::time::Instant::now();
    let result = damage_tracker.render_output(renderer, framebuffer, age, &elements, clear_color);
    part("damage and draw", t);
    // A frame over a 120 Hz frame's time says where it went: gathering the
    // elements (which imports what clients committed) or drawing them.
    let took = start.elapsed();
    frame_done(result.as_ref().is_ok_and(|r| r.damage.is_some_and(|d| !d.is_empty())));
    // Which elements weren't in the output's last frame (a texture, or a
    // kind of drawing, new to the GPU may cost it more the first time).
    let ids: std::collections::HashSet<Id> = elements.iter().map(|e| e.id().clone()).collect();
    let new = LAST_IDS.with(|last| {
        let mut last = last.borrow_mut();
        let seen = last.entry(output.name()).or_default();
        let new = ids.difference(seen).count();
        *seen = ids;
        new
    });
    if took > std::time::Duration::from_micros(8_333) {
        let damaged: i64 = match &result {
            Ok(r) => r.damage.map_or(0, |d| d.iter().map(|r| r.size.w as i64 * r.size.h as i64).sum()),
            Err(_) => 0,
        };
        // (Whether it was working, waiting for the GPU or a lock, or
        // waiting for a CPU while others ran.)
        let used = ThreadUse::now().since(&cpu);
        tracing::info!(
            "slow frame on {}: {:.1} ms gathering {} elements ({new} new), {:.1} ms drawing {:.1} Mpx; {used}",
            output.name(),
            gathered.as_secs_f64() * 1e3,
            elements.len(),
            (took - gathered).as_secs_f64() * 1e3,
            damaged as f64 / 1e6,
        );
    }
    result
}

/// One element of a frame, as far as what it shows goes: which, its
/// content's commit, where, how see-through.
type Shown = (Id, CommitCounter, Rectangle<i32, Physical>, u32);

thread_local! {
    /// Whether the frames drawn now are watched (`watch_content`).
    static WATCHED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Each watched display's last frame, the keep-warm pixel aside.
    static CONTENT: std::cell::RefCell<std::collections::HashMap<String, Vec<Shown>>> = Default::default();
    /// Whether the frame just gathered shows something new.
    static CHANGED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Whether the next frames are watched for what they show (someone takes
/// `EventKind::Painted`): only then is each compared with the last.
pub fn watch_content(on: bool) {
    WATCHED.with(|w| w.set(on));
}

/// Whether the frame just drawn showed anything new, `keep_warm`'s
/// pixel aside (it damages as much as the whole display: smithay counts
/// an element's place in the stack from the top, so one coming and
/// going there moves all the others'). True when not watched.
pub fn content_changed() -> bool {
    CHANGED.with(|c| c.get())
}

fn note_content<E: Element>(output: &Output, elements: &[E]) {
    if !WATCHED.with(|w| w.get()) {
        CHANGED.with(|c| c.set(true));
        return;
    }
    let scale = output.current_scale().fractional_scale();
    let name = output.name();
    let warm = WARM.with(|w| w.borrow().ids.get(&name).cloned());
    let now: Vec<Shown> = elements.iter().filter(|e| Some(e.id()) != warm.as_ref()).map(|e| (e.id().clone(), e.current_commit(), e.geometry(scale.into()), e.alpha().to_bits())).collect();
    let changed = CONTENT.with(|c| c.borrow_mut().insert(name, now.clone()).is_none_or(|last| last != now));
    CHANGED.with(|c| c.set(changed));
}

/// What this thread has used: CPU time, and how often it gave the CPU up
/// to wait (for the GPU, a lock, a file) or had it taken (by another
/// thread or program, its time up).
pub struct ThreadUse {
    pub cpu_ms: f64,
    pub waits: i64,
    pub preempted: i64,
    /// Pages it touched that weren't in memory: read back from swap or a
    /// file (major), or made (minor).
    pub major_faults: i64,
    pub minor_faults: i64,
}

impl ThreadUse {
    pub fn now() -> ThreadUse {
        // SAFETY: getrusage fills the struct it is given.
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_THREAD, &mut u) };
        let ms = |t: libc::timeval| t.tv_sec as f64 * 1e3 + t.tv_usec as f64 / 1e3;
        ThreadUse { cpu_ms: ms(u.ru_utime) + ms(u.ru_stime), waits: u.ru_nvcsw, preempted: u.ru_nivcsw, major_faults: u.ru_majflt, minor_faults: u.ru_minflt }
    }

    pub fn since(&self, before: &ThreadUse) -> ThreadUse {
        ThreadUse {
            cpu_ms: self.cpu_ms - before.cpu_ms,
            waits: self.waits - before.waits,
            preempted: self.preempted - before.preempted,
            major_faults: self.major_faults - before.major_faults,
            minor_faults: self.minor_faults - before.minor_faults,
        }
    }
}

impl std::fmt::Display for ThreadUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:.1} ms on a CPU, {} waits, {} times preempted, {} pages faulted in ({} from swap or files)",
            self.cpu_ms,
            self.waits,
            self.preempted,
            self.major_faults + self.minor_faults,
            self.major_faults
        )
    }
}

thread_local! {
    /// Each output's last frame's elements (`render_output`).
    static LAST_IDS: std::cell::RefCell<std::collections::HashMap<String, std::collections::HashSet<Id>>> = Default::default();
}

/// Render `output` offscreen and read it back (see screenshot.rs).
pub fn capture<R, T>(
    renderer: &mut R,
    output: &Output,
    space: &Space<WindowElement>,
    overlays: Vec<CustomRenderElements<R>>,
    overview: Option<&crate::overview::Overview>,
    clear_color: Color32F,
    locked: bool,
) -> Result<image::RgbaImage, String>
where
    R: Renderer + ImportAll + ImportMem + Offscreen<T> + ExportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: std::fmt::Display,
{
    let size = crate::capture::size(output).ok_or("output has no mode")?;
    let scale = output.current_scale().fractional_scale();
    let (elements, clear_color) = capture_elements(renderer, output, space, overlays, overview, clear_color, locked, None);
    offscreen(renderer, size, scale, &elements, clear_color)
}

/// What a picture of `output` shows (a mirror's is its source's, fitted),
/// with the pointer over it when `pointer` gives where it is and what it
/// shows; and the background.
#[allow(clippy::too_many_arguments)]
pub fn capture_elements<R>(
    renderer: &mut R,
    output: &Output,
    space: &Space<WindowElement>,
    mut overlays: Vec<CustomRenderElements<R>>,
    overview: Option<&crate::overview::Overview>,
    clear_color: Color32F,
    locked: bool,
    pointer: Option<(Point<f64, Logical>, &smithay::input::pointer::CursorImageStatus)>,
) -> (Vec<FrameElements<R, WindowRenderElement<R>>>, Color32F)
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: smithay::backend::renderer::Texture + Clone + Send + 'static,
{
    let source = mirror_source(output);
    let fit = source.as_ref().map(|s| fit_outputs(output, s, space));
    let from = source.as_ref().unwrap_or(output);
    if let Some((at, status)) = pointer {
        overlays.splice(0..0, pointer_elements(renderer, from, space, at, status));
    }
    let (elements, clear_color) = output_elements::<R>(from, space, overlays, renderer, overview, clear_color, locked);
    (frame(elements, fit), clear_color)
}

/// A picture of window `id` alone, for a screen share of it: only what
/// its client draws (no tab or border), whole even when it is behind
/// another of its group's tabs, at the scale of the screen it is mostly
/// on; the elements, and its size in pixels and that scale.
pub fn window_picture<R>(renderer: &mut R, space: &Space<WindowElement>, id: u64) -> Option<(Vec<FrameElements<R, WindowRenderElement<R>>>, Size<i32, Physical>, f64)>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: smithay::backend::renderer::Texture + Clone + Send + 'static,
{
    let window = space.elements().find(|w| w.id() == id)?;
    let at = space.element_location(window)?;
    let scale = space.outputs_for_element(window).first().map_or(1.0, |o| o.current_scale().fractional_scale());
    let geo = SpaceElement::geometry(window);
    // Where the element draws from, as Space does it.
    let origin = at - geo.loc;
    let rect = Rectangle::new(origin + window.content_offset(), window.content_size());
    let location = (origin - rect.loc).to_f64().to_physical(scale).to_i32_round();
    // Whole, though it is behind others in its group (marked so again
    // before the next frame).
    window.decoration().behind = false;
    let elements: Vec<WindowRenderElement<R>> = window.render_elements(renderer, location, scale.into(), 1.0);
    let size = rect.size.to_f64().to_physical(scale).to_i32_round();
    let elements = elements.into_iter().map(|e| FrameElements::Normal(OutputRenderElements::Window(Wrap::from(e)))).collect();
    (size.w > 0 && size.h > 0).then_some((elements, size, scale))
}

/// The pointer as drawn on `output`, if it is over it: the built-in
/// cursor for a named shape, or the client's surface.
pub fn pointer_elements<R>(
    renderer: &mut R,
    output: &Output,
    space: &Space<WindowElement>,
    at: Point<f64, Logical>,
    status: &smithay::input::pointer::CursorImageStatus,
) -> Vec<CustomRenderElements<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: smithay::backend::renderer::Texture + Clone + Send + 'static,
{
    let Some(geo) = space.output_geometry(output) else { return Vec::new() };
    if !geo.to_f64().contains(at) {
        return Vec::new();
    }
    let scale = output.current_scale().fractional_scale();
    let shape = crate::cursor::named(status).unwrap_or(crate::cursor::Shape::SystemDefault);
    let mut pointer = crate::drawing::PointerElement::default();
    pointer.set_buffer(crate::cursor::buffer(shape, scale), crate::cursor::image(shape, scale).size as i32);
    pointer.set_status(status.clone());
    let location = (at - geo.loc.to_f64() - crate::cursor::hotspot(status).to_f64()).to_physical(scale).to_i32_round();
    pointer.render_elements(renderer, location, scale.into(), 1.0)
}

/// Draw `elements` into a new `size` buffer and read it back, top row first
/// (premultiplied RGBA).
pub fn offscreen<R, T, E>(
    renderer: &mut R,
    size: Size<i32, Physical>,
    scale: f64,
    elements: &[E],
    clear_color: Color32F,
) -> Result<image::RgbaImage, String>
where
    R: Renderer + Offscreen<T> + ExportMem,
    R::Error: std::fmt::Display,
    E: RenderElement<R>,
{
    if size.w <= 0 || size.h <= 0 {
        return Err("nothing to capture".into());
    }
    let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
    let err = |e: R::Error| e.to_string();
    let mut target = renderer.create_buffer(Fourcc::Abgr8888, buffer_size).map_err(err)?;
    let mapping = {
        let mut fb = renderer.bind(&mut target).map_err(err)?;
        let mut damage_tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
        damage_tracker
            .render_output(renderer, &mut fb, 0, elements, clear_color)
            .map_err(|e| format!("{e:?}"))?;
        renderer
            .copy_framebuffer(&fb, Rectangle::from_size(buffer_size), Fourcc::Abgr8888)
            .map_err(err)?
    };
    let flipped = mapping.flipped();
    let data = renderer.map_texture(&mapping).map_err(err)?.to_vec();
    let mut img = image::RgbaImage::from_raw(size.w as u32, size.h as u32, data).ok_or("short read")?;
    // GL reads back bottom-up unless the mapping already accounts for it.
    if !flipped {
        image::imageops::flip_vertical_in_place(&mut img);
    }
    Ok(img)
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    #[test]
    fn fits_and_centres() {
        // 16:10 onto 16:9: full height, bars left and right.
        let f = fit((2560, 1600).into(), (1920, 1080).into());
        assert!((f.factor - 0.675).abs() < 1e-9);
        assert_eq!(f.offset, (96, 0).into());
        // The same shape: edge to edge.
        assert_eq!(fit((1280, 800).into(), (2560, 1600).into()), Fit { factor: 2.0, offset: (0, 0).into() });
    }
}
