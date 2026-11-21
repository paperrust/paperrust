// upstream: src/documents/data_models.py@v3.4.2 (DocumentMetadataOverrides, DocumentSource,
//           ConsumableDocument, StoredBarcode, ConsumeFile*Result)
//
// JSON form like docs/contract/pl-queue.md B5 (`consume_file` kwargs), so that the task can run without
// Python pickle via the own queue.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use pl_matching::workflow::DocumentSource;

/// `StoredBarcode` (page 1-based).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBarcode {
    pub page: i64,
    pub value: String,
    pub format: String,
}

/// `DocumentMetadataOverrides`. All fields `None` = no default.
///
/// `custom_fields`: field ID → value (JSON, as `value_<type>` is stored).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentMetadataOverrides {
    pub filename: Option<String>,
    pub title: Option<String>,
    pub correspondent_id: Option<i64>,
    pub document_type_id: Option<i64>,
    pub tag_ids: Option<Vec<i64>>,
    pub storage_path_id: Option<i64>,
    pub created: Option<NaiveDate>,
    pub asn: Option<i64>,
    pub owner_id: Option<i64>,
    pub view_users: Option<Vec<i64>>,
    pub view_groups: Option<Vec<i64>>,
    pub change_users: Option<Vec<i64>>,
    pub change_groups: Option<Vec<i64>>,
    pub custom_fields: Option<BTreeMap<i64, Value>>,
    pub skip_asn_if_exists: bool,
    pub version_label: Option<String>,
    pub actor_id: Option<i64>,
    pub remote_ocr: bool,
    /// PaperRust: "AI text recognition" chosen for this document (upload field `ai_ocr`). Serialized only
    /// when set, so that `input_data` of the tasks stays like upstream.
    #[serde(skip_serializing_if = "is_false")]
    pub ai_ocr: bool,
    pub barcodes: Option<Vec<StoredBarcode>>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn merge_set(dst: &mut Option<Vec<i64>>, other: &Option<Vec<i64>>) {
    match (dst.as_mut(), other) {
        (None, o) => *dst = o.clone(),
        (Some(d), Some(o)) => {
            d.extend(o.iter().copied());
            // upstream: list(set(...)) – order of a Python set; here ascending.
            d.sort_unstable();
            d.dedup();
        }
        (Some(_), None) => {}
    }
}

impl DocumentMetadataOverrides {
    /// `DocumentMetadataOverrides.update(other)` (in place).
    pub fn update(&mut self, other: &DocumentMetadataOverrides) -> &mut Self {
        if other.title.is_some() {
            self.title.clone_from(&other.title);
        }
        if other.correspondent_id.is_some() {
            self.correspondent_id = other.correspondent_id;
        }
        if other.document_type_id.is_some() {
            self.document_type_id = other.document_type_id;
        }
        if other.storage_path_id.is_some() {
            self.storage_path_id = other.storage_path_id;
        }
        if other.owner_id.is_some() {
            self.owner_id = other.owner_id;
        }
        if other.actor_id.is_some() {
            self.actor_id = other.actor_id;
        }
        if other.skip_asn_if_exists {
            self.skip_asn_if_exists = true;
        }
        if other.remote_ocr {
            self.remote_ocr = true;
        }
        if other.ai_ocr {
            self.ai_ocr = true;
        }
        if other.version_label.is_some() {
            self.version_label.clone_from(&other.version_label);
        }
        merge_set(&mut self.tag_ids, &other.tag_ids);
        merge_set(&mut self.view_users, &other.view_users);
        merge_set(&mut self.view_groups, &other.view_groups);
        merge_set(&mut self.change_users, &other.change_users);
        merge_set(&mut self.change_groups, &other.change_groups);
        match (self.custom_fields.as_mut(), &other.custom_fields) {
            (None, o) => self.custom_fields = o.clone(),
            (Some(d), Some(o)) => d.extend(o.iter().map(|(k, v)| (*k, v.clone()))),
            (Some(_), None) => {}
        }
        self
    }

