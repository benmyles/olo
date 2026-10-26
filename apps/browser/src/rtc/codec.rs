//! A call's codecs: H.264 (openh264, constrained baseline, as browsers
//! and Meet's and Zoom's servers all take) and Opus (rusty-opus to
//! send, opus-decoder to hear), turning hub frames into RTP payloads
//! and back.

use openh264::{
    OpenH264API,
    decoder::{Decoder, DecoderConfig},
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, RateControlMode, UsageType},
    formats::YUVSource,
};

use super::frames::{Picture, Sound};

impl YUVSource for Picture {
    fn dimensions(&self) -> (usize, usize) {
        (self.width as usize, self.height as usize)
    }
    fn strides(&self) -> (usize, usize, usize) {
        let w = self.width as usize;
        (w, w / 2, w / 2)
    }
    fn y(&self) -> &[u8] {
        self.planes().0
    }
    fn u(&self) -> &[u8] {
        self.planes().1
    }
    fn v(&self) -> &[u8] {
        self.planes().2
    }
}

/// The biggest picture we send: what a call shows a face at.
pub const MAX_SEND: (u32, u32) = (1280, 720);

/// Bits a second for a picture of `w`×`h`.
fn bitrate_for(w: u32, h: u32) -> u32 {
    match w * h {
        p if p >= 1280 * 720 => 1_500_000,
        p if p >= 640 * 480 => 800_000,
        p if p >= 320 * 240 => 350_000,
        _ => 150_000,
    }
}

pub struct VideoEncoder {
    encoder: Option<(Encoder, u32, u32)>,
    /// Asked for by the far end (a lost picture) or a new size.
    pub key_wanted: bool,
    /// The page's cap (`setParameters` maxBitrate), if any.
    pub max_bitrate: Option<u32>,
}

impl VideoEncoder {
    pub fn new() -> VideoEncoder {
        VideoEncoder { encoder: None, key_wanted: true, max_bitrate: None }
    }

    /// One picture as an Annex B access unit (empty when the encoder
    /// skips it to keep to the bitrate).
    pub fn encode(&mut self, p: &Picture) -> Result<Vec<u8>, String> {
        let scaled = p.fit(MAX_SEND.0, MAX_SEND.1);
        let p = scaled.as_ref().unwrap_or(p);
        if self.encoder.as_ref().is_none_or(|(_, w, h)| (*w, *h) != (p.width, p.height)) {
            let rate = bitrate_for(p.width, p.height).min(self.max_bitrate.unwrap_or(u32::MAX)).max(50_000);
            let config = EncoderConfig::new()
                .bitrate(BitRate::from_bps(rate))
                .max_frame_rate(FrameRate::from_hz(30.0))
                .usage_type(UsageType::CameraVideoRealTime)
                .rate_control_mode(RateControlMode::Bitrate)
                .skip_frames(true);
            let e = Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| e.to_string())?;
            self.encoder = Some((e, p.width, p.height));
            self.key_wanted = true;
        }
        let Some((e, _, _)) = &mut self.encoder else { return Ok(Vec::new()) };
        if std::mem::take(&mut self.key_wanted) {
            e.force_intra_frame();
        }
        Ok(e.encode(p).map_err(|e| e.to_string())?.to_vec())
    }
}

pub struct VideoDecoder {
    decoder: Decoder,
}

impl VideoDecoder {
    pub fn new() -> Result<VideoDecoder, String> {
        let decoder = Decoder::with_api_config(OpenH264API::from_source(), DecoderConfig::new()).map_err(|e| e.to_string())?;
        Ok(VideoDecoder { decoder })
    }

    /// An Annex B access unit to a picture (None while the decoder waits
    /// for more, or on a broken one).
    pub fn decode(&mut self, unit: &[u8]) -> Result<Option<Picture>, String> {
        let Some(yuv) = self.decoder.decode(unit).map_err(|e| e.to_string())? else { return Ok(None) };
        let (w, h) = yuv.dimensions();
        let (sy, su, sv) = yuv.strides();
        let (w, h) = (w & !1, h & !1);
        let mut data = Vec::with_capacity(w * h * 3 / 2);
        for row in 0..h {
            data.extend_from_slice(&yuv.y()[row * sy..row * sy + w]);
        }
        for (plane, stride) in [(yuv.u(), su), (yuv.v(), sv)] {
            for row in 0..h / 2 {
                data.extend_from_slice(&plane[row * stride..row * stride + w / 2]);
            }
        }
        Ok(Some(Picture { width: w as u32, height: h as u32, data }))
    }
}

