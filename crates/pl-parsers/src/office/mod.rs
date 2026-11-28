//! Office documents without Tika: DOCX/XLSX/PPTX/PPSX (OOXML), ODT/ODS/ODP/ODG (ODF), RTF.
//!
//! upstream: src/paperless/parsers/tika.py@v3.4.2 (TikaDocumentParser: MIME list, score,
//! parse → `parsed.content.strip()`, `parsed.created` as date (naive → make_aware),
//! extract_metadata → list `{namespace:"", prefix:"", key, value}` of all Tika metadata).
//!
//! Deviation class (documented in the contract): Tika formats paragraphs and tables slightly
//! differently. Excel cells are output like Tika/POI by their cell format ([`numfmt`]:
//! date, percent, thousands …). The metadata follows Tika's keys (core, app and
//! custom properties); Tika's processing info (`X-TIKA:*`) is deliberately missing.
//!
//! DOC/XLS/PPT (OLE2) is read by [`ole`] (Word 97 piece table, BIFF8, PowerPoint persistence).
//!
//! For the archive PDF of presentations, [`slides`] supplies the slides (title, text lines,
//! notes) and [`slides_html`] a plain HTML page with one slide per page, which the
//! PDF generator (WordCraft in a child process) sets in landscape.

pub mod numfmt;
pub mod odf;
pub mod ole;
pub mod ooxml;
pub mod rtf;

use std::path::Path;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};

use crate::{Deadline, DocumentParser, MetadataEntry, ParseContext, ParseError, ParsedDocument, Result, read_limited};

/// Tika MIME list in upstream order.
pub const TIKA_MIME_TYPES: &[(&str, &str)] = &[
    ("application/msword", ".doc"),
    ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", ".docx"),
    ("application/vnd.ms-excel", ".xls"),
    ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", ".xlsx"),
    ("application/vnd.ms-powerpoint", ".ppt"),
    ("application/vnd.openxmlformats-officedocument.presentationml.presentation", ".pptx"),
    ("application/vnd.openxmlformats-officedocument.presentationml.slideshow", ".ppsx"),
    ("application/vnd.oasis.opendocument.presentation", ".odp"),
    ("application/vnd.oasis.opendocument.spreadsheet", ".ods"),
    ("application/vnd.oasis.opendocument.text", ".odt"),
    ("application/vnd.oasis.opendocument.graphics", ".odg"),
    ("text/rtf", ".rtf"),
];

/// Formats that cannot (yet) be read without Tika. Empty since gap E9 (old
/// Word/Excel/PowerPoint is read by [`ole`]); remains as a public constant.
pub const UNSUPPORTED_LEGACY: &[&str] = &[];

/// Presentations and drawings: archive PDF via [`slides_html`] (one slide per page).
pub const PRESENTATION_MIME_TYPES: &[&str] = &[
    "application/vnd.ms-powerpoint",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
    "application/vnd.oasis.opendocument.presentation",
    "application/vnd.oasis.opendocument.graphics",
];

/// Is this a presentation/drawing (archive PDF from slides)?
pub fn is_presentation(mime_type: &str) -> bool {
    PRESENTATION_MIME_TYPES.contains(&mime_type)
}

/// A slide (or drawing page).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slide {
    /// Title (placeholder "Titel"), if recognizable.
    pub title: Option<String>,
    /// Remaining text lines in reading order of the file.
    pub lines: Vec<String>,
    /// Notes for the slide.
    pub notes: Vec<String>,
}

/// Slides of a presentation/drawing (PPTX/PPSX, ODP/ODG, PPT).
pub fn slides(data: &[u8], mime_type: &str, ctx: &ParseContext) -> Result<Vec<Slide>> {
    let deadline = Deadline::from_limits(&ctx.limits);
    match mime_type {
        "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/vnd.openxmlformats-officedocument.presentationml.slideshow" => ooxml::pptx_slides(data, &ctx.limits, deadline),
        "application/vnd.oasis.opendocument.presentation" | "application/vnd.oasis.opendocument.graphics" => {
            odf::odp_slides(data, &ctx.limits, deadline)
        }
        "application/vnd.ms-powerpoint" => ole::ppt_slides(data, &ctx.limits, deadline),
        m => Err(ParseError::Unsupported(format!("{m} has no slides"))),
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\n' => o.push_str("<br>"),
            c if c.is_control() && c != '\t' => {}
            c => o.push(c),
        }
    }
    o
}

