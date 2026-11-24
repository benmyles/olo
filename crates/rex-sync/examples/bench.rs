//! Benchmark table for `olo-rex-sync` over `UnixStream::pair()`:
//!   1. Nothing changed in a tree of 5,000 files
//!   2. 200 MB JSONL transcript with 4 KiB appended
//!   3. One byte changed in the middle of a 50 MB structured binary file
//!   4. Rename of a 50 MB file
//!   5. 500 new small text files (~2 KiB each)
//!
//! Run with: `cargo run --release -p olo-rex-sync --example bench`

use std::fs;
use std::io::{BufWriter, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use olo_rex_sync::{Filter, FolderSource, Index, Options, Report, receive, send};

struct BenchRow {
    name: &'static str,
    change: String,
    report: Report,
    elapsed: Duration,
}

fn run_sync(src: &Path, dst: &Path, src_cache: &Path, dst_cache: &Path, opts: Options) -> (Report, Duration) {
    let (a, b) = UnixStream::pair().expect("UnixStream::pair");
    let src_dir = src.to_path_buf();
    let dst_dir = dst.to_path_buf();
    let sc = src_cache.to_path_buf();
    let dc = dst_cache.to_path_buf();
    let send_opts = opts.clone();
    let mut recv_opts = opts;
    recv_opts.cache_path = Some(dc);

    let start = Instant::now();
    let t = thread::spawn(move || {
        let source = FolderSource { root: src_dir, filter: send_opts.filter.clone(), cache_path: Some(sc) };
        send(a, &source, send_opts).expect("send")
    });
    let recv_rep = receive(b, &dst_dir, recv_opts).expect("receive");
    let send_rep = t.join().expect("sender thread");
    let elapsed = start.elapsed();

    assert!(recv_rep.errors.is_empty(), "errors: {:?}", recv_rep.errors);
    assert_eq!(send_rep.bytes_sent, recv_rep.bytes_received);
    assert_eq!(send_rep.bytes_received, recv_rep.bytes_sent);
    (send_rep, elapsed)
}

fn write_jsonl(path: &Path, target_bytes: usize, start_turn: usize) -> usize {
    let f = fs::File::create(path).expect("create jsonl");
    let mut w = BufWriter::with_capacity(256 << 10, f);
    let mut written = 0usize;
    let mut i = start_turn;
    while written < target_bytes {
        let line = format!(
            "{{\"ts\":{},\"type\":\"event\",\"session\":\"sess-bench-0042\",\"turn\":{},\"role\":\"assistant\",\"tool\":\"read_file\",\"path\":\"/home/me/src/app/src/wire.rs\",\"tokens\":{},\"text\":\"Analyzing delta recipe piece {} and checking zstd reference prefix compression ratio.\"}}\n",
            1_760_000_000_000u64 + i as u64 * 137,
            i,
            120 + (i % 512),
            i
        );
        w.write_all(line.as_bytes()).expect("write jsonl");
        written += line.len();
        i += 1;
    }
    w.flush().expect("flush jsonl");
    i
}

fn append_jsonl(path: &Path, target_bytes: usize, start_turn: usize) -> usize {
    let f = fs::OpenOptions::new().append(true).open(path).expect("open jsonl for append");
    let mut w = BufWriter::new(f);
    let mut written = 0usize;
    let mut i = start_turn;
    while written < target_bytes {
        let line = format!(
            "{{\"ts\":{},\"type\":\"event\",\"session\":\"sess-bench-0042\",\"turn\":{},\"role\":\"assistant\",\"tool\":\"read_file\",\"path\":\"/home/me/src/app/src/wire.rs\",\"tokens\":{},\"text\":\"Analyzing delta recipe piece {} and checking zstd reference prefix compression ratio.\"}}\n",
            1_760_000_000_000u64 + i as u64 * 137,
            i,
            120 + (i % 512),
            i
        );
        w.write_all(line.as_bytes()).expect("append jsonl");
        written += line.len();
        i += 1;
    }
    w.flush().expect("flush append");
    written
}

fn write_structured_binary(path: &Path, target_bytes: usize) {
    let f = fs::File::create(path).expect("create binary");
    let mut w = BufWriter::with_capacity(256 << 10, f);
    let mut written = 0usize;
    let mut seq = 0u64;
    let mut rec = [0u8; 64];
    while written < target_bytes {
        rec[0..4].copy_from_slice(b"OLOB");
        rec[4..12].copy_from_slice(&seq.to_le_bytes());
        // Deterministic mixed header + structured table fields (binary-like, not pure noise).
        let mix = seq.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left((seq & 31) as u32);
        rec[12..20].copy_from_slice(&mix.to_le_bytes());
        rec[20..24].copy_from_slice(&((seq % 4096) as u32).to_le_bytes());
        rec[24..28].copy_from_slice(&((seq / 64) as u32).to_le_bytes());
        rec[28..32].copy_from_slice(&0x0001_0004u32.to_le_bytes());
        rec[32..48].copy_from_slice(b"rex-sync-record:");
        rec[48..56].copy_from_slice(&(seq.wrapping_mul(31)).to_le_bytes());
        rec[56..64].copy_from_slice(&[0x00, 0x00, 0x01, 0x00, 0xFF, 0x00, 0xAA, 0x55]);
        let take = (target_bytes - written).min(rec.len());
        w.write_all(&rec[..take]).expect("write binary");
        written += take;
        seq += 1;
    }
    w.flush().expect("flush binary");
}

fn fmt_bytes(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.2} MiB ({n} B)", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.2} KiB ({n} B)", n as f64 / (1 << 10) as f64)
    } else {
        format!("{n} B")
    }
}

