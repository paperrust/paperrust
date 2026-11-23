//! Domain types. Serialised they directly yield the v2 JSON shapes (docs/SYNC.md §3.3,
//! docs/contract/pl-api-v2.md): references as UUID, timestamps RFC 3339 UTC (`…Z`), plain dates
//! `YYYY-MM-DD`, permissions as [`Access`].

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Object kind. Codes = `paperrust_sync_object.kind` (docs/SYNC.md §2, stable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Document,
    Tag,
    Correspondent,
    DocumentType,
    StoragePath,
    CustomField,
    SavedView,
    User,
    Group,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Document,
        Kind::Tag,
        Kind::Correspondent,
        Kind::DocumentType,
        Kind::StoragePath,
        Kind::CustomField,
        Kind::SavedView,
        Kind::User,
        Kind::Group,
    ];

    pub fn code(self) -> i16 {
        match self {
            Kind::Document => 1,
            Kind::Tag => 2,
            Kind::Correspondent => 3,
            Kind::DocumentType => 4,
            Kind::StoragePath => 5,
            Kind::CustomField => 6,
            Kind::SavedView => 7,
            Kind::User => 8,
            Kind::Group => 9,
        }
    }

    pub fn from_code(c: i16) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.code() == c)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Document => "document",
            Kind::Tag => "tag",
            Kind::Correspondent => "correspondent",
            Kind::DocumentType => "document_type",
            Kind::StoragePath => "storage_path",
            Kind::CustomField => "custom_field",
            Kind::SavedView => "saved_view",
            Kind::User => "user",
            Kind::Group => "group",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Path segment of the list (`/rs/api/v2/<plural>`), if the kind has its own list.
    pub fn plural(self) -> &'static str {
        match self {
            Kind::Document => "documents",
            Kind::Tag => "tags",
            Kind::Correspondent => "correspondents",
            Kind::DocumentType => "document_types",
            Kind::StoragePath => "storage_paths",
            Kind::CustomField => "custom_fields",
            Kind::SavedView => "saved_views",
            Kind::User => "users",
            Kind::Group => "groups",
        }
    }

    pub fn from_plural(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.plural() == s)
    }
}

/// What the requesting user may do with the object (core decision, same as v1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Access {
    pub can_view: bool,
    pub can_change: bool,
    pub can_delete: bool,
}

/// Reference to a user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRef {
    pub uuid: Uuid,
    pub username: String,
}

/// Reference to a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRef {
    pub uuid: Uuid,
    pub name: String,
}

/// Shares of an object (only for owner/superuser, `include=sharing`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Sharing {
    pub view: Grantees,
    pub change: Grantees,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Grantees {
    pub users: Vec<UserRef>,
    pub groups: Vec<GroupRef>,
}

/// Matching rule (algorithm as a name instead of a number).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Matching {
    /// `none|any|all|literal|regex|fuzzy|auto`
    pub algorithm: String,
    pub pattern: String,
    pub is_insensitive: bool,
}

/// upstream `MatchingModel.MATCHING_ALGORITHMS` (0…6) ↔ names.
pub const MATCHING_ALGORITHMS: [&str; 7] = ["none", "any", "all", "literal", "regex", "fuzzy", "auto"];

pub fn matching_name(code: i16) -> String {
    MATCHING_ALGORITHMS.get(code as usize).copied().unwrap_or("none").to_string()
}

pub fn matching_code(name: &str) -> Option<i16> {
    MATCHING_ALGORITHMS.iter().position(|n| *n == name).map(|i| i as i16)
}

/// Common header data of every object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub uuid: Uuid,
    pub legacy_id: i64,
    pub rev: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    #[serde(flatten)]
    pub meta: Meta,
    pub name: String,
    pub color: String,
    /// `#ffffff`/`#000000` like upstream `TagSerializer.get_text_color`.
    pub text_color: String,
    pub is_inbox_tag: bool,
    pub matching: Matching,
    pub parent: Option<Uuid>,
    pub owner: Option<UserRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_count: Option<i64>,
    pub access: Access,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharing: Option<Sharing>,
}

