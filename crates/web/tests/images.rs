use olo_web::{
    font::{Builder, FontSet, GenericPolicy, Scope},
    html,
    image::{Image, Limits, Set},
    layout, page, paint,
    text::shaping::Engine,
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
#[path = "support/paint_host.rs"]
mod paint_host;
fn scope() -> Scope {
    let mut builder = Builder::new(Default::default());
    builder.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    FontSet::new(builder.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
}
#[test]
fn independent_native_chrome_image_pixels_and_explicit_nonpasses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/image-paint-fixtures");
    let inputs: serde_json::Value = serde_json::from_str(include_str!("image-paint-fixtures/inputs.json")).unwrap();
    let raw: serde_json::Value = serde_json::from_str(include_str!("image-paint-fixtures/assets.json")).unwrap();
    let nonpasses: serde_json::Value = serde_json::from_str(include_str!("image-paint-fixtures/nonpasses.json")).unwrap();
    let assets: std::collections::HashMap<_, _> = raw
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
    let scope = scope();
    let mut shaper = Engine::default();
    let mut painter = paint::Engine::default();
    let (mut exact, mut known) = (0, 0);
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let host = paint_host::Scaled::new(scope.clone(), scale);
        for input in inputs.as_array().unwrap() {
            let name = format!("{}-{scale}.png", input["id"].as_str().unwrap());
            let source = Arc::from(format!("<!doctype html><style>{}{}</style>{}", include_str!("image-paint-fixtures/base.css"), input["css"].as_str().unwrap(), input["body"].as_str().unwrap()));
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
            assert_eq!(page.stylesheet_issues, 0, "{name}");
            let size = (100. * scale) as u32;
            let tile = painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: size, height: size }, None).unwrap();
            let reference = tiny_skia::Pixmap::load_png(root.join(&name)).unwrap();
            assert_eq!((reference.width(), reference.height()), (size, size));
            let differences: Vec<_> = tile.rgba().iter().zip(reference.data()).enumerate().filter(|(_, (a, b))| a != b).map(|(i, (a, b))| (i, a.abs_diff(*b))).collect();
            if let Some(known_case) = nonpasses.as_array().unwrap().iter().find(|r| r["file"].as_str() == Some(&name)) {
                // These are recorded failures, not accepted pixels. Pin their
                // current extent so widening failures cannot silently pass.
                assert_eq!(differences.len(), known_case["channels"].as_u64().unwrap() as usize, "{name}: known nonpass changed");
                assert_eq!(differences.iter().map(|(_, d)| *d).max().unwrap_or(0), known_case["max_error"].as_u64().unwrap() as u8, "{name}");
                known += 1;
            } else {
                assert!(differences.is_empty(), "{name}: {} differing channels; first {:?}", differences.len(), differences.iter().take(8).collect::<Vec<_>>());
                exact += 1;
            }
        }
    }
    assert_eq!((exact, known), (53, 7));
}
#[test]
fn asset_premultiplication_and_shared_binding_budgets() {
    let mut pixels = Vec::new();
    for alpha in 0..=255u8 {
        for red in 0..=255u8 {
            pixels.extend([red, red / 2, 255 - red, alpha]);
        }
    }
    let image = Image::from_rgba(256, 256, pixels.clone(), Default::default(), None).unwrap();
    for (before, after) in pixels.as_chunks::<4>().0.iter().zip(image.rgba().as_chunks::<4>().0) {
        for c in 0..3 {
            assert_eq!(after[c], (f64::from(before[c]) * f64::from(before[3]) / 255.).round() as u8);
        }
        assert_eq!(after[3], before[3]);
    }
    assert!(Image::from_rgba(0, 1, vec![], Default::default(), None).is_err());
    assert!(Image::from_rgba(1, 1, vec![0; 3], Default::default(), None).is_err());
    assert!(Image::from_rgba(1, 1, vec![0; 4], Limits { pixels: 0, ..Default::default() }, None).is_err());
    let cancelled = AtomicBool::new(true);
    assert!(Image::from_rgba(1, 1, vec![0; 4], Default::default(), Some(&cancelled)).is_err());
    let doc = html::parse(Arc::from("<img id=a><img id=b>"), Default::default(), None).unwrap().document;
    let mut ids = doc.children(doc.root()).unwrap().collect::<Vec<_>>();
    // Connected parser output: find the two actual img handles iteratively.
    let mut images = Vec::new();
    while let Some(node) = ids.pop() {
        if doc.name(node).unwrap() == Some("img") {
            images.push(node);
        }
        ids.extend(doc.children(node).unwrap());
    }
    let pixel = Image::from_rgba(1, 1, vec![255; 4], Default::default(), None).unwrap();
    let budget = Limits { bytes: 4, pixels: 1, ..Default::default() };
    let set = Set::new(&doc, images.iter().map(|n| (*n, pixel.clone())).collect(), budget, None).unwrap();
    set.validate(&doc).unwrap();
    assert!(Arc::ptr_eq(set.get(images[0]).unwrap(), set.get(images[1]).unwrap()));
    assert!(Set::new(&doc, vec![(images[0], pixel.clone()), (images[0], pixel.clone())], Default::default(), None).is_err());
    let distinct = Image::from_rgba(1, 1, vec![255; 4], Default::default(), None).unwrap();
    // Past the budget: the latest bound kept, the earliest let go (the
    // page goes on without it).
    let over = Set::new(&doc, vec![(images[0], pixel), (images[1], distinct.clone())], budget, None).unwrap();
    assert!(over.get(images[0]).is_none());
    assert!(Arc::ptr_eq(over.get(images[1]).unwrap(), &distinct));
    let other = html::parse(Arc::from("<img>"), Default::default(), None).unwrap().document;
    assert!(set.validate(&other).is_err());
}
#[test]
fn images_past_the_page_binding_limit_are_left_out_not_the_page() {
    let scope = scope();
    let host = paint_host::Scaled::new(scope.clone(), 1.);
    let mut shaper = Engine::default();
    let mut painter = paint::Engine::default();
    let limit = page::PAGE_IMAGE_LIMITS.bindings;
    let source = format!("<!doctype html>{}", "<img src=large>".repeat(limit + 3));
    let mut calls = 0;
    let page = page::load_assets(
        source.into(),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
        |_, _, _| {
            calls += 1;
            Ok(None)
        },
    )
    .unwrap();
    // The images past the limit are not even fetched; each is an issue.
    assert_eq!(calls, limit);
    assert_eq!(page.stylesheet_issues, limit + 3);
}
#[test]
fn full_pipeline_image_boxes_match_independent_chrome_objects() {
    let scope = scope();
    let mut shaper = Engine::default();
    let mut painter = paint::Engine::default();
    let pixels = Image::from_rgba(1, 1, vec![255, 0, 0, 255], Default::default(), None).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/object-fixtures");
    let mut failures = Vec::new();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let report: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(format!("chrome-{scale}.json"))).unwrap()).unwrap();
        let host = paint_host::Scaled::new(scope.clone(), scale);
        for case in report["rows"].as_array().unwrap() {
            let source = Arc::from(format!(
                "<!doctype html><html id=root><head><style>{}{}</style></head><body id=body>{}</body></html>",
                report["base"].as_str().unwrap(),
                case["css"].as_str().unwrap(),
                case["body"].as_str().unwrap()
            ));
            let page = page::load_assets(
                source,
                &host,
                &scope,
                &mut shaper,
                &mut painter,
                layout::Viewport { width: (case["layoutViewport"]["width"].as_f64().unwrap() * scale).round() / scale, height: 800., scale },
                None,
                |_, _| {},
                |kind, _, _| match kind {
                    page::ResourceKind::Image => Ok(Some(page::Resource::Image(pixels.clone()))),
                    _ => Ok(None),
                },
            )
            .unwrap();
            for (id, b) in page.scene.layout().cache().tree().iter() {
                if page.document.node_type(b.node).unwrap() != olo_web::dom::NodeType::Element {
                    continue;
                }
                let Some(name) = page.document.attribute(b.node, "id").unwrap() else { continue };
                if name != "p" && !name.starts_with('o') {
                    continue;
                }
                if name == "p" && !matches!(b.kind, layout::BoxKind::Block { .. }) {
                    continue;
                }
                let expected = &case["boxes"][name];
                let bounds = page.scene.layout().bounds(id).unwrap();
                for (key, value) in [("x", bounds.x), ("y", bounds.y), ("width", bounds.width), ("height", bounds.height)] {
                    let value = f64::from((value * scale) as f32 * (1f32 / scale as f32));
                    let expected = expected[key].as_f64().unwrap();
                    if (value - expected).abs() > 1e-5 {
                        failures.push(format!("{scale}:{}:{name}.{key} {value} !={expected}", case["name"].as_str().unwrap()));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn native_image_alpha_tiles_and_restyle_keep_the_same_asset() {
    let scope = scope();
    let mut shaper = Engine::default();
    let mut painter = paint::Engine::default();
    let pixels = Image::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 128], Default::default(), None).unwrap();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let host = paint_host::Scaled::new(scope.clone(), scale);
        let mut calls = 0;
        let mut page = page::load_assets(
            Arc::from("<!doctype html><style>body{margin:0}img{display:block;width:50%;height:7px;margin-left:1px;border-width:1px;border-style:solid;border-color:green;padding:1px}</style><img src=test alt=Colours>"),
            &host,
            &scope,
            &mut shaper,
            &mut painter,
            layout::Viewport { width: 32., height: 24., scale },
            None,
            |_, _| {},
            |kind, _, _| {
                assert_eq!(kind, page::ResourceKind::Image);
                calls += 1;
                Ok(Some(page::Resource::Image(pixels.clone())))
            },
        )
        .unwrap();
        assert_eq!(page.stylesheet_issues, 0);
        assert_eq!(calls, 1);
        let command = page.scene.commands().iter().position(|c| matches!(c.primitive, paint::Primitive::Image { .. })).unwrap();
        let node = page.scene.node(command as u32).unwrap();
        assert_eq!(page.document.attribute(node, "alt").unwrap(), Some("Colours"));
        let region = paint::DeviceRect { x: 0, y: 0, width: 64, height: 48 };
        let whole = painter.raster(&page.scene, region, None).unwrap();
        let mut tiles = vec![0; 64 * 48 * 4];
        for y in (0..48).step_by(8) {
            for x in (0..64).step_by(8) {
                let tile = painter.raster(&page.scene, paint::DeviceRect { x, y, width: 8, height: 8 }, None).unwrap();
                for row in 0..8 {
                    let from = row * 8 * 4;
                    let to = ((y as usize + row) * 64 + x as usize) * 4;
                    tiles[to..to + 32].copy_from_slice(&tile.rgba()[from..from + 32]);
                }
            }
        }
        let differences: Vec<_> = tiles.iter().zip(whole.rgba()).enumerate().filter(|(_, (a, b))| a != b).map(|(i, (a, b))| (i / 4 % 64, i / 4 / 64, i % 4, *a, *b)).collect();
        assert!(differences.is_empty(), "tile seam at scale {scale}: {} differing channels; first {:?}", differences.len(), differences.iter().take(16).collect::<Vec<_>>());
        page.restyle(&host, &mut shaper, &mut painter, layout::Viewport { width: 40., height: 24., scale }, None, |_, _| {}).unwrap();
        let set = page.scene.layout().cache().tree().images().unwrap();
        assert!(Arc::ptr_eq(set.get(node).unwrap(), &pixels));
        assert_eq!(calls, 1);
    }
}

#[test]
fn minified_alpha_tiles_are_seamless_cached_and_reclaimed() {
    let scope = scope();
    let mut shaper = Engine::default();
    let mut painter = paint::Engine::default();
    let pixels = Image::from_rgba(63, 35, (0..63 * 35).flat_map(|i| [(i * 17) as u8, (i * 61) as u8, (i * 11) as u8, (i * 29) as u8]).collect(), Default::default(), None).unwrap();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let host = paint_host::Scaled::new(scope.clone(), scale);
        let mut calls = 0;
        let mut page=page::load_assets(Arc::from("<!doctype html><style>body{margin:0}div{width:18px;height:13px;overflow:hidden}img{display:block;width:15.5px;height:10.25px;margin-left:2.125px;padding:1px;border-width:1px;border-style:solid;border-color:green}</style><div><img src=source></div>"),&host,&scope,&mut shaper,&mut painter,layout::Viewport{width:32.,height:24.,scale},None,|_,_|{},|_,_,_|{calls+=1;Ok(Some(page::Resource::Image(pixels.clone())))}).unwrap();
        assert_eq!(page.stylesheet_issues, 0);
        let region = paint::DeviceRect { x: 0, y: 0, width: 64, height: 48 };
        let whole = painter.raster(&page.scene, region, None).unwrap();
        let generated = painter.image_cache_stats().generated_pixels;
        let mut tiled = vec![0; 64 * 48 * 4];
        for y in (0..48).step_by(8) {
            for x in (0..64).step_by(8) {
                let tile = painter.raster(&page.scene, paint::DeviceRect { x, y, width: 8, height: 8 }, None).unwrap();
                for row in 0..8 {
                    let at = ((y as usize + row) * 64 + x as usize) * 4;
                    tiled[at..at + 32].copy_from_slice(&tile.rgba()[row * 32..row * 32 + 32]);
                }
            }
        }
        assert_eq!(tiled.iter().zip(whole.rgba()).filter(|(a, b)| a != b).count(), 0, "minification seam at {scale}");
        assert_eq!(painter.image_cache_stats().generated_pixels, generated);
        assert!(painter.image_cache_stats().bytes <= paint::Limits::default().image_mip_bytes);
        page.restyle(&host, &mut shaper, &mut painter, layout::Viewport { width: 40., height: 24., scale }, None, |_, _| {}).unwrap();
        painter.raster(&page.scene, region, None).unwrap();
        assert_eq!(painter.image_cache_stats().generated_pixels, generated);
        assert_eq!(calls, 1);
    }
    assert!(painter.image_cache_stats().hits > 0);
    drop(pixels);
    let host = paint_host::Scaled::new(scope.clone(), 1.);
    let plain =
        page::load(Arc::from("<!doctype html><p>New document</p>"), &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {}).unwrap();
    painter.raster(&plain.scene, paint::DeviceRect { x: 0, y: 0, width: 32, height: 32 }, None).unwrap();
    assert_eq!(painter.image_cache_stats().bytes, 0);
}

