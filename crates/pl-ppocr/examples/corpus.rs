//! Stage-0/1 measurement tool: runs PP-OCR (and the orientation classifiers) on the synthetic
//! evaluation corpus and writes one text file per document in the layout `tools/score.py` expects.
//!
//! ```text
//! cargo run -p pl-ppocr --release --example corpus -- \
//!     --runtime ort --threads 4 --corpus <dir> --models <dir> --out <dir> \
//!     [--ort-lib <onnxruntime.dll>] [--orient none|pplcnet|doctr] [--only name,name] [--orient-eval]
//! ```
//! Real (private) pages are never read: only `<corpus>/*.json` documents are used.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use pl_ppocr::image::Image;
use pl_ppocr::orient::{OrientModel, Orientation};
use pl_ppocr::pipeline::{OcrParams, PpOcr};
use pl_ppocr::runtime::Runtime;

struct Args {
    runtime: String,
    threads: usize,
    corpus: PathBuf,
    models: PathBuf,
    out: PathBuf,
    ort_lib: Option<PathBuf>,
    orient: String,
    only: Vec<String>,
    orient_eval: bool,
    variant: String,
    htr_model: Option<PathBuf>,
    htr_below: f32,
    sessions: usize,
    parallel: usize,
    max_side: usize,
    rec_batch: usize,
}

fn parse() -> Args {
    let mut a = Args {
        runtime: "ort".into(),
        threads: 4,
        corpus: PathBuf::new(),
        models: PathBuf::new(),
        out: PathBuf::from("out"),
        ort_lib: None,
        orient: "none".into(),
        only: Vec::new(),
        orient_eval: false,
        variant: "small".into(),
        htr_model: None,
        htr_below: 0.9,
        sessions: 1,
        parallel: 1,
        max_side: pl_ppocr::pipeline::DEFAULT_MAX_SIDE,
        rec_batch: 6,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_default();
        match k.as_str() {
            "--runtime" => a.runtime = v(),
            "--threads" => a.threads = v().parse().unwrap_or(4),
            "--corpus" => a.corpus = v().into(),
            "--models" => a.models = v().into(),
            "--out" => a.out = v().into(),
            "--ort-lib" => a.ort_lib = Some(v().into()),
            "--orient" => a.orient = v(),
            "--only" => a.only = v().split(',').map(str::to_string).collect(),
            "--orient-eval" => a.orient_eval = true,
            "--variant" => a.variant = v(),
            "--sessions" => a.sessions = v().parse().unwrap_or(1),
            "--parallel" => a.parallel = v().parse().unwrap_or(1),
            "--rec-batch" => a.rec_batch = v().parse().unwrap_or(6),
            "--max-side" => a.max_side = v().parse().unwrap_or(pl_ppocr::pipeline::DEFAULT_MAX_SIDE),
            "--htr-model" => a.htr_model = Some(v().into()),
            "--htr-below" => a.htr_below = v().parse().unwrap_or(0.9),
            other => eprintln!("unknown argument {other}"),
        }
    }
    a
}

fn runtime(a: &Args) -> Box<dyn Runtime> {
    match a.runtime.as_str() {
        #[cfg(feature = "ort")]
        "ort" => {
            let cands: Vec<PathBuf> = a.ort_lib.iter().cloned().collect();
            Box::new(pl_ppocr::runtime::ort_rt::OrtRuntime::load(&cands).expect("load onnxruntime"))
        }
        other => panic!("runtime {other} not compiled in"),
    }
}

fn load_image(p: &Path) -> Image {
    let img = image::open(p).expect("open image").to_rgb8();
    let (w, h) = img.dimensions();
    Image::new(w as usize, h as usize, 3, img.into_raw()).expect("image")
}

/// (name, category, image paths)
fn docs(corpus: &Path, only: &[String]) -> Vec<(String, String, Vec<PathBuf>)> {
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(corpus).expect("corpus dir").filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(s) = std::fs::read_to_string(&p) else { continue };
        let Ok(v) = serde_json_lite(&s) else { continue };
        let (name, cat, imgs) = v;
        if !only.is_empty() && !only.iter().any(|o| name.contains(o.as_str())) {
            continue;
        }
        let imgs: Vec<PathBuf> = imgs.iter().map(|i| corpus.join(i)).collect();
        if imgs.iter().all(|i| i.exists()) {
            out.push((name, cat, imgs));
        }
    }
    out
}

