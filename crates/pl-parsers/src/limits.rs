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
#[derive(Debug)]
pub(crate) struct TextSink {
    pub buf: String,
    max: usize,
}

impl TextSink {
    pub fn new(max: usize) -> Self {
        Self { buf: String::new(), max }
    }

    pub fn push_str(&mut self, s: &str) -> Result<()> {
        if self.buf.len().saturating_add(s.len()) > self.max {
            return Err(ParseError::LimitExceeded(format!("extracted text > {} bytes", self.max)));
        }
        self.buf.push_str(s);
        Ok(())
    }

    pub fn push(&mut self, c: char) -> Result<()> {
        let mut b = [0u8; 4];
        self.push_str(c.encode_utf8(&mut b))
    }
}
