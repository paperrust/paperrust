//! `paperrust_asn_print`: printed (reserved) ASN ranges, collision check against
//! existing document ASNs and earlier prints, next free number.
//!
//! Existing ASNs count **including** trash and versions: the column
//! `documents_document.archive_serial_number` is unique across all rows (upstream
//! `unique=True`), so a number of a deleted document cannot be reassigned.

use serde::Serialize;
use sqlx::{PgConnection, PgPool, Row};

use crate::{Resolved, Result};

/// Migration (name, SQL) for `pl-cli migrate` (`paperrust_migrations`).
pub const MIGRATION: (&str, &str) = ("pl_labels.0001_paperrust_asn_print", include_str!("../migrations/0001_paperrust_asn_print.sql"));

pub mod migrations {
    pub const ALL: &[(&str, &str)] = &[super::MIGRATION, crate::ranges::MIGRATION];
}

/// Creates the tables if they are missing (idempotent; the same SQL as the migrations).
pub async fn migrate(pool: &PgPool) -> Result<()> {
    for (_, sql) in migrations::ALL {
        sqlx::raw_sql(sql).execute(pool).await?;
    }
    Ok(())
}

/// A printed range.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrintedRange {
    pub id: i64,
    pub first_asn: i64,
    pub last_asn: i64,
    pub prefix: String,
    pub digits: i16,
    pub code_type: String,
    pub sheet: String,
    pub label_count: i32,
    pub user_id: Option<i32>,
    pub username: Option<String>,
    pub created: chrono::DateTime<chrono::Utc>,
    /// Documents that now carry a number from the range.
    pub used: i64,
}

/// Collisions of a planned range.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Collisions {
    /// Existing document ASNs in the range (at most [`MAX_LISTED`]).
    pub documents: Vec<i64>,
    pub documents_total: i64,
    /// Overlapping earlier prints.
    pub printed: Vec<PrintedRange>,
}

impl Collisions {
    pub fn is_empty(&self) -> bool {
        self.documents_total == 0 && self.printed.is_empty()
    }
}

pub const MAX_LISTED: i64 = 50;

/// Next numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NextAsn {
    /// like v1 `/api/documents/next_asn/` (largest ASN of non-deleted documents + 1)
    pub documents_next: i64,
    /// largest assigned ASN overall (incl. trash), 0 if none
    pub max_document: i64,
    /// largest printed ASN, 0 if none
    pub max_printed: i64,
    /// Suggestion for the next print: larger than everything assigned and printed
    pub suggested: i64,
}

const RANGE_SELECT: &str = "SELECT p.id, p.first_asn, p.last_asn, p.prefix, p.digits, p.code_type, p.sheet, p.label_count,
        p.user_id, u.username, p.created,
        (SELECT count(*) FROM documents_document d WHERE d.archive_serial_number BETWEEN p.first_asn AND p.last_asn) AS used
   FROM paperrust_asn_print p LEFT JOIN auth_user u ON u.id = p.user_id";

fn range_of(r: &sqlx::postgres::PgRow) -> Result<PrintedRange> {
    Ok(PrintedRange {
        id: r.try_get("id")?,
        first_asn: r.try_get("first_asn")?,
        last_asn: r.try_get("last_asn")?,
        prefix: r.try_get("prefix")?,
        digits: r.try_get("digits")?,
        code_type: r.try_get("code_type")?,
        sheet: r.try_get("sheet")?,
        label_count: r.try_get("label_count")?,
        user_id: r.try_get("user_id")?,
        username: r.try_get("username")?,
        created: r.try_get("created")?,
        used: r.try_get("used")?,
    })
}

pub async fn next_asn(db: &mut PgConnection) -> Result<NextAsn> {
    let r = sqlx::query(
        "SELECT COALESCE((SELECT max(archive_serial_number) FROM documents_document WHERE deleted_at IS NULL), 0)::bigint AS live,
                COALESCE((SELECT max(archive_serial_number) FROM documents_document), 0)::bigint AS any,
                COALESCE((SELECT max(last_asn) FROM paperrust_asn_print), 0)::bigint AS printed",
    )
    .fetch_one(&mut *db)
    .await?;
    let live: i64 = r.try_get("live")?;
    let any: i64 = r.try_get("any")?;
    let printed: i64 = r.try_get("printed")?;
    Ok(NextAsn { documents_next: live + 1, max_document: any, max_printed: printed, suggested: any.max(printed) + 1 })
}

