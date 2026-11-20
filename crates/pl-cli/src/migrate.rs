//! `manage migrate` / `showmigrations`.
//!
//! upstream: django/core/management/commands/migrate.py (output "Operations to perform" /
//! "Running migrations" / `--check`, `--plan`), django/contrib/contenttypes/management
//! (`create_contenttypes`), django/contrib/auth/management (`create_permissions`),
//! guardian/management (`create_anonymous_user`), Datenmigration
//! src/paperless/migrations (ApplicationConfiguration singleton) – all at v3.4.2.
//!
//! - **Empty DB** (no relation in `public`): new installation in one transaction: schema
//!   (pre-data from pl-dbmigrate), `django_migrations` (247 rows in upstream order),
//!   ContentTypes, permissions, `AnonymousUser`, ApplicationConfiguration, sequence values, post-data
//!   (constraints/indexes) – result identical to after upstream `manage.py migrate`.
//! - **Existing DB**: must be exactly v3.4.2 (logic like pl-dbmigrate `check_migrations`), otherwise
//!   a clear message. Then the post_migrate catch-up (missing ContentTypes/permissions/AnonymousUser).
//! - Afterwards the own expand-only migrations `paperrust_*` (pl-queue, pl-ai), recorded in
//!   `paperrust_migrations`.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::{Connection, PgConnection, Row};

use crate::args::{self, flag};
use crate::baseline;
use crate::ctx::{self, Ctx};
use crate::{CliError, CliResult};

/// Own migrations (name, SQL) in order. All idempotent and expand-only.
pub fn paperrust_migrations() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<(&'static str, &'static str)> = pl_queue::migrations::ALL.to_vec();
    // pl-ai does not (yet) provide a constant → file directly (CORE-REQUEST to W8: pl_ai::migrations::ALL).
    v.push(("pl_ai.0001_paperrust_ai", include_str!("../../pl-ai/migrations/0001_paperrust_ai.sql")));
    v.push(("pl_ai.0002_paperrust_ai_mcp_scope", include_str!("../../pl-ai/migrations/0002_paperrust_ai_mcp_scope.sql")));
    // tests/chaos C6: without 0003, paperrust_ai_prompt/_option/_job were missing after `manage migrate` (only the
    // server start created them) – a restore via document_importer lost their content.
    v.push(("pl_ai.0003_paperrust_ai_prompts_jobs", include_str!("../../pl-ai/migrations/0003_paperrust_ai_prompts_jobs.sql")));
    // Sync change capture (P2, docs/SYNC.md §2): side tables + AFTER triggers.
    v.extend_from_slice(pl_sync::migrations::ALL);
    // Device tokens (P3b, pl-authz): paperrust_device_token.
    v.extend_from_slice(pl_authz::device::migrations::ALL);
    // Z2 integrity check: confirmed checksums for the incremental mode.
    v.push(crate::sanity::MIGRATION);
    // System settings from the UI (docs/SETTINGS.md §2.2): paperrust_setting/_history/_rev.
    v.extend_from_slice(pl_settings::migrations::ALL);
    // ASN label generator (pl-labels): printed/reserved ASN ranges paperrust_asn_print.
    v.extend_from_slice(pl_labels::store::migrations::ALL);
    // PaperRust UI (pl-timeline): life areas, document data, timeline state, /ask history.
    v.extend_from_slice(pl_timeline::store::migrations::ALL);
    // Roles (pl-api-v2, docs/DESIGN2-ENTWIRREN.md §1): paperrust_role/_assignment/_config. File directly
    // (pl-cli does not depend on pl-api-v2); same SQL as pl_api_v2::roles::MIGRATION.
    v.push(("pl_api_v2.0001_paperrust_role", include_str!("../../pl-api-v2/migrations/0001_paperrust_role.sql")));
    // L1 (ARCHITECTURE-V2 §5): partial index of the document list for the filter AST (pl_store::docsql).
    v.extend_from_slice(pl_store::migrations::ALL);
    // L5 (ARCHITECTURE-V2 §5): grant index paperrust_grant (derived from guardian, trigger).
    v.extend_from_slice(pl_authz::grants::migrations::ALL);
    // L3 (docs/contract/pl-search.md): search index consistency for permissions/trash
    v.extend_from_slice(pl_indexer::migrations::ALL);
    // L6 step 1 (docs/DATAMODEL-V2.md §1.9): tag tree as adjacency list, treenode columns removed.
    v.extend_from_slice(pl_objects::migrations::ALL);
    // Scanner integration (pl-scan): known eSCL scanners, devices for SMTP receipt.
    v.extend_from_slice(pl_scan::store::migrations::ALL);
    // Ransomware protection (pl-guard): guard state, events, backup runs.
    v.extend_from_slice(pl_guard::store::migrations::ALL);
    // Shared cache for multi-process operation (PAPERRUST_CACHE=postgres, replaces Redis).
    v.extend_from_slice(pl_core::cache::migrations::ALL);
    // Workflows new (docs/WORKFLOWS-NEU.md): history, else branch, AI condition (pl-consumer workflow_ext).
    v.extend_from_slice(pl_consumer::workflow_ext::migrations::ALL);
    // Watch list of redacted versions (pl-documents-write redaction_marks, sharing hint).
    v.extend_from_slice(pl_documents_write::redaction_marks::migrations::ALL);
    // L6 conversion stages (docs/DATAMODEL-V2.md §2.2): not expand-only, always last, backup beforehand
    // (crate::datamodel::backup_before), rollback `manage datamodel rollback`.
    v.extend(pl_store::datamodel::migrations());
    // Indexes on tables of the conversion stages (expand-only, check themselves which table exists).
    v.extend_from_slice(pl_store::migrations::AFTER_STAGES);
    v
}

