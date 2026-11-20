//! Password reset (mail with link, check link, set new password).
//!
//! upstream: allauth 65.20.1 account/views.py (`PasswordResetView`, `PasswordResetDoneView`,
//! `PasswordResetFromKeyView`, `PasswordResetFromKeyDoneView`), account/forms.py
//! (`ResetPasswordForm`, `ResetPasswordKeyForm`, `UserTokenForm`,
//! `EmailAwarePasswordResetTokenGenerator`), account/internal/flows/password_reset.py,
//! account/internal/flows/signup.py (`send_unknown_account_mail`), account/internal/userkit.py
//! (`filter_users_by_email`), account/internal/flows/manage_email.py (`sync_user_email_address`
//! – creates an `account_emailaddress` row if needed when generating the token), templates
//! `account/email/password_reset_key_*`, `unknown_account_*` + paperless `base_message.txt`,
//! src/paperless/adapter.py@v3.4.2 (`get_reset_password_from_key_url` with `PAPERLESS_URL`),
//! django/contrib/auth/tokens.py@5.2 (`PasswordResetTokenGenerator`).

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use chrono::{NaiveDate, Utc};
use pl_authz::User;
use pl_core::AppState;
use serde_json::{Value, json};

use crate::flow::{base36_to_int, int_to_base36};
use crate::mail::{self, Mail};
use crate::page;
use crate::ratelimit::{self, Subject};
use crate::settings::{PASSWORD_RESET_TIMEOUT, Settings};
use crate::views::{limit_or_429, not_allowed, run, simple_page};
use crate::web::{Ctx, Internal, R, redirect};
use crate::{messages, tr};

pub const INTERNAL_RESET_SESSION_KEY: &str = "_password_reset_key";
const RESET_URL_KEY: &str = "set-password";
const KEY_SALT: &str = "django.contrib.auth.tokens.PasswordResetTokenGenerator";

/// Seconds since 2001-01-01 in the local time of `TIME_ZONE` (`datetime.now()`).
fn num_seconds_now(ctx: &Ctx) -> i64 {
    let local = Utc::now().with_timezone(ctx.st.tz()).naive_local();
    let base = NaiveDate::from_ymd_opt(2001, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).unwrap_or_default();
    (local - base).num_seconds()
}

async fn user_emails(ctx: &Ctx, u: &User) -> R<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT email FROM account_emailaddress WHERE user_id = $1").bind(u.id).fetch_all(ctx.st.db()).await?;
    let mut set: std::collections::BTreeSet<String> = rows.into_iter().map(|r| r.0).collect();
    if !u.email.is_empty() {
        set.insert(u.email.clone());
    }
    Ok(set.into_iter().collect())
}

