// upstream: src/documents/tests/search/test_backend.py (TestSearch, TestSearchIds,
// TestAutocomplete, TestMoreLikeThis, TestHighlightHits) – non-query-mode parts,
// against a temp index written like the Python worker writes it.

use crate::backend::{SearchBackend, SearchMode, SearchUser, search_language_setting};
use crate::testutil::{TestDoc, backend_with, settings, write_docs};

fn text(b: &SearchBackend, q: &str) -> Vec<i64> {
    b.search_ids(q, None, None, false, SearchMode::Text, None, None)
        .unwrap()
}

#[test]
fn empty_and_missing_index_returns_no_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let b = SearchBackend::new(settings(&tmp.path().join("nope"), Some("en"), None));
    assert_eq!(text(&b, "missing"), Vec::<i64>::new());
    assert_eq!(b.autocomplete("mis", 10, None).unwrap(), Vec::<String>::new());
    let (_t, b) = backend_with(&[]);
    assert_eq!(text(&b, "missing"), Vec::<i64>::new());
}

#[test]
fn text_mode_title_and_content_only() {
    let mut d = TestDoc::new(9, "Invoice document", "monthly statement");
    d.document_type = Some("invoice".into());
    let (_t, b) = backend_with(&[d]);
    assert!(text(&b, "document_type:invoice").is_empty());
    assert_eq!(text(&b, "monthly"), vec![9]);
    let title = |q: &str| {
        b.search_ids(q, None, None, false, SearchMode::Title, None, None)
            .unwrap()
    };
    assert!(title("monthly").is_empty());
    assert_eq!(title("invoice"), vec![9]);
}

#[test]
fn text_mode_substrings_and_order() {
    let (_t, b) = backend_with(&[
        TestDoc::new(11, "Account access", "password reset instructions"),
        TestDoc::new(19, "complete-medical-history", "Samsung Odyssey curved monitor"),
        TestDoc::new(20, "Complete record", "Patient history"),
        TestDoc::new(13, "Adobe Acrobat PDF Files", "Lorem ipsum dolor sit amet"),
    ]);
    assert_eq!(text(&b, "pass"), vec![11]);
    assert_eq!(text(&b, "sswo"), vec![11]);
    assert_eq!(text(&b, "sswo re"), vec![11]);
    assert_eq!(text(&b, "re sswo"), vec![11]);
    for q in ["Samsung curved", "curved Samsung"] {
        assert_eq!(text(&b, q), vec![19], "{q}");
    }
    let mut ids = text(&b, "complete history");
    ids.sort();
    assert_eq!(ids, vec![19, 20]);
    assert!(text(&b, "raptor").is_empty());
}

#[test]
fn text_mode_numeric_anchor() {
    let (_t, b) = backend_with(&[
        TestDoc::new(15, "Z-Berichte 6", "monthly report"),
        TestDoc::new(16, "Z-Berichte 60", "monthly report"),
        TestDoc::new(17, "Z-Berichte 16", "monthly report"),
    ]);
    for q in ["Z-Berichte 6", "6 Z-Berichte"] {
        let ids = text(&b, q);
        assert!(ids.contains(&15) && ids.contains(&16), "{q}");
        assert!(!ids.contains(&17), "{q}");
    }
}

#[test]
fn text_mode_folds_special_letters_and_cjk() {
    let (_t, b) = backend_with(&[
        TestDoc::new(1, "Straße", "details"),
        TestDoc::new(2, "東京都の会議", "議事録 です"),
    ]);
    assert_eq!(text(&b, "strasse"), vec![1]);
    assert_eq!(text(&b, "東京"), vec![2]);
}

#[test]
fn sort_fields() {
    let mut a = TestDoc::new(1, "alpha report", "x");
    a.asn = Some(30);
    let mut c = TestDoc::new(2, "beta report", "x");
    c.asn = Some(10);
    let mut e = TestDoc::new(3, "gamma report", "x");
    e.asn = Some(20);
    let (_t, b) = backend_with(&[a, c, e]);
    let asc = b
        .search_ids("report", None, Some("archive_serial_number"), false, SearchMode::Text, None, None)
        .unwrap();
    assert_eq!(asc, vec![2, 3, 1]);
    let desc = b
        .search_ids("report", None, Some("archive_serial_number"), true, SearchMode::Text, None, None)
        .unwrap();
    assert_eq!(desc, vec![1, 3, 2]);
}

