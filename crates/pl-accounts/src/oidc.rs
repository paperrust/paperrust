//! OpenID Connect protocol as allauth 65.20.1 (`openid_connect` provider on an OAuth2 basis).
//!
//! upstream: allauth socialaccount/providers/openid_connect/{provider,views}.py
//! (`server_url` + `/.well-known/openid-configuration`, `openid_configuration_overrides`,
//! `basic_auth`, `complete_login`: userinfo + ID token, `_decode_id_token` with
//! `verify_signature = not did_fetch_access_token`), socialaccount/providers/oauth2/{client,
//! provider,utils,views}.py (`get_redirect_url`, `get_access_token`, `_strip_empty_keys`,
//! `generate_code_challenge`, `get_pkce_params`, `get_scope`, `get_auth_params`,
//! `parse_token`), socialaccount/internal/jwtkit.py (`verify_and_decode`, `verify_jti`),
//! socialaccount/adapter.py (`get_requests_session`: Timeout `REQUESTS_TIMEOUT` = 5 s),
//! PyJWT 2.15 (`decode` without signature check: only `iss`, `aud`, `exp` are checked, because allauth
//! enables exactly these three explicitly; `iat`/`nbf`/`sub`/`jti` stay off).
//!
//! ID token signature: allauth does **not** check it in the code flow (token comes directly via TLS from the
//! token endpoint, OIDC Core 3.1.3.7 no. 6) – Rust does the same. allauth does not send a `nonce` for
//! `openid_connect`, so none is checked either.

use std::time::Duration;

use base64::Engine as _;
use rand::Rng;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::social::{self, App};
use crate::web::Ctx;

/// `SOCIALACCOUNT_REQUESTS_TIMEOUT`.
pub const REQUESTS_TIMEOUT: Duration = Duration::from_secs(5);

/// Error classes of the flow (like the Python exceptions).
#[derive(Debug)]
pub enum OidcError {
    /// `OAuth2Error` (→ error page 401).
    OAuth2(String),
    /// `requests.RequestException` (→ error page 401 in the callback, 500 at start).
    Request(String),
    /// Everything else (KeyError, ValueError …) → 500.
    Internal(String),
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OidcError::OAuth2(s) | OidcError::Request(s) | OidcError::Internal(s) => f.write_str(s),
        }
    }
}

fn client() -> Result<reqwest::Client, OidcError> {
    reqwest::Client::builder()
        .user_agent(pl_core::brand::USER_AGENT)
        .timeout(REQUESTS_TIMEOUT)
        .build()
        .map_err(|e| OidcError::Internal(e.to_string()))
}

/// Python `urllib.parse.quote_plus(s, safe="")` (UTF-8).
pub fn quote_plus(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `django.utils.http.urlencode` / `urllib.parse.urlencode` for `(str, str)` pairs.
pub fn urlencode(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v))).collect::<Vec<_>>().join("&")
}

/// Python `dict.update` on an ordered pair list (existing keys keep their position).
pub fn dict_update(pairs: &mut Vec<(String, String)>, k: &str, v: String) {
    match pairs.iter_mut().find(|(a, _)| a == k) {
        Some((_, old)) => *old = v,
        None => pairs.push((k.to_string(), v)),
    }
}

