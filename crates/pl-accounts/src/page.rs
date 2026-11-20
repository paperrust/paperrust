//! Shared template context (`documents.context_processors.settings`, `messages`,
//! `get_entrance_context_data`, `{% get_providers %}`).
//!
//! upstream: src/documents/context_processors.py@v3.4.2, src/paperless/config.py
//! (`GeneralConfig`), allauth 65.20.1 account/internal/templatekit.py, account/utils.py
//! (`passthrough_next_redirect_url`), socialaccount/templatetags/socialaccount.py
//! (`provider_login_url`), socialaccount/adapter.py (`list_apps`/`list_providers`).

use axum::http::StatusCode;
use axum::response::Response;
use minijinja::Value as J;
use serde_json::{Map, Value, json};

use crate::social;
use crate::web::{Ctx, R, html};

/// `reverse(...)` paths (with `FORCE_SCRIPT_NAME`).
pub fn url(ctx: &Ctx, name: &str) -> String {
    let p = match name {
        "account_login" => "accounts/login/",
        "account_logout" => "accounts/logout/",
        "account_signup" => "accounts/signup/",
        "account_inactive" => "accounts/account_inactive/",
        "account_reset_password" => "accounts/password/reset/",
        "account_reset_password_done" => "accounts/password/reset/done/",
        "account_reset_password_from_key_done" => "accounts/password/reset/key/done/",
        "mfa_authenticate" => "accounts/2fa/authenticate/",
        "account_email_verification_sent" => "accounts/confirm-email/",
        "socialaccount_signup" => "accounts/3rdparty/signup/",
        "socialaccount_login_cancelled" => "accounts/3rdparty/login/cancelled/",
        "socialaccount_login_error" => "accounts/3rdparty/login/error/",
        "base" => "",
        _ => "",
    };
    format!("{}{p}", ctx.s.base_url)
}

/// `add_query_params(url, {"next": next})` – Python `urlencode` (quote_plus).
pub fn add_next(url: &str, next: &str) -> String {
    let q: String = url::form_urlencoded::Serializer::new(String::new()).append_pair("next", next).finish();
    if url.contains('?') { format!("{url}&{q}") } else { format!("{url}?{q}") }
}

/// paperless `CustomAccountAdapter.is_safe_url` (ALLOWED_HOSTS without `*` + current host).
pub fn is_safe_url(ctx: &Ctx, url: &str) -> bool {
    let mut hosts: Vec<String> = ctx.s.allowed_hosts.iter().filter(|h| h.as_str() != "*").cloned().collect();
    hosts.push(ctx.host());
    url_has_allowed_host_and_scheme(url, &hosts, false)
}

/// `django.utils.http.url_has_allowed_host_and_scheme(url, allowed_hosts, require_https)`.
pub fn url_has_allowed_host_and_scheme(url: &str, hosts: &[String], require_https: bool) -> bool {
    let url = url.trim();
    if url.is_empty() {
        return false;
    }
    let ok = |u: &str| -> bool {
        // Chrome treats \ completely as / in paths but it could be part of some basic auth credentials.
        if u.starts_with("///") {
            return false;
        }
        let Some(p) = pl_authz::csrf::urlsplit(u) else { return false };
        // Forbid URLs like http:///example.com - with a scheme, but without a hostname.
        if p.netloc.is_empty() && !p.scheme.is_empty() {
            return false;
        }
        // Forbid URLs that start with control characters.
        if u.chars().next().is_some_and(is_unicode_category_c) {
            return false;
        }
        let scheme_ok = p.scheme.is_empty() || p.scheme == "https" || (!require_https && p.scheme == "http");
        let host_ok = p.netloc.is_empty() || hosts.iter().any(|h| h == &p.netloc);
        scheme_ok && host_ok
    };
    ok(url) && ok(&url.replace('\\', "/"))
}

/// `get_next_redirect_url(request)`.
pub fn next_url(ctx: &Ctx) -> Option<String> {
    ctx.param("next").filter(|n| !n.is_empty() && is_safe_url(ctx, n)).map(str::to_string)
}

/// `passthrough_next_redirect_url(request, url, "next")`.
pub fn passthrough(ctx: &Ctx, url: &str) -> String {
    match next_url(ctx) {
        Some(n) => add_next(url, &n),
        None => url.to_string(),
    }
}

