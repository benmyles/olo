//! `olo-eld`: an AAC-ELD decoder, in Rust, for the streams AirPods send
//! from their own microphone (mono, 480 samples a frame, no SBR: the
//! AudioSpecificConfig `f8 e6 30 00`). ISO/IEC 14496-3's Enhanced Low
//! Delay AAC is AAC's spectral coding (Huffman codebooks, scale factors,
//! noise substitution, TNS) in the error-resilient syntax, with one long
//! window and a low-delay filter bank: an inverse MDCT and a 4-frame
//! overlap through the standard's synthesis window.
//!
//! The Huffman codebooks are Symphonia's (MPL-2.0, `codebooks.rs`); the
//! standard's other numbers are in `tables.rs`; the rest is Olo's. Tested
//! against libfdk-aac's decode of a stream its encoder made (`tests/`).
//!
//! ```
//! let mut d = olo_eld::Decoder::new(&[0xF8, 0xE6, 0x30, 0x00]).unwrap();
//! let mut pcm = [0f32; olo_eld::FRAME];
//! # let frame: &[u8] = &[];
//! # if !frame.is_empty() {
//! d.decode(frame, &mut pcm).unwrap();
//! # }
//! ```

mod bits;
#[allow(dead_code, clippy::unreadable_literal)]
mod codebooks;
mod huffman;
mod imdct;
#[allow(clippy::excessive_precision)]
mod tables;

use bits::Bits;

/// Samples a frame.
pub const FRAME: usize = 480;

const ZERO_BT: u8 = 0;
const NOISE_BT: u8 = 13;
const INTENSITY_BT2: u8 = 14;
const INTENSITY_BT: u8 = 15;
const MAX_SFB: usize = 40;
const TNS_MAX_ORDER: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// An AudioSpecificConfig this decoder doesn't take (not ELD, not
    /// mono, not 480 samples a frame, or with SBR).
    Config(&'static str),
    /// A frame that doesn't read as one.
    Frame(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Config(e) => write!(f, "AAC-ELD config: {e}"),
            Error::Frame(e) => write!(f, "AAC-ELD frame: {e}"),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// What the AudioSpecificConfig says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The sampling frequency index (3: 48 kHz); the bands follow it.
    pub frequency_index: u8,
}

/// Read an AudioSpecificConfig: ELD (object type 39), one channel, 480
/// samples a frame, no SBR, none of the error-resilience tools.
pub fn config(asc: &[u8]) -> Result<Config> {
    let mut b = Bits::new(asc);
    let mut aot = b.read(5).ok_or(Error::Config("short"))?;
    if aot == 31 {
        aot = 32 + b.read(6).ok_or(Error::Config("short"))?;
    }
    if aot != 39 {
        return Err(Error::Config("not ELD"));
    }
    let frequency_index = b.read(4).ok_or(Error::Config("short"))? as u8;
    if frequency_index == 15 {
        b.read(24).ok_or(Error::Config("short"))?;
    }
    if b.read(4) != Some(1) {
        return Err(Error::Config("not one channel"));
    }
    // ELDSpecificConfig.
    if b.read(1) != Some(1) {
        return Err(Error::Config("not 480 samples a frame"));
    }
    if b.read(3) != Some(0) {
        return Err(Error::Config("error resilience tools"));
    }
    if b.read(1) != Some(0) {
        return Err(Error::Config("low-delay SBR"));
    }
    swb_offsets(frequency_index).ok_or(Error::Config("sampling rate"))?;
    Ok(Config { frequency_index })
}

fn swb_offsets(frequency_index: u8) -> Option<&'static [u16]> {
    Some(match frequency_index {
        // 96 to 64 kHz use 48 kHz's bands (AirPods' 64 kHz stream says 48).
        0..=4 => &tables::SWB_480_48,
        5 => &tables::SWB_480_32,
        6 | 7 => &tables::SWB_480_24,
        _ => return None,
    })
}

/// One frame's channel, read.
struct Channel {
    band_type: [u8; MAX_SFB],
    /// Scale factors, noise energies (by band).
    sf: [i32; MAX_SFB],
    tns: Option<Tns>,
}

struct Tns {
    filters: Vec<(usize, usize, bool, Vec<f32>)>,
}

/// A decoder: keeps the last three frames' transform output for the
/// overlap, and the noise generator's state.
pub struct Decoder {
    swb: &'static [u16],
    num_swb: usize,
    tns_max_bands: usize,
    imdct: imdct::Imdct,
    coef: [f32; FRAME],
    buf: [f32; FRAME],
    saved: [f32; 3 * FRAME],
    seed: u32,
}

impl Decoder {
    pub fn new(asc: &[u8]) -> Result<Decoder> {
        let c = config(asc)?;
        let swb = swb_offsets(c.frequency_index).ok_or(Error::Config("sampling rate"))?;
        Ok(Decoder {
            swb,
            num_swb: swb.len() - 1,
            tns_max_bands: tables::TNS_MAX_BANDS_480[(c.frequency_index as usize).min(12)].max(1) as usize,
            imdct: imdct::Imdct::new(FRAME),
            coef: [0.0; FRAME],
            buf: [0.0; FRAME],
            saved: [0.0; 3 * FRAME],
            seed: 0x1F2E_3D4C,
        })
    }

