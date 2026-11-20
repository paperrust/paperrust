//! `/api/share_links/`
//!
//! upstream: src/documents/views.py@v3.4.2 (ShareLinkViewSet), src/documents/serialisers.py@v3.4.2
//! (ShareLinkSerializer), src/documents/filters.py@v3.4.2 (ShareLinkFilterSet),
//! src/documents/models.py@v3.4.2 (ShareLink, SoftDeleteModel from django-soft-delete).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use pl_core::AppState;
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query, SelectStatement};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};

use crate::authz::{can_share_document, check_object_perm, require_model_perm, visible};
use pl_authz::models::{ObjectModel, SHARE_LINK};
use crate::fields::{ChoiceOpts, Errors, F, choice_str, datetime_field, pk_input, pk_missing_msg};
use crate::query::{FilterErrors, apply_ordering, datetime_filters, ordering_terms};
use crate::shim::{
    ApiError, ApiResult, Ctx, finish, fmt_dt, fmt_dt_opt, get_random_string, json_response,
    no_content, page_request, paginated, parse_body, parse_pk, require_object, resolve_page,
};

const OM: ObjectModel = SHARE_LINK;
const LABEL: &str = "ShareLink";
pub const LIST_ALLOW: &str = "GET, POST, HEAD, OPTIONS";
pub const DETAIL_ALLOW: &str = "GET, DELETE, HEAD, OPTIONS";
pub const FILE_VERSIONS: &[&str] = &["archive", "original"];

fn a(s: &str) -> Alias {
    Alias::new(s)
}

struct Link {
    id: i64,
    created: DateTime<Utc>,
    expiration: Option<DateTime<Utc>>,
    slug: String,
    document: i64,
    document_title: String,
    file_version: String,
    owner_id: Option<i64>,
}

fn base(ctx: &Ctx) -> SelectStatement {
    let mut q = Query::select();
    q.from_as(a("documents_sharelink"), a("sl"))
        .join_as(
            sea_query::JoinType::InnerJoin,
            a("documents_document"),
            a("d"),
            Expr::col((a("d"), a("id"))).equals((a("sl"), a("document_id"))),
        )
        .and_where(Expr::cust("sl.deleted_at IS NULL"));
    q.cond_where(visible(ctx, &OM, "sl"));
    q
}

fn select_columns(q: &mut SelectStatement) {
    for (e, n) in [
        ("sl.id::int8", "id"),
        ("sl.created", "created"),
        ("sl.expiration", "expiration"),
        ("sl.slug", "slug"),
        ("sl.document_id::int8", "document"),
        ("d.title", "document_title"),
        ("sl.file_version", "file_version"),
        ("sl.owner_id::int8", "owner_id"),
    ] {
        q.expr_as(Expr::cust(e), a(n));
    }
}

fn from_row(r: &sqlx::postgres::PgRow) -> Result<Link, sqlx::Error> {
    Ok(Link {
        id: r.try_get("id")?,
        created: r.try_get("created")?,
        expiration: r.try_get("expiration")?,
        slug: r.try_get("slug")?,
        document: r.try_get("document")?,
        document_title: r.try_get("document_title")?,
        file_version: r.try_get("file_version")?,
        owner_id: r.try_get("owner_id")?,
    })
}

fn to_json(l: &Link) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(l.id));
    m.insert("created".into(), json!(fmt_dt(&l.created)));
    m.insert("expiration".into(), fmt_dt_opt(&l.expiration));
    m.insert("slug".into(), json!(l.slug));
    m.insert("document".into(), json!(l.document));
    m.insert("document_title".into(), json!(l.document_title));
    m.insert("file_version".into(), json!(l.file_version));
    Value::Object(m)
}

async fn get_object(db: &PgPool, ctx: &Ctx, _method: &Method, pk: &str) -> ApiResult<Link> {
    let id = parse_pk(pk)?;
    let mut q = base(ctx);
    select_columns(&mut q);
    q.and_where(Expr::cust_with_values("sl.id = $1", [id]));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let Some(row) = sqlx::query_with(&sql, values).fetch_optional(db).await? else {
        return Err(ApiError::not_found(LABEL));
    };
    let l = from_row(&row)?;
    check_object_perm(db, ctx, &OM, l.id, l.owner_id).await?;
    Ok(l)
}

pub async fn list(State(st): State<AppState>, ctx: Ctx) -> Response {
    finish(LIST_ALLOW, list_inner(&st.inner.db, &ctx).await)
}

async fn list_inner(db: &PgPool, ctx: &Ctx) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let mut q = base(ctx);
    let mut errs = FilterErrors::default();
    datetime_filters(&mut q, ctx, "created", "sl.created", &mut errs);
    datetime_filters(&mut q, ctx, "expiration", "sl.expiration", &mut errs);
    errs.into_result()?;
    let mut cq = q.clone();
    cq.expr(Expr::cust("COUNT(*)::int8"));
    let (sql, values) = cq.build_sqlx(PostgresQueryBuilder);
    let count: i64 = sqlx::query_scalar_with(&sql, values).fetch_one(db).await?;
    match ordering_terms(ctx, &["created", "expiration", "document__title"]) {
        Some(t) => apply_ordering(&mut q, &t, |n| match n {
            "document__title" => "d.title".into(),
            other => format!("sl.{other}"),
        }),
        None => {
            q.order_by_expr(Expr::cust("sl.created"), sea_query::Order::Asc);
        }
    }
    let page = resolve_page(&page_request(ctx), count)?;
    let all_ids = if ctx.version < 10 {
        let mut iq = q.clone();
        iq.expr(Expr::cust("sl.id::int8"));
        let (sql, values) = iq.build_sqlx(PostgresQueryBuilder);
        Some(sqlx::query_scalar_with::<_, i64, _>(&sql, values).fetch_all(db).await?)
    } else {
        None
    };
    select_columns(&mut q);
    q.limit(page.page_size).offset(page.offset());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    let mut results = Vec::with_capacity(rows.len());
    for r in &rows {
        results.push(to_json(&from_row(r)?));
    }
    Ok(paginated(ctx, &page, all_ids, results))
}

