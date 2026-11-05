#[path = "../tests/support/css_fixtures.rs"]
mod fixtures;
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/css-parsing-tests");
    let report = fixtures::evaluate(&dir);
    let path = std::env::args().nth(1).expect("report path under /var/tmp");
    std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", report["counts"]);
    let checked = report["counts"]["passed"].as_u64().unwrap_or(0) + report["counts"]["updated_specification"].as_u64().unwrap_or(0);
    if checked != report["total"].as_u64().unwrap() {
        std::process::exit(1);
    }
}
