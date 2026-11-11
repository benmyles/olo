mod blade;
mod keyboard;
mod keystroke;
mod linux;

use crate::{
    Bounds, DevicePixels, Font, FontId, FontMetrics, FontRun, GlyphId, LineLayout, Pixels, PlatformInput, Point, RenderGlyphParams, RenderImage, RenderImageParams, RenderSvgParams, Scene,
    ShapedGlyph, ShapedRun, SharedString, Size, SvgRenderer, SvgSize, TaskLabel, hash, point, px, size,
};
use anyhow::Result;
use async_task::Runnable;
use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder as _, Frame};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use schemars::JsonSchema;
use seahash::SeaHasher;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::ops;
use std::time::{Duration, Instant};
use std::{
    fmt::{self, Debug},
    ops::Range,
    rc::Rc,
    sync::Arc,
};
use strum::EnumIter;
use uuid::Uuid;

pub use keyboard::*;
pub use keystroke::*;

pub(crate) use blade::*;
pub use linux::ImeInput;
pub(crate) use linux::*;
pub(crate) use linux::{lock_session_on_display, unlock_session_on_display};

/// Whether the GPU device was lost (a driver reset): nothing more can be
/// drawn with it, and the program starts again with its state.
pub fn gpu_device_lost() -> bool {
    blade_graphics::device_lost()
}

/// Marks the GPU device lost, as a driver reset would: to test what
/// follows.
pub fn lose_gpu_device() {
    blade_graphics::lose_device();
}

/// The size a window opens at when nothing says.
pub(crate) const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1536.), px(864.));

/// Which GPU draws, as Vulkan names it.
#[derive(Debug, Clone, Default)]
pub struct GpuSpecs {
    /// Whether the GPU is really a fake (like `llvmpipe`) running on the CPU.
    pub is_software_emulated: bool,
    /// The name of the device, as reported by Vulkan.
    pub device_name: String,
    /// The name of the driver, as reported by Vulkan.
    pub driver_name: String,
    /// Further information about the driver, as reported by Vulkan.
    pub driver_info: String,
}

/// What came of handing an event to a window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchEventResult {
    /// Nothing stopped it.
    pub propagate: bool,
    /// Something said its default isn't to happen.
    pub default_prevented: bool,
}

/// A window, by number: what the display thread and the loopers know it
/// by.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub(crate) u64);

impl WindowId {
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// A handle to a platform's display, e.g. a monitor or laptop screen.
pub trait PlatformDisplay: Send + Sync + Debug {
    /// Get the ID for this display
    fn id(&self) -> DisplayId;

    /// Returns a stable identifier for this display that can be persisted and used
    /// across system restarts.
    fn uuid(&self) -> Result<Uuid>;

    /// Get the bounds for this display
    fn bounds(&self) -> Bounds<Pixels>;

    /// Olo: its name (the output's connector, "DP-1"), if it says.
    fn name(&self) -> Option<String> {
        None
    }

    /// Get the default bounds for this display to place a window
    fn default_bounds(&self) -> Bounds<Pixels> {
        let bounds = self.bounds();
        let center = bounds.center();
        let clipped_window_size = DEFAULT_WINDOW_SIZE.min(&bounds.size);

        let offset = clipped_window_size / 2.0;
        let origin = point(center.x - offset.width, center.y - offset.height);
        Bounds::new(origin, clipped_window_size)
    }
}

/// An opaque identifier for a hardware display
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
pub struct DisplayId(pub(crate) u32);

impl From<DisplayId> for u32 {
    fn from(id: DisplayId) -> Self {
        id.0
    }
}

impl Debug for DisplayId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DisplayId({})", self.0)
    }
}

/// Which part of the window to resize
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    /// The top edge
    Top,
    /// The top right corner
    TopRight,
    /// The right edge
    Right,
    /// The bottom right corner
    BottomRight,
    /// The bottom edge
    Bottom,
    /// The bottom left corner
    BottomLeft,
    /// The left edge
    Left,
    /// The top left corner
    TopLeft,
}

/// A type to describe the appearance of a window
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default)]
pub enum WindowDecorations {
    #[default]
    /// Server side decorations
    Server,
    /// Client side decorations
    Client,
}

/// A type to describe how this window is currently configured
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default)]
pub enum Decorations {
    /// The window is configured to use server side decorations
    #[default]
    Server,
    /// The window is configured to use client side decorations
    Client {
        /// The edge tiling state
        tiling: Tiling,
    },
}

/// What window controls this platform supports
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct WindowControls {
    /// Whether this platform supports fullscreen
    pub fullscreen: bool,
    /// Whether this platform supports maximize
    pub maximize: bool,
    /// Whether this platform supports minimize
    pub minimize: bool,
    /// Whether this platform supports a window menu
    pub window_menu: bool,
}

impl Default for WindowControls {
    fn default() -> Self {
        // Assume that we can do anything, unless told otherwise
        Self { fullscreen: true, maximize: true, minimize: true, window_menu: true }
    }
}

/// A type to describe which sides of the window are currently tiled in some way
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default)]
pub struct Tiling {
    /// Whether the top edge is tiled
    pub top: bool,
    /// Whether the left edge is tiled
    pub left: bool,
    /// Whether the right edge is tiled
    pub right: bool,
    /// Whether the bottom edge is tiled
    pub bottom: bool,
}

impl Tiling {
    /// Initializes a [`Tiling`] type with all sides tiled
    pub fn tiled() -> Self {
        Self { top: true, left: true, right: true, bottom: true }
    }

