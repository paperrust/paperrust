//! End-to-end tests with real tools (counterpart to the upstream tests in
//! `paperless/tests/parsers/test_tesseract_parser.py`, which need real OCRmyPDF/Tesseract).
//!
//! Tools (each an environment variable or default location in the repo):
//! - PDFium: `PAPERRUST_PDFIUM_PATH`, otherwise `reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll`
//! - Tesseract: `PAPERRUST_TESSERACT_PATH` (+ `PAPERRUST_TESSDATA_DIR`), otherwise
//!   `tests/parity/pl-ocr/.tools/tesseract/bin/tesseract(.exe)` (see fetch_tesseract.py)
//!
//! If a tool is missing, the test ends immediately with a `SKIP` message on stderr (counts as passed;
//! reported in the report).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pl_ocr::config::{Mode, OcrConfig, OutputType};
use pl_ocr::engine::{OcrEngine, TesseractCli};
use pl_ocr::native::{NativeOcrmypdf, parse_document};
use pl_ocr::render::{PdfRenderer, PdfiumRenderer};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pdfium() -> Option<PdfiumRenderer> {
    static R: OnceLock<Option<PdfiumRenderer>> = OnceLock::new();
    R.get_or_init(|| {
        let p = std::env::var_os("PAPERRUST_PDFIUM_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll"));
        PdfiumRenderer::load(&p).ok()
    })
    .clone()
}

fn tesseract() -> Option<TesseractCli> {
    let (prog, data) = match std::env::var_os("PAPERRUST_TESSERACT_PATH") {
        Some(p) => (PathBuf::from(p), std::env::var_os("PAPERRUST_TESSDATA_DIR").map(PathBuf::from)),
        None => {
            let base = repo().join("tests/parity/pl-ocr/.tools/tesseract");
            let exe = if cfg!(windows) { "tesseract.exe" } else { "tesseract" };
            (base.join("bin").join(exe), Some(base.join("tessdata")))
        }
    };
    let t = TesseractCli { program: prog, tessdata_dir: data, psm: None };
    t.is_available().then_some(t)
}

fn upstream(name: &str) -> PathBuf {
    let a = repo().join("upstream/src/paperless/tests/samples/tesseract").join(name);
    if a.exists() { a } else { repo().join("upstream/src/paperless_testing/sample_files").join(name) }
}

macro_rules! need {
    ($($tool:ident),+) => {
        $( let Some($tool) = $tool() else {
            eprintln!("SKIP {}: {} not available", module_path!(), stringify!($tool));
            return;
        }; )+
    };
}

fn backend(t: Option<TesseractCli>, r: PdfiumRenderer, work: &Path) -> NativeOcrmypdf {
    NativeOcrmypdf::new(t.map(|t| Box::new(t) as Box<dyn OcrEngine>), Box::new(r), work.join("pages"))
}

fn cfg(f: impl FnOnce(&mut OcrConfig)) -> OcrConfig {
    let mut c = OcrConfig::defaults();
    c.threads_per_worker = 1;
    f(&mut c);
    c
}

fn assert_ordered(text: &str, parts: &[&str]) {
    let mut pos = 0;
    for p in parts {
        match text[pos..].find(p) {
            Some(i) => pos += i + p.len(),
            None => panic!("{p:?} not found (in order) in {text:?}"),
        }
    }
}

fn archive_text(p: &Path) -> String {
    let data = std::fs::read(p).unwrap();
    pl_ocr::text::post_process_text(pl_ocr::pdfwrite::extract_text(&data).as_deref()).unwrap_or_default()
}

// --------------------------------------------------------------------------------------------
// PDFium only
// --------------------------------------------------------------------------------------------

#[test]
fn pdfium_renders_pages() {
    need!(pdfium);
    let data = std::fs::read(upstream("multi-page-digital.pdf")).unwrap();
    let doc = pdfium.open(&data).unwrap();
    assert_eq!(doc.page_count(), 3);
    let (w, h) = doc.page_size_pt(0).unwrap();
    assert!(w > 500.0 && h > 700.0, "{w}x{h}");
    let r = doc.render(0, 72.0, true).unwrap();
    assert_eq!(r.width, w.ceil() as u32);
    assert!(!r.is_blank());
    let enc = std::fs::read(upstream("encrypted.pdf")).unwrap();
    assert!(pdfium.open(&enc).is_err() || pl_ocr::pdfwrite::load_pdf(&enc).is_err());
}

