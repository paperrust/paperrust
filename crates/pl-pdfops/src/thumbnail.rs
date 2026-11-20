// upstream: src/paperless/parsers/text.py@v3.4.2 (TextDocumentParser.get_thumbnail),
// src/paperless/settings/__init__.py (THUMBNAIL_FONT_NAME), Pillow 13 (ImageDraw.multiline_text,
// ImageFont.truetype(size=20, layout_engine=BASIC))
//
//! Text thumbnail: white 500×700 image, text from (5, 5) at font size 20 px (em), line spacing
//! = ascent + 4 (Pillow `_multiline_spacing`), black, as WebP. For text files like
//! upstream; for office/mail (upstream: first page of the Gotenberg PDF) as a substitute with the
//! extracted text instead of the default image.

use std::path::{Path, PathBuf};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};

use crate::{PdfOpsError, Result};

/// `PAPERLESS_THUMBNAIL_FONT_NAME`-Standard.
pub const DEFAULT_FONT: &str = "/usr/share/fonts/liberation/LiberationSerif-Regular.ttf";
pub const WIDTH: u32 = 500;
pub const HEIGHT: u32 = 700;
const FONT_SIZE: f32 = 20.0;
const SPACING: f32 = 4.0;
const MAX_CHARS: usize = 100_000;
const FILE_SIZE_LIMIT: u64 = 50 * 1024 * 1024;

/// Loaded TrueType/OpenType font.
pub struct ThumbnailFont {
    font: FontVec,
}

impl ThumbnailFont {
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).map_err(|e| PdfOpsError::Unavailable(format!("cannot open font {}: {e}", path.display())))?;
        Self::from_bytes(data)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let font = FontVec::try_from_vec(data).map_err(|e| PdfOpsError::Unavailable(format!("invalid font: {e}")))?;
        Ok(ThumbnailFont { font })
    }

    /// `PAPERLESS_THUMBNAIL_FONT_NAME` or the default path.
    pub fn from_env() -> Result<Self> {
        let p = pl_envlive::var("PAPERLESS_THUMBNAIL_FONT_NAME").unwrap_or_else(|| DEFAULT_FONT.into());
        Self::load(Path::new(&p))
    }
}

/// Text like `open(encoding="utf-8", errors="replace").read(100_000)` (universal newlines).
pub fn read_text_for_thumbnail(path: &Path) -> Result<String> {
    if std::fs::metadata(path)?.len() > FILE_SIZE_LIMIT {
        return Ok("[File too large to preview]".into());
    }
    let data = std::fs::read(path)?;
    let s = String::from_utf8_lossy(&data);
    let s = s.replace("\r\n", "\n").replace('\r', "\n");
    Ok(s.chars().take(MAX_CHARS).collect())
}

/// Set text onto a 500×700 page (RGB).
pub fn render_text(text: &str, font: &ThumbnailFont) -> image::RgbImage {
    let mut img = image::RgbImage::from_pixel(WIDTH, HEIGHT, image::Rgb([255, 255, 255]));
    let f = &font.font;
    let upem = f.units_per_em().unwrap_or(1000.0);
    // Pillow/FreeType: size 20 = em in pixels; ab_glyph PxScale refers to the line height.
    let scale = PxScale::from(FONT_SIZE * f.height_unscaled() / upem);
    let sf = f.as_scaled(scale);
    let ascent = (f.ascent_unscaled() * FONT_SIZE / upem).ceil();
    let line_h = ascent + SPACING;
    for (i, line) in text.split('\n').enumerate() {
        let top = 5.0 + i as f32 * line_h;
        if top > HEIGHT as f32 {
            break;
        }
        let baseline = top + ascent;
        let mut x = 5.0f32;
        let mut prev: Option<ab_glyph::GlyphId> = None;
        for c in line.chars() {
            if x > WIDTH as f32 {
                break;
            }
            let id = f.glyph_id(c);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(scale, point(x, baseline));
            if let Some(og) = f.outline_glyph(g) {
                let b = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    let px = b.min.x as i64 + i64::from(gx);
                    let py = b.min.y as i64 + i64::from(gy);
                    if px >= 0 && py >= 0 && (px as u32) < WIDTH && (py as u32) < HEIGHT {
                        let p = img.get_pixel_mut(px as u32, py as u32);
                        let v = (f32::from(p.0[0]) * (1.0 - cov.clamp(0.0, 1.0))).round() as u8;
                        *p = image::Rgb([v, v, v]);
                    }
                });
            }
            x += sf.h_advance(id);
            prev = Some(id);
        }
    }
    img
}

/// RGB image → WebP (lossless; Pillow saves lossy with quality 80).
pub fn encode_webp(img: &image::RgbImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| PdfOpsError::Value(format!("WebP encoding failed: {e}")))?;
    Ok(out)
}

/// Text → WebP-Bytes.
pub fn text_thumbnail_webp(text: &str, font: &ThumbnailFont) -> Result<Vec<u8>> {
    encode_webp(&render_text(text, font))
}

/// `TextDocumentParser.get_thumbnail`: `<tempdir>/thumb.webp`.
pub fn make_text_thumbnail(document_path: &Path, font: &ThumbnailFont, tempdir: &Path) -> Result<PathBuf> {
    let text = read_text_for_thumbnail(document_path)?;
    let out = tempdir.join("thumb.webp");
    std::fs::write(&out, text_thumbnail_webp(&text, font)?)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_font() -> Option<ThumbnailFont> {
        let candidates = [
            pl_envlive::var("PAPERLESS_THUMBNAIL_FONT_NAME").unwrap_or_default(),
            DEFAULT_FONT.to_string(),
            "C:/Windows/Fonts/times.ttf".into(),
            "C:/Windows/Fonts/arial.ttf".into(),
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into(),
        ];
        candidates.iter().filter(|c| !c.is_empty()).find_map(|c| ThumbnailFont::load(Path::new(c)).ok())
    }

    #[test]
    fn renders_text_and_encodes_webp() {
        let Some(font) = system_font() else {
            eprintln!("no TrueType font found, skipping");
            return;
        };
        let img = render_text("Hello World\nZweite Zeile äöü", &font);
        let dark = img.pixels().filter(|p| p.0[0] < 128).count();
        assert!(dark > 100, "text drawn ({dark} dark pixels)");
        // nothing outside the text area (below the 2nd line)
        assert!((100..HEIGHT).all(|y| (0..WIDTH).all(|x| img.get_pixel(x, y).0[0] == 255)));
        let webp = encode_webp(&img).unwrap();
        assert_eq!(&webp[..4], b"RIFF");
        assert_eq!(&webp[8..12], b"WEBP");
        let back = image::load_from_memory(&webp).unwrap();
        assert_eq!((back.width(), back.height()), (WIDTH, HEIGHT));
    }

    #[test]
    fn reads_text_like_python() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.txt");
        std::fs::write(&p, b"a\r\nb\rc\xffd").unwrap();
        assert_eq!(read_text_for_thumbnail(&p).unwrap(), "a\nb\nc\u{fffd}d");
        assert!(ThumbnailFont::load(&t.path().join("missing.ttf")).is_err());
    }
}