    /// Whether any edge is tiled
    pub fn is_tiled(&self) -> bool {
        self.top || self.left || self.right || self.bottom
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub(crate) struct RequestFrameOptions {
    pub(crate) require_presentation: bool,
    /// Force refresh of all rendering states when true
    pub(crate) force_render: bool,
}

pub(crate) trait PlatformWindow: HasWindowHandle + HasDisplayHandle {
    fn bounds(&self) -> Bounds<Pixels>;
    fn is_maximized(&self) -> bool;
    fn window_bounds(&self) -> WindowBounds;
    fn content_size(&self) -> Size<Pixels>;
    fn resize(&mut self, size: Size<Pixels>);
    fn scale_factor(&self) -> f32;
    fn appearance(&self) -> WindowAppearance;
    fn display(&self) -> Option<Rc<dyn PlatformDisplay>>;
    fn mouse_position(&self) -> Point<Pixels>;
    fn modifiers(&self) -> Modifiers;
    fn capslock(&self) -> Capslock;
    fn activate(&self);
    /// Brings the window forward with an activation token another
    /// program gave (a notification clicked): unlike [`activate`](Self::activate),
    /// the compositor lets it take the focus.
    fn activate_with_token(&self, _token: &str) {
        self.activate()
    }
    fn is_active(&self) -> bool;
    fn is_hovered(&self) -> bool;
    fn set_title(&mut self, title: &str);
    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance);
    /// Olo: keeps the screen from dimming, locking or sleeping while the
    /// window shows (a playing video), until called with `false`.
    fn set_keep_awake(&self, _on: bool) {}
    /// Olo: an activation token made with this window (and the last
    /// press in it), for another program to take the focus with (a
    /// notification's app, as its action is invoked); None if there is
    /// no activation protocol.
    fn activation_token(&self, done: Box<dyn FnOnce(Option<String>)>) {
        done(None)
    }
    /// Olo: a handle another program's window can name to open over this
    /// one (xdg-foreign); None if the compositor can't export windows.
    fn export_handle(&self, done: Box<dyn FnOnce(Option<String>)>) {
        done(None)
    }
    /// Olo: whether a layer surface takes the keyboard (as its
    /// `LayerShellOptions::keyboard`), changed after it opened.
    fn set_layer_keyboard(&self, _keyboard: LayerKeyboard) {}
    fn set_layer_margin(&self, _margin: (i32, i32, i32, i32)) {}
    /// Olo: where the window takes what's dragged over it now (None:
    /// nowhere), as its last frame found.
    fn set_drop_accept(&self, _accept: Option<DropAccept>) {}
    /// Olo: where each of the window's external pictures shows
    /// ([`crate::Window::place_external`]), in the frame about to be
    /// presented: those not listed are hidden. Called just before `draw`.
    fn place_externals(&self, _places: &[(u64, Bounds<Pixels>)]) {}
    /// Olo: the picture `key` shows from now on
    /// ([`crate::Window::set_external_buffer`]); the one it replaces is
    /// released once the compositor is done with it. None: `key` is let
    /// go with all its buffers.
    fn set_external_buffer(&self, _key: u64, buffer: Option<ExternalBuffer>) {
        if let Some(b) = buffer {
            (b.released)(b.id)
        }
    }
    fn minimize(&self);
    fn zoom(&self);
    fn toggle_fullscreen(&self);
    /// Olo: close as the compositor's close does (the window's
    /// should-close hook asked first: its confirmations), on the event
    /// loop's next turn, outside any update of the window.
    fn request_close(&self) {}
    fn is_fullscreen(&self) -> bool;
    /// The callback draws a frame if anything changed, and says if it did.
    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions) -> bool>);
    /// Something to call when the window has something new to draw, for
    /// platforms that draw only then (Olo's Wayland).
    fn frame_waker(&self) -> Option<Rc<dyn Fn()>> {
        None
    }
    /// Draws the window's next frame now, though the compositor hasn't
    /// said the last is showing (it may never, for a window out of
    /// sight): for a frame wanted for itself, as a layout review's (Olo).
    fn draw_now(&self) {}
    /// How to draw text for the display the window is on (Olo).
    fn text_rendering(&self) -> crate::TextRendering {
        crate::TextRendering::default()
    }
    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput)>);
    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>);
    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>);
    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>);
    fn on_moved(&self, callback: Box<dyn FnMut()>);
    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>);
    fn on_close(&self, callback: Box<dyn FnOnce()>);
    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>);
    /// What an input method does is handed to `callback`.
    fn on_ime(&self, _callback: Box<dyn FnMut(ImeInput)>) {}
    /// Begins a frame that shows `damage` (what changed since the last,
    /// in device pixels; None: everything): how much of it is to be
    /// drawn. The scene then drawn need hold no more.
    fn begin_frame(&self, _damage: Option<Bounds<crate::ScaledPixels>>) -> FrameRegion {
        FrameRegion::All
    }
    fn draw(&self, scene: &Scene);
    fn completed_frame(&self) {}
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas>;

    // macOS specific methods
    fn get_title(&self) -> String {
        String::new()
    }

    // Linux specific methods
    fn inner_window_bounds(&self) -> WindowBounds {
        self.window_bounds()
    }
    fn start_window_move(&self) {}
    fn start_window_resize(&self, _edge: ResizeEdge) {}
    /// Olo: drags `drag` out of the window, under the pointer (only while
    /// a button is held), to wherever it's dropped; `done` gets the
    /// action it was dropped as, or None.
    fn start_drag(&self, _drag: OutgoingDrag, _done: futures::channel::oneshot::Sender<Option<DragAction>>) {}
    /// Olo: the window's surface's protocol id (Wayland's `wl_surface`),
    /// which app_server names it by.
    fn surface_id(&self) -> Option<u32> {
        None
    }
    fn window_decorations(&self) -> Decorations {
        Decorations::Server
    }
    /// Olo: which sides the compositor holds (xdg's tiled states: in a
    /// tile, maximized or full screen), whoever draws the decorations.
    fn tiling(&self) -> Tiling {
        Tiling::default()
    }
    fn set_app_id(&mut self, _app_id: &str) {}
    /// Olo: draw in `space` from the next frame, telling the compositor;
    /// whether it does (a compositor without colour management: sRGB).
    fn set_gamut(&mut self, _space: crate::Gamut) -> bool {
        false
    }
    /// Olo: whether it's on a display whose HDR is on
    /// (`crate::set_display_hdr`).
    fn on_hdr_display(&self) -> bool {
        false
    }
    fn map_window(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn gpu_specs(&self) -> Option<GpuSpecs>;

    fn update_ime_position(&self, _bounds: Bounds<Pixels>);

    /// Olo: the wheel's notches still easing in aren't dropped though
    /// the step just handed out drew nothing in its frame: what took it
    /// draws from another thread.
    fn hold_wheel(&self) {}
}

/// Olo: data dragged out of a window to another app or a folder (the
/// platform's drag and drop, Wayland's data device): the same data in
/// each type, most faithful first, the actions the place it's dropped on
/// may take, and a picture under the pointer.
#[derive(Clone, Debug, Default)]
pub struct OutgoingDrag {
    /// Each MIME type and the data in it.
    pub types: Vec<(String, Arc<[u8]>)>,
    /// It may be copied.
    pub copy: bool,
    /// It may be moved.
    pub move_: bool,
    /// Let the place it's dropped on ask (Move, Copy or Link).
    pub ask: bool,
    /// Drawn under the pointer.
    pub icon: Option<DragImage>,
}

