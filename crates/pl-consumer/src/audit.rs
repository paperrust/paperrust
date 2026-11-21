// Audit log like django-auditlog (version from reference/venv) for the entries written during ingestion.
//
// upstream: auditlog/diff.py (get_field_value, model_instance_diff, get_fields_in_model),
//           auditlog/receivers.py (log_create, log_update, make_log_m2m_changes),
//           auditlog/models.py (LogEntryManager.log_create, log_m2m_changes),
//           src/documents/models.py@v3.4.2 (auditlog.register(Document, m2m_fields={"tags"},
//           exclude_fields=["content_length", "modified"]), CustomFieldInstance, Document.__str__)
//
// Value representation (`get_field_value`): DateTimeField → naive UTC (`str(datetime)`) or Python None;
// ForeignKey → ID or None (unchanged, `smart_str(..., strings_only=True)`); everything else →
// `smart_str(value)` ("None" for None). On creation the old value is always the string "None"
// (`get_default_value()` without an instance), which is why None DateTimes/FKs appear as ["None", "None"],
// None text fields do not.

use chrono::{DateTime, NaiveDate, Utc};
use pl_core::sqlbind::SqlxBinder as _;
use sea_query::{PostgresQueryBuilder, Query};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use crate::db::{DocRow, a, content_type_id, now_micros};

/// Python value of a field for the comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AV {
    /// Python `None` (DateTimeField/ForeignKey without a value)
    None,
    S(String),
}

impl AV {
    fn smart_str(&self) -> String {
        match self {
            AV::None => "None".into(),
            AV::S(s) => s.clone(),
        }
    }
}

/// `str(datetime)` naive in UTC (microseconds only if ≠ 0).
pub fn py_naive_utc(t: &DateTime<Utc>) -> String {
    let n = t.naive_utc();
    if n.and_utc().timestamp_subsec_micros() == 0 {
        n.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        n.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
    }
}

fn s_opt(v: &Option<String>) -> AV {
    AV::S(v.clone().unwrap_or_else(|| "None".into()))
}
fn i_opt(v: Option<i64>) -> AV {
    AV::S(v.map_or_else(|| "None".into(), |i| i.to_string()))
}
fn fk(v: Option<i64>) -> AV {
    v.map_or(AV::None, |i| AV::S(i.to_string()))
}
fn dt(v: &Option<DateTime<Utc>>) -> AV {
    v.as_ref().map_or(AV::None, |t| AV::S(py_naive_utc(t)))
}

/// What the `created` attribute of the Python instance currently looks like. The consumer passes
/// `Document.objects.create(created=<datetime>)`; until it is reloaded from the DB the
/// instance holds the `datetime` (auditlog then compares `str(datetime)` with `str(date)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreatedAttr {
    Date(NaiveDate),
    /// (`str(dt)`, `dt.isoformat()`)
    DateTime {
        str_: String,
        iso: String,
    },
}

impl CreatedAttr {
    pub fn smart_str(&self) -> String {
        match self {
            CreatedAttr::Date(d) => d.format("%Y-%m-%d").to_string(),
            CreatedAttr::DateTime { str_, .. } => str_.clone(),
        }
    }
    pub fn isoformat(&self) -> String {
        match self {
            CreatedAttr::Date(d) => d.format("%Y-%m-%d").to_string(),
            CreatedAttr::DateTime { iso, .. } => iso.clone(),
        }
    }
}

