//! Parser selection by score.
//!
//! upstream: src/paperless/parsers/registry.py@v3.4.2 (ParserRegistry.register_defaults,
//! get_parser_for_file, all_parsers), src/documents/parsers.py@v3.4.2 (is_mime_type_supported,
//! get_default_file_extension, is_file_ext_supported, get_supported_file_extensions).
//!
//! Order like `register_defaults`: Text, Remote, Tika, Mail, Tesseract. Highest score
//! wins, on a tie the first registered. The Remote parser (Azure) is not
//! reproduced (without configuration it always returns `None` upstream).

use std::collections::BTreeSet;
use std::path::Path;

use crate::DocumentParser;
use crate::html::HtmlDocumentParser;
use crate::mail::MailDocumentParser;
use crate::msg::MsgDocumentParser;
use crate::office::OfficeDocumentParser;
use crate::pdf::RasterisedDocumentParser;
use crate::text::TextDocumentParser;

pub use crate::office::PdfConverter;

#[derive(Debug, Clone, Copy)]
pub struct RegistryConfig {
    /// upstream `PAPERLESS_TIKA_ENABLED`. Here it controls the office parser (Tika replacement).
    /// Default `true`: without a Tika server, office formats are readable anyway (standalone target).
    pub tika_enabled: bool,
    /// Extensions beyond upstream (HTML documents, Outlook MSG). Default `false`.
    pub extended_formats: bool,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self { tika_enabled: true, extended_formats: false }
    }
}

pub struct ParserRegistry {
    parsers: Vec<Box<dyn DocumentParser>>,
}

/// Python `mimetypes` without OS tables (`MimeTypes(filenames=())`): all extensions per type in
/// insertion order; first = `guess_extension`. Only the types relevant for paperless.
const PY_MIMETYPES: &[(&str, &[&str])] = &[
    ("application/msword", &[".doc", ".dot", ".wiz"]),
    ("application/pdf", &[".pdf"]),
    ("application/vnd.ms-excel", &[".xls", ".xlb"]),
    ("application/vnd.ms-powerpoint", &[".ppt", ".pot", ".ppa", ".pps", ".pwz"]),
    ("image/bmp", &[".bmp"]),
    ("image/gif", &[".gif"]),
    ("image/heic", &[".heic"]),
    ("image/jpeg", &[".jpg", ".jpe", ".jpeg"]),
    ("image/png", &[".png"]),
    ("image/tiff", &[".tiff", ".tif"]),
    ("message/rfc822", &[".eml", ".mht", ".mhtml", ".nws"]),
    ("text/csv", &[".csv"]),
    ("text/plain", &[".txt", ".bat", ".c", ".h", ".ksh", ".pl", ".srt"]),
    ("application/json", &[".json"]),
    ("application/octet-stream", &[".bin", ".a", ".dll", ".exe", ".o", ".obj", ".so"]),
    ("application/zip", &[".zip"]),
    ("text/html", &[".html", ".htm"]),
    ("text/xml", &[".xml"]),
    ("text/javascript", &[".js", ".mjs"]),
    ("image/svg+xml", &[".svg"]),
    ("application/postscript", &[".ps", ".ai", ".eps"]),
    ("video/mp4", &[".mp4"]),
    ("audio/x-wav", &[".wav"]),
    ("image/avif", &[".avif"]),
    ("image/heif", &[".heif"]),
    ("application/gzip", &[".gz"]),
    ("text/calendar", &[".ics"]),
    ("text/vcard", &[".vcf"]),
    ("text/x-python", &[".py"]),
];

fn py_all_extensions(mime: &str) -> &'static [&'static str] {
    PY_MIMETYPES.iter().find(|(m, _)| *m == mime).map(|(_, e)| *e).unwrap_or(&[])
}

impl ParserRegistry {
    pub fn new(cfg: RegistryConfig) -> Self {
        let mut parsers: Vec<Box<dyn DocumentParser>> = vec![
            Box::new(TextDocumentParser),
            Box::new(OfficeDocumentParser { converter: None, enabled: cfg.tika_enabled }),
            Box::new(MailDocumentParser::default()),
            Box::new(RasterisedDocumentParser),
        ];
        if cfg.extended_formats {
            parsers.push(Box::new(HtmlDocumentParser));
            parsers.push(Box::new(MsgDocumentParser));
        }
        Self { parsers }
    }

