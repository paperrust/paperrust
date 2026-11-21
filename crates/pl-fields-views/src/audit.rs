//! Audit log entries like django-auditlog 3.4.1 (`auditlog_logentry`).
//!
//! upstream: src/documents/models.py@v3.4.2 (`auditlog.register(CustomField)`,
//! `auditlog.register(CustomFieldInstance)`), auditlog/{receivers,diff,models,middleware,cid}.py.
//! `AUDITLOG_STORE_JSON_CHANGES = False` → all values as `str()`.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value};

use crate::fields::py_float;
use crate::shim::{ApiResult, Ctx, audit_log_enabled};

pub const ACTION_CREATE: i16 = 0;
pub const ACTION_UPDATE: i16 = 1;
pub const ACTION_DELETE: i16 = 2;

/// Python `str(None)`
pub const NONE: &str = "None";

/// Python `str(datetime)` of a naive UTC time (`make_naive(value, utc)`).
pub fn py_dt(dt: &DateTime<Utc>) -> String {
    let n = dt.naive_utc();
    let base = n.format("%Y-%m-%d %H:%M:%S").to_string();
    let us = dt.timestamp_subsec_micros();
    if us != 0 { format!("{base}.{us:06}") } else { base }
}

pub fn py_date(d: &NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Python `json.dumps(value, sort_keys=True)` (default separators, ensure_ascii=True).
pub fn py_json_dumps(v: &Value) -> String {
    let mut out = String::new();
    dump(v, &mut out);
    out
}

fn dump(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string())
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string())
            } else {
                let f = n.as_f64().unwrap_or(0.0);
                if f.is_nan() {
                    out.push_str("NaN")
                } else if f.is_infinite() {
                    out.push_str(if f > 0.0 { "Infinity" } else { "-Infinity" })
                } else {
                    out.push_str(&py_float(f))
                }
            }
        }
        Value::String(s) => dump_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            // Python sorts by code points; a UTF-8 byte comparison is equivalent
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump_str(k, out);
                out.push_str(": ");
                if let Some(x) = o.get(*k) {
                    dump(x, out);
                }
            }
            out.push('}');
        }
    }
}

fn dump_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `AuditlogMiddleware._get_remote_addr`
pub fn remote_addr(ctx: &Ctx) -> Option<String> {
    let xff = ctx.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).filter(|v| !v.is_empty());
    let Some(xff) = xff else {
        return ctx.remote_ip.clone();
    };
    let mut addr = xff.split(',').next().unwrap_or("").to_string();
    if addr.contains('.') && addr.contains(':') {
        addr = addr.split(':').next().unwrap_or("").to_string();
    } else if addr.contains('[') {
        addr = addr.get(1..).unwrap_or("").split(']').next().unwrap_or("").to_string();
    }
    Some(addr)
}

fn remote_port(ctx: &Ctx) -> Option<i32> {
    ctx.headers.get("x-forwarded-port").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<i32>().ok())
}

/// A change list `{field: [old, new]}` (insertion order; jsonb normalises anyway).
pub type Changes = Vec<(String, String, String)>;

/// Writes a `LogEntry`. Empty `changes` → no entry (like `model_instance_diff` → None).
pub async fn log(
    db: impl sqlx::PgExecutor<'_>,
    ctx: &Ctx,
    model: &str,
    object_id: i64,
    object_repr: &str,
    action: i16,
    changes: &Changes,
) -> ApiResult<()> {
    if !audit_log_enabled() || changes.is_empty() {
        return Ok(());
    }
    let mut m = Map::new();
    for (k, a, b) in changes {
        m.insert(k.clone(), Value::Array(vec![Value::String(a.clone()), Value::String(b.clone())]));
    }
    let (actor_id, actor_email) = if ctx.user.via_session { (Some(ctx.user.id), Some(ctx.user.email.clone())) } else { (None, None) };
    let cid = ctx.headers.get("x-correlation-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    sqlx::query(
        "INSERT INTO auditlog_logentry \
         (content_type_id, object_pk, object_id, object_repr, serialized_data, action, \
          changes_text, changes, actor_id, cid, remote_addr, remote_port, timestamp, \
          additional_data, actor_email) \
         VALUES ((SELECT id FROM django_content_type WHERE app_label = 'documents' AND model = $1), \
                 $2, $3, $4, NULL, $5, '', $6, $7, $8, $9::inet, $10, $11, NULL, $12)",
    )
    .bind(model)
    .bind(object_id.to_string())
    .bind(object_id)
    .bind(object_repr)
    .bind(action)
    .bind(Value::Object(m))
    .bind(actor_id)
    .bind(cid)
    .bind(remote_addr(ctx))
    .bind(remote_port(ctx))
    .bind(Utc::now())
    .bind(actor_email)
    .execute(db)
    .await?;
    Ok(())
}

/// State of a CustomField for the diff.
#[derive(Clone, Debug)]
pub struct CfState {
    pub id: i64,
    pub created: DateTime<Utc>,
    pub name: String,
    pub data_type: String,
    pub extra_data: Value,
}

