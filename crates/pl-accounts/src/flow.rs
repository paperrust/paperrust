//! allauth login flow: `Login` state in the session, stages, `perform_login`.
//!
//! upstream: allauth 65.20.1 account/models.py (`Login.serialize`), account/internal/stagekit.py
//! (`LOGIN_SESSION_KEY = "account_login"`, `stash_login`, `unstash_login` with `LOGIN_TIMEOUT`,
//! `clear_login`), account/stages.py (`LoginStageController`, `LoginByCodeStage`,
//! `PhoneVerificationStage`, `EmailVerificationStage`), mfa/stages.py + paperless/adapter.py
//! (`SocialAccountAuthenticateStage`), account/internal/flows/login.py (`record_authentication`,
//! `perform_login`, `resume_login`), account/adapter.py (`pre_login`, `post_login`, `login`),
//! account/utils.py (`get_login_redirect_url`, `user_pk_to_url_str` = base36).

use axum::response::Response;
use pl_authz::User;
use serde_json::{Map, Value, json};

use crate::auth;
use crate::messages;
use crate::page;
use crate::web::{Ctx, R, redirect};

pub const LOGIN_SESSION_KEY: &str = "account_login";
pub const AUTHENTICATION_METHODS_SESSION_KEY: &str = "account_authentication_methods";
const STAGES: [&str; 4] = ["login_by_code", "verify_phone", "verify_email", "mfa_authenticate"];

