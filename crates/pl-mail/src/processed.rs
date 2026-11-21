//! `/api/processed_mail/` – ProcessedMailViewSet (read only + bulk_delete).
//!
//! upstream: src/paperless_mail/views.py@v3.4.2 (ProcessedMailViewSet, DeleteProcessedMailPermissions),
//! src/paperless_mail/filters.py (ProcessedMailFilterSet), src/paperless_mail/serialisers.py
//! (ProcessedMailSerializer), rest_framework/filters.py@3.16.1 (OrderingFilter),
//! django_filters (ModelChoiceFilter → "Select a valid choice...").

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use pl_core::AppState;
use serde_json::{Map, Value, json};
use sqlx::PgPool;

use crate::body::parse_body;
use crate::ctx::MailUser;
use crate::drf;
use crate::error::{ApiError, ApiResult, json_response};
use crate::listing::{self, ReqInfo};
use crate::perms::{self, PROCESSED_MAIL};
use crate::pyval;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProcessedRow {
    pub id: i32,
    pub owner_id: Option<i32>,
    pub rule_id: i32,
    pub folder: String,
    pub uid: String,
    pub subject: String,
    pub received: DateTime<Utc>,
    pub processed: DateTime<Utc>,
    pub status: String,
    pub error: Option<String>,
}

const COLS: &str = "p.id, p.owner_id, p.rule_id, p.folder, p.uid, p.subject, p.received, p.processed, p.status, p.error";

fn represent(r: &ProcessedRow, tz: chrono_tz::Tz) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(r.id));
    m.insert("owner".into(), json!(r.owner_id));
    m.insert("rule".into(), json!(r.rule_id));
    m.insert("folder".into(), json!(r.folder));
    m.insert("uid".into(), json!(r.uid));
    m.insert("subject".into(), json!(r.subject));
    m.insert("received".into(), json!(drf::fmt_datetime(&r.received, tz)));
    m.insert("processed".into(), json!(drf::fmt_datetime(&r.processed, tz)));
    m.insert("status".into(), json!(r.status));
    m.insert("error".into(), json!(r.error));
    Value::Object(m)
}

/// Valid `ordering` fields (default fields of the serializer) → SQL column.
fn ordering_column(field: &str) -> Option<&'static str> {
    Some(match field {
        "id" => "p.id",
        "owner" => "p.owner_id",
        "rule" => "p.rule_id",
        "folder" => "p.folder",
        "uid" => "p.uid",
        "subject" => "p.subject",
        "received" => "p.received",
        "processed" => "p.processed",
        "status" => "p.status",
        "error" => "p.error",
        _ => return None,
    })
}

/// `OrderingFilter.get_ordering` → ORDER BY.
pub fn order_by(param: Option<&str>) -> String {
    if let Some(p) = param.filter(|p| !p.is_empty()) {
        let terms: Vec<String> = p
            .split(',')
            .map(|t| pyval::py_strip(t).to_string())
            .filter_map(|t| {
                let (desc, f) = match t.strip_prefix('-') {
                    Some(f) => (true, f.to_string()),
                    None => (false, t.clone()),
                };
                ordering_column(&f).map(|c| format!("{c} {}", if desc { "DESC" } else { "ASC" }))
            })
            .collect();
        if !terms.is_empty() {
            return terms.join(", ");
        }
    }
    "p.processed DESC".into()
}

/// `ProcessedMailFilterSet`: `rule` (ModelChoiceFilter), `status` (exact).
async fn filters(db: &PgPool, info: &ReqInfo) -> ApiResult<(Option<i32>, Option<String>)> {
    let mut rule = None;
    if let Some(r) = info.param("rule").filter(|r| !r.is_empty()) {
        let invalid = || {
            ApiError::bad_request(json!({
                "rule": ["Select a valid choice. That choice is not one of the available choices."]
            }))
        };
        let id = pyval::parse_py_int(r).and_then(|i| i32::try_from(i).ok()).ok_or_else(invalid)?;
        let exists: Option<(i32,)> =
            sqlx::query_as("SELECT id FROM paperless_mail_mailrule WHERE id = $1").bind(id).fetch_optional(db).await?;
        if exists.is_none() {
            return Err(invalid());
        }
        rule = Some(id);
    }
    let status = info.param("status").filter(|s| !s.is_empty()).map(str::to_string);
    Ok((rule, status))
}

async fn list(State(st): State<AppState>, info: ReqInfo, user: MailUser) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::GET, PROCESSED_MAIL).await?;
    let (rule, status) = filters(db, &info).await?;
    let ct = perms::content_type_id(db, PROCESSED_MAIL).await?;
    // placeholders: $1 rule, $2 status, from $3 visibility
    let (vis, binds) = crate::util::visible_where(&user, "p", PROCESSED_MAIL, 3);
    let cond = format!("($1::int IS NULL OR p.rule_id = $1) AND ($2::text IS NULL OR p.status = $2) AND {vis}");
    let count_sql = format!("SELECT COUNT(*) FROM paperless_mail_processedmail p WHERE {cond}");
    let count: (i64,) =
        crate::util::bind_vis(sqlx::query_as(&count_sql).bind(rule).bind(&status), &user, binds, PROCESSED_MAIL, ct).fetch_one(db).await?;
    let page = listing::paginate(&info, count.0)?;
    let order = order_by(info.param("ordering"));
    let sql = format!(
        "SELECT {COLS} FROM paperless_mail_processedmail p WHERE {cond} ORDER BY {order} LIMIT {} OFFSET {}",
        page.size,
        page.offset()
    );
    let rows: Vec<ProcessedRow> =
        crate::util::bind_vis(sqlx::query_as(&sql).bind(rule).bind(&status), &user, binds, PROCESSED_MAIL, ct).fetch_all(db).await?;
    let all_ids = if info.api_version < 10 {
        let sql = format!("SELECT p.id FROM paperless_mail_processedmail p WHERE {cond} ORDER BY {order}");
        let ids: Vec<(i32,)> =
            crate::util::bind_vis(sqlx::query_as(&sql).bind(rule).bind(&status), &user, binds, PROCESSED_MAIL, ct).fetch_all(db).await?;
        Some(ids.into_iter().map(|r| r.0).collect())
    } else {
        None
    };
    let results = rows.iter().map(|r| represent(r, info.tz)).collect();
    Ok(json_response(StatusCode::OK, &listing::paginated(&info, &page, results, all_ids)))
}

