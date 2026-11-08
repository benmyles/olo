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
#[test]
fn fixed_boxes_take_the_viewport_unless_a_transform_holds_them() {
    // GitHub's menu panel: `position: fixed; height: 100%` inside a short
    // positioned header is the viewport's height, at the viewport's top.
    let p = load(
        ".h{position:relative;height:52px}#m{position:fixed;top:0;width:100%;height:100%}#n{position:fixed;top:0;width:50%}#t{transform:translateX(0);position:static;margin-top:10px;width:100px;height:40px}#k{position:fixed;top:0;height:100%;width:10px}</style><div class=h><div id=m></div></div><div class=h><div id=n><div style='height:30px'></div></div></div><div id=t><div id=k></div></div>",
        1.,
    );
    assert_eq!(rect(&p, "m"), layout::Rect { x: 0., y: 0., width: 400., height: 300. });
    assert_eq!(rect(&p, "n"), layout::Rect { x: 0., y: 0., width: 200., height: 30. });
    // A transformed ancestor is its containing block.
    let t = rect(&p, "t");
    assert_eq!(rect(&p, "k"), layout::Rect { x: 0., y: t.y, width: 10., height: 40. });
}
#[test]
fn a_flex_containers_bare_text_follows_its_align_items() {
    // GitHub's menu buttons: `display:flex; align-items:center` with the
    // label as bare text beside an icon.
    let p = load(
        "#f{display:flex;align-items:center;justify-content:space-between;height:60px;line-height:20px}#i{width:10px;height:10px}#e{display:flex;align-items:flex-end;height:60px;line-height:20px}</style><div id=f>Label<div id=i></div></div><div id=e>Low</div>",
        1.,
    );
    let l = p.scene.layout();
    let text_of = |name: &str| {
        let tree = l.cache().tree();
        let (id, _) = tree.iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::InlineContent { .. }) && p.document.attribute(b.node, "id").unwrap() == Some(name)).unwrap();
        l.bounds(id).unwrap()
    };
    assert_eq!(text_of("f").y, 20.);
    assert_eq!(rect(&p, "i").y, 25.);
    assert_eq!(text_of("e").y, 60. + 40.);
}
#[test]
fn column_flex_auto_margins_follow_a_height_given_by_the_parent() {
    // GitHub's menu: a grown column wrapper whose last item has
    // `margin-top: auto` sits at the wrapper's bottom; a grid's stretched
    // column card puts its auto-margin button at the card's bottom.
    let p = load(
        "#m{display:flex;flex-direction:column;height:300px}#h{height:50px;flex-shrink:0}#w{display:flex;flex-direction:column;flex-grow:1}#n{height:100px;flex-shrink:0}#c{margin-top:auto;height:40px;flex-shrink:0;width:50px;margin-left:auto;margin-right:auto}#g{display:grid;grid-template-columns:1fr 1fr}.card{display:flex;flex-direction:column}.b{margin-top:auto;height:20px}</style><div id=m><div id=h></div><div id=w><div id=n></div><div id=c></div></div></div><div id=g><div class=card><div style='height:50px'></div><div class=b id=b1></div></div><div class=card><div style='height:150px'></div><div class=b id=b2></div></div></div>",
        1.,
    );
    assert_eq!(rect(&p, "w"), layout::Rect { x: 0., y: 50., width: 400., height: 250. });
    assert_eq!(rect(&p, "c"), layout::Rect { x: 175., y: 260., width: 50., height: 40. });
    assert_eq!(rect(&p, "b1").y, 300. + 150.);
    assert_eq!(rect(&p, "b2").y, 300. + 150.);
}
#[test]
fn a_transparent_box_over_a_painted_one_takes_the_hit() {
    // GitHub's menu rows: a background-less button across a bordered row
    // is what a click anywhere on the row reaches, not only its label.
    let p = load(
        "#row{background-color:#eee;border-bottom:1px solid gray;padding:5px}#b{display:flex;height:40px;width:300px}#t{position:absolute;left:350px;top:0;width:20px;height:20px}</style><div id=row><div id=b>Label</div><div id=t></div></div>",
        1.,
    );
    let id = |node: Option<olo_web::dom::NodeId>| node.and_then(|n| p.document.attribute(n, "id").unwrap().map(str::to_owned));
    assert_eq!(id(p.scene.hit_test(250., 25., 0., 300., &[])), Some("b".into()));
    assert_eq!(id(p.scene.hit_test(320., 25., 0., 300., &[])), Some("row".into()));
    // Positioned boxes are not entered from their parent's background.
    assert_eq!(id(p.scene.hit_test(355., 10., 0., 300., &[])), Some("row".into()));
}
#[test]
fn text_box_trims_the_first_and_last_lines_to_their_edges() {
    let p = load(
        "div{font-size:20px;line-height:40px}#a{text-box:trim-both cap alphabetic}#b{text-box:trim-start cap}#c{text-box-trim:trim-end;text-box-edge:text alphabetic}#d{text-box:trim-both text}#e{text-box:normal}</style><div id=a>Hx</div><div id=b>Hx</div><div id=c>Hx<br>Hx</div><div id=d>Hx</div><div id=e>Hx</div><div id=f>Hx</div>",
        1.,
    );
    let h = |id: &str| rect(&p, id).height;
    let full = h("f");
    assert_eq!(full, 40.);
    // Cap height to baseline: shorter than the font's ascent.
    assert!(h("a") > 0. && h("a") < 20., "{}", h("a"));
    // `cap` alone is no edge (one value must suit both sides): ignored, as Chrome does.
    assert_eq!(h("b"), full);
    // The last line ends at its baseline: its descent and half-leading go.
    assert!(h("c") < 2. * full && h("c") > full + 20., "{}", h("c"));
    // `text`: the font's ascent and descent, without the half-leading.
    assert!(h("d") < full && h("d") >= 20., "{}", h("d"));
    assert_eq!(h("e"), full);
    let computed = |id: &str, property| {
        let tree = p.scene.layout().cache().tree();
        let (box_id, _) = tree.iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::Block { .. }) && p.document.attribute(b.node, "id").unwrap() == Some(id)).unwrap();
        page::computed_value(tree.style(box_id).unwrap(), property)
    };
    use olo_web::css::properties::Property as P;
    assert_eq!(computed("a", P::TextBoxTrim).as_deref(), Some("trim-both"));
    assert_eq!(computed("a", P::TextBoxEdge).as_deref(), Some("cap alphabetic"));
    assert_eq!(computed("b", P::TextBoxEdge).as_deref(), Some("auto"));
}
#[test]
fn computed_line_height_and_font_family_read_as_chrome_reports_them() {
    let p = load("#a{font:16px/1.5 'Olo Inline plain',\"My Font\",sans-serif}</style><div id=a>x</div>", 1.);
    let tree = p.scene.layout().cache().tree();
    let (box_id, _) = tree.iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::Block { .. }) && p.document.attribute(b.node, "id").unwrap() == Some("a")).unwrap();
    let style = tree.style(box_id).unwrap();
    use olo_web::css::properties::Property as P;
    assert_eq!(page::computed_value(style, P::LineHeight).as_deref(), Some("24px"));
    assert_eq!(page::computed_value(style, P::FontFamily).as_deref(), Some("\"Olo Inline plain\", \"My Font\", sans-serif"));
}
#[test]
fn scroll_containers_move_what_they_hold_within_their_padding_box() {
    let scale = 1.;
    let mut p = load(
        "#s{overflow:auto;height:100px;width:200px}#s div{height:40px}#f{position:fixed;top:0;left:300px;width:10px;height:500px;background-color:red}#h{overflow:hidden;height:20px}#h div{height:300px}</style><div id=s><div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=f></div><div id=h><div></div></div></div>",
        scale,
    );
    let node = |p: &page::Page, id: &str| p.scene.layout().cache().tree().iter().find(|(_, b)| p.document.attribute(b.node, "id").ok().flatten() == Some(id)).unwrap().1.node;
    let s = node(&p, "s");
    let sc = *p.scene.scrollers().iter().find(|x| x.node == s).unwrap();
    // Five 40 px rows (the clipping box's height is `#s div`'s): 200 px;
    // the fixed box and what the inner box clips count for nothing.
    assert_eq!(sc.content, [200., 200.]);
    assert_eq!(sc.max(), [0., 100.]);
    assert_eq!(sc.user, [true, true]);
    assert_eq!(p.scroll_box(s, [0., 50.]), Some([0., 50.]));
    assert_eq!(p.scroll_box(s, [0., 500.]), Some([0., 100.]));
    assert_eq!(p.scroll_box(s, [0., 5.]), None);
    let host = paint_host::Scaled::new(p.scope().clone(), scale);
    p.repaint(&host, &mut paint::Engine::default(), None).unwrap();
    // The third row (80 to 120 in the box) now shows at its top; a click there finds it.
    let id = |n: Option<olo_web::dom::NodeId>| n.and_then(|n| p.document.attribute(n, "id").unwrap().map(str::to_owned));
    let mut painter = paint::Engine::default();
    let tile = painter.raster(&p.scene, paint::DeviceRect { x: 0, y: 0, width: 400, height: 300 }, None).unwrap();
    assert_eq!(&tile.rgba()[(5 * 400 + 305) * 4..][..4], &[255, 0, 0, 255]);
    assert_eq!(id(p.scene.hit_test(10., 5., 0., 300., &[])), Some("c".into()));
    assert_eq!(p.scene.scrollers().iter().find(|x| x.node == s).unwrap().offset, [0., 100.]);
    // The fixed box stays where the viewport has it.
    assert_eq!(id(p.scene.hit_test(305., 5., 0., 300., &[])), Some("f".into()));
}
#[test]
fn container_queries_ask_the_nearest_container_of_their_name() {
    let p = load(
        ".c{container-type:inline-size;width:200px}.n{container:signup/inline-size;width:300px}.x{height:1px}
        @container (width > 150px){.a{height:2px}}
        @container (width < 150px){.b{height:3px}}
        @container signup (width >= 17.5rem){.s{height:4px}}
        @container other (width > 1px){.o{height:5px}}
        @container (min-width: 250px){@container (max-width: 350px){.nest{height:6px}}}
        </style><div class=c><div class='x a' id=a></div><div class='x b' id=b></div><div class='x s' id=s1></div></div><div class=n><div class=c><div class='x s' id=s2></div></div><div class='x o' id=o></div><div class='x nest' id=nest></div></div><div class='x a' id=out></div>",
        1.,
    );
    let h = |id: &str| rect(&p, id).height;
    // Unnamed queries ask the nearest container; named ones skip past
    // others to theirs; a name no container has holds nowhere; nested
    // conditions must all hold; outside any container nothing holds.
    assert_eq!([h("a"), h("b"), h("s1"), h("s2"), h("o"), h("nest"), h("out")], [2., 1., 1., 4., 1., 6., 1.]);
}
#[test]
fn an_item_spanning_minmax_zero_fr_tracks_does_not_grow_them() {
    // BBC's 24-column grid (`repeat(24, minmax(0, 1fr))`): its carousel
    // spans every column and holds a wide scroller; the columns stay
    // their share, as `1fr` (an `auto` minimum) would not.
    let p = load(
        "#o{display:grid;grid-template-columns:repeat(24,minmax(0,1fr));column-gap:4px;width:300px}#g{grid-column:1/span 24}#s{display:flex;overflow-x:scroll}#s div{width:200px;flex-shrink:0;height:10px}#a{display:grid;grid-template-columns:repeat(2,1fr);width:100px}#w{grid-column:1/span 2;width:300px;height:5px}</style><div id=o><div id=g><div id=s><div></div><div></div><div></div></div></div></div><div id=a><div id=w></div></div>",
        1.,
    );
    // (24 columns of 8.67 px, each on the device grid.)
    assert!((rect(&p, "g").width - 300.).abs() < 1., "{:?}", rect(&p, "g"));
    assert!((rect(&p, "s").width - 300.).abs() < 1.);
    // `1fr` tracks do grow for it (CSS Grid §11.5): 150 px each.
    assert_eq!(rect(&p, "a").width, 100.);
    assert_eq!(rect(&p, "w").width, 300.);
}
