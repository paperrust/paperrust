// TEST ONLY: builds an on-disk index in a temp dir the way the Python worker
// does (upstream: src/documents/search/_backend.py::TantivyBackend._build_tantivy_doc,
// WriteBatch, _schema._write_sentinels). Production code never writes the index.

use std::path::Path;

use tantivy::schema::OwnedValue;
use tantivy::{DateTime, Index, IndexWriter, TantivyDocument, Term};

use crate::backend::{SearchBackend, SearchSettings};
use crate::query_simple::{extract_cjk_text, normalize_search_text};
use crate::schema::{SCHEMA_VERSION, SETTINGS_FILE, build_schema};
use crate::tokenizer::{autocomplete_tokens, register_tokenizers};

#[derive(Debug, Clone, Default)]
pub struct TestDoc {
    pub id: u64,
    pub title: String,
    pub content: String,
    pub checksum: String,
    pub correspondent: Option<String>,
    pub document_type: Option<String>,
    pub tags: Vec<String>,
    /// (note, username)
    pub notes: Vec<(String, Option<String>)>,
    pub created: Option<(i32, u32, u32)>,
    pub asn: Option<u64>,
    pub page_count: Option<u64>,
    pub owner_id: Option<u64>,
    pub viewer_ids: Vec<u64>,
    pub viewer_group_ids: Vec<u64>,
}

impl TestDoc {
    pub fn new(id: u64, title: &str, content: &str) -> Self {
        Self {
            id,
            title: title.into(),
            content: content.into(),
            checksum: format!("CHK{id}"),
            created: Some((2024, 1, 1)),
            ..Default::default()
        }
    }
}

fn f(index: &Index, name: &str) -> tantivy::schema::Field {
    match index.schema().get_field(name) {
        Ok(f) => f,
        Err(e) => panic!("field {name}: {e}"),
    }
}

pub fn build_doc(index: &Index, d: &TestDoc) -> TantivyDocument {
    let mut doc = TantivyDocument::default();
    let content = normalize_search_text(&d.content);
    let title = normalize_search_text(&d.title);
    doc.add_u64(f(index, "id"), d.id);
    doc.add_text(f(index, "checksum"), &d.checksum);
    doc.add_text(f(index, "title"), &title);
    doc.add_text(f(index, "title_sort"), &title);
    doc.add_text(f(index, "simple_title"), &title);
    doc.add_text(f(index, "content"), &content);
    doc.add_text(f(index, "simple_content"), &content);
    let cjk = extract_cjk_text(&title);
    if !cjk.is_empty() {
        doc.add_text(f(index, "bigram_title"), &cjk);
    }
    let cjk = extract_cjk_text(&content);
    if !content.is_empty() && !cjk.is_empty() {
        doc.add_text(f(index, "bigram_content"), &cjk);
    }
    let mut sources = vec![title.clone(), content.clone()];
    if let Some(c) = &d.correspondent {
        doc.add_text(f(index, "correspondent"), c);
        doc.add_text(f(index, "correspondent_sort"), c);
        sources.push(c.clone());
    }
    if let Some(t) = &d.document_type {
        doc.add_text(f(index, "document_type"), t);
        doc.add_text(f(index, "type_sort"), t);
        sources.push(t.clone());
    }
    for t in &d.tags {
        doc.add_text(f(index, "tag"), t);
        sources.push(t.clone());
    }
    let mut note_texts = Vec::new();
    for (note, user) in &d.notes {
        let obj: Vec<(String, OwnedValue)> = vec![
            ("note".into(), OwnedValue::Str(note.clone())),
            (
                "user".into(),
                match user {
                    Some(u) => OwnedValue::Str(u.clone()),
                    None => OwnedValue::Null,
                },
            ),
        ];
        doc.add_field_value(f(index, "notes"), &OwnedValue::Object(obj));
        note_texts.push(note.clone());
    }
    if !note_texts.is_empty() {
        doc.add_text(f(index, "notes_text"), note_texts.join(" "));
    }
    if let Some((y, m, day)) = d.created {
        let dt = chrono::NaiveDate::from_ymd_opt(y, m, day)
            .and_then(|nd| nd.and_hms_opt(0, 0, 0))
            .map(|ndt| ndt.and_utc().timestamp())
            .unwrap_or(0);
        doc.add_date(f(index, "created"), DateTime::from_timestamp_secs(dt));
    }
    doc.add_date(f(index, "modified"), DateTime::from_timestamp_secs(1_700_000_000));
    doc.add_date(f(index, "added"), DateTime::from_timestamp_secs(1_700_000_000));
    if let Some(a) = d.asn {
        doc.add_u64(f(index, "asn"), a);
    }
    if let Some(p) = d.page_count {
        doc.add_u64(f(index, "page_count"), p);
    }
    doc.add_u64(f(index, "num_notes"), d.notes.len() as u64);
    if let Some(o) = d.owner_id {
        doc.add_u64(f(index, "owner_id"), o);
    }
    for v in &d.viewer_ids {
        doc.add_u64(f(index, "viewer_id"), *v);
    }
    for g in &d.viewer_group_ids {
        doc.add_u64(f(index, "viewer_group_id"), *g);
    }
    let mut words: Vec<String> = sources.iter().flat_map(|s| autocomplete_tokens(s)).collect();
    words.sort();
    words.dedup();
    for w in words {
        doc.add_text(f(index, "autocomplete_word"), w);
    }
    doc
}

/// Create (or open) the index in `dir`, upsert `docs`, commit, stamp settings.
pub fn write_docs(dir: &Path, language: Option<&str>, docs: &[TestDoc]) {
    std::fs::create_dir_all(dir).unwrap();
    let directory = tantivy::directory::MmapDirectory::open(dir).unwrap();
    let index = Index::open_or_create(directory, build_schema()).unwrap();
    register_tokenizers(&index, language);
    let mut writer: IndexWriter = index.writer(15_000_000).unwrap();
    for d in docs {
        writer.delete_term(Term::from_field_u64(f(&index, "id"), d.id));
        writer.add_document(build_doc(&index, d)).unwrap();
    }
    writer.commit().unwrap();
    writer.wait_merging_threads().unwrap();
    std::fs::write(
        dir.join(SETTINGS_FILE),
        serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "language": language,
            "schema_fingerprint": "test",
        })
        .to_string(),
    )
    .unwrap();
}

pub fn settings(dir: &Path, language: Option<&str>, fuzzy: Option<f64>) -> SearchSettings {
    SearchSettings {
        index_dir: dir.to_path_buf(),
        language: language.map(str::to_string),
        fuzzy_threshold: fuzzy,
        time_zone: chrono_tz::UTC,
    }
}

pub fn backend_with(docs: &[TestDoc]) -> (tempfile::TempDir, SearchBackend) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("index");
    write_docs(&dir, Some("en"), docs);
    let b = SearchBackend::new(settings(&dir, Some("en"), None));
    (tmp, b)
}
