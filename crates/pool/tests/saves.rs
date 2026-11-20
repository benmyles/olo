//! What's written of the pool, and when: a change of the accounts a
//! moment later; a request's bookkeeping (when used, what it used) not
//! at once (each request asked for two saves, so with agents at work the
//! sealed file was written more than once a second: 2026-11-15); all of
//! it at a flush, before a handover's new copy reads the file.

use std::time::{Duration, SystemTime};

use olo_token_pool::{
    gateway::Shared,
    model::{Account, Provider},
    store::Store,
    upstream::Upstreams,
};

#[tokio::test(flavor = "multi_thread")]
async fn bookkeeping_waits_changes_dont_and_a_flush_writes_all() {
    let dir = std::path::PathBuf::from(format!("/var/tmp/olo-pool-saves-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: this test alone uses the variable.
    unsafe { std::env::set_var("OLO_POOL_KEYFILE", dir.join("key")) };
    let path = dir.join("pool.vault");
    let (store, mut saved) = Store::open_at(path.clone()).unwrap();
    saved.accounts.push(Account::new("claude-a".into(), "claude-a".into(), Provider::Claude));
    store.save(&saved).unwrap();
    let shared = Shared::new(saved, Some(Store::open_at(path.clone()).unwrap().0), Upstreams::fake("http://127.0.0.1:9"), || {});
    let written = || std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = written();

    // Requests' bookkeeping alone: not written at once.
    for _ in 0..10 {
        shared.noted();
    }
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(written(), before, "a request's bookkeeping isn't written at once");

    // An account's change: written a moment later (with what waited),
    // within a deadline (a save syncs the file and its folder, which a
    // disk busy with a build's writes can make slow: once, past a fixed
    // 800 ms).
    shared.touch();
    let t = std::time::Instant::now();
    while written() == before && t.elapsed() < Duration::from_secs(10) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let changed = written();
    assert!(changed > before, "an account's change is written");

    // Waiting bookkeeping, then a flush: written before it returns.
    tokio::time::sleep(Duration::from_millis(20)).await;
    shared.noted();
    tokio::task::block_in_place(|| shared.flush());
    assert!(written() > changed, "a flush writes what waits");
    let _ = std::fs::remove_dir_all(&dir);
}

/// How long a save takes (sealed, written, the file and its folder synced):
/// `cargo test --release -p olo-token-pool --test saves save_time -- --ignored --nocapture`.
#[test]
#[ignore]
fn save_time() {
    let dir = std::path::PathBuf::from(format!("/var/tmp/olo-pool-save-time-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: this test alone uses the variable.
    unsafe { std::env::set_var("OLO_POOL_KEYFILE", dir.join("key")) };
    let (store, mut saved) = Store::open_at(dir.join("pool.vault")).unwrap();
    for i in 0..8 {
        saved.accounts.push(Account::new(format!("claude-{i}"), format!("claude-{i}"), Provider::Claude));
    }
    let mut took: Vec<f64> = (0..60)
        .map(|_| {
            let t = std::time::Instant::now();
            store.save(&saved).unwrap();
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    took.sort_by(f64::total_cmp);
    println!("a save: median {:.2} ms, 90th {:.2} ms, most {:.2} ms", took[30], took[54], took[59]);
    let _ = std::fs::remove_dir_all(&dir);
}
