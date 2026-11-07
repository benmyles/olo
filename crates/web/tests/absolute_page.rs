use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
use std::{path::Path, sync::Arc};
#[path = "support/paint_host.rs"]
mod paint_host;
fn load(body: &str, scale: f64) -> page::Page {
    load_viewport(body, layout::Viewport { width: 400., height: 300., scale })
}
fn load_viewport(body: &str, viewport: layout::Viewport) -> page::Page {
    let scale = viewport.scale;
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = paint_host::Scaled::new(scope.clone(), scale);
    page::load(
        Arc::from(format!("<!doctype html><style>html,body{{margin:0}}body{{font-family:'Olo Inline plain'}}{body}")),
        &host,
        &scope,
        &mut shaping::Engine::default(),
        &mut paint::Engine::default(),
        viewport,
        None,
        |_, _| {},
    )
    .unwrap()
}
fn rect(p: &page::Page, name: &str) -> layout::Rect {
    let l = p.scene.layout();
    let id = l.cache().tree().iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::Block { .. }) && p.document.attribute(b.node, "id").unwrap() == Some(name)).unwrap().0;
    l.bounds(id).unwrap()
}
#[test]
fn positioned_blocks_wait_for_padding_containing_block_and_do_not_size_flow() {
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let p = load(
            "#cb{position:relative;width:200px;padding:10px;border:2px solid red}#a{position:absolute;left:5px;right:15px;top:50%;height:20px}#flow{height:100px}</style><div id=cb><div id=a></div><div id=flow></div></div><div id=after style='height:10px'></div>",
            scale,
        );
        let b = (2. * scale).floor() / scale;
        assert_eq!(rect(&p, "cb"), layout::Rect { x: 0., y: 0., width: 220. + 2. * b, height: 120. + 2. * b });
        let a = rect(&p, "a");
        assert!((a.x - b - 5.).abs() < 1e-9 && (a.y - b - 60.).abs() < 1e-9);
        assert_eq!((a.width, a.height), (200., 20.));
        assert_eq!(rect(&p, "after").y, 120. + 2. * b);
    }
}
#[test]
fn positioned_nested_descendants_and_auto_content_height() {
    let p = load(
        "#a{position:absolute;left:20px;top:30px;width:100px;padding:5px}#b{position:absolute;right:3px;bottom:4px;width:10px;height:7px}#flow{height:40px}</style><div id=a><div id=b></div><div id=flow></div></div>",
        1.,
    );
    assert_eq!(rect(&p, "a"), layout::Rect { x: 20., y: 30., width: 110., height: 50. });
    assert_eq!(rect(&p, "b"), layout::Rect { x: 117., y: 69., width: 10., height: 7. });
}
#[test]
fn absolute_flex_child_does_not_participate_in_flex_distribution() {
    let p = load(
        "#cb{display:flex;position:relative;width:200px;height:100px}#a{position:absolute;left:0;top:0;width:150px;height:20px}#flow{width:60px;height:30px;flex-shrink:0}</style><div id=cb><div id=a></div><div id=flow></div></div>",
        1.,
    );
    assert_eq!(rect(&p, "flow").width, 60.);
    assert_eq!(rect(&p, "flow").x, 0.);
    assert_eq!(rect(&p, "a").width, 150.);
}

#[test]
fn positioned_paint_escapes_intermediate_static_overflow_clip() {
    let p = load(
        "#cb{position:relative;width:100px;height:100px}#clip{overflow:hidden;width:10px;height:10px}#a{position:absolute;left:50px;top:50px;width:20px;height:20px;background-color:red}</style><div id=cb><div id=clip><div id=a></div></div></div>",
        1.,
    );
    let mut painter = paint::Engine::default();
    let tile = painter.raster(&p.scene, paint::DeviceRect { x: 0, y: 0, width: 100, height: 100 }, None).unwrap();
    assert_eq!(&tile.rgba()[(55 * 100 + 55) * 4..][..4], &[255, 0, 0, 255]);
}

#[test]
fn independent_absolute_page_rectangles() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/absolute-page-fixtures");
    let bytes = std::fs::read(root.join("cases.json")).unwrap();
    let cases: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let reference: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("chrome.json")).unwrap()).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(bytes)), reference["cases_sha256"]);
    for row in reference["rows"].as_array().unwrap() {
        let c = cases.as_array().unwrap().iter().find(|c| c["id"] == row["id"]).unwrap();
        let p = load_viewport(
            &format!("{}</style><div id=probe>{}</div>", c["css"].as_str().unwrap(), c["html"].as_str().unwrap()),
            layout::Viewport { width: row["viewport"][0].as_f64().unwrap(), height: row["viewport"][1].as_f64().unwrap(), scale: row["scale"].as_f64().unwrap() },
        );
        assert_eq!(p.stylesheet_issues, 0, "{}", c["id"]);
        for (id, expected) in row["rects"].as_object().unwrap() {
            let r = rect(&p, id);
            for (key, value) in [("x", r.x), ("y", r.y), ("width", r.width), ("height", r.height)] {
                let e = expected[key].as_f64().unwrap();
                assert!((value - e).abs() < 0.0001, "{} @{} {id}/{key}: {value} != {e}", c["id"], row["scale"]);
            }
        }
    }
}
