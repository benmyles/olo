//! Video made in process, nothing shelled out to: pictures (BGRA or
//! RGBA rows) to H.264 ([`H264`], OpenH264 in its screen-content mode),
//! or through NVIDIA's encoder to H.264, HEVC or AV1 ([`Codec`]), into an
//! MP4 ([`Mp4`], written as it goes: each sample to disk as it comes,
//! with its own duration, the index at the end), for the screen recorder
//! (olo-screenshot) and Agent Screens' Export Video.

pub mod aac;
pub mod config;
pub mod export;
pub mod gif;
pub mod nvenc;
pub mod read;
pub use olo_vaapi as vaapi;
pub mod trim;

use std::{
    fs::File,
    io::{BufWriter, Seek, SeekFrom, Write},
    path::Path,
};

use openh264::{
    OpenH264API,
    encoder::{BitRate, Complexity, Encoder as OpenH264Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, RateControlMode, UsageType},
    formats::{BgraSliceU8, RgbaSliceU8, YUVBuffer},
};

/// A parameter set (or sequence header) kept for the sample entry: the
/// first; one that differs later is an error, as the entry describes only
/// one (an encoder opened again with other settings).
fn keep(held: &mut Option<Vec<u8>>, set: &[u8]) -> std::io::Result<()> {
    match held {
        Some(h) if h.as_slice() != set => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the stream's parameter sets changed")),
        Some(_) => Ok(()),
        None => {
            *held = Some(set.to_vec());
            Ok(())
        }
    }
}

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

/// The bitrate a screen's video is given: about a tenth of a bit a pixel
/// a frame (a 4K screen at 30 a second: 25 Mbit/s), 1 to 80 Mbit/s.
pub fn bitrate(width: u32, height: u32, fps: u32) -> u32 {
    (f64::from(width) * f64::from(height) * f64::from(fps) * 0.1).clamp(1e6, 80e6) as u32
}

/// What a video is coded as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    /// Plays everywhere; the processor's encoder makes only this.
    #[default]
    H264,
    /// About half H.264's size for the same picture (NVIDIA's encoder).
    Hevc,
    /// For players that want it (NVIDIA's encoder, from its Ada GPUs on):
    /// no smaller than H.264 on screens.
    Av1,
}

impl Codec {
    pub fn parse(s: &str) -> Option<Codec> {
        match s.to_ascii_lowercase().as_str() {
            "h264" | "h.264" | "avc" => Some(Codec::H264),
            "hevc" | "h265" | "h.265" => Some(Codec::Hevc),
            "av1" => Some(Codec::Av1),
            _ => None,
        }
    }

    /// As said to a person and to agents ("HEVC").
    pub fn name(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
        }
    }

    /// Its setting's value ("hevc").
    pub fn id(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
        }
    }

    /// The MP4 sample entry's kind.
    fn entry(self) -> &'static [u8; 4] {
        match self {
            Codec::H264 => b"avc1",
            Codec::Hevc => b"hvc1",
            Codec::Av1 => b"av01",
        }
    }

    fn of_entry(kind: &[u8]) -> Option<Codec> {
        match kind {
            b"avc1" | b"avc3" => Some(Codec::H264),
            b"hvc1" | b"hev1" => Some(Codec::Hevc),
            b"av01" => Some(Codec::Av1),
            _ => None,
        }
    }
}

/// NVENC's rate for `codec`: a constant quality, as many bits as keep it
/// (at most twice `bps`). Each keeps a screen's text as the bitrate did
/// (48.4–48.9 dB luma PSNR on a 1280 × 800 screen typed into, `codec_sizes`)
/// in fewer bytes: H.264 at 20, 190 KB for 3 s where the bitrate spent
/// 277; HEVC at 26, 105 (P6) or 145 KB (P4); AV1 at 28, about 220 (no
/// smaller than H.264 on screens: it lacks the screen content tools).
fn quality(codec: Codec, bps: u32, q: Quality) -> nvenc::Rate {
    let at = match codec {
        Codec::H264 => 20,
        Codec::Hevc => 26,
        Codec::Av1 => 28,
    };
    // (Each step down a third or so of the bits.)
    let (more, share) = match q {
        Quality::High => (0, 1.0),
        Quality::Medium => (4, 0.6),
        Quality::Low => (8, 0.35),
    };
    nvenc::Rate::Quality(at + more, (f64::from(bps.saturating_mul(2)) * share) as u32)
}

/// How finely a video is coded: a recording's own (`High`), or smaller
/// (an export's Medium and Low: about 60% and 35% of its bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    #[default]
    High,
    Medium,
    Low,
}

impl Quality {
    pub fn parse(s: &str) -> Option<Quality> {
        match s {
            "high" => Some(Quality::High),
            "medium" => Some(Quality::Medium),
            "low" => Some(Quality::Low),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Quality::High => "high",
            Quality::Medium => "medium",
            Quality::Low => "low",
        }
    }
}

/// Which encoder a recording uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The most powerful there is: NVIDIA's (NVENC) where it opens, else
    /// the processor.
    Auto,
    /// The processor (OpenH264).
    Cpu,
    /// NVIDIA's GPU (NVENC), or an error saying why not.
    Nvidia,
    /// Intel's or AMD's GPU through VA-API: AV1 only (what Fedora's
    /// drivers encode), or an error saying why not.
    Vaapi,
}

impl Choice {
    pub fn parse(s: &str) -> Option<Choice> {
        match s {
            "auto" => Some(Choice::Auto),
            "cpu" => Some(Choice::Cpu),
            "nvidia" | "nvenc" | "gpu" => Some(Choice::Nvidia),
            "vaapi" | "intel" | "amd" => Some(Choice::Vaapi),
            _ => None,
        }
    }

    /// As `--encoder` takes it.
    pub fn id(self) -> &'static str {
        match self {
            Choice::Auto => "auto",
            Choice::Cpu => "cpu",
            Choice::Nvidia => "nvidia",
            Choice::Vaapi => "vaapi",
        }
    }
}

/// An encoder: the GPU's (any [`Codec`]) or the processor's (H.264)
/// ([`Choice`]).
pub enum Encoder {
    Cpu(H264),
    Nvidia(nvenc::Nvenc, Vec<u8>),
    Vaapi(vaapi::Av1),
}

impl Encoder {
    /// One for `width` × `height` (even) pictures at up to `fps` a second,
    /// coded as `codec`. Auto takes NVIDIA's where it opens, else, for
    /// AV1, Intel's or AMD's through VA-API, else the processor, and so
    /// H.264 ([`Encoder::codec`] says which); the processor asked for
    /// another codec is an error, as VA-API asked for other than AV1.
    pub fn new(width: u32, height: u32, fps: u32, choice: Choice, codec: Codec) -> Result<Encoder, String> {
        Encoder::with_quality(width, height, fps, choice, codec, Quality::High)
    }

    /// [`new`](Self::new), coded as finely as `q` says.
    pub fn with_quality(width: u32, height: u32, fps: u32, choice: Choice, codec: Codec, q: Quality) -> Result<Encoder, String> {
        let bps = bitrate(width, height, fps);
        let nvidia = || nvenc::Nvenc::with_rate(width, height, fps.clamp(1, 120), quality(codec, bps, q), codec).map(|e| Encoder::Nvidia(e, Vec::new()));
        let cpu = || {
            if choice == Choice::Cpu && codec != Codec::H264 {
                return Err(format!("the processor encodes H.264 only, not {}", codec.name()));
            }
            let share = match q {
                Quality::High => 1.0,
                Quality::Medium => 0.6,
                Quality::Low => 0.35,
            };
            H264::with_bitrate(width, height, fps, (f64::from(bps) * share) as u32).map(Encoder::Cpu)
        };
        let vaapi = || {
            if codec != Codec::Av1 {
                return Err(format!("the GPU's driver encodes AV1 only here, not {}", codec.name()));
            }
            let more = match q {
                Quality::High => 0,
                Quality::Medium => 30,
                Quality::Low => 60,
            };
            vaapi::Av1::new(None, width, height, fps, vaapi::QUALITY.saturating_add(more)).map(Encoder::Vaapi)
        };
        match choice {
            Choice::Cpu => cpu(),
            Choice::Nvidia => nvidia(),
            Choice::Vaapi => vaapi(),
            // On battery, AV1 on the built-in GPU first: NVIDIA's would be
            // woken from its sleep for it.
            Choice::Auto if codec == Codec::Av1 && on_battery() => vaapi().or_else(|_| nvidia()).or_else(|_| cpu()),
            Choice::Auto => nvidia().or_else(|_| vaapi()).or_else(|_| cpu()),
        }
    }

