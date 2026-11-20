// upstream: src/documents/converters.py@v3.4.2 (convert_from_tiff_to_pdf),
// src/documents/utils.py (remove_alpha_channel, copy_basic_file_stats), img2pdf 0.6.3
// (convert, pil_get_dpi, default_dpi = 96, get_imgmetadata: Orientation → Rotation)
//
//! TIFF → PDF (img2pdf replacement): one page per TIFF image, page size = pixels / dpi · 72 pt with
//! the dpi **of the respective image**, lossless (Flate; black and white as 1 bit). Like img2pdf,
//! CCITT Group 4 data (one strip) is taken over **unchanged** (`/CCITTFaxDecode`);
//! additionally (img2pdf re-encodes as Flate here) JPEG data (compression 7, one strip,
//! `JPEGTables` inserted) as `/DCTDecode` – both without re-encoding, so smaller and without
//! loss compared to the TIFF ([`tiff_raw_frames`], gap E18). The
//! TIFF Orientation is taken over as page `/Rotate` like img2pdf (3 → 180, 6 → 90,
//! 8 → 270); mirrored values (2/4/5/7) are corrected in the pixels. Alpha is
//! discarded (not blended), like `remove_alpha_channel`.

use std::io::Write;
use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Object, Stream};

use crate::{PdfOpsError, Result};

/// img2pdf `default_dpi`.
pub const IMG2PDF_DEFAULT_DPI: f64 = 96.0;

/// img2pdf `pil_get_dpi`: rounded; missing/0.0 → 96; TIFF with the Pillow default (1,1) → 96.
pub fn img2pdf_dpi(info: &pl_ocr::imageinfo::ImageInfo) -> (f64, f64) {
    img2pdf_dpi_from(info.dpi)
}

/// [`img2pdf_dpi`] for a single dpi value (e.g. of a TIFF single image).
pub fn img2pdf_dpi_from(dpi: Option<(f64, f64)>) -> (f64, f64) {
    let (x, y) = dpi.unwrap_or((IMG2PDF_DEFAULT_DPI, IMG2PDF_DEFAULT_DPI));
    let r = (pl_ocr::imageinfo::py_round(x), pl_ocr::imageinfo::py_round(y));
    match r {
        (0, 0) | (1, 1) => (IMG2PDF_DEFAULT_DPI, IMG2PDF_DEFAULT_DPI),
        (a, b) => (a.max(1) as f64, b.max(1) as f64),
    }
}

/// Details of a TIFF single image that img2pdf evaluates per page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TiffFrameMeta {
    /// dpi like Pillow (`ResolutionUnit` 2/missing: value, 3: ·2.54, 1: no dpi).
    pub dpi: Option<(f64, f64)>,
    /// EXIF/TIFF-Orientation (1–8, 1 = normal).
    pub orientation: u16,
}

fn rational(v: tiff::decoder::ifd::Value) -> Option<f64> {
    use tiff::decoder::ifd::Value;
    match v {
        Value::Rational(n, d) if d != 0 => Some(f64::from(n) / f64::from(d)),
        Value::Short(n) => Some(f64::from(n)),
        Value::Unsigned(n) => Some(f64::from(n)),
        Value::Float(f) => Some(f64::from(f)),
        Value::Double(f) => Some(f),
        Value::List(mut l) if !l.is_empty() => rational(l.swap_remove(0)),
        _ => None,
    }
}

/// dpi and Orientation per TIFF single image (in file order).
pub fn tiff_frame_meta(data: &[u8]) -> Vec<TiffFrameMeta> {
    use tiff::tags::Tag;
    let mut out = Vec::new();
    let Ok(mut dec) = tiff::decoder::Decoder::new(std::io::Cursor::new(data)) else { return out };
    loop {
        let x = dec.find_tag(Tag::XResolution).ok().flatten().and_then(rational);
        let y = dec.find_tag(Tag::YResolution).ok().flatten().and_then(rational);
        let unit = dec.find_tag(Tag::ResolutionUnit).ok().flatten().and_then(|v| v.into_u16().ok());
        let orientation = dec.find_tag(Tag::Orientation).ok().flatten().and_then(|v| v.into_u16().ok()).unwrap_or(1);
        let dpi = match (x, y, unit) {
            (Some(x), Some(y), None | Some(2)) => Some((x, y)),
            (Some(x), Some(y), Some(3)) => Some((x * 2.54, y * 2.54)),
            _ => None,
        };
        out.push(TiffFrameMeta { dpi, orientation });
        if !dec.more_images() || out.len() >= 10_000 || dec.next_image().is_err() {
            break;
        }
    }
    out
}

