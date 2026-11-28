//! PP-OCR (PaddleOCR PP-OCRv6) as an [`OcrEngine`] (`PAPERRUST_OCR_ENGINE=ppocr`) and the page
//! orientation model used before any recognition (`PAPERRUST_OCR_ORIENTATION`).
//!
//! PaperRust addition (no upstream counterpart); plan `docs/OCR-NEXT.md` stages 1 and 2, details
//! and known differences `docs/OCR-PPOCR.md`. Detection/recognition/word boxes/reading order live
//! in the `pl-ppocr` crate; this module only finds models and the runtime, shares one model
//! instance per process and converts the result into [`PageOcr`].
//!
//! - **Runtime:** ONNX Runtime, loaded at run time (`PAPERRUST_ONNXRUNTIME_PATH`, the model
//!   directories, next to the program, then the system library). Missing → the caller falls back
//!   to Tesseract (warning once).
//! - **Models:** `PAPERRUST_OCR_MODEL_DIR` (only this directory), otherwise
//!   `<DATA_DIR>/models/ocr`, `/usr/local/share/paperrust/ocr-models` (Docker image) and
//!   `<program dir>/ocr-models`. Missing files are downloaded (fixed URL + SHA-256) only with
//!   `PAPERRUST_OCR_MODEL_DOWNLOAD=true`, into `<DATA_DIR>/models/ocr`.
//! - **Threads:** one shared session per model and process; its intra-op threads = size of the
//!   shared OCR page pool ([`crate::permits`]), runs of one model are serialized (see
//!   `pl_ppocr::runtime`), so concurrent documents never oversubscribe the cores.
//! - **Languages:** PP-OCRv6 is used for Latin-script languages only; if `PAPERLESS_OCR_LANGUAGE`
//!   contains another script (or an unknown code), the page is read by Tesseract.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pl_ppocr::models::{self, ModelFile, Variant};
use pl_ppocr::orient::{OrientModel, Orientation as OrientClassifier};
use pl_ppocr::runtime::Runtime;
use pl_ppocr::{Image, OcrParams, PpOcr};

use crate::engine::{OcrEngine, OcrRequest, Orientation, PageOcr, TesseractCli, Word};
use crate::engine_select::{Script, script_of};
use crate::render::Raster;
use crate::{OcrError, Result};

/// Directory below `DATA_DIR` for downloaded models.
pub const DATA_SUBDIR: &str = "models/ocr";
/// Model directory of the Docker image.
pub const IMAGE_MODEL_DIR: &str = "/usr/local/share/paperrust/ocr-models";

fn env(key: &str) -> Option<String> {
    pl_core::config::var(key).filter(|v| !v.trim().is_empty())
}

fn truthy(v: Option<String>) -> bool {
    v.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "y" | "on"))
}

/// `DATA_DIR` like pl-core (`PAPERLESS_DATA_DIR`, otherwise `<BASE_DIR>/data`).
fn data_dir() -> PathBuf {
    if let Some(d) = env("PAPERLESS_DATA_DIR") {
        return PathBuf::from(d);
    }
    env("PAPERRUST_BASE_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("data")
}

/// Download target for models (`<DATA_DIR>/models/ocr`, or `PAPERRUST_OCR_MODEL_DIR`).
pub fn download_dir() -> PathBuf {
    env("PAPERRUST_OCR_MODEL_DIR").map(PathBuf::from).unwrap_or_else(|| data_dir().join(DATA_SUBDIR))
}

/// Model directories in search order (see module docs).
pub fn model_dirs() -> Vec<PathBuf> {
    if let Some(d) = env("PAPERRUST_OCR_MODEL_DIR") {
        return vec![PathBuf::from(d)];
    }
    let mut v = vec![data_dir().join(DATA_SUBDIR)];
    if cfg!(unix) {
        v.push(PathBuf::from(IMAGE_MODEL_DIR));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        v.push(dir.join("ocr-models"));
    }
    v
}

/// Candidate paths of the ONNX Runtime library (before the bare system names).
fn library_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = env("PAPERRUST_ONNXRUNTIME_PATH") {
        v.push(PathBuf::from(p));
    }
    let names = pl_ppocr::runtime::ort_rt::default_library_names();
    for d in model_dirs() {
        v.extend(names.iter().map(|n| d.join(n)));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        v.extend(names.iter().map(|n| dir.join(n)));
    }
    v
}

