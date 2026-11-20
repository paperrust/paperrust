//! HTTP interface `/api/x-scan/…` (PaperRust extension, no upstream counterpart).
//!
//! Permissions (server-side, pl-authz):
//! * Scanning and scanner list/capabilities: `documents.add_document` (like uploading).
//! * Manage scanners, search the network, manage devices/tokens: active superusers.
//! * View/cancel a job: only whoever started it (or a superuser).
//!
//! Only stored scanners can be used: users do not supply addresses, so that the
//! server cannot be made to connect to arbitrary addresses (SSRF).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::{AuthUser, User};
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::{Value, json};

use crate::escl::{Client, ColorMode, ScanSettings, Source, normalize_base};
use crate::job::{self, JobSpec};
use crate::{mdns, smtp, store};

fn require_superuser(auth: &AuthUser) -> Result<&User, ApiError> {
    let u = auth.user();
    if u.is_active && u.is_superuser { Ok(u) } else { Err(ApiError::permission_denied()) }
}

fn require_add(auth: &AuthUser) -> Result<&User, ApiError> {
    let u = auth.user();
    if u.is_active && u.has_perm("documents.add_document") { Ok(u) } else { Err(ApiError::permission_denied()) }
}

fn db_err(e: sqlx::Error) -> ApiError {
    tracing::error!(target: "paperrust.scan", error = %e, "scan storage failed");
    ApiError::internal("scan storage failed")
}

fn json_body(body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::parse_error(Some("JSON parse error")))?;
    if !v.is_object() {
        return Err(ApiError::validation(json!({"non_field_errors": ["Expected an object."]})));
    }
    Ok(v)
}

fn ok(ctx: &RequestContext, v: Value) -> Response {
    Drf::ok(v).indent(ctx.negotiation.indent).into_response()
}

fn scan_error(e: crate::escl::ScanError) -> ApiError {
    let status = match e {
        crate::escl::ScanError::Busy => StatusCode::CONFLICT,
        crate::escl::ScanError::Unsupported(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    };
    ApiError::json(status, json!({"code": e.code(), "detail": e.to_string()}))
}

fn parse_id(s: &str) -> Result<i64, ApiError> {
    s.parse::<i64>().map_err(|_| ApiError::not_found())
}

fn name_field(v: &Value, key: &str, required: bool) -> Result<Option<String>, ApiError> {
    match v.get(key) {
        None | Some(Value::Null) if required => Err(ApiError::validation(json!({key: ["This field is required."]}))),
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                Err(ApiError::validation(json!({key: ["This field may not be blank."]})))
            } else if s.chars().count() > 128 {
                Err(ApiError::validation(json!({key: ["Ensure this field has no more than 128 characters."]})))
            } else {
                Ok(Some(s.to_string()))
            }
        }
        _ => Err(ApiError::validation(json!({key: ["Not a valid string."]}))),
    }
}

// --------------------------------------------------------------------------------- Scanner

/// GET /api/x-scan/scanners/
pub async fn scanners_list(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    let u = require_add(&auth)?;
    let rows = store::list_scanners(state.db()).await.map_err(db_err)?;
    // Only superusers see addresses (internal network)
    let results: Vec<Value> = rows
        .iter()
        .map(|s| {
            let mut v = json!({"id": s.id, "name": s.name, "model": s.model});
            if u.is_superuser {
                v["base_url"] = json!(s.base_url);
                v["uuid"] = json!(s.uuid);
                v["created"] = json!(s.created);
            }
            v
        })
        .collect();
    Ok(ok(&ctx, json!({"count": results.len(), "results": results})))
}

/// POST /api/x-scan/scanners/ `{"name"?, "url"}` – checks the address (reads capabilities).
pub async fn scanners_create(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let u = require_superuser(&auth)?.clone();
    let v = json_body(&body)?;
    let Some(url) = v.get("url").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(ApiError::validation(json!({"url": ["This field is required."]})));
    };
    let base = normalize_base(url).map_err(|m| ApiError::validation(json!({"url": [m]})))?;
    let name = name_field(&v, "name", false)?;
    let client = Client::new(base.clone()).map_err(scan_error)?;
    let caps = client.capabilities().await.map_err(|e| ApiError::validation(json!({"url": [format!("No eSCL scanner found at this address ({e}).")]})))?;
    let name = name.or_else(|| caps.make_and_model.clone()).unwrap_or_else(|| base.host_str().unwrap_or("Scanner").to_string());
    let name: String = name.chars().take(128).collect();
    let model: Option<String> = caps.make_and_model.as_ref().map(|m| m.chars().take(128).collect());
    let uuid: Option<String> = caps.uuid.as_ref().map(|m| m.chars().take(64).collect());
    let Some(s) = store::insert_scanner(state.db(), &name, base.as_str(), uuid.as_deref(), model.as_deref(), u.id).await.map_err(db_err)? else {
        return Err(ApiError::validation(json!({"url": ["This scanner is already known."]})));
    };
    tracing::info!(target: "paperrust.scan", user = u.id, scanner = s.id, "scanner added");
    Ok(Drf::created(json!(s)).indent(ctx.negotiation.indent).into_response())
}

