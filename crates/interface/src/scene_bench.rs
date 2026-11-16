//! What a drawing of many lines costs the scene, alone (`#[ignore]`d: run
//! it under `perf stat`, release, before and after a change to
//! `Scene::insert_segments`, `changed_since` or `append`):
//!
//!   cargo test --release -p olo-interface --no-run
//!   perf stat -e instructions:u TEST-BINARY --ignored scene_bench
//!
//! A screen saver's curve of 1,500 pieces, recorded 2,000 times, each
//! compared with the last (the same, as half its frames are) and put in
//! a frame drawing part of the window (what changed, which cuts it), as
//! the stage does.

use crate::{Bounds, ContentMask, Hsla, Point, ScaledPixels, point, scene::Scene, scene::Segment, size};

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds { origin: point(ScaledPixels(x), ScaledPixels(y)), size: size(ScaledPixels(w), ScaledPixels(h)) }
}

#[test]
#[ignore]
fn scene_bench() {
    let pieces: Vec<(Point<ScaledPixels>, Point<ScaledPixels>)> = (0..1500)
        .map(|i| {
            let (t, u) = (i as f32 * 0.0042, (i + 1) as f32 * 0.0042);
            let at = |t: f32| point(ScaledPixels(1700.0 + 1500.0 * (3.0 * t).cos()), ScaledPixels(1110.0 + 1100.0 * (2.0 * t).sin()));
            (at(t), at(u))
        })
        .collect();
    let mask = ContentMask { bounds: rect(0.0, 0.0, 3456.0, 2234.0) };
    let (half, reach) = (ScaledPixels(1.2), ScaledPixels(1.2 + 1.0 + 1.2));
    let make = || {
        pieces.iter().map(|&(a, b)| Segment {
            order: 0,
            round: 1,
            bounds: Bounds::from_corners(point(a.x.min(b.x) - reach, a.y.min(b.y) - reach), point(a.x.max(b.x) + reach, a.y.max(b.y) + reach)),
            content_mask: mask.clone(),
            color: Hsla { h: 0.5, s: 0.8, l: 0.5, a: 1.0 },
            from: a,
            to: b,
            half_width: half,
            bottom: ScaledPixels(0.0),
        })
    };
    // (As the painter hands them over.)
    let insert = |s: &mut Scene| s.insert_segments(make());
    let mut last = Scene::recording();
    insert(&mut last);
    let (mut frame, mut out, mut same) = (Scene::default(), Vec::new(), 0);
    for _ in 0..2000 {
        let mut s = Scene::recording();
        s.reserve_like(&last);
        insert(&mut s);
        out.clear();
        same += (s.changed_since(&last, &mut out) && out.is_empty()) as usize;
        frame.clear();
        frame.append(&s, point(ScaledPixels(0.0), ScaledPixels(0.0)), &rect(0.0, 74.0, 3456.0, 2160.0));
        last = s;
    }
    assert_eq!(same, 2000);
    assert!(frame.segments.len() > 1000);
}
