//! Video made in process, nothing shelled out to: pictures (BGRA or
//! RGBA rows) to H.264 ([`H264`], OpenH264 in its screen-content mode)
//! into an MP4 ([`Mp4`], written as it goes: each sample to disk as it
//! comes, with its own duration, the index at the end), for the screen
//! recorder (olo-screenshot) and Agent Screens' Export Video.

use std::{
    fs::File,
    io::{BufWriter, Seek, SeekFrom, Write},
    path::Path,
};

use openh264::{
    OpenH264API,
    encoder::{BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, RateControlMode, UsageType},
    formats::{BgraSliceU8, RgbaSliceU8, YUVBuffer},
};

/// The NAL units of an Annex B stream (start codes taken off).
pub fn nal_units(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &s)| {
            let mut end = starts.get(n + 1).map_or(stream.len(), |&e| e - 3);
            // (A four-byte start code's leading zero.)
            while end > s && stream[end - 1] == 0 && starts.get(n + 1).is_some() {
                end -= 1;
            }
            &stream[s..end]
        })
        .filter(|u| !u.is_empty())
        .collect()
}

/// How a picture's bytes are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Blue, green, red, then alpha or nothing (Wayland's ARGB/XRGB8888
    /// in memory: what screen capture gives).
    Bgra,
    Rgba,
}

/// An H.264 encoder for pictures of one size (even: H.264's; [`even`]).
pub struct H264 {
    encoder: Encoder,
    width: usize,
    height: usize,
    /// One picture's rows, packed (a capture's may be wider).
    packed: Vec<u8>,
}

/// `(w, h)` made even, as H.264 wants (the last column or row cut).
pub fn even(w: u32, h: u32) -> (u32, u32) {
    (w & !1, h & !1)
}

impl H264 {
    /// An encoder for `width` × `height` (even) pictures, at up to `fps`
    /// a second: screen content (OpenH264's real-time screen mode: its
    /// non-real-time one is refused, "Invalid usage type"), its quality
    /// kept (a bitrate from its
    /// size: a 4K screen 25 Mbit/s at 30 a second), a key frame every
    /// two seconds so it seeks.
    pub fn new(width: u32, height: u32, fps: u32) -> Result<H264, String> {
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || w % 2 == 1 || h % 2 == 1 {
            return Err(format!("{w}×{h} isn't an even size"));
        }
        let fps = fps.clamp(1, 120);
        let bps = ((w * h) as f64 * fps as f64 * 0.1).clamp(1e6, 80e6) as u32;
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Quality)
            .bitrate(BitRate::from_bps(bps))
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 2))
            .complexity(Complexity::Medium)
            .skip_frames(false)
            // (OpenH264 says nothing of its own on stderr.)
            .debug(false);
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| e.to_string())?;
        Ok(H264 { encoder, width: w, height: h, packed: vec![0; w * h * 4] })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width as u32, self.height as u32)
    }

    /// One picture (`stride` bytes a row, at least this size: what's past
    /// it is cut) encoded: its Annex B bytes, and whether it's a key frame.
    pub fn encode(&mut self, data: &[u8], stride: usize, order: Order) -> Result<(Vec<u8>, bool), String> {
        let row = self.width * 4;
        if stride < row || data.len() < stride * (self.height - 1) + row {
            return Err("the picture is smaller than the video".into());
        }
        for (y, out) in self.packed.chunks_exact_mut(row).enumerate() {
            out.copy_from_slice(&data[y * stride..y * stride + row]);
        }
        let size = (self.width, self.height);
        let yuv = match order {
            Order::Bgra => YUVBuffer::from_rgb_source(BgraSliceU8::new(&self.packed, size)),
            Order::Rgba => YUVBuffer::from_rgb_source(RgbaSliceU8::new(&self.packed, size)),
        };
        let bits = self.encoder.encode(&yuv).map_err(|e| e.to_string())?;
        let key = matches!(bits.frame_type(), FrameType::IDR | FrameType::I);
        Ok((bits.to_vec(), key))
    }
}

