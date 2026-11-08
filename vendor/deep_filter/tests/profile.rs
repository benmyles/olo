//! Where a model's time goes, network by network (Olo):
//! `OLO_DF_MODEL=model.tar.gz cargo test --release -p deep_filter --test profile -- --ignored --nocapture`.

use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;

#[test]
#[ignore]
fn where_a_frame_goes() {
    let path = std::env::var("OLO_DF_MODEL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/models/DeepFilterNet3_onnx.tar.gz").into());
    let params = DfParams::new(path.clone().into()).unwrap();
    let rp = RuntimeParams::default_with_ch(1).with_atten_lim(30.0).with_thresholds(-10.0, 35.0, 35.0);
    let mut d = DfTract::new(params, &rp).unwrap();
    let hop = d.hop_size;
    if std::env::var_os("OLO_DF_NODES").is_some() {
        d.nodes = Some(Default::default());
    }
    // A voice (the voice crate's, 16 kHz, each sample thrice) in room
    // noise: every stage runs, as on a call.
    let raw = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/voice/tests/data/slt.raw")).unwrap();
    let voice: Vec<f32> = raw.chunks_exact(2).flat_map(|c| std::iter::repeat_n(i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0, 3)).collect();
    let mut x = 1u32;
    let mut noisy = Array2::<f32>::zeros((1, hop));
    let mut enh = Array2::<f32>::zeros((1, hop));
    // Paced as live (a frame every 10 ms, the caches cold between) with
    // OLO_DF_PACED, fewer frames.
    let paced = std::env::var_os("OLO_DF_PACED").is_some();
    let n = (voice.len() / hop).min(if paced { 400 } else { 1500 });
    let mut busy = std::time::Duration::ZERO;
    let t = std::time::Instant::now();
    for k in 0..n {
        for (i, v) in noisy.iter_mut().enumerate() {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let w = (x >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            *v = 0.6 * voice[k * hop + i] + 0.03 * w;
        }
        if paced {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let t = std::time::Instant::now();
        d.process(noisy.view(), enh.view_mut()).unwrap();
        busy += t.elapsed();
    }
    let _ = t;
    let all = busy.as_secs_f64() * 1e6 / n as f64;
    let per = |i: usize| d.spent[i].as_secs_f64() * 1e6 / d.frames.max(1) as f64;
    eprintln!("{path}: {all:.0} µs a frame: encoder {:.0}, ERB decoder {:.0}, DF decoder {:.0}, the rest {:.0}", per(0), per(1), per(2), all - per(0) - per(1) - per(2));
    if let Some(nodes) = &d.nodes {
        let mut v: Vec<_> = nodes.iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        for (name, (t, runs)) in v.iter().take(40) {
            eprintln!("{:>7.1} µs  {:>5} runs  {name}", t.as_secs_f64() * 1e6 / d.frames.max(1) as f64, runs);
        }
    }
}

/// The 8-bit product the same with each kernel this processor has, and
/// close to the float product (to the weights' and the vector's rounding).
#[test]
fn q8_products_match() {
    use df::q8::{Kernel, Q8};
    for (n, k) in [(37, 70), (64, 512), (5, 1024)] {
        let mut seed = 7u32;
        let mut r = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let w: Vec<f32> = (0..n * k).map(|_| r()).collect();
        // The largest weights and the largest vector, for the sums' range.
        let w: Vec<f32> = w.iter().enumerate().map(|(i, v)| if i % k == 3 { -1.0 } else { *v }).collect();
        let x: Vec<f32> = (0..k).map(|i| if i % 2 == 0 { -3.0 } else { r() * 3.0 }).collect();
        let product = |kernel: Kernel| {
            let mut y = vec![0f32; n];
            Q8::new(n, k, |r, c| w[r * k + c]).with(kernel).matvec(&x, &mut y);
            y
        };
        let y = product(Kernel::best());
        let exact: Vec<f32> = (0..n).map(|r| (0..k).map(|c| w[r * k + c] * x[c]).sum()).collect();
        // Off by under 1% (rms), as 8-bit weights are.
        let rms = |v: &mut dyn Iterator<Item = f32>| (v.map(|e| e * e).sum::<f32>() / n as f32).sqrt();
        let off = rms(&mut y.iter().zip(&exact).map(|(a, b)| a - b)) / rms(&mut exact.iter().copied());
        assert!(off < 0.01, "{off}");
        assert_eq!(y, product(Kernel::Plain));
        if Kernel::best() == Kernel::Vnni {
            assert_eq!(y, product(Kernel::Avx2));
        }
    }
}

/// The 8-bit product's speed for a 1536 × 512 matrix (DeepFilterNet3-LL's
/// GRUs): one matrix again and again (in L2: the kernel's own speed), twelve
/// in turn (9 MB: from the last-level cache) and twelve with the caches
/// flushed between rounds (from memory).
#[test]
#[ignore]
fn q8_speed() {
    use df::q8::Q8;
    let (n, k) = (1536, 512);
    let mats: Vec<Q8> = (0..12).map(|m| Q8::new(n, k, |r, c| (((r * 7 + c * 13 + m) % 255) as f32 - 127.0) / 127.0)).collect();
    let x: Vec<f32> = (0..k).map(|i| ((i % 17) as f32 - 8.0) / 8.0).collect();
    let mut y = vec![0f32; n];
    let time = |f: &mut dyn FnMut()| {
        let t = std::time::Instant::now();
        f();
        t.elapsed().as_secs_f64() * 1e6
    };
    eprintln!("{:?}", mats[0]);
    let one = time(&mut || (0..2000).for_each(|_| mats[0].matvec(&x, &mut y))) / 2000.0;
    let twelve = time(&mut || (0..200).for_each(|_| mats.iter().for_each(|m| m.matvec(&x, &mut y)))) / 2400.0;
    let mut scrub = vec![0u8; 64 << 20];
    let mut cold = 0.0;
    for r in 0..50 {
        scrub.iter_mut().step_by(64).for_each(|b| *b = b.wrapping_add(r));
        cold += time(&mut || mats.iter().for_each(|m| m.matvec(&x, &mut y)));
    }
    eprintln!("1536 × 512 in 8 bits: {one:.1} µs hot, {twelve:.1} µs from the cache, {:.1} µs from memory ({} bytes)", cold / 600.0, mats[0].bytes());
}

/// The local SNR each model sees in a voice's pauses and in its words (room
/// noise under it): where frames of noise alone are gated (under -10 dB).
#[test]
#[ignore]
fn lsnr_in_pauses() {
    let raw = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/voice/tests/data/slt.raw")).unwrap();
    let voice: Vec<f32> = raw.chunks_exact(2).flat_map(|c| std::iter::repeat_n(i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0, 3)).collect();
    for path in [concat!(env!("CARGO_MANIFEST_DIR"), "/models/DeepFilterNet3_onnx.tar.gz"), concat!(env!("CARGO_MANIFEST_DIR"), "/models/DeepFilterNet3_ll_onnx.tar.gz")] {
        let params = DfParams::new(path.into()).unwrap();
        let rp = RuntimeParams::default_with_ch(1).with_atten_lim(100.0).with_thresholds(-10.0, 35.0, 35.0);
        let mut d = DfTract::new(params, &rp).unwrap();
        let hop = d.hop_size;
        let la = d.lookahead;
        let (mut noisy, mut enh) = (Array2::<f32>::zeros((1, hop)), Array2::<f32>::zeros((1, hop)));
        let mut x = 9u32;
        let mut low = 0f32;
        let (mut pauses, mut words) = (Vec::new(), Vec::new());
        let n = voice.len() / hop;
        let mut quiet = Vec::new();
        for k in 0..n {
            let v = &voice[k * hop..(k + 1) * hop];
            quiet.push(v.iter().map(|s| s * s * 0.09).sum::<f32>() / hop as f32 <= 1e-8);
            for (i, o) in noisy.iter_mut().enumerate() {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let w = (x >> 8) as f32 / (1u32 << 24) as f32;
                low += 0.05 * (w - low);
                *o = 0.3 * v[i] + 0.06 * (0.5 * w + 2.0 * low);
            }
            let lsnr = d.process(noisy.view(), enh.view_mut()).unwrap();
            // The estimate is of the frame `la` back.
            if k >= la {
                if quiet[k - la] { pauses.push(lsnr) } else { words.push(lsnr) }
            }
        }
        let pct = |v: &mut Vec<f32>, p: usize| {
            v.sort_by(f32::total_cmp);
            v[v.len() * p / 100]
        };
        let gated = pauses.iter().filter(|l| **l < -10.0).count() as f32 / pauses.len() as f32;
        eprintln!(
            "{}: pauses' lsnr 10th {:.1}, median {:.1}, 90th {:.1} ({:.0}% gated); words' 10th {:.1}, median {:.1}",
            path.rsplit('/').next().unwrap(),
            pct(&mut pauses, 10),
            pct(&mut pauses, 50),
            pct(&mut pauses, 90),
            gated * 100.0,
            pct(&mut words, 10),
            pct(&mut words, 50)
        );
    }
}
