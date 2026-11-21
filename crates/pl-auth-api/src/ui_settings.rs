//! `/api/ui_settings/` (`UiSettingsView`).
//!
//! upstream: src/documents/views.py@v3.4.2 (UiSettingsView.get/post),
//! src/documents/serialisers.py@v3.4.2 (UiSettingsViewSerializer: DictField, validate_settings,
//! create → update_or_create), src/paperless/config.py@v3.4.2 (GeneralConfig, AIConfig,
//! RemoteOCRConfig, BaseConfig._get_config_instance), src/paperless/parsers/remote.py@v3.4.2
//! (RemoteEngineConfig.engine_is_valid), src/paperless/version.py@v3.4.2,
//! src/paperless/settings/__init__.py@v3.4.2 (ENABLE_UPDATE_CHECK, EMPTY_TRASH_DELAY,
//! APP_TITLE, APP_LOGO, AUDIT_LOG_ENABLED, GMAIL/OUTLOOK_OAUTH_ENABLED, EMAIL_ENABLED,
//! AI_ENABLED, REMOTE_OCR_*).

use crate::db::a;
use crate::http::{Data, FieldInput, FieldOpts, HResult, get_value, py_type_name, server_error};
use crate::profile::begin;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use pl_authz::ObjectModel;
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{ApiError, AppState, Config, Drf};
use sea_query::{Expr, PostgresQueryBuilder, Query};
use serde_json::{Map, Value, json};
use sqlx::Row;

pub const UISETTINGS_MODEL: ObjectModel = ObjectModel {
    app_label: "documents",
    model_name: "uisettings",
    object_name: "UiSettings",
    table: "documents_uisettings",
    soft_delete: false,
};

/// `paperless.version.__full_version_str__` – replicated upstream version. The unchanged
/// UI shows it in the sidebar; the own version is in `/api/status/`
/// `paperrust.version` (R4).
pub const VERSION: &str = pl_core::brand::UPSTREAM_VERSION;

