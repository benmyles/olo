//! Reading pictures for thumbnails, Quick Look and the like: everything the
//! `image` crate reads, plus JPEG XL (what Fedora ships its backgrounds
//! as), the right way up and within a memory limit.

use std::{io::Read, path::Path};

use image::{DynamicImage, ImageDecoder, ImageReader};

/// JPEG XL by its signature: a bare codestream or the ISO container.
pub fn is_jxl(head: &[u8]) -> bool {
    head.starts_with(&[0xff, 0x0a]) || head.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n")
}

/// Decode `path`, returning the picture and its format's name ("PNG").
pub fn open(path: &Path) -> Result<(DynamicImage, String), String> {
    let mut head = [0u8; 12];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).map_err(|e| e.to_string())?;
    if is_jxl(&head[..n]) {
        let file = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
        let decoder = jxl_oxide::integration::JxlDecoder::new(file).map_err(|e| e.to_string())?;
        let img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
        return Ok((img, "JPEG XL".into()));
    }
    let mut reader = ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    let format = reader.format().map(|f| format!("{f:?}").to_uppercase()).unwrap_or_else(|| "Image".into());
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok((img, format))
}
