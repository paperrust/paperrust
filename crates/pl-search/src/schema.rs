// upstream: src/documents/search/_schema.py@v3.4.2 (field_descriptors, build_schema,
//           needs_rebuild/_settings_mismatch), src/documents/search/_fields.py (PUBLIC_FIELDS)
//
// Rust only READS the index. `build_schema()` exists so that (a) tests can create
// an index with exactly the layout tantivy-py writes and (b) an opened index can be
// checked against the expected field list (see `schema_matches`).

use std::path::Path;

use tantivy::schema::{
    DateOptions, IndexRecordOption, JsonObjectOptions, NumericOptions, Schema, TextFieldIndexing,
    TextOptions,
};

use crate::tokenizer::{BIGRAM_ANALYZER, PAPERLESS_TEXT, SIMPLE_ANALYZER, SIMPLE_SEARCH_ANALYZER};

/// `SCHEMA_VERSION` stamped into `.index_settings.json`.
pub const SCHEMA_VERSION: i64 = 4;
/// `REBUILD_MARKER`.
pub const REBUILD_MARKER: &str = ".rebuilding";
pub const SETTINGS_FILE: &str = ".index_settings.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Json,
    U64,
    Date,
}

/// `FieldDescriptor(name, kind, stored, indexed, fast, tokenizer)`.
#[derive(Debug, Clone, Copy)]
pub struct FieldDescriptor {
    pub name: &'static str,
    pub kind: Kind,
    pub stored: bool,
    pub indexed: bool,
    pub fast: bool,
    pub tokenizer: Option<&'static str>,
}

const fn d(
    name: &'static str,
    kind: Kind,
    stored: bool,
    fast: bool,
    tokenizer: Option<&'static str>,
) -> FieldDescriptor {
    FieldDescriptor {
        name,
        kind,
        stored,
        indexed: true,
        fast,
        tokenizer,
    }
}

/// `field_descriptors()`: every field in declaration order (on-disk contract).
pub const FIELD_DESCRIPTORS: &[FieldDescriptor] = &[
    d("id", Kind::U64, true, true, None),
    // PUBLIC_FIELDS
    d("title", Kind::Text, true, false, Some(PAPERLESS_TEXT)),
    d("content", Kind::Text, true, false, Some(PAPERLESS_TEXT)),
    d("correspondent", Kind::Text, true, false, Some(PAPERLESS_TEXT)),
    d("document_type", Kind::Text, true, false, Some(PAPERLESS_TEXT)),
    d("storage_path", Kind::Text, true, false, Some(PAPERLESS_TEXT)),
    d("original_filename", Kind::Text, true, false, Some(PAPERLESS_TEXT)),
    d("tag", Kind::Text, true, false, Some(PAPERLESS_TEXT)),
    d("checksum", Kind::Text, true, false, Some("raw")),
    d("asn", Kind::U64, true, true, None),
    d("page_count", Kind::U64, true, true, None),
    d("num_notes", Kind::U64, true, true, None),
    d("created", Kind::Date, true, true, None),
    d("modified", Kind::Date, true, true, None),
    d("added", Kind::Date, true, true, None),
    d("notes", Kind::Json, true, false, Some(PAPERLESS_TEXT)),
    d("notes_text", Kind::Text, true, false, Some(PAPERLESS_TEXT)),
    d("custom_fields", Kind::Json, true, false, Some(PAPERLESS_TEXT)),
    d("barcodes", Kind::Json, true, false, Some(PAPERLESS_TEXT)),
    // Shadow sort fields - fast, not stored
    d("title_sort", Kind::Text, false, true, Some(SIMPLE_ANALYZER)),
    d("correspondent_sort", Kind::Text, false, true, Some(SIMPLE_ANALYZER)),
    d("type_sort", Kind::Text, false, true, Some(SIMPLE_ANALYZER)),
    // CJK support
    d("bigram_content", Kind::Text, false, false, Some(BIGRAM_ANALYZER)),
    d("bigram_title", Kind::Text, false, false, Some(BIGRAM_ANALYZER)),
    d("bigram_correspondent", Kind::Text, false, false, Some(BIGRAM_ANALYZER)),
    d("bigram_document_type", Kind::Text, false, false, Some(BIGRAM_ANALYZER)),
    d("bigram_tag", Kind::Text, false, false, Some(BIGRAM_ANALYZER)),
    // Simple substring search
    d("simple_title", Kind::Text, false, false, Some(SIMPLE_SEARCH_ANALYZER)),
    d("simple_content", Kind::Text, false, false, Some(SIMPLE_SEARCH_ANALYZER)),
    // Autocomplete
    d("autocomplete_word", Kind::Text, false, false, Some("raw")),
    // Permission filter columns
    d("owner_id", Kind::U64, false, true, None),
    d("viewer_id", Kind::U64, false, true, None),
    d("viewer_group_id", Kind::U64, false, true, None),
];