const PAPERRUST_TRACKING: &str = "CREATE TABLE IF NOT EXISTS paperrust_migrations (
    name varchar(255) PRIMARY KEY,
    applied timestamptz NOT NULL DEFAULT now()
)";

/// State of the target database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbState {
    /// no relation in `public`
    Empty,
    /// v3.4.2 complete; `extra` = additional, allowed rows (replaced/historical/foreign app)
    Current { extra: usize },
    /// missing v3.4.2 migrations (older version) and/or unknown ones (newer version)
    Mismatch { missing: Vec<(String, String)>, unknown: Vec<(String, String)> },
    /// tables present, but no `django_migrations`
    Foreign { relations: i64 },
}

pub async fn detect_state(conn: &mut PgConnection) -> CliResult<DbState> {
    let rels: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND c.relkind IN ('r','p','v','m','S','f')",
    )
    .fetch_one(&mut *conn)
    .await?;
    if rels == 0 {
        return Ok(DbState::Empty);
    }
    let has: bool = sqlx::query_scalar("SELECT to_regclass('public.django_migrations') IS NOT NULL").fetch_one(&mut *conn).await?;
    if !has {
        return Ok(DbState::Foreign { relations: rels });
    }
    let rows = sqlx::query("SELECT app, name FROM django_migrations").fetch_all(&mut *conn).await?;
    let mut present = BTreeSet::new();
    for r in rows {
        present.insert((r.try_get::<String, _>(0)?, r.try_get::<String, _>(1)?));
    }
    Ok(classify(&present))
}

/// Migrations from paperless-ngx after v3.4.2 (upstream `dev`) whose effect PaperRust knows (same
/// list as pl-dbmigrate `schema::KNOWN_NEWER`): documents.0028 removes the treenode columns like
/// `pl_objects.0001_paperrust_tag_adjacency` (idempotent), 0029 only changes a choice list.
pub const KNOWN_NEWER: &[(&str, &str)] = &[
    ("documents", "0028_replace_tag_tree_fields"),
    ("documents", "0029_alter_workflowrun_type_alter_workflowtrigger_type"),
];

/// First migration that only needs a paperless-ngx 2.x database.
const V2_MARKER: (&str, &str) = ("documents", "0003_remove_document_storage_type");

/// Migration state like pl-dbmigrate `schema.rs::check_migrations`.
pub fn classify(present: &BTreeSet<(String, String)>) -> DbState {
    let expected: BTreeSet<(String, String)> =
        baseline::rows(baseline::MIGRATIONS_TSV).into_iter().map(|r| (r[0].to_string(), r[1].to_string())).collect();
    let replaced: BTreeSet<(String, String)> =
        baseline::rows(baseline::REPLACED_TSV).into_iter().map(|r| (r[0].to_string(), r[1].to_string())).collect();
    let historical: BTreeSet<(String, String)> =
        baseline::rows(baseline::HISTORICAL_TSV).into_iter().map(|r| (r[0].to_string(), r[1].to_string())).collect();
    let apps = baseline::installed_apps();
    let missing: Vec<(String, String)> = expected.difference(present).cloned().collect();
    let mut unknown = Vec::new();
    let mut extra = 0;
    for m in present.difference(&expected) {
        let newer = KNOWN_NEWER.iter().any(|(a, n)| *a == m.0 && *n == m.1);
        if replaced.contains(m) || historical.contains(m) || newer || !apps.contains(&m.0) {
            extra += 1;
        } else {
            unknown.push(m.clone());
        }
    }
    if missing.is_empty() && unknown.is_empty() {
        DbState::Current { extra }
    } else {
        DbState::Mismatch { missing, unknown }
    }
}

