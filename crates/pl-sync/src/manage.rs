//! `paperrust manage sync {enable,disable,rebuild,reset-identity,status,sequence,retention}`.
//!
//! Included by pl-cli (`dispatch`: `"sync" => …`); the crate does not know pl-cli, so
//! [`command`] receives the pool and returns the output text.

use sqlx::PgPool;

use crate::{SyncConfig, SyncError};

/// Fehler eines `manage sync`-Aufrufs.
#[derive(Debug, thiserror::Error)]
pub enum ManageError {
    /// falsche Argumente (pl-cli: Exit 2)
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Sync(#[from] SyncError),
}

pub const HELP: &str = "usage: paperrust manage sync <subcommand>

  status [--json]         state of the change capture (triggers, pending, feed position)
  enable                  install triggers (and the migration if missing), rebuild the feed
  disable [--purge]       remove the triggers; tables stay (paperless-ngx keeps working).
                          --purge also drops all paperrust_sync_* objects
  rebuild                 re-capture every object (after a large import or while disabled)
  reset-identity          new server_uuid, min_cursor = current seq (after copying the DB)
  sequence                run the sequencer once (normally done by the worker)
  retention [--days N]    delete tombstones older than N days (PAPERRUST_SYNC_TOMBSTONE_DAYS, 180)
";

/// Runs `argv` (without `sync`); returns = output for stdout.
pub async fn command(pool: &PgPool, argv: &[String]) -> Result<String, ManageError> {
    let Some(sub) = argv.first().map(String::as_str) else {
        return Err(ManageError::Usage(HELP.trim_end().to_string()));
    };
    let rest = &argv[1..];
    let flag = |name: &str| rest.iter().any(|a| a == name);
    let unknown = |allowed: &[&str]| -> Result<(), ManageError> {
        let mut it = rest.iter();
        while let Some(a) = it.next() {
            if allowed.contains(&a.as_str()) {
                if a == "--days" {
                    it.next();
                }
                continue;
            }
            return Err(ManageError::Usage(format!("unrecognized argument: {a}")));
        }
        Ok(())
    };
    match sub {
        "help" | "--help" | "-h" => Ok(HELP.to_string()),
        "status" => {
            unknown(&["--json"])?;
            let s = crate::status(pool).await?;
            if flag("--json") {
                return Ok(format!("{}\n", serde_json::to_string_pretty(&s).unwrap_or_default()));
            }
            if !s.installed {
                return Ok("sync capture: not installed (paperrust manage migrate / manage sync enable)\n".into());
            }
            let state = match (s.enabled, s.triggers == s.expected_triggers) {
                (true, true) => "enabled".to_string(),
                (true, false) => {
                    format!("enabled, but {} of {} triggers present (run `manage sync enable`)", s.triggers, s.expected_triggers)
                }
                (false, _) => "disabled".to_string(),
            };
            Ok(format!(
                "sync capture: {state}\n  triggers:     {}/{}\n  server_uuid:  {}\n  last seq:     {}\n  min_cursor:   {}\n  perm_epoch:   {}\n  objects:      {}\n  feed entries: {} ({} tombstones)\n  pending:      {} ({} ready)\n",
                s.triggers,
                s.expected_triggers,
                s.server_uuid.unwrap_or_default(),
                s.last_seq,
                s.min_cursor,
                s.perm_epoch,
                s.objects,
                s.log_entries,
                s.tombstones,
                s.pending,
                s.pending_ready,
            ))
        }
        "enable" => {
            unknown(&[])?;
            let r = crate::enable(pool).await?;
            Ok(format!(
                "{}sync capture enabled: {} triggers, {} feed entries rebuilt\n",
                if r.migrated { "migration pl_sync.0001_paperrust_sync applied; " } else { "" },
                r.triggers,
                r.rebuilt
            ))
        }
        "disable" => {
            unknown(&["--purge"])?;
            let purge = flag("--purge");
            let n = crate::disable(pool, purge).await?;
            Ok(if purge {
                format!("sync capture removed: {n} triggers and all paperrust_sync_* objects dropped\n")
            } else {
                format!("sync capture disabled: {n} triggers removed (tables kept; `manage sync enable` re-enables)\n")
            })
        }
        "rebuild" => {
            unknown(&[])?;
            let n = crate::rebuild(pool).await?;
            Ok(format!("feed rebuilt: {n} entries\n"))
        }
        "reset-identity" => {
            unknown(&[])?;
            let (id, min) = crate::reset_identity(pool).await?;
            Ok(format!("new server_uuid {id}, min_cursor {min} (all clients will re-sync)\n"))
        }
        "sequence" => {
            unknown(&[])?;
            if !crate::installed(pool).await? {
                return Err(SyncError::NotInstalled.into());
            }
            let r = crate::sequence_once(pool, SyncConfig::from_env().batch).await?;
            Ok(if r.busy { "another sequencer is running\n".to_string() } else { format!("sequenced {} pending entries\n", r.pending) })
        }
        "retention" => {
            unknown(&["--days"])?;
            let mut days = SyncConfig::from_env().tombstone_days;
            if let Some(i) = rest.iter().position(|a| a == "--days") {
                days = rest
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| ManageError::Usage("--days needs a non-negative number".into()))?;
            }
            if !crate::installed(pool).await? {
                return Err(SyncError::NotInstalled.into());
            }
            let n = crate::retention(pool, days).await?;
            Ok(format!("{n} tombstones older than {days} days removed\n"))
        }
        other => Err(ManageError::Usage(format!("unknown subcommand '{other}'\n{}", HELP.trim_end()))),
    }
}