/// Concrete fields (`_meta.fields` without `content_length`, `modified`).
pub fn document_fields(d: &DocRow, created: &CreatedAttr) -> Vec<(&'static str, AV)> {
    vec![
        ("id", AV::S(d.id.to_string())),
        ("deleted_at", dt(&d.deleted_at)),
        ("restored_at", dt(&d.restored_at)),
        ("transaction_id", AV::S(d.transaction_id.map_or_else(|| "None".into(), |u| u.to_string()))),
        ("title", AV::S(d.title.clone())),
        ("content", AV::S(d.content.clone())),
        ("mime_type", AV::S(d.mime_type.clone())),
        ("checksum", AV::S(d.checksum.clone())),
        ("archive_checksum", s_opt(&d.archive_checksum)),
        ("page_count", i_opt(d.page_count)),
        ("created", AV::S(created.smart_str())),
        ("added", AV::S(py_naive_utc(&d.added))),
        ("filename", s_opt(&d.filename)),
        ("archive_filename", s_opt(&d.archive_filename)),
        ("original_filename", s_opt(&d.original_filename)),
        ("archive_serial_number", i_opt(d.archive_serial_number)),
        ("correspondent", fk(d.correspondent_id)),
        ("document_type", fk(d.document_type_id)),
        ("storage_path", fk(d.storage_path_id)),
        ("owner", fk(d.owner_id)),
        ("root_document", fk(d.root_document_id)),
        ("version_label", s_opt(&d.version_label)),
        ("version_index", i_opt(d.version_index)),
    ]
}

/// Reverse relations (`get_fields_in_model` on creation): `str(RelatedManager)`.
const DOCUMENT_REVERSE: &[(&str, &str)] = &[
    ("notes", "documents.Note.None"),
    ("barcodes", "documents.DocumentBarcode.None"),
    ("versions", "documents.Document.None"),
    ("share_links", "documents.ShareLink.None"),
    ("custom_fields", "documents.CustomFieldInstance.None"),
    ("workflow_runs", "documents.WorkflowRun.None"),
];

