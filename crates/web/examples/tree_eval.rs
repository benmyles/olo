#[path = "../tests/support/tree_fixtures.rs"]
mod fixtures;

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        eprintln!("usage: tree_eval REPORT.json");
        std::process::exit(2);
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt-tree");
    let report = fixtures::evaluate(&dir);
    std::fs::write(&args[0], serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", report["counts"]);
    if report["counts"]["passed"].as_u64().unwrap_or(0) != report["total"].as_u64().unwrap() {
        std::process::exit(1);
    }
}
