//! Read and re-encode image XObjects of a PDF (for color conversion [`crate::color`],
//! archive size reduction [`crate::optimize`] and PDF/A cleanup [`crate::pdfa`]).
//!
//! Supported: 8-bit gray/RGB/CMYK (Device*, ICCBased with N 1/3/4, Indexed over these spaces) and
//! 1-bit gray, filters Flate/LZW/RunLength/ASCIIHex/ASCII85 (via lopdf, incl. predictors) and
//! DCTDecode (JPEG via `image`). JPX, JBIG2 and CCITT are not decoded (stay unchanged).

use std::io::Write as _;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// Upper limit for decoded image data (protection against decompression bombs).
pub const MAX_DECODED: usize = 512 * 1024 * 1024;

/// Color space of an image (simplified).
#[derive(Debug, Clone, PartialEq)]
pub enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
    /// `[/ICCBased s]` with N components.
    Icc {
        n: u8,
        stream: Object,
    },
    /// `[/Indexed base hival lookup]` (Basis Gray/RGB/CMYK/ICC).
    Indexed {
        base: Box<ColorSpace>,
        hival: u8,
        lookup: Vec<u8>,
    },
    /// Everything else (Lab, Cal*, Separation, DeviceN, Pattern, missing …).
    Other,
}

impl ColorSpace {
    pub fn components(&self) -> u8 {
        match self {
            ColorSpace::Gray => 1,
            ColorSpace::Rgb => 3,
            ColorSpace::Cmyk => 4,
            ColorSpace::Icc { n, .. } => *n,
            ColorSpace::Indexed { .. } => 1,
            ColorSpace::Other => 0,
        }
    }

    /// Device color space that is equivalent for conversions (ICC by component count).
    pub fn device(&self) -> Option<ColorSpace> {
        match self {
            ColorSpace::Gray | ColorSpace::Rgb | ColorSpace::Cmyk => Some(self.clone()),
            ColorSpace::Icc { n: 1, .. } => Some(ColorSpace::Gray),
            ColorSpace::Icc { n: 3, .. } => Some(ColorSpace::Rgb),
            ColorSpace::Icc { n: 4, .. } => Some(ColorSpace::Cmyk),
            _ => None,
        }
    }

    pub fn to_object(&self) -> Object {
        match self {
            ColorSpace::Gray => Object::Name(b"DeviceGray".to_vec()),
            ColorSpace::Rgb => Object::Name(b"DeviceRGB".to_vec()),
            ColorSpace::Cmyk => Object::Name(b"DeviceCMYK".to_vec()),
            ColorSpace::Icc { stream, .. } => Object::Array(vec![Object::Name(b"ICCBased".to_vec()), stream.clone()]),
            ColorSpace::Indexed { base, hival, lookup } => Object::Array(vec![
                Object::Name(b"Indexed".to_vec()),
                base.to_object(),
                Object::Integer(i64::from(*hival)),
                Object::String(lookup.clone(), lopdf::StringFormat::Hexadecimal),
            ]),
            ColorSpace::Other => Object::Null,
        }
    }
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

/// Evaluate a color space object (resource names are resolved by the caller).
pub fn parse_colorspace(doc: &Document, o: &Object) -> ColorSpace {
    match deref(doc, o) {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" => ColorSpace::Gray,
            b"DeviceRGB" | b"RGB" => ColorSpace::Rgb,
            b"DeviceCMYK" | b"CMYK" => ColorSpace::Cmyk,
            _ => ColorSpace::Other,
        },
        Object::Array(a) => {
            let Some(Object::Name(kind)) = a.first().map(|o| deref(doc, o)) else { return ColorSpace::Other };
            match kind.as_slice() {
                b"ICCBased" => {
                    let Some(s) = a.get(1) else { return ColorSpace::Other };
                    let n = deref(doc, s).as_stream().ok().and_then(|st| st.dict.get(b"N").ok()).and_then(|n| n.as_i64().ok());
                    match n {
                        Some(n @ (1 | 3 | 4)) => ColorSpace::Icc { n: n as u8, stream: s.clone() },
                        _ => ColorSpace::Other,
                    }
                }
                b"Indexed" | b"I" => {
                    let base = a.get(1).map(|b| parse_colorspace(doc, b)).unwrap_or(ColorSpace::Other);
                    let hival = a.get(2).and_then(|h| deref(doc, h).as_i64().ok()).unwrap_or(-1);
                    let lookup = match a.get(3).map(|l| deref(doc, l)) {
                        Some(Object::String(s, _)) => Some(s.clone()),
                        Some(Object::Stream(st)) => st.decompressed_content_with_limit(1 << 20).ok(),
                        _ => None,
                    };
                    match (base.device(), lookup) {
                        (Some(_), Some(lookup)) if (0..=255).contains(&hival) => {
                            ColorSpace::Indexed { base: Box::new(base), hival: hival as u8, lookup }
                        }
                        _ => ColorSpace::Other,
                    }
                }
                b"DeviceGray" => ColorSpace::Gray,
                b"DeviceRGB" => ColorSpace::Rgb,
                b"DeviceCMYK" => ColorSpace::Cmyk,
                _ => ColorSpace::Other,
            }
        }
        _ => ColorSpace::Other,
    }
}

