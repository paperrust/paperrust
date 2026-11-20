// upstream: src/documents/consumer.py@v3.4.2 (ConsumerPlugin.run: `qpdf --replace-input`
// for extension .pdf + MIME in CONSUMER_PDF_RECOVERABLE_MIME_TYPES), src/documents/parsers.py
// (_repair_pdf_with_qpdf: `qpdf --warning-exit-0 --replace-input`)
//
//! qpdf replacement: read the file with lopdf (leniently; broken/missing cross references are rebuilt by
//! scanning the file, garbage before `%PDF-` within the first 1024 bytes is
//! skipped) and rewrite cleanly.

use std::path::Path;

use crate::{PdfOpsError, Result};

/// Result of a repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairOutcome {
    /// Rewritten without anomalies (qpdf exit 0).
    Clean,
    /// Rewritten, but the file was damaged (cross references rebuilt) – qpdf
    /// also writes then, but ends with exit 3 (warnings).
    RepairedWithWarnings,
}

impl RepairOutcome {
    /// qpdf-Exit-Code.
    pub fn exit_code(self) -> i32 {
        match self {
            RepairOutcome::Clean => 0,
            RepairOutcome::RepairedWithWarnings => 3,
        }
    }
}

/// Start of `%PDF-` within the first 1024 bytes (qpdf searches just as far).
pub fn header_offset(data: &[u8]) -> Option<usize> {
    let end = data.len().min(1024);
    data[..end].windows(5).position(|w| w == b"%PDF-")
}

/// Repair bytes → new PDF bytes. Errors (`PdfError`) like qpdf exit 2.
pub fn repair_bytes(data: &[u8]) -> Result<(Vec<u8>, RepairOutcome)> {
    let off = header_offset(data).ok_or_else(|| PdfOpsError::Pdf("can't find PDF header".into()))?;
    let body = &data[off..];
    let mut doc = crate::pdf::load_bytes(body, "input", None)?;
    let warned = doc.xref_recovery.is_some();
    let bytes = crate::pdf::save_to_vec(&mut doc)?;
    Ok((bytes, if warned { RepairOutcome::RepairedWithWarnings } else { RepairOutcome::Clean }))
}

/// `qpdf --replace-input <path>`: replace the file in place (atomically via a
/// neighbouring file). Encrypted files without an empty user password → `Password`.
pub fn replace_input(path: &Path) -> Result<RepairOutcome> {
    let data = std::fs::read(path)?;
    let (bytes, outcome) = repair_bytes(&data)?;
    let tmp = path.with_extension("~qpdf-temp");
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_leading_garbage_and_rebuilds_xref() {
        let t = tempfile::tempdir().unwrap();
        let mut d = crate::pdf::testpdf::make(&["Hello"]);
        let mut good = Vec::new();
        d.save_to(&mut good).unwrap();
        // garbage before it
        let p = t.path().join("a.pdf");
        let mut junk = b"\x00\x01garbage-bytes\n".to_vec();
        junk.extend_from_slice(&good);
        std::fs::write(&p, &junk).unwrap();
        assert!(replace_input(&p).is_ok());
        assert_eq!(crate::pdf::testpdf::texts(&p), ["Hello"]);
        assert!(std::fs::read(&p).unwrap().starts_with(b"%PDF-"));
        // broken startxref
        let mut broken = good.clone();
        let pos = broken.windows(9).rposition(|w| w == b"startxref").unwrap();
        broken.truncate(pos);
        broken.extend_from_slice(b"startxref\n999999\n%%EOF\n");
        let q = t.path().join("b.pdf");
        std::fs::write(&q, &broken).unwrap();
        let outcome = replace_input(&q).unwrap();
        assert_eq!(outcome, RepairOutcome::RepairedWithWarnings);
        assert_eq!(crate::pdf::testpdf::texts(&q), ["Hello"]);
        // not a PDF
        let r = t.path().join("c.pdf");
        std::fs::write(&r, b"just text").unwrap();
        assert!(replace_input(&r).is_err());
        assert_eq!(std::fs::read(&r).unwrap(), b"just text");
    }
}
