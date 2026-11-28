//! Device token (`Authorization: Bearer prt_…`) – PaperRust extension without a Django counterpart.
//!
//! Specification: docs/SYNC.md §8, docs/ARCHITECTURE-V2.md §3 "Shared login".
//! Contract: docs/contract/pl-authz.md ("Device token", known difference).
//!
//! * Format: `prt_` + 43 base62 characters (32 random bytes from the OS RNG, ≈ 256 bit).
//! * Only `sha256(token)` (hex) is stored in `paperrust_device_token`; the plaintext is shown
//!   exactly once (at issue).
//! * Revocation sets `revoked_at`; there is no cache – every request checks the table, so
//!   revocation takes effect immediately.
//! * If the table is missing (migration not applied), every device token is invalid.

use chrono::{DateTime, Utc};
use rand::RngCore as _;
use sha2::{Digest, Sha256};
use sqlx::{Connection as _, PgConnection, PgPool};
use uuid::Uuid;

/// Prefix of all device tokens.
pub const PREFIX: &str = "prt_";

/// Migrations of this crate (name, SQL) – order = application order.
pub mod migrations {
    /// `migrations/0001_paperrust_device_token.sql`.
    pub const M0001_DEVICE_TOKEN: &str = include_str!("../migrations/0001_paperrust_device_token.sql");
    pub const NAME_0001: &str = "pl_authz.0001_paperrust_device_token";
    /// For `pl_cli::migrate::paperrust_migrations()` (CORE-REQUEST P3b).
    pub const ALL: &[(&str, &str)] = &[(NAME_0001, M0001_DEVICE_TOKEN)];

    /// Applies the migration directly (idempotent; for tests and tools).
    pub async fn apply(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        sqlx::raw_sql(M0001_DEVICE_TOKEN).execute(pool).await?;
        Ok(())
    }
}

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// Length of the base62 part (32 bytes → ⌈256 / log2(62)⌉ = 43 characters).
pub const BODY_LEN: usize = 43;

/// New token (plaintext). For one-time output only.
pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{PREFIX}{}", base62(&bytes))
}

