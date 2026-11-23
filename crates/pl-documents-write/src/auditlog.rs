// upstream: django-auditlog 3.4.1 (reference/venv): receivers.log_create/log_update/log_delete,
//           diff.get_field_value/model_instance_diff/get_fields_in_model, models.LogEntryManager.
//           log_create/log_m2m_changes, context.set_actor/_set_extra_data, middleware.AuditlogMiddleware,
//           cid.get_cid; src/documents/models.py@v3.4.2 (auditlog.register(Document, m2m_fields={"tags"},
//           exclude_fields=["content_length", "modified"]), Note, CustomFieldInstance, __str__)
//
//! django-auditlog for Document, Note and CustomFieldInstance.
//!
//! Value representation (`get_field_value`, not JSON mode): `Option<String>` with `None` = Python
//! `None` (only DateTimeField/ForeignKey yield a raw `None`), otherwise `smart_str(value)`.
//! If an instance is missing (create/delete, or `old` not found via the default manager),
//! every value is the default `smart_str(None) == "None"`.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use pl_consumer::db::DocRow;

pub const CREATE: i16 = 0;
pub const UPDATE: i16 = 1;
pub const DELETE: i16 = 2;

/// What the current auditlog context attaches to every `LogEntry` (`_set_extra_data`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogCtx {
    pub enabled: bool,
    pub actor_id: Option<i64>,
    pub actor_email: Option<String>,
    pub remote_addr: Option<String>,
    pub remote_port: Option<i32>,
    /// `get_cid()` (header `X-Correlation-ID`), applies to the whole request.
    pub cid: Option<String>,
}

impl LogCtx {
    /// `AuditlogMiddleware`: user only if Django knows it **before** DRF (session login);
    /// token/basic is only authenticated by DRF → `actor = None`.
    pub fn middleware(
        enabled: bool,
        session_user: Option<(&i64, Option<&str>)>,
        remote_addr: Option<String>,
        remote_port: Option<i32>,
        cid: Option<String>,
    ) -> Self {
        LogCtx {
            enabled,
            actor_id: session_user.map(|(id, _)| *id),
            actor_email: session_user.and_then(|(_, e)| e.map(str::to_string)),
            remote_addr,
            remote_port,
            cid,
        }
    }

    /// `with set_actor(user)`: actor + E-Mail, `remote_addr`/`remote_port` = None.
    pub fn set_actor(&self, user_id: i64, email: &str) -> Self {
        LogCtx {
            enabled: self.enabled,
            actor_id: Some(user_id),
            actor_email: Some(email.to_string()),
            remote_addr: None,
            remote_port: None,
            cid: self.cid.clone(),
        }
    }

    /// After a `set_actor` block: the ContextVar still points to the finished block, whose
    /// receiver is disconnected; the middleware receiver recognizes the foreign `signal_duid` →
    /// nothing is set any more (only `cid`).
    pub fn after_set_actor(&self) -> Self {
        LogCtx { enabled: self.enabled, cid: self.cid.clone(), ..Default::default() }
    }
}

