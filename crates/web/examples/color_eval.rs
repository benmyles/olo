#[path = "../tests/support/color_fixtures.rs"]
mod fixtures;
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/color-fixtures");
    let generic = fixtures::evaluate(&dir);
    let extra = fixtures::evaluate_extra(&dir);
    let mut counts = std::collections::BTreeMap::<String, u64>::new();
    for report in [&generic, &extra] {
        for (key, value) in report["counts"].as_object().unwrap() {
            *counts.entry(key.clone()).or_default() += value.as_u64().unwrap();
        }
    }
    let report = serde_json::json!({"scope":"Native CSS color grammar/conversion only; no style/layout/rendering or Chrome conformance", "total":generic["total"].as_u64().unwrap()+extra["total"].as_u64().unwrap(),"counts":counts,"generic":generic,"extra":extra});
    let path = std::env::args().nth(1).expect("report path under /var/tmp");
    std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", report["counts"]);
    let checked = report["counts"]["passed"].as_u64().unwrap_or(0) + report["counts"]["updated_specification"].as_u64().unwrap_or(0);
    if checked != report["total"].as_u64().unwrap() {
        std::process::exit(1);
    }
}