/// Correspondent or document type (same shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedObject {
    #[serde(flatten)]
    pub meta: Meta,
    pub name: String,
    pub matching: Matching,
    pub owner: Option<UserRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_count: Option<i64>,
    pub access: Access,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharing: Option<Sharing>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoragePath {
    #[serde(flatten)]
    pub meta: Meta,
    pub name: String,
    /// Template (legacy syntax, ARCHITECTURE-V2 L2).
    pub path: String,
    pub matching: Matching,
    pub owner: Option<UserRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_count: Option<i64>,
    pub access: Access,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharing: Option<Sharing>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomField {
    #[serde(flatten)]
    pub meta: Meta,
    pub name: String,
    pub data_type: String,
    pub extra_data: Value,
    pub created: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilterRule {
    pub rule_type: i64,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedView {
    #[serde(flatten)]
    pub meta: Meta,
    pub name: String,
    pub sort_field: Option<String>,
    pub sort_reverse: bool,
    pub page_size: Option<i64>,
    pub display_mode: Option<String>,
    pub display_fields: Option<Value>,
    pub icon: Option<String>,
    /// v1 filter rules (transitional, values as in v1: integer IDs as text).
    pub filter_rules: Vec<FilterRule>,
    pub owner: Option<UserRef>,
    pub access: Access,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    #[serde(flatten)]
    pub meta: Meta,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    #[serde(flatten)]
    pub meta: Meta,
    pub name: String,
}

/// An object of any (non-document) kind.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Object {
    Tag(Tag),
    Named(NamedObject),
    StoragePath(StoragePath),
    CustomField(CustomField),
    SavedView(SavedView),
    User(User),
    Group(Group),
    Document(Box<Document>),
}

impl Object {
    pub fn meta(&self) -> &Meta {
        match self {
            Object::Tag(o) => &o.meta,
            Object::Named(o) => &o.meta,
            Object::StoragePath(o) => &o.meta,
            Object::CustomField(o) => &o.meta,
            Object::SavedView(o) => &o.meta,
            Object::User(o) => &o.meta,
            Object::Group(o) => &o.meta,
            Object::Document(o) => &o.meta,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomFieldValue {
    pub field: Uuid,
    /// Typed value; document links as a list of UUIDs.
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileInfo {
    pub checksum: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThumbInfo {
    pub etag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Files {
    pub original: FileInfo,
    pub archive: Option<FileInfo>,
    pub thumbnail: ThumbInfo,
}

fn note_format_plain() -> String {
    "plain".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub legacy_id: i64,
    pub note: String,
    /// `plain` or `markdown` (paperless-ngx 3.5 `Note.format`, #14626)
    #[serde(default = "note_format_plain")]
    pub format: String,
    pub created: DateTime<Utc>,
    pub user: Option<UserRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    #[serde(flatten)]
    pub meta: Meta,
    pub title: String,
    pub created: NaiveDate,
    pub added: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub correspondent: Option<Uuid>,
    pub document_type: Option<Uuid>,
    pub storage_path: Option<Uuid>,
    pub tags: Vec<Uuid>,
    pub custom_fields: Vec<CustomFieldValue>,
    pub asn: Option<i64>,
    pub owner: Option<UserRef>,
    pub page_count: Option<i64>,
    pub mime_type: String,
    pub original_filename: Option<String>,
    pub notes_count: i64,
    pub files: Files,
    pub access: Access,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<Note>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharing: Option<Sharing>,
}

/// Extra parts of a document (`include=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocInclude {
    pub content: bool,
    pub notes: bool,
    pub sharing: bool,
}

impl DocInclude {
    /// `include=content,notes,sharing` (unknown parts → error).
    pub fn parse<'a>(parts: impl IntoIterator<Item = &'a str>) -> Result<Self, crate::DomainError> {
        let mut i = DocInclude::default();
        for p in parts {
            match p.trim() {
                "" => {}
                "content" => i.content = true,
                "notes" => i.notes = true,
                "sharing" => i.sharing = true,
                other => return Err(crate::DomainError::field("include", format!("Unknown include \"{other}\"."))),
            }
        }
        Ok(i)
    }
}

/// File variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileVariant {
    Original,
    Archive,
    /// Archive PDF if present, otherwise original; `inline` (like v1 `/preview/`).
    Preview,
    Thumbnail,
}

impl FileVariant {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "original" => Some(FileVariant::Original),
            "archive" => Some(FileVariant::Archive),
            "preview" => Some(FileVariant::Preview),
            "thumbnail" => Some(FileVariant::Thumbnail),
            _ => None,
        }
    }
}

/// A file to be served (path + header data); serving (ETag/Range) is done by the API.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRef {
    pub path: std::path::PathBuf,
    pub content_type: String,
    /// File name for `Content-Disposition`.
    pub filename: String,
    /// Strong ETag (checksum) without quotation marks; `None` → weak ETag from mtime/size.
    pub checksum: Option<String>,
    /// `inline` instead of `attachment`.
    pub inline: bool,
}

/// The signed-in user (`GET /me`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Me {
    pub uuid: Uuid,
    pub legacy_id: i64,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_superuser: bool,
    pub is_staff: bool,
    pub groups: Vec<GroupRef>,
    /// Model permissions (`app_label.codename`), sorted.
    pub permissions: Vec<String>,
    /// Sign-in method of this request (`session|token|device_token|basic|remote_user|auto_login`).
    pub auth: String,
}

/// One page of a list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}

/// List request for non-document kinds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListRequest {
    pub cursor: Option<String>,
    pub limit: i64,
    /// `name`, `-name`, `id`, `-id` (default `name`).
    pub sort: Option<String>,
    /// Substring in the name (case-insensitive).
    pub name_contains: Option<String>,
    pub count: bool,
    pub document_count: bool,
    pub sharing: bool,
}

