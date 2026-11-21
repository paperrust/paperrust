// PaperRust-specific (no upstream counterpart): repair PDFs and shrink large scans on
// ingestion, via pl-pdftools (PdfCraft in the child process). Contract: docs/contract/pl-pdftools.md §5.
//
//! Runs in `ConsumerPlugin.run` after MIME detection, before the parser:
//!
//! 1. **Repair:** if a PDF cannot be opened with lopdf (the reader of the parsers), the
//!    lenient repair of PdfCraft is tried. If it succeeds and the result is readable, processing
//!    continues with the repaired working copy; the **unchanged original** is stored.
//! 2. **Shrink (optional):** `PAPERRUST_SHRINK_ON_INGEST_MB` > 0 and a scan (hardly any text on
//!    the first pages) larger than this limit → shrink with `PAPERRUST_SHRINK_PRESET`. If
//!    the file gets smaller, the shrunk file is stored (that is the purpose: save space).
//!
//! Both are "best effort": every error (tool missing, time limit, crash of the child process)
//! is logged and ingestion continues with the previous file.

use std::path::{Path, PathBuf};

use pl_pdftools::job::{Op, Reply, ShrinkPreset};
use pl_pdftools::runner::{Limits, ToolLocation};

const LOG: &str = "paperless.consumer";

/// What happened to the working copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestOutcome {
    /// Working copy was replaced by a repaired version.
    pub repaired: bool,
    /// Working copy was replaced by a shrunk version (before, after in bytes).
    pub shrunk: Option<(u64, u64)>,
    /// Repair or shrinking would have been necessary, but `paperrust-pdftool` is missing: skipped,
    /// ingestion continues with the file as delivered.
    pub skipped_no_tool: bool,
}

/// Readability for the parsers (lopdf).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readability {
    /// Opens; `text_chars` = characters of text on the first pages.
    Ok { text_chars: usize },
    /// Encrypted (password) – do not touch, the parser decides.
    Encrypted,
    /// Does not open.
    Broken,
}

/// Open with lopdf and count the text of the first three pages.
pub fn readability(path: &Path) -> Readability {
    match pl_pdfops::pdf::load(path, None) {
        Ok(doc) => {
            let pages: Vec<u32> = doc.get_pages().keys().copied().take(3).collect();
            if pages.is_empty() {
                // opens, but no readable page tree: as good as broken for the parsers
                return Readability::Broken;
            }
            let text_chars = doc.extract_text(&pages).map(|t| t.chars().filter(|c| !c.is_whitespace()).count()).unwrap_or(0);
            Readability::Ok { text_chars }
        }
        Err(pl_pdfops::PdfOpsError::Password(_) | pl_pdfops::PdfOpsError::UnsupportedEncryption(_)) => Readability::Encrypted,
        Err(_) => Readability::Broken,
    }
}

/// Below this many characters of text on the first pages a PDF counts as a scan.
pub const SCAN_TEXT_CHARS: usize = 50;

/// `PAPERRUST_SHRINK_ON_INGEST_MB` (0/empty = off) in bytes.
pub fn shrink_threshold() -> Option<u64> {
    let mb = pl_core::config::var("PAPERRUST_SHRINK_ON_INGEST_MB").and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
    (mb > 0).then(|| mb.saturating_mul(1024 * 1024))
}

/// `PAPERRUST_SHRINK_PRESET` (Standard `standard`).
pub fn shrink_preset() -> ShrinkPreset {
    pl_core::config::var("PAPERRUST_SHRINK_PRESET").and_then(|s| ShrinkPreset::parse(&s)).unwrap_or_default()
}

/// Should it be shrunk?
pub fn wants_shrink(size: u64, threshold: Option<u64>, read: Readability) -> bool {
    matches!((threshold, read), (Some(t), Readability::Ok { text_chars }) if size > t && text_chars < SCAN_TEXT_CHARS)
}