/// Tiny extraction of `name`, `category`, `images` from the corpus JSON (avoids a serde dependency).
fn serde_json_lite(s: &str) -> Result<(String, String, Vec<String>), ()> {
    let field = |key: &str| -> Option<String> {
        let i = s.find(&format!("\"{key}\""))?;
        let rest = &s[i + key.len() + 2..];
        let q = rest.find('"')?;
        let rest = &rest[q + 1..];
        let e = rest.find('"')?;
        Some(rest[..e].to_string())
    };
    let name = field("name").ok_or(())?;
    let cat = field("category").unwrap_or_default();
    let i = s.find("\"images\"").ok_or(())?;
    let rest = &s[i..];
    let a = rest.find('[').ok_or(())?;
    let b = rest.find(']').ok_or(())?;
    let imgs = rest[a + 1..b].split(',').map(|x| x.trim().trim_matches('"').to_string()).filter(|x| !x.is_empty()).collect();
    Ok((name, cat, imgs))
}

fn main() {
    run();
    // peak resident memory (Linux)
    if let Ok(st) = std::fs::read_to_string("/proc/self/status")
        && let Some(l) = st.lines().find(|l| l.starts_with("VmHWM"))
    {
        eprintln!("PEAK {}", l.trim_start_matches("VmHWM:").trim());
    }
}

fn run() {
    let a = parse();
    if std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_writer(std::io::stderr).init();
    }
    let rt = runtime(&a);
    eprintln!("runtime: {}, threads {}", rt.name(), a.threads);
    if a.orient_eval {
        return orient_eval(&a, rt.as_ref());
    }
    let (detf, recf) = match a.variant.as_str() {
        "medium" => ("PP-OCRv6_det_medium.onnx", "PP-OCRv6_rec_medium.onnx"),
        "tiny" => ("PP-OCRv6_det_tiny.onnx", "PP-OCRv6_rec_tiny.onnx"),
        "small-tinyrec" => ("PP-OCRv6_det_small.onnx", "PP-OCRv6_rec_tiny.onnx"),
        _ => ("PP-OCRv6_det_small.onnx", "PP-OCRv6_rec_small.onnx"),
    };
    let t0 = Instant::now();
    let layout = pl_ppocr::runtime::session_threads(a.threads, a.sessions);
    eprintln!("session layout {layout:?}, parallel {}", a.parallel);
    let det = rt.load_pool(&a.models.join(detf), &layout).expect("det");
    let rec = rt.load_pool(&a.models.join(recf), &layout).expect("rec");
    let ocr = PpOcr::new(det, rec, OcrParams { max_side: a.max_side, rec_batch: a.rec_batch, ..OcrParams::default() }).expect("ppocr");
    let orient = match a.orient.as_str() {
        "pplcnet" => {
            Some(Orientation::new(rt.load(&a.models.join("pp-lcnet_x1_0_doc_ori.onnx"), a.threads).expect("orient"), OrientModel::PpLcnet))
        }
        "doctr" => Some(Orientation::new(
            rt.load(&a.models.join("mobilenet_v3_small_page_orientation-60606ce4.onnx"), a.threads).expect("orient"),
            OrientModel::DoctrMobilenet,
        )),
        _ => None,
    };
    let htr = a.htr_model.as_ref().map(|p| pl_ppocr::htr::Htr::new(rt.load(p, a.threads).expect("htr")));
    let pass = htr.as_ref().map(|h| pl_ppocr::HtrPass { htr: h, below: a.htr_below, max_lines: pl_ppocr::pipeline::HTR_MAX_LINES });
    let load_s = t0.elapsed().as_secs_f64();
    eprintln!("models loaded in {load_s:.2}s, charset {}", ocr.charset_len());
    std::fs::create_dir_all(&a.out).expect("out dir");
    if a.parallel > 1 {
        return parallel_run(&a, &ocr, pass.as_ref());
    }
    let mut csv = std::fs::File::create(a.out.join("timing.csv")).expect("timing.csv");
    let _ = writeln!(csv, "name,category,pages,seconds,seconds_per_page,lines,orient_s");
    let (mut total, mut pages) = (0f64, 0usize);
    for (name, cat, imgs) in docs(&a.corpus, &a.only) {
        let mut texts = Vec::new();
        let mut secs = 0f64;
        let mut osecs = 0f64;
        let mut nlines = 0;
        for p in &imgs {
            let mut img = load_image(p);
            let t = Instant::now();
            if let Some(o) = &orient {
                let r = o.classify(&img).expect("classify");
                osecs += t.elapsed().as_secs_f64();
                if r.rotate_cw != 0 && r.prob >= 0.85 {
                    eprintln!("  {name}: rotate {} (p={:.3})", r.rotate_cw, r.prob);
                    img = img.rotate_cw(r.rotate_cw);
                }
            }
            let page = ocr.recognize_with(&img, pass.as_ref()).expect("recognize");
            secs += t.elapsed().as_secs_f64();
            nlines += page.lines.len();
            texts.push(page.text().trim_end().to_string());
        }
        total += secs;
        pages += imgs.len();
        let _ = writeln!(csv, "{name},{cat},{},{secs:.3},{:.3},{nlines},{osecs:.3}", imgs.len(), secs / imgs.len() as f64);
        let out = a.out.join(format!("{name}.txt"));
        std::fs::write(&out, texts.join("\n\u{c}\n")).expect("write");
        eprintln!("{name}: {:.2}s/page, {nlines} lines", secs / imgs.len() as f64);
    }
    eprintln!("TOTAL {pages} pages, {:.3} s/page, load {load_s:.2}s", total / pages.max(1) as f64);
    let _ = writeln!(csv, "TOTAL,,{pages},{total:.3},{:.3},,", total / pages.max(1) as f64);
}

