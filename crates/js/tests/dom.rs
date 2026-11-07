//! The DOM and Web API surface, exercised by a page of scripts whose console
//! output is the expectation: every line a script logs is checked.
use olo_js::{Config, Realm, Script, Source};
use olo_web::html;
use std::sync::Arc;
fn run(page: &str) -> (olo_js::Report, olo_web::dom::Document) {
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/dir/page.html")
        .into_iter()
        .map(|(source, name)| match source {
            Source::Inline(text) => Script { source: text, name },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/dir/page.html".into(), ..Default::default() }, None, None).unwrap();
    let report = realm.run_page(&scripts);
    let (doc, _) = realm.finish();
    (report, doc)
}
#[test]
fn dom_page() {
    let (report, _) = run(include_str!("dom.html"));
    assert_eq!(report.errors, Vec::<String>::new());
    // tests/dom.expected: the console lines the page logs, one per line.
    let expected: Vec<String> = include_str!("dom.expected").lines().map(String::from).collect();
    assert_eq!(report.console, expected);
    assert_eq!(report.scripts, 6);
}
#[test]
fn scripts_in_order_then_async() {
    let page = "<script async src=a.js></script><script>1</script><script type=module>2</script><script defer src=b.js></script><script type=text/template>no</script><script nomodule>3</script>";
    let parsed = html::parse(Arc::from(page), Default::default(), None).unwrap();
    let names: Vec<String> = olo_js::page_scripts(&parsed.document, "https://x.test/p/").into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["inline script 2", "https://x.test/p/b.js", "inline script 4", "https://x.test/p/a.js"]);
}
#[test]
fn errors_are_reported_not_fatal() {
    let (report, doc) = run("<body><script>throw new Error('boom')</script><script>document.body.id='after'</script>");
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].starts_with("Error: boom (inline script 1:1)"), "{}", report.errors[0]);
    let body = doc.children(doc.root()).unwrap().last().unwrap();
    let html = body;
    let body = doc.children(html).unwrap().last().unwrap();
    assert_eq!(doc.attribute(body, "id").unwrap(), Some("after"));
}
