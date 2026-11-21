//! Built-in OCR language default `deu+eng` (PaperRust; paperless-ngx: `eng`) and its fallback:
//! without German data the default falls back to `eng` instead of failing; explicitly chosen
//! languages that are missing still fail like OCRmyPDF (`MissingDependencyError`).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pl_ocr::config::{DEFAULT_OCR_LANGUAGE, OcrSettings};
use pl_ocr::engine::{OcrEngine, OcrRequest, Orientation, PageOcr, Word};
use pl_ocr::native::NativeOcrmypdf;
use pl_ocr::render::{PdfRenderer, RenderDoc};
use pl_ocr::{OcrArgs, Result};
use serde_json::json;

/// Engine with a fixed set of installed languages; records the languages of every request.
struct Engine {
    installed: Vec<String>,
    requested: Arc<Mutex<Vec<String>>>,
}

impl OcrEngine for Engine {
    fn name(&self) -> String {
        "fake".into()
    }
    fn recognize(&self, _: &Path, req: &OcrRequest) -> Result<PageOcr> {
        self.requested.lock().unwrap().push(req.languages.clone());
        let words = vec![Word { text: "Text".into(), x0: 1.0, y0: 1.0, x1: 50.0, y1: 10.0, conf: 90.0, line: (1, 1, 1) }];
        Ok(PageOcr { width: 64, height: 64, words, text: "Text\n\x0c".into() })
    }
    fn orientation(&self, _: &Path, _: Option<Duration>) -> Result<Option<Orientation>> {
        Ok(None)
    }
    fn languages(&self) -> Result<Vec<String>> {
        Ok(self.installed.clone())
    }
}

struct NoRenderer;
impl PdfRenderer for NoRenderer {
    fn name(&self) -> String {
        "none".into()
    }
    fn open<'a>(&'a self, _: &'a [u8]) -> Result<Box<dyn RenderDoc + 'a>> {
        unreachable!("image input")
    }
}

/// OCR one small gray PNG with `language`; returns the result and the languages the engine was asked for.
fn ocr(installed: &[&str], language: &str) -> (Result<()>, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("seite.png");
    image::GrayImage::from_fn(200, 120, |x, y| image::Luma([if (40..160).contains(&x) && (50..60).contains(&y) { 20 } else { 240 }]))
        .save(&input)
        .unwrap();
    let requested = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine { installed: installed.iter().map(|s| s.to_string()).collect(), requested: requested.clone() };
    let backend = NativeOcrmypdf::new(Some(Box::new(engine)), Box::new(NoRenderer), dir.path().join("pages"));
    let mut m = serde_json::Map::new();
    m.insert("input_file_or_options".into(), json!(input.to_string_lossy()));
    m.insert("output_file".into(), json!(dir.path().join("out.pdf").to_string_lossy()));
    m.insert("language".into(), json!(language));
    m.insert("output_type".into(), json!("pdf"));
    m.insert("progress_bar".into(), json!(false));
    m.insert("image_dpi".into(), json!(200));
    let r = backend.run(&OcrArgs(m));
    let langs = requested.lock().unwrap().clone();
    (r, langs)
}

#[test]
fn default_is_german_plus_english() {
    assert_eq!(DEFAULT_OCR_LANGUAGE, "deu+eng");
    assert_eq!(OcrSettings::default().ocr_language, "deu+eng");
}

#[test]
fn default_uses_both_languages_when_installed() {
    let (r, langs) = ocr(&["deu", "eng", "osd"], DEFAULT_OCR_LANGUAGE);
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(langs, vec!["deu+eng".to_string()]);
}

#[test]
fn default_falls_back_to_english_without_german_data() {
    let (r, langs) = ocr(&["eng", "osd"], DEFAULT_OCR_LANGUAGE);
    assert!(r.is_ok(), "default must not fail without German data: {r:?}");
    assert_eq!(langs, vec!["eng".to_string()]);
}

#[test]
fn explicitly_chosen_missing_language_still_fails() {
    let (r, langs) = ocr(&["eng", "osd"], "fra");
    let err = format!("{:?}", r.expect_err("missing explicit language must fail"));
    assert!(err.contains("MissingDependencyError"), "{err}");
    assert!(langs.is_empty());
}