#[test]
fn dimensions_are_cascade_hints_and_one_to_one_pixels_composite_exactly() {
    let scope = scope();
    let host = paint_host::Scaled::new(scope.clone(), 1.);
    let mut shaper = Engine::default();
    let mut painter = paint::Engine::default();
    let image = Image::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 128], Default::default(), None).unwrap();
    for (attributes, css, width, height) in [
        ("width=20", "", 20., 10.),
        ("height=7", "", 14., 7.),
        ("width=20 height=7", "", 20., 7.),
        ("width=20", "img{width:auto}", 2., 1.),
        ("width='23.5px'", "", 23.5, 11.75),
        ("width='50%'", "", 50., 25.),
        ("width='+20'", "", 2., 1.),
    ] {
        let source = Arc::from(format!("<!doctype html><style>body{{margin:0;width:100px}}img{{display:block}}{css}</style><img src=test {attributes}>"));
        let page = page::load_assets(
            source,
            &host,
            &scope,
            &mut shaper,
            &mut painter,
            layout::Viewport { width: 100., height: 100., scale: 1. },
            None,
            |_, _| {},
            |_, _, _| Ok(Some(page::Resource::Image(image.clone()))),
        )
        .unwrap();
        let c = page.scene.commands().iter().find(|c| matches!(c.primitive, paint::Primitive::Image { .. })).unwrap();
        assert_eq!((c.bounds.width, c.bounds.height), (width, height), "{attributes}/{css}");
    }
    let page = page::load_assets(
        Arc::from("<!doctype html><style>body{margin:0}img{display:block}</style><img src=test>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 4., height: 2., scale: 1. },
        None,
        |_, _| {},
        |_, _, _| Ok(Some(page::Resource::Image(image.clone()))),
    )
    .unwrap();
    let raster = painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: 4, height: 2 }, None).unwrap();
    assert_eq!(&raster.rgba()[..12], &[255, 0, 0, 255, 127, 127, 255, 255, 255, 255, 255, 255]);
}

