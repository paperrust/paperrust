//! `/api/profile/` and its sub-routes.
//!
//! upstream: src/paperless/views.py@v3.4.2 (ProfileView, GenerateAuthTokenView,
//! DisconnectSocialAccountView, TOTPView), src/paperless/serialisers.py@v3.4.2
//! (ProfileSerializer, PasswordValidationMixin, SocialAccountSerializer),
//! allauth 65.20.1 mfa (see totp.rs / db.rs).

use crate::http::{Data, FieldInput, FieldOpts, HErr, HResult, django_text, drf_char, get_value, is_valid_email, parse_data, server_error};
use crate::password::{UserAttrs, has_real_password, validate_password};
use crate::{db, qr, session, social, totp};
use axum::body::Bytes;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use pl_authz::{AuthUser, User};
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{ApiError, AppState, Drf, RequestContext, ValidationErrors};
use sea_query::{Expr, PostgresQueryBuilder, Query};
use serde_json::{Map, Value, json};
use sqlx::Row;

/// DRF `initial()` for an `IsAuthenticated` view: negotiation/version, authentication,
/// `IsAuthenticated`. The body is read but parsed lazily ([`Req::data`]) like `request.data`.
pub async fn begin(state: &AppState, req: Request) -> HResult<(Req, AuthUser)> {
    let (mut parts, body) = req.into_parts();
    let _ctx = RequestContext::from_request_parts(&mut parts, state).await?;
    let auth = AuthUser::from_request_parts(&mut parts, state).await?;
    let bytes: Bytes = pl_core::body::read_body(body).await?;
    Ok((Req { parts, body: bytes }, auth))
}

/// Request parts + raw body.
pub struct Req {
    pub parts: Parts,
    pub body: Bytes,
}

impl Req {
    /// `request.data` (parsed on first access in DRF; errors 400/415 at that point).
    pub async fn data(&self) -> Result<Data, ApiError> {
        parse_data(&self.parts, self.body.clone()).await
    }
}

fn respond(r: HResult<Response>) -> Response {
    match r {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// `ObfuscatedPasswordField.to_representation` (upstream 64bf7f845 / #14784, after v3.4.2: constant mask; v3.4.2 sent `"*" * max(10, len(value))`, which leaked the length).
pub fn obfuscate(_password: &str) -> String {
    "*".repeat(10)
}

/// `ProfileSerializer.to_representation(user)`.
async fn profile_repr(state: &AppState, user_id: i64) -> HResult<Value> {
    let db = state.db();
    let Some(u) = pl_authz::user::load_user_by_id(db, user_id).await? else {
        return Err(server_error("profile user vanished"));
    };
    let token = db::token_for_user(db, u.id).await?;
    let socials = social::social_accounts(db, state.config(), u.id).await?.map_err(server_error)?;
    let mfa = pl_authz::user::is_mfa_enabled(db, u.id).await?;
    Ok(json!({
        "email": u.email,
        "password": obfuscate(&u.password),
        "first_name": u.first_name,
        "last_name": u.last_name,
        "auth_token": token,
        "social_accounts": socials,
        "has_usable_password": u.has_usable_password(),
        "is_mfa_enabled": mfa,
    }))
}

pub async fn get_profile(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            let _data = r.data().await?; // get_serializer(data=request.data)
            let body = profile_repr(&state, auth.user().id).await?;
            Ok(Drf::ok(body).into_response())
        }
        .await,
    )
}

pub fn user_attrs(u: &User) -> UserAttrs {
    UserAttrs { username: u.username.clone(), first_name: u.first_name.clone(), last_name: u.last_name.clone(), email: u.email.clone() }
}

/// Validated `ProfileSerializer` data.
#[derive(Default)]
struct ProfileUpdate {
    email: Option<String>,
    password: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
}