/// Olo: a picture dragged under the pointer: premultiplied RGBA,
/// `width` × `height` device pixels at `scale`, the pointer at `hotspot`
/// (logical pixels from its top left).
#[derive(Clone, Debug)]
pub struct DragImage {
    /// Premultiplied RGBA, row by row.
    pub rgba: Arc<[u8]>,
    /// In device pixels.
    pub width: u32,
    /// In device pixels.
    pub height: u32,
    /// Device pixels to a logical pixel.
    pub scale: u32,
    /// Where the pointer is on it.
    pub hotspot: Point<Pixels>,
}

/// Olo: what a drag out was dropped as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragAction {
    /// Copied.
    Copy,
    /// Moved.
    Move,
    /// Asked about (a menu of Move, Copy or Link).
    Ask,
}

/// Olo: a picture in another program's GPU memory (a dmabuf: an Agent
/// Screen's frame), shown in a window by the compositor itself, never
/// copied ([`crate::Window::place_external`]).
#[derive(Clone)]
pub struct ExternalBuffer {
    /// Names the memory: the same id is the same memory (imported once),
    /// and is what `released` is told.
    pub id: u64,
    /// In pixels.
    pub width: u32,
    /// In pixels.
    pub height: u32,
    /// Its DRM format (`XR24`: BGRx).
    pub fourcc: u32,
    /// Its DRM format modifier (its layout in memory).
    pub modifier: u64,
    /// Its planes.
    pub planes: Vec<ExternalPlane>,
    /// Told the id once the compositor no longer reads the memory, to be
    /// drawn into again; called on any thread.
    pub released: Arc<dyn Fn(u64) + Send + Sync>,
}

impl std::fmt::Debug for ExternalBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalBuffer").field("id", &self.id).field("width", &self.width).field("height", &self.height).field("fourcc", &self.fourcc).field("modifier", &self.modifier).finish()
    }
}

/// Olo: one plane of an [`ExternalBuffer`].
#[derive(Clone, Debug)]
pub struct ExternalPlane {
    /// The dmabuf.
    pub fd: Arc<std::os::fd::OwnedFd>,
    /// Where the plane starts in it, in bytes.
    pub offset: u32,
    /// Bytes a row.
    pub stride: u32,
}

/// Olo: what is being dragged over a window ([`crate::Window::drop_offer`]),
/// from another program or one of this one's windows.
#[derive(Clone, Debug, Default)]
pub struct DropOffer {
    /// What it can be had as (MIME types).
    pub types: Vec<String>,
    /// What its source lets it be: copied, moved, asked about.
    pub copy: bool,
    /// Moved.
    pub move_: bool,
    /// Asked about (a menu of Move, Copy or Link).
    pub ask: bool,
    /// Started by this program (one of its windows dragged it out).
    pub from_us: bool,
    /// The files it carries (none if it carries none, or until they are
    /// read).
    pub paths: Vec<std::path::PathBuf>,
}

impl DropOffer {
    /// Whether it can be had as `mime`.
    pub fn has(&self, mime: &str) -> bool {
        self.types.iter().any(|t| t == mime)
    }

    /// Whether its source lets it be dropped as `action`.
    pub fn allows(&self, action: DragAction) -> bool {
        match action {
            DragAction::Copy => self.copy,
            DragAction::Move => self.move_,
            DragAction::Ask => self.ask,
        }
    }
}

/// Olo: where a window takes what is dragged over it, as a frame found
/// ([`crate::Window::accept_drop`]): the part's name, the type, how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropAccept {
    /// The part's name.
    pub target: String,
    /// The type it's taken as.
    pub mime: String,
    /// How.
    pub action: DragAction,
}

/// Olo: what was dropped on a window: the part that took it (its name),
/// the type it came as and its data, how, and where.
#[derive(Clone, Debug)]
pub struct Dropped {
    /// The part that took it.
    pub target: String,
    /// The type it came as.
    pub mime: String,
    /// Its data, as that type.
    pub data: Vec<u8>,
    /// How it was dropped.
    pub action: DragAction,
    /// Where in the window.
    pub position: Point<Pixels>,
    /// All the types it came as (an app's own type saying how it was
    /// dragged).
    pub types: Vec<String>,
    /// Dragged out of one of this program's windows.
    pub from_us: bool,
}

impl Dropped {
    /// The files it carries (`text/uri-list`).
    pub fn paths(&self) -> Vec<std::path::PathBuf> {
        if self.mime != "text/uri-list" {
            return Vec::new();
        }
        String::from_utf8_lossy(&self.data).lines().filter(|l| !l.starts_with('#')).filter_map(|l| file_uri_path(l.trim())).collect()
    }
}

/// Olo: whether the right Ctrl is held: app_server's command key (⌘),
/// which it sends as the right one (either Ctrl and Caps Lock are the
/// left), so a terminal can tell ⌘ from the program's Ctrl.
pub fn right_control_held() -> bool {
    RIGHT_CONTROL.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) static RIGHT_CONTROL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Olo: the path a `file://` URI names (its host, if any, left out; its
/// %-escapes undone).
pub fn file_uri_path(uri: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt as _;
    let rest = uri.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let (b, mut out, mut i) = (path.as_bytes(), Vec::with_capacity(path.len()), 0);
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (b.get(i + 1).and_then(|&c| hex(c)), b.get(i + 2).and_then(|&c| hex(c)))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(out)))
}

/// This type is public so that our test macro can generate and use it, but it should not
/// be considered part of our public API.
#[doc(hidden)]
pub trait PlatformDispatcher: Send + Sync {
    fn is_main_thread(&self) -> bool;
    fn dispatch(&self, runnable: Runnable, label: Option<TaskLabel>);
    fn dispatch_on_main_thread(&self, runnable: Runnable);
    fn dispatch_after(&self, duration: Duration, runnable: Runnable);
    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub(crate) trait PlatformTextSystem: Send + Sync {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()>;
    /// Olo: reads the system's fonts now (on the calling thread, not
    /// holding the text system while it reads), so the first text in a
    /// face only the system has doesn't wait for it.
    fn preload_system_fonts(&self) {}
    fn all_font_names(&self) -> Vec<String>;
    fn font_id(&self, descriptor: &Font) -> Result<FontId>;
    fn font_metrics(&self, font_id: FontId) -> FontMetrics;
    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>>;
    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>>;
    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId>;
    /// Olo: draws `chars` of `font_id` at `font_size` (at every subpixel
    /// offset) ahead of need, without holding up a frame: their bounds,
    /// the pixels kept for the frame that draws them.
    fn draw_ahead(&self, _font_id: FontId, _font_size: Pixels, _scale_factor: f32, _rendering: crate::TextRendering, _chars: &[char]) -> Vec<(RenderGlyphParams, Bounds<DevicePixels>)> {
        Vec::new()
    }
    /// Olo: draws the glyphs of `params` not yet drawn ahead of need
    /// (colour ones: emoji), without holding up a frame; their bounds.
    /// Colour glyphs drawn are kept for every window until
    /// [`forget_kept`](Self::forget_kept).
    fn draw_glyphs_ahead(&self, _params: &[RenderGlyphParams]) -> Vec<(RenderGlyphParams, Bounds<DevicePixels>)> {
        Vec::new()
    }
    /// Olo: lets go of the colour glyphs kept.
    fn forget_kept(&self) {}
    fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>>;
    fn rasterize_glyph(&self, params: &RenderGlyphParams, raster_bounds: Bounds<DevicePixels>) -> Result<(Size<DevicePixels>, Vec<u8>)>;
    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout;
}

pub(crate) struct NoopTextSystem;

impl NoopTextSystem {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self
    }
}

