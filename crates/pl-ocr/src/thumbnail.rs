// upstream: src/documents/parsers.py@v3.4.2 (make_thumbnail_from_pdf, _compute_thumbnail_dpi,
//           _render_pdf_thumbnail, encode_thumbnail_webp; constants _THUMBNAIL_*)
// upstream: src/paperless/parsers/tesseract.py@v3.4.2 (get_thumbnail: archive, otherwise original;
//           upstream fix 759f9cd22: image without archive → make_thumbnail_from_image)
//
// pdftoppm → PDFium (via [`PdfRenderer`]); qpdf repair is dropped (error → the caller takes the
// default thumbnail, like upstream `get_default_thumbnail`). WebP lossless (the image crate has
// no lossy encoder; Pillow saves with quality 80) – see Known differences.
// Before encoding 32 levels per color channel ([`posterize`]): halves the file size (benchmark
// 2026-11-18: 42 → 20 KiB per thumbnail; ngx with q80 ~12 KiB).

use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::render::PdfRenderer;
use crate::{OcrError, Result};

pub const THUMBNAIL_MAX_WIDTH: f64 = 500.0;
pub const THUMBNAIL_MAX_HEIGHT: f64 = 5000.0;
pub const THUMBNAIL_FALLBACK_DPI: f64 = 150.0;
pub const THUMBNAIL_MAX_DPI: f64 = 300.0;
pub const THUMBNAIL_SUPERSAMPLE: u32 = 2;

/// `_compute_thumbnail_dpi`: (dpi, supersample).
pub fn compute_thumbnail_dpi(size_pt: Option<(f64, f64)>) -> (f64, u32) {
    match size_pt {
        Some((w, h)) if w > 0.0 && h > 0.0 => {
            let dw = THUMBNAIL_MAX_WIDTH * 72.0 / w;
            let dh = THUMBNAIL_MAX_HEIGHT * 72.0 / h;
            (THUMBNAIL_MAX_DPI.min(dw).min(dh), THUMBNAIL_SUPERSAMPLE)
        }
        _ => (THUMBNAIL_FALLBACK_DPI, 1),
    }
}

/// Target size of `encode_thumbnail_webp` (only shrink).
pub fn thumbnail_target(w: u32, h: u32, supersample: u32) -> (u32, u32) {
    let scale = (1.0 / f64::from(supersample.max(1)))
        .min(THUMBNAIL_MAX_WIDTH / f64::from(w.max(1)))
        .min(THUMBNAIL_MAX_HEIGHT / f64::from(h.max(1)));
    if scale < 1.0 { (((f64::from(w) * scale).round() as u32).max(1), ((f64::from(h) * scale).round() as u32).max(1)) } else { (w, h) }
}

/// Levels per color channel in the thumbnail (5 bit, like RGB555).
pub const THUMBNAIL_LEVELS: u32 = 32;

/// Rounds every channel to [`THUMBNAIL_LEVELS`] evenly distributed levels (0 and 255 are kept,
/// gray stays gray). Lossless WebP then compresses the edge smoothing and the noise
/// of scanned pages much better; at 500 px width the difference is not visible.
pub fn posterize(img: &mut image::RgbImage) {
    let n = THUMBNAIL_LEVELS - 1;
    let lut: [u8; 256] = std::array::from_fn(|v| {
        let q = (v as u32 * n + 127) / 255;
        ((q * 255 + n / 2) / n) as u8
    });
    for c in img.iter_mut() {
        *c = lut[usize::from(*c)];
    }
}

/// Thumbnail as (lossless) WebP after [`posterize`].
fn encode_webp(mut rgb: image::RgbImage) -> std::result::Result<Vec<u8>, image::ImageError> {
    posterize(&mut rgb);
    let mut buf = Vec::new();
    rgb.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::WebP)?;
    Ok(buf)
}

