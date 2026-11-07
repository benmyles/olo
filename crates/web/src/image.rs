//! Immutable decoded image assets, supplied by a resource worker. No I/O or
//! codec dependency lives here. Pixels are shared across boxes/restyles.
use crate::dom::{Document, Namespace, NodeId};
use std::collections::HashMap;
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub pixels: usize,
    pub bytes: usize,
    pub bindings: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { pixels: 16 << 20, bytes: 64 << 20, bindings: 128 }
    }
}
#[derive(Debug)]
pub enum Error {
    Invalid,
    Limit,
    Cancelled,
    Stale,
    Dom(crate::dom::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "image asset {self:?}")
    }
}
impl std::error::Error for Error {}
fn check(cancel: Option<&AtomicBool>) -> Result<(), Error> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
pub struct Image {
    width: u32,
    height: u32,
    rgba: Box<[u8]>,
    /// Device pixels per CSS pixel of its natural size (an SVG rasterised
    /// for a scale is sharper than its CSS size).
    density: f64,
    /// Every pixel is opaque: drawn at 1:1, rows are copied, not blended.
    opaque: bool,
}
/// Whether every alpha byte is 255 (vectorised: 16 bytes a step).
fn all_opaque(rgba: &[u8]) -> bool {
    let (chunks, rest) = rgba.as_chunks::<16>();
    chunks.iter().all(|c| c[3] & c[7] & c[11] & c[15] == 255) && rest.chunks_exact(4).all(|p| p[3] == 255)
}
impl Image {
    /// Premultiplied RGBA8 as rasterised, `density` pixels per CSS pixel.
    pub fn from_premultiplied(width: u32, height: u32, rgba: Vec<u8>, density: f64, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Arc<Self>, Error> {
        check(cancel)?;
        let pixels = (width as usize).checked_mul(height as usize).ok_or(Error::Limit)?;
        let bytes = pixels.checked_mul(4).ok_or(Error::Limit)?;
        if pixels > limits.pixels || bytes > limits.bytes {
            return Err(Error::Limit);
        }
        if width == 0 || height == 0 || rgba.len() != bytes || !(density.is_finite() && density > 0.) {
            return Err(Error::Invalid);
        }
        let opaque = all_opaque(&rgba);
        Ok(Arc::new(Self { width, height, rgba: rgba.into_boxed_slice(), density, opaque }))
    }
    /// The natural size in CSS pixels.
    pub fn natural(&self) -> (f64, f64) {
        (f64::from(self.width) / self.density, f64::from(self.height) / self.density)
    }
    /// Consume straight sRGB RGBA8, premultiplying in place once on the worker.
    pub fn from_rgba(width: u32, height: u32, mut rgba: Vec<u8>, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Arc<Self>, Error> {
        check(cancel)?;
        let pixels = (width as usize).checked_mul(height as usize).ok_or(Error::Limit)?;
        let bytes = pixels.checked_mul(4).ok_or(Error::Limit)?;
        if pixels > limits.pixels || bytes > limits.bytes {
            return Err(Error::Limit);
        }
        if width == 0 || height == 0 || rgba.len() != bytes {
            return Err(Error::Invalid);
        }
        for chunk in rgba.chunks_mut(32 << 10) {
            check(cancel)?;
            if cfg!(feature = "scalar-image-processing") {
                for pixel in chunk.as_chunks_mut::<4>().0 {
                    let alpha = u16::from(pixel[3]);
                    if alpha != 255 {
                        for c in &mut pixel[..3] {
                            *c = ((u16::from(*c) * alpha + 127) / 255) as u8;
                        }
                    }
                }
            } else {
                let (quads, tail) = chunk.as_chunks_mut::<16>();
                for quad in quads {
                    if quad[3] & quad[7] & quad[11] & quad[15] == 255 {
                        continue;
                    }
                    // Four packed RGBA pixels. R/B products stay in separate
                    // 16-bit fields; no byte widening or alpha lane gathers.
                    let lanes: &[[u8; 4]; 4] = quad.as_chunks::<4>().0.try_into().unwrap();
                    let p = wide::u32x4::from(lanes.map(u32::from_le_bytes));
                    let a = p >> 24u32;
                    let mask = wide::u32x4::splat(0x00ff00ff);
                    let rb = (p & mask) * a + wide::u32x4::splat(0x00800080);
                    let rb = ((rb + ((rb >> 8u32) & mask)) >> 8u32) & mask;
                    let g = ((p >> 8u32) & wide::u32x4::splat(255)) * a + wide::u32x4::splat(128);
                    let g = (g + (g >> 8u32)) >> 8u32;
                    let result = rb | (g << 8u32) | (p & wide::u32x4::splat(0xff000000));
                    for (out, value) in quad.as_chunks_mut::<4>().0.iter_mut().zip(result.to_array()) {
                        *out = value.to_le_bytes();
                    }
                }
                for pixel in tail.as_chunks_mut::<4>().0 {
                    let alpha = u16::from(pixel[3]);
                    for c in &mut pixel[..3] {
                        *c = ((u16::from(*c) * alpha + 127) / 255) as u8;
                    }
                }
            }
        }
        check(cancel)?;
        let opaque = all_opaque(&rgba);
        Ok(Arc::new(Self { width, height, rgba: rgba.into_boxed_slice(), density: 1., opaque }))
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    /// Whether every pixel is opaque.
    pub fn opaque(&self) -> bool {
        self.opaque
    }
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
    pub fn payload_bytes(&self) -> usize {
        self.rgba.len()
    }
}
/// Document-scoped bindings to shared image allocations. Mutation invalidates
/// the set; a new document can never inherit stale handles from an old page.
pub struct Set {
    document: u64,
    revision: u64,
    bindings: Box<[(NodeId, Arc<Image>)]>,
    /// `background-image` images by the URL as written in the stylesheet.
    backgrounds: HashMap<Arc<str>, Arc<Image>>,
    bytes: usize,
}
impl Set {
    pub fn new(document: &Document, bindings: Vec<(NodeId, Arc<Image>)>, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Arc<Self>, Error> {
        Self::with_backgrounds(document, bindings, HashMap::new(), limits, cancel)
    }
    pub fn with_backgrounds(
        document: &Document,
        mut bindings: Vec<(NodeId, Arc<Image>)>,
        backgrounds: HashMap<Arc<str>, Arc<Image>>,
        limits: Limits,
        cancel: Option<&AtomicBool>,
    ) -> Result<Arc<Self>, Error> {
        check(cancel)?;
        if bindings.len() > limits.bindings || bindings.len() > u32::MAX as usize {
            return Err(Error::Limit);
        }
        let mut unique = HashSet::new();
        let mut bytes = 0usize;
        let mut pixels = 0usize;
        for image in backgrounds.values() {
            check(cancel)?;
            if unique.insert(Arc::as_ptr(image)) {
                bytes = bytes.checked_add(image.payload_bytes()).ok_or(Error::Limit)?;
                pixels = pixels.checked_add(image.payload_bytes() / 4).ok_or(Error::Limit)?;
                if bytes > limits.bytes || pixels > limits.pixels {
                    return Err(Error::Limit);
                }
            }
        }
        for (node, image) in &bindings {
            check(cancel)?;
            if document.namespace(*node).map_err(Error::Dom)? != Some(Namespace::Html) || !matches!(document.name(*node).map_err(Error::Dom)?, Some("img" | "canvas")) {
                return Err(Error::Invalid);
            }
            if unique.insert(Arc::as_ptr(image)) {
                bytes = bytes.checked_add(image.payload_bytes()).ok_or(Error::Limit)?;
                pixels = pixels.checked_add(image.payload_bytes() / 4).ok_or(Error::Limit)?;
                if bytes > limits.bytes || pixels > limits.pixels {
                    return Err(Error::Limit);
                }
            }
        }
        bindings.sort_unstable_by_key(|(node, _)| node.slot());
        if bindings.windows(2).any(|w| w[0].0.slot() == w[1].0.slot()) {
            return Err(Error::Invalid);
        }
        check(cancel)?;
        Ok(Arc::new(Self { document: document.id(), revision: document.revision(), bindings: bindings.into_boxed_slice(), backgrounds, bytes }))
    }
    /// This set bound to the document as it is now: a later revision of the
    /// same document, keeping the bindings whose images are still there.
    pub fn rebind(&self, document: &Document) -> Option<Arc<Self>> {
        if document.id() != self.document {
            return None;
        }
        if document.revision() == self.revision {
            return None;
        }
        let bindings: Vec<(NodeId, Arc<Image>)> = self
            .bindings
            .iter()
            .filter(|(node, _)| document.namespace(*node).ok().flatten() == Some(Namespace::Html) && matches!(document.name(*node).ok().flatten(), Some("img" | "canvas")))
            .cloned()
            .collect();
        Some(Arc::new(Self { document: self.document, revision: document.revision(), bindings: bindings.into_boxed_slice(), backgrounds: self.backgrounds.clone(), bytes: self.bytes }))
    }
    /// This set with more bindings (canvases the scripts drew), replacing
    /// any for the same element, for the document as it is now.
    pub fn with_extra(&self, document: &Document, extra: Vec<(NodeId, Arc<Image>)>, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Arc<Self>, Error> {
        let mut bindings: Vec<(NodeId, Arc<Image>)> = self.bindings.iter().filter(|(n, _)| !extra.iter().any(|(e, _)| e == n)).cloned().collect();
        bindings.extend(extra);
        let bindings = bindings
            .into_iter()
            .filter(|(node, _)| document.namespace(*node).ok().flatten() == Some(Namespace::Html) && matches!(document.name(*node).ok().flatten(), Some("img" | "canvas")))
            .collect();
        Self::with_backgrounds(document, bindings, self.backgrounds.clone(), limits, cancel)
    }
    /// A set from bindings alone.
    pub fn from_bindings(document: &Document, bindings: Vec<(NodeId, Arc<Image>)>, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Arc<Self>, Error> {
        Self::with_backgrounds(document, bindings, HashMap::new(), limits, cancel)
    }
    /// Which elements are bound (not what to), for telling whether a layout
    /// made with another set has the same replaced boxes.
    pub fn bound_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.bindings.iter().map(|(n, _)| *n)
    }
    pub fn validate(&self, document: &Document) -> Result<(), Error> {
        if document.id() == self.document && document.revision() == self.revision { Ok(()) } else { Err(Error::Stale) }
    }
    pub fn get(&self, node: NodeId) -> Option<&Arc<Image>> {
        if node.document() != self.document {
            return None;
        }
        let i = self.index(node)? as usize;
        Some(&self.bindings[i].1)
    }
    pub fn index(&self, node: NodeId) -> Option<u32> {
        if node.document() != self.document {
            return None;
        }
        let i = self.bindings.binary_search_by_key(&node.slot(), |(n, _)| n.slot()).ok()?;
        (self.bindings[i].0 == node).then_some(i as u32)
    }
    pub fn image(&self, index: u32) -> Option<&Arc<Image>> {
        self.bindings.get(index as usize).map(|(_, image)| image)
    }
    /// A hash of the background images' identities (URL and pixels): the
    /// same across rebinds, so painted tiles can tell they are unchanged.
    pub fn backgrounds_identity(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut keys: Vec<(&Arc<str>, usize)> = self.backgrounds.iter().map(|(k, v)| (k, Arc::as_ptr(v) as usize)).collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        let mut h = std::hash::DefaultHasher::new();
        for (k, v) in keys {
            k.hash(&mut h);
            v.hash(&mut h);
        }
        h.finish()
    }
    pub fn background(&self, url: &str) -> Option<&Arc<Image>> {
        self.backgrounds.get(url)
    }
    pub fn payload_bytes(&self) -> usize {
        self.bytes + std::mem::size_of_val(self.bindings.as_ref())
    }
}
