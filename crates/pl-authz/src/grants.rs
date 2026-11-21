//! L5 (ARCHITECTURE-V2 §5): grant index `paperrust_grant`. The path is no longer configurable
//! (switch `PAPERRUST_PERMISSION_ENGINE` removed since data model stage 7): with a ready index
//! or native data model always `grants`, otherwise – only on databases without PaperRust migrations,
//! e.g. fresh test copies – the guardian tables directly.
//!
//! `paperrust_grant(perm_id, subject, object_id, ct)` is a derived index of the guardian object
//! permissions maintained by triggers (migration `pl_authz.0002_paperrust_grant`): `subject` > 0 =
//! user, < 0 = group (`-group_id`), `object_id` = canonical integer of the `object_pk`, `ct` = the
//! `content_type_id` of the guardian row. The Django tables remain the truth; owner,
//! superuser/active and groups are read per request as before.
//!
//! Read paths with `grants` (same sets as upstream, proof in the contract "L5"):
//! * List/visibility: `key IN (SELECT object_id FROM paperrust_grant WHERE perm_id = ANY(..)
//!   AND subject = ANY(..))` – bigint instead of `CAST(id AS varchar) IN (object_pk …)`, an index-only
//!   scan over the primary key instead of two guardian scans with an `auth_permission` join.
//! * Single check (`ObjectPermissionChecker`): additionally `ct = <model>` (guardian also filters
//!   the row's content type column there).
//!
//! The index is used only when it is **ready** (table present, `enabled`, `built`, all
//! 8 triggers active); otherwise every call falls back to `legacy`. The state is checked by
//! [`refresh`] (server: background task every [`REFRESH_SECS`] s, started by `Authz::new`).
//! Contract: docs/contract/pl-authz.md "L5".

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::models::ObjectModel;
use crate::objects::{bare_codename, content_type_id_subquery};
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query, SelectStatement, SimpleExpr};
use sqlx::PgPool;

/// Table of the index.
pub const TABLE: &str = "paperrust_grant";
/// Readiness check interval (server).
pub const REFRESH_SECS: u64 = 5;
/// Number of triggers (2 guardian tables × ins/upd/del/trunc).
pub const EXPECTED_TRIGGERS: i64 = 8;

/// Permission read path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// guardian tables directly (as upstream)
    Legacy,
    /// Grant index `paperrust_grant`
    Grants,
}

/// Path when the index is ready (differential test and parity runs without deviation, docs/contract/pl-authz.md
/// "L5"). Without a ready index `legacy` applies ([`active`]).
pub const DEFAULT_ENGINE: Engine = Engine::Grants;

/// Configured path – fixed [`DEFAULT_ENGINE`] (formerly `PAPERRUST_PERMISSION_ENGINE`).
pub fn configured() -> Engine {
    DEFAULT_ENGINE
}

static READY: AtomicBool = AtomicBool::new(false);
/// 0 = no override, 1 = legacy, 2 = grants (tests/tools).
static OVERRIDE: AtomicU8 = AtomicU8::new(0);
static REFRESHER: std::sync::Once = std::sync::Once::new();

/// Overrides readiness (differential tests only). `None` = off.
pub fn set_override(e: Option<Engine>) {
    OVERRIDE.store(
        match e {
            None => 0,
            Some(Engine::Legacy) => 1,
            Some(Engine::Grants) => 2,
        },
        Ordering::SeqCst,
    );
}

/// Was the index last detected as ready?
pub fn is_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// Path actually used: `grants` only if configured **and** ready.
pub fn active() -> Engine {
    match OVERRIDE.load(Ordering::Relaxed) {
        1 => return Engine::Legacy,
        2 => return Engine::Grants,
        _ => {}
    }
    if configured() == Engine::Grants && is_ready() { Engine::Grants } else { Engine::Legacy }
}

/// Data model stage 4 detected (set by the background refresher): grants are stored natively in
/// `paperrust_object_grant`; the SQL building blocks then read there directly (through the union view
/// `paperrust_grant` the planner does not reliably find the index paths).
static NATIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// See [`NATIVE`]; tests/tools can set it.
pub fn set_native(v: bool) {
    NATIVE.store(v, Ordering::SeqCst);
}

pub fn native() -> bool {
    NATIVE.load(Ordering::Relaxed)
}

