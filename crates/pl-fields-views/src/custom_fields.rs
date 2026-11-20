//! `/api/custom_fields/`
//!
//! upstream: src/documents/views.py@v3.4.2 (CustomFieldViewSet, PermissionsAwareDocumentCountMixin),
//! src/documents/serialisers.py@v3.4.2 (CustomFieldSerializer),
//! src/documents/filters.py@v3.4.2 (CustomFieldFilterSet),
//! src/documents/signals/handlers.py@v3.4.2 (check_paths_and_prune_custom_fields,
//! cleanup_custom_field_deletion), src/documents/models.py@v3.4.2 (CustomField).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use chrono::{DateTime, Utc};
use pl_core::AppState;
use sea_query::{Alias, Expr, JoinType, PostgresQueryBuilder, Query, SelectStatement};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};

use crate::audit::{self, CfState, CfiState};
use crate::authz::require_model_perm;
use pl_authz::models::{CUSTOM_FIELD, ObjectModel};
use crate::fields::{CharOpts, ChoiceOpts, Errors, F, char_field, choice_str, json_field};
use crate::query::{FilterErrors, apply_ordering, char_filters, id_filters, ordering_terms};
use crate::shim::{
    ApiError, ApiResult, Ctx, finish, get_random_string, json_response, no_content, page_request,
    paginated, parse_body, parse_pk, require_object, resolve_page,
};

pub const MODEL: &str = "customfield";
const OM: ObjectModel = CUSTOM_FIELD;
const LABEL: &str = "CustomField";
pub const LIST_ALLOW: &str = "GET, POST, HEAD, OPTIONS";
pub const DETAIL_ALLOW: &str = "GET, PUT, PATCH, DELETE, HEAD, OPTIONS";

pub const DATA_TYPES: &[&str] = &[
    "string",
    "url",
    "date",
    "boolean",
    "integer",
    "float",
    "monetary",
    "documentlink",
    "select",
    "longtext",
];

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// `document_count` per field (annotate_document_count_for_related_queryset over
/// `CustomFieldInstance.objects`, `document_id IN permitted_document_ids(user)`), grouped by
/// `field_id`. PERF-SCALE #3: `COUNT(*)` instead of `COUNT(DISTINCT document_id)` – `(document_id,
/// field_id)` is UNIQUE and the join on `documents_document` is 1:1 (primary key); visibility
/// as a row condition without self-JOIN. `ids`: count only these fields (displayed page).
/// With `counters` (data model stage 3, [`crate::cf_counters::available`]) from the
/// label counters (same values, columns `field_id`, `c`).
pub fn count_select_ext(ctx: &Ctx, ids: Option<&[i64]>, counters: bool) -> SelectStatement {
    count_select_for(ctx.authz_user(), ids, counters)
}

/// [`count_select_ext`] for a user (without request context).
pub fn count_select_for(user: &pl_authz::User, ids: Option<&[i64]>, counters: bool) -> SelectStatement {
    if counters && let Some(q) = crate::cf_counters::count_source(user, ids) {
        return q;
    }
    let mut cnt = Query::select();
    cnt.expr_as(Expr::col((a("j"), a("field_id"))), a("field_id"))
        .expr_as(Expr::cust("COUNT(*)"), a("c"))
        .from_as(a("documents_customfieldinstance"), a("j"))
        .join_as(
            JoinType::InnerJoin,
            a("documents_document"),
            a("cd"),
            Expr::col((a("cd"), a("id"))).equals((a("j"), a("document_id"))),
        )
        .and_where(Expr::col((a("j"), a("deleted_at"))).is_null())
        .cond_where(crate::authz::permitted_doc_row_for(user, "cd"))
        .group_by_col((a("j"), a("field_id")));
    if let Some(ids) = ids {
        cnt.and_where(Expr::col((a("j"), a("field_id"))).is_in(ids.iter().copied()));
    }
    cnt
}