impl CfState {
    /// `CustomField.__str__`
    pub fn repr(&self) -> String {
        format!("{} : {}", self.name, self.data_type)
    }
    fn values(&self) -> Vec<(&'static str, String)> {
        vec![
            ("id", self.id.to_string()),
            ("created", py_dt(&self.created)),
            ("name", self.name.clone()),
            ("data_type", self.data_type.clone()),
            ("extra_data", py_json_dumps(&self.extra_data)),
        ]
    }
}

/// Reverse relation `fields` (CustomFieldInstance.field, related_name="fields"):
/// `str(instance.fields)` = manager name.
const CF_REVERSE_MANAGER: &str = "documents.CustomFieldInstance.None";

pub fn cf_create_changes(new: &CfState) -> Changes {
    let mut c: Changes = new
        .values()
        .into_iter()
        // old: `get_default_value()` → "None"
        .filter(|(_, v)| v != NONE)
        .map(|(k, v)| (k.to_string(), NONE.to_string(), v))
        .collect();
    c.push(("fields".into(), NONE.into(), CF_REVERSE_MANAGER.into()));
    c
}

pub fn cf_delete_changes(old: &CfState) -> Changes {
    let mut c: Changes = old.values().into_iter().filter(|(_, v)| v != NONE).map(|(k, v)| (k.to_string(), v, NONE.to_string())).collect();
    c.push(("fields".into(), CF_REVERSE_MANAGER.into(), NONE.into()));
    c
}

pub fn cf_update_changes(old: &CfState, new: &CfState) -> Changes {
    old.values().into_iter().zip(new.values()).filter(|((_, a), (_, b))| a != b).map(|((k, a), (_, b))| (k.to_string(), a, b)).collect()
}

/// Fields of a CustomFieldInstance for the delete diff (old = instance, new = "None").
/// `None` in a value = Python `None` (only relevant for DateTimeFields, see diff.get_field_value).
#[derive(Clone, Debug, Default)]
pub struct CfiState {
    pub id: i64,
    pub created: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub restored_at: Option<DateTime<Utc>>,
    pub transaction_id: Option<String>,
    pub document_id: i64,
    pub field_id: i64,
    /// (field name, `str()` of the value or `None`) for value_* except value_document_ids
    pub plain_values: Vec<(&'static str, Option<String>)>,
    pub value_document_ids: Value,
}

pub fn cfi_delete_changes(old: &CfiState) -> Changes {
    let mut c: Changes = Vec::new();
    c.push(("id".into(), old.id.to_string(), NONE.into()));
    // DateTimeField: None stays None (≠ "None") → always in the diff
    for (k, v) in [("created", &old.created), ("deleted_at", &old.deleted_at), ("restored_at", &old.restored_at)] {
        c.push((k.into(), v.as_ref().map(py_dt).unwrap_or_else(|| NONE.into()), NONE.into()));
    }
    if let Some(t) = &old.transaction_id {
        c.push(("transaction_id".into(), t.clone(), NONE.into()));
    }
    c.push(("document".into(), old.document_id.to_string(), NONE.into()));
    c.push(("field".into(), old.field_id.to_string(), NONE.into()));
    for (k, v) in &old.plain_values {
        if let Some(v) = v
            && v != NONE
        {
            c.push(((*k).into(), v.clone(), NONE.into()));
        }
    }
    c.push(("value_document_ids".into(), py_json_dumps(&old.value_document_ids), NONE.into()));
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dumps_like_python() {
        assert_eq!(
            py_json_dumps(&json!({"select_options": [{"label": "Ä", "id": "x"}], "a": 1.0})),
            r#"{"a": 1.0, "select_options": [{"id": "x", "label": "\u00c4"}]}"#
        );
        assert_eq!(py_json_dumps(&Value::Null), "null");
        assert_eq!(py_json_dumps(&json!("\u{1F600}")), "\"\\ud83d\\ude00\"");
    }

    #[test]
    fn cf_diffs() {
        let created = DateTime::parse_from_rfc3339("2024-01-02T03:04:05.000006Z").map(|d| d.with_timezone(&Utc)).unwrap_or_default();
        let s = CfState { id: 5, created, name: "N".into(), data_type: "string".into(), extra_data: Value::Null };
        let c = cf_create_changes(&s);
        assert!(c.contains(&("extra_data".into(), "None".into(), "null".into())));
        assert!(c.contains(&("created".into(), "None".into(), "2024-01-02 03:04:05.000006".into())));
        assert!(c.contains(&("fields".into(), "None".into(), CF_REVERSE_MANAGER.into())));
        let mut n = s.clone();
        n.name = "M".into();
        assert_eq!(cf_update_changes(&s, &n), vec![("name".into(), "N".into(), "M".into())]);
        assert!(cf_update_changes(&s, &s).is_empty());
        assert_eq!(s.repr(), "N : string");
    }
}
