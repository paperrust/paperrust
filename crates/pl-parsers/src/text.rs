//! Text parser (txt, csv) and the shared read function with encoding detection.
//!
//! upstream: src/paperless/parsers/text.py@v3.4.2 (TextDocumentParser)
//! upstream: src/paperless/parsers/utils.py@v3.4.2 (read_file_handle_unicode_errors)

use std::path::Path;

use crate::{DocumentParser, MetadataEntry, ParseContext, ParsedDocument, Result, read_limited};

const BOM_UTF8: &[u8] = b"\xef\xbb\xbf";
const BOM_UTF16_LE: &[u8] = b"\xff\xfe";
const BOM_UTF16_BE: &[u8] = b"\xfe\xff";

/// `read_file_handle_unicode_errors` on bytes:
/// - BOM UTF-16 LE/BE → `utf-16` (BOM is consumed), BOM UTF-8 → `utf-8-sig`, otherwise `utf-8`.
/// - On decode error: warning and `raw.decode("utf-8", errors="replace")` on the *raw*
///   bytes (incl. BOM).
/// - NUL characters are removed (PostgreSQL).
pub fn decode_text_bytes(raw: &[u8]) -> String {
    let strict: Option<String> = if raw.starts_with(BOM_UTF16_LE) {
        encoding_rs::UTF_16LE
            .decode_without_bom_handling_and_without_replacement(&raw[2..])
            .map(|c| c.into_owned())
            .filter(|_| raw.len().is_multiple_of(2))
    } else if raw.starts_with(BOM_UTF16_BE) {
        encoding_rs::UTF_16BE
            .decode_without_bom_handling_and_without_replacement(&raw[2..])
            .map(|c| c.into_owned())
            .filter(|_| raw.len().is_multiple_of(2))
    } else if let Some(rest) = raw.strip_prefix(BOM_UTF8) {
        std::str::from_utf8(rest).ok().map(str::to_owned)
    } else {
        std::str::from_utf8(raw).ok().map(str::to_owned)
    };
    let text = match strict {
        Some(t) => t,
        None => {
            tracing::warn!(target: "paperless.parsers.utils", "Unicode error during text reading, continuing");
            String::from_utf8_lossy(raw).into_owned()
        }
    };
    if text.contains('\0') { text.replace('\0', "") } else { text }
}

/// `read_file_handle_unicode_errors(path)`.
pub fn read_file_handle_unicode_errors(path: &Path, ctx: &ParseContext) -> Result<String> {
    let raw = read_limited(path, &ctx.limits)?;
    Ok(decode_text_bytes(&raw))
}

pub const TEXT_MIME_TYPES: &[(&str, &str)] = &[("text/plain", ".txt"), ("text/csv", ".csv"), ("application/csv", ".csv")];

/// upstream `TextDocumentParser`.
#[derive(Debug, Default)]
pub struct TextDocumentParser;

impl DocumentParser for TextDocumentParser {
    fn class_name(&self) -> &'static str {
        "TextDocumentParser"
    }
    fn name(&self) -> &'static str {
        "PaperRust Text Parser"
    }
    fn supported_mime_types(&self) -> &'static [(&'static str, &'static str)] {
        TEXT_MIME_TYPES
    }
    fn score(&self, mime_type: &str, _filename: &str, _path: Option<&Path>) -> Option<i32> {
        TEXT_MIME_TYPES.iter().any(|(m, _)| *m == mime_type).then_some(10)
    }
    fn can_produce_archive(&self) -> bool {
        false
    }
    fn requires_pdf_rendition(&self) -> bool {
        false
    }
    fn parse(&self, path: &Path, _mime_type: &str, ctx: &ParseContext) -> Result<ParsedDocument> {
        Ok(ParsedDocument { text: read_file_handle_unicode_errors(path, ctx)?, ..Default::default() })
    }
    fn page_count(&self, _path: &Path, _mime_type: &str, _ctx: &ParseContext) -> Option<u32> {
        None
    }
    fn extract_metadata(&self, _p: &Path, _m: &str, _c: &ParseContext) -> Vec<MetadataEntry> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings() {
        assert_eq!(decode_text_bytes("äb".as_bytes()), "äb");
        assert_eq!(decode_text_bytes(b"\xef\xbb\xbfx"), "x");
        assert_eq!(decode_text_bytes(b"\xff\xfea\x00"), "a");
        assert_eq!(decode_text_bytes(b"\xfe\xff\x00a"), "a");
        // UTF-16 with odd length -> fallback utf-8 replace on raw bytes
        assert_eq!(decode_text_bytes(b"\xff\xfea\x00b"), "\u{fffd}\u{fffd}ab");
        assert_eq!(decode_text_bytes(b"a\xfcb\x00c"), "a\u{fffd}bc");
        // UTF-8 BOM + invalid -> BOM is kept as U+FEFF
        assert_eq!(decode_text_bytes(b"\xef\xbb\xbf\xff"), "\u{feff}\u{fffd}");
    }
}
