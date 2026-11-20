//! OCR quality/timing measurement Tesseract vs. Windows OCR (`tests/bench/ocr-quality/`).
//!
//! ```text
//! ocr_quality images <corpus-dir> <out-dir> <runs> <languages> [tesseract|windows]...
//! ocr_quality pages  <in.pdf> <out-dir> <dpi> <languages> [tesseract|windows]...
//! ocr_quality ingest <in.pdf> <out-dir> <languages> <jobs> <tesseract|windows>
//! ```
//! - `images`: recognize every image from `<corpus>/manifest.json` (`images`); `<out>/<engine>/<name>.txt`
//!   (page text after `post_process_text`, like the stored content), `timings.jsonl` (median/min
//!   over `runs` warm runs + first run), `throughput.jsonl` (all images with 1 and 4 threads).
//! - `pages`: rasterize PDF pages with PDFium (grayscale, `dpi`), recognize per engine and page;
//!   `<out>/<engine>/page-NN.txt`, `<out>/img/page-NN.jpg` (preview, 1100 px wide),
//!   `<out>/pages.jsonl` (time, characters, words, mean word confidence if available).
//! - `ingest`: real ingestion path `native::parse_document` (OCRmyPDF replacement, default OcrConfig with
//!   `languages`, `rotate_pages` as default, `jobs` pages in parallel); `<out>/ingest-<engine>.txt`
//!   (document text), `<out>/ingest-<engine>-layer.txt` (text of the invisible text layer in the
//!   archive PDF), JSON line on stdout.
//!
//! Tesseract as in ingestion (`TesseractCli::from_env`, libtesseract if present); Windows OCR
//! with `ingest` using Tesseract OSD for page rotation (like `PAPERRUST_OCR_ENGINE=windows`).
//! PDFium: `PAPERRUST_PDFIUM_PATH`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use pl_ocr::engine::{OcrEngine, OcrRequest, PageOcr, TesseractCli};
use pl_ocr::render::{PdfRenderer, PdfiumRenderer, Raster};

fn tesseract() -> TesseractCli {
    let t = TesseractCli::from_env();
    assert!(t.is_available(), "tesseract not available (PAPERRUST_TESSERACT_PATH)");
    t
}

fn engine(kind: &str, with_osd: bool) -> Box<dyn OcrEngine> {
    match kind {
        "tesseract" => Box::new(tesseract()),
        #[cfg(windows)]
        "windows" => Box::new(
            pl_ocr::winocr::WindowsOcr::new(with_osd.then(tesseract)).expect("Windows OCR not available"),
        ),
        other => {
            let _ = with_osd;
            panic!("unknown/unsupported engine {other}")
        }
    }
}

fn req(langs: &str, r: &Raster) -> OcrRequest {
    OcrRequest { languages: langs.to_string(), dpi: (r.dpi > 0.0).then(|| r.dpi.round() as u32), timeout: None, ..Default::default() }
}

fn clean_text(p: &PageOcr) -> String {
    pl_ocr::text::post_process_text(Some(&p.text)).unwrap_or_default()
}

fn mean_conf(p: &PageOcr) -> Option<f64> {
    let c: Vec<f64> = p.words.iter().map(|w| f64::from(w.conf)).filter(|c| *c >= 0.0).collect();
    (!c.is_empty()).then(|| c.iter().sum::<f64>() / c.len() as f64)
}

fn main() {
    // Everything in one worker thread as in the server (libtesseract limits OpenMP per worker thread).
    std::thread::Builder::new().stack_size(16 << 20).spawn(run).unwrap().join().unwrap();
}

fn run() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("images") if a.len() >= 6 => images(&a[2..]),
        Some("pages") if a.len() >= 6 => pages(&a[2..]),
        Some("ingest") if a.len() >= 7 => ingest(&a[2..]),
        _ => {
            eprintln!(
                "usage:\n  ocr_quality images <corpus> <out> <runs> <langs> [engines]\n  ocr_quality pages <in.pdf> <out> <dpi> <langs> [engines]\n  ocr_quality ingest <in.pdf> <out> <langs> <jobs> <engine>"
            );
            std::process::exit(2);
        }
    }
}

fn engines_arg(rest: &[String]) -> Vec<String> {
    if rest.is_empty() { vec!["tesseract".into(), "windows".into()] } else { rest.to_vec() }
}

