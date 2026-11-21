//! Social/OIDC login (allauth 65.20.1 `socialaccount`, configured as paperless-ngx v3.4.2 does).
//!
//! upstream: allauth socialaccount/providers/oauth2/views.py (`OAuth2LoginView`,
//! `OAuth2CallbackView.dispatch`/`_get_state`), socialaccount/providers/base/{provider,views,
//! utils}.py (`redirect_from_request`, `stash_redirect_state`, `sociallogin_from_response`,
//! `cleanup_email_addresses`, `respond_to_login_on_get`), socialaccount/providers/openid_connect/
//! provider.py (`extract_uid`, `extract_common_fields`, `extract_email_addresses`, `_pick_data`),
//! socialaccount/internal/statekit.py (`socialaccount_states`: `{id: [state, ts, provider_id]}`,
//! TTL 600 s, max. 10), socialaccount/models.py (`SocialLogin.serialize`/`deserialize`, `lookup`,
//! `_lookup_by_socialaccount`, `_lookup_by_email`, `_accept_login`, `save`, `connect`),
//! socialaccount/internal/flows/{login,signup,connect,email_authentication}.py,
//! socialaccount/helpers.py (`render_authentication_error`), socialaccount/views.py
//! (`SignupView`, `LoginCancelledView`, `LoginErrorView`), socialaccount/forms.py (`SignupForm`),
//! socialaccount/adapter.py (`populate_user`, `save_user`, `is_email_verified`,
//! `can_authenticate_by_email`, `authenticate_by_email`, `generate_state_param`),
//! core/internal/modelkit.py (`serialize_instance`: DjangoJSONEncoder), account/adapter.py
//! (`clean_username`, `populate_username`, `save_user`), allauth/utils.py
//! (`generate_unique_username`), account/forms.py (`BaseSignupForm`), src/paperless/adapter.py
//! @v3.4.2 (`CustomSocialAccountAdapter`: `SOCIALACCOUNT_ALLOW_SIGNUPS`, connect target `base`,
//! `SOCIAL_ACCOUNT_DEFAULT_GROUPS`, error log), src/paperless/signals.py@v3.4.2
//! (`handle_social_account_updated`: group/role sync).
//!
//! paperless does not set `SOCIALACCOUNT_STORE_TOKENS` → `socialaccount_socialtoken` stays empty.
//!
//! Besides `openid_connect`, GitHub, GitLab, Google, Microsoft, Nextcloud, Discord and Gitea run through the same
//! flow (`/accounts/<id>/login/`, `…/callback/`; endpoints and field mapping in `oauth2.rs`).
//! `ACCOUNT_EMAIL_VERIFICATION=mandatory` is handled like allauth (confirmation stage in the
//! login flow, enumeration protection via mail "account already exists").

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use pl_authz::User;
use pl_core::AppState;
use rand::Rng;
use serde_json::{Map, Value, json};

use crate::auth;
use crate::emailaddr::{self, EmailAddr};
use crate::flow::{self, Login};
use crate::oauth2;
use crate::oidc::{self, OidcError};
use crate::page;
use crate::social::{self, App, OIDC};
use crate::views::{not_allowed, run};
use crate::web::{Ctx, Internal, R, not_found, redirect};
use crate::{messages, tr};

pub const STATES_SESSION_KEY: &str = "socialaccount_states";
pub const PENDING_SESSION_KEY: &str = "socialaccount_sociallogin";
const STATE_TTL: f64 = 600.0;
const MAX_STATES: usize = 10;
const UID_MAX_LENGTH: usize = 191;

// ---------------------------------------------------------------------------------------------
// Helpers

/// `DjangoJSONEncoder` for `datetime` (UTC): milliseconds, `Z`.
pub fn django_iso(t: DateTime<Utc>) -> String {
    let base = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let us = t.timestamp_subsec_micros();
    if us == 0 { format!("{base}Z") } else { format!("{base}.{:03}Z", us / 1000) }
}

fn parse_iso(v: Option<&Value>) -> Option<DateTime<Utc>> {
    let s = v?.as_str()?;
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

/// Python `str(value)` for JSON values.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

fn opt_str(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(v) => Some(py_str(v)),
    }
}

/// `get_random_string(n)`.
fn random_string(n: usize) -> String {
    pl_authz::hashers::get_random_string(n)
}

/// `user_field(user, field, value)`: truncate to `max_length`.
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// `reverse(...)` quotes path parameters (`safe = RFC3986_SUBDELIMS + "/~:@"`).
fn reverse_quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~!$&'()*+,;=/:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `adapter.get_callback_url(request, app)`: OIDC `reverse("openid_connect_callback",
/// provider_id)`, otherwise `reverse("<id>_callback")`, absolute (`build_absolute_uri`).
fn callback_url(ctx: &Ctx, app: &App) -> String {
    let path = if app.provider == OIDC {
        format!("{}accounts/oidc/{}/login/callback/", ctx.s.base_url, reverse_quote(&app.provider_id))
    } else {
        format!("{}accounts/{}/login/callback/", ctx.s.base_url, app.provider)
    };
    emailaddr::absolute_uri(ctx, &path)
}

fn oidc_err(e: OidcError) -> Internal {
    Internal(e.to_string())
}

// ---------------------------------------------------------------------------------------------
// statekit

fn get_states(ctx: &Ctx) -> Map<String, Value> {
    match ctx.session.get(STATES_SESSION_KEY) {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    }
}

fn state_ts(v: &Value) -> f64 {
    v.get(1).and_then(Value::as_f64).unwrap_or(0.0)
}

/// `gc_states(states)`.
fn gc_states(states: &mut Map<String, Value>, now: f64) {
    states.retain(|_, v| now - state_ts(v) <= STATE_TTL);
    if states.len() > MAX_STATES {
        let mut oldest: Option<(String, f64)> = None;
        for (k, v) in states.iter() {
            let ts = state_ts(v);
            if oldest.as_ref().is_none_or(|(_, o)| *o > ts) {
                oldest = Some((k.clone(), ts));
            }
        }
        if let Some((k, _)) = oldest {
            states.shift_remove(&k);
        }
    }
}

/// `stash_state(request, provider_id=…, state=…)` → state id.
pub fn stash_state(ctx: &mut Ctx, provider_id: &str, state: Map<String, Value>) -> String {
    let now = flow::now_f64();
    let mut states = get_states(ctx);
    gc_states(&mut states, now);
    let id = random_string(16);
    states.insert(id.clone(), json!([Value::Object(state), now, provider_id]));
    ctx.session.set(STATES_SESSION_KEY, Value::Object(states));
    id
}

fn matching_state<'a>(states: &'a Map<String, Value>, state_id: &str, provider_id: &str) -> Option<&'a Value> {
    let s = states.get(state_id)?;
    let a = s.as_array()?;
    (a.len() == 3 && a[2].as_str() == Some(provider_id)).then_some(s)
}

/// `unstash_state(request, state_id=…, provider_id=…)`.
pub fn unstash_state(ctx: &mut Ctx, state_id: &str, provider_id: &str) -> Option<Map<String, Value>> {
    let mut states = get_states(ctx);
    let stashed = matching_state(&states, state_id, provider_id)?.clone();
    let mut state = stashed.get(0).and_then(Value::as_object).cloned();
    if flow::now_f64() - state_ts(&stashed) > STATE_TTL {
        state = None;
    }
    states.shift_remove(state_id);
    ctx.session.set(STATES_SESSION_KEY, Value::Object(states));
    state
}

// ---------------------------------------------------------------------------------------------
// SocialLogin