/// Decoded image.
#[derive(Debug, Clone, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bpc: u8,
    pub cs: ColorSpace,
    /// 8 bit: `width*height*comps` bytes; 1 bit: packed rows (MSB first, padded to bytes).
    pub data: Vec<u8>,
    /// Source was JPEG (DCTDecode).
    pub was_jpeg: bool,
}

impl Pixels {
    pub fn comps(&self) -> usize {
        usize::from(self.cs.components().max(1))
    }
}

/// Filter list of a stream (names).
pub fn filters(dict: &Dictionary) -> Vec<Vec<u8>> {
    match dict.get(b"Filter") {
        Ok(Object::Name(n)) => vec![n.clone()],
        Ok(Object::Array(a)) => a.iter().filter_map(|o| o.as_name().ok().map(<[u8]>::to_vec)).collect(),
        _ => Vec::new(),
    }
}

/// Decode an image XObject. `None`: not supported (stays unchanged).
pub fn decode(doc: &Document, s: &Stream) -> Option<Pixels> {
    let d = &s.dict;
    if d.has(b"Decode") || matches!(d.get(b"ImageMask"), Ok(Object::Boolean(true))) {
        return None;
    }
    let width = u32::try_from(d.get(b"Width").ok()?.as_i64().ok()?).ok()?;
    let height = u32::try_from(d.get(b"Height").ok()?.as_i64().ok()?).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let cs = parse_colorspace(doc, d.get(b"ColorSpace").ok()?);
    if cs == ColorSpace::Other {
        return None;
    }
    let f = filters(d);
    let is_jpeg = f.last().is_some_and(|l| l.as_slice() == b"DCTDecode");
    if is_jpeg {
        // Remove preceding filters (e.g. Flate over JPEG) first.
        let jpeg = if f.len() > 1 {
            let mut tmp = s.clone();
            let mut rest: Vec<Object> = f[..f.len() - 1].iter().map(|n| Object::Name(n.clone())).collect();
            if rest.len() == 1 {
                tmp.dict.set("Filter", rest.remove(0));
            } else {
                tmp.dict.set("Filter", Object::Array(rest));
            }
            tmp.decompressed_content_with_limit(MAX_DECODED).ok()?
        } else {
            s.content.clone()
        };
        let img = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).ok()?;
        if img.width() != width || img.height() != height {
            return None;
        }
        let (cs, data) = match (cs.device()?, img.color()) {
            (ColorSpace::Gray, image::ColorType::L8) => (cs, img.into_luma8().into_raw()),
            // CMYK JPEG: zune-jpeg returns RGB → color space becomes RGB.
            (ColorSpace::Cmyk, _) => (ColorSpace::Rgb, img.into_rgb8().into_raw()),
            (ColorSpace::Rgb, _) => (cs, img.into_rgb8().into_raw()),
            (ColorSpace::Gray, _) => (cs, img.into_luma8().into_raw()),
            _ => return None,
        };
        return Some(Pixels { width, height, bpc: 8, cs, data, was_jpeg: true });
    }
    if f.iter().any(|n| matches!(n.as_slice(), b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode" | b"Crypt")) {
        return None;
    }
    let bpc = d.get(b"BitsPerComponent").ok()?.as_i64().ok()?;
    let comps = usize::from(cs.components());
    let data = s.decompressed_content_with_limit(MAX_DECODED).ok()?;
    let (w, h) = (width as usize, height as usize);
    match bpc {
        8 => {
            let need = w.checked_mul(h)?.checked_mul(comps)?;
            if data.len() < need {
                return None;
            }
            let mut data = data;
            data.truncate(need);
            Some(Pixels { width, height, bpc: 8, cs, data, was_jpeg: false })
        }
        1 if comps == 1 && !matches!(cs, ColorSpace::Indexed { .. }) => {
            let need = w.div_ceil(8).checked_mul(h)?;
            if data.len() < need {
                return None;
            }
            let mut data = data;
            data.truncate(need);
            Some(Pixels { width, height, bpc: 1, cs, data, was_jpeg: false })
        }
        _ => None,
    }
}

