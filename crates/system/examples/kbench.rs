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
//!   kbench hold [--file MB] [--own MB] [--pin] [--busy] [PATH]
//!       a program asleep that holds memory: MB of a file (PATH,
//!       /d/held unless said) written, mapped and read, locked in
//!       memory with --pin; and MB of its own, written to. Says "ready"
//!       once it holds them, then sleeps until it is killed. What a
//!       keeper must take back by itself: nothing here asks for more.
//!       With `--busy` it doesn't sleep but reads a byte of every 4 KiB
//!       of its own, round after round, 20 ms asleep between two (what
//!       went to swap is brought back into use).
//!   kbench fragment --mb N [--hold SECS]
//!       free memory left in single pages: N megabytes taken and written
//!       to, then every other page of them given back, the rest held. A
//!       2 MB page can then only be had by moving the held ones together
//!       (compaction), as on a computer that has run for days.
//!   kbench huge --mb N [--secs N] [--threads N]
//!       a program that asks for 2 MB pages (`MADV_HUGEPAGE`), as a
//!       linker's and a browser's allocators do: N megabytes mapped,
//!       each 2 MB of it first touched and timed (where the kernel
//!       makes room there and then, the program waits here), then
//!       every page written, then given back; over and over. Megabytes
//!       a second, how long a 2 MB took to get (the middle, 99 in 100,
//!       the worst), in milliseconds, and how much of the last pass
//!       came as huge pages.
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

fn fragment(args: &[String]) {
    let (mb, hold) = (num(args, "--mb", 500usize), num(args, "--hold", 3600.0f64));
    let len = mb << 20;
    // SAFETY: memory of our own, private.
    let map = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
    assert!(map != libc::MAP_FAILED, "{}", std::io::Error::last_os_error());
    // (Never huge pages itself: its pages are to be single ones.)
    // SAFETY: the mapping just made.
    unsafe { libc::madvise(map, len, libc::MADV_NOHUGEPAGE) };
    let bytes = map.cast::<u8>();
    let mut x = 7u64;
    for page in 0..len / 4096 {
        // SAFETY: within the mapping.
        unsafe { bytes.add(page * 4096).write(next(&mut x) as u8) };
    }
    // Every other page back to the kernel: what is free is single pages
    // between the ones kept.
    for page in (0..len / 4096).step_by(2) {
        // SAFETY: a whole page within the mapping.
        unsafe { libc::madvise(bytes.add(page * 4096).cast(), 4096, libc::MADV_DONTNEED) };
    }
    println!("{{\"what\":\"fragment\",\"mb\":{mb},\"held_mb\":{}}}", mb / 2);
    std::thread::sleep(Duration::from_secs_f64(hold));
}

fn huge(args: &[String]) {
    let (mb, secs, threads) = (num(args, "--mb", 128usize), num(args, "--secs", 20.0f64), num(args, "--threads", 1usize).max(1));
    const HUGE: usize = 2 << 20;
    let len = (mb << 20) / HUGE * HUGE;
    let t0 = Instant::now();
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            std::thread::spawn(move || {
                let (mut took, mut passes, mut as_huge) = (Vec::new(), 0u64, 0.0f64);
                while t0.elapsed().as_secs_f64() < secs {
                    // SAFETY: memory of our own, private; room for a 2 MB boundary inside.
                    let map = unsafe { libc::mmap(std::ptr::null_mut(), len + HUGE, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
                    assert!(map != libc::MAP_FAILED, "{}", std::io::Error::last_os_error());
                    let start = (map as usize).div_ceil(HUGE) * HUGE;
                    let bytes = start as *mut u8;
                    // SAFETY: the aligned part of the mapping.
                    unsafe { libc::madvise(bytes.cast(), len, libc::MADV_HUGEPAGE) };
                    for region in 0..len / HUGE {
                        let r0 = Instant::now();
                        // SAFETY: within the mapping. (The first touch of a
                        // 2 MB: where it is given, whole or as a single page.)
                        unsafe { bytes.add(region * HUGE).write(1) };
                        took.push(r0.elapsed().as_secs_f64() * 1e3);
                        for page in 1..HUGE / 4096 {
                            // SAFETY: within the mapping.
                            unsafe { bytes.add(region * HUGE + page * 4096).write(1) };
                        }
                    }
                    as_huge = huge_share(start, len);
                    // SAFETY: the mapping made above, whole.
                    unsafe { libc::munmap(map, len + HUGE) };
                    passes += 1;
                }
                (took, passes, as_huge)
            })
        })
        .collect();
    let (mut took, mut passes, mut as_huge) = (Vec::new(), 0u64, 0.0f64);
    for w in workers {
        let (t, p, h) = w.join().expect("a worker");
        took.extend(t);
        passes += p;
        as_huge += h / threads as f64;
    }
    let (p50, p99, _, max) = spread(&mut took);
    println!(
        "{{\"what\":\"huge\",\"mb\":{mb},\"threads\":{threads},\"mb_a_sec\":{:.0},\"ms_p50\":{p50:.3},\"ms_p99\":{p99:.2},\"ms_max\":{max:.1},\"as_huge\":{as_huge:.2}}}",
        (passes as usize * (len >> 20)) as f64 / t0.elapsed().as_secs_f64()
    );
}

