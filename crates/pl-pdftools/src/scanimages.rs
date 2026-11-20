//! PaperRust: in scans redact only the image area under a redaction mark (test log
//! 2026-11-16 C-068).
//!
//! `pdfcraft_redact::apply` deletes pixels under a mark only in images that it can
//! unpack itself (Flate, LZW, CCITT, JBIG2 …). JPEG images (`DCTDecode`, the usual format of
//! scans) it removes **entirely** – a page with one redacted word became a white
//! page with a bar. Therefore here, in the child process only:
//!
//! 1. [`prepare`] before `apply`: every JPEG image (gray/RGB, 8 bit, without masks/`/Decode`) on the
//!    pages with marks is unpacked with the `image` crate and stored as uncompressed pixels
//!    (Flate) under the same object number, marked with `/PaperRustRecode`. PdfCraft
//!    then only deletes the pixels under the marks in it (`images_cleared`) and creates a
//!    new image for that (the dictionary including the marker is carried over).
//! 2. [`finish`] after `apply`: images that PdfCraft did not touch get their
//!    original JPEG data back (unchanged, no quality loss); the new, redacted
//!    images are stored as JPEG again (from the already deleted pixels – nothing of the
//!    original content under the mark remains).
//!
//! Anything that does not fit here (CMYK, masks, errors on unpacking) stays as it is; PdfCraft
//! then still removes such images under a mark entirely (safe, but coarse).

use std::collections::HashSet;
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

/// Marker in the image dictionary (removed again in [`finish`]).
const MARK: &[u8] = b"PaperRustRecode";
/// At most this many pixels per image (PdfCraft does not delete larger images anyway).
const MAX_PIXELS: u64 = pdfcraft_cos::MAX_REDACTED_IMAGE_PIXELS;
/// JPEG quality of the newly stored, redacted images.
const JPEG_QUALITY: u8 = 90;
/// Nesting depth of form XObjects in which images are searched.
const MAX_FORM_DEPTH: usize = 8;

/// Converted images with their original objects.
#[derive(Default)]
pub struct Prepared {
    originals: Vec<(ObjRef, Arc<Object>)>,
}

impl Prepared {
    /// Number of JPEG images unpacked for PdfCraft.
    pub fn len(&self) -> usize {
        self.originals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.originals.is_empty()
    }
}

fn resolved_dict(doc: &Document, o: Option<&Object>) -> Option<Dict> {
    o.and_then(|o| doc.resolve(o).as_dict().cloned())
}

/// All image XObjects named by the resources `res` (and forms contained in them).
fn collect_images(doc: &Document, res: &Dict, depth: usize, seen: &mut HashSet<ObjRef>, out: &mut Vec<ObjRef>) {
    let Some(xobjects) = resolved_dict(doc, res.get(b"XObject")) else { return };
    for (_, v) in xobjects.iter() {
        let Some(r) = v.as_ref() else { continue };
        if !seen.insert(r) {
            continue;
        }
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        match s.dict.name(b"Subtype") {
            Some(b"Image") => out.push(r),
            Some(b"Form") if depth < MAX_FORM_DEPTH => {
                if let Some(inner) = resolved_dict(doc, s.dict.get(b"Resources")) {
                    collect_images(doc, &inner, depth + 1, seen, out);
                }
            }
            _ => {}
        }
    }
}