    /// Which encoder it is.
    pub fn choice(&self) -> Choice {
        match self {
            Encoder::Cpu(_) => Choice::Cpu,
            Encoder::Nvidia(..) => Choice::Nvidia,
            Encoder::Vaapi(_) => Choice::Vaapi,
        }
    }

    /// The other encoders that make what this one does, best first (as
    /// `Auto` takes them): where a recording goes on if this one fails.
    pub fn others(&self) -> Vec<Choice> {
        let all: &[Choice] = match self.codec() {
            Codec::H264 => &[Choice::Nvidia, Choice::Cpu],
            Codec::Hevc => &[Choice::Nvidia],
            Codec::Av1 if on_battery() => &[Choice::Vaapi, Choice::Nvidia],
            Codec::Av1 => &[Choice::Nvidia, Choice::Vaapi],
        };
        all.iter().copied().filter(|c| *c != self.choice()).collect()
    }

    /// What it codes as.
    pub fn codec(&self) -> Codec {
        match self {
            Encoder::Cpu(_) => Codec::H264,
            Encoder::Nvidia(e, _) => e.codec(),
            Encoder::Vaapi(_) => Codec::Av1,
        }
    }

    /// What it is, as said to a person ("NVIDIA's GPU (NVENC)").
    pub fn name(&self) -> &'static str {
        match self {
            Encoder::Cpu(_) => "the processor (OpenH264)",
            Encoder::Nvidia(..) => "NVIDIA's GPU (NVENC)",
            Encoder::Vaapi(e) if e.driver().contains("Intel") => "the Intel GPU (VA-API)",
            Encoder::Vaapi(e) if e.driver().contains("AMD") || e.driver().contains("Radeon") => "the AMD GPU (VA-API)",
            Encoder::Vaapi(_) => "the GPU (VA-API)",
        }
    }

    pub fn size(&self) -> (u32, u32) {
        match self {
            Encoder::Cpu(e) => e.size(),
            Encoder::Nvidia(e, _) => e.size(),
            Encoder::Vaapi(e) => e.size(),
        }
    }

    /// One picture encoded: its Annex B bytes (AV1: a temporal unit's
    /// OBUs), and whether it's a key frame.
    pub fn encode(&mut self, data: &[u8], stride: usize, order: Order) -> Result<(Vec<u8>, bool), String> {
        match self {
            Encoder::Cpu(e) => e.encode(data, stride, order),
            Encoder::Nvidia(e, swapped) => match order {
                Order::Bgra => e.encode(data, stride),
                // (NVENC is given BGRA: red and blue swapped first.)
                Order::Rgba => {
                    swapped.clear();
                    swapped.extend_from_slice(data);
                    for px in swapped.chunks_exact_mut(4) {
                        px.swap(0, 2);
                    }
                    e.encode(swapped, stride)
                }
            },
            Encoder::Vaapi(e) => e.encode(data, stride, order == Order::Rgba),
        }
    }

    /// The GPU that can encode pictures in its memory as they are
    /// ([`encode_prime`](Self::encode_prime)), as its render node's
    /// device number: VA-API's (NVENC can't take another GPU's buffers,
    /// and the processor reads memory).
    pub fn prime_device(&self) -> Option<u64> {
        match self {
            Encoder::Vaapi(e) => e.device(),
            _ => None,
        }
    }

    /// A picture in that GPU's memory (a capture's dmabuf) encoded, never
    /// copied through the CPU; as [`encode`](Self::encode) otherwise.
    pub fn encode_prime(&mut self, picture: &vaapi::Prime) -> Result<(Vec<u8>, bool), String> {
        match self {
            Encoder::Vaapi(e) => e.encode_prime(picture),
            _ => Err(format!("{} encodes pictures in memory only", self.name())),
        }
    }
}

/// Whether the computer runs on its battery: it has one, and no mains
/// supply (or USB-C one) is online.
pub fn on_battery() -> bool {
    on_battery_in(std::path::Path::new("/sys/class/power_supply"))
}

/// [`on_battery`], from the power supplies in `supplies` (sysfs's).
fn on_battery_in(supplies: &std::path::Path) -> bool {
    let Ok(dir) = std::fs::read_dir(supplies) else { return false };
    let (mut battery, mut powered) = (false, false);
    for e in dir.flatten() {
        let kind = std::fs::read_to_string(e.path().join("type")).unwrap_or_default();
        let online = std::fs::read_to_string(e.path().join("online")).is_ok_and(|o| o.trim() == "1");
        match kind.trim() {
            "Battery" => battery |= std::fs::read_to_string(e.path().join("scope")).map_or(true, |s| s.trim() != "Device"),
            "Mains" | "USB" => powered |= online,
            _ => {}
        }
    }
    battery && !powered
}

/// An H.264 encoder for pictures of one size (even: H.264's; [`even`]).
pub struct H264 {
    encoder: OpenH264Encoder,
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
        H264::with_bitrate(width, height, fps, bitrate(width, height, fps.clamp(1, 120)))
    }

    /// [`new`](Self::new) at `bps` bits a second.
    pub fn with_bitrate(width: u32, height: u32, fps: u32, bps: u32) -> Result<H264, String> {
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || w % 2 == 1 || h % 2 == 1 {
            return Err(format!("{w}×{h} isn't an even size"));
        }
        let fps = fps.clamp(1, 120);
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
        let encoder = OpenH264Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| e.to_string())?;
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
    codec: Codec,
    /// The parameter sets (HEVC's VPS too), or AV1's sequence header OBU.
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    /// The sample entry as given (a trimmed copy's, its source's), in
    /// place of one made from the stream.
    entry: Option<Vec<u8>>,
    /// Sample entries before this one (another encoder's stream before,
    /// `new_entry`), each with its first sample (from 0).
    entries: Vec<(u32, Vec<u8>)>,
    /// This entry's first sample.
    entry_from: u32,
    /// Each sample's place in the file, size and duration.
    offsets: Vec<u64>,
    sizes: Vec<u32>,
    durations: Vec<u32>,
    keys: Vec<u32>,
    at: u64,
    /// Where `mdat`'s 64-bit size goes.
    mdat_size_at: u64,
    /// A sound track (AAC), if there is one.
    audio: Option<Audio>,
    /// Where playback starts in the first picture (ticks), by an edit
    /// list: a trimmed video's (`trim`).
    start: u32,
    /// Chapters (`chapters`): where each starts (milliseconds) and its
    /// title.
    chapters: Vec<(u64, String)>,
}

impl Mp4 {
    /// The video's sample entry, from the stream's parameter sets (or
    /// sequence header): `avc1` with `avcC`, `hvc1` with `hvcC`, `av01`
    /// with `av1C`.
    fn sample_entry(&self) -> Option<Vec<u8>> {
        let u16b = |v: u16| v.to_be_bytes().to_vec();
        let config = match self.codec {
            Codec::H264 => {
                let (sps, pps) = (self.sps.as_ref()?, self.pps.as_ref()?);
                bx(b"avcC", &[vec![1, *sps.get(1)?, *sps.get(2)?, *sps.get(3)?, 0xff, 0xe1], u16b(sps.len() as u16), sps.clone(), vec![1], u16b(pps.len() as u16), pps.clone()].concat())
            }
            Codec::Hevc => bx(b"hvcC", &config::hvcc(self.vps.as_ref()?, self.sps.as_ref()?, self.pps.as_ref()?)?),
            Codec::Av1 => bx(b"av1C", &config::av1c(self.sps.as_ref()?)?),
        };
        // (No compressor name: 32 zero bytes.)
        Some(bx(
            self.codec.entry(),
            &[
                vec![0; 6],
                u16b(1),
                vec![0; 16],
                u16b(self.width as u16),
                u16b(self.height as u16),
                0x0048_0000u32.to_be_bytes().to_vec(),
                0x0048_0000u32.to_be_bytes().to_vec(),
                vec![0; 4],
                u16b(1),
                vec![0; 32],
                u16b(0x18),
                u16b(0xffff),
                config,
            ]
            .concat(),
        ))
    }
}

