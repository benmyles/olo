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

#[test]
fn viewport_scroll_requests_snap_clamp_and_coalesce() {
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let (mut r, _) = realm(scale);
        r.eval("document.querySelector('#content').style.width='2000px';globalThis.events=0;document.addEventListener('scroll',()=>events++)").unwrap();
        // Independent Chrome observations: positive and negative half-pixel
        // ties differ; RTL starts at zero and extends into negative x.
        let expected = match scale {
            1. => [1., 0.],
            1.25 => [0.8, -0.8],
            1.5 => [2. / 3., -2. / 3.],
            1.75 => [4. / 7., -4. / 7.],
            _ => [0.5, -0.5],
        };
        r.eval("scrollTo(.5,.5)").unwrap();
        let to = r.take_scroll_request().unwrap();
        assert!((to[0] - expected[0]).abs() < 1e-9, "{scale}: {to:?}");
        assert_eq!(to[0], to[1]);
        r.eval("scrollTo({top:32});scrollBy({left:16});scrollTo({left:80})").unwrap();
        assert_eq!(r.eval("[scrollX,scrollY,pageXOffset,pageYOffset]").unwrap(), "[80,32,80,32]");
        assert_eq!(r.take_scroll_request(), Some([80., 32.]));
        assert!(!r.set_scroll_position([80., 32.]));
        assert!(!r.has_scroll_request());
        assert_eq!(r.eval("events").unwrap(), "0");
        r.advance(20., std::time::Duration::from_secs(1));
        assert_eq!(r.eval("events").unwrap(), "1");
        r.eval("scrollTo(80,32)").unwrap();
        assert!(!r.has_scroll_request());
        r.advance(40., std::time::Duration::from_secs(1));
        assert_eq!(r.eval("events").unwrap(), "1");
        assert!(r.set_scroll_position([120., 64.]));
        assert_eq!(r.eval("[scrollX,scrollY]").unwrap(), "[120,64]");
        assert!(!r.has_scroll_request(), "accepting native movement must not echo a request");
        r.eval("scrollBy(Infinity,NaN);scrollBy(-Infinity,0);scrollTo();scrollTo(undefined);scrollTo(null);scrollTo(1);scrollTo({})").unwrap();
        assert_eq!(r.eval("[scrollX,scrollY]").unwrap(), "[120,64]");
        assert!(!r.has_scroll_request());
        r.eval("document.documentElement.style.direction='rtl';scrollTo(-.5,.5)").unwrap();
        let to = r.take_scroll_request().unwrap();
        assert!((to[0] - expected[1]).abs() < 1e-9, "RTL {scale}: {to:?}");
        r.eval("scrollTo(-10000,10000)").unwrap();
        let to = r.take_scroll_request().unwrap();
        let bar = (15. * scale).round() / scale;
        assert!((to[0] - (-1000. - bar)).abs() < 1e-9, "RTL end {scale}: {to:?}");
        assert!((to[1] - (1300. + bar)).abs() < 1e-9);
        r.eval("scrollTo(Infinity,NaN)").unwrap();
        assert_eq!(r.take_scroll_request(), Some([0., 0.]));
        assert!(r.report().errors.is_empty(), "{:?}", r.report().errors);
    }
}

#[test]
fn fixed_geometry_and_pointer_coordinates_follow_both_scroll_axes() {
    let (mut r, _) = realm(1.25);
    r.eval("document.body.insertAdjacentHTML('beforeend','<div id=fixed style=\"position:fixed;left:24px;top:32px;width:100px;height:40px\"><span>Fixed</span></div><div style=\"transform:translateX(0);position:absolute;left:0;top:0\"><div id=contained style=\"position:fixed;left:24px;top:32px;width:100px;height:40px\"></div></div>');document.querySelector('#content').style.width='2000px';scrollTo(80,120)").unwrap();
    assert_eq!(r.eval("(()=>{let r=document.querySelector('#fixed').getBoundingClientRect();return [r.x,r.y]})()").unwrap(), "[24,32]");
    assert_eq!(r.eval("(()=>{let r=document.querySelector('#fixed').getClientRects()[0];return [r.x,r.y]})()").unwrap(), "[24,32]");
    assert_eq!(r.eval("(()=>{let r=document.querySelector('#contained').getBoundingClientRect();return [r.x,r.y]})()").unwrap(), "[-56,-88]");
    r.eval("globalThis.pointer=null;document.body.addEventListener('mousemove',e=>pointer=[e.clientX,e.clientY,e.pageX,e.pageY]);document.body.addEventListener('wheel',e=>e.preventDefault(),{passive:false})").unwrap();
    let mut input = olo_js::Input { kind: olo_js::InputKind::Move, x: 110., y: 170., scroll_x: 80., scroll_y: 120., buttons: 0, modifiers: 0 };
    assert!(!r.input(&input, None));
    assert_eq!(r.eval("pointer").unwrap(), "[30,50,110,170]");
    input.kind = olo_js::InputKind::Wheel { dx: 10., dy: 20. };
    assert!(r.input(&input, None));
    assert_eq!(r.eval("document.elementFromPoint(990,50)").unwrap(), "null");
    assert!(r.report().errors.is_empty(), "{:?}", r.report().errors);
}