/// `_make_hash_value` (Django + allauth email addition).
async fn hash_value(ctx: &Ctx, u: &User, ts: i64) -> R<String> {
    // sync_user_email_address(user)
    if !u.email.is_empty() {
        sqlx::query(
            "INSERT INTO account_emailaddress (user_id, email, verified, \"primary\") \
             SELECT $1, $2, false, false WHERE NOT EXISTS (SELECT 1 FROM account_emailaddress WHERE user_id = $1 AND email = $2)",
        )
        .bind(u.id)
        .bind(&u.email)
        .execute(ctx.st.db())
        .await?;
    }
    let login_ts = u.last_login.map(|t| t.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default();
    let emails = user_emails(ctx, u).await?;
    Ok(format!("{}{}{}{}{}{}", u.id, u.password, login_ts, ts, u.email, emails.join("|")))
}

async fn make_token_with_timestamp(ctx: &Ctx, u: &User, ts: i64) -> R<String> {
    let v = hash_value(ctx, u, ts).await?;
    let mac = pl_authz::signing::salted_hmac_sha256(KEY_SALT, v.as_bytes(), &ctx.s.secret_key);
    let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
    let short: String = hex.chars().step_by(2).collect();
    Ok(format!("{}-{short}", int_to_base36(ts)))
}

/// `default_token_generator.make_token(user)`.
pub async fn make_token(ctx: &Ctx, u: &User) -> R<String> {
    make_token_with_timestamp(ctx, u, num_seconds_now(ctx)).await
}

/// `check_token(user, token)`.
pub async fn check_token(ctx: &Ctx, u: &User, token: &str) -> R<bool> {
    let parts: Vec<&str> = token.split('-').collect();
    let [ts_b36, _] = parts.as_slice() else { return Ok(false) };
    let Some(ts) = base36_to_int(ts_b36) else { return Ok(false) };
    let expected = make_token_with_timestamp(ctx, u, ts).await?;
    if !pl_authz::signing::constant_time_eq(expected.as_bytes(), token.as_bytes()) {
        return Ok(false);
    }
    Ok(num_seconds_now(ctx) - ts <= PASSWORD_RESET_TIMEOUT)
}

/// `filter_users_by_email(email, is_active=True, prefer_verified=True, for_login=True)`.
async fn users_by_email(ctx: &Ctx, email: &str) -> R<Vec<User>> {
    let email = email.to_lowercase();
    let rows: Vec<(i64, String, bool)> =
        sqlx::query_as("SELECT user_id::int8, email, verified FROM account_emailaddress WHERE email = $1 ORDER BY id")
            .bind(&email)
            .fetch_all(ctx.st.db())
            .await?;
    let mut mails: Vec<(i64, bool)> = rows.into_iter().filter(|(_, e, _)| e.to_lowercase() == email).map(|(u, _, v)| (u, v)).collect();
    let verified: Vec<(i64, bool)> = mails.iter().copied().filter(|(_, v)| *v).collect();
    let is_verified = !verified.is_empty();
    if is_verified {
        mails = verified;
    }
    let mut ids: Vec<i64> = mails.into_iter().map(|(u, _)| u).collect();
    if !is_verified {
        let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id::int8, email FROM auth_user WHERE email = $1").bind(&email).fetch_all(ctx.st.db()).await?;
        ids.extend(rows.into_iter().filter(|(_, e)| e.to_lowercase() == email).map(|(i, _)| i));
    }
    ids.sort_unstable();
    ids.dedup();
    let mut out = Vec::new();
    for id in ids {
        if let Some(u) = pl_authz::user::load_user_by_id(ctx.st.db(), id).await?
            && u.is_active
        {
            out.push(u);
        }
    }
    Ok(out)
}

fn mail_wrap(ctx: &Ctx, content: &str) -> String {
    let l = ctx.lang;
    let hello = tr!(l, "auth.mail.greeting");
    let bye = crate::i18n::tf(l, "auth.mail.signoff", &[("SITE", &ctx.s.site_name)]);
    format!("{hello}\n\n{content}\n\n{bye}").trim().to_string()
}

fn subject(ctx: &Ctx, s: &str) -> String {
    format!("[{}] {s}", ctx.s.site_name)
}

/// `get_reset_password_from_key_url` (paperless: `PAPERLESS_URL` + path, otherwise absolute URI with
/// `ACCOUNT_DEFAULT_HTTP_PROTOCOL`).
fn from_key_url(ctx: &Ctx, key: &str) -> String {
    let path = format!("{}accounts/password/reset/key/{}/", ctx.s.base_url, pl_auth_api::totp::py_quote(key, "/"));
    match &ctx.s.paperless_url {
        Some(u) => format!("{u}{path}"),
        None => format!("{}://{}{path}", ctx.s.account_default_http_protocol, ctx.host()),
    }
}

async fn send_reset_mails(ctx: &Ctx, email: &str, users: &[User]) -> R<()> {
    let l = ctx.lang;
    if users.is_empty() {
        if !ctx.s.account_email_unknown_accounts {
            return Ok(());
        }
        let signup = format!("{}://{}{}", ctx.s.account_default_http_protocol, ctx.host(), page::url(ctx, "account_signup"));
        let body = format!(
            "{}\n\n{}\n\n{}\n\n{signup}",
            crate::i18n::tf(l, "auth.mail.unknown_text", &[("EMAIL", email)]),
            tr!(l, "auth.mail.ignore"),
            tr!(l, "auth.mail.unknown_signup"),
        );
        let m = Mail { from: ctx.s.default_from_email.clone(), to: email.to_string(), subject: subject(ctx, tr!(l, "auth.mail.unknown_subject")), body: mail_wrap(ctx, &body) };
        deliver(&ctx.s, &m).await;
        return Ok(());
    }
    for u in users {
        let token = make_token(ctx, u).await?;
        let key = format!("{}-{token}", int_to_base36(u.id));
        let url = from_key_url(ctx, &key);
        let mut body = format!("{}\n\n{url}", tr!(l, "auth.mail.reset_text"));
        if !u.username.is_empty() {
            body.push_str("\n\n");
            body.push_str(&crate::i18n::tf(l, "auth.mail.reset_username", &[("USERNAME", &u.username)]));
        }
        body.push_str("\n\n");
        body.push_str(tr!(l, "auth.mail.ignore"));
        let m = Mail { from: ctx.s.default_from_email.clone(), to: email.to_string(), subject: subject(ctx, tr!(l, "auth.mail.reset_subject")), body: mail_wrap(ctx, &body) };
        deliver(&ctx.s, &m).await;
    }
    Ok(())
}

/// Send the mail; an error (no SMTP, server unreachable, rejection) only ends up in the log.
/// Deviation from upstream (A-011/B-313): allauth lets the exception through (500). The page
/// always shows "email sent" so nobody learns whether the address exists or whether the mail
/// went out. Only host/port and the short error text are logged (`mail::send` returns no
/// credentials), never recipient, link or password.
async fn deliver(s: &Settings, m: &Mail) -> bool {
    match mail::send(s, m).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(target: "paperless.auth", host = %s.email_host, port = s.email_port, error = %e, "password reset mail could not be sent");
            false
        }
    }
}