pub type FieldVals = Vec<(&'static str, Option<String>)>;

/// `str(datetime)` naive in UTC (microseconds only if ≠ 0).
pub fn py_dt(t: &DateTime<Utc>) -> String {
    let n = t.naive_utc();
    if t.timestamp_subsec_micros() == 0 { n.format("%Y-%m-%d %H:%M:%S").to_string() } else { n.format("%Y-%m-%d %H:%M:%S%.6f").to_string() }
}

fn dt(v: &Option<DateTime<Utc>>) -> Option<String> {
    v.as_ref().map(py_dt)
}
fn fk(v: Option<i64>) -> Option<String> {
    v.map(|i| i.to_string())
}
fn s(v: impl Into<String>) -> Option<String> {
    Some(v.into())
}
fn so(v: &Option<String>) -> Option<String> {
    Some(v.clone().unwrap_or_else(|| "None".into()))
}
fn io(v: Option<i64>) -> Option<String> {
    Some(v.map_or_else(|| "None".into(), |i| i.to_string()))
}
fn uuid_s(v: &Option<uuid::Uuid>) -> Option<String> {
    Some(v.map_or_else(|| "None".into(), |u| u.to_string()))
}

/// Concrete fields of a document (without `content_length`, `modified`).
pub fn document_fields(d: &DocRow) -> FieldVals {
    vec![
        ("id", s(d.id.to_string())),
        ("deleted_at", dt(&d.deleted_at)),
        ("restored_at", dt(&d.restored_at)),
        ("transaction_id", uuid_s(&d.transaction_id)),
        ("title", s(d.title.clone())),
        ("content", s(d.content.clone())),
        ("mime_type", s(d.mime_type.clone())),
        ("checksum", s(d.checksum.clone())),
        ("archive_checksum", so(&d.archive_checksum)),
        ("page_count", io(d.page_count)),
        ("created", s(d.created.format("%Y-%m-%d").to_string())),
        ("added", dt(&Some(d.added))),
        ("filename", so(&d.filename)),
        ("archive_filename", so(&d.archive_filename)),
        ("original_filename", so(&d.original_filename)),
        ("archive_serial_number", io(d.archive_serial_number)),
        ("correspondent", fk(d.correspondent_id)),
        ("document_type", fk(d.document_type_id)),
        ("storage_path", fk(d.storage_path_id)),
        ("owner", fk(d.owner_id)),
        ("root_document", fk(d.root_document_id)),
        ("version_label", so(&d.version_label)),
        ("version_index", io(d.version_index)),
    ]
}

/// Reverse relations of the document (`get_fields_in_model`): `str(RelatedManager)`.
pub const DOCUMENT_REVERSE: &[(&str, &str)] = &[
    ("versions", "documents.Document.None"),
    ("notes", "documents.Note.None"),
    ("barcodes", "documents.DocumentBarcode.None"),
    ("share_links", "documents.ShareLink.None"),
    ("custom_fields", "documents.CustomFieldInstance.None"),
    ("workflow_runs", "documents.WorkflowRun.None"),
];

/// A `documents_note` record.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteRow {
    pub id: i64,
    pub note: String,
    /// `Note.format` (paperless-ngx 3.5, `plain`/`markdown`)
    pub format: String,
    pub created: DateTime<Utc>,
    pub document_id: Option<i64>,
    pub user_id: Option<i64>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub restored_at: Option<DateTime<Utc>>,
    pub transaction_id: Option<uuid::Uuid>,
}

pub fn note_fields(n: &NoteRow) -> FieldVals {
    vec![
        ("id", s(n.id.to_string())),
        ("deleted_at", dt(&n.deleted_at)),
        ("restored_at", dt(&n.restored_at)),
        ("transaction_id", uuid_s(&n.transaction_id)),
        ("note", s(n.note.clone())),
        ("format", s(n.format.clone())),
        ("created", dt(&Some(n.created))),
        ("document", fk(n.document_id)),
        ("user", fk(n.user_id)),
    ]
}

/// A `documents_customfieldinstance` record (all value columns as Python `smart_str`).
#[derive(Clone, Debug, PartialEq)]
pub struct CfiRow {
    pub id: i64,
    pub created: DateTime<Utc>,
    pub document_id: i64,
    pub field_id: i64,
    pub value_text: Option<String>,
    pub value_bool: Option<bool>,
    pub value_url: Option<String>,
    pub value_date: Option<NaiveDate>,
    pub value_int: Option<i64>,
    pub value_float: Option<f64>,
    pub value_monetary: Option<String>,
    /// Generated column (`str(Decimal)`), as stored in the DB.
    pub value_monetary_amount: Option<String>,
    pub value_document_ids: Option<Value>,
    pub value_select: Option<String>,
    pub value_long_text: Option<String>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub restored_at: Option<DateTime<Utc>>,
    pub transaction_id: Option<uuid::Uuid>,
}

/// Python `json.dumps(value, sort_keys=True)` for JSON fields.
pub fn py_json_dumps(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => {
            if *b {
                "true".into()
            } else {
                "false".into()
            }
        }
        Value::Number(n) => {
            if n.is_f64() {
                pl_core::json::python_float_repr(n.as_f64().unwrap_or(0.0))
            } else {
                n.to_string()
            }
        }
        Value::String(s) => py_json_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_json_dumps).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys.iter().map(|k| format!("{}: {}", py_json_str(k), py_json_dumps(&o[k.as_str()]))).collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// `json.dumps(str)` with `ensure_ascii=True`.
fn py_json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn py_bool(b: Option<bool>) -> Option<String> {
    Some(match b {
        None => "None".into(),
        Some(true) => "True".into(),
        Some(false) => "False".into(),
    })
}

