//! Job format between the server and the child process `paperrust-pdftool` (JSON over stdin/stdout).
//!
//! The child process only gets paths (read input, write output) and limits, never
//! database access or secrets. The reply is a single JSON line on stdout.

use serde::{Deserialize, Serialize};

/// A rectangle in PDF user coordinates (points, origin bottom left): `[x0, y0, x1, y1]`.
pub type Rect = [f64; 4];

/// An area to redact on a page (0-based).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub page: usize,
    pub rect: Rect,
}

/// Quality when shrinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ShrinkPreset {
    /// Lossless only: remove duplicate and unused objects, compress streams,
    /// images stay unchanged.
    Lossless,
    /// Images above 225 dpi to 150 dpi, JPEG quality 60 (black and white 300 dpi, CCITT G4).
    #[default]
    Standard,
    /// Images above 144 dpi to 96 dpi, JPEG quality 80 – for reading on screen.
    Strong,
}

impl ShrinkPreset {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "lossless" => Some(Self::Lossless),
            "standard" | "" => Some(Self::Standard),
            "strong" | "mobile" => Some(Self::Strong),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Lossless => "lossless",
            Self::Standard => "standard",
            Self::Strong => "strong",
        }
    }
}

/// Self-test failure cases (only debug builds of the child process run them): prove that
/// a crash, an infinite loop and a memory hog do not hit the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelfTest {
    Panic,
    Hang,
    Oom,
    Abort,
    StackOverflow,
}

/// What the child process should do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    /// Redact areas and search terms (really remove the content underneath).
    Redact {
        input: String,
        output: String,
        #[serde(default)]
        areas: Vec<Area>,
        #[serde(default)]
        terms: Vec<String>,
        #[serde(default)]
        remove_metadata: bool,
    },
    /// Find search terms (matches with rectangles, without changing anything).
    Search {
        input: String,
        terms: Vec<String>,
        #[serde(default = "default_max_hits")]
        max_hits: usize,
    },
    /// Shrinking.
    Shrink { input: String, output: String, preset: ShrinkPreset },
    /// Read leniently and rewrite cleanly.
    Repair { input: String, output: String },
    /// Convert an office/text document to a PDF with WordCraft. `format` is the file extension
    /// without a dot (see [`office_format`]).
    Office { input: String, output: String, format: String },
    /// HTML page prepared by the server (slides of a presentation, one page per slide with
    /// page break) set to A4 landscape and written as PDF.
    Slides { input: String, output: String },
    /// Decode an HEIC/HEIF photo (iPhone) and write it as PNG (rotation/mirroring/crop of the
    /// container applied, ICC profile carried over). `max_pixels` = 0: default limit.
    Heic {
        input: String,
        output: String,
        #[serde(default)]
        max_pixels: u64,
    },
    /// Self-test (isolation proof).
    SelfTest { mode: SelfTest },
}

/// Formats that `Op::Office` converts: (MIME type, extension for WordCraft).
///
/// Word processing only: WordCraft cannot read spreadsheets (.xlsx/.ods) and presentations (.pptx/.odp);
/// PaperRust does not accept old Word (.doc) at all (no parser).
pub const OFFICE_FORMATS: &[(&str, &str)] = &[
    ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", "docx"),
    ("application/vnd.ms-word.document.macroenabled.12", "docm"),
    ("application/vnd.openxmlformats-officedocument.wordprocessingml.template", "dotx"),
    ("application/vnd.oasis.opendocument.text", "odt"),
    ("application/vnd.oasis.opendocument.text-template", "ott"),
    ("text/rtf", "rtf"),
    ("application/rtf", "rtf"),
    ("text/markdown", "md"),
    ("text/html", "html"),
];

/// Further formats that `Op::Office` converts into an archive PDF on **ingestion** (gaps E9/E10):
/// old Word (.doc, via WordCraft `craft-doc-binary`) and spreadsheets (.xlsx/.xlsm/.xls/.ods: cells
/// via WordCraft `sheet`, one table per sheet). Also "convert to PDF"
/// (pl-documents-write) uses [`document_format`].
pub const DOCUMENT_FORMATS: &[(&str, &str)] = &[
    ("application/msword", "doc"),
    ("application/vnd.ms-excel", "xls"),
    ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "xlsx"),
    ("application/vnd.ms-excel.sheet.macroenabled.12", "xlsm"),
    ("application/vnd.oasis.opendocument.spreadsheet", "ods"),
];

/// Extensions that the child process additionally accepts without a MIME type pointing to them:
/// `mht` (web archive built by the mail parser: HTML part with embedded `cid:` images).
const EXTRA_FORMATS: &[&str] = &["mht", "dot"];

fn mime_key(mime_type: &str) -> String {
    mime_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// Extension for WordCraft for a MIME type (`None` = not convertible). Word processing only.
pub fn office_format(mime_type: &str) -> Option<&'static str> {
    let m = mime_key(mime_type);
    OFFICE_FORMATS.iter().find(|(t, _)| *t == m).map(|(_, e)| *e)
}

/// Like [`office_format`], additionally old Word and spreadsheets ([`DOCUMENT_FORMATS`]).
pub fn document_format(mime_type: &str) -> Option<&'static str> {
    office_format(mime_type).or_else(|| {
        let m = mime_key(mime_type);
        DOCUMENT_FORMATS.iter().find(|(t, _)| *t == m).map(|(_, e)| *e)
    })
}