/// `SocialLogin` (account, user, state, addresses, token) in allauth serialization form.
#[derive(Debug, Clone)]
pub struct SocialLogin {
    /// `provider.serialize()["id"]` (class, e.g. `openid_connect`, `github`).
    pub provider_id: String,
    /// `provider.serialize()["app.client_id"]`.
    pub client_id: String,
    /// `serialize_instance(SocialAccount)`.
    pub account: Map<String, Value>,
    /// `serialize_instance(User)`.
    pub user: Map<String, Value>,
    pub state: Map<String, Value>,
    pub email_addresses: Vec<EmailAddr>,
    pub phone: Value,
    pub phone_verified: Value,
    pub token: Option<Map<String, Value>>,
    /// `_did_authenticate_by_email` (not serialized).
    pub did_auth_by_email: Option<String>,
}

impl SocialLogin {
    pub fn serialize(&self) -> Value {
        let mut m = Map::new();
        m.insert("provider".into(), json!({"id": self.provider_id, "app.client_id": self.client_id}));
        m.insert("account".into(), Value::Object(self.account.clone()));
        m.insert("user".into(), Value::Object(self.user.clone()));
        m.insert("state".into(), Value::Object(self.state.clone()));
        m.insert("email_addresses".into(), Value::Array(self.email_addresses.iter().map(EmailAddr::serialize).collect()));
        m.insert("phone".into(), self.phone.clone());
        m.insert("phone_verified".into(), self.phone_verified.clone());
        if let Some(t) = &self.token {
            m.insert("token".into(), Value::Object(t.clone()));
        }
        Value::Object(m)
    }

    /// `SocialLogin.deserialize` (supported provider classes; otherwise `None` = ValueError).
    pub fn deserialize(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let provider = o.get("provider")?.as_object()?;
        let provider_id = provider.get("id")?.as_str()?.to_string();
        if !social::PROVIDER_CLASSES.iter().any(|c| c.id == provider_id) {
            return None;
        }
        let client_id = match provider.get("app.client_id") {
            Some(Value::String(s)) => s.clone(),
            None | Some(Value::Null) => String::new(),
            Some(_) => return None,
        };
        Some(SocialLogin {
            provider_id,
            client_id,
            account: o.get("account")?.as_object()?.clone(),
            user: o.get("user")?.as_object()?.clone(),
            state: o.get("state").and_then(Value::as_object).cloned().unwrap_or_default(),
            email_addresses: o
                .get("email_addresses")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(EmailAddr::deserialize).collect())
                .unwrap_or_default(),
            phone: o.get("phone").cloned().unwrap_or(Value::Null),
            phone_verified: o.get("phone_verified").cloned().unwrap_or(Value::Bool(false)),
            token: o.get("token").and_then(Value::as_object).cloned(),
            did_auth_by_email: None,
        })
    }

    fn user_str(&self, k: &str) -> String {
        self.user.get(k).and_then(Value::as_str).unwrap_or("").to_string()
    }

    pub fn user_id(&self) -> Option<i64> {
        self.user.get("id").and_then(Value::as_i64)
    }

    pub fn provider_sub_id(&self) -> String {
        self.account.get("provider").and_then(Value::as_str).unwrap_or("").to_string()
    }

    pub fn uid(&self) -> String {
        self.account.get("uid").map(py_str).unwrap_or_default()
    }

    pub fn extra_data(&self) -> Value {
        self.account.get("extra_data").cloned().unwrap_or(Value::Null)
    }

    /// `get_redirect_url` = `state["next"]`.
    pub fn redirect_url(&self) -> Option<String> {
        self.state.get("next").and_then(Value::as_str).map(str::to_string)
    }
}

/// `serialize_instance(User)` of a stored user.
fn user_map(u: &User) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(u.id));
    m.insert("password".into(), Value::from(u.password.clone()));
    m.insert("last_login".into(), u.last_login.map(|t| Value::from(django_iso(t))).unwrap_or(Value::Null));
    m.insert("is_superuser".into(), Value::Bool(u.is_superuser));
    m.insert("username".into(), Value::from(u.username.clone()));
    m.insert("first_name".into(), Value::from(u.first_name.clone()));
    m.insert("last_name".into(), Value::from(u.last_name.clone()));
    m.insert("email".into(), Value::from(u.email.clone()));
    m.insert("is_staff".into(), Value::Bool(u.is_staff));
    m.insert("is_active".into(), Value::Bool(u.is_active));
    m.insert("date_joined".into(), Value::from(django_iso(u.date_joined)));
    m
}

/// `set_unusable_password()`.
fn unusable_password() -> String {
    format!("!{}", random_string(40))
}

/// `adapter.new_user` + `set_unusable_password` (fresh `User()`).
fn new_user_map() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), Value::Null);
    m.insert("password".into(), Value::from(unusable_password()));
    m.insert("last_login".into(), Value::Null);
    m.insert("is_superuser".into(), Value::Bool(false));
    m.insert("username".into(), Value::from(""));
    m.insert("first_name".into(), Value::from(""));
    m.insert("last_name".into(), Value::from(""));
    m.insert("email".into(), Value::from(""));
    m.insert("is_staff".into(), Value::Bool(false));
    m.insert("is_active".into(), Value::Bool(true));
    m.insert("date_joined".into(), Value::from(django_iso(Utc::now())));
    m
}

/// `_pick_data`: userinfo before ID token.
fn pick_data(data: &Map<String, Value>) -> Map<String, Value> {
    if let Some(u) = data.get("userinfo").filter(|v| social::py_truthy(v)) {
        return u.as_object().cloned().unwrap_or_default();
    }
    if let Some(t) = data.get("id_token").filter(|v| social::py_truthy(v)) {
        return t.as_object().cloned().unwrap_or_default();
    }
    data.clone()
}

/// `DefaultSocialAccountAdapter.is_email_verified(provider, email)`.
fn provider_email_verified(ctx: &Ctx, app: &App, email: &str) -> R<bool> {
    let v = match app.settings.get("verified_email") {
        Some(Value::Null) | None => {
            social::provider_settings(ctx, &app.provider).get("VERIFIED_EMAIL").cloned().unwrap_or(Value::Bool(false))
        }
        Some(v) => v.clone(),
    };
    match v {
        Value::Bool(b) => Ok(b),
        Value::Array(domains) => {
            let d = email.rsplit_once('@').map(|(_, d)| d).unwrap_or("").to_lowercase();
            Ok(domains.iter().filter_map(Value::as_str).any(|x| x.to_lowercase() == d))
        }
        _ => Err(Internal("verified_email wrongly configured".into())),
    }
}

/// `OpenIDConnectProvider.extract_*` (userinfo before ID token, `uid_field`).
fn oidc_extract(app: &App, data: Map<String, Value>) -> R<oauth2::Extracted> {
    let picked = pick_data(&data);
    let uid_field = app.settings.get("uid_field").and_then(Value::as_str).unwrap_or("sub").to_string();
    let Some(uid) = picked.get(&uid_field) else { return Err(Internal(format!("KeyError: '{uid_field}'"))) };
    let mut addresses: Vec<EmailAddr> = Vec::new();
    if let Some(e) = picked.get("email").filter(|v| social::py_truthy(v)) {
        // only a real `true`/"true" counts as confirmed (security review 2026-11-19; upstream:
        // Python truthiness, "false" would also be confirmed)
        let verified = picked.get("email_verified").is_some_and(crate::emailaddr::is_true_claim);
        addresses.push(EmailAddr { email: py_str(e), verified: Value::Bool(verified), primary: true });
    }
    Ok(oauth2::Extracted {
        uid: py_str(uid),
        email: opt_str(picked.get("email")),
        email_verified: false,
        username: opt_str(picked.get("preferred_username")),
        name: opt_str(picked.get("name")),
        last_name: opt_str(picked.get("family_name")),
        first_name: opt_str(picked.get("given_name")),
        addresses,
        extra_data: data,
    })
}

