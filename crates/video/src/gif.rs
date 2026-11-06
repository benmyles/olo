//! A recording made a GIF (`olo-screenshot --gif`, Screenshot's Save as
//! GIF): the MP4's pictures decoded (OpenH264; HEVC and AV1 by
//! `olo_images`), made smaller by area
//! averages to at most `Options::width`, shown at `Options::fps`, in one
//! palette for the whole GIF (NeuQuant trained on pictures from across
//! it, so colours don't shift from frame to frame), each frame only the
//! rectangle that changed, what stayed the same in it transparent (the
//! LZW codes such runs in a few bytes). Two passes over the video: the
//! first for the palette, the second for the frames; cancellable between
//! any two pictures.

use std::{fs::File, io::BufWriter, path::Path, sync::atomic::AtomicBool};

/// How a GIF is made.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Frames a second (a picture held longer is one frame, longer).
    pub fps: u32,
    /// The widest it's made (pixels; 0: the video's own).
    pub width: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options { fps: 10, width: 960 }
    }
}

/// What was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Made {
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub seconds: f64,
    pub bytes: u64,
}

/// Colours in the palette; the last index is transparency.
const COLOURS: usize = 255;
const CLEAR: u8 = 255;
/// Pictures the palette is trained on.
const TRAIN: usize = 16;

