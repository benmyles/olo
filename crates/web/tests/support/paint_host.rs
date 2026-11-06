use olo_web::css::{
    color, math,
    style::{self, Axes, FontMetrics, FontRequest, Host, Keyword},
};
#[path = "native_font_host.rs"]
pub mod native_host;
pub struct Scaled(pub native_host::NativeHost, pub f64);
impl Host for Scaled {
    fn revision(&self) -> u64 {
        self.0.revision() ^ self.1.to_bits()
    }
    fn scheme(&self) -> color::Scheme {
        self.0.scheme()
    }
    fn system_color(&self, c: color::SystemColor, s: color::Scheme) -> color::Result<color::Color> {
        self.0.system_color(c, s)
    }
    fn font(&self, r: &FontRequest) -> style::Result<FontMetrics> {
        self.0.font(r)
    }
    fn font_context(&self) -> u64 {
        self.0.font_context()
    }
    fn font_size(&self, k: Keyword, p: f64) -> style::Result<f64> {
        self.0.font_size(k, p)
    }
    fn device_scale(&self) -> f64 {
        self.1
    }
    fn factor(&self, _: math::RelativeUnit, _: Option<Axes>) -> math::Result<f64> {
        Err(math::Error::ContextUnavailable)
    }
}

impl Scaled {
    pub fn new(scope: olo_web::font::Scope, scale: f64) -> Self {
        Self(native_host::NativeHost(scope), scale)
    }
}
