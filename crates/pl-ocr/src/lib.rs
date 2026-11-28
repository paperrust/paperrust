//! pl-ocr – replacement for OCRmyPDF in the Python-free worker (see docs/contract/pl-ocr.md).
//!
//! Structure:
//! - [`config`]: `OcrConfig` (environment `PAPERLESS_OCR_*` + `ApplicationConfiguration` overrides).
//! - [`args`]: `construct_ocrmypdf_parameters` 1:1 (ordered argument list like the Python dict).
//! - [`imageinfo`]: image header data like Pillow (`info["dpi"]`, `mode` for alpha, size).
//! - [`parser`]: decision logic of `RasterisedDocumentParser.parse()` and
//!   `should_produce_archive`, independent of the OCR tool (trait [`parser::OcrBackend`]).
//! - [`engine`]: trait [`engine::OcrEngine`] (image → words with boxes + text), Tesseract CLI.
//! - [`engine_select`]: `PAPERRUST_OCR_ENGINE` (default Tesseract), language mapping for OS engines;
//!   [`winocr`]: `Windows.Media.Ocr` (Windows only), [`macocr`]: placeholder Apple Vision.
//! - [`render`]: trait [`render::PdfRenderer`] (PDF page → image), PDFium via `libloading`.
//! - [`pdfwrite`]: trait [`pdfwrite::ArchiveBuilder`] – invisible text layer, image pages, PDF/A stamp.
//! - [`native`]: [`native::NativeOcrmypdf`] – executes the OCRmyPDF arguments with engine/renderer.
//! - [`preprocess`]: straightening (deskew) and cleaning (clean, unpaper replacement) before OCR.
//! - [`pipeline`]/[`permits`]: page-parallel OCR; process-wide page permits shared by all
//!   documents (a lone document uses all cores, concurrent documents share them).
//! - [`thumbnail`]: `make_thumbnail_from_pdf` (WebP, 500 px wide).
//! - [`pdfa`]: PDF/A cleanup and self-check (replacement for Ghostscript `-dPDFA`);
//!   [`glyphless`]: embedded text layer font; [`color`]: `OCR_COLOR_CONVERSION_STRATEGY`;
//!   [`optimize`]/[`jbig2`]/[`pdfimage`]: archive size reduction like `ocrmypdf --optimize`.
//! - [`remote`]: remote OCR via Azure AI Document Intelligence (`RemoteDocumentParser`).
//! - [`vision_pages`]: page images (PNG/JPEG) for AI text recognition (pl-core `vision`).
//! - [`ppocr`]: PP-OCRv6 engine (`PAPERRUST_OCR_ENGINE=ppocr`) and the page orientation model
//!   (`PAPERRUST_OCR_ORIENTATION`), on top of the `pl-ppocr` crate.
//! - [`uncertain`]: uncertain lines/pages of the classic OCR for the AI transcription.

pub mod args;
pub mod color;
pub mod config;
pub mod engine;
pub mod engine_select;
pub mod glyphless;
pub mod icc;
pub mod imageinfo;
pub mod jbig2;
pub mod macocr;
pub mod native;
pub mod optimize;
pub mod parser;
pub mod pdfa;
pub mod pdfa_fonts;
pub mod pdfimage;
pub mod pdfwrite;
pub mod permits;
pub mod pipeline;
pub mod ppocr;
pub mod preprocess;
pub mod remote;
pub mod render;
pub mod tesslib;
pub mod text;
pub mod thumbnail;
pub mod uncertain;
pub mod vision_pages;
pub mod winocr;

pub use args::OcrArgs;
pub use config::{OcrConfig, OcrSettings};
pub use parser::{OcrBackend, ParseOutcome, RasterisedParser};

/// Errors of the OCR layer. The variants map the exceptions that upstream distinguishes in
/// `RasterisedDocumentParser.parse()` (OCRmyPDF exceptions + `ParseError`).
#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    /// `documents.parsers.ParseError` (text like upstream).
    #[error("{0}")]
    Parse(String),
    /// `ocrmypdf.EncryptedPdfError`.
    #[error("EncryptedPdfError: Input PDF is encrypted")]
    EncryptedPdf,
    /// `ocrmypdf.exceptions.DigitalSignatureError`.
    #[error("DigitalSignatureError: Input PDF has a digital signature")]
    DigitalSignature,
    /// `ocrmypdf.exceptions.PriorOcrFoundError`.
    #[error("PriorOcrFoundError: {0}")]
    PriorOcrFound(String),
    /// `ocrmypdf.InputFileError` (incl. `TaggedPDFError`).
    #[error("InputFileError: {0}")]
    InputFile(String),
    /// `ocrmypdf.SubprocessOutputError` (e.g. Tesseract/renderer failed).
    #[error("SubprocessOutputError: {0}")]
    SubprocessOutput(String),
    /// PIL `DecompressionBombError`.
    #[error("DecompressionBombError: {0}")]
    DecompressionBomb(String),
    /// Tool (Tesseract/PDFium) is missing or not configured.
    #[error("EngineUnavailable: {0}")]
    Unavailable(String),
    /// Invalid configuration (Python `ValueError` for choices/int()).
    #[error("ValueError: {0}")]
    Config(String),
    /// Other errors (Python: `e.__class__.__name__: e`).
    #[error("{0}")]
    Other(String),
    #[error("OSError: {0}")]
    Io(#[from] std::io::Error),
}

impl OcrError {
    /// Python class name for `f"{e.__class__.__name__}: {e!s}"`.
    pub fn class_name(&self) -> &'static str {
        match self {
            OcrError::Parse(_) => "ParseError",
            OcrError::EncryptedPdf => "EncryptedPdfError",
            OcrError::DigitalSignature => "DigitalSignatureError",
            OcrError::PriorOcrFound(_) => "PriorOcrFoundError",
            OcrError::InputFile(_) => "InputFileError",
            OcrError::SubprocessOutput(_) => "SubprocessOutputError",
            OcrError::DecompressionBomb(_) => "DecompressionBombError",
            OcrError::Unavailable(_) => "EngineUnavailable",
            OcrError::Config(_) => "ValueError",
            OcrError::Other(_) => "Exception",
            OcrError::Io(_) => "OSError",
        }
    }

    /// `str(e)` without class prefix.
    pub fn message(&self) -> String {
        match self {
            OcrError::Parse(m)
            | OcrError::PriorOcrFound(m)
            | OcrError::InputFile(m)
            | OcrError::SubprocessOutput(m)
            | OcrError::DecompressionBomb(m)
            | OcrError::Unavailable(m)
            | OcrError::Config(m)
            | OcrError::Other(m) => m.clone(),
            OcrError::EncryptedPdf => "Input PDF is encrypted".into(),
            OcrError::DigitalSignature => "Input PDF has a digital signature".into(),
            OcrError::Io(e) => e.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, OcrError>;
