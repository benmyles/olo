//! Deep (10-bit) and tiled video decoded on every device here, each
//! picture checked against the reference decoder's (libvpx's and
//! libaom's MD5s, `tests/data/README.md`): the processor's decoders
//! (rav1d, filmcraft-vp9), Intel's GPU through VA-API and NVIDIA's
//! through NVDEC, each as the Viewer uses it. A device that isn't here,
//! or doesn't decode a format, is said and passed over; the processor is
//! always checked.

use olo_images::{av1, hevc, vp9};

/// An IVF file's frames.
fn ivf(data: &[u8]) -> Vec<&[u8]> {
    assert_eq!(&data[..4], b"DKIF");
    let header = u16::from_le_bytes([data[6], data[7]]) as usize;
    let mut at = header;
    let mut frames = Vec::new();
    while at + 12 <= data.len() {
        let size = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        at += 12;
        frames.push(&data[at..at + size]);
        at += size;
    }
    frames
}

/// An Annex B stream's NAL units, each with its start code.
fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    // (Each at its start code, three bytes or four.)
    let starts: Vec<usize> = (0..data.len().saturating_sub(2)).filter(|&i| data[i..i + 3] == [0, 0, 1]).map(|i| if i > 0 && data[i - 1] == 0 { i - 1 } else { i }).collect();
    starts.iter().enumerate().map(|(k, &a)| &data[a..starts.get(k + 1).copied().unwrap_or(data.len())]).collect()
}

/// A NAL unit without its start code.
fn bare(nal: &[u8]) -> &[u8] {
    let n = nal.iter().position(|&b| b == 1).map_or(0, |i| i + 1);
    &nal[n..]
}

/// A picture as every decoder here hands it to the window's GPU: luma
/// rows, then Cb and Cr interleaved; a deep one's samples 16-bit
/// little-endian in the high bits.
struct Planes {
    width: usize,
    height: usize,
    deep: bool,
    y: Vec<u8>,
    uv: Vec<u8>,
}