static RUNTIME: OnceLock<std::result::Result<Arc<dyn Runtime>, String>> = OnceLock::new();

/// The process-wide ONNX Runtime (loaded once; the error is remembered).
pub fn runtime() -> std::result::Result<Arc<dyn Runtime>, String> {
    RUNTIME
        .get_or_init(|| {
            pl_ppocr::runtime::ort_rt::OrtRuntime::load(&library_candidates())
                .map(|r| {
                    tracing::info!("OCR models: {}", r.name());
                    Arc::new(r) as Arc<dyn Runtime>
                })
                .map_err(|e| e.to_string())
        })
        .clone()
}

/// Intra-op threads for one model run: the shared page pool size.
fn threads() -> usize {
    crate::permits::global().size()
}

/// Path of `m`, downloading it if allowed (`PAPERRUST_OCR_MODEL_DOWNLOAD`).
fn model_path(m: &ModelFile) -> std::result::Result<PathBuf, String> {
    let dirs = model_dirs();
    if let Some(p) = models::find(m, &dirs) {
        return Ok(p);
    }
    if !truthy(env("PAPERRUST_OCR_MODEL_DOWNLOAD")) {
        return Err(format!(
            "model file {} not found in {} (put it there, or set PAPERRUST_OCR_MODEL_DOWNLOAD=true to download it from {})",
            m.file,
            dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", "),
            m.url
        ));
    }
    let dir = download_dir();
    tracing::info!("downloading OCR model {} ({} MB) to {}", m.file, m.size / 1_000_000, dir.display());
    models::install(m, &dir, &mut http_get).map_err(|e| e.to_string())
}

/// Blocking HTTPS GET with a size cap (own current-thread runtime on a helper thread, like
/// `remote.rs`; callers may run inside a Tokio runtime).
fn http_get(url: &str) -> std::result::Result<Vec<u8>, String> {
    const MAX: usize = 200 * 1024 * 1024;
    let url = url.to_string();
    let h = std::thread::Builder::new()
        .name("ocr-model-download".into())
        .spawn(move || -> std::result::Result<Vec<u8>, String> {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            rt.block_on(async move {
                let client = reqwest::Client::builder()
                    .user_agent(pl_core::brand::USER_AGENT)
                    .timeout(Duration::from_secs(600))
                    .build()
                    .map_err(|e| e.to_string())?;
                let mut resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    return Err(format!("HTTP {}", resp.status()));
                }
                let mut buf = Vec::new();
                while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
                    if buf.len() + chunk.len() > MAX {
                        return Err("response too large".into());
                    }
                    buf.extend_from_slice(&chunk);
                }
                Ok(buf)
            })
        })
        .map_err(|e| e.to_string())?;
    h.join().map_err(|_| "download thread panicked".to_string())?
}

/// `PAPERRUST_PPOCR_MODEL` (`small` default; `medium` and `fast` only on explicit choice).
pub fn variant() -> Variant {
    let v = env("PAPERRUST_PPOCR_MODEL").unwrap_or_default();
    Variant::parse(&v).unwrap_or_else(|| {
        tracing::warn!("PAPERRUST_PPOCR_MODEL: invalid value {v:?} (allowed: small, medium, fast); using small");
        Variant::Small
    })
}

/// Shared instance or the remembered reason why it is unavailable.
type SharedOcr = std::result::Result<Arc<PpOcr>, String>;

/// `PAPERRUST_PPOCR_SESSIONS`: model sessions per process (default 1). More sessions let pages run
/// concurrently (one session with all pool threads for a lone page, the others share the cores),
/// but each costs ~0.3–0.6 GB; measured on the Jetson with 6 parallel pages: 1 session 6.27 s/page,
/// 3: 6.32, 4: 6.20, 7: 5.65 (5 GB) – the CPU is already saturated by one session.
fn sessions() -> usize {
    env("PAPERRUST_PPOCR_SESSIONS").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 16)
}

