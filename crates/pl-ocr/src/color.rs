//! `OCR_COLOR_CONVERSION_STRATEGY` (Ghostscript `-sColorConversionStrategy`) without Ghostscript.
//!
//! upstream passes the strategy on to OCRmyPDF/Ghostscript only for PDF/A output
//! (`construct_ocrmypdf_parameters`, `_convert_pdf_to_pdfa`); here it acts on the finished
//! archive PDF:
//! - image XObjects (8-bit gray/RGB/CMYK, Indexed, ICCBased; Flate/LZW/DCT …) are converted,
//!   for Indexed only the color table.
//! - page/form content: color operators `g/G rg/RG k/K` and `cs/CS sc/scn` in device color spaces.
//!
//! | Strategy | Effect |
//! |---|---|
//! | `LeaveColorUnchanged` | nothing; with PDF/A only DeviceCMYK → RGB is converted (otherwise a rule violation with an sRGB OutputIntent) |
//! | `RGB` | CMYK (Device and ICC) → DeviceRGB; gray stays gray |
//! | `Gray` | everything → DeviceGray |
//! | `UseDeviceIndependentColor` | images → `ICCBased` (sRGB or gray profile), CMYK via RGB; vector colors stay Device (permitted with OutputIntent) |
//! | `CMYK` | without PDF/A: RGB → DeviceCMYK. With PDF/A like `RGB` + warning (no CMYK OutputIntent profile available) |

use std::collections::HashMap;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::config::ColorConversion;
use crate::pdfimage::{self, ColorSpace, Pixels};

/// Quality with which converted JPEG images are re-encoded (Ghostscript `-dJPEGQ=95`).
pub const RECODE_JPEG_QUALITY: u8 = 95;

/// Target per color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Keep,
    /// Only DeviceCMYK → RGB (PDF/A with sRGB intent).
    CmykToRgb,
    Rgb,
    Gray,
    Cmyk,
    Icc,
}

