//! Same-process-code control for bounded byte decoding, excluding file/network I/O.
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use olo_images::raster;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{hint::black_box, io::Cursor, sync::atomic::AtomicBool, time::Instant};
fn direct(data: &[u8]) -> RgbaImage {
    let mut reader = ImageReader::new(Cursor::new(data)).with_guessed_format().unwrap();
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().unwrap();
    let orientation = decoder.orientation().ok();
    let profile = decoder.icc_profile().ok().flatten();
    let mut image = DynamicImage::from_decoder(decoder).unwrap();
    if let Some(o) = orientation {
        image.apply_orientation(o);
    }
    olo_images::to_srgb(image, profile.as_deref()).into_rgba8()
}
fn main() {
    let mut args = std::env::args().skip(1);
    let rounds: usize = args.next().unwrap_or_else(|| "10".into()).parse().unwrap();
    let mode = args.next().unwrap_or_else(|| "bounded".into());
    assert!(rounds > 0 && matches!(mode.as_str(), "direct" | "bounded"));
    let cancel = AtomicBool::new(false);
    let mut rows = Vec::new();
    for (w, h) in [(256, 256), (1920, 1080)] {
        let pixels = RgbaImage::from_fn(w, h, |x, y| image::Rgba([[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]][((x / 17 + y / 13) % 4) as usize]));
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP, ImageFormat::Gif] {
            let mut encoded = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(pixels.clone()).write_to(&mut encoded, format).unwrap();
            let data = encoded.into_inner();
            let run = || if mode == "direct" { direct(&data) } else { raster::decode(&data, Default::default(), Some(&cancel)).unwrap().pixels };
            let warm = run();
            let start = Instant::now();
            let mut worst = 0.;
            for _ in 0..rounds {
                let at = Instant::now();
                black_box(run());
                worst = f64::max(worst, at.elapsed().as_secs_f64() * 1e3);
            }
            rows.push(json!({"format":format!("{format:?}"), "width":w, "height":h, "encoded_bytes":data.len(),
                "encoded_sha256":format!("{:x}", Sha256::digest(&data)), "pixels_sha256":format!("{:x}", Sha256::digest(warm.as_raw())),
                "decode_ms":start.elapsed().as_secs_f64()*1e3/rounds as f64, "worst_decode_ms":worst}));
        }
    }
    println!(
        "{}",
        json!({"scope":"Authored four-colour stills, direct existing codec policy versus bounded worker with a real cancellation flag. Same binary; timings include allocation/conversion/drop, exclude fixture encoding, file/network I/O, rendering and animation. Peak process RSS also includes fixture encoding.", "rounds":rounds, "mode":mode,"rows":rows})
    );
}
