//! Social apps and providers: provider list of the login/signup page (`{% get_providers %}`,
//! `{% provider_login_url %}`) and provider lookup of the OIDC pages.
//!
//! upstream: allauth 65.20.1 socialaccount/adapter.py (`list_apps`: DB apps first, then
//! `SOCIALACCOUNT_PROVIDERS[*]["APPS"|"APP"]` via `_build_apps_from_settings`; `get_app` with
//! `hidden` selection; `get_provider`; `list_providers` per installed provider app),
//! socialaccount/app_settings.py (`PROVIDERS` incl. `_migrate_oidc` for `SERVERS`),
//! socialaccount/templatetags/socialaccount.py (`provider_login_url`: `process=""`,
//! `scope`/`auth_params` removed when empty, `next` from the request),
//! socialaccount/providers/openid_connect/{provider,urls}.py (`/accounts/oidc/<id>/login/`).
//!
//! The OIDC flow itself lives in `oidc.rs` (protocol) and `socialaccount.rs` (pages, accounts),
//! the other OAuth2 providers (GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea) in `oauth2.rs`.
//! Provider apps from `PAPERLESS_APPS` that PaperRust does not know are skipped (one
//! warning in the log); password login and the known providers stay usable (gap S2).

use serde_json::{Map, Value, json};

use pl_core::AppState;

use crate::settings::Settings;
use crate::web::{Ctx, Internal, R};

pub const OIDC_APP: &str = "allauth.socialaccount.providers.openid_connect";
pub const OIDC: &str = "openid_connect";
/// Prefix of the provider apps in `INSTALLED_APPS`.
pub const PROVIDER_APP_PREFIX: &str = "allauth.socialaccount.providers.";

/// A supported provider class (`Provider.id`, app module, `Provider.name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderClass {
    pub id: &'static str,
    pub app: &'static str,
    pub name: &'static str,
}

/// Provider classes that PaperRust reimplements (allauth 65.20.1).
pub const PROVIDER_CLASSES: &[ProviderClass] = &[
    ProviderClass { id: OIDC, app: OIDC_APP, name: "OpenID Connect" },
    ProviderClass { id: "github", app: "allauth.socialaccount.providers.github", name: "GitHub" },
    ProviderClass { id: "gitlab", app: "allauth.socialaccount.providers.gitlab", name: "GitLab" },
    ProviderClass { id: "google", app: "allauth.socialaccount.providers.google", name: "Google" },
    ProviderClass { id: "microsoft", app: "allauth.socialaccount.providers.microsoft", name: "Microsoft" },
    ProviderClass { id: "nextcloud", app: "allauth.socialaccount.providers.nextcloud", name: "NextCloud" },
    ProviderClass { id: "discord", app: "allauth.socialaccount.providers.discord", name: "Discord" },
    ProviderClass { id: "gitea", app: "allauth.socialaccount.providers.gitea", name: "Gitea" },
];

/// Installed, supported provider classes in the order of `PAPERLESS_APPS`
/// (`registry.get_class_list()` follows `INSTALLED_APPS`).
pub fn installed_classes(s: &Settings) -> Vec<&'static ProviderClass> {
    let mut out: Vec<&'static ProviderClass> = Vec::new();
    for a in &s.installed_provider_apps {
        if let Some(c) = PROVIDER_CLASSES.iter().find(|c| c.app == a)
            && !out.iter().any(|o| o.id == c.id)
        {
            out.push(c);
        }
    }
    out
}

/// Provider class `id` installed (`registry.get_class(id)`)?
pub fn class_installed(s: &Settings, id: &str) -> Option<&'static ProviderClass> {
    installed_classes(s).into_iter().find(|c| c.id == id)
}

/// Provider apps from `PAPERLESS_APPS` that PaperRust does not reimplement.
pub fn unsupported_provider_apps(s: &Settings) -> Vec<String> {
    s.installed_provider_apps
        .iter()
        .filter(|a| a.starts_with(PROVIDER_APP_PREFIX) && !PROVIDER_CLASSES.iter().any(|c| c.app == a.as_str()))
        .cloned()
        .collect()
}

