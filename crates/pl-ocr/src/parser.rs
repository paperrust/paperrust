// upstream: src/paperless/parsers/tesseract.py@v3.4.2 (RasterisedDocumentParser.parse, extract_text,
//           _handle_subprocess_output_error, can_produce_archive, requires_pdf_rendition)
// upstream: src/documents/consumer.py@v3.4.2 (should_produce_archive)
// upstream: src/paperless/parsers/remote.py@v3.4.2 (RemoteDocumentParser.parse: decision without Azure)
//
// The control logic is ported 1:1 and separated from the OCR tool: everything that upstream delegates to
// `ocrmypdf.ocr`, `pdftotext`, img2pdf/pikepdf and Ghostscript goes through
// [`OcrBackend`]. This lets the upstream tests run with fakes (like `mocker.patch` there).

use std::path::{Path, PathBuf};

use pl_parsers::limits::Truncation;
use pl_parsers::pdf::PdfText;

use crate::args::{ArgsRequest, ImageOps, construct_ocrmypdf_parameters};
use crate::config::{ArchiveFileGeneration, Mode, OcrConfig};
use crate::imageinfo::is_image;
use crate::text::{is_born_digital_text, post_process_text, read_file_handle_unicode_errors};
use crate::{OcrArgs, OcrError, Result};

/// What the parser needs from outside (OCRmyPDF, pdftotext, img2pdf+pikepdf, Ghostscript).
pub trait OcrBackend {
    /// `extract_pdf_text` (pdftotext): raw text or `None` on error.
    fn extract_pdf_text(&self, pdf: &Path) -> Option<String>;
    /// [`Self::extract_pdf_text`] plus the limit that cut the text short, if any (PaperRust:
    /// page, time and text-size limits keep the pages read so far). Default: no limits known.
    fn extract_pdf_text_report(&self, pdf: &Path) -> PdfText {
        PdfText { text: self.extract_pdf_text(pdf), truncated: None }
    }
    /// `is_tagged_pdf`.
    fn is_tagged_pdf(&self, pdf: &Path) -> bool;
    /// `ocrmypdf.ocr(**args)`.
    fn ocr(&self, args: &OcrArgs) -> Result<()>;
    /// `_convert_image_to_pdfa` → path of the archive.
    fn convert_image_to_pdfa(&self, image: &Path, cfg: &OcrConfig, tempdir: &Path) -> Result<PathBuf>;
    /// `_convert_pdf_to_pdfa` (Ghostscript without OCR; `pdf` → copy).
    fn convert_pdf_to_pdfa(&self, input: &Path, output: &Path, cfg: &OcrConfig) -> Result<()>;

    /// `RasterisedDocumentParser.extract_text(sidecar_file, pdf_file)`.
    fn extract_text(&self, sidecar: Option<&Path>, pdf: &Path, mode: Mode) -> Option<String> {
        self.extract_text_report(sidecar, pdf, mode).text
    }

    /// [`Self::extract_text`] plus the truncation of the PDF text extraction (the sidecar is
    /// never cut). This is what [`RasterisedParser::parse`] calls, so the reported truncation
    /// always belongs to the very text that ends up in [`ParseOutcome::text`].
    fn extract_text_report(&self, sidecar: Option<&Path>, pdf: &Path, mode: Mode) -> PdfText {
        if let Some(sc) = sidecar {
            // When re-doing OCR, the sidecar contains ONLY the new text
            if sc.is_file()
                && mode != Mode::Redo
                && let Ok(text) = read_file_handle_unicode_errors(sc)
            {
                if !text.contains("[OCR skipped on page") {
                    tracing::debug!("Using text from sidecar file");
                    return PdfText { text: post_process_text(Some(&text)), truncated: None };
                }
                tracing::debug!("Incomplete sidecar file: discarding.");
            }
        }
        if !pdf.is_file() {
            return PdfText::default();
        }
        let r = self.extract_pdf_text_report(pdf);
        PdfText { text: post_process_text(r.text.as_deref()), truncated: r.truncated }
    }
}

