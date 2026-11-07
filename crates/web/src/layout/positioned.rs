//! Absolute non-replaced axis constraints (CSS2.2 10.3.7/10.6.4).
//! Inputs are resolved content-box lengths in CSS pixels. Containing-block
//! selection, static-position discovery and intrinsic measurement belong to
//! the layout caller; this allocation-free solver does not traverse the DOM.
use super::{Error, Result};
#[derive(Clone, Copy, Debug)]
pub enum Axis {
    Horizontal { rtl: bool, static_rtl: bool },
    Vertical,
}
#[derive(Clone, Copy, Debug)]
pub struct Input {
    pub axis: Axis,
    pub containing: f64,
    /// Left/top and right/bottom offsets; None is auto.
    pub inset: [Option<f64>; 2],
    pub size: Option<f64>,
    pub margin: [Option<f64>; 2],
    /// Total padding + border extent along this axis.
    pub decoration: f64,
    pub minimum: f64,
    pub maximum: Option<f64>,
    /// Min/max-content widths for horizontal shrink-to-fit; for vertical
    /// layout both entries must be the measured auto content height.
    pub intrinsic: [f64; 2],
    /// Distance from the static-position side of the containing block.
    pub static_position: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Used {
    pub inset: [f64; 2],
    pub size: f64,
    pub margin: [f64; 2],
}
impl Used {
    pub fn border_start(self) -> f64 {
        self.inset[0] + self.margin[0]
    }
}
/// Resolve in layout quanta, bounding arithmetic before doing any sums.
/// No allocation, iteration over content, or UI work; at most three equations.
pub fn solve(input: Input, scale: f64) -> Result<Used> {
    let unit = scale * 64.;
    if !scale.is_finite() || scale <= 0. || !unit.is_finite() || unit == 0. {
        return Err(Error::Invalid);
    }
    let q = |v: f64| -> Result<f64> {
        if !v.is_finite() {
            return Err(Error::Invalid);
        }
        let value = (v * unit).trunc();
        if !value.is_finite() || value.abs() > (1u64 << 48) as f64 {
            return Err(Error::Limit);
        }
        Ok(value)
    };
    if input.containing < 0.
        || input.decoration < 0.
        || input.minimum < 0.
        || input.maximum.is_some_and(|v| v < 0.)
        || input.size.is_some_and(|v| v < 0.)
        || input.intrinsic[0] < 0.
        || input.intrinsic[1] < input.intrinsic[0]
        || matches!(input.axis, Axis::Vertical) && input.intrinsic[0] != input.intrinsic[1]
    {
        return Err(Error::Invalid);
    }
    let i = Input {
        containing: q(input.containing)?,
        inset: [input.inset[0].map(q).transpose()?, input.inset[1].map(q).transpose()?],
        size: input.size.map(q).transpose()?,
        margin: [input.margin[0].map(q).transpose()?, input.margin[1].map(q).transpose()?],
        decoration: q(input.decoration)?,
        minimum: q(input.minimum)?,
        maximum: input.maximum.map(q).transpose()?,
        intrinsic: [q(input.intrinsic[0])?, q(input.intrinsic[1])?],
        static_position: q(input.static_position)?,
        ..input
    };
    let mut out = equation(i, i.size);
    if let Some(max) = i.maximum
        && out.size > max
    {
        out = equation(i, Some(max));
    }
    if out.size < i.minimum {
        out = equation(i, Some(i.minimum));
    }
    let result = Used { inset: out.inset.map(|v| v / unit), size: out.size / unit, margin: out.margin.map(|v| v / unit) };
    if !result.inset.into_iter().chain(result.margin).chain([result.size, result.border_start()]).all(f64::is_finite) {
        return Err(Error::Limit);
    }
    Ok(result)
}
fn equation(i: Input, specified: Option<f64>) -> Used {
    let rtl = matches!(i.axis, Axis::Horizontal { rtl: true, .. });
    let static_end = matches!(i.axis, Axis::Horizontal { static_rtl: true, .. });
    let mut inset = i.inset;
    let auto = specified.is_none() || inset.iter().any(Option::is_none);
    if inset.iter().all(Option::is_none) {
        inset[usize::from(static_end)] = Some(i.static_position);
    }
    let mut margin = i.margin.map(|v| v.unwrap_or(0.));
    let available = i.containing - i.decoration - margin[0] - margin[1] - inset[0].unwrap_or(0.) - inset[1].unwrap_or(0.);
    let size = specified.unwrap_or_else(|| if inset.iter().all(Option::is_some) { available.max(0.) } else { available.max(i.intrinsic[0]).min(i.intrinsic[1]) });
    if !auto {
        let remaining = available - size;
        match i.margin {
            [None, None] => {
                if remaining < 0. && matches!(i.axis, Axis::Horizontal { .. }) {
                    margin[usize::from(!rtl)] = remaining;
                } else {
                    margin[0] = (remaining / 2.).trunc();
                    margin[1] = remaining - margin[0];
                }
            }
            [None, Some(_)] => margin[0] = remaining,
            [Some(_), None] => margin[1] = remaining,
            _ => {}
        }
    }
    let remaining = i.containing - i.decoration - size - margin[0] - margin[1];
    let [left, right] = match inset {
        [None, Some(right)] => [remaining - right, right],
        [Some(left), None] => [left, remaining - left],
        [Some(left), Some(right)] => {
            if rtl {
                [remaining - right, right]
            } else {
                [left, remaining - left]
            }
        }
        _ => unreachable!(),
    };
    Used { inset: [left, right], size, margin }
}
