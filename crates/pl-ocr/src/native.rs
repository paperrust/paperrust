//! [`NativeOcrmypdf`]: executes the arguments produced by `construct_ocrmypdf_parameters` without
//! Python (replacement for `ocrmypdf.ocr`, img2pdf+pikepdf and Ghostscript PDF/A).
//!
//! Model: OCRmyPDF 17.12 (`_pipeline.py`: `is_ocr_required`, `get_page_square_dpi`,
//! `validate_pdfinfo_options`, `merge_sidecars`; `pdfinfo/info.py`). Flow per page:
//! analysis (text/images/vector) → decision → rasterize (PDFium) → possibly rotation (Tesseract OSD)
//! → straighten/clean ([`crate::preprocess`]) → OCR (engine) → text layer/page replacement (lopdf)
//! → PDF/A stamp → sidecar.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::args::ImageOps;
use crate::config::{OcrConfig, OutputType};
use crate::engine::{OcrEngine, OcrRequest, PageOcr, check_languages};
use crate::imageinfo::{self, ImageFormat, ImageInfo};
use crate::parser::OcrBackend;
use crate::pdfwrite::{self, ImagePageSpec, OutputMeta, PageAction, PageEdit, PageImage};
use crate::render::{PdfRenderer, Raster};
use crate::{OcrArgs, OcrError, Result};

/// OCRmyPDF `VECTOR_PAGE_DPI`.
pub const VECTOR_PAGE_DPI: f64 = 400.0;

/// PaperRust: smallest resolution to which a page that is too large is downscaled for OCR
/// (below that it stays at the error `DecompressionBombError`).
pub const MIN_FIT_DPI: f64 = 50.0;
/// OCRmyPDF default for `max_image_mpixels`.
pub const DEFAULT_MAX_IMAGE_MPIXELS: f64 = 250.0;

/// Known OCRmyPDF arguments that are evaluated here (paperless sets the first ones itself,
/// the rest comes from `OCR_USER_ARGS`).
const HANDLED: &[&str] = &[
    "input_file_or_options",
    "output_file",
    "use_threads",
    "jobs",
    "language",
    "output_type",
    "progress_bar",
    "color_conversion_strategy",
    "force_ocr",
    "redo_ocr",
    "skip_text",
    "mode",
    "clean",
    "clean_final",
    "deskew",
    "rotate_pages",
    "rotate_pages_threshold",
    "pages",
    "sidecar",
    "image_dpi",
    "max_image_mpixels",
    "invalidate_digital_signatures",
    "oversample",
    "tesseract_timeout",
    "skip_big",
    "continue_on_soft_render_error",
    "tagged_pdf_mode",
    // E5/E6: optimization, metadata, Tesseract fine settings
    "optimize",
    "jpeg_quality",
    "jpg_quality",
    "png_quality",
    "title",
    "author",
    "subject",
    "keywords",
    "tesseract_pagesegmode",
    "tesseract_oem",
    "tesseract_thresholding",
    "tesseract_config",
    "user_words",
    "user_patterns",
    "tesseract_non_ocr_timeout",
    "tesseract_downsample_large_images",
    "tesseract_downsample_above",
    "max_ocr_image_mpixels",
    "remove_vectors",
    "pdfa_image_compression",
    "ocr_engine",
    // E6 (2026-11-18): evaluated or assessed depending on the value, see `value_note`
    "unpaper_args",
    "remove_background",
    "fast_web_view",
    "sidecar_format",
    "pdf_renderer",
    "rasterizer",
];

/// No effect, but known (debug log only): output/flow control of OCRmyPDF.
const NO_EFFECT: &[&str] = &["quiet", "verbose", "keep_temporary_files", "no_overwrite", "use_threads", "progress_bar"];

/// Warning per OCRmyPDF key that is not (fully) implemented.
pub fn unsupported_reason(key: &str) -> Option<&'static str> {
    Some(match key {
        "jbig2_lossy" => "The --jbig2-lossy option is deprecated and will be ignored. Lossy JBIG2 is no longer supported.",
        "jbig2_page_group_size" => "The --jbig2-page-group-size option is deprecated and will be ignored.",
        "jbig2_threshold" => "jbig2_threshold only affects lossy JBIG2, which is not supported; ignored",
        "plugins" | "plugin" => "OCRmyPDF plugins are not supported; ignored",
        _ => return None,
    })
}

/// Gap E6: options whose effect depends on the value. `Some(warning)` if the value cannot work like
/// in OCRmyPDF; `None` if it works or equals the built-in behavior.
pub fn value_note(key: &str, v: &Value) -> Option<String> {
    let s = v.as_str().map(str::trim).unwrap_or("");
    match key {
        // OCRmyPDF ≥ 13 aborts with NotImplementedError; here only a warning.
        "remove_background" => {
            truthy(Some(v)).then(|| "remove_background is not implemented (OCRmyPDF raises NotImplementedError for it); ignored".to_owned())
        }
        "unpaper_args" => {
            let (_, ignored) = crate::preprocess::CleanFilters::from_unpaper_args(s);
            (!ignored.is_empty()).then(|| {
                format!("unpaper_args: {} not supported by the built-in cleaner (supported: --no-border-scan, --no-mask-scan, --no-border, --no-blackfilter, --no-grayfilter, --no-noisefilter); ignored", ignored.join(" "))
            })
        }
        // Threshold in MB; very large values practically disable linearization (= here).
        "fast_web_view" => {
            let mb = v.as_f64().or_else(|| s.parse().ok()).unwrap_or(1.0);
            (mb < 10_000.0).then(|| {
                "fast_web_view (linearization) is not supported by the native PDF writer; the archive is not linearized".to_owned()
            })
        }
        "sidecar_format" => (!matches!(s, "" | "txt" | "text"))
            .then(|| format!("sidecar_format '{s}' is not supported (sidecar is always plain text); ignored")),
        // Text layer: invisible GlyphLessFont layer like hocr/sandwich/fpdf2.
        "pdf_renderer" => (!matches!(s, "" | "auto" | "hocr" | "sandwich" | "fpdf2"))
            .then(|| format!("pdf_renderer '{s}' is not supported; the native invisible text layer is used")),
        "rasterizer" => (!matches!(s, "" | "auto" | "pypdfium" | "pdfium"))
            .then(|| format!("rasterizer '{s}' is not available; pages are rasterized with PDFium")),
        _ => None,
    }
}

pub struct NativeOcrmypdf {
    pub engine: Option<Box<dyn OcrEngine>>,
    pub renderer: Box<dyn PdfRenderer>,
    pub meta: OutputMeta,
    /// Upper limit of the raster DPI (RAM protection; OCRmyPDF has none).
    pub max_render_dpi: f64,
    /// Working directory for page images.
    pub workdir: PathBuf,
}

/// Evaluated OCRmyPDF options.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub input: PathBuf,
    pub output: PathBuf,
    pub sidecar: Option<PathBuf>,
    pub language: String,
    pub output_type: OutputType,
    pub mode: ProcessingMode,
    pub pages: Option<BTreeSet<usize>>,
    pub image_dpi: Option<f64>,
    pub rotate_pages: bool,
    pub rotate_threshold: f64,
    pub deskew: bool,
    pub clean: bool,
    pub clean_final: bool,
    pub max_image_mpixels: f64,
    pub invalidate_digital_signatures: bool,
    pub oversample: Option<f64>,
    pub tesseract_timeout: Option<Duration>,
    pub skip_big: Option<f64>,
    pub tagged_ignore: bool,
    /// OCRmyPDF `--jobs`: pages in parallel (≥ 1; 1 = serial).
    pub jobs: usize,
    /// `--optimize`, `--jpeg-quality`, `--png-quality`, `--pdfa-image-compression jpeg`.
    pub optimize: crate::optimize::OptimizeOptions,
    /// `--color-conversion-strategy` (effective with PDF/A only).
    pub color: crate::config::ColorConversion,
    /// `--title`/`--author`/`--subject`/`--keywords`.
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    /// Tesseract fine settings.
    pub tess: crate::engine::TessOptions,
    /// `--tesseract-non-ocr-timeout` (orientation detection).
    pub non_ocr_timeout: Option<Duration>,
    /// `--max-ocr-image-mpixels` or `--tesseract-downsample-large-images` (+ `--tesseract-downsample-above`):
    /// downscale the recognition image (visible page stays).
    pub ocr_max_pixels: Option<f64>,
    pub ocr_max_side: Option<u32>,
    /// `--remove-vectors`: remove vector graphics before rasterizing for OCR.
    pub remove_vectors: bool,
    /// `--ocr-engine none`: no text recognition (PDF/A output only).
    pub no_ocr: bool,
    /// `--unpaper-args`: sub-steps of cleaning (gap E6).
    pub clean_filters: crate::preprocess::CleanFilters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingMode {
    Default,
    Force,
    Skip,
    Redo,
}