#[test]
fn thumbnail_webp_500px() {
    need!(pdfium);
    let tmp = tempfile::tempdir().unwrap();
    let out = pl_ocr::thumbnail::make_thumbnail_from_pdf(&pdfium, &upstream("multi-page-digital.pdf"), tmp.path()).unwrap();
    let img = image::open(&out).unwrap();
    assert_eq!(img.width(), 500);
    assert_eq!(out.file_name().unwrap(), "convert.webp");
}

#[test]
fn off_mode_without_ocr_engine() {
    // OCR_MODE=off needs neither Tesseract nor a renderer.
    need!(pdfium);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(None, pdfium, tmp.path());
    let c = cfg(|c| c.mode = Mode::Off);
    let r = parse_document(&b, &c, tmp.path(), &upstream("simple.png"), "image/png", true).unwrap();
    let a = r.archive_path.unwrap();
    assert_eq!(r.text, "");
    assert_eq!(pl_ocr::pdfwrite::page_count(&std::fs::read(&a).unwrap()), Some(1));
    let r = parse_document(&b, &c, tmp.path(), &upstream("multi-page-digital.pdf"), "application/pdf", true).unwrap();
    assert_ordered(&r.text.to_lowercase(), &["page 1", "page 2", "page 3"]);
    assert_ordered(&archive_text(&r.archive_path.unwrap()).to_lowercase(), &["page 1", "page 2", "page 3"]);
    // alpha PNG: img2pdf rejects (upstream behavior)
    let e = parse_document(&b, &c, tmp.path(), &upstream("simple-alpha.png"), "image/png", true).unwrap_err();
    assert!(e.message().contains("Refusing to work on images with alpha channel"), "{}", e.message());
}

#[test]
fn auto_digital_pdf_skip_text_without_tesseract() {
    // AUTO + text present + archive → skip_text; no page needs OCR → no Tesseract needed.
    need!(pdfium);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(None, pdfium, tmp.path());
    let r = parse_document(&b, &cfg(|_| {}), tmp.path(), &upstream("multi-page-digital.pdf"), "application/pdf", true).unwrap();
    let a = r.archive_path.unwrap();
    assert_ordered(&r.text.to_lowercase(), &["page 1", "page 2", "page 3"]);
    let doc = pl_ocr::pdfwrite::load_pdf(&std::fs::read(&a).unwrap()).unwrap();
    assert!(doc.catalog().unwrap().has(b"OutputIntents"));
    let side = std::fs::read_to_string(tmp.path().join("sidecar.txt")).unwrap();
    assert_eq!(side, "[OCR skipped on page(s) 1-3]");
}

// --------------------------------------------------------------------------------------------
// PDFium + Tesseract
// --------------------------------------------------------------------------------------------

#[test]
fn e2e_images() {
    need!(pdfium, tesseract);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(Some(tesseract), pdfium, tmp.path());
    let cases: &[(&str, &str, Option<i64>)] = &[
        ("simple.png", "image/png", None),
        ("simple-alpha.png", "image/png", None),
        ("simple-no-dpi.png", "image/png", Some(72)),
        ("simple.jpg", "image/jpeg", None),
        ("simple.bmp", "image/bmp", None),
        ("simple.tif", "image/tiff", None),
        ("simple.gif", "image/gif", Some(200)),
    ];
    for (name, mime, dpi) in cases {
        let d = tmp.path().join(name.replace('.', "_"));
        let c = cfg(|c| c.image_dpi = *dpi);
        let r = parse_document(&b, &c, &d, &upstream(name), mime, true).unwrap_or_else(|e| panic!("{name}: {}", e.message()));
        let a = r.archive_path.clone().unwrap_or_else(|| panic!("{name}: no archive"));
        assert!(r.text.to_lowercase().contains("this is a test document"), "{name}: {:?}", r.text);
        assert!(archive_text(&a).to_lowercase().contains("this is a test document"), "{name}: archive text");
    }
    let d = tmp.path().join("webp");
    let r = parse_document(&b, &cfg(|c| c.image_dpi = Some(72)), &d, &upstream("document.webp"), "image/webp", true).unwrap();
    assert!(r.text.to_lowercase().contains("webp document"), "{:?}", r.text);
}

#[test]
fn e2e_multi_page_tiffs() {
    need!(pdfium, tesseract);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(Some(tesseract), pdfium, tmp.path());
    for name in ["multi-page-images.tiff", "multi-page-images-alpha.tiff", "multi-page-images-alpha-rgb.tiff"] {
        let d = tmp.path().join(name.replace('.', "_"));
        let r = parse_document(&b, &cfg(|_| {}), &d, &upstream(name), "image/tiff", true).unwrap_or_else(|e| panic!("{name}: {}", e.message()));
        assert_ordered(&r.text.to_lowercase(), &["page 1", "page 2", "page 3"]);
        let a = r.archive_path.unwrap();
        assert_eq!(pl_ocr::pdfwrite::page_count(&std::fs::read(&a).unwrap()), Some(3), "{name}");
    }
}

