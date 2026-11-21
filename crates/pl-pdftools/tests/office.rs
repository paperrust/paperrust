//! Office → PDF with WordCraft in the child process (`Op::Office`), then the PDF tools on the
//! result (search, redact, shrink). Sample files: tests/fixtures/office/ (make_samples.py).

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use pl_pdftools::job::{Op, Reply};
use pl_pdftools::runner::{Limits, RunError, run_blocking_with, run_with};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/office").join(name)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

async fn convert(name: &str, format: &str, out: &Path) -> Result<pl_pdftools::ConvertReport, RunError> {
    match run_with(&tool(), Op::Office { input: s(&fixture(name)), output: s(out), format: format.into() }, limits()).await? {
        Reply::Converted(r) => Ok(r),
        other => panic!("unexpected reply {other:?}"),
    }
}

fn page_count(p: &Path) -> usize {
    lopdf::Document::load(p).map(|d| d.get_pages().len()).unwrap_or(0)
}

async fn search(pdf: &Path, term: &str) -> usize {
    match run_with(&tool(), Op::Search { input: s(pdf), terms: vec![term.into()], max_hits: 50 }, limits()).await {
        Ok(Reply::Hits { hits, .. }) => hits.len(),
        other => panic!("search failed: {other:?}"),
    }
}

#[tokio::test]
async fn docx_becomes_a_searchable_multi_page_pdf() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("sample.pdf");
    let r = convert("sample.docx", "docx", &out).await.unwrap();
    assert_eq!(r.format, "docx");
    assert!(r.size_after > 1000, "{r:?}");
    let data = std::fs::read(&out).unwrap();
    assert!(data.starts_with(b"%PDF"));
    // page break in the document → at least two pages
    assert!(page_count(&out) >= 2, "pages: {}", page_count(&out));
    // Text is real text (embedded font with Unicode mapping): PdfCraft finds it
    assert!(search(&out, "Geheimnummer").await >= 1);
    assert!(search(&out, "Installation").await >= 1, "table text");
}

#[tokio::test]
async fn odt_rtf_and_markdown_convert() {
    let t = tempfile::tempdir().unwrap();
    for (name, fmt, needle, min_pages) in
        [("sample.odt", "odt", "Laufzeit", 2), ("sample.rtf", "rtf", "Bank", 2), ("sample.md", "md", "Notiz", 1)]
    {
        let out = t.path().join(format!("{fmt}.pdf"));
        let r = convert(name, fmt, &out).await.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(r.format, fmt);
        assert!(page_count(&out) >= min_pages, "{name}: {} pages", page_count(&out));
        assert!(search(&out, needle).await >= 1, "{name}: '{needle}' not found");
    }
}

#[tokio::test]
async fn broken_docx_is_a_clean_error() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("broken.pdf");
    let e = convert("broken.docx", "docx", &out).await.unwrap_err();
    let RunError::Failed(msg) = &e else { panic!("{e:?}") };
    assert!(msg.contains("docx"), "{msg}");
    // no paths in the message
    assert!(!msg.contains(&s(t.path())), "{msg}");
    assert!(!out.exists() || std::fs::metadata(&out).unwrap().len() == 0);
}

#[tokio::test]
async fn unknown_format_and_missing_file_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("x.pdf");
    let e = run_with(&tool(), Op::Office { input: s(&fixture("sample.docx")), output: s(&out), format: "exe".into() }, limits())
        .await
        .unwrap_err();
    assert!(matches!(e, RunError::Failed(_)), "{e:?}");
    let e = run_with(&tool(), Op::Office { input: s(&t.path().join("nope.docx")), output: s(&out), format: "docx".into() }, limits())
        .await
        .unwrap_err();
    assert!(matches!(e, RunError::Failed(_)), "{e:?}");
}

/// After the conversion the PDF tools work: redacting really removes the term,
/// shrinking yields a valid PDF.
#[tokio::test]
async fn pdf_tools_work_on_the_converted_pdf() {
    let t = tempfile::tempdir().unwrap();
    let pdf = t.path().join("sample.pdf");
    convert("sample.docx", "docx", &pdf).await.unwrap();
    let red = t.path().join("redacted.pdf");
    let r = run_with(
        &tool(),
        Op::Redact { input: s(&pdf), output: s(&red), areas: vec![], terms: vec!["Geheimnummer".into()], remove_metadata: true },
        limits(),
    )
    .await
    .unwrap();
    let Reply::Redacted(rep) = r else { panic!("{r:?}") };
    assert!(rep.term_hits >= 1 && rep.glyphs > 0, "{rep:?}");
    assert_eq!(search(&red, "Geheimnummer").await, 0, "term must be gone after redaction");
    assert!(search(&red, "Installation").await >= 1, "other text stays");
    let small = t.path().join("small.pdf");
    let r = run_with(&tool(), Op::Shrink { input: s(&pdf), output: s(&small), preset: pl_pdftools::ShrinkPreset::Strong }, limits())
        .await
        .unwrap();
    assert!(matches!(r, Reply::Shrunk(_)), "{r:?}");
    assert!(page_count(&small) >= 2);
}

