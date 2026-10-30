//! olo-portal: Olo's xdg-desktop-portal backend (screen sharing first).
//! For now its capture and stream: `olo-portal capture-test OUT.png
//! [OUTPUT]` takes a frame of an output of the compositor at
//! `WAYLAND_DISPLAY` over ext-image-copy-capture and writes it, and
//! `olo-portal stream-test [OUTPUT|-] [SECONDS]` shares one as a PipeWire
//! video source for that long, to check the paths app_server serves.

mod capture;
mod stream;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("capture-test") => match capture_test(args.get(1).map(String::as_str), args.get(2).map(String::as_str)) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("olo-portal: {e}");
                std::process::ExitCode::FAILURE
            }
        },
        Some("stream-test") => {
            // A share of a screen as a PipeWire node, for SECONDS.
            let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
            match stream::run(args.get(1).cloned().filter(|o| o != "-"), true) {
                Ok(share) => {
                    println!("node {} ({}×{})", share.node, share.size.0, share.size.1);
                    std::thread::sleep(std::time::Duration::from_secs(secs));
                    share.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("olo-portal: {e}");
                    std::process::ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: olo-portal capture-test OUT.png [OUTPUT] | stream-test [OUTPUT|-] [SECONDS]");
            std::process::ExitCode::FAILURE
        }
    }
}

fn capture_test(out: Option<&str>, output: Option<&str>) -> Result<(), String> {
    let out = out.ok_or("capture-test needs a file to write")?;
    let mut c = capture::Capture::output(output, true)?;
    let started = std::time::Instant::now();
    let p = c.frame()?;
    let first = started.elapsed();
    // A second, as a stream would.
    let again = std::time::Instant::now();
    c.frame()?;
    eprintln!("{}×{} {:?} ({}): first frame in {first:?}, the next in {:?}", p.width, p.height, p.format, if c.on_gpu() { "dmabuf" } else { "shared memory" }, again.elapsed());
    let mut rgba = Vec::with_capacity((p.width * p.height * 4) as usize);
    for row in p.data.chunks_exact(p.stride as usize).take(p.height as usize) {
        for px in row.chunks_exact(4).take(p.width as usize) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    let file = std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), p.width, p.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut w| w.write_image_data(&rgba)).map_err(|e| e.to_string())
}