impl Options {
    pub fn lossless_reconstruction(&self) -> bool {
        !(self.deskew || self.clean_final || self.mode == ProcessingMode::Force)
    }

    /// Preprocessing per page. OCRmyPDF: `--clean-final` also cleans the recognition image.
    pub fn steps(&self) -> crate::preprocess::Steps {
        crate::preprocess::Steps {
            deskew: self.deskew,
            clean: self.clean || self.clean_final,
            clean_final: self.clean_final,
            filters: self.clean_filters,
        }
    }
}

/// Result of a page from [`NativeOcrmypdf::ocr_raster`].
#[derive(Debug)]
pub struct RasterResult {
    /// Rotation by 90/180/270° (`rotate_pages`).
    pub rotate: u16,
    /// Visible page image (in new display orientation; straightened, cleaned with clean-final).
    pub visible: Raster,
    /// `visible` differs from the input image → replace the page with `visible`.
    pub visible_changed: bool,
    pub ocr: PageOcr,
}

/// OCRmyPDF quality of the visible JPEG page (`create_visible_page_jpg`: Pillow default 75).
const VISIBLE_JPEG_QUALITY: u8 = 75;

/// Replaced page image: JPEG if the original contained only JPEG (OCRmyPDF
/// `should_visible_page_image_use_jpg`), otherwise lossless (Flate, like PNG).
fn visible_page_image(r: Raster, jpeg: bool) -> PageImage {
    if jpeg {
        let color = if r.channels == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
        let mut buf = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, VISIBLE_JPEG_QUALITY);
        if enc.encode(&r.data, r.width, r.height, color).is_ok() {
            return PageImage::Jpeg { data: buf, width: r.width, height: r.height, components: r.channels };
        }
    }
    PageImage::Raw(r)
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(crate::config::py_truthy)
}

/// `--pages` expression (`1-3,5`) → 1-based page set.
pub fn parse_pages(s: &str) -> Result<BTreeSet<usize>> {
    let mut out = BTreeSet::new();
    for group in s.split(',').map(str::trim).filter(|g| !g.is_empty()) {
        let bad = || OcrError::Other(format!("BadArgsError: invalid page subrange '{group}'"));
        if let Some((a, b)) = group.split_once('-') {
            let a: usize = a.trim().parse().map_err(|_| bad())?;
            let b: usize = b.trim().parse().map_err(|_| bad())?;
            if a == 0 || b < a {
                return Err(bad());
            }
            out.extend(a..=b.min(a + 1_000_000));
        } else {
            let n: usize = group.parse().map_err(|_| OcrError::Other(format!("BadArgsError: invalid page number '{group}'")))?;
            if n == 0 {
                return Err(bad());
            }
            out.insert(n);
        }
    }
    Ok(out)
}

/// OcrArgs → Options (incl. OCRmyPDF checks of the argument combination).
pub fn parse_options(args: &OcrArgs) -> Result<Options> {
    for k in args.keys() {
        if HANDLED.contains(&k) {
            if let Some(note) = args.get(k).and_then(|v| value_note(k, v)) {
                tracing::warn!("OCRmyPDF option '{k}': {note}");
            }
            continue;
        }
        if NO_EFFECT.contains(&k) {
            tracing::debug!("OCRmyPDF option '{k}' has no effect in the native OCR backend");
        } else if let Some(reason) = unsupported_reason(k) {
            tracing::warn!("OCRmyPDF option '{k}': {reason}");
        } else {
            tracing::warn!("OCRmyPDF option '{k}' is not supported by the native OCR backend and is ignored");
        }
    }
    let mut mode_force = false;
    let mut mode_redo = false;
    let mut mode_skip = false;
    match args.str("mode") {
        None | Some("default" | "auto") => {}
        Some("force") => mode_force = true,
        Some("redo") => mode_redo = true,
        Some("skip") => mode_skip = true,
        Some(other) => return Err(OcrError::Other(format!("BadArgsError: invalid --mode '{other}'"))),
    }
    let force = args.flag("force_ocr") || mode_force;
    let redo = args.flag("redo_ocr") || mode_redo;
    let skip = args.flag("skip_text") || mode_skip;
    if [force, redo, skip].iter().filter(|b| **b).count() > 1 {
        return Err(OcrError::Other("BadArgsError: Choose only one of --force-ocr, --skip-text, --redo-ocr.".into()));
    }
    let mode = if force {
        ProcessingMode::Force
    } else if redo {
        ProcessingMode::Redo
    } else if skip {
        ProcessingMode::Skip
    } else {
        ProcessingMode::Default
    };
    let deskew = args.flag("deskew");
    let clean_final = args.flag("clean_final");
    if mode == ProcessingMode::Redo && (deskew || clean_final) {
        return Err(OcrError::Other(
            "BadArgsError: --redo-ocr is not currently compatible with --deskew, --clean-final, and --remove-background".into(),
        ));
    }
    let output_type = match args.str("output_type") {
        Some(s) => OutputType::parse(s)?,
        None => OutputType::PdfA,
    };
    let pages = match args.get("pages") {
        Some(Value::String(s)) => Some(parse_pages(s)?),
        Some(Value::Number(n)) => n.as_u64().map(|n| BTreeSet::from([n as usize])),
        _ => None,
    };
    let path = |k: &str| args.str(k).map(PathBuf::from);
    Ok(Options {
        input: path("input_file_or_options").ok_or_else(|| OcrError::Other("BadArgsError: no input file".into()))?,
        output: path("output_file").ok_or_else(|| OcrError::Other("BadArgsError: no output file".into()))?,
        sidecar: path("sidecar"),
        language: args.str("language").unwrap_or("eng").to_string(),
        output_type,
        mode,
        pages,
        image_dpi: args.f64("image_dpi"),
        rotate_pages: args.flag("rotate_pages"),
        rotate_threshold: args.f64("rotate_pages_threshold").unwrap_or(14.0),
        deskew,
        clean: args.flag("clean"),
        clean_final,
        max_image_mpixels: args.f64("max_image_mpixels").unwrap_or(DEFAULT_MAX_IMAGE_MPIXELS),
        invalidate_digital_signatures: args.flag("invalidate_digital_signatures"),
        oversample: args.f64("oversample").filter(|v| *v > 0.0),
        tesseract_timeout: args.f64("tesseract_timeout").filter(|v| *v > 0.0).map(Duration::from_secs_f64),
        skip_big: args.f64("skip_big").filter(|v| *v > 0.0),
        tagged_ignore: args.str("tagged_pdf_mode") == Some("ignore") || truthy(args.get("tagged_pdf_mode_ignore")),
        jobs: crate::pipeline::effective_jobs(args.get("jobs").and_then(Value::as_i64)),
        optimize: parse_optimize(args)?,
        color: match args.str("color_conversion_strategy") {
            Some(c) => crate::config::ColorConversion::parse(c)?,
            None => crate::config::ColorConversion::LeaveColorUnchanged,
        },
        title: args.str("title").map(str::to_string),
        author: args.str("author").map(str::to_string),
        subject: args.str("subject").map(str::to_string),
        keywords: args.str("keywords").map(str::to_string),
        tess: parse_tess(args)?,
        non_ocr_timeout: args.f64("tesseract_non_ocr_timeout").filter(|v| *v > 0.0).map(Duration::from_secs_f64),
        ocr_max_pixels: args.f64("max_ocr_image_mpixels").filter(|v| *v > 0.0),
        ocr_max_side: if args.flag("tesseract_downsample_large_images") {
            Some(args.f64("tesseract_downsample_above").filter(|v| *v >= 100.0).map_or(32767, |v| v as u32))
        } else {
            None
        },
        remove_vectors: args.flag("remove_vectors"),
        no_ocr: args.str("ocr_engine") == Some("none"),
        clean_filters: args.str("unpaper_args").map(|a| crate::preprocess::CleanFilters::from_unpaper_args(a).0).unwrap_or_default(),
    })
}

