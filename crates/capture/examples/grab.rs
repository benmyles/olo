//! Frames of a capture source to files, for a test to look at:
//!
//!   grab [--display NAME] [--window ID | --workspace N] [--frames K] OUT
//!
//! takes K frames (1 if not said; each after the first once what's
//! captured has changed) of a display (the first, or NAME), a window
//! (as Olo numbers them) or one of a display's workspaces (from 1), and
//! writes them as OUT-1.ppm, OUT-2.ppm, … Run it on a test session
//! (`tools/dev/tsession run NAME`), never the person's.

use olo_capture::{Capture, Source};

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.last().filter(|a| !a.starts_with("--")).ok_or("grab [--display NAME] [--window ID | --workspace N] [--frames K] OUT")?;
    let display = option(&args, "--display").map(str::to_owned);
    let source = if let Some(id) = option(&args, "--window") {
        Source::Window { id: id.parse().map_err(|_| "--window takes a window's number")?, app_id: String::new(), title: String::new() }
    } else if let Some(n) = option(&args, "--workspace") {
        let n: u32 = n.parse().ok().filter(|n| *n >= 1).ok_or("--workspace is a number from 1")?;
        Source::Workspace { output: display, index: n - 1 }
    } else {
        Source::Output(display)
    };
    let frames: u32 = option(&args, "--frames").map_or(Ok(1), str::parse).map_err(|_| "--frames takes a number")?;
    let mut capture = Capture::new(&source, false)?;
    for k in 1..=frames {
        let start = std::time::Instant::now();
        let p = capture.frame()?;
        let mut ppm = format!("P6\n{} {}\n255\n", p.width, p.height).into_bytes();
        for row in p.data.chunks(p.stride as usize).take(p.height as usize) {
            for px in row[..p.width as usize * 4].chunks(4) {
                ppm.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        let path = format!("{out}-{k}.ppm");
        std::fs::write(&path, ppm).map_err(|e| format!("{path}: {e}"))?;
        println!("{path}: {} × {}, after {:.0} ms", p.width, p.height, start.elapsed().as_secs_f64() * 1e3);
    }
    Ok(())
}
