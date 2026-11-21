// upstream: src/documents/search/_schema.py@v3.4.2 (schema_fingerprint, needs_rebuild,
//           _settings_mismatch, wipe_index, _write_sentinels, open_or_rebuild_index,
//           rebuild_in_progress)
//
// Writer-side lifecycle of the index directory. The schema itself and the
// SCHEMA_VERSION / marker names come from pl-search (single source).

use std::path::Path;

use pl_search::schema::{FIELD_DESCRIPTORS, Kind, REBUILD_MARKER, SCHEMA_VERSION, SETTINGS_FILE};
use tantivy::Index;
use tantivy::directory::MmapDirectory;

use crate::blake2b::blake2b_512_hex;
use crate::error::Result;
use crate::lock::LOCK_FILE;

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "text",
        Kind::Json => "json",
        Kind::U64 => "u64",
        Kind::Date => "date",
    }
}

/// `json.dumps([list(d) for d in field_descriptors()])` (default separators).
pub fn descriptors_json() -> String {
    let b = |v: bool| if v { "true" } else { "false" };
    let items: Vec<String> = FIELD_DESCRIPTORS
        .iter()
        .map(|d| {
            let tok = match d.tokenizer {
                Some(t) => format!("\"{t}\""),
                None => "null".to_string(),
            };
            format!("[\"{}\", \"{}\", {}, {}, {}, {}]", d.name, kind_name(d.kind), b(d.stored), b(d.indexed), b(d.fast), tok)
        })
        .collect();
    format!("[{}]", items.join(", "))
}

/// `schema_fingerprint()`: blake2b hexdigest of the descriptor JSON.
pub fn schema_fingerprint() -> String {
    blake2b_512_hex(descriptors_json().as_bytes())
}

/// Python `json.dumps(str)` for an ASCII-safe language code (`ensure_ascii`).
fn py_json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Content of `.index_settings.json` exactly as `_write_sentinels` writes it.
pub fn sentinels_json(language: Option<&str>) -> String {
    let lang = match language {
        Some(l) => py_json_str(l),
        None => "null".to_string(),
    };
    format!("{{\"schema_version\": {SCHEMA_VERSION}, \"language\": {lang}, \"schema_fingerprint\": \"{}\"}}", schema_fingerprint())
}

/// `_write_sentinels(index_dir)`.
pub fn write_sentinels(index_dir: &Path, language: Option<&str>) -> Result<()> {
    std::fs::write(index_dir.join(SETTINGS_FILE), sentinels_json(language))?;
    Ok(())
}

/// `_settings_mismatch(index_dir)`: missing/unparsable file or any value differs
/// (schema_version, language, schema_fingerprint).
pub fn settings_mismatch(index_dir: &Path, language: Option<&str>) -> bool {
    let Ok(raw) = std::fs::read_to_string(index_dir.join(SETTINGS_FILE)) else {
        return true;
    };
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return true;
    };
    // Python `data.get(...)` on a non-dict raises AttributeError, not ValueError;
    // a JSON non-object is treated as mismatch here (would crash upstream).
    let Some(obj) = data.as_object() else {
        return true;
    };
    // `!= SCHEMA_VERSION`: Python compares int 4 == 4.0 as equal.
    let version_ok = obj.get("schema_version").and_then(|v| v.as_f64()).is_some_and(|v| v == SCHEMA_VERSION as f64)
        && !matches!(obj.get("schema_version"), Some(serde_json::Value::Bool(_)));
    if !version_ok {
        tracing::info!("Search index schema version mismatch - rebuilding.");
        return true;
    }
    let lang_ok = match obj.get("language") {
        None => false,
        Some(serde_json::Value::Null) => language.is_none(),
        Some(serde_json::Value::String(s)) => Some(s.as_str()) == language,
        Some(_) => false,
    };
    if !lang_ok {
        tracing::info!("Search index language changed - rebuilding.");
        return true;
    }
    let fp_ok = obj.get("schema_fingerprint").and_then(|v| v.as_str()).is_some_and(|fp| fp == schema_fingerprint());
    if !fp_ok {
        tracing::info!("Search index schema fingerprint mismatch - rebuilding.");
        return true;
    }
    false
}

/// `needs_rebuild(index_dir)`: rebuild marker present or settings mismatch.
pub fn needs_rebuild(index_dir: &Path, language: Option<&str>) -> bool {
    if index_dir.join(REBUILD_MARKER).exists() {
        tracing::warn!("Previous search index rebuild did not finish - rebuilding.");
        return true;
    }
    settings_mismatch(index_dir, language)
}

