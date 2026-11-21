//! Straightening/cleaning end to end with real Tesseract (+ PDFium for PDF inputs).
//!
//! A text page is rendered with a system font (300 dpi), skewed and soiled
//! (specks, dark scanner edge, punch hole). Measured is the word rate (rendered words that
//! the recognition finds again) without and with preprocessing, and what goes into the archive PDF.
//!
//! Tools as in `tools.rs`; font: `PAPERRUST_TEST_FONT` or a common system font.
//! If something is missing, the test ends with `SKIP` on stderr (counts as passed).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use pl_ocr::config::{Clean, OcrConfig};
use pl_ocr::engine::{OcrEngine, TesseractCli};
use pl_ocr::native::{NativeOcrmypdf, parse_document};
use pl_ocr::preprocess;
use pl_ocr::render::{PdfiumRenderer, Raster};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pdfium() -> Option<PdfiumRenderer> {
    let p = std::env::var_os("PAPERRUST_PDFIUM_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll"));
    PdfiumRenderer::load(&p).ok()
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

fn font() -> Option<FontVec> {
    let mut c: Vec<PathBuf> = std::env::var_os("PAPERRUST_TEST_FONT").map(PathBuf::from).into_iter().collect();
    c.extend(
        [
            "C:/Windows/Fonts/arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
        ]
        .map(PathBuf::from),
    );
    c.iter().find_map(|p| std::fs::read(p).ok()).and_then(|b| FontVec::try_from_vec(b).ok())
}

macro_rules! need {
    ($($tool:ident),+) => {
        $( let Some($tool) = $tool() else {
            eprintln!("SKIP {}: {} not available", module_path!(), stringify!($tool));
            return;
        }; )+
    };
}

const WORDS: &[&str] = &[
    "invoice",
    "number",
    "customer",
    "account",
    "payment",
    "received",
    "thank",
    "you",
    "for",
    "your",
    "order",
    "please",
    "transfer",
    "the",
    "amount",
    "within",
    "days",
    "delivery",
    "address",
    "street",
    "city",
    "insurance",
    "contract",
    "period",
    "monthly",
    "premium",
    "document",
    "reference",
    "date",
    "total",
    "balance",
    "service",
    "office",
    "letter",
    "notice",
    "tax",
    "return",
    "statement",
    "bank",
    "credit",
    "note",
    "product",
    "quantity",
    "price",
    "discount",
    "shipping",
    "company",
    "limited",
    "regards",
    "manager",
    "phone",
    "email",
    "website",
    "department",
    "information",
    "request",
    "confirm",
    "receipt",
    "appointment",
    "schedule",
    "renewal",
];

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize
    }
}

/// A4 page (300 dpi) with running text; result: image + rendered words.
fn render_page(font: &FontVec) -> (Raster, Vec<String>) {
    let (w, h) = (2480usize, 3508usize);
    let mut data = vec![255u8; w * h];
    let scale = PxScale::from(52.0); // ≈ 11 pt
    let sf = font.as_scaled(scale);
    let space = sf.h_advance(font.glyph_id(' '));
    let mut rng = Lcg(2026);
    let mut words = Vec::new();
    let (left, right) = (250.0f32, (w - 250) as f32);
    let mut baseline = 330.0f32;
    while baseline < (h - 300) as f32 {
        let mut x = left;
        loop {
            let word = WORDS[rng.next() % WORDS.len()];
            let width: f32 = word.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum();
            if x + width > right {
                break;
            }
            for c in word.chars() {
                let id = font.glyph_id(c);
                let g = id.with_scale_and_position(scale, point(x, baseline));
                if let Some(og) = font.outline_glyph(g) {
                    let b = og.px_bounds();
                    og.draw(|gx, gy, cov| {
                        let px = b.min.x as i64 + i64::from(gx);
                        let py = b.min.y as i64 + i64::from(gy);
                        if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                            let i = py as usize * w + px as usize;
                            let v = (255.0 * (1.0 - cov.clamp(0.0, 1.0))) as u8;
                            data[i] = data[i].min(v);
                        }
                    });
                }
                x += sf.h_advance(id);
            }
            words.push(word.to_string());
            x += space;
        }
        baseline += 78.0;
    }
    (Raster { width: w as u32, height: h as u32, channels: 1, data, dpi: 300.0 }, words)
}

/// Skew and soil (specks, scanner edge left, punch hole).
fn make_dirty(page: &Raster, angle: f64) -> Raster {
    let mut r = preprocess::rotate(page, angle);
    let (w, h) = (r.width as usize, r.height as usize);
    for y in 0..h {
        for x in 0..40 {
            r.data[y * w + x] = 50;
        }
    }
    let (hx, hy, rad) = (140i64, (h / 2) as i64, 36i64);
    for y in hy - rad..=hy + rad {
        for x in hx - rad..=hx + rad {
            if (x - hx).pow(2) + (y - hy).pow(2) <= rad * rad {
                r.data[y as usize * w + x as usize] = 15;
            }
        }
    }
    let mut rng = Lcg(99);
    for _ in 0..4000 {
        let (x, y) = (60 + rng.next() % (w - 120), 20 + rng.next() % (h - 40));
        r.data[y * w + x] = 0;
        if rng.next().is_multiple_of(2) {
            r.data[y * w + x + 1] = 0;
        }
    }
    r
}

