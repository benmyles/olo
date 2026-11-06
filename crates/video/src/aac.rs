//! Sound for recordings: AAC-LC (what every player plays in an MP4),
//! encoded by Fraunhofer's libfdk-aac, opened at run time (`dlopen`, as
//! NVENC is) so Olo builds and starts without it, and [`Aac::new`] says
//! why there's no sound where it isn't installed (Fedora's
//! `fdk-aac-free`). 16-bit stereo or mono, 1024 samples a channel a
//! frame, each frame's access unit raw (the MP4's `esds` carries the
//! decoder's configuration).

use std::{ffi::c_void, ptr::null_mut};

use crate::nvenc::Lib;

/// The encoder's handle, opaque.
type Handle = *mut c_void;

#[repr(C)]
struct BufDesc {
    num_bufs: i32,
    bufs: *mut *mut c_void,
    ids: *mut i32,
    sizes: *mut i32,
    el_sizes: *mut i32,
}

#[repr(C)]
struct InArgs {
    num_in_samples: i32,
    num_anc_bytes: i32,
}

#[repr(C)]
#[derive(Default)]
struct OutArgs {
    num_out_bytes: i32,
    num_in_samples: i32,
    num_anc_bytes: i32,
    bit_res_state: i32,
}

#[repr(C)]
struct Info {
    max_out_buf_bytes: u32,
    max_anc_bytes: u32,
    in_buf_fill_level: u32,
    input_channels: u32,
    frame_length: u32,
    n_delay: u32,
    n_delay_core: u32,
    conf_buf: [u8; 64],
    conf_size: u32,
}

type Open = unsafe extern "C" fn(*mut Handle, u32, u32) -> i32;
type Close = unsafe extern "C" fn(*mut Handle) -> i32;
type SetParam = unsafe extern "C" fn(Handle, u32, u32) -> i32;
type Encode = unsafe extern "C" fn(Handle, *const BufDesc, *const BufDesc, *const InArgs, *mut OutArgs) -> i32;
type GetInfo = unsafe extern "C" fn(Handle, *mut Info) -> i32;

const AOT: u32 = 0x0100;
const BITRATE: u32 = 0x0101;
const SAMPLERATE: u32 = 0x0103;
const CHANNELMODE: u32 = 0x0106;
const AFTERBURNER: u32 = 0x0200;
const TRANSMUX: u32 = 0x0300;
const IN_AUDIO_DATA: i32 = 0;
const OUT_BITSTREAM_DATA: i32 = 3;
const ENCODE_EOF: i32 = 0x80;

/// Samples a channel in each frame.
pub const FRAME: usize = 1024;
/// A recording's sound: 160 kbit/s (stereo AAC-LC, transparent).
pub const SOUND_BITRATE: u32 = 160_000;

/// An AAC-LC encoder.
pub struct Aac {
    handle: Handle,
    encode: Encode,
    close: Close,
    channels: u32,
    /// The decoder's configuration (AudioSpecificConfig), for the MP4.
    pub config: Vec<u8>,
    /// Samples a channel the encoder holds back (its priming), cut by
    /// the MP4's edit list.
    pub delay: u32,
    out: Vec<u8>,
    _lib: Lib,
}

// (Used from one thread at a time.)
unsafe impl Send for Aac {}

impl Aac {
    /// An encoder of `channels` (1 or 2) at `rate` samples a second and
    /// `bitrate` bits a second.
    pub fn new(rate: u32, channels: u32, bitrate: u32) -> Result<Aac, String> {
        let lib = Lib::open("libfdk-aac.so.2").map_err(|e| format!("no AAC encoder (libfdk-aac): {e}"))?;
        // SAFETY: the functions as aacenc_lib.h declares them.
        let (open, close, set, encode, info): (Open, Close, SetParam, Encode, GetInfo) =
            unsafe { (lib.get("aacEncOpen")?, lib.get("aacEncClose")?, lib.get("aacEncoder_SetParam")?, lib.get("aacEncEncode")?, lib.get("aacEncInfo")?) };
        let mut handle: Handle = null_mut();
        // SAFETY: a handle for the encoder to make; every module, this
        // many channels.
        if unsafe { open(&mut handle, 0, channels) } != 0 {
            return Err("libfdk-aac wouldn't open an encoder".into());
        }
        let mut aac = Aac { handle, encode, close, channels, config: Vec::new(), delay: 0, out: vec![0u8; 8192], _lib: lib };
        // AAC-LC, raw access units, the afterburner (better sound for a
        // little more work).
        for (p, v) in [(AOT, 2), (SAMPLERATE, rate), (CHANNELMODE, channels), (BITRATE, bitrate), (TRANSMUX, 0), (AFTERBURNER, 1)] {
            // SAFETY: our handle; parameters libfdk takes.
            if unsafe { set(aac.handle, p, v) } != 0 {
                return Err(format!("libfdk-aac refused a setting ({p:#06x} = {v})"));
            }
        }
        // (Encoding nothing applies the settings.)
        // SAFETY: null buffers with the encoder's own arguments: allowed.
        if unsafe { encode(aac.handle, std::ptr::null(), std::ptr::null(), std::ptr::null(), null_mut()) } != 0 {
            return Err("libfdk-aac couldn't start with those settings".into());
        }
        let mut i = std::mem::MaybeUninit::<Info>::zeroed();
        // SAFETY: the encoder fills the whole structure.
        if unsafe { info(aac.handle, i.as_mut_ptr()) } != 0 {
            return Err("libfdk-aac gave no configuration".into());
        }
        // SAFETY: zeroed, then filled: every field a plain number.
        let i = unsafe { i.assume_init() };
        aac.config = i.conf_buf[..(i.conf_size as usize).min(64)].to_vec();
        aac.delay = i.n_delay;
        aac.out.resize((i.max_out_buf_bytes as usize).max(8192), 0);
        Ok(aac)
    }

