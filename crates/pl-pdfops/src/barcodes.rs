// upstream: src/documents/barcodes.py@v3.4.2 (Barcode, BarcodePlugin.asn/tags/get_separation_pages/
// separate_pages, scannable_mime_types, read_barcodes_zxing, scan_pdf, read_barcode_values),
// src/paperless/config.py (BarcodeConfig), src/paperless/settings/__init__.py (CONSUMER_*BARCODE*)
//
//! Barcodes: rasterise pages (pl-ocr `PdfRenderer`, PDFium instead of pdf2image/pdftoppm), read with
//! rxing (instead of zxing-cpp), separator pages, ASN, tag assignment, splitting (lopdf instead of pikepdf).
//! The DB parts (`Tag.objects.get_or_create`, ASN existence) are done by the caller (pl-consumer).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pdf::{self, PageCopier};
use crate::{PdfOpsError, Result};

/// `BarcodeConfig` (environment + ApplicationConfiguration; DB overrides are set by the caller).
#[derive(Debug, Clone, PartialEq)]
pub struct BarcodeConfig {
    pub barcodes_enabled: bool,
    pub barcode_enable_tiff_support: bool,
    pub barcode_string: String,
    pub barcode_retain_split_pages: bool,
    pub barcode_enable_asn: bool,
    pub barcode_asn_prefix: String,
    pub barcode_upscale: f64,
    pub barcode_dpi: u32,
    pub barcode_max_pages: u32,
    pub barcode_enable_tag: bool,
    /// Order like the Python dict (JSON order).
    pub barcode_tag_mapping: Vec<(String, String)>,
    pub barcode_tag_split: bool,
    pub barcode_store_values: bool,
}

impl Default for BarcodeConfig {
    fn default() -> Self {
        BarcodeConfig {
            barcodes_enabled: false,
            barcode_enable_tiff_support: false,
            barcode_string: "PATCHT".into(),
            barcode_retain_split_pages: false,
            barcode_enable_asn: false,
            barcode_asn_prefix: "ASN".into(),
            barcode_upscale: 0.0,
            barcode_dpi: 300,
            barcode_max_pages: 0,
            barcode_enable_tag: false,
            barcode_tag_mapping: vec![("TAG:(.*)".into(), "\\g<1>".into())],
            barcode_tag_split: false,
            barcode_store_values: false,
        }
    }
}

fn env_bool(key: &str) -> bool {
    // get_bool_from_env(key, "NO"): value.lower() in ("yes", "y", "1", "t", "true")
    std::env::var(key).is_ok_and(|v| matches!(v.to_lowercase().as_str(), "yes" | "y" | "1" | "t" | "true"))
}

impl BarcodeConfig {
    /// From `PAPERLESS_CONSUMER_*` (defaults like `paperless/settings`). Invalid numbers
    /// make upstream fail at startup; here the default value applies (with a warning).
    pub fn from_env() -> Self {
        let d = BarcodeConfig::default();
        let num = |key: &str| std::env::var(key).ok().map(|v| v.trim().to_string());
        let warn = |key: &str| tracing::warn!(target: "paperless.barcodes", "Invalid value for {}, using default", key.replacen("PAPERLESS_", "PAPERRUST_", 1));
        let barcode_upscale = match num("PAPERLESS_CONSUMER_BARCODE_UPSCALE") {
            Some(v) => v.parse::<f64>().unwrap_or_else(|_| {
                warn("PAPERLESS_CONSUMER_BARCODE_UPSCALE");
                d.barcode_upscale
            }),
            None => d.barcode_upscale,
        };
        let int = |key: &str, def: u32| match num(key) {
            Some(v) => v.parse::<u32>().unwrap_or_else(|_| {
                warn(key);
                def
            }),
            None => def,
        };
        let mapping = match pl_envlive::var("PAPERLESS_CONSUMER_TAG_BARCODE_MAPPING") {
            Some(v) => match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&v) {
                Ok(m) => m.into_iter().map(|(k, v)| (k, v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))).collect(),
                Err(_) => {
                    warn("PAPERLESS_CONSUMER_TAG_BARCODE_MAPPING");
                    d.barcode_tag_mapping.clone()
                }
            },
            None => d.barcode_tag_mapping.clone(),
        };
        BarcodeConfig {
            barcodes_enabled: env_bool("PAPERLESS_CONSUMER_ENABLE_BARCODES"),
            barcode_enable_tiff_support: env_bool("PAPERLESS_CONSUMER_BARCODE_TIFF_SUPPORT"),
            barcode_string: pl_envlive::var("PAPERLESS_CONSUMER_BARCODE_STRING").unwrap_or(d.barcode_string),
            barcode_retain_split_pages: env_bool("PAPERLESS_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES"),
            barcode_enable_asn: env_bool("PAPERLESS_CONSUMER_ENABLE_ASN_BARCODE"),
            barcode_asn_prefix: pl_envlive::var("PAPERLESS_CONSUMER_ASN_BARCODE_PREFIX").unwrap_or(d.barcode_asn_prefix),
            barcode_upscale,
            barcode_dpi: int("PAPERLESS_CONSUMER_BARCODE_DPI", d.barcode_dpi),
            barcode_max_pages: int("PAPERLESS_CONSUMER_BARCODE_MAX_PAGES", d.barcode_max_pages),
            barcode_enable_tag: env_bool("PAPERLESS_CONSUMER_ENABLE_TAG_BARCODE"),
            barcode_tag_mapping: mapping,
            barcode_tag_split: env_bool("PAPERLESS_CONSUMER_TAG_BARCODE_SPLIT"),
            barcode_store_values: env_bool("PAPERLESS_CONSUMER_STORE_BARCODE_VALUES"),
        }
    }

    /// `BarcodePlugin.able_to_run` (without scanner availability).
    pub fn able_to_run(&self, mime_type: &str) -> bool {
        (self.barcode_enable_asn || self.barcodes_enabled || self.barcode_enable_tag || self.barcode_store_values)
            && scannable_mime_types(self).contains(&mime_type)
    }
}

