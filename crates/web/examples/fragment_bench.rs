//! Complete-buffer inert fragment parsing and standalone arena storage.
use olo_web::{
    dom::Namespace,
    html::{FragmentContext, ParseOptions, parse_fragment},
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};

fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(10);
    assert!(rounds > 0);
    let mut results = Vec::new();
    for count in [10_000, 20_000, 40_000] {
        for (case, context, namespace, markup) in [
            ("html", "div", Namespace::Html, "<section><p>Text with <a href='/'>link</a>.</p></section>"),
            ("table", "table", Namespace::Html, "<tr><td>one<td>two"),
            ("svg", "svg", Namespace::Svg, "<linearGradient id='paint'/><use xlink:href='#paint'/><title><b>Label</b></title>"),
            ("math", "math", Namespace::MathMl, "<mrow><mi>x</mi><mo>+</mo><mn>2</mn></mrow>"),
            ("template", "template", Namespace::Html, "<tr><td>one<td>two</tr>"),
            ("rcdata", "textarea", Namespace::Html, "&amp; <a>text</a> </textarea>"),
        ] {
            let context = FragmentContext::new(context, namespace);
            let source: Arc<str> = Arc::from(markup.repeat(count));
            let first = Instant::now();
            let parsed = parse_fragment(source.clone(), &context, ParseOptions::default(), None).unwrap();
            let first_ms = first.elapsed().as_secs_f64() * 1000.0;
            let memory = parsed.document.memory();
            assert!(parsed.document.first_child(parsed.fragment).unwrap().is_some());
            assert_eq!(parsed.document.children(parsed.document.root()).unwrap().count(), 0);
            if case != "rcdata" {
                assert_eq!(memory.owned_text_bytes, 0);
            }
            drop(parsed);
            let start = Instant::now();
            for _ in 0..rounds {
                black_box(parse_fragment(source.clone(), &context, ParseOptions::default(), None).unwrap());
            }
            results.push(json!({"case":case,"count":count,"rounds":rounds,"source_bytes":source.len(),"first_parse_ms":first_ms,"parse_and_drop_ms":start.elapsed().as_secs_f64()*1000.0/rounds as f64,"nodes":memory.live_nodes,"attributes":memory.live_attributes,"node_size":memory.node_size,"node_reserved_bytes":memory.node_reserved_bytes,"attribute_reserved_bytes":memory.attribute_reserved_bytes,"owned_text_capacity_bytes":memory.owned_text_bytes}));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"scope":"Static inert fragment construction/storage only; no live-document adoption, Web IDL, scripts, network, layout, UI or Chrome comparison","results":results})
        )
        .unwrap()
    );
}
