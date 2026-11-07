//! Run a saved page's scripts and report: parse FILE.html with scripting on,
//! run its scripts (external ones read from disk next to the page, by their
//! path relative to --url or the file), fire the lifecycle, pump timers, and
//! print the report and the document's size before and after.
//!
//!   cargo run --release -p olo-js --example run -- FILE.html [--url URL] [--html OUT.html] [--eval JS]
use olo_js::{Config, Realm, Script, Source};
use olo_web::html;
use std::{path::Path, sync::Arc};
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = args.first().expect("FILE.html");
    let mut url = format!("file://{}", std::fs::canonicalize(file).unwrap().display());
    let mut out = None;
    let mut eval = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--url" => {
                url = args[i + 1].clone();
                i += 1;
            }
            "--html" => {
                out = Some(args[i + 1].clone());
                i += 1;
            }
            "--eval" => {
                eval = Some(args[i + 1].clone());
                i += 1;
            }
            a => panic!("unknown argument {a}"),
        }
        i += 1;
    }
    let source: Arc<str> = Arc::from(std::fs::read_to_string(file).unwrap());
    let parsed = html::parse(source, html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
    let doc = parsed.document;
    let before = doc.memory();
    let dir = Path::new(file).parent().unwrap().to_path_buf();
    let page_url = url::Url::parse(&url).unwrap();
    let scripts: Vec<Script> = olo_js::page_scripts(&doc, &url)
        .into_iter()
        .filter_map(|(source, name, module)| match source {
            Source::Inline(text) => Some(Script { source: text, name, module }),
            Source::External(u) => {
                // A URL under the page's directory maps to a file next to it.
                let parsed = url::Url::parse(&u).ok()?;
                let path = if parsed.scheme() == "file" {
                    parsed.to_file_path().ok()?
                } else if parsed.host_str() == page_url.host_str() {
                    dir.join(parsed.path().trim_start_matches('/'))
                } else {
                    let mut p = dir.join("ext").join(parsed.host_str()?);
                    p.push(parsed.path().trim_start_matches('/'));
                    p
                };
                match std::fs::read_to_string(&path) {
                    Ok(text) => Some(Script { source: Arc::from(text), name: u, module }),
                    Err(e) => {
                        eprintln!("skip {u}: {e} ({})", path.display());
                        None
                    }
                }
            }
        })
        .collect();
    eprintln!("{} scripts", scripts.len());
    let config = Config { url: url.clone(), quirks: parsed.quirks == html::QuirksMode::Quirks, ..Default::default() };
    let mut realm = Realm::new(doc, config, None, None).unwrap();
    let report = realm.run_page(&scripts);
    if let Some(js) = eval {
        match realm.run(&js, "--eval") {
            Ok(()) => {}
            Err(e) => eprintln!("eval: {e}"),
        }
    }
    let (doc, _) = realm.finish();
    let after = doc.memory();
    println!(
        "scripts {} errors {} console {} timers {} listeners {} layouts {} ({:.1}ms) wall {:.1}ms",
        report.scripts,
        report.errors.len(),
        report.console.len(),
        report.timers,
        report.listeners,
        report.layouts,
        report.layout_ms,
        report.wall_ms
    );
    println!("document revision {} -> {} nodes {:?} -> {:?}", report.revision_before, report.revision_after, before, after);
    for line in &report.console {
        println!("console: {line}");
    }
    for e in &report.errors {
        println!("error: {e}");
    }
    if let Some(out) = out {
        let mut text = String::new();
        let mut budget = 64 << 20;
        olo_js::html::serialize(&doc, doc.root(), &mut text, &mut budget).unwrap();
        std::fs::write(out, text).unwrap();
    }
}
