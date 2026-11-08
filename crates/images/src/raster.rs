//! Byte-backed PNG/JPEG/WebP/GIF still decoding on a worker.
//! Input length, dimensions, pixel count and individual surfaces have hard
//! bounds. The codec's allocation limit is additionally set, but upstream
//! documents it as best effort: this is not a process memory sandbox.
//! Cancellation is checked at reads and stage boundaries; codec CPU work
//! between reads and colour conversion cannot yet be interrupted.
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use std::{
    io::{self, BufRead, Cursor, Read, Seek, SeekFrom},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub encoded_bytes: usize,
    pub pixels: u64,
    pub dimension: u32,
    /// Maximum size of either a decoded surface or the final RGBA8 surface.
    pub surface_bytes: u64,
    /// Best-effort codec working/output allocation limit, excluding this API's
    /// orientation/colour/output conversion buffers and the borrowed input.
    pub codec_bytes: u64,
    pub profile_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { encoded_bytes: 16 << 20, pixels: 16 << 20, dimension: 32768, surface_bytes: 64 << 20, codec_bytes: 128 << 20, profile_bytes: 4 << 20 }
    }
}
#[derive(Debug)]
pub enum Error {
    Limit,
    Cancelled,
    Unsupported,
    Decode(image::ImageError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "raster decode {self:?}")
    }
}
impl std::error::Error for Error {}
/// An oriented sRGB still, in straight-alpha RGBA8. Animated resources require
/// a separate frame/timing implementation; this exposes only the codec's still.
pub struct Raster {
    pub pixels: RgbaImage,
    pub format: ImageFormat,
}
fn check(cancel: Option<&AtomicBool>) -> Result<(), Error> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
fn decoded<T>(result: image::ImageResult<T>, cancel: Option<&AtomicBool>) -> Result<T, Error> {
    check(cancel)?;
    result.map_err(|e| if matches!(e, image::ImageError::Limits(_)) { Error::Limit } else { Error::Decode(e) })
}
/// Decode borrowed bytes without a temporary file. This API makes no encoded
/// input clone of its own; individual codecs may buffer/copy encoded bytes.
/// Run on a worker. EXIF orientation and ICC conversion follow `open`'s policy.
/// BMP/ICO/TIFF, JPEG XL, AVIF, HEIC and vector images are not yet supported by this bounded
/// path; they must not silently fall through to the file API's larger limits.
pub fn decode(data: &[u8], limits: Limits, cancel: Option<&AtomicBool>) -> Result<Raster, Error> {
    let result = decode_with_codec(data, limits, cancel);
    // A GIF with no colour table at all (the 1 × 1 placeholder pages swap
    // in while they load pictures lazily): its pixels can only be
    // transparent, which is how Chrome shows it; the codec refuses it.
    if let Err(Error::Decode(e)) = &result
        && data.starts_with(b"GIF8")
        && data.len() >= 10
        && e.to_string().contains("color table")
    {
        let (w, h) = (u32::from(u16::from_le_bytes([data[6], data[7]])), u32::from(u16::from_le_bytes([data[8], data[9]])));
        if w > 0 && h > 0 && w <= limits.dimension && h <= limits.dimension && u64::from(w) * u64::from(h) <= limits.pixels {
            return Ok(Raster { pixels: image::RgbaImage::new(w, h), format: ImageFormat::Gif });
        }
    }
    result
}
fn decode_with_codec(data: &[u8], limits: Limits, cancel: Option<&AtomicBool>) -> Result<Raster, Error> {
    check(cancel)?;
    if data.len() > limits.encoded_bytes {
        return Err(Error::Limit);
    }
    let format = image::guess_format(data).map_err(|_| Error::Unsupported)?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif) {
        return Err(Error::Unsupported);
    }
    let mut reader = ImageReader::with_format(Input { cursor: Cursor::new(data), cancel }, format);
    let mut codec = image::Limits::default();
    codec.max_image_width = Some(limits.dimension);
    codec.max_image_height = Some(limits.dimension);
    codec.max_alloc = Some(limits.codec_bytes);
    reader.limits(codec);
    let mut decoder = decoded(reader.into_decoder(), cancel)?;
    let (w, h) = decoder.dimensions();
    let area = u64::from(w) * u64::from(h);
    let output = area.checked_mul(4).ok_or(Error::Limit)?;
    if w == 0 || h == 0 || w > limits.dimension || h > limits.dimension || area > limits.pixels || output > limits.surface_bytes || decoder.total_bytes() > limits.surface_bytes {
        return Err(Error::Limit);
    }
    let orientation = decoder.orientation().ok();
    check(cancel)?;
    let profile = decoder.icc_profile().ok().flatten();
    check(cancel)?;
    if profile.as_ref().is_some_and(|p| p.len() > limits.profile_bytes) {
        return Err(Error::Limit);
    }
    if output > isize::MAX as u64 || decoder.total_bytes() > isize::MAX as u64 {
        return Err(Error::Limit);
    }
    let mut image = decoded(DynamicImage::from_decoder(decoder), cancel)?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    check(cancel)?;
    let image = crate::to_srgb(image, profile.as_deref());
    check(cancel)?;
    let pixels = image.into_rgba8();
    check(cancel)?;
    Ok(Raster { pixels, format })
}
struct Input<'a> {
    cursor: Cursor<&'a [u8]>,
    cancel: Option<&'a AtomicBool>,
}
impl Input<'_> {
    fn ready(&self) -> io::Result<()> {
        check(self.cancel).map_err(|_| io::Error::other("raster decode cancelled"))
    }
}
impl Read for Input<'_> {
    fn read_to_end(&mut self, output: &mut Vec<u8>) -> io::Result<usize> {
        self.ready()?;
        let before = output.len();
        let remaining = self.cursor.fill_buf()?.len();
        output.try_reserve_exact(remaining).map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        // JPEG buffers its input. Reserve its known borrowed length once,
        // avoiding geometric growth while retaining cooperative checks.
        loop {
            self.ready()?;
            let bytes = self.cursor.fill_buf()?;
            let n = bytes.len().min(32 << 10);
            if n == 0 {
                break;
            }
            output.extend_from_slice(&bytes[..n]);
            self.cursor.consume(n);
        }
        Ok(output.len() - before)
    }
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.ready()?;
        // A large read must not consume the whole input between checks.
        let n = output.len().min(32 << 10);
        self.cursor.read(&mut output[..n])
    }
}
impl BufRead for Input<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.ready()?;
        let bytes = self.cursor.fill_buf()?;
        Ok(&bytes[..bytes.len().min(32 << 10)])
    }
    fn consume(&mut self, amount: usize) {
        self.cursor.consume(amount);
    }
}
impl Seek for Input<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.ready()?;
        self.cursor.seek(pos)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_gif_without_a_colour_table_is_transparent() {
        // The placeholder apple.com swaps in while it loads its pictures.
        let gif = [71u8, 73, 70, 56, 57, 97, 1, 0, 1, 0, 112, 0, 0, 33, 249, 4, 1, 0, 0, 0, 0, 44, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 68, 1, 0, 59];
        let r = super::decode(&gif, Default::default(), None).unwrap();
        assert_eq!((r.pixels.width(), r.pixels.height()), (1, 1));
        assert_eq!(r.pixels.as_raw().as_slice(), &[0, 0, 0, 0]);
    }
    use super::*;
    #[test]
    fn borrowed_reader_checks_cancellation_between_bounded_reads() {
        let bytes = vec![0; 128 << 10];
        let cancelled = AtomicBool::new(false);
        let mut input = Input { cursor: Cursor::new(bytes.as_slice()), cancel: Some(&cancelled) };
        let mut output = vec![0; bytes.len()];
        assert_eq!(input.read(&mut output).unwrap(), 32 << 10);
        assert_eq!(input.fill_buf().unwrap().len(), 32 << 10);
        cancelled.store(true, Ordering::Relaxed);
        assert!(input.read(&mut output).is_err());
        assert!(input.fill_buf().is_err());
        assert!(input.seek(SeekFrom::Start(0)).is_err());
        let mut appended = vec![7; 4];
        assert!(input.read_to_end(&mut appended).is_err());
        assert_eq!(appended, [7; 4]);
        cancelled.store(false, Ordering::Relaxed);
        input.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(input.read_to_end(&mut appended).unwrap(), bytes.len());
        assert_eq!(&appended[..4], &[7; 4]);
        assert_eq!(&appended[4..], bytes.as_slice());
    }
}