/// How much of the mapping at `start` (`len` bytes) is held as huge
/// pages now (its `AnonHugePages` in smaps), of 1.
fn huge_share(start: usize, len: usize) -> f64 {
    let Ok(smaps) = std::fs::read_to_string("/proc/self/smaps") else { return 0.0 };
    let mut inside = false;
    let mut kb = 0usize;
    for line in smaps.lines() {
        if let Some((range, _)) = line.split_once(' ')
            && let Some((a, b)) = range.split_once('-')
            && let (Ok(a), Ok(b)) = (usize::from_str_radix(a, 16), usize::from_str_radix(b, 16))
        {
            inside = a < start + len && b > start;
        } else if inside && let Some(v) = line.strip_prefix("AnonHugePages:") {
            kb += v.trim().trim_end_matches("kB").trim().parse::<usize>().unwrap_or(0);
        }
    }
    (kb * 1024) as f64 / len as f64
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
        Some("fragment") => fragment(rest),
        Some("huge") => huge(rest),
        Some("touch") => touch(rest),
        Some("writer") => writer(rest),
        Some("fill") => fill(rest),
        Some("splitlock") => splitlock(rest),
        Some("pressure") => pressure(rest),
        Some("session-groups") => session_groups(rest),
        Some("keep") => keep(rest),
        Some("hold") => hold(rest),
        Some("session-check") => session_check(rest),
        Some("caps") => {
            println!("check|{}|{}", rest.first().map_or("a program started", String::as_str), if olo_system::caps::held(olo_system::caps::SYS_NICE) { "has the capability" } else { "has none" })
        }
        _ => {
            eprintln!("kbench io|wake|relay|spin|chat|hog|touch|writer|fill|splitlock … (see the top of crates/system/examples/kbench.rs)");
            std::process::exit(2);
        }
    }
    let _ = std::io::stdout().flush();
}