    /// Decode one access unit into `out` (FRAME samples, ±1.0 full scale).
    /// A frame that doesn't read is an error, and `out` is silence; the
    /// overlap goes on.
    pub fn decode(&mut self, frame: &[u8], out: &mut [f32; FRAME]) -> Result<()> {
        let r = self.spectrum(frame);
        if r.is_err() {
            self.coef = [0.0; FRAME];
        }
        self.synthesize(out);
        r
    }

    /// Read the frame into `self.coef` (dequantized, TNS applied).
    fn spectrum(&mut self, frame: &[u8]) -> Result<()> {
        let mut b = Bits::new(frame);
        let short = Error::Frame("short");
        let global_gain = b.read(8).ok_or(short.clone())? as i32;
        let max_sfb = b.read(6).ok_or(short.clone())? as usize;
        if max_sfb > self.num_swb {
            return Err(Error::Frame("too many bands"));
        }
        let mut ch = Channel { band_type: [0; MAX_SFB], sf: [0; MAX_SFB], tns: None };
        // Sections: a codebook, and how many bands use it.
        let mut k = 0;
        while k < max_sfb {
            let cb = b.read(4).ok_or(short.clone())? as u8;
            if cb == 12 {
                return Err(Error::Frame("reserved codebook"));
            }
            let mut end = k;
            loop {
                let inc = b.read(5).ok_or(short.clone())? as usize;
                end += inc;
                if inc != 31 {
                    break;
                }
            }
            if end > max_sfb {
                return Err(Error::Frame("sections past the bands"));
            }
            for t in &mut ch.band_type[k..end] {
                *t = cb;
            }
            k = end;
        }
        // Scale factors, each a difference from the last of its kind.
        let (mut scale, mut noise, mut intensity, mut first_noise) = (global_gain, global_gain - 90, 0i32, true);
        for sfb in 0..max_sfb {
            match ch.band_type[sfb] {
                ZERO_BT => {}
                INTENSITY_BT | INTENSITY_BT2 => {
                    intensity += huffman::scale_factor(&mut b).ok_or(short.clone())? - 60;
                    ch.sf[sfb] = intensity;
                }
                NOISE_BT => {
                    if first_noise {
                        noise += b.read(9).ok_or(short.clone())? as i32 - 256;
                        first_noise = false;
                    } else {
                        noise += huffman::scale_factor(&mut b).ok_or(short.clone())? - 60;
                    }
                    ch.sf[sfb] = noise.clamp(-100, 155);
                }
                _ => {
                    scale += huffman::scale_factor(&mut b).ok_or(short.clone())? - 60;
                    if !(0..=255).contains(&scale) {
                        return Err(Error::Frame("scale factor out of range"));
                    }
                    ch.sf[sfb] = scale;
                }
            }
        }
        // ELD has no pulse data and no gain control: TNS, then the lines.
        if b.read(1).ok_or(short.clone())? == 1 {
            ch.tns = Some(read_tns(&mut b)?);
        }
        self.coef = [0.0; FRAME];
        for sfb in 0..max_sfb {
            let (lo, hi) = (self.swb[sfb] as usize, self.swb[sfb + 1] as usize);
            let cb = ch.band_type[sfb];
            match cb {
                ZERO_BT | INTENSITY_BT | INTENSITY_BT2 => {}
                NOISE_BT => {
                    let mut energy = 0.0f32;
                    for c in &mut self.coef[lo..hi] {
                        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        *c = self.seed as i32 as f32;
                        energy += *c * *c;
                    }
                    let gain = 2f32.powf(0.25 * ch.sf[sfb] as f32) / energy.sqrt().max(1e-9);
                    for c in &mut self.coef[lo..hi] {
                        *c *= gain;
                    }
                }
                _ => {
                    huffman::spectrum(&mut b, cb, &mut self.coef[lo..hi]).ok_or(Error::Frame("spectral data"))?;
                    let gain = 2f32.powf(0.25 * (ch.sf[sfb] - 100) as f32);
                    for c in &mut self.coef[lo..hi] {
                        *c = c.signum() * c.abs().powf(4.0 / 3.0) * gain;
                    }
                }
            }
        }
        if let Some(tns) = &ch.tns {
            self.apply_tns(tns, max_sfb);
        }
        Ok(())
    }

