//! Database dump without `pg_dump`: all tables in schema `public` via `COPY … TO STDOUT`
//! (text format) in one read transaction (`REPEATABLE READ`, i.e. a consistent state), gzip-compressed.
//!
//! The schema itself is **not** included: it is created on restore with
//! `paperrust manage migrate` in the same version. [`restore`] then empties all backed-up
//! tables and restores the rows via `COPY … FROM STDIN` (one transaction; Django's
//! foreign keys are `DEFERRABLE INITIALLY DEFERRED`, own triggers are switched off briefly),
//! afterwards the sequences are reset.
//!
//! Format (before gzip):
//! ```text
//! PAPERRUST-DBDUMP 1\n
//! T {"table":"documents_tag","columns":["id","name",…]}\n
//! D <length>\n<COPY text>      (any number of times)
//! E\n                          (end of table)
//! Z\n                          (end)
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

const MAGIC: &str = "PAPERRUST-DBDUMP 1";

#[derive(Debug, Serialize, Deserialize)]
struct TableHeader {
    table: String,
    columns: Vec<String>,
}

/// Result of a dump.
#[derive(Debug, Default, Clone)]
pub struct DumpStats {
    pub tables: usize,
    /// uncompressed bytes
    pub bytes: u64,
}

fn qi(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Tables in schema `public` (alphabetical) with their writable columns.
async fn tables(conn: &mut sqlx::PgConnection) -> Result<Vec<TableHeader>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT c.table_name::text AS t, array_agg(c.column_name::text ORDER BY c.ordinal_position) AS cols
         FROM information_schema.columns c
         JOIN information_schema.tables t ON t.table_schema = c.table_schema AND t.table_name = c.table_name AND t.table_type = 'BASE TABLE'
         WHERE c.table_schema = 'public' AND c.is_generated = 'NEVER'
         GROUP BY c.table_name ORDER BY c.table_name",
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.iter().map(|r| Ok(TableHeader { table: r.try_get("t")?, columns: r.try_get("cols")? })).collect()
}

/// Write the dump to `out` (gzip).
pub async fn dump(pool: &PgPool, out: &Path) -> Result<DumpStats, String> {
    let file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut gz = flate2::write::GzEncoder::new(std::io::BufWriter::new(file), flate2::Compression::new(6));
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY").execute(&mut *tx).await.map_err(|e| e.to_string())?;
    let list = tables(&mut tx).await.map_err(|e| e.to_string())?;
    let w = |gz: &mut flate2::write::GzEncoder<_>, b: &[u8]| gz.write_all(b).map_err(|e: std::io::Error| e.to_string());
    w(&mut gz, format!("{MAGIC}\n").as_bytes())?;
    let mut stats = DumpStats::default();
    for t in &list {
        let header = serde_json::to_string(t).map_err(|e| e.to_string())?;
        w(&mut gz, format!("T {header}\n").as_bytes())?;
        let cols = t.columns.iter().map(|c| qi(c)).collect::<Vec<_>>().join(", ");
        let stmt = format!("COPY (SELECT {cols} FROM public.{}) TO STDOUT", qi(&t.table));
        {
            let mut s = tx.copy_out_raw(&stmt).await.map_err(|e| format!("{}: {e}", t.table))?;
            while let Some(chunk) = s.try_next().await.map_err(|e| format!("{}: {e}", t.table))? {
                w(&mut gz, format!("D {}\n", chunk.len()).as_bytes())?;
                w(&mut gz, &chunk)?;
                stats.bytes += chunk.len() as u64;
            }
        }
        w(&mut gz, b"E\n")?;
        stats.tables += 1;
    }
    w(&mut gz, b"Z\n")?;
    tx.rollback().await.map_err(|e| e.to_string())?;
    let mut inner = gz.finish().map_err(|e| e.to_string())?;
    inner.flush().map_err(|e| e.to_string())?;
    Ok(stats)
}

/// Result of the restore.
#[derive(Debug, Default, Clone)]
pub struct RestoreStats {
    pub tables: usize,
    /// tables in the dump that do not exist in the target database (skipped)
    pub skipped: Vec<String>,
    pub rows: u64,
}

fn read_line(r: &mut impl BufRead) -> Result<String, String> {
    let mut s = String::new();
    r.read_line(&mut s).map_err(|e| e.to_string())?;
    if s.is_empty() {
        return Err("database dump ends unexpectedly".into());
    }
    Ok(s.trim_end_matches('\n').to_string())
}

