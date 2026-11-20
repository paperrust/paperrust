//! Tables, columns and serializer fields of the workflow models.
//! upstream: src/documents/models.py@v3.4.2 (WorkflowTrigger, WorkflowActionEmail,
//! WorkflowActionWebhook, WorkflowAction, Workflow) and src/documents/serialisers.py@v3.4.2
//! (WorkflowTriggerSerializer, WorkflowActionEmailSerializer, WorkflowActionWebhookSerializer,
//! WorkflowActionSerializer, WorkflowSerializer).

use crate::drf::{Choices, FieldSpec, Kind};
use serde_json::{Value, json};

pub const T_TRIGGER: &str = "documents_workflowtrigger";
pub const T_ACTION: &str = "documents_workflowaction";
pub const T_EMAIL: &str = "documents_workflowactionemail";
pub const T_WEBHOOK: &str = "documents_workflowactionwebhook";
pub const T_WORKFLOW: &str = "documents_workflow";
pub const T_WF_TRIGGERS: &str = "documents_workflow_triggers";
pub const T_WF_ACTIONS: &str = "documents_workflow_actions";
pub const T_WF_RUN: &str = "documents_workflowrun";

pub const T_TAG: &str = "documents_tag";
pub const T_CORR: &str = "documents_correspondent";
pub const T_DT: &str = "documents_documenttype";
pub const T_SP: &str = "documents_storagepath";
pub const T_CF: &str = "documents_customfield";
pub const T_USER: &str = "auth_user";
pub const T_GROUP: &str = "auth_group";
pub const T_MAILRULE: &str = "paperless_mail_mailrule";

/// Type of a column (for bind values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColTy {
    Int,
    Bool,
    Str,
    Json,
}

/// Ordering of the target objects of an m2m relation (Meta.ordering of the target model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum M2mOrder {
    Name,
    Created,
    Unordered,
}

#[derive(Debug, Clone, Copy)]
pub struct M2m {
    pub through: &'static str,
    pub own: &'static str,
    pub other: &'static str,
    pub target: &'static str,
    pub order: M2mOrder,
}

