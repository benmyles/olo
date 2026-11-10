//! Viewport-unit context. Media selection and window metrics keep the full size.
use crate::{
    css::{
        color, math,
        style::{self, Axes, FontMetrics, FontRequest, Host, Keyword, ScrollbarWidths},
        variables,
    },
    layout,
};
use std::{cell::Cell, sync::Arc};
pub(super) struct ViewportHost<'a, H> {
    inner: &'a H,
    full: [f64; 2],
    units: Cell<[f64; 2]>,
}
impl<'a, H: Host> ViewportHost<'a, H> {
    pub(super) fn new(inner: &'a H, viewport: layout::Viewport, previous: Option<&layout::Layout>) -> Self {
        let full = [viewport.width, viewport.height];
        let units = previous.map_or(full, |p| p.cache().tree().viewport_unit_size(viewport, inner.scrollbar_widths()));
        Self { inner, full, units: Cell::new(units) }
    }
    pub(super) fn update(&self, layout: &layout::Layout) -> bool {
        let units = layout.cache().tree().viewport_unit_size(layout.viewport_geometry().full, self.inner.scrollbar_widths());
        self.units.replace(units) != units
    }
}
impl<H: Host> Host for ViewportHost<'_, H> {
    fn revision(&self) -> u64 {
        let units = self.units.get();
        self.inner.revision() ^ if units == self.full { 0 } else { units[0].to_bits().rotate_left(17) ^ units[1].to_bits().rotate_left(37) ^ 0xb711ea41d38d1765 }
    }
    fn scrollbar_widths(&self) -> ScrollbarWidths {
        self.inner.scrollbar_widths()
    }
    fn environment(&self) -> style::Result<Option<Arc<variables::Environment>>> {
        self.inner.environment()
    }
    fn scheme(&self) -> color::Scheme {
        self.inner.scheme()
    }
    fn system_color(&self, c: color::SystemColor, s: color::Scheme) -> color::Result<color::Color> {
        self.inner.system_color(c, s)
    }
    fn font(&self, r: &FontRequest) -> style::Result<FontMetrics> {
        self.inner.font(r)
    }
    fn font_context(&self) -> u64 {
        self.inner.font_context()
    }
    fn font_size(&self, k: Keyword, p: f64) -> style::Result<f64> {
        self.inner.font_size(k, p)
    }
    fn device_scale(&self) -> f64 {
        self.inner.device_scale()
    }
    fn tree_count(&self, k: math::TreeCount) -> math::Result<u32> {
        self.inner.tree_count(k)
    }
    fn factor(&self, unit: math::RelativeUnit, axes: Option<Axes>) -> math::Result<f64> {
        // Also tells the embedder that this sheet depends on viewport units.
        let original = self.inner.factor(unit, axes)?;
        let [w, h] = self.units.get();
        if [w, h] == self.full {
            return Ok(original);
        }
        use math::RelativeUnit::*;
        let vertical = axes.unwrap_or_default().writing_mode != style::WritingMode::HorizontalTb;
        Ok(match unit {
            Vw | Svw | Lvw | Dvw => w,
            Vh | Svh | Lvh | Dvh => h,
            Vi | Svi | Lvi | Dvi => {
                if vertical {
                    h
                } else {
                    w
                }
            }
            Vb | Svb | Lvb | Dvb => {
                if vertical {
                    w
                } else {
                    h
                }
            }
            Vmin | Svmin | Lvmin | Dvmin => w.min(h),
            Vmax | Svmax | Lvmax | Dvmax => w.max(h),
            _ => return Ok(original),
        } / 100.)
    }
}
