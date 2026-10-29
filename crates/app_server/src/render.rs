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
                ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, Relocate, RelocateRenderElement,
                RescaleRenderElement,
            },
        },
    },
    },
    desktop::{
        LayerSurface, layer_map_for_output,
        space::{ConstrainBehavior, ConstrainReference, Space, SpaceRenderElements, constrain_space_element},
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

pub fn space_preview_elements<'a, R, C>(
    renderer: &'a mut R,
    space: &'a Space<WindowElement>,
    output: &'a Output,
) -> impl Iterator<Item = C> + 'a
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
    C: From<CropRenderElement<RelocateRenderElement<RescaleRenderElement<WindowRenderElement<R>>>>> + 'a,
{
    let constrain_behavior = ConstrainBehavior {
        reference: ConstrainReference::BoundingBox,
        behavior: ConstrainScaleBehavior::Fit,
        align: ConstrainAlign::CENTER,
    };

    let preview_padding = 10;

    let elements_on_space = space.elements_for_output(output).count();
    let output_scale = output.current_scale().fractional_scale();
    let output_transform = output.current_transform();
    let output_size = output
        .current_mode()
        .map(|mode| {
            output_transform
                .transform_size(mode.size)
                .to_f64()
                .to_logical(output_scale)
        })
        .unwrap_or_default();

    let max_elements_per_row = 4;
    let elements_per_row = usize::min(elements_on_space, max_elements_per_row);
    let rows = f64::ceil(elements_on_space as f64 / elements_per_row as f64);

    let preview_size = Size::from((
        f64::round(output_size.w / elements_per_row as f64) as i32 - preview_padding * 2,
        f64::round(output_size.h / rows) as i32 - preview_padding * 2,
    ));

    space
        .elements_for_output(output)
        .enumerate()
        .flat_map(move |(element_index, window)| {
            let column = element_index % elements_per_row;
            let row = element_index / elements_per_row;
            let preview_location = Point::from((
                preview_padding + (preview_padding + preview_size.w) * column as i32,
                preview_padding + (preview_padding + preview_size.h) * row as i32,
            ));
            let constrain = Rectangle::new(preview_location, preview_size);
            constrain_space_element(
                renderer,
                window,
                preview_location,
                1.0,
                output_scale,
                constrain,
                constrain_behavior,
            )
        })
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
/// the Top layer rather than under the windows with their surface.
fn space_elements<R>(
    renderer: &mut R,
    space: &Space<WindowElement>,
    output: &Output,
) -> Vec<SpaceRenderElements<R, WindowRenderElement<R>>>
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
            let elements: Vec<WaylandSurfaceRenderElement<R>> =
                AsRenderElements::<R>::render_elements(l, renderer, at.to_physical_precise_round(scale), scale.into(), 1.0);
            out.extend(elements.into_iter().map(SpaceRenderElements::Surface));
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
        out.extend(elements.into_iter().map(SpaceRenderElements::Surface));
    };
    whole(renderer, &mut out, Layer::Overlay);
    for (menu, at) in crate::shell::lower_layer_menus(output) {
        tree(renderer, &mut out, &menu, at);
    }
    whole(renderer, &mut out, Layer::Top);
    if let Some(region) = space.output_geometry(output) {
        out.extend(
            space
                .render_elements_for_region(renderer, &region, scale, 1.0)
                .into_iter()
                .map(|e| SpaceRenderElements::Element(Wrap::from(e))),
        );
    }
    // The lower layers themselves, without their menus.
    for (l, at) in layers.iter().filter(|(l, _)| matches!(l.layer(), Layer::Bottom | Layer::Background)) {
        tree(renderer, &mut out, l.wl_surface(), *at);
    }
    out
}

pub fn output_elements<R>(
    output: &Output,
    space: &Space<WindowElement>,
    custom_elements: impl IntoIterator<Item = CustomRenderElements<R>>,
    renderer: &mut R,
    show_window_preview: bool,
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

        if show_window_preview && space.elements_for_output(output).count() > 0 {
            output_render_elements.extend(space_preview_elements(renderer, space, output));
        }

        crate::shell::groups::mark_behind(space);
        let space_elements = space_elements(renderer, space, output);
        output_render_elements.extend(space_elements.into_iter().map(OutputRenderElements::Space));
        // The wallpaper goes under everything, over the workspace colour.
        output_render_elements.extend(
            crate::wallpaper::element(renderer, output)
                .map(|e| OutputRenderElements::Custom(CustomRenderElements::Overlay(e))),
        );

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
    show_window_preview: bool,
    desktop_color: Color32F,
    locked: bool,
    fit: Option<Fit>,
) -> Result<RenderOutputResult<'d>, OutputDamageTrackerError<R::Error>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let (elements, clear_color) =
        output_elements(output, space, custom_elements, renderer, show_window_preview, desktop_color, locked);
    let elements = frame(elements, fit);
    damage_tracker.render_output(renderer, framebuffer, age, &elements, clear_color)
}

/// Render `output` offscreen and read it back (see screenshot.rs).
pub fn capture<R, T>(
    renderer: &mut R,
    output: &Output,
    space: &Space<WindowElement>,
    overlays: Vec<CustomRenderElements<R>>,
    clear_color: Color32F,
    locked: bool,
) -> Result<image::RgbaImage, String>
where
    R: Renderer + ImportAll + ImportMem + Offscreen<T> + ExportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: std::fmt::Display,
{
    let mode = output.current_mode().ok_or("output has no mode")?;
    let size: Size<i32, Physical> = output.current_transform().transform_size(mode.size);
    let scale = output.current_scale().fractional_scale();
    // A mirror's picture is its source's, fitted.
    let source = mirror_source(output);
    let fit = source.as_ref().map(|s| fit_outputs(output, s, space));
    let from = source.as_ref().unwrap_or(output);
    let (elements, clear_color) = output_elements::<R>(from, space, overlays, renderer, false, clear_color, locked);
    let elements = frame(elements, fit);
    offscreen(renderer, size, scale, &elements, clear_color)
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