impl PlatformTextSystem for NoopTextSystem {
    fn add_fonts(&self, _fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        Ok(())
    }

    fn all_font_names(&self) -> Vec<String> {
        Vec::new()
    }

    fn font_id(&self, _descriptor: &Font) -> Result<FontId> {
        Ok(FontId(1))
    }

    fn font_metrics(&self, _font_id: FontId) -> FontMetrics {
        FontMetrics {
            units_per_em: 1000,
            ascent: 1025.0,
            descent: -275.0,
            line_gap: 0.0,
            underline_position: -95.0,
            underline_thickness: 60.0,
            cap_height: 698.0,
            x_height: 516.0,
            bounding_box: Bounds { origin: Point { x: -260.0, y: -245.0 }, size: Size { width: 1501.0, height: 1364.0 } },
        }
    }

    fn typographic_bounds(&self, _font_id: FontId, _glyph_id: GlyphId) -> Result<Bounds<f32>> {
        Ok(Bounds { origin: Point { x: 54.0, y: 0.0 }, size: size(392.0, 528.0) })
    }

    fn advance(&self, _font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        Ok(size(600.0 * glyph_id.0 as f32, 0.0))
    }

    fn glyph_for_char(&self, _font_id: FontId, ch: char) -> Option<GlyphId> {
        Some(GlyphId(ch.len_utf16() as u32))
    }

    fn glyph_raster_bounds(&self, _params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        Ok(Default::default())
    }

    fn rasterize_glyph(&self, _params: &RenderGlyphParams, raster_bounds: Bounds<DevicePixels>) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        Ok((raster_bounds.size, Vec::new()))
    }

    fn layout_line(&self, text: &str, font_size: Pixels, _runs: &[FontRun]) -> LineLayout {
        let mut position = px(0.);
        let metrics = self.font_metrics(FontId(0));
        let em_width = font_size * self.advance(FontId(0), self.glyph_for_char(FontId(0), 'm').unwrap()).unwrap().width / metrics.units_per_em as f32;
        let mut glyphs = Vec::new();
        for (ix, c) in text.char_indices() {
            if let Some(glyph) = self.glyph_for_char(FontId(0), c) {
                glyphs.push(ShapedGlyph { id: glyph, position: point(position, px(0.)), index: ix, is_emoji: glyph.0 == 2 });
                if glyph.0 == 2 {
                    position += em_width * 2.0;
                } else {
                    position += em_width;
                }
            } else {
                position += em_width
            }
        }
        let mut runs = Vec::default();
        if !glyphs.is_empty() {
            runs.push(ShapedRun { font_id: FontId(0), font_size, glyphs });
        } else {
            position = px(0.);
        }

        LineLayout {
            font_size,
            width: position,
            ascent: font_size * (metrics.ascent / metrics.units_per_em as f32),
            descent: font_size * (metrics.descent / metrics.units_per_em as f32),
            runs,
            len: text.len(),
            painted: Default::default(),
        }
    }
}

#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) enum AtlasKey {
    Glyph(RenderGlyphParams),
    Svg(RenderSvgParams),
    Image(RenderImageParams),
    /// Olo: a path's coverage (`crate::outline`).
    Outline(crate::outline::OutlineKey),
}

impl AtlasKey {
    pub(crate) fn texture_kind(&self) -> AtlasTextureKind {
        match self {
            AtlasKey::Glyph(params) => {
                if params.is_emoji {
                    AtlasTextureKind::Polychrome
                } else if matches!(params.rendering, crate::TextRendering::Subpixel(_)) {
                    AtlasTextureKind::Subpixel
                } else {
                    AtlasTextureKind::Monochrome
                }
            }
            AtlasKey::Svg(_) | AtlasKey::Outline(_) => AtlasTextureKind::Monochrome,
            AtlasKey::Image(params) if params.deep => AtlasTextureKind::Deep,
            AtlasKey::Image(_) => AtlasTextureKind::Polychrome,
        }
    }
}

impl From<RenderGlyphParams> for AtlasKey {
    fn from(params: RenderGlyphParams) -> Self {
        Self::Glyph(params)
    }
}

impl From<RenderSvgParams> for AtlasKey {
    fn from(params: RenderSvgParams) -> Self {
        Self::Svg(params)
    }
}

impl From<RenderImageParams> for AtlasKey {
    fn from(params: RenderImageParams) -> Self {
        Self::Image(params)
    }
}

pub(crate) trait PlatformAtlas: Send + Sync {
    fn get_or_insert_with<'a>(&self, key: &AtlasKey, build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>) -> Result<Option<AtlasTile>>;
    fn remove(&self, key: &AtlasKey);
}

struct AtlasTextureList<T> {
    textures: Vec<Option<T>>,
    free_list: Vec<usize>,
}

impl<T> Default for AtlasTextureList<T> {
    fn default() -> Self {
        Self { textures: Vec::default(), free_list: Vec::default() }
    }
}

impl<T> ops::Index<usize> for AtlasTextureList<T> {
    type Output = Option<T>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.textures[index]
    }
}

