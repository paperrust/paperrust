//! Pages of a scan → one PDF.
//!
//! * JPEG pages are embedded **unchanged** (`DCTDecode`, no re-encoding, no
//!   quality loss). Page size = pixels / dpi · 72 pt with the requested resolution.
//! * PDF pages (devices that only deliver PDF) are merged with `pl_pdfops::PdfMerger`.
//! * Other image formats (PNG/TIFF) go through `pl_pdfops::convert::images_to_pdf` (lossless).

use std::path::Path;

use lopdf::{Dictionary, Document, Object, Stream};

use crate::escl::{Page, ScanError};

/// Width, height, colour components from the SOF segment of a JPEG.
pub fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // fill bytes and markers without a length
        if marker == 0xFF || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let s = data.get(i + 4..i + 10)?;
            let h = u32::from(u16::from_be_bytes([s[1], s[2]]));
            let w = u32::from(u16::from_be_bytes([s[3], s[4]]));
            return (w > 0 && h > 0).then_some((w, h, s[5]));
        }
        i += 2 + len;
    }
    None
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "0".into() } else { s.to_string() }
}

/// JPEG pages → PDF document (without re-encoding).
pub fn jpegs_to_document(pages: &[&[u8]], dpi: u32) -> Result<Document, ScanError> {
    let dpi = f64::from(dpi.max(1));
    let (mut doc, pages_id) = pl_pdfops::pdf::new_document();
    let mut kids = Vec::new();
    for (n, data) in pages.iter().enumerate() {
        let (w, h, comps) = jpeg_info(data).ok_or_else(|| ScanError::Pdf(format!("page {} is not a readable JPEG", n + 1)))?;
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"XObject".to_vec()));
        d.set("Subtype", Object::Name(b"Image".to_vec()));
        d.set("Width", i64::from(w));
        d.set("Height", i64::from(h));
        d.set("BitsPerComponent", 8);
        d.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        let cs: &[u8] = match comps {
            1 => b"DeviceGray",
            4 => b"DeviceCMYK",
            _ => b"DeviceRGB",
        };
        d.set("ColorSpace", Object::Name(cs.to_vec()));
        let img_id = doc.add_object(Stream::new(d, data.to_vec()).with_compression(false));
        let wpt = f64::from(w) * 72.0 / dpi;
        let hpt = f64::from(h) * 72.0 / dpi;
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
        kids.push(Object::Reference(doc.add_object(page)));
    }
    let n = kids.len() as i64;
    let pages = doc.get_dictionary_mut(pages_id).map_err(|e| ScanError::Pdf(e.to_string()))?;
    pages.set("Kids", kids);
    pages.set("Count", n);
    let now = format!("D:{}", chrono::Utc::now().format("%Y%m%d%H%M%SZ"));
    let mut info = Dictionary::new();
    info.set("Producer", Object::string_literal(pl_core::brand::PRODUCT_NAME));
    info.set("CreationDate", Object::string_literal(now.clone()));
    info.set("ModDate", Object::string_literal(now));
    let info_id = doc.add_object(info);
    doc.trailer.set("Info", Object::Reference(info_id));
    Ok(doc)
}

fn is_jpeg(p: &Page) -> bool {
    p.data.starts_with(&[0xFF, 0xD8])
}

fn is_pdf(p: &Page) -> bool {
    p.data.starts_with(b"%PDF")
}