/// `scannable_mime_types`.
pub fn scannable_mime_types(cfg: &BarcodeConfig) -> Vec<&'static str> {
    if cfg.barcode_enable_tiff_support { vec!["application/pdf", "image/tiff"] } else { vec!["application/pdf"] }
}

/// A barcode and its page (0-based).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Barcode {
    pub page: i64,
    pub value: String,
    /// zxing-cpp-Enum-Name (`DocumentBarcode.Format`), z. B. `QRCode`, `Code128`.
    pub format: String,
}

/// As it is stored on the document (page 1-based).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBarcode {
    pub page: i64,
    pub value: String,
    pub format: String,
}

impl Barcode {
    pub fn is_separator(&self, cfg: &BarcodeConfig) -> bool {
        self.value == cfg.barcode_string
    }
    pub fn is_asn(&self, cfg: &BarcodeConfig) -> bool {
        self.value.starts_with(&cfg.barcode_asn_prefix)
    }
    /// Matches a pattern of the tag mapping (does not exclude ASN/separator).
    pub fn is_tag(&self, cfg: &BarcodeConfig) -> bool {
        cfg.barcode_tag_mapping.iter().any(|(p, _)| pl_matching::regex_compat::safe_regex_match(p, &self.value, true).is_some())
    }
    pub fn stored(&self) -> StoredBarcode {
        StoredBarcode { page: self.page + 1, value: self.value.clone(), format: self.format.clone() }
    }
}

/// `get_separation_pages`: page → keep the page?
pub fn separation_pages(barcodes: &[Barcode], cfg: &BarcodeConfig) -> BTreeMap<i64, bool> {
    let retain = cfg.barcode_retain_split_pages;
    let mut out = BTreeMap::new();
    for bc in barcodes {
        // not the first page if it is kept (otherwise infinite loop)
        if bc.is_separator(cfg) && (!retain || bc.page > 0) {
            out.insert(bc.page, retain);
        }
    }
    if cfg.barcode_enable_asn {
        for bc in barcodes.iter().filter(|b| b.is_asn(cfg) && b.page != 0) {
            out.insert(bc.page, true);
        }
    }
    if cfg.barcode_tag_split && cfg.barcode_enable_tag {
        for bc in barcodes.iter().filter(|b| b.is_tag(cfg) && b.page != 0) {
            out.insert(bc.page, true);
        }
    }
    out
}

/// `BarcodePlugin.asn`: first barcode with the ASN prefix → number. `Err(text)` = upstream warning
/// `Failed to parse ASN number because: {text}`.
pub fn located_asn(barcodes: &[Barcode], cfg: &BarcodeConfig) -> std::result::Result<Option<i64>, String> {
    let Some(text) = barcodes.iter().find(|b| b.is_asn(cfg)).map(|b| b.value.as_str()) else {
        return Ok(None);
    };
    tracing::debug!(target: "paperless.barcodes", "Found ASN Barcode: {text}");
    let rest = text.get(cfg.barcode_asn_prefix.len()..).unwrap_or("").trim();
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    match digits.parse::<i64>() {
        Ok(n) => Ok(Some(n)),
        Err(_) => Err(format!("invalid literal for int() with base 10: '{digits}'")),
    }
}