/// Base query; with `with_count` including the `document_count` annotation (LEFT JOIN of the count).
fn base_select(ctx: &Ctx, with_count: bool, count_ids: Option<&[i64]>, counters: bool) -> SelectStatement {
    let mut q = Query::select();
    q.from_as(a("documents_customfield"), a("cf"));
    if with_count {
        q.join_subquery(
            JoinType::LeftJoin,
            count_select_ext(ctx, count_ids, counters),
            a("cnt"),
            Expr::col((a("cnt"), a("field_id"))).equals((a("cf"), a("id"))),
        );
    }
    q
}

fn select_columns(q: &mut SelectStatement, with_count: bool) {
    q.expr_as(Expr::cust("cf.id::int8"), a("id"))
        .expr_as(Expr::cust("cf.created"), a("created"))
        .expr_as(Expr::cust("cf.name"), a("name"))
        .expr_as(Expr::cust("cf.data_type"), a("data_type"))
        .expr_as(Expr::cust("cf.extra_data"), a("extra_data"))
        .expr_as(
            Expr::cust(if with_count { "COALESCE(cnt.c, 0)::int8" } else { "0::int8" }),
            a("document_count"),
        );
}

/// `document_count` of the fields `ids` (no hit → missing → 0).
async fn counts_for(db: &PgPool, ctx: &Ctx, ids: &[i64]) -> ApiResult<std::collections::HashMap<i64, i64>> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let counters = crate::cf_counters::available(db).await;
    let q = count_select_ext(ctx, Some(ids), counters);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let sql = format!("SELECT pl_cq.field_id::int8, pl_cq.c::int8 FROM ({sql}) pl_cq");
    let rows: Vec<(i64, i64)> = sqlx::query_as_with(&sql, values).fetch_all(db).await?;
    Ok(rows.into_iter().collect())
}

#[derive(Clone, Debug)]
struct Row_ {
    id: i64,
    created: DateTime<Utc>,
    name: String,
    data_type: String,
    extra_data: Value,
    document_count: i64,
}

fn from_row(r: &sqlx::postgres::PgRow) -> Result<Row_, sqlx::Error> {
    Ok(Row_ {
        id: r.try_get("id")?,
        created: r.try_get("created")?,
        name: r.try_get("name")?,
        data_type: r.try_get("data_type")?,
        extra_data: r
            .try_get::<Option<sqlx::types::Json<Value>>, _>("extra_data")?
            .map(|j| j.0)
            .unwrap_or(Value::Null),
        document_count: r.try_get("document_count")?,
    })
}

fn to_json(id: i64, name: &str, dt: &str, extra: &Value, count: Option<i64>) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(id));
    m.insert("name".into(), json!(name));
    m.insert("data_type".into(), json!(dt));
    m.insert("extra_data".into(), extra.clone());
    if let Some(c) = count {
        m.insert("document_count".into(), json!(c));
    }
    Value::Object(m)
}