impl<T> AtlasTextureList<T> {
    #[allow(unused)]
    fn drain(&mut self) -> std::vec::Drain<'_, Option<T>> {
        self.free_list.clear();
        self.textures.drain(..)
    }

    #[allow(dead_code)]
    fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut T> {
        self.textures.iter_mut().flatten()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct AtlasTile {
    pub(crate) texture_id: AtlasTextureId,
    pub(crate) tile_id: TileId,
    pub(crate) padding: u32,
    pub(crate) bounds: Bounds<DevicePixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub(crate) struct AtlasTextureId {
    // We use u32 instead of usize for Metal Shader Language compatibility
    pub(crate) index: u32,
    pub(crate) kind: AtlasTextureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub(crate) enum AtlasTextureKind {
    Monochrome = 0,
    Polychrome = 1,
    /// Olo: glyphs with a coverage for each colour stripe (RGBA).
    Subpixel = 2,
    /// Olo: deep pictures, half floats a channel (`RenderImage::deep`).
    Deep = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub(crate) struct TileId(pub(crate) u32);

impl From<etagere::AllocId> for TileId {
    fn from(id: etagere::AllocId) -> Self {
        Self(id.serialize())
    }
}

impl From<TileId> for etagere::AllocId {
    fn from(id: TileId) -> Self {
        Self::deserialize(id.0)
    }
}

/// A struct representing a selection in a text buffer, in UTF16 characters.
/// This is different from a range because the head may be before the tail.
#[derive(Debug)]
pub struct UTF16Selection {
    /// The range of text in the document this selection corresponds to
    /// in UTF16 characters.
    pub range: Range<usize>,
    /// Whether the head of this selection is at the start (true), or end (false)
    /// of the range
    pub reversed: bool,
}

/// What opening a window takes (the display thread's side of
/// [`crate::WindowOptions`]).
#[derive(Debug)]
pub(crate) struct WindowParams {
    pub bounds: Bounds<Pixels>,

    /// The titlebar configuration of the window
    pub titlebar: Option<TitlebarOptions>,

    /// The kind of window to create
    pub kind: WindowKind,

    /// Whether the window should be movable by the user
    pub is_movable: bool,

    /// Whether the window should be resizable by the user
    pub is_resizable: bool,

    /// Whether the window should be minimized by the user
    pub is_minimizable: bool,
    pub focus: bool,
    pub show: bool,
    pub display_id: Option<DisplayId>,

    /// Olo: the window's tags (`WindowOptions::toplevel_tag`).
    pub toplevel_tag: Option<String>,

    /// Olo: its app id (`WindowOptions::app_id`), set with the title
    /// before the surface's first commit, so the compositor knows whose
    /// window it is from the start (app_server puts a window coming back
    /// from a restart in its own place, its tile, then).
    pub app_id: Option<String>,

    /// Olo: where a layer surface sits (`WindowOptions::layer_shell`).
    pub layer_shell: Option<LayerShellOptions>,

    /// Olo: a dialog (`WindowOptions::dialog`).
    pub dialog: Option<Dialog>,

    /// Olo: it opens full screen (`WindowSpec::fullscreen`).
    pub fullscreen: bool,

    /// Olo: another program's window it belongs to (`WindowOptions::foreign_parent`).
    pub foreign_parent: Option<String>,

    /// Olo: the output a lock surface covers (`WindowOptions::session_lock_output`).
    pub session_lock_output: Option<String>,

    /// Olo: its background and the decorations it will ask for, so its
    /// renderer is made for them (not made transparent, then again).
    pub background: WindowBackgroundAppearance,
    pub decorations: WindowDecorations,

    pub window_min_size: Option<Size<Pixels>>,
    pub window_max_size: Option<Size<Pixels>>,
}

/// Represents the status of how a window should be opened.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum WindowBounds {
    /// Indicates that the window should open in a windowed state with the given bounds.
    Windowed(Bounds<Pixels>),
    /// Indicates that the window should open in a maximized state.
    /// The bounds provided here represent the restore size of the window.
    Maximized(Bounds<Pixels>),
    /// Indicates that the window should open in fullscreen mode.
    /// The bounds provided here represent the restore size of the window.
    Fullscreen(Bounds<Pixels>),
}

impl Default for WindowBounds {
    fn default() -> Self {
        WindowBounds::Windowed(Bounds::default())
    }
}

impl WindowBounds {
    /// Retrieve the inner bounds
    pub fn get_bounds(&self) -> Bounds<Pixels> {
        match self {
            WindowBounds::Windowed(bounds) => *bounds,
            WindowBounds::Maximized(bounds) => *bounds,
            WindowBounds::Fullscreen(bounds) => *bounds,
        }
    }
}

/// The options that can be configured for a window's titlebar
#[derive(Debug, Default)]
pub struct TitlebarOptions {
    /// The initial title of the window
    pub title: Option<SharedString>,

    /// Should the default system titlebar be hidden to allow for a custom-drawn titlebar? (macOS and Windows only)
    /// Refer to [`WindowOptions::window_decorations`] on Linux
    pub appears_transparent: bool,

    /// The position of the macOS traffic light buttons
    pub traffic_light_position: Option<Point<Pixels>>,
}

/// Olo: where a layer surface (`WindowKind::LayerShell`) sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerShellOptions {
    /// Which of the compositor's layers it's in.
    pub layer: LayerShellLayer,
    /// The edges it's anchored to: top, bottom, left, right.
    pub anchor: (bool, bool, bool, bool),
    /// How much of the output's edge it keeps others from (Deskbar's
    /// width), or -1 to cover what others keep.
    pub exclusive_zone: i32,
    /// From the anchored edges: top, right, bottom, left.
    pub margin: (i32, i32, i32, i32),
    /// Whether it takes the keyboard.
    pub keyboard: LayerKeyboard,
    /// What it is ("deskbar", "notifications"), for the compositor.
    pub namespace: String,
    /// The output to sit on, by name; None lets the compositor choose.
    pub output: Option<String>,
}

/// Olo: what happens to the session lock (`lock_session`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionLockEvent {
    /// The session is locked: cover every output.
    Locked,
    /// Outputs changed while locked: cover any new one.
    Outputs,
    /// The compositor ended the lock, or refused it.
    Finished,
}

/// Olo: a dialog window (xdg-dialog), over the window that has the
/// keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// A dialog (a file panel, a settings sheet).
    Plain,
    /// Modal: its parent takes nothing until it's answered (an alert).
    Modal,
}

/// Which of the compositor's layers a layer surface is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerShellLayer {
    /// Under everything (the desktop's picture).
    Background,
    /// Under windows (the desktop's icons).
    Bottom,
    /// Over windows (Deskbar, notifications).
    Top,
    /// Over everything, full-screen windows too (a screensaver, a
    /// screenshot's selection).
    Overlay,
}

/// Whether a layer surface takes the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKeyboard {
    /// Never.
    None,
    /// When clicked, like a window (the desktop).
    OnDemand,
    /// All keys while it is up (a picker, a modal overlay).
    Exclusive,
}