/// `BarcodePlugin.tags` without DB: per barcode value, split at `,`, the tag name after the first
/// matching mapping (`(raw, tag_name)`, empty names are dropped). The caller does
/// `Tag.objects.get_or_create(name__iexact=…)`.
pub fn tag_names(barcodes: &[Barcode], cfg: &BarcodeConfig) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for bc in barcodes {
        for raw in bc.value.split(',') {
            let mut tag: Option<String> = None;
            for (pattern, sub) in &cfg.barcode_tag_mapping {
                if pl_matching::regex_compat::safe_regex_match(pattern, raw, true).is_some() {
                    tag = Some(if sub.is_empty() {
                        raw.to_string()
                    } else {
                        pl_matching::regex_compat::safe_regex_sub(pattern, sub, raw, true).unwrap_or_default()
                    });
                    break;
                }
            }
            if let Some(t) = tag.filter(|t| !t.is_empty()) {
                out.push((raw.to_string(), t));
            }
        }
    }
    out
}

/// Image → barcodes (`read_barcodes_zxing`): `(text, format)`, empty texts are dropped.
pub trait BarcodeReader: Send + Sync {
    fn read(&self, img: &image::GrayImage) -> Vec<(String, String)>;
}

/// rxing (pure Rust, port of ZXing) with `TryHarder` and inverted search
/// (zxing-cpp default: try_harder/try_rotate/try_invert).
#[derive(Debug, Default, Clone, Copy)]
pub struct RxingReader;

/// rxing-Format → zxing-cpp-3-Enum-Name (`DocumentBarcode.Format`).
pub fn zxing_format_name(f: &rxing::BarcodeFormat) -> &'static str {
    use rxing::BarcodeFormat as F;
    match f {
        F::AZTEC => "Aztec",
        F::CODABAR => "Codabar",
        F::CODE_39 => "Code39",
        F::CODE_93 => "Code93",
        F::CODE_128 => "Code128",
        F::DATA_MATRIX => "DataMatrix",
        F::EAN_8 => "EAN8",
        F::EAN_13 => "EAN13",
        F::ITF => "ITF",
        F::MAXICODE => "MaxiCode",
        F::PDF_417 => "PDF417",
        F::QR_CODE => "QRCode",
        F::MICRO_QR_CODE => "MicroQRCode",
        F::RECTANGULAR_MICRO_QR_CODE => "RMQRCode",
        F::RSS_14 => "DataBar",
        F::RSS_EXPANDED => "DataBarExp",
        F::TELEPEN => "Telepen",
        F::UPC_A => "UPCA",
        F::UPC_E => "UPCE",
        F::UPC_EAN_EXTENSION => "EAN5",
        F::DXFilmEdge => "DXFilmEdge",
        _ => "OtherBarcode",
    }
}

/// Levels of the image pyramid (replacement for zxing-cpp `try_downscale`): rxing often finds large or
/// distorted codes at 300 dpi only when downscaled. The next level is read only if the
/// previous one found **nothing** (reading all levels together yields duplicate and false hits, e.g.
/// UPC-E in text) and as long as the shorter side has at least [`MIN_LAYER_SIDE`] pixels.
pub const DOWNSCALE_FACTORS: &[u32] = &[1, 2, 3, 6];
pub const MIN_LAYER_SIDE: u32 = 300;

impl RxingReader {
    fn read_layer(img: &image::GrayImage) -> Vec<rxing::RXingResult> {
        let (w, h) = img.dimensions();
        let mut hints = rxing::DecodeHints { TryHarder: Some(true), AlsoInverted: Some(true), ..Default::default() };
        rxing::helpers::detect_multiple_in_luma_with_hints(img.as_raw().clone(), w, h, &mut hints).unwrap_or_default()
    }
}

