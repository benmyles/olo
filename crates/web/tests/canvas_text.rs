//! Canvas text: the `font` string as a request, and a string shaped into
//! outlines with the metrics measureText gives.
use olo_web::{
    canvas_text,
    css::style::FontSlant,
    font::{Builder, FontSet, GenericPolicy, Scope},
    text::shaping::Engine,
};
use std::path::Path;
fn scope() -> Scope {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut b = Builder::new(Default::default());
    b.load_file(&root.join("data/fonts/NotoSans[wght].ttf"), |_| {}, None).unwrap();
    FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans"]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
}
#[test]
fn a_canvas_font_string_is_the_css_font_shorthand() {
    let r = canvas_text::font_request("bold italic 20px 'Noto Sans', sans-serif").unwrap();
    assert_eq!((r.size, r.weight, r.slant), (20., 700., FontSlant::Italic));
    assert_eq!(r.families.as_slice().len(), 2);
    assert_eq!(canvas_text::font_request("12pt serif").unwrap().size, 16.);
    assert_eq!(canvas_text::font_request("2em serif").unwrap().size, 20.);
    assert_eq!(canvas_text::font_request("300 large monospace").unwrap().weight, 300.);
    // What isn't a CSS font (or is a CSS-wide keyword) is refused: the
    // context keeps the font it had.
    for bad in ["20px", "bold", "nonsense", "inherit", "", "20px serif !important"] {
        assert!(canvas_text::font_request(bad).is_none(), "{bad}");
    }
}
#[test]
fn text_is_shaped_into_outlines_with_its_metrics() {
    let scope = scope();
    let mut engine = Engine::default();
    let request = canvas_text::font_request("20px sans-serif").unwrap();
    let hello = canvas_text::shape(&mut engine, &scope, &request, "Hello", None).unwrap();
    let path = hello.path.as_ref().unwrap();
    // Ink on the baseline at the pen: above it, to the right, within the advance.
    assert!(hello.advance > 40. && hello.advance < 60., "{}", hello.advance);
    let b = path.bounds();
    assert!(b.top() < -10. && b.bottom() <= 0.5 && b.left() >= 0. && f64::from(b.right()) <= hello.advance + 0.5, "{b:?}");
    assert!(hello.ascent > 15. && hello.descent > 3., "{} {}", hello.ascent, hello.descent);
    assert_eq!(hello.ink[2], -f64::from(b.top()));
    // Longer text, longer advance; a newline is a space; nothing, no ink.
    let more = canvas_text::shape(&mut engine, &scope, &request, "Hello, world", None).unwrap();
    assert!(more.advance > hello.advance);
    let spaced = canvas_text::shape(&mut engine, &scope, &request, "a\nb", None).unwrap();
    let plain = canvas_text::shape(&mut engine, &scope, &request, "a b", None).unwrap();
    assert_eq!(spaced.advance, plain.advance);
    let empty = canvas_text::shape(&mut engine, &scope, &request, "", None).unwrap();
    assert!(empty.path.is_none() && empty.advance == 0.);
}
