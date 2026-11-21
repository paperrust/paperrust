// upstream: src/documents/search/_backend.py@v3.4.2 (TantivyBackend._build_tantivy_doc,
//           _is_indexable_date, _extract_autocomplete_words)
//           tantivy-py 0.26.2 src/document.rs (Document.add_* -> BTreeMap<String, Vec<Value>>,
//           pydatetime_to_tv, add_json via pythonize -> serde_json::Map) and src/index.rs
//           (IndexWriter.add_document -> TantivyDocument::convert_named_doc).
//
// The document is assembled exactly like tantivy-py does it: values are collected
// per field name in a BTreeMap (so fields end up in field-NAME order, values of one
// field in insertion order) and converted with `TantivyDocument::convert_named_doc`.
// JSON objects are built with sorted keys (tantivy-py's serde_json has no
// `preserve_order`) and every JSON value goes through `OwnedValue::from(serde_json::Value)`
// (incl. tantivy's RFC 3339 string -> date detection), like `Value::from(serde_json::Map)`.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use pl_search::query_simple::{extract_cjk_text, normalize_search_text};
use pl_search::tokenizer::autocomplete_tokens;
use tantivy::TantivyDocument;
use tantivy::schema::{NamedFieldDocument, OwnedValue, Schema};

use crate::error::{IndexerError, Result};
use crate::record::DocumentRecord;

/// tantivy stores dates as i64 nanoseconds since the epoch.
fn is_indexable_date(value: DateTime<Utc>) -> bool {
    // ((value - epoch) // 1s) * 1e9 within i64 – whole-second precision, floor division.
    let secs = value.timestamp() as i128; // timestamp() floors toward -inf
    let nanos = secs * 1_000_000_000;
    (i64::MIN as i128..=i64::MAX as i128).contains(&nanos)
}

fn to_tv_date(value: DateTime<Utc>) -> Result<tantivy::DateTime> {
    value
        .timestamp_nanos_opt()
        .map(tantivy::DateTime::from_timestamp_nanos)
        .ok_or_else(|| IndexerError::InvalidValue("datetime out of representable range".into()))
}

fn to_u64(name: &str, v: i64) -> Result<u64> {
    u64::try_from(v).map_err(|_| IndexerError::InvalidValue(format!("negative value for {name}")))
}

/// Field-name keyed value collector = tantivy-py `Document`.
#[derive(Default)]
struct PyDoc(BTreeMap<String, Vec<OwnedValue>>);

impl PyDoc {
    fn add(&mut self, field: &str, v: OwnedValue) {
        self.0.entry(field.to_string()).or_default().push(v);
    }
    fn add_text(&mut self, field: &str, text: &str) {
        self.add(field, OwnedValue::Str(text.to_string()));
    }
    fn add_unsigned(&mut self, field: &str, v: u64) {
        self.add(field, OwnedValue::U64(v));
    }
    fn add_date(&mut self, field: &str, v: DateTime<Utc>) -> Result<()> {
        let d = to_tv_date(v)?;
        self.add(field, OwnedValue::Date(d));
        Ok(())
    }
    /// `add_json(field, {k: v, ...})`; keys must be given sorted.
    fn add_json(&mut self, field: &str, sorted: Vec<(&str, serde_json::Value)>) {
        let obj: Vec<(String, OwnedValue)> = sorted.into_iter().map(|(k, v)| (k.to_string(), OwnedValue::from(v))).collect();
        self.add(field, OwnedValue::Object(obj));
    }
}

