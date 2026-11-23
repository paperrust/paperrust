//! Timing harness for the shared page pool (`pl_ocr::permits`): one scan alone vs. a batch of
//! documents, with fixed per-document threads (previous behaviour) and with adaptive sharing.
//!
//! ```text
//! page_pool_bench <scan.pdf> <cores> [docs] [runs]
//! ```
//!
//! `cores` mimics a smaller machine (e.g. 3 for a Raspberry Pi 5 measurement): it is the pool size
//! and the base of the per-document threads. Scenarios (each `parse_document` end to end, like the
//! consumer; batch = `docs` copies submitted at once and taken by `slots` task workers):
//!
//! - `serial-fixed`:   1 slot,  jobs = cores,          no pool (TASK_WORKERS unset, before)
//! - `auto-fixed`:     auto slots, jobs = cores/slots,  no pool (TASK_WORKERS=auto, before)
//! - `adaptive-1`:     1 slot,  jobs = cores, pool of `cores` (TASK_WORKERS unset, now)
//! - `adaptive-auto`:  auto slots, jobs = cores, pool of `cores` (TASK_WORKERS=auto, now)
//!
//! Prints one JSON line per measurement and checks that every run yields the same text.
//! Tools like `ingest_bench`: `PAPERRUST_TESSERACT_PATH`/`PAPERRUST_TESSDATA_DIR`,
//! `PAPERRUST_PDFIUM_PATH`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use pl_ocr::engine::{OcrEngine, TesseractCli};
use pl_ocr::native::{NativeOcrmypdf, parse_document};
use pl_ocr::permits::PagePool;
use pl_ocr::render::{PdfRenderer, PdfiumRenderer};

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}

struct Setup {
    tess: TesseractCli,
    pdfium: PdfiumRenderer,
    input: PathBuf,
    tmp: PathBuf,
}

/// One document; returns (seconds, text).
fn one(s: &Setup, jobs: usize, pool: Option<Arc<PagePool>>, tag: &str) -> (f64, String) {
    let dir = s.tmp.join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| die(&e.to_string()));
    let mut cfg = pl_ocr::OcrConfig::defaults();
    cfg.threads_per_worker = jobs as i64;
    let mut backend = NativeOcrmypdf::new(
        Some(Box::new(s.tess.clone()) as Box<dyn OcrEngine>),
        Box::new(s.pdfium.clone()) as Box<dyn PdfRenderer>,
        dir.join("pages"),
    );
    backend.page_pool = pool;
    let t = Instant::now();
    let out = parse_document(&backend, &cfg, &dir, &s.input, "application/pdf", true).unwrap_or_else(|e| die(&e.to_string()));
    let secs = t.elapsed().as_secs_f64();
    let _ = std::fs::remove_dir_all(&dir);
    (secs, out.text)
}

/// `docs` documents taken by `slots` workers from a shared queue; returns (wall seconds, texts).
fn batch(s: &Setup, slots: usize, docs: usize, jobs: usize, pool: Option<Arc<PagePool>>, tag: &str) -> (f64, Vec<String>) {
    let next = AtomicUsize::new(0);
    let texts = Mutex::new(Vec::new());
    let t = Instant::now();
    std::thread::scope(|sc| {
        for w in 0..slots {
            let (next, texts, pool) = (&next, &texts, pool.clone());
            sc.spawn(move || {
                loop {
                    let k = next.fetch_add(1, Ordering::SeqCst);
                    if k >= docs {
                        break;
                    }
                    let (_, text) = one(s, jobs, pool.clone(), &format!("{tag}-w{w}-d{k}"));
                    if let Ok(mut v) = texts.lock() {
                        v.push(text);
                    }
                }
            });
        }
    });
    (t.elapsed().as_secs_f64(), texts.into_inner().unwrap_or_default())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        die("usage: page_pool_bench <scan.pdf> <cores> [docs] [runs]");
    }
    let cores: usize = a[2].parse().unwrap_or_else(|_| die("cores"));
    let docs: usize = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(3);
    let runs: usize = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(1);
    let tess = TesseractCli::from_env();
    if !tess.is_available() {
        die("tesseract not available (PAPERRUST_TESSERACT_PATH)");
    }
    let pdfium = PdfiumRenderer::from_env().unwrap_or_else(|e| die(&format!("pdfium: {e}")));
    let s = Setup {
        tess,
        pdfium,
        input: PathBuf::from(&a[1]),
        tmp: std::env::temp_dir().join(format!("pl-page-pool-bench-{}", std::process::id())),
    };
    let auto_slots = pl_core::resources::default_task_workers(None, cores as i64, None).max(1) as usize;
    let auto_jobs = (cores / auto_slots).max(1);
    // (name, slots, jobs, pool)
    let modes: Vec<(&str, usize, usize, bool)> = vec![
        ("serial-fixed", 1, cores, false),
        ("auto-fixed", auto_slots, auto_jobs, false),
        ("adaptive-1", 1, cores, true),
        ("adaptive-auto", auto_slots, cores, true),
    ];
    // warm-up (library instances, file cache)
    let (_, reference) = one(&s, cores, None, "warmup");
    let check = |what: &str, t: &str| {
        if t != reference {
            die(&format!("{what}: text differs from the reference run"));
        }
    };
    for run in 0..runs {
        for (name, slots, jobs, pooled) in &modes {
            let pool = pooled.then(|| Arc::new(PagePool::new(cores)));
            let (solo, text) = one(&s, *jobs, pool.clone(), &format!("{name}-solo"));
            check(name, &text);
            let pool2 = pooled.then(|| Arc::new(PagePool::new(cores)));
            let (wall, texts) = batch(&s, *slots, docs, *jobs, pool2.clone(), name);
            for t in &texts {
                check(name, t);
            }
            let peak = pool2.map_or(0, |p| p.peak());
            println!(
                "{{\"run\":{run},\"mode\":\"{name}\",\"cores\":{cores},\"slots\":{slots},\"jobs\":{jobs},\"pool\":{pooled},\"solo_secs\":{solo:.2},\"batch_docs\":{docs},\"batch_secs\":{wall:.2},\"batch_pool_peak\":{peak}}}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&s.tmp);
}