async fn load(db: &PgPool, ctx: &Ctx, id: i64) -> ApiResult<Row_> {
    let counters = crate::cf_counters::available(db).await;
    let mut q = base_select(ctx, true, Some(&[id]), counters);
    select_columns(&mut q, true);
    q.and_where(Expr::cust_with_values("cf.id = $1", [id]));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(db).await?;
    match row {
        Some(r) => Ok(from_row(&r)?),
        None => Err(ApiError::not_found(LABEL)),
    }
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

pub async fn list(State(st): State<AppState>, ctx: Ctx) -> Response {
    finish(LIST_ALLOW, list_inner(&st.inner.db, &ctx).await)
}

async fn list_inner(db: &PgPool, ctx: &Ctx) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    // Sorting (DRF default: all readable serializer fields)
    let valid = ["id", "name", "data_type", "extra_data", "document_count"];
    let terms = ordering_terms(ctx, &valid);
    // PERF-SCALE #3b: count only in the list query when sorting by it; otherwise only for
    // the fields of the page (same values)
    let in_query = terms.as_ref().is_some_and(|ts| ts.iter().any(|(n, _)| n == "document_count"));
    let counters = in_query && crate::cf_counters::available(db).await;
    let mut q = base_select(ctx, in_query, None, counters);
    let mut errs = FilterErrors::default();
    id_filters(&mut q, ctx, "cf.id", &mut errs);
    char_filters(&mut q, ctx, "name", "cf.name");
    errs.into_result()?;

    // Counting
    let mut count_q = q.clone();
    count_q.expr(Expr::cust("1"));
    let mut cq = Query::select();
    cq.expr(Expr::cust("COUNT(*)::int8"))
        .from_subquery(count_q, a("sub"));
    let (sql, values) = cq.build_sqlx(PostgresQueryBuilder);
    let count: i64 = sqlx::query_scalar_with(&sql, values).fetch_one(db).await?;

    match terms {
        Some(terms) => apply_ordering(&mut q, &terms, |n| match n {
            "document_count" => "COALESCE(cnt.c, 0)".into(),
            other => format!("cf.{other}"),
        }),
        None => {
            q.order_by_expr(Expr::cust("cf.name"), sea_query::Order::Asc);
        }
    }

    let page = resolve_page(&page_request(ctx), count)?;

    let all_ids = if ctx.version < 10 {
        let mut iq = q.clone();
        iq.expr_as(Expr::cust("cf.id::int8"), a("id"));
        let (sql, values) = iq.build_sqlx(PostgresQueryBuilder);
        Some(sqlx::query_scalar_with::<_, i64, _>(&sql, values).fetch_all(db).await?)
    } else {
        None
    };

    select_columns(&mut q, in_query);
    q.limit(page.page_size).offset(page.offset());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    let mut rows = rows.iter().map(from_row).collect::<Result<Vec<_>, _>>()?;
    if !in_query {
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        let counts = counts_for(db, ctx, &ids).await?;
        for r in &mut rows {
            r.document_count = counts.get(&r.id).copied().unwrap_or(0);
        }
    }
    let mut results = Vec::with_capacity(rows.len());
    for r in rows {
        results.push(to_json(
            r.id,
            &r.name,
            &r.data_type,
            &r.extra_data,
            Some(r.document_count),
        ));
    }
    Ok(paginated(ctx, &page, all_ids, results))
}

// ---------------------------------------------------------------------------
// retrieve
// ---------------------------------------------------------------------------

pub async fn retrieve(State(st): State<AppState>, Path(pk): Path<String>, ctx: Ctx) -> Response {
    finish(DETAIL_ALLOW, retrieve_inner(&st.inner.db, &ctx, &pk).await)
}

async fn retrieve_inner(db: &PgPool, ctx: &Ctx, pk: &str) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let id = parse_pk(pk)?;
    let r = load(db, ctx, id).await?;
    Ok(json_response(
        StatusCode::OK,
        &to_json(r.id, &r.name, &r.data_type, &r.extra_data, Some(r.document_count)),
    ))
}

// ---------------------------------------------------------------------------
// Validation (CustomFieldSerializer)
// ---------------------------------------------------------------------------

/// Validated attributes (only existing fields).
#[derive(Debug, Default)]
pub struct Attrs {
    pub name: Option<String>,
    pub data_type: Option<String>,
    /// `Some(v)` if `extra_data` is in the request (v can be `null`)
    pub extra_data: Option<Value>,
}

