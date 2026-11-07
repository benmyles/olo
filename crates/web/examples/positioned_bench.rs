//! Absolute axis arithmetic only; no DOM, layout traversal, paint or UI.
use olo_web::layout::positioned::{self, Axis, Input};
use std::{hint::black_box, time::Instant};
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "1000000".into()).parse().unwrap();
    assert!(rounds > 0);
    let cases: Vec<_> = (0..8)
        .map(|mask| Input {
            axis: Axis::Horizontal { rtl: mask & 1 != 0, static_rtl: false },
            containing: 200.,
            inset: [(mask & 1 != 0).then_some(10.), (mask & 4 != 0).then_some(20.)],
            size: (mask & 2 != 0).then_some(50.),
            margin: [None, None],
            decoration: 10.,
            minimum: 0.,
            maximum: Some(150.),
            intrinsic: [40., 60.],
            static_position: 0.,
        })
        .collect();
    let start = Instant::now();
    let mut sum = 0.;
    for _ in 0..rounds {
        for &case in &cases {
            sum += black_box(positioned::solve(black_box(case), 1.25).unwrap()).border_start();
        }
    }
    println!(
        "{}",
        serde_json::json!({"scope":"Absolute axis scalar arithmetic only; no page integration or speedup claim","calls":rounds*cases.len(),"ns_per_axis":start.elapsed().as_secs_f64()*1e9/(rounds*cases.len())as f64,"checksum":sum,"input_bytes":size_of::<Input>(),"output_bytes":size_of::<positioned::Used>()})
    );
}
