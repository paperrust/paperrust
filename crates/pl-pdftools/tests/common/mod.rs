//! Sample PDFs for the tests (generated with lopdf, no third-party files).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, Stream, dictionary};

use pl_pdftools::Limits;

/// Executable of the child process (cargo builds it for integration tests with `--features engine`).
pub fn tool() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_paperrust-pdftool"))
}

pub fn limits() -> Limits {
    Limits { timeout: Duration::from_secs(300), memory_mb: 2048 }
}

fn base(pages: Vec<Vec<Operation>>, info: bool) -> Document {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });
    let mut kids = Vec::new();
    for ops in pages {
        let (w, h) = (612, 792);
        let content = Content { operations: ops };
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.encode().unwrap_or_default()));
        let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), w.into(), h.into()],
            "Contents" => content_id, "Resources" => resources,
        });
        kids.push(Object::Reference(page_id));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    if info {
        let info_id = doc.add_object(dictionary! {
            "Title" => Object::string_literal("Vertrag Max Mustermann"),
            "Author" => Object::string_literal("Erika Musterfrau"),
        });
        doc.trailer.set("Info", info_id);
    }
    doc
}

fn text_ops(lines: &[(&str, f64, f64)]) -> Vec<Operation> {
    let mut ops = vec![Operation::new("BT", vec![]), Operation::new("Tf", vec!["F1".into(), 24.into()])];
    for (t, x, y) in lines {
        ops.push(Operation::new("Tm", vec![1.into(), 0.into(), 0.into(), 1.into(), (*x).into(), (*y).into()]));
        ops.push(Operation::new("Tj", vec![Object::string_literal(*t)]));
    }
    ops.push(Operation::new("ET", vec![]));
    ops
}

/// Two pages of text; page 1: "GEHEIM 4711" at (72, 700), "Öffentlicher Teil" at (72, 600);
/// page 2: "Kontonummer GEHEIM" at (72, 500). With `/Info` (title/author with names).
pub fn text_pdf(path: &Path) {
    let p1 = text_ops(&[("GEHEIM 4711", 72.0, 700.0), ("Offener Teil bleibt", 72.0, 600.0)]);
    let p2 = text_ops(&[("Kontonummer GEHEIM", 72.0, 500.0)]);
    let mut doc = base(vec![p1, p2], true);
    doc.compress();
    doc.save(path).unwrap();
}

/// A page of 6×8 inches with a 1800×2400 RGB image (300 ppi, noisy, Flate) and text.
pub fn scan_pdf(path: &Path) {
    let (w, h) = (1800u32, 2400u32);
    let mut raw = Vec::with_capacity((w * h * 3) as usize);
    let mut seed: u32 = 12345;
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = ((seed >> 16) & 0x1f) as u8;
            raw.push(((x * 255 / w) as u8).saturating_add(n));
            raw.push(((y * 255 / h) as u8).saturating_add(n));
            raw.push(200u8.saturating_sub(n));
        }
    }
    let mut img = Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
        raw,
    );
    img.compress().unwrap();
    let mut doc = Document::with_version("1.5");
    let img_id = doc.add_object(img);
    let mut xo = Dictionary::new();
    xo.set("Im0", img_id);
    let ops = vec![
        Operation::new("q", vec![]),
        Operation::new("cm", vec![432.into(), 0.into(), 0.into(), 576.into(), 0.into(), 0.into()]),
        Operation::new("Do", vec!["Im0".into()]),
        Operation::new("Q", vec![]),
    ];
    // build a page over `base` (own document with an image)
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Stream::new(Dictionary::new(), Content { operations: ops }.encode().unwrap_or_default()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), 432.into(), 576.into()],
        "Contents" => content_id, "Resources" => dictionary! { "XObject" => xo },
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

/// Text of all pages with lopdf (this is also how PaperRust's text extraction reads it without OCR).
pub fn extract_text(path: &Path) -> String {
    let doc = Document::load(path).unwrap();
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    doc.extract_text(&pages).unwrap_or_default()
}

