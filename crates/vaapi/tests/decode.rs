//! AV1 and VP9 decoded on this machine's GPU through VA-API: every
//! picture as the reference decoder makes it (each one's MD5), hidden
//! frames and frames shown again included. Where no GPU here decodes a
//! codec, its test says so and passes.

use olo_vaapi::decode::{Codec, Decoder, Picture, available};

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

fn md5_of(p: &Picture) -> String {
    let mut c = md5::Context::new();
    c.consume(&p.y);
    c.consume(&p.uv);
    format!("{:x}", c.compute())
}

/// Decodes `name` and checks each picture against its MD5s.
fn check(codec: Codec, name: &str) {
    if !available(codec) {
        eprintln!("no GPU here decodes {codec:?} through VA-API: {name} not checked");
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let data = std::fs::read(dir.join(name)).unwrap();
    let sums = std::fs::read_to_string(dir.join(format!("{name}.md5"))).unwrap();
    let want: Vec<&str> = sums.lines().collect();
    let mut d = Decoder::new(codec).unwrap();
    let mut out = Vec::new();
    for (i, frame) in ivf(&data).into_iter().enumerate() {
        d.decode(frame, i as i64, &mut out).unwrap_or_else(|e| panic!("{name}: frame {i}: {e}"));
    }
    assert_eq!(out.len(), want.len(), "{name}: pictures shown");
    for (k, (p, w)) in out.iter().zip(&want).enumerate() {
        assert_eq!(md5_of(p), *w, "{name}: picture {k} ({}×{}, from frame {})", p.width, p.height, p.pts);
    }
    eprintln!("{name}: {} pictures as the reference decoder made them, on {}", out.len(), d.driver());
}

#[test]
fn av1_pictures_are_the_reference_decoder_s() {
    check(Codec::Av1, "av1-25fps.ivf");
}

#[test]
fn vp9_pictures_are_the_reference_decoder_s() {
    check(Codec::Vp9, "vp9-25fps.ivf");
}

#[test]
fn vp9_frames_shown_again_and_hidden_ones() {
    check(Codec::Vp9, "vp9-show-existing.ivf");
}

/// After a seek (reset), pictures start again at the next key frame and
/// are the same as when decoded from the start.
#[test]
fn a_reset_starts_again_at_a_key_frame() {
    if !available(Codec::Vp9) {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let data = std::fs::read(dir.join("vp9-25fps.ivf")).unwrap();
    let frames = ivf(&data);
    let mut d = Decoder::new(Codec::Vp9).unwrap();
    let mut out = Vec::new();
    for (i, f) in frames.iter().take(10).enumerate() {
        d.decode(f, i as i64, &mut out).unwrap();
    }
    d.reset();
    out.clear();
    // From the first frame again (the stream's only key frame).
    for (i, f) in frames.iter().take(5).enumerate() {
        d.decode(f, i as i64, &mut out).unwrap();
    }
    let sums = std::fs::read_to_string(dir.join("vp9-25fps.ivf.md5")).unwrap();
    let want: Vec<&str> = sums.lines().take(5).collect();
    assert_eq!(out.iter().map(md5_of).collect::<Vec<_>>(), want);
}