/// Indexed → base color space (8 bit).
pub fn expand_indexed(p: &Pixels) -> Option<Pixels> {
    let ColorSpace::Indexed { base, hival, lookup } = &p.cs else { return Some(p.clone()) };
    let n = usize::from(base.components());
    let mut out = Vec::with_capacity(p.data.len() * n);
    for &i in &p.data {
        let i = usize::from(i.min(*hival));
        let e = lookup.get(i * n..i * n + n)?;
        out.extend_from_slice(e);
    }
    Some(Pixels { cs: (**base).clone(), data: out, ..p.clone() })
}

#[inline]
pub fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 + 500) / 1000) as u8
}

/// Simple CMYK→RGB conversion (like Ghostscript without color management: R = (1−C)(1−K)).
#[inline]
pub fn cmyk_to_rgb(c: u8, m: u8, y: u8, k: u8) -> [u8; 3] {
    let k = 255 - u32::from(k);
    [c, m, y].map(|v| ((255 - u32::from(v)) * k / 255) as u8)
}

#[inline]
pub fn rgb_to_cmyk(r: u8, g: u8, b: u8) -> [u8; 4] {
    let k = 255 - r.max(g).max(b);
    if k == 255 {
        return [0, 0, 0, 255];
    }
    let d = 255 - u32::from(k);
    let c = |v: u8| (((255 - u32::from(v)) - u32::from(k)) * 255 / d) as u8;
    [c(r), c(g), c(b), k]
}

/// Convert an 8-bit image into a device color space (Gray/Rgb/Cmyk).
pub fn convert(p: &Pixels, target: &ColorSpace) -> Option<Pixels> {
    if p.bpc != 8 {
        return None;
    }
    let p = expand_indexed(p)?;
    let src = p.cs.device()?;
    if &src == target {
        return Some(Pixels { cs: target.clone(), ..p });
    }
    let n = usize::from(src.components());
    let mut out = Vec::with_capacity(p.data.len() / n * usize::from(target.components()));
    for px in p.data.chunks_exact(n) {
        let rgb = match src {
            ColorSpace::Gray => [px[0]; 3],
            ColorSpace::Rgb => [px[0], px[1], px[2]],
            ColorSpace::Cmyk => cmyk_to_rgb(px[0], px[1], px[2], px[3]),
            _ => return None,
        };
        match target {
            ColorSpace::Gray => out.push(luma(rgb[0], rgb[1], rgb[2])),
            ColorSpace::Rgb => out.extend_from_slice(&rgb),
            ColorSpace::Cmyk => out.extend_from_slice(&rgb_to_cmyk(rgb[0], rgb[1], rgb[2])),
            _ => return None,
        }
    }
    Some(Pixels { cs: target.clone(), data: out, ..p })
}

