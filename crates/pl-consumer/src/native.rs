// Standard implementations of the interfaces from `stages` with the PaperRust building blocks.
//
// upstream: src/documents/consumer.py@v3.4.2 (should_produce_archive), src/paperless/parsers/*
//           (get_thumbnail/get_page_count per parser), src/documents/parsers.py@v3.4.2
//           (make_thumbnail_from_pdf: fallback to documents/resources/document.webp),
//           src/documents/plugins/date_parsing/__init__.py@v3.4.2 (get_date_parser),
//           src/documents/classifier.py@v3.4.2 (load_classifier), src/documents/utils.py@v3.4.2
//           (run_subprocess), src/paperless/parsers/text.py@v3.4.2 (TextDocumentParser.get_thumbnail),
//           src/documents/barcodes.py@v3.4.2 (BarcodePlugin.detect/separate_pages),
//           src/documents/converters.py@v3.4.2 (convert_from_tiff_to_pdf)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Utc};
use futures::future::BoxFuture;

use crate::settings::ConsumerSettings;
use crate::stages::*;

/// Default thumbnail (`documents/resources/document.webp`, unchanged from upstream).
pub static DEFAULT_THUMBNAIL: &[u8] = include_bytes!("../../../upstream/src/documents/resources/document.webp");

const RASTERISED: &str = "RasterisedDocumentParser";
const TEXT: &str = "TextDocumentParser";

/// pl-parsers-Registry + pl-ocr (Tesseract-CLI, PDFium).
pub struct NativeParsing {
    pub registry: pl_parsers::ParserRegistry,
    pub ocr: pl_ocr::OcrConfig,
    pub remote: Option<pl_ocr::config::RemoteOcrConfig>,
    pub tesseract: Option<pl_ocr::engine::TesseractCli>,
    pub pdfium: Option<pl_ocr::render::PdfiumRenderer>,
    pub time_zone: chrono_tz::Tz,
    pub limits: pl_parsers::Limits,
    /// `PAPERLESS_THUMBNAIL_FONT_NAME` (text thumbnails); if it is missing -> default image.
    pub thumbnail_font: Option<Arc<pl_pdfops::thumbnail::ThumbnailFont>>,
    /// Database for `MailRule.pdf_layout` (mail → PDF, gap E12); without it
    /// `PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT` applies.
    pub pool: Option<sqlx::PgPool>,
}

