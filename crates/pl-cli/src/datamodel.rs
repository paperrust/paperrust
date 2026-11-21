//! `manage datamodel …` and migration framework for the conversion stages of the own data model
//! (L6, docs/DATAMODEL-V2.md §2.2 stage 0). Stage register: [`pl_store::datamodel`].
//!
//! * **Backup first:** before `manage migrate` applies a conversion stage to a database with
//!   documents, it backs it up with `pg_dump -Fc` to
//!   `<PAPERLESS_DATA_DIR>/backups/paperrust-vor-<stufe>-<zeit>.pgdump` ([`backup_before`]).
//!   Without `pg_dump`, `migrate` aborts with a clear message before anything is changed
//!   (test log 2026-11-16 D-073; the Docker image ships `postgresql-client`). Whoever
//!   deliberately wants to continue without a backup: `PAPERRUST_MIGRATE_WITHOUT_BACKUP=true` – then
//!   only a warning, rollback ([`rollback`]) remains possible. Disable completely (already backed
//!   up yourself): `--no-backup` or `PAPERRUST_DATAMODEL_BACKUP=off`.
//! * **Rollback:** `datamodel rollback --to <n>` applies the `down` scripts of stages > n in
//!   reverse order (one transaction per stage) and removes their rows from
//!   `paperrust_migrations`. Afterwards the database is back in the state before the stage; a
//!   further `migrate` converts again.
//! * **Check:** `datamodel check` compares the compatibility views column by column
//!   (name, type, length, order) with the embedded v3.4.2 schema
//!   (`pl-dbmigrate/schema/v3.4.2-columns.tsv`) – risk "view is not 100 % identical" (§4.2).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pl_store::datamodel::{self, STAGES};
use sqlx::{Connection, PgConnection, Row};

use crate::args::{self, flag};
use crate::baseline;
use crate::ctx;
use crate::{CliError, CliResult};

/// Applied conversion stages (names from `paperrust_migrations`).
pub async fn applied(conn: &mut PgConnection) -> CliResult<BTreeSet<String>> {
    let has: bool = sqlx::query_scalar("SELECT to_regclass('public.paperrust_migrations') IS NOT NULL").fetch_one(&mut *conn).await?;
    if !has {
        return Ok(BTreeSet::new());
    }
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM paperrust_migrations").fetch_all(&mut *conn).await?;
    Ok(names.into_iter().filter(|n| datamodel::is_stage(n)).collect())
}

/// Is the automatic backup disabled (`PAPERRUST_DATAMODEL_BACKUP=off|no|false|0`)?
pub fn backup_disabled(env: &pl_core::config::Env) -> bool {
    env.get("PAPERRUST_DATAMODEL_BACKUP").is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "no" | "false" | "0"))
}

