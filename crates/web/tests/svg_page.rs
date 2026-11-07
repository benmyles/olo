use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
use std::{path::Path, sync::Arc};
#[path = "support/paint_host.rs"]
mod paint_host;
fn load(body: &str, scale: f64) -> (page::Page, paint::Engine) {
    let mut fonts = Builder::new(Default::default());
    fonts.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(fonts.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = paint_host::Scaled::new(scope.clone(), scale);
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let source =
        Arc::from(format!("<!doctype html><style>html,body{{margin:0;padding:0}}svg{{display:block}}body{{width:128px;font-size:0;line-height:0;font-family:\"Olo Inline plain\"}}</style>{body}"));
    let page = page::load(source, &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 128., height: 80., scale }, None, |_, _| {}).unwrap();
    (page, painter)
}
#[test]
fn svg_paths_reach_real_page_geometry_paint_and_document_identity() {
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let (page, mut painter) = load("<svg aria-label='Mark' width=40 height=20 viewBox='0 0 20 10' fill=red><path d='M0 0H20V10H0Z' /></svg>", scale);
        assert_eq!(page.stylesheet_issues, 0, "{:?}", page.scene.layout().cache().tree().styles().report());
        let (index, command) = page.scene.commands().iter().enumerate().find(|(_, c)| matches!(c.primitive, paint::Primitive::Vector { .. })).unwrap();
        assert_eq!(command.bounds, layout::Rect { x: 0., y: 0., width: 40., height: 20. });
        let node = page.scene.node(index as u32).unwrap();
        assert_eq!(page.document.attribute(node, "aria-label").unwrap(), Some("Mark"));
        let tile = painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: 100, height: 100 }, None).unwrap();
        let pixel = |x: usize, y: usize| &tile.rgba()[(y * 100 + x) * 4..][..4];
        assert_eq!(pixel(5, 5), [255, 0, 0, 255]);
        assert_eq!(pixel((45. * scale) as usize, 5), [255; 4]);
    }
}
#[test]
fn svg_geometry_hints_obey_css_and_clip_meet_slice_and_zero_viewboxes() {
    let (page, mut painter) = load("<svg width=10 height=10 style='width:40px;height:20px' viewBox='0 0 10 10' fill=blue><path d='M0 0H10V10H0Z' /></svg>", 1.);
    let tile = painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: 100, height: 100 }, None).unwrap();
    let pixel = |x: usize, y: usize| &tile.rgba()[(y * 100 + x) * 4..][..4];
    assert_eq!(pixel(5, 5), [255; 4]);
    assert_eq!(pixel(15, 5), [0, 0, 255, 255]);
    assert_eq!(pixel(35, 5), [255; 4]);
    let (page, mut painter) = load("<svg width=40 height=20 viewBox='0 0 0 10' fill=blue><path d='M0 0H10V10H0Z' /></svg>", 1.);
    assert!(painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: 100, height: 100 }, None).unwrap().rgba().iter().all(|v| *v == 255));
}

#[test]
fn independent_svg_page_geometry_and_pixels() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/svg-page-fixtures");
    let cases: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("cases.json")).unwrap()).unwrap();
    let reference: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("chrome.json")).unwrap()).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join("cases.json")).unwrap())), reference["cases_sha256"]);
    let mut failures = Vec::new();
    let mut exact = 0;
    for row in reference["rows"].as_array().unwrap() {
        let case = cases.as_array().unwrap().iter().find(|c| c["id"] == row["id"]).unwrap();
        let scale = row["scale"].as_f64().unwrap();
        let (page, mut painter) = load(&format!("<style>{}</style>{}", case["css"].as_str().unwrap(), case["html"].as_str().unwrap()), scale);
        assert_eq!(page.stylesheet_issues, 0);
        let command = page.scene.commands().iter().find(|c| matches!(c.primitive, paint::Primitive::Vector { .. })).unwrap();
        let b = command.bounds;
        let expected = &row["bounds"];
        let geometry = [("x", b.x), ("y", b.y), ("width", b.width), ("height", b.height)].into_iter().map(|(k, n)| (n - expected[k].as_f64().unwrap()).abs()).fold(0., f64::max);
        let bytes = std::fs::read(root.join(row["png"].as_str().unwrap())).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), row["sha256"]);
        let reference = tiny_skia::Pixmap::decode_png(&bytes).unwrap();
        assert_eq!((reference.width(), reference.height()), ((128. * scale) as u32, (80. * scale) as u32));
        let actual = painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: reference.width(), height: reference.height() }, None).unwrap();
        let different = actual.rgba().as_chunks::<4>().0.iter().zip(reference.data().as_chunks::<4>().0).filter(|(a, b)| a != b).count();
        let maximum = actual.rgba().iter().zip(reference.data()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        if geometry > 0.0001 || different != 0 {
            failures.push(serde_json::json!({"id":row["id"],"scale":scale,"geometry_error":geometry,"different_pixels":different,"max_channel_error":maximum}));
        } else {
            exact += 1;
        }
        if let Ok(dir) = std::env::var("OLO_SVG_REPORT") {
            assert!(dir.starts_with("/var/tmp/"));
            actual.save_png(Path::new(&dir).join(row["png"].as_str().unwrap())).unwrap();
        }
    }
    let report = serde_json::json!({"exact":exact,"total":reference["rows"].as_array().unwrap().len(),"nonpasses":failures});
    if let Ok(dir) = std::env::var("OLO_SVG_REPORT") {
        std::fs::write(Path::new(&dir).join("report.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let known: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("REFERENCE-GAPS.json")).unwrap()).unwrap();
    assert_eq!(report, known, "Independent pixel nonpasses changed; inspect and update evidence, never silently accept a website.");
    if std::env::var_os("OLO_SVG_REQUIRE_COMPLETE").is_some() {
        assert!(failures.is_empty(), "{report:#}");
    }
}

#[test]
fn svg_worker_quota_cancellation_and_stale_document_are_explicit() {
    use std::sync::atomic::AtomicBool;
    let (mut page, _) = load("<svg width=40 height=20 viewBox='0 0 10 10'><path d='M0 0H10V10H0Z' /></svg>", 1.);
    let tree = page.scene.layout().cache().tree();
    let root = tree.iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::BlockVector { .. })).unwrap().1.node;
    let compile = |nodes, work, cancel| olo_web::svg::Viewport::load(&page.document, tree.styles(), root, nodes, work, cancel);
    assert!(matches!(compile(0, 100, None), Err(layout::Error::Limit)));
    assert!(matches!(compile(100, 0, None), Err(layout::Error::Limit)));
    assert!(matches!(compile(100, 100, Some(&AtomicBool::new(true))), Err(layout::Error::Cancelled)));
    let mut job = layout::BuildJob::new(&page.document, tree.styles().clone(), layout::BuildLimits { vector_bytes: 0, ..Default::default() }, None).unwrap();
    loop {
        match job.step(1) {
            Ok(p) => assert!(!p.finished),
            Err(layout::Error::Limit) => break,
            Err(e) => panic!("{e:?}"),
        }
    }
    assert!(matches!(job.step(1), Err(layout::Error::Failed)));
    drop(job);
    page.document.set_attribute(root, "width", "80").unwrap();
    assert!(olo_web::svg::Viewport::load(&page.document, tree.styles(), root, 100, 100, None).is_err());
}

