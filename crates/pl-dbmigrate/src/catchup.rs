//! Catching up missing paperless-ngx migrations (v2.20 / v3.0.0 … v3.4.1 → v3.4.2) without Python.
//!
//! Two uses:
//! - **Copy** (move to a new database): the source stays unchanged; when reading, missing
//!   columns are filled with the value the Django migration would have set, missing
//!   tables stay empty, data migrations (RunPython) are applied to the values and the
//!   missing `django_migrations` rows are added. Checksums are compared against these expected
//!   values ([`Plan::adapt`]).
//! - **in place** (same PostgreSQL database): SQL from
//!   `schema/catchup-v2.20-v3.0.sql` and `schema/catchup-v3.0.0-v3.4.2.sql` in one transaction
//!   ([`apply_in_place`]); two data migrations from 2.20 → 3.0 run in Rust ([`run_hook`]).
//!
//! **paperless-ngx 2.20** (like upstream: v3 can only be reached from 2.20.15,
//! docs/migration-v3.md; 2.19.6 and 2.20.0–2.20.4 only lack documents.1075, which is run along here):
//! v3 squashed the migration history (documents.0001_squashed/0002_squashed,
//! paperless_mail.0001_squashed replace all 2.x migrations). Django considers the squashed
//! migrations applied if all replaced ones are applied, and records them at the end
//! ([`Plan::marks`]); after that documents 0003 … 0027, paperless 0005 … 0018, paperless_mail
//! 0002 … 0004 run. The copy path copies a 2.20 source with the embedded 2.20 schema and
//! converts the target in place in the same transaction (migrate.rs).
//!
//! upstream: src/{documents,paperless,paperless_mail}/migrations @v3.4.2, documents.1075 @v2.20.15.

use crate::schema::{Column, MigrationCheck, Table};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};

pub const SQL: &str = include_str!("../schema/catchup-v3.0.0-v3.4.2.sql");
/// paperless-ngx 2.20 → 3.0 (steps before [`SQL`]).
pub const SQL_V2: &str = include_str!("../schema/catchup-v2.20-v3.0.sql");

/// Oldest version from which catching up is possible.
pub const FROM_VERSION: &str = "2.20";

/// First step that only a 2.x source needs (detection of "2.x").
pub const V2_MARKER: (&str, &str) = ("documents", "0003_remove_document_storage_type");
/// Only missing in 2.19.6 and 2.20.0 … 2.20.4.
pub const V2_1075: (&str, &str) = ("documents", "1075_workflowaction_order");

/// What a migration does to data (for the copy path).
pub struct StepData {
    pub app: &'static str,
    pub name: &'static str,
    /// new columns: (table, column, canonical value for existing rows; None = NULL)
    pub add_columns: &'static [(&'static str, &'static str, Option<&'static str>)],
    /// new tables (stay empty)
    pub new_tables: &'static [&'static str],
    /// RunPython: bool column `false` → NULL
    pub false_to_null: &'static [(&'static str, &'static str)],
}

/// All steps in order of application (same order as the SQL file).
pub const STEPS: &[StepData] = &[
    StepData {
        app: "documents",
        name: "0023_savedview_icon",
        add_columns: &[("documents_savedview", "icon", Some("funnel"))],
        new_tables: &[],
        false_to_null: &[],
    },
    StepData { app: "documents", name: "0024_alter_workflowaction_type", add_columns: &[], new_tables: &[], false_to_null: &[] },
    StepData {
        app: "documents",
        name: "0025_workflowaction_apply_ai_suggestions",
        add_columns: &[
            ("documents_workflowaction", "ai_create_missing", Some("f")),
            ("documents_workflowaction", "ai_overwrite_existing", Some("f")),
            ("documents_workflowaction", "ai_suggestion_fields", None),
        ],
        new_tables: &[],
        false_to_null: &[],
    },
    StepData {
        app: "documents",
        name: "0026_alter_document_archive_checksum_and_more",
        add_columns: &[],
        new_tables: &[],
        false_to_null: &[],
    },
    StepData {
        app: "documents",
        name: "0027_documentbarcode",
        add_columns: &[],
        new_tables: &["documents_documentbarcode"],
        false_to_null: &[],
    },
    StepData {
        app: "paperless",
        name: "0014_applicationconfiguration_remote_ocr_api_key_and_more",
        add_columns: &[
            ("paperless_applicationconfiguration", "remote_ocr_api_key", None),
            ("paperless_applicationconfiguration", "remote_ocr_endpoint", None),
            ("paperless_applicationconfiguration", "remote_ocr_engine", None),
        ],
        new_tables: &[],
        false_to_null: &[],
    },
    StepData {
        app: "paperless",
        name: "0015_applicationconfiguration_remote_ocr_mode",
        add_columns: &[("paperless_applicationconfiguration", "remote_ocr_mode", None)],
        new_tables: &[],
        false_to_null: &[],
    },
    StepData {
        app: "paperless",
        name: "0016_alter_applicationconfiguration_ai_enabled",
        add_columns: &[],
        new_tables: &[],
        false_to_null: &[("paperless_applicationconfiguration", "ai_enabled")],
    },
    StepData {
        app: "paperless",
        name: "0017_applicationconfiguration_llm_embedding_api_key",
        add_columns: &[("paperless_applicationconfiguration", "llm_embedding_api_key", None)],
        new_tables: &[],
        false_to_null: &[],
    },
    StepData {
        app: "paperless",
        name: "0018_applicationconfiguration_barcode_store_values",
        add_columns: &[("paperless_applicationconfiguration", "barcode_store_values", None)],
        new_tables: &[],
        false_to_null: &[],
    },
];

