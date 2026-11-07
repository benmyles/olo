//! An SVG file as an image: parsed, styled (its own `<style>` and the UA
//! sheet), loaded as a viewport and rasterised at its natural size for a
//! device scale, for `<img>`, `background-image` and `mask-image`. Text,
//! `use`, masks and filters follow the viewport's own support.
use crate::{
    css::{
        Syntax,
        cascade::{Origin, SheetBuilder},
        style::{self, Host},
    },
    dom::{Document, Namespace, NodeId, NodeType},
    font::Scope,
    html,
    image::Image,
    paint, svg,
};
use std::sync::{Arc, atomic::AtomicBool};
/// The largest surface rasterised for one SVG image, per side.
const MAX_SIDE: u32 = 4096;
/// Rasterise the SVG `source` at `scale` device pixels per CSS pixel.
pub fn rasterize(source: &[u8], host: &impl Host, scope: &Scope, scale: f64, cancel: Option<&AtomicBool>) -> paint::Result<Arc<Image>> {
    let _ = scope;
    let text = String::from_utf8_lossy(source);
    let parsed = html::parse(Arc::from(text.as_ref()), html::ParseOptions { scripting: false, ..Default::default() }, cancel).map_err(|_| paint::Error::Invalid)?;
    let doc = parsed.document;
    let mut root = None;
    let mut styles = String::new();
    let mut next = Some(doc.root());
    while let Some(node) = next {
        if doc.node_type(node)? == NodeType::Element {
            match (doc.namespace(node)?, doc.name(node)?) {
                (Some(Namespace::Svg), Some("svg")) if root.is_none() => root = Some(node),
                (_, Some("style")) => {
                    for child in doc.children(node)? {
                        if let Some(value) = doc.value(child)? {
                            styles.push_str(value);
                        }
                    }
                }
                _ => {}
            }
        }
        next = crate::page::next_node(&doc, node).map_err(|_| paint::Error::Invalid)?;
    }
    let Some(root) = root else { return Err(paint::Error::Invalid) };
    let mut sheets = SheetBuilder::new(Default::default());
    sheets.add_sheet(&Syntax::parse(Arc::from(crate::page::UA), Default::default(), cancel).map_err(|_| paint::Error::Invalid)?, Origin::UserAgent, cancel).map_err(|_| paint::Error::Invalid)?;
    if !styles.is_empty() {
        sheets.add_sheet(&Syntax::parse(Arc::from(styles), Default::default(), cancel).map_err(|_| paint::Error::Invalid)?, Origin::Author, cancel).map_err(|_| paint::Error::Invalid)?;
    }
    let rules = sheets.finish(cancel).map_err(|_| paint::Error::Invalid)?;
    let defaults = style::Defaults::new(host, Default::default(), cancel)?;
    let mut job = style::Job::new(&doc, &rules, defaults, Default::default(), Default::default(), cancel)?;
    while !job.step(256, |_| host)?.finished {}
    let styles = job.finish()?;
    let vector = svg::Viewport::load(&doc, &styles, root, 200_000, 50_000_000, cancel)?;
    let (w, h) = natural(&doc, root, &vector)?;
    let scale = if scale.is_finite() && scale > 0. { scale } else { 1. };
    let scale = scale.min(f64::from(MAX_SIDE) / w.max(1.)).min(f64::from(MAX_SIDE) / h.max(1.));
    let (pw, ph) = (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1));
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).ok_or(paint::Error::Limit)?;
    if let Some([sx, sy, tx, ty]) = vector.transform(w, h) {
        let t = tiny_skia::Transform::from_row((sx * scale) as f32, 0., 0., (sy * scale) as f32, (tx * scale) as f32, (ty * scale) as f32);
        // CSS-sized geometry (rect x/y/width/height) and clip paths resolve
        // against the viewport, as the page painter does for inline SVG.
        let limits = paint::Limits::default();
        let viewport = vector.view_box.map_or([w, h], |v| [v[2], v[3]]);
        let used = if vector.rectangles != 0 { Some(svg::used::Geometry::resolve(&vector, viewport, Vec::new(), limits.path_segments, limits.path_bytes, cancel)?) } else { None };
        let clips = if vector.clip_nodes().is_empty() {
            Vec::new()
        } else {
            let mut viewports = std::collections::HashMap::new();
            viewports.insert(vector.node, viewport);
            vector.resolve_clips(&viewports, limits.path_segments, limits.path_bytes, cancel)?
        };
        paint::raster_vector(&mut pixmap, &vector, used.as_ref(), &clips, t, limits, cancel)?;
    }
    Image::from_premultiplied(pw, ph, pixmap.take(), scale, Default::default(), cancel).map_err(|_| paint::Error::Limit)
}
/// The natural size (CSS px): the root's `width` and `height` when absolute,
/// the viewBox's ratio for a missing one, else the default object size.
fn natural(doc: &Document, root: NodeId, vector: &svg::Viewport) -> paint::Result<(f64, f64)> {
    let length = |name: &str| -> paint::Result<Option<f64>> {
        let Some(value) = doc.attribute(root, name)? else { return Ok(None) };
        let value = value.trim();
        let digits: String = value.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
        let unit = value[digits.len()..].trim();
        let factor = match unit {
            "" | "px" => 1.,
            "pt" => 96. / 72.,
            "pc" => 16.,
            "in" => 96.,
            "cm" => 96. / 2.54,
            "mm" => 96. / 25.4,
            _ => return Ok(None),
        };
        Ok(digits.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.).map(|v| (v * factor).min(1e5)))
    };
    let (w, h) = (length("width")?, length("height")?);
    let ratio = vector.view_box.filter(|[_, _, w, h]| *w > 0. && *h > 0.).map(|[_, _, w, h]| w / h);
    Ok(match (w, h, ratio) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(r)) => (w, w / r),
        (None, Some(h), Some(r)) => (h * r, h),
        (Some(w), None, None) => (w, 150.),
        (None, Some(h), None) => (300., h),
        (None, None, _) => vector.natural(),
    })
}