fn validate_profile(data: &Data, user: &User) -> Result<ProfileUpdate, ApiError> {
    data.require_mapping()?;
    let mut errs = ValidationErrors::new();
    let mut out = ProfileUpdate::default();
    let opt = FieldOpts { allow_blank: true, ..Default::default() };

    // email = EmailField(allow_blank=True, required=False)
    if let FieldInput::Value(v) = get_value(data, "email", opt) {
        match drf_char(&v, true, None, true) {
            Ok(s) if s.is_empty() || is_valid_email(&s) => out.email = Some(s),
            Ok(_) => errs.add("email", "Enter a valid email address."),
            Err(m) => errs.add("email", m),
        }
    }
    // password = ObfuscatedPasswordField(required=False, allow_null=False) + validate_password
    if let FieldInput::Value(v) = get_value(data, "password", FieldOpts::default()) {
        match &v {
            Value::String(s) if s.is_empty() || s.trim().is_empty() => errs.add("password", "This field may not be blank."),
            Value::String(s) => {
                if has_real_password(s) {
                    let msgs = validate_password(s, Some(&user_attrs(user)));
                    if msgs.is_empty() {
                        out.password = Some(s.clone());
                    } else {
                        errs.set("password", Value::Array(msgs.into_iter().map(Value::String).collect()));
                    }
                } else {
                    out.password = Some(s.clone());
                }
            }
            Value::Null => errs.add("password", "This field may not be null."),
            // to_internal_value returns non-strings unchanged; `.replace` then raises → 500
            _ => return Err(ApiError::internal("password is not a string")),
        }
    }
    for (name, slot) in [("first_name", &mut out.first_name), ("last_name", &mut out.last_name)] {
        if let FieldInput::Value(v) = get_value(data, name, opt) {
            match drf_char(&v, true, Some(150), true) {
                Ok(s) => *slot = Some(s),
                Err(m) => errs.add(name, m),
            }
        }
    }
    errs.into_result()?;
    Ok(out)
}

pub async fn patch_profile(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            let data = r.data().await?;
            let user = auth.user();
            let upd = validate_profile(&data, user)?;
            let mut q = Query::update();
            q.table(db::a("auth_user")).and_where(Expr::col(db::a("id")).eq(user.id));
            let mut any = false;
            let mut password_changed = false;
            if let Some(pw) = upd.password.filter(|p| has_real_password(p)) {
                let encoded = tokio::task::spawn_blocking(move || pl_authz::hashers::make_password(&pw)).await.map_err(server_error)?;
                q.value(db::a("password"), encoded);
                any = true;
                password_changed = true;
            }
            for (col, v) in [("email", upd.email), ("first_name", upd.first_name), ("last_name", upd.last_name)] {
                if let Some(v) = v {
                    q.value(db::a(col), v);
                    any = true;
                }
            }
            if any {
                let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
                sqlx::query_with(&sql, values).execute(state.db()).await?;
            }
            if password_changed {
                // PaperRust: revoke device token (`prt_…`); DRF token stays as upstream
                // (security review 2026-11-19 F13).
                pl_authz::device::revoke_all_for_user(state.db(), user.id).await?;
            }
            let body = profile_repr(&state, user.id).await?;
            Ok(Drf::ok(body).into_response())
        }
        .await,
    )
}

pub async fn generate_auth_token(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (_r, auth) = begin(&state, req).await?;
            let key = db::regenerate_token(state.db(), auth.user().id).await?;
            Ok(Drf::ok(Value::String(key)).into_response())
        }
        .await,
    )
}

/// Django `IntegerField.get_prep_value` (`int(value)`); `Err` = ValueError/TypeError → 500.
fn py_int_pk(v: &Value) -> Result<Option<i64>, String> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(i64::from(*b))),
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64))
            .map(Some)
            .ok_or_else(|| "int() overflow".to_string()),
        Value::String(s) => {
            s.trim().replace('_', "").parse::<i64>().map(Some).map_err(|_| format!("Field 'id' expected a number but got {s:?}."))
        }
        _ => Err("TypeError: int() argument must be a string or a number".into()),
    }
}

pub async fn disconnect_social_account(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            let data = r.data().await?;
            let raw = data.get("id").ok_or_else(|| server_error("KeyError: 'id'"))?;
            let pk = py_int_pk(&raw).map_err(server_error)?;
            let Some(pk) = pk else {
                return Err(django_text(StatusCode::BAD_REQUEST, "Social account not found"));
            };
            let db = state.db();
            let mut tx = db.begin().await?;
            let (sql, values) = Query::select()
                .expr_as(Expr::col(db::a("id")).cast_as(db::a("int8")), db::a("id"))
                .from(db::a("socialaccount_socialaccount"))
                .and_where(Expr::col(db::a("user_id")).eq(auth.user().id))
                .and_where(Expr::col(db::a("id")).eq(pk))
                .build_sqlx(PostgresQueryBuilder);
            let Some(row) = sqlx::query_with(&sql, values).fetch_optional(&mut *tx).await? else {
                return Err(django_text(StatusCode::BAD_REQUEST, "Social account not found"));
            };
            let account_id: i64 = row.try_get("id")?;
            // Collector: SocialToken.account CASCADE, then the account itself
            let (sql, values) = Query::delete()
                .from_table(db::a("socialaccount_socialtoken"))
                .and_where(Expr::col(db::a("account_id")).eq(account_id))
                .build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, values).execute(&mut *tx).await?;
            let (sql, values) = Query::delete()
                .from_table(db::a("socialaccount_socialaccount"))
                .and_where(Expr::col(db::a("id")).eq(account_id))
                .build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, values).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(Drf::ok(json!(account_id)).into_response())
        }
        .await,
    )
}

