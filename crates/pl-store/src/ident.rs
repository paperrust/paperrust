//! Object identity (`uuid`, `rev`) from `paperrust_sync_object` (docs/SYNC.md §2): before data model stage 5
//! the side table, afterwards a read view over `uuid`/`rev` in the rows (docs/DATAMODEL-V2.md §1.10).
//! `local_id` is `integer` in the view (primary key) – hence `::int8` when reading.

use std::collections::HashMap;

use pl_domain::{DomainError, DomainResult, Kind};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db_err;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ident {
    pub uuid: Uuid,
    pub rev: i64,
}

/// Identity per `local_id`; missing rows are created (idempotent).
pub(crate) async fn idents(db: &PgPool, kind: Kind, ids: &[i64]) -> DomainResult<HashMap<i64, Ident>> {
    let mut ids: Vec<i64> = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut out = fetch(db, kind, &ids).await?;
    if out.len() < ids.len() {
        let missing: Vec<i64> = ids.iter().copied().filter(|i| !out.contains_key(i)).collect();
        // pl_sync 0003: creates missing identities (from stage 5 only kinds 7–9)
        sqlx::query("SELECT paperrust_sync_ensure($1, $2::int8[])")
        .bind(kind.code())
        .bind(&missing)
        .execute(db)
        .await
        .map_err(db_err)?;
        out.extend(fetch(db, kind, &missing).await?);
    }
    Ok(out)
}

async fn fetch(db: &PgPool, kind: Kind, ids: &[i64]) -> DomainResult<HashMap<i64, Ident>> {
    let rows: Vec<(i64, Uuid, i64)> = sqlx::query_as(
        "SELECT local_id::int8, uuid, rev FROM paperrust_sync_object WHERE kind = $1 AND local_id = ANY($2::int8[])",
    )
    .bind(kind.code())
    .bind(ids)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(|(id, uuid, rev)| (id, Ident { uuid, rev })).collect())
}

/// UUIDs → (`local_id`, `rev`) of live objects (tombstones count as unknown).
pub(crate) async fn resolve(db: &PgPool, kind: Kind, uuids: &[Uuid]) -> DomainResult<HashMap<Uuid, (i64, i64)>> {
    if uuids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT uuid, local_id::int8, rev FROM paperrust_sync_object
         WHERE kind = $1 AND uuid = ANY($2::uuid[]) AND deleted_at IS NULL",
    )
    .bind(kind.code())
    .bind(uuids)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(|(u, id, rev)| (u, (id, rev))).collect())
}

/// One UUID → (`local_id`, `rev`) or 404.
pub(crate) async fn resolve_one(db: &PgPool, kind: Kind, uuid: Uuid) -> DomainResult<(i64, i64)> {
    resolve(db, kind, &[uuid]).await?.remove(&uuid).ok_or(DomainError::NotFound)
}

/// References (UUIDs) of a kind into IDs; unknown → field error.
pub(crate) async fn ref_ids(db: &PgPool, kind: Kind, field: &str, uuids: &[Uuid]) -> DomainResult<Vec<i64>> {
    let map = resolve(db, kind, uuids).await?;
    uuids
        .iter()
        .map(|u| {
            map.get(u)
                .map(|(id, _)| *id)
                .ok_or_else(|| DomainError::field(field, format!("Unknown {} \"{u}\".", kind.as_str())))
        })
        .collect()
}
