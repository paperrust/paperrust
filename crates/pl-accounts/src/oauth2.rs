//! OAuth2 social login providers other than OpenID Connect (gap S2): GitHub, GitLab, Google,
//! Microsoft (Graph), Nextcloud, Discord, Gitea – endpoints, default scopes, profile retrieval and field mapping as in
//! the allauth 65.20.1 providers. The flow (start, callback, account, signup) is the same as
//! for OIDC and lives in `socialaccount.rs`; Keycloak/Authentik/Authelia etc. run via
//! `openid_connect` (allauth 65 no longer has its own Keycloak provider).
//!
//! upstream: allauth socialaccount/providers/{github,gitlab,google,microsoft,nextcloud,discord,gitea}/
//! {provider,views}.py (`get_default_scope`, `extract_uid`, `extract_common_fields`,
//! `extract_email_addresses`, `extract_extra_data`, `*OAuth2Adapter`: `authorize_url`,
//! `access_token_url`, `profile_url`, `complete_login`, `_check_errors`),
//! socialaccount/providers/oauth2/{provider,views,urls}.py (`get_scope`, `get_auth_params`,
//! `get_pkce_params` with `pkce_enabled_default = False`, `default_urlpatterns` →
//! `/accounts/<id>/login/` and `/accounts/<id>/login/callback/`), socialaccount/app_settings.py
//! (`QUERY_EMAIL` = `EMAIL_REQUIRED` = False in paperless: no `user:email`/`email` scope, no
//! GitHub address list), socialaccount/internal/jwtkit.py (Google ID token without signature check,
//! because fetched from the token endpoint via TLS).

use serde_json::{Map, Value};

use crate::emailaddr::EmailAddr;
use crate::oidc::{self, OidcError};
use crate::social::{self, App};
use crate::web::Ctx;

pub const GITHUB: &str = "github";
pub const GITLAB: &str = "gitlab";
pub const GOOGLE: &str = "google";
pub const MICROSOFT: &str = "microsoft";
pub const NEXTCLOUD: &str = "nextcloud";
pub const DISCORD: &str = "discord";
pub const GITEA: &str = "gitea";

/// DiscordOAuth2Adapter: fixed endpoints.
const DISCORD_API: &str = "https://discord.com/api";

/// `user_properties` of the Microsoft Graph adapter (`$select`).
const MS_USER_PROPERTIES: &str =
    "businessPhones,displayName,givenName,id,jobTitle,mail,mobilePhone,officeLocation,preferredLanguage,surname,userPrincipalName,mailNickname,companyName";

/// Setting `PROVIDERS[<provider>][key]` as text.
fn provider_str(ctx: &Ctx, provider: &str, key: &str) -> Option<String> {
    social::provider_settings(ctx, provider).get(key).and_then(Value::as_str).map(str::to_string)
}

/// App setting (`app.settings[key]`) as text.
fn app_str(app: &App, key: &str) -> Option<String> {
    app.settings.get(key).and_then(Value::as_str).map(str::to_string)
}

/// `get_default_scope()` per provider (`QUERY_EMAIL` is off in paperless).
pub fn default_scope(provider: &str) -> Vec<String> {
    let s: &[&str] = match provider {
        social::OIDC => &["openid", "profile", "email"],
        GITLAB => &["read_user"],
        GOOGLE => &["profile"],
        MICROSOFT => &["User.Read"],
        NEXTCLOUD => &["read"],
        DISCORD => &["email", "identify"],
        _ => &[],
    };
    s.iter().map(|x| x.to_string()).collect()
}

fn github_urls(ctx: &Ctx) -> (String, String) {
    // GitHubOAuth2Adapter: GITHUB_URL (Enterprise) → web_url, api_url = web_url + "/api/v3"
    match provider_str(ctx, GITHUB, "GITHUB_URL") {
        Some(u) => {
            let web = u.trim_end_matches('/').to_string();
            let api = format!("{web}/api/v3");
            (web, api)
        }
        None => ("https://github.com".into(), "https://api.github.com".into()),
    }
}

/// GiteaOAuth2Adapter: `GITEA_URL` (without trailing `/`), otherwise `https://gitea.com`.
fn gitea_web_url(setting: Option<String>) -> String {
    match setting {
        Some(u) => u.trim_end_matches('/').to_string(),
        None => "https://gitea.com".into(),
    }
}