/// `Provider.sociallogin_from_response(request, data)` (without DB access) for OIDC and the
/// OAuth2 providers from `oauth2.rs`.
fn sociallogin_from_response(ctx: &Ctx, app: &App, data: Map<String, Value>) -> R<SocialLogin> {
    let ex = if app.provider == OIDC { oidc_extract(app, data)? } else { oauth2::extract(&app.provider, &data).map_err(Internal)? };
    let uid = ex.uid;
    if uid.chars().count() > UID_MAX_LENGTH {
        return Err(Internal(format!("SOCIALACCOUNT_UID_MAX_LENGTH too small (<{})", uid.chars().count())));
    }
    if uid.is_empty() {
        return Err(Internal("uid must be a non-empty string".into()));
    }
    let mut addresses = ex.addresses;
    // cleanup_email_addresses (provider)
    let mut email = emailaddr::valid_email_or_none(ex.email.as_deref());
    addresses.retain_mut(|a| match emailaddr::valid_email_or_none(Some(&a.email)) {
        Some(e) => {
            a.email = e;
            true
        }
        None => false,
    });
    if let Some(e) = &email
        && !addresses.iter().any(|a| &a.email == e)
    {
        addresses.insert(0, EmailAddr { email: e.clone(), verified: Value::Bool(ex.email_verified), primary: true });
    }
    for a in addresses.iter_mut() {
        if provider_email_verified(ctx, app, &a.email)? {
            a.verified = Value::Bool(true);
        }
    }
    // sort(key=(primary, verified, email), reverse=True) – stable
    addresses.sort_by_key(|a| std::cmp::Reverse((a.primary, a.is_verified(), a.email.clone())));
    if email.is_none() && !addresses.is_empty() {
        email = addresses.first().map(|a| a.email.clone());
    }
    // new_user + populate_user
    let mut user = new_user_map();
    user.insert("username".into(), Value::from(truncate(ex.username.as_deref().unwrap_or(""), 150)));
    user.insert("email".into(), Value::from(truncate(&emailaddr::valid_email_or_none(email.as_deref()).unwrap_or_default(), 254)));
    let name = ex.name.unwrap_or_default();
    let (n0, n2) = match name.split_once(' ') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (name.clone(), String::new()),
    };
    let first = ex.first_name.filter(|s| !s.is_empty()).unwrap_or(n0);
    let last = ex.last_name.filter(|s| !s.is_empty()).unwrap_or(n2);
    user.insert("first_name".into(), Value::from(truncate(&first, 150)));
    user.insert("last_name".into(), Value::from(truncate(&last, 150)));
    let mut account = Map::new();
    account.insert("id".into(), Value::Null);
    account.insert("user_id".into(), Value::Null);
    account.insert("provider".into(), Value::from(app.sub_id()));
    account.insert("uid".into(), Value::from(uid));
    account.insert("last_login".into(), Value::Null);
    account.insert("date_joined".into(), Value::Null);
    account.insert("extra_data".into(), Value::Object(ex.extra_data));
    Ok(SocialLogin {
        provider_id: app.provider.clone(),
        client_id: app.client_id.clone(),
        account,
        user,
        state: Map::new(),
        email_addresses: addresses,
        phone: Value::Null,
        phone_verified: Value::Bool(false),
        token: None,
        did_auth_by_email: None,
    })
}

// ---------------------------------------------------------------------------------------------
// paperless handle_social_account_updated

