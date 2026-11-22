use olo_rex_vt::{GhosttyTerminal, RenderState, TerminalSearch, TerminalTheme};
use std::{hint::black_box, time::Instant};

fn timed(name: &str, n: u32, mut f: impl FnMut()) {
    for _ in 0..20 {
        f();
    }
    let mut times = [0; 9];
    for ns in &mut times {
        let start = Instant::now();
        for _ in 0..n {
            f();
        }
        *ns = start.elapsed().as_nanos() / n as u128;
    }
    times.sort();
    println!("{name} {} ns", times[4]);
}
#[test]
#[ignore = "release timing, run with --ignored --nocapture"]
fn wrapper_cost() {
    let mut term = GhosttyTerminal::new(120, 32).unwrap();
    let _search = TerminalSearch::new(&term).unwrap();
    let mut rs = RenderState::new().unwrap();
    for _ in 0..32 {
        term.vt_write(b"one ordinary line with text and \x1b[32mgreen\x1b[0m\r\n");
    }
    let theme = TerminalTheme::default();
    timed("theme", 500, || term.apply_theme(black_box(&theme)).unwrap());
    timed("full frame", 200, || {
        rs.update(&term).unwrap();
        black_box(rs.extract_rows(&rs.colors().palette, false).unwrap());
        rs.clean().unwrap();
    });
    timed("one edited row", 800, || {
        term.vt_write(b"\rX");
        rs.update(&term).unwrap();
        black_box(rs.extract_rows(&rs.colors().palette, true).unwrap());
        rs.clean().unwrap();
    });
    timed("clean frame", 800, || {
        rs.update(&term).unwrap();
        black_box(rs.extract_rows(&rs.colors().palette, true).unwrap());
        rs.clean().unwrap();
    });
    timed("vt_write 48 bytes", 2000, || term.vt_write(black_box(b"one ordinary line with text and \x1b[32mgreen\x1b[0m\r\n")));
}

#[test]
#[ignore = "run alone on Linux to measure process memory"]
fn pane_memory() {
    fn rss_kib() -> usize {
        std::fs::read_to_string("/proc/self/smaps_rollup").unwrap().lines().find_map(|line| line.strip_prefix("Rss:")?.split_whitespace().next()?.parse().ok()).unwrap()
    }
    // Warm the linker, allocator and native code before taking the baseline.
    drop(GhosttyTerminal::new(120, 32).unwrap());
    let base = rss_kib();
    let panes: Vec<_> = (0..32)
        .map(|_| {
            let mut t = GhosttyTerminal::new(120, 32).unwrap();
            for _ in 0..180 {
                t.vt_write(b"\x1b[32mtext for a grid page\x1b[0m\r\n");
            }
            t.vt_write(b"\x1b_Ga=t,f=24,s=1,v=1,i=10;ESIz\x1b\\");
            t
        })
        .collect();
    let mut pages = 0;
    let mut virtual_bytes = 0;
    let mut heap = 0;
    let mut images = 0;
    for t in &panes {
        let m = t.memory_usage();
        pages += m.resident_bytes;
        virtual_bytes += m.virtual_bytes;
        heap += m.heap_bytes;
        images += m.image_bytes;
    }
    let live = rss_kib();
    drop(panes);
    println!("32 panes: grid resident {pages} B, grid virtual {virtual_bytes} B, counted heap {heap} B, images {images} B; RSS KiB before {base}, live {live}, after {}", rss_kib());
}
