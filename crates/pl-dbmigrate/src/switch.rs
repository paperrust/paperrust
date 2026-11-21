//! Switchover from paperless-ngx in one step (package Z1, docs/user/migration.md):
//!
//! `paperrust-migrate-db switch --from <export|folder|URL> [--media <MEDIA_ROOT>] --target <PG-URL>`
//!
//! 1. Detect the source ([`crate::detect`]): export (folder/ZIP) → import with
//!    `paperrust manage migrate` + `manage document_importer`; data folder with SQLite or
//!    database URL → check migration state (exactly v3.4.2, otherwise explanation of the export route) →
//!    database move ([`crate::migrate`]) → copy media folder without overwriting.
//! 2. Verification report ([`crate::verify`]): JSON + text + HTML in the report folder.
//!
//! `--dry-run` only checks (read source, conversion, files), `verify` compares a switchover that has already
//! happened (also with a sample via the API of the running server). The source is never
//! changed.

use crate::detect::{self, Detection};
use crate::export::{DocRow, ExportSource};
use crate::migrate::{self, Mode, Options};
use crate::report::Report as DbReport;
use crate::source::{Source, redact};
use crate::value::{NulPolicy, Policy, Utf8Policy};
use crate::verify::{self, Counts, Files, Level, Report};
use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Switch,
    DryRun,
    Verify,
    Detect,
}

#[derive(Debug, Clone)]
pub struct SwitchOptions {
    pub cmd: Cmd,
    pub from: String,
    pub media: Option<PathBuf>,
    pub target: Option<String>,
    pub target_media: Option<PathBuf>,
    pub report_dir: PathBuf,
    pub api_url: Option<String>,
    pub api_token: Option<String>,
    pub api_temp_token: bool,
    pub sample: usize,
    pub download: bool,
    pub paperrust: Option<PathBuf>,
    pub threads: usize,
    pub quiet: bool,
    pub policy: Policy,
    pub skip_unknown_tables: bool,
    /// no backup before catching up in place (same database)
    pub no_backup: bool,
    /// Folder for the backup (default: report folder)
    pub backup_dir: Option<PathBuf>,
}

pub const USAGE: &str = "\
paperrust-migrate-db switch – von paperless-ngx umsteigen (ein Schritt, mit Prüfbericht)

Aufruf:
  paperrust-migrate-db switch  --from <QUELLE> [--media <ORDNER>] [--target <PG-URL>] [Optionen]
  paperrust-migrate-db switch  --dry-run --from <QUELLE> …   nur prüfen, nichts schreiben
  paperrust-migrate-db verify  --from <QUELLE> --target <PG-URL> --target-media <ORDNER> [--api-url …]
  paperrust-migrate-db detect  --from <QUELLE>                Quelle erkennen (JSON)

QUELLE (wird erkannt, nur gelesen):
  Export-Ordner mit manifest.json oder Export-ZIP (document_exporter, jede neuere Version)
  Datenordner mit db.sqlite3 (bzw. paperless-Ordner mit data/ und media/) oder die SQLite-Datei
  Datenbank-URL: sqlite:///…, postgres://…, mysql://…, mariadb://…  (paperless-ngx 3.0.0 bis 3.4.2;
                 ältere Stände ab 3.0 werden beim Umstieg selbst nachgeholt)

