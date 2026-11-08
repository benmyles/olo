use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, SheetBuilder},
        style::{self, Defaults},
    },
    font::{Builder, FontSet, GenericPolicy},
    html, layout, paint,
    text::shaping::Engine as Shaper,
};
use paint_host::native_host;
use std::{path::Path, sync::Arc};
const BASE: &str = "html,body,div,p{display:block;margin:0;padding:0;border-width:0;border-style:none}head{display:none}html{font-family:'Olo Inline plain';font-size:20px;line-height:20px}";
fn scene(body: &str, css: &str, painter: &mut paint::Engine) -> Arc<paint::Scene> {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let d = html::parse(Arc::from(format!("<!doctype html><html><head></head><body>{body}</body></html>")), Default::default(), None).unwrap().document;
    let syntax = Syntax::parse(Arc::from(format!("{BASE}{css}")), Default::default(), None).unwrap();
    let mut builder = SheetBuilder::new(Default::default());
    builder.add_sheet(&syntax, Origin::Author, None).unwrap();
    let rules = builder.finish(None).unwrap();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let mut j = style::Job::new(&d, &rules, defaults, Default::default(), Default::default(), None).unwrap();
    while !j.step(128, |_| &host).unwrap().finished {}
    let styles = Arc::new(j.finish().unwrap());
    assert_eq!(styles.report().stylesheet_issues, 0);
    let mut j = layout::BuildJob::new(&d, styles, Default::default(), None).unwrap();
    while !j.step(128).unwrap().finished {}
    let tree = j.finish().unwrap();
    let mut e = Shaper::default();
    let mut j = layout::PrepareJob::new(&mut e, &scope, tree, Default::default(), None).unwrap();
    while !j.step(1).unwrap().finished {}
    let cache = j.finish().unwrap();
    let mut j = layout::Job::new(&mut e, &scope, cache, layout::Viewport { width: 100., height: 100., scale: 1. }, Default::default(), None).unwrap();
    while !j.step(128).unwrap().finished {}
    let layout = Arc::new(j.finish().unwrap());
    let mut j = paint::BuildJob::new(&d, layout, painter, Default::default(), Default::default(), None).unwrap();
    while !j.step(1).unwrap().finished {}
    j.finish().unwrap()
}
fn render(p: &mut paint::Engine, s: &paint::Scene) -> paint::Tile {
    p.raster(s, paint::DeviceRect { x: 0, y: 0, width: 100, height: 100 }, None).unwrap()
}
fn pixel(tile: &paint::Tile, x: usize, y: usize) -> [u8; 4] {
    tile.rgba()[(y * tile.region.width as usize + x) * 4..][..4].try_into().unwrap()
}
#[test]
fn paints_background_canvas_borders_and_native_glyph_positions() {
    let mut p = paint::Engine::default();
    let s = scene("<div>A B C</div>", "div{color:green}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 1, 3), [0, 128, 0, 255]);
    assert_eq!(pixel(&t, 9, 3), [255; 4]);
    assert_eq!(pixel(&t, 31, 3), [0, 128, 0, 255]);
    assert_eq!(pixel(&t, 1, 17), [255; 4]);
    let s = scene("<div></div>", "html{background-color:blue}body{background-color:red}div{width:20px;height:20px;border-width:2px;border-style:solid;border-color:green}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 0, 0), [0, 128, 0, 255]);
    assert_eq!(pixel(&t, 4, 4), [255, 0, 0, 255]);
    assert_eq!(pixel(&t, 50, 50), [0, 0, 255, 255]);
    let s = scene("", "body{background-color:red}", &mut p);
    assert_eq!(pixel(&render(&mut p, &s), 50, 50), [255, 0, 0, 255]);
}

#[test]
fn semantic_breaks_move_paint_and_never_expose_carriers_as_text_or_targets() {
    let mut painter = paint::Engine::default();
    let s = scene("<div>A<wbr>B<br>C</div>", "", &mut painter);
    let text: String = (0..s.commands().len()).filter_map(|i| s.text_content(i as u32)).collect();
    assert_eq!(text, "ABC");
    let source = s.layout().cache().tree().text_source(0).unwrap();
    let controls = s.layout().cache().tree().controls(0);
    assert_eq!(controls.len(), 2);
    let nodes: Vec<_> = controls.iter().map(|c| source.span_at(c.offset).unwrap().node).collect();
    for i in 0..s.commands().len() {
        assert!(!s.text_node(i as u32).is_some_and(|n| nodes.contains(&n)));
    }
    let tile = render(&mut painter, &s);
    for (x, y) in [(1, 3), (21, 3), (1, 23)] {
        assert_eq!(pixel(&tile, x, y), [0, 0, 0, 255]);
    }
    let empty = scene("<div><wbr></div>", "", &mut painter);
    assert!(!(0..empty.commands().len()).any(|i| empty.text_content(i as u32).is_some()));
}

#[test]
fn a_shared_ligature_can_be_read_without_an_internal_optional_break_marker() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/ligature.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline ligature"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let source = format!("<!doctype html><style>{BASE}html{{font-family:'Olo Inline ligature'}}</style><div id=p>f<wbr style='color:red'>i</div>");
    let page =
        olo_web::page::load(source.into(), &host, &scope, &mut Shaper::default(), &mut paint::Engine::default(), layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {}).unwrap();
    let labels: String = (0..page.scene.commands().len()).filter_map(|i| page.scene.text_content(i as u32)).collect();
    assert_eq!(labels, "fi");
    let parent =
        page.scene.layout().cache().tree().iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::Block { .. }) && page.document.attribute(b.node, "id").unwrap() == Some("p")).unwrap().1.node;
    let first = page.document.first_child(parent).unwrap().unwrap();
    let wbr = page.document.next_sibling(first).unwrap().unwrap();
    let last = page.document.next_sibling(wbr).unwrap().unwrap();
    assert_eq!(page.document.value(first).unwrap(), Some("f"));
    assert_eq!(page.document.value(wbr).unwrap(), None);
    assert_eq!(page.document.value(last).unwrap(), Some("i"));
    let page = olo_web::page::load(
        format!("<!doctype html><style>{BASE}html{{font-family:'Olo Inline ligature'}}</style><div>f<span style='color:red'>i</span></div>").into(),
        &host,
        &scope,
        &mut Shaper::default(),
        &mut paint::Engine::default(),
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    // The shared ligature paints once, in its first span's colour, and reads back whole.
    let scene = &page.scene;
    let texts: Vec<_> = (0..scene.commands().len() as u32).filter_map(|c| scene.text_content(c).map(|t| t.into_owned())).collect();
    assert_eq!(texts.concat(), "fi");
    assert!(page.fallbacks.is_empty(), "{}", page.fallbacks);
}

