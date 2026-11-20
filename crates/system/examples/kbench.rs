//! kbench: small measurements for judging a kernel setting, made to run
//! in a KVM guest (tools/dev/kvm-guest; tools/dev/kernel-check runs
//! them). Nothing of Olo runs it. Each prints a line of JSON.
//!
//!   kbench io PATH [--secs N] [--threads N] [--bs BYTES] [--write] [--buffered]
//!       random reads (or writes) of BYTES over a file or device, each
//!       timed: operations a second, megabytes a second, and how long
//!       one takes (the middle, 99 in 100, the worst), in microseconds.
//!   kbench wake [--secs N]
//!       a thread that sleeps a millisecond over and over: how late it
//!       wakes (the middle, 99 in 100, 999 in 1000, the worst), in
//!       microseconds. What something waiting to run is made to wait.
//!   kbench relay [--secs N] [--every MICROSECONDS]
//!       one thread wakes another through a pipe and goes on working (a
//!       program sending an event to another): how long until the other
//!       runs (the middle, 99 in 100, 999 in 1000, the worst), in
//!       microseconds.
//!   kbench spin [--secs N] [--threads N] [--nice N]
//!       threads that only compute: how much they got done (rounds a
//!       second, all together). A build's place in a measurement, and
//!       what the build loses to whatever else is measured.
//!   kbench chat [--secs N] [--groups N]
//!       groups of four writers and four readers passing 100-byte
//!       messages through pipes: messages a second. Work that is all
//!       waking and waiting, where a kernel that switches more eagerly
//!       gets less done.
//!   kbench hog --mb N [--rate MB-A-SECOND] [--hold SECS] [--random BYTES] [--then-walk]
//!       memory taken and written to, N megabytes of it, then held; of
//!       each page BYTES are random (2 unless said: 2048 is memory that
//!       compresses to half). How long a megabyte took to get (the
//!       middle, 99 in 100, the worst), in milliseconds. With
//!       --then-walk, a second line after the hold: every page read
//!       again (a program that sat idle, used again), how long that
//!       took in milliseconds and how many pages had to be brought
//!       back (major faults: from swap).
//!   kbench touch PATH|--own --mb N [--secs N]
//!       a program's working set: N megabytes of PATH mapped (its code
//!       and files), or of memory of its own (--own: what it has made),
//!       a page in every 64 read ten times a second; how long a round
//!       takes (the middle, the worst), in milliseconds, and how many
//!       rounds were got through. A round that waits on the disk, or on
//!       memory being unpacked, is a program that has stopped answering.
//!   kbench writer PATH [--secs N]
//!       a small write made durable (fsync) ten times a second: how long
//!       one takes (the middle, the worst), in milliseconds. What saving
//!       a document waits for.
//!   kbench fill PATH --mb N
//!       N megabytes written to PATH through the page cache: how long
//!       the writing took, how much was still unwritten in memory when
//!       it returned (Dirty and Writeback in /proc/meminfo), and how
//!       long making it durable (fsync) took then.
//!   kbench splitlock [--count N]
//!       N atomic additions across a cache line's edge (a "split lock"):
//!       how long each took, in microseconds.
//!   kbench pressure [--secs N] [--stall MS] [--window MS]
//!       how often the kernel says programs were held for memory (a
//!       pressure-stall trigger, `olo_system::memory_pressure::watch`:
//!       MS of stall within a window, 100 in 2000 unless said), for N
//!       seconds: how many times it said so, and when first and last.
//!       Root (the guest's).

use std::{
    fs::OpenOptions,
    io::{Seek as _, SeekFrom, Write as _},
    os::{fd::AsRawFd as _, unix::fs::OpenOptionsExt as _},
    time::{Duration, Instant},
};

fn opt(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn num<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    opt(args, flag).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// (the middle, 99 in 100, 999 in 1000, the worst) of `samples`.
fn spread(samples: &mut [f64]) -> (f64, f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() as f64 - 1.0) * q).round() as usize];
    (at(0.5), at(0.99), at(0.999), samples[samples.len() - 1])
}

/// A cheap random number (xorshift).
fn next(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

fn meminfo(what: &str) -> u64 {
    std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix(what)?.strip_prefix(':')?.split_whitespace().next()?.parse().ok())).unwrap_or(0)
}

/// A buffer whose address is a multiple of 4096 (what O_DIRECT wants).
struct Aligned(*mut u8, usize);

impl Aligned {
    fn new(len: usize) -> Aligned {
        // SAFETY: an aligned allocation of `len` bytes, freed in Drop.
        let p = unsafe { std::alloc::alloc_zeroed(std::alloc::Layout::from_size_align(len, 4096).expect("a layout")) };
        assert!(!p.is_null());
        Aligned(p, len)
    }

