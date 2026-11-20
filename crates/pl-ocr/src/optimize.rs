//! Archive size reduction like OCRmyPDF `--optimize 0..3` (`ocrmypdf/optimize.py`, 17.12), without
//! jbig2enc/pngquant: own encoders in pure Rust.
//!
//! | Level | OCRmyPDF | here |
//! |---|---|---|
//! | 0 | nothing | nothing |
//! | 1 (default) | JBIG2 lossless for 1-bit images, Flate over JPEG (< 10000 px), unused objects removed, object streams | 1-bit → JBIG2 ([`crate::jbig2`]); 8-bit Flate images re-encoded losslessly (PNG predictor per row, colorless RGB images → gray, pure black/white images → 1 bit → JBIG2, ≤ 256 colors → Indexed); Flate over JPEG; unused objects removed; object streams (not with PDF/A-1) |
//! | 2 | + JPEG re-encoded with `jpeg_quality` (75), pngquant (`png_quality` 70 ± 10) | + JPEG re-encoded (quality 75), color reduction ≤ 256 colors (median cut) if the deviation fits the quality |
//! | 3 | like 2 with quality 40/30, Flate level 9, Flate over all JPEGs | likewise |
//!
//! Every re-encoding is accepted only if it is smaller (OCRmyPDF: "made larger – skip").

use lopdf::{Document, Object};

use crate::pdfimage::{self, ColorSpace, Encoded, Pixels};

/// OCRmyPDF `DEFAULT_JPEG_QUALITY` / `DEFAULT_PNG_QUALITY`.
pub const DEFAULT_JPEG_QUALITY: u8 = 75;
pub const DEFAULT_PNG_QUALITY: u8 = 70;
/// OCRmyPDF `FLATE_JPEG_THRESHOLD`.
pub const FLATE_JPEG_THRESHOLD: i64 = 10000;

/// Optimization options (`optimize`, `jpeg_quality`, `png_quality` from OCRmyPDF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptimizeOptions {
    pub level: u8,
    /// 0 = default per level.
    pub jpeg_quality: u8,
    pub png_quality: u8,
    /// `--pdfa-image-compression jpeg`: 8-bit color/gray images as JPEG (Ghostscript `/DCTEncode`).
    pub force_jpeg: bool,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        // OCRmyPDF-Standard `--optimize 1`
        Self { level: 1, jpeg_quality: 0, png_quality: 0, force_jpeg: false }
    }
}

