use super::*;

fn collect(realm: &mut Realm) {
    let _entered = Entered::new(&realm.isolate);
    realm.isolate.clear_kept_objects();
    realm.isolate.low_memory_notification();
}

#[test]
fn media_query_ownership_follows_change_listeners() {
    let parsed = olo_web::html::parse(Arc::from("<body>"), Default::default(), None).unwrap();
    let mut realm = Realm::new(parsed.document, Config { dark: false, ..Default::default() }, None, None).unwrap();
    realm.run_page(&[]);
    realm
        .eval(
            r#"
        globalThis.unobserved = new WeakRef(matchMedia('screen'));
        globalThis.delivered = 0;
        (() => {
            const q = matchMedia('(prefers-color-scheme: dark)');
            q.addEventListener('change', () => delivered++, {once:true});
            globalThis.observed = new WeakRef(q);
        })();
        (() => {
            const q = matchMedia('(prefers-color-scheme: dark)');
            q.onchange = e => { delivered++; e.currentTarget.onchange = null; };
            globalThis.handler = new WeakRef(q);
        })();
    "#,
        )
        .unwrap();
    collect(&mut realm);
    assert_eq!(realm.eval("[!!unobserved.deref(),!!observed.deref(),!!handler.deref()]").unwrap(), "[false,true,true]");
    realm.set_environment((1000., 700.), 1., true);
    realm.advance_frame(100., std::time::Duration::from_secs(1), Some(true));
    assert_eq!(realm.eval("delivered").unwrap(), "2");
    collect(&mut realm);
    assert_eq!(realm.eval("[!!observed.deref(),!!handler.deref()]").unwrap(), "[false,false]");
}
