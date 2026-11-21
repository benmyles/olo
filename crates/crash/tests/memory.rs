//! A thread that needed much memory for a moment gives it back
//! (`olo_crash::give_memory_back`, which the compositor calls as it
//! starts). In a process of its own: the allocator's settings are the
//! process's.

/// What this process holds of its own, in bytes (`RssAnon`).
fn held() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status.lines().find_map(|l| l.strip_prefix("RssAnon:")).and_then(|v| v.split_ascii_whitespace().next()).and_then(|kb| kb.parse::<u64>().ok()).unwrap() * 1024
}

/// What decoding a picture does: one large block written and freed,
/// then several a little smaller, written and freed.
fn work() {
    let written = |bytes: usize| {
        let mut block = vec![0u8; bytes];
        for i in (0..bytes).step_by(4096) {
            block[i] = 1;
        }
        std::hint::black_box(block)
    };
    drop(written(24 << 20));
    drop((0..3).map(|_| written(10 << 20)).collect::<Vec<_>>());
}

#[test]
fn a_thread_gives_back_what_it_needed_for_a_moment() {
    const MIB: u64 = 1 << 20;
    let start = held();
    // As the C library comes: the thread's 30 MiB stay with the program
    // (if they don't, there is nothing here for the setting to show).
    std::thread::spawn(work).join().unwrap();
    let kept = held().saturating_sub(start);
    if kept < 20 * MIB {
        eprintln!("skipped: this C library gave back all but {kept} bytes by itself");
        return;
    }
    // Held to its thresholds: the same work again leaves nothing, and
    // what the first left goes back with it.
    assert!(olo_crash::give_memory_back(), "the allocator took the settings");
    std::thread::spawn(work).join().unwrap();
    let kept = held().saturating_sub(start);
    assert!(kept < 2 * MIB, "{kept} bytes still held after the work was done");
}
