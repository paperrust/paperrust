//! HTTP interface `/api/x-guard/…` (PaperRust extension, no upstream counterpart).
//!
//! Permissions (server-side, pl-authz):
//! * `status`: every logged-in user (for the banner); details for superusers only.
//! * View events, lift read-only mode, backup status, start backup: active superusers.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::{AuthUser, User};
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::{Value, json};

use crate::config::{BackupConfig, GuardConfig};
use crate::{readonly, store};

fn require_superuser(auth: &AuthUser) -> Result<&User, ApiError> {
    let u = auth.user();
    if u.is_active && u.is_superuser { Ok(u) } else { Err(ApiError::permission_denied()) }
}

fn db_err(e: sqlx::Error) -> ApiError {
    tracing::error!(target: "paperless.guard", error = %e, "guard storage failed");
    ApiError::internal("guard storage failed")
}

fn ok(ctx: &RequestContext, v: Value) -> Response {
    Drf::ok(v).indent(ctx.negotiation.indent).into_response()
}

/// GET /api/x-guard/status/
pub async fn status(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    let u = auth.user();
    let s = store::state(state.db()).await.map_err(db_err)?;
    readonly::set_local(s.read_only);
    let mut v = json!({"read_only": s.read_only, "since": s.since});
    if u.is_active && u.is_superuser {
        let g = GuardConfig::from_env();
        v["reason"] = json!(s.reason);
        v["event_id"] = json!(s.event_id);
        v["guard_enabled"] = json!(g.enabled);
        v["backup_configured"] = json!(BackupConfig::from_env().ok().and_then(|c| c.target).is_some());
        let last = store::last_success(state.db()).await.map_err(db_err)?;
        v["last_backup"] = json!(last.and_then(|r| r.finished));
    }
    Ok(ok(&ctx, v))
}

/// GET /api/x-guard/events/
pub async fn events_list(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let pool = state.db();
    let s = store::state(pool).await.map_err(db_err)?;
    let events = store::list_events(pool, 50).await.map_err(db_err)?;
    let loose = store::findings(pool, None, 50).await.map_err(db_err)?;
    let g = GuardConfig::from_env();
    Ok(ok(
        &ctx,
        json!({
            "state": s,
            "guard": {
                "enabled": g.enabled, "threshold": g.threshold, "window_minutes": g.window_minutes,
                "scan_interval": g.scan_interval_secs, "sample_files": g.sample_files, "watch": g.watch,
            },
            "count": events.len(),
            "results": events,
            "recent_findings": loose,
        }),
    ))
}

/// GET /api/x-guard/events/{id}/
pub async fn events_retrieve(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, Path(id): Path<String>) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    let e = store::get_event(state.db(), id).await.map_err(db_err)?.ok_or_else(ApiError::not_found)?;
    let f = store::findings(state.db(), Some(id), 5000).await.map_err(db_err)?;
    Ok(ok(&ctx, json!({"event": e, "findings": f})))
}

/// POST /api/x-guard/lift/ `{"note"?}` – lift read-only mode, mark open events as reviewed.
pub async fn lift(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let u = require_superuser(&auth)?;
    let v: Value = if body.is_empty() { json!({}) } else { serde_json::from_slice(&body).map_err(|_| ApiError::parse_error(Some("JSON parse error")))? };
    let note: String = v.get("note").and_then(Value::as_str).unwrap_or("").chars().take(2000).collect();
    let uid = i32::try_from(u.id).ok();
    store::lift_read_only(state.db(), uid, &note).await.map_err(db_err)?;
    readonly::set_local(false);
    tracing::warn!(target: "paperless.guard", user = %u.username, "read-only mode lifted by administrator");
    let s = store::state(state.db()).await.map_err(db_err)?;
    Ok(ok(&ctx, json!({"state": s})))
}

/// GET /api/x-guard/backup/
pub async fn backup_status(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let pool = state.db();
    let _ = store::fail_stale_runs(pool, 24).await;
    let (target, error) = match BackupConfig::from_env() {
        Ok(c) => (c.target.map(|t| t.display()), None),
        Err(e) => (None, Some(e)),
    };
    let runs = store::list_runs(pool, 20).await.map_err(db_err)?;
    let last = store::last_success(pool).await.map_err(db_err)?;
    let cron = pl_core::config::var("PAPERRUST_BACKUP_CRON").unwrap_or_else(|| crate::BACKUP_CRON_DEFAULT.into());
    Ok(ok(&ctx, json!({"target": target, "config_error": error, "schedule": cron, "last_success": last, "runs": runs})))
}

/// POST /api/x-guard/backup/run/ – start a backup now (runs in the background).
pub async fn backup_run(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    let u = require_superuser(&auth)?;
    let cfg = BackupConfig::from_env().map_err(|e| ApiError::validation(json!({"non_field_errors": [e]})))?;
    if cfg.target.is_none() {
        return Err(ApiError::validation(json!({"non_field_errors": ["No backup target configured (PAPERRUST_BACKUP_TARGET)."]})));
    }
    tracing::info!(target: "paperless.backup", user = %u.username, "backup started from the web UI");
    let st = state.clone();
    tokio::spawn(async move {
        let src = crate::backup::Sources { media_root: st.config().media_root.clone(), data_dir: st.config().data_dir.clone() };
        let _ = crate::backup::run(st.db(), &src, &cfg, "manual").await;
    });
    let _ = ctx;
    Ok((StatusCode::ACCEPTED, axum::Json(json!({"started": true}))).into_response())
}
