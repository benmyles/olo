#[path = "../tests/support/property_fixtures.rs"]
mod fixtures;
fn main() {
    let report = fixtures::evaluate(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/property-fixtures"));
    std::fs::write(std::env::args().nth(1).expect("report path under /var/tmp"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", report["counts"]);
    if report["counts"]["passed"] != report["total"] {
        std::process::exit(1);
    }
}
