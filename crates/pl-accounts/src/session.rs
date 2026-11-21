//! Read **and write** Django sessions (`SESSION_ENGINE = cached_db`).
//!
//! upstream: django/contrib/sessions/backends/{base,db,cached_db}.py@5.2 (`load`, `save`,
//! `create`, `cycle_key`, `flush`, `get_expiry_age`, `get_expire_at_browser_close`,
//! `_get_new_session_key`: 32 characters `[a-z0-9]`), sessions/middleware.py (`process_response`:
//! set/delete cookie), contrib/auth/__init__.py (`login`, `logout`, `SESSION_KEY` …).
//!
//! Data like `SessionBase.encode` (`signing.dumps(..., compress=True)`, compact JSON with
//! `ensure_ascii`). `cached_db` additionally keeps a pickle copy in the Django cache
//! (`django.contrib.sessions.cached_db<key>`); Rust always writes the DB first and then **deletes**
//! the cache entry so that Django loads the DB version on the next access.

use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde_json::{Map, Value};
use sqlx::PgPool;

use crate::cache::Cache;
use crate::settings::Settings;

pub const SESSION_KEY: &str = "_auth_user_id";
pub const BACKEND_SESSION_KEY: &str = "_auth_user_backend";
pub const HASH_SESSION_KEY: &str = "_auth_user_hash";
pub const CACHE_KEY_PREFIX: &str = "django.contrib.sessions.cached_db";

/// A Django session for the duration of one request.
#[derive(Debug, Clone, Default)]
pub struct Session {
    key: Option<String>,
    pub data: Map<String, Value>,
    /// Cookie value was present.
    pub cookie_sent: bool,
    /// `session.accessed`.
    pub accessed: bool,
    /// `session.modified`.
    pub modified: bool,
    /// Old keys to delete (cycle_key/flush).
    deleted: Vec<String>,
}

#[derive(Debug)]
pub enum SessionError {
    Db(sqlx::Error),
    Cache(String),
}

impl From<sqlx::Error> for SessionError {
    fn from(e: sqlx::Error) -> Self {
        SessionError::Db(e)
    }
}

fn random_key() -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
}

impl Session {
    /// `SessionStore(session_key=cookie).load()` (DB authoritative, valid only until `expire_date`).
    pub async fn load(db: &PgPool, s: &Settings, cookie: Option<&str>) -> Result<Self, SessionError> {
        let mut sess = Session { cookie_sent: cookie.is_some(), ..Default::default() };
        let Some(key) = cookie else { return Ok(sess) };
        sess.accessed = true;
        if key.chars().count() < 8 || key.contains('\0') {
            return Ok(sess);
        }
        let row: Option<(String,)> =
            sqlx::query_as("SELECT session_data FROM django_session WHERE session_key = $1 AND expire_date > now()")
                .bind(key)
                .fetch_optional(db)
                .await?;
        // db.SessionStore.load: no row -> `_session_key = None` (a new key is created on save)
        if let Some((data,)) = row {
            sess.key = Some(key.to_string());
            sess.data = pl_authz::signing::decode_session(&data, &s.secret_key);
        }
        Ok(sess)
    }

    pub fn session_key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub fn get(&self, k: &str) -> Option<&Value> {
        self.data.get(k)
    }

    pub fn set(&mut self, k: &str, v: Value) {
        self.accessed = true;
        self.modified = true;
        self.data.insert(k.to_string(), v);
    }

    pub fn pop(&mut self, k: &str) -> Option<Value> {
        self.accessed = true;
        let v = self.data.shift_remove(k);
        if v.is_some() {
            self.modified = true;
        }
        v
    }

    /// `is_empty()`: no key and no data.
    pub fn is_empty(&self) -> bool {
        self.key.is_none() && self.data.is_empty()
    }

    /// `flush()`: clear data, delete row + cache, new key on next save.
    pub fn flush(&mut self) {
        self.data.clear();
        if let Some(k) = self.key.take() {
            self.deleted.push(k);
        }
        self.accessed = true;
        self.modified = true;
    }

    /// `cycle_key()`: same data under a new key (the old one is deleted).
    pub fn cycle_key(&mut self) {
        if let Some(k) = self.key.take() {
            self.deleted.push(k);
        }
        self.accessed = true;
        self.modified = true;
    }

    /// `set_expiry(value)` with an integer value (0 = browser close).
    pub fn set_expiry(&mut self, seconds: i64) {
        self.set("_session_expiry", Value::from(seconds));
    }

    /// `get_expiry_age()`.
    pub fn expiry_age(&self, s: &Settings) -> i64 {
        match self.data.get("_session_expiry") {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(s.session_cookie_age),
            Some(Value::String(iso)) => DateTime::parse_from_rfc3339(iso)
                .map(|d| (d.with_timezone(&Utc) - Utc::now()).num_seconds().max(0))
                .unwrap_or(s.session_cookie_age),
            _ => s.session_cookie_age,
        }
    }