/// `PAPERRUST_PPOCR_MAX_SIDE`: longest image side for PP-OCR (default 1920, 960..=4096).
fn max_side() -> usize {
    env("PAPERRUST_PPOCR_MAX_SIDE")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map(|v| v.clamp(960, 4096))
        .unwrap_or(pl_ppocr::pipeline::DEFAULT_MAX_SIDE)
}

static OCR: OnceLock<Mutex<Option<SharedOcr>>> = OnceLock::new();

/// The shared PP-OCR instance (one per process). Errors are remembered, so a missing runtime or
/// model costs one lookup, not one per page.
pub fn shared_ocr() -> SharedOcr {
    let m = OCR.get_or_init(|| Mutex::new(None));
    let mut g = m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(r) = g.as_ref() {
        return r.clone();
    }
    let r = (|| {
        let rt = runtime()?;
        let (detf, recf) = variant().files();
        let layout = pl_ppocr::runtime::session_threads(threads(), sessions());
        let det = rt.load_pool(&model_path(&detf)?, &layout).map_err(|e| e.to_string())?;
        let rec = rt.load_pool(&model_path(&recf)?, &layout).map_err(|e| e.to_string())?;
        let params = OcrParams { max_side: max_side(), ..OcrParams::default() };
        let ocr = PpOcr::new(det, rec, params).map_err(|e| e.to_string())?;
        tracing::info!("PP-OCR ready ({} / {}, sessions {layout:?}, max side {})", detf.file, recf.file, max_side());
        Ok(Arc::new(ocr))
    })();
    *g = Some(r.clone());
    r
}

static ORIENT: OnceLock<std::result::Result<Arc<OrientClassifier>, String>> = OnceLock::new();

/// The shared page orientation classifier (PP-LCNet doc orientation, chosen in stage 1).
pub fn shared_orientation() -> std::result::Result<Arc<OrientClassifier>, String> {
    ORIENT
        .get_or_init(|| {
            let rt = runtime()?;
            let path = model_path(&models::ORIENT_PPLCNET)?;
            // A small model: one thread is enough (< 0.1 s per page) and leaves the pool alone.
            let m = rt.load(&path, 1).map_err(|e| e.to_string())?;
            Ok(Arc::new(OrientClassifier::new(m, OrientModel::PpLcnet)))
        })
        .clone()
}

/// Minimum class probability for the orientation model to turn a page (stage-1 measurement,
/// `results/ocr-eval/2026-11-25/stage1.md`).
pub const ORIENTATION_MIN_PROB: f64 = 0.85;

/// `PAPERRUST_OCR_HANDWRITING`: DE·HTR second pass for low-confidence lines (`off` default, `auto`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandwritingMode {
    Off,
    Auto,
}

impl HandwritingMode {
    pub fn parse(v: &str) -> std::result::Result<Self, String> {
        match v.trim().to_ascii_lowercase().as_str() {
            "" | "off" | "false" | "0" | "no" => Ok(Self::Off),
            "auto" | "on" | "true" | "1" | "yes" => Ok(Self::Auto),
            other => Err(format!("PAPERRUST_OCR_HANDWRITING: invalid value {other:?} (allowed: off, auto)")),
        }
    }

    pub fn from_env() -> Self {
        Self::parse(&env("PAPERRUST_OCR_HANDWRITING").unwrap_or_default()).unwrap_or_else(|e| {
            warn_once("handwriting-invalid", &format!("{e}; using off"));
            Self::Off
        })
    }
}

/// Lines with a PP-OCR score below this are read again by DE·HTR (stage-5 measurement,
/// `results/ocr-eval/2026-11-25/stage5.md`).
pub const HANDWRITING_BELOW: f32 = 0.95;

static HTR: OnceLock<std::result::Result<Arc<pl_ppocr::htr::Htr>, String>> = OnceLock::new();

/// The shared DE·HTR model (one session per process, intra-op threads = page pool size).
pub fn shared_htr() -> std::result::Result<Arc<pl_ppocr::htr::Htr>, String> {
    HTR.get_or_init(|| {
        let rt = runtime()?;
        let path = model_path(&models::DEHTR_V2)?;
        let m = rt.load(&path, threads()).map_err(|e| e.to_string())?;
        Ok(Arc::new(pl_ppocr::htr::Htr::new(m)))
    })
    .clone()
}

