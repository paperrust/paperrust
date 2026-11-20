// upstream: src/paperless/parsers/tesseract.py@v3.4.2 (is_image, has_alpha, get_dpi, calculate_a4_dpi)
// upstream: src/documents/utils.py@v3.4.2 (maybe_override_pixel_limit)
// Behavior of Pillow 13 reproduced (source: reference/venv PIL/*ImagePlugin.py):
// - PNG: `pHYs` only with unit 1 (meter) → dpi = px * 0.0254.
// - JPEG: JFIF unit 1 → density, unit 2 → density * 2.54; otherwise, if an EXIF block is present,
//   XResolution (×2.54 with ResolutionUnit 3); EXIF without resolution → (72, 72).
// - TIFF: XResolution/YResolution (default 1!), ResolutionUnit 2 → dpi, 3 → ×2.54, missing → dpi
//   as stored, 1 → no dpi.
// - BMP: pixels_per_meter / 39.3701, only if ≠ (0, 0).
// - GIF, WebP, HEIC: no dpi.
// Alpha (`im.mode in ("RGBA", "LA")`): PNG color type 4/6; TIFF RGB with 4 samples except
// ExtraSamples=0, gray+alpha only with ExtraSamples=2 ("La" at 1 does not count upstream);
// BMP 32-bit BITFIELDS with alpha mask; WebP VP8X alpha flag or VP8L alpha bit; HEIC with
// auxiliary alpha image; GIF/JPEG never.

use std::path::Path;

use crate::OcrError;

/// Image types that the Tesseract parser accepts (`is_image`).
pub const IMAGE_MIME_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/tiff",
    "image/bmp",
    "image/gif",
    "image/webp",
    "image/heic",
];

/// `RasterisedDocumentParser._SUPPORTED_MIME_TYPES` (order as upstream).
pub const SUPPORTED_MIME_TYPES: &[(&str, &str)] = &[
    ("application/pdf", ".pdf"),
    ("image/jpeg", ".jpg"),
    ("image/png", ".png"),
    ("image/tiff", ".tif"),
    ("image/gif", ".gif"),
    ("image/bmp", ".bmp"),
    ("image/webp", ".webp"),
    ("image/heic", ".heic"),
];

pub fn is_image(mime_type: &str) -> bool {
    IMAGE_MIME_TYPES.contains(&mime_type)
}

/// `RasterisedDocumentParser.score`.
pub fn score(mime_type: &str) -> Option<i32> {
    SUPPORTED_MIME_TYPES.iter().any(|(m, _)| *m == mime_type).then_some(10)
}

/// Pillow-Standard `Image.MAX_IMAGE_PIXELS`.
pub const PIL_DEFAULT_MAX_IMAGE_PIXELS: u64 = 1024 * 1024 * 1024 / 4 / 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Tiff,
    Bmp,
    Gif,
    Webp,
    Heic,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    /// Pillow `im.info["dpi"]`.
    pub dpi: Option<(f64, f64)>,
    /// Pillow `im.mode in ("RGBA", "LA")`.
    pub has_alpha: bool,
    /// Pillow mode (approximation, for logs/tests).
    pub mode: &'static str,
    /// EXIF orientation (1–8), if present (for img2pdf `Rotation.ifvalid`).
    pub exif_orientation: Option<u16>,
}

impl ImageInfo {
    /// `get_dpi`: `round(info["dpi"][0])` (Python rounding: half to even).
    pub fn get_dpi(&self) -> Option<i64> {
        self.dpi.map(|(x, _)| py_round(x))
    }

    /// `calculate_a4_dpi`: `int(width / (21 / 2.54))`.
    pub fn calculate_a4_dpi(&self) -> i64 {
        (self.width as f64 / (21.0 / 2.54)) as i64
    }

    /// Pillow `_decompression_bomb_check` with limit `limit` (`None` = off).
    pub fn decompression_bomb_check(&self, limit: Option<u64>) -> Result<(), OcrError> {
        let Some(limit) = limit else { return Ok(()) };
        let pixels = u64::from(self.width.max(1)) * u64::from(self.height.max(1));
        if pixels > 2 * limit {
            return Err(OcrError::DecompressionBomb(format!(
                "Image size ({pixels} pixels) exceeds limit of {} pixels, could be decompression bomb DOS attack.",
                2 * limit
            )));
        }
        if pixels > limit {
            tracing::warn!(
                "Image size ({pixels} pixels) exceeds limit of {limit} pixels, could be decompression bomb DOS attack."
            );
        }
        Ok(())
    }
}

