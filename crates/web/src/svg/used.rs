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
    pub fn resolve(source: &Viewport, viewport: [f64; 2], segments: usize, bytes: usize, cancel: Option<&AtomicBool>) -> Result<Self> {
        let base = (source.rectangles as usize)
            .checked_mul(size_of::<Option<tiny_skia::Path>>())
            .and_then(|n| n.checked_add(source.layers().len() * size_of::<Option<tiny_skia::Rect>>()))
            .ok_or(Error::Limit)?;
        let mut left = bytes.checked_sub(base).ok_or(Error::Limit)?;
        let mut work = segments;
        let mut result = Self { rectangles: Vec::with_capacity(source.rectangles as usize), layers: source.layers().iter().map(|l| l.bounds).collect(), ink_bounds: source.ink_bounds };
        let mut stack: Vec<usize> = Vec::new();
        let mut scratch = math::Scratch::default();
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
            if let Source::Rectangle { slot, source, transform } = &source.shapes[shape].geometry {
                if *slot as usize != result.rectangles.len() {
                    return Err(Error::Invalid);
                }
                let mut builder = tiny_skia::PathBuilder::new();
                source.append(&mut builder, viewport, &mut scratch, work.min(left / 32), cancel)?;
                // Charge a bounded amount of geometry work even for empty rectangles.
                work = work.checked_sub(10).ok_or(Error::Limit)?;
                let path = match transform {
                    Some(t) => builder.finish().and_then(|p| p.transform(*t)),
                    None => builder.finish(),
                };
                left = left.checked_sub(path.as_ref().map_or(0, |p| size_of_val(p.verbs()) + size_of_val(p.points()))).ok_or(Error::Limit)?;
                let b = path.as_ref().map(|p| p.bounds());
                join(&mut result.ink_bounds, b)?;
                if let Some(&i) = stack.last() {
                    join(&mut result.layers[i], b)?;
                }
                result.rectangles.push(path);
            }
            shape += 1;
        }
        Ok(result)
    }
    pub fn payload_bytes(&self) -> usize {
        self.rectangles.capacity() * size_of::<Option<tiny_skia::Path>>()
            + size_of_val(self.layers.as_ref())
            + self.rectangles.iter().flatten().map(|p| size_of_val(p.verbs()) + size_of_val(p.points())).sum::<usize>()
    }
}
