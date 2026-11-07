use olo_web::layout::positioned::{self, Axis, Input};
use std::path::Path;
fn input(c: &serde_json::Value) -> Input {
    let horizontal = c["axis"] == "horizontal";
    let rtl = c["rtl"].as_bool().unwrap();
    Input {
        axis: if horizontal { Axis::Horizontal { rtl, static_rtl: rtl } } else { Axis::Vertical },
        containing: if horizontal { 200. } else { 160. },
        inset: [c["inset"][0].as_f64(), c["inset"][1].as_f64()],
        size: c["size"].as_f64(),
        margin: [c["margin"][0].as_f64(), c["margin"][1].as_f64()],
        decoration: c["decoration"].as_f64().unwrap(),
        minimum: c["minimum"].as_f64().unwrap(),
        maximum: c["maximum"].as_f64(),
        intrinsic: [c["intrinsic"][0].as_f64().unwrap(), c["intrinsic"][1].as_f64().unwrap()],
        static_position: 0.,
    }
}
#[test]
fn independent_absolute_axis_constraints() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/positioned-fixtures");
    let bytes = std::fs::read(root.join("cases.json")).unwrap();
    let cases: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let reference: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("chrome.json")).unwrap()).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(bytes)), reference["cases_sha256"]);
    for row in reference["rows"].as_array().unwrap() {
        let c = cases.as_array().unwrap().iter().find(|c| c["id"] == row["id"]).unwrap();
        let scale = row["scale"].as_f64().unwrap();
        let i = input(c);
        let used = positioned::solve(i, scale).unwrap();
        for (actual, expected) in [
            (used.border_start(), row["start"].as_f64().unwrap()),
            (used.size, row["size"].as_f64().unwrap()),
            (used.margin[0], row["margin"][0].as_f64().unwrap()),
            (used.margin[1], row["margin"][1].as_f64().unwrap()),
        ] {
            assert!((actual - expected).abs() < 0.0001, "{} @{scale}: {actual} != {expected}; {used:?}", c["id"]);
        }
        assert!((used.inset.iter().sum::<f64>() + used.margin.iter().sum::<f64>() + used.size + i.decoration - i.containing).abs() < 0.0001);
    }
}
#[test]
fn static_side_and_numeric_limits_are_explicit() {
    let mut i = Input {
        axis: Axis::Horizontal { rtl: false, static_rtl: true },
        containing: 200.,
        inset: [None, None],
        size: Some(50.),
        margin: [None, None],
        decoration: 10.,
        minimum: 0.,
        maximum: None,
        intrinsic: [40., 60.],
        static_position: 17.,
    };
    let used = positioned::solve(i, 1.).unwrap();
    assert_eq!(used.inset, [123., 17.]);
    i.containing = f64::NAN;
    assert!(positioned::solve(i, 1.).is_err());
    i.containing = 1e20;
    assert!(positioned::solve(i, 1.).is_err());
    i.containing = 200.;
    assert!(positioned::solve(i, 0.).is_err());
    i.size = Some(-1.);
    assert!(positioned::solve(i, 1.).is_err());
    i.size = Some(1.);
    i.containing = 1e308;
    i.margin = [Some(-1e308), Some(-1e308)];
    assert!(positioned::solve(i, 1e-300).is_err());
}