fn int_arg(args: &OcrArgs, k: &str, lo: i64, hi: i64) -> Result<Option<i64>> {
    let v = match args.get(k) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Number(n)) => n.as_f64().map(|f| f as i64),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        Some(Value::Bool(b)) => Some(i64::from(*b)),
        _ => None,
    };
    match v {
        Some(v) if (lo..=hi).contains(&v) => Ok(Some(v)),
        _ => Err(OcrError::Other(format!(
            "BadArgsError: invalid value for {k}: {}",
            args.get(k).map(ToString::to_string).unwrap_or_default()
        ))),
    }
}

/// `--optimize` (0–3, Standard 1), `--jpeg-quality`, `--png-quality` (0–100), `--pdfa-image-compression`.
pub fn parse_optimize(args: &OcrArgs) -> Result<crate::optimize::OptimizeOptions> {
    let level = int_arg(args, "optimize", 0, 3)?.unwrap_or(1) as u8;
    let jq = int_arg(args, "jpeg_quality", 0, 100)?.or(int_arg(args, "jpg_quality", 0, 100)?).unwrap_or(0) as u8;
    let pq = int_arg(args, "png_quality", 0, 100)?.unwrap_or(0) as u8;
    if level == 0 && (jq > 0 || pq > 0) {
        tracing::warn!("The arguments --jpeg-quality and --png-quality will be ignored because --optimize=0.");
    }
    let force_jpeg = match args.str("pdfa_image_compression") {
        None | Some("auto" | "lossless") => false,
        Some("jpeg") => true,
        Some(o) => return Err(OcrError::Other(format!("BadArgsError: invalid --pdfa-image-compression '{o}'"))),
    };
    Ok(crate::optimize::OptimizeOptions { level, jpeg_quality: jq, png_quality: pq, force_jpeg })
}

/// Tesseract options (OCRmyPDF `tesseract_*`, `user_words`, `user_patterns`).
pub fn parse_tess(args: &OcrArgs) -> Result<crate::engine::TessOptions> {
    let thresholding = match args.get("tesseract_thresholding") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => match s.as_str() {
            "auto" => None,
            "otsu" => Some(0),
            "adaptive-otsu" => Some(1),
            "sauvola" => Some(2),
            other => return Err(OcrError::Other(format!("BadArgsError: Invalid thresholding method '{other}'"))),
        },
        Some(_) => int_arg(args, "tesseract_thresholding", 0, 2)?.map(|v| v as u8),
    };
    let config = match args.get("tesseract_config") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    Ok(crate::engine::TessOptions {
        psm: int_arg(args, "tesseract_pagesegmode", 0, 13)?.map(|v| v as u8),
        oem: int_arg(args, "tesseract_oem", 0, 3)?.map(|v| v as u8),
        thresholding,
        config,
        user_words: args.str("user_words").map(PathBuf::from),
        user_patterns: args.str("user_patterns").map(PathBuf::from),
    })
}

/// Downscale the recognition image (`max_ocr_image_mpixels`, `tesseract_downsample_above`).
fn downsample_for_ocr(r: &Raster, max_pixels: Option<f64>, max_side: Option<u32>) -> Option<Raster> {
    let (w, h) = (f64::from(r.width), f64::from(r.height));
    let mut f: f64 = 1.0;
    if let Some(mp) = max_pixels
        && w * h > mp * 1e6
    {
        f = f.min((mp * 1e6 / (w * h)).sqrt());
    }
    if let Some(side) = max_side
        && w.max(h) > f64::from(side)
    {
        f = f.min(f64::from(side) / w.max(h));
    }
    if f >= 1.0 {
        return None;
    }
    let img = r.to_dynamic()?;
    let nw = ((w * f).floor() as u32).max(1);
    let nh = ((h * f).floor() as u32).max(1);
    let small = img.resize_exact(nw, nh, image::imageops::FilterType::Triangle);
    Some(Raster::from_dynamic(&small, r.dpi * f))
}

/// `merge_sidecars`: `None` = page skipped.
pub fn merge_sidecars(pages: &[Option<String>]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < pages.len() {
        let from = i + 1;
        if from != 1 {
            out.push('\x0c');
        }
        match &pages[i] {
            Some(t) => {
                out.push_str(t.strip_suffix('\x0c').unwrap_or(t));
                i += 1;
            }
            None => {
                let mut j = i;
                while j + 1 < pages.len() && pages[j + 1].is_none() {
                    j += 1;
                }
                let to = j + 1;
                if from == to {
                    out.push_str(&format!("[OCR skipped on page(s) {from}]"));
                } else {
                    out.push_str(&format!("[OCR skipped on page(s) {from}-{to}]"));
                }
                i = j + 1;
            }
        }
    }
    out
}

fn is_pdf(data: &[u8]) -> bool {
    data.windows(5).take(1024).any(|w| w == b"%PDF-")
}

fn pixel_limit(mp: f64) -> Option<u64> {
    if mp == 0.0 { None } else { Some((mp * 1_000_000.0) as u64) }
}

// ----------------------------------------------------------------------------------------------
// Decode images
// ----------------------------------------------------------------------------------------------

/// All single images (TIFF: all pages; otherwise first image).
pub fn decode_frames(data: &[u8], info: &ImageInfo) -> Result<Vec<image::DynamicImage>> {
    let err = |e: &dyn std::fmt::Display| OcrError::Other(format!("UnidentifiedImageError: {e}"));
    match info.format {
        ImageFormat::Heic => {
            Err(OcrError::Other("UnidentifiedImageError: HEIC decoding is not supported (no HEVC decoder available)".into()))
        }
        ImageFormat::Tiff => decode_tiff_frames(data).map_err(|e| err(&e)),
        _ => {
            let img = image::load_from_memory(data).map_err(|e| err(&e))?;
            Ok(vec![img])
        }
    }
}