#[test]
fn independent_chrome_intrinsic_constraints_display_and_dimension_hints() {
    let scope = scope();
    let mut shaper = Engine::default();
    let mut painter = paint::Engine::default();
    let pixels = Image::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 128], Default::default(), None).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/image-fixtures");
    let mut failures = Vec::new();
    let mut supported = 0;
    let mut unsupported = 0;
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let report: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(format!("chrome-{scale}.json"))).unwrap()).unwrap();
        let host = paint_host::Scaled::new(scope.clone(), scale);
        for case in report["rows"].as_array().unwrap() {
            let source = Arc::from(format!(
                "<!doctype html><html id=root><head><style>{}{}</style></head><body id=body>{}</body></html>",
                report["base"].as_str().unwrap(),
                case["css"].as_str().unwrap(),
                case["body"].as_str().unwrap()
            ));
            let page = page::load_assets(
                source,
                &host,
                &scope,
                &mut shaper,
                &mut painter,
                layout::Viewport { width: (case["layoutViewport"]["width"].as_f64().unwrap() * scale).round() / scale, height: 800., scale },
                None,
                |_, _| {},
                |kind, _, _| match kind {
                    page::ResourceKind::Image => Ok(Some(page::Resource::Image(pixels.clone()))),
                    _ => Ok(None),
                },
            );
            if let Some(reason) = case["unsupported"].as_str() {
                // Alignment the engine does not lay out yet: the image still
                // renders at the baseline, and the page records the substitute.
                let page = page.unwrap_or_else(|error| panic!("{} at{scale}: {error}", case["name"]));
                assert!(page.fallbacks.counts().iter().any(|(what, _)| *what == reason), "{}: {}", case["name"], page.fallbacks);
                unsupported += 1;
                continue;
            }
            let page = page.unwrap_or_else(|error| panic!("{} at{scale}: {error}", case["name"]));
            assert_eq!(page.stylesheet_issues, 0, "{}", case["name"]);
            for (id, b) in page.scene.layout().cache().tree().iter() {
                if page.document.node_type(b.node).unwrap() != olo_web::dom::NodeType::Element {
                    continue;
                }
                let Some(name) = page.document.attribute(b.node, "id").unwrap() else { continue };
                if name != "image" && !(name == "p" && matches!(b.kind, layout::BoxKind::Block { .. })) {
                    continue;
                }
                let bounds = page.scene.layout().bounds(id).unwrap();
                for (key, value) in [("x", bounds.x), ("y", bounds.y), ("width", bounds.width), ("height", bounds.height)] {
                    let value = f64::from((value * scale) as f32 * (1f32 / scale as f32));
                    let expected = case["boxes"][name][key].as_f64().unwrap();
                    if (value - expected).abs() > 1e-5 {
                        failures.push(format!("{scale}:{} {name}.{key}: {value} !={expected}", case["name"].as_str().unwrap()));
                    }
                }
            }
            supported += 1;
        }
    }
    assert_eq!((supported, unsupported), (300, 0));
    assert!(failures.is_empty(), "{} geometry differences:\n{}", failures.len(), failures.join("\n"));
}
#[test]
fn a_new_frame_of_the_same_size_replaces_its_picture_in_place() {
    let doc = html::parse(Arc::from("<canvas id=a></canvas><video id=b></video>"), Default::default(), None).unwrap().document;
    let mut stack = vec![doc.root()];
    let mut nodes = Vec::new();
    while let Some(n) = stack.pop() {
        if matches!(doc.name(n).unwrap(), Some("canvas" | "video")) {
            nodes.push(n);
        }
        stack.extend(doc.children(n).unwrap());
    }
    nodes.sort_by_key(|n| n.slot());
    let frame = |v: u8, w: u32| Image::from_rgba(w, 2, vec![v; (w * 2 * 4) as usize], Default::default(), None).unwrap();
    let set = Set::new(&doc, vec![(nodes[0], frame(1, 2)), (nodes[1], frame(2, 4))], Default::default(), None).unwrap();
    let next = frame(9, 4);
    let swapped = set.with_replaced(&[(nodes[1], next.clone())]).unwrap();
    // Every index as before; the new picture where the old one was.
    assert_eq!(swapped.index(nodes[1]), set.index(nodes[1]));
    assert!(Arc::ptr_eq(swapped.get(nodes[1]).unwrap(), &next));
    assert!(set.same_layout(&swapped));
    // Another size is no swap, and no layout of the old set serves it.
    assert!(set.with_replaced(&[(nodes[1], frame(9, 8))]).is_none());
    let resized = Set::new(&doc, vec![(nodes[0], frame(1, 2)), (nodes[1], frame(2, 8))], Default::default(), None).unwrap();
    assert!(!set.same_layout(&resized));
}
#[test]
fn a_deferred_image_makes_its_pixels_once_and_only_when_asked() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let made = Arc::new(AtomicUsize::new(0));
    let counter = made.clone();
    let source: Arc<dyn std::any::Any + Send + Sync> = Arc::new(7u32);
    let image = olo_web::image::Image::deferred(2, 1, 2., source, move || {
        counter.fetch_add(1, Ordering::Relaxed);
        vec![1, 2, 3, 255]
    });
    assert_eq!(made.load(Ordering::Relaxed), 0, "nothing made until asked");
    assert_eq!(image.natural(), (1., 0.5));
    assert_eq!(image.source().and_then(|s| s.downcast_ref::<u32>()), Some(&7));
    // Short of its size: filled out opaque.
    assert_eq!(image.rgba(), [1, 2, 3, 255, 255, 255, 255, 255]);
    assert_eq!(image.rgba().len(), 8);
    assert_eq!(made.load(Ordering::Relaxed), 1, "made once");
}

