// upstream: src/documents/pdf_ops.py@v3.4.2 (PageSpec, rotate_pdf, remove_pages, build_pdfs,
// validate_page_operations, needs_decrypt, decrypt_pdf, PdfMerger)
//
//! Pure PDF page operations for `documents/bulk_edit.py`, 1:1 after `pdf_ops.py`. Every
//! function that writes a PDF removes unused resources beforehand.

use std::path::{Path, PathBuf};

use lopdf::Document;
use serde::{Deserialize, Serialize};

use crate::pdf::{self, PageCopier};
use crate::{PdfOpsError, Result};

/// A page of an output PDF: 1-based source page, optionally rotated relatively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSpec {
    pub page: i64,
    /// relative degrees, 0 leaves the page unchanged
    #[serde(default)]
    pub rotate: i64,
}

impl PageSpec {
    pub fn new(page: i64) -> Self {
        PageSpec { page, rotate: 0 }
    }
}

/// An `edit_pdf` operation (`{"page": n, "rotate"?: degrees, "doc"?: output}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageOperation {
    pub page: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<i64>,
}

impl PageOperation {
    fn doc_idx(&self) -> i64 {
        self.doc.unwrap_or(0)
    }
}

fn require_positive(pages: impl IntoIterator<Item = i64>) -> Result<()> {
    for p in pages {
        if p < 1 {
            return Err(PdfOpsError::Value(format!("Page numbers start at 1, got {p}")));
        }
    }
    Ok(())
}

/// `rotate_pdf`: rotate each page relatively, on the opened document (info, XMP, bookmarks
/// stay). `src` stays unchanged.
pub fn rotate_pdf(src: &Path, dst: &Path, degrees: i64) -> Result<()> {
    let mut doc = pdf::load(src, None)?;
    for p in pdf::page_ids(&doc) {
        pdf::rotate_page(&mut doc, p, degrees, true)?;
    }
    pdf::remove_unreferenced_resources(&mut doc);
    pdf::save(&mut doc, dst)
}

/// `remove_pages`: remove 1-based pages on the opened document (info, XMP, bookmarks
/// stay). Duplicates are ignored, highest first.
pub fn remove_pages(src: &Path, dst: &Path, pages: &[i64]) -> Result<()> {
    let mut unique: Vec<i64> = pages.to_vec();
    unique.sort_unstable();
    unique.dedup();
    require_positive(unique.iter().copied())?;
    let mut doc = pdf::load(src, None)?;
    let ids = pdf::page_ids(&doc);
    for page_num in unique.iter().rev() {
        // pikepdf: del pdf.pages[page_num - 1] → IndexError if the number is too large
        let Some(id) = ids.get((*page_num - 1) as usize) else {
            return Err(PdfOpsError::Index("Accessing nonexistent PDF page number".into()));
        };
        pdf::remove_page_from_tree(&mut doc, *id)?;
    }
    pdf::remove_unreferenced_resources(&mut doc);
    pdf::save(&mut doc, dst)
}

/// `build_pdfs`: per output a new PDF from pages of `src` (opened once).
///
/// All page numbers are checked before an output is created; `make_dst(index)` is called only
/// immediately before saving the respective output (an error leaves no
/// target). Document-wide data (info, XMP, bookmarks) is not taken over.
pub fn build_pdfs(src: &Path, outputs: &[Vec<PageSpec>], mut make_dst: impl FnMut(usize) -> Result<PathBuf>) -> Result<Vec<PathBuf>> {
    for specs in outputs {
        require_positive(specs.iter().map(|s| s.page))?;
    }
    let source = pdf::load(src, None)?;
    let ids = pdf::page_ids(&source);
    let page_count = ids.len() as i64;
    for specs in outputs {
        for s in specs {
            if s.page > page_count {
                return Err(PdfOpsError::Index(format!("Page {} is out of range, the PDF has {page_count} pages", s.page)));
            }
        }
    }
    let mut written = Vec::with_capacity(outputs.len());
    for (idx, specs) in outputs.iter().enumerate() {
        let mut c = PageCopier::new();
        for s in specs {
            let id = c.append(0, &source, ids[(s.page - 1) as usize])?;
            if s.rotate != 0 {
                pdf::rotate_page(&mut c.doc, id, s.rotate, true)?;
            }
        }
        let mut dst = c.finish();
        pdf::remove_unreferenced_resources(&mut dst);
        let path = make_dst(idx)?;
        pdf::save(&mut dst, &path)?;
        written.push(path);
    }
    Ok(written)
}

