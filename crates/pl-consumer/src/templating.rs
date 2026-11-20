// Document from the DB as template context for pl-templating (generate_filename, workflow titles).
//
// upstream: src/documents/templating/filepath.py@v3.4.2 (context), src/documents/models.py@v3.4.2
//           (Document.file_type = get_default_file_extension(mime_type)), src/documents/workflows/
//           mutations.py@v3.4.2 (parse_w_workflow_placeholders arguments)

use chrono::NaiveDate;
use pl_templating::{
    CustomFieldInstance, CustomFieldValue, DocumentContext, NamedRef, OwnerRef, SelectOption, StoragePathRef, TagRef,
};
use sea_query::{Expr, Order, Query};
use serde_json::Value;
use sqlx::{PgConnection, Row};

use crate::db::{self, DocRow, a, col};

async fn named(conn: &mut PgConnection, table: &str, id: Option<i64>) -> Result<Option<NamedRef>, sqlx::Error> {
    let Some(id) = id else { return Ok(None) };
    Ok(db::object_name(conn, table, id).await?.map(|name| NamedRef { id: Some(id), name }))
}

/// Custom fields from the overrides as template values: `[CustomFieldInstance(field=f,
/// value_<type>=overrides.custom_fields[f.pk]) for f in CustomField.objects.filter(pk__in=…)]`
/// (order `created`; unknown IDs are dropped). For `_parse_title_placeholders` and
/// `build_workflow_action_context(..., overrides)`.
pub async fn override_custom_fields(
    conn: &mut PgConnection,
    custom_fields: &std::collections::BTreeMap<i64, Value>,
) -> Result<Vec<CustomFieldInstance>, sqlx::Error> {
    let ids: Vec<i64> = custom_fields.keys().copied().collect();
    Ok(db::custom_fields(conn, &ids)
        .await?
        .iter()
        .map(|f| crate::consumer::cf_template_value(f, custom_fields.get(&f.id).unwrap_or(&Value::Null)))
        .collect())
}

/// `CustomFieldInstance.global_objects.filter(document=document)` as template values.
pub async fn custom_field_instances(conn: &mut PgConnection, doc_id: i64) -> Result<Vec<CustomFieldInstance>, sqlx::Error> {
    let q = Query::select()
        .expr(Expr::col((a("i"), a("value_text"))))
        .expr(Expr::col((a("i"), a("value_bool"))))
        .expr(Expr::col((a("i"), a("value_url"))))
        .expr(Expr::col((a("i"), a("value_date"))))
        .expr(Expr::col((a("i"), a("value_int"))))
        .expr(Expr::col((a("i"), a("value_float"))))
        .expr(Expr::col((a("i"), a("value_monetary"))))
        .expr(Expr::col((a("i"), a("value_document_ids"))))
        .expr(Expr::col((a("i"), a("value_select"))))
        .expr(Expr::col((a("i"), a("value_long_text"))))
        .expr(Expr::col((a("f"), a("name"))))
        .expr(Expr::col((a("f"), a("data_type"))))
        .expr(Expr::col((a("f"), a("extra_data"))))
        .from_as(a("documents_customfieldinstance"), a("i"))
        .join_as(
            sea_query::JoinType::InnerJoin,
            a("documents_customfield"),
            a("f"),
            Expr::col((a("f"), a("id"))).equals((a("i"), a("field_id"))),
        )
        .and_where(col("i", "document_id").eq(doc_id))
        .order_by((a("i"), a("created")), Order::Asc)
        .order_by((a("i"), a("id")), Order::Asc)
        .to_owned();
    let rows = db::fetch_all(conn, &q).await?;
    rows.iter().map(cf_instance_from_row).collect()
}