impl Planes {
    /// Its MD5 as the reference decoders take it: I420, each plane
    /// packed; a deep picture's `bits`-bit samples as 16-bit
    /// little-endian numbers.
    fn md5(&self, bits: u32) -> String {
        let (w, h) = (self.width, self.height);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut c = md5::Context::new();
        if self.deep {
            let shift = 16 - bits;
            let samples = |b: &[u8]| -> Vec<u16> { b.as_chunks::<2>().0.iter().map(|s| u16::from_le_bytes(*s) >> shift).collect() };
            let (y, uv) = (samples(&self.y), samples(&self.uv));
            let bytes = |s: &mut dyn Iterator<Item = u16>| -> Vec<u8> { s.flat_map(u16::to_le_bytes).collect() };
            c.consume(bytes(&mut y[..w * h].iter().copied()));
            c.consume(bytes(&mut uv[..cw * ch * 2].iter().step_by(2).copied()));
            c.consume(bytes(&mut uv[1..cw * ch * 2].iter().step_by(2).copied()));
        } else {
            c.consume(&self.y[..w * h]);
            c.consume(self.uv[..cw * ch * 2].iter().step_by(2).copied().collect::<Vec<u8>>());
            c.consume(self.uv[1..cw * ch * 2].iter().step_by(2).copied().collect::<Vec<u8>>());
        }
        format!("{:x}", c.finalize())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Codec {
    Vp9,
    Av1,
    /// Annex B: `frames` are its NAL units.
    Hevc,
}

#[derive(Clone, Copy, Debug)]
enum Device {
    Processor,
    Intel,
    Nvidia,
}

/// Every picture `frames` show, decoded on `device`; Err (why) when it
/// isn't here or doesn't decode them.
fn decoded(device: Device, codec: Codec, frames: &[&[u8]]) -> Result<Vec<Planes>, String> {
    let from_frame = |f: &av1::Frame| -> Result<Planes, String> {
        let p = f.yuv420().ok_or("not 4:2:0 planes")?;
        Ok(Planes { width: p.width as usize, height: p.height as usize, deep: p.deep, y: p.y, uv: p.uv })
    };
    match device {
        Device::Processor => {
            let mut out = Vec::new();
            match codec {
                Codec::Av1 => {
                    let mut d = av1::Decoder::new(1, 1)?;
                    let mut pics = Vec::new();
                    for f in frames {
                        d.decode(f, &mut pics)?;
                    }
                    d.flush(&mut pics);
                    for p in &pics {
                        out.push(from_frame(p)?);
                    }
                }
                // (As MP4 carries it: the parameter sets its configuration,
                // each NAL unit a length-prefixed sample.)
                Codec::Hevc => {
                    let sets = frames.iter().map(|n| bare(n)).filter(|n| matches!((n[0] >> 1) & 0x3f, 32..=34)).map(<[u8]>::to_vec).collect();
                    let mut d = hevc::Decoder::new(hevc::Config { nal_len: 4, sets }, hevc::Colour::default(), 1);
                    let mut pics = Vec::new();
                    for n in frames {
                        let n = bare(n);
                        let mut sample = (n.len() as u32).to_be_bytes().to_vec();
                        sample.extend_from_slice(n);
                        d.decode(&sample, &mut pics)?;
                    }
                    d.flush(&mut pics);
                    for p in &pics {
                        out.push(from_frame(p)?);
                    }
                }
                Codec::Vp9 => {
                    let mut d = vp9::Decoder::new(1);
                    let mut pics = Vec::new();
                    for (i, f) in frames.iter().enumerate() {
                        d.decode(f, i as i64, &mut pics)?;
                    }
                    d.flush(&mut pics);
                    for (_, p) in &pics {
                        out.push(from_frame(p)?);
                    }
                }
            }
            Ok(out)
        }
        Device::Intel => {
            let c = match codec {
                Codec::Av1 => olo_vaapi::decode::Codec::Av1,
                Codec::Vp9 => olo_vaapi::decode::Codec::Vp9,
                Codec::Hevc => olo_vaapi::decode::Codec::Hevc,
            };
            if !olo_vaapi::decode::available(c) {
                return Err("no GPU here decodes it through VA-API".into());
            }
            let mut d = olo_vaapi::decode::Decoder::new(c)?;
            let mut pics = Vec::new();
            for (i, f) in frames.iter().enumerate() {
                d.decode(f, i as i64, &mut pics)?;
            }
            d.flush(&mut pics)?;
            Ok(pics.into_iter().map(|p| Planes { width: p.width as usize, height: p.height as usize, deep: p.deep, y: p.y, uv: p.uv }).collect())
        }
        Device::Nvidia => {
            let c = match codec {
                Codec::Av1 => olo_video::nvdec::Codec::Av1,
                Codec::Vp9 => olo_video::nvdec::Codec::Vp9,
                Codec::Hevc => olo_video::nvdec::Codec::Hevc,
            };
            // (As the Viewer has them: left where they were copied.)
            let mut d = olo_video::nvdec::Nvdec::new(c)?;
            d.hold(true);
            let mut pics = Vec::new();
            for (i, f) in frames.iter().enumerate() {
                d.decode(f, i as i64, &mut pics)?;
            }
            d.flush(&mut pics);
            Ok(pics
                .into_iter()
                .map(|p| {
                    let (y, uv) = match &p.held {
                        Some(h) => {
                            let bytes = if p.deep { 2 } else { 1 };
                            let (row_y, row_uv) = (p.width as usize * bytes, p.width.div_ceil(2) as usize * 2 * bytes);
                            let rows = |(at, pitch): (usize, usize), row: usize, n: usize| -> Vec<u8> { (0..n).flat_map(|r| h.bytes()[at + r * pitch..][..row].to_vec()).collect() };
                            (rows(h.y, row_y, p.height as usize), rows(h.uv, row_uv, p.height.div_ceil(2) as usize))
                        }
                        None => (p.y, p.uv),
                    };
                    Planes { width: p.width as usize, height: p.height as usize, deep: p.deep, y, uv }
                })
                .collect())
        }
    }
}

/// `name` decoded on every device here, each picture as the reference
/// decoder's (`bits` deep).
fn check(codec: Codec, name: &str, bits: u32) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let data = std::fs::read(dir.join(name)).unwrap();
    let sums = std::fs::read_to_string(dir.join(format!("{name}.md5"))).unwrap();
    let want: Vec<&str> = sums.lines().filter_map(|l| l.split_whitespace().next()).collect();
    let frames = if codec == Codec::Hevc { nal_units(&data) } else { ivf(&data) };
    for device in [Device::Processor, Device::Intel, Device::Nvidia] {
        let pics = match decoded(device, codec, &frames) {
            Ok(p) => p,
            Err(e) if !matches!(device, Device::Processor) => {
                eprintln!("{name} on {device:?}: not checked ({e})");
                continue;
            }
            Err(e) => panic!("{name} on the processor: {e}"),
        };
        assert_eq!(pics.len(), want.len(), "{name} on {device:?}: pictures shown");
        for (k, (p, w)) in pics.iter().zip(&want).enumerate() {
            assert_eq!((p.deep, p.md5(bits).as_str()), (bits > 8, *w), "{name} on {device:?}: picture {k} ({}×{})", p.width, p.height);
        }
        eprintln!("{name} on {device:?}: {} pictures as the reference decoder made them", pics.len());
    }
}

#[test]
fn vp9_10_bit_is_the_reference_decoder_s() {
    check(Codec::Vp9, "vp9-10bit.ivf", 10);
}

#[test]
fn vp9_in_tiles_is_the_reference_decoder_s() {
    check(Codec::Vp9, "vp9-tiles.ivf", 8);
}

#[test]
fn av1_10_bit_is_the_reference_decoder_s() {
    check(Codec::Av1, "av1-10bit.ivf", 10);
}

/// (H.264 and HEVC on Intel's GPU need RPM Fusion's driver, which Olo
/// Doctor installs; `LIBVA_DRIVERS_PATH` can point the test at one
/// unpacked elsewhere.)
#[test]
fn hevc_main_10_is_the_reference_decoder_s() {
    check(Codec::Hevc, "hevc-10bit.h265", 10);
}