/// Once per process: note in the log about skipped provider apps (instead of disabling the
/// login pages). Django would start with the provider; PaperRust does not show it.
pub fn warn_unsupported_providers(s: &Settings) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    let unknown = unsupported_provider_apps(s);
    if unknown.is_empty() {
        return;
    }
    WARNED.call_once(|| {
        tracing::warn!(
            target: "paperless.auth",
            "Social login providers from PAPERLESS_APPS are not supported by PaperRust and are ignored: {}. Password login and the supported providers (OpenID Connect, GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea) keep working; Keycloak/Authentik etc. work via openid_connect.",
            unknown.join(", ")
        );
    });
}

/// `str(provider)` / `provider.name`: OIDC apps are named like the app, otherwise like the class.
pub fn display_name(app: &App) -> String {
    if app.provider == OIDC {
        return app.name.clone();
    }
    PROVIDER_CLASSES.iter().find(|c| c.id == app.provider).map(|c| c.name.to_string()).unwrap_or_else(|| app.provider.clone())
}

/// `provider.get_login_url(request)` without query: OIDC `/accounts/oidc/<provider_id>/login/`,
/// otherwise `reverse("<id>_login")` = `/accounts/<id>/login/`.
pub fn login_path(base_url: &str, app: &App) -> R<String> {
    if app.provider == OIDC {
        return oidc_login_path(base_url, &app.provider_id);
    }
    Ok(format!("{base_url}accounts/{}/login/", app.provider))
}

/// A social app (`SocialApp`, from DB or settings).
#[derive(Debug, Clone, Default)]
pub struct App {
    /// DB primary key (`None` = from the settings).
    pub pk: Option<i64>,
    pub provider: String,
    pub provider_id: String,
    pub name: String,
    pub client_id: String,
    pub secret: String,
    pub key: String,
    pub settings: Map<String, Value>,
}

impl App {
    pub fn hidden(&self) -> bool {
        self.settings.get("hidden").is_some_and(py_truthy)
    }

    /// `Provider.sub_id` (`provider_id or provider`).
    pub fn sub_id(&self) -> &str {
        if self.provider_id.is_empty() { &self.provider } else { &self.provider_id }
    }
}

/// Python truthiness of a JSON value.
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `app_settings.PROVIDERS` (with `_migrate_oidc`).
pub fn providers_setting(ctx: &Ctx) -> Map<String, Value> {
    providers_from(&ctx.s)
}

