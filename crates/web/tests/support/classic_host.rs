//! Native classic viewport geometry against captured Wayland Chrome.
use olo_web::{
    css::{
        color, math,
        style::{self, Axes, FontMetrics, FontRequest, Host, Keyword},
    },
    font::{Builder, FontSet, GenericPolicy},
    layout,
};
use std::path::Path;
#[path = "paint_host.rs"]
pub mod paint_host;
pub struct Classic {
    pub base: paint_host::Scaled,
    pub view: layout::Viewport,
    pub widths: style::ScrollbarWidths,
}
impl Classic {
    pub fn new(scope: olo_web::font::Scope, view: layout::Viewport) -> Self {
        Self { base: paint_host::Scaled::new(scope, view.scale), view, widths: style::ScrollbarWidths { auto: 15., thin: 10. } }
    }
}
impl Host for Classic {
    fn revision(&self) -> u64 {
        self.base.revision()
            ^ self.view.width.to_bits().rotate_left(7)
            ^ self.view.height.to_bits().rotate_left(29)
            ^ self.widths.auto.to_bits().rotate_left(11)
            ^ self.widths.thin.to_bits().rotate_left(43)
    }
    fn scrollbar_widths(&self) -> style::ScrollbarWidths {
        self.widths
    }
    fn scheme(&self) -> color::Scheme {
        self.base.scheme()
    }
    fn system_color(&self, c: color::SystemColor, s: color::Scheme) -> color::Result<color::Color> {
        self.base.system_color(c, s)
    }
    fn font(&self, r: &FontRequest) -> style::Result<FontMetrics> {
        self.base.font(r)
    }
    fn font_context(&self) -> u64 {
        self.base.font_context()
    }
    fn font_size(&self, k: Keyword, p: f64) -> style::Result<f64> {
        self.base.font_size(k, p)
    }
    fn device_scale(&self) -> f64 {
        self.view.scale
    }
    fn factor(&self, u: math::RelativeUnit, _: Option<Axes>) -> math::Result<f64> {
        match u {
            math::RelativeUnit::Vw => Ok(self.view.width / 100.),
            math::RelativeUnit::Vh => Ok(self.view.height / 100.),
            _ => Err(math::Error::ContextUnavailable),
        }
    }
}
pub fn scope() -> olo_web::font::Scope {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
}
