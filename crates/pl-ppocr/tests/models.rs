//! Model tests (need ONNX Runtime + model files, therefore `#[ignore]`):
//!
//! ```text
//! PL_PPOCR_MODEL_DIR=<dir with PP-OCRv6_*_small.onnx, pp-lcnet_x1_0_doc_ori.onnx>
//! PL_PPOCR_ORT_LIB=<path to onnxruntime.dll / libonnxruntime.so>   (optional, else system search)
//! PL_PPOCR_CORPUS=<results/ocr-eval/2026-11-24/corpus>               (synthetic pages + .gt.txt)
//! PL_PPOCR_DEHTR=<path to the DE·HTR v2 model.onnx>                  (handwriting pass test only)
//! cargo test -p pl-ppocr -j 2 -- --ignored
//! ```

use std::collections::HashSet;
use std::path::PathBuf;

use pl_ppocr::image::Image;
use pl_ppocr::orient::{OrientModel, Orientation};
use pl_ppocr::pipeline::{OcrParams, PpOcr};
use pl_ppocr::runtime::Runtime;
use pl_ppocr::runtime::ort_rt::OrtRuntime;

fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn setup() -> Option<(OrtRuntime, PathBuf, PathBuf)> {
    let (Some(models), Some(corpus)) = (env("PL_PPOCR_MODEL_DIR"), env("PL_PPOCR_CORPUS")) else {
        eprintln!("skipped: PL_PPOCR_MODEL_DIR / PL_PPOCR_CORPUS not set");
        return None;
    };
    let rt = OrtRuntime::load(&env("PL_PPOCR_ORT_LIB").into_iter().collect::<Vec<_>>()).expect("onnxruntime");
    Some((rt, models, corpus))
}

fn load(p: &std::path::Path) -> Image {
    let img = image::open(p).expect("image").to_rgb8();
    let (w, h) = img.dimensions();
    Image::new(w as usize, h as usize, 3, img.into_raw()).expect("buffer")
}

/// `tools/score.py` word recall: distinct GT words (>= 3 chars, lower case, `\w+`) found in the output.
fn word_recall(gt: &str, out: &str) -> f64 {
    let words = |s: &str| -> HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| w.chars().count() >= 3).map(|w| w.to_lowercase()).collect()
    };
    let (g, o) = (words(gt), words(out));
    if g.is_empty() { 1.0 } else { g.intersection(&o).count() as f64 / g.len() as f64 }
}

#[test]
#[ignore = "needs ONNX Runtime and the PP-OCR model files (see module docs)"]
fn dense_page_is_read_with_word_boxes_on_the_image() {
    let Some((rt, models, corpus)) = setup() else { return };
    let det = rt.load(&models.join("PP-OCRv6_det_small.onnx"), 4).expect("det");
    let rec = rt.load(&models.join("PP-OCRv6_rec_small.onnx"), 4).expect("rec");
    let ocr = PpOcr::new(det, rec, OcrParams::default()).expect("ppocr");
    let name = "dense_01_invoice_arial_de";
    let img = load(&corpus.join(format!("{name}.png")));
    let page = ocr.recognize(&img).expect("recognize");
    let gt = std::fs::read_to_string(corpus.join(format!("{name}.gt.txt"))).expect("gt");
    let r = word_recall(&gt, &page.text());
    assert!(r >= 0.97, "word recall {r}");
    // every word box lies on the image and inside its line's box (plus a small tolerance)
    for (_, l) in &page.lines {
        let lb = l.bbox();
        for w in &l.words {
            assert!(w.bbox[0] >= 0.0 && w.bbox[2] <= img.width as f32 && w.bbox[1] >= 0.0 && w.bbox[3] <= img.height as f32);
            assert!(w.bbox[0] >= lb[0] - 2.0 && w.bbox[2] <= lb[2] + 2.0, "{w:?} outside {lb:?}");
            assert!((0.0..=1.0).contains(&w.conf));
        }
    }
}

#[test]
#[ignore = "needs ONNX Runtime and the orientation model (see module docs)"]
fn orientation_model_detects_all_four_rotations() {
    let Some((rt, models, corpus)) = setup() else { return };
    let cls = Orientation::new(rt.load(&models.join("pp-lcnet_x1_0_doc_ori.onnx"), 1).expect("model"), OrientModel::PpLcnet);
    let img = load(&corpus.join("dense_08_letter_times_de.png"));
    for rot in [0u16, 90, 180, 270] {
        let r = cls.classify(&img.rotate_cw(rot)).expect("classify");
        assert_eq!(r.rotate_cw, (360 - rot) % 360, "applied {rot}: {r:?}");
    }
}

#[test]
#[ignore = "needs ONNX Runtime, the PP-OCR models, PL_PPOCR_DEHTR (DE·HTR v2 model file) and PL_PPOCR_HW_SAMPLE (local handwriting page)"]
fn handwriting_pass_improves_real_handwriting_lines() {
    let Some((rt, models, corpus)) = setup() else { return };
    let Some(dehtr) = env("PL_PPOCR_DEHTR") else {
        eprintln!("skipped: PL_PPOCR_DEHTR not set");
        return;
    };
    let det = rt.load(&models.join("PP-OCRv6_det_small.onnx"), 4).expect("det");
    let rec = rt.load(&models.join("PP-OCRv6_rec_small.onnx"), 4).expect("rec");
    let ocr = PpOcr::new(det, rec, OcrParams::default()).expect("ppocr");
    let htr = pl_ppocr::htr::Htr::new(rt.load(&dehtr, 4).expect("dehtr"));
    let pass = pl_ppocr::HtrPass { htr: &htr, below: 0.95, max_lines: pl_ppocr::pipeline::HTR_MAX_LINES };
    // local handwriting page `<name>.png` + `<name>.gt.txt` in the corpus (never part of the repo)
    let Some(name) = std::env::var("PL_PPOCR_HW_SAMPLE").ok() else {
        eprintln!("skipped: PL_PPOCR_HW_SAMPLE not set");
        return;
    };
    let img = load(&corpus.join(format!("{name}.png")));
    let gt = std::fs::read_to_string(corpus.join(format!("{name}.gt.txt"))).expect("gt");
    let plain = word_recall(&gt, &ocr.recognize(&img).expect("plain").text());
    let page = ocr.recognize_with(&img, Some(&pass)).expect("with htr");
    let with = word_recall(&gt, &page.text());
    assert!(with >= plain + 0.05, "recall {plain} -> {with}");
    for (_, l) in &page.lines {
        for w in &l.words {
            assert!(w.bbox[0] <= w.bbox[2] && w.bbox[1] <= w.bbox[3], "{w:?}");
        }
    }
}