fn images(a: &[String]) {
    let dir = PathBuf::from(&a[0]);
    let out = PathBuf::from(&a[1]);
    let runs: usize = a[2].parse().expect("runs");
    let langs = a[3].clone();
    let kinds = engines_arg(&a[4..]);
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).expect("manifest")).expect("json");
    let pages: Vec<(String, Raster)> = manifest["images"]
        .as_array()
        .expect("images")
        .iter()
        .map(|m| {
            let img = image::open(dir.join(m["image"].as_str().unwrap())).expect("image");
            (m["name"].as_str().unwrap().to_string(), Raster::from_dynamic(&img, m["dpi"].as_f64().unwrap_or(0.0)))
        })
        .collect();
    std::fs::create_dir_all(&out).unwrap();
    let tmp = out.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let mut timings = String::new();
    let mut thr = String::new();
    for kind in &kinds {
        let e = engine(kind, false);
        eprintln!("engine {kind}: {}", e.name());
        e.check_languages(&langs).expect("languages");
        let edir = out.join(kind);
        std::fs::create_dir_all(&edir).unwrap();
        // Measure in a worker thread as in the page pipeline (libtesseract limits OpenMP per
        // worker thread; on the main thread the times would not be comparable).
        let per_image: Vec<(String, PageOcr, Vec<f64>)> = std::thread::scope(|s| {
            s.spawn(|| {
                pages
                    .iter()
                    .map(|(name, r)| {
                        let file = tmp.join(format!("{kind}-{name}.{}", e.raster_file_ext()));
                        let mut ms = Vec::new();
                        let mut page = PageOcr::default();
                        for _ in 0..=runs {
                            let _ = std::fs::remove_file(&file);
                            let t = Instant::now();
                            page = e.recognize_raster(r, &file, &req(&langs, r)).expect("recognize");
                            ms.push(t.elapsed().as_secs_f64() * 1000.0);
                        }
                        let _ = std::fs::remove_file(&file);
                        eprintln!("{kind} {name}: {:?} ms", ms);
                        (name.clone(), page, ms)
                    })
                    .collect()
            })
            .join()
            .expect("worker")
        });
        for ((name, page, ms), (_, r)) in per_image.into_iter().zip(&pages) {
            let name = &name;
            std::fs::write(edir.join(format!("{name}.txt")), clean_text(&page)).unwrap();
            let first = ms[0];
            let mut warm = ms[1..].to_vec();
            warm.sort_by(f64::total_cmp);
            let med = warm.get(warm.len() / 2).copied().unwrap_or(first);
            let min = warm.first().copied().unwrap_or(first);
            let line = serde_json::json!({"engine": kind, "name": name, "px": [r.width, r.height], "first_ms": first, "median_ms": med, "min_ms": min, "runs": runs, "mean_conf": mean_conf(&page)});
            eprintln!("{line}");
            timings.push_str(&format!("{line}\n"));
        }
        // Throughput with several threads (like the page pipeline): all images, 1 and 4 workers.
        for threads in [1usize, 4] {
            let next = AtomicUsize::new(0);
            let t = Instant::now();
            std::thread::scope(|s| {
                for w in 0..threads {
                    let (next, pages, e, tmp, langs) = (&next, &pages, &e, &tmp, &langs);
                    s.spawn(move || {
                        loop {
                            let i = next.fetch_add(1, Ordering::SeqCst);
                            let Some((name, r)) = pages.get(i) else { break };
                            let file = tmp.join(format!("thr-{kind}-{w}-{name}.{}", e.raster_file_ext()));
                            e.recognize_raster(r, &file, &req(langs, r)).expect("recognize");
                            let _ = std::fs::remove_file(&file);
                        }
                    });
                }
            });
            let line = serde_json::json!({"engine": kind, "threads": threads, "pages": pages.len(), "wall_ms": t.elapsed().as_secs_f64() * 1000.0});
            eprintln!("{line}");
            thr.push_str(&format!("{line}\n"));
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::write(out.join("timings.jsonl"), timings).unwrap();
    std::fs::write(out.join("throughput.jsonl"), thr).unwrap();
}

fn pdfium() -> PdfiumRenderer {
    PdfiumRenderer::from_env().expect("PDFium not available (PAPERRUST_PDFIUM_PATH)")
}

fn pages(a: &[String]) {
    let pdf = PathBuf::from(&a[0]);
    let out = PathBuf::from(&a[1]);
    let dpi: f64 = a[2].parse().expect("dpi");
    let langs = a[3].clone();
    let kinds = engines_arg(&a[4..]);
    let pdfium = pdfium();
    let data = std::fs::read(&pdf).expect("pdf");
    let doc = pdfium.open(&data).expect("open pdf");
    let rasters: Vec<Raster> = (0..doc.page_count()).map(|i| doc.render(i, dpi, true).expect("render")).collect();
    drop(doc);
    let img_dir = out.join("img");
    std::fs::create_dir_all(&img_dir).unwrap();
    for (i, r) in rasters.iter().enumerate() {
        if let Some(img) = r.to_dynamic() {
            let w = 1100u32.min(r.width);
            let h = (u64::from(r.height) * u64::from(w) / u64::from(r.width.max(1))) as u32;
            img.resize(w, h.max(1), image::imageops::FilterType::Triangle)
                .to_luma8()
                .save(img_dir.join(format!("page-{:02}.jpg", i + 1)))
                .unwrap();
        }
    }
    let tmp = out.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let mut lines = String::new();
    for kind in &kinds {
        let e = engine(kind, false);
        e.check_languages(&langs).expect("languages");
        let edir = out.join(kind);
        std::fs::create_dir_all(&edir).unwrap();
        for (i, r) in rasters.iter().enumerate() {
            let file = tmp.join(format!("{kind}-{i}.{}", e.raster_file_ext()));
            let _ = std::fs::remove_file(&file);
            let t = Instant::now();
            let p = e.recognize_raster(r, &file, &req(&langs, r)).expect("recognize");
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let _ = std::fs::remove_file(&file);
            let text = clean_text(&p);
            std::fs::write(edir.join(format!("page-{:02}.txt", i + 1)), &text).unwrap();
            let line = serde_json::json!({"engine": kind, "page": i + 1, "px": [r.width, r.height], "ms": ms, "chars": text.chars().filter(|c| !c.is_whitespace()).count(), "words": p.words.len(), "mean_conf": mean_conf(&p)});
            eprintln!("{line}");
            lines.push_str(&format!("{line}\n"));
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::write(out.join("pages.jsonl"), lines).unwrap();
}

fn ingest(a: &[String]) {
    use pl_ocr::native::{NativeOcrmypdf, parse_document};
    let pdf = PathBuf::from(&a[0]);
    let out = PathBuf::from(&a[1]);
    let langs = a[2].clone();
    let jobs: i64 = a[3].parse().expect("jobs");
    let kind = a[4].as_str();
    std::fs::create_dir_all(&out).unwrap();
    let tmp = tempdir_in(&out, kind);
    let backend = NativeOcrmypdf::new(Some(engine(kind, true)), Box::new(pdfium()), tmp.join("pages"));
    let mut cfg = pl_ocr::OcrConfig::defaults();
    cfg.language = langs;
    cfg.threads_per_worker = jobs;
    let t = Instant::now();
    let r = parse_document(&backend, &cfg, &tmp, &pdf, "application/pdf", true).expect("parse_document");
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let layer = r
        .archive_path
        .as_deref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|d| pl_ocr::text::post_process_text(pl_ocr::pdfwrite::extract_text(&d).as_deref()))
        .unwrap_or_default();
    let pages = r.archive_path.as_deref().and_then(|p| std::fs::read(p).ok()).and_then(|d| pl_ocr::pdfwrite::page_count(&d));
    std::fs::write(out.join(format!("ingest-{kind}.txt")), &r.text).unwrap();
    std::fs::write(out.join(format!("ingest-{kind}-layer.txt")), &layer).unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    println!(
        "{}",
        serde_json::json!({"engine": kind, "pdf": pdf.file_name().map(|f| f.to_string_lossy().into_owned()), "pages": pages, "jobs": jobs, "ms": ms, "chars": r.text.chars().filter(|c| !c.is_whitespace()).count(), "layer_chars": layer.chars().filter(|c| !c.is_whitespace()).count()})
    );
}

fn tempdir_in(out: &Path, kind: &str) -> PathBuf {
    let d = out.join(format!("tmp-ingest-{kind}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