fn gitlab_url(ctx: &Ctx, app: &App, path: &str) -> String {
    let base = app_str(app, "gitlab_url").or_else(|| provider_str(ctx, GITLAB, "GITLAB_URL")).unwrap_or_else(|| "https://gitlab.com".into());
    format!("{base}{path}")
}

fn microsoft_tenant_url(ctx: &Ctx, app: &App, path: &str) -> String {
    let st = social::provider_settings(ctx, MICROSOFT);
    let tenant = st.get("TENANT").or_else(|| st.get("tenant")).and_then(Value::as_str).unwrap_or("common").to_string();
    let tenant = app_str(app, "tenant").unwrap_or(tenant);
    let login = app_str(app, "login_url").unwrap_or_else(|| "https://login.microsoftonline.com".into());
    format!("{login}/{tenant}{path}")
}

fn nextcloud_url(ctx: &Ctx, app: &App, path: &str) -> String {
    let server = app_str(app, "server").or_else(|| provider_str(ctx, NEXTCLOUD, "SERVER")).unwrap_or_else(|| "https://nextcloud.example.org".into());
    format!("{server}{path}")
}

fn google_setting(ctx: &Ctx, key: &str, default: &str) -> String {
    provider_str(ctx, GOOGLE, key).unwrap_or_else(|| default.to_string())
}

/// (`authorize_url`, `access_token_url`) of the adapter.
pub fn endpoints(ctx: &Ctx, app: &App) -> Result<(String, String), OidcError> {
    Ok(match app.provider.as_str() {
        GITHUB => {
            let (web, _) = github_urls(ctx);
            (format!("{web}/login/oauth/authorize"), format!("{web}/login/oauth/access_token"))
        }
        GITLAB => (gitlab_url(ctx, app, "/oauth/authorize"), gitlab_url(ctx, app, "/oauth/token")),
        GOOGLE => (
            google_setting(ctx, "AUTHORIZE_URL", "https://accounts.google.com/o/oauth2/v2/auth"),
            google_setting(ctx, "ACCESS_TOKEN_URL", "https://oauth2.googleapis.com/token"),
        ),
        MICROSOFT => (microsoft_tenant_url(ctx, app, "/oauth2/v2.0/authorize"), microsoft_tenant_url(ctx, app, "/oauth2/v2.0/token")),
        NEXTCLOUD => (nextcloud_url(ctx, app, "/apps/oauth2/authorize"), nextcloud_url(ctx, app, "/apps/oauth2/api/v1/token")),
        DISCORD => (format!("{DISCORD_API}/oauth2/authorize"), format!("{DISCORD_API}/oauth2/token")),
        GITEA => {
            let web = gitea_web_url(provider_str(ctx, GITEA, "GITEA_URL"));
            (format!("{web}/login/oauth/authorize"), format!("{web}/login/oauth/access_token"))
        }
        other => return Err(OidcError::Internal(format!("no oauth2 adapter for provider {other:?}"))),
    })
}

/// GET with timeout (`get_requests_session`); returns status and body.
async fn get(url: &str, query: &[(&str, &str)], auth: Option<String>) -> Result<(u16, Vec<u8>), OidcError> {
    let c = reqwest::Client::builder()
        .user_agent(pl_core::brand::USER_AGENT)
        .timeout(oidc::REQUESTS_TIMEOUT)
        .build()
        .map_err(|e| OidcError::Internal(e.to_string()))?;
    let mut rb = c.get(url);
    if !query.is_empty() {
        rb = rb.query(query);
    }
    if let Some(a) = auth {
        rb = rb.header("Authorization", a);
    }
    let r = rb.send().await.map_err(request_error)?;
    let status = r.status().as_u16();
    let body = r.bytes().await.map_err(request_error)?;
    Ok((status, body.to_vec()))
}

/// reqwest error without URL: the GitLab profile request carries the access token like allauth as
/// `?access_token=` in the URL, and reqwest appends the URL including query to its error text. The
/// text ends up in the log via `auth_error` (security review 2026-11-19 F14).
fn request_error(e: reqwest::Error) -> OidcError {
    OidcError::Request(e.without_url().to_string())
}