/// Find `pg_dump`: `PAPERRUST_PG_BIN`, next to the own executable, `PATH` (like
/// pl-dbmigrate `assist::find_pg_dump`).
pub fn find_pg_dump(env: &pl_core::config::Env) -> Option<PathBuf> {
    let name = format!("pg_dump{}", std::env::consts::EXE_SUFFIX);
    let mut dirs: Vec<PathBuf> = Vec::new();
    for k in ["PAPERRUST_PG_BIN", "PAPERRUST_DESKTOP_PG_BIN"] {
        if let Some(d) = env.get(k).filter(|s| !s.is_empty()) {
            dirs.push(PathBuf::from(d));
        }
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        dirs.push(exe_dir.join("pgsql").join("bin"));
        dirs.push(exe_dir);
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter().map(|d| d.join(&name)).find(|p| p.is_file())
}

/// `PAPERRUST_MIGRATE_WITHOUT_BACKUP` (bool, default false): conversion stages even without `pg_dump`.
pub fn migrate_without_backup(env: &pl_core::config::Env) -> CliResult<bool> {
    env.bool("PAPERRUST_MIGRATE_WITHOUT_BACKUP", "false").map_err(|e| CliError::Command(format!("configuration error: {e}")))
}

/// Abort message when no `pg_dump` is available before a conversion stage (D-073).
pub fn no_pg_dump_message(stage: &str) -> String {
    format!(
        "Backup before data model stage {stage} is not possible: pg_dump was not found \
         (PAPERRUST_PG_BIN, next to paperrust, PATH). Nothing was changed. Install the PostgreSQL \
         client tools (same major version as the server) or set PAPERRUST_PG_BIN; if you have \
         backed up the database yourself, run `migrate --no-backup` or set \
         PAPERRUST_MIGRATE_WITHOUT_BACKUP=true to continue without a backup."
    )
}

/// Result of the backup before a conversion stage.
#[derive(Debug, PartialEq, Eq)]
pub enum Backup {
    /// Database without documents (new installation, freshly imported): nothing to back up.
    NotNeeded,
    /// disabled (`--no-backup` / `PAPERRUST_DATAMODEL_BACKUP=off`)
    Skipped,
    /// no `pg_dump` found, but `PAPERRUST_MIGRATE_WITHOUT_BACKUP=true` – warning, continue
    NoTool,
    Written(PathBuf),
}

/// Backup before applying stage `stage`, if necessary (see module header).
pub async fn backup_before(cfg: &pl_core::config::Config, conn: &mut PgConnection, stage: &str, no_backup: bool) -> CliResult<Backup> {
    let docs: bool =
        sqlx::query_scalar("SELECT to_regclass('public.documents_document') IS NOT NULL AND EXISTS (SELECT 1 FROM documents_document)")
            .fetch_one(&mut *conn)
            .await
            .unwrap_or(false);
    if !docs {
        return Ok(Backup::NotNeeded);
    }
    if no_backup || backup_disabled(&cfg.env) {
        return Ok(Backup::Skipped);
    }
    let Some(pg_dump) = find_pg_dump(&cfg.env) else {
        return if migrate_without_backup(&cfg.env)? { Ok(Backup::NoTool) } else { Err(CliError::Command(no_pg_dump_message(stage))) };
    };
    let dir = cfg.data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let short = stage.rsplit('.').next().unwrap_or(stage);
    let file = dir.join(format!("paperrust-vor-{short}-{}.pgdump", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let db = &cfg.db;
    let mut c = tokio::process::Command::new(&pg_dump);
    c.arg("-Fc").arg("--no-password").arg("-f").arg(&file).arg("-d").arg(&db.name);
    // credentials via the libpq environment, never on the command line
    c.env("PGUSER", &db.user).env("PGPASSWORD", &db.password).env("PGSSLMODE", &db.sslmode);
    if let Some(h) = &db.host {
        c.env("PGHOST", h);
    }
    if let Some(p) = db.port {
        c.env("PGPORT", p.to_string());
    }
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
    let out = c.output().await.map_err(|e| CliError::Command(format!("cannot start {}: {e}", pg_dump.display())))?;
    if !out.status.success() || std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0) == 0 {
        let _ = std::fs::remove_file(&file);
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(CliError::Command(format!(
            "Backup before data model stage {stage} failed (pg_dump: {}). Nothing was changed. \
             Fix the cause, or back up yourself and run `migrate --no-backup`.",
            msg.lines().next().unwrap_or("").trim()
        )));
    }
    Ok(Backup::Written(file))
}

/// Differences of the compatibility views compared to v3.4.2 (empty = same).
pub async fn check_compat(conn: &mut PgConnection) -> CliResult<Vec<String>> {
    let applied = applied(conn).await?;
    let views = datamodel::compat_views(applied.iter().map(String::as_str));
    let want_all = baseline::rows(baseline::COLUMNS_TSV);
    let mut diffs = Vec::new();
    for v in views {
        let kind: Option<String> = sqlx::query_scalar(
            "SELECT c.relkind::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND c.relname = $1",
        )
        .bind(v)
        .fetch_optional(&mut *conn)
        .await?;
        if kind.as_deref() != Some("v") {
            diffs.push(format!("{v}: expected a view, found {}", kind.as_deref().unwrap_or("nothing")));
            continue;
        }
        let rows = sqlx::query(
            "SELECT column_name::text, udt_name::text, coalesce(character_maximum_length::text, '') FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position",
        )
        .bind(v)
        .fetch_all(&mut *conn)
        .await?;
        let mut got = Vec::new();
        for r in rows {
            got.push(format!("{}\t{}\t{}", r.try_get::<String, _>(0)?, r.try_get::<String, _>(1)?, r.try_get::<String, _>(2)?));
        }
        let want: Vec<String> = want_all
            .iter()
            .filter(|r| r[0] == v)
            // tag tree as adjacency list (pl_objects.0001_paperrust_tag_adjacency): of the
            // django-treenode columns only tn_parent_id remains
            .filter(|r| !(v == "documents_tag" && r[1].starts_with("tn_") && r[1] != "tn_parent_id"))
            .map(|r| format!("{}\t{}\t{}", r[1], r[3], r.get(4).copied().unwrap_or("")))
            .collect();
        if got != want {
            diffs.push(format!("{v}: columns differ from v3.4.2\n    want: {}\n    got:  {}", want.join(" | "), got.join(" | ")));
        }
    }
    Ok(diffs)
}

/// Rollback of all applied stages with number > `to`, newest first. Returns: names.
pub async fn rollback(conn: &mut PgConnection, to: u8) -> CliResult<Vec<String>> {
    let applied = applied(conn).await?;
    let mut done = Vec::new();
    for s in STAGES.iter().rev().filter(|s| s.n > to && applied.contains(s.name)) {
        let mut tx = conn.begin().await?;
        sqlx::raw_sql(s.down).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM paperrust_migrations WHERE name = $1").bind(s.name).execute(&mut *tx).await?;
        tx.commit().await?;
        done.push(s.name.to_string());
    }
    Ok(done)
}

const OPTS: &[args::Opt] = &[args::value(&["--to"], "to"), flag(&["--no-input", "--noinput"], "no_input")];

/// Counter differences versus a recomputation (per kind, label, owner bucket); empty = same.
pub async fn counts_diff(conn: &mut PgConnection) -> CliResult<Vec<String>> {
    let rows = sqlx::query(
        "WITH d AS (SELECT x.id, x.correspondent_id AS c, x.document_type_id AS t, x.storage_path_id AS s,
                           CASE WHEN x.root_document_id IS NULL THEN coalesce(x.owner_id, 0) ELSE coalesce(r.owner_id, 0) END AS b
                      FROM paperrust_document x LEFT JOIN paperrust_document r ON r.id = x.root_document_id
                     WHERE x.deleted_at IS NULL),
              want AS (SELECT k.kind, k.label, d.b, count(*)::bigint AS n FROM d
                         CROSS JOIN LATERAL (VALUES (3::smallint, d.c), (4::smallint, d.t), (5::smallint, d.s)) k(kind, label)
                        WHERE k.label IS NOT NULL GROUP BY 1, 2, 3
                       UNION ALL
                       SELECT 2::smallint, dt.tag_id, d.b, count(*)::bigint FROM d JOIN paperrust_document_tag dt ON dt.document_id = d.id
                        GROUP BY 2, 3
                       UNION ALL
                       SELECT 6::smallint, fv.field_id, d.b, count(*)::bigint FROM d JOIN documents_customfieldinstance fv ON fv.document_id = d.id
                        WHERE fv.deleted_at IS NULL AND to_regclass('paperrust_field_value') IS NOT NULL GROUP BY 2, 3),
              got AS (SELECT kind, label_id AS label, owner_bucket AS b, n FROM paperrust_label_count_live WHERE n <> 0)
         SELECT coalesce(w.kind, g.kind)::int4, coalesce(w.label, g.label), coalesce(w.b, g.b), coalesce(w.n, 0), coalesce(g.n, 0)
           FROM want w FULL JOIN got g ON g.kind = w.kind AND g.label = w.label AND g.b = w.b
          WHERE w.n IS DISTINCT FROM g.n ORDER BY 1, 2, 3 LIMIT 50",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        out.push(format!(
            "kind {} label {} owner {}: expected {}, counter {}",
            r.try_get::<i32, _>(0)?,
            r.try_get::<i32, _>(1)?,
            r.try_get::<i32, _>(2)?,
            r.try_get::<i64, _>(3)?,
            r.try_get::<i64, _>(4)?
        ));
    }
    Ok(out)
}

