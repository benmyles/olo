//! Export authored static paint fixtures, independently compared to Chrome.
use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
use std::{path::Path, sync::Arc};
#[path = "../tests/support/paint_host.rs"]
mod paint_host;
use paint_host::Scaled;
fn main() {
    let out = std::env::args().nth(1).expect("output directory");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let inputs: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("tests/paint-fixtures/inputs.json")).unwrap()).unwrap();
    let mut b = Builder::new(Default::default());
    b.load_file(&root.join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    for scale in [1., 1.5, 2.] {
        let host = Scaled::new(scope.clone(), scale);
        for input in inputs.as_array().unwrap() {
            let source = Arc::from(format!("<!doctype html><style>{}{}</style>{}", include_str!("../tests/paint-fixtures/base.css"), input["css"].as_str().unwrap(), input["body"].as_str().unwrap()));
            let page = page::load(source, &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 100., height: 100., scale }, None, |_, _| {}).unwrap();
            assert_eq!(page.stylesheet_issues, 0);
            painter
                .raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: (100. * scale) as u32, height: (100. * scale) as u32 }, None)
                .unwrap()
                .save_png(Path::new(&out).join(format!("{}-{scale}.png", input["id"].as_str().unwrap())))
                .unwrap();
        }
    }
}
