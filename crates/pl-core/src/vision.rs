//! AI text recognition (vision OCR, "KI-Abschrift") as a plugged-in service.
//!
//! pl-consumer (ingestion, reprocessing) should not depend on pl-ai. Therefore this module only
//! describes the interface; pl-server plugs in the implementation from pl-ai at startup
//! (`pl_ai::server::assist`). Without a plugged-in service (build without AI, tests) text
//! recognition stays unchanged.
//!
//! Principle (docs/OCR-NEXT.md 2.8): the classic OCR always runs, the LLM only adds to it.
//!
//! Flow in the consumer: normal recognition (OCR/text layer) → [`VisionOcr::plan`] (mode,
//! sensitivity, limits) → selection of whole pages and/or uncertain lines from the classic OCR
//! confidences (pl-ocr `uncertain`) → page images / line crops → [`VisionOcr::recognize`] → accepted
//! texts are merged into the content per page / per line (loops and token-limit hits keep the
//! classic text) → after storing the document [`VisionOcr::record`] keeps the transcript (original
//! OCR lines, AI text, model) and labels the document as "KI-Abschrift". The PDF text layer always
//! stays from the classic OCR. AI errors never abort ingestion.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Return type of the service methods (object-safe).
pub type VisionFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Largest page image the consumer sends (8 MiB).
pub const MAX_PAGE_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Resolution of the page images before downscaling to [`VisionPlan::max_side`].
pub const PAGE_IMAGE_DPI: f64 = 170.0;

/// Default long side of a page image in pixels. Measured (results/ocr-eval/2026-11-24): on CPU about
/// 3x faster than 2048 px with little loss.
pub const DEFAULT_MAX_SIDE: u32 = 1540;

/// When the LLM reads along (user setting "KI-Abschrift").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VisionMode {
    /// Only classic recognition (manual "Mit KI neu lesen" still works).
    Off,
    /// Only uncertain lines/regions are cropped and sent ("Nur unsichere Stellen").
    UncertainLines,
    /// Whole page if the page as a whole is uncertain ("Ganze Seite, wenn unsicher").
    #[default]
    UncertainPages,
    /// Every page additionally through the LLM ("Immer").
    Always,
}

/// How eagerly lines/pages count as uncertain (only for the "uncertain" modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// "sparsam": only clearly bad text.
    Sparing,
    #[default]
    Normal,
    /// "gründlich": also borderline text.
    Thorough,
}

/// Decision of the service for one document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisionPlan {
    pub mode: VisionMode,
    pub sensitivity: Sensitivity,
    /// At most this many whole pages per document.
    pub max_pages: usize,
    /// Long side of whole-page images in pixels.
    pub max_side: u32,
}

/// An encoded image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImage {
    /// `image/png`, `image/jpeg` or `image/webp`.
    pub mime: String,
    pub data: Vec<u8>,
}

/// A whole page to transcribe.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionPage {
    /// 1-based page number.
    pub page: usize,
    pub image: PageImage,
    /// Visible characters of the classic OCR of this page (`None` = unknown); used by the loop guard.
    pub ocr_chars: Option<usize>,
}

/// A crop of an uncertain line/region.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionCrop {
    /// 1-based page number.
    pub page: usize,
    /// Index of the line in the page's OCR line list.
    pub line: usize,
    pub image: PageImage,
    /// Crop area / page area (0..1); the daily budget counts crops proportionally.
    pub area_fraction: f32,
    /// Text the classic OCR read there.
    pub ocr_text: String,
}

/// Input of [`VisionOcr::recognize`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VisionRequest {
    pub pages: Vec<VisionPage>,
    pub crops: Vec<VisionCrop>,
}

/// Outcome of one page or crop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Text accepted (replaces the classic text of the page/line).
    Accepted,
    /// Repetition loop detected; classic text stays.
    Loop,
    /// Output stopped at the token limit; classic text stays.
    TokenLimit,
    /// Implausibly long compared with the classic OCR; classic text stays.
    TooLong,
    /// Empty answer; classic text stays.
    Empty,
    /// Not sent (daily budget/page limit).
    Skipped,
}

/// Result of one page (`line == None`) or crop.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemResult {
    pub page: usize,
    pub line: Option<usize>,
    pub status: ItemStatus,
    /// Cleaned model text (for loops: truncated before the repetition). Use only if `Accepted`.
    pub text: String,
}

/// Output of [`VisionOcr::recognize`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VisionResponse {
    /// Model that answered.
    pub model: String,
    /// Whether the backend is not on the local network (documents left the house).
    pub remote: bool,
    pub items: Vec<ItemResult>,
}

/// One stored transcript entry (page or line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub page: usize,
    /// `None` = whole page.
    pub line: Option<usize>,
    /// Line box in OCR pixels `[x0, y0, x1, y1]` (crops only).
    pub bbox: Option<[f32; 4]>,
    /// Classic OCR text (kept for comparison and to revert).
    pub ocr_text: String,
    /// AI text (also stored when not accepted, for diagnosis).
    pub ai_text: String,
    pub status: ItemStatus,
}

/// Transcript of one AI run on a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub model: String,
    pub mode: VisionMode,
    pub entries: Vec<TranscriptEntry>,
}

impl Transcript {
    /// Number of entries whose AI text went into the content.
    pub fn accepted(&self) -> usize {
        self.entries.iter().filter(|e| e.status == ItemStatus::Accepted).count()
    }
}

/// AI text recognition.
pub trait VisionOcr: Send + Sync {
    /// Should AI text recognition run for this document, and how? `requested` = user chose
    /// "Mit KI neu lesen"/"AI text recognition" for this document (then [`VisionMode::Always`]).
    /// `None` = AI off / not configured / mode off without request.
    fn plan(&self, requested: bool) -> VisionFuture<'_, Option<VisionPlan>>;

    /// Transcribe pages and crops. `Ok(None)` = AI/option switched off in the meantime.
    fn recognize(&self, request: VisionRequest) -> VisionFuture<'_, Result<Option<VisionResponse>, String>>;

    /// Keep the transcript of a stored document (original OCR lines, AI text) and label the document.
    /// Default: nothing.
    fn record(&self, document_id: i64, transcript: Transcript) -> VisionFuture<'_, ()> {
        let _ = (document_id, transcript);
        Box::pin(async {})
    }
}

/// Plugged-in service (`state.set_service(VisionOcrService(...))` or field in the consumer).
#[derive(Clone)]
pub struct VisionOcrService(pub Arc<dyn VisionOcr>);

/// Non-whitespace characters of a recognized text (measure for "little text").
pub fn text_chars(text: Option<&str>) -> usize {
    text.map(|t| t.chars().filter(|c| !c.is_whitespace()).count()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_visible_chars() {
        assert_eq!(text_chars(None), 0);
        assert_eq!(text_chars(Some(" a b\n\tc ")), 3);
    }

    #[test]
    fn transcript_counts_accepted() {
        let e = |s| TranscriptEntry { page: 1, line: None, bbox: None, ocr_text: String::new(), ai_text: String::new(), status: s };
        let t = Transcript { model: "m".into(), mode: VisionMode::Always, entries: vec![e(ItemStatus::Accepted), e(ItemStatus::Loop)] };
        assert_eq!(t.accepted(), 1);
        assert_eq!(serde_json::to_value(VisionMode::UncertainLines).unwrap_or_default(), "uncertain_lines");
    }
}