/// HTML page with one slide per page (page break before every further slide): small
/// slide number, title as heading, text lines as paragraphs, notes small below.
pub fn slides_html(slides: &[Slide]) -> String {
    let mut h = String::from("<html><head><meta charset=\"utf-8\"></head><body>");
    let n = slides.len();
    for (i, s) in slides.iter().enumerate() {
        let brk = if i > 0 { "page-break-before:always;" } else { "" };
        h.push_str(&format!("<p style=\"{brk}font-size:8pt;color:#808080\">{} / {n}</p>", i + 1));
        if let Some(t) = s.title.as_deref().filter(|t| !t.trim().is_empty()) {
            h.push_str(&format!("<h1>{}</h1>", esc(t)));
        }
        for l in &s.lines {
            h.push_str(&format!("<p>{}</p>", esc(l)));
        }
        if !s.notes.is_empty() {
            h.push_str("<p style=\"font-size:8pt;color:#808080\"><i>Notes</i></p>");
            for l in &s.notes {
                h.push_str(&format!("<p style=\"font-size:9pt\">{}</p>", esc(l)));
            }
        }
    }
    if n == 0 {
        h.push_str("<p>(no slides)</p>");
    }
    h.push_str("</body></html>");
    h
}

/// Result of an office extraction.
#[derive(Debug, Default, Clone)]
pub struct OfficeContent {
    pub text: String,
    /// Tika-like metadata (keys like Tika, in fixed order).
    pub metadata: Vec<(String, String)>,
    /// Value of `dcterms:created` (Tika `parsed.created`).
    pub created: Option<String>,
    /// PaperRust: `text` stopped at the text-size limit (`max_text_chars`); `text` holds the part
    /// read so far instead of failing the whole document.
    pub truncated: Option<crate::limits::Truncation>,
}

/// Extracts text and metadata of an office document from bytes.
pub fn extract(data: &[u8], mime_type: &str, ctx: &ParseContext) -> Result<OfficeContent> {
    let deadline = Deadline::from_limits(&ctx.limits);
    match mime_type {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/vnd.openxmlformats-officedocument.presentationml.slideshow" => {
            ooxml::extract(data, mime_type, &ctx.limits, deadline)
        }
        "application/vnd.oasis.opendocument.text"
        | "application/vnd.oasis.opendocument.spreadsheet"
        | "application/vnd.oasis.opendocument.presentation"
        | "application/vnd.oasis.opendocument.graphics" => odf::extract(data, mime_type, &ctx.limits, deadline),
        "text/rtf" => rtf::extract(data, &ctx.limits, deadline),
        ole::MIME_DOC | ole::MIME_XLS | ole::MIME_PPT => ole::extract(data, mime_type, &ctx.limits, deadline),
        m => Err(ParseError::Unsupported(m.to_owned())),
    }
}

/// Date like tika_client (`dateutil`/ISO) → aware (naive → `make_aware(TIME_ZONE)`).
pub fn parse_tika_date(s: &str, tz: chrono_tz::Tz) -> Option<DateTime<FixedOffset>> {
    let s = s.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d);
    }
    if let Ok(d) = DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%:z") {
        return Some(d);
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .ok()
        .or_else(|| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)))?;
    Some(crate::mail::make_aware(naive, tz).0)
}

/// Optional PDF generator (e.g. local LibreOffice) – guardrail "external services behind
/// traits". Without a generator there is no archive PDF.
pub trait PdfConverter: Send + Sync {
    fn convert(&self, path: &Path, mime_type: &str, out_dir: &Path) -> Result<std::path::PathBuf>;
}

/// Replacement for upstream `TikaDocumentParser` (class name stays for parity comparisons).
pub struct OfficeDocumentParser {
    pub converter: Option<Box<dyn PdfConverter>>,
    /// upstream `settings.TIKA_ENABLED`: controls whether the parser accepts files.
    pub enabled: bool,
}

impl Default for OfficeDocumentParser {
    fn default() -> Self {
        Self { converter: None, enabled: true }
    }
}

impl std::fmt::Debug for OfficeDocumentParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfficeDocumentParser").field("enabled", &self.enabled).field("converter", &self.converter.is_some()).finish()
    }
}