/// `handle_social_account_updated(sender, request, sociallogin)`.
pub async fn sync_social_groups(ctx: &Ctx, user_id: i64, extra_data: &Value) -> R<()> {
    let Some(u) = pl_authz::user::load_user_by_id(ctx.st.db(), user_id).await? else { return Ok(()) };
    if !u.is_active {
        tracing::debug!(target: "paperless.auth", "Skipping social account sync for inactive user `{}`", u.username);
        return Ok(());
    }
    let claim = &ctx.s.social_account_sync_groups_claim;
    let mut groups = extra_data.get(claim).cloned().unwrap_or(Value::Array(Vec::new()));
    if !social::py_truthy(&groups) {
        let from = |k: &str| extra_data.get(k).and_then(|v| v.get(claim)).cloned().filter(social::py_truthy);
        groups = from("userinfo").or_else(|| from("id_token")).unwrap_or(Value::Array(Vec::new()));
    }
    let names: Vec<String> = match &groups {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter(|v| !v.is_null()).map(py_str).collect(),
        Value::Object(o) => o.keys().cloned().collect(),
        _ => Vec::new(),
    };
    if ctx.s.social_account_sync_groups && !groups.is_null() {
        tracing::debug!(target: "paperless.auth", "Syncing groups for user `{}`: {:?}", u.username, names);
        let mut tx = ctx.st.db().begin().await?;
        sqlx::query("DELETE FROM auth_user_groups WHERE user_id = $1").bind(user_id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO auth_user_groups (user_id, group_id) SELECT $1, g.id FROM auth_group g WHERE g.name = ANY($2) ORDER BY g.id ON CONFLICT DO NOTHING")
            .bind(user_id)
            .bind(&names)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    let mut is_superuser = u.is_superuser;
    let mut is_staff = u.is_staff;
    if let Some(g) = &ctx.s.social_account_sync_superuser_group {
        is_superuser = names.iter().any(|n| n == g);
    }
    if let Some(g) = &ctx.s.social_account_sync_staff_group {
        is_staff = names.iter().any(|n| n == g) || is_superuser;
    } else if ctx.s.social_account_sync_superuser_group.is_some() {
        is_staff = is_superuser || is_staff;
    }
    if is_superuser != u.is_superuser || is_staff != u.is_staff {
        sqlx::query("UPDATE auth_user SET is_superuser = $1, is_staff = $2 WHERE id = $3")
            .bind(is_superuser)
            .bind(is_staff)
            .bind(user_id)
            .execute(ctx.st.db())
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Pages

/// `render_authentication_error` (paperless template, 401) or abort → `login_cancelled`.
async fn auth_error(ctx: &mut Ctx, app: &App, error: &str, exception: Option<&str>) -> R<Response> {
    tracing::warn!(target: "paperless.auth", "Social authentication error for provider `{}`: {error} ({})", social::display_name(app), exception.unwrap_or("None"));
    if error == "cancelled" {
        return Ok(redirect(&page::url(ctx, "socialaccount_login_cancelled")));
    }
    let m = page::base(ctx, &[]).await?;
    page::render("social_authentication_error.html", m, StatusCode::UNAUTHORIZED)
}

/// Pages that upstream shows in the allauth default layout (signup closed, login
/// cancelled, confirm email): here in the own layout, without CSRF field (unless `extra`
/// brings one, like the confirmation form).
pub async fn allauth_page(ctx: &mut Ctx, tpl: &str, extra: Map<String, Value>, status: StatusCode) -> R<Response> {
    let mut m = page::base_with(ctx, &[], false).await?;
    m.insert("authenticated".into(), Value::Bool(ctx.user.is_some()));
    for (k, v) in extra {
        m.insert(k, v);
    }
    page::render(tpl, m, status)
}

/// `account/signup_closed.html` (`CloseableSignupMixin.respond_closed`, `SignupClosedException`).
pub async fn signup_closed(ctx: &mut Ctx) -> R<Response> {
    allauth_page(ctx, "signup_closed.html", Map::new(), StatusCode::OK).await
}

/// `/accounts/oidc/<provider_id>/login/` (GET: intermediate page, otherwise redirect to the IdP).
pub async fn oidc_login(State(st): State<AppState>, Path(provider_id): Path<String>, req: Request<Body>) -> Response {
    run(st, req, move |ctx| Box::pin(oidc_login_view(ctx, provider_id))).await
}

async fn oidc_login_view(ctx: &mut Ctx, provider_id: String) -> R<Response> {
    social::warn_unsupported_providers(&ctx.s);
    let Some(app) = social::get_oidc_provider(ctx, &provider_id, None).await? else {
        return Ok(not_found(&ctx.path));
    };
    login_view(ctx, app).await
}

/// `/accounts/<provider>/login/` of the OAuth2 providers (`OAuth2LoginView.adapter_view`). Not
/// installed/unknown providers: no URL pattern → 404; installed without app:
/// `SocialApp.DoesNotExist` (500 as in allauth).
pub async fn provider_login(State(st): State<AppState>, Path(provider): Path<String>, req: Request<Body>) -> Response {
    run(st, req, move |ctx| Box::pin(provider_login_view(ctx, provider))).await
}

async fn oauth2_app(ctx: &mut Ctx, provider: &str) -> R<Option<App>> {
    social::warn_unsupported_providers(&ctx.s);
    if provider == OIDC || social::class_installed(&ctx.s, provider).is_none() {
        return Ok(None);
    }
    match social::get_provider(ctx, provider, None).await? {
        Some(app) => Ok(Some(app)),
        None => Err(Internal("SocialApp.DoesNotExist".into())),
    }
}

async fn provider_login_view(ctx: &mut Ctx, provider: String) -> R<Response> {
    let Some(app) = oauth2_app(ctx, &provider).await? else {
        return Ok(not_found(&ctx.path));
    };
    login_view(ctx, app).await
}

/// `BaseLoginView.dispatch`: `respond_to_login_on_get`, otherwise `provider.redirect_from_request`.
async fn login_view(ctx: &mut Ctx, app: App) -> R<Response> {
    if ctx.method == axum::http::Method::GET {
        // respond_to_login_on_get (SOCIALACCOUNT_LOGIN_ON_GET = False)
        let mut m = page::base(ctx, &[]).await?;
        m.insert("provider".into(), json!({"name": social::display_name(&app)}));
        m.insert("process".into(), ctx.get("process").map(Value::from).unwrap_or(Value::Null));
        return page::render("social_login.html", m, StatusCode::OK);
    }
    // redirect_from_request
    let next = page::next_url(ctx);
    let process = match ctx.post("process").filter(|p| !p.is_empty()) {
        Some(p) => p.to_string(),
        None => ctx.get("process").unwrap_or("login").to_string(),
    };
    let scope = oidc::scope(ctx, &app);
    let mut auth_params = oidc::auth_params(ctx, &app);
    let callback = callback_url(ctx, &app);
    let authorize = if app.provider == OIDC {
        let cfg = oidc::openid_config(&app).await.map_err(oidc_err)?;
        oidc::cfg_str(&cfg, "token_endpoint").map_err(oidc_err)?;
        oidc::cfg_str(&cfg, "authorization_endpoint").map_err(oidc_err)?
    } else {
        oauth2::endpoints(ctx, &app).map_err(oidc_err)?.0
    };
    let mut verifier = Value::Null;
    if oidc::pkce_enabled(ctx, &app) {
        let (v, c) = oidc::code_challenge();
        verifier = Value::from(v);
        oidc::dict_update(&mut auth_params, "code_challenge_method", "S256".into());
        oidc::dict_update(&mut auth_params, "code_challenge", c);
    }
    let mut state = Map::new();
    state.insert("process".into(), Value::from(process));
    state.insert("data".into(), Value::Null);
    state.insert("pkce_code_verifier".into(), verifier);
    if let Some(n) = next {
        state.insert("next".into(), Value::from(n));
    }
    let state_id = stash_state(ctx, app.sub_id(), state);
    let url = oidc::redirect_url(&authorize, &app.client_id, &callback, &scope, &state_id, &auth_params);
    Ok(redirect(&url))
}

/// `/accounts/oidc/<provider_id>/login/callback/`.
pub async fn oidc_callback(State(st): State<AppState>, Path(provider_id): Path<String>, req: Request<Body>) -> Response {
    run(st, req, move |ctx| Box::pin(oidc_callback_view(ctx, provider_id))).await
}

async fn oidc_callback_view(ctx: &mut Ctx, provider_id: String) -> R<Response> {
    social::warn_unsupported_providers(&ctx.s);
    let Some(app) = social::get_oidc_provider(ctx, &provider_id, None).await? else {
        return Ok(not_found(&ctx.path));
    };
    callback_view(ctx, app).await
}

/// `/accounts/<provider>/login/callback/` of the OAuth2 providers (`OAuth2CallbackView`).
pub async fn provider_callback(State(st): State<AppState>, Path(provider): Path<String>, req: Request<Body>) -> Response {
    run(st, req, move |ctx| Box::pin(provider_callback_view(ctx, provider))).await
}

async fn provider_callback_view(ctx: &mut Ctx, provider: String) -> R<Response> {
    let Some(app) = oauth2_app(ctx, &provider).await? else {
        return Ok(not_found(&ctx.path));
    };
    callback_view(ctx, app).await
}

/// `OAuth2CallbackView.dispatch` (+ `_get_state`).
async fn callback_view(ctx: &mut Ctx, app: App) -> R<Response> {
    let state_id = ctx.param("state").map(str::to_string);
    // Headless flows (state["headless"]) are not reimplemented (allauth-headless is missing; PaperRust
    // never creates such a state – only a session from a former Django installation can hold one):
    // the login error page like an unknown state (docs/contract/pl-accounts.md, Known differences).
    if let Some(id) = state_id.as_deref().filter(|s| !s.is_empty()) {
        let states = get_states(ctx);
        if let Some(s) = matching_state(&states, id, app.sub_id())
            && s.get(0).and_then(|st| st.get("headless")).is_some_and(social::py_truthy)
        {
            return auth_error(ctx, &app, "unknown", None).await;
        }
    }
    let state = match state_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => unstash_state(ctx, id, app.sub_id()),
        None => None,
    };
    let Some(state) = state else {
        return auth_error(ctx, &app, "unknown", None).await;
    };
    if ctx.get("error").is_some() || ctx.get("code").is_none() {
        let error = if ctx.get("error") == Some("access_denied") { "cancelled" } else { "unknown" };
        return auth_error(ctx, &app, error, None).await;
    }
    match fetch_login(ctx, &app, &state).await {
        Ok(mut sl) => {
            sl.state = state;
            complete_login(ctx, &app, sl).await
        }
        Err(Fetch::Auth(e)) => auth_error(ctx, &app, "unknown", Some(&e)).await,
        Err(Fetch::Internal(e)) => Err(e),
    }
}

enum Fetch {
    Auth(String),
    Internal(Internal),
}

impl From<OidcError> for Fetch {
    fn from(e: OidcError) -> Self {
        match e {
            OidcError::OAuth2(s) | OidcError::Request(s) => Fetch::Auth(s),
            OidcError::Internal(s) => Fetch::Internal(Internal(s)),
        }
    }
}

impl From<Internal> for Fetch {
    fn from(e: Internal) -> Self {
        Fetch::Internal(e)
    }
}

/// Fetch token, profile (OIDC: userinfo + ID token; otherwise `oauth2::fetch_profile`), build `SocialLogin`.
async fn fetch_login(ctx: &mut Ctx, app: &App, state: &Map<String, Value>) -> Result<SocialLogin, Fetch> {
    let (cfg, token_url, basic) = if app.provider == OIDC {
        let cfg = oidc::openid_config(app).await?;
        let token_url = oidc::cfg_str(&cfg, "token_endpoint")?;
        let basic = oidc::basic_auth(app, &cfg);
        (Some(cfg), token_url, basic)
    } else {
        // OAuth2Adapter.basic_auth = False for all reimplemented providers
        (None, oauth2::endpoints(ctx, app)?.1, false)
    };
    let callback = callback_url(ctx, app);
    let code = ctx.param("code").unwrap_or("").to_string();
    let verifier = state.get("pkce_code_verifier").and_then(Value::as_str);
    let tok = oidc::access_token(app, &token_url, &callback, &code, basic, verifier).await?;
    // parse_token
    let mut token = Map::new();
    token.insert("id".into(), Value::Null);
    token.insert("app_id".into(), app.pk.map(Value::from).unwrap_or(Value::Null));
    token.insert("account_id".into(), Value::Null);
    let access = tok.get("access_token").cloned().unwrap_or(Value::Null);
    token.insert("token".into(), access.clone());
    token.insert("token_secret".into(), tok.get("refresh_token").cloned().unwrap_or(Value::from("")));
    let expires = match tok.get("expires_in").filter(|v| social::py_truthy(v)) {
        Some(v) => {
            let secs = match v {
                Value::Number(n) => n.as_f64().map(|f| f.trunc() as i64),
                Value::String(s) => s.trim().parse::<i64>().ok(),
                Value::Bool(b) => Some(i64::from(*b)),
                _ => None,
            };
            let Some(secs) = secs else { return Err(Fetch::Internal(Internal("ValueError: expires_in".into()))) };
            Value::from(django_iso(Utc::now() + chrono::Duration::seconds(secs)))
        }
        None => Value::Null,
    };
    token.insert("expires_at".into(), expires);
    let Some(cfg) = cfg else {
        let data = oauth2::fetch_profile(ctx, app, &py_str(&access), &tok).await?;
        let mut sl = sociallogin_from_response(ctx, app, data)?;
        sl.token = Some(token);
        return Ok(sl);
    };
    // complete_login
    let id_token = tok.get("id_token").filter(|v| social::py_truthy(v)).cloned();
    let fetch_userinfo = app.settings.get("fetch_userinfo").map(social::py_truthy).unwrap_or(true);
    let mut data = Map::new();
    if fetch_userinfo || id_token.is_none() {
        let url = oidc::cfg_str(&cfg, "userinfo_endpoint")?;
        let info = oidc::userinfo(&url, &py_str(&access)).await?;
        data.insert("userinfo".into(), info);
    }
    if let Some(t) = id_token {
        oidc::cfg_str(&cfg, "jwks_uri")?;
        let issuer = oidc::cfg_str(&cfg, "issuer")?;
        let now = flow::now_f64();
        let claims = oidc::decode_id_token(&py_str(&t), &issuer, &app.client_id, now)?;
        if let Some((key, timeout)) = oidc::jti_key(&claims, now) {
            let t = if timeout > 0.0 { timeout as u64 } else { 0 };
            let added = if t == 0 { false } else { ctx.cache.add(&key, crate::cache::PICKLED_TRUE, t).await.map_err(|e| Internal(e.0))? };
            if !added {
                return Err(Fetch::Auth("Invalid id_token".into()));
            }
        }
        data.insert("id_token".into(), Value::Object(claims));
    }
    let mut sl = sociallogin_from_response(ctx, app, data)?;
    sl.token = Some(token);
    Ok(sl)
}

/// `flows.login.complete_login(request, sociallogin)`.
async fn complete_login(ctx: &mut Ctx, app: &App, mut sl: SocialLogin) -> R<Response> {
    // pre_social_login
    ctx.session.pop(PENDING_SESSION_KEY);
    lookup(ctx, app, &mut sl).await?;
    let process = sl.state.get("process").and_then(Value::as_str).unwrap_or("").to_string();
    match process.as_str() {
        "redirect" => {
            let next = sl.redirect_url().filter(|n| !n.is_empty()).unwrap_or_else(|| "/".into());
            Ok(redirect(&next))
        }
        "connect" => connect(ctx, sl).await,
        _ => authenticate(ctx, app, sl).await,
    }
}

/// `SocialLogin.lookup()`: by account, otherwise by confirmed email (if allowed).
async fn lookup(ctx: &mut Ctx, app: &App, sl: &mut SocialLogin) -> R<()> {
    let row: Option<(i64, i64, DateTime<Utc>)> =
        sqlx::query_as("SELECT id::int8, user_id::int8, date_joined FROM socialaccount_socialaccount WHERE provider = $1 AND uid = $2")
            .bind(sl.provider_sub_id())
            .bind(sl.uid())
            .fetch_optional(ctx.st.db())
            .await?;
    if let Some((id, user_id, date_joined)) = row {
        let now = Utc::now();
        let extra = sl.extra_data();
        sqlx::query("UPDATE socialaccount_socialaccount SET extra_data = $1, last_login = $2 WHERE id = $3")
            .bind(&extra)
            .bind(now)
            .bind(id)
            .execute(ctx.st.db())
            .await?;
        let mut a = Map::new();
        a.insert("id".into(), json!(id));
        a.insert("user_id".into(), json!(user_id));
        a.insert("provider".into(), Value::from(sl.provider_sub_id()));
        a.insert("uid".into(), Value::from(sl.uid()));
        a.insert("last_login".into(), Value::from(django_iso(now)));
        a.insert("date_joined".into(), Value::from(django_iso(date_joined)));
        a.insert("extra_data".into(), extra.clone());
        sl.account = a;
        if let Some(u) = pl_authz::user::load_user_by_id(ctx.st.db(), user_id).await? {
            sl.user = user_map(&u);
        }
        // signals.social_account_updated → paperless handle_social_account_updated
        sync_social_groups(ctx, user_id, &extra).await?;
        return Ok(());
    }
    // _lookup_by_email (adapter.authenticate_by_email)
    let verified: Vec<String> = sl.email_addresses.iter().filter(|e| e.is_verified()).map(|e| e.email.clone()).collect();
    for email in verified {
        let allowed = match app.settings.get("email_authentication") {
            Some(Value::Null) | None => {
                social::provider_settings(ctx, &app.provider).get("EMAIL_AUTHENTICATION").is_some_and(social::py_truthy)
            }
            Some(v) => social::py_truthy(v),
        };
        if !allowed {
            continue;
        }
        if let Some(uid) = first_user_for_login_email(ctx, &email).await?
            && let Some(u) = pl_authz::user::load_user_by_id(ctx.st.db(), uid).await?
        {
            sl.user = user_map(&u);
            sl.did_auth_by_email = Some(email);
            return Ok(());
        }
    }
    Ok(())
}

/// `filter_users_by_email(email, prefer_verified=True, for_login=True)[0]`.
async fn first_user_for_login_email(ctx: &Ctx, email: &str) -> R<Option<i64>> {
    let email = email.to_lowercase();
    let rows: Vec<(i64, String, bool)> =
        sqlx::query_as("SELECT user_id::int8, email, verified FROM account_emailaddress WHERE email = $1 ORDER BY id")
            .bind(&email)
            .fetch_all(ctx.st.db())
            .await?;
    let mails: Vec<(i64, bool)> = rows.into_iter().filter(|(_, e, _)| e.to_lowercase() == email).map(|(u, _, v)| (u, v)).collect();
    if let Some((u, _)) = mails.iter().find(|(_, v)| *v) {
        return Ok(Some(*u));
    }
    if let Some((u, _)) = mails.first() {
        return Ok(Some(*u));
    }
    let row: Option<(i64,)> = sqlx::query_as("SELECT id::int8 FROM auth_user WHERE email = $1 AND lower(email) = $1 ORDER BY id LIMIT 1")
        .bind(&email)
        .fetch_optional(ctx.st.db())
        .await?;
    Ok(row.map(|r| r.0))
}

/// `connect(request, sociallogin)` (`process=connect`).
async fn connect(ctx: &mut Ctx, sl: SocialLogin) -> R<Response> {
    let default_next = page::url(ctx, "base");
    let Some(me) = ctx.user.clone() else {
        // PermissionDenied → get_connect_redirect_url
        return Ok(redirect(&default_next));
    };
    let existing_user = sl.user_id();
    let account_saved = sl.account.get("id").is_some_and(|v| !v.is_null());
    let (ok, text) = match existing_user {
        Some(uid) if uid != me.id => (false, tr!(ctx.lang, "auth.msg.external_taken")),
        Some(_) if account_saved => (true, tr!(ctx.lang, "auth.msg.external_linked")),
        _ => {
            // sociallogin.connect(request, request.user) → save(connect=True)
            insert_account(ctx, me.id, &sl).await?;
            (true, tr!(ctx.lang, "auth.msg.external_linked"))
        }
    };
    let text = text.to_string();
    ctx.messages.add(if ok { messages::INFO } else { messages::ERROR }, text);
    let next = sl.redirect_url().filter(|n| !n.is_empty()).unwrap_or(default_next);
    Ok(redirect(&next))
}

async fn insert_account(ctx: &Ctx, user_id: i64, sl: &SocialLogin) -> R<i64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO socialaccount_socialaccount (provider, uid, last_login, date_joined, extra_data, user_id) VALUES ($1, $2, now(), now(), $3, $4) RETURNING id::int8",
    )
    .bind(sl.provider_sub_id())
    .bind(sl.uid())
    .bind(sl.extra_data())
    .bind(user_id)
    .fetch_one(ctx.st.db())
    .await?;
    Ok(id)
}

/// `_authenticate(request, sociallogin)`.
async fn authenticate(ctx: &mut Ctx, app: &App, sl: SocialLogin) -> R<Response> {
    if ctx.user.is_some() {
        auth::logout(ctx);
    }
    let existing = match sl.user_id() {
        Some(id) => pl_authz::user::load_user_by_id(ctx.st.db(), id).await?,
        None => None,
    };
    let Some(user) = existing else { return process_signup(ctx, app, sl).await };
    // _accept_login
    if let Some(email) = sl.did_auth_by_email.clone() {
        let (verified,): (bool,) =
            sqlx::query_as("SELECT COALESCE((SELECT verified FROM account_emailaddress WHERE user_id = $1 AND email = $2 LIMIT 1), false)")
                .bind(user.id)
                .bind(email.to_lowercase())
                .fetch_one(ctx.st.db())
                .await?;
        if !verified && user.has_usable_password() {
            sqlx::query("UPDATE auth_user SET password = $1 WHERE id = $2")
                .bind(unusable_password())
                .bind(user.id)
                .execute(ctx.st.db())
                .await?;
        }
    }
    let user = pl_authz::user::load_user_by_id(ctx.st.db(), user.id).await?.unwrap_or(user);
    record_social_authentication(ctx, &sl);
    let mut login = Login::new(ctx, user.id, sl.redirect_url());
    login.signal_kwargs = json!({"sociallogin": sl.serialize()});
    flow::perform_login(ctx, user, auth::ALLAUTH_BACKEND, login).await
}

fn record_social_authentication(ctx: &mut Ctx, sl: &SocialLogin) {
    flow::record_authentication(ctx, "socialaccount", &[("provider", Value::from(sl.provider_sub_id())), ("uid", Value::from(sl.uid()))]);
}

/// `process_signup(request, sociallogin)`.
async fn process_signup(ctx: &mut Ctx, app: &App, mut sl: SocialLogin) -> R<Response> {
    if !ctx.s.socialaccount_allow_signups {
        return signup_closed(ctx).await;
    }
    let mut auto = ctx.s.socialaccount_auto_signup;
    if auto && let Some(email) = sl.email_addresses.first().map(|a| a.email.clone()) {
        match emailaddr::assess_unique_email(ctx, &email).await? {
            Some(true) => {}
            Some(false) => auto = false,
            // enumeration protection (only `mandatory`): mail "account already exists", notice page
            None => return flow::prevent_enumeration(ctx, &email).await,
        }
    }
    if !auto {
        ctx.session.set(PENDING_SESSION_KEY, sl.serialize());
        return Ok(redirect(&page::url(ctx, "socialaccount_signup")));
    }
    let username = sl.user_str("username");
    if clean_username(ctx, &username, false).await?.is_err() {
        sl.user.insert("username".into(), Value::from(""));
    }
    let uid = save_social_user(ctx, app, &mut sl, None).await?;
    complete_social_signup(ctx, sl, uid).await
}

/// `adapter.clean_username(username, shallow)`: `Ok(Ok(()))`, `Ok(Err(text))`.
async fn clean_username(ctx: &Ctx, username: &str, shallow: bool) -> R<Result<(), String>> {
    let l = ctx.lang;
    if !pl_auth_api::http::is_valid_username(username) {
        return Ok(Err(tr!(l, "auth.error.username_invalid").to_string()));
    }
    if username.chars().count() > 150 {
        return Ok(Err(format!("Ensure this value has at most 150 characters (it has {}).", username.chars().count())));
    }
    if !shallow {
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM auth_user WHERE UPPER(username::text) = UPPER($1)")
            .bind(username)
            .fetch_one(ctx.st.db())
            .await?;
        if n > 0 {
            return Ok(Err(tr!(l, "auth.error.username_taken").to_string()));
        }
    }
    Ok(Ok(()))
}

/// `generate_unique_username([first_name, last_name, email, username, "user"])`.
async fn generate_unique_username(ctx: &Ctx, txts: &[String]) -> R<String> {
    let mut base: Option<String> = None;
    for t in txts {
        if t.is_empty() {
            continue;
        }
        let ascii: String = deunicode_nfkd(t);
        if ascii.is_empty() {
            continue;
        }
        let cleaned: String = ascii
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || c.is_whitespace() || "@+.-".contains(*c))
            .collect::<String>()
            .to_lowercase();
        let cleaned = cleaned.split('@').next().unwrap_or("").trim().to_string();
        let mut out = String::new();
        let mut ws = false;
        for c in cleaned.chars() {
            if c.is_whitespace() {
                if !ws {
                    out.push('_');
                }
                ws = true;
            } else {
                out.push(c);
                ws = false;
            }
        }
        base = Some(out.clone());
        if clean_username(ctx, &out, true).await?.is_ok() {
            break;
        }
    }
    let base = base.filter(|b| !b.is_empty()).unwrap_or_else(|| "user".into());
    let mut candidates = vec![base.clone()];
    for len in 1..7usize {
        let mut rng = rand::thread_rng();
        let suffix: String = (0..len)
            .map(|i| {
                if i < 4 {
                    char::from(b'0' + rng.gen_range(0..10u8))
                } else {
                    let set = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
                    char::from(set[rng.gen_range(0..set.len())])
                }
            })
            .collect();
        let keep = 150usize.saturating_sub(suffix.len());
        candidates.push(format!("{}{suffix}", base.chars().take(keep).collect::<String>()));
    }
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT username FROM auth_user WHERE UPPER(username::text) = ANY(SELECT UPPER(x) FROM unnest($1::text[]) x)")
            .bind(&candidates)
            .fetch_all(ctx.st.db())
            .await?;
    let existing: Vec<String> = rows.into_iter().map(|r| r.0.to_lowercase()).collect();
    for c in candidates {
        if !existing.contains(&c.to_lowercase()) && clean_username(ctx, &c, true).await?.is_ok() {
            return Ok(c);
        }
    }
    Err(Internal("Unable to find a unique username".into()))
}