/// Classifies every corpus page rotated by 0/90/180/270 with both orientation models.
fn orient_eval(a: &Args, rt: &dyn Runtime) {
    let pp = Orientation::new(rt.load(&a.models.join("pp-lcnet_x1_0_doc_ori.onnx"), a.threads).expect("pplcnet"), OrientModel::PpLcnet);
    let dt = Orientation::new(
        rt.load(&a.models.join("mobilenet_v3_small_page_orientation-60606ce4.onnx"), a.threads).expect("doctr"),
        OrientModel::DoctrMobilenet,
    );
    std::fs::create_dir_all(&a.out).expect("out dir");
    let mut csv = std::fs::File::create(a.out.join("orient.csv")).expect("orient.csv");
    let _ = writeln!(csv, "name,category,page,applied_cw,expected_fix,pplcnet_fix,pplcnet_p,pplcnet_s,doctr_fix,doctr_p,doctr_s");
    for (name, cat, imgs) in docs(&a.corpus, &a.only) {
        for (pi, p) in imgs.iter().enumerate() {
            let base = load_image(p);
            for rot in [0u16, 90, 180, 270] {
                let img = base.rotate_cw(rot);
                let t = Instant::now();
                let r1 = pp.classify(&img).expect("pp");
                let s1 = t.elapsed().as_secs_f64();
                let t = Instant::now();
                let r2 = dt.classify(&img).expect("dt");
                let s2 = t.elapsed().as_secs_f64();
                let expected = (360 - rot) % 360;
                let _ = writeln!(
                    csv,
                    "{name},{cat},{},{rot},{expected},{},{:.4},{s1:.4},{},{:.4},{s2:.4}",
                    pi + 1,
                    r1.rotate_cw,
                    r1.prob,
                    r2.rotate_cw,
                    r2.prob
                );
            }
        }
        eprintln!("{name} done");
    }
}

/// All pages of all selected documents through `a.parallel` worker threads (like the page pool of
/// the app); reports wall time per page. Texts are written per document as in the serial mode.
fn parallel_run(a: &Args, ocr: &PpOcr, pass: Option<&pl_ppocr::HtrPass<'_>>) {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let docs = docs(&a.corpus, &a.only);
    let jobs: Vec<(usize, usize, PathBuf)> =
        docs.iter().enumerate().flat_map(|(d, (_, _, imgs))| imgs.iter().enumerate().map(move |(p, i)| (d, p, i.clone()))).collect();
    let texts: Mutex<Vec<Vec<String>>> = Mutex::new(docs.iter().map(|(_, _, i)| vec![String::new(); i.len()]).collect());
    let next = AtomicUsize::new(0);
    let t0 = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..a.parallel {
            s.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::SeqCst);
                    let Some((d, p, path)) = jobs.get(k) else { break };
                    let img = load_image(path);
                    let page = ocr.recognize_with(&img, pass).expect("recognize");
                    texts.lock().expect("lock")[*d][*p] = page.text().trim_end().to_string();
                }
            });
        }
    });
    let wall = t0.elapsed().as_secs_f64();
    let texts = texts.into_inner().expect("lock");
    for ((name, _, _), t) in docs.iter().zip(texts) {
        std::fs::write(a.out.join(format!("{name}.txt")), t.join("\n\u{c}\n")).expect("write");
    }
    eprintln!("TOTAL {} pages, {:.3} s/page wall (parallel {})", jobs.len(), wall / jobs.len().max(1) as f64, a.parallel);
}
