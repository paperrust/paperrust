//! Guard against the real database: catalogue from `documents_document`, findings/events in the
//! `paperrust_guard_*` tables, alarm (log + mail) on triggering.

use std::path::PathBuf;

use futures::future::BoxFuture;
use pl_core::AppState;
use sqlx::{PgPool, Row};

use crate::config::GuardConfig;
use crate::detect::Area;
use crate::guard::{Catalog, Expected, Finding, Reporter, SampleDoc};
use crate::{readonly, store};

/// Catalogue from `documents_document` (incl. trash and versions: their files stay in place).
pub struct DbCatalog {
    pub pool: PgPool,
}

impl Catalog for DbCatalog {
    fn lookup<'a>(&'a self, area: Area, rel: &'a str) -> BoxFuture<'a, Result<Option<Expected>, String>> {
        Box::pin(async move {
            let row = match area {
                Area::Originals => sqlx::query("SELECT id::bigint AS id, checksum AS c FROM documents_document WHERE filename = $1 LIMIT 1")
                    .bind(rel)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?,
                Area::Archive => sqlx::query("SELECT id::bigint AS id, archive_checksum AS c FROM documents_document WHERE archive_filename = $1 LIMIT 1")
                    .bind(rel)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?,
                Area::Thumbnails => {
                    // `0000123.webp` (or `.png` from older versions)
                    let stem = rel.rsplit('/').next().unwrap_or(rel).split('.').next().unwrap_or("");
                    let Ok(id) = stem.parse::<i64>() else { return Ok(None) };
                    let found: Option<i64> = sqlx::query_scalar("SELECT id::bigint FROM documents_document WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&self.pool)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(found.map(|doc_id| Expected { doc_id, checksum: None }));
                }
                Area::Other => return Ok(None),
            };
            Ok(match row {
                Some(r) => {
                    let id: i64 = r.try_get("id").map_err(|e| e.to_string())?;
                    let c: Option<String> = r.try_get("c").map_err(|e| e.to_string())?;
                    // without a stored checksum check only presence/content
                    Some(Expected { doc_id: id, checksum: c.filter(|s| !s.is_empty()) })
                }
                None => None,
            })
        })
    }

    fn sample(&self, after: i64, n: i64) -> BoxFuture<'_, Result<Vec<SampleDoc>, String>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT id::bigint AS id, filename, checksum, archive_filename, archive_checksum FROM documents_document
                 WHERE id > $1 ORDER BY id LIMIT $2",
            )
            .bind(after)
            .bind(n)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                let id: i64 = r.try_get("id").map_err(|e| e.to_string())?;
                let mut files = Vec::new();
                let f: Option<String> = r.try_get("filename").map_err(|e| e.to_string())?;
                let c: Option<String> = r.try_get("checksum").map_err(|e| e.to_string())?;
                if let (Some(f), Some(c)) = (f, c) {
                    files.push((format!("documents/originals/{f}"), c));
                }
                let f: Option<String> = r.try_get("archive_filename").map_err(|e| e.to_string())?;
                let c: Option<String> = r.try_get("archive_checksum").map_err(|e| e.to_string())?;
                if let (Some(f), Some(c)) = (f, c) {
                    files.push((format!("documents/archive/{f}"), c));
                }
                out.push(SampleDoc { id, files });
            }
            Ok(out)
        })
    }
}

/// Findings into the DB, on triggering read-only mode + alarm.
pub struct DbReporter {
    pub state: AppState,
    pub cfg: GuardConfig,
}

impl Reporter for DbReporter {
    fn finding<'a>(&'a self, f: &'a Finding) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let pool = self.state.db();
            let event = if readonly::is_active() { store::open_event(pool).await.map_err(|e| e.to_string())? } else { None };
            store::insert_finding(pool, &f.path, f.reason.as_str(), &f.detail, 1, event).await.map_err(|e| e.to_string())?;
            if let Some(id) = event {
                store::touch_event(pool, id).await.map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }

    fn trigger(&self, files: usize, window_minutes: i64) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let pool = self.state.db();
            let summary = format!("{files} suspicious file change(s) within {window_minutes} minute(s)");
            let id = store::create_event(pool, window_minutes, &summary).await.map_err(|e| e.to_string())?;
            store::set_read_only(pool, &summary, Some(id)).await.map_err(|e| e.to_string())?;
            readonly::set_local(true);
            let findings = store::findings(pool, Some(id), 20).await.unwrap_or_default();
            raise_alarm(&self.state, &self.cfg, id, &summary, &findings).await;
            Ok(())
        })
    }

    fn is_triggered(&self) -> bool {
        readonly::is_active()
    }
}