/// Result of the `validate()` check of `extra_data`.
#[derive(Debug, PartialEq)]
pub enum Check {
    Ok,
    Invalid(&'static str),
    /// Python type error → 500
    Crash,
}

const SELECT_ERR: &str = "extra_data.select_options must be a valid list";
const CURRENCY_ERR: &str = "extra_data.default_currency must be a 3-character string";

/// Python `"key" in value` for a JSON value; `None` = TypeError.
fn py_contains(container: &Value, key: &str) -> Option<bool> {
    match container {
        Value::Object(m) => Some(m.contains_key(key)),
        Value::Array(a) => Some(a.iter().any(|x| x.as_str() == Some(key))),
        Value::String(s) => Some(s.contains(key)),
        _ => None,
    }
}

/// Python `len(x)`; `None` = TypeError.
fn py_len(v: &Value) -> Option<usize> {
    match v {
        Value::String(s) => Some(s.chars().count()),
        Value::Array(a) => Some(a.len()),
        Value::Object(m) => Some(m.len()),
        _ => None,
    }
}

/// Select check; assigns missing option IDs (mutates `extra`).
pub fn check_select(extra: Option<&mut Value>) -> Check {
    let Some(extra) = extra else {
        return Check::Invalid(SELECT_ERR);
    };
    match py_contains(extra, "select_options") {
        None => return Check::Crash,
        Some(false) => return Check::Invalid(SELECT_ERR),
        Some(true) => {}
    }
    let Value::Object(m) = extra else {
        // list["select_options"] / str["select_options"] → TypeError
        return Check::Crash;
    };
    let Some(Value::Array(opts)) = m.get_mut("select_options") else {
        return Check::Invalid(SELECT_ERR);
    };
    if opts.is_empty() {
        return Check::Invalid(SELECT_ERR);
    }
    // all(len(option.get("label", "")) > 0 ...) – stops at the first False
    for o in opts.iter() {
        let Value::Object(om) = o else {
            return Check::Crash;
        };
        let len = match om.get("label") {
            None => Some(0),
            Some(l) => py_len(l),
        };
        match len {
            None => return Check::Crash,
            Some(0) => return Check::Invalid(SELECT_ERR),
            Some(_) => {}
        }
    }
    for o in opts.iter_mut() {
        if let Value::Object(om) = o
            && om.get("id").is_none_or(Value::is_null) {
                om.insert("id".into(), Value::String(get_random_string(16)));
            }
    }
    Check::Ok
}

pub fn check_currency(extra: &Value) -> Check {
    match py_contains(extra, "default_currency") {
        None => return Check::Crash,
        Some(false) => return Check::Ok,
        Some(true) => {}
    }
    let Value::Object(m) = extra else {
        return Check::Crash;
    };
    match m.get("default_currency") {
        None | Some(Value::Null) => Check::Ok,
        Some(Value::String(s)) => {
            let n = s.chars().count();
            if n > 0 && n != 3 {
                Check::Invalid(CURRENCY_ERR)
            } else {
                Check::Ok
            }
        }
        Some(_) => Check::Invalid(CURRENCY_ERR),
    }
}

async fn validate(
    db: &PgPool,
    data: &Map<String, Value>,
    instance: Option<&Row_>,
    partial: bool,
) -> ApiResult<Attrs> {
    let mut errs = Errors::default();
    let name = char_field(
        data,
        "name",
        CharOpts {
            required: true,
            allow_null: false,
            allow_blank: false,
            max_length: Some(128),
        },
        partial,
        &mut errs,
    );
    let name = match name {
        F::Val(Some(n)) => {
            // UniqueValidator(queryset=CustomField.objects) excluding the instance itself
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM documents_customfield WHERE name = $1 AND id <> $2)",
            )
            .bind(&n)
            .bind(instance.map(|i| i.id).unwrap_or(-1) as i32)
            .fetch_one(db)
            .await?;
            if exists {
                errs.add("name", "custom field with this name already exists.");
                None
            } else {
                Some(n)
            }
        }
        _ => None,
    };
    let data_type = match choice_str(
        data,
        "data_type",
        DATA_TYPES,
        ChoiceOpts {
            required: true,
            allow_null: false,
            allow_blank: false,
        },
        partial,
        &mut errs,
    ) {
        F::Val(Some(d)) => Some(d),
        _ => None,
    };
    let extra_data = match json_field(data, "extra_data", true, partial, &mut errs) {
        F::Val(v) => Some(v),
        _ => None,
    };
    if !errs.is_empty() {
        errs.into_result()?;
    }
    let mut attrs = Attrs {
        name,
        data_type,
        extra_data,
    };

    // validate()
    let is_select = attrs.data_type.as_deref() == Some("select")
        || instance.is_some_and(|i| i.data_type == "select");
    if is_select {
        match check_select(attrs.extra_data.as_mut()) {
            Check::Ok => {}
            Check::Invalid(m) => return Err(ApiError::bad_request(json!({"error": [m]}))),
            Check::Crash => return Err(ApiError::ServerError),
        }
    } else if attrs.data_type.as_deref() == Some("monetary")
        && let Some(extra) = &attrs.extra_data {
            match check_currency(extra) {
                Check::Ok => {}
                Check::Invalid(m) => return Err(ApiError::bad_request(json!({"error": [m]}))),
                Check::Crash => return Err(ApiError::ServerError),
            }
        }
    Ok(attrs)
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

