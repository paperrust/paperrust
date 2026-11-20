//! Parallel page OCR (`jobs > 1`) must deliver exactly the same as serial processing
//! (`jobs = 1`): same archive bytes (apart from timestamps/trailer ID, which depend on the clock),
//! same sidecar file, same page order, same error.
//!
//! Fake engine with artificial, pseudo-random latency per page (completion order ≠
//! page order) and fake renderer (without PDFium). At the end a test with real
//! Tesseract/PDFium (skipped if not present).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pl_ocr::OcrArgs;
use pl_ocr::engine::{OcrEngine, OcrRequest, Orientation, PageOcr, Word};
use pl_ocr::native::NativeOcrmypdf;
use pl_ocr::pdfwrite::{self, ImagePageSpec, OutputMeta, PageImage};
use pl_ocr::render::{PdfRenderer, Raster, RenderDoc};
use pl_ocr::{OcrError, Result};
use serde_json::{Value, json};

// --------------------------------------------------------------------------------------------
// Fakes
// --------------------------------------------------------------------------------------------

/// Page number from `page-0007.pnm`.
fn pageno(p: &Path) -> usize {
    p.file_stem().unwrap().to_string_lossy().trim_start_matches("page-").parse().unwrap()
}

/// Pseudo-random latency (deterministic per page).
fn latency(page: usize, base_ms: u64, jitter_ms: u64) -> Duration {
    let mut x = (page as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    x ^= x >> 33;
    Duration::from_millis(base_ms + if jitter_ms == 0 { 0 } else { x % jitter_ms })
}

struct FakeEngine {
    base_ms: u64,
    jitter_ms: u64,
    /// Pages (1-based) on which `recognize` fails; value = additional delay.
    fail: Vec<(usize, u64)>,
    live: AtomicUsize,
    peak: AtomicUsize,
    order: Mutex<Vec<usize>>,
    osd_rotate: Option<u16>,
}

impl FakeEngine {
    fn new(base_ms: u64, jitter_ms: u64) -> Self {
        Self { base_ms, jitter_ms, fail: vec![], live: AtomicUsize::new(0), peak: AtomicUsize::new(0), order: Mutex::new(vec![]), osd_rotate: None }
    }
}

impl OcrEngine for FakeEngine {
    fn name(&self) -> String {
        "fake".into()
    }
    fn recognize(&self, image: &Path, req: &OcrRequest) -> Result<PageOcr> {
        let n = pageno(image);
        let l = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(l, Ordering::SeqCst);
        // really read the image (file must exist until the end of recognize)
        let bytes = std::fs::read(image).expect("page image exists");
        let fail = self.fail.iter().find(|(p, _)| *p == n);
        std::thread::sleep(latency(n, self.base_ms, self.jitter_ms) + Duration::from_millis(fail.map_or(0, |f| f.1)));
        self.order.lock().unwrap().push(n);
        self.live.fetch_sub(1, Ordering::SeqCst);
        if fail.is_some() {
            return Err(OcrError::SubprocessOutput(format!("tesseract failed on page {n}")));
        }
        let sum: u64 = bytes.iter().map(|b| u64::from(*b)).sum();
        let dpi = req.dpi.unwrap_or(0);
        let words = (0..3)
            .map(|k| Word {
                text: format!("p{n}w{k}"),
                x0: 10.0 + 60.0 * k as f64,
                y0: 10.0 + n as f64,
                x1: 60.0 + 60.0 * k as f64,
                y1: 30.0 + n as f64,
                conf: 90.0,
                line: (1, 1, 1),
            })
            .collect();
        let hdr = String::from_utf8_lossy(&bytes[..bytes.len().min(32)]).into_owned();
        let mut it = hdr.split_ascii_whitespace().skip(1);
        let width = it.next().unwrap().parse().unwrap();
        let height = it.next().unwrap().parse().unwrap();
        Ok(PageOcr { width, height, words, text: format!("Page {n} dpi {dpi} sum {sum}\n\x0c") })
    }
    fn orientation(&self, image: &Path, _: Option<Duration>) -> Result<Option<Orientation>> {
        let n = pageno(image);
        Ok(self.osd_rotate.filter(|_| n.is_multiple_of(3)).map(|r| Orientation { rotate: r, confidence: 50.0 }))
    }
    fn languages(&self) -> Result<Vec<String>> {
        Ok(vec!["eng".into(), "deu".into()])
    }
}

/// Renderer without PDFium: deterministic raster per page; counts concurrent calls
/// (must always be ≤ 1 – PDFium is not thread-safe).
struct FakeRenderer {
    concurrent: AtomicUsize,
    max_concurrent: AtomicUsize,
    renders: AtomicUsize,
}

struct FakeDoc<'a> {
    r: &'a FakeRenderer,
    pages: usize,
}

