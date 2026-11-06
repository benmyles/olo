use olo_web::css::{
    Syntax,
    cascade::{Builder, Priority},
    properties::{Declaration, Property},
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};

fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "1000000".into()).parse().unwrap();
    let syntax = Syntax::parse(Arc::from("width:10px; margin-left:20px; color:red"), Default::default(), None).unwrap();
    let declarations: Vec<_> = syntax.declarations(None).unwrap().iter().map(|d| Declaration::compile(d, Default::default(), None).unwrap()).collect();
    let mut builder = Builder::new(Default::default(), None);
    for (i, d) in declarations.iter().enumerate() {
        builder.add(d, Priority { declaration: i as u32, ..Default::default() }).unwrap();
    }
    let cascade = builder.finish(false, None).unwrap();
    let mut resolver = cascade.resolver();
    let mut cases = Vec::new();
    for property in [Property::Width, Property::MarginLeft, Property::Color, Property::Height] {
        let start = Instant::now();
        black_box(resolver.property(black_box(property), true).unwrap());
        let first_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(resolver.property(black_box(property), true).unwrap());
        }
        cases.push(json!({"property":property.as_str(), "first_ns":first_ns,"ns_per_query":start.elapsed().as_secs_f64()*1e9/rounds as f64}));
    }
    println!(
        "{}",
        json!({"rounds":rounds,"snapshot_payload_bytes":cascade.reserved_payload_bytes(),"cases":cases,"scope":"Repeated raw declared-property queries on one frozen snapshot; no layout, UI or website evidence."})
    );
}