Optionen:
  --media <ORDNER>        Medienordner der Quelle (MEDIA_ROOT); bei SQLite-Ordnern automatisch
  --target <PG-URL>       Ziel-Datenbank (leer); sonst PAPERRUST_MIGRATE_TARGET oder PAPERRUST_DB*
  --target-media <ORDNER> Medienordner des Ziels; Standard: PAPERRUST_MEDIA_ROOT, beim Datenbank-
                          Umzug sonst derselbe Ordner wie die Quelle (Docker: gleiches Volume)
  --report-dir <ORDNER>   Bericht umstieg-bericht.{json,txt,html}; Standard: <DATA_DIR>/umstieg bzw. .
  --api-url <URL>         Stichprobe über API v1 des laufenden PaperRust (z. B. http://127.0.0.1:8000)
  --api-token <TOKEN>     API-Token (oder PAPERRUST_MIGRATE_API_TOKEN)
  --api-temp-token        Token eines Superusers aus der Ziel-DB nehmen bzw. kurz anlegen
  --sample <N>            Stichprobengröße (Standard 10, 0 = aus)
  --no-download           Stichprobe ohne Herunterladen der Originale
  --paperrust <PROGRAMM>  paperrust-Programm für den Export-Import (Standard: daneben bzw. PATH)
  --threads <N>           parallele Prüfsummen-Leser (Standard 4)
  --nul …, --invalid-utf8 …, --skip-unknown-tables   wie beim Datenbank-Umzug
  --backup-dir <ORDNER>   wohin die Sicherung (pg_dump) vor Änderungen an der bestehenden Datenbank
                          geht (nur wenn Quelle = Ziel und die Quelle älter ist); Standard: Berichtsordner
  --no-backup             diese Sicherung auslassen (nur, wenn schon selbst gesichert)
  --quiet

Exit-Code: 0 = OK, 3 = WARN (Hinweise), 1 = FAIL, 2 = falscher Aufruf.
";

fn env_var(get: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    get(&format!("PAPERRUST_{name}")).or_else(|| get(&format!("PAPERLESS_{name}"))).filter(|s| !s.is_empty())
}

pub fn parse(cmd: &str, args: &[String], get: &dyn Fn(&str) -> Option<String>) -> Result<SwitchOptions, String> {
    let mut o = SwitchOptions {
        cmd: match cmd {
            "switch" => Cmd::Switch,
            "verify" => Cmd::Verify,
            "detect" => Cmd::Detect,
            c => return Err(format!("unbekannter Befehl {c}")),
        },
        from: String::new(),
        media: None,
        target: None,
        target_media: None,
        report_dir: PathBuf::new(),
        api_url: None,
        api_token: env_var(get, "MIGRATE_API_TOKEN"),
        api_temp_token: false,
        sample: 10,
        download: true,
        paperrust: None,
        threads: 4,
        quiet: false,
        policy: Policy { nul: NulPolicy::Fail, utf8: Utf8Policy::Fail },
        skip_unknown_tables: false,
        no_backup: false,
        backup_dir: None,
    };
    let mut from = None;
    let mut report_dir = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |n: &str| it.next().cloned().ok_or_else(|| format!("{n} braucht einen Wert"));
        match a.as_str() {
            "--from" | "--source" => from = Some(val(a)?),
            "--media" => o.media = Some(PathBuf::from(val(a)?)),
            "--target" => o.target = Some(val(a)?),
            "--target-media" => o.target_media = Some(PathBuf::from(val(a)?)),
            "--report-dir" => report_dir = Some(PathBuf::from(val(a)?)),
            "--api-url" => o.api_url = Some(val(a)?),
            "--api-token" => o.api_token = Some(val(a)?),
            "--api-temp-token" => o.api_temp_token = true,
            "--sample" => o.sample = val(a)?.parse().map_err(|_| "--sample: Zahl erwartet")?,
            "--no-download" => o.download = false,
            "--paperrust" => o.paperrust = Some(PathBuf::from(val(a)?)),
            "--threads" => o.threads = val(a)?.parse::<usize>().map_err(|_| "--threads: Zahl erwartet")?.clamp(1, 64),
            "--dry-run" if o.cmd == Cmd::Switch => o.cmd = Cmd::DryRun,
            "--quiet" => o.quiet = true,
            "--skip-unknown-tables" => o.skip_unknown_tables = true,
            "--no-backup" => o.no_backup = true,
            "--backup-dir" => o.backup_dir = Some(PathBuf::from(val(a)?)),
            "--nul" => {
                o.policy.nul = match val(a)?.as_str() {
                    "fail" => NulPolicy::Fail,
                    "replace" => NulPolicy::Replace,
                    "strip" => NulPolicy::Strip,
                    v => return Err(format!("--nul: unbekannter Wert {v}")),
                }
            }
            "--invalid-utf8" => {
                o.policy.utf8 = match val(a)?.as_str() {
                    "fail" => Utf8Policy::Fail,
                    "replace" => Utf8Policy::Replace,
                    v => return Err(format!("--invalid-utf8: unbekannter Wert {v}")),
                }
            }
            "-h" | "--help" => return Err(String::new()),
            x => return Err(format!("unbekannte Option {x}")),
        }
    }
    o.from = from.or_else(|| env_var(get, "MIGRATE_SOURCE")).ok_or("--from fehlt (Export, Datenordner oder Datenbank-URL)")?;
    o.target = o.target.or_else(|| env_var(get, "MIGRATE_TARGET")).or_else(|| detect::target_from_env(|k| get(k)));
    if let Some(t) = &o.target
        && !(t.starts_with("postgres://") || t.starts_with("postgresql://"))
    {
        return Err("--target muss eine PostgreSQL-URL sein (postgres://…)".into());
    }
    if matches!(o.cmd, Cmd::Switch | Cmd::Verify) && o.target.is_none() {
        return Err("Ziel fehlt: --target postgres://… (oder PAPERRUST_DBHOST/… bzw. PAPERRUST_MIGRATE_TARGET)".into());
    }
    if o.api_url.is_some() && o.api_token.is_none() && !o.api_temp_token {
        return Err("--api-url braucht --api-token, PAPERRUST_MIGRATE_API_TOKEN oder --api-temp-token".into());
    }
    if o.target_media.is_none() {
        o.target_media = env_var(get, "MEDIA_ROOT").map(PathBuf::from);
    }
    o.report_dir =
        report_dir.or_else(|| env_var(get, "DATA_DIR").map(|d| PathBuf::from(d).join("umstieg"))).unwrap_or_else(|| PathBuf::from("."));
    Ok(o)
}

fn step(rep: &mut Report, quiet: bool, level: Level, name: &str, msg: impl Into<String>) {
    let msg = msg.into();
    if !quiet {
        eprintln!("[{:<4}] {name}: {msg}", level.as_str());
    }
    rep.step(level, name, msg);
}

