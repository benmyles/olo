//! Release-only retained paint/query benchmark. No network, JS or UI evidence.
#[path = "support/features.rs"]
mod build_features;
use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{hint::black_box, path::Path, sync::Arc, time::Instant};
#[path = "../tests/support/paint_host.rs"]
mod paint_host;
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or("30".into()).parse().unwrap();
    assert!(rounds > 0);
    let uncached = std::env::args().nth(2).as_deref() == Some("uncached");
    let mode = std::env::args().nth(3).unwrap_or_else(|| "solid".into());
    let extra = match mode.as_str() {
        "solid" => "",
        "color" => "p{border-width:3px;border-top-color:red;border-right-color:green;border-bottom-color:blue;border-left-color:black}",
        "alpha" => "p{border-width:3px;border-top-color:rgb(255 0 0 / 50%);border-right-color:rgb(0 128 0 / 50%);border-bottom-color:rgb(0 0 255 / 50%);border-left-color:rgb(0 0 0 / 50%)}",
        _ => panic!("paint mode must be solid, color or alpha"),
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut b = Builder::new(Default::default());
    b.load_file(&root.join("../../data/fonts/NotoSans[wght].ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::new(paint::Limits { fonts: if uncached { 0 } else { 64 }, ..Default::default() });
    let mut cases = Vec::new();
    for count in [100, 1000] {
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            let host = paint_host::Scaled::new(scope.clone(), scale);
            let source = Arc::from(format!(
                "<!doctype html><style>html{{font-family:'Noto Sans';font-size:16px;line-height:24px}}body{{margin:0}}p{{margin:0;padding:4px;border-width:1px;border-style:solid;border-color:gray}}p:nth-child(2n){{background-color:#eee}}{extra}</style>{}",
                "<p>office alpha beta gamma delta.</p>".repeat(count)
            ));
            let at = Instant::now();
            let page = page::load(source, &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 800., height: 600., scale }, None, |_, _| {}).unwrap();
            let load = at.elapsed().as_secs_f64() * 1e6;
            let mut geometry = Sha256::new();
            for (id, _) in page.scene.layout().cache().tree().iter() {
                let r = page.scene.layout().bounds(id).unwrap();
                for v in [r.x, r.y, r.width, r.height] {
                    geometry.update(v.to_le_bytes());
                }
            }
            let geometry_digest = format!("{:x}", geometry.finalize());
            let mut job = paint::BuildJob::new(&page.document, page.scene.layout().clone(), &mut painter, Default::default(), Default::default(), None).unwrap();
            let at = Instant::now();
            let mut max_step = 0f64;
            loop {
                let start = Instant::now();
                let p = job.step(32).unwrap();
                max_step = max_step.max(start.elapsed().as_secs_f64() * 1e6);
                if p.finished {
                    break;
                }
            }
            let scene = job.finish().unwrap();
            let scene_us = at.elapsed().as_secs_f64() * 1e6;
            let r = layout::Rect { x: 0., y: 1000., width: 256. / scale, height: 256. / scale };
            let mut ids = Vec::new();
            let at = Instant::now();
            for _ in 0..rounds * 100 {
                scene.query(r, &mut ids, None).unwrap();
                black_box(&ids);
            }
            let query_us = at.elapsed().as_secs_f64() * 1e6 / (rounds * 100) as f64;
            let expected: Vec<_> = scene
                .commands()
                .iter()
                .enumerate()
                .filter(|(_, c)| c.bounds.x < r.x + r.width && r.x < c.bounds.x + c.bounds.width && c.bounds.y < r.y + r.height && r.y < c.bounds.y + c.bounds.height)
                .map(|(i, _)| i as u32)
                .collect();
            assert_eq!(ids, expected);
            let at = Instant::now();
            for _ in 0..rounds * 100 {
                let ids: Vec<_> = scene
                    .commands()
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.bounds.x < r.x + r.width && r.x < c.bounds.x + c.bounds.width && c.bounds.y < r.y + r.height && r.y < c.bounds.y + c.bounds.height)
                    .map(|(i, _)| i as u32)
                    .collect();
                black_box(ids);
            }
            let linear_us = at.elapsed().as_secs_f64() * 1e6 / (rounds * 100) as f64;
            let region = paint::DeviceRect { x: 0, y: (1000. * scale) as i32, width: 256, height: 256 };
            let tile = painter.raster(&scene, region, None).unwrap();
            let digest = format!("{:x}", Sha256::digest(tile.rgba()));
            let at = Instant::now();
            for _ in 0..rounds {
                black_box(painter.raster(&scene, region, None).unwrap());
            }
            let raster_us = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            let stats = painter.cache_stats();
            cases.push(json!({"paragraphs":count,"scale":scale,"geometry_sha256":geometry_digest,"load_us":load,"scene_build_us":scene_us,"max_32_box_step_us":max_step,"commands":scene.commands().len(),"command_payload_bytes":scene.command_payload_bytes(),"query_us":query_us,"linear_query_us":linear_us,"tile_raster_us":raster_us,"tile_bytes":tile.rgba().len(),"tile_sha256":digest,"cached_fonts":stats.fonts,"font_source_bytes":stats.font_bytes,"cached_glyphs":stats.glyphs,"outline_payload_bytes":stats.path_bytes,"hits":stats.hits,"misses":stats.misses}));
        }
    }
    println!(
        "{}",
        json!({"features":build_features::enabled(),"rounds":rounds,"uncached":uncached,"mode":mode,"scope":"Static page/scene build; indexed vs linear query; outline cache vs disabled; 256px CPU tile. Payload excludes retained DOM/style/layout/text, allocator/header/hash/GPU costs. No website/UI evidence.","cases":cases})
    );
}
