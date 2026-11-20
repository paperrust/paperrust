//! Email addresses (`account_emailaddress`) and email confirmation via link.
//!
//! upstream: allauth 65.20.1 account/internal/emailkit.py (`valid_email_or_none`),
//! account/utils.py (`setup_user_email`, `cleanup_email_addresses`, `has_verified_email`),
//! account/adapter.py (`unstash_verified_email`, `is_email_verified`, `send_confirmation_mail`,
//! `send_mail`/`render_mail`, `add_message`), account/internal/flows/email_verification.py
//! (`send_verification_email_at_login`, `…_at_real_login`, `send_verification_email_to_address`,
//! `handle_verification_email_rate_limit`, `get_address_for_user`, `verify_email`,
//! `get_email_verification_url`), account/internal/flows/manage_email.py
//! (`assess_unique_email`, `sync_user_email_address`), account/models.py
//! (`EmailConfirmationHMAC.key`/`from_key`: `signing.dumps(pk, salt="account")`, 3 days),
//! account/internal/userkit.py (`filter_users_by_email`), templates
//! `account/email/email_confirmation_signup_*`, `account/messages/email_confirmation_sent.txt`,
//! paperless `account/email/base_message.txt`.
//!
//! paperless: `ACCOUNT_EMAIL_CONFIRMATION_HMAC` (default True) → no `account_emailconfirmation`
//! rows; `ACCOUNT_UNIQUE_EMAIL` True, `ACCOUNT_PREVENT_ENUMERATION` True,
//! `ACCOUNT_EMAIL_VERIFICATION_BY_CODE_ENABLED` False, `ACCOUNT_EMAIL_NOTIFICATIONS` False.

use serde_json::Value;

use crate::mail::{self, Mail};
use crate::ratelimit::{self, Subject};
use crate::settings::Settings;
use crate::web::{Ctx, Internal, R};
use crate::{messages, tr};

/// `app_settings.SALT`.
pub const SALT: &str = "account";
/// `EMAIL_CONFIRMATION_EXPIRE_DAYS` (3) in seconds.
pub const CONFIRMATION_MAX_AGE: i64 = 60 * 60 * 24 * 3;
pub const VERIFIED_EMAIL_SESSION_KEY: &str = "account_verified_email";

/// `valid_email_or_none(email)` (Django `validate_email`, `EmailField.max_length` 254, lowercase).
pub fn valid_email_or_none(email: Option<&str>) -> Option<String> {
    let e = email.filter(|e| !e.is_empty())?;
    if !pl_auth_api::http::is_valid_email(e) || e.chars().count() > 254 {
        return None;
    }
    Some(e.to_lowercase())
}

/// An (not yet stored) `EmailAddress`.
#[derive(Debug, Clone, PartialEq)]
pub struct EmailAddr {
    pub email: String,
    /// May come from the IdP (`email_verified`) and is then possibly not a bool.
    pub verified: Value,
    pub primary: bool,
}