/// Table for writers that change only columns of the narrow document row (everything except `content`,
/// file names and checksums): `paperrust_document` as soon as the own data model is detected
/// (stage 4 presupposes stage 1), otherwise the Django name. Through the view `documents_document` every
/// row costs an INSTEAD-OF trigger with its own statement (10,000 documents 6.8 s instead of 2.5 s,
/// docs/DATAMODEL-V2.md §6 "Stage 6"). As long as the refresher does not know the state, the
/// Django name applies – it is always correct.
pub fn document_row_table() -> &'static str {
    if native() { "paperrust_document" } else { "documents_document" }
}

/// (kind, bit) of a permission in the native grid (`paperrust_grant_perm`), `None` outside it.
fn native_kind_bit(model: &ObjectModel, perm: &str) -> Option<(i16, i16)> {
    let kind = native_kind(model)?;
    let code = bare_codename(perm);
    let verb = code.strip_suffix(&format!("_{}", model.model_name))?;
    let bit = match verb {
        "view" => 1,
        "change" => 2,
        "delete" => 4,
        "add" => 8,
        _ => return None,
    };
    Some((kind, bit))
}

/// Kind of a model in the native grid (numbers as in SYNC.md), `None` outside it.
fn native_kind(model: &ObjectModel) -> Option<i16> {
    if model.app_label != "documents" {
        return None;
    }
    Some(match model.model_name {
        "document" => 1,
        "tag" => 2,
        "correspondent" => 3,
        "documenttype" => 4,
        "storagepath" => 5,
        "customfield" => 6,
        "savedview" => 7,
        _ => return None,
    })
}

/// `true` if the index is used.
pub fn use_grants() -> bool {
    active() == Engine::Grants
}

/// State of the index.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Status {
    /// Data model stage 4: grants are the truth (`paperrust_object_grant`), `paperrust_grant`
    /// is a view of them – always current, no maintenance triggers.
    #[serde(default)]
    pub native: bool,
    pub installed: bool,
    pub enabled: bool,
    pub built: bool,
    pub triggers: i64,
    pub rows: i64,
}

impl Status {
    pub fn ready(&self) -> bool {
        self.native || (self.installed && self.enabled && self.built && self.triggers == EXPECTED_TRIGGERS)
    }
}

/// Read the state from the DB.
pub async fn status(db: &PgPool) -> Result<Status, sqlx::Error> {
    let (t, m): (bool, bool) =
        sqlx::query_as("SELECT to_regclass('paperrust_grant') IS NOT NULL, to_regclass('paperrust_grant_meta') IS NOT NULL")
            .fetch_one(db)
            .await?;
    let native: bool = sqlx::query_scalar("SELECT to_regclass('paperrust_object_grant') IS NOT NULL").fetch_one(db).await?;
    if native {
        let rows: i64 =
            sqlx::query_scalar("SELECT COALESCE(reltuples, 0)::int8 FROM pg_class WHERE oid = 'paperrust_object_grant'::regclass")
                .fetch_one(db)
                .await?;
        return Ok(Status { native: true, installed: true, enabled: true, built: true, triggers: EXPECTED_TRIGGERS, rows });
    }
    if !(t && m) {
        return Ok(Status::default());
    }
    let (enabled, built, triggers, rows): (bool, bool, i64, i64) = sqlx::query_as(
        "SELECT COALESCE((SELECT value = 'true'::jsonb FROM paperrust_grant_meta WHERE key = 'enabled'), false),
                COALESCE((SELECT value = 'true'::jsonb FROM paperrust_grant_meta WHERE key = 'built'), false),
                (SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgenabled <> 'D'
                    AND tgname IN ('paperrust_grant_ins', 'paperrust_grant_upd', 'paperrust_grant_del', 'paperrust_grant_trunc')
                    AND tgrelid IN ('guardian_userobjectpermission'::regclass, 'guardian_groupobjectpermission'::regclass)),
                (SELECT COALESCE(reltuples, 0)::int8 FROM pg_class WHERE oid = 'paperrust_grant'::regclass)",
    )
    .fetch_one(db)
    .await?;
    Ok(Status { native: false, installed: true, enabled, built, triggers, rows })
}