fn write_png(r: &Raster, path: &Path) {
    let img = image::GrayImage::from_raw(r.width, r.height, r.data.clone()).expect("raster");
    img.save(path).expect("png");
}

/// Share of the rendered words that occur in the recognized text (multiset).
fn word_recall(expected: &[String], text: &str) -> f64 {
    let mut got: HashMap<String, usize> = HashMap::new();
    for t in text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()) {
        *got.entry(t.to_lowercase()).or_default() += 1;
    }
    let mut hit = 0;
    for w in expected {
        if let Some(n) = got.get_mut(w)
            && *n > 0
        {
            *n -= 1;
            hit += 1;
        }
    }
    hit as f64 / expected.len().max(1) as f64
}

fn backend(t: TesseractCli, r: PdfiumRenderer, work: &Path) -> NativeOcrmypdf {
    NativeOcrmypdf::new(Some(Box::new(t) as Box<dyn OcrEngine>), Box::new(r), work.join("pages"))
}

fn cfg(deskew: bool, clean: Clean) -> OcrConfig {
    let mut c = OcrConfig::defaults();
    c.threads_per_worker = 1;
    c.image_dpi = Some(300);
    c.deskew = deskew;
    c.clean = clean;
    c
}

/// Images (XObjects) of the first page: (width, height, filter, decoded data for Flate).
fn first_page_images(pdf: &Path) -> Vec<(i64, i64, String, Option<Vec<u8>>)> {
    let doc = lopdf::Document::load(pdf).expect("pdf");
    let pid = *doc.get_pages().values().next().expect("page");
    let mut out = Vec::new();
    let Ok((Some(res), _)) = doc.get_page_resources(pid) else { return out };
    let Ok(xo) = res.get(b"XObject").and_then(|o| match o {
        lopdf::Object::Reference(r) => doc.get_dictionary(*r),
        o => o.as_dict(),
    }) else {
        return out;
    };
    for (_, v) in xo.iter() {
        let Ok(r) = v.as_reference() else { continue };
        let Ok(s) = doc.get_object(r).and_then(lopdf::Object::as_stream) else { continue };
        if s.dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok() != Some(b"Image".as_slice()) {
            continue;
        }
        let num = |k: &[u8]| s.dict.get(k).and_then(lopdf::Object::as_i64).unwrap_or(0);
        let filter =
            s.dict.get(b"Filter").and_then(lopdf::Object::as_name).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
        let data = if filter == "FlateDecode" { s.decompressed_content().ok() } else { None };
        out.push((num(b"Width"), num(b"Height"), filter, data));
    }
    out
}

#[test]
fn skewed_dirty_scan_word_recall_and_archive() {
    need!(pdfium, tesseract, font);
    let tmp = tempfile::tempdir().unwrap();
    let (page, words) = render_page(&font);
    let dirty = make_dirty(&page, 5.0);
    let input = tmp.path().join("skewed.png");
    write_png(&dirty, &input);
    let b = backend(tesseract, pdfium, tmp.path());

    let mut results = Vec::new();
    for (name, deskew, clean) in [
        ("none", false, Clean::None),
        ("clean", false, Clean::Clean),
        ("deskew", true, Clean::None),
        ("deskew+clean", true, Clean::Clean),
        ("deskew+clean-final", true, Clean::Final),
    ] {
        let d = tmp.path().join(name.replace('+', "_"));
        let t = Instant::now();
        let r =
            parse_document(&b, &cfg(deskew, clean), &d, &input, "image/png", true).unwrap_or_else(|e| panic!("{name}: {}", e.message()));
        let secs = t.elapsed().as_secs_f64();
        let recall = word_recall(&words, &r.text);
        let imgs = first_page_images(r.archive_path.as_ref().unwrap());
        eprintln!("{name:>20}: Wortquote {:.1} % ({} Wörter), {secs:.1} s", recall * 100.0, words.len());
        if recall < 0.5 {
            let snippet: String = r.text.chars().take(160).collect();
            eprintln!("{:>20}  Text: {snippet:?}", "");
        }
        results.push((name, recall, imgs));
    }
    let get = |n: &str| results.iter().find(|r| r.0 == n).unwrap();
    // Preprocessing helps (or at least does not hurt) and reaches a high rate. Note:
    // Tesseract 5 returns no text at all on this dense page from ≈3.5° skew.
    assert!(get("deskew+clean").1 >= get("none").1, "deskew+clean must not be worse");
    assert!(get("deskew+clean").1 >= 0.9, "recall {}", get("deskew+clean").1);
    // Archive: without deskew/clean-final the original, otherwise the processed image.
    let orig = &get("none").2;
    assert_eq!(orig.len(), 1);
    assert_eq!(orig[0].3.as_deref(), Some(dirty.data.as_slice()), "none: archive shows the original");
    assert_eq!(get("clean").2[0].3.as_deref(), Some(dirty.data.as_slice()), "clean: archive shows the original");
    let desk = get("deskew").2[0].3.clone().unwrap();
    assert_ne!(desk, dirty.data, "deskew: archive shows the straightened page");
    let fin = get("deskew+clean-final").2[0].3.clone().unwrap();
    assert_ne!(fin, desk, "clean-final: archive shows the cleaned page");
    // scanner edge on the left is gone in the clean-final archive
    let w = dirty.width as usize;
    let y = dirty.height as usize / 3;
    assert!(fin[y * w..y * w + 30].iter().all(|v| *v > 200), "border removed in clean-final archive");
}

