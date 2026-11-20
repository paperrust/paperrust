//! Test log 2026-11-16 C-017: a valid PDF page at the PDF maximum size (14400 × 14400 pt)
//! must not abort OCR with `DecompressionBombError`. It is rasterized at such a low
//! resolution that the pixels stay below the limit (`max_image_mpixels`).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pl_ocr::OcrArgs;
use pl_ocr::engine::{OcrEngine, OcrRequest, Orientation, PageOcr, Word};
use pl_ocr::native::NativeOcrmypdf;
use pl_ocr::pdfwrite::{self, ImagePageSpec, OutputMeta, PageImage};
use pl_ocr::render::{PdfRenderer, Raster, RenderDoc, fit_dpi, page_pixels};
use pl_ocr::Result;
use serde_json::json;

const SIDE_PT: f64 = 14_400.0;

struct Engine;

impl OcrEngine for Engine {
    fn name(&self) -> String {
        "fake".into()
    }
    fn recognize(&self, _: &Path, req: &OcrRequest) -> Result<PageOcr> {
        let words = vec![Word { text: "Riesenseite".into(), x0: 1.0, y0: 1.0, x1: 50.0, y1: 10.0, conf: 90.0, line: (1, 1, 1) }];
        Ok(PageOcr { width: 64, height: 64, words, text: format!("Riesenseite dpi {}\n\x0c", req.dpi.unwrap_or(0)) })
    }
    fn orientation(&self, _: &Path, _: Option<Duration>) -> Result<Option<Orientation>> {
        Ok(None)
    }
    fn languages(&self) -> Result<Vec<String>> {
        Ok(vec!["eng".into()])
    }
}

/// Renderer without PDFium: reports the huge page, remembers the requested resolution and returns
/// a small raster (a real 80000 × 80000 image would be the error at issue).
struct Renderer(Arc<Mutex<Vec<f64>>>);
struct Doc<'a>(&'a Renderer);

impl PdfRenderer for Renderer {
    fn name(&self) -> String {
        "fake".into()
    }
    fn open<'a>(&'a self, _: &'a [u8]) -> Result<Box<dyn RenderDoc + 'a>> {
        Ok(Box::new(Doc(self)))
    }
}

impl RenderDoc for Doc<'_> {
    fn page_count(&self) -> usize {
        1
    }
    fn page_size_pt(&self, _: usize) -> Result<(f64, f64)> {
        Ok((SIDE_PT, SIDE_PT))
    }
    fn render(&self, _: usize, dpi: f64, gray: bool) -> Result<Raster> {
        self.0.0.lock().unwrap().push(dpi);
        let ch: u8 = if gray { 1 } else { 3 };
        Ok(Raster { width: 64, height: 64, channels: ch, data: vec![255; 64 * 64 * usize::from(ch)], dpi })
    }
}

/// Image PDF with one page 14400 × 14400 pt (200 × 200 pixels at 1 dpi).
fn giant_page_pdf() -> Vec<u8> {
    let data = (0..200 * 200).map(|k| if k % 7 == 0 { 0 } else { 255 }).collect();
    let raster = Raster { width: 200, height: 200, channels: 1, data, dpi: 1.0 };
    let spec = ImagePageSpec { image: PageImage::Raw(raster), dpi_x: 1.0, dpi_y: 1.0, rotate: 0, ocr: None };
    pdfwrite::build_from_images(&[spec], &OutputMeta { output_type: pl_ocr::config::OutputType::Pdf, ..Default::default() }).unwrap()
}

#[test]
fn fit_dpi_keeps_pixels_under_the_limit() {
    // normal page: unchanged
    assert_eq!(fit_dpi(595.0, 842.0, 300.0, 250_000_000), 300.0);
    // PDF maximum size at 400 dpi would be 80000 × 80000 = 6.4 billion pixels
    let d = fit_dpi(SIDE_PT, SIDE_PT, 400.0, 250_000_000);
    let (w, h) = page_pixels(SIDE_PT, SIDE_PT, d);
    assert!(w * h <= 250_000_000.0, "{d} dpi → {w}x{h}");
    assert!(d > 78.0 && d < 80.0, "{d}");
    // page side at most 65000 pixels, even without an area limit
    let d = fit_dpi(SIDE_PT, 10.0, 400.0, u64::MAX);
    assert!(page_pixels(SIDE_PT, 10.0, d).0 <= 65_000.0);
    // invalid dimensions: change nothing
    assert_eq!(fit_dpi(0.0, 10.0, 300.0, 1), 300.0);
}

#[test]
fn giant_page_is_rendered_at_lower_dpi_instead_of_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("riesenseite.pdf");
    std::fs::write(&input, giant_page_pdf()).unwrap();
    let dpis = Arc::new(Mutex::new(Vec::new()));
    let backend = NativeOcrmypdf::new(Some(Box::new(Engine)), Box::new(Renderer(dpis.clone())), dir.path().join("pages"));
    let mut m = serde_json::Map::new();
    m.insert("input_file_or_options".into(), json!(input.to_string_lossy()));
    m.insert("output_file".into(), json!(dir.path().join("out.pdf").to_string_lossy()));
    m.insert("sidecar".into(), json!(dir.path().join("sidecar.txt").to_string_lossy()));
    m.insert("language".into(), json!("eng"));
    m.insert("output_type".into(), json!("pdf"));
    m.insert("progress_bar".into(), json!(false));
    // like a page with text/vectors: 400 dpi requested
    m.insert("oversample".into(), json!(400));
    m.insert("max_image_mpixels".into(), json!(250.0));
    backend.run(&OcrArgs(m)).expect("giant page must not be rejected");
    let dpis = dpis.lock().unwrap().clone();
    assert_eq!(dpis.len(), 1, "{dpis:?}");
    let (w, h) = page_pixels(SIDE_PT, SIDE_PT, dpis[0]);
    assert!(dpis[0] < 400.0 && w * h <= 250_000_000.0, "rendered at {} dpi = {w}x{h}", dpis[0]);
    let sidecar = std::fs::read_to_string(dir.path().join("sidecar.txt")).unwrap();
    assert!(sidecar.contains("Riesenseite"), "{sidecar}");
}