    fn slice(&mut self) -> &mut [u8] {
        // SAFETY: allocated with this length, and ours alone.
        unsafe { std::slice::from_raw_parts_mut(self.0, self.1) }
    }
}

impl Drop for Aligned {
    fn drop(&mut self) {
        // SAFETY: as allocated.
        unsafe { std::alloc::dealloc(self.0, std::alloc::Layout::from_size_align(self.1, 4096).expect("a layout")) }
    }
}

fn io(args: &[String]) {
    let path = args[0].clone();
    let (secs, threads, bs) = (num(args, "--secs", 5.0f64), num(args, "--threads", 1usize), num(args, "--bs", 4096usize));
    let (write, direct) = (args.iter().any(|a| a == "--write"), !args.iter().any(|a| a == "--buffered"));
    let size = {
        let mut f = OpenOptions::new().read(true).open(&path).expect("the file or device");
        f.seek(SeekFrom::End(0)).expect("its size")
    };
    let blocks = (size / bs as u64).max(1);
    let until = Instant::now() + Duration::from_secs_f64(secs);
    let workers: Vec<_> = (0..threads)
        .map(|t| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut o = OpenOptions::new();
                o.read(true).write(write);
                if direct {
                    o.custom_flags(libc::O_DIRECT);
                }
                let f = o.open(&path).expect("the file or device");
                let mut buf = Aligned::new(bs);
                let (mut x, mut took) = (0x9E37_79B9_7F4A_7C15u64 ^ (t as u64 + 1), Vec::new());
                while Instant::now() < until {
                    let at = (next(&mut x) % blocks) * bs as u64;
                    let t0 = Instant::now();
                    // SAFETY: pread/pwrite of the buffer's own length at an offset within the file.
                    let n = unsafe {
                        if write { libc::pwrite(f.as_raw_fd(), buf.slice().as_ptr().cast(), bs, at as i64) } else { libc::pread(f.as_raw_fd(), buf.slice().as_mut_ptr().cast(), bs, at as i64) }
                    };
                    assert!(n == bs as isize, "an operation at {at} gave {n}: {}", std::io::Error::last_os_error());
                    took.push(t0.elapsed().as_secs_f64() * 1e6);
                }
                took
            })
        })
        .collect();
    let mut all: Vec<f64> = workers.into_iter().flat_map(|w| w.join().expect("a worker")).collect();
    let n = all.len();
    let (p50, p99, p999, max) = spread(&mut all);
    println!(
        "{{\"what\":\"io\",\"write\":{write},\"direct\":{direct},\"bs\":{bs},\"threads\":{threads},\"ops_per_sec\":{:.0},\"mb_per_sec\":{:.1},\"us_p50\":{p50:.1},\"us_p99\":{p99:.1},\"us_p999\":{p999:.1},\"us_max\":{max:.1}}}",
        n as f64 / secs,
        n as f64 * bs as f64 / 1e6 / secs
    );
}

fn wake(args: &[String]) {
    let secs = num(args, "--secs", 5.0f64);
    let until = Instant::now() + Duration::from_secs_f64(secs);
    let mut late = Vec::new();
    while Instant::now() < until {
        let t0 = Instant::now();
        std::thread::sleep(Duration::from_millis(1));
        // (How much past the millisecond asked for, less the timer's own
        // slack: a sleeping thread's 50 microseconds.)
        late.push((t0.elapsed().as_secs_f64() * 1e6 - 1000.0).max(0.0));
    }
    let n = late.len();
    let (p50, p99, p999, max) = spread(&mut late);
    println!("{{\"what\":\"wake\",\"wakes\":{n},\"us_p50\":{p50:.0},\"us_p99\":{p99:.0},\"us_p999\":{p999:.0},\"us_max\":{max:.0}}}");
}

/// A pipe's two ends.
fn pipe() -> (i32, i32) {
    let mut ends = [0i32; 2];
    // SAFETY: two descriptors written into the array.
    assert!(unsafe { libc::pipe(ends.as_mut_ptr()) } == 0, "{}", std::io::Error::last_os_error());
    (ends[0], ends[1])
}