/// `kbench session-groups MOUNT OWN SESSION UID GID [--mark]`: what
/// Olo's login does with a login session's scope, run as root in a guest
/// by a process that is alone in a scope made by hand
/// (`olo_system::session_groups::prepare`), then the session's first
/// process as the login starts it: into the desktop's group, the one
/// capability kept through the change of user, and `session-check` as
/// that user. `--mark` marks the scope delegated first, as systemd does.
fn session_groups(args: &[String]) {
    use olo_system::{caps, session_groups as groups};
    let [mount, own, session, uid, gid, ..] = args else { return eprintln!("kbench session-groups MOUNT OWN SESSION UID GID [--mark]") };
    let (uid, gid): (u32, u32) = (uid.parse().unwrap_or(1000), gid.parse().unwrap_or(1000));
    let scope = std::path::Path::new(mount).join(own.trim_start_matches('/'));
    if args.iter().any(|a| a == "--mark") {
        let path = std::ffi::CString::new(scope.to_str().unwrap_or_default()).unwrap_or_default();
        // SAFETY: our strings.
        if unsafe { libc::setxattr(path.as_ptr(), c"user.delegate".as_ptr(), b"1".as_ptr().cast(), 1, 0) } != 0 {
            return println!("check|the scope marked delegated|failed: {}", std::io::Error::last_os_error());
        }
    }
    let made = match groups::prepare(std::path::Path::new(mount), own, session, std::process::id(), uid, gid) {
        Ok(made) => made,
        Err(why) => return println!("check|prepare|refused: {why}"),
    };
    println!("check|prepare|made; the worker is in {}", groups::own().unwrap_or_default());
    let me = std::env::current_exe().unwrap_or_default();
    let argv: Vec<std::ffi::CString> =
        [me.to_str().unwrap_or_default(), "session-check", made.desk.to_str().unwrap_or_default(), made.bg.to_str().unwrap_or_default(), &std::process::id().to_string()]
            .iter()
            .map(|a| std::ffi::CString::new(*a).unwrap_or_default())
            .collect();
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    // SAFETY: one thread; the two do what the login's worker and its child
    // do, in their order: the child waits at a gate until the worker has
    // put it in the desktop's group.
    unsafe {
        let mut gate = [-1i32; 2];
        if libc::pipe2(gate.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return println!("check|a gate|failed: {}", std::io::Error::last_os_error());
        }
        let pid = libc::fork();
        if pid == 0 {
            libc::close(gate[1]);
            let mut byte = 0u8;
            while libc::read(gate[0], (&raw mut byte).cast(), 1) < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {}
            libc::close(gate[0]);
            // (As the login's worker does it: with the owner's choice for
            // this account, `--chosen`, the one capability kept through
            // the change of user and handed on; without, everything
            // dropped and read back.)
            let keep = args.iter().any(|a| a == "--chosen").then(caps::keep_for_login);
            if libc::setgroups(0, std::ptr::null()) != 0 || libc::setgid(gid) != 0 || libc::setuid(uid) != 0 {
                println!("check|becoming the person|failed: {}", std::io::Error::last_os_error());
                libc::_exit(1);
            }
            let grant = keep.map(|r| r.and_then(|()| caps::for_exec(caps::SYS_NICE)));
            if !matches!(grant, Some(Ok(()))) {
                println!("check|nothing handed on|{}", caps::rest(None).map_or_else(|e| format!("NOT VERIFIED: {e}"), |()| "all dropped, read back".into()));
            }
            println!("check|root taken back|{}", if libc::setuid(0) == 0 { "yes" } else { "no" });
            libc::execv(ptrs[0], ptrs.as_ptr());
            libc::_exit(127);
        }
        libc::close(gate[0]);
        // (`--misplace`: as if the child couldn't be put there, to see the
        // division taken apart with both back where they were.)
        let placed = if args.iter().any(|a| a == "--misplace") { Err("as asked".to_owned()) } else { groups::place(&made, pid as u32) };
        match placed {
            Ok(()) => println!("check|the session's first process placed|done"),
            Err(why) => {
                println!("check|the division abandoned|{why}: {}", groups::abandon(made, std::process::id(), Some(pid as u32)).map_or_else(|left| format!("left behind: {left}"), |()| "undone".into()))
            }
        }
        libc::close(gate[1]);
        let mut status = 0;
        libc::waitpid(pid, &mut status, 0);
    }
}

