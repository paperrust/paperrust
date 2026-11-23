//! PaperRust extensions in export and import (tests/chaos C6, docs/contract/pl-cli.md "Extras").
//!
//! The upstream format (`manifest.json`, Django's serializer) only knows the models of paperless-ngx.
//! PaperRust's own data – "else" branches and AI conditions of the workflows, workflow history,
//! system settings, roles, ASN ranges, life areas, folders, term aliases, detected deadlines,
//! AI history, question history, scanners, processed mails – was so far **lost** when backing up
//! with `document_exporter` (finding C6-1). It is now stored in a separate file [`FILE_NAME`] next to
//! `manifest.json`: paperless-ngx ignores it on the way back, PaperRust's `document_importer`
//! loads it after the upstream models in the same transaction.
//!
//! Format: `{"version": 1, "tables": {"<table>": [{column: value, …}, …], …}}` (rows as
//! `row_to_json`). On import the rows of the contained tables are replaced (default rows
//! of the fresh installation, e.g. built-in roles, give way to the backed-up ones); only columns that exist
//! in source **and** target are written, then the sequences are adjusted.
//!
//! Not included (with reason): encrypted settings (`paperrust_setting.secret_ct`, the
//! key is tied to the target system – re-enter secrets after restoring),
//! device/MCP tokens (`paperrust_device_token`, `paperrust_ai_mcp_scope`: reissue access),
//! derived or operational states (counters, grant index, sync, queue, backup,
//! protection and check states – are rebuilt).

use serde_json::{Map, Value};
use sqlx::{PgConnection, Row};

use crate::{CliError, CliResult};

/// File name next to `manifest.json`.
pub const FILE_NAME: &str = "paperrust-extras.json";
/// Format version.
pub const VERSION: i64 = 1;

/// Tables in insertion order (foreign keys first), with optional row selection.
pub const TABLES: &[(&str, Option<&str>)] = &[
    ("paperrust_role_config", None),
    ("paperrust_role", None),
    ("paperrust_role_assignment", None),
    ("paperrust_setting", Some("secret_ct IS NULL")),
    ("paperrust_asn_config", None),
    ("paperrust_asn_range", None),
    ("paperrust_asn_print", None),
    ("paperrust_life_area", None),
    // folder view (pl-api-v2 folders): tree, document → folder, shares – parents first
    ("paperrust_folder", None),
    ("paperrust_document_folder", None),
    ("paperrust_folder_grant", None),
    // original checksum of shrunk documents (pl-consumer source_checksum, duplicate detection)
    ("paperrust_source_checksum", None),
    ("paperrust_taxonomy_alias", None),
    ("paperrust_workflow_ext", None),
    ("paperrust_workflow_else_action", None),
    ("paperrust_workflow_run", None),
    ("paperrust_doc_date", None),
    ("paperrust_ai_prompt", None),
    ("paperrust_ai_option", None),
    ("paperrust_ai_audit", None),
    ("paperrust_ai_review", None),
    ("paperrust_ask_conversation", None),
    ("paperrust_ask_message", None),
    ("paperrust_scanner", None),
    ("paperrust_scan_device", None),
    // upstream table that the upstream exporter does not back up: without it the mail fetch after a
    // restore fetches already processed mails again (duplicates caught only via the checksum).
    ("paperless_mail_processedmail", None),
];

fn ident(t: &str) -> String {
    format!("\"{}\"", t.replace('"', "\"\""))
}

async fn table_exists(conn: &mut PgConnection, t: &str) -> CliResult<bool> {
    let ok: bool = sqlx::query_scalar("SELECT to_regclass('public.' || quote_ident($1)) IS NOT NULL").bind(t).fetch_one(&mut *conn).await?;
    Ok(ok)
}

async fn columns(conn: &mut PgConnection, t: &str) -> CliResult<Vec<String>> {
    let rows = sqlx::query(
        "SELECT column_name::text FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1 \
         AND is_generated = 'NEVER' ORDER BY ordinal_position",
    )
    .bind(t)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter().map(|r| r.try_get::<String, _>(0).map_err(CliError::from)).collect()
}

/// All existing extension tables as a JSON document.
pub async fn dump(conn: &mut PgConnection) -> CliResult<Value> {
    let mut tables = Map::new();
    for (t, filter) in TABLES {
        if !table_exists(conn, t).await? {
            continue;
        }
        let w = filter.map(|f| format!(" WHERE {f}")).unwrap_or_default();
        let rows: Value = sqlx::query_scalar(&format!(
            "SELECT coalesce(json_agg(row_to_json(x) ORDER BY row_to_json(x)::text), '[]'::json) FROM {} x{w}",
            ident(t)
        ))
        .fetch_one(&mut *conn)
        .await?;
        tables.insert((*t).to_string(), rows);
    }
    let mut m = Map::new();
    m.insert("version".into(), Value::from(VERSION));
    m.insert("tables".into(), Value::Object(tables));
    Ok(Value::Object(m))
}