impl DocumentParser for OfficeDocumentParser {
    fn class_name(&self) -> &'static str {
        "TikaDocumentParser"
    }
    fn name(&self) -> &'static str {
        "PaperRust Office Parser (Tika replacement)"
    }
    fn supported_mime_types(&self) -> &'static [(&'static str, &'static str)] {
        TIKA_MIME_TYPES
    }
    fn score(&self, mime_type: &str, _f: &str, _p: Option<&Path>) -> Option<i32> {
        if !self.enabled || UNSUPPORTED_LEGACY.contains(&mime_type) {
            return None;
        }
        TIKA_MIME_TYPES.iter().any(|(m, _)| *m == mime_type).then_some(10)
    }
    fn can_produce_archive(&self) -> bool {
        false
    }
    /// upstream: always `True`. Without a converter no PDF can be produced here → `false`, so that
    /// the worker stores the original without archive (docs/STANDALONE.md).
    fn requires_pdf_rendition(&self) -> bool {
        self.converter.is_some()
    }
    fn parse(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Result<ParsedDocument> {
        let data = read_limited(path, &ctx.limits)?;
        let c = extract(&data, mime_type, ctx)?;
        let date = c.created.as_deref().and_then(|s| parse_tika_date(s, ctx.time_zone));
        // PaperRust: a generator error (e.g. WordCraft in a child process) does not abort
        // reading – it continues as without a generator (original without archive PDF).
        let archive_path = match &self.converter {
            Some(conv) => {
                let dir = ctx.work_dir.as_deref().or_else(|| path.parent()).unwrap_or_else(|| Path::new("."));
                match conv.convert(path, mime_type, dir) {
                    Ok(p) => Some(p),
                    Err(ParseError::Unsupported(m)) => {
                        tracing::debug!(target: "paperless.parsing.tika", "No PDF version for {mime_type}: {m}");
                        None
                    }
                    Err(e) => {
                        tracing::warn!(target: "paperless.parsing.tika", "Office to PDF conversion failed, continuing without archive PDF: {e}");
                        None
                    }
                }
            }
            None => None,
        };
        Ok(ParsedDocument {
            text: crate::pystr::py_strip(&c.text).to_owned(),
            date,
            archive_path,
            truncated: c.truncated.into_iter().collect(),
        })
    }
    fn page_count(&self, _p: &Path, _m: &str, _c: &ParseContext) -> Option<u32> {
        // upstream: pages of the archive PDF; without archive None
        None
    }
    fn extract_metadata(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Vec<MetadataEntry> {
        let res = read_limited(path, &ctx.limits).and_then(|d| extract(&d, mime_type, ctx));
        match res {
            Ok(c) => c.metadata.into_iter().map(|(k, v)| MetadataEntry::new("", "", k, v)).collect(),
            Err(e) => {
                tracing::warn!(target: "paperless.parsing.tika", "Error while fetching document metadata for {}: {}", path.display(), e);
                Vec::new()
            }
        }
    }
}

/// Shared helpers for text output.
pub(crate) fn push_line(out: &mut crate::limits::TextSink, line: &str) -> Result<()> {
    out.push_str(line)?;
    out.push('\n')
}

#[cfg(test)]
mod converter_tests {
    use super::*;

    struct Failing;
    impl PdfConverter for Failing {
        fn convert(&self, _: &Path, _: &str, _: &Path) -> Result<std::path::PathBuf> {
            Err(ParseError::Invalid("WordCraft could not convert broken.rtf: boom".into()))
        }
    }

    struct Fixed(std::path::PathBuf);
    impl PdfConverter for Fixed {
        fn convert(&self, _: &Path, _: &str, _: &Path) -> Result<std::path::PathBuf> {
            Ok(self.0.clone())
        }
    }

    fn rtf(dir: &Path) -> std::path::PathBuf {
        let p = dir.join("a.rtf");
        std::fs::write(&p, br"{\rtf1\ansi Hallo Welt\par}").unwrap();
        p
    }

    /// PaperRust: a failing PDF generator does not abort reading.
    #[test]
    fn failing_converter_keeps_text_and_drops_archive() {
        let t = tempfile::tempdir().unwrap();
        let p = OfficeDocumentParser { converter: Some(Box::new(Failing)), enabled: true };
        assert!(p.requires_pdf_rendition());
        let d = p.parse(&rtf(t.path()), "text/rtf", &ParseContext::default()).unwrap();
        assert!(d.text.contains("Hallo Welt"), "{}", d.text);
        assert!(d.archive_path.is_none());
    }

    #[test]
    fn working_converter_gives_archive() {
        let t = tempfile::tempdir().unwrap();
        let pdf = t.path().join("x.pdf");
        let p = OfficeDocumentParser { converter: Some(Box::new(Fixed(pdf.clone()))), enabled: true };
        let d = p.parse(&rtf(t.path()), "text/rtf", &ParseContext::default()).unwrap();
        assert_eq!(d.archive_path, Some(pdf));
    }

    #[test]
    fn registry_converter_can_be_set() {
        let mut r = crate::ParserRegistry::new(crate::RegistryConfig::default());
        r.set_office_converter(Some(Box::new(Failing)));
        let p = r.get_parser_for_file("text/rtf", "a.rtf", None, true).unwrap();
        assert_eq!(p.class_name(), "TikaDocumentParser");
        assert!(p.requires_pdf_rendition());
        r.set_office_converter(None);
        assert!(!r.get_parser_for_file("text/rtf", "a.rtf", None, true).unwrap().requires_pdf_rendition());
    }
}
