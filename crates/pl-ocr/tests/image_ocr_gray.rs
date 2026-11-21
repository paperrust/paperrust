//! Image files (PNG/JPEG …) are recognized on our own grayscale image, like PDF pages
//! (Jetson check 2026-11-21: a deskewed color phone photo lost the umlaut dots of one word when
//! Tesseract converted the color image itself; on gray input it was stable at every angle).
//! The archive PDF keeps the color image.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pl_ocr::OcrArgs;
use pl_ocr::Result;
use pl_ocr::engine::{OcrEngine, OcrRequest, Orientation, PageOcr, Word};
use pl_ocr::native::NativeOcrmypdf;
use pl_ocr::render::{PdfRenderer, Raster, RenderDoc};
use serde_json::json;

/// Records the channel count of every raster it is asked to recognize.
struct Engine(Arc<Mutex<Vec<u8>>>);

impl OcrEngine for Engine {
    fn name(&self) -> String {
        "fake".into()
    }
    fn recognize(&self, _: &Path, _: &OcrRequest) -> Result<PageOcr> {
        let words = vec![Word { text: "Farbfoto".into(), x0: 1.0, y0: 1.0, x1: 50.0, y1: 10.0, conf: 90.0, line: (1, 1, 1) }];
        Ok(PageOcr { width: 64, height: 64, words, text: "Farbfoto\n\x0c".into() })
    }
    fn recognize_raster(&self, raster: &Raster, file: &Path, req: &OcrRequest) -> Result<PageOcr> {
        self.0.lock().unwrap().push(raster.channels);
        self.recognize(file, req)
    }
    fn orientation(&self, _: &Path, _: Option<Duration>) -> Result<Option<Orientation>> {
        Ok(None)
    }
    fn languages(&self) -> Result<Vec<String>> {
        Ok(vec!["eng".into()])
    }
}

/// Images never reach the PDF renderer.
struct NoRenderer;
impl PdfRenderer for NoRenderer {
    fn name(&self) -> String {
        "none".into()
    }
    fn open<'a>(&'a self, _: &'a [u8]) -> Result<Box<dyn RenderDoc + 'a>> {
        unreachable!("image input must not be rendered as PDF")
    }
}

/// Colored "photo": brownish table, light paper with a dark text bar and a red stamp.
fn color_png(path: &Path) {
    let (w, h) = (400u32, 300u32);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        if !(40..360).contains(&x) || !(30..270).contains(&y) {
            image::Rgb([70, 58, 48])
        } else if (80..320).contains(&x) && (100..112).contains(&y) {
            image::Rgb([20, 20, 30])
        } else if (250..300).contains(&x) && (180..220).contains(&y) {
            image::Rgb([200, 30, 30])
        } else {
            image::Rgb([215, 210, 200])
        }
    });
    img.save(path).unwrap();
}

#[test]
fn color_image_is_recognized_in_grayscale() {
    for deskew in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("foto.png");
        color_png(&input);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let backend = NativeOcrmypdf::new(Some(Box::new(Engine(seen.clone()))), Box::new(NoRenderer), dir.path().join("pages"));
        let mut m = serde_json::Map::new();
        m.insert("input_file_or_options".into(), json!(input.to_string_lossy()));
        m.insert("output_file".into(), json!(dir.path().join("out.pdf").to_string_lossy()));
        m.insert("sidecar".into(), json!(dir.path().join("sidecar.txt").to_string_lossy()));
        m.insert("language".into(), json!("eng"));
        m.insert("output_type".into(), json!("pdf"));
        m.insert("progress_bar".into(), json!(false));
        m.insert("image_dpi".into(), json!(200));
        m.insert("deskew".into(), json!(deskew));
        backend.run(&OcrArgs(m)).expect("color PNG");
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen, vec![1], "deskew={deskew}: OCR must get a 1-channel (gray) raster");
        let sidecar = std::fs::read_to_string(dir.path().join("sidecar.txt")).unwrap();
        assert!(sidecar.contains("Farbfoto"), "{sidecar}");
    }
}
