use super::*;
use crate::css::{
    math,
    properties::{Keyword as K, Property as P},
    style::{Direction, Style, Value},
};
pub(super) fn grid(value: f64, scale: f64) -> f64 {
    (value * scale * 64.).trunc() / (scale * 64.)
}
struct Basis(Option<f64>);
impl math::Context for Basis {
    fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
        if unit == math::RelativeUnit::Percentage { self.0.map(|v| v / 100.).ok_or(math::Error::ContextUnavailable) } else { Err(math::Error::ContextUnavailable) }
    }
}
fn length(s: &Style, p: P, basis: Option<f64>, fallback: Option<f64>, scratch: &mut math::Scratch, cancel: Option<&AtomicBool>, scale: f64) -> Result<Option<f64>> {
    match s.value(p) {
        Value::Keyword(K::Auto | K::None) => Ok(fallback),
        Value::Numeric(n) => {
            if basis.is_none() && n.expression().dependencies().contains(math::RelativeUnit::Percentage) {
                return Ok(fallback);
            }
            Ok(Some(grid(n.resolve(&Basis(basis), scratch, Default::default(), cancel)?, scale)))
        }
        _ => Err(Error::Unsupported("intrinsic/fit-content box sizing")),
    }
}
pub(super) fn has_inline_edge(s: &Style, start: bool) -> bool {
    let props = if start { [P::MarginLeft, P::PaddingLeft] } else { [P::MarginRight, P::PaddingRight] };
    props.into_iter().any(|p| !matches!(s.value(p), Value::Keyword(K::Auto)) && s.value(p).scalar() != Some(0.))
}
pub(super) fn inline_edges(s: &Style, width: f64, scratch: &mut math::Scratch, cancel: Option<&AtomicBool>, scale: f64) -> Result<((f64, bool), (f64, bool))> {
    let mut value = |p| length(s, p, Some(width), Some(0.), scratch, cancel, scale).map(|v| v.unwrap_or(0.));
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
    pub fn new(s: &Style, width: f64, height: Option<f64>, parent_rtl: bool, scratch: &mut math::Scratch, cancel: Option<&AtomicBool>, scale: f64) -> Result<Self> {
        let mut value = |p, fallback| length(s, p, Some(width), fallback, scratch, cancel, scale);
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
        let mt = length(s, P::MarginTop, Some(width), Some(0.), scratch, cancel, scale)?.unwrap();
        let mb = length(s, P::MarginBottom, Some(width), Some(0.), scratch, cancel, scale)?.unwrap();
        let ml = length(s, P::MarginLeft, Some(width), None, scratch, cancel, scale)?;
        let mr = length(s, P::MarginRight, Some(width), None, scratch, cancel, scale)?;
        let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
        let extra = padding.horizontal() + border.horizontal();
        let content = |v: f64| nonnegative(v - if border_box { extra } else { 0. });
        let specified = length(s, P::Width, Some(width), None, scratch, cancel, scale)?.map(content);
        let min = length(s, P::MinWidth, Some(width), Some(0.), scratch, cancel, scale)?.map(content).unwrap();
        let max = length(s, P::MaxWidth, Some(width), None, scratch, cancel, scale)?.map(content).unwrap_or(f64::MAX);
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
        let extra_y = padding.vertical() + border.vertical();
        let content_y = |v: f64| nonnegative(v - if border_box { extra_y } else { 0. });
        let h = length(s, P::Height, height, None, scratch, cancel, scale)?.map(content_y);
        let min_height = length(s, P::MinHeight, height, Some(0.), scratch, cancel, scale)?.map(content_y).unwrap();
        let max_height = length(s, P::MaxHeight, height, None, scratch, cancel, scale)?.map(content_y).unwrap_or(f64::MAX);
        let mut relative = Point::default();
        if matches!(s.value(P::Position), Value::Keyword(K::Relative)) {
            let l = length(s, P::Left, Some(width), None, scratch, cancel, scale)?;
            let r = length(s, P::Right, Some(width), None, scratch, cancel, scale)?;
            relative.x = match (l, r) {
                (Some(l), r) if !parent_rtl || r.is_none() => l,
                (_, r) => -r.unwrap_or(0.),
            };
            relative.y = length(s, P::Top, height, None, scratch, cancel, scale)?.unwrap_or(-length(s, P::Bottom, height, None, scratch, cancel, scale)?.unwrap_or(0.));
        }
        let mut bfc = false;
        for p in [P::OverflowX, P::OverflowY] {
            match s.value(p) {
                Value::Keyword(K::Hidden) => bfc = true,
                Value::Keyword(K::Visible | K::Clip) => {}
                _ => return Err(Error::Unsupported("scrollbar-dependent layout")),
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
}