/// 20 ms of 48 kHz sound at a time, as browsers send.
pub const OPUS_FRAME: usize = 960;

pub struct AudioEncoder {
    encoder: rusty_opus::OpusEncoder,
    /// Samples waiting for a whole frame.
    pending: Vec<f32>,
    out: Vec<u8>,
}

impl AudioEncoder {
    pub fn new() -> Result<AudioEncoder, String> {
        let mut encoder = rusty_opus::OpusEncoder::new(48000, 1, rusty_opus::Application::Voip).map_err(|e| format!("{e:?}"))?;
        encoder.bitrate_bps = 32_000;
        encoder.use_inband_fec = true;
        encoder.packet_loss_perc = 5;
        Ok(AudioEncoder { encoder, pending: Vec::with_capacity(OPUS_FRAME * 2), out: vec![0; 1500] })
    }

    /// Feed sound (48 kHz; channels mixed to one); returns each Opus
    /// packet now complete.
    pub fn push(&mut self, s: &Sound) -> Vec<Vec<u8>> {
        let ch = s.channels.max(1) as usize;
        if s.rate == 48000 {
            self.pending.extend(s.samples.chunks(ch).map(|f| f.iter().map(|&x| x as f32).sum::<f32>() / ch as f32 / 32768.0));
        }
        let mut packets = Vec::new();
        while self.pending.len() >= OPUS_FRAME {
            let frame: Vec<f32> = self.pending.drain(..OPUS_FRAME).collect();
            match self.encoder.encode(&frame, OPUS_FRAME, &mut self.out) {
                Ok(n) => packets.push(self.out[..n].to_vec()),
                Err(e) => tracing::debug!("opus: {e:?}"),
            }
        }
        packets
    }
}

pub struct AudioDecoder {
    decoder: opus_decoder::OpusDecoder,
    pcm: Vec<i16>,
}

impl AudioDecoder {
    pub fn new() -> Result<AudioDecoder, String> {
        let decoder = opus_decoder::OpusDecoder::new(48000, 2).map_err(|e| format!("{e:?}"))?;
        Ok(AudioDecoder { decoder, pcm: vec![0; 5760 * 2] })
    }

    /// One packet to stereo samples; an empty packet stands for one lost.
    pub fn decode(&mut self, packet: &[u8]) -> Option<Sound> {
        let n = self.decoder.decode(packet, &mut self.pcm, false).ok()?;
        Some(Sound { rate: 48000, channels: 2, samples: self.pcm[..n * 2].to_vec() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_and_sound_round_trip() {
        let mut p = Picture::black(320, 240);
        for (i, y) in p.data[..320 * 240].iter_mut().enumerate() {
            *y = (i % 320) as u8 / 2 + 40;
        }
        let mut enc = VideoEncoder::new();
        let mut dec = VideoDecoder::new().unwrap();
        let unit = enc.encode(&p).unwrap();
        assert!(unit.starts_with(&[0, 0, 0, 1]));
        let out = dec.decode(&unit).unwrap().expect("a picture");
        assert_eq!((out.width, out.height), (320, 240));
        assert!((out.data[160] as i32 - p.data[160] as i32).abs() < 12);

        let mut aenc = AudioEncoder::new().unwrap();
        let mut adec = AudioDecoder::new().unwrap();
        let tone: Vec<i16> = (0..4800).map(|i| ((i as f64 * 440.0 * std::f64::consts::TAU / 48000.0).sin() * 8000.0) as i16).collect();
        let packets = aenc.push(&Sound { rate: 48000, channels: 1, samples: tone });
        assert_eq!(packets.len(), 5);
        let heard: Vec<i16> = packets.iter().flat_map(|p| adec.decode(p).unwrap().samples).collect();
        assert_eq!(heard.len(), 4800 * 2);
        let rms = (heard[4800..].iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / 4800.0).sqrt();
        assert!(rms > 3000.0, "rms {rms}");
    }
}
