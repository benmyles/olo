//! Load a local HTML file (its linked stylesheets read from disk, relative to
//! it) and print what the engine made of it: the error, or the fallbacks and
//! the box tree with geometry. For debugging pages saved to /var/tmp.
//!
//!   cargo run --release -p olo-web --example page_dump -- FILE.html [WIDTH] [--boxes] [--style CLASS] [--png OUT.png [HEIGHT [Y]]] [--issues]
//!
//! `--style CLASS` prints the computed font, spacing and bounds of every box
//! whose element has that class, for comparing with Chrome's getComputedStyle.
#[path = "../tests/support/native_font_host.rs"]
mod native_host;
use olo_web::{
    css::{
        color, math,
        properties::Keyword,
        style::{self, Axes, FontMetrics, FontRequest, Host},
    },
    font::{Builder, FontSet},
    layout, page, paint,
    text::shaping,
};
use std::{path::Path, sync::Arc};
/// The test host with the viewport units the browser's own host resolves.
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
    fn factor(&self, unit: math::RelativeUnit, axes: Option<Axes>) -> math::Result<f64> {
        use math::RelativeUnit::*;
        let (w, h) = (self.1.width, self.1.height);
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
            _ => return Err(math::Error::UnitContextUnavailable(unit)),
        } / 100.)
    }
}
fn number(v: &style::Value) -> String {
    match v {
        style::Value::Numeric(n) => n.expression().constant().map_or_else(|| "?".into(), |q| format!("{}", q.value)),
        other => format!("{other:?}"),
    }
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = Path::new(&args[0]);
    let width: f64 = args.get(1).and_then(|w| w.parse().ok()).unwrap_or(1000.);
    let boxes = args.iter().any(|a| a == "--boxes");
    let style_class = args.iter().position(|a| a == "--style").and_then(|i| args.get(i + 1)).cloned();
    let issues = args.iter().any(|a| a == "--issues");
    let png = args.iter().position(|a| a == "--png").map(|i| {
        (args.get(i + 1).cloned().unwrap_or_else(|| "page.png".into()), args.get(i + 2).and_then(|h| h.parse().ok()).unwrap_or(700u32), args.get(i + 3).and_then(|y| y.parse().ok()).unwrap_or(0i32))
    });
    // Olo's Noto faces and the system's fonts, with Chrome's family policy,
    // as the browser does.
    let mut fonts = Builder::new(Default::default());
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts");
    for file in ["NotoSans[wght].ttf", "NotoSans-Italic[wght].ttf", "NotoSansMono[wght].ttf", "NotoSansSymbols[wght].ttf", "NotoSansSymbols2-Regular.ttf", "NotoSansMath-Regular.ttf"] {
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
    let viewport = layout::Viewport { width, height: 700., scale: 1. };
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let source: Arc<str> = Arc::from(std::fs::read_to_string(file).unwrap());
    let dir = file.parent().unwrap().to_path_buf();
    // The browser's two phases: prepare (sheets, @font-face files from a
    // `fonts/` directory next to the page), then style, layout and paint
    // with a host bound to the page's font scope.
    let media = olo_web::css::media::Media::screen(viewport.width, viewport.height, viewport.scale, false);
    let prepared = page::prepare(
        source,
        &scope,
        media,
        true,
        None,
        |_, _| {},
        |kind, _base, href| {
            let name = href.split(['?', '#']).next().unwrap_or(href);
            match kind {
                page::ResourceKind::Stylesheet => match std::fs::read_to_string(dir.join(name)) {
                    Ok(css) => Ok(Some(page::Resource::Stylesheet(olo_web::css::Syntax::parse(Arc::from(css), Default::default(), None)?, None))),
                    Err(_) => Ok(None),
                },
                page::ResourceKind::Font => {
                    let file = dir.join("fonts").join(Path::new(name).file_name().unwrap_or_default());
                    Ok(std::fs::read(file).ok().map(|b| page::Resource::Font(Arc::from(b))))
                }
                page::ResourceKind::Image => {
                    // PNG files next to the page (tiny-skia decodes; its pixels
                    // are premultiplied, which the image type takes as straight:
                    // exact for opaque images).
                    let path = dir.join(name.trim_start_matches('/'));
                    let Ok(pixmap) = tiny_skia::Pixmap::load_png(&path) else { return Ok(None) };
                    let image = olo_web::image::Image::from_rgba(pixmap.width(), pixmap.height(), pixmap.data().to_vec(), Default::default(), None).map_err(|_| olo_web::paint::Error::Limit)?;
                    Ok(Some(page::Resource::Image(image)))
                }
            }
        },
    );
    let mut prepared_scope = scope.clone();
    let result = prepared.and_then(|prepared| {
        prepared_scope = prepared.scope().clone();
        if issues {
            // A histogram of the dropped declarations and selectors.
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            for issue in prepared.issues() {
                use olo_web::css::cascade::Issue;
                let key = match issue {
                    Issue::Property(e, name) => format!("property {name}: {e:?}"),
                    Issue::Selector(e, text) => format!("selector {e:?}: {text}"),
                    other => format!("{other:?}"),
                };
                *counts.entry(key).or_default() += 1;
            }
            let mut counts: Vec<_> = counts.into_iter().collect();
            counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            for (key, n) in counts {
                println!("{n:5} {key}");
            }
        }
        let host = DumpHost(native_host::NativeHost(prepared.scope().clone()), viewport);
        page::finish(prepared, &host, &mut shaper, &mut painter, viewport, None, |_, _| {})
    });
    match result {
        Err(e) => {
            println!("error: {e}");
            std::process::exit(1);
        }
        Ok(page) => {
            println!("title: {}", page.title);
            println!("fallbacks ({}): {}", page.fallbacks.total(), page.fallbacks);
            println!("stylesheet issues: {}", page.stylesheet_issues);
            if let Some(class) = style_class {
                use olo_web::css::style::Property as P;
                let layout = page.scene.layout();
                let tree = layout.cache().tree();
                for (id, b) in tree.iter() {
                    let classes = page.document.attribute(b.node, "class").ok().flatten().unwrap_or("");
                    if !classes.split_ascii_whitespace().any(|c| c == class) {
                        continue;
                    }
                    let Some(s) = tree.style(id) else { continue };
                    let f = &s.font().request;
                    let face = prepared_scope.first_available(f, None).ok().and_then(|sel| prepared_scope.catalog().face(sel.face).ok().map(|face| face.families.join("/")));
                    println!("  face {}", face.unwrap_or_default());
                    println!(
                        "{:?} <{} class=\"{classes}\"> bounds {:?}\n  font {:?} size {} weight {} letter-spacing {} line-height {} max-width {} color {:?}",
                        b.kind,
                        page.document.name(b.node).ok().flatten().unwrap_or("#text"),
                        layout.bounds(id),
                        f.families.as_slice().iter().map(|f| format!("{f:?}")).collect::<Vec<_>>().join(","),
                        f.size,
                        f.weight,
                        number(s.value(P::LetterSpacing)),
                        number(s.value(P::LineHeight)),
                        number(s.value(P::MaxWidth)),
                        s.used_color(P::Color).map(|c| c.srgb8_clipped()),
                    );
                }
            }
            if let Some((file, height, top)) = png {
                // 512 × 512 tiles, as the browser paints, composed into one image.
                let (w, h) = (width as u32, height);
                let mut whole = tiny_skia::Pixmap::new(w, h).expect("pixmap");
                for y in (0..h).step_by(512) {
                    for x in (0..w).step_by(512) {
                        let region = paint::DeviceRect { x: x as i32, y: y as i32 + top, width: 512.min(w - x), height: 512.min(h - y) };
                        let tile = painter.raster(&page.scene, region, None).expect("raster");
                        let pixmap = tiny_skia::Pixmap::from_vec(tile.into_rgba(), tiny_skia::IntSize::from_wh(region.width, region.height).unwrap()).unwrap();
                        whole.draw_pixmap(x as i32, y as i32, pixmap.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
                    }
                }
                whole.save_png(&file).expect("png");
                println!("wrote {file}");
            }
            if boxes {
                let layout = page.scene.layout();
                for (id, b) in layout.cache().tree().iter() {
                    let name = page.document.name(b.node).ok().flatten().unwrap_or("#text");
                    let class = page.document.attribute(b.node, "class").ok().flatten().unwrap_or("");
                    println!("{:?} <{name} class=\"{class}\"> {:?}", b.kind, layout.bounds(id));
                    for pseudo in [olo_web::css::selectors::PseudoElement::Before, olo_web::css::selectors::PseudoElement::After] {
                        if let Some(s) = layout.cache().tree().styles().pseudo(b.node, pseudo) {
                            println!("    {pseudo:?} content {:?} display {:?}", s.value(olo_web::css::style::Property::Content), s.value(olo_web::css::style::Property::Display));
                        }
                    }
                    if let olo_web::layout::BoxKind::InlineContent { source } = b.kind
                        && let Some(text) = layout.cache().tree().text_source(source)
                    {
                        println!("    text {:?}", &text.source[..text.source.len().min(120)]);
                    }
                }
            }
        }
    }
}
