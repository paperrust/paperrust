// Interfaces to the worker building blocks. The consumer knows only these traits; the default
// implementations in `native` connect pl-parsers, pl-ocr, pl-matching, pl-classifier, pl-indexer
// and pl-queue. Tests (and later variants, e.g. remote OCR, Gotenberg) replace them.
//
// upstream counterparts: ParserProtocol/registry (paperless/parsers), get_date_parser
// (documents/plugins/date_parsing), load_classifier (documents/classifier.py), add_to_index
// (documents/signals/handlers.py), ProgressManager (documents/plugins/helpers.py),
// read_barcode_values/scan_pdf (documents/barcodes.py), execute_email_action/execute_webhook_action
// (documents/workflows/actions.py), CollatePlugin (documents/double_sided.py, pikepdf).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
pub use futures::future::BoxFuture;
use sqlx::PgConnection;

pub use pl_pdfops::barcodes::ScanSettings as BarcodeScanSettings;
pub use pl_queue::events::{ProgressStatus, ViewPermissions};

use crate::data::DocumentMetadataOverrides;
use crate::db::DocRow;

/// The chosen parser (`get_parser_for_file`) with the properties that the consumer reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParserInfo {
    /// upstream class name (e.g. `RasterisedDocumentParser`)
    pub class_name: String,
    /// `parser.name`
    pub name: String,
    pub can_produce_archive: bool,
    pub requires_pdf_rendition: bool,
    pub uses_remote_service: bool,
}

/// Result of `parse()` + `get_text()/get_date()/get_archive_path()`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedOutput {
    pub text: Option<String>,
    pub date: Option<DateTime<FixedOffset>>,
    pub archive_path: Option<PathBuf>,
}

/// Error of a building block: `Parse` = upstream `ParseError` (text = `str(e)`), `Other` = other exception.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StageError {
    #[error("{0}")]
    Parse(String),
    #[error("{0}")]
    Other(String),
}

/// Parameters of a parse call.
#[derive(Debug, Clone)]
pub struct ParseRequest<'a> {
    pub path: &'a Path,
    pub mime_type: &'a str,
    pub filename: &'a str,
    pub mailrule_id: Option<i64>,
    /// Working directory of the parser (cleaned up by the consumer)
    pub tempdir: &'a Path,
}