impl NativeParsing {
    /// From the environment (`PAPERLESS_OCR_*`, `PAPERRUST_TESSERACT_PATH`, `PAPERRUST_PDFIUM_PATH`) and
    /// the DB overrides (`paperless_applicationconfiguration`).
    pub fn new(ocr: pl_ocr::OcrConfig, time_zone: chrono_tz::Tz) -> Self {
        let tesseract = Some(pl_ocr::engine::TesseractCli::from_env()).filter(|t| t.is_available());
        let pdfium = pl_ocr::render::PdfiumRenderer::from_env().ok();
        let thumbnail_font = match pl_pdfops::thumbnail::ThumbnailFont::from_env() {
            Ok(f) => Some(Arc::new(f)),
            Err(e) => {
                tracing::debug!(target: "paperless.parsing", "Thumbnail font not available ({e}), text thumbnails use the default image");
                None
            }
        };
        let mut registry = pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default());
        // PaperRust: Office → PDF with WordCraft (child process), if enabled and installed
        crate::office::install(&mut registry);
        Self {
            registry,
            ocr,
            remote: None,
            tesseract,
            pdfium,
            time_zone,
            limits: pl_parsers::Limits::default(),
            thumbnail_font,
            pool: None,
        }
    }

    fn ctx(&self, mailrule_id: Option<i64>) -> pl_parsers::ParseContext {
        pl_parsers::ParseContext { mailrule_id, mail_pdf_layout: None, work_dir: None, time_zone: self.time_zone, limits: self.limits.clone() }
    }

    /// Effective mail PDF layout like upstream `MailDocumentParser.parse`: with a rule
    /// `MailRule.PdfLayout(rule.pdf_layout)` (0 = "System default" falls into the `_` branch there =
    /// text, then HTML), without a rule `EMAIL_PARSE_DEFAULT_LAYOUT` (default 1).
    fn mail_pdf_layout(&self, mailrule_id: Option<i64>) -> i64 {
        if let Some(id) = mailrule_id
            && let Some(v) = self.rule_pdf_layout(id)
        {
            return v;
        }
        pl_core::config::var("PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(1)
    }

    /// `MailRule.objects.get(pk=id).pdf_layout`. Runs in the parser (`spawn_blocking`): the query
    /// is executed on its own thread with the runtime handle.
    fn rule_pdf_layout(&self, id: i64) -> Option<i64> {
        let pool = self.pool.clone()?;
        let handle = tokio::runtime::Handle::try_current().ok()?;
        let r = std::thread::scope(|sc| {
            sc.spawn(move || {
                handle.block_on(async move {
                    sqlx::query_scalar::<_, i16>("SELECT pdf_layout FROM paperless_mail_mailrule WHERE id = $1").bind(id).fetch_optional(&pool).await
                })
            })
            .join()
        });
        match r {
            Ok(Ok(Some(v))) => Some(i64::from(v)),
            Ok(Ok(None)) => {
                // upstream: MailRule.DoesNotExist → error; here default layout (Known difference)
                tracing::warn!(target: "paperless.parsing.mail", "Mail rule {id} not found, using the default PDF layout");
                None
            }
            Ok(Err(e)) => {
                tracing::warn!(target: "paperless.parsing.mail", "Could not read the PDF layout of mail rule {id}: {e}");
                None
            }
            Err(_) => None,
        }
    }

    /// HEIC/HEIF → PNG in the child process (gap E8); `None` if the tool is missing or fails.
    fn heic_png(&self, path: &Path, tempdir: &Path) -> Option<PathBuf> {
        let out = tempdir.join("heic-decoded.png");
        match crate::office::heic_to_png_blocking(path, &out, 0) {
            Ok(_) => Some(out),
            Err(e) => {
                tracing::warn!(target: "paperless.parsing", "HEIC image could not be decoded: {}", e.user_message());
                None
            }
        }
    }

    /// upstream `make_thumbnail_from_pdf` with `make_thumbnail_from_pdf_qpdf_fallback` (gap E15):
    /// if rendering fails, the PDF is repaired (lopdf, otherwise PdfCraft in the child process) and
    /// rendered again; only then the default image.
    fn pdf_thumbnail_with_repair(&self, pdf: &Path, tempdir: &Path) -> Result<PathBuf, StageError> {
        let renderer = self.renderer();
        let err = match pl_ocr::thumbnail::make_thumbnail_from_pdf(renderer.as_ref(), pdf, tempdir) {
            Ok(p) => return Ok(p),
            Err(e) => e,
        };
        tracing::error!(target: "paperless.parsing", "Unable to make thumbnail with PDFium: {}", err.message());
        tracing::warn!(target: "paperless.parsing", "Thumbnail generation with PDFium failed, attempting repair and retry.");
        let repaired = tempdir.join("repaired.pdf");
        let mut last = String::from("repair failed");
        // 1. lopdf (qpdf replacement in the process)
        match std::fs::read(pdf).map_err(|e| e.to_string()).and_then(|d| pl_pdfops::repair::repair_bytes(&d).map_err(|e| e.to_string())) {
            Ok((bytes, _)) => {
                if std::fs::write(&repaired, bytes).is_ok() {
                    match pl_ocr::thumbnail::make_thumbnail_from_pdf(renderer.as_ref(), &repaired, tempdir) {
                        Ok(p) => return Ok(p),
                        Err(e) => last = e.message().to_string(),
                    }
                }
            }
            Err(e) => last = e,
        }
        // 2. PdfCraft (lenient reader) in the child process
        if let Some(tool) = pl_pdftools::runner::tool_path() {
            let op = pl_pdftools::job::Op::Repair { input: pdf.to_string_lossy().into_owned(), output: repaired.to_string_lossy().into_owned() };
            match pl_pdftools::runner::run_blocking_with(&tool, op, pl_pdftools::runner::Limits::from_env()) {
                Ok(pl_pdftools::job::Reply::Repaired(_)) => match pl_ocr::thumbnail::make_thumbnail_from_pdf(renderer.as_ref(), &repaired, tempdir) {
                    Ok(p) => return Ok(p),
                    Err(e) => last = e.message().to_string(),
                },
                Ok(_) => {}
                Err(e) => last = e.user_message(),
            }
        }
        tracing::error!(target: "paperless.parsing", "Unable to make thumbnail after repair: {last}");
        Self::default_thumbnail(tempdir)
    }

    fn find(&self, info: &ParserInfo, mime_type: &str, filename: &str, path: &Path) -> Option<&dyn pl_parsers::DocumentParser> {
        self.registry
            .get_parser_for_file(mime_type, filename, Some(path), true)
            .filter(|p| p.class_name() == info.class_name)
    }

    fn renderer(&self) -> Box<dyn pl_ocr::render::PdfRenderer> {
        match &self.pdfium {
            Some(p) => Box::new(p.clone()),
            None => Box::new(pl_ocr::render::UnavailableRenderer { reason: "PDFium not loaded".into() }),
        }
    }

    fn default_thumbnail(tempdir: &Path) -> Result<PathBuf, StageError> {
        let out = tempdir.join("document.webp");
        std::fs::write(&out, DEFAULT_THUMBNAIL).map_err(|e| StageError::Other(e.to_string()))?;
        Ok(out)
    }
}

