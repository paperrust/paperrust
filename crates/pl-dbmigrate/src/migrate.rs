//! Flow: precheck → schema (pre-data) → data (COPY) → sequences → constraints/indexes
//! (post-data) → verification → COMMIT. Everything on the target in **one** transaction: on any error
//! (also abort/crash) PostgreSQL rolls everything back, the target stays empty.

use crate::catchup::{self, Adapt};
use crate::hash::{RowHash, TableHash, row_hash};
use crate::report::{Issue, Report, TableReport};
use crate::schema::{self, Column, Table};
use crate::source::Source;
use crate::value::{self, Conv, Policy, canon_from_pg};
use anyhow::{Context, Result, anyhow, bail};
use futures::StreamExt;
use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{Connection, Row};
use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Migrate,
    DryRun,
    VerifyOnly,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub source: String,
    pub target: Option<String>,
    pub mode: Mode,
    pub policy: Policy,
    pub batch_bytes: usize,
    pub skip_unknown_tables: bool,
    pub quiet: bool,
    /// Maximum stored individual findings per table (everything is always counted)
    pub max_issues: usize,
    /// Media folder of the source (only for converting a 2.x source: SHA-256 of the files)
    pub media: Option<std::path::PathBuf>,
}

/// Which schema is created and copied in the target.
pub struct Shape {
    pub version: &'static str,
    pub pre: &'static str,
    pub post: String,
    pub post_raw: &'static str,
    pub tables: Vec<Table>,
    /// documents_tag without the treenode columns (source with documents.0028 from paperless-ngx > 3.4.2)
    pub drop_tag_tree: bool,
    /// documents_note with the `format` column (source with documents.0030 from paperless-ngx 3.5)
    pub note_format: bool,
}

impl Shape {
    pub fn v342(drop_tag_tree: bool, note_format: bool) -> Result<Shape> {
        let mut tables = schema::tables().map_err(|e| anyhow!(e))?;
        if drop_tag_tree {
            for t in tables.iter_mut().filter(|t| t.name == "documents_tag") {
                t.columns.retain(|c| !schema::TREENODE_DERIVED.contains(&c.name.as_str()));
            }
        }
        if note_format {
            for t in tables.iter_mut().filter(|t| t.name == "documents_note") {
                t.columns.push(schema::note_format_column());
            }
        }
        Ok(Shape {
            version: schema::VERSION,
            pre: schema::PRE_DATA,
            post: schema::post_data(),
            post_raw: schema::POST_DATA,
            tables,
            drop_tag_tree,
            note_format,
        })
    }

    /// DDL applied right after pre-data for sources newer than v3.4.2 (known migrations only).
    pub fn newer_ddl(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.drop_tag_tree {
            // like documents.0028 (paperless-ngx > 3.4.2) or PaperRust pl_objects.0001
            let cols: Vec<String> = schema::TREENODE_DERIVED.iter().map(|c| format!("DROP COLUMN \"{c}\"")).collect();
            out.push(format!("ALTER TABLE public.documents_tag {}", cols.join(", ")));
        }
        if self.note_format {
            // exactly like documents.0030 on PostgreSQL (AddField with default, then the default is
            // dropped); PaperRust's own migration sets the default again on `manage migrate`
            out.push("ALTER TABLE public.documents_note ADD COLUMN \"format\" varchar(16) DEFAULT 'plain' NOT NULL".into());
            out.push("ALTER TABLE public.documents_note ALTER COLUMN \"format\" DROP DEFAULT".into());
        }
        out
    }

    pub fn v220() -> Result<Shape> {
        Ok(Shape {
            version: schema::v220::VERSION,
            pre: schema::v220::PRE_DATA,
            post: schema::v220::post_data(),
            post_raw: schema::v220::POST_DATA,
            tables: schema::v220::tables().map_err(|e| anyhow!(e))?,
            drop_tag_tree: false,
            note_format: false,
        })
    }
}

const MAX_DIFF_ROWS: usize = 20;

pub async fn connect_target(url: &str) -> Result<PgConnection> {
    let opts = PgConnectOptions::from_str(url).context("Ziel-URL ungültig (postgres://…)")?;
    let mut c = PgConnection::connect_with(&opts).await.context("Verbindung zur Ziel-PostgreSQL fehlgeschlagen")?;
    sqlx::raw_sql(
        "SET TIME ZONE 'UTC'; SET bytea_output = 'hex'; SET extra_float_digits = 1; \
         SET client_encoding = 'UTF8'; SET statement_timeout = 0; SET lock_timeout = 0; \
         SET idle_in_transaction_session_timeout = 0",
    )
    .execute(&mut c)
    .await?;
    Ok(c)
}

pub async fn run(opts: &Options, report: &mut Report) -> Result<()> {
    report.schema_version = schema::VERSION.into();

    // ---- Source -----------------------------------------------------------------------
    let mut src = Source::connect(&opts.source).await?;
    report.source_dialect = src.dialect().into();
    report.source_version = src.version().await.unwrap_or_default();
    let res = run_inner(opts, report, &mut src).await;
    src.close().await;
    res
}

/// Result of the precheck: what is caught up when reading an older source.
pub struct Prep {
    pub plan: catchup::Plan,
    /// additional `django_migrations` rows (canonical)
    pub mig_rows: Vec<Vec<Option<String>>>,
    /// Schema with which to copy and verify
    pub shape: Shape,
    /// 2.x source: after copy and verification convert to v3.4.2 in the target (same transaction)
    pub convert: Option<catchup::Plan>,
}

