//! Image ingestion and retained tile sampling; no codec/network/GPU timing.
#[path = "support/features.rs"]
mod build_features;
#[path = "../tests/support/paint_host.rs"]
mod paint_host;
use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    image::Image,
    layout, page, paint,
    text::shaping,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    hint::black_box,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};
fn pixels(w: u32, h: u32, opaque: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    let mut value = 7u32;
    for _ in 0..w * h {
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        let mut p = value.to_le_bytes();
        if opaque {
            p[3] = 255;
        }
        out.extend(p);
    }
    out
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "50".into()).parse().unwrap();
    assert!(rounds > 0);
    let cancel = AtomicBool::new(false);
    let mut fonts = Builder::new(Default::default());
    fonts.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(fonts.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let mut rows = Vec::new();
    for opaque in [true, false] {
        let source = pixels(1920, 1080, opaque);
        let at = Instant::now();
        for _ in 0..rounds {
            black_box(Image::from_rgba(1920, 1080, source.clone(), Default::default(), Some(&cancel)).unwrap());
        }
        rows.push(json!({"operation":"ingest","opaque":opaque,"micros":at.elapsed().as_secs_f64()*1e6/rounds as f64,"pixels":1920*1080,"source_sha256":format!("{:x}",Sha256::digest(&source)),"result_sha256":format!("{:x}",Sha256::digest(Image::from_rgba(1920,1080,source,Default::default(),Some(&cancel)).unwrap().rgba()))}));
        let pixels = Image::from_rgba(64, 64, pixels(64, 64, opaque), Default::default(), Some(&cancel)).unwrap();
        for scale in [1., 1.25, 2.] {
            let host = paint_host::Scaled::new(scope.clone(), scale);
            let source: Arc<str> =
                Arc::from("<!doctype html><style>body{margin:0;font-size:0;line-height:0}img{display:block;width:600px;height:400px;margin-left:1.5px;margin-top:2.25px}</style><img src=x>");
            let page = page::load_assets(
                source.clone(),
                &host,
                &scope,
                &mut shaper,
                &mut painter,
                layout::Viewport { width: 800., height: 600., scale },
                Some(&cancel),
                |_, _| {},
                |_, _, _| Ok(Some(page::Resource::Image(pixels.clone()))),
            )
            .unwrap();
            let region = paint::DeviceRect { x: 17, y: 23, width: 256, height: 256 };
            let warm = painter.raster(&page.scene, region, Some(&cancel)).unwrap();
            let at = Instant::now();
            for _ in 0..rounds {
                black_box(painter.raster(&page.scene, region, Some(&cancel)).unwrap());
            }
            rows.push(json!({"operation":"raster","opaque":opaque,"scale":scale,"micros":at.elapsed().as_secs_f64()*1e6/rounds as f64,"pixels":256*256,"source_sha256":format!("{:x}",Sha256::digest(source.as_bytes())),"result_sha256":format!("{:x}",Sha256::digest(warm.rgba()))}));
        }
    }
    println!(
        "{}",
        json!({"features":build_features::enabled(),"rounds":rounds,"rows":rows,"scope":"Authored RGBA input clone/ingestion and retained 256x256 CPU tiles. Real false cancellation flag. Excludes decode/network/GPU/presentation/animation. Ingestion includes the fresh caller buffer clone; raster includes output allocation/query and cleanup."})
    );
}