fn respond(r: HResult<Response>) -> Response {
    match r {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// `rest_framework.utils.html.parse_html_dict(dictionary, prefix="settings")` + `DictField`
/// child `_UnvalidatedField`: `{key: last value}` for every `settings.<key>` (MultiValueDict:
/// position of the first occurrence, value of the last).
fn form_settings(pairs: &[(String, String)]) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in pairs {
        if let Some(key) = k.strip_prefix("settings.")
            && !key.is_empty()
        {
            out.insert(key.to_string(), Value::String(v.clone()));
        }
    }
    out
}

/// `UiSettingsViewSerializer(data=request.data).is_valid(raise_exception=True)` →
/// `Some(validated settings)` (`None` = field absent). `Err` 400 / 500 like upstream.
fn validate(data: &Data) -> HResult<Option<Value>> {
    data.require_mapping()?;
    let v = match data {
        // DictField.get_value for HTML input: parse_html_dict(prefix="settings") - only
        // "settings.<k>" keys count and the field is always present (empty dict without them).
        // W2 parity run 2026-11-08 (w2.ui_settings.errors: form settings="{}" -> 200, {}).
        Data::Form(pairs) => Value::Object(form_settings(pairs)),
        Data::Json(_) => {
            let opts = FieldOpts { allow_null: true, ..Default::default() };
            match get_value(data, "settings", opts) {
                FieldInput::Missing => return Ok(None),
                FieldInput::Value(v) => v,
            }
        }
    };
    match v {
        Value::Null => Err(server_error("TypeError: argument of type 'NoneType' is not iterable")),
        Value::Object(mut m) => {
            // validate_settings: never store the backend setting
            if let Some(uc) = m.get_mut("update_checking") {
                match uc {
                    Value::Object(o) => {
                        o.shift_remove("backend_setting");
                    }
                    Value::Array(_) => return Err(server_error("TypeError: list.pop() with str index")),
                    _ => return Err(server_error("AttributeError: object has no attribute 'pop'")),
                }
            }
            Ok(Some(Value::Object(m)))
        }
        other => {
            // form input "settings=x" is a str as well
            Err(ApiError::field_error("settings", format!("Expected a dictionary of items but got type \"{}\".", py_type_name(&other)))
                .into())
        }
    }
}

/// `settings.GMAIL_OAUTH_ENABLED or settings.OUTLOOK_OAUTH_ENABLED` (computed once in
/// `pl_core::Config`).
fn mail_oauth_enabled(cfg: &Config) -> bool {
    cfg.gmail_oauth_enabled || cfg.outlook_oauth_enabled
}

/// `secrets.token_urlsafe(32)`.
fn token_urlsafe_32() -> String {
    use base64::Engine as _;
    use rand::RngCore as _;
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// `PaperlessMailOAuth2Manager.oauth_callback_url`:
/// `f"{OAUTH_CALLBACK_BASE_URL if not None else PAPERLESS_URL}{BASE_URL}api/oauth/callback/"`.
pub fn oauth_callback_url(cfg: &Config) -> String {
    let base = cfg.oauth_callback_base_url.clone().or_else(|| cfg.paperless_url.clone()).unwrap_or_else(|| "None".into());
    format!("{base}{}api/oauth/callback/", cfg.base_url)
}

/// `urllib.parse.urlencode(params)`.
fn urlencode(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", pl_core::urlenc::quote_plus(k, ""), pl_core::urlenc::quote_plus(v, "")))
        .collect::<Vec<_>>()
        .join("&")
}

/// httpx-oauth `GoogleOAuth2.get_authorization_url(redirect_uri, scope=["https://mail.google.com/"],
/// extras_params={"prompt": "consent", "access_type": "offline"}, state)`.
pub fn gmail_authorization_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    let q = urlencode(&[
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("state", state),
        ("scope", "https://mail.google.com/"),
        ("prompt", "consent"),
        ("access_type", "offline"),
    ]);
    format!("https://accounts.google.com/o/oauth2/v2/auth?{q}")
}

/// httpx-oauth `MicrosoftGraphOAuth2(tenant="common").get_authorization_url(redirect_uri,
/// scope=["offline_access", "https://outlook.office.com/IMAP.AccessAsUser.All"], state)`
/// (`extras_params["response_mode"] = "query"`).
pub fn outlook_authorization_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    let q = urlencode(&[
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("state", state),
        ("scope", "offline_access https://outlook.office.com/IMAP.AccessAsUser.All"),
        ("response_mode", "query"),
    ]);
    format!("https://login.microsoftonline.com/common/oauth2/v2.0/authorize?{q}")
}

/// Django `filepath_to_uri` (`quote(path, safe="/~!*()'")`).
fn filepath_to_uri(name: &str) -> String {
    crate::totp::py_quote(&name.replace('\\', "/"), "/~!*()'")
}

/// `ApplicationConfiguration.objects.all().first()` columns used here.
#[derive(Default)]
struct AppConfig {
    app_title: Option<String>,
    app_logo: Option<String>,
    ai_enabled: Option<bool>,
    remote_ocr_engine: Option<String>,
    remote_ocr_api_key: Option<String>,
    remote_ocr_endpoint: Option<String>,
    remote_ocr_mode: Option<String>,
}

/// Stored settings of one user plus the `ApplicationConfiguration` columns in **one** query
/// (R4, 2026-11-09, Bericht O1: before two round trips).
///
/// `stored`: `None` = no `UiSettings` row, `Some(None)` = row with SQL NULL.
struct Loaded {
    stored: Option<Option<sqlx::types::Json<Value>>>,
    ac: AppConfig,
}

/// `user.ui_settings` (OneToOne, `user_id` unique) and `ApplicationConfiguration.objects.first()`.
const LOAD_SQL: &str = "\
SELECT (u.user_id IS NOT NULL) AS has_ui, u.settings, \
       c.app_title, c.app_logo, c.ai_enabled, \
       c.remote_ocr_engine, c.remote_ocr_api_key, c.remote_ocr_endpoint, c.remote_ocr_mode \
FROM (SELECT 1) AS one \
LEFT JOIN documents_uisettings u ON u.user_id = $1 \
LEFT JOIN LATERAL (SELECT app_title, app_logo, ai_enabled, remote_ocr_engine, remote_ocr_api_key, \
    remote_ocr_endpoint, remote_ocr_mode \
    FROM paperless_applicationconfiguration ORDER BY id ASC LIMIT 1) c ON true";