/// The handwriting pass for this request: only with `PAPERRUST_OCR_HANDWRITING=auto` and German in
/// the OCR languages (DE·HTR is a German model); a missing model → warning once, no pass.
fn handwriting_pass(languages: &str) -> Option<Arc<pl_ppocr::htr::Htr>> {
    if HandwritingMode::from_env() == HandwritingMode::Off || !languages.split('+').any(|l| l.trim() == "deu") {
        return None;
    }
    match shared_htr() {
        Ok(h) => Some(h),
        Err(e) => {
            warn_once(
                "handwriting-missing",
                &format!("PAPERRUST_OCR_HANDWRITING=auto, but DE·HTR is not available ({e}); skipping the handwriting pass"),
            );
            None
        }
    }
}

/// `PAPERRUST_OCR_ORIENTATION`: who decides the page orientation (with `rotate_pages`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientationMode {
    /// The orientation model (PP-LCNet), for every engine.
    Model,
    /// The OCR engine itself, as before (Tesseract OSD, Windows OCR via Tesseract OSD).
    Engine,
    /// Never rotate.
    Off,
}

impl OrientationMode {
    /// `None` = not set (automatic: model if runtime + model file are available, else engine).
    pub fn parse(v: &str) -> std::result::Result<Option<Self>, String> {
        match v.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(None),
            "model" => Ok(Some(Self::Model)),
            "engine" | "tesseract" => Ok(Some(Self::Engine)),
            "off" | "none" | "false" | "0" => Ok(Some(Self::Off)),
            other => Err(format!("PAPERRUST_OCR_ORIENTATION: invalid value {other:?} (allowed: model, engine, off)")),
        }
    }

    /// Effective mode: the setting, with `model` falling back to `engine` (warning once) when the
    /// runtime or model is missing; unset = `model` if available, else `engine`.
    pub fn resolve() -> (Self, Option<Arc<OrientClassifier>>) {
        let set = Self::parse(&env("PAPERRUST_OCR_ORIENTATION").unwrap_or_default()).unwrap_or_else(|e| {
            warn_once("orientation-invalid", &format!("{e}; using automatic choice"));
            None
        });
        match set {
            Some(Self::Off) => (Self::Off, None),
            Some(Self::Engine) => (Self::Engine, None),
            Some(Self::Model) | None => match shared_orientation() {
                Ok(c) => (Self::Model, Some(c)),
                Err(e) => {
                    if set == Some(Self::Model) {
                        warn_once(
                            "orientation-missing",
                            &format!(
                                "PAPERRUST_OCR_ORIENTATION=model, but the orientation model is not available ({e}); using the OCR engine"
                            ),
                        );
                    } else {
                        tracing::debug!("orientation model not available ({e}); using the OCR engine");
                    }
                    (Self::Engine, None)
                }
            },
        }
    }
}