/// Parser registry + text extraction + OCR + thumbnail (pl-parsers, pl-ocr).
pub trait DocumentParsing: Send + Sync {
    /// `get_parser_registry().get_parser_for_file(mime, filename, path, allow_remote=)`.
    fn parser_for(&self, mime_type: &str, filename: &str, path: &Path, allow_remote: bool) -> Option<ParserInfo>;
    /// `should_produce_archive(parser, mime_type, path)`.
    fn should_produce_archive(&self, parser: &ParserInfo, mime_type: &str, path: &Path) -> bool;
    /// `parser.parse(path, mime, produce_archive=)` → text, date, archive path.
    fn parse(&self, parser: &ParserInfo, req: &ParseRequest<'_>, produce_archive: bool) -> Result<ParsedOutput, StageError>;
    /// `parser.get_thumbnail(path, mime)` (WebP file in the tempdir).
    fn thumbnail(&self, parser: &ParserInfo, req: &ParseRequest<'_>, archive: Option<&Path>) -> Result<PathBuf, StageError>;
    /// Like [`Self::thumbnail`], with the text just extracted (Office/mail without a PDF version:
    /// text thumbnail instead of the default image). Default: `thumbnail`.
    fn thumbnail_with_text(
        &self,
        parser: &ParserInfo,
        req: &ParseRequest<'_>,
        archive: Option<&Path>,
        text: Option<&str>,
    ) -> Result<PathBuf, StageError> {
        let _ = text;
        self.thumbnail(parser, req, archive)
    }
    /// `parser.get_page_count(path, mime)`.
    fn page_count(&self, parser: &ParserInfo, req: &ParseRequest<'_>, archive: Option<&Path>) -> Option<i64>;
    /// PaperRust: at most `max_pages` page images (PNG/JPEG/WebP, ≤ 8 MiB, ~170 dpi) for the
    /// AI text recognition (pl-core `vision`). Default: none (AI text recognition is skipped).
    fn page_images(&self, path: &Path, mime_type: &str, max_pages: usize) -> Result<Vec<pl_core::vision::PageImage>, StageError> {
        let _ = (path, mime_type, max_pages);
        Ok(Vec::new())
    }
}

/// `get_date_parser()` + `next(date_parser.parse(filename, text), None)`.
pub trait DateDetector: Send + Sync {
    fn detect(&self, filename: &str, content: &str) -> Option<DateTime<FixedOffset>>;
}

/// Classifier (pl-classifier), interface like `pl_matching::Classifier`.
pub type SharedClassifier = Arc<dyn pl_matching::Classifier + Send + Sync>;

/// `load_classifier()`.
pub trait ClassifierSource: Send + Sync {
    fn load(&self) -> Option<SharedClassifier>;
}

/// Result of an index write operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexOutcome {
    Written,
    Skipped,
    /// Lock not obtained → upstream schedules `index_document` with countdown=60.
    Deferred,
}

/// `get_backend().add_or_update(document)` (pl-indexer).
pub trait SearchIndex: Send + Sync {
    fn add_or_update<'a>(&'a self, pool: &'a sqlx::PgPool, document_id: i64) -> BoxFuture<'a, Result<IndexOutcome, String>>;
}

/// A status message (`ProgressManager.send_progress`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressUpdate {
    pub filename: Option<String>,
    pub task_id: Option<String>,
    pub status: ProgressStatus,
    pub message: String,
    pub current_progress: i64,
    pub max_progress: i64,
    pub document_id: Option<i64>,
    pub perms: ViewPermissions,
}

/// Status messages to the UI (pl-queue `EventBus`/WebSocket).
pub trait ProgressSink: Send + Sync {
    fn send(&self, update: ProgressUpdate);
}

/// A detected barcode (`documents.barcodes.Barcode`, page 0-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Barcode {
    pub page: i64,
    pub value: String,
    pub format: String,
}

/// `scan_pdf` (barcodes of all pages). Upstream: pdf2image/pypdfium2 + zxing-cpp.
pub trait BarcodeScanner: Send + Sync {
    /// `false` = no scanner available (the plugin then does not run).
    fn available(&self) -> bool;
    /// `scan_pdf(pdf, settings, work_dir)` with `barcode_dpi`/`barcode_upscale`/`barcode_max_pages`
    /// (environment + ApplicationConfiguration, read per ingestion run).
    fn scan(&self, pdf: &Path, tempdir: &Path, settings: &BarcodeScanSettings) -> Result<Vec<Barcode>, StageError>;
    /// `BarcodePlugin.detect`: like `scan`, errors logged and treated as "no barcodes".
    fn detect(&self, pdf: &Path, tempdir: &Path, settings: &BarcodeScanSettings) -> Vec<Barcode> {
        match self.scan(pdf, tempdir, settings) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(target: "paperless.barcodes", "Exception during barcode scanning: {e}");
                Vec::new()
            }
        }
    }
    /// Separate pages (`separate_pages`): list of the generated PDF files. `name_stem` =
    /// `input_doc.original_file.stem`, `stats_source` = original (file times).
    fn split(
        &self,
        pdf: &Path,
        separator_pages: &[(i64, bool)],
        tempdir: &Path,
        name_stem: &str,
        stats_source: &Path,
    ) -> Result<Vec<PathBuf>, StageError>;
}

/// PDF page operations for double-sided scans (`CollatePlugin`) and TIFF→PDF.
pub trait PdfPageOps: Send + Sync {
    /// Insert the pages of `even` in reverse order between the pages of `odd` and
    /// write to `out`. `Err(StageError::Other(text))` with upstream text if `even` has more
    /// pages.
    fn collate(&self, odd: &Path, even: &Path, out: &Path) -> Result<(), StageError>;
    /// `convert_from_tiff_to_pdf`.
    fn tiff_to_pdf(&self, tiff: &Path, tempdir: &Path) -> Result<PathBuf, StageError>;
}

