//! PP-OCR engine through the real ingestion path (render → OCR → archive PDF with text layer).
//! Needs ONNX Runtime + model files: `PAPERRUST_ONNXRUNTIME_PATH`, `PAPERRUST_OCR_MODEL_DIR`
//! (and PDFium via `PAPERRUST_PDFIUM_PATH` or the reference venv); otherwise the test prints
//! `SKIP` and ends.

use std::path::{Path, PathBuf};

use pl_ocr::config::OcrConfig;
use pl_ocr::engine::{OcrEngine, OcrRequest};
use pl_ocr::native::{NativeOcrmypdf, parse_document};
use pl_ocr::ppocr::PpOcrEngine;
use pl_ocr::render::{PdfiumRenderer, Raster};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sample(name: &str) -> PathBuf {
    repo().join("upstream/src/paperless/tests/samples/tesseract").join(name)
}

fn engine() -> Option<PpOcrEngine> {
    if std::env::var_os("PAPERRUST_OCR_MODEL_DIR").is_none() {
        eprintln!("SKIP: PAPERRUST_OCR_MODEL_DIR not set");
        return None;
    }
    match PpOcrEngine::new(None) {
        Ok(e) => Some(e),
        Err(e) => {
            eprintln!("SKIP: PP-OCR not available: {e}");
            None
        }
    }
}

#[test]
fn ppocr_words_have_boxes_and_confidence() {
    let Some(e) = engine() else { return };
    let r = Raster::from_dynamic(&image::open(sample("simple.png")).unwrap(), 300.0);
    let req = OcrRequest { languages: "deu+eng".into(), dpi: Some(300), ..Default::default() };
    let p = e.recognize_raster(&r, Path::new("unused.tif"), &req).unwrap();
    assert!(p.text.to_lowercase().contains("test document"), "{:?}", p.text);
    assert_eq!((p.width, p.height), (r.width, r.height));
    assert!(p.words.len() >= 4, "{:?}", p.words);
    for w in &p.words {
        assert!(w.x0 < w.x1 && w.y0 < w.y1, "{w:?}");
        assert!(w.x1 <= f64::from(r.width) + 1.0 && w.y1 <= f64::from(r.height) + 1.0, "{w:?}");
        assert!((0.0..=100.0).contains(&w.conf) && w.conf > 50.0, "{w:?}");
    }
    assert!(e.name().starts_with("ppocr"), "{}", e.name());
    // non-Latin language without tesseract: clear error, no silent PP-OCR run
    assert!(e.check_languages("rus").is_err());
    assert!(e.check_languages("deu+eng").is_ok());
}

#[test]
fn ingestion_path_with_ppocr_text_layer() {
    let Some(e) = engine() else { return };
    let pdfium = std::env::var_os("PAPERRUST_PDFIUM_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll"));
    let Ok(pdfium) = PdfiumRenderer::load(&pdfium) else {
        eprintln!("SKIP ingestion_path_with_ppocr_text_layer: pdfium not available");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let backend = NativeOcrmypdf::new(Some(Box::new(e)), Box::new(pdfium), tmp.path().join("pages"));
    let mut cfg = OcrConfig::defaults();
    cfg.language = "deu+eng".into();
    cfg.threads_per_worker = 2;
    let r = parse_document(&backend, &cfg, tmp.path(), &sample("simple.png"), "image/png", true).unwrap();
    assert!(r.text.to_lowercase().contains("test document"), "{:?}", r.text);
    let archive = std::fs::read(r.archive_path.unwrap()).unwrap();
    let layer = pl_ocr::text::post_process_text(pl_ocr::pdfwrite::extract_text(&archive).as_deref()).unwrap_or_default();
    assert!(layer.to_lowercase().contains("test document"), "{layer:?}");
    // multi-page TIFF through the parallel page pipeline, page order kept
    let r = parse_document(&backend, &cfg, tmp.path(), &sample("multi-page-images.tiff"), "image/tiff", true).unwrap();
    let t = r.text.to_lowercase();
    // PP-OCRv6 small drops the trailing "3." on page 3 at 2560 px (the Python reference too)
    let (a, b) = (t.find("page 1"), t.find("page 2"));
    assert!(a.is_some() && a < b, "{t:?}");
    assert_eq!(t.matches("multi page document").count(), 3, "{t:?}");
}

/// Regression (Jetson run 2026-11-26: AI transcription with PP-OCR sent nothing): the ingestion path
/// must write `ocr-lines.json` with engine `ppocr` and usable per-line confidences, otherwise the
/// uncertain-line selection silently never fires.
#[test]
fn ppocr_writes_line_data_for_ai_transcription() {
    let Some(e) = engine() else { return };
    let pdfium = std::env::var_os("PAPERRUST_PDFIUM_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll"));
    let Ok(pdfium) = PdfiumRenderer::load(&pdfium) else {
        eprintln!("SKIP ppocr_writes_line_data_for_ai_transcription: pdfium not available");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let backend = NativeOcrmypdf::new(Some(Box::new(e)), Box::new(pdfium), tmp.path().join("pages"));
    let mut cfg = OcrConfig::defaults();
    cfg.language = "deu+eng".into();
    cfg.threads_per_worker = 2;
    // optional: a local sample (e.g. a synthetic handwriting page) to dump per-line confidences
    let input = std::env::var_os("PAPERRUST_PPOCR_SAMPLE").map(PathBuf::from).unwrap_or_else(|| sample("simple.png"));
    let mime = if input.extension().is_some_and(|x| x == "pdf") { "application/pdf" } else { "image/png" };
    parse_document(&backend, &cfg, tmp.path(), &input, mime, true).unwrap();
    let lines = pl_ocr::uncertain::read_for_dir(tmp.path()).expect("ocr-lines.json written next to the sidecar");
    assert_eq!(lines.engine, "ppocr");
    let page = lines.pages.iter().flatten().next().expect("one OCRed page");
    assert!(!page.lines.is_empty());
    for l in &page.lines {
        eprintln!("{:6.1}  {}", l.conf, l.text);
        assert!((0.0..=100.0).contains(&l.conf), "{l:?}");
    }
}