/// `_build_tantivy_doc(document, viewer_ids, viewer_group_ids)`.
pub fn build_tantivy_doc(schema: &Schema, rec: &DocumentRecord) -> Result<TantivyDocument> {
    let content = normalize_search_text(&rec.effective_content);
    let title = normalize_search_text(&rec.title);

    let mut doc = PyDoc::default();

    doc.add_unsigned("id", to_u64("id", rec.id)?);
    doc.add_text("checksum", &rec.checksum);
    doc.add_text("title", &title);
    doc.add_text("title_sort", &title);
    doc.add_text("simple_title", &title);
    doc.add_text("content", &content);
    doc.add_text("simple_content", &content);
    let cjk_title = extract_cjk_text(&title);
    if !cjk_title.is_empty() {
        doc.add_text("bigram_title", &cjk_title);
    }
    if !content.is_empty() {
        let cjk_content = extract_cjk_text(&content);
        if !cjk_content.is_empty() {
            doc.add_text("bigram_content", &cjk_content);
        }
    }

    if let Some(f) = rec.original_filename.as_deref().filter(|f| !f.is_empty()) {
        doc.add_text("original_filename", &normalize_search_text(f));
    }

    let correspondent = rec.correspondent.as_deref().map(normalize_search_text);
    if let Some(c) = &correspondent {
        doc.add_text("correspondent", c);
        doc.add_text("correspondent_sort", c);
        let cjk = extract_cjk_text(c);
        if !cjk.is_empty() {
            doc.add_text("bigram_correspondent", &cjk);
        }
    }

    let document_type = rec.document_type.as_deref().map(normalize_search_text);
    if let Some(t) = &document_type {
        doc.add_text("document_type", t);
        doc.add_text("type_sort", t);
        let cjk = extract_cjk_text(t);
        if !cjk.is_empty() {
            doc.add_text("bigram_document_type", &cjk);
        }
    }

    if let Some(sp) = &rec.storage_path {
        doc.add_text("storage_path", &normalize_search_text(sp));
    }

    let mut tag_names: Vec<String> = Vec::with_capacity(rec.tags.len());
    for tag in &rec.tags {
        let tag_name = normalize_search_text(tag);
        doc.add_text("tag", &tag_name);
        let cjk = extract_cjk_text(&tag_name);
        if !cjk.is_empty() {
            doc.add_text("bigram_tag", &cjk);
        }
        tag_names.push(tag_name);
    }

    let mut num_notes: u64 = 0;
    let mut note_texts: Vec<String> = Vec::new();
    for note in &rec.notes {
        num_notes += 1;
        let note_text = normalize_search_text(&note.note);
        let user = match &note.user {
            Some(u) => serde_json::Value::String(normalize_search_text(u)),
            None => serde_json::Value::Null,
        };
        doc.add_json("notes", vec![("note", serde_json::Value::String(note_text.clone())), ("user", user)]);
        note_texts.push(note_text);
    }
    if !note_texts.is_empty() {
        doc.add_text("notes_text", &note_texts.join(" "));
    }

    for cf in &rec.custom_fields {
        let Some(value) = &cf.search_value else {
            continue;
        };
        doc.add_json(
            "custom_fields",
            vec![
                ("name", serde_json::Value::String(normalize_search_text(&cf.name))),
                ("value", serde_json::Value::String(normalize_search_text(value))),
            ],
        );
    }

    for bc in &rec.barcodes {
        doc.add_json(
            "barcodes",
            vec![
                ("format", serde_json::Value::String(normalize_search_text(&bc.format))),
                ("value", serde_json::Value::String(normalize_search_text(&bc.value))),
            ],
        );
    }

    let created = created_datetime(rec.created);
    if is_indexable_date(created) {
        doc.add_date("created", created)?;
    } else {
        tracing::warn!(
            "Document {} has a created date ({}) outside the range the search index can store; \
             it will be indexed without a created date",
            rec.id,
            rec.created
        );
    }
    doc.add_date("modified", rec.modified)?;
    doc.add_date("added", rec.added)?;

    if let Some(asn) = rec.archive_serial_number {
        doc.add_unsigned("asn", to_u64("asn", asn)?);
    }
    if let Some(pc) = rec.page_count {
        doc.add_unsigned("page_count", to_u64("page_count", pc)?);
    }
    doc.add_unsigned("num_notes", num_notes);

    // `if document.owner_id:` (truthy: not None and not 0)
    if let Some(owner) = rec.owner_id.filter(|o| *o != 0) {
        doc.add_unsigned("owner_id", to_u64("owner_id", owner)?);
    }
    for v in &rec.viewer_ids {
        doc.add_unsigned("viewer_id", to_u64("viewer_id", *v)?);
    }
    for g in &rec.viewer_group_ids {
        doc.add_unsigned("viewer_group_id", to_u64("viewer_group_id", *g)?);
    }

    // Autocomplete words: [title, content, correspondent?, document_type?, *tags]
    let mut sources: Vec<&str> = vec![&title, &content];
    if let Some(c) = &correspondent {
        sources.push(c);
    }
    if let Some(t) = &document_type {
        sources.push(t);
    }
    sources.extend(tag_names.iter().map(String::as_str));
    let mut words: BTreeSet<String> = BTreeSet::new();
    for text in sources {
        if !text.is_empty() {
            words.extend(autocomplete_tokens(text));
        }
    }
    // Python sorted(set) on str = code point order = Rust String order.
    for w in &words {
        doc.add_text("autocomplete_word", w);
    }

    TantivyDocument::convert_named_doc(schema, NamedFieldDocument(doc.0)).map_err(|e| IndexerError::InvalidValue(e.to_string()))
}