/// Pages (in scan order) → PDF file `dst`. Returns the page count.
pub fn write_pdf(pages: &[Page], dpi: u32, dst: &Path) -> Result<usize, ScanError> {
    if pages.is_empty() {
        return Err(ScanError::Pdf("no pages".into()));
    }
    let pdf_err = |e: pl_pdfops::PdfOpsError| ScanError::Pdf(e.to_string());
    // Most common case: JPEG only → one document, write directly.
    if pages.iter().all(is_jpeg) {
        let refs: Vec<&[u8]> = pages.iter().map(|p| p.data.as_slice()).collect();
        let mut doc = jpegs_to_document(&refs, dpi)?;
        pl_pdfops::pdf::save(&mut doc, dst).map_err(pdf_err)?;
        return Ok(pages.len());
    }
    // Mixed: each page (or sequence of JPEGs) as its own document, then merge.
    let mut docs: Vec<Document> = Vec::new();
    let mut run: Vec<&[u8]> = Vec::new();
    let flush = |run: &mut Vec<&[u8]>, docs: &mut Vec<Document>| -> Result<(), ScanError> {
        if !run.is_empty() {
            docs.push(jpegs_to_document(run, dpi)?);
            run.clear();
        }
        Ok(())
    };
    for (n, p) in pages.iter().enumerate() {
        if is_jpeg(p) {
            run.push(&p.data);
            continue;
        }
        flush(&mut run, &mut docs)?;
        if is_pdf(p) {
            docs.push(pl_pdfops::pdf::load_bytes(&p.data, "scan", None).map_err(pdf_err)?);
        } else {
            let img = image::load_from_memory(&p.data).map_err(|_| ScanError::Pdf(format!("page {} has an unsupported format ({})", n + 1, p.content_type)))?;
            let bytes = pl_pdfops::convert::images_to_pdf(&[img], (f64::from(dpi), f64::from(dpi))).map_err(pdf_err)?;
            docs.push(pl_pdfops::pdf::load_bytes(&bytes, "scan", None).map_err(pdf_err)?);
        }
    }
    flush(&mut run, &mut docs)?;
    if docs.len() == 1 && pages.len() == 1 && is_pdf(&pages[0]) {
        std::fs::write(dst, &pages[0].data).map_err(|e| ScanError::Pdf(e.to_string()))?;
    } else {
        let mut m = pl_pdfops::PdfMerger::new();
        for d in &docs {
            m.add_document(d).map_err(pdf_err)?;
        }
        m.save(dst).map_err(pdf_err)?;
    }
    let count = Document::load(dst).map(|d| d.get_pages().len()).unwrap_or(pages.len());
    Ok(count)
}

#[cfg(test)]
pub(crate) mod testimg {
    /// Small JPEG (w×h, grayscale or RGB) with the image crate.
    pub fn jpeg(w: u32, h: u32, rgb: bool) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let img = if rgb {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 128])))
        } else {
            image::DynamicImage::ImageLuma8(image::GrayImage::from_fn(w, h, |x, _| image::Luma([(x % 255) as u8])))
        };
        img.write_to(&mut out, image::ImageFormat::Jpeg).expect("encode jpeg");
        out.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_sof() {
        let j = testimg::jpeg(40, 30, true);
        assert_eq!(jpeg_info(&j), Some((40, 30, 3)));
        assert_eq!(jpeg_info(&testimg::jpeg(8, 9, false)), Some((8, 9, 1)));
        assert_eq!(jpeg_info(b"nope"), None);
    }

    #[test]
    fn jpeg_pages_to_pdf() {
        let t = tempfile::tempdir().unwrap();
        let dst = t.path().join("scan.pdf");
        let pages: Vec<Page> = (0..3).map(|_| Page { content_type: "image/jpeg".into(), data: testimg::jpeg(300, 600, true) }).collect();
        assert_eq!(write_pdf(&pages, 300, &dst).unwrap(), 3);
        let doc = Document::load(&dst).unwrap();
        assert_eq!(doc.get_pages().len(), 3);
        // 300 px at 300 dpi = 72 pt
        let first = *doc.get_pages().values().next().unwrap();
        let mb = doc.get_dictionary(first).unwrap().get(b"MediaBox").unwrap().as_array().unwrap().clone();
        assert!((mb[2].as_float().unwrap() - 72.0).abs() < 0.01);
        // image embedded unchanged
        let raw = std::fs::read(&dst).unwrap();
        assert!(raw.windows(9).any(|w| w == b"DCTDecode"));
    }

    #[test]
    fn mixed_pdf_and_png() {
        let t = tempfile::tempdir().unwrap();
        let one = t.path().join("one.pdf");
        write_pdf(&[Page { content_type: "image/jpeg".into(), data: testimg::jpeg(100, 100, false) }], 100, &one).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image::GrayImage::new(50, 50)).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let pages = vec![
            Page { content_type: "application/pdf".into(), data: std::fs::read(&one).unwrap() },
            Page { content_type: "image/png".into(), data: png.into_inner() },
            Page { content_type: "image/jpeg".into(), data: testimg::jpeg(100, 100, true) },
        ];
        let dst = t.path().join("all.pdf");
        assert_eq!(write_pdf(&pages, 100, &dst).unwrap(), 3);
        assert!(write_pdf(&[Page { content_type: "x/y".into(), data: b"garbage".to_vec() }], 100, &dst).is_err());
        assert!(write_pdf(&[], 100, &dst).is_err());
    }
}
