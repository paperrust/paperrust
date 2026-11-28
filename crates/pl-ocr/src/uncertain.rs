//! PaperRust: uncertain lines/pages of the classic OCR for the AI transcription ("KI-Abschrift",
//! docs/OCR-NEXT.md 2.8).
//!
//! * [`LinesFile`]: per-page line data of the classic OCR (text, box, confidence) as JSON next to the
//!   sidecar (`ocr-lines.json`, [`write_for_sidecar`]). The OCR pipeline fills it from its
//!   [`PageOcr`] results; the consumer reads it after parsing ([`read_for_dir`]).
//! * [`select`]: which whole pages and which line crops go to the LLM, by mode and sensitivity.
//!   Thresholds per OCR engine ([`thresholds`]); users never see confidence numbers.
//! * [`build_request`]: page images (downscaled) and line crops for pl-core `vision`.
//! * [`merge_content`] / [`replace_lines`]: accepted AI texts into the content, page by page and line by
//!   line; the PDF text layer is not touched (it always stays from the classic OCR).
//!
//! Confidence scale: [`crate::engine::Word::conf`] is 0–100 for Tesseract. Engines that report 0–1
//! scores (PP-OCR) are normalized to 0–100 in [`PageLines::from_page_ocr`] if all values are ≤ 1.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pl_core::vision::{PageImage, Sensitivity, VisionCrop, VisionMode, VisionPage, VisionRequest};
use serde::{Deserialize, Serialize};

use crate::engine::PageOcr;
use crate::render::PdfRenderer;
use crate::vision_pages;
use crate::{OcrError, Result};

/// File name of the line data next to the sidecar file.
pub const LINES_FILE_NAME: &str = "ocr-lines.json";

/// At most this many crops per document (more → whole pages are cheaper anyway).
pub const MAX_CROPS_PER_DOCUMENT: usize = 60;

/// A page counts as uncertain below this many visible characters (old `vision_when = difficult` rule:
/// fewer than 50 characters per page).
pub const MIN_PAGE_CHARS: usize = 50;

/// One OCR line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrLine {
    /// Words joined by a space.
    pub text: String,
    /// Box in pixels of the OCR input image `[x0, y0, x1, y1]`.
    pub bbox: [f32; 4],
    /// Character-weighted mean confidence (0–100) of the words with a confidence ≥ 0.
    pub conf: f32,
    /// Visible characters.
    pub chars: usize,
}

/// OCR lines of one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageLines {
    /// Size of the OCR input image in pixels.
    pub width: u32,
    pub height: u32,
    /// Plain text of the page as the engine produced it (sidecar text of this page).
    pub text: String,
    pub lines: Vec<OcrLine>,
    /// `false` if the engine reported no usable confidences (all equal) → no line selection.
    pub has_confidence: bool,
}

/// Line data of a document (one entry per page; `None` = page not OCRed, e.g. it had text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinesFile {
    pub version: u32,
    /// OCR engine (`tesseract`, `ppocr`, `winocr`, `macocr`, …) – selects the thresholds.
    pub engine: String,
    pub pages: Vec<Option<PageLines>>,
}

impl PageLines {
    /// Lines of an engine result. `engine` decides the confidence scale (see module doc).
    pub fn from_page_ocr(ocr: &PageOcr) -> Self {
        let confs: Vec<f32> = ocr.words.iter().map(|w| w.conf).filter(|c| *c >= 0.0).collect();
        let scale = if !confs.is_empty() && confs.iter().all(|c| *c <= 1.0) && confs.iter().any(|c| *c > 0.0) { 100.0 } else { 1.0 };
        let has_confidence = confs.windows(2).any(|w| (w[0] - w[1]).abs() > f32::EPSILON);
        let lines = ocr
            .lines()
            .into_iter()
            .filter_map(|ws| {
                let words: Vec<_> = ws.into_iter().filter(|w| !w.text.trim().is_empty()).collect();
                if words.is_empty() {
                    return None;
                }
                let text = words.iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ");
                let chars = text.chars().filter(|c| !c.is_whitespace()).count();
                let (mut sum, mut weight) = (0.0f64, 0.0f64);
                for w in &words {
                    if w.conf >= 0.0 {
                        let n = w.text.chars().filter(|c| !c.is_whitespace()).count().max(1) as f64;
                        sum += f64::from(w.conf * scale) * n;
                        weight += n;
                    }
                }
                let conf = if weight > 0.0 { (sum / weight) as f32 } else { 0.0 };
                let bbox = words.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, w| {
                    [b[0].min(w.x0 as f32), b[1].min(w.y0 as f32), b[2].max(w.x1 as f32), b[3].max(w.y1 as f32)]
                });
                Some(OcrLine { text, bbox, conf, chars })
            })
            .collect();
        Self { width: ocr.width, height: ocr.height, text: ocr.text.clone(), lines, has_confidence }
    }

    /// Visible characters of the page.
    pub fn chars(&self) -> usize {
        self.lines.iter().map(|l| l.chars).sum()
    }
}

