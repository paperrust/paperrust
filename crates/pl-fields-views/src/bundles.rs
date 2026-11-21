//! `/api/share_link_bundles/` (+ `rebuild`)
//!
//! upstream: src/documents/views.py@v3.4.2 (ShareLinkBundleViewSet.create/rebuild),
//! src/documents/serialisers.py@v3.4.2 (ShareLinkBundleSerializer),
//! src/documents/filters.py@v3.4.2 (ShareLinkBundleFilterSet),
//! src/documents/models.py@v3.4.2 (ShareLinkBundle.absolute_file_path/remove_file/delete),
//! src/documents/tasks.py@v3.4.2 (build_share_link_bundle – runs in the Python worker).

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, PathBuf};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use pl_core::AppState;
use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query, SelectStatement};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};

use crate::authz::{check_object_perm, permitted_document_ids_sql, require_model_perm, visible};
use crate::celery::enqueue_build_share_link_bundle;
use crate::fields::{ChoiceOpts, Errors, F, IntOpts, check_int, choice_str, int_field};
use crate::query::{FilterErrors, apply_ordering, datetime_filters, ordering_terms};
use crate::share_links::FILE_VERSIONS;
use crate::shim::{
    ApiError, ApiResult, Ctx, finish, fmt_dt, fmt_dt_opt, get_random_string, json_response, no_content, page_request, paginated,
    parse_body, parse_pk, py_type_name, require_object, resolve_page,
};
use pl_authz::models::{ObjectModel, SHARE_LINK_BUNDLE};

const OM: ObjectModel = SHARE_LINK_BUNDLE;
const LABEL: &str = "ShareLinkBundle";
pub const LIST_ALLOW: &str = "GET, POST, HEAD, OPTIONS";
pub const DETAIL_ALLOW: &str = "GET, DELETE, HEAD, OPTIONS";
pub const REBUILD_ALLOW: &str = "POST, OPTIONS";
pub const STATUSES: &[&str] = &["pending", "processing", "ready", "failed"];

fn a(s: &str) -> Alias {
    Alias::new(s)
}

#[derive(Debug, Clone)]
struct Bundle {
    id: i64,
    created: DateTime<Utc>,
    expiration: Option<DateTime<Utc>>,
    slug: String,
    file_version: String,
    status: String,
    size_bytes: Option<i64>,
    last_error: Value,
    built_at: Option<DateTime<Utc>>,
    file_path: String,
    owner_id: Option<i64>,
    /// `document_total` (annotation incl. deleted documents)
    document_total: i64,
}

const TOTAL_SQL: &str = "(SELECT COUNT(DISTINCT t.document_id) FROM documents_sharelinkbundle_documents t \
     WHERE t.sharelinkbundle_id = b.id)::int8";

fn base(ctx: &Ctx) -> SelectStatement {
    let mut q = Query::select();
    q.from_as(a("documents_sharelinkbundle"), a("b"));
    q.cond_where(visible(ctx, &OM, "b"));
    q
}

fn select_columns(q: &mut SelectStatement) {
    for (e, n) in [
        ("b.id::int8", "id"),
        ("b.created", "created"),
        ("b.expiration", "expiration"),
        ("b.slug", "slug"),
        ("b.file_version", "file_version"),
        ("b.status", "status"),
        ("b.size_bytes::int8", "size_bytes"),
        ("b.last_error", "last_error"),
        ("b.built_at", "built_at"),
        ("b.file_path", "file_path"),
        ("b.owner_id::int8", "owner_id"),
        (TOTAL_SQL, "document_total"),
    ] {
        q.expr_as(Expr::cust(e), a(n));
    }
}

fn from_row(r: &sqlx::postgres::PgRow) -> Result<Bundle, sqlx::Error> {
    Ok(Bundle {
        id: r.try_get("id")?,
        created: r.try_get("created")?,
        expiration: r.try_get("expiration")?,
        slug: r.try_get("slug")?,
        file_version: r.try_get("file_version")?,
        status: r.try_get("status")?,
        size_bytes: r.try_get("size_bytes")?,
        last_error: r.try_get::<Option<sqlx::types::Json<Value>>, _>("last_error")?.map(|j| j.0).unwrap_or(Value::Null),
        built_at: r.try_get("built_at")?,
        file_path: r.try_get("file_path")?,
        owner_id: r.try_get("owner_id")?,
        document_total: r.try_get("document_total")?,
    })
}