/// `datamodel counts check | rebuild | fold` (stage 2).
async fn counts(conn: &mut PgConnection, sub: &str) -> CliResult {
    let has: bool = sqlx::query_scalar("SELECT to_regclass('paperrust_label_count') IS NOT NULL").fetch_one(&mut *conn).await?;
    if !has {
        return Err(CliError::Command("label counters are not installed (data model stage 2 not applied)".into()));
    }
    match sub {
        "check" => {
            let d = counts_diff(conn).await?;
            if d.is_empty() {
                println!("Label counters match the documents.");
                Ok(())
            } else {
                Err(CliError::Command(format!("label counters differ (run `datamodel counts rebuild`):\n{}", d.join("\n"))))
            }
        }
        "rebuild" => {
            let n: i64 = sqlx::query_scalar("SELECT paperrust_label_count_rebuild()").fetch_one(&mut *conn).await?;
            println!("Label counters rebuilt ({n} rows).");
            Ok(())
        }
        "fold" => {
            let n: i32 = sqlx::query_scalar("SELECT paperrust_label_count_fold()").fetch_one(&mut *conn).await?;
            println!("Folded {n} counter deltas.");
            Ok(())
        }
        other => Err(CliError::Usage(format!("unknown counts subcommand '{other}' (check | rebuild | fold)"))),
    }
}

