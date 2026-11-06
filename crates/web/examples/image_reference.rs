//! Export original image-only fixtures for independent Chrome comparisons.
use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    image::Image,
    layout, page, paint,
    text::shaping,
};
use std::{collections::HashMap, path::Path, sync::Arc};
#[path = "../tests/support/paint_host.rs"]
mod paint_host;
fn main() {
    let out = std::env::args().nth(1).expect("owned output directory");
    std::fs::create_dir_all(&out).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/image-paint-fixtures");
    let inputs: serde_json::Value = serde_json::from_str(include_str!("../tests/image-paint-fixtures/inputs.json")).unwrap();
    let raw: serde_json::Value = serde_json::from_str(include_str!("../tests/image-paint-fixtures/assets.json")).unwrap();
    let assets: HashMap<_, _> = raw
        .as_object()
        .unwrap()
        .iter()
        .map(|(url, a)| {
            (
                url.clone(),
                Image::from_rgba(
                    a["width"].as_u64().unwrap() as u32,
                    a["height"].as_u64().unwrap() as u32,
                    a["rgba"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect(),
                    Default::default(),
                    None,
                )
                .unwrap(),
            )
        })
        .collect();
    let mut builder = Builder::new(Default::default());
    builder.load_file(&root.join("../inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(builder.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let host = paint_host::Scaled::new(scope.clone(), scale);
        for input in inputs.as_array().unwrap() {
            let source =
                Arc::from(format!("<!doctype html><style>{}{}</style>{}", include_str!("../tests/image-paint-fixtures/base.css"), input["css"].as_str().unwrap(), input["body"].as_str().unwrap()));
            let page = page::load_assets(
                source,
                &host,
                &scope,
                &mut shaper,
                &mut painter,
                layout::Viewport { width: 100., height: 100., scale },
                None,
                |_, _| {},
                |kind, _, href| {
                    assert_eq!(kind, page::ResourceKind::Image);
                    Ok(Some(page::Resource::Image(assets[href].clone())))
                },
            )
            .unwrap();
            assert_eq!(page.stylesheet_issues, 0, "{}", input["id"]);
            painter
                .raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: (100. * scale) as u32, height: (100. * scale) as u32 }, None)
                .unwrap()
                .save_png(Path::new(&out).join(format!("{}-{scale}.png", input["id"].as_str().unwrap())))
                .unwrap();
        }
    }
}