/// Tables of the dump that do not exist in the target database (in dump order).
pub fn missing_tables(in_dump: &[String], existing: &[String]) -> Vec<String> {
    in_dump.iter().filter(|t| !existing.contains(t)).cloned().collect()
}

/// Missing tables mean lost data (e.g. the target was migrated with an older version):
/// refuse unless the caller explicitly allows skipping them.
pub fn check_missing_tables(missing: &[String], allow_missing: bool) -> Result<(), String> {
    if missing.is_empty() || allow_missing {
        return Ok(());
    }
    Err(format!(
        "{} table(s) of the dump do not exist in the target database: {}. Run `paperrust manage migrate` with the same          version first, or pass --allow-missing-tables to skip them (their rows are not restored)",
        missing.len(),
        missing.join(", ")
    ))
}

/// Restore a dump into a **migrated** database. Existing rows of the backed-up
/// tables are replaced. Tables of the dump missing in the target fail the restore
/// (before anything is changed) unless `allow_missing` is set; then they are skipped
/// and listed in [`RestoreStats::skipped`].
pub async fn restore(pool: &PgPool, dump_file: &Path, allow_missing: bool) -> Result<RestoreStats, String> {
    let f = std::fs::File::open(dump_file).map_err(|e| format!("{}: {e}", dump_file.display()))?;
    let mut r = BufReader::new(flate2::read::GzDecoder::new(f));
    if read_line(&mut r)? != MAGIC {
        return Err("not a PaperRust database dump".into());
    }
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let existing: Vec<String> = sqlx::query_scalar("SELECT tablename::text FROM pg_tables WHERE schemaname = 'public' ORDER BY 1")
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    // 0. check before TRUNCATE (pre-scan of the dump, without holding it in memory)
    let p = dump_file.to_path_buf();
    let in_dump = tokio::task::spawn_blocking(move || tables_in_dump(&p)).await.map_err(|e| e.to_string())??;
    check_missing_tables(&missing_tables(&in_dump, &existing), allow_missing)?;
    // 1. own triggers off while no checks are pending yet
    //    (PostgreSQL forbids ALTER TABLE with deferred trigger events).
    for t in &existing {
        sqlx::query(&format!("ALTER TABLE public.{} DISABLE TRIGGER USER", qi(t)))
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("{t}: {e}"))?;
    }
    // 2. check foreign keys only at the end (Django: DEFERRABLE INITIALLY DEFERRED), empty everything
    sqlx::query("SET CONSTRAINTS ALL DEFERRED").execute(&mut *tx).await.map_err(|e| e.to_string())?;
    if !existing.is_empty() {
        let all = existing.iter().map(|t| format!("public.{}", qi(t))).collect::<Vec<_>>().join(", ");
        // CASCADE (tests/chaos C6, finding C6-2): after the data model conversion, tables in the
        // schema `ngx_legacy` (old guardian permissions) reference auth_user/auth_group/auth_permission. Without
        // CASCADE every restore failed with "cannot truncate a table referenced in a foreign
        // key constraint". These legacy rows belong to the replaced state and are emptied as well.
        sqlx::query(&format!("TRUNCATE {all} RESTART IDENTITY CASCADE")).execute(&mut *tx).await.map_err(|e| format!("TRUNCATE: {e}"))?;
    }
    // 3. restore tables
    let mut stats = RestoreStats::default();
    loop {
        let line = read_line(&mut r)?;
        if line == "Z" {
            break;
        }
        let Some(h) = line.strip_prefix("T ") else {
            return Err(format!("damaged database dump (unexpected line {:?})", line.chars().take(40).collect::<String>()));
        };
        let h: TableHeader = serde_json::from_str(h).map_err(|e| e.to_string())?;
        let present = existing.contains(&h.table);
        if !present {
            stats.skipped.push(h.table.clone());
        }
        let cols = h.columns.iter().map(|c| qi(c)).collect::<Vec<_>>().join(", ");
        let mut copy = if present {
            Some(
                tx.copy_in_raw(&format!("COPY public.{} ({cols}) FROM STDIN", qi(&h.table)))
                    .await
                    .map_err(|e| format!("{}: {e}", h.table))?,
            )
        } else {
            None
        };
        loop {
            let l = read_line(&mut r)?;
            if l == "E" {
                break;
            }
            let Some(n) = l.strip_prefix("D ").and_then(|n| n.parse::<usize>().ok()) else {
                return Err("damaged database dump (bad chunk header)".into());
            };
            let mut buf = vec![0u8; n];
            r.read_exact(&mut buf).map_err(|e| e.to_string())?;
            if let Some(c) = copy.as_mut() {
                c.send(buf).await.map_err(|e| format!("{}: {e}", h.table))?;
            }
        }
        if let Some(c) = copy {
            stats.rows += c.finish().await.map_err(|e| format!("{}: {e}", h.table))?;
            stats.tables += 1;
        }
    }
    // 4. check foreign keys now (afterwards no events are pending), triggers back on
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE").execute(&mut *tx).await.map_err(|e| format!("foreign keys: {e}"))?;
    for t in &existing {
        sqlx::query(&format!("ALTER TABLE public.{} ENABLE TRIGGER USER", qi(t)))
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("{t}: {e}"))?;
    }
    reset_sequences(&mut tx).await.map_err(|e| format!("sequences: {e}"))?;
    tx.commit().await.map_err(|e| format!("commit (foreign keys checked here): {e}"))?;
    Ok(stats)
}

