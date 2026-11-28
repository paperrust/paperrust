//! Guard against FINDINGS B3 (tests/stress/BEFUNDE.md): sqlx remembers prepared statements
//! per connection **by SQL text** – even for `persistent(false)` memory is checked first
//! (sqlx-postgres `get_or_prepare`). If a second path binds the same text with different
//! Rust types (`i64` instead of `i32` …), it fails on a connection that the first has already used:
//! `incorrect binary data format in bind parameter` or `insufficient data left in message`.
//!
//! This test searches the production code of all crates (`crates/*/src`) for literally identical SQL texts
//! with parameters in several places. Every such text must be entered here as reviewed
//! (all places bind the same Rust type per parameter). New hit → check and align the binding types of all
//! places, then add to [`REVIEWED`]; even better: use a shared
//! building block (e.g. `pl_consumer::db::insert_document_tags`).
//!
//! Statements assembled with sea-query (text only at runtime) are protected by
//! `pl_core::sqlbind::SqlxBinder` (type signature in the SQL text, FINDINGS N4; guard
//! `pl-core/tests/sqlbind_guard.rs`). Not covered: texts built with `format!` with `.bind(…)` –
//! there the rule applies: bind `integer` columns → `i32`, `bigint` → `i64`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Reviewed multiple texts (2026-11-18, FINDINGS B3): all places bind the same types.
const REVIEWED: &[&str] = &[
    "DELETE FROM django_session WHERE session_key = $1",
    "DELETE FROM documents_document_tags WHERE document_id = $1 AND tag_id = ANY($2)",
    "DELETE FROM documents_documentbarcode WHERE document_id = $1",
    "DELETE FROM documents_savedviewfilterrule WHERE saved_view_id = $1::int8",
    "DELETE FROM documents_sharelinkbundle WHERE id = $1::int8",
    "DELETE FROM documents_sharelinkbundle_documents WHERE sharelinkbundle_id = $1::int8",
    "DELETE FROM paperrust_migrations WHERE name = $1",
    "INSERT INTO auth_user_groups (user_id, group_id) SELECT $1, g.id FROM auth_group g WHERE g.name = ANY($2) ON CONFLICT DO NOTHING",
    "INSERT INTO django_migrations (app, name, applied) VALUES ($1, $2, clock_timestamp())",
    "INSERT INTO django_migrations (app, name, applied) VALUES ($1, $2, now())",
    "INSERT INTO django_session (session_key, session_data, expire_date) VALUES ($1, $2, $3) ON CONFLICT (session_key) DO UPDATE SET session_data = EXCLUDED.session_data, expire_date = EXCLUDED.expire_date",
    "INSERT INTO django_session (session_key, session_data, expire_date) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    "INSERT INTO paperrust_migrations (name) VALUES ($1) ON CONFLICT DO NOTHING",
    "SELECT COALESCE((SELECT verified FROM account_emailaddress WHERE user_id = $1 AND email = $2 LIMIT 1), false)",
    "SELECT EXISTS(SELECT 1 FROM account_emailaddress WHERE id <> $1 AND verified AND email = $2)",
    "SELECT EXISTS(SELECT 1 FROM account_emailaddress WHERE user_id = $1 AND \"primary\")",
    "SELECT count(*) FROM auth_user WHERE UPPER(username::text) = UPPER($1)",
    "SELECT email FROM account_emailaddress WHERE user_id = $1",
    "SELECT id FROM django_content_type WHERE app_label = $1 AND model = $2",
    "SELECT id FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY created DESC, id",
    "SELECT id FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY created DESC, id ASC",
    "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL ORDER BY created DESC, id",
    "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL ORDER BY created DESC, id ASC",
    "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL ORDER BY version_index DESC NULLS LAST, id DESC LIMIT 1",
    "SELECT id FROM documents_document WHERE root_document_id = ANY($1)",
    "SELECT id FROM documents_document WHERE storage_path_id = $1::integer AND deleted_at IS NULL ORDER BY id",
    "SELECT id, title FROM documents_document WHERE id = ANY($1)",
    "SELECT id::int8, email FROM auth_user WHERE email = $1",
    "SELECT owner_id FROM documents_document WHERE id = $1",
    "SELECT paperrust_sync_ensure($1, $2::int8[])",
    // pl-consumer::db and pl-store::legacy (2026-11-27): text key (&str / String)
    "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
    "SELECT pg_notify($1, $2)",
    "SELECT saved_view_id::int8 AS sv, rule_type::int8 AS rule_type, value FROM documents_savedviewfilterrule WHERE saved_view_id = ANY($1::int8[]) ORDER BY id",
    "SELECT session_data FROM django_session WHERE session_key = $1 AND expire_date > now()",
    "SELECT settings FROM documents_uisettings WHERE user_id = $1::int8",
    "SELECT user_id::int8, email, verified FROM account_emailaddress WHERE email = $1 ORDER BY id",
    // pl-accounts and pl-documents-write::workflows: both i64 (previously int4/int8 mixed, B3)
    "SELECT username FROM auth_user WHERE id = $1",
    "UPDATE auth_user SET email = $1 WHERE id = $2",
    "UPDATE auth_user SET password = $1 WHERE id = $2",
    "UPDATE documents_document SET filename = $2, archive_filename = $3, modified = $4 WHERE id = $1",
    "UPDATE documents_document SET modified = $2 WHERE id = $1 AND deleted_at IS NULL",
    "UPDATE documents_document SET modified = $2, filename = CASE WHEN $3 THEN $4 ELSE filename END, archive_filename = CASE WHEN $5 THEN $6 ELSE archive_filename END WHERE id = $1 AND deleted_at IS NULL",
    // pl-ai::server::vision and pl-consumer::system_note (2026-11-27): text, i32
    "UPDATE documents_note SET note = $1 WHERE id = $2",
    "UPDATE documents_sharelinkbundle SET status = 'pending', last_error = NULL, size_bytes = NULL, built_at = NULL, file_path = '' WHERE id = $1::int8",
    "UPDATE mfa_authenticator SET data = $1 WHERE id = $2",
    "{RANGE_SELECT} WHERE p.id = $1",
];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Content of a Rust string literal from `start` (directly after the opening `"`; `raw` = `r"`/`r#"`).
fn literal(src: &str, start: usize, raw: Option<&str>) -> Option<String> {
    let rest = src.get(start..)?;
    if let Some(r) = raw {
        let end = if r == "r#\"" { rest.find("\"#")? } else { rest.find('"')? };
        return Some(rest[..end].to_string());
    }
    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                '\n' | '\r' => {
                    while chars.peek().is_some_and(|c| c.is_whitespace()) {
                        chars.next();
                    }
                }
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

/// All literal SQL texts with `$1` from `sqlx::query*(…)` calls: text → locations.
fn sql_literals() -> BTreeMap<String, Vec<String>> {
    let call = regex::Regex::new(r#"sqlx::query(?:_as|_scalar)?(?:::<[^>]*>)?\(\s*(?:&format!\(\s*)?(r#"|r"|")"#).expect("regex");
    let mut files = Vec::new();
    let Ok(rd) = std::fs::read_dir(crates_dir()) else { return BTreeMap::new() };
    for e in rd.flatten() {
        rust_files(&e.path().join("src"), &mut files);
    }
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in files {
        let Ok(src) = std::fs::read_to_string(&f) else { continue };
        let src = src.replace("\r\n", "\n");
        for m in call.captures_iter(&src) {
            let (Some(whole), Some(q)) = (m.get(0), m.get(1)) else { continue };
            let raw = q.as_str().starts_with('r').then_some(q.as_str());
            let Some(lit) = literal(&src, whole.end(), raw) else { continue };
            let norm = lit.split_whitespace().collect::<Vec<_>>().join(" ");
            if !norm.contains("$1") {
                continue;
            }
            let line = src[..whole.start()].matches('\n').count() + 1;
            let rel = f.strip_prefix(crates_dir()).unwrap_or(&f).display().to_string().replace('\\', "/");
            out.entry(norm).or_default().push(format!("{rel}:{line}"));
        }
    }
    out
}

#[test]
fn identical_sql_texts_are_reviewed() {
    let all = sql_literals();
    assert!(all.len() > 200, "Scanner findet zu wenig SQL ({}), Pfad falsch?", all.len());
    let unreviewed: Vec<String> = all
        .iter()
        .filter(|(sql, sites)| sites.len() > 1 && !REVIEWED.contains(&sql.as_str()))
        .map(|(sql, sites)| format!("{sql}\n    {}", sites.join("\n    ")))
        .collect();
    assert!(
        unreviewed.is_empty(),
        "Gleicher SQL-Text an mehreren Stellen (Statement-Cache, BEFUNDE B3). Bindungstypen aller Stellen \
         angleichen und in REVIEWED eintragen:\n{}",
        unreviewed.join("\n")
    );
}

/// The earlier failure case itself: the tag-assignment INSERT now exists in only one place.
#[test]
fn tag_insert_has_single_builder() {
    let mut files = Vec::new();
    for c in ["pl-consumer", "pl-documents-write"] {
        rust_files(&crates_dir().join(c).join("src"), &mut files);
    }
    let sites: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let src = std::fs::read_to_string(f).ok()?;
            src.contains("into_table(a(\"documents_document_tags\"))").then(|| f.display().to_string())
        })
        .collect();
    assert_eq!(sites.len(), 1, "documents_document_tags-INSERT an mehreren Stellen: {sites:?}");
    assert!(sites[0].replace('\\', "/").ends_with("pl-consumer/src/db.rs"), "{sites:?}");
}