impl PdfRenderer for FakeRenderer {
    fn name(&self) -> String {
        "fake".into()
    }
    fn open<'a>(&'a self, pdf: &'a [u8]) -> Result<Box<dyn RenderDoc + 'a>> {
        let doc = pdfwrite::load_pdf(pdf)?;
        Ok(Box::new(FakeDoc { r: self, pages: doc.get_pages().len() }))
    }
}

impl RenderDoc for FakeDoc<'_> {
    fn page_count(&self) -> usize {
        self.pages
    }
    fn page_size_pt(&self, _: usize) -> Result<(f64, f64)> {
        Ok((144.0, 216.0))
    }
    fn render(&self, index: usize, dpi: f64, gray: bool) -> Result<Raster> {
        let c = self.r.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
        self.r.max_concurrent.fetch_max(c, Ordering::SeqCst);
        self.r.renders.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(2));
        let (w, h) = ((144.0 * dpi / 72.0) as u32, (216.0 * dpi / 72.0) as u32);
        let ch = if gray { 1 } else { 3 };
        let data = (0..w * h * ch).map(|k| ((k as usize * (index + 3)) % 251) as u8).collect();
        self.r.concurrent.fetch_sub(1, Ordering::SeqCst);
        Ok(Raster { width: w, height: h, channels: ch as u8, data, dpi })
    }
}

fn renderer() -> FakeRenderer {
    FakeRenderer { concurrent: AtomicUsize::new(0), max_concurrent: AtomicUsize::new(0), renders: AtomicUsize::new(0) }
}

// --------------------------------------------------------------------------------------------
// inputs
// --------------------------------------------------------------------------------------------

fn page_raster(i: usize, w: u32, h: u32, dpi: f64) -> Raster {
    let data = (0..w * h).map(|k| if (k as usize + i * 13) % 17 < 3 { 0 } else { 255 }).collect();
    Raster { width: w, height: h, channels: 1, data, dpi }
}

/// Pure image PDF (like a scan) with `n` pages.
fn image_pdf(n: usize) -> Vec<u8> {
    let specs: Vec<ImagePageSpec> = (0..n)
        .map(|i| ImagePageSpec { image: PageImage::Raw(page_raster(i, 200, 300, 100.0)), dpi_x: 100.0, dpi_y: 100.0, rotate: 0, ocr: None })
        .collect();
    pdfwrite::build_from_images(&specs, &OutputMeta { output_type: pl_ocr::config::OutputType::Pdf, ..Default::default() }).unwrap()
}

/// Multi-page grayscale TIFF.
fn multi_tiff(n: usize) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut enc = tiff::encoder::TiffEncoder::new(&mut buf).unwrap();
        for i in 0..n {
            let r = page_raster(i, 120 + i as u32, 160, 300.0);
            enc.write_image::<tiff::encoder::colortype::Gray8>(r.width, r.height, &r.data).unwrap();
        }
    }
    buf.into_inner()
}

/// Neutralize clock-dependent parts: `D:YYYYMMDDHHMMSSZ`, XMP times, second trailer ID.
fn normalize(pdf: &[u8]) -> Vec<u8> {
    let mut v = pdf.to_vec();
    let mut i = 0;
    while i + 17 <= v.len() {
        if &v[i..i + 2] == b"D:" && v[i + 2..i + 16].iter().all(u8::is_ascii_digit) && v[i + 16] == b'Z' {
            v[i + 2..i + 16].fill(b'0');
            i += 17;
            continue;
        }
        // 2026-11-09T12:34:56Z
        if i + 20 <= v.len()
            && v[i + 4] == b'-'
            && v[i + 7] == b'-'
            && v[i + 10] == b'T'
            && v[i + 13] == b':'
            && v[i + 16] == b':'
            && v[i + 19] == b'Z'
            && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18].iter().all(|k| v[i + k].is_ascii_digit())
        {
            for k in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
                v[i + k] = b'0';
            }
            i += 20;
            continue;
        }
        i += 1;
    }
    // trailer: /ID[<..><..>] – zero the hex content
    if let Some(p) = v.windows(4).rposition(|w| w == b"/ID[" || w == b"/ID ") {
        let mut k = p + 3;
        let mut depth = 0;
        while k < v.len() {
            match v[k] {
                b'<' => depth += 1,
                b'>' => depth -= 1,
                b']' if depth == 0 => break,
                c if depth > 0 && c.is_ascii_hexdigit() => v[k] = b'0',
                _ => {}
            }
            k += 1;
        }
    }
    v
}