pub async fn create(State(st): State<AppState>, ctx: Ctx, body: Bytes) -> Response {
    finish(LIST_ALLOW, create_inner(&st.inner.db, &ctx, &body).await)
}

async fn create_inner(db: &PgPool, ctx: &Ctx, body: &[u8]) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let data = require_object(parse_body(&ctx.headers, body)?)?;
    let attrs = validate(db, &data, None, false).await?;
    let (Some(name), Some(data_type)) = (attrs.name, attrs.data_type) else {
        return Err(ApiError::ServerError);
    };
    let extra = attrs.extra_data.unwrap_or(Value::Null);
    let now = Utc::now();
    let mut tx = db.begin().await?;
    let mut iq = Query::insert();
    iq.into_table(a("documents_customfield"))
        .columns([a("created"), a("name"), a("data_type"), a("extra_data")])
        .values_panic([
            now.into(),
            name.clone().into(),
            data_type.clone().into(),
            if extra.is_null() {
                sea_query::Value::Json(None).into()
            } else {
                extra.clone().into()
            },
        ])
        .returning(Query::returning().expr(Expr::cust("id::int8")));
    let (sql, values) = iq.build_sqlx(PostgresQueryBuilder);
    let id: i64 = sqlx::query_scalar_with(&sql, values)
        .fetch_one(&mut *tx)
        .await?;
    let state = CfState {
        id,
        created: now,
        name: name.clone(),
        data_type: data_type.clone(),
        extra_data: extra.clone(),
    };
    audit::log(
        &mut *tx,
        ctx,
        MODEL,
        id,
        &state.repr(),
        audit::ACTION_CREATE,
        &audit::cf_create_changes(&state),
    )
    .await?;
    tx.commit().await?;
    // post_save: new fields have no instances → no task.
    Ok(json_response(
        StatusCode::CREATED,
        &to_json(id, &name, &data_type, &extra, None),
    ))
}

// ---------------------------------------------------------------------------
// update / partial_update
// ---------------------------------------------------------------------------

pub async fn update(
    State(st): State<AppState>,
    Path(pk): Path<String>,
    ctx: Ctx,
    body: Bytes,
) -> Response {
    finish(DETAIL_ALLOW, update_inner(&st, &ctx, &pk, &body, false).await)
}

pub async fn partial_update(
    State(st): State<AppState>,
    Path(pk): Path<String>,
    ctx: Ctx,
    body: Bytes,
) -> Response {
    finish(DETAIL_ALLOW, update_inner(&st, &ctx, &pk, &body, true).await)
}

async fn update_inner(
    st: &AppState,
    ctx: &Ctx,
    pk: &str,
    body: &[u8],
    partial: bool,
) -> ApiResult<Response> {
    let db = &st.inner.db;
    require_model_perm(ctx, &OM)?;
    let id = parse_pk(pk)?;
    let inst = load(db, ctx, id).await?;
    let data = require_object(parse_body(&ctx.headers, body)?)?;
    let attrs = validate(db, &data, Some(&inst), partial).await?;

    let new_name = attrs.name.clone().unwrap_or_else(|| inst.name.clone());
    let new_type = attrs
        .data_type
        .clone()
        .unwrap_or_else(|| inst.data_type.clone());
    let new_extra = attrs
        .extra_data
        .clone()
        .unwrap_or_else(|| inst.extra_data.clone());

    // post_save → check_paths_and_prune_custom_fields: process_cf_select_update(custom_field).
    // Upstream pickles the instance (Celery); here the task goes to the Rust worker with `custom_field_id`
    // (docs/STANDALONE-GAPS.md §4.3).
    let mut send_cf_select_update = false;
    if new_type == "select" && truthy(&new_extra) {
        let has_instances: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM documents_customfieldinstance \
             WHERE field_id = $1 AND deleted_at IS NULL)",
        )
        .bind(id as i32)
        .fetch_one(db)
        .await?;
        if has_instances {
            send_cf_select_update = true;
        }
    }

    let mut tx = db.begin().await?;
    // auditlog log_update: old state from the DB (pre_save)
    let old = CfState {
        id,
        created: inst.created,
        name: inst.name.clone(),
        data_type: inst.data_type.clone(),
        extra_data: inst.extra_data.clone(),
    };
    let mut uq = Query::update();
    uq.table(a("documents_customfield"))
        .values([
            (a("name"), new_name.clone().into()),
            (a("data_type"), new_type.clone().into()),
            (
                a("extra_data"),
                if new_extra.is_null() {
                    sea_query::Value::Json(None).into()
                } else {
                    new_extra.clone().into()
                },
            ),
        ])
        .and_where(Expr::col(a("id")).eq(id));
    let (sql, values) = uq.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *tx).await?;
    let new = CfState {
        id,
        created: inst.created,
        name: new_name.clone(),
        data_type: new_type.clone(),
        extra_data: new_extra.clone(),
    };
    audit::log(
        &mut *tx,
        ctx,
        MODEL,
        id,
        &new.repr(),
        audit::ACTION_UPDATE,
        &audit::cf_update_changes(&old, &new),
    )
    .await?;
    tx.commit().await?;
    if send_cf_select_update {
        let req = pl_core::tasks::TaskRequest::new(crate::celery::PROCESS_CF_SELECT_UPDATE).kwarg("custom_field_id", id);
        if let Err(e) = pl_core::tasks::send_task(st, req).await {
            tracing::error!(target: "paperless.handlers", "process_cf_select_update not queued: {e:?}");
        }
    }
    Ok(json_response(
        StatusCode::OK,
        &to_json(id, &new_name, &new_type, &new_extra, Some(inst.document_count)),
    ))
}