pub async fn retrieve(State(st): State<AppState>, Path(pk): Path<String>, ctx: Ctx) -> Response {
    finish(DETAIL_ALLOW, retrieve_inner(&st.inner.db, &ctx, &pk).await)
}

async fn retrieve_inner(db: &PgPool, ctx: &Ctx, pk: &str) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let l = get_object(db, ctx, &Method::GET, pk).await?;
    Ok(json_response(StatusCode::OK, &to_json(&l)))
}

pub async fn create(State(st): State<AppState>, ctx: Ctx, body: Bytes) -> Response {
    finish(LIST_ALLOW, create_inner(&st.inner.db, &ctx, &body).await)
}

/// Consumes one value of the `documents_sharelink.id` identity sequence (like a failed INSERT).
const BURN_ID_SQL: &str = "SELECT nextval(pg_get_serial_sequence('documents_sharelink', 'id'))";

async fn create_inner(db: &PgPool, ctx: &Ctx, body: &[u8]) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let data = require_object(parse_body(&ctx.headers, body)?)?;
    let mut errs = Errors::default();
    // Field order: expiration, document, file_version
    let expiration = match datetime_field(&data, "expiration", false, &mut errs) {
        F::Val(v) => v,
        _ => None,
    };
    let mut document: Option<(i64, String)> = None;
    match data.get("document") {
        // blank=True → not required; missing → IntegrityError on INSERT (500)
        None => {}
        Some(Value::Null) => errs.add("document", crate::fields::NULL),
        Some(v) => match pk_input(v) {
            Err(m) => errs.add("document", &m),
            Ok(pk) => {
                let title: Option<String> = sqlx::query_scalar(
                    "SELECT title FROM documents_document WHERE id = $1::int8 AND deleted_at IS NULL",
                )
                .bind(pk)
                .fetch_optional(db)
                .await?;
                match title {
                    None => errs.add("document", &pk_missing_msg(v)),
                    Some(t) => {
                        // validate_document: PermissionDenied aborts immediately (403)
                        let ok = can_share_document(db, ctx, pk).await?;
                        if !ok {
                            return Err(ApiError::insufficient());
                        }
                        document = Some((pk, t));
                    }
                }
            }
        },
    }
    let file_version = match choice_str(
        &data,
        "file_version",
        FILE_VERSIONS,
        ChoiceOpts {
            required: false,
            allow_null: false,
            allow_blank: false,
        },
        false,
        &mut errs,
    ) {
        F::Val(Some(f)) => f,
        _ => "archive".to_string(),
    };
    errs.into_result()?;
    let Some((doc_id, title)) = document else {
        // upstream: IntegrityError (document_id NOT NULL) → 500. The failed INSERT has already
        // drawn an id from the identity sequence; draw one too so later ids stay the same
        // (W2 parity run 2026-11-08, w2.sl.create.validation: next link got id 7 vs 6).
        sqlx::query(BURN_ID_SQL).execute(db).await?;
        return Err(ApiError::ServerError);
    };
    let slug = get_random_string(50);
    let now = Utc::now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO documents_sharelink (created, expiration, slug, document_id, file_version, owner_id) \
         VALUES ($1, $2, $3, $4::int8, $5, $6::int8) RETURNING id::int8",
    )
    .bind(now)
    .bind(expiration)
    .bind(&slug)
    .bind(doc_id)
    .bind(&file_version)
    .bind(ctx.user.id)
    .fetch_one(db)
    .await?;
    let l = Link {
        id,
        created: now,
        expiration,
        slug,
        document: doc_id,
        document_title: title,
        file_version,
        owner_id: Some(ctx.user.id),
    };
    Ok(json_response(StatusCode::CREATED, &to_json(&l)))
}

pub async fn destroy(State(st): State<AppState>, Path(pk): Path<String>, ctx: Ctx) -> Response {
    finish(DETAIL_ALLOW, destroy_inner(&st.inner.db, &ctx, &pk).await)
}

async fn destroy_inner(db: &PgPool, ctx: &Ctx, pk: &str) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let l = get_object(db, ctx, &Method::DELETE, pk).await?;
    // SoftDeleteModel.delete(): deleted_at, restored_at, transaction_id
    sqlx::query(
        "UPDATE documents_sharelink SET deleted_at = $1, restored_at = NULL, transaction_id = $2 \
         WHERE id = $3::int8",
    )
    .bind(Utc::now())
    .bind(uuid::Uuid::new_v4())
    .bind(l.id)
    .execute(db)
    .await?;
    Ok(no_content())
}
