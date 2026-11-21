//! Round trip: generate label PDF → rasterise with PDFium → cut out one label and paste it onto a
//! synthetic "scanned" page (slightly blurred, with text dummies) → image PDF →
//! barcode detection of the consumer (pl-pdfops `scan_pdf` + `RxingReader`, like `BarcodePlugin`) →
//! correct ASN, splitting at ASN pages (ENABLE_BARCODES + ENABLE_ASN_BARCODE) and at separator sheets.
//!
//! Needs PDFium: `PAPERRUST_PDFIUM_PATH`, otherwise `reference/venv/.../pypdfium2_raw/pdfium.dll`;
//! if both are missing, the test is skipped with a notice.
//! ```text
//! cargo test -p pl-labels --test roundtrip -j 2 -- --nocapture
//! ```

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use image::GrayImage;
use pl_labels::{CodeType, LabelJob, SeparatorJob};
use pl_pdfops::barcodes::{self, BarcodeConfig, RxingReader, ScanSettings};

const DPI: f64 = 300.0;

fn renderer() -> Option<pl_ocr::render::PdfiumRenderer> {
    if std::env::var_os("PAPERRUST_PDFIUM_PATH").is_none() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll");
        if p.is_file() {
            return pl_ocr::render::PdfiumRenderer::load(&p).ok();
        }
        eprintln!("SKIP: PDFium not found (set PAPERRUST_PDFIUM_PATH)");
        return None;
    }
    match pl_ocr::render::PdfiumRenderer::from_env() {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("SKIP: PDFium not loadable: {e}");
            None
        }
    }
}

fn render_page(r: &dyn pl_ocr::render::PdfRenderer, pdf: &[u8], page: usize) -> GrayImage {
    let doc = r.open(pdf).expect("open pdf");
    let ras = doc.render(page, DPI, true).expect("render");
    GrayImage::from_raw(ras.width, ras.height, ras.data).expect("gray raster")
}

fn mm(v: f64) -> u32 {
    (v / 25.4 * DPI).round() as u32
}

/// Cut out label `slot` from the rasterised sheet.
fn cut_label(sheet_img: &GrayImage, job: &pl_labels::Resolved, slot: u32) -> GrayImage {
    let s = job.sheet.slot(slot);
    image::imageops::crop_imm(sheet_img, mm(s.x), mm(s.y), mm(s.w), mm(s.h)).to_image()
}

/// Synthetic scan page (A4, 300 dpi): text line dummies, optionally a label at the top right.
fn scanned_page(label: Option<&GrayImage>, seed: u32) -> GrayImage {
    let (w, h) = (mm(210.0), mm(297.0));
    let mut img = GrayImage::from_pixel(w, h, image::Luma([246]));
    // "Text": grey bars of different lengths
    for line in 0..40u32 {
        let y = mm(60.0) + line * mm(5.5);
        let len = mm(60.0) + ((line * 37 + seed * 11) % 90) * mm(1.0);
        for yy in y..y + mm(2.2) {
            for xx in mm(25.0)..(mm(25.0) + len).min(w - mm(20.0)) {
                if !(xx / 7 + yy / 3 + seed).is_multiple_of(5) {
                    img.put_pixel(xx, yy, image::Luma([70]));
                }
            }
        }
    }
    if let Some(l) = label {
        // A label pasted on slightly skewed does not survive blur; hence only offset + paper tone
        image::imageops::overlay(&mut img, l, i64::from(w - mm(20.0) - l.width()), i64::from(mm(15.0)));
    }
    // Scanner blur
    image::imageops::blur(&img, 0.7)
}

/// Image PDF (one greyscale page per image, A4, Flate).
fn image_pdf(pages: &[GrayImage], path: &Path) {
    let mut objs: Vec<Vec<u8>> = Vec::new();
    let n = pages.len();
    objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 3 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes());
    for (i, img) in pages.iter().enumerate() {
        let (pid, cid, iid) = (3 + 3 * i, 4 + 3 * i, 5 + 3 * i);
        let _ = pid;
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595.276 841.89] /Resources << /XObject << /Im0 {iid} 0 R >> >> /Contents {cid} 0 R >>").into_bytes());
        let content = b"q 595.276 0 0 841.89 0 0 cm /Im0 Do Q".to_vec();
        let mut c = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        c.extend_from_slice(&content);
        c.extend_from_slice(b"\nendstream");
        objs.push(c);
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(img.as_raw()).unwrap();
        let data = enc.finish().unwrap();
        let mut o = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            img.width(),
            img.height(),
            data.len()
        )
        .into_bytes();
        o.extend_from_slice(&data);
        o.extend_from_slice(b"\nendstream");
        objs.push(o);
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    std::fs::write(path, out).unwrap();
}

fn scan(path: &Path, r: &dyn pl_ocr::render::PdfRenderer) -> Vec<barcodes::Barcode> {
    let s = ScanSettings { dpi: 300, upscale: 0.0, max_pages: 0 };
    barcodes::scan_pdf(path, s, r, &RxingReader).expect("scan")
}

