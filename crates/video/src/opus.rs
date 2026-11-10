//! Ogg Opus written as it comes: sound (a voice message's, mono, 48 kHz)
//! coded with rusty-opus (a pure-Rust Opus, RFC 6716) and put in Ogg
//! pages (RFC 7845) as they fill, a page a second, so a recording of any
//! length holds only its last second. What Telegram takes as a voice
//! message (`audio/ogg`), and what any player plays.
//!
//! Speech only, and SILK only: wideband (8 kHz of sound, as voice
//! messages are), VoIP, 20 ms frames. Measured against libopus 1.6 (the
//! system's, in a scratch program, never here), decoding rusty-opus's
//! packets the same as `opus-decoder` does: SILK matches libopus's own
//! encoder (5.6 and 8.6 dB on the two voices in olo-voice's tests, 17 on
//! a chirp, at 24, 32 and 64 kb/s alike), but left to choose, rusty-opus
//! went to CELT or hybrid at 64 kb/s, or on a tone at 32, and came out
//! at −1 to −3 dB where libopus has 6 to 17: distorted. Kept to
//! wideband, it never does. More than 24 kb/s buys SILK nothing.

use std::io::{self, Write};

/// Samples a second, in and out (Opus always counts at 48 kHz).
pub const RATE: u32 = 48_000;
/// A frame: 20 ms.
pub const FRAME: usize = 960;
/// The samples a decoder drops at the start: the encoder's delay. Not
/// libopus's 312 (6.5 ms): rusty-opus's speech comes 328 late, measured
/// (a voice message's, decoded, lined up with what was said, at every
/// bit rate; the test checks it), so with 312 every message began a
/// third of a millisecond late.
pub const PRE_SKIP: u16 = 328;
/// Frames in a page: a second's.
const PAGE_FRAMES: usize = 50;

/// Ogg's CRC-32 (polynomial 0x04C11DB7, not reflected, from 0).
fn crc(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
            }
            *e = r;
        }
        t
    });
    data.iter().fold(0u32, |c, &b| (c << 8) ^ t[((c >> 24) as u8 ^ b) as usize])
}

/// An Ogg Opus stream being written to `out`.
pub struct OggOpus<W: Write> {
    out: W,
    enc: rusty_opus::OpusEncoder,
    serial: u32,
    seq: u32,
    /// Samples short of a whole frame.
    pending: Vec<f32>,
    /// The page being filled: its packets' bytes and lacing.
    data: Vec<u8>,
    lacing: Vec<u8>,
    frames_in_page: usize,
    /// Frames coded so far.
    frames: u64,
    /// Samples given.
    samples: u64,
    packet: Vec<u8>,
}

impl<W: Write> OggOpus<W> {
    /// Start a mono stream at `bitrate` (bits a second; 24 000 for a
    /// voice), its two header pages written at once.
    pub fn new(mut out: W, bitrate: i32) -> io::Result<OggOpus<W>> {
        let mut enc = rusty_opus::OpusEncoder::new(RATE as i32, 1, rusty_opus::Application::Voip).map_err(|e| io::Error::other(format!("Opus: {e:?}")))?;
        enc.bitrate_bps = bitrate;
        // SILK only (above).
        enc.max_bandwidth = rusty_opus::Bandwidth::Wideband;
        enc.signal_type = Some(rusty_opus::SignalType::Voice);
        let mut serial = [0u8; 4];
        let _ = getrandom::fill(&mut serial);
        let serial = u32::from_le_bytes(serial);
        // OpusHead: version 1, one channel, the pre-skip, the rate it was
        // made at, no gain, mapping family 0.
        let mut head = b"OpusHead".to_vec();
        head.push(1);
        head.push(1);
        head.extend_from_slice(&PRE_SKIP.to_le_bytes());
        head.extend_from_slice(&RATE.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        page(&mut out, serial, 0, 0x02, 0, &head, &lacing(head.len()))?;
        let vendor = b"Olo";
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes());
        page(&mut out, serial, 1, 0, 0, &tags, &lacing(tags.len()))?;
        Ok(OggOpus { out, enc, serial, seq: 2, pending: Vec::with_capacity(FRAME), data: Vec::new(), lacing: Vec::new(), frames_in_page: 0, frames: 0, samples: 0, packet: vec![0; 1500] })
    }

