//! Read-only routes of `/api/documents/`.
//!
//! upstream: src/documents/views.py@v3.4.2 (UnifiedSearchViewSet.list/next_asn, DocumentViewSet
//! get_queryset/retrieve/root/notes/share_links/history/_get_selection_data_for_queryset,
//! SelectionDataView, DocumentSelectionMixin), src/paperless/views.py (StandardPagination).
//!
//! Authentication and permissions exclusively via `pl_authz` (extractor [`AuthUser`]),
//! pagination via `pl_core::pagination`, filter/sorting via `pl_docfilter`,
//! full text via `pl_search` (see [`crate::listing`]).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use pl_authz::models::DOCUMENT;
use pl_authz::perms::{
    NOTE_PERMISSIONS, ObjRef, VIEW_DOCUMENTS_PERMISSIONS, has_perms_owner_aware,
};
use pl_authz::{AuthUser, User};
use pl_core::datetime::format_datetime;
use pl_core::pagination::{self, PageRequest};
use pl_core::{ApiError, AppState, Drf, RequestContext};
use sea_query::{Cond, Expr, Func, Order, Query};
use serde_json::{Map, Value, json};

use crate::db::{
    ApiResult, a, fetch_all, fetch_optional, get, i64_col, id_in_list, id_list, opt_i64,
};
use crate::listing::{self, DOC};
use crate::serializer::{SerializeAction, SerializeOpts, serialize_documents_db};

/// `true` if `GET /api/documents/` is a full-text search (`text`, `title_search`, `query`,
/// `more_like_id`); then the list serves the search branch via pl-search.
pub fn is_search_request(params: &[(String, String)]) -> bool {
    pl_search::views::is_search_request(params)
}

/// DRF JSON with indentation from content negotiation.
fn ok(ctx: &RequestContext, v: Value) -> Response {
    Drf::ok(v).indent(ctx.negotiation.indent).into_response()
}

fn forbidden_text(msg: &str) -> ApiError {
    ApiError::django_text(StatusCode::FORBIDDEN, msg)
}

/// `DocumentViewSet._get_selection_data_for_queryset` over explicit ids (full-text list:
/// `filtered_qs.filter(pk__in=ordered_ids)`). Query shape: [`crate::selection_data`].
pub async fn selection_data_list(db: &sqlx::PgPool, ids: &[i64]) -> ApiResult<Value> {
    crate::selection_data::for_ids(db, ids, crate::selection_data::Variant::List).await
}

// ---------------------------------------------------------------- documents.list

/// GET /api/documents/ – `UnifiedSearchViewSet.list` (with and without full text).
pub async fn list(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
) -> ApiResult<Response> {
    // (IsAuthenticated, PaperlessObjectPermissions) → documents.view_document
    let user = auth.require_model_perms(&DOCUMENT)?;
    let db = state.db();
    let params = ctx.query.pairs();
    if is_search_request(params) {
        // parse_search_params → _validate_tantivy_search_params (before the filters)
        if let Err(r) = pl_search::views::validate_search_params(
            &pl_search::views::active_search_params(params),
        ) {
            return Err(listing::view_response(r));
        }
        let q = listing::filtered_documents(db, *state.tz(), user, &ctx.query).await?;
        let body = listing::search_list(&state, &ctx, user, q).await?;
        return Ok(ok(&ctx, body));
    }
    let (q, content_text) = listing::filtered_documents_ext(db, *state.tz(), user, &ctx.query).await?;
    // DocumentViewSet.list: selection_data over the filtered set (before pagination)
    let selection =
        if pl_authz::owned::get_boolean(ctx.query.get("include_selection_data").unwrap_or("false"))
        {
            // without filter: from the label counters (PERF-SCALE §13), otherwise selection as a subquery
            // (no ID list over the wire, PERF-SCALE Q3)
            let fast = if crate::selection_data::list_is_unfiltered(db, *state.tz(), &ctx.query).await {
                crate::selection_data::for_all_unfiltered(db, user, &[], crate::selection_data::Variant::List).await?
            } else {
                None
            };
            match fast {
                Some(v) => Some(v),
                None => Some(crate::selection_data::for_query(db, &q, crate::selection_data::Variant::List).await?),
            }
        } else {
            None
        };
    let req = PageRequest::from_query(&ctx.query);
    // Substring search in the content: page + count in one scan (pgvalidate.md section 9)
    let (rows, info) = if content_text {
        pagination::fetch_page_windowed(db, &q, &req).await?
    } else {
        pagination::fetch_page(db, &q, &req).await?
    };
    let ids: Vec<i64> = rows
        .iter()
        .map(|r| i64_col(r, "id"))
        .collect::<ApiResult<_>>()?;
    let opts = SerializeOpts::from_query(&ctx.query, SerializeAction::List);
    let results = serialize_documents_db(db, *state.tz(), user, &ids, &opts).await?;
    let all = if ctx.version().includes_all() {
        let all = pagination::fetch_all_ids(db, &q, Expr::col((a(DOC), a("id"))).into()).await?;
        Some(all.into_iter().map(Value::from).collect())
    } else {
        None
    };
    let mut body = pagination::paginated_body(&ctx, &info, results, all);
    if let (Some(sel), Some(m)) = (selection, body.as_object_mut()) {
        m.insert("selection_data".into(), sel);
    }
    Ok(ok(&ctx, body))
}

