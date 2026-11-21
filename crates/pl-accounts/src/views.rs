//! Pages: login, logout, account inactive, MFA step.
//!
//! upstream: allauth 65.20.1 account/views.py (`LoginView`, `LogoutView`, `AccountInactiveView`),
//! account/forms.py (`LoginForm`: `login` CharField max 150 + strip, `password` without strip,
//! `remember` is dropped because of `ACCOUNT_SESSION_REMEMBER`), account/mixins.py
//! (`RedirectAuthenticatedUserMixin`, `NextRedirectMixin`), account/adapter.py
//! (`pre_authenticate` with `login_failed` limiting, `authenticate` + rollback, error texts),
//! src/paperless/adapter.py@v3.4.2 (`pre_authenticate`: DISABLE_REGULAR_LOGIN), mfa/base/views.py
//! (`AuthenticateView`), mfa/base/forms.py (`AuthenticateForm.clean_code`), mfa/base/internal/flows.py
//! (`check_rate_limit`, `post_authentication`), mfa/totp/internal/auth.py,
//! mfa/recovery_codes/internal/auth.py, account/internal/decorators.py (`login_stage_required`),
//! account/internal/flows/logout.py.

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use pl_core::AppState;
use serde_json::{Value, json};

use crate::auth::{self, Authenticated};
use crate::flow::{self, Login};
use crate::page;
use crate::ratelimit::{self, Subject};
use crate::settings::Settings;
use crate::web::{Ctx, Internal, R, html, redirect, server_error};
use crate::{messages, tr};

/// Common frame: build context, resolve user, CSRF, view, completion.
pub async fn run<F>(st: AppState, req: Request<Body>, view: F) -> Response
where
    F: for<'a> FnOnce(&'a mut Ctx) -> std::pin::Pin<Box<dyn std::future::Future<Output = R<Response>> + Send + 'a>>,
{
    let mut ctx = match Ctx::new(st, req).await {
        Ok(c) => c,
        Err(e) => return server_error(&e),
    };
    if let Err(e) = auth::resolve_user(&mut ctx).await {
        return server_error(&e);
    }
    if let Err(reason) = ctx.check_csrf() {
        tracing::warn!(target: "django.security.csrf", "Forbidden ({reason}): {}", ctx.path);
        let resp = csrf_failure(&ctx, &reason);
        return ctx.finish(resp).await;
    }
    let resp = match view(&mut ctx).await {
        Ok(r) => r,
        Err(e) => return server_error(&e),
    };
    if resp.status() == StatusCode::NOT_IMPLEMENTED && pl_core::markers::is_delegation(&resp) {
        return resp;
    }
    ctx.finish(resp).await
}

/// `csrf_failure`: own 403 page (status and reasons as Django, texts from the own catalog).
pub fn csrf_failure(ctx: &Ctx, reason: &str) -> Response {
    let v = minijinja::context! {
        no_referer => reason == pl_authz::csrf::REASON_NO_REFERER,
        no_cookie => reason == pl_authz::csrf::REASON_NO_CSRF_COOKIE,
        LANG => ctx.lang,
    };
    match crate::tpl::render("csrf_403.html", v) {
        Ok(b) => html(StatusCode::FORBIDDEN, b),
        Err(e) => server_error(&Internal(e.to_string())),
    }
}

fn subject_ip(ip: &Option<String>) -> Subject<'_> {
    Subject { ip: ip.as_deref(), ..Default::default() }
}

/// `rate_limit(action=…)` decorator (POST only). `Some(429)` if limited.
pub async fn limit_or_429(ctx: &Ctx, action: &str) -> R<Option<Response>> {
    if !ctx.is_post() {
        return Ok(None);
    }
    let Some(ip) = ctx.client_ip()? else {
        return Ok(Some(crate::web::forbidden()));
    };
    let ip = Some(ip);
    if ratelimit::consume(&ctx.cache, action, Settings::rate_limits(action), &subject_ip(&ip), false)
        .await
        .map_err(|e| Internal(e.0))?
        .is_none()
    {
        let mut r = html(StatusCode::TOO_MANY_REQUESTS, ratelimit::HTML_429.into());
        r.headers_mut().insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("text/html"));
        return Ok(Some(r));
    }
    Ok(None)
}