// ---------------------------------------------------------------------------
// destroy
// ---------------------------------------------------------------------------

pub async fn destroy(State(st): State<AppState>, Path(pk): Path<String>, ctx: Ctx) -> Response {
    finish(DETAIL_ALLOW, destroy_inner(&st.inner.db, &ctx, &pk).await)
}

/// Python `str(value)` of a CFI value according to `value_for_search`.
fn value_for_search(cf: &Row_, raw: &Option<String>) -> String {
    let Some(v) = raw else {
        return "None".into();
    };
    if cf.data_type == "select" {
        let label = cf
            .extra_data
            .get("select_options")
            .and_then(Value::as_array)
            .and_then(|opts| {
                opts.iter().find_map(|o| {
                    (o.get("id").and_then(Value::as_str) == Some(v.as_str()))
                        .then(|| o.get("label").map(crate::fields::py_str))
                        .flatten()
                })
            });
        return label.unwrap_or_else(|| "None".into());
    }
    v.clone()
}

/// Instances of a field to be deleted, in the order of the Django collector (pk descending).
const CFI_DELETE_SQL: &str = "SELECT id::int8 AS id, created, deleted_at, restored_at, transaction_id::text AS transaction_id, \
                document_id::int8 AS document_id, \
                value_text, \
                CASE WHEN value_bool IS NULL THEN NULL WHEN value_bool THEN 'True' ELSE 'False' END AS value_bool, \
                value_url, to_char(value_date, 'YYYY-MM-DD') AS value_date, value_int::text AS value_int, \
                value_float, value_monetary, value_monetary_amount::text AS value_monetary_amount, \
                value_document_ids, value_select, value_long_text \
         FROM documents_customfieldinstance WHERE field_id = $1 ORDER BY id DESC";

