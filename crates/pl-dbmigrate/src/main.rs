//! paperrust-migrate-db: move a paperless-ngx v3.4.2 database (SQLite, MariaDB/MySQL,
//! PostgreSQL) into an empty PostgreSQL database with exactly the Django schema of v3.4.2.
//! Owner: DB migration agent. Contract: docs/contract/pl-dbmigrate.md, guide: docs/MIGRATION.md.
//!
//! upstream: src/*/migrations@v3.4.2 (schema), src/paperless/settings/custom.py@v3.4.2 (parse_db_settings),
//! Django field types per backend (django/db/backends/{sqlite3,mysql,postgresql}).

mod api;
mod assist;
mod catchup;
mod detect;
mod export;
mod hash;
mod migrate;
mod report;
mod schema;
mod source;
mod switch;
mod value;
mod verify;
mod zipread;

use migrate::{Mode, Options};
use report::Report;
use std::time::Instant;
use value::{NulPolicy, Policy, Utf8Policy};

const USAGE: &str = "\
paperrust-migrate-db – paperless-ngx-Datenbank (v3.4.2) nach PostgreSQL umziehen

Aufruf:
  paperrust-migrate-db --source <URL> --target <URL> [Optionen]
  paperrust-migrate-db switch|verify|detect …   Umstieg von paperless-ngx in einem Schritt mit
                         Prüfbericht (Export, Datenordner oder URL; Hilfe: switch --help)

Quelle (wird nur gelesen):
  sqlite:///pfad/zu/data/db.sqlite3     (Windows: sqlite:///C:/pfad/db.sqlite3)
  mysql://user:pass@host:3306/paperless  bzw. mariadb://…
  postgres://user:pass@host:5432/paperless
Ziel: postgres://user:pass@host:5432/neue_leere_db  (muss leer sein, Kodierung UTF8)
  Statt auf der Kommandozeile auch über PAPERRUST_MIGRATE_SOURCE / PAPERRUST_MIGRATE_TARGET
  (die älteren Namen PAPERLESS_MIGRATE_SOURCE / PAPERLESS_MIGRATE_TARGET gelten weiter).

Optionen:
  --dry-run               nur Vorprüfung, vollständiger Lesedurchlauf mit Umwandlungsprüfung,
                          Zeilenzahlen/Größen; schreibt nichts (Ziel optional, wird nur geprüft)
  --verify-only           Quelle und bereits befülltes Ziel vergleichen (nur lesend)
  --report <DATEI>        Bericht als JSON (zusätzlich <DATEI>.txt als Text)
  --nul fail|replace|strip  NUL-Zeichen (U+0000) in Texten/JSON: Abbruch (Standard),
                          durch U+FFFD ersetzen oder entfernen – jede Zelle wird gemeldet
  --invalid-utf8 fail|replace  ungültiges UTF-8 (nur SQLite möglich): Abbruch (Standard) oder
                          durch U+FFFD ersetzen – jede Zelle wird gemeldet
  --skip-unknown-tables   Tabellen außerhalb des Django-Schemas bewusst weglassen
  --batch-bytes <N>       COPY-Puffer in Bytes (Standard 4194304)
  --max-issues <N>        gespeicherte Einzelbefunde je Tabelle (Standard 200)
  --quiet                 keine Fortschrittsanzeige
  --media <ORDNER>        Medienordner der Quelle (paperless-ngx 2.x: SHA-256 aus den Dateien)
  -h, --help

Exit-Code: 0 = Erfolg, 1 = fehlgeschlagen/Probleme, 2 = falscher Aufruf, 130 = abgebrochen.
";

/// `PAPERRUST_<name>`, otherwise (compatibility) `PAPERLESS_<name>` – the same rule as
/// `pl_core::config::var` (this tool builds without pl-core).
fn env_var(name: &str) -> Option<String> {
    std::env::var(format!("PAPERRUST_{name}")).or_else(|_| std::env::var(format!("PAPERLESS_{name}"))).ok()
}