impl DocumentParsing for NativeParsing {
    fn page_images(&self, path: &Path, mime_type: &str, max_pages: usize) -> Result<Vec<pl_core::vision::PageImage>, StageError> {
        let r = self.renderer();
        pl_ocr::vision_pages::page_images(
            r.as_ref(),
            path,
            mime_type,
            max_pages,
            pl_core::vision::PAGE_IMAGE_DPI,
            pl_core::vision::MAX_PAGE_IMAGE_BYTES,
        )
        .map(|v| v.into_iter().map(|(mime, data)| pl_core::vision::PageImage { mime, data }).collect())
        .map_err(|e| StageError::Other(e.to_string()))
    }

    fn parser_for(&self, mime_type: &str, filename: &str, path: &Path, allow_remote: bool) -> Option<ParserInfo> {
        let p = self.registry.get_parser_for_file(mime_type, filename, Some(path), allow_remote)?;
        Some(ParserInfo {
            class_name: p.class_name().to_string(),
            name: p.name().to_string(),
            can_produce_archive: p.can_produce_archive(),
            requires_pdf_rendition: p.requires_pdf_rendition(),
            uses_remote_service: p.uses_remote_service(),
        })
    }

    fn should_produce_archive(&self, parser: &ParserInfo, mime_type: &str, path: &Path) -> bool {
        let ctx = self.ctx(None);
        pl_ocr::parser::should_produce_archive(
            parser.requires_pdf_rendition,
            parser.can_produce_archive,
            self.ocr.archive_file_generation,
            mime_type,
            || pl_parsers::pdf::pdf_born_digital_text(path, &ctx).1,
        )
    }