/// `resp.raise_for_status(); resp.json()`.
fn ok_json(status: u16, body: &[u8]) -> Result<Value, OidcError> {
    if status >= 400 {
        return Err(OidcError::Request(format!("HTTP {status}")));
    }
    serde_json::from_slice(body).map_err(|e| OidcError::Request(format!("JSONDecodeError: {e}")))
}

/// `complete_login(request, app, token, response=...)` → raw data for
/// `sociallogin_from_response` (`token` = access token, `response` = response of the token endpoint).
pub async fn fetch_profile(ctx: &mut Ctx, app: &App, token: &str, response: &Map<String, Value>) -> Result<Map<String, Value>, OidcError> {
    let data: Value = match app.provider.as_str() {
        GITHUB => {
            let (_, api) = github_urls(ctx);
            let (st, body) = get(&format!("{api}/user"), &[], Some(format!("token {token}"))).await?;
            ok_json(st, &body)?
        }
        GITLAB => {
            let (st, body) = get(&gitlab_url(ctx, app, "/api/v4/user"), &[("access_token", token)], None).await?;
            gitlab_check_errors(st, &body)?
        }
        GOOGLE => match response.get("id_token").filter(|v| social::py_truthy(v)) {
            Some(id_token) => {
                let issuer = google_setting(ctx, "ID_TOKEN_ISSUER", "https://accounts.google.com");
                let now = crate::flow::now_f64();
                let raw = id_token.as_str().map(str::to_string).unwrap_or_else(|| id_token.to_string());
                let claims = oidc::decode_id_token(&raw, &issuer, &app.client_id, now)?;
                if let Some((key, timeout)) = oidc::jti_key(&claims, now) {
                    let t = if timeout > 0.0 { timeout as u64 } else { 0 };
                    let added = if t == 0 { false } else { ctx.cache.add(&key, crate::cache::PICKLED_TRUE, t).await.map_err(|e| OidcError::Internal(e.0))? };
                    if !added {
                        return Err(OidcError::OAuth2("Invalid id_token".into()));
                    }
                }
                let mut data = claims;
                let fetch = social::provider_settings(ctx, GOOGLE).get("FETCH_USERINFO").is_some_and(social::py_truthy);
                if fetch && !data.contains_key("picture") {
                    let info = google_userinfo(ctx, token).await?;
                    if let Some(p) = info.get("picture").filter(|v| social::py_truthy(v)) {
                        data.insert("picture".into(), p.clone());
                    }
                }
                Value::Object(data)
            }
            None => google_userinfo(ctx, token).await?,
        },
        MICROSOFT => {
            let graph = app_str(app, "graph_url").unwrap_or_else(|| "https://graph.microsoft.com".into());
            let (_, body) = get(&format!("{graph}/v1.0/me"), &[("$select", MS_USER_PROPERTIES)], Some(format!("Bearer {token}"))).await?;
            microsoft_check_errors(&body)?
        }
        NEXTCLOUD => {
            // kwargs["response"]["user_id"]: if missing, KeyError (500)
            let Some(uid) = response.get("user_id") else {
                return Err(OidcError::Internal("KeyError: 'user_id'".into()));
            };
            let uid = uid.as_str().map(str::to_string).unwrap_or_else(|| uid.to_string());
            let url = format!("{}{uid}", nextcloud_url(ctx, app, "/ocs/v1.php/cloud/users/"));
            let (st, body) = get(&url, &[("format", "json")], Some(format!("Bearer {token}"))).await?;
            let v = ok_json(st, &body)?;
            v.get("ocs").and_then(|o| o.get("data")).cloned().ok_or_else(|| OidcError::Internal("KeyError: 'ocs'/'data'".into()))?
        }
        DISCORD => {
            // no raise_for_status, only `.json()`
            let (_, body) = get(&format!("{DISCORD_API}/users/@me"), &[], Some(format!("Bearer {token}"))).await?;
            serde_json::from_slice(&body).map_err(|e| OidcError::Request(format!("JSONDecodeError: {e}")))?
        }
        GITEA => {
            let web = gitea_web_url(provider_str(ctx, GITEA, "GITEA_URL"));
            let (st, body) = get(&format!("{web}/api/v1/user"), &[], Some(format!("token {token}"))).await?;
            ok_json(st, &body)?
        }
        other => return Err(OidcError::Internal(format!("no oauth2 adapter for provider {other:?}"))),
    };
    match data {
        Value::Object(m) => Ok(m),
        _ => Err(OidcError::Internal("profile response is not an object".into())),
    }
}

