//! Throughput of the Tesseract steps only (OSD + recognition) on pre-rasterized pages, without PDF
//! reading/writing: program (TIFF file) against libtesseract, with N threads.
//!
//! ```text
//! tess_throughput <in.pdf> <threads> <rounds> [cli|lib]...
//! ```
//! Tools like ingest_bench (`PAPERRUST_TESSERACT_PATH`, `PAPERRUST_TESSDATA_DIR`, `PAPERRUST_PDFIUM_PATH`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use pl_ocr::engine::{OcrRequest, TesseractCli};
use pl_ocr::render::{PdfRenderer, PdfiumRenderer, Raster};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: tess_throughput <in.pdf> <threads> <rounds> [cli|lib]...");
        std::process::exit(2);
    }
    let threads: usize = a[2].parse().expect("threads");
    let rounds: usize = a[3].parse().expect("rounds");
    let modes: Vec<String> = if a.len() > 4 { a[4..].to_vec() } else { vec!["cli".into(), "lib".into()] };
    let pdfium = PdfiumRenderer::from_env().expect("pdfium");
    let data = std::fs::read(&a[1]).expect("pdf");
    let doc = pdfium.open(&data).expect("open");
    let pages: Vec<Raster> = (0..doc.page_count()).map(|i| doc.render(i, 300.0, true).expect("render")).collect();
    drop(doc);
    let tess = TesseractCli::from_env();
    let dir = std::env::temp_dir().join(format!("pl-tess-throughput-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for mode in &modes {
        let lib = if mode == "lib" { Some(tess.library().expect("libtesseract not found")) } else { None };
        let td = tess.library_tessdata();
        for round in 0..rounds {
            let next = AtomicUsize::new(0);
            let t = Instant::now();
            std::thread::scope(|s| {
                for _ in 0..threads {
                    s.spawn(|| {
                        loop {
                            let i = next.fetch_add(1, Ordering::SeqCst);
                            if i >= pages.len() {
                                break;
                            }
                            let r = &pages[i];
                            let req = OcrRequest { languages: "eng".into(), dpi: Some(300), timeout: None, ..Default::default() };
                            match &lib {
                                Some(l) => {
                                    l.orientation(td.as_deref(), r, None).unwrap();
                                    l.recognize(td.as_deref(), None, r, &req).unwrap();
                                }
                                None => {
                                    let f = dir.join(format!("p{i}.tif"));
                                    tess.orientation_raster_cli(r, &f, None).unwrap();
                                    tess.recognize_raster_cli(r, &f, &req).unwrap();
                                    let _ = std::fs::remove_file(&f);
                                }
                            }
                        }
                    });
                }
            });
            let secs = t.elapsed().as_secs_f64();
            println!("{{\"mode\":\"{mode}\",\"threads\":{threads},\"round\":{round},\"pages\":{},\"secs\":{secs:.3}}}", pages.len());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
