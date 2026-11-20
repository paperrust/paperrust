//! Shared command context: settings (like `django.conf.settings`) and DB pool.
//!
//! upstream: src/paperless/settings/__init__.py, custom.py (via pl_core::config::Config).

use std::path::PathBuf;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::{CliError, CliResult};

pub struct Ctx {
    pub cfg: pl_core::config::Config,
    pub pool: PgPool,
}

impl Ctx {
    /// Read settings and connect to the database (PostgreSQL; other engines → error).
    pub async fn connect() -> CliResult<Ctx> {
        let cfg = load_config()?;
        let pool = connect_pool(&cfg).await?;
        Ok(Ctx { cfg, pool })
    }

    pub fn thumbnail_path(&self, pk: i64) -> PathBuf {
        self.cfg.thumbnail_dir.join(format!("{pk:07}.webp"))
    }

    /// `Document.source_path` – name from the DB only via `safe_join` (security review 2026-11-19
    /// F9); unsafe name → path that never exists (like "file missing", warning in the log).
    pub fn source_path(&self, filename: &str) -> PathBuf {
        pl_consumer::files::join(&self.cfg.originals_dir, filename)
    }

    /// `Document.archive_path`, rules as for [`Ctx::source_path`].
    pub fn archive_path(&self, filename: &str) -> PathBuf {
        pl_consumer::files::join(&self.cfg.archive_dir, filename)
    }

    pub fn templating_options(&self) -> pl_templating::Options {
        pl_templating::Options {
            filename_format: self.cfg.filename_format.clone(),
            filename_format_remove_none: self.cfg.filename_format_remove_none,
            time_zone: self.cfg.tz,
        }
    }
}

pub fn load_config() -> CliResult<pl_core::config::Config> {
    let cfg = pl_core::config::Config::from_env().map_err(|e| CliError::Command(format!("configuration error: {e}")))?;
    if cfg.db.engine != pl_core::config::DbEngine::Postgresql {
        return Err(CliError::Command(
            "PaperRust supports PostgreSQL only. Migrate SQLite/MariaDB first with paperrust-migrate-db (docs/MIGRATION.md)."
                .into(),
        ));
    }
    Ok(cfg)
}

pub async fn connect_pool(cfg: &pl_core::config::Config) -> CliResult<PgPool> {
    let opts = pl_core::state::pg_connect_options(&cfg.db);
    PgPoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .map_err(|e| CliError::Command(format!("cannot connect to the database: {e}")))
}

/// Console: output like `self.stdout.write(...)` / `self.console.print(...)`.
pub fn out(s: &str) {
    println!("{s}");
}