struct Out {
    pdf: Vec<u8>,
    sidecar: String,
    wall: Duration,
}

fn args(dir: &Path, input: &Path, jobs: i64, extra: &[(&str, Value)]) -> OcrArgs {
    let mut m = serde_json::Map::new();
    m.insert("input_file_or_options".into(), json!(input.to_string_lossy()));
    m.insert("output_file".into(), json!(dir.join("out.pdf").to_string_lossy()));
    m.insert("use_threads".into(), json!(true));
    m.insert("jobs".into(), json!(jobs));
    m.insert("language".into(), json!("eng"));
    m.insert("output_type".into(), json!("pdfa"));
    m.insert("progress_bar".into(), json!(false));
    m.insert("sidecar".into(), json!(dir.join("sidecar.txt").to_string_lossy()));
    for (k, v) in extra {
        m.insert((*k).into(), v.clone());
    }
    OcrArgs(m)
}

fn run(engine: FakeEngine, input_bytes: &[u8], ext: &str, jobs: i64, extra: &[(&str, Value)]) -> (Result<Out>, FakeEngine, FakeRenderer) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join(format!("in.{ext}"));
    std::fs::write(&input, input_bytes).unwrap();
    let engine = std::sync::Arc::new(engine);
    let rend = std::sync::Arc::new(renderer());
    struct E(std::sync::Arc<FakeEngine>);
    impl OcrEngine for E {
        fn name(&self) -> String { self.0.name() }
        fn recognize(&self, i: &Path, r: &OcrRequest) -> Result<PageOcr> { self.0.recognize(i, r) }
        fn orientation(&self, i: &Path, t: Option<Duration>) -> Result<Option<Orientation>> { self.0.orientation(i, t) }
        fn languages(&self) -> Result<Vec<String>> { self.0.languages() }
    }
    struct R(std::sync::Arc<FakeRenderer>);
    impl PdfRenderer for R {
        fn name(&self) -> String { self.0.name() }
        fn open<'a>(&'a self, pdf: &'a [u8]) -> Result<Box<dyn RenderDoc + 'a>> { self.0.open(pdf) }
    }
    let backend = NativeOcrmypdf::new(Some(Box::new(E(engine.clone()))), Box::new(R(rend.clone())), dir.path().join("pages"));
    let a = args(dir.path(), &input, jobs, extra);
    let t = Instant::now();
    let r = backend.run(&a).map(|()| Out {
        pdf: std::fs::read(dir.path().join("out.pdf")).unwrap(),
        sidecar: std::fs::read_to_string(dir.path().join("sidecar.txt")).unwrap(),
        wall: t.elapsed(),
    });
    // no leftover page images
    if let Ok(rd) = std::fs::read_dir(dir.path().join("pages")) {
        let left: Vec<PathBuf> = rd.map(|e| e.unwrap().path()).collect();
        assert!(left.is_empty(), "leftover page files: {left:?}");
    }
    drop(backend);
    let engine = std::sync::Arc::try_unwrap(engine).ok().unwrap();
    let rend = std::sync::Arc::try_unwrap(rend).ok().unwrap();
    (r, engine, rend)
}

/// Archive without object streams and compression (optimize ≥ 1 puts info with a timestamp into a
/// compressed object stream), then zero the clock fields.
fn canonical(pdf: &[u8]) -> Vec<u8> {
    let mut d = lopdf::Document::load_mem(pdf).unwrap();
    d.objects.retain(|_, o| {
        !matches!(o, lopdf::Object::Stream(s) if matches!(s.dict.get(b"Type"), Ok(lopdf::Object::Name(n)) if n.as_slice() == b"ObjStm" || n.as_slice() == b"XRef"))
    });
    d.decompress();
    d.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
    let mut out = Vec::new();
    d.save_to(&mut out).unwrap();
    normalize(&out)
}

fn assert_same(a: &Out, b: &Out, what: &str) {
    assert_eq!(a.sidecar, b.sidecar, "{what}: sidecar differs");
    assert_eq!(canonical(&a.pdf), canonical(&b.pdf), "{what}: archive bytes differ");
}