impl Prep {
    pub fn adapt<'a>(&self, t: &'a Table) -> Adapt<'a> {
        if let Some(conv) = &self.convert {
            // 2.19.6/2.20.0–2.20.4: WorkflowAction.order missing; documents.1075 sets order = id
            if conv.needs_1075() && t.name == "documents_workflowaction" {
                let all = t.copy_columns();
                let select: Vec<&Column> = all.iter().copied().filter(|c| c.name != "order").collect();
                let id = select.iter().position(|c| c.name == "id").unwrap_or(0);
                let mut fill = Vec::new();
                let mut i = 0;
                for c in &all {
                    if c.name == "order" {
                        fill.push(catchup::Fill::Src(id));
                    } else {
                        fill.push(catchup::Fill::Src(i));
                        i += 1;
                    }
                }
                return Adapt { select, fill, skip_source: false, extra_rows: vec![] };
            }
            return Adapt::identity(t);
        }
        if self.plan.is_empty() {
            return Adapt::identity(t);
        }
        let mut a = self.plan.adapt(t);
        if t.name == "django_migrations" {
            a.extra_rows = self.mig_rows.clone();
        }
        a
    }
}

async fn run_inner(opts: &Options, report: &mut Report, src: &mut Source) -> Result<()> {
    let prep = precheck_source(opts, report, src).await?;
    let prep = &prep;
    let tables: &[Table] = &prep.shape.tables;

    let mut target = match &opts.target {
        Some(t) => Some(connect_target(t).await?),
        None => None,
    };
    if let Some(t) = target.as_mut() {
        report.target_version = sqlx::query_scalar::<_, String>("SELECT version()").fetch_one(&mut *t).await.unwrap_or_default();
        let enc: String =
            sqlx::query_scalar("SELECT pg_encoding_to_char(encoding)::text FROM pg_database WHERE datname = current_database()")
                .fetch_one(&mut *t)
                .await?;
        if enc != "UTF8" {
            bail!("Ziel-Datenbank hat Kodierung {enc}, benötigt wird UTF8 (CREATE DATABASE … ENCODING 'UTF8' TEMPLATE template0)");
        }
        let ver: i32 = sqlx::query_scalar("SELECT current_setting('server_version_num')::int").fetch_one(&mut *t).await?;
        if ver < 140000 {
            report.warnings.push(format!("Ziel-PostgreSQL {ver} ist älter als 14; getestet ist das Schema nur auf PostgreSQL 18"));
        }
    }

    match opts.mode {
        Mode::DryRun => {
            if let Some(t) = target.as_mut() {
                let n = target_object_count(t).await?;
                if n > 0 {
                    report.problems.push(format!("Ziel ist nicht leer ({n} Objekte im Schema public) – Umzug würde abbrechen"));
                }
            }
            scan_all(opts, report, tables, src, prep).await?;
            report.result = if report.problems.is_empty() && !report.has_errors() { "dry-run-ok" } else { "dry-run-problems" }.into();
            Ok(())
        }
        Mode::VerifyOnly => {
            let t = target.as_mut().ok_or_else(|| anyhow!("--verify-only braucht --target"))?;
            sqlx::raw_sql("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *t).await?;
            check_target_structure(t, tables, report).await?;
            scan_all(opts, report, tables, src, prep).await?;
            verify_target(opts, report, tables, src, t, prep).await?;
            let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *t).await;
            report.result = if report.problems.is_empty() && !report.has_errors() { "verified" } else { "verify-failed" }.into();
            Ok(())
        }
        Mode::Migrate => {
            let t = target.as_mut().ok_or_else(|| anyhow!("Ziel fehlt (--target postgres://…)"))?;
            let n = target_object_count(t).await?;
            if n > 0 {
                bail!(
                    "Ziel-Datenbank ist nicht leer ({n} Objekte im Schema public). Bitte eine neue, leere Datenbank \
                     anlegen (CREATE DATABASE … ENCODING 'UTF8' TEMPLATE template0) – das Werkzeug überschreibt nichts."
                );
            }
            migrate(opts, report, tables, src, t, prep).await
        }
    }
}

async fn target_object_count(t: &mut PgConnection) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r','p','v','m','S','f')",
    )
    .fetch_one(t)
    .await?)
}