#[test]
fn e2e_image_pdf_modes() {
    need!(pdfium, tesseract);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(Some(tesseract), pdfium, tmp.path());
    let src = upstream("multi-page-images.pdf");
    // auto (no text) → normal OCR
    let d = tmp.path().join("auto");
    let r = parse_document(&b, &cfg(|_| {}), &d, &src, "application/pdf", true).unwrap();
    assert_ordered(&r.text.to_lowercase(), &["page 1", "page 2", "page 3"]);
    assert_ordered(&archive_text(&r.archive_path.unwrap()).to_lowercase(), &["page 1", "page 2", "page 3"]);
    // auto without archive → OCR, but no archive_path
    let d = tmp.path().join("noarch");
    let r = parse_document(&b, &cfg(|_| {}), &d, &src, "application/pdf", false).unwrap();
    assert!(r.archive_path.is_none());
    assert_ordered(&r.text.to_lowercase(), &["page 1", "page 2", "page 3"]);
    // redo, pages=2 → page 3 missing
    let d = tmp.path().join("redo2");
    let r = parse_document(&b, &cfg(|c| {
        c.mode = Mode::Redo;
        c.pages = Some(2);
    }), &d, &src, "application/pdf", true)
    .unwrap();
    let t = r.text.to_lowercase();
    assert_ordered(&t, &["page 1", "page 2"]);
    assert!(!t.contains("page 3"), "{t}");
    // force, pages=1 → only page 1
    let d = tmp.path().join("force1");
    let r = parse_document(&b, &cfg(|c| {
        c.mode = Mode::Force;
        c.pages = Some(1);
    }), &d, &src, "application/pdf", true)
    .unwrap();
    let t = r.text.to_lowercase();
    assert!(t.contains("page 1") && !t.contains("page 2") && !t.contains("page 3"), "{t}");
}

#[test]
fn e2e_mixed_and_digital() {
    need!(pdfium, tesseract);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(Some(tesseract), pdfium, tmp.path());
    // test_multi_page_mixed_skip_mode
    let d = tmp.path().join("mixed");
    let r = parse_document(&b, &cfg(|_| {}), &d, &upstream("multi-page-mixed.pdf"), "application/pdf", true).unwrap();
    assert_ordered(&r.text.to_lowercase(), &["page 1", "page 2", "page 3", "page 4", "page 5", "page 6"]);
    let side = std::fs::read_to_string(d.join("sidecar.txt")).unwrap();
    assert!(side.contains("[OCR skipped on page(s) 4-6]"), "{side}");
    // test_multi_page_mixed_skip_noarchive
    let d = tmp.path().join("mixed-noarch");
    let r = parse_document(&b, &cfg(|_| {}), &d, &upstream("multi-page-mixed.pdf"), "application/pdf", false).unwrap();
    assert!(r.archive_path.is_none());
    assert_ordered(&r.text.to_lowercase(), &["page 4", "page 5", "page 6"]);
    // test_single_page_mixed_redo_mode
    let d = tmp.path().join("redo");
    let r = parse_document(&b, &cfg(|c| c.mode = Mode::Redo), &d, &upstream("single-page-mixed.pdf"), "application/pdf", true).unwrap();
    assert!(r.archive_path.is_some());
    // Known difference 1: lopdf reads the original content first, then the appended OCR layer
    // (pdftotext -layout sorts by position) → only check presence.
    let t = r.text.to_lowercase();
    for part in [
        "this is some normal text, present on page 1 of the document.",
        "this is some text, but in an image, also on page 1.",
        "this is further text on page 1.",
    ] {
        assert!(t.contains(part), "{part:?} missing in {t:?}");
    }
    let side = std::fs::read_to_string(d.join("sidecar.txt")).unwrap().to_lowercase();
    assert!(side.contains("this is some text, but in an image, also on page 1."), "{side}");
    assert!(!side.contains("this is some normal text, present on page 1 of the document."), "{side}");
    // test_multi_page_digital_pages_2 (all modes)
    for mode in [Mode::Auto, Mode::Redo, Mode::Force] {
        let d = tmp.path().join(format!("digital-{mode}"));
        let r = parse_document(&b, &cfg(|c| {
            c.mode = mode;
            c.pages = Some(2);
        }), &d, &upstream("multi-page-digital.pdf"), "application/pdf", true)
        .unwrap_or_else(|e| panic!("{mode}: {}", e.message()));
        assert!(r.archive_path.is_some());
        assert_ordered(&r.text.to_lowercase(), &["page 1", "page 2", "page 3"]);
    }
}

