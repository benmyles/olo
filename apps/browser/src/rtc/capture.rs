//! Cameras and microphones: cameras through Video4Linux (pure Rust, the
//! frames' NV12, YUYV or MJPEG made I420 here), microphones through the
//! sound server (tabula-preview's recorder). Each device opened becomes a
//! hub source that runs until no track uses it.
//!
//! `TABULA_BROWSER_FAKE_CAMERA=1` makes the only camera one that draws a
//! moving test card, for tests (sound for tests comes from tabula-fakesound).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use v4l::{FourCC, buffer::Type, io::traits::CaptureStream, prelude::*, video::Capture};

use super::{
    frames::{Frame, Picture, Sound},
    hub::{HUB, Kind, SourceId},
};

/// A device as the page may learn of it.
#[derive(Clone, Debug)]
pub struct Device {
    /// Ours for it: a camera's path, a microphone's server name.
    pub key: String,
    pub label: String,
    /// What it can give, for `getCapabilities()` (cameras).
    pub sizes: Vec<(u32, u32)>,
}

pub const FAKE_CAMERA: &str = "fake:camera";

fn fake_camera() -> bool {
    std::env::var_os("TABULA_BROWSER_FAKE_CAMERA").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Formats we can read, best first.
const FORMATS: [&[u8; 4]; 3] = [b"NV12", b"YUYV", b"MJPG"];

/// The cameras there are (Video4Linux capture devices with a format we
/// read; decoders and other memory-to-memory devices aren't cameras).
pub fn cameras() -> Vec<Device> {
    let mut out = Vec::new();
    // Tests never touch the real cameras (not even to list them).
    if fake_camera() {
        out.push(Device { key: FAKE_CAMERA.into(), label: "Test Camera".into(), sizes: vec![(640, 480), (1280, 720)] });
        return out;
    }
    let Ok(dir) = std::fs::read_dir("/dev") else { return out };
    let mut paths: Vec<_> = dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("video") && n[5..].parse::<u32>().is_ok()))
        .collect();
    paths.sort_by_key(|p| p.file_name().and_then(|n| n.to_str()).and_then(|n| n[5..].parse::<u32>().ok()));
    for path in paths {
        let Ok(dev) = Device4::with_path(&path) else { continue };
        let Ok(caps) = dev.query_caps() else { continue };
        use v4l::capability::Flags;
        if !caps.capabilities.contains(Flags::VIDEO_CAPTURE) || caps.capabilities.intersects(Flags::VIDEO_M2M | Flags::VIDEO_M2M_MPLANE) {
            continue;
        }
        let Ok(formats) = dev.enum_formats() else { continue };
        let Some(fourcc) = FORMATS.iter().map(|f| FourCC::new(f)).find(|f| formats.iter().any(|d| d.fourcc == *f)) else { continue };
        let mut sizes: Vec<(u32, u32)> = dev
            .enum_framesizes(fourcc)
            .unwrap_or_default()
            .into_iter()
            .flat_map(|s| s.size.to_discrete())
            .map(|d| (d.width, d.height))
            .filter(|(w, h)| w >= h)
            .collect();
        sizes.sort();
        sizes.dedup();
        let card = caps.card.trim().to_string();
        let label = if card.to_lowercase().contains("camera") { card } else { format!("{card} Camera") };
        out.push(Device { key: path.to_string_lossy().into_owned(), label, sizes });
    }
    out
}

type Device4 = v4l::Device;

/// The microphones the sound server has.
pub fn microphones() -> Vec<Device> {
    let Ok(d) = tabula_system::audio::devices() else { return Vec::new() };
    let mut inputs = d.inputs;
    // The default first: pages take the first as the one in use.
    inputs.sort_by_key(|i| !i.is_default);
    inputs.into_iter().map(|i| Device { key: i.name, label: i.description, sizes: Vec::new() }).collect()
}

/// Speakers, for `enumerateDevices()` (sound plays to the default).
pub fn speakers() -> Vec<Device> {
    let Ok(d) = tabula_system::audio::devices() else { return Vec::new() };
    let mut outputs = d.outputs;
    outputs.sort_by_key(|o| !o.is_default);
    outputs.into_iter().map(|o| Device { key: o.name, label: o.description, sizes: Vec::new() }).collect()
}