/// `kbench session-check DESK BG WORKER`: as the person, in the
/// session's desktop group, with what the login handed on: what they can
/// do with the groups and what they can't, and who has the capability.
fn session_check(args: &[String]) {
    use olo_system::{caps, session_groups as groups};
    use std::{io::Write as _, path::Path};
    let [desk, bg, worker, ..] = args else { return };
    let (desk, bg) = (Path::new(desk), Path::new(bg));
    let write = |file: std::path::PathBuf, value: &str| std::fs::OpenOptions::new().write(true).open(&file).and_then(|mut f| f.write_all(value.as_bytes()));
    let said = |what: &str, r: std::io::Result<()>| println!("check|{what}|{}", r.map_or_else(|e| format!("refused: {e}"), |()| "done".into()));
    let me = std::process::id().to_string();
    let own = || groups::own().unwrap_or_default();
    // SAFETY: plain system calls.
    println!("check|who|uid {} in {}", unsafe { libc::getuid() }, own());
    let sets = || {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        ["CapInh", "CapPrm", "CapEff", "CapAmb"]
            .map(|k| status.lines().find_map(|l| l.strip_prefix(k)).map_or(String::new(), |v| v.trim_start_matches(':').trim().trim_start_matches('0').to_owned()))
            .join("/")
    };
    println!("check|the session's first process|{}", if caps::held(caps::SYS_NICE) { "has the capability" } else { "has none" });
    println!("check|its four sets as it starts|{}", sets());
    // (As `olo-session` keeps it: usable, and neither inheritable nor
    // handed on.)
    let cap = caps::held(caps::SYS_NICE).then_some(caps::SYS_NICE);
    println!("check|kept at rest|{} {}", caps::rest(cap).map_or_else(|e| format!("NOT VERIFIED: {e}"), |()| "read back".into()), sets());
    println!("check|found from desk|{}", groups::find(Path::new("/cg"), &own()).map_or("nothing".into(), |g| format!("{} and {}", g.desk.display(), g.bg.display())));
    said("itself to bg", write(bg.join("cgroup.procs"), &me));
    println!("check|then in|{}", own());
    said("itself back to desk", write(desk.join("cgroup.procs"), &me));
    println!("check|and in|{}", own());
    said("bg's allowance set", write(bg.join("memory.high"), "1073741824"));
    println!("check|bg's allowance reads|{}", std::fs::read_to_string(bg.join("memory.high")).unwrap_or_default().trim());
    said("bg's allowance lifted", write(bg.join("memory.high"), "max"));
    // (Nothing asked for: whether each is the person's to write.)
    said("bg's memory taken back", write(bg.join("memory.reclaim"), "0"));
    said("desk's memory taken back", write(desk.join("memory.reclaim"), "0"));
    said("bg's swap ceiling set", write(bg.join("memory.swap.max"), "max"));
    said("desk's swap ceiling set", write(desk.join("memory.swap.max"), "max"));
    println!("check|bg idle on the processors|{}", std::fs::read_to_string(bg.join("cpu.idle")).unwrap_or_default().trim());
    said("the login's worker to bg", write(bg.join("cgroup.procs"), worker));
    said("itself to the worker's group", write(desk.join("../../leader/cgroup.procs"), &me));
    said("itself to the scope's own group", write(desk.join("../../cgroup.procs"), &me));
    said("desk's own allowance set", write(desk.join("memory.high"), "1073741824"));
    said("bg taken off idle", write(bg.join("cpu.idle"), "0"));
    said("a group made in olo", std::fs::create_dir(desk.join("../more")));
    // SAFETY: a plain system call.
    println!("check|its nice raised to -5|{}", if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, -5) } == 0 { "done" } else { "refused" });
    // What it starts: with the capability only where it hands it on.
    let me = std::env::current_exe().unwrap_or_default();
    use std::os::unix::process::CommandExt as _;
    let _ = std::process::Command::new(&me).args(["caps", "a program it starts"]).status();
    if let Some(cap) = cap {
        let mut one = std::process::Command::new(&me);
        one.args(["caps", "the one it hands it to"]);
        // SAFETY: plain system calls in the child, as `olo-session` makes them.
        unsafe { one.pre_exec(move || caps::for_exec(cap)) };
        let _ = one.status();
    }
    let _ = std::process::Command::new(&me).args(["caps", "one started after"]).status();
}

/// `kbench keep DESK BG --reserve PERCENT [--report FILE]`: Olo's own
/// keeper of a background group's allowance
/// (`olo_system::background::Keeper`, as the supervisor runs it) on the
/// two groups given, until it is killed: what a guest's measurements are
/// of. What it says goes to stderr; with --report, the longest one of
/// its looks has taken so far, in milliseconds, is kept in FILE (written
/// four times a second: the keeper is killed, not stopped).
fn keep(args: &[String]) {
    let [desk, bg, ..] = args else { return eprintln!("kbench keep DESK BG --reserve PERCENT [--report FILE]") };
    let (notes, said) = std::sync::mpsc::channel();
    let groups = olo_system::session_groups::Groups { desk: desk.into(), bg: bg.into() };
    let keeper = match olo_system::background::Keeper::start(groups, num(args, "--reserve", 12.0), std::path::Path::new("/tmp/olo-keeper.lock"), None, notes) {
        Ok(keeper) => keeper,
        Err(why) => return eprintln!("{why}"),
    };
    let Some(report) = opt(args, "--report").map(std::path::PathBuf::from) else {
        for note in said {
            eprintln!("{note}");
        }
        return;
    };
    let report_dir = report.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
    loop {
        match said.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(note) => eprintln!("{note}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let updated = (|| -> std::io::Result<()> {
            use std::io::Write as _;
            let mut temp = tempfile::NamedTempFile::new_in(report_dir)?;
            writeln!(temp, "{}", keeper.longest_look().as_millis())?;
            temp.persist(&report)?;
            Ok(())
        })();
        if let Err(error) = updated {
            eprintln!("keeper report: {error}");
            break;
        }
    }
}