/// 32 bytes → exactly [`BODY_LEN`] base62 characters (big number, leading zeros padded).
fn base62(bytes: &[u8; 32]) -> String {
    let mut num: Vec<u8> = bytes.to_vec();
    let mut out = Vec::with_capacity(BODY_LEN);
    while num.iter().any(|b| *b != 0) {
        let mut rem: u32 = 0;
        for b in num.iter_mut() {
            let acc = (rem << 8) | u32::from(*b);
            *b = (acc / 62) as u8;
            rem = acc % 62;
        }
        out.push(BASE62[rem as usize]);
    }
    while out.len() < BODY_LEN {
        out.push(b'0');
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Formally valid (prefix + 43 base62 characters)? Invalid forms are rejected without DB access.
pub fn is_well_formed(token: &str) -> bool {
    token.strip_prefix(PREFIX).is_some_and(|b| b.len() == BODY_LEN && b.bytes().all(|c| c.is_ascii_alphanumeric()))
}

/// `sha256(token)` as 64 hex characters (stored form).
pub fn hash(token: &str) -> String {
    let d = Sha256::digest(token.as_bytes());
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// A device (without token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub uuid: Uuid,
    pub user_id: i64,
    pub name: String,
    pub created: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

fn is_undefined_table(e: &sqlx::Error) -> bool {
    e.as_database_error().and_then(|d| d.code()).as_deref() == Some("42P01")
}

/// Error on create/read.
#[derive(Debug)]
pub enum DeviceError {
    /// Table missing (migration `pl_authz.0001_paperrust_device_token` not applied).
    NotInstalled,
    Db(sqlx::Error),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceError::NotInstalled => f.write_str("device tokens are not installed (run `paperrust manage migrate`)"),
            DeviceError::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for DeviceError {}

impl From<sqlx::Error> for DeviceError {
    fn from(e: sqlx::Error) -> Self {
        if is_undefined_table(&e) { DeviceError::NotInstalled } else { DeviceError::Db(e) }
    }
}

/// Create a new device for `user_id`. Returns: (device, plaintext token).
pub async fn create(db: &PgPool, user_id: i64, name: &str) -> Result<(Device, String), DeviceError> {
    let token = generate();
    let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO paperrust_device_token (user_id, name, token_hash) VALUES ($1::int4, $2, $3)
         RETURNING uuid, created",
    )
    .bind(user_id)
    .bind(name)
    .bind(hash(&token))
    .fetch_one(db)
    .await?;
    Ok((Device { uuid: row.0, user_id, name: name.to_string(), created: row.1, last_used: None, revoked_at: None }, token))
}

/// All **non-revoked** devices of a user (newest first).
pub async fn list(db: &PgPool, user_id: i64) -> Result<Vec<Device>, DeviceError> {
    type Row = (Uuid, i32, String, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT uuid, user_id, name, created, last_used, revoked_at FROM paperrust_device_token
         WHERE user_id = $1::int4 AND revoked_at IS NULL ORDER BY created DESC, id DESC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Device { uuid: r.0, user_id: i64::from(r.1), name: r.2, created: r.3, last_used: r.4, revoked_at: r.5 })
        .collect())
}

/// Revoke (own device only). `true` if an active device was revoked.
pub async fn revoke(db: &PgPool, user_id: i64, uuid: Uuid) -> Result<bool, DeviceError> {
    let n = sqlx::query(
        "UPDATE paperrust_device_token SET revoked_at = now()
         WHERE uuid = $1 AND user_id = $2::int4 AND revoked_at IS NULL",
    )
    .bind(uuid)
    .bind(user_id)
    .execute(db)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// Statement of [`revoke_all_for_user`] / [`revoke_all_for_user_tx`] (one text, one bind type:
/// `i64` cast to `int4`, for the sqlx statement cache).
const REVOKE_ALL_FOR_USER_SQL: &str =
    "UPDATE paperrust_device_token SET revoked_at = now() WHERE user_id = $1::int4 AND revoked_at IS NULL";

/// Revoke all active device tokens of a user (after password reset/change,
/// security review 2026-11-19 F13: a stolen device should not be able to keep
/// accessing after the password change). DRF `authtoken` stays untouched, as upstream. If the table is missing
/// (migration not applied), there are no tokens: `Ok(0)`. Returns: number revoked.
pub async fn revoke_all_for_user(db: &PgPool, user_id: i64) -> Result<u64, sqlx::Error> {
    let mut conn = db.acquire().await?;
    revoke_all_for_user_tx(&mut conn, user_id).await
}

/// [`revoke_all_for_user`] on the caller's connection, typically inside its transaction, so that a
/// password change and the revocation commit together. The statement runs in a savepoint (or its
/// own transaction on an autocommit connection): a missing `paperrust_device_token` table counts as
/// "nothing to revoke" (`Ok(0)`) and does not abort the caller's transaction. Never commits the
/// caller's transaction.
pub async fn revoke_all_for_user_tx(conn: &mut PgConnection, user_id: i64) -> Result<u64, sqlx::Error> {
    let mut sp = conn.begin().await?;
    match sqlx::query(REVOKE_ALL_FOR_USER_SQL).bind(user_id).execute(&mut *sp).await {
        Ok(done) => {
            sp.commit().await?;
            Ok(done.rows_affected())
        }
        Err(e) if is_undefined_table(&e) => {
            sp.rollback().await?;
            Ok(0)
        }
        Err(e) => Err(e),
    }
}

/// Check token: user ID of the active device or `None` (unknown, revoked, missing
/// table). Updates `last_used` at most once per minute (in the same statement).
pub async fn lookup_user_id(db: &PgPool, token: &str) -> Result<Option<i64>, sqlx::Error> {
    if !is_well_formed(token) {
        return Ok(None);
    }
    let r: Result<Option<(i32,)>, sqlx::Error> = sqlx::query_as(
        "WITH t AS (SELECT id, user_id, last_used FROM paperrust_device_token
                     WHERE token_hash = $1 AND revoked_at IS NULL),
              u AS (UPDATE paperrust_device_token d SET last_used = now() FROM t
                     WHERE d.id = t.id AND (t.last_used IS NULL OR t.last_used < now() - interval '60 seconds')
                     RETURNING d.id)
         SELECT user_id FROM t",
    )
    .bind(hash(token))
    .fetch_optional(db)
    .await;
    match r {
        Ok(v) => Ok(v.map(|(id,)| i64::from(id))),
        Err(e) if is_undefined_table(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_shape() {
        let t = generate();
        assert!(t.starts_with("prt_"));
        assert_eq!(t.len(), 4 + BODY_LEN);
        assert!(is_well_formed(&t));
        assert_ne!(generate(), t);
        assert!(!is_well_formed("prt_short"));
        assert!(!is_well_formed(&format!("prt_{}", "-".repeat(BODY_LEN))));
        assert!(!is_well_formed(&t.replacen("prt_", "prx_", 1)));
    }

    #[test]
    fn base62_edges() {
        assert_eq!(base62(&[0u8; 32]), "0".repeat(BODY_LEN));
        let max = base62(&[0xffu8; 32]);
        assert_eq!(max.len(), BODY_LEN);
        assert!(max.bytes().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn hash_is_sha256_hex() {
        assert_eq!(hash("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hash(&generate()).len(), 64);
    }
}
