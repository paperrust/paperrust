//! `/api/tasks/` – `TasksViewSet`.
//!
//! upstream: src/documents/views.py@v3.4.2 (`TasksViewSet`: `list`, `retrieve`, `acknowledge`,
//! `summary`, `status_counts`, `active`, `run`), src/documents/permissions.py@v3.4.2
//! (`PaperlessObjectPermissions`, `AcknowledgeTasksPermissions`, `has_system_status_permission`).

pub mod body;
pub mod query;
pub mod run;
pub mod serialize;

use std::collections::HashMap;

use axum::extract::{Path as UrlPath, State};
use axum::response::{IntoResponse, Response};
use chrono::{Duration, Utc};
use chrono_tz::Tz;
use pl_authz::models::{DOCUMENT, PAPERLESS_TASK};
use pl_authz::perms::{self, ACKNOWLEDGE_TASKS_PERMISSIONS, ObjRef};
use pl_authz::{AuthUser, User, objects};
use pl_core::pagination::{PageRequest, fetch_page, paginated_body};
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{ApiError, AppState, Drf, QueryParams, RequestContext};
use sea_query::{Cond, Condition, Expr, ExprTrait as _, Func, PostgresQueryBuilder, Query, SelectStatement};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row as _};

use crate::error::{Error, Result};
use query::{T, a, col};
use serialize::TaskRow;

const TABLE: &str = "documents_paperlesstask";

fn select_tasks(cond: Condition) -> SelectStatement {
    let mut q = Query::select();
    for c in serialize::COLUMNS {
        q.column((a(T), a(c)));
    }
    q.from_as(a(TABLE), a(T)).cond_where(cond);
    q
}

async fn owner_exists(db: &PgPool, id: Option<i64>) -> Result<bool> {
    let Some(id) = id else { return Ok(false) };
    let q = Query::select()
        .expr(Expr::exists(Query::select().expr(Expr::val(1)).from(a("auth_user")).and_where(Expr::col(a("id")).eq(id)).to_owned()))
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let (b,): (bool,) = sqlx::query_as_with(&sql, values).fetch_one(db).await?;
    Ok(b)
}

/// `filterset.qs` validation + condition (base queryset ∧ filters).
async fn filtered_condition(db: &PgPool, ctx: &RequestContext, tz: &Tz, user: &User, q: &QueryParams) -> Result<Condition> {
    let candidate = query::owner_candidate(q);
    let exists = owner_exists(db, candidate).await?;
    let f = query::parse_filter(q, ctx.language, tz, |id| exists && Some(id) == candidate)
        .map_err(|e| Error::Api(ApiError::validation(e.to_value())))?;
    Ok(Cond::all().add(query::base_condition(user, is_v9(ctx), q)).add(query::filter_condition(&f, ctx.language)))
}

fn is_v9(ctx: &RequestContext) -> bool {
    ctx.version().as_u16() < 10
}

/// Serialize rows with the serializer of the request's API version.
async fn serialize_rows(db: &PgPool, ctx: &RequestContext, tz: &Tz, user: &User, rows: &[TaskRow]) -> Result<Vec<Value>> {
    if !is_v9(ctx) {
        return rows.iter().map(|r| serialize::v10(r, ctx.language, tz).map_err(|e| Error::Api(ApiError::internal(e)))).collect();
    }
    let dups = duplicate_documents(db, user, rows).await?;
    Ok(rows.iter().map(|r| serialize::v9(r, tz, dups.get(&r.id).cloned().unwrap_or_default())).collect())
}

