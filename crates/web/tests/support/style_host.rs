use olo_web::css::{
    color, properties as p,
    style::{self, FontMetrics, FontRequest, Host},
};
/// Explicit native context. Metrics are supplied data, not font-selection or
/// shaping evidence. Selected passing assertions do not depend on these ratios.
pub struct FixtureHost;
impl Host for FixtureHost {
    fn revision(&self) -> u64 {
        1
    }
    fn scheme(&self) -> color::Scheme {
        color::Scheme::Light
    }
    fn system_color(&self, c: color::SystemColor, _: color::Scheme) -> color::Result<color::Color> {
        match c {
            color::SystemColor::CanvasText => Ok(color::Color::new(color::Space::Srgb, [0.; 3], 1., 0)),
            color::SystemColor::Canvas => Ok(color::Color::new(color::Space::Srgb, [1.; 3], 1., 0)),
            _ => Err(color::Error::ContextUnavailable),
        }
    }
    fn font(&self, r: &FontRequest) -> style::Result<FontMetrics> {
        Ok(FontMetrics { id: 1, ex: r.size * 0.5, cap: r.size * 0.75, ch: r.size * 0.5, ic: r.size, normal_line_height: r.size * 1.25 })
    }
    fn font_size(&self, k: p::Keyword, _: f64) -> style::Result<f64> {
        if k == p::Keyword::Medium { Ok(16.) } else { Err(style::Error::Unsupported("UA font-size keyword policy")) }
    }
    fn device_scale(&self) -> f64 {
        1.
    }
}