fn decode_tiff_frames(data: &[u8]) -> std::result::Result<Vec<image::DynamicImage>, String> {
    use tiff::ColorType as C;
    use tiff::decoder::{Decoder, DecodingResult};
    let mut dec = Decoder::new(Cursor::new(data)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    loop {
        let (w, h) = dec.dimensions().map_err(|e| e.to_string())?;
        let ct = dec.colortype().map_err(|e| e.to_string())?;
        let img = dec.read_image().map_err(|e| e.to_string())?;
        let dynimg = match (ct, img) {
            (C::Gray(8), DecodingResult::U8(v)) => image::GrayImage::from_raw(w, h, v).map(image::DynamicImage::ImageLuma8),
            (C::Gray(1), DecodingResult::U8(v)) => {
                // 1 bit, rows padded to bytes; WhiteIsZero is not inverted by the decoder
                let row = w.div_ceil(8) as usize;
                let mut px = Vec::with_capacity((w * h) as usize);
                for y in 0..h as usize {
                    for x in 0..w as usize {
                        let b = v.get(y * row + x / 8).copied().unwrap_or(0);
                        px.push(if b & (0x80 >> (x % 8)) != 0 { 255 } else { 0 });
                    }
                }
                image::GrayImage::from_raw(w, h, px).map(image::DynamicImage::ImageLuma8)
            }
            (C::Gray(16), DecodingResult::U16(v)) => image::ImageBuffer::from_raw(w, h, v).map(image::DynamicImage::ImageLuma16),
            (C::GrayA(8), DecodingResult::U8(v)) => image::GrayAlphaImage::from_raw(w, h, v).map(image::DynamicImage::ImageLumaA8),
            (C::RGB(8), DecodingResult::U8(v)) => image::RgbImage::from_raw(w, h, v).map(image::DynamicImage::ImageRgb8),
            (C::RGBA(8), DecodingResult::U8(v)) => image::RgbaImage::from_raw(w, h, v).map(image::DynamicImage::ImageRgba8),
            (C::RGB(16), DecodingResult::U16(v)) => image::ImageBuffer::from_raw(w, h, v).map(image::DynamicImage::ImageRgb16),
            (C::RGBA(16), DecodingResult::U16(v)) => image::ImageBuffer::from_raw(w, h, v).map(image::DynamicImage::ImageRgba16),
            (C::CMYK(8), DecodingResult::U8(v)) => {
                let rgb: Vec<u8> = v
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| {
                        let k = 255 - u16::from(p[3]);
                        [p[0], p[1], p[2]].map(|c| ((255 - u16::from(c)) * k / 255) as u8)
                    })
                    .collect();
                image::RgbImage::from_raw(w, h, rgb).map(image::DynamicImage::ImageRgb8)
            }
            (C::Multiband { bit_depth: 8, num_samples: 2 }, DecodingResult::U8(v)) => {
                image::GrayAlphaImage::from_raw(w, h, v).map(image::DynamicImage::ImageLumaA8)
            }
            (C::Multiband { bit_depth: 8, num_samples: 3 }, DecodingResult::U8(v)) => {
                image::RgbImage::from_raw(w, h, v).map(image::DynamicImage::ImageRgb8)
            }
            (C::Multiband { bit_depth: 8, num_samples: 4 }, DecodingResult::U8(v)) => {
                image::RgbaImage::from_raw(w, h, v).map(image::DynamicImage::ImageRgba8)
            }
            (ct, _) => return Err(format!("unsupported TIFF color type {ct:?}")),
        };
        out.push(dynimg.ok_or("TIFF buffer size mismatch")?);
        if !dec.more_images() {
            break;
        }
        dec.next_image().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// EXIF orientation → clockwise rotation (img2pdf `Rotation.ifvalid`; mirroring ignored).
pub fn exif_rotation(o: Option<u16>) -> u16 {
    match o {
        Some(3) => 180,
        Some(6) => 90,
        Some(8) => 270,
        _ => 0,
    }
}

/// [`ImageOps`] on the file system (Pillow semantics, see `imageinfo`).
pub struct FsImageOps {
    pub tempdir: PathBuf,
    /// `maybe_override_pixel_limit` (from `settings.MAX_IMAGE_PIXELS`).
    pub pixel_limit: Option<u64>,
}

impl FsImageOps {
    pub fn new(cfg: &OcrConfig, tempdir: &Path) -> Self {
        Self { tempdir: tempdir.to_path_buf(), pixel_limit: imageinfo::pil_pixel_limit(cfg.max_image_pixels_setting) }
    }
    fn open(&self, path: &str) -> Result<ImageInfo> {
        let i = imageinfo::probe_path(Path::new(path))?;
        i.decompression_bomb_check(self.pixel_limit)?;
        Ok(i)
    }
}

impl ImageOps for FsImageOps {
    fn get_dpi(&self, path: &str) -> Option<i64> {
        match self.open(path) {
            Ok(i) => i.get_dpi().or_else(|| {
                tracing::warn!("Error while getting DPI from image {path}: 'dpi'");
                None
            }),
            Err(e) => {
                tracing::warn!("Error while getting DPI from image {path}: {}", e.message());
                None
            }
        }
    }
    fn calculate_a4_dpi(&self, path: &str) -> Option<i64> {
        match self.open(path) {
            Ok(i) => {
                let d = i.calculate_a4_dpi();
                tracing::debug!("Estimated DPI {d} based on image width {}", i.width);
                Some(d)
            }
            Err(e) => {
                tracing::warn!("Error while calculating DPI for image {path}: {}", e.message());
                None
            }
        }
    }
    fn has_alpha(&self, path: &str) -> Result<bool> {
        Ok(self.open(path)?.has_alpha)
    }
    fn remove_alpha(&self, path: &str) -> Result<String> {
        // `remove_alpha_channel`: discard alpha (do not blend), all single images.
        let data = std::fs::read(path)?;
        let info = imageinfo::probe(&data)?;
        let frames = decode_frames(&data, &info)?;
        let dest = self.tempdir.join("image-no-alpha");
        let rasters: Vec<Raster> = frames.iter().map(|f| Raster::from_dynamic(f, 0.0)).collect();
        write_frames(&dest, &rasters)?;
        Ok(dest.to_string_lossy().into_owned())
    }
}

/// Write single images as PNG (one) or multi-page TIFF.
fn write_frames(dest: &Path, frames: &[Raster]) -> Result<()> {
    let ioerr = |e: &dyn std::fmt::Display| OcrError::Other(format!("OSError: {e}"));
    if frames.len() == 1 {
        let img = frames[0].to_dynamic().ok_or_else(|| ioerr(&"bad raster"))?;
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png).map_err(|e| ioerr(&e))?;
        std::fs::write(dest, buf)?;
        return Ok(());
    }
    let mut buf = Cursor::new(Vec::new());
    {
        let mut enc = tiff::encoder::TiffEncoder::new(&mut buf).map_err(|e| ioerr(&e))?;
        for f in frames {
            let r = if f.channels == 1 {
                enc.write_image::<tiff::encoder::colortype::Gray8>(f.width, f.height, &f.data)
            } else {
                enc.write_image::<tiff::encoder::colortype::RGB8>(f.width, f.height, &f.data)
            };
            r.map_err(|e| ioerr(&e))?;
        }
    }
    std::fs::write(dest, buf.into_inner())?;
    Ok(())
}

// ----------------------------------------------------------------------------------------------
// Flow
// ----------------------------------------------------------------------------------------------

/// Decision per PDF page (`is_ocr_required`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagePlan {
    /// Do not touch (`--pages`, skip_text with text, vector page without images).
    Skip,
    /// OCR, text layer over the content.
    Overlay,
    /// OCR, first remove the old invisible text layer (redo).
    Redo,
    /// Rasterize and replace the page (force).
    Rasterize,
}

/// OCRmyPDF `is_ocr_required` (1-based page number).
pub fn plan_page(opts: &Options, pageno: usize, a: &pdfwrite::PageAnalysis) -> Result<PagePlan> {
    if let Some(p) = &opts.pages
        && !p.contains(&pageno)
    {
        return Ok(PagePlan::Skip);
    }
    if a.has_text {
        return match opts.mode {
            ProcessingMode::Default => Err(OcrError::PriorOcrFound(
                "page already has text! - aborting (use --force-ocr or --mode force to force OCR; see also help for --skip-text, --redo-ocr, and --mode)".into(),
            )),
            ProcessingMode::Force => Ok(PagePlan::Rasterize),
            ProcessingMode::Redo => Ok(PagePlan::Redo),
            ProcessingMode::Skip => Ok(PagePlan::Skip),
        };
    }
    if !a.has_images && !opts.lossless_reconstruction() {
        return Ok(if opts.mode == ProcessingMode::Force { PagePlan::Rasterize } else { PagePlan::Skip });
    }
    Ok(match opts.mode {
        ProcessingMode::Force => PagePlan::Rasterize,
        ProcessingMode::Redo => PagePlan::Redo,
        _ => PagePlan::Overlay,
    })
}