/// `unicodedata.normalize("NFKD", s).encode("ascii", "ignore")` for common characters.
fn deunicode_nfkd(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        let base = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' => "A",
            'ç' | 'ć' | 'č' => "c",
            'Ç' | 'Ć' | 'Č' => "C",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ě' => "e",
            'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ě' => "E",
            'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
            'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' => "I",
            'ñ' | 'ń' | 'ň' => "n",
            'Ñ' | 'Ń' | 'Ň' => "N",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' => "o",
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ō' => "O",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' => "u",
            'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů' => "U",
            'ý' | 'ÿ' => "y",
            'Ý' => "Y",
            'š' | 'ś' => "s",
            'Š' | 'Ś' => "S",
            'ž' | 'ź' | 'ż' => "z",
            'Ž' | 'Ź' | 'Ż' => "Z",
            'ř' => "r",
            'Ř' => "R",
            'ď' => "d",
            'Ď' => "D",
            'ť' => "t",
            'Ť' => "T",
            _ => "",
        };
        out.push_str(base);
    }
    out
}

/// Form data of the social signup.
struct SignupData {
    username: String,
    email: String,
}

/// `CustomSocialAccountAdapter.save_user(request, sociallogin, form)` + `SocialLogin.save` +
/// `SOCIAL_ACCOUNT_DEFAULT_GROUPS` + `handle_social_account_updated`. → new user id.
async fn save_social_user(ctx: &mut Ctx, _app: &App, sl: &mut SocialLogin, form: Option<&SignupData>) -> R<i64> {
    sl.user.insert("password".into(), Value::from(unusable_password()));
    if let Some(f) = form {
        sl.user.insert("email".into(), Value::from(truncate(&f.email, 254)));
        sl.user.insert("username".into(), Value::from(truncate(&f.username, 150)));
        sl.user.insert("password".into(), Value::from(unusable_password()));
    }
    // populate_username
    if sl.user_str("username").is_empty() {
        let txts = [sl.user_str("first_name"), sl.user_str("last_name"), sl.user_str("email"), sl.user_str("username"), "user".to_string()];
        let u = generate_unique_username(ctx, &txts).await?;
        sl.user.insert("username".into(), Value::from(u));
    }
    let date_joined = parse_iso(sl.user.get("date_joined")).unwrap_or_else(Utc::now);
    // paperless CustomAccountAdapter.save_user (only with form): first install → superuser,
    // @transaction.atomic + row lock (upstream fix 22746aa20, after v3.4.2)
    let mut tx = ctx.st.db().begin().await?;
    let is_superuser = form.is_some() && page::first_install_locked(&mut tx).await?;
    let (uid,): (i64,) = sqlx::query_as(
        "INSERT INTO auth_user (password, last_login, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined) \
         VALUES ($1, NULL, $2, $3, $4, $5, $6, $2, true, $7) RETURNING id::int8",
    )
    .bind(sl.user_str("password"))
    .bind(is_superuser)
    .bind(sl.user_str("username"))
    .bind(sl.user_str("first_name"))
    .bind(sl.user_str("last_name"))
    .bind(sl.user_str("email"))
    .bind(date_joined)
    .fetch_one(&mut *tx)
    .await?;
    if form.is_some() && !ctx.s.account_default_groups.is_empty() {
        sqlx::query("INSERT INTO auth_user_groups (user_id, group_id) SELECT $1, g.id FROM auth_group g WHERE g.name = ANY($2) ON CONFLICT DO NOTHING")
            .bind(uid)
            .bind(&ctx.s.account_default_groups)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    sl.user.insert("id".into(), json!(uid));
    let account_id = insert_account(ctx, uid, sl).await?;
    sl.account.insert("id".into(), json!(account_id));
    sl.account.insert("user_id".into(), json!(uid));
    let email = sl.user_str("email");
    let addrs = sl.email_addresses.clone();
    emailaddr::setup_user_email(ctx, uid, &email, &addrs).await?;
    if !ctx.s.social_account_default_groups.is_empty() {
        add_groups(ctx, uid, &ctx.s.social_account_default_groups).await?;
    }
    sync_social_groups(ctx, uid, &sl.extra_data()).await?;
    Ok(uid)
}

async fn add_groups(ctx: &Ctx, uid: i64, names: &[String]) -> R<()> {
    sqlx::query(
        "INSERT INTO auth_user_groups (user_id, group_id) SELECT $1, g.id FROM auth_group g WHERE g.name = ANY($2) ON CONFLICT DO NOTHING",
    )
    .bind(uid)
    .bind(names)
    .execute(ctx.st.db())
    .await?;
    Ok(())
}

/// `complete_social_signup` → `complete_signup` → `perform_login(Login(signup=True))`.
async fn complete_social_signup(ctx: &mut Ctx, sl: SocialLogin, uid: i64) -> R<Response> {
    record_social_authentication(ctx, &sl);
    let Some(user) = pl_authz::user::load_user_by_id(ctx.st.db(), uid).await? else {
        return Err(Internal("created user vanished".into()));
    };
    let mut login = Login::new(ctx, uid, sl.redirect_url());
    login.signup = true;
    login.signal_kwargs = json!({"sociallogin": sl.serialize()});
    flow::perform_login(ctx, user, auth::ALLAUTH_BACKEND, login).await
}

/// `/accounts/3rdparty/signup/` (`socialaccount.views.SignupView`).
pub async fn signup(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| Box::pin(signup_view(ctx))).await
}