/// `make_thumbnail_from_pdf(in_path, temp_dir)` → `<temp_dir>/convert.webp`.
pub fn make_thumbnail_from_pdf(renderer: &dyn PdfRenderer, pdf: &Path, temp_dir: &Path) -> Result<PathBuf> {
    let data = std::fs::read(pdf)?;
    let doc = renderer.open(&data)?;
    if doc.page_count() == 0 {
        return Err(OcrError::Parse(format!("Unable to make thumbnail: {} has no pages", pdf.display())));
    }
    let (dpi, ss) = compute_thumbnail_dpi(doc.page_size_pt(0).ok());
    let raster = doc.render(0, dpi * f64::from(ss), false)?;
    let img = raster.to_dynamic().ok_or_else(|| OcrError::Parse("Unable to encode thumbnail".into()))?;
    let (tw, th) = thumbnail_target(raster.width, raster.height, ss);
    let img = if (tw, th) != (raster.width, raster.height) { img.resize_exact(tw, th, image::imageops::FilterType::Lanczos3) } else { img };
    let buf = encode_webp(img.to_rgb8()).map_err(|e| OcrError::Parse(format!("Unable to encode thumbnail: {e}")))?;
    std::fs::create_dir_all(temp_dir)?;
    let out = temp_dir.join("convert.webp");
    std::fs::write(&out, buf)?;
    Ok(out)
}