/// `RedirectAuthenticatedUserMixin.dispatch` (part before the form).
pub fn redirect_authenticated(ctx: &mut Ctx) -> Option<Response> {
    if ctx.user.is_some() {
        let to = flow::login_redirect_url(ctx, page::next_url(ctx).as_deref());
        return Some(redirect(&to));
    }
    // get_pending_stage: MFA step is resumable
    if let Some(login) = flow::peek_login(ctx)
        && login.state.get("stages").and_then(|s| s.get("mfa_authenticate")).and_then(|m| m.get("handled")).is_none()
        && login.current() == Some("mfa_authenticate")
    {
        return Some(redirect(&page::url(ctx, "mfa_authenticate")));
    }
    None
}

fn field_errors_login(ctx: &Ctx, login: &str, password: Option<&str>) -> (Vec<String>, Vec<String>) {
    let required = tr!(ctx.lang, "auth.error.required").to_string();
    let mut le = Vec::new();
    let mut pe = Vec::new();
    if login.is_empty() {
        le.push(required.clone());
    } else if login.chars().count() > 150 {
        le.push(format!("Ensure this value has at most 150 characters (it has {}).", login.chars().count()));
    }
    if password.is_none_or(str::is_empty) {
        pe.push(required);
    }
    (le, pe)
}

/// `LoginView` (GET/POST `/accounts/login/`).
pub async fn login(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| Box::pin(login_view(ctx))).await
}

async fn login_view(ctx: &mut Ctx) -> R<Response> {
    if let Some(r) = limit_or_429(ctx, "login").await? {
        return Ok(r);
    }
    ctx.never_cache = true;
    social::warn_unsupported_providers(&ctx.s);
    if let Some(r) = redirect_authenticated(ctx) {
        return Ok(r);
    }
    let mut errors: Vec<Vec<String>> = Vec::new();
    if ctx.is_post() {
        let login_raw = ctx.post("login").unwrap_or("").trim().to_string();
        let password = ctx.post("password").map(str::to_string);
        let (le, pe) = field_errors_login(ctx, &login_raw, password.as_deref());
        if le.is_empty() && pe.is_empty() {
            match clean_with_password(ctx, &login_raw, password.as_deref().unwrap_or("")).await? {
                Ok((user, backend)) => {
                    let next = page::next_url(ctx);
                    let l = Login::new(ctx, user.id, next);
                    flow::record_authentication(ctx, "password", &[("username", Value::from(login_raw.clone()))]);
                    let resp = flow::perform_login(ctx, user, backend, l).await?;
                    // LoginForm._login_with_password: remember = ACCOUNT_SESSION_REMEMBER
                    let remember = !ctx.s.session_expire_at_browser_close;
                    ctx.session.set_expiry(if remember { ctx.s.session_cookie_age } else { 0 });
                    return Ok(resp);
                }
                Err(e) => errors.push(vec![e]),
            }
        } else {
            errors.push(le);
            errors.push(pe);
        }
    }
    let mut m = page::base(ctx, &errors).await?;
    // Desktop mode, request from the own computer: hint about the automatic login (the
    // key itself is never on the page – other local users can reach it too).
    let desktop_hint = crate::desktop::DesktopLogin::from_process().is_some()
        && crate::desktop::is_loopback(ctx.remote_addr.as_deref().and_then(|a| a.parse().ok()));
    m.insert("DESKTOP_HINT".into(), Value::from(desktop_hint));
    page::render("login.html", m, StatusCode::OK)
}

/// `LoginForm._clean_with_password`: `Ok((user, backend))` or error text.
async fn clean_with_password(ctx: &mut Ctx, login: &str, password: &str) -> R<Result<(pl_authz::User, &'static str), String>> {
    // paperless CustomAccountAdapter.pre_authenticate
    if ctx.s.disable_regular_login {
        return Ok(Err("Regular login is disabled".into()));
    }
    // DefaultAccountAdapter.pre_authenticate: login_failed (10/m/ip, 5/300s/key)
    let key = format!("{}:{}", ctx.host(), login.to_lowercase());
    let Some(ip) = ctx.client_ip()? else { return Err(Internal("Unable to determine client IP address".into())) };
    let subj = Subject { ip: Some(&ip), key: Some(&key), user_id: None };
    let usage = ratelimit::consume(&ctx.cache, "login_failed", Settings::rate_limits("login_failed"), &subj, false)
        .await
        .map_err(|e| Internal(e.0))?;
    let Some(usage) = usage else {
        return Ok(Err(tr!(ctx.lang, "auth.error.too_many").to_string()));
    };
    let r = auth::authenticate(ctx.st.db(), login, password, &auth::FailedLoginClient::of(ctx)).await?;
    let user = match r {
        Authenticated::Active(u, b) => Some((u, b)),
        Authenticated::Inactive(u) => Some((u, auth::ALLAUTH_BACKEND)),
        Authenticated::None => None,
    };
    match user {
        Some(u) => {
            ratelimit::rollback(&ctx.cache, &usage).await.map_err(|e| Internal(e.0))?;
            Ok(Ok(u))
        }
        None => Ok(Err(tr!(ctx.lang, "auth.error.wrong_login").to_string())),
    }
}