fn sidecar_pages(s: &str) -> Vec<usize> {
    s.split('\x0c')
        .filter_map(|p| p.strip_prefix("Page ").and_then(|r| r.split(' ').next()).and_then(|n| n.parse().ok()))
        .collect()
}

// --------------------------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------------------------

#[test]
fn normalize_only_touches_clock_fields() {
    let pdf = image_pdf(1);
    assert_eq!(normalize(&pdf).len(), pdf.len());
    assert_ne!(normalize(&pdf), b"".to_vec());
}

#[test]
fn pdf_parallel_equals_serial() {
    let input = image_pdf(14);
    let (serial, _, r1) = run(FakeEngine::new(3, 25), &input, "pdf", 1, &[]);
    let serial = serial.unwrap();
    assert_eq!(sidecar_pages(&serial.sidecar), (1..=14).collect::<Vec<_>>());
    assert_eq!(r1.renders.load(Ordering::SeqCst), 14);
    for jobs in [2, 4, 7, 32] {
        let (par, e, r) = run(FakeEngine::new(3, 25), &input, "pdf", jobs, &[]);
        let par = par.unwrap();
        assert_same(&serial, &par, &format!("pdf jobs={jobs}"));
        assert_eq!(r.max_concurrent.load(Ordering::SeqCst), 1, "renderer must never run concurrently");
        assert!(e.peak.load(Ordering::SeqCst) <= jobs as usize);
        assert!(e.peak.load(Ordering::SeqCst) > 1, "jobs={jobs}: no concurrency observed");
        // completion was not in page order, result ordered anyway
        let order = e.order.lock().unwrap().clone();
        assert_ne!(order, (1..=14).collect::<Vec<_>>(), "jitter should reorder completion");
    }
}

#[test]
fn pdf_parallel_equals_serial_with_pages_force_and_rotation() {
    let input = image_pdf(9);
    // --pages: subset (skipped pages stay Keep) – sidecar is dropped with pages, so without
    let extra = [("pages", json!("2-4,7")), ("force_ocr", json!(true)), ("rotate_pages", json!(true)), ("rotate_pages_threshold", json!(10.0))];
    let mk = || {
        let mut e = FakeEngine::new(2, 15);
        e.osd_rotate = Some(90);
        e
    };
    let (serial, _, _) = run(mk(), &input, "pdf", 1, &extra);
    let serial = serial.unwrap();
    assert_eq!(serial.sidecar.matches("[OCR skipped on page(s)").count(), 3);
    let (par, _, _) = run(mk(), &input, "pdf", 4, &extra);
    assert_same(&serial, &par.unwrap(), "pdf pages/force/rotate");
}

#[test]
fn tiff_parallel_equals_serial() {
    let input = multi_tiff(11);
    let extra = [("image_dpi", json!(300))];
    let (serial, _, _) = run(FakeEngine::new(3, 25), &input, "tiff", 1, &extra);
    let serial = serial.unwrap();
    assert_eq!(sidecar_pages(&serial.sidecar), (1..=11).collect::<Vec<_>>());
    for jobs in [3, 12] {
        let (par, e, _) = run(FakeEngine::new(3, 25), &input, "tiff", jobs, &extra);
        assert_same(&serial, &par.unwrap(), &format!("tiff jobs={jobs}"));
        assert!(e.peak.load(Ordering::SeqCst) > 1);
    }
}

#[test]
fn error_on_page_k_reported_identically() {
    let input = image_pdf(12);
    // page 5 fails slowly, page 8 fails immediately -> both modes report page 5.
    let mk = || {
        let mut e = FakeEngine::new(5, 10);
        e.fail = vec![(5, 120), (8, 0)];
        e
    };
    let (serial, es, _) = run(mk(), &input, "pdf", 1, &[]);
    let serial = serial.err().expect("serial must fail");
    assert!(serial.message().contains("page 5"), "{}", serial.message());
    // serial: nothing is OCRed after page 5
    assert_eq!(es.order.lock().unwrap().clone(), (1..=5).collect::<Vec<_>>());
    for jobs in [2, 4, 12] {
        let (par, _, _) = run(mk(), &input, "pdf", jobs, &[]);
        let par = par.err().expect("parallel must fail");
        assert_eq!(par.class_name(), serial.class_name());
        assert_eq!(par.message(), serial.message(), "jobs={jobs}");
    }
    // TIFF likewise
    let tif = multi_tiff(9);
    let extra = [("image_dpi", json!(300))];
    let (s, _, _) = run(mk(), &tif, "tiff", 1, &extra);
    let (p, _, _) = run(mk(), &tif, "tiff", 4, &extra);
    assert_eq!(s.err().unwrap().message(), p.err().unwrap().message());
}

