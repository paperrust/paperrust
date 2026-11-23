//! `Note.format` (paperless-ngx 3.5, #14626, migration documents.0030_note_format) in export and
//! import. The embedded descriptor is the v3.4.2 model, which has no `format` field.
//!
//! * Import: 3.5 exports carry `"format"` on every note. Written when `documents_note.format`
//!   exists in the target (always after `manage migrate`), otherwise skipped.
//! * Export: `format` is written only for notes that are **not** `plain`, right after `note` (3.5
//!   field order). Plain notes stay byte-identical to a v3.4.2 export, so the export imports into
//!   paperless-ngx 3.4.2 (whose importer rejects unknown fields) and 3.5 (missing field → model
//!   default `plain`). Markdown notes need paperless-ngx >= 3.5 – the exporter warns about them.

use serde_json::{Map, Value};
use sqlx::PgConnection;

use super::descriptor::{Default_, Descriptor, Field, Model};

pub const NOTE_MODEL: &str = "documents.note";
pub const FIELD: &str = "format";
pub const PLAIN: &str = "plain";

/// Does `documents_note.format` exist (paperless-ngx 3.5 column / PaperRust migration)?
pub async fn has_column(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = 'documents_note' AND column_name = 'format')",
    )
    .fetch_one(&mut *conn)
    .await
}

/// `Note.format` as a descriptor field (models.py@v3.5.1: CharField(max_length=16, default="plain")).
pub fn field(d: &Descriptor) -> Option<Field> {
    let mut f = d.models.get(NOTE_MODEL)?.field("note")?.clone();
    f.name = FIELD.into();
    f.attname = FIELD.into();
    f.column = FIELD.into();
    f.internal_type = "CharField".into();
    f.db_type = "varchar(16)".into();
    f.null = false;
    f.default = Default_ { kind: "value".into(), name: None, value: Some(Value::String(PLAIN.into())) };
    Some(f)
}

/// Note model with `format` after `note` (for [`super::exporter`]'s `build_record`).
pub fn model(d: &Descriptor) -> Option<Model> {
    let mut m = d.models.get(NOTE_MODEL)?.clone();
    let at = m.fields.iter().position(|f| f.name == "note")? + 1;
    m.fields.insert(at, field(d)?);
    Some(m)
}

/// Export query of the notes with the column selected after `note` (column order = field order).
/// Django orders by position (`ORDER BY 6` = `created`); the inserted column shifts it to 7.
pub fn sql(sql: &str) -> String {
    sql.replacen(
        "\"documents_note\".\"note\" AS \"note\", ",
        "\"documents_note\".\"note\" AS \"note\", \"documents_note\".\"format\" AS \"format\", ",
        1,
    )
    .replacen(" ORDER BY 6 ASC", " ORDER BY 7 ASC", 1)
}

/// Drops `format` from a record when it is `plain` (see module docs). Returns true if the note
/// keeps a non-plain format.
pub fn strip_plain(rec: &mut Map<String, Value>) -> bool {
    let Some(Value::Object(f)) = rec.get_mut("fields") else { return false };
    match f.get(FIELD) {
        Some(Value::String(s)) if s != PLAIN => true,
        Some(_) => {
            f.shift_remove(FIELD);
            false
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn export_model_and_sql() {
        let d = super::super::descriptor::get();
        let m = model(d).unwrap();
        let names: Vec<&str> = m.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["id", "deleted_at", "restored_at", "transaction_id", "note", "format", "created", "document", "user"]);
        let key = d.keys.iter().find(|k| k.key == "notes").unwrap();
        let q = sql(&key.sql);
        assert!(q.contains(
            "\"documents_note\".\"note\" AS \"note\", \"documents_note\".\"format\" AS \"format\", \"documents_note\".\"created\""
        ));
        // ORDER BY 6 (created) must follow the shifted column
        assert!(key.sql.ends_with("ORDER BY 6 ASC"));
        assert!(q.ends_with("ORDER BY 7 ASC"));
        assert!(d.split["notes"].sql.ends_with("ORDER BY 6 ASC"));
        let split = sql(&d.split["notes"].sql);
        assert!(split.contains("\"documents_note\".\"format\" AS \"format\""));
        assert!(split.ends_with("ORDER BY 7 ASC"));
    }

    #[test]
    fn plain_format_is_omitted() {
        let mut r = json!({"model": "documents.note", "pk": 1, "fields": {"note": "a", "format": "plain", "created": "x"}});
        assert!(!strip_plain(r.as_object_mut().unwrap()));
        assert_eq!(r["fields"], json!({"note": "a", "created": "x"}));
        let mut r = json!({"model": "documents.note", "pk": 1, "fields": {"note": "a", "format": "markdown", "created": "x"}});
        assert!(strip_plain(r.as_object_mut().unwrap()));
        let keys: Vec<&String> = r["fields"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["note", "format", "created"]);
    }
}