/// `LogoutView` (GET = POST because of `ACCOUNT_LOGOUT_ON_GET`).
pub async fn logout(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| Box::pin(logout_view(ctx))).await
}

async fn logout_view(ctx: &mut Ctx) -> R<Response> {
    if !matches!(ctx.method.as_str(), "GET" | "POST" | "HEAD") {
        return Ok(not_allowed("GET, POST, HEAD"));
    }
    let url = page::next_url(ctx).unwrap_or_else(|| ctx.s.logout_redirect_url.clone());
    do_logout(ctx);
    Ok(redirect(&url))
}

/// `flows.logout.logout(request)`.
pub fn do_logout(ctx: &mut Ctx) {
    if ctx.user.is_some() {
        let t = tr!(ctx.lang, "auth.msg.signed_out").to_string();
        ctx.messages.add(messages::SUCCESS, t);
        auth::logout(ctx);
    }
    flow::clear_login(ctx);
    ctx.session.pop("account_password_reset_by_code");
    ctx.session.pop("_password_reset_key");
}

/// `HttpResponseNotAllowed`.
pub fn not_allowed(allow: &'static str) -> Response {
    let mut r = html(StatusCode::METHOD_NOT_ALLOWED, String::new());
    r.headers_mut().insert(axum::http::header::ALLOW, axum::http::HeaderValue::from_static(allow));
    r
}

/// `AccountInactiveView` (GET).
pub async fn account_inactive(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| Box::pin(simple_page(ctx, "account_inactive.html"))).await
}

/// `TemplateView` without further logic (GET/HEAD; otherwise 405).
pub async fn simple_page(ctx: &mut Ctx, tpl: &str) -> R<Response> {
    if !matches!(ctx.method.as_str(), "GET" | "HEAD") {
        return Ok(not_allowed("GET, HEAD, OPTIONS"));
    }
    let m = page::base(ctx, &[]).await?;
    page::render(tpl, m, StatusCode::OK)
}

/// `AuthenticateView` (`/accounts/2fa/authenticate/`).
pub async fn mfa_authenticate(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| Box::pin(mfa_view(ctx))).await
}

async fn mfa_view(ctx: &mut Ctx) -> R<Response> {
    ctx.never_cache = true;
    // login_stage_required
    if ctx.user.is_some() {
        return Ok(redirect(&flow::login_redirect_url(ctx, None)));
    }
    let Some(mut login) = flow::peek_login(ctx).filter(|l| l.current() == Some("mfa_authenticate")) else {
        return Ok(redirect(&page::url(ctx, "account_login")));
    };
    let Some(uid) = login.user_id else { return Ok(redirect(&page::url(ctx, "account_login"))) };
    let Some(user) = pl_authz::user::load_user_by_id(ctx.st.db(), uid).await? else {
        return Ok(redirect(&page::url(ctx, "account_login")));
    };
    if !flow::mfa_required(ctx, uid).await? {
        return Ok(redirect(&page::url(ctx, "account_login")));
    }
    if !matches!(ctx.method.as_str(), "GET" | "HEAD" | "POST") {
        return Ok(not_allowed("GET, POST, HEAD, OPTIONS"));
    }
    let mut errors: Vec<Vec<String>> = Vec::new();
    if ctx.is_post() && ctx.post("code").is_some() {
        let code = ctx.post("code").unwrap_or("").trim().to_string();
        if code.is_empty() {
            errors.push(vec![tr!(ctx.lang, "auth.error.required").to_string()]);
        } else {
            match clean_code(ctx, uid, &code).await? {
                Ok((auth_id, auth_type)) => {
                    sqlx::query("UPDATE mfa_authenticator SET last_used_at = now() WHERE id = $1")
                        .bind(auth_id)
                        .execute(ctx.st.db())
                        .await?;
                    flow::record_authentication(ctx, "mfa", &[("id", json!(auth_id)), ("type", Value::from(auth_type))]);
                    if let Some(Value::Object(st)) = login.state.get_mut("stages")
                        && let Some(Value::Object(o)) = st.get_mut("mfa_authenticate")
                    {
                        o.insert("handled".into(), Value::Bool(true));
                    }
                    // Login.deserialize: user without .backend → adapter.login picks the allauth backend
                    return flow::resume_login(ctx, user, auth::ALLAUTH_BACKEND, &mut login).await;
                }
                Err(e) => errors.push(vec![e]),
            }
        }
    }
    let m = page::base(ctx, &errors).await?;
    page::render("mfa_authenticate.html", m, StatusCode::OK)
}