#[test]
fn images_in_implicit_grid_tracks_match_chrome_intrinsic_widths() {
    use sha2::{Digest, Sha256};
    let cases: serde_json::Value = serde_json::from_str(include_str!("replaced-intrinsic-fixtures/cases.json")).unwrap();
    let reference: serde_json::Value = serde_json::from_str(include_str!("replaced-intrinsic-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(include_bytes!("replaced-intrinsic-fixtures/cases.json"))), reference["cases_sha256"]);
    let scope = scope();
    let mut shaper = Engine::default();
    let mut painter = paint::Engine::default();
    let pixels = Image::from_rgba(2, 1, vec![255, 0, 0, 255, 255, 0, 0, 255], Default::default(), None).unwrap();
    let mut failures = Vec::new();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let host = paint_host::Scaled::new(scope.clone(), scale);
        for case in cases.as_array().unwrap() {
            let expected = reference["rows"].as_array().unwrap().iter().find(|row| row["id"] == case["id"] && row["scale"].as_f64() == Some(scale)).unwrap();
            let source = format!("<!doctype html><style>{}{}</style>{}", include_str!("flow-fixtures/base.css"), case["css"].as_str().unwrap(), case["html"].as_str().unwrap());
            let page = page::load_assets(
                Arc::from(source),
                &host,
                &scope,
                &mut shaper,
                &mut painter,
                layout::Viewport { width: 800., height: 600., scale },
                None,
                |_, _| {},
                |kind, _, _| Ok(matches!(kind, page::ResourceKind::Image).then(|| page::Resource::Image(pixels.clone()))),
            )
            .unwrap();
            // A source-less image records one missing resource, not a CSS fallback.
            assert_eq!(page.stylesheet_issues, usize::from(case["broken"] == true), "{} at {scale}", case["id"]);
            assert_eq!(page.scene.layout().cache().tree().styles().report().stylesheet_issues, 0);
            let l = page.scene.layout();
            for name in case["measure"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()) {
                let (id, _) =
                    l.cache().tree().iter().find(|(_, b)| !matches!(b.kind, layout::BoxKind::InlineContent { .. }) && page.document.attribute(b.node, "id").ok().flatten() == Some(name)).unwrap();
                let rect = l.bounds(id).unwrap();
                for (key, value) in [("width", rect.width), ("height", rect.height)] {
                    let want = expected["rects"][name][key].as_f64().unwrap();
                    if (value - want).abs() > 0.02 {
                        failures.push(format!("{} at {scale}: {name}.{key} {value} != {want}", case["id"]));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}