/// Migration state, tables and columns of the source against the embedded v3.4.2 schema (2.x source:
/// against the 2.20 schema, conversion afterwards in the target).
async fn precheck_source(opts: &Options, report: &mut Report, src: &mut Source) -> Result<Prep> {
    let have = src.tables().await?;
    if !have.contains("django_migrations") {
        bail!("Quelle enthält keine Tabelle django_migrations – ist das eine paperless-ngx-Datenbank?");
    }
    let mut mc = schema::check_migrations(&src.migrations().await?);
    // Older source (v3.0.0 … v3.4.1): missing migrations are caught up while copying.
    let mut plan = if mc.ok() {
        catchup::Plan::default()
    } else {
        match catchup::plan(&mc) {
            Ok(p) => {
                mc.missing.clear();
                p
            }
            Err(_) => catchup::Plan::default(), // Message below (with the full list)
        }
    };
    report.migrations_replaced_rows = mc.replaced_present + mc.historical_present;
    let v2 = plan.is_v2();
    if !mc.newer.is_empty() {
        report.warnings.push(format!(
            "Quelle stammt aus einer neueren paperless-ngx-Version als {} mit bekannten Migrationen {:?}: wird übernommen{}",
            schema::VERSION,
            mc.newer.iter().map(|(a, n)| format!("{a}.{n}")).collect::<Vec<_>>(),
            match (mc.tag_tree_dropped(), mc.note_format()) {
                (true, true) =>
                    "; documents_tag ohne die treenode-Spalten (wie PaperRust pl_objects.0001), documents_note mit Spalte format",
                (true, false) => "; documents_tag ohne die treenode-Spalten (wie PaperRust pl_objects.0001)",
                (false, true) => "; documents_note mit Spalte format",
                (false, false) => "",
            }
        ));
    }
    if v2 && mc.foreign_apps.iter().all(|(a, _)| a == "django_celery_results") {
        // 3.0 (documents.0020) deletes these rows and tables; no note needed
    } else if !mc.foreign_apps.is_empty() {
        report.warnings.push(format!(
            "django_migrations enthält Zeilen von Apps, die v{} nicht installiert (werden unverändert übernommen, Django ignoriert sie): {:?}",
            schema::VERSION,
            mc.foreign_apps.iter().map(|(a, n)| format!("{a}.{n}")).collect::<Vec<_>>()
        ));
    }
    if !mc.ok() {
        let fmt = |v: &[(String, String)]| v.iter().take(30).map(|(a, n)| format!("{a}.{n}")).collect::<Vec<_>>().join(", ");
        let mut msg = format!(
            "Migrationsstand der Quelle entspricht nicht paperless-ngx {} und lässt sich nicht nachholen (möglich ab {}). \
             Bitte zuerst paperless-ngx auf {} aktualisieren und einmal starten (führt `migrate` aus), dann erneut \
             versuchen – oder (ab paperless-ngx 2.x; Exporte aus 1.x gehen nicht) einen Export machen              (`document_exporter`) und diesen übernehmen.",
            schema::VERSION,
            catchup::FROM_VERSION,
            schema::VERSION
        );
        if !mc.missing.is_empty() {
            msg.push_str(&format!("\n  fehlend ({}): {}", mc.missing.len(), fmt(&mc.missing)));
        }
        if !mc.unknown.is_empty() {
            msg.push_str(&format!(
                "\n  unbekannt/neuer ({}): {} (neuere paperless-ngx-Version oder Zusatz-App)",
                mc.unknown.len(),
                fmt(&mc.unknown)
            ));
        }
        bail!(msg);
    }
    if v2 {
        return precheck_v2(opts, report, src, std::mem::take(&mut plan)).await;
    }
    let shape = Shape::v342(mc.tag_tree_dropped(), mc.note_format())?;
    let tables: &[Table] = &shape.tables;
    if !plan.is_empty() {
        report.catchup = plan.names();
        report.warnings.push(format!(
            "Quelle ist älter (etwa paperless-ngx {}): {} Migration(en) bis {} werden beim Kopieren nachgeholt, die Quelle \
             bleibt unverändert: {}",
            catchup::guess_version(&plan),
            plan.steps.len(),
            schema::VERSION,
            plan.names().join(", ")
        ));
    }
    let new_tables = plan.new_tables();
    let added = plan.added_columns();
    let want: BTreeSet<String> = tables.iter().map(|t| t.name.clone()).collect();
    let missing: Vec<&String> = want.difference(&have).filter(|t| !new_tables.contains(t.as_str())).collect();
    if !missing.is_empty() {
        bail!("Quelle fehlen Tabellen des v3.4.2-Schemas: {missing:?}");
    }
    let extra: Vec<String> = have.difference(&want).cloned().collect();
    if !extra.is_empty() {
        if opts.skip_unknown_tables {
            report.warnings.push(format!("Tabellen außerhalb des Django-Schemas werden NICHT übernommen: {extra:?}"));
        } else {
            bail!(
                "Quelle enthält Tabellen außerhalb des paperless-ngx-{}-Schemas: {extra:?}. Sie würden nicht \
                 übernommen. Prüfen, ob sie gebraucht werden; zum bewussten Weglassen --skip-unknown-tables angeben.",
                schema::VERSION
            );
        }
    }
    report.skipped_tables = extra;
    for t in tables {
        if new_tables.contains(t.name.as_str()) {
            continue;
        }
        let cols = src.columns(&t.name).await?;
        let src_set: BTreeSet<String> = cols.iter().filter(|(_, g)| !g).map(|(n, _)| n.clone()).collect();
        let later = added.get(t.name.as_str());
        let want_set: BTreeSet<String> =
            t.copy_columns().iter().filter(|c| !later.is_some_and(|a| a.contains_key(c.name.as_str()))).map(|c| c.name.clone()).collect();
        let gen_ok = |n: &String| t.columns.iter().any(|c| c.generated && &c.name == n);
        let extra: Vec<&String> = src_set.difference(&want_set).filter(|n| !gen_ok(n)).collect();
        let miss: Vec<&String> = want_set.difference(&src_set).collect();
        if !extra.is_empty() || !miss.is_empty() {
            bail!("Spalten von {} weichen vom v3.4.2-Schema ab: fehlend {miss:?}, zusätzlich {extra:?}", t.name);
        }
    }
    let fk = src.foreign_key_violations().await?;
    if !fk.is_empty() {
        report.problems.push(format!(
            "Quelle hat {} verletzte Fremdschlüssel (PRAGMA foreign_key_check), PostgreSQL wird sie ablehnen: {}",
            fk.len(),
            fk.join("; ")
        ));
    }
    let mut mig_rows = Vec::new();
    if !plan.is_empty() {
        // Next id and – so that dry run, move and later verification yield the same values – as
        // `applied` the most recent timestamp of the source (not "now").
        let mut max_id: i64 = 0;
        let mut last_applied: Option<String> = None;
        let sql = format!("SELECT {}, {} FROM django_migrations", src.text("max(id)"), src.text("max(applied)"));
        src.for_each_text_row(&sql, |r| {
            max_id = r.first().cloned().flatten().and_then(|s| s.parse().ok()).unwrap_or(0);
            last_applied = r.get(1).cloned().flatten();
        })
        .await?;
        let applied = last_applied.as_deref().and_then(value::parse_ts).unwrap_or_else(|| chrono::Utc::now().naive_utc());
        mig_rows = plan.migration_rows(max_id + 1, &value::fmt_ts(&applied, true));
    }
    Ok(Prep { plan, mig_rows, shape, convert: None })
}