/// `documents` (non-deleted documents, `Document.Meta.ordering = -created`).
async fn load_documents(db: &PgPool, ids: &[i64]) -> ApiResult<HashMap<i64, Vec<i64>>> {
    let mut out: HashMap<i64, Vec<i64>> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let rows = sqlx::query(
        "SELECT t.sharelinkbundle_id::int8 AS b, d.id::int8 AS d \
         FROM documents_document d JOIN documents_sharelinkbundle_documents t ON t.document_id = d.id \
         WHERE d.deleted_at IS NULL AND t.sharelinkbundle_id = ANY($1::int8[]) \
         ORDER BY d.created DESC",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    for r in rows {
        out.entry(r.try_get("b")?).or_default().push(r.try_get("d")?);
    }
    Ok(out)
}

fn to_json(b: &Bundle, docs: &[i64], document_count: i64) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(b.id));
    m.insert("created".into(), json!(fmt_dt(&b.created)));
    m.insert("expiration".into(), fmt_dt_opt(&b.expiration));
    m.insert("slug".into(), json!(b.slug));
    m.insert("file_version".into(), json!(b.file_version));
    m.insert("status".into(), json!(b.status));
    m.insert("size_bytes".into(), json!(b.size_bytes));
    m.insert("last_error".into(), b.last_error.clone());
    m.insert("built_at".into(), fmt_dt_opt(&b.built_at));
    m.insert("documents".into(), json!(docs));
    m.insert("document_count".into(), json!(document_count));
    Value::Object(m)
}

/// `get_document_count`: `document_total or documents.count()`
fn doc_count(b: &Bundle, docs: &[i64]) -> i64 {
    if b.document_total != 0 { b.document_total } else { docs.len() as i64 }
}

async fn single_json(db: &PgPool, b: &Bundle, override_total: Option<i64>) -> ApiResult<Value> {
    let docs = load_documents(db, &[b.id]).await?;
    let docs = docs.get(&b.id).cloned().unwrap_or_default();
    let count = match override_total {
        Some(n) if n != 0 => n,
        _ => doc_count(b, &docs),
    };
    Ok(to_json(b, &docs, count))
}

async fn get_object(db: &PgPool, ctx: &Ctx, _method: &Method, pk: &str) -> ApiResult<Bundle> {
    let id = parse_pk(pk)?;
    let mut q = base(ctx);
    select_columns(&mut q);
    q.and_where(Expr::cust_with_values("b.id = $1", [id]));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let Some(row) = sqlx::query_with(&sql, values).fetch_optional(db).await? else {
        return Err(ApiError::not_found(LABEL));
    };
    let b = from_row(&row)?;
    check_object_perm(db, ctx, &OM, b.id, b.owner_id).await?;
    Ok(b)
}

async fn reload(db: &PgPool, id: i64) -> ApiResult<Bundle> {
    let mut q = Query::select();
    q.from_as(a("documents_sharelinkbundle"), a("b"));
    select_columns(&mut q);
    q.and_where(Expr::cust_with_values("b.id = $1", [id]));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_one(db).await?;
    Ok(from_row(&row)?)
}

/// `ShareLinkBundle.absolute_file_path` + `remove_file()` (errors are ignored).
pub(crate) fn remove_file(bundle_dir: &std::path::Path, file_path: &str) {
    if file_path.is_empty() {
        return;
    }
    let rel = PathBuf::from(file_path);
    if rel.is_absolute() || rel.components().any(|c| matches!(c, Component::Prefix(_))) {
        return;
    }
    let Ok(dir) = bundle_dir.canonicalize() else { return };
    let Ok(abs) = dir.join(&rel).canonicalize() else {
        return;
    };
    if !abs.starts_with(&dir) || !abs.is_file() {
        return;
    }
    let _ = std::fs::remove_file(abs);
}

// ---------------------------------------------------------------------------
// list / retrieve
// ---------------------------------------------------------------------------

pub async fn list(State(st): State<AppState>, ctx: Ctx) -> Response {
    finish(LIST_ALLOW, list_inner(&st.inner.db, &ctx).await)
}