/// Re-determine readiness. A connection error does not change the state (without a DB every
/// read query fails anyway); only a status that was actually read counts.
pub async fn refresh(db: &PgPool) -> bool {
    let ready = match status(db).await {
        Ok(s) => {
            NATIVE.store(s.native, Ordering::SeqCst);
            s.ready()
        }
        Err(e) => {
            tracing::debug!("paperrust_grant status: {e}");
            return is_ready();
        }
    };
    let was = READY.swap(ready, Ordering::SeqCst);
    if was != ready {
        tracing::info!(ready, "permission index paperrust_grant");
    }
    ready
}

/// Starts (once per process) the background check if a tokio runtime is running.
pub fn spawn_refresher(db: PgPool) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    REFRESHER.call_once(|| {
        handle.spawn(async move {
            loop {
                refresh(&db).await;
                tokio::time::sleep(std::time::Duration::from_secs(REFRESH_SECS)).await;
            }
        });
    });
}

// ------------------------------------------------------------------------------ SQL building blocks

/// `perm_id = ANY(ARRAY(<auth_permission.id with codename and model content type>))` –
/// the array is an InitPlan (once per query), after that a pure index condition.
pub fn perm_cond(alias: &str, model: &ObjectModel, perm: &str) -> String {
    let ids = Query::select()
        .column(Alias::new("id"))
        .from(Alias::new("auth_permission"))
        .and_where(Expr::col(Alias::new("codename")).eq(bare_codename(perm)))
        .and_where(Expr::col(Alias::new("content_type_id")).in_subquery(content_type_id_subquery(model)))
        .to_string(PostgresQueryBuilder);
    format!("{alias}.perm_id = ANY(ARRAY({ids}))")
}

/// Subjects of a user as an SQL expression (`uid` = SQL expression of the user ID, e.g. `7` or `$1`):
/// the user themselves and `-group_id` of their groups (`auth_user_groups`, as upstream per query).
pub fn subjects_sql(uid: &str) -> String {
    format!("(ARRAY(SELECT -ug.group_id::int8 FROM auth_user_groups ug WHERE ug.user_id = {uid}) || ({uid})::int8)")
}

/// `SELECT object_id FROM paperrust_grant WHERE <perm> AND subject = ANY(<subjects>)`:
/// object IDs on which `user_id` (directly or via a group) has `perm` (list rule: model
/// via the permission, the row's `ct` column does not count – like `permitted_object_ids`).
pub fn granted_ids_subquery(user_id: i64, model: &ObjectModel, perm: &str) -> SelectStatement {
    if native()
        && let Some((kind, bit)) = native_kind_bit(model, perm)
    {
        let subj = subjects_sql(&user_id.to_string());
        let mut raw = Query::select();
        raw.expr(Expr::cust("paperrust_grant_key(pg_r.object_pk)"))
            .from_as(Alias::new("paperrust_grant_raw"), Alias::new("pg_r"))
            .and_where(Expr::cust(perm_cond("pg_r", model, perm).replacen("pg_r.perm_id", "pg_r.permission_id", 1)))
            .and_where(Expr::cust(format!("pg_r.subject = ANY{subj}")))
            .and_where(Expr::cust("paperrust_grant_key(pg_r.object_pk) IS NOT NULL"));
        return Query::select()
            .expr(Expr::cust("pg_g.object_id"))
            .from_as(Alias::new("paperrust_object_grant"), Alias::new("pg_g"))
            .and_where(Expr::cust(format!("pg_g.object_kind = {kind} AND pg_g.subject = ANY{subj} AND pg_g.perms & {bit} <> 0")))
            .union(sea_query::UnionType::All, raw)
            .to_owned();
    }
    Query::select()
        .column((Alias::new("pg_g"), Alias::new("object_id")))
        .from_as(Alias::new(TABLE), Alias::new("pg_g"))
        .and_where(Expr::cust(perm_cond("pg_g", model, perm)))
        .and_where(Expr::cust(format!("pg_g.subject = ANY{}", subjects_sql(&user_id.to_string()))))
        .to_owned()
}