pub async fn totp_get(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            let u = auth.user();
            // allauth `get_totp_secret(regenerate=True)`: the new secret is stored in the session
            // (`request.session["mfa.totp.secret"]`), so SessionMiddleware saves it and sets the cookie.
            let secret = totp::generate_totp_secret();
            let set_cookie =
                session::set_session_value(&state, &r.parts.headers, totp::SECRET_SESSION_KEY, Value::String(secret.clone())).await?;
            let url = totp::build_totp_url(&totp::totp_label(&u.email, &u.username), &secret);
            let svg = qr::build_svg(&url).map_err(|_| server_error("QR data overflow"))?;
            let mut m = Map::new();
            m.insert("url".into(), Value::String(url));
            m.insert("qr_svg".into(), Value::String(svg));
            m.insert("secret".into(), Value::String(secret));
            let mut resp = Drf::ok(Value::Object(m)).into_response();
            if let Ok(v) = axum::http::HeaderValue::from_str(&set_cookie) {
                resp.headers_mut().append(axum::http::header::SET_COOKIE, v);
            }
            Ok(resp)
        }
        .await,
    )
}

pub async fn totp_post(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            let data = r.data().await?;
            let user_id = auth.user().id;
            let secret = data.get("secret").ok_or_else(|| server_error("KeyError: 'secret'"))?;
            let code = data.get("code").ok_or_else(|| server_error("KeyError: 'code'"))?;
            let Value::String(secret) = secret else {
                return Err(server_error("AttributeError: secret is not a str"));
            };
            let Value::String(code) = code else {
                return Err(server_error("AttributeError: code is not a str"));
            };
            let valid = totp::validate_totp_code(&secret, &code).map_err(|_| server_error("binascii.Error: invalid base32 secret"))?;
            let mut recovery: Value = Value::Null;
            if valid {
                let db = state.db();
                db::insert_authenticator(db, user_id, "totp", json!({ "secret": secret }))
                    .await
                    .map_err(|e| if db::is_unique_violation(&e) { server_error("IntegrityError: TOTP exists") } else { HErr::from(e) })?;
                // auto_generate_recovery_codes
                let has_rc = !db::authenticators(db, user_id, Some("recovery_codes")).await?.is_empty();
                if !has_rc {
                    let seed = totp::generate_recovery_seed();
                    let rc_data = json!({ "seed": seed, "used_mask": 0 });
                    db::insert_authenticator(db, user_id, "recovery_codes", rc_data.clone()).await?;
                    recovery = Value::Array(totp::unused_recovery_codes(&rc_data).into_iter().map(Value::String).collect());
                }
            }
            let mut m = Map::new();
            m.insert("success".into(), Value::Bool(valid));
            m.insert("recovery_codes".into(), recovery);
            Ok(Drf::ok(Value::Object(m)).into_response())
        }
        .await,
    )
}

pub async fn totp_delete(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (_r, auth) = begin(&state, req).await?;
            let user_id = auth.user().id;
            delete_totp_of(&state, user_id).await
        }
        .await,
    )
}

/// Shared by `TOTPView.delete` and `UserViewSet.deactivate_totp`.
pub async fn delete_totp_of(state: &AppState, user_id: i64) -> HResult<Response> {
    let db = state.db();
    match db::authenticators(db, user_id, Some("totp")).await?.into_iter().next() {
        Some(a) => {
            db::delete_and_cleanup(db, a.id, user_id).await?;
            Ok(Drf::ok(Value::Bool(true)).into_response())
        }
        None => Err(django_text(StatusCode::NOT_FOUND, "TOTP not found")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obfuscation() {
        assert_eq!(obfuscate(""), "**********");
        assert_eq!(obfuscate(&"x".repeat(88)), "**********", "constant mask (#14784)");
    }

    #[test]
    fn int_pk() {
        assert_eq!(py_int_pk(&json!("5")), Ok(Some(5)));
        assert_eq!(py_int_pk(&json!(5.7)), Ok(Some(5)));
        assert_eq!(py_int_pk(&json!(null)), Ok(None));
        assert!(py_int_pk(&json!("abc")).is_err());
        assert!(py_int_pk(&json!([1])).is_err());
    }
}