#[test]
fn inline_spacing_moves_glyphs_without_block_axis_padding_moving_the_line() {
    let mut p = paint::Engine::default();
    let s = scene("<div>A<span>A</span>A</div>", "span{margin-left:10px;padding-left:5px;padding-right:7px;padding-top:30px;padding-bottom:40px}", &mut p);
    let t = render(&mut p, &s);
    for x in [1, 36, 63] {
        assert_eq!(pixel(&t, x, 3), [0, 0, 0, 255]);
    }
    for x in [20, 30, 45, 60] {
        assert_eq!(pixel(&t, x, 3), [255; 4]);
    }
    assert_eq!(pixel(&t, 36, 40), [255; 4]);
}

#[test]
fn unimplemented_inline_backgrounds_cannot_disappear_on_empty_or_nested_boxes() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    for body in ["<span></span>", "<span><span style='background-color:transparent'>A</span></span>"] {
        let source = format!("<!doctype html><style>{BASE}span{{padding-left:5px;background-color:red}}</style>{body}");
        let page = olo_web::page::load(source.into(), &host, &scope, &mut Shaper::default(), &mut paint::Engine::default(), layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {})
            .unwrap();
        assert_eq!(page.fallbacks.counts(), vec![("inline background not painted", 1)], "{body}");
    }
}
#[test]
fn colors_visibility_clipping_and_positioned_paint_order() {
    let mut p = paint::Engine::default();
    let s = scene("<div>A <span>B</span> C</div>", "span{color:red}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 1, 3), [0, 0, 0, 255]);
    assert_eq!(pixel(&t, 31, 3), [255, 0, 0, 255]);
    let s = scene("<div id=a>A</div><div id=b></div>", "#a{height:20px}#b{height:20px;margin-top:-20px;background-color:blue}", &mut p);
    assert_eq!(pixel(&render(&mut p, &s), 1, 3), [0, 0, 0, 255]);
    let s = scene("<div id=a>A</div><div id=b></div>", "#a{height:20px}#b{position:relative;height:20px;margin-top:-20px;background-color:blue}", &mut p);
    assert_eq!(pixel(&render(&mut p, &s), 1, 3), [0, 0, 255, 255]);
    let s = scene("<div>A B</div>", "div{width:4px;height:10px;overflow:hidden}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 1, 3), [0, 0, 0, 255]);
    assert_eq!(pixel(&t, 5, 3), [255; 4]);
    assert_eq!(pixel(&t, 1, 11), [255; 4]);
}
#[test]
fn glyph_color_reuse_tracks_real_spans_across_breaks_and_hidden_text() {
    let mut p = paint::Engine::default();
    let s = scene("<div><span style='color:red'>AB<br>CD</span><span style='color:blue'>E</span><br><span style='visibility:hidden'>F</span><span style='color:green'>G</span></div>", "", &mut p);
    let tile = render(&mut p, &s);
    for (x, y, expected) in
        [(1, 3, [255, 0, 0, 255]), (21, 3, [255, 0, 0, 255]), (1, 23, [255, 0, 0, 255]), (21, 23, [255, 0, 0, 255]), (41, 23, [0, 0, 255, 255]), (1, 43, [255; 4]), (21, 43, [0, 128, 0, 255])]
    {
        assert_eq!(pixel(&tile, x, y), expected, "{x},{y}");
    }
}
#[test]
fn tiled_output_equals_whole_surface_and_index_matches_brute_force() {
    let mut p = paint::Engine::default();
    let s = scene("<div>A B C</div><p>A B</p>", "p{margin-top:5px;color:red}", &mut p);
    let whole = render(&mut p, &s);
    for y in [0, 50] {
        for x in [0, 50] {
            let tile = p.raster(&s, paint::DeviceRect { x, y, width: 50, height: 50 }, None).unwrap();
            for j in 0..50 {
                for i in 0..50 {
                    assert_eq!(pixel(&whole, x as usize + i, y as usize + j), pixel(&tile, i, j));
                }
            }
        }
    }
    for y in (-20..120).step_by(7) {
        let r = layout::Rect { x: 0., y: f64::from(y), width: 100., height: 10. };
        let mut hits = Vec::new();
        s.query(r, &mut hits, None).unwrap();
        let expected: Vec<_> = s
            .commands()
            .iter()
            .enumerate()
            .filter(|(_, c)| c.bounds.x < r.x + r.width && r.x < c.bounds.x + c.bounds.width && c.bounds.y < r.y + r.height && r.y < c.bounds.y + c.bounds.height)
            .map(|(i, _)| i as u32)
            .collect();
        assert_eq!(hits, expected);
    }
}
#[test]
fn raster_cancellation_limits_and_cache_reuse() {
    let mut p = paint::Engine::default();
    let s = scene("<div>A B C</div>", "", &mut p);
    let before = p.cache_stats();
    render(&mut p, &s);
    let after = p.cache_stats();
    assert_eq!(before.misses, after.misses);
    assert!(after.hits > before.hits);
    let cancelled = std::sync::atomic::AtomicBool::new(true);
    assert!(matches!(p.raster(&s, paint::DeviceRect { x: 0, y: 0, width: 100, height: 100 }, Some(&cancelled)), Err(paint::Error::Cancelled)));
    assert!(matches!(p.raster(&s, paint::DeviceRect { x: 0, y: 0, width: 1000, height: 1000 }, None), Err(paint::Error::Limit)));
    assert!(matches!(p.raster(&s, paint::DeviceRect { x: 0, y: 0, width: 0, height: 10 }, None), Err(paint::Error::Invalid)));
    fn send_sync<T: Send + Sync>() {}
    send_sync::<paint::Scene>();
    send_sync::<paint::Tile>();
}

#[test]
fn complete_static_pipeline_paints_style_elements_and_reports_inert_scripts() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let mut phases = Vec::new();
    let page=olo_web::page::load(Arc::from("<!doctype html><title>Native paint</title><style>html{font-family:'Olo Inline plain';font-size:20px;line-height:20px}body{margin:0}div{background-color:red;height:20px}</style><div>A</div><script>throw Error('must stay inert')</script>"),&host,&scope,&mut shaper,&mut painter,layout::Viewport{width:100.,height:100.,scale:1.},None,|p,_|phases.push(p)).unwrap();
    assert_eq!(page.title, "Native paint");
    assert_eq!(page.scripts, 1);
    assert_eq!(page.stylesheet_issues, 0);
    assert_eq!(pixel(&render(&mut painter, &page.scene), 50, 10), [255, 0, 0, 255]);
    assert!(phases.contains(&olo_web::page::Phase::Paint));
}

#[test]
fn distant_tiles_keep_local_subpixel_precision() {
    let mut p = paint::Engine::default();
    let css = "div{position:relative;top:0.25px;left:0.25px;width:30px;height:15px;border-width:1px;border-style:solid;border-color:red;background-color:blue;overflow:hidden}";
    let near = scene("<div>A B</div>", css, &mut p);
    let far = scene("<div>A B</div>", &css.replace("top:0.25px", "top:16777216.25px"), &mut p);
    let a = render(&mut p, &near);
    let b = p.raster(&far, paint::DeviceRect { x: 0, y: 16777216, width: 100, height: 100 }, None).unwrap();
    assert_eq!(a.rgba(), b.rgba());
}

#[path = "support/paint_host.rs"]
mod paint_host;
fn chrome_pixels(known_gaps: bool) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let inputs: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("tests/paint-fixtures/inputs.json")).unwrap()).unwrap();
    let mut b = Builder::new(Default::default());
    b.load_file(&root.join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let host = paint_host::Scaled::new(scope.clone(), scale);
        for input in inputs.as_array().unwrap() {
            let id = input["id"].as_str().unwrap();
            // Every native observation remains in the full comparator. This
            // exact test's companion explicitly ignores the known failing set.
            let unresolved = match id {
                "fractional" | "asymmetric" | "transparent-side" | "unequal-alpha" => true,
                "multicolor-border" => scale != 1.,
                "glyphs" | "colors" | "border" | "clip" | "normal-order" | "hidden" => scale == 1.25 || scale == 1.75,
                _ => false,
            };
            if unresolved != known_gaps {
                continue;
            }
            let source = Arc::from(format!("<!doctype html><style>{}{}</style>{}", include_str!("paint-fixtures/base.css"), input["css"].as_str().unwrap(), input["body"].as_str().unwrap()));
            let page = olo_web::page::load(source, &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 100., height: 100., scale }, None, |_, _| {}).unwrap();
            assert_eq!(page.stylesheet_issues, 0);
            let actual = painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: (100. * scale) as u32, height: (100. * scale) as u32 }, None).unwrap();
            let expected = tiny_skia::Pixmap::load_png(root.join(format!("tests/paint-fixtures/{id}-{scale}.png"))).unwrap();
            assert_eq!((expected.width(), expected.height()), (actual.region.width, actual.region.height));
            assert_eq!(actual.rgba().len(), expected.data().len());
            let differing = actual.rgba().as_chunks::<4>().0.iter().zip(expected.data().as_chunks::<4>().0).filter(|(a, b)| a != b).count();
            assert_eq!(differing, 0, "{id} at {scale}x differs from independent Chrome pixels");
        }
    }
}
#[test]
fn independent_chrome_native_pixels_at_five_scales() {
    chrome_pixels(false);
}
#[test]
#[ignore = "36 native font-mask/corner/colour-precision observations still fail exactly. See paint-fixtures/README.md."]
fn independent_chrome_native_pixels_not_yet_matching() {
    chrome_pixels(true);
}