/// Loads the tables from [`dump`] (in the import's transaction). Returns the row count.
pub async fn load(conn: &mut PgConnection, doc: &Value) -> CliResult<usize> {
    let v = doc.get("version").and_then(Value::as_i64).unwrap_or(0);
    if v != VERSION {
        return Err(CliError::Command(format!("{FILE_NAME}: unsupported version {v} (expected {VERSION})")));
    }
    let Some(tables) = doc.get("tables").and_then(Value::as_object) else {
        return Err(CliError::Command(format!("{FILE_NAME}: missing \"tables\"")));
    };
    // first empty all affected tables (reverse order), then fill
    let present: Vec<&str> = TABLES.iter().map(|(t, _)| *t).filter(|t| tables.contains_key(*t)).collect();
    let mut usable = Vec::new();
    for t in &present {
        if table_exists(conn, t).await? {
            usable.push(*t);
        } else {
            println!("Skipping {t} from {FILE_NAME}: table does not exist in this version");
        }
    }
    for t in usable.iter().rev() {
        sqlx::query(&format!("DELETE FROM {}", ident(t))).execute(&mut *conn).await?;
    }
    let mut total = 0usize;
    for t in &usable {
        let rows = tables.get(*t).cloned().unwrap_or(Value::Array(Vec::new()));
        let n = rows.as_array().map(Vec::len).unwrap_or(0);
        if n == 0 {
            continue;
        }
        let have = columns(conn, t).await?;
        let mut keys: Vec<String> = Vec::new();
        if let Some(first) = rows.as_array().and_then(|a| a.first()).and_then(Value::as_object) {
            for k in first.keys() {
                if have.iter().any(|c| c == k) {
                    keys.push(k.clone());
                }
            }
        }
        if keys.is_empty() {
            continue;
        }
        let cols = keys.iter().map(|k| ident(k)).collect::<Vec<_>>().join(", ");
        // identity columns with GENERATED ALWAYS need OVERRIDING SYSTEM VALUE
        let always: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1 \
             AND identity_generation = 'ALWAYS')",
        )
        .bind(*t)
        .fetch_one(&mut *conn)
        .await?;
        let overriding = if always { " OVERRIDING SYSTEM VALUE" } else { "" };
        sqlx::query(&format!(
            "INSERT INTO {tbl} ({cols}){overriding} SELECT {cols} FROM json_populate_recordset(NULL::{tbl}, $1::json)",
            tbl = ident(t)
        ))
        .bind(&rows)
        .execute(&mut *conn)
        .await
        .map_err(|e| CliError::Command(format!("{FILE_NAME}: could not restore {t}: {e}")))?;
        // adjust the sequence of the id column (if present)
        if keys.iter().any(|k| k == "id") {
            sqlx::query(&format!(
                "SELECT setval(s, coalesce((SELECT max(id) FROM {tbl}), 1), (SELECT max(id) FROM {tbl}) IS NOT NULL) \
                 FROM pg_get_serial_sequence($1, 'id') s WHERE s IS NOT NULL",
                tbl = ident(t)
            ))
            .bind(format!("public.{t}"))
            .execute(&mut *conn)
            .await?;
        }
        total += n;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_order_has_parents_first() {
        let pos = |t: &str| TABLES.iter().position(|(x, _)| *x == t).unwrap();
        assert!(pos("paperrust_role") < pos("paperrust_role_assignment"));
        assert!(pos("paperrust_ai_audit") < pos("paperrust_ai_review"));
        assert!(pos("paperrust_ask_conversation") < pos("paperrust_ask_message"));
        assert!(pos("paperrust_folder") < pos("paperrust_document_folder"));
        assert!(pos("paperrust_folder") < pos("paperrust_folder_grant"));
        // secrets and access credentials are not included
        assert!(!TABLES.iter().any(|(t, _)| *t == "paperrust_device_token" || *t == "paperrust_ai_mcp_scope"));
        assert_eq!(TABLES.iter().find(|(t, _)| *t == "paperrust_setting").unwrap().1, Some("secret_ct IS NULL"));
    }

    #[test]
    fn quoting() {
        assert_eq!(ident("a\"b"), "\"a\"\"b\"");
    }
}
