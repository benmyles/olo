//! Time styling (and the layout phases) of a saved page, for making the
//! style job fast: the page's sheets from disk next to it, as page_dump.
//! With OLO_STYLE_TIMING=1 the style job prints where its time goes.
//!
//!   cargo run --release -p olo-web --example style_bench -- FILE.html [ROUNDS]
#[path = "../tests/support/native_font_host.rs"]
mod native_host;
use olo_web::{
    css::{
        color, math,
        properties::Keyword,
        style::{self, Axes, FontMetrics, FontRequest, Host},
    },
    font::{Builder, FontSet},
    layout, page,
    text::shaping,
};
use std::{path::Path, sync::Arc};
struct DumpHost(native_host::NativeHost, layout::Viewport);
impl Host for DumpHost {
    fn revision(&self) -> u64 {
        self.0.revision()
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
        self.1.scale
    }
    fn factor(&self, unit: math::RelativeUnit, _axes: Option<Axes>) -> math::Result<f64> {
        use math::RelativeUnit::*;
        let (w, h) = (self.1.width, self.1.height);
        Ok(match unit {
            Vw | Svw | Lvw | Dvw | Vi | Svi | Lvi | Dvi => w,
            Vh | Svh | Lvh | Dvh | Vb | Svb | Lvb | Dvb => h,
            Vmin | Svmin | Lvmin | Dvmin => w.min(h),
            Vmax | Svmax | Lvmax | Dvmax => w.max(h),
            _ => return Err(math::Error::UnitContextUnavailable(unit)),
        } / 100.)
    }
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = Path::new(&args[0]);
    let rounds: usize = args.get(1).and_then(|r| r.parse().ok()).unwrap_or(5);
    let mut fonts = Builder::new(Default::default());
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts");
    for file in ["NotoSans[wght].ttf", "NotoSans-Italic[wght].ttf", "NotoSansMono[wght].ttf"] {
        let path = data.join(file);
        if path.is_file() {
            fonts.load_file(&path, |_| {}, None).unwrap();
        }
    }
    let system = olo_web::font::system::directories();
    olo_web::font::system::map_directories(&mut fonts, &system, |path| {
        let bytes = std::fs::read(path).ok()?;
        Some(Arc::new(bytes) as Arc<dyn AsRef<[u8]> + Send + Sync>)
    });
    let scope = FontSet::new(fonts.finish(None).unwrap(), olo_web::font::system::chrome_policy().unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let viewport = layout::Viewport { width: 1000., height: 700., scale: 1. };
    let media = olo_web::css::media::Media::screen(viewport.width, viewport.height, viewport.scale, false);
    let source: Arc<str> = Arc::from(std::fs::read_to_string(file).unwrap());
    let dir = file.parent().unwrap().to_path_buf();
    let prepared = page::prepare(
        source,
        &scope,
        media,
        false,
        None,
        |_, _| {},
        |kind, _base, href| {
            let name = href.split(['?', '#']).next().unwrap_or(href);
            match kind {
                page::ResourceKind::Stylesheet => match std::fs::read_to_string(dir.join(name)) {
                    Ok(css) => Ok(Some(page::Resource::Stylesheet(olo_web::css::Syntax::parse(Arc::from(css), Default::default(), None)?, None))),
                    Err(_) => Ok(None),
                },
                _ => Ok(None),
            }
        },
    )
    .unwrap();
    let host = DumpHost(native_host::NativeHost(prepared.scope().clone()), viewport);
    let measurer = prepared.measurer();
    let mut shaper = shaping::Engine::default();
    let mut best = [f64::INFINITY; 4];
    for _ in 0..rounds {
        let m = measurer.measure(prepared.document(), &host, &mut shaper, viewport, None).unwrap();
        let p = m.phases_ms();
        for i in 0..4 {
            best[i] = best[i].min(p[i]);
        }
        println!("style {:.1} ms, boxes {:.1} ms, text {:.1} ms, layout {:.1} ms", p[0], p[1], p[2], p[3]);
    }
    println!("best: style {:.1} ms, boxes {:.1} ms, text {:.1} ms, layout {:.1} ms", best[0], best[1], best[2], best[3]);
}
