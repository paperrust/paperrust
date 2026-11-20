//! End-to-end of the three tools via the real child process (`cargo test -p pl-pdftools
//! --features engine`).

mod common;

use common::*;
use pl_pdftools::job::{Area, Op, Reply, ShrinkPreset};
use pl_pdftools::runner::run_with;

fn s(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

#[tokio::test]
async fn search_finds_terms_with_rects() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("text.pdf");
    text_pdf(&src);
    let r = run_with(&tool(), Op::Search { input: s(&src), terms: vec!["geheim".into()], max_hits: 50 }, limits()).await.unwrap();
    let Reply::Hits { hits, truncated } = r else { panic!("{r:?}") };
    assert!(!truncated);
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert_eq!((hits[0].page, hits[1].page), (0, 1));
    // match 1 lies around (72, 700) and covers only the word, not "4711"
    let r0 = hits[0].rects[0];
    assert!(r0[0] < 75.0 && r0[1] < 702.0 && r0[3] > 705.0, "{r0:?}");
    assert!(r0[2] < 72.0 + 24.0 * 6.0, "only the word GEHEIM: {r0:?}");
}

#[tokio::test]
async fn redact_term_really_removes_text_and_metadata() {
    let t = tempfile::tempdir().unwrap();
    let (src, out) = (t.path().join("text.pdf"), t.path().join("red.pdf"));
    text_pdf(&src);
    assert!(extract_text(&src).contains("GEHEIM"));
    let op = Op::Redact { input: s(&src), output: s(&out), areas: vec![], terms: vec!["Geheim".into()], remove_metadata: true };
    let r = run_with(&tool(), op, limits()).await.unwrap();
    let Reply::Redacted(rep) = r else { panic!("{r:?}") };
    assert_eq!(rep.term_hits, 2);
    assert!(rep.glyphs >= 12, "{rep:?}");
    // text extraction: term gone, rest still there
    let text = extract_text(&out);
    assert!(!text.to_lowercase().contains("geheim"), "{text}");
    assert!(text.contains("4711") && text.contains("Offener Teil bleibt") && text.contains("Kontonummer"), "{text}");
    // also not raw in any (unpacked) stream or in the metadata
    let raw = all_bytes_decoded(&out);
    assert!(!contains(&raw, b"GEHEIM"));
    assert!(!contains(&raw, b"Mustermann") && !contains(&raw, b"Musterfrau"), "metadata removed");
    // original unchanged
    assert!(extract_text(&src).contains("GEHEIM"));
}

#[tokio::test]
async fn redact_area_removes_only_covered_text() {
    let t = tempfile::tempdir().unwrap();
    let (src, out) = (t.path().join("text.pdf"), t.path().join("red.pdf"));
    text_pdf(&src);
    // rectangle over "GEHEIM 4711" on page 1 (baseline 700, 24 pt)
    let op = Op::Redact { input: s(&src), output: s(&out), areas: vec![Area { page: 0, rect: [60.0, 690.0, 260.0, 725.0] }], terms: vec![], remove_metadata: false };
    let r = run_with(&tool(), op, limits()).await.unwrap();
    assert!(matches!(r, Reply::Redacted(_)), "{r:?}");
    let text = extract_text(&out);
    assert!(!text.contains("4711"), "{text}");
    assert!(text.contains("Offener Teil bleibt"));
    // page 2 not affected
    assert!(text.contains("Kontonummer GEHEIM"), "{text}");
    // metadata stayed (not requested)
    assert!(contains(&all_bytes_decoded(&out), b"Mustermann"));
}

