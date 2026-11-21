//! `GET /api/statistics/`
//!
//! upstream: src/documents/views.py@v3.4.2 (`StatisticsView.get`),
//! src/documents/permissions.py@v3.4.2 (`has_global_statistics_permission`,
//! `restrict_queryset_to_visible`, `permitted_document_ids`).

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use pl_authz::models::{CORRESPONDENT, DOCUMENT, DOCUMENT_TYPE, ObjectModel, STORAGE_PATH, TAG};
use pl_authz::{AuthUser, User, objects, perms};
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{AppState, Drf, RequestContext};
use sea_query::{Alias, Condition, Expr, Func, PostgresQueryBuilder, Query, SelectStatement, SimpleExpr};
use serde_json::{Map, Value};
use sqlx::PgPool;

use crate::error::Result;

/// `current_asn` over the permission-filtered documents (upstream fix #14710, after v3.4.2).
/// `false` = v3.4.2 behaviour (`Document.objects.aggregate(Max(...))`, unfiltered).
// Decision by the user 2026-11-08: filtered like upstream fix #14710 (no leak of foreign ASNs). Known difference from v3.4.2.
pub const ASN_MAX_USES_FILTERED_DOCUMENTS: bool = true;

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// Condition on alias `d` (`documents_document`): `documents` queryset of the view
/// (`Document.objects.filter(root_document__isnull=True)` [`.filter(id__in=permitted_document_ids(user))`]).
///
/// PERF-SCALE #6: on root rows `permitted_document_ids` (authorizing owner/id = the row's own) is
/// the plain row predicate `owner = user OR owner IS NULL OR granted(id)` – no self-join, no second
/// scan; the grant test is `pl_authz::objects::granted_key_cond` (L5 swap point).
fn documents_cond(user: &User, global: bool) -> Condition {
    let mut c =
        Condition::all().add(Expr::col((a("d"), a("deleted_at"))).is_null()).add(Expr::col((a("d"), a("root_document_id"))).is_null());
    if !global {
        if !user.is_active {
            c = c.add(Expr::cust("FALSE"));
        } else if !user.is_superuser {
            let owner = Expr::col((a("d"), a("owner_id")));
            c = c.add(Condition::any().add(owner.clone().eq(user.id)).add(owner.is_null()).add(objects::granted_key_cond(
                user,
                &DOCUMENT,
                "view_document",
                Expr::col((a("d"), a("id"))).into(),
            )));
        }
    }
    c
}

/// `SELECT COUNT(alias.id) FROM <model> alias [WHERE visible]` as a scalar subquery.
fn count_subquery(user: &User, global: bool, model: &ObjectModel, alias: &str) -> SimpleExpr {
    let mut q = Query::select();
    q.expr(Expr::col((a(alias), a("id"))).count()).from_as(a(model.table), a(alias));
    if !global {
        q.cond_where(objects::restrict_to_visible(Some(user), model, &model.codename("view"), alias));
    }
    SimpleExpr::SubQuery(None, Box::new(q.into_sub_query_statement()))
}

/// Visible tags (alias `t`), optionally only inbox tags.
fn tags_select(user: &User, global: bool, inbox_only: bool) -> SelectStatement {
    let mut q = Query::select();
    q.from_as(a(TAG.table), a("t"));
    if !global {
        q.cond_where(objects::restrict_to_visible(Some(user), &TAG, "view_tag", "t"));
    }
    if inbox_only {
        q.and_where(Expr::col((a("t"), a("is_inbox_tag"))).eq(true));
    }
    q
}

/// Columns of [`statement`].
type StatsRow = (i64, Vec<i32>, i64, i64, i64, i64, Option<String>, Option<i64>);

/// Statistics as JSON (field order of the view).
///
/// PERF-SCALE #6 (2026-11-10): **one** statement. The visible documents (`documents` of the view)
/// are a CTE `vis` (id, mime_type, content_length, archive_serial_number) evaluated once – for
/// restricted users `MATERIALIZED` (one scan with the visibility predicate, the inbox count and
/// the mime groups read the small result), for global statistics `NOT MATERIALIZED` (inlined:
/// the inbox count runs from the inbox tag rows into the primary key, the groups are one scan).
/// Before: two statements, each evaluating `permitted_document_ids` with its self-join (R4).
/// `documents_inbox` = visible documents with an inbox tag (`.values("id").distinct().count()` =
/// `COUNT(*)` over a semi-join). Mime groups ordered by count descending like upstream.
pub async fn compute(db: &PgPool, user: &User) -> Result<Value> {
    let global = perms::has_global_statistics_permission(Some(user));
    let sql = statement(user, global);
    let (tag_count, inbox_ids, inbox_docs, correspondent_count, document_type_count, storage_path_count, groups, asn_max): StatsRow =
        sqlx::query_as(&sql).fetch_one(db).await?;
    let inbox_tag_pks: Vec<i64> = inbox_ids.into_iter().map(i64::from).collect();
    let documents_inbox = if inbox_tag_pks.is_empty() { Value::Null } else { Value::from(inbox_docs) };

    // mime_type_stats: [[mime_type, count, chars], …] ordered by count descending.
    // With ASN_MAX_USES_FILTERED_DOCUMENTS the ASN maximum runs over the same documents.
    let rows: Vec<(String, i64, Option<i64>)> = match groups {
        Some(g) => {
            serde_json::from_str(&g).map_err(|e| crate::error::Error::Api(pl_core::ApiError::internal(format!("mime groups: {e}"))))?
        }
        None => Vec::new(),
    };
    let filtered_asn_max: i64 = asn_max.unwrap_or(0);
    let documents_total: i64 = rows.iter().map(|r| r.1).sum();
    let character_count: i64 = rows.iter().map(|r| r.2.unwrap_or(0)).sum();
    let file_type_counts: Vec<Value> = rows
        .iter()
        .map(|(mt, n, _)| {
            let mut m = Map::new();
            m.insert("mime_type".into(), Value::from(mt.clone()));
            m.insert("mime_type_count".into(), Value::from(*n));
            Value::Object(m)
        })
        .collect();

    // current_asn
    let current_asn = if ASN_MAX_USES_FILTERED_DOCUMENTS {
        filtered_asn_max
    } else {
        let mut q = Query::select();
        q.expr(Func::coalesce([
            Func::cast_as(Func::max(Expr::col((a("d"), a("archive_serial_number")))), a("bigint")).into(),
            Expr::val(0i64).into(),
        ]))
        .from_as(a("documents_document"), a("d"))
        .and_where(Expr::col((a("d"), a("deleted_at"))).is_null());
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        let (n,): (i64,) = sqlx::query_as_with(&sql, values).fetch_one(db).await?;
        n
    };

    let mut out = Map::new();
    out.insert("documents_total".into(), Value::from(documents_total));
    out.insert("documents_inbox".into(), documents_inbox);
    out.insert("inbox_tag".into(), inbox_tag_pks.first().map_or(Value::Null, |v| Value::from(*v)));
    out.insert("inbox_tags".into(), if inbox_tag_pks.is_empty() { Value::Null } else { Value::from(inbox_tag_pks.clone()) });
    out.insert("document_file_type_counts".into(), Value::Array(file_type_counts));
    out.insert("character_count".into(), Value::from(character_count));
    out.insert("tag_count".into(), Value::from(tag_count));
    out.insert("correspondent_count".into(), Value::from(correspondent_count));
    out.insert("document_type_count".into(), Value::from(document_type_count));
    out.insert("storage_path_count".into(), Value::from(storage_path_count));
    out.insert("current_asn".into(), Value::from(current_asn));
    Ok(Value::Object(out))
}

