//! pl-sync – change capture for sync (docs/SYNC.md §2, contract docs/contract/pl-sync.md).
//!
//! * [`migrations`]: `pl_sync.0001_paperrust_sync` (expand-only side tables `paperrust_sync_*`,
//!   generic AFTER statement triggers with transition tables, SQL functions for sequencer,
//!   rebuild and retention). Applied via `pl_cli::migrate::paperrust_migrations()`.
//! * [`sequence_once`] / [`sequencer::run`]: assigns `seq` only for pending-list rows
//!   of finished transactions (`xid < pg_snapshot_xmin`) – cursors are thereby
//!   safe with respect to visibility order.
//! * [`changes`]: raw feed query (without permission filter; `pl-api-v2` does that, SYNC.md §3.2).
//! * [`retention`], [`rebuild`], [`enable`], [`disable`], [`reset_identity`], [`status`] and
//!   [`manage::command`] (`paperrust manage sync …`).
//!
//! The crate depends on no v1 crate; all queries run against `paperrust_sync_*` or
//! the SQL functions of the migration.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub mod manage;
pub mod sequencer;

/// NOTIFY channel of the triggers (pending list has new rows) – wakes the sequencer.
pub const PENDING_CHANNEL: &str = "paperrust_sync_pending";
/// NOTIFY channel of the sequencer (feed has new entries) – for long poll (`wait=`).
pub const FEED_CHANNEL: &str = "paperrust_sync";

/// Migrationen (SQL-Dateien, expand-only, idempotent).
pub mod migrations {
    /// `migrations/0001_paperrust_sync.sql`.
    pub const M0001_SYNC: &str = include_str!("../migrations/0001_paperrust_sync.sql");
    /// Name in `paperrust_migrations` (SYNC.md §2).
    pub const NAME_0001: &str = "pl_sync.0001_paperrust_sync";
    /// `migrations/0002_paperrust_sync_sequencer_v2.sql`: sequencer with existence check per
    /// primary key instead of a join on `paperrust_sync_live` (docs/PERF-SCALE.md §9).
    pub const M0002_SEQUENCER_V2: &str = include_str!("../migrations/0002_paperrust_sync_sequencer_v2.sql");
    pub const NAME_0002: &str = "pl_sync.0002_paperrust_sync_sequencer_v2";
    /// `migrations/0003_paperrust_sync_native.sql`: capture, sequencer, rebuild and retention
    /// for both states of the data model (before/from stage 5: uuid/rev in the row instead of in
    /// `paperrust_sync_object`, docs/DATAMODEL-V2.md §1.10). Also runs as part of stage 5.
    pub const M0003_NATIVE: &str = include_str!("../migrations/0003_paperrust_sync_native.sql");
    pub const NAME_0003: &str = "pl_sync.0003_paperrust_sync_native";

    /// Alle Migrationen in Reihenfolge `(name, sql)`.
    pub const ALL: &[(&str, &str)] = &[(NAME_0001, M0001_SYNC), (NAME_0002, M0002_SEQUENCER_V2), (NAME_0003, M0003_NATIVE)];

    /// Applies all migrations (one transaction each, idempotent). Call only deliberately
    /// (operators/tests); in operation via `paperrust manage migrate`.
    pub async fn apply(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        for (_, sql) in ALL {
            let mut tx = pool.begin().await?;
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        Ok(())
    }
}

/// Object kind (stable, never renumber; SYNC.md §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(i16)]
pub enum Kind {
    Document = 1,
    Tag = 2,
    Correspondent = 3,
    DocumentType = 4,
    StoragePath = 5,
    CustomField = 6,
    SavedView = 7,
    User = 8,
    Group = 9,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Document,
        Kind::Tag,
        Kind::Correspondent,
        Kind::DocumentType,
        Kind::StoragePath,
        Kind::CustomField,
        Kind::SavedView,
        Kind::User,
        Kind::Group,
    ];

    pub fn code(self) -> i16 {
        self as i16
    }

