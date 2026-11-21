//! `TaskSerializerV10` / `TaskSerializerV9` / `TaskSummarySerializer`.
//!
//! upstream: src/documents/serialisers.py@v3.4.2, src/documents/models.py@v3.4.2
//! (`PaperlessTask.related_document_ids`).

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use pl_core::datetime::format_datetime_opt;
use serde_json::{Map, Value};
use sqlx::Row as _;
use sqlx::postgres::PgRow;

use crate::labels::{STATUS, TASK_TYPE, TRIGGER_SOURCE, display};
use crate::py;

/// One row of `documents_paperlesstask`.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub id: i64,
    pub task_id: String,
    pub task_type: String,
    pub trigger_source: String,
    pub status: String,
    pub date_created: DateTime<Utc>,
    pub date_started: Option<DateTime<Utc>>,
    pub date_done: Option<DateTime<Utc>>,
    pub duration_seconds: Option<f64>,
    pub wait_time_seconds: Option<f64>,
    pub input_data: Value,
    /// SQL NULL and JSON `null` both read as `None` in Django.
    pub result_data: Option<Value>,
    pub acknowledged: bool,
    pub owner_id: Option<i64>,
}

/// Columns selected for [`TaskRow::from_row`] (alias `t`).
pub const COLUMNS: &[&str] = &[
    "id",
    "task_id",
    "task_type",
    "trigger_source",
    "status",
    "date_created",
    "date_started",
    "date_done",
    "duration_seconds",
    "wait_time_seconds",
    "input_data",
    "result_data",
    "acknowledged",
    "owner_id",
];

impl TaskRow {
    pub fn from_row(r: &PgRow) -> Result<Self, sqlx::Error> {
        let result_data: Option<Value> = r.try_get("result_data")?;
        Ok(TaskRow {
            id: i64::from(r.try_get::<i32, _>("id")?),
            task_id: r.try_get("task_id")?,
            task_type: r.try_get("task_type")?,
            trigger_source: r.try_get("trigger_source")?,
            status: r.try_get("status")?,
            date_created: r.try_get("date_created")?,
            date_started: r.try_get("date_started")?,
            date_done: r.try_get("date_done")?,
            duration_seconds: r.try_get("duration_seconds")?,
            wait_time_seconds: r.try_get("wait_time_seconds")?,
            input_data: r.try_get("input_data")?,
            result_data: result_data.filter(|v| !v.is_null()),
            acknowledged: r.try_get("acknowledged")?,
            owner_id: r.try_get::<Option<i32>, _>("owner_id")?.map(i64::from),
        })
    }

    fn result_get(&self, key: &str) -> Option<&Value> {
        self.result_data.as_ref().and_then(|v| v.as_object()).and_then(|m| m.get(key))
    }

    /// `PaperlessTask.related_document_ids` (raw values, before `IntegerField` conversion).
    pub fn related_document_ids_raw(&self) -> Vec<Value> {
        if let Some(d) = self.result_get("document_id").filter(|v| truthy(v)) {
            return vec![d.clone()];
        }
        if let Some(d) = self.result_get("duplicate_of").filter(|v| truthy(v)) {
            return vec![d.clone()];
        }
        Vec::new()
    }

    /// `result_data.get("duplicate_of")` if not `None` (v9 `duplicate_documents`).
    pub fn duplicate_of(&self) -> Option<&Value> {
        self.result_get("duplicate_of").filter(|v| !v.is_null())
    }
}

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// `int(value)` as done by `IntegerField.to_representation` / Django `IntegerField.get_prep_value`.
pub fn py_int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() { Ok(f.trunc() as i64) } else { Err(format!("cannot convert float {f} to integer")) }
            } else {
                Err(format!("int too large: {n}"))
            }
        }
        Value::String(s) => {
            py::int(s).and_then(|i| i64::try_from(i).ok()).ok_or_else(|| format!("invalid literal for int() with base 10: {s:?}"))
        }
        other => Err(format!("int() argument must be a string or a number, not {other}")),
    }
}

