//! What saving a session costs as it works: a 730 KB session (about what
//! fits before compaction) read and saved, then read, appended to and
//! saved, as each event does. Run on a release build:
//! `cargo test --release -p olo-coder-core --test store_cost -- --ignored --nocapture`
//! (TMPDIR on disk, not tmpfs, for real writes).
use olo_coder_core::{Config, Session, store::Store};
#[test]
#[ignore]
fn store_cost() {
    let dir = &std::env::temp_dir().join(format!("olo-coder-store-cost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(dir);
    let mut db = Store::open(dir).unwrap();
    let mut s = Session::new("/var/tmp".into(), Config::default());
    for i in 0..400 {
        s.context.push(serde_json::json!({"type":"function_call_output","call_id":format!("c{i}"),"output":"x".repeat(1800)}));
    }
    s.context_refs = vec![0; s.context.len()];
    let size = serde_json::to_string(&s).unwrap().len();
    db.save(&mut s, "created", serde_json::json!({})).unwrap();
    let n = 50;
    let t = std::time::Instant::now();
    for _ in 0..n {
        let mut s = db.get(&s.id).unwrap();
        db.save(&mut s, "boundary", serde_json::json!({})).unwrap();
    }
    let per = t.elapsed() / n;
    let t = std::time::Instant::now();
    for _ in 0..n {
        let _ = db.get(&s.id).unwrap();
    }
    let get = t.elapsed() / n;
    let wchar = || std::fs::read_to_string("/proc/self/io").unwrap().lines().find_map(|l| l.strip_prefix("wchar: ").map(|v| v.parse::<u64>().unwrap())).unwrap();
    let written = wchar();
    let t = std::time::Instant::now();
    for i in 0..n {
        let mut s = db.get(&s.id).unwrap();
        s.context.push(serde_json::json!({"type":"function_call_output","call_id":format!("n{i}"),"output":"y".repeat(1800)}));
        db.save(&mut s, "tool_result", serde_json::json!({})).unwrap();
    }
    let append = t.elapsed() / n;
    let per_save = (wchar() - written) / n as u64 / 1024;
    println!("COST session {} KB: get+save {:?}, get {:?}, get+append+save {:?}, written {} KB a save", size / 1024, per, get, append, per_save);
    let _ = std::fs::remove_dir_all(dir);
}