#[test]
fn permission_filter() {
    let public = TestDoc::new(1, "shared report", "x");
    let mut own = TestDoc::new(2, "shared report", "x");
    own.owner_id = Some(5);
    let mut other = TestDoc::new(3, "shared report", "x");
    other.owner_id = Some(6);
    let mut viewer = TestDoc::new(4, "shared report", "x");
    viewer.owner_id = Some(6);
    viewer.viewer_ids = vec![5];
    let mut group = TestDoc::new(5, "shared report", "x");
    group.owner_id = Some(6);
    group.viewer_group_ids = vec![77];
    let (_t, b) = backend_with(&[public, own, other, viewer, group]);
    let u = SearchUser { id: 5, group_ids: vec![] };
    let mut ids = b
        .search_ids("report", Some(&u), None, false, SearchMode::Text, None, None)
        .unwrap();
    ids.sort();
    assert_eq!(ids, vec![1, 2, 4]);
    let u = SearchUser { id: 5, group_ids: vec![77] };
    let mut ids = b
        .search_ids("report", Some(&u), None, false, SearchMode::Text, None, None)
        .unwrap();
    ids.sort();
    assert_eq!(ids, vec![1, 2, 4, 5]);

    // autocomplete must not leak words of invisible docs
    let mut secret = TestDoc::new(9, "secretword", "x");
    secret.owner_id = Some(6);
    let (_t2, b2) = backend_with(&[secret]);
    let stranger = SearchUser { id: 5, group_ids: vec![] };
    assert!(b2.autocomplete("secr", 10, Some(&stranger)).unwrap().is_empty());
    assert_eq!(b2.autocomplete("secr", 10, None).unwrap(), vec!["secretword"]);
}

#[test]
fn limit_zero_and_one() {
    let (_t, b) = backend_with(&[
        TestDoc::new(1, "invoice", "invoice invoice invoice"),
        TestDoc::new(2, "other", "invoice"),
    ]);
    let ids = b
        .search_ids("invoice", None, None, false, SearchMode::Text, None, Some(1))
        .unwrap();
    assert_eq!(ids.len(), 1);
    assert!(b
        .search_ids("invoice", None, None, false, SearchMode::Text, None, Some(0))
        .unwrap()
        .is_empty());
}

#[test]
fn autocomplete_frequency_and_folding() {
    let (_t, b) = backend_with(&[
        TestDoc::new(41, "payment invoice", "details"),
        TestDoc::new(42, "payment receipt", "details"),
        TestDoc::new(43, "payment confirmation", "details"),
        TestDoc::new(44, "payslip march", "details"),
        TestDoc::new(45, "Straße Microsoft", "details"),
    ]);
    assert_eq!(b.autocomplete("pay", 10, None).unwrap(), vec!["payment", "payslip"]);
    assert_eq!(b.autocomplete("pay", 1, None).unwrap(), vec!["payment"]);
    assert!(b.autocomplete("stras", 10, None).unwrap().contains(&"strasse".to_string()));
    assert!(b.autocomplete("MICRO", 10, None).unwrap().contains(&"microsoft".to_string()));
    assert!(b.autocomplete("", 10, None).unwrap().is_empty());
}

#[test]
fn more_like_this_excludes_original() {
    let (_t, b) = backend_with(&[
        TestDoc::new(150, "Important document", "financial information report"),
        TestDoc::new(151, "Another document", "financial information report"),
        TestDoc::new(152, "Unrelated", "zebra giraffe"),
    ]);
    let ids = b.more_like_this_ids(150, None, None).unwrap();
    assert!(!ids.contains(&150));
    assert!(ids.contains(&151));
    assert!(b.more_like_this_ids(999, None, None).unwrap().is_empty());
}