/// `PasswordResetView` (`/accounts/password/reset/`).
pub async fn password_reset(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| Box::pin(reset_view(ctx))).await
}

async fn reset_view(ctx: &mut Ctx) -> R<Response> {
    let mut errors: Vec<Vec<String>> = Vec::new();
    if ctx.is_post() {
        let raw = ctx.post("email").unwrap_or("").trim().to_string();
        if raw.is_empty() {
            errors.push(vec![tr!(ctx.lang, "auth.error.required").to_string()]);
        } else if !pl_auth_api::http::is_valid_email(&raw) {
            errors.push(vec![tr!(ctx.lang, "auth.error.email_invalid").to_string()]);
        } else {
            let email = raw.to_lowercase();
            let users = users_by_email(ctx, &email).await?;
            let ip = ctx.client_ip()?;
            let subj = Subject { ip: ip.as_deref(), key: Some(&email), user_id: None };
            if ratelimit::consume(&ctx.cache, "reset_password", Settings::rate_limits("reset_password"), &subj, false).await.map_err(|e| Internal(e.0))?.is_none() {
                let mut r = crate::web::html(StatusCode::TOO_MANY_REQUESTS, ratelimit::HTML_429.into());
                r.headers_mut().insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("text/html"));
                return Ok(r);
            }
            send_reset_mails(ctx, &email, &users).await?;
            let to = page::next_url(ctx).unwrap_or_else(|| page::url(ctx, "account_reset_password_done"));
            return Ok(redirect(&to));
        }
    } else if !matches!(ctx.method.as_str(), "GET" | "HEAD") {
        return Ok(not_allowed("GET, POST, PUT, HEAD, OPTIONS"));
    }
    let mut m = page::base(ctx, &errors).await?;
    m.insert("form".into(), json!({"email": {"name": "email"}}));
    page::render("password_reset.html", m, StatusCode::OK)
}

/// `PasswordResetDoneView`.
pub async fn password_reset_done(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| Box::pin(simple_page(ctx, "password_reset_done.html"))).await
}

/// `PasswordResetFromKeyDoneView`.
pub async fn from_key_done(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| Box::pin(simple_page(ctx, "password_reset_from_key_done.html"))).await
}

/// `PasswordResetFromKeyView` (`/accounts/password/reset/key/<uidb36>-<key>/`).
pub async fn from_key(State(st): State<AppState>, Path(raw): Path<String>, req: Request<Body>) -> Response {
    run(st, req, move |ctx| Box::pin(from_key_view(ctx, raw))).await
}

async fn token_user(ctx: &Ctx, uidb36: &str, key: &str) -> R<Option<User>> {
    if key.is_empty() {
        return Ok(None);
    }
    let Some(id) = base36_to_int(uidb36) else { return Ok(None) };
    let Some(u) = pl_authz::user::load_user_by_id(ctx.st.db(), id).await? else { return Ok(None) };
    Ok(check_token(ctx, &u, key).await?.then_some(u))
}

async fn from_key_view(ctx: &mut Ctx, raw: String) -> R<Response> {
    // ^(?P<uidb36>[0-9A-Za-z]+)-(?P<key>.+)$
    let Some((uidb36, url_key)) = raw.split_once('-').filter(|(u, k)| !u.is_empty() && u.bytes().all(|b| b.is_ascii_alphanumeric()) && !k.is_empty()) else {
        return Ok(crate::web::not_found(&ctx.path));
    };
    let (uidb36, url_key) = (uidb36.to_string(), url_key.to_string());
    if let Some(r) = limit_or_429(ctx, "reset_password_from_key").await? {
        return Ok(r);
    }
    if url_key == RESET_URL_KEY {
        let key = ctx.session.get(INTERNAL_RESET_SESSION_KEY).and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(u) = token_user(ctx, &uidb36, &key).await? {
            if ctx.user.as_ref().is_some_and(|cur| cur.id != u.id) {
                crate::views::do_logout(ctx);
                ctx.session.set(INTERNAL_RESET_SESSION_KEY, Value::from(key.clone()));
            }
            return set_password_form(ctx, u).await;
        }
    } else if token_user(ctx, &uidb36, &url_key).await?.is_some() {
        ctx.session.set(INTERNAL_RESET_SESSION_KEY, Value::from(url_key.clone()));
        let to = page::passthrough(ctx, &ctx.path.replace(&url_key, RESET_URL_KEY));
        return Ok(redirect(&to));
    }
    let mut m = page::base(ctx, &[]).await?;
    m.insert("token_fail".into(), Value::Bool(true));
    page::render("password_reset_from_key.html", m, StatusCode::OK)
}