async fn list_inner(db: &PgPool, ctx: &Ctx) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let mut q = base(ctx);
    let mut errs = FilterErrors::default();
    datetime_filters(&mut q, ctx, "created", "b.created", &mut errs);
    datetime_filters(&mut q, ctx, "expiration", "b.expiration", &mut errs);
    if let Some(s) = ctx.param("status").filter(|s| !s.is_empty()) {
        if STATUSES.contains(&s) {
            q.and_where(Expr::cust_with_values("b.status = $1", [s.to_string()]));
        } else {
            errs.add("status", &format!("Select a valid choice. {s} is not one of the available choices."));
        }
    }
    errs.into_result()?;
    // filter_documents: invalid number → empty, no IDs → no filter
    if let Some(v) = ctx.param("documents").filter(|v| !v.is_empty()) {
        let parsed: Result<Vec<i64>, _> = v.split(',').filter(|s| !s.is_empty()).map(|s| s.trim().parse::<i64>()).collect();
        match parsed {
            Err(_) => {
                q.and_where(Expr::cust("FALSE"));
            }
            Ok(ids) if ids.is_empty() => {}
            Ok(ids) => {
                let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
                q.and_where(Expr::cust(format!(
                    "b.id IN (SELECT t.sharelinkbundle_id FROM documents_sharelinkbundle_documents t \
                     WHERE t.document_id IN ({list}))"
                )));
            }
        }
    }
    let mut cq = q.clone();
    cq.expr(Expr::cust("COUNT(*)::int8"));
    let (sql, values) = cq.build_sqlx(PostgresQueryBuilder);
    let count: i64 = sqlx::query_scalar_with(&sql, values).fetch_one(db).await?;
    match ordering_terms(ctx, &["created", "expiration", "status"]) {
        Some(t) => apply_ordering(&mut q, &t, |n| format!("b.{n}")),
        None => {
            q.order_by_expr(Expr::cust("b.created"), sea_query::Order::Desc);
        }
    }
    let page = resolve_page(&page_request(ctx), count)?;
    let all_ids = if ctx.version < 10 {
        let mut iq = q.clone();
        iq.expr(Expr::cust("b.id::int8"));
        let (sql, values) = iq.build_sqlx(PostgresQueryBuilder);
        Some(sqlx::query_scalar_with::<_, i64, _>(&sql, values).fetch_all(db).await?)
    } else {
        None
    };
    select_columns(&mut q);
    q.limit(page.page_size).offset(page.offset());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    let bundles: Vec<Bundle> = rows.iter().map(from_row).collect::<Result<_, _>>()?;
    let ids: Vec<i64> = bundles.iter().map(|b| b.id).collect();
    let docs = load_documents(db, &ids).await?;
    let results = bundles
        .iter()
        .map(|b| {
            let d = docs.get(&b.id).cloned().unwrap_or_default();
            let c = doc_count(b, &d);
            to_json(b, &d, c)
        })
        .collect();
    Ok(paginated(ctx, &page, all_ids, results))
}

pub async fn retrieve(State(st): State<AppState>, Path(pk): Path<String>, ctx: Ctx) -> Response {
    finish(DETAIL_ALLOW, retrieve_inner(&st.inner.db, &ctx, &pk).await)
}

async fn retrieve_inner(db: &PgPool, ctx: &Ctx, pk: &str) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let b = get_object(db, ctx, &Method::GET, pk).await?;
    Ok(json_response(StatusCode::OK, &single_json(db, &b, None).await?))
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