    /// Sound to code (mono, 48 kHz, -1..1), any amount.
    pub fn push(&mut self, mut pcm: &[f32]) -> io::Result<()> {
        self.samples += pcm.len() as u64;
        while !pcm.is_empty() {
            let take = (FRAME - self.pending.len()).min(pcm.len());
            self.pending.extend_from_slice(&pcm[..take]);
            pcm = &pcm[take..];
            if self.pending.len() == FRAME {
                self.frame()?;
            }
        }
        Ok(())
    }

    /// The seconds given so far.
    pub fn seconds(&self) -> f64 {
        self.samples as f64 / f64::from(RATE)
    }

    fn frame(&mut self) -> io::Result<()> {
        let n = self.enc.encode(&self.pending, FRAME, &mut self.packet).map_err(|e| io::Error::other(format!("Opus: {e:?}")))?;
        self.pending.clear();
        self.frames += 1;
        self.data.extend_from_slice(&self.packet[..n]);
        self.lacing.extend(lacing(n));
        self.frames_in_page += 1;
        // A page a second, or sooner if its lacing would overflow (255
        // entries; a packet takes one for each 255 bytes, and one more).
        if self.frames_in_page >= PAGE_FRAMES || self.lacing.len() > 255 - 8 {
            self.flush(0, self.frames * FRAME as u64 + u64::from(PRE_SKIP))?;
        }
        Ok(())
    }

    fn flush(&mut self, flags: u8, granule: u64) -> io::Result<()> {
        page(&mut self.out, self.serial, self.seq, flags, granule, &self.data, &self.lacing)?;
        self.seq += 1;
        self.data.clear();
        self.lacing.clear();
        self.frames_in_page = 0;
        Ok(())
    }

    /// End it: what's given is all coded (silence after it, for the
    /// encoder's lookahead, trimmed off by the last page's position), the
    /// last page written. The writer back, and the seconds.
    pub fn finish(mut self) -> io::Result<(W, f64)> {
        let want = self.samples + u64::from(PRE_SKIP);
        while self.frames * (FRAME as u64) < want {
            self.pending.resize(FRAME, 0.0);
            self.frame()?;
        }
        // The end, trimmed to what was given.
        self.flush(0x04, self.samples + u64::from(PRE_SKIP))?;
        self.out.flush()?;
        let seconds = self.seconds();
        Ok((self.out, seconds))
    }
}

/// The lacing for a packet of `n` bytes: 255s, then what's left (0 if
/// none: a packet a multiple of 255 long ends with a 0).
fn lacing(n: usize) -> Vec<u8> {
    let mut l = vec![255u8; n / 255];
    l.push((n % 255) as u8);
    l
}

