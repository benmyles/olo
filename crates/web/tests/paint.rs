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
fn inline_backgrounds_paint_and_an_empty_one_says_it_is_not() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    for (body, notes) in [("<span></span>", vec![("empty inline background not painted", 1)]), ("<span><span style='background-color:transparent'>A</span></span>", vec![])] {
        let source = format!("<!doctype html><style>{BASE}span{{padding-left:5px;background-color:red}}</style>{body}");
        let mut p = paint::Engine::default();
        let page = olo_web::page::load(source.into(), &host, &scope, &mut Shaper::default(), &mut p, layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {}).unwrap();
        assert_eq!(page.fallbacks.counts(), notes, "{body}");
        if notes.is_empty() {
            // The outer span's background under the inner one's text, its
            // padding before it.
            let t = render(&mut p, &page.scene);
            let reds = (0..100).flat_map(|y| (0..100).map(move |x| (x, y))).filter(|&(x, y)| pixel(&t, x, y) == [255, 0, 0, 255]).count();
            assert!(reds > 40, "{reds}");
            let first = (0..100).flat_map(|y| (0..100).map(move |x| (x, y))).filter(|&(x, y)| pixel(&t, x, y) == [255, 0, 0, 255]).min_by_key(|&(x, _)| x).unwrap();
            assert_eq!(first.0, 0, "its padding");
        }
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
    // A url() filter (not drawn yet: a paint fallback) and a flex
    // baseline (a layout fallback) on the page; then an inline style
    // changes on another box, patched over, its layout kept.
    let mut page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}#a{filter:url(#f);height:10px}</style><div id=a></div><div style=display:flex;align-items:baseline><span>o</span></div><div id=b style='height:5px'>x</div>"),
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
    assert!(before.contains("flex baseline alignment") && before.contains("url()"), "{before}");
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
#[test]
fn a_focus_set_by_the_page_matches_focus_within_on_a_rerender() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // GitHub's sign-up label floats while its field has the focus.
    let mut page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}#f{height:40px}#l{display:block;height:20px;background-color:blue}#f:focus-within #l{background-color:red}</style><div id=f><span id=l></span><input id=i></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let find = |page: &olo_web::page::Page, id: &str| {
        let mut stack = vec![page.document.root()];
        while let Some(n) = stack.pop() {
            if page.document.attribute(n, "id").ok().flatten() == Some(id) {
                return n;
            }
            stack.extend(page.document.children(n).unwrap());
        }
        panic!("{id}")
    };
    assert_eq!(pixel(&render(&mut painter, &page.scene), 10, 10), [0, 0, 255, 255]);
    page.interaction.focus = Some(find(&page, "i"));
    let doc = page.document.clone();
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert_eq!(pixel(&render(&mut painter, &page.scene), 10, 10), [255, 0, 0, 255], "focused");
    page.interaction.focus = None;
    let doc = page.document.clone();
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert_eq!(pixel(&render(&mut painter, &page.scene), 10, 10), [0, 0, 255, 255], "blurred");
}

