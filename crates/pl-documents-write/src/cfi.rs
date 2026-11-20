// upstream: src/documents/models.py@v3.4.2 (CustomFieldInstance, TYPE_TO_DATA_STORE_NAME_MAP),
//           django/db/models/query.py (update_or_create: select_for_update().get(), save with
//           update_fields = defaults + fields with their own pre_save; otherwise create()),
//           src/documents/bulk_edit.py@v3.4.2 (reflect_doclinks, remove_doclink),
//           django_softdelete 1.0.23 (SoftDeleteModel.delete/restore), django/db/models/deletion.py
//           (Collector: post_delete per instance, descending by pk)
//
//! `CustomFieldInstance` with the side effects of the Django models (auditlog, file name).

use chrono::NaiveDate;
use serde_json::Value;
use sqlx::{PgConnection, Row};

use crate::auditlog::{self, CfiRow, LogCtx};
use crate::docs::now_micros;
use crate::error::ApiError;
use crate::files::{self, MediaCfg};

/// `CustomField`.
#[derive(Clone, Debug)]
pub struct FieldDef {
    pub id: i64,
    pub name: String,
    pub data_type: String,
    pub extra_data: Option<Value>,
}

pub async fn field_def(conn: &mut PgConnection, id: i64) -> Result<Option<FieldDef>, sqlx::Error> {
    let r = sqlx::query("SELECT id, name, data_type, extra_data FROM documents_customfield WHERE id = $1")
        .bind(id as i32)
        .fetch_optional(&mut *conn)
        .await?;
    r.map(|r| -> Result<FieldDef, sqlx::Error> {
        Ok(FieldDef {
            id: i64::from(r.try_get::<i32, _>(0)?),
            name: r.try_get(1)?,
            data_type: r.try_get(2)?,
            extra_data: r.try_get(3)?,
        })
    })
    .transpose()
}

const COLS: &str = "id, created, document_id, field_id, value_text, value_bool, value_url, value_date, value_int, \
                    value_float, value_monetary, value_monetary_amount::text, value_document_ids, value_select, \
                    value_long_text, deleted_at, restored_at, transaction_id";

fn from_row(r: &sqlx::postgres::PgRow) -> Result<CfiRow, sqlx::Error> {
    Ok(CfiRow {
        id: i64::from(r.try_get::<i32, _>(0)?),
        created: r.try_get(1)?,
        document_id: i64::from(r.try_get::<i32, _>(2)?),
        field_id: i64::from(r.try_get::<i32, _>(3)?),
        value_text: r.try_get(4)?,
        value_bool: r.try_get(5)?,
        value_url: r.try_get(6)?,
        value_date: r.try_get(7)?,
        value_int: r.try_get::<Option<i32>, _>(8)?.map(i64::from),
        value_float: r.try_get(9)?,
        value_monetary: r.try_get(10)?,
        value_monetary_amount: r.try_get(11)?,
        value_document_ids: r.try_get(12)?,
        value_select: r.try_get(13)?,
        value_long_text: r.try_get(14)?,
        deleted_at: r.try_get(15)?,
        restored_at: r.try_get(16)?,
        transaction_id: r.try_get(17)?,
    })
}

/// Loads instances (`where` with `$1` = document id), order `created, id` (Meta.ordering).
async fn query(conn: &mut PgConnection, where_: &str, doc_id: i64) -> Result<Vec<CfiRow>, sqlx::Error> {
    let sql = format!("SELECT {COLS} FROM documents_customfieldinstance WHERE {where_} ORDER BY created, id");
    let rows = sqlx::query(&sql).bind(doc_id as i32).fetch_all(&mut *conn).await?;
    rows.iter().map(from_row).collect()
}

/// `document.custom_fields.all()` (not deleted).
pub async fn live_for_document(conn: &mut PgConnection, doc_id: i64) -> Result<Vec<CfiRow>, sqlx::Error> {
    query(conn, "document_id = $1 AND deleted_at IS NULL", doc_id).await
}

/// `CustomFieldInstance.deleted_objects.filter(document=…)`.
pub async fn deleted_for_document(conn: &mut PgConnection, doc_id: i64) -> Result<Vec<CfiRow>, sqlx::Error> {
    query(conn, "document_id = $1 AND deleted_at IS NOT NULL", doc_id).await
}