/// An MP4's sound track: AAC access units of `aac::FRAME` samples each.
struct Audio {
    /// AudioSpecificConfig.
    config: Vec<u8>,
    rate: u32,
    channels: u32,
    /// The encoder's priming, cut by an edit list.
    delay: u32,
    bitrate: u32,
    offsets: Vec<u64>,
    sizes: Vec<u32>,
}

impl Mp4 {
    /// A new MP4 at `path` of `width` × `height`, its durations in
    /// `timescale` ticks a second.
    pub fn create(path: &Path, width: u32, height: u32, timescale: u32) -> std::io::Result<Mp4> {
        Mp4::create_for(path, width, height, timescale, Codec::H264)
    }

    /// [`Mp4::create`] for a video coded as `codec` (what [`Mp4::add`]
    /// is given is that encoder's).
    pub fn create_for(path: &Path, width: u32, height: u32, timescale: u32, codec: Codec) -> std::io::Result<Mp4> {
        let mut file = BufWriter::new(File::create(path)?);
        let brand: &[u8] = match codec {
            Codec::H264 => b"avc1",
            Codec::Hevc => b"iso4",
            Codec::Av1 => b"av01",
        };
        let ftyp = bx(b"ftyp", &[b"isom".as_slice(), &0x200u32.to_be_bytes(), b"isom", b"iso2", brand, b"mp41"].concat());
        file.write_all(&ftyp)?;
        // `mdat` with a 64-bit size (size 1, then the large size), filled
        // in at the end.
        let mdat_size_at = ftyp.len() as u64 + 8;
        file.write_all(&1u32.to_be_bytes())?;
        file.write_all(b"mdat")?;
        file.write_all(&0u64.to_be_bytes())?;
        let at = mdat_size_at + 8;
        Ok(Mp4 {
            file,
            width,
            height,
            timescale: timescale.max(1),
            codec,
            vps: None,
            sps: None,
            pps: None,
            entry: None,
            entries: Vec::new(),
            entry_from: 0,
            offsets: Vec::new(),
            sizes: Vec::new(),
            durations: Vec::new(),
            keys: Vec::new(),
            at,
            mdat_size_at,
            audio: None,
            start: 0,
            chapters: Vec::new(),
        })
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

    /// A copy of another MP4's video (`trim`): its sample entry (`entry`,
    /// the whole box) kept as it is, its samples given whole
    /// ([`Mp4::add_sample`]).
    pub fn copying(path: &Path, width: u32, height: u32, timescale: u32, entry: Vec<u8>) -> std::io::Result<Mp4> {
        let codec = entry.get(4..8).and_then(Codec::of_entry).unwrap_or_default();
        let mut mp4 = Mp4::create_for(path, width, height, timescale, codec)?;
        mp4.entry = Some(entry);
        Ok(mp4)
    }

    /// Another encoder's stream from here on (one opened after the last
    /// failed): its parameter sets, which may differ, go in a sample entry
    /// of their own, from its first picture (a key frame). The same sets
    /// again make no new one (`finish` joins them).
    pub fn new_entry(&mut self) -> std::io::Result<()> {
        self.close_entry(None)
    }

    /// A copy's next sample entry (`copying`): another of its source's,
    /// from the next sample on.
    pub fn next_entry(&mut self, entry: Vec<u8>) -> std::io::Result<()> {
        self.close_entry(Some(entry))
    }

    fn close_entry(&mut self, next: Option<Vec<u8>>) -> std::io::Result<()> {
        let here = self.sizes.len() as u32;
        if here > self.entry_from {
            let done = match self.entry.take() {
                Some(e) => e,
                None => self.sample_entry().ok_or_else(|| std::io::Error::other("the encoder gave no parameter sets"))?,
            };
            self.entries.push((self.entry_from, done));
            self.entry_from = here;
        }
        (self.vps, self.sps, self.pps) = (None, None, None);
        if next.is_some() {
            self.entry = next;
        }
        Ok(())
    }

    /// One encoded picture shown for `duration` ticks, as its encoder gave
    /// it: H.264 or HEVC in Annex B, AV1 as OBUs. Its parameter sets (or
    /// sequence header) are kept for the track's description, not in the
    /// sample; access unit delimiters, temporal delimiters and padding go.
    pub fn add(&mut self, coded: &[u8], key: bool, duration: u32) -> std::io::Result<()> {
        let mut sample = Vec::with_capacity(coded.len());
        let unit = |u: &[u8], sample: &mut Vec<u8>| {
            sample.extend_from_slice(&(u.len() as u32).to_be_bytes());
            sample.extend_from_slice(u);
        };
        match self.codec {
            Codec::H264 => {
                for u in nal_units(coded) {
                    match u[0] & 0x1f {
                        7 => keep(&mut self.sps, u)?,
                        8 => keep(&mut self.pps, u)?,
                        9 => {}
                        _ => unit(u, &mut sample),
                    }
                }
            }
            Codec::Hevc => {
                for u in nal_units(coded) {
                    match (u[0] >> 1) & 0x3f {
                        32 => keep(&mut self.vps, u)?,
                        33 => keep(&mut self.sps, u)?,
                        34 => keep(&mut self.pps, u)?,
                        35 => {}
                        _ => unit(u, &mut sample),
                    }
                }
            }
            Codec::Av1 => {
                let mut b = coded;
                while let Some((kind, head, size)) = config::obu(b) {
                    let o = &b[..head + size];
                    match kind {
                        1 => keep(&mut self.sps, o)?,
                        2 | 15 => {}
                        _ => sample.extend_from_slice(o),
                    }
                    b = &b[head + size..];
                }
            }
        }
        self.add_sample(&sample, key, duration)
    }

    /// One sample, as the file holds it (length-prefixed NAL units, or
    /// OBUs), shown for `duration` ticks.
    pub fn add_sample(&mut self, sample: &[u8], key: bool, duration: u32) -> std::io::Result<()> {
        if sample.is_empty() {
            return Ok(());
        }
        // (A later sample entry's stream starts at a key frame.)
        if !key && self.entry_from > 0 && self.sizes.len() as u32 == self.entry_from {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "another encoder's stream must start with a key frame"));
        }
        self.file.write_all(sample)?;
        self.offsets.push(self.at);
        self.sizes.push(sample.len() as u32);
        self.durations.push(duration.max(1));
        self.at += sample.len() as u64;
        if key {
            self.keys.push(self.sizes.len() as u32);
        }
        Ok(())
    }

    /// A sound track too, of the access units of the encoder `track`
    /// describes (`add_sound`).
    pub fn with_sound(&mut self, track: &aac::Track, rate: u32, bitrate: u32) {
        self.audio = Some(Audio { config: track.config.clone(), rate, channels: track.channels, delay: track.delay, bitrate, offsets: Vec::new(), sizes: Vec::new() });
    }

    /// One AAC access unit (`aac::FRAME` samples) of the sound track.
    pub fn add_sound(&mut self, au: &[u8]) -> std::io::Result<()> {
        let Some(a) = self.audio.as_mut() else { return Ok(()) };
        self.file.write_all(au)?;
        a.offsets.push(self.at);
        a.sizes.push(au.len() as u32);
        self.at += au.len() as u64;
        Ok(())
    }

    /// Played from `ticks` into the video (an edit list), what's before
    /// kept only to decode from (a trimmed recording starts at its key
    /// frame before).
    /// Chapters: each mark's start (milliseconds into the video) and
    /// title, written as Nero's `chpl` (in `moov`'s `udta`), which
    /// FFmpeg, mpv and VLC read; at most 255, titles cut to 255 bytes.
    pub fn chapters(&mut self, marks: Vec<(u64, String)>) {
        self.chapters = marks;
    }

    pub fn start_at(&mut self, ticks: u32) {
        self.start = ticks;
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
        // Each sample entry from its first sample (one with none left out),
        // one the same as the one before joined to it.
        let mut entries = std::mem::take(&mut self.entries);
        if self.sizes.len() as u32 > self.entry_from || entries.is_empty() {
            let last = match self.entry.take() {
                Some(e) => e,
                None => self.sample_entry().ok_or_else(|| std::io::Error::other("the encoder gave no parameter sets"))?,
            };
            entries.push((self.entry_from, last));
        }
        entries.dedup_by(|later, earlier| later.1 == earlier.1);
        let n = self.sizes.len() as u32;
        let duration: u64 = self.durations.iter().map(|&d| u64::from(d)).sum();
        let duration = duration.min(u64::from(u32::MAX)) as u32;
        let u32b = |v: u32| v.to_be_bytes().to_vec();
        let u16b = |v: u16| v.to_be_bytes().to_vec();
        let matrix: Vec<u8> = [0x10000u32, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000].iter().flat_map(|v| v.to_be_bytes()).collect();
        // The sound's length, its priming cut, in the movie's ticks.
        let sound = self.audio.as_ref().filter(|a| !a.sizes.is_empty()).map(|a| {
            let samples = (a.sizes.len() as u64 * aac::FRAME as u64).saturating_sub(u64::from(a.delay));
            (samples * u64::from(self.timescale) / u64::from(a.rate.max(1))).min(u64::from(u32::MAX)) as u32
        });
        // (What's before an edit list's start isn't shown.)
        let shown = duration.saturating_sub(self.start);
        let movie = shown.max(sound.unwrap_or(0));
        let next_track = if sound.is_some() { 3 } else { 2 };
        let mvhd = full(b"mvhd", 0, &[u32b(0), u32b(0), u32b(self.timescale), u32b(movie), u32b(0x10000), u16b(0x100), vec![0; 10], matrix.clone(), vec![0; 24], u32b(next_track)].concat());
        let tkhd = full(
            b"tkhd",
            3,
            &[u32b(0), u32b(0), u32b(1), u32b(0), u32b(shown), vec![0; 8], u16b(0), u16b(0), u16b(0), u16b(0), matrix.clone(), u32b(self.width << 16), u32b(self.height << 16)].concat(),
        );
        let mdhd = full(b"mdhd", 0, &[u32b(0), u32b(0), u32b(self.timescale), u32b(duration), u16b(0x55c4), u16b(0)].concat());
        let hdlr = full(b"hdlr", 0, &[u32b(0), b"vide".to_vec(), vec![0; 12], b"Olo video\0".to_vec()].concat());
        let stsd = full(b"stsd", 0, &[u32b(entries.len() as u32), entries.iter().flat_map(|(_, e)| e.clone()).collect()].concat());
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
        // Each sample a chunk of its own, where it is in the file; each
        // entry's from its first.
        let rows: Vec<u8> = entries.iter().enumerate().flat_map(|(i, (from, _))| [u32b(from + 1), u32b(1), u32b(i as u32 + 1)].concat()).collect();
        let stsc = full(b"stsc", 0, &[u32b(entries.len() as u32), rows].concat());
        let stsz = full(b"stsz", 0, &[u32b(0), u32b(n), self.sizes.iter().flat_map(|s| s.to_be_bytes()).collect()].concat());
        let co64 = full(b"co64", 0, &[u32b(n), self.offsets.iter().flat_map(|o| o.to_be_bytes()).collect()].concat());
        let stbl = bx(b"stbl", &[stsd, stts, stss, stsc, stsz, co64].concat());
        let vmhd = full(b"vmhd", 1, &[0u8; 8]);
        let dinf = bx(b"dinf", &full(b"dref", 0, &[u32b(1), full(b"url ", 1, &[])].concat()));
        let minf = bx(b"minf", &[vmhd, dinf, stbl].concat());
        let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
        let edts = if self.start > 0 { bx(b"edts", &full(b"elst", 0, &[u32b(1), u32b(shown), u32b(self.start), u16b(1), u16b(0)].concat())) } else { Vec::new() };
        let trak = bx(b"trak", &[tkhd, edts, mdia].concat());
        let sound_trak = match (&self.audio, sound) {
            (Some(a), Some(length)) => sound_track(a, length, &matrix),
            _ => Vec::new(),
        };
        let udta = if self.chapters.is_empty() {
            Vec::new()
        } else {
            let mut body = vec![0u8; 4];
            let marks = &self.chapters[..self.chapters.len().min(255)];
            body.push(marks.len() as u8);
            for (ms, title) in marks {
                // (Hundreds of nanoseconds; the title cut at a character.)
                body.extend_from_slice(&(ms * 10_000).to_be_bytes());
                let mut end = title.len().min(255);
                while !title.is_char_boundary(end) {
                    end -= 1;
                }
                body.push(end as u8);
                body.extend_from_slice(&title.as_bytes()[..end]);
            }
            bx(b"udta", &full(b"chpl", 0x0100_0000, &body))
        };
        let moov = bx(b"moov", &[mvhd, trak, sound_trak, udta].concat());
        self.file.write_all(&moov)?;
        // `mdat`'s size: its header (16) and every sample.
        let mdat_size = self.at - (self.mdat_size_at - 8);
        let mut file = self.file.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(self.mdat_size_at))?;
        file.write_all(&mdat_size.to_be_bytes())?;
        file.sync_all()
    }
}

