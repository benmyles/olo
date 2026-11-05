#[path = "../tests/support/selector_fixtures.rs"]
mod fixtures;
fn main() {
    let report = fixtures::evaluate(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/selector-fixtures"));
    let path = std::env::args().nth(1).expect("report path under /var/tmp");
    std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", report["counts"]);
    if report["counts"]["passed"].as_u64().unwrap_or(0) + report["counts"]["updated_specification"].as_u64().unwrap_or(0) != report["total"].as_u64().unwrap() {
        std::process::exit(1);
    }
}