// ---------------------------------------------------------------- documents.retrieve

fn parse_pk(pk: &str) -> Option<i64> {
    pk.parse::<i64>().ok()
}

/// GET /api/documents/{id}/
pub async fn retrieve(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
) -> ApiResult<Response> {
    let user = auth.require_model_perms(&DOCUMENT)?;
    let db = state.db();
    let p = &ctx.query;
    // get_object(): filter_queryset(get_queryset()) → filter backends take effect here too
    let mut q = listing::filtered_documents(db, *state.tz(), user, p).await?;
    // get_object_or_404 catches ValueError → Http404 without text
    let id = parse_pk(pk.as_str()).ok_or_else(ApiError::not_found)?;
    q.and_where(Expr::col((a(DOC), a("id"))).eq(id));
    q.clear_order_by();
    if fetch_optional(db, &q).await?.is_none() {
        return Err(ApiError::not_found_model("Document"));
    }
    // check_object_permissions (PaperlessObjectPermissions, GET → view_document)
    let Some(doc) = doc_auth(db, id, false).await? else {
        return Err(ApiError::not_found_model("Document"));
    };
    auth.check_object_perms(
        db,
        &DOCUMENT,
        ObjRef {
            id: doc.id,
            owner_id: doc.own_owner_self,
        },
    )
    .await?;
    if is_search_request(p.pairs()) {
        // UnifiedSearchViewSet.get_serializer_class → SearchResultSerializer, whose
        // to_representation(Document) fails with TypeError (hit["id"]) → 500 as upstream.
        return Err(ApiError::internal(
            "SearchResultSerializer cannot serialize a Document instance",
        ));
    }
    let opts = SerializeOpts::from_query(p, SerializeAction::Retrieve);
    let mut docs = serialize_documents_db(db, *state.tz(), user, &[id], &opts).await?;
    let Some(mut out) = docs.pop() else {
        return Err(ApiError::not_found_model("Document"));
    };
    if p.contains("version")
        && let Some(obj) = out.as_object_mut()
        && (obj.contains_key("content") || obj.contains_key("page_count"))
    {
        let (content, page_count) = resolve_version(db, id, p.get("version").unwrap_or("")).await?;
        if obj.contains_key("content") {
            obj.insert("content".into(), json!(content.unwrap_or_default()));
        }
        if obj.contains_key("page_count") {
            obj.insert("page_count".into(), json!(page_count));
        }
    }
    Ok(ok(&ctx, out))
}

/// `_resolve_file_doc` with `include_deleted=True` → (content, page_count) of the version.
async fn resolve_version(
    db: &sqlx::PgPool,
    root_id: i64,
    param: &str,
) -> ApiResult<(Option<String>, Option<i64>)> {
    let mut q = Query::select();
    q.columns(["id", "root_document_id", "content", "page_count"].map(a))
        .from(a(DOC));
    if param.is_empty() {
        // get_latest_version_for_root(include_deleted=True) or root
        q.cond_where(
            Cond::any()
                .add(Expr::col(a("root_document_id")).eq(root_id))
                .add(Expr::col(a("id")).eq(root_id)),
        )
        // Root last: first versions by versions_newest_first
        .order_by_expr(Expr::col(a("root_document_id")).is_null(), Order::Asc)
        .order_by_with_nulls(
            a("version_index"),
            Order::Desc,
            sea_query::NullOrdering::Last,
        )
        .order_by(a("id"), Order::Desc)
        .limit(1);
    } else {
        let vid = pagination::python_int(param)
            .and_then(|v| i64::try_from(v).ok())
            .ok_or_else(|| {
                ApiError::with_detail(StatusCode::NOT_FOUND, "Invalid version parameter")
            })?;
        q.and_where(Expr::col(a("id")).eq(vid));
    }
    let Some(r) = fetch_optional(db, &q).await? else {
        return Err(ApiError::not_found());
    };
    let id = i64_col(&r, "id")?;
    let root = opt_i64(&r, "root_document_id")?;
    if id != root_id && root != Some(root_id) {
        return Err(ApiError::not_found());
    }
    Ok((get(&r, "content")?, opt_i64(&r, "page_count")?))
}

