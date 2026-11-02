//! The AirPods' own microphone: their wideband stream (AAC-ELD, 480
//! samples a frame), which leaves their sound at music quality. Frames
//! come from each pair's channel (`Message::Audio`). For now they can be
//! kept as they come (`OLO_AIRPODS_CAPTURE=FILE`: each frame as its
//! timestamp, u32 little-endian, its length, u16, and its bytes) to study
//! and to test the decoder against; the decoder and the microphone apps
//! record from come next.

use std::{io::Write, sync::OnceLock};

use olo_sync::Mutex;

struct Capture {
    file: Option<std::fs::File>,
    frames: u64,
    bytes: u64,
}

fn capture() -> &'static Mutex<Capture> {
    static C: OnceLock<Mutex<Capture>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Capture { file: None, frames: 0, bytes: 0 }))
}

/// The stream was started or stopped.
pub fn switched(addr: &str, on: bool) {
    let mut c = capture().lock().unwrap();
    if on {
        c.file = std::env::var_os("OLO_AIRPODS_CAPTURE").and_then(|p| std::fs::File::create(p).ok());
        (c.frames, c.bytes) = (0, 0);
        tracing::info!("airpods {addr}: microphone on");
    } else {
        tracing::info!("airpods {addr}: microphone off ({} frames, {} bytes)", c.frames, c.bytes);
        c.file = None;
    }
}

/// Frames from their microphone.
pub fn frames(addr: &str, frames: Vec<(u32, Vec<u8>)>) {
    let mut c = capture().lock().unwrap();
    for (ts, au) in frames {
        c.frames += 1;
        c.bytes += au.len() as u64;
        if let Some(f) = &mut c.file {
            let _ = f.write_all(&ts.to_le_bytes()).and_then(|_| f.write_all(&(au.len() as u16).to_le_bytes())).and_then(|_| f.write_all(&au));
        }
    }
    if c.frames % 400 == 1 {
        tracing::debug!("airpods {addr}: microphone: {} frames", c.frames);
    }
}