    fn parse(&self, parser: &ParserInfo, req: &ParseRequest<'_>, produce_archive: bool) -> Result<ParsedOutput, StageError> {
        if parser.class_name == RASTERISED {
            let engine = pl_ocr::engine_select::engine_from_env(self.tesseract.clone(), &self.ocr.language);
            let backend = pl_ocr::native::NativeOcrmypdf::new(engine, self.renderer(), req.tempdir.join("pages"));
            // Gap E8: HEIC first to PNG (child process, craft-heif), then like any other image
            let heic = if req.mime_type == "image/heic" { self.heic_png(req.path, req.tempdir) } else { None };
            let (path, mime_type) = match &heic {
                Some(png) => (png.as_path(), "image/png"),
                None => (req.path, req.mime_type),
            };
            let out = pl_ocr::native::parse_document(&backend, &self.ocr, req.tempdir, path, mime_type, produce_archive)
                .map_err(|e| match e {
                    pl_ocr::OcrError::Parse(m) => StageError::Parse(m),
                    other => StageError::Parse(format!("{}: {}", other.class_name(), other.message())),
                })?;
            return Ok(ParsedOutput { text: Some(out.text), date: None, archive_path: out.archive_path });
        }
        if parser.class_name == crate::remote::CLASS_NAME {
            // RemoteDocumentParser.parse(…, produce_archive=…) – Azure bzw. lokaler Text
            let Some(cfg) = &self.remote else {
                return Err(StageError::Other("remote OCR is not configured".into()));
            };
            let p = crate::remote::RemoteDocumentParser { cfg: cfg.clone() };
            let ctx = self.ctx(req.mailrule_id);
            return match p.run(req.path, req.mime_type, produce_archive, req.tempdir, &ctx) {
                Ok(d) => Ok(ParsedOutput { text: Some(d.text), date: None, archive_path: d.archive_path }),
                Err(pl_parsers::ParseError::Upstream(m)) => Err(StageError::Parse(m)),
                Err(e) => Err(StageError::Parse(e.to_string())),
            };
        }
        let Some(p) = self.find(parser, req.mime_type, req.filename, req.path) else {
            return Err(StageError::Other(format!("parser {} not available", parser.class_name)));
        };
        let mut ctx = self.ctx(req.mailrule_id);
        // generated files (archive PDF) into the parser folder, never next to the original (reprocessing
        // reads directly from ORIGINALS_DIR)
        ctx.work_dir = Some(req.tempdir.to_path_buf());
        if parser.class_name == "MailDocumentParser" && p.requires_pdf_rendition() {
            ctx.mail_pdf_layout = Some(self.mail_pdf_layout(req.mailrule_id));
        }
        match p.parse(req.path, req.mime_type, &ctx) {
            Ok(d) => Ok(ParsedOutput { text: Some(d.text), date: d.date, archive_path: d.archive_path }),
            Err(pl_parsers::ParseError::Upstream(m)) => Err(StageError::Parse(m)),
            Err(pl_parsers::ParseError::Io(e)) => Err(StageError::Other(e.to_string())),
            Err(e) => Err(StageError::Parse(e.to_string())),
        }
    }

    fn thumbnail(&self, parser: &ParserInfo, req: &ParseRequest<'_>, archive: Option<&Path>) -> Result<PathBuf, StageError> {
        self.thumbnail_with_text(parser, req, archive, None)
    }

    fn thumbnail_with_text(
        &self,
        parser: &ParserInfo,
        req: &ParseRequest<'_>,
        archive: Option<&Path>,
        text: Option<&str>,
    ) -> Result<PathBuf, StageError> {
        // Text: TextDocumentParser.get_thumbnail (set the file as text at 500×700).
        if parser.class_name == TEXT {
            let Some(font) = &self.thumbnail_font else {
                return Self::default_thumbnail(req.tempdir);
            };
            // upstream: exception while reading/drawing -> parser error
            return pl_pdfops::thumbnail::make_text_thumbnail(req.path, font, req.tempdir).map_err(|e| StageError::Other(e.to_string()));
        }
        // Mail/Tika/HTML without a PDF version (upstream: first page of the Gotenberg PDF): text preview
        // from the extracted text (Known difference), otherwise default image.
        let remote = parser.class_name == crate::remote::CLASS_NAME;
        if parser.class_name != RASTERISED
            && !remote
            && archive.is_none()
            && let (Some(font), Some(t)) = (&self.thumbnail_font, text)
        {
            match pl_pdfops::thumbnail::text_thumbnail_webp(t, font) {
                Ok(bytes) => {
                    let out = req.tempdir.join("thumb.webp");
                    std::fs::write(&out, bytes).map_err(|e| StageError::Other(e.to_string()))?;
                    return Ok(out);
                }
                Err(e) => tracing::warn!(target: "paperless.parsing", "Unable to make text thumbnail: {e}, falling back to default"),
            }
        }
        // upstream fix 759f9cd22 (#14741): image without an archive PDF (OCR skipped, regenerate
        // thumbnails) → preview directly from the image instead of the default image.
        if parser.class_name == RASTERISED && archive.is_none() && pl_ocr::imageinfo::is_image(req.mime_type) {
            // HEIC: decode to PNG first (gap E8)
            let heic = if req.mime_type == "image/heic" { self.heic_png(req.path, req.tempdir) } else { None };
            let image = heic.as_deref().unwrap_or(req.path);
            return match pl_ocr::thumbnail::make_thumbnail_from_image(image, req.tempdir) {
                Ok(p) => Ok(p),
                Err(e) => {
                    // upstream: ParseError; here default image, because the image crate e.g. cannot read
                    // HEIC, which Pillow can (Known difference in docs/contract/pl-consumer.md).
                    tracing::warn!(target: "paperless.parsing", "{}, falling back to default", e.message());
                    Self::default_thumbnail(req.tempdir)
                }
            };
        }
        // Tesseract/Remote: archive or document (`make_thumbnail_from_pdf(archive or document)`);
        // Mail/Tika: PDF version.
        let source: Option<&Path> = if parser.class_name == RASTERISED || remote {
            Some(archive.unwrap_or(req.path))
        } else {
            archive
        };
        let Some(pdf) = source else {
            return Self::default_thumbnail(req.tempdir);
        };
        let is_pdf = pdf.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            || std::fs::read(pdf).map(|d| d.starts_with(b"%PDF")).unwrap_or(false);
        if !is_pdf || self.pdfium.is_none() {
            return Self::default_thumbnail(req.tempdir);
        }
        self.pdf_thumbnail_with_repair(pdf, req.tempdir)
    }