struct DocAuth {
    id: i64,
    root_id: i64,
    /// Owner of the root document (`has_perms_owner_aware`).
    own_owner: Option<i64>,
    /// Owner of the document itself (`has_object_permission`).
    own_owner_self: Option<i64>,
}

impl DocAuth {
    /// The root document as an object for `has_perms_owner_aware`.
    fn root_ref(&self) -> ObjRef {
        ObjRef {
            id: self.root_id,
            owner_id: self.own_owner,
        }
    }
}

/// Document + root owner (`select_related("owner", "root_document__owner")`).
async fn doc_auth(db: &sqlx::PgPool, id: i64, include_deleted: bool) -> ApiResult<Option<DocAuth>> {
    let mut q = Query::select();
    q.column((a("d"), a("id")))
        .column((a("d"), a("root_document_id")))
        .column((a("d"), a("owner_id")))
        .expr_as(Expr::col((a("r"), a("id"))), a("r_id"))
        .expr_as(Expr::col((a("r"), a("owner_id"))), a("r_owner"))
        .from_as(a(DOC), a("d"))
        .join_as(
            sea_query::JoinType::LeftJoin,
            a(DOC),
            a("r"),
            Expr::col((a("r"), a("id"))).equals((a("d"), a("root_document_id"))),
        )
        .and_where(Expr::col((a("d"), a("id"))).eq(id));
    if !include_deleted {
        q.and_where(Expr::col((a("d"), a("deleted_at"))).is_null());
    }
    let Some(r) = fetch_optional(db, &q).await? else {
        return Ok(None);
    };
    let id = i64_col(&r, "id")?;
    let owner = opt_i64(&r, "owner_id")?;
    // get_root_document: version with existing root → root, otherwise the document itself
    Ok(Some(match opt_i64(&r, "r_id")? {
        Some(rid) if opt_i64(&r, "root_document_id")?.is_some() => DocAuth {
            id,
            root_id: rid,
            own_owner: opt_i64(&r, "r_owner")?,
            own_owner_self: owner,
        },
        _ => DocAuth {
            id,
            root_id: id,
            own_owner: owner,
            own_owner_self: owner,
        },
    }))
}

/// Primary key of a detail action: `Model.objects.get(pk=...)` with a non-number → ValueError → 500.
fn action_pk(pk: &str) -> ApiResult<i64> {
    parse_pk(pk)
        .ok_or_else(|| ApiError::internal(format!("Field 'id' expected a number but got '{pk}'.")))
}

/// `has_perms_owner_aware(request.user, perm, root)`.
async fn owner_aware(db: &sqlx::PgPool, user: &User, perm: &str, doc: &DocAuth) -> ApiResult<bool> {
    Ok(has_perms_owner_aware(db, Some(user), perm, &DOCUMENT, doc.root_ref()).await?)
}

// ---------------------------------------------------------------- documents.root

/// GET /api/documents/{id}/root/
pub async fn root(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
) -> ApiResult<Response> {
    let user = auth.require_model_perms(&DOCUMENT)?;
    let db = state.db();
    let id = action_pk(&pk)?;
    let Some(doc) = doc_auth(db, id, true).await? else {
        return Err(ApiError::not_found());
    };
    if !owner_aware(db, user, "view_document", &doc).await? {
        return Err(forbidden_text("Insufficient permissions"));
    }
    Ok(ok(&ctx, json!({ "root_id": doc.root_id })))
}

// ---------------------------------------------------------------- documents.notes.list

/// GET /api/documents/{id}/notes/
pub async fn notes(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
) -> ApiResult<Response> {
    // permission_classes=[PaperlessNotePermissions] (GET: view_note + view_document)
    auth.check_perms_by_method(NOTE_PERMISSIONS)?;
    let user = auth.user();
    let db = state.db();
    let id = action_pk(&pk)?;
    let Some(doc) = doc_auth(db, id, false).await? else {
        return Err(ApiError::not_found());
    };
    if !owner_aware(db, user, "view_document", &doc).await? {
        return Err(forbidden_text("Insufficient permissions to view notes"));
    }
    let mut notes = crate::serializer::load_notes(db, &[doc.id], *state.tz()).await?;
    Ok(ok(
        &ctx,
        Value::Array(notes.remove(&doc.id).unwrap_or_default()),
    ))
}