/// Result of `parse()`: `get_text()` and `get_archive_path()`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParseOutcome {
    pub text: String,
    pub archive_path: Option<PathBuf>,
    /// All `ocrmypdf.ocr` calls (for logs/tests).
    pub ocr_calls: Vec<OcrArgs>,
    /// PaperRust: a parser limit cut `text` short (only possible when `text` comes from the
    /// PDF text extraction, not from the OCR sidecar). The consumer turns it into a note.
    pub truncated: Option<Truncation>,
}

/// `RasterisedDocumentParser` (without thumbnail/metadata, see `thumbnail`, pl-parsers).
pub struct RasterisedParser<'a> {
    pub settings: OcrConfig,
    pub tempdir: PathBuf,
    pub backend: &'a dyn OcrBackend,
    pub images: &'a dyn ImageOps,
}

impl RasterisedParser<'_> {
    pub const NAME: &'static str = "PaperRust Tesseract OCR Parser";

    pub fn can_produce_archive(&self) -> bool {
        true
    }
    pub fn requires_pdf_rendition(&self) -> bool {
        false
    }

    fn args(&self, input: &Path, mime: &str, out: &Path, sidecar: &Path, safe_fallback: bool, skip_text: bool) -> Result<OcrArgs> {
        let (i, o, s) = (path_str(input), path_str(out), path_str(sidecar));
        construct_ocrmypdf_parameters(
            &self.settings,
            &ArgsRequest { input_file: &i, mime_type: mime, output_file: &o, sidecar_file: &s, safe_fallback, skip_text },
            self.images,
        )
    }

    fn subprocess_error(e: &str) -> OcrError {
        if e.contains("Ghostscript PDF/A rendering") {
            tracing::warn!(
                "Ghostscript PDF/A rendering failed, consider setting PAPERRUST_OCR_USER_ARGS: '{{\"continue_on_soft_render_error\": true}}'"
            );
        }
        OcrError::Parse(format!("SubprocessOutputError: {e}. See logs for more information."))
    }

    fn wrap(e: &OcrError) -> OcrError {
        match e {
            OcrError::Parse(m) => OcrError::Parse(format!("ParseError: {m}")),
            other => OcrError::Parse(format!("{}: {}", other.class_name(), other.message())),
        }
    }

    /// `parse(document_path, mime_type, produce_archive=...)`.
    pub fn parse(&self, document_path: &Path, mime_type: &str, produce_archive: bool) -> Result<ParseOutcome> {
        let mode = self.settings.mode;
        let mut out = ParseOutcome::default();
        let mut text: Option<String>;
        // truncation of the text in `text` (PaperRust)
        let mut truncated: Option<Truncation> = None;

        let (text_original, trunc_original, original_has_text) = if mime_type == "application/pdf" {
            let r = self.backend.extract_text_report(None, document_path, mode);
            let has = is_born_digital_text(r.text.as_deref(), || self.backend.is_tagged_pdf(document_path));
            (r.text, r.truncated, has)
        } else {
            (None, None, false)
        };
        tracing::debug!(
            "Text detection: original_has_text={} (text_length={}, mode={}, produce_archive={})",
            if original_has_text { "True" } else { "False" },
            text_original.as_deref().map_or(0, |t| t.chars().count()),
            mode,
            if produce_archive { "True" } else { "False" },
        );

        // --- OCR_MODE=off: never invoke OCR engine ---
        if mode == Mode::Off {
            if !produce_archive {
                out.text = text_original.unwrap_or_default();
                out.truncated = trunc_original;
                return Ok(out);
            }
            if is_image(mime_type) {
                match self.backend.convert_image_to_pdfa(document_path, &self.settings, &self.tempdir) {
                    Ok(p) => {
                        out.archive_path = Some(p);
                        out.text = String::new();
                    }
                    Err(e) => {
                        return Err(OcrError::Parse(format!("Image to PDF/A conversion failed: {}", e.message())));
                    }
                }
                return Ok(out);
            }
            let archive_path = self.tempdir.join("archive.pdf");
            match self.backend.convert_pdf_to_pdfa(document_path, &archive_path, &self.settings) {
                Ok(()) => {
                    out.archive_path = Some(archive_path);
                    out.text = text_original.unwrap_or_default();
                    out.truncated = trunc_original;
                }
                Err(OcrError::SubprocessOutput(m)) => return Err(Self::subprocess_error(&m)),
                Err(e) => return Err(Self::wrap(&e)),
            }
            return Ok(out);
        }

        // --- OCR_MODE=auto: skip ocrmypdf entirely if text exists and no archive needed ---
        if mode == Mode::Auto && original_has_text && !produce_archive {
            tracing::debug!("Document has text and no archive requested; skipping OCRmyPDF entirely.");
            out.text = text_original.unwrap_or_default();
            out.truncated = trunc_original;
            return Ok(out);
        }

        let archive_path = self.tempdir.join("archive.pdf");
        let sidecar_file = self.tempdir.join("sidecar.txt");
        let skip_text = mode == Mode::Auto && original_has_text;
        if skip_text {
            tracing::debug!("OCR strategy: PDF/A conversion only (skip_text) — OCR_MODE=auto, document already has text");
        } else {
            tracing::debug!("OCR strategy: full OCR — OCR_MODE={mode}");
        }

        let args = self.args(document_path, mime_type, &archive_path, &sidecar_file, false, skip_text)?;
        tracing::debug!("Calling OCRmyPDF with args: {}", args.to_json());
        out.ocr_calls.push(args.clone());

        let first: Result<PdfText> = (|| {
            self.backend.ocr(&args)?;
            Ok(())
        })()
        .map(|()| {
            if produce_archive {
                out.archive_path = Some(archive_path.clone());
            }
            self.backend.extract_text_report(Some(&sidecar_file), &archive_path, mode)
        });

        text = None;
        let mut needs_fallback: Option<String> = None;
        match first {
            Ok(PdfText { text: Some(t), truncated: tr }) if !t.is_empty() => {
                text = Some(t);
                truncated = tr;
            }
            Ok(_) => needs_fallback = Some("No text was found in the original document".into()),
            Err(OcrError::DigitalSignature) | Err(OcrError::EncryptedPdf) => {
                tracing::warn!("This file is encrypted and/or signed, OCR is impossible. Using any text present in the original file.");
                if original_has_text {
                    text = text_original.clone();
                    truncated = trunc_original.clone();
                }
            }
            Err(OcrError::SubprocessOutput(m)) => return Err(Self::subprocess_error(&m)),
            Err(e @ (OcrError::InputFile(_) | OcrError::PriorOcrFound(_))) => needs_fallback = Some(e.message()),
            Err(e) => return Err(Self::wrap(&e)),
        }

        if let Some(reason) = needs_fallback {
            tracing::warn!("Encountered an error while running OCR: {reason}. Attempting force OCR to get the text.");
            let archive_fb = self.tempdir.join("archive-fallback.pdf");
            let sidecar_fb = self.tempdir.join("sidecar-fallback.txt");
            let args = self.args(document_path, mime_type, &archive_fb, &sidecar_fb, true, false)?;
            tracing::debug!("Fallback: Calling OCRmyPDF with args: {}", args.to_json());
            out.ocr_calls.push(args.clone());
            match self.backend.ocr(&args) {
                Ok(()) => {
                    let r = self.backend.extract_text_report(Some(&sidecar_fb), &archive_fb, mode);
                    text = r.text;
                    truncated = r.truncated;
                    if produce_archive {
                        out.archive_path = Some(archive_fb);
                    }
                }
                Err(e) => return Err(Self::wrap(&e)),
            }
        }

        (out.text, out.truncated) = match text.filter(|t| !t.is_empty()) {
            Some(t) => (t, truncated),
            None if original_has_text => (text_original.unwrap_or_default(), trunc_original),
            None => {
                tracing::warn!("No text was found in {}, the content will be empty.", document_path.display());
                (String::new(), None)
            }
        };
        Ok(out)
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `documents.consumer.should_produce_archive` (parser properties as values).
/// `born_digital` is called only for PDFs in mode `auto`.
pub fn should_produce_archive(
    requires_pdf_rendition: bool,
    can_produce_archive: bool,
    generation: ArchiveFileGeneration,
    mime_type: &str,
    born_digital: impl FnOnce() -> bool,
) -> bool {
    if requires_pdf_rendition {
        return true;
    }
    if !can_produce_archive {
        return false;
    }
    match generation {
        ArchiveFileGeneration::Always => return true,
        ArchiveFileGeneration::Never => return false,
        ArchiveFileGeneration::Auto => {}
    }
    if mime_type.starts_with("image/") {
        return true;
    }
    if mime_type == "application/pdf" {
        return !born_digital();
    }
    false
}

/// `RemoteDocumentParser.parse` – decision; the Azure call is in [`crate::remote::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteDecision {
    /// No valid engine: content empty (warning).
    EmptyNoEngine,
    /// PDF without archive request: local text (`post_process_text(extract_pdf_text) or ""`).
    LocalText,
    /// Send the document to Azure (`prebuilt-read`, output text + searchable PDF).
    CallAzure,
}