pub async fn load(conn: &mut PgConnection, id: i64) -> Result<Option<CfiRow>, sqlx::Error> {
    let sql = format!("SELECT {COLS} FROM documents_customfieldinstance WHERE id = $1");
    let r = sqlx::query(&sql).bind(id as i32).fetch_optional(&mut *conn).await?;
    r.as_ref().map(from_row).transpose()
}

/// `CustomFieldInstance.objects.filter(document=…, field=…).first()`.
pub async fn live_for(conn: &mut PgConnection, doc_id: i64, field_id: i64) -> Result<Option<CfiRow>, sqlx::Error> {
    let sql = format!(
        "SELECT {COLS} FROM documents_customfieldinstance WHERE document_id = $1 AND field_id = $2 AND deleted_at IS NULL \
         ORDER BY created, id LIMIT 1"
    );
    let r = sqlx::query(&sql).bind(doc_id as i32).bind(field_id as i32).fetch_optional(&mut *conn).await?;
    r.as_ref().map(from_row).transpose()
}

/// Column for the data type (`get_value_field_name`).
pub fn value_field(data_type: &str) -> Option<&'static str> {
    pl_consumer::db::value_field_name(data_type)
}

/// Value of an instance's column as JSON (for `CustomFieldInstance.value`).
pub fn value_of(c: &CfiRow, data_type: &str) -> Value {
    match data_type {
        "string" => c.value_text.clone().map(Value::String).unwrap_or(Value::Null),
        "url" => c.value_url.clone().map(Value::String).unwrap_or(Value::Null),
        "date" => c.value_date.map(|d| Value::String(d.format("%Y-%m-%d").to_string())).unwrap_or(Value::Null),
        "boolean" => c.value_bool.map(Value::Bool).unwrap_or(Value::Null),
        "integer" => c.value_int.map(Value::from).unwrap_or(Value::Null),
        "float" => c.value_float.map(Value::from).unwrap_or(Value::Null),
        "monetary" => c.value_monetary.clone().map(Value::String).unwrap_or(Value::Null),
        "documentlink" => c.value_document_ids.clone().unwrap_or(Value::Null),
        "select" => c.value_select.clone().map(Value::String).unwrap_or(Value::Null),
        "longtext" => c.value_long_text.clone().map(Value::String).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// Sets the value in the instance (like `setattr(instance, value_field, value)`; the value is already
/// validated or converted like Django's `get_prep_value`). `Err` = Django would raise
/// an exception on save (→ 500).
pub fn set_value(c: &mut CfiRow, data_type: &str, v: &Value) -> Result<(), String> {
    let bad = || format!("invalid value {v} for {data_type}");
    match data_type {
        "string" => c.value_text = py_text(v),
        "url" => c.value_url = py_text(v),
        "longtext" => c.value_long_text = py_text(v),
        "monetary" => c.value_monetary = py_text(v),
        "select" => c.value_select = py_text(v),
        "date" => {
            c.value_date = match v {
                Value::Null => None,
                Value::String(s) => Some(NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| bad())?),
                _ => return Err(bad()),
            }
        }
        "boolean" => {
            c.value_bool = match v {
                Value::Null => None,
                Value::Bool(b) => Some(*b),
                Value::Number(n) if n.as_f64() == Some(1.0) => Some(true),
                Value::Number(n) if n.as_f64() == Some(0.0) => Some(false),
                Value::String(s) => match s.as_str() {
                    "t" | "True" | "1" => Some(true),
                    "f" | "False" | "0" => Some(false),
                    _ => return Err(bad()),
                },
                _ => return Err(bad()),
            }
        }
        "integer" => {
            c.value_int = match v {
                Value::Null => None,
                other => Some(crate::drf::django_int_lookup(other).map_err(|_| bad())?.ok_or_else(bad)?),
            }
        }
        "float" => {
            c.value_float = match v {
                Value::Null => None,
                Value::Number(n) => n.as_f64(),
                Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
                Value::String(s) => Some(s.trim().parse::<f64>().map_err(|_| bad())?),
                _ => return Err(bad()),
            }
        }
        "documentlink" => c.value_document_ids = if v.is_null() { None } else { Some(v.clone()) },
        _ => return Err(bad()),
    }
    Ok(())
}

/// Django's `CharField.to_python` → `str(value)` (numbers as Python `str`).
fn py_text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(crate::drf::py_str(other)),
    }
}

