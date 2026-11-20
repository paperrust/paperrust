//! Tesseract paths must deliver the same result as the previous call (program + PNM file):
//! - program with TIFF file ([`Raster::write_tiff`]),
//! - `libtesseract` in-process ([`pl_ocr::tesslib`]), also with reused instances from the
//!   pool and in parallel,
//! - `parse_document` end to end (text + archive content).
//!
//! Tools like tests/tools.rs (PDFium from reference/venv, Tesseract from tests/parity/pl-ocr/.tools);
//! if they are missing, the tests are skipped. Benchmark pages: results/bench/ingest/scan-16p.pdf
//! (tests/bench/ingest_bench.sh); if the file is missing, only the upstream samples.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use pl_ocr::engine::{OcrEngine, OcrRequest, Orientation, PageOcr, TesseractCli};
use pl_ocr::native::{NativeOcrmypdf, parse_document};
use pl_ocr::render::{PdfRenderer, PdfiumRenderer, Raster};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pdfium() -> Option<PdfiumRenderer> {
    static R: OnceLock<Option<PdfiumRenderer>> = OnceLock::new();
    R.get_or_init(|| {
        let p = std::env::var_os("PAPERRUST_PDFIUM_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll"));
        PdfiumRenderer::load(&p).ok()
    })
    .clone()
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

macro_rules! need {
    ($($tool:ident),+) => {
        $( let Some($tool) = $tool() else {
            eprintln!("SKIP {}: {} not available", module_path!(), stringify!($tool));
            return;
        }; )+
    };
}

fn sample(name: &str) -> PathBuf {
    let a = repo().join("upstream/src/paperless/tests/samples/tesseract").join(name);
    if a.exists() { a } else { repo().join("upstream/src/paperless_testing/sample_files").join(name) }
}