/// Nanoseconds on the clock that never goes back.
fn now_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: the clock read into our own timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn relay(args: &[String]) {
    let (secs, every) = (num(args, "--secs", 5.0f64), num(args, "--every", 2000u64));
    let (rd, wr) = pipe();
    let reader = std::thread::spawn(move || {
        let (mut took, mut sent) = (Vec::new(), [0u8; 8]);
        // SAFETY: eight bytes read into our own buffer, until the pipe closes.
        while unsafe { libc::read(rd, sent.as_mut_ptr().cast(), 8) } == 8 {
            took.push((now_ns() - u64::from_ne_bytes(sent)) as f64 / 1e3);
        }
        took
    });
    let (until, mut x) = (Instant::now() + Duration::from_secs_f64(secs), 0x2545_F491_4F6C_DD1Du64);
    while Instant::now() < until {
        // Asleep for about `every` (never the same twice, so the wake
        // falls anywhere in the kernel's tick), then the message, then
        // work of our own for a while: the other is woken by a thread
        // that keeps running, as a program is by another's event.
        std::thread::sleep(Duration::from_micros(every / 2 + next(&mut x) % every));
        let sent = now_ns().to_ne_bytes();
        // SAFETY: eight bytes written from our own buffer.
        unsafe { libc::write(wr, sent.as_ptr().cast(), 8) };
        let busy = Instant::now();
        while busy.elapsed() < Duration::from_micros(150) {
            std::hint::spin_loop();
        }
    }
    // SAFETY: our own descriptor, closed once.
    unsafe { libc::close(wr) };
    let mut took = reader.join().expect("the reader");
    let n = took.len();
    let (p50, p99, p999, max) = spread(&mut took);
    println!("{{\"what\":\"relay\",\"wakes\":{n},\"us_p50\":{p50:.0},\"us_p99\":{p99:.0},\"us_p999\":{p999:.0},\"us_max\":{max:.0}}}");
}

fn spin(args: &[String]) {
    let (secs, threads, nice) = (num(args, "--secs", 5.0f64), num(args, "--threads", 1usize), num(args, "--nice", 0i32));
    let until = Instant::now() + Duration::from_secs_f64(secs);
    let workers: Vec<_> = (0..threads)
        .map(|t| {
            std::thread::spawn(move || {
                // SAFETY: this thread's own niceness (Linux: a thread's).
                unsafe { libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as u32, nice) };
                let (mut x, mut rounds) = (t as u64 + 1, 0u64);
                while Instant::now() < until {
                    for _ in 0..10_000 {
                        std::hint::black_box(next(&mut x));
                    }
                    rounds += 1;
                }
                rounds
            })
        })
        .collect();
    let rounds: u64 = workers.into_iter().map(|w| w.join().expect("a worker")).sum();
    println!("{{\"what\":\"spin\",\"threads\":{threads},\"rounds_per_sec\":{:.0}}}", rounds as f64 / secs);
}

fn chat(args: &[String]) {
    let (secs, groups) = (num(args, "--secs", 5.0f64), num(args, "--groups", 4usize));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (mut readers, mut writers) = (Vec::new(), Vec::new());
    for _ in 0..groups {
        // Four pipes a group; each writer writes to all four in turn.
        let pipes: Vec<(i32, i32)> = (0..4).map(|_| pipe()).collect();
        for &(rd, _) in &pipes {
            readers.push(std::thread::spawn(move || {
                let (mut buf, mut got) = ([0u8; 100], 0u64);
                loop {
                    // SAFETY: a hundred bytes read into our own buffer.
                    let n = unsafe { libc::read(rd, buf.as_mut_ptr().cast(), 100) };
                    if n <= 0 {
                        return got;
                    }
                    got += n as u64;
                }
            }));
        }
        for _ in 0..4 {
            let (stop, ends): (_, Vec<i32>) = (stop.clone(), pipes.iter().map(|p| p.1).collect());
            writers.push(std::thread::spawn(move || {
                let buf = [7u8; 100];
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for &wr in &ends {
                        // SAFETY: a hundred bytes written from our own buffer.
                        unsafe { libc::write(wr, buf.as_ptr().cast(), 100) };
                    }
                }
                ends
            }));
        }
    }
    std::thread::sleep(Duration::from_secs_f64(secs));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut ends: Vec<i32> = writers.into_iter().flat_map(|w| w.join().expect("a writer")).collect();
    ends.sort_unstable();
    ends.dedup();
    for wr in ends {
        // SAFETY: each writing end closed once, so the readers end.
        unsafe { libc::close(wr) };
    }
    let bytes: u64 = readers.into_iter().map(|r| r.join().expect("a reader")).sum();
    println!("{{\"what\":\"chat\",\"groups\":{groups},\"messages_per_sec\":{:.0}}}", bytes as f64 / 100.0 / secs);
}

