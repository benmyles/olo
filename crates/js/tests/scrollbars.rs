//! CSSOM distinguishes the full window, client viewport and containing block.
use olo_js::{Config, Realm};
use olo_web::{css::media::Media, layout, page, text::shaping};
use std::{cell::Cell, rc::Rc, sync::Arc};
#[path = "../../web/tests/support/classic_host.rs"]
mod classic_host;
use classic_host::{Classic, scope};
fn realm(scale: f64) -> (Realm, Rc<Cell<usize>>) {
    let view = layout::Viewport { width: 1000., height: 700., scale };
    let source = "<!doctype html><style>html,body{margin:0}#content{height:2000px}#percent{width:100%;height:1px}#vw{width:100vw;height:1px}</style><div id=content><div id=percent></div><div id=vw></div></div>";
    let p = page::prepare(Arc::from(source), &scope(), Media::screen(1000., 700., scale, false), false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
    let host = Classic::new(p.scope().clone(), view);
    let measurer = p.measurer();
    let mut shaper = shaping::Engine::default();
    let calls = Rc::new(Cell::new(0));
    let count = calls.clone();
    let measure: olo_js::Measure = Box::new(move |doc, _, previous| {
        count.set(count.get() + 1);
        Some(measurer.measure(doc, &host, &mut shaper, view, previous, None).unwrap())
    });
    let mut r = Realm::new(p.into_document(), Config { viewport: (1000., 700.), scale, ..Default::default() }, None, Some(measure)).unwrap();
    r.run_page(&[]);
    (r, calls)
}
#[test]
fn live_classic_metrics_and_cached_reads() {
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let (mut r, calls) = realm(scale);
        let bar = (15_f64 * scale).round() / scale;
        let client = 1000. - bar;
        // 100vw overflows the narrowed containing block, on both axes.
        let read = "[innerWidth,innerHeight,document.documentElement.clientWidth,document.documentElement.clientHeight,visualViewport.width,visualViewport.height,document.querySelector('#percent').getBoundingClientRect().width,document.querySelector('#vw').getBoundingClientRect().width,matchMedia('(width: 1000px)').matches]";
        let value: serde_json::Value = serde_json::from_str(&r.eval(read).unwrap()).unwrap();
        let expected = serde_json::json!([1000, 700, client.round(), (700. - bar).round(), client, 700. - bar, client, 1000, true]);
        for i in 0..8 {
            assert!((value[i].as_f64().unwrap() - expected[i].as_f64().unwrap()).abs() < 0.0001, "{scale} metric {i}: {value} != {expected}");
        }
        assert_eq!(value[8], true);
        let before = calls.get();
        r.eval("for(let i=0;i<1000;i++){void visualViewport.width;void document.documentElement.clientWidth}").unwrap();
        assert_eq!(calls.get(), before, "unchanged metrics must reuse their layout");
        r.eval("document.documentElement.style.overflow='scroll'").unwrap();
        assert_eq!(r.eval("document.querySelector('#vw').getBoundingClientRect().width").unwrap(), "985");
        r.eval("document.documentElement.style.cssText='scrollbar-gutter:stable both-edges';document.querySelector('#content').style.height='200px'").unwrap();
        let stable: serde_json::Value = serde_json::from_str(&r.eval(read).unwrap()).unwrap();
        assert_eq!(stable[2], 1000, "empty stable gutters are inside client width");
        assert_eq!(stable[4], 1000);
        assert!((stable[6].as_f64().unwrap() - (1000. - 2. * bar)).abs() < 1e-10);
        assert_eq!(stable[7], 970);
        r.eval("document.documentElement.style.cssText='scrollbar-width:none';document.querySelector('#content').style.height='2000px';scrollTo(0,99999)").unwrap();
        assert_eq!(r.eval("[visualViewport.width,document.documentElement.clientWidth,scrollY]").unwrap(), "[1000,1000,1300]");
        assert_eq!(r.eval("(()=>{try{Object.getOwnPropertyDescriptor(VisualViewport.prototype,'width').get.call({});return false}catch(e){return e instanceof TypeError}})()").unwrap(), "true");
        assert!(r.report().errors.is_empty(), "{:?}", r.report().errors);
    }
}