/// 2.x source (paperless-ngx 2.20, also 2.19.6/2.20.0–2.20.4): verify against the embedded 2.20 schema;
/// copied unchanged, converted afterwards in the target (like `switch` in place).
async fn precheck_v2(opts: &Options, report: &mut Report, src: &mut Source, plan: catchup::Plan) -> Result<Prep> {
    if let Some(msg) = catchup::precheck_v2(src).await? {
        bail!(msg);
    }
    if opts.mode == Mode::VerifyOnly {
        bail!(
            "--verify-only vergleicht Tabelle für Tabelle; eine paperless-ngx-2.x-Quelle ist nach dem Umbau anders aufgebaut. \
             Den Abgleich (Anzahlen, Dokumente, Dateien) macht `paperrust-migrate-db verify --from … --target …`."
        );
    }
    let shape = Shape::v220()?;
    let present = src.migrations().await?;
    let skip_1075 = plan.needs_1075();
    let miss: Vec<String> = schema::v220::expected_migrations()
        .into_iter()
        .filter(|m| !present.contains(m))
        .filter(|m| !(skip_1075 && m.0 == catchup::V2_1075.0 && m.1 == catchup::V2_1075.1))
        .map(|(a, n)| format!("{a}.{n}"))
        .collect();
    if !miss.is_empty() {
        bail!(
            "Quelle ist paperless-ngx 2.x, entspricht aber nicht dem Stand von 2.20.15 ({} Migration(en) fehlen, z. B. {}). \
             Bitte paperless-ngx zuerst auf 2.20.15 aktualisieren und einmal starten – oder einen Export machen \
             (`document_exporter`) und diesen angeben.",
            miss.len(),
            miss.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
        );
    }
    report.catchup = plan.names().into_iter().chain(plan.marks.iter().map(|(a, n)| format!("{a}.{n}"))).collect();
    report.warnings.push(format!(
        "Quelle ist paperless-ngx {} (Schema {}): sie wird unverändert kopiert und geprüft, danach baut PaperRust das Ziel in \
         derselben Transaktion auf {} um ({} Migrationen wie paperless-ngx 3.4.2 selbst, u. a. SHA-256-Prüfsummen aus den \
         Dateien). Die Quelle bleibt unverändert.",
        catchup::guess_version(&plan),
        shape.version,
        schema::VERSION,
        plan.steps.len()
    ));
    let have = src.tables().await?;
    let want: BTreeSet<String> = shape.tables.iter().map(|t| t.name.clone()).collect();
    let missing: Vec<&String> = want.difference(&have).collect();
    if !missing.is_empty() {
        bail!("Quelle fehlen Tabellen des paperless-ngx-2.20-Schemas: {missing:?}");
    }
    let dropped: Vec<String> = have.difference(&want).filter(|t| schema::v220::dropped_table(t)).cloned().collect();
    let extra: Vec<String> = have.difference(&want).filter(|t| !schema::v220::dropped_table(t)).cloned().collect();
    if !extra.is_empty() && !opts.skip_unknown_tables {
        bail!(
            "Quelle enthält Tabellen außerhalb des paperless-ngx-2.20-Schemas: {extra:?}. Sie würden nicht übernommen. \
             Prüfen, ob sie gebraucht werden; zum bewussten Weglassen --skip-unknown-tables angeben."
        );
    }
    if !extra.is_empty() {
        report.warnings.push(format!("Tabellen außerhalb des Django-Schemas werden NICHT übernommen: {extra:?}"));
    }
    if !dropped.is_empty() {
        report.warnings.push(format!(
            "Tabellen von django-celery-results werden nicht übernommen (paperless-ngx 3.0 löscht sie, documents.0020): {dropped:?}"
        ));
    }
    report.skipped_tables = extra.into_iter().chain(dropped).collect();
    for t in &shape.tables {
        let cols = src.columns(&t.name).await?;
        let src_set: BTreeSet<String> = cols.iter().filter(|(_, g)| !g).map(|(n, _)| n.clone()).collect();
        let want_set: BTreeSet<String> = t
            .copy_columns()
            .iter()
            .filter(|c| !(skip_1075 && t.name == "documents_workflowaction" && c.name == "order"))
            .map(|c| c.name.clone())
            .collect();
        let gen_ok = |n: &String| t.columns.iter().any(|c| c.generated && &c.name == n);
        let extra: Vec<&String> = src_set.difference(&want_set).filter(|n| !gen_ok(n)).collect();
        let miss: Vec<&String> = want_set.difference(&src_set).collect();
        if !extra.is_empty() || !miss.is_empty() {
            bail!("Spalten von {} weichen vom paperless-ngx-2.20-Schema ab: fehlend {miss:?}, zusätzlich {extra:?}", t.name);
        }
    }
    let fk = src.foreign_key_violations().await?;
    if !fk.is_empty() {
        report.problems.push(format!(
            "Quelle hat {} verletzte Fremdschlüssel (PRAGMA foreign_key_check), PostgreSQL wird sie ablehnen: {}",
            fk.len(),
            fk.join("; ")
        ));
    }
    Ok(Prep { plan: catchup::Plan::default(), mig_rows: vec![], shape, convert: Some(plan) })
}

