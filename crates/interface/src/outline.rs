//! Olo: paths drawn with exact coverage. GPUI fills a path's triangles
//! multisampled (four samples: an edge pixel is one of five shades),
//! where egui and Skia work out how much of each pixel the shape covers.
//! A path built with [`crate::PathBuilder`] keeps its outline; a small
//! one in a plain colour (a check mark, a menu's triangle, an arrow) is
//! filled or stroked by vello_cpu into a coverage mask, a tile in the
//! window's atlas, and drawn as a sprite, its coverage used as it is
//! (not gamma-corrected as text's is). Masks are cached by the outline
//! as placed on the device pixel grid, so a control drawn again costs a
//! lookup, and those not drawn for a while go (`Outlines::end_frame`).
//! Larger paths (a graph) are filled on the GPU as before.

use std::{borrow::Cow, sync::Arc};

use collections::FxHashMap;
use lyon::{
    path::{LineCap, LineJoin, PathEvent},
    tessellation::{FillRule, StrokeOptions},
};
use vello_cpu::{kurbo, peniko};

use crate::{AtlasKey, AtlasTile, Bounds, DevicePixels, Pixels, PlatformAtlas, ScaledPixels, point, size};

/// What a path was built from.
#[derive(Clone, Debug)]
pub(crate) struct PathOutline {
    pub path: lyon::path::Path,
    /// Stroked with these, else filled.
    pub stroke: Option<StrokeOptions>,
    pub fill_rule: FillRule,
    /// A filled polygon whose triangles weren't made as it was built:
    /// what they're made with, if it comes to be drawn by them
    /// (`PathBuilder::build`, `Painter::paint_path`).
    pub unmade: Option<lyon::tessellation::FillOptions>,
}

impl PartialEq for PathOutline {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// Larger than this (in device pixels), a path is filled on the GPU:
/// a mask for it would cost more to make than it saves.
const MAX_AREA: u32 = 256 * 256;
/// The masks a window keeps, in bytes; past it, those drawn longest ago
/// go.
const BUDGET: usize = 4 << 20;
/// Kept at least this many frames after it was last drawn.
const KEEP_FRAMES: u64 = 120;
/// Outline coordinates are kept to this fraction of a device pixel.
const STEPS: f32 = 64.0;

/// An outline placed on the device grid: its points relative to the
/// mask's top left, in 1/64 pixels, how it's drawn, and the mask's size.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OutlineKey(Arc<[i32]>);

/// The masks a window drew recently.
#[derive(Default)]
pub(crate) struct Outlines {
    used: FxHashMap<OutlineKey, (u64, usize)>,
    bytes: usize,
    frame: u64,
}

/// Where a path's mask goes and what it is.
pub(crate) struct Placed {
    pub key: OutlineKey,
    pub bounds: Bounds<ScaledPixels>,
}

/// Whether paths are drawn this way (`OLO_EXACT_PATHS=0`: all on the
/// GPU, multisampled, to compare).
fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("OLO_EXACT_PATHS").map_or(true, |v| v != "0"))
}

/// Places `outline` (whose tessellation spans `bounds`, logical) at
/// `scale` on the device grid, or None if it's too large for a mask.
pub(crate) fn place(outline: &PathOutline, bounds: Bounds<Pixels>, scale: f32) -> Option<Placed> {
    if !enabled() {
        return None;
    }
    let b = bounds.scale(scale);
    // A pixel more on each side for the edges' coverage.
    let x0 = (b.origin.x.0 - 1.0).floor();
    let y0 = (b.origin.y.0 - 1.0).floor();
    let x1 = (b.origin.x.0 + b.size.width.0 + 1.0).ceil();
    let y1 = (b.origin.y.0 + b.size.height.0 + 1.0).ceil();
    let (w, h) = ((x1 - x0).max(0.0) as u32, (y1 - y0).max(0.0) as u32);
    if w == 0 || h == 0 || w * h > MAX_AREA || !(x0.is_finite() && y0.is_finite()) {
        return None;
    }
    let q = |v: f32| (v * STEPS).round() as i32;
    let mut data = Vec::with_capacity(8 + outline.path.iter().size_hint().0 * 4);
    data.extend([w as i32, h as i32]);
    match &outline.stroke {
        Some(s) => data.extend([1, q(s.line_width * scale), s.line_join as i32, s.start_cap as i32, s.end_cap as i32, q(s.miter_limit)]),
        None => data.extend([0, matches!(outline.fill_rule, FillRule::EvenOdd) as i32]),
    }
    let p = |v: lyon::math::Point| [q(v.x * scale - x0), q(v.y * scale - y0)];
    for event in outline.path.iter() {
        match event {
            PathEvent::Begin { at } => {
                data.push(-1);
                data.extend(p(at));
            }
            PathEvent::Line { to, .. } => {
                data.push(-2);
                data.extend(p(to));
            }
            PathEvent::Quadratic { ctrl, to, .. } => {
                data.push(-3);
                data.extend(p(ctrl));
                data.extend(p(to));
            }
            PathEvent::Cubic { ctrl1, ctrl2, to, .. } => {
                data.push(-4);
                data.extend(p(ctrl1));
                data.extend(p(ctrl2));
                data.extend(p(to));
            }
            PathEvent::End { close, .. } => data.push(if close { -5 } else { -6 }),
        }
    }
    Some(Placed { key: OutlineKey(data.into()), bounds: Bounds { origin: point(ScaledPixels(x0), ScaledPixels(y0)), size: size(ScaledPixels(w as f32), ScaledPixels(h as f32)) } })
}