/// Catch-up steps (app, name, sql) from `CATCHUP_SQL` in file order (format like
/// pl-dbmigrate `catchup::sql_steps`).
pub fn catchup_steps() -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for line in baseline::CATCHUP_SQL.lines() {
        let l = line.trim_end_matches('\r');
        if let Some(rest) = l.strip_prefix("-- @@ ") {
            let mut it = rest.split_whitespace();
            out.push((it.next().unwrap_or_default().to_string(), it.next().unwrap_or_default().to_string(), String::new()));
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

/// Can an older database (paperless-ngx from v3.0.0) be caught up? Then the missing
/// steps in order; `None` for a newer/foreign or too old database.
pub fn catchup_plan(missing: &[(String, String)], unknown: &[(String, String)]) -> Option<Vec<(String, String, String)>> {
    if !unknown.is_empty() || missing.is_empty() {
        return None;
    }
    let steps = catchup_steps();
    if !missing.iter().all(|m| steps.iter().any(|(a, n, _)| a == &m.0 && n == &m.1)) {
        return None;
    }
    Some(steps.into_iter().filter(|(a, n, _)| missing.iter().any(|m| &m.0 == a && &m.1 == n)).collect())
}

fn mismatch_message(missing: &[(String, String)], unknown: &[(String, String)]) -> String {
    if unknown.is_empty() && missing.iter().any(|(a, n)| a == V2_MARKER.0 && n == V2_MARKER.1) {
        return "This is a paperless-ngx 2.x database (2.20 or 2.19.6). PaperRust converts it with the migration \
                assistant, which makes a backup first and then applies the same changes as paperless-ngx 3: \
                `paperrust migrate --from <database URL> --media <media folder>` (the media folder is needed to \
                recompute the SHA-256 checksums). Nothing was changed."
            .to_string();
    }
    let mut s = String::from("This database is not at paperless-ngx v3.4.2.\n");
    if !missing.is_empty() {
        s.push_str(&format!(
            "  {} migration(s) of v3.4.2 are not applied (paperless-ngx older than 2.20 – PaperRust takes over 2.20 and \
             3.x directly). Update paperless-ngx to 2.20.15 (or 3.4.2) and start it once (it runs `manage.py migrate`), \
             or import a `document_exporter` export instead. First missing:\n",
            missing.len()
        ));
        for (a, n) in missing.iter().take(10) {
            s.push_str(&format!("    {a}.{n}\n"));
        }
    }
    if !unknown.is_empty() {
        s.push_str(&format!(
            "  {} unknown migration(s) (database from a newer paperless-ngx than 3.4.2 that PaperRust does not know \
             yet). Nothing was changed. Make an export in paperless-ngx (`document_exporter`) and import it instead \
             (`paperrust migrate --from <export folder>`):\n",
            unknown.len()
        ));
        for (a, n) in unknown.iter().take(10) {
            s.push_str(&format!("    {a}.{n}\n"));
        }
    }
    s.trim_end().to_string()
}

const OPTS: &[args::Opt] = &[
    flag(&["--no-input", "--noinput"], "no_input"),
    flag(&["--check"], "check"),
    flag(&["--plan"], "plan"),
    args::value(&["--database"], "database"),
    flag(&["--skip-paperrust"], "skip_paperrust"),
    flag(&["--no-backup"], "no_backup"),
];

/// Lines "Applying app.name... OK" in Django's plan order (replaced names of squashed
/// migrations do not appear; from the upstream `migrate` output, `baseline/v3.4.2-applying.txt`).
fn applying_lines() -> Vec<String> {
    include_str!("../baseline/v3.4.2-applying.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 0)?;
    let cfg = ctx::load_config()?;
    let pool = ctx::connect_pool(&cfg).await?;
    let mut conn = pool.acquire().await?;
    let state = detect_state(&mut conn).await?;
    let apps: Vec<String> = baseline::installed_apps().into_iter().collect();
    if a.flag("plan") {
        println!("Planned operations:");
        let mut any = false;
        // Django part; `migrate` would only continue with an empty, current or catch-up-able DB.
        let runs = match &state {
            DbState::Empty => {
                for l in applying_lines() {
                    println!("{l}");
                    println!("    (PaperRust: v3.4.2 schema snapshot)");
                    any = true;
                }
                true
            }
            DbState::Mismatch { missing, unknown } => match catchup_plan(missing, unknown) {
                Some(steps) => {
                    for (app, name, _) in steps {
                        println!("{app}.{name}");
                        println!("    (PaperRust: catch-up from an older paperless-ngx)");
                        any = true;
                    }
                    true
                }
                None => false,
            },
            DbState::Current { .. } => true,
            DbState::Foreign { .. } => false,
        };
        // Own migrations (D-070): show them too, otherwise an open conversion looks done.
        if runs && !a.flag("skip_paperrust") {
            for l in paperrust_plan_lines(&pending_paperrust(&mut conn).await?) {
                println!("{l}");
                any = true;
            }
        }
        if !any {
            println!("  No planned migration operations.");
        }
        return Ok(());
    }
    if a.flag("check") {
        // Django: --check exits with 1 if migrations are pending.
        return match state {
            DbState::Current { .. } => {
                let pending = pending_paperrust(&mut conn).await?;
                if pending.is_empty() || a.flag("skip_paperrust") { Ok(()) } else { Err(CliError::Command(format!("unapplied PaperRust migrations: {}", pending.join(", ")))) }
            }
            DbState::Empty => Err(CliError::Command("database is empty, migrations pending".into())),
            DbState::Mismatch { missing, unknown } => Err(CliError::Command(match catchup_plan(&missing, &unknown) {
                Some(steps) => format!(
                    "unapplied migrations (older paperless-ngx, `migrate` catches them up): {}",
                    steps.iter().map(|(a, n, _)| format!("{a}.{n}")).collect::<Vec<_>>().join(", ")
                ),
                None => mismatch_message(&missing, &unknown),
            })),
            DbState::Foreign { relations } => Err(CliError::Command(foreign_message(relations))),
        };
    }
    match &state {
        DbState::Mismatch { missing, unknown } if catchup_plan(missing, unknown).is_none() => {
            return Err(CliError::Command(mismatch_message(missing, unknown)));
        }
        DbState::Foreign { relations } => return Err(CliError::Command(foreign_message(*relations))),
        _ => {}
    }
    println!("Operations to perform:");
    println!("  Apply all migrations: {}", apps.join(", "));
    println!("Running migrations:");
    match state {
        DbState::Empty => {
            fresh_install(&mut conn).await?;
            for l in applying_lines() {
                println!("  Applying {l}... OK");
            }
        }
        DbState::Current { .. } => {
            let mut tx = conn.begin().await?;
            post_migrate(&mut tx).await?;
            tx.commit().await?;
            println!("  No migrations to apply.");
        }
        DbState::Mismatch { missing, unknown } => {
            // Older paperless-ngx database (from v3.0.0): catch up missing migrations like Django,
            // in one transaction (on error everything stays as it was).
            let steps = catchup_plan(&missing, &unknown)
                .ok_or_else(|| CliError::Command(mismatch_message(&missing, &unknown)))?;
            let mut tx = conn.begin().await?;
            for (app, name, sql) in &steps {
                if !sql.trim().is_empty() {
                    sqlx::raw_sql(sql.as_str()).execute(&mut *tx).await?;
                }
                sqlx::query("INSERT INTO django_migrations (app, name, applied) VALUES ($1, $2, clock_timestamp())")
                    .bind(app)
                    .bind(name)
                    .execute(&mut *tx)
                    .await?;
            }
            post_migrate(&mut tx).await?;
            verify_structure(&mut tx).await?;
            tx.commit().await?;
            for (app, name, _) in &steps {
                println!("  Applying {app}.{name}... OK");
            }
        }
        DbState::Foreign { relations } => return Err(CliError::Command(foreign_message(relations))),
    }
    if !a.flag("skip_paperrust") {
        let applied = apply_paperrust_with(&mut conn, Some((&cfg, a.flag("no_backup")))).await?;
        if !applied.is_empty() {
            println!("Running PaperRust migrations:");
            for n in applied {
                println!("  Applying {n}... OK");
            }
        }
    }
    Ok(())
}

fn foreign_message(relations: i64) -> String {
    format!(
        "The database is not empty ({relations} relations in schema public) but has no django_migrations table – \
         it is not a paperless-ngx database. Use an empty database for a new installation."
    )
}

/// post-data with Django's original expression for the partial index of allauth-mfa: the
/// pg_dump text (`= ANY ((ARRAY[…])::text[])`) would be stored differently on re-reading
/// (`ARRAY[(…)::text, …]`); `IN (…)` like Django's `UniqueConstraint(condition=Q(type__in=…))`
/// yields the same expression tree as after upstream `migrate`.
pub fn schema_post() -> String {
    baseline::SCHEMA_POST.replace(
        "WHERE ((type)::text = ANY ((ARRAY['totp'::character varying, 'recovery_codes'::character varying])::text[]));",
        "WHERE (\"type\" IN ('totp', 'recovery_codes'));",
    )
}

/// New installation (one transaction; on error the DB stays empty).
pub async fn fresh_install(conn: &mut PgConnection) -> CliResult {
    let mut tx = conn.begin().await?;
    sqlx::raw_sql(baseline::SCHEMA_PRE).execute(&mut *tx).await?;
    // django_migrations in application order (applied = now() per row, like Django)
    for (app, name) in baseline::migrations_applied() {
        sqlx::query("INSERT INTO django_migrations (app, name, applied) VALUES ($1, $2, clock_timestamp())")
            .bind(&app)
            .bind(&name)
            .execute(&mut *tx)
            .await?;
    }
    for ct in baseline::content_types() {
        sqlx::query("INSERT INTO django_content_type (id, app_label, model) VALUES ($1, $2, $3)")
            .bind(ct.id)
            .bind(&ct.app_label)
            .bind(&ct.model)
            .execute(&mut *tx)
            .await?;
    }
    for p in baseline::permissions() {
        sqlx::query("INSERT INTO auth_permission (id, name, content_type_id, codename) VALUES ($1, $2, $3, $4)")
            .bind(p.id)
            .bind(&p.name)
            .bind(p.content_type_id)
            .bind(&p.codename)
            .execute(&mut *tx)
            .await?;
    }
    // guardian create_anonymous_user (id 1, unusable password)
    insert_anonymous_user(&mut tx, Some(1)).await?;
    // data migration paperless: ApplicationConfiguration singleton
    for line in include_str!("../baseline/v3.4.2-applicationconfiguration.json").lines().filter(|l| !l.trim().is_empty()) {
        sqlx::query(
            "INSERT INTO paperless_applicationconfiguration SELECT * FROM json_populate_record(NULL::paperless_applicationconfiguration, $1::json)",
        )
        .bind(line)
        .execute(&mut *tx)
        .await?;
    }
    for r in baseline::rows(include_str!("../baseline/v3.4.2-sequences.tsv")) {
        let (Some(seq), Some(val)) = (r.first(), r.get(1).and_then(|v| v.parse::<i64>().ok())) else { continue };
        sqlx::query("SELECT setval($1::regclass, $2, true)").bind(format!("public.{seq}")).bind(val).execute(&mut *tx).await?;
    }
    sqlx::raw_sql(&schema_post()).execute(&mut *tx).await?;
    verify_structure(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn insert_anonymous_user(conn: &mut PgConnection, id: Option<i32>) -> CliResult {
    let pw = pl_authz::hashers::make_unusable_password();
    match id {
        Some(id) => {
            sqlx::query(
                "INSERT INTO auth_user (id, password, last_login, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
                 VALUES ($1, $2, NULL, false, 'AnonymousUser', '', '', '', false, true, now())",
            )
            .bind(id)
            .bind(&pw)
            .execute(&mut *conn)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO auth_user (password, last_login, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
                 VALUES ($1, NULL, false, 'AnonymousUser', '', '', '', false, true, now())",
            )
            .bind(&pw)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

/// Columns (`information_schema.columns`) against `v3.4.2-columns.tsv` (like pl-dbmigrate).
async fn verify_structure(conn: &mut PgConnection) -> CliResult {
    let rows = sqlx::query(
        "SELECT c.table_name, c.column_name, c.udt_name FROM information_schema.columns c
         JOIN information_schema.tables t ON t.table_schema = c.table_schema AND t.table_name = c.table_name AND t.table_type = 'BASE TABLE'
         WHERE c.table_schema = 'public' ORDER BY 1, c.ordinal_position",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut got = Vec::new();
    for r in rows {
        let t: String = r.try_get(0)?;
        if t.starts_with("paperrust_") {
            continue;
        }
        got.push(format!("{t}\t{}\t{}", r.try_get::<String, _>(1)?, r.try_get::<String, _>(2)?));
    }
    let mut want: Vec<String> = baseline::rows(baseline::COLUMNS_TSV).into_iter().map(|r| format!("{}\t{}\t{}", r[0], r[1], r[3])).collect();
    // Order does not matter: when catching up an older DB (e.g. 3.1.3) ALTER TABLE ADD COLUMN appends
    // new columns at the end, so the order then differs from a new installation
    // (real test 2026-11-16: otherwise rollback of the whole catch-up).
    got.sort();
    want.sort();
    if got != want {
        return Err(CliError::Other("schema check failed after installation (columns differ from v3.4.2)".into()));
    }
    Ok(())
}

/// post_migrate on an existing v3.4.2 DB: missing ContentTypes/permissions (like
/// `create_contenttypes`/`create_permissions`, new IDs from the sequence) and `AnonymousUser`.
/// Existing rows are never changed or deleted (Django removes stale types only with
/// `remove_stale_contenttypes`).
pub async fn post_migrate(conn: &mut PgConnection) -> CliResult {
    let rows = sqlx::query("SELECT id, app_label, model FROM django_content_type").fetch_all(&mut *conn).await?;
    let mut ct_by_key: BTreeMap<(String, String), i32> = BTreeMap::new();
    for r in rows {
        ct_by_key.insert((r.try_get(1)?, r.try_get(2)?), r.try_get(0)?);
    }
    let base_cts = baseline::content_types();
    let mut base_to_db: BTreeMap<i32, i32> = BTreeMap::new();
    for ct in &base_cts {
        let key = (ct.app_label.clone(), ct.model.clone());
        let id = match ct_by_key.get(&key) {
            Some(id) => *id,
            None => {
                let id: i32 = sqlx::query_scalar("INSERT INTO django_content_type (app_label, model) VALUES ($1, $2) RETURNING id")
                    .bind(&ct.app_label)
                    .bind(&ct.model)
                    .fetch_one(&mut *conn)
                    .await?;
                ct_by_key.insert(key, id);
                id
            }
        };
        base_to_db.insert(ct.id, id);
    }
    let rows = sqlx::query("SELECT content_type_id, codename FROM auth_permission").fetch_all(&mut *conn).await?;
    let mut perms: BTreeSet<(i32, String)> = BTreeSet::new();
    for r in rows {
        perms.insert((r.try_get(0)?, r.try_get(1)?));
    }
    for p in baseline::permissions() {
        let Some(ct) = base_to_db.get(&p.content_type_id) else { continue };
        if !perms.contains(&(*ct, p.codename.clone())) {
            sqlx::query("INSERT INTO auth_permission (name, content_type_id, codename) VALUES ($1, $2, $3)")
                .bind(&p.name)
                .bind(ct)
                .bind(&p.codename)
                .execute(&mut *conn)
                .await?;
        }
    }
    let anon: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth_user WHERE username = 'AnonymousUser')").fetch_one(&mut *conn).await?;
    if !anon {
        insert_anonymous_user(conn, None).await?;
    }
    Ok(())
}

/// `--plan` lines for open own migrations (per name + explanation, like Django's plan).
pub fn paperrust_plan_lines(pending: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for n in pending {
        out.push(format!("paperrust.{n}"));
        out.push(if pl_store::datamodel::is_stage(n) {
            "    (PaperRust: data model conversion stage – backup with pg_dump first)".to_string()
        } else {
            "    (PaperRust: own migration)".to_string()
        });
    }
    out
}

/// Startup check for `paperrust serve` (test log 2026-11-16 A-181/B-312): `Ok(())` if the
/// database is at the state that `manage migrate` produces (v3.4.2 + all own
/// migrations); otherwise the message that the server prints before aborting.
pub async fn startup_check(conn: &mut PgConnection) -> CliResult<()> {
    let state = detect_state(conn).await?;
    let pending = match state {
        DbState::Current { .. } => pending_paperrust(conn).await?,
        _ => Vec::new(),
    };
    match startup_message(&state, &pending) {
        None => Ok(()),
        Some(m) => Err(CliError::Command(m)),
    }
}

/// Message for [`startup_check`] (without DB, testable). `None` = everything applied.
pub fn startup_message(state: &DbState, pending: &[String]) -> Option<String> {
    const FIX: &str = "Datenbank ist nicht auf dem neuesten Stand – zuerst `paperrust manage migrate` ausführen \
                       (database is not up to date – run `paperrust manage migrate` first)";
    let why = match state {
        DbState::Current { .. } if pending.is_empty() => return None,
        DbState::Current { .. } => {
            let mut names: Vec<&str> = pending.iter().take(5).map(String::as_str).collect();
            if pending.len() > 5 {
                names.push("…");
            }
            format!("{} PaperRust migration(s) not applied: {}", pending.len(), names.join(", "))
        }
        DbState::Empty => "the database is empty".to_string(),
        DbState::Mismatch { .. } => "the paperless-ngx schema version does not match v3.4.2".to_string(),
        DbState::Foreign { relations } => format!("{relations} relations but no django_migrations table"),
    };
    Some(format!("{FIX}: {why}"))
}

async fn pending_paperrust(conn: &mut PgConnection) -> CliResult<Vec<String>> {
    let has: bool = sqlx::query_scalar("SELECT to_regclass('public.paperrust_migrations') IS NOT NULL").fetch_one(&mut *conn).await?;
    let done: BTreeSet<String> = if has {
        sqlx::query_scalar("SELECT name FROM paperrust_migrations").fetch_all(&mut *conn).await?.into_iter().collect()
    } else {
        BTreeSet::new()
    };
    Ok(paperrust_migrations().into_iter().map(|(n, _)| n.to_string()).filter(|n| !done.contains(n)).collect())
}

/// Gives the sequences of the paperless-ngx tables their usual name `<table>_<column>_seq`.
/// Databases from old versions still carry the old names after table renames
/// (e.g. `documents_sender_id_seq` for `documents_correspondent`, `documents_comment_id_seq` for
/// `documents_note`); the data model conversion stages address the sequences by the usual
/// name (real test 2026-11-16 with a real 3.1.3 database). Only rename if the
/// target name is free; names over 63 characters stay (PostgreSQL truncates them differently).
const NORMALIZE_SEQUENCES: &str = r#"
DO $$
DECLARE r record;
BEGIN
  FOR r IN
    SELECT s.relname AS seq, c.relname || '_' || a.attname || '_seq' AS want
    FROM pg_depend d
    JOIN pg_class s ON s.oid = d.objid AND s.relkind = 'S'
    JOIN pg_namespace n ON n.oid = s.relnamespace AND n.nspname = 'public'
    JOIN pg_class c ON c.oid = d.refobjid AND c.relkind IN ('r', 'p')
    JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = d.refobjsubid
    WHERE d.deptype IN ('a', 'i')
      AND c.relname NOT LIKE 'paperrust\_%'
      AND s.relname <> c.relname || '_' || a.attname || '_seq'
      AND length(c.relname || '_' || a.attname || '_seq') <= 63
  LOOP
    IF to_regclass(format('public.%I', r.want)) IS NULL THEN
      EXECUTE format('ALTER SEQUENCE public.%I RENAME TO %I', r.seq, r.want);
    END IF;
  END LOOP;
END $$;
"#;

/// Applies pending `paperrust_*` migrations (one transaction each). Returns: names.
/// Data model conversion stages run without a prior backup (see [`apply_paperrust_with`]).
pub async fn apply_paperrust(conn: &mut PgConnection) -> CliResult<Vec<String>> {
    apply_paperrust_with(conn, None).await
}

/// Like [`apply_paperrust`]; with `backup = Some((config, no_backup))` a backup is made before every conversion stage of the
/// data model (`pl_store::datamodel`), provided the database contains documents.
pub async fn apply_paperrust_with(conn: &mut PgConnection, backup: Option<(&pl_core::config::Config, bool)>) -> CliResult<Vec<String>> {
    sqlx::raw_sql(PAPERRUST_TRACKING).execute(&mut *conn).await?;
    let pending = pending_paperrust(conn).await?;
    if !pending.is_empty() {
        sqlx::raw_sql(NORMALIZE_SEQUENCES).execute(&mut *conn).await?;
    }
    let mut applied = Vec::new();
    for (name, sql) in paperrust_migrations() {
        if !pending.iter().any(|p| p == name) {
            continue;
        }
        if let Some((cfg, no_backup)) = backup
            && pl_store::datamodel::is_stage(name)
        {
            match crate::datamodel::backup_before(cfg, conn, name, no_backup).await? {
                crate::datamodel::Backup::NotNeeded => {}
                crate::datamodel::Backup::Skipped => println!("  Backup before {name}: skipped (--no-backup)"),
                crate::datamodel::Backup::NoTool => println!(
                    "  Backup before {name}: pg_dump not found – continuing WITHOUT backup \
                     (PAPERRUST_MIGRATE_WITHOUT_BACKUP=true); `paperrust manage datamodel rollback` can undo the stage"
                ),
                crate::datamodel::Backup::Written(f) => println!("  Backup before {name}: {}", f.display()),
            }
        }
        // Files with their own BEGIN/COMMIT (pl-ai) run unchanged; otherwise their own transaction.
        let own_tx = sql.lines().any(|l| l.trim().eq_ignore_ascii_case("BEGIN;"));
        if own_tx {
            sqlx::raw_sql(sql).execute(&mut *conn).await?;
            sqlx::query("INSERT INTO paperrust_migrations (name) VALUES ($1) ON CONFLICT DO NOTHING").bind(name).execute(&mut *conn).await?;
        } else {
            let mut tx = conn.begin().await?;
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO paperrust_migrations (name) VALUES ($1) ON CONFLICT DO NOTHING").bind(name).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        applied.push(name.to_string());
    }
    Ok(applied)
}

const SHOW_OPTS: &[args::Opt] = &[flag(&["-l", "--list"], "list"), flag(&["-p", "--plan"], "plan"), args::value(&["--database"], "database")];

/// `showmigrations`: `[X]` per v3.4.2 migration (Django format "app\n [X] name") + PaperRust.
pub async fn showmigrations(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, SHOW_OPTS, 64)?;
    let c = Ctx::connect().await?;
    let mut conn = c.pool.acquire().await?;
    let has: bool = sqlx::query_scalar("SELECT to_regclass('public.django_migrations') IS NOT NULL").fetch_one(&mut *conn).await?;
    let present: BTreeSet<(String, String)> = if has {
        let rows = sqlx::query("SELECT app, name FROM django_migrations").fetch_all(&mut *conn).await?;
        rows.into_iter().filter_map(|r| Some((r.try_get(0).ok()?, r.try_get(1).ok()?))).collect()
    } else {
        BTreeSet::new()
    };
    let replaced: BTreeSet<(String, String)> =
        baseline::rows(baseline::REPLACED_TSV).into_iter().map(|r| (r[0].to_string(), r[1].to_string())).collect();
    let mut by_app: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (app, name) in baseline::migrations_applied() {
        if replaced.contains(&(app.clone(), name.clone())) {
            continue;
        }
        by_app.entry(app).or_default().push(name);
    }
    let filter: BTreeSet<&str> = a.positionals.iter().map(String::as_str).collect();
    for (app, names) in by_app {
        if !filter.is_empty() && !filter.contains(app.as_str()) {
            continue;
        }
        println!("{app}");
        for n in names {
            let x = if present.contains(&(app.clone(), n.clone())) { "X" } else { " " };
            println!(" [{x}] {n}");
        }
    }
    if filter.is_empty() {
        let pending = pending_paperrust(&mut conn).await.unwrap_or_default();
        println!("paperrust");
        for (n, _) in paperrust_migrations() {
            let x = if pending.iter().any(|p| p == n) { " " } else { "X" };
            println!(" [{x}] {n}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present_all() -> BTreeSet<(String, String)> {
        baseline::migrations_applied().into_iter().collect()
    }

    /// D-070: open own migrations appear in the plan, conversion stages with a hint.
    #[test]
    fn plan_lists_paperrust_migrations() {
        let all: Vec<String> = paperrust_migrations().into_iter().map(|(n, _)| n.to_string()).collect();
        let lines = paperrust_plan_lines(&all);
        assert_eq!(lines.len(), all.len() * 2);
        assert_eq!(lines[0], format!("paperrust.{}", all[0]));
        let stage = all.iter().position(|n| pl_store::datamodel::is_stage(n)).expect("eine Umbaustufe");
        assert!(lines[stage * 2 + 1].contains("backup"));
        assert!(paperrust_plan_lines(&[]).is_empty());
    }

    /// A-181/B-312: no silent start on a non-migrated database.
    #[test]
    fn startup_message_cases() {
        assert_eq!(startup_message(&DbState::Current { extra: 0 }, &[]), None);
        let m = startup_message(&DbState::Current { extra: 0 }, &["a".into(), "b".into()]).unwrap();
        assert!(m.contains("zuerst `paperrust manage migrate` ausführen") && m.contains("2 PaperRust migration(s) not applied: a, b"));
        let many: Vec<String> = (0..8).map(|i| format!("m{i}")).collect();
        assert!(startup_message(&DbState::Current { extra: 0 }, &many).unwrap().ends_with("m4, …"));
        assert!(startup_message(&DbState::Empty, &[]).unwrap().contains("empty"));
        assert!(startup_message(&DbState::Foreign { relations: 3 }, &[]).unwrap().contains("django_migrations"));
        let mm = DbState::Mismatch { missing: vec![], unknown: vec![] };
        assert!(startup_message(&mm, &[]).unwrap().contains("manage migrate"));
    }

    #[test]
    fn classify_states() {
        assert_eq!(classify(&present_all()), DbState::Current { extra: 0 });
        let mut p = present_all();
        p.remove(&("documents".to_string(), "0027_documentbarcode".to_string()));
        assert!(matches!(classify(&p), DbState::Mismatch { ref missing, .. } if missing.len() == 1));
        let mut p = present_all();
        p.insert(("documents".into(), "0099_future".into()));
        assert!(matches!(classify(&p), DbState::Mismatch { ref unknown, .. } if unknown.len() == 1));
        let mut p = present_all();
        p.insert(("django_q".into(), "0001_initial".into()));
        assert_eq!(classify(&p), DbState::Current { extra: 1 });
        // paperless-ngx dev (newer than 3.4.2) with known migrations: accepted
        let mut p = present_all();
        for (a, n) in KNOWN_NEWER {
            p.insert((a.to_string(), n.to_string()));
        }
        assert_eq!(classify(&p), DbState::Current { extra: 2 });
        p.insert(("documents".into(), "0030_future".into()));
        let DbState::Mismatch { missing, unknown } = classify(&p) else { panic!("Mismatch erwartet") };
        assert!(mismatch_message(&missing, &unknown).contains("document_exporter"));
    }

    #[test]
    fn v2_database_points_to_assistant() {
        let s = |a: &str, n: &str| (a.to_string(), n.to_string());
        let missing = vec![s("documents", "0001_squashed"), s("documents", "0003_remove_document_storage_type")];
        assert!(catchup_plan(&missing, &[]).is_none());
        let m = mismatch_message(&missing, &[]);
        assert!(m.contains("paperless-ngx 2.x") && m.contains("paperrust migrate"), "{m}");
    }

    #[test]
    fn applying_lines_skip_replaced() {
        let l = applying_lines();
        assert_eq!(l.len(), 109);
        assert_eq!(l[0], "contenttypes.0001_initial");
        assert!(l.contains(&"documents.0001_squashed".to_string()));
    }

    #[test]
    fn catchup_from_313() {
        let s = |a: &str, n: &str| (a.to_string(), n.to_string());
        // paperless-ngx 3.1.3 (git ls-tree v3.1.3): these five are missing
        let missing = vec![
            s("documents", "0026_alter_document_archive_checksum_and_more"),
            s("documents", "0027_documentbarcode"),
            s("paperless", "0016_alter_applicationconfiguration_ai_enabled"),
            s("paperless", "0017_applicationconfiguration_llm_embedding_api_key"),
            s("paperless", "0018_applicationconfiguration_barcode_store_values"),
        ];
        let mut present = present_all();
        for m in &missing {
            present.remove(m);
        }
        let DbState::Mismatch { missing: got, unknown } = classify(&present) else { panic!("Mismatch erwartet") };
        let plan = catchup_plan(&got, &unknown).expect("nachholbar");
        assert_eq!(plan.len(), 5);
        assert!(plan[1].2.contains("CREATE TABLE \"documents_documentbarcode\""));
        assert!(plan[2].2.contains("UPDATE \"paperless_applicationconfiguration\""));
        // every catch-up migration belongs to the v3.4.2 state
        let all: BTreeSet<(String, String)> = present_all();
        for (a, n, _) in catchup_steps() {
            assert!(all.contains(&(a.clone(), n.clone())), "{a}.{n}");
        }
        // too old (2.x) or newer: cannot be caught up
        assert!(catchup_plan(&[s("documents", "0003_remove_document_storage_type")], &[]).is_none());
        assert!(catchup_plan(&missing, &[s("documents", "0099_future")]).is_none());
    }
}