/// `TaskSerializerV9.get_duplicate_documents` for all rows in one query.
async fn duplicate_documents(db: &PgPool, user: &User, rows: &[TaskRow]) -> Result<HashMap<i64, Vec<Value>>> {
    let mut wanted: Vec<(i64, i64)> = Vec::new();
    for r in rows {
        if let Some(d) = r.duplicate_of() {
            let pk = serialize::py_int(d).map_err(|e| Error::Api(ApiError::internal(e)))?;
            wanted.push((r.id, pk));
        }
    }
    let mut out: HashMap<i64, Vec<Value>> = HashMap::new();
    if wanted.is_empty() {
        return Ok(out);
    }
    let ids: Vec<i64> = wanted.iter().map(|(_, d)| *d).collect();
    let mut q = Query::select();
    q.column((a("d"), a("id")))
        .column((a("d"), a("title")))
        .column((a("d"), a("deleted_at")))
        .from_as(a(DOCUMENT.table), a("d"))
        .and_where(Expr::col((a("d"), a("id"))).is_in(ids));
    if !user.is_staff {
        q.and_where(Expr::col((a("d"), a("id"))).in_subquery(objects::permitted_document_ids(Some(user), "view_document", true)));
    }
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let docs = sqlx::query_with(&sql, values).fetch_all(db).await?;
    let mut by_id: HashMap<i64, Value> = HashMap::new();
    for d in docs {
        let id = i64::from(d.try_get::<i32, _>("id")?);
        let title: String = d.try_get("title")?;
        let deleted_at: Option<chrono::DateTime<Utc>> = d.try_get("deleted_at")?;
        let mut m = Map::new();
        m.insert("id".into(), Value::from(id));
        m.insert("title".into(), Value::from(title));
        // values() → raw aware datetime, rendered by the JSON encoder (UTC, "Z")
        m.insert("deleted_at".into(), pl_core::datetime::format_datetime_opt(deleted_at.as_ref(), &chrono_tz::UTC));
        by_id.insert(id, Value::Object(m));
    }
    for (task, doc) in wanted {
        out.insert(task, by_id.get(&doc).cloned().into_iter().collect());
    }
    Ok(out)
}

fn rows_of(rows: &[sqlx::postgres::PgRow]) -> Result<Vec<TaskRow>> {
    rows.iter().map(|r| TaskRow::from_row(r).map_err(Error::from)).collect()
}

