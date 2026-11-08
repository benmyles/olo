//! Opus for symphonia: its Ogg reader finds Opus streams but it has no
//! decoder, so this one wraps the pure-Rust `opus-decoder` (all modes,
//! RFC 8251 conformant) in symphonia's `Decoder` trait. [`codecs`] is
//! symphonia's registry with it added.

use std::sync::OnceLock;

use opus_decoder::{OpusDecoder, OpusMultistreamDecoder};
use symphonia::core::{
    audio::{AsAudioBufferRef, AudioBuffer, AudioBufferRef, Channels, Signal, SignalSpec},
    codecs::{CODEC_TYPE_OPUS, CodecDescriptor, CodecParameters, CodecRegistry, Decoder, DecoderOptions, FinalizeResult},
    errors::{Error, Result},
    formats::Packet,
    support_codec,
};

/// Symphonia's codecs and Opus.
pub fn codecs() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut r = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut r);
        r.register_all::<Opus>();
        r
    })
}

/// The longest Opus packet: 120 ms at 48 kHz.
const MAX_FRAME: usize = 5760;

enum Inner {
    Single(Box<OpusDecoder>),
    Multi(OpusMultistreamDecoder),
}

pub struct Opus {
    params: CodecParameters,
    inner: Inner,
    channels: usize,
    pcm: Vec<f32>,
    buf: AudioBuffer<f32>,
}

/// Channel count and decoder from the OpusHead in `extra` (RFC 7845 §5.1).
fn from_head(extra: &[u8]) -> Result<(usize, Inner)> {
    if extra.len() < 19 || &extra[..8] != b"OpusHead" {
        return Err(Error::DecodeError("opus: no identification header"));
    }
    let channels = extra[9] as usize;
    let bad = |_| Error::DecodeError("opus: bad stream parameters");
    match extra[18] {
        0 if (1..=2).contains(&channels) => Ok((channels, Inner::Single(Box::new(OpusDecoder::new(48_000, channels).map_err(bad)?)))),
        _ => {
            let table = extra.get(19..21 + channels).ok_or(Error::DecodeError("opus: short channel mapping"))?;
            let (streams, coupled, mapping) = (table[0] as usize, table[1] as usize, &table[2..]);
            Ok((channels, Inner::Multi(OpusMultistreamDecoder::new(48_000, channels, streams, coupled, mapping).map_err(bad)?)))
        }
    }
}

impl Decoder for Opus {
    fn try_new(params: &CodecParameters, _options: &DecoderOptions) -> Result<Self> {
        let extra = params.extra_data.as_deref().ok_or(Error::DecodeError("opus: no identification header"))?;
        let (channels, inner) = from_head(extra)?;
        // Matroska leaves the layout out: take the header's count.
        let layout = params.channels.unwrap_or(match channels {
            1 => Channels::FRONT_LEFT,
            2 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            n => Channels::from_bits_truncate((1u32 << n.min(31)) - 1),
        });
        let mut params = params.clone();
        params.channels = Some(layout);
        Ok(Opus { params, inner, channels, pcm: vec![0.0; MAX_FRAME * channels], buf: AudioBuffer::new(MAX_FRAME as u64, SignalSpec::new(48_000, layout)) })
    }

    fn supported_codecs() -> &'static [CodecDescriptor] {
        &[support_codec!(CODEC_TYPE_OPUS, "opus", "Opus")]
    }

    fn reset(&mut self) {
        match &mut self.inner {
            Inner::Single(d) => d.reset(),
            Inner::Multi(d) => d.reset(),
        }
    }

    fn codec_params(&self) -> &CodecParameters {
        &self.params
    }

    fn decode(&mut self, packet: &Packet) -> Result<AudioBufferRef<'_>> {
        let n = match &mut self.inner {
            Inner::Single(d) => d.decode_float(&packet.data, &mut self.pcm, false),
            Inner::Multi(d) => d.decode_float(&packet.data, &mut self.pcm, false),
        }
        .map_err(|_| Error::DecodeError("opus: bad packet"))?;
        self.buf.clear();
        self.buf.render_reserved(Some(n));
        for ch in 0..self.channels {
            let plane = self.buf.chan_mut(ch);
            for (i, s) in plane.iter_mut().enumerate() {
                *s = self.pcm[i * self.channels + ch];
            }
        }
        // Encoder delay at the start, padding at the end (gapless).
        self.buf.trim(packet.trim_start() as usize, packet.trim_end() as usize);
        Ok(self.buf.as_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> AudioBufferRef<'_> {
        self.buf.as_audio_buffer_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads() {
        let mut head = b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0".to_vec();
        assert!(matches!(from_head(&head), Ok((2, Inner::Single(_)))));
        // Family 1, 6 channels: 4 streams, 2 coupled, a mapping table.
        head[9] = 6;
        head[18] = 1;
        head.extend_from_slice(&[4, 2, 0, 4, 1, 2, 3, 5]);
        assert!(matches!(from_head(&head), Ok((6, Inner::Multi(_)))));
        assert!(from_head(b"OpusTags").is_err());
    }

    /// `OLO_SAMPLES=dir cargo test -p olo-preview opus_file -- --ignored`
    /// with an .opus file named sample.opus in `dir`.
    #[test]
    #[ignore]
    fn opus_file() {
        use symphonia::core::{formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint};
        let dir = std::path::PathBuf::from(std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES"));
        let file = std::fs::File::open(dir.join("sample.opus")).unwrap();
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("opus");
        let mut format = symphonia::default::get_probe().format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default()).unwrap().format;
        // The first track it can decode, as audio.rs takes it (an Ogg file
        // may carry a picture first: Commons' .opus files with VP8).
        let (track, mut dec) = format.tracks().iter().find_map(|t| codecs().make(&t.codec_params, &DecoderOptions::default()).ok().map(|d| (t.id, d))).expect("an Opus track");
        let (mut frames, mut energy) = (0usize, 0f64);
        while let Ok(p) = format.next_packet() {
            if p.track_id() != track {
                continue;
            }
            let AudioBufferRef::F32(b) = dec.decode(&p).unwrap() else { panic!("f32") };
            frames += b.frames();
            energy += b.chan(0).iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
            if frames > 48_000 * 10 {
                break;
            }
        }
        assert!(frames > 48_000 * 5, "{frames} frames");
        assert!(energy / frames as f64 > 1e-5, "audible");
    }
}