#[test]
fn gradient_layers_take_their_size_position_and_repeat() {
    let mut p = paint::Engine::default();
    // Half the width, not repeated: the right half shows the page.
    let s = scene("<div></div>", "div{width:100px;height:20px;background:linear-gradient(red,red) no-repeat;background-size:50% 100%}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 10, 10), [255, 0, 0, 255]);
    assert_eq!(pixel(&t, 75, 10), [255; 4]);
    // Placed at the far end of the free space.
    let s = scene("<div></div>", "div{width:100px;height:20px;background:linear-gradient(red,red) no-repeat 100% 0;background-size:50% 100%}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 10, 10), [255; 4]);
    assert_eq!(pixel(&t, 75, 10), [255, 0, 0, 255]);
    // A 20px tile repeated along x: each tile goes red to blue again.
    let s = scene("<div></div>", "div{width:100px;height:20px;background:linear-gradient(to right, red, blue) repeat-x;background-size:20px 100%}", &mut p);
    let t = render(&mut p, &s);
    for x in [1, 21, 61] {
        let c = pixel(&t, x, 10);
        assert!(c[0] > 200 && c[2] < 60, "x {x}: {c:?}");
    }
    let c = pixel(&t, 39, 10);
    assert!(c[2] > 200 && c[0] < 60, "{c:?}");
    // Three times as wide and at the start: the box shows its first third.
    let s = scene("<div></div>", "div{width:100px;height:20px;background:linear-gradient(to right, red 33.3%, blue 33.4%) no-repeat 0 0;background-size:300% 100%}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 95, 10), [255, 0, 0, 255]);
}

#[test]
fn background_clip_text_paints_the_backgrounds_through_the_glyphs() {
    let mut p = paint::Engine::default();
    let s = scene("<p>HHHH</p>", "p{margin:0;width:100px;font-size:40px;line-height:50px;color:transparent;background:linear-gradient(to right, red, blue);background-clip:text}", &mut p);
    assert!(!s.fallbacks().counts().iter().any(|(what, _)| what.contains("background-clip")), "{:?}", s.fallbacks());
    let t = render(&mut p, &s);
    let mut red = 0;
    let mut blue = 0;
    let mut white = 0;
    for y in 0..50 {
        for x in 0..100 {
            match pixel(&t, x, y) {
                [255, 255, 255, 255] => white += 1,
                [r, _, b, _] if r > 180 && b < 120 => red += 1,
                [r, _, b, _] if b > 180 && r < 120 => blue += 1,
                _ => {}
            }
        }
    }
    // The glyphs show the gradient, red at the left and blue at the right;
    // between and around them the page shows.
    assert!(red > 20 && blue > 20, "red {red}, blue {blue}");
    assert!(white > 500, "white {white}");
    // An opaque colour paints over the background.
    let s = scene("<p>HHHH</p>", "p{margin:0;width:100px;font-size:40px;color:lime;background:red;background-clip:text}", &mut p);
    let t = render(&mut p, &s);
    let reds = (0..50).flat_map(|y| (0..100).map(move |x| (x, y))).filter(|&(x, y)| pixel(&t, x, y) == [255, 0, 0, 255]).count();
    assert_eq!(reds, 0);
}

#[test]
fn a_transitioned_property_eases_to_its_new_value_and_says_so() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // apple.com's search field focuses on transitionend; menus fade.
    let mut page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}#d{height:20px;background:rgb(0,0,255);opacity:0;transition:opacity 1s linear, transform .5s}#d.on{opacity:1;transform:translateX(40px)}</style><div id=d></div>"),
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
    let opacity = |page: &olo_web::page::Page| {
        let l = page.scene.layout();
        let tree = l.cache().tree();
        let (id, _) = tree.iter().find(|(_, b)| b.node == div).unwrap();
        tree.style(id).unwrap().value(olo_web::css::properties::Property::Opacity).scalar().unwrap()
    };
    page.set_time(1000.);
    let mut doc = page.document.clone();
    doc.set_attribute(div, "class", "on").unwrap();
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert_eq!(opacity(&page), 0., "it starts where it was");
    assert!(page.animating());
    let kinds = |page: &mut olo_web::page::Page| page.take_animation_events().into_iter().map(|(_, k, n, _)| format!("{k} {n}")).collect::<Vec<_>>();
    assert_eq!(kinds(&mut page).len(), 4, "run and start for opacity and transform");
    for (t, want) in [(1250., 0.25), (1500., 0.5)] {
        page.set_time(t);
        page.rerender(page.document.clone(), Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        assert!((opacity(&page) - want).abs() < 0.02, "at {t}: {}", opacity(&page));
    }
    assert_eq!(kinds(&mut page), ["transitionend transform"]);
    page.set_time(2100.);
    page.rerender(page.document.clone(), Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert_eq!(opacity(&page), 1.);
    assert_eq!(kinds(&mut page), ["transitionend opacity"]);
    page.set_time(2200.);
    page.rerender(page.document.clone(), Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert!(!page.animating(), "done");
    // Back, halfway: it turns around from where it is.
    let mut doc = page.document.clone();
    doc.set_attribute(div, "class", "").unwrap();
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    page.set_time(2700.);
    page.rerender(page.document.clone(), Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert!((opacity(&page) - 0.5).abs() < 0.02, "{}", opacity(&page));
}

#[test]
fn a_transition_patched_over_a_whole_restyle_keeps_its_new_geometry() {
    // BBC's menu: a section opened (its rule written into a <style> by
    // its CSS-in-JS, a whole restyle) while its chevron starts turning
    // (a transition, patched after): the old layout was kept, shut.
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    let mut page = olo_web::page::load(
        Arc::from("<!doctype html><style id=s>body{margin:0}#d{width:10px;height:10px;transition:transform 1s}#d.on{transform:rotate(180deg)}</style><div id=d></div><div id=c></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let find = |page: &olo_web::page::Page, id: &str| {
        let mut stack = vec![page.document.root()];
        while let Some(n) = stack.pop() {
            if page.document.attribute(n, "id").ok().flatten() == Some(id) {
                return n;
            }
            stack.extend(page.document.children(n).unwrap());
        }
        panic!("no #{id}")
    };
    let (d, c, s) = (find(&page, "d"), find(&page, "c"), find(&page, "s"));
    page.set_time(1000.);
    let mut doc = page.document.clone();
    doc.set_attribute(d, "class", "on").unwrap();
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert!(page.animating(), "the transition runs");
    // While it turns: the section's rule comes, and its children.
    page.set_time(1100.);
    let mut doc = page.document.clone();
    let text = doc.children(s).unwrap().next().unwrap();
    doc.set_value(text, "body{margin:0}#d{width:10px;height:10px;transition:transform 1s}#d.on{transform:rotate(180deg)}.open div{height:15px}").unwrap();
    doc.set_attribute(c, "class", "open").unwrap();
    for _ in 0..2 {
        let child = doc.create_element("div", olo_web::dom::Namespace::Html).unwrap();
        doc.append_child(c, child).unwrap();
    }
    assert!(page.refresh(&doc, None, &mut |_, _, _| Ok(None), None).unwrap(), "the rules are made again");
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    let l = page.scene.layout();
    let (id, _) = l.cache().tree().iter().find(|(_, b)| b.node == c).unwrap();
    assert_eq!(l.bounds(id).unwrap().height, 30.);
}

#[test]
fn text_overflow_ellipsis_cuts_a_clipped_line_with_its_mark() {
    let mut p = paint::Engine::default();
    let glyphs = |css: &str, p: &mut paint::Engine| {
        let s = scene("<div>AAAAAAAAAAAAAAAAAAAA</div>", &format!("div{{width:50px;white-space:nowrap;overflow:hidden;{css}}}"), p);
        s.commands().iter().filter(|c| matches!(c.primitive, paint::Primitive::Glyph { .. })).map(|c| c.bounds.x).collect::<Vec<_>>()
    };
    // The fixture font has neither "…" nor "." (only A-E, f, i and space):
    // no mark, the line as it was (a page's fonts have them; compared with
    // Chrome by eye: same cut, same mark).
    assert!(glyphs("text-overflow:ellipsis", &mut p).is_empty());
    assert!(glyphs("text-overflow:clip", &mut p).is_empty());
    assert!(glyphs("text-overflow:ellipsis;overflow:visible", &mut p).is_empty(), "only when it clips");
}

#[test]
fn inset_shadows_paint_inside_the_padding_box() {
    let mut p = paint::Engine::default();
    // A hard inset shadow 4 px from the top left; a ring of spread.
    let s = scene("<div></div>", "div{width:40px;height:40px;margin:10px;background:white;box-shadow:inset 4px 4px 0 rgb(200,0,0)}", &mut p);
    assert!(s.fallbacks().is_empty(), "{:?}", s.fallbacks());
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 12, 30), [200, 0, 0, 255], "inside, the shadowed left");
    assert_eq!(pixel(&t, 30, 30), [255; 4], "the middle shows the background");
    assert_eq!(pixel(&t, 48, 30), [255; 4], "the right edge (the hole moved right)");
    assert_eq!(pixel(&t, 8, 30), [255; 4], "nothing outside the box");
    let s = scene("<div></div>", "div{width:40px;height:40px;margin:10px;background:white;box-shadow:inset 0 0 0 3px rgb(0,0,200)}", &mut p);
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 11, 30), [0, 0, 200, 255]);
    assert_eq!(pixel(&t, 48, 30), [0, 0, 200, 255]);
    assert_eq!(pixel(&t, 30, 30), [255; 4]);
}

#[test]
fn a_shadow_tree_renders_in_its_hosts_place_with_its_slots() {
    // GitHub's <relative-time>: its shadow root's text ("last week") in
    // place of its own (the server's date); a <slot> takes the host's
    // children.
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 400., height: 100., scale: 1. };
    let mut page = olo_web::page::load(
        Arc::from(format!("<!doctype html><style>{BASE}b{{display:block}}</style><p><span id=t>DDDD</span></p><div id=h><b id=l>CC</b></div>")),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let find = |doc: &olo_web::dom::Document, id: &str| {
        let mut stack = vec![doc.root()];
        while let Some(n) = stack.pop() {
            if doc.attribute(n, "id").ok().flatten() == Some(id) {
                return n;
            }
            stack.extend(doc.children(n).unwrap());
        }
        panic!("no #{id}")
    };
    let mut doc = page.document.clone();
    let (t, h, l) = (find(&doc, "t"), find(&doc, "h"), find(&doc, "l"));
    // The span shows its shadow text instead of its own.
    let root = doc.attach_shadow(t).unwrap();
    let text = doc.create_text("AB").unwrap();
    doc.append_child(root, text).unwrap();
    // The div: "E", its child through a slot, then "f".
    let root = doc.attach_shadow(h).unwrap();
    for part in ["E", "slot", "f"] {
        let n = if part == "slot" { doc.create_element("slot", olo_web::dom::Namespace::Html).unwrap() } else { doc.create_text(part).unwrap() };
        doc.append_child(root, n).unwrap();
    }
    assert!(doc.is_connected(text).unwrap());
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    let shown: Vec<String> = page
        .scene
        .commands()
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c.primitive, paint::Primitive::Text { .. }))
        .filter_map(|(i, _)| page.scene.text_content(i as u32).map(|t| t.into_owned()))
        .collect();
    let all = shown.join("|");
    assert!(all.contains("AB"), "{all}");
    assert!(!all.contains("DDDD"), "{all}");
    assert!(all.contains('E') && all.contains("CC") && all.contains('f'), "{all}");
    let lb = page.scene.layout().cache().tree().iter().find(|(_, x)| x.node == l && matches!(x.kind, layout::BoxKind::Block { .. })).map(|(id, _)| page.scene.layout().bounds(id).unwrap());
    assert!(lb.is_some_and(|r| r.height > 0.), "the slotted child laid out: {lb:?}");
}
#[test]
fn a_live_canvas_is_a_surface_cut_by_its_clip_and_rounded_by_its_own() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 300., height: 200., scale: 1. };
    // One canvas half outside a straight clip (GitHub's mascot, below the
    // edge of its box), one filling a rounded box (a video tile), one
    // alone in an opacity group (a video fading in: shown at its opacity),
    // and one sharing its group with a background (painted with the page).
    let mut page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0}div{overflow:hidden}canvas{display:block;width:100px;height:100px}</style>\
             <div style='width:60px;height:40px'><canvas></canvas></div>\
             <div style='width:100px;height:100px;border-radius:10px'><canvas></canvas></div>\
             <section style='opacity:.5'><canvas></canvas></section>\
             <section style='opacity:.5;background:red'><canvas></canvas></section>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let doc = page.document.clone();
    let mut canvases = Vec::new();
    let mut next = Some(doc.root());
    while let Some(n) = next {
        if doc.name(n).unwrap() == Some("canvas") {
            canvases.push((n, olo_web::image::Image::shared(100, 100, canvases.len() as u64 + 1, true)));
        }
        next = doc.first_child(n).unwrap().or_else(|| {
            let mut at = Some(n);
            while let Some(a) = at {
                if let Some(s) = doc.next_sibling(a).unwrap() {
                    return Some(s);
                }
                at = doc.parent(a).unwrap();
            }
            None
        });
    }
    assert_eq!(canvases.len(), 4);
    page.rerender(doc, canvases, &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    let scene = &page.scene;
    assert_eq!(scene.surface_count(), 3);
    let shown: Vec<paint::SurfaceShown> = (0..scene.strata().len()).filter_map(|i| scene.surface(i).map(|(s, _)| s)).collect();
    let rect = |x, y, width, height| layout::Rect { x, y, width, height };
    assert_eq!(shown[0], paint::SurfaceShown { bounds: rect(0., 0., 100., 100.), clip: Some(rect(0., 0., 60., 40.)), radii: None, opacity: 1. });
    assert_eq!(shown[1], paint::SurfaceShown { bounds: rect(0., 40., 100., 100.), clip: Some(rect(0., 40., 100., 100.)), radii: Some([10.; 4]), opacity: 1. });
    assert_eq!(shown[2], paint::SurfaceShown { bounds: rect(0., 140., 100., 100.), clip: None, radii: None, opacity: 0.5 });
}
#[test]
fn attribute_changes_reach_through_states_and_not_through_any_element() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // `:disabled + div` follows the input's `disabled` attribute (a state
    // it reflects); `* > p` matches whatever the attributes are, so an
    // attribute flip under it is patched, not restyled whole.
    let mut page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0}div,p{height:20px;margin:0;background-color:red}:disabled+div{background-color:green}\
             * > p{background-color:blue}p[data-on]{background-color:green}input{display:block;height:10px;margin:0;border:0;padding:0}</style>\
             <input><div></div><p></p>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!(pixel(&tile, 50, 15), [255, 0, 0, 255]);
    assert_eq!(pixel(&tile, 50, 35), [0, 0, 255, 255]);
    let mut doc = page.document.clone();
    let body = doc.children(doc.root()).unwrap().last().unwrap();
    let body = doc.children(body).unwrap().last().unwrap();
    let children = doc.children(body).unwrap();
    let elements: Vec<_> = children.into_iter().filter(|&n| doc.name(n).unwrap().is_some()).collect();
    doc.set_attribute(elements[0], "disabled", "").unwrap();
    doc.set_attribute(elements[2], "data-on", "").unwrap();
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!(pixel(&tile, 50, 15), [0, 128, 0, 255], "the div after the input's disabled");
    assert_eq!(pixel(&tile, 50, 35), [0, 128, 0, 255], "the p with data-on");
}
#[test]
fn an_interaction_restyle_matches_only_what_the_states_reach_and_paints_as_a_whole_one() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // A hovered card colours its title (a descendant reached through
    // `:hover`), the box after it (a sibling), and the page (`:has()`).
    let html = "<!doctype html><style>body{margin:0}div{height:10px;background-color:white}p{margin:0;height:10px;background-color:white}\
        .card:hover .title{background-color:green}.card:hover+.next{background-color:blue}body:has(.card:hover) .last{background-color:red}</style>\
        <div class=card><p class=title></p></div><div class=next></div><div class=last></div><div class=other></div>";
    let load = |shaper: &mut Shaper, painter: &mut paint::Engine| olo_web::page::load(Arc::from(html), &host, &scope, shaper, painter, viewport, None, |_, _| {}).unwrap();
    let mut page = load(&mut shaper, &mut painter);
    // [title, other, card] of a page's document.
    let nodes = |doc: &olo_web::dom::Document| {
        let html_node = doc.children(doc.root()).unwrap().last().unwrap();
        let body = doc.children(html_node).unwrap().last().unwrap();
        let divs: Vec<_> = doc.children(body).unwrap().into_iter().filter(|&n| doc.name(n).unwrap() == Some("div")).collect();
        let title = doc.children(divs[0]).unwrap().into_iter().find(|&n| doc.name(n).unwrap() == Some("p")).unwrap();
        [title, divs[3], divs[0]]
    };
    let rows = |page: &olo_web::page::Page, painter: &mut paint::Engine| {
        let tile = render(painter, &page.scene);
        [5, 15, 25, 35].map(|y| pixel(&tile, 50, y))
    };
    let white = [255, 255, 255, 255];
    let hovered = [[0, 128, 0, 255], [0, 0, 255, 255], [255, 0, 0, 255], white];
    for (hover, expected) in [(Some(0), hovered), (Some(1), [white; 4]), (Some(2), hovered), (None, [white; 4])] {
        page.interaction.hover = hover.map(|i| nodes(&page.document)[i]);
        page.restyle_interaction(&host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        assert_eq!(rows(&page, &mut painter), expected, "hovering {hover:?}");
        // As a page styled whole with that hover has it.
        let mut whole = load(&mut shaper, &mut painter);
        whole.interaction.hover = hover.map(|i| nodes(&whole.document)[i]);
        whole.restyle(&host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        assert_eq!(rows(&whole, &mut painter), expected, "styled whole, hovering {hover:?}");
    }
}
#[test]
fn inline_elements_paint_at_their_opacity() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 200., height: 100., scale: 1. };
    // A word faded out, one half faded inside a faded span (0.5 × 0.5),
    // and an inline background at half opacity.
    let page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0;font:16px 'Olo Inline plain';color:rgb(0,0,0)}</style>\
             <p style=margin:0><span style=opacity:0>gone</span> <span style=opacity:0.5>half <b style=opacity:0.5>quarter</b></span></p>\
             <p style=margin:0><span style='opacity:0.5;background-color:rgb(0,0,255)'>blue</span></p>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let alphas: Vec<u8> = page.scene.commands().iter().filter_map(|c| if let paint::Primitive::Text { color, .. } = c.primitive { Some(color[3]) } else { None }).collect();
    assert!(alphas.contains(&128) && alphas.contains(&64), "half and quarter: {alphas:?}");
    let fills: Vec<[u8; 4]> = page.scene.commands().iter().filter_map(|c| if let paint::Primitive::Fill { color, .. } = c.primitive { Some(color) } else { None }).collect();
    assert!(fills.contains(&[0, 0, 255, 128]), "the background at half: {fills:?}");
    // Nothing of the faded-out word shows: no opaque black glyph run.
    assert!(page.scene.commands().iter().all(|c| !matches!(c.primitive, paint::Primitive::Text { color: [0, 0, 0, 255], .. })), "{alphas:?}");
}
#[test]
fn inline_backgrounds_round_their_ends_and_not_their_line_breaks() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // A span across two lines: its first piece rounded on the left only,
    // its last on the right only.
    let page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0;font:16px 'Olo Inline plain';width:60px}span{background-color:rgb(0,0,255);border-radius:6px}</style><p style=margin:0><span>aaaa aaaa</span></p>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let pieces: Vec<[f32; 4]> = page.scene.commands().iter().filter_map(|c| if let paint::Primitive::Fill { color: [0, 0, 255, 255], radii, .. } = c.primitive { Some(radii) } else { None }).collect();
    assert_eq!(pieces, [[6., 0., 0., 6.], [0., 6., 6., 0.]]);
    // The first piece's top-left corner shows the page; its top right doesn't.
    let tile = render(&mut painter, &page.scene);
    let first = page.scene.commands().iter().find(|c| matches!(c.primitive, paint::Primitive::Fill { color: [0, 0, 255, 255], .. })).unwrap().bounds;
    let (x0, y0) = (first.x.ceil() as usize, first.y.ceil() as usize);
    let x1 = (first.x + first.width).floor() as usize - 1;
    assert_eq!(pixel(&tile, x0, y0), [255, 255, 255, 255]);
    assert_eq!(pixel(&tile, x1, y0), [0, 0, 255, 255]);
}
#[test]
fn a_shadow_trees_sheets_style_its_tree_its_host_and_what_is_slotted() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // The document's `p` rule doesn't reach the shadow tree's p; the
    // shadow sheet's `p` doesn't reach the document's. The host takes
    // `:host`'s height, but the document's background over it (the outer
    // tree wins); the slotted span takes `::slotted(span)`'s.
    let mut page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0}p{margin:0;height:10px;background-color:red}#h{background-color:blue}span{display:block;height:10px}#h::part(tail){background-color:rgb(255,0,255)}</style>\
             <div id=h><span>s</span></div><p id=outer></p>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let mut doc = page.document.clone();
    let mut stack = vec![doc.root()];
    let mut h = None;
    while let Some(n) = stack.pop() {
        if doc.attribute(n, "id").ok().flatten() == Some("h") {
            h = Some(n);
        }
        stack.extend(doc.children(n).unwrap());
    }
    let h = h.unwrap();
    let root = doc.attach_shadow(h).unwrap();
    let style = doc.create_element("style", olo_web::dom::Namespace::Html).unwrap();
    let css = doc.create_text(":host{display:block;padding-top:20px;background-color:yellow}p{margin:0;height:10px;background-color:green}::slotted(span){background-color:rgb(0,255,255)}i{display:block;height:10px;background-color:black}").unwrap();
    doc.append_child(style, css).unwrap();
    doc.append_child(root, style).unwrap();
    let p = doc.create_element("p", olo_web::dom::Namespace::Html).unwrap();
    doc.append_child(root, p).unwrap();
    let slot = doc.create_element("slot", olo_web::dom::Namespace::Html).unwrap();
    doc.append_child(root, slot).unwrap();
    // A part the document styles: its rule wins over the shadow tree's own.
    let tail = doc.create_element("p", olo_web::dom::Namespace::Html).unwrap();
    doc.set_attribute(tail, "part", "tail").unwrap();
    doc.append_child(root, tail).unwrap();
    // A host inside the shadow tree: the tree's `:host` rule is not its.
    let inner = doc.create_element("i", olo_web::dom::Namespace::Html).unwrap();
    doc.append_child(root, inner).unwrap();
    doc.attach_shadow(inner).unwrap();
    page.refresh(&doc, None, &mut |_, _, _| Ok(None), None).unwrap();
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    let tile = render(&mut painter, &page.scene);
    // The host: 20px of padding (from :host) in blue (the document's).
    assert_eq!(pixel(&tile, 50, 10), [0, 0, 255, 255], "the host's padding, the document's background");
    assert_eq!(pixel(&tile, 50, 25), [0, 128, 0, 255], "the shadow tree's p");
    assert_eq!(pixel(&tile, 50, 35), [0, 255, 255, 255], "the slotted span");
    assert_eq!(pixel(&tile, 50, 45), [255, 0, 255, 255], "the part, as the document styles it");
    assert_eq!(pixel(&tile, 50, 55), [0, 0, 0, 255], "a host in the shadow tree, without the tree's :host rule");
    assert_eq!(pixel(&tile, 50, 65), [255, 0, 0, 255], "the document's p");
}
#[test]
fn a_filter_blurs_and_recolours_its_box_past_its_edges() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // A red box blurred by 4px (it spreads 12px past its edges, where no
    // other command is), and a green one greyed.
    let page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0}div{position:absolute;width:30px;height:30px}</style>\
             <div style='left:30px;top:10px;background:#f00;filter:blur(4px)'></div>\
             <div style='left:30px;top:60px;background:#0a0;filter:grayscale(1)'></div>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!(pixel(&tile, 45, 25), [255, 0, 0, 255], "inside, far from the edges: as it was");
    let edge = pixel(&tile, 30, 25);
    assert!(edge[0] == 255 && edge[1] > 80 && edge[1] < 180, "at the edge, half blurred: {edge:?}");
    let outside = pixel(&tile, 25, 25);
    assert!(outside[1] < 250 && outside[1] > 180, "past the edge, the blur's tail: {outside:?}");
    assert_eq!(pixel(&tile, 10, 25), [255, 255, 255, 255], "beyond its reach: the page");
    let grey = pixel(&tile, 45, 75);
    assert!(grey[0] == grey[1] && grey[1] == grey[2] && grey[0] > 118 && grey[0] < 126, "greyed to its luma (0.7152 x 170): {grey:?}");
}
#[test]
fn a_blur_spreads_before_its_ancestors_clip_and_scales_with_its_transform() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // Reddit's media backdrop: a picture scaled past a box that clips it,
    // and blurred. Its red reaches 10px past the box each way. (A box
    // painted after it: the blurred group is left mid-way, not at the end.)
    let page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0}</style>\
             <div style='position:absolute;left:30px;top:30px;width:40px;height:40px;overflow:hidden'>\
             <div style='width:40px;height:40px;background:#f00;transform:scale(1.5);filter:blur(4px)'></div></div>\
             <div style='position:absolute;left:90px;top:90px;width:5px;height:5px;background:#00f'></div>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!(pixel(&tile, 50, 50), [255, 0, 0, 255], "the middle");
    // Blurred before the clip, as Chrome draws it: at the clip's edge the
    // red's edge is still 10.5px off, not half faded (G 128) as if the
    // clip had cut it first. And blurred in the box's space, then scaled:
    // 6px on screen (Φ(10.5 / 6), G about 10), not 4 (G about 1).
    let edge = pixel(&tile, 30, 50);
    assert!(edge[0] == 255 && edge[1] > 4 && edge[1] < 25, "at the clip, nearly all red, the blur scaled with the box: {edge:?}");
    assert_eq!(pixel(&tile, 25, 50), [255, 255, 255, 255], "outside the clip: the page");
    assert_eq!(pixel(&tile, 50, 75), [255, 255, 255, 255], "below the clip: the page");
    assert_eq!(pixel(&tile, 92, 92), [0, 0, 255, 255], "the box after it");
}
#[test]
fn an_atomic_inline_takes_its_opacity_filter_and_transform() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // Inline SVGs in a line, as images are: half transparent, greyed, and
    // moved down 30px.
    let svg = |style: &str| format!("<svg width=20 height=20 style='vertical-align:top;{style}'><rect width=20 height=20 fill='#f00'/></svg>");
    let page = olo_web::page::load(
        Arc::from(format!(
            "<!doctype html><style>body{{margin:0;font-size:0}}</style><p style='margin:0'>{}{}{}</p>",
            svg("opacity:.5"),
            svg("filter:grayscale(1)"),
            svg("transform:translateY(30px)")
        )),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    let half = pixel(&tile, 10, 10);
    assert!(half[0] == 255 && half[1] > 120 && half[1] < 135, "half transparent over white: {half:?}");
    let grey = pixel(&tile, 30, 10);
    assert!(grey[0] == grey[1] && grey[1] == grey[2] && grey[0] > 50 && grey[0] < 60, "greyed to its luma (0.2126 × 255): {grey:?}");
    assert_eq!(pixel(&tile, 50, 10), [255, 255, 255, 255], "moved away from its place");
    assert_eq!(pixel(&tile, 50, 40), [255, 0, 0, 255], "moved down");
}
#[test]
fn a_class_that_selectors_reach_from_restyles_what_they_reach() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // apple.com's menu: a class on an item colours what is under it and
    // the box after it.
    let mut page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0}div,p{margin:0;height:10px;background-color:white}.open .item{background-color:green}.open+.after{background-color:blue}</style>\
             <div id=m><p class=item></p></div><div class=after></div>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let rows = |page: &olo_web::page::Page, painter: &mut paint::Engine| {
        let tile = render(painter, &page.scene);
        [5, 15].map(|y| pixel(&tile, 50, y))
    };
    assert_eq!(rows(&page, &mut painter), [[255, 255, 255, 255]; 2]);
    for (class, expected) in [("open", [[0, 128, 0, 255], [0, 0, 255, 255]]), ("", [[255, 255, 255, 255]; 2])] {
        let mut doc = page.document.clone();
        let mut stack = vec![doc.root()];
        let mut m = None;
        while let Some(n) = stack.pop() {
            if doc.attribute(n, "id").ok().flatten() == Some("m") {
                m = Some(n);
            }
            stack.extend(doc.children(n).unwrap());
        }
        doc.set_attribute(m.unwrap(), "class", class).unwrap();
        page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        assert_eq!(rows(&page, &mut painter), expected, "class {class:?}");
    }
}
#[test]
fn a_style_attribute_a_has_tests_restyles_what_has_it() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // YouTube's sheets: a `:has()` testing the style attribute, one with a
    // sibling combinator. Patched (the elements before and above the one
    // changed matched again), not the page restyled whole.
    let mut page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0}div{height:10px;background-color:white}\
             .a:has(+ .b[style]){background-color:green}.c:has(i[style]){background-color:blue}</style>\
             <div class=a></div><div class=b id=b></div><div class=c><p style='margin:0'><i id=i></i></p></div>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let rows = |page: &olo_web::page::Page, painter: &mut paint::Engine| {
        let tile = render(painter, &page.scene);
        [5, 25].map(|y| pixel(&tile, 50, y))
    };
    let white = [255, 255, 255, 255];
    assert_eq!(rows(&page, &mut painter), [white; 2]);
    let find = |doc: &olo_web::dom::Document, id: &str| {
        let mut stack = vec![doc.root()];
        while let Some(n) = stack.pop() {
            if doc.attribute(n, "id").ok().flatten() == Some(id) {
                return n;
            }
            stack.extend(doc.children(n).unwrap());
        }
        panic!("no #{id}")
    };
    for (id, style, expected) in
        [("b", Some("color:red"), [[0, 128, 0, 255], white]), ("i", Some("color:red"), [[0, 128, 0, 255], [0, 0, 255, 255]]), ("b", None, [white, [0, 0, 255, 255]]), ("i", None, [white; 2])]
    {
        let mut doc = page.document.clone();
        let n = find(&doc, id);
        match style {
            Some(v) => drop(doc.set_attribute(n, "style", v).unwrap()),
            None => drop(doc.remove_attribute(n, "style").unwrap()),
        }
        page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        assert_eq!(rows(&page, &mut painter), expected, "#{id} style {style:?}");
    }
}
#[test]
fn a_declarative_shadow_root_renders_with_its_own_sheet() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    // Server-rendered web components: the template becomes the div's
    // shadow root, its <style> styling only its own <p>.
    let page = olo_web::page::load(
        Arc::from(
            "<!doctype html><style>body{margin:0}p{margin:0;height:10px;background-color:red}</style>\
             <div><template shadowrootmode=open><style>p{margin:0;height:10px;background-color:green}</style><p></p><slot></slot></template><p></p></div>",
        ),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let tile = render(&mut painter, &page.scene);
    assert_eq!(pixel(&tile, 50, 5), [0, 128, 0, 255], "the shadow tree's p");
    assert_eq!(pixel(&tile, 50, 15), [255, 0, 0, 255], "the slotted light p, the document's style");
    let doc = &page.document;
    assert_eq!(doc.shadow_roots().len(), 1);
}
#[test]
fn an_iframe_with_frameborder_0_has_no_border() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    let page = olo_web::page::load(
        Arc::from("<!doctype html><style>body{margin:0}</style><iframe width=40 height=20 frameborder=0></iframe><iframe width=40 height=20></iframe>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let borders = page.scene.commands().iter().filter(|c| matches!(c.primitive, paint::Primitive::Border { .. })).count();
    assert_eq!(borders, 1, "only the iframe without frameborder=0 has its border");
}

#[test]
fn checkboxes_radios_and_selects_draw_as_chrome_in_their_scheme() {
    let mut p = paint::Engine::default();
    let css = "input,select{display:inline-block;appearance:auto;width:13px;height:13px;margin:0;padding:0;border:0}div{height:13px}";
    let s = scene(
        "<div><input type=checkbox checked><input type=checkbox><input type=radio checked><input type=checkbox checked style='appearance:none'></div><div style='color-scheme:dark'><input type=checkbox checked><input type=checkbox></div><div><input type=checkbox checked style='accent-color:red'><input type=checkbox disabled></div>",
        css,
        &mut p,
    );
    let t = render(&mut p, &s);
    // Checked: Chrome's accent under a white check; unchecked: white in a
    // #767676 border; a radio: its accent ring and dot about white.
    assert_eq!(pixel(&t, 2, 3 + 2), [0x00, 0x75, 0xff, 255]);
    assert_eq!(pixel(&t, 13 + 6, 3 + 6), [255; 4]);
    assert_eq!(pixel(&t, 13, 3 + 6), [0x76, 0x76, 0x76, 255]);
    assert_eq!(pixel(&t, 26 + 6, 3 + 6), [0x00, 0x75, 0xff, 255]);
    assert_eq!(pixel(&t, 26 + 1, 3 + 6), [255; 4]);
    // `appearance: none`: nothing drawn.
    assert_eq!(pixel(&t, 39 + 2, 3 + 2), [255; 4]);
    // Dark: #99c8ff checked, #3b3b3b unchecked in #858585.
    assert_eq!(pixel(&t, 2, 16 + 2), [0x99, 0xc8, 0xff, 255]);
    assert_eq!(pixel(&t, 13 + 6, 16 + 6), [0x3b, 0x3b, 0x3b, 255]);
    assert_eq!(pixel(&t, 13, 16 + 6), [0x85, 0x85, 0x85, 255]);
    // `accent-color`, and a disabled one faded.
    assert_eq!(pixel(&t, 2, 29 + 2), [255, 0, 0, 255]);
    assert_eq!(pixel(&t, 13, 29 + 6), [0xd1, 0xd1, 0xd1, 255]);
}

#[test]
fn drop_shadow_draws_the_alpha_moved_under_the_box_and_later_functions_colour_both() {
    let mut p = paint::Engine::default();
    let s = scene(
        "<div class=a></div><div class=b></div>",
        ".a,.b{width:10px;height:10px;background-color:red}.a{filter:drop-shadow(5px 5px 0 blue)}.b{margin-top:10px;filter:drop-shadow(5px 5px 0 blue) grayscale(1)}",
        &mut p,
    );
    let t = render(&mut p, &s);
    assert_eq!(pixel(&t, 2, 2), [255, 0, 0, 255]);
    // The shadow where the box isn't: moved 5 px right and down.
    assert_eq!(pixel(&t, 12, 12), [0, 0, 255, 255]);
    assert_eq!(pixel(&t, 12, 2), [255; 4]);
    // grayscale() after the shadow greys it and the box alike.
    let grey = pixel(&t, 2, 22);
    assert!(grey[0] == grey[1] && grey[1] == grey[2] && grey[0] < 128, "{grey:?}");
    let shadow = pixel(&t, 12, 32);
    assert!(shadow[0] == shadow[1] && shadow[1] == shadow[2] && shadow[0] < 64, "{shadow:?}");
}

#[test]
fn unstyled_fields_and_buttons_draw_a_thin_rounded_frame_not_their_bevel() {
    let mut p = paint::Engine::default();
    let css = "input,button{display:inline-block;appearance:auto;width:20px;height:16px;margin:0;padding:0;font-size:10px;vertical-align:top}input{border:2px inset #767676;background-color:#fff}button{border:2px outset;color:#000;background-color:#efefef}.styled{border:2px solid red}div{height:20px}";
    let s = scene("<div><input><button></button><input class=styled></div>", css, &mut p);
    let t = render(&mut p, &s);
    // Mid-height on the left edge: the 1 px border, then the field's white
    // where the inset bevel's second pixel was.
    assert_eq!(pixel(&t, 0, 10), [0x76, 0x76, 0x76, 255]);
    assert_eq!(pixel(&t, 1, 10), [255; 4]);
    // The button: its grey inside the border.
    assert_eq!(pixel(&t, 24, 10), [0x76, 0x76, 0x76, 255]);
    assert_eq!(pixel(&t, 25, 10), [0xef, 0xef, 0xef, 255]);
    // Styled: its own border.
    assert_eq!(pixel(&t, 49, 10), [255, 0, 0, 255]);
}

#[test]
fn a_live_refresh_compiles_new_rules_in_the_background() {
    // A sheet the page's scripts changed: with a waker, the refresh starts
    // its compile on a thread of its own and answers that it is under way
    // (the page keeps what it shows); told when it is done, the next
    // refresh takes the rules.
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    let mut page =
        olo_web::page::load(Arc::from("<!doctype html><style id=s>body{margin:0}#d{height:10px}</style><div id=d></div>"), &host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {})
            .unwrap();
    let mut stack = vec![page.document.root()];
    let mut s = None;
    while let Some(n) = stack.pop() {
        if page.document.attribute(n, "id").ok().flatten() == Some("s") {
            s = Some(n);
        }
        stack.extend(page.document.children(n).unwrap());
    }
    let mut doc = page.document.clone();
    let text = doc.children(s.unwrap()).unwrap().next().unwrap();
    doc.set_value(text, "body{margin:0}#d{height:37px}/* a_live_refresh_compiles_new_rules_in_the_background */").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let wake: olo_web::page::Wake = Arc::new(move || drop(tx.send(())));
    let first = page.refresh(&doc, None, &mut |_, _, _| Ok(None), Some(&wake));
    assert_eq!(first.err().map(|e| e.to_string()).as_deref(), Some(olo_web::page::COMPILING_RULES));
    rx.recv_timeout(std::time::Duration::from_secs(10)).expect("told when the rules are ready");
    assert!(page.refresh(&doc, None, &mut |_, _, _| Ok(None), Some(&wake)).unwrap(), "the rules taken");
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    let l = page.scene.layout();
    let height = l.cache().tree().iter().filter_map(|(id, _)| l.bounds(id)).map(|b| b.height).fold(0f64, f64::max);
    assert!(height >= 37., "the new rule holds: {height}");
}

#[test]
fn pictures_not_here_yet_show_when_they_land() {
    // The host answers PENDING for a picture still on its way: the page
    // shows without it (its <img> keeping its attribute size, its
    // background empty) and binds both once they land.
    use olo_web::{image::Image, page};
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let red = Image::from_rgba(1, 1, vec![255, 0, 0, 255], Default::default(), None).unwrap();
    let html = "<!doctype html><style>body{margin:0}div{width:20px;height:20px;background:url(b.png)}img{display:block}</style><img src=a.png width=30 height=10><div></div>";
    let mut shaper = Shaper::default();
    let mut p = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    let mut page = page::load_assets(
        Arc::from(html),
        &host,
        &scope,
        &mut shaper,
        &mut p,
        viewport,
        None,
        |_, _| {},
        |kind, _, _| {
            assert_eq!(kind, page::ResourceKind::Image);
            Err(page::PENDING.into())
        },
    )
    .unwrap();
    assert!(page.has_late_images());
    let px = |page: &page::Page, p: &mut paint::Engine, x: usize, y: usize| -> [u8; 4] {
        let t = p.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: 100, height: 100 }, None).unwrap().into_rgba();
        t[(y * 100 + x) * 4..][..4].try_into().unwrap()
    };
    assert_eq!(px(&page, &mut p, 5, 5), [255; 4], "the picture's room, empty");
    assert_eq!(px(&page, &mut p, 5, 15), [255; 4], "the background, empty: the div starts below the img's 10px");
    // Still on their way: nothing bound.
    let doc = page.document.clone();
    assert!(!page.bind_late(&doc, None, &mut |_, _, _| Err(page::PENDING.into())).unwrap());
    assert_eq!(page.late_images(), 2);
    // Landed.
    assert!(page.bind_late(&doc, None, &mut |_, _, _| Ok(Some(page::Resource::Image(red.clone())))).unwrap());
    assert!(!page.has_late_images());
    page.rerender(doc, Vec::new(), &host, &mut shaper, &mut p, viewport, None, |_, _| {}).unwrap();
    assert_eq!(px(&page, &mut p, 5, 5), [255, 0, 0, 255], "the picture");
    assert_eq!(px(&page, &mut p, 5, 15), [255, 0, 0, 255], "the background");
}