impl BarcodeReader for RxingReader {
    fn read(&self, img: &image::GrayImage) -> Vec<(String, String)> {
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Vec::new();
        }
        // (text, format, center_x, center_y) in original coordinates
        let mut found: Vec<(String, &'static str, f32, f32)> = Vec::new();
        let tol = (w.max(h) as f32) * 0.02;
        for &f in DOWNSCALE_FACTORS {
            if w.min(h) / f < MIN_LAYER_SIDE {
                break;
            }
            let layer;
            let src = if f == 1 {
                img
            } else {
                layer = image::imageops::resize(img, w / f, h / f, image::imageops::FilterType::Triangle);
                &layer
            };
            for r in Self::read_layer(src) {
                let text = r.getText().to_string();
                if text.is_empty() {
                    continue;
                }
                let fmt = zxing_format_name(r.getBarcodeFormat());
                let pts = r.getPoints();
                let (cx, cy) = if pts.is_empty() {
                    (f32::NAN, f32::NAN)
                } else {
                    let n = pts.len() as f32;
                    (pts.iter().map(|p| p.x).sum::<f32>() / n * f as f32, pts.iter().map(|p| p.y).sum::<f32>() / n * f as f32)
                };
                // the same symbol from another level not twice
                let dup = found.iter().any(|(t, ff, x, y)| {
                    *t == text && *ff == fmt && (cx.is_nan() || x.is_nan() || ((x - cx).abs() < tol && (y - cy).abs() < tol))
                });
                if !dup {
                    tracing::debug!(target: "paperless.barcodes", "Barcode of type {fmt} found: {text}");
                    found.push((text, fmt, cx, cy));
                }
            }
            if !found.is_empty() {
                break;
            }
        }
        found.into_iter().map(|(t, f, _, _)| (t, f.to_string())).collect()
    }
}

/// Raster settings of `scan_pdf`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanSettings {
    pub dpi: u32,
    pub upscale: f64,
    pub max_pages: u32,
}

impl From<&BarcodeConfig> for ScanSettings {
    fn from(c: &BarcodeConfig) -> Self {
        ScanSettings { dpi: c.barcode_dpi, upscale: c.barcode_upscale, max_pages: c.barcode_max_pages }
    }
}

/// Rasterise the page (grayscale) and upscale if needed (`page.resize((round(x*f), round(y*f)))`,
/// Pillow default BICUBIC → Catmull-Rom).
pub fn render_page(doc: &dyn pl_ocr::render::RenderDoc, index: usize, s: ScanSettings) -> Result<image::GrayImage> {
    let r = doc
        .render(index, f64::from(s.dpi.max(1)), true)
        .map_err(|e| PdfOpsError::Pdf(format!("rendering page {} failed: {e}", index + 1)))?;
    let img = image::GrayImage::from_raw(r.width, r.height, r.data)
        .ok_or_else(|| PdfOpsError::Pdf("renderer returned an invalid bitmap".into()))?;
    if s.upscale > 1.0 {
        tracing::debug!(target: "paperless.barcodes", "Upscaling image by {} for better barcode detection", s.upscale);
        let nw = pl_ocr::imageinfo::py_round(f64::from(img.width()) * s.upscale).max(1) as u32;
        let nh = pl_ocr::imageinfo::py_round(f64::from(img.height()) * s.upscale).max(1) as u32;
        return Ok(image::imageops::resize(&img, nw, nh, image::imageops::FilterType::CatmullRom));
    }
    Ok(img)
}

/// `scan_pdf`: barcodes of all (or the first `max_pages`) pages. Errors are not
/// caught (encrypted → `Password`), so that the caller can distinguish "nothing found" from
/// "failed".
pub fn scan_pdf(
    pdf_path: &Path,
    settings: ScanSettings,
    renderer: &dyn pl_ocr::render::PdfRenderer,
    reader: &dyn BarcodeReader,
) -> Result<Vec<Barcode>> {
    let num_of_pages = pdf::page_ids(&pdf::load(pdf_path, None)?).len();
    tracing::debug!(target: "paperless.barcodes", "PDF has {num_of_pages} pages");
    let max = if settings.max_pages == 0 { num_of_pages } else { settings.max_pages as usize };
    if max < num_of_pages {
        tracing::debug!(target: "paperless.barcodes", "Barcodes detection will be limited to the first {max} pages");
    }
    let data = std::fs::read(pdf_path)?;
    let doc = renderer.open(&data).map_err(|e| match e {
        pl_ocr::OcrError::Unavailable(m) => PdfOpsError::Unavailable(m),
        other => PdfOpsError::Pdf(other.to_string()),
    })?;
    let mut out = Vec::new();
    for page in 0..num_of_pages.min(max).min(doc.page_count()) {
        tracing::debug!(target: "paperless.barcodes", "Processing page {page}");
        let img = render_page(doc.as_ref(), page, settings)?;
        for (value, format) in reader.read(&img) {
            out.push(Barcode { page: page as i64, value, format });
        }
    }
    Ok(out)
}