    pub fn from_code(c: i16) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.code() == c)
    }

    /// Name in the v2 API (`kind=document,tag,…`).
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Document => "document",
            Kind::Tag => "tag",
            Kind::Correspondent => "correspondent",
            Kind::DocumentType => "document_type",
            Kind::StoragePath => "storage_path",
            Kind::CustomField => "custom_field",
            Kind::SavedView => "saved_view",
            Kind::User => "user",
            Kind::Group => "group",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// Errors of the crate.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("sync capture is not installed (run `paperrust manage migrate` or `paperrust manage sync enable`)")]
    NotInstalled,
    /// Cursor older than `min_cursor` (tombstones cleaned up / identity reset) → `410`.
    #[error("cursor expired (min_cursor {min_cursor})")]
    CursorExpired { min_cursor: i64 },
    #[error("invalid cursor: {0}")]
    InvalidCursor(String),
}

pub type Result<T, E = SyncError> = std::result::Result<T, E>;

/// Settings of the sequencer (environment variables, docs/contract/pl-sync.md).
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// `PAPERRUST_SYNC_SEQUENCER` (default on): start the sequencer in the worker.
    pub sequencer: bool,
    /// `PAPERRUST_SYNC_TOMBSTONE_DAYS` (180).
    pub tombstone_days: u32,
    /// `PAPERRUST_SYNC_INTERVAL` seconds (1): polling interval in addition to NOTIFY.
    pub interval: Duration,
    /// `PAPERRUST_SYNC_RETENTION_INTERVAL` Sekunden (3600).
    pub retention_interval: Duration,
    /// `PAPERRUST_SYNC_BATCH` (5,000): pending-list rows per sequencer transaction (short
    /// transactions, docs/PERF-SCALE.md §9).
    pub batch: i32,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            sequencer: true,
            tombstone_days: 180,
            interval: Duration::from_secs(1),
            retention_interval: Duration::from_secs(3600),
            batch: 5_000,
        }
    }
}

impl SyncConfig {
    /// From a lookup function (e.g. `|k| env.get(k).map(str::to_string)`); invalid
    /// values → default.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> SyncConfig {
        let d = SyncConfig::default();
        let secs = |k: &str, def: Duration| {
            get(k)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(Duration::from_secs_f64)
                .unwrap_or(def)
        };
        SyncConfig {
            sequencer: get("PAPERRUST_SYNC_SEQUENCER")
                .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
                .unwrap_or(d.sequencer),
            tombstone_days: get("PAPERRUST_SYNC_TOMBSTONE_DAYS").and_then(|v| v.trim().parse().ok()).unwrap_or(d.tombstone_days),
            interval: secs("PAPERRUST_SYNC_INTERVAL", d.interval),
            retention_interval: secs("PAPERRUST_SYNC_RETENTION_INTERVAL", d.retention_interval),
            batch: get("PAPERRUST_SYNC_BATCH").and_then(|v| v.trim().parse().ok()).filter(|b: &i32| *b > 0).unwrap_or(d.batch),
        }
    }

    pub fn from_env() -> SyncConfig {
        SyncConfig::from_lookup(|k| std::env::var(k).ok())
    }
}

// ------------------------------------------------------------------------------------ Verwaltung

/// Is the migration applied (tables + functions present)?
pub async fn installed(pool: &PgPool) -> Result<bool> {
    let ok: bool = sqlx::query_scalar(
        "SELECT to_regclass('paperrust_sync_log') IS NOT NULL
            AND to_regclass('paperrust_sync_meta') IS NOT NULL
            AND to_regprocedure('paperrust_sync_sequence(integer)') IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    Ok(ok)
}

async fn require_installed(pool: &PgPool) -> Result<()> {
    if installed(pool).await? { Ok(()) } else { Err(SyncError::NotInstalled) }
}

/// Ergebnis eines Sequenzer-Aufrufs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Sequenced {
    /// verarbeitete Wartelisten-Zeilen
    pub pending: u64,
    /// Runs (transactions)
    pub rounds: u32,
    /// another sequencer held the lock (nothing done)
    pub busy: bool,
}

/// Upper limit of a sequencer transaction (`statement_timeout`). A batch normally takes
/// milliseconds to a few 100 ms; if it runs longer, PostgreSQL aborts (error → back-off with
/// wait time in [`sequencer::run`]) instead of holding locks and snapshot for minutes.
pub const SEQUENCE_STATEMENT_TIMEOUT: &str = "60s";
/// `lock_timeout` per sequencer transaction (the SQL function v2 additionally sets 5 s itself).
pub const SEQUENCE_LOCK_TIMEOUT: &str = "5s";