async fn load(db: &sqlx::PgPool, user_id: i64) -> Result<Loaded, sqlx::Error> {
    let r = sqlx::query(LOAD_SQL).bind(user_id).fetch_one(db).await?;
    let has_ui: bool = r.try_get("has_ui")?;
    let stored = if has_ui { Some(r.try_get::<Option<sqlx::types::Json<Value>>, _>("settings")?) } else { None };
    Ok(Loaded {
        stored,
        ac: AppConfig {
            app_title: r.try_get("app_title")?,
            app_logo: r.try_get("app_logo")?,
            ai_enabled: r.try_get("ai_enabled")?,
            remote_ocr_engine: r.try_get("remote_ocr_engine")?,
            remote_ocr_api_key: r.try_get("remote_ocr_api_key")?,
            remote_ocr_endpoint: r.try_get("remote_ocr_endpoint")?,
            remote_ocr_mode: r.try_get("remote_ocr_mode")?,
        },
    })
}

/// Python `a or b` on optional strings (empty string is falsy).
fn or_env(db: Option<String>, env: Option<String>) -> Option<String> {
    db.filter(|s| !s.is_empty()).or(env)
}

pub async fn get(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            let user = auth.require_model_perms(&UISETTINGS_MODEL)?.clone();
            let data = r.data().await?;
            validate(&data)?;
            // email_enabled / trash_delay live (docs/SETTINGS.md WP4b).
            let cfg = state.live_config();
            let db = state.db();

            let Loaded { stored, ac } = load(db, user.id).await?;
            let mut settings: Map<String, Value> = match stored {
                None => Map::new(),
                Some(None) => return Err(server_error("TypeError: argument of type 'NoneType' is not iterable")),
                Some(Some(sqlx::types::Json(Value::Object(m)))) => m,
                Some(Some(_)) => return Err(server_error("TypeError: stored ui settings are not a dict")),
            };

            let backend_setting = match cfg.enable_update_check {
                None => Value::String("default".into()),
                Some(b) => Value::Bool(b),
            };
            match settings.get_mut("update_checking") {
                Some(Value::Object(o)) => {
                    o.insert("backend_setting".into(), backend_setting);
                }
                Some(_) => return Err(server_error("TypeError: update_checking is not a dict")),
                None => {
                    settings.insert("update_checking".into(), json!({ "backend_setting": backend_setting }));
                }
            }
            settings.insert("trash_delay".into(), Value::from(cfg.empty_trash_delay));

            settings.insert("version".into(), Value::String(VERSION.into()));
            let title = ac.app_title.clone().filter(|s| !s.is_empty()).or_else(|| cfg.app_title.clone());
            settings.insert("app_title".into(), title.map(Value::String).unwrap_or(Value::Null));
            let logo = match ac.app_logo.clone().filter(|s| !s.is_empty()) {
                Some(name) => Some(format!("{}{}", cfg.base_url, filepath_to_uri(&name))),
                None => cfg.app_logo.clone(),
            };
            settings.insert("app_logo".into(), logo.map(Value::String).unwrap_or(Value::Null));
            settings.insert("auditlog_enabled".into(), Value::Bool(cfg.audit_log_enabled));

            let engine = or_env(ac.remote_ocr_engine, cfg.remote_ocr_engine.clone());
            let api_key = or_env(ac.remote_ocr_api_key, cfg.remote_ocr_api_key.clone());
            let endpoint = or_env(ac.remote_ocr_endpoint, cfg.remote_ocr_endpoint.clone());
            let mode = or_env(ac.remote_ocr_mode, Some(cfg.remote_ocr_mode.clone()));
            let configured = engine.as_deref() == Some("azureai") && api_key.is_some() && endpoint.is_some();
            settings.insert("remote_ocr".into(), json!({ "configured": configured, "mode": mode }));

            // PaperlessMailOAuth2Manager(): new state per request, stored in the session (standalone,
            // docs/STANDALONE-GAPS.md; consumed by `/api/oauth/callback/` in pl-mail).
            let mut set_cookie = None;
            if mail_oauth_enabled(&cfg) {
                let oauth_state = token_urlsafe_32();
                let callback = oauth_callback_url(&cfg);
                if cfg.gmail_oauth_enabled {
                    let url = gmail_authorization_url(cfg.gmail_oauth_client_id.as_deref().unwrap_or(""), &callback, &oauth_state);
                    settings.insert("gmail_oauth_url".into(), Value::String(url));
                }
                if cfg.outlook_oauth_enabled {
                    let url = outlook_authorization_url(cfg.outlook_oauth_client_id.as_deref().unwrap_or(""), &callback, &oauth_state);
                    settings.insert("outlook_oauth_url".into(), Value::String(url));
                }
                set_cookie =
                    Some(crate::session::set_session_value(&state, &r.parts.headers, "oauth_state", Value::String(oauth_state)).await?);
            }

            settings.insert("email_enabled".into(), Value::Bool(cfg.email_enabled));
            settings.insert("ai_enabled".into(), Value::Bool(ac.ai_enabled.unwrap_or(cfg.ai_enabled)));

            let mut user_resp = Map::new();
            user_resp.insert("id".into(), Value::from(user.id));
            user_resp.insert("username".into(), Value::String(user.username.clone()));
            user_resp.insert("is_staff".into(), Value::Bool(user.is_staff));
            user_resp.insert("is_superuser".into(), Value::Bool(user.is_superuser));
            user_resp.insert("groups".into(), Value::Array(user.groups.iter().copied().map(Value::from).collect()));
            if !user.first_name.is_empty() {
                user_resp.insert("first_name".into(), Value::String(user.first_name.clone()));
            }
            if !user.last_name.is_empty() {
                user_resp.insert("last_name".into(), Value::String(user.last_name.clone()));
            }

            // get_all_permissions() with `^\w+.` stripped; inactive users have none
            let mut perms: Vec<String> =
                if user.is_active { user.permissions.iter().map(|p| strip_app_label(p)).collect() } else { Vec::new() };
            perms.sort();

            let mut body = Map::new();
            body.insert("user".into(), Value::Object(user_resp));
            body.insert("settings".into(), Value::Object(settings));
            body.insert("permissions".into(), Value::Array(perms.into_iter().map(Value::String).collect()));
            let mut resp = Drf::ok(Value::Object(body)).into_response();
            if let Some(c) = set_cookie
                && let Ok(v) = axum::http::HeaderValue::from_str(&c)
            {
                resp.headers_mut().append(axum::http::header::SET_COOKIE, v);
            }
            Ok(resp)
        }
        .await,
    )
}