/// Image data of a TIFF single image that can be taken over unchanged (gap E18).
#[derive(Debug, Clone, PartialEq)]
pub enum RawImage {
    /// CCITT Group 4 like img2pdf (`/K -1`). `black_is_1`: TIFF `PhotometricInterpretation` 1
    /// (BlackIsZero; libtiff encodes 0 bits as "white" runs) → `/BlackIs1 true`.
    Group4 { width: u32, height: u32, black_is_1: bool, data: Vec<u8> },
    /// Complete JPEG stream (baseline/progressive, 8 bit). `color_transform`: value for
    /// `/DecodeParms /ColorTransform`, if it deviates from the PDF standard (RGB JPEG without YCbCr).
    Jpeg { width: u32, height: u32, components: u8, color_transform: Option<i64>, data: Vec<u8> },
}

impl RawImage {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            RawImage::Group4 { width, height, .. } | RawImage::Jpeg { width, height, .. } => (*width, *height),
        }
    }
}

type TiffDec<'a> = tiff::decoder::Decoder<std::io::Cursor<&'a [u8]>>;

/// Reads the image strip (exactly one, no tiled format) of a single image.
fn single_strip(dec: &mut TiffDec<'_>, data: &[u8], height: u32) -> Option<Vec<u8>> {
    use tiff::tags::Tag;
    if dec.find_tag(Tag::TileWidth).ok().flatten().is_some() {
        return None;
    }
    let offsets = dec.find_tag(Tag::StripOffsets).ok().flatten()?.into_u64_vec().ok()?;
    let counts = dec.find_tag(Tag::StripByteCounts).ok().flatten()?.into_u64_vec().ok()?;
    let rows = dec.find_tag(Tag::RowsPerStrip).ok().flatten().and_then(|v| v.into_u64().ok()).unwrap_or(u64::from(u32::MAX));
    if offsets.len() != 1 || counts.len() != 1 || rows < u64::from(height) {
        return None;
    }
    let start = usize::try_from(offsets[0]).ok()?;
    let len = usize::try_from(counts[0]).ok()?;
    data.get(start..start.checked_add(len)?).map(<[u8]>::to_vec)
}