fn sibling(work: &Path, sub: &str, name: &std::ffi::OsStr) -> std::io::Result<PathBuf> {
    let dir = work.join(sub);
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(name))
}

async fn blocking_readability(p: &Path) -> Readability {
    let p = p.to_path_buf();
    tokio::task::spawn_blocking(move || readability(&p)).await.unwrap_or(Readability::Broken)
}

/// Steps 1 and 2 on `working_copy` (file in `work_dir`). Replaces the working copy in place;
/// the caller decides from the result which file is stored.
pub async fn prepare(working_copy: &Path, work_dir: &Path) -> IngestOutcome {
    prepare_with(&pl_pdftools::runner::tool_location(), working_copy, work_dir).await
}

/// Like [`prepare`] with an explicit tool search result (testable). If the tool is missing,
/// nothing is attempted: a clear warning (where it was searched, what to do), ingestion continues.
pub async fn prepare_with(tool: &ToolLocation, working_copy: &Path, work_dir: &Path) -> IngestOutcome {
    let mut outcome = IngestOutcome::default();
    let Some(name) = working_copy.file_name().map(|n| n.to_os_string()) else { return outcome };
    let mut read = blocking_readability(working_copy).await;
    if read == Readability::Broken && tool.path().is_none() {
        outcome.skipped_no_tool = true;
        tracing::warn!(
            target: LOG,
            "PDF could not be read; repair skipped because the PDF tool is missing ({}). Continuing with the file as delivered.",
            tool.hint()
        );
    } else if read == Readability::Broken {
        match repair(tool, working_copy, work_dir, &name).await {
            Ok(Some(r)) => {
                outcome.repaired = true;
                read = r;
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(target: LOG, "PDF could not be read and the repair failed: {e}"),
        }
    }
    let size = std::fs::metadata(working_copy).map(|m| m.len()).unwrap_or(0);
    if wants_shrink(size, shrink_threshold(), read) && tool.path().is_none() {
        outcome.skipped_no_tool = true;
        tracing::warn!(target: LOG, "Shrinking the scan on ingest skipped because the PDF tool is missing ({}).", tool.hint());
    } else if wants_shrink(size, shrink_threshold(), read) {
        match shrink(tool, working_copy, work_dir, &name).await {
            Ok(Some(s)) => outcome.shrunk = Some(s),
            Ok(None) => {}
            Err(e) => tracing::warn!(target: LOG, "Shrinking the scan on ingest failed, keeping it as it is: {e}"),
        }
    }
    outcome
}

async fn repair(tool: &ToolLocation, working_copy: &Path, work_dir: &Path, name: &std::ffi::OsStr) -> Result<Option<Readability>, String> {
    let out = sibling(work_dir, "repaired", name).map_err(|e| e.to_string())?;
    let op = Op::Repair { input: working_copy.to_string_lossy().into(), output: out.to_string_lossy().into() };
    let report = match pl_pdftools::runner::run_located(tool, op, Limits::from_env()).await.map_err(|e| e.to_string())? {
        Reply::Repaired(r) => r,
        other => return Err(format!("unexpected reply {other:?}")),
    };
    let read = blocking_readability(&out).await;
    if read == Readability::Broken {
        return Err("the repaired copy is still not readable".into());
    }
    std::fs::rename(&out, working_copy).or_else(|_| std::fs::copy(&out, working_copy).map(|_| ())).map_err(|e| e.to_string())?;
    let what = if report.log.is_empty() { "no details".to_string() } else { report.log.join("; ") };
    tracing::info!(target: LOG, "PDF could not be read; repaired it with PdfCraft and continuing with the repaired copy ({what}). The original file is kept.");
    Ok(Some(read))
}

async fn shrink(tool: &ToolLocation, working_copy: &Path, work_dir: &Path, name: &std::ffi::OsStr) -> Result<Option<(u64, u64)>, String> {
    let out = sibling(work_dir, "shrunk", name).map_err(|e| e.to_string())?;
    let preset = shrink_preset();
    let op = Op::Shrink { input: working_copy.to_string_lossy().into(), output: out.to_string_lossy().into(), preset };
    let report = match pl_pdftools::runner::run_located(tool, op, Limits::from_env()).await.map_err(|e| e.to_string())? {
        Reply::Shrunk(r) => r,
        other => return Err(format!("unexpected reply {other:?}")),
    };
    if report.size_after >= report.size_before {
        tracing::info!(target: LOG, "Shrinking the scan did not make it smaller ({} bytes), keeping it as it is", report.size_before);
        let _ = std::fs::remove_file(&out);
        return Ok(None);
    }
    if blocking_readability(&out).await == Readability::Broken {
        return Err("the shrunk copy is not readable".into());
    }
    std::fs::rename(&out, working_copy).or_else(|_| std::fs::copy(&out, working_copy).map(|_| ())).map_err(|e| e.to_string())?;
    let (before, after, level) = (report.size_before, report.size_after, preset.name());
    tracing::info!(target: LOG, "Shrunk scan on ingest ({level}): {before} -> {after} bytes; storing the smaller file");
    Ok(Some((report.size_before, report.size_after)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrink_decision() {
        let mb = 1024 * 1024;
        assert!(wants_shrink(11 * mb, Some(10 * mb), Readability::Ok { text_chars: 0 }));
        assert!(!wants_shrink(9 * mb, Some(10 * mb), Readability::Ok { text_chars: 0 }));
        assert!(!wants_shrink(11 * mb, None, Readability::Ok { text_chars: 0 }));
        assert!(!wants_shrink(11 * mb, Some(10 * mb), Readability::Ok { text_chars: 500 }), "born-digital PDFs stay");
        assert!(!wants_shrink(11 * mb, Some(10 * mb), Readability::Encrypted));
        assert!(!wants_shrink(11 * mb, Some(10 * mb), Readability::Broken));
    }

    #[test]
    fn readability_of_files() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x.pdf");
        std::fs::write(&p, b"%PDF-1.4 nothing useful").unwrap();
        assert_eq!(readability(&p), Readability::Broken);
        std::fs::write(&p, b"").unwrap();
        assert_eq!(readability(&p), Readability::Broken);
    }

    /// Helper program missing: broken PDF is not touched, the result reports "skipped",
    /// ingestion continues with the file as delivered (also with a wrongly set
    /// `PAPERRUST_PDFTOOL_BIN`).
    #[tokio::test]
    async fn missing_tool_skips_repair_and_keeps_file() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("broken.pdf");
        std::fs::write(&p, b"%PDF-1.4 broken").unwrap();
        for loc in
            [ToolLocation::NotFound(vec![t.path().join("paperrust-pdftool")]), ToolLocation::ConfiguredMissing(t.path().join("nope.exe"))]
        {
            let o = prepare_with(&loc, &p, t.path()).await;
            assert_eq!(o, IngestOutcome { repaired: false, shrunk: None, skipped_no_tool: true });
            assert_eq!(std::fs::read(&p).unwrap(), b"%PDF-1.4 broken");
            assert!(!t.path().join("repaired").exists(), "nothing attempted");
        }
    }

    /// Readable PDF without a tool: nothing to do, no "skipped".
    #[tokio::test]
    async fn readable_pdf_without_tool_is_untouched() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("ok.pdf");
        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(
            lopdf::dictionary! { "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()] },
        );
        doc.objects.insert(
            pages_id,
            lopdf::Object::Dictionary(lopdf::dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }),
        );
        let cat = doc.add_object(lopdf::dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", cat);
        doc.save(&p).unwrap();
        let before = std::fs::read(&p).unwrap();
        let o = prepare_with(&ToolLocation::NotFound(vec![]), &p, t.path()).await;
        assert_eq!(o, IngestOutcome::default());
        assert_eq!(std::fs::read(&p).unwrap(), before);
    }
}
