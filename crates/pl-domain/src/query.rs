//! Query AST for documents (`POST /rs/api/v2/documents/query`), ARCHITECTURE-V2 §5 L1.
//!
//! JSON shape (externally tagged, unknown keys → error):
//!
//! ```json
//! {"filter": {"all": [
//!     {"text": {"query": "rechnung", "field": "title_content"}},
//!     {"tags": {"all": ["<uuid>"], "none": ["<uuid>"]}},
//!     {"correspondent": {"any": ["<uuid>"]}},
//!     {"created": {"from": "2024-01-01", "to": "2024-12-31"}}
//!  ]},
//!  "sort": "-created", "limit": 50, "cursor": null, "include": ["content"], "count": true}
//! ```
//!
//! `pl-store` translates the tree into the resolved filter AST [`crate::filter`] (L1): leaves
//! via the v1 check (`pl_docfilter`), `all`/`any`/`not` as AND/OR/set complement.
//! [`Cond::conjunction`] (`all` only) remains for the previous path
//! (formerly `PAPERRUST_QUERY_ENGINE=legacy` without `any`/`not`; since data model stage 7 AST only).

use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::DomainError;

/// Request body of `documents/query`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocQuery {
    #[serde(default)]
    pub filter: Option<Cond>,
    /// Sort order (see [`SORT_FIELDS`]), prefix `-` = descending. Default `-created`.
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub cursor: Option<String>,
    /// `content`, `notes`, `sharing`.
    #[serde(default)]
    pub include: Vec<String>,
    /// Also return the total count (expensive for large collections, docs/SYNC.md §3.6).
    #[serde(default)]
    pub count: bool,
}

/// Sortable fields of the document list (keyset pagination, tie-breaker `legacy_id`).
pub const SORT_FIELDS: [&str; 5] = ["created", "added", "modified", "title", "id"];

/// Default sort order like the v1 list (`-created`).
pub const DEFAULT_SORT: &str = "-created";

/// Check the sort order: (field, descending).
pub fn parse_sort(s: Option<&str>) -> Result<(&'static str, bool), DomainError> {
    let s = s.unwrap_or(DEFAULT_SORT).trim();
    let (desc, name) = match s.strip_prefix('-') {
        Some(n) => (true, n),
        None => (false, s),
    };
    SORT_FIELDS
        .iter()
        .find(|f| **f == name)
        .map(|f| (*f, desc))
        .ok_or_else(|| DomainError::field("sort", format!("Unknown sort field \"{name}\".")))
}

/// A condition.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Cond {
    /// All sub-conditions (AND).
    All(Vec<Cond>),
    /// At least one (OR).
    Any(Vec<Cond>),
    /// Negation (set complement).
    Not(Box<Cond>),
    /// Substring search (case-insensitive).
    Text(TextCond),
    Tags(RefSet),
    Correspondent(RefSet),
    DocumentType(RefSet),
    StoragePath(RefSet),
    /// Owner (user UUIDs).
    Owner(RefSet),
    /// Document has custom fields (field UUIDs).
    CustomFields(RefSet),
    /// Custom field query in v1 format (`custom_field_query`), fields as UUID **or**
    /// name (transition, ARCHITECTURE-V2 L1).
    CustomFieldQuery(Value),
    Created(DateRange),
    Added(DateRange),
    Modified(DateRange),
    Asn(AsnCond),
    /// Only these documents.
    Uuids(Vec<Uuid>),
    MimeType(String),
    /// Documents shared by the requesting user.
    SharedByMe(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextField {
    #[default]
    TitleContent,
    Title,
    Content,
    OriginalFilename,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextCond {
    pub query: String,
    #[serde(default)]
    pub field: TextField,
}

/// Reference set: `any` (one of them), `all` (all; tags/custom fields only), `none` (none of them),
/// `is_null` (without assignment; for tags = without tags).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefSet {
    #[serde(default)]
    pub any: Vec<Uuid>,
    #[serde(default)]
    pub all: Vec<Uuid>,
    #[serde(default)]
    pub none: Vec<Uuid>,
    #[serde(default)]
    pub is_null: Option<bool>,
}

/// Date range, both bounds inclusive.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateRange {
    #[serde(default)]
    pub from: Option<NaiveDate>,
    #[serde(default)]
    pub to: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AsnCond {
    #[serde(default)]
    pub eq: Option<i64>,
    #[serde(default)]
    pub gt: Option<i64>,
    #[serde(default)]
    pub lt: Option<i64>,
    #[serde(default)]
    pub is_null: Option<bool>,
}

impl Cond {
    /// Resolve nested `all` → list of leaves (AND). `any`/`not` → 501 (only for the
    /// previous path via v1 parameters; the AST path can do both).
    pub fn conjunction(&self) -> Result<Vec<&Cond>, DomainError> {
        let mut out = Vec::new();
        self.collect(&mut out)?;
        Ok(out)
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a Cond>) -> Result<(), DomainError> {
        match self {
            Cond::All(v) => {
                for c in v {
                    c.collect(out)?;
                }
                Ok(())
            }
            Cond::Any(_) | Cond::Not(_) => Err(DomainError::NotImplemented(
                "\"any\"/\"not\" over arbitrary conditions are not supported yet; use the any/none lists of the reference conditions"
                    .into(),
            )),
            leaf => {
                out.push(leaf);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_example() {
        let q: DocQuery = serde_json::from_str(
            r#"{"filter":{"all":[{"text":{"query":"a"}},{"all":[{"tags":{"all":["00000000-0000-0000-0000-000000000001"]}}]},
                {"created":{"from":"2024-01-01"}},{"asn":{"is_null":true}}]},"sort":"title","limit":5}"#,
        )
        .unwrap();
        let leaves = q.filter.as_ref().unwrap().conjunction().unwrap();
        assert_eq!(leaves.len(), 4);
        assert!(matches!(leaves[0], Cond::Text(TextCond { field: TextField::TitleContent, .. })));
        assert_eq!(parse_sort(q.sort.as_deref()).unwrap(), ("title", false));
        assert_eq!(parse_sort(None).unwrap(), ("created", true));
        assert!(parse_sort(Some("content")).is_err());
    }

    #[test]
    fn rejects_unknown_and_or() {
        assert!(serde_json::from_str::<DocQuery>(r#"{"filter":{"bogus":1}}"#).is_err());
        assert!(serde_json::from_str::<DocQuery>(r#"{"filtr":{}}"#).is_err());
        let q: DocQuery = serde_json::from_str(r#"{"filter":{"any":[{"mime_type":"a"}]}}"#).unwrap();
        assert!(matches!(q.filter.unwrap().conjunction(), Err(DomainError::NotImplemented(_))));
    }
}