fn main() {
    let base = PathBuf::from(format!("/var/tmp/olo-rex-sync-bench-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("create bench dir");

    let mut rows = Vec::new();

    // 1. Nothing changed in a tree of 5,000 files.
    {
        let root = base.join("noop5k");
        let (src, dst, caches) = (root.join("src"), root.join("dst"), root.join("cache"));
        fs::create_dir_all(&src).unwrap();
        for d in 0..50 {
            let sub = src.join(format!("dir_{d:02}"));
            fs::create_dir_all(&sub).unwrap();
            for f in 0..100 {
                fs::write(sub.join(format!("file_{f:03}.txt")), format!("dir {d} file {f}: sample contents for 5000-file tree benchmark\n")).unwrap();
            }
        }
        let sc = caches.join("src.idx");
        let dc = caches.join("dst.idx");
        let (r0, _) = run_sync(&src, &dst, &sc, &dc, Options::default());
        assert_eq!(r0.new, 5000);

        let (rep, elapsed) = run_sync(&src, &dst, &sc, &dc, Options::default());
        assert_eq!(rep.same, 5000);
        rows.push(BenchRow { name: "Nothing changed (5,000 files)", change: "0 B".into(), report: rep, elapsed });
        let _ = fs::remove_dir_all(&root);
    }

    // 2. 200 MB JSONL file with 4 KiB appended.
    {
        let root = base.join("append200m");
        let (src, dst, caches) = (root.join("src"), root.join("dst"), root.join("cache"));
        fs::create_dir_all(&src).unwrap();
        let next_turn = write_jsonl(&src.join("session.jsonl"), 200 << 20, 0);

        let sc = caches.join("src.idx");
        let dc = caches.join("dst.idx");
        let (r0, _) = run_sync(&src, &dst, &sc, &dc, Options::default());
        assert_eq!(r0.new, 1);

        let appended = append_jsonl(&src.join("session.jsonl"), 4096, next_turn);
        let (rep, elapsed) = run_sync(&src, &dst, &sc, &dc, Options::default());
        assert_eq!(rep.appended, 1);
        assert_eq!(fs::metadata(src.join("session.jsonl")).unwrap().len(), fs::metadata(dst.join("session.jsonl")).unwrap().len());
        rows.push(BenchRow { name: "200 MB JSONL + 4 KiB append", change: format!("+{appended} B"), report: rep, elapsed });
        let _ = fs::remove_dir_all(&root);
    }

    // 3. One byte changed in the middle of a 50 MB structured binary file.
    {
        let root = base.join("edit50m");
        let (src, dst, caches) = (root.join("src"), root.join("dst"), root.join("cache"));
        fs::create_dir_all(&src).unwrap();
        let file = src.join("records.bin");
        write_structured_binary(&file, 50 << 20);

        let sc = caches.join("src.idx");
        let dc = caches.join("dst.idx");
        run_sync(&src, &dst, &sc, &dc, Options::default());

        // Flip 1 byte in the middle in place.
        {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = fs::OpenOptions::new().read(true).write(true).open(&file).unwrap();
            let mid = (50u64 << 20) / 2;
            f.seek(SeekFrom::Start(mid)).unwrap();
            let mut b = [0u8; 1];
            f.read_exact(&mut b).unwrap();
            b[0] ^= 0xFF;
            f.seek(SeekFrom::Start(mid)).unwrap();
            f.write_all(&b).unwrap();
        }

        let (rep, elapsed) = run_sync(&src, &dst, &sc, &dc, Options::default());
        assert_eq!(rep.patched, 1);
        assert_eq!(Index::scan(&src, &Filter::all(), None).unwrap().tree_hash(), Index::scan(&dst, &Filter::all(), None).unwrap().tree_hash());
        rows.push(BenchRow { name: "1 byte changed in 50 MB binary", change: "1 B edit".into(), report: rep, elapsed });
        let _ = fs::remove_dir_all(&root);
    }

    // 4. Rename of a 50 MB file.
    {
        let root = base.join("rename50m");
        let (src, dst, caches) = (root.join("src"), root.join("dst"), root.join("cache"));
        fs::create_dir_all(&src).unwrap();
        write_structured_binary(&src.join("release-v1.bin"), 50 << 20);

        let sc = caches.join("src.idx");
        let dc = caches.join("dst.idx");
        let opts = Options { delete_extra: true, ..Options::default() };
        run_sync(&src, &dst, &sc, &dc, opts.clone());

        fs::rename(src.join("release-v1.bin"), src.join("release-v2.bin")).unwrap();
        let (rep, elapsed) = run_sync(&src, &dst, &sc, &dc, opts);
        assert_eq!(rep.copied, 1);
        assert_eq!(rep.removed, 1);
        assert!(!dst.join("release-v1.bin").exists());
        assert!(dst.join("release-v2.bin").exists());
        rows.push(BenchRow { name: "Rename of a 50 MB file", change: "50 MiB rename".into(), report: rep, elapsed });
        let _ = fs::remove_dir_all(&root);
    }

    // 5. 500 new small files (~2 KiB of text each).
    {
        let root = base.join("small500");
        let (src, dst, caches) = (root.join("src"), root.join("dst"), root.join("cache"));
        fs::create_dir_all(&src).unwrap();
        let mut total_raw = 0usize;
        for i in 0..500 {
            let mut text = String::with_capacity(2200);
            text.push_str(&format!("// module_{i:03}.rs - generated benchmark source\n"));
            for line in 0..32 {
                text.push_str(&format!("pub fn handle_item_{i}_{line}(val: u64) -> u64 {{ val.wrapping_add({}) }}\n", i * 37 + line));
            }
            total_raw += text.len();
            fs::write(src.join(format!("mod_{i:03}.rs")), text).unwrap();
        }

        let sc = caches.join("src.idx");
        let dc = caches.join("dst.idx");
        let (rep, elapsed) = run_sync(&src, &dst, &sc, &dc, Options::default());
        assert_eq!(rep.new, 500);
        assert_eq!(Index::scan(&src, &Filter::all(), None).unwrap().tree_hash(), Index::scan(&dst, &Filter::all(), None).unwrap().tree_hash());
        rows.push(BenchRow { name: "500 new small text files (~2 KiB each)", change: fmt_bytes(total_raw as u64), report: rep, elapsed });
        let _ = fs::remove_dir_all(&root);
    }

    let _ = fs::remove_dir_all(&base);

    println!("| Scenario | Change | Sent (S->R) | Recv (R->S) | Total Wire | Data Raw -> Compressed | Time |");
    println!("| --- | --- | ---: | ---: | ---: | --- | ---: |");
    for row in &rows {
        let r = &row.report;
        println!(
            "| {} | {} | {} | {} | {} | {} -> {} | {:.1} ms |",
            row.name,
            row.change,
            fmt_bytes(r.bytes_sent),
            fmt_bytes(r.bytes_received),
            fmt_bytes(r.wire_bytes()),
            fmt_bytes(r.data_bytes_raw),
            fmt_bytes(r.data_bytes_compressed),
            row.elapsed.as_secs_f64() * 1000.0
        );
    }
}