/// `BaseAuthenticateForm.clean_code`: limiting, TOTP (+ reuse lock), recovery codes.
async fn clean_code(ctx: &mut Ctx, uid: i64, code: &str) -> R<Result<(i64, String), String>> {
    let key = format!("mfa-auth-user-{uid}");
    let Some(ip) = ctx.client_ip()? else { return Err(Internal("Unable to determine client IP address".into())) };
    let subj = Subject { ip: Some(&ip), key: Some(&key), user_id: None };
    if ratelimit::consume(&ctx.cache, "login_failed", Settings::rate_limits("login_failed"), &subj, false)
        .await
        .map_err(|e| Internal(e.0))?
        .is_none()
    {
        return Ok(Err(tr!(ctx.lang, "auth.error.too_many").to_string()));
    }
    let rows: Vec<(i64, String, Value)> =
        sqlx::query_as("SELECT id::int8, type, data FROM mfa_authenticator WHERE user_id = $1 AND type <> 'webauthn' ORDER BY id")
            .bind(uid)
            .fetch_all(ctx.st.db())
            .await?;
    for (id, typ, data) in rows {
        let ok = match typ.as_str() {
            "totp" => {
                let secret = data.get("secret").and_then(Value::as_str).unwrap_or("");
                let valid = pl_auth_api::totp::validate_totp_code(secret, code).map_err(|_| Internal("bad totp secret".into()))?;
                valid && {
                    let k = format!("allauth.mfa.totp.used?user={uid}&code={}", pl_auth_api::totp::normalize_user_code(code));
                    ctx.cache.add(&k, pl_auth_api::django_cache::PICKLED_Y, 30).await.map_err(|e| Internal(e.0))?
                }
            }
            "recovery_codes" => validate_recovery_code(ctx, id, code).await?,
            _ => false,
        };
        if ok {
            ratelimit::clear(&ctx.cache, "login_failed", Settings::rate_limits("login_failed"), &subj).await.map_err(|e| Internal(e.0))?;
            return Ok(Ok((id, typ)));
        }
    }
    Ok(Err(tr!(ctx.lang, "auth.error.wrong_code").to_string()))
}

/// `RecoveryCodes.validate_code` (row locked, set bit in `used_mask`).
async fn validate_recovery_code(ctx: &Ctx, id: i64, code: &str) -> R<bool> {
    let mut tx = ctx.st.db().begin().await?;
    let row: Option<(Value,)> =
        sqlx::query_as("SELECT data FROM mfa_authenticator WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    let Some((mut data,)) = row else { return Ok(false) };
    if let Some(Value::Array(migrated)) = data.get("migrated_codes").cloned() {
        let pos = migrated.iter().position(|c| c.as_str() == Some(code));
        if let Some(p) = pos {
            let mut m = migrated;
            m.remove(p);
            data["migrated_codes"] = Value::Array(m);
            sqlx::query("UPDATE mfa_authenticator SET data = $1 WHERE id = $2").bind(&data).bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
        return Ok(false);
    }
    let seed = data.get("seed").and_then(Value::as_str).unwrap_or("").to_string();
    let mask = data.get("used_mask").and_then(Value::as_u64).unwrap_or(0);
    for (i, c) in pl_auth_api::totp::generate_recovery_codes(&seed).iter().enumerate() {
        if mask & (1 << i) != 0 {
            continue;
        }
        if pl_auth_api::totp::compare_user_code(code, c) {
            data["used_mask"] = json!(mask | (1 << i));
            sqlx::query("UPDATE mfa_authenticator SET data = $1 WHERE id = $2").bind(&data).bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
    }
    Ok(false)
}

use crate::social;