impl LinesFile {
    /// From the engine results of all pages (`None` = page skipped).
    pub fn from_pages(engine: &str, pages: &[Option<&PageOcr>]) -> Self {
        Self { version: 1, engine: engine.to_string(), pages: pages.iter().map(|p| p.map(PageLines::from_page_ocr)).collect() }
    }
}

/// Path of the line data next to a sidecar file.
pub fn lines_path_for_sidecar(sidecar: &Path) -> PathBuf {
    sidecar.with_file_name(LINES_FILE_NAME)
}

/// Writes the line data next to the sidecar (hook for the OCR pipeline; errors are only logged by
/// callers – the line data is optional).
pub fn write_for_sidecar(sidecar: &Path, engine: &str, pages: &[Option<&PageOcr>]) -> Result<()> {
    let f = LinesFile::from_pages(engine, pages);
    let json = serde_json::to_vec(&f).map_err(|e| OcrError::Other(format!("line data: {e}")))?;
    std::fs::write(lines_path_for_sidecar(sidecar), json)?;
    Ok(())
}

/// Reads the line data from a parser working folder (`None` = not there/unreadable).
pub fn read_for_dir(dir: &Path) -> Option<LinesFile> {
    let bytes = std::fs::read(dir.join(LINES_FILE_NAME)).ok()?;
    let f: LinesFile = serde_json::from_slice(&bytes).ok()?;
    (f.version == 1).then_some(f)
}

/// Thresholds of an engine and sensitivity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// A line is uncertain below this mean confidence (0–100).
    pub line_conf: f32,
    /// A page is uncertain if more than this share of its characters is in uncertain lines.
    pub page_low_share: f32,
}

/// Threshold table per OCR engine (confidences of Tesseract and PP-OCR are not comparable).
///
/// TODO(calibrate): values are first estimates; derive them from the measurements in
/// `results/ocr-eval/2026-11-24` (per-line confidences vs. character error rate per engine) once the
/// pipeline writes `ocr-lines.json` for the evaluation corpus.
pub fn thresholds(engine: &str, s: Sensitivity) -> Thresholds {
    let (sparing, normal, thorough) = match engine {
        // Tesseract word confidences: clean print ≥ 90, handwriting/poor scans mostly 20–60.
        "tesseract" | "tesslib" | "tesseract-cli" => (40.0, 55.0, 70.0),
        // PP-OCR recognition scores (0–1, normalized ×100) are overconfident: on the synthetic
        // handwriting pages (2026-11-26) misread lines scored 94–98, correct ones mostly 98–100;
        // the first guess (65/78/88) never selected anything. Small sample, recalibrate with more data.
        "ppocr" | "paddle" | "onnx" => (95.0, 97.0, 98.0),
        // Unknown engines: Tesseract-like.
        _ => (40.0, 55.0, 70.0),
    };
    match s {
        Sensitivity::Sparing => Thresholds { line_conf: sparing, page_low_share: 0.5 },
        Sensitivity::Normal => Thresholds { line_conf: normal, page_low_share: 0.3 },
        Sensitivity::Thorough => Thresholds { line_conf: thorough, page_low_share: 0.15 },
    }
}

/// Whether a line is uncertain (tiny specks are ignored).
fn line_uncertain(page: &PageLines, l: &OcrLine, t: &Thresholds) -> bool {
    let h = l.bbox[3] - l.bbox[1];
    page.has_confidence && l.conf < t.line_conf && h >= (page.height as f32 * 0.006).max(4.0)
}