#[test]
fn word_recall_by_angle() {
    // Word rate without / with preprocessing (deskew + clean) at various angles.
    need!(pdfium, tesseract, font);
    let tmp = tempfile::tempdir().unwrap();
    let (page, words) = render_page(&font);
    let b = backend(tesseract, pdfium, tmp.path());
    for angle in [1.0, 2.0, 3.5] {
        let input = tmp.path().join(format!("p{angle}.png"));
        write_png(&make_dirty(&page, angle), &input);
        let mut q = Vec::new();
        for (name, deskew, clean) in [("none", false, Clean::None), ("deskew+clean", true, Clean::Clean)] {
            let d = tmp.path().join(format!("{name}-{angle}").replace('+', "_"));
            let r = parse_document(&b, &cfg(deskew, clean), &d, &input, "image/png", true).unwrap();
            q.push(word_recall(&words, &r.text));
        }
        eprintln!("{angle:>4}°: Wortquote ohne {:.1} %, mit deskew+clean {:.1} %", q[0] * 100.0, q[1] * 100.0);
        assert!(q[1] >= q[0] && q[1] >= 0.9, "{angle}: {q:?}");
    }
}

#[test]
fn skewed_pdf_page_is_replaced_only_when_deskewed() {
    need!(pdfium, tesseract, font);
    let tmp = tempfile::tempdir().unwrap();
    let (page, words) = render_page(&font);
    let b = backend(tesseract, pdfium, tmp.path());
    // PDF from the skewed image (img2pdf path without OCR)
    let png = tmp.path().join("in.png");
    write_png(&make_dirty(&page, -3.0), &png);
    let pdf = b.image_to_pdfa(&png, Some(300), &tmp.path().join("conv")).unwrap();
    let straight_png = tmp.path().join("straight.png");
    write_png(&page, &straight_png);
    let straight_pdf = b.image_to_pdfa(&straight_png, Some(300), &tmp.path().join("conv2")).unwrap();
    let pdf_in = tmp.path().join("skewed.pdf");
    std::fs::copy(&pdf, &pdf_in).unwrap();
    let straight_in = tmp.path().join("straight.pdf");
    std::fs::copy(&straight_pdf, &straight_in).unwrap();

    let names = |p: &Path| -> String {
        let doc = lopdf::Document::load(p).unwrap();
        let pid = *doc.get_pages().values().next().unwrap();
        String::from_utf8_lossy(&doc.get_page_content(pid)).into_owned()
    };
    // deskew off: page stays (text layer on top)
    let r = parse_document(&b, &cfg(false, Clean::Clean), &tmp.path().join("a"), &pdf_in, "application/pdf", true).unwrap();
    let a = r.archive_path.unwrap();
    assert!(!names(&a).contains("/PlOcrPage"), "page kept");
    let recall_off = word_recall(&words, &r.text);
    // deskew on: page is replaced by the straightened image
    let r = parse_document(&b, &cfg(true, Clean::Clean), &tmp.path().join("b"), &pdf_in, "application/pdf", true).unwrap();
    let a = r.archive_path.unwrap();
    assert!(names(&a).contains("/PlOcrPage"), "page replaced");
    let imgs = first_page_images(&a);
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[0].2, "FlateDecode", "PNG source → lossless");
    let recall_on = word_recall(&words, &r.text);
    eprintln!("PDF −3°: Wortquote ohne deskew {:.1} %, mit deskew {:.1} %", recall_off * 100.0, recall_on * 100.0);
    assert!(recall_on >= 0.9, "{recall_on}");
    // straight page + deskew: nothing to rotate → original stays
    let r = parse_document(&b, &cfg(true, Clean::None), &tmp.path().join("c"), &straight_in, "application/pdf", true).unwrap();
    assert!(!names(&r.archive_path.unwrap()).contains("/PlOcrPage"), "straight page kept");
}