/// The mask for `key`: each pixel's coverage.
fn rasterize(key: &OutlineKey) -> Option<(u16, u16, Vec<u8>)> {
    let d = &key.0;
    let (w, h) = (u16::try_from(d[0]).ok()?, u16::try_from(d[1]).ok()?);
    let f = |v: i32| f64::from(v) / f64::from(STEPS);
    let (stroke, mut i) = if d[2] == 1 {
        let join = match d[4] {
            j if j == LineJoin::Round as i32 => kurbo::Join::Round,
            j if j == LineJoin::Bevel as i32 => kurbo::Join::Bevel,
            _ => kurbo::Join::Miter,
        };
        let cap = |c: i32| match c {
            c if c == LineCap::Round as i32 => kurbo::Cap::Round,
            c if c == LineCap::Square as i32 => kurbo::Cap::Square,
            _ => kurbo::Cap::Butt,
        };
        let s = kurbo::Stroke::new(f(d[3])).with_join(join).with_start_cap(cap(d[5])).with_end_cap(cap(d[6])).with_miter_limit(f(d[7]));
        (Some(s), 8)
    } else {
        (None, 4)
    };
    let mut path = kurbo::BezPath::new();
    let pt = |i: usize| kurbo::Point::new(f(d[i]), f(d[i + 1]));
    while i < d.len() {
        match d[i] {
            -1 => {
                path.move_to(pt(i + 1));
                i += 3;
            }
            -2 => {
                path.line_to(pt(i + 1));
                i += 3;
            }
            -3 => {
                path.quad_to(pt(i + 1), pt(i + 3));
                i += 5;
            }
            -4 => {
                path.curve_to(pt(i + 1), pt(i + 3), pt(i + 5));
                i += 7;
            }
            -5 => {
                path.close_path();
                i += 1;
            }
            _ => i += 1,
        }
    }
    let mut ctx = vello_cpu::RenderContext::new(w, h);
    ctx.set_paint(vello_cpu::color::OpaqueColor::<vello_cpu::color::Srgb>::WHITE);
    match stroke {
        Some(s) => {
            ctx.set_stroke(s);
            ctx.stroke_path(&path);
        }
        None => {
            ctx.set_fill_rule(if d[3] == 1 { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
            ctx.fill_path(&path);
        }
    }
    let mut pixmap = vello_cpu::Pixmap::new(w, h);
    let mut resources = vello_cpu::Resources::new();
    ctx.render(&mut pixmap, &mut resources);
    let coverage = pixmap.data_as_u8_slice().chunks_exact(4).map(|p| p[3]).collect();
    Some((w, h, coverage))
}

impl Outlines {
    /// The atlas tile for `key`, made if it isn't there.
    pub fn tile(&mut self, atlas: &dyn PlatformAtlas, key: &OutlineKey) -> anyhow::Result<Option<AtlasTile>> {
        let tile = atlas
            .get_or_insert_with(&AtlasKey::Outline(key.clone()), &mut || Ok(rasterize(key).map(|(w, h, bytes)| (size(DevicePixels(i32::from(w)), DevicePixels(i32::from(h))), Cow::Owned(bytes)))))?;
        let frame = self.frame;
        let bytes = (key.0[0] * key.0[1]) as usize;
        let entry = self.used.entry(key.clone()).or_insert_with(|| {
            self.bytes += bytes;
            (frame, bytes)
        });
        entry.0 = frame;
        Ok(tile)
    }

    /// A frame was drawn: masks not drawn for a while go, and those
    /// drawn longest ago while there are more than the budget.
    pub fn end_frame(&mut self, atlas: &dyn PlatformAtlas) {
        let frame = self.frame;
        self.frame += 1;
        if self.used.is_empty() {
            return;
        }
        let mut gone: Vec<OutlineKey> = self.used.iter().filter(|(_, (at, _))| at + KEEP_FRAMES < frame).map(|(k, _)| k.clone()).collect();
        let mut bytes = self.bytes - gone.iter().map(|k| self.used[k].1).sum::<usize>();
        if bytes > BUDGET {
            let mut rest: Vec<_> = self.used.iter().filter(|(_, (at, _))| at + KEEP_FRAMES >= frame && *at < frame).map(|(k, (at, b))| (*at, *b, k.clone())).collect();
            rest.sort_by_key(|(at, _, _)| *at);
            for (_, b, k) in rest {
                if bytes <= BUDGET {
                    break;
                }
                bytes -= b;
                gone.push(k);
            }
        }
        for k in gone {
            if let Some((_, b)) = self.used.remove(&k) {
                self.bytes -= b;
                atlas.remove(&AtlasKey::Outline(k));
            }
        }
    }
}
