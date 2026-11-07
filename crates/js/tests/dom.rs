//! The DOM and Web API surface, exercised by a page of scripts whose console
//! output is the expectation: every line a script logs is checked.
use olo_js::{Config, Realm, Script, Source};
use olo_web::html;
use std::sync::Arc;
fn run(page: &str) -> (olo_js::Report, olo_web::dom::Document) {
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/dir/page.html")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
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
    let names: Vec<String> = olo_js::page_scripts(&parsed.document, "https://x.test/p/").into_iter().map(|(_, n, _, _)| n).collect();
    // Modules run in order with the classic scripts; `nomodule` ones not at all.
    assert_eq!(names, ["inline script 2", "https://x.test/p/#inline-module-3", "https://x.test/p/b.js", "https://x.test/p/a.js"]);
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
#[test]
fn modules_import_bare_names_through_the_import_map() {
    let page = r#"<script type=importmap>{"imports":{"react":"https://cdn.test/react.js","lib/":"https://cdn.test/lib/"}}</script>
<script type=module>import { name } from "react"; import { twice } from "lib/math.js"; console.log(name, twice(21));</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let fetcher: olo_js::Fetcher = Box::new(|url: &str, _: &str, _: &str| {
        let body = match url {
            "https://cdn.test/react.js" => "export const name = 'react';",
            "https://cdn.test/lib/math.js" => "export const twice = n => n * 2;",
            _ => return Err(format!("unexpected fetch {url}")),
        };
        Ok(olo_js::Fetched { status: 200, content_type: "text/javascript".into(), body: body.as_bytes().to_vec(), url: url.into() })
    });
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, Some(fetcher), None).unwrap();
    let report = realm.run_page(&scripts);
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] react 42"]);
}
#[test]
fn listeners_without_this_go_to_the_global_and_signals_remove_them() {
    let (report, _) = run(r#"<script>"use strict";
const add = EventTarget.prototype.addEventListener;
let calls = 0;
add.call(undefined, "ping", () => calls++);
const c = new AbortController();
window.addEventListener("ping", () => calls += 10, { signal: c.signal });
dispatchEvent(new Event("ping"));
c.abort();
dispatchEvent(new Event("ping"));
window.addEventListener("ping", () => calls += 100, { signal: AbortSignal.abort() });
dispatchEvent(new Event("ping"));
console.log(calls);
</script>"#);
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] 13"]);
}
#[test]
fn style_answers_in_for_the_properties_the_engine_knows() {
    let (report, _) = run(r#"<div id=d></div><script>
const s = document.getElementById("d").style;
console.log("opacity" in s, "backgroundColor" in s, "background-color" in s, "cssFloat" in s, "--x" in s, "noSuchThing" in s, "setProperty" in s);
</script>"#);
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] true true true true true false true"]);
}
#[test]
fn current_script_is_the_running_classic_scripts_element() {
    let (report, _) = run(
        r#"<script id=first>console.log(document.currentScript && document.currentScript.id);</script><script type=module>console.log(String(document.currentScript));</script><script id=third>console.log(document.currentScript.id); setTimeout(() => console.log(String(document.currentScript)), 0);</script>"#,
    );
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] first", "[log] null", "[log] third", "[log] null"]);
}
#[test]
fn page_images_fire_their_load_or_error_before_the_windows_load() {
    let (report, _) = run(r#"<img id=i src=missing.png><img id=none><script>
document.getElementById('i').addEventListener('error', () => console.log('img error'));
document.getElementById('none').addEventListener('error', () => console.log('wrong: no source'));
addEventListener('load', () => console.log('window load'));
</script>"#);
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] img error", "[log] window load"]);
}
#[test]
fn style_sheets_take_inserted_rules_and_write_them_back() {
    let (report, doc) = run(r#"<style id=s>.a{color:red}</style><script>
const sheet = document.getElementById('s').sheet;
console.log(sheet.cssRules.length, sheet.cssRules[0].selectorText);
sheet.insertRule('.b{color:blue}', 1);
sheet.insertRule('@media (min-width:1px){.c{color:green}}', 0);
console.log(sheet.cssRules.length, sheet.cssRules[0].type, sheet.cssRules[2].cssText);
sheet.deleteRule(0);
try { sheet.insertRule('.d{}', 9); } catch (e) { console.log(e.name); }
console.log(document.styleSheets.length, document.styleSheets[0] === sheet);
Promise.resolve().then(() => console.log(JSON.stringify(document.getElementById('s').textContent)));
</script>"#);
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] 1 .a", "[log] 3 4 .b{color:blue}", "[log] IndexSizeError", "[log] 1 true", "[log] \".a{color:red}\\n.b{color:blue}\""]);
    let _ = doc;
}