/// Is a confirmation statement of the IdP (`email_verified`, `verified`) really "yes"? Only JSON
/// `true` or the text `"true"` (case-insensitive). upstream takes Python truthiness
/// (`bool(value)`), in which case the text `"false"` would also count as confirmed – deliberate deviation
/// (security review 2026-11-19, contract pl-accounts Known differences).
pub fn is_true_claim(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

impl EmailAddr {
    pub fn is_verified(&self) -> bool {
        is_true_claim(&self.verified)
    }

    /// `serialize_instance(EmailAddress(...))`.
    pub fn serialize(&self) -> Value {
        serde_json::json!({"id": null, "user_id": null, "email": self.email, "verified": self.verified, "primary": self.primary})
    }

    pub fn deserialize(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        Some(EmailAddr {
            email: o.get("email").and_then(Value::as_str).unwrap_or("").to_string(),
            verified: o.get("verified").cloned().unwrap_or(Value::Bool(false)),
            primary: o.get("primary").map(crate::social::py_truthy).unwrap_or(false),
        })
    }
}

/// `DefaultAccountAdapter.is_email_verified(request, email)` (session `account_verified_email`).
fn session_verified(ctx: &Ctx, email: &str) -> bool {
    ctx.session.get(VERIFIED_EMAIL_SESSION_KEY).and_then(Value::as_str).is_some_and(|v| v.to_lowercase() == email.to_lowercase())
}

/// `cleanup_email_addresses(request, addresses)` (account variant, with DB reconciliation).
pub async fn cleanup_email_addresses(ctx: &Ctx, addresses: Vec<EmailAddr>) -> R<(Vec<EmailAddr>, Option<String>)> {
    let mut e2a: Vec<EmailAddr> = Vec::new();
    let mut primary: Vec<String> = Vec::new();
    let mut verified: Vec<String> = Vec::new();
    let mut primary_verified: Vec<String> = Vec::new();
    for mut address in addresses {
        let Some(email) = valid_email_or_none(Some(&address.email)) else { continue };
        address.email = email.clone();
        let (exists,): (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM account_emailaddress WHERE email = $1)").bind(&email).fetch_one(ctx.st.db()).await?;
        if exists {
            continue;
        }
        let a = if let Some(a) = e2a.iter_mut().find(|a| a.email == email) {
            a.primary = a.primary || address.primary;
            if !a.is_verified() && address.is_verified() {
                a.verified = address.verified.clone();
            }
            a.clone()
        } else {
            if !address.is_verified() && session_verified(ctx, &address.email) {
                address.verified = Value::Bool(true);
            }
            e2a.push(address.clone());
            address
        };
        if a.primary {
            primary.push(a.email.clone());
            if a.is_verified() {
                primary_verified.push(a.email.clone());
            }
        }
        if a.is_verified() {
            verified.push(a.email.clone());
        }
    }
    let primary_email = primary_verified.first().or(verified.first()).or(primary.first()).cloned().or_else(|| e2a.first().map(|a| a.email.clone()));
    if let Some(p) = &primary_email {
        for a in e2a.iter_mut() {
            a.primary = a.email.to_lowercase() == p.to_lowercase();
        }
    }
    Ok((e2a, primary_email))
}

/// `setup_user_email(request, user, addresses)`: create rows (order as in allauth),
/// set `account_verified_email` in the session to `None`, align `user.email` if needed.
pub async fn setup_user_email(ctx: &mut Ctx, user_id: i64, user_email: &str, addresses: &[EmailAddr]) -> R<()> {
    let mut prio = Vec::new();
    // adapter.unstash_verified_email
    let stashed = ctx.session.get(VERIFIED_EMAIL_SESSION_KEY).and_then(Value::as_str).map(str::to_string);
    ctx.session.set(VERIFIED_EMAIL_SESSION_KEY, Value::Null);
    if let Some(s) = stashed.filter(|s| !s.is_empty()) {
        prio.push(EmailAddr { email: s.to_lowercase(), verified: Value::Bool(true), primary: true });
    }
    if !user_email.is_empty() {
        prio.push(EmailAddr { email: user_email.to_lowercase(), verified: Value::Bool(false), primary: true });
    }
    prio.extend(addresses.iter().cloned());
    let (addrs, primary) = cleanup_email_addresses(ctx, prio).await?;
    for a in &addrs {
        sqlx::query("INSERT INTO account_emailaddress (email, verified, \"primary\", user_id) VALUES ($1, $2, $3, $4)")
            .bind(&a.email)
            .bind(a.is_verified())
            .bind(a.primary)
            .bind(user_id)
            .execute(ctx.st.db())
            .await?;
    }
    if let Some(p) = primary
        && user_email.to_lowercase() != p.to_lowercase()
    {
        sqlx::query("UPDATE auth_user SET email = $1 WHERE id = $2").bind(&p).bind(user_id).execute(ctx.st.db()).await?;
    }
    Ok(())
}

/// `has_verified_email(user, email)`.
pub async fn has_verified_email(ctx: &Ctx, user_id: i64, email: Option<&str>) -> R<bool> {
    let (v,): (bool,) = match email.filter(|e| !e.is_empty()) {
        Some(e) => sqlx::query_as("SELECT COALESCE((SELECT verified FROM account_emailaddress WHERE user_id = $1 AND email = $2 LIMIT 1), false)")
            .bind(user_id)
            .bind(e.to_lowercase())
            .fetch_one(ctx.st.db())
            .await?,
        None => sqlx::query_as("SELECT EXISTS(SELECT 1 FROM account_emailaddress WHERE user_id = $1 AND verified)").bind(user_id).fetch_one(ctx.st.db()).await?,
    };
    Ok(v)
}

/// Does the user have any address at all to which a confirmation could go
/// (`get_address_for_login` ≠ None)? Only for the log note.
pub async fn has_any_address(ctx: &Ctx, user_id: i64, email: Option<&str>) -> R<bool> {
    if email.is_some_and(|e| !e.is_empty()) {
        return Ok(true);
    }
    let (n,): (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM account_emailaddress WHERE user_id = $1) OR EXISTS(SELECT 1 FROM auth_user WHERE id = $1 AND email <> '')",
    )
    .bind(user_id)
    .fetch_one(ctx.st.db())
    .await?;
    Ok(n)
}