/// `GET /api/tasks/`
pub async fn list(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response> {
    auth.require_model_perms(&PAPERLESS_TASK)?;
    let db = state.db();
    let tz = state.config().tz;
    let user = auth.user();
    let cond = filtered_condition(db, &ctx, &tz, user, &ctx.query).await?;
    let mut sel = select_tasks(cond);
    query::apply_ordering(&mut sel, &query::ordering(&ctx.query));
    if is_v9(&ctx) {
        // v9: not paginated
        let (sql, values) = sel.build_sqlx(PostgresQueryBuilder);
        let rows = rows_of(&sqlx::query_with(&sql, values).fetch_all(db).await?)?;
        let data = serialize_rows(db, &ctx, &tz, user, &rows).await?;
        return Ok(Drf::ok(Value::Array(data)).into_response());
    }
    let (rows, info) = fetch_page(db, &sel, &PageRequest::from_query(&ctx.query)).await?;
    let rows = rows_of(&rows)?;
    let data = serialize_rows(db, &ctx, &tz, user, &rows).await?;
    Ok(Drf::ok(paginated_body(&ctx, &info, data, None)).into_response())
}

/// `GET /api/tasks/{pk}/`
pub async fn retrieve(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    UrlPath(pk): UrlPath<String>,
) -> Result<Response> {
    auth.require_model_perms(&PAPERLESS_TASK)?;
    let db = state.db();
    let tz = state.config().tz;
    let user = auth.user();
    // get_object: filter_queryset(get_queryset()) first, then the lookup
    let cond = filtered_condition(db, &ctx, &tz, user, &ctx.query).await?;
    // get_object_or_404: non-integer pk → Http404() ("Not found."); integers outside the
    // column range match nothing (Django: EmptyResultSet) → "No PaperlessTask matches ..."
    let Some(pk) = crate::py::int(&pk) else {
        return Err(ApiError::not_found().into());
    };
    let Ok(id) = i32::try_from(pk) else {
        return Err(ApiError::not_found_model("PaperlessTask").into());
    };
    let mut sel = select_tasks(cond);
    sel.and_where(col("id").eq(id));
    let (sql, values) = sel.build_sqlx(PostgresQueryBuilder);
    let rows = rows_of(&sqlx::query_with(&sql, values).fetch_all(db).await?)?;
    let Some(row) = rows.into_iter().next() else {
        return Err(ApiError::not_found_model("PaperlessTask").into());
    };
    auth.check_object_perms(db, &PAPERLESS_TASK, ObjRef { id: row.id, owner_id: row.owner_id }).await?;
    let data = serialize_rows(db, &ctx, &tz, user, std::slice::from_ref(&row)).await?;
    Ok(Drf::ok(data.into_iter().next().unwrap_or(Value::Null)).into_response())
}

/// `POST /api/tasks/acknowledge/`
pub async fn acknowledge(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    req: axum::extract::Request,
) -> Result<Response> {
    auth.require_perms_by_method(ACKNOWLEDGE_TASKS_PERMISSIONS)?;
    let db = state.db();
    let user = auth.user();
    let data = body::parse_request(req).await?;
    let base = query::base_condition(user, is_v9(&ctx), &ctx.query);
    let fields = body::acknowledge_fields(&data).map_err(Error::Api)?;
    let mut db_error = None;
    if let Some(Ok(ids)) = &fields.tasks {
        // validate_tasks → _validate_task_id_list: matching rows must equal len(tasks)
        let mut q = Query::select();
        q.expr(Expr::col((a(T), a("id"))).count())
            .from_as(a(TABLE), a(T))
            .cond_where(base.clone())
            .and_where(col("id").is_in(ids.iter().copied()));
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        let (n,): (i64,) = sqlx::query_as_with(&sql, values).fetch_one(db).await?;
        if n != ids.len() as i64 {
            db_error = Some("Some tasks in tasks don't exist or were specified twice.");
        }
    }
    let target = body::acknowledge_finish(fields, db_error).map_err(Error::Api)?;
    let mut upd = Query::update();
    upd.table(a(TABLE)).value(a("acknowledged"), true);
    // Django's queryset.update() → UPDATE ... WHERE id IN (SELECT id ... filtered)
    let mut ids = Query::select();
    ids.column((a(T), a("id"))).from_as(a(TABLE), a(T)).cond_where(base);
    match target {
        body::AckTarget::All => {
            ids.and_where(col("acknowledged").eq(false));
        }
        body::AckTarget::Tasks(list) => {
            ids.and_where(col("id").is_in(list));
        }
    }
    upd.and_where(Expr::col(a("id")).in_subquery(ids));
    let (sql, values) = upd.build_sqlx(PostgresQueryBuilder);
    let res = sqlx::query_with(&sql, values).execute(db).await?;
    Ok(Drf::ok(json!({"result": res.rows_affected()})).into_response())
}

/// `GET /api/tasks/summary/`
pub async fn summary(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response> {
    let user = auth.user();
    let monitoring = perms::has_system_status_permission(Some(user));
    if !monitoring {
        auth.require_model_perms(&PAPERLESS_TASK)?;
    }
    let days = match ctx.query.get("days") {
        None => 30,
        Some(raw) => match crate::py::int(raw) {
            Some(d) => d.clamp(1, 365) as i64,
            None => return Err(ApiError::validation(json!({"days": "Must be a positive integer."})).into()),
        },
    };
    let cutoff = Utc::now() - Duration::days(days);
    let mut cond = Cond::all().add(col("date_created").gte(cutoff));
    if !monitoring {
        cond = cond.add(query::base_condition(user, is_v9(&ctx), &ctx.query));
    }
    let count_if = |status: &str| -> sea_query::SimpleExpr {
        // custom SQL: COUNT(...) FILTER (WHERE ...) has no sea-query form
        Expr::cust_with_values("COUNT(\"t\".\"id\") FILTER (WHERE \"t\".\"status\" = $1)", [status.to_string()])
    };
    let mut q = Query::select();
    q.column((a(T), a("task_type")))
        .expr_as(Expr::col((a(T), a("id"))).count(), a("total_count"))
        .expr_as(count_if("pending"), a("pending_count"))
        .expr_as(count_if("success"), a("success_count"))
        .expr_as(count_if("failure"), a("failure_count"))
        .expr_as(
            Expr::cust("AVG(\"t\".\"duration_seconds\") FILTER (WHERE \"t\".\"duration_seconds\" IS NOT NULL)"),
            a("avg_duration_seconds"),
        )
        .expr_as(
            Expr::cust("AVG(\"t\".\"wait_time_seconds\") FILTER (WHERE \"t\".\"wait_time_seconds\" IS NOT NULL)"),
            a("avg_wait_time_seconds"),
        )
        .expr_as(Func::max(Expr::col((a(T), a("date_created")))), a("last_run"))
        .expr_as(Expr::cust("MAX(\"t\".\"date_done\") FILTER (WHERE \"t\".\"status\" = 'success')"), a("last_success"))
        .expr_as(Expr::cust("MAX(\"t\".\"date_done\") FILTER (WHERE \"t\".\"status\" = 'failure')"), a("last_failure"))
        .from_as(a(TABLE), a(T))
        .cond_where(cond)
        .group_by_col((a(T), a("task_type")));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(state.db()).await?;
    let tz = state.config().tz;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let mut m = Map::new();
        let f = |v: Option<f64>| v.and_then(serde_json::Number::from_f64).map_or(Value::Null, Value::Number);
        let dt = |v: Option<chrono::DateTime<Utc>>| pl_core::datetime::format_datetime_opt(v.as_ref(), &tz);
        m.insert("task_type".into(), Value::from(r.try_get::<String, _>("task_type")?));
        m.insert("total_count".into(), Value::from(r.try_get::<i64, _>("total_count")?));
        m.insert("pending_count".into(), Value::from(r.try_get::<i64, _>("pending_count")?));
        m.insert("success_count".into(), Value::from(r.try_get::<i64, _>("success_count")?));
        m.insert("failure_count".into(), Value::from(r.try_get::<i64, _>("failure_count")?));
        m.insert("avg_duration_seconds".into(), f(r.try_get("avg_duration_seconds")?));
        m.insert("avg_wait_time_seconds".into(), f(r.try_get("avg_wait_time_seconds")?));
        m.insert("last_run".into(), dt(r.try_get("last_run")?));
        m.insert("last_success".into(), dt(r.try_get("last_success")?));
        m.insert("last_failure".into(), dt(r.try_get("last_failure")?));
        out.push(Value::Object(m));
    }
    Ok(Drf::ok(Value::Array(out)).into_response())
}

/// `GET /api/tasks/status_counts/`
pub async fn status_counts(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response> {
    auth.require_model_perms(&PAPERLESS_TASK)?;
    let db = state.db();
    let tz = state.config().tz;
    // _STATUS_COUNT_EXCLUDED_FILTERS = {"status", "is_complete"}
    let reduced = QueryParams::from_pairs(ctx.query.pairs().iter().filter(|(k, _)| k != "status" && k != "is_complete").cloned());
    let cond = filtered_condition(db, &ctx, &tz, auth.user(), &reduced).await?;
    let in_statuses = |s: &[&str]| -> sea_query::SimpleExpr {
        let list = s.iter().map(|v| format!("'{v}'")).collect::<Vec<_>>().join(", ");
        // custom SQL: COUNT(...) FILTER; values are fixed choice constants, not user input
        Expr::cust(format!("COUNT(\"t\".\"id\") FILTER (WHERE \"t\".\"status\" IN ({list}))"))
    };
    let mut q = Query::select();
    q.expr_as(Expr::col((a(T), a("id"))).count(), a("all"))
        .expr_as(in_statuses(&["failure", "revoked"]), a("needs_attention"))
        .expr_as(in_statuses(&["pending", "started"]), a("in_progress"))
        .expr_as(in_statuses(&["success"]), a("completed"))
        .from_as(a(TABLE), a(T))
        .cond_where(cond);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let r = sqlx::query_with(&sql, values).fetch_one(db).await?;
    let mut m = Map::new();
    for k in ["all", "needs_attention", "in_progress", "completed"] {
        m.insert(k.into(), Value::from(r.try_get::<i64, _>(k)?));
    }
    Ok(Drf::ok(Value::Object(m)).into_response())
}

/// `GET /api/tasks/active/`
pub async fn active(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response> {
    auth.require_model_perms(&PAPERLESS_TASK)?;
    let db = state.db();
    let tz = state.config().tz;
    let user = auth.user();
    let mut sel = select_tasks(query::base_condition(user, is_v9(&ctx), &ctx.query));
    sel.and_where(col("status").is_in(["pending", "started"])).order_by((a(T), a("date_created")), sea_query::Order::Desc).limit(50);
    let (sql, values) = sel.build_sqlx(PostgresQueryBuilder);
    let rows = rows_of(&sqlx::query_with(&sql, values).fetch_all(db).await?)?;
    let data = serialize_rows(db, &ctx, &tz, user, &rows).await?;
    Ok(Drf::ok(Value::Array(data)).into_response())
}