/// PNG row filter (predictor 15) with per-row selection (smallest sum of magnitudes, like libpng).
pub fn png_predict(data: &[u8], row_bytes: usize, bpp: usize) -> Vec<u8> {
    if row_bytes == 0 {
        return Vec::new();
    }
    let rows = data.len() / row_bytes;
    let mut out = Vec::with_capacity(data.len() + rows);
    let zero = vec![0u8; row_bytes];
    let mut cand: [Vec<u8>; 5] = Default::default();
    for y in 0..rows {
        let cur = &data[y * row_bytes..(y + 1) * row_bytes];
        let prev = if y == 0 { &zero[..] } else { &data[(y - 1) * row_bytes..y * row_bytes] };
        for (f, c) in cand.iter_mut().enumerate() {
            c.clear();
            for i in 0..row_bytes {
                let a = if i >= bpp { cur[i - bpp] } else { 0 };
                let b = prev[i];
                let cc = if i >= bpp { prev[i - bpp] } else { 0 };
                let pred = match f {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    _ => {
                        let p = i16::from(a) + i16::from(b) - i16::from(cc);
                        let (pa, pb, pc) = ((p - i16::from(a)).abs(), (p - i16::from(b)).abs(), (p - i16::from(cc)).abs());
                        if pa <= pb && pa <= pc {
                            a
                        } else if pb <= pc {
                            b
                        } else {
                            cc
                        }
                    }
                };
                c.push(cur[i].wrapping_sub(pred));
            }
        }
        let score = |v: &Vec<u8>| v.iter().map(|&x| u64::from((x as i8).unsigned_abs())).sum::<u64>();
        let (best, _) = cand.iter().enumerate().map(|(i, c)| (i, score(c))).min_by_key(|(_, s)| *s).unwrap_or((0, 0));
        out.push(best as u8);
        out.extend_from_slice(&cand[best]);
    }
    out
}

pub fn zlib(data: &[u8], level: u32) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 4), flate2::Compression::new(level));
    let _ = enc.write_all(data);
    enc.finish().unwrap_or_default()
}

/// Re-encoded image: stream content + entries to set.
#[derive(Debug, Clone)]
pub struct Encoded {
    pub content: Vec<u8>,
    pub filter: Object,
    pub decode_parms: Option<Dictionary>,
    pub bpc: u8,
    pub cs: Object,
}

/// Flate with PNG predictor (8 or 1 bit).
pub fn encode_flate(p: &Pixels, level: u32) -> Encoded {
    let (row_bytes, bpp) = if p.bpc == 1 { ((p.width as usize).div_ceil(8), 1) } else { (p.width as usize * p.comps(), p.comps()) };
    let pred = png_predict(&p.data, row_bytes, bpp);
    let with_pred = zlib(&pred, level);
    // 8 bit: predictor practically always wins for scans/photos – save the second compression.
    let plain = if p.bpc == 8 { Vec::new() } else { zlib(&p.data, level) };
    let colors = if p.bpc == 1 { 1 } else { p.comps() };
    if plain.is_empty() || with_pred.len() < plain.len() {
        let mut parms = Dictionary::new();
        parms.set("Predictor", 15);
        parms.set("Colors", colors as i64);
        parms.set("BitsPerComponent", i64::from(p.bpc));
        parms.set("Columns", i64::from(p.width));
        Encoded {
            content: with_pred,
            filter: Object::Name(b"FlateDecode".to_vec()),
            decode_parms: Some(parms),
            bpc: p.bpc,
            cs: p.cs.to_object(),
        }
    } else {
        Encoded { content: plain, filter: Object::Name(b"FlateDecode".to_vec()), decode_parms: None, bpc: p.bpc, cs: p.cs.to_object() }
    }
}

