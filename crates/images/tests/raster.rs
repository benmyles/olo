use image::{
    DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, RgbaImage,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
};
use olo_images::raster::{self, Error, Limits};
use std::{io::Cursor, sync::atomic::AtomicBool};

fn pixels() -> RgbaImage {
    RgbaImage::from_raw(2, 3, vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255, 0, 255, 255, 255, 255, 0, 255, 255]).unwrap()
}
fn encoded(format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(pixels()).write_to(&mut out, format).unwrap();
    out.into_inner()
}
#[test]
fn bounded_bytes_decode_lossless_formats_without_files() {
    // ICO and BMP too: the web's favicons (held to the same limits, checked
    // on the header before any pixels are decoded).
    for format in [ImageFormat::Png, ImageFormat::WebP, ImageFormat::Gif, ImageFormat::Ico, ImageFormat::Bmp] {
        let data = encoded(format);
        let raster = raster::decode(&data, Default::default(), None).unwrap();
        assert_eq!(raster.format, format);
        assert_eq!(raster.pixels, pixels(), "{format:?}");
    }
}
#[test]
fn encoded_dimensions_pixels_and_surfaces_fail_closed() {
    let data = encoded(ImageFormat::Png);
    for limits in [
        Limits { encoded_bytes: data.len() - 1, ..Default::default() },
        Limits { dimension: 2, ..Default::default() },
        Limits { pixels: 5, ..Default::default() },
        Limits { surface_bytes: 23, ..Default::default() },
        Limits { codec_bytes: 0, ..Default::default() },
    ] {
        assert!(matches!(raster::decode(&data, limits, None), Err(Error::Limit)), "{limits:?}");
        // An icon or bitmap over the limits is refused the same way (its
        // own size aside: theirs differ from the PNG's). Their codecs take no
        // allocation cap, but allocate only for the dimensions, which are
        // held to the pixel and surface limits before anything is decoded.
        for format in [ImageFormat::Ico, ImageFormat::Bmp].into_iter().filter(|_| limits.encoded_bytes != data.len() - 1 && limits.codec_bytes != 0) {
            assert!(matches!(raster::decode(&encoded(format), Limits { encoded_bytes: usize::MAX, ..limits }, None), Err(Error::Limit) | Err(Error::Decode(_))), "{format:?} {limits:?}");
        }
    }
    assert!(raster::decode(&data, Limits { encoded_bytes: data.len(), dimension: 3, pixels: 6, surface_bytes: 24, ..Default::default() }, None).is_ok());
    // Formats no browser shows stay out.
    for format in [ImageFormat::Tiff] {
        assert!(matches!(raster::decode(&encoded(format), Default::default(), None), Err(Error::Unsupported)));
    }
    let cancel = AtomicBool::new(true);
    assert!(matches!(raster::decode(&data, Default::default(), Some(&cancel)), Err(Error::Cancelled)));
    assert!(matches!(raster::decode(&[], Default::default(), Some(&cancel)), Err(Error::Cancelled)));
    assert!(matches!(raster::decode(&[], Default::default(), None), Err(Error::Unsupported)));
    assert!(matches!(raster::decode(b"II*\0\0\0\0\0", Default::default(), None), Err(Error::Unsupported)));
    assert!(matches!(raster::decode(&data[..24], Default::default(), None), Err(Error::Decode(_))));
}
#[test]
fn jpeg_exif_rotates_and_png_profile_converts_like_native_files() {
    let mut jpeg = Vec::new();
    // Little-endian TIFF IFD0: one SHORT orientation entry, value 6 (90 CW).
    let exif = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 100);
    encoder.set_exif_metadata(exif.to_vec()).unwrap();
    encoder.write_image(&DynamicImage::ImageRgba8(pixels()).into_rgb8(), 2, 3, ExtendedColorType::Rgb8).unwrap();
    let raster = raster::decode(&jpeg, Default::default(), None).unwrap();
    assert_eq!(raster.format, ImageFormat::Jpeg);
    assert_eq!(raster.pixels.dimensions(), (3, 2));
    let unrotated = image::load_from_memory(&jpeg).unwrap().into_rgba8();
    assert_eq!(raster.pixels, image::imageops::rotate90(&unrotated));

    let original = [200u8, 100, 50, 127];
    let profile = moxcms::ColorProfile::new_display_p3().encode().unwrap();
    let mut png = Vec::new();
    let mut encoder = PngEncoder::new(&mut png);
    encoder.set_icc_profile(profile.clone()).unwrap();
    encoder.write_image(&original, 1, 1, ExtendedColorType::Rgba8).unwrap();
    let raster = raster::decode(&png, Default::default(), None).unwrap();
    let expected = olo_images::to_srgb(DynamicImage::ImageRgba8(RgbaImage::from_raw(1, 1, original.to_vec()).unwrap()), Some(&profile)).into_rgba8();
    assert_eq!(raster.pixels, expected);
    assert_eq!(raster.pixels.get_pixel(0, 0)[3], 127);
    assert_ne!(&raster.pixels.as_raw()[..3], &original[..3]);
    assert!(matches!(raster::decode(&png, Limits { profile_bytes: profile.len() - 1, ..Default::default() }, None), Err(Error::Limit)));
}
#[test]
fn sixteen_bit_sources_check_native_surface_before_conversion() {
    let mut data = Vec::new();
    let values = [0u16, u16::MAX, 32768, u16::MAX];
    let bytes: Vec<_> = values.into_iter().flat_map(u16::to_ne_bytes).collect();
    PngEncoder::new(&mut data).write_image(&bytes, 1, 1, ExtendedColorType::Rgba16).unwrap();
    assert!(matches!(raster::decode(&data, Limits { surface_bytes: 4, ..Default::default() }, None), Err(Error::Limit)));
    let raster = raster::decode(&data, Limits { surface_bytes: 8, ..Default::default() }, None).unwrap();
    assert_eq!(raster.pixels.as_raw(), &[0, 255, 128, 255]);
}

#[test]
fn oversized_png_headers_fail_before_the_incompatible_pixel_body() {
    // Retain a tiny pixel body and replace only valid IHDR dimensions/CRC.
    // These must be quota errors before the decoder tries to inflate pixels.
    // CRCs authored independently with Python zlib.crc32 over IHDR's bytes.
    for (w, h, crc) in [(32769u32, 1u32, 0x68f4f116u32), (16384, 16384, 0xa9c81084)] {
        let mut data = encoded(ImageFormat::Png);
        assert_eq!(&data[24..29], &[8, 6, 0, 0, 0]);
        data[16..20].copy_from_slice(&w.to_be_bytes());
        data[20..24].copy_from_slice(&h.to_be_bytes());
        data[29..33].copy_from_slice(&crc.to_be_bytes());
        assert!(matches!(raster::decode(&data, Default::default(), None), Err(Error::Limit)));
    }
}