#[test]
fn native_pixel_capture_manifest_has_all_inputs_and_immutable_reference_bytes() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let report: serde_json::Value = serde_json::from_str(include_str!("paint-fixtures/chrome.json")).unwrap();
    let inputs: serde_json::Value = serde_json::from_str(include_str!("paint-fixtures/inputs.json")).unwrap();
    let rows = report["rows"].as_array().unwrap();
    assert_eq!(rows.len(), inputs.as_array().unwrap().len() * 5);
    let mut names = std::collections::HashSet::new();
    for row in rows {
        let name = row["file"].as_str().unwrap();
        assert!(names.insert(name));
        assert_eq!(row["metrics"]["dpr"], row["metrics"]["frameDpr"]);
        assert_eq!(row["metrics"]["width"], 100);
        assert_eq!(row["metrics"]["height"], 100);
        let bytes = std::fs::read(root.join("tests/paint-fixtures").join(name)).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(bytes)), row["sha256"].as_str().unwrap());
    }
    for input in inputs.as_array().unwrap() {
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            assert!(names.contains(format!("{}-{scale}.png", input["id"].as_str().unwrap()).as_str()));
        }
    }
    for (path, hash) in report["source_hashes"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap());
    }
}
#[test]
fn translucent_and_asymmetric_border_tiles_have_no_seams_or_double_alpha() {
    let mut painter = paint::Engine::default();
    let s = scene(
        "<div><p></p></div>",
        "div{width:55px;height:45px;overflow:hidden}p{position:relative;left:.25px;top:.25px;width:29px;height:27px;border-style:solid;border-top-width:3px;border-right-width:7px;border-bottom-width:5px;border-left-width:4px;border-top-color:rgb(255 0 0 / 50%);border-right-color:rgb(0 128 0 / 75%);border-bottom-color:blue;border-left-color:rgb(0 0 0 / 25%)}",
        &mut painter,
    );
    let whole = render(&mut painter, &s);
    for y in (0..100).step_by(7) {
        for x in (0..100).step_by(7) {
            let tile = painter.raster(&s, paint::DeviceRect { x, y, width: (100 - x).min(7) as u32, height: (100 - y).min(7) as u32 }, None).unwrap();
            for j in 0..tile.region.height as usize {
                for i in 0..tile.region.width as usize {
                    assert_eq!(pixel(&tile, i, j), pixel(&whole, x as usize + i, y as usize + j), "tile {x},{y} pixel {i},{j}");
                }
            }
        }
    }
}

#[test]
fn linked_sheets_use_first_base_and_document_cascade_order() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let source = Arc::from(
        "<!doctype html><link rel=stylesheet href=first.css><base href='/assets/'><base href='/ignored/'><style>body{margin:0}div{height:20px;background-color:red}</style><link rel=stylesheet href=last.css><link rel=stylesheet href=missing.css><div></div>",
    );
    let mut requests = Vec::new();
    let page = olo_web::page::load_resources(
        source,
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
        |base, href| {
            assert_eq!(base, Some("/assets/"));
            requests.push(href.to_owned());
            let css = match href {
                "first.css" => "div{background-color:blue}",
                "last.css" => "div{background-color:green}",
                "missing.css" => return Ok(None),
                _ => unreachable!(),
            };
            Ok(Some(olo_web::css::Syntax::parse(Arc::from(css), Default::default(), None)?))
        },
    )
    .unwrap();
    assert_eq!(requests, ["first.css", "last.css", "missing.css"]);
    assert_eq!(page.stylesheet_issues, 1);
    assert_eq!(pixel(&render(&mut painter, &page.scene), 50, 10), [0, 128, 0, 255]);
}