async fn destroy_inner(db: &PgPool, ctx: &Ctx, pk: &str) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let id = parse_pk(pk)?;
    let cf = load(db, ctx, id).await?;
    let id32 = id as i32;

    let mut tx = db.begin().await?;
    // Collector: load instances (base manager, including soft-deleted) for the audit entries.
    // Order of the post_delete signals (= audit entries): Collector.sort() sorts by pk,
    // Collector.delete() reverses the list -> pk descending (django/db/models/deletion.py;
    // W2 parity run 2026-11-08, cf.destroy.state). See CFI_DELETE_SQL.
    let cfis = sqlx::query(CFI_DELETE_SQL)
    .bind(id32)
    .fetch_all(&mut *tx)
    .await?;

    // Fast deletes: M2M through tables of the WorkflowAction
    for t in [
        "documents_workflowaction_assign_custom_fields",
        "documents_workflowaction_remove_custom_fields",
    ] {
        sqlx::query(&format!("DELETE FROM {t} WHERE customfield_id = $1"))
            .bind(id32)
            .execute(&mut *tx)
            .await?;
    }
    // SET_NULL
    sqlx::query(
        "UPDATE documents_workflowtrigger SET schedule_date_custom_field_id = NULL \
         WHERE schedule_date_custom_field_id = $1",
    )
    .bind(id32)
    .execute(&mut *tx)
    .await?;
    // Hard-delete instances (+ post_delete auditlog per instance)
    sqlx::query("DELETE FROM documents_customfieldinstance WHERE field_id = $1")
        .bind(id32)
        .execute(&mut *tx)
        .await?;
    for r in &cfis {
        let value_text: Option<String> = r.try_get("value_text")?;
        let value_bool: Option<String> = r.try_get("value_bool")?;
        let value_url: Option<String> = r.try_get("value_url")?;
        let value_date: Option<String> = r.try_get("value_date")?;
        let value_int: Option<String> = r.try_get("value_int")?;
        let value_float: Option<f64> = r.try_get("value_float")?;
        let value_monetary: Option<String> = r.try_get("value_monetary")?;
        let value_monetary_amount: Option<String> = r.try_get("value_monetary_amount")?;
        let value_select: Option<String> = r.try_get("value_select")?;
        let value_long_text: Option<String> = r.try_get("value_long_text")?;
        let value_document_ids: Value = r
            .try_get::<Option<sqlx::types::Json<Value>>, _>("value_document_ids")?
            .map(|j| j.0)
            .unwrap_or(Value::Null);
        let value_float_s = value_float.map(crate::fields::py_float);
        let doc_ids_s = if value_document_ids.is_null() {
            None
        } else {
            Some(crate::fields::py_repr(&value_document_ids))
        };
        // CustomFieldInstance.value by data_type
        let current: Option<String> = match cf.data_type.as_str() {
            "string" => value_text.clone(),
            "url" => value_url.clone(),
            "date" => value_date.clone(),
            "boolean" => value_bool.clone(),
            "integer" => value_int.clone(),
            "float" => value_float_s.clone(),
            "monetary" => value_monetary.clone(),
            "documentlink" => doc_ids_s.clone(),
            "select" => value_select.clone(),
            "longtext" => value_long_text.clone(),
            _ => None,
        };
        let repr = format!("{} : {}", cf.name, value_for_search(&cf, &current));
        let state = CfiState {
            id: r.try_get("id")?,
            created: r.try_get("created")?,
            deleted_at: r.try_get("deleted_at")?,
            restored_at: r.try_get("restored_at")?,
            transaction_id: r.try_get("transaction_id")?,
            document_id: r.try_get("document_id")?,
            field_id: id,
            plain_values: vec![
                ("value_text", value_text),
                ("value_bool", value_bool),
                ("value_url", value_url),
                ("value_date", value_date),
                ("value_int", value_int),
                ("value_float", value_float_s),
                ("value_monetary", value_monetary),
                ("value_monetary_amount", value_monetary_amount),
                ("value_select", value_select),
                ("value_long_text", value_long_text),
            ],
            value_document_ids,
        };
        audit::log(
            &mut *tx,
            ctx,
            "customfieldinstance",
            state.id,
            &repr,
            audit::ACTION_DELETE,
            &audit::cfi_delete_changes(&state),
        )
        .await?;
    }
    sqlx::query("DELETE FROM documents_customfield WHERE id = $1")
        .bind(id32)
        .execute(&mut *tx)
        .await?;
    let state = CfState {
        id,
        created: cf.created,
        name: cf.name.clone(),
        data_type: cf.data_type.clone(),
        extra_data: cf.extra_data.clone(),
    };
    audit::log(
        &mut *tx,
        ctx,
        MODEL,
        id,
        &state.repr(),
        audit::ACTION_DELETE,
        &audit::cf_delete_changes(&state),
    )
    .await?;

    // post_delete: cleanup_custom_field_deletion
    let ident = format!("custom_field_{id}");
    let views = sqlx::query(
        "SELECT id, display_fields FROM documents_savedview WHERE display_fields IS NOT NULL",
    )
    .fetch_all(&mut *tx)
    .await?;
    for v in views {
        let vid: i32 = v.try_get("id")?;
        let df: Value = v
            .try_get::<Option<sqlx::types::Json<Value>>, _>("display_fields")?
            .map(|j| j.0)
            .unwrap_or(Value::Null);
        match df {
            Value::Array(mut arr) => {
                if let Some(pos) = arr.iter().position(|x| x.as_str() == Some(ident.as_str())) {
                    arr.remove(pos);
                    sqlx::query("UPDATE documents_savedview SET display_fields = $1 WHERE id = $2")
                        .bind(sqlx::types::Json(Value::Array(arr)))
                        .bind(vid)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            // `in` matches, but `.remove` does not exist → AttributeError → rollback/500
            Value::Object(ref m) if m.contains_key(&ident) => return Err(ApiError::ServerError),
            Value::String(ref s) if s.contains(&ident) => return Err(ApiError::ServerError),
            // Number/bool: `in` → TypeError
            Value::Number(_) | Value::Bool(_) => return Err(ApiError::ServerError),
            _ => {}
        }
    }
    sqlx::query("UPDATE documents_savedview SET sort_field = 'created' WHERE sort_field = $1")
        .bind(&ident)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(no_content())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W2 parity run 2026-11-08 (`w2.cf.destroy.state`): Django sends post_delete (audit log)
    /// of the instances in pk-descending order (Collector.sort + reverse).
    #[test]
    fn cfi_delete_order_like_collector() {
        assert!(CFI_DELETE_SQL.trim_end().ends_with("WHERE field_id = $1 ORDER BY id DESC"));
    }

    #[test]
    fn select_validation() {
        let mut v = json!({"select_options": [{"label": "A"}, {"label": "B", "id": "x"}]});
        assert_eq!(check_select(Some(&mut v)), Check::Ok);
        let opts = v["select_options"].as_array().cloned().unwrap_or_default();
        assert_eq!(opts[0]["id"].as_str().map(str::len), Some(16));
        assert_eq!(opts[1]["id"], "x");
        // Input order is kept: label before id
        assert_eq!(
            opts[0].as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()),
            Some(vec!["label".to_string(), "id".to_string()])
        );
        assert_eq!(check_select(None), Check::Invalid(SELECT_ERR));
        assert_eq!(check_select(Some(&mut json!({"select_options": "x"}))), Check::Invalid(SELECT_ERR));
        assert_eq!(check_select(Some(&mut json!({"select_options": []}))), Check::Invalid(SELECT_ERR));
        assert_eq!(check_select(Some(&mut json!({"select_options": [{"label": ""}, 5]}))), Check::Invalid(SELECT_ERR));
        assert_eq!(check_select(Some(&mut json!({"select_options": [5]}))), Check::Crash);
        assert_eq!(check_select(Some(&mut Value::Null)), Check::Crash);
        assert_eq!(check_select(Some(&mut json!({}))), Check::Invalid(SELECT_ERR));
        assert_eq!(check_select(Some(&mut json!("abc"))), Check::Invalid(SELECT_ERR));
        assert_eq!(check_select(Some(&mut json!("select_options"))), Check::Crash);
        assert_eq!(check_select(Some(&mut json!({"select_options": [{"label": 3}]}))), Check::Crash);
    }

    #[test]
    fn currency_validation() {
        assert_eq!(check_currency(&json!({"default_currency": "EUR"})), Check::Ok);
        assert_eq!(check_currency(&json!({"default_currency": ""})), Check::Ok);
        assert_eq!(check_currency(&json!({"default_currency": null})), Check::Ok);
        assert_eq!(check_currency(&json!({"default_currency": "EU"})), Check::Invalid(CURRENCY_ERR));
        assert_eq!(check_currency(&json!({"default_currency": 123})), Check::Invalid(CURRENCY_ERR));
        assert_eq!(check_currency(&json!({})), Check::Ok);
        assert_eq!(check_currency(&Value::Null), Check::Crash);
    }

    #[test]
    fn truthiness() {
        assert!(!truthy(&json!({})));
        assert!(truthy(&json!({"a": 1})));
        assert!(!truthy(&Value::Null));
    }
}