fn page(out: &mut impl Write, serial: u32, seq: u32, flags: u8, granule: u64, data: &[u8], lacing: &[u8]) -> io::Result<()> {
    let mut p = Vec::with_capacity(27 + lacing.len() + data.len());
    p.extend_from_slice(b"OggS");
    p.push(0);
    p.push(flags);
    p.extend_from_slice(&granule.to_le_bytes());
    p.extend_from_slice(&serial.to_le_bytes());
    p.extend_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(&[0; 4]);
    p.push(lacing.len() as u8);
    p.extend_from_slice(lacing);
    p.extend_from_slice(data);
    let c = crc(&p);
    p[22..26].copy_from_slice(&c.to_le_bytes());
    out.write_all(&p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each page of `file`: its flags, position and packets, its CRC
    /// checked.
    fn pages(file: &[u8]) -> Vec<(u8, u64, Vec<Vec<u8>>)> {
        let mut at = 0;
        let mut out = Vec::new();
        let mut partial: Vec<u8> = Vec::new();
        while at < file.len() {
            assert_eq!(&file[at..at + 4], b"OggS");
            let flags = file[at + 5];
            let granule = u64::from_le_bytes(file[at + 6..at + 14].try_into().unwrap());
            let n = file[at + 26] as usize;
            let lacing = &file[at + 27..at + 27 + n];
            let len: usize = lacing.iter().map(|&l| l as usize).sum();
            let end = at + 27 + n + len;
            let mut copy = file[at..end].to_vec();
            copy[22..26].fill(0);
            assert_eq!(crc(&copy), u32::from_le_bytes(file[at + 22..at + 26].try_into().unwrap()), "page CRC");
            let mut packets = Vec::new();
            let mut d = at + 27 + n;
            for &l in lacing {
                partial.extend_from_slice(&file[d..d + l as usize]);
                d += l as usize;
                if l < 255 {
                    packets.push(std::mem::take(&mut partial));
                }
            }
            out.push((flags, granule, packets));
            at = end;
        }
        out
    }

    #[test]
    fn crc_is_oggs() {
        // CRC-32/POSIX's parameters without its final inversion: its
        // catalogued check value (0x765E7680 for "123456789") inverted.
        assert_eq!(crc(b""), 0);
        assert_eq!(crc(b"123456789"), !0x765E_7680);
    }

    #[test]
    fn a_voice_written_and_decoded_again() {
        // 2.5 s: a rising tone that comes and goes, as speech does (no
        // two stretches alike, so it lines up one way only).
        let n = RATE as usize * 5 / 2;
        let pcm: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let env = (t * 3.0 * std::f32::consts::PI).sin().abs();
                0.4 * env * (2.0 * std::f32::consts::PI * (200.0 * t + 160.0 * t * t)).sin()
            })
            .collect();
        let mut w = OggOpus::new(Vec::new(), 24_000).unwrap();
        for piece in pcm.chunks(1234) {
            w.push(piece).unwrap();
        }
        let (file, secs) = w.finish().unwrap();
        assert!((secs - 2.5).abs() < 1e-9);
        let pages = pages(&file);
        // Headers, then a page a second, the last ending the stream.
        assert_eq!(pages[0].0, 0x02);
        assert_eq!(&pages[0].2[0][..8], b"OpusHead");
        assert_eq!(u16::from_le_bytes([pages[0].2[0][10], pages[0].2[0][11]]), PRE_SKIP);
        assert_eq!(&pages[1].2[0][..8], b"OpusTags");
        let audio = &pages[2..];
        assert_eq!(audio.len(), 3);
        assert_eq!(audio.last().unwrap().0, 0x04);
        assert_eq!(audio.last().unwrap().1, n as u64 + u64::from(PRE_SKIP));
        assert!(audio.windows(2).all(|w| w[0].1 < w[1].1));
        // Every packet decodes, to about what went in.
        let mut dec = opus_decoder::OpusDecoder::new(RATE, 1).unwrap();
        let mut out = Vec::new();
        let mut buf = vec![0f32; 5760];
        for (_, _, packets) in audio {
            for p in packets {
                let k = dec.decode_float(p, &mut buf, false).unwrap();
                out.extend_from_slice(&buf[..k]);
            }
        }
        assert!(out.len() >= n + PRE_SKIP as usize);
        let got = &out[PRE_SKIP as usize..PRE_SKIP as usize + n];
        // The pre-skip is the encoder's delay: what comes out lines up with
        // what went in (speech to the sample; a tone, as here, up to 10
        // later, SILK's phase on it).
        let lag = (-40i64..=40)
            .max_by(|&a, &b| {
                let corr = |lag: i64| (2000..n - 2000).map(|i| f64::from(pcm[i]) * f64::from(got[(i as i64 + lag) as usize])).sum::<f64>();
                corr(a).total_cmp(&corr(b))
            })
            .unwrap();
        assert!((-1..=10).contains(&lag), "decoded {lag} samples off");
        let energy = |x: &[f32]| x.iter().map(|s| s * s).sum::<f32>();
        let ratio = energy(got) / energy(&pcm);
        assert!((0.5..2.0).contains(&ratio), "energy ratio {ratio}");
        // Bytes: about the bit rate asked (VBR, so roughly).
        let kbps = file.len() as f64 * 8.0 / 2.5 / 1000.0;
        assert!((10.0..60.0).contains(&kbps), "{kbps} kb/s");
    }
}