/// The single statistics statement (SQL text; values are integers/booleans rendered by sea-query).
/// Columns: tag_count, inbox_ids, inbox_docs, correspondent_count, document_type_count,
/// storage_path_count, mime groups (JSON text `[[mime, n, chars], …]`), ASN maximum.
fn statement(user: &User, global: bool) -> String {
    // tags = (all | visible).only("id", "is_inbox_tag"); Tag.Meta.ordering = ("name",)
    let mut tag_count = tags_select(user, global, false);
    tag_count.expr(Expr::col((a("t"), a("id"))).count());
    let mut inbox_ids = tags_select(user, global, true);
    // custom SQL: array_agg(... ORDER BY ...) has no sea-query form; '{}' keeps an empty array
    inbox_ids.expr(Expr::cust(r#"COALESCE(array_agg("t"."id" ORDER BY "t"."name"), '{}')"#));
    let mut inbox_tag_ids = tags_select(user, global, true);
    inbox_tag_ids.column((a("t"), a("id")));
    let mut inbox_doc_ids = Query::select();
    inbox_doc_ids
        .column((a("dt"), a("document_id")))
        .from_as(a("documents_document_tags"), a("dt"))
        .and_where(Expr::col((a("dt"), a("tag_id"))).in_subquery(inbox_tag_ids));
    let mut inbox_docs = Query::select();
    inbox_docs.expr(Expr::cust("COUNT(*)")).from_as(a("vis"), a("v")).and_where(Expr::col((a("v"), a("id"))).in_subquery(inbox_doc_ids));
    let sub = |s: SelectStatement| SimpleExpr::SubQuery(None, Box::new(s.into_sub_query_statement()));
    let mut q = Query::select();
    q.expr_as(sub(tag_count), a("tag_count"))
        .expr_as(sub(inbox_ids), a("inbox_ids"))
        .expr_as(sub(inbox_docs), a("inbox_docs"))
        .expr_as(count_subquery(user, global, &CORRESPONDENT, "c"), a("correspondent_count"))
        .expr_as(count_subquery(user, global, &DOCUMENT_TYPE, "y"), a("document_type_count"))
        .expr_as(count_subquery(user, global, &STORAGE_PATH, "p"), a("storage_path_count"))
        .expr_as(
            Expr::cust("(SELECT json_agg(json_build_array(m.mime_type, m.n, m.chars) ORDER BY m.n DESC)::text FROM mt m)"),
            a("mime_groups"),
        )
        .expr_as(Expr::cust("(SELECT MAX(m.asn_max) FROM mt m)"), a("asn_max"));

    // documents.values("mime_type").annotate(Count("id"), Sum("content_length")) (+ ASN maximum)
    let mut vis = Query::select();
    vis.columns([(a("d"), a("id")), (a("d"), a("mime_type")), (a("d"), a("content_length")), (a("d"), a("archive_serial_number"))])
        .from_as(a("documents_document"), a("d"))
        .cond_where(documents_cond(user, global));
    let materialized = if global { "NOT MATERIALIZED" } else { "MATERIALIZED" };
    format!(
        "WITH vis AS {materialized} ({vis}), \
         mt AS (SELECT v.mime_type, COUNT(v.id)::bigint AS n, SUM(v.content_length)::bigint AS chars, \
                MAX(v.archive_serial_number)::bigint AS asn_max FROM vis v GROUP BY v.mime_type) {main}",
        vis = vis.to_string(PostgresQueryBuilder),
        main = q.to_string(PostgresQueryBuilder),
    )
}

/// `GET /api/statistics/` – `permission_classes = (IsAuthenticated,)`.
pub async fn get(State(state): State<AppState>, _ctx: RequestContext, auth: AuthUser) -> Result<Response> {
    let v = compute(state.db(), auth.user()).await?;
    Ok(Drf::ok(v).into_response())
}
