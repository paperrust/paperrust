//! Limits against zip bombs, huge XML files, broken files and infinite loops.
//!
//! Upstream has no limits of its own (Tika/Gotenberg run under `CELERY_TASK_TIME_LIMIT`);
//! here they are needed because the parsers run in the worker process.

use std::time::{Duration, Instant};

use crate::{ParseError, Result};

#[derive(Debug, Clone)]
pub struct Limits {
    /// Maximum file size that is read at all.
    pub max_file_size: u64,
    /// Maximum number of entries in a ZIP (office container).
    pub max_zip_entries: usize,
    /// Maximum unpacked size of a single ZIP entry.
    pub max_entry_size: u64,
    /// Maximum total unpacked size of all ZIP entries read.
    pub max_total_uncompressed: u64,
    /// Maximum compression ratio (unpacked / packed) from `ratio_check_min` bytes onwards.
    pub max_compression_ratio: u64,
    pub ratio_check_min: u64,
    /// Maximum nesting depth in XML/RTF/MIME.
    pub max_depth: usize,
    /// Maximum length of the extracted text (characters); beyond that, processing is aborted.
    pub max_text_chars: usize,
    /// Maximum number of pages for which PDF text is extracted.
    pub max_pdf_pages: u32,
    /// Time limit per parse operation.
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_size: 512 * 1024 * 1024,
            max_zip_entries: 20_000,
            max_entry_size: 256 * 1024 * 1024,
            max_total_uncompressed: 1024 * 1024 * 1024,
            max_compression_ratio: 200,
            ratio_check_min: 16 * 1024 * 1024,
            max_depth: 256,
            max_text_chars: 64 * 1024 * 1024,
            max_pdf_pages: 5_000,
            timeout: Duration::from_secs(300),
        }
    }
}

/// Point in time after which a parse operation aborts.
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    start: Instant,
    limit: Duration,
}

impl Deadline {
    pub fn new(limit: Duration) -> Self {
        Self { start: Instant::now(), limit }
    }

    pub fn from_limits(l: &Limits) -> Self {
        Self::new(l.timeout)
    }

    pub fn check(&self) -> Result<()> {
        if self.start.elapsed() > self.limit { Err(ParseError::Timeout(self.limit)) } else { Ok(()) }
    }
}

/// Bounded text buffer: aborts with `LimitExceeded` when `max_text_chars` is exceeded.
///
/// On overflow the part of the input that still fits is kept (cut at a character boundary) and
/// the sink is marked full; [`TextSink::finish`] then turns the abort into a
/// [`Truncation::Text`] next to the text read so far instead of an error.
#[derive(Debug)]
pub(crate) struct TextSink {
    pub buf: String,
    max: usize,
    full: bool,
}

impl TextSink {
    pub fn new(max: usize) -> Self {
        Self { buf: String::new(), max, full: false }
    }

    fn overflow(&self) -> ParseError {
        ParseError::LimitExceeded(format!("extracted text > {} bytes", self.max))
    }

    pub fn push_str(&mut self, s: &str) -> Result<()> {
        if self.full {
            return Err(self.overflow());
        }
        if self.buf.len().saturating_add(s.len()) > self.max {
            let mut cut = self.max.saturating_sub(self.buf.len()).min(s.len());
            while cut > 0 && !s.is_char_boundary(cut) {
                cut -= 1;
            }
            self.buf.push_str(&s[..cut]);
            self.full = true;
            return Err(self.overflow());
        }
        self.buf.push_str(s);
        Ok(())
    }

    pub fn push(&mut self, c: char) -> Result<()> {
        let mut b = [0u8; 4];
        self.push_str(c.encode_utf8(&mut b))
    }

