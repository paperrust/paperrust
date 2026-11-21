//! pl-guard – protection against ransomware (contract: docs/contract/pl-guard.md, docs: docs/user/ransomware.md).
//!
//! 1. **Guard** ([`guard`], [`db`]): watches the media folder, evaluates changes against the
//!    database and, above a threshold, switches to **read-only mode** ([`readonly`]) with an alarm.
//! 2. **Backups** ([`backup`]): encrypted (age), deduplicated, with versions and retention,
//!    local/network share or S3 with Object Lock; restore with checksum comparison.
//!
//! | Route id | Method | Path | Permission |
//! |---|---|---|---|
//! | `guard.status` | GET | `/api/x-guard/status/` | logged in (details: superuser) |
//! | `guard.events.list` | GET | `/api/x-guard/events/` | superuser |
//! | `guard.events.retrieve` | GET | `/api/x-guard/events/{id}/` | superuser |
//! | `guard.lift` | POST | `/api/x-guard/lift/` | superuser |
//! | `guard.backup.status` | GET | `/api/x-guard/backup/` | superuser |
//! | `guard.backup.run` | POST | `/api/x-guard/backup/run/` | superuser |

pub mod api;
pub mod backup;
pub mod cli;
pub mod config;
pub mod db;
pub mod detect;
pub mod guard;
pub mod readonly;
pub mod store;

use axum::http::Method;
use axum::routing::{get, post};
use pl_core::ApiRoute;
use pl_core::route::drf_view;

/// Task "backup" (worker, schedule `PAPERRUST_BACKUP_CRON`).
pub const TASK_BACKUP: &str = "paperrust.guard.backup";
/// Default schedule of the backup (daily 02:15), takes effect only with `PAPERRUST_BACKUP_TARGET`.
pub const BACKUP_CRON_DEFAULT: &str = "15 2 * * *";

const SRC: &str = "pl-guard extension (ransomware protection)";
const ALLOW_GET: &str = "GET, HEAD, OPTIONS";
const ALLOW_POST: &str = "POST, OPTIONS";

/// All routes (ids in routes.toml).
pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute::new("guard.status", Method::GET, "/api/x-guard/status/", SRC, drf_view(get(api::status), ALLOW_GET)),
        ApiRoute::new("guard.events.list", Method::GET, "/api/x-guard/events/", SRC, drf_view(get(api::events_list), ALLOW_GET)),
        ApiRoute::new(
            "guard.events.retrieve",
            Method::GET,
            "/api/x-guard/events/{id}/",
            SRC,
            drf_view(get(api::events_retrieve), ALLOW_GET),
        ),
        ApiRoute::new("guard.lift", Method::POST, "/api/x-guard/lift/", SRC, drf_view(post(api::lift), ALLOW_POST)),
        ApiRoute::new("guard.backup.status", Method::GET, "/api/x-guard/backup/", SRC, drf_view(get(api::backup_status), ALLOW_GET)),
        ApiRoute::new("guard.backup.run", Method::POST, "/api/x-guard/backup/run/", SRC, drf_view(post(api::backup_run), ALLOW_POST)),
    ]
}

/// Route ids (for routes.toml and tests).
pub fn route_ids() -> Vec<&'static str> {
    routes().iter().map(|r| r.id).collect()
}

/// Schedule entry of the backup (only with a target; `disable` switches it off).
pub fn backup_beat_entry(get: impl Fn(&str) -> Option<String>) -> Result<Option<pl_queue::beat::BeatEntry>, String> {
    let has_target = get("PAPERRUST_BACKUP_TARGET").is_some_and(|s| !s.trim().is_empty());
    let value = get("PAPERRUST_BACKUP_CRON").filter(|s| !s.trim().is_empty()).unwrap_or_else(|| BACKUP_CRON_DEFAULT.into());
    if !has_target || value.trim() == "disable" {
        return Ok(None);
    }
    let schedule = pl_queue::cron::Crontab::from_paperless(value.trim()).map_err(|e| format!("PAPERRUST_BACKUP_CRON: {e}"))?;
    let mut headers = serde_json::Map::new();
    headers.insert("trigger_source".into(), serde_json::Value::String("scheduled".into()));
    Ok(Some(pl_queue::beat::BeatEntry {
        name: "PaperRust: encrypted backup".into(),
        task: TASK_BACKUP.into(),
        schedule,
        expires: 6.0 * 3600.0,
        headers,
    }))
}

/// Handler of the backup task (worker).
pub async fn backup_task(state: &pl_core::AppState, trigger: &str) -> Result<serde_json::Value, String> {
    let cfg = config::BackupConfig::from_env()?;
    let src = backup::Sources { media_root: state.config().media_root.clone(), data_dir: state.config().data_dir.clone() };
    let r = backup::run(state.db(), &src, &cfg, trigger).await?;
    Ok(serde_json::json!(format!(
        "Backup {} finished: {} file(s), {} new, {} byte(s) uploaded",
        r.snapshot.unwrap_or_default(),
        r.files_total,
        r.files_new,
        r.bytes_new
    )))
}

#[cfg(test)]
mod tests {
    #[test]
    fn beat_entry() {
        assert!(super::backup_beat_entry(|_| None).unwrap().is_none(), "ohne Ziel kein Zeitplan");
        let e = super::backup_beat_entry(|k| (k == "PAPERRUST_BACKUP_TARGET").then(|| "/backup".to_string())).unwrap().unwrap();
        assert_eq!(e.task, super::TASK_BACKUP);
        let off = super::backup_beat_entry(|k| match k {
            "PAPERRUST_BACKUP_TARGET" => Some("/b".into()),
            "PAPERRUST_BACKUP_CRON" => Some("disable".into()),
            _ => None,
        });
        assert!(off.unwrap().is_none());
        assert_eq!(super::route_ids().len(), 6);
    }
}