fn warn_once(key: &'static str, msg: &str) {
    use std::collections::HashSet;
    static SEEN: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut g = SEEN.get_or_init(Default::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if g.insert(key) {
        tracing::warn!("{msg}");
    }
}

/// Orientation of a raster by the model: `Some(rotation)` if confident, `Ok(None)` = keep.
pub fn model_orientation(cls: &OrientClassifier, raster: &Raster) -> Result<Option<Orientation>> {
    let img = to_image(raster)?;
    let r = cls.classify(&img).map_err(|e| OcrError::SubprocessOutput(format!("orientation model: {e}")))?;
    Ok(Some(Orientation { rotate: r.rotate_cw, confidence: f64::from(r.prob) }))
}

fn to_image(r: &Raster) -> Result<Image> {
    Image::new(r.width as usize, r.height as usize, r.channels as usize, r.data.clone())
        .ok_or_else(|| OcrError::Other(format!("raster {}x{}x{} has an unexpected size", r.width, r.height, r.channels)))
}

/// Are all requested languages (`deu+eng`) Latin-script languages PP-OCRv6 reads?
pub fn latin_only(languages: &str) -> bool {
    let codes: Vec<&str> = languages.split('+').map(str::trim).filter(|c| !c.is_empty()).collect();
    !codes.is_empty() && codes.iter().all(|c| script_of(c) == Some(Script::Latin))
}

/// Converts a `pl-ppocr` page into [`PageOcr`]: words with boxes, confidence 0..100 (like
/// Tesseract's scale; the values are not comparable with Tesseract's), line id
/// `(block, row, segment)` – every detector box is its own text-layer line.
pub fn to_page_ocr(p: &pl_ppocr::Page) -> PageOcr {
    let mut words = Vec::new();
    for ((b, r, s), line) in &p.lines {
        for w in &line.words {
            if w.text.trim().is_empty() {
                continue;
            }
            words.push(Word {
                text: w.text.clone(),
                x0: f64::from(w.bbox[0]),
                y0: f64::from(w.bbox[1]),
                x1: f64::from(w.bbox[2]),
                y1: f64::from(w.bbox[3]),
                conf: (w.conf * 100.0).clamp(0.0, 100.0),
                line: (*b + 1, *r + 1, *s + 1),
            });
        }
    }
    PageOcr { width: p.width as u32, height: p.height as u32, words, text: p.text() }
}

/// `PAPERRUST_OCR_ENGINE=ppocr`.
pub struct PpOcrEngine {
    ocr: Arc<PpOcr>,
    /// Tesseract for non-Latin languages (and its language list).
    tesseract: Option<TesseractCli>,
    variant: Variant,
}

impl PpOcrEngine {
    /// Loads runtime and models; `Err` (with the reason) if anything is missing.
    pub fn new(tesseract: Option<TesseractCli>) -> std::result::Result<Self, String> {
        Ok(Self { ocr: shared_ocr()?, tesseract, variant: variant() })
    }

    fn tess(&self) -> Result<&TesseractCli> {
        self.tesseract.as_ref().ok_or_else(|| OcrError::Unavailable("tesseract not found (needed for non-Latin OCR languages)".into()))
    }

    fn run(&self, raster: &Raster, languages: &str) -> Result<PageOcr> {
        let img = to_image(raster)?;
        let htr = handwriting_pass(languages);
        let pass =
            htr.as_deref().map(|h| pl_ppocr::HtrPass { htr: h, below: HANDWRITING_BELOW, max_lines: pl_ppocr::pipeline::HTR_MAX_LINES });
        let page = self.ocr.recognize_with(&img, pass.as_ref()).map_err(|e| OcrError::SubprocessOutput(format!("PP-OCR: {e}")))?;
        Ok(to_page_ocr(&page))
    }
}

impl OcrEngine for PpOcrEngine {
    fn name(&self) -> String {
        let rt = runtime().map(|r| r.name()).unwrap_or_default();
        format!(
            "ppocr (PP-OCRv6 {}, {rt})",
            match self.variant {
                Variant::Medium => "medium",
                Variant::Fast => "fast",
                Variant::Small => "small",
            }
        )
    }

    fn recognize(&self, image: &Path, req: &OcrRequest) -> Result<PageOcr> {
        if !latin_only(&req.languages) {
            return self.tess()?.recognize(image, req);
        }
        let img = image::open(image).map_err(|e| OcrError::InputFile(format!("{}: {e}", image.display())))?;
        let raster = if img.color().has_color() {
            let rgb = img.to_rgb8();
            Raster { width: rgb.width(), height: rgb.height(), channels: 3, data: rgb.into_raw(), dpi: 0.0 }
        } else {
            let g = img.to_luma8();
            Raster { width: g.width(), height: g.height(), channels: 1, data: g.into_raw(), dpi: 0.0 }
        };
        self.run(&raster, &req.languages)
    }

    fn orientation(&self, image: &Path, timeout: Option<Duration>) -> Result<Option<Orientation>> {
        match &self.tesseract {
            Some(t) => t.orientation(image, timeout),
            None => Ok(None),
        }
    }

    fn languages(&self) -> Result<Vec<String>> {
        let mut v: Vec<String> = match &self.tesseract {
            Some(t) => t.languages().unwrap_or_default(),
            None => Vec::new(),
        };
        for code in crate::engine_select::latin_codes() {
            if !v.iter().any(|l| l == code) {
                v.push(code.to_string());
            }
        }
        Ok(v)
    }

    fn check_languages(&self, languages: &str) -> Result<()> {
        if latin_only(languages) {
            return Ok(());
        }
        match &self.tesseract {
            Some(t) => crate::engine::check_installed(&t.languages()?, languages),
            None => Err(OcrError::Unavailable(format!(
                "PP-OCR reads Latin-script languages only and tesseract is not available for: {languages}"
            ))),
        }
    }

    fn raster_file_ext(&self) -> &'static str {
        "tif"
    }

    fn recognize_raster(&self, raster: &Raster, file: &Path, req: &OcrRequest) -> Result<PageOcr> {
        if !latin_only(&req.languages) {
            return self.tess()?.recognize_raster(raster, file, req);
        }
        self.run(raster, &req.languages)
    }

    fn orientation_raster(&self, raster: &Raster, file: &Path, timeout: Option<Duration>) -> Result<Option<Orientation>> {
        // `PAPERRUST_OCR_ORIENTATION=engine` with ppocr: PP-OCR has no page orientation of its
        // own, so this is Tesseract OSD as before (same confidence scale as `rotate_pages_threshold`).
        match &self.tesseract {
            Some(t) => t.orientation_raster(raster, file, timeout),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pl_ppocr::{OcrWord, TextLine};

    #[test]
    fn latin_detection() {
        assert!(latin_only("deu+eng"));
        assert!(latin_only("fra"));
        assert!(!latin_only("deu+rus"));
        assert!(!latin_only("chi_sim"));
        assert!(!latin_only("frk"), "unknown codes go to tesseract");
        assert!(!latin_only(""));
    }

    #[test]
    fn page_conversion_keeps_lines_and_scales_confidence() {
        let w = |t: &str, x0: f32, c: f32| OcrWord { text: t.into(), bbox: [x0, 10.0, x0 + 40.0, 30.0], conf: c };
        let l1 = TextLine {
            quad: [[0.0, 10.0], [100.0, 10.0], [100.0, 30.0], [0.0, 30.0]],
            text: "Hallo Welt".into(),
            score: 0.9,
            words: vec![w("Hallo", 0.0, 0.9), w("Welt", 50.0, 0.5)],
        };
        let l2 = TextLine {
            quad: [[300.0, 10.0], [400.0, 10.0], [400.0, 30.0], [300.0, 30.0]],
            text: "rechts".into(),
            score: 0.9,
            words: vec![w("rechts", 300.0, 1.2)],
        };
        let page = pl_ppocr::Page::from_lines(1000, 800, vec![l2, l1]);
        let p = to_page_ocr(&page);
        assert_eq!((p.width, p.height), (1000, 800));
        assert_eq!(p.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), vec!["Hallo", "Welt", "rechts"]);
        assert_eq!(p.words[0].line, p.words[1].line);
        assert_ne!(p.words[1].line, p.words[2].line, "each detector box is its own text-layer line");
        assert!((p.words[0].conf - 90.0).abs() < 1e-3 && p.words[2].conf == 100.0);
        assert_eq!(p.text, "Hallo Welt rechts\n");
        assert_eq!(p.lines().len(), 2);
    }

    #[test]
    fn orientation_setting() {
        assert_eq!(OrientationMode::parse(""), Ok(None));
        assert_eq!(OrientationMode::parse("auto"), Ok(None));
        assert_eq!(OrientationMode::parse(" Model "), Ok(Some(OrientationMode::Model)));
        assert_eq!(OrientationMode::parse("engine"), Ok(Some(OrientationMode::Engine)));
        assert_eq!(OrientationMode::parse("off"), Ok(Some(OrientationMode::Off)));
        assert!(OrientationMode::parse("sideways").is_err());
    }

    #[test]
    fn handwriting_setting() {
        assert_eq!(HandwritingMode::parse(""), Ok(HandwritingMode::Off));
        assert_eq!(HandwritingMode::parse("AUTO"), Ok(HandwritingMode::Auto));
        assert_eq!(HandwritingMode::parse("off"), Ok(HandwritingMode::Off));
        assert!(HandwritingMode::parse("always").is_err());
    }

    #[test]
    fn missing_model_dirs_find_nothing() {
        // Only checks the pure path logic with an explicit directory list.
        let dirs = [PathBuf::from("/nonexistent-a"), PathBuf::from("/nonexistent-b")];
        assert!(models::find(&models::DET_SMALL, &dirs).is_none());
    }
}
