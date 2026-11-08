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
    let fetcher: olo_js::Fetcher = Box::new(|url: &str, _: &str, _: &[u8], _: &str| {
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
fn find(doc: &olo_web::dom::Document, id: &str) -> olo_web::dom::NodeId {
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if doc.attribute(n, "id").ok().flatten() == Some(id) {
            return n;
        }
        stack.extend(doc.children(n).unwrap());
    }
    panic!("no #{id}")
}
#[test]
fn typing_ticking_and_submitting_a_form() {
    use olo_js::{Input, InputKind};
    let page = r#"<form id=f action="/search"><input id=q name=q value="ol"><input id=c type=checkbox name=lucky><select name=s><option value=a>A<option value=b selected>B</select><button id=go>Go</button></form><script>
const q = document.getElementById('q');
q.addEventListener('input', (e) => console.log('input', e.inputType, q.value));
document.getElementById('c').addEventListener('change', (e) => console.log('change', e.target.checked));
document.getElementById('f').addEventListener('submit', () => console.log('submit'));
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let (q, c) = (find(&parsed.document, "q"), find(&parsed.document, "c"));
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/dir/page.html")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/dir/page.html".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    let at = |kind| Input { kind, x: 0., y: 0., scroll_y: 0., buttons: 0, modifiers: 0 };
    let key = |k: &str, text: Option<&str>| at(InputKind::Key { down: true, key: k.into(), code: String::new(), text: text.map(Into::into) });
    realm.input(&at(InputKind::Down), Some(q));
    realm.input(&at(InputKind::Up), Some(q));
    realm.input(&key("o", Some("o")), None);
    realm.input(&key("x", Some("x")), None);
    realm.input(&key("Backspace", None), None);
    realm.input(&at(InputKind::Down), Some(c));
    realm.input(&at(InputKind::Up), Some(c));
    realm.input(&at(InputKind::Down), Some(q));
    realm.input(&at(InputKind::Up), Some(q));
    realm.input(&key("Enter", None), None);
    let report = realm.report();
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] input insertText olo", "[log] input insertText olox", "[log] input deleteContentBackward olo", "[log] change true", "[log] submit"]);
    assert_eq!(realm.take_navigation(), Some(("https://example.test/search?q=olo&lucky=on&s=b".to_owned(), false)));
    let (doc, _) = realm.finish();
    assert_eq!(doc.control_value(q).unwrap(), "olo");
}
#[test]
fn responses_stream_their_bodies_and_take_streams() {
    let (report, _) = run(r#"<script>
const r = new Response('hello');
const reader = r.body.getReader();
reader.read().then(({ done, value }) => {
  console.log(done, new TextDecoder().decode(value));
  const s = new ReadableStream({ start(c) { c.enqueue(new Uint8Array([104, 105])); c.enqueue(new Uint8Array([33])); c.close(); } });
  return new Response(s).arrayBuffer();
}).then((b) => console.log(new TextDecoder().decode(new Uint8Array(b))));
</script>"#);
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] false hello", "[log] hi!"]);
}
#[test]
fn an_elements_load_event_never_reaches_the_window() {
    let (report, _) = run(r#"<img id=i src=x.png><script>
addEventListener('load', (e) => console.log('window saw', e.target === window ? 'window' : e.target.tagName), true);
document.addEventListener('load', (e) => console.log('document saw', e.target.tagName), true);
</script>"#);
    assert_eq!(report.errors, Vec::<String>::new());
    // The image's error and the window's own load; the document captures
    // nothing of the window's load, the window nothing of the image's.
    assert_eq!(report.console, ["[log] window saw window"]);
}
#[test]
fn match_media_answers_whole_queries_as_the_sheets_do() {
    // GitHub opens its menus on hover only for `(hover: hover) and
    // (pointer: fine) and (min-width: 1012px)`: false at 1000px wide.
    let (report, _) = run(
        "<script>for (const q of ['(hover: hover) and (pointer: fine) and (min-width: 1012px)', '(min-width: 768px) and (hover: hover)', 'screen and (max-width: 999px)', '(prefers-color-scheme: dark)', '(prefers-reduced-motion: reduce)', '(min-resolution: 1dppx)', 'not all and (min-width: 2000px)', '(orientation: landscape)', 'print']) console.log(q + ' ' + matchMedia(q).matches); const m = matchMedia('(min-width: 1px)'); let n = 0; m.addListener(() => n++); m.addEventListener('change', () => n++); console.log(m instanceof MediaQueryList, m.media, n)</script>",
    );
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(
        report.console,
        [
            "[log] (hover: hover) and (pointer: fine) and (min-width: 1012px) false",
            "[log] (min-width: 768px) and (hover: hover) true",
            "[log] screen and (max-width: 999px) false",
            "[log] (prefers-color-scheme: dark) false",
            "[log] (prefers-reduced-motion: reduce) false",
            "[log] (min-resolution: 1dppx) true",
            "[log] not all and (min-width: 2000px) true",
            "[log] (orientation: landscape) true",
            "[log] print false",
            "[log] true (min-width: 1px) 0",
        ]
    );
}
#[test]
fn a_box_a_person_scrolls_tells_its_scripts() {
    let page = r#"<div id=s style="overflow:auto;height:50px"><p style="height:400px">x</p></div><script>
const s = document.getElementById('s');
s.addEventListener('scroll', (e) => console.log('scroll', e.target.id, s.scrollTop, s.scrollLeft, e.bubbles));
document.addEventListener('scroll', () => console.log('document saw it'));
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let s = find(&parsed.document, "s");
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    let serial = realm.scroll_serial();
    // Two wheel steps before the page's next task: one event, at the last offset.
    assert!(realm.scroll_box(s, [0., 30.]));
    assert!(realm.scroll_box(s, [0., 45.]));
    assert!(!realm.scroll_box(s, [0., 45.]));
    assert!(realm.scroll_serial() > serial);
    realm.advance(100., std::time::Duration::from_secs(1));
    let report = realm.report();
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] scroll s 45 0 false"]);
    let (doc, _) = realm.finish();
    assert_eq!(doc.scroll_offset(s), [0., 45.]);
}
#[test]
fn media_elements_play_through_the_browsers_player() {
    let page = r#"<video id=v src="film.mp4" muted></video><script>
const v = document.getElementById('v');
console.log(v instanceof HTMLMediaElement, v.paused, v.readyState, isNaN(v.duration), v.muted, v.canPlayType('video/mp4') !== '', v.HAVE_FUTURE_DATA, HTMLMediaElement.HAVE_METADATA);
for (const e of ['play', 'loadedmetadata', 'playing', 'timeupdate', 'ended']) v.addEventListener(e, () => console.log(e, v.paused, v.currentTime, v.duration));
v.play();
console.log('after play', v.paused);
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let v = find(&parsed.document, "v");
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    assert_eq!(realm.take_media_commands(), [(v, olo_js::MediaCommand::Play)]);
    realm.advance(10., std::time::Duration::from_secs(1));
    let media = olo_js::Media { paused: false, ended: false, time: 0., duration: 4., width: 640, height: 360, ready: 4 };
    realm.media_update(v, media, &["loadedmetadata", "playing"]);
    realm.media_update(v, olo_js::Media { time: 4., paused: true, ended: true, ..media }, &["timeupdate", "ended"]);
    let report = realm.report();
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(
        report.console,
        [
            "[log] true true 0 true true true 3 1",
            "[log] after play false",
            "[log] play false 0 NaN",
            "[log] loadedmetadata false 0 4",
            "[log] playing false 0 4",
            "[log] timeupdate true 4 4",
            "[log] ended true 4 4"
        ]
    );
}
#[test]
fn an_images_superseded_load_neither_shows_nor_fires() {
    // apple.com: a 1 × 1 placeholder (fetched) then the real picture
    // (known at once); the placeholder finishing later must not win.
    let page = r#"<img id=i><script>
const i = document.getElementById('i');
i.addEventListener('load', () => console.log('load', i.naturalWidth));
i.src = 'data:image/gif;base64,R0lGODlhAQABAAAAACw=';
setTimeout(() => { i.src = 'https://example.test/real.png'; }, 20);
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let img = find(&parsed.document, "i");
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let real = olo_web::image::Image::from_rgba(4, 3, vec![255; 48], Default::default(), None).unwrap();
    let known = real.clone();
    let lookup: olo_js::ImageLookup = Box::new(move |url: &str| (url == "https://example.test/real.png").then(|| known.clone()));
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap().with_image_lookup(Some(lookup));
    realm.run_page(&scripts);
    realm.advance(10., std::time::Duration::from_secs(1));
    let requests = realm.take_requests();
    assert_eq!(requests.len(), 1, "the placeholder is fetched");
    // The real picture, known at once, while the placeholder is in flight.
    realm.advance(100., std::time::Duration::from_secs(1));
    let placeholder = olo_web::image::Image::from_rgba(1, 1, vec![0; 4], Default::default(), None).unwrap();
    realm.deliver(requests[0].id, Ok(olo_js::Loaded::Image(placeholder)));
    let bound: Vec<_> = realm.canvases().into_iter().filter(|(n, _)| *n == img).collect();
    assert_eq!(bound.len(), 1);
    assert!(Arc::ptr_eq(&bound[0].1, &real), "the real picture stays");
    let report = realm.report();
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] load 4"]);
}
#[test]
fn a_picture_a_script_makes_or_changes_loads_its_image() {
    // apple.com: a lazy <picture> replaced by one parsed from its
    // <noscript> markup, and a placeholder source swapped for real ones.
    let page = r#"<div id=w></div><picture id=p><source srcset="data:image/gif;base64,R0lGODlhAQABAAAAACw=" media="(min-width:0px)"><img id=i src="https://example.test/static.png"></picture><script>
const w = document.getElementById('w');
w.innerHTML = '<picture><source srcset="https://example.test/made.png"><img alt=made></picture>';
w.querySelector('img').addEventListener('load', () => console.log('made', w.querySelector('img').naturalWidth));
const p = document.getElementById('p');
p.firstElementChild.remove();
const s = document.createElement('source');
s.srcset = 'https://example.test/real.png';
p.insertBefore(s, p.firstChild);
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let lookup: olo_js::ImageLookup = Box::new(|_: &str| None);
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap().with_image_lookup(Some(lookup));
    realm.run_page(&scripts);
    realm.advance(10., std::time::Duration::from_secs(1));
    let mut urls: Vec<String> = realm.take_requests().into_iter().map(|r| r.url).collect();
    urls.sort();
    assert_eq!(urls, ["https://example.test/made.png", "https://example.test/real.png"]);
    assert_eq!(realm.report().errors, Vec::<String>::new());
}
#[test]
fn the_page_is_not_a_touch_device() {
    // apple.com laid itself out for a tablet: Modernizr-style checks.
    let page =
        r#"<script>console.log('ontouchstart' in window, 'ontouchstart' in document.documentElement, navigator.maxTouchPoints, 'onclick' in window, matchMedia('(pointer: fine)').matches)</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    assert_eq!(realm.report().console, ["[log] false false 0 true true"]);
}
#[test]
fn raw_text_elements_serialize_their_text_as_it_is() {
    // apple.com's lazy pictures parse their sources from the <noscript>'s
    // innerHTML (text, with scripting on), which came back escaped.
    let page = r#"<noscript><picture><source srcset="a.jpg"></picture></noscript><style>a > b {}</style><template><p>t</p></template><p>1 &lt; 2</p><script>
console.log(document.querySelector('noscript').innerHTML, document.querySelector('style').innerHTML, document.querySelector('template').innerHTML, document.querySelector('p').innerHTML);
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    assert_eq!(realm.report().console, [r#"[log] <picture><source srcset="a.jpg"></picture> a > b {} <p>t</p> 1 &lt; 2"#]);
}
#[test]
fn a_clicked_link_is_followed_unless_the_page_cancels_it() {
    // apple.com's search button is a link to /us/search whose click
    // handler opens a panel in its place.
    use olo_js::{Input, InputKind};
    let page = r#"<a id=s href="/us/search"><span id=si>Search</span></a><a id=l href="/mac/">Mac</a><a id=j href="javascript:console.log('ran')">J</a><script>
document.getElementById('s').addEventListener('click', (e) => { e.preventDefault(); console.log('panel'); });
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let (si, l, j) = (find(&parsed.document, "si"), find(&parsed.document, "l"), find(&parsed.document, "j"));
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    let at = |kind| Input { kind, x: 0., y: 0., scroll_y: 0., buttons: 0, modifiers: 0 };
    let click = |realm: &mut Realm, n| {
        realm.input(&at(InputKind::Down), Some(n));
        realm.input(&at(InputKind::Up), Some(n));
    };
    click(&mut realm, si);
    assert_eq!(realm.take_navigation(), None, "cancelled");
    click(&mut realm, j);
    assert_eq!(realm.take_navigation(), None, "javascript: runs, goes nowhere");
    click(&mut realm, l);
    assert_eq!(realm.take_navigation(), Some(("https://example.test/mac/".to_owned(), false)));
    assert_eq!(realm.report().console, ["[log] panel", "[log] ran"]);
}
#[test]
fn an_upgraded_elements_observed_attributes_reach_its_callback() {
    // React makes <relative-time> then sets its datetime.
    let page = r#"<script>
class T extends HTMLElement { static get observedAttributes() { return ['datetime']; } attributeChangedCallback(n, o, v) { console.log('changed', n, o, v); } }
customElements.define('x-time', T);
const t = document.createElement('x-time');
t.setAttribute('datetime', '2026');
t.setAttribute('title', 'ignored');
t.setAttribute('datetime', '2027');
t.removeAttribute('datetime');
const root = t.attachShadow({ mode: 'open' });
root.textContent = '4 hours ago';
document.body.appendChild(t);
console.log(t.shadowRoot === root, t.isConnected, root.host === t);
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    assert_eq!(realm.report().errors, Vec::<String>::new());
    assert_eq!(realm.report().console, ["[log] changed datetime null 2026", "[log] changed datetime 2026 2027", "[log] changed datetime 2027 null", "[log] true true true"]);
    // The engine's document has the shadow root under its host.
    let (doc, _) = realm.finish();
    let mut stack = vec![doc.root()];
    let mut found = None;
    while let Some(n) = stack.pop() {
        if doc.name(n).ok().flatten() == Some("x-time") {
            found = Some(n);
        }
        stack.extend(doc.children(n).unwrap());
    }
    let t = found.unwrap();
    let root = doc.shadow_root(t).expect("a shadow root");
    let text = doc.composed_first_child(t).unwrap().unwrap();
    assert_eq!(doc.parent(text).unwrap(), Some(root));
    assert_eq!(doc.value(text).unwrap(), Some("4 hours ago"));
}
#[test]
fn tree_walkers_step_in_document_order_skip_and_reject_and_define_reaches_shadow_trees() {
    let page = r#"<div id=r><a id=a><b id=b></b></a><!--c--><i id=i><u id=u></u></i><s id=s></s></div><script>
const r = document.getElementById('r');
const ids = (w, step) => { const out = []; let n; while ((n = w[step]())) out.push(n.id || '#' + n.nodeType); return out.join(' '); };
console.log(ids(document.createTreeWalker(r, NodeFilter.SHOW_ELEMENT), 'nextNode'));
console.log(ids(document.createTreeWalker(r, 129), 'nextNode'));
// REJECT skips <i>'s subtree; SKIP skips <a> but not its children.
const f = { acceptNode: (n) => n.id === 'i' ? NodeFilter.FILTER_REJECT : n.id === 'a' ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT };
console.log(ids(document.createTreeWalker(r, NodeFilter.SHOW_ELEMENT, f), 'nextNode'));
const back = document.createTreeWalker(r, NodeFilter.SHOW_ELEMENT); back.currentNode = document.getElementById('s');
console.log(ids(back, 'previousNode'));
const w = document.createTreeWalker(r, NodeFilter.SHOW_ELEMENT);
console.log(w.firstChild().id, w.nextSibling().id, w.nextSibling().id, w.parentNode().id, w.lastChild().id);
// A definition upgrades the elements already in shadow trees too.
const host = document.createElement('div'); document.body.appendChild(host);
host.attachShadow({ mode: 'open' }).innerHTML = '<x-late></x-late>';
console.log(host.shadowRoot.isConnected, host.shadowRoot.firstChild.isConnected);
customElements.define('x-late', class extends HTMLElement { connectedCallback() { console.log('connected in shadow', this.getRootNode() === host.shadowRoot); } });
console.log(host.shadowRoot.querySelectorAll(':not(:defined)').length, document.querySelectorAll(':not(:defined)').length);
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    assert_eq!(realm.report().errors, Vec::<String>::new());
    assert_eq!(realm.report().console, ["[log] a b i u s", "[log] a b #8 i u s", "[log] b s", "[log] u i b a r", "[log] a i s r s", "[log] true true", "[log] connected in shadow true", "[log] 0 0"]);
}
#[test]
fn declarative_shadow_roots_are_their_hosts_shadow_roots() {
    let page = r#"<div id=h><template shadowrootmode="open"><b>inside</b></template>light</div><script>
const h = document.getElementById('h');
console.log(!!h.shadowRoot, h.shadowRoot && h.shadowRoot.firstChild.textContent, h.querySelector('template'), h.textContent);
const again = h.attachShadow({ mode: 'open' });
console.log(again === h.shadowRoot, again.childNodes.length);
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    assert_eq!(realm.report().errors, Vec::<String>::new());
    assert_eq!(realm.report().console, ["[log] true inside null light", "[log] true 0"]);
}
#[test]
fn pressing_a_select_asks_for_its_menu_and_a_choice_is_the_persons() {
    use olo_js::{Input, InputKind};
    let page = r#"<select id=s><option value=a>A<option value=b selected>B<option value=c>C</select><select id=d disabled><option>X</select><script>
const s = document.getElementById('s');
s.addEventListener('input', () => console.log('input', s.value));
s.addEventListener('change', () => console.log('change', s.value, s.selectedIndex));
</script>"#;
    let parsed = html::parse(Arc::from(page), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let (s, d) = (find(&parsed.document, "s"), find(&parsed.document, "d"));
    let scripts: Vec<Script> = olo_js::page_scripts(&parsed.document, "https://example.test/")
        .into_iter()
        .map(|(source, name, module, node)| match source {
            Source::Inline(text) => Script { source: text, name, module, node: Some(node) },
            Source::External(u) => panic!("external script {u}"),
        })
        .collect();
    let mut realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
    realm.run_page(&scripts);
    let at = |kind| Input { kind, x: 0., y: 0., scroll_y: 0., buttons: 0, modifiers: 0 };
    // A disabled select opens nothing; an enabled one asks for its menu.
    realm.input(&at(InputKind::Down), Some(d));
    assert_eq!(realm.take_select_menu(), None);
    realm.input(&at(InputKind::Down), Some(s));
    realm.input(&at(InputKind::Up), Some(s));
    assert_eq!(realm.take_select_menu(), Some(s));
    // The same option again changes nothing; another fires input and change.
    realm.choose_option(s, 1);
    realm.choose_option(s, 2);
    let report = realm.report();
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] input c", "[log] change c 2"]);
}
#[test]
fn web_crypto_digests_and_signs_and_form_data_bodies_are_multipart() {
    let (report, _) = run(r#"<script>
const hex = (b) => Array.from(new Uint8Array(b), (x) => x.toString(16).padStart(2, '0')).join('');
crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc')).then((b) => console.log('sha256', hex(b)));
crypto.subtle.importKey('raw', new TextEncoder().encode('Jefe'), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign'])
  .then((k) => crypto.subtle.sign('HMAC', k, new TextEncoder().encode('what do ya want for nothing?')))
  .then((s) => console.log('hmac', hex(s)));
crypto.subtle.digest('MD5', new Uint8Array(1)).catch((e) => console.log('md5', e.name));
console.log('uuid', /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID()));
const fd = new FormData(); fd.append('a', '1'); fd.append('f', new File(['xy'], 'x.txt', { type: 'text/plain' }));
const r = new Request('https://example.test/up', { method: 'POST', body: fd });
const type = r.headers.get('content-type');
const boundary = type.split('boundary=')[1];
r.text().then((t) => console.log('form', type.startsWith('multipart/form-data; boundary='), t === '--' + boundary + '\r\nContent-Disposition: form-data; name="a"\r\n\r\n1\r\n--' + boundary + '\r\nContent-Disposition: form-data; name="f"; filename="x.txt"\r\nContent-Type: text/plain\r\n\r\nxy\r\n--' + boundary + '--\r\n'));
</script>"#);
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(
        report.console,
        [
            "[log] uuid true",
            "[log] sha256 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "[log] md5 NotSupportedError",
            "[log] form true true",
            "[log] hmac 5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        ]
    );
}
#[test]
fn dialogs_open_close_and_summaries_toggle_their_details() {
    let (report, doc) = run("<dialog id=d><button id=b>OK</button></dialog><details id=x><summary id=s>More</summary><p>Hidden</p></details><script>
        const d = document.getElementById('d');
        console.log('closed', d.open);
        d.showModal();
        console.log('modal', d.open, d.matches(':modal'), document.activeElement && document.activeElement.id);
        d.close('done');
        console.log('closed again', d.open, d.matches(':modal'), d.returnValue);
        d.show();
        console.log('shown', d.open, d.matches(':modal'));
        const x = document.getElementById('x');
        document.getElementById('s').click();
        console.log('details', x.open);
        document.getElementById('s').click();
        console.log('details', x.open);
        </script>");
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.console, ["[log] closed false", "[log] modal true true b", "[log] closed again false false done", "[log] shown true false", "[log] details true", "[log] details false"]);
    let d = find(&doc, "d");
    assert!(doc.attribute(d, "open").unwrap().is_some());
}