/// Whether a page as a whole is uncertain: little text (old rule), or a large share of its characters in
/// uncertain lines.
pub fn page_uncertain(page: &PageLines, t: &Thresholds) -> bool {
    let chars = page.chars();
    if chars < MIN_PAGE_CHARS {
        return true;
    }
    let low: usize = page.lines.iter().filter(|l| line_uncertain(page, l, t)).map(|l| l.chars).sum();
    low as f32 > chars as f32 * t.page_low_share
}

/// What goes to the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    /// 1-based pages sent whole.
    pub pages: Vec<usize>,
    /// `(page, line index)` sent as crops.
    pub crops: Vec<(usize, usize)>,
    /// Pages left out because of the page limit.
    pub skipped_pages: usize,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty() && self.crops.is_empty()
    }
}

/// Selection by mode. `lines` = line data (if the pipeline wrote it); without it the old document rule
/// applies (fewer than 50 characters per page → all pages). `doc_chars` = visible characters of the
/// classic text, `page_count` ≥ 1.
pub fn select(
    mode: VisionMode,
    s: Sensitivity,
    lines: Option<&LinesFile>,
    doc_chars: usize,
    page_count: usize,
    max_pages: usize,
) -> Selection {
    let page_count = page_count.max(1);
    let mut out = Selection::default();
    let mut want_pages: Vec<usize> = Vec::new();
    match (mode, lines) {
        (VisionMode::Off, _) => return out,
        (VisionMode::Always, _) => want_pages = (1..=page_count).collect(),
        (_, None) => {
            if doc_chars < MIN_PAGE_CHARS * page_count {
                want_pages = (1..=page_count).collect();
            }
        }
        (VisionMode::UncertainPages, Some(f)) => {
            let t = thresholds(&f.engine, s);
            for (i, p) in f.pages.iter().enumerate() {
                if p.as_ref().is_some_and(|p| page_uncertain(p, &t)) {
                    want_pages.push(i + 1);
                }
            }
        }
        (VisionMode::UncertainLines, Some(f)) => {
            let t = thresholds(&f.engine, s);
            for (i, p) in f.pages.iter().enumerate() {
                let Some(p) = p else { continue };
                let low: Vec<usize> = (0..p.lines.len()).filter(|&j| line_uncertain(p, &p.lines[j], &t)).collect();
                // Little text (handwriting-only page, photo) or mostly uncertain → whole page is cheaper.
                let mostly = low.len() > 12usize.max(p.lines.len() * 6 / 10);
                if p.chars() < MIN_PAGE_CHARS || mostly {
                    want_pages.push(i + 1);
                } else {
                    out.crops.extend(low.into_iter().map(|j| (i + 1, j)));
                }
            }
        }
    }
    if want_pages.len() > max_pages {
        out.skipped_pages = want_pages.len() - max_pages;
        want_pages.truncate(max_pages);
    }
    out.pages = want_pages;
    out.crops.truncate(MAX_CROPS_PER_DOCUMENT);
    out
}

/// Crop of a line from a page raster: box scaled from OCR pixels to the raster, padded by a quarter line
/// height (half a line height to the sides), upscaled if smaller than 48 px high. `None` if the raster
/// does not match the OCR image (other orientation) or the box is empty.
pub fn crop_line(raster: &image::DynamicImage, page: &PageLines, line: &OcrLine) -> Option<(image::DynamicImage, f32)> {
    if page.width == 0 || page.height == 0 || raster.width() == 0 || raster.height() == 0 {
        return None;
    }
    let ocr_ratio = page.width as f32 / page.height as f32;
    let r_ratio = raster.width() as f32 / raster.height() as f32;
    if (ocr_ratio - r_ratio).abs() / r_ratio > 0.05 {
        return None;
    }
    let sx = raster.width() as f32 / page.width as f32;
    let sy = raster.height() as f32 / page.height as f32;
    let lh = (line.bbox[3] - line.bbox[1]).max(1.0);
    let x0 = ((line.bbox[0] - lh * 0.5) * sx).max(0.0);
    let y0 = ((line.bbox[1] - lh * 0.25) * sy).max(0.0);
    let x1 = ((line.bbox[2] + lh * 0.5) * sx).min(raster.width() as f32);
    let y1 = ((line.bbox[3] + lh * 0.25) * sy).min(raster.height() as f32);
    if x1 - x0 < 2.0 || y1 - y0 < 2.0 {
        return None;
    }
    let (x, y, w, h) = (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
    let mut img = raster.crop_imm(x, y, w.max(1), h.max(1));
    if img.height() < 48 {
        let f = 48.0 / img.height() as f32;
        img = img.resize_exact(((img.width() as f32 * f) as u32).max(1), 48, image::imageops::FilterType::CatmullRom);
    }
    let area = (w as f32 * h as f32) / (raster.width() as f32 * raster.height() as f32);
    Some((img, area.clamp(0.0, 1.0)))
}

/// Sources of the images.
pub struct RequestSources<'a> {
    /// File for whole pages (original or archive PDF) and its MIME type.
    pub pages: (&'a Path, &'a str),
    /// File for line crops – preferably the archive PDF (rotation/deskew as the OCR saw it).
    pub crops: (&'a Path, &'a str),
}