    fn page_count(&self, parser: &ParserInfo, req: &ParseRequest<'_>, archive: Option<&Path>) -> Option<i64> {
        let ctx = self.ctx(req.mailrule_id);
        if parser.class_name == RASTERISED || parser.class_name == crate::remote::CLASS_NAME {
            if req.mime_type == "application/pdf" {
                return pl_parsers::pdf::get_page_count(req.path, &ctx).map(i64::from);
            }
            return None;
        }
        if parser.class_name == TEXT {
            return None;
        }
        archive.and_then(|a| pl_parsers::pdf::get_page_count(a, &ctx)).map(i64::from)
    }
}

/// `get_date_parser()` with `RegexDateParser` (pl-matching).
pub struct RegexDates {
    pub languages: Vec<String>,
    pub time_zone: String,
    pub ignore_dates_raw: Option<String>,
    pub filename_date_order: Option<String>,
    pub date_order: String,
}

impl RegexDates {
    pub fn from_settings(s: &ConsumerSettings, ocr_language: &str) -> Self {
        Self {
            languages: pl_matching::dates::resolve_languages(s.date_parser_languages.as_deref(), ocr_language),
            time_zone: s.time_zone.name().to_string(),
            ignore_dates_raw: s.ignore_dates_raw.clone(),
            filename_date_order: s.filename_date_order.clone(),
            date_order: s.date_order.clone(),
        }
    }
}

impl DateDetector for RegexDates {
    fn detect(&self, filename: &str, content: &str) -> Option<DateTime<FixedOffset>> {
        let now = Utc::now();
        let ignore = match &self.ignore_dates_raw {
            Some(raw) => pl_matching::dates::parse_ignore_dates(raw, &self.date_order, now),
            None => Default::default(),
        };
        let cfg = pl_matching::DateParserConfig {
            languages: self.languages.clone(),
            timezone_str: self.time_zone.clone(),
            ignore_dates: ignore,
            reference_time: now.fixed_offset(),
            filename_date_order: self.filename_date_order.clone(),
            content_date_order: self.date_order.clone(),
        };
        let p = pl_matching::RegexDateParser::new(cfg);
        p.parse(filename, content).next()
    }
}

/// pl-classifier model with `CLASSIFIER_MATCH_THRESHOLD`.
pub struct ThresholdClassifier {
    pub model: pl_classifier::DocumentClassifier,
    pub threshold: f64,
}

impl pl_matching::Classifier for ThresholdClassifier {
    fn predict_correspondent(&self, content: &str) -> Option<i64> {
        self.model.predict_correspondent(content, self.threshold)
    }
    fn predict_document_type(&self, content: &str) -> Option<i64> {
        self.model.predict_document_type(content, self.threshold)
    }
    fn predict_tags(&self, content: &str) -> Vec<i64> {
        self.model.predict_tags(content)
    }
    fn predict_storage_path(&self, content: &str) -> Option<i64> {
        self.model.predict_storage_path(content, self.threshold)
    }
}

/// `load_classifier()` from the PaperRust model file.
pub struct FileClassifier {
    pub path: PathBuf,
    pub key: Vec<u8>,
    pub threshold: f64,
}