/// All bytes of the file, streams unpacked (also finds text that no text extractor shows).
pub fn all_bytes_decoded(path: &Path) -> Vec<u8> {
    let doc = Document::load(path).unwrap();
    let mut out = std::fs::read(path).unwrap();
    for obj in doc.objects.values() {
        if let Object::Stream(s) = obj
            && let Ok(d) = s.decompressed_content()
        {
            out.extend_from_slice(&d);
        }
    }
    out
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Like [`text_pdf`], but without `/Info` and without XMP (no metadata, test log C-066b).
pub fn text_pdf_without_metadata(path: &Path) {
    let p1 = text_ops(&[("GEHEIM 4711", 72.0, 700.0), ("Offener Teil bleibt", 72.0, 600.0)]);
    let p2 = text_ops(&[("Kontonummer GEHEIM", 72.0, 500.0)]);
    let mut doc = base(vec![p1, p2], false);
    doc.compress();
    doc.save(path).unwrap();
}

/// Width/height of the JPEG scan in pixels (1 pt = 1 px: page 600 × 800 pt).
pub const SCAN_W: u32 = 600;
pub const SCAN_H: u32 = 800;

/// A page of 600 × 800 pt consisting only of a JPEG image (grayscale, like a scan):
/// white background, two black "words" as bars – word A at x 100–300, y (PDF) 600–640,
/// word B at x 100–300, y 300–340. There is no text (test log C-068).
pub fn jpeg_scan_pdf(path: &Path) {
    let mut img = image::GrayImage::from_pixel(SCAN_W, SCAN_H, image::Luma([255u8]));
    for (y0, y1) in [(600u32, 640u32), (300, 340)] {
        for y in y0..y1 {
            for x in 100..300u32 {
                // PDF y grows upwards, image rows downwards
                img.put_pixel(x, SCAN_H - 1 - y, image::Luma([0u8]));
            }
        }
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode(img.as_raw(), SCAN_W, SCAN_H, image::ExtendedColorType::L8)
        .unwrap();
    let img = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => SCAN_W as i64, "Height" => SCAN_H as i64,
            "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8, "Filter" => "DCTDecode",
        },
        jpeg,
    );
    let mut doc = Document::with_version("1.5");
    let img_id = doc.add_object(img);
    let mut xo = Dictionary::new();
    xo.set("Im0", img_id);
    let ops = vec![
        Operation::new("q", vec![]),
        Operation::new("cm", vec![(SCAN_W as i64).into(), 0.into(), 0.into(), (SCAN_H as i64).into(), 0.into(), 0.into()]),
        Operation::new("Do", vec!["Im0".into()]),
        Operation::new("Q", vec![]),
    ];
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Stream::new(Dictionary::new(), Content { operations: ops }.encode().unwrap_or_default()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), (SCAN_W as i64).into(), (SCAN_H as i64).into()],
        "Contents" => content_id, "Resources" => dictionary! { "XObject" => xo },
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

/// The (only) image of the first page: filter and unpacked grayscale pixels.
pub fn first_page_image(path: &Path) -> Option<(String, image::GrayImage)> {
    let doc = Document::load(path).unwrap();
    let page = *doc.get_pages().values().next()?;
    let (res, _) = doc.get_page_resources(page).ok()?;
    let res = res?;
    let xo = match res.get(b"XObject").ok()? {
        Object::Reference(r) => doc.get_dictionary(*r).ok()?,
        Object::Dictionary(d) => d,
        _ => return None,
    };
    let (_, v) = xo.iter().next()?;
    let s = doc.get_object(v.as_reference().ok()?).ok()?.as_stream().ok()?;
    let filter =
        s.dict.get(b"Filter").ok().and_then(|f| f.as_name().ok()).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
    let w = s.dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let h = s.dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    let img = if filter == "DCTDecode" {
        image::load_from_memory_with_format(&s.content, image::ImageFormat::Jpeg).ok()?.to_luma8()
    } else {
        let data = s.decompressed_content().ok()?;
        image::GrayImage::from_raw(w, h, data[..(w * h) as usize].to_vec())?
    };
    Some((filter, img))
}