/// Like [`granted_ids_subquery`], as SQL text with a freely chosen user expression (`$1`) and
/// permission condition (for crates with hand-written SQL).
pub fn granted_ids_sql(uid: &str, model: &ObjectModel, perm: &str) -> String {
    if native()
        && let Some((kind, bit)) = native_kind_bit(model, perm)
    {
        let subj = subjects_sql(uid);
        return format!(
            "SELECT pg_g.object_id FROM paperrust_object_grant pg_g WHERE pg_g.object_kind = {kind}              AND pg_g.subject = ANY{subj} AND pg_g.perms & {bit} <> 0              UNION ALL SELECT paperrust_grant_key(pg_r.object_pk) FROM paperrust_grant_raw pg_r WHERE {}              AND pg_r.subject = ANY{subj} AND paperrust_grant_key(pg_r.object_pk) IS NOT NULL",
            perm_cond("pg_r", model, perm).replacen("pg_r.perm_id", "pg_r.permission_id", 1)
        );
    }
    format!("SELECT pg_g.object_id FROM {TABLE} pg_g WHERE {} AND pg_g.subject = ANY{}", perm_cond("pg_g", model, perm), subjects_sql(uid))
}

/// guardian `ObjectPermissionChecker.has_perm(perm, obj)` without the superuser/active rule:
/// `EXISTS` of a grant of `perm` on `id_sql` with row content type = model.
pub fn checker_exists_sql(uid: &str, model: &ObjectModel, perm: &str, id_sql: &str) -> String {
    let ct = content_type_id_subquery(model).to_string(PostgresQueryBuilder);
    if native()
        && let Some((kind, bit)) = native_kind_bit(model, perm)
    {
        let subj = subjects_sql(uid);
        return format!(
            "(EXISTS (SELECT 1 FROM paperrust_object_grant pg_c WHERE pg_c.object_kind = {kind} AND pg_c.object_id = {id_sql}               AND pg_c.subject = ANY{subj} AND pg_c.perms & {bit} <> 0)              OR EXISTS (SELECT 1 FROM paperrust_grant_raw pg_r WHERE paperrust_grant_key(pg_r.object_pk) = {id_sql}               AND pg_r.content_type_id IN ({ct}) AND {} AND pg_r.subject = ANY{subj}))",
            perm_cond("pg_r", model, perm).replacen("pg_r.perm_id", "pg_r.permission_id", 1)
        );
    }
    format!(
        "EXISTS (SELECT 1 FROM {TABLE} pg_c WHERE pg_c.object_id = {id_sql} AND pg_c.ct IN ({ct}) AND {} \
         AND pg_c.subject = ANY{})",
        perm_cond("pg_c", model, perm),
        subjects_sql(uid)
    )
}

/// `EXISTS` of any grant (user or group, any permission) on `id_expr` with
/// row content type = model (`shared_by__id`, `is_shared_by_requester`).
pub fn any_grant_exists(model: &ObjectModel, id_expr: SimpleExpr) -> SimpleExpr {
    let ct = content_type_id_subquery(model).to_string(PostgresQueryBuilder);
    if native()
        && let Some(kind) = native_kind(model)
    {
        return Expr::cust_with_exprs(
            format!(
                "(EXISTS (SELECT 1 FROM paperrust_object_grant pg_s WHERE pg_s.object_kind = {kind} AND pg_s.object_id = $1)                  OR EXISTS (SELECT 1 FROM paperrust_grant_raw pg_r WHERE paperrust_grant_key(pg_r.object_pk) = $2                  AND pg_r.content_type_id IN ({ct})))"
            ),
            [id_expr.clone(), id_expr],
        );
    }
    Expr::cust_with_exprs(format!("EXISTS (SELECT 1 FROM {TABLE} pg_s WHERE pg_s.object_id = $1 AND pg_s.ct IN ({ct}))"), [id_expr])
}

// ------------------------------------------------------------------------------ Administration

/// Migrations (name, SQL) of this stage.
pub mod migrations {
    pub const M0002_GRANT: &str = include_str!("../migrations/0002_paperrust_grant.sql");
    pub const NAME_0002: &str = "pl_authz.0002_paperrust_grant";
    pub const ALL: &[(&str, &str)] = &[(NAME_0002, M0002_GRANT)];

    /// Apply directly (idempotent; tests/tools).
    pub async fn apply(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        sqlx::raw_sql(M0002_GRANT).execute(pool).await?;
        Ok(())
    }
}

/// Result of the consistency check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Check {
    /// guardian rows without an index row
    pub missing: i64,
    /// index rows without a guardian row (or with a different `ct`)
    pub extra: i64,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.missing == 0 && self.extra == 0
    }
}