/// `send_verification_email_at_fake_login` → `send_verification_email_to_address(address without
/// user, signup=True)` → `send_account_already_exists_mail(email)` (template
/// `account/email/account_already_exists_*` with link to the password reset), afterwards the
/// "sent" message as for a real confirmation (enumeration protection). Limit
/// `confirm_email` per address: silently skipped.
pub async fn send_account_already_exists(ctx: &mut Ctx, email: &str) -> R<bool> {
    let key = email.to_lowercase();
    let subj = Subject { ip: None, user_id: None, key: Some(&key) };
    if ratelimit::consume(&ctx.cache, "confirm_email", Settings::rate_limits("confirm_email"), &subj, false).await.map_err(|e| Internal(e.0))?.is_none() {
        return Ok(false);
    }
    let l = ctx.lang;
    let reset = absolute_uri(ctx, &crate::page::url(ctx, "account_reset_password"));
    let body = format!(
        "{}\n\n{reset}\n\n{}",
        crate::i18n::tf(l, "auth.mail.exists_text", &[("EMAIL", email)]),
        tr!(l, "auth.mail.ignore"),
    );
    let subject = tr!(l, "auth.mail.exists_subject");
    let m = Mail { from: ctx.s.default_from_email.clone(), to: email.to_string(), subject: format!("[{}] {subject}", ctx.s.site_name), body: mail_wrap(ctx, &body) };
    if let Err(e) = mail::send(&ctx.s, &m).await {
        tracing::warn!(target: "paperless.auth", host = %ctx.s.email_host, port = ctx.s.email_port, error = %e, "account-exists mail could not be sent");
    }
    // add_email_verification_sent_message: same message as for a real confirmation
    let text = crate::i18n::tf(l, "auth.msg.confirm_sent", &[("EMAIL", email)]);
    ctx.messages.add(messages::INFO, text);
    Ok(true)
}

/// `EmailVerificationSentView` (`/accounts/confirm-email/`, `account_email_verification_sent`):
/// notice page after sending (TemplateView, `login_not_required`).
pub async fn verification_sent(
    axum::extract::State(st): axum::extract::State<pl_core::AppState>,
    req: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    crate::views::run(st, req, |ctx| {
        Box::pin(async move {
            if !matches!(ctx.method.as_str(), "GET" | "HEAD") {
                return Ok(crate::views::not_allowed("GET, HEAD, OPTIONS"));
            }
            crate::socialaccount::allauth_page(ctx, "verification_sent.html", serde_json::Map::new(), axum::http::StatusCode::OK).await
        })
    })
    .await
}

