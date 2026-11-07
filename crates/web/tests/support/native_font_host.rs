//! Integration adapter for tests/measurements, not an OS font-discovery policy.
use olo_web::{
    css::{
        color,
        properties::Keyword,
        style::{self, FontMetrics, FontRequest, Host},
    },
    font::Scope,
};
#[path = "style_host.rs"]
mod fixed;
pub struct NativeHost(pub Scope);
impl Host for NativeHost {
    fn revision(&self) -> u64 {
        self.0.revision()
    }
    fn scheme(&self) -> color::Scheme {
        color::Scheme::Light
    }
    fn system_color(&self, c: color::SystemColor, s: color::Scheme) -> color::Result<color::Color> {
        fixed::FixtureHost.system_color(c, s)
    }
    fn font(&self, r: &FontRequest) -> style::Result<FontMetrics> {
        self.0.metrics(r, None).map(|m| m.style_at(1.)).map_err(|_| style::Error::ContextUnavailable)
    }
    fn font_context(&self) -> u64 {
        self.0.context()
    }
    fn font_size(&self, k: Keyword, p: f64) -> style::Result<f64> {
        fixed::FixtureHost.font_size(k, p)
    }
    fn device_scale(&self) -> f64 {
        1.
    }
}
