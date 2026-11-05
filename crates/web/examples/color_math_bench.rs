//! Contextual numeric color compilation/resolution only, not a browser.
use olo_web::css::{
    self, Syntax,
    color::{Color, Context, Limits, Scheme, Value},
    math::{self, RelativeUnit as U, TreeCount},
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
struct Host;
impl Context for Host {
    fn scheme(&self) -> Scheme {
        Scheme::Dark
    }
    fn math_factor(&self, unit: U) -> math::Result<f64> {
        match unit {
            U::Em => Ok(16.),
            U::Cqw => Ok(10.),
            _ => Err(math::Error::ContextUnavailable),
        }
    }
    fn math_tree_count(&self, kind: TreeCount) -> math::Result<u32> {
        Ok(if kind == TreeCount::SiblingIndex { 3 } else { 4 })
    }
}
fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(20);
    assert!(rounds > 0);
    let sources = [
        "rgb(calc(100 + 20) 0 0)".to_owned(),
        "oklch(calc(25% * 2) .2 calc(1turn + 30deg))".into(),
        "color(srgb calc(50% + sign(1em - 10px) * 10%) 0 0 / .5)".into(),
        "rgb(calc(sign(2cqw - 10px) * 255) 0 0)".into(),
        "color(srgb calc(sibling-index() / sibling-count()) 0 0)".into(),
        "light-dark(Canvas, oklab(calc(.5 + sign(1em - 10px) * .1) .2 .3))".into(),
        format!("rgb(calc((1em{}) / 1px) 0 0)", " + 1px".repeat(1000)),
    ];
    let mut cases = Vec::new();
    for source in &sources {
        let syntax = Syntax::parse(Arc::from(source.as_str()), css::Limits::default(), None).unwrap();
        let start = Instant::now();
        let value = Value::parse(syntax.components(), Limits::default(), None).unwrap();
        let first_parse_us = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds * 100 {
            black_box(Value::parse(black_box(syntax.components()), Limits::default(), None).unwrap());
        }
        let parse_us = start.elapsed().as_secs_f64() * 1e6 / (rounds * 100) as f64;
        let start = Instant::now();
        let color = value.resolve(black_box(&Host), Limits::default(), None).unwrap();
        let first_resolve_us = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds * 1000 {
            black_box(value.resolve(black_box(&Host), Limits::default(), None).unwrap());
        }
        let resolve_us = start.elapsed().as_secs_f64() * 1e6 / (rounds * 1000) as f64;
        cases.push(json!({"source":if source.len() < 150 {source.clone()} else {format!("{}… ({} bytes)",&source[..50],source.len())},"source_bytes":source.len(),"parse_us":parse_us,"resolve_us":resolve_us,"first_parse_us":first_parse_us,"first_resolve_us":first_resolve_us,"extra_retained_bytes":value.expression_reserved_bytes(),"conditional_nodes":value.expression_nodes(),"result":{"space":format!("{:?}",color.space()),"coordinates":color.components(),"alpha":color.alpha()}}));
    }
    let mut pools = Vec::new();
    for source in &sources[..3] {
        let syntax = Syntax::parse(Arc::from(source.as_str()), css::Limits::default(), None).unwrap();
        let values: Vec<_> = (0..10000).map(|_| Value::parse(syntax.components(), Limits::default(), None).unwrap()).collect();
        let reserved = values.capacity() * size_of::<Value>() + values.iter().map(Value::expression_reserved_bytes).sum::<usize>();
        black_box(values);
        pools.push(json!({"source":source,"count":10000,"separately_compiled_reserved_bytes":reserved}));
    }
    println!("{}",serde_json::to_string_pretty(&json!({"scope":"Typed contextual color math from prebuilt syntax with explicit host snapshots; no CSSOM/style/layout/painting/JS/network/UI or Chrome comparison","rounds":rounds,"value_size":size_of::<Value>(),"color_size":size_of::<Color>(),"cases":cases,"pools":pools})).unwrap());
}