/// `FIRST_INSTALL`.
pub async fn first_install(ctx: &Ctx) -> R<bool> {
    let (users, docs): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM auth_user WHERE username NOT IN ('consumer', 'AnonymousUser')), \
                (SELECT count(*) FROM documents_document)",
    )
    .fetch_one(ctx.st.db())
    .await?;
    Ok(users == 0 && docs == 0)
}

/// `FIRST_INSTALL` inside `CustomAccountAdapter.save_user` (upstream fix 22746aa20 / #14688,
/// after v3.4.2): `@transaction.atomic` + `ContentType.objects.select_for_update().get(auth.user)`
/// serialise concurrent first signups, so only one of them becomes superuser. The caller inserts
/// the user (and its default groups) in the same transaction and commits afterwards.
pub async fn first_install_locked(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> R<bool> {
    sqlx::query("SELECT id FROM django_content_type WHERE app_label = 'auth' AND model = 'user' FOR UPDATE")
        .fetch_one(&mut **tx)
        .await?;
    let (users, docs): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM auth_user WHERE username NOT IN ('consumer', 'AnonymousUser')),                 (SELECT count(*) FROM documents_document)",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(users == 0 && docs == 0)
}

async fn app_branding(ctx: &Ctx) -> R<(Option<String>, Option<String>)> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT app_title, app_logo FROM paperless_applicationconfiguration ORDER BY id LIMIT 1")
            .fetch_optional(ctx.st.db())
            .await?;
    let (db_title, db_logo) = row.unwrap_or((None, None));
    let title = match db_title.filter(|t| !t.is_empty()) {
        Some(t) => Some(t),
        None => ctx.s.app_title.clone(),
    };
    let logo = match db_logo.filter(|l| !l.is_empty()) {
        Some(l) => Some(format!("{}{}", ctx.s.base_url, l.trim_start_matches('/'))),
        None => ctx.s.app_logo.clone(),
    };
    Ok((title, logo))
}

/// Base context for `base.html`. `form_errors`: error lists per field (order as in `form.errors`).
pub async fn base(ctx: &mut Ctx, form_errors: &[Vec<String>]) -> R<Map<String, Value>> {
    base_with(ctx, form_errors, true).await
}

/// Like [`base`]; `with_csrf = false` produces no CSRF field (and thus sets no
/// CSRF cookie either) – for pages without a form that upstream shows with the allauth layout.
pub async fn base_with(ctx: &mut Ctx, form_errors: &[Vec<String>], with_csrf: bool) -> R<Map<String, Value>> {
    let (title, logo) = app_branding(ctx).await?;
    let first = first_install(ctx).await?;
    let csrf = if with_csrf { Some(ctx.csrf_input()) } else { None };
    let msgs: Vec<Value> = ctx.messages.iterate().into_iter().map(|m| json!({"level_tag": m.level_tag(), "text": m.text})).collect();
    let mut get = Map::new();
    for (k, v) in &ctx.query {
        get.insert(k.clone(), Value::from(v.clone()));
    }
    let urls: Map<String, Value> = [
        "account_login",
        "account_logout",
        "account_signup",
        "account_reset_password",
        "account_inactive",
    ]
    .iter()
    .map(|n| (n.to_string(), Value::from(url(ctx, n))))
    .collect();
    let providers = social::template_providers(ctx).await?;
    let mut m = Map::new();
    m.insert("LANG".into(), Value::from(ctx.lang));
    m.insert("SITE_NAME".into(), Value::from(ctx.s.site_name.clone()));
    m.insert("static_url".into(), Value::from(ctx.s.static_url.clone()));
    if let Some(csrf) = csrf {
        m.insert("csrf_input".into(), Value::from(csrf));
    }
    m.insert("APP_TITLE".into(), title.map(Value::from).unwrap_or(Value::Null));
    // Built-in PaperRust logo (pl-server `branding::LOGO_PATH`): the light variant in dark mode.
    let logo_dark = logo.as_deref().and_then(|l| l.strip_suffix("/__rs/brand/logo.svg")).map(|p| format!("{p}/__rs/brand/logo-inverse.svg"));
    m.insert("APP_LOGO_DARK".into(), logo_dark.map(Value::from).unwrap_or(Value::Null));
    m.insert("APP_LOGO".into(), logo.map(Value::from).unwrap_or(Value::Null));
    m.insert("messages".into(), Value::Array(msgs));
    m.insert("form_errors".into(), json!(form_errors.iter().filter(|e| !e.is_empty()).collect::<Vec<_>>()));
    m.insert("GET".into(), Value::Object(get));
    m.insert("EMAIL_ENABLED".into(), Value::from(ctx.s.email_enabled));
    m.insert("DISABLE_REGULAR_LOGIN".into(), Value::from(ctx.s.disable_regular_login));
    m.insert("REDIRECT_LOGIN_TO_SSO".into(), Value::from(ctx.s.redirect_login_to_sso));
    m.insert("ACCOUNT_ALLOW_SIGNUPS".into(), Value::from(ctx.s.account_allow_signups));
    m.insert("FIRST_INSTALL".into(), Value::from(first));
    m.insert("signup_url".into(), Value::from(passthrough(ctx, &url(ctx, "account_signup"))));
    m.insert("login_url".into(), Value::from(passthrough(ctx, &url(ctx, "account_login"))));
    m.insert("urls".into(), Value::Object(urls));
    m.insert("socialaccount_providers".into(), Value::Array(providers));
    Ok(m)
}

