//! Numeric compilation/evaluation only; no style/layout/UI or Chrome.
use olo_web::css::{
    self, Syntax,
    math::{self, Context, Dimension, Options, RelativeUnit, Scratch, Value},
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
struct Host;
impl Context for Host {
    fn factor(&self, unit: RelativeUnit) -> math::Result<f64> {
        match unit {
            RelativeUnit::Percentage => Ok(10.),
            RelativeUnit::Em | RelativeUnit::Rem => Ok(16.),
            RelativeUnit::Vw => Ok(12.),
            _ => Err(math::Error::ContextUnavailable),
        }
    }
}
fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(20);
    assert!(rounds > 0);
    let mut cases = Vec::new();
    for source in [
        "12px".to_owned(),
        "0em".into(),
        "calc(1 + 2 * 3)".into(),
        "clamp(12px, 2em + 1vw, 50px)".into(),
        "calc(50% - 1em + 2px)".into(),
        "calc(0.5 + sign(1em - 10px) * 0.1)".into(),
        "calc(sin(pi / 6) * 2 + hypot(3, 4))".into(),
        format!("min({})", (0..256).map(|i| format!("{i}px")).collect::<Vec<_>>().join(",")),
        format!("calc(1em{})", " + 1px".repeat(1000)),
    ] {
        let syntax = Syntax::parse(Arc::from(source.clone()), css::Limits::default(), None).unwrap();
        let options = Options { percentage_hint: Some(Dimension::Length), ..Default::default() };
        let start = Instant::now();
        let value = Value::parse(syntax.components(), options, Default::default(), None).unwrap();
        let first_compile_us = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds * 100 {
            black_box(Value::parse(black_box(syntax.components()), options, Default::default(), None).unwrap());
        }
        let compile_us = start.elapsed().as_secs_f64() * 1e6 / (rounds * 100) as f64;
        let mut scratch = Scratch::default();
        let start = Instant::now();
        let result = value.resolve_with(black_box(&Host), &mut scratch, Default::default(), None).unwrap();
        let first_resolve_us = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds * 1000 {
            black_box(value.resolve_with(black_box(&Host), &mut scratch, Default::default(), None).unwrap());
        }
        let resolve_us = start.elapsed().as_secs_f64() * 1e6 / (rounds * 1000) as f64;
        let memory = value.memory();
        cases.push(json!({"source":if source.len()<150 { source.clone() } else { format!("{}… ({} bytes)",&source[..50],source.len()) },"source_bytes":source.len(),"first_compile_us":first_compile_us,"compile_us":compile_us,"first_resolve_us":first_resolve_us,"resolve_us":resolve_us,"result":result.value,"instructions":memory.instructions,"instruction_size":memory.instruction_size,"stack":memory.stack,"accounted_reserved_bytes":memory.accounted_reserved_bytes}));
    }
    let syntax = Syntax::parse(Arc::from("calc(1 + 2 * 3)"), css::Limits::default(), None).unwrap();
    let values: Vec<_> = (0..10000).map(|_| Value::parse(syntax.components(), Default::default(), Default::default(), None).unwrap()).collect();
    let reserved = values.capacity() * size_of::<Value>() + values.iter().map(|v| v.memory().accounted_reserved_bytes).sum::<usize>();
    black_box(values);
    println!("{}",serde_json::to_string_pretty(&json!({"scope":"Typed math from prebuilt CSS components and raw CPU resolution; no CSSOM/style/layout/UI/JS/network or Chrome comparison","rounds":rounds,"baseline_math_folding":cfg!(feature="baseline-math-folding"),"baseline_math_sums":cfg!(feature="baseline-math-sums"),"value_size":size_of::<Value>(),"ten_thousand_separately_compiled_static_values_reserved_bytes":reserved,"cases":cases})).unwrap());
}