/// Columns 0–12 as in [`custom_field_instances`] (`value_text` … `value_long_text`, `f.name`,
/// `f.data_type`, `f.extra_data`) → template value. Shared with the bundled loading
/// ([`crate::bulk`]).
pub(crate) fn cf_instance_from_row(r: &sqlx::postgres::PgRow) -> Result<CustomFieldInstance, sqlx::Error> {
    {
        let data_type: String = r.try_get(11)?;
        let extra: Option<Value> = r.try_get(12)?;
        let value = match data_type.as_str() {
            "string" => r.try_get::<Option<String>, _>(0)?.map(CustomFieldValue::Str),
            "boolean" => r.try_get::<Option<bool>, _>(1)?.map(CustomFieldValue::Bool),
            "url" => r.try_get::<Option<String>, _>(2)?.map(CustomFieldValue::Str),
            "date" => r.try_get::<Option<NaiveDate>, _>(3)?.map(CustomFieldValue::Date),
            "integer" => r.try_get::<Option<i32>, _>(4)?.map(|v| CustomFieldValue::Int(i64::from(v))),
            "float" => r.try_get::<Option<f64>, _>(5)?.map(CustomFieldValue::Float),
            "monetary" => r.try_get::<Option<String>, _>(6)?.map(CustomFieldValue::Str),
            "documentlink" => r
                .try_get::<Option<Value>, _>(7)?
                .and_then(|v| v.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect::<Vec<_>>()))
                .map(CustomFieldValue::IntList),
            "select" => r.try_get::<Option<String>, _>(8)?.map(CustomFieldValue::Str),
            "longtext" => r.try_get::<Option<String>, _>(9)?.map(CustomFieldValue::Str),
            _ => None,
        }
        .unwrap_or(CustomFieldValue::Null);
        let select_options = if data_type == "select" {
            extra.as_ref().and_then(|e| e.get("select_options")).and_then(Value::as_array).map(|opts| {
                opts.iter()
                    .map(|o| SelectOption {
                        id: o.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                        label: o.get("label").and_then(Value::as_str).unwrap_or_default().to_string(),
                    })
                    .collect()
            })
        } else {
            None
        };
        Ok(CustomFieldInstance { field_name: r.try_get(10)?, data_type, select_options, value })
    }
}

pub(crate) fn base_context(d: &DocRow) -> DocumentContext {
    let mut c = DocumentContext::new(d.id, d.title.clone(), d.mime_type.clone());
    c.content = d.content.clone();
    c.page_count = d.page_count;
    c.created = d.created;
    c.added = d.added;
    c.modified = d.modified;
    c.archive_serial_number = d.archive_serial_number;
    c.checksum = d.checksum.clone();
    c.archive_checksum = d.archive_checksum.clone();
    c.filename = d.filename.clone();
    c.archive_filename = d.archive_filename.clone();
    c.original_filename = d.original_filename.clone();
    c.version_index = d.version_index;
    c
}

async fn fill(conn: &mut PgConnection, d: &DocRow, c: &mut DocumentContext) -> Result<(), sqlx::Error> {
    c.correspondent = named(conn, "documents_correspondent", d.correspondent_id).await?;
    c.document_type = named(conn, "documents_documenttype", d.document_type_id).await?;
    if let Some(sp) = d.storage_path_id {
        let q = Query::select()
            .columns([a("name"), a("path")])
            .from(a("documents_storagepath"))
            .and_where(Expr::col(a("id")).eq(sp))
            .to_owned();
        if let Some(r) = db::fetch_optional(conn, &q).await? {
            c.storage_path = Some(StoragePathRef { id: Some(sp), name: r.try_get(0)?, path: r.try_get(1)? });
        }
    }
    if let Some(o) = d.owner_id
        && let Some(username) = db::username(conn, o).await? {
            c.owner = Some(OwnerRef { id: o, username });
        }
    c.tags = db::document_tags(conn, d.id).await?.into_iter().map(|(id, name)| TagRef { id: Some(id), name }).collect();
    c.custom_fields = custom_field_instances(conn, d.id).await?;
    Ok(())
}

/// Context of a saved document (versions with `root_document`).
pub async fn document_context(conn: &mut PgConnection, d: &DocRow) -> Result<DocumentContext, sqlx::Error> {
    let mut c = base_context(d);
    fill(conn, d, &mut c).await?;
    if let Some(root_id) = d.root_document_id
        && let Some(root) = db::load_document(conn, root_id).await? {
            let mut rc = base_context(&root);
            fill(conn, &root, &mut rc).await?;
            c.root_document = Some(Box::new(rc));
        }
    Ok(c)
}