async fn signup_view(ctx: &mut Ctx) -> R<Response> {
    social::warn_unsupported_providers(&ctx.s);
    // get_pending_signup: missing or unreadable (ValueError) → login page
    let pending = ctx.session.get(PENDING_SESSION_KEY).cloned().filter(social::py_truthy);
    let Some(mut sl) = pending.as_ref().and_then(SocialLogin::deserialize) else { return Ok(redirect(&page::url(ctx, "account_login"))) };
    let provider_id = sl.provider_id.clone();
    let Some(app) = social::get_provider(ctx, &provider_id, Some(&sl.client_id)).await? else {
        return Err(Internal("SocialApp.DoesNotExist".into()));
    };
    // RedirectAuthenticatedUserMixin (never_cache)
    ctx.never_cache = true;
    if ctx.user.is_some() {
        // get_authenticated_redirect_url → reverse("socialaccount_connections"): does not exist in paperless
        return Err(Internal("NoReverseMatch: socialaccount_connections".into()));
    }
    if let Some(login) = flow::peek_login(ctx)
        && login.current() == Some("mfa_authenticate")
        && login.state.get("stages").and_then(|s| s.get("mfa_authenticate")).and_then(|m| m.get("handled")).is_none()
    {
        return Ok(redirect(&page::url(ctx, "mfa_authenticate")));
    }
    // CloseableSignupMixin
    if !ctx.s.socialaccount_allow_signups {
        return signup_closed(ctx).await;
    }
    if !matches!(ctx.method.as_str(), "GET" | "HEAD" | "POST") {
        return Ok(not_allowed("GET, POST, PUT, HEAD, OPTIONS"));
    }
    let initial_email = {
        let e = sl.user_str("email");
        if e.is_empty() { sl.email_addresses.first().map(|a| a.email.clone()).unwrap_or_default() } else { e }
    };
    let mut values = (sl.user_str("username"), initial_email);
    let mut errors: Vec<(String, Vec<String>)> = Vec::new();
    if ctx.is_post() {
        let raw_user = ctx.post("username").unwrap_or("").to_string();
        let raw_email = ctx.post("email").unwrap_or("").to_string();
        values = (raw_user.clone(), raw_email.clone());
        let l = ctx.lang;
        let mut add = |f: &str, e: String| match errors.iter_mut().find(|(k, _)| k == f) {
            Some((_, v)) => v.push(e),
            None => errors.push((f.to_string(), vec![e])),
        };
        let username = raw_user.trim().to_string();
        if username.is_empty() {
            add("username", tr!(l, "auth.error.required").to_string());
        } else if username.chars().count() > 150 {
            add("username", format!("Ensure this value has at most 150 characters (it has {}).", username.chars().count()));
        } else if let Err(e) = clean_username(ctx, &username, false).await? {
            add("username", e);
        }
        let email = raw_email.trim().to_string();
        let mut clean_email = String::new();
        let mut already_exists = false;
        if !email.is_empty() {
            if !pl_auth_api::http::is_valid_email(&email) || email.chars().count() > 320 {
                add("email", tr!(l, "auth.error.email_invalid").to_string());
            } else {
                clean_email = email.to_lowercase();
                match emailaddr::assess_unique_email(ctx, &clean_email).await? {
                    Some(true) => {}
                    Some(false) => {
                        add("email", crate::i18n::tf(l, "auth.error.external_email_taken", &[("PROVIDER", &social::display_name(&app))]))
                    }
                    None => already_exists = true,
                }
            }
        }
        if errors.is_empty() && already_exists {
            // signup_by_form: clear_pending_signup, form.try_save → prevent_enumeration
            ctx.session.pop(PENDING_SESSION_KEY);
            return flow::prevent_enumeration(ctx, &clean_email).await;
        }
        if errors.is_empty() {
            // signup_by_form
            ctx.session.pop(PENDING_SESSION_KEY);
            let form = SignupData { username, email: clean_email };
            let uid = save_social_user(ctx, &app, &mut sl, Some(&form)).await?;
            return complete_social_signup(ctx, sl, uid).await;
        }
    }
    let flat: Vec<Vec<String>> = errors.into_iter().map(|(_, v)| v).collect();
    let mut m = page::base(ctx, &flat).await?;
    m.insert("account".into(), json!({"get_provider": {"name": social::display_name(&app)}}));
    m.insert("form".into(), json!({"username": {"name": "username", "value": values.0}, "email": {"name": "email", "value": values.1}}));
    m.insert("redirect_field_value".into(), Value::Null);
    page::render("social_signup.html", m, StatusCode::OK)
}