/// A part of a step: SQL or a data migration in Rust (`-- @rust <name>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Sql(String),
    Rust(String),
}

/// Step from an SQL file.
#[derive(Debug, Clone)]
pub struct SqlStep {
    pub app: String,
    pub name: String,
    pub parts: Vec<Part>,
}

/// Steps of a file with Rust hooks in order.
pub fn parse_steps(text: &str) -> Vec<SqlStep> {
    let mut out: Vec<SqlStep> = Vec::new();
    for line in text.lines() {
        let l = line.trim_end_matches('\r');
        if let Some(rest) = l.strip_prefix("-- @@ ") {
            let mut it = rest.split_whitespace();
            let app = it.next().unwrap_or_default().to_string();
            let name = it.next().unwrap_or_default().to_string();
            out.push(SqlStep { app, name, parts: vec![] });
        } else if let Some(last) = out.last_mut() {
            if let Some(hook) = l.strip_prefix("-- @rust ") {
                last.parts.push(Part::Rust(hook.trim().to_string()));
            } else if !l.trim_start().starts_with("--") && !l.trim().is_empty() {
                match last.parts.last_mut() {
                    Some(Part::Sql(s)) => {
                        s.push_str(l);
                        s.push('\n');
                    }
                    _ => last.parts.push(Part::Sql(format!("{l}\n"))),
                }
            }
        }
    }
    out
}

/// All catch-up steps in order of application (2.20 → 3.0, then 3.0 → 3.4.2).
pub fn all_steps() -> Vec<SqlStep> {
    let mut v = parse_steps(SQL_V2);
    v.extend(parse_steps(SQL));
    v
}

/// Squashed migration (v3 `0001_squashed` or 2.x `…_squashed_…`) that an installation has not
/// necessarily recorded: Django records it as soon as all replaced ones are applied.
fn is_squash_name(name: &str) -> bool {
    name.contains("squashed")
}

/// SQL steps of the file v3.0.0 → v3.4.2: (app, name, sql).
#[cfg(test)]
pub fn sql_steps() -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for line in SQL.lines() {
        let l = line.trim_end_matches('\r');
        if let Some(rest) = l.strip_prefix("-- @@ ") {
            let mut it = rest.split_whitespace();
            let app = it.next().unwrap_or_default().to_string();
            let name = it.next().unwrap_or_default().to_string();
            out.push((app, name, String::new()));
        } else if let Some(last) = out.last_mut()
            && !l.trim_start().starts_with("--")
            && !l.trim().is_empty()
        {
            last.2.push_str(l);
            last.2.push('\n');
        }
    }
    out
}

