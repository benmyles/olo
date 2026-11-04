// Doing `if let` gives you nice scoping with passes/encoders
#![allow(irrefutable_let_patterns)]

use super::{BladeAtlas, BladeContext, blade_context::PipelineCache};
use crate::{
    Background, Bounds, DevicePixels, GpuSpecs, ImageTriangle, MonochromeSprite, Path, Point,
    PolychromeSprite, PrimitiveBatch, Quad, ScaledPixels, Scene, Sector, Segment, Shadow, Size,
    Underline,
};
use blade_graphics as gpu;
use blade_util::{BufferBelt, BufferBeltDescriptor};
use bytemuck::{Pod, Zeroable};
#[cfg(target_os = "macos")]
use media::core_video::CVMetalTextureCache;
use std::sync::{Arc, Mutex};

const MAX_FRAME_TIME_MS: u32 = 10000;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlobalParams {
    viewport_size: [f32; 2],
    premultiplied_alpha: u32,
    pad: u32,
}

//Note: we can't use `Bounds` directly here because
// it doesn't implement Pod + Zeroable
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PodBounds {
    origin: [f32; 2],
    size: [f32; 2],
}

impl From<Bounds<ScaledPixels>> for PodBounds {
    fn from(bounds: Bounds<ScaledPixels>) -> Self {
        Self {
            origin: [bounds.origin.x.0, bounds.origin.y.0],
            size: [bounds.size.width.0, bounds.size.height.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SurfaceParams {
    bounds: PodBounds,
    content_mask: PodBounds,
}

#[derive(blade_macros::ShaderData)]
struct ShaderQuadsData {
    globals: GlobalParams,
    b_quads: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderShadowsData {
    globals: GlobalParams,
    b_shadows: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderPathRasterizationData {
    globals: GlobalParams,
    b_path_vertices: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderPathsData {
    globals: GlobalParams,
    t_sprite: gpu::TextureView,
    s_sprite: gpu::Sampler,
    b_path_sprites: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderUnderlinesData {
    globals: GlobalParams,
    b_underlines: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderSegmentsData {
    globals: GlobalParams,
    b_segments: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderSectorsData {
    globals: GlobalParams,
    b_sectors: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderMonoSpritesData {
    globals: GlobalParams,
    gamma_ratios: [f32; 4],
    grayscale_enhanced_contrast: f32,
    t_sprite: gpu::TextureView,
    s_sprite: gpu::Sampler,
    b_mono_sprites: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderPolySpritesData {
    globals: GlobalParams,
    t_sprite: gpu::TextureView,
    s_sprite: gpu::Sampler,
    b_poly_sprites: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderImageTrianglesData {
    globals: GlobalParams,
    t_sprite: gpu::TextureView,
    s_sprite: gpu::Sampler,
    b_image_triangles: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ShaderSurfacesData {
    globals: GlobalParams,
    surface_locals: SurfaceParams,
    t_y: gpu::TextureView,
    t_cb_cr: gpu::TextureView,
    s_surface: gpu::Sampler,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
struct PathSprite {
    bounds: Bounds<ScaledPixels>,
}

#[derive(Clone, Debug)]
#[repr(C)]
struct PathRasterizationVertex {
    xy_position: Point<ScaledPixels>,
    st_position: Point<f32>,
    color: Background,
    bounds: Bounds<ScaledPixels>,
}

pub(super) struct BladePipelines {
    quads: gpu::RenderPipeline,
    shadows: gpu::RenderPipeline,
    path_rasterization: gpu::RenderPipeline,
    paths: gpu::RenderPipeline,
    underlines: gpu::RenderPipeline,
    segments: gpu::RenderPipeline,
    sectors: gpu::RenderPipeline,
    mono_sprites: gpu::RenderPipeline,
    poly_sprites: gpu::RenderPipeline,
    image_triangles: gpu::RenderPipeline,
    surfaces: gpu::RenderPipeline,
}

impl BladePipelines {
    fn new(
        gpu: &gpu::Context,
        shader: &gpu::Shader,
        surface_info: gpu::SurfaceInfo,
        path_sample_count: u32,
    ) -> Self {
        use gpu::ShaderData as _;

        log::info!(
            "Initializing Blade pipelines for surface {:?}",
            surface_info
        );
        shader.check_struct_size::<GlobalParams>();
        shader.check_struct_size::<SurfaceParams>();
        shader.check_struct_size::<Quad>();
        shader.check_struct_size::<Shadow>();
        shader.check_struct_size::<PathRasterizationVertex>();
        shader.check_struct_size::<PathSprite>();
        shader.check_struct_size::<Underline>();
        shader.check_struct_size::<Segment>();
        shader.check_struct_size::<Sector>();
        shader.check_struct_size::<MonochromeSprite>();
        shader.check_struct_size::<PolychromeSprite>();
        shader.check_struct_size::<ImageTriangle>();

        // See https://apoorvaj.io/alpha-compositing-opengl-blending-and-premultiplied-alpha/
        let blend_mode = match surface_info.alpha {
            gpu::AlphaMode::Ignored => gpu::BlendState::ALPHA_BLENDING,
            gpu::AlphaMode::PreMultiplied => gpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            gpu::AlphaMode::PostMultiplied => gpu::BlendState::ALPHA_BLENDING,
        };
        let color_targets = &[gpu::ColorTargetState {
            format: surface_info.format,
            blend: Some(blend_mode),
            write_mask: gpu::ColorWrites::default(),
        }];

        // Olo: made side by side (the driver compiles each on the thread
        // that asks; one after another they held a window's opening up
        // ~10 ms).
        let shader = &shader;
        std::thread::scope(|s| {
            let quads = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "quads",
                    data_layouts: &[&ShaderQuadsData::layout()],
                    vertex: shader.at("vs_quad"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_quad")),
                    color_targets,
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let shadows = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "shadows",
                    data_layouts: &[&ShaderShadowsData::layout()],
                    vertex: shader.at("vs_shadow"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_shadow")),
                    color_targets,
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let path_rasterization = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "path_rasterization",
                    data_layouts: &[&ShaderPathRasterizationData::layout()],
                    vertex: shader.at("vs_path_rasterization"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleList,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_path_rasterization")),
                    // The original implementation was using ADDITIVE blende mode,
                    // I don't know why
                    // color_targets: &[gpu::ColorTargetState {
                    //     format: PATH_TEXTURE_FORMAT,
                    //     blend: Some(gpu::BlendState::ADDITIVE),
                    //     write_mask: gpu::ColorWrites::default(),
                    // }],
                    color_targets: &[gpu::ColorTargetState {
                        format: surface_info.format,
                        blend: Some(gpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: gpu::ColorWrites::default(),
                    }],
                    multisample_state: gpu::MultisampleState {
                        sample_count: path_sample_count,
                        ..Default::default()
                    },
                })
            });
            let paths = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "paths",
                    data_layouts: &[&ShaderPathsData::layout()],
                    vertex: shader.at("vs_path"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_path")),
                    color_targets: &[gpu::ColorTargetState {
                        format: surface_info.format,
                        blend: Some(gpu::BlendState {
                            color: gpu::BlendComponent::OVER,
                            alpha: gpu::BlendComponent::ADDITIVE,
                        }),
                        write_mask: gpu::ColorWrites::default(),
                    }],
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let underlines = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "underlines",
                    data_layouts: &[&ShaderUnderlinesData::layout()],
                    vertex: shader.at("vs_underline"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_underline")),
                    color_targets,
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let segments = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "segments",
                    data_layouts: &[&ShaderSegmentsData::layout()],
                    vertex: shader.at("vs_segment"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_segment")),
                    color_targets,
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let sectors = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "sectors",
                    data_layouts: &[&ShaderSectorsData::layout()],
                    vertex: shader.at("vs_sector"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_sector")),
                    color_targets,
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let mono_sprites = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "mono-sprites",
                    data_layouts: &[&ShaderMonoSpritesData::layout()],
                    vertex: shader.at("vs_mono_sprite"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_mono_sprite")),
                    color_targets,
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let poly_sprites = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "poly-sprites",
                    data_layouts: &[&ShaderPolySpritesData::layout()],
                    vertex: shader.at("vs_poly_sprite"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_poly_sprite")),
                    color_targets,
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let image_triangles = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "image-triangles",
                    data_layouts: &[&ShaderImageTrianglesData::layout()],
                    vertex: shader.at("vs_image_triangle"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleList,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_image_triangle")),
                    color_targets,
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let surfaces = s.spawn(move || {
                gpu.create_render_pipeline(gpu::RenderPipelineDesc {
                    name: "surfaces",
                    data_layouts: &[&ShaderSurfacesData::layout()],
                    vertex: shader.at("vs_surface"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState {
                        topology: gpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    fragment: Some(shader.at("fs_surface")),
                    color_targets,
                    multisample_state: gpu::MultisampleState::default(),
                })
            });
            let made = |p: std::thread::ScopedJoinHandle<gpu::RenderPipeline>| {
                p.join().expect("a pipeline made")
            };
            Self {
                quads: made(quads),
                shadows: made(shadows),
                path_rasterization: made(path_rasterization),
                paths: made(paths),
                underlines: made(underlines),
                segments: made(segments),
                sectors: made(sectors),
                mono_sprites: made(mono_sprites),
                poly_sprites: made(poly_sprites),
                image_triangles: made(image_triangles),
                surfaces: made(surfaces),
            }
        })
    }
}

pub struct BladeSurfaceConfig {
    pub size: gpu::Extent,
    pub transparent: bool,
}

//Note: we could see some of these fields moved into `BladeContext`
// so that they are shared between windows. E.g. `pipelines`.
// But that is complicated by the fact that pipelines depend on
// the format and alpha mode.
pub struct BladeRenderer {
    gpu: Arc<gpu::Context>,
    surface: gpu::Surface,
    surface_config: gpu::SurfaceConfig,
    command_encoder: gpu::CommandEncoder,
    last_sync_point: Option<gpu::SyncPoint>,
    /// Olo: the part of the swapchain drawn in: all of it, or less while
    /// it has room to grow (`update_drawable_size_within`).
    drawn: gpu::Extent,
    pipelines: Arc<BladePipelines>,
    pipeline_cache: Arc<Mutex<PipelineCache>>,
    instance_belt: BufferBelt,
    atlas: Arc<BladeAtlas>,
    atlas_sampler: gpu::Sampler,
    #[cfg(target_os = "macos")]
    core_video_texture_cache: CVMetalTextureCache,
    /// Olo: made the first time a frame has paths, and dropped when the
    /// window resizes; most windows never draw a path, and these are
    /// window-sized (the multisampled one four times over).
    path_targets: Option<PathTargets>,
    /// Olo: for LCD subpixel glyphs, made the first time a frame has
    /// some (only where the GPU blends with a second source).
    subpixel_sprites: Option<Arc<gpu::RenderPipeline>>,
    rendering_parameters: RenderingParameters,
    /// Olo: the window draws in Display P3 (`Window::set_gamut`):
    /// the shaders convert sRGB's colours into P3's numbers.
    display_p3: bool,
    /// Olo: the window drawn into, to make its surface again when it's
    /// lost (`remake_surface`), and when that was last tried.
    window: Option<RawHandles>,
    remade: Option<std::time::Instant>,
}

/// Olo: a window's raw handles, kept to make its surface again: they
/// stay valid while the window does, which outlives its renderer.
struct RawHandles {
    window: raw_window_handle::RawWindowHandle,
    display: raw_window_handle::RawDisplayHandle,
}

impl raw_window_handle::HasWindowHandle for RawHandles {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: the window this was taken from is still there (above).
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.window) })
    }
}

impl raw_window_handle::HasDisplayHandle for RawHandles {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: as for the window.
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(self.display) })
    }
}

/// Olo: the pipelines for a surface like `info`, from the context's
/// cache (made the first time).
fn shared_pipelines(
    gpu: &gpu::Context,
    cache: &Mutex<PipelineCache>,
    info: gpu::SurfaceInfo,
    path_sample_count: u32,
) -> Arc<BladePipelines> {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, set)) = cache
        .sets
        .iter()
        .find(|(key, _)| *key == (info, path_sample_count))
    {
        return set.clone();
    }
    let shader = cache.shader(gpu);
    let set = Arc::new(BladePipelines::new(gpu, &shader, info, path_sample_count));
    cache.sets.push(((info, path_sample_count), set.clone()));
    set
}

impl BladeRenderer {
    /// Olo: draw in Display P3 (`on`) or sRGB from the next frame, the
    /// swapchain made again in that colour space (Mesa's WSI tells the
    /// compositor); whether it is (a surface not offering P3: sRGB).
    pub fn set_display_p3(&mut self, on: bool) -> bool {
        let space = if on {
            gpu::ColorSpace::DisplayP3
        } else {
            gpu::ColorSpace::Srgb
        };
        if space != self.surface_config.color_space {
            self.wait_for_gpu();
            self.surface_config.color_space = space;
            let format = self.surface.info().format;
            self.gpu
                .reconfigure_surface(&mut self.surface, self.surface_config);
            // (P3's swapchain is ten bits a channel where offered: the
            // pipelines made again for it, as for a change of alpha.)
            if self.surface.info().format != format {
                self.pipelines = shared_pipelines(
                    &self.gpu,
                    &self.pipeline_cache,
                    self.surface.info(),
                    self.rendering_parameters.path_sample_count,
                );
                self.subpixel_sprites = None;
                self.destroy_path_targets();
            }
        }
        self.display_p3 = self.surface.color_space() == gpu::ColorSpace::DisplayP3;
        if on {
            log::info!(
                "asked to draw in Display P3: {}",
                if self.display_p3 {
                    "drawing in it"
                } else {
                    "the surface doesn't offer it, sRGB"
                }
            );
        }
        self.display_p3
    }

    pub fn new<I: raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle>(
        context: &BladeContext,
        window: &I,
        config: BladeSurfaceConfig,
    ) -> anyhow::Result<Self> {
        let surface_config = gpu::SurfaceConfig {
            size: config.size,
            usage: gpu::TextureUsage::TARGET,
            display_sync: gpu::DisplaySync::Recent,
            color_space: gpu::ColorSpace::Srgb,
            allow_exclusive_full_screen: false,
            transparent: config.transparent,
        };
        let surface = context
            .gpu
            .create_surface_configured(window, surface_config)
            .map_err(|err| anyhow::anyhow!("Failed to create surface: {err:?}"))?;
        let handles = window
            .window_handle()
            .ok()
            .zip(window.display_handle().ok())
            .map(|(w, d)| RawHandles {
                window: w.as_raw(),
                display: d.as_raw(),
            });

        let command_encoder = context.gpu.create_command_encoder(gpu::CommandEncoderDesc {
            name: "main",
            buffer_count: 2,
        });
        let rendering_parameters = RenderingParameters::from_env(context);
        let pipelines = shared_pipelines(
            &context.gpu,
            &context.pipelines,
            surface.info(),
            rendering_parameters.path_sample_count,
        );
        let instance_belt = BufferBelt::new(BufferBeltDescriptor {
            memory: gpu::Memory::Shared,
            min_chunk_size: 0x1000,
            alignment: 0x40, // Vulkan `minStorageBufferOffsetAlignment` on Intel Xe
        });
        let atlas = context.atlas.clone();
        let atlas_sampler = context.gpu.create_sampler(gpu::SamplerDesc {
            name: "path rasterization sampler",
            mag_filter: gpu::FilterMode::Linear,
            min_filter: gpu::FilterMode::Linear,
            ..Default::default()
        });

        #[cfg(target_os = "macos")]
        let core_video_texture_cache = unsafe {
            CVMetalTextureCache::new(
                objc2::rc::Retained::as_ptr(&context.gpu.metal_device()) as *mut _
            )
            .unwrap()
        };

        Ok(Self {
            gpu: Arc::clone(&context.gpu),
            surface,
            surface_config,
            command_encoder,
            last_sync_point: None,
            drawn: surface_config.size,
            pipelines,
            pipeline_cache: context.pipelines.clone(),
            instance_belt,
            atlas,
            atlas_sampler,
            #[cfg(target_os = "macos")]
            core_video_texture_cache,
            path_targets: None,
            subpixel_sprites: None,
            rendering_parameters,
            display_p3: false,
            window: handles,
            remade: None,
        })
    }

    /// Olo: the window's surface was lost (its compositor went away and
    /// came back, an upgrade among it: Telegram panicked on it): a new
    /// one on the same window, at most every 2 s; the lost one kept if
    /// none can be made.
    fn remake_surface(&mut self) {
        let Some(window) = &self.window else { return };
        if self
            .remade
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2))
        {
            return;
        }
        self.remade = Some(std::time::Instant::now());
        let mut surface = match self.gpu.create_surface(window) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("the window's surface is lost and can't be made again ({e:?})");
                return;
            }
        };
        self.wait_for_gpu();
        let format = self.surface.info().format;
        self.gpu.destroy_surface(&mut self.surface);
        self.gpu
            .reconfigure_surface(&mut surface, self.surface_config);
        if surface.info().format != format {
            log::warn!(
                "the window's new surface is {:?}, its pipelines {format:?}",
                surface.info().format
            );
        }
        self.surface = surface;
        log::info!("the window's surface was lost: made again");
    }

    fn wait_for_gpu(&mut self) {
        if let Some(last_sp) = self.last_sync_point.take()
            && !self.gpu.wait_for(&last_sp, MAX_FRAME_TIME_MS)
        {
            log::error!("GPU hung");
            #[cfg(target_os = "linux")]
            if self.gpu.device_information().driver_name == "radv" {
                log::error!(
                    "there's a known bug with amdgpu/radv, try setting ZED_PATH_SAMPLE_COUNT=0 as a workaround"
                );
                log::error!(
                    "if that helps you're running into https://github.com/zed-industries/zed/issues/26143"
                );
            }
            log::error!(
                "your device information is: {:?}",
                self.gpu.device_information()
            );
            while !self.gpu.wait_for(&last_sp, MAX_FRAME_TIME_MS) && !gpu::device_lost() {}
        }
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        self.update_drawable_size_impl(size, false);
    }

    /// Olo: drawn at `size` in a swapchain of at least `room` (each
    /// bigger): a window being resized gets room to grow, so the sizes it
    /// goes through cost nothing (a swapchain made is new GPU memory,
    /// 10 ms and more for a big one on Apple's GPU, every frame of a
    /// resize). It shows only its part (`drawn`, the viewport's source).
    pub fn update_drawable_size_within(
        &mut self,
        size: Size<DevicePixels>,
        room: Size<DevicePixels>,
    ) {
        let (w, h) = (size.width.0.max(1) as u32, size.height.0.max(1) as u32);
        let fits = self.surface_config.size.width >= w && self.surface_config.size.height >= h;
        let roomy = self.surface_config.size.width >= room.width.0 as u32
            && self.surface_config.size.height >= room.height.0 as u32;
        if !(fits && roomy) {
            let room = Size {
                width: DevicePixels(room.width.0.max(size.width.0)),
                height: DevicePixels(room.height.0.max(size.height.0)),
            };
            self.update_drawable_size_impl(room, false);
        }
        self.drawn = gpu::Extent {
            width: w,
            height: h,
            depth: 1,
        };
    }

    /// Olo: the part of the swapchain drawn in (the window's size).
    pub fn drawn_size(&self) -> Size<DevicePixels> {
        Size {
            width: DevicePixels(self.drawn.width as i32),
            height: DevicePixels(self.drawn.height as i32),
        }
    }

    /// Olo: whether the swapchain is bigger than what's drawn (room left
    /// from a resize).
    pub fn has_room(&self) -> bool {
        self.surface_config.size.width > self.drawn.width
            || self.surface_config.size.height > self.drawn.height
    }

    /// Like `update_drawable_size` but skips the check that the size has changed. This is useful in
    /// cases like restoring a window from minimization where the size is the same but the
    /// renderer's swap chain needs to be recreated.
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux", target_os = "freebsd"),
        allow(dead_code)
    )]
    pub fn update_drawable_size_even_if_unchanged(&mut self, size: Size<DevicePixels>) {
        self.update_drawable_size_impl(size, true);
    }

    fn update_drawable_size_impl(&mut self, size: Size<DevicePixels>, always_resize: bool) {
        let gpu_size = gpu::Extent {
            width: size.width.0 as u32,
            height: size.height.0 as u32,
            depth: 1,
        };

        if always_resize || gpu_size != self.surface_config.size {
            self.wait_for_gpu();
            self.surface_config.size = gpu_size;
            self.gpu
                .reconfigure_surface(&mut self.surface, self.surface_config);
            self.destroy_path_targets();
        }
        self.drawn = gpu_size;
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        if transparent != self.surface_config.transparent {
            self.wait_for_gpu();
            self.surface_config.transparent = transparent;
            self.gpu
                .reconfigure_surface(&mut self.surface, self.surface_config);
            self.pipelines = shared_pipelines(
                &self.gpu,
                &self.pipeline_cache,
                self.surface.info(),
                self.rendering_parameters.path_sample_count,
            );
            self.subpixel_sprites = None;
        }
    }

    #[cfg_attr(
        any(target_os = "macos", feature = "wayland", target_os = "windows"),
        allow(dead_code)
    )]
    pub fn viewport_size(&self) -> gpu::Extent {
        self.surface_config.size
    }

    pub fn sprite_atlas(&self) -> &Arc<BladeAtlas> {
        &self.atlas
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn gpu_specs(&self) -> GpuSpecs {
        let info = self.gpu.device_information();

        GpuSpecs {
            is_software_emulated: info.is_software_emulated,
            device_name: info.device_name.clone(),
            driver_name: info.driver_name.clone(),
            driver_info: info.driver_info.clone(),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn layer(&self) -> metal::MetalLayer {
        unsafe { foreign_types::ForeignType::from_ptr(self.layer_ptr()) }
    }

    #[cfg(target_os = "macos")]
    pub fn layer_ptr(&self) -> *mut metal::CAMetalLayer {
        objc2::rc::Retained::as_ptr(&self.surface.metal_layer()) as *mut _
    }

    #[profiling::function]
    fn draw_paths_to_intermediate(
        &mut self,
        paths: &[Path<ScaledPixels>],
        width: f32,
        height: f32,
    ) {
        let targets = self.path_targets();
        self.command_encoder.init_texture(targets.texture);
        if let Some((msaa_texture, _)) = targets.msaa {
            self.command_encoder.init_texture(msaa_texture);
        }

        let target = if let Some((_, msaa_view)) = targets.msaa {
            gpu::RenderTarget {
                view: msaa_view,
                init_op: gpu::InitOp::Clear(gpu::TextureColor::TransparentBlack),
                finish_op: gpu::FinishOp::ResolveTo(targets.view),
            }
        } else {
            gpu::RenderTarget {
                view: targets.view,
                init_op: gpu::InitOp::Clear(gpu::TextureColor::TransparentBlack),
                finish_op: gpu::FinishOp::Store,
            }
        };
        if let mut pass = self.command_encoder.render(
            "rasterize paths",
            gpu::RenderTargetSet {
                colors: &[target],
                depth_stencil: None,
            },
        ) {
            let globals = GlobalParams {
                viewport_size: [width, height],
                premultiplied_alpha: 0,
                pad: 0,
            };
            let mut encoder = pass.with(&self.pipelines.path_rasterization);

            let mut vertices = Vec::new();
            for path in paths {
                vertices.extend(path.vertices.iter().map(|v| PathRasterizationVertex {
                    xy_position: v.xy_position,
                    st_position: v.st_position,
                    color: path.color,
                    bounds: path.clipped_bounds(),
                }));
            }
            let vertex_buf = unsafe { self.instance_belt.alloc_typed(&vertices, &self.gpu) };
            encoder.bind(
                0,
                &ShaderPathRasterizationData {
                    globals,
                    b_path_vertices: vertex_buf,
                },
            );
            encoder.draw(0, vertices.len() as u32, 0, 1);
        }
    }

    pub fn destroy(&mut self) {
        self.wait_for_gpu();
        // (The atlas is the context's, kept for the next window.)
        self.gpu.destroy_sampler(self.atlas_sampler);
        self.instance_belt.destroy(&self.gpu);
        self.gpu.destroy_command_encoder(&mut self.command_encoder);
        // (The pipelines are the context's, kept for the next window.)
        self.gpu.destroy_surface(&mut self.surface);
        self.destroy_path_targets();
    }

    /// Whether this GPU can draw LCD subpixel glyphs.
    pub fn supports_subpixel_text(&self) -> bool {
        self.gpu.capabilities().dual_source_blending
    }

    /// Makes the pipeline for LCD subpixel glyphs, if needed and possible.
    fn prepare_subpixel_sprites(&mut self, scene: &Scene) {
        use gpu::ShaderData as _;
        if self.subpixel_sprites.is_some()
            || !self.supports_subpixel_text()
            || !scene
                .monochrome_sprites
                .iter()
                .any(|s| s.tile.texture_id.kind == crate::AtlasTextureKind::Subpixel)
        {
            return;
        }
        let info = self.surface.info();
        let cache = self.pipeline_cache.clone();
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, pipeline)) = cache
            .subpixel
            .iter()
            .find(|(format, _)| *format == info.format)
        {
            self.subpixel_sprites = Some(pipeline.clone());
            return;
        }
        let source = format!(
            "enable dual_source_blending;\n{}\n{}",
            include_str!("shaders.wgsl"),
            include_str!("subpixel.wgsl")
        );
        let Ok(shader) = self
            .gpu
            .try_create_shader(gpu::ShaderDesc { source: &source })
        else {
            log::error!("the LCD subpixel glyph shader didn't compile");
            return;
        };
        let pipeline = Arc::new(self.gpu.create_render_pipeline(gpu::RenderPipelineDesc {
            name: "subpixel-sprites",
            data_layouts: &[&ShaderMonoSpritesData::layout()],
            vertex: shader.at("vs_mono_sprite"),
            vertex_fetches: &[],
            primitive: gpu::PrimitiveState {
                topology: gpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            fragment: Some(shader.at("fs_subpixel_sprite")),
            color_targets: &[gpu::ColorTargetState {
                format: info.format,
                blend: Some(gpu::BlendState {
                    color: gpu::BlendComponent {
                        src_factor: gpu::BlendFactor::One,
                        dst_factor: gpu::BlendFactor::OneMinusSrc1,
                        operation: gpu::BlendOperation::Add,
                    },
                    alpha: gpu::BlendComponent {
                        src_factor: gpu::BlendFactor::One,
                        dst_factor: gpu::BlendFactor::OneMinusSrc1Alpha,
                        operation: gpu::BlendOperation::Add,
                    },
                }),
                write_mask: gpu::ColorWrites::default(),
            }],
            multisample_state: gpu::MultisampleState::default(),
        }));
        cache.subpixel.push((info.format, pipeline.clone()));
        self.subpixel_sprites = Some(pipeline);
    }

    /// The textures paths are drawn into, made if not yet.
    fn path_targets(&mut self) -> PathTargets {
        if let Some(targets) = self.path_targets {
            return targets;
        }
        let format = self.surface.info().format;
        let size = self.surface_config.size;
        let (texture, view) =
            create_path_intermediate_texture(&self.gpu, format, size.width, size.height);
        let msaa = create_msaa_texture_if_needed(
            &self.gpu,
            format,
            size.width,
            size.height,
            self.rendering_parameters.path_sample_count,
        );
        let targets = PathTargets {
            texture,
            view,
            msaa,
        };
        self.path_targets = Some(targets);
        targets
    }

    fn destroy_path_targets(&mut self) {
        if let Some(targets) = self.path_targets.take() {
            self.gpu.destroy_texture(targets.texture);
            self.gpu.destroy_texture_view(targets.view);
            if let Some((msaa_texture, msaa_view)) = targets.msaa {
                self.gpu.destroy_texture(msaa_texture);
                self.gpu.destroy_texture_view(msaa_view);
            }
        }
    }

    pub fn draw(&mut self, scene: &Scene) {
        // Olo: nothing to draw with (a driver reset); the program starts
        // anew at the end of this turn of its loop.
        if gpu::device_lost() {
            return;
        }
        self.prepare_subpixel_sprites(scene);
        self.command_encoder.start();
        if super::gpu_timings() {
            // (The passes of the frame this command buffer last drew.)
            let t = self.command_encoder.timings();
            if !t.is_empty() {
                let total: std::time::Duration = t.iter().map(|(_, d)| *d).sum();
                let passes: Vec<String> = t
                    .iter()
                    .map(|(n, d)| format!("{n} {:.3}", d.as_secs_f64() * 1e3))
                    .collect();
                // What this frame fills: its quads' area, in screens.
                let screen = (self.surface_config.size.width * self.surface_config.size.height)
                    .max(1) as f64;
                let area =
                    |b: &Bounds<ScaledPixels>| (b.size.width.0 * b.size.height.0).max(0.0) as f64;
                let quads: f64 = scene
                    .quads
                    .iter()
                    .map(|q| area(&q.bounds.intersect(&q.content_mask.bounds)))
                    .sum::<f64>()
                    / screen;
                let big: Vec<String> = scene
                    .quads
                    .iter()
                    .filter(|q| area(&q.bounds.intersect(&q.content_mask.bounds)) > screen * 0.05)
                    .map(|q| {
                        let b = q.bounds.intersect(&q.content_mask.bounds);
                        format!(
                            "[{:.0}×{:.0} at {:.0},{:.0} order {} {:?} alpha {:.7}{}]",
                            b.size.width.0,
                            b.size.height.0,
                            b.origin.x.0,
                            b.origin.y.0,
                            q.order,
                            q.background.tag,
                            q.background.solid.a,
                            if q.corner_radii != crate::Corners::default()
                                || q.border_widths != crate::Edges::default()
                            {
                                format!(" radii {:?} borders {:?}", q.corner_radii, q.border_widths)
                            } else {
                                String::new()
                            }
                        )
                    })
                    .collect();
                eprintln!(
                    "olo gpu frame {:.3} ms: {} | quads {} ({quads:.2} screens) sprites {} {}",
                    total.as_secs_f64() * 1e3,
                    passes.join(", "),
                    scene.quads.len(),
                    scene.monochrome_sprites.len() + scene.polychrome_sprites.len(),
                    big.join(" ")
                );
            }
        }
        self.atlas.before_frame(&mut self.command_encoder);

        let frame = {
            profiling::scope!("acquire frame");
            self.surface.acquire_frame()
        };
        // Olo: no image to draw into (the surface is lost): what's
        // recorded (the atlas's uploads) goes, nothing is drawn, and the
        // surface is made again for the next frame.
        if frame.is_lost() {
            let sync_point = self.gpu.submit(&mut self.command_encoder);
            self.instance_belt.flush(&sync_point);
            self.atlas.after_frame(&sync_point);
            self.wait_for_gpu();
            self.last_sync_point = Some(sync_point);
            self.remake_surface();
            return;
        }
        self.command_encoder.init_texture(frame.texture());

        let globals = GlobalParams {
            viewport_size: [
                self.surface_config.size.width as f32,
                self.surface_config.size.height as f32,
            ],
            premultiplied_alpha: match self.surface.info().alpha {
                gpu::AlphaMode::Ignored | gpu::AlphaMode::PostMultiplied => 0,
                gpu::AlphaMode::PreMultiplied => 1,
            },
            pad: u32::from(self.display_p3),
        };

        let mut pass = self.command_encoder.render(
            "main",
            gpu::RenderTargetSet {
                colors: &[gpu::RenderTarget {
                    view: frame.texture_view(),
                    init_op: gpu::InitOp::Clear(gpu::TextureColor::TransparentBlack),
                    finish_op: gpu::FinishOp::Store,
                }],
                depth_stencil: None,
            },
        );

        profiling::scope!("render pass");
        for batch in scene.batches() {
            match batch {
                PrimitiveBatch::Quads(quads) => {
                    let instance_buf = unsafe { self.instance_belt.alloc_typed(quads, &self.gpu) };
                    let mut encoder = pass.with(&self.pipelines.quads);
                    encoder.bind(
                        0,
                        &ShaderQuadsData {
                            globals,
                            b_quads: instance_buf,
                        },
                    );
                    encoder.draw(0, 4, 0, quads.len() as u32);
                }
                PrimitiveBatch::Shadows(shadows) => {
                    let instance_buf =
                        unsafe { self.instance_belt.alloc_typed(shadows, &self.gpu) };
                    let mut encoder = pass.with(&self.pipelines.shadows);
                    encoder.bind(
                        0,
                        &ShaderShadowsData {
                            globals,
                            b_shadows: instance_buf,
                        },
                    );
                    encoder.draw(0, 4, 0, shadows.len() as u32);
                }
                PrimitiveBatch::Paths(paths) => {
                    let Some(first_path) = paths.first() else {
                        continue;
                    };
                    drop(pass);
                    self.draw_paths_to_intermediate(
                        paths,
                        self.surface_config.size.width as f32,
                        self.surface_config.size.height as f32,
                    );
                    let path_view = self.path_targets().view;
                    pass = self.command_encoder.render(
                        "main",
                        gpu::RenderTargetSet {
                            colors: &[gpu::RenderTarget {
                                view: frame.texture_view(),
                                init_op: gpu::InitOp::Load,
                                finish_op: gpu::FinishOp::Store,
                            }],
                            depth_stencil: None,
                        },
                    );
                    let mut encoder = pass.with(&self.pipelines.paths);
                    // When copying paths from the intermediate texture to the drawable,
                    // each pixel must only be copied once, in case of transparent paths.
                    //
                    // If all paths have the same draw order, then their bounds are all
                    // disjoint, so we can copy each path's bounds individually. If this
                    // batch combines different draw orders, we perform a single copy
                    // for a minimal spanning rect.
                    let sprites = if paths.last().unwrap().order == first_path.order {
                        paths
                            .iter()
                            .map(|path| PathSprite {
                                bounds: path.clipped_bounds(),
                            })
                            .collect()
                    } else {
                        let mut bounds = first_path.clipped_bounds();
                        for path in paths.iter().skip(1) {
                            bounds = bounds.union(&path.clipped_bounds());
                        }
                        vec![PathSprite { bounds }]
                    };
                    let instance_buf =
                        unsafe { self.instance_belt.alloc_typed(&sprites, &self.gpu) };
                    encoder.bind(
                        0,
                        &ShaderPathsData {
                            globals,
                            t_sprite: path_view,
                            s_sprite: self.atlas_sampler,
                            b_path_sprites: instance_buf,
                        },
                    );
                    encoder.draw(0, 4, 0, sprites.len() as u32);
                }
                PrimitiveBatch::Underlines(underlines) => {
                    let instance_buf =
                        unsafe { self.instance_belt.alloc_typed(underlines, &self.gpu) };
                    let mut encoder = pass.with(&self.pipelines.underlines);
                    encoder.bind(
                        0,
                        &ShaderUnderlinesData {
                            globals,
                            b_underlines: instance_buf,
                        },
                    );
                    encoder.draw(0, 4, 0, underlines.len() as u32);
                }
                PrimitiveBatch::Segments(segments) => {
                    let instance_buf =
                        unsafe { self.instance_belt.alloc_typed(segments, &self.gpu) };
                    let mut encoder = pass.with(&self.pipelines.segments);
                    encoder.bind(
                        0,
                        &ShaderSegmentsData {
                            globals,
                            b_segments: instance_buf,
                        },
                    );
                    encoder.draw(0, 4, 0, segments.len() as u32);
                }
                PrimitiveBatch::Sectors(sectors) => {
                    let instance_buf =
                        unsafe { self.instance_belt.alloc_typed(sectors, &self.gpu) };
                    let mut encoder = pass.with(&self.pipelines.sectors);
                    encoder.bind(
                        0,
                        &ShaderSectorsData {
                            globals,
                            b_sectors: instance_buf,
                        },
                    );
                    encoder.draw(0, 4, 0, sectors.len() as u32);
                }
                PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    sprites,
                } => {
                    let tex_info = self.atlas.get_texture_info(texture_id);
                    let instance_buf =
                        unsafe { self.instance_belt.alloc_typed(sprites, &self.gpu) };
                    let pipeline = match &self.subpixel_sprites {
                        Some(subpixel) if texture_id.kind == crate::AtlasTextureKind::Subpixel => {
                            &**subpixel
                        }
                        _ => &self.pipelines.mono_sprites,
                    };
                    let mut encoder = pass.with(pipeline);
                    encoder.bind(
                        0,
                        &ShaderMonoSpritesData {
                            globals,
                            gamma_ratios: self.rendering_parameters.gamma_ratios,
                            grayscale_enhanced_contrast: self
                                .rendering_parameters
                                .grayscale_enhanced_contrast,
                            t_sprite: tex_info.raw_view,
                            s_sprite: self.atlas_sampler,
                            b_mono_sprites: instance_buf,
                        },
                    );
                    encoder.draw(0, 4, 0, sprites.len() as u32);
                }
                PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    sprites,
                } => {
                    let tex_info = self.atlas.get_texture_info(texture_id);
                    let instance_buf =
                        unsafe { self.instance_belt.alloc_typed(sprites, &self.gpu) };
                    let mut encoder = pass.with(&self.pipelines.poly_sprites);
                    encoder.bind(
                        0,
                        &ShaderPolySpritesData {
                            globals,
                            t_sprite: tex_info.raw_view,
                            s_sprite: self.atlas_sampler,
                            b_poly_sprites: instance_buf,
                        },
                    );
                    encoder.draw(0, 4, 0, sprites.len() as u32);
                }
                PrimitiveBatch::ImageTriangles {
                    texture_id,
                    triangles,
                } => {
                    let tex_info = self.atlas.get_texture_info(texture_id);
                    let instance_buf =
                        unsafe { self.instance_belt.alloc_typed(triangles, &self.gpu) };
                    let mut encoder = pass.with(&self.pipelines.image_triangles);
                    encoder.bind(
                        0,
                        &ShaderImageTrianglesData {
                            globals,
                            t_sprite: tex_info.raw_view,
                            s_sprite: self.atlas_sampler,
                            b_image_triangles: instance_buf,
                        },
                    );
                    encoder.draw(0, 3, 0, triangles.len() as u32);
                }
                PrimitiveBatch::Surfaces(surfaces) => {
                    let mut _encoder = pass.with(&self.pipelines.surfaces);

                    for surface in surfaces {
                        #[cfg(not(target_os = "macos"))]
                        {
                            let _ = surface;
                            continue;
                        };

                        #[cfg(target_os = "macos")]
                        {
                            let (t_y, t_cb_cr) = unsafe {
                                use core_foundation::base::TCFType as _;
                                use std::ptr;

                                assert_eq!(
                                        surface.image_buffer.get_pixel_format(),
                                        core_video::pixel_buffer::kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
                                    );

                                let y_texture = self
                                    .core_video_texture_cache
                                    .create_texture_from_image(
                                        surface.image_buffer.as_concrete_TypeRef(),
                                        ptr::null(),
                                        metal::MTLPixelFormat::R8Unorm,
                                        surface.image_buffer.get_width_of_plane(0),
                                        surface.image_buffer.get_height_of_plane(0),
                                        0,
                                    )
                                    .unwrap();
                                let cb_cr_texture = self
                                    .core_video_texture_cache
                                    .create_texture_from_image(
                                        surface.image_buffer.as_concrete_TypeRef(),
                                        ptr::null(),
                                        metal::MTLPixelFormat::RG8Unorm,
                                        surface.image_buffer.get_width_of_plane(1),
                                        surface.image_buffer.get_height_of_plane(1),
                                        1,
                                    )
                                    .unwrap();
                                (
                                    gpu::TextureView::from_metal_texture(
                                        &objc2::rc::Retained::retain(
                                            foreign_types::ForeignTypeRef::as_ptr(
                                                y_texture.as_texture_ref(),
                                            )
                                                as *mut objc2::runtime::ProtocolObject<
                                                    dyn objc2_metal::MTLTexture,
                                                >,
                                        )
                                        .unwrap(),
                                        gpu::TexelAspects::COLOR,
                                    ),
                                    gpu::TextureView::from_metal_texture(
                                        &objc2::rc::Retained::retain(
                                            foreign_types::ForeignTypeRef::as_ptr(
                                                cb_cr_texture.as_texture_ref(),
                                            )
                                                as *mut objc2::runtime::ProtocolObject<
                                                    dyn objc2_metal::MTLTexture,
                                                >,
                                        )
                                        .unwrap(),
                                        gpu::TexelAspects::COLOR,
                                    ),
                                )
                            };

                            _encoder.bind(
                                0,
                                &ShaderSurfacesData {
                                    globals,
                                    surface_locals: SurfaceParams {
                                        bounds: surface.bounds.into(),
                                        content_mask: surface.content_mask.bounds.into(),
                                    },
                                    t_y,
                                    t_cb_cr,
                                    s_surface: self.atlas_sampler,
                                },
                            );

                            _encoder.draw(0, 4, 0, 1);
                        }
                    }
                }
            }
        }
        drop(pass);

        self.command_encoder.present(frame);
        let sync_point = self.gpu.submit(&mut self.command_encoder);

        profiling::scope!("finish");
        self.instance_belt.flush(&sync_point);
        self.atlas.after_frame(&sync_point);

        self.wait_for_gpu();
        self.last_sync_point = Some(sync_point);
    }
}

#[derive(Clone, Copy)]
struct PathTargets {
    texture: gpu::Texture,
    view: gpu::TextureView,
    msaa: Option<(gpu::Texture, gpu::TextureView)>,
}

fn create_path_intermediate_texture(
    gpu: &gpu::Context,
    format: gpu::TextureFormat,
    width: u32,
    height: u32,
) -> (gpu::Texture, gpu::TextureView) {
    let texture = gpu.create_texture(gpu::TextureDesc {
        name: "path intermediate",
        format,
        size: gpu::Extent {
            width,
            height,
            depth: 1,
        },
        array_layer_count: 1,
        mip_level_count: 1,
        sample_count: 1,
        dimension: gpu::TextureDimension::D2,
        usage: gpu::TextureUsage::COPY | gpu::TextureUsage::RESOURCE | gpu::TextureUsage::TARGET,
        external: None,
    });
    let texture_view = gpu.create_texture_view(
        texture,
        gpu::TextureViewDesc {
            name: "path intermediate view",
            format,
            dimension: gpu::ViewDimension::D2,
            subresources: &Default::default(),
        },
    );
    (texture, texture_view)
}

fn create_msaa_texture_if_needed(
    gpu: &gpu::Context,
    format: gpu::TextureFormat,
    width: u32,
    height: u32,
    sample_count: u32,
) -> Option<(gpu::Texture, gpu::TextureView)> {
    if sample_count <= 1 {
        return None;
    }
    let texture_msaa = gpu.create_texture(gpu::TextureDesc {
        name: "path intermediate msaa",
        format,
        size: gpu::Extent {
            width,
            height,
            depth: 1,
        },
        array_layer_count: 1,
        mip_level_count: 1,
        sample_count,
        dimension: gpu::TextureDimension::D2,
        usage: gpu::TextureUsage::TARGET,
        external: None,
    });
    let texture_view_msaa = gpu.create_texture_view(
        texture_msaa,
        gpu::TextureViewDesc {
            name: "path intermediate msaa view",
            format,
            dimension: gpu::ViewDimension::D2,
            subresources: &Default::default(),
        },
    );

    Some((texture_msaa, texture_view_msaa))
}

/// A set of parameters that can be set using a corresponding environment variable.
struct RenderingParameters {
    // Env var: ZED_PATH_SAMPLE_COUNT
    // workaround for https://github.com/zed-industries/zed/issues/26143
    path_sample_count: u32,

    // Env var: ZED_FONTS_GAMMA
    // Allowed range [1.0, 2.2], other values are clipped
    // Default: 1.8
    gamma_ratios: [f32; 4],
    // Env var: ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST
    // Allowed range: [0.0, ..), other values are clipped
    // Default: 1.0
    grayscale_enhanced_contrast: f32,
}

impl RenderingParameters {
    fn from_env(context: &BladeContext) -> Self {
        use std::env;

        let path_sample_count = env::var("ZED_PATH_SAMPLE_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .or_else(|| {
                [4, 2, 1]
                    .into_iter()
                    .find(|&n| (context.gpu.capabilities().sample_count_mask & n) != 0)
            })
            .unwrap_or(1);
        let gamma = env::var("ZED_FONTS_GAMMA")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.8_f32)
            .clamp(1.0, 2.2);
        let gamma_ratios = Self::get_gamma_ratios(gamma);
        let grayscale_enhanced_contrast = env::var("ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0_f32)
            .max(0.0);

        Self {
            path_sample_count,
            gamma_ratios,
            grayscale_enhanced_contrast,
        }
    }

    // Gamma ratios for brightening/darkening edges for better contrast
    // https://github.com/microsoft/terminal/blob/1283c0f5b99a2961673249fa77c6b986efb5086c/src/renderer/atlas/dwrite.cpp#L50
    fn get_gamma_ratios(gamma: f32) -> [f32; 4] {
        const GAMMA_INCORRECT_TARGET_RATIOS: [[f32; 4]; 13] = [
            [0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0], // gamma = 1.0
            [0.0166 / 4.0, -0.0807 / 4.0, 0.2227 / 4.0, -0.0751 / 4.0], // gamma = 1.1
            [0.0350 / 4.0, -0.1760 / 4.0, 0.4325 / 4.0, -0.1370 / 4.0], // gamma = 1.2
            [0.0543 / 4.0, -0.2821 / 4.0, 0.6302 / 4.0, -0.1876 / 4.0], // gamma = 1.3
            [0.0739 / 4.0, -0.3963 / 4.0, 0.8167 / 4.0, -0.2287 / 4.0], // gamma = 1.4
            [0.0933 / 4.0, -0.5161 / 4.0, 0.9926 / 4.0, -0.2616 / 4.0], // gamma = 1.5
            [0.1121 / 4.0, -0.6395 / 4.0, 1.1588 / 4.0, -0.2877 / 4.0], // gamma = 1.6
            [0.1300 / 4.0, -0.7649 / 4.0, 1.3159 / 4.0, -0.3080 / 4.0], // gamma = 1.7
            [0.1469 / 4.0, -0.8911 / 4.0, 1.4644 / 4.0, -0.3234 / 4.0], // gamma = 1.8
            [0.1627 / 4.0, -1.0170 / 4.0, 1.6051 / 4.0, -0.3347 / 4.0], // gamma = 1.9
            [0.1773 / 4.0, -1.1420 / 4.0, 1.7385 / 4.0, -0.3426 / 4.0], // gamma = 2.0
            [0.1908 / 4.0, -1.2652 / 4.0, 1.8650 / 4.0, -0.3476 / 4.0], // gamma = 2.1
            [0.2031 / 4.0, -1.3864 / 4.0, 1.9851 / 4.0, -0.3501 / 4.0], // gamma = 2.2
        ];

        const NORM13: f32 = ((0x10000 as f64) / (255.0 * 255.0) * 4.0) as f32;
        const NORM24: f32 = ((0x100 as f64) / (255.0) * 4.0) as f32;

        let index = ((gamma * 10.0).round() as usize).clamp(10, 22) - 10;
        let ratios = GAMMA_INCORRECT_TARGET_RATIOS[index];

        [
            ratios[0] * NORM13,
            ratios[1] * NORM24,
            ratios[2] * NORM13,
            ratios[3] * NORM24,
        ]
    }
}