// ---------------------------------------------------------------- documents.share_links

/// GET /api/documents/{id}/share_links/
pub async fn share_links(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
) -> ApiResult<Response> {
    let user = auth.require_model_perms(&DOCUMENT)?;
    let db = state.db();
    let tz = *state.tz();
    let id = action_pk(&pk)?;
    let Some(doc) = doc_auth(db, id, false).await? else {
        return Err(ApiError::not_found());
    };
    if !owner_aware(db, user, "change_document", &doc).await? {
        return Err(forbidden_text("Insufficient permissions to add share link"));
    }
    let now = Utc::now();
    let q = Query::select()
        .column((a("s"), a("id")))
        .column((a("s"), a("created")))
        .column((a("s"), a("expiration")))
        .column((a("s"), a("slug")))
        .column((a("s"), a("document_id")))
        .column((a("s"), a("file_version")))
        .expr_as(Expr::col((a("d"), a("title"))), a("document_title"))
        .from_as(a("documents_sharelink"), a("s"))
        .join_as(
            sea_query::JoinType::InnerJoin,
            a(DOC),
            a("d"),
            Expr::col((a("d"), a("id"))).equals((a("s"), a("document_id"))),
        )
        // PaperRust (pl-pdftools §4.1, Known difference): for the root document also the links to its
        // versions, so that the sharing dialog shows which version (redacted or not) a link serves.
        .cond_where(
            Cond::any()
                .add(Expr::col((a("s"), a("document_id"))).eq(id))
                .add(Expr::col((a("d"), a("root_document_id"))).eq(id)),
        )
        .and_where(Expr::col((a("s"), a("deleted_at"))).is_null())
        .cond_where(
            Cond::any()
                .add(Expr::col((a("s"), a("expiration"))).is_null())
                .add(Expr::col((a("s"), a("expiration"))).gte(now)),
        )
        .order_by((a("s"), a("created")), Order::Desc)
        .to_owned();
    let mut out = Vec::new();
    for r in fetch_all(db, &q).await? {
        let created: DateTime<Utc> = get(&r, "created")?;
        let exp: Option<DateTime<Utc>> = get(&r, "expiration")?;
        out.push(json!({
            "id": i64_col(&r, "id")?,
            "created": format_datetime(&created, &tz),
            "expiration": exp.map(|e| json!(format_datetime(&e, &tz))).unwrap_or(Value::Null),
            "slug": get::<String>(&r, "slug")?,
            "document": i64_col(&r, "document_id")?,
            "document_title": get::<String>(&r, "document_title")?,
            "file_version": get::<String>(&r, "file_version")?,
        }));
    }
    Ok(ok(&ctx, Value::Array(out)))
}

// ---------------------------------------------------------------- documents.history

fn action_display(a: i64) -> &'static str {
    match a {
        0 => "create",
        1 => "update",
        2 => "delete",
        3 => "access",
        _ => "",
    }
}

async fn log_entries(
    db: &sqlx::PgPool,
    model: &str,
    object_ids: &[i64],
) -> ApiResult<Vec<sqlx::postgres::PgRow>> {
    let ctype = Query::select()
        .column(a("id"))
        .from(a("django_content_type"))
        .and_where(Expr::col(a("app_label")).eq("documents"))
        .and_where(Expr::col(a("model")).eq(model))
        .to_owned();
    let q = Query::select()
        .distinct()
        .column((a("l"), a("id")))
        .column((a("l"), a("timestamp")))
        .column((a("l"), a("action")))
        .column((a("l"), a("changes")))
        .column((a("l"), a("object_repr")))
        .expr_as(Expr::col((a("u"), a("id"))), a("actor_id"))
        .expr_as(Expr::col((a("u"), a("username"))), a("actor_username"))
        .from_as(a("auditlog_logentry"), a("l"))
        .join_as(
            sea_query::JoinType::LeftJoin,
            a("auth_user"),
            a("u"),
            Expr::col((a("u"), a("id"))).equals((a("l"), a("actor_id"))),
        )
        .and_where(Expr::col((a("l"), a("content_type_id"))).in_subquery(ctype))
        .and_where(Expr::col((a("l"), a("object_id"))).is_in(object_ids.iter().copied()))
        .order_by((a("l"), a("timestamp")), Order::Desc)
        .to_owned();
    fetch_all(db, &q).await
}

fn actor_json(r: &sqlx::postgres::PgRow) -> ApiResult<Value> {
    Ok(match opt_i64(r, "actor_id")? {
        Some(id) => json!({ "id": id, "username": get::<String>(r, "actor_username")? }),
        None => Value::Null,
    })
}