#[test]
fn pictures_styles_name_later_are_noted_where_the_styles_changed() {
    use olo_web::page;
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    // Enough elements for the style tree's several chunks; the last ones
    // change, so the change is in a chunk of its own.
    let html = format!("<!doctype html><style>:root{{--c:url(c.png)}}div{{height:2px}}</style>{}<div id=x></div><div id=y></div>", "<div></div>".repeat(150));
    let mut shaper = Shaper::default();
    let mut p = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    let mut page = page::load(Arc::from(html), &host, &scope, &mut shaper, &mut p, viewport, None, |_, _| {}).unwrap();
    assert!(!page.note_pictures(), "none named");
    let find = |page: &page::Page, id: &str| {
        let mut stack = vec![page.document.root()];
        while let Some(n) = stack.pop() {
            if page.document.attribute(n, "id").ok().flatten() == Some(id) {
                return n;
            }
            stack.extend(page.document.children(n).unwrap());
        }
        panic!("{id}")
    };
    let mut set = |page: &mut page::Page, id: &str, style: &str| {
        let node = find(page, id);
        let mut doc = page.document.clone();
        doc.set_attribute(node, "style", style).unwrap();
        page.rerender(doc, Vec::new(), &host, &mut shaper, &mut p, viewport, None, |_, _| {}).unwrap();
    };
    // A script names one through a custom property.
    set(&mut page, "x", "background-image:var(--c)");
    assert!(page.note_pictures(), "named through var()");
    assert_eq!(page.late_images(), 1);
    assert!(!page.note_pictures(), "the same tree: not looked in again");
    // Another element changes, naming none.
    set(&mut page, "y", "color:red");
    assert!(!page.note_pictures());
    // Then one names another picture.
    set(&mut page, "y", "mask-image:url(d.png)");
    assert!(page.note_pictures(), "a second picture");
    assert_eq!(page.late_images(), 2);
}