    /// One frame's samples (`FRAME` a channel, interleaved), its access
    /// unit if one came out (the first few go into the encoder's delay).
    pub fn encode(&mut self, samples: &[i16]) -> Result<Option<Vec<u8>>, String> {
        self.call(samples.as_ptr() as *mut c_void, samples.len() as i32)
    }

    /// What the encoder still holds, frame by frame, at the end.
    pub fn flush(&mut self) -> Result<Vec<Vec<u8>>, String> {
        let mut out = Vec::new();
        for _ in 0..16 {
            match self.call(null_mut(), -1)? {
                Some(au) => out.push(au),
                None => break,
            }
        }
        Ok(out)
    }

    fn call(&mut self, input: *mut c_void, n: i32) -> Result<Option<Vec<u8>>, String> {
        let mut in_ptr = input;
        let (mut in_id, mut in_size, mut in_el) = (IN_AUDIO_DATA, n.max(0) * 2, 2i32);
        let in_desc = BufDesc { num_bufs: if n > 0 { 1 } else { 0 }, bufs: &mut in_ptr, ids: &mut in_id, sizes: &mut in_size, el_sizes: &mut in_el };
        let mut out_ptr = self.out.as_mut_ptr() as *mut c_void;
        let (mut out_id, mut out_size, mut out_el) = (OUT_BITSTREAM_DATA, self.out.len() as i32, 1i32);
        let out_desc = BufDesc { num_bufs: 1, bufs: &mut out_ptr, ids: &mut out_id, sizes: &mut out_size, el_sizes: &mut out_el };
        let args = InArgs { num_in_samples: n, num_anc_bytes: 0 };
        let mut outargs = OutArgs::default();
        // SAFETY: buffers that live through the call, sized as said.
        let r = unsafe { (self.encode)(self.handle, &in_desc, &out_desc, &args, &mut outargs) };
        if r == ENCODE_EOF {
            return Ok(None);
        }
        if r != 0 {
            return Err(format!("libfdk-aac failed to encode ({r:#x})"));
        }
        let got = outargs.num_out_bytes.max(0) as usize;
        Ok((got > 0).then(|| self.out[..got].to_vec()))
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    /// What an MP4's sound track says of it.
    pub fn track(&self) -> Track {
        Track { config: self.config.clone(), channels: self.channels, delay: self.delay }
    }
}

/// An encoder's description, for an MP4 (`Mp4::with_sound`), made where
/// the encoder is and handed to where the file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub config: Vec<u8>,
    pub channels: u32,
    pub delay: u32,
}

impl Drop for Aac {
    fn drop(&mut self) {
        // SAFETY: our handle, once.
        unsafe { (self.close)(&mut self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second of a tone encoded, where libfdk-aac is installed.
    #[test]
    fn a_tone() {
        let Ok(mut aac) = Aac::new(48_000, 2, 160_000) else { return eprintln!("no libfdk-aac here") };
        // AAC-LC, 48 kHz (index 3), stereo: 0x11 0x90.
        assert_eq!(aac.config, [0x11, 0x90]);
        let mut aus = Vec::new();
        for f in 0..47 {
            let frame: Vec<i16> = (0..FRAME)
                .flat_map(|i| {
                    let t = (f * FRAME + i) as f32 / 48_000.0;
                    let v = ((t * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16;
                    [v, v]
                })
                .collect();
            aus.extend(aac.encode(&frame).unwrap());
        }
        aus.extend(aac.flush().unwrap());
        assert!(aus.len() >= 47, "{} frames", aus.len());
        let bytes: usize = aus.iter().map(Vec::len).sum();
        // About 160 kbit/s over a second.
        assert!((10_000..30_000).contains(&bytes), "{bytes} bytes");
        assert!(aac.delay > 0);
    }
}