async fn write_value(conn: &mut PgConnection, c: &CfiRow, data_type: &str) -> Result<(), sqlx::Error> {
    let Some(col) = value_field(data_type) else { return Ok(()) };
    let sql = format!("UPDATE documents_customfieldinstance SET {col} = $2 WHERE id = $1");
    let q = sqlx::query(&sql).bind(c.id as i32);
    let q = match data_type {
        "string" => q.bind(c.value_text.clone()),
        "url" => q.bind(c.value_url.clone()),
        "longtext" => q.bind(c.value_long_text.clone()),
        "monetary" => q.bind(c.value_monetary.clone()),
        "select" => q.bind(c.value_select.clone()),
        "date" => q.bind(c.value_date),
        "boolean" => q.bind(c.value_bool),
        "integer" => q.bind(c.value_int),
        "float" => q.bind(c.value_float),
        "documentlink" => q.bind(c.value_document_ids.clone()),
        _ => return Ok(()),
    };
    q.execute(&mut *conn).await?;
    Ok(())
}

async fn repr(conn: &mut PgConnection, c: &CfiRow) -> Result<String, sqlx::Error> {
    let f = field_def(conn, c.field_id).await?;
    Ok(match f {
        Some(f) => auditlog::cfi_repr(&f.name, &f.data_type, f.extra_data.as_ref(), c),
        None => format!(" : {}", "None"),
    })
}

/// `instance.save(update_fields=…)` of an existing instance: pre_save audit (old via
/// `objects`, i.e. without trash), UPDATE of the value column or the delete fields, post_save.
async fn save_audit(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    new: &CfiRow,
    only: Option<&[&str]>,
) -> Result<(), sqlx::Error> {
    if !ctx.enabled {
        return Ok(());
    }
    let old = load(conn, new.id).await?.filter(|o| o.deleted_at.is_none());
    let ov = old.as_ref().map(auditlog::cfi_fields);
    let nv = auditlog::cfi_fields(new);
    let ch = auditlog::diff(ov.as_ref(), Some(&nv), &[], only);
    if ch.is_some() {
        let r = repr(conn, new).await?;
        auditlog::write_diff(conn, ctx, "customfieldinstance", new.id, &r, auditlog::UPDATE, ch).await?;
    }
    Ok(())
}

/// Fields that `update_or_create` saves on update (defaults + fields with pre_save).
fn update_or_create_fields(value_col: &str) -> Vec<&str> {
    vec![value_col, "created", "deleted_at", "restored_at"]
}

/// `CustomFieldInstance.objects.update_or_create(document=…, field=…, defaults={col: value})`.
/// Returns the id.
pub async fn update_or_create(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    doc_id: i64,
    field: &FieldDef,
    value: &Value,
) -> Result<i64, ApiError> {
    let col = value_field(&field.data_type).ok_or_else(|| ApiError::Uncaught(format!("data type {}", field.data_type)))?;
    match live_for(conn, doc_id, field.id).await? {
        Some(mut c) => {
            set_value(&mut c, &field.data_type, value).map_err(ApiError::Uncaught)?;
            let only = update_or_create_fields(col);
            save_audit(conn, ctx, &c, Some(&only)).await?;
            write_value(conn, &c, &field.data_type).await?;
            files::after_custom_field_saved(conn, media, doc_id).await?;
            Ok(c.id)
        }
        None => create(conn, ctx, media, doc_id, field, value).await,
    }
}

/// `CustomFieldInstance.objects.create(document=…, field=…, <col>=value)`.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    doc_id: i64,
    field: &FieldDef,
    value: &Value,
) -> Result<i64, ApiError> {
    let mut c = CfiRow {
        id: 0,
        created: now_micros(),
        document_id: doc_id,
        field_id: field.id,
        value_text: None,
        value_bool: None,
        value_url: None,
        value_date: None,
        value_int: None,
        value_float: None,
        value_monetary: None,
        value_monetary_amount: None,
        value_document_ids: None,
        value_select: None,
        value_long_text: None,
        deleted_at: None,
        restored_at: None,
        transaction_id: None,
    };
    set_value(&mut c, &field.data_type, value).map_err(ApiError::Uncaught)?;
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO documents_customfieldinstance (created, document_id, field_id, value_text, value_bool, value_url, \
         value_date, value_int, value_float, value_monetary, value_document_ids, value_select, value_long_text) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING id",
    )
    .bind(c.created)
    .bind(doc_id as i32)
    .bind(field.id as i32)
    .bind(&c.value_text)
    .bind(c.value_bool)
    .bind(&c.value_url)
    .bind(c.value_date)
    .bind(c.value_int)
    .bind(c.value_float)
    .bind(&c.value_monetary)
    .bind(&c.value_document_ids)
    .bind(&c.value_select)
    .bind(&c.value_long_text)
    .fetch_one(&mut *conn)
    .await?;
    let c = load(conn, i64::from(id)).await?.ok_or_else(|| ApiError::Uncaught("cfi vanished".into()))?;
    if ctx.enabled {
        let nv = auditlog::cfi_fields(&c);
        let ch = auditlog::diff(None, Some(&nv), &[], None);
        let r = auditlog::cfi_repr(&field.name, &field.data_type, field.extra_data.as_ref(), &c);
        auditlog::write_diff(conn, ctx, "customfieldinstance", c.id, &r, auditlog::CREATE, ch).await?;
    }
    files::after_custom_field_saved(conn, media, doc_id).await?;
    Ok(c.id)
}