/// Components of the colour space (gray and RGB only, also as ICC profile).
fn components(doc: &Document, cs: Option<&Object>) -> Option<usize> {
    let cs = doc.resolve(cs?);
    match &*cs {
        Object::Name(n) if n.as_slice() == b"DeviceGray" || n.as_slice() == b"G" => Some(1),
        Object::Name(n) if n.as_slice() == b"DeviceRGB" || n.as_slice() == b"RGB" => Some(3),
        Object::Array(a) if a.first().and_then(Object::as_name) == Some(b"ICCBased".as_slice()) => {
            let profile = doc.resolve(a.get(1)?);
            let Object::Stream(p) = &*profile else { return None };
            match p.dict.get(b"N").map(|n| doc.resolve(n)).and_then(|n| n.as_int()) {
                Some(1) => Some(1),
                Some(3) => Some(3),
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_dct(s: &Stream) -> bool {
    match s.dict.get(b"Filter") {
        Some(Object::Name(n)) => n.as_slice() == b"DCTDecode" || n.as_slice() == b"DCT",
        Some(Object::Array(a)) => a.len() == 1 && matches!(a[0].as_name(), Some(b"DCTDecode") | Some(b"DCT")),
        _ => false,
    }
}

/// JPEG image `s` as pixels (rows without padding), if it can be converted safely.
fn jpeg_pixels(doc: &Document, s: &Stream) -> Option<Vec<u8>> {
    if !is_dct(s)
        || s.dict.contains(b"SMask")
        || s.dict.contains(b"Mask")
        || s.dict.contains(b"Decode")
        || s.dict.contains(b"DecodeParms")
        || s.dict.contains(b"DP")
        || s.dict.contains(b"F")
        || matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)))
    {
        return None;
    }
    if s.dict.get(b"BitsPerComponent").map(|b| doc.resolve(b)).and_then(|b| b.as_int()) != Some(8) {
        return None;
    }
    let n = components(doc, s.dict.get(b"ColorSpace"))?;
    let w = s.dict.get(b"Width").map(|v| doc.resolve(v)).and_then(|v| v.as_int())?;
    let h = s.dict.get(b"Height").map(|v| doc.resolve(v)).and_then(|v| v.as_int())?;
    if w <= 0 || h <= 0 || (w as u64).saturating_mul(h as u64) > MAX_PIXELS {
        return None;
    }
    let img = image::load_from_memory_with_format(&s.raw, image::ImageFormat::Jpeg).ok()?;
    if (i64::from(img.width()), i64::from(img.height())) != (w, h) {
        return None;
    }
    let data = match (n, img) {
        (1, image::DynamicImage::ImageLuma8(g)) => g.into_raw(),
        (3, image::DynamicImage::ImageRgb8(c)) => c.into_raw(),
        // gray stored as RGB or similar: channel count does not match the colour space – do not touch
        _ => return None,
    };
    (data.len() as u64 == (w as u64) * (h as u64) * n as u64).then_some(data)
}

/// Step 1: unpack JPEG images on the pages `pages` (0-based) for PdfCraft.
pub fn prepare(doc: &mut Document, pages: &[usize]) -> Prepared {
    let all = pdfcraft_model::pages(doc);
    let mut seen = HashSet::new();
    let mut images = Vec::new();
    for &i in pages {
        let Some(page) = all.get(i) else { continue };
        if let Some(res) = resolved_dict(doc, page.dict.get(b"Resources")) {
            collect_images(doc, &res, 0, &mut seen, &mut images);
        }
    }
    let mut prepared = Prepared::default();
    for r in images {
        let original = doc.get(r);
        let Object::Stream(s) = &*original else { continue };
        let Some(pixels) = jpeg_pixels(doc, s) else { continue };
        let mut dict = s.dict.clone();
        for key in [b"Filter".as_slice(), b"DecodeParms", b"DP", b"Length"] {
            dict.remove(key);
        }
        dict.set(MARK.to_vec(), Object::Bool(true));
        doc.set(r, Object::Stream(Stream::flate(dict, &pixels)));
        prepared.originals.push((r, original.clone()));
    }
    prepared
}

fn marked(o: &Object) -> Option<&Stream> {
    match o {
        Object::Stream(s) if s.dict.contains(MARK) => Some(s),
        _ => None,
    }
}

/// Redacted image (Flate, from PdfCraft) back to JPEG.
fn as_jpeg(doc: &Document, s: &Stream) -> Option<Stream> {
    let w = u32::try_from(s.dict.int(b"Width")?).ok()?;
    let h = u32::try_from(s.dict.int(b"Height")?).ok()?;
    let n = components(doc, s.dict.get(b"ColorSpace"))?;
    if s.dict.get(b"Filter").and_then(Object::as_name) != Some(b"FlateDecode".as_slice()) {
        return None;
    }
    let need = (w as usize).checked_mul(h as usize)?.checked_mul(n)?;
    let mut data = s.decoded_within(need.checked_add(1)?).ok()?;
    if data.len() < need {
        return None;
    }
    data.truncate(need);
    let color = if n == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY).encode(&data, w, h, color).ok()?;
    let mut dict = s.dict.clone();
    for key in [MARK, b"DecodeParms", b"DP", b"Length"] {
        dict.remove(key);
    }
    dict.set(b"Filter".to_vec(), Object::name("DCTDecode"));
    Some(Stream::from_raw(dict, buf))
}

/// Step 2 (after `apply`): untouched images back to their JPEG data, redacted images stored as
/// JPEG. Returns the number of images newly stored as JPEG.
pub fn finish(doc: &mut Document, prepared: Prepared) -> usize {
    let restored: HashSet<ObjRef> = prepared.originals.iter().map(|(r, _)| *r).collect();
    for (r, original) in prepared.originals {
        if marked(&doc.get(r)).is_some() {
            doc.set(r, (*original).clone());
        }
    }
    let mut recoded = 0;
    for num in doc.object_numbers() {
        let r = ObjRef::new(num, doc.generation(num));
        if restored.contains(&r) {
            continue;
        }
        let obj = doc.get(r);
        let Some(s) = marked(&obj) else { continue };
        match as_jpeg(doc, s) {
            Some(new) => {
                doc.set(r, Object::Stream(new));
                recoded += 1;
            }
            None => {
                // stays Flate (larger, but correct); only the marker has to go
                let mut plain = s.clone();
                plain.dict.remove(MARK);
                doc.set(r, Object::Stream(plain));
            }
        }
    }
    recoded
}
