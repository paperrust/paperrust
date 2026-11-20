//! AI text recognition (vision OCR) as a plugged-in service.
//!
//! pl-consumer (ingestion, reprocessing) should not depend on pl-ai. Therefore this module only
//! describes the interface; pl-server plugs in the implementation from pl-ai at startup
//! (`pl_ai::server::assist::{vision_wanted, vision_ocr}`). Without a plugged-in service (build without AI,
//! tests) text recognition stays unchanged.
//!
//! Flow in the consumer: normal recognition (Tesseract/text layer) → [`VisionOcr::plan`] with the number
//! of recognized characters → only for `Some(n)` render at most `n` pages as PNG/JPEG →
//! [`VisionOcr::recognize`] → non-empty text replaces the content. AI errors never lead to
//! aborting ingestion (the normal text stays).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Return type of the service methods (object-safe).
pub type VisionFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Largest page image the consumer sends (8 MiB).
pub const MAX_PAGE_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Resolution of the page images (150–200 dpi; higher barely helps the models).
pub const PAGE_IMAGE_DPI: f64 = 170.0;

/// A page image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImage {
    /// `image/png`, `image/jpeg` or `image/webp`.
    pub mime: String,
    pub data: Vec<u8>,
}

/// AI text recognition.
pub trait VisionOcr: Send + Sync {
    /// Should AI text recognition run? `requested` = user chose "AI text recognition" for this
    /// document; `ocr_chars` = characters from normal recognition; `pages` = page count.
    /// `None` = no, `Some(n)` = yes, render at most `n` pages.
    fn plan(&self, requested: bool, ocr_chars: Option<usize>, pages: usize) -> VisionFuture<'_, Option<usize>>;

    /// Recognize pages. `Ok(None)` = AI/option switched off in the meantime; `Ok(Some(text))` = text of all
    /// sent pages (separated by a blank line).
    fn recognize(&self, pages: Vec<PageImage>) -> VisionFuture<'_, Result<Option<String>, String>>;
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
}
