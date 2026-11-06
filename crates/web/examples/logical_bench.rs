use olo_web::css::{
    Syntax,
    cascade::{Axes, Builder, Defaulted, Priority},
    math,
    properties::{Declaration, Property as P, Value},
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "1000000".into()).parse().unwrap();
    let mut cases = Vec::new();
    for (name, source, property, expected) in [
        ("one-physical", "width:10px", P::Width, 10.),
        ("both-aliases", "width:10px; inline-size:20px", P::Width, 20.),
        ("logical-side", "direction:rtl; writing-mode:sideways-lr; margin-inline:3px 7px", P::MarginTop, 3.),
        ("pending-shorthand", "--m:3px 7px; margin-inline:var(--m)", P::MarginRight, 7.),
        ("rollback", "width:10px; inline-size:revert-rule", P::Width, 10.),
    ] {
        let syntax = Syntax::parse(Arc::from(source), Default::default(), None).unwrap();
        let declarations: Vec<_> = syntax.declarations(None).unwrap().iter().map(|d| Declaration::compile(d, Default::default(), None).unwrap()).collect();
        let mut b = Builder::new(Default::default(), None);
        for (i, d) in declarations.iter().enumerate() {
            b.add(d, Priority { declaration: i as u32, rule: i as u64 + 1, ..Default::default() }).unwrap();
        }
        let cascade = b.finish(false, None).unwrap();
        let mut resolver = cascade.resolver();
        let first = Instant::now();
        let axes = resolver.axes(None, true).unwrap();
        let first_axes_ns = first.elapsed().as_nanos();
        let start = Instant::now();
        let result = resolver.projected(property, axes, true).unwrap();
        let first_query_ns = start.elapsed().as_nanos();
        let Defaulted::Specified(Value::Numeric(n)) = result.value else { panic!() };
        assert_eq!(n.resolve(&math::NoContext, &mut Default::default(), Default::default(), None).unwrap(), expected);
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(resolver.projected(black_box(property), black_box(axes), true).unwrap());
        }
        cases.push(json!({"name":name,"ns_per_query":start.elapsed().as_secs_f64()*1e9/rounds as f64,"first_axes_ns":first_axes_ns,"first_query_ns":first_query_ns,"snapshot_payload_bytes":cascade.reserved_payload_bytes(),"pending_cached":resolver.pending_cached()}));
    }
    println!(
        "{}",
        json!({"rounds":rounds,"axes_bytes":size_of::<Axes>(),"cases":cases,"scope":"Paired defaulting on frozen snapshots. Axes computed once per snapshot; first calls reported separately. Literal scalar checks outside timings. No layout, shaping, UI, website or Chrome evidence."})
    );
}
