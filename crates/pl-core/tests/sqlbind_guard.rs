//! Guard for FINDINGS B3/N4 (`tests/stress/BEFUNDE.md`): statements built dynamically with sea-query
//! must be bound via [`pl_core::sqlbind::SqlxBinder`]. Its `build_sqlx`
//! appends the type signature of the values to the SQL text; otherwise identical texts with
//! different binding types (`i32`/`i64`) collide in a connection's statement cache
//! (`incorrect binary data format in bind parameter` / `insufficient data left in message`).
//!
//! This test forbids, in the whole project (`crates/*/src`, `crates/*/tests`), the direct
//! use of `sea_query_binder::SqlxBinder` and `build_any_sqlx` (except in `pl-core/src/sqlbind.rs`).

use std::path::{Path, PathBuf};

/// Only allowed places: the building block itself, this guard, the evidence test.
const ALLOWED: &[&str] = &["pl-core/src/sqlbind.rs", "pl-core/tests/sqlbind_guard.rs", "pl-core/tests/sqlbind_db.rs"];

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

#[test]
fn no_raw_sea_query_binder() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for c in std::fs::read_dir(&crates).expect("crates/").flatten() {
        rust_files(&c.path().join("src"), &mut files);
        rust_files(&c.path().join("tests"), &mut files);
    }
    assert!(files.len() > 100, "zu wenige Dateien gefunden: {}", files.len());
    let mut bad = Vec::new();
    for f in &files {
        let norm = f.to_string_lossy().replace('\\', "/");
        if ALLOWED.iter().any(|a| norm.ends_with(a)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let flat: String = text.split_whitespace().collect();
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if t.contains("sea_query_binder::SqlxBinder") || t.contains("build_any_sqlx") {
                bad.push(format!("{norm}:{}: {}", i + 1, t));
            }
        }
        // `use sea_query_binder::{…, SqlxBinder, …}` across several lines
        let mut rest = flat.as_str();
        while let Some(p) = rest.find("sea_query_binder::{") {
            let tail = &rest[p..];
            let end = tail.find('}').unwrap_or(tail.len());
            if tail[..end].contains("SqlxBinder") {
                bad.push(format!("{norm}: use sea_query_binder::{{…SqlxBinder…}}"));
            }
            rest = &tail[end.min(tail.len())..];
            if rest.len() == tail.len() {
                break;
            }
        }
    }
    assert!(
        bad.is_empty(),
        "sea_query_binder::SqlxBinder direkt verwendet – stattdessen `use pl_core::sqlbind::SqlxBinder` \
         (Typ-Signatur gegen Statement-Cache-Kollisionen, BEFUNDE N4):\n{}",
        bad.join("\n")
    );
}