fn parse_args(args: &[String]) -> Result<(Options, Option<String>), String> {
    let mut source = env_var("MIGRATE_SOURCE");
    let mut target = env_var("MIGRATE_TARGET");
    let mut mode = Mode::Migrate;
    let mut report = None;
    let mut policy = Policy { nul: NulPolicy::Fail, utf8: Utf8Policy::Fail };
    let mut batch_bytes: usize = 4 << 20;
    let mut max_issues = 200;
    let mut skip = false;
    let mut quiet = false;
    let mut media = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |n: &str| it.next().cloned().ok_or_else(|| format!("{n} braucht einen Wert"));
        match a.as_str() {
            "--source" => source = Some(val(a)?),
            "--target" => target = Some(val(a)?),
            "--report" => report = Some(val(a)?),
            "--dry-run" => mode = Mode::DryRun,
            "--verify-only" => mode = Mode::VerifyOnly,
            "--skip-unknown-tables" => skip = true,
            "--quiet" => quiet = true,
            "--media" => media = Some(std::path::PathBuf::from(val(a)?)),
            "--nul" => {
                policy.nul = match val(a)?.as_str() {
                    "fail" => NulPolicy::Fail,
                    "replace" => NulPolicy::Replace,
                    "strip" => NulPolicy::Strip,
                    o => return Err(format!("--nul: unbekannter Wert {o}")),
                }
            }
            "--invalid-utf8" => {
                policy.utf8 = match val(a)?.as_str() {
                    "fail" => Utf8Policy::Fail,
                    "replace" => Utf8Policy::Replace,
                    o => return Err(format!("--invalid-utf8: unbekannter Wert {o}")),
                }
            }
            "--batch-bytes" => batch_bytes = val(a)?.parse().map_err(|_| "--batch-bytes: Zahl erwartet")?,
            "--max-issues" => max_issues = val(a)?.parse().map_err(|_| "--max-issues: Zahl erwartet")?,
            "-h" | "--help" => return Err(String::new()),
            o => return Err(format!("unbekannte Option {o}")),
        }
    }
    let source = source.ok_or("--source fehlt")?;
    if mode != Mode::DryRun && target.is_none() {
        return Err("--target fehlt".into());
    }
    if let Some(t) = &target
        && !(t.starts_with("postgres://") || t.starts_with("postgresql://"))
    {
        return Err("--target muss eine PostgreSQL-URL sein (postgres://…)".into());
    }
    Ok((
        Options {
            source,
            target,
            mode,
            policy,
            batch_bytes: batch_bytes.clamp(64 << 10, 256 << 20),
            skip_unknown_tables: skip,
            quiet,
            max_issues,
            media,
        },
        report,
    ))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd @ ("switch" | "verify" | "detect")) = args.first().map(String::as_str) {
        std::process::exit(switch::main(cmd, &args[1..]).await);
    }
    let (opts, report_path) = match parse_args(&args) {
        Ok(x) => x,
        Err(e) => {
            if e.is_empty() {
                print!("{USAGE}");
                std::process::exit(0);
            }
            eprintln!("Fehler: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let mut rep = Report {
        tool_version: env!("CARGO_PKG_VERSION").into(),
        mode: match opts.mode {
            Mode::Migrate => "migrate",
            Mode::DryRun => "dry-run",
            Mode::VerifyOnly => "verify-only",
        }
        .into(),
        source: source::redact(&opts.source),
        target: opts.target.as_deref().map(source::redact),
        started: chrono::Utc::now().to_rfc3339(),
        nul_policy: format!("{:?}", opts.policy.nul).to_lowercase(),
        utf8_policy: format!("{:?}", opts.policy.utf8).to_lowercase(),
        ..Default::default()
    };
    let t0 = Instant::now();

    let outcome = tokio::select! {
        r = migrate::run(&opts, &mut rep) => Some(r),
        _ = tokio::signal::ctrl_c() => None,
    };
    match outcome {
        Some(Ok(())) => {}
        Some(Err(e)) => {
            rep.error = Some(format!("{e:#}"));
            rep.result = if opts.mode == Mode::VerifyOnly { "verify-failed" } else { "failed" }.into();
        }
        None => {
            rep.error = Some("durch Benutzer abgebrochen (Strg+C)".into());
            rep.result = "aborted".into();
        }
    }
    rep.finished = chrono::Utc::now().to_rfc3339();
    rep.duration_s = t0.elapsed().as_secs_f64();

    let text = rep.to_text();
    print!("{text}");
    if let Some(p) = report_path {
        let json = serde_json::to_string_pretty(&rep.to_json()).unwrap_or_default();
        if let Err(e) = std::fs::write(&p, json) {
            eprintln!("Bericht {p} konnte nicht geschrieben werden: {e}");
        }
        if let Err(e) = std::fs::write(format!("{p}.txt"), &text) {
            eprintln!("Bericht {p}.txt konnte nicht geschrieben werden: {e}");
        }
    }
    std::process::exit(if rep.result == "aborted" {
        130
    } else if rep.ok() {
        0
    } else {
        1
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn args() {
        let (o, r) = parse_args(&a(&["--source", "sqlite:///x", "--target", "postgres://h/db", "--report", "r.json"])).unwrap();
        assert_eq!(o.mode, Mode::Migrate);
        assert_eq!(r.as_deref(), Some("r.json"));
        assert!(parse_args(&a(&["--source", "sqlite:///x"])).is_err());
        let (o, _) = parse_args(&a(&["--source", "sqlite:///x", "--dry-run", "--nul", "replace"])).unwrap();
        assert_eq!(o.mode, Mode::DryRun);
        assert_eq!(o.policy.nul, NulPolicy::Replace);
        assert!(parse_args(&a(&["--source", "sqlite:///x", "--target", "mysql://x"])).is_err());
    }
}