pub fn remote_decision(engine_valid: bool, engine: Option<&str>, mime_type: &str, produce_archive: bool) -> Option<RemoteDecision> {
    if !engine_valid {
        return Some(RemoteDecision::EmptyNoEngine);
    }
    if !produce_archive && mime_type == "application/pdf" {
        return Some(RemoteDecision::LocalText);
    }
    // Only "azureai" is known; otherwise the text stays None (→ "").
    (engine == Some("azureai")).then_some(RemoteDecision::CallAzure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfigOverrides, OcrSettings};
    use std::cell::RefCell;

    const LONG: &str = "This is a test document with enough text. This is a test document with enough text. This is a test document with enough text. This is a test document with enough text. This is a test document with enough text.";

    #[derive(Default)]
    struct Mock {
        /// Responses of `extract_text` in order (last one repeated).
        texts: RefCell<Vec<Option<String>>>,
        tagged: bool,
        /// Results of `ocr` in order (empty → Ok).
        ocr_results: RefCell<Vec<Result<()>>>,
        ocr_calls: RefCell<u32>,
        img_calls: RefCell<u32>,
        gs_calls: RefCell<u32>,
        gs_result: RefCell<Option<OcrError>>,
        extract_calls: RefCell<Vec<Option<PathBuf>>>,
        /// Truncations reported next to `texts`, in the same order (last one repeated).
        truncations: RefCell<Vec<Option<Truncation>>>,
    }
    impl Mock {
        fn with_text(t: Option<&str>) -> Self {
            Mock { texts: RefCell::new(vec![t.map(str::to_string)]), ..Default::default() }
        }
        fn with_texts(t: &[Option<&str>]) -> Self {
            Mock { texts: RefCell::new(t.iter().map(|x| x.map(str::to_string)).collect()), ..Default::default() }
        }
    }
    impl OcrBackend for Mock {
        fn extract_pdf_text(&self, _: &Path) -> Option<String> {
            unreachable!()
        }
        fn is_tagged_pdf(&self, _: &Path) -> bool {
            self.tagged
        }
        fn ocr(&self, _: &OcrArgs) -> Result<()> {
            *self.ocr_calls.borrow_mut() += 1;
            let mut r = self.ocr_results.borrow_mut();
            if r.is_empty() { Ok(()) } else { r.remove(0) }
        }
        fn convert_image_to_pdfa(&self, _: &Path, _: &OcrConfig, _: &Path) -> Result<PathBuf> {
            *self.img_calls.borrow_mut() += 1;
            Ok(PathBuf::from("/tmp/fake-archive.pdf"))
        }
        fn convert_pdf_to_pdfa(&self, _: &Path, _: &Path, _: &OcrConfig) -> Result<()> {
            *self.gs_calls.borrow_mut() += 1;
            match self.gs_result.borrow_mut().take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
        fn extract_text_report(&self, sidecar: Option<&Path>, _: &Path, _: Mode) -> PdfText {
            self.extract_calls.borrow_mut().push(sidecar.map(Path::to_path_buf));
            let mut t = self.texts.borrow_mut();
            let text = if t.len() > 1 { t.remove(0) } else { t.first().cloned().flatten() };
            let mut tr = self.truncations.borrow_mut();
            let truncated = if tr.len() > 1 { tr.remove(0) } else { tr.first().cloned().flatten() };
            PdfText { text, truncated }
        }
    }

    struct NoImg;
    impl ImageOps for NoImg {
        fn get_dpi(&self, _: &str) -> Option<i64> {
            Some(300)
        }
        fn calculate_a4_dpi(&self, _: &str) -> Option<i64> {
            Some(62)
        }
        fn has_alpha(&self, _: &str) -> Result<bool> {
            Ok(false)
        }
        fn remove_alpha(&self, p: &str) -> Result<String> {
            Ok(p.into())
        }
    }

    fn parser<'a>(m: &'a Mock, mode: Mode) -> RasterisedParser<'a> {
        let mut settings =
            OcrConfig::new(&OcrSettings { threads_per_worker: 1, ..Default::default() }, &AppConfigOverrides::default()).unwrap();
        settings.mode = mode;
        RasterisedParser { settings, tempdir: PathBuf::from("/tmp/pl-ocr-test"), backend: m, images: &NoImg }
    }
    const PDF: &str = "application/pdf";

    // --- upstream: tests/parsers/test_parse_modes.py ---
    #[test]
    fn auto_text_no_archive_skips_ocrmypdf() {
        let m = Mock::with_text(Some(LONG));
        let r = parser(&m, Mode::Auto).parse(Path::new("d.pdf"), PDF, false).unwrap();
        assert_eq!(*m.ocr_calls.borrow(), 0);
        assert_eq!(r.archive_path, None);
        assert_eq!(r.text, LONG);
    }

    #[test]
    fn auto_text_with_archive_calls_ocrmypdf_skip_text() {
        let m = Mock::with_text(Some(LONG));
        let r = parser(&m, Mode::Auto).parse(Path::new("d.pdf"), PDF, true).unwrap();
        assert_eq!(r.ocr_calls.len(), 1);
        let a = &r.ocr_calls[0];
        assert_eq!(a.get("skip_text"), Some(&serde_json::json!(true)));
        assert!(a.get("force_ocr").is_none() && a.get("redo_ocr").is_none());
        assert!(r.archive_path.is_some());
    }

    #[test]
    fn auto_no_text_with_archive_calls_ocrmypdf_no_extra_flag() {
        let m = Mock::with_texts(&[None, Some(LONG)]);
        let r = parser(&m, Mode::Auto).parse(Path::new("d.pdf"), PDF, true).unwrap();
        assert_eq!(r.ocr_calls.len(), 1);
        let a = &r.ocr_calls[0];
        assert!(a.get("skip_text").is_none() && a.get("force_ocr").is_none() && a.get("redo_ocr").is_none());
        assert!(r.archive_path.is_some());
        assert_eq!(r.text, LONG);
    }

    #[test]
    fn auto_no_text_no_archive_calls_ocrmypdf() {
        let m = Mock::with_texts(&[None, Some(LONG)]);
        let r = parser(&m, Mode::Auto).parse(Path::new("d.pdf"), PDF, false).unwrap();
        assert_eq!(*m.ocr_calls.borrow(), 1);
        assert_eq!(r.archive_path, None);
    }

    #[test]
    fn off_no_archive_returns_pdftotext() {
        let m = Mock::with_text(Some(LONG));
        let r = parser(&m, Mode::Off).parse(Path::new("d.pdf"), PDF, false).unwrap();
        assert_eq!(*m.ocr_calls.borrow(), 0);
        assert_eq!(r.archive_path, None);
        assert_eq!(r.text, LONG);
    }

    #[test]
    fn off_with_archive_uses_ghostscript_not_ocr() {
        let m = Mock::with_text(Some(LONG));
        let r = parser(&m, Mode::Off).parse(Path::new("d.pdf"), PDF, true).unwrap();
        assert_eq!(*m.ocr_calls.borrow(), 0);
        assert_eq!(*m.gs_calls.borrow(), 1);
        assert_eq!(r.archive_path, Some(PathBuf::from("/tmp/pl-ocr-test").join("archive.pdf")));
        assert_eq!(r.text, LONG);
    }

    #[test]
    fn off_image_no_archive_no_ocrmypdf() {
        let m = Mock::default();
        let r = parser(&m, Mode::Off).parse(Path::new("s.png"), "image/png", false).unwrap();
        assert_eq!(*m.ocr_calls.borrow(), 0);
        assert_eq!(r.archive_path, None);
        assert_eq!(r.text, "");
    }

    #[test]
    fn off_image_with_archive_uses_img2pdf_path() {
        let m = Mock::default();
        let r = parser(&m, Mode::Off).parse(Path::new("s.png"), "image/png", true).unwrap();
        assert_eq!(*m.img_calls.borrow(), 1);
        assert_eq!(*m.ocr_calls.borrow(), 0);
        assert_eq!(r.archive_path, Some(PathBuf::from("/tmp/fake-archive.pdf")));
        assert_eq!(r.text, "");
    }

    #[test]
    fn produce_archive_false_force_redo_modes() {
        for mode in [Mode::Force, Mode::Redo] {
            let m = Mock::with_text(Some(LONG));
            let r = parser(&m, mode).parse(Path::new("d.pdf"), PDF, false).unwrap();
            assert_eq!(r.archive_path, None);
            assert_eq!(r.text, LONG);
            let flag = if mode == Mode::Force { "force_ocr" } else { "redo_ocr" };
            assert!(r.ocr_calls[0].flag(flag));
        }
    }

    // --- upstream: test_tesseract_parser.py (with a fake instead of real OCRmyPDF) ---
    #[test]
    fn tagged_short_text_counts_as_text() {
        // test_tagged_pdf_skips_ocr_in_auto_mode
        let mut m = Mock::with_text(Some("Hi."));
        m.tagged = true;
        let r = parser(&m, Mode::Auto).parse(Path::new("d.pdf"), PDF, false).unwrap();
        assert_eq!(*m.ocr_calls.borrow(), 0);
        assert_eq!(r.text, "Hi.");
        // test_tagged_pdf_without_text_does_not_skip_ocr
        let mut m = Mock::with_texts(&[None, Some("ocr")]);
        m.tagged = true;
        parser(&m, Mode::Auto).parse(Path::new("d.pdf"), PDF, false).unwrap();
        assert_eq!(*m.ocr_calls.borrow(), 1);
    }

    // --- PaperRust: truncation of the PDF text extraction ---
    fn pages_cut() -> Truncation {
        Truncation::PdfPages { read: 2, total: 3, limit: 2 }
    }

    #[test]
    fn auto_text_no_archive_reports_truncation_of_the_original_text() {
        let m = Mock::with_text(Some(LONG));
        *m.truncations.borrow_mut() = vec![Some(pages_cut())];
        let r = parser(&m, Mode::Auto).parse(Path::new("d.pdf"), PDF, false).unwrap();
        assert_eq!(*m.ocr_calls.borrow(), 0);
        assert_eq!(r.text, LONG);
        assert_eq!(r.truncated, Some(pages_cut()));
    }

    #[test]
    fn off_mode_reports_truncation_of_the_original_text() {
        let m = Mock::with_text(Some(LONG));
        *m.truncations.borrow_mut() = vec![Some(pages_cut())];
        let r = parser(&m, Mode::Off).parse(Path::new("d.pdf"), PDF, false).unwrap();
        assert_eq!(r.truncated, Some(pages_cut()));
    }

    #[test]
    fn truncation_follows_the_text_that_is_used() {
        // original cut short, OCR text (sidecar) complete → the stored OCR text is complete
        let m = Mock::with_texts(&[Some(LONG), Some("ocr text")]);
        *m.truncations.borrow_mut() = vec![Some(pages_cut()), None];
        let r = parser(&m, Mode::Force).parse(Path::new("d.pdf"), PDF, true).unwrap();
        assert_eq!(r.text, "ocr text");
        assert_eq!(r.truncated, None);
        // original complete, archive text cut short → reported
        let m = Mock::with_texts(&[Some(LONG), Some("archive text")]);
        *m.truncations.borrow_mut() = vec![None, Some(pages_cut())];
        let r = parser(&m, Mode::Force).parse(Path::new("d.pdf"), PDF, true).unwrap();
        assert_eq!(r.text, "archive text");
        assert_eq!(r.truncated, Some(pages_cut()));
    }

    #[test]
    fn signed_pdf_keeps_truncation_of_the_original_text() {
        let m = Mock::with_text(Some(LONG));
        *m.truncations.borrow_mut() = vec![Some(pages_cut())];
        m.ocr_results.borrow_mut().push(Err(OcrError::DigitalSignature));
        let r = parser(&m, Mode::Auto).parse(Path::new("signed.pdf"), PDF, true).unwrap();
        assert_eq!(r.text, LONG);
        assert_eq!(r.truncated, Some(pages_cut()));
    }

    #[test]
    fn signed_skip_mode_no_archive() {
        let m = Mock::with_text(Some(LONG));
        m.ocr_results.borrow_mut().push(Err(OcrError::DigitalSignature));
        let r = parser(&m, Mode::Auto).parse(Path::new("signed.pdf"), PDF, true).unwrap();
        assert_eq!(r.archive_path, None);
        assert_eq!(r.text, LONG);
    }

    #[test]
    fn encrypted_skip_mode_empty_text() {
        let m = Mock::with_text(None);
        m.ocr_results.borrow_mut().push(Err(OcrError::EncryptedPdf));
        let r = parser(&m, Mode::Auto).parse(Path::new("encrypted.pdf"), PDF, true).unwrap();
        assert_eq!(r.archive_path, None);
        assert_eq!(r.text, "");
    }

    #[test]
    fn gs_rendering_error_raises_parse_error() {
        let m = Mock::with_text(Some(LONG));
        m.ocr_results.borrow_mut().push(Err(OcrError::SubprocessOutput("Ghostscript PDF/A rendering failed".into())));
        let e = parser(&m, Mode::Auto).parse(Path::new("d.pdf"), PDF, true).unwrap_err();
        assert_eq!(e.message(), "SubprocessOutputError: Ghostscript PDF/A rendering failed. See logs for more information.");
    }

    #[test]
    fn prior_ocr_found_falls_back_to_force() {
        // RTL note upstream: short text (<50), auto → PriorOcrFoundError → force_ocr
        let m = Mock::with_texts(&[Some("short"), Some("forced text")]);
        m.ocr_results.borrow_mut().push(Err(OcrError::PriorOcrFound("page already has text!".into())));
        let r = parser(&m, Mode::Auto).parse(Path::new("d.pdf"), PDF, true).unwrap();
        assert_eq!(r.ocr_calls.len(), 2);
        assert!(r.ocr_calls[0].get("force_ocr").is_none());
        assert!(r.ocr_calls[1].flag("force_ocr"));
        let t = PathBuf::from("/tmp/pl-ocr-test");
        assert_eq!(r.ocr_calls[1].str("output_file"), Some(t.join("archive-fallback.pdf").to_string_lossy().as_ref()));
        assert_eq!(r.ocr_calls[1].str("sidecar"), Some(t.join("sidecar-fallback.txt").to_string_lossy().as_ref()));
        assert_eq!(r.archive_path, Some(t.join("archive-fallback.pdf")));
        assert_eq!(r.text, "forced text");
    }

    #[test]
    fn no_text_after_ocr_falls_back_and_keeps_original() {
        // NoTextFoundException → fallback; fallback also returns nothing → original text (has_text)
        let m = Mock::with_texts(&[Some(LONG), None, None]);
        let r = parser(&m, Mode::Redo).parse(Path::new("d.pdf"), PDF, true).unwrap();
        assert_eq!(r.ocr_calls.len(), 2);
        assert_eq!(r.text, LONG);
        assert_eq!(r.archive_path, Some(PathBuf::from("/tmp/pl-ocr-test").join("archive-fallback.pdf")));
    }

    #[test]
    fn fallback_failure_is_parse_error() {
        let m = Mock::with_texts(&[None]);
        m.ocr_results.borrow_mut().push(Err(OcrError::InputFile("This PDF has a user fillable form.".into())));
        m.ocr_results.borrow_mut().push(Err(OcrError::Other("boom".into())));
        let e = parser(&m, Mode::Redo).parse(Path::new("d.pdf"), PDF, true).unwrap_err();
        assert_eq!(e.message(), "Exception: boom");
    }

    #[test]
    fn other_errors_wrapped() {
        let m = Mock::with_texts(&[None]);
        m.ocr_results.borrow_mut().push(Err(OcrError::DecompressionBomb("too big".into())));
        let e = parser(&m, Mode::Force).parse(Path::new("d.pdf"), PDF, true).unwrap_err();
        assert_eq!(e.message(), "DecompressionBombError: too big");
    }

    #[test]
    fn images_never_extract_original_text() {
        let m = Mock::with_texts(&[Some("img text")]);
        let r = parser(&m, Mode::Auto).parse(Path::new("s.png"), "image/png", true).unwrap();
        assert_eq!(m.extract_calls.borrow().len(), 1); // only after OCR
        assert_eq!(r.ocr_calls[0].get("image_dpi"), Some(&serde_json::json!(300)));
        assert_eq!(r.text, "img text");
    }

    #[test]
    fn off_pdf_gs_error() {
        let m = Mock::with_text(Some(LONG));
        *m.gs_result.borrow_mut() = Some(OcrError::Other("gs missing".into()));
        let e = parser(&m, Mode::Off).parse(Path::new("d.pdf"), PDF, true).unwrap_err();
        assert_eq!(e.message(), "Exception: gs missing");
    }

    // --- upstream: documents/consumer.py::should_produce_archive ---
    #[test]
    fn archive_decision() {
        use ArchiveFileGeneration::*;
        assert!(should_produce_archive(true, false, Never, "text/plain", || true));
        assert!(!should_produce_archive(false, false, Always, "application/pdf", || false));
        assert!(should_produce_archive(false, true, Always, "application/pdf", || true));
        assert!(!should_produce_archive(false, true, Never, "image/png", || false));
        assert!(should_produce_archive(false, true, Auto, "image/png", || unreachable!()));
        assert!(!should_produce_archive(false, true, Auto, "application/pdf", || true));
        assert!(should_produce_archive(false, true, Auto, "application/pdf", || false));
        assert!(!should_produce_archive(false, true, Auto, "text/plain", || false));
    }

    #[test]
    fn remote_decisions() {
        assert_eq!(remote_decision(false, None, PDF, true), Some(RemoteDecision::EmptyNoEngine));
        assert_eq!(remote_decision(true, Some("azureai"), PDF, false), Some(RemoteDecision::LocalText));
        assert_eq!(remote_decision(true, Some("azureai"), "image/png", false), Some(RemoteDecision::CallAzure));
    }
}
