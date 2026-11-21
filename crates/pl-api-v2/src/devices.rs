//! Device tokens (P3b, docs/SYNC.md §8): `POST|GET /auth/devices`, `DELETE /auth/devices/{uuid}`.
//!
//! Issuing: username + password (+ TOTP `code` with MFA) → new token `prt_…`, shown exactly once.
//! Same throttle as `/api/token/` (scope `login`, `PAPERLESS_TOKEN_THROTTLE_RATE`),
//! same MFA requirement and TOTP check (codes are then also consumed for Django).

use axum::extract::{FromRequestParts, Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::device::{self, DeviceError};
use pl_authz::{Authz, MaybeUser};
use pl_core::AppState;
use pl_domain::DomainError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{V2Error, V2Result};
use crate::http::{Caller, V2Json, auth_error, json};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDevice {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub code: Option<String>,
    pub device_name: String,
}

#[derive(Debug, Serialize)]
pub struct DeviceOut {
    pub uuid: Uuid,
    pub name: String,
    pub created: chrono::DateTime<chrono::Utc>,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
}

fn dev_err(e: DeviceError) -> V2Error {
    match e {
        DeviceError::NotInstalled => V2Error(DomainError::NotInstalled("device tokens (paperrust_device_token)")),
        DeviceError::Db(e) => V2Error(DomainError::internal(e)),
    }
}

fn out(d: device::Device) -> DeviceOut {
    DeviceOut { uuid: d.uuid, name: d.name, created: d.created, last_used: d.last_used }
}

/// v1 throttle error (`pl_core::ApiError`) → v2.
fn throttle_err(e: pl_core::ApiError) -> V2Error {
    let r = e.into_response();
    if r.status() == StatusCode::TOO_MANY_REQUESTS {
        let wait = r.headers().get(axum::http::header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok());
        V2Error(DomainError::Throttled { wait })
    } else {
        V2Error(DomainError::Internal(format!("throttle check failed ({})", r.status())))
    }
}

pub async fn create(State(state): State<AppState>, req: Request) -> Response {
    match create_inner(state, req).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn create_inner(state: AppState, req: Request) -> V2Result<Response> {
    let (mut parts, body) = req.into_parts();
    // Throttle like `/api/token/` (before every password check; key = user or client address)
    let maybe = MaybeUser::from_request_parts(&mut parts, &state).await.map_err(auth_error)?;
    pl_auth_api::token::check_login_throttle(&state, &parts, maybe.user().map(|u| u.id)).await.map_err(throttle_err)?;
    let authz = parts.extensions.get::<Authz>().cloned().ok_or_else(|| V2Error(DomainError::Internal("pl_authz::Authz missing".into())))?;
    // S7: client IP (ipware over headers + TCP peer) in the failed-login log
    let headers = parts.headers.clone();
    let meta = pl_authz::RequestMeta {
        remote_addr: parts.extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0.ip()),
        ..Default::default()
    };
    let req = Request::from_parts(parts, body);
    let V2Json(b) = V2Json::<CreateDevice>::from_request(req, &state).await?;
    let name = b.device_name.trim();
    if name.is_empty() || name.chars().count() > 128 {
        return Err(V2Error(DomainError::field("device_name", "Must be 1–128 characters.")));
    }
    // F3: throttle per username (X-Forwarded-For is freely choosable)
    pl_auth_api::token::check_username_throttle(&state, &b.username).await.map_err(throttle_err)?;
    let Some(user) = authz.authenticate_credentials_from(&b.username, &b.password, &headers, &meta).await.map_err(auth_error)? else {
        return Err(V2Error(DomainError::invalid("Unable to log in with provided credentials.")));
    };
    let db = state.db();
    if pl_authz::user::is_mfa_enabled(db, user.id).await.map_err(DomainError::internal)? {
        let Some(code) = b.code.as_deref().map(str::trim).filter(|c| !c.is_empty()) else {
            return Err(V2Error(DomainError::field("code", "MFA code is required")));
        };
        let auths = pl_auth_api::db::authenticators(db, user.id, Some("totp")).await.map_err(DomainError::internal)?;
        // F6: without a TOTP secret (MFA only with other methods) never check against an empty secret
        let Some(secret) =
            auths.first().and_then(|a| a.data.get("secret")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
        else {
            return Err(V2Error(DomainError::field("code", "Invalid MFA code")));
        };
        let valid = pl_auth_api::totp::validate_totp_code(&secret, code).unwrap_or(false);
        // like allauth `_mark_code_used` (shared Django cache): code valid only once
        let fresh = valid
            && state
                .cache()
                .add(
                    &pl_auth_api::token::used_code_key(user.id, code),
                    pl_auth_api::django_cache::PICKLED_Y,
                    Some(pl_auth_api::totp::TOTP_PERIOD.max(1)),
                )
                .await
                .map_err(|e| V2Error(DomainError::Internal(format!("{e:?}"))))?;
        if !fresh {
            return Err(V2Error(DomainError::field("code", "Invalid MFA code")));
        }
    }
    let (dev, token) = device::create(db, user.id, name).await.map_err(dev_err)?;
    let body = json!({ "token": token, "device": out(dev) });
    let mut r = (StatusCode::CREATED, axum::Json(body)).into_response();
    r.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    Ok(r)
}

pub async fn list(State(state): State<AppState>, Caller(p): Caller) -> V2Result<Response> {
    let items: Vec<DeviceOut> = device::list(state.db(), p.user_id).await.map_err(dev_err)?.into_iter().map(out).collect();
    Ok(json(&json!({ "items": items })))
}

pub async fn revoke(State(state): State<AppState>, Caller(p): Caller, Path(uuid): Path<String>) -> V2Result<Response> {
    let uuid = Uuid::parse_str(&uuid).map_err(|_| V2Error(DomainError::NotFound))?;
    if device::revoke(state.db(), p.user_id, uuid).await.map_err(dev_err)? {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(V2Error(DomainError::NotFound))
    }
}

use axum::extract::FromRequest as _;