/// Copy a folder recursively, existing target files are kept (no overwriting), symlinks
/// are skipped. Returns: (copied, already present).
#[cfg(test)]
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<(u64, u64)> {
    crate::assist::copy_tree_progress(from, to, 0, true)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Environment for `paperrust manage …` from the target URL (PAPERRUST_DB* removed, so that nothing
/// foreign takes precedence).
pub fn manage_env(target: &str, media: Option<&Path>) -> Result<Vec<(String, String)>> {
    let u = url::Url::parse(target).context("Ziel-URL ungültig")?;
    let dec = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
    let mut env = vec![
        ("PAPERLESS_DBENGINE".to_string(), "postgresql".to_string()),
        ("PAPERLESS_DBHOST".into(), u.host_str().unwrap_or("localhost").trim_matches(['[', ']']).to_string()),
        ("PAPERLESS_DBPORT".into(), u.port().unwrap_or(5432).to_string()),
        ("PAPERLESS_DBNAME".into(), dec(u.path().trim_start_matches('/'))),
        ("PAPERLESS_DBUSER".into(), dec(u.username())),
        ("PAPERLESS_DBPASS".into(), dec(u.password().unwrap_or(""))),
    ];
    if let Some((_, m)) = u.query_pairs().find(|(k, _)| k == "sslmode") {
        env.push(("PAPERLESS_DBSSLMODE".into(), m.into_owned()));
    }
    if let Some(m) = media {
        env.push(("PAPERLESS_MEDIA_ROOT".into(), m.display().to_string()));
    }
    Ok(env)
}

fn paperrust_bin(o: &SwitchOptions) -> PathBuf {
    if let Some(p) = &o.paperrust {
        return p.clone();
    }
    let name = format!("paperrust{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(&name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

async fn run_manage(o: &SwitchOptions, target: &str, args: &[&str]) -> Result<()> {
    let bin = paperrust_bin(o);
    let mut c = tokio::process::Command::new(&bin);
    c.arg("manage").args(args);
    for (k, _) in std::env::vars() {
        if k.starts_with("PAPERRUST_DB") || k == "PAPERRUST_MEDIA_ROOT" {
            c.env_remove(k);
        }
    }
    for (k, v) in manage_env(target, o.target_media.as_deref())? {
        c.env(k, v);
    }
    c.stdin(std::process::Stdio::null());
    if o.quiet {
        c.stdout(std::process::Stdio::null());
    }
    let st = c.status().await.with_context(|| format!("{} lässt sich nicht starten (--paperrust angeben)", bin.display()))?;
    if !st.success() {
        bail!("paperrust manage {} fehlgeschlagen ({st})", args.join(" "));
    }
    Ok(())
}

/// What was read from the source.
struct SourceData {
    counts: Counts,
    docs: Vec<DocRow>,
}

async fn read_db(url: &str) -> Result<SourceData> {
    let mut src = Source::connect(url).await?;
    let r = async {
        let counts = verify::db_counts(&mut src).await?;
        let docs = verify::db_docs(&mut src).await?;
        Ok(SourceData { counts, docs })
    }
    .await;
    src.close().await;
    r
}

/// Migration state of the source.
enum SchemaState {
    /// exactly v3.4.2 (or newer with known migrations, see `schema::KNOWN_NEWER`)
    Current(Vec<(String, String)>),
    /// older, can be caught up (v2.20, v3.0.0 … v3.4.1)
    CatchUp(crate::catchup::Plan),
    /// cannot be adopted (with explanation)
    Fail(String),
}

async fn schema_check(url: &str) -> Result<SchemaState> {
    let mut src = Source::connect(url).await?;
    let r = async {
        if !src.tables().await?.contains("django_migrations") {
            return Ok(SchemaState::Fail("keine Tabelle django_migrations – ist das eine paperless-ngx-Datenbank?".to_string()));
        }
        let mc = crate::schema::check_migrations(&src.migrations().await?);
        if mc.ok() {
            return Ok(SchemaState::Current(mc.newer));
        }
        Ok(match crate::catchup::plan(&mc) {
            Ok(p) if p.is_v2() => match crate::catchup::precheck_v2(&mut src).await? {
                Some(msg) => SchemaState::Fail(msg),
                None => SchemaState::CatchUp(p),
            },
            Ok(p) => SchemaState::CatchUp(p),
            Err(msg) => SchemaState::Fail(msg),
        })
    }
    .await;
    src.close().await;
    r
}

/// Target reachable? Returns the PostgreSQL version.
async fn target_ping(url: &str) -> Result<String> {
    let mut c = migrate::connect_target(url).await?;
    let v: String = sqlx::query_scalar("SELECT current_setting('server_version')").fetch_one(&mut c).await?;
    let _ = sqlx::Connection::close(c).await;
    Ok(v)
}

/// Catch up missing migrations in place (source = target), backup first.
async fn catch_up_in_place(
    o: &SwitchOptions,
    rep: &mut Report,
    url: &str,
    plan: &crate::catchup::Plan,
    media: Option<&Path>,
) -> Result<bool> {
    let q = o.quiet;
    if o.no_backup {
        step(rep, q, Level::Warn, "Sicherung", "ausgelassen (--no-backup) – Sie haben selbst gesichert");
    } else {
        // 1. pg_dump file (if the program is there), 2. otherwise copy of the database on the server
        let get = |k: &str| std::env::var(k).ok();
        let mut why: Vec<String> = Vec::new();
        let mut done = false;
        if let Some(pg_dump) = crate::assist::find_pg_dump(&get) {
            let dir = o.backup_dir.clone().unwrap_or_else(|| o.report_dir.clone());
            if !q {
                eprintln!("[....] Sicherung der Datenbank mit pg_dump …");
            }
            match crate::assist::pg_dump_backup(&pg_dump, url, &dir).await {
                Ok(f) => {
                    step(
                        rep,
                        q,
                        Level::Ok,
                        "Sicherung",
                        format!(
                            "{} ({}); zurückspielen mit pg_restore --clean",
                            f.display(),
                            crate::assist::human(std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0))
                        ),
                    );
                    done = true;
                }
                Err(e) => why.push(format!("{e:#}")),
            }
        } else {
            why.push("pg_dump nicht gefunden".into());
        }
        if !done {
            if !q {
                eprintln!("[....] Sicherung als Kopie der Datenbank auf dem Server …");
            }
            match crate::assist::template_copy(url).await {
                Ok(name) => {
                    step(
                        rep,
                        q,
                        Level::Ok,
                        "Sicherung",
                        format!(
                            "vollständige Kopie der Datenbank auf dem Server: {name} (zurück: diese Datenbank statt der alten verwenden)"
                        ),
                    );
                    done = true;
                }
                Err(e) => why.push(format!("{e:#}")),
            }
        }
        if !done {
            step(
                rep,
                q,
                Level::Fail,
                "Sicherung",
                format!(
                    "Die bestehende Datenbank würde verändert (nachholen), aber PaperRust konnte nicht selbst sichern ({}). \
                     Nichts verändert. Bitte selbst sichern (Docker: `docker compose exec db pg_dump -U paperless -Fc paperless \
                     > vorher.pgdump`) und dann mit --no-backup erneut starten.",
                    why.join("; ")
                ),
            );
            return Ok(false);
        }
    }
    let mut c = migrate::connect_target(url).await?;
    if !q && plan.is_v2() {
        eprintln!("[....] Umbau paperless-ngx 2.x → {} ({} Schritte, SHA-256 aller Dateien) …", crate::schema::VERSION, plan.steps.len());
    }
    let r = crate::catchup::apply_in_place(&mut c, plan, media).await;
    let _ = sqlx::Connection::close(c).await;
    match r {
        Ok(applied) => {
            step(
                rep,
                q,
                Level::Ok,
                "Nachholen",
                format!(
                    "{} Änderung(en) bis paperless-ngx {} übernommen: {}",
                    applied.done.len(),
                    crate::schema::VERSION,
                    applied.done.join(", ")
                ),
            );
            for n in applied.notes {
                step(rep, q, Level::Warn, "Nachholen", n);
            }
            Ok(true)
        }
        Err(e) => {
            step(rep, q, Level::Fail, "Nachholen", format!("{e:#} – zurückgerollt, Datenbank unverändert"));
            Ok(false)
        }
    }
}

pub async fn run(o: &SwitchOptions) -> Report {
    let mut rep = Report {
        tool_version: env!("CARGO_PKG_VERSION").into(),
        mode: match o.cmd {
            Cmd::Switch => "switch",
            Cmd::DryRun => "dry-run",
            Cmd::Verify => "verify",
            Cmd::Detect => "detect",
        }
        .into(),
        started: chrono::Utc::now().to_rfc3339(),
        target: o.target.as_deref().map(redact),
        ..Default::default()
    };
    let t0 = Instant::now();
    if let Err(e) = run_inner(o, &mut rep).await {
        step(&mut rep, o.quiet, Level::Fail, "Abbruch", format!("{e:#}"));
    }
    rep.finished = chrono::Utc::now().to_rfc3339();
    rep.duration_s = t0.elapsed().as_secs_f64();
    rep.finish();
    rep
}

async fn run_inner(o: &SwitchOptions, rep: &mut Report) -> Result<()> {
    let q = o.quiet;
    // ---- 1. Detect the source ----------------------------------------------------------------
    let mut det: Detection = detect::detect(&o.from, o.media.as_deref())?;
    // Same database (Docker: only the image swapped): media folder of the source = that of the target
    if det.media.is_none()
        && let (Some(u), Some(t)) = (&det.url, &o.target)
        && detect::same_database(u, t)
    {
        det.media = o.target_media.clone();
    }
    rep.source_kind = det.kind.as_str().into();
    rep.source_label = det.kind.label().into();
    rep.source = match (&det.export, &det.url) {
        (Some(p), _) => p.display().to_string(),
        (_, Some(u)) => redact(u),
        _ => String::new(),
    };
    rep.source_media = det.media.as_ref().map(|p| p.display().to_string());
    step(rep, q, Level::Ok, "Quelle erkannt", format!("{} – {}", det.kind.label(), rep.source));
    for n in &det.notes {
        step(rep, q, Level::Info, "Hinweis", n.clone());
    }

    // ---- 2. Read the source (read-only) ------------------------------------------------------
    let export = match &det.export {
        Some(p) => Some(ExportSource::open(p)?),
        None => None,
    };
    let mut catchup: Option<crate::catchup::Plan> = None;
    let src = if let Some(exp) = &export {
        rep.source_version = exp.version();
        if exp.is_v1_export() {
            step(
                rep,
                q,
                Level::Fail,
                "Version",
                format!(
                    "Export aus paperless-ngx 1.x ({}) lässt sich nicht übernehmen: er enthält Felder, die es seit                      paperless-ngx 2.0 nicht mehr gibt (auch paperless-ngx {} lehnt ihn ab). Nichts verändert. Weg:                      paperless-ngx zuerst auf 2.20 aktualisieren und einmal starten, dann direkt über die Datenbank                      umsteigen oder neu exportieren (docs/user/umstieg.md).",
                    rep.source_version.as_deref().unwrap_or("ohne Versionsangabe"),
                    crate::schema::VERSION
                ),
            );
            return Ok(());
        }
        match rep.source_version.as_deref() {
            Some(v) if v.trim_start_matches('v') == crate::schema::VERSION => {
                step(rep, q, Level::Ok, "Version", format!("Export aus paperless-ngx {v}"))
            }
            Some(v) if v.trim_start_matches('v').starts_with("3.") => step(
                rep,
                q,
                Level::Info,
                "Version",
                format!(
                    "Export aus paperless-ngx {v} (PaperRust entspricht {}); der Import liest ihn, der Bericht zeigt jede Abweichung",
                    crate::schema::VERSION
                ),
            ),
            Some(v) => step(
                rep,
                q,
                Level::Warn,
                "Version",
                format!(
                    "Export aus paperless-ngx {v} – nicht {}; der Import wird versucht, Ergebnis genau prüfen (empfohlen: paperless-ngx vorher aktualisieren)",
                    crate::schema::VERSION
                ),
            ),
            None => step(rep, q, Level::Warn, "Version", "Export ohne metadata.json – Version unbekannt (sehr alter Export?)"),
        }
        let sum = exp.summarize()?;
        step(rep, q, Level::Ok, "Manifest gelesen", format!("{} Manifest-Datei(en), {} Dokumente", sum.manifests.len(), sum.docs.len()));
        SourceData { counts: verify::export_counts(&sum), docs: sum.docs }
    } else {
        let url = det.url.as_deref().ok_or_else(|| anyhow!("Quelle ohne URL"))?;
        match schema_check(url).await? {
            SchemaState::Current(newer) if newer.is_empty() => {
                rep.source_version = Some(crate::schema::VERSION.into());
                step(rep, q, Level::Ok, "Schema", format!("Migrationsstand entspricht paperless-ngx {}", crate::schema::VERSION));
            }
            SchemaState::Current(newer) => {
                rep.source_version = Some(format!("neuer als {}", crate::schema::VERSION));
                step(
                    rep,
                    q,
                    Level::Info,
                    "Schema",
                    format!(
                        "neuere paperless-ngx-Version als {} mit bekannten Änderungen ({}) – wird so übernommen \
                         (Tags ohne die alten treenode-Spalten, wie PaperRust selbst)",
                        crate::schema::VERSION,
                        newer.iter().map(|(a, n)| format!("{a}.{n}")).collect::<Vec<_>>().join(", ")
                    ),
                );
            }
            SchemaState::CatchUp(p) => {
                let v = crate::catchup::guess_version(&p);
                rep.source_version = Some(v.into());
                step(
                    rep,
                    q,
                    Level::Info,
                    "Schema",
                    format!(
                        "ältere Version (etwa paperless-ngx {v}) – {} Änderung(en) bis {} holt PaperRust selbst nach: {}",
                        p.steps.len(),
                        crate::schema::VERSION,
                        p.names().join(", ")
                    ),
                );
                catchup = Some(p);
            }
            SchemaState::Fail(msg) => {
                step(rep, q, Level::Fail, "Schema", msg);
                return Ok(());
            }
        }
        let d = read_db(url).await?;
        step(rep, q, Level::Ok, "Quelle gelesen", format!("{} Dokumente", d.docs.len()));
        d
    };

    // Files of the source
    let src_files = match (&export, &det.media) {
        (Some(exp), _) => Some(verify::check_files(&Files::Export(exp), &src.docs, o.threads)),
        (None, Some(m)) => Some(verify::check_files(&Files::Media(m), &src.docs, o.threads)),
        (None, None) => None,
    };
    if let Some(f) = &src_files {
        let level = if f.problems.is_empty() { Level::Ok } else { Level::Warn };
        step(
            rep,
            q,
            level,
            "Dateien der Quelle",
            format!("{} geprüft, {} Befund(e)", f.originals + f.archives + f.thumbnails, f.problems.len()),
        );
    }
    rep.source_files = src_files;

    let same_db = matches!((&det.url, &o.target), (Some(s), Some(t)) if detect::same_database(s, t));
    let target_media: Option<PathBuf> = o.target_media.clone().or_else(|| if export.is_none() { det.media.clone() } else { None });
    rep.target_media = target_media.as_ref().map(|p| p.display().to_string());

    // ---- Up front: target reachable, enough space, backup ------------------------------------
    if matches!(o.cmd, Cmd::Switch | Cmd::DryRun) {
        if let Some(t) = &o.target {
            match target_ping(t).await {
                Ok(v) => step(rep, q, Level::Ok, "Verbindung", format!("Ziel-Datenbank erreichbar (PostgreSQL {v})")),
                Err(e) => {
                    step(rep, q, Level::Fail, "Verbindung", format!("Ziel-Datenbank nicht erreichbar: {e:#}"));
                    return Ok(());
                }
            }
        }
        // Space for the document files: export → media folder of the target, media folder → copy
        let copy: Option<(PathBuf, PathBuf)> = match (&det.export, &det.media, &target_media) {
            (Some(p), _, Some(t)) => Some((p.clone(), t.clone())),
            (None, Some(m), Some(t)) if !same_dir(m, t) => Some((m.clone(), t.clone())),
            _ => None,
        };
        if let Some((from, to)) = copy {
            let (need, files) = crate::assist::dir_size(&from);
            match crate::assist::free_space(&to) {
                Some(free) if free < need + need / 20 + (64 << 20) => {
                    step(
                        rep,
                        q,
                        Level::Fail,
                        "Platz",
                        format!(
                            "zu wenig Platz für die Dateien: nötig etwa {}, frei {} unter {} – bitte Platz schaffen oder einen anderen Ordner (--target-media) wählen",
                            crate::assist::human(need),
                            crate::assist::human(free),
                            to.display()
                        ),
                    );
                    return Ok(());
                }
                Some(free) => step(
                    rep,
                    q,
                    Level::Ok,
                    "Platz",
                    format!(
                        "{files} Dateien ({}) passen, frei {} unter {}",
                        crate::assist::human(need),
                        crate::assist::human(free),
                        to.display()
                    ),
                ),
                None => step(rep, q, Level::Info, "Platz", format!("freier Platz unter {} nicht ermittelbar", to.display())),
            }
        }
        if same_db && catchup.is_some() {
            let get = |k: &str| std::env::var(k).ok();
            let msg = match (o.no_backup, crate::assist::find_pg_dump(&get)) {
                (true, _) => "Ihre bestehende Datenbank wird beim Umstieg verändert (nachholen); Sicherung ausgelassen (--no-backup)".to_string(),
                (false, Some(p)) => format!(
                    "Ihre bestehende Datenbank wird beim Umstieg verändert (nachholen) – vorher wird automatisch gesichert ({})",
                    p.display()
                ),
                (false, None) => "Ihre bestehende Datenbank wird beim Umstieg verändert (nachholen) – vorher legt PaperRust eine Kopie der Datenbank auf dem Server an (pg_dump nicht gefunden)".to_string(),
            };
            step(rep, q, Level::Info, "Sicherung", msg);
        } else {
            step(
                rep,
                q,
                Level::Info,
                "Sicherung",
                "Die Quelle wird nur gelesen und nicht verändert. Eine eigene Sicherung vorher ist trotzdem empfehlenswert.",
            );
        }
    }

    // ---- 3. Dry run -------------------------------------------------------------------------
    if o.cmd == Cmd::DryRun {
        rep.set_counts(&src.counts, None);
        if let Some(url) = &det.url {
            if same_db {
                step(
                    rep,
                    q,
                    Level::Info,
                    "Datenbank",
                    if catchup.is_some() {
                        "Quelle und Ziel sind dieselbe Datenbank – beim Umstieg wird gesichert und nachgeholt, sonst nichts kopiert"
                    } else {
                        "Quelle und Ziel sind dieselbe Datenbank – nichts zu kopieren, nur prüfen"
                    },
                );
            } else {
                let mut db = DbReport::default();
                let opts = db_options(o, url, o.target.clone(), Mode::DryRun, det.media.clone());
                match migrate::run(&opts, &mut db).await {
                    Ok(()) if db.result == "dry-run-ok" => step(
                        rep,
                        q,
                        Level::Ok,
                        "Probelauf Datenbank",
                        format!("{} Tabellen vollständig gelesen und umgewandelt, Ziel bereit", db.tables.len()),
                    ),
                    Ok(()) => step(rep, q, Level::Fail, "Probelauf Datenbank", db_problems(&db)),
                    Err(e) => step(rep, q, Level::Fail, "Probelauf Datenbank", format!("{e:#}")),
                }
            }
        } else {
            if let Some(t) = &o.target {
                match target_documents(t).await {
                    Ok(Some(n)) if n > 0 => {
                        step(rep, q, Level::Fail, "Ziel", format!("Ziel-Datenbank enthält schon {n} Dokumente – Import würde abbrechen"))
                    }
                    Ok(_) => step(rep, q, Level::Ok, "Ziel", "Ziel-Datenbank erreichbar und ohne Dokumente"),
                    Err(e) => step(rep, q, Level::Fail, "Ziel", format!("{e:#}")),
                }
            }
            let bin = paperrust_bin(o);
            if o.paperrust.is_some() && !bin.is_file() {
                step(rep, q, Level::Fail, "paperrust", format!("{} nicht gefunden", bin.display()));
            }
        }
        rep.file_findings();
        rep.spot.status = "skipped".into();
        rep.spot.message = Some("Probelauf".into());
        return Ok(());
    }
    if o.cmd == Cmd::Detect {
        return Ok(());
    }
    let target = o.target.clone().ok_or_else(|| anyhow!("Ziel fehlt"))?;

    // ---- 4. Umstieg -------------------------------------------------------------------------
    if o.cmd == Cmd::Switch {
        if let Some(exp_path) = &det.export {
            let p = exp_path.display().to_string();
            run_manage(o, &target, &["migrate"]).await.context("Ziel-Schema anlegen")?;
            match run_manage(o, &target, &["document_importer", &p]).await {
                Ok(()) => step(rep, q, Level::Ok, "Import", "document_importer abgeschlossen"),
                Err(e) => {
                    step(rep, q, Level::Fail, "Import", format!("{e:#}"));
                    return Ok(());
                }
            }
        } else if same_db {
            match &catchup {
                Some(p) => {
                    let url = det.url.as_deref().unwrap_or_default();
                    if !catch_up_in_place(o, rep, url, p, det.media.as_deref()).await? {
                        return Ok(());
                    }
                }
                None => step(rep, q, Level::Info, "Datenbank", "Quelle und Ziel sind dieselbe Datenbank – nichts zu kopieren, nur prüfen"),
            }
        } else {
            let url = det.url.as_deref().unwrap_or_default();
            let mut db = DbReport {
                tool_version: env!("CARGO_PKG_VERSION").into(),
                mode: "migrate".into(),
                source: redact(url),
                target: Some(redact(&target)),
                started: chrono::Utc::now().to_rfc3339(),
                ..Default::default()
            };
            let opts = db_options(o, url, Some(target.clone()), Mode::Migrate, det.media.clone());
            let r = migrate::run(&opts, &mut db).await;
            if let Err(e) = &r {
                db.error = Some(format!("{e:#}"));
                db.result = "failed".into();
            }
            let _ = std::fs::create_dir_all(&o.report_dir);
            let _ = std::fs::write(
                o.report_dir.join("migrate-db-report.json"),
                serde_json::to_string_pretty(&db.to_json()).unwrap_or_default(),
            );
            if !db.ok() {
                step(
                    rep,
                    q,
                    Level::Fail,
                    "Datenbank-Umzug",
                    format!("{} – Ziel zurückgerollt (leer), Quelle unverändert", db.error.clone().unwrap_or_else(|| db_problems(&db))),
                );
                return Ok(());
            }
            let rows: i64 = db.tables.iter().map(|t| t.source_rows).sum();
            step(
                rep,
                q,
                Level::Ok,
                "Datenbank-Umzug",
                format!("{} Tabellen, {rows} Zeilen übernommen, Prüfsumme je Tabelle gleich", db.tables.len()),
            );
            if catchup.as_ref().is_some_and(|p| p.is_v2()) {
                step(
                    rep,
                    q,
                    Level::Ok,
                    "Nachholen",
                    format!(
                        "im Ziel auf paperless-ngx {} umgebaut ({} Änderungen), Schema geprüft",
                        crate::schema::VERSION,
                        db.catchup.len()
                    ),
                );
                for w in db.warnings.iter().filter(|w| w.starts_with("SHA-256")) {
                    step(rep, q, Level::Warn, "Nachholen", w.clone());
                }
            }
        }
        // Media folder (database route only; the import copies by itself)
        if export.is_none() {
            match (&det.media, &target_media) {
                (Some(from), Some(to)) if !same_dir(from, to) => {
                    let (from2, to2, quiet) = (from.clone(), to.clone(), q);
                    let total = crate::assist::dir_size(from).1;
                    let (c, k) = tokio::task::spawn_blocking(move || crate::assist::copy_tree_progress(&from2, &to2, total, quiet))
                        .await
                        .map_err(|e| anyhow!("Kopieren: {e}"))?
                        .with_context(|| format!("{} → {} kopieren", from.display(), to.display()))?;
                    step(rep, q, Level::Ok, "Dateien kopiert", format!("{c} kopiert, {k} schon vorhanden (nicht überschrieben)"));
                }
                (Some(_), Some(_)) => step(rep, q, Level::Info, "Dateien", "Ziel nutzt denselben Medienordner – nichts zu kopieren"),
                _ => step(
                    rep,
                    q,
                    Level::Warn,
                    "Dateien",
                    "Medienordner der Quelle unbekannt – Dokumentdateien nicht kopiert (--media angeben)",
                ),
            }
        }
    }

    // ---- 5. Verify ---------------------------------------------------------------------------
    let tgt = read_db(&target).await.context("Ziel lesen")?;
    rep.set_counts(&src.counts, Some(&tgt.counts));
    rep.compare_docs(&src.docs, &tgt.docs);
    let bad_counts = rep.counts.iter().filter(|c| c.level() == Level::Fail).count();
    step(
        rep,
        q,
        if bad_counts == 0 { Level::Ok } else { Level::Fail },
        "Vergleich",
        format!("{} Objektarten, {} Dokumentzeilen verglichen", rep.counts.len(), src.docs.len()),
    );
    match &target_media {
        Some(m) => {
            let f = verify::check_files(&Files::Media(m), &tgt.docs, o.threads);
            rep.extra_files = verify::extra_files(m, &tgt.docs);
            step(
                rep,
                q,
                if f.problems.is_empty() { Level::Ok } else { Level::Warn },
                "Dateien des Ziels",
                format!(
                    "{} geprüft, {} Befund(e), {} verwaist",
                    f.originals + f.archives + f.thumbnails,
                    f.problems.len(),
                    rep.extra_files.len()
                ),
            );
            rep.target_files = Some(f);
        }
        None => step(rep, q, Level::Warn, "Dateien des Ziels", "Ziel-Medienordner unbekannt – nicht geprüft (--target-media)"),
    }
    rep.file_findings();

    // ---- 6. Sample via API v1 ----------------------------------------------------------------
    match &o.api_url {
        Some(api) if o.sample > 0 => {
            let token = match &o.api_token {
                Some(t) => Ok(crate::api::TempToken { key: t.clone(), created: false }),
                None => crate::api::temp_token(&target).await,
            };
            match token {
                Ok(t) => {
                    let sample = crate::api::pick_sample(&src.docs, o.sample);
                    rep.spot = crate::api::spot_check(api, &t.key, &src.docs, &sample, o.download).await;
                    if let Err(e) = crate::api::drop_temp_token(&target, &t).await {
                        step(rep, q, Level::Warn, "API-Token", format!("vorübergehender Token nicht gelöscht: {e:#}"));
                    }
                }
                Err(e) => {
                    rep.spot.status = "error".into();
                    rep.spot.message = Some(format!("{e:#}"));
                }
            }
            rep.spot_findings();
            if !q {
                eprintln!("[....] Stichprobe API v1: {} ({} Dokumente)", rep.spot.status, rep.spot.rows.len());
            }
        }
        _ => {
            rep.spot.status = "skipped".into();
            rep.spot.message = Some("ohne --api-url; nach dem Start von PaperRust mit `verify --api-url …` nachholen".into());
        }
    }
    Ok(())
}

fn db_options(o: &SwitchOptions, source: &str, target: Option<String>, mode: Mode, media: Option<PathBuf>) -> Options {
    Options {
        source: source.to_string(),
        target,
        mode,
        policy: o.policy,
        batch_bytes: 4 << 20,
        skip_unknown_tables: o.skip_unknown_tables,
        quiet: o.quiet,
        max_issues: 200,
        media,
    }
}

fn db_problems(db: &DbReport) -> String {
    let mut v: Vec<String> = db.problems.clone();
    for t in &db.tables {
        if t.errors > 0 {
            v.push(format!("{}: {} Umwandlungsfehler", t.name, t.errors));
        }
    }
    if let Some(e) = &db.error {
        v.push(e.clone());
    }
    if v.is_empty() { db.result.clone() } else { v.join("; ") }
}

async fn target_documents(target: &str) -> Result<Option<i64>> {
    let mut src = Source::connect(target).await?;
    let r = async {
        if !src.tables().await?.contains("documents_document") {
            return Ok(None);
        }
        Ok(Some(src.count_where("documents_document", None).await?))
    }
    .await;
    src.close().await;
    r
}

/// `detect`: detection as JSON, for databases with migration state.
pub async fn detect_json(o: &SwitchOptions) -> serde_json::Value {
    match detect::detect(&o.from, o.media.as_deref()) {
        Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
        Ok(det) => {
            let mut v = det.to_json();
            v["ok"] = true.into();
            if let Some(p) = &det.export {
                match ExportSource::open(p) {
                    Ok(e) => v["paperless_version"] = e.version().into(),
                    Err(e) => {
                        v["ok"] = false.into();
                        v["error"] = format!("{e:#}").into();
                    }
                }
                v["path"] = "import".into();
            } else if let Some(u) = &det.url {
                v["path"] = "migrate-db".into();
                match schema_check(u).await {
                    Ok(SchemaState::Current(newer)) => {
                        v["schema"] = crate::schema::VERSION.into();
                        if !newer.is_empty() {
                            v["newer"] = newer.iter().map(|(a, n)| format!("{a}.{n}")).collect::<Vec<_>>().into();
                        }
                    }
                    Ok(SchemaState::CatchUp(p)) => {
                        v["schema"] = crate::catchup::guess_version(&p).into();
                        v["catchup"] = p.names().into();
                    }
                    Ok(SchemaState::Fail(m)) => {
                        v["ok"] = false.into();
                        v["error"] = m.into();
                    }
                    Err(e) => {
                        v["ok"] = false.into();
                        v["error"] = format!("{e:#}").into();
                    }
                }
            }
            v
        }
    }
}

/// Entry point for `switch`/`verify`/`detect` from main.
pub async fn main(cmd: &str, args: &[String]) -> i32 {
    let o = match parse(cmd, args, &|k| std::env::var(k).ok()) {
        Ok(o) => o,
        Err(e) if e.is_empty() => {
            print!("{USAGE}");
            return 0;
        }
        Err(e) => {
            eprintln!("Fehler: {e}\n\n{USAGE}");
            return 2;
        }
    };
    if o.cmd == Cmd::Detect {
        let v = detect_json(&o).await;
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return if v["ok"] == true { 0 } else { 1 };
    }
    let rep = tokio::select! {
        r = run(&o) => r,
        _ = tokio::signal::ctrl_c() => {
            eprintln!("abgebrochen (Strg+C) – Datenbank-Ziel wird zurückgerollt, Quelle unverändert");
            return 130;
        }
    };
    let text = rep.to_text();
    if !o.quiet {
        print!("{text}");
    }
    match rep.write(&o.report_dir) {
        Ok(files) => {
            if !o.quiet {
                for f in files {
                    println!("Bericht: {}", f.display());
                }
            }
        }
        Err(e) => eprintln!("Bericht in {} nicht schreibbar: {e}", o.report_dir.display()),
    }
    match rep.verdict() {
        Level::Ok | Level::Info => 0,
        Level::Warn => 3,
        Level::Fail => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_options_and_env() {
        let none = |_: &str| None;
        let o = parse("switch", &a(&["--from", "x", "--target", "postgres://h/db", "--dry-run"]), &none).unwrap();
        assert_eq!(o.cmd, Cmd::DryRun);
        assert_eq!(o.report_dir, PathBuf::from("."));
        assert!(parse("switch", &a(&["--from", "x"]), &none).unwrap_err().contains("Ziel fehlt"));
        // Dry run without a target allowed
        assert!(parse("switch", &a(&["--from", "x", "--dry-run"]), &none).is_ok());
        assert!(parse("verify", &a(&["--from", "x", "--target", "mysql://h"]), &none).is_err());
        assert!(parse("verify", &a(&["--from", "x", "--target", "postgres://h/d", "--api-url", "http://a"]), &none).is_err());
        assert!(
            parse("verify", &a(&["--from", "x", "--target", "postgres://h/d", "--dry-run"]), &none).is_err(),
            "--dry-run nur bei switch"
        );
        let env = |k: &str| match k {
            "PAPERLESS_DBHOST" => Some("db".into()),
            "PAPERLESS_MEDIA_ROOT" => Some("/usr/src/paperless/media".into()),
            "PAPERLESS_DATA_DIR" => Some("/usr/src/paperless/data".into()),
            "PAPERRUST_MIGRATE_SOURCE" => Some("/export".into()),
            _ => None,
        };
        let o = parse("switch", &[], &env).unwrap();
        assert_eq!(o.from, "/export");
        assert_eq!(o.target.as_deref(), Some("postgres://paperless:paperless@db:5432/paperless"));
        assert_eq!(o.target_media, Some(PathBuf::from("/usr/src/paperless/media")));
        assert_eq!(o.report_dir, PathBuf::from("/usr/src/paperless/data").join("umstieg"));
        let o = parse("verify", &a(&["--from", "x", "--target", "postgres://h/d", "--api-url", "http://a", "--api-temp-token"]), &none)
            .unwrap();
        assert!(o.api_temp_token);
    }

    #[test]
    fn manage_env_from_url() {
        let e = manage_env("postgres://us%40r:p%3Aw@127.0.0.1:54329/db1?sslmode=disable", Some(Path::new("/m"))).unwrap();
        let get = |k: &str| e.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        assert_eq!(get("PAPERLESS_DBUSER"), Some("us@r"));
        assert_eq!(get("PAPERLESS_DBPASS"), Some("p:w"));
        assert_eq!(get("PAPERLESS_DBPORT"), Some("54329"));
        assert_eq!(get("PAPERLESS_DBNAME"), Some("db1"));
        assert_eq!(get("PAPERLESS_DBSSLMODE"), Some("disable"));
        assert_eq!(get("PAPERLESS_MEDIA_ROOT"), Some("/m"));
    }

    #[test]
    fn copy_without_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let (src, dst) = (d.path().join("s"), d.path().join("t"));
        std::fs::create_dir_all(src.join("documents/originals")).unwrap();
        std::fs::write(src.join("documents/originals/a"), "neu").unwrap();
        std::fs::write(src.join("documents/originals/b"), "b").unwrap();
        std::fs::create_dir_all(dst.join("documents/originals")).unwrap();
        std::fs::write(dst.join("documents/originals/a"), "alt").unwrap();
        assert_eq!(copy_tree(&src, &dst).unwrap(), (1, 1));
        assert_eq!(std::fs::read_to_string(dst.join("documents/originals/a")).unwrap(), "alt");
        assert!(same_dir(&src, &src.join(".")));
    }

    /// End-to-end without a database: export folder in dry run, one original damaged.
    #[tokio::test]
    async fn dry_run_export_reports_corruption() {
        let d = tempfile::tempdir().unwrap();
        let exp = d.path().join("export");
        std::fs::create_dir_all(exp.join("originals")).unwrap();
        std::fs::write(exp.join("originals/1.pdf"), b"original").unwrap();
        std::fs::write(exp.join("originals/2.pdf"), b"beschaedigt").unwrap();
        let sum1 = crate::export::sha256_hex(b"original");
        let sum2 = crate::export::sha256_hex(b"echt");
        std::fs::write(
            exp.join("manifest.json"),
            format!(
                r#"[{{"model":"documents.document","pk":1,"fields":{{"title":"a","content":"","checksum":"{sum1}","tags":[]}},"__exported_file_name__":"originals/1.pdf"}},
                   {{"model":"documents.document","pk":2,"fields":{{"title":"b","content":"","checksum":"{sum2}","tags":[]}},"__exported_file_name__":"originals/2.pdf"}}]"#
            ),
        )
        .unwrap();
        std::fs::write(exp.join("metadata.json"), r#"{"version":"3.4.2"}"#).unwrap();
        let mut o = parse("switch", &a(&["--from", exp.to_str().unwrap(), "--dry-run", "--quiet"]), &|_| None).unwrap();
        o.report_dir = d.path().join("bericht");
        let rep = run(&o).await;
        assert_eq!(rep.verdict(), Level::Warn, "{}", rep.to_text());
        let j = rep.to_json();
        assert_eq!(j["files"]["source"]["problems"], 1);
        assert_eq!(j["files"]["source"]["problem_list"][0]["issue"], "checksum-mismatch");
        assert_eq!(j["files"]["source"]["problem_list"][0]["document"], 2);
        assert_eq!(j["counts"][0]["source"], 2);
        // Source unchanged
        assert_eq!(std::fs::read(exp.join("originals/2.pdf")).unwrap(), b"beschaedigt");

        // Database URL with the wrong schema: SQLite without django_migrations → FAIL with explanation
        let db = d.path().join("db.sqlite3");
        {
            use sqlx::Connection;
            let mut c =
                sqlx::SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&db).create_if_missing(true))
                    .await
                    .unwrap();
            sqlx::raw_sql(
                "CREATE TABLE django_migrations (id integer primary key, app text, name text, applied text); \
                           INSERT INTO django_migrations (app, name, applied) VALUES ('documents', '0001_initial', '2020-01-01')",
            )
            .execute(&mut c)
            .await
            .unwrap();
            c.close().await.unwrap();
        }
        let o = parse("switch", &a(&["--from", db.to_str().unwrap(), "--dry-run", "--quiet"]), &|_| None).unwrap();
        let rep = run(&o).await;
        assert_eq!(rep.verdict(), Level::Fail);
        let t = rep.to_text();
        assert!(t.contains("document_exporter") && t.contains("älter als"), "{t}");
        let v = detect_json(&o).await;
        assert_eq!(v["ok"], false);
        assert_eq!(v["kind"], "sqlite-folder");
    }
}
