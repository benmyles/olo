//! SVG paint servers: `<linearGradient>` and `<radialGradient>` resolved
//! for a shape (their attributes and stops through `href`, their units and
//! transform), for the painter's gradient shaders.
use super::clip;
use crate::{
    css::style::{Property as P, StyleTree},
    dom::{Document, NodeId, NodeType},
    layout::{Error, Result},
};
use std::sync::{Arc, atomic::AtomicBool};
/// A gradient in the shape's (the viewport's) space.
#[derive(Clone, Debug)]
pub struct Gradient {
    pub kind: Kind,
    /// Offsets (0 to 1, in order) and colours (straight RGBA).
    pub stops: Vec<(f32, [u8; 4])>,
    pub spread: tiny_skia::SpreadMode,
    /// From the gradient's own space to the viewport's.
    pub transform: tiny_skia::Transform,
}
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// x1, y1, x2, y2.
    Linear([f32; 4]),
    /// cx, cy, r, fx, fy.
    Radial([f32; 5]),
}
impl Gradient {
    /// Its shader, `Paint`-ready (None when it paints nothing usable).
    pub fn shader(&self) -> Option<tiny_skia::Shader<'static>> {
        let stops: Vec<tiny_skia::GradientStop> = self.stops.iter().map(|(o, c)| tiny_skia::GradientStop::new(*o, tiny_skia::Color::from_rgba8(c[0], c[1], c[2], c[3]))).collect();
        match self.kind {
            Kind::Linear([x1, y1, x2, y2]) => tiny_skia::LinearGradient::new(tiny_skia::Point::from_xy(x1, y1), tiny_skia::Point::from_xy(x2, y2), stops, self.spread, self.transform),
            Kind::Radial([cx, cy, r, fx, fy]) => {
                tiny_skia::RadialGradient::new(tiny_skia::Point::from_xy(fx, fy), 0., tiny_skia::Point::from_xy(cx, cy), r.max(1e-4), stops, self.spread, self.transform)
            }
        }
    }
}
/// A length of a gradient's: a number, or a percentage (of the bounding
/// box with `objectBoundingBox` units: a fraction; else of `basis`).
fn length(value: Option<&str>, default: f32, bounding: bool, basis: f32) -> f32 {
    let Some(v) = value.map(str::trim) else { return default };
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse::<f32>().map_or(default, |p| if bounding { p / 100. } else { p / 100. * basis });
    }
    v.trim_end_matches("px").parse::<f32>().unwrap_or(default)
}
/// The gradient `reference` (`url(#id)`) names, for a shape whose geometry
/// (in the viewport's space) is `bounds`, drawn under `ctm`; `viewport` the
/// size percentages of user-space units are of. None when it names none.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve<'a>(
    doc: &'a Document,
    styles: &StyleTree,
    sources: &mut clip::Sources<'a>,
    reference: &str,
    ctm: tiny_skia::Transform,
    bounds: Option<tiny_skia::Rect>,
    viewport: (f32, f32),
    work_limit: usize,
    bytes_limit: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Option<Arc<Gradient>>> {
    let inner = reference.strip_prefix("url(").and_then(|r| r.split(')').next()).unwrap_or("").trim().trim_matches(['"', '\'']);
    let Some(id) = inner.strip_prefix('#') else { return Ok(None) };
    // The chain through href: the first element of each attribute that has
    // it, the first with stops.
    let mut chain: Vec<NodeId> = Vec::new();
    let mut target = sources.lookup(doc, id, work_limit, bytes_limit, cancel)?;
    while let Some(g) = target {
        if chain.len() >= 8 || chain.contains(&g) || !matches!(doc.name(g)?, Some("linearGradient" | "radialGradient")) {
            break;
        }
        chain.push(g);
        let href = doc.attribute(g, "href")?.or(doc.attribute(g, "xlink:href")?);
        target = match href.and_then(|h| h.trim().strip_prefix('#')) {
            Some(id) => sources.lookup(doc, id, work_limit, bytes_limit, cancel)?,
            None => None,
        };
    }
    let Some(&first) = chain.first() else { return Ok(None) };
    let attr = |name: &str| -> Result<Option<&'a str>> {
        for &g in &chain {
            if let Some(v) = doc.attribute(g, name)? {
                return Ok(Some(v));
            }
        }
        Ok(None)
    };
    let mut stops = Vec::new();
    for &g in &chain {
        let mut child = doc.first_child(g)?;
        while let Some(stop) = child {
            if doc.node_type(stop)? == NodeType::Element && doc.name(stop)? == Some("stop") {
                let s = styles.snapshot(stop).ok_or(Error::Stale)?;
                let mut color = s.used_color(P::StopColor)?.srgb8_clipped();
                color[3] = (f32::from(color[3]) * s.value(P::StopOpacity).scalar().unwrap_or(1.).clamp(0., 1.) as f32).round() as u8;
                // Offsets clamp to [0, 1] and never go back (SVG 2 §14.3.2).
                let offset = length(doc.attribute(stop, "offset")?, 0., true, 1.).clamp(0., 1.);
                let offset = stops.last().map_or(offset, |&(last, _): &(f32, [u8; 4])| offset.max(last));
                stops.push((offset, color));
                if stops.len() > 256 {
                    return Err(Error::Limit);
                }
            }
            child = doc.next_sibling(stop)?;
        }
        if !stops.is_empty() {
            break;
        }
    }
    if stops.is_empty() {
        return Ok(None);
    }
    let bounding = attr("gradientUnits")?.is_none_or(|u| u.trim() != "userSpaceOnUse");
    let (w, h) = viewport;
    let diagonal = ((w * w + h * h) / 2.).sqrt();
    let kind = if doc.name(first)? == Some("radialGradient") {
        let cx = length(attr("cx")?, if bounding { 0.5 } else { w * 0.5 }, bounding, w);
        let cy = length(attr("cy")?, if bounding { 0.5 } else { h * 0.5 }, bounding, h);
        let r = length(attr("r")?, if bounding { 0.5 } else { diagonal * 0.5 }, bounding, diagonal);
        let fx = length(attr("fx")?, cx, bounding, w);
        let fy = length(attr("fy")?, cy, bounding, h);
        Kind::Radial([cx, cy, r, fx, fy])
    } else {
        Kind::Linear([
            length(attr("x1")?, 0., bounding, w),
            length(attr("y1")?, 0., bounding, h),
            length(attr("x2")?, if bounding { 1. } else { w }, bounding, w),
            length(attr("y2")?, 0., bounding, h),
        ])
    };
    let spread = match attr("spreadMethod")?.map(str::trim) {
        Some("reflect") => tiny_skia::SpreadMode::Reflect,
        Some("repeat") => tiny_skia::SpreadMode::Repeat,
        _ => tiny_skia::SpreadMode::Pad,
    };
    let own = match attr("gradientTransform")? {
        Some(t) => match t.parse::<svgtypes::Transform>() {
            Ok(t) => tiny_skia::Transform::from_row(t.a as f32, t.b as f32, t.c as f32, t.d as f32, t.e as f32, t.f as f32),
            Err(_) => tiny_skia::Transform::identity(),
        },
        None => tiny_skia::Transform::identity(),
    };
    // Object bounding box units: the unit square is the shape's box, in its
    // own space (its viewport-space bounds brought back through its ctm).
    let units = if bounding {
        let Some(b) = bounds.and_then(|b| ctm.invert().and_then(|inv| b.transform(inv))) else { return Ok(None) };
        if b.width() <= 0. || b.height() <= 0. {
            return Ok(None);
        }
        tiny_skia::Transform::from_row(b.width(), 0., 0., b.height(), b.left(), b.top())
    } else {
        tiny_skia::Transform::identity()
    };
    let transform = own.post_concat(units).post_concat(ctm);
    Ok(Some(Arc::new(Gradient { kind, stops, spread, transform })))
}