/// An MP4 of one H.264 track, written as it goes: `ftyp`, then `mdat`
/// with each sample (length-prefixed NAL units) as it comes, then, at
/// [`Mp4::finish`], `moov` saying where each is and how long it shows,
/// `mdat`'s size filled in last. Until then the file isn't playable.
pub struct Mp4 {
    file: BufWriter<File>,
    width: u32,
    height: u32,
    /// Ticks a second of the durations given.
    timescale: u32,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    /// Each sample's place in the file, size and duration.
    offsets: Vec<u64>,
    sizes: Vec<u32>,
    durations: Vec<u32>,
    keys: Vec<u32>,
    at: u64,
    /// Where `mdat`'s 64-bit size goes.
    mdat_size_at: u64,
}

impl Mp4 {
    /// A new MP4 at `path` of `width` × `height`, its durations in
    /// `timescale` ticks a second.
    pub fn create(path: &Path, width: u32, height: u32, timescale: u32) -> std::io::Result<Mp4> {
        let mut file = BufWriter::new(File::create(path)?);
        let ftyp = bx(b"ftyp", &[b"isom".as_slice(), &0x200u32.to_be_bytes(), b"isom", b"iso2", b"avc1", b"mp41"].concat());
        file.write_all(&ftyp)?;
        // `mdat` with a 64-bit size (size 1, then the large size), filled
        // in at the end.
        let mdat_size_at = ftyp.len() as u64 + 8;
        file.write_all(&1u32.to_be_bytes())?;
        file.write_all(b"mdat")?;
        file.write_all(&0u64.to_be_bytes())?;
        let at = mdat_size_at + 8;
        Ok(Mp4 { file, width, height, timescale: timescale.max(1), sps: None, pps: None, offsets: Vec::new(), sizes: Vec::new(), durations: Vec::new(), keys: Vec::new(), at, mdat_size_at })
    }

