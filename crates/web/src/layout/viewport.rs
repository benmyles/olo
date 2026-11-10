//! Classic viewport gutters. Full viewport, layout and client sizes are distinct.
use super::*;
use crate::{
    css::{
        properties::Keyword as K,
        style::{Property as P, ScrollbarWidths, Style, Value},
    },
    font::Scope,
    text::shaping,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportGeometry {
    pub full: Viewport,
    /// Initial containing block, excluding reserved gutters (CSS px).
    pub content: Rect,
    /// The viewport minus actual visible scrollbar space. Empty stable
    /// gutters do not reduce documentElement.clientWidth/VisualViewport.
    pub client: [f64; 2],
    /// Horizontal, vertical: an actual scrollbar, including forced ones.
    pub bars: [bool; 2],
    pub thickness: f64,
    /// CSS viewport-unit basis; automatic scrollbars do not reduce it.
    pub units: [f64; 2],
}
impl ViewportGeometry {
    pub fn overlay(full: Viewport) -> Self {
        Self { full, content: Rect { x: 0., y: 0., width: full.width, height: full.height }, client: [full.width, full.height], bars: [false; 2], thickness: 0., units: [full.width, full.height] }
    }
    fn new(full: Viewport, widths: ScrollbarWidths, policy: ViewportOverflow, bars: [bool; 2], units: [f64; 2]) -> Self {
        let thickness = (width(policy, widths) * full.scale).round() / full.scale;
        let bars = bars.map(|b| b && thickness > 0.);
        let stable = matches!(policy.scrollbar_gutter, K::Stable | K::StableBothEdges) && matches!(policy.axes[1], K::Auto | K::Scroll | K::Hidden);
        let right = if bars[1] || stable { thickness } else { 0. };
        let left = if right > 0. && policy.scrollbar_gutter == K::StableBothEdges { right } else { 0. };
        let bottom = if bars[0] { thickness } else { 0. };
        Self {
            full,
            content: Rect { x: left, y: 0., width: (full.width - left - right).max(0.), height: (full.height - bottom).max(0.) },
            client: [(full.width - if bars[1] { thickness } else { 0. }).max(0.), (full.height - bottom).max(0.)],
            bars,
            thickness,
            units,
        }
    }
}
fn width(policy: ViewportOverflow, widths: ScrollbarWidths) -> f64 {
    match policy.scrollbar_width {
        K::None => 0.,
        K::Thin => widths.thin,
        _ => widths.auto,
    }
}
pub(super) fn unit_size(full: Viewport, widths: ScrollbarWidths, policy: ViewportOverflow, root: Option<&Style>) -> [f64; 2] {
    let Some(root) = root.filter(|s| !matches!(s.value(P::Display), Value::Display(css::properties::Display::None))) else { return [full.width, full.height] };
    let thickness = width(policy, widths);
    let forced = |p| matches!(root.value(p), Value::Keyword(K::Scroll));
    let stable = matches!(policy.scrollbar_gutter, K::Stable | K::StableBothEdges) && matches!(policy.axes[1], K::Auto | K::Scroll | K::Hidden);
    let inline = if stable || forced(P::OverflowY) { thickness * if policy.scrollbar_gutter == K::StableBothEdges { 2. } else { 1. } } else { 0. };
    [(full.width - inline).max(0.), (full.height - if forced(P::OverflowX) { thickness } else { 0. }).max(0.)]
}
pub struct ViewportLayout {
    pub layout: Layout,
    pub fallbacks: Fallbacks,
    pub progress: Progress,
}
/// Lay out a viewport, then add automatic bars required by its overflow.
/// Adding one axis can require the other; at most three layout passes.
/// Styles, shaped text and intrinsic widths are shared across those passes.
/// Start without automatic bars, so a prior narrow layout cannot trap a page
/// in an unnecessary scrollbar after its content shrinks.
#[allow(clippy::too_many_arguments)]
pub fn layout_viewport(
    engine: &mut shaping::Engine,
    scope: &Scope,
    mut cache: Arc<Cache>,
    viewport: Viewport,
    widths: ScrollbarWidths,
    previous: Option<(&Layout, &dom::NodeSet)>,
    limits: Limits,
    cancel: Option<&AtomicBool>,
    mut report: impl FnMut(Progress),
) -> Result<ViewportLayout> {
    if [widths.auto, widths.thin].into_iter().any(|v| !v.is_finite() || v < 0. || !(v * viewport.scale).is_finite()) {
        return Err(Error::Invalid);
    }
    let policy = cache.tree().viewport_overflow();
    let units = cache.tree().viewport_unit_size(viewport, widths);
    let mut bars = policy.axes.map(|k| k == K::Scroll);
    let mut state = ViewportGeometry::new(viewport, widths, policy, bars, units);
    let mut work = 0;
    loop {
        let remaining = Limits { work: limits.work.checked_sub(work).ok_or(Error::Limit)?, ..limits };
        let mut job = Job::new(engine, scope, cache, viewport, remaining, cancel)?.with_viewport_geometry(state)?;
        if let Some((p, dirty)) = previous {
            job = job.with_previous(p, dirty);
        }
        while !job.step(64)?.finished {
            let mut progress = job.progress();
            progress.work += work;
            report(progress);
        }
        let mut progress = job.progress();
        work += progress.work;
        progress.work = work;
        let fallbacks = job.fallbacks().clone();
        let layout = job.finish()?;
        if state.thickness > 0. {
            work = work.checked_add(layout.cache().tree().boxes().len()).filter(|w| *w <= limits.work).ok_or(Error::Limit)?;
            progress.work = work;
            let size = layout.document_size_checked(cancel)?;
            // Overflow reaches the visible client area, including empty
            // stable gutters. Compare snapped device edges, as painting and
            // the native reference do at fractional display scales.
            let extent = [size.0, size.1];
            let needed = std::array::from_fn::<_, 2, _>(|i| (extent[i] * viewport.scale).round() > (state.client[i] * viewport.scale).round());
            let next = std::array::from_fn(|i| bars[i] || policy.axes[i] == K::Auto && needed[i]);
            if next != bars {
                bars = next;
                state = ViewportGeometry::new(viewport, widths, policy, bars, units);
                cache = layout.cache().clone();
                continue;
            }
        }
        report(progress);
        return Ok(ViewportLayout { layout, fallbacks, progress });
    }
}
