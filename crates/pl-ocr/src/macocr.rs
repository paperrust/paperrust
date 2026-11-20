//! Apple Vision (`VNRecognizeTextRequest`) as OCR engine – **not implemented yet**.
//!
//! There is no Mac here: an implementation could only be compiled via `cargo check --target aarch64-apple-darwin`,
//! not run or tested.
//! Therefore only a placeholder that reports itself as "not available"; `PAPERRUST_OCR_ENGINE=macos`
//! falls back to Tesseract (warning). Design for the later implementation:
//! `docs/contract/pl-ocr.md`, section "OS-native OCR engines" → "macOS Vision (design)".

use crate::{OcrError, Result};

/// Reason why the Vision engine is not available.
pub fn unavailable_reason() -> String {
    if cfg!(target_os = "macos") {
        "PAPERRUST_OCR_ENGINE=macos: the Apple Vision engine is not implemented yet; using tesseract".into()
    } else {
        "PAPERRUST_OCR_ENGINE=macos: the Apple Vision engine exists only on macOS (and is not implemented yet); using tesseract".into()
    }
}

/// Placeholder for the Vision engine (macOS only).
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct MacVision;

#[cfg(target_os = "macos")]
impl MacVision {
    /// Always [`OcrError::Unavailable`] (see module description).
    pub fn new() -> Result<Self> {
        Err(OcrError::Unavailable(unavailable_reason()))
    }
}

/// Platform-independent check: `Err(Unavailable)` everywhere until the engine is implemented.
pub fn probe() -> Result<()> {
    Err(OcrError::Unavailable(unavailable_reason()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn stub_is_unavailable() {
        assert!(matches!(super::probe(), Err(crate::OcrError::Unavailable(_))));
    }
}