/// JPEG (gray or RGB, 8 bit).
pub fn encode_jpeg(p: &Pixels, quality: u8) -> Option<Encoded> {
    let color = match p.cs.device()? {
        ColorSpace::Gray => image::ExtendedColorType::L8,
        ColorSpace::Rgb => image::ExtendedColorType::Rgb8,
        _ => return None,
    };
    if p.bpc != 8 || matches!(p.cs, ColorSpace::Indexed { .. }) {
        return None;
    }
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
    enc.encode(&p.data, p.width, p.height, color).ok()?;
    Some(Encoded { content: buf, filter: Object::Name(b"DCTDecode".to_vec()), decode_parms: None, bpc: 8, cs: p.cs.to_object() })
}

/// Write the new encoding into the stream (lopdf sets the length when saving).
pub fn apply(s: &mut Stream, e: Encoded) {
    s.dict.set("Filter", e.filter);
    match e.decode_parms {
        Some(p) => s.dict.set("DecodeParms", p),
        None => {
            s.dict.remove(b"DecodeParms");
        }
    }
    s.dict.set("BitsPerComponent", i64::from(e.bpc));
    s.dict.set("ColorSpace", e.cs);
    s.set_content(e.content);
    s.allows_compression = false;
}

/// All image XObjects (object numbers) and the images used as `/SMask`/`/Mask`.
pub fn image_ids(doc: &Document) -> (Vec<ObjectId>, std::collections::HashSet<ObjectId>) {
    let mut ids = Vec::new();
    let mut masks = std::collections::HashSet::new();
    for (id, o) in &doc.objects {
        if let Object::Stream(s) = o
            && matches!(s.dict.get(b"Subtype"), Ok(Object::Name(n)) if n.as_slice() == b"Image")
        {
            ids.push(*id);
            for k in [b"SMask".as_slice(), b"Mask"] {
                if let Ok(Object::Reference(m)) = s.dict.get(k) {
                    masks.insert(*m);
                }
            }
        }
    }
    (ids, masks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_math() {
        assert_eq!(cmyk_to_rgb(0, 0, 0, 0), [255, 255, 255]);
        assert_eq!(cmyk_to_rgb(0, 0, 0, 255), [0, 0, 0]);
        assert_eq!(cmyk_to_rgb(255, 0, 0, 0), [0, 255, 255]);
        assert_eq!(rgb_to_cmyk(255, 255, 255), [0, 0, 0, 0]);
        assert_eq!(rgb_to_cmyk(0, 0, 0), [0, 0, 0, 255]);
        assert_eq!(rgb_to_cmyk(0, 255, 255), [255, 0, 0, 0]);
        let c = rgb_to_cmyk(10, 200, 90);
        let back = cmyk_to_rgb(c[0], c[1], c[2], c[3]);
        for i in 0..3 {
            assert!(back[i].abs_diff([10, 200, 90][i]) <= 2);
        }
    }

    #[test]
    fn png_predictor_roundtrip_via_lopdf() {
        let (w, h) = (37u32, 11u32);
        let data: Vec<u8> = (0..w * h * 3).map(|i| ((i * 7) % 251) as u8).collect();
        let p = Pixels { width: w, height: h, bpc: 8, cs: ColorSpace::Rgb, data: data.clone(), was_jpeg: false };
        let e = encode_flate(&p, 6);
        let mut s = Stream::new(Dictionary::new(), Vec::new());
        s.dict.set("Subtype", Object::Name(b"Image".to_vec()));
        s.dict.set("Width", i64::from(w));
        s.dict.set("Height", i64::from(h));
        apply(&mut s, e);
        let doc = Document::with_version("1.7");
        let back = decode(&doc, &s).unwrap();
        assert_eq!(back.data, data);
    }
}
