//! pl-pdfops – PDF page operations, barcodes, TIFF→PDF, qpdf replacement and text thumbnails
//! for the Python-free worker (see docs/contract/pl-pdfops.md).
//!
//! Replaces pikepdf/qpdf (`documents/pdf_ops.py`, `barcodes.py::separate_pages`,
//! `consumer.py` qpdf cleaning), zxing-cpp + pdf2image (`barcodes.py::scan_pdf`), img2pdf
//! (`converters.py::convert_from_tiff_to_pdf`) and the Pillow text preview (`parsers/text.py`).
//! Pure Rust crates only (lopdf, rxing, image, ab_glyph); rasterising via pl-ocr `PdfRenderer`
//! (PDFium as a dynamic library, like OCRmyPDF/pypdfium2).
//!
//! Modules:
//! - [`pdf`]: lopdf basics (loading with password, qpdf page rotation, copying pages,
//!   `remove_unreferenced_resources`, saving).
//! - [`ops`]: 1:1 `documents/pdf_ops.py` (`rotate_pdf`, `remove_pages`, `build_pdfs`,
//!   `validate_page_operations`, `needs_decrypt`, `decrypt_pdf`, `PdfMerger`).
//! - [`bulk`]: pure decision logic from `documents/bulk_edit.py` (file names, overrides,
//!   source document selection) for pl-documents-write.
//! - [`barcodes`]: `documents/barcodes.py` (scan, separator pages, ASN, tags, splitting).
//! - [`convert`]: `convert_from_tiff_to_pdf` (img2pdf replacement).
//! - [`repair`]: `qpdf --replace-input` replacement.
//! - [`thumbnail`]: text thumbnail (WebP) like `TextDocumentParser.get_thumbnail`.
//! - [`scanner`]: ready-made building blocks for pl-consumer (`BarcodeScanner`/`PdfPageOps`).

pub mod barcodes;
pub mod bulk;
pub mod convert;
pub mod ops;
pub mod pdf;
pub mod repair;
pub mod scanner;
pub mod thumbnail;

pub use ops::{PageOperation, PageSpec, PdfMerger};

/// Error. The variants correspond to the Python exceptions that upstream distinguishes
/// (the texts are those of upstream, as far as they reach the outside).
#[derive(Debug, thiserror::Error)]
pub enum PdfOpsError {
    /// `ValueError` (z. B. `Page numbers start at 1, got 0`).
    #[error("{0}")]
    Value(String),
    /// `IndexError` (z. B. `Page 5 is out of range, the PDF has 3 pages`).
    #[error("{0}")]
    Index(String),
    /// `pikepdf.PasswordError` (`{pfad}: invalid password`).
    #[error("{0}")]
    Password(String),
    /// qpdf `RuntimeError` (e.g. rotation angle not a multiple of 90).
    #[error("{0}")]
    Runtime(String),
    /// `pikepdf.PdfError` (file not readable/broken).
    #[error("{0}")]
    Pdf(String),
    /// Encryption that lopdf does not support (e.g. public-key handler).
    #[error("{0}")]
    UnsupportedEncryption(String),
    /// Renderer/barcode reader/font missing.
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl PdfOpsError {
    /// Python class name (for logs/parity tests).
    pub fn kind(&self) -> &'static str {
        match self {
            PdfOpsError::Value(_) => "ValueError",
            PdfOpsError::Index(_) => "IndexError",
            PdfOpsError::Password(_) => "PasswordError",
            PdfOpsError::Runtime(_) => "RuntimeError",
            PdfOpsError::Pdf(_) | PdfOpsError::UnsupportedEncryption(_) => "PdfError",
            PdfOpsError::Unavailable(_) => "Unavailable",
            PdfOpsError::Io(_) => "OSError",
        }
    }
}

pub type Result<T, E = PdfOpsError> = std::result::Result<T, E>;

/// `copy_basic_file_stats`: transfer atime/mtime from `source` to `dest`
/// (permission errors are ignored like upstream).
pub fn copy_basic_file_stats(source: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(source)?;
    let mut times = std::fs::FileTimes::new();
    if let Ok(a) = meta.accessed() {
        times = times.set_accessed(a);
    }
    if let Ok(m) = meta.modified() {
        times = times.set_modified(m);
    }
    match std::fs::File::options().write(true).open(dest) {
        Ok(f) => match f.set_times(times) {
            Err(e) if e.kind() != std::io::ErrorKind::PermissionDenied => Err(e),
            _ => Ok(()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
        Err(e) => Err(e),
    }
}