/// The MP4 at `src` as a GIF at `dst`; `progress` told how far (0 to 1).
pub fn from_mp4(src: &Path, dst: &Path, opts: &Options, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Result<Made, String> {
    let video = crate::read::Video::open(src)?;
    let (vw, vh) = (video.width, video.height);
    let (ow, oh) = if opts.width > 0 && vw > opts.width { (opts.width, ((u64::from(vh) * u64::from(opts.width) / u64::from(vw)) as u32).max(1)) } else { (vw, vh) };
    let fps = opts.fps.clamp(1, 50);
    let seconds = video.seconds();
    let shown = ((seconds * f64::from(fps)).ceil() as usize).max(1);
    // Pass one: pictures from across it, for the palette.
    let every = shown.div_ceil(TRAIN).max(1);
    let mut sample: Vec<u8> = Vec::new();
    video.each_shown(fps, ow, oh, cancel, &mut |k, rgba| {
        if k % every == 0 {
            sample.extend_from_slice(rgba);
        }
        progress(0.3 * k as f32 / shown as f32);
        Ok(())
    })?;
    if sample.is_empty() {
        return Err("the video shows nothing".into());
    }
    let quant = color_quant::NeuQuant::new(10, COLOURS, &sample);
    drop(sample);
    let mut palette = quant.color_map_rgb();
    palette.resize(256 * 3, 0);
    // A colour's index, by its 15 bits (what a GIF can tell apart anyway).
    let mut cache = vec![u16::MAX; 1 << 15];
    let mut index = |p: &[u8]| -> u8 {
        let key = (usize::from(p[0] >> 3) << 10) | (usize::from(p[1] >> 3) << 5) | usize::from(p[2] >> 3);
        if cache[key] == u16::MAX {
            cache[key] = quant.index_of(&[p[0], p[1], p[2], 255]) as u16;
        }
        cache[key] as u8
    };
    // Pass two: each change a frame, written once the next comes (its
    // delay is till then).
    let out = File::create(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let mut gif = gif::Encoder::new(BufWriter::new(out), ow as u16, oh as u16, &palette).map_err(|e| e.to_string())?;
    gif.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
    // (Olo's mark: a comment, so a library knows it whatever it's named.)
    gif.write_raw_extension(gif::Extension::Comment.into(), &[crate::olo_gif_comment()]).map_err(|e| e.to_string())?;
    let mut shown_now: Option<Vec<u8>> = None;
    let mut pending: Option<(gif::Frame<'static>, usize)> = None;
    let mut frames = 0usize;
    let mut last_k = 0;
    video.each_shown(fps, ow, oh, cancel, &mut |k, rgba| {
        last_k = k;
        let indexed: Vec<u8> = rgba.chunks_exact(4).map(&mut index).collect();
        let frame = match &shown_now {
            None => Some(gif::Frame { width: ow as u16, height: oh as u16, buffer: indexed.clone().into(), ..Default::default() }),
            Some(prev) => changed(prev, &indexed, ow as usize, oh as usize).map(|[x, y, w, h]| {
                let mut buf = Vec::with_capacity(w * h);
                for row in y..y + h {
                    let at = row * ow as usize;
                    for col in x..x + w {
                        let (now, was) = (indexed[at + col], prev[at + col]);
                        buf.push(if now == was { CLEAR } else { now });
                    }
                }
                gif::Frame { left: x as u16, top: y as u16, width: w as u16, height: h as u16, transparent: Some(CLEAR), buffer: buf.into(), ..Default::default() }
            }),
        };
        if let Some(mut f) = frame {
            f.dispose = gif::DisposalMethod::Keep;
            if let Some(p) = pending.take() {
                write(&mut gif, p, k, fps)?;
                frames += 1;
            }
            pending = Some((f, k));
            shown_now = Some(indexed);
        }
        progress(0.3 + 0.7 * k as f32 / shown as f32);
        Ok(())
    })?;
    if let Some(p) = pending.take() {
        write(&mut gif, p, (last_k + 1).max(shown), fps)?;
        frames += 1;
    }
    drop(gif);
    let bytes = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
    progress(1.0);
    Ok(Made { frames, width: ow, height: oh, seconds, bytes })
}

/// Frame `f`, shown from moment `from` till `to` (at `fps`), written.
fn write(gif: &mut gif::Encoder<BufWriter<File>>, (mut f, from): (gif::Frame<'static>, usize), to: usize, fps: u32) -> Result<(), String> {
    // (In hundredths, from the start: no drift from rounding each.)
    let centis = |k: usize| (k as f64 * 100.0 / f64::from(fps)).round() as u64;
    f.delay = (centis(to) - centis(from)).clamp(2, u64::from(u16::MAX)) as u16;
    gif.write_frame(&f).map_err(|e| e.to_string())
}

/// The rectangle (x, y, w, h) in which `now` differs from `was`, if it does.
fn changed(was: &[u8], now: &[u8], w: usize, h: usize) -> Option<[usize; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        let (a, b) = (&was[y * w..(y + 1) * w], &now[y * w..(y + 1) * w]);
        if a == b {
            continue;
        }
        let first = a.iter().zip(b).position(|(p, q)| p != q).unwrap_or(0);
        let last = w - 1 - a.iter().rev().zip(b.iter().rev()).position(|(p, q)| p != q).unwrap_or(0);
        x0 = x0.min(first);
        x1 = x1.max(last);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    (x0 <= x1 && y0 <= y1).then(|| [x0, y0, x1 - x0 + 1, y1 - y0 + 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recording of a red square moving on grey, two seconds, made a
    /// GIF and read back: its size, a frame a move, the square's colour.
    #[test]
    fn a_recording_made_a_gif() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-gif-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (mp4_path, gif_path) = (dir.join("r.mp4"), dir.join("r.gif"));
        let (w, h) = (320u32, 240u32);
        let mut enc = crate::H264::new(w, h, 30).unwrap();
        let mut mp4 = crate::Mp4::create(&mp4_path, w, h, 1000).unwrap();
        for i in 0..4u32 {
            // BGRA: grey, the square red, 40 px further each half second.
            let px: Vec<u8> = (0..w * h)
                .flat_map(|p| {
                    let (x, y) = (p % w, p / w);
                    if (40 + i * 40..80 + i * 40).contains(&x) && (100..140).contains(&y) { [0, 0, 220, 255] } else { [128, 128, 128, 255] }
                })
                .collect();
            let (bits, key) = enc.encode(&px, w as usize * 4, crate::Order::Bgra).unwrap();
            mp4.add(&bits, key, 500).unwrap();
        }
        mp4.finish().unwrap();
        let never = AtomicBool::new(false);
        let made = from_mp4(&mp4_path, &gif_path, &Options { fps: 10, width: 160 }, &never, &|_| {}).unwrap();
        assert_eq!((made.width, made.height), (160, 120));
        // Olo's mark, in a comment.
        assert!(crate::made_by_olo(&gif_path));
        // A frame for each move: the still stretches held, not repeated.
        assert_eq!(made.frames, 4, "{made:?}");
        let mut d = gif::DecodeOptions::new();
        d.set_color_output(gif::ColorOutput::RGBA);
        let mut r = d.read_info(std::fs::File::open(&gif_path).unwrap()).unwrap();
        assert_eq!((r.width(), r.height()), (160, 120));
        let first = r.read_next_frame().unwrap().unwrap().clone();
        assert_eq!(first.delay, 50);
        // The square at (20..40, 50..70) in the half-size picture: red.
        let at = |x: usize, y: usize| &first.buffer[(y * 160 + x) * 4..(y * 160 + x) * 4 + 3];
        let red = at(30, 60);
        assert!(red[0] > 180 && red[1] < 50 && red[2] < 50, "{red:?}");
        let grey = at(100, 20);
        assert!((110..150).contains(&grey[0]) && (110..150).contains(&grey[2]), "{grey:?}");
        // The next: only the part that changed.
        let second = r.read_next_frame().unwrap().unwrap();
        assert!(second.width < 160 && second.height < 120, "{}x{}", second.width, second.height);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_what_changed() {
        let was = vec![0u8; 8 * 4];
        let mut now = was.clone();
        assert_eq!(changed(&was, &now, 8, 4), None);
        now[8 + 3] = 1;
        now[2 * 8 + 5] = 1;
        assert_eq!(changed(&was, &now, 8, 4), Some([3, 1, 3, 2]));
    }

    #[test]
    fn averages_when_smaller() {
        // 4 × 2 of red and blue halves to 2 × 1.
        let src: Vec<u8> = (0..8).flat_map(|i| if i % 4 < 2 { [200, 0, 0] } else { [0, 0, 100] }).collect();
        let mut dst = vec![0u8; 2 * 4];
        crate::read::shrink(&src, 4, 2, &mut dst, 2, 1);
        assert_eq!(dst, [200, 0, 0, 255, 0, 0, 100, 255]);
    }
}