/// Header of a JPEG stream: (width, height, components, Adobe APP14 transform). Only 8-bit
/// baseline/extended/progressive (Huffman) with SOF before the first SOS; otherwise `None`.
fn jpeg_header(j: &[u8]) -> Option<(u32, u32, u8, Option<u8>)> {
    if j.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    let mut adobe = None;
    let mut sof = None;
    loop {
        if *j.get(i)? != 0xFF {
            return None;
        }
        let m = *j.get(i + 1)?;
        if m == 0xFF {
            i += 1;
            continue;
        }
        if m == 0xDA {
            let (w, h, c) = sof?;
            return Some((w, h, c, adobe));
        }
        let len = usize::from(u16::from_be_bytes([*j.get(i + 2)?, *j.get(i + 3)?]));
        if len < 2 {
            return None;
        }
        let seg = j.get(i + 4..i + 2 + len)?;
        match m {
            0xEE if seg.len() >= 12 && seg.starts_with(b"Adobe") => adobe = Some(seg[11]),
            0xC0..=0xC2 => {
                if *seg.first()? != 8 || sof.is_some() {
                    return None;
                }
                let h = u32::from(u16::from_be_bytes([*seg.get(1)?, *seg.get(2)?]));
                let w = u32::from(u16::from_be_bytes([*seg.get(3)?, *seg.get(4)?]));
                sof = Some((w, h, *seg.get(5)?));
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xD9 => return None,
            _ => {}
        }
        i += 2 + len;
    }
}

/// Put `JPEGTables` (own SOI…EOI stream with tables) before the image data.
fn merge_jpeg_tables(tables: Option<&[u8]>, strip: Vec<u8>) -> Option<Vec<u8>> {
    match tables {
        Some(t) if t.len() >= 4 => {
            if t[..2] != [0xFF, 0xD8] || t[t.len() - 2..] != [0xFF, 0xD9] || strip.get(..2)? != [0xFF, 0xD8] {
                return None;
            }
            let mut out = Vec::with_capacity(t.len() + strip.len());
            out.extend_from_slice(&t[..t.len() - 2]);
            out.extend_from_slice(&strip[2..]);
            Some(out)
        }
        _ => Some(strip),
    }
}

fn tag_u16(dec: &mut TiffDec<'_>, t: tiff::tags::Tag) -> Option<u16> {
    dec.find_tag(t).ok().flatten().and_then(|v| v.into_u16().ok())
}

/// A single image as [`RawImage`], if it can be taken over without re-encoding.
fn raw_frame(dec: &mut TiffDec<'_>, data: &[u8]) -> Option<RawImage> {
    use tiff::tags::Tag;
    let (width, height) = dec.dimensions().ok()?;
    let compression = tag_u16(dec, Tag::Compression)?;
    let photometric = tag_u16(dec, Tag::PhotometricInterpretation)?;
    let samples = tag_u16(dec, Tag::SamplesPerPixel).unwrap_or(1);
    let planar = tag_u16(dec, Tag::PlanarConfiguration).unwrap_or(1);
    let bits = dec.find_tag(Tag::BitsPerSample).ok().flatten().and_then(|v| v.into_u16_vec().ok()).unwrap_or_else(|| vec![1]);
    if planar != 1 || dec.find_tag(Tag::ExtraSamples).ok().flatten().is_some() {
        return None;
    }
    match compression {
        4 => {
            // T6Options bit 1: uncompressed mode allowed → cannot be safely represented in PDF
            let t6 = dec.find_tag(Tag::Unknown(293)).ok().flatten().and_then(|v| v.into_u32().ok()).unwrap_or(0);
            if samples != 1 || bits != [1] || !matches!(photometric, 0 | 1) || t6 & 2 != 0 {
                return None;
            }
            let mut strip = single_strip(dec, data, height)?;
            // FillOrder 2 (least significant bit first): PDF only knows 1 → reverse the bits per byte
            // (lossless).
            if tag_u16(dec, Tag::FillOrder) == Some(2) {
                strip.iter_mut().for_each(|b| *b = b.reverse_bits());
            }
            Some(RawImage::Group4 { width, height, black_is_1: photometric == 1, data: strip })
        }
        7 => {
            if bits.iter().any(|b| *b != 8) {
                return None;
            }
            let color_transform = match (samples, photometric) {
                (1, 1) | (3, 6) => None,
                (3, 2) => Some(0),
                _ => return None,
            };
            let strip = single_strip(dec, data, height)?;
            let tables = dec.find_tag(Tag::JPEGTables).ok().flatten().and_then(|v| v.into_u8_vec().ok());
            let jpeg = merge_jpeg_tables(tables.as_deref(), strip)?;
            let (w, h, comps, adobe) = jpeg_header(&jpeg)?;
            if (w, h) != (width, height) || u16::from(comps) != samples {
                return None;
            }
            // An Adobe marker with a contradicting colour conversion → better re-encode.
            if let Some(t) = adobe
                && samples == 3
                && (t == 0) != (color_transform == Some(0))
            {
                return None;
            }
            Some(RawImage::Jpeg { width, height, components: comps, color_transform, data: jpeg })
        }
        _ => None,
    }
}

/// Per TIFF single image (in file order) the data that can be taken over unchanged or `None`
/// (then it is re-encoded losslessly as Flate as before).
pub fn tiff_raw_frames(data: &[u8]) -> Vec<Option<RawImage>> {
    let mut out = Vec::new();
    let Ok(mut dec) = tiff::decoder::Decoder::new(std::io::Cursor::new(data)) else { return out };
    loop {
        out.push(raw_frame(&mut dec, data));
        if !dec.more_images() || out.len() >= 10_000 || dec.next_image().is_err() {
            break;
        }
    }
    out
}

fn raw_xobject(raw: &RawImage) -> Stream {
    let (w, h) = raw.dimensions();
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"XObject".to_vec()));
    d.set("Subtype", Object::Name(b"Image".to_vec()));
    d.set("Width", i64::from(w));
    d.set("Height", i64::from(h));
    let data = match raw {
        RawImage::Group4 { black_is_1, data, .. } => {
            d.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
            d.set("BitsPerComponent", 1);
            d.set("Filter", Object::Name(b"CCITTFaxDecode".to_vec()));
            let mut p = Dictionary::new();
            p.set("K", -1);
            p.set("Columns", i64::from(w));
            p.set("Rows", i64::from(h));
            if *black_is_1 {
                p.set("BlackIs1", true);
            }
            d.set("DecodeParms", p);
            data.clone()
        }
        RawImage::Jpeg { components, color_transform, data, .. } => {
            let cs: &[u8] = if *components == 1 { b"DeviceGray" } else { b"DeviceRGB" };
            d.set("ColorSpace", Object::Name(cs.to_vec()));
            d.set("BitsPerComponent", 8);
            d.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            if let Some(t) = color_transform {
                let mut p = Dictionary::new();
                p.set("ColorTransform", *t);
                d.set("DecodeParms", p);
            }
            data.clone()
        }
    };
    Stream::new(d, data).with_compression(false)
}