    /// `input_data["overrides"]` of `PaperlessTask` (`_extract_input_data`): only fields that are
    /// not `None`; `created` as ISO date.
    pub fn to_input_data(&self) -> Map<String, Value> {
        let mut out = Map::new();
        if let Ok(Value::Object(m)) = serde_json::to_value(self) {
            for (k, v) in m {
                if !v.is_null() {
                    out.insert(k, v);
                }
            }
        }
        out
    }

    /// `view_users + change_users` or groups (for status messages, `_send_progress`).
    pub fn users_can_view(&self) -> Vec<i64> {
        let mut v = self.view_users.clone().unwrap_or_default();
        v.extend(self.change_users.clone().unwrap_or_default());
        v
    }
    pub fn groups_can_view(&self) -> Vec<i64> {
        let mut v = self.view_groups.clone().unwrap_or_default();
        v.extend(self.change_groups.clone().unwrap_or_default());
        v
    }
}

/// `ConsumableDocument`. `mime_type` is determined on creation as upstream (`__post_init__`) via a
/// libmagic replica.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsumableDocument {
    pub source: DocumentSourceRepr,
    pub original_file: PathBuf,
    #[serde(default)]
    pub root_document_id: Option<i64>,
    #[serde(default)]
    pub original_path: Option<PathBuf>,
    #[serde(default)]
    pub mailrule_id: Option<i64>,
    #[serde(default)]
    pub mime_type: String,
}

/// `DocumentSource` as a number in JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct DocumentSourceRepr(pub DocumentSource);

impl TryFrom<i64> for DocumentSourceRepr {
    type Error = String;
    fn try_from(v: i64) -> Result<Self, String> {
        DocumentSource::from_i64(v).map(DocumentSourceRepr).ok_or_else(|| format!("{v} is not a valid DocumentSource"))
    }
}

impl From<DocumentSourceRepr> for i64 {
    fn from(v: DocumentSourceRepr) -> i64 {
        v.0 as i64
    }
}

impl ConsumableDocument {
    /// `ConsumableDocument(source, original_file, ...)`: make the path absolute, determine MIME.
    pub fn new(source: DocumentSource, original_file: impl AsRef<Path>) -> std::io::Result<Self> {
        let p = absolute(original_file.as_ref());
        let mime_type = pl_parsers::mime::detect_file(&p)?;
        Ok(Self {
            source: DocumentSourceRepr(source),
            original_file: p,
            root_document_id: None,
            original_path: None,
            mailrule_id: None,
            mime_type,
        })
    }

    pub fn source(&self) -> DocumentSource {
        self.source.0
    }

    /// `original_file.name`.
    pub fn file_name(&self) -> String {
        self.original_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// For `pl_matching::workflow` (trigger evaluation).
    pub fn as_matching(&self) -> pl_matching::workflow::ConsumableDocument {
        pl_matching::workflow::ConsumableDocument {
            source: self.source.0,
            original_file: path_string(&self.original_file),
            original_path: self.original_path.as_deref().map(path_string),
            mailrule_id: self.mailrule_id,
        }
    }

    /// `input_data` of the `PaperlessTask` (`_extract_input_data` for `consume_file`).
    pub fn task_input_data(&self, overrides: &DocumentMetadataOverrides) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("filename".into(), Value::from(self.file_name()));
        m.insert("mime_type".into(), Value::from(self.mime_type.clone()));
        if let Some(p) = &self.original_path {
            m.insert("source_path".into(), Value::from(path_string(p)));
        }
        if let Some(r) = self.mailrule_id {
            m.insert("mailrule_id".into(), Value::from(r));
        }
        let o = overrides.to_input_data();
        if !o.is_empty() {
            m.insert("overrides".into(), Value::Object(o));
        }
        m
    }
}

/// `Path(...).resolve()` without a symlink requirement (file need not exist).
pub fn absolute(p: &Path) -> PathBuf {
    let abs =
        if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) };
    let canon = std::fs::canonicalize(&abs).unwrap_or(abs);
    strip_verbatim(canon)
}

/// Windows: `\\?\C:\...` (result of `canonicalize`) → `C:\...`.
pub fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p
}