/// Test rasters: image samples (gray/RGB, various formats), rasterized PDF pages (gray as in the
/// normal pipeline case, RGB as with `Rasterize`), rotated variants (for OSD) and – if
/// present – the pages of the benchmark PDF.
fn rasters(pdfium: &PdfiumRenderer, bench_pages: usize) -> Vec<(String, Raster)> {
    let mut out = Vec::new();
    for name in ["simple.png", "simple.jpg", "simple.tif", "simple.bmp", "simple-no-dpi.png", "simple-alpha.png"] {
        let img = image::open(sample(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        out.push((name.to_string(), Raster::from_dynamic(&img, 300.0)));
    }
    for (name, dpi) in [("rotated.pdf", 300.0), ("multi-page-mixed.pdf", 300.0), ("single-page-mixed.pdf", 200.0), ("rtl-test.pdf", 300.0)] {
        let data = std::fs::read(sample(name)).unwrap();
        let doc = pdfium.open(&data).unwrap();
        for i in 0..doc.page_count() {
            for gray in [true, false] {
                out.push((format!("{name}#{i}{}", if gray { "g" } else { "c" }), doc.render(i, dpi, gray).unwrap()));
            }
        }
    }
    let bench = repo().join("results/bench/ingest/scan-16p.pdf");
    if bench.exists() {
        let data = std::fs::read(&bench).unwrap();
        let doc = pdfium.open(&data).unwrap();
        for i in 0..doc.page_count().min(bench_pages) {
            out.push((format!("scan-16p#{i}"), doc.render(i, 300.0, true).unwrap()));
        }
    } else {
        eprintln!("NOTE: {} missing (tests/bench/ingest_bench.sh), using upstream samples only", bench.display());
    }
    // Rotated pages: OSD must find the same rotation/confidence.
    let rotated: Vec<(String, Raster)> = out
        .iter()
        .filter(|(n, _)| n.starts_with("simple.png") || n.starts_with("multi-page-mixed.pdf#0g") || n.starts_with("scan-16p#0") || n.starts_with("scan-16p#4"))
        .flat_map(|(n, r)| [90u16, 180, 270].map(|d| (format!("{n}@{d}"), r.rotate_cw(d))))
        .collect();
    out.extend(rotated);
    out
}

#[derive(Debug, Clone, PartialEq)]
struct Res {
    osd: Option<Orientation>,
    ocr: PageOcr,
}

fn req(langs: &str, dpi: f64) -> OcrRequest {
    OcrRequest { languages: langs.into(), dpi: Some(dpi.round() as u32), timeout: None, ..Default::default() }
}

/// Previous path: program with PNM file.
fn via_cli(t: &TesseractCli, r: &Raster, dir: &Path, name: &str, ext: &str, langs: &str) -> Res {
    let f = dir.join(format!("{}.{ext}", name.replace(['#', '@', '.'], "_")));
    match ext {
        "pnm" => r.write_pnm(&f).unwrap(),
        _ => r.write_tiff(&f).unwrap(),
    }
    let osd = t.orientation(&f, None).unwrap();
    let ocr = t.recognize(&f, &req(langs, r.dpi)).unwrap();
    std::fs::remove_file(&f).unwrap();
    Res { osd, ocr }
}

fn via_lib(t: &TesseractCli, r: &Raster, langs: &str) -> Res {
    let lib = t.library().expect("library");
    let td = t.library_tessdata();
    let osd = lib.orientation(td.as_deref(), r, None).unwrap();
    let ocr = lib.recognize(td.as_deref(), t.psm, r, &req(langs, r.dpi)).unwrap();
    Res { osd, ocr }
}

/// `f` over all elements, distributed across `n` threads; result in input order.
fn par_map<T: Sync, R: Send>(items: &[T], n: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let chunk = items.len().div_ceil(n.max(1)).max(1);
    std::thread::scope(|s| {
        let hs: Vec<_> = items.chunks(chunk).map(|c| s.spawn(|| c.iter().map(&f).collect::<Vec<R>>())).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

fn assert_same(what: &str, names: &[String], a: &[Res], b: &[Res]) {
    let mut bad = Vec::new();
    for ((n, x), y) in names.iter().zip(a).zip(b) {
        if x != y {
            bad.push(format!("{n}: osd {:?} vs {:?}, words {} vs {}, text eq {}", x.osd, y.osd, x.ocr.words.len(), y.ocr.words.len(), x.ocr.text == y.ocr.text));
        }
    }
    assert!(bad.is_empty(), "{what}: {} of {} differ:\n{}", bad.len(), names.len(), bad.join("\n"));
}

#[test]
fn tiff_file_gives_same_result_as_pnm() {
    need!(pdfium, tesseract);
    let dir = tempfile::tempdir().unwrap();
    let rs = rasters(&pdfium, 16);
    let names: Vec<String> = rs.iter().map(|(n, _)| n.clone()).collect();
    let pnm = par_map(&rs, 6, |(n, r)| via_cli(&tesseract, r, dir.path(), n, "pnm", "eng"));
    let tif = par_map(&rs, 6, |(n, r)| via_cli(&tesseract, r, dir.path(), n, "tif", "eng"));
    assert_same("CLI TIFF vs PNM", &names, &pnm, &tif);
    // The rotated samples must really trigger OSD (otherwise the test does not check the rotation).
    assert!(pnm.iter().zip(&names).any(|(r, n)| n.contains('@') && r.osd.is_some_and(|o| o.rotate != 0)));
}

#[test]
fn library_matches_cli_serial_parallel_and_reused() {
    need!(pdfium, tesseract);
    if tesseract.library().is_none() {
        eprintln!("SKIP: no libtesseract next to {} (or PAPERRUST_TESSERACT_LIB=off)", tesseract.program.display());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let rs = rasters(&pdfium, 16);
    let names: Vec<String> = rs.iter().map(|(n, _)| n.clone()).collect();
    for langs in ["eng", "deu+eng"] {
        let cli = par_map(&rs, 6, |(n, r)| via_cli(&tesseract, r, dir.path(), n, "pnm", langs));
        // serial: one instance per language, reused across all pages
        let serial: Vec<Res> = rs.iter().map(|(_, r)| via_lib(&tesseract, r, langs)).collect();
        assert_same(&format!("lib serial {langs}"), &names, &cli, &serial);
        // parallel, reverse order (different history per instance)
        let mut rev: Vec<(usize, &(String, Raster))> = rs.iter().enumerate().collect();
        rev.reverse();
        let mut par: Vec<(usize, Res)> = par_map(&rev, 5, |(i, (_, r))| (*i, via_lib(&tesseract, r, langs)));
        par.sort_by_key(|(i, _)| *i);
        let par: Vec<Res> = par.into_iter().map(|(_, r)| r).collect();
        assert_same(&format!("lib parallel {langs}"), &names, &cli, &par);
    }
    let lib = tesseract.library().unwrap();
    // Pool: at most as many instances as are used simultaneously (5 threads × {eng, deu+eng, osd}).
    assert!(lib.idle_instances() <= 15, "{}", lib.idle_instances());
}

#[test]
fn library_osd_too_few_characters_and_timeout() {
    need!(tesseract);
    let Some(lib) = tesseract.library() else { return };
    let td = tesseract.library_tessdata();
    let blank = Raster { width: 600, height: 800, channels: 1, data: vec![255; 600 * 800], dpi: 300.0 };
    // CLI: "Too few characters" → error code → None
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("blank.pnm");
    blank.write_pnm(&f).unwrap();
    assert_eq!(tesseract.orientation(&f, None).unwrap(), None);
    assert_eq!(lib.orientation(td.as_deref(), &blank, None).unwrap(), None);
    // empty page: both without words, same text
    let a = tesseract.recognize(&f, &req("eng", 300.0)).unwrap();
    let b = lib.recognize(td.as_deref(), None, &blank, &req("eng", 300.0)).unwrap();
    assert_eq!(a, b);
    // time limit (monitor deadline) → error like with an aborted process
    let page = image::open(sample("simple.png")).unwrap();
    let r = Raster::from_dynamic(&page, 300.0);
    let tiny = OcrRequest { languages: "eng".into(), dpi: Some(300), timeout: Some(Duration::from_nanos(1)), ..Default::default() };
    assert!(lib.recognize(td.as_deref(), None, &r, &tiny).is_err());
    // afterwards the instance keeps working
    assert!(!lib.recognize(td.as_deref(), None, &r, &req("eng", 300.0)).unwrap().words.is_empty());
    // unknown language → error, no crash
    assert!(lib.recognize(td.as_deref(), None, &r, &req("xyz", 300.0)).is_err());
}

#[test]
fn write_tiff_roundtrip() {
    for (ch, w, h) in [(1u8, 7u32, 5u32), (3, 4, 3)] {
        let data: Vec<u8> = (0..(w * h * u32::from(ch))).map(|v| (v * 37 % 251) as u8).collect();
        let r = Raster { width: w, height: h, channels: ch, data, dpi: 300.0 };
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.tif");
        r.write_tiff(&f).unwrap();
        let back = Raster::from_dynamic(&image::open(&f).unwrap(), 300.0);
        assert_eq!(back, r);
        // resolution 0 (like PNM: no value)
        let mut dec = tiff::decoder::Decoder::new(std::fs::File::open(&f).unwrap()).unwrap();
        let x = dec.get_tag(tiff::tags::Tag::XResolution).unwrap();
        assert_eq!(x.clone().into_f64().unwrap_or(0.0), 0.0, "{x:?}");
    }
}

/// Previous behavior as an engine: program + PNM file (default raster methods of the trait).
struct CliPnm(TesseractCli);

impl OcrEngine for CliPnm {
    fn name(&self) -> String {
        "cli-pnm".into()
    }
    fn recognize(&self, image: &Path, req: &OcrRequest) -> pl_ocr::Result<PageOcr> {
        self.0.recognize(image, req)
    }
    fn orientation(&self, image: &Path, timeout: Option<Duration>) -> pl_ocr::Result<Option<Orientation>> {
        self.0.orientation(image, timeout)
    }
    fn languages(&self) -> pl_ocr::Result<Vec<String>> {
        self.0.languages()
    }
}

#[test]
fn parse_document_same_text_and_archive_content() {
    need!(pdfium, tesseract);
    let mut docs = vec![sample("rotated.pdf"), sample("multi-page-mixed.pdf"), sample("simple.png"), sample("multi-page-images.tiff")];
    let bench = repo().join("results/bench/ingest/scan-16p.pdf");
    if bench.exists() {
        docs.push(bench);
    }
    for doc in docs {
        let mime = match doc.extension().and_then(|e| e.to_str()) {
            Some("pdf") => "application/pdf",
            Some("png") => "image/png",
            _ => "image/tiff",
        };
        let mut outs = Vec::new();
        for (k, engine) in [Box::new(CliPnm(tesseract.clone())) as Box<dyn OcrEngine>, Box::new(tesseract.clone())].into_iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let mut cfg = pl_ocr::OcrConfig::defaults();
            cfg.threads_per_worker = 4;
            cfg.language = "deu+eng".into();
            let backend = NativeOcrmypdf::new(Some(engine), Box::new(pdfium.clone()) as Box<dyn PdfRenderer>, dir.path().join("pages"));
            let out = parse_document(&backend, &cfg, dir.path(), &doc, mime, true).unwrap_or_else(|e| panic!("{} ({k}): {e}", doc.display()));
            let archive = out.archive_path.as_deref().map(|p| content_digest(&std::fs::read(p).unwrap()));
            // no image files left behind
            let left: Vec<_> = std::fs::read_dir(dir.path().join("pages")).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            assert!(left.is_empty(), "{left:?}");
            outs.push((out.text, archive));
        }
        assert_eq!(outs[0], outs[1], "{}", doc.display());
    }
}

/// PDF content without time-dependent parts (info, XMP, trailer ID).
fn content_digest(data: &[u8]) -> Vec<u8> {
    let doc = lopdf::Document::load_mem(data).unwrap();
    let mut skip = Vec::new();
    if let Ok(r) = doc.trailer.get(b"Info").and_then(lopdf::Object::as_reference) {
        skip.push(r);
    }
    if let Ok(r) = doc.catalog().and_then(|c| c.get(b"Metadata")).and_then(lopdf::Object::as_reference) {
        skip.push(r);
    }
    let mut h = Vec::new();
    for (id, obj) in &doc.objects {
        if skip.contains(id) {
            continue;
        }
        // Object/xref streams (optimize ≥ 1) contain info or trailer ID; their content is
        // present after loading anyway as single objects in `doc.objects`.
        if let lopdf::Object::Stream(s) = obj
            && matches!(s.dict.get(b"Type"), Ok(lopdf::Object::Name(n)) if n.as_slice() == b"ObjStm" || n.as_slice() == b"XRef")
        {
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
    h
}

/// Release when idle (`PAPERRUST_OCR_ENGINE_IDLE_SECS`): unused instances are deleted,
/// afterwards a new one is created on the next call with the same result. Prints the time of the first
/// recognition after release (reloading the language models) against a warm recognition.
#[test]
fn library_releases_idle_instances_and_recreates() {
    need!(tesseract);
    let Some(lib) = tesseract.library() else { return };
    let td = tesseract.library_tessdata();
    let img = image::open(sample("simple.png")).unwrap();
    let r = Raster::from_dynamic(&img, 300.0);
    let run = || lib.recognize(td.as_deref(), Some(4), &r, &req("deu+eng", r.dpi)).unwrap();
    let first = run();
    let t = std::time::Instant::now();
    let warm = run();
    let warm_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(first.text, warm.text);
    // Deadline not reached yet: release nothing.
    assert_eq!(lib.release_idle(Duration::from_secs(3600), std::time::Instant::now()), 0);
    // Deadline expired (point in time in the future): at least our instance is deleted.
    assert!(lib.release_idle(Duration::from_secs(1), std::time::Instant::now() + Duration::from_secs(5)) >= 1);
    pl_ocr::tesslib::release_to_os();
    let t = std::time::Instant::now();
    let again = run();
    let cold_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(first.text, again.text);
    assert_eq!(first.words.len(), again.words.len());
    eprintln!("deu+eng simple.png: warm {warm_ms:.0} ms, first after release {cold_ms:.0} ms");
}
