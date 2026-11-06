use olo_web::layout::{
    self,
    flex::{Item, Job},
};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};

// The solver consumes already-used lengths. These fixtures separately define
// ordinary numeric content-box bases/decorations, not auto/intrinsic CSS sizes.
fn used(value: f64, scale: f64) -> f64 {
    (value * scale * 64.).trunc() / (scale * 64.)
}
fn border(value: f64, scale: f64) -> f64 {
    if value == 0. { 0. } else { (value * scale).floor().max(1.) / scale }
}
#[test]
fn definite_flex_lines_match_original_chrome_at_five_native_scales() {
    let cases: serde_json::Value = serde_json::from_str(include_str!("flex-fixtures/cases.json")).unwrap();
    let reference: serde_json::Value = serde_json::from_str(include_str!("flex-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(include_bytes!("flex-fixtures/cases.json"))), reference["cases_sha256"]);
    let mut results = Vec::new();
    let mut failures = Vec::new();
    for row in reference["rows"].as_array().unwrap() {
        let case = cases.as_array().unwrap().iter().find(|c| c["id"] == row["id"]).unwrap();
        let scale = row["scale"].as_f64().unwrap();
        let items: Vec<_> = case["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                let sum = |key| i[key].as_array().unwrap().iter().map(|x| used(x.as_f64().unwrap(), scale)).sum::<f64>();
                let borders = i["border"].as_array().unwrap().iter().map(|x| border(x.as_f64().unwrap(), scale)).sum::<f64>();
                Item {
                    base: used(i["base"].as_f64().unwrap(), scale),
                    minimum: used(i["min"].as_f64().unwrap(), scale),
                    maximum: i["max"].as_f64().map_or(f64::INFINITY, |v| used(v, scale)),
                    grow: i["grow"].as_f64().unwrap(),
                    shrink: i["shrink"].as_f64().unwrap(),
                    outer: sum("margin") + sum("padding") + borders,
                }
            })
            .collect();
        let mut job = Job::new(items, used(case["available"].as_f64().unwrap(), scale), used(case["gap"].as_f64().unwrap(), scale), scale, Default::default(), None).unwrap();
        while !job.step(1).unwrap().finished {}
        let line = job.finish().unwrap();
        // This original Chrome case requests only 37.5% of its initial spare
        // space; the remainder must survive for later alignment/auto margins.
        if row["id"] == "partial-grow" {
            assert_eq!(line.remaining(), 237.5);
        }
        assert_eq!(line.sizes().len(), row["items"].as_array().unwrap().len());
        let expected: Vec<_> = row["items"].as_array().unwrap().iter().map(|x| x["content"].as_f64().unwrap()).collect();
        let passed = line.sizes().iter().zip(&expected).all(|(a, b)| (a - b).abs() < 0.0001);
        let result = serde_json::json!({"id":row["id"],"scale":scale,"got":line.sizes(),"expected":expected,"remaining":line.remaining(),"passed":passed});
        if !passed {
            failures.push(result.clone());
        }
        results.push(result);
    }
    if let Ok(path) = std::env::var("OLO_FLEX_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
    }
    assert_eq!(results.len(), 320);
    assert!(failures.is_empty(), "{}", serde_json::to_string_pretty(&failures).unwrap());
}
#[test]
fn flex_work_is_bounded_cancellable_and_not_published_early() {
    let items = vec![Item { base: 100., grow: 1., ..Default::default() }; 3];
    let job = Job::new(items.clone(), 301., 0., 1., Default::default(), None).unwrap();
    assert!(matches!(job.finish(), Err(layout::Error::Incomplete)));
    let mut job = Job::new(items.clone(), 301., 0., 1., layout::Limits { work: 0, ..Default::default() }, None).unwrap();
    assert!(matches!(job.step(1), Err(layout::Error::Limit)));
    assert!(matches!(job.step(1), Err(layout::Error::Failed)));
    assert!(matches!(job.finish(), Err(layout::Error::Failed)));
    let cancel = AtomicBool::new(false);
    let mut job = Job::new(items.clone(), 301., 0., 1., Default::default(), Some(&cancel)).unwrap();
    assert!(!job.step(1).unwrap().finished);
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(job.step(1), Err(layout::Error::Cancelled)));
    assert!(matches!(job.finish(), Err(layout::Error::Cancelled)));
    for budget in [1, 3, 128] {
        let mut job = Job::new(items.clone(), 301., 0., 1., Default::default(), None).unwrap();
        while !job.step(budget).unwrap().finished {}
        assert_eq!(job.finish().unwrap().sizes(), &[100.328125, 100.34375, 100.328125]);
    }
    for invalid in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(matches!(Job::new(items.clone(), 301., 0., invalid, Default::default(), None), Err(layout::Error::Invalid)));
    }
    assert!(matches!(Job::new(items, 301., 0., 1., layout::Limits { boxes: 0, ..Default::default() }, None), Err(layout::Error::Limit)));
}

#[test]
fn hostile_flex_inputs_fail_without_partial_geometry_or_unbounded_arithmetic() {
    for item in [
        Item { base: f64::NAN, ..Default::default() },
        Item { base: f64::INFINITY, ..Default::default() },
        Item { minimum: -1., ..Default::default() },
        Item { maximum: f64::NAN, ..Default::default() },
        Item { grow: f64::INFINITY, ..Default::default() },
        Item { outer: f64::NAN, ..Default::default() },
    ] {
        let mut job = Job::new(vec![item], 100., 0., 1., Default::default(), None).unwrap();
        assert!(matches!(job.step(128), Err(layout::Error::Invalid)));
        assert!(matches!(job.finish(), Err(layout::Error::Failed)));
    }
    let mut job = Job::new(vec![Item { grow: f64::MAX, ..Default::default() }], 100., 0., 1., Default::default(), None).unwrap();
    assert!(matches!(job.step(128), Err(layout::Error::Limit)));
    let extent = (1u64 << 52) as f64 / 64.;
    let mut job = Job::new(vec![Item { outer: -extent, ..Default::default() }; 2], 100., 0., 1., Default::default(), None).unwrap();
    assert!(matches!(job.step(128), Err(layout::Error::Limit)));
    assert!(matches!(job.finish(), Err(layout::Error::Failed)));
}
