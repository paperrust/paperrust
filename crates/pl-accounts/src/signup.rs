//! Registration (`/accounts/signup/`), when allowed.
//!
//! upstream: allauth 65.20.1 account/views.py (`SignupView`: `rate_limit("signup")`,
//! `RedirectAuthenticatedUserMixin`, `CloseableSignupMixin`), account/forms.py (`BaseSignupForm`,
//! `SignupForm.clean`: password check with dummy user, "You must type the same password each
//! time."), account/adapter.py (`clean_username`: `UnicodeUsernameValidator`, uniqueness
//! case-insensitive), account/internal/flows/signup.py (`complete_signup`), src/paperless/adapter.py
//! @v3.4.2 (`is_open_for_signup`, `save_user`: first install → superuser, `ACCOUNT_DEFAULT_GROUPS`).
//!
//! Registration with email: `clean_email` (`email_already_exists` → "A user is already registered
//! with this email address."), `setup_user_email`, confirmation mail via the
//! `EmailVerificationStage` (`optional`/`mandatory`). Closed registration →
//! `signup_closed.html` (`CloseableSignupMixin.respond_closed`). `mandatory` with an address already taken
//! → `try_save` → `prevent_enumeration` (mail "account already exists", notice page).

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use pl_core::AppState;
use serde_json::Value;

use crate::auth;
use crate::flow::{self, Login};
use crate::page;
use crate::views::{limit_or_429, not_allowed, redirect_authenticated, run};
use crate::web::{Ctx, R};
use crate::tr;

/// paperless `CustomAccountAdapter.is_open_for_signup`.
pub async fn is_open(ctx: &Ctx) -> R<bool> {
    Ok(page::first_install(ctx).await? || ctx.s.account_allow_signups)
}

pub async fn signup(State(st): State<AppState>, req: Request<Body>) -> Response {
    run(st, req, |ctx| Box::pin(signup_view(ctx))).await
}