/// Works off the pending list: one separate, short transaction per batch of `batch` rows with
/// `statement_timeout`/`lock_timeout`, until fewer than `batch` are left.
pub async fn sequence_once(pool: &PgPool, batch: i32) -> Result<Sequenced> {
    let batch = batch.max(1);
    let mut out = Sequenced::default();
    loop {
        let mut tx = pool.begin().await?;
        // set_config(..., true) = SET LOCAL; bound parameters instead of a format! temporary
        // (which made the future non-Send across the await → spawn in pl-server failed).
        sqlx::query("SELECT set_config('statement_timeout', $1, true), set_config('lock_timeout', $2, true)")
            .bind(SEQUENCE_STATEMENT_TIMEOUT)
            .bind(SEQUENCE_LOCK_TIMEOUT)
            .execute(&mut *tx)
            .await?;
        let n: i32 = sqlx::query_scalar("SELECT paperrust_sync_sequence($1)").bind(batch).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        if n < 0 {
            out.busy = true;
            return Ok(out);
        }
        out.rounds += 1;
        out.pending += n as u64;
        if n < batch {
            return Ok(out);
        }
    }
}

/// Deletes tombstones older than `days` days; sets `min_cursor`. Returns: deleted entries.
pub async fn retention(pool: &PgPool, days: u32) -> Result<u64> {
    let days = i32::try_from(days).unwrap_or(i32::MAX);
    let n: i32 = sqlx::query_scalar("SELECT paperrust_sync_retention($1)").bind(days).fetch_one(pool).await?;
    Ok(n.max(0) as u64)
}

/// Recapture the existing data completely (`manage sync rebuild`). Returns: new log entries.
pub async fn rebuild(pool: &PgPool) -> Result<u64> {
    require_installed(pool).await?;
    let n: i64 = sqlx::query_scalar("SELECT paperrust_sync_rebuild()").fetch_one(pool).await?;
    Ok(n.max(0) as u64)
}

/// Result of [`enable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Enabled {
    /// Migration was not applied and has been applied
    pub migrated: bool,
    pub triggers: i64,
    pub rebuilt: u64,
}