/// Set sequences (serial/identity) to max(id)+1.
async fn reset_sequences(conn: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT s.oid::regclass::text AS seq, t.relname::text AS tbl, a.attname::text AS col
         FROM pg_class s
         JOIN pg_depend d ON d.objid = s.oid AND d.classid = 'pg_class'::regclass AND d.refclassid = 'pg_class'::regclass
         JOIN pg_class t ON t.oid = d.refobjid
         JOIN pg_namespace n ON n.oid = t.relnamespace AND n.nspname = 'public'
         JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = d.refobjsubid
         WHERE s.relkind = 'S' AND d.deptype IN ('a', 'i')",
    )
    .fetch_all(&mut *conn)
    .await?;
    for r in rows {
        let seq: String = r.try_get("seq")?;
        let tbl: String = r.try_get("tbl")?;
        let col: String = r.try_get("col")?;
        let q = format!(
            "SELECT setval('{}', COALESCE((SELECT max({}) FROM public.{}), 0) + 1, false)",
            seq.replace('\'', "''"),
            qi(&col),
            qi(&tbl)
        );
        sqlx::query(&q).execute(&mut *conn).await?;
    }
    Ok(())
}

/// Table list of a dump (for `backup list --verbose` and tests), without a database.
pub fn tables_in_dump(dump_file: &Path) -> Result<Vec<String>, String> {
    let f = std::fs::File::open(dump_file).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(flate2::read::GzDecoder::new(f));
    if read_line(&mut r)? != MAGIC {
        return Err("not a PaperRust database dump".into());
    }
    let mut out = Vec::new();
    loop {
        let line = read_line(&mut r)?;
        if line == "Z" {
            return Ok(out);
        }
        if let Some(h) = line.strip_prefix("T ") {
            let h: TableHeader = serde_json::from_str(h).map_err(|e| e.to_string())?;
            out.push(h.table);
            continue;
        }
        if let Some(n) = line.strip_prefix("D ").and_then(|n| n.parse::<u64>().ok()) {
            std::io::copy(&mut (&mut r).take(n), &mut std::io::sink()).map_err(|e| e.to_string())?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(super::qi("a\"b"), "\"a\"\"b\"");
    }

    /// Regression: tables missing in the target schema were skipped silently (only a count
    /// was printed); now the restore fails with their names unless explicitly allowed.
    #[test]
    fn missing_tables_fail_unless_allowed() {
        let dump: Vec<String> = ["documents_tag", "paperrust_new", "auth_user"].iter().map(|s| s.to_string()).collect();
        let existing: Vec<String> = ["auth_user", "documents_tag"].iter().map(|s| s.to_string()).collect();
        let missing = missing_tables(&dump, &existing);
        assert_eq!(missing, vec!["paperrust_new".to_string()]);
        let e = check_missing_tables(&missing, false).unwrap_err();
        assert!(e.contains("paperrust_new") && e.contains("--allow-missing-tables"), "{e}");
        assert!(check_missing_tables(&missing, true).is_ok());
        assert!(check_missing_tables(&[], false).is_ok());
    }

    /// The pre-scan reads table names from a dump file without a database.
    #[test]
    fn tables_in_dump_file() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("d.gz");
        let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(&p).unwrap(), flate2::Compression::fast());
        gz.write_all(
            b"PAPERRUST-DBDUMP 1
T {\"table\":\"a\",\"columns\":[\"id\"]}
D 2
1
E
T {\"table\":\"b\",\"columns\":[\"id\"]}
E
Z
",
        )
        .unwrap();
        gz.finish().unwrap();
        assert_eq!(tables_in_dump(&p).unwrap(), vec!["a".to_string(), "b".to_string()]);
    }
}
