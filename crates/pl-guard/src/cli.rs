//! Command line (`paperrust manage guard …`, `paperrust manage backup …`), mounted by pl-cli.

use std::path::{Path, PathBuf};

use sqlx::PgPool;

use crate::config::{BackupConfig, GuardConfig};
use crate::guard::{self, Catalog};
use crate::{backup, db, readonly, store};

/// Result of a command.
#[derive(Debug)]
pub enum Outcome {
    Ok(String),
    /// wrong invocation (exit 2)
    Usage(String),
    /// error (exit 1, "CommandError: …")
    Err(String),
}

pub const GUARD_USAGE: &str = "usage: manage guard status | lift [--note TEXT] | scan";
pub const BACKUP_USAGE: &str = "usage: manage backup init [--show-key] | run | list | check [--snapshot ID] [--key-file FILE] | \
restore [--snapshot ID|latest] [--media DIR] [--database] [--overwrite] [--key-file FILE]\n\
The passphrase is read from PAPERRUST_BACKUP_PASSPHRASE or --passphrase-file FILE.";

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned().or_else(|| {
        let p = format!("{name}=");
        args.iter().find_map(|a| a.strip_prefix(&p).map(str::to_string))
    })
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// `manage guard …`
pub async fn guard_command(args: &[String], pool: &PgPool, media_root: &Path) -> Outcome {
    let Some(sub) = args.first() else { return Outcome::Usage(GUARD_USAGE.into()) };
    match sub.as_str() {
        "status" => match status_text(pool).await {
            Ok(s) => Outcome::Ok(s),
            Err(e) => Outcome::Err(e),
        },
        "lift" => {
            let note = opt(args, "--note").unwrap_or_else(|| "lifted on the command line".into());
            match store::lift_read_only(pool, None, &note).await {
                Ok(()) => {
                    readonly::set_local(false);
                    Outcome::Ok("Read-only mode lifted.".into())
                }
                Err(e) => Outcome::Err(e.to_string()),
            }
        }
        "scan" => match scan(pool, media_root).await {
            Ok(findings) if findings.is_empty() => Outcome::Ok("No suspicious files found.".into()),
            Ok(findings) => {
                let mut s = format!("{} suspicious file(s):\n", findings.len());
                for f in findings.iter().take(1000) {
                    s.push_str(&format!("{}\t{}\t{}\n", f.reason.as_str(), f.path, f.detail));
                }
                Outcome::Err(s.trim_end().to_string())
            }
            Err(e) => Outcome::Err(e),
        },
        _ => Outcome::Usage(GUARD_USAGE.into()),
    }
}

async fn status_text(pool: &PgPool) -> Result<String, String> {
    let s = store::state(pool).await.map_err(|e| e.to_string())?;
    let mut out = if s.read_only {
        format!("READ-ONLY since {}: {}\n", s.since.map(|t| t.to_rfc3339()).unwrap_or_default(), s.reason)
    } else {
        "Normal operation (not read-only).\n".to_string()
    };
    for e in store::list_events(pool, 10).await.map_err(|e| e.to_string())? {
        out.push_str(&format!("event {} [{}] {} – {} file(s): {}\n", e.id, e.status, e.created.to_rfc3339(), e.files, e.summary));
        if e.status == "open" {
            for f in store::findings(pool, Some(e.id), 50).await.map_err(|e| e.to_string())? {
                out.push_str(&format!("  {}\t{}\t{}\n", f.reason, f.path, f.detail));
            }
        }
    }
    Ok(out.trim_end().to_string())
}

/// One-off complete check of all files and documents against the database (without waiting time,
/// without read-only mode). For the check after an incident.
pub async fn scan(pool: &PgPool, media_root: &Path) -> Result<Vec<guard::Finding>, String> {
    let cfg = GuardConfig::from_env();
    let catalog = db::DbCatalog { pool: pool.clone() };
    scan_with(&catalog, &cfg, media_root).await
}