    /// The video's size.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Samples so far.
    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }

    /// One encoded picture (Annex B) shown for `duration` ticks; its
    /// parameter sets are kept for the track's description.
    pub fn add(&mut self, annex_b: &[u8], key: bool, duration: u32) -> std::io::Result<()> {
        let mut sample = Vec::with_capacity(annex_b.len());
        for u in nal_units(annex_b) {
            match u[0] & 0x1f {
                7 => self.sps = Some(u.to_vec()),
                8 => self.pps = Some(u.to_vec()),
                _ => {
                    sample.extend_from_slice(&(u.len() as u32).to_be_bytes());
                    sample.extend_from_slice(u);
                }
            }
        }
        if sample.is_empty() {
            return Ok(());
        }
        self.file.write_all(&sample)?;
        self.offsets.push(self.at);
        self.sizes.push(sample.len() as u32);
        self.durations.push(duration.max(1));
        self.at += sample.len() as u64;
        if key {
            self.keys.push(self.sizes.len() as u32);
        }
        Ok(())
    }

    /// The last sample shown `duration` ticks longer (a still screen: its
    /// last picture held to the end).
    pub fn extend_last(&mut self, duration: u32) {
        if let Some(d) = self.durations.last_mut() {
            *d = d.saturating_add(duration);
        }
    }

    /// The index written, the file whole and playable.
    pub fn finish(mut self) -> std::io::Result<()> {
        let (Some(sps), Some(pps)) = (self.sps.clone(), self.pps.clone()) else { return Err(std::io::Error::other("the encoder gave no parameter sets")) };
        let n = self.sizes.len() as u32;
        let duration: u64 = self.durations.iter().map(|&d| u64::from(d)).sum();
        let duration = duration.min(u64::from(u32::MAX)) as u32;
        let u32b = |v: u32| v.to_be_bytes().to_vec();
        let u16b = |v: u16| v.to_be_bytes().to_vec();
        let matrix: Vec<u8> = [0x10000u32, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000].iter().flat_map(|v| v.to_be_bytes()).collect();
        let mvhd = full(b"mvhd", 0, &[u32b(0), u32b(0), u32b(self.timescale), u32b(duration), u32b(0x10000), u16b(0x100), vec![0; 10], matrix.clone(), vec![0; 24], u32b(2)].concat());
        let tkhd =
            full(b"tkhd", 3, &[u32b(0), u32b(0), u32b(1), u32b(0), u32b(duration), vec![0; 8], u16b(0), u16b(0), u16b(0), u16b(0), matrix, u32b(self.width << 16), u32b(self.height << 16)].concat());
        let mdhd = full(b"mdhd", 0, &[u32b(0), u32b(0), u32b(self.timescale), u32b(duration), u16b(0x55c4), u16b(0)].concat());
        let hdlr = full(b"hdlr", 0, &[u32b(0), b"vide".to_vec(), vec![0; 12], b"Olo video\0".to_vec()].concat());
        let avcc = bx(b"avcC", &[vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1], u16b(sps.len() as u16), sps.clone(), vec![1], u16b(pps.len() as u16), pps.clone()].concat());
        // (No compressor name: 32 zero bytes.)
        let avc1 = bx(
            b"avc1",
            &[vec![0; 6], u16b(1), vec![0; 16], u16b(self.width as u16), u16b(self.height as u16), u32b(0x0048_0000), u32b(0x0048_0000), u32b(0), u16b(1), vec![0; 32], u16b(0x18), u16b(0xffff), avcc]
                .concat(),
        );
        let stsd = full(b"stsd", 0, &[u32b(1), avc1].concat());
        // Durations, run by run.
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &d in &self.durations {
            match runs.last_mut() {
                Some((count, delta)) if *delta == d => *count += 1,
                _ => runs.push((1, d)),
            }
        }
        let stts = full(b"stts", 0, &[u32b(runs.len() as u32), runs.iter().flat_map(|(c, d)| [c.to_be_bytes(), d.to_be_bytes()].concat()).collect()].concat());
        let stss = full(b"stss", 0, &[u32b(self.keys.len() as u32), self.keys.iter().flat_map(|k| k.to_be_bytes()).collect()].concat());
        // Each sample a chunk of its own, where it is in the file.
        let stsc = full(b"stsc", 0, &[u32b(1), u32b(1), u32b(1), u32b(1)].concat());
        let stsz = full(b"stsz", 0, &[u32b(0), u32b(n), self.sizes.iter().flat_map(|s| s.to_be_bytes()).collect()].concat());
        let co64 = full(b"co64", 0, &[u32b(n), self.offsets.iter().flat_map(|o| o.to_be_bytes()).collect()].concat());
        let stbl = bx(b"stbl", &[stsd, stts, stss, stsc, stsz, co64].concat());
        let vmhd = full(b"vmhd", 1, &[0u8; 8]);
        let dinf = bx(b"dinf", &full(b"dref", 0, &[u32b(1), full(b"url ", 1, &[])].concat()));
        let minf = bx(b"minf", &[vmhd, dinf, stbl].concat());
        let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
        let trak = bx(b"trak", &[tkhd, mdia].concat());
        let moov = bx(b"moov", &[mvhd, trak].concat());
        self.file.write_all(&moov)?;
        // `mdat`'s size: its header (16) and every sample.
        let mdat_size = self.at - (self.mdat_size_at - 8);
        let mut file = self.file.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(self.mdat_size_at))?;
        file.write_all(&mdat_size.to_be_bytes())?;
        file.sync_all()
    }
}

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [((body.len() + 8) as u32).to_be_bytes().as_slice(), kind, body].concat()
}

