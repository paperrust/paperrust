// upstream: src/paperless/parsers/remote.py@v3.4.2 (RemoteDocumentParser, RemoteEngineConfig),
//           src/paperless/parsers/registry.py@v3.4.2 (register_defaults: Text, Remote, Tika, …),
//           src/paperless/config.py@v3.4.2 (RemoteOCRConfig)
//
//! Remote OCR parser (Azure Document Intelligence) in the parser registry.
//!
//! The actual work (Azure call, local text extraction for born-digital PDFs without archive)
//! is in `pl_ocr::remote::parse`. This parser is only the registry shell: score 20 with a
//! valid engine and supported type (before Tesseract with 10), otherwise `None` (invisible),
//! `uses_remote_service = true` (the registry skips it without `allow_remote`). It lives in
//! pl-consumer because pl-parsers cannot depend on pl-ocr (pl-ocr uses pl-parsers for
//! text extraction).
//!
//! The run with `produce_archive` is done by `NativeParsing::parse` (the trait does not know the
//! switch); thumbnail from the archive or the PDF original like `get_thumbnail`.

use std::cell::RefCell;
use std::path::Path;

use pl_ocr::config::{REMOTE_SUPPORTED_MIME_TYPES, RemoteOcrConfig};
use pl_parsers::{DocumentParser, MetadataEntry, ParseContext, ParseError, ParsedDocument};

/// `RemoteDocumentParser` (class name like upstream, for logs/`ParserInfo`).
pub const CLASS_NAME: &str = "RemoteDocumentParser";

pub struct RemoteDocumentParser {
    pub cfg: RemoteOcrConfig,
}

impl RemoteDocumentParser {
    /// `parse(document_path, mime_type, produce_archive=…)` → text and archive PDF, plus the
    /// truncation of the local text extraction (born-digital PDF without archive) in
    /// [`ParsedDocument::truncated`], if a parser limit cut it short (same single pass).
    pub fn run(
        &self,
        path: &Path,
        mime_type: &str,
        produce_archive: bool,
        tempdir: &Path,
        ctx: &ParseContext,
    ) -> Result<ParsedDocument, ParseError> {
        let truncated = RefCell::new(Vec::new());
        let extract = |p: &Path| {
            let r = pl_parsers::pdf::extract_pdf_text_report(p, ctx)?;
            truncated.borrow_mut().extend(r.truncated);
            r.text
        };
        let out = pl_ocr::remote::parse(&self.cfg, tempdir, path, mime_type, produce_archive, &extract)
            .map_err(|e| ParseError::Upstream(e.message()))?;
        // get_date() is always None
        Ok(ParsedDocument { text: out.text, date: None, archive_path: out.archive_path, truncated: truncated.into_inner() })
    }
}

impl DocumentParser for RemoteDocumentParser {
    fn class_name(&self) -> &'static str {
        CLASS_NAME
    }
    fn name(&self) -> &'static str {
        "Paperless-ngx Remote OCR Parser"
    }
    fn supported_mime_types(&self) -> &'static [(&'static str, &'static str)] {
        REMOTE_SUPPORTED_MIME_TYPES
    }
    fn score(&self, mime_type: &str, _filename: &str, _path: Option<&Path>) -> Option<i32> {
        self.cfg.score(mime_type)
    }
    fn can_produce_archive(&self) -> bool {
        true
    }
    fn requires_pdf_rendition(&self) -> bool {
        false
    }
    fn uses_remote_service(&self) -> bool {
        true
    }
    fn parse(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Result<ParsedDocument, ParseError> {
        let dir = ctx.work_dir.clone().or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
        self.run(path, mime_type, true, &dir, ctx)
    }
    fn page_count(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Option<u32> {
        if mime_type != "application/pdf" {
            return None;
        }
        pl_parsers::pdf::get_page_count(path, ctx)
    }
    fn extract_metadata(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Vec<MetadataEntry> {
        if mime_type != "application/pdf" {
            return Vec::new();
        }
        pl_parsers::pdf::extract_pdf_metadata(path, ctx)
    }
}

/// Attach the parser to the registry (always; without a valid engine `score` returns `None`).
pub fn install(registry: &mut pl_parsers::ParserRegistry, cfg: &RemoteOcrConfig) {
    registry.register_external(Box::new(RemoteDocumentParser { cfg: cfg.clone() }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use pl_ocr::config::RemoteOcrMode;

    fn cfg(valid: bool) -> RemoteOcrConfig {
        RemoteOcrConfig {
            engine: Some("azureai".into()),
            api_key: valid.then(|| "k".into()),
            endpoint: Some("https://example.invalid".into()),
            mode: RemoteOcrMode::WorkflowOnly,
            allow_internal_endpoints: false,
        }
    }

    #[test]
    fn registry_picks_remote_only_when_allowed_and_configured() {
        let mut reg = pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default());
        install(&mut reg, &cfg(true));
        let p = reg.get_parser_for_file("application/pdf", "a.pdf", None, true).map(|p| p.class_name());
        assert_eq!(p, Some(CLASS_NAME));
        let p = reg.get_parser_for_file("application/pdf", "a.pdf", None, false).map(|p| p.class_name());
        assert_eq!(p, Some("RasterisedDocumentParser"));
        // HEIC not supported
        let p = reg.get_parser_for_file("image/heic", "a.heic", None, true).map(|p| p.class_name());
        assert_eq!(p, Some("RasterisedDocumentParser"));

        let mut reg = pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default());
        install(&mut reg, &cfg(false));
        let p = reg.get_parser_for_file("application/pdf", "a.pdf", None, true).map(|p| p.class_name());
        assert_eq!(p, Some("RasterisedDocumentParser"));
    }

    /// Without a valid engine: empty text, no network (upstream warning "content will be empty").
    #[test]
    fn invalid_engine_gives_empty_text() {
        let p = RemoteDocumentParser { cfg: cfg(false) };
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("x.pdf");
        std::fs::write(&f, b"%PDF-1.4\n").expect("write");
        let d = p.run(&f, "application/pdf", true, dir.path(), &ParseContext::default()).expect("parse");
        assert_eq!(d.text, "");
        assert!(d.archive_path.is_none());
    }
}