/// OCRmyPDF `get_page_square_dpi`.
pub fn page_dpi(opts: &Options, a: &pdfwrite::PageAnalysis) -> f64 {
    let img = a.image_dpi.filter(|d| *d > 0.0).unwrap_or(VECTOR_PAGE_DPI);
    let vec = if a.has_vector || a.has_text { VECTOR_PAGE_DPI } else { 0.0 };
    img.max(vec).max(opts.oversample.unwrap_or(0.0))
}

impl NativeOcrmypdf {
    pub fn new(engine: Option<Box<dyn OcrEngine>>, renderer: Box<dyn PdfRenderer>, workdir: PathBuf) -> Self {
        Self { engine, renderer, meta: OutputMeta::default(), max_render_dpi: 600.0, workdir }
    }

    fn engine(&self, languages: &str, checked: &mut bool) -> Result<&dyn OcrEngine> {
        let e = self.engine.as_deref().ok_or_else(|| OcrError::Unavailable("no OCR engine configured (tesseract not found)".into()))?;
        if !*checked {
            check_languages(e, languages)?;
            *checked = true;
        }
        Ok(e)
    }

    /// OCR a raster; with `rotate_pages` check the orientation first, then straighten/clean
    /// (OCRmyPDF order). `ocr_gray`: recognition on the gray image (PDF pages that are not
    /// replaced by a raster – like the PDFium gray path so far).
    ///
    /// With `jobs > 1` runs in worker threads (engine/file system only, no PDFium); the engine
    /// was fetched and checked beforehand on the caller thread ([`Self::engine`]).
    fn ocr_raster(&self, engine: &dyn OcrEngine, opts: &Options, raster: Raster, name: &str, ocr_gray: bool) -> Result<RasterResult> {
        std::fs::create_dir_all(&self.workdir)?;
        // Image file only if the engine needs one (the engine writes it on demand).
        let path = self.workdir.join(format!("{name}.{}", engine.raster_file_ext()));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        let result = (|| {
            let mut raster = raster;
            let mut rot = 0;
            let as_gray = |r: &Raster| -> Raster { crate::preprocess::to_gray(r) };
            let gray_needed = |r: &Raster| ocr_gray && r.channels != 1;
            let orientation = if !opts.rotate_pages {
                None
            } else if gray_needed(&raster) {
                engine.orientation_raster(&as_gray(&raster), &path, opts.non_ocr_timeout.or(opts.tesseract_timeout))?
            } else {
                engine.orientation_raster(&raster, &path, opts.non_ocr_timeout.or(opts.tesseract_timeout))?
            };
            if let Some(o) = orientation {
                if o.rotate != 0 && o.confidence >= opts.rotate_threshold {
                    tracing::info!("{name}: page is facing ⇒ rotating {} degrees (confidence {})", o.rotate, o.confidence);
                    rot = o.rotate;
                    raster = raster.rotate_cw(rot);
                    // File of the unrotated image must go, otherwise the engine recognizes the old image.
                    if path.exists() {
                        std::fs::remove_file(&path)?;
                    }
                } else if o.rotate != 0 {
                    tracing::info!("{name}: rotation {} below threshold ({} < {})", o.rotate, o.confidence, opts.rotate_threshold);
                }
            }
            let steps = opts.steps();
            let prep = crate::preprocess::prepare(raster, steps);
            if let Some(s) = prep.skew
                && s.angle.abs() >= crate::preprocess::MIN_DESKEW_DEGREES
            {
                tracing::info!("{name}: deskewing by {:.2} degrees", s.angle);
            }
            if let Some(c) = &prep.clean
                && c.skipped_photo
            {
                tracing::info!("{name}: page looks like a photo (grey paper); not cleaned");
            } else if let Some(c) = &prep.clean {
                tracing::debug!(
                    "{name}: cleaned (border {} px, holes {}, gray tiles {}, specks {})",
                    c.border_pixels,
                    c.holes,
                    c.gray_tiles,
                    c.specks
                );
            }
            if steps.any() {
                tracing::debug!("{name}: preprocessing took deskew {:?}, clean {:?}", prep.deskew_time, prep.clean_time);
            }
            // recognition image differs from the image of the orientation check → old file gone.
            if (prep.ocr.is_some() || prep.visible_changed) && path.exists() {
                std::fs::remove_file(&path)?;
            }
            let ocr_img: Cow<'_, Raster> = match &prep.ocr {
                Some(g) => Cow::Borrowed(g),
                None if gray_needed(&prep.visible) => Cow::Owned(as_gray(&prep.visible)),
                None => Cow::Borrowed(&prep.visible),
            };
            let ocr_img = match downsample_for_ocr(&ocr_img, opts.ocr_max_pixels, opts.ocr_max_side) {
                Some(small) => {
                    tracing::info!("{name}: OCR image downsampled to {}x{}", small.width, small.height);
                    if path.exists() {
                        std::fs::remove_file(&path)?;
                    }
                    Cow::Owned(small)
                }
                None => ocr_img,
            };
            let req = OcrRequest {
                languages: opts.language.clone(),
                dpi: Some(ocr_img.dpi.round() as u32),
                timeout: opts.tesseract_timeout,
                tess: opts.tess.clone(),
            };
            let ocr = engine.recognize_raster(&ocr_img, &path, &req)?;
            drop(ocr_img);
            Ok(RasterResult { rotate: rot, visible: prep.visible, visible_changed: prep.visible_changed, ocr })
        })();
        let _ = std::fs::remove_file(&path);
        result
    }

    /// `ocrmypdf.ocr(**args)`.
    pub fn run(&self, args: &OcrArgs) -> Result<()> {
        let mut opts = parse_options(args)?;
        // Built-in default `deu+eng` without German language data (own builds): use `eng` instead of
        // failing – installations that never set OCR_LANGUAGE keep working as before.
        if opts.language == crate::config::DEFAULT_OCR_LANGUAGE
            && let Some(e) = self.engine.as_deref()
            && e.check_languages(&opts.language).is_err()
            && e.check_languages(crate::config::DEFAULT_OCR_LANGUAGE_FALLBACK).is_ok()
        {
            tracing::warn!(
                "OCR language data for '{}' incomplete; using '{}' (install the German data or set PAPERLESS_OCR_LANGUAGE)",
                crate::config::DEFAULT_OCR_LANGUAGE,
                crate::config::DEFAULT_OCR_LANGUAGE_FALLBACK
            );
            opts.language = crate::config::DEFAULT_OCR_LANGUAGE_FALLBACK.to_string();
        }
        let data = std::fs::read(&opts.input).map_err(|e| OcrError::InputFile(format!("{}: {e}", opts.input.display())))?;
        let meta = OutputMeta {
            output_type: opts.output_type,
            color: opts.color,
            optimize: opts.optimize,
            title: opts.title.clone(),
            author: opts.author.clone(),
            subject: opts.subject.clone(),
            keywords: opts.keywords.clone(),
            ..self.meta.clone()
        };
        if !opts.tess.is_default()
            && let Some(e) = self.engine.as_deref()
            && !e.name().contains("tesseract")
        {
            tracing::warn!("Tesseract options from OCR_USER_ARGS have no effect with OCR engine {}", e.name());
        }
        let (pdf, sidecar) = if is_pdf(&data) { self.run_pdf(&opts, &data, &meta)? } else { self.run_image(&opts, &data, &meta)? };
        std::fs::write(&opts.output, pdf)?;
        if let Some(sc) = &opts.sidecar {
            std::fs::write(sc, merge_sidecars(&sidecar))?;
        }
        Ok(())
    }

    fn run_image(&self, opts: &Options, data: &[u8], meta: &OutputMeta) -> Result<(Vec<u8>, Vec<Option<String>>)> {
        let info = imageinfo::probe(data).map_err(|_| OcrError::InputFile("UnsupportedImageFormatError".into()))?;
        info.decompression_bomb_check(pixel_limit(opts.max_image_mpixels))?;
        // triage_image_file
        let dpi = match (opts.image_dpi, info.dpi) {
            (Some(d), _) => (d, d),
            (None, Some((x, y))) if x > 96.0 || y > 96.0 => (x, y),
            (None, Some(_)) => {
                return Err(OcrError::InputFile(
                    "DpiError: Input file is an image, but the resolution (DPI) is not credible.  Estimate the resolution at which the image was scanned and specify it using --image-dpi.".into(),
                ));
            }
            (None, None) => {
                return Err(OcrError::InputFile(
                    "DpiError: Input file is an image, but has no resolution (DPI) in its metadata.  Estimate the resolution at which image was scanned and specify it using --image-dpi.".into(),
                ));
            }
        };
        if info.has_alpha {
            return Err(OcrError::InputFile(
                "UnsupportedImageFormatError: The input image has an alpha channel. Remove the alpha channel first.".into(),
            ));
        }
        let frames = decode_frames(data, &info)?;
        let exif_rot = if info.format == ImageFormat::Jpeg { exif_rotation(info.exif_orientation) } else { 0 };
        let mut checked = false;
        // Preparation per single image serial (order as before), OCR parallel (`jobs`).
        let mut prepared: Vec<PageImage> = Vec::with_capacity(frames.len());
        let ocrs = crate::pipeline::ordered(
            opts.jobs,
            frames.len(),
            |i| {
                let pageno = i + 1;
                let raster = Raster::from_dynamic(&frames[i], dpi.0);
                let image = if info.format == ImageFormat::Jpeg && frames.len() == 1 && matches!(info.mode, "L" | "RGB") {
                    PageImage::Jpeg { data: data.to_vec(), width: raster.width, height: raster.height, components: raster.channels }
                } else {
                    PageImage::Raw(raster.clone())
                };
                let selected = !opts.no_ocr && opts.pages.as_ref().is_none_or(|p| p.contains(&pageno));
                let big = opts.skip_big.is_some_and(|mp| f64::from(raster.width) * f64::from(raster.height) > mp * 1e6);
                prepared.push(image);
                if selected && !big {
                    let engine = self.engine(&opts.language, &mut checked)?;
                    Ok(Some((engine, raster.rotate_cw(exif_rot))))
                } else {
                    Ok(None)
                }
            },
            // OCR on our own grayscale (ITU-R 601 luma) like PDF pages, not on color: Tesseract's
            // internal color→gray weighting made photos fragile (Jetson check 2026-11-21: deskewed
            // phone photo lost the umlaut dots of one word on color input, gray input was stable at
            // every angle). Visible/archive image stays in color; gray is also less data to recognize.
            |i, (engine, shown)| self.ocr_raster(engine, opts, shown, &format!("page-{:04}", i + 1), true),
        )?;
        let mut specs = Vec::with_capacity(frames.len());
        let mut sidecar = Vec::with_capacity(frames.len());
        for (image, r) in prepared.into_iter().zip(ocrs) {
            match r {
                Some(res) if res.visible_changed => {
                    // Straightened/cleaned: the displayed (rotated) image replaces the original,
                    // without `/Rotate` (OCRmyPDF: visible page image from preprocessing).
                    sidecar.push(Some(res.ocr.text.clone()));
                    let turned = (exif_rot + res.rotate) % 180 == 90;
                    let (dx, dy) = if turned { (dpi.1, dpi.0) } else { dpi };
                    let image = visible_page_image(res.visible, info.format == ImageFormat::Jpeg);
                    specs.push(ImagePageSpec { image, dpi_x: dx, dpi_y: dy, rotate: 0, ocr: Some(res.ocr) });
                }
                Some(res) => {
                    sidecar.push(Some(res.ocr.text.clone()));
                    // OCR coordinates refer to the image rotated by `exif_rot + rot`.
                    let rotate = (exif_rot + res.rotate) % 360;
                    specs.push(ImagePageSpec { image, dpi_x: dpi.0, dpi_y: dpi.1, rotate, ocr: Some(res.ocr) });
                }
                None => {
                    sidecar.push(None);
                    specs.push(ImagePageSpec { image, dpi_x: dpi.0, dpi_y: dpi.1, rotate: exif_rot, ocr: None });
                }
            }
        }
        Ok((pdfwrite::build_from_images(&specs, meta)?, sidecar))
    }

    fn run_pdf(&self, opts: &Options, data: &[u8], meta: &OutputMeta) -> Result<(Vec<u8>, Vec<Option<String>>)> {
        let doc = pdfwrite::load_pdf_for_ocr(data)?;
        let info = pdfwrite::doc_info(&doc);
        if info.needs_rendering {
            return Err(OcrError::InputFile(
                "This PDF contains dynamic XFA forms created by Adobe LiveCycle Designer and can only be read by Adobe Acrobat or Adobe Reader.".into(),
            ));
        }
        if info.has_signature {
            if opts.invalidate_digital_signatures {
                tracing::warn!("All digital signatures will be invalidated");
            } else {
                return Err(OcrError::DigitalSignature);
            }
        }
        if info.has_acroform {
            if opts.mode == ProcessingMode::Redo {
                return Err(OcrError::InputFile(
                    "This PDF has a user fillable form. --redo-ocr (or --mode redo) is not currently possible on such files.".into(),
                ));
            }
            tracing::warn!("This PDF has a fillable form. Chances are it is a pure digital document that does not need OCR.");
        }
        if (info.is_tagged || info.has_structure_tree) && opts.mode == ProcessingMode::Default && !opts.tagged_ignore {
            return Err(OcrError::InputFile(
                "This PDF is marked as a Tagged PDF. This often indicates that the PDF was generated from an office document and does not need OCR. Use --force-ocr, --skip-text or --redo-ocr to override this error.".into(),
            ));
        }
        let page_ids: Vec<_> = doc.get_pages().values().copied().collect();
        // 1. Plan (PriorOcrFoundError before any work)
        let mut plans = Vec::with_capacity(page_ids.len());
        for (i, pid) in page_ids.iter().enumerate() {
            let a = pdfwrite::analyze_page(&doc, *pid);
            let mut plan = if opts.no_ocr { PagePlan::Skip } else { plan_page(opts, i + 1, &a)? };
            if plan != PagePlan::Skip
                && let Some(mp) = opts.skip_big
            {
                let (w, h) = pdfwrite::display_size(pdfwrite::display_box(&doc, *pid), pdfwrite::page_rotation(&doc, *pid));
                let d = page_dpi(opts, &a);
                if a.has_images && (w * d / 72.0) * (h * d / 72.0) > mp * 1e6 {
                    tracing::warn!("page too big, skipping OCR");
                    plan = PagePlan::Skip;
                }
            }
            plans.push((plan, a));
        }
        // 2. For redo: rasterize a copy without any text (OCRmyPDF hides existing text).
        // `remove_vectors`: remove vector graphics for the recognition image (overlay/redo pages).
        let vec_mask = |p: PagePlan| opts.remove_vectors && matches!(p, PagePlan::Overlay | PagePlan::Redo);
        let masked: Option<Vec<u8>> = if plans.iter().any(|(p, _)| *p == PagePlan::Redo || vec_mask(*p)) {
            let mut m = doc.clone();
            for ((p, _), pid) in plans.iter().zip(&page_ids) {
                if *p == PagePlan::Redo {
                    pdfwrite::strip_page_text(&mut m, *pid, false)?;
                }
                if vec_mask(*p) {
                    pdfwrite::strip_page_vectors(&mut m, *pid)?;
                }
            }
            let mut buf = Vec::new();
            m.save_to(&mut buf).map_err(|e| OcrError::Other(format!("PDF write failed: {e}")))?;
            Some(buf)
        } else {
            None
        };
        let need_render = plans.iter().any(|(p, _)| *p != PagePlan::Skip);
        let rdoc = if need_render { Some(self.renderer.open(data)?) } else { None };
        let mdoc = match &masked {
            Some(m) => Some(self.renderer.open(m)?),
            None => None,
        };
        let limit = pixel_limit(opts.max_image_mpixels);
        let mut checked = false;
        // Rasterizing (PDFium, not thread-safe) serially on this thread, OCR parallel (`jobs`);
        // results in page order, first error in page order wins.
        let results = crate::pipeline::ordered(
            opts.jobs,
            plans.len(),
            |i| {
                let (plan, a) = &plans[i];
                if *plan == PagePlan::Skip {
                    return Ok(None);
                }
                let want = page_dpi(opts, a).min(self.max_render_dpi);
                let src = if *plan == PagePlan::Redo || vec_mask(*plan) { mdoc.as_deref() } else { rdoc.as_deref() };
                let Some(src) = src else { return Err(OcrError::Other("renderer missing".into())) };
                let (wpt, hpt) = src.page_size_pt(i)?;
                // PaperRust (C-017): rasterize very large pages at a lower resolution so that the
                // pixel count stays below the limit; rejected only if less than
                // MIN_FIT_DPI would be needed for that (then the image is no longer suitable for OCR).
                let cap = limit.unwrap_or(crate::render::MAX_RENDER_PIXELS).min(crate::render::MAX_RENDER_PIXELS);
                let fitted = crate::render::fit_dpi(wpt, hpt, want, cap);
                let dpi = if fitted < want && fitted >= MIN_FIT_DPI {
                    tracing::warn!("Page {} is very large; OCR renders it at {fitted:.2} dpi instead of {want} dpi", i + 1);
                    fitted
                } else {
                    want
                };
                if let Some(l) = limit {
                    let (pw, ph) = crate::render::page_pixels(wpt, hpt, dpi);
                    let px = pw * ph;
                    if px > 2.0 * l as f64 {
                        return Err(OcrError::DecompressionBomb(format!(
                            "Image size ({} pixels) exceeds limit of {} pixels, could be decompression bomb DOS attack.",
                            px as u64,
                            2 * l
                        )));
                    }
                }
                // Can the page be replaced by the preprocessed image (deskew/clean-final),
                // rasterize in color; recognition still gets the gray image.
                let gray = *plan == PagePlan::Redo || (*plan == PagePlan::Overlay && !opts.steps().may_change_visible());
                let raster = src.render(i, dpi, gray)?;
                let engine = self.engine(&opts.language, &mut checked)?;
                Ok(Some((engine, raster)))
            },
            |i, (engine, raster)| {
                let ocr_gray = plans[i].0 != PagePlan::Rasterize;
                self.ocr_raster(engine, opts, raster, &format!("page-{:04}", i + 1), ocr_gray)
            },
        )?;
        let mut edits = Vec::with_capacity(plans.len());
        let mut sidecar = Vec::with_capacity(plans.len());
        for ((plan, a), r) in plans.iter().zip(results) {
            let Some(RasterResult { rotate: rot, visible, visible_changed, ocr }) = r else {
                edits.push(PageEdit { action: PageAction::Keep, rotate_add: 0 });
                sidecar.push(None);
                continue;
            };
            sidecar.push(Some(ocr.text.clone()));
            let action = match plan {
                // Raster is RGB and already in the new display orientation (incl. `rot`).
                PagePlan::Rasterize => PageAction::Replace { image: PageImage::Raw(visible), ocr },
                PagePlan::Redo => PageAction::Overlay { ocr, strip_old_ocr: true },
                // deskew/clean-final (not lossless): replace the page with the preprocessed image
                // (OCRmyPDF: visible page image). Unchanged → keep the original.
                _ if visible_changed => {
                    let image = visible_page_image(crate::preprocess::reduce_if_neutral(visible), a.all_images_jpeg());
                    PageAction::Replace { image, ocr }
                }
                _ => PageAction::Overlay { ocr, strip_old_ocr: false },
            };
            edits.push(PageEdit { action, rotate_add: rot });
        }
        drop(rdoc);
        drop(mdoc);
        Ok((pdfwrite::build_from_pdf(data, &edits, meta)?, sidecar))
    }

    /// `_convert_image_to_pdfa`: img2pdf (+ PDF/A-2b stamp), without OCR.
    pub fn image_to_pdfa(&self, image: &Path, image_dpi: Option<i64>, tempdir: &Path) -> Result<PathBuf> {
        let wrap = |m: String| OcrError::Parse(format!("img2pdf conversion failed for {}: {m}", image.display()));
        let data = std::fs::read(image).map_err(|e| wrap(e.to_string()))?;
        let info = imageinfo::probe(&data).map_err(|e| wrap(e.message()))?;
        if info.has_alpha {
            // img2pdf: AlphaChannelError
            return Err(wrap("Refusing to work on images with alpha channel".into()));
        }
        let frames = decode_frames(&data, &info).map_err(|e| wrap(e.message()))?;
        // img2pdf: fixed DPI from OCR_IMAGE_DPI, otherwise image DPI, otherwise 96
        let (dx, dy) = match (image_dpi, info.dpi) {
            (Some(d), _) => (d as f64, d as f64),
            (None, Some((x, y))) if x > 0.0 && y > 0.0 => (x, y),
            _ => (96.0, 96.0),
        };
        let rot = if info.format == ImageFormat::Jpeg { exif_rotation(info.exif_orientation) } else { 0 };
        let specs: Vec<ImagePageSpec> = frames
            .iter()
            .map(|f| {
                let r = Raster::from_dynamic(f, dx);
                let image = if info.format == ImageFormat::Jpeg && frames.len() == 1 && matches!(info.mode, "L" | "RGB") {
                    PageImage::Jpeg { data: data.clone(), width: r.width, height: r.height, components: r.channels }
                } else {
                    PageImage::Raw(r)
                };
                ImagePageSpec { image, dpi_x: dx, dpi_y: dy, rotate: rot, ocr: None }
            })
            .collect();
        // img2pdf does not optimize; color conversion like the PDF/A output of OCRmyPDF.
        let meta = OutputMeta { output_type: OutputType::PdfA2, optimize: crate::optimize::OptimizeOptions::none(), ..self.meta.clone() };
        let pdf = pdfwrite::build_from_images(&specs, &meta).map_err(|e| wrap(e.message()))?;
        let out = tempdir.join("archive.pdf");
        std::fs::create_dir_all(tempdir)?;
        std::fs::write(&out, pdf)?;
        Ok(out)
    }
}