/// GET /api/documents/{id}/history/
pub async fn history(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
) -> ApiResult<Response> {
    let user = auth.require_model_perms(&DOCUMENT)?;
    let db = state.db();
    if !state.config().audit_log_enabled {
        return Err(ApiError::django_text(
            StatusCode::BAD_REQUEST,
            "Audit log is disabled",
        ));
    }
    let id = action_pk(&pk)?;
    let Some(doc) = doc_auth(db, id, false).await? else {
        return Err(ApiError::not_found());
    };
    if !user.has_perm("auditlog.view_logentry")
        || (doc.own_owner.is_some() && doc.own_owner != Some(user.id) && !user.is_superuser)
    {
        return Err(forbidden_text("Insufficient permissions"));
    }
    let mut entries: Vec<(DateTime<Utc>, Value)> = Vec::new();
    for r in log_entries(db, "document", &[id]).await? {
        let ts: DateTime<Utc> = get(&r, "timestamp")?;
        let changes: Option<Value> = get(&r, "changes")?;
        entries.push((
            ts,
            json!({
                "id": i64_col(&r, "id")?,
                "timestamp": format_datetime(&ts, &Tz::UTC),
                "action": action_display(i64_col(&r, "action")?),
                "changes": changes.unwrap_or(Value::Null),
                "actor": actor_json(&r)?,
            }),
        ));
    }
    let cfq = Query::select()
        .column(a("id"))
        .from(a("documents_customfieldinstance"))
        .and_where(Expr::col(a("document_id")).eq(id))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .to_owned();
    let cfi_ids = id_list(db, &cfq).await?;
    if !cfi_ids.is_empty() {
        for r in log_entries(db, "customfieldinstance", &cfi_ids).await? {
            let ts: DateTime<Utc> = get(&r, "timestamp")?;
            let repr: String = get(&r, "object_repr")?;
            let mut parts = repr.split(':');
            let field = parts.next().unwrap_or("").trim().to_string();
            let Some(value) = parts.next() else {
                return Err(ApiError::internal("IndexError: object_repr without ':'"));
            };
            entries.push((
                ts,
                json!({
                    "id": i64_col(&r, "id")?,
                    "timestamp": format_datetime(&ts, &Tz::UTC),
                    "action": action_display(i64_col(&r, "action")?),
                    "changes": { "custom_fields": {
                        "type": "custom_field",
                        "field": field,
                        "value": value.trim(),
                    }},
                    "actor": actor_json(&r)?,
                }),
            ));
        }
    }
    // sorted(..., key=timestamp, reverse=True) – stable
    entries.sort_by_key(|e| std::cmp::Reverse(e.0));
    Ok(ok(
        &ctx,
        Value::Array(entries.into_iter().map(|(_, v)| v).collect()),
    ))
}

// ---------------------------------------------------------------- documents.next_asn

/// GET /api/documents/next_asn/
pub async fn next_asn(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
) -> ApiResult<Response> {
    auth.require_model_perms(&DOCUMENT)?;
    let q = next_asn_query();
    let max = match fetch_optional(state.db(), &q).await? {
        Some(r) => opt_i64(&r, "m")?.unwrap_or(0),
        None => 0,
    };
    Ok(ok(&ctx, json!(max + 1)))
}

/// Highest ASN over **all** documents, including those in the trash. upstream uses `Document.objects`
/// (without trash) and so suggests an ASN that a deleted document still holds – saving
/// then fails on uniqueness (A-109). PaperRust counts the trash too
/// (Known differences).
fn next_asn_query() -> sea_query::SelectStatement {
    Query::select()
        .expr_as(Func::max(Expr::col(a("archive_serial_number"))), a("m"))
        .from(a(DOC))
        .to_owned()
}

#[cfg(test)]
mod next_asn_tests {
    /// A-109: trash documents count too (no `deleted_at` filter).
    #[test]
    fn next_asn_counts_trash() {
        let sql = super::next_asn_query().to_string(sea_query::PostgresQueryBuilder);
        assert_eq!(sql, r#"SELECT MAX("archive_serial_number") AS "m" FROM "documents_document""#);
    }
}

// ---------------------------------------------------------------- documents.selection_data

fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// DRF `IntegerField.to_internal_value`.
fn drf_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
        Value::String(s) if s.len() <= 1000 => {
            let t = s.trim();
            let t = t.strip_suffix('.').unwrap_or(t);
            let stripped = match t.find('.') {
                Some(pos) if t[pos + 1..].chars().all(|c| c == '0') => &t[..pos],
                _ => t,
            };
            pagination::python_int(stripped).and_then(|v| i64::try_from(v).ok())
        }
        _ => None,
    }
}

