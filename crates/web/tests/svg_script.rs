//! SVG geometry for scripts against Chrome 157's answers on the same SVG
//! (getTotalLength, getPointAtLength, getBBox, getCTM, getScreenCTM).
use olo_web::{html, svg::script};
use std::sync::Arc;
fn page() -> (olo_web::dom::Document, impl Fn(&olo_web::dom::Document, &str) -> olo_web::dom::NodeId) {
    let source = r#"<svg width=200 height=100 viewBox="0 0 100 50"><g id=g transform="translate(10,5)"><circle id=c cx=20 cy=20 r=10 /><rect id=r x=1 y=2 width=30 height=10 rx=3 /><path id=p d="M0 0 L30 40 Q 50 0 60 40" /></g></svg>"#;
    let parsed = html::parse(Arc::from(source), Default::default(), None).unwrap();
    let find = |doc: &olo_web::dom::Document, id: &str| {
        let mut stack = vec![doc.root()];
        while let Some(n) = stack.pop() {
            if doc.attribute(n, "id").ok().flatten() == Some(id) {
                return n;
            }
            let mut c = doc.first_child(n).unwrap();
            while let Some(child) = c {
                stack.push(child);
                c = doc.next_sibling(child).unwrap();
            }
        }
        panic!("{id}")
    };
    (parsed.document, find)
}
#[test]
fn lengths_and_points_are_chromes() {
    let (doc, find) = page();
    let f32_of = |v: f64| v as f32;
    // Skia's measure for shapes, to the bit.
    assert_eq!(f32_of(script::total_length(&doc, find(&doc, "c")).unwrap()), 62.426971435546875);
    assert_eq!(f32_of(script::total_length(&doc, find(&doc, "r")).unwrap()), 74.36880493164062);
    let (x, y) = script::point_at_length(&doc, find(&doc, "c"), 1.).unwrap();
    assert_eq!((x as f32, y as f32), (29.953807830810547, 20.960065841674805));
    // Path data, the true length.
    assert_eq!(f32_of(script::total_length(&doc, find(&doc, "p")).unwrap()), 102.51518249511719);
    let (x, y) = script::point_at_length(&doc, find(&doc, "p"), 70.).unwrap();
    assert!((x - 40.93567657470703).abs() < 1e-4 && (y - 23.356332778930664).abs() < 1e-4, "{x} {y}");
    // Past the end, the end; before the start, the start.
    assert_eq!(script::point_at_length(&doc, find(&doc, "p"), 1e9).unwrap(), (60., 40.));
    assert_eq!(script::point_at_length(&doc, find(&doc, "p"), -5.).unwrap(), (0., 0.));
}
#[test]
fn boxes_and_matrices_are_chromes() {
    let (doc, find) = page();
    let mut no_text = |_| None;
    let b = |id: &str, t: &mut dyn FnMut(olo_web::dom::NodeId) -> Option<kurbo::Rect>| {
        let r = script::bbox(&doc, find(&doc, id), t).unwrap();
        [r.x0, r.y0, r.width(), r.height()]
    };
    assert_eq!(b("c", &mut no_text), [10., 10., 20., 20.]);
    assert_eq!(b("r", &mut no_text), [1., 2., 30., 10.]);
    assert_eq!(b("p", &mut no_text), [0., 0., 60., 40.]);
    assert_eq!(b("g", &mut no_text), [0., 0., 60., 40.]);
    let svg = doc.parent(find(&doc, "g")).unwrap().unwrap();
    assert_eq!(script::bbox(&doc, svg, &mut no_text).map(|r| [r.x0, r.y0, r.width(), r.height()]), Some([10., 5., 60., 40.]));
    // The viewBox scales by 2; the svg's box at (8, 8) in the page.
    let mut placed = |_| Some([8., 8., 200., 100.]);
    assert_eq!(script::ctm(&doc, find(&doc, "c"), false, &mut placed).unwrap().as_coeffs(), [2., 0., 0., 2., 20., 10.]);
    assert_eq!(script::ctm(&doc, find(&doc, "c"), true, &mut placed).unwrap().as_coeffs(), [2., 0., 0., 2., 28., 18.]);
}