/// Steps to catch up for a source.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// (app, name) in order of application
    pub steps: Vec<(String, String)>,
    /// missing rows of squashed migrations: only record in django_migrations (at the end, like
    /// Django's `check_replacements`), no SQL
    pub marks: Vec<(String, String)>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.marks.is_empty()
    }

    /// Source is paperless-ngx 2.x (needs the conversion 2.20 → 3.0).
    pub fn is_v2(&self) -> bool {
        self.steps.iter().any(|(a, n)| a == V2_MARKER.0 && n == V2_MARKER.1)
    }

    /// documents.1075 missing (2.19.6 / 2.20.0 … 2.20.4).
    pub fn needs_1075(&self) -> bool {
        self.steps.iter().any(|(a, n)| a == V2_1075.0 && n == V2_1075.1)
    }

    pub fn names(&self) -> Vec<String> {
        self.steps.iter().map(|(a, n)| format!("{a}.{n}")).collect()
    }

    fn data(&self) -> impl Iterator<Item = &'static StepData> + '_ {
        STEPS.iter().filter(|s| self.steps.iter().any(|(a, n)| a == s.app && n == s.name))
    }

    /// Tables that may still be missing in the source.
    pub fn new_tables(&self) -> BTreeSet<&'static str> {
        self.data().flat_map(|s| s.new_tables.iter().copied()).collect()
    }

    /// Columns that may still be missing in the source: table → (column → value).
    pub fn added_columns(&self) -> BTreeMap<&'static str, BTreeMap<&'static str, Option<&'static str>>> {
        let mut m: BTreeMap<&str, BTreeMap<&str, Option<&str>>> = BTreeMap::new();
        for s in self.data() {
            for (t, c, v) in s.add_columns {
                m.entry(*t).or_default().insert(*c, *v);
            }
        }
        m
    }

    /// Adjustment for a table when copying.
    pub fn adapt<'a>(&self, table: &'a Table) -> Adapt<'a> {
        let cols = table.copy_columns();
        if self.new_tables().contains(table.name.as_str()) {
            return Adapt { select: vec![], fill: vec![], skip_source: true, extra_rows: vec![] };
        }
        let added = self.added_columns();
        let add = added.get(table.name.as_str());
        let fnull: BTreeSet<&str> =
            self.data().flat_map(|s| s.false_to_null.iter()).filter(|(t, _)| *t == table.name).map(|(_, c)| *c).collect();
        let mut select: Vec<&'a Column> = Vec::new();
        let mut fill = Vec::new();
        for c in cols {
            if let Some(v) = add.and_then(|a| a.get(c.name.as_str())) {
                fill.push(Fill::Const(v.map(str::to_string)));
            } else {
                fill.push(if fnull.contains(c.name.as_str()) { Fill::FalseToNull(select.len()) } else { Fill::Src(select.len()) });
                select.push(c);
            }
        }
        Adapt { select, fill, skip_source: false, extra_rows: vec![] }
    }

    /// Additional `django_migrations` rows (id, app, name, applied) from `next_id`.
    pub fn migration_rows(&self, next_id: i64, applied: &str) -> Vec<Vec<Option<String>>> {
        self.steps
            .iter()
            .chain(self.marks.iter())
            .enumerate()
            .map(|(i, (a, n))| vec![Some((next_id + i as i64).to_string()), Some(a.clone()), Some(n.clone()), Some(applied.to_string())])
            .collect()
    }
}

/// How a target column is filled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fill {
    /// Value of the i-th column read
    Src(usize),
    /// fixed (canonical) value
    Const(Option<String>),
    /// bool column read, `f` → NULL (RunPython)
    FalseToNull(usize),
}

/// Adjustment of a table when copying from an older source.
#[derive(Debug, Clone)]
pub struct Adapt<'a> {
    /// columns of the source to read
    pub select: Vec<&'a Column>,
    /// per target column (copy_columns order)
    pub fill: Vec<Fill>,
    /// Table does not exist in the source yet
    pub skip_source: bool,
    /// additional rows (canonical values in target column order)
    pub extra_rows: Vec<Vec<Option<String>>>,
}

impl Adapt<'_> {
    /// Unchanged (source on v3.4.2).
    pub fn identity(table: &Table) -> Adapt<'_> {
        let select = table.copy_columns();
        let fill = (0..select.len()).map(Fill::Src).collect();
        Adapt { select, fill, skip_source: false, extra_rows: vec![] }
    }

    /// Read, converted values → target values.
    pub fn apply(&self, read: &[Option<String>]) -> Vec<Option<String>> {
        self.fill
            .iter()
            .map(|f| match f {
                Fill::Src(i) => read.get(*i).cloned().flatten(),
                Fill::Const(v) => v.clone(),
                Fill::FalseToNull(i) => match read.get(*i).cloned().flatten() {
                    Some(v) if v == "f" => None,
                    v => v,
                },
            })
            .collect()
    }
}