    /// Truncation for the result `r` of filling this sink: `Ok(None)` when filling succeeded,
    /// `Ok(Some(Truncation::Text))` when it stopped at the text-size limit (the text read so far
    /// stays in [`TextSink::buf`]; logged at WARN), the error otherwise.
    pub fn truncation(&self, r: Result<()>) -> Result<Option<Truncation>> {
        match r {
            Ok(()) => Ok(None),
            Err(_) if self.full => {
                let t = Truncation::Text { limit: self.max };
                tracing::warn!(target: "paperless.parsers.utils", "Extracted text is incomplete: {t}");
                Ok(Some(t))
            }
            Err(e) => Err(e),
        }
    }
}

/// A limit that cut extraction short while the text read so far was kept.
///
/// Limits that abort a parse surface as [`ParseError::LimitExceeded`]/[`ParseError::Timeout`].
/// A few extractors (PDF text, MSG properties, the text-size limit of Office/HTML/mail text)
/// instead keep the partial result, because
/// "some text" beats "no document"; they must never do that silently, so they return this
/// value next to the text and log it at WARN. The consumer can turn [`Truncation::message`]
/// into a document note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Truncation {
    /// `max_pdf_pages` reached: pages after `read` were not extracted.
    PdfPages { read: u32, total: u32, limit: u32 },
    /// The per-parse time limit expired after `read` pages.
    PdfTime { read: u32, total: u32, limit: Duration },
    /// `max_text_chars` reached after `read` pages.
    PdfText { read: u32, total: u32, limit: usize },
    /// An MSG property stream was larger than the per-stream limit; only `limit` bytes were read.
    MsgProperty { stream: String, size: u64, limit: u64 },
    /// `max_text_chars` reached by a text extractor without pages (Office, HTML, mail bodies):
    /// only the first `limit` bytes of text were kept.
    Text { limit: usize },
}

impl Truncation {
    /// Descriptions of several truncations joined by `; ` (`None` without any).
    pub fn join(t: &[Truncation]) -> Option<String> {
        (!t.is_empty()).then(|| t.iter().map(Truncation::message).collect::<Vec<_>>().join("; "))
    }

    /// Human-readable description (English, for logs and document notes).
    pub fn message(&self) -> String {
        match self {
            Truncation::PdfPages { read, total, limit } => {
                format!("Text extraction stopped at the page limit ({limit}): only {read} of {total} pages were read")
            }
            Truncation::PdfTime { read, total, limit } => {
                format!("Text extraction timed out after {}s: only {read} of {total} pages were read", limit.as_secs())
            }
            Truncation::PdfText { read, total, limit } => {
                format!("Text extraction stopped at the text size limit ({limit} bytes): only {read} of {total} pages were read")
            }
            Truncation::MsgProperty { stream, size, limit } => {
                format!("MSG property {stream} is {size} bytes, only the first {limit} bytes were read")
            }
            Truncation::Text { limit } => {
                format!("Text extraction stopped at the text size limit ({limit} bytes): only the first {limit} bytes of text were kept")
            }
        }
    }
}

impl std::fmt::Display for Truncation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_keeps_what_fits_and_reports_text_truncation() {
        let mut s = TextSink::new(5);
        assert!(s.push_str("ab").is_ok());
        // room for 3 more bytes: "cd" fits, the 2-byte "\u{e9}" does not and is cut at its boundary
        let r = s.push_str("cd\u{e9}");
        assert!(matches!(r, Err(ParseError::LimitExceeded(_))));
        assert_eq!(s.buf, "abcd");
        // full: nothing more is accepted, not even a single byte that would still fit
        assert!(s.push('x').is_err());
        assert_eq!(s.buf, "abcd");
        assert_eq!(s.truncation(Err(ParseError::LimitExceeded(String::new()))).unwrap(), Some(Truncation::Text { limit: 5 }));
        // other errors stay errors; success reports nothing
        let fresh = TextSink::new(5);
        assert!(fresh.truncation(Err(ParseError::Invalid("x".into()))).is_err());
        assert_eq!(fresh.truncation(Ok(())).unwrap(), None);
    }
}