/// `/accounts/3rdparty/login/cancelled/`.
pub async fn login_cancelled(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| {
        Box::pin(async move {
            if !matches!(ctx.method.as_str(), "GET" | "HEAD") {
                return Ok(not_allowed("GET, HEAD, OPTIONS"));
            }
            allauth_page(ctx, "social_login_cancelled.html", Map::new(), StatusCode::OK).await
        })
    })
    .await
}

/// `/accounts/3rdparty/login/error/` (401).
pub async fn login_error(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| {
        Box::pin(async move {
            if !matches!(ctx.method.as_str(), "GET" | "HEAD") {
                return Ok(not_allowed("GET, HEAD, OPTIONS"));
            }
            let m = page::base(ctx, &[]).await?;
            page::render("social_authentication_error.html", m, StatusCode::UNAUTHORIZED)
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_like_django() {
        let t = DateTime::parse_from_rfc3339("2026-11-08T10:31:44.127456Z").unwrap().with_timezone(&Utc);
        assert_eq!(django_iso(t), "2026-11-08T10:31:44.127Z");
        let t = DateTime::parse_from_rfc3339("2026-11-08T10:31:44Z").unwrap().with_timezone(&Utc);
        assert_eq!(django_iso(t), "2026-11-08T10:31:44Z");
    }

    /// Security review 2026-11-19: OIDC `email_verified` as text "false" yields an
    /// unconfirmed address (upstream: Python truthiness → confirmed).
    #[test]
    fn oidc_email_verified_string_false_is_unverified() {
        let app = App {
            pk: None,
            provider: OIDC.into(),
            provider_id: "idp".into(),
            name: "IdP".into(),
            client_id: "c".into(),
            secret: String::new(),
            key: String::new(),
            settings: Map::new(),
        };
        let ex = |v: Value| {
            let data = json!({"userinfo": {"sub": "u1", "email": "u@example.invalid", "email_verified": v}});
            oidc_extract(&app, data.as_object().cloned().unwrap_or_default()).map(|e| e.addresses)
        };
        for (v, want) in [
            (json!("false"), false),
            (json!("False"), false),
            (json!(false), false),
            (json!(null), false),
            (json!("true"), true),
            (json!(true), true),
        ] {
            let addrs = ex(v.clone()).unwrap_or_default();
            assert_eq!(addrs.len(), 1, "{v}");
            assert_eq!(addrs[0].is_verified(), want, "{v}");
            assert_eq!(addrs[0].verified, Value::Bool(want), "stored as bool: {v}");
        }
    }

    #[test]
    fn states_gc() {
        let mut m = Map::new();
        for i in 0..11 {
            m.insert(format!("s{i}"), json!([{}, 1000.0 + f64::from(i), "p"]));
        }
        m.insert("old".into(), json!([{}, 1.0, "p"]));
        gc_states(&mut m, 1500.0);
        assert!(!m.contains_key("old"));
        assert_eq!(m.len(), 10);
        assert!(!m.contains_key("s0"));
    }

    #[test]
    fn sociallogin_roundtrip_matches_django_shape() {
        let v = json!({"provider":{"id":"openid_connect","app.client_id":"w12client"},"account":{"id":null,"user_id":null,"provider":"fakeidp","uid":"sub-newbie","last_login":null,"date_joined":null,"extra_data":{"userinfo":{"sub":"sub-newbie"}}},"user":{"id":null,"password":"!x","last_login":null,"is_superuser":false,"username":"newbie","first_name":"Newbie","last_name":"Tester","email":"newbie@example.invalid","is_staff":false,"is_active":true,"date_joined":"2026-11-08T10:31:44.127Z"},"state":{"process":"","data":null,"pkce_code_verifier":"v","next":"/documents/"},"email_addresses":[{"id":null,"user_id":null,"email":"newbie@example.invalid","verified":false,"primary":true}],"phone":null,"phone_verified":false,"token":{"id":null,"app_id":null,"account_id":null,"token":"t","token_secret":"","expires_at":"2026-11-08T10:36:43.992Z"}});
        let sl = SocialLogin::deserialize(&v).unwrap();
        assert_eq!(pl_authz::signing::python_json_dumps(&sl.serialize()), pl_authz::signing::python_json_dumps(&v));
        assert_eq!(sl.redirect_url().as_deref(), Some("/documents/"));
        assert_eq!(sl.uid(), "sub-newbie");
    }

    #[test]
    fn username_base_ascii() {
        assert_eq!(deunicode_nfkd("Jürgen Müller"), "Jurgen Muller");
        assert_eq!(reverse_quote("a b/é"), "a%20b/%C3%A9");
    }
}
