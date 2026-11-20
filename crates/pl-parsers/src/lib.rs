//! pl-parsers – worker building block: file type detection, text extraction without Tika, metadata.
//!
//! Pure library (no DB, no routes). Contract: `docs/contract/pl-parsers.md`.
//!
//! upstream: src/paperless/parsers/__init__.py@v3.4.2 (ParserProtocol, MetadataEntry, ParserContext)
//!
//! Layout:
//! - [`mime`]: MIME detection like libmagic (`magic.from_file(path, mime=True)`) for all
//!   formats paperless supports.
//! - [`registry`]: parser selection by score like `ParserRegistry.get_parser_for_file`.
//! - [`text`], [`mail`], [`office`], [`html`], [`pdf`], [`msg`]: the actual parsers.
//! - [`limits`]: limits for size, time, unpacking, XML depth (zip bombs, broken files).

pub mod html;
pub mod limits;
pub mod mail;
pub mod mime;
pub mod msg;
pub mod office;
pub mod pdf;
pub mod pdftext;
pub mod pystr;
pub mod registry;
pub mod text;
mod xmlutil;
mod zipread;

use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use serde::Serialize;

pub use limits::{Deadline, Limits};
pub use registry::{ParserRegistry, RegistryConfig};

/// A metadata field like upstream `MetadataEntry` (TypedDict). Field order =
/// order of the keys in the JSON of `/api/documents/{id}/metadata/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct MetadataEntry {
    pub namespace: String,
    pub prefix: String,
    pub key: String,
    pub value: String,
}

impl MetadataEntry {
    pub fn new(
        namespace: impl Into<String>,
        prefix: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            prefix: prefix.into(),
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Parse error. Upstream raises `documents.parsers.ParseError(str)`; the texts of the
/// variants that mimic upstream messages are in `Display`.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// Like upstream `ParseError(message)` – text 1:1 (e.g. mail: `Could not parse {path}: ...`).
    #[error("{0}")]
    Upstream(String),
    /// Format is not (yet) supported (e.g. DOC/XLS/PPT, OCR needed).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// File is broken or has an invalid format.
    #[error("invalid file: {0}")]
    Invalid(String),
    /// A limit from [`Limits`] was exceeded (zip bomb, too large, too deep).
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),
    /// Time limit exceeded.
    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),
    /// Encrypted, not readable without a password.
    #[error("encrypted document")]
    Encrypted,
    /// Text can only be obtained via OCR (image, scanned PDF) – OCR is not part of this crate.
    #[error("OCR required")]
    OcrRequired,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ParseError>;

/// Context for a parse operation (upstream `ParserContext` + settings that upstream reads from
/// `settings.*`).
#[derive(Debug, Clone)]
pub struct ParseContext {
    /// `MailRule` ID (only relevant for the PDF layout of mails).
    pub mailrule_id: Option<i64>,
    /// Effective mail PDF layout (`MailRule.pdf_layout` of the rule, otherwise
    /// `PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT`); `None` = 1 (text, then HTML).
    pub mail_pdf_layout: Option<i64>,
    /// `PAPERLESS_TIME_ZONE` – for `make_aware` of naive dates (mail date, ODF creation date).
    pub time_zone: chrono_tz::Tz,
    pub limits: Limits,
    /// Working directory for generated files (archive PDF of office/mail, upstream `self._tempdir`).
    /// `None` = directory of the input file.
    pub work_dir: Option<PathBuf>,
}

impl Default for ParseContext {
    fn default() -> Self {
        Self {
            mailrule_id: None,
            mail_pdf_layout: None,
            work_dir: None,
            time_zone: chrono_tz::UTC,
            limits: Limits::default(),
        }
    }
}

/// Result of `parse()` (upstream: `get_text()`, `get_date()`, `get_archive_path()`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedDocument {
    pub text: String,
    pub date: Option<DateTime<FixedOffset>>,
    /// Path to a generated PDF version. This crate generates no PDFs → always `None`,
    /// unless a [`registry::PdfConverter`] is plugged in.
    pub archive_path: Option<PathBuf>,
}

/// Parser interface (upstream `ParserProtocol`). Stateless: `parse` returns the result
/// directly instead of via `get_text()`/`get_date()`.
pub trait DocumentParser: Send + Sync {
    /// Class name like upstream (e.g. `TextDocumentParser`) – for logs and parity tests.
    fn class_name(&self) -> &'static str;
    /// Display name (upstream `name`).
    fn name(&self) -> &'static str;
    /// `{mime: extension}` in upstream order.
    fn supported_mime_types(&self) -> &'static [(&'static str, &'static str)];
    /// Score or `None` (reject).
    fn score(&self, mime_type: &str, filename: &str, path: Option<&Path>) -> Option<i32>;
    fn can_produce_archive(&self) -> bool;
    fn requires_pdf_rendition(&self) -> bool;
    fn uses_remote_service(&self) -> bool {
        false
    }
    fn parse(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Result<ParsedDocument>;
    fn page_count(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Option<u32>;
    /// Must not fail (upstream: "Implementations must not raise").
    fn extract_metadata(
        &self,
        path: &Path,
        mime_type: &str,
        ctx: &ParseContext,
    ) -> Vec<MetadataEntry>;
    /// Thumbnail (WebP). Not part of this crate (needs renderer/font) → `Unsupported`.
    fn thumbnail(&self, _path: &Path, _mime_type: &str, _ctx: &ParseContext) -> Result<PathBuf> {
        Err(ParseError::Unsupported("thumbnail generation".into()))
    }
}

/// Reads a file respecting `limits.max_file_size`.
pub(crate) fn read_limited(path: &Path, limits: &Limits) -> Result<Vec<u8>> {
    let meta = std::fs::metadata(path)?;
    if meta.len() > limits.max_file_size {
        return Err(ParseError::LimitExceeded(format!(
            "file size {} > {}",
            meta.len(),
            limits.max_file_size
        )));
    }
    Ok(std::fs::read(path)?)
}
