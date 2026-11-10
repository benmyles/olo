//! Live environment changes keep one document/clock and notify at a frame.
use olo_js::{Config, Realm};
use std::{sync::Arc, time::Duration};

fn realm() -> Realm {
    let parsed = olo_web::html::parse(Arc::from("<!doctype html><body>viewport"), Default::default(), None).unwrap();
    let mut realm = Realm::new(parsed.document, Config { viewport: (1000., 700.), scale: 1., dark: false, ..Default::default() }, None, None).unwrap();
    realm.run_page(&[]);
    realm
}

#[test]
fn environment_updates_are_atomic_coalesced_and_frame_ordered() {
    let mut r = realm();
    r.eval(r#"
        globalThis.records = [];
        globalThis.queries = ['(min-width: 900px)', '(min-resolution: 1.5dppx)', '(prefers-color-scheme: dark)'].map(q => matchMedia(q));
        globalThis.saved = [document, performance.timeOrigin, history.length];
        globalThis.record = (name,e) => records.push([name,innerWidth,innerHeight,devicePixelRatio,visualViewport.width,visualViewport.height,queries.map(q=>q.matches),e.constructor.name,e.isTrusted,e.bubbles]);
        addEventListener('resize',e => record('window',e));
        visualViewport.addEventListener('resize',e => record('visual',e));
        queries.forEach((q,i) => q.addEventListener('change',e => record('media'+i,e)));
        requestAnimationFrame(() => records.push(['raf']));
    "#).unwrap();
    assert!(r.set_environment((850., 650.), 1.25, false));
    assert!(r.set_environment((800., 600.), 2., true));
    assert!(!r.set_environment((800., 600.), 2., true));
    assert_eq!(r.eval("[records.length,innerWidth,innerHeight,devicePixelRatio,queries.map(q=>q.matches)]").unwrap(), "[0,800,600,2,[false,true,true]]");
    let before_frame = r.advance_frame(100., Duration::from_secs(1), Some(false));
    assert!(before_frame.wants_frame);
    assert_eq!(r.eval("records.length").unwrap(), "0");
    let frame = r.advance_frame(101., Duration::from_secs(1), Some(true));
    assert!(!frame.wants_frame);
    assert_eq!(r.eval("records.map(r=>r[0])").unwrap(), r#"["window","visual","media0","media1","media2","raf"]"#);
    assert_eq!(r.eval("records.slice(0,5).every(r=>JSON.stringify(r.slice(1,7))==='[800,600,2,800,600,[false,true,true]]'&&r[8]===true&&r[9]===false)").unwrap(), "true");
    assert_eq!(r.eval("records.slice(2,5).every(r=>r[7]==='MediaQueryListEvent')").unwrap(), "true");
    assert_eq!(r.eval("saved[0]===document&&saved[1]===performance.timeOrigin&&saved[2]===history.length").unwrap(), "true");
    r.advance_frame(150., Duration::from_secs(1), Some(true));
    assert_eq!(r.eval("records.length").unwrap(), "6");
    assert!(r.report().errors.is_empty(), "{:?}", r.report().errors);
}

#[test]
fn media_listeners_dpr_theme_and_reverted_changes() {
    let mut r = realm();
    r.eval(
        r#"
        globalThis.events=[];
        globalThis.q=matchMedia('(prefers-color-scheme: dark)');
        globalThis.removed=e=>events.push('removed');
        q.addListener(removed);q.removeListener(removed);
        q.addEventListener('change',()=>q.removeEventListener('change',removed));
        q.addEventListener('change',removed);
        q.addListener({handleEvent(e){events.push([e.media,e.matches,e.isTrusted]);}});
        q.addEventListener('change',()=>events.push('once'),{once:true});
        globalThis.aborted=new AbortController();
        q.addEventListener('change',()=>events.push('aborted'),{signal:aborted.signal});aborted.abort();
        addEventListener('resize',()=>events.push('resize'));
        visualViewport.addEventListener('resize',()=>events.push('visual'));
    "#,
    )
    .unwrap();
    r.set_environment((1000., 700.), 1., true);
    r.advance_frame(100., Duration::from_secs(1), Some(true));
    assert_eq!(r.eval("events.splice(0)").unwrap(), r#"[["(prefers-color-scheme: dark)",true,true],"once"]"#);
    r.set_environment((1000., 700.), 2., true);
    r.advance_frame(120., Duration::from_secs(1), Some(true));
    assert_eq!(r.eval("events.splice(0)").unwrap(), r#"["resize","visual"]"#);
    r.set_environment((800., 600.), 1., false);
    r.set_environment((1000., 700.), 2., true);
    r.advance_frame(140., Duration::from_secs(1), Some(true));
    assert_eq!(r.eval("events.splice(0)").unwrap(), "[]");
    r.set_environment((1000., 700.), 2., false);
    r.advance_frame(160., Duration::from_secs(1), Some(true));
    assert_eq!(r.eval("events.splice(0)").unwrap(), r#"[["(prefers-color-scheme: dark)",false,true]]"#);
    assert_eq!(
        r.eval("(()=>{const e=new MediaQueryListEvent('change',{media:'screen',matches:1});return [e.media,e.matches,e.isTrusted,Object.prototype.toString.call(e)]})()").unwrap(),
        r#"["screen",true,false,"[object MediaQueryListEvent]"]"#
    );
    assert!(!r.set_environment((f64::NAN, 1.), 1., false));
    assert!(!r.set_environment((1., 1.), 0., false));
    assert!(r.report().errors.is_empty(), "{:?}", r.report().errors);
}