/// `maybe_override_pixel_limit()`: effective Pillow limit from `settings.MAX_IMAGE_PIXELS`.
pub fn pil_pixel_limit(max_image_pixels_setting: Option<i64>) -> Option<u64> {
    match max_image_pixels_setting {
        Some(0) => None,
        Some(n) if n > 0 => Some(n as u64),
        _ => Some(PIL_DEFAULT_MAX_IMAGE_PIXELS),
    }
}

/// Python `round(float)` → int (Banker's rounding).
pub fn py_round(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        let t = x.trunc();
        if (t as i64) % 2 == 0 { t as i64 } else { r as i64 }
    } else {
        r as i64
    }
}

pub fn probe_path(path: &Path) -> Result<ImageInfo, OcrError> {
    let data = std::fs::read(path)?;
    probe(&data)
}

/// Read header data (without decoding pixels). Errors like Pillow `UnidentifiedImageError`.
pub fn probe(data: &[u8]) -> Result<ImageInfo, OcrError> {
    let r = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        probe_png(data)
    } else if data.starts_with(&[0xFF, 0xD8]) {
        probe_jpeg(data)
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        probe_tiff(data)
    } else if data.starts_with(b"BM") {
        probe_bmp(data)
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        probe_gif(data)
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        probe_webp(data)
    } else if data.len() >= 12 && &data[4..8] == b"ftyp" {
        probe_heic(data)
    } else {
        None
    };
    r.ok_or_else(|| OcrError::Other("UnidentifiedImageError: cannot identify image file".into()))
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}
fn be32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
fn le16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn le32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn probe_png(d: &[u8]) -> Option<ImageInfo> {
    let mut pos = 8;
    let mut info = ImageInfo {
        format: ImageFormat::Png,
        width: 0,
        height: 0,
        dpi: None,
        has_alpha: false,
        mode: "RGB",
        exif_orientation: None,
    };
    let mut seen_ihdr = false;
    while pos + 8 <= d.len() {
        let len = be32(d, pos)? as usize;
        let ty = &d[pos + 4..pos + 8];
        let body = d.get(pos + 8..pos + 8 + len)?;
        match ty {
            b"IHDR" => {
                info.width = be32(body, 0)?;
                info.height = be32(body, 4)?;
                let depth = *body.get(8)?;
                let ctype = *body.get(9)?;
                info.mode = match (depth, ctype) {
                    (1, 0) => "1",
                    (16, 0) => "I;16",
                    (_, 0) => "L",
                    (_, 2) => "RGB",
                    (_, 3) => "P",
                    (8, 4) => "LA",
                    (16, 4) => "RGBA",
                    (_, 4) => "LA",
                    (_, 6) => "RGBA",
                    _ => return None,
                };
                info.has_alpha = matches!(info.mode, "RGBA" | "LA");
                seen_ihdr = true;
            }
            b"pHYs" => {
                if len >= 9 && body[8] == 1 {
                    let px = be32(body, 0)? as f64;
                    let py = be32(body, 4)? as f64;
                    info.dpi = Some((px * 0.0254, py * 0.0254));
                }
            }
            b"IDAT" | b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    seen_ihdr.then_some(info)
}

/// Minimal TIFF/EXIF IFD reader: (tag → first value as f64), byte order from the header.
struct Ifd {
    entries: Vec<(u16, f64, u32)>, // tag, first value, count
    extra: Vec<u16>,               // ExtraSamples values
    bps: Vec<u16>,                 // BitsPerSample
}

fn read_ifd(t: &[u8], off: usize) -> Option<Ifd> {
    let le = t.get(0..2)? == b"II";
    let r16 = |o: usize| if le { le16(t, o) } else { be16(t, o) };
    let r32 = |o: usize| if le { le32(t, o) } else { be32(t, o) };
    let n = r16(off)? as usize;
    let mut entries = Vec::new();
    let mut extra = Vec::new();
    let mut bps = Vec::new();
    for i in 0..n.min(4096) {
        let e = off + 2 + i * 12;
        let tag = r16(e)?;
        let typ = r16(e + 2)?;
        let count = r32(e + 4)?;
        let size = match typ {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => continue,
        };
        let total = size as u64 * u64::from(count);
        let voff = if total <= 4 { e + 8 } else { r32(e + 8)? as usize };
        let val_at = |idx: usize| -> Option<f64> {
            let o = voff + idx * size;
            Some(match typ {
                1 | 7 => f64::from(*t.get(o)?),
                6 => f64::from(*t.get(o)? as i8),
                3 => f64::from(r16(o)?),
                8 => f64::from(r16(o)? as i16),
                4 => f64::from(r32(o)?),
                9 => f64::from(r32(o)? as i32),
                5 => {
                    let (a, b) = (f64::from(r32(o)?), f64::from(r32(o + 4)?));
                    if b == 0.0 { f64::NAN } else { a / b }
                }
                10 => {
                    let (a, b) = (f64::from(r32(o)? as i32), f64::from(r32(o + 4)? as i32));
                    if b == 0.0 { f64::NAN } else { a / b }
                }
                11 => f64::from(f32::from_bits(r32(o)?)),
                12 => {
                    let lo = r32(o)? as u64;
                    let hi = r32(o + 4)? as u64;
                    let bits = if le { lo | (hi << 32) } else { (lo << 32) | hi };
                    f64::from_bits(bits)
                }
                _ => return None,
            })
        };
        if count == 0 {
            continue;
        }
        if tag == 338 || tag == 258 {
            let mut v = Vec::new();
            for k in 0..(count as usize).min(16) {
                if let Some(x) = val_at(k) {
                    v.push(x as u16);
                }
            }
            if tag == 338 { extra = v } else { bps = v }
        }
        if let Some(v) = val_at(0) {
            entries.push((tag, v, count));
        }
    }
    Some(Ifd { entries, extra, bps })
}

impl Ifd {
    fn get(&self, tag: u16) -> Option<f64> {
        self.entries.iter().find(|e| e.0 == tag).map(|e| e.1)
    }
    fn has(&self, tag: u16) -> bool {
        self.entries.iter().any(|e| e.0 == tag)
    }
}

fn tiff_first_ifd(t: &[u8]) -> Option<Ifd> {
    let le = t.get(0..2)? == b"II";
    let off = if le { le32(t, 4)? } else { be32(t, 4)? } as usize;
    read_ifd(t, off)
}

fn probe_tiff(d: &[u8]) -> Option<ImageInfo> {
    let ifd = tiff_first_ifd(d)?;
    let width = ifd.get(256)? as u32;
    let height = ifd.get(257)? as u32;
    let photometric = ifd.get(262).map(|v| v as u16).unwrap_or(0);
    let spp = ifd.get(277).map(|v| v as u16).unwrap_or(1);
    let extra: Option<u16> = ifd.extra.first().copied();
    let (mode, alpha) = match photometric {
        2 if spp >= 4 => match (ifd.has(338), extra) {
            (true, Some(0)) => ("RGB", false),
            _ => ("RGBA", true),
        },
        2 => ("RGB", false),
        0 | 1 if spp >= 2 => match extra {
            Some(2) => ("LA", true),
            Some(1) => ("La", false),
            _ => ("L", false),
        },
        0 | 1 => {
            if ifd.bps.first() == Some(&1) { ("1", false) } else { ("L", false) }
        }
        3 => ("P", false),
        5 => ("CMYK", false),
        6 => ("YCbCr", false),
        _ => ("RGB", false),
    };
    // Pillow: xres = tag.get(X_RESOLUTION, 1), yres likewise; `if xres and yres`
    let xres = ifd.get(282).unwrap_or(1.0);
    let yres = ifd.get(283).unwrap_or(1.0);
    let truthy = |v: f64| v != 0.0; // NaN is truthy in Python
    let dpi = if truthy(xres) && truthy(yres) {
        match ifd.get(296).map(|v| v as u16) {
            Some(2) | None => Some((xres, yres)),
            Some(3) => Some((xres * 2.54, yres * 2.54)),
            Some(_) => None,
        }
    } else {
        None
    };
    Some(ImageInfo {
        format: ImageFormat::Tiff,
        width,
        height,
        dpi,
        has_alpha: alpha,
        mode,
        exif_orientation: ifd.get(274).map(|v| v as u16),
    })
}

fn probe_jpeg(d: &[u8]) -> Option<ImageInfo> {
    let mut pos = 2;
    let mut size: Option<(u32, u32, u8)> = None;
    let mut jfif_dpi: Option<(f64, f64)> = None;
    let mut exif: Option<&[u8]> = None;
    while pos + 4 <= d.len() {
        if d[pos] != 0xFF {
            pos += 1;
            continue;
        }
        let marker = d[pos + 1];
        if marker == 0xFF {
            pos += 1;
            continue;
        }
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            pos += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        let len = be16(d, pos + 2)? as usize;
        let seg = d.get(pos + 4..pos + 2 + len)?;
        match marker {
            0xE0 if seg.starts_with(b"JFIF\0") && seg.len() >= 12 => {
                let unit = seg[7];
                let xd = f64::from(be16(seg, 8)?);
                let yd = f64::from(be16(seg, 10)?);
                if unit == 1 {
                    jfif_dpi = Some((xd, yd));
                } else if unit == 2 {
                    jfif_dpi = Some((xd * 2.54, yd * 2.54));
                }
            }
            0xE1 if seg.starts_with(b"Exif\0\0") && exif.is_none() => exif = Some(&seg[6..]),
            0xC0..=0xCF if marker != 0xC4 && marker != 0xC8 && marker != 0xCC => {
                let h = u32::from(be16(seg, 1)?);
                let w = u32::from(be16(seg, 3)?);
                let comps = *seg.get(5)?;
                size = Some((w, h, comps));
            }
            _ => {}
        }
        pos += 2 + len;
    }
    let (width, height, comps) = size?;
    let mut orientation = None;
    let dpi = match jfif_dpi {
        Some(v) => Some(v),
        None => match exif {
            None => None,
            Some(t) => {
                let ifd = tiff_first_ifd(t);
                orientation = ifd.as_ref().and_then(|i| i.get(274)).map(|v| v as u16);
                // KeyError/ZeroDivision/NaN → (72, 72)
                match ifd.and_then(|i| Some((i.get(0x0128)?, i.get(0x011A)?))) {
                    Some((unit, x)) if !x.is_nan() => {
                        let x = if unit as u16 == 3 { x * 2.54 } else { x };
                        Some((x, x))
                    }
                    _ => Some((72.0, 72.0)),
                }
            }
        },
    };
    if orientation.is_none()
        && let Some(t) = exif {
            orientation = tiff_first_ifd(t).and_then(|i| i.get(274)).map(|v| v as u16);
        }
    Some(ImageInfo {
        format: ImageFormat::Jpeg,
        width,
        height,
        dpi,
        has_alpha: false,
        mode: match comps {
            1 => "L",
            4 => "CMYK",
            _ => "RGB",
        },
        exif_orientation: orientation,
    })
}

fn probe_bmp(d: &[u8]) -> Option<ImageInfo> {
    let hsize = le32(d, 14)?;
    let h = d.get(18..)?;
    if hsize == 12 {
        return Some(ImageInfo {
            format: ImageFormat::Bmp,
            width: u32::from(le16(h, 0)?),
            height: u32::from(le16(h, 2)?),
            dpi: None,
            has_alpha: false,
            mode: "RGB",
            exif_orientation: None,
        });
    }
    let width = le32(h, 0)? as i32;
    let height = le32(h, 4)? as i32;
    let bits = le16(h, 10)?;
    let compression = le32(h, 12)?;
    let ppm_x = le32(h, 20)? as i32 as f64;
    let ppm_y = le32(h, 24)? as i32 as f64;
    let dpi = (ppm_x != 0.0 || ppm_y != 0.0).then(|| (ppm_x / 39.3701, ppm_y / 39.3701));
    // Alpha only with 32-bit BITFIELDS with alpha mask (header ≥ 52 bytes after the size).
    let mut alpha = false;
    if bits == 32 && compression == 3 && hsize >= 56 {
        let a_mask = le32(h, 48)?;
        alpha = a_mask != 0;
    }
    Some(ImageInfo {
        format: ImageFormat::Bmp,
        width: width.unsigned_abs(),
        height: height.unsigned_abs(),
        dpi,
        has_alpha: alpha,
        mode: if alpha {
            "RGBA"
        } else if bits <= 8 {
            "P"
        } else {
            "RGB"
        },
        exif_orientation: None,
    })
}

fn probe_gif(d: &[u8]) -> Option<ImageInfo> {
    let gct = d.get(10)? & 0x80 != 0;
    Some(ImageInfo {
        format: ImageFormat::Gif,
        width: u32::from(le16(d, 6)?),
        height: u32::from(le16(d, 8)?),
        dpi: None,
        has_alpha: false,
        mode: if gct { "P" } else { "L" },
        exif_orientation: None,
    })
}

fn probe_webp(d: &[u8]) -> Option<ImageInfo> {
    let mut pos = 12;
    let mut out: Option<(u32, u32, bool)> = None;
    while pos + 8 <= d.len() {
        let ty = &d[pos..pos + 4];
        let len = le32(d, pos + 4)? as usize;
        let body = d.get(pos + 8..(pos + 8 + len).min(d.len()))?;
        match ty {
            b"VP8X" => {
                let flags = *body.first()?;
                let w = (u32::from(*body.get(4)?) | u32::from(*body.get(5)?) << 8 | u32::from(*body.get(6)?) << 16) + 1;
                let h = (u32::from(*body.get(7)?) | u32::from(*body.get(8)?) << 8 | u32::from(*body.get(9)?) << 16) + 1;
                out = Some((w, h, flags & 0x10 != 0));
                break;
            }
            b"VP8L" => {
                let bits = le32(body, 1)?;
                let w = (bits & 0x3FFF) + 1;
                let h = ((bits >> 14) & 0x3FFF) + 1;
                out = Some((w, h, (bits >> 28) & 1 == 1));
                break;
            }
            b"VP8 " => {
                let w = u32::from(le16(body, 6)? & 0x3FFF);
                let h = u32::from(le16(body, 8)? & 0x3FFF);
                out = Some((w, h, false));
                break;
            }
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    let (w, h, a) = out?;
    Some(ImageInfo {
        format: ImageFormat::Webp,
        width: w,
        height: h,
        dpi: None,
        has_alpha: a,
        mode: if a { "RGBA" } else { "RGB" },
        exif_orientation: None,
    })
}

/// ISO-BMFF boxes of a range: (type, content).
fn bmff_boxes(d: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 8 <= d.len() {
        let Some(size32) = be32(d, pos) else { break };
        let ty = &d[pos + 4..pos + 8];
        let (hdr, size) = match size32 {
            1 => match d.get(pos + 8..pos + 16) {
                Some(b) => (16usize, u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as usize),
                None => break,
            },
            0 => (8, d.len() - pos),
            n => (8, n as usize),
        };
        // 64-bit size from the file: addition checked (fuzz finding 20.11.: overflow at size ≈ u64::MAX)
        let Some(end) = pos.checked_add(size) else { break };
        if size < hdr || end > d.len() {
            break;
        }
        out.push((ty, &d[pos + hdr..end]));
        pos = end;
    }
    out
}

/// HEIC: size of the main image (`pitm` → `ipma` → `ispe`, with `clap`/`irot` like pillow_heif),
/// alpha via an `auxC` auxiliary image with alpha URN.
fn probe_heic(d: &[u8]) -> Option<ImageInfo> {
    let brand = d.get(8..12)?;
    if !matches!(brand, b"heic" | b"heix" | b"mif1" | b"msf1" | b"heim" | b"heis" | b"hevc" | b"hevx") {
        return None;
    }
    let meta = bmff_boxes(d).into_iter().find(|(t, _)| *t == b"meta")?.1.get(4..)?;
    let mut primary: Option<u32> = None;
    let mut props: Vec<(&[u8], &[u8])> = Vec::new();
    let mut assoc: Vec<(u32, Vec<usize>)> = Vec::new();
    for (ty, body) in bmff_boxes(meta) {
        match ty {
            b"pitm" => {
                primary = if body.first() == Some(&0) { be16(body, 4).map(u32::from) } else { be32(body, 4) };
            }
            b"iprp" => {
                for (t2, b2) in bmff_boxes(body) {
                    if t2 == b"ipco" {
                        props = bmff_boxes(b2);
                    } else if t2 == b"ipma" {
                        let version = *b2.first()?;
                        let flags = be32(b2, 0)? & 0xFF_FFFF;
                        let count = be32(b2, 4)? as usize;
                        let mut pos = 8;
                        for _ in 0..count.min(100_000) {
                            let item = if version < 1 {
                                let v = u32::from(be16(b2, pos)?);
                                pos += 2;
                                v
                            } else {
                                let v = be32(b2, pos)?;
                                pos += 4;
                                v
                            };
                            let n = *b2.get(pos)? as usize;
                            pos += 1;
                            let mut idx = Vec::with_capacity(n);
                            for _ in 0..n {
                                if flags & 1 == 1 {
                                    idx.push((be16(b2, pos)? & 0x7FFF) as usize);
                                    pos += 2;
                                } else {
                                    idx.push((b2.get(pos)? & 0x7F) as usize);
                                    pos += 1;
                                }
                            }
                            assoc.push((item, idx));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let alpha = props.iter().any(|(t, b)| {
        *t == b"auxC" && {
            let urn = String::from_utf8_lossy(b.get(4..).unwrap_or_default());
            let urn = urn.split(char::from(0)).next().unwrap_or_default().to_string();
            urn.ends_with(":alpha") || urn == "urn:mpeg:hevc:2015:auxid:1"
        }
    });
    let primary_props: Vec<&(&[u8], &[u8])> = match primary.and_then(|p| assoc.iter().find(|(i, _)| *i == p)) {
        Some((_, idx)) => idx.iter().filter_map(|i| i.checked_sub(1).and_then(|i| props.get(i))).collect(),
        None => props.iter().collect(),
    };
    let (mut w, mut h) = primary_props
        .iter()
        .find(|(t, _)| *t == b"ispe")
        .and_then(|(_, b)| Some((be32(b, 4)?, be32(b, 8)?)))?;
    for (t, b) in &primary_props {
        if *t == b"clap"
            && let (Some(wn), Some(wd), Some(hn), Some(hd)) = (be32(b, 0), be32(b, 4), be32(b, 8), be32(b, 12))
                && wd != 0 && hd != 0 && wn / wd > 0 && hn / hd > 0 {
                    (w, h) = (wn / wd, hn / hd);
                }
    }
    for (t, b) in &primary_props {
        if *t == b"irot" && b.first().is_some_and(|r| r & 1 == 1) {
            (w, h) = (h, w);
        }
    }
    Some(ImageInfo {
        format: ImageFormat::Heic,
        width: w,
        height: h,
        dpi: None,
        has_alpha: alpha,
        mode: if alpha { "RGBA" } else { "RGB" },
        exif_orientation: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmff_huge_box_size_does_not_overflow() {
        // ftyp heic, then a box with size32 = 1 and 64-bit size u64::MAX (fuzz finding image, 20.11.)
        let mut d = 24u32.to_be_bytes().to_vec();
        d.extend_from_slice(b"ftypheic\0\0\0\0mif1heic");
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(b"meta");
        d.extend_from_slice(&u64::MAX.to_be_bytes());
        d.extend_from_slice(&[0u8; 16]);
        assert!(bmff_boxes(&d).len() == 1);
        let _ = probe(&d);
    }

    #[test]
    fn py_round_half_even() {
        assert_eq!(py_round(72.4), 72);
        assert_eq!(py_round(72.5), 72);
        assert_eq!(py_round(73.5), 74);
        assert_eq!(py_round(299.9994), 300);
    }

    #[test]
    fn png_phys_meter_only() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let chunk = |ty: &[u8], body: &[u8]| {
            let mut c = (body.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(ty);
            c.extend_from_slice(body);
            c.extend_from_slice(&[0, 0, 0, 0]);
            c
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&100u32.to_be_bytes());
        ihdr.extend_from_slice(&50u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        png.extend(chunk(b"IHDR", &ihdr));
        let mut phys = 2835u32.to_be_bytes().to_vec();
        phys.extend_from_slice(&2835u32.to_be_bytes());
        phys.push(1);
        png.extend(chunk(b"pHYs", &phys));
        let i = probe(&png).unwrap();
        assert!(i.has_alpha);
        assert_eq!(i.get_dpi(), Some(72));
        assert_eq!(i.calculate_a4_dpi(), 12);
    }

    #[test]
    fn bomb_check_like_pillow() {
        let i = ImageInfo {
            format: ImageFormat::Png,
            width: 1000,
            height: 1000,
            dpi: None,
            has_alpha: false,
            mode: "RGB",
            exif_orientation: None,
        };
        assert!(i.decompression_bomb_check(Some(600_000)).is_ok());
        let e = i.decompression_bomb_check(Some(400_000)).unwrap_err();
        assert_eq!(
            e.message(),
            "Image size (1000000 pixels) exceeds limit of 800000 pixels, could be decompression bomb DOS attack."
        );
        assert_eq!(pil_pixel_limit(Some(0)), None);
        assert_eq!(pil_pixel_limit(None), Some(89_478_485));
    }
}
