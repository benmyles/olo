//! Native token compilation/substitution only; no stylesheet cascade or UI.
use olo_web::css::{
    self, Syntax,
    variables::{Binding, Context, Limits, Resolver, Value},
};
use serde_json::json;
use std::{collections::BTreeMap, hint::black_box, sync::Arc, time::Instant};
struct Host {
    entries: BTreeMap<String, (u64, Value)>,
}
impl Context for Host {
    fn lookup(&self, name: &str) -> Option<Binding<'_>> {
        self.entries.get(name).map(|(id, value)| Binding { id: *id, value, animation_tainted: false })
    }
}
fn syntax(s: &str) -> Syntax {
    Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap()
}
fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(20);
    let cold = std::env::args().nth(2).is_some_and(|s| s == "cold");
    assert!(rounds > 0);
    let mut declarations = vec![
        ("--base", "10px".to_owned()),
        ("--r", "50%".into()),
        ("--delta", "10%".into()),
        ("--name", "--base".into()),
        ("--a", "var(--base)".into()),
        ("--b", "var(--a)".into()),
        ("--left", "var(--a) 2px".into()),
        ("--right", "var(--a) 3px".into()),
        ("--diamond", "var(--left) var(--right)".into()),
        ("--cycle-a", "var(--cycle-b,red)".into()),
        ("--cycle-b", "var(--cycle-a,green)".into()),
        ("--long", "var(--base) ".repeat(1000)),
    ];
    let deep_names: Vec<_> = (0..33).map(|i| format!("--deep{i}")).collect();
    for i in 0..32 {
        declarations.push((&deep_names[i], format!("var({})", deep_names[i + 1])));
    }
    declarations.push((&deep_names[32], "var(--deep0,ok) ".repeat(1000)));
    let trees: Vec<_> = declarations.iter().map(|(_, s)| syntax(s)).collect();
    let values = Value::compile_batch(trees.iter().map(Syntax::components), Limits::default(), None).unwrap();
    let host = Host { entries: declarations.iter().zip(values).enumerate().map(|(id, ((name, _), value))| (name.to_string(), (id as u64, value))).collect() };
    let mut cases = Vec::new();
    for source in [
        "var(--base)",
        "var(--b)",
        "var(var(--name))",
        "var(--missing, rgb(var(--r) 0 0))",
        "rgb(calc(var(--r) + var(--delta)) 0 0)",
        "var(--diamond)",
        "var(--cycle-a,red)",
        "var(--long)",
        "var(--deep0,red)",
    ] {
        let tree = syntax(source);
        let first = Instant::now();
        let value = Value::compile(tree.components(), Limits::default(), None).unwrap();
        let first_compile_us = first.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds * 100 {
            black_box(Value::compile(black_box(tree.components()), Limits::default(), None).unwrap());
        }
        let compile_us = start.elapsed().as_secs_f64() * 1e6 / (rounds * 100) as f64;
        let start = Instant::now();
        let mut resolver = Resolver::new(&host, Limits::default(), None);
        let first_resolver_new_us = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        let result = resolver.resolve(&value, true).unwrap();
        let first_resolve_us = start.elapsed().as_secs_f64() * 1e6;
        let result_source_bytes = result.value().unwrap().source_spelling().len();
        let shared_returned_arena_bytes = result.value().unwrap().arena_reserved_bytes();
        let iterations = rounds * if matches!(source, "var(--long)" | "var(--deep0,red)") { 2 } else { 1000 };
        let start = Instant::now();
        for _ in 0..iterations {
            if cold {
                resolver.clear();
            }
            black_box(resolver.resolve(black_box(&value), true).unwrap());
        }
        let resolve_us = start.elapsed().as_secs_f64() * 1e6 / iterations as f64;
        let stats = resolver.stats();
        cases.push(json!({"source":source,"compile_us":compile_us,"resolve_us":resolve_us,"first_compile_us":first_compile_us,"first_resolve_us":first_resolve_us,"first_resolver_new_us":first_resolver_new_us,"resolve_iterations":iterations,"result_source_bytes":result_source_bytes,"shared_returned_arena_bytes":shared_returned_arena_bytes,"cached_properties":stats.cached_properties,"cached_arena_bytes":stats.cached_arena_bytes,"cache_hits":stats.cache_hits}));
    }
    let inputs: Vec<_> = (0..1000).map(|i| syntax(&format!("{i}px"))).collect();
    let mut batch = Vec::new();
    for independent in [true, false] {
        let start = Instant::now();
        let mut retained = 0;
        for _ in 0..rounds {
            let values: Vec<_> = if independent {
                inputs.iter().map(|s| Value::compile(s.components(), Limits::default(), None).unwrap()).collect()
            } else {
                Value::compile_batch(inputs.iter().map(Syntax::components), Limits::default(), None).unwrap()
            };
            retained = values.capacity() * size_of::<Value>() + if independent { values.iter().map(Value::arena_reserved_bytes).sum::<usize>() } else { values[0].arena_reserved_bytes() };
            black_box(values);
        }
        batch.push(json!({"independent":independent,"count":1000,"compile_drop_us":start.elapsed().as_secs_f64()*1e6/rounds as f64,"accounted_reserved_bytes":retained}));
    }
    println!("{}",serde_json::to_string_pretty(&json!({"scope":"Prebuilt CSS token value compilation and substitution against an explicit immutable winning-property snapshot. No CSSOM, cascade, layout, rendering, JS, network, GPU, UI or Chrome comparison","rounds":rounds,"cold_cache":cold,"method":"Cold control clears results before each public request, retaining worker capacities; warm mode reuses memoized properties. Both modes memoize cycles/dependencies within one request.","value_size":size_of::<Value>(),"batch":batch,"cases":cases})).unwrap());
}
