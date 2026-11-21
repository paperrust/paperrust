//! `POST /api/token/`
//!
//! upstream: src/paperless/views.py@v3.4.2 (PaperlessObtainAuthTokenView: ScopedRateThrottle,
//! throttle_scope "login"), src/paperless/serialisers.py@v3.4.2 (PaperlessAuthTokenSerializer),
//! DRF 3.18.3 rest_framework/authtoken/views.py (ObtainAuthToken.post),
//! rest_framework/authtoken/serializers.py (AuthTokenSerializer.validate),
//! allauth 65.20.1 mfa/totp/internal/auth.py (TOTP.validate_code, _mark_code_used).

use crate::http::{FieldInput, FieldOpts, HResult, get_value, parse_data, remote_addr, server_error};
use crate::{db, django_cache, throttle, totp};
use axum::body::Bytes;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use pl_authz::{Authz, MaybeUser};
use pl_core::{ApiError, AppState, Drf, RequestContext, ValidationErrors};
use serde_json::{Value, json};

/// allauth `TOTP._get_used_cache_key`.
pub fn used_code_key(user_id: i64, code: &str) -> String {
    format!("allauth.mfa.totp.used?user={user_id}&code={}", totp::normalize_user_code(code))
}

/// allauth `TOTP._mark_code_used`: `cache.add(key, "y", timeout=TOTP_PERIOD)` in Django's
/// cache (atomic `SET NX EX`), so a code accepted here is used up for Django too and vice versa
/// (Review 2 M2).
async fn mark_code_used(state: &AppState, user_id: i64, code: &str) -> Result<bool, ApiError> {
    let key = used_code_key(user_id, code);
    // Redis: `SET NX EX` (byte-identical with Django); local: `LocMemCache.add` in the shared
    // Prozess-Cache (pl_core::cache).
    state.cache().add(&key, django_cache::PICKLED_Y, Some(totp::TOTP_PERIOD.max(1))).await
}

fn now_f64() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `APIView.initial` throttling for scope `login`.
pub async fn check_login_throttle(state: &AppState, parts: &Parts, user_id: Option<i64>) -> Result<(), ApiError> {
    // PAPERLESS_TOKEN_THROTTLE_RATE live from the web UI (docs/SETTINGS.md WP4b).
    let cfg = state.live_config();
    let Some(rate) = throttle::login_rate(&cfg) else {
        return Err(ApiError::internal("invalid PAPERRUST_TOKEN_THROTTLE_RATE"));
    };
    let ident = match user_id {
        Some(id) => id.to_string(),
        None => {
            let xff = parts.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
            throttle::get_ident(xff, remote_addr(parts).as_deref())
        }
    };
    let key = format!("throttle_login_{ident}");
    let outcome = throttle::allow_cached(&state.cache(), &key, rate, now_f64()).await?;
    outcome.map_err(ApiError::throttled)
}

/// Cache key of the throttle per username (lowercased, hex, at most 64 characters of name).
pub fn username_throttle_key(username: &str) -> String {
    let lower = username.trim().to_lowercase();
    let hex: String = lower.chars().take(64).collect::<String>().bytes().map(|b| format!("{b:02x}")).collect();
    format!("throttle_login_user_{hex}")
}

/// PaperRust (security review 2026-11-19 F3): in addition to the upstream throttle per client address,
/// a throttle per **username** with the same rate. The address comes from `X-Forwarded-For` and
/// is freely choosable; without this second limit, password and then the TOTP code could be
/// guessed without limit. Counts every attempt (like DRF), i.e. password + code together too.
pub async fn check_username_throttle(state: &AppState, username: &str) -> Result<(), ApiError> {
    let cfg = state.live_config();
    let Some(rate) = throttle::login_rate(&cfg) else {
        return Err(ApiError::internal("invalid PAPERRUST_TOKEN_THROTTLE_RATE"));
    };
    let outcome = throttle::allow_cached(&state.cache(), &username_throttle_key(username), rate, now_f64()).await?;
    outcome.map_err(ApiError::throttled)
}

