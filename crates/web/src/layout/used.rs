use super::containers::QueryBasis;
use super::*;
use crate::css::{
    math,
    properties::{Keyword as K, Property as P},
    style::{Direction, Style, Value},
};
pub(super) fn grid(value: f64, scale: f64) -> f64 {
    (value * scale * 64.).trunc() / (scale * 64.)
}
struct Basis(Option<f64>, QueryBasis);
impl math::Context for Basis {
    fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
        if unit == math::RelativeUnit::Percentage { self.0.map(|v| v / 100.).ok_or(math::Error::ContextUnavailable) } else { self.1.factor(unit) }
    }
}
#[allow(clippy::too_many_arguments)] // Explicit immutable length bases and worker resources.
pub(super) fn length(s: &Style, p: P, basis: Option<f64>, fallback: Option<f64>, queries: QueryBasis, scratch: &mut math::Scratch, cancel: Option<&AtomicBool>, scale: f64) -> Result<Option<f64>> {
    match s.value(p) {
        Value::Keyword(K::Auto | K::None) => Ok(fallback),
        Value::Numeric(n) => {
            if basis.is_none() && n.expression().dependencies().contains(math::RelativeUnit::Percentage) {
                return Ok(fallback);
            }
            Ok(Some(grid(n.resolve(&Basis(basis, queries), scratch, Default::default(), cancel)?, scale)))
        }
        // min-content, max-content, fit-content and stretch keywords size from
        // the box's content; until block layout measures that, they behave as
        // auto. Flex items and absolutes resolve them from intrinsic widths.
        _ => Ok(fallback),
    }
}
pub(super) fn has_inline_edge(s: &Style, start: bool) -> bool {
    let props = if start { [P::MarginLeft, P::PaddingLeft] } else { [P::MarginRight, P::PaddingRight] };
    props.into_iter().any(|p| !matches!(s.value(p), Value::Keyword(K::Auto)) && s.value(p).scalar() != Some(0.))
}
pub(super) fn inline_edges(s: &Style, width: f64, queries: QueryBasis, scratch: &mut math::Scratch, cancel: Option<&AtomicBool>, scale: f64) -> Result<((f64, bool), (f64, bool))> {
    let mut value = |p| length(s, p, Some(width), Some(0.), queries, scratch, cancel, scale).map(|v| v.unwrap_or(0.));
    let (ml, pl) = (value(P::MarginLeft)?, value(P::PaddingLeft)?.max(0.));
    let (mr, pr) = (value(P::MarginRight)?, value(P::PaddingRight)?.max(0.));
    let (left, right) = (ml + pl, mr + pr);
    if !left.is_finite() || !right.is_finite() {
        return Err(Error::Invalid);
    }
    Ok(((left, ml != 0. || pl != 0.), (right, mr != 0. || pr != 0.)))
}
fn nonnegative(v: f64) -> f64 {
    v.max(0.)
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Used {
    pub width: f64,
    pub padding: Edges,
    pub border: Edges,
    pub margin: Edges,
    pub height: Option<f64>,
    pub min_height: f64,
    pub max_height: f64,
    pub relative: Point,
    pub bfc: bool,
    pub rtl: bool,
}
impl Used {
    #[allow(clippy::too_many_arguments)] // Explicit parent geometry/context and worker resources.
    pub fn new(s: &Style, width: f64, height: Option<f64>, parent_rtl: bool, queries: QueryBasis, scratch: &mut math::Scratch, cancel: Option<&AtomicBool>, scale: f64) -> Result<Self> {
        Self::resolve(s, width, height, parent_rtl, queries, scratch, cancel, scale, None)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn flex(s: &Style, width: f64, height: Option<f64>, queries: QueryBasis, scratch: &mut math::Scratch, cancel: Option<&AtomicBool>, scale: f64) -> Result<Self> {
        let mut used = Self::resolve(s, width, height, false, queries, scratch, cancel, scale, Some(0.))?;
        used.bfc = true;
        Ok(used)
    }
    #[allow(clippy::too_many_arguments)]
    fn resolve(
        s: &Style,
        width: f64,
        height: Option<f64>,
        parent_rtl: bool,
        queries: QueryBasis,
        scratch: &mut math::Scratch,
        cancel: Option<&AtomicBool>,
        scale: f64,
        flex: Option<f64>,
    ) -> Result<Self> {
        let mut value = |p, fallback| length(s, p, Some(width), fallback, queries, scratch, cancel, scale);
        let padding = Edges {
            top: value(P::PaddingTop, None)?.ok_or(Error::Invalid)?,
            right: value(P::PaddingRight, None)?.ok_or(Error::Invalid)?,
            bottom: value(P::PaddingBottom, None)?.ok_or(Error::Invalid)?,
            left: value(P::PaddingLeft, None)?.ok_or(Error::Invalid)?,
        };
        let border = Edges {
            top: s.used_border_width(P::BorderTopWidth)?,
            right: s.used_border_width(P::BorderRightWidth)?,
            bottom: s.used_border_width(P::BorderBottomWidth)?,
            left: s.used_border_width(P::BorderLeftWidth)?,
        };
        let mt = length(s, P::MarginTop, Some(width), Some(0.), queries, scratch, cancel, scale)?.unwrap();
        let mb = length(s, P::MarginBottom, Some(width), Some(0.), queries, scratch, cancel, scale)?.unwrap();
        let ml = length(s, P::MarginLeft, Some(width), None, queries, scratch, cancel, scale)?;
        let mr = length(s, P::MarginRight, Some(width), None, queries, scratch, cancel, scale)?;
        let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
        let extra = padding.horizontal() + border.horizontal();
        let content = |v: f64| nonnegative(v - if border_box { extra } else { 0. });
        let (used, left, right) = if let Some(used) = flex {
            // Flex sizing/alignment, not the block width equation, owns these
            // values. In particular it must not absorb free space in a margin.
            (used, ml.unwrap_or(0.), mr.unwrap_or(0.))
        } else {
            let specified = length(s, P::Width, Some(width), None, queries, scratch, cancel, scale)?.map(content);
            let min = length(s, P::MinWidth, Some(width), Some(0.), queries, scratch, cancel, scale)?.map(content).unwrap();
            let max = length(s, P::MaxWidth, Some(width), None, queries, scratch, cancel, scale)?.map(content).unwrap_or(f64::MAX);
            let solve = |w: Option<f64>| {
                let mut left = ml.unwrap_or(0.);
                let mut right = mr.unwrap_or(0.);
                let used = w.unwrap_or_else(|| nonnegative(width - extra - left - right));
                let remainder = width - extra - used - left - right;
                if remainder >= 0. && w.is_some() && (ml.is_none() || mr.is_none()) {
                    match (ml.is_none(), mr.is_none()) {
                        (true, true) => {
                            let half = ((remainder * scale * 64.).round() / 2.).trunc() / (scale * 64.);
                            left = if parent_rtl { remainder - half } else { half };
                            right = remainder - left
                        }
                        (true, false) => left = remainder,
                        (false, true) => right = remainder,
                        _ => {}
                    }
                } else if parent_rtl {
                    left += remainder
                } else {
                    right += remainder
                }
                (used, left, right)
            };
            let (mut used, mut left, mut right) = solve(specified);
            if used > max {
                (used, left, right) = solve(Some(max))
            }
            if used < min {
                (used, left, right) = solve(Some(min))
            }
            (used, left, right)
        };
        let extra_y = padding.vertical() + border.vertical();
        let content_y = |v: f64| nonnegative(v - if border_box { extra_y } else { 0. });
        let mut h = length(s, P::Height, height, None, queries, scratch, cancel, scale)?.map(content_y);
        if h.is_none() && flex.is_none() {
            h = aspect_height(s, used, padding, border);
        }
        let min_height = length(s, P::MinHeight, height, Some(0.), queries, scratch, cancel, scale)?.map(content_y).unwrap();
        let max_height = length(s, P::MaxHeight, height, None, queries, scratch, cancel, scale)?.map(content_y).unwrap_or(f64::MAX);
        let contained = match s.value(P::ContainerType) {
            Value::Keyword(K::Normal) => false,
            Value::Keyword(K::InlineSize) => true,
            Value::Keyword(K::Size) => {
                // Size containment: no content height. A flex or grid item's
                // height waits for its ratio (`apply_aspect`, after its width).
                if flex.is_none() {
                    h = Some(h.unwrap_or(0.));
                }
                true
            }
            _ => return Err(Error::Invalid),
        };
        let mut relative = Point::default();
        if matches!(s.value(P::Position), Value::Keyword(K::Relative)) {
            let l = length(s, P::Left, Some(width), None, queries, scratch, cancel, scale)?;
            let r = length(s, P::Right, Some(width), None, queries, scratch, cancel, scale)?;
            relative.x = match (l, r) {
                (Some(l), r) if !parent_rtl || r.is_none() => l,
                (_, r) => -r.unwrap_or(0.),
            };
            relative.y = length(s, P::Top, height, None, queries, scratch, cancel, scale)?.unwrap_or(-length(s, P::Bottom, height, None, queries, scratch, cancel, scale)?.unwrap_or(0.));
        }
        let mut bfc = contained;
        for p in [P::OverflowX, P::OverflowY] {
            // Scrollable overflow clips like hidden; scrollbars take no space
            // (as overlay scrollbars do) until scrollable boxes scroll.
            if !matches!(s.value(p), Value::Keyword(K::Visible | K::Clip)) {
                bfc = true;
            }
        }
        let out =
            Self { width: used, padding, border, margin: Edges { top: mt, right, bottom: mb, left }, height: h, min_height, max_height, relative, bfc, rtl: s.axes().direction == Direction::Rtl };
        if ![used, padding.horizontal(), padding.vertical(), border.horizontal(), border.vertical(), left, right, mt, mb, min_height, max_height, relative.x, relative.y]
            .into_iter()
            .all(f64::is_finite)
            || h.is_some_and(|h| !h.is_finite())
        {
            return Err(Error::Invalid);
        }
        Ok(out)
    }
    pub fn height(self, auto: f64) -> f64 {
        self.height.unwrap_or(auto).min(self.max_height).max(self.min_height)
    }
    /// A flex or grid item's height from `aspect-ratio` once its width is
    /// known (the sizes come after the main axis is solved).
    pub fn apply_aspect(&mut self, s: &Style) {
        if self.height.is_none() {
            self.height = aspect_height(s, self.width, self.padding, self.border);
        }
        // A size container without a ratio has no content height.
        if self.height.is_none() && matches!(s.value(P::ContainerType), Value::Keyword(K::Size)) {
            self.height = Some(0.);
        }
    }
}
/// The content width `aspect-ratio` gives a box of this content height
/// (CSS Sizing 4 §5.1), the ratio being of the box per its `box-sizing`.
pub(super) fn aspect_width(s: &Style, content_height: f64, padding: Edges, border: Edges) -> Option<f64> {
    let ratio = s.value(P::AspectRatio).scalar().filter(|r| r.is_finite() && *r > 0.)?;
    let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
    let (extra_x, extra_y) = (padding.horizontal() + border.horizontal(), padding.vertical() + border.vertical());
    let w = if border_box { (content_height + extra_y) * ratio - extra_x } else { content_height * ratio };
    w.is_finite().then_some(w.max(0.))
}
/// The content height `aspect-ratio` gives a box of this content width
/// (CSS Sizing 4 §5.1): the ratio is of the box per its `box-sizing`.
pub(super) fn aspect_height(s: &Style, content_width: f64, padding: Edges, border: Edges) -> Option<f64> {
    let ratio = s.value(P::AspectRatio).scalar().filter(|r| r.is_finite() && *r > 0.)?;
    let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
    let (extra_x, extra_y) = (padding.horizontal() + border.horizontal(), padding.vertical() + border.vertical());
    let h = if border_box { (content_width + extra_x) / ratio - extra_y } else { content_width / ratio };
    h.is_finite().then_some(h.max(0.))
}