/// Python `str(value)` of a JSON-loaded value (f-strings).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => pl_core::pyrepr::repr_value(other),
    }
}

fn float(v: Option<f64>) -> Value {
    v.and_then(serde_json::Number::from_f64).map_or(Value::Null, Value::Number)
}

/// `TaskSerializerV10(task).data`
pub fn v10(t: &TaskRow, lang: &str, tz: &Tz) -> Result<Value, String> {
    let related: Result<Vec<Value>, String> = t.related_document_ids_raw().iter().map(|v| py_int(v).map(Value::from)).collect();
    let mut m = Map::new();
    m.insert("id".into(), Value::from(t.id));
    m.insert("task_id".into(), Value::from(t.task_id.clone()));
    m.insert("task_type".into(), Value::from(t.task_type.clone()));
    m.insert("task_type_display".into(), Value::from(display(TASK_TYPE, &t.task_type, lang)));
    m.insert("trigger_source".into(), Value::from(t.trigger_source.clone()));
    m.insert("trigger_source_display".into(), Value::from(display(TRIGGER_SOURCE, &t.trigger_source, lang)));
    m.insert("status".into(), Value::from(t.status.clone()));
    m.insert("status_display".into(), Value::from(display(STATUS, &t.status, lang)));
    m.insert("date_created".into(), format_datetime_opt(Some(&t.date_created), tz));
    m.insert("date_started".into(), format_datetime_opt(t.date_started.as_ref(), tz));
    m.insert("date_done".into(), format_datetime_opt(t.date_done.as_ref(), tz));
    m.insert("duration_seconds".into(), float(t.duration_seconds));
    m.insert("wait_time_seconds".into(), float(t.wait_time_seconds));
    m.insert("input_data".into(), t.input_data.clone());
    m.insert("result_data".into(), t.result_data.clone().unwrap_or(Value::Null));
    m.insert("related_document_ids".into(), Value::Array(related?));
    m.insert("acknowledged".into(), Value::Bool(t.acknowledged));
    m.insert("owner".into(), t.owner_id.map_or(Value::Null, Value::from));
    Ok(Value::Object(m))
}

/// `TaskSerializerV9.get_result`
pub fn v9_result(t: &TaskRow) -> Value {
    if t.result_data.as_ref().is_none_or(|v| !truthy(v)) {
        return Value::Null;
    }
    if let Some(d) = t.result_get("document_id").filter(|v| truthy(v)) {
        return Value::from(format!("Success. New document id {} created", py_str(d)));
    }
    if let Some(r) = t.result_get("reason").filter(|v| truthy(v)) {
        return r.clone();
    }
    if let Some(d) = t.result_get("duplicate_of").filter(|v| truthy(v)) {
        return Value::from(format!("Not consuming: It is a duplicate of document #{}", py_str(d)));
    }
    if let Some(e) = t.result_get("error_message").filter(|v| truthy(v)) {
        return e.clone();
    }
    Value::Null
}

/// `TaskSerializerV9._TRIGGER_SOURCE_TO_V9_TYPE`
pub fn v9_type(trigger_source: &str) -> &'static str {
    match trigger_source {
        "scheduled" => "scheduled_task",
        "system" | "email_consume" | "folder_consume" => "auto_task",
        _ => "manual_task",
    }
}

/// `TaskSerializerV9._TASK_TYPE_TO_V9_NAME`
pub fn v9_task_name(task_type: &str) -> &str {
    match task_type {
        "sanity_check" => "check_sanity",
        "llm_index" => "llmindex_update",
        other => other,
    }
}