/// `filter_users_by_email(email)` → count (for `assess_unique_email`, without active filter).
pub async fn users_with_email(ctx: &Ctx, email: &str, exclude_user: Option<i64>) -> R<usize> {
    let email = email.to_lowercase();
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT user_id::int8, email FROM account_emailaddress WHERE email = $1").bind(&email).fetch_all(ctx.st.db()).await?;
    let mut ids: Vec<i64> = rows.into_iter().filter(|(_, e)| unicode_ci_eq(e, &email)).map(|(u, _)| u).collect();
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id::int8, email FROM auth_user WHERE email = $1").bind(&email).fetch_all(ctx.st.db()).await?;
    ids.extend(rows.into_iter().filter(|(_, e)| unicode_ci_eq(e, &email)).map(|(u, _)| u));
    Ok(ids.into_iter().filter(|u| Some(*u) != exclude_user).count())
}

/// `unicode_ci_compare` (NFKC + casefold; for ASCII = lowercase).
fn unicode_ci_eq(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// `assess_unique_email(email)`: `Some(true)` unique, `Some(false)` taken,
/// `None` taken and enumeration protection via confirmation mail (only `mandatory`).
pub async fn assess_unique_email(ctx: &Ctx, email: &str) -> R<Option<bool>> {
    if users_with_email(ctx, email, None).await? == 0 {
        return Ok(Some(true));
    }
    if ctx.s.account_email_verification == "mandatory" {
        return Ok(None);
    }
    Ok(Some(false))
}

/// `get_address_for_user(user)`: first address by `-primary, -verified`, otherwise
/// `sync_user_email_address`.
async fn address_for_user(ctx: &Ctx, user_id: i64) -> R<Option<(i64, String, bool)>> {
    let row: Option<(i64, String, bool)> =
        sqlx::query_as("SELECT id::int8, email, verified FROM account_emailaddress WHERE user_id = $1 ORDER BY \"primary\" DESC, verified DESC LIMIT 1")
            .bind(user_id)
            .fetch_optional(ctx.st.db())
            .await?;
    if row.is_some() {
        return Ok(row);
    }
    // sync_user_email_address(user)
    let (email,): (String,) = sqlx::query_as("SELECT email FROM auth_user WHERE id = $1").bind(user_id).fetch_one(ctx.st.db()).await?;
    if email.is_empty() {
        return Ok(None);
    }
    let (primary_exists,): (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM account_emailaddress WHERE user_id = $1 AND \"primary\")").bind(user_id).fetch_one(ctx.st.db()).await?;
    let (id,): (i64,) = sqlx::query_as("INSERT INTO account_emailaddress (email, verified, \"primary\", user_id) VALUES ($1, false, $2, $3) RETURNING id::int8")
        .bind(email.to_lowercase())
        .bind(!primary_exists)
        .bind(user_id)
        .fetch_one(ctx.st.db())
        .await?;
    Ok(Some((id, email.to_lowercase(), false)))
}

/// `EmailConfirmationHMAC.key`.
pub fn confirmation_key(ctx: &Ctx, pk: i64) -> String {
    pl_authz::signing::dumps_at(&pk.to_string(), SALT, &ctx.s.secret_key, false, chrono::Utc::now().timestamp())
}

/// `get_email_verification_url` (absolute URI, `ACCOUNT_DEFAULT_HTTP_PROTOCOL=https` forces https).
pub fn verification_url(ctx: &Ctx, key: &str) -> String {
    let path = format!("{}accounts/confirm-email/{}/", ctx.s.base_url, crate::web::iri_to_uri(key));
    absolute_uri(ctx, &path)
}

/// `allauth.utils.build_absolute_uri(request, location)` without `protocol`.
pub fn absolute_uri(ctx: &Ctx, path: &str) -> String {
    let uri = ctx.absolute(path);
    if ctx.s.account_default_http_protocol.eq_ignore_ascii_case("https") {
        format!("https:{}", uri.split_once(':').map(|(_, r)| r).unwrap_or(""))
    } else {
        uri
    }
}

/// Mail in the paperless frame (`base_message.txt`).
pub fn mail_wrap(ctx: &Ctx, content: &str) -> String {
    let l = ctx.lang;
    let hello = tr!(l, "auth.mail.greeting");
    let bye = crate::i18n::tf(l, "auth.mail.signoff", &[("SITE", &ctx.s.site_name)]);
    format!("{hello}\n\n{content}\n\n{bye}").trim().to_string()
}

/// `send_verification_email_to_address(request, address, signup)` for a stored address
/// (limit `confirm_email` 1/180 s per address: silently skipped).
pub async fn send_verification_to_address(ctx: &mut Ctx, pk: i64, email: &str, username: &str, signup: bool) -> R<bool> {
    let key = email.to_lowercase();
    let subj = Subject { ip: None, user_id: None, key: Some(&key) };
    if ratelimit::consume(&ctx.cache, "confirm_email", Settings::rate_limits("confirm_email"), &subj, false).await.map_err(|e| Internal(e.0))?.is_none() {
        return Ok(false);
    }
    let l = ctx.lang;
    let ckey = confirmation_key(ctx, pk);
    let url = verification_url(ctx, &ckey);
    let host = ctx.host();
    let body = format!(
        "{}\n\n{url}\n\n{}",
        crate::i18n::tf(l, "auth.mail.confirm_text", &[("USER", username), ("HOST", &host)]),
        tr!(l, "auth.mail.ignore"),
    );
    // email_confirmation_signup_* and email_confirmation_* have the same content
    let _ = signup;
    let subject = tr!(l, "auth.mail.confirm_subject");
    let m = Mail { from: ctx.s.default_from_email.clone(), to: email.to_string(), subject: format!("[{}] {subject}", ctx.s.site_name), body: mail_wrap(ctx, &body) };
    // Mail errors (no SMTP, server unreachable) only go to the log, no 500 – as with the
    // password reset (Known differences, test log 2026-11-16). No "sent" message.
    if let Err(e) = mail::send(&ctx.s, &m).await {
        tracing::warn!(target: "paperless.auth", host = %ctx.s.email_host, port = ctx.s.email_port, error = %e, "email confirmation mail could not be sent");
        return Ok(false);
    }
    let text = crate::i18n::tf(l, "auth.msg.confirm_sent", &[("EMAIL", email)]);
    ctx.messages.add(messages::INFO, text);
    Ok(true)
}

/// `send_verification_email_at_real_login(request, login)`.
pub async fn send_verification_at_login(ctx: &mut Ctx, user_id: i64, username: &str, login_email: Option<&str>, signup: bool) -> R<bool> {
    let addr = match login_email.filter(|e| !e.is_empty()) {
        Some(e) => {
            let row: Option<(i64, String, bool)> = sqlx::query_as("SELECT id::int8, email, verified FROM account_emailaddress WHERE user_id = $1 AND email = $2")
                .bind(user_id)
                .bind(e.to_lowercase())
                .fetch_optional(ctx.st.db())
                .await?;
            row
        }
        None => address_for_user(ctx, user_id).await?,
    };
    let Some((pk, email, verified)) = addr else { return Ok(false) };
    if verified {
        return Ok(false);
    }
    send_verification_to_address(ctx, pk, &email, username, signup).await
}

/// `EmailConfirmationHMAC.from_key(key)` → (pk, email, user_id) of an **unconfirmed** address.
pub async fn from_key(ctx: &Ctx, key: &str) -> R<Option<(i64, String, i64)>> {
    let Ok((_, ts)) = pl_authz::signing::unsign_timestamped(key, SALT, &ctx.s.secret_key) else { return Ok(None) };
    if chrono::Utc::now().timestamp() - ts > CONFIRMATION_MAX_AGE {
        return Ok(None);
    }
    let Ok(v) = pl_authz::signing::loads(key, SALT, &ctx.s.secret_key) else { return Ok(None) };
    let Some(pk) = v.as_i64() else { return Ok(None) };
    let row: Option<(i64, String, i64)> = sqlx::query_as("SELECT id::int8, email, user_id::int8 FROM account_emailaddress WHERE id = $1 AND NOT verified")
        .bind(pk)
        .fetch_optional(ctx.st.db())
        .await?;
    Ok(row)
}

/// `ConfirmEmailView` (`/accounts/confirm-email/<key>/`): GET shows the confirmation page
/// (allauth layout, 200 even with an invalid key), POST confirms (`verify_email`) and redirects
/// (`next` or `get_email_verification_redirect_url`). `CONFIRM_EMAIL_ON_GET` False,
/// `LOGIN_ON_EMAIL_CONFIRMATION` False, `CHANGE_EMAIL` False.
pub async fn confirm_email(
    axum::extract::State(st): axum::extract::State<pl_core::AppState>,
    axum::extract::Path(key): axum::extract::Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    crate::views::run(st, req, move |ctx| Box::pin(confirm_view(ctx, key))).await
}

fn key_matches_url(key: &str) -> bool {
    // re_path(r"^confirm-email/(?P<key>[-:\w]+)/$")
    !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ':')
}