/// PATCH /api/x-scan/scanners/{id}/ `{"name"}`
pub async fn scanners_update(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, Path(id): Path<String>, body: Bytes) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let id = parse_id(&id)?;
    let v = json_body(&body)?;
    let Some(name) = name_field(&v, "name", true)? else { return Err(ApiError::not_found()) };
    match store::rename_scanner(state.db(), id, &name).await.map_err(db_err)? {
        Some(s) => Ok(ok(&ctx, json!(s))),
        None => Err(ApiError::not_found()),
    }
}

/// DELETE /api/x-scan/scanners/{id}/
pub async fn scanners_delete(State(state): State<AppState>, auth: AuthUser, Path(id): Path<String>) -> Result<Response, ApiError> {
    let u = require_superuser(&auth)?;
    let id = parse_id(&id)?;
    if !store::delete_scanner(state.db(), id).await.map_err(db_err)? {
        return Err(ApiError::not_found());
    }
    tracing::info!(target: "paperrust.scan", user = u.id, scanner = id, "scanner removed");
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// GET /api/x-scan/scanners/{id}/capabilities/
pub async fn scanners_capabilities(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, Path(id): Path<String>) -> Result<Response, ApiError> {
    require_add(&auth)?;
    let id = parse_id(&id)?;
    let Some(s) = store::get_scanner(state.db(), id).await.map_err(db_err)? else { return Err(ApiError::not_found()) };
    let base = normalize_base(&s.base_url).map_err(ApiError::internal)?;
    let client = Client::new(base).map_err(scan_error)?;
    let caps = client.capabilities().await.map_err(scan_error)?;
    let status = client.status().await.ok();
    Ok(ok(
        &ctx,
        json!({
            "scanner": s.id,
            "make_and_model": caps.make_and_model,
            "sources": caps.sources,
            "state": status.as_ref().map(|s| s.state.clone()),
            "adf_state": status.and_then(|s| s.adf_state),
        }),
    ))
}

/// GET /api/x-scan/discover/ – mDNS on the local network (≈ 3 s).
pub async fn discover(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let found = match mdns::discover(std::time::Duration::from_secs(3)).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(target: "paperrust.scan", error = %e, "mDNS discovery failed");
            return Ok(ok(&ctx, json!({"count": 0, "results": [], "error": "discovery_failed"})));
        }
    };
    let known: Vec<String> = store::list_scanners(state.db()).await.map_err(db_err)?.into_iter().map(|s| s.base_url).collect();
    let results: Vec<Value> = found
        .into_iter()
        .map(|f| {
            let k = known.contains(&f.base_url);
            let mut v = json!(f);
            v["known"] = json!(k);
            v
        })
        .collect();
    Ok(ok(&ctx, json!({"count": results.len(), "results": results})))
}

// --------------------------------------------------------------------------------- Jobs

/// POST /api/x-scan/jobs/ `{"scanner", "source", "duplex"?, "resolution"?, "color_mode"?, "title"?}`
pub async fn jobs_create(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let u = require_add(&auth)?.clone();
    let v = json_body(&body)?;
    let Some(scanner_id) = v.get("scanner").and_then(Value::as_i64) else {
        return Err(ApiError::validation(json!({"scanner": ["This field is required."]})));
    };
    let mut source = match v.get("source").and_then(Value::as_str) {
        None => Source::Flatbed,
        Some(s) => Source::parse(s).ok_or_else(|| ApiError::validation(json!({"source": ["Expected flatbed, adf or adf_duplex."]})))?,
    };
    if v.get("duplex").and_then(Value::as_bool) == Some(true) {
        if source == Source::Flatbed {
            return Err(ApiError::validation(json!({"duplex": ["Duplex needs the document feeder."]})));
        }
        source = Source::AdfDuplex;
    }
    let color = match v.get("color_mode") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(ColorMode::parse(s).ok_or_else(|| ApiError::validation(json!({"color_mode": ["Expected color, grayscale or bw."]})))?),
        Some(_) => return Err(ApiError::validation(json!({"color_mode": ["Expected color, grayscale or bw."]}))),
    };
    let resolution = match v.get("resolution") {
        None | Some(Value::Null) => None,
        Some(r) => Some(r.as_u64().filter(|r| (50..=2400).contains(r)).ok_or_else(|| ApiError::validation(json!({"resolution": ["Expected a resolution in dpi."]})))? as u32),
    };
    let title = name_field(&v, "title", false)?;
    let Some(s) = store::get_scanner(state.db(), scanner_id).await.map_err(db_err)? else {
        return Err(ApiError::validation(json!({"scanner": ["Unknown scanner."]})));
    };
    let Some(intake) = crate::intake::from_state(&state) else {
        return Err(ApiError::internal("no task queue configured"));
    };
    let base = normalize_base(&s.base_url).map_err(ApiError::internal)?;
    let client = Client::new(base).map_err(scan_error)?;
    let caps = client.capabilities().await.map_err(scan_error)?;
    let settings = ScanSettings::resolve(&caps, source, color, resolution).map_err(scan_error)?;
    let spec = JobSpec { scanner_id, client, settings, user_id: u.id, title, scratch: state.config().scratch_dir.clone(), intake };
    let job = job::registry().start(spec).map_err(scan_error)?;
    tracing::info!(target: "paperrust.scan", user = u.id, scanner = scanner_id, source = source.as_str(), "scan started");
    Ok(Drf::created(json!(job.view())).indent(ctx.negotiation.indent).into_response())
}