/// `re.sub(r"^\w+.", "", perm)`: greedy word run at the start plus one more character.
fn strip_app_label(perm: &str) -> String {
    let mut chars = perm.char_indices().peekable();
    let mut end_word = None;
    while let Some((i, c)) = chars.peek().copied() {
        if c.is_alphanumeric() || c == '_' {
            end_word = Some(i + c.len_utf8());
            chars.next();
        } else {
            break;
        }
    }
    let Some(end) = end_word else {
        return perm.to_string();
    };
    // `\w+` backtracks one char if the word run reaches the end of the string
    let rest = &perm[end..];
    match rest.chars().next() {
        Some(c) => perm[end + c.len_utf8()..].to_string(),
        None => {
            let word = &perm[..end];
            if word.chars().count() >= 2 { String::new() } else { perm.to_string() }
        }
    }
}

pub async fn post(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            let user = auth.require_model_perms(&UISETTINGS_MODEL)?.clone();
            let data = r.data().await?;
            let settings = validate(&data)?;
            let value = match settings {
                Some(v) => Expr::val(v.to_string()).cast_as(a("jsonb")),
                None => Expr::val(Option::<String>::None).cast_as(a("jsonb")),
            };
            // UiSettings.objects.update_or_create(user=user, defaults={"settings": ...})
            let (sql, values) = Query::insert()
                .into_table(a("documents_uisettings"))
                .columns([a("settings"), a("user_id")])
                .values([value, user.id.into()])
                .map_err(crate::db::qerr)?
                .on_conflict(sea_query::OnConflict::column(a("user_id")).update_column(a("settings")).to_owned())
                .build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, values).execute(state.db()).await?;
            Ok(Drf::ok(json!({ "success": true })).into_response())
        }
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same strings as Python `urlencode` (checked against CPython 3.12).
    #[test]
    fn oauth_urls_like_httpx_oauth() {
        assert_eq!(
            gmail_authorization_url("cid.apps", "http://h:8000/api/oauth/callback/", "S-t_1"),
            "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id=cid.apps&redirect_uri=http%3A%2F%2Fh%3A8000%2Fapi%2Foauth%2Fcallback%2F&state=S-t_1&scope=https%3A%2F%2Fmail.google.com%2F&prompt=consent&access_type=offline"
        );
        assert_eq!(
            outlook_authorization_url("cid", "http://h:8000/api/oauth/callback/", "S"),
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?response_type=code&client_id=cid&redirect_uri=http%3A%2F%2Fh%3A8000%2Fapi%2Foauth%2Fcallback%2F&state=S&scope=offline_access+https%3A%2F%2Foutlook.office.com%2FIMAP.AccessAsUser.All&response_mode=query"
        );
        let t = token_urlsafe_32();
        assert_eq!(t.len(), 43);
        assert!(t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }

    /// Settings duplicates removed: OAuth flag comes from `pl_core::Config`.
    #[test]
    fn mail_oauth_from_core_config() {
        let mk = |pairs: &[(&str, &str)]| {
            let mut v = vec![("PAPERLESS_SECRET_KEY", "test-only-0123456789abcdefghijklmnopqrstuvwxyzABCDEFG")];
            v.extend_from_slice(pairs);
            Config::from_env_map(pl_core::config::Env::from_pairs(v)).unwrap()
        };
        assert!(!mail_oauth_enabled(&mk(&[])));
        assert!(mail_oauth_enabled(&mk(&[
            ("PAPERLESS_URL", "https://p.example"),
            ("PAPERLESS_GMAIL_OAUTH_CLIENT_ID", "id"),
            ("PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET", "s"),
        ])));
        assert!(!mail_oauth_enabled(&mk(&[("PAPERLESS_GMAIL_OAUTH_CLIENT_ID", "id"), ("PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET", "s")])));
    }

    #[test]
    fn strip_label() {
        assert_eq!(strip_app_label("documents.view_document"), "view_document");
        assert_eq!(strip_app_label("auth.add_user"), "add_user");
        assert_eq!(strip_app_label("paperless_mail.view_mailrule"), "view_mailrule");
    }

    #[test]
    fn validate_settings_field() {
        let d = Data::Json(json!({"settings": {"update_checking": {"enabled": true, "backend_setting": "x"}}}));
        assert_eq!(validate(&d).unwrap(), Some(json!({"update_checking": {"enabled": true}})));
        let bad = Data::Json(json!({"settings": "not a dict"}));
        assert!(validate(&bad).is_err());
        assert_eq!(validate(&Data::empty()).unwrap(), None);
    }

    /// W2 parity run 2026-11-08 (`w2.ui_settings.errors`): form bodies go through
    /// `parse_html_dict(prefix="settings")`.
    #[test]
    fn validate_settings_form() {
        let f = |p: &[(&str, &str)]| Data::Form(p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        assert_eq!(validate(&f(&[("settings", "{}")])).unwrap(), Some(json!({})));
        assert_eq!(validate(&f(&[])).unwrap(), Some(json!({})));
        assert_eq!(
            validate(&f(&[("settings.a", "1"), ("x", "y"), ("settings.b", "2"), ("settings.a", "3")])).unwrap(),
            Some(json!({"a": "3", "b": "2"}))
        );
        // validate_settings: str has no .pop -> AttributeError -> 500
        assert!(validate(&f(&[("settings.update_checking", "x")])).is_err());
    }

    #[test]
    fn logo_uri() {
        assert_eq!(filepath_to_uri("logo/my logo.png"), "logo/my%20logo.png");
    }
}