fn hog(args: &[String]) {
    let (mb, rate, hold, random) = (num(args, "--mb", 100usize), num(args, "--rate", 0.0f64), num(args, "--hold", 0.0f64), num(args, "--random", 2usize).min(4096));
    let t0 = Instant::now();
    let mut held: Vec<Vec<u8>> = Vec::new();
    let mut took = Vec::new();
    for i in 0..mb {
        let c0 = Instant::now();
        let mut chunk = vec![0u8; 1 << 20];
        // (Written to, a page at a time, and not all the same: memory
        // that's really taken, and of which a compressor can fold away
        // only what isn't random.)
        let mut x = i as u64 + 1;
        for page in chunk.chunks_mut(4096) {
            for b in &mut page[..random] {
                *b = next(&mut x) as u8;
            }
        }
        took.push(c0.elapsed().as_secs_f64() * 1e3);
        held.push(chunk);
        if rate > 0.0 {
            let ahead = (i + 1) as f64 / rate - t0.elapsed().as_secs_f64();
            if ahead > 0.0 {
                std::thread::sleep(Duration::from_secs_f64(ahead));
            }
        }
    }
    let (p50, p99, _, max) = spread(&mut took);
    println!("{{\"what\":\"hog\",\"mb\":{mb},\"secs\":{:.2},\"ms_p50\":{p50:.2},\"ms_p99\":{p99:.2},\"ms_max\":{max:.1}}}", t0.elapsed().as_secs_f64());
    std::thread::sleep(Duration::from_secs_f64(hold));
    if args.iter().any(|a| a == "--then-walk") {
        // (This process's major faults: the 12th field of its stat line.)
        let faults = || std::fs::read_to_string("/proc/self/stat").ok().and_then(|t| t.rsplit(')').next()?.split_whitespace().nth(9)?.parse::<u64>().ok()).unwrap_or(0);
        let (before, w0) = (faults(), Instant::now());
        let mut sum = 0u64;
        for chunk in &held {
            for page in chunk.chunks(4096) {
                sum = sum.wrapping_add(std::hint::black_box(page[0]) as u64);
            }
        }
        std::hint::black_box(sum);
        println!("{{\"what\":\"walk\",\"mb\":{mb},\"ms\":{:.1},\"brought_back\":{}}}", w0.elapsed().as_secs_f64() * 1e3, faults() - before);
    }
    drop(held);
}

fn touch(args: &[String]) {
    let path = &args[0];
    let (mb, secs) = (num(args, "--mb", 200usize), num(args, "--secs", 20.0f64));
    let len = mb << 20;
    let bytes = if path == "--own" {
        // SAFETY: memory of our own, private, unmapped at exit.
        let map = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
        assert!(map != libc::MAP_FAILED, "{}", std::io::Error::last_os_error());
        let (bytes, mut x) = (map.cast::<u8>(), 99u64);
        // (Half of every page random: memory that compresses to half.)
        for at in 0..len / 2 {
            // SAFETY: within the mapping.
            unsafe { bytes.add((at / 2048) * 4096 + at % 2048).write(next(&mut x) as u8) };
        }
        bytes
    } else {
        let f = OpenOptions::new().read(true).open(path).expect("the file");
        // SAFETY: a read-only private mapping of the file, unmapped at exit.
        let map = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, f.as_raw_fd(), 0) };
        assert!(map != libc::MAP_FAILED, "{}", std::io::Error::last_os_error());
        map.cast::<u8>()
    };
    let (t0, mut rounds, mut took) = (Instant::now(), 0u64, Vec::new());
    let mut sum = 0u64;
    while t0.elapsed().as_secs_f64() < secs {
        let r0 = Instant::now();
        // A page in every 64, a different lot each round: all of the
        // working set in 64 rounds (6.4 s).
        let first = (rounds % 64) as usize * 4096;
        let mut at = first;
        while at < len {
            // SAFETY: within the mapping.
            sum = sum.wrapping_add(unsafe { bytes.add(at).read_volatile() } as u64);
            at += 64 * 4096;
        }
        took.push(r0.elapsed().as_secs_f64() * 1e3);
        rounds += 1;
        std::thread::sleep(Duration::from_millis(100).saturating_sub(r0.elapsed()));
    }
    let (p50, p99, _, max) = spread(&mut took);
    println!("{{\"what\":\"touch\",\"mb\":{mb},\"rounds\":{rounds},\"rounds_expected\":{:.0},\"ms_p50\":{p50:.2},\"ms_p99\":{p99:.2},\"ms_max\":{max:.1},\"sum\":{sum}}}", secs * 10.0);
}