async fn google_userinfo(ctx: &Ctx, token: &str) -> Result<Value, OidcError> {
    let url = google_setting(ctx, "IDENTITY_URL", "https://www.googleapis.com/oauth2/v2/userinfo");
    let (st, body) = get(&url, &[], Some(format!("Bearer {token}"))).await?;
    if !(200..400).contains(&st) {
        return Err(OidcError::OAuth2("Request to user info failed".into()));
    }
    serde_json::from_slice(&body).map_err(|e| OidcError::Request(format!("JSONDecodeError: {e}")))
}

/// gitlab `_check_errors(response)`.
fn gitlab_check_errors(status: u16, body: &[u8]) -> Result<Value, OidcError> {
    if status == 403 {
        return Err(OidcError::OAuth2("Invalid data from GitLab API".into()));
    }
    let data: Value = serde_json::from_slice(body).map_err(|_| OidcError::OAuth2("Invalid JSON from GitLab API".into()))?;
    let has_error = data.as_object().is_some_and(|o| o.contains_key("error"));
    if status >= 400 || has_error {
        let err = data.get("error").filter(|v| social::py_truthy(v)).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_else(|| status.to_string());
        return Err(OidcError::OAuth2(format!("GitLab error: {err}")));
    }
    if !data.as_object().is_some_and(|o| o.contains_key("id")) {
        return Err(OidcError::OAuth2("Invalid data from GitLab API".into()));
    }
    Ok(data)
}

/// microsoft `_check_errors(response)`.
fn microsoft_check_errors(body: &[u8]) -> Result<Value, OidcError> {
    let data: Value = serde_json::from_slice(body).map_err(|_| OidcError::OAuth2("Invalid JSON from Microsoft Graph API".into()))?;
    if !data.as_object().is_some_and(|o| o.contains_key("id")) {
        let mut msg = "Error retrieving Microsoft profile".to_string();
        if let Some(m) = data.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).filter(|m| !m.is_empty()) {
            msg.push_str(": ");
            msg.push_str(m);
        }
        return Err(OidcError::OAuth2(msg));
    }
    Ok(data)
}

/// Python `str(value)` (numbers without quotes).
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

fn opt(data: &Map<String, Value>, k: &str) -> Option<String> {
    match data.get(k) {
        None | Some(Value::Null) => None,
        Some(v) => Some(py_str(v)),
    }
}

/// Result of the provider-specific evaluation (`extract_*`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extracted {
    pub uid: String,
    pub extra_data: Map<String, Value>,
    pub email: Option<String>,
    pub email_verified: bool,
    pub username: Option<String>,
    pub name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub addresses: Vec<EmailAddr>,
}