/// `ListField(child=IntegerField())`.
fn int_list(v: &Value) -> Result<Vec<i64>, Value> {
    let Value::Array(items) = v else {
        if v.is_null() {
            return Err(json!(["This field may not be null."]));
        }
        return Err(json!([format!(
            "Expected a list of items but got type \"{}\".",
            py_type(v)
        )]));
    };
    let mut out = Vec::new();
    let mut errs = Map::new();
    for (i, it) in items.iter().enumerate() {
        match drf_int(it) {
            Some(n) => out.push(n),
            None => {
                let msg = match it {
                    Value::String(s) if s.len() > 1000 => "String value too large.",
                    _ => "A valid integer is required.",
                };
                errs.insert(i.to_string(), json!([msg]));
            }
        }
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(Value::Object(errs))
    }
}

/// DRF `BooleanField.to_internal_value`.
fn drf_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) if n.as_i64() == Some(1) => Some(true),
        Value::Number(n) if n.as_i64() == Some(0) => Some(false),
        Value::String(s) => match s.as_str() {
            "t" | "T" | "y" | "Y" | "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on"
            | "On" | "ON" | "1" => Some(true),
            "f" | "F" | "n" | "N" | "no" | "No" | "NO" | "false" | "False" | "FALSE" | "off"
            | "Off" | "OFF" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

async fn count_existing(db: &sqlx::PgPool, ids: &[i64]) -> ApiResult<i64> {
    let q = Query::select()
        .expr_as(Func::count(Expr::col(a("id"))), a("c"))
        .from(a(DOC))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .and_where(id_in_list(Expr::col(a("id")).into(), ids))
        .to_owned();
    Ok(match fetch_optional(db, &q).await? {
        Some(r) => opt_i64(&r, "c")?.unwrap_or(0),
        None => 0,
    })
}

/// `documents.exclude(id__in=permitted_document_ids(user))` as a condition (`None` = all allowed).
///
/// As `NOT EXISTS` (anti-join) instead of `NOT (id IN (…))`: the negated IN subquery becomes a
/// SubPlan; if the allowed set does not fit into `work_mem` (1 million, restricted user ~600 000
/// ids), it is re-run for each row (10 000 selected documents: 98 s instead of 0.18 s). `id` is never
/// NULL, so `NOT EXISTS` is equivalent to `NOT IN`.
fn not_permitted_cond(user: &User) -> Option<sea_query::SimpleExpr> {
    if pl_authz::objects::user_is_active_superuser(Some(user)) {
        return None;
    }
    let permitted = pl_authz::objects::permitted_document_ids(Some(user), "view_document", false);
    let sub = Query::select()
        .expr(Expr::cust("1"))
        .from_subquery(permitted, a("pl_perm"))
        .and_where(Expr::col((a("pl_perm"), a("id"))).equals((a(DOC), a("id"))))
        .to_owned();
    Some(Expr::exists(sub).not())
}

/// Text fields of a multipart body (file parts skipped) in body order (DRF `MultiPartParser`).
async fn multipart_text_fields(headers: &axum::http::HeaderMap, body: &Bytes) -> ApiResult<Vec<(String, String)>> {
    use axum::extract::FromRequest;
    let mut req = axum::http::Request::new(axum::body::Body::from(body.clone()));
    *req.headers_mut() = headers.clone();
    let mut mp = axum::extract::Multipart::from_request(req, &())
        .await
        .map_err(|_| ApiError::parse_error(Some("Multipart form parse error - Invalid boundary in multipart: None")))?;
    let mut out = Vec::new();
    loop {
        match mp.next_field().await {
            Ok(Some(f)) => {
                let name = f.name().unwrap_or("").to_string();
                if f.file_name().is_some() {
                    continue;
                }
                out.push((name, f.text().await.unwrap_or_default()));
            }
            Ok(None) => break,
            Err(_) => return Err(ApiError::parse_error(Some("Multipart form parse error - Invalid multipart body"))),
        }
    }
    Ok(out)
}

/// `DocumentSelectionSerializer` input from a multipart body, like DRF's `get_value` for HTML
/// input (rest_framework 3.18.3 fields.py, utils/html.py):
/// * `documents`, `excluded_documents` (`ListField`): `getlist`, else `parse_html_list`
///   (`documents[0]=…`), else not given;
/// * `all` (`BooleanField`): missing → `false` (`default_empty_html`), `""` → not given;
/// * `filters` (`DictField`): `parse_html_dict(prefix="filters")` – always a dict (possibly
///   empty), values = last value per key.
pub(crate) fn selection_form_data(pairs: &[(String, String)]) -> Map<String, Value> {
    let last = |k: &str| pairs.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let list = |name: &str| -> Option<Value> {
        let direct: Vec<Value> = pairs.iter().filter(|(k, _)| k == name).map(|(_, v)| Value::String(v.clone())).collect();
        if !direct.is_empty() {
            return Some(Value::Array(direct));
        }
        // parse_html_list: `name[<n>]` keys, sorted by index, last value per key
        let mut items: std::collections::BTreeMap<u64, Value> = std::collections::BTreeMap::new();
        for (k, _) in pairs {
            let Some(idx) = k.strip_prefix(name).and_then(|r| r.strip_prefix('[')).and_then(|r| r.strip_suffix(']')) else {
                continue;
            };
            if let Ok(i) = idx.parse::<u64>()
                && idx.bytes().all(|b| b.is_ascii_digit())
            {
                items.insert(i, Value::String(last(k).unwrap_or_default().to_owned()));
            }
        }
        if items.is_empty() { None } else { Some(Value::Array(items.into_values().collect())) }
    };
    let mut m = Map::new();
    if let Some(d) = list("documents") {
        m.insert("documents".into(), d);
    }
    match last("all") {
        None => {
            m.insert("all".into(), Value::Bool(false));
        }
        Some("") => {}
        Some(v) => {
            m.insert("all".into(), Value::String(v.to_owned()));
        }
    }
    let mut filters = Map::new();
    for (k, _) in pairs {
        if let Some(key) = k.strip_prefix("filters.").filter(|r| !r.is_empty())
            && !filters.contains_key(key)
        {
            filters.insert(key.to_owned(), Value::String(last(k).unwrap_or_default().to_owned()));
        }
    }
    m.insert("filters".into(), Value::Object(filters));
    if let Some(e) = list("excluded_documents") {
        m.insert("excluded_documents".into(), e);
    }
    m
}

/// POST /api/documents/selection_data/
pub async fn selection_data_view(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    body: Bytes,
) -> ApiResult<Response> {
    // (IsAuthenticated, ViewDocumentsPermissions)
    let user = auth.require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS)?;
    let db = state.db();
    // SelectionDataView.parser_classes = (MultiPartParser, JSONParser): JSON as is, multipart via
    // `selection_form_data`, anything else (incl. form-urlencoded) DRF's 415.
    let ct_raw = ctx
        .headers
        .get("content-type")
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .unwrap_or_default();
    let media = ct_raw.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let data: Value = if body.is_empty() {
        // Request._parse without stream: {} (an empty QueryDict validates the same way)
        json!({})
    } else {
        match media.as_str() {
            "application/json" => match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => return Err(ApiError::json_body_parse_error(&body, e)),
            },
            "multipart/form-data" => Value::Object(selection_form_data(&multipart_text_fields(&ctx.headers, &body).await?)),
            _ => return Err(ApiError::unsupported_media_type(&ct_raw)),
        }
    };
    let Value::Object(data) = data else {
        return Err(ApiError::validation(json!({
            "non_field_errors": [format!("Invalid data. Expected a dictionary, but got {}.", py_type(&data))]
        })));
    };

    // Field validation in field order
    let mut errors = Map::new();
    let mut documents: Option<Vec<i64>> = None;
    if let Some(d) = data.get("documents") {
        match int_list(d) {
            Ok(list) => {
                // validate_documents → _validate_document_id_list
                if count_existing(db, &list).await? != list.len() as i64 {
                    errors.insert(
                        "documents".into(),
                        json!(["Some documents in documents don't exist or were specified twice."]),
                    );
                } else {
                    documents = Some(list);
                }
            }
            Err(e) => {
                errors.insert("documents".into(), e);
            }
        }
    }
    let mut all = false;
    if let Some(x) = data.get("all") {
        match drf_bool(x) {
            Some(b) => all = b,
            None if x.is_null() => {
                errors.insert("all".into(), json!(["This field may not be null."]));
            }
            None => {
                errors.insert("all".into(), json!(["Must be a valid boolean."]));
            }
        }
    }
    let mut filter_map = Map::new();
    if let Some(f) = data.get("filters") {
        match f {
            Value::Object(m) => filter_map = m.clone(),
            Value::Null => {
                errors.insert("filters".into(), json!(["This field may not be null."]));
            }
            other => {
                errors.insert(
                    "filters".into(),
                    json!([format!(
                        "Expected a dictionary of items but got type \"{}\".",
                        py_type(other)
                    )]),
                );
            }
        }
    }
    let mut excluded: Vec<i64> = Vec::new();
    if let Some(x) = data.get("excluded_documents") {
        match int_list(x) {
            Ok(l) => excluded = l,
            Err(e) => {
                errors.insert("excluded_documents".into(), e);
            }
        }
    }
    if !errors.is_empty() {
        return Err(ApiError::validation(Value::Object(errors)));
    }
    if all {
        // DocumentSelectionMixin._resolve_document_ids (docs/STANDALONE-GAPS.md §4.1) as a
        // subquery instead of an ID list (PERF-SCALE §12). The two checks below are always satisfied for this
        // set: it contains only non-deleted, visible (`view_document`)
        // root documents, each id once – `count() == len(ids)` and `exclude(permitted)` empty.
        // without filter: from the label counters (PERF-SCALE §13)
        if crate::selection_data::selection_is_unfiltered(db, *state.tz(), &filter_map).await
            && let Some(v) =
                crate::selection_data::for_all_unfiltered(db, user, &excluded, crate::selection_data::Variant::Post).await?
        {
            return Ok(ok(&ctx, v));
        }
        let q = crate::selection_filter::resolve_all_query(&state, user, Some(&filter_map), &excluded).await?;
        return Ok(ok(&ctx, crate::selection_data::for_query(db, &q, crate::selection_data::Variant::Post).await?));
    }
    let ids: Vec<i64> = {
        if !excluded.is_empty() {
            return Err(ApiError::validation(json!({
                "non_field_errors": ["excluded_documents is only supported when all is true."]
            })));
        }
        match documents {
            Some(d) => d,
            None => {
                return Err(ApiError::validation(json!({
                    "non_field_errors": ["documents is required unless all is true."]
                })));
            }
        }
    };

    // documents.count() != len(ids) or documents.exclude(id__in=permitted).exists()
    if count_existing(db, &ids).await? != ids.len() as i64 {
        return Err(forbidden_text("Insufficient permissions"));
    }
    if let Some(c) = not_permitted_cond(user) {
        let q = Query::select()
            .column((a(DOC), a("id")))
            .from(a(DOC))
            .and_where(Expr::col((a(DOC), a("deleted_at"))).is_null())
            .and_where(id_in_list(Expr::col((a(DOC), a("id"))).into(), &ids))
            .and_where(c)
            .limit(1)
            .to_owned();
        if fetch_optional(db, &q).await?.is_some() {
            return Err(forbidden_text("Insufficient permissions"));
        }
    }
    Ok(ok(&ctx, crate::selection_data::for_ids(db, &ids, crate::selection_data::Variant::Post).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drf_ints() {
        assert_eq!(drf_int(&json!("5")), Some(5));
        assert_eq!(drf_int(&json!(5.0)), Some(5));
        assert_eq!(drf_int(&json!("5.0")), Some(5));
        assert_eq!(drf_int(&json!(5.5)), None);
        assert_eq!(drf_int(&json!(true)), None);
        assert!(int_list(&json!("x")).is_err());
        assert!(int_list(&json!(null)).is_err());
    }

    #[test]
    fn bools() {
        assert_eq!(drf_bool(&json!("on")), Some(true));
        assert_eq!(drf_bool(&json!(0)), Some(false));
        assert_eq!(drf_bool(&json!("maybe")), None);
    }

    #[test]
    fn search_detection() {
        let p = |s: &str| pl_core::QueryParams::parse(s);
        assert!(is_search_request(p("query=a").pairs()));
        assert!(is_search_request(p("more_like_id=").pairs()));
        assert!(!is_search_request(p("search=a&title__icontains=b").pairs()));
    }
}

#[cfg(test)]
mod selection_form_tests {
    use super::*;

    fn p(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    /// Multipart selection_data body like DRF's HTML-input field access.
    #[test]
    fn multipart_like_drf() {
        assert_eq!(
            Value::Object(selection_form_data(&p(&[("documents", "1"), ("documents", "2"), ("filters.title__icontains", "a"), ("filters.title__icontains", "b")]))),
            json!({"documents": ["1", "2"], "all": false, "filters": {"title__icontains": "b"}})
        );
        assert_eq!(
            Value::Object(selection_form_data(&p(&[("documents[1]", "5"), ("documents[0]", "4"), ("all", ""), ("excluded_documents", "9")]))),
            json!({"documents": ["4", "5"], "filters": {}, "excluded_documents": ["9"]})
        );
        assert_eq!(Value::Object(selection_form_data(&[])), json!({"all": false, "filters": {}}));
    }
}