#[test]
fn a_sheet_linked_again_counts_once_and_too_much_css_drops_sheets_not_the_page() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    // GitHub links one 0.8 MB sheet four times; React apps repeat links.
    // The repeated sheet loads once and wins at its last place; a sheet
    // past the byte budget is left out and the page still renders.
    let padding = format!("/*{}*/", "x".repeat(3 << 20)); // 3 MB a sheet, of an 8 MB budget
    let source = Arc::from(
        "<!doctype html><style>body{margin:0}div{height:20px}</style><link rel=stylesheet href=big.css><link rel=stylesheet href=blue.css><link rel=stylesheet href=green.css><link rel=stylesheet href=big.css><link rel=stylesheet href=big2.css><link rel=stylesheet href=big3.css><div></div>",
    );
    let mut requests = Vec::new();
    let page = olo_web::page::load_resources(
        source,
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
        |_, href| {
            requests.push(href.to_owned());
            let css = match href {
                "big.css" | "big2.css" => format!("{padding}div{{background-color:red}}"),
                "blue.css" => "div{background-color:blue}".into(),
                "green.css" => "div{background-color:green}".into(),
                "big3.css" => format!("{padding}div{{background-color:yellow}}"),
                _ => unreachable!(),
            };
            Ok(Some(olo_web::css::Syntax::parse(Arc::from(css), Default::default(), None)?))
        },
    )
    .unwrap();
    assert_eq!(requests, ["big.css", "blue.css", "green.css", "big2.css", "big3.css"]);
    // big.css (its last place) and big2.css fit; big3.css is past the budget.
    assert_eq!(page.stylesheet_issues, 1);
    assert_eq!(pixel(&render(&mut painter, &page.scene), 50, 10), [255, 0, 0, 255]);
}

#[test]
fn a_repeated_layered_sheet_keeps_its_first_place_in_the_layer_order() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    // Layer b is named first (by x.css's first copy), then a: a wins, red,
    // as in Chrome. Without the first copy's place, b would come last.
    let source = Arc::from("<!doctype html><style>body{margin:0}div{height:20px}</style><link rel=stylesheet href=x.css><link rel=stylesheet href=y.css><link rel=stylesheet href=x.css><div></div>");
    let mut requests = Vec::new();
    let page = olo_web::page::load_resources(
        source,
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
        |_, href| {
            requests.push(href.to_owned());
            let css = match href {
                "x.css" => "@layer b { div { background-color: green } @layer inner { div { color: red } } }",
                "y.css" => "@layer a { div { background-color: red } }",
                _ => unreachable!(),
            };
            Ok(Some(olo_web::css::Syntax::parse(Arc::from(css), Default::default(), None)?))
        },
    )
    .unwrap();
    assert_eq!(requests, ["x.css", "y.css"]);
    assert_eq!(page.stylesheet_issues, 0);
    assert_eq!(pixel(&render(&mut painter, &page.scene), 50, 10), [255, 0, 0, 255]);
}

#[test]
fn document_input_restyles_link_states_and_keeps_equal_color_owners_separate() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let mut page=olo_web::page::load(Arc::from("<!doctype html><style>body{margin:0}a:link{color:red}a:visited{color:blue}a:hover{color:green}a:active{color:blue}a:focus{color:purple}</style><a id=first href=x>AA</a><a id=second href=y>BB</a>"),&host,&scope,&mut shaper,&mut painter,layout::Viewport{width:100.,height:100.,scale:1.},None,|_,_|{}).unwrap();
    let owned: Vec<_> = page
        .scene
        .commands()
        .iter()
        .enumerate()
        .filter_map(|(i, c)| if let paint::Primitive::Text { color, .. } = c.primitive { Some((page.scene.text_node(i as u32).unwrap(), color)) } else { None })
        .collect();
    assert_eq!(owned.len(), 2);
    assert_ne!(owned[0].0, owned[1].0);
    assert!(owned.iter().all(|(_, c)| *c == [255, 0, 0, 255]));
    let first_text = owned[0].0;
    let first = page.document.parent(first_text).unwrap().unwrap();
    page.interaction.hover = Some(first_text);
    assert!(olo_web::page::Interaction::default().changes_style(page.interaction, page.interaction_mask));
    page.restyle(&host, &mut shaper, &mut painter, layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {}).unwrap();
    assert!(page.scene.commands().iter().any(|c| matches!(c.primitive, paint::Primitive::Text { color: [0, 128, 0, 255], .. })));
    page.interaction.active = Some(first_text);
    page.interaction.focus = Some(first);
    page.restyle(&host, &mut shaper, &mut painter, layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {}).unwrap();
    assert!(page.scene.commands().iter().any(|c| matches!(c.primitive, paint::Primitive::Text { color: [128, 0, 128, 255], .. })));
    assert!(!olo_web::page::Interaction::default().changes_style(page.interaction, 0));
}

#[test]
fn cross_node_ligatures_and_clipped_ink_do_not_invent_click_targets() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/ligature.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline ligature"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let mut page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0;color:red;font-size:20px}a{color:red}</style><a href=x>A </a><a href=y>B</a> C"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    assert!(page.scene.commands().iter().enumerate().any(|(i, c)| matches!(c.primitive, paint::Primitive::Text { .. }) && page.scene.text_node(i as u32).is_none()));
    let foreign = html::parse(Arc::from("<!doctype html><p>other"), Default::default(), None).unwrap().document;
    page.interaction.focus = Some(foreign.root());
    assert!(page.restyle(&host, &mut shaper, &mut painter, layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {}).is_err());
    let s = scene("<div class=clip><p>A</p></div>", ".clip{height:5px;width:10px;overflow:hidden}p{position:relative;top:50px}", &mut painter);
    let text: Vec<_> = s.commands().iter().enumerate().filter(|(_, c)| matches!(c.primitive, paint::Primitive::Text { .. })).collect();
    assert!(!text.is_empty());
    assert!(text.iter().all(|(i, _)| s.visible_bounds(*i as u32).is_none()));
}