/// `document_ids`: ListField(IntegerField(min_value=1), allow_empty=False) + duplicate check.
fn validate_document_ids(data: &Map<String, Value>, errs: &mut Errors) -> Option<Vec<i64>> {
    let v = data.get("__list__document_ids").or_else(|| data.get("document_ids"));
    let Some(v) = v else {
        errs.add("document_ids", crate::fields::REQUIRED);
        return None;
    };
    let items = match v {
        Value::Null => {
            errs.add("document_ids", crate::fields::NULL);
            return None;
        }
        Value::Array(items) => items,
        other => {
            errs.add("document_ids", &format!("Expected a list of items but got type \"{}\".", py_type_name(other)));
            return None;
        }
    };
    if items.is_empty() {
        errs.add("document_ids", "This list may not be empty.");
        return None;
    }
    let mut ids = Vec::new();
    let mut child = Map::new();
    for (i, x) in items.iter().enumerate() {
        let r = if x.is_null() { Err(crate::fields::NULL.to_string()) } else { check_int(x, Some(1), None) };
        match r {
            Ok(n) => ids.push(n),
            Err(m) => {
                child.insert(i.to_string(), json!([m]));
            }
        }
    }
    if !child.is_empty() {
        errs.set("document_ids", Value::Object(child));
        return None;
    }
    let unique: BTreeSet<i64> = ids.iter().copied().collect();
    if unique.len() != ids.len() {
        errs.add("document_ids", "Duplicate document identifiers are not allowed.");
        return None;
    }
    Some(ids)
}

pub async fn create(State(st): State<AppState>, ctx: Ctx, body: Bytes) -> Response {
    finish(LIST_ALLOW, create_inner(&st, &ctx, &body).await)
}

async fn create_inner(st: &AppState, ctx: &Ctx, body: &[u8]) -> ApiResult<Response> {
    let db = &st.inner.db;
    require_model_perm(ctx, &OM)?;
    // get_permissions(): additionally ViewDocumentsPermissions (POST → documents.view_document)
    if !ctx.user.has_perm("documents.view_document") {
        return Err(ApiError::forbidden());
    }
    let data = require_object(parse_body(&ctx.headers, body)?)?;
    let mut errs = Errors::default();
    // Field order: expiration_days, file_version, document_ids
    let expiration_days =
        match int_field(&data, "expiration_days", IntOpts { required: false, allow_null: true, min: Some(1), max: None }, false, &mut errs)
        {
            F::Val(v) => v,
            _ => None,
        };
    let file_version = match choice_str(
        &data,
        "file_version",
        FILE_VERSIONS,
        ChoiceOpts { required: false, allow_null: false, allow_blank: false },
        false,
        &mut errs,
    ) {
        F::Val(Some(f)) => f,
        _ => "archive".to_string(),
    };
    let ids = validate_document_ids(&data, &mut errs);
    errs.into_result()?;
    let Some(ids) = ids else {
        return Err(ApiError::ServerError);
    };

    // View: missing documents (default manager: non-deleted)
    let found: Vec<i64> = sqlx::query_scalar("SELECT id::int8 FROM documents_document WHERE id = ANY($1::int8[]) AND deleted_at IS NULL")
        .bind(&ids)
        .fetch_all(db)
        .await?;
    let found: BTreeSet<i64> = found.into_iter().collect();
    let missing: Vec<String> = ids.iter().copied().collect::<BTreeSet<i64>>().difference(&found).map(|i| i.to_string()).collect();
    if !missing.is_empty() {
        return Err(ApiError::bad_request(json!({
            "document_ids": format!("Documents not found: {}", missing.join(", "))
        })));
    }
    let denied_sql = format!(
        "SELECT id::int8 FROM documents_document WHERE id = ANY($1::int8[]) AND deleted_at IS NULL \
         AND id NOT IN ({}) ORDER BY id LIMIT 1",
        permitted_document_ids_sql(ctx)
    );
    let denied: Option<i64> = sqlx::query_scalar(&denied_sql).bind(&ids).fetch_optional(db).await?;
    if let Some(d) = denied {
        return Err(ApiError::bad_request(json!({
            "document_ids": format!("Insufficient permissions to share document {d}.")
        })));
    }

    let now = Utc::now();
    let expiration = match expiration_days {
        Some(d) if d != 0 => Some(now + chrono::Duration::days(d)),
        _ => None,
    };
    let slug = get_random_string(50);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO documents_sharelinkbundle \
         (created, expiration, slug, owner_id, file_version, status, size_bytes, last_error, file_path, built_at) \
         VALUES ($1, $2, $3, $4::int8, $5, 'pending', NULL, NULL, '', NULL) RETURNING id::int8",
    )
    .bind(now)
    .bind(expiration)
    .bind(&slug)
    .bind(ctx.user.id)
    .bind(&file_version)
    .fetch_one(db)
    .await?;
    sqlx::query(
        "INSERT INTO documents_sharelinkbundle_documents (sharelinkbundle_id, document_id) \
         SELECT $1::int8, d FROM unnest($2::int8[]) d ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(&ids)
    .execute(db)
    .await?;
    // View: remove_file() + reset status fields (file does not exist yet)
    sqlx::query(
        "UPDATE documents_sharelinkbundle SET status = 'pending', last_error = NULL, \
         size_bytes = NULL, built_at = NULL, file_path = '' WHERE id = $1::int8",
    )
    .bind(id)
    .execute(db)
    .await?;
    enqueue_build_share_link_bundle(st, id).await?;
    let b = reload(db, id).await?;
    Ok(json_response(StatusCode::CREATED, &single_json(db, &b, Some(ids.len() as i64)).await?))
}

