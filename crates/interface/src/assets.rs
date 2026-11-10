use crate::{DevicePixels, Pixels, Result, SharedString, Size, size};
use smallvec::SmallVec;

use image::{Delay, Frame};
use std::{
    borrow::Cow,
    fmt,
    hash::Hash,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

/// A source of assets for this app to use.
pub trait AssetSource: 'static + Send + Sync {
    /// Load the given asset from the source path.
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;

    /// List the assets at the given path.
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}

impl AssetSource for () {
    fn load(&self, _path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(None)
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

/// A unique identifier for the image cache
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(pub usize);

#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) struct RenderImageParams {
    pub(crate) image_id: ImageId,
    pub(crate) frame_index: usize,
    /// Olo: sixteen bits a channel (`RenderImage::deep`), in an atlas of
    /// its own.
    pub(crate) deep: bool,
}

/// Olo: the colours a window draws in, or an image's numbers are in.
/// sRGB unless said; a window in Display P3 (`Window::set_gamut`)
/// shows a Display P3 image's colours wider than sRGB's (on a display
/// that has them), its own sRGB colours converted as drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Gamut {
    /// sRGB, what everything is drawn in.
    #[default]
    Srgb,
    /// Display P3 (an iPhone's photos, Apple's and wide laptops' panels).
    DisplayP3,
    /// HDR10: BT.2020's primaries, PQ's curve (absolute light, sRGB's
    /// white at 203 cd/m²). A window drawing in it shows an HDR10 image
    /// (`RenderImage::deep`) as it is, everything else converted.
    Hdr10,
}

/// A cached and processed image, in BGRA format
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    data: SmallVec<[Frame; 1]>,
    /// Olo: what its numbers are in (sRGB unless said).
    pub(crate) color_space: Gamut,
    /// Olo: a deep picture instead (`deep`): its size, and its RGBA as
    /// half floats, eight bytes a pixel.
    deep: Option<(Size<DevicePixels>, Vec<u8>)>,
}

impl PartialEq for RenderImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for RenderImage {}

impl RenderImage {
    /// Create a new image from the given data.
    pub fn new(data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self { id: ImageId(NEXT_ID.fetch_add(1, SeqCst)), scale_factor: 1.0, data: data.into(), color_space: Gamut::Srgb, deep: None }
    }

    /// Olo: a picture of `width` × `height` deeper than eight bits a
    /// channel: RGBA as half floats (`rgba`, four a pixel, IEEE 754
    /// binary16 bits), its numbers in `space` (HDR10's PQ, which eight
    /// bits would band). One frame.
    pub fn deep(width: u32, height: u32, rgba: &[u16], space: Gamut) -> Self {
        let mut image = Self::new(SmallVec::<[Frame; 1]>::new());
        let bytes = rgba.iter().flat_map(|v| v.to_le_bytes()).collect();
        image.deep = Some((Size { width: DevicePixels(width as i32), height: DevicePixels(height as i32) }, bytes));
        image.color_space = space;
        image
    }

    /// Olo: a deep picture (`deep`).
    pub fn is_deep(&self) -> bool {
        self.deep.is_some()
    }

    /// Olo: its numbers are in `space` (a Display P3 photo kept so): drawn
    /// as they are in a window drawing in it, converted in one that isn't.
    pub fn with_gamut(mut self, space: Gamut) -> Self {
        self.color_space = space;
        self
    }

    /// Olo: what its numbers are in.
    pub fn gamut(&self) -> Gamut {
        self.color_space
    }

    /// Convert this image into a byte slice.
    pub fn as_bytes(&self, frame_index: usize) -> Option<&[u8]> {
        if let Some((_, bytes)) = &self.deep {
            return (frame_index == 0).then_some(bytes.as_slice());
        }
        self.data.get(frame_index).map(|frame| frame.buffer().as_raw().as_slice())
    }

    /// Get the size of this image, in pixels.
    pub fn size(&self, frame_index: usize) -> Size<DevicePixels> {
        if let Some((size, _)) = &self.deep {
            return *size;
        }
        let (width, height) = self.data[frame_index].buffer().dimensions();
        size(width.into(), height.into())
    }

    /// Get the size of this image, in pixels for display, adjusted for the scale factor.
    pub(crate) fn render_size(&self, frame_index: usize) -> Size<Pixels> {
        self.size(frame_index).map(|v| (v.0 as f32 / self.scale_factor).into())
    }

    /// Get the delay of this frame from the previous
    pub fn delay(&self, frame_index: usize) -> Delay {
        match self.data.get(frame_index) {
            Some(frame) => frame.delay(),
            None => Delay::from_numer_denom_ms(0, 1),
        }
    }

    /// Get the number of frames for this image.
    pub fn frame_count(&self) -> usize {
        if self.deep.is_some() { 1 } else { self.data.len() }
    }
}

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageData").field("id", &self.id).field("size", &self.size(0)).finish()
    }
}
