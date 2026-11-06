//! Cameras, in process: a V4L2 camera's stream (MJPEG decoded by
//! zune-jpeg, or YUYV converted) as BGRA frames, for a screen recording's
//! camera bubble (and, later, face sign-in's colour camera).
//!
//! Listing reads sysfs only: no camera is opened until one is asked for,
//! and its light comes on only while it streams. `OLO_CAMERA_FAKE=1`
//! puts a stand-in first (`Fake`, "Test Camera": a moving pattern at 30
//! frames a second), for tests and headless sessions, which never open a
//! real one.
//!
//! ```no_run
//! let mut cam = olo_camera::open(None, (640, 480)).unwrap();
//! let frame = cam.frame().unwrap();
//! assert_eq!(frame.bgra.len(), (frame.width * frame.height * 4) as usize);
//! ```

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use v4l::{
    FourCC,
    buffer::Type,
    io::{mmap::Stream as Mmap, traits::CaptureStream},
    video::Capture,
};

/// The stand-in's id.
pub const FAKE: &str = "fake";

/// A camera, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// "video0" (its node), or [`FAKE`].
    pub id: String,
    /// As the device names itself ("Integrated Camera: Integrated C").
    pub name: String,
}

/// The cameras there are: each device's first node (the others are its
/// metadata), from sysfs; the stand-in first with `OLO_CAMERA_FAKE`. A
/// node that turns out to be an infrared camera (grey only) is passed
/// over when one is opened.
pub fn cameras() -> Vec<Info> {
    let mut out = Vec::new();
    if fake_wanted() {
        out.push(Info { id: FAKE.into(), name: "Test Camera".into() });
    }
    let Ok(dir) = std::fs::read_dir("/sys/class/video4linux") else { return out };
    let mut nodes: Vec<(u32, Info)> = dir
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let n: u32 = id.strip_prefix("video")?.parse().ok()?;
            let read = |f: &str| std::fs::read_to_string(e.path().join(f)).map(|s| s.trim().to_owned()).ok();
            (read("index").as_deref() == Some("0")).then(|| (n, Info { name: read("name").unwrap_or_else(|| id.clone()), id }))
        })
        .collect();
    nodes.sort_by_key(|(n, _)| *n);
    out.extend(nodes.into_iter().map(|(_, i)| i));
    out
}

fn fake_wanted() -> bool {
    std::env::var_os("OLO_CAMERA_FAKE").is_some_and(|v| !v.is_empty() && v != "0")
}

/// A frame: BGRA, top row first, `width` × `height`.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// A camera streaming.
pub struct Camera {
    pub info: Info,
    /// What it sends: "MJPG", "YUYV", or "fake".
    pub format: String,
    pub width: u32,
    pub height: u32,
    source: Source,
}

enum Source {
    V4l {
        // (The stream before the device: stopped, then closed.)
        stream: Mmap<'static>,
        _device: v4l::Device,
        mjpeg: bool,
    },
    Fake {
        started: Instant,
        shown: u64,
    },
}

const MJPG: [u8; 4] = *b"MJPG";
const YUYV: [u8; 4] = *b"YUYV";
/// How long a frame may take before the camera is given up on.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Open camera `id` (None: the stand-in with `OLO_CAMERA_FAKE`, else the
/// first colour camera), streaming at about `want` (the size nearest it,
/// MJPEG if it sends that, else YUYV) and 30 frames a second.
pub fn open(id: Option<&str>, want: (u32, u32)) -> Result<Camera, String> {
    let list = cameras();
    let candidates: Vec<Info> = match id {
        Some(id) => list.into_iter().filter(|c| c.id == id || c.name == id).collect(),
        None => list,
    };
    if candidates.is_empty() {
        return Err(match id {
            Some(id) => format!("no camera {id}"),
            None => "no camera".into(),
        });
    }
    let mut why = String::new();
    for info in candidates {
        if info.id == FAKE {
            return Ok(Camera { info, format: "fake".into(), width: want.0, height: want.1, source: Source::Fake { started: Instant::now(), shown: 0 } });
        }
        match open_v4l(&info, want) {
            Ok(c) => return Ok(c),
            Err(e) => why = format!("{}: {e}", info.name),
        }
    }
    Err(why)
}