/// Target tables/columns must correspond exactly to the embedded schema.
async fn check_target_structure(t: &mut PgConnection, tables: &[Table], report: &mut Report) -> Result<()> {
    let rows = sqlx::query(
        "SELECT table_name::text, column_name::text, udt_name::text, character_maximum_length, is_generated::text \
         FROM information_schema.columns WHERE table_schema = 'public'",
    )
    .fetch_all(&mut *t)
    .await?;
    let mut have: BTreeSet<(String, String, String, Option<i32>, bool)> = BTreeSet::new();
    for r in rows {
        have.insert((r.try_get(0)?, r.try_get(1)?, r.try_get(2)?, r.try_get(3)?, r.try_get::<String, _>(4)? == "ALWAYS"));
    }
    let want: BTreeSet<(String, String, String, Option<i32>, bool)> = tables
        .iter()
        .flat_map(|t| t.columns.iter().map(|c| (t.name.clone(), c.name.clone(), c.udt.clone(), c.max_len.map(|m| m as i32), c.generated)))
        .collect();
    if have != want {
        let a: Vec<_> = want.difference(&have).take(10).collect();
        let b: Vec<_> = have.difference(&want).take(10).collect();
        report.problems.push(format!("Ziel-Schema weicht vom eingebetteten Schema ab: fehlend {a:?}, zusätzlich {b:?}"));
    }
    Ok(())
}

struct Progress {
    quiet: bool,
    last: Instant,
}

impl Progress {
    fn tick(&mut self, idx: usize, n: usize, table: &str, rows: u64, total: i64, force: bool) {
        if self.quiet || (!force && self.last.elapsed() < Duration::from_secs(2)) {
            return;
        }
        self.last = Instant::now();
        let pct = if total > 0 { (rows as f64 / total as f64 * 100.0).min(100.0) } else { 100.0 };
        eprintln!("[{:>2}/{n}] {table}: {rows}/{total} Zeilen ({pct:.0} %)", idx + 1);
    }
}

/// Reads a table of the source, converts every value, computes the checksum and optionally writes
/// via COPY into the target. Returns the table hash and – if `collect` – the row hashes per PK.
#[allow(clippy::too_many_arguments)]
async fn scan_table(
    opts: &Options,
    table: &Table,
    src: &mut Source,
    mut copy: Option<sqlx::postgres::PgCopyIn<&mut PgConnection>>,
    tr: &mut TableReport,
    progress: &mut Progress,
    idx: usize,
    n: usize,
    collect: bool,
    adapt: &Adapt<'_>,
) -> Result<(TableHash, Option<HashMap<String, RowHash>>)> {
    let cols: Vec<&Column> = adapt.select.clone();
    let pk_idx = cols.iter().position(|c| c.pk);
    let target_pk = table.copy_columns().iter().position(|c| c.pk);
    let sql = src.select_sql(&table.name, &cols);
    let mut th = TableHash::default();
    let mut map = collect.then(HashMap::new);
    let mut buf: Vec<u8> = Vec::with_capacity(opts.batch_bytes.min(16 << 20) + 4096);
    let mut read: Vec<Option<String>> = Vec::with_capacity(cols.len());
    let mut errors = false;
    // additional rows (catch-up: django_migrations) go through checksum/COPY like read rows
    let mut extra = adapt.extra_rows.iter();
    {
        let mut stream = if adapt.skip_source { futures::stream::empty().boxed() } else { src.stream(&sql, &cols) };
        loop {
            let vals: Vec<Option<String>> = if let Some(row) = stream.next().await {
                let row = row.with_context(|| format!("Lesen aus {} fehlgeschlagen", table.name))?;
                read.clear();
                let mut row_issues: Vec<(usize, bool, String)> = Vec::new();
                for (i, c) in cols.iter().enumerate() {
                    match value::convert(c.kind, &row[i], opts.policy) {
                        Conv::Ok(None) if !c.nullable => {
                            read.push(None);
                            row_issues.push((i, true, "NULL in NOT-NULL-Spalte".into()));
                        }
                        Conv::Ok(v) => read.push(v),
                        Conv::Fixed(v, msg) => {
                            read.push(v);
                            row_issues.push((i, false, msg));
                        }
                        Conv::Err(msg) => {
                            read.push(None);
                            row_issues.push((i, true, msg));
                        }
                    }
                }
                if !row_issues.is_empty() {
                    let pk = pk_idx.and_then(|p| read[p].clone()).unwrap_or_else(|| "?".into());
                    for (i, fatal, msg) in row_issues {
                        if fatal {
                            tr.errors += 1;
                            errors = true;
                        } else {
                            tr.fixed += 1;
                        }
                        if tr.issues.len() < opts.max_issues {
                            tr.issues.push(Issue { pk: pk.clone(), column: cols[i].name.clone(), fatal, message: msg });
                        }
                    }
                }
                adapt.apply(&read)
            } else if let Some(x) = extra.next() {
                x.clone()
            } else {
                break;
            };
            let rh = row_hash(vals.iter().map(|v| v.as_deref()));
            th.add(&rh);
            if let Some(m) = map.as_mut() {
                let pk = target_pk.and_then(|p| vals[p].clone()).unwrap_or_default();
                m.insert(pk, rh);
            }
            if copy.is_some() && !errors {
                for (i, v) in vals.iter().enumerate() {
                    if i > 0 {
                        buf.push(b'\t');
                    }
                    value::copy_escape(&mut buf, v.as_deref());
                }
                buf.push(b'\n');
                if buf.len() >= opts.batch_bytes {
                    if let Some(c) = copy.as_mut() {
                        c.send(buf.as_slice()).await.with_context(|| format!("COPY {} fehlgeschlagen", table.name))?;
                    }
                    buf.clear();
                }
            }
            progress.tick(idx, n, &table.name, th.rows, tr.source_rows, false);
        }
    }
    if let Some(mut c) = copy {
        if errors {
            let _ = c.abort("paperrust-migrate-db: Umwandlungsfehler").await;
        } else {
            if !buf.is_empty() {
                c.send(buf.as_slice()).await.with_context(|| format!("COPY {} fehlgeschlagen", table.name))?;
            }
            let written = c.finish().await.with_context(|| format!("COPY {} fehlgeschlagen", table.name))?;
            tr.copied_rows = Some(written);
        }
    }
    progress.tick(idx, n, &table.name, th.rows, tr.source_rows, true);
    Ok((th, map))
}