/// The kind of window to create
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowKind {
    /// A normal application window
    Normal,

    /// A window that appears above all other windows, usually used for alerts or popups
    /// use sparingly!
    PopUp,

    /// A floating window that appears on top of its parent window
    Floating,

    /// Olo: a surface of the desktop's own (wlr-layer-shell): a panel, a
    /// desktop, notifications, an overlay, placed as
    /// `WindowOptions::layer_shell` says; its size is the window's, or
    /// the output's along an axis anchored at both ends.
    LayerShell,

    /// Olo: what covers an output while the session is locked
    /// (ext-session-lock: the lock screen), after `lock_session`; the
    /// output's size.
    SessionLock,

    /// Olo: a popup (a menu, a tip) over `parent`, its top left at
    /// `corner` of `anchor` (a rectangle in the parent's content, as a
    /// menu title's or the pointer's), kept on screen by the compositor
    /// (flipped to the other side, or slid), and taking the pointer and
    /// keyboard while it's up. Its size is the window's.
    Popup {
        /// The window it belongs to.
        parent: WindowId,
        /// What it opens from.
        anchor: Bounds<Pixels>,
        /// Which corner of `anchor` it opens at.
        corner: crate::Corner,
        /// Moved by this from there (a menu field's menu, its current
        /// item over the field). With an offset, it's slid to stay on
        /// screen rather than flipped, since it belongs where it is.
        offset: Point<Pixels>,
    },

    /// Olo: a tooltip over a window: a popup (Wayland's xdg_popup) that
    /// takes no input and grabs nothing, below `anchor` (in the parent's
    /// content), else above it, slid to stay on screen: it may reach past
    /// its window's edges (Deskbar's tray, narrower than what it says).
    Tip {
        /// The window it belongs to.
        parent: WindowId,
        /// What it tells about.
        anchor: Bounds<Pixels>,
    },
}

/// The appearance of the window, as defined by the operating system.
///
/// On macOS, this corresponds to named [`NSAppearance`](https://developer.apple.com/documentation/appkit/nsappearance)
/// values.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowAppearance {
    /// A light appearance.
    ///
    /// On macOS, this corresponds to the `aqua` appearance.
    Light,

    /// A light appearance with vibrant colors.
    ///
    /// On macOS, this corresponds to the `NSAppearanceNameVibrantLight` appearance.
    VibrantLight,

    /// A dark appearance.
    ///
    /// On macOS, this corresponds to the `darkAqua` appearance.
    Dark,

    /// A dark appearance with vibrant colors.
    ///
    /// On macOS, this corresponds to the `NSAppearanceNameVibrantDark` appearance.
    VibrantDark,
}

impl Default for WindowAppearance {
    fn default() -> Self {
        Self::Light
    }
}

/// The appearance of the background of the window itself, when there is
/// no content or the content is transparent.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum WindowBackgroundAppearance {
    /// Opaque.
    ///
    /// This lets the window manager know that content behind this
    /// window does not need to be drawn.
    ///
    /// Actual color depends on the system and themes should define a fully
    /// opaque background color instead.
    #[default]
    Opaque,
    /// Plain alpha transparency.
    Transparent,
    /// Transparency, but the contents behind the window are blurred.
    ///
    /// Not always supported.
    Blurred,
}

/// The options that can be configured for a file dialog prompt
#[derive(Clone, Debug)]
pub struct PathPromptOptions {
    /// Should the prompt allow files to be selected?
    pub files: bool,
    /// Should the prompt allow directories to be selected?
    pub directories: bool,
    /// Should the prompt allow multiple files to be selected?
    pub multiple: bool,
    /// The prompt to show to a user when selecting a path
    pub prompt: Option<SharedString>,
}

/// What kind of prompt styling to show
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PromptLevel {
    /// A prompt that is shown when the user should be notified of something
    Info,

    /// A prompt that is shown when the user needs to be warned of a potential problem
    Warning,

    /// A prompt that is shown when a critical problem has occurred
    Critical,
}

/// Prompt Button
#[derive(Clone, Debug, PartialEq)]
pub enum PromptButton {
    /// Ok button
    Ok(SharedString),
    /// Cancel button
    Cancel(SharedString),
    /// Other button
    Other(SharedString),
}

impl PromptButton {
    /// Create a button with label
    pub fn new(label: impl Into<SharedString>) -> Self {
        PromptButton::Other(label.into())
    }

    /// Create an Ok button
    pub fn ok(label: impl Into<SharedString>) -> Self {
        PromptButton::Ok(label.into())
    }

    /// Create a Cancel button
    pub fn cancel(label: impl Into<SharedString>) -> Self {
        PromptButton::Cancel(label.into())
    }

    #[allow(dead_code)]
    pub(crate) fn is_cancel(&self) -> bool {
        matches!(self, PromptButton::Cancel(_))
    }

    /// Returns the label of the button
    pub fn label(&self) -> &SharedString {
        match self {
            PromptButton::Ok(label) => label,
            PromptButton::Cancel(label) => label,
            PromptButton::Other(label) => label,
        }
    }
}

impl From<&str> for PromptButton {
    fn from(value: &str) -> Self {
        match value.to_lowercase().as_str() {
            "ok" => PromptButton::Ok("Ok".into()),
            "cancel" => PromptButton::Cancel("Cancel".into()),
            _ => PromptButton::Other(SharedString::from(value.to_owned())),
        }
    }
}