/// Page images and crops for a selection. Rendering errors of the crop source only drop the crops.
pub fn build_request(
    renderer: &dyn PdfRenderer,
    src: &RequestSources<'_>,
    sel: &Selection,
    lines: Option<&LinesFile>,
    max_side: u32,
    max_bytes: usize,
) -> Result<VisionRequest> {
    let mut req = VisionRequest::default();
    let page_lines = |p: usize| lines.and_then(|f| f.pages.get(p.wrapping_sub(1))).and_then(Option::as_ref);
    if !sel.pages.is_empty() {
        for (p, img) in vision_pages::page_rasters(renderer, src.pages.0, src.pages.1, &sel.pages, pl_core::vision::PAGE_IMAGE_DPI)? {
            let (mime, data) = vision_pages::encode(&vision_pages::downscale(img, max_side), max_bytes)?;
            req.pages.push(VisionPage { page: p, image: PageImage { mime, data }, ocr_chars: page_lines(p).map(PageLines::chars) });
        }
    }
    if !sel.crops.is_empty() {
        let mut crop_pages: Vec<usize> = sel.crops.iter().map(|c| c.0).collect();
        crop_pages.dedup();
        let rasters = match vision_pages::page_rasters(renderer, src.crops.0, src.crops.1, &crop_pages, pl_core::vision::PAGE_IMAGE_DPI) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("AI transcription: line crops skipped, page rendering failed: {e}");
                Vec::new()
            }
        };
        for (p, raster) in &rasters {
            let Some(pl) = page_lines(*p) else { continue };
            for &(_, j) in sel.crops.iter().filter(|c| c.0 == *p) {
                let Some(line) = pl.lines.get(j) else { continue };
                let Some((img, area)) = crop_line(raster, pl, line) else { continue };
                let (mime, data) = vision_pages::encode(&img, max_bytes)?;
                req.crops.push(VisionCrop {
                    page: *p,
                    line: j,
                    image: PageImage { mime, data },
                    area_fraction: area,
                    ocr_text: line.text.clone(),
                });
            }
        }
    }
    Ok(req)
}

/// Replaces OCR line texts by AI texts in a page text, in order (each search starts after the previous
/// replacement). Returns the new text and the indices of the replacements that were found.
pub fn replace_lines(page_text: &str, repl: &[(String, String)]) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(page_text.len());
    let mut rest = page_text;
    let mut applied = Vec::new();
    for (i, (ocr, ai)) in repl.iter().enumerate() {
        if ocr.is_empty() {
            continue;
        }
        if let Some(pos) = rest.find(ocr.as_str()) {
            out.push_str(&rest[..pos]);
            out.push_str(ai);
            rest = &rest[pos + ocr.len()..];
            applied.push(i);
        }
    }
    out.push_str(rest);
    (out, applied)
}