/// Read all tables of the source (without writing): count, conversion check, checksum.
/// Rows that go from the source (adjusted) into the target.
async fn source_rows(src: &mut Source, table: &Table, adapt: &Adapt<'_>) -> Result<i64> {
    let base = if adapt.skip_source { 0 } else { src.count(&table.name).await? };
    Ok(base + adapt.extra_rows.len() as i64)
}

async fn scan_all(opts: &Options, report: &mut Report, tables: &[Table], src: &mut Source, prep: &Prep) -> Result<()> {
    let mut progress = Progress { quiet: opts.quiet, last: Instant::now() };
    let n = tables.len();
    for (idx, table) in tables.iter().enumerate() {
        let adapt = prep.adapt(table);
        let mut tr = TableReport::new(&table.name);
        tr.source_rows = source_rows(src, table, &adapt).await?;
        tr.source_bytes = if adapt.skip_source { None } else { src.approx_bytes(&table.name).await };
        let (th, _) = scan_table(opts, table, src, None, &mut tr, &mut progress, idx, n, false, &adapt).await?;
        tr.source_checksum = Some(th.hex());
        if th.rows as i64 != tr.source_rows {
            report.problems.push(format!("{}: count(*) = {}, gelesen {}", table.name, tr.source_rows, th.rows));
        }
        report.tables.push(tr);
    }
    Ok(())
}

async fn migrate(
    opts: &Options,
    report: &mut Report,
    tables: &[Table],
    src: &mut Source,
    conn: &mut PgConnection,
    prep: &Prep,
) -> Result<()> {
    let mut tx = conn.begin().await?;
    if !opts.quiet {
        eprintln!("Schema v{} anlegen (Tabellen, Sequenzen) …", prep.shape.version);
    }
    sqlx::raw_sql(prep.shape.pre).execute(&mut *tx).await.context("Anlegen des Schemas (pre-data) fehlgeschlagen")?;
    for ddl in prep.shape.newer_ddl() {
        sqlx::raw_sql(&ddl).execute(&mut *tx).await?;
    }
    check_target_structure(&mut tx, tables, report).await?;
    if !report.problems.is_empty() {
        bail!("Ziel-Schema inkonsistent – Abbruch, Ziel bleibt leer");
    }

    let mut progress = Progress { quiet: opts.quiet, last: Instant::now() };
    let n = tables.len();
    for (idx, table) in tables.iter().enumerate() {
        let adapt = prep.adapt(table);
        let mut tr = TableReport::new(&table.name);
        tr.source_rows = source_rows(src, table, &adapt).await?;
        let cols = table.copy_columns();
        let collist: Vec<String> = cols.iter().map(|c| format!("\"{}\"", c.name)).collect();
        let stmt = format!("COPY public.\"{}\" ({}) FROM STDIN", table.name, collist.join(", "));
        let copy = tx.copy_in_raw(&stmt).await?;
        let (th, _) = scan_table(opts, table, src, Some(copy), &mut tr, &mut progress, idx, n, false, &adapt).await?;
        tr.source_checksum = Some(th.hex());
        let failed = tr.errors > 0;
        report.tables.push(tr);
        if failed {
            bail!(
                "Umwandlungsfehler in {} – Abbruch, Ziel wird zurückgerollt (bleibt leer). Details im Bericht; \
                 mit --dry-run lassen sich alle Befunde vorab auflisten.",
                table.name
            );
        }
    }

    if !opts.quiet {
        eprintln!("Sequenzen setzen …");
    }
    set_sequences(&mut tx).await?;
    if !opts.quiet {
        eprintln!("Primärschlüssel, Indizes, Unique- und Fremdschlüssel-Constraints anlegen (prüft alle Zeilen) …");
    }
    sqlx::raw_sql(&prep.shape.post).execute(&mut *tx).await.context(
        "Anlegen von Constraints/Indizes (post-data) fehlgeschlagen – die Quelldaten verletzen einen \
         Primärschlüssel, Unique- oder Fremdschlüssel-Constraint",
    )?;
    sqlx::raw_sql("ANALYZE").execute(&mut *tx).await?;

    if !opts.quiet {
        eprintln!("Prüfung (Zeilenzahlen, Prüfsummen, Sequenzen, Constraints) …");
    }
    verify_target(opts, report, tables, src, &mut tx, prep).await?;
    if !report.problems.is_empty() || report.has_errors() {
        tx.rollback().await?;
        bail!("Prüfung fehlgeschlagen – Ziel zurückgerollt (bleibt leer), Quelle unverändert");
    }
    if let Some(plan) = &prep.convert {
        if !opts.quiet {
            eprintln!("Umbau paperless-ngx {} → {} im Ziel ({} Migrationen) …", prep.shape.version, schema::VERSION, plan.steps.len());
        }
        let applied = catchup::apply_in_place(&mut tx, plan, opts.media.as_deref()).await?;
        report.catchup = applied.done;
        report.warnings.extend(applied.notes);
        let t342 = schema::tables().map_err(|e| anyhow!(e))?;
        check_target_structure(&mut tx, &t342, report).await?;
        // Do not check names: a converted 2.x DB carries (as after upstream `migrate`) partly older
        // constraint/index names than a fresh 3.4.2 DB (e.g. documents_sender_pkey); what is checked
        // are columns (above) and that all constraints/indexes are valid.
        check_post_objects(&mut tx, None, report).await?;
        set_sequences(&mut tx).await?;
        sqlx::raw_sql("ANALYZE").execute(&mut *tx).await?;
        if !report.problems.is_empty() {
            tx.rollback().await?;
            bail!("Umbau auf v{} ergab nicht das erwartete Schema – Ziel zurückgerollt (bleibt leer), Quelle unverändert", schema::VERSION);
        }
    }
    tx.commit().await.context("COMMIT fehlgeschlagen – Ziel unverändert")?;
    report.result = "migrated".into();
    Ok(())
}