/// The style of the cursor (pointer)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum CursorStyle {
    /// The default cursor
    Arrow,

    /// A text input cursor
    /// corresponds to the CSS cursor value `text`
    IBeam,

    /// A crosshair cursor
    /// corresponds to the CSS cursor value `crosshair`
    Crosshair,

    /// A closed hand cursor
    /// corresponds to the CSS cursor value `grabbing`
    ClosedHand,

    /// An open hand cursor
    /// corresponds to the CSS cursor value `grab`
    OpenHand,

    /// A pointing hand cursor
    /// corresponds to the CSS cursor value `pointer`
    PointingHand,

    /// A resize left cursor
    /// corresponds to the CSS cursor value `w-resize`
    ResizeLeft,

    /// A resize right cursor
    /// corresponds to the CSS cursor value `e-resize`
    ResizeRight,

    /// A resize cursor to the left and right
    /// corresponds to the CSS cursor value `ew-resize`
    ResizeLeftRight,

    /// A resize up cursor
    /// corresponds to the CSS cursor value `n-resize`
    ResizeUp,

    /// A resize down cursor
    /// corresponds to the CSS cursor value `s-resize`
    ResizeDown,

    /// A resize cursor directing up and down
    /// corresponds to the CSS cursor value `ns-resize`
    ResizeUpDown,

    /// A resize cursor directing up-left and down-right
    /// corresponds to the CSS cursor value `nesw-resize`
    ResizeUpLeftDownRight,

    /// A resize cursor directing up-right and down-left
    /// corresponds to the CSS cursor value `nwse-resize`
    ResizeUpRightDownLeft,

    /// A cursor indicating that the item/column can be resized horizontally.
    /// corresponds to the CSS cursor value `col-resize`
    ResizeColumn,

    /// A cursor indicating that the item/row can be resized vertically.
    /// corresponds to the CSS cursor value `row-resize`
    ResizeRow,

    /// A text input cursor for vertical layout
    /// corresponds to the CSS cursor value `vertical-text`
    IBeamCursorForVerticalLayout,

    /// A cursor indicating that the operation is not allowed
    /// corresponds to the CSS cursor value `not-allowed`
    OperationNotAllowed,

    /// A cursor indicating that the operation will result in a link
    /// corresponds to the CSS cursor value `alias`
    DragLink,

    /// A cursor indicating that the operation will result in a copy
    /// corresponds to the CSS cursor value `copy`
    DragCopy,

    /// A cursor indicating that the operation will result in a context menu
    /// corresponds to the CSS cursor value `context-menu`
    ContextualMenu,

    /// Hide the cursor
    None,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self::Arrow
    }
}

/// A clipboard item that should be copied to the clipboard
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardItem {
    entries: Vec<ClipboardEntry>,
}

/// Either a ClipboardString or a ClipboardImage
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClipboardEntry {
    /// A string entry
    String(ClipboardString),
    /// An image entry
    Image(Image),
    /// Olo: data in a form of its own (files as `text/uri-list`, a
    /// format other programs know by its MIME type).
    Data(ClipboardData),
}

/// Olo: what's on the clipboard, asked for as a type
/// ([`crate::App::paste_as`]).
pub enum ClipboardReceipt {
    /// Ours: here already.
    Ready(Vec<u8>),
    /// Another program's: read it (it blocks until that program has
    /// written it all, or gives up after a few seconds).
    Read(Box<dyn FnOnce() -> Option<Vec<u8>> + Send>),
}

/// Olo: clipboard data of any MIME type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardData {
    /// What it is (`text/uri-list`, `x-special/gnome-copied-files`).
    pub mime: String,
    /// The data, as pasted.
    pub bytes: std::sync::Arc<[u8]>,
}

impl ClipboardItem {
    /// Create a new ClipboardItem::String with no associated metadata
    pub fn new_string(text: String) -> Self {
        Self { entries: vec![ClipboardEntry::String(ClipboardString::new(text))] }
    }

    /// Create a new ClipboardItem::String with the given text and associated metadata
    pub fn new_string_with_metadata(text: String, metadata: String) -> Self {
        Self { entries: vec![ClipboardEntry::String(ClipboardString { text, metadata: Some(metadata) })] }
    }

    /// Create a new ClipboardItem::String with the given text and associated metadata
    pub fn new_string_with_json_metadata<T: Serialize>(text: String, metadata: T) -> Self {
        Self { entries: vec![ClipboardEntry::String(ClipboardString::new(text).with_json_metadata(metadata))] }
    }

    /// Create a new ClipboardItem::Image with the given image with no associated metadata
    pub fn new_image(image: &Image) -> Self {
        Self { entries: vec![ClipboardEntry::Image(image.clone())] }
    }

    /// Olo: the same thing in several forms, each offered under its MIME
    /// types (text under the text ones, an image under its format's,
    /// data under its own); whoever pastes takes the one it wants.
    pub fn from_entries(entries: Vec<ClipboardEntry>) -> Self {
        Self { entries }
    }

    /// Olo: `bytes` offered as `mime` (one form; see [`Self::from_entries`]).
    pub fn new_data(mime: impl Into<String>, bytes: impl Into<std::sync::Arc<[u8]>>) -> Self {
        Self { entries: vec![ClipboardEntry::Data(ClipboardData { mime: mime.into(), bytes: bytes.into() })] }
    }

    /// Olo: also as `mime` (a hint beside text: a password marked for
    /// clipboard managers to leave alone).
    pub fn and_data(mut self, mime: impl Into<String>, bytes: impl Into<std::sync::Arc<[u8]>>) -> Self {
        self.entries.push(ClipboardEntry::Data(ClipboardData { mime: mime.into(), bytes: bytes.into() }));
        self
    }

    /// Olo: the MIME types the item is offered as, each once, `text`
    /// standing for the platform's text types.
    #[allow(dead_code)]
    pub(crate) fn mime_types(&self, text: &[&str]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut add = |m: &str| {
            if !out.iter().any(|t| t == m) {
                out.push(m.to_owned());
            }
        };
        for entry in &self.entries {
            match entry {
                ClipboardEntry::String(_) => text.iter().for_each(|t| add(t)),
                ClipboardEntry::Image(image) => add(image.format.mime_type()),
                ClipboardEntry::Data(data) => add(&data.mime),
            }
        }
        out
    }

    /// Olo: what is pasted as `mime`: data or an image offered as it,
    /// else the text.
    #[allow(dead_code)]
    pub(crate) fn bytes_for(&self, mime: &str) -> Option<Vec<u8>> {
        for entry in &self.entries {
            match entry {
                ClipboardEntry::Data(data) if data.mime == mime => {
                    return Some(data.bytes.to_vec());
                }
                ClipboardEntry::Image(image) if image.format.mime_type() == mime => {
                    return Some(image.bytes.clone());
                }
                _ => {}
            }
        }
        self.text().map(String::into_bytes)
    }

    /// Concatenates together all the ClipboardString entries in the item.
    /// Returns None if there were no ClipboardString entries.
    pub fn text(&self) -> Option<String> {
        let mut answer = String::new();
        let mut any_entries = false;

        for entry in self.entries.iter() {
            if let ClipboardEntry::String(ClipboardString { text, metadata: _ }) = entry {
                answer.push_str(text);
                any_entries = true;
            }
        }

        if any_entries { Some(answer) } else { None }
    }

    /// If this item is one ClipboardEntry::String, returns its metadata.
    pub fn metadata(&self) -> Option<&String> {
        match self.entries().first() {
            Some(ClipboardEntry::String(clipboard_string)) if self.entries.len() == 1 => clipboard_string.metadata.as_ref(),
            _ => None,
        }
    }

    /// Get the item's entries
    pub fn entries(&self) -> &[ClipboardEntry] {
        &self.entries
    }