/// `secrets.token_urlsafe(n)`.
pub fn token_urlsafe(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::thread_rng().fill(&mut b[..]);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// `generate_code_challenge()` → (verifier, challenge).
pub fn code_challenge() -> (String, String) {
    let verifier = token_urlsafe(96);
    (verifier.clone(), challenge_for(&verifier))
}

pub fn challenge_for(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Setting of an app: `app.settings[key]`, otherwise `PROVIDERS[<provider>][KEY]`
/// (`<provider>` = class of the app, e.g. `openid_connect` or `github`).
fn app_or_provider(ctx: &Ctx, app: &App, key: &str, provider_key: &str) -> Option<Value> {
    match app.settings.get(key) {
        Some(Value::Null) | None => social::provider_settings(ctx, &app.provider).get(provider_key).cloned(),
        Some(v) => Some(v.clone()),
    }
}

/// `get_pkce_params` → enabled?
pub fn pkce_enabled(ctx: &Ctx, app: &App) -> bool {
    app_or_provider(ctx, app, "oauth_pkce_enabled", "OAUTH_PKCE_ENABLED").is_some_and(|v| social::py_truthy(&v))
}

fn py_str_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect(),
        Value::String(s) => s.chars().map(|c| c.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// `get_scope_from_request`: provider default (OIDC `["openid","profile","email"]`, see
/// `oauth2::default_scope`) or setting, plus `?scope=a,b`. Order in the URL does not matter
/// (allauth uses a `set`).
pub fn scope(ctx: &Ctx, app: &App) -> Vec<String> {
    let mut s = match app_or_provider(ctx, app, "scope", "SCOPE") {
        Some(v) => py_str_list(&v),
        None => crate::oauth2::default_scope(&app.provider),
    };
    if let Some(d) = ctx.get("scope").filter(|d| !d.is_empty()) {
        s.extend(d.split(',').map(str::to_string));
    }
    s
}

/// `get_auth_params_from_request`: setting `auth_params`/`AUTH_PARAMS` + `?auth_params=…`.
pub fn auth_params(ctx: &Ctx, app: &App) -> Vec<(String, String)> {
    let mut ret: Vec<(String, String)> = match app_or_provider(ctx, app, "auth_params", "AUTH_PARAMS") {
        Some(Value::Object(m)) => {
            m.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect()
        }
        _ => Vec::new(),
    };
    if let Some(d) = ctx.get("auth_params").filter(|d| !d.is_empty()) {
        // dict(parse_qsl(...)): blank values discarded, last one wins
        for (k, v) in url::form_urlencoded::parse(d.as_bytes()) {
            if v.is_empty() {
                continue;
            }
            dict_update(&mut ret, &k, v.into_owned());
        }
    }
    ret
}

/// `provider.server_url` (`wk_server_url`).
pub fn server_url(app: &App) -> Result<String, OidcError> {
    let Some(Value::String(url)) = app.settings.get("server_url") else {
        return Err(OidcError::Internal("KeyError: 'server_url'".into()));
    };
    Ok(if url.contains("/.well-known/") { url.clone() } else { format!("{url}/.well-known/openid-configuration") })
}

async fn get_json(url: &str, bearer: Option<&str>) -> Result<Value, OidcError> {
    let c = client()?;
    let mut rb = c.get(url);
    if let Some(t) = bearer {
        rb = rb.header("Authorization", format!("Bearer {t}"));
    }
    let r = rb.send().await.map_err(|e| OidcError::Request(e.to_string()))?;
    if r.status().is_client_error() || r.status().is_server_error() {
        return Err(OidcError::Request(format!("HTTP {}", r.status())));
    }
    let bytes = r.bytes().await.map_err(|e| OidcError::Request(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| OidcError::Request(format!("JSONDecodeError: {e}")))
}

/// `OpenIDConnectOAuth2Adapter.openid_config` (discovery + overrides).
pub async fn openid_config(app: &App) -> Result<Map<String, Value>, OidcError> {
    let url = server_url(app)?;
    let mut cfg = match get_json(&url, None).await? {
        Value::Object(m) => m,
        _ => return Err(OidcError::Internal("openid configuration is not an object".into())),
    };
    if let Some(Value::Object(o)) = app.settings.get("openid_configuration_overrides") {
        for (k, v) in o {
            cfg.insert(k.clone(), v.clone());
        }
    }
    Ok(cfg)
}

pub fn cfg_str(cfg: &Map<String, Value>, k: &str) -> Result<String, OidcError> {
    match cfg.get(k) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(v) => Ok(v.to_string()),
        None => Err(OidcError::Internal(format!("KeyError: '{k}'"))),
    }
}

/// `OpenIDConnectOAuth2Adapter.basic_auth`.
pub fn basic_auth(app: &App, cfg: &Map<String, Value>) -> bool {
    if let Some(m) = app.settings.get("token_auth_method").filter(|v| social::py_truthy(v)) {
        return m.as_str() == Some("client_secret_basic");
    }
    let methods: Vec<&str> = cfg
        .get("token_endpoint_auth_methods_supported")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    !methods.contains(&"client_secret_post") && methods.contains(&"client_secret_basic")
}

/// `OAuth2Client.get_redirect_url(authorization_url, scope, extra_params)`.
pub fn redirect_url(
    authorize_url: &str,
    client_id: &str,
    callback: &str,
    scope: &[String],
    state: &str,
    extra: &[(String, String)],
) -> String {
    let mut uniq: Vec<String> = Vec::new();
    for s in scope {
        if !uniq.contains(s) {
            uniq.push(s.clone());
        }
    }
    let mut params: Vec<(String, String)> = vec![
        ("client_id".into(), client_id.to_string()),
        ("redirect_uri".into(), callback.to_string()),
        ("scope".into(), uniq.join(" ")),
        ("response_type".into(), "code".into()),
    ];
    if !state.is_empty() {
        params.push(("state".into(), state.to_string()));
    }
    for (k, v) in extra {
        dict_update(&mut params, k, v.clone());
    }
    format!("{authorize_url}?{}", urlencode(&params))
}

/// Python `base64.b64encode(f"{id}:{secret}".encode("latin1"))` (requests `HTTPBasicAuth`).
fn basic_header(id: &str, secret: &str) -> String {
    let raw: Vec<u8> = format!("{id}:{secret}").chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect();
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

/// `OAuth2Client.get_access_token(code, pkce_code_verifier)`.
pub async fn access_token(
    app: &App,
    token_url: &str,
    callback: &str,
    code: &str,
    basic: bool,
    verifier: Option<&str>,
) -> Result<Map<String, Value>, OidcError> {
    let mut data: Vec<(String, String)> =
        vec![("redirect_uri".into(), callback.into()), ("grant_type".into(), "authorization_code".into()), ("code".into(), code.into())];
    if !basic {
        data.push(("client_id".into(), app.client_id.clone()));
        data.push(("client_secret".into(), app.secret.clone()));
    }
    data.retain(|(_, v)| !v.is_empty());
    if let Some(v) = verifier.filter(|v| !v.is_empty())
        && !data.is_empty()
    {
        data.push(("code_verifier".into(), v.into()));
    }
    let c = client()?;
    let mut rb = c.post(token_url).header("Content-Type", "application/x-www-form-urlencoded").body(urlencode(&data));
    if basic {
        rb = rb.header("Authorization", basic_header(&app.client_id, &app.secret));
    }
    let r = rb.send().await.map_err(|e| OidcError::Request(e.to_string()))?;
    let status = r.status().as_u16();
    let ct = r.headers().get("content-type").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    let body = r.bytes().await.map_err(|e| OidcError::Request(e.to_string()))?;
    let mut tok: Option<Map<String, Value>> = None;
    if status == 200 || status == 201 {
        let Some(ct) = ct else { return Err(OidcError::Internal("KeyError: 'content-type'".into())) };
        let text = String::from_utf8_lossy(&body);
        if ct.split(';').next() == Some("application/json") || text.starts_with("{\"") {
            match serde_json::from_slice::<Value>(&body) {
                Ok(Value::Object(m)) => tok = Some(m),
                Ok(_) => tok = None,
                Err(e) => return Err(OidcError::Request(format!("JSONDecodeError: {e}"))),
            }
        } else {
            let mut m = Map::new();
            for (k, v) in url::form_urlencoded::parse(&body) {
                if !v.is_empty() {
                    m.insert(k.into_owned(), Value::from(v.into_owned()));
                }
            }
            tok = Some(m);
        }
    }
    match tok {
        Some(m) if !m.is_empty() && m.contains_key("access_token") => Ok(m),
        _ => Err(OidcError::OAuth2("Error retrieving access token".into())),
    }
}

/// `_fetch_user_info(access_token)`.
pub async fn userinfo(url: &str, token: &str) -> Result<Value, OidcError> {
    get_json(url, Some(token)).await
}

fn b64url(seg: &str) -> Option<Vec<u8>> {
    // PyJWT base64url_decode: add padding, urlsafe_b64decode (non-alphabet characters are discarded)
    let clean: String = seg.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(clean.trim_end_matches('=')).ok()
}

/// `jwt.decode(credential, options={verify_signature: False, verify_iss/aud/exp: True},
/// issuer=…, audience=client_id)`. Fehler → `OAuth2Error("Invalid id_token")`.
pub fn decode_id_token(token: &str, issuer: &str, audience: &str, now: f64) -> Result<Map<String, Value>, OidcError> {
    let bad = || OidcError::OAuth2("Invalid id_token".into());
    let (signing_input, sig) = token.rsplit_once('.').ok_or_else(bad)?;
    let (head, payload) = signing_input.split_once('.').ok_or_else(bad)?;
    let header: Value = serde_json::from_slice(&b64url(head).ok_or_else(bad)?).map_err(|_| bad())?;
    let Value::Object(header) = header else { return Err(bad()) };
    if header.get("kid").is_some_and(|k| !k.is_string()) {
        return Err(bad());
    }
    b64url(sig).ok_or_else(bad)?;
    let payload: Value = serde_json::from_slice(&b64url(payload).ok_or_else(bad)?).map_err(|_| bad())?;
    let Value::Object(p) = payload else { return Err(bad()) };
    // exp
    if let Some(exp) = p.get("exp") {
        let e = match exp {
            Value::Number(n) => n.as_f64().map(|f| f.trunc()),
            Value::String(s) => s.trim().parse::<i64>().ok().map(|i| i as f64),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        };
        match e {
            Some(e) if e > now => {}
            _ => return Err(bad()),
        }
    }
    // iss
    match p.get("iss") {
        Some(Value::String(s)) if s == issuer => {}
        _ => return Err(bad()),
    }
    // aud
    let claims: Vec<&Value> = match p.get("aud") {
        None => return Err(bad()),
        Some(v) if !social::py_truthy(v) => return Err(bad()),
        Some(Value::String(_)) => vec![p.get("aud").ok_or_else(bad)?],
        Some(Value::Array(a)) => a.iter().collect(),
        Some(_) => return Err(bad()),
    };
    if claims.iter().any(|c| !c.is_string()) || !claims.iter().any(|c| c.as_str() == Some(audience)) {
        return Err(bad());
    }
    Ok(p)
}

/// `verify_jti`: cache key and lifetime (`None` = no check).
pub fn jti_key(p: &Map<String, Value>, now: f64) -> Option<(String, f64)> {
    let (iss, exp, jti) = (p.get("iss")?, p.get("exp")?, p.get("jti")?);
    if iss.is_null() || exp.is_null() || jti.is_null() {
        return None;
    }
    let py = |v: &Value| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
    let timeout = exp.as_f64().unwrap_or(0.0) - now;
    Some((format!("jwt:iss={},jti={}", py(iss), py(jti)), timeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(claims: &Value) -> String {
        let e = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        format!("{}.{}.{}", e(&serde_json::json!({"alg":"HS256","kid":"k"})), e(claims), "c2ln")
    }

    #[test]
    fn quote_like_python() {
        assert_eq!(quote_plus("a b~c*d/é"), "a+b~c%2Ad%2F%C3%A9");
        assert_eq!(urlencode(&[("scope".into(), "openid email".into())]), "scope=openid+email");
    }

    #[test]
    fn pkce_challenge() {
        // RFC 7636 Appendix B
        assert_eq!(challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let (v, c) = code_challenge();
        assert_eq!(v.len(), 128);
        assert_eq!(challenge_for(&v), c);
    }

    #[test]
    fn id_token_claims_like_pyjwt() {
        let now = 1_000_000.0;
        let ok = serde_json::json!({"iss":"https://idp","aud":"cid","exp":1_000_100,"sub":"x"});
        assert!(decode_id_token(&jwt(&ok), "https://idp", "cid", now).is_ok());
        let list = serde_json::json!({"iss":"https://idp","aud":["a","cid"],"exp":1_000_100});
        assert!(decode_id_token(&jwt(&list), "https://idp", "cid", now).is_ok());
        for bad in [
            serde_json::json!({"iss":"https://other","aud":"cid","exp":1_000_100}),
            serde_json::json!({"aud":"cid","exp":1_000_100}),
            serde_json::json!({"iss":"https://idp","aud":"other","exp":1_000_100}),
            serde_json::json!({"iss":"https://idp","exp":1_000_100}),
            serde_json::json!({"iss":"https://idp","aud":"cid","exp":1_000_000}),
            serde_json::json!({"iss":"https://idp","aud":"cid","exp":"soon"}),
            serde_json::json!({"iss":"https://idp","aud":["cid", 1],"exp":1_000_100}),
        ] {
            assert!(decode_id_token(&jwt(&bad), "https://idp", "cid", now).is_err(), "{bad}");
        }
        // without exp: not checked; signature irrelevant
        let noexp = serde_json::json!({"iss":"https://idp","aud":"cid"});
        assert!(decode_id_token(&jwt(&noexp), "https://idp", "cid", now).is_ok());
        assert!(decode_id_token("a.b", "https://idp", "cid", now).is_err());
    }

    #[test]
    fn redirect_url_order() {
        let u = redirect_url(
            "http://i/authorize",
            "cid",
            "https://h/cb/",
            &["openid".into(), "openid".into()],
            "st",
            &[("login_hint".into(), "a~b".into())],
        );
        assert_eq!(
            u,
            "http://i/authorize?client_id=cid&redirect_uri=https%3A%2F%2Fh%2Fcb%2F&scope=openid&response_type=code&state=st&login_hint=a~b"
        );
    }
}
