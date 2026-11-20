//! Static description of the four "matching" object models.
//!
//! upstream: src/documents/models.py@v3.4.2 (MatchingModel, Correspondent, Tag, DocumentType, StoragePath)
//!           src/documents/views.py@v3.4.2 (ordering_fields of the four ViewSets)
//!           src/documents/filters.py@v3.4.2 (CorrespondentFilterSet, TagFilterSet, ...)

/// One of the four object kinds handled by this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Correspondent,
    DocumentType,
    Tag,
    StoragePath,
}

/// A relation from another table to an object kind, needed to emulate Django's `Collector`
/// on delete (Django 5.2 has no DB-level ON DELETE for these FKs).
pub enum Relation {
    /// `on_delete=SET_NULL` foreign key: `UPDATE <table> SET <column> = NULL WHERE <column> IN (...)`.
    SetNull { table: &'static str, column: &'static str },
    /// Auto-created many-to-many through table: `DELETE FROM <table> WHERE <column> IN (...)`.
    M2m { table: &'static str, column: &'static str },
}

pub const MATCH_REGEX: i64 = 4;

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Correspondent, Kind::DocumentType, Kind::Tag, Kind::StoragePath];

    /// pl-authz model descriptor.
    pub fn model(self) -> &'static pl_authz::ObjectModel {
        match self {
            Kind::Correspondent => &pl_authz::models::CORRESPONDENT,
            Kind::DocumentType => &pl_authz::models::DOCUMENT_TYPE,
            Kind::Tag => &pl_authz::models::TAG,
            Kind::StoragePath => &pl_authz::models::STORAGE_PATH,
        }
    }

    /// Static guardian codename `<action>_<model>` (pl-authz filter options need `&'static str`).
    pub fn codename(self, action: &str) -> &'static str {
        match (self, action) {
            (Kind::Correspondent, "change") => "change_correspondent",
            (Kind::Correspondent, "delete") => "delete_correspondent",
            (Kind::Correspondent, _) => "view_correspondent",
            (Kind::DocumentType, "change") => "change_documenttype",
            (Kind::DocumentType, "delete") => "delete_documenttype",
            (Kind::DocumentType, _) => "view_documenttype",
            (Kind::Tag, "change") => "change_tag",
            (Kind::Tag, "delete") => "delete_tag",
            (Kind::Tag, _) => "view_tag",
            (Kind::StoragePath, "change") => "change_storagepath",
            (Kind::StoragePath, "delete") => "delete_storagepath",
            (Kind::StoragePath, _) => "view_storagepath",
        }
    }

    /// Django `_meta.model_name`.
    pub fn model_name(self) -> &'static str {
        match self {
            Kind::Correspondent => "correspondent",
            Kind::DocumentType => "documenttype",
            Kind::Tag => "tag",
            Kind::StoragePath => "storagepath",
        }
    }

    /// Django `_meta.object_name` (used in 404 texts).
    pub fn object_name(self) -> &'static str {
        match self {
            Kind::Correspondent => "Correspondent",
            Kind::DocumentType => "DocumentType",
            Kind::Tag => "Tag",
            Kind::StoragePath => "StoragePath",
        }
    }

    pub fn table(self) -> &'static str {
        match self {
            Kind::Correspondent => "documents_correspondent",
            Kind::DocumentType => "documents_documenttype",
            Kind::Tag => "documents_tag",
            Kind::StoragePath => "documents_storagepath",
        }
    }

    /// `object_type` value of `/api/bulk_edit_objects/`.
    pub fn from_object_type(s: &str) -> Option<Kind> {
        match s {
            "tags" => Some(Kind::Tag),
            "correspondents" => Some(Kind::Correspondent),
            "document_types" => Some(Kind::DocumentType),
            "storage_paths" => Some(Kind::StoragePath),
            _ => None,
        }
    }

    /// FK column on `documents_document` (not for tags, which use the M2M table).
    pub fn document_fk(self) -> Option<&'static str> {
        match self {
            Kind::Correspondent => Some("correspondent_id"),
            Kind::DocumentType => Some("document_type_id"),
            Kind::StoragePath => Some("storage_path_id"),
            Kind::Tag => None,
        }
    }

    /// `ordering_fields` of the ViewSet (without `last_correspondence`, handled separately).
    pub fn ordering_fields(self) -> &'static [&'static str] {
        match self {
            Kind::Correspondent => &["name", "matching_algorithm", "match", "document_count"],
            Kind::Tag => &["color", "name", "matching_algorithm", "match", "document_count"],
            Kind::DocumentType => &["name", "matching_algorithm", "match", "document_count"],
            Kind::StoragePath => &["name", "path", "matching_algorithm", "match", "document_count"],
        }
    }

    /// Char fields of the FilterSet (`name`, and `path` for storage paths).
    pub fn char_filter_fields(self) -> &'static [&'static str] {
        match self {
            Kind::StoragePath => &["name", "path"],
            _ => &["name"],
        }
    }

    /// Audit-logged with django-auditlog (`documents/models.py` bottom).
    pub fn audited(self) -> bool {
        !matches!(self, Kind::StoragePath)
    }

    /// Relations that Django's Collector handles when an object of this kind is deleted.
    /// The tag self-relation (`tn_parent`, CASCADE) is handled separately.
    pub fn relations(self) -> &'static [Relation] {
        use Relation::*;
        match self {
            Kind::Correspondent => &[
                SetNull { table: "documents_document", column: "correspondent_id" },
                SetNull { table: "documents_workflowtrigger", column: "filter_has_correspondent_id" },
                SetNull { table: "documents_workflowaction", column: "assign_correspondent_id" },
                SetNull { table: "paperless_mail_mailrule", column: "assign_correspondent_id" },
                M2m { table: "documents_workflowtrigger_filter_has_not_correspondents", column: "correspondent_id" },
                M2m { table: "documents_workflowtrigger_filter_has_any_correspondents", column: "correspondent_id" },
                M2m { table: "documents_workflowaction_remove_correspondents", column: "correspondent_id" },
            ],
            Kind::DocumentType => &[
                SetNull { table: "documents_document", column: "document_type_id" },
                SetNull { table: "documents_workflowtrigger", column: "filter_has_document_type_id" },
                SetNull { table: "documents_workflowaction", column: "assign_document_type_id" },
                SetNull { table: "paperless_mail_mailrule", column: "assign_document_type_id" },
                M2m { table: "documents_workflowtrigger_filter_has_any_document_types", column: "documenttype_id" },
                M2m { table: "documents_workflowtrigger_filter_has_not_document_types", column: "documenttype_id" },
                M2m { table: "documents_workflowaction_remove_document_types", column: "documenttype_id" },
            ],
            Kind::StoragePath => &[
                SetNull { table: "documents_document", column: "storage_path_id" },
                SetNull { table: "documents_workflowtrigger", column: "filter_has_storage_path_id" },
                SetNull { table: "documents_workflowaction", column: "assign_storage_path_id" },
                M2m { table: "documents_workflowtrigger_filter_has_any_storage_paths", column: "storagepath_id" },
                M2m { table: "documents_workflowtrigger_filter_has_not_storage_paths", column: "storagepath_id" },
                M2m { table: "documents_workflowaction_remove_storage_paths", column: "storagepath_id" },
            ],
            Kind::Tag => &[
                M2m { table: "documents_document_tags", column: "tag_id" },
                M2m { table: "documents_workflowtrigger_filter_has_tags", column: "tag_id" },
                M2m { table: "documents_workflowtrigger_filter_has_all_tags", column: "tag_id" },
                M2m { table: "documents_workflowtrigger_filter_has_not_tags", column: "tag_id" },
                M2m { table: "documents_workflowaction_assign_tags", column: "tag_id" },
                M2m { table: "documents_workflowaction_remove_tags", column: "tag_id" },
                M2m { table: "paperless_mail_mailrule_assign_tags", column: "tag_id" },
            ],
        }
    }
}

/// One row of an object table, as needed for serialization.
#[derive(Clone, Debug, Default)]
pub struct ObjectRow {
    pub id: i64,
    pub name: String,
    pub r#match: String,
    pub matching_algorithm: i16,
    pub is_insensitive: bool,
    pub owner_id: Option<i64>,
    /// `None` when not annotated (e.g. right after create).
    pub document_count: Option<i64>,
    /// Correspondents only; outer `None` = not annotated, inner `None` = SQL NULL.
    pub last_correspondence: Option<Option<chrono::NaiveDate>>,
    /// Storage paths only.
    pub path: Option<String>,
    pub tag: Option<TagExtra>,
}

/// Tag-specific columns. Hierarchy: only the parent edge (`tn_parent_id`), see [`crate::tagtree`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagExtra {
    pub color: String,
    pub is_inbox_tag: bool,
    pub tn_parent_id: Option<i64>,
}
