//! History per workflow (`paperrust_workflow_run`): list with cursor pagination (newest first),
//! counters, cleanup.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::body::Bytes;
use pl_consumer::workflow_ext;
use pl_core::AppState;
use serde_json::{Value, json};
use sqlx::Row;

use crate::common::{self, VIEW_WORKFLOW, Who};
use crate::error::{AErr, json as json_resp};

pub const DEFAULT_PAGE: i64 = 50;
pub const MAX_PAGE: i64 = 200;

#[derive(serde::Deserialize, Default)]
pub struct RunQuery {
    pub cursor: Option<String>,
    pub limit: Option<String>,
    pub result: Option<String>,
    pub document: Option<String>,
}

/// GET /workflows/{id}/runs
pub async fn list(State(st): State<AppState>, who: Who, Path(id): Path<String>, Query(q): Query<RunQuery>) -> Result<Response, AErr> {
    let user = who.require(&[VIEW_WORKFLOW])?;
    let id = common::path_id(&id)?;
    let limit = match q.limit.as_deref() {
        None | Some("") => DEFAULT_PAGE,
        Some(s) => s.parse::<i64>().ok().filter(|n| (1..=MAX_PAGE).contains(n)).ok_or_else(|| AErr::field("limit", &format!("Ensure this value is between 1 and {MAX_PAGE}.")))?,
    };
    let cursor = match q.cursor.as_deref() {
        None | Some("") => None,
        Some(s) => Some(s.parse::<i64>().map_err(|_| AErr::field("cursor", "Invalid cursor."))?),
    };
    let result = match q.result.as_deref() {
        None | Some("") => None,
        Some(r @ ("ok" | "skipped" | "error")) => Some(r.to_string()),
        Some(_) => return Err(AErr::field("result", "\"result\" must be ok, skipped or error.")),
    };
    let document = match q.document.as_deref() {
        None | Some("") => None,
        Some(s) => Some(s.parse::<i32>().map_err(|_| AErr::field("document", "A valid integer is required."))?),
    };
    workflow_ext::ensure_pool(st.db()).await?;
    let mut conn = st.db().acquire().await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM documents_workflow WHERE id = $1)")
        .bind(i32::try_from(id).unwrap_or(-1))
        .fetch_one(&mut *conn)
        .await?;
    if !exists {
        return Err(AErr::NotFound);
    }
    let rows = sqlx::query(
        "SELECT r.id, r.trigger_type, r.document_id, r.run_at, r.result, r.branch, r.code, r.message, r.changed_fields, r.source \
         FROM paperrust_workflow_run r WHERE r.workflow_id = $1 \
         AND ($2::bigint IS NULL OR r.id < $2) AND ($3::text IS NULL OR r.result = $3) AND ($4::int IS NULL OR r.document_id = $4) \
         ORDER BY r.id DESC LIMIT $5",
    )
    .bind(i32::try_from(id).unwrap_or(-1))
    .bind(cursor)
    .bind(result.as_deref())
    .bind(document)
    .bind(limit + 1)
    .fetch_all(&mut *conn)
    .await?;
    let has_more = rows.len() as i64 > limit;
    let rows = &rows[..rows.len().min(limit as usize)];
    let doc_ids: Vec<i64> = rows.iter().filter_map(|r| r.try_get::<Option<i32>, _>(2).ok().flatten().map(i64::from)).collect();
    let visible = common::permitted_docs(&mut conn, user, "view_document", &doc_ids).await?;
    let ids32: Vec<i32> = visible.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let titles: std::collections::HashMap<i64, (String, bool)> = sqlx::query("SELECT id, title, deleted_at IS NOT NULL FROM documents_document WHERE id = ANY($1)")
        .bind(&ids32)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .filter_map(|r| Some((i64::from(r.try_get::<i32, _>(0).ok()?), (r.try_get::<String, _>(1).ok()?, r.try_get::<bool, _>(2).ok()?))))
        .collect();
    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        let at: chrono::DateTime<chrono::Utc> = r.try_get(3)?;
        let doc: Option<i64> = r.try_get::<Option<i32>, _>(2)?.map(i64::from);
        let document = match doc {
            None => Value::Null,
            Some(d) => match titles.get(&d) {
                Some((title, trashed)) => json!({"id": d, "title": title, "in_trash": trashed}),
                // not visible or permanently deleted: just "a document", without id/title
                None if visible.contains(&d) => json!({"id": d, "title": null, "deleted": true}),
                None => json!({"id": null, "hidden": true}),
            },
        };
        let tt = i64::from(r.try_get::<i16, _>(1)?);
        items.push(json!({
            "id": r.try_get::<i64, _>(0)?,
            "trigger_type": tt,
            "trigger_key": crate::summary::trigger_key(tt),
            "document": document,
            "run_at": at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            "result": r.try_get::<String, _>(4)?,
            "branch": r.try_get::<String, _>(5)?,
            "code": r.try_get::<String, _>(6)?,
            "message": r.try_get::<String, _>(7)?,
            "changed_fields": r.try_get::<Value, _>(8)?,
            "source": r.try_get::<Option<i16>, _>(9)?,
        }));
    }
    let next = if has_more { rows.last().and_then(|r| r.try_get::<i64, _>(0).ok()).map(|i| i.to_string()) } else { None };
    let week_start = crate::workflows::week_start_utc(*st.tz(), chrono::Utc::now());
    let c = sqlx::query(
        "SELECT count(*) FILTER (WHERE run_at >= $2 AND result <> 'skipped'), count(*) FILTER (WHERE run_at >= $2 AND result = 'error'), count(*) \
         FROM paperrust_workflow_run WHERE workflow_id = $1",
    )
    .bind(i32::try_from(id).unwrap_or(-1))
    .bind(week_start)
    .fetch_one(&mut *conn)
    .await?;
    Ok(json_resp(
        StatusCode::OK,
        &json!({
            "items": items,
            "next_cursor": next,
            "counts": {"this_week": c.try_get::<i64, _>(0)?, "errors_this_week": c.try_get::<i64, _>(1)?, "total": c.try_get::<i64, _>(2)?},
            "retention_days": workflow_ext::retention_days(),
        }),
    ))
}

/// POST /runs/cleanup `{"older_than_days"?}` – superuser; without it the configured period.
pub async fn cleanup(State(st): State<AppState>, who: Who, headers: HeaderMap, body: Bytes) -> Result<Response, AErr> {
    if !who.user().is_superuser {
        return Err(AErr::forbidden());
    }
    let b = common::body_object(&headers, &body)?;
    let days = common::opt_int(&b, "older_than_days")?.unwrap_or_else(workflow_ext::retention_days);
    if days < 1 {
        return Err(AErr::field("older_than_days", "Ensure this value is greater than or equal to 1."));
    }
    workflow_ext::ensure_pool(st.db()).await?;
    let mut conn = st.db().acquire().await?;
    let n = workflow_ext::cleanup(&mut conn, days).await?;
    Ok(json_resp(StatusCode::OK, &json!({"deleted": n, "older_than_days": days})))
}
