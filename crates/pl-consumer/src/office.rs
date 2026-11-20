// PaperRust-specific (no upstream counterpart): Office → PDF on ingestion with WordCraft, isolated in the
// child process `paperrust-pdftool` (pl-pdftools `Op::Office`/`Op::Slides`/`Op::Heic`). Contract:
// docs/contract/pl-pdftools.md §6.
//
// upstream: src/paperless_tika/parsers.py@v3.4.2 TikaDocumentParser.convert_to_pdf (Gotenberg) –
// here without Gotenberg: a real archive PDF from which the thumbnail (first page) is also produced.
// upstream: src/paperless/parsers/mail.py@v3.4.2 generate_pdf (Gotenberg Chromium) – here the
// same generator sets the header page (HTML) and the HTML part (MHT) of the mail.
// upstream: src/paperless/parsers/tesseract.py@v3.4.2 (HEIC via pillow_heif) – here the
// child process decodes the photo to PNG before OCR/archive/thumbnail treat it like any other image.
//
//! * `PAPERRUST_OFFICE_TO_PDF` (default on): Office files (.docx, .odt, .rtf, .doc, spreadsheets,
//!   presentations) and e-mails get an archive PDF on ingestion.
//! * `PAPERRUST_OFFICE_TO_PDF_TIMEOUT` (seconds, default like `PAPERRUST_PDFTOOL_TIMEOUT`).
//!
//! If WordCraft fails (broken file, time limit, crash of the child process), the parser writes
//! a warning and ingestion continues as before (original without archive, text thumbnail).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pl_parsers::ParseError;
use pl_pdftools::job::{Op, Reply, document_format};
use pl_pdftools::runner::{Limits, RunError};

const LOG: &str = "paperless.parsing.tika";

/// MIME type under which the mail parser passes its MHT web archive (HTML part + `cid:` images).
pub const MHT_MIME: &str = "multipart/related";

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" | "t" => Some(true),
        "0" | "false" | "no" | "n" | "off" | "f" => Some(false),
        _ => None,
    }
}

/// `PAPERRUST_OFFICE_TO_PDF` (Standard `true`).
pub fn enabled() -> bool {
    pl_core::config::var("PAPERRUST_OFFICE_TO_PDF").and_then(|v| parse_bool(&v)).unwrap_or(true)
}

/// Limits: memory like the PDF tool, time from `PAPERRUST_OFFICE_TO_PDF_TIMEOUT` (≥ 1).
pub fn limits() -> Limits {
    let mut l = Limits::from_env();
    if let Some(s) = pl_core::config::var("PAPERRUST_OFFICE_TO_PDF_TIMEOUT").and_then(|v| v.trim().parse::<u64>().ok()).filter(|s| *s >= 1) {
        l.timeout = Duration::from_secs(s);
    }
    l
}

/// Extension for WordCraft (word processing, old Word, spreadsheets, MHT of the mail).
fn format_for(mime_type: &str) -> Option<&'static str> {
    if mime_type.eq_ignore_ascii_case(MHT_MIME) {
        return Some("mht");
    }
    document_format(mime_type)
}

/// Can this MIME type be converted?
pub fn convertible(mime_type: &str) -> bool {
    format_for(mime_type).is_some() || pl_parsers::office::is_presentation(mime_type)
}