pub fn now_f64() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `int_to_base36`.
pub fn int_to_base36(mut n: i64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n <= 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// `base36_to_int` (max. 13 characters like Django).
pub fn base36_to_int(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 13 {
        return None;
    }
    i64::from_str_radix(&s.to_ascii_lowercase(), 36).ok()
}

/// Running login (`allauth.account.models.Login`).
#[derive(Debug, Clone)]
pub struct Login {
    pub user_id: Option<i64>,
    pub email_verification: String,
    pub signup: bool,
    pub redirect_url: Option<String>,
    pub email: Option<String>,
    pub signal_kwargs: Value,
    pub state: Map<String, Value>,
    pub initiated_at: f64,
}

impl Login {
    pub fn new(ctx: &Ctx, user_id: i64, redirect_url: Option<String>) -> Self {
        Login {
            user_id: Some(user_id),
            email_verification: ctx.s.account_email_verification.clone(),
            signup: false,
            redirect_url,
            email: None,
            signal_kwargs: Value::Null,
            state: Map::new(),
            initiated_at: now_f64(),
        }
    }

    pub fn serialize(&self) -> Value {
        json!({
            "user_pk": self.user_id.map(int_to_base36),
            "email_verification": self.email_verification,
            "signup": self.signup,
            "redirect_url": self.redirect_url,
            "email": self.email,
            "phone": null,
            "signal_kwargs": self.signal_kwargs,
            "state": Value::Object(self.state.clone()),
            "initiated_at": self.initiated_at,
        })
    }

    pub fn deserialize(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        for k in ["user_pk", "email_verification", "redirect_url", "email", "phone", "signup", "signal_kwargs", "state", "initiated_at"] {
            o.get(k)?;
        }
        Some(Login {
            user_id: o.get("user_pk").and_then(Value::as_str).and_then(base36_to_int),
            email_verification: o.get("email_verification").and_then(Value::as_str).unwrap_or("none").to_string(),
            signup: o.get("signup").and_then(Value::as_bool).unwrap_or(false),
            redirect_url: o.get("redirect_url").and_then(Value::as_str).map(str::to_string),
            email: o.get("email").and_then(Value::as_str).map(str::to_string),
            signal_kwargs: o.get("signal_kwargs").cloned().unwrap_or(Value::Null),
            state: o.get("state").and_then(Value::as_object).cloned().unwrap_or_default(),
            initiated_at: o.get("initiated_at").and_then(Value::as_f64).unwrap_or(0.0),
        })
    }

    fn stages(&mut self) -> &mut Map<String, Value> {
        let st = self.state.entry("stages").or_insert_with(|| Value::Object(Map::new()));
        if !st.is_object() {
            *st = Value::Object(Map::new());
        }
        let Value::Object(m) = st else { unreachable!() };
        // LoginStage.__init__: setdefault(key, {}).setdefault("data", {}) for every stage
        for k in STAGES {
            let e = m.entry(k).or_insert_with(|| json!({}));
            if let Value::Object(o) = e {
                o.entry("data").or_insert_with(|| json!({}));
            }
        }
        m
    }

    fn is_handled(&mut self, key: &str) -> bool {
        self.stages().get(key).and_then(|v| v.get("handled")).and_then(Value::as_bool).unwrap_or(false)
    }

    fn set_handled(&mut self, key: &str) {
        if let Some(Value::Object(o)) = self.stages().get_mut(key) {
            o.insert("handled".into(), Value::Bool(true));
        }
    }

    pub fn current(&self) -> Option<&str> {
        self.state.get("stages").and_then(|s| s.get("current")).and_then(Value::as_str)
    }
}

/// `unstash_login(request, peek=True)` incl. `LOGIN_TIMEOUT`.
pub fn peek_login(ctx: &mut Ctx) -> Option<Login> {
    let v = ctx.session.get(LOGIN_SESSION_KEY)?.clone();
    let login = Login::deserialize(&v)?;
    if now_f64() - login.initiated_at > crate::settings::LOGIN_TIMEOUT {
        clear_login(ctx);
        return None;
    }
    Some(login)
}

pub fn clear_login(ctx: &mut Ctx) {
    ctx.session.pop(LOGIN_SESSION_KEY);
    ctx.session.pop(AUTHENTICATION_METHODS_SESSION_KEY);
}

/// `record_authentication(request, user, method, **extra)`.
pub fn record_authentication(ctx: &mut Ctx, method: &str, extra: &[(&str, Value)]) {
    let mut methods = match ctx.session.get(AUTHENTICATION_METHODS_SESSION_KEY) {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    let mut d = Map::new();
    d.insert("method".into(), Value::from(method));
    d.insert("at".into(), json!(now_f64()));
    for (k, v) in extra {
        if !v.is_null() {
            d.insert(k.to_string(), v.clone());
        }
    }
    methods.push(Value::Object(d));
    ctx.session.set(AUTHENTICATION_METHODS_SESSION_KEY, Value::Array(methods));
}

/// Does the user have TOTP/WebAuthn (`is_mfa_enabled(user, [TOTP, WEBAUTHN])`)?
pub async fn mfa_required(ctx: &Ctx, user_id: i64) -> R<bool> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM mfa_authenticator WHERE user_id = $1 AND type IN ('totp', 'webauthn')")
        .bind(user_id)
        .fetch_one(ctx.st.db())
        .await?;
    Ok(n > 0)
}

/// `get_login_redirect_url(request, url)`.
pub fn login_redirect_url(ctx: &Ctx, url: Option<&str>) -> String {
    url.map(str::to_string).filter(|u| !u.is_empty()).or_else(|| page::next_url(ctx)).unwrap_or_else(|| ctx.s.login_redirect_url.clone())
}

/// `perform_login` → `pre_login` (inactive) → `resume_login`.
pub async fn perform_login(ctx: &mut Ctx, user: User, backend: &'static str, mut login: Login) -> R<Response> {
    if !user.is_active {
        return Ok(redirect(&page::url(ctx, "account_inactive")));
    }
    resume_login(ctx, user, backend, &mut login).await
}

/// `resume_login`: work through stages; rest → `adapter.login` + `post_login`.
pub async fn resume_login(ctx: &mut Ctx, user: User, backend: &'static str, login: &mut Login) -> R<Response> {
    for key in STAGES {
        if login.is_handled(key) {
            continue;
        }
        if let Some(Value::Object(st)) = login.state.get_mut("stages") {
            st.insert("current".into(), Value::from(key));
        }
        if key == "mfa_authenticate" && mfa_stage_applies(ctx, &user, login).await? {
            if let Some(Value::Object(o)) = login.stages().get_mut(key)
                && let Some(Value::Object(d)) = o.get_mut("data")
            {
                d.insert("authentication_required".into(), Value::Bool(true));
            }
            ctx.session.set(LOGIN_SESSION_KEY, login.serialize());
            return Ok(redirect(&page::url(ctx, "mfa_authenticate")));
        }
        // login_by_code / verify_phone: not configured. verify_email (EmailVerificationStage):
        // "optional" + signup → confirmation mail to the first address, flow continues;
        // "mandatory" without a confirmed address → confirmation mail, login remembered (stash_login),
        // redirect to "confirm email" (`respond_email_verification_sent`).
        let needs_mail = key == "verify_email"
            && (login.email_verification == "mandatory" || (login.email_verification == "optional" && login.signup))
            && !crate::emailaddr::has_verified_email(ctx, user.id, login.email.as_deref()).await?;
        if needs_mail && login.email_verification == "optional" && login.signup {
            crate::emailaddr::send_verification_at_login(ctx, user.id, &user.username, login.email.as_deref(), login.signup).await?;
        } else if needs_mail && login.email_verification == "mandatory" {
            let sent =
                crate::emailaddr::send_verification_at_login(ctx, user.id, &user.username, login.email.as_deref(), login.signup).await?;
            if !sent && !crate::emailaddr::has_any_address(ctx, user.id, login.email.as_deref()).await? {
                // as in allauth: without an address only the notice page remains (no error, but no
                // login either) – reason goes to the log
                tracing::warn!(
                    target: "paperless.auth",
                    "User `{}` has no email address; with ACCOUNT_EMAIL_VERIFICATION=mandatory the login waits for a verified address (give the user an email address or use PAPERLESS_ACCOUNT_EMAIL_VERIFICATION=optional)",
                    user.username
                );
            }
            ctx.session.set(LOGIN_SESSION_KEY, login.serialize());
            return Ok(redirect(&page::url(ctx, "account_email_verification_sent")));
        }
        login.set_handled(key);
    }
    ctx.session.pop(LOGIN_SESSION_KEY);
    let redirect_to = login_redirect_url(ctx, login.redirect_url.as_deref());
    let name = user.username.clone();
    auth::login(ctx, user, backend).await?;
    let text = crate::i18n::tf(ctx.lang, "auth.msg.signed_in", &[("NAME", &name)]);
    ctx.messages.add(messages::SUCCESS, text);
    Ok(redirect(&redirect_to))
}

/// `flows.signup.prevent_enumeration(request, email)`: `perform_login(Login(user=None,
/// email=email, signup=True))` – the stages `login_by_code`/`verify_phone` do not apply,
/// `EmailVerificationStage` (only reachable with `mandatory`) sends the "account already
/// exists" mail (`send_verification_email_at_fake_login`) and shows "confirm email".
pub async fn prevent_enumeration(ctx: &mut Ctx, email: &str) -> R<Response> {
    let mut login = Login {
        user_id: None,
        email_verification: ctx.s.account_email_verification.clone(),
        signup: true,
        redirect_url: None,
        email: Some(email.to_string()),
        signal_kwargs: Value::Null,
        state: Map::new(),
        initiated_at: now_f64(),
    };
    for key in ["login_by_code", "verify_phone"] {
        login.set_handled(key);
    }
    if let Some(Value::Object(st)) = login.state.get_mut("stages") {
        st.insert("current".into(), Value::from("verify_email"));
    }
    if login.email_verification != "mandatory" {
        // allauth: adapter.login(request, None) → AssertionError (500)
        return Err(crate::web::Internal("prevent_enumeration without mandatory email verification".into()));
    }
    crate::emailaddr::send_account_already_exists(ctx, email).await?;
    ctx.session.set(LOGIN_SESSION_KEY, login.serialize());
    Ok(redirect(&page::url(ctx, "account_email_verification_sent")))
}

/// `SocialAccountAuthenticateStage.handle` / `AuthenticateStage._should_handle`.
async fn mfa_stage_applies(ctx: &Ctx, user: &User, login: &Login) -> R<bool> {
    if let Some(sl) = login.signal_kwargs.get("sociallogin")
        && !ctx.s.social_account_mfa_trusted_amr.is_empty()
    {
        let amr = sl.pointer("/account/extra_data/id_token/amr");
        if let Some(Value::Array(a)) = amr
            && a.iter().all(Value::is_string)
            && a.iter().any(|v| v.as_str().is_some_and(|s| ctx.s.social_account_mfa_trusted_amr.iter().any(|t| t == s)))
        {
            return Ok(false);
        }
    }
    mfa_required(ctx, user.id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base36() {
        assert_eq!(int_to_base36(5), "5");
        assert_eq!(int_to_base36(36), "10");
        assert_eq!(int_to_base36(1295), "zz");
        assert_eq!(base36_to_int("zz"), Some(1295));
        assert_eq!(base36_to_int("ZZ"), Some(1295));
        assert_eq!(base36_to_int(""), None);
    }

    #[test]
    fn login_roundtrip_matches_allauth_shape() {
        let v = json!({"user_pk":"5","email_verification":"optional","signup":false,"redirect_url":null,"email":null,"phone":null,"signal_kwargs":null,"state":{"stages":{"login_by_code":{"data":{},"handled":true},"verify_phone":{"data":{},"handled":true},"verify_email":{"data":{},"handled":true},"mfa_authenticate":{"data":{"authentication_required":true}},"current":"mfa_authenticate"}},"initiated_at":1794121142.9135323});
        let l = Login::deserialize(&v).unwrap();
        assert_eq!(l.user_id, Some(5));
        assert_eq!(l.current(), Some("mfa_authenticate"));
        assert_eq!(pl_authz::signing::python_json_dumps(&l.serialize()), pl_authz::signing::python_json_dumps(&v));
    }
}