#[derive(Debug, Clone, Copy)]
pub enum Col {
    /// Column in the own table
    Scalar(&'static str, ColTy),
    M2m(M2m),
    /// Nested object (email/webhook) via FK column
    Nested(&'static str),
    /// Not stored / special handling (id)
    Pk,
}

#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub spec: FieldSpec,
    pub col: Col,
}

const fn f(spec: FieldSpec, col: Col) -> Field {
    Field { spec, col }
}

/// `WorkflowTriggerType`: 1 Consumption Started, 2 Document Added, 3 Document Updated,
/// 4 Scheduled, 5 Manual (PaperRust, after paperless-ngx dev #14704; value as there).
pub const TRIGGER_TYPES: &[i64] = &[1, 2, 3, 4, 5];
pub const SOURCES: &[i64] = &[1, 2, 3, 4];
pub const MATCHING: &[i64] = &[0, 1, 2, 3, 4, 5];
pub const SCHEDULE_DATE_FIELDS: &[&str] = &["added", "created", "modified", "custom_field"];
pub const ACTION_TYPES: &[i64] = &[1, 2, 3, 4, 5, 6, 7, 8];
pub const TYPE_CONSUMPTION: i64 = 1;
/// `WorkflowTriggerType.MANUAL`.
pub const TYPE_MANUAL: i64 = 5;
pub const ACTION_EMAIL: i64 = 3;
pub const ACTION_WEBHOOK: i64 = 4;
pub const ACTION_PASSWORD_REMOVAL: i64 = 5;
pub const ACTION_REMOTE_OCR: i64 = 7;
pub const ACTION_AI: i64 = 8;
pub const AI_FIELDS: &[&str] = &["title", "tags", "correspondent", "document_type", "storage_path", "created"];

const INT4: (i64, i64) = (-2_147_483_648, 2_147_483_647);

const fn tm2m(name_through: &'static str, other: &'static str, target: &'static str, order: M2mOrder) -> Col {
    Col::M2m(M2m { through: name_through, own: "workflowtrigger_id", other, target, order })
}
const fn am2m(name_through: &'static str, other: &'static str, target: &'static str, order: M2mOrder) -> Col {
    Col::M2m(M2m { through: name_through, own: "workflowaction_id", other, target, order })
}

fn default_sources() -> Value {
    json!([1, 2, 3])
}

const CHAR256_NULL_BLANK: Kind = Kind::Char { max_len: Some(256), allow_blank: true };
const TEXT_NULL_BLANK: Kind = Kind::Char { max_len: None, allow_blank: true };

/// `WorkflowTriggerSerializer.Meta.fields` in Reihenfolge.
pub const TRIGGER_FIELDS: &[Field] = &[
    f(FieldSpec::null("id", Kind::Int { min: None, max: None }), Col::Pk),
    f(
        FieldSpec {
            name: "sources",
            kind: Kind::MultiChoice(Choices::Ints(SOURCES)),
            required: false,
            allow_null: false,
            default: Some(default_sources),
        },
        Col::Scalar("sources", ColTy::Str),
    ),
    f(FieldSpec::req("type", Kind::Choice(Choices::Ints(TRIGGER_TYPES))), Col::Scalar("type", ColTy::Int)),
    f(FieldSpec::null("filter_path", CHAR256_NULL_BLANK), Col::Scalar("filter_path", ColTy::Str)),
    f(FieldSpec::null("filter_filename", CHAR256_NULL_BLANK), Col::Scalar("filter_filename", ColTy::Str)),
    f(FieldSpec::null("filter_mailrule", Kind::Pk(T_MAILRULE)), Col::Scalar("filter_mailrule_id", ColTy::Int)),
    f(
        FieldSpec::opt("matching_algorithm", Kind::Choice(Choices::Ints(MATCHING))),
        Col::Scalar("matching_algorithm", ColTy::Int),
    ),
    f(
        FieldSpec::opt("match", Kind::Char { max_len: Some(256), allow_blank: true }),
        Col::Scalar("match", ColTy::Str),
    ),
    f(FieldSpec::opt("is_insensitive", Kind::Bool), Col::Scalar("is_insensitive", ColTy::Bool)),
    f(
        FieldSpec::opt("filter_has_tags", Kind::ManyPk(T_TAG)),
        tm2m("documents_workflowtrigger_filter_has_tags", "tag_id", T_TAG, M2mOrder::Name),
    ),
    f(
        FieldSpec::opt("filter_has_all_tags", Kind::ManyPk(T_TAG)),
        tm2m("documents_workflowtrigger_filter_has_all_tags", "tag_id", T_TAG, M2mOrder::Name),
    ),
    f(
        FieldSpec::opt("filter_has_not_tags", Kind::ManyPk(T_TAG)),
        tm2m("documents_workflowtrigger_filter_has_not_tags", "tag_id", T_TAG, M2mOrder::Name),
    ),
    f(
        FieldSpec::null("filter_custom_field_query", TEXT_NULL_BLANK),
        Col::Scalar("filter_custom_field_query", ColTy::Str),
    ),
    f(
        FieldSpec::opt("filter_has_any_correspondents", Kind::ManyPk(T_CORR)),
        tm2m(
            "documents_workflowtrigger_filter_has_any_correspondents",
            "correspondent_id",
            T_CORR,
            M2mOrder::Name,
        ),
    ),
    f(
        FieldSpec::opt("filter_has_not_correspondents", Kind::ManyPk(T_CORR)),
        tm2m(
            "documents_workflowtrigger_filter_has_not_correspondents",
            "correspondent_id",
            T_CORR,
            M2mOrder::Name,
        ),
    ),
    f(
        FieldSpec::opt("filter_has_any_document_types", Kind::ManyPk(T_DT)),
        tm2m(
            "documents_workflowtrigger_filter_has_any_document_types",
            "documenttype_id",
            T_DT,
            M2mOrder::Name,
        ),
    ),
    f(
        FieldSpec::opt("filter_has_not_document_types", Kind::ManyPk(T_DT)),
        tm2m(
            "documents_workflowtrigger_filter_has_not_document_types",
            "documenttype_id",
            T_DT,
            M2mOrder::Name,
        ),
    ),
    f(
        FieldSpec::opt("filter_has_any_storage_paths", Kind::ManyPk(T_SP)),
        tm2m(
            "documents_workflowtrigger_filter_has_any_storage_paths",
            "storagepath_id",
            T_SP,
            M2mOrder::Name,
        ),
    ),
    f(
        FieldSpec::opt("filter_has_not_storage_paths", Kind::ManyPk(T_SP)),
        tm2m(
            "documents_workflowtrigger_filter_has_not_storage_paths",
            "storagepath_id",
            T_SP,
            M2mOrder::Name,
        ),
    ),
    f(
        FieldSpec::null("filter_has_correspondent", Kind::Pk(T_CORR)),
        Col::Scalar("filter_has_correspondent_id", ColTy::Int),
    ),
    f(
        FieldSpec::null("filter_has_document_type", Kind::Pk(T_DT)),
        Col::Scalar("filter_has_document_type_id", ColTy::Int),
    ),
    f(
        FieldSpec::null("filter_has_storage_path", Kind::Pk(T_SP)),
        Col::Scalar("filter_has_storage_path_id", ColTy::Int),
    ),
    f(
        FieldSpec::opt("schedule_offset_days", Kind::Int { min: Some(INT4.0), max: Some(INT4.1) }),
        Col::Scalar("schedule_offset_days", ColTy::Int),
    ),
    f(FieldSpec::opt("schedule_is_recurring", Kind::Bool), Col::Scalar("schedule_is_recurring", ColTy::Bool)),
    f(
        FieldSpec::opt("schedule_recurring_interval_days", Kind::Int { min: Some(1), max: Some(INT4.1) }),
        Col::Scalar("schedule_recurring_interval_days", ColTy::Int),
    ),
    f(
        FieldSpec::opt("schedule_date_field", Kind::Choice(Choices::Strs(SCHEDULE_DATE_FIELDS))),
        Col::Scalar("schedule_date_field", ColTy::Str),
    ),
    f(
        FieldSpec::null("schedule_date_custom_field", Kind::Pk(T_CF)),
        Col::Scalar("schedule_date_custom_field_id", ColTy::Int),
    ),
];

/// Model defaults of `WorkflowTrigger` (Django writes all columns on INSERT).
pub fn trigger_defaults() -> Vec<(&'static str, ColTy, Value)> {
    vec![
        ("type", ColTy::Int, json!(1)),
        ("sources", ColTy::Str, json!("1,2,3,4")),
        ("filter_path", ColTy::Str, Value::Null),
        ("filter_filename", ColTy::Str, Value::Null),
        ("filter_mailrule_id", ColTy::Int, Value::Null),
        ("match", ColTy::Str, json!("")),
        ("matching_algorithm", ColTy::Int, json!(0)),
        ("is_insensitive", ColTy::Bool, json!(true)),
        ("filter_has_document_type_id", ColTy::Int, Value::Null),
        ("filter_has_correspondent_id", ColTy::Int, Value::Null),
        ("filter_has_storage_path_id", ColTy::Int, Value::Null),
        ("filter_custom_field_query", ColTy::Str, Value::Null),
        ("schedule_offset_days", ColTy::Int, json!(0)),
        ("schedule_is_recurring", ColTy::Bool, json!(false)),
        ("schedule_recurring_interval_days", ColTy::Int, json!(1)),
        ("schedule_date_field", ColTy::Str, json!("added")),
        ("schedule_date_custom_field_id", ColTy::Int, Value::Null),
    ]
}

pub const EMAIL_FIELDS: &[Field] = &[
    f(FieldSpec::null("id", Kind::Int { min: None, max: None }), Col::Pk),
    f(
        FieldSpec::req("subject", Kind::Char { max_len: Some(256), allow_blank: false }),
        Col::Scalar("subject", ColTy::Str),
    ),
    f(FieldSpec::req("body", Kind::Char { max_len: None, allow_blank: false }), Col::Scalar("body", ColTy::Str)),
    f(FieldSpec::req("to", Kind::Char { max_len: None, allow_blank: false }), Col::Scalar("to", ColTy::Str)),
    f(FieldSpec::opt("include_document", Kind::Bool), Col::Scalar("include_document", ColTy::Bool)),
];

pub fn email_defaults() -> Vec<(&'static str, ColTy, Value)> {
    vec![
        ("subject", ColTy::Str, json!("")),
        ("body", ColTy::Str, json!("")),
        ("to", ColTy::Str, json!("")),
        ("include_document", ColTy::Bool, json!(false)),
    ]
}

pub const WEBHOOK_FIELDS: &[Field] = &[
    f(FieldSpec::null("id", Kind::Int { min: None, max: None }), Col::Pk),
    f(
        FieldSpec::req("url", Kind::Char { max_len: Some(256), allow_blank: false }),
        Col::Scalar("url", ColTy::Str),
    ),
    f(FieldSpec::opt("use_params", Kind::Bool), Col::Scalar("use_params", ColTy::Bool)),
    f(FieldSpec::opt("as_json", Kind::Bool), Col::Scalar("as_json", ColTy::Bool)),
    f(FieldSpec::null("params", Kind::Json), Col::Scalar("params", ColTy::Json)),
    f(FieldSpec::null("body", TEXT_NULL_BLANK), Col::Scalar("body", ColTy::Str)),
    f(FieldSpec::null("headers", Kind::Json), Col::Scalar("headers", ColTy::Json)),
    f(FieldSpec::opt("include_document", Kind::Bool), Col::Scalar("include_document", ColTy::Bool)),
];

pub fn webhook_defaults() -> Vec<(&'static str, ColTy, Value)> {
    vec![
        ("url", ColTy::Str, json!("")),
        ("use_params", ColTy::Bool, json!(true)),
        ("as_json", ColTy::Bool, json!(false)),
        ("params", ColTy::Json, Value::Null),
        ("body", ColTy::Str, Value::Null),
        ("headers", ColTy::Json, Value::Null),
        ("include_document", ColTy::Bool, json!(false)),
    ]
}

/// Placeholder for nested serializers (own validation in `serializers.rs`).
const NESTED: Kind = Kind::Json;

/// `WorkflowActionSerializer.Meta.fields` in Reihenfolge.
pub const ACTION_FIELDS: &[Field] = &[
    f(FieldSpec::null("id", Kind::Int { min: None, max: None }), Col::Pk),
    f(FieldSpec::opt("type", Kind::Choice(Choices::Ints(ACTION_TYPES))), Col::Scalar("type", ColTy::Int)),
    f(FieldSpec::null("assign_title", TEXT_NULL_BLANK), Col::Scalar("assign_title", ColTy::Str)),
    f(
        FieldSpec::opt("assign_tags", Kind::ManyPk(T_TAG)),
        am2m("documents_workflowaction_assign_tags", "tag_id", T_TAG, M2mOrder::Name),
    ),
    f(
        FieldSpec::null("assign_correspondent", Kind::Pk(T_CORR)),
        Col::Scalar("assign_correspondent_id", ColTy::Int),
    ),
    f(
        FieldSpec::null("assign_document_type", Kind::Pk(T_DT)),
        Col::Scalar("assign_document_type_id", ColTy::Int),
    ),
    f(
        FieldSpec::null("assign_storage_path", Kind::Pk(T_SP)),
        Col::Scalar("assign_storage_path_id", ColTy::Int),
    ),
    f(FieldSpec::null("assign_owner", Kind::Pk(T_USER)), Col::Scalar("assign_owner_id", ColTy::Int)),
    f(
        FieldSpec::opt("assign_view_users", Kind::ManyPk(T_USER)),
        am2m("documents_workflowaction_assign_view_users", "user_id", T_USER, M2mOrder::Unordered),
    ),
    f(
        FieldSpec::opt("assign_view_groups", Kind::ManyPk(T_GROUP)),
        am2m("documents_workflowaction_assign_view_groups", "group_id", T_GROUP, M2mOrder::Unordered),
    ),
    f(
        FieldSpec::opt("assign_change_users", Kind::ManyPk(T_USER)),
        am2m("documents_workflowaction_assign_change_users", "user_id", T_USER, M2mOrder::Unordered),
    ),
    f(
        FieldSpec::opt("assign_change_groups", Kind::ManyPk(T_GROUP)),
        am2m("documents_workflowaction_assign_change_groups", "group_id", T_GROUP, M2mOrder::Unordered),
    ),
    f(
        FieldSpec::opt("assign_custom_fields", Kind::ManyPk(T_CF)),
        am2m("documents_workflowaction_assign_custom_fields", "customfield_id", T_CF, M2mOrder::Created),
    ),
    f(
        FieldSpec::null("assign_custom_fields_values", Kind::Json),
        Col::Scalar("assign_custom_fields_values", ColTy::Json),
    ),
    f(FieldSpec::opt("remove_all_tags", Kind::Bool), Col::Scalar("remove_all_tags", ColTy::Bool)),
    f(
        FieldSpec::opt("remove_tags", Kind::ManyPk(T_TAG)),
        am2m("documents_workflowaction_remove_tags", "tag_id", T_TAG, M2mOrder::Name),
    ),
    f(
        FieldSpec::opt("remove_all_correspondents", Kind::Bool),
        Col::Scalar("remove_all_correspondents", ColTy::Bool),
    ),
    f(
        FieldSpec::opt("remove_correspondents", Kind::ManyPk(T_CORR)),
        am2m("documents_workflowaction_remove_correspondents", "correspondent_id", T_CORR, M2mOrder::Name),
    ),
    f(
        FieldSpec::opt("remove_all_document_types", Kind::Bool),
        Col::Scalar("remove_all_document_types", ColTy::Bool),
    ),
    f(
        FieldSpec::opt("remove_document_types", Kind::ManyPk(T_DT)),
        am2m("documents_workflowaction_remove_document_types", "documenttype_id", T_DT, M2mOrder::Name),
    ),
    f(
        FieldSpec::opt("remove_all_storage_paths", Kind::Bool),
        Col::Scalar("remove_all_storage_paths", ColTy::Bool),
    ),
    f(
        FieldSpec::opt("remove_storage_paths", Kind::ManyPk(T_SP)),
        am2m("documents_workflowaction_remove_storage_paths", "storagepath_id", T_SP, M2mOrder::Name),
    ),
    f(
        FieldSpec::opt("remove_custom_fields", Kind::ManyPk(T_CF)),
        am2m("documents_workflowaction_remove_custom_fields", "customfield_id", T_CF, M2mOrder::Created),
    ),
    f(
        FieldSpec::opt("remove_all_custom_fields", Kind::Bool),
        Col::Scalar("remove_all_custom_fields", ColTy::Bool),
    ),
    f(FieldSpec::opt("remove_all_owners", Kind::Bool), Col::Scalar("remove_all_owners", ColTy::Bool)),
    f(
        FieldSpec::opt("remove_owners", Kind::ManyPk(T_USER)),
        am2m("documents_workflowaction_remove_owners", "user_id", T_USER, M2mOrder::Unordered),
    ),
    f(
        FieldSpec::opt("remove_all_permissions", Kind::Bool),
        Col::Scalar("remove_all_permissions", ColTy::Bool),
    ),
    f(
        FieldSpec::opt("remove_view_users", Kind::ManyPk(T_USER)),
        am2m("documents_workflowaction_remove_view_users", "user_id", T_USER, M2mOrder::Unordered),
    ),
    f(
        FieldSpec::opt("remove_view_groups", Kind::ManyPk(T_GROUP)),
        am2m("documents_workflowaction_remove_view_groups", "group_id", T_GROUP, M2mOrder::Unordered),
    ),
    f(
        FieldSpec::opt("remove_change_users", Kind::ManyPk(T_USER)),
        am2m("documents_workflowaction_remove_change_users", "user_id", T_USER, M2mOrder::Unordered),
    ),
    f(
        FieldSpec::opt("remove_change_groups", Kind::ManyPk(T_GROUP)),
        am2m("documents_workflowaction_remove_change_groups", "group_id", T_GROUP, M2mOrder::Unordered),
    ),
    f(FieldSpec::null("email", NESTED), Col::Nested("email_id")),
    f(FieldSpec::null("webhook", NESTED), Col::Nested("webhook_id")),
    f(FieldSpec::null("passwords", Kind::Json), Col::Scalar("passwords", ColTy::Json)),
    f(FieldSpec::null("ai_suggestion_fields", Kind::Json), Col::Scalar("ai_suggestion_fields", ColTy::Json)),
    f(FieldSpec::opt("ai_create_missing", Kind::Bool), Col::Scalar("ai_create_missing", ColTy::Bool)),
    f(FieldSpec::opt("ai_overwrite_existing", Kind::Bool), Col::Scalar("ai_overwrite_existing", ColTy::Bool)),
];

pub fn action_defaults() -> Vec<(&'static str, ColTy, Value)> {
    vec![
        ("type", ColTy::Int, json!(1)),
        ("order", ColTy::Int, json!(0)),
        ("assign_title", ColTy::Str, Value::Null),
        ("assign_document_type_id", ColTy::Int, Value::Null),
        ("assign_correspondent_id", ColTy::Int, Value::Null),
        ("assign_storage_path_id", ColTy::Int, Value::Null),
        ("assign_owner_id", ColTy::Int, Value::Null),
        ("assign_custom_fields_values", ColTy::Json, json!({})),
        ("remove_all_tags", ColTy::Bool, json!(false)),
        ("remove_all_document_types", ColTy::Bool, json!(false)),
        ("remove_all_correspondents", ColTy::Bool, json!(false)),
        ("remove_all_storage_paths", ColTy::Bool, json!(false)),
        ("remove_all_owners", ColTy::Bool, json!(false)),
        ("remove_all_permissions", ColTy::Bool, json!(false)),
        ("remove_all_custom_fields", ColTy::Bool, json!(false)),
        ("email_id", ColTy::Int, Value::Null),
        ("webhook_id", ColTy::Int, Value::Null),
        ("passwords", ColTy::Json, Value::Null),
        ("ai_suggestion_fields", ColTy::Json, Value::Null),
        ("ai_create_missing", ColTy::Bool, json!(false)),
        ("ai_overwrite_existing", ColTy::Bool, json!(false)),
    ]
}

/// `WorkflowSerializer`: scalar fields (triggers/actions separate).
pub const WORKFLOW_FIELDS: &[Field] = &[
    f(
        FieldSpec::req("name", Kind::Char { max_len: Some(256), allow_blank: false }),
        Col::Scalar("name", ColTy::Str),
    ),
    f(FieldSpec::opt("order", Kind::Int { min: None, max: None }), Col::Scalar("order", ColTy::Int)),
    f(FieldSpec::opt("enabled", Kind::Bool), Col::Scalar("enabled", ColTy::Bool)),
];

pub fn workflow_defaults() -> Vec<(&'static str, ColTy, Value)> {
    vec![("name", ColTy::Str, json!("")), ("order", ColTy::Int, json!(0)), ("enabled", ColTy::Bool, json!(true))]
}

/// m2m fields of a field list.
pub fn m2m_fields(fields: &'static [Field]) -> impl Iterator<Item = (&'static str, M2m)> {
    fields.iter().filter_map(|f| match f.col {
        Col::M2m(m) => Some((f.spec.name, m)),
        _ => None,
    })
}
