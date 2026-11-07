use olo_web::svg::{Error, Limits, Path, PathJob};
use std::sync::atomic::{AtomicBool, Ordering};
fn compile(source: &str, budget: usize) -> Path {
    let mut job = PathJob::new(source, Limits::default(), None).unwrap();
    while !job.step(budget).unwrap().finished {}
    job.finish().unwrap()
}
fn points(path: &Path) -> Vec<(f32, f32)> {
    path.geometry().unwrap().points().iter().map(|p| (p.x, p.y)).collect()
}
#[test]
fn compact_relative_and_implicit_commands() {
    let source = "m10 20 5-5h5v10l-5 0z l2 3";
    let path = compile(source, 1);
    assert!(!path.malformed());
    assert_eq!(points(&path), [(10., 20.), (15., 15.), (20., 15.), (20., 25.), (15., 25.), (10., 20.), (12., 23.)]);
    assert_eq!(points(&path), points(&compile(source, 128)));
}
#[test]
fn smooth_controls_reflect_only_matching_previous_commands() {
    let path = compile("M0 0 C1 2 3 4 5 6 S7 8 9 10 L10 10 S11 12 13 14 Q15 16 17 18 T19 20 L20 20 T21 22", 1);
    assert_eq!(
        points(&path),
        [
            (0., 0.),
            (1., 2.),
            (3., 4.),
            (5., 6.),
            (7., 8.),
            (7., 8.),
            (9., 10.),
            (10., 10.),
            (10., 10.),
            (11., 12.),
            (13., 14.),
            (15., 16.),
            (17., 18.),
            (19., 20.),
            (19., 20.),
            (20., 20.),
            (20., 20.),
            (21., 22.)
        ]
    );
}
#[test]
fn arcs_degenerate_and_regular() {
    let line = compile("M1 2 A0 10 0 0 1 5 6", 1);
    assert_eq!(points(&line), [(1., 2.), (5., 6.)]);
    let same = compile("M1 2 A10 10 0 0 1 1 2 L3 4", 1);
    assert_eq!(points(&same), [(1., 2.), (3., 4.)]);
    let arc = compile("M10 0 A10 10 0 0 1 0 10", 1);
    let p = points(&arc);
    assert_eq!(p[0], (10., 0.));
    assert!(p.last().unwrap().0.abs() < 0.0001);
    assert!((p.last().unwrap().1 - 10.).abs() < 0.0001);
    // Check the analytic quarter circle, independent of subdivision count.
    let mut start = p[0];
    for cubic in p[1..].as_chunks::<3>().0 {
        for i in 0..=100 {
            let t = i as f64 / 100.;
            let u = 1. - t;
            let x = u * u * u * start.0 as f64 + 3. * u * u * t * cubic[0].0 as f64 + 3. * u * t * t * cubic[1].0 as f64 + t * t * t * cubic[2].0 as f64;
            let y = u * u * u * start.1 as f64 + 3. * u * u * t * cubic[0].1 as f64 + 3. * u * t * t * cubic[1].1 as f64 + t * t * t * cubic[2].1 as f64;
            assert!((x.hypot(y) - 10.).abs() < 0.001);
        }
        start = cubic[2];
    }
    assert_eq!(points(&arc), points(&compile("M10 0 a-10-10 0 0 1-10 10", 128)));
}
#[test]
fn syntax_errors_keep_complete_prefix_and_flag_diagnostic() {
    let path = compile("M0 0 L10 10 20 20 30 broken", 1);
    assert!(path.malformed());
    assert_eq!(points(&path), [(0., 0.), (10., 10.), (20., 20.)]);
    assert!(compile("L1 1", 1).malformed());
    assert!(compile("L1 1", 1).geometry().is_none());
    assert!(compile("M1 1", 1).geometry().is_none());
    assert!(compile("  ", 1).geometry().is_none());
    assert!(!compile("  ", 1).malformed());
}
#[test]
fn quotas_cancellation_and_poisoning() {
    let cancelled = AtomicBool::new(false);
    let mut job = PathJob::new("M0 0L1 1", Limits { segments: 1, ..Limits::default() }, Some(&cancelled)).unwrap();
    assert_eq!(job.step(0).unwrap().segments, 0);
    assert_eq!(job.step(1).unwrap().segments, 1);
    assert_eq!(job.step(1).unwrap_err(), Error::Limit);
    assert_eq!(job.step(1).unwrap_err(), Error::Failed);
    assert!(matches!(job.finish(), Err(Error::Failed)));
    let mut job = PathJob::new("M0 0L1 1", Limits::default(), Some(&cancelled)).unwrap();
    job.step(1).unwrap();
    cancelled.store(true, Ordering::Relaxed);
    assert_eq!(job.step(1).unwrap_err(), Error::Cancelled);
    assert!(matches!(PathJob::new("", Limits::default(), Some(&cancelled)), Err(Error::Cancelled)));
    assert!(matches!(PathJob::new("123", Limits { source_bytes: 2, ..Limits::default() }, None), Err(Error::Limit)));
    assert!(matches!(PathJob::new("", Limits::default(), None).unwrap().finish(), Err(Error::Incomplete)));
    let mut job = PathJob::new("M0 0L1e100 0", Limits::default(), None).unwrap();
    assert_eq!(job.step(100).unwrap_err(), Error::Coordinates);
    let mut job = PathJob::new("M0 0L1 1", Limits { segments: 2, ..Limits::default() }, None).unwrap();
    assert!(job.step(3).unwrap().finished);
    assert_eq!(job.finish().unwrap().payload_bytes(), 18);
}

#[test]
fn arc_expansion_is_incremental_bounded_and_resets_smooth_controls() {
    let path = compile("M0 0Q1 1 2 2 A10 10 0 0 1 2 2T3 3", 1);
    assert_eq!(points(&path), [(0., 0.), (1., 1.), (2., 2.), (2., 2.), (3., 3.)]);
    let mut job = PathJob::new("M1e10 0A1e10 1e10 0 1 1 0 1e10", Limits { segments: 3, ..Limits::default() }, None).unwrap();
    assert_eq!(job.step(1).unwrap().segments, 1);
    assert_eq!(job.step(1).unwrap().segments, 2);
    assert_eq!(job.step(1).unwrap().segments, 3);
    assert_eq!(job.step(1).unwrap_err(), Error::Limit);
    // The geometry backend treats radii <=1e-5 as a line. Reject that
    // precision range explicitly instead of silently losing a nonzero arc.
    for source in ["M0 0A1e-6 1e-6 0 0 1 1e-6 0", "M0 0A1 1 0 0 1 1e-300 0"] {
        let mut job = PathJob::new(source, Limits::default(), None).unwrap();
        assert_eq!(job.step(100).unwrap_err(), Error::Coordinates);
    }
}
