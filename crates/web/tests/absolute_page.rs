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
#[test]
fn an_inline_level_absolute_box_takes_its_place_in_its_line() {
    // apple.com's iPhone Duo video: `position:absolute; transform:
    // translate(-50%)` and no inset, in centred text: its static position
    // is the middle of its line. A line of nothing but such a box is no
    // line box (zero tall); the paragraph isn't split around one. As
    // Chrome: a at (200, 0), c at (400, 18), the second paragraph one line.
    let p = load(
        "#w{position:relative;width:400px;text-align:center;line-height:18px}#a{position:absolute;width:50px;height:50px}#b{position:absolute;display:inline-block;width:20px;height:20px}#c{position:absolute;width:30px;height:30px}#d{position:absolute;display:block;width:10px;height:10px}</style><div id=w><div id=p1><span id=a></span></div><div id=p2>ab <span id=b></span> cd</div><div id=p3 style='text-align:right'><span id=c></span></div><div id=p4>x<div id=d></div></div></div>",
        1.,
    );
    assert_eq!(rect(&p, "p1").height, 0.);
    assert_eq!((rect(&p, "a").x, rect(&p, "a").y), (200., 0.));
    assert_eq!(rect(&p, "p2").height, 18.);
    let b = rect(&p, "b");
    assert!(b.x > 190. && b.x < 210. && b.y == 0., "{b:?}");
    assert_eq!((rect(&p, "c").x, rect(&p, "c").y), (400., 18.));
    // A block-level one keeps a block's static position: after the line.
    assert_eq!((rect(&p, "d").x, rect(&p, "d").y), (0., 36.));
}
#[test]
fn an_svg_with_only_a_viewbox_fills_the_width_at_its_ratio() {
    // apple.com's Music, Fitness+ and Arcade logos: `<svg viewBox>` in a
    // 64 × 20 box is 64 wide (Chrome 63.95 × 15.09), not the 300 × 150
    // default; one with a width keeps it; one without a viewBox is 300 × 150.
    let p = load(
        ".l{width:64px;height:20px}</style><div class=l><svg id=a viewBox='0 0 339 80'></svg></div><div style='width:200px'><svg id=b viewBox='0 0 100 50'></svg><svg id=c width=40 viewBox='0 0 100 50'></svg></div><svg id=d></svg>",
        1.,
    );
    let r = |name: &str| {
        let l = p.scene.layout();
        let id = l
            .cache()
            .tree()
            .iter()
            .find(|(_, b)| matches!(b.kind, layout::BoxKind::InlineVector { .. } | layout::BoxKind::BlockVector { .. }) && p.document.attribute(b.node, "id").unwrap() == Some(name))
            .unwrap()
            .0;
        l.bounds(id).unwrap()
    };
    assert_eq!((r("a").width, (r("a").height * 100.).round() / 100.), (64., 15.09));
    assert_eq!((r("b").width, r("b").height), (200., 100.));
    assert_eq!((r("c").width, r("c").height), (40., 20.));
    assert_eq!((r("d").width, r("d").height), (300., 150.));
}
#[test]
fn what_takes_no_pointer_events_passes_the_point_to_the_box_it_is_in() {
    // apple.com's nav: links holding `pointer-events: none` spans and
    // SVGs, painted over an open flyout's background (absolute, earlier in
    // the tree), under a `pointer-events: none` scrim (`body::before`).
    let p = load(
        "body::before{content:'';position:fixed;top:0;left:0;width:400px;height:40px;background:#eee;pointer-events:none;z-index:0}nav{position:fixed;top:0;left:0;width:400px;height:40px;z-index:1}#f{position:absolute;top:0;left:0;width:400px;height:200px;background:#fff}a{position:relative;display:inline-block;margin-left:100px}#s{pointer-events:none}#v{pointer-events:none;display:block}</style><nav><div id=f></div><a id=l href=#><span id=s><svg id=v width=30 height=40></svg></span></a><a id=t href=#><span style=pointer-events:none>Text</span></a></nav>",
        1.,
    );
    let hit = |x: f64, y: f64| {
        let n = p.scene.hit_test(x, y, 0., 300., &[]).unwrap();
        p.document.attribute(n, "id").unwrap().map(str::to_owned)
    };
    assert_eq!(hit(110., 20.), Some("l".into()), "the link, not the flyout under it");
    assert_eq!(hit(50., 20.), Some("f".into()), "the flyout, not the scrim over it");
    assert_eq!(hit(50., 100.), Some("f".into()));
}
#[test]
fn a_fixed_table_takes_its_first_rows_widths() {
    // GitHub's file list: table-layout: fixed, a hidden colspan=2 cell,
    // then 40%, auto and 136 px (Chrome: 272.39, 272.61, 136 at 2, 276.39,
    // 551 in a 689 px table).
    let p = load(
        "table{table-layout:fixed;width:689px}th,td{padding:0}.a{width:100%;display:none}.b{width:40%}.d{width:136px}.small{display:none}</style><table><thead><tr><th class=a colspan=2>A</th><th id=b class=b>B</th><th id=c>C</th><th id=d class=d>D</th></tr></thead><tbody><tr><td class=small colspan=2>s</td><td id=n>name</td><td id=m>a message long enough to need room</td><td id=t>date</td></tr></tbody></table>",
        1.,
    );
    let w = |id: &str| {
        let r = rect(&p, id);
        ((r.x * 100.).round() / 100., (r.width * 100.).round() / 100.)
    };
    assert_eq!(w("b"), (2., 272.39));
    assert_eq!(w("c"), (276.39, 272.61));
    assert_eq!(w("d"), (551., 136.));
    assert_eq!(w("m"), w("c"), "the rows follow the first");
}
#[test]
fn cells_fill_their_row_and_align_their_content_in_it() {
    // GitHub's file list: its row lines (cells' borders) fell apart when a
    // middle-aligned cell's box moved instead of its content (Chrome: both
    // cells 0 + 43, the small one's text in the middle).
    let p = load(
        "table{border-spacing:0;width:300px}td{border-top:1px solid red;padding:4px;vertical-align:middle;line-height:18px}.big{font-size:30px;line-height:34px}</style><table><tr><td id=a>small</td><td id=b class=big>Big</td></tr></table>",
        1.,
    );
    let (a, b) = (rect(&p, "a"), rect(&p, "b"));
    assert_eq!((a.y, a.height), (b.y, b.height));
    let l = p.scene.layout();
    let text = l.cache().tree().iter().find(|(_, x)| matches!(x.kind, layout::BoxKind::InlineContent { .. }) && p.document.attribute(x.node, "id").unwrap() == Some("a")).unwrap().0;
    let t = l.bounds(text).unwrap();
    assert!((t.y - (a.y + (a.height - 18.) / 2.)).abs() < 1., "{t:?} in {a:?}");
}
#[test]
fn atomic_inlines_align_to_the_line_box_top_bottom_and_middle() {
    // Chrome's numbers: a 40 px inline-block sets a line of 45; the 10 px
    // boxes top at 0, bottom at 35; a second line whose top-aligned 40 px
    // box makes it 40 tall.
    let p = load(
        "body{line-height:20px}p{margin:0 0 10px}span.b{display:inline-block;width:20px;height:40px}span.s{display:inline-block;width:10px;height:10px}</style><p id=p1>x<span class=b id=b1></span><span class=s id=t style='vertical-align:top'></span>y<span class=s id=o style='vertical-align:bottom'></span>z</p><p id=p2>text <span class=b id=b2 style='vertical-align:top'></span> end</p>",
        1.,
    );
    let y = |id: &str| rect(&p, id).y;
    assert_eq!(rect(&p, "p1").height, 45.);
    assert_eq!(y("t"), 0.);
    assert_eq!(y("o"), 35.);
    assert_eq!(rect(&p, "p2").height, 40.);
    assert_eq!(y("b2"), y("p2"));
}
/// An absolutely positioned box with an auto width, a definite height and
/// an `aspect-ratio` takes the width the ratio gives (CSS Sizing 4 §5.1),
/// not its content's (Wikipedia's play button was 0 wide).
#[test]
fn an_absolute_box_with_a_ratio_takes_its_width_from_its_height() {
    for scale in [1., 1.5, 2.] {
        let p = load(
            "#cb{position:relative;width:250px;height:160px}#b{position:absolute;left:50%;top:50%;width:auto;height:min(50px,70%);aspect-ratio:1/1}#c{position:absolute;left:0;top:0;height:20px;aspect-ratio:2;padding:0 5px;box-sizing:border-box}</style><div id=cb><div id=b></div><div id=c></div></div>",
            scale,
        );
        let b = rect(&p, "b");
        assert_eq!((b.width, b.height), (50., 50.), "at {scale}");
        assert_eq!(rect(&p, "c").width, 40., "border-box ratio at {scale}");
    }
}
/// An absolutely positioned child of a flex container, its insets auto,
/// sits as the container's sole item would (CSS Flexbox 1 §4.1): centred
/// by `justify-content` and `align-items`, at the end by `flex-end`, in a
/// row or a column (Reddit's loading ring sat at the start).
#[test]
fn an_absolute_flex_child_sits_as_the_sole_item_would() {
    for scale in [1., 1.5, 2.] {
        let p = load(
            "#r{display:flex;justify-content:center;align-items:center;position:relative;width:200px;height:100px}#a{position:absolute;width:40px;height:20px}\
             #c{display:flex;flex-direction:column;justify-content:flex-end;align-items:flex-end;position:relative;width:100px;height:60px}#b{position:absolute;width:30px;height:10px}\
             #s{display:flex;justify-content:center;position:relative;width:100px;height:50px}#t{position:absolute;left:5px;width:10px;height:10px}</style>\
             <div id=r><div id=a></div></div><div id=c><div id=b></div></div><div id=s><div id=t></div></div>",
            scale,
        );
        let (r, a) = (rect(&p, "r"), rect(&p, "a"));
        assert_eq!((a.x - r.x, a.y - r.y), (80., 40.), "centred at {scale}");
        let (c, b) = (rect(&p, "c"), rect(&p, "b"));
        assert_eq!((b.x - c.x, b.y - c.y), (70., 50.), "at the ends at {scale}");
        let (s, t) = (rect(&p, "s"), rect(&p, "t"));
        assert_eq!(t.x - s.x, 5., "an inset set: as it says, at {scale}");
    }
}
#[test]
fn clip_rect_hides_an_absolute_box_and_takes_its_hits_away() {
    // `clip: rect()` (CSS 2.1 §11.1.2) on absolutely positioned boxes only:
    // a skip link `rect(0 0 0 0)` shows nothing and takes no clicks; a
    // partial rect keeps its part (auto: that edge of the border box).
    let p = load(
        "#skip{position:absolute;left:10px;top:10px;width:1px;height:1px;padding:10px;overflow:hidden;background-color:red;clip:rect(0 0 0 0)}#part{position:absolute;left:100px;top:0;width:100px;height:60px;background-color:blue;clip:rect(10px,80px,auto,20px)}#flow{width:50px;height:50px;margin-top:100px;background-color:green;clip:rect(0 0 0 0)}</style><div id=skip>Skip</div><div id=part></div><div id=flow></div>",
        1.,
    );
    let id = |node: Option<olo_web::dom::NodeId>| node.and_then(|n| p.document.attribute(n, "id").unwrap().map(str::to_owned));
    assert_ne!(id(p.scene.hit_test(15., 15., 0., 300., &[])), Some("skip".into()));
    assert_eq!(id(p.scene.hit_test(150., 30., 0., 300., &[])), Some("part".into()));
    assert_ne!(id(p.scene.hit_test(110., 30., 0., 300., &[])), Some("part".into()));
    assert_ne!(id(p.scene.hit_test(150., 5., 0., 300., &[])), Some("part".into()));
    assert_ne!(id(p.scene.hit_test(190., 30., 0., 300., &[])), Some("part".into()));
    assert_eq!(id(p.scene.hit_test(150., 59., 0., 300., &[])), Some("part".into()));
    // Not positioned: clip doesn't apply.
    assert_eq!(id(p.scene.hit_test(20., 120., 0., 300., &[])), Some("flow".into()));
}