/// Can the migration state be caught up? `Ok(plan)` (empty = already v3.4.2), otherwise error text.
pub fn plan(mc: &MigrationCheck) -> Result<Plan, String> {
    if !mc.unknown.is_empty() {
        return Err(newer_message(&mc.unknown));
    }
    let known: Vec<(String, String)> = all_steps().into_iter().map(|s| (s.app, s.name)).collect();
    let replaced = crate::schema::replaced_migrations();
    let mut marks = Vec::new();
    let mut not_catchable: Vec<&(String, String)> = Vec::new();
    for m in &mc.missing {
        if known.contains(m) {
            continue;
        }
        // Squash rows (v3: 0001_squashed …, 2.x: …_squashed_…) are recorded by Django itself
        if is_squash_name(&m.1) && (replaced.contains_key(m) || replaced.values().any(|v| v == m)) {
            marks.push(m.clone());
        } else {
            not_catchable.push(m);
        }
    }
    if !not_catchable.is_empty() {
        return Err(format!(
            "Die Datenbank stammt aus einer zu alten paperless-ngx-Version (älter als {FROM_VERSION}; {} fehlende Migration(en), \
             z. B. {}). Wege: (a) paperless-ngx zuerst auf 2.20.15 (oder neuer) aktualisieren und einmal starten, dann erneut – \
             paperless-ngx selbst verlangt das für Version 3 auch; oder (b) in paperless-ngx einen Export machen \
             (`document_exporter`) und diesen angeben.",
            not_catchable.len(),
            not_catchable.iter().take(3).map(|(a, n)| format!("{a}.{n}")).collect::<Vec<_>>().join(", "),
        ));
    }
    let steps: Vec<(String, String)> = known.into_iter().filter(|k| mc.missing.contains(k)).collect();
    let p = Plan { steps, marks };
    // 1075 without the conversion to v3 exists in no paperless-ngx version
    if p.needs_1075() && !p.is_v2() {
        return Err(
            "Migrationsstand passt zu keiner paperless-ngx-Version (documents.1075 fehlt, aber v3-Migrationen sind angewandt)".into()
        );
    }
    Ok(p)
}

/// Message for a newer, unknown paperless-ngx version.
pub fn newer_message(unknown: &[(String, String)]) -> String {
    format!(
        "Die Datenbank stammt aus einer neueren paperless-ngx-Version als {} ({} unbekannte Migration(en), z. B. {}). \
         Das kann PaperRust noch nicht direkt übernehmen; Ihre Datenbank bleibt unverändert. Ausweg: in paperless-ngx \
         einen Export machen (`document_exporter`) und diesen angeben (`paperrust migrate --from /pfad/zum/export`).",
        crate::schema::VERSION,
        unknown.len(),
        unknown.iter().take(3).map(|(a, n)| format!("{a}.{n}")).collect::<Vec<_>>().join(", ")
    )
}

/// Approximate paperless-ngx version from the migration state (for messages).
pub fn guess_version(plan: &Plan) -> &'static str {
    let has = |a: &str, n: &str| plan.steps.iter().any(|(x, y)| x == a && y.starts_with(n));
    if plan.is_empty() {
        "3.4.x"
    } else if plan.needs_1075() {
        "2.19.6/2.20.0–2.20.4"
    } else if plan.is_v2() {
        "2.20.x"
    } else if has("documents", "0023") || has("paperless", "0014") {
        "3.0.x"
    } else if has("paperless", "0016") || has("documents", "0026") {
        "3.1.x"
    } else {
        "3.2.x"
    }
}

/// Precheck of a 2.x source (all dialects): v3 no longer reads encrypted documents.
pub async fn precheck_v2(src: &mut crate::source::Source) -> Result<Option<String>> {
    let n = src.count_where("documents_document", Some("storage_type <> 'unencrypted'")).await?;
    Ok((n > 0).then(|| {
        format!(
            "{n} Dokument(e) sind in paperless-ngx verschlüsselt (GPG). paperless-ngx 3 und PaperRust lesen keine \
             verschlüsselten Dokumente mehr. Bitte zuerst in paperless-ngx 2.20 entschlüsseln (`decrypt_documents`, \
             siehe paperless-ngx docs/migration-v3.md) und dann erneut starten. Nichts verändert."
        )
    }))
}