/// `wipe_index(index_dir)`: delete every child of the directory.
///
/// Difference: the `.tantivy.lock` file (the cross-process writer lock, which the
/// caller may be holding) is kept; deleting it while held would break the mutual
/// exclusion with the Python writer. See contract "Known differences".
pub fn wipe_index(index_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(index_dir)? {
        let entry = entry?;
        if entry.file_name() == LOCK_FILE {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// `tantivy.Index(build_schema(), path=...)` (tantivy-py `reuse=True`):
/// `Index::open_or_create(MmapDirectory::open(path), schema)` + tokenizers.
pub fn open_or_create(index_dir: &Path, language: Option<&str>) -> Result<Index> {
    let dir = MmapDirectory::open(index_dir).map_err(tantivy::TantivyError::from)?;
    let index = Index::open_or_create(dir, pl_search::schema::build_schema())?;
    pl_search::tokenizer::register_tokenizers(&index, language);
    Ok(index)
}

/// What `open_or_rebuild_index` did (for logs/tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenOutcome {
    /// Directory missing: upstream uses an in-RAM index (nothing written).
    Missing,
    Opened,
    /// Settings mismatch: wiped, recreated empty, sentinels written.
    RecreatedSettings,
    /// `Index.open` failed (corrupt): wiped, recreated empty, sentinels written.
    RecreatedCorrupt,
}

/// Side effects of `open_or_rebuild_index(index_dir)` (run by every process that
/// opens the backend, also before each write batch via `_ensure_open`).
pub fn open_or_rebuild_index(index_dir: &Path, language: Option<&str>) -> Result<OpenOutcome> {
    if !index_dir.exists() {
        return Ok(OpenOutcome::Missing);
    }
    if settings_mismatch(index_dir, language) {
        wipe_index(index_dir)?;
        open_or_create(index_dir, language)?;
        write_sentinels(index_dir, language)?;
        // Known difference (Jetson check B1, 2026-11-19): the freshly emptied index is incomplete.
        // Upstream relies on the Docker start calling `document_index reindex --if-needed`
        // before any process opens the index; with PaperRust the server opens first, and the
        // worker's rebuild would afterwards have reported "Search index is up to date." (search empty after
        // switching from 2.x). The marker keeps `needs_rebuild` true until a rebuild has finished.
        touch_rebuild_marker(index_dir)?;
        return Ok(OpenOutcome::RecreatedSettings);
    }
    // tantivy-py Index.open = Index::open_in_dir (no schema check).
    match Index::open_in_dir(index_dir) {
        Ok(_) => Ok(OpenOutcome::Opened),
        Err(e) => {
            tracing::error!("Search index is corrupted or incomplete - rebuilding from scratch: {e}");
            wipe_index(index_dir)?;
            open_or_create(index_dir, language)?;
            write_sentinels(index_dir, language)?;
            touch_rebuild_marker(index_dir)?; // as above (B1)
            Ok(OpenOutcome::RecreatedCorrupt)
        }
    }
}

/// `rebuild_in_progress`: touch the marker; the caller removes it only on success.
pub fn touch_rebuild_marker(index_dir: &Path) -> Result<()> {
    std::fs::OpenOptions::new().create(true).append(true).open(index_dir.join(REBUILD_MARKER))?;
    Ok(())
}

pub fn clear_rebuild_marker(index_dir: &Path) -> Result<()> {
    match std::fs::remove_file(index_dir.join(REBUILD_MARKER)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fingerprint stamped by upstream into reference/data/data/index/.index_settings.json
    /// (read-only copy of the value; the reference index is never opened for writing).
    const REFERENCE_FINGERPRINT: &str =
        "cac16f9170382fd53b4ac9d854878783811c87c772ee23363692a2e3bf639b0f2959b4e978fae617830b43df24ca07dfee8d2f9f77eb7943e20c90b3a764a469";

    #[test]
    fn fingerprint_matches_upstream() {
        assert!(
            descriptors_json()
                .starts_with("[[\"id\", \"u64\", true, true, true, null], [\"title\", \"text\", true, true, false, \"paperless_text\"]")
        );
        assert_eq!(schema_fingerprint(), REFERENCE_FINGERPRINT);
    }

    #[test]
    fn sentinels_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(settings_mismatch(dir.path(), Some("en")));
        write_sentinels(dir.path(), Some("en")).unwrap();
        let raw = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert_eq!(raw, format!("{{\"schema_version\": 4, \"language\": \"en\", \"schema_fingerprint\": \"{REFERENCE_FINGERPRINT}\"}}"));
        assert!(!settings_mismatch(dir.path(), Some("en")));
        assert!(settings_mismatch(dir.path(), Some("de")));
        assert!(settings_mismatch(dir.path(), None));
        write_sentinels(dir.path(), None).unwrap();
        assert!(!settings_mismatch(dir.path(), None));
        assert!(!needs_rebuild(dir.path(), None));
        touch_rebuild_marker(dir.path()).unwrap();
        assert!(needs_rebuild(dir.path(), None));
        clear_rebuild_marker(dir.path()).unwrap();
        assert!(!needs_rebuild(dir.path(), None));
    }

    #[test]
    fn open_or_rebuild_recreates_on_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("junk.txt"), "x").unwrap();
        assert_eq!(open_or_rebuild_index(dir.path(), Some("en")).unwrap(), OpenOutcome::RecreatedSettings);
        assert!(!dir.path().join("junk.txt").exists());
        assert!(dir.path().join("meta.json").exists());
        assert_eq!(open_or_rebuild_index(dir.path(), Some("en")).unwrap(), OpenOutcome::Opened);
        let missing = dir.path().join("nope");
        assert_eq!(open_or_rebuild_index(&missing, Some("en")).unwrap(), OpenOutcome::Missing);
    }

    /// Jetson check B1: switch from ngx 2.x (Whoosh files, no settings file). If the
    /// server opens the index before the rebuild, `needs_rebuild` must still be true afterwards.
    #[test]
    fn recreated_index_still_needs_rebuild_until_a_rebuild_finishes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MAIN_1.toc"), b"whoosh").unwrap();
        assert_eq!(open_or_rebuild_index(&dir, Some("en")).unwrap(), OpenOutcome::RecreatedSettings);
        assert!(!settings_mismatch(&dir, Some("en")), "Einstellungen gestempelt wie upstream");
        assert!(needs_rebuild(&dir, Some("en")), "leerer Index darf nicht als aktuell gelten");
        // opening a second time (other process) changes nothing
        assert_eq!(open_or_rebuild_index(&dir, Some("en")).unwrap(), OpenOutcome::Opened);
        assert!(needs_rebuild(&dir, Some("en")));
        // successful rebuild clears the marker
        clear_rebuild_marker(&dir).unwrap();
        assert!(!needs_rebuild(&dir, Some("en")));
    }
}