    /// Get owned versions of the item's entries
    pub fn into_entries(self) -> impl Iterator<Item = ClipboardEntry> {
        self.entries.into_iter()
    }
}

impl From<ClipboardString> for ClipboardEntry {
    fn from(value: ClipboardString) -> Self {
        Self::String(value)
    }
}

impl From<String> for ClipboardEntry {
    fn from(value: String) -> Self {
        Self::from(ClipboardString::from(value))
    }
}

impl From<Image> for ClipboardEntry {
    fn from(value: Image) -> Self {
        Self::Image(value)
    }
}

impl From<ClipboardEntry> for ClipboardItem {
    fn from(value: ClipboardEntry) -> Self {
        Self { entries: vec![value] }
    }
}

impl From<String> for ClipboardItem {
    fn from(value: String) -> Self {
        Self::from(ClipboardEntry::from(value))
    }
}

impl From<Image> for ClipboardItem {
    fn from(value: Image) -> Self {
        Self::from(ClipboardEntry::from(value))
    }
}

/// One of the editor's supported image formats (e.g. PNG, JPEG) - used when dealing with images in the clipboard
#[derive(Clone, Copy, Debug, Eq, PartialEq, EnumIter, Hash)]
pub enum ImageFormat {
    // Sorted from most to least likely to be pasted into an editor,
    // which matters when we iterate through them trying to see if
    // clipboard content matches them.
    /// .png
    Png,
    /// .jpeg or .jpg
    Jpeg,
    /// .webp
    Webp,
    /// .gif
    Gif,
    /// .svg
    Svg,
    /// .bmp
    Bmp,
    /// .tif or .tiff
    Tiff,
}

impl ImageFormat {
    /// Returns the mime type for the ImageFormat
    pub const fn mime_type(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Webp => "image/webp",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Svg => "image/svg+xml",
            ImageFormat::Bmp => "image/bmp",
            ImageFormat::Tiff => "image/tiff",
        }
    }

    /// Returns the ImageFormat for the given mime type
    pub fn from_mime_type(mime_type: &str) -> Option<Self> {
        match mime_type {
            "image/png" => Some(Self::Png),
            "image/jpeg" | "image/jpg" => Some(Self::Jpeg),
            "image/webp" => Some(Self::Webp),
            "image/gif" => Some(Self::Gif),
            "image/svg+xml" => Some(Self::Svg),
            "image/bmp" => Some(Self::Bmp),
            "image/tiff" | "image/tif" => Some(Self::Tiff),
            _ => None,
        }
    }
}

/// An image, with a format and certain bytes
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// The image format the bytes represent (e.g. PNG)
    pub format: ImageFormat,
    /// The raw image bytes
    pub bytes: Vec<u8>,
    /// The unique ID for the image
    id: u64,
}

impl Hash for Image {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.id);
    }
}

impl Image {
    /// An empty image containing no data
    pub fn empty() -> Self {
        Self::from_bytes(ImageFormat::Png, Vec::new())
    }

    /// Create an image from a format and bytes
    pub fn from_bytes(format: ImageFormat, bytes: Vec<u8>) -> Self {
        Self { id: hash(&bytes), format, bytes }
    }

    /// Get this image's ID
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Convert the clipboard image to an `ImageData` object.
    pub fn to_image_data(&self, svg_renderer: SvgRenderer) -> Result<Arc<RenderImage>> {
        fn frames_for_image(bytes: &[u8], format: image::ImageFormat) -> Result<SmallVec<[Frame; 1]>> {
            let mut data = image::load_from_memory_with_format(bytes, format)?.into_rgba8();

            // Convert from RGBA to BGRA.
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }

            Ok(SmallVec::from_elem(Frame::new(data), 1))
        }

        let frames = match self.format {
            ImageFormat::Gif => {
                let decoder = GifDecoder::new(Cursor::new(&self.bytes))?;
                let mut frames = SmallVec::new();

                for frame in decoder.into_frames() {
                    let mut frame = frame?;
                    // Convert from RGBA to BGRA.
                    for pixel in frame.buffer_mut().chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                    frames.push(frame);
                }

                frames
            }
            ImageFormat::Png => frames_for_image(&self.bytes, image::ImageFormat::Png)?,
            ImageFormat::Jpeg => frames_for_image(&self.bytes, image::ImageFormat::Jpeg)?,
            ImageFormat::Webp => frames_for_image(&self.bytes, image::ImageFormat::WebP)?,
            ImageFormat::Bmp => frames_for_image(&self.bytes, image::ImageFormat::Bmp)?,
            ImageFormat::Tiff => frames_for_image(&self.bytes, image::ImageFormat::Tiff)?,
            ImageFormat::Svg => {
                let pixmap = svg_renderer.render_pixmap(&self.bytes, SvgSize::ScaleFactor(1.0))?;

                let buffer = image::ImageBuffer::from_raw(pixmap.width(), pixmap.height(), pixmap.take()).unwrap();

                SmallVec::from_elem(Frame::new(buffer), 1)
            }
        };

        Ok(Arc::new(RenderImage::new(frames)))
    }

    /// Get the format of the clipboard image
    pub fn format(&self) -> ImageFormat {
        self.format
    }

    /// Get the raw bytes of the clipboard image
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

/// A clipboard item that should be copied to the clipboard
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardString {
    pub(crate) text: String,
    pub(crate) metadata: Option<String>,
}

impl ClipboardString {
    /// Create a new clipboard string with the given text
    pub fn new(text: String) -> Self {
        Self { text, metadata: None }
    }

    /// Return a new clipboard item with the metadata replaced by the given metadata,
    /// after serializing it as JSON.
    pub fn with_json_metadata<T: Serialize>(mut self, metadata: T) -> Self {
        self.metadata = Some(serde_json::to_string(&metadata).unwrap());
        self
    }

    /// Get the text of the clipboard string
    pub fn text(&self) -> &String {
        &self.text
    }

    /// Get the owned text of the clipboard string
    pub fn into_text(self) -> String {
        self.text
    }

    /// Get the metadata of the clipboard string, formatted as JSON
    pub fn metadata_json<T>(&self) -> Option<T>
    where
        T: for<'a> Deserialize<'a>,
    {
        self.metadata.as_ref().and_then(|m| serde_json::from_str(m).ok())
    }
    pub(crate) fn text_hash(text: &str) -> u64 {
        let mut hasher = SeaHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

impl From<String> for ClipboardString {
    fn from(value: String) -> Self {
        Self { text: value, metadata: None }
    }
}