/// `TaskSerializerV9(task).data`; `duplicates` = rows of `Document.global_objects`
/// (`{"id","title","deleted_at"}`) already permission-filtered for this task.
pub fn v9(t: &TaskRow, tz: &Tz, duplicates: Vec<Value>) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), Value::from(t.id));
    m.insert("task_id".into(), Value::from(t.task_id.clone()));
    m.insert("task_name".into(), Value::from(v9_task_name(&t.task_type)));
    let file_name = if truthy(&t.input_data) {
        t.input_data.as_object().and_then(|o| o.get("filename")).cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    m.insert("task_file_name".into(), file_name);
    m.insert("type".into(), Value::from(v9_type(&t.trigger_source)));
    m.insert("status".into(), Value::from(t.status.to_uppercase()));
    m.insert("date_created".into(), format_datetime_opt(Some(&t.date_created), tz));
    m.insert("date_done".into(), format_datetime_opt(t.date_done.as_ref(), tz));
    m.insert("result".into(), v9_result(t));
    m.insert("acknowledged".into(), Value::Bool(t.acknowledged));
    m.insert("related_document".into(), t.related_document_ids_raw().into_iter().next().unwrap_or(Value::Null));
    m.insert("duplicate_documents".into(), Value::Array(duplicates));
    m.insert("owner".into(), t.owner_id.map_or(Value::Null, Value::from));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn row() -> TaskRow {
        TaskRow {
            id: 1,
            task_id: "abc".into(),
            task_type: "sanity_check".into(),
            trigger_source: "email_consume".into(),
            status: "success".into(),
            date_created: Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap(),
            date_started: None,
            date_done: None,
            duration_seconds: Some(1.5),
            wait_time_seconds: None,
            input_data: json!({"filename": "report.pdf"}),
            result_data: Some(json!({"document_id": 7})),
            acknowledged: false,
            owner_id: None,
        }
    }

    #[test]
    fn v10_fields_in_order() {
        let v = v10(&row(), "en-us", &chrono_tz::UTC).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            vec![
                "id",
                "task_id",
                "task_type",
                "task_type_display",
                "trigger_source",
                "trigger_source_display",
                "status",
                "status_display",
                "date_created",
                "date_started",
                "date_done",
                "duration_seconds",
                "wait_time_seconds",
                "input_data",
                "result_data",
                "related_document_ids",
                "acknowledged",
                "owner"
            ]
        );
        assert_eq!(v["related_document_ids"], json!([7]));
        assert_eq!(v["date_created"], "2024-01-02T03:04:05Z");
        assert_eq!(v["task_type_display"], "Sanity Check");
    }

    #[test]
    fn related_ids_from_duplicate() {
        let mut r = row();
        r.result_data = Some(json!({"duplicate_of": "12"}));
        assert_eq!(v10(&r, "en-us", &chrono_tz::UTC).unwrap()["related_document_ids"], json!([12]));
        r.result_data = Some(json!({"document_id": 0}));
        assert_eq!(v10(&r, "en-us", &chrono_tz::UTC).unwrap()["related_document_ids"], json!([]));
    }

    #[test]
    fn v9_mapping() {
        let v = v9(&row(), &chrono_tz::UTC, vec![]);
        assert_eq!(v["task_name"], "check_sanity");
        assert_eq!(v["type"], "auto_task");
        assert_eq!(v["status"], "SUCCESS");
        assert_eq!(v["task_file_name"], "report.pdf");
        assert_eq!(v["result"], "Success. New document id 7 created");
        assert_eq!(v["related_document"], 7);
        let mut r = row();
        r.result_data = Some(json!({"duplicate_of": 3}));
        assert_eq!(v9_result(&r), "Not consuming: It is a duplicate of document #3");
        r.result_data = Some(json!({"error_message": "boom"}));
        assert_eq!(v9_result(&r), "boom");
        r.result_data = Some(json!({}));
        assert_eq!(v9_result(&r), Value::Null);
        r.input_data = json!({});
        assert_eq!(v9(&r, &chrono_tz::UTC, vec![])["task_file_name"], Value::Null);
        assert_eq!(v9_type("web_ui"), "manual_task");
        assert_eq!(v9_type("scheduled"), "scheduled_task");
    }
}
