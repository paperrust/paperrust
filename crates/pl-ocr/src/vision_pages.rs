//! PaperRust: page images for AI text recognition (vision OCR, pl-core `vision`).
//!
//! PDF: every page rasterized with the renderer (PDFium) at `dpi` (default 170, capped to
//! [`MAX_PIXELS`]), downscaled to a long side of at most `max_side` pixels (default 1540, see
//! pl-core `vision::DEFAULT_MAX_SIDE`; `0` = no downscaling) and encoded as PNG; too large → JPEG
//! (quality 85, if necessary 70 and downscaled). Images: PNG/JPEG/WebP within `max_bytes` and
//! `max_side` unchanged, other formats (TIFF, BMP, GIF) or too large images: first image decoded,
//! downscaled and re-encoded. At most `max_pages` pages.
//!
//! [`page_rasters`] returns decoded rasters of selected pages (line crops, pl-ocr `uncertain`).

use std::io::Cursor;
use std::path::Path;

use crate::render::{PdfRenderer, fit_dpi};
use crate::{OcrError, Result};

/// Largest pixel count per page (≈ A4 at 200 dpi × 1.5).
pub const MAX_PIXELS: u64 = 6_000_000;

/// A page image `(mime, bytes)`.
pub type PageImage = (String, Vec<u8>);

/// Page images of a file (PDF or image) without downscaling (see [`page_images_sized`]).
pub fn page_images(
    renderer: &dyn PdfRenderer,
    path: &Path,
    mime: &str,
    max_pages: usize,
    dpi: f64,
    max_bytes: usize,
) -> Result<Vec<PageImage>> {
    page_images_sized(renderer, path, mime, max_pages, dpi, max_bytes, 0)
}

/// Page images of a file (PDF or image), long side at most `max_side` px (`0` = unlimited).
/// Other types → empty.
pub fn page_images_sized(
    renderer: &dyn PdfRenderer,
    path: &Path,
    mime: &str,
    max_pages: usize,
    dpi: f64,
    max_bytes: usize,
    max_side: u32,
) -> Result<Vec<PageImage>> {
    if max_pages == 0 {
        return Ok(Vec::new());
    }
    if mime == "application/pdf" {
        let pages: Vec<usize> = (1..=max_pages).collect();
        return page_rasters(renderer, path, mime, &pages, dpi)?
            .into_iter()
            .map(|(_, img)| encode(&downscale(img, max_side), max_bytes))
            .collect();
    }
    if !mime.starts_with("image/") {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(path)?;
    if matches!(mime, "image/png" | "image/jpeg" | "image/webp") && bytes.len() <= max_bytes && !needs_downscale(&bytes, max_side) {
        return Ok(vec![(mime.to_string(), bytes)]);
    }
    let img = image::load_from_memory(&bytes).map_err(|e| OcrError::Other(format!("image decode failed: {e}")))?;
    Ok(vec![encode(&downscale(img, max_side), max_bytes)?])
}

/// Decoded rasters of the given 1-based `pages` (PDF: rendered at `dpi`, capped to [`MAX_PIXELS`];
/// images: page 1 = the first image). Pages beyond the document are left out.
pub fn page_rasters(
    renderer: &dyn PdfRenderer,
    path: &Path,
    mime: &str,
    pages: &[usize],
    dpi: f64,
) -> Result<Vec<(usize, image::DynamicImage)>> {
    if pages.is_empty() {
        return Ok(Vec::new());
    }
    if mime == "application/pdf" {
        let bytes = std::fs::read(path)?;
        let doc = renderer.open(&bytes)?;
        let n = doc.page_count();
        let mut out = Vec::with_capacity(pages.len());
        for &p in pages {
            if p == 0 || p > n {
                continue;
            }
            let i = p - 1;
            let d = match doc.page_size_pt(i) {
                Ok((w, h)) => fit_dpi(w, h, dpi, MAX_PIXELS),
                Err(_) => dpi,
            };
            let raster = doc.render(i, d, false)?;
            let img = raster.to_dynamic().ok_or_else(|| OcrError::Other("page raster has an invalid size".into()))?;
            out.push((p, img));
        }
        return Ok(out);
    }
    if !mime.starts_with("image/") || !pages.contains(&1) {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(path)?;
    let img = image::load_from_memory(&bytes).map_err(|e| OcrError::Other(format!("image decode failed: {e}")))?;
    Ok(vec![(1, img)])
}

/// Downscale so that the long side is at most `max_side` (`0` = unchanged; never upscales).
pub fn downscale(img: image::DynamicImage, max_side: u32) -> image::DynamicImage {
    let long = img.width().max(img.height());
    if max_side == 0 || long <= max_side {
        return img;
    }
    let f = f64::from(max_side) / f64::from(long);
    let w = ((f64::from(img.width()) * f).round() as u32).max(1);
    let h = ((f64::from(img.height()) * f).round() as u32).max(1);
    img.resize_exact(w, h, image::imageops::FilterType::Triangle)
}

/// Whether an encoded image is larger than `max_side` (header only; unknown size → `false`).
fn needs_downscale(bytes: &[u8], max_side: u32) -> bool {
    if max_side == 0 {
        return false;
    }
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_some_and(|(w, h)| w.max(h) > max_side)
}

/// PNG, otherwise JPEG 85/70, otherwise halve and try again.
pub fn encode(img: &image::DynamicImage, max_bytes: usize) -> Result<PageImage> {
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

    #[test]
    fn images_are_downscaled_to_max_side() {
        let dir = tempfile::tempdir().unwrap();
        let r = UnavailableRenderer { reason: "test".into() };
        let p = dir.path().join("a.png");
        std::fs::write(&p, png(400, 200)).unwrap();
        // within the limit: passed through unchanged
        let out = page_images_sized(&r, &p, "image/png", 5, 170.0, 1 << 20, 400).unwrap();
        assert_eq!(image::load_from_memory(&out[0].1).unwrap().width(), 400);
        // larger: long side becomes max_side, aspect ratio kept
        let out = page_images_sized(&r, &p, "image/png", 5, 170.0, 1 << 20, 100).unwrap();
        let img = image::load_from_memory(&out[0].1).unwrap();
        assert_eq!((img.width(), img.height()), (100, 50));
        // rasters of an image: page 1 only
        assert_eq!(page_rasters(&r, &p, "image/png", &[1, 2], 170.0).unwrap().len(), 1);
        assert!(page_rasters(&r, &p, "image/png", &[2], 170.0).unwrap().is_empty());
        // never upscales
        let small = downscale(image::DynamicImage::ImageRgb8(image::RgbImage::new(10, 20)), 1540);
        assert_eq!((small.width(), small.height()), (10, 20));
    }
}