/// Result of the catch-up.
#[derive(Debug, Default)]
pub struct Applied {
    /// applied steps (app.name), then recorded squash rows
    pub done: Vec<String>,
    /// Notes from the data migrations (e.g. missing files)
    pub notes: Vec<String>,
}

/// Catches up the steps in place (PostgreSQL, one transaction – inside an already
/// open transaction as a savepoint). `media`: media folder (documents.0016 reads the files).
pub async fn apply_in_place(conn: &mut sqlx::PgConnection, plan: &Plan, media: Option<&std::path::Path>) -> Result<Applied> {
    use sqlx::Connection;
    let steps = all_steps();
    let mut tx = conn.begin().await?;
    // instead of Django's `atomic = False` (paperless_mail.0002): no open FK checks before ALTER TABLE
    sqlx::raw_sql("SET CONSTRAINTS ALL IMMEDIATE").execute(&mut *tx).await?;
    let mut out = Applied::default();
    for (app, name) in &plan.steps {
        let Some(step) = steps.iter().find(|s| &s.app == app && &s.name == name) else {
            bail!("Nachhol-Schritt {app}.{name} fehlt in den SQL-Dateien");
        };
        for part in &step.parts {
            match part {
                Part::Sql(sql) => {
                    sqlx::raw_sql(sql.as_str())
                        .execute(&mut *tx)
                        .await
                        .with_context(|| format!("Nachholen {app}.{name} fehlgeschlagen"))?;
                }
                Part::Rust(hook) => {
                    run_hook(&mut tx, hook, media, &mut out.notes)
                        .await
                        .with_context(|| format!("Nachholen {app}.{name} ({hook}) fehlgeschlagen"))?;
                }
            }
        }
        sqlx::query("INSERT INTO django_migrations (app, name, applied) VALUES ($1, $2, now())")
            .bind(app)
            .bind(name)
            .execute(&mut *tx)
            .await?;
        out.done.push(format!("{app}.{name}"));
    }
    for (app, name) in &plan.marks {
        sqlx::query("INSERT INTO django_migrations (app, name, applied) VALUES ($1, $2, now())")
            .bind(app)
            .bind(name)
            .execute(&mut *tx)
            .await?;
        out.done.push(format!("{app}.{name}"));
    }
    sqlx::raw_sql("SET CONSTRAINTS ALL DEFERRED").execute(&mut *tx).await?;
    tx.commit().await.context("COMMIT des Nachholens fehlgeschlagen – Datenbank unverändert")?;
    Ok(out)
}

/// Data migrations that cannot be written as SQL.
pub async fn run_hook(tx: &mut sqlx::PgConnection, hook: &str, media: Option<&std::path::Path>, notes: &mut Vec<String>) -> Result<()> {
    match hook {
        "sha256_checksums" => sha256_checksums(tx, media, notes).await,
        "fulltext_query_prefixes" => fulltext_query_prefixes(tx).await,
        other => bail!("unbekannte Datenmigration {other}"),
    }
}