impl OcrBackend for NativeOcrmypdf {
    fn extract_pdf_text(&self, pdf: &Path) -> Option<String> {
        // E14: like upstream `pdftotext -layout` (reading order via text positions, pl-parsers
        // `pdftext`); if pl-parsers cannot load the file, the simple text via lopdf.
        let t = pl_parsers::pdf::extract_pdf_text(pdf, &pl_parsers::ParseContext::default()).or_else(|| {
            let data = std::fs::read(pdf).ok()?;
            pdfwrite::extract_text(&data)
        });
        if t.is_none() {
            tracing::warn!("Error while getting text from PDF document");
        }
        t
    }

    fn is_tagged_pdf(&self, pdf: &Path) -> bool {
        std::fs::read(pdf).ok().and_then(|d| pdfwrite::load_pdf(&d).ok()).is_some_and(|d| pdfwrite::doc_info(&d).is_tagged)
    }

    fn ocr(&self, args: &OcrArgs) -> Result<()> {
        self.run(args)
    }

    fn convert_image_to_pdfa(&self, image: &Path, cfg: &OcrConfig, tempdir: &Path) -> Result<PathBuf> {
        self.image_to_pdfa(image, cfg.image_dpi, tempdir)
    }

    fn convert_pdf_to_pdfa(&self, input: &Path, output: &Path, cfg: &OcrConfig) -> Result<()> {
        if cfg.output_type == OutputType::Pdf {
            std::fs::copy(input, output)?;
            return Ok(());
        }
        let data = std::fs::read(input)?;
        // `_convert_pdf_to_pdfa`: Ghostscript with `color_conversion_strategy` (without OCRmyPDF optimization,
        // Ghostscript compresses itself → here lossless level 1).
        let meta = OutputMeta { output_type: cfg.output_type, color: cfg.color_conversion_strategy, ..self.meta.clone() };
        std::fs::write(output, pdfwrite::stamp_pdf(&data, &meta)?)?;
        Ok(())
    }
}