/// `model_instance_diff(None, new)`.
pub fn create_diff(fields: &[(&'static str, AV)], reverse: &[(&str, &str)]) -> Map<String, Value> {
    let mut m = Map::new();
    let old = AV::S("None".into());
    for (k, v) in fields {
        if *v != old {
            m.insert((*k).to_string(), json!(["None", v.smart_str()]));
        }
    }
    for (k, v) in reverse {
        m.insert((*k).to_string(), json!(["None", v]));
    }
    m
}

/// `model_instance_diff(old, new, fields_to_check)`.
pub fn update_diff(old: &[(&'static str, AV)], new: &[(&'static str, AV)], only: Option<&[&str]>) -> Map<String, Value> {
    let mut m = Map::new();
    for ((k, o), (_, n)) in old.iter().zip(new.iter()) {
        if let Some(f) = only
            && !f.contains(k)
        {
            continue;
        }
        if o != n {
            m.insert((*k).to_string(), json!([o.smart_str(), n.smart_str()]));
        }
    }
    m
}

/// `str(document)`: `f"{created.isoformat()}[ {correspondent}][ {title}]"`.
pub fn document_repr(created: &CreatedAttr, correspondent: Option<&str>, title: &str) -> String {
    let mut s = created.isoformat();
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

/// Writes entries to `auditlog_logentry` if `AUDIT_LOG_ENABLED`.
#[derive(Debug, Clone, Copy)]
pub struct AuditLog {
    pub enabled: bool,
    pub actor_id: Option<i64>,
}

pub const ACTION_CREATE: i16 = 0;
pub const ACTION_UPDATE: i16 = 1;

impl AuditLog {
    #[allow(clippy::too_many_arguments)]
    pub async fn write(
        &self,
        conn: &mut PgConnection,
        model: &str,
        object_id: i64,
        object_repr: &str,
        action: i16,
        changes: Map<String, Value>,
        additional_data: Option<Value>,
    ) -> Result<(), sqlx::Error> {
        if !self.enabled {
            return Ok(());
        }
        let Some(ct) = content_type_id(conn, "documents", model).await? else {
            return Err(sqlx::Error::Protocol(format!("ContentType documents.{model} missing")));
        };
        let q = Query::insert()
            .into_table(a("auditlog_logentry"))
            .columns([
                a("object_pk"),
                a("object_id"),
                a("object_repr"),
                a("action"),
                a("changes"),
                a("timestamp"),
                a("actor_id"),
                a("content_type_id"),
                a("additional_data"),
                a("changes_text"),
            ])
            .values_panic([
                object_id.to_string().into(),
                object_id.into(),
                object_repr.into(),
                action.into(),
                Value::Object(changes).into(),
                now_micros().into(),
                self.actor_id.map(|i| i as i32).into(),
                ct.into(),
                additional_data.into(),
                "".into(),
            ])
            .to_owned();
        let (sql, v) = q.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, v).execute(&mut *conn).await?;
        Ok(())
    }

    /// `log_create` for a new document.
    pub async fn document_created(
        &self,
        conn: &mut PgConnection,
        d: &DocRow,
        created: &CreatedAttr,
        correspondent: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let ch = create_diff(&document_fields(d, created), DOCUMENT_REVERSE);
        self.write(conn, "document", d.id, &document_repr(created, correspondent, &d.title), ACTION_CREATE, ch, None).await
    }

    /// `log_update` (pre_save): difference DB state → instance; no entry without a change.
    #[allow(clippy::too_many_arguments)]
    pub async fn document_updated(
        &self,
        conn: &mut PgConnection,
        old: &DocRow,
        new: &DocRow,
        new_created: &CreatedAttr,
        only: Option<&[&str]>,
        correspondent: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let ch = update_diff(&document_fields(old, &CreatedAttr::Date(old.created)), &document_fields(new, new_created), only);
        if ch.is_empty() {
            return Ok(());
        }
        self.write(conn, "document", new.id, &document_repr(new_created, correspondent, &new.title), ACTION_UPDATE, ch, None).await
    }

    /// `log_m2m_changes(..., "add", "tags")`; names by `Tag.Meta.ordering`.
    pub async fn tags_added(&self, conn: &mut PgConnection, doc_id: i64, repr: &str, names: &[String]) -> Result<(), sqlx::Error> {
        if names.is_empty() {
            return Ok(());
        }
        let mut ch = Map::new();
        ch.insert("tags".into(), json!({"type": "m2m", "operation": "add", "objects": names}));
        self.write(conn, "document", doc_id, repr, ACTION_UPDATE, ch, None).await
    }

    /// `log_create` for a `CustomFieldInstance`.
    #[allow(clippy::too_many_arguments)]
    pub async fn custom_field_instance_created(
        &self,
        conn: &mut PgConnection,
        id: i64,
        field_id: i64,
        doc_id: i64,
        created: &DateTime<Utc>,
        value_field: &str,
        value_repr: Option<String>,
        monetary_amount: Option<String>,
        repr: &str,
    ) -> Result<(), sqlx::Error> {
        let mut fields: Vec<(&'static str, AV)> = vec![
            ("id", AV::S(id.to_string())),
            ("deleted_at", AV::None),
            ("restored_at", AV::None),
            ("transaction_id", AV::S("None".into())),
            ("created", AV::S(py_naive_utc(created))),
            ("field", AV::S(field_id.to_string())),
            ("document", AV::S(doc_id.to_string())),
        ];
        for vf in [
            "value_text",
            "value_bool",
            "value_url",
            "value_date",
            "value_int",
            "value_float",
            "value_monetary",
            "value_select",
            "value_long_text",
        ] {
            let v = if vf == value_field { value_repr.clone() } else { None };
            fields.push((static_name(vf), AV::S(v.unwrap_or_else(|| "None".into()))));
        }
        // JSONField: json.dumps(None) == "null"
        let docids = if value_field == "value_document_ids" { value_repr.unwrap_or_else(|| "null".into()) } else { "null".into() };
        fields.push(("value_document_ids", AV::S(docids)));
        fields.push(("value_monetary_amount", AV::S(monetary_amount.unwrap_or_else(|| "None".into()))));
        let ch = create_diff(&fields, &[]);
        self.write(conn, "customfieldinstance", id, repr, ACTION_CREATE, ch, None).await
    }

    /// `log_update` for a `CustomFieldInstance` after `instance.save()` with a new value
    /// (`apply_assignment_to_document`, gap W7): only the value field; without a change no entry
    /// (auditlog writes only for a non-empty diff).
    pub async fn custom_field_instance_updated(
        &self,
        conn: &mut PgConnection,
        id: i64,
        value_field: &str,
        old: Option<String>,
        new: Option<String>,
        repr: &str,
    ) -> Result<(), sqlx::Error> {
        let Some(ch) = cfi_update_diff(value_field, old, new) else { return Ok(()) };
        self.write(conn, "customfieldinstance", id, repr, ACTION_UPDATE, ch, None).await
    }
}

/// Diff `{value_field: [old, new]}` (`smart_str`, `None` for NULL); `None` without a change.
pub fn cfi_update_diff(value_field: &str, old: Option<String>, new: Option<String>) -> Option<Map<String, Value>> {
    let old = old.unwrap_or_else(|| "None".into());
    let new = new.unwrap_or_else(|| "None".into());
    if old == new {
        return None;
    }
    let mut ch = Map::new();
    ch.insert(value_field.to_string(), json!([old, new]));
    Some(ch)
}

fn static_name(s: &str) -> &'static str {
    match s {
        "value_text" => "value_text",
        "value_bool" => "value_bool",
        "value_url" => "value_url",
        "value_date" => "value_date",
        "value_int" => "value_int",
        "value_float" => "value_float",
        "value_monetary" => "value_monetary",
        "value_select" => "value_select",
        _ => "value_long_text",
    }
}

/// Python representation of a custom field value (`smart_str(value)` or `json.dumps` for documentlink).
pub fn custom_value_smart_str(data_type: &str, v: &Value) -> Option<String> {
    if v.is_null() {
        return None;
    }
    Some(match (data_type, v) {
        ("boolean", Value::Bool(b)) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        ("float", Value::Number(n)) => py_float_repr(n.as_f64().unwrap_or(0.0)),
        ("documentlink", other) => python_json_dumps(other),
        (_, Value::String(s)) => s.clone(),
        (_, other) => other.to_string(),
    })
}

/// `json.dumps` (separators ", " and ": ").
fn python_json_dumps(v: &Value) -> String {
    pl_queue::events::python_json_dumps(v)
}

/// Python `repr(float)` (shortest representation).
pub fn py_float_repr(f: f64) -> String {
    if f.is_finite() && f == f.trunc() && f.abs() < 1e16 {
        return format!("{f:.1}");
    }
    let s = format!("{f}");
    if s.contains('e') || s.contains('.') { s } else { format!("{s}.0") }
}

/// `value_for_search` for the representation `str(CustomFieldInstance)`.
pub fn custom_value_for_search(data_type: &str, v: &Value, extra: Option<&Value>) -> String {
    if v.is_null() {
        return "None".into();
    }
    match data_type {
        "select" => {
            let id = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            extra
                .and_then(|e| e.get("select_options"))
                .and_then(Value::as_array)
                .and_then(|opts| opts.iter().find(|o| o.get("id").and_then(Value::as_str) == Some(id.as_str())))
                .and_then(|o| o.get("label").and_then(Value::as_str))
                .map_or_else(|| "None".into(), str::to_string)
        }
        "documentlink" => match v {
            Value::Array(items) => {
                let inner: Vec<String> = items.iter().map(|i| i.to_string()).collect();
                format!("[{}]", inner.join(", "))
            }
            other => other.to_string(),
        },
        _ => custom_value_smart_str(data_type, v).unwrap_or_else(|| "None".into()),
    }
}

/// `Document.objects.filter(pk=root).update(...)` and the like need no audit; only a helper for
/// the version entry (`LogEntry.objects.log_create(instance=root_doc, changes={"Version Added": ...})`).
pub async fn version_added(
    log: &AuditLog,
    conn: &mut PgConnection,
    root: &DocRow,
    root_correspondent: Option<&str>,
    version_id: i64,
) -> Result<(), sqlx::Error> {
    let mut ch = Map::new();
    ch.insert("Version Added".into(), json!(["None", version_id]));
    log.write(
        conn,
        "document",
        root.id,
        &document_repr(&CreatedAttr::Date(root.created), root_correspondent, &root.title),
        ACTION_UPDATE,
        ch,
        Some(json!({"reason": "Version added", "version_id": version_id})),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> DocRow {
        let added = DateTime::parse_from_rfc3339("2026-11-07T22:26:01.798356Z").unwrap().with_timezone(&Utc);
        DocRow {
            id: 1,
            title: "T".into(),
            content: "c".into(),
            mime_type: "application/pdf".into(),
            checksum: "abc".into(),
            archive_checksum: None,
            page_count: Some(2),
            created: NaiveDate::from_ymd_opt(2019, 1, 14).unwrap(),
            modified: added,
            added,
            filename: None,
            archive_filename: None,
            original_filename: Some("scan_0001.pdf".into()),
            archive_serial_number: Some(1001),
            correspondent_id: Some(2),
            document_type_id: Some(2),
            storage_path_id: None,
            owner_id: Some(2),
            root_document_id: None,
            version_label: None,
            version_index: None,
            deleted_at: None,
            restored_at: None,
            transaction_id: None,
        }
    }

    #[test]
    fn create_diff_matches_reference_entry() {
        // reference DB auditlog_logentry id 52 (seed via ORM)
        let d = doc();
        let ch = create_diff(&document_fields(&d, &CreatedAttr::Date(d.created)), DOCUMENT_REVERSE);
        let expected = json!({"id": ["None", "1"], "added": ["None", "2026-11-07 22:26:01.798356"], "notes": ["None", "documents.Note.None"], "owner": ["None", "2"], "title": ["None", "T"], "content": ["None", "c"], "created": ["None", "2019-01-14"], "barcodes": ["None", "documents.DocumentBarcode.None"], "checksum": ["None", "abc"], "versions": ["None", "documents.Document.None"], "mime_type": ["None", "application/pdf"], "deleted_at": ["None", "None"], "page_count": ["None", "2"], "restored_at": ["None", "None"], "share_links": ["None", "documents.ShareLink.None"], "storage_path": ["None", "None"], "correspondent": ["None", "2"], "custom_fields": ["None", "documents.CustomFieldInstance.None"], "document_type": ["None", "2"], "root_document": ["None", "None"], "workflow_runs": ["None", "documents.WorkflowRun.None"], "original_filename": ["None", "scan_0001.pdf"], "archive_serial_number": ["None", "1001"]});
        assert_eq!(Value::Object(ch), expected);
    }

    #[test]
    fn cfi_update_diff_only_on_change() {
        assert_eq!(cfi_update_diff("value_int", Some("5".into()), Some("5".into())), None);
        let ch = cfi_update_diff("value_text", None, Some("neu".into())).map(Value::Object);
        assert_eq!(ch, Some(json!({"value_text": ["None", "neu"]})));
    }

    #[test]
    fn update_diff_created_datetime() {
        let old = doc();
        let mut new = doc();
        new.correspondent_id = Some(3);
        let c = CreatedAttr::DateTime { str_: "2019-01-14 10:00:00+01:00".into(), iso: "2019-01-14T10:00:00+01:00".into() };
        let ch = update_diff(&document_fields(&old, &CreatedAttr::Date(old.created)), &document_fields(&new, &c), None);
        assert_eq!(Value::Object(ch), json!({"created": ["2019-01-14", "2019-01-14 10:00:00+01:00"], "correspondent": ["2", "3"]}));
        assert_eq!(document_repr(&c, Some("ACME"), "T"), "2019-01-14T10:00:00+01:00 ACME T");
    }

    #[test]
    fn floats() {
        assert_eq!(py_float_repr(1.0), "1.0");
        assert_eq!(py_float_repr(0.5), "0.5");
        assert_eq!(custom_value_smart_str("boolean", &Value::Bool(false)).as_deref(), Some("False"));
        assert_eq!(custom_value_smart_str("documentlink", &json!([1, 2])).as_deref(), Some("[1, 2]"));
    }
}