fn run(op: Op) -> Result<pl_pdftools::ConvertReport, RunError> {
    let tool = pl_pdftools::runner::tool_path().ok_or(RunError::NotInstalled)?;
    match pl_pdftools::runner::run_blocking_with(&tool, op, limits())? {
        Reply::Converted(r) => Ok(r),
        _ => Err(RunError::Protocol("unexpected reply".into())),
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Convert with the tool found (synchronous; for `spawn_blocking`/parser). Presentations
/// are first turned into slide HTML in the server process (pl-parsers) and then set in landscape.
pub fn convert_blocking(input: &Path, mime_type: &str, output: &Path) -> Result<pl_pdftools::ConvertReport, RunError> {
    if pl_parsers::office::is_presentation(mime_type) {
        let ctx = pl_parsers::ParseContext::default();
        let data = std::fs::read(input).map_err(|e| RunError::Failed(format!("The file could not be read: {}", e.kind())))?;
        let slides = pl_parsers::office::slides(&data, mime_type, &ctx).map_err(|e| RunError::Failed(format!("The slides could not be read: {e}")))?;
        let html = output.with_extension("slides.html");
        std::fs::write(&html, pl_parsers::office::slides_html(&slides)).map_err(|e| RunError::Failed(format!("The slides could not be prepared: {}", e.kind())))?;
        let r = run(Op::Slides { input: s(&html), output: s(output) });
        let _ = std::fs::remove_file(&html);
        return r;
    }
    let Some(format) = format_for(mime_type) else {
        return Err(RunError::Failed(format!("{mime_type} cannot be converted to PDF.")));
    };
    run(Op::Office { input: s(input), output: s(output), format: format.to_string() })
}

/// HEIC/HEIF → PNG in the child process (gap E8). `max_pixels` = 0: default limit of the tool.
pub fn heic_to_png_blocking(input: &Path, output: &Path, max_pixels: u64) -> Result<pl_pdftools::ConvertReport, RunError> {
    run(Op::Heic { input: s(input), output: s(output), max_pixels })
}

/// PDF generator for the Office and the mail parser (pl-parsers `PdfConverter`).
#[derive(Debug, Default, Clone, Copy)]
pub struct WordCraftConverter;

impl pl_parsers::office::PdfConverter for WordCraftConverter {
    fn convert(&self, path: &Path, mime_type: &str, out_dir: &Path) -> pl_parsers::Result<PathBuf> {
        if !convertible(mime_type) {
            return Err(ParseError::Unsupported(format!("{mime_type} cannot be converted to PDF")));
        }
        // own name per input: the mail parser converts two files in the same folder
        let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
        let out = out_dir.join(format!("{stem}.office-convert.pdf"));
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match convert_blocking(path, mime_type, &out) {
            Ok(r) => {
                tracing::info!(target: LOG, "Converted {name} to PDF with WordCraft ({} -> {} bytes)", r.size_before, r.size_after);
                Ok(out)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&out);
                // clear log line; the parser turns it into a warning and continues reading without an archive
                Err(ParseError::Invalid(format!("WordCraft could not convert {name}: {e}")))
            }
        }
    }
}

/// Install the generator if enabled and the tool is installed.
pub fn install(registry: &mut pl_parsers::ParserRegistry) {
    if !enabled() {
        tracing::debug!(target: LOG, "Office to PDF conversion is switched off (PAPERRUST_OFFICE_TO_PDF)");
        return;
    }
    if !pl_pdftools::runner::available() {
        tracing::debug!(target: LOG, "Office to PDF conversion not available: {} not installed", pl_pdftools::runner::TOOL_NAME);
        return;
    }
    registry.set_office_converter(Some(Box::new(WordCraftConverter)));
    registry.set_mail_converter(Some(Arc::new(WordCraftConverter)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bools() {
        assert_eq!(parse_bool(" On "), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("x"), None);
    }

    #[test]
    fn convertible_types() {
        assert!(convertible("application/vnd.openxmlformats-officedocument.wordprocessingml.document"));
        assert!(convertible("text/rtf"));
        // Gaps E9/E10: spreadsheets, presentations, old Office; E12: MHT of the mail
        assert!(convertible("application/vnd.oasis.opendocument.spreadsheet"));
        assert!(convertible("application/vnd.openxmlformats-officedocument.presentationml.presentation"));
        assert!(convertible("application/vnd.oasis.opendocument.graphics"));
        assert!(convertible("application/msword"));
        assert!(convertible("application/vnd.ms-excel"));
        assert!(convertible("application/vnd.ms-powerpoint"));
        assert!(convertible(MHT_MIME));
        assert!(!convertible("image/png"));
    }

    #[test]
    fn unknown_type_is_unsupported_not_an_error() {
        use pl_parsers::office::PdfConverter;
        let t = tempfile::tempdir().unwrap();
        let r = WordCraftConverter.convert(&t.path().join("a.bin"), "application/octet-stream", t.path());
        assert!(matches!(r, Err(ParseError::Unsupported(_))), "{r:?}");
    }
}