/// tantivy 0.26: `doc_freq` counts deleted documents too, but MoreLikeThis computes the idf
/// with the live ones. After updates (delete + re-add) a term occurring everywhere is in
/// more documents than there are live ones → previously a panic, caught as an empty list.
#[test]
fn more_like_this_survives_deleted_docs() {
    let docs: Vec<TestDoc> = (1..=10)
        .map(|i| {
            let extra = if i <= 2 { "walrus pelican" } else { "other words" };
            TestDoc::new(200 + i, "common", &format!("common {extra}"))
        })
        .collect();
    let (t, b) = backend_with(&docs);
    // second commit: update five documents → five deleted old versions in the index
    write_docs(&t.path().join("index"), Some("en"), &docs[5..]);
    b.reload().unwrap();
    let ids = b.more_like_this_ids(201, None, None).unwrap();
    assert!(ids.contains(&202), "{ids:?}");
    let w = b.more_like_this_window(201, None, Some(5)).unwrap();
    assert!(w.total >= 1 && w.ids.contains(&202), "{} {:?}", w.total, w.ids);
}

#[test]
fn highlights_text_mode() {
    let (_t, b) = backend_with(&[TestDoc::new(
        89,
        "Highlight Test",
        "The quick brown fox jumps over the lazy dog",
    )]);
    let hits = b.highlight_hits("quick", &[89], SearchMode::Text, 1).unwrap();
    assert_eq!(hits.len(), 1);
    let h = hits[0].content_highlight.as_deref().unwrap();
    assert!(h.contains("<b>quick</b>"), "{h}");
    assert_eq!(hits[0].rank, 1);
    assert!(b.highlight_hits("quick", &[], SearchMode::Text, 1).unwrap().is_empty());
    assert!(b.highlight_hits("zzz", &[89], SearchMode::Text, 1).unwrap().is_empty());
}

#[test]
fn reader_sees_later_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("index");
    write_docs(&dir, Some("en"), &[TestDoc::new(1, "first", "x")]);
    let b = SearchBackend::new(settings(&dir, Some("en"), None));
    assert_eq!(text(&b, "first"), vec![1]);
    write_docs(&dir, Some("en"), &[TestDoc::new(2, "second", "x")]);
    b.reload().unwrap();
    assert_eq!(text(&b, "second"), vec![2]);
    // upsert: id 1 replaced
    write_docs(&dir, Some("en"), &[TestDoc::new(1, "renamed", "x")]);
    b.reload().unwrap();
    assert!(text(&b, "first").is_empty());
    assert_eq!(text(&b, "renamed"), vec![1]);
}

#[test]
fn language_mismatch_means_empty_index() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("index");
    write_docs(&dir, Some("de"), &[TestDoc::new(1, "first", "x")]);
    let b = SearchBackend::new(settings(&dir, Some("en"), None));
    assert!(text(&b, "first").is_empty());
}

#[test]
fn search_language_from_ocr() {
    assert_eq!(search_language_setting(None, "deu+eng").as_deref(), Some("de"));
    assert_eq!(search_language_setting(None, "chi_sim"), None);
    assert_eq!(search_language_setting(Some("fr"), "eng").as_deref(), Some("fr"));
}

#[test]
fn query_mode_search_and_note_highlights() {
    let mut d = TestDoc::new(91, "Doc with matching note", "unrelated content");
    d.notes = vec![("urgent payment required".into(), Some("hl_noteuser".into()))];
    let (_t, b) = backend_with(&[d, TestDoc::new(90, "Highlight Test", "The quick brown fox")]);
    let q = |s: &str| b.search_ids(s, None, None, false, SearchMode::Query, None, None).unwrap();
    assert_eq!(q("quick"), vec![90]);
    assert_eq!(q("notes.note:urgent"), vec![91]);
    let hits = b.highlight_hits("notes.note:urgent", &[91], SearchMode::Query, 1).unwrap();
    assert!(hits[0].notes_highlight.as_deref().unwrap().contains("<b>"));
    let hits = b.highlight_hits("quick", &[90], SearchMode::Query, 1).unwrap();
    assert!(hits[0].content_highlight.as_deref().unwrap().contains("<b>quick</b>"));
    let err = b
        .search_ids("asn:abc", None, None, false, SearchMode::Query, None, None)
        .unwrap_err();
    assert_eq!(err.messages().unwrap(), ["Invalid numeric value 'abc' for field 'asn'.".to_string()]);
}
