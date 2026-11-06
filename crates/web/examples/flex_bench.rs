//! Cost of the new single-line worker stage; no existing page-layout counterpart.
use olo_web::layout::flex::{Item, Job};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{hint::black_box, time::Instant};
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "200".into()).parse().unwrap();
    assert!(rounds > 0);
    let mut rows = Vec::new();
    for count in [4, 32, 256, 4096] {
        for growing in [true, false] {
            let items: Vec<_> = (0..count)
                .map(|i| Item { base: 64., minimum: if i % 7 == 0 { 72. } else { 12. }, maximum: if i % 5 == 0 { 50. } else { f64::INFINITY }, grow: 1. + (i % 3) as f64, shrink: 1., outer: 4. })
                .collect();
            let available = count as f64 * if growing { 100. } else { 35. };
            let run = || {
                let input = items.clone(); // caller preparation excluded from ctor/slice timing
                let at = Instant::now();
                let mut job = Job::new(input, available, 7., 1., Default::default(), None).unwrap();
                let ctor = at.elapsed().as_secs_f64() * 1e6;
                let mut step = 0f64;
                let mut payload = 0;
                loop {
                    let at = Instant::now();
                    let progress = job.step(128).unwrap();
                    step = step.max(at.elapsed().as_secs_f64() * 1e6);
                    payload = payload.max(job.working_payload_bytes());
                    if progress.finished {
                        break;
                    }
                }
                let progress = job.progress();
                (job.finish().unwrap(), ctor, step, payload, progress)
            };
            let (warm, _, _, payload, progress) = run();
            let mut hash = Sha256::new();
            for size in warm.sizes() {
                assert!(size.is_finite() && *size >= 0.);
                hash.update(size.to_le_bytes());
            }
            hash.update(warm.remaining().to_le_bytes());
            let digest = format!("{:x}", hash.finalize());
            let mut ctor = 0f64;
            let mut step = 0f64;
            let at = Instant::now();
            for _ in 0..rounds {
                let (line, c, s, _, _) = run();
                ctor = ctor.max(c);
                step = step.max(s);
                black_box(line);
            }
            rows.push(json!({"items":count,"growing":growing,"work":progress.work,"freeze_rounds":progress.rounds,"digest":digest,"input_and_scratch_payload_bytes":payload,"published_size_bytes":count*8,"prepare_measure_drop_us":at.elapsed().as_secs_f64()*1e6/rounds as f64,"max_ctor_us":ctor,"max_128_visit_step_us":step}));
        }
    }
    println!(
        "{}",
        json!({"scope":"new definite single-line worker stage; no parent page-layout counterpart. Total includes scalar-input clone and result drop; ctor/steps exclude clone. Payload excludes capacities/headers/allocator overhead and the benchmark input template. No DOM, text sizing, GPU or presentation.","rounds":rounds,"job_bytes":size_of::<Job<'static>>(),"item_bytes":size_of::<Item>(),"rows":rows})
    );
}
