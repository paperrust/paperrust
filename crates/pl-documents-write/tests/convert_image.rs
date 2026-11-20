//! "Convert to PDF" for images without a database (docs/contract/pl-pdftools.md §9): PNG, JPEG,
//! multi-page TIFF, WebP, PNG with transparency, broken image. OCR needs Tesseract and is not
//! part of the tests here (see E2E in the contract).

use std::path::Path;

use pl_documents_write::convert::{ImageError, convert_image};

fn cfg() -> pl_ocr::OcrConfig {
    pl_ocr::OcrConfig::defaults()
}

fn pages(pdf: &Path) -> usize {
    lopdf::Document::load(pdf).unwrap().get_pages().len()
}

fn rgb(w: u32, h: u32) -> image::RgbImage {
    image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]))
}

#[test]
fn png_and_webp_become_one_page_pdfs() {
    let t = tempfile::tempdir().unwrap();
    for (name, fmt, mime) in [("a.png", image::ImageFormat::Png, "image/png"), ("a.webp", image::ImageFormat::WebP, "image/webp")] {
        let src = t.path().join(name);
        rgb(300, 200).save_with_format(&src, fmt).unwrap();
        let work = t.path().join(format!("work-{name}"));
        let pdf = convert_image(&src, mime, false, &cfg(), &work).unwrap();
        let data = std::fs::read(&pdf).unwrap();
        assert!(data.starts_with(b"%PDF"), "{name}");
        assert_eq!(pages(&pdf), 1, "{name}");
    }
}

#[test]
fn jpeg_is_embedded_unchanged() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("photo.jpg");
    rgb(320, 240).save_with_format(&src, image::ImageFormat::Jpeg).unwrap();
    let pdf = convert_image(&src, "image/jpeg", false, &cfg(), &t.path().join("w")).unwrap();
    let jpg = std::fs::read(&src).unwrap();
    let out = std::fs::read(&pdf).unwrap();
    // img2pdf behavior: JPEG data 1:1 in the PDF (DCTDecode), not re-encoded
    assert!(out.windows(jpg.len()).any(|w| w == jpg.as_slice()), "JPEG bytes not found in PDF");
}

#[test]
fn multi_page_tiff_gives_one_pdf_page_per_tiff_page() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("scan.tiff");
    {
        let f = std::fs::File::create(&src).unwrap();
        let mut enc = tiff::encoder::TiffEncoder::new(f).unwrap();
        for shade in [0u8, 100, 200] {
            let px = vec![shade; 200 * 100];
            let mut im = enc.new_image::<tiff::encoder::colortype::Gray8>(200, 100).unwrap();
            im.resolution(tiff::tags::ResolutionUnit::Inch, tiff::encoder::Rational { n: 200, d: 1 });
            im.write_data(&px).unwrap();
        }
    }
    let pdf = convert_image(&src, "image/tiff", false, &cfg(), &t.path().join("w")).unwrap();
    assert_eq!(pages(&pdf), 3);
}

#[test]
fn transparency_is_flattened_onto_white() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("logo.png");
    let img = image::RgbaImage::from_fn(100, 50, |x, _| if x < 50 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) });
    img.save_with_format(&src, image::ImageFormat::Png).unwrap();
    // pl-ocr alone (img2pdf) rejects alpha; the conversion places the image on white beforehand
    let pdf = convert_image(&src, "image/png", false, &cfg(), &t.path().join("w")).unwrap();
    assert_eq!(pages(&pdf), 1);
    let flat = image::open(t.path().join("w").join("flattened.png")).unwrap().to_rgb8();
    assert_eq!(flat.get_pixel(10, 10).0, [255, 0, 0]);
    assert_eq!(flat.get_pixel(90, 10).0, [255, 255, 255]);
}

#[test]
fn broken_image_is_a_user_error() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("broken.png");
    std::fs::write(&src, b"\x89PNG\r\n\x1a\nnot really a png").unwrap();
    let e = convert_image(&src, "image/png", false, &cfg(), &t.path().join("w")).unwrap_err();
    let ImageError::Bad(msg) = e else { panic!("{e:?}") };
    assert!(msg.starts_with("The image could not be converted"), "{msg}");
}