/// Content from per-page texts: whole-page AI texts replace the page, line replacements are applied to
/// the classic page text. `None` if a page has no classic text in the line data (then the caller cannot
/// merge per page). Pages are separated by a blank line; the result is cleaned like the sidecar text.
pub fn merge_content(
    lines: &LinesFile,
    page_ai: &BTreeMap<usize, String>,
    line_ai: &BTreeMap<usize, Vec<(String, String)>>,
) -> Option<(String, Vec<(usize, usize)>)> {
    let mut parts = Vec::with_capacity(lines.pages.len());
    let mut applied = Vec::new();
    for (i, p) in lines.pages.iter().enumerate() {
        let p = p.as_ref()?;
        let n = i + 1;
        if let Some(t) = page_ai.get(&n) {
            parts.push(t.clone());
        } else if let Some(r) = line_ai.get(&n) {
            let (t, a) = replace_lines(&p.text, r);
            applied.extend(a.into_iter().map(|j| (n, j)));
            parts.push(t);
        } else {
            parts.push(p.text.clone());
        }
    }
    let joined = parts.iter().map(|s| s.trim()).collect::<Vec<_>>().join("\n\n");
    Some((crate::text::post_process_text(Some(&joined)).unwrap_or_default(), applied))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Word;

    fn w(text: &str, x0: f64, y0: f64, conf: f32, line: u32) -> Word {
        Word { text: text.into(), x0, y0, x1: x0 + 10.0 * text.len() as f64, y1: y0 + 30.0, conf, line: (1, 1, line) }
    }

    fn page(words: Vec<Word>, text: &str) -> PageOcr {
        PageOcr { width: 2480, height: 3508, words, text: text.into() }
    }

    /// Printed letter (high confidence) with one handwritten margin note (low confidence).
    fn letter() -> PageOcr {
        let mut words = Vec::new();
        let mut text = String::new();
        for l in 0..10u32 {
            let y = 200.0 + f64::from(l) * 60.0;
            words.push(w("Sehr", 100.0, y, 96.0, l));
            words.push(w("geehrte", 160.0, y, 94.0, l));
            words.push(w("Damen", 250.0, y, 95.0, l));
            text.push_str("Sehr geehrte Damen\n");
        }
        words.push(w("Bifte", 100.0, 900.0, 31.0, 20));
        words.push(w("anrufen!", 170.0, 900.0, 22.0, 20));
        text.push_str("Bifte anrufen!\n");
        page(words, &text)
    }

    #[test]
    fn lines_and_scale() {
        let p = PageLines::from_page_ocr(&letter());
        assert_eq!(p.lines.len(), 11);
        assert_eq!(p.lines[10].text, "Bifte anrufen!");
        assert!(p.lines[10].conf < 30.0 && p.lines[0].conf > 90.0);
        assert!(p.has_confidence);
        assert_eq!(p.lines[10].bbox[1], 900.0);
        // 0–1 scores (PP-OCR) are normalized to 0–100
        let pp = page(vec![w("Hallo", 0.0, 0.0, 0.98, 1), w("Welt", 60.0, 0.0, 0.5, 2)], "Hallo\nWelt");
        let p = PageLines::from_page_ocr(&pp);
        assert!((p.lines[0].conf - 98.0).abs() < 0.01 && (p.lines[1].conf - 50.0).abs() < 0.01);
        // no usable confidences (all equal) → no line selection
        let flat = page(vec![w("a", 0.0, 0.0, 100.0, 1), w("b", 0.0, 50.0, 100.0, 2)], "a\nb");
        assert!(!PageLines::from_page_ocr(&flat).has_confidence);
    }

    #[test]
    fn file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let sc = dir.path().join("sidecar.txt");
        let l = letter();
        write_for_sidecar(&sc, "tesseract", &[Some(&l), None]).unwrap();
        let f = read_for_dir(dir.path()).unwrap();
        assert_eq!(f.engine, "tesseract");
        assert_eq!(f.pages.len(), 2);
        assert!(f.pages[1].is_none());
        assert!(read_for_dir(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn selection_by_mode() {
        let f = LinesFile::from_pages("tesseract", &[Some(&letter()), Some(&letter())]);
        // only the margin note goes as a crop, per page
        let s = select(VisionMode::UncertainLines, Sensitivity::Normal, Some(&f), 400, 2, 10);
        assert_eq!(s.crops, vec![(1, 10), (2, 10)]);
        assert!(s.pages.is_empty());
        // one uncertain line out of 11 lines is not an uncertain page (share of characters small)
        let s = select(VisionMode::UncertainPages, Sensitivity::Normal, Some(&f), 400, 2, 10);
        assert!(s.is_empty());
        // "gründlich" (share 0.15): 13 of 173 characters uncertain still keeps the page certain
        let s = select(VisionMode::UncertainPages, Sensitivity::Thorough, Some(&f), 400, 2, 10);
        assert!(s.pages.is_empty());
        // always: every page, capped by the page limit
        let s = select(VisionMode::Always, Sensitivity::Normal, Some(&f), 400, 2, 1);
        assert_eq!((s.pages.clone(), s.skipped_pages), (vec![1], 1));
        assert!(select(VisionMode::Off, Sensitivity::Normal, Some(&f), 0, 2, 10).is_empty());
        // without line data: old rule (fewer than 50 characters per page → all pages)
        assert_eq!(select(VisionMode::UncertainLines, Sensitivity::Normal, None, 80, 2, 10).pages, vec![1, 2]);
        assert!(select(VisionMode::UncertainPages, Sensitivity::Normal, None, 400, 2, 10).is_empty());
    }

    #[test]
    fn handwriting_page_goes_whole() {
        // handwriting-only page: few characters, all uncertain → whole page in both uncertain modes
        let hw = page(vec![w("Eirkaufe", 100.0, 100.0, 20.0, 1), w("Mlch", 100.0, 200.0, 15.0, 2)], "Eirkaufe\nMlch");
        let f = LinesFile::from_pages("tesseract", &[Some(&hw)]);
        assert_eq!(select(VisionMode::UncertainLines, Sensitivity::Sparing, Some(&f), 12, 1, 10).pages, vec![1]);
        assert_eq!(select(VisionMode::UncertainPages, Sensitivity::Sparing, Some(&f), 12, 1, 10).pages, vec![1]);
    }

    #[test]
    fn engines_have_own_thresholds() {
        let t = thresholds("tesseract", Sensitivity::Normal);
        let p = thresholds("ppocr", Sensitivity::Normal);
        assert!(p.line_conf > t.line_conf);
        assert!(thresholds("tesseract", Sensitivity::Sparing).line_conf < thresholds("tesseract", Sensitivity::Thorough).line_conf);
        // Regression (2026-11-26): PP-OCR misread handwriting lines at 94–97; "normal" must catch them
        // and must not flag clean lines (≥ 98).
        for (conf, uncertain) in [(94.4, true), (95.3, true), (96.5, true), (98.3, false), (99.8, false)] {
            assert_eq!(conf < p.line_conf, uncertain, "{conf}");
        }
        assert!(thresholds("ppocr", Sensitivity::Sparing).line_conf < thresholds("ppocr", Sensitivity::Thorough).line_conf);
    }

    #[test]
    fn crops_scale_and_reject_other_orientation() {
        let p = PageLines::from_page_ocr(&letter());
        // raster at a lower resolution (same aspect ratio)
        let raster = image::DynamicImage::ImageRgb8(image::RgbImage::new(1240, 1754));
        let (img, area) = crop_line(&raster, &p, &p.lines[10]).unwrap();
        assert!(img.height() >= 48);
        assert!(area > 0.0 && area < 0.01);
        // rotated raster (landscape) → no crop
        let rot = image::DynamicImage::ImageRgb8(image::RgbImage::new(1754, 1240));
        assert!(crop_line(&rot, &p, &p.lines[10]).is_none());
    }

    #[test]
    fn merge_pages_and_lines() {
        let f = LinesFile::from_pages("tesseract", &[Some(&letter()), Some(&page(vec![w("x", 0.0, 0.0, 10.0, 1)], "Seite zwei"))]);
        let mut lines = BTreeMap::new();
        lines.insert(1, vec![("Bifte anrufen!".to_string(), "Bitte anrufen!".to_string()), ("fehlt".to_string(), "nie".to_string())]);
        let mut pages = BTreeMap::new();
        pages.insert(2, "Seite 2 von der KI".to_string());
        let (text, applied) = merge_content(&f, &pages, &lines).unwrap();
        assert!(text.contains("Bitte anrufen!") && !text.contains("Bifte"));
        assert!(text.ends_with("Seite 2 von der KI"));
        assert_eq!(applied, vec![(1, 0)]);
        // a page without classic text → no per-page merge
        let g = LinesFile { version: 1, engine: "tesseract".into(), pages: vec![None] };
        assert!(merge_content(&g, &pages, &lines).is_none());
    }

    #[test]
    fn replace_in_order() {
        let (t, a) = replace_lines("a b\nc a b\n", &[("a b".into(), "X".into()), ("a b".into(), "Y".into())]);
        assert_eq!(t, "X\nc Y\n");
        assert_eq!(a, vec![0, 1]);
    }
}