/// `instance.save()` (all fields) of an existing instance with a new value (workflow assignment).
pub async fn save_full(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    c: &CfiRow,
    data_type: &str,
) -> Result<(), ApiError> {
    save_audit(conn, ctx, c, None).await?;
    write_value(conn, c, data_type).await?;
    files::after_custom_field_saved(conn, media, c.document_id).await?;
    Ok(())
}

/// Soft delete of an instance (`SoftDeleteModel.delete`): `deleted_at`, `restored_at = None`,
/// `transaction_id`; `then_full_save` = additionally `related_object.save()` of the cascade.
pub async fn soft_delete(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    c: &CfiRow,
    transaction_id: uuid::Uuid,
    then_full_save: bool,
) -> Result<(), ApiError> {
    let mut n = c.clone();
    n.deleted_at = Some(now_micros());
    n.restored_at = None;
    n.transaction_id = Some(transaction_id);
    let only = ["deleted_at", "restored_at", "transaction_id"];
    save_audit(conn, ctx, &n, Some(&only)).await?;
    set_delete_fields(conn, &n).await?;
    files::after_custom_field_saved(conn, media, c.document_id).await?;
    if then_full_save {
        save_audit(conn, ctx, &n, None).await?;
        files::after_custom_field_saved(conn, media, c.document_id).await?;
    }
    Ok(())
}

/// `SoftDeleteModel.restore` + `related_object.save()`.
pub async fn restore(conn: &mut PgConnection, ctx: &LogCtx, media: &MediaCfg, c: &CfiRow) -> Result<(), ApiError> {
    let mut n = c.clone();
    n.deleted_at = None;
    n.restored_at = Some(now_micros());
    n.transaction_id = None;
    let only = ["deleted_at", "restored_at", "transaction_id"];
    save_audit(conn, ctx, &n, Some(&only)).await?;
    set_delete_fields(conn, &n).await?;
    files::after_custom_field_saved(conn, media, c.document_id).await?;
    // related_object.save(): old found now, no change → no entry
    save_audit(conn, ctx, &n, None).await?;
    files::after_custom_field_saved(conn, media, c.document_id).await?;
    Ok(())
}

async fn set_delete_fields(conn: &mut PgConnection, c: &CfiRow) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE documents_customfieldinstance SET deleted_at = $2, restored_at = $3, transaction_id = $4 WHERE id = $1")
        .bind(c.id as i32)
        .bind(c.deleted_at)
        .bind(c.restored_at)
        .bind(c.transaction_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Hard delete (collector): post_delete → auditlog DELETE per instance, descending by pk.
pub async fn hard_delete(conn: &mut PgConnection, ctx: &LogCtx, rows: &[CfiRow]) -> Result<(), ApiError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut sorted: Vec<&CfiRow> = rows.iter().collect();
    sorted.sort_by_key(|c| c.id);
    let ids: Vec<i32> = sorted.iter().map(|c| c.id as i32).collect();
    let mut reprs = Vec::new();
    for c in &sorted {
        reprs.push(repr(conn, c).await?);
    }
    sqlx::query("DELETE FROM documents_customfieldinstance WHERE id = ANY($1)").bind(&ids).execute(&mut *conn).await?;
    if ctx.enabled {
        for (c, r) in sorted.iter().zip(reprs.iter()).rev() {
            let ov = auditlog::cfi_fields(c);
            let ch = auditlog::diff(Some(&ov), None, &[], None);
            auditlog::write_diff(conn, ctx, "customfieldinstance", c.id, r, auditlog::DELETE, ch).await?;
        }
    }
    Ok(())
}

