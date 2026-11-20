//! OAuth token refresh for the connection test (Gmail/Outlook).
//!
//! upstream: src/paperless_mail/oauth.py@v3.4.2 (PaperlessMailOAuth2Manager.refresh_account_oauth_token),
//! httpx_oauth/oauth2.py@0.17.0 (BaseOAuth2.refresh_token, build_request with client_secret_post,
//! send_request → RefreshTokenError on HTTP error/status ≥ 400, get_json).
//! The callback `/api/oauth/callback/` is in [`crate::oauth_callback`].

use chrono::{Duration, Utc};
use serde_json::Value;
use sqlx::PgPool;

use crate::ctx;
use crate::error::{ApiError, ApiResult};

pub const ACCOUNT_TYPE_GMAIL: i16 = 2;
pub const ACCOUNT_TYPE_OUTLOOK: i16 = 3;

/// Stored account, as far as needed for the refresh.
#[derive(Debug, Clone)]
pub struct StoredAccount {
    pub id: i32,
    pub account_type: i16,
    pub refresh_token: Option<String>,
}

/// httpx default (review 2 N27): 5 s timeout, no redirects. Without a timeout,
/// `POST /api/mail_accounts/test/` would hang until the OS TCP timeout.
pub const OAUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn oauth_client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(pl_core::brand::USER_AGENT)
            .timeout(OAUTH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    })
}

/// `refresh_account_oauth_token(account)`: `Ok(Some(new password))` on success,
/// `Ok(None)` on failure (→ MailError), `Err` for exceptions that upstream does not
/// catch (KeyError on missing response fields → 500).
pub async fn refresh_account_oauth_token(db: &PgPool, account: &StoredAccount, cfg: &ctx::MailConfig) -> ApiResult<Option<String>> {
    let Some(refresh_token) = account.refresh_token.as_deref() else {
        tracing::error!(target: "pl_mail", "Account {} has no refresh token to use", account.id);
        return Ok(None);
    };
    let (url, client_id, client_secret) = match account.account_type {
        ACCOUNT_TYPE_GMAIL => (&cfg.gmail_token_url, &cfg.gmail_client_id, &cfg.gmail_client_secret),
        ACCOUNT_TYPE_OUTLOOK => (
            &cfg.outlook_token_url,
            &cfg.outlook_client_id,
            &cfg.outlook_client_secret,
        ),
        _ => {
            tracing::error!(target: "pl_mail", "Account {} does not use OAuth", account.id);
            return Ok(None);
        }
    };
    // httpx encodes None as an empty string
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id.as_deref().unwrap_or("")),
        ("client_secret", client_secret.as_deref().unwrap_or("")),
    ];
    let resp = oauth_client()
        .post(url)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await;
    let resp = match resp {
        Ok(r) if r.status().is_client_error() || r.status().is_server_error() => {
            tracing::error!(target: "pl_mail", "Failed to refresh oauth token for account {}: HTTP {}", account.id, r.status());
            return Ok(None);
        }
        Ok(r) => r,
        Err(e) => {
            tracing::error!(target: "pl_mail", "Failed to refresh oauth token for account {}: {}", account.id, e.without_url());
            return Ok(None);
        }
    };
    let data: Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => {
            tracing::error!(target: "pl_mail", "Failed to refresh oauth token for account {}: Invalid JSON content", account.id);
            return Ok(None);
        }
    };
    let access_token = data
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::internal("oauth refresh: access_token missing"))?
        .to_string();
    let expires_in = data
        .get("expires_in")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .ok_or_else(|| ApiError::internal("oauth refresh: expires_in missing"))?;
    let new_refresh = data.get("refresh_token").and_then(Value::as_str).map(str::to_string);
    let expiration = Utc::now() + Duration::seconds(expires_in);
    // account.save(): password, expiry and possibly new refresh token
    sqlx::query(
        "UPDATE paperless_mail_mailaccount SET password = $2, expiration = $3, \
         refresh_token = COALESCE($4, refresh_token) WHERE id = $1",
    )
    .bind(account.id)
    .bind(&access_token)
    .bind(expiration)
    .bind(new_refresh)
    .execute(db)
    .await?;
    tracing::debug!(target: "pl_mail", "Successfully refreshed oauth token for account {}", account.id);
    Ok(Some(access_token))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 2 N27: a token endpoint that never answers leads, after the httpx timeout, to
    /// `Ok(None)` (→ "Unable to refresh oauth token") instead of a hanging request.
    #[tokio::test]
    async fn refresh_times_out_like_httpx() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s); // never answer
            }
        });
        let core = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([(
            "PAPERLESS_SECRET_KEY",
            "pl-mail-tests-only-0123456789abcdefghijklmnopqrstuvwxyz",
        )]))
        .unwrap();
        let mut cfg = ctx::MailConfig::from_core(&core);
        cfg.gmail_token_url = format!("http://{addr}/token");
        let db = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://u:p@127.0.0.1:1/x").unwrap();
        let acc = StoredAccount { id: 1, account_type: ACCOUNT_TYPE_GMAIL, refresh_token: Some("r".into()) };
        let start = std::time::Instant::now();
        let r = tokio::time::timeout(OAUTH_TIMEOUT * 3, refresh_account_oauth_token(&db, &acc, &cfg)).await;
        assert!(matches!(r, Ok(Ok(None))), "must give up after the client timeout");
        assert!(start.elapsed() >= OAUTH_TIMEOUT);
    }

    #[test]
    fn mail_config_from_core() {
        let core = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([
            ("PAPERLESS_SECRET_KEY", "pl-mail-tests-only-0123456789abcdefghijklmnopqrstuvwxyz"),
            ("PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS", "false"),
            ("PAPERLESS_GMAIL_OAUTH_CLIENT_ID", "cid"),
            ("PAPERLESS_PROXY_SSL_HEADER", r#"["HTTP_X_FORWARDED_PROTO", "https"]"#),
        ]))
        .unwrap();
        let c = ctx::MailConfig::from_core(&core);
        assert!(!c.allow_internal_hosts);
        assert_eq!(c.gmail_client_id.as_deref(), Some("cid"));
        assert_eq!(c.proxy_ssl_header, Some(("x-forwarded-proto".into(), "https".into())));
    }
}