async fn confirm_view(ctx: &mut Ctx, key: String) -> R<axum::response::Response> {
    use axum::http::StatusCode;
    if !key_matches_url(&key) {
        // Django's URL resolver does not match this view (upstream: catch-all index view);
        // PaperRust: the 404 page of a path without a view (docs/contract/pl-accounts.md).
        return Ok(crate::web::not_found(&ctx.path));
    }
    if !matches!(ctx.method.as_str(), "GET" | "HEAD" | "POST") {
        return Ok(crate::views::not_allowed("GET, POST, HEAD, OPTIONS"));
    }
    let obj = from_key(ctx, &key).await?;
    if ctx.is_post() {
        let Some((pk, email, uid)) = obj else { return Ok(crate::web::not_found(&ctx.path)) };
        let confirmed = verify_email(ctx, pk, &email, uid).await?;
        // login_on_verification: an open verify_email step is aborted
        if let Some(l) = crate::flow::peek_login(ctx)
            && l.current() == Some("verify_email")
        {
            crate::flow::clear_login(ctx);
        }
        if confirmed {
            logout_other_user(ctx, uid);
        }
        let to = match crate::page::next_url(ctx) {
            Some(n) => n,
            None if ctx.user.is_some() => crate::flow::login_redirect_url(ctx, None),
            None => crate::page::url(ctx, "account_login"),
        };
        return Ok(crate::web::redirect(&to));
    }
    let mut extra = serde_json::Map::new();
    if let Some((pk, email, uid)) = &obj {
        logout_other_user(ctx, *uid);
        let (conflict,): (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM account_emailaddress WHERE id <> $1 AND verified AND email = $2)")
            .bind(pk)
            .bind(email)
            .fetch_one(ctx.st.db())
            .await?;
        let (username,): (String,) = sqlx::query_as("SELECT username FROM auth_user WHERE id = $1").bind(uid).fetch_one(ctx.st.db()).await?;
        extra.insert("confirmation".into(), Value::Bool(true));
        extra.insert("can_confirm".into(), Value::Bool(!conflict));
        extra.insert("email".into(), Value::from(email.clone()));
        extra.insert("user_display".into(), Value::from(username));
        extra.insert("action_url".into(), Value::from(format!("{}accounts/confirm-email/{}/", ctx.s.base_url, crate::web::iri_to_uri(&key))));
        if !conflict {
            // {% csrf_token %} only in the form (get_token sets the cookie)
            let csrf = ctx.csrf_input();
            extra.insert("csrf_input".into(), Value::from(csrf));
        }
        let rf = match ctx.param("next").filter(|n| !n.is_empty()) {
            Some(n) => format!("<input type=\"hidden\" name=\"next\" value=\"{}\">", crate::tpl::escape(n)),
            None => String::new(),
        };
        extra.insert("redirect_field".into(), Value::from(rf));
    }
    crate::socialaccount::allauth_page(ctx, "email_confirm.html", extra, StatusCode::OK).await
}