fn full(kind: &[u8; 4], flags: u32, body: &[u8]) -> Vec<u8> {
    bx(kind, &[&flags.to_be_bytes()[..], body].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_into_units() {
        let s = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9];
        assert_eq!(nal_units(&s), vec![&[0x67, 1, 2][..], &[0x68, 3], &[0x65, 9, 9]]);
    }

    /// A few pictures encoded and written, held for their durations: the
    /// file's boxes where they should be, its samples and times as given.
    #[test]
    fn an_mp4_written_as_it_goes() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-video-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.mp4");
        let (w, h) = (64u32, 48u32);
        let mut enc = H264::new(w, h, 30).unwrap();
        let mut mp4 = Mp4::create(&path, w, h, 1000).unwrap();
        for i in 0..5u8 {
            let px: Vec<u8> = (0..(w * h)).flat_map(|p| [i * 40, (p % 255) as u8, 128, 255]).collect();
            let (bits, key) = enc.encode(&px, w as usize * 4, Order::Bgra).unwrap();
            mp4.add(&bits, key, 100).unwrap();
        }
        mp4.extend_last(400);
        assert_eq!(mp4.len(), 5);
        mp4.finish().unwrap();
        let b = std::fs::read(&path).unwrap();
        assert_eq!(&b[4..8], b"ftyp");
        let ftyp = u32::from_be_bytes(b[0..4].try_into().unwrap()) as usize;
        assert_eq!(&b[ftyp + 4..ftyp + 8], b"mdat");
        let mdat = u64::from_be_bytes(b[ftyp + 8..ftyp + 16].try_into().unwrap()) as usize;
        assert_eq!(&b[ftyp + mdat + 4..ftyp + mdat + 8], b"moov", "mdat's size leads to moov");
        // Durations: four of 100, the last held 500 (stts runs).
        let at = b.windows(4).position(|w| w == b"stts").unwrap();
        let entries = u32::from_be_bytes(b[at + 8..at + 12].try_into().unwrap());
        assert_eq!(entries, 2);
        let run = |i: usize| (u32::from_be_bytes(b[at + 12 + i * 8..at + 16 + i * 8].try_into().unwrap()), u32::from_be_bytes(b[at + 16 + i * 8..at + 20 + i * 8].try_into().unwrap()));
        assert_eq!((run(0), run(1)), ((4, 100), (1, 500)));
        // Read back as the Viewer reads videos (the mp4 crate), and each
        // sample decoded.
        let f = std::fs::File::open(&path).unwrap();
        let size = f.metadata().unwrap().len();
        let mut r = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).unwrap();
        let (&track, t) = r.tracks().iter().next().unwrap();
        assert_eq!((t.width(), t.height(), t.sample_count()), (64, 48, 5));
        assert_eq!(r.duration().as_millis(), 900);
        let mut dec = openh264::decoder::Decoder::new().unwrap();
        let mut avcc = Vec::new();
        if let Ok(sps) = t.sequence_parameter_set() {
            avcc.extend_from_slice(&[0, 0, 0, 1]);
            avcc.extend_from_slice(sps);
        }
        if let Ok(pps) = t.picture_parameter_set() {
            avcc.extend_from_slice(&[0, 0, 0, 1]);
            avcc.extend_from_slice(pps);
        }
        let mut pictures = 0;
        for i in 1..=5 {
            let sample = r.read_sample(track, i).unwrap().unwrap();
            let mut annex_b = if i == 1 { avcc.clone() } else { Vec::new() };
            let mut b = &sample.bytes[..];
            while b.len() >= 4 {
                let n = u32::from_be_bytes(b[..4].try_into().unwrap()) as usize;
                annex_b.extend_from_slice(&[0, 0, 0, 1]);
                annex_b.extend_from_slice(&b[4..4 + n]);
                b = &b[4 + n..];
            }
            if dec.decode(&annex_b).unwrap().is_some() {
                pictures += 1;
            }
        }
        assert!(pictures >= 4, "{pictures} of 5 decoded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a frame costs to encode at 1440 × 900, 2560 × 1600 and 4K
    /// (`--ignored`, release): a screen with text, a run of frames each a
    /// little changed (a caret, a line typed), as a recording's are.
    #[test]
    #[ignore]
    fn encode_cost() {
        for (w, h) in [(1440u32, 900u32), (2560, 1600), (3840, 2160)] {
            let mut enc = H264::new(w, h, 30).unwrap();
            let mut px: Vec<u8> = (0..(w * h))
                .flat_map(|p| {
                    let (x, y) = (p % w, p / w);
                    let ink = if (y / 24) % 2 == 0 && (x / 7) % 3 != 0 { 40 } else { 235 };
                    [ink, ink, ink, 255]
                })
                .collect();
            let mut times = Vec::new();
            for i in 0..30u32 {
                // A little of it changed each frame.
                let y0 = (i * 16) % (h - 16);
                for y in y0..y0 + 16 {
                    for x in 100..400 {
                        let o = ((y * w + x) * 4) as usize;
                        px[o] = (i * 8) as u8;
                    }
                }
                let t = std::time::Instant::now();
                std::hint::black_box(enc.encode(&px, w as usize * 4, Order::Bgra).unwrap());
                times.push(t.elapsed());
            }
            times.sort();
            eprintln!("{w}×{h}: median {:?}, max {:?} a frame (the first, a key frame, among them)", times[15], times[29]);
        }
    }
}