#[test]
fn visible_svg_overflow_survives_spatial_query_and_tile_origin() {
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let (page, mut painter) = load("<svg width=20 height=20 style='overflow:visible'><path d='M160 0H180V20H160Z' fill=red /></svg>", scale);
        let region = paint::DeviceRect { x: (160. * scale) as i32, y: 0, width: (20. * scale) as u32, height: (20. * scale) as u32 };
        let tile = painter.raster(&page.scene, region, None).unwrap();
        assert!(tile.rgba().as_chunks::<4>().0.iter().all(|p| *p == [255, 0, 0, 255]));
        assert!(page.scene.content_extent().width >= 180.);
    }
}

#[test]
fn independent_svg_overflow_geometry_and_pixels() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/svg-overflow-fixtures");
    let cases: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("cases.json")).unwrap()).unwrap();
    let reference: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("chrome.json")).unwrap()).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join("cases.json")).unwrap())), reference["cases_sha256"]);
    let mut failures = Vec::new();
    for row in reference["rows"].as_array().unwrap() {
        let case = cases.as_array().unwrap().iter().find(|c| c["id"] == row["id"]).unwrap();
        let scale = row["scale"].as_f64().unwrap();
        let (page, mut painter) = load(&format!("<style>{}</style>{}", case["css"].as_str().unwrap(), case["html"].as_str().unwrap()), scale);
        assert_eq!(page.stylesheet_issues, 0);
        let tree = page.scene.layout().cache().tree();
        let (id, _) = tree.iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::BlockVector { .. } | layout::BoxKind::InlineVector { .. })).unwrap();
        let g = page.scene.layout().geometry(id).unwrap();
        // Reconstruct the CSS border box independently from the enlarged ink bounds.
        let mut b = g.border_box;
        let mut parent = tree.get(id).unwrap().parent;
        while let Some(id) = parent {
            let p = page.scene.layout().geometry(id).unwrap().content_box();
            b.x += p.x;
            b.y += p.y;
            parent = tree.get(id).unwrap().parent;
        }
        let expected = &row["bounds"];
        let geometry = [("x", b.x), ("y", b.y), ("width", b.width), ("height", b.height)].into_iter().map(|(k, n)| (n - expected[k].as_f64().unwrap()).abs()).fold(0., f64::max);
        let bytes = std::fs::read(root.join(row["png"].as_str().unwrap())).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), row["sha256"]);
        let reference = tiny_skia::Pixmap::decode_png(&bytes).unwrap();
        assert_eq!((reference.width(), reference.height()), ((128. * scale) as u32, (80. * scale) as u32));
        let actual = painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: reference.width(), height: reference.height() }, None).unwrap();
        let different = actual.rgba().as_chunks::<4>().0.iter().zip(reference.data().as_chunks::<4>().0).filter(|(a, b)| a != b).count();
        let maximum = actual.rgba().iter().zip(reference.data()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        if geometry > 0.0001 || different != 0 {
            failures.push(serde_json::json!({"id":row["id"],"scale":scale,"geometry_error":geometry,"different_pixels":different,"max_channel_error":maximum}));
        }
        if let Ok(dir) = std::env::var("OLO_SVG_OVERFLOW_REPORT") {
            assert!(dir.starts_with("/var/tmp/"));
            actual.save_png(Path::new(&dir).join(row["png"].as_str().unwrap())).unwrap();
        }
    }
    let report = serde_json::json!({"total":reference["rows"].as_array().unwrap().len(),"nonpasses":failures});
    if let Ok(dir) = std::env::var("OLO_SVG_OVERFLOW_REPORT") {
        std::fs::write(Path::new(&dir).join("report.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let known: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("REFERENCE-GAPS.json")).unwrap()).unwrap();
    assert_eq!(report, known, "Independent overflow pixel nonpasses changed; inspect evidence. These are not site passes.");
    if std::env::var_os("OLO_SVG_REQUIRE_COMPLETE").is_some() {
        assert!(failures.is_empty(), "{report:#}");
    }
}
