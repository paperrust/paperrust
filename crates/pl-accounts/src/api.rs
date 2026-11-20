//! DRF routes of this crate.
//!
//! upstream: src/paperless/views.py@v3.4.2 (`SocialAccountProvidersView`: `IsAuthenticated`,
//! `GET` → sorted list `{"name", "login_url"}`).

use axum::extract::{FromRequestParts, Request, State};
use axum::response::{IntoResponse, Response};
use pl_authz::AuthUser;
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::Value;

use crate::social;
use crate::web;

/// `GET /api/profile/social_account_providers/`.
pub async fn social_account_providers(State(st): State<AppState>, req: Request) -> Response {
    let (mut parts, _body) = req.into_parts();
    // initial(): content negotiation/version, authentication, IsAuthenticated
    if let Err(e) = RequestContext::from_request_parts(&mut parts, &st).await {
        return e.into_response();
    }
    if let Err(e) = AuthUser::from_request_parts(&mut parts, &st).await {
        return e.into_response();
    }
    let s = web::settings(&st);
    match social::api_providers(&st, &s).await {
        Ok(Some(list)) => Drf::ok(Value::Array(list)).into_response(),
        // no longer occurs since S2 (unknown providers are skipped); upstream: uncaught → 500
        Ok(None) => ApiError::internal("social account provider without a provider class").into_response(),
        Err(e) => ApiError::internal(e.0).into_response(),
    }
}
