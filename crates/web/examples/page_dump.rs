//! Load a local HTML file (its linked stylesheets read from disk, relative to
//! it) and print what the engine made of it: the error, or the fallbacks and
//! the box tree with geometry. For debugging pages saved to /var/tmp.
//!
//!   cargo run --release -p olo-web --example page_dump -- FILE.html [WIDTH] [--boxes]
#[path = "../tests/support/native_font_host.rs"]
mod native_host;
use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
use std::{path::Path, sync::Arc};
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = Path::new(&args[0]);
    let width: f64 = args.get(1).and_then(|w| w.parse().ok()).unwrap_or(1000.);
    let boxes = args.iter().any(|a| a == "--boxes");
    let mut fonts = Builder::new(Default::default());
    let font = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts/NotoSans[wght].ttf");
    fonts.load_file(&font, |_| {}, None).unwrap();
    let scope = FontSet::new(fonts.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let source: Arc<str> = Arc::from(std::fs::read_to_string(file).unwrap());
    let dir = file.parent().unwrap().to_path_buf();
    let viewport = layout::Viewport { width, height: 700., scale: 1. };
    let result = page::load_resources(
        source,
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
        |_, href| {
            let path = dir.join(href.split(['?', '#']).next().unwrap_or(href));
            match std::fs::read_to_string(&path) {
                Ok(css) => Ok(Some(olo_web::css::Syntax::parse(Arc::from(css), Default::default(), None)?)),
                Err(_) => Ok(None),
            }
        },
    );
    match result {
        Err(e) => {
            println!("error: {e}");
            std::process::exit(1);
        }
        Ok(page) => {
            println!("title: {}", page.title);
            println!("fallbacks ({}): {}", page.fallbacks.total(), page.fallbacks);
            println!("stylesheet issues: {}", page.stylesheet_issues);
            if boxes {
                let layout = page.scene.layout();
                for (id, b) in layout.cache().tree().iter() {
                    let name = page.document.name(b.node).ok().flatten().unwrap_or("#text");
                    let class = page.document.attribute(b.node, "class").ok().flatten().unwrap_or("");
                    println!("{:?} <{name} class=\"{class}\"> {:?}", b.kind, layout.bounds(id));
                }
            }
        }
    }
}