async fn signup_view(ctx: &mut Ctx) -> R<Response> {
    if let Some(r) = limit_or_429(ctx, "signup").await? {
        return Ok(r);
    }
    ctx.never_cache = true;
    crate::social::warn_unsupported_providers(&ctx.s);
    if let Some(r) = redirect_authenticated(ctx) {
        return Ok(r);
    }
    if !is_open(ctx).await? {
        // CloseableSignupMixin.respond_closed
        return crate::socialaccount::signup_closed(ctx).await;
    }
    if !matches!(ctx.method.as_str(), "GET" | "HEAD" | "POST") {
        return Ok(not_allowed("GET, POST, PUT, HEAD, OPTIONS"));
    }
    let mut errors: Vec<(String, Vec<String>)> = Vec::new();
    if ctx.is_post() {
        let l = ctx.lang;
        let required = tr!(l, "auth.error.required").to_string();
        let username = ctx.post("username").unwrap_or("").trim().to_string();
        let p1 = ctx.post("password1").unwrap_or("").to_string();
        let p2 = ctx.post("password2").unwrap_or("").to_string();
        let mut add = |f: &str, e: String| match errors.iter_mut().find(|(k, _)| k == f) {
            Some((_, v)) => v.push(e),
            None => errors.push((f.to_string(), vec![e])),
        };
        let mut username_ok = false;
        if username.is_empty() {
            add("username", required.clone());
        } else if username.chars().count() > 150 {
            add("username", format!("Ensure this value has at most 150 characters (it has {}).", username.chars().count()));
        } else if !pl_auth_api::http::is_valid_username(&username) {
            add("username", tr!(l, "auth.error.username_invalid").to_string());
        } else {
            let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM auth_user WHERE UPPER(username::text) = UPPER($1)").bind(&username).fetch_one(ctx.st.db()).await?;
            if n > 0 {
                add("username", tr!(l, "auth.error.username_taken").to_string());
            } else {
                username_ok = true;
            }
        }
        // clean_email (EmailField, not required): lowercase, unique (assess_unique_email)
        let raw_email = ctx.post("email").unwrap_or("").trim().to_string();
        let mut email = String::new();
        // `account_already_exists` (assess_unique_email → None, only with `mandatory`)
        let mut already_exists = false;
        if !raw_email.is_empty() {
            if !pl_auth_api::http::is_valid_email(&raw_email) || raw_email.chars().count() > 320 {
                add("email", tr!(l, "auth.error.email_invalid").to_string());
            } else {
                let e = raw_email.to_lowercase();
                match crate::emailaddr::assess_unique_email(ctx, &e).await? {
                    Some(false) => add("email", tr!(l, "auth.error.email_taken").to_string()),
                    Some(true) => email = e,
                    None => {
                        already_exists = true;
                        email = e;
                    }
                }
            }
        }
        if p1.is_empty() {
            add("password1", required.clone());
        }
        if p2.is_empty() {
            add("password2", required);
        }
        if !p1.is_empty() {
            let attrs = pl_auth_api::password::UserAttrs {
                username: if username_ok { username.clone() } else { String::new() },
                first_name: String::new(),
                last_name: String::new(),
                email: email.clone(),
            };
            for e in pl_auth_api::password::validate_password(&p1, Some(&attrs)) {
                add("password1", e);
            }
            if !p2.is_empty() && p1 != p2 {
                add("password2", tr!(l, "auth.error.password_mismatch").to_string());
            }
        }
        if errors.is_empty() && already_exists {
            // BaseSignupForm.try_save → flows.signup.prevent_enumeration
            return flow::prevent_enumeration(ctx, &email).await;
        }
        if errors.is_empty() {
            let pw = p1.clone();
            let enc = tokio::task::spawn_blocking(move || pl_authz::hashers::make_password(&pw)).await?;
            // save_user: @transaction.atomic + row lock (upstream fix 22746aa20, after v3.4.2)
            let mut tx = ctx.st.db().begin().await?;
            let first = page::first_install_locked(&mut tx).await?;
            let (id,): (i64,) = sqlx::query_as(
                "INSERT INTO auth_user (password, last_login, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined) \
                 VALUES ($1, NULL, $2, $3, '', '', $4, $2, true, now()) RETURNING id::int8",
            )
            .bind(&enc)
            .bind(first)
            .bind(&username)
            .bind(&email)
            .fetch_one(&mut *tx)
            .await?;
            if !ctx.s.account_default_groups.is_empty() {
                sqlx::query(
                    "INSERT INTO auth_user_groups (user_id, group_id) SELECT $1, g.id FROM auth_group g WHERE g.name = ANY($2) ON CONFLICT DO NOTHING",
                )
                .bind(id)
                .bind(&ctx.s.account_default_groups)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            // BaseSignupForm.save → setup_user_email(request, user, [EmailAddress(email=email)])
            let extra: Vec<crate::emailaddr::EmailAddr> = if email.is_empty() {
                Vec::new()
            } else {
                vec![crate::emailaddr::EmailAddr { email: email.clone(), verified: Value::Bool(false), primary: false }]
            };
            crate::emailaddr::setup_user_email(ctx, id, &email, &extra).await?;
            let Some(user) = pl_authz::user::load_user_by_id(ctx.st.db(), id).await? else {
                return Err(crate::web::Internal("created user vanished".into()));
            };
            let mut login = Login::new(ctx, id, page::next_url(ctx));
            login.signup = true;
            login.signal_kwargs = Value::Object(Default::default());
            return flow::perform_login(ctx, user, auth::ALLAUTH_BACKEND, login).await;
        }
    }
    let flat: Vec<Vec<String>> = errors.into_iter().map(|(_, v)| v).collect();
    let mut m = page::base(ctx, &flat).await?;
    let next = ctx.param("next").map(str::to_string);
    m.insert("redirect_field_name".into(), Value::from("next"));
    m.insert("redirect_field_value".into(), next.map(Value::from).unwrap_or(Value::Null));
    page::render("signup.html", m, StatusCode::OK)
}