/// `datetime(created.year, created.month, created.day, tzinfo=UTC)`.
pub fn created_datetime(d: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::record::{BarcodeRecord, CustomFieldRecord, NoteRecord};
    use tantivy::schema::document::Document as _;

    pub(crate) fn sample() -> DocumentRecord {
        DocumentRecord {
            id: 7,
            title: "Invoice Café 東京".into(),
            effective_content: "Quarterly tax statement 2024".into(),
            checksum: "abc123".into(),
            original_filename: Some("scan.pdf".into()),
            correspondent: Some("ACME".into()),
            document_type: None,
            storage_path: Some("Archive".into()),
            tags: vec!["Finance".into(), "Inbox".into()],
            notes: vec![
                NoteRecord { note: "call back".into(), user: Some("alice".into()) },
                NoteRecord { note: "2025-01-06T10:00:00Z".into(), user: None },
            ],
            custom_fields: vec![
                CustomFieldRecord { name: "Amount".into(), search_value: Some("EUR12.50".into()) },
                CustomFieldRecord { name: "Empty".into(), search_value: None },
            ],
            barcodes: vec![BarcodeRecord { value: "ASN0001".into(), format: "QRCode".into() }],
            created: NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            modified: Utc.with_ymd_and_hms(2025, 1, 6, 8, 0, 0).unwrap(),
            added: Utc.with_ymd_and_hms(2025, 1, 6, 8, 0, 1).unwrap(),
            archive_serial_number: Some(1007),
            page_count: None,
            owner_id: Some(3),
            viewer_ids: vec![4],
            viewer_group_ids: vec![1, 2],
        }
    }

    #[test]
    fn builds_like_tantivy_py() {
        let schema = pl_search::schema::build_schema();
        let doc = build_tantivy_doc(&schema, &sample()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&doc.to_json(&schema)).unwrap();
        assert_eq!(json["id"], serde_json::json!([7]));
        assert_eq!(json["tag"], serde_json::json!(["Finance", "Inbox"]));
        assert_eq!(json["asn"], serde_json::json!([1007]));
        assert!(json.get("page_count").is_none());
        assert!(json.get("document_type").is_none());
        assert_eq!(json["num_notes"], serde_json::json!([2]));
        assert_eq!(json["notes_text"], serde_json::json!(["call back 2025-01-06T10:00:00Z"]));
        assert_eq!(json["custom_fields"], serde_json::json!([{"name": "Amount", "value": "EUR12.50"}]));
        assert_eq!(json["barcodes"], serde_json::json!([{"format": "QRCode", "value": "ASN0001"}]));
        assert_eq!(json["created"], serde_json::json!(["2024-03-01T00:00:00Z"]));
        // field values are in field-NAME order (BTreeMap), like tantivy-py
        let names: Vec<String> = doc.iter_fields_and_values().map(|(f, _)| schema.get_field_name(f).to_string()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        // the second note's text looks like RFC 3339 -> stored as a date inside the JSON
        let notes: Vec<_> = doc.get_all(schema.get_field("notes").unwrap()).map(OwnedValue::from).collect();
        match &notes[1] {
            OwnedValue::Object(kv) => {
                assert_eq!(kv[0].0, "note");
                assert!(matches!(kv[0].1, OwnedValue::Date(_)));
                assert!(matches!(kv[1].1, OwnedValue::Null));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn owner_zero_and_bigrams() {
        let schema = pl_search::schema::build_schema();
        let mut r = sample();
        r.owner_id = Some(0);
        let doc = build_tantivy_doc(&schema, &r).unwrap();
        assert_eq!(doc.get_all(schema.get_field("owner_id").unwrap()).count(), 0);
        let bigram: Vec<_> = doc.get_all(schema.get_field("bigram_title").unwrap()).map(OwnedValue::from).collect();
        assert_eq!(bigram, vec![OwnedValue::Str("東京".into())]);
        assert_eq!(doc.get_all(schema.get_field("bigram_content").unwrap()).count(), 0);
    }

    #[test]
    fn date_range() {
        assert!(is_indexable_date(created_datetime(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())));
        assert!(!is_indexable_date(created_datetime(NaiveDate::from_ymd_opt(1600, 1, 1).unwrap())));
        assert!(!is_indexable_date(created_datetime(NaiveDate::from_ymd_opt(2300, 1, 1).unwrap())));
    }
}
