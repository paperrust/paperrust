//! pl-ppocr – PP-OCR (PaddleOCR PP-OCRv6) text recognition and page orientation on ONNX models,
//! without Python. PaperRust addition (no upstream counterpart); plan: `docs/OCR-NEXT.md`,
//! description and known differences: `docs/OCR-PPOCR.md`.
//!
//! Structure:
//! - [`image`]: resize, perspective crop, normalisation (replaces the OpenCV calls of RapidOCR).
//! - [`geom`], [`db`]: DB detector post-processing (probability map → line quadrilaterals).
//! - [`ctc`]: CTC decoding, word boxes from the CTC time steps, per-word confidence.
//! - [`order`]: reading order by recursive XY-cut.
//! - [`pipeline`]: detector + recognizer on one page ([`pipeline::PpOcr`]).
//! - [`orient`]: page orientation classifier (0/90/180/270°).
//! - [`htr`]: DE·HTR v2 handwriting second pass for low-confidence lines.
//! - [`runtime`]: inference runtimes (ONNX Runtime loaded at run time).
//! - [`models`]: model files with fixed sources and SHA-256, lookup and verified installation.
//!
//! The crate knows nothing about PDFs or PaperRust settings; `pl-ocr` wraps it as an `OcrEngine`.

pub mod ctc;
pub mod db;
pub mod geom;
pub mod htr;
pub mod image;
pub mod models;
pub mod order;
pub mod orient;
pub mod pipeline;
pub mod runtime;

pub use image::Image;
pub use pipeline::{HtrPass, OcrParams, OcrWord, Page, PpOcr, TextLine};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The inference runtime (ONNX Runtime library) is missing or could not be loaded.
    #[error("runtime unavailable: {0}")]
    RuntimeMissing(String),
    /// A model file is missing.
    #[error("model missing: {0}")]
    ModelMissing(String),
    /// Loading or running a model failed.
    #[error("model error: {0}")]
    Model(String),
    #[error("checksum mismatch: {0}")]
    Checksum(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