pub fn cfi_fields(c: &CfiRow) -> FieldVals {
    vec![
        ("id", s(c.id.to_string())),
        ("deleted_at", dt(&c.deleted_at)),
        ("restored_at", dt(&c.restored_at)),
        ("transaction_id", uuid_s(&c.transaction_id)),
        ("created", dt(&Some(c.created))),
        ("document", fk(Some(c.document_id))),
        ("field", fk(Some(c.field_id))),
        ("value_text", so(&c.value_text)),
        ("value_bool", py_bool(c.value_bool)),
        ("value_url", so(&c.value_url)),
        ("value_date", Some(c.value_date.map_or_else(|| "None".into(), |d| d.format("%Y-%m-%d").to_string()))),
        ("value_int", io(c.value_int)),
        ("value_float", Some(c.value_float.map_or_else(|| "None".into(), pl_core::json::python_float_repr))),
        ("value_monetary", so(&c.value_monetary)),
        ("value_monetary_amount", so(&c.value_monetary_amount)),
        ("value_document_ids", Some(py_json_dumps(c.value_document_ids.as_ref().unwrap_or(&Value::Null)))),
        ("value_select", so(&c.value_select)),
        ("value_long_text", so(&c.value_long_text)),
    ]
}

/// `model_instance_diff(old, new, fields_to_check)`.
///
/// `old`/`new` = `None` → missing instance (values = default `"None"`); then the
/// reverse relations (`reverse`) count too. `only` = `update_fields` (field names).
pub fn diff(
    old: Option<&FieldVals>,
    new: Option<&FieldVals>,
    reverse: &[(&str, &str)],
    only: Option<&[&str]>,
) -> Option<Map<String, Value>> {
    let template = old.or(new)?;
    let with_reverse = old.is_none() || new.is_none();
    let expand = |v: Option<&FieldVals>| -> Vec<(&str, Option<String>)> {
        let mut out: Vec<(&str, Option<String>)> = match v {
            Some(v) => v.iter().map(|(k, x)| (*k, x.clone())).collect(),
            None => template.iter().map(|(k, _)| (*k, Some("None".to_string()))).collect(),
        };
        if with_reverse {
            for (k, r) in reverse {
                out.push((k, Some(if v.is_some() { (*r).to_string() } else { "None".to_string() })));
            }
        }
        out
    };
    let a = expand(old);
    let b = expand(new);
    let mut m = Map::new();
    for ((k, o), (_, n)) in a.iter().zip(b.iter()) {
        if let Some(f) = only
            && !f.contains(k)
        {
            continue;
        }
        if o != n {
            let st = |x: &Option<String>| Value::String(x.clone().unwrap_or_else(|| "None".into()));
            m.insert((*k).to_string(), Value::Array(vec![st(o), st(n)]));
        }
    }
    if m.is_empty() { None } else { Some(m) }
}

/// `str(document)`: `f"{created.isoformat()}[ {correspondent}][ {title}]"`.
pub fn document_repr(created: NaiveDate, correspondent: Option<&str>, title: &str) -> String {
    let mut s = created.format("%Y-%m-%d").to_string();
    if let Some(c) = correspondent {
        s.push(' ');
        s.push_str(c);
    }
    if !title.is_empty() {
        s.push(' ');
        s.push_str(title);
    }
    s
}

async fn content_type(conn: &mut PgConnection, model: &str) -> Result<i32, sqlx::Error> {
    let (id,): (i32,) = sqlx::query_as("SELECT id FROM django_content_type WHERE app_label = 'documents' AND model = $1")
        .bind(model)
        .fetch_one(&mut *conn)
        .await?;
    Ok(id)
}

