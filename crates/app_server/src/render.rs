use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
        Color32F, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, TextureMapping,
        damage::{Error as OutputDamageTrackerError, OutputDamageTracker, RenderOutputResult},
        element::{
            AsRenderElements, RenderElement, Wrap,
            surface::WaylandSurfaceRenderElement,
            utils::{
                ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, RelocateRenderElement,
                RescaleRenderElement,
            },
        },
    },
    },
    desktop::space::{
        ConstrainBehavior, ConstrainReference, Space, SpaceRenderElements, constrain_space_element,
    },
    output::Output,
    utils::{Buffer, Physical, Point, Rectangle, Size, Transform},
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
    Overlay=smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<R>,
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
    // Locked: nothing but what the caller passed (the pointer and the lock
    // screen) over an opaque background.
    if locked {
        let [r, g, b, a] = crate::lock::BACKGROUND;
        return (custom_elements.into_iter().map(OutputRenderElements::from).collect(), Color32F::new(r, g, b, a));
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
        (elements, CLEAR_COLOR_FULLSCREEN)
    } else {
        let mut output_render_elements = custom_elements
            .into_iter()
            .map(OutputRenderElements::from)
            .collect::<Vec<_>>();

        if show_window_preview && space.elements_for_output(output).count() > 0 {
            output_render_elements.extend(space_preview_elements(renderer, space, output));
        }

        let space_elements = smithay::desktop::space::space_render_elements::<_, WindowElement, _>(
            renderer,
            [space],
            output,
            1.0,
        )
        .expect("output without mode?");
        output_render_elements.extend(space_elements.into_iter().map(OutputRenderElements::Space));

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
) -> Result<RenderOutputResult<'d>, OutputDamageTrackerError<R::Error>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let (elements, clear_color) =
        output_elements(output, space, custom_elements, renderer, show_window_preview, desktop_color, locked);
    damage_tracker.render_output(renderer, framebuffer, age, &elements, clear_color)
}

/// Render `output` offscreen and read it back (the GPU part of a
/// screenshot; encoding is left to a thread, see `IpcState::save_screenshot`).
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
    let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
    let scale = output.current_scale().fractional_scale();
    let err = |e: R::Error| e.to_string();

    let mut target = renderer.create_buffer(Fourcc::Abgr8888, buffer_size).map_err(err)?;
    let mapping = {
        let mut fb = renderer.bind(&mut target).map_err(err)?;
        let (elements, clear_color) =
            output_elements::<R>(output, space, overlays, renderer, false, clear_color, locked);
        let mut damage_tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
        damage_tracker
            .render_output(renderer, &mut fb, 0, &elements, clear_color)
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
