//! Computed SVG rectangle geometry shared by visible shapes and clip resources.
use crate::{
    css::{
        math,
        properties::Numeric,
        style::{Keyword as K, Property as P, Style, Value},
    },
    layout::{Error, Result},
};
use std::sync::atomic::{AtomicBool, Ordering};
pub(crate) struct Rectangle([Option<Numeric>; 6]);
impl Rectangle {
    pub fn read(style: &Style) -> Result<Self> {
        let mut values: [Option<Numeric>; 6] = Default::default();
        for (i, p) in [P::X, P::Y, P::Width, P::Height, P::Rx, P::Ry].into_iter().enumerate() {
            values[i] = match style.value(p) {
                Value::Numeric(n) => Some(n.clone()),
                Value::Keyword(K::Auto) if i >= 2 => None,
                _ => return Err(Error::Unsupported("SVG rectangle geometry")),
            };
        }
        Ok(Self(values))
    }
    pub fn append(&self, path: &mut tiny_skia::PathBuilder, viewport: [f64; 2], scratch: &mut math::Scratch, segments: usize, cancel: Option<&AtomicBool>) -> Result<()> {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Error::Cancelled);
        }
        if path.len().saturating_add(10) > segments {
            return Err(Error::Limit);
        }
        struct Basis(f64);
        impl math::Context for Basis {
            fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
                if unit == math::RelativeUnit::Percentage { Ok(self.0 / 100.) } else { Err(math::Error::ContextUnavailable) }
            }
        }
        let mut v = [0.; 6];
        for (i, n) in self.0.iter().enumerate() {
            if let Some(n) = n {
                v[i] = n.resolve(&Basis(viewport[i % 2]), scratch, Default::default(), cancel).map_err(crate::css::style::Error::Property)?;
            }
        }
        let [x, y, w, h, mut rx, mut ry] = v;
        if w <= 0. || h <= 0. {
            return Ok(());
        }
        if self.0[4].is_none() {
            rx = ry;
        }
        if self.0[5].is_none() {
            ry = rx;
        }
        rx = rx.min(w / 2.);
        ry = ry.min(h / 2.);
        if [x, y, w, h, x + w, y + h, rx, ry].iter().any(|v| !v.is_finite() || v.abs() > f64::from(f32::MAX)) {
            return Err(Error::Unsupported("SVG rectangle precision"));
        }
        let [x, y, w, h, rx, ry] = [x, y, w, h, rx, ry].map(|v| v as f32);
        if rx == 0. || ry == 0. {
            if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
                path.push_rect(rect);
            }
        } else {
            // SVG's quarter-ellipses, with the same cubic approximation as
            // the retained path backend. Fractional edge parity is evaluated.
            let k = 0.552_284_8;
            path.move_to(x + rx, y);
            path.line_to(x + w - rx, y);
            path.cubic_to(x + w - rx + k * rx, y, x + w, y + ry - k * ry, x + w, y + ry);
            path.line_to(x + w, y + h - ry);
            path.cubic_to(x + w, y + h - ry + k * ry, x + w - rx + k * rx, y + h, x + w - rx, y + h);
            path.line_to(x + rx, y + h);
            path.cubic_to(x + rx - k * rx, y + h, x, y + h - ry + k * ry, x, y + h - ry);
            path.line_to(x, y + ry);
            path.cubic_to(x, y + ry - k * ry, x + rx - k * rx, y, x + rx, y);
            path.close();
        }
        Ok(())
    }
}
