//! `POST /api/storage_paths/test/`.
//!
//! upstream: src/documents/views.py@v3.4.2 (StoragePathViewSet.test, get_permissions:
//!           `(IsAuthenticated, ViewDocumentsPermissions)`), src/documents/serialisers.py@v3.4.2
//!           (StoragePathTestSerializer: `path` CharField required, `document`
//!           PrimaryKeyRelatedField over `Document.objects.filter(id__in=permitted_document_ids(user))`),
//!           src/documents/file_handling.py@v3.4.2 (format_filename),
//!           src/documents/templating/filepath.py@v3.4.2 (document context).
//!
//! The result itself is computed by `pl_templating::storage_path_test_response` (one rendering, B-N1).

use axum::body::Bytes;
use axum::extract::State;
use chrono::NaiveDate;
use pl_authz::AuthUser;
use pl_authz::perms::VIEW_DOCUMENTS_PERMISSIONS;
use pl_core::{ApiError, AppState, RequestContext};
use pl_templating::{
    CustomFieldInstance, CustomFieldValue, DocumentContext, NamedRef, OwnerRef, SelectOption, StoragePathRef, TagRef,
};
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder as _;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};

use crate::drf::{self, HResult};
use crate::fields::{self, CharOpts, Input, Out, PkCandidate, input};

const DOC_COLS: &str = "id::int8, title, content, page_count::int8, created, added, modified, \
    archive_serial_number::int8, mime_type, checksum, archive_checksum, filename, archive_filename, \
    original_filename, owner_id::int8, correspondent_id::int8, document_type_id::int8, storage_path_id::int8, \
    root_document_id::int8, version_index::int8";

/// `document` field: `PrimaryKeyRelatedField(required=True)` (allow_null=False).
async fn document_field(db: &PgPool, user: &pl_authz::User, inp: Input<'_>) -> Result<Result<i64, Value>, ApiError> {
    match inp {
        Input::Missing => return Ok(Err(json!(["This field is required."]))),
        Input::Null => return Ok(Err(json!(["This field may not be null."]))),
        // RelatedField.validate_empty_values: "" → None → allow_null=False
        Input::Val(Value::String(s)) if s.is_empty() => return Ok(Err(json!(["This field may not be null."]))),
        _ => {}
    }
    match fields::pk_field(inp, false, false) {
        Ok(Out::Set(Some((PkCandidate::Id(id), repr)))) => {
            let found = match id {
                Some(id) => permitted_document_exists(db, user, id).await?,
                None => false,
            };
            match (found, id) {
                (true, Some(id)) => Ok(Ok(id)),
                _ => Ok(Err(json!([format!("Invalid pk \"{repr}\" - object does not exist.")]))),
            }
        }
        Ok(_) => Ok(Err(json!(["This field may not be null."]))),
        Err(e) => Ok(Err(e)),
    }
}

pub async fn post(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> HResult {
    // permission_classes = (IsAuthenticated, ViewDocumentsPermissions)
    let user = auth.require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS).map_err(drf::auth)?.clone();
    let body = drf::parse_json_body(&ctx, &body)?;
    let Value::Object(data) = &body else { return Err(drf::not_a_dict(&body)) };
    let db = state.db();

    // Serializer.to_internal_value: fields in declaration order (path, document)
    let mut errors = Map::new();
    let path = match fields::char_field(
        input(data, "path"),
        false,
        &CharOpts { required: true, allow_blank: false, max_length: usize::MAX },
    ) {
        Ok(Out::Set(p)) => Some(p),
        Ok(Out::Skip) => None,
        Err(e) => {
            errors.insert("path".into(), e);
            None
        }
    };
    let doc_id = match document_field(db, &user, input(data, "document")).await? {
        Ok(id) => Some(id),
        Err(e) => {
            errors.insert("document".into(), e);
            None
        }
    };
    if !errors.is_empty() {
        return Err(drf::validation(Value::Object(errors)));
    }
    let (Some(path), Some(doc_id)) = (path, doc_id) else {
        return Err(ApiError::internal("storage_paths.test: validated data incomplete"));
    };
    let Some(doc) = load_context(db, doc_id).await? else {
        return Err(ApiError::internal("storage_paths.test: document vanished"));
    };
    let cfg = state.config();
    let opts = pl_templating::Options {
        filename_format: cfg.filename_format.clone(),
        filename_format_remove_none: cfg.filename_format_remove_none,
        time_zone: cfg.tz,
    };
    // `if result:` – an empty result stays `""` (without extension)
    // B-N1: render the template only once
    let result = pl_templating::storage_path_test_response(&doc, &path, &opts).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(match result {
        Some(s) => drf::ok(Value::String(s)),
        None => empty_response(),
    })
}

/// DRF `Response(None)`: status 200, empty body, **no** `Content-Type` (the renderer returns
/// `b""` and `Response.rendered_content` then sets no type).
fn empty_response() -> axum::response::Response {
    let mut r = axum::response::Response::new(axum::body::Body::empty());
    r.headers_mut().insert(axum::http::header::CONTENT_LENGTH, axum::http::HeaderValue::from_static("0"));
    r
}