// ---------------------------------------------------------------------------
// rebuild
// ---------------------------------------------------------------------------

pub async fn rebuild(State(st): State<AppState>, Path(pk): Path<String>, ctx: Ctx) -> Response {
    finish(REBUILD_ALLOW, rebuild_inner(&st, &ctx, &pk).await)
}

async fn rebuild_inner(st: &AppState, ctx: &Ctx, pk: &str) -> ApiResult<Response> {
    let db = &st.inner.db;
    require_model_perm(ctx, &OM)?;
    let b = get_object(db, ctx, &Method::POST, pk).await?;
    if b.status == "processing" {
        return Err(ApiError::detail(StatusCode::BAD_REQUEST, "Bundle is already being processed."));
    }
    remove_file(&ctx.share_link_bundle_dir, &b.file_path);
    sqlx::query(
        "UPDATE documents_sharelinkbundle SET status = 'pending', last_error = NULL, \
         size_bytes = NULL, built_at = NULL, file_path = '' WHERE id = $1::int8",
    )
    .bind(b.id)
    .execute(db)
    .await?;
    enqueue_build_share_link_bundle(st, b.id).await?;
    let nb = Bundle {
        status: "pending".into(),
        last_error: Value::Null,
        size_bytes: None,
        built_at: None,
        file_path: String::new(),
        ..b.clone()
    };
    Ok(json_response(StatusCode::OK, &single_json(db, &nb, None).await?))
}

// ---------------------------------------------------------------------------
// destroy
// ---------------------------------------------------------------------------

pub async fn destroy(State(st): State<AppState>, Path(pk): Path<String>, ctx: Ctx) -> Response {
    finish(DETAIL_ALLOW, destroy_inner(&st.inner.db, &ctx, &pk).await)
}

async fn destroy_inner(db: &PgPool, ctx: &Ctx, pk: &str) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let b = get_object(db, ctx, &Method::DELETE, pk).await?;
    remove_file(&ctx.share_link_bundle_dir, &b.file_path);
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM documents_sharelinkbundle_documents WHERE sharelinkbundle_id = $1::int8").bind(b.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM documents_sharelinkbundle WHERE id = $1::int8").bind(b.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(no_content())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn document_ids_validation() {
        let mut e = Errors::default();
        assert_eq!(validate_document_ids(&m(json!({"document_ids": [1, 2]})), &mut e), Some(vec![1, 2]));
        let mut e = Errors::default();
        validate_document_ids(&m(json!({"document_ids": [1, 1]})), &mut e);
        assert_eq!(e.0["document_ids"], json!(["Duplicate document identifiers are not allowed."]));
        let mut e = Errors::default();
        validate_document_ids(&m(json!({"document_ids": []})), &mut e);
        assert_eq!(e.0["document_ids"], json!(["This list may not be empty."]));
        let mut e = Errors::default();
        validate_document_ids(&m(json!({"document_ids": [0, "x"]})), &mut e);
        assert_eq!(
            e.0["document_ids"],
            json!({"0": ["Ensure this value is greater than or equal to 1."], "1": ["A valid integer is required."]})
        );
        let mut e = Errors::default();
        validate_document_ids(&m(json!({})), &mut e);
        assert_eq!(e.0["document_ids"], json!(["This field is required."]));
    }

    #[test]
    fn remove_file_rejects_absolute() {
        // must not do anything and must not panic
        let d = std::path::Path::new("does-not-exist");
        remove_file(d, "/etc/passwd");
        remove_file(d, "");
        remove_file(d, "../../x");
    }
}
