//! Olo's decoder against libfdk-aac's, on a stream libfdk's encoder made
//! from a synthetic sweep (`data/sweep.eld`: the AudioSpecificConfig, then
//! frames, each after its length as u16 little-endian; `data/sweep.pcm`:
//! libfdk's decode, 16-bit). Made by the script in `data/README.md`.

fn frames(data: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut all = Vec::new();
    let mut at = 0;
    while at + 2 <= data.len() {
        let n = u16::from_le_bytes([data[at], data[at + 1]]) as usize;
        all.push(data[at + 2..at + 2 + n].to_vec());
        at += 2 + n;
    }
    let asc = all.remove(0);
    (asc, all)
}

#[test]
fn decodes_as_libfdk_does() {
    let (asc, fs) = frames(include_bytes!("data/sweep.eld"));
    let want: Vec<f32> = include_bytes!("data/sweep.pcm").as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect();
    let mut d = olo_eld::Decoder::new(&asc).unwrap();
    let mut got = Vec::new();
    let mut pcm = [0f32; olo_eld::FRAME];
    for f in &fs {
        d.decode(f, &mut pcm).unwrap();
        got.extend_from_slice(&pcm);
    }
    assert_eq!(got.len(), want.len());
    // Signal against the difference, over everything but the first frames.
    let (mut sig, mut err) = (0f64, 0f64);
    for (g, w) in got.iter().zip(&want).skip(4 * olo_eld::FRAME) {
        sig += (*w as f64).powi(2);
        err += (*g as f64 - *w as f64).powi(2);
    }
    let snr = 10.0 * (sig / err.max(1e-20)).log10();
    eprintln!("SNR against libfdk: {snr:.1} dB");
    assert!(snr > 60.0, "SNR {snr:.1} dB");
}

/// A real AirPods capture against libfdk's decode of it (not in the repo:
/// `OLO_ELD_CAPTURE=mic.bin OLO_ELD_REFERENCE=ref.wav cargo test -p
/// olo-eld -- --ignored`; mic.bin as `olo-airpods` keeps it with
/// OLO_AIRPODS_CAPTURE, ref.wav a 16-bit WAV).
#[test]
#[ignore]
fn decodes_a_capture_as_libfdk_does() {
    let (Ok(cap), Ok(reference)) = (std::env::var("OLO_ELD_CAPTURE"), std::env::var("OLO_ELD_REFERENCE")) else { return };
    let data = std::fs::read(cap).unwrap();
    let wav = std::fs::read(reference).unwrap();
    let want: Vec<f32> = wav[44..].as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect();
    let mut d = olo_eld::Decoder::new(&[0xF8, 0xE6, 0x30, 0x00]).unwrap();
    let (mut got, mut pcm, mut at, mut bad) = (Vec::new(), [0f32; olo_eld::FRAME], 0, 0);
    let started = std::time::Instant::now();
    while at + 6 <= data.len() {
        let n = u16::from_le_bytes([data[at + 4], data[at + 5]]) as usize;
        if d.decode(&data[at + 6..at + 6 + n], &mut pcm).is_err() {
            bad += 1;
        }
        got.extend_from_slice(&pcm);
        at += 6 + n;
    }
    let took = started.elapsed();
    let (mut sig, mut err) = (0f64, 0f64);
    for (g, w) in got.iter().zip(&want).skip(4 * olo_eld::FRAME) {
        sig += (*w as f64).powi(2);
        err += (*g as f64 - *w as f64).powi(2);
    }
    let snr = 10.0 * (sig / err.max(1e-20)).log10();
    eprintln!("{} frames ({bad} not read) in {:.1} ms; SNR against libfdk: {snr:.1} dB", got.len() / olo_eld::FRAME, took.as_secs_f64() * 1000.0);
    assert_eq!(bad, 0);
    assert!(snr > 50.0);
}