/// `kbench hold [--file MB] [--own MB] [--pin] [--busy] [PATH]`: memory
/// held by a program asleep (see the top); `--busy`, by one that reads
/// through all of its own again and again, a fiftieth of a second
/// asleep between two rounds (memory in use, as a build's is, brought
/// back as often as it is taken; how many rounds a second that makes
/// is the computer's doing, and which program's memory the kernel
/// then takes first is still its choice).
fn hold(args: &[String]) {
    use std::io::Write as _;
    let (file_mb, own_mb): (usize, usize) = (num(args, "--file", 0), num(args, "--own", 0));
    let pin = args.iter().any(|a| a == "--pin");
    let path = args.iter().rev().find(|a| a.starts_with('/')).cloned().unwrap_or_else(|| "/d/held".into());
    let fail = |what: &str| -> ! {
        eprintln!("kbench hold: {what}: {}", std::io::Error::last_os_error());
        std::process::exit(1)
    };
    if file_mb > 0 {
        let mut f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap_or_else(|_| fail("the file"));
        let chunk = vec![0u8; 1 << 20];
        for _ in 0..file_mb {
            f.write_all(&chunk).unwrap_or_else(|_| fail("writing the file"));
        }
        f.sync_all().unwrap_or_else(|_| fail("the file to disk"));
        use std::os::fd::AsRawFd as _;
        // SAFETY: a mapping of our file, read below within its length, kept for the program's life.
        let map = unsafe { libc::mmap(std::ptr::null_mut(), file_mb << 20, libc::PROT_READ, libc::MAP_SHARED, f.as_raw_fd(), 0) };
        if map == libc::MAP_FAILED {
            fail("mapping the file");
        }
        for at in (0..file_mb << 20).step_by(4096) {
            // SAFETY: within the mapping.
            std::hint::black_box(unsafe { std::ptr::read_volatile((map as *const u8).add(at)) });
        }
        if pin {
            let unlimited = libc::rlimit { rlim_cur: libc::RLIM_INFINITY, rlim_max: libc::RLIM_INFINITY };
            // SAFETY: plain system calls on our mapping.
            if unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &unlimited) } != 0 || unsafe { libc::mlock(map, file_mb << 20) } != 0 {
                fail("locking the file in memory");
            }
        }
    }
    let mut own_at = std::ptr::null_mut();
    if own_mb > 0 {
        // SAFETY: memory of our own, written below within its length, kept for the program's life.
        let own = unsafe { libc::mmap(std::ptr::null_mut(), own_mb << 20, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
        if own == libc::MAP_FAILED {
            fail("memory of its own");
        }
        own_at = own;
        // (In single pages: what goes to swap is counted by the page.)
        // SAFETY: advice about our own mapping.
        unsafe { libc::madvise(own, own_mb << 20, libc::MADV_NOHUGEPAGE) };
        for at in (0..own_mb << 20).step_by(4096) {
            // SAFETY: within the mapping.
            unsafe { std::ptr::write_volatile((own as *mut u8).add(at), 197) };
        }
    }
    println!("ready");
    let _ = std::io::stdout().flush();
    let busy = args.iter().any(|a| a == "--busy") && own_mb > 0;
    loop {
        if busy {
            for at in (0..own_mb << 20).step_by(4096) {
                // SAFETY: within the mapping.
                std::hint::black_box(unsafe { std::ptr::read_volatile((own_at as *const u8).add(at)) });
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        } else {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
}