#[test]
fn structured_text_matches_linear_cluster_reference_and_preserves_literal_zwsp() {
    let mut painter = paint::Engine::default();
    for css in ["", "div{direction:rtl}"] {
        let s = scene("<div>A<span style='color:red'> B C </span>D\u{200b}E</div>", css, &mut painter);
        let mut values = String::new();
        for (id, command) in s.commands().iter().enumerate() {
            let paint::Primitive::Text { owner, fragment, glyphs, .. } = &command.primitive else { continue };
            let text = s.layout().text_layout(*owner).unwrap();
            let f = &text.fragments()[*fragment as usize];
            let run = text.run(f).unwrap();
            let glyphs = &run.glyphs[glyphs.start as usize..glyphs.end as usize];
            let start = glyphs.iter().map(|g| g.cluster).min().unwrap();
            let last = glyphs.iter().map(|g| g.cluster).max().unwrap();
            let end = run.glyphs.iter().filter(|g| g.cluster > last).map(|g| g.cluster).min().unwrap_or(run.range.end).min(f.source.end);
            let expected = &run.source[start as usize..end as usize];
            let actual = s.text_content(id as u32).unwrap();
            assert!(matches!(actual, std::borrow::Cow::Borrowed(_)));
            assert_eq!(&*actual, expected);
            values.push_str(&actual);
        }
        assert!(values.contains('\u{200b}'), "DOM-authored zero-width spaces must not be filtered");
    }
}