pub fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Result of `consume_file` (return value of the task).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsumeOutcome {
    /// `ConsumeFileSuccessResult {document_id}`
    Success { document_id: i64 },
    /// `ConsumeFileStoppedResult {reason}` (barcode splitting, double-sided scan)
    Stopped { reason: String },
    /// `ConsumeFileDuplicateResult {duplicate_of, duplicate_in_trash}`
    Duplicate { duplicate_of: i64, duplicate_in_trash: bool },
}

impl ConsumeOutcome {
    /// Celery return value (dict) – ends up in `PaperlessTask.result_data`.
    pub fn to_value(&self) -> Value {
        match self {
            Self::Success { document_id } => serde_json::json!({ "document_id": document_id }),
            Self::Stopped { reason } => serde_json::json!({ "reason": reason }),
            Self::Duplicate { duplicate_of, duplicate_in_trash } => {
                serde_json::json!({ "duplicate_of": duplicate_of, "duplicate_in_trash": duplicate_in_trash })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_merges_like_upstream() {
        let mut a = DocumentMetadataOverrides { tag_ids: Some(vec![3, 1]), title: Some("a".into()), ..Default::default() };
        let b = DocumentMetadataOverrides {
            tag_ids: Some(vec![1, 2]),
            correspondent_id: Some(5),
            custom_fields: Some(BTreeMap::from([(1, Value::from("x"))])),
            skip_asn_if_exists: true,
            ..Default::default()
        };
        a.update(&b);
        assert_eq!(a.tag_ids, Some(vec![1, 2, 3]));
        assert_eq!(a.title.as_deref(), Some("a"));
        assert_eq!(a.correspondent_id, Some(5));
        assert!(a.skip_asn_if_exists);
        assert_eq!(a.custom_fields.as_ref().map(|m| m.len()), Some(1));
        // None in other does not reset
        let mut c = DocumentMetadataOverrides { view_users: Some(vec![2]), ..Default::default() };
        c.update(&DocumentMetadataOverrides::default());
        assert_eq!(c.view_users, Some(vec![2]));
    }

    #[test]
    fn json_roundtrip_b5() {
        let v = serde_json::json!({
            "source": 1, "original_file": "/tmp/x.pdf", "mime_type": "application/pdf",
            "original_path": null, "mailrule_id": null, "root_document_id": null
        });
        let d: ConsumableDocument = serde_json::from_value(v).unwrap();
        assert_eq!(d.source(), DocumentSource::ConsumeFolder);
        let o: DocumentMetadataOverrides =
            serde_json::from_value(serde_json::json!({"tag_ids": [1], "created": "2024-03-01", "custom_fields": {"3": "x"}})).unwrap();
        assert_eq!(o.created, NaiveDate::from_ymd_opt(2024, 3, 1));
        assert_eq!(o.custom_fields.unwrap()[&3], Value::from("x"));
        let inp = DocumentMetadataOverrides { tag_ids: Some(vec![1]), ..Default::default() }.to_input_data();
        assert_eq!(Value::Object(inp), serde_json::json!({"tag_ids": [1], "skip_asn_if_exists": false, "remote_ocr": false}));
        // PaperRust ai_ocr: serialized only when set, round trip and update()
        let mut o = DocumentMetadataOverrides { ai_ocr: true, ..Default::default() };
        assert_eq!(o.to_input_data().get("ai_ocr"), Some(&Value::Bool(true)));
        let back: DocumentMetadataOverrides = serde_json::from_value(Value::Object(o.to_input_data())).unwrap();
        assert!(back.ai_ocr);
        o.update(&DocumentMetadataOverrides::default());
        assert!(o.ai_ocr);
        let mut p = DocumentMetadataOverrides::default();
        p.update(&o);
        assert!(p.ai_ocr);
    }

    #[test]
    fn outcome_values() {
        assert_eq!(ConsumeOutcome::Success { document_id: 4 }.to_value(), serde_json::json!({"document_id": 4}));
        assert_eq!(
            ConsumeOutcome::Duplicate { duplicate_of: 1, duplicate_in_trash: false }.to_value(),
            serde_json::json!({"duplicate_of": 1, "duplicate_in_trash": false})
        );
    }
}