fn own_job(auth: &AuthUser, id: &str) -> Result<std::sync::Arc<job::ScanJob>, ApiError> {
    let u = require_add(auth)?;
    let id = uuid::Uuid::parse_str(id).map_err(|_| ApiError::not_found())?;
    let j = job::registry().get(id).ok_or_else(ApiError::not_found)?;
    if j.user_id != u.id && !u.is_superuser {
        return Err(ApiError::not_found());
    }
    Ok(j)
}

/// GET /api/x-scan/jobs/{id}/
pub async fn jobs_retrieve(ctx: RequestContext, auth: AuthUser, Path(id): Path<String>) -> Result<Response, ApiError> {
    let j = own_job(&auth, &id)?;
    Ok(ok(&ctx, json!(j.view())))
}

/// POST /api/x-scan/jobs/{id}/cancel/
pub async fn jobs_cancel(ctx: RequestContext, auth: AuthUser, Path(id): Path<String>) -> Result<Response, ApiError> {
    let j = own_job(&auth, &id)?;
    let requested = j.cancel();
    Ok(ok(&ctx, json!({"cancel_requested": requested, "job": j.view()})))
}

// --------------------------------------------------------------------------------- Devices

/// GET /api/x-scan/devices/ → devices + state of the SMTP intake.
pub async fn devices_list(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let rows = store::list_devices(state.db()).await.map_err(db_err)?;
    Ok(ok(&ctx, json!({"count": rows.len(), "results": rows, "smtp": smtp::info()})))
}

/// POST /api/x-scan/devices/ `{"name", "owner", "allowed_ips"?}` → device + `token` (once).
pub async fn devices_create(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let u = require_superuser(&auth)?.clone();
    let v = json_body(&body)?;
    let name = name_field(&v, "name", true)?.unwrap_or_default();
    let Some(owner) = v.get("owner").and_then(Value::as_i64) else {
        return Err(ApiError::validation(json!({"owner": ["This field is required."]})));
    };
    let allowed = match v.get("allowed_ips") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => {
            crate::net::parse_list(s).map_err(|e| ApiError::validation(json!({"allowed_ips": [format!("Invalid address or network: {e}")]})))?;
            s.trim().chars().take(2000).collect()
        }
        Some(_) => return Err(ApiError::validation(json!({"allowed_ips": ["Not a valid string."]}))),
    };
    match pl_authz::user::load_user_by_id(state.db(), owner).await.map_err(db_err)? {
        Some(o) if o.is_active && o.has_perm("documents.add_document") => {}
        Some(_) => return Err(ApiError::validation(json!({"owner": ["This user is inactive or may not add documents."]}))),
        None => return Err(ApiError::validation(json!({"owner": ["Unknown user."]}))),
    }
    let (device, token) = store::insert_device(state.db(), &name, owner, &allowed, u.id).await.map_err(db_err)?;
    tracing::info!(target: "paperrust.scan", user = u.id, device = device.id, owner, "intake device created");
    let mut out = json!(device);
    out["token"] = json!(token);
    Ok(Drf::created(out).indent(ctx.negotiation.indent).into_response())
}

/// POST /api/x-scan/devices/{id}/revoke/
pub async fn devices_revoke(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, Path(id): Path<String>) -> Result<Response, ApiError> {
    let u = require_superuser(&auth)?;
    let id = parse_id(&id)?;
    if !store::revoke_device(state.db(), id).await.map_err(db_err)? {
        return Err(ApiError::not_found());
    }
    tracing::info!(target: "paperrust.scan", user = u.id, device = id, "intake device token revoked");
    Ok(ok(&ctx, json!({"revoked": true})))
}

/// DELETE /api/x-scan/devices/{id}/ (revoked devices only)
pub async fn devices_delete(State(state): State<AppState>, auth: AuthUser, Path(id): Path<String>) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let id = parse_id(&id)?;
    if !store::delete_device(state.db(), id).await.map_err(db_err)? {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