/// `LogEntry.objects.create(...)` with the context data.
#[allow(clippy::too_many_arguments)]
pub async fn write(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    model: &str,
    object_id: i64,
    object_repr: &str,
    action: i16,
    changes: Map<String, Value>,
    additional_data: Option<Value>,
) -> Result<(), sqlx::Error> {
    if !ctx.enabled {
        return Ok(());
    }
    let ct = content_type(conn, model).await?;
    sqlx::query(
        "INSERT INTO auditlog_logentry (content_type_id, object_pk, object_id, object_repr, serialized_data, action, \
         changes_text, changes, actor_id, cid, remote_addr, remote_port, timestamp, additional_data, actor_email) \
         VALUES ($1, $2, $3, $4, NULL, $5, '', $6, $7, $8, $9::inet, $10, clock_timestamp(), $11, $12)",
    )
    .bind(ct)
    .bind(object_id.to_string())
    .bind(object_id)
    .bind(object_repr)
    .bind(action)
    .bind(Value::Object(changes))
    .bind(ctx.actor_id.map(|i| i as i32))
    .bind(&ctx.cid)
    .bind(&ctx.remote_addr)
    .bind(ctx.remote_port)
    .bind(additional_data)
    .bind(&ctx.actor_email)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `_create_log_entry`: writes only if `changes` is not empty.
#[allow(clippy::too_many_arguments)]
pub async fn write_diff(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    model: &str,
    object_id: i64,
    object_repr: &str,
    action: i16,
    changes: Option<Map<String, Value>>,
) -> Result<(), sqlx::Error> {
    match changes {
        Some(c) => write(conn, ctx, model, object_id, object_repr, action, c, None).await,
        None => Ok(()),
    }
}

/// `log_m2m_changes(changed_queryset, instance, operation, "tags")`; `names` by `Tag.Meta.ordering`.
pub async fn tags_m2m(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    doc_id: i64,
    repr: &str,
    operation: &str,
    names: &[String],
) -> Result<(), sqlx::Error> {
    if names.is_empty() {
        return Ok(());
    }
    let mut ch = Map::new();
    ch.insert("tags".into(), json!({"type": "m2m", "operation": operation, "objects": names}));
    write(conn, ctx, "document", doc_id, repr, UPDATE, ch, None).await
}

/// `CustomFieldInstance.__str__`: `f"{field.name} : {value_for_search}"`.
pub fn cfi_repr(field_name: &str, data_type: &str, extra: Option<&Value>, c: &CfiRow) -> String {
    format!("{field_name} : {}", value_for_search(data_type, extra, c).unwrap_or_else(|| "None".into()))
}

/// `CustomFieldInstance.value_for_search`.
pub fn value_for_search(data_type: &str, extra: Option<&Value>, c: &CfiRow) -> Option<String> {
    match data_type {
        "string" => c.value_text.clone(),
        "url" => c.value_url.clone(),
        "date" => c.value_date.map(|d| d.format("%Y-%m-%d").to_string()),
        "boolean" => c.value_bool.map(|b| if b { "True".into() } else { "False".into() }),
        "integer" => c.value_int.map(|i| i.to_string()),
        "float" => c.value_float.map(pl_core::json::python_float_repr),
        "monetary" => c.value_monetary.clone(),
        "documentlink" => c.value_document_ids.as_ref().filter(|v| !v.is_null()).map(py_str_json),
        "select" => {
            let v = c.value_select.as_ref()?;
            extra
                .and_then(|e| e.get("select_options"))
                .and_then(Value::as_array)
                .and_then(|opts| opts.iter().find(|o| o.get("id").and_then(Value::as_str) == Some(v.as_str())))
                .and_then(|o| o.get("label"))
                .map(|l| match l {
                    Value::String(s) => s.clone(),
                    other => py_str_json(other),
                })
        }
        "longtext" => c.value_long_text.clone(),
        _ => None,
    }
}

/// Python `str()` of a JSON value (lists/numbers; strings in lists with `'…'`).
pub fn py_str_json(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        Value::Number(n) => {
            if n.is_f64() {
                pl_core::json::python_float_repr(n.as_f64().unwrap_or(0.0))
            } else {
                n.to_string()
            }
        }
        Value::String(s) => s.clone(),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr_json).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            let parts: Vec<String> = o.iter().map(|(k, x)| format!("'{k}': {}", py_repr_json(x))).collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Python `repr()` of a JSON value.
pub fn py_repr_json(v: &Value) -> String {
    match v {
        Value::String(s) => {
            if s.contains('\'') && !s.contains('"') {
                format!("\"{s}\"")
            } else {
                format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
            }
        }
        other => py_str_json(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_dumps_like_python() {
        assert_eq!(py_json_dumps(&json!([31, 32])), "[31, 32]");
        assert_eq!(py_json_dumps(&Value::Null), "null");
        assert_eq!(py_json_dumps(&json!({"b": 1, "a": "ä"})), "{\"a\": \"\\u00e4\", \"b\": 1}");
    }

    #[test]
    fn diff_update_fields_and_missing_old() {
        let a: FieldVals = vec![("id", s("1")), ("deleted_at", None), ("restored_at", None), ("transaction_id", s("None"))];
        let mut b = a.clone();
        b[2].1 = s("2025-01-01 00:00:00");
        let only = ["deleted_at", "restored_at", "transaction_id"];
        let d = diff(None, Some(&b), &[], Some(&only)).unwrap();
        assert_eq!(d["deleted_at"], json!(["None", "None"]));
        assert_eq!(d["restored_at"], json!(["None", "2025-01-01 00:00:00"]));
        assert!(!d.contains_key("transaction_id"));
        assert!(diff(Some(&a), Some(&a), &[], None).is_none());
    }
}