/// [`providers_setting`] directly from the settings.
pub fn providers_from(s: &Settings) -> Map<String, Value> {
    let mut ret = match &s.socialaccount_providers {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    if let Some(Value::Object(oidc)) = ret.get(OIDC).cloned()
        && let Some(Value::Array(servers)) = oidc.get("SERVERS")
    {
        let mut apps = Vec::new();
        for server in servers {
            let mut app = server.get("APP").and_then(Value::as_object).cloned().unwrap_or_default();
            let mut st = Map::new();
            if let Some(t) = server.get("token_auth_method") {
                st.insert("token_auth_method".into(), t.clone());
            }
            st.insert("server_url".into(), server.get("server_url").cloned().unwrap_or(Value::Null));
            app.insert("name".into(), server.get("name").cloned().unwrap_or(Value::from("")));
            app.insert("provider_id".into(), server.get("id").cloned().unwrap_or(Value::Null));
            app.insert("settings".into(), Value::Object(st));
            apps.push(Value::Object(app));
        }
        ret.insert(OIDC.into(), json!({ "APPS": apps }));
    }
    ret
}

/// Settings of a provider type (`Provider.get_settings()`).
pub fn provider_settings(ctx: &Ctx, provider: &str) -> Map<String, Value> {
    providers_setting(ctx).get(provider).and_then(Value::as_object).cloned().unwrap_or_default()
}

fn text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// `adapter.list_apps(request, provider=None, client_id=None)`.
pub async fn list_apps(ctx: &Ctx, provider: Option<&str>, client_id: Option<&str>) -> R<Vec<App>> {
    list_apps_in(&ctx.st, &ctx.s, provider, client_id).await
}

/// [`list_apps`] without page context (also for the DRF route `social_account_providers`).
pub async fn list_apps_in(st: &AppState, s: &Settings, provider: Option<&str>, client_id: Option<&str>) -> R<Vec<App>> {
    let mut by_provider: Vec<(String, Vec<App>)> = Vec::new();
    let mut push = |a: App| {
        if let Some((_, v)) = by_provider.iter_mut().find(|(p, _)| *p == a.provider) {
            v.push(a);
        } else {
            by_provider.push((a.provider.clone(), vec![a]));
        }
    };
    let client_id = client_id.filter(|c| !c.is_empty());
    type Row = (i64, String, String, String, String, String, String, Value);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id::int8, provider, provider_id, name, client_id, secret, key, settings FROM socialaccount_socialapp \
         WHERE ($1::text IS NULL OR provider = $1 OR provider_id = $1) AND ($2::text IS NULL OR client_id = $2) ORDER BY id",
    )
    .bind(provider)
    .bind(client_id)
    .fetch_all(st.db())
    .await?;
    for (pk, provider, provider_id, name, client_id, secret, key, settings) in rows {
        let settings = match settings {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        push(App { pk: Some(pk), provider, provider_id, name, client_id, secret, key, settings });
    }
    for (p, conf) in providers_from(s) {
        let configs: Vec<Value> = match conf.get("APPS") {
            Some(Value::Array(a)) => a.clone(),
            Some(Value::Null) | None => match conf.get("APP") {
                Some(Value::Null) | None => continue,
                Some(a) => vec![a.clone()],
            },
            Some(other) => vec![other.clone()],
        };
        for c in configs {
            let mut settings = c.get("settings").and_then(Value::as_object).cloned().unwrap_or_default();
            if let Some(k) = c.get("certificate_key") {
                settings.insert("certificate_key".into(), k.clone());
            }
            let app = App {
                pk: None,
                provider: p.clone(),
                provider_id: text(c.get("provider_id")),
                name: text(c.get("name")),
                client_id: text(c.get("client_id")),
                secret: text(c.get("secret")),
                key: text(c.get("key")),
                settings,
            };
            if let Some(cid) = client_id
                && app.client_id != cid
            {
                continue;
            }
            if let Some(pr) = provider
                && app.provider_id != pr
                && app.provider != pr
            {
                continue;
            }
            push(app);
        }
    }
    Ok(by_provider.into_iter().flat_map(|(_, v)| v).collect())
}

/// `adapter.get_app(request, provider, client_id)`: `None` = `SocialApp.DoesNotExist`
/// (→ 404 in the OIDC views), `MultipleObjectsReturned` → 500.
pub async fn get_app(ctx: &Ctx, provider: &str, client_id: Option<&str>) -> R<Option<App>> {
    let mut apps = list_apps(ctx, Some(provider), client_id).await?;
    if apps.len() > 1 {
        let visible: Vec<App> = apps.into_iter().filter(|a| !a.hidden()).collect();
        if visible.len() != 1 {
            return Err(Internal("MultipleObjectsReturned".into()));
        }
        apps = visible;
    }
    Ok(apps.into_iter().next())
}

/// `adapter.get_provider(request, provider)` for `openid_connect` apps. `Ok(None)` = no app,
/// `Err` = other provider class or not installed (ImproperlyConfigured → 500).
pub async fn get_oidc_provider(ctx: &Ctx, provider: &str, client_id: Option<&str>) -> R<Option<App>> {
    let Some(app) = get_app(ctx, provider, client_id).await? else { return Ok(None) };
    if app.provider != OIDC || !oidc_installed(ctx) {
        return Err(Internal(format!("unknown provider: {}", app.provider)));
    }
    Ok(Some(app))
}

/// `adapter.get_provider(request, provider, client_id)` for all supported classes.
/// `Ok(None)` = no app (`SocialApp.DoesNotExist`), `Err` = class not installed/unknown.
pub async fn get_provider(ctx: &Ctx, provider: &str, client_id: Option<&str>) -> R<Option<App>> {
    let Some(app) = get_app(ctx, provider, client_id).await? else { return Ok(None) };
    if class_installed(&ctx.s, &app.provider).is_none() {
        return Err(Internal(format!("unknown provider: {}", app.provider)));
    }
    Ok(Some(app))
}

pub fn oidc_installed(ctx: &Ctx) -> bool {
    oidc_installed_in(&ctx.s)
}

pub fn oidc_installed_in(s: &Settings) -> bool {
    s.installed_provider_apps.iter().any(|a| a == OIDC_APP)
}

/// `[{"name", "login_url"}]` of `SocialAccountProvidersView.get` (src/paperless/views.py@v3.4.2):
/// `adapter.list_providers(request)` (only the supported classes; there is no `openid`),
/// `name = provider.name` (OIDC: app name), `get_login_url(request, process="connect")`, sorted
/// by `name`. Skipped provider apps are missing (note in the log). `Option` stays for the
/// callers; `None` no longer occurs.
pub async fn api_providers(st: &AppState, s: &Settings) -> R<Option<Vec<Value>>> {
    warn_unsupported_providers(s);
    let mut out: Vec<(String, String)> = Vec::new();
    for app in list_providers(st, s).await? {
        let url = format!("{}?process=connect", login_path(&s.base_url, &app)?);
        out.push((display_name(&app), url));
    }
    // sorted(resp, key=lambda p: p["name"]) – stable, code point order
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Some(out.into_iter().map(|(n, u)| json!({"name": n, "login_url": u})).collect()))
}

