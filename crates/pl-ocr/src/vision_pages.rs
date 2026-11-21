//! PaperRust: page images for AI text recognition (vision OCR, pl-core `vision`).
//!
//! PDF: every page rasterized with the renderer (PDFium) at `dpi` (default 170, capped to
//! [`MAX_PIXELS`]) and encoded as PNG; too large → JPEG (quality 85, if necessary 70 and
//! downscaled). Images: PNG/JPEG/WebP up to `max_bytes` unchanged, other formats (TIFF, BMP, GIF)
//! or too large images: first image decoded and re-encoded. At most `max_pages` pages.

use std::io::Cursor;
use std::path::Path;

use crate::render::{PdfRenderer, fit_dpi};
use crate::{OcrError, Result};

/// Largest pixel count per page (≈ A4 at 200 dpi × 1.5).
pub const MAX_PIXELS: u64 = 6_000_000;

/// A page image `(mime, bytes)`.
pub type PageImage = (String, Vec<u8>);

/// Page images of a file (PDF or image). Other types → empty.
pub fn page_images(
    renderer: &dyn PdfRenderer,
    path: &Path,
    mime: &str,
    max_pages: usize,
    dpi: f64,
    max_bytes: usize,
) -> Result<Vec<PageImage>> {
    if max_pages == 0 {
        return Ok(Vec::new());
    }
    if mime == "application/pdf" {
        let bytes = std::fs::read(path)?;
        let doc = renderer.open(&bytes)?;
        let n = doc.page_count().min(max_pages);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let d = match doc.page_size_pt(i) {
                Ok((w, h)) => fit_dpi(w, h, dpi, MAX_PIXELS),
                Err(_) => dpi,
            };
            let raster = doc.render(i, d, false)?;
            let img = raster.to_dynamic().ok_or_else(|| OcrError::Other("page raster has an invalid size".into()))?;
            out.push(encode(&img, max_bytes)?);
        }
        return Ok(out);
    }
    if !mime.starts_with("image/") {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(path)?;
    if matches!(mime, "image/png" | "image/jpeg" | "image/webp") && bytes.len() <= max_bytes {
        return Ok(vec![(mime.to_string(), bytes)]);
    }
    let img = image::load_from_memory(&bytes).map_err(|e| OcrError::Other(format!("image decode failed: {e}")))?;
    Ok(vec![encode(&img, max_bytes)?])
}

/// PNG, otherwise JPEG 85/70, otherwise halve and try again.
fn encode(img: &image::DynamicImage, max_bytes: usize) -> Result<PageImage> {
    let mut img = std::borrow::Cow::Borrowed(img);
    for _ in 0..4 {
        let png = write(&img, image::ImageFormat::Png)?;
        if png.len() <= max_bytes {
            return Ok(("image/png".into(), png));
        }
        for q in [85u8, 70] {
            let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8());
            let mut buf = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q)
                .encode_image(&rgb)
                .map_err(|e| OcrError::Other(format!("JPEG encode failed: {e}")))?;
            if buf.len() <= max_bytes {
                return Ok(("image/jpeg".into(), buf));
            }
        }
        let (w, h) = (img.width() / 2, img.height() / 2);
        if w == 0 || h == 0 {
            break;
        }
        img = std::borrow::Cow::Owned(img.resize(w, h, image::imageops::FilterType::Triangle));
    }
    Err(OcrError::Other("page image is too large".into()))
}

fn write(img: &image::DynamicImage, fmt: image::ImageFormat) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, fmt).map_err(|e| OcrError::Other(format!("image encode failed: {e}")))?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::UnavailableRenderer;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 7 % 256) as u8, (y * 13 % 256) as u8, ((x ^ y) % 256) as u8])
        }));
        write(&img, image::ImageFormat::Png).unwrap()
    }

    #[test]
    fn images_pass_through_or_shrink() {
        let dir = tempfile::tempdir().unwrap();
        let r = UnavailableRenderer { reason: "test".into() };
        let p = dir.path().join("a.png");
        std::fs::write(&p, png(64, 64)).unwrap();
        let out = page_images(&r, &p, "image/png", 5, 170.0, 1 << 20).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "image/png");
        // too large → re-encoded and smaller
        let big = png(600, 600);
        std::fs::write(&p, &big).unwrap();
        let out = page_images(&r, &p, "image/png", 5, 170.0, big.len() / 4).unwrap();
        assert!(out[0].1.len() <= big.len() / 4);
        // BMP → PNG
        let b = dir.path().join("a.bmp");
        image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8)).save(&b).unwrap();
        let out = page_images(&r, &b, "image/bmp", 5, 170.0, 1 << 20).unwrap();
        assert_eq!(out[0].0, "image/png");
        // no image, no pages
        assert!(page_images(&r, &p, "text/plain", 5, 170.0, 1 << 20).unwrap().is_empty());
        assert!(page_images(&r, &p, "image/png", 0, 170.0, 1 << 20).unwrap().is_empty());
        // PDF without renderer → error
        let pdf = dir.path().join("a.pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        assert!(page_images(&r, &pdf, "application/pdf", 5, 170.0, 1 << 20).is_err());
    }
}