pub async fn obtain_token(State(state): State<AppState>, req: Request) -> Response {
    match obtain_token_inner(state, req).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn obtain_token_inner(state: AppState, req: Request) -> HResult<Response> {
    let (mut parts, body) = req.into_parts();
    // initial(): negotiation/version, authentication, (no permissions), throttling
    let _ctx = RequestContext::from_request_parts(&mut parts, &state).await?;
    let maybe = MaybeUser::from_request_parts(&mut parts, &state).await?;
    check_login_throttle(&state, &parts, maybe.user().map(|u| u.id)).await?;
    // APIView.dispatch: the handler is looked up only after `initial()` – so a GET (or any other
    // method) counts in the login throttle too, then OPTIONS gets the metadata and every other
    // method DRF's 405 (parity run 2026-11-20, F2).
    match parts.method {
        axum::http::Method::POST => {}
        axum::http::Method::OPTIONS => return Ok(Drf::ok(options_metadata()).into_response()),
        ref m => return Err(ApiError::method_not_allowed(m.as_str()).into()),
    }

    let body: Bytes = pl_core::body::read_body(body).await?;
    let data = parse_data(&parts, body).await?;
    data.require_mapping()?;

    // field validation (declaration order: username, password, code)
    let mut errs = ValidationErrors::new();
    let username = char_field(&data, "username", true, true, &mut errs);
    let password = char_field(&data, "password", true, false, &mut errs);
    let code = char_field(&data, "code", false, true, &mut errs);
    errs.into_result()?;
    let (Some(username), Some(password)) = (username, password) else {
        return Err(ApiError::non_field_error("Must include \"username\" and \"password\".").into());
    };
    check_username_throttle(&state, &username).await?;

    let authz = parts.extensions.get::<Authz>().cloned().ok_or_else(|| server_error("pl_authz::Authz missing in request extensions"))?;
    // S7: client IP (ipware over headers + TCP peer) in the failed-login log
    let meta = pl_authz::RequestMeta {
        remote_addr: parts.extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0.ip()),
        ..Default::default()
    };
    let Some(user) = authz.authenticate_credentials_from(&username, &password, &parts.headers, &meta).await? else {
        return Err(ApiError::non_field_error("Unable to log in with provided credentials.").into());
    };

    let db = state.db();
    if pl_authz::user::is_mfa_enabled(db, user.id).await? {
        let Some(code) = code.filter(|c| !c.is_empty()) else {
            return Err(ApiError::non_field_error("MFA code is required").into());
        };
        // Authenticator.objects.get(user=user, type=TOTP): DoesNotExist → 500
        let totp_auth = db::authenticators(db, user.id, Some("totp"))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| server_error("Authenticator matching query does not exist."))?;
        let secret = totp_auth.data.get("secret").and_then(Value::as_str).unwrap_or("");
        let valid = totp::validate_totp_code(secret, &code).map_err(|_| server_error("binascii.Error: invalid TOTP secret"))?;
        if !(valid && mark_code_used(&state, user.id, &code).await?) {
            return Err(ApiError::non_field_error("Invalid MFA code").into());
        }
    }

    let key = db::get_or_create_token(db, user.id).await?;
    Ok(Drf::ok(json!({ "token": key })).into_response())
}

/// `SimpleMetadata.determine_metadata` for `PaperlessObtainAuthTokenView` (DRF 3.18.3
/// rest_framework/metadata.py, authtoken/views.py `ObtainAuthToken`: `renderer_classes =
/// (JSONRenderer,)`, `parser_classes = (FormParser, MultiPartParser, JSONParser)`,
/// `permission_classes = ()` → `actions.POST` with the fields of `PaperlessAuthTokenSerializer`).
/// Labels in English (upstream translates DRF's `Username`/`Password`/`Token` with the active
/// language; Known difference).
pub fn options_metadata() -> Value {
    let field = |required: bool, read_only: bool, label: &str| json!({"type": "string", "required": required, "read_only": read_only, "label": label});
    json!({
        "name": "Paperless Obtain Auth Token",
        "description": "",
        "renders": ["application/json"],
        "parses": ["application/x-www-form-urlencoded", "multipart/form-data", "application/json"],
        "actions": {
            "POST": {
                "username": field(true, false, "Username"),
                "password": field(true, false, "Password"),
                "token": field(false, true, "Token"),
                "code": field(false, false, "MFA Code"),
            }
        }
    })
}

/// `serializers.CharField(write_only=True[, required=False])` → value or error collected.
fn char_field(data: &crate::http::Data, name: &str, required: bool, trim: bool, errs: &mut ValidationErrors) -> Option<String> {
    let opts = FieldOpts { required, ..Default::default() };
    match get_value(data, name, opts) {
        FieldInput::Missing => {
            if required {
                errs.add(name, "This field is required.");
            }
            None
        }
        FieldInput::Value(v) => match crate::http::drf_char(&v, false, None, trim) {
            Ok(s) => Some(s),
            Err(m) => {
                errs.add(name, m);
                None
            }
        },
    }
}

#[cfg(test)]
mod username_throttle_tests {
    use super::*;

    /// Security review F3: same key regardless of case and whitespace, different
    /// names kept apart; overlong names truncated.
    #[test]
    fn username_key_is_normalized() {
        assert_eq!(username_throttle_key("Admin"), username_throttle_key(" admin "));
        assert_ne!(username_throttle_key("admin"), username_throttle_key("admin2"));
        assert!(username_throttle_key(&"x".repeat(10_000)).len() <= "throttle_login_user_".len() + 128);
        // five attempts per name and minute, the sixth is throttled – regardless of which address
        let rate = throttle::parse_rate("5/min").unwrap();
        let mut h = Vec::new();
        for i in 0..5 {
            h = throttle::decide(h, rate, 1000.0 + f64::from(i)).unwrap();
        }
        assert!(throttle::decide(h, rate, 1006.0).is_err());
    }
}

#[cfg(test)]
mod options_tests {
    use super::*;

    /// `OPTIONS /api/token/`: SimpleMetadata of ObtainAuthToken, key order as DRF renders it.
    #[test]
    fn metadata_like_drf() {
        let s = serde_json::to_string(&options_metadata()).unwrap_or_default();
        assert_eq!(
            s,
            concat!(
                r#"{"name":"Paperless Obtain Auth Token","description":"","renders":["application/json"],"#,
                r#""parses":["application/x-www-form-urlencoded","multipart/form-data","application/json"],"#,
                r#""actions":{"POST":{"username":{"type":"string","required":true,"read_only":false,"label":"Username"},"#,
                r#""password":{"type":"string","required":true,"read_only":false,"label":"Password"},"#,
                r#""token":{"type":"string","required":false,"read_only":true,"label":"Token"},"#,
                r#""code":{"type":"string","required":false,"read_only":false,"label":"MFA Code"}}}}"#
            )
        );
    }
}