fn writer(args: &[String]) {
    let path = &args[0];
    let secs = num(args, "--secs", 20.0f64);
    let mut f = OpenOptions::new().create(true).write(true).truncate(true).open(path).expect("the file");
    let (t0, mut took) = (Instant::now(), Vec::new());
    while t0.elapsed().as_secs_f64() < secs {
        let w0 = Instant::now();
        f.seek(SeekFrom::Start(0)).expect("the start");
        f.write_all(&[0x5au8; 16384]).expect("a write");
        f.sync_all().expect("fsync");
        took.push(w0.elapsed().as_secs_f64() * 1e3);
        std::thread::sleep(Duration::from_millis(100).saturating_sub(w0.elapsed()));
    }
    let n = took.len();
    let (p50, p99, _, max) = spread(&mut took);
    println!("{{\"what\":\"writer\",\"writes\":{n},\"writes_expected\":{:.0},\"ms_p50\":{p50:.2},\"ms_p99\":{p99:.2},\"ms_max\":{max:.1}}}", secs * 10.0);
}

fn fill(args: &[String]) {
    let path = &args[0];
    let mb = num(args, "--mb", 500usize);
    let mut f = OpenOptions::new().create(true).write(true).truncate(true).open(path).expect("the file");
    let chunk = vec![0x5au8; 1 << 20];
    let t0 = Instant::now();
    for _ in 0..mb {
        f.write_all(&chunk).expect("a write");
    }
    let wrote = t0.elapsed().as_secs_f64();
    let unwritten = (meminfo("Dirty") + meminfo("Writeback")) / 1024;
    let t1 = Instant::now();
    f.sync_all().expect("fsync");
    println!("{{\"what\":\"fill\",\"mb\":{mb},\"write_secs\":{wrote:.2},\"unwritten_mb_after\":{unwritten},\"sync_secs\":{:.2}}}", t1.elapsed().as_secs_f64());
}

fn splitlock(args: &[String]) {
    use std::sync::atomic::{AtomicU32, Ordering};
    let count = num(args, "--count", 200usize);
    // Two cache lines, and a 4-byte atomic that begins two bytes before
    // the edge between them: every addition to it is a split lock.
    let mut lines = Aligned::new(4096);
    // SAFETY: within the buffer; an AtomicU32 has no alignment the
    // processor needs (x86), which is the point.
    let split = unsafe { &*lines.slice().as_mut_ptr().add(62).cast::<AtomicU32>() };
    let mut took = Vec::new();
    let t0 = Instant::now();
    for _ in 0..count {
        let s0 = Instant::now();
        split.fetch_add(1, Ordering::SeqCst);
        took.push(s0.elapsed().as_secs_f64() * 1e6);
    }
    let all = t0.elapsed().as_secs_f64();
    let (p50, p99, _, max) = spread(&mut took);
    println!("{{\"what\":\"splitlock\",\"count\":{count},\"secs\":{all:.3},\"us_p50\":{p50:.1},\"us_p99\":{p99:.1},\"us_max\":{max:.1}}}");
}

fn pressure(args: &[String]) {
    use std::sync::{Arc, Mutex};
    let secs = num(args, "--secs", 30u64);
    let (stall, window) = (num(args, "--stall", 100u64), num(args, "--window", 2000u64));
    let t0 = Instant::now();
    let told: Arc<Mutex<Vec<f64>>> = Arc::default();
    let heard = told.clone();
    if let Err(e) = olo_system::memory_pressure::watch(Duration::from_millis(stall), Duration::from_millis(window), move || {
        heard.lock().unwrap().push(t0.elapsed().as_secs_f64());
        true
    }) {
        println!("{{\"what\":\"pressure\",\"error\":\"{e}\"}}");
        return;
    }
    std::thread::sleep(Duration::from_secs(secs));
    let told = told.lock().unwrap();
    println!(
        "{{\"what\":\"pressure\",\"secs\":{secs},\"stall_ms\":{stall},\"window_ms\":{window},\"told\":{},\"first_s\":{:.1},\"last_s\":{:.1}}}",
        told.len(),
        told.first().copied().unwrap_or(-1.0),
        told.last().copied().unwrap_or(-1.0)
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("io") => io(rest),
        Some("wake") => wake(rest),
        Some("relay") => relay(rest),
        Some("spin") => spin(rest),
        Some("chat") => chat(rest),
        Some("hog") => hog(rest),
        Some("touch") => touch(rest),
        Some("writer") => writer(rest),
        Some("fill") => fill(rest),
        Some("splitlock") => splitlock(rest),
        Some("pressure") => pressure(rest),
        _ => {
            eprintln!("kbench io|wake|relay|spin|chat|hog|touch|writer|fill|splitlock … (see the top of crates/system/examples/kbench.rs)");
            std::process::exit(2);
        }
    }
    let _ = std::io::stdout().flush();
}
