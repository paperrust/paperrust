//! Evaluate a paperless-ngx export (`document_exporter`, folder or ZIP) read-only:
//! object counts per model, document rows (checksums, title, content hash, tags) and the
//! exported files. The manifest is **streamed** (one record after the other), so that
//! large exports do not end up entirely in memory.
//!
//! upstream: src/documents/management/commands/document_exporter.py (keys `__exported_*`,
//! `metadata.json`, `--split-manifest` → `*-manifest.json`), document_importer.py (search for
//! `**/*-manifest.json`).

use crate::zipread::{ZipIndex, looks_like_zip, normalize};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde::de::{DeserializeSeed, Deserializer, SeqAccess, Visitor};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

pub enum ExportSource {
    Dir(PathBuf),
    Zip(ZipIndex),
}

/// A document from source or target (everything the verification report and sample need).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocRow {
    pub id: i64,
    pub title: String,
    /// SHA-256 (hex) of the content text
    pub content_sha256: String,
    pub checksum: String,
    pub archive_checksum: Option<String>,
    /// relative path (media folder: under documents/originals; export: path in the export)
    pub filename: Option<String>,
    pub archive_filename: Option<String>,
    pub thumbnail: Option<String>,
    pub deleted: bool,
    pub tags: Vec<i64>,
    /// Version of another document (`root_document_id`, from v3.x); API v1 shows only roots
    pub root: Option<i64>,
    pub version_index: Option<i64>,
}

#[derive(Debug, Default)]
pub struct ManifestSummary {
    /// Count per model (`documents.document` …), users without consumer/AnonymousUser
    pub counts: BTreeMap<String, i64>,
    pub docs: Vec<DocRow>,
    pub manifests: Vec<String>,
}

pub fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    crate::value::hex(&sha2::Sha256::digest(b))
}

impl ExportSource {
    /// Folder with manifest.json or ZIP file with manifest.json.
    pub fn open(path: &Path) -> Result<ExportSource> {
        if path.is_dir() {
            if !path.join("manifest.json").is_file() {
                bail!("{} enthält keine manifest.json – kein paperless-ngx-Export", path.display());
            }
            return Ok(ExportSource::Dir(path.to_path_buf()));
        }
        if path.is_file() && looks_like_zip(path) {
            let z = ZipIndex::open(path)?;
            if z.get("manifest.json").is_none() {
                bail!("{} enthält keine manifest.json – kein paperless-ngx-Export", path.display());
            }
            return Ok(ExportSource::Zip(z));
        }
        bail!("{} ist weder ein Export-Ordner noch eine ZIP-Datei", path.display())
    }

    pub fn exists(&self, rel: &str) -> bool {
        match self {
            ExportSource::Dir(d) => d.join(normalize(rel)).is_file(),
            ExportSource::Zip(z) => z.get(rel).is_some(),
        }
    }

    pub fn open_file(&self, rel: &str) -> Result<Box<dyn Read>> {
        match self {
            ExportSource::Dir(d) => {
                let p = d.join(normalize(rel));
                let f = std::fs::File::open(&p).with_context(|| format!("{} fehlt", p.display()))?;
                Ok(Box::new(BufReader::with_capacity(1 << 16, f)))
            }
            ExportSource::Zip(z) => z.open_entry(rel),
        }
    }

    /// Does the export come from paperless-ngx 1.x (or paperless-ng)? Jetson check B2: such exports
    /// contain fields that no longer exist from ngx 2.0 on (`storage_type`); the import then aborts
    /// in the middle of loading (upstream ngx 3.x likewise). Detection via the version in `metadata.json`,
    /// without a version via the field in the manifest.
    pub fn is_v1_export(&self) -> bool {
        match self.version() {
            Some(v) => {
                let v = v.trim_start_matches('v');
                v.starts_with("1.") || v.starts_with("0.")
            }
            None => self.manifest_names().iter().any(|n| self.manifest_has(n, b"\"storage_type\"")),
        }
    }

