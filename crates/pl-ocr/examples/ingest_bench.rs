//! Measurement of OCR ingestion (`parse_document`, as the consumer calls it for PDFs/images).
//!
//! ```text
//! ingest_bench make <out.pdf> <pages> <dpi> <src.pdf>...   # create a scan-like image PDF
//! ingest_bench run <in.pdf> <jobs> <runs>                  # JSON line per run
//! ```
//!
//! Tools as in the consumer: `PAPERRUST_TESSERACT_PATH`/`PAPERRUST_TESSDATA_DIR`,
//! `PAPERRUST_PDFIUM_PATH`. See `tests/bench/ingest_bench.sh`.

use std::path::PathBuf;
use std::time::Instant;

use pl_ocr::engine::{OcrEngine, TesseractCli};
use pl_ocr::native::{NativeOcrmypdf, parse_document};
use pl_ocr::pdfwrite::{self, ImagePageSpec, OutputMeta, PageImage};
use pl_ocr::render::{PdfRenderer, PdfiumRenderer};

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}

fn sha(data: &[u8]) -> String {
    let d = <sha2::Sha256 as sha2::Digest>::digest(data);
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Immediately returns one word per page.
struct NullEngine;

impl OcrEngine for NullEngine {
    fn name(&self) -> String {
        "null".into()
    }
    fn recognize(&self, _: &std::path::Path, _: &pl_ocr::engine::OcrRequest) -> pl_ocr::Result<pl_ocr::engine::PageOcr> {
        let w = pl_ocr::engine::Word { text: "x".into(), x0: 10.0, y0: 10.0, x1: 20.0, y1: 20.0, conf: 90.0, line: (1, 1, 1) };
        Ok(pl_ocr::engine::PageOcr { width: 2480, height: 3508, words: vec![w], text: "x
".into() })
    }
    fn orientation(&self, _: &std::path::Path, _: Option<std::time::Duration>) -> pl_ocr::Result<Option<pl_ocr::engine::Orientation>> {
        Ok(None)
    }
    fn languages(&self) -> pl_ocr::Result<Vec<String>> {
        Ok(vec!["eng".into()])
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pdfium = PdfiumRenderer::from_env().unwrap_or_else(|e| die(&format!("pdfium: {e}")));
    match a.get(1).map(String::as_str) {
        Some("make") if a.len() >= 6 => {
            let out = PathBuf::from(&a[2]);
            let pages: usize = a[3].parse().unwrap_or_else(|_| die("pages"));
            let dpi: f64 = a[4].parse().unwrap_or_else(|_| die("dpi"));
            let srcs: Vec<Vec<u8>> = a[5..].iter().map(|p| std::fs::read(p).unwrap_or_else(|e| die(&format!("{p}: {e}")))).collect();
            let docs: Vec<_> = srcs.iter().map(|d| pdfium.open(d).unwrap_or_else(|e| die(&e.message()))).collect();
            let all: Vec<(usize, usize)> = docs.iter().enumerate().flat_map(|(k, d)| (0..d.page_count()).map(move |i| (k, i))).collect();
            let specs: Vec<ImagePageSpec> = (0..pages)
                .map(|n| {
                    let (k, i) = all[n % all.len()];
                    let r = docs[k].render(i, dpi, true).unwrap_or_else(|e| die(&e.message()));
                    ImagePageSpec { image: PageImage::Raw(r), dpi_x: dpi, dpi_y: dpi, rotate: 0, ocr: None }
                })
                .collect();
            drop(docs);
            let pdf = pdfwrite::build_from_images(&specs, &OutputMeta { output_type: pl_ocr::config::OutputType::Pdf, ..Default::default() })
                .unwrap_or_else(|e| die(&e.message()));
            std::fs::write(&out, &pdf).unwrap_or_else(|e| die(&e.to_string()));
            println!("{{\"pages\":{pages},\"dpi\":{dpi},\"bytes\":{},\"sources\":{}}}", pdf.len(), all.len());
        }
        Some("run") if a.len() == 5 => {
            let input = PathBuf::from(&a[2]);
            let jobs: i64 = a[3].parse().unwrap_or_else(|_| die("jobs"));
            let runs: usize = a[4].parse().unwrap_or_else(|_| die("runs"));
            // `INGEST_BENCH_NULL_ENGINE=1`: engine without work (measures everything except Tesseract).
            let null = std::env::var_os("INGEST_BENCH_NULL_ENGINE").is_some();
            let tess = TesseractCli::from_env();
            if !null && !tess.is_available() {
                die("tesseract not available (PAPERRUST_TESSERACT_PATH)");
            }
            let engine_name = if null { "null".to_string() } else { tess.name() };
            let engine_name = serde_json::to_string(&engine_name).unwrap_or_default();
            let mut cfg = pl_ocr::OcrConfig::defaults();
            cfg.threads_per_worker = jobs;
            for run in 0..runs {
                let tmp = std::env::temp_dir().join(format!("pl-ingest-bench-{}-{run}", std::process::id()));
                let _ = std::fs::remove_dir_all(&tmp);
                let backend = NativeOcrmypdf::new(Some(if null { Box::new(NullEngine) as Box<dyn OcrEngine> } else { Box::new(tess.clone()) }), Box::new(pdfium.clone()) as Box<dyn PdfRenderer>, tmp.join("pages"));
                let t = Instant::now();
                let out = parse_document(&backend, &cfg, &tmp, &input, "application/pdf", true).unwrap_or_else(|e| die(&e.to_string()));
                let secs = t.elapsed().as_secs_f64();
                let archive_data = out.archive_path.as_deref().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
                let archive = archive_data.len();
                println!(
                    "{{\"jobs\":{jobs},\"run\":{run},\"secs\":{secs:.3},\"engine\":{},\"text_chars\":{},\"text_sha\":\"{}\",\"archive_bytes\":{archive},\"archive_sha\":\"{}\",\"archive_content_sha\":\"{}\"}}",
                    engine_name,
                    out.text.chars().count(),
                    sha(out.text.as_bytes()),
                    sha(&archive_data),
                    archive_content_sha(&archive_data)
                );
                let _ = std::fs::remove_dir_all(&tmp);
            }
        }
        _ => die("usage: ingest_bench make <out.pdf> <pages> <dpi> <src.pdf>... | run <in.pdf> <jobs> <runs>"),
    }
}

/// Archive hash without time-dependent parts (info dictionary, XMP metadata, trailer `/ID`): all
/// other objects in numeric order, streams with content.
fn archive_content_sha(data: &[u8]) -> String {
    let Ok(doc) = lopdf::Document::load_mem(data) else { return "unparsable".into() };
    let mut skip = Vec::new();
    if let Ok(r) = doc.trailer.get(b"Info").and_then(lopdf::Object::as_reference) {
        skip.push(r);
    }
    if let Ok(cat) = doc.catalog()
        && let Ok(r) = cat.get(b"Metadata").and_then(lopdf::Object::as_reference)
    {
        skip.push(r);
    }
    let mut h = Vec::new();
    for (id, obj) in &doc.objects {
        if skip.contains(id) {
            continue;
        }
        h.extend_from_slice(format!("{id:?}").as_bytes());
        match obj {
            lopdf::Object::Stream(s) => {
                h.extend_from_slice(format!("{:?}", s.dict).as_bytes());
                h.extend_from_slice(&s.content);
            }
            o => h.extend_from_slice(format!("{o:?}").as_bytes()),
        }
    }
    sha(&h)
}