#[test]
fn rounded_corners_gradients_and_shadows_paint_from_style() {
    let mut p = paint::Engine::default();
    // A 40×40 box with 10px radii at (10,10): the corner pixel outside the
    // curve stays white, the centre is red, the straight edge mid-side is red.
    let s = scene("<div id=b></div>", "div{width:40px;height:40px;margin:10px;background:red;border-radius:10px}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 10, 10), [255; 4], "corner outside the curve");
    assert_eq!(pixel(&t, 30, 30), [255, 0, 0, 255]);
    assert_eq!(pixel(&t, 30, 10), [255, 0, 0, 255], "top edge, mid side");
    assert_eq!(pixel(&t, 15, 15), [255, 0, 0, 255], "inside the curve");
    // A horizontal gradient from red to blue: red at the left, blue at the right.
    let s = scene("<div></div>", "div{width:100px;height:20px;background:linear-gradient(to right, red, blue)}", &mut p);
    let t = render(&mut p, &s);
    let left = pixel(&t, 1, 10);
    let right = pixel(&t, 98, 10);
    assert!(left[0] > 240 && left[2] < 15, "{left:?}");
    assert!(right[2] > 240 && right[0] < 15, "{right:?}");
    let middle = pixel(&t, 50, 10);
    assert!(middle[0] > 100 && middle[2] > 100, "{middle:?}");
    // A box shadow below and to the right, blurred: darkens pixels outside the box.
    let s = scene("<div></div>", "div{width:40px;height:40px;margin:10px;background:white;box-shadow:10px 10px 6px rgba(0,0,0,0.8)}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 30, 30), [255; 4], "box interior stays white");
    assert!(pixel(&t, 55, 55)[0] < 120, "shadow below-right: {:?}", pixel(&t, 55, 55));
    assert_eq!(pixel(&t, 5, 5), [255; 4], "no shadow above-left");
    // A rounded uniform border paints as a ring; the corner pixel stays white.
    let s = scene("<div></div>", "div{width:30px;height:30px;margin:10px;border:5px solid blue;border-radius:12px}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 10, 10), [255; 4]);
    assert_eq!(pixel(&t, 25, 12), [0, 0, 255, 255], "top border mid side");
    assert_eq!(pixel(&t, 25, 25), [255; 4], "interior");
}

#[test]
fn opacity_composites_a_box_and_its_content_as_one_layer() {
    let mut p = paint::Engine::default();
    // Half-transparent red over white: pink. Its black text inside the group
    // does not darken the red beneath it more than the group's opacity allows.
    let s = scene("<div id=a>A</div>", "div{width:60px;height:40px;background:red;opacity:0.5;color:black}", &mut p);
    let t = render(&mut p, &s);
    let pink = pixel(&t, 55, 35);
    assert!(pink[0] == 255 && (126..=129).contains(&pink[1]) && (126..=129).contains(&pink[2]), "{pink:?}");
    // Nested groups multiply.
    let s = scene("<div><div id=b></div></div>", "div{width:60px;height:40px;background:red;opacity:0.5}#b{background:blue;opacity:0.5;margin:0}", &mut p);
    let t = render(&mut p, &s);
    let px = pixel(&t, 30, 20);
    // Outer layer: blue at 0.5 over red → (128,0,128); at 0.5 over white → (191,128,191).
    assert!((190..=192).contains(&px[0]) && (126..=129).contains(&px[1]) && (190..=192).contains(&px[2]), "{px:?}");
    assert!(s.commands().len() >= 2);
}

#[test]
fn transforms_move_rotate_and_scale_a_box_with_its_content() {
    let mut p = paint::Engine::default();
    // translate: the box's pixels land 30px right; its old place is white.
    let s = scene("<div></div>", "div{width:20px;height:20px;background:blue;transform:translate(30px,10px)}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 5, 5), [255; 4]);
    assert_eq!(pixel(&t, 40, 20), [0, 0, 255, 255]);
    // Percentages are of the box's own size; translate(-50%,-50%) centres a
    // box placed at (40, 40) on that point.
    let s = scene("<div></div>", "div{width:20px;height:20px;margin:40px;background:blue;transform:translate(-50%,-50%)}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 32, 32), [0, 0, 255, 255]);
    assert_eq!(pixel(&t, 48, 48), [0, 0, 255, 255]);
    assert_eq!(pixel(&t, 52, 52), [255; 4]);
    // scale(2) about the centre (the default origin) doubles the box both ways.
    let s = scene("<div></div>", "div{width:20px;height:20px;margin:40px;background:blue;transform:scale(2)}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 32, 32), [0, 0, 255, 255]);
    assert_eq!(pixel(&t, 68, 68), [0, 0, 255, 255]);
    assert_eq!(pixel(&t, 25, 25), [255; 4]);
    // transform-origin: scaled about its top-left corner, the box grows right and down only.
    let s = scene("<div></div>", "div{width:20px;height:20px;margin:40px;background:blue;transform:scale(2);transform-origin:0 0}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 35, 35), [255; 4]);
    assert_eq!(pixel(&t, 75, 75), [0, 0, 255, 255]);
    // rotate(45deg): the corners leave the box; the centre and a point on the
    // diagonal stay blue, a former corner is white.
    let s = scene("<div></div>", "div{width:40px;height:40px;margin:30px;background:blue;transform:rotate(45deg)}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 50, 50), [0, 0, 255, 255]);
    assert_eq!(pixel(&t, 32, 32), [255; 4], "former corner");
    assert_eq!(pixel(&t, 50, 26), [0, 0, 255, 255], "rotated corner reaches further up");
    // Content follows: text inside a translated box paints at the new place,
    // and a nested transform composes with its parent's.
    let s = scene("<div><p>A</p></div>", "div{width:40px;height:40px;transform:translate(20px,0)}p{color:green;transform:translate(20px,0)}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 1, 3), [255; 4]);
    assert_eq!(pixel(&t, 41, 3), [0, 128, 0, 255]);
    // An overflow clip inside a transformed box moves with it.
    let s = scene("<div><p></p></div>", "div{width:20px;height:20px;margin:40px;overflow:hidden;transform:translate(30px,0)}p{width:60px;height:60px;background:blue}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 75, 45), [0, 0, 255, 255]);
    assert_eq!(pixel(&t, 45, 45), [255; 4], "old place");
    assert_eq!(pixel(&t, 95, 45), [255; 4], "clipped at the moved box's edge");
    // transform: none and an identity list paint in place.
    let s = scene("<div></div>", "div{width:20px;height:20px;background:blue;transform:none}", &mut p);
    assert_eq!(pixel(&render(&mut p, &s), 5, 5), [0, 0, 255, 255]);
}

#[test]
fn text_wrap_balance_evens_the_lines_without_adding_one() {
    let mut p = paint::Engine::default();
    // Five glyph-wide words in a 100px box: greedy wrapping fills the first
    // line; balancing shares the words between the same number of lines.
    let s = scene("<div>AAA BBB CCC DD</div>", "div{width:100px;color:black}", &mut p);
    let greedy = s.layout().text_layout(s.layout().cache().tree().iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::InlineContent { .. })).unwrap().0).unwrap().lines().len();
    let s = scene("<div>AAA BBB CCC DD</div>", "div{width:100px;color:black;text-wrap:balance}", &mut p);
    let text = s.layout().text_layout(s.layout().cache().tree().iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::InlineContent { .. })).unwrap().0).unwrap();
    assert_eq!(text.lines().len(), greedy, "balancing keeps the line count");
    let widths: Vec<f64> = text.lines().iter().map(|l| l.advance).collect();
    let (min, max) = (widths.iter().cloned().fold(f64::INFINITY, f64::min), widths.iter().cloned().fold(0., f64::max));
    assert!(max - min < 40., "balanced lines are about as long as each other: {widths:?}");
    assert!(widths.iter().all(|w| *w <= 100.));
}

#[test]
fn generated_content_paints_before_and_after_text_and_blocks() {
    let mut p = paint::Engine::default();
    // ::before text in the pseudo-element's own colour precedes the element's
    // text; ::after with attr() follows it; a block ::after sits below.
    let s = scene("<div><p data-x=\"B\">A</p></div>", "p{color:black}p::before{content:\"A \";color:green}p::after{content:attr(data-x);color:blue}", &mut p);
    let t = render(&mut p, &s);
    let first = |color: [u8; 4]| (0..100).find(|&x| pixel(&t, x, 3) == color);
    assert_eq!(first([0, 128, 0, 255]), Some(0), "::before glyph in green first");
    let own = first([0, 0, 0, 255]).expect("the element's own A");
    let after = first([0, 0, 255, 255]).expect("::after attr() glyph in blue");
    assert!(own > 0 && after > own, "before {:?} own {own} after {after}", first([0, 128, 0, 255]));
    let s = scene("<div><p>A</p></div>", "p::after{content:\"\";display:block;width:30px;height:10px;background:red}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 5, 25), [255, 0, 0, 255], "block ::after below the line");
    assert_eq!(pixel(&t, 35, 25), [255; 4]);
    // content: none and display: none generate nothing; a pseudo without
    // content is not a box.
    let s = scene("<div><p>A</p></div>", "p::before{content:none;color:green}p::after{color:green}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 1, 3), [0, 0, 0, 255]);
    assert_eq!(pixel(&t, 11, 3), [255; 4]);
}

#[test]
fn background_image_layers_tile_position_size_and_clip() {
    use olo_web::{image::Image, page};
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    // A 2 × 2 image: red left, blue right.
    let rgba: Vec<u8> = [[255, 0, 0, 255], [0, 0, 255, 255], [255, 0, 0, 255], [0, 0, 255, 255]].concat();
    let image = Image::from_rgba(2, 2, rgba, Default::default(), None).unwrap();
    let render = |css: &str| {
        let html = format!("<!doctype html><html><head><style>body,div{{margin:0;padding:0;border:0}}div{{width:40px;height:40px}}{css}</style></head><body><div></div></body></html>");
        let mut shaper = Shaper::default();
        let mut p = paint::Engine::default();
        let page = page::load_assets(
            Arc::from(html),
            &host,
            &scope,
            &mut shaper,
            &mut p,
            layout::Viewport { width: 100., height: 100., scale: 1. },
            None,
            |_, _| {},
            |kind, _, href| Ok((kind == page::ResourceKind::Image && href == "a.png").then(|| page::Resource::Image(image.clone()))),
        )
        .unwrap();
        assert!(page.fallbacks.is_empty(), "{}", page.fallbacks);
        let tile = p.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: 100, height: 100 }, None).unwrap();
        tile.into_rgba()
    };
    let px = |t: &[u8], x: usize, y: usize| -> [u8; 4] { t[(y * 100 + x) * 4..][..4].try_into().unwrap() };
    // Repeated at its natural size: red and blue alternate every pixel.
    let t = render("div{background:url(a.png)}");
    assert_eq!(px(&t, 0, 0), [255, 0, 0, 255]);
    assert_eq!(px(&t, 1, 0), [0, 0, 255, 255]);
    assert_eq!(px(&t, 39, 39), [0, 0, 255, 255]);
    assert_eq!(px(&t, 50, 50), [255; 4], "outside the box");
    // Sized to 20px and centred, no repeat: the box's corner is white and
    // the middle splits red | blue at x = 20.
    let t = render("div{background:url(a.png) center / 20px 20px no-repeat}");
    assert_eq!(px(&t, 2, 2), [255; 4]);
    assert_eq!(px(&t, 14, 20), [255, 0, 0, 255]);
    assert_eq!(px(&t, 26, 20), [0, 0, 255, 255]);
    // cover: scaled to the box, so the right half is blue everywhere.
    let t = render("div{background:url(a.png) center / cover no-repeat}");
    assert_eq!(px(&t, 5, 35), [255, 0, 0, 255]);
    assert_eq!(px(&t, 35, 5), [0, 0, 255, 255]);
}

#[test]
fn text_decoration_underline_and_line_through_paint_under_the_text() {
    let mut p = paint::Engine::default();
    // Without a decoration, the glyph's ink reaches down to some row; an
    // underline paints below that, in the text's colour; the line's own
    // colour when given; line-through crosses the x-height.
    let s = scene("<div>AA</div>", "div{color:black;text-decoration:none}", &mut p);
    let plain = render(&mut p, &s);
    let ink_bottom = (0..20).filter(|&y| pixel(&plain, 4, y) != [255; 4]).max().expect("glyph ink");
    let s = scene("<div><span id=u>AA</span> <span id=l>AA</span></div>", "div{color:black}#u{text-decoration:underline}#l{text-decoration:line-through red}", &mut p);
    let t = render(&mut p, &s);
    let column = |x: usize| (0..20).filter(|&y| pixel(&t, x, y) != [255; 4]).collect::<Vec<_>>();
    let under = column(4);
    assert!(under.iter().any(|&y| y > ink_bottom && pixel(&t, 4, y) == [0, 0, 0, 255]), "underline below the ink ({ink_bottom}): {under:?}");
    assert!((0..100).any(|x| (0..20).any(|y| pixel(&t, x, y) == [255, 0, 0, 255])), "red line-through");
    assert!((28..48).any(|x| column(x).iter().all(|&y| y <= ink_bottom)), "the space between the spans is not underlined");
}

#[test]
fn floats_shorten_lines_beside_them_and_clear_moves_below() {
    let mut p = paint::Engine::default();
    // A 30px left float, 20px tall, in a 100px container: the first line of
    // the paragraph starts after it, the lines below it start at 0; a right
    // float keeps lines off its left edge; a cleared block sits below.
    let s = scene(
        "<div id=c><div id=f></div><p>AA AA AA AA AA AA</p><div id=r></div><p id=q>BB BB BB</p><div id=k>C</div></div>",
        "#c{width:100px}#f{float:left;width:30px;height:20px;background:blue}#r{float:right;width:40px;height:20px;background:red}#k{clear:both}",
        &mut p,
    );
    let layout = s.layout();
    let tree = layout.cache().tree();
    let text_of = |id: layout::BoxId| layout.text_layout(id).unwrap();
    let paragraphs: Vec<_> = tree.iter().filter(|(_, b)| matches!(b.kind, layout::BoxKind::InlineContent { .. })).map(|(id, _)| id).collect();
    let first = text_of(paragraphs[0]);
    assert!(first.lines().len() >= 2, "{}", first.lines().len());
    assert_eq!(first.lines()[0].offset, 30., "first line starts after the float");
    assert_eq!(first.lines()[1].offset, 0., "the second line (y 20) is below the 20px float");
    let second = text_of(paragraphs[1]);
    // The right float is placed at the second paragraph's top: its lines end
    // before x = 60 (100 - 40).
    for line in second.lines().iter().take(1) {
        assert!(line.offset + line.advance <= 60.5, "line beside the right float: offset {} advance {}", line.offset, line.advance);
    }
    // The cleared block is below both floats' bottoms.
    let right_float = tree
        .iter()
        .find(|(id, b)| matches!(b.kind, layout::BoxKind::Block { .. }) && layout.bounds(*id).is_some_and(|r| r.width == 40.))
        .map(|(id, _)| layout.bounds(id).unwrap())
        .expect("right float");
    let k = tree
        .iter()
        .filter(|(id, b)| matches!(b.kind, layout::BoxKind::Block { .. }) && layout.bounds(*id).is_some_and(|r| r.width == 100.))
        .map(|(id, _)| layout.bounds(id).unwrap())
        .last()
        .expect("cleared block");
    assert!(k.y >= right_float.y + right_float.height, "cleared block y {} below float bottom {}", k.y, right_float.y + right_float.height);
    assert!(right_float.x == 60., "right float against the right edge: {:?}", right_float);
}
#[test]
fn text_edits_restyle_empty_only_when_text_comes_or_goes() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // An :empty test and a :has() of one: a text edit that keeps text
    // patches the styles; one that empties the element matches again.
    let mut page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}div{height:20px;color:transparent;background-color:green}div:empty{background-color:red}body:has(div:empty){background-color:blue}</style><div>x</div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let text_of = |doc: &olo_web::dom::Document| {
        let body = doc.children(doc.root()).unwrap().last().unwrap();
        let body = doc.children(body).unwrap().last().unwrap();
        let div = doc.children(body).unwrap().last().unwrap();
        doc.first_child(div).unwrap().unwrap()
    };
    assert_eq!(pixel(&render(&mut painter, &page.scene), 50, 10), [0, 128, 0, 255]);
    for (text, div, below) in [("yy", [0, 128, 0, 255], [255, 255, 255, 255]), ("", [255, 0, 0, 255], [0, 0, 255, 255]), ("z", [0, 128, 0, 255], [255, 255, 255, 255])] {
        let mut doc = page.document.clone();
        let t = text_of(&doc);
        doc.set_value(t, text).unwrap();
        page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        let tile = render(&mut painter, &page.scene);
        assert_eq!(pixel(&tile, 50, 10), div, "div after {text:?}");
        assert_eq!(pixel(&tile, 50, 50), below, "body after {text:?}");
    }
}
#[test]
fn a_patched_rerender_reports_the_whole_pages_fallbacks() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // A dashed border (painted solid) and a fixed box (laid out as
    // absolute, a layout fallback) on the page; then an inline style
    // changes on another box, patched over, its layout kept.
    let mut page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}#a{border:2px dashed red;height:10px}</style><div id=a></div><div style=display:flex;align-items:baseline><span>o</span></div><div id=b style='height:5px'>x</div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let before = page.fallbacks.to_string();
    assert!(before.contains("flex baseline alignment") && before.contains("solid"), "{before}");
    for colour in ["red", "blue", "green"] {
        let mut doc = page.document.clone();
        let body = doc.children(doc.root()).unwrap().last().unwrap();
        let body = doc.children(body).unwrap().last().unwrap();
        let b = doc.children(body).unwrap().last().unwrap();
        doc.set_attribute(b, "style", &format!("height:5px;color:{colour}")).unwrap();
        page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        assert_eq!(page.fallbacks.to_string(), before, "after {colour}");
    }
}
#[test]
fn flex_items_lay_out_in_order_modified_document_order() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}div{display:flex}span{display:block;width:20px;height:20px}</style><div><span style='order:1;background-color:red'></span><span style='order:-1;background-color:green'></span><span style='background-color:blue'></span><span style='order:1;background-color:black'></span></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    assert!(!page.fallbacks.to_string().contains("order"), "{}", page.fallbacks);
    let tile = render(&mut painter, &page.scene);
    // green (-1), blue (0), then red and black (1) in document order.
    assert_eq!([10, 30, 50, 70].map(|x| pixel(&tile, x, 10)), [[0, 128, 0, 255], [0, 0, 255, 255], [255, 0, 0, 255], [0, 0, 0, 255]]);
}
#[test]
fn column_flex_items_follow_order_too() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}div{display:flex;flex-direction:column}span{display:block;height:20px}</style><div><span style='order:1;background-color:red'></span><span style='background-color:green'></span><span style='order:-1;background-color:blue'></span></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!([10, 30, 50].map(|y| pixel(&tile, 50, y)), [[0, 0, 255, 255], [0, 128, 0, 255], [255, 0, 0, 255]]);
}
#[test]
fn a_z_index_inside_a_positioned_auto_box_rises_above_later_positioned_siblings() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    // Apple's hero: the copy (z-index 2) inside a relative box with z-index
    // auto, then an absolute image wrapper covering the tile. The copy
    // belongs to the root stacking context at z 2: above the image (red).
    // A z-index: auto child under the same box stays in tree order: under.
    let page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}</style><div style='position:relative;height:60px'><div style='position:relative'><div style='position:relative;z-index:2;height:20px;background-color:red'></div><div style='position:relative;height:20px;background-color:green'></div></div><div style='position:absolute;top:0;left:0;width:100%;height:60px;background-color:blue'></div></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!(pixel(&tile, 50, 10), [255, 0, 0, 255]);
    assert_eq!(pixel(&tile, 50, 30), [0, 0, 255, 255]);
}
#[test]
fn the_container_shorthand_makes_a_query_container() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    // 50cqw of a 60px inline-size container: 30px; of a named-only one
    // (not a size container): of the viewport, 50px.
    let page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}.c{container:card / inline-size;width:60px}.d{container:card;width:60px}.c div,.d div{width:50cqw;height:10px;background-color:red}</style><div class=c><div></div></div><div class=d><div></div></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!([(25, 5), (35, 5), (45, 15), (55, 15)].map(|(x, y)| pixel(&tile, x, y)[1]), [0, 255, 0, 255]);
}
#[test]
fn a_patched_element_keeps_its_descendant_rules_and_pseudo_elements() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // An attribute no selector reads changes on #x: its style is patched,
    // matched again, with the rules reaching it through its ancestors.
    let mut page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}.outer #x{padding-top:20px;background-color:green}html:not([dir=rtl]) #x::after{content:'';display:block;height:20px;background-color:blue}</style><div class=outer><div id=x></div></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    for i in 0..2 {
        let mut doc = page.document.clone();
        let body = doc.children(doc.root()).unwrap().last().unwrap();
        let body = doc.children(body).unwrap().last().unwrap();
        let outer = doc.children(body).unwrap().last().unwrap();
        let x = doc.first_child(outer).unwrap().unwrap();
        doc.set_attribute(x, "data-n", &i.to_string()).unwrap();
        page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        let tile = render(&mut painter, &page.scene);
        assert_eq!(pixel(&tile, 50, 10), [0, 128, 0, 255], "after change {i}");
        assert_eq!(pixel(&tile, 50, 30), [0, 0, 255, 255], "after change {i}");
    }
}
#[test]
fn intrinsic_widths_and_column_flow_grids() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    // A max-content block centred by its auto margins (40px of 100); a
    // column-flow grid as wide as its items side by side, each in a column.
    let page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}.f{width:max-content;margin:0 auto;background-color:red;height:10px}.f i{display:inline-block;width:40px;height:10px}.g{display:grid;grid-auto-flow:column;width:max-content;height:10px}.g div{width:20px}</style><div class=f><i></i></div><div class=g><div style=background-color:red></div><div style=background-color:blue></div><div style=background-color:green></div></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!([25, 35, 65, 75].map(|x| pixel(&tile, x, 5)), [[255; 4], [255, 0, 0, 255], [255, 0, 0, 255], [255; 4]]);
    assert_eq!([10, 30, 50, 70].map(|x| pixel(&tile, x, 15)), [[255, 0, 0, 255], [0, 0, 255, 255], [0, 128, 0, 255], [255; 4]]);
}
#[test]
fn a_grid_items_min_width_is_its_minimum_contribution() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    // Two 1fr columns of a 100px grid; the first item holds a 300px box.
    // With min-width: 0 it stays 50px (the second item at 50); without,
    // its column takes the box's width (as in Chrome).
    let page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}.g{display:grid;grid-template-columns:repeat(2,1fr);width:100px;height:10px}.w{width:300px;height:10px}.b{background-color:blue;height:10px}</style><div class=g><div style=min-width:0><div class=w></div></div><div class=b></div></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!([40, 60].map(|x| pixel(&tile, x, 5)), [[255; 4], [0, 0, 255, 255]]);
}
#[test]
fn an_opacity_change_patches_its_group_and_paints_as_a_new_scene_would() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // GitHub's corner dots: an element fading, frame by frame.
    let mut page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}#d{height:20px;background-color:rgb(0,0,255)}</style><div id=d style='opacity:0.5'></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let div = {
        let mut stack = vec![page.document.root()];
        let mut found = None;
        while let Some(n) = stack.pop() {
            if page.document.attribute(n, "id").ok().flatten() == Some("d") {
                found = Some(n);
            }
            stack.extend(page.document.children(n).unwrap());
        }
        found.unwrap()
    };
    // The first change builds the scene once (the element starts animating).
    for (opacity, blue_over_white) in [("0.4", 153u8), ("0.25", 191), ("0.75", 64), ("0", 255), ("0.5", 128), ("1", 0)] {
        let before = page.scene.clone();
        let mut doc = page.document.clone();
        doc.set_attribute(div, "style", &format!("opacity:{opacity}")).unwrap();
        page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        // Between 0 and 1 from a group: the same commands, its alpha patched.
        if opacity == "0.25" || opacity == "0.75" {
            assert!(page.scene.same_commands(&before), "opacity {opacity} built the scene");
        }
        let p = pixel(&render(&mut painter, &page.scene), 50, 10);
        assert!((i32::from(p[0]) - i32::from(blue_over_white)).abs() <= 1 && p[2] == 255, "opacity {opacity}: {p:?}");
    }
}
