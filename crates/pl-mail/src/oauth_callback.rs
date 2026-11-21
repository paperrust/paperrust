//! `GET /api/oauth/callback/` – `OauthCallbackView` (standalone, docs/STANDALONE-GAPS.md).
//!
//! upstream: src/paperless_mail/views.py@v3.4.2 (`OauthCallbackView.get`),
//! src/paperless_mail/oauth.py@v3.4.2 (`PaperlessMailOAuth2Manager`: `oauth_callback_url`,
//! `oauth_redirect_url`, `validate_state`, `get_gmail_access_token`/`get_outlook_access_token`),
//! httpx_oauth/oauth2.py@0.17.0 (`get_access_token`: `client_secret_post`, `Accept:
//! application/json`, HTTP status ≥ 400 / invalid JSON → `GetAccessTokenError`).
//!
//! The `oauth_state` session value is written by `ui_settings.get` (pl-auth-api).
//! Permissions: `IsAuthenticated` only (DRF 401/403 from pl-authz); without
//! `paperless_mail.add_mailaccount` → 400 text.

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{Duration, Utc};
use pl_authz::AuthUser;
use pl_core::{ApiError, AppState, QueryParams};
use serde_json::Value;

const INVALID: &str = "Invalid request, see logs for more detail";

fn bad_request(text: &str) -> Response {
    ApiError::django_text(StatusCode::BAD_REQUEST, text).into_response()
}

/// `HttpResponseRedirect(url)`.
fn redirect(url: &str) -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::FOUND;
    if let Ok(v) = HeaderValue::from_str(url) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    r
}

/// `oauth_redirect_url`: `f"{'http://localhost:4200/' if DEBUG else BASE_URL}mail"`.
pub fn oauth_redirect_url(cfg: &pl_core::Config) -> String {
    format!("{}mail", if cfg.debug { "http://localhost:4200/" } else { cfg.base_url.as_str() })
}

/// `oauth_callback_url` (same as pl-auth-api `ui_settings::oauth_callback_url`).
pub fn oauth_callback_url(cfg: &pl_core::Config) -> String {
    let base = cfg.oauth_callback_base_url.clone().or_else(|| cfg.paperless_url.clone()).unwrap_or_else(|| "None".into());
    format!("{base}{}api/oauth/callback/", cfg.base_url)
}

/// `request.session.get("oauth_state")`.
async fn session_oauth_state(state: &AppState, headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let cfg = state.config();
    let cookies = pl_authz::csrf::parse_cookies(headers);
    let Some(key) = pl_authz::csrf::cookie(&cookies, &cfg.session_cookie_name).map(str::to_string) else {
        return Ok(None);
    };
    if key.chars().count() < 8 || key.contains('\0') {
        return Ok(None);
    }
    let row: Option<(String,)> = sqlx::query_as("SELECT session_data FROM django_session WHERE session_key = $1 AND expire_date > now()")
        .bind(&key)
        .fetch_optional(state.db())
        .await?;
    Ok(row.and_then(|(d,)| {
        pl_authz::signing::decode_session(&d, &cfg.secret_key).get("oauth_state").and_then(Value::as_str).map(str::to_string)
    }))
}

/// Which provider the callback is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Gmail,
    Outlook,
}

/// `scope is not None and "google" in scope` → Gmail; `scope is None` → Outlook; else invalid.
pub fn provider_for_scope(scope: Option<&str>) -> Option<Provider> {
    match scope {
        Some(s) if s.contains("google") => Some(Provider::Gmail),
        None => Some(Provider::Outlook),
        Some(_) => None,
    }
}

/// `str(timezone.now())` (aware UTC datetime).
fn py_now_str(now: chrono::DateTime<Utc>) -> String {
    let us = now.timestamp_subsec_micros();
    if us == 0 { now.format("%Y-%m-%d %H:%M:%S+00:00").to_string() } else { format!("{}.{us:06}+00:00", now.format("%Y-%m-%d %H:%M:%S")) }
}

/// `get_access_token(code, redirect_uri)`; `Ok(None)` = `GetAccessTokenError`.
async fn get_access_token(url: &str, code: &str, redirect_uri: &str, client_id: &str, client_secret: &str) -> Option<Value> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ];
    let client = reqwest::Client::builder()
        .user_agent(pl_core::brand::USER_AGENT)
        .timeout(crate::oauth::OAUTH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let resp = match client.post(url).header("Accept", "application/json").form(&form).send().await {
        Ok(r) if r.status().as_u16() >= 400 => {
            tracing::error!(target: "paperless_mail", "Error getting access token from OAuth provider (HTTP {})", r.status());
            return None;
        }
        Ok(r) => r,
        Err(e) => {
            tracing::error!(target: "paperless_mail", "Error getting access token from OAuth provider: {}", e.without_url());
            return None;
        }
    };
    resp.json::<Value>().await.ok()
}