impl ClassifierSource for FileClassifier {
    fn load(&self) -> Option<SharedClassifier> {
        match pl_classifier::load_classifier(&self.path, &self.key, false) {
            Ok(Some(model)) => Some(Arc::new(ThresholdClassifier { model, threshold: self.threshold })),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(target: "paperless.classifier", "Unknown error while loading document classification model: {e}");
                None
            }
        }
    }
}

/// No classifier (e.g. tests).
pub struct NoClassifier;
impl ClassifierSource for NoClassifier {
    fn load(&self) -> Option<SharedClassifier> {
        None
    }
}

/// tantivy-Schreiber (pl-indexer).
pub struct TantivyIndex {
    pub indexer: pl_indexer::Indexer,
}

impl TantivyIndex {
    pub fn open(cfg: pl_indexer::IndexerConfig) -> Result<Self, String> {
        let (indexer, _) = pl_indexer::Indexer::open(cfg).map_err(|e| e.to_string())?;
        Ok(Self { indexer })
    }
}

impl SearchIndex for TantivyIndex {
    fn add_or_update<'a>(&'a self, pool: &'a sqlx::PgPool, document_id: i64) -> BoxFuture<'a, Result<IndexOutcome, String>> {
        Box::pin(async move {
            match self.indexer.add_or_update(pool, document_id).await {
                Ok(pl_indexer::writer::WriteOutcome::Written) => Ok(IndexOutcome::Written),
                Ok(pl_indexer::writer::WriteOutcome::Skipped) => Ok(IndexOutcome::Skipped),
                Ok(pl_indexer::writer::WriteOutcome::Deferred) => Ok(IndexOutcome::Deferred),
                Err(e) => Err(e.to_string()),
            }
        })
    }
}

/// No index (e.g. tests without a search index).
pub struct NoIndex;
impl SearchIndex for NoIndex {
    fn add_or_update<'a>(&'a self, _pool: &'a sqlx::PgPool, _document_id: i64) -> BoxFuture<'a, Result<IndexOutcome, String>> {
        Box::pin(async { Ok(IndexOutcome::Skipped) })
    }
}

/// Status messages via the pl-queue `EventBus` (in-process WebSocket).
pub struct EventBusProgress {
    pub bus: pl_queue::events::EventBus,
}

impl ProgressSink for EventBusProgress {
    fn send(&self, u: ProgressUpdate) {
        pl_queue::events::ProgressManager::new(self.bus.clone(), u.filename, u.task_id).send_progress(
            u.status,
            &u.message,
            u.current_progress,
            u.max_progress,
            u.document_id,
            u.perms,
        );
    }
}

/// Collects messages (tests).
#[derive(Default)]
pub struct RecordingProgress {
    pub updates: std::sync::Mutex<Vec<ProgressUpdate>>,
}

impl ProgressSink for RecordingProgress {
    fn send(&self, update: ProgressUpdate) {
        if let Ok(mut g) = self.updates.lock() {
            g.push(update);
        }
    }
}

/// No barcode reader (e.g. tests without PDFium). The barcode plugin then does not run.
pub struct NoBarcodes;
impl BarcodeScanner for NoBarcodes {
    fn available(&self) -> bool {
        false
    }
    fn scan(&self, _pdf: &Path, _tempdir: &Path, _s: &BarcodeScanSettings) -> Result<Vec<Barcode>, StageError> {
        Ok(Vec::new())
    }
    fn split(&self, _pdf: &Path, _pages: &[(i64, bool)], _tempdir: &Path, _stem: &str, _stats: &Path) -> Result<Vec<PathBuf>, StageError> {
        Err(StageError::Other("Barcode splitting is not supported".into()))
    }
}

/// Barcodes with pl-pdfops: PDFium raster (pl-ocr) + rxing instead of pdf2image + zxing-cpp,
/// page splitting with lopdf instead of pikepdf (docs/contract/pl-pdfops.md §5).
pub struct PdfOpsBarcodes {
    pub renderer: Arc<dyn pl_ocr::render::PdfRenderer>,
    pub reader: Arc<dyn pl_pdfops::barcodes::BarcodeReader>,
}

impl PdfOpsBarcodes {
    pub fn new(renderer: Arc<dyn pl_ocr::render::PdfRenderer>) -> Self {
        Self { renderer, reader: Arc::new(pl_pdfops::barcodes::RxingReader) }
    }