/// `validate_page_operations`: checks `edit_pdf` operations and returns the number of outputs.
/// The boundary rule stays as upstream: a `doc` index must be smaller than the number of
/// operations.
pub fn validate_page_operations(operations: &[PageOperation], single_output: bool) -> Result<usize> {
    if operations.is_empty() {
        return Err(PdfOpsError::Value("Output document index is out of bounds".into()));
    }
    let max_idx = operations.iter().map(PageOperation::doc_idx).max().unwrap_or(0);
    if single_output && max_idx > 0 {
        return Err(PdfOpsError::Value("Multiple output documents specified".into()));
    }
    let n = operations.len() as i64;
    if operations.iter().any(|op| op.doc_idx() < 0 || op.doc_idx() >= n) {
        return Err(PdfOpsError::Value("Output document index is out of bounds".into()));
    }
    Ok((max_idx + 1) as usize)
}

/// `edit_pdf` operations into page lists per output (after `validate_page_operations`).
pub fn page_specs_for(operations: &[PageOperation], output_count: usize) -> Vec<Vec<PageSpec>> {
    let mut out: Vec<Vec<PageSpec>> = vec![Vec::new(); output_count];
    for op in operations {
        if let Some(v) = out.get_mut(op.doc_idx() as usize) {
            v.push(PageSpec { page: op.page, rotate: op.rotate.unwrap_or(0) });
        }
    }
    out
}

/// `needs_decrypt`: encrypted? A file that cannot be opened at all without a password
/// counts as encrypted. Other read errors are passed on.
pub fn needs_decrypt(src: &Path) -> Result<bool> {
    match pdf::load(src, None) {
        Ok(doc) => Ok(doc.was_encrypted()),
        Err(PdfOpsError::Password(_)) => Ok(true),
        Err(e) => Err(e),
    }
}

/// `decrypt_pdf`: write an unencrypted copy. `make_dst` only after the password was accepted.
pub fn decrypt_pdf(src: &Path, make_dst: impl FnOnce() -> Result<PathBuf>, password: &str) -> Result<PathBuf> {
    let mut doc = pdf::load(src, Some(password))?;
    pdf::remove_unreferenced_resources(&mut doc);
    let dst = make_dst()?;
    pdf::save(&mut doc, &dst)?;
    Ok(dst)
}

/// `PdfMerger`: collects the pages of several PDFs into a new PDF. `add` fails if
/// a source is not readable (skipping is decided by the caller).
pub struct PdfMerger {
    copier: PageCopier,
    version: String,
    sources: usize,
}

impl Default for PdfMerger {
    fn default() -> Self {
        Self::new()
    }
}

impl PdfMerger {
    pub fn new() -> Self {
        let copier = PageCopier::new();
        let version = copier.doc.version.clone();
        PdfMerger { copier, version, sources: 0 }
    }

    pub fn add(&mut self, path: &Path) -> Result<()> {
        let doc = pdf::load(path, None)?;
        self.add_document(&doc)
    }

    pub fn add_document(&mut self, doc: &Document) -> Result<()> {
        // Python: max(self._version, pdf.pdf_version) – string comparison like upstream.
        if doc.version > self.version {
            self.version = doc.version.clone();
        }
        let key = self.sources;
        self.sources += 1;
        for p in pdf::page_ids(doc) {
            self.copier.append(key, doc, p)?;
        }
        Ok(())
    }

    pub fn page_count(&self) -> usize {
        self.copier.page_count()
    }