#[tokio::test]
async fn redact_rejects_bad_input() {
    let t = tempfile::tempdir().unwrap();
    let (src, out) = (t.path().join("text.pdf"), t.path().join("red.pdf"));
    text_pdf(&src);
    let op = Op::Redact { input: s(&src), output: s(&out), areas: vec![Area { page: 9, rect: [0.0, 0.0, 10.0, 10.0] }], terms: vec![], remove_metadata: false };
    let e = run_with(&tool(), op, limits()).await.unwrap_err();
    assert!(e.user_message().contains("page 10 does not exist"), "{e:?}");
    let op = Op::Redact { input: s(&src), output: s(&out), areas: vec![], terms: vec!["nicht-vorhanden".into()], remove_metadata: false };
    let e = run_with(&tool(), op, limits()).await.unwrap_err();
    assert!(e.user_message().contains("nothing to redact"), "{e:?}");
    assert!(!out.exists());
}

#[tokio::test]
async fn shrink_reports_sizes_and_keeps_pages() {
    let t = tempfile::tempdir().unwrap();
    let (src, out, out2) = (t.path().join("scan.pdf"), t.path().join("small.pdf"), t.path().join("lossless.pdf"));
    scan_pdf(&src);
    let r = run_with(&tool(), Op::Shrink { input: s(&src), output: s(&out), preset: ShrinkPreset::Standard }, limits()).await.unwrap();
    let Reply::Shrunk(rep) = r else { panic!("{r:?}") };
    assert_eq!(rep.size_before, std::fs::metadata(&src).unwrap().len());
    assert_eq!(rep.size_after, std::fs::metadata(&out).unwrap().len());
    assert!(rep.size_after * 2 < rep.size_before, "{rep:?}");
    assert_eq!(rep.images_resampled, 1, "{rep:?}");
    assert_eq!(lopdf::Document::load(&out).unwrap().get_pages().len(), 1);
    let r = run_with(&tool(), Op::Shrink { input: s(&src), output: s(&out2), preset: ShrinkPreset::Lossless }, limits()).await.unwrap();
    let Reply::Shrunk(rep) = r else { panic!("{r:?}") };
    assert_eq!(rep.images_resampled, 0);
}

#[tokio::test]
async fn repair_broken_xref() {
    let t = tempfile::tempdir().unwrap();
    let (src, broken, out) = (t.path().join("text.pdf"), t.path().join("broken.pdf"), t.path().join("fixed.pdf"));
    text_pdf(&src);
    let mut data = std::fs::read(&src).unwrap();
    // garbage before the header, cross references broken
    let pos = data.windows(9).rposition(|w| w == b"startxref").unwrap();
    data.truncate(pos);
    data.extend_from_slice(b"startxref\n999999\n%%EOF\n");
    let mut junk = b"\x00garbage\n".to_vec();
    junk.extend_from_slice(&data);
    std::fs::write(&broken, &junk).unwrap();
    let r = run_with(&tool(), Op::Repair { input: s(&broken), output: s(&out) }, limits()).await.unwrap();
    let Reply::Repaired(rep) = r else { panic!("{r:?}") };
    assert!(!rep.log.is_empty(), "repair log should say what was fixed");
    assert!(extract_text(&out).contains("GEHEIM 4711"));
}

/// Broken cross-reference stream in a file with object streams (PDF 1.5+): lopdf (the reader of the
/// parser) gives up, the lenient repair restores pages and text – exactly the case
/// "repair on ingestion". The file is also used for the E2E test (`broken_objstm_pdf`).
#[tokio::test]
async fn repair_fixes_files_lopdf_cannot_read() {
    let t = tempfile::tempdir().unwrap();
    let (src, broken, out) = (t.path().join("text.pdf"), t.path().join("broken.pdf"), t.path().join("fixed.pdf"));
    text_pdf(&src);
    broken_objstm_pdf(&src, &broken).await;
    assert!(lopdf::Document::load(&broken).is_err(), "lopdf must not be able to read the damaged file");
    let r = run_with(&tool(), Op::Repair { input: s(&broken), output: s(&out) }, limits()).await.unwrap();
    let Reply::Repaired(rep) = r else { panic!("{r:?}") };
    assert!(!rep.log.is_empty(), "{rep:?}");
    assert_eq!(lopdf::Document::load(&out).unwrap().get_pages().len(), 2);
    assert!(extract_text(&out).contains("GEHEIM 4711"));
}