/// Renders `name` with `m`; `csrf_input` is inserted as safe HTML text.
pub fn render(name: &str, m: Map<String, Value>, status: StatusCode) -> R<Response> {
    // csrf_input / redirect_field are HTML (format_html in Django)
    const SAFE: [&str; 2] = ["csrf_input", "redirect_field"];
    let mut v = J::from_serialize(Value::Object(m.clone()));
    if SAFE.iter().any(|k| m.get(*k).is_some_and(Value::is_string)) {
        let mut obj: std::collections::BTreeMap<String, J> = std::collections::BTreeMap::new();
        for (k, val) in &m {
            obj.insert(k.clone(), J::from_serialize(val));
        }
        for k in SAFE {
            if let Some(Value::String(c)) = m.get(k) {
                obj.insert(k.into(), J::from_safe_string(c.clone()));
            }
        }
        v = J::from_iter(obj);
    }
    let body = crate::tpl::render(name, v).map_err(|e| crate::web::Internal(format!("template {name}: {e:#}")))?;
    Ok(html(status, body))
}

/// `unicodedata.category(c)[0] == "C"` for the practically relevant characters (Cc, Cf, Cs, Co).
fn is_unicode_category_c(c: char) -> bool {
    let u = c as u32;
    u < 0x20
        || (0x7f..=0x9f).contains(&u)
        || u == 0xad
        || (0x600..=0x605).contains(&u)
        || u == 0x61c
        || u == 0x6dd
        || u == 0x70f
        || u == 0x180e
        || (0x200b..=0x200f).contains(&u)
        || (0x202a..=0x202e).contains(&u)
        || (0x2060..=0x2064).contains(&u)
        || (0x2066..=0x206f).contains(&u)
        || u == 0xfeff
        || (0xfff9..=0xfffb).contains(&u)
        || (0xe000..=0xf8ff).contains(&u)
        || (0xf0000..=0x10ffff).contains(&u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_urls_like_django() {
        let h: Vec<String> = vec!["127.0.0.1:8120".into(), "localhost".into()];
        let ok = |u: &str| url_has_allowed_host_and_scheme(u, &h, false);
        assert!(ok("/documents/"));
        assert!(ok("/accounts/login/?next=/"));
        assert!(ok("http://127.0.0.1:8120/x"));
        assert!(ok("https://localhost/x"));
        assert!(!ok("//evil.example/"));
        assert!(!ok("///evil.example/"));
        assert!(!ok("https://evil.example/"));
        assert!(!ok("http:///evil.example"));
        assert!(!ok(r"/\evil.example"));
        assert!(!ok(r"\\evil.example"));
        assert!(!ok("javascript:alert(1)"));
        assert!(!ok("\u{200b}//evil.example"));
        assert!(!ok("/\t/evil.example"));
        assert!(!ok(" "));
        assert!(!ok("ftp://127.0.0.1:8120/"));
    }

    #[test]
    fn next_param_encoding() {
        assert_eq!(add_next("/accounts/signup/", "/documents/?a=1"), "/accounts/signup/?next=%2Fdocuments%2F%3Fa%3D1");
    }
}