/// What the page asked a camera for.
#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// What a camera opened as.
#[derive(Clone, Copy, Debug)]
pub struct Opened {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// Open camera `key` and start its source; returns once the first
/// picture is there (so the page's element has something at once).
pub fn open_camera(key: &str, want: Want) -> Result<(SourceId, Opened), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let id = HUB.add_source(Kind::Video, stop.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    let key = key.to_string();
    std::thread::Builder::new()
        .name("camera".into())
        .spawn(move || {
            let r = if key == FAKE_CAMERA { test_card(id, want, &stop, &tx) } else { camera(id, &key, want, &stop, &tx) };
            if let Err(e) = r {
                tracing::warn!("camera {key}: {e}");
                let _ = tx.send(Err(e));
            }
            HUB.end_source(id);
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(opened)) => Ok((id, opened)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("The camera didn’t start.".into()),
    }
}

fn camera(id: SourceId, path: &str, want: Want, stop: &AtomicBool, started: &std::sync::mpsc::Sender<Result<Opened, String>>) -> Result<(), String> {
    let busy = |e: std::io::Error| if e.raw_os_error() == Some(libc::EBUSY) { "The camera is in use by another app.".to_string() } else { e.to_string() };
    let dev = Device4::with_path(path).map_err(busy)?;
    let formats = dev.enum_formats().map_err(|e| e.to_string())?;
    let fourcc = FORMATS.iter().map(|f| FourCC::new(f)).find(|f| formats.iter().any(|d| d.fourcc == *f)).ok_or("The camera’s formats aren’t supported.")?;
    // The smallest size at least as big as asked for, else the biggest.
    let mut sizes: Vec<(u32, u32)> =
        dev.enum_framesizes(fourcc).unwrap_or_default().into_iter().flat_map(|s| s.size.to_discrete()).map(|d| (d.width, d.height)).filter(|(w, h)| w >= h).collect();
    sizes.sort_by_key(|&(w, h)| w * h);
    let (w, h) = sizes.iter().copied().find(|&(w, h)| w >= want.width && h >= want.height).or(sizes.last().copied()).unwrap_or((want.width, want.height));
    let mut f = dev.format().map_err(|e| e.to_string())?;
    (f.width, f.height, f.fourcc) = (w, h, fourcc);
    let f = dev.set_format(&f).map_err(busy)?;
    let _ = dev.set_params(&v4l::video::capture::Parameters::with_fps(want.fps.clamp(1, 60)));
    let fps = dev.params().ok().map(|p| p.interval).filter(|i| i.numerator > 0).map_or(30, |i| i.denominator / i.numerator);
    let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, 4).map_err(busy)?;
    stream.set_timeout(Duration::from_secs(3));
    let mut first = true;
    let mut jpeg_out = Vec::new();
    tracing::debug!("camera {path}: {}x{} {} at {fps} fps", f.width, f.height, f.fourcc);
    while !stop.load(Ordering::Relaxed) {
        let (buf, meta) = stream.next().map_err(|e| e.to_string())?;
        let data = &buf[..(meta.bytesused as usize).min(buf.len())];
        let picture = match &f.fourcc.repr {
            b"NV12" => Picture::from_nv12(data, f.width, f.height, f.stride),
            b"YUYV" => Picture::from_yuyv(data, f.width, f.height, f.stride),
            _ => decode_jpeg(data, &mut jpeg_out),
        };
        let Some(picture) = picture else { continue };
        if first {
            first = false;
            let _ = started.send(Ok(Opened { width: picture.width, height: picture.height, fps }));
        }
        HUB.publish(id, Frame::Picture(Arc::new(picture)));
    }
    Ok(())
}

fn decode_jpeg(data: &[u8], out: &mut Vec<u8>) -> Option<Picture> {
    use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::YCbCr);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
    d.decode_headers().ok()?;
    let (w, h) = d.dimensions()?;
    out.resize(d.output_buffer_size()?, 0);
    d.decode_into(out).ok()?;
    Picture::from_ycbcr(out, w as u32, h as u32)
}

/// The fake camera: colour bars with a block crossing them, at the size
/// asked for.
fn test_card(id: SourceId, want: Want, stop: &AtomicBool, started: &std::sync::mpsc::Sender<Result<Opened, String>>) -> Result<(), String> {
    let (w, h) = (want.width.clamp(160, 1920) & !1, want.height.clamp(120, 1080) & !1);
    let fps = want.fps.clamp(1, 30);
    let _ = started.send(Ok(Opened { width: w, height: h, fps }));
    let bars: [(u8, u8, u8); 7] = [(180, 128, 128), (168, 44, 136), (145, 147, 44), (133, 63, 52), (63, 193, 204), (51, 109, 212), (28, 212, 120)];
    let start = Instant::now();
    let mut n = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let (wu, hu) = (w as usize, h as usize);
        let mut p = Picture::black(w, h);
        let x0 = ((n * 8) as usize) % wu;
        {
            let (y, uv) = p.data.split_at_mut(wu * hu);
            let (u, v) = uv.split_at_mut(wu * hu / 4);
            for row in 0..hu {
                for x in 0..wu {
                    let (by, bu, bv) = bars[x * 7 / wu];
                    let block = row > hu / 3 && row < hu * 2 / 3 && x >= x0 && x < x0 + wu / 8;
                    y[row * wu + x] = if block { 235 } else { by };
                    if row % 2 == 0 && x % 2 == 0 {
                        let c = row / 2 * (wu / 2) + x / 2;
                        (u[c], v[c]) = if block { (128, 128) } else { (bu, bv) };
                    }
                }
            }
        }
        HUB.publish(id, Frame::Picture(Arc::new(p)));
        n += 1;
        let next = start + Duration::from_secs_f64(n as f64 / fps as f64);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    Ok(())
}

/// Start listening to microphone `key` (the default when None).
pub fn open_microphone(key: Option<&str>) -> Result<SourceId, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let id = HUB.add_source(Kind::Audio, stop.clone());
    let recorder = tabula_preview::audio::Recorder::start(key, 48000, 1, move |samples| {
        HUB.publish(id, Frame::Sound(Arc::new(Sound { rate: 48000, channels: 1, samples: samples.to_vec() })));
    })?;
    // The recorder stops when dropped: keep it until no track uses it.
    std::thread::Builder::new()
        .name("microphone".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            }
            drop(recorder);
        })
        .map_err(|e| e.to_string())?;
    Ok(id)
}