/// Alarm: log (error, logger `paperless.guard`) and mail to the admins if mail is configured.
pub async fn raise_alarm(state: &AppState, cfg: &GuardConfig, event_id: i64, summary: &str, sample: &[store::FindingRow]) {
    tracing::error!(target: "paperless.guard", event = event_id, %summary, "RANSOMWARE GUARD: read-only mode switched on");
    for f in sample {
        tracing::error!(target: "paperless.guard", event = event_id, path = %f.path, reason = %f.reason, "affected file");
    }
    let c = state.config();
    let settings = pl_accounts::settings::Settings::from_core(c);
    // Django default is EMAIL_HOST=localhost: send only if explicitly configured
    if settings.email_host.is_empty() || pl_core::config::var("PAPERLESS_EMAIL_HOST").is_none_or(|v| v.trim().is_empty()) {
        tracing::info!(target: "paperless.guard", "no mail server configured (PAPERRUST_EMAIL_HOST); alarm only in log and UI");
        return;
    }
    let recipients = if cfg.mail_to.is_empty() { admin_emails(state.db()).await } else { cfg.mail_to.clone() };
    if recipients.is_empty() {
        tracing::warn!(target: "paperless.guard", "no admin e-mail address for the guard alarm (set PAPERRUST_GUARD_MAIL)");
        return;
    }
    let brand = pl_core::brand::PRODUCT_NAME;
    let mut body = format!(
        "{brand} switched to read-only mode.\n\n{summary}.\n\nThe ransomware guard found files in the media folder that changed without {brand} \
         (encrypted, renamed or deleted). Uploads, consumption and changes are blocked until an administrator lifts read-only mode.\n\n\
         What to do:\n1. Find the infected computer and disconnect it from the network.\n2. Check the affected files in {brand} under Administration > Protection.\n\
         3. Restore damaged files from a snapshot or backup.\n4. Then lift read-only mode.\n\nFirst affected files:\n"
    );
    for f in sample {
        body.push_str(&format!("- {} ({})\n", f.path, f.reason));
    }
    if let Some(url) = c.paperless_url.as_deref() {
        body.push_str(&format!("\n{}/rs/app/protection\n", url.trim_end_matches('/')));
    }
    for to in recipients {
        let m = pl_accounts::mail::Mail {
            from: settings.default_from_email.clone(),
            to: to.clone(),
            subject: format!("[{brand}] ALERT: read-only mode after suspicious file changes"),
            body: body.clone(),
        };
        match pl_accounts::mail::send(&settings, &m).await {
            Ok(()) => tracing::info!(target: "paperless.guard", "guard alarm mail sent"),
            Err(e) => tracing::error!(target: "paperless.guard", error = %e, "guard alarm mail failed"),
        }
    }
}

async fn admin_emails(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar::<_, String>("SELECT email FROM auth_user WHERE is_active AND is_superuser AND email <> '' ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

/// Build the guard with a DB catalogue and DB receiver.
pub fn engine(state: &AppState, cfg: GuardConfig) -> crate::guard::Engine {
    let media: PathBuf = state.config().media_root.clone();
    let catalog = std::sync::Arc::new(DbCatalog { pool: state.db().clone() });
    let reporter = std::sync::Arc::new(DbReporter { state: state.clone(), cfg: cfg.clone() });
    crate::guard::Engine::new(media, cfg, catalog, reporter)
}

/// Start the guard in the worker process (until `stop`). Off if `PAPERRUST_GUARD=off`.
pub async fn run_guard(state: AppState, stop: tokio::sync::watch::Receiver<bool>) {
    let cfg = GuardConfig::from_env();
    if !cfg.enabled {
        tracing::warn!(target: "paperless.guard", "ransomware guard is off (PAPERRUST_GUARD=off)");
        return;
    }
    if let Err(e) = store::ensure(state.db()).await {
        tracing::error!(target: "paperless.guard", error = %e, "ransomware guard not started: tables missing");
        return;
    }
    engine(&state, cfg).run(stop).await;
}