/// Check the index against a full recomputation from guardian.
pub async fn check(db: &PgPool) -> Result<Check, sqlx::Error> {
    let (missing, extra): (i64, i64) = sqlx::query_as("SELECT missing, extra FROM paperrust_grant_check()").fetch_one(db).await?;
    Ok(Check { missing, extra })
}

/// Rebuild the index (one transaction, guardian writers wait meanwhile). Returns: rows.
pub async fn rebuild(db: &PgPool) -> Result<i64, sqlx::Error> {
    if status(db).await?.native {
        // nothing to derive: paperrust_grant is a view of the truth
        return sqlx::query_scalar("SELECT count(*) FROM paperrust_grant").fetch_one(db).await;
    }
    let mut tx = db.begin().await?;
    let (n,): (i64,) = sqlx::query_as("SELECT paperrust_grant_rebuild()").fetch_one(&mut *tx).await?;
    tx.commit().await?;
    vacuum(db).await;
    Ok(n)
}

/// `VACUUM ANALYZE paperrust_grant` after a rebuild: sets the visibility map, otherwise
/// PostgreSQL plans a seq scan instead of index-only scans until the next autovacuum (L5 measurement).
/// Errors (e.g. missing privileges) are not fatal.
pub async fn vacuum(db: &PgPool) {
    let native = status(db).await.map(|s| s.native).unwrap_or(false);
    let sql = if native { "VACUUM (ANALYZE) paperrust_object_grant" } else { "VACUUM (ANALYZE) paperrust_grant" };
    if let Err(e) = sqlx::raw_sql(sql).execute(db).await {
        tracing::warn!("VACUUM paperrust_grant: {e}");
    }
}

pub const HELP: &str = "usage: paperrust manage grants <subcommand>

  status [--json]     state of the permission index paperrust_grant (L5)
  check               compare the index with a full recompute from guardian (exit 1 on difference)
  rebuild             rebuild the index from guardian (blocks guardian writers meanwhile)
  enable              install migration/triggers, rebuild, mark ready
  disable [--purge]   stop reading the index (triggers keep it up to date);
                      --purge also drops triggers, tables and functions
";