/// [`scan`] with an arbitrary catalogue (tests).
pub async fn scan_with(catalog: &dyn Catalog, cfg: &GuardConfig, media_root: &Path) -> Result<Vec<guard::Finding>, String> {
    let mut snap = guard::Snapshot::default();
    let mut files = Vec::new();
    let root = media_root.to_path_buf();
    let all = tokio::task::spawn_blocking(move || {
        // report all files as "changed": empty initial state, no baseline
        let r = snap.scan(&root, false);
        r.changed
    })
    .await
    .map_err(|e| e.to_string())?;
    files.extend(all);
    let mut seen: std::collections::HashSet<String> = files.iter().cloned().collect();
    // missing files: all documents of the database
    let mut after = 0;
    loop {
        let docs = catalog.sample(after, 1000).await?;
        if docs.is_empty() {
            break;
        }
        for d in docs {
            after = after.max(d.id);
            for (rel, _) in d.files {
                if seen.insert(rel.clone()) {
                    files.push(rel);
                }
            }
        }
    }
    let mut out = Vec::new();
    for rel in files {
        if let Some(f) = guard::evaluate(media_root, cfg, catalog, &rel).await? {
            out.push(f);
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn passphrase(args: &[String]) -> Result<Option<String>, String> {
    if let Some(f) = opt(args, "--passphrase-file") {
        let s = std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?;
        return Ok(Some(s.trim_end_matches(['\r', '\n']).to_string()));
    }
    Ok(crate::config::passphrase_from_env())
}

/// `manage backup …`. `restore_db`: database pool for `restore --database` (after `migrate`).
pub async fn backup_command(args: &[String], pool: &PgPool, media_root: &Path, data_dir: &Path) -> Outcome {
    let Some(sub) = args.first().cloned() else { return Outcome::Usage(BACKUP_USAGE.into()) };
    let cfg = match BackupConfig::from_env() {
        Ok(c) => c,
        Err(e) => return Outcome::Err(e),
    };
    let Some(target) = cfg.target.clone() else {
        return Outcome::Err("no backup target configured (PAPERRUST_BACKUP_TARGET)".into());
    };
    let res: Result<String, String> = async {
        match sub.as_str() {
            "init" => {
                let pass = passphrase(args)?.ok_or("set PAPERRUST_BACKUP_PASSPHRASE or use --passphrase-file")?;
                let (public, secret) = backup::init(&target, data_dir, &pass, None).await?;
                let mut s = format!("Backup target {} initialized.\nPublic key: {public}\n", target.display());
                if flag(args, "--show-key") {
                    s.push_str(&format!(
                        "\nSecret key (keep it offline, e.g. printed in a safe; with it, backups can be restored without the passphrase):\n{secret}\n"
                    ));
                } else {
                    s.push_str("Keep the passphrase safe: without it the backups cannot be restored.\n");
                }
                Ok(s)
            }
            "run" => {
                let src = backup::Sources { media_root: media_root.to_path_buf(), data_dir: data_dir.to_path_buf() };
                let r = backup::run(pool, &src, &cfg, "manage").await?;
                let mut s = format!(
                    "Snapshot {}: {} file(s) ({} bytes), {} new object(s), {} bytes uploaded, database dump {} bytes, {} old snapshot(s) removed.",
                    r.snapshot.unwrap_or_default(),
                    r.files_total,
                    r.bytes_total,
                    r.files_new,
                    r.bytes_new,
                    r.db_bytes,
                    r.pruned
                );
                if !r.warnings.is_empty() {
                    s.push_str(&format!("\nWarnings:\n{}", r.warnings));
                }
                Ok(s)
            }
            "list" => {
                let v = backup::list_snapshots(&target).await?;
                if v.is_empty() {
                    return Ok("No snapshots.".into());
                }
                Ok(v.iter().map(|(id, t)| format!("{id}\t{}", t.to_rfc3339())).collect::<Vec<_>>().join("\n"))
            }
            "check" => {
                let key = opt(args, "--key-file").map(PathBuf::from);
                let pass = passphrase(args)?;
                let id = backup::identity(&target, pass.as_deref(), key.as_deref()).await?;
                let snap = opt(args, "--snapshot").unwrap_or_else(|| "latest".into());
                let (n, missing) = backup::check(&target, &id, &snap).await?;
                if missing.is_empty() {
                    Ok(format!("Snapshot {snap}: all {n} object(s) present."))
                } else {
                    Err(format!("Snapshot {snap}: {} of {n} object(s) missing, e.g. {}", missing.len(), backup::short(&missing[0])))
                }
            }
            "restore" => {
                let key = opt(args, "--key-file").map(PathBuf::from);
                let pass = passphrase(args)?;
                let id = backup::identity(&target, pass.as_deref(), key.as_deref()).await?;
                let media = opt(args, "--media").map(PathBuf::from);
                let with_db = flag(args, "--database");
                if media.is_none() && !with_db {
                    return Err("nothing to restore: give --media DIR and/or --database".into());
                }
                let snap = opt(args, "--snapshot").unwrap_or_else(|| "latest".into());
                let r = backup::restore(
                    &target,
                    &id,
                    &backup::RestoreOptions { snapshot: &snap, media_dir: media.as_deref(), db: with_db.then_some(pool), overwrite: flag(args, "--overwrite") },
                )
                .await?;
                let mut s = format!("Restored snapshot {}: {} file(s), {} bytes", r.snapshot, r.files, r.bytes);
                if r.skipped_existing > 0 {
                    s.push_str(&format!(", {} already present", r.skipped_existing));
                }
                if let Some(t) = r.db_tables {
                    s.push_str(&format!(", database: {t} table(s)"));
                }
                s.push_str(". All checksums verified.");
                Ok(s)
            }
            _ => Err(String::new()),
        }
    }
    .await;
    match res {
        Ok(s) => Outcome::Ok(s),
        Err(e) if e.is_empty() => Outcome::Usage(BACKUP_USAGE.into()),
        Err(e) => Outcome::Err(e),
    }
}