/// `ConfirmEmailView.logout_other_user`.
fn logout_other_user(ctx: &mut Ctx, uid: i64) {
    if ctx.user.as_ref().is_some_and(|u| u.id != uid) {
        crate::views::do_logout(ctx);
    }
}

/// `flows.email_verification.verify_email(request, email_address)`.
async fn verify_email(ctx: &mut Ctx, pk: i64, email: &str, uid: i64) -> R<bool> {
    let l = ctx.lang;
    let (conflict,): (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM account_emailaddress WHERE id <> $1 AND verified AND email = $2)")
        .bind(pk)
        .bind(email)
        .fetch_one(ctx.st.db())
        .await?;
    if conflict {
        let t = crate::i18n::tf(l, "auth.confirm.taken", &[("EMAIL", email)]);
        ctx.messages.add(messages::ERROR, t);
        return Ok(false);
    }
    // set_as_primary(conditional=True): only without an existing primary address
    let (has_primary,): (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM account_emailaddress WHERE user_id = $1 AND \"primary\")").bind(uid).fetch_one(ctx.st.db()).await?;
    if has_primary {
        sqlx::query("UPDATE account_emailaddress SET verified = true WHERE id = $1").bind(pk).execute(ctx.st.db()).await?;
    } else {
        sqlx::query("UPDATE account_emailaddress SET verified = true, \"primary\" = true WHERE id = $1").bind(pk).execute(ctx.st.db()).await?;
        sqlx::query("UPDATE auth_user SET email = $1 WHERE id = $2").bind(email).bind(uid).execute(ctx.st.db()).await?;
    }
    let t = crate::i18n::tf(l, "auth.msg.confirmed", &[("EMAIL", email)]);
    ctx.messages.add(messages::SUCCESS, t);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_key_charset() {
        assert!(super::key_matches_url("NDQ:1xQ0mE:iM0t1DmRCDKGs0OcgOJ-7Pv__dyUZGona4Y3KhyxxWU"));
        assert!(!super::key_matches_url("a/b"));
        assert!(!super::key_matches_url("a.b"));
        assert!(!super::key_matches_url(""));
    }

    #[test]
    fn email_cleaning() {
        assert_eq!(valid_email_or_none(Some("Foo@Example.INVALID")), Some("foo@example.invalid".into()));
        assert_eq!(valid_email_or_none(Some("nope")), None);
        assert_eq!(valid_email_or_none(Some("")), None);
        assert_eq!(valid_email_or_none(None), None);
    }

    /// Security review 2026-11-19: `email_verified` as text "false" (or 1, "yes") does
    /// not count as confirmed; only `true` or "true".
    #[test]
    fn only_real_true_is_verified() {
        let a = |v: Value| EmailAddr { email: "a@b.invalid".into(), verified: v, primary: true }.is_verified();
        assert!(a(Value::Bool(true)));
        assert!(a(Value::from("true")));
        assert!(a(Value::from("TRUE")));
        assert!(!a(Value::from("false")));
        assert!(!a(Value::from("False")));
        assert!(!a(Value::from("yes")));
        assert!(!a(Value::from(1)));
        assert!(!a(Value::Null));
        assert!(!a(Value::Bool(false)));
    }

    #[test]
    fn email_addr_roundtrip() {
        let a = EmailAddr { email: "a@b.invalid".into(), verified: Value::Bool(false), primary: true };
        let v = a.serialize();
        assert_eq!(pl_authz::signing::python_json_dumps(&v), r#"{"id":null,"user_id":null,"email":"a@b.invalid","verified":false,"primary":true}"#);
        assert_eq!(EmailAddr::deserialize(&v), Some(a));
    }
}