impl OptimizeOptions {
    pub fn none() -> Self {
        Self { level: 0, jpeg_quality: 0, png_quality: 0, force_jpeg: false }
    }
    pub fn effective_jpeg_quality(&self) -> u8 {
        if self.jpeg_quality > 0 { self.jpeg_quality } else if self.level < 3 { DEFAULT_JPEG_QUALITY } else { 40 }
    }
    pub fn effective_png_quality(&self) -> u8 {
        if self.png_quality > 0 { self.png_quality } else if self.level < 3 { DEFAULT_PNG_QUALITY } else { 30 }
    }
    fn complevel(&self) -> u32 {
        if self.level >= 3 { 9 } else { 6 }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OptimizeReport {
    pub images_seen: usize,
    pub jbig2: usize,
    pub flate: usize,
    pub jpeg_recoded: usize,
    pub jpeg_deflated: usize,
    pub quantized: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

/// Gray RGB (all pixels R=G=B) → gray (lossless).
fn reduce_gray(p: &Pixels) -> Option<Pixels> {
    if p.bpc != 8 || p.cs != ColorSpace::Rgb {
        return None;
    }
    if !p.data.as_chunks::<3>().0.iter().all(|c| c[0] == c[1] && c[1] == c[2]) {
        return None;
    }
    Some(Pixels { cs: ColorSpace::Gray, data: p.data.as_chunks::<3>().0.iter().map(|c| c[0]).collect(), ..p.clone() })
}

/// Gray with only 0/255 → 1 bit (lossless).
fn reduce_bitonal(p: &Pixels) -> Option<Pixels> {
    if p.bpc != 8 || p.cs != ColorSpace::Gray || !p.data.iter().all(|&v| v == 0 || v == 255) {
        return None;
    }
    let w = p.width as usize;
    let stride = w.div_ceil(8);
    let mut out = vec![0u8; stride * p.height as usize];
    for (y, row) in p.data.chunks_exact(w).enumerate() {
        for (x, &v) in row.iter().enumerate() {
            if v == 255 {
                out[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    Some(Pixels { bpc: 1, data: out, ..p.clone() })
}

/// ≤ 256 colors → Indexed (lossless).
fn reduce_palette(p: &Pixels) -> Option<Pixels> {
    if p.bpc != 8 || p.cs != ColorSpace::Rgb {
        return None;
    }
    let mut map: std::collections::HashMap<[u8; 3], u8> = std::collections::HashMap::new();
    let mut lookup = Vec::new();
    let mut idx = Vec::with_capacity(p.data.len() / 3);
    for c in p.data.as_chunks::<3>().0 {
        let k = [c[0], c[1], c[2]];
        let i = match map.get(&k) {
            Some(i) => *i,
            None => {
                if map.len() == 256 {
                    return None;
                }
                let i = map.len() as u8;
                map.insert(k, i);
                lookup.extend_from_slice(&k);
                i
            }
        };
        idx.push(i);
    }
    let hival = (map.len() - 1) as u8;
    Some(Pixels { cs: ColorSpace::Indexed { base: Box::new(ColorSpace::Rgb), hival, lookup }, data: idx, ..p.clone() })
}

/// Median cut to ≤ 256 colors; `None` if the deviation is too large (like pngquant `--quality min-max`).
/// Quality → PSNR threshold: q_min 60 ≈ 32 dB, q_min 20 ≈ 24 dB (approximation of pngquant).
fn quantize(p: &Pixels, png_quality: u8) -> Option<Pixels> {
    if p.bpc != 8 || p.cs != ColorSpace::Rgb {
        return None;
    }
    let q_min = f64::from(png_quality.saturating_sub(10).max(10));
    let min_psnr = 20.0 + 0.2 * q_min;
    // sample (at most ~256k pixels) for the boxes
    let n = p.data.len() / 3;
    let step = (n / 262_144).max(1);
    let mut sample: Vec<[u8; 3]> = p.data.as_chunks::<3>().0.iter().step_by(step).copied().collect();
    let mut boxes: Vec<(usize, usize)> = vec![(0, sample.len())];
    while boxes.len() < 256 {
        // split the box with the largest span
        let mut best: Option<(usize, usize, i32)> = None;
        for (bi, &(a, b)) in boxes.iter().enumerate() {
            if b - a < 2 {
                continue;
            }
            for ch in 0..3 {
                let (mn, mx) = sample[a..b].iter().fold((255u8, 0u8), |(mn, mx), c| (mn.min(c[ch]), mx.max(c[ch])));
                let range = i32::from(mx) - i32::from(mn);
                if best.is_none_or(|(_, _, r)| range > r) {
                    best = Some((bi, ch, range));
                }
            }
        }
        let Some((bi, ch, range)) = best else { break };
        if range == 0 {
            break;
        }
        let (a, b) = boxes[bi];
        sample[a..b].sort_unstable_by_key(|c| c[ch]);
        let mid = a + (b - a) / 2;
        boxes[bi] = (a, mid);
        boxes.push((mid, b));
    }
    let palette: Vec<[u8; 3]> = boxes
        .iter()
        .filter(|(a, b)| b > a)
        .map(|&(a, b)| {
            let mut s = [0u64; 3];
            for c in &sample[a..b] {
                for i in 0..3 {
                    s[i] += u64::from(c[i]);
                }
            }
            let n = (b - a) as u64;
            [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8]
        })
        .collect();
    if palette.is_empty() {
        return None;
    }
    // mapping with cache (5 bits per channel)
    let mut cache = vec![u16::MAX; 1 << 15];
    let mut idx = Vec::with_capacity(n);
    let mut sq_err = 0u64;
    for c in p.data.as_chunks::<3>().0 {
        let key = (usize::from(c[0] >> 3) << 10) | (usize::from(c[1] >> 3) << 5) | usize::from(c[2] >> 3);
        let i = if cache[key] != u16::MAX {
            cache[key] as usize
        } else {
            let mut bi = 0;
            let mut bd = u32::MAX;
            for (i, pc) in palette.iter().enumerate() {
                let d: u32 = (0..3).map(|k| (i32::from(pc[k]) - i32::from(c[k])).pow(2) as u32).sum();
                if d < bd {
                    bd = d;
                    bi = i;
                }
            }
            cache[key] = bi as u16;
            bi
        };
        let pc = palette[i];
        sq_err += (0..3).map(|k| (i64::from(pc[k]) - i64::from(c[k])).pow(2) as u64).sum::<u64>();
        idx.push(i as u8);
    }
    let mse = sq_err as f64 / (n as f64 * 3.0);
    let psnr = if mse <= 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
    if psnr < min_psnr {
        tracing::debug!("quantize: PSNR {psnr:.1} dB below {min_psnr:.1} dB, skipped");
        return None;
    }
    let lookup: Vec<u8> = palette.iter().flatten().copied().collect();
    Some(Pixels {
        cs: ColorSpace::Indexed { base: Box::new(ColorSpace::Rgb), hival: (palette.len() - 1) as u8, lookup },
        data: idx,
        ..p.clone()
    })
}

fn jbig2(p: &Pixels) -> Option<Encoded> {
    let data = crate::jbig2::encode_generic(&p.data, p.width, p.height)?;
    Some(Encoded { content: data, filter: Object::Name(b"JBIG2Decode".to_vec()), decode_parms: None, bpc: 1, cs: Object::Name(b"DeviceGray".to_vec()) })
}

/// Best possible lossless encoding of an 8-/1-bit image.
fn best_lossless(p: &Pixels, o: &OptimizeOptions, allow_cs_change: bool) -> Encoded {
    let mut cands: Vec<Encoded> = Vec::new();
    let mut base = p.clone();
    if allow_cs_change {
        if let Some(g) = reduce_gray(&base) {
            base = g;
        }
        if let Some(b) = reduce_bitonal(&base) {
            base = b;
        }
    }
    if base.bpc == 1
        && base.cs == ColorSpace::Gray
        && let Some(e) = jbig2(&base)
    {
        cands.push(e);
    }
    if allow_cs_change && let Some(pal) = reduce_palette(&base) {
        cands.push(pdfimage::encode_flate(&pal, o.complevel()));
    }
    cands.push(pdfimage::encode_flate(&base, o.complevel()));
    cands.into_iter().min_by_key(|e| e.content.len()).unwrap_or_else(|| pdfimage::encode_flate(p, o.complevel()))
}

/// `pdfa_image_compression=jpeg`: losslessly stored 8-bit images (gray/RGB, no masks)
/// as JPEG (quality `jpeg_quality`, otherwise 95 like Ghostscript `-dJPEGQ`).
fn force_jpeg(doc: &mut Document, o: &OptimizeOptions) -> usize {
    let (ids, masks) = pdfimage::image_ids(doc);
    let q = if o.jpeg_quality > 0 { o.jpeg_quality } else { 95 };
    let mut n = 0;
    for id in ids {
        if masks.contains(&id) {
            continue;
        }
        let Ok(Object::Stream(s)) = doc.get_object(id) else { continue };
        let Some(px) = pdfimage::decode(doc, s) else { continue };
        if px.was_jpeg || px.bpc != 8 || matches!(px.cs, ColorSpace::Indexed { .. }) {
            continue;
        }
        let Some(e) = pdfimage::encode_jpeg(&px, q) else { continue };
        if let Ok(Object::Stream(s)) = doc.get_object_mut(id) {
            pdfimage::apply(s, e);
            n += 1;
        }
    }
    n
}

/// Shrink images in the document. `allow_cs_change`: color space may change (RGB→gray/Indexed);
/// with `UseDeviceIndependentColor` (ICC images) it stays, because only device spaces are reduced.
pub fn optimize_images(doc: &mut Document, o: &OptimizeOptions) -> OptimizeReport {
    let mut r = OptimizeReport::default();
    if o.force_jpeg {
        r.jpeg_recoded += force_jpeg(doc, o);
    }
    if o.level == 0 {
        return r;
    }
    let (ids, masks) = pdfimage::image_ids(doc);
    let total = |doc: &Document| -> usize {
        ids.iter().filter_map(|id| doc.get_object(*id).ok()).filter_map(|o| o.as_stream().ok()).map(|s| s.content.len()).sum()
    };
    r.bytes_before = total(doc);
    for &id in &ids {
        let Ok(Object::Stream(s)) = doc.get_object(id) else { continue };
        r.images_seen += 1;
        let before = s.content.len();
        let f = pdfimage::filters(&s.dict);
        let is_mask = masks.contains(&id);
        let image_mask = matches!(s.dict.get(b"ImageMask"), Ok(Object::Boolean(true)));
        let w = s.dict.get(b"Width").ok().and_then(|v| v.as_i64().ok()).unwrap_or(0);
        let h = s.dict.get(b"Height").ok().and_then(|v| v.as_i64().ok()).unwrap_or(0);
        // 1-bit masks (ImageMask): bits unchanged to JBIG2.
        if image_mask {
            if f.iter().any(|n| matches!(n.as_slice(), b"JBIG2Decode" | b"CCITTFaxDecode" | b"DCTDecode" | b"JPXDecode")) || s.dict.has(b"Decode") {
                continue;
            }
            let Ok(data) = s.decompressed_content_with_limit(pdfimage::MAX_DECODED) else { continue };
            let (Ok(wu), Ok(hu)) = (u32::try_from(w), u32::try_from(h)) else { continue };
            let p = Pixels { width: wu, height: hu, bpc: 1, cs: ColorSpace::Gray, data, was_jpeg: false };
            if p.data.len() < (wu as usize).div_ceil(8) * hu as usize {
                continue;
            }
            if let Some(e) = jbig2(&p).filter(|e| e.content.len() < before)
                && let Ok(Object::Stream(s)) = doc.get_object_mut(id)
            {
                s.dict.set("Filter", e.filter);
                s.dict.remove(b"DecodeParms");
                s.set_content(e.content);
                s.allows_compression = false;
                r.jbig2 += 1;
            }
            continue;
        }
        let src_cs = s.dict.get(b"ColorSpace").map(|c| pdfimage::parse_colorspace(doc, c)).unwrap_or(ColorSpace::Other);
        let is_dct = f.last().is_some_and(|l| l.as_slice() == b"DCTDecode");
        // JPEG below level 2 or CMYK JPEG: do not decode, only Flate over it (lossless).
        if is_dct && (o.level < 2 || src_cs.device() == Some(ColorSpace::Cmyk) || is_mask) {
            if f.len() == 1
                && (o.level == 3 || (w < FLATE_JPEG_THRESHOLD && h < FLATE_JPEG_THRESHOLD))
                && let Ok(Object::Stream(s)) = doc.get_object_mut(id)
            {
                let z = pdfimage::zlib(&s.content, o.complevel());
                if z.len() < s.content.len() {
                    s.dict.set("Filter", Object::Array(vec![Object::Name(b"FlateDecode".to_vec()), Object::Name(b"DCTDecode".to_vec())]));
                    s.set_content(z);
                    s.allows_compression = false;
                    r.jpeg_deflated += 1;
                }
            }
            continue;
        }
        let Some(px) = pdfimage::decode(doc, s) else { continue };
        let device_cs = matches!(px.cs, ColorSpace::Gray | ColorSpace::Rgb);
        let mut best: Option<Encoded> = None;
        if px.was_jpeg {
            // Level ≥ 2: re-encode JPEG with jpeg_quality (not for masks)
            if o.level >= 2 && !is_mask {
                let q = o.effective_jpeg_quality();
                if let Some(e) = pdfimage::encode_jpeg(&px, q) {
                    best = Some(e);
                }
            }
        } else {
            let mut e = best_lossless(&px, o, device_cs && !is_mask);
            if o.level >= 2
                && !is_mask
                && px.cs == ColorSpace::Rgb
                && e.bpc == 8
                && !matches!(e.cs, Object::Array(_))
                && let Some(q) = quantize(&px, o.effective_png_quality())
            {
                let qe = pdfimage::encode_flate(&q, o.complevel());
                if qe.content.len() < e.content.len() {
                    e = qe;
                    r.quantized += 1;
                }
            }
            best = Some(e);
        }
        // Flate over JPEG (lossless), levels 1–2 only up to 10000 px edge length
        let jpeg_deflate = |content: &[u8]| -> Option<Vec<u8>> {
            let allowed = o.level == 3 || (w < FLATE_JPEG_THRESHOLD && h < FLATE_JPEG_THRESHOLD);
            if !allowed {
                return None;
            }
            let z = pdfimage::zlib(content, o.complevel());
            (z.len() < content.len()).then_some(z)
        };
        let Ok(Object::Stream(s)) = doc.get_object_mut(id) else { continue };
        match best {
            Some(e) if e.content.len() < before || (px.was_jpeg && e.filter == Object::Name(b"DCTDecode".to_vec())) => {
                let is_dct = e.filter == Object::Name(b"DCTDecode".to_vec());
                let mut final_len = e.content.len();
                let mut deflated = None;
                if is_dct && let Some(z) = jpeg_deflate(&e.content) {
                    final_len = z.len();
                    deflated = Some(z);
                }
                if final_len >= before {
                    // Not smaller: only try Flate over the original JPEG.
                    if px.was_jpeg && f.len() == 1 && let Some(z) = jpeg_deflate(&s.content) {
                        s.dict.set("Filter", Object::Array(vec![Object::Name(b"FlateDecode".to_vec()), Object::Name(b"DCTDecode".to_vec())]));
                        s.dict.remove(b"DecodeParms");
                        s.set_content(z);
                        s.allows_compression = false;
                        r.jpeg_deflated += 1;
                    }
                    continue;
                }
                let kind = if e.filter == Object::Name(b"JBIG2Decode".to_vec()) { 1 } else if is_dct { 2 } else { 0 };
                pdfimage::apply(s, e);
                if let Some(z) = deflated {
                    s.dict.set("Filter", Object::Array(vec![Object::Name(b"FlateDecode".to_vec()), Object::Name(b"DCTDecode".to_vec())]));
                    s.set_content(z);
                    r.jpeg_deflated += 1;
                }
                match kind {
                    1 => r.jbig2 += 1,
                    2 => r.jpeg_recoded += 1,
                    _ => r.flate += 1,
                }
            }
            _ => {
                if px.was_jpeg && f.len() == 1 && let Some(z) = jpeg_deflate(&s.content) {
                    s.dict.set("Filter", Object::Array(vec![Object::Name(b"FlateDecode".to_vec()), Object::Name(b"DCTDecode".to_vec())]));
                    s.dict.remove(b"DecodeParms");
                    s.set_content(z);
                    s.allows_compression = false;
                    r.jpeg_deflated += 1;
                }
            }
        }
    }
    r.bytes_after = total(doc);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(w: u32, h: u32, mut f: impl FnMut(u32, u32) -> [u8; 3]) -> Pixels {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&f(x, y));
            }
        }
        Pixels { width: w, height: h, bpc: 8, cs: ColorSpace::Rgb, data, was_jpeg: false }
    }

    #[test]
    fn reductions() {
        let g = rgb(10, 10, |x, _| [if x < 5 { 0 } else { 255 }; 3]);
        let gray = reduce_gray(&g).unwrap();
        assert_eq!(gray.cs, ColorSpace::Gray);
        let bit = reduce_bitonal(&gray).unwrap();
        assert_eq!(bit.bpc, 1);
        assert_eq!(bit.data.len(), 2 * 10);
        assert_eq!(bit.data[0], 0b0000_0111);
        let c = rgb(4, 4, |x, y| [x as u8 * 10, y as u8 * 10, 7]);
        let pal = reduce_palette(&c).unwrap();
        let back = pdfimage::expand_indexed(&pal).unwrap();
        assert_eq!(back.data, c.data);
        assert!(reduce_gray(&c).is_none());
    }

    #[test]
    fn quantize_quality_gate() {
        // gradient with few colors: fits
        let smooth = rgb(64, 64, |x, y| [(x * 4) as u8, (y * 4) as u8, 128]);
        assert!(quantize(&smooth, 70).is_some());
        // noise: deviation too large at high quality
        let mut s = 1u32;
        let noise = rgb(64, 64, |_, _| {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            [(s >> 16) as u8, (s >> 8) as u8, s as u8]
        });
        assert!(quantize(&noise, 95).is_none());
    }
}