    /// PaperRust: set the PDF generator for the office parser (e.g. WordCraft via
    /// `paperrust-pdftool`). `None` = back to no generator (no archive PDF).
    pub fn set_office_converter(&mut self, converter: Option<Box<dyn crate::office::PdfConverter>>) {
        let enabled =
            self.parsers.iter().find_map(|p| (p.class_name() == "TikaDocumentParser").then(|| p.score("text/rtf", "", None).is_some()));
        let Some(enabled) = enabled else { return };
        for p in self.parsers.iter_mut() {
            if p.class_name() == "TikaDocumentParser" {
                *p = Box::new(OfficeDocumentParser { converter, enabled });
                return;
            }
        }
    }

    /// Add a custom parser (upstream: entry-point plugins, evaluated *before* the built-in ones
    /// and win on a tie).
    /// Install or remove the PDF generator for the archive PDF of e-mails (gap E12).
    pub fn set_mail_converter(&mut self, converter: Option<std::sync::Arc<dyn crate::office::PdfConverter>>) {
        for p in self.parsers.iter_mut() {
            if p.class_name() == "MailDocumentParser" {
                *p = Box::new(MailDocumentParser { converter, ..MailDocumentParser::default() });
                return;
            }
        }
    }

    pub fn register_external(&mut self, p: Box<dyn DocumentParser>) {
        self.parsers.insert(0, p);
    }

    pub fn all_parsers(&self) -> impl Iterator<Item = &dyn DocumentParser> {
        self.parsers.iter().map(|b| b.as_ref())
    }

    /// `get_parser_for_file(mime_type, filename, path, allow_remote=...)`.
    pub fn get_parser_for_file(
        &self,
        mime_type: &str,
        filename: &str,
        path: Option<&Path>,
        allow_remote: bool,
    ) -> Option<&dyn DocumentParser> {
        let mut best: Option<(i32, &dyn DocumentParser)> = None;
        for p in self.all_parsers() {
            if !p.supported_mime_types().iter().any(|(m, _)| *m == mime_type) {
                continue;
            }
            if !allow_remote && p.uses_remote_service() {
                continue;
            }
            let Some(score) = p.score(mime_type, filename, path) else {
                continue;
            };
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, p));
            }
        }
        best.map(|(_, p)| p)
    }

    /// `is_mime_type_supported`.
    pub fn is_mime_type_supported(&self, mime_type: &str) -> bool {
        self.get_parser_for_file(mime_type, "", None, true).is_some()
    }

    /// `get_default_file_extension` (fallback: Python `mimetypes.guess_extension`).
    pub fn get_default_file_extension(&self, mime_type: &str) -> String {
        if let Some(p) = self.get_parser_for_file(mime_type, "", None, true)
            && let Some((_, ext)) = p.supported_mime_types().iter().find(|(m, _)| *m == mime_type)
        {
            return (*ext).to_owned();
        }
        py_all_extensions(mime_type).first().map(|s| (*s).to_owned()).unwrap_or_default()
    }

    /// `get_supported_file_extensions` (without score, like upstream).
    pub fn get_supported_file_extensions(&self) -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for p in self.all_parsers() {
            for (m, ext) in p.supported_mime_types() {
                for e in py_all_extensions(m) {
                    set.insert((*e).to_owned());
                }
                set.insert((*ext).to_owned());
            }
        }
        set
    }

    /// `is_file_ext_supported`.
    pub fn is_file_ext_supported(&self, ext: &str) -> bool {
        !ext.is_empty() && self.get_supported_file_extensions().contains(&ext.to_lowercase())
    }
}

impl Default for ParserRegistry {
    fn default() -> Self {
        Self::new(RegistryConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection() {
        let r = ParserRegistry::new(RegistryConfig { tika_enabled: false, extended_formats: false });
        assert_eq!(r.get_parser_for_file("text/plain", "a.txt", None, true).unwrap().class_name(), "TextDocumentParser");
        assert!(r.get_parser_for_file("application/vnd.oasis.opendocument.text", "a", None, true).is_none());
        assert_eq!(r.get_default_file_extension("application/vnd.oasis.opendocument.text"), "");
        assert_eq!(r.get_default_file_extension("image/tiff"), ".tif");
        assert_eq!(r.get_default_file_extension("application/zip"), ".zip");
        let r = ParserRegistry::default();
        assert_eq!(
            r.get_parser_for_file("application/vnd.oasis.opendocument.text", "a", None, true).unwrap().class_name(),
            "TikaDocumentParser"
        );
        // Gap E9: old Word/Excel/PowerPoint (OLE2) is read like upstream with Tika
        assert_eq!(r.get_parser_for_file("application/msword", "a.doc", None, true).unwrap().class_name(), "TikaDocumentParser");
        assert!(r.get_parser_for_file("text/html", "a.html", None, true).is_none());
        assert!(r.is_file_ext_supported(".JPEG"));
    }
}
