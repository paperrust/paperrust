//! HEIC/HEIF (iPhone photos) → PNG with craft-heif (child process only, feature `heif`).
//!
//! upstream: src/paperless/parsers/tesseract.py@v3.4.2 (`image/heic` in `is_image`, Pillow with
//! `pillow_heif.register_heif_opener()`; Pillow/OCRmyPDF read the photo like any other image).
//!
//! craft-heif (https://github.com/storytold/craft-libs, MIT OR Apache-2.0) uses heic-rs (pure
//! Rust, no `unsafe`, no C) for container and HEVC and converts the colours like libheif. The
//! rotation/mirroring/crop from the container (`irot`/`imir`/`clap`) are applied –
//! like pillow_heif, which also applies the transformations on open. The ICC profile is carried over into
//! the PNG. heic-rs is young: therefore this only runs in `paperrust-pdftool` (own
//! process with time and memory limit; craft-heif catches panics itself).

use crate::job::ConvertReport;

/// Domain error with user text (without paths).
#[derive(Debug)]
pub struct HeifError(pub String);

/// Maximum size of an input file.
pub const MAX_INPUT: u64 = 256 * 1024 * 1024;

/// Default limit in pixels (larger than any phone photo; 48 MP ≈ 48,000,000).
pub const DEFAULT_MAX_PIXELS: u64 = 178_956_970;

/// 16-bit value (full range) → 8 bit, rounded.
fn to8(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32_767) / 65_535) as u8
}

/// HEIC bytes → PNG bytes (RGB or RGBA, 8 bit) and image size.
pub fn heic_to_png_bytes(bytes: &[u8], max_pixels: u64) -> Result<(Vec<u8>, u32, u32), HeifError> {
    use image::ImageEncoder;
    let max_pixels = if max_pixels == 0 { DEFAULT_MAX_PIXELS } else { max_pixels };
    let opts = craft_heif::Options { max_pixels, ..Default::default() };
    let d = craft_heif::decode(bytes, &opts).map_err(|e| HeifError(format!("The HEIC image could not be decoded: {e}")))?;
    let channels = if d.has_alpha { 4usize } else { 3 };
    let expected = (d.width as usize).saturating_mul(d.height as usize).saturating_mul(channels);
    if d.width == 0 || d.height == 0 || d.data.len() < expected {
        return Err(HeifError("The HEIC image could not be decoded: incomplete picture".into()));
    }
    let px: Vec<u8> = d.data[..expected].iter().map(|v| to8(*v)).collect();
    let mut out = Vec::new();
    let mut enc = image::codecs::png::PngEncoder::new(&mut out);
    if let Some(icc) = d.icc.clone() {
        // An invalid profile is no reason to abort (colours then without a profile).
        let _ = enc.set_icc_profile(icc);
    }
    let ct = if d.has_alpha { image::ExtendedColorType::Rgba8 } else { image::ExtendedColorType::Rgb8 };
    enc.write_image(&px, d.width, d.height, ct).map_err(|e| HeifError(format!("The PNG could not be written: {e}")))?;
    Ok((out, d.width, d.height))
}

/// `Op::Heic`: `input` (HEIC) to `output` (PNG).
pub fn convert(input: &str, output: &str, max_pixels: u64) -> Result<ConvertReport, HeifError> {
    let meta = std::fs::metadata(input).map_err(|_| HeifError("The file could not be read.".into()))?;
    if meta.len() > MAX_INPUT {
        return Err(HeifError("The file is too large to convert.".into()));
    }
    let bytes = std::fs::read(input).map_err(|_| HeifError("The file could not be read.".into()))?;
    let (png, _, _) = heic_to_png_bytes(&bytes, max_pixels)?;
    drop(bytes);
    std::fs::write(output, &png).map_err(|_| HeifError("The result could not be written.".into()))?;
    Ok(ConvertReport { format: "heic".into(), size_before: meta.len(), size_after: png.len() as u64 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_bit() {
        assert_eq!(to8(0), 0);
        assert_eq!(to8(65_535), 255);
        assert_eq!(to8(32_896), 128);
    }

    /// Synthetic HEIF 64×32, container rotates by 90° → PNG 32×64 (like pillow_heif).
    #[test]
    fn decodes_and_applies_rotation() {
        use craft_heif::testdata::{Prop, Spec, build};
        let grey = |_x: u32, _y: u32| [200u16, 128, 128];
        let bytes = build(&Spec { props: vec![Prop::Nclx(1, 13, 6, true), Prop::Irot(1)], ..Spec::new(64, 32, &grey) });
        let (png, w, h) = heic_to_png_bytes(&bytes, 0).unwrap();
        assert_eq!((w, h), (32, 64));
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (32, 64));
        let p = img.to_rgb8().get_pixel(5, 5).0;
        assert!(p.iter().all(|c| c.abs_diff(200) <= 2), "{p:?}");
        // the pixel limit applies before decoding
        assert!(heic_to_png_bytes(&bytes, 100).is_err());
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(heic_to_png_bytes(b"not a heic file at all", 0).is_err());
        assert!(heic_to_png_bytes(&[], 0).is_err());
    }
}
