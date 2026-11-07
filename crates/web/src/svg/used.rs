//! Layout-dependent shape paths and ink bounds, retained once per SVG viewport.
//! Ordinary path-only SVGs never allocate this data.
use super::{Geometry as Source, Viewport};
use crate::{
    css::math,
    layout::{Error, Result},
};
use std::sync::atomic::{AtomicBool, Ordering};
pub(crate) struct Geometry {
    pub rectangles: Vec<Option<tiny_skia::Path>>,
    /// Shaped texts by slot (empty when the viewport has none, or no
    /// shaper prepared them).
    pub texts: Vec<Option<tiny_skia::Path>>,
    pub layers: Box<[Option<tiny_skia::Rect>]>,
    pub ink_bounds: Option<tiny_skia::Rect>,
}
fn join(target: &mut Option<tiny_skia::Rect>, b: Option<tiny_skia::Rect>) -> Result<()> {
    if let Some(b) = b {
        *target = Some(target.map_or(Some(b), |old| old.join(&b)).ok_or(Error::Unsupported("SVG rectangle bounds precision"))?);
    }
    Ok(())
}
impl Geometry {
    /// Resolve the viewport's rectangles (its masks' too) against the
    /// viewport's size, take the shaped texts, and bound each layer.
    pub fn resolve(source: &Viewport, viewport: [f64; 2], texts: Vec<Option<tiny_skia::Path>>, segments: usize, bytes: usize, cancel: Option<&AtomicBool>) -> Result<Self> {
        let base = (source.rectangles as usize)
            .checked_mul(size_of::<Option<tiny_skia::Path>>())
            .and_then(|n| n.checked_add(source.layers().len() * size_of::<Option<tiny_skia::Rect>>()))
            .ok_or(Error::Limit)?;
        let mut left = bytes.checked_sub(base).ok_or(Error::Limit)?;
        let mut work = segments;
        // Texts are placed here: percentage positions need the viewport.
        let texts = texts.into_iter().enumerate().map(|(i, p)| p.and_then(|p| source.texts.get(i).and_then(|t| super::text::place(t, &p, viewport)))).collect();
        let mut result = Self {
            rectangles: std::iter::repeat_with(|| None).take(source.rectangles as usize).collect(),
            texts,
            layers: source.layers().iter().map(|l| l.bounds).collect(),
            ink_bounds: source.ink_bounds,
        };
        let mut scratch = math::Scratch::default();
        let mut resolve_rect = |slot: u32, rect: &super::rect::Rectangle, transform: &Option<tiny_skia::Transform>, work: &mut usize, left: &mut usize| -> Result<Option<tiny_skia::Rect>> {
            let mut builder = tiny_skia::PathBuilder::new();
            rect.append(&mut builder, viewport, &mut scratch, (*work).min(*left / 32), cancel)?;
            // Charge a bounded amount of geometry work even for empty rectangles.
            *work = work.checked_sub(10).ok_or(Error::Limit)?;
            let path = match transform {
                Some(t) => builder.finish().and_then(|p| p.transform(*t)),
                None => builder.finish(),
            };
            *left = left.checked_sub(path.as_ref().map_or(0, |p| size_of_val(p.verbs()) + size_of_val(p.points()))).ok_or(Error::Limit)?;
            let b = path.as_ref().map(|p| p.bounds());
            *result.rectangles.get_mut(slot as usize).ok_or(Error::Invalid)? = path;
            Ok(b)
        };
        let mut stack: Vec<usize> = Vec::new();
        let mut next = 0usize;
        let mut shape = 0usize;
        loop {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(Error::Cancelled);
            }
            if stack.last().is_some_and(|&i| source.layers()[i].end as usize == shape && next >= source.layers()[i].end_layer as usize) {
                let i = stack.pop().ok_or(Error::Invalid)?;
                if let Some(&parent) = stack.last() {
                    let b = result.layers[i];
                    join(&mut result.layers[parent], b)?;
                }
                continue;
            }
            if source.layers().get(next).is_some_and(|l| l.first as usize == shape) {
                stack.push(next);
                next += 1;
                continue;
            }
            if shape == source.shapes.len() {
                break;
            }
            let b = match &source.shapes[shape].geometry {
                Source::Rectangle { slot, source: rect, transform } => resolve_rect(*slot, rect, transform, &mut work, &mut left)?,
                Source::Text { slot } => result.texts.get(*slot as usize).and_then(Option::as_ref).map(|p| p.bounds()),
                Source::Path(_) => None,
            };
            if b.is_some() {
                join(&mut result.ink_bounds, b)?;
                if let Some(&i) = stack.last() {
                    join(&mut result.layers[i], b)?;
                }
            }
            shape += 1;
        }
        // Masks' rectangles: resolved, never bounding anything.
        for mask in &source.masks {
            for s in &mask.shapes {
                if let Source::Rectangle { slot, source: rect, transform } = &s.geometry {
                    resolve_rect(*slot, rect, transform, &mut work, &mut left)?;
                }
            }
        }
        Ok(result)
    }
    pub fn payload_bytes(&self) -> usize {
        self.rectangles.capacity() * size_of::<Option<tiny_skia::Path>>()
            + size_of_val(self.layers.as_ref())
            + self.rectangles.iter().chain(self.texts.iter()).flatten().map(|p| size_of_val(p.verbs()) + size_of_val(p.points())).sum::<usize>()
    }
}