/// `GET /api/oauth/callback/`
pub async fn callback(State(state): State<AppState>, auth: AuthUser, headers: HeaderMap, RawQuery(q): RawQuery) -> Response {
    let user = auth.user();
    if !user.has_perm("paperless_mail.add_mailaccount") {
        return bad_request("You do not have permission to add mail accounts");
    }
    let params = QueryParams::parse(q.as_deref().unwrap_or(""));
    let Some(code) = params.get("code").map(str::to_string) else {
        tracing::error!(target: "paperless_mail", "Invalid oauth callback request: missing code");
        return bad_request(INVALID);
    };
    // Gmail passes scope as a query param, Outlook does not
    let scope = params.get("scope").map(str::to_string);
    let cfg = state.config().clone();
    let stored = match session_oauth_state(&state, &headers).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let given = params.get("state").unwrap_or("");
    // validate_state: settings.DEBUG or (len(state) > 0 and state == self.state)
    if !(cfg.debug || (!given.is_empty() && stored.as_deref() == Some(given))) {
        tracing::error!(target: "paperless_mail", "Invalid oauth callback request: state validation failed");
        return bad_request(INVALID);
    }
    let Some(provider) = provider_for_scope(scope.as_deref()) else {
        tracing::error!(target: "paperless_mail", "Invalid oauth callback request: unrecognized scope {:?}", scope.unwrap_or_default());
        return bad_request(INVALID);
    };
    let mail = crate::ctx::config(&cfg);
    let now = Utc::now();
    let (account_type, imap_server, name, token_url, client_id, client_secret) = match provider {
        Provider::Gmail => (
            crate::oauth::ACCOUNT_TYPE_GMAIL,
            "imap.gmail.com",
            format!("Gmail OAuth {}", py_now_str(now)),
            mail.gmail_token_url.clone(),
            mail.gmail_client_id.clone().unwrap_or_default(),
            mail.gmail_client_secret.clone().unwrap_or_default(),
        ),
        Provider::Outlook => (
            crate::oauth::ACCOUNT_TYPE_OUTLOOK,
            "outlook.office365.com",
            format!("Outlook OAuth {}", py_now_str(now)),
            mail.outlook_token_url.clone(),
            mail.outlook_client_id.clone().unwrap_or_default(),
            mail.outlook_client_secret.clone().unwrap_or_default(),
        ),
    };
    let redirect_base = oauth_redirect_url(&cfg);
    let Some(result) = get_access_token(&token_url, &code, &oauth_callback_url(&cfg), &client_id, &client_secret).await else {
        tracing::error!(target: "paperless_mail", "Error getting access token from OAuth provider");
        return redirect(&format!("{redirect_base}?oauth_success=0"));
    };
    // result["access_token"] / ["refresh_token"] / ["expires_in"]: KeyError → 500
    let (Some(access_token), Some(refresh_token), Some(expires_in)) = (
        result.get("access_token").and_then(Value::as_str),
        result.get("refresh_token").and_then(Value::as_str),
        result.get("expires_in").and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))),
    ) else {
        return ApiError::internal("KeyError in OAuth token response").into_response();
    };
    let expiration = Utc::now() + Duration::seconds(expires_in);
    // MailAccount.objects.update_or_create(password, is_token, imap_server, refresh_token,
    // expiration, owner, defaults={name, username "", imap_security SSL, imap_port 993, account_type})
    let inserted: Result<(i32,), sqlx::Error> = sqlx::query_as(
        "INSERT INTO paperless_mail_mailaccount \
         (name, imap_server, imap_port, imap_security, username, password, is_token, character_set, \
          account_type, refresh_token, expiration, owner_id) \
         VALUES ($1, $2, 993, 2, '', $3, TRUE, 'UTF-8', $4, $5, $6, $7) RETURNING id",
    )
    .bind(&name)
    .bind(imap_server)
    .bind(access_token)
    .bind(account_type)
    .bind(refresh_token)
    .bind(expiration)
    .bind(i32::try_from(user.id).unwrap_or_default())
    .fetch_one(state.db())
    .await;
    match inserted {
        Ok((id,)) => redirect(&format!("{redirect_base}?oauth_success=1&account_id={id}")),
        Err(e) => ApiError::from(e).into_response(),
    }
}

/// Route `oauth.callback`.
pub fn routes() -> Vec<pl_core::ApiRoute> {
    use axum::http::Method;
    use axum::routing::get;
    use pl_core::route::{ALLOW_READ_ONLY, drf_view};
    vec![pl_core::ApiRoute::new(
        "oauth.callback",
        Method::GET,
        "/api/oauth/callback/",
        "src/paperless_mail/views.py::OauthCallbackView.get",
        drf_view(get(callback), ALLOW_READ_ONLY),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_dispatch() {
        assert_eq!(provider_for_scope(Some("https://mail.google.com/")), Some(Provider::Gmail));
        assert_eq!(provider_for_scope(None), Some(Provider::Outlook));
        assert_eq!(provider_for_scope(Some("")), None);
        assert_eq!(provider_for_scope(Some("openid")), None);
    }

    #[test]
    fn python_datetime_str() {
        let t = chrono::DateTime::parse_from_rfc3339("2026-11-09T23:40:12.000120Z").unwrap().with_timezone(&Utc);
        assert_eq!(py_now_str(t), "2026-11-09 23:40:12.000120+00:00");
        let t = chrono::DateTime::parse_from_rfc3339("2026-11-09T23:40:12Z").unwrap().with_timezone(&Utc);
        assert_eq!(py_now_str(t), "2026-11-09 23:40:12+00:00");
    }
}
