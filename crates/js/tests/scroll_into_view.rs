use olo_js::{Config, Realm};
use olo_web::{css::media::Media, layout, page, text::shaping};
use std::{cell::Cell, rc::Rc, sync::Arc, time::Duration};
#[path = "../../web/tests/support/classic_host.rs"]
mod classic_host;
#[path = "../../web/tests/support/paint_host.rs"]
mod paint_host;
fn realm(scale: f64) -> (Realm, Rc<Cell<usize>>) {
    let view = layout::Viewport { width: 800., height: 600., scale };
    let p = page::prepare(Arc::from(include_str!("scroll-into-view-fixtures/base.html")), &classic_host::scope(), Media::screen(800., 600., scale, false), false, None, |_, _| {}, |_, _, _| Ok(None))
        .unwrap();
    let host = paint_host::Scaled::new(p.scope().clone(), scale);
    let measurer = p.measurer();
    let mut shaper = shaping::Engine::default();
    let calls = Rc::new(Cell::new(0));
    let count = calls.clone();
    let measure: olo_js::Measure = Box::new(move |doc, _, previous| {
        count.set(count.get() + 1);
        Some(measurer.measure(doc, &host, &mut shaper, view, previous, None).unwrap())
    });
    let mut r = Realm::new(p.into_document(), Config { viewport: (800., 600.), scale, ..Default::default() }, None, Some(measure)).unwrap();
    r.run_page(&[]);
    (r, calls)
}
#[test]
fn ancestor_alignment_matches_independent_chrome() {
    compare(false);
}
#[test]
#[ignore = "vertical-rl block layout does not place its wide child from the block-start edge; retained Chrome nonpass"]
fn vertical_rl_block_layout_matches_chrome() {
    compare(true);
}
fn compare(vertical: bool) {
    let oracle: serde_json::Value = serde_json::from_str(include_str!("scroll-into-view-fixtures/chrome.json")).unwrap();
    let mut differences = Vec::new();
    assert_eq!(oracle.as_array().unwrap().len(), 5);
    for row in oracle.as_array().unwrap() {
        let scale = row["dpr"].as_f64().unwrap();
        let (mut r, _) = realm(scale);
        let actual: serde_json::Value = serde_json::from_str(&r.eval(include_str!("scroll-into-view-fixtures/journey.js")).unwrap()).unwrap();
        assert_eq!(actual["viewport"], row["viewport"]);
        assert_eq!(actual["dpr"], row["dpr"]);
        assert_eq!(actual["cases"].as_array().unwrap().len(), 28);
        assert_eq!(row["cases"].as_array().unwrap().len(), 28);
        if std::env::var_os("OLO_SCROLL_TEST_TRACE").is_some() {
            eprintln!("ACTUAL {actual}");
        }
        for (want, got) in row["cases"].as_array().unwrap().iter().zip(actual["cases"].as_array().unwrap()) {
            assert_eq!(want["name"], got["name"]);
            if (want["name"] == "vertical-rl-start") != vertical {
                continue;
            }
            let (mut fresh, _) = realm(scale);
            fresh.eval(&format!("globalThis.caseOnly={}", want["name"])).unwrap();
            let fresh: serde_json::Value = serde_json::from_str(&fresh.eval(include_str!("scroll-into-view-fixtures/journey.js")).unwrap()).unwrap();
            for key in ["offsets", "rect"] {
                assert_eq!(got[key], fresh["cases"][0][key], "fresh/reused {} {key}", want["name"]);
                assert_eq!(want[key].as_array().unwrap().len(), got[key].as_array().unwrap().len());
                for (i, (w, g)) in want[key].as_array().unwrap().iter().zip(got[key].as_array().unwrap()).enumerate() {
                    if (w.as_f64().unwrap() - g.as_f64().unwrap()).abs() > 0.02 {
                        differences.push(format!("{scale} {} {key}[{i}]: {g} != {w}", want["name"]));
                    }
                }
            }
        }
        assert!(r.report().errors.is_empty(), "{:?}", r.report().errors);
    }
    assert!(differences.is_empty(), "{} discrepancies:\n{}", differences.len(), differences.join("\n"));
}
#[test]
fn smooth_scroll_finishes_and_rests_without_layout_polling() {
    let (mut r, calls) = realm(1.);
    r.eval("globalThis.events=[];const i=document.getElementById('inner');i.addEventListener('scroll',()=>events.push('scroll'));i.addEventListener('scrollend',()=>events.push('end'));document.getElementById('target').scrollIntoView({behavior:'smooth',container:'nearest'})").unwrap();
    let count = calls.get();
    let base = r.eval("performance.now()").unwrap().parse::<f64>().unwrap();
    assert_eq!(r.eval("document.getElementById('inner').scrollLeft").unwrap(), "0");
    assert!(r.advance_frame(base + 16., Duration::from_secs(1), Some(true)).wants_frame);
    assert!(r.eval("document.getElementById('inner').scrollLeft>0").unwrap() == "true");
    for t in (32..=640).step_by(16) {
        r.advance_frame(base + t as f64, Duration::from_secs(1), Some(true));
    }
    assert_eq!(r.eval("[document.getElementById('inner').scrollLeft,document.getElementById('inner').scrollTop]").unwrap(), "[280,300]");
    assert_eq!(r.eval("events[events.length-1]").unwrap(), "end");
    let a = r.advance_frame(base + 1000., Duration::from_secs(1), Some(true));
    assert!(!a.wants_frame);
    assert_eq!(a.next_due_ms, None);
    assert_eq!(calls.get(), count, "scrolling must retain its measurement");
    r.eval("document.getElementById('inner').scrollTo({left:0,top:0,behavior:'smooth'})").unwrap();
    r.advance_frame(base + 1050., Duration::from_secs(1), Some(true));
    r.eval("document.getElementById('inner').scrollTo({left:42,top:23,behavior:'instant'})").unwrap();
    r.advance_frame(base + 2000., Duration::from_secs(1), Some(true));
    assert_eq!(r.eval("[document.getElementById('inner').scrollLeft,document.getElementById('inner').scrollTop]").unwrap(), "[42,23]");
}
#[test]
fn child_mutated_before_parent_inherits_final_parent_style() {
    let (mut r, _) = realm(1.);
    r.eval("document.getElementById('target').style.position='fixed';void document.getElementById('target').offsetWidth").unwrap();
    // The child's mutation is logged first, then its parent's inherited change.
    assert_eq!(r.eval("document.getElementById('target').removeAttribute('style');document.getElementById('inner').style.direction='rtl';['inner','inside','target'].map(id=>getComputedStyle(document.getElementById(id)).direction)").unwrap(),"[\"rtl\",\"rtl\",\"rtl\"]");
}

#[test]
fn smooth_sequence_matches_chrome() {
    let (mut r, _) = realm(1.);
    r.eval(include_str!("scroll-into-view-fixtures/smooth.js")).unwrap();
    for t in (16..=3000).step_by(16) {
        r.advance_frame(t as f64, Duration::from_secs(1), Some(true));
    }
    let result: serde_json::Value = serde_json::from_str(&r.eval("smoothResult").unwrap()).unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!("scroll-into-view-fixtures/chrome-smooth.json")).unwrap();
    assert_eq!(result, expected);
    assert!(r.report().errors.is_empty(), "{:?}", r.report().errors);
}

#[test]
fn scrolling_idl_matches_current_chrome() {
    let (mut r, _) = realm(1.);
    r.eval(include_str!("scroll-into-view-fixtures/idl.js")).unwrap();
    for t in (16..=1000).step_by(16) {
        r.advance_frame(t as f64, Duration::from_secs(1), Some(true));
    }
    let actual: serde_json::Value = serde_json::from_str(&r.eval("idlResult").unwrap()).unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!("scroll-into-view-fixtures/chrome-idl.json")).unwrap();
    assert_eq!(actual, expected);
    assert!(r.report().errors.is_empty(), "{:?}", r.report().errors);
}
