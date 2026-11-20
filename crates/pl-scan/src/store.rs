//! Tables `paperrust_scanner` and `paperrust_scan_device` (migrations/0001_paperrust_scan.sql).
//! Created on first access ([`ensure`]) or with `paperrust manage migrate`.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::Serialize;
use sqlx::{PgPool, Row};

use crate::token;

/// Migration (name, SQL) for `paperrust manage migrate`.
pub const MIGRATION: (&str, &str) = ("pl_scan.0001_paperrust_scan", include_str!("../migrations/0001_paperrust_scan.sql"));

pub mod migrations {
    pub const ALL: &[(&str, &str)] = &[super::MIGRATION];
}

static READY: AtomicBool = AtomicBool::new(false);

/// SQL file into statements (without `--` comments).
fn statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in sql.lines() {
        let l = line.split("--").next().unwrap_or("").trim_end();
        if l.trim().is_empty() {
            continue;
        }
        cur.push_str(l);
        cur.push('\n');
        if l.ends_with(';') {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Create tables (idempotent, advisory lock).
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('paperrust_scan'))").execute(&mut *tx).await?;
    for stmt in statements(MIGRATION.1) {
        sqlx::query(&stmt).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    READY.store(true, Ordering::Relaxed);
    Ok(())
}

/// [`migrate`] once per process.
pub async fn ensure(pool: &PgPool) -> Result<(), sqlx::Error> {
    if !READY.load(Ordering::Relaxed) {
        migrate(pool).await?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------- Scanner

#[derive(Debug, Clone, Serialize)]
pub struct Scanner {
    pub id: i64,
    pub name: String,
    pub base_url: String,
    pub uuid: Option<String>,
    pub model: Option<String>,
    pub created: DateTime<Utc>,
}

fn scanner_row(r: &sqlx::postgres::PgRow) -> Result<Scanner, sqlx::Error> {
    Ok(Scanner {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        base_url: r.try_get("base_url")?,
        uuid: r.try_get("uuid")?,
        model: r.try_get("model")?,
        created: r.try_get("created")?,
    })
}

const SCANNER_COLS: &str = "id, name, base_url, uuid, model, created";

pub async fn list_scanners(pool: &PgPool) -> Result<Vec<Scanner>, sqlx::Error> {
    ensure(pool).await?;
    let rows = sqlx::query(&format!("SELECT {SCANNER_COLS} FROM paperrust_scanner ORDER BY lower(name), id")).fetch_all(pool).await?;
    rows.iter().map(scanner_row).collect()
}

pub async fn get_scanner(pool: &PgPool, id: i64) -> Result<Option<Scanner>, sqlx::Error> {
    ensure(pool).await?;
    let row = sqlx::query(&format!("SELECT {SCANNER_COLS} FROM paperrust_scanner WHERE id = $1")).bind(id).fetch_optional(pool).await?;
    row.as_ref().map(scanner_row).transpose()
}

/// Create; `None` if the address is already known.
pub async fn insert_scanner(pool: &PgPool, name: &str, base_url: &str, uuid: Option<&str>, model: Option<&str>, created_by: i64) -> Result<Option<Scanner>, sqlx::Error> {
    ensure(pool).await?;
    let row = sqlx::query(&format!(
        "INSERT INTO paperrust_scanner (name, base_url, uuid, model, created_by) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (base_url) DO NOTHING RETURNING {SCANNER_COLS}"
    ))
    .bind(name)
    .bind(base_url)
    .bind(uuid)
    .bind(model)
    .bind(created_by as i32)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(scanner_row).transpose()
}

pub async fn rename_scanner(pool: &PgPool, id: i64, name: &str) -> Result<Option<Scanner>, sqlx::Error> {
    ensure(pool).await?;
    let row = sqlx::query(&format!("UPDATE paperrust_scanner SET name = $2 WHERE id = $1 RETURNING {SCANNER_COLS}"))
        .bind(id)
        .bind(name)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(scanner_row).transpose()
}

pub async fn delete_scanner(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    ensure(pool).await?;
    Ok(sqlx::query("DELETE FROM paperrust_scanner WHERE id = $1").bind(id).execute(pool).await?.rows_affected() > 0)
}

// ------------------------------------------------------------------------------- Devices

/// Device for the SMTP intake (without the token hash in the output).
#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: i64,
    pub name: String,
    pub token_hint: String,
    pub owner_id: i64,
    pub owner_username: Option<String>,
    pub allowed_ips: String,
    pub created: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used: Option<DateTime<Utc>>,
    pub last_ip: Option<String>,
    pub documents_received: i64,
}

fn device_row(r: &sqlx::postgres::PgRow) -> Result<Device, sqlx::Error> {
    Ok(Device {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        token_hint: r.try_get("token_hint")?,
        owner_id: i64::from(r.try_get::<i32, _>("owner_id")?),
        owner_username: r.try_get("owner_username")?,
        allowed_ips: r.try_get("allowed_ips")?,
        created: r.try_get("created")?,
        revoked_at: r.try_get("revoked_at")?,
        last_used: r.try_get("last_used")?,
        last_ip: r.try_get("last_ip")?,
        documents_received: r.try_get("documents_received")?,
    })
}

const DEVICE_SELECT: &str = "SELECT d.id, d.name, d.token_hint, d.owner_id, u.username AS owner_username, d.allowed_ips, d.created,
    d.revoked_at, d.last_used, d.last_ip, d.documents_received
    FROM paperrust_scan_device d LEFT JOIN auth_user u ON u.id = d.owner_id";

pub async fn list_devices(pool: &PgPool) -> Result<Vec<Device>, sqlx::Error> {
    ensure(pool).await?;
    let rows = sqlx::query(&format!("{DEVICE_SELECT} ORDER BY d.revoked_at IS NOT NULL, lower(d.name), d.id")).fetch_all(pool).await?;
    rows.iter().map(device_row).collect()
}

/// Create → (device, plaintext token). The plaintext is returned only here.
pub async fn insert_device(pool: &PgPool, name: &str, owner_id: i64, allowed_ips: &str, created_by: i64) -> Result<(Device, String), sqlx::Error> {
    ensure(pool).await?;
    let tok = token::generate();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO paperrust_scan_device (name, token_hash, token_hint, owner_id, allowed_ips, created_by)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(name)
    .bind(token::hash(&tok))
    .bind(token::hint(&tok))
    .bind(owner_id as i32)
    .bind(allowed_ips)
    .bind(created_by as i32)
    .fetch_one(pool)
    .await?;
    let row = sqlx::query(&format!("{DEVICE_SELECT} WHERE d.id = $1")).bind(id).fetch_one(pool).await?;
    Ok((device_row(&row)?, tok))
}

/// Revoke (effective immediately, no cache). `false` if unknown or already revoked.
pub async fn revoke_device(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    ensure(pool).await?;
    Ok(sqlx::query("UPDATE paperrust_scan_device SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Delete permanently (revoked only).
pub async fn delete_device(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    ensure(pool).await?;
    Ok(sqlx::query("DELETE FROM paperrust_scan_device WHERE id = $1 AND revoked_at IS NOT NULL").bind(id).execute(pool).await?.rows_affected() > 0)
}

/// Device as the SMTP intake needs it.
#[derive(Debug, Clone)]
pub struct ActiveDevice {
    pub id: i64,
    pub name: String,
    pub owner_id: i64,
    pub allowed_ips: String,
}

/// Access of the SMTP intake (replaceable for tests).
pub trait DeviceStore: Send + Sync {
    /// Active device for the token hash (not revoked, owner active).
    fn find_active(&self, token_hash: &str) -> BoxFuture<'_, Result<Option<ActiveDevice>, String>>;
    /// Record usage.
    fn record_use(&self, id: i64, ip: &str, documents: i64) -> BoxFuture<'_, ()>;
}

pub struct PgDevices(pub PgPool);

impl DeviceStore for PgDevices {
    fn find_active(&self, token_hash: &str) -> BoxFuture<'_, Result<Option<ActiveDevice>, String>> {
        let hash = token_hash.to_string();
        Box::pin(async move {
            ensure(&self.0).await.map_err(|e| e.to_string())?;
            let row = sqlx::query(
                "SELECT d.id, d.name, d.owner_id, d.allowed_ips FROM paperrust_scan_device d
                 JOIN auth_user u ON u.id = d.owner_id AND u.is_active
                 WHERE d.token_hash = $1 AND d.revoked_at IS NULL",
            )
            .bind(&hash)
            .fetch_optional(&self.0)
            .await
            .map_err(|e| e.to_string())?;
            row.map(|r| -> Result<ActiveDevice, sqlx::Error> {
                Ok(ActiveDevice {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    owner_id: i64::from(r.try_get::<i32, _>("owner_id")?),
                    allowed_ips: r.try_get("allowed_ips")?,
                })
            })
            .transpose()
            .map_err(|e| e.to_string())
        })
    }

    fn record_use(&self, id: i64, ip: &str, documents: i64) -> BoxFuture<'_, ()> {
        let ip: String = ip.chars().take(64).collect();
        Box::pin(async move {
            let r = sqlx::query(
                "UPDATE paperrust_scan_device SET last_used = now(), last_ip = $2, documents_received = documents_received + $3 WHERE id = $1",
            )
            .bind(id)
            .bind(ip)
            .bind(documents)
            .execute(&self.0)
            .await;
            if let Err(e) = r {
                tracing::warn!(target: "paperrust.scan", error = %e, "could not record device use");
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_statements() {
        let s = statements(MIGRATION.1);
        assert_eq!(s.len(), 2);
        assert!(s[0].contains("paperrust_scanner") && s[1].contains("paperrust_scan_device"));
    }
}