/// Context of an e-mail/webhook action (`build_workflow_action_context`).
#[derive(Debug, Clone, PartialEq)]
pub struct ActionContext {
    pub workflow_action_id: i64,
    pub action_type: i64,
    pub document_id: Option<i64>,
    pub title: String,
    pub original_file: PathBuf,
    pub trigger_type: i64,
}

/// Full input of an e-mail/webhook/password action from the consumer
/// (`run_workflows(..., document, overrides)` → `build_workflow_action_context(document, overrides)`).
///
/// `ActionContext` stays field-identical (other crates build it directly); the details that upstream
/// reads in addition are here:
/// * `overrides`: `Some` for the ingestion trigger (`use_overrides`) – state of the overrides **at the
///   time of the action** (after the previous actions of the same run); `None` otherwise.
/// * `document`: for the "document added" trigger the in-memory instance (with the fields assigned
///   so far but not yet saved, such as correspondent, title, owner).
/// * `mime_type`: `document.mime_type` (`ConsumableDocument.mime_type` or `Document.mime_type`).
#[derive(Debug, Clone, PartialEq)]
pub struct ActionInput {
    pub ctx: ActionContext,
    pub overrides: Option<DocumentMetadataOverrides>,
    pub document: Option<DocRow>,
    pub mime_type: String,
}

/// Side-effect actions of workflows (e-mail, webhook, remove password). Default: stub
/// that only logs (no network, docs/STANDALONE.md guardrail 5).
///
/// The consumer calls the `*_in` methods: asynchronously, with the connection of the running
/// ingestion transaction (sees the not yet committed document), just as upstream executes the
/// actions synchronously in the signal. Their default implementation calls the synchronous methods
/// (which e.g. pl-documents-write uses for "document updated").
pub trait WorkflowEffects: Send + Sync {
    fn email(&self, ctx: &ActionContext);
    fn webhook(&self, ctx: &ActionContext);
    fn password_removal(&self, ctx: &ActionContext);

    /// "Ingestion finished" hook (`document_consumption_finished`), after the commit of the
    /// new document `doc_id`. `original_file` is the input file that also appears in the
    /// [`ActionContext`] of the ingestion trigger (where `document_id` is still `None`):
    /// this is how actions that registered at the ingestion trigger for later (upstream
    /// `document_consumption_finished.connect(handler)`, e.g. "remove password") find their document.
    fn consumption_finished(&self, doc_id: i64, original_file: &Path) {
        let _ = (doc_id, original_file);
    }

    /// `execute_email_action` with full context inside the transaction.
    fn email_in<'a>(&'a self, conn: &'a mut PgConnection, input: &'a ActionInput) -> BoxFuture<'a, ()> {
        let _ = conn;
        Box::pin(async move { self.email(&input.ctx) })
    }
    /// `execute_webhook_action` with full context inside the transaction.
    fn webhook_in<'a>(&'a self, conn: &'a mut PgConnection, input: &'a ActionInput) -> BoxFuture<'a, ()> {
        let _ = conn;
        Box::pin(async move { self.webhook(&input.ctx) })
    }
    /// `execute_password_removal_action`.
    fn password_removal_in<'a>(&'a self, conn: &'a mut PgConnection, input: &'a ActionInput) -> BoxFuture<'a, ()> {
        let _ = conn;
        Box::pin(async move { self.password_removal(&input.ctx) })
    }
}

/// External scripts (`run_subprocess`): `PRE_CONSUME_SCRIPT`/`POST_CONSUME_SCRIPT`.
pub trait ScriptRunner: Send + Sync {
    fn run(&self, script: &Path, env: &[(String, String)]) -> Result<(), String>;
}