/// `separate_pages`: split at the pages from `pages_to_split_on` (value = keep page).
/// Outputs `{fname}_document_{i}.pdf` in `out_dir`, file times from `stats_source`.
pub fn separate_pages(
    pdf_file: &Path,
    pages_to_split_on: &BTreeMap<i64, bool>,
    fname: &str,
    out_dir: &Path,
    stats_source: &Path,
) -> Result<Vec<PathBuf>> {
    let src = pdf::load(pdf_file, None)?;
    let mut documents: Vec<Vec<lopdf::ObjectId>> = vec![Vec::new()];
    for (idx, page) in pdf::page_ids(&src).into_iter().enumerate() {
        match pages_to_split_on.get(&(idx as i64)) {
            None => {
                if let Some(cur) = documents.last_mut() {
                    cur.push(page);
                }
            }
            Some(keep) => {
                tracing::debug!(target: "paperless.barcodes", "Starting new document at idx {idx}");
                documents.push(if *keep { vec![page] } else { Vec::new() });
            }
        }
    }
    documents.retain(|d| !d.is_empty());
    tracing::debug!(target: "paperless.barcodes", "Split into {} new documents", documents.len());
    let mut paths = Vec::new();
    for (doc_idx, pages) in documents.iter().enumerate() {
        let mut c = PageCopier::new();
        for p in pages {
            c.append(0, &src, *p)?;
        }
        let mut dst = c.finish();
        tracing::debug!(target: "paperless.barcodes", "pdf no:{doc_idx} has {} pages", pages.len());
        let save_path = out_dir.join(format!("{fname}_document_{doc_idx}.pdf"));
        pdf::save(&mut dst, &save_path)?;
        crate::copy_basic_file_stats(stats_source, &save_path)?;
        paths.push(save_path);
    }
    Ok(paths)
}