/// Effective target for the strategy.
pub fn target_for(strategy: ColorConversion, pdfa: bool) -> Target {
    match strategy {
        ColorConversion::LeaveColorUnchanged => {
            if pdfa { Target::CmykToRgb } else { Target::Keep }
        }
        ColorConversion::Rgb => Target::Rgb,
        ColorConversion::Gray => Target::Gray,
        ColorConversion::UseDeviceIndependentColor => Target::Icc,
        ColorConversion::Cmyk => {
            if pdfa {
                tracing::warn!(
                    "OCR_COLOR_CONVERSION_STRATEGY=CMYK is not supported for PDF/A output (no CMYK output intent profile); converting to RGB instead"
                );
                Target::Rgb
            } else {
                Target::Cmyk
            }
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ColorReport {
    pub images_converted: usize,
    pub streams_rewritten: usize,
}

/// Shared ICC profiles (once per document).
struct Profiles {
    rgb: Option<ObjectId>,
    gray: Option<ObjectId>,
}

impl Profiles {
    fn get(&mut self, doc: &mut Document, n: u8) -> Object {
        let slot = if n == 1 { &mut self.gray } else { &mut self.rgb };
        let id = *slot.get_or_insert_with(|| {
            let mut d = Dictionary::new();
            d.set("N", i64::from(n));
            d.set("Alternate", Object::Name(if n == 1 { b"DeviceGray".to_vec() } else { b"DeviceRGB".to_vec() }));
            let data = if n == 1 { crate::icc::gray_profile() } else { crate::icc::srgb_profile() };
            doc.add_object(Stream::new(d, data))
        });
        Object::Array(vec![Object::Name(b"ICCBased".to_vec()), Object::Reference(id)])
    }
}

/// Target color space for a source color space (`None` = leave unchanged).
fn wanted(src: &ColorSpace, t: Target) -> Option<ColorSpace> {
    let dev = src.device()?;
    let is_icc = matches!(src, ColorSpace::Icc { .. });
    match t {
        Target::Keep => None,
        Target::CmykToRgb => (dev == ColorSpace::Cmyk && !is_icc).then_some(ColorSpace::Rgb),
        Target::Rgb => (dev == ColorSpace::Cmyk).then_some(ColorSpace::Rgb),
        Target::Gray => (dev != ColorSpace::Gray || is_icc).then_some(ColorSpace::Gray),
        Target::Cmyk => (dev == ColorSpace::Rgb).then_some(ColorSpace::Cmyk),
        // ICC: device color spaces → ICC (CMYK via RGB); existing ICC spaces stay.
        Target::Icc => (!is_icc).then(|| if dev == ColorSpace::Cmyk { ColorSpace::Rgb } else { dev }),
    }
}

/// Convert the color table of Indexed.
fn convert_lookup(lookup: &[u8], base: &ColorSpace, to: &ColorSpace) -> Option<Vec<u8>> {
    let n = usize::from(base.components());
    let p = Pixels { width: (lookup.len() / n.max(1)) as u32, height: 1, bpc: 8, cs: base.device()?, data: lookup.to_vec(), was_jpeg: false };
    let mut p = p;
    p.data.truncate(p.width as usize * n);
    Some(pdfimage::convert(&p, to)?.data)
}

fn final_cs(doc: &mut Document, profiles: &mut Profiles, dev: &ColorSpace, t: Target) -> Object {
    if t == Target::Icc {
        profiles.get(doc, dev.components())
    } else {
        dev.to_object()
    }
}

/// Convert images. `masks`: images used as SMask/Mask stay unchanged.
fn convert_images(doc: &mut Document, t: Target, profiles: &mut Profiles) -> usize {
    let (ids, masks) = pdfimage::image_ids(doc);
    let mut n = 0;
    for id in ids {
        if masks.contains(&id) {
            continue;
        }
        let Ok(Object::Stream(s)) = doc.get_object(id) else { continue };
        let Some(cs_obj) = s.dict.get(b"ColorSpace").ok().cloned() else { continue };
        let cs = pdfimage::parse_colorspace(doc, &cs_obj);
        // Indexed: only the table
        if let ColorSpace::Indexed { base, hival, lookup } = &cs {
            let Some(to) = wanted(base, t) else { continue };
            let Some(new_lookup) = convert_lookup(lookup, base, &to) else { continue };
            let base_obj = final_cs(doc, profiles, &to, t);
            let arr = Object::Array(vec![
                Object::Name(b"Indexed".to_vec()),
                base_obj,
                Object::Integer(i64::from(*hival)),
                Object::String(new_lookup, lopdf::StringFormat::Hexadecimal),
            ]);
            if let Ok(Object::Stream(s)) = doc.get_object_mut(id) {
                s.dict.set("ColorSpace", arr);
                n += 1;
            }
            continue;
        }
        let Some(to) = wanted(&cs, t) else { continue };
        let Some(px) = pdfimage::decode(doc, s) else {
            tracing::debug!("color conversion: image {id:?} cannot be decoded, left unchanged");
            continue;
        };
        if px.bpc == 1 {
            // 1-bit gray stays (only color space ICC not needed with UseDeviceIndependentColor).
            continue;
        }
        let Some(conv) = pdfimage::convert(&px, &to) else { continue };
        let was_jpeg = px.was_jpeg;
        let cs_out = final_cs(doc, profiles, &to, t);
        let mut enc = if was_jpeg && to != ColorSpace::Cmyk {
            pdfimage::encode_jpeg(&conv, RECODE_JPEG_QUALITY).unwrap_or_else(|| pdfimage::encode_flate(&conv, 6))
        } else {
            pdfimage::encode_flate(&conv, 6)
        };
        enc.cs = cs_out;
        if let Ok(Object::Stream(s)) = doc.get_object_mut(id) {
            pdfimage::apply(s, enc);
            n += 1;
        }
    }
    n
}

/// Current color space (fill/stroke) in the content stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cur {
    Gray,
    Rgb,
    Cmyk,
    Other,
}

fn nums(ops: &[Object]) -> Option<Vec<f64>> {
    ops.iter()
        .map(|o| match o {
            Object::Integer(i) => Some(*i as f64),
            Object::Real(r) => Some(f64::from(*r)),
            _ => None,
        })
        .collect()
}

fn reals(v: &[f64]) -> Vec<Object> {
    v.iter()
        .map(|x| {
            let r = (x.clamp(0.0, 1.0) * 10000.0).round() / 10000.0;
            if r.fract() == 0.0 { Object::Integer(r as i64) } else { Object::Real(r as f32) }
        })
        .collect()
}

fn to_rgb(cur: Cur, v: &[f64]) -> Option<[f64; 3]> {
    match (cur, v.len()) {
        (Cur::Gray, 1) => Some([v[0]; 3]),
        (Cur::Rgb, 3) => Some([v[0], v[1], v[2]]),
        (Cur::Cmyk, 4) => {
            let k = 1.0 - v[3];
            Some([(1.0 - v[0]) * k, (1.0 - v[1]) * k, (1.0 - v[2]) * k])
        }
        _ => None,
    }
}

fn gray_of(rgb: [f64; 3]) -> f64 {
    0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]
}

fn cmyk_of(rgb: [f64; 3]) -> [f64; 4] {
    let k = 1.0 - rgb[0].max(rgb[1]).max(rgb[2]);
    if k >= 1.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let d = 1.0 - k;
    [(1.0 - rgb[0] - k) / d, (1.0 - rgb[1] - k) / d, (1.0 - rgb[2] - k) / d, k]
}

/// Target color space of the vector color (`None` = unchanged).
fn vector_target(cur: Cur, t: Target) -> Option<Cur> {
    match (t, cur) {
        (_, Cur::Other) => None,
        (Target::Gray, Cur::Rgb | Cur::Cmyk) => Some(Cur::Gray),
        (Target::Rgb | Target::CmykToRgb | Target::Icc, Cur::Cmyk) => Some(Cur::Rgb),
        (Target::Cmyk, Cur::Rgb) => Some(Cur::Cmyk),
        _ => None,
    }
}

fn color_ops(to: Cur, stroke: bool, rgb: [f64; 3]) -> Operation {
    let (op, vals): (&str, Vec<f64>) = match to {
        Cur::Gray => (if stroke { "G" } else { "g" }, vec![gray_of(rgb)]),
        Cur::Rgb => (if stroke { "RG" } else { "rg" }, rgb.to_vec()),
        _ => (if stroke { "K" } else { "k" }, cmyk_of(rgb).to_vec()),
    };
    Operation::new(op, reals(&vals))
}

fn cs_name(c: Cur) -> &'static [u8] {
    match c {
        Cur::Gray => b"DeviceGray",
        Cur::Rgb => b"DeviceRGB",
        _ => b"DeviceCMYK",
    }
}