/// `adapter.list_providers(request)`: for each installed class (registry order) its apps.
pub async fn list_providers(st: &AppState, s: &Settings) -> R<Vec<App>> {
    let classes = installed_classes(s);
    if classes.is_empty() {
        return Ok(Vec::new());
    }
    let apps = list_apps_in(st, s, None, None).await?;
    let mut out = Vec::new();
    for c in classes {
        out.extend(apps.iter().filter(|a| a.provider == c.id).cloned());
    }
    Ok(out)
}

/// `reverse("openid_connect_login", kwargs={"provider_id": id})` (allauth
/// `OpenIDConnectProvider.get_login_url`; URL pattern `accounts/` + `SOCIALACCOUNT_OPENID_CONNECT_URL_PREFIX`
/// (default `oidc`, paperless does not set it) + `^(?P<provider_id>[^/]+)/login/`), with script prefix
/// (`FORCE_SCRIPT_NAME` = `base_url`). A `provider_id` that does not match `[^/]+` yields, like
/// Django's `NoReverseMatch`, an internal error (security review final B-N2: parity test).
pub fn oidc_login_path(base_url: &str, provider_id: &str) -> R<String> {
    if provider_id.is_empty() || provider_id.contains('/') {
        return Err(Internal(format!("NoReverseMatch: openid_connect_login provider_id={provider_id:?}")));
    }
    Ok(format!("{base_url}accounts/oidc/{}/login/", url_path_escape(provider_id)))
}

/// Django `reverse()` quotes path arguments with a `quote(safe="/~")`-like `iri_to_uri` set
/// (RFC 3986 unreserved + sub-delims + `:@/`).
fn url_path_escape(s: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-').remove(b'.').remove(b'_').remove(b'~')
        .remove(b'!').remove(b'$').remove(b'&').remove(0x27).remove(b'(').remove(b')')
        .remove(b'*').remove(b'+').remove(b',').remove(b';').remove(b'=').remove(b':').remove(b'@').remove(b'/');
    percent_encoding::utf8_percent_encode(s, SET).to_string()
}