async fn set_sequences(t: &mut PgConnection) -> Result<()> {
    for (table, col, seq) in sequences(t).await? {
        let sql = format!(
            "SELECT setval('{seq}', COALESCE((SELECT max(\"{col}\") FROM public.\"{table}\"), 0) + 1, false)",
            seq = seq.replace('\'', "''")
        );
        sqlx::query(&sql).execute(&mut *t).await?;
    }
    Ok(())
}

async fn sequences(t: &mut PgConnection) -> Result<Vec<(String, String, String)>> {
    let rows = sqlx::query(
        "SELECT table_name::text, column_name::text, \
                pg_get_serial_sequence(quote_ident(table_schema) || '.' || quote_ident(table_name), column_name)::text \
         FROM information_schema.columns WHERE table_schema = 'public' \
           AND (is_identity = 'YES' OR column_default LIKE 'nextval(%') ORDER BY 1, 2",
    )
    .fetch_all(&mut *t)
    .await?;
    let mut out = vec![];
    for r in rows {
        let s: Option<String> = r.try_get(2)?;
        if let Some(s) = s {
            out.push((r.try_get(0)?, r.try_get(1)?, s));
        }
    }
    Ok(out)
}

/// Target against source: row count, checksum (target re-read), sequences, constraints.
async fn verify_target(
    opts: &Options,
    report: &mut Report,
    tables: &[Table],
    src: &mut Source,
    t: &mut PgConnection,
    prep: &Prep,
) -> Result<()> {
    for (i, table) in tables.iter().enumerate() {
        let target_rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM public.\"{}\"", table.name)).fetch_one(&mut *t).await?;
        let (th, _) = target_hash(table, t, false).await?;
        let tr = &mut report.tables[i];
        tr.target_rows = Some(target_rows);
        tr.target_checksum = Some(th.hex());
        let ok = target_rows == tr.source_rows && tr.target_checksum == tr.source_checksum;
        tr.verified = Some(ok);
        if !ok {
            report.problems.push(format!(
                "{}: Quelle {} Zeilen / Ziel {} Zeilen, Prüfsumme {}",
                table.name,
                tr.source_rows,
                target_rows,
                if tr.target_checksum == tr.source_checksum { "gleich" } else { "VERSCHIEDEN" }
            ));
            // Diagnosis: differing primary keys
            let mut dummy = TableReport::new(&table.name);
            let mut p = Progress { quiet: true, last: Instant::now() };
            let adapt = prep.adapt(table);
            let (_, sm) = scan_table(opts, table, src, None, &mut dummy, &mut p, 0, 1, true, &adapt).await?;
            let (_, tm) = target_hash(table, t, true).await?;
            if let (Some(sm), Some(tm)) = (sm, tm) {
                let mut diff: Vec<String> = vec![];
                for (k, v) in &sm {
                    match tm.get(k) {
                        Some(w) if w == v => {}
                        Some(_) => diff.push(format!("pk={k}: Werte verschieden")),
                        None => diff.push(format!("pk={k}: fehlt im Ziel")),
                    }
                    if diff.len() >= MAX_DIFF_ROWS {
                        break;
                    }
                }
                for k in tm.keys() {
                    if diff.len() >= MAX_DIFF_ROWS {
                        break;
                    }
                    if !sm.contains_key(k) {
                        diff.push(format!("pk={k}: nur im Ziel"));
                    }
                }
                report.tables[i].diff_rows = diff;
            }
        }
    }

    // Sequences: next value > max(id)
    for (table, col, seq) in sequences(t).await? {
        let max: Option<i64> =
            sqlx::query_scalar(&format!("SELECT max(\"{col}\")::bigint FROM public.\"{table}\"")).fetch_one(&mut *t).await?;
        let row = sqlx::query(&format!("SELECT last_value, is_called FROM {seq}")).fetch_one(&mut *t).await?;
        let last: i64 = row.try_get(0)?;
        let called: bool = row.try_get(1)?;
        let next = if called { last + 1 } else { last };
        report.sequences_checked += 1;
        if next <= max.unwrap_or(0) {
            report.problems.push(format!("Sequenz {seq}: nächster Wert {next} ≤ max({table}.{col}) = {}", max.unwrap_or(0)));
        }
    }

    check_post_objects(t, Some(prep.shape.post_raw), report).await
}

/// Constraints/indexes from post-data must all exist (`post_raw`, otherwise without name check) and be valid.
async fn check_post_objects(t: &mut PgConnection, post_raw: Option<&str>, report: &mut Report) -> Result<()> {
    let cons: BTreeSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT c.conname::text FROM pg_constraint c JOIN pg_namespace n ON n.oid = c.connamespace WHERE n.nspname = 'public'",
    )
    .fetch_all(&mut *t)
    .await?
    .into_iter()
    .collect();
    let idx: BTreeSet<String> = sqlx::query_scalar::<_, String>("SELECT indexname::text FROM pg_indexes WHERE schemaname = 'public'")
        .fetch_all(&mut *t)
        .await?
        .into_iter()
        .collect();
    if let Some(post_raw) = post_raw {
        let (want_cons, want_idx) = post_data_objects_of(post_raw);
        let miss_c: Vec<&String> = want_cons.iter().filter(|c| !cons.contains(*c)).collect();
        let miss_i: Vec<&String> = want_idx.iter().filter(|c| !idx.contains(*c)).collect();
        report.constraints_checked = want_cons.len();
        report.indexes_checked = want_idx.len();
        if !miss_c.is_empty() || !miss_i.is_empty() {
            report.problems.push(format!("Ziel fehlen Constraints {miss_c:?} / Indizes {miss_i:?}"));
        }
    }
    let invalid: Vec<String> = sqlx::query_scalar(
        "SELECT c.conname::text FROM pg_constraint c JOIN pg_namespace n ON n.oid = c.connamespace \
         WHERE n.nspname = 'public' AND NOT c.convalidated",
    )
    .fetch_all(&mut *t)
    .await?;
    if !invalid.is_empty() {
        report.problems.push(format!("nicht validierte Constraints: {invalid:?}"));
    }
    let bad_idx: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND NOT i.indisvalid",
    )
    .fetch_all(&mut *t)
    .await?;
    if !bad_idx.is_empty() {
        report.problems.push(format!("ungültige Indizes: {bad_idx:?}"));
    }
    Ok(())
}