/// Rewrite color operators of a content stream. `None`: nothing changed.
pub fn rewrite_content(content: &[u8], t: Target, cs_names: &HashMap<Vec<u8>, Cur>) -> Option<Vec<u8>> {
    if t == Target::Keep {
        return None;
    }
    let c = Content::decode(content).ok()?;
    let mut changed = false;
    let mut fill = Cur::Gray;
    let mut stroke = Cur::Gray;
    let mut stack: Vec<(Cur, Cur)> = Vec::new();
    let mut out = Vec::with_capacity(c.operations.len());
    for op in c.operations {
        let s = op.operator.as_str();
        match s {
            "q" => stack.push((fill, stroke)),
            "Q" => {
                if let Some((f, st)) = stack.pop() {
                    fill = f;
                    stroke = st;
                }
            }
            _ => {}
        }
        let is_stroke = s.chars().next().is_some_and(char::is_uppercase);
        let replaced: Option<Operation> = match s {
            "g" | "G" | "rg" | "RG" | "k" | "K" => {
                let cur = match s {
                    "g" | "G" => Cur::Gray,
                    "rg" | "RG" => Cur::Rgb,
                    _ => Cur::Cmyk,
                };
                if is_stroke { stroke = cur } else { fill = cur }
                match (vector_target(cur, t), nums(&op.operands).and_then(|v| to_rgb(cur, &v))) {
                    (Some(to), Some(rgb)) => {
                        if is_stroke { stroke = to } else { fill = to }
                        Some(color_ops(to, is_stroke, rgb))
                    }
                    _ => None,
                }
            }
            "cs" | "CS" => {
                let cur = op
                    .operands
                    .first()
                    .and_then(|o| o.as_name().ok())
                    .map(|n| match n {
                        b"DeviceGray" => Cur::Gray,
                        b"DeviceRGB" => Cur::Rgb,
                        b"DeviceCMYK" => Cur::Cmyk,
                        other => cs_names.get(other).copied().unwrap_or(Cur::Other),
                    })
                    .unwrap_or(Cur::Other);
                match vector_target(cur, t) {
                    Some(to) => {
                        if is_stroke { stroke = to } else { fill = to }
                        Some(Operation::new(s, vec![Object::Name(cs_name(to).to_vec())]))
                    }
                    None => {
                        if is_stroke { stroke = cur } else { fill = cur }
                        None
                    }
                }
            }
            "sc" | "SC" | "scn" | "SCN" => {
                // After a rewritten `cs` there are 4 (CMYK) or 3 (RGB) values in the old space.
                let cur = if is_stroke { stroke } else { fill };
                let v = nums(&op.operands);
                match v {
                    Some(v) if v.len() == 4 && cur != Cur::Cmyk && cur != Cur::Other => {
                        to_rgb(Cur::Cmyk, &v).map(|rgb| color_ops(cur, is_stroke, rgb)).map(|o| Operation::new(s, o.operands))
                    }
                    Some(v) if v.len() == 3 && (cur == Cur::Gray || cur == Cur::Cmyk) => {
                        to_rgb(Cur::Rgb, &v).map(|rgb| color_ops(cur, is_stroke, rgb)).map(|o| Operation::new(s, o.operands))
                    }
                    Some(v) if v.len() == 1 && (cur == Cur::Rgb || cur == Cur::Cmyk) => {
                        to_rgb(Cur::Gray, &v).map(|rgb| color_ops(cur, is_stroke, rgb)).map(|o| Operation::new(s, o.operands))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        match replaced {
            Some(r) => {
                changed = true;
                out.push(r);
            }
            None => out.push(op),
        }
    }
    if !changed {
        return None;
    }
    Content { operations: out }.encode().ok()
}

/// Names from `/Resources /ColorSpace` that point to a device color space.
fn resource_cs_names(doc: &Document, res: Option<&Dictionary>) -> HashMap<Vec<u8>, Cur> {
    let mut m = HashMap::new();
    let Some(d) = res
        .and_then(|r| r.get(b"ColorSpace").ok())
        .and_then(|o| doc.dereference(o).ok().map(|(_, o)| o))
        .and_then(|o| o.as_dict().ok())
    else {
        return m;
    };
    for (k, v) in d.iter() {
        let cur = match pdfimage::parse_colorspace(doc, v) {
            ColorSpace::Gray => Cur::Gray,
            ColorSpace::Rgb => Cur::Rgb,
            ColorSpace::Cmyk => Cur::Cmyk,
            _ => Cur::Other,
        };
        if cur != Cur::Other {
            m.insert(k.clone(), cur);
        }
    }
    m
}

fn page_resources(doc: &Document, page: ObjectId) -> Option<Dictionary> {
    let mut id = page;
    for _ in 0..64 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(r) = d.get(b"Resources") {
            return doc.dereference(r).ok().and_then(|(_, o)| o.as_dict().ok()).cloned();
        }
        id = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// Rewrite content streams (pages, forms, tiling patterns).
fn convert_contents(doc: &mut Document, t: Target) -> usize {
    let mut n = 0;
    // pages
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    for pid in pages {
        let res = page_resources(doc, pid);
        let names = resource_cs_names(doc, res.as_ref());
        let content = doc.get_page_content(pid);
        if let Some(new) = rewrite_content(&content, t, &names) {
            let sid = doc.add_object(Stream::new(Dictionary::new(), new));
            if let Ok(d) = doc.get_dictionary_mut(pid) {
                d.set("Contents", Object::Reference(sid));
                n += 1;
            }
        }
    }
    // form XObjects and tiling patterns
    let forms: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, o)| match o {
            Object::Stream(s)
                if matches!(s.dict.get(b"Subtype"), Ok(Object::Name(n)) if n.as_slice() == b"Form")
                    || matches!(s.dict.get(b"PatternType"), Ok(Object::Integer(1))) =>
            {
                Some(*id)
            }
            _ => None,
        })
        .collect();
    for id in forms {
        let Ok(Object::Stream(s)) = doc.get_object(id) else { continue };
        let res = s.dict.get(b"Resources").ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_dict().ok());
        let names = resource_cs_names(doc, res);
        let Ok(content) = s.decompressed_content_with_limit(pdfimage::MAX_DECODED) else { continue };
        if let Some(new) = rewrite_content(&content, t, &names)
            && let Ok(Object::Stream(s)) = doc.get_object_mut(id)
        {
            s.dict.remove(b"Filter");
            s.dict.remove(b"DecodeParms");
            s.set_content(new);
            s.allows_compression = true;
            n += 1;
        }
    }
    n
}

/// Apply the strategy to the whole document.
pub fn apply(doc: &mut Document, t: Target) -> ColorReport {
    if t == Target::Keep {
        return ColorReport::default();
    }
    let mut profiles = Profiles { rgb: None, gray: None };
    let images_converted = convert_images(doc, t, &mut profiles);
    let streams_rewritten = convert_contents(doc, t);
    ColorReport { images_converted, streams_rewritten }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(c: &[u8], t: Target) -> String {
        String::from_utf8(rewrite_content(c, t, &HashMap::new()).unwrap_or_else(|| c.to_vec())).unwrap()
    }

    #[test]
    fn vector_colors() {
        // CMYK → RGB (PDF/A, strategy unchanged)
        let out = s(b"0 0 0 1 k 0 0 10 10 re f", Target::CmykToRgb);
        assert!(out.contains("0 0 0 rg"), "{out}");
        // Grau
        let out = s(b"1 0 0 rg 0 1 0 RG", Target::Gray);
        assert!(out.contains("0.299 g") && out.contains("0.587 G"), "{out}");
        // cs/sc
        let out = s(b"/DeviceCMYK cs 0 0 0 0 sc", Target::Rgb);
        assert!(out.contains("/DeviceRGB cs") && out.contains("1 1 1 sc"), "{out}");
        // Keep: unchanged
        assert!(rewrite_content(b"0 0 0 1 k", Target::Keep, &HashMap::new()).is_none());
        // RGB stays with target RGB
        assert!(rewrite_content(b"1 0 0 rg", Target::Rgb, &HashMap::new()).is_none());
        // RGB → CMYK
        let out = s(b"1 0 0 rg", Target::Cmyk);
        assert!(out.contains("0 1 1 0 k"), "{out}");
    }

    #[test]
    fn targets() {
        assert_eq!(target_for(ColorConversion::LeaveColorUnchanged, false), Target::Keep);
        assert_eq!(target_for(ColorConversion::LeaveColorUnchanged, true), Target::CmykToRgb);
        assert_eq!(target_for(ColorConversion::Cmyk, true), Target::Rgb);
        assert_eq!(target_for(ColorConversion::Cmyk, false), Target::Cmyk);
        assert_eq!(wanted(&ColorSpace::Rgb, Target::Icc), Some(ColorSpace::Rgb));
        assert_eq!(wanted(&ColorSpace::Cmyk, Target::Icc), Some(ColorSpace::Rgb));
        assert_eq!(wanted(&ColorSpace::Gray, Target::Rgb), None);
        assert_eq!(wanted(&ColorSpace::Rgb, Target::Gray), Some(ColorSpace::Gray));
    }
}