    /// TNS: each filter, from the top band down, runs along its bands'
    /// lines (upwards, or downwards for direction 1).
    fn apply_tns(&mut self, tns: &Tns, max_sfb: usize) {
        let mmm = self.tns_max_bands.min(max_sfb);
        let mut bottom = self.num_swb;
        for (length, order, down, coefs) in &tns.filters {
            let top = bottom;
            bottom = top.saturating_sub(*length);
            if *order == 0 {
                continue;
            }
            let lpc = lpc(coefs);
            let start = self.swb[bottom.min(mmm)] as usize;
            let end = self.swb[top.min(mmm)] as usize;
            if end <= start {
                continue;
            }
            let size = end - start;
            let at = |m: usize| if *down { end - 1 - m } else { start + m };
            for m in 0..size {
                let mut v = self.coef[at(m)];
                for i in 1..=m.min(*order) {
                    v -= self.coef[at(m - i)] * lpc[i - 1];
                }
                self.coef[at(m)] = v;
            }
        }
    }

    /// The low-delay synthesis: an inverse transform, then this frame's
    /// half and the last three frames' through the window.
    fn synthesize(&mut self, out: &mut [f32; FRAME]) {
        const N: usize = FRAME;
        const N2: usize = N / 2;
        const N4: usize = N / 4;
        // To full scale (16-bit samples are ±32768).
        let scale = 1.0 / (N as f32 * 32768.0);
        let mut x = self.coef;
        for i in (0..N2).step_by(2) {
            let t = x[i];
            x[i] = -x[N - 1 - i];
            x[N - 1 - i] = t;
            let t = -x[i + 1];
            x[i + 1] = x[N - 2 - i];
            x[N - 2 - i] = t;
        }
        self.imdct.half(&x, &mut self.buf, scale);
        for i in (0..N).step_by(2) {
            self.buf[i] = -self.buf[i];
        }
        let (w, buf, s) = (&tables::WINDOW_480, &self.buf, &self.saved);
        for i in N4..N2 {
            out[i - N4] = buf[N2 - 1 - i] * w[i - N4] + s[i + N2] * w[i + N - N4] - s[N + N2 - 1 - i] * w[i + 2 * N - N4] - s[2 * N + N2 + i] * w[i + 3 * N - N4];
        }
        for i in 0..N2 {
            out[N4 + i] = buf[i] * w[i + N2 - N4] - s[N - 1 - i] * w[i + N2 + N - N4] - s[N + i] * w[i + N2 + 2 * N - N4] + s[2 * N + N - 1 - i] * w[i + N2 + 3 * N - N4];
        }
        for i in 0..N4 {
            out[N2 + N4 + i] = buf[i + N2] * w[i + N - N4] - s[N2 - 1 - i] * w[i + 2 * N - N4] - s[N + N2 + i] * w[i + 3 * N - N4];
        }
        self.saved.copy_within(0..2 * N, N);
        self.saved[..N].copy_from_slice(&self.buf);
    }
}

/// TNS data for the one long window.
fn read_tns(b: &mut Bits<'_>) -> Result<Tns> {
    let short = || Error::Frame("short TNS");
    let n_filt = b.read(2).ok_or_else(short)? as usize;
    let mut filters = Vec::with_capacity(n_filt);
    if n_filt == 0 {
        return Ok(Tns { filters });
    }
    let coef_res = b.read(1).ok_or_else(short)?;
    for _ in 0..n_filt {
        let length = b.read(6).ok_or_else(short)? as usize;
        let order = b.read(5).ok_or_else(short)? as usize;
        if order > TNS_MAX_ORDER {
            return Err(Error::Frame("TNS order"));
        }
        let mut coefs = Vec::with_capacity(order);
        let mut down = false;
        if order > 0 {
            down = b.read(1).ok_or_else(short)? == 1;
            let compress = b.read(1).ok_or_else(short)?;
            let res = coef_res + 3;
            let len = res - compress;
            for _ in 0..order {
                let raw = b.read(len).ok_or_else(short)? as i32;
                // Signed, `len` bits; inverse-quantized on the arcsine scale.
                let v = if raw >= 1 << (len - 1) { raw - (1 << len) } else { raw };
                let half = (1 << (res - 1)) as f32;
                let step = if v >= 0 { (half - 0.5) / std::f32::consts::FRAC_PI_2 } else { (half + 0.5) / std::f32::consts::FRAC_PI_2 };
                coefs.push(-(v as f32 / step).sin());
            }
        }
        filters.push((length, order, down, coefs));
    }
    Ok(Tns { filters })
}

/// Reflection coefficients to the filter's (Levinson's step-up).
fn lpc(coefs: &[f32]) -> [f32; TNS_MAX_ORDER] {
    let mut lpc = [0f32; TNS_MAX_ORDER];
    for (i, c) in coefs.iter().enumerate() {
        let r = -c;
        lpc[i] = r;
        for j in 0..(i + 1) >> 1 {
            let (f, b) = (lpc[j], lpc[i - 1 - j]);
            lpc[j] = f + r * b;
            lpc[i - 1 - j] = b + r * f;
        }
    }
    lpc
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_airpods_config() {
        assert_eq!(super::config(&[0xF8, 0xE6, 0x30, 0x00]), Ok(super::Config { frequency_index: 3 }));
        assert!(super::config(&[0x12, 0x10]).is_err());
    }
}