/// The synchronous launcher (used during ingestion) converts the same way and keeps the time limit.
#[test]
fn blocking_runner_converts_and_times_out() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("b.pdf");
    let r = run_blocking_with(&tool(), Op::Office { input: s(&fixture("sample.rtf")), output: s(&out), format: "rtf".into() }, limits())
        .unwrap();
    assert!(matches!(r, Reply::Converted(_)), "{r:?}");
    let start = std::time::Instant::now();
    let e = run_blocking_with(
        &tool(),
        Op::SelfTest { mode: pl_pdftools::job::SelfTest::Hang },
        Limits { timeout: Duration::from_secs(2), memory_mb: 256 },
    )
    .unwrap_err();
    assert_eq!(e, RunError::Timeout(2));
    assert!(start.elapsed() < Duration::from_secs(20));
    let e = run_blocking_with(
        &tool(),
        Op::SelfTest { mode: pl_pdftools::job::SelfTest::Abort },
        Limits { timeout: Duration::from_secs(30), memory_mb: 256 },
    )
    .unwrap_err();
    assert!(matches!(e, RunError::Crashed(_)), "{e:?}");
}

fn upstream_sample(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream/src/paperless/tests/samples").join(rel)
}

/// Gap E9: old Word (.doc) – real file from upstream (test_tika_liva.py::test_tika_parse_doc).
#[tokio::test]
async fn legacy_doc_becomes_a_searchable_pdf() {
    let src = upstream_sample("tika/sample.doc");
    if !src.exists() {
        eprintln!("upstream sample missing, skipped");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("doc.pdf");
    let r = run_with(&tool(), Op::Office { input: s(&src), output: s(&out), format: "doc".into() }, limits()).await.unwrap();
    assert!(matches!(r, Reply::Converted(_)), "{r:?}");
    assert!(page_count(&out) >= 1);
    assert!(search(&out, "older").await >= 1, "text of the .doc must be in the PDF");
}

/// Gap E10: spreadsheet (.xls, BIFF8) → PDF with one table per sheet, searchable.
#[tokio::test]
async fn spreadsheet_becomes_a_pdf_with_tables() {
    let rows = |r: &[&[&str]]| r.iter().map(|row| row.iter().map(|c| (*c).to_string()).collect()).collect::<Vec<Vec<String>>>();
    let xls = wordcraft_formats::sheet::xls_fixture(&[
        ("Umsatz", rows(&[&["Kunde", "Betrag"], &["Bäckerei Huber", "1234.5"]])),
        ("Kosten", rows(&[&["Miete", "900"]])),
    ])
    .unwrap();
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("a.xls");
    std::fs::write(&src, xls).unwrap();
    let out = t.path().join("xls.pdf");
    let r = run_with(&tool(), Op::Office { input: s(&src), output: s(&out), format: "xls".into() }, limits()).await.unwrap();
    assert!(matches!(r, Reply::Converted(_)), "{r:?}");
    assert_eq!(page_count(&out), 2, "one page per sheet");
    assert!(search(&out, "Huber").await >= 1);
    assert!(search(&out, "Miete").await >= 1);
}

/// Gap E10: slides HTML → A4 landscape, one page per slide.
#[tokio::test]
async fn slides_become_landscape_pages() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("s.html");
    std::fs::write(&src, "<p>1 / 2</p><h1>Quartal</h1><p>Umsatz</p><p style=\"page-break-before:always\">2 / 2</p><h1>Ausblick</h1>")
        .unwrap();
    let out = t.path().join("s.pdf");
    let r = run_with(&tool(), Op::Slides { input: s(&src), output: s(&out) }, limits()).await.unwrap();
    assert!(matches!(r, Reply::Converted(_)), "{r:?}");
    assert_eq!(page_count(&out), 2);
    assert!(search(&out, "Ausblick").await >= 1);
}

/// Gap E8: HEIC photo from upstream → PNG in the child process.
#[tokio::test]
async fn heic_becomes_png() {
    let src = upstream_sample("tesseract/simple.heic");
    if !src.exists() {
        eprintln!("upstream sample missing, skipped");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("h.png");
    let r = run_with(&tool(), Op::Heic { input: s(&src), output: s(&out), max_pixels: 0 }, limits()).await.unwrap();
    let Reply::Converted(rep) = r else { panic!("{r:?}") };
    assert_eq!(rep.format, "heic");
    let img = image::open(&out).unwrap();
    assert!(img.width() > 10 && img.height() > 10);
    // broken file: clean error without a path
    let bad = t.path().join("bad.heic");
    std::fs::write(&bad, b"ftypheic nonsense").unwrap();
    let e = run_with(&tool(), Op::Heic { input: s(&bad), output: s(&out), max_pixels: 0 }, limits()).await.unwrap_err();
    let RunError::Failed(msg) = &e else { panic!("{e:?}") };
    assert!(msg.contains("HEIC") && !msg.contains(&s(t.path())), "{msg}");
}