/// A PDF page from an image.
pub struct PageImage<'a> {
    pub image: &'a image::DynamicImage,
    pub dpi: (f64, f64),
    /// Page `/Rotate` (0, 90, 180, 270).
    pub rotate: i64,
    /// Data to be taken over unchanged (instead of re-encoding with `image`).
    pub raw: Option<&'a RawImage>,
}

fn flate(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = enc.write_all(data);
    enc.finish().unwrap_or_default()
}

/// Image XObject: gray (1 bit if only black/white) or RGB, alpha discarded.
fn image_xobject(img: &image::DynamicImage) -> Stream {
    use image::DynamicImage as D;
    let (w, h) = (img.width(), img.height());
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"XObject".to_vec()));
    d.set("Subtype", Object::Name(b"Image".to_vec()));
    d.set("Width", i64::from(w));
    d.set("Height", i64::from(h));
    d.set("Filter", Object::Name(b"FlateDecode".to_vec()));
    let gray = matches!(img, D::ImageLuma8(_) | D::ImageLuma16(_) | D::ImageLumaA8(_) | D::ImageLumaA16(_));
    let data = if gray {
        let g = img.to_luma8();
        d.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        if g.as_raw().iter().all(|p| *p == 0 || *p == 255) {
            d.set("BitsPerComponent", 1);
            let row = w.div_ceil(8) as usize;
            let mut packed = vec![0u8; row * h as usize];
            for (y, line) in g.as_raw().chunks(w as usize).enumerate() {
                for (x, px) in line.iter().enumerate() {
                    if *px == 255 {
                        packed[y * row + x / 8] |= 0x80 >> (x % 8);
                    }
                }
            }
            packed
        } else {
            d.set("BitsPerComponent", 8);
            g.into_raw()
        }
    } else {
        d.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        d.set("BitsPerComponent", 8);
        img.to_rgb8().into_raw()
    };
    Stream::new(d, flate(&data)).with_compression(false)
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "0".into() } else { s.to_string() }
}

/// Images → PDF bytes (one page per image, all with the same dpi).
pub fn images_to_pdf(frames: &[image::DynamicImage], dpi: (f64, f64)) -> Result<Vec<u8>> {
    let pages: Vec<PageImage<'_>> = frames.iter().map(|image| PageImage { image, dpi, rotate: 0, raw: None }).collect();
    pages_to_pdf(&pages)
}