#[test]
fn bomb_limit_checked_before_work() {
    // max_image_mpixels tiny -> DecompressionBomb on page 1 before any OCR runs
    let input = image_pdf(6);
    for jobs in [1, 4] {
        let (r, e, _) = run(FakeEngine::new(1, 0), &input, "pdf", jobs, &[("max_image_mpixels", json!(0.0001))]);
        let err = r.err().expect("must fail");
        assert_eq!(err.class_name(), OcrError::DecompressionBomb(String::new()).class_name());
        assert!(e.order.lock().unwrap().is_empty(), "jobs={jobs}: OCR ran before limit error");
    }
}

#[test]
fn parallel_wall_time_scales() {
    // 12 pages × 120 ms: serial ≈ 1.44 s, jobs=4 ideal ≈ 0.36 s.
    let input = image_pdf(12);
    let (s, _, _) = run(FakeEngine::new(120, 0), &input, "pdf", 1, &[]);
    let (p, _, _) = run(FakeEngine::new(120, 0), &input, "pdf", 4, &[]);
    let (s, p) = (s.unwrap(), p.unwrap());
    eprintln!("serial {:?}, jobs=4 {:?}", s.wall, p.wall);
    assert!(s.wall >= Duration::from_millis(1400));
    assert!(p.wall < s.wall / 2, "serial {:?} vs parallel {:?}", s.wall, p.wall);
}

// --------------------------------------------------------------------------------------------
// Real tools
// --------------------------------------------------------------------------------------------

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn real_tesseract_pdfium_parallel_equals_serial() {
    use pl_ocr::engine::TesseractCli;
    use pl_ocr::render::PdfiumRenderer;
    let pdfium = std::env::var_os("PAPERRUST_PDFIUM_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll"));
    let Ok(pdfium) = PdfiumRenderer::load(&pdfium) else {
        eprintln!("SKIP real_tesseract_pdfium_parallel_equals_serial: pdfium not available");
        return;
    };
    let tess = match std::env::var_os("PAPERRUST_TESSERACT_PATH") {
        Some(p) => TesseractCli { program: p.into(), tessdata_dir: std::env::var_os("PAPERRUST_TESSDATA_DIR").map(PathBuf::from), psm: None },
        None => {
            let base = repo().join("tests/parity/pl-ocr/.tools/tesseract");
            let exe = if cfg!(windows) { "tesseract.exe" } else { "tesseract" };
            TesseractCli { program: base.join("bin").join(exe), tessdata_dir: Some(base.join("tessdata")), psm: None }
        }
    };
    if !tess.is_available() {
        eprintln!("SKIP real_tesseract_pdfium_parallel_equals_serial: tesseract not available");
        return;
    }
    // Scan-like PDF: rasterize pages of a digital upstream PDF with PDFium, rebuild as image pages.
    let src = std::fs::read(repo().join("upstream/src/paperless_testing/sample_files/multi-page-digital.pdf")).unwrap();
    let specs: Vec<ImagePageSpec> = {
        let doc = pdfium.open(&src).unwrap();
        (0..doc.page_count())
            .map(|i| ImagePageSpec { image: PageImage::Raw(doc.render(i, 150.0, true).unwrap()), dpi_x: 150.0, dpi_y: 150.0, rotate: 0, ocr: None })
            .collect()
    };
    let input = pdfwrite::build_from_images(&specs, &OutputMeta { output_type: pl_ocr::config::OutputType::Pdf, ..Default::default() }).unwrap();
    let go = |jobs: i64| {
        let dir = tempfile::tempdir().unwrap();
        let inp = dir.path().join("in.pdf");
        std::fs::write(&inp, &input).unwrap();
        let backend = NativeOcrmypdf::new(Some(Box::new(tess.clone())), Box::new(pdfium.clone()), dir.path().join("pages"));
        backend.run(&args(dir.path(), &inp, jobs, &[])).unwrap();
        Out {
            pdf: std::fs::read(dir.path().join("out.pdf")).unwrap(),
            sidecar: std::fs::read_to_string(dir.path().join("sidecar.txt")).unwrap(),
            wall: Duration::ZERO,
        }
    };
    let s = go(1);
    let p = go(3);
    assert!(s.sidecar.to_lowercase().contains("page"), "OCR text: {:?}", s.sidecar);
    assert_same(&s, &p, "real tesseract");
}