/// Names of all constraints (`ADD CONSTRAINT x`) and indexes (`CREATE [UNIQUE] INDEX x`) from post-data.
#[cfg(test)]
pub fn post_data_objects() -> (BTreeSet<String>, BTreeSet<String>) {
    post_data_objects_of(schema::POST_DATA)
}

pub fn post_data_objects_of(post: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut cons = BTreeSet::new();
    let mut idx = BTreeSet::new();
    for line in post.lines() {
        let l = line.trim();
        if let Some(p) = l.find("ADD CONSTRAINT ")
            && let Some(name) = l[p + 15..].split_whitespace().next()
        {
            cons.insert(name.trim_matches('"').to_string());
        }
        let rest = l.strip_prefix("CREATE UNIQUE INDEX ").or_else(|| l.strip_prefix("CREATE INDEX "));
        if let Some(r) = rest
            && let Some(name) = r.split_whitespace().next()
        {
            idx.insert(name.trim_matches('"').to_string());
        }
    }
    (cons, idx)
}

async fn target_hash(table: &Table, t: &mut PgConnection, collect: bool) -> Result<(TableHash, Option<HashMap<String, RowHash>>)> {
    let cols = table.copy_columns();
    let pk_idx = cols.iter().position(|c| c.pk);
    let exprs: Vec<String> = cols.iter().map(|c| format!("\"{}\"::text", c.name)).collect();
    let sql = format!("SELECT {} FROM public.\"{}\"", exprs.join(", "), table.name);
    let mut th = TableHash::default();
    let mut map = collect.then(HashMap::new);
    let mut stream = sqlx::query(&sql).fetch(&mut *t);
    let mut vals: Vec<Option<String>> = Vec::with_capacity(cols.len());
    while let Some(row) = stream.next().await {
        let row = row?;
        vals.clear();
        for (i, c) in cols.iter().enumerate() {
            let v: Option<String> = row.try_get(i)?;
            vals.push(match v {
                None => None,
                Some(s) => Some(canon_from_pg(c.kind, &s).map_err(|e| anyhow!("{}.{}: {e}", table.name, c.name))?),
            });
        }
        let rh = row_hash(vals.iter().map(|v| v.as_deref()));
        th.add(&rh);
        if let Some(m) = map.as_mut() {
            m.insert(pk_idx.and_then(|p| vals[p].clone()).unwrap_or_default(), rh);
        }
    }
    Ok((th, map))
}

#[cfg(test)]
mod tests {
    #[test]
    fn post_data_has_objects() {
        let (c, i) = super::post_data_objects();
        assert!(c.iter().any(|x| x == "documents_document_pkey"), "PK-Constraint fehlt in post-data");
        assert!(!i.is_empty());
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    /// paperless-ngx 3.5.x source (documents.0028–0030): the expected copy schema is the v3.4.2
    /// snapshot with documents.0028 and documents.0030 applied.
    #[test]
    fn v351_shape_is_v342_plus_0028_and_0030() {
        let base = Shape::v342(false, false).unwrap();
        assert!(base.newer_ddl().is_empty());
        let s = Shape::v342(true, true).unwrap();
        let note = s.tables.iter().find(|t| t.name == "documents_note").unwrap();
        let last = note.columns.last().unwrap();
        assert_eq!((last.name.as_str(), last.udt.as_str(), last.max_len, last.nullable), ("format", "varchar", Some(16), false));
        assert_eq!(note.copy_columns().len(), base.tables.iter().find(|t| t.name == "documents_note").unwrap().copy_columns().len() + 1);
        let tag = s.tables.iter().find(|t| t.name == "documents_tag").unwrap();
        assert!(tag.columns.iter().all(|c| !schema::TREENODE_DERIVED.contains(&c.name.as_str())));
        assert!(tag.columns.iter().any(|c| c.name == "tn_parent_id"));
        let ddl = s.newer_ddl();
        assert_eq!(ddl.len(), 3);
        assert!(ddl[0].starts_with("ALTER TABLE public.documents_tag DROP COLUMN \"tn_ancestors_pks\""));
        assert!(ddl[1].contains("ADD COLUMN \"format\" varchar(16) DEFAULT 'plain' NOT NULL"));
        assert!(ddl[2].ends_with("DROP DEFAULT"));
        // the pre-data snapshot itself is v3.4.2 (no format column yet)
        assert!(!schema::PRE_DATA.contains("format character varying(16)"));
    }

    #[test]
    fn v351_migration_set_is_accepted() {
        let mut src = schema::expected_migrations();
        for (a, n) in schema::KNOWN_NEWER {
            src.insert((a.to_string(), n.to_string()));
        }
        let mc = schema::check_migrations(&src);
        assert!(mc.ok() && mc.tag_tree_dropped() && mc.note_format(), "{mc:?}");
        let s = Shape::v342(mc.tag_tree_dropped(), mc.note_format()).unwrap();
        assert!(s.note_format && s.drop_tag_tree);
    }
}