/// Images → PDF bytes with dpi and rotation per page (img2pdf: page size from the dpi of the
/// respective image, EXIF rotation as `/Rotate`).
pub fn pages_to_pdf(pages: &[PageImage<'_>]) -> Result<Vec<u8>> {
    let (mut doc, pages_id) = crate::pdf::new_document();
    let mut kids = Vec::new();
    for p in pages {
        let (img, dpi) = (p.image, p.dpi);
        let wpt = f64::from(img.width()) * 72.0 / dpi.0;
        let hpt = f64::from(img.height()) * 72.0 / dpi.1;
        let xobj = match p.raw {
            Some(r) if r.dimensions() == (img.width(), img.height()) => raw_xobject(r),
            _ => image_xobject(img),
        };
        let img_id = doc.add_object(xobj);
        let mut xo = Dictionary::new();
        xo.set("Im0", Object::Reference(img_id));
        let mut res = Dictionary::new();
        res.set("XObject", xo);
        let content = format!("q\n{} 0 0 {} 0 0 cm\n/Im0 Do\nQ", fmt_num(wpt), fmt_num(hpt)).into_bytes();
        let c_id = doc.add_object(Stream::new(Dictionary::new(), content));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("MediaBox", vec![0.into(), 0.into(), Object::Real(wpt as f32), Object::Real(hpt as f32)]);
        page.set("Resources", res);
        page.set("Contents", Object::Reference(c_id));
        if p.rotate != 0 {
            page.set("Rotate", p.rotate);
        }
        kids.push(Object::Reference(doc.add_object(page)));
    }
    let n = kids.len() as i64;
    let pages = doc.get_dictionary_mut(pages_id).map_err(|e| PdfOpsError::Pdf(e.to_string()))?;
    pages.set("Kids", kids);
    pages.set("Count", n);
    // img2pdf: only /CreationDate and /ModDate = now (UTC, "D:%Y%m%d%H%M%SZ")
    let now = format!("D:{}", chrono::Utc::now().format("%Y%m%d%H%M%SZ"));
    let mut info = Dictionary::new();
    info.set("CreationDate", Object::string_literal(now.clone()));
    info.set("ModDate", Object::string_literal(now));
    let info_id = doc.add_object(info);
    doc.trailer.set("Info", Object::Reference(info_id));
    crate::pdf::save_to_vec(&mut doc)
}

/// `convert_from_tiff_to_pdf(tiff_path, target_directory)`: PDF `target/<name>.pdf` with the
/// file times of the TIFF; returns the path.
pub fn convert_from_tiff_to_pdf(tiff_path: &Path, target_directory: &Path) -> Result<PathBuf> {
    let data = std::fs::read(tiff_path)?;
    let info = pl_ocr::imageinfo::probe(&data).map_err(|e| PdfOpsError::Value(e.to_string()))?;
    // maybe_override_pixel_limit + Pillow bomb check
    let limit = pl_ocr::imageinfo::pil_pixel_limit(pl_envlive::var("PAPERLESS_MAX_IMAGE_PIXELS").and_then(|v| v.trim().parse::<i64>().ok()));
    info.decompression_bomb_check(limit).map_err(|e| PdfOpsError::Value(e.to_string()))?;
    let mut frames = pl_ocr::native::decode_frames(&data, &info).map_err(|e| PdfOpsError::Value(e.to_string()))?;
    // img2pdf: dpi and Orientation per single image (gap E18)
    let meta = tiff_frame_meta(&data);
    // take over G4/JPEG unchanged (only if the single images can be assigned unambiguously)
    let mut raws = tiff_raw_frames(&data);
    if raws.len() != frames.len() {
        raws = vec![None; frames.len()];
    }
    let mut rotations = Vec::with_capacity(frames.len());
    for (i, img) in frames.iter_mut().enumerate() {
        let m = meta.get(i).copied();
        let rotate = match m.map(|m| m.orientation).unwrap_or(1) {
            3 => 180,
            6 => 90,
            8 => 270,
            // mirrored (2/4/5/7): img2pdf aborts with ExifOrientationError; here the
            // pixels are corrected (Known difference)
            o @ (2 | 4 | 5 | 7) => {
                if let Some(orient) = image::metadata::Orientation::from_exif(o as u8) {
                    img.apply_orientation(orient);
                }
                // changed pixels: the original data no longer fit
                if let Some(r) = raws.get_mut(i) {
                    *r = None;
                }
                0
            }
            _ => 0,
        };
        rotations.push(rotate);
    }
    let pages: Vec<PageImage<'_>> = frames
        .iter()
        .enumerate()
        .map(|(i, image)| {
            let dpi = match meta.get(i) {
                Some(m) => img2pdf_dpi_from(m.dpi),
                None => img2pdf_dpi(&info),
            };
            PageImage { image, dpi, rotate: rotations.get(i).copied().unwrap_or(0), raw: raws.get(i).and_then(Option::as_ref) }
        })
        .collect();
    let bytes = pages_to_pdf(&pages)?;
    let name = tiff_path.file_name().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("image.tiff"));
    let pdf_path = target_directory.join(name).with_extension("pdf");
    std::fs::write(&pdf_path, bytes)?;
    crate::copy_basic_file_stats(tiff_path, &pdf_path)?;
    Ok(pdf_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media_box(d: &lopdf::Document, page: lopdf::ObjectId) -> (f32, f32) {
        let mb = d.get_dictionary(page).unwrap().get(b"MediaBox").unwrap().as_array().unwrap().clone();
        (mb[2].as_float().unwrap(), mb[3].as_float().unwrap())
    }

    /// Gap E18: dpi per single image, orientation 6 → `/Rotate 90`, mirrored (2) → pixels mirrored.
    #[test]
    fn per_frame_dpi_and_orientation() {
        use tiff::encoder::{Rational, TiffEncoder, colortype::Gray8};
        use tiff::tags::{ResolutionUnit, Tag};
        let t = tempfile::tempdir().unwrap();
        let tif = t.path().join("multi.tif");
        {
            let f = std::fs::File::create(&tif).unwrap();
            let mut enc = TiffEncoder::new(f).unwrap();
            // page 1: 200 dpi, Orientation 6
            let px = vec![200u8; 200 * 100];
            let mut im = enc.new_image::<Gray8>(200, 100).unwrap();
            im.resolution(ResolutionUnit::Inch, Rational { n: 200, d: 1 });
            im.encoder().write_tag(Tag::Orientation, 6u16).unwrap();
            im.write_data(&px).unwrap();
            // page 2: 100 dpi in centimetres (≈ 39.37 px/cm), mirrored (2): left column black
            let mut px = vec![255u8; 4 * 2];
            px[0] = 0;
            px[4] = 0;
            let mut im = enc.new_image::<Gray8>(4, 2).unwrap();
            im.resolution(ResolutionUnit::Centimeter, Rational { n: 3937, d: 100 });
            im.encoder().write_tag(Tag::Orientation, 2u16).unwrap();
            im.write_data(&px).unwrap();
        }
        let meta = tiff_frame_meta(&std::fs::read(&tif).unwrap());
        assert_eq!(meta.len(), 2);
        assert_eq!(meta[0].orientation, 6);
        assert_eq!(meta[1].orientation, 2);
        let pdf = convert_from_tiff_to_pdf(&tif, t.path()).unwrap();
        let d = lopdf::Document::load(&pdf).unwrap();
        let pages = crate::pdf::page_ids(&d);
        assert_eq!(pages.len(), 2);
        let (w, h) = media_box(&d, pages[0]);
        assert!((w - 72.0).abs() < 0.01 && (h - 36.0).abs() < 0.01, "{w}x{h}");
        assert_eq!(d.get_dictionary(pages[0]).unwrap().get(b"Rotate").unwrap().as_i64().unwrap(), 90);
        let (w, h) = media_box(&d, pages[1]);
        assert!((w - 4.0 * 72.0 / 100.0).abs() < 0.01 && (h - 2.0 * 72.0 / 100.0).abs() < 0.01, "{w}x{h}");
        assert!(d.get_dictionary(pages[1]).unwrap().get(b"Rotate").is_err());
        // mirrored: row [black, white, white, white] → [white, white, white, black] = 1110....
        let res = d.get_dictionary(pages[1]).unwrap().get(b"Resources").unwrap().as_dict().unwrap().clone();
        let im = res.get(b"XObject").unwrap().as_dict().unwrap().get(b"Im0").unwrap().as_reference().unwrap();
        let data = d.get_object(im).unwrap().as_stream().unwrap().decompressed_content().unwrap();
        assert_eq!(data[0] & 0xF0, 0xE0, "{data:?}");
    }

    fn image_stream(d: &lopdf::Document, page: lopdf::ObjectId) -> lopdf::Stream {
        let res = d.get_dictionary(page).unwrap().get(b"Resources").unwrap().as_dict().unwrap().clone();
        let im = res.get(b"XObject").unwrap().as_dict().unwrap().get(b"Im0").unwrap().as_reference().unwrap();
        d.get_object(im).unwrap().as_stream().unwrap().clone()
    }

    fn convert_fixture(name: &str) -> (Vec<u8>, lopdf::Document) {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
        let t = tempfile::tempdir().unwrap();
        let tif = t.path().join(name);
        std::fs::copy(&src, &tif).unwrap();
        let pdf = convert_from_tiff_to_pdf(&tif, t.path()).unwrap();
        (std::fs::read(&src).unwrap(), lopdf::Document::load(&pdf).unwrap())
    }

    /// Gap E18: CCITT G4 is taken over unchanged like img2pdf.
    #[test]
    fn group4_is_passed_through() {
        let (tif, d) = convert_fixture("g4.tif");
        let pages = crate::pdf::page_ids(&d);
        assert_eq!(pages.len(), 1);
        let s = image_stream(&d, pages[0]);
        assert_eq!(s.dict.get(b"Filter").unwrap().as_name().unwrap(), b"CCITTFaxDecode");
        assert_eq!(s.dict.get(b"BitsPerComponent").unwrap().as_i64().unwrap(), 1);
        let p = s.dict.get(b"DecodeParms").unwrap().as_dict().unwrap();
        assert_eq!(p.get(b"K").unwrap().as_i64().unwrap(), -1);
        assert_eq!(p.get(b"Columns").unwrap().as_i64().unwrap(), 64);
        assert_eq!(p.get(b"Rows").unwrap().as_i64().unwrap(), 40);
        // PIL schreibt PhotometricInterpretation 1 → /BlackIs1 true
        assert!(p.get(b"BlackIs1").unwrap().as_bool().unwrap());
        // bytes 1:1 from the TIFF strip
        let raws = tiff_raw_frames(&tif);
        let Some(RawImage::Group4 { data, .. }) = &raws[0] else { panic!("{raws:?}") };
        assert_eq!(&s.content, data);
        assert!(tif.windows(data.len()).any(|w| w == data.as_slice()));
        // 200 dpi
        let (w, h) = media_box(&d, pages[0]);
        assert!((w - 64.0 * 72.0 / 200.0).abs() < 0.01 && (h - 40.0 * 72.0 / 200.0).abs() < 0.01);
    }

    /// Gap E18: JPEG in a TIFF is taken over as DCTDecode (RGB without YCbCr → ColorTransform 0);
    /// the stream is a valid JPEG with the same pixels as the TIFF.
    #[test]
    fn jpeg_is_passed_through() {
        for (name, comps, ct) in [("jpeg-rgb.tif", 3u8, Some(0)), ("jpeg-gray.tif", 1, None)] {
            let (tif, d) = convert_fixture(name);
            let pages = crate::pdf::page_ids(&d);
            let s = image_stream(&d, pages[0]);
            assert_eq!(s.dict.get(b"Filter").unwrap().as_name().unwrap(), b"DCTDecode", "{name}");
            assert_eq!(
                s.dict.get(b"DecodeParms").ok().and_then(|p| p.as_dict().ok()).and_then(|p| p.get(b"ColorTransform").ok()).and_then(|v| v.as_i64().ok()),
                ct,
                "{name}"
            );
            let Some(RawImage::Jpeg { components, .. }) = &tiff_raw_frames(&tif)[0] else { panic!("{name}") };
            assert_eq!(*components, comps);
            let (w, h, c, _) = jpeg_header(&s.content).unwrap();
            assert_eq!((w, h, c), (48, 32, comps));
            if comps == 1 {
                // grayscale: JPEG decoding of the PDF stream = TIFF decoding
                let from_pdf = image::load_from_memory_with_format(&s.content, image::ImageFormat::Jpeg).unwrap().to_luma8();
                let from_tif = image::load_from_memory_with_format(&tif, image::ImageFormat::Tiff).unwrap().to_luma8();
                let diff = from_pdf.as_raw().iter().zip(from_tif.as_raw()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
                assert!(diff <= 2, "max diff {diff}");
            }
        }
    }

    /// Uncompressed/LZW TIFF and mirrored pages: still Flate (nothing to pass through).
    #[test]
    fn uncompressed_stays_flate_and_helpers() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut enc = tiff::encoder::TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<tiff::encoder::colortype::Gray8>(4, 4, &[7u8; 16]).unwrap();
        }
        assert_eq!(tiff_raw_frames(buf.get_ref()), vec![None]);
        // tables + strip
        let merged = merge_jpeg_tables(Some(&[0xFF, 0xD8, 0xFF, 0xDB, 0xFF, 0xD9]), vec![0xFF, 0xD8, 0xFF, 0xDA]).unwrap();
        assert_eq!(merged, vec![0xFF, 0xD8, 0xFF, 0xDB, 0xFF, 0xDA]);
        assert!(merge_jpeg_tables(Some(&[1, 2, 3, 4]), vec![0xFF, 0xD8]).is_none());
        assert!(jpeg_header(&[0xFF, 0xD8, 0xFF, 0xDA]).is_none());
        // FillOrder 2: reverse the bits per byte
        assert_eq!(0b1000_0001u8.reverse_bits(), 0b1000_0001);
        assert_eq!(0b1100_0000u8.reverse_bits(), 0b0000_0011);
    }

    #[test]
    fn dpi_rounding_like_img2pdf() {
        assert_eq!(img2pdf_dpi_from(None), (96.0, 96.0));
        assert_eq!(img2pdf_dpi_from(Some((1.0, 1.0))), (96.0, 96.0));
        assert_eq!(img2pdf_dpi_from(Some((299.6, 300.4))), (300.0, 300.0));
    }

    #[test]
    fn multi_frame_tiff_with_dpi() {
        let t = tempfile::tempdir().unwrap();
        let tif = t.path().join("scan.tiff");
        {
            let f = std::fs::File::create(&tif).unwrap();
            let mut enc = tiff::encoder::TiffEncoder::new(f).unwrap();
            for shade in [0u8, 128] {
                let mut px = vec![255u8; 200 * 100];
                px[..200 * 10].iter_mut().for_each(|p| *p = shade);
                let mut im = enc.new_image::<tiff::encoder::colortype::Gray8>(200, 100).unwrap();
                im.resolution(tiff::tags::ResolutionUnit::Inch, tiff::encoder::Rational { n: 200, d: 1 });
                im.write_data(&px).unwrap();
            }
        }
        let out_dir = t.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();
        let pdf = convert_from_tiff_to_pdf(&tif, &out_dir).unwrap();
        assert_eq!(pdf, out_dir.join("scan.pdf"));
        let d = lopdf::Document::load(&pdf).unwrap();
        let pages = crate::pdf::page_ids(&d);
        assert_eq!(pages.len(), 2);
        let mb = d.get_dictionary(pages[0]).unwrap().get(b"MediaBox").unwrap().as_array().unwrap().clone();
        assert!((mb[2].as_float().unwrap() - 72.0).abs() < 0.01);
        assert!((mb[3].as_float().unwrap() - 36.0).abs() < 0.01);
    }
}