async fn retrieve(State(st): State<AppState>, info: ReqInfo, user: MailUser, Path(id): Path<String>) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::GET, PROCESSED_MAIL).await?;
    // get_object(): filter_queryset → filters (rule/status) and visibility also apply here
    let (rule, status) = filters(db, &info).await?;
    let Some(pk) = crate::util::url_pk(&id)? else {
        return Err(ApiError::not_found_model(PROCESSED_MAIL.object_name));
    };
    let visible = perms::permitted_among(db, &user, PROCESSED_MAIL, "view_processedmail", &[pk]).await?;
    let sql = format!(
        "SELECT {COLS} FROM paperless_mail_processedmail p WHERE p.id = $1 \
         AND ($2::int IS NULL OR p.rule_id = $2) AND ($3::text IS NULL OR p.status = $3)"
    );
    let row: Option<ProcessedRow> = sqlx::query_as(&sql).bind(pk).bind(rule).bind(&status).fetch_optional(db).await?;
    let Some(row) = row.filter(|_| visible.contains(&pk)) else {
        return Err(ApiError::not_found_model(PROCESSED_MAIL.object_name));
    };
    let ct = perms::content_type_id(db, PROCESSED_MAIL).await?;
    perms::check_object_permission(db, &user, &Method::GET, PROCESSED_MAIL, ct, row.id, row.owner_id).await?;
    Ok(json_response(StatusCode::OK, &represent(&row, info.tz)))
}

/// POST /api/processed_mail/bulk_delete/
async fn bulk_delete(State(st): State<AppState>, _info: ReqInfo, user: MailUser, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let db = &st.inner.db;
    if !perms::has_global_perm(db, &user, "delete_processedmail").await? {
        return Err(ApiError::forbidden());
    }
    let data = parse_body(&headers, &body)?;
    // request.data.get(...) on a non-dict → AttributeError → 500
    let Value::Object(map) = &data else {
        return Err(ApiError::internal("request.data has no get()"));
    };
    let mail_ids = map.get("mail_ids").cloned().unwrap_or(Value::Array(Vec::new()));
    let bad = ApiError::Html(StatusCode::BAD_REQUEST, "mail_ids must be a list of integers");
    let Value::Array(items) = &mail_ids else {
        return Err(bad);
    };
    let mut ids = Vec::new();
    for it in items {
        match it {
            Value::Bool(b) => ids.push(*b as i64),
            Value::Number(n) if n.is_i64() || n.is_u64() => {
                if let Some(i) = n.as_i64() {
                    ids.push(i);
                }
            }
            _ => return Err(bad),
        }
    }
    let ids: Vec<i32> = ids.into_iter().filter_map(|i| i32::try_from(i).ok()).collect();
    let existing: Vec<(i32,)> =
        sqlx::query_as("SELECT id FROM paperless_mail_processedmail WHERE id = ANY($1)").bind(&ids).fetch_all(db).await?;
    let existing: Vec<i32> = existing.into_iter().map(|r| r.0).collect();
    let permitted = perms::permitted_among(db, &user, PROCESSED_MAIL, "delete_processedmail", &existing).await?;
    if existing.iter().any(|i| !permitted.contains(i)) {
        return Err(ApiError::Html(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }
    sqlx::query("DELETE FROM paperless_mail_processedmail WHERE id = ANY($1)").bind(&existing).execute(db).await?;
    Ok(json_response(StatusCode::OK, &json!({"result": "OK", "deleted_mail_ids": mail_ids})))
}

pub fn routes() -> Vec<pl_core::ApiRoute> {
    use axum::routing::{get, post};
    use pl_core::ApiRoute;
    use pl_core::route::{ALLOW_POST_ONLY, ALLOW_READ_ONLY, drf_view};
    const SRC: &str = "src/paperless_mail/views.py::ProcessedMailViewSet";
    vec![
        ApiRoute {
            id: "processed_mail.list",
            method: Method::GET,
            path: "/api/processed_mail/",
            upstream: SRC,
            handler: drf_view(get(list), ALLOW_READ_ONLY),
        },
        ApiRoute {
            id: "processed_mail.bulk_delete",
            method: Method::POST,
            path: "/api/processed_mail/bulk_delete/",
            upstream: "src/paperless_mail/views.py::ProcessedMailViewSet.bulk_delete",
            handler: drf_view(post(bulk_delete), ALLOW_POST_ONLY),
        },
        ApiRoute {
            id: "processed_mail.retrieve",
            method: Method::GET,
            path: "/api/processed_mail/{id}/",
            upstream: SRC,
            handler: drf_view(get(retrieve), ALLOW_READ_ONLY),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering() {
        assert_eq!(order_by(None), "p.processed DESC");
        assert_eq!(order_by(Some("")), "p.processed DESC");
        assert_eq!(order_by(Some("bogus")), "p.processed DESC");
        assert_eq!(order_by(Some("-received, uid,bogus")), "p.received DESC, p.uid ASC");
        assert_eq!(order_by(Some("owner")), "p.owner_id ASC");
    }
}
