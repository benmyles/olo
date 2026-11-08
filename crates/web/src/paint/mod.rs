//! Retained native page paint commands and bounded CPU raster tiles. Worker-only.
//! Outline rasterization is not yet Chromium font rasterization or full CSS paint.
mod border;
mod control;
pub mod decoration;
mod glyph;
mod image;
mod index;
mod mipmap;
mod raster;
mod scene;
mod vector;
use crate::{css, dom, font, layout};
pub use glyph::{CacheStats, Engine};
pub use mipmap::ImageCacheStats;
pub use raster::{DeviceRect, Tile};
pub use scene::{BuildJob, Command, ControlKind, ControlLook, LayerKind, Moved, Options, Primitive, Progress, Scene, Scroller, SignatureHasher, Stratum, SurfaceShown, signature_hasher};
/// Draw a loaded SVG viewport into a surface through `transform` (its user
/// units to device pixels), for rasterising an SVG as an image.
pub(crate) fn raster_vector(
    out: &mut tiny_skia::Pixmap,
    vector: &crate::svg::Viewport,
    used: Option<&crate::svg::used::Geometry>,
    clips: &[crate::svg::clip::Resolved],
    transform: tiny_skia::Transform,
    limits: Limits,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let mut work = 0usize;
    vector::raster(out, vector, used, clips, transform, None, limits, &mut work, cancel)
}
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Debug)]
pub enum Error {
    Invalid,
    Stale,
    Limit,
    Cancelled,
    Incomplete,
    Failed,
    Unsupported(&'static str),
    Dom(dom::Error),
    Style(css::style::Error),
    Font(font::Error),
    Layout(layout::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "page paint {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
impl From<dom::Error> for Error {
    fn from(e: dom::Error) -> Self {
        Self::Dom(e)
    }
}
impl From<css::style::Error> for Error {
    fn from(e: css::style::Error) -> Self {
        Self::Style(e)
    }
}
impl From<font::Error> for Error {
    fn from(e: font::Error) -> Self {
        Self::Font(e)
    }
}
impl From<layout::Error> for Error {
    fn from(e: layout::Error) -> Self {
        Self::Layout(e)
    }
}
fn check(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub boxes: usize,
    pub commands: usize,
    pub glyph_work: usize,
    pub fonts: usize,
    pub font_bytes: usize,
    pub glyphs: usize,
    pub path_bytes: usize,
    pub path_segments: usize,
    pub tile_pixels: usize,
    /// Concurrent isolated surfaces, masks and transformed clip geometry.
    pub layer_bytes: usize,
    /// Layer pixel operations plus clip verbs across one tile.
    pub layer_pixel_work: usize,
    pub image_mip_bytes: usize,
    pub image_mip_sources: usize,
    /// Blurred box-shadow coverage kept across tiles and frames.
    pub shadow_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            boxes: 1_000_000,
            commands: 1_000_000,
            glyph_work: 4_000_000,
            fonts: 64,
            font_bytes: 32 * 1024 * 1024,
            glyphs: 8192,
            path_bytes: 16 * 1024 * 1024,
            path_segments: 262144,
            tile_pixels: 512 * 512,
            layer_bytes: 16 << 20,
            layer_pixel_work: 64 << 20,
            image_mip_bytes: 32 << 20,
            image_mip_sources: 128,
            shadow_bytes: 48 * 1024 * 1024,
        }
    }
}
fn valid(r: layout::Rect) -> bool {
    [r.x, r.y, r.width, r.height, r.x + r.width, r.y + r.height].into_iter().all(f64::is_finite) && r.width >= 0. && r.height >= 0.
}
fn intersects(a: layout::Rect, b: layout::Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}
fn union(a: layout::Rect, b: layout::Rect) -> layout::Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    layout::Rect { x, y, width: (a.x + a.width).max(b.x + b.width) - x, height: (a.y + a.height).max(b.y + b.height) - y }
}