/// Write data for tags/correspondents/document types (only set fields are changed).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectWrite {
    pub name: Option<String>,
    pub color: Option<String>,
    pub is_inbox_tag: Option<bool>,
    pub matching: Option<MatchingWrite>,
    /// Parent tag (UUID) or `null` for "none".
    #[serde(default, deserialize_with = "double_option")]
    pub parent: Option<Option<Uuid>>,
    /// New owner (user UUID) or `null`.
    #[serde(default, deserialize_with = "double_option")]
    pub owner: Option<Option<Uuid>>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchingWrite {
    pub algorithm: Option<String>,
    pub pattern: Option<String>,
    pub is_insensitive: Option<bool>,
}

/// `PATCH /documents/{uuid}` (only set fields).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentPatch {
    pub title: Option<String>,
    pub created: Option<NaiveDate>,
    #[serde(default, deserialize_with = "double_option")]
    pub correspondent: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "double_option")]
    pub document_type: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "double_option")]
    pub storage_path: Option<Option<Uuid>>,
    pub tags: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "double_option")]
    pub asn: Option<Option<i64>>,
    pub custom_fields: Option<Vec<CustomFieldValue>>,
    #[serde(default, deserialize_with = "double_option")]
    pub owner: Option<Option<Uuid>>,
}

/// Field present with `null` → `Some(None)`, missing → `None`.
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Result of `POST /sync/resolve` per document.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedDoc {
    pub uuid: Uuid,
    pub rev: i64,
    pub size: ResolvedSize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedSize {
    pub original: Option<u64>,
    pub archive: Option<u64>,
}

/// Selection for `POST /sync/resolve` (docs/SYNC.md §3.5).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    #[serde(default)]
    pub documents: Vec<Uuid>,
    #[serde(default)]
    pub tags: Vec<Uuid>,
    #[serde(default)]
    pub saved_views: Vec<Uuid>,
    /// `any` (default) or `all` – combination of the tags.
    #[serde(default)]
    pub tag_mode: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_codes_match_sync_md() {
        for k in Kind::ALL {
            assert_eq!(Kind::from_code(k.code()), Some(k));
            assert_eq!(Kind::parse(k.as_str()), Some(k));
            assert_eq!(Kind::from_plural(k.plural()), Some(k));
        }
        assert_eq!(Kind::Document.code(), 1);
        assert_eq!(Kind::Group.code(), 9);
    }

    #[test]
    fn matching_names() {
        assert_eq!(matching_name(4), "regex");
        assert_eq!(matching_code("auto"), Some(6));
        assert_eq!(matching_code("x"), None);
    }

    #[test]
    fn patch_null_vs_missing() {
        let p: DocumentPatch = serde_json::from_str(r#"{"correspondent":null,"title":"x"}"#).unwrap();
        assert_eq!(p.correspondent, Some(None));
        assert_eq!(p.document_type, None);
        assert!(serde_json::from_str::<DocumentPatch>(r#"{"bogus":1}"#).is_err());
    }

    #[test]
    fn include_parse() {
        let i = DocInclude::parse("content,notes".split(',')).unwrap();
        assert!(i.content && i.notes && !i.sharing);
        assert!(DocInclude::parse(["x"]).is_err());
    }
}