fn asn_cfg() -> BarcodeConfig {
    BarcodeConfig { barcodes_enabled: true, barcode_enable_asn: true, ..Default::default() }
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A stack of two documents with ASN labels on the first page of each.
fn run_case(r: &pl_ocr::render::PdfiumRenderer, preset: &str, code: CodeType, start: u64, slots: (u32, u32), dir: &Path) {
    let job = LabelJob { preset: Some(preset.into()), code, start: Some(start), ..Default::default() }.resolve("ASN", 1).unwrap();
    let pdf = job.pdf().unwrap();
    let sheet = render_page(r, &pdf, 0);
    let (a, b) = (cut_label(&sheet, &job, slots.0), cut_label(&sheet, &job, slots.1));
    let stack =
        vec![scanned_page(Some(&a), 1), scanned_page(None, 2), scanned_page(Some(&b), 3), scanned_page(None, 4), scanned_page(None, 5)];
    let path: PathBuf = dir.join(format!("stack-{preset}-{}.pdf", code.as_str()));
    image_pdf(&stack, &path);
    let codes = scan(&path, r);
    let want_a = job.content(start + u64::from(slots.0));
    let want_b = job.content(start + u64::from(slots.1));
    let found: Vec<(i64, &str, &str)> = codes.iter().map(|c| (c.page, c.value.as_str(), c.format.as_str())).collect();
    eprintln!("{preset} {code:?}: {found:?}");
    assert_eq!(found, vec![(0, want_a.as_str(), code.zxing_name()), (2, want_b.as_str(), code.zxing_name())], "{preset} {code:?}");
    // Splitting like BarcodePlugin.get_separation_pages: an ASN page (except the first) starts a document
    let cfg = asn_cfg();
    let split = barcodes::separation_pages(&codes, &cfg);
    assert_eq!(split, BTreeMap::from([(2, true)]));
    // without ENABLE_ASN_BARCODE no separation at ASN labels (upstream behaviour)
    assert!(barcodes::separation_pages(&codes, &BarcodeConfig { barcodes_enabled: true, ..Default::default() }).is_empty());
    let parts = barcodes::separate_pages(&path, &split, "stack", dir, &path).unwrap();
    assert_eq!(parts.len(), 2);
    // every sub-document is processed again: ASN is then on page 0 → no further
    // separation, ASN is assigned (BarcodePlugin.asn)
    for (part, want) in parts.iter().zip([start + u64::from(slots.0), start + u64::from(slots.1)]) {
        let c = scan(part, r);
        assert!(barcodes::separation_pages(&c, &cfg).is_empty());
        assert_eq!(barcodes::located_asn(&c, &cfg), Ok(Some(want as i64)), "{preset} {code:?} {}", part.display());
    }
}

#[test]
fn labels_are_detected_by_the_consumer() {
    let Some(r) = renderer() else { return };
    let t = tmp();
    // smallest label (25.4 × 10 mm) – the real stress test
    run_case(&r, "avery-l4731", CodeType::Qr, 4242, (0, 100), t.path());
    run_case(&r, "avery-l4731", CodeType::DataMatrix, 777, (8, 188), t.path());
    run_case(&r, "avery-4780", CodeType::Qr, 1, (0, 39), t.path());
    run_case(&r, "avery-4780", CodeType::Code128, 123_456, (5, 6), t.path());
    run_case(&r, "avery-5160", CodeType::Code128, 90, (0, 29), t.path());
    run_case(&r, "avery-5167", CodeType::DataMatrix, 2_000_000, (3, 77), t.path());
}

#[test]
fn separator_sheet_splits_the_stack() {
    let Some(r) = renderer() else { return };
    let t = tmp();
    for code in [CodeType::Qr, CodeType::Code128] {
        let sep_pdf = SeparatorJob { code, lang: Some("de".into()), ..Default::default() }.pdf("PATCHT", false).unwrap();
        let sep = image::imageops::blur(&render_page(&r, &sep_pdf, 0), 0.7);
        let stack = vec![scanned_page(None, 1), scanned_page(None, 2), sep.clone(), scanned_page(None, 3), sep, scanned_page(None, 4)];
        let path = t.path().join(format!("sep-{}.pdf", code.as_str()));
        image_pdf(&stack, &path);
        let codes = scan(&path, &r);
        assert!(codes.iter().all(|c| c.value == "PATCHT"), "{codes:?}");
        let cfg = BarcodeConfig { barcodes_enabled: true, ..Default::default() };
        assert_eq!(barcodes::separation_pages(&codes, &cfg), BTreeMap::from([(2, false), (4, false)]), "{code:?}");
        let parts = barcodes::separate_pages(&path, &barcodes::separation_pages(&codes, &cfg), "sep", t.path(), &path).unwrap();
        assert_eq!(parts.len(), 3);
    }
}