/// `manage sync enable`: migration (if missing), create triggers and recapture the existing data –
/// in one transaction (the trigger creation locks the tables until commit, so no
/// gap between rebuild and capture).
pub async fn enable(pool: &PgPool) -> Result<Enabled> {
    let migrated = !installed(pool).await?;
    let mut tx = pool.begin().await?;
    if migrated {
        // all sync migrations: 0003 brings the functions for the data model from stage 5
        let tracked: bool = sqlx::query_scalar("SELECT to_regclass('paperrust_migrations') IS NOT NULL").fetch_one(&mut *tx).await?;
        for (name, sql) in migrations::ALL {
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
            if tracked {
                sqlx::query("INSERT INTO paperrust_migrations (name) VALUES ($1) ON CONFLICT DO NOTHING")
                    .bind(name)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    sqlx::query("UPDATE paperrust_sync_meta SET value = 'true'::jsonb WHERE key = 'enabled'").execute(&mut *tx).await?;
    let triggers: i32 = sqlx::query_scalar("SELECT paperrust_sync_install_triggers()").fetch_one(&mut *tx).await?;
    let rebuilt: i64 = sqlx::query_scalar("SELECT paperrust_sync_rebuild()").fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Enabled { migrated, triggers: triggers as i64, rebuilt: rebuilt.max(0) as u64 })
}

/// `manage sync disable [--purge]`: remove triggers (tables stay, Django/paperless-ngx
/// keeps working unchanged). `purge` additionally removes all `paperrust_sync_*` objects and the
/// row in `paperrust_migrations` (complete rollback). Returns: removed triggers.
pub async fn disable(pool: &PgPool, purge: bool) -> Result<i64> {
    if !installed(pool).await? {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let dropped: i32 = sqlx::query_scalar("SELECT paperrust_sync_drop_triggers()").fetch_one(&mut *tx).await?;
    if purge {
        sqlx::raw_sql(PURGE_SQL).execute(&mut *tx).await?;
        let tracked: bool = sqlx::query_scalar("SELECT to_regclass('paperrust_migrations') IS NOT NULL").fetch_one(&mut *tx).await?;
        if tracked {
            for (name, _) in migrations::ALL {
                sqlx::query("DELETE FROM paperrust_migrations WHERE name = $1").bind(name).execute(&mut *tx).await?;
            }
        }
    } else {
        sqlx::query("UPDATE paperrust_sync_meta SET value = 'false'::jsonb WHERE key = 'enabled'").execute(&mut *tx).await?;
        // without triggers the feed is outdated anyway; `enable` rebuilds it
        sqlx::query("DELETE FROM paperrust_sync_pending").execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(dropped as i64)
}

const PURGE_SQL: &str = "
DROP FUNCTION IF EXISTS paperrust_sync_install_triggers();
DROP FUNCTION IF EXISTS paperrust_sync_drop_triggers();
DROP FUNCTION IF EXISTS paperrust_sync_tables();
DROP FUNCTION IF EXISTS paperrust_sync_capture_body(text);
DROP FUNCTION IF EXISTS paperrust_sync_epoch_body(text);
DROP FUNCTION IF EXISTS paperrust_sync_capture();
DROP FUNCTION IF EXISTS paperrust_sync_capture_guardian();
DROP FUNCTION IF EXISTS paperrust_sync_bump_perm_epoch();
DROP FUNCTION IF EXISTS paperrust_sync_sequence(integer);
DROP FUNCTION IF EXISTS paperrust_sync_rebuild();
DROP FUNCTION IF EXISTS paperrust_sync_retention(integer);
DROP FUNCTION IF EXISTS paperrust_sync_exists(smallint, bigint);
DROP FUNCTION IF EXISTS paperrust_sync_capture_body_native(text, text, text);
DROP FUNCTION IF EXISTS paperrust_sync_bump(smallint, bigint[]);
DROP FUNCTION IF EXISTS paperrust_sync_rev_bump();
DROP VIEW IF EXISTS paperrust_sync_live;
-- from data model stage 5 paperrust_sync_object is a read view over the identity in the
-- rows (belongs to the data model and stays), before that the side table
DO $p$
BEGIN
    IF (SELECT c.relkind FROM pg_class c WHERE c.oid = to_regclass('public.paperrust_sync_object')) = 'r' THEN
        DROP TABLE paperrust_sync_object;
    END IF;
    -- creating the identity is still needed by every reader from stage 5 (kinds 7–9); before that it belongs to the sync
    IF to_regclass('public.paperrust_sync_ident') IS NULL THEN
        DROP FUNCTION IF EXISTS paperrust_sync_ensure(smallint, bigint[]);
        DROP FUNCTION IF EXISTS paperrust_sync_ensure_needed(smallint);
    END IF;
END
$p$;
DROP TABLE IF EXISTS paperrust_sync_pending, paperrust_sync_log, paperrust_sync_meta;
DROP SEQUENCE IF EXISTS paperrust_sync_seq;
";

/// `manage sync reset-identity`: new `server_uuid`, `min_cursor` = current `seq` (all clients
/// resync; after a DB copy server ↔ desktop).
pub async fn reset_identity(pool: &PgPool) -> Result<(Uuid, i64)> {
    require_installed(pool).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('paperrust_sync_sequencer'))").execute(&mut *tx).await?;
    let id = Uuid::new_v4();
    let min: i64 = sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM paperrust_sync_log").fetch_one(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO paperrust_sync_meta (key, value) VALUES ('server_uuid', to_jsonb($1::text)), ('min_cursor', to_jsonb($2::bigint))
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(id.to_string())
    .bind(min)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((id, min))
}

/// State for `manage sync status` (and `/rs/api/v2/server`).
#[derive(Debug, Clone, Serialize, Default)]
pub struct Status {
    pub installed: bool,
    pub enabled: bool,
    pub triggers: i64,
    pub expected_triggers: i64,
    pub server_uuid: Option<String>,
    pub perm_epoch: i64,
    pub min_cursor: i64,
    pub last_seq: i64,
    pub objects: i64,
    pub log_entries: i64,
    pub tombstones: i64,
    pub pending: i64,
    /// of which from finished transactions (processable by the next sequencer run)
    pub pending_ready: i64,
}

/// Short info from `paperrust_sync_meta`.
#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub server_uuid: String,
    pub perm_epoch: i64,
    pub min_cursor: i64,
}

async fn meta<'e, E: sqlx::PgExecutor<'e>>(ex: E) -> Result<Meta> {
    let r = sqlx::query(
        "SELECT coalesce(max(value #>> '{}') FILTER (WHERE key = 'server_uuid'), ''),
                coalesce(max((value #>> '{}')::bigint) FILTER (WHERE key = 'perm_epoch'), 0),
                coalesce(max((value #>> '{}')::bigint) FILTER (WHERE key = 'min_cursor'), 0)
         FROM paperrust_sync_meta WHERE key IN ('server_uuid', 'perm_epoch', 'min_cursor')",
    )
    .fetch_one(ex)
    .await?;
    Ok(Meta { server_uuid: r.try_get(0)?, perm_epoch: r.try_get(1)?, min_cursor: r.try_get(2)? })
}

/// Highest assigned `seq` (0 = empty feed).
pub async fn last_seq(pool: &PgPool) -> Result<i64> {
    require_installed(pool).await?;
    Ok(sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM paperrust_sync_log").fetch_one(pool).await?)
}

/// `server_uuid`, `perm_epoch`, `min_cursor`.
pub async fn read_meta(pool: &PgPool) -> Result<Meta> {
    require_installed(pool).await?;
    meta(pool).await
}

pub async fn status(pool: &PgPool) -> Result<Status> {
    if !installed(pool).await? {
        return Ok(Status::default());
    }
    let m = meta(pool).await?;
    let r = sqlx::query(
        "SELECT coalesce((SELECT value = 'true'::jsonb FROM paperrust_sync_meta WHERE key = 'enabled'), true),
                (SELECT count(*) FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid
                  JOIN pg_namespace ns ON ns.oid = c.relnamespace
                  WHERE NOT t.tgisinternal AND t.tgname LIKE 'paperrust\\_sync\\_%' AND ns.nspname = current_schema()),
                (SELECT count(*) FROM paperrust_sync_tables() WHERE to_regclass(tbl) IS NOT NULL) * 3
                  + (SELECT count(*) FROM unnest(ARRAY['guardian_userobjectpermission','guardian_groupobjectpermission',
                       'auth_user_groups','auth_group_permissions','auth_user_user_permissions','paperrust_object_grant']) x
                      WHERE (SELECT c.relkind FROM pg_class c WHERE c.oid = to_regclass(x)) = 'r') * 3
                  + (CASE WHEN to_regclass('auth_user') IS NOT NULL THEN 1 ELSE 0 END)
                  -- from data model stage 5: rev trigger (BEFORE UPDATE) on the native tables
                  + (SELECT count(*) FROM unnest(ARRAY['paperrust_document','paperrust_tag','paperrust_correspondent',
                       'paperrust_document_type','paperrust_storage_path','paperrust_custom_field']) x
                      WHERE to_regclass(x) IS NOT NULL AND to_regclass('paperrust_sync_ident') IS NOT NULL),
                (SELECT coalesce(max(seq), 0) FROM paperrust_sync_log),
                (SELECT count(*) FROM paperrust_sync_object),
                (SELECT count(*) FROM paperrust_sync_log),
                (SELECT count(*) FROM paperrust_sync_log WHERE op = 'd'),
                (SELECT count(*) FROM paperrust_sync_pending),
                (SELECT count(*) FROM paperrust_sync_pending WHERE xid < pg_snapshot_xmin(pg_current_snapshot()))",
    )
    .fetch_one(pool)
    .await?;
    Ok(Status {
        installed: true,
        enabled: r.try_get(0)?,
        triggers: r.try_get(1)?,
        expected_triggers: r.try_get(2)?,
        server_uuid: Some(m.server_uuid),
        perm_epoch: m.perm_epoch,
        min_cursor: m.min_cursor,
        last_seq: r.try_get(3)?,
        objects: r.try_get(4)?,
        log_entries: r.try_get(5)?,
        tombstones: r.try_get(6)?,
        pending: r.try_get(7)?,
        pending_ready: r.try_get(8)?,
    })
}

// ------------------------------------------------------------------------------------ Feed

/// Format of the opaque cursor (`c1.<seq>`, SYNC.md §3.2).
pub fn format_cursor(seq: i64) -> String {
    format!("c1.{seq}")
}

/// Liest `c1.<seq>`.
pub fn parse_cursor(s: &str) -> Result<i64> {
    s.strip_prefix("c1.").and_then(|n| n.parse::<i64>().ok()).filter(|n| *n >= 0).ok_or_else(|| SyncError::InvalidCursor(s.to_string()))
}

/// `u` = upsert, `d` = delete (Tombstone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Upsert,
    Delete,
}

/// A feed entry (without permission filter).
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub seq: i64,
    pub kind: Kind,
    pub local_id: i64,
    pub uuid: Uuid,
    pub op: Op,
    pub rev: i64,
    /// `seq` of the last permission change (owner/guardian), `None` = none known.
    pub perm_seq: Option<i64>,
    pub at: DateTime<Utc>,
}

impl Change {
    /// Permission change after `cursor`: an entry invisible to the user is then reported as
    /// `delete` (revocation), otherwise left out (SYNC.md §3.2).
    pub fn perm_changed_after(&self, cursor: i64) -> bool {
        self.perm_seq.is_some_and(|p| p > cursor)
    }
}

/// A page of the feed.
#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub changes: Vec<Change>,
    /// last **read** `seq` (even if everything was filtered)
    pub next_cursor: i64,
    pub has_more: bool,
    pub perm_epoch: i64,
    pub server_uuid: String,
    pub min_cursor: i64,
    /// highest assigned `seq` in the same snapshot (`as_of_seq`)
    pub last_seq: i64,
}

/// Feed after `after` (exclusive; `None` = complete state), at most `limit` entries (1–1000),
/// optionally only certain kinds. Reads in one snapshot (REPEATABLE READ). Returns
/// [`SyncError::CursorExpired`] for `after < min_cursor`.
pub async fn changes(pool: &PgPool, after: Option<i64>, limit: i64, kinds: Option<&[Kind]>) -> Result<Page> {
    require_installed(pool).await?;
    let limit = limit.clamp(1, 1000);
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY").execute(&mut *tx).await?;
    let m = meta(&mut *tx).await?;
    if let Some(c) = after
        && c < m.min_cursor
    {
        return Err(SyncError::CursorExpired { min_cursor: m.min_cursor });
    }
    let cursor = after.unwrap_or(0);
    let codes: Option<Vec<i16>> = kinds.map(|ks| ks.iter().map(|k| k.code()).collect());
    let rows = sqlx::query(
        "SELECT seq, kind, local_id, uuid, op, rev, perm_seq, at FROM paperrust_sync_log
         WHERE seq > $1 AND ($2::smallint[] IS NULL OR kind = ANY($2))
         ORDER BY seq LIMIT $3",
    )
    .bind(cursor)
    .bind(codes)
    .bind(limit + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let code: i16 = r.try_get(1)?;
        let Some(kind) = Kind::from_code(code) else { continue };
        let op: String = r.try_get(4)?;
        out.push(Change {
            seq: r.try_get(0)?,
            kind,
            local_id: r.try_get(2)?,
            uuid: r.try_get(3)?,
            op: if op == "d" { Op::Delete } else { Op::Upsert },
            rev: r.try_get(5)?,
            perm_seq: r.try_get(6)?,
            at: r.try_get(7)?,
        });
    }
    let has_more = out.len() as i64 > limit;
    let last_seq: i64 = sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM paperrust_sync_log").fetch_one(&mut *tx).await?;
    let next_cursor = if has_more {
        out.truncate(limit as usize);
        out.last().map(|c| c.seq).unwrap_or(cursor)
    } else {
        // read everything up to the current end: seq values only become visible in commit order
        // (one sequencer at a time), hence the maximum is safe.
        last_seq.max(cursor)
    };
    tx.commit().await?;
    Ok(Page {
        changes: out,
        next_cursor,
        has_more,
        perm_epoch: m.perm_epoch,
        server_uuid: m.server_uuid,
        min_cursor: m.min_cursor,
        last_seq,
    })
}

/// Long-poll helper: waits at most `timeout` until the feed has an entry `> after`.
/// Returns `true` if new entries are present.
pub async fn wait_for_changes(pool: &PgPool, after: i64, timeout: Duration) -> Result<bool> {
    let mut listener = sqlx::postgres::PgListener::connect_with(pool).await?;
    listener.listen(FEED_CHANNEL).await?;
    // FINDINGS B2: the check query runs over the listener's own connection, not over a
    // second connection from `pool` (otherwise every waiting client would hold one connection and wait
    // for a second – with as many clients as pool connections everything would stall until the time limit).
    async fn has(listener: &mut sqlx::postgres::PgListener, after: i64) -> Result<bool> {
        let max: i64 = sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM paperrust_sync_log").fetch_one(&mut *listener).await?;
        Ok(max > after)
    }
    if has(&mut listener, after).await? {
        return Ok(true);
    }
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::time::timeout_at(deadline, listener.recv()).await {
            Err(_) => return has(&mut listener, after).await,
            Ok(Err(e)) => return Err(e.into()),
            Ok(Ok(_)) => {
                if has(&mut listener, after).await? {
                    return Ok(true);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_roundtrip() {
        for k in Kind::ALL {
            assert_eq!(Kind::from_code(k.code()), Some(k));
            assert_eq!(Kind::parse(k.as_str()), Some(k));
        }
        assert_eq!(Kind::Document.code(), 1);
        assert_eq!(Kind::Group.code(), 9);
        assert_eq!(Kind::from_code(10), None);
    }

    #[test]
    fn cursor_format() {
        assert_eq!(format_cursor(1043), "c1.1043");
        assert_eq!(parse_cursor("c1.1043").unwrap(), 1043);
        assert!(parse_cursor("c2.1").is_err());
        assert!(parse_cursor("c1.-1").is_err());
        assert!(parse_cursor("1043").is_err());
    }

    #[test]
    fn config_from_lookup() {
        let c = SyncConfig::from_lookup(|k| match k {
            "PAPERRUST_SYNC_SEQUENCER" => Some("off".into()),
            "PAPERRUST_SYNC_TOMBSTONE_DAYS" => Some("30".into()),
            "PAPERRUST_SYNC_INTERVAL" => Some("0.25".into()),
            "PAPERRUST_SYNC_BATCH" => Some("-5".into()),
            _ => None,
        });
        assert!(!c.sequencer);
        assert_eq!(c.tombstone_days, 30);
        assert_eq!(c.interval, Duration::from_millis(250));
        assert_eq!(c.batch, 5_000);
        assert_eq!(c.retention_interval, Duration::from_secs(3600));
        let d = SyncConfig::from_lookup(|_| None);
        assert!(d.sequencer);
    }

    #[test]
    fn perm_changed_after() {
        let c = Change {
            seq: 10,
            kind: Kind::Document,
            local_id: 1,
            uuid: Uuid::nil(),
            op: Op::Upsert,
            rev: 1,
            perm_seq: Some(7),
            at: Utc::now(),
        };
        assert!(c.perm_changed_after(6));
        assert!(!c.perm_changed_after(7));
        assert!(!Change { perm_seq: None, ..c }.perm_changed_after(0));
    }

    #[test]
    fn migration_is_expand_only() {
        // no change to Django tables: no ALTER TABLE, no BEFORE trigger
        let sql = migrations::M0001_SYNC.to_ascii_uppercase();
        for line in sql.lines().filter(|l| l.contains("ALTER TABLE")) {
            assert!(line.trim_start().starts_with("ALTER TABLE PAPERRUST_"), "{line}");
        }
        assert!(!sql.contains("BEFORE INSERT") && !sql.contains("BEFORE UPDATE"));
        for line in sql.lines().filter(|l| l.trim_start().starts_with("CREATE TABLE")) {
            assert!(line.contains("PAPERRUST_"), "{line}");
        }
    }
}