#[test]
fn e2e_special_pdfs() {
    need!(pdfium, tesseract);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(Some(tesseract), pdfium, tmp.path());
    // signed → no archive, text from the original
    let d = tmp.path().join("signed");
    let r = parse_document(&b, &cfg(|_| {}), &d, &upstream("signed.pdf"), "application/pdf", true).unwrap();
    assert!(r.archive_path.is_none());
    assert!(r.text.contains("This is a digitally signed PDF"), "{:?}", r.text);
    // encrypted → no archive, empty text
    let d = tmp.path().join("enc");
    let r = parse_document(&b, &cfg(|_| {}), &d, &upstream("encrypted.pdf"), "application/pdf", true).unwrap();
    assert!(r.archive_path.is_none());
    assert_eq!(r.text, "");
    // with-form: auto and force
    for mode in [Mode::Auto, Mode::Force] {
        let d = tmp.path().join(format!("form-{mode}"));
        let r = parse_document(&b, &cfg(|c| c.mode = mode), &d, &upstream("with-form.pdf"), "application/pdf", true).unwrap();
        assert_ordered(&r.text, &["Please enter your name in here:", "This is a PDF document with a form."]);
    }
    // redo without archive
    let d = tmp.path().join("form-redo");
    let r = parse_document(&b, &cfg(|c| c.mode = Mode::Redo), &d, &upstream("with-form.pdf"), "application/pdf", false).unwrap();
    assert!(r.archive_path.is_none());
    assert_ordered(&r.text, &["Please enter your name in here:", "This is a PDF document with a form."]);
    // tagged without text → OCR still runs
    let d = tmp.path().join("tagged");
    let r = parse_document(&b, &cfg(|_| {}), &d, &upstream("tagged-but-no-text.pdf"), "application/pdf", true);
    assert!(r.is_ok(), "{:?}", r.err().map(|e| e.message()));
}

#[test]
fn e2e_rotated() {
    need!(pdfium, tesseract);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(Some(tesseract), pdfium, tmp.path());
    let r = parse_document(&b, &cfg(|c| c.rotate = true), tmp.path(), &upstream("rotated.pdf"), "application/pdf", true).unwrap();
    assert_ordered(
        &r.text,
        &[
            "This is the text that appears on the first page. It\u{2019}s a lot of text.",
            "Even if the pages are rotated, OCRmyPDF still gets the job done.",
            "This is a really weird file with lots of nonsense text.",
            "If you read this, it\u{2019}s your own fault. Also check your screen orientation.",
        ],
    );
}

#[test]
fn e2e_heic_is_known_difference() {
    need!(pdfium, tesseract);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(Some(tesseract), pdfium, tmp.path());
    // upstream (pillow_heif) recognizes "pizza"; here an HEVC decoder is missing → ParseError.
    let e = parse_document(&b, &cfg(|_| {}), tmp.path(), &upstream("simple.heic"), "image/heic", true).unwrap_err();
    assert!(e.message().contains("HEIC"), "{}", e.message());
}

#[test]
fn pdfa_output_types() {
    need!(pdfium, tesseract);
    let tmp = tempfile::tempdir().unwrap();
    let b = backend(Some(tesseract), pdfium, tmp.path());
    for (ot, ver, oi) in [(OutputType::Pdf, None, false), (OutputType::PdfA1, Some("1.4"), true), (OutputType::PdfA3, Some("1.7"), true)] {
        let d = tmp.path().join(ot.as_str());
        let r = parse_document(&b, &cfg(|c| c.output_type = ot), &d, &upstream("simple.png"), "image/png", true).unwrap();
        let data = std::fs::read(r.archive_path.unwrap()).unwrap();
        let doc = pl_ocr::pdfwrite::load_pdf(&data).unwrap();
        assert_eq!(doc.catalog().unwrap().has(b"OutputIntents"), oi, "{ot}");
        if let Some(v) = ver {
            assert_eq!(doc.version, v);
            let xmp = String::from_utf8_lossy(&data);
            let part = ot.pdfa_part().unwrap();
            assert!(xmp.contains(&format!("<pdfaid:part>{part}</pdfaid:part>")), "{ot}");
        }
    }
}