    /// `get_expire_at_browser_close()`.
    pub fn expire_at_browser_close(&self, s: &Settings) -> bool {
        match self.data.get("_session_expiry") {
            None | Some(Value::Null) => s.session_expire_at_browser_close,
            Some(v) => v.as_i64() == Some(0),
        }
    }

    /// `SessionMiddleware.process_response` + `cached_db.save/delete`: write DB, remove cache entries,
    /// return `Set-Cookie` value (`None` = no cookie).
    pub async fn finish(&mut self, db: &PgPool, cache: &Cache, s: &Settings, status: u16) -> Result<Option<String>, SessionError> {
        for old in std::mem::take(&mut self.deleted) {
            sqlx::query("DELETE FROM django_session WHERE session_key = $1").bind(&old).execute(db).await?;
            cache.delete(&format!("{CACHE_KEY_PREFIX}{old}")).await.map_err(|e| SessionError::Cache(e.0))?;
        }
        let name = &s.session_cookie_name;
        if self.cookie_sent && self.is_empty() {
            return Ok(Some(format!("{name}=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; Path=/; SameSite=Lax")));
        }
        if !self.modified || self.is_empty() || status >= 500 {
            return Ok(None);
        }
        let age = self.expiry_age(s);
        let json = pl_authz::signing::python_json_dumps(&Value::Object(self.data.clone()));
        let now = Utc::now();
        let encoded = pl_authz::signing::encode_session(&json, &s.secret_key, now.timestamp());
        let expire = now + Duration::seconds(age);
        let key = match self.key.clone() {
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
        cache.delete(&format!("{CACHE_KEY_PREFIX}{key}")).await.map_err(|e| SessionError::Cache(e.0))?;
        self.key = Some(key.clone());
        Ok(Some(if self.expire_at_browser_close(s) {
            format!("{name}={key}; HttpOnly; Path=/; SameSite=Lax")
        } else {
            format!("{name}={key}; expires={}; HttpOnly; Max-Age={age}; Path=/; SameSite=Lax", http_date(now + Duration::seconds(age)))
        }))
    }

    /// Logged-in user id (`_auth_user_id`), unverified.
    pub fn user_id(&self) -> Option<i64> {
        match self.data.get(SESSION_KEY) {
            Some(Value::String(s)) => s.trim().parse().ok(),
            Some(Value::Number(n)) => n.as_i64(),
            _ => None,
        }
    }
}

/// Size of one deletion pass of [`clear_expired_sessions`].
pub const CLEAR_SESSIONS_BATCH: i64 = 500;

/// Result of [`clear_expired_sessions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClearedSessions {
    /// Deleted sessions.
    pub deleted: u64,
    /// Executed `DELETE` statements.
    pub batches: u32,
}

/// Delete expired sessions from `django_session`, in passes of at most
/// [`CLEAR_SESSIONS_BATCH`] rows (oldest first), so that no single deletion locks the table
/// for long. "Expired" means `expire_date` before the start time `now`.
///
/// upstream fix a139bdff4 (#14722, paperless-ngx `dev` after v3.4.2):
/// `documents/tasks.py::clear_expired_sessions` (beat "Clear expired sessions"). Own
/// implementation: one `DELETE … WHERE session_key IN (SELECT … LIMIT 500)` per pass; stop
/// as soon as a pass hits fewer than 500 rows (upstream: as soon as the selection is empty –
/// same number of `DELETE`s). The `cached_db` cache copies stay around as upstream; they
/// expire on their own and are no longer loaded without a DB row.
pub async fn clear_expired_sessions(db: &PgPool, now: DateTime<Utc>) -> Result<ClearedSessions, sqlx::Error> {
    let mut out = ClearedSessions { deleted: 0, batches: 0 };
    loop {
        let n = sqlx::query(
            "DELETE FROM django_session WHERE session_key IN \
             (SELECT session_key FROM django_session WHERE expire_date < $1 ORDER BY expire_date LIMIT $2)",
        )
        .bind(now)
        .bind(CLEAR_SESSIONS_BATCH)
        .execute(db)
        .await?
        .rows_affected();
        out.batches += 1;
        out.deleted += n;
        if n < CLEAR_SESSIONS_BATCH as u64 {
            break;
        }
    }
    tracing::info!(target: "paperless.tasks", deleted = out.deleted, "Cleared expired sessions");
    Ok(out)
}

/// `django.utils.http.http_date` (`Sun, 29 Nov 2026 06:56:19 GMT`).
pub fn http_date(t: DateTime<Utc>) -> String {
    t.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_dates() {
        let k = random_key();
        assert_eq!(k.len(), 32);
        assert!(k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        let t = DateTime::parse_from_rfc3339("2026-11-29T06:56:19Z").unwrap().with_timezone(&Utc);
        assert_eq!(http_date(t), "Sun, 29 Nov 2026 06:56:19 GMT");
    }
}