/// The sound track's `trak`: AAC in `mp4a`, its priming cut by an edit
/// list, `length` long in the movie's ticks.
fn sound_track(a: &Audio, length: u32, matrix: &[u8]) -> Vec<u8> {
    let u32b = |v: u32| v.to_be_bytes().to_vec();
    let u16b = |v: u16| v.to_be_bytes().to_vec();
    let n = a.sizes.len() as u32;
    let media = n.saturating_mul(aac::FRAME as u32);
    let tkhd = full(b"tkhd", 3, &[u32b(0), u32b(0), u32b(2), u32b(0), u32b(length), vec![0; 8], u16b(0), u16b(1), u16b(0x0100), u16b(0), matrix.to_vec(), u32b(0), u32b(0)].concat());
    // Played from after the encoder's priming.
    let elst = full(b"elst", 0, &[u32b(1), u32b(length), u32b(a.delay), u16b(1), u16b(0)].concat());
    let edts = bx(b"edts", &elst);
    let mdhd = full(b"mdhd", 0, &[u32b(0), u32b(0), u32b(a.rate), u32b(media), u16b(0x55c4), u16b(0)].concat());
    let hdlr = full(b"hdlr", 0, &[u32b(0), b"soun".to_vec(), vec![0; 12], b"Olo sound\0".to_vec()].concat());
    // The decoder's configuration: an ES descriptor holding a decoder
    // config descriptor (MPEG-4 audio, an audio stream) holding the
    // AudioSpecificConfig, and the sync layer's.
    let desc = |tag: u8, body: Vec<u8>| [vec![tag, body.len() as u8], body].concat();
    let dsi = desc(0x05, a.config.clone());
    let dcd = desc(0x04, [vec![0x40, 0x15, 0, 0x18, 0], u32b(a.bitrate), u32b(a.bitrate), dsi].concat());
    let es = desc(0x03, [u16b(2), vec![0], dcd, desc(0x06, vec![0x02])].concat());
    let esds = full(b"esds", 0, &es);
    let mp4a = bx(b"mp4a", &[vec![0; 6], u16b(1), vec![0; 8], u16b(a.channels as u16), u16b(16), u16b(0), u16b(0), u32b(a.rate << 16), esds].concat());
    let stsd = full(b"stsd", 0, &[u32b(1), mp4a].concat());
    let stts = full(b"stts", 0, &[u32b(1), u32b(n), u32b(aac::FRAME as u32)].concat());
    let stsc = full(b"stsc", 0, &[u32b(1), u32b(1), u32b(1), u32b(1)].concat());
    let stsz = full(b"stsz", 0, &[u32b(0), u32b(n), a.sizes.iter().flat_map(|s| s.to_be_bytes()).collect()].concat());
    let co64 = full(b"co64", 0, &[u32b(n), a.offsets.iter().flat_map(|o| o.to_be_bytes()).collect()].concat());
    let stbl = bx(b"stbl", &[stsd, stts, stsc, stsz, co64].concat());
    let smhd = full(b"smhd", 0, &[0u8; 4]);
    let dinf = bx(b"dinf", &full(b"dref", 0, &[u32b(1), full(b"url ", 1, &[])].concat()));
    let minf = bx(b"minf", &[smhd, dinf, stbl].concat());
    let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
    bx(b"trak", &[tkhd, edts, mdia].concat())
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
    /// How long opening an encoder takes (NVENC's CUDA context), run by
    /// hand: `cargo test -p olo-video encoder_opening -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn encoder_opening() {
        for choice in [Choice::Nvidia, Choice::Cpu] {
            let t = std::time::Instant::now();
            let e = Encoder::new(1280, 800, 30, choice, Codec::H264);
            eprintln!("{choice:?}: {:?} in {:?}", e.as_ref().map(|e| e.name()), t.elapsed());
        }
    }

    #[test]
    fn an_mp4_with_sound() {
        let Ok(mut aac) = aac::Aac::new(48_000, 2, 160_000) else { return eprintln!("no libfdk-aac here") };
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-video-sound-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("s.mp4");
        let (w, h) = (64u32, 48u32);
        let mut enc = H264::new(w, h, 30).unwrap();
        let mut mp4 = Mp4::create(&path, w, h, 1000).unwrap();
        mp4.with_sound(&aac.track(), 48_000, 160_000);
        // Two seconds: a picture a second, a tone throughout.
        let mut sent = 0usize;
        for second in 0..2u8 {
            let px: Vec<u8> = (0..(w * h)).flat_map(|p| [second * 100, (p % 255) as u8, 128, 255]).collect();
            let (bits, key) = enc.encode(&px, w as usize * 4, Order::Bgra).unwrap();
            mp4.add(&bits, key, 1000).unwrap();
            while sent < (second as usize + 1) * 48_000 {
                let frame: Vec<i16> = (0..aac::FRAME)
                    .flat_map(|i| {
                        let v = (((sent + i) as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16;
                        [v, v]
                    })
                    .collect();
                sent += aac::FRAME;
                for au in aac.encode(&frame).unwrap() {
                    mp4.add_sound(&au).unwrap();
                }
            }
        }
        for au in aac.flush().unwrap() {
            mp4.add_sound(&au).unwrap();
        }
        mp4.finish().unwrap();
        // Read back: a video track and an AAC one, about as long.
        let f = std::fs::File::open(&path).unwrap();
        let size = f.metadata().unwrap().len();
        let r = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).unwrap();
        assert_eq!(r.tracks().len(), 2);
        let sound = r.tracks().values().find(|t| t.track_type().ok() == Some(mp4::TrackType::Audio)).expect("a sound track");
        assert_eq!(sound.media_type().unwrap(), mp4::MediaType::AAC);
        assert_eq!(sound.sample_freq_index().unwrap().freq(), 48_000);
        assert_eq!(sound.channel_config().unwrap(), mp4::ChannelConfig::Stereo);
        let secs = sound.duration().as_secs_f64();
        assert!((1.9..2.2).contains(&secs), "{secs} s of sound");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Another encoder's stream after `new_entry`: its parameter sets in a
    /// sample entry of their own from its first picture, mapped by
    /// `stsc`; the same sets again join the entry before; Trim across the
    /// switch keeps both. (Made-up NAL units: the file's structure only.)
    #[test]
    fn another_encoders_stream_gets_its_own_sample_entry() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-entries-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let annex = |units: &[&[u8]]| units.iter().flat_map(|u| [&[0u8, 0, 0, 1][..], u].concat()).collect::<Vec<u8>>();
        let (sps_a, sps_b, pps): (&[u8], &[u8], &[u8]) = (&[0x67, 0x42, 0xc0, 0x1e, 1], &[0x67, 0x64, 0x00, 0x28, 2], &[0x68, 0xce]);
        let (idr, p): (&[u8], &[u8]) = (&[0x65, 0x88, 1], &[0x41, 0x9a, 2]);
        let path = dir.join("two.mp4");
        let mut mp4 = Mp4::create(&path, 64, 48, 1000).unwrap();
        mp4.add(&annex(&[sps_a, pps, idr]), true, 100).unwrap();
        mp4.add(&annex(&[p]), false, 100).unwrap();
        // The same encoder opened again: the same sets, no new entry.
        mp4.new_entry().unwrap();
        mp4.add(&annex(&[sps_a, pps, idr]), true, 100).unwrap();
        // Another: its first picture must be a key frame.
        mp4.new_entry().unwrap();
        assert!(mp4.add(&annex(&[sps_b, pps, p]), false, 100).is_err());
        mp4.add(&annex(&[sps_b, pps, idr]), true, 100).unwrap();
        mp4.add(&annex(&[p]), false, 100).unwrap();
        mp4.finish().unwrap();
        let entries = trim::video_entries(&path).unwrap();
        assert_eq!(entries.iter().map(|(from, _)| *from).collect::<Vec<_>>(), [1, 4]);
        assert_ne!(entries[0].1, entries[1].1);
        assert!(entries[1].1.windows(sps_b.len()).any(|w| w == sps_b), "the second entry's avcC has the new SPS");
        let f = std::fs::File::open(&path).unwrap();
        let size = f.metadata().unwrap().len();
        let r = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).unwrap();
        assert_eq!(r.tracks().values().next().unwrap().sample_count(), 5);
        // Trimmed across the switch: both entries, from their samples.
        let cut = dir.join("cut.mp4");
        trim::trim(&path, &cut, 0.25, 0.5).unwrap();
        let kept = trim::video_entries(&cut).unwrap();
        // (From the key frame before the cut, sample 1: the switch is its 4th.)
        assert_eq!(kept.iter().map(|(from, _)| *from).collect::<Vec<_>>(), [1, 4], "{kept:?}");
        assert_eq!((&kept[0].1, &kept[1].1), (&entries[0].1, &entries[1].1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A recording whose NVIDIA encoder failed going on with the
    /// processor's: one file, two sample entries, every picture decoded
    /// (the GIF maker's decoder taking each entry's sets).
    #[test]
    fn a_recording_goes_on_with_another_encoder() {
        if !nvenc::available() {
            eprintln!("no NVIDIA encoder here: nothing to try");
            return;
        }
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-fallback-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (w, h) = (320u32, 192u32);
        let path = dir.join("fallback.mp4");
        let mut mp4 = Mp4::create_for(&path, w, h, 1000, Codec::H264).unwrap();
        let mut enc = Encoder::new(w, h, 30, Choice::Nvidia, Codec::H264).unwrap();
        let frame = |i: u32| -> Vec<u8> { (0..w * h).flat_map(|p| if (p / w + 192 - i * 3 % 192) % 192 < 16 { [230u8, 230, 230, 255] } else { [40, 40, 40, 255] }).collect() };
        for i in 0..60u32 {
            if i == 30 {
                // NVENC "fails": the next of the codec takes over.
                assert_eq!(enc.others(), [Choice::Cpu]);
                enc = Encoder::new(w, h, 30, Choice::Cpu, Codec::H264).unwrap();
                mp4.new_entry().unwrap();
            }
            let (bits, key) = enc.encode(&frame(i), w as usize * 4, Order::Bgra).unwrap();
            mp4.add(&bits, key, 33).unwrap();
        }
        mp4.finish().unwrap();
        let entries = trim::video_entries(&path).unwrap();
        assert_eq!(entries.iter().map(|(from, _)| *from).collect::<Vec<_>>(), [1, 31]);
        let v = read::tests_open(&path).unwrap();
        let mut seen = 0;
        v.each_decoded(&mut |_| {
            seen += 1;
            Ok(())
        })
        .unwrap();
        assert!(seen >= 58, "{seen} pictures decoded of 60");
        eprintln!("NVENC then OpenH264: {seen} pictures decoded, {} bytes, at {}", std::fs::metadata(&path).unwrap().len(), path.display());
        if std::env::var_os("OLO_KEEP_TEST_VIDEO").is_none() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// An encoder opened again with the same settings goes on in the same
    /// file; one whose parameter sets differ is refused (the file's entry
    /// describes one), and what came before is still a whole file.
    #[test]
    fn changed_parameter_sets_are_refused() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-params-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("p.mp4");
        let px = |w: u32, h: u32| vec![128u8; (w * h * 4) as usize];
        let mut mp4 = Mp4::create(&path, 64, 48, 1000).unwrap();
        let mut a = H264::new(64, 48, 30).unwrap();
        let (bits, key) = a.encode(&px(64, 48), 64 * 4, Order::Bgra).unwrap();
        mp4.add(&bits, key, 33).unwrap();
        // The same settings again: the same sets, going on.
        let mut again = H264::new(64, 48, 30).unwrap();
        let (bits, key) = again.encode(&px(64, 48), 64 * 4, Order::Bgra).unwrap();
        assert!(key);
        mp4.add(&bits, key, 33).unwrap();
        // Another size: refused, the file as it was.
        let mut other = H264::new(96, 64, 30).unwrap();
        let (bits, key) = other.encode(&px(96, 64), 96 * 4, Order::Bgra).unwrap();
        assert_eq!(mp4.add(&bits, key, 33).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(mp4.len(), 2);
        mp4.finish().unwrap();
        let f = std::fs::File::open(&path).unwrap();
        let size = f.metadata().unwrap().len();
        let r = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).unwrap();
        assert_eq!(r.tracks().values().next().unwrap().sample_count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    /// NVIDIA's HEVC and AV1, where there's an encoder for them: an MP4
    /// of each made (its `hvcC` or `av1C` from the stream), its pictures
    /// decoded back (as a GIF is made), trimmed, and a GIF of it
    /// (`--ignored`: it needs the GPU).
    #[test]
    #[ignore]
    fn nvenc_makes_hevc_and_av1_mp4s() {
        if !nvenc::available() {
            eprintln!("no NVIDIA encoder here: nothing to try");
            return;
        }
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-codec-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (w, h) = (640u32, 360u32);
        for codec in [Codec::Hevc, Codec::Av1] {
            let mut e = match Encoder::new(w, h, 30, Choice::Nvidia, codec) {
                Ok(e) => e,
                Err(err) => panic!("{}: {err}", codec.name()),
            };
            assert_eq!(e.codec(), codec);
            let path = dir.join(format!("{}.mp4", codec.id()));
            let mut mp4 = Mp4::create_for(&path, w, h, 1000, codec).unwrap();
            let mut keys = 0;
            for i in 0..90u32 {
                // A bar moving down a grey picture, a third of a second each.
                let px: Vec<u8> = (0..w * h)
                    .flat_map(|p| {
                        let y = p / w;
                        let v = if (y + 360 - (i * 4) % 360) % 360 < 30 { 230 } else { 60 };
                        [v, v, v, 255]
                    })
                    .collect();
                let (bits, key) = e.encode(&px, w as usize * 4, Order::Bgra).unwrap();
                keys += usize::from(key);
                mp4.add(&bits, key, 33).unwrap();
            }
            assert!(keys >= 1);
            mp4.finish().unwrap();
            let entry = trim::video_entry(&path).unwrap();
            assert_eq!(&entry[4..8], codec.entry());
            // Decoded back, every picture.
            let v = read::tests_open(&path).unwrap();
            let mut seen = 0;
            v.each_decoded(&mut |rgb| {
                seen += 1;
                // The bar's rows bright, the rest dark.
                let at = |y: usize| rgb[(y * w as usize + 300) * 3];
                let bar = (0..h as usize).find(|&y| at(y) > 180 && at((y + 360 - 1) % 360) < 120);
                assert!(at(((seen - 1) * 4 % 360 + 15) % 360) > 180 || seen > 85, "picture {seen}: bar at {bar:?}");
                Ok(())
            })
            .unwrap();
            assert!(seen >= 88, "{}: {seen} pictures", codec.name());
            // Trimmed: its entry copied, about a second.
            let cut = dir.join(format!("{}-cut.mp4", codec.id()));
            let t = trim::trim(&path, &cut, 0.5, 1.6).unwrap();
            assert!(t.frames >= 30, "{t:?}");
            assert_eq!(trim::video_entry(&cut).unwrap(), entry);
            // And made a GIF.
            let g = gif::from_mp4(&path, &dir.join(format!("{}.gif", codec.id())), &gif::Options::default(), &std::sync::atomic::AtomicBool::new(false), &|_| {}).unwrap();
            assert!(g.frames >= 10, "{g:?}");
            eprintln!("{}: {} bytes, {seen} pictures decoded, trimmed to {} frames, GIF of {}", codec.name(), std::fs::metadata(&path).unwrap().len(), t.frames, g.frames);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a 4K frame costs NVENC in each codec (`--ignored`: the GPU).
    #[test]
    #[ignore]
    fn nvenc_cost_by_codec() {
        if !nvenc::available() {
            return;
        }
        let (w, h) = (3840u32, 2160u32);
        let mut px: Vec<u8> = (0..w * h)
            .flat_map(|p| {
                let (x, y) = (p % w, p / w);
                let v = if (y / 24) % 2 == 0 && (x / 7) % 3 != 0 { 40 } else { 235 };
                [v, v, v, 255]
            })
            .collect();
        for codec in [Codec::H264, Codec::Hevc, Codec::Av1] {
            let mut e = nvenc::Nvenc::with_rate(w, h, 30, nvenc::Rate::Quality(24, 50_000_000), codec).unwrap();
            let mut times = Vec::new();
            for i in 0..40u32 {
                for y in (i * 20) % (h - 40)..(i * 20) % (h - 40) + 40 {
                    for x in 100..2000 {
                        px[((y * w + x) * 4) as usize] = (i * 6) as u8;
                    }
                }
                let t = std::time::Instant::now();
                e.encode(&px, w as usize * 4).unwrap();
                times.push(t.elapsed());
            }
            times.sort();
            eprintln!("{:5} 4K: median {:?}, slowest {:?} a frame", codec.name(), times[20], times[39]);
        }
    }

    /// A real screen (`OLO_BENCH_SCREEN`, a PNG; None if unset) with
    /// typing drawn onto it, 90 frames of BGRA (or, with
    /// `OLO_BENCH_SCROLL`, the screen scrolling 6 rows a frame): its
    /// (even) size and the frames.
    fn bench_frames() -> Option<(u32, u32, Vec<Vec<u8>>)> {
        let Some(path) = std::env::var_os("OLO_BENCH_SCREEN") else {
            eprintln!("OLO_BENCH_SCREEN unset: nothing to try");
            return None;
        };
        let png = std::fs::read(&path).unwrap();
        let mut dec = png::Decoder::new(std::io::Cursor::new(png));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut reader = dec.read_info().unwrap();
        let mut rgba = vec![0u8; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut rgba).unwrap();
        let (w, h) = (info.width & !1, info.height & !1);
        let base: Vec<u8> = (0..h)
            .flat_map(|y| {
                (0..w).flat_map({
                    let rgba = &rgba;
                    move |x| {
                        let i = ((y * info.width + x) * 4) as usize;
                        [rgba[i + 2], rgba[i + 1], rgba[i], 255]
                    }
                })
            })
            .collect();
        // Frame i: "typing": a glyph-like block per frame along a line.
        let frame = |i: u32| {
            let mut px = base.clone();
            for k in 0..=i {
                let (x0, y0) = (400 + (k % 60) * 14, 700 + (k / 60) * 30);
                for y in y0..(y0 + 22).min(h) {
                    for x in x0..(x0 + 9).min(w) {
                        if (x + y + k) % 3 != 0 {
                            let o = ((y * w + x) * 4) as usize;
                            px[o..o + 3].copy_from_slice(&[30, 30, 30]);
                        }
                    }
                }
            }
            px
        };
        let scroll = std::env::var_os("OLO_BENCH_SCROLL").is_some();
        let frames: Vec<Vec<u8>> = (0..90)
            .map(|i| {
                if !scroll {
                    return frame(i);
                }
                let row = w as usize * 4;
                let by = (i as usize * 6) % h as usize;
                let mut px = base[by * row..].to_vec();
                px.extend_from_slice(&base[..by * row]);
                px
            })
            .collect();
        Some((w, h, frames))
    }

    /// An MP4's luma PSNR against the `frames` it was made from, over the
    /// pictures decoded (BT.601 luma, as the others were measured), and
    /// how many decoded.
    fn mp4_psnr(file: &std::path::Path, w: u32, h: u32, frames: &[Vec<u8>]) -> (f64, usize) {
        let luma = |p: &[u8], o: usize| 0.299 * f64::from(p[o + 2]) + 0.587 * f64::from(p[o + 1]) + 0.114 * f64::from(p[o]);
        let v = read::tests_open(file).unwrap();
        let (mut n, mut mse_sum) = (0usize, 0f64);
        v.each_decoded(&mut |rgb| {
            let src = &frames[n];
            let mut mse = 0f64;
            for p in 0..(w * h) as usize {
                let y_src = luma(src, p * 4);
                let y_dec = 0.299 * f64::from(rgb[p * 3]) + 0.587 * f64::from(rgb[p * 3 + 1]) + 0.114 * f64::from(rgb[p * 3 + 2]);
                mse += (y_src - y_dec).powi(2);
            }
            mse_sum += mse / f64::from(w * h);
            n += 1;
            Ok(())
        })
        .unwrap();
        (10.0 * (255f64.powi(2) / (mse_sum / n.max(1) as f64)).log10(), n)
    }

    /// Sizes and picture quality of each codec at each rate, on a real
    /// screen (`bench_frames`) three seconds at 30 a second: what Olo's
    /// recordings should be set to (`--ignored`: it needs the GPU).
    #[test]
    #[ignore]
    fn codec_sizes() {
        if !nvenc::available() {
            return;
        }
        let Some((w, h, frames)) = bench_frames() else { return };
        let bps = bitrate(w, h, 30);
        let codecs: Vec<Codec> = std::env::var("OLO_BENCH_CODECS").ok().map(|v| v.split(',').filter_map(Codec::parse).collect()).unwrap_or_else(|| vec![Codec::H264, Codec::Hevc, Codec::Av1]);
        for codec in codecs {
            let mut rates = vec![nvenc::Rate::Bitrate(if codec == Codec::H264 { bps } else { bps / 3 * 2 })];
            let qs: Vec<u8> = match std::env::var("OLO_BENCH_Q") {
                Ok(v) => v.split(',').filter_map(|q| q.parse().ok()).collect(),
                Err(_) => vec![20, 26, 30, 34],
            };
            rates.extend(qs.into_iter().map(|q| nvenc::Rate::Quality(q, bps * 2)));
            for rate in rates {
                let mut e = nvenc::Nvenc::with_rate(w, h, 30, rate, codec).unwrap();
                // (On disk, not /tmp: that is memory.)
                let file = std::path::PathBuf::from(format!("/var/tmp/olo-bench-{}.mp4", std::process::id()));
                let mut mp4 = Mp4::create_for(&file, w, h, 1000, codec).unwrap();
                for f in &frames {
                    let (bits, key) = e.encode(f, w as usize * 4).unwrap();
                    mp4.add(&bits, key, 33).unwrap();
                }
                mp4.finish().unwrap();
                let bytes = std::fs::metadata(&file).unwrap().len();
                let (psnr, n) = mp4_psnr(&file, w, h, &frames);
                eprintln!("{:5} {:24} {:9} bytes  PSNR {:5.2} dB  ({n} decoded)", codec.name(), format!("{rate:?}"), bytes, psnr);
                let _ = std::fs::remove_file(&file);
            }
        }
    }

    /// AV1's sequence header as written for VA-API reads back as its
    /// av1C says it.
    #[test]
    fn the_vaapi_sequence_header_reads_back() {
        let seq = vaapi::obu(vaapi::OBU_SEQUENCE_HEADER, &vaapi::sequence_header(2560, 1600, 12), 1);
        let c = config::av1c(&seq).expect("av1C");
        // Marker and version, profile 0 at level 12, Main tier, 8 bits,
        // 4:2:0.
        assert_eq!(c[0], 0x81);
        assert_eq!(c[1], 12);
        assert_eq!(c[2] & 0b0110_1100, 0b0000_1100);
    }

    /// Encodes a few frames on this computer's GPU (where VA-API encodes
    /// AV1) and decodes them back with rav1d: the pictures come back.
    #[test]
    fn vaapi_frames_come_back() {
        let Some(node) = vaapi::av1_node() else {
            eprintln!("no GPU encodes AV1 through VA-API here");
            return;
        };
        let (w, h) = (640u32, 360u32);
        let mut e = vaapi::Av1::new(Some(&node), w, h, 30, vaapi::QUALITY).unwrap();
        let mut d = olo_images::av1::Decoder::new(1, 1).unwrap();
        let mut shown = Vec::new();
        let mut sent = Vec::new();
        for i in 0..8u32 {
            let px: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| [(x + i * 4) as u8, y as u8, 128, 255])).collect();
            let (bits, key) = e.encode(&px, w as usize * 4, false).unwrap();
            assert_eq!(key, i == 0);
            d.decode(&bits, &mut shown).unwrap();
            sent.push(px);
        }
        d.flush(&mut shown);
        assert_eq!(shown.len(), 8, "every frame decoded");
        assert_eq!((shown[0].width, shown[0].height), (w, h));
        // Each as it was sent: its luma within a few levels (PSNR).
        for (f, px) in shown.iter().zip(&sent) {
            let rgba = f.to_rgba();
            let mut mse = 0f64;
            for p in 0..(w * h) as usize {
                let y = |r: u8, g: u8, b: u8| 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
                let a = y(px[p * 4 + 2], px[p * 4 + 1], px[p * 4]);
                let b = y(rgba[p * 4], rgba[p * 4 + 1], rgba[p * 4 + 2]);
                mse += (a - b).powi(2);
            }
            let psnr = 10.0 * (255f64.powi(2) / (mse / f64::from(w * h)).max(1e-9)).log10();
            assert!(psnr > 38.0, "luma PSNR {psnr:.1} dB");
        }
    }

    /// The same from dmabufs (as a capture's frames come), through
    /// VA-API's video processor: linear ones, and ones as the GPU lays out
    /// pictures it draws; each buffer drawn in again and encoded again
    /// (its import kept).
    #[test]
    fn vaapi_dmabuf_frames_come_back() {
        use std::os::fd::{AsFd as _, AsRawFd as _};
        let Some(node) = vaapi::av1_node() else {
            eprintln!("no GPU encodes AV1 through VA-API here");
            return;
        };
        let (w, h) = (640u32, 360u32);
        let gpu = gbm::Device::new(std::fs::OpenOptions::new().read(true).write(true).open(&node).unwrap()).unwrap();
        // Intel's tiled layouts, best first (4 with Xe2's compression, 4,
        // Y, X): the GPU takes the first it can draw.
        let tiled: Vec<gbm::Modifier> = [0x0100_0000_0000_0009u64, 0x0100_0000_0000_0002, 0x0100_0000_0000_0001].into_iter().map(gbm::Modifier::from).collect();
        for linear in [true, false] {
            let make = || match linear {
                true => gpu.create_buffer_object::<()>(w, h, gbm::Format::Xrgb8888, gbm::BufferObjectFlags::LINEAR | gbm::BufferObjectFlags::RENDERING),
                false => gpu.create_buffer_object_with_modifiers2::<()>(w, h, gbm::Format::Xrgb8888, tiled.iter().copied(), gbm::BufferObjectFlags::RENDERING),
            };
            let Ok(first) = make() else {
                eprintln!("this GPU draws none of Intel's tiled layouts");
                continue;
            };
            let mut bos = vec![first, make().unwrap()];
            let mut e = vaapi::Av1::new(Some(&node), w, h, 30, vaapi::QUALITY).unwrap();
            let mut d = olo_images::av1::Decoder::new(1, 1).unwrap();
            let (mut shown, mut sent) = (Vec::new(), Vec::new());
            for i in 0..8u32 {
                let px: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| [(x + i * 4) as u8, y as u8, 128, 255])).collect();
                let bo = &mut bos[i as usize % 2];
                bo.map_mut(0, 0, w, h, |m| {
                    let stride = m.stride() as usize;
                    for (row, from) in m.buffer_mut().chunks_mut(stride).zip(px.chunks(w as usize * 4)) {
                        row[..from.len()].copy_from_slice(from);
                    }
                })
                .unwrap();
                let fd = bo.fd_for_plane(0).unwrap();
                // SAFETY: lseek on our dmabuf: its size.
                let size = unsafe { libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_END) } as u32;
                let picture =
                    vaapi::Prime { key: u64::from(i % 2), fd: fd.as_fd(), modifier: u64::from(bo.modifier()), offset: bo.offset(0), pitch: bo.stride_for_plane(0), size, width: w, height: h };
                let (bits, key) = e.encode_prime(&picture).unwrap_or_else(|err| panic!("{} (modifier {:#x}): {err}", if linear { "linear" } else { "tiled" }, picture.modifier));
                assert_eq!(key, i == 0);
                d.decode(&bits, &mut shown).unwrap();
                sent.push(px);
            }
            d.flush(&mut shown);
            assert_eq!(shown.len(), 8, "every frame decoded");
            for (f, px) in shown.iter().zip(&sent) {
                let rgba = f.to_rgba();
                let mut mse = 0f64;
                for p in 0..(w * h) as usize {
                    let y = |r: u8, g: u8, b: u8| 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
                    let a = y(px[p * 4 + 2], px[p * 4 + 1], px[p * 4]);
                    let b = y(rgba[p * 4], rgba[p * 4 + 1], rgba[p * 4 + 2]);
                    mse += (a - b).powi(2);
                }
                let psnr = 10.0 * (255f64.powi(2) / (mse / f64::from(w * h)).max(1e-9)).log10();
                eprintln!("{} {:#x}: luma PSNR {psnr:.1} dB", if linear { "linear" } else { "tiled" }, u64::from(bos[0].modifier()));
                assert!(psnr > 38.0, "luma PSNR {psnr:.1} dB");
            }
        }
    }

    /// AV1 through VA-API (the Intel or AMD GPU), as `codec_sizes` does
    /// NVENC's: each quantizer's size, picture quality and time a frame
    /// (`OLO_BENCH_Q`: the quantizers, 0–255).
    #[test]
    #[ignore]
    fn vaapi_sizes() {
        let Some(node) = vaapi::av1_node() else {
            eprintln!("no GPU encodes AV1 through VA-API here");
            return;
        };
        let Some((w, h, frames)) = bench_frames() else { return };
        let qs: Vec<u8> = match std::env::var("OLO_BENCH_Q") {
            Ok(v) => v.split(',').filter_map(|q| q.parse().ok()).collect(),
            Err(_) => vec![40, 60, 72, 90, 110],
        };
        // The processor's H.264 on the same frames, to measure against.
        {
            let mut e = H264::new(w, h, 30).unwrap();
            let file = std::path::PathBuf::from(format!("/var/tmp/olo-bench-{}.mp4", std::process::id()));
            let mut mp4 = Mp4::create(&file, w, h, 1000).unwrap();
            for f in &frames {
                let (bits, key) = e.encode(f, w as usize * 4, Order::Bgra).unwrap();
                mp4.add(&bits, key, 33).unwrap();
            }
            mp4.finish().unwrap();
            let bytes = std::fs::metadata(&file).unwrap().len();
            let (psnr, n) = mp4_psnr(&file, w, h, &frames);
            eprintln!("H.264 (processor)    {bytes:9} bytes  PSNR {psnr:5.2} dB  ({n} decoded)");
            let _ = std::fs::remove_file(&file);
        }
        for q in qs {
            let mut e = vaapi::Av1::new(Some(&node), w, h, 30, q).unwrap();
            let file = std::path::PathBuf::from(format!("/var/tmp/olo-bench-{}.mp4", std::process::id()));
            let mut mp4 = Mp4::create_for(&file, w, h, 1000, Codec::Av1).unwrap();
            let mut times = Vec::new();
            for f in &frames {
                let t = std::time::Instant::now();
                let (bits, key) = e.encode(f, w as usize * 4, false).unwrap();
                times.push(t.elapsed());
                mp4.add(&bits, key, 33).unwrap();
            }
            mp4.finish().unwrap();
            times.sort();
            let bytes = std::fs::metadata(&file).unwrap().len();
            let (psnr, n) = mp4_psnr(&file, w, h, &frames);
            eprintln!("AV1 (VA-API) q {q:3} {bytes:9} bytes  PSNR {psnr:5.2} dB  ({n} decoded)  median {:?}, slowest {:?} a frame", times[times.len() / 2], times[times.len() - 1]);
            let _ = std::fs::remove_file(&file);
        }
    }

    /// On battery: a battery and nothing that powers the computer online
    /// (a mouse's battery doesn't count).
    #[test]
    fn battery_or_mains() {
        let base = std::path::PathBuf::from(format!("/var/tmp/olo-supplies-{}", std::process::id()));
        let supply = |name: &str, kind: &str, online: Option<&str>, scope: Option<&str>| {
            let d = base.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("type"), kind).unwrap();
            if let Some(o) = online {
                std::fs::write(d.join("online"), o).unwrap();
            }
            if let Some(s) = scope {
                std::fs::write(d.join("scope"), s).unwrap();
            }
        };
        supply("BAT0", "Battery\n", None, None);
        supply("AC", "Mains\n", Some("1\n"), None);
        assert!(!on_battery_in(&base));
        supply("AC", "Mains\n", Some("0\n"), None);
        supply("ucsi-source-psy-USBC000:001", "USB\n", Some("0\n"), None);
        assert!(on_battery_in(&base));
        // Charging over USB-C.
        supply("ucsi-source-psy-USBC000:001", "USB\n", Some("1\n"), None);
        assert!(!on_battery_in(&base));
        // A desktop: a mouse's battery only.
        let _ = std::fs::remove_dir_all(&base);
        supply("hidpp_battery_0", "Battery\n", None, Some("Device\n"));
        assert!(!on_battery_in(&base));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// NVIDIA's encoder, where there is one: its H.264 decodes, an MP4 of
    /// it reads back, and what a 4K frame costs against the processor's
    /// (`--ignored`: it needs the GPU).
    #[test]
    #[ignore]
    fn nvenc_makes_h264_that_decodes() {
        if !nvenc::available() {
            eprintln!("no NVIDIA encoder here: nothing to try");
            return;
        }
        for (w, h) in [(640u32, 360u32), (3840, 2160)] {
            let mut e = Encoder::new(w, h, 30, Choice::Nvidia, Codec::H264).unwrap();
            assert_eq!(e.name(), "NVIDIA's GPU (NVENC)");
            let mut dec = openh264::decoder::Decoder::new().unwrap();
            let mut px: Vec<u8> = (0..(w * h))
                .flat_map(|p| {
                    let (x, y) = (p % w, p / w);
                    let ink = if (y / 24) % 2 == 0 && (x / 7) % 3 != 0 { 40 } else { 235 };
                    [ink, ink, ink, 255]
                })
                .collect();
            let (mut times, mut decoded, mut keys) = (Vec::new(), 0, 0);
            for i in 0..30u32 {
                let y0 = (i * 8) % (h - 16);
                for y in y0..y0 + 16 {
                    for x in 10..300 {
                        px[((y * w + x) * 4) as usize] = (i * 8) as u8;
                    }
                }
                let t = std::time::Instant::now();
                let (bits, key) = e.encode(&px, w as usize * 4, Order::Bgra).unwrap();
                times.push(t.elapsed());
                keys += usize::from(key);
                if dec.decode(&bits).unwrap().is_some() {
                    decoded += 1;
                }
            }
            times.sort();
            eprintln!("NVENC {w}×{h}: median {:?}, max {:?} a frame; {decoded} of 30 decoded, {keys} key", times[15], times[29]);
            assert!(decoded >= 29 && keys >= 1);
        }
    }
}