/// SHA-256 of a file, streamed (64 KiB like upstream `_CHUNK_SIZE`).
fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// upstream documents/migrations/0016_sha256_checksums.py::recompute_checksums: for every document
/// (also in the trash – the historical model class does not filter) original
/// `ORIGINALS_DIR / (filename or f"{pk:07}.pdf")` and, if `archive_filename` is set, archive
/// `ARCHIVE_DIR / archive_filename`; existing files → SHA-256, missing → checksum stays
/// (upstream: warning in the log, here a note in the report).
async fn sha256_checksums(tx: &mut sqlx::PgConnection, media: Option<&std::path::Path>, notes: &mut Vec<String>) -> Result<()> {
    use sqlx::Row;
    let rows = sqlx::query("SELECT id, filename, archive_filename FROM documents_document ORDER BY id").fetch_all(&mut *tx).await?;
    if rows.is_empty() {
        return Ok(());
    }
    let Some(media) = media else {
        notes.push(format!(
            "SHA-256-Prüfsummen nicht neu berechnet (Medienordner unbekannt): {} Dokument(e) behalten ihre MD5-Prüfsumme. \
             PaperRust prüft Dateien mit beiden Verfahren.",
            rows.len()
        ));
        return Ok(());
    };
    let orig = media.join("documents").join("originals");
    let arch = media.join("documents").join("archive");
    let mut missing: Vec<String> = Vec::new();
    let mut updated = 0usize;
    for r in rows {
        let id: i32 = r.try_get(0)?;
        let filename: Option<String> = r.try_get(1)?;
        let archive: Option<String> = r.try_get(2)?;
        let fname = filename.filter(|f| !f.is_empty()).unwrap_or_else(|| format!("{id:07}.pdf"));
        let p = orig.join(&fname);
        let ck = if p.is_file() {
            Some(sha256_file(&p).with_context(|| format!("{} lesen", p.display()))?)
        } else {
            missing.push(format!("Dokument {id}: Original {fname}"));
            None
        };
        let ack = match &archive {
            Some(a) => {
                let p = arch.join(a);
                if p.is_file() {
                    Some(sha256_file(&p).with_context(|| format!("{} lesen", p.display()))?)
                } else {
                    missing.push(format!("Dokument {id}: Archiv {a}"));
                    None
                }
            }
            None => None,
        };
        if ck.is_some() || ack.is_some() {
            sqlx::query(
                "UPDATE documents_document SET checksum = COALESCE($2, checksum), archive_checksum = COALESCE($3, archive_checksum) WHERE id = $1",
            )
            .bind(id)
            .bind(ck)
            .bind(ack)
            .execute(&mut *tx)
            .await?;
            updated += 1;
        }
    }
    if !missing.is_empty() {
        notes.push(format!(
            "SHA-256: {updated} Dokument(e) neu berechnet; {} Datei(en) fehlen, deren Prüfsumme bleibt (wie bei paperless-ngx): {}",
            missing.len(),
            missing.iter().take(10).cloned().collect::<Vec<_>>().join("; ")
        ));
    }
    Ok(())
}

/// Python `re`: `\w` (Unicode word character, `_`) or a dot before the match blocks the replacement.
fn blocks_prefix(c: char) -> bool {
    c == '.' || c == '_' || c.is_alphanumeric()
}

/// Replaces `pat` with `rep` if no word character and no dot precedes it – like Python
/// `re.sub(r"(?<![.\w])pat", rep, s)` (left to right, without overlap).
pub fn sub_prefix(s: &str, pat: &str, rep: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while let Some(off) = s[i..].find(pat) {
        let at = i + off;
        out.push_str(&s[i..at]);
        if s[..at].chars().next_back().is_some_and(blocks_prefix) {
            out.push_str(pat);
        } else {
            out.push_str(rep);
        }
        i = at + pat.len();
    }
    out.push_str(&s[i..]);
    out
}

/// upstream documents/migrations/0017_migrate_fulltext_query_field_prefixes.py.
pub fn migrate_fulltext_query(value: &str) -> String {
    let v = sub_prefix(value, "note:", "notes.note:");
    sub_prefix(&v, "custom_field:", "custom_fields.value:")
}