/// Providers for the templates (`{% get_providers %}`: without `hidden`, sorted by name,
/// `{% provider_login_url provider process=process %}`): `{id, name, href}`.
pub async fn template_providers(ctx: &Ctx) -> R<Vec<Value>> {
    let apps = list_providers(&ctx.st, &ctx.s).await?;
    let mut out: Vec<(String, Value)> = Vec::new();
    for app in apps.into_iter().filter(|a| !a.hidden()) {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        q.append_pair("process", "");
        if let Some(n) = ctx.param("next").filter(|n| !n.is_empty()) {
            q.append_pair("next", n);
        }
        let href = format!("{}?{}", login_path(&ctx.s.base_url, &app)?, q.finish());
        let name = display_name(&app);
        out.push((name.clone(), json!({"id": app.provider, "name": name, "href": href, "hidden": false})));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truthiness_like_python() {
        assert!(!py_truthy(&json!(null)));
        assert!(!py_truthy(&json!(0)));
        assert!(!py_truthy(&json!("")));
        assert!(!py_truthy(&json!([])));
        assert!(py_truthy(&json!(true)));
        assert!(py_truthy(&json!("x")));
        assert!(py_truthy(&json!({"a": 1})));
    }

    /// B-N2: login URL like allauth `reverse("openid_connect_login")` with Django quoting
    /// (`quote(safe=RFC3986_SUBDELIMS + "/~:@")`) and script prefix.
    #[test]
    fn oidc_login_path_like_django_reverse() {
        assert_eq!(oidc_login_path("/", "keycloak").ok().as_deref(), Some("/accounts/oidc/keycloak/login/"));
        assert_eq!(oidc_login_path("/paperless/", "kc").ok().as_deref(), Some("/paperless/accounts/oidc/kc/login/"));
        // Django: quote("a b?#ä", safe="!$&'()*+,;=/~:@") -> a%20b%3F%23%C3%A4
        assert_eq!(oidc_login_path("/", "a b?#ä").ok().as_deref(), Some("/accounts/oidc/a%20b%3F%23%C3%A4/login/"));
        assert_eq!(oidc_login_path("/", "x:y@z!$&'()*+,;=~").ok().as_deref(), Some("/accounts/oidc/x:y@z!$&'()*+,;=~/login/"));
        // `[^/]+` does not match -> NoReverseMatch (Django 500)
        assert!(oidc_login_path("/", "").is_err());
        assert!(oidc_login_path("/", "a/b").is_err());
    }

    #[test]
    fn provider_classes_from_apps() {
        let mut s = crate::settings::Settings::from_core(&pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([
            ("PAPERLESS_SECRET_KEY", "unit-test-only-secret-key-0123456789abcdef"),
            ("PAPERLESS_APPS", "allauth.socialaccount.providers.github,allauth.socialaccount.providers.facebook,allauth.socialaccount.providers.openid_connect"),
        ])).expect("config"));
        let ids: Vec<&str> = installed_classes(&s).iter().map(|c| c.id).collect();
        assert_eq!(ids, vec!["github", OIDC]);
        assert_eq!(unsupported_provider_apps(&s), vec!["allauth.socialaccount.providers.facebook".to_string()]);
        assert!(class_installed(&s, "google").is_none());
        s.installed_provider_apps.clear();
        assert!(installed_classes(&s).is_empty());
        let gh = App { provider: "github".into(), name: "ignored".into(), ..Default::default() };
        assert_eq!(display_name(&gh), "GitHub");
        assert_eq!(login_path("/p/", &gh).ok().as_deref(), Some("/p/accounts/github/login/"));
        let oidc = App { provider: OIDC.into(), provider_id: "kc".into(), name: "Keycloak".into(), ..Default::default() };
        assert_eq!(display_name(&oidc), "Keycloak");
        assert_eq!(login_path("/", &oidc).ok().as_deref(), Some("/accounts/oidc/kc/login/"));
    }

    #[test]
    fn sub_id_prefers_provider_id() {
        let a = App { provider: OIDC.into(), provider_id: "kc".into(), ..Default::default() };
        assert_eq!(a.sub_id(), "kc");
        let b = App { provider: OIDC.into(), ..Default::default() };
        assert_eq!(b.sub_id(), OIDC);
    }
}