async fn set_password_form(ctx: &mut Ctx, u: User) -> R<Response> {
    let mut errors: Vec<Vec<String>> = Vec::new();
    if ctx.is_post() {
        let req = tr!(ctx.lang, "auth.error.required").to_string();
        let p1 = ctx.post("password1").unwrap_or("").to_string();
        let p2 = ctx.post("password2").unwrap_or("").to_string();
        let mut e1 = Vec::new();
        let mut e2 = Vec::new();
        if p1.is_empty() {
            e1.push(req.clone());
        } else {
            let attrs = pl_auth_api::password::UserAttrs { username: u.username.clone(), first_name: u.first_name.clone(), last_name: u.last_name.clone(), email: u.email.clone() };
            e1.extend(pl_auth_api::password::validate_password(&p1, Some(&attrs)));
        }
        if p2.is_empty() {
            e2.push(req);
        }
        if e1.is_empty() && e2.is_empty() && p1 != p2 {
            e2.push(tr!(ctx.lang, "auth.error.password_mismatch").to_string());
        }
        if e1.is_empty() && e2.is_empty() {
            let pw = p1.clone();
            let enc = tokio::task::spawn_blocking(move || pl_authz::hashers::make_password(&pw)).await?;
            pl_authz::user::update_password(ctx.st.db(), u.id, &enc).await?;
            // PaperRust: revoke device tokens (`prt_…`); DRF token stays as upstream
            // (security review 2026-11-19 F13).
            pl_authz::device::revoke_all_for_user(ctx.st.db(), u.id).await?;
            // finalize_password_reset: delete login_failed counter per email address
            let rows: Vec<(String,)> = sqlx::query_as("SELECT email FROM account_emailaddress WHERE user_id = $1").bind(u.id).fetch_all(ctx.st.db()).await?;
            let ip = ctx.client_ip()?;
            for (email,) in rows {
                let key = format!("{}:{}", ctx.host(), email.to_lowercase());
                let subj = Subject { ip: ip.as_deref(), key: Some(&key), user_id: None };
                ratelimit::clear(&ctx.cache, "login_failed", Settings::rate_limits("login_failed"), &subj).await.map_err(|e| Internal(e.0))?;
            }
            let t = tr!(ctx.lang, "auth.msg.password_changed").to_string();
            ctx.messages.add(messages::SUCCESS, t);
            let to = page::next_url(ctx).unwrap_or_else(|| page::url(ctx, "account_reset_password_from_key_done"));
            return Ok(redirect(&to));
        }
        errors.push(e1);
        errors.push(e2);
    }
    let mut m = page::base(ctx, &errors).await?;
    m.insert("token_fail".into(), Value::Bool(false));
    m.insert("form".into(), json!({"password1": {"name": "password1"}, "password2": {"name": "password2"}}));
    page::render("password_reset_from_key.html", m, StatusCode::OK)
}

#[cfg(test)]
mod tests {
    /// A-011: an unreachable SMTP server is not an error of the request (log only).
    #[tokio::test]
    async fn unreachable_smtp_is_logged_not_raised() {
        // occupy a free port and release it again → connection is refused
        let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(1);
        let env = pl_core::config::Env::from_pairs([("PAPERLESS_SECRET_KEY", "unit-test-only-secret-key-0123456789abcdef".to_string()), ("PAPERLESS_EMAIL_HOST", "127.0.0.1".to_string()), ("PAPERLESS_EMAIL_PORT", port.to_string())]);
        let cfg = pl_core::Config::from_env_map(env).expect("config");
        let s = super::Settings::from_core(&cfg);
        let m = super::Mail { from: "a@localhost".into(), to: "niemand@example.invalid".into(), subject: "x".into(), body: "y".into() };
        assert!(!super::deliver(&s, &m).await);
    }

    #[test]
    fn token_shape() {
        // Django: int_to_base36(seconds since 2001-01-01)
        assert_eq!(super::int_to_base36(817_000_000).len(), 6);
    }
}
