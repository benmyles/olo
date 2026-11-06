use olo_web::text::prepare::{Collapse, Job, Prepared, Span, WhiteSpace};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn prepare(source: Arc<str>, spans: Arc<[Span]>) -> (Prepared, f64, f64) {
    let at = Instant::now();
    let mut job = Job::new(source, spans, Default::default(), None).unwrap();
    let ctor = at.elapsed().as_secs_f64() * 1e6;
    let mut max_step = 0f64;
    loop {
        let at = Instant::now();
        let p = job.step(4096).unwrap();
        max_step = max_step.max(at.elapsed().as_secs_f64() * 1e6);
        if p.finished {
            break;
        }
    }
    (job.finish().unwrap(), ctor, max_step)
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "100".into()).parse().unwrap();
    assert!(rounds > 0);
    let mut cases = Vec::new();
    for count in [100, 1000] {
        for (scene, unit, expected, mode, split) in [
            ("unchanged", "An unchanged paragraph with ordinary spaces. ", "An unchanged paragraph with ordinary spaces. ", Collapse::Collapse, false),
            ("collapsed", "A  paragraph\t with\n    collapsed whitespace. ", "A paragraph with collapsed whitespace. ", Collapse::Collapse, false),
            ("multilingual", "العربية  עברית\n    हिन्दी\t中文. ", "العربية עברית हिन्दी 中文. ", Collapse::Collapse, false),
            ("pre", "An unchanged\t preformatted paragraph.\n", "An unchanged\t preformatted paragraph.\n", Collapse::Preserve, false),
            ("many-spans", "A  paragraph\t with\n    collapsed whitespace. ", "A paragraph with collapsed whitespace. ", Collapse::Collapse, true),
            ("long-word", "abcdefghijklmnopqrstuvwxyz0123456789", "abcdefghijklmnopqrstuvwxyz0123456789", Collapse::Collapse, false),
        ] {
            let source: Arc<str> = Arc::from(unit.repeat(count));
            let mut spans = Vec::new();
            if split {
                for i in 0..count {
                    spans.push(Span { range: (i * unit.len()) as u32..((i + 1) * unit.len()) as u32, whitespace: WhiteSpace { collapse: mode, wrap: true } });
                }
            } else {
                spans.push(Span { range: 0..source.len() as u32, whitespace: WhiteSpace { collapse: mode, wrap: true } });
            }
            let spans: Arc<[Span]> = Arc::from(spans);
            let at = Instant::now();
            let (first, ctor, first_step) = prepare(source.clone(), spans.clone());
            let cold = at.elapsed().as_secs_f64() * 1e6;
            assert_eq!(&**first.text(), expected.repeat(count));
            let bytes = first.text().len();
            let map_bytes = first.mapping_payload_bytes();
            let maps = first.mappings().len();
            let shared = Arc::ptr_eq(first.text(), first.source());
            let mut maximum = 0f64;
            let mut ctor_max = 0f64;
            drop(first);
            let at = Instant::now();
            for _ in 0..rounds {
                let (p, c, s) = prepare(black_box(source.clone()), black_box(spans.clone()));
                ctor_max = ctor_max.max(c);
                maximum = maximum.max(s);
                black_box(p);
            }
            let mean = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            cases.push(json!({"scene":scene,"source_bytes":source.len(),"output_bytes":bytes,"spans":spans.len(),"mappings":maps,"mapping_payload_bytes":map_bytes,"unchanged_source_shared":shared,"first_prepare_us":cold,"first_ctor_us":ctor,"first_max_4096_byte_step_us":first_step,"prepare_drop_us":mean,"max_ctor_us":ctor_max,"max_4096_byte_step_us":maximum}));
        }
    }
    println!(
        "{}",
        json!({"rounds":rounds,"wide_control":cfg!(feature="baseline-text-map-wide"),"scalar_control":cfg!(feature="baseline-text-prepare"),"mapping_size":Prepared::mapping_record_bytes(),"cases":cases})
    );
}
