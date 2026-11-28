//! Management of the MCP token scopes (`/api/x-ai/mcp-scopes/`, active superusers only).
//!
//! * `GET` → `{"count", "results": [{id, token_key_prefix, user_id, scopes, label, created}]}`
//! * `POST {"token": "<plaintext token>" | "user_id": <id>, "scopes": ["read", …], "label": "…"}`
//!   → creates or replaces (per token or per user); only the SHA-256 of the token is stored.
//! * `DELETE /{id}/` → 204.
//!
//! Every change lands in the AI audit log (`channel = 'ui'`, `action = 'mcp_scope_set'/'mcp_scope_delete'`)
//! in the **same transaction** as the change: if the audit entry cannot be written, the change is
//! rolled back and the request fails (external review, finding 9).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::{AuthUser, User};
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::{Value, json};

use crate::mcp::scopes;
use crate::pg::{DELETE_SCOPE_SQL, PgSideStore, UPSERT_SCOPE_TOKEN_SQL, UPSERT_SCOPE_USER_SQL};

fn require_superuser(auth: &AuthUser) -> Result<&User, ApiError> {
    let u = auth.user();
    if u.is_active && u.is_superuser { Ok(u) } else { Err(ApiError::permission_denied()) }
}

fn storage_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::Database(d) if d.code().as_deref() == Some("42P01")) {
        return ApiError::with_detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "MCP scope storage is not installed (migration 0002_paperrust_ai_mcp_scope.sql).",
        );
    }
    tracing::error!(target: "pl_ai", error = %e, "mcp scope storage failed");
    ApiError::internal("mcp scope storage failed")
}

/// Audit entry on the change's transaction; an error aborts the change.
pub const AUDIT_SQL: &str = "INSERT INTO paperrust_ai_audit (actor_id, channel, action, object_kind, object_id, after) \
     VALUES ($1, 'ui', $2, 'mcp_scope', $3, $4)";

async fn audit(conn: &mut sqlx::PgConnection, user: &User, action: &'static str, id: i64, after: &Value) -> Result<(), sqlx::Error> {
    sqlx::query(AUDIT_SQL)
        .bind(i32::try_from(user.id).ok())
        .bind(action)
        .bind(i32::try_from(id).ok())
        .bind(after)
        .execute(&mut *conn)
        .await
        .map(|_| ())
}

/// `PgSideStore::upsert_scope` on the caller's transaction.
async fn upsert_scope(
    conn: &mut sqlx::PgConnection,
    token_key: Option<&str>,
    user_id: Option<i64>,
    scopes: &[&str],
    label: Option<&str>,
    by: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let list = scopes.join(",");
    let by = by.and_then(|b| i32::try_from(b).ok());
    match token_key {
        Some(k) => sqlx::query_scalar(UPSERT_SCOPE_TOKEN_SQL).bind(k).bind(&list).bind(label).bind(by).fetch_one(&mut *conn).await,
        None => {
            sqlx::query_scalar(UPSERT_SCOPE_USER_SQL)
                .bind(user_id.and_then(|u| i32::try_from(u).ok()))
                .bind(&list)
                .bind(label)
                .bind(by)
                .fetch_one(&mut *conn)
                .await
        }
    }
}

/// GET /api/x-ai/mcp-scopes/
pub async fn list(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let rows = PgSideStore::new(state.db().clone()).list_scopes().await.map_err(storage_err)?;
    Ok(Drf::ok(json!({"count": rows.len(), "results": rows})).indent(ctx.negotiation.indent).into_response())
}

/// POST /api/x-ai/mcp-scopes/
pub async fn create(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let user = require_superuser(&auth)?.clone();
    let v: Value = serde_json::from_slice(&body).map_err(|_| ApiError::parse_error(Some("JSON parse error")))?;
    let token = v.get("token").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    let user_id = v.get("user_id").and_then(Value::as_i64);
    if token.is_some() == user_id.is_some() {
        return Err(ApiError::validation(json!({"non_field_errors": ["Give exactly one of 'token' or 'user_id'."]})));
    }
    let Some(list) = v.get("scopes").and_then(Value::as_array) else {
        return Err(ApiError::validation(json!({"scopes": ["This field is required."]})));
    };
    let mut wanted: Vec<&'static str> = Vec::new();
    for s in list {
        match s.as_str().and_then(|s| scopes::GROUPS.iter().find(|g| g.eq_ignore_ascii_case(s.trim()))) {
            Some(g) if !wanted.contains(g) => wanted.push(g),
            Some(_) => {}
            None => return Err(ApiError::validation(json!({"scopes": [format!("Unknown scope {s}; allowed: read, write, ingest.")]}))),
        }
    }
    let label: Option<String> = v.get("label").and_then(Value::as_str).map(|l| l.chars().take(128).collect());
    let key = token.map(scopes::token_key);
    if let Some(u) = user_id
        && pl_authz::user::load_user_by_id(state.db(), u).await.map_err(storage_err)?.is_none()
    {
        return Err(ApiError::validation(json!({"user_id": ["Unknown user."]})));
    }
    let mut tx = state.db().begin().await.map_err(storage_err)?;
    let id = upsert_scope(&mut tx, key.as_deref(), user_id, &wanted, label.as_deref(), Some(user.id)).await.map_err(storage_err)?;
    let out = json!({
        "id": id,
        "token_key_prefix": key.as_ref().map(|k| k.chars().take(8).collect::<String>()),
        "user_id": user_id,
        "scopes": wanted,
        "label": label,
    });
    audit(&mut tx, &user, "mcp_scope_set", id, &out).await.map_err(storage_err)?;
    tx.commit().await.map_err(storage_err)?;
    Ok(Drf::created(out).indent(ctx.negotiation.indent).into_response())
}

/// DELETE /api/x-ai/mcp-scopes/{id}/
pub async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<String>) -> Result<Response, ApiError> {
    let user = require_superuser(&auth)?.clone();
    let Ok(id) = id.parse::<i64>() else { return Err(ApiError::not_found()) };
    let mut tx = state.db().begin().await.map_err(storage_err)?;
    let n = sqlx::query(DELETE_SCOPE_SQL).bind(id).execute(&mut *tx).await.map_err(storage_err)?;
    if n.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    audit(&mut tx, &user, "mcp_scope_delete", id, &json!({"deleted": id})).await.map_err(storage_err)?;
    tx.commit().await.map_err(storage_err)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding 9: the audit write is part of the change (same transaction, errors propagate);
    /// before the fix its result was discarded (`let _ = store.audit(…)`, which itself only logged).
    #[test]
    fn audit_is_transactional() {
        assert!(AUDIT_SQL.starts_with("INSERT INTO paperrust_ai_audit"), "{AUDIT_SQL}");
        assert!(AUDIT_SQL.contains("'ui'") && AUDIT_SQL.contains("'mcp_scope'"), "{AUDIT_SQL}");
    }
}