/// Error of a `manage grants` call.
#[derive(Debug)]
pub enum ManageError {
    Usage(String),
    /// check with deviation (output, exit 1)
    Mismatch(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for ManageError {
    fn from(e: sqlx::Error) -> Self {
        ManageError::Db(e)
    }
}

/// `paperrust manage grants …` (argv without `grants`). Returns: output for stdout.
pub async fn command(db: &PgPool, argv: &[String]) -> Result<String, ManageError> {
    let Some(sub) = argv.first().map(String::as_str) else {
        return Err(ManageError::Usage(HELP.trim_end().to_string()));
    };
    let rest = &argv[1..];
    let only = |allowed: &[&str]| -> Result<(), ManageError> {
        match rest.iter().find(|a| !allowed.contains(&a.as_str())) {
            Some(a) => Err(ManageError::Usage(format!("unrecognized argument: {a}"))),
            None => Ok(()),
        }
    };
    match sub {
        "help" | "--help" | "-h" => Ok(HELP.to_string()),
        "status" => {
            only(&["--json"])?;
            let s = status(db).await?;
            if rest.iter().any(|a| a == "--json") {
                return Ok(format!("{}\n", serde_json::to_string_pretty(&s).unwrap_or_default()));
            }
            if s.native {
                return Ok(format!(
                    "permission index: native (data model stage 4: paperrust_object_grant is the truth, paperrust_grant a view)\n  rows:     ~{}\n  engine:   {} (configured {})\n",
                    s.rows,
                    if configured() == Engine::Grants { "grants" } else { "legacy" },
                    match configured() {
                        Engine::Grants => "grants",
                        Engine::Legacy => "legacy",
                    }
                ));
            }
            if !s.installed {
                return Ok("permission index: not installed (paperrust manage migrate / manage grants enable)\n".into());
            }
            Ok(format!(
                "permission index: {}\n  enabled:  {}\n  built:    {}\n  triggers: {}/{}\n  rows:     ~{}\n  engine:   {} (configured {})\n",
                if s.ready() { "ready" } else { "not ready" },
                s.enabled,
                s.built,
                s.triggers,
                EXPECTED_TRIGGERS,
                s.rows,
                if s.ready() && configured() == Engine::Grants { "grants" } else { "legacy" },
                match configured() {
                    Engine::Grants => "grants",
                    Engine::Legacy => "legacy",
                }
            ))
        }
        "check" => {
            only(&[])?;
            let c = check(db).await?;
            let line = format!("paperrust_grant vs guardian: {} missing, {} extra\n", c.missing, c.extra);
            if c.ok() { Ok(line) } else { Err(ManageError::Mismatch(line)) }
        }
        "rebuild" => {
            only(&[])?;
            let n = rebuild(db).await?;
            Ok(format!("paperrust_grant rebuilt: {n} rows\n"))
        }
        "enable" => {
            only(&[])?;
            if status(db).await?.native {
                return Ok("permission index: native since data model stage 4, nothing to enable\n".into());
            }
            migrations::apply(db).await?;
            let mut tx = db.begin().await?;
            sqlx::query("UPDATE paperrust_grant_meta SET value = 'true'::jsonb WHERE key = 'enabled'").execute(&mut *tx).await?;
            let (t,): (i32,) = sqlx::query_as("SELECT paperrust_grant_install()").fetch_one(&mut *tx).await?;
            let (n,): (i64,) = sqlx::query_as("SELECT paperrust_grant_rebuild()").fetch_one(&mut *tx).await?;
            tx.commit().await?;
            vacuum(db).await;
            Ok(format!("permission index enabled: {t} triggers, {n} rows\n"))
        }
        "disable" => {
            only(&["--purge"])?;
            let purge = rest.iter().any(|a| a == "--purge");
            if !status(db).await?.installed {
                return Ok("permission index: not installed\n".into());
            }
            sqlx::query("UPDATE paperrust_grant_meta SET value = 'false'::jsonb WHERE key = 'enabled'").execute(db).await?;
            if !purge {
                return Ok("permission index disabled (reads use guardian; triggers keep the index current)\n".into());
            }
            // Running servers detect "not ready" after REFRESH_SECS at the latest; only then delete.
            tokio::time::sleep(std::time::Duration::from_secs(REFRESH_SECS + 1)).await;
            sqlx::raw_sql(
                "SELECT paperrust_grant_drop_triggers();
                 DROP TABLE IF EXISTS paperrust_grant, paperrust_grant_meta;
                 DROP FUNCTION IF EXISTS paperrust_grant_expected(), paperrust_grant_rebuild(),
                     paperrust_grant_check(), paperrust_grant_install(), paperrust_grant_drop_triggers(),
                     paperrust_grant_user_ins(), paperrust_grant_user_upd(), paperrust_grant_user_del(),
                     paperrust_grant_user_trunc(), paperrust_grant_group_ins(), paperrust_grant_group_upd(),
                     paperrust_grant_group_del(), paperrust_grant_group_trunc(), paperrust_grant_key(text);
                 DO $$ BEGIN IF to_regclass('paperrust_migrations') IS NOT NULL THEN DELETE FROM paperrust_migrations WHERE name = 'pl_authz.0002_paperrust_grant'; END IF; END $$;",
            )
            .execute(db)
            .await?;
            Ok("permission index purged\n".into())
        }
        other => Err(ManageError::Usage(format!("unknown subcommand: {other}\n{}", HELP.trim_end()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DOCUMENT, TAG};

    #[test]
    fn sql_shapes() {
        let s = granted_ids_subquery(7, &TAG, "documents.view_tag").to_string(PostgresQueryBuilder);
        assert!(s.contains(r#"FROM "paperrust_grant" AS "pg_g""#), "{s}");
        assert!(s.contains("pg_g.perm_id = ANY(ARRAY(SELECT \"id\" FROM \"auth_permission\""), "{s}");
        assert!(s.contains("\"codename\" = 'view_tag'"), "{s}");
        assert!(s.contains("WHERE ug.user_id = 7) || (7)::int8)"), "{s}");
        let c = checker_exists_sql("$3", &DOCUMENT, "change_document", "$4");
        assert!(c.contains("pg_c.object_id = $4 AND pg_c.ct IN (SELECT"), "{c}");
        assert!(c.contains("ug.user_id = $3"), "{c}");
    }
}