/// Convenient entry point: `RasterisedDocumentParser.parse` with the native backend.
pub fn parse_document(
    backend: &NativeOcrmypdf,
    cfg: &OcrConfig,
    tempdir: &Path,
    document: &Path,
    mime_type: &str,
    produce_archive: bool,
) -> Result<crate::ParseOutcome> {
    std::fs::create_dir_all(tempdir)?;
    let images = FsImageOps::new(cfg, tempdir);
    let p = crate::RasterisedParser { settings: cfg.clone(), tempdir: tempdir.to_path_buf(), backend, images: &images };
    p.parse(document, mime_type, produce_archive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdfwrite::PageAnalysis;

    #[test]
    fn sidecar_merge_like_ocrmypdf() {
        let pages = vec![Some("p1\x0c".to_string()), Some("p2".into()), Some("p3".into()), None, None, None];
        assert_eq!(merge_sidecars(&pages), "p1\x0cp2\x0cp3\x0c[OCR skipped on page(s) 4-6]");
        assert_eq!(merge_sidecars(&[None, Some("x".into())]), "[OCR skipped on page(s) 1]\x0cx");
        assert_eq!(merge_sidecars(&[None, None]), "[OCR skipped on page(s) 1-2]");
    }

    #[test]
    fn pages_expr() {
        assert_eq!(parse_pages("1-3").unwrap(), BTreeSet::from([1, 2, 3]));
        assert_eq!(parse_pages("1,4-5").unwrap(), BTreeSet::from([1, 4, 5]));
        assert!(parse_pages("0").is_err());
        assert!(parse_pages("3-1").is_err());
    }

    fn opts(mode: ProcessingMode, deskew: bool) -> Options {
        Options {
            input: "i".into(),
            output: "o".into(),
            sidecar: None,
            language: "eng".into(),
            output_type: OutputType::PdfA,
            mode,
            pages: None,
            image_dpi: None,
            rotate_pages: false,
            rotate_threshold: 12.0,
            deskew,
            clean: false,
            clean_final: false,
            max_image_mpixels: 250.0,
            invalidate_digital_signatures: false,
            oversample: None,
            tesseract_timeout: None,
            skip_big: None,
            tagged_ignore: false,
            jobs: 1,
            optimize: crate::optimize::OptimizeOptions::default(),
            color: crate::config::ColorConversion::Rgb,
            title: None,
            author: None,
            subject: None,
            keywords: None,
            tess: Default::default(),
            non_ocr_timeout: None,
            ocr_max_pixels: None,
            ocr_max_side: None,
            remove_vectors: false,
            no_ocr: false,
            clean_filters: Default::default(),
        }
    }

    #[test]
    fn page_plans_like_is_ocr_required() {
        let text = PageAnalysis { has_text: true, has_images: false, has_vector: true, image_dpi: None, ..PageAnalysis::default() };
        let img = PageAnalysis { has_text: false, has_images: true, has_vector: false, image_dpi: Some(300.0), ..PageAnalysis::default() };
        let vector = PageAnalysis { has_text: false, has_images: false, has_vector: true, image_dpi: None, ..PageAnalysis::default() };
        use ProcessingMode::*;
        assert!(matches!(plan_page(&opts(Default, true), 1, &text), Err(OcrError::PriorOcrFound(_))));
        assert_eq!(plan_page(&opts(Skip, true), 1, &text).unwrap(), PagePlan::Skip);
        assert_eq!(plan_page(&opts(Redo, false), 1, &text).unwrap(), PagePlan::Redo);
        assert_eq!(plan_page(&opts(Force, true), 1, &text).unwrap(), PagePlan::Rasterize);
        assert_eq!(plan_page(&opts(Skip, true), 1, &img).unwrap(), PagePlan::Overlay);
        assert_eq!(plan_page(&opts(Default, true), 1, &img).unwrap(), PagePlan::Overlay);
        // Vector page: skipped with deskew (not lossless), without deskew OCR
        assert_eq!(plan_page(&opts(Default, true), 1, &vector).unwrap(), PagePlan::Skip);
        assert_eq!(plan_page(&opts(Default, false), 1, &vector).unwrap(), PagePlan::Overlay);
        assert_eq!(plan_page(&opts(Force, false), 1, &vector).unwrap(), PagePlan::Rasterize);
        let mut o = opts(Force, false);
        o.pages = Some(BTreeSet::from([1]));
        assert_eq!(plan_page(&o, 2, &text).unwrap(), PagePlan::Skip);
        assert_eq!(page_dpi(&opts(Default, false), &img), 300.0);
        assert_eq!(page_dpi(&opts(Default, false), &vector), 400.0);
    }

    /// Gap E6: value-dependent OCRmyPDF options.
    #[test]
    fn value_dependent_options() {
        use serde_json::json;
        assert!(value_note("pdf_renderer", &json!("hocr")).is_none());
        assert!(value_note("pdf_renderer", &json!("sandwich")).is_none());
        assert!(value_note("pdf_renderer", &json!("hocrdebug")).is_some());
        assert!(value_note("rasterizer", &json!("pypdfium")).is_none());
        assert!(value_note("rasterizer", &json!("ghostscript")).is_some());
        assert!(value_note("sidecar_format", &json!("txt")).is_none());
        assert!(value_note("sidecar_format", &json!("hocr")).is_some());
        assert!(value_note("remove_background", &json!(false)).is_none());
        assert!(value_note("remove_background", &json!(true)).is_some());
        assert!(value_note("fast_web_view", &json!(999999)).is_none());
        assert!(value_note("fast_web_view", &json!(1.0)).is_some());
        assert!(value_note("unpaper_args", &json!("--no-noisefilter")).is_none());
        assert!(value_note("unpaper_args", &json!("--layout double --pre-rotate 90")).unwrap().contains("--pre-rotate"));
        let mut m = serde_json::Map::new();
        m.insert("input_file_or_options".into(), json!("in.pdf"));
        m.insert("output_file".into(), json!("out.pdf"));
        m.insert("clean".into(), json!(true));
        m.insert("unpaper_args".into(), json!("--no-grayfilter"));
        let o = parse_options(&OcrArgs(m)).unwrap();
        assert!(!o.steps().filters.gray && o.steps().filters.specks && o.steps().clean);
    }

    #[test]
    fn options_from_args() {
        let cfg = OcrConfig::defaults();
        let r = crate::args::ArgsRequest {
            input_file: "in.pdf",
            mime_type: "application/pdf",
            output_file: "out.pdf",
            sidecar_file: "s.txt",
            ..Default::default()
        };
        struct N;
        impl ImageOps for N {
            fn get_dpi(&self, _: &str) -> Option<i64> {
                None
            }
            fn calculate_a4_dpi(&self, _: &str) -> Option<i64> {
                None
            }
            fn has_alpha(&self, _: &str) -> Result<bool> {
                Ok(false)
            }
            fn remove_alpha(&self, p: &str) -> Result<String> {
                Ok(p.into())
            }
        }
        let a = crate::args::construct_ocrmypdf_parameters(&cfg, &r, &N).unwrap();
        let o = parse_options(&a).unwrap();
        assert_eq!(o.mode, ProcessingMode::Default);
        assert!(o.deskew && o.clean && o.rotate_pages);
        assert_eq!(o.rotate_threshold, 12.0);
        assert_eq!(o.sidecar, Some(PathBuf::from("s.txt")));
        let mut m = a.clone();
        m.0.insert("force_ocr".into(), serde_json::json!(true));
        m.0.insert("skip_text".into(), serde_json::json!(true));
        assert!(parse_options(&m).is_err());
    }
}
