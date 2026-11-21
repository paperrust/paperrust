//! Writing a value into the Django session of the current request
//! (`request.session[key] = value` + `SessionMiddleware.process_response`).
//!
//! upstream: django/contrib/sessions/backends/{base,db,cached_db}.py@5.2 (`load`, `save`,
//! `create`, `_get_new_session_key`, `get_expiry_age`, `get_expire_at_browser_close`),
//! django/contrib/sessions/middleware.py@5.2 (`process_response`: save + `Set-Cookie`),
//! paperless settings `SESSION_ENGINE = "django.contrib.sessions.backends.cached_db"`.
//!
//! Same encoding as pl-accounts `session.rs` (`signing.dumps(..., compress=True)` with
//! `SessionBase.encode`'s JSON): the DB row is written first, then the `cached_db` copy in
//! Django's cache is **deleted** so Django reloads the DB version on its next access.
//! pl-accounts depends on this crate, so the minimal writer lives here (see contract,
//! CORE-REQUEST: one shared session store in pl-authz).

use axum::http::HeaderMap;
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde_json::{Map, Value};
use sqlx::PgPool;

use pl_core::AppState;

use crate::http::{HResult, server_error};

/// `cached_db.KEY_PREFIX`.
pub const CACHE_KEY_PREFIX: &str = "django.contrib.sessions.cached_db";

/// `SessionBase._get_new_session_key`: 32 characters of `[a-z0-9]`.
fn random_key() -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
}

/// `get_expiry_age()` (no `_session_expiry` → `SESSION_COOKIE_AGE`).
pub fn expiry_age(data: &Map<String, Value>, cookie_age: i64, now: DateTime<Utc>) -> i64 {
    match data.get("_session_expiry") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(cookie_age),
        Some(Value::String(iso)) => {
            DateTime::parse_from_rfc3339(iso).map(|d| (d.with_timezone(&Utc) - now).num_seconds().max(0)).unwrap_or(cookie_age)
        }
        _ => cookie_age,
    }
}

/// `get_expire_at_browser_close()`.
pub fn expire_at_browser_close(data: &Map<String, Value>, default: bool) -> bool {
    match data.get("_session_expiry") {
        None | Some(Value::Null) => default,
        Some(v) => v.as_i64() == Some(0),
    }
}

/// The `Set-Cookie` value `SessionMiddleware` emits after saving.
pub fn session_cookie(name: &str, key: &str, age: i64, at_browser_close: bool, now: DateTime<Utc>) -> String {
    if at_browser_close {
        format!("{name}={key}; HttpOnly; Path=/; SameSite=Lax")
    } else {
        let expires = (now + Duration::seconds(age)).format("%a, %d %b %Y %H:%M:%S GMT");
        format!("{name}={key}; expires={expires}; HttpOnly; Max-Age={age}; Path=/; SameSite=Lax")
    }
}

async fn load(db: &PgPool, secret: &str, key: &str) -> Result<Option<Map<String, Value>>, sqlx::Error> {
    if key.chars().count() < 8 || key.contains('\0') {
        return Ok(None);
    }
    let row: Option<(String,)> = sqlx::query_as("SELECT session_data FROM django_session WHERE session_key = $1 AND expire_date > now()")
        .bind(key)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|(d,)| pl_authz::signing::decode_session(&d, secret)))
}

/// `request.session[key] = value` for the session named by the request's session cookie
/// (a new session when there is none or it is unknown/expired), saved like
/// `SessionMiddleware.process_response` for a successful response. Returns the
/// `Set-Cookie` header value.
pub async fn set_session_value(state: &AppState, headers: &HeaderMap, key: &str, value: Value) -> HResult<String> {
    // SESSION_COOKIE_AGE live (docs/SETTINGS.md WP4b); restart-only keys keep their start value.
    let cfg = state.live_config();
    let db = state.db();
    let cookies = pl_authz::csrf::parse_cookies(headers);
    let existing = pl_authz::csrf::cookie(&cookies, &cfg.session_cookie_name).map(str::to_string);
    let (mut session_key, mut data) = (None, Map::new());
    if let Some(k) = existing
        && let Some(d) = load(db, &cfg.secret_key, &k).await?
    {
        session_key = Some(k);
        data = d;
    }
    data.insert(key.to_string(), value);

    let now = Utc::now();
    let age = expiry_age(&data, cfg.session_cookie_age, now);
    let json = pl_authz::signing::python_json_dumps(&Value::Object(data.clone()));
    let encoded = pl_authz::signing::encode_session(&json, &cfg.secret_key, now.timestamp());
    let expire = now + Duration::seconds(age);
    let saved_key = match session_key {
        Some(k) => {
            sqlx::query(
                "INSERT INTO django_session (session_key, session_data, expire_date) VALUES ($1, $2, $3) \
                 ON CONFLICT (session_key) DO UPDATE SET session_data = EXCLUDED.session_data, expire_date = EXCLUDED.expire_date",
            )
            .bind(&k)
            .bind(&encoded)
            .bind(expire)
            .execute(db)
            .await?;
            k
        }
        None => loop {
            let k = random_key();
            let r = sqlx::query(
                "INSERT INTO django_session (session_key, session_data, expire_date) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            )
            .bind(&k)
            .bind(&encoded)
            .bind(expire)
            .execute(db)
            .await?;
            if r.rows_affected() == 1 {
                break k;
            }
        },
    };
    // cached_db: `cache.delete(cache_key)` (lokal: Prozess-Cache; `PAPERRUST_CACHE=postgres`: gemeinsame Tabelle)
    state.cache().delete(&format!("{CACHE_KEY_PREFIX}{saved_key}")).await.map_err(|e| server_error(format!("session cache: {e:?}")))?;
    let at_close = expire_at_browser_close(&data, cfg.session_expire_at_browser_close);
    Ok(session_cookie(&cfg.session_cookie_name, &saved_key, age, at_close, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn new_key_shape() {
        let k = random_key();
        assert_eq!(k.len(), 32);
        assert!(k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }

    #[test]
    fn cookie_like_django() {
        let now = Utc.with_ymd_and_hms(2026, 11, 8, 12, 0, 0).unwrap();
        assert_eq!(
            session_cookie("sessionid", "abc", 1_814_400, false, now),
            "sessionid=abc; expires=Sun, 29 Nov 2026 12:00:00 GMT; HttpOnly; Max-Age=1814400; Path=/; SameSite=Lax"
        );
        assert_eq!(session_cookie("sessionid", "abc", 5, true, now), "sessionid=abc; HttpOnly; Path=/; SameSite=Lax");
    }

    #[test]
    fn expiry_rules() {
        let now = Utc::now();
        let mut d = Map::new();
        assert_eq!(expiry_age(&d, 100, now), 100);
        assert!(!expire_at_browser_close(&d, false));
        d.insert("_session_expiry".into(), Value::from(0));
        assert_eq!(expiry_age(&d, 100, now), 0);
        assert!(expire_at_browser_close(&d, false));
    }
}