/// `manage datamodel status | check | rollback --to <n> | counts check|rebuild|fold`.
pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 2)?;
    let sub = a.positionals.first().cloned().unwrap_or_else(|| "status".into());
    let cfg = ctx::load_config()?;
    let pool = ctx::connect_pool(&cfg).await?;
    let mut conn = pool.acquire().await?;
    match sub.as_str() {
        "status" => {
            let applied = applied(&mut conn).await?;
            println!("PaperRust data model (docs/DATAMODEL-V2.md):");
            if STAGES.is_empty() {
                println!("  no conversion stages in this version");
            }
            for s in STAGES {
                let mark = if applied.contains(s.name) { "X" } else { " " };
                println!(" [{mark}] stage {}: {} ({})", s.n, s.title, s.name);
            }
            Ok(())
        }
        "check" => {
            let diffs = check_compat(&mut conn).await?;
            if diffs.is_empty() {
                println!("Compatibility views match the paperless-ngx v3.4.2 columns.");
                Ok(())
            } else {
                Err(CliError::Command(diffs.join("\n")))
            }
        }
        "rollback" => {
            let to: u8 = a
                .value("to")
                .ok_or_else(|| CliError::Usage("rollback needs --to <stage> (0 = before stage 1)".into()))?
                .parse()
                .map_err(|_| CliError::Usage("--to expects a stage number".into()))?;
            let done = rollback(&mut conn, to).await?;
            if done.is_empty() {
                println!("Nothing to roll back.");
            }
            for n in done {
                println!("  Rolled back {n}... OK");
            }
            Ok(())
        }
        "counts" => counts(&mut conn, a.positionals.get(1).map(String::as_str).unwrap_or("check")).await,
        other => {
            Err(CliError::Usage(format!("unknown subcommand '{other}' (status | check | rollback --to <n> | counts check|rebuild|fold)")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_switch() {
        let off = pl_core::config::Env::from_pairs([("PAPERRUST_DATAMODEL_BACKUP", "off")]);
        assert!(backup_disabled(&off));
        let on = pl_core::config::Env::from_pairs([("PAPERRUST_DATAMODEL_BACKUP", "yes")]);
        assert!(!backup_disabled(&on));
        assert!(!backup_disabled(&pl_core::config::Env::from_pairs(Vec::<(String, String)>::new())));
    }

    /// D-073: without pg_dump only continue with explicit permission; the message names the way out.
    #[test]
    fn without_backup_switch_and_message() {
        let none = pl_core::config::Env::from_pairs(Vec::<(String, String)>::new());
        assert!(!migrate_without_backup(&none).unwrap());
        let yes = pl_core::config::Env::from_pairs([("PAPERRUST_MIGRATE_WITHOUT_BACKUP", "true")]);
        assert!(migrate_without_backup(&yes).unwrap());
        let bad = pl_core::config::Env::from_pairs([("PAPERRUST_MIGRATE_WITHOUT_BACKUP", "vielleicht")]);
        assert!(migrate_without_backup(&bad).is_err());
        let m = no_pg_dump_message("pl_store.dm1_paperrust_document");
        assert!(m.contains("pg_dump was not found") && m.contains("Nothing was changed"));
        assert!(m.contains("PAPERRUST_MIGRATE_WITHOUT_BACKUP=true") && m.contains("PAPERRUST_PG_BIN"));
        assert!(!m.contains("  "), "keine Leerzeichenfolgen in der Meldung: {m}");
    }

    #[test]
    fn compat_views_are_v342_tables() {
        let tables: BTreeSet<&str> = baseline::rows(baseline::COLUMNS_TSV).into_iter().map(|r| r[0]).collect();
        for s in STAGES {
            for v in s.views {
                assert!(tables.contains(v), "{v} ist keine v3.4.2-Tabelle");
            }
        }
    }
}