async fn fulltext_query_prefixes(tx: &mut sqlx::PgConnection) -> Result<()> {
    use sqlx::Row;
    let rows = sqlx::query("SELECT id, value FROM documents_savedviewfilterrule WHERE rule_type = 20 AND value IS NOT NULL ORDER BY id")
        .fetch_all(&mut *tx)
        .await?;
    for r in rows {
        let id: i32 = r.try_get(0)?;
        let v: String = r.try_get(1)?;
        let n = migrate_fulltext_query(&v);
        if n != v {
            sqlx::query("UPDATE documents_savedviewfilterrule SET value = $2 WHERE id = $1").bind(id).bind(n).execute(&mut *tx).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    #[test]
    fn steps_match_sql_file_and_schema() {
        let sql = sql_steps();
        assert_eq!(sql.len(), STEPS.len());
        for (s, (a, n, _)) in STEPS.iter().zip(&sql) {
            assert_eq!((s.app, s.name), (a.as_str(), n.as_str()), "Reihenfolge SQL-Datei ≠ STEPS");
        }
        let exp = schema::expected_migrations();
        let tables = schema::tables().unwrap();
        for s in STEPS {
            assert!(exp.contains(&(s.app.to_string(), s.name.to_string())), "{} {} nicht im v3.4.2-Stand", s.app, s.name);
            for (t, c, v) in s.add_columns {
                let tab = tables.iter().find(|x| x.name == *t).unwrap();
                let col = tab.columns.iter().find(|x| x.name == *c).unwrap_or_else(|| panic!("{t}.{c}"));
                assert!(v.is_some() || col.nullable, "{t}.{c}: NOT NULL ohne Wert");
            }
            for t in s.new_tables {
                assert!(tables.iter().any(|x| x.name == *t));
            }
        }
        // SQL creates exactly the columns/tables that STEPS describes
        for s in STEPS {
            let body = &sql.iter().find(|(a, n, _)| a == s.app && n == s.name).unwrap().2;
            for (t, c, _) in s.add_columns {
                assert!(body.contains(&format!("ALTER TABLE \"{t}\" ADD COLUMN \"{c}\"")), "{t}.{c}");
            }
            for t in s.new_tables {
                assert!(body.contains(&format!("CREATE TABLE \"{t}\"")));
            }
        }
    }

    fn check(missing: &[(&str, &str)]) -> MigrationCheck {
        MigrationCheck { missing: missing.iter().map(|(a, n)| (a.to_string(), n.to_string())).collect(), ..Default::default() }
    }

    #[test]
    fn plan_for_313_and_limits() {
        // State of paperless-ngx 3.1.3 (git ls-tree v3.1.3)
        let v313 = check(&[
            ("documents", "0026_alter_document_archive_checksum_and_more"),
            ("documents", "0027_documentbarcode"),
            ("paperless", "0016_alter_applicationconfiguration_ai_enabled"),
            ("paperless", "0017_applicationconfiguration_llm_embedding_api_key"),
            ("paperless", "0018_applicationconfiguration_barcode_store_values"),
        ]);
        let p = plan(&v313).unwrap();
        assert_eq!(p.steps.len(), 5);
        assert_eq!(p.steps[0].1, "0026_alter_document_archive_checksum_and_more");
        assert_eq!(guess_version(&p), "3.1.x");
        assert!(p.new_tables().contains("documents_documentbarcode"));
        // older than 2.19.6 (documents.1074 missing): cannot be caught up
        let old = check(&[("documents", "1074_workflowrun_deleted_at_workflowrun_restored_at_and_more")]);
        assert!(plan(&old).unwrap_err().contains("zu alten"));
        // newer: not supported
        let mut newer = MigrationCheck::default();
        newer.unknown.push(("documents".into(), "0028_future".into()));
        assert!(plan(&newer).unwrap_err().contains("neueren"));
        assert!(plan(&MigrationCheck::default()).unwrap().is_empty());
    }

    #[test]
    fn adapt_fills_and_transforms() {
        let tables = schema::tables().unwrap();
        let p = plan(&check(&[
            ("documents", "0023_savedview_icon"),
            ("paperless", "0014_applicationconfiguration_remote_ocr_api_key_and_more"),
            ("paperless", "0016_alter_applicationconfiguration_ai_enabled"),
            ("documents", "0027_documentbarcode"),
        ]))
        .unwrap();
        let sv = tables.iter().find(|t| t.name == "documents_savedview").unwrap();
        let a = p.adapt(sv);
        assert_eq!(a.select.len() + 1, sv.copy_columns().len());
        let read: Vec<Option<String>> = a.select.iter().map(|c| Some(format!("v_{}", c.name))).collect();
        let out = a.apply(&read);
        let icon = sv.copy_columns().iter().position(|c| c.name == "icon").unwrap();
        assert_eq!(out[icon].as_deref(), Some("funnel"));
        let ac = tables.iter().find(|t| t.name == "paperless_applicationconfiguration").unwrap();
        let a = p.adapt(ac);
        let ai_sel = a.select.iter().position(|c| c.name == "ai_enabled").unwrap();
        let mut read: Vec<Option<String>> = vec![None; a.select.len()];
        read[ai_sel] = Some("f".into());
        let out = a.apply(&read);
        let ai = ac.copy_columns().iter().position(|c| c.name == "ai_enabled").unwrap();
        assert_eq!(out[ai], None, "RunPython 0016: false → NULL");
        read[ai_sel] = Some("t".into());
        assert_eq!(a.apply(&read)[ai].as_deref(), Some("t"));
        let bc = tables.iter().find(|t| t.name == "documents_documentbarcode").unwrap();
        assert!(p.adapt(bc).skip_source);
        let rows = p.migration_rows(500, "2026-01-01 00:00:00+00");
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0][0].as_deref(), Some("500"));
    }

    /// django_migrations of a freshly migrated 2.20.15 DB (embedded) → plan like upstream 3.4.2
    /// `migrate --plan` on this DB (42 steps) plus the three squash rows.
    fn v220_check(without_1075: bool) -> MigrationCheck {
        let mut present = schema::v220::expected_migrations();
        present.insert(("django_celery_results".into(), "0001_initial".into()));
        if without_1075 {
            present.remove(&(V2_1075.0.to_string(), V2_1075.1.to_string()));
        }
        schema::check_migrations(&present)
    }

    #[test]
    fn plan_for_220() {
        let mc = v220_check(false);
        assert_eq!(mc.foreign_apps.len(), 1, "django_celery_results gilt als fremde App");
        let p = plan(&mc).unwrap();
        assert!(p.is_v2() && !p.needs_1075());
        assert_eq!(p.steps.len(), 42, "{:?}", p.names());
        assert_eq!(p.steps[0].1, "0003_remove_document_storage_type");
        assert_eq!(p.steps.last().unwrap().1, "0018_applicationconfiguration_barcode_store_values");
        let marks: Vec<String> = p.marks.iter().map(|(a, n)| format!("{a}.{n}")).collect();
        assert_eq!(marks, vec!["documents.0001_squashed", "documents.0002_squashed", "paperless_mail.0001_squashed"]);
        assert_eq!(guess_version(&p), "2.20.x");
        // after the conversion = exactly the v3.4.2 state (all rows present)
        let mut after = schema::v220::expected_migrations();
        for (a, n) in p.steps.iter().chain(p.marks.iter()) {
            after.insert((a.clone(), n.clone()));
        }
        let mc2 = schema::check_migrations(&after);
        assert!(mc2.ok(), "{:?}", mc2.missing);

        let p = plan(&v220_check(true)).unwrap();
        assert!(p.needs_1075());
        assert_eq!(p.steps[0].1, "1075_workflowaction_order");
        assert_eq!(guess_version(&p), "2.19.6/2.20.0–2.20.4");
        // 1075 alone (without the v2 conversion) is not a valid state
        assert!(plan(&check(&[("documents", "1075_workflowaction_order")])).is_err());
    }

    #[test]
    fn hooks_and_steps_parse() {
        let v2 = parse_steps(SQL_V2);
        assert_eq!(v2.len(), 33);
        let hooks: Vec<String> = v2
            .iter()
            .flat_map(|s| s.parts.iter())
            .filter_map(|p| match p {
                Part::Rust(h) => Some(h.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(hooks, vec!["sha256_checksums", "fulltext_query_prefixes"]);
        let all = all_steps();
        assert_eq!(all.len(), v2.len() + STEPS.len());
        let exp = schema::expected_migrations();
        for s in &all {
            assert!(exp.contains(&(s.app.clone(), s.name.clone())), "{} {} nicht im v3.4.2-Stand", s.app, s.name);
        }
        // documents.0016: SQL before the hook (column wider, then SHA-256)
        let s16 = v2.iter().find(|s| s.name == "0016_sha256_checksums").unwrap();
        assert!(matches!(&s16.parts[0], Part::Sql(x) if x.contains("varchar(64)")));
        assert_eq!(s16.parts[1], Part::Rust("sha256_checksums".into()));
    }

    #[test]
    fn fulltext_prefix_like_python_re() {
        // expected values from Python: re.sub(r"(?<![.\w])note:", "notes.note:", …) etc.
        assert_eq!(
            migrate_fulltext_query("note:wichtig custom_field:K-4711 denote:x notes.note:schon (note:a)"),
            "notes.note:wichtig custom_fields.value:K-4711 denote:x notes.note:schon (notes.note:a)"
        );
        assert_eq!(migrate_fulltext_query("-note:b +custom_field:c"), "-notes.note:b +custom_fields.value:c");
        assert_eq!(migrate_fulltext_query("ünote: _note: 1note: .note:"), "ünote: _note: 1note: .note:");
        assert_eq!(migrate_fulltext_query("note:note:"), "notes.note:notes.note:");
        assert_eq!(migrate_fulltext_query("kein Treffer"), "kein Treffer");
    }
}