    /// Does `needle` occur in the manifest file `rel` (read in pieces, little memory)?
    fn manifest_has(&self, rel: &str, needle: &[u8]) -> bool {
        let Ok(mut r) = self.open_file(rel) else {
            return false;
        };
        let mut buf = vec![0u8; 1 << 16];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            let n = match r.read(&mut buf) {
                Ok(0) | Err(_) => return false,
                Ok(n) => n,
            };
            carry.extend_from_slice(&buf[..n]);
            if carry.windows(needle.len()).any(|w| w == needle) {
                return true;
            }
            let keep = carry.len().saturating_sub(needle.len());
            carry.drain(..keep);
        }
    }

    /// All manifest files: manifest.json + `**/*-manifest.json` (like the upstream importer).
    pub fn manifest_names(&self) -> Vec<String> {
        let mut v = vec!["manifest.json".to_string()];
        match self {
            ExportSource::Dir(d) => {
                let mut stack = vec![d.clone()];
                while let Some(dir) = stack.pop() {
                    let Ok(rd) = std::fs::read_dir(&dir) else {
                        continue;
                    };
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            stack.push(p);
                        } else if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("-manifest.json"))
                            && let Ok(rel) = p.strip_prefix(d)
                        {
                            v.push(rel.to_string_lossy().replace('\\', "/"));
                        }
                    }
                }
            }
            ExportSource::Zip(z) => {
                for e in z.entries() {
                    if e.name.ends_with("-manifest.json") {
                        v.push(normalize(&e.name));
                    }
                }
            }
        }
        v[1..].sort();
        v
    }

    /// `metadata.json` → `version` (missing in very old exports).
    pub fn version(&self) -> Option<String> {
        let mut s = String::new();
        self.open_file("metadata.json").ok()?.read_to_string(&mut s).ok()?;
        let v: Value = serde_json::from_str(&s).ok()?;
        v.get("version").and_then(Value::as_str).map(str::to_string)
    }

    /// Read all manifests in a streaming fashion.
    pub fn summarize(&self) -> Result<ManifestSummary> {
        let mut sum = ManifestSummary::default();
        for name in self.manifest_names() {
            let r = self.open_file(&name)?;
            let mut de = serde_json::Deserializer::from_reader(r);
            de.deserialize_seq(RecordsVisitor { sum: &mut sum }).map_err(|e| anyhow!("{name}: Manifest nicht lesbar: {e}"))?;
            sum.manifests.push(name);
        }
        sum.docs.sort_by_key(|d| d.id);
        Ok(sum)
    }
}

#[derive(Deserialize)]
struct Record {
    model: String,
    #[serde(default)]
    pk: Value,
    #[serde(default)]
    fields: Fields,
    #[serde(rename = "__exported_file_name__")]
    file: Option<String>,
    #[serde(rename = "__exported_archive_name__")]
    archive: Option<String>,
    #[serde(rename = "__exported_thumbnail_name__")]
    thumb: Option<String>,
}

#[derive(Deserialize, Default)]
struct Fields {
    username: Option<String>,
    title: Option<String>,
    content: Option<String>,
    checksum: Option<String>,
    archive_checksum: Option<String>,
    deleted_at: Option<Value>,
    root_document: Option<Value>,
    version_index: Option<Value>,
    #[serde(default)]
    tags: Option<Vec<Value>>,
}

fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Excluded users like upstream (`_excluded_usernames`).
pub const EXCLUDED_USERS: [&str; 2] = ["consumer", "AnonymousUser"];

impl ManifestSummary {
    fn add(&mut self, r: Record) {
        if r.model == "auth.user" && r.fields.username.as_deref().is_some_and(|u| EXCLUDED_USERS.contains(&u)) {
            return;
        }
        *self.counts.entry(r.model.clone()).or_default() += 1;
        if r.model == "documents.document" {
            let tags: Vec<i64> = r.fields.tags.as_deref().unwrap_or_default().iter().filter_map(as_i64).collect();
            *self.counts.entry("documents.document_tags".into()).or_default() += tags.len() as i64;
            self.docs.push(DocRow {
                id: as_i64(&r.pk).unwrap_or(-1),
                title: r.fields.title.unwrap_or_default(),
                content_sha256: sha256_hex(r.fields.content.unwrap_or_default().as_bytes()),
                checksum: r.fields.checksum.unwrap_or_default(),
                archive_checksum: r.fields.archive_checksum.filter(|s| !s.is_empty()),
                filename: r.file,
                archive_filename: r.archive,
                thumbnail: r.thumb,
                deleted: r.fields.deleted_at.as_ref().is_some_and(|v| !v.is_null()),
                root: r.fields.root_document.as_ref().and_then(as_i64),
                version_index: r.fields.version_index.as_ref().and_then(as_i64),
                tags: {
                    let mut t = tags;
                    t.sort_unstable();
                    t
                },
            });
        }
    }
}

struct RecordsVisitor<'a> {
    sum: &'a mut ManifestSummary,
}

impl<'de> Visitor<'de> for RecordsVisitor<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON-Liste von Manifest-Datensätzen")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(r) = seq.next_element_seed(RecordSeed)? {
            self.sum.add(r);
        }
        Ok(())
    }
}

