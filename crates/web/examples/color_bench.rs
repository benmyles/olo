//! Typed values and CPU color conversion only, not style/layout/painting.
use olo_web::css::{
    self, Syntax,
    color::{Color, Context, Conversion, Limits, Scheme, Space, Value},
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
struct Host;
impl Context for Host {
    fn scheme(&self) -> Scheme {
        Scheme::Dark
    }
    fn current_color(&self) -> Result<Color, css::color::Error> {
        Ok(Color::new(Space::Srgb, [0.2, 0.3, 0.4], 1., 0))
    }
}
fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(20);
    assert!(rounds > 0);
    let mut parsing = Vec::new();
    for source in ["#369c", "rebeccapurple", "rgb(30% 60% 90% / 70%)", "oklch(60% 0.25 150 / 0.8)", "color(display-p3 1 0.5 -0.2)", "light-dark(#fff,light-dark(currentColor,oklch(.2 .1 20)))"] {
        let syntax = Syntax::parse(Arc::from(source), css::Limits::default(), None).unwrap();
        let first = Instant::now();
        let value = Value::parse(syntax.components(), Limits::default(), None).unwrap();
        let first_ns = first.elapsed().as_nanos();
        let start = Instant::now();
        for _ in 0..rounds * 1000 {
            black_box(Value::parse(black_box(syntax.components()), Limits::default(), None).unwrap());
        }
        let parse_ns = start.elapsed().as_secs_f64() * 1e9 / (rounds * 1000) as f64;
        let start = Instant::now();
        for _ in 0..rounds * 1000 {
            black_box(value.resolve(black_box(&Host), Limits::default(), None).unwrap());
        }
        let resolve_ns = start.elapsed().as_secs_f64() * 1e9 / (rounds * 1000) as f64;
        parsing.push(json!({"source":source,"first_parse_ns":first_ns,"parse_ns":parse_ns,"resolve_ns":resolve_ns,"expression_nodes":value.expression_nodes(),"expression_reserved_bytes":value.expression_reserved_bytes()}));
    }
    let count = 262144;
    let mut conversion = Vec::new();
    for (source, target) in
        [(Space::Srgb, Space::Srgb), (Space::DisplayP3Linear, Space::SrgbLinear), (Space::XyzD50, Space::DisplayP3Linear), (Space::Oklab, Space::Srgb), (Space::DisplayP3, Space::Srgb)]
    {
        let original: Vec<_> = (0..count).map(|i| Color::new(source, [(i % 251) as f64 / 250., (i % 179) as f64 / 178., (i % 113) as f64 / 112.], 0.7, 0)).collect();
        let mut values = original.clone();
        let convert = |values: &mut [Color]| {
            let mut worker = Conversion::new(black_box(values), black_box(target));
            while !worker.step(4096, None).unwrap().finished() {}
        };
        let first = Instant::now();
        let mut worker = Conversion::new(&mut values, target);
        let mut step_us = Vec::new();
        loop {
            let start = Instant::now();
            let progress = worker.step(4096, None).unwrap();
            step_us.push(start.elapsed().as_secs_f64() * 1e6);
            if progress.finished() {
                break;
            }
        }
        let first_ms = first.elapsed().as_secs_f64() * 1e3;
        let first_step_us = step_us[0];
        let max_step_us = step_us.into_iter().fold(0., f64::max);
        let mut samples = Vec::new();
        for _ in 0..rounds {
            values.copy_from_slice(&original);
            let start = Instant::now();
            convert(&mut values);
            samples.push(start.elapsed().as_secs_f64() * 1e3);
        }
        black_box(values);
        conversion
            .push(json!({"from":format!("{source:?}"),"to":format!("{target:?}"),"count":count,"first_ms":first_ms,"first_step_us":first_step_us,"max_step_us":max_step_us,"samples_ms":samples}));
    }
    let pool: Vec<_> = (0..10000).map(|i| Value::Absolute(Color::new(Space::Oklab, [i as f64 / 10000., 0., 0.], 1., 0))).collect();
    let retained = pool.capacity() * size_of::<Value>();
    black_box(pool);
    println!("{}",serde_json::to_string_pretty(&json!({"scope":"Typed colors and allocation-free CPU conversion worker; no style/layout/GPU/UI/network/JS or Chrome comparison","rounds":rounds,"f64_storage":cfg!(feature="baseline-color-f64"),"scalar_conversion":cfg!(feature="baseline-color-conversion"),"color_size":size_of::<Color>(),"value_size":size_of::<Value>(),"ten_thousand_absolute_values_reserved_bytes":retained,"parsing":parsing,"conversion":conversion})).unwrap());
}