fn text_options(desc: &FieldDescriptor) -> TextOptions {
    // tantivy-py schemabuilder: index_option "position" =
    // WithFreqsAndPositions; fast -> set_fast(Some(tokenizer)) unless "raw".
    let tok = desc.tokenizer.unwrap_or("default");
    let mut opts = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(tok)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    if desc.stored {
        opts = opts.set_stored();
    }
    if desc.fast {
        opts = opts.set_fast(if tok == "raw" { None } else { Some(tok) });
    }
    opts
}

/// `build_schema()`.
pub fn build_schema() -> Schema {
    let mut sb = Schema::builder();
    for desc in FIELD_DESCRIPTORS {
        match desc.kind {
            Kind::Text => {
                sb.add_text_field(desc.name, text_options(desc));
            }
            Kind::Json => {
                let tok = desc.tokenizer.unwrap_or("default");
                let mut opts = JsonObjectOptions::default().set_indexing_options(
                    TextFieldIndexing::default()
                        .set_tokenizer(tok)
                        .set_index_option(IndexRecordOption::WithFreqsAndPositions),
                );
                if desc.stored {
                    opts = opts.set_stored();
                }
                sb.add_json_field(desc.name, opts);
            }
            Kind::U64 => {
                let mut opts = NumericOptions::default();
                if desc.stored {
                    opts = opts.set_stored();
                }
                if desc.indexed {
                    opts = opts.set_indexed();
                }
                if desc.fast {
                    opts = opts.set_fast();
                }
                sb.add_u64_field(desc.name, opts);
            }
            Kind::Date => {
                let mut opts = DateOptions::default();
                if desc.stored {
                    opts = opts.set_stored();
                }
                if desc.indexed {
                    opts = opts.set_indexed();
                }
                if desc.fast {
                    opts = opts.set_fast();
                }
                sb.add_date_field(desc.name, opts);
            }
        }
    }
    sb.build()
}

/// Ordered field-name/type check of an opened index against the expected
/// layout. Stand-in for upstream's `schema_fingerprint()` comparison (which
/// hashes the descriptors with blake2b): any added/removed/reordered/retyped
/// field makes this false.
pub fn schema_matches(schema: &Schema) -> bool {
    let fields: Vec<_> = schema.fields().collect();
    if fields.len() != FIELD_DESCRIPTORS.len() {
        return false;
    }
    fields.iter().zip(FIELD_DESCRIPTORS).all(|((_, entry), desc)| {
        if entry.name() != desc.name {
            return false;
        }
        use tantivy::schema::FieldType;
        let kind_ok = matches!(
            (entry.field_type(), desc.kind),
            (FieldType::Str(_), Kind::Text)
                | (FieldType::JsonObject(_), Kind::Json)
                | (FieldType::U64(_), Kind::U64)
                | (FieldType::Date(_), Kind::Date)
        );
        kind_ok && entry.is_fast() == desc.fast && entry.is_stored() == desc.stored
    })
}

/// Why an on-disk index is not usable as-is (upstream would rebuild it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexState {
    Ok,
    Missing,
    /// `.rebuilding` marker present: a rebuild is running or died.
    Rebuilding,
    /// `.index_settings.json` missing, unparsable or not matching.
    SettingsMismatch(String),
}

/// `needs_rebuild()` / `_settings_mismatch()` without the side effects.
pub fn check_index_dir(index_dir: &Path, language: Option<&str>) -> IndexState {
    if !index_dir.exists() {
        return IndexState::Missing;
    }
    if index_dir.join(REBUILD_MARKER).exists() {
        return IndexState::Rebuilding;
    }
    let settings_file = index_dir.join(SETTINGS_FILE);
    let Ok(raw) = std::fs::read_to_string(&settings_file) else {
        return IndexState::SettingsMismatch("settings file missing".into());
    };
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return IndexState::SettingsMismatch("settings file unparsable".into());
    };
    if data.get("schema_version").and_then(|v| v.as_i64()) != Some(SCHEMA_VERSION) {
        return IndexState::SettingsMismatch("schema version".into());
    }
    // settings.SEARCH_LANGUAGE may be None -> JSON null.
    let stored_lang = match data.get("language") {
        None => return IndexState::SettingsMismatch("language missing".into()),
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.as_str()),
        Some(_) => return IndexState::SettingsMismatch("language".into()),
    };
    if stored_lang != language {
        return IndexState::SettingsMismatch("language".into());
    }
    IndexState::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_self_consistent() {
        let s = build_schema();
        assert!(schema_matches(&s));
        assert_eq!(s.fields().count(), 33);
    }

    #[test]
    fn settings_check() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            check_index_dir(dir.path(), Some("en")),
            IndexState::SettingsMismatch(_)
        ));
        std::fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"schema_version": 4, "language": "en", "schema_fingerprint": "x"}"#,
        )
        .unwrap();
        assert_eq!(check_index_dir(dir.path(), Some("en")), IndexState::Ok);
        assert!(matches!(
            check_index_dir(dir.path(), None),
            IndexState::SettingsMismatch(_)
        ));
        std::fs::write(dir.path().join(REBUILD_MARKER), "").unwrap();
        assert_eq!(check_index_dir(dir.path(), Some("en")), IndexState::Rebuilding);
    }
}