struct RecordSeed;
impl<'de> DeserializeSeed<'de> for RecordSeed {
    type Value = Record;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Record, D::Error> {
        Record::deserialize(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const MANIFEST: &str = r#"[
 {"model":"auth.user","pk":1,"fields":{"username":"admin"}},
 {"model":"auth.user","pk":2,"fields":{"username":"AnonymousUser"}},
 {"model":"documents.tag","pk":1,"fields":{"name":"a"}},
 {"model":"documents.tag","pk":2,"fields":{"name":"b"}},
 {"model":"documents.document","pk":7,"fields":{"title":"T","content":"hallo","checksum":"abc","archive_checksum":null,"tags":[2,1],"deleted_at":null},
  "__exported_file_name__":"originals/0000007.pdf","__exported_thumbnail_name__":"thumbnails/0000007.webp"}
]"#;

    /// Jetson check B2: detect an export from ngx 1.x (with and without metadata.json).
    #[test]
    fn detects_v1_exports() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("manifest.json"), MANIFEST).unwrap();
        std::fs::write(d.path().join("metadata.json"), r#"{"version":"3.4.2"}"#).unwrap();
        assert!(!ExportSource::open(d.path()).unwrap().is_v1_export());
        std::fs::write(d.path().join("metadata.json"), r#"{"version":"1.17.4"}"#).unwrap();
        assert!(ExportSource::open(d.path()).unwrap().is_v1_export());
        std::fs::write(d.path().join("metadata.json"), r#"{"version":"2.20.0"}"#).unwrap();
        assert!(!ExportSource::open(d.path()).unwrap().is_v1_export());
        // without metadata.json: field in the manifest decides (also beyond 64 KiB boundaries)
        std::fs::remove_file(d.path().join("metadata.json")).unwrap();
        assert!(!ExportSource::open(d.path()).unwrap().is_v1_export());
        let pad = " ".repeat(65_530);
        std::fs::write(
            d.path().join("manifest.json"),
            format!(r#"[{pad}{{"model":"documents.document","pk":1,"fields":{{"title":"T","storage_type":"unencrypted"}}}}]"#),
        )
        .unwrap();
        assert!(ExportSource::open(d.path()).unwrap().is_v1_export());
    }

    #[test]
    fn summarize_dir_and_split() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("manifest.json"), MANIFEST).unwrap();
        std::fs::create_dir_all(d.path().join("json")).unwrap();
        std::fs::write(
            d.path().join("json/0000008-manifest.json"),
            r#"[{"model":"documents.document","pk":8,"fields":{"title":"U","content":"","checksum":"x","tags":[],"deleted_at":"2025-01-01T00:00:00Z"}},
                {"model":"documents.note","pk":1,"fields":{"note":"n"}}]"#,
        )
        .unwrap();
        std::fs::write(d.path().join("metadata.json"), r#"{"version":"3.4.2"}"#).unwrap();
        let e = ExportSource::open(d.path()).unwrap();
        assert!(matches!(e, ExportSource::Dir(_)));
        assert_eq!(e.version().as_deref(), Some("3.4.2"));
        let s = e.summarize().unwrap();
        assert_eq!(s.manifests, vec!["manifest.json", "json/0000008-manifest.json"]);
        assert_eq!(s.counts["auth.user"], 1, "AnonymousUser zählt nicht");
        assert_eq!(s.counts["documents.tag"], 2);
        assert_eq!(s.counts["documents.document"], 2);
        assert_eq!(s.counts["documents.note"], 1);
        assert_eq!(s.counts["documents.document_tags"], 2);
        assert_eq!(s.docs[0].tags, vec![1, 2]);
        assert_eq!(s.docs[0].content_sha256, sha256_hex(b"hallo"));
        assert_eq!(s.docs[0].filename.as_deref(), Some("originals/0000007.pdf"));
        assert!(s.docs[1].deleted);
        assert!(!s.docs[0].deleted);
    }

    #[test]
    fn zip_export_and_rejects() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("export.zip");
        std::fs::write(
            &p,
            crate::zipread::testzip::build(&[("manifest.json", MANIFEST.as_bytes(), true), ("originals/0000007.pdf", b"%PDF", false)]),
        )
        .unwrap();
        let e = ExportSource::open(&p).unwrap();
        assert!(matches!(e, ExportSource::Zip(_)));
        assert!(e.exists("originals/0000007.pdf"));
        assert_eq!(e.version(), None);
        assert_eq!(e.summarize().unwrap().docs.len(), 1);
        assert!(ExportSource::open(d.path()).is_err(), "Ordner ohne manifest.json");
        let bad = d.path().join("bad.zip");
        std::fs::write(&bad, crate::zipread::testzip::build(&[("x.txt", b"1", false)])).unwrap();
        assert!(ExportSource::open(&bad).is_err());
    }
}