/// Thumbnail directly from an image (without PDF): alpha onto white, downscaled to 500×5000
/// (never enlarged), WebP → `<temp_dir>/convert.webp`.
///
/// upstream fix 759f9cd22 (#14741, paperless-ngx `dev` after v3.4.2):
/// `RasterisedDocumentParser.get_thumbnail` takes, for an image **without archive PDF**,
/// `encode_thumbnail_webp(original, …)` instead of `make_thumbnail_from_pdf(original)` (which fails for an
/// image and returns the default image, e.g. when regenerating thumbnails).
/// Error when reading/encoding → `OcrError::Parse("Unable to encode thumbnail from …")` like
/// upstream; the image crate cannot do HEIC (the caller then falls back to the default image).
pub fn make_thumbnail_from_image(image_path: &Path, temp_dir: &Path) -> Result<PathBuf> {
    let fail = |e: &dyn std::fmt::Display| OcrError::Parse(format!("Unable to encode thumbnail from {}: {e}", image_path.display()));
    let img = image::ImageReader::open(image_path)
        .map_err(|e| fail(&e))?
        .with_guessed_format()
        .map_err(|e| fail(&e))?
        .decode()
        .map_err(|e| fail(&e))?;
    let rgb = if img.color().has_alpha() {
        // `Image.new("RGB", size, white).paste(im, mask=alpha)`
        let rgba = img.to_rgba8();
        let mut out = image::RgbImage::new(rgba.width(), rgba.height());
        for (o, p) in out.pixels_mut().zip(rgba.pixels()) {
            let a = u16::from(p[3]);
            let blend = |c: u8| ((u16::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
            *o = image::Rgb([blend(p[0]), blend(p[1]), blend(p[2])]);
        }
        out
    } else {
        img.to_rgb8()
    };
    let (tw, th) = thumbnail_target(rgb.width(), rgb.height(), 1);
    let rgb = if (tw, th) != rgb.dimensions() { image::imageops::resize(&rgb, tw, th, image::imageops::FilterType::Lanczos3) } else { rgb };
    let buf = encode_webp(rgb).map_err(|e| fail(&e))?;
    std::fs::create_dir_all(temp_dir)?;
    let out = temp_dir.join("convert.webp");
    std::fs::write(&out, buf)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// upstream fix 759f9cd22 (#14741): `test_image_without_archive_has_image_thumbnail` – every
    /// readable image format yields a WebP ≤ 500×5000 (no PDF rendering, no default image).
    #[test]
    fn image_without_archive_has_image_thumbnail() {
        let dir = std::env::temp_dir().join(format!("pl-ocr-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rgb =
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(1000, 1400, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])));
        let rgba = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(300, 200, |x, _| image::Rgba([0, 0, 0, (x % 256) as u8])));
        let cases: &[(&str, &image::DynamicImage, image::ImageFormat, (u32, u32))] = &[
            ("simple.jpg", &rgb, image::ImageFormat::Jpeg, (500, 700)),
            ("simple.png", &rgb, image::ImageFormat::Png, (500, 700)),
            ("simple.tif", &rgb, image::ImageFormat::Tiff, (500, 700)),
            ("simple.gif", &rgb, image::ImageFormat::Gif, (500, 700)),
            ("simple.bmp", &rgb, image::ImageFormat::Bmp, (500, 700)),
            ("document.webp", &rgb, image::ImageFormat::WebP, (500, 700)),
            ("alpha.png", &rgba, image::ImageFormat::Png, (300, 200)),
        ];
        for (name, img, fmt, want) in cases {
            let src = dir.join(name);
            img.save_with_format(&src, *fmt).unwrap();
            let tmp = dir.join(format!("t-{name}"));
            let out = make_thumbnail_from_image(&src, &tmp).unwrap();
            assert_eq!(out, tmp.join("convert.webp"));
            let bytes = std::fs::read(&out).unwrap();
            assert_eq!(image::guess_format(&bytes).unwrap(), image::ImageFormat::WebP, "{name}");
            let t = image::load_from_memory(&bytes).unwrap();
            assert_eq!((t.width(), t.height()), *want, "{name}");
        }
        // transparent → white, opaque black stays black
        let t = image::open(dir.join("t-alpha.png/convert.webp")).unwrap().to_rgb8();
        assert_eq!(t.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(t.get_pixel(255, 0).0, [0, 0, 0]);
        // no image → parse error (the caller takes the default image)
        std::fs::write(dir.join("broken.heic"), b"not an image").unwrap();
        let e = make_thumbnail_from_image(&dir.join("broken.heic"), &dir).unwrap_err();
        assert!(e.to_string().contains("Unable to encode thumbnail from"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn posterize_keeps_extremes_and_gray() {
        let mut img = image::RgbImage::from_fn(256, 1, |x, _| image::Rgb([x as u8, x as u8, 255 - x as u8]));
        posterize(&mut img);
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 255]);
        assert_eq!(img.get_pixel(255, 0).0, [255, 255, 0]);
        let mut levels: Vec<u8> = img.pixels().map(|p| p[0]).collect();
        levels.dedup();
        assert_eq!(levels.len(), THUMBNAIL_LEVELS as usize);
        // at most half a level deviation, gray stays gray
        for (x, p) in img.pixels().enumerate() {
            assert!((i32::from(p[0]) - x as i32).abs() <= 5, "{x} -> {}", p[0]);
            assert_eq!(p[0], p[1]);
        }
    }

    #[test]
    fn posterized_thumbnail_is_smaller() {
        // Text-like page with noise (like a scan): lossless after posterize clearly smaller.
        let mut seed = 7u32;
        let img = image::RgbImage::from_fn(500, 700, |x, y| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let noise = (seed >> 16) % 9;
            let ink = (y / 12) % 2 == 0 && (x / 5) % 3 != 0 && x > 40 && x < 460;
            let v = if ink { 30 + noise } else { 246 + noise } as u8;
            image::Rgb([v, v, v])
        });
        let mut plain = Vec::new();
        img.write_to(&mut Cursor::new(&mut plain), image::ImageFormat::WebP).unwrap();
        let small = encode_webp(img).unwrap();
        assert!(small.len() * 3 < plain.len() * 2, "{} vs {}", small.len(), plain.len());
    }

    #[test]
    fn dpi_like_upstream() {
        // A4: 595x842 pt → 500*72/595 = 60.5 dpi
        let (d, s) = compute_thumbnail_dpi(Some((595.0, 842.0)));
        assert!((d - 60.50).abs() < 0.01 && s == 2);
        // tiny page: upper limit 300
        assert_eq!(compute_thumbnail_dpi(Some((50.0, 50.0))), (300.0, 2));
        assert_eq!(compute_thumbnail_dpi(None), (150.0, 1));
        assert_eq!(thumbnail_target(1000, 1415, 2), (500, 708));
        assert_eq!(thumbnail_target(300, 300, 1), (300, 300));
    }
}