    /// `save(dst)` with `min_version` = highest source version.
    pub fn save(self, dst: &Path) -> Result<()> {
        let mut doc = self.copier.finish();
        doc.version = self.version;
        pdf::remove_unreferenced_resources(&mut doc);
        pdf::save(&mut doc, dst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::testpdf;

    #[test]
    fn rotate_keeps_info_and_rotates_all() {
        let t = tempfile::tempdir().unwrap();
        let (s, d) = (t.path().join("s.pdf"), t.path().join("d.pdf"));
        testpdf::write(&s, &["A", "B"]);
        rotate_pdf(&s, &d, 90).unwrap();
        assert_eq!(testpdf::rotations(&d), [90, 90]);
        assert_eq!(testpdf::texts(&d), ["A", "B"]);
        let doc = Document::load(&d).unwrap();
        assert!(doc.trailer.get(b"Info").is_ok());
    }

    #[test]
    fn remove_pages_dedup_and_errors() {
        let t = tempfile::tempdir().unwrap();
        let (s, d) = (t.path().join("s.pdf"), t.path().join("d.pdf"));
        testpdf::write(&s, &["A", "B", "C", "D"]);
        remove_pages(&s, &d, &[3, 1, 3]).unwrap();
        assert_eq!(testpdf::texts(&d), ["B", "D"]);
        assert_eq!(remove_pages(&s, &d, &[0]).unwrap_err().to_string(), "Page numbers start at 1, got 0");
        assert_eq!(remove_pages(&s, &d, &[-1, 2]).unwrap_err().to_string(), "Page numbers start at 1, got -1");
    }

    #[test]
    fn build_pdfs_orders_rotates_and_checks_range() {
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("s.pdf");
        testpdf::write(&s, &["A", "B", "C"]);
        let outs = vec![vec![PageSpec { page: 3, rotate: 90 }, PageSpec::new(1)], vec![], vec![PageSpec::new(2), PageSpec::new(2)]];
        let dir = t.path().to_path_buf();
        let paths = build_pdfs(&s, &outs, |i| Ok(dir.join(format!("o{i}.pdf")))).unwrap();
        assert_eq!(paths.len(), 3);
        assert_eq!(testpdf::texts(&paths[0]), ["C", "A"]);
        assert_eq!(testpdf::rotations(&paths[0]), [90, 0]);
        assert_eq!(Document::load(&paths[1]).unwrap().get_pages().len(), 0);
        assert_eq!(testpdf::texts(&paths[2]), ["B", "B"]);
        let d0 = Document::load(&paths[0]).unwrap();
        assert!(d0.trailer.get(b"Info").is_err());
        let mut called = false;
        let e = build_pdfs(&s, &[vec![PageSpec::new(1)], vec![PageSpec::new(5)]], |_| {
            called = true;
            Ok(dir.join("x.pdf"))
        })
        .unwrap_err();
        assert!(!called);
        assert_eq!(e.to_string(), "Page 5 is out of range, the PDF has 3 pages");
        let e = build_pdfs(&dir.join("missing.pdf"), &[vec![PageSpec::new(0)]], |_| Ok(dir.join("x.pdf"))).unwrap_err();
        assert_eq!(e.kind(), "ValueError");
    }

    #[test]
    fn validate_ops() {
        let op = |page, doc| PageOperation { page, rotate: None, doc };
        assert_eq!(validate_page_operations(&[op(1, None), op(2, Some(1))], false).unwrap(), 2);
        assert_eq!(validate_page_operations(&[op(1, None), op(2, None), op(3, Some(2))], false).unwrap(), 3);
        assert_eq!(validate_page_operations(&[], false).unwrap_err().to_string(), "Output document index is out of bounds");
        assert_eq!(
            validate_page_operations(&[op(1, None), op(2, Some(1))], true).unwrap_err().to_string(),
            "Multiple output documents specified"
        );
        assert_eq!(validate_page_operations(&[op(1, Some(1))], false).unwrap_err().to_string(), "Output document index is out of bounds");
        assert_eq!(validate_page_operations(&[op(1, Some(-1))], false).unwrap_err().to_string(), "Output document index is out of bounds");
        let ops = [PageOperation { page: 2, rotate: Some(90), doc: None }, op(1, Some(1))];
        assert_eq!(page_specs_for(&ops, 2), vec![vec![PageSpec { page: 2, rotate: 90 }], vec![PageSpec::new(1)]]);
        let parsed: Vec<PageOperation> = serde_json::from_str(r#"[{"page":1},{"page":2,"rotate":180,"doc":1}]"#).unwrap();
        assert_eq!(parsed[1], PageOperation { page: 2, rotate: Some(180), doc: Some(1) });
    }

    #[test]
    fn merger_orders_and_versions() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, o) = (t.path().join("a.pdf"), t.path().join("b.pdf"), t.path().join("o.pdf"));
        testpdf::write(&a, &["A1", "A2"]);
        let mut d = testpdf::make(&["B1"]);
        d.version = "1.7".into();
        d.save(&b).unwrap();
        let mut m = PdfMerger::new();
        m.add(&b).unwrap();
        m.add(&a).unwrap();
        assert!(m.add(&t.path().join("missing.pdf")).is_err());
        m.save(&o).unwrap();
        assert_eq!(testpdf::texts(&o), ["B1", "A1", "A2"]);
        assert_eq!(Document::load(&o).unwrap().version, "1.7");
    }

    #[test]
    fn decrypt_plain_file() {
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("s.pdf");
        testpdf::write(&s, &["A"]);
        assert!(!needs_decrypt(&s).unwrap());
    }
}