/// `extract_uid`/`extract_extra_data`/`extract_common_fields`/`extract_email_addresses`.
/// `Err` = KeyError/ValueError (500 as allauth).
pub fn extract(provider: &str, data: &Map<String, Value>) -> Result<Extracted, String> {
    let key = |k: &str| data.get(k).ok_or_else(|| format!("KeyError: '{k}'"));
    let mut e = Extracted { extra_data: data.clone(), ..Default::default() };
    match provider {
        GITHUB => {
            e.uid = py_str(key("id")?);
            e.email = opt(data, "email");
            e.username = opt(data, "login");
            e.name = opt(data, "name");
            e.extra_data.remove("emails");
            if let Some(Value::Array(list)) = data.get("emails") {
                for m in list {
                    let email = m.get("email").ok_or("KeyError: 'email'")?;
                    let primary = m.get("primary").ok_or("KeyError: 'primary'")?;
                    let verified = m.get("verified").ok_or("KeyError: 'verified'")?;
                    e.addresses.push(EmailAddr { email: py_str(email), verified: verified.clone(), primary: social::py_truthy(primary) });
                }
            }
        }
        GITLAB => {
            e.uid = py_str(key("id")?);
            e.email = opt(data, "email");
            e.username = opt(data, "username");
            e.name = opt(data, "name");
        }
        GOOGLE => {
            e.uid = match data.get("sub") {
                Some(v) => py_str(v),
                None => py_str(key("id")?),
            };
            e.email = opt(data, "email");
            e.last_name = opt(data, "family_name");
            e.first_name = opt(data, "given_name");
            if let Some(m) = data.get("email").filter(|v| social::py_truthy(v)) {
                // only a real `true`/"true" (security review 2026-11-19, as for OIDC)
                let verified = data.get("email_verified").is_some_and(crate::emailaddr::is_true_claim)
                    || data.get("verified_email").is_some_and(crate::emailaddr::is_true_claim);
                e.addresses.push(EmailAddr { email: py_str(m), verified: Value::Bool(verified), primary: true });
            }
        }
        MICROSOFT => {
            e.uid = py_str(key("id")?);
            e.email = data.get("mail").filter(|v| social::py_truthy(v)).or_else(|| data.get("userPrincipalName")).filter(|v| !v.is_null()).map(py_str);
            e.username = opt(data, "mailNickname");
            e.last_name = opt(data, "surname");
            e.first_name = opt(data, "givenName");
        }
        NEXTCLOUD => {
            e.uid = py_str(key("id")?);
            e.username = Some(py_str(key("displayname")?));
            e.email = opt(data, "email");
        }
        DISCORD => {
            e.uid = py_str(key("id")?);
            e.email = opt(data, "email");
            e.username = opt(data, "username");
            e.name = opt(data, "username");
            // extract_email_addresses: only `email and data.get("verified")` → verified=True
            if let Some(m) = data.get("email").filter(|v| social::py_truthy(v))
                && data.get("verified").is_some_and(crate::emailaddr::is_true_claim)
            {
                e.addresses.push(EmailAddr { email: py_str(m), verified: Value::Bool(true), primary: true });
            }
        }
        GITEA => {
            e.uid = py_str(key("id")?);
            e.email = opt(data, "email");
            e.username = opt(data, "login");
            // GiteaProvider.extract_common_fields: name=data.get("name") (not `full_name`)
            e.name = opt(data, "name");
        }
        other => return Err(format!("no oauth2 provider {other:?}")),
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review 2026-11-19 F14: error text of a failed profile request does not contain
    /// the access token from the query.
    #[tokio::test]
    async fn request_error_text_has_no_access_token() {
        // occupy a free port and release it again → connection is refused
        let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(1);
        let url = format!("http://127.0.0.1:{port}/api/v4/user");
        let Err(e) = get(&url, &[("access_token", "SECRET-TOKEN-F14")], None).await else {
            panic!("closed port must fail");
        };
        let text = e.to_string();
        assert!(!text.contains("SECRET-TOKEN-F14"), "token leaked: {text}");
        assert!(!text.contains("access_token"), "query leaked: {text}");
    }
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn github_fields() {
        let d = obj(json!({"id": 583231, "login": "octocat", "name": "The Octocat", "email": null, "emails": [{"email": "o@example.invalid", "primary": true, "verified": true}]}));
        let e = extract(GITHUB, &d).unwrap();
        assert_eq!(e.uid, "583231");
        assert_eq!(e.username.as_deref(), Some("octocat"));
        assert_eq!(e.email, None);
        assert!(!e.extra_data.contains_key("emails"));
        assert_eq!(e.addresses.len(), 1);
        assert!(e.addresses[0].is_verified());
        assert!(extract(GITHUB, &obj(json!({"login": "x"}))).is_err());
    }

    #[test]
    fn google_fields() {
        let d = obj(json!({"sub": "1234", "email": "a@example.invalid", "email_verified": true, "given_name": "A", "family_name": "B"}));
        let e = extract(GOOGLE, &d).unwrap();
        assert_eq!(e.uid, "1234");
        assert_eq!((e.first_name.as_deref(), e.last_name.as_deref()), (Some("A"), Some("B")));
        assert_eq!(e.addresses, vec![EmailAddr { email: "a@example.invalid".into(), verified: Value::Bool(true), primary: true }]);
        // v2/userinfo form
        let e = extract(GOOGLE, &obj(json!({"id": "9", "email": "b@example.invalid", "verified_email": false}))).unwrap();
        assert_eq!(e.uid, "9");
        assert!(!e.addresses[0].is_verified());
    }

    #[test]
    fn microsoft_gitlab_nextcloud_fields() {
        let e = extract(MICROSOFT, &obj(json!({"id": "abc", "mail": null, "userPrincipalName": "u@example.invalid", "mailNickname": "u", "givenName": "U", "surname": "S"}))).unwrap();
        assert_eq!(e.email.as_deref(), Some("u@example.invalid"));
        assert_eq!(e.username.as_deref(), Some("u"));
        let e = extract(GITLAB, &obj(json!({"id": 7, "username": "g", "email": "g@example.invalid", "name": "G L"}))).unwrap();
        assert_eq!((e.uid.as_str(), e.name.as_deref()), ("7", Some("G L")));
        let e = extract(NEXTCLOUD, &obj(json!({"id": "nc", "displayname": "N C"}))).unwrap();
        assert_eq!(e.username.as_deref(), Some("N C"));
        assert!(extract(NEXTCLOUD, &obj(json!({"id": "nc"}))).is_err());
    }

    #[test]
    fn discord_gitea_fields() {
        let d = obj(json!({"id": "80351110224678912", "username": "nelly", "email": "n@example.invalid", "verified": true}));
        let e = extract(DISCORD, &d).unwrap();
        assert_eq!((e.uid.as_str(), e.username.as_deref(), e.name.as_deref()), ("80351110224678912", Some("nelly"), Some("nelly")));
        assert_eq!(e.addresses, vec![EmailAddr { email: "n@example.invalid".into(), verified: Value::Bool(true), primary: true }]);
        let e = extract(DISCORD, &obj(json!({"id": 1, "username": "x"}))).unwrap();
        assert!(e.addresses.is_empty());
        assert!(extract(DISCORD, &obj(json!({"username": "x"}))).is_err());
        // unconfirmed or missing confirmation: no address (allauth 65.20.1 discord/provider.py)
        let e = extract(DISCORD, &obj(json!({"id": 2, "username": "u", "email": "u@example.invalid", "verified": false}))).unwrap();
        assert!(e.addresses.is_empty());
        assert_eq!(e.email.as_deref(), Some("u@example.invalid"));
        let e = extract(DISCORD, &obj(json!({"id": 2, "username": "u", "email": "u@example.invalid"}))).unwrap();
        assert!(e.addresses.is_empty());
        // Gitea: `name` from data["name"]; `full_name` is not read
        let e = extract(GITEA, &obj(json!({"id": 3, "login": "gt", "full_name": "Full", "name": "G T", "email": "g@example.invalid"}))).unwrap();
        assert_eq!((e.uid.as_str(), e.username.as_deref(), e.name.as_deref()), ("3", Some("gt"), Some("G T")));
        let e = extract(GITEA, &obj(json!({"id": 3, "login": "gt", "full_name": "Full"}))).unwrap();
        assert_eq!(e.name, None);
        assert_eq!(default_scope(DISCORD), vec!["email".to_string(), "identify".to_string()]);
        assert_eq!(default_scope(GITEA), Vec::<String>::new());
        assert_eq!(gitea_web_url(None), "https://gitea.com");
        assert_eq!(gitea_web_url(Some("http://127.0.0.1:3000/".into())), "http://127.0.0.1:3000");
    }

    #[test]
    fn scopes_and_errors() {
        assert_eq!(default_scope(GITHUB), Vec::<String>::new());
        assert_eq!(default_scope(GOOGLE), vec!["profile".to_string()]);
        assert_eq!(default_scope(MICROSOFT), vec!["User.Read".to_string()]);
        assert!(matches!(gitlab_check_errors(403, b"{}"), Err(OidcError::OAuth2(_))));
        assert!(matches!(gitlab_check_errors(200, b"{\"error\":\"invalid_token\"}"), Err(OidcError::OAuth2(_))));
        assert!(gitlab_check_errors(200, b"{\"id\":1}").is_ok());
        assert!(matches!(microsoft_check_errors(b"{\"error\":{\"message\":\"bad\"}}"), Err(OidcError::OAuth2(m)) if m.ends_with(": bad")));
        assert!(matches!(ok_json(404, b"{}"), Err(OidcError::Request(_))));
    }
}
