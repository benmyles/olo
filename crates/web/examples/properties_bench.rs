use olo_web::css::{
    self, Syntax, math,
    properties::{self as p, Declaration, Value},
    variables,
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn syntax(s: &str) -> Syntax {
    Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap()
}
struct Host {
    spacing: variables::Value,
}
impl variables::Context for Host {
    fn lookup(&self, name: &str) -> Option<variables::Binding<'_>> {
        (name == "--spacing").then_some(variables::Binding { id: 1, value: &self.spacing, animation_tainted: false })
    }
}
struct NumericHost;
impl math::Context for NumericHost {
    fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
        match unit {
            math::RelativeUnit::Em => Ok(16.),
            math::RelativeUnit::Percentage => Ok(10.),
            _ => Err(math::Error::ContextUnavailable),
        }
    }
}
fn main() {
    let rounds = std::env::args().nth(1).map(|s| s.parse::<usize>().unwrap()).unwrap_or(20000);
    let sources = [
        ("width", "320px"),
        ("width", "calc(100% - 2em)"),
        ("margin", "1px 2px 3px 4px"),
        ("border-color", "red blue rgb(0 255 0) transparent"),
        ("display", "inline flex"),
        ("font-weight", "650.5"),
        ("padding", "var(--spacing)"),
        ("width", "var(--missing, 50%)"),
    ];
    let host = Host { spacing: variables::Value::compile(syntax("1px 2px").components(), variables::Limits::default(), None).unwrap() };
    let mut cases = Vec::new();
    for (name, source) in sources {
        let syntax = syntax(source);
        let start = Instant::now();
        let value = Declaration::parse(name, syntax.components(), false, p::Limits::default(), None).unwrap();
        let first_compile_us = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(Declaration::parse(name, syntax.components(), false, p::Limits::default(), None).unwrap());
        }
        let compile_drop_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        let Declaration::Known { values, .. } = &value else { unreachable!() };
        let mut resolver = variables::Resolver::new(&host, variables::Limits::default(), None);
        let mut scratch = math::Scratch::default();
        let resolve = |resolver: &mut variables::Resolver<'_, Host>, scratch: &mut math::Scratch| match &values[0].value {
            Value::Numeric(n) => {
                black_box(n.resolve(&NumericHost, scratch, math::Limits::default(), None).unwrap());
            }
            Value::Pending(v) => {
                black_box(v.resolve(resolver, true, p::Limits::default(), None).unwrap());
            }
            _ => {
                black_box(&values[0].value);
            }
        };
        let start = Instant::now();
        resolve(&mut resolver, &mut scratch);
        let first_resolve_us = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds {
            resolve(&mut resolver, &mut scratch);
        }
        let resolve_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        cases.push(json!({"property":name,"source":source,"compile_drop_us":compile_drop_us,"first_compile_us":first_compile_us,"resolve_us":resolve_us,"first_resolve_us":first_resolve_us,"longhands":values.len(),"assignment_spilled":values.spilled()}));
    }
    let mut retained = Vec::new();
    for (name, source) in [("width", "320px"), ("padding", "1px 2px 3px 4px"), ("width", "var(--spacing)")] {
        let syntax = syntax(source);
        let start = Instant::now();
        let declarations: Vec<_> = (0..1000).map(|_| Declaration::parse(name, syntax.components(), false, p::Limits::default(), None).unwrap()).collect();
        let first_compile_1000_us = start.elapsed().as_secs_f64() * 1e6;
        let mut bytes = declarations.capacity() * size_of::<Declaration>();
        let mut spill = 0;
        let mut value_bytes = 0;
        for d in &declarations {
            if let Declaration::Known { values, .. } = d {
                if values.spilled() {
                    spill += values.capacity() * size_of::<p::Assignment>();
                }
                value_bytes += values[0].value.reserved_bytes();
            }
        }
        bytes += spill + value_bytes;
        retained.push(json!({"property":name,"source":source,"count":1000,"first_compile_1000_us":first_compile_1000_us,"declaration_array_bytes":declarations.capacity()*size_of::<Declaration>(),"assignment_heap_bytes":spill,"unique_value_heap_bytes":value_bytes,"accounted_reserved_bytes":bytes}));
        black_box(declarations);
    }
    println!(
        "{}",
        json!({"scope":"Prebuilt component declaration compilation and isolated numeric or cached substitution resolution, no cascade/style/layout/painting/browser/Chrome evidence","method":"Compile/drop mean over rounds; resolve mean with scratch and variable resolver retained; pending resolution includes typed reparsing and assignment allocation, other variants are reference access controls. Retained batches hold 1000 independent declarations. Bytes exclude allocator metadata and original Syntax. Memory batch cases have independent scalar programs/one pending per declaration, so referenced heaps are unique.","rounds":rounds,"value_size":size_of::<Value>(),"assignment_size":size_of::<p::Assignment>(),"declaration_size":size_of::<Declaration>(),"assignments_size":size_of::<p::Assignments>(),"cases":cases,"retained":retained})
    );
}
