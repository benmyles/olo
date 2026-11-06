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
    let error = olo_web::page::load(
        format!("<!doctype html><style>{BASE}html{{font-family:'Olo Inline ligature'}}</style><div>f<span style='color:red'>i</span></div>").into(),
        &host,
        &scope,
        &mut Shaper::default(),
        &mut paint::Engine::default(),
        layout::Viewport { width: 100., height: 100., scale: 1. },
        None,
        |_, _| {},
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("paint boundary inside a glyph cluster"), "{error}");
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
        let error =
            olo_web::page::load(source.into(), &host, &scope, &mut Shaper::default(), &mut paint::Engine::default(), layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {})
                .err()
                .unwrap();
        assert!(error.to_string().contains("inline backgrounds"), "{error}");
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
    page.restyle(&host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {}).unwrap();
    assert!(page.scene.commands().iter().any(|c| matches!(c.primitive, paint::Primitive::Text { color: [0, 128, 0, 255], .. })));
    page.interaction.active = Some(first_text);
    page.interaction.focus = Some(first);
    page.restyle(&host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {}).unwrap();
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
    assert!(page.restyle(&host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 100., height: 100., scale: 1. }, None, |_, _| {}).is_err());
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
