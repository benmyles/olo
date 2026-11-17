//! The recogniser on a picture of two lines set in DejaVu Sans
//! (`tests/data/two-lines.png`: "The quick brown fox" at (40, 30), 36
//! pixels; "jumps over 12 lazy dogs." at (40, 110), 28 pixels), with
//! the repository's own models.

use std::path::Path;

fn picture() -> (Vec<u8>, u32, u32) {
    let file = std::fs::File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/two-lines.png")).unwrap();
    let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgb);
    pixels.truncate(info.buffer_size());
    (pixels, info.width, info.height)
}

#[test]
fn reads_two_lines_and_where_they_are() {
    // SAFETY: the only test in this process.
    unsafe { std::env::set_var("OLO_OCR_MODELS", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/ocr")) };
    assert!(olo_ocr::available());
    let (pixels, width, height) = picture();
    let start = std::time::Instant::now();
    let lines = olo_ocr::read(&pixels, width, height).expect("read");
    let first = start.elapsed();
    assert_eq!(lines.iter().map(olo_ocr::Line::text).collect::<Vec<_>>(), ["The quick brown fox", "jumps over 12 lazy dogs."]);
    // Where: each line's box round where it was set, each character
    // inside its line's and after the one before.
    let [x, y, w, h] = lines[0].rect();
    assert!((30.0..50.0).contains(&x) && (25.0..45.0).contains(&y) && (330.0..420.0).contains(&w) && (25.0..50.0).contains(&h), "{:?}", lines[0].rect());
    let [x, y, ..] = lines[1].rect();
    assert!((30.0..50.0).contains(&x) && (105.0..125.0).contains(&y), "{:?}", lines[1].rect());
    for line in &lines {
        let mut before = f32::MIN;
        for c in &line.chars {
            assert!(c.rect[0] >= before - 2.0, "{:?} before {before}", c);
            before = c.rect[0];
        }
    }
    // Again, with the models loaded: the reading alone.
    let start = std::time::Instant::now();
    let again = olo_ocr::read(&pixels, width, height).expect("read");
    assert_eq!(again, lines);
    eprintln!("first {first:?} (the models loaded), again {:?}", start.elapsed());
    // Four bytes a pixel read the same.
    let rgba: Vec<u8> = pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    assert_eq!(olo_ocr::read(&rgba, width, height).expect("read").len(), 2);
    olo_ocr::let_go();
    assert_eq!(olo_ocr::read(&pixels, width, height).expect("read after letting go").len(), 2);
    // A picture with nothing written on it.
    let blank = vec![255u8; 200 * 100 * 3];
    assert_eq!(olo_ocr::read(&blank, 200, 100).expect("read"), []);
}

/// How long a picture takes to read (`OLO_OCR_TIME=FILE.png cargo test
/// --release -p olo-ocr how_long -- --ignored --nocapture`: an RGB or
/// RGBA PNG), with the models loaded, and how much is resident then and
/// once they're let go.
#[test]
#[ignore]
fn how_long() {
    let Some(file) = std::env::var_os("OLO_OCR_TIME") else { return };
    // SAFETY: run alone (ignored unless asked for).
    unsafe { std::env::set_var("OLO_OCR_MODELS", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/ocr")) };
    let mut reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&file).unwrap())).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    pixels.truncate(info.buffer_size());
    let resident =
        || std::fs::read_to_string("/proc/self/status").unwrap().lines().find(|l| l.starts_with("VmRSS")).map(|l| l.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap() / 1024).unwrap();
    let before = resident();
    let start = std::time::Instant::now();
    let lines = olo_ocr::read(&pixels, info.width, info.height).unwrap();
    let first = start.elapsed();
    let start = std::time::Instant::now();
    let again = olo_ocr::read(&pixels, info.width, info.height).unwrap();
    let loaded = resident();
    let took = start.elapsed();
    olo_ocr::let_go();
    // (How it stands with a small picture read after: what a reading
    // leaves isn't given back until the next has been and gone.)
    if std::env::var_os("OLO_OCR_AFTER").is_some() {
        let small = vec![255u8; 64 * 64 * 3];
        let _ = olo_ocr::read(&small, 64, 64);
        olo_ocr::let_go();
        eprintln!("after a small one read and let go: {} MB", resident());
        std::thread::sleep(std::time::Duration::from_secs(2));
        // SAFETY: glibc's own report of its heap.
        let m = unsafe { libc::mallinfo2() };
        eprintln!("heap: {} MB in use, {} MB free and kept, {} MB mapped apart, arena {} MB", m.uordblks >> 20, m.fordblks >> 20, m.hblkhd >> 20, m.arena >> 20);
        eprintln!(
            "two seconds on: {} MB; smaps: {}",
            resident(),
            std::fs::read_to_string("/proc/self/smaps_rollup")
                .unwrap()
                .lines()
                .filter(|l| l.starts_with("Anonymous") || l.starts_with("Rss") || l.starts_with("Private_Dirty"))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    eprintln!(
        "{} × {}: {} lines, {} characters; first {first:?} (models loaded), again {took:?}; resident {before} MB before, {loaded} MB with the models, {} MB let go",
        info.width,
        info.height,
        lines.len(),
        again.iter().map(|l| l.chars.len()).sum::<usize>(),
        resident()
    );
}