/// Extensions that the child process accepts.
pub fn office_format_known(ext: &str) -> bool {
    OFFICE_FORMATS.iter().chain(DOCUMENT_FORMATS).any(|(_, e)| *e == ext) || EXTRA_FORMATS.contains(&ext)
}

fn default_max_hits() -> usize {
    500
}

/// A job with limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    #[serde(flatten)]
    pub op: Op,
    /// Maximum heap usage of the child process in MiB (0 = no limit).
    #[serde(default)]
    pub memory_mb: u64,
}

/// A search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    /// 0-based page.
    pub page: usize,
    /// The term that was found.
    pub term: String,
    /// The line it is in (for confirmation in the UI).
    pub context: String,
    /// Rectangles (words that contain the match), PDF user coordinates.
    pub rects: Vec<Rect>,
}

/// Report after redacting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RedactReport {
    pub marks: usize,
    pub pages: usize,
    pub glyphs: usize,
    pub images_removed: usize,
    pub images_cleared: usize,
    pub paths_removed: usize,
    pub paths_clipped: usize,
    pub annotations_removed: usize,
    pub metadata_removed: usize,
    pub term_hits: usize,
}

/// Report after shrinking.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShrinkReport {
    pub preset: String,
    pub size_before: u64,
    pub size_after: u64,
    pub images: usize,
    pub images_resampled: usize,
    pub images_recompressed: usize,
    pub unused_objects_removed: usize,
    pub streams_compressed: usize,
}

/// Report after repair.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepairReport {
    /// What the lenient reader had to repair (empty = the file was fine).
    pub log: Vec<String>,
    pub size_before: u64,
    pub size_after: u64,
}

/// Report after the office → PDF conversion.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConvertReport {
    /// Detected format (extension).
    pub format: String,
    pub size_before: u64,
    pub size_after: u64,
}

/// Reply of the child process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Reply {
    Redacted(RedactReport),
    Hits {
        hits: Vec<Hit>,
        truncated: bool,
    },
    Shrunk(ShrinkReport),
    Repaired(RepairReport),
    Converted(ConvertReport),
    /// Domain error (file not readable, redaction not provable, …). The text is intended for
    /// users and contains no paths.
    Failed {
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_roundtrip_and_shape() {
        let j = Job {
            op: Op::Redact {
                input: "in.pdf".into(),
                output: "out.pdf".into(),
                areas: vec![Area { page: 0, rect: [1.0, 2.0, 3.0, 4.0] }],
                terms: vec!["geheim".into()],
                remove_metadata: true,
            },
            memory_mb: 512,
        };
        let s = serde_json::to_string(&j).unwrap();
        assert!(s.starts_with(r#"{"op":"redact","#), "{s}");
        let back: Job = serde_json::from_str(&s).unwrap();
        assert_eq!(back, j);
        let r: Reply = serde_json::from_str(r#"{"status":"failed","error":"x"}"#).unwrap();
        assert_eq!(r, Reply::Failed { error: "x".into() });
    }

    #[test]
    fn office_formats() {
        assert_eq!(office_format("application/vnd.openxmlformats-officedocument.wordprocessingml.document"), Some("docx"));
        assert_eq!(office_format("text/rtf; charset=x"), Some("rtf"));
        assert_eq!(office_format("application/vnd.oasis.opendocument.text"), Some("odt"));
        assert_eq!(office_format("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"), None);
        assert_eq!(office_format("application/msword"), None);
        assert!(office_format_known("md"));
        assert_eq!(document_format("application/vnd.ms-excel"), Some("xls"));
        assert_eq!(document_format("application/msword"), Some("doc"));
        assert_eq!(document_format("text/rtf"), Some("rtf"));
        assert_eq!(document_format("application/vnd.ms-powerpoint"), None);
        assert!(office_format_known("xlsx") && office_format_known("mht") && office_format_known("doc"));
        let j: Job = serde_json::from_str(r#"{"op":"heic","input":"a.heic","output":"a.png"}"#).unwrap();
        assert_eq!(j.op, Op::Heic { input: "a.heic".into(), output: "a.png".into(), max_pixels: 0 });
        let j: Job = serde_json::from_str(r#"{"op":"slides","input":"a.html","output":"a.pdf"}"#).unwrap();
        assert!(matches!(j.op, Op::Slides { .. }));
        assert!(!office_format_known("exe"));
        let j = Job { op: Op::Office { input: "a.docx".into(), output: "a.pdf".into(), format: "docx".into() }, memory_mb: 0 };
        let s = serde_json::to_string(&j).unwrap();
        assert!(s.starts_with(r#"{"op":"office","#), "{s}");
        let r: Reply = serde_json::from_str(r#"{"status":"converted","format":"docx","size_before":1,"size_after":2}"#).unwrap();
        assert!(matches!(r, Reply::Converted(_)));
    }

    #[test]
    fn presets() {
        assert_eq!(ShrinkPreset::parse(" Strong "), Some(ShrinkPreset::Strong));
        assert_eq!(ShrinkPreset::parse("mobile"), Some(ShrinkPreset::Strong));
        assert_eq!(ShrinkPreset::parse(""), Some(ShrinkPreset::Standard));
        assert_eq!(ShrinkPreset::parse("x"), None);
        assert_eq!(serde_json::to_string(&ShrinkPreset::Lossless).unwrap(), r#""lossless""#);
    }
}