pub async fn collisions(db: &mut PgConnection, first: u64, last: u64) -> Result<Collisions> {
    let (f, l) = (first as i64, last as i64);
    let documents: Vec<i64> = sqlx::query_scalar(
        "SELECT archive_serial_number::bigint FROM documents_document WHERE archive_serial_number BETWEEN $1 AND $2 ORDER BY 1 LIMIT $3",
    )
    .bind(f)
    .bind(l)
    .bind(MAX_LISTED)
    .fetch_all(&mut *db)
    .await?;
    let documents_total: i64 = if (documents.len() as i64) < MAX_LISTED {
        documents.len() as i64
    } else {
        sqlx::query_scalar("SELECT count(*) FROM documents_document WHERE archive_serial_number BETWEEN $1 AND $2")
            .bind(f)
            .bind(l)
            .fetch_one(&mut *db)
            .await?
    };
    let rows = sqlx::query(&format!("{RANGE_SELECT} WHERE p.first_asn <= $2 AND p.last_asn >= $1 ORDER BY p.first_asn LIMIT 20"))
        .bind(f)
        .bind(l)
        .fetch_all(&mut *db)
        .await?;
    let printed = rows.iter().map(range_of).collect::<Result<Vec<_>>>()?;
    Ok(Collisions { documents, documents_total, printed })
}

/// Result of [`reserve`].
#[derive(Debug, Clone, PartialEq)]
pub enum Reserve {
    Recorded(PrintedRange),
    /// Collision and `allow_collisions = false`: nothing saved.
    Conflict(Collisions),
}

/// Checks collisions and saves the range in **one** transaction (serialised via an
/// advisory lock so that two simultaneous prints do not get the same numbers).
pub async fn reserve(pool: &PgPool, job: &Resolved, user_id: Option<i64>, allow_collisions: bool) -> Result<Reserve> {
    let mut tx = pool.begin().await?;
    // fixed lock key of this generator
    sqlx::query("SELECT pg_advisory_xact_lock(7052041510)").execute(&mut *tx).await?;
    let c = collisions(&mut tx, job.start, job.last()).await?;
    if !c.is_empty() && !allow_collisions {
        tx.rollback().await?;
        return Ok(Reserve::Conflict(c));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO paperrust_asn_print (first_asn, last_asn, prefix, digits, code_type, sheet, label_count, user_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(job.start as i64)
    .bind(job.last() as i64)
    .bind(&job.prefix)
    .bind(job.digits as i16)
    .bind(job.code.as_str())
    .bind(&job.sheet.id)
    .bind(job.count as i32)
    .bind(user_id.and_then(|u| i32::try_from(u).ok()))
    .fetch_one(&mut *tx)
    .await?;
    let row = sqlx::query(&format!("{RANGE_SELECT} WHERE p.id = $1")).bind(id).fetch_one(&mut *tx).await?;
    let range = range_of(&row)?;
    tx.commit().await?;
    Ok(Reserve::Recorded(range))
}

/// History, newest first.
pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<PrintedRange>> {
    let rows = sqlx::query(&format!("{RANGE_SELECT} ORDER BY p.created DESC, p.id DESC LIMIT $1"))
        .bind(limit.clamp(1, 500))
        .fetch_all(pool)
        .await?;
    rows.iter().map(range_of).collect()
}

pub async fn get(pool: &PgPool, id: i64) -> Result<Option<PrintedRange>> {
    let row = sqlx::query(&format!("{RANGE_SELECT} WHERE p.id = $1")).bind(id).fetch_optional(pool).await?;
    row.as_ref().map(range_of).transpose()
}

/// Releases a range (e.g. misprint). `true` = deleted.
pub async fn delete(pool: &PgPool, id: i64) -> Result<bool> {
    let n = sqlx::query("DELETE FROM paperrust_asn_print WHERE id = $1").bind(id).execute(pool).await?.rows_affected();
    Ok(n > 0)
}