/// Mean brightness of an area (PDF coordinates, 1 pt = 1 px).
pub fn mean_luma(img: &image::GrayImage, x0: u32, y0: u32, x1: u32, y1: u32) -> f64 {
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += u64::from(img.get_pixel(x, img.height() - 1 - y)[0]);
            n += 1;
        }
    }
    sum as f64 / n.max(1) as f64
}

/// The grayscale JPEG scan of [`jpeg_scan_pdf`] as an image XObject.
fn scan_image_stream() -> Stream {
    let mut img = image::GrayImage::from_pixel(SCAN_W, SCAN_H, image::Luma([255u8]));
    for (y0, y1) in [(600u32, 640u32), (300, 340)] {
        for y in y0..y1 {
            for x in 100..300u32 {
                img.put_pixel(x, SCAN_H - 1 - y, image::Luma([0u8]));
            }
        }
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode(img.as_raw(), SCAN_W, SCAN_H, image::ExtendedColorType::L8)
        .unwrap();
    Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => SCAN_W as i64, "Height" => SCAN_H as i64,
            "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8, "Filter" => "DCTDecode",
        },
        jpeg,
    )
}

/// A page of a synthetic document: visible text lines, or the JPEG scan with (optionally)
/// invisible OCR text (render mode 3, like OCRmyPDF's text layer).
pub enum MixedPage<'a> {
    Text(&'a [(&'a str, f64, f64)]),
    Scan(&'a [(&'a str, f64, f64)]),
}

/// Write a document of 600 × 800 pt pages built from `pages`.
pub fn mixed_pdf(path: &Path, pages: &[MixedPage<'_>]) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });
    let mut kids = Vec::new();
    for p in pages {
        let mut resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
        let ops = match p {
            MixedPage::Text(lines) => text_ops(lines),
            MixedPage::Scan(ocr) => {
                let img_id = doc.add_object(scan_image_stream());
                let mut xo = Dictionary::new();
                xo.set("Im0", img_id);
                resources.set("XObject", xo);
                let mut ops = vec![
                    Operation::new("q", vec![]),
                    Operation::new("cm", vec![(SCAN_W as i64).into(), 0.into(), 0.into(), (SCAN_H as i64).into(), 0.into(), 0.into()]),
                    Operation::new("Do", vec!["Im0".into()]),
                    Operation::new("Q", vec![]),
                ];
                if !ocr.is_empty() {
                    let mut t = text_ops(ocr);
                    t.insert(1, Operation::new("Tr", vec![3.into()]));
                    ops.extend(t);
                }
                ops
            }
        };
        let content_id = doc.add_object(Stream::new(Dictionary::new(), Content { operations: ops }.encode().unwrap_or_default()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), (SCAN_W as i64).into(), (SCAN_H as i64).into()],
            "Contents" => content_id, "Resources" => resources,
        });
        kids.push(Object::Reference(page_id));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

/// The image of page `index` (0-based), unpacked as grayscale.
pub fn page_image(path: &Path, index: usize) -> Option<image::GrayImage> {
    let doc = Document::load(path).unwrap();
    let page = *doc.get_pages().values().nth(index)?;
    let (res, _) = doc.get_page_resources(page).ok()?;
    let xo = match res?.get(b"XObject").ok()? {
        Object::Reference(r) => doc.get_dictionary(*r).ok()?,
        Object::Dictionary(d) => d,
        _ => return None,
    };
    let (_, v) = xo.iter().next()?;
    let s = doc.get_object(v.as_reference().ok()?).ok()?.as_stream().ok()?;
    let filter = s.dict.get(b"Filter").ok().and_then(|f| f.as_name().ok()).map(|n| n.to_vec()).unwrap_or_default();
    let (w, h) = (s.dict.get(b"Width").ok()?.as_i64().ok()? as u32, s.dict.get(b"Height").ok()?.as_i64().ok()? as u32);
    if filter == b"DCTDecode" {
        Some(image::load_from_memory_with_format(&s.content, image::ImageFormat::Jpeg).ok()?.to_luma8())
    } else {
        image::GrayImage::from_raw(w, h, s.decompressed_content().ok()?[..(w * h) as usize].to_vec())
    }
}