/// `Document.objects.filter(id__in=permitted_document_ids(user)).get(pk=id)` exists?
async fn permitted_document_exists(db: &PgPool, user: &pl_authz::User, id: i64) -> Result<bool, ApiError> {
    let ids = pl_authz::objects::permitted_document_ids(Some(user), "view_document", false);
    let q = Query::select()
        .expr(Expr::val(1))
        .from(Alias::new("documents_document"))
        .and_where(Expr::col(Alias::new("id")).eq(id))
        .and_where(Expr::col(Alias::new("deleted_at")).is_null())
        .and_where(Expr::col(Alias::new("id")).in_subquery(ids))
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    Ok(sqlx::query_with(&sql, values).fetch_optional(db).await?.is_some())
}

struct DocRow {
    ctx: DocumentContext,
    owner: Option<i64>,
    correspondent: Option<i64>,
    document_type: Option<i64>,
    storage_path: Option<i64>,
    root: Option<i64>,
}

async fn load_row(db: &PgPool, id: i64) -> Result<Option<DocRow>, sqlx::Error> {
    let Some(r) = sqlx::query(&format!("SELECT {DOC_COLS} FROM documents_document WHERE id = $1::int8"))
        .bind(id)
        .fetch_optional(db)
        .await?
    else {
        return Ok(None);
    };
    let mut c = DocumentContext::new(r.try_get::<i64, _>(0)?, r.try_get::<String, _>(1)?, r.try_get::<String, _>(8)?);
    c.content = r.try_get(2)?;
    c.page_count = r.try_get(3)?;
    c.created = r.try_get::<NaiveDate, _>(4)?;
    c.added = r.try_get(5)?;
    c.modified = r.try_get(6)?;
    c.archive_serial_number = r.try_get(7)?;
    c.checksum = r.try_get(9)?;
    c.archive_checksum = r.try_get(10)?;
    c.filename = r.try_get(11)?;
    c.archive_filename = r.try_get(12)?;
    c.original_filename = r.try_get(13)?;
    c.version_index = r.try_get(19)?;
    Ok(Some(DocRow {
        ctx: c,
        owner: r.try_get(14)?,
        correspondent: r.try_get(15)?,
        document_type: r.try_get(16)?,
        storage_path: r.try_get(17)?,
        root: r.try_get(18)?,
    }))
}

async fn named(db: &PgPool, table: &str, id: Option<i64>) -> Result<Option<NamedRef>, sqlx::Error> {
    let Some(id) = id else { return Ok(None) };
    let name: Option<String> = sqlx::query_scalar(&format!("SELECT name FROM {table} WHERE id = $1::int8"))
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(name.map(|name| NamedRef { id: Some(id), name }))
}

async fn fill(db: &PgPool, r: DocRow) -> Result<DocumentContext, sqlx::Error> {
    let mut c = r.ctx;
    c.correspondent = named(db, "documents_correspondent", r.correspondent).await?;
    c.document_type = named(db, "documents_documenttype", r.document_type).await?;
    if let Some(sp) = r.storage_path {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT name, path FROM documents_storagepath WHERE id = $1::int8").bind(sp).fetch_optional(db).await?;
        c.storage_path = row.map(|(name, path)| StoragePathRef { id: Some(sp), name, path });
    }
    if let Some(o) = r.owner {
        let u: Option<String> =
            sqlx::query_scalar("SELECT username FROM auth_user WHERE id = $1::int8").bind(o).fetch_optional(db).await?;
        c.owner = u.map(|username| OwnerRef { id: o, username });
    }
    let tags: Vec<(i64, String)> = sqlx::query_as(
        "SELECT t.id::int8, t.name FROM documents_tag t JOIN documents_document_tags dt ON dt.tag_id = t.id \
         WHERE dt.document_id = $1::int8 ORDER BY t.name, t.id",
    )
    .bind(c.pk)
    .fetch_all(db)
    .await?;
    c.tags = tags.into_iter().map(|(id, name)| TagRef { id: Some(id), name }).collect();
    c.custom_fields = custom_field_instances(db, c.pk).await?;
    Ok(c)
}

/// Template context of a stored document (versions with `root_document`), like
/// `pl_consumer::templating::document_context` (pl-objects does not depend on pl-consumer).
pub async fn load_context(db: &PgPool, id: i64) -> Result<Option<DocumentContext>, sqlx::Error> {
    let Some(r) = load_row(db, id).await? else { return Ok(None) };
    let root = r.root;
    let mut c = fill(db, r).await?;
    if let Some(root_id) = root
        && let Some(rr) = load_row(db, root_id).await?
    {
        c.root_document = Some(Box::new(fill(db, rr).await?));
    }
    Ok(Some(c))
}

/// `CustomFieldInstance.global_objects.filter(document=document)` as template values.
async fn custom_field_instances(db: &PgPool, doc_id: i64) -> Result<Vec<CustomFieldInstance>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT i.value_text, i.value_bool, i.value_url, i.value_date, i.value_int, i.value_float, i.value_monetary, \
         i.value_document_ids, i.value_select, i.value_long_text, f.name, f.data_type, f.extra_data \
         FROM documents_customfieldinstance i JOIN documents_customfield f ON f.id = i.field_id \
         WHERE i.document_id = $1::int8 ORDER BY i.created, i.id",
    )
    .bind(doc_id)
    .fetch_all(db)
    .await?;
    let mut out = Vec::new();
    for r in rows {
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
        out.push(CustomFieldInstance { field_name: r.try_get(10)?, data_type, select_options, value });
    }
    Ok(out)
}
