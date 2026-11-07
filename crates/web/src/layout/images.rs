use super::containers::QueryBasis;
use super::used::{Used, grid, length};
use super::*;
use crate::css::{
    math,
    style::{Keyword as K, Property as P, Style, Value},
};
#[allow(clippy::too_many_arguments)]
pub(super) fn used(
    s: &Style,
    width: f64,
    height: Option<f64>,
    natural: (f64, f64),
    block: bool,
    rtl: bool,
    queries: QueryBasis,
    scratch: &mut math::Scratch,
    cancel: Option<&AtomicBool>,
    scale: f64,
) -> Result<Used> {
    check(cancel)?;
    let padding = Edges {
        top: length(s, P::PaddingTop, Some(width), Some(0.), queries, scratch, cancel, scale)?.unwrap().max(0.),
        right: length(s, P::PaddingRight, Some(width), Some(0.), queries, scratch, cancel, scale)?.unwrap().max(0.),
        bottom: length(s, P::PaddingBottom, Some(width), Some(0.), queries, scratch, cancel, scale)?.unwrap().max(0.),
        left: length(s, P::PaddingLeft, Some(width), Some(0.), queries, scratch, cancel, scale)?.unwrap().max(0.),
    };
    let border = Edges {
        top: s.used_border_width(P::BorderTopWidth)?,
        right: s.used_border_width(P::BorderRightWidth)?,
        bottom: s.used_border_width(P::BorderBottomWidth)?,
        left: s.used_border_width(P::BorderLeftWidth)?,
    };
    let (ex, ey) = (padding.horizontal() + border.horizontal(), padding.vertical() + border.vertical());
    let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
    let x = |n: f64| (n - if border_box { ex } else { 0. }).max(0.);
    let y = |n: f64| (n - if border_box { ey } else { 0. }).max(0.);
    let sw = length(s, P::Width, Some(width), None, queries, scratch, cancel, scale)?.map(x);
    let sh = length(s, P::Height, height, None, queries, scratch, cancel, scale)?.map(y);
    let minw = x(length(s, P::MinWidth, Some(width), Some(0.), queries, scratch, cancel, scale)?.unwrap());
    let maxw = length(s, P::MaxWidth, Some(width), None, queries, scratch, cancel, scale)?.map(x).unwrap_or(f64::MAX).max(minw);
    let minh = y(length(s, P::MinHeight, height, Some(0.), queries, scratch, cancel, scale)?.unwrap());
    let maxh = length(s, P::MaxHeight, height, None, queries, scratch, cancel, scale)?.map(y).unwrap_or(f64::MAX).max(minh);
    // A box without a natural ratio (an empty placeholder) keeps its natural
    // size on the automatic axis, as a replaced element without a ratio does.
    let ratio = if natural.0 > 0. && natural.1 > 0. { Some(natural.0 / natural.1) } else { None };
    let (mut w, mut h) = match (sw, sh) {
        (Some(w), Some(h)) => (w.clamp(minw, maxw), h.clamp(minh, maxh)),
        (Some(w), None) => {
            let w = w.clamp(minw, maxw);
            (w, ratio.map_or(natural.1, |r| w / r).clamp(minh, maxh))
        }
        (None, Some(h)) => {
            let h = h.clamp(minh, maxh);
            (ratio.map_or(natural.0, |r| h * r).clamp(minw, maxw), h)
        }
        (None, None) => {
            // CSS2 replaced-element intrinsic-ratio constraint table.
            let (w, h) = (natural.0, natural.1);
            if w < minw && h > maxh {
                (minw, maxh)
            } else if w > maxw && h < minh {
                (maxw, minh)
            } else if w > maxw && h > maxh {
                let k = (maxw / w).min(maxh / h);
                ((w * k).max(minw), (h * k).max(minh))
            } else if w < minw && h < minh {
                let k = (minw / w).max(minh / h);
                ((w * k).min(maxw), (h * k).min(maxh))
            } else if w > maxw {
                (maxw, (maxw * h / w).max(minh))
            } else if w < minw {
                (minw, (minw * h / w).min(maxh))
            } else if h > maxh {
                ((maxh * w / h).max(minw), maxh)
            } else if h < minh {
                ((minh * w / h).min(maxw), minh)
            } else {
                (w, h)
            }
        }
    };
    w = grid(w, scale);
    h = grid(h, scale);
    let ml = length(s, P::MarginLeft, Some(width), None, queries, scratch, cancel, scale)?;
    let mr = length(s, P::MarginRight, Some(width), None, queries, scratch, cancel, scale)?;
    let mut margin = Edges {
        left: ml.unwrap_or(0.),
        right: mr.unwrap_or(0.),
        top: length(s, P::MarginTop, Some(width), Some(0.), queries, scratch, cancel, scale)?.unwrap(),
        bottom: length(s, P::MarginBottom, Some(width), Some(0.), queries, scratch, cancel, scale)?.unwrap(),
    };
    if block {
        let remainder = width - ex - w - margin.left - margin.right;
        if remainder >= 0. && (ml.is_none() || mr.is_none()) {
            match (ml.is_none(), mr.is_none()) {
                (true, true) => {
                    let half = ((remainder * scale * 64.).round() / 2.).trunc() / (scale * 64.);
                    margin.left = if rtl { remainder - half } else { half };
                    margin.right = remainder - margin.left;
                }
                (true, false) => margin.left = remainder,
                (false, true) => margin.right = remainder,
                _ => {}
            }
        } else if rtl {
            margin.left += remainder;
        } else {
            margin.right += remainder;
        }
    }
    if ![w, h, ex, ey, margin.left, margin.right, margin.top, margin.bottom].into_iter().all(f64::is_finite) {
        return Err(Error::Invalid);
    }
    Ok(Used {
        width: w,
        height: Some(h),
        padding,
        border,
        margin,
        min_height: minh,
        max_height: maxh,
        relative: Point::default(),
        bfc: true,
        rtl: s.axes().direction == crate::css::style::Direction::Rtl,
    })
}
pub(super) fn geometry(u: Used) -> Geometry {
    Geometry {
        border_box: Rect { x: u.margin.left, y: 0., width: u.width + u.padding.horizontal() + u.border.horizontal(), height: u.height.unwrap() + u.padding.vertical() + u.border.vertical() },
        padding: u.padding,
        border_width: u.border,
        margin: u.margin,
    }
}