/// `read_barcode_values`: barcodes outside the plugin (new versions, reprocessing).
/// `None` if the type is not scannable with these settings.
pub fn read_barcode_values(
    path: &Path,
    mime_type: &str,
    cfg: &BarcodeConfig,
    work_dir: &Path,
    renderer: &dyn pl_ocr::render::PdfRenderer,
    reader: &dyn BarcodeReader,
) -> Result<Option<Vec<StoredBarcode>>> {
    if !scannable_mime_types(cfg).contains(&mime_type) {
        return Ok(None);
    }
    let pdf_path = if mime_type == "image/tiff" { crate::convert::convert_from_tiff_to_pdf(path, work_dir)? } else { path.to_path_buf() };
    Ok(Some(scan_pdf(&pdf_path, cfg.into(), renderer, reader)?.iter().map(Barcode::stored).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bc(page: i64, value: &str) -> Barcode {
        Barcode { page, value: value.into(), format: "QRCode".into() }
    }

    #[test]
    fn separators_like_upstream() {
        let mut cfg = BarcodeConfig { barcodes_enabled: true, ..Default::default() };
        let codes = [bc(0, "PATCHT"), bc(2, "PATCHT"), bc(3, "ASN0042"), bc(4, "TAG:x")];
        assert_eq!(separation_pages(&codes, &cfg), BTreeMap::from([(0, false), (2, false)]));
        cfg.barcode_retain_split_pages = true;
        assert_eq!(separation_pages(&codes, &cfg), BTreeMap::from([(2, true)]));
        cfg.barcode_retain_split_pages = false;
        cfg.barcode_enable_asn = true;
        assert_eq!(separation_pages(&codes, &cfg), BTreeMap::from([(0, false), (2, false), (3, true)]));
        cfg.barcode_enable_tag = true;
        cfg.barcode_tag_split = true;
        assert_eq!(separation_pages(&codes, &cfg), BTreeMap::from([(0, false), (2, false), (3, true), (4, true)]));
        // ASN on page 0 never separates
        assert!(separation_pages(&[bc(0, "ASN1")], &cfg).is_empty());
    }

    #[test]
    fn asn_parsing() {
        let cfg = BarcodeConfig::default();
        assert_eq!(located_asn(&[bc(0, "x"), bc(1, "ASN 00123"), bc(2, "ASN9")], &cfg), Ok(Some(123)));
        assert_eq!(located_asn(&[bc(0, "ASN-12-3x")], &cfg), Ok(Some(123)));
        assert_eq!(located_asn(&[bc(0, "x")], &cfg), Ok(None));
        assert_eq!(located_asn(&[bc(0, "ASNabc")], &cfg), Err("invalid literal for int() with base 10: ''".into()));
        let cfg = BarcodeConfig { barcode_asn_prefix: "CUSTOM-PREFIX-".into(), ..Default::default() };
        assert_eq!(located_asn(&[bc(0, "CUSTOM-PREFIX-00123")], &cfg), Ok(Some(123)));
    }

    #[test]
    fn tags_with_mapping() {
        let cfg = BarcodeConfig::default();
        assert_eq!(
            tag_names(&[bc(0, "TAG:invoice,tag:Bank,other"), bc(1, "TAG:")], &cfg),
            vec![("TAG:invoice".to_string(), "invoice".to_string()), ("tag:Bank".into(), "Bank".into())]
        );
        let cfg = BarcodeConfig {
            barcode_tag_mapping: vec![("ASN12.*".into(), "JOHN".into()), ("TAG:(.*)".into(), "".into()), ("CUST(.*)".into(), "\\g<1>".into())],
            ..Default::default()
        };
        assert_eq!(
            tag_names(&[bc(0, "ASN123"), bc(0, "TAG:abc"), bc(0, "CUSTOMER")], &cfg),
            vec![("ASN123".into(), "JOHN".into()), ("TAG:abc".into(), "TAG:abc".into()), ("CUSTOMER".into(), "OMER".into())]
        );
        assert!(bc(0, "asn123").is_tag(&cfg));
        assert!(!bc(0, "zzz").is_tag(&cfg));
    }

    #[test]
    fn mime_types_and_able_to_run() {
        let mut cfg = BarcodeConfig::default();
        assert!(!cfg.able_to_run("application/pdf"));
        cfg.barcode_store_values = true;
        assert!(cfg.able_to_run("application/pdf"));
        assert!(!cfg.able_to_run("image/tiff"));
        cfg.barcode_enable_tiff_support = true;
        assert!(cfg.able_to_run("image/tiff"));
    }

    #[test]
    fn separate_pages_like_upstream() {
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("in.pdf");
        crate::pdf::testpdf::write(&s, &["A", "SEP", "B", "C", "ASN"]);
        let split = BTreeMap::from([(1, false), (4, true)]);
        let out = separate_pages(&s, &split, "in", t.path(), &s).unwrap();
        let names: Vec<_> = out.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["in_document_0.pdf", "in_document_1.pdf", "in_document_2.pdf"]);
        assert_eq!(crate::pdf::testpdf::texts(&out[0]), ["A"]);
        assert_eq!(crate::pdf::testpdf::texts(&out[1]), ["B", "C"]);
        assert_eq!(crate::pdf::testpdf::texts(&out[2]), ["ASN"]);
        // separator page at the front, not kept → no empty document
        let out = separate_pages(&s, &BTreeMap::from([(0, false)]), "x", t.path(), &s).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(crate::pdf::testpdf::texts(&out[0]), ["SEP", "B", "C", "ASN"]);
    }

    #[test]
    fn rxing_reads_generated_qr_and_code128() {
        use rxing::Writer;
        let qr = rxing::qrcode::QRCodeWriter.encode("PATCHT", &rxing::BarcodeFormat::QR_CODE, 300, 300).unwrap();
        let c128 = rxing::oned::Code128Writer.encode("ASN00123", &rxing::BarcodeFormat::CODE_128, 600, 150).unwrap();
        let mut img = image::GrayImage::from_pixel(1200, 900, image::Luma([255]));
        let blit = |img: &mut image::GrayImage, m: &rxing::common::BitMatrix, ox: u32, oy: u32| {
            for y in 0..m.getHeight() {
                for x in 0..m.getWidth() {
                    if m.get(x, y) {
                        img.put_pixel(ox + x, oy + y, image::Luma([0]));
                    }
                }
            }
        };
        blit(&mut img, &qr, 50, 50);
        blit(&mut img, &c128, 450, 600);
        let mut found = RxingReader.read(&img);
        found.sort();
        assert_eq!(found, vec![("ASN00123".to_string(), "Code128".to_string()), ("PATCHT".to_string(), "QRCode".to_string())]);
    }
}