fn open_v4l(info: &Info, want: (u32, u32)) -> Result<Camera, String> {
    let path = PathBuf::from("/dev").join(&info.id);
    let device = v4l::Device::with_path(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let formats = device.enum_formats().map_err(|e| e.to_string())?;
    let has = |f: [u8; 4]| formats.iter().any(|d| d.fourcc.repr == f);
    // (An infrared camera sends grey only: not one to show.)
    let fourcc = if has(MJPG) {
        MJPG
    } else if has(YUYV) {
        YUYV
    } else {
        return Err("it sends neither MJPEG nor YUYV (an infrared camera?)".into());
    };
    let (w, h) = nearest(&device, fourcc, want);
    let mut fmt = device.format().map_err(|e| e.to_string())?;
    (fmt.width, fmt.height, fmt.fourcc) = (w, h, FourCC::new(&fourcc));
    let set = device.set_format(&fmt).map_err(|e| e.to_string())?;
    if set.fourcc.repr != fourcc {
        return Err(format!("it wouldn't send {}", String::from_utf8_lossy(&fourcc)));
    }
    let _ = device.set_params(&v4l::video::capture::Parameters::with_fps(30));
    let mut stream = Mmap::with_buffers(&device, Type::VideoCapture, 4).map_err(|e| e.to_string())?;
    stream.set_timeout(TIMEOUT);
    Ok(Camera {
        info: info.clone(),
        format: String::from_utf8_lossy(&fourcc).into_owned(),
        width: set.width,
        height: set.height,
        source: Source::V4l { stream, _device: device, mjpeg: fourcc == MJPG },
    })
}

/// The size `fourcc` comes in nearest `want` (by area, larger preferred).
fn nearest(device: &v4l::Device, fourcc: [u8; 4], want: (u32, u32)) -> (u32, u32) {
    let sizes: Vec<(u32, u32)> = device.enum_framesizes(FourCC::new(&fourcc)).unwrap_or_default().into_iter().flat_map(|s| s.size.to_discrete()).map(|d| (d.width, d.height)).collect();
    pick(&sizes, want)
}

/// Of `sizes`, the one nearest `want`: the smallest at least as big, else
/// the biggest.
fn pick(sizes: &[(u32, u32)], want: (u32, u32)) -> (u32, u32) {
    let area = |(w, h): (u32, u32)| u64::from(w) * u64::from(h);
    sizes.iter().copied().filter(|&(w, h)| w >= want.0 && h >= want.1).min_by_key(|&s| area(s)).or_else(|| sizes.iter().copied().max_by_key(|&s| area(s))).unwrap_or(want)
}

impl Camera {
    /// The next frame, waiting for it (a real camera paces itself; the
    /// stand-in sleeps till its next 30th of a second).
    pub fn frame(&mut self) -> Result<Frame, String> {
        let (width, height) = (self.width, self.height);
        match &mut self.source {
            Source::V4l { stream, mjpeg, .. } => {
                let (bytes, meta) = CaptureStream::next(stream).map_err(|e| e.to_string())?;
                let data = &bytes[..(meta.bytesused as usize).min(bytes.len())];
                if *mjpeg { jpeg(data) } else { Ok(Frame { width, height, bgra: yuyv(data, width, height) }) }
            }
            Source::Fake { started, shown } => {
                *shown += 1;
                let due = *started + Duration::from_secs_f64(*shown as f64 / 30.0);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                Ok(Frame { width, height, bgra: pattern(width, height, started.elapsed().as_secs_f32()) })
            }
        }
    }
}

/// An MJPEG frame decoded.
fn jpeg(data: &[u8]) -> Result<Frame, String> {
    use zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(data, options);
    let bgra = d.decode().map_err(|e| format!("a frame that isn't a JPEG: {e:?}"))?;
    let (w, h) = d.dimensions().ok_or("a JPEG without a size")?;
    Ok(Frame { width: w as u32, height: h as u32, bgra })
}

/// YUYV (4:2:2, BT.601, limited range) to BGRA.
fn yuyv(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![255u8; w as usize * h as usize * 4];
    let px = |y: i32, u: i32, v: i32, o: &mut [u8]| {
        let c = 298 * (y - 16);
        let (d, e) = (u - 128, v - 128);
        let clip = |x: i32| ((x + 128) >> 8).clamp(0, 255) as u8;
        o[0] = clip(c + 516 * d);
        o[1] = clip(c - 100 * d - 208 * e);
        o[2] = clip(c + 409 * e);
    };
    for (quad, o) in data.as_chunks::<4>().0.iter().zip(out.as_chunks_mut::<8>().0) {
        let (y0, u, y1, v) = (i32::from(quad[0]), i32::from(quad[1]), i32::from(quad[2]), i32::from(quad[3]));
        px(y0, u, v, &mut o[..4]);
        px(y1, u, v, &mut o[4..]);
    }
    out
}

/// The stand-in's picture at `t` seconds: a soft gradient, a disc going
/// round, a bar sweeping (motion, so a recording has something to show).
fn pattern(w: u32, h: u32, t: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    let (cx, cy) = (w as f32 * (0.5 + 0.25 * (t * 1.7).cos()), h as f32 * (0.5 + 0.25 * (t * 1.7).sin()));
    let r = w.min(h) as f32 * 0.16;
    let bar = (t * 0.5).fract() * w as f32;
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32, y as f32);
            let mut c = [(60.0 + 120.0 * fx / w as f32) as u8, (90.0 + 60.0 * fy / h as f32) as u8, 150u8];
            if (fx - cx).hypot(fy - cy) < r {
                c = [70, 170, 250];
            }
            if (fx - bar).abs() < 3.0 {
                c = [240, 240, 240];
            }
            out.extend_from_slice(&[c[2], c[1], c[0], 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_nearest_size() {
        let sizes = [(1920, 1080), (1280, 720), (640, 480), (320, 240)];
        assert_eq!(pick(&sizes, (640, 480)), (640, 480));
        assert_eq!(pick(&sizes, (600, 400)), (640, 480));
        assert_eq!(pick(&sizes, (4000, 3000)), (1920, 1080));
        assert_eq!(pick(&[], (640, 480)), (640, 480));
    }

    #[test]
    fn yuyv_greys_and_colours() {
        // Black, white and mid grey stay grey; pure red's V up, U down.
        let grey = yuyv(&[16, 128, 235, 128], 2, 1);
        assert_eq!(&grey[..3], &[0, 0, 0]);
        assert_eq!(&grey[4..7], &[255, 255, 255]);
        let red = yuyv(&[81, 90, 81, 240], 2, 1);
        assert!(red[2] > 240 && red[1] < 10 && red[0] < 10, "{red:?}");
    }

    #[test]
    fn the_stand_in_streams() {
        // (Set for this process only: no real camera is opened.)
        unsafe { std::env::set_var("OLO_CAMERA_FAKE", "1") };
        assert_eq!(cameras().first().map(|c| c.id.as_str()), Some(FAKE));
        let mut cam = open(None, (320, 240)).unwrap();
        assert_eq!(cam.format, "fake");
        let t = Instant::now();
        let a = cam.frame().unwrap();
        let b = cam.frame().unwrap();
        assert_eq!(a.bgra.len(), 320 * 240 * 4);
        assert_ne!(a.bgra, b.bgra, "it moves");
        assert!(t.elapsed() >= Duration::from_millis(60), "paced: {:?}", t.elapsed());
    }

    #[test]
    fn jpeg_frames_decode() {
        // An 8 × 8 red square, as a camera's MJPEG frame would be one.
        let f = jpeg(&RED_8X8).unwrap();
        assert_eq!((f.width, f.height), (8, 8));
        assert!(f.bgra[2] > 190 && f.bgra[0] < 70, "{:?}", &f.bgra[..4]);
    }

    /// An 8 × 8 JPEG of red (220, 20, 30), made with PIL.
    #[rustfmt::skip]
    const RED_8X8: [u8; 634] = [255, 216, 255, 224, 0, 16, 74, 70, 73, 70, 0, 1, 1, 0, 0, 1, 0, 1, 0, 0, 255, 219, 0, 67, 0, 3, 2, 2, 3, 2, 2, 3, 3, 3, 3, 4, 3, 3, 4, 5, 8, 5, 5, 4, 4, 5, 10, 7, 7, 6, 8, 12, 10, 12, 12, 11, 10, 11, 11, 13, 14, 18, 16, 13, 14, 17, 14, 11, 11, 16, 22, 16, 17, 19, 20, 21, 21, 21, 12, 15, 23, 24, 22, 20, 24, 18, 20, 21, 20, 255, 219, 0, 67, 1, 3, 4, 4, 5, 4, 5, 9, 5, 5, 9, 20, 13, 11, 13, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 255, 192, 0, 17, 8, 0, 8, 0, 8, 3, 1, 34, 0, 2, 17, 1, 3, 17, 1, 255, 196, 0, 31, 0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 16, 0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125, 1, 2, 3, 0, 4, 17, 5, 18, 33, 49, 65, 6, 19, 81, 97, 7, 34, 113, 20, 50, 129, 145, 161, 8, 35, 66, 177, 193, 21, 82, 209, 240, 36, 51, 98, 114, 130, 9, 10, 22, 23, 24, 25, 26, 37, 38, 39, 40, 41, 42, 52, 53, 54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73, 74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105, 106, 115, 116, 117, 118, 119, 120, 121, 122, 131, 132, 133, 134, 135, 136, 137, 138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166, 167, 168, 169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196, 197, 198, 199, 200, 201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 255, 196, 0, 31, 1, 0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 17, 0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119, 0, 1, 2, 3, 17, 4, 5, 33, 49, 6, 18, 65, 81, 7, 97, 113, 19, 34, 50, 129, 8, 20, 66, 145, 161, 177, 193, 9, 35, 51, 82, 240, 21, 98, 114, 209, 10, 22, 36, 52, 225, 37, 241, 23, 24, 25, 26, 38, 39, 40, 41, 42, 53, 54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73, 74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105, 106, 115, 116, 117, 118, 119, 120, 121, 122, 130, 131, 132, 133, 134, 135, 136, 137, 138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166, 167, 168, 169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196, 197, 198, 199, 200, 201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 226, 227, 228, 229, 230, 231, 232, 233, 234, 242, 243, 244, 245, 246, 247, 248, 249, 250, 255, 218, 0, 12, 3, 1, 0, 2, 17, 3, 17, 0, 63, 0, 240, 42, 40, 162, 191, 50, 63, 208, 131, 255, 217];
}