/// `CustomFieldInstance.objects.filter(…).hard_delete()` (QuerySet.delete without soft delete).
pub async fn hard_delete_where(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    doc_ids: &[i64],
    field_ids: Option<&[i64]>,
) -> Result<(), ApiError> {
    let d: Vec<i32> = doc_ids.iter().map(|i| *i as i32).collect();
    let mut rows = Vec::new();
    let sql = format!("SELECT {COLS} FROM documents_customfieldinstance WHERE document_id = ANY($1) AND deleted_at IS NULL");
    for r in sqlx::query(&sql).bind(&d).fetch_all(&mut *conn).await? {
        let c = from_row(&r)?;
        if field_ids.is_none_or(|f| f.contains(&c.field_id)) {
            rows.push(c);
        }
    }
    hard_delete(conn, ctx, &rows).await
}

/// `remove_doclink(document, field, target_doc_id)`.
pub async fn remove_doclink(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    document_id: i64,
    field: &FieldDef,
    target_doc_id: i64,
) -> Result<(), ApiError> {
    if let Some(mut t) = live_for(conn, target_doc_id, field.id).await? {
        let list: Option<Vec<Value>> = t.value_document_ids.as_ref().and_then(Value::as_array).cloned();
        match list {
            Some(mut l) if l.iter().any(|x| x.as_i64() == Some(document_id)) => {
                if let Some(pos) = l.iter().position(|x| x.as_i64() == Some(document_id)) {
                    l.remove(pos);
                }
                t.value_document_ids = Some(Value::Array(l));
                save_full(conn, ctx, media, &t, "documentlink").await?;
            }
            Some(_) => {}
            // `document.id in None` → TypeError
            None => return Err(ApiError::Uncaught("argument of type 'NoneType' is not iterable".into())),
        }
    }
    sqlx::query("UPDATE documents_document SET modified = $2 WHERE id = $1 AND deleted_at IS NULL")
        .bind(target_doc_id as i32)
        .bind(now_micros())
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// `reflect_doclinks(document, field, target_doc_ids)`.
pub async fn reflect_doclinks(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    document_id: i64,
    field: &FieldDef,
    target_doc_ids: &[i64],
) -> Result<(), ApiError> {
    if let Some(cur) = live_for(conn, document_id, field.id).await?
        && let Some(list) = cur.value_document_ids.as_ref().and_then(Value::as_array)
    {
        for d in list.iter().filter_map(Value::as_i64) {
            if !target_doc_ids.contains(&d) {
                remove_doclink(conn, ctx, media, document_id, field, d).await?;
            }
        }
    }
    // bulk_create / bulk_update: no signals
    for t in target_doc_ids {
        let existing = sqlx::query(
            "SELECT id, value_document_ids FROM documents_customfieldinstance WHERE field_id = $1 AND document_id = $2 \
             AND deleted_at IS NULL ORDER BY created, id LIMIT 1",
        )
        .bind(field.id as i32)
        .bind(*t as i32)
        .fetch_optional(&mut *conn)
        .await?;
        match existing {
            None => {
                sqlx::query(
                    "INSERT INTO documents_customfieldinstance (created, document_id, field_id, value_document_ids) \
                     VALUES ($1, $2, $3, $4)",
                )
                .bind(now_micros())
                .bind(*t as i32)
                .bind(field.id as i32)
                .bind(Value::Array(vec![Value::from(document_id)]))
                .execute(&mut *conn)
                .await?;
            }
            Some(r) => {
                let id: i32 = r.try_get(0)?;
                let v: Option<Value> = r.try_get(1)?;
                let new = match v.as_ref().and_then(Value::as_array) {
                    None => Some(Value::Array(vec![Value::from(document_id)])),
                    Some(l) if !l.iter().any(|x| x.as_i64() == Some(document_id)) => {
                        let mut l = l.clone();
                        l.push(Value::from(document_id));
                        Some(Value::Array(l))
                    }
                    Some(_) => None,
                };
                if let Some(n) = new {
                    sqlx::query("UPDATE documents_customfieldinstance SET value_document_ids = $2 WHERE id = $1")
                        .bind(id)
                        .bind(n)
                        .execute(&mut *conn)
                        .await?;
                }
            }
        }
    }
    if !target_doc_ids.is_empty() {
        // Locks in id order (FINDINGS B4)
        pl_consumer::db::touch_modified_sorted(conn, target_doc_ids, now_micros()).await?;
    }
    Ok(())
}
