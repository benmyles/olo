use std::{fs::File, io::BufReader, path::Path};

use png::{BitDepth, ColorType, Decoder, Limits, Reader, Transformations};
use serde::Serialize;

use crate::{Result, require};

#[derive(Debug, Serialize)]
pub struct PixelDifference {
    pub pixels: u64,
    pub changed_pixels: u64,
    pub changed_fraction: f64,
    pub mean_channel_delta: f64,
    pub maximum_channel_delta: u8,
    pub reference_non_background_pixels: u64,
    pub candidate_non_background_pixels: u64,
}

impl PixelDifference {
    pub fn passes(&self) -> bool {
        self.pixels > 0 && self.changed_fraction <= 0.0001 && self.mean_channel_delta <= 0.05 && self.reference_non_background_pixels >= 64 && self.candidate_non_background_pixels >= 64
    }
}

fn reader(path: &Path, expected: (u32, u32)) -> Result<Reader<BufReader<File>>> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    require(file.metadata().map_err(|e| e.to_string())?.len() <= 64 * 1024 * 1024, "PNG exceeds file limit")?;
    let mut decoder = Decoder::new_with_limits(BufReader::new(file), Limits { bytes: 16 * 1024 * 1024 });
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let reader = decoder.read_info().map_err(|e| e.to_string())?;
    let info = reader.info();
    require((info.width, info.height) == expected, "PNG dimensions do not match viewport and scale")?;
    require(!info.interlaced && info.animation_control.is_none(), "capture must be a static noninterlaced PNG")?;
    require(reader.output_color_type().1 == BitDepth::Eight, "capture must decode to 8-bit channels")?;
    Ok(reader)
}

fn rgba(row: &[u8], color: ColorType, x: usize) -> [u8; 4] {
    match color {
        ColorType::Grayscale => [row[x], row[x], row[x], 255],
        ColorType::GrayscaleAlpha => [row[x * 2], row[x * 2], row[x * 2], row[x * 2 + 1]],
        ColorType::Rgb => [row[x * 3], row[x * 3 + 1], row[x * 3 + 2], 255],
        ColorType::Rgba => [row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]],
        ColorType::Indexed => unreachable!("palette expanded by decoder"),
    }
}

/// Stream pairs of scanlines: memory follows width, not screenshot area.
/// Captures must be opaque; missing/blank/transparent images cannot pass.
pub fn compare_png(reference: &Path, candidate: &Path, expected: (u32, u32)) -> Result<PixelDifference> {
    require(expected.0 > 0 && expected.1 > 0 && expected.0 <= 4096 && expected.1 <= 4096, "invalid capture dimensions")?;
    let mut r = reader(reference, expected)?;
    let mut c = reader(candidate, expected)?;
    let rc = r.output_color_type().0;
    let cc = c.output_color_type().0;
    let mut changed_pixels = 0;
    let mut total_delta = 0u64;
    let mut maximum_channel_delta = 0;
    let mut reference_non_background_pixels = 0;
    let mut candidate_non_background_pixels = 0;
    let mut background = None;
    for _ in 0..expected.1 {
        let r_row = r.next_row().map_err(|e| e.to_string())?.ok_or("reference PNG is truncated")?;
        let c_row = c.next_row().map_err(|e| e.to_string())?.ok_or("candidate PNG is truncated")?;
        for x in 0..expected.0 as usize {
            let a = rgba(r_row.data(), rc, x);
            let b = rgba(c_row.data(), cc, x);
            require(a[3] == 255 && b[3] == 255, "screenshot contains transparent pixels")?;
            let (r_background, c_background) = *background.get_or_insert((a, b));
            reference_non_background_pixels += u64::from(a.iter().zip(r_background).any(|(a, b)| a.abs_diff(b) > 8));
            candidate_non_background_pixels += u64::from(b.iter().zip(c_background).any(|(a, b)| a.abs_diff(b) > 8));
            let mut delta = 0;
            for (a, b) in a[..3].iter().zip(b) {
                let d = a.abs_diff(b);
                delta = delta.max(d);
                total_delta += d as u64;
            }
            changed_pixels += u64::from(delta > 8);
            maximum_channel_delta = maximum_channel_delta.max(delta);
        }
    }
    r.finish().map_err(|e| e.to_string())?;
    c.finish().map_err(|e| e.to_string())?;
    let pixels = expected.0 as u64 * expected.1 as u64;
    Ok(PixelDifference {
        pixels,
        changed_pixels,
        changed_fraction: changed_pixels as f64 / pixels as f64,
        mean_channel_delta: total_delta as f64 / (pixels * 3) as f64,
        maximum_channel_delta,
        reference_non_background_pixels,
        candidate_non_background_pixels,
    })
}