    /// PDFium from `PAPERRUST_PDFIUM_PATH` (or the default search of pl-ocr).
    pub fn from_env() -> Self {
        Self::new(Arc::from(pl_ocr::render::default_renderer()))
    }

    fn scan_raw(&self, pdf: &Path, s: &BarcodeScanSettings) -> pl_pdfops::Result<Vec<Barcode>> {
        let codes = pl_pdfops::barcodes::scan_pdf(pdf, *s, self.renderer.as_ref(), self.reader.as_ref())?;
        Ok(codes.into_iter().map(|b| Barcode { page: b.page, value: b.value, format: b.format }).collect())
    }
}

impl BarcodeScanner for PdfOpsBarcodes {
    fn available(&self) -> bool {
        self.renderer.name() != "unavailable"
    }

    fn scan(&self, pdf: &Path, _tempdir: &Path, s: &BarcodeScanSettings) -> Result<Vec<Barcode>, StageError> {
        self.scan_raw(pdf, s).map_err(|e| StageError::Other(e.to_string()))
    }

    fn detect(&self, pdf: &Path, _tempdir: &Path, s: &BarcodeScanSettings) -> Vec<Barcode> {
        match self.scan_raw(pdf, s) {
            Ok(v) => v,
            Err(pl_pdfops::PdfOpsError::Password(e)) => {
                tracing::warn!(target: "paperless.barcodes", "File is likely password protected, not checking for barcodes: {e}");
                Vec::new()
            }
            Err(e) => {
                tracing::warn!(target: "paperless.barcodes", "Exception during barcode scanning: {e}");
                Vec::new()
            }
        }
    }

    fn split(
        &self,
        pdf: &Path,
        separator_pages: &[(i64, bool)],
        tempdir: &Path,
        name_stem: &str,
        stats_source: &Path,
    ) -> Result<Vec<PathBuf>, StageError> {
        let map: std::collections::BTreeMap<i64, bool> = separator_pages.iter().copied().collect();
        pl_pdfops::barcodes::separate_pages(pdf, &map, name_stem, tempdir, stats_source).map_err(|e| StageError::Other(e.to_string()))
    }
}

/// Page operations with lopdf (double-sided scans); TIFF→PDF via pl-pdfops (img2pdf replacement).
pub struct LopdfPages;

impl PdfPageOps for LopdfPages {
    fn collate(&self, odd: &Path, even: &Path, out: &Path) -> Result<(), StageError> {
        crate::collate::collate_pdfs(odd, even, out)
    }
    fn tiff_to_pdf(&self, tiff: &Path, tempdir: &Path) -> Result<PathBuf, StageError> {
        pl_pdfops::convert::convert_from_tiff_to_pdf(tiff, tempdir).map_err(|e| StageError::Other(e.to_string()))
    }
}

/// Workflow actions with external effects: only log.
pub struct LoggingEffects;
impl WorkflowEffects for LoggingEffects {
    fn email(&self, ctx: &ActionContext) {
        tracing::warn!(target: "paperless.workflows", "E-mail action {} not executed (not supported by PaperRust worker yet)", ctx.workflow_action_id);
    }
    fn webhook(&self, ctx: &ActionContext) {
        tracing::warn!(target: "paperless.workflows", "Webhook action {} not executed (not supported by PaperRust worker yet)", ctx.workflow_action_id);
    }
    fn password_removal(&self, ctx: &ActionContext) {
        tracing::warn!(target: "paperless.workflows", "Password removal action {} not executed (not supported by PaperRust worker yet)", ctx.workflow_action_id);
    }
}

/// `run_subprocess([script], env)`: error on exit code ≠ 0.
pub struct CommandScripts;
impl ScriptRunner for CommandScripts {
    fn run(&self, script: &Path, env: &[(String, String)]) -> Result<(), String> {
        let mut cmd = std::process::Command::new(script);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().map_err(|e| e.to_string())?;
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            tracing::info!(target: "paperless.consumer", "{line}");
        }
        for line in String::from_utf8_lossy(&out.stderr).lines() {
            tracing::warn!(target: "paperless.consumer", "{line}");
        }
        if !out.status.success() {
            let code = out.status.code().unwrap_or(-1);
            return Err(format!("Command '['{}']' returned non-zero exit status {code}.", script.display()));
        }
        Ok(())
    }
}