/// rewrite `src` with object streams (lossless shrink) and then make the cross-reference
/// stream unusable (`/Type /XRef` → `/XRex`, `startxref` into the void).
async fn broken_objstm_pdf(src: &std::path::Path, dst: &std::path::Path) {
    let packed = dst.with_extension("packed.pdf");
    let r = run_with(&tool(), Op::Shrink { input: s(src), output: s(&packed), preset: ShrinkPreset::Lossless }, limits()).await;
    assert!(r.is_ok(), "{r:?}");
    let p = std::fs::read(&packed).unwrap();
    assert!(contains(&p, b"/ObjStm"));
    let sx = p.windows(9).rposition(|w| w == b"startxref").unwrap();
    let mut v = p[..sx].to_vec();
    v.extend_from_slice(b"startxref\n12\n%%EOF\n");
    let i = v.windows(5).position(|w| w == b"/XRef").unwrap();
    v[i + 1..i + 5].copy_from_slice(b"XRex");
    std::fs::write(dst, v).unwrap();
}

/// Test log 2026-11-16 C-068: redacting a word in a JPEG scan redacts only the
/// area under the mark; the rest of the image (including the second word) is preserved.
#[tokio::test]
async fn redact_area_in_jpeg_scan_clears_only_that_area() {
    let t = tempfile::tempdir().unwrap();
    let (src, out) = (t.path().join("scan.pdf"), t.path().join("red.pdf"));
    jpeg_scan_pdf(&src);
    // mark over word B (x 100–300, y 300–340) and some white background to the right of it
    let op = Op::Redact { input: s(&src), output: s(&out), areas: vec![Area { page: 0, rect: [90.0, 290.0, 400.0, 350.0] }], terms: vec![], remove_metadata: false };
    let r = run_with(&tool(), op, limits()).await.unwrap();
    let Reply::Redacted(rep) = r else { panic!("{r:?}") };
    assert_eq!((rep.images_removed, rep.images_cleared), (0, 1), "{rep:?}");
    let (filter, img) = first_page_image(&out).expect("the scan image must still be there");
    assert_eq!(filter, "DCTDecode", "stored as JPEG again");
    assert_eq!((img.width(), img.height()), (SCAN_W, SCAN_H));
    // black under the mark (also the previously white part to the right of the word)
    assert!(mean_luma(&img, 310, 300, 390, 340) < 40.0, "area cleared");
    // word A (outside) still black, white background outside still white
    assert!(mean_luma(&img, 110, 605, 290, 635) < 40.0, "other word kept");
    assert!(mean_luma(&img, 400, 500, 580, 780) > 215.0, "background kept");
    assert!(mean_luma(&img, 110, 100, 290, 250) > 215.0, "background kept");
    // original unchanged
    let (_, orig) = first_page_image(&src).unwrap();
    assert!(mean_luma(&orig, 310, 300, 390, 340) > 215.0);
}

/// Test log 2026-11-16 C-066b: redacting with `remove_metadata` also works if the PDF
/// has no (further) metadata.
#[tokio::test]
async fn redact_with_remove_metadata_when_there_is_none() {
    let t = tempfile::tempdir().unwrap();
    let (src, out) = (t.path().join("text.pdf"), t.path().join("red.pdf"));
    text_pdf_without_metadata(&src);
    let op = Op::Redact { input: s(&src), output: s(&out), areas: vec![], terms: vec!["Geheim".into()], remove_metadata: true };
    let r = run_with(&tool(), op, limits()).await.unwrap();
    let Reply::Redacted(rep) = r else { panic!("{r:?}") };
    assert_eq!(rep.term_hits, 2, "{rep:?}");
    let text = extract_text(&out);
    assert!(!text.to_lowercase().contains("geheim") && text.contains("Offener Teil bleibt"), "{text}");
}
