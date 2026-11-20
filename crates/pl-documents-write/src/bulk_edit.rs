// upstream: src/documents/bulk_edit.py@v3.4.2 (set_correspondent, set_document_type, set_storage_path,
//           add_tag, remove_tag, modify_tags, set_permissions, reprocess)
//           src/documents/serialisers.py@v3.4.2 (BulkEditSerializer, SetPermissionsSerializer,
//           SetPermissionsMixin.validate_set_permissions)
//           run_workflow: own implementation after paperless-ngx dev 3dbeca603 (#14704,
//           BulkEditSerializer._validate_parameters_run_workflow, bulk_edit.run_workflow)
//
//! Validation and execution of the ported bulk_edit methods. All other methods go
//! unchanged to Django (see contract).

use std::collections::{BTreeSet, HashSet};

use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use crate::audit::Snapshot;
use crate::enqueue::{BULK_UPDATE_DOCUMENTS, RUN_WORKFLOWS_FOR_DOCUMENTS, Task, TriggerSource, UPDATE_CONTENT_MAYBE_ARCHIVE};
use crate::drf::{self, django_int_lookup, py_truthy};
use crate::error::ApiError;
use crate::perms::{GranteesUpdate, OperationNeeds, PermissionsSpec, count_existing, set_permissions_for_documents};

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// All choice values of `BulkEditSerializer.method`.
pub const METHOD_CHOICES: &[&str] = &[
    "set_correspondent",
    "set_document_type",
    "set_storage_path",
    "add_tag",
    "remove_tag",
    "modify_tags",
    "modify_custom_fields",
    "set_permissions",
    // PaperRust: manual workflow run (like paperless-ngx dev #14704)
    "run_workflow",
    "delete",
    "reprocess",
    "rotate",
    "merge",
    "edit_pdf",
    "remove_password",
    "split",
    "delete_pages",
];

/// Methods executed in Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    SetCorrespondent,
    SetDocumentType,
    SetStoragePath,
    AddTag,
    RemoveTag,
    ModifyTags,
    SetPermissions,
    Reprocess,
    ModifyCustomFields,
    Delete,
    RunWorkflow,
}

impl Method {
    pub fn from_choice(s: &str) -> Option<Self> {
        Some(match s {
            "set_correspondent" => Method::SetCorrespondent,
            "set_document_type" => Method::SetDocumentType,
            "set_storage_path" => Method::SetStoragePath,
            "add_tag" => Method::AddTag,
            "remove_tag" => Method::RemoveTag,
            "modify_tags" => Method::ModifyTags,
            "set_permissions" => Method::SetPermissions,
            "modify_custom_fields" => Method::ModifyCustomFields,
            "delete" => Method::Delete,
            "reprocess" => Method::Reprocess,
            "run_workflow" => Method::RunWorkflow,
            _ => return None,
        })
    }

    /// `method.__name__`
    pub fn name(self) -> &'static str {
        match self {
            Method::SetCorrespondent => "set_correspondent",
            Method::SetDocumentType => "set_document_type",
            Method::SetStoragePath => "set_storage_path",
            Method::AddTag => "add_tag",
            Method::RemoveTag => "remove_tag",
            Method::ModifyTags => "modify_tags",
            Method::SetPermissions => "set_permissions",
            Method::ModifyCustomFields => "modify_custom_fields",
            Method::Delete => "delete",
            Method::Reprocess => "reprocess",
            Method::RunWorkflow => "run_workflow",
        }
    }

    /// Allowed keyword arguments of the Python function (otherwise `TypeError` → 400).
    fn accepted_kwargs(self) -> &'static [&'static str] {
        match self {
            Method::SetCorrespondent => &["correspondent"],
            Method::SetDocumentType => &["document_type"],
            Method::SetStoragePath => &["storage_path"],
            Method::AddTag | Method::RemoveTag => &["tag"],
            Method::ModifyTags => &["add_tags", "remove_tags"],
            Method::SetPermissions => &["set_permissions", "owner", "merge"],
            Method::ModifyCustomFields => &["add_custom_fields", "remove_custom_fields"],
            Method::Delete => &[],
            Method::Reprocess => &["remote_ocr", "ai_ocr"],
            Method::RunWorkflow => &["workflow", "filter_documents"],
        }
    }

    /// `MODIFIED_FIELD_BY_METHOD`
    pub fn snapshot(self) -> Option<Snapshot> {
        match self {
            Method::SetCorrespondent => Some(Snapshot::Column("correspondent_id")),
            Method::SetDocumentType => Some(Snapshot::Column("document_type_id")),
            Method::SetStoragePath => Some(Snapshot::Column("storage_path_id")),
            Method::AddTag | Method::RemoveTag | Method::ModifyTags => Some(Snapshot::Tags),
            Method::Reprocess => Some(Snapshot::Column("checksum")),
            Method::SetPermissions => None,
            Method::ModifyCustomFields => Some(Snapshot::CustomFields),
            Method::Delete => Some(Snapshot::Column("deleted_at")),
            // Workflow runs later in the worker; its saves write their own audit log.
            Method::RunWorkflow => None,
        }
    }

    /// Pure metadata changes that may run in the background for large selections
    /// (upstream `dev` 60a35edee: `QUEUED_OPERATIONS`), see [`crate::bulk_task`].
    pub fn queueable(self) -> bool {
        matches!(
            self,
            Method::SetCorrespondent
                | Method::SetDocumentType
                | Method::SetStoragePath
                | Method::AddTag
                | Method::RemoveTag
                | Method::ModifyTags
                | Method::ModifyCustomFields
                | Method::SetPermissions
        )
    }

    /// Additional permission conditions (`_has_document_permissions`).
    pub fn needs(self) -> OperationNeeds {
        match self {
            Method::Delete => OperationNeeds { ownership: true, delete_document: true, ..Default::default() },
            // Actions change owner/permissions and can move to the trash → like
            // set_permissions only for owners (upstream: run_workflow in the owner list).
            _ => OperationNeeds {
                ownership: matches!(self, Method::SetPermissions | Method::RunWorkflow),
                ..Default::default()
            },
        }
    }
}

/// Validated parameters.
#[derive(Clone, Debug)]
pub enum Params {
    Fk { column: &'static str, value: Option<i64> },
    Tag { id: i64, add: bool },
    ModifyTags { add: Vec<i64>, remove: Vec<i64> },
    SetPermissions { spec: PermissionsSpec, owner: Option<i64>, merge: bool },
    /// `ai_ocr`: PaperRust "AI text recognition" (only passed on to the task when set).
    Reprocess { remote_ocr: bool, ai_ocr: bool },
    /// `(field_id, value)` in input order; list (old API) → value `None`.
    ModifyCustomFields { add: Vec<(i64, Value)>, remove: Vec<i64> },
    Delete,
    /// Workflow id (exists, active, has trigger `MANUAL`) and whether the trigger filters apply.
    RunWorkflow { workflow: i64, filter_documents: bool },
}

/// `_validate_tag_id_list` (isinstance int; JSON bool counts as int in Python).
fn strict_int_list(v: &Value, name: &str) -> Result<Vec<i64>, String> {
    let Value::Array(items) = v else {
        return Err(format!("{name} must be a list"));
    };
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        match it {
            Value::Bool(b) => out.push(i64::from(*b)),
            Value::Number(n) if n.is_i64() || n.is_u64() => match n.as_i64() {
                Some(i) => out.push(i),
                // Python integer > i64: cannot match any row
                None => out.push(i64::MIN),
            },
            _ => return Err(format!("{name} must be a list of integers")),
        }
    }
    Ok(out)
}

async fn count_ids(conn: &mut PgConnection, table: &str, ids: &[i64]) -> Result<usize, ApiError> {
    count_existing(conn, table, ids).await
}

async fn exists(conn: &mut PgConnection, table: &str, id: i64) -> Result<bool, ApiError> {
    Ok(count_ids(conn, table, &[id]).await? == 1)
}

/// Result of parameter validation: `Ok(params)` or `non_field_errors`/dict error.
pub enum Validated {
    Ok(Params),
    Invalid(Value),
    /// DRF `PermissionDenied(detail)` from validation → 403 `{"detail": …}`.
    Forbidden(&'static str),
}

fn nf(m: impl Into<String>) -> Result<Validated, ApiError> {
    Ok(Validated::Invalid(drf::non_field(m)))
}

/// `Model.objects.get(id=x)` for validation: Some(id) if present, None on DoesNotExist,
/// Err on type error (not caught upstream → 500).
async fn get_by_id(conn: &mut PgConnection, table: &str, v: &Value) -> Result<Option<i64>, ApiError> {
    match django_int_lookup(v) {
        Err(_) => Err(ApiError::Uncaught(format!("invalid id lookup on {table}"))),
        Ok(None) => Ok(None),
        Ok(Some(id)) => Ok(if exists(conn, table, id).await? { Some(id) } else { None }),
    }
}

/// `BulkEditSerializer.validate` – method-specific part (after selection and all=true check).
pub async fn validate_parameters(
    conn: &mut PgConnection,
    user: &crate::user::CurrentUser,
    method: Method,
    parameters: &mut Map<String, Value>,
) -> Result<Validated, ApiError> {
    // source_mode
    if let Some(sm) = parameters.get("source_mode") {
        // `SourceModeChoices.__dict__.values()` also contains "__module__" and `__doc__` (None).
        let ok = matches!(sm, Value::Null)
            || matches!(sm.as_str(), Some("latest_version" | "explicit_selection" | "documents.bulk_edit"));
        if !ok {
            return nf("Invalid source_mode");
        }
    }

    let fk = |param: &'static str, table: &'static str, column: &'static str, missing: &'static str, unknown: &'static str| {
        (param, table, column, missing, unknown)
    };
    let fk_spec = match method {
        Method::SetCorrespondent => Some(fk("correspondent", "documents_correspondent", "correspondent_id", "correspondent not specified", "Correspondent does not exist")),
        Method::SetDocumentType => Some(fk("document_type", "documents_documenttype", "document_type_id", "document_type not specified", "Document type does not exist")),
        Method::SetStoragePath => Some(fk("storage_path", "documents_storagepath", "storage_path_id", "storage path not specified", "Storage path does not exist")),
        _ => None,
    };
    if let Some((param, table, column, missing, unknown)) = fk_spec {
        let Some(v) = parameters.get(param) else { return nf(missing) };
        if v.is_null() {
            return Ok(Validated::Ok(Params::Fk { column, value: None }));
        }
        return match get_by_id(conn, table, v).await? {
            None => nf(unknown),
            // `if correspondent:` – 0 is falsy; if no row with id 0 exists, that is "does not exist"
            // above anyway.
            Some(id) => Ok(Validated::Ok(Params::Fk { column, value: if py_truthy(v) { Some(id) } else { None } })),
        };
    }

    match method {
        Method::AddTag | Method::RemoveTag => {
            let Some(v) = parameters.get("tag") else { return nf("tag not specified") };
            match get_by_id(conn, "documents_tag", v).await? {
                None => nf("Tag does not exist"),
                Some(id) => Ok(Validated::Ok(Params::Tag { id, add: method == Method::AddTag })),
            }
        }
        Method::ModifyTags => {
            let mut lists = Vec::new();
            for name in ["add_tags", "remove_tags"] {
                let Some(v) = parameters.get(name) else { return nf(format!("{name} not specified")) };
                let ids = match strict_int_list(v, name) {
                    Ok(ids) => ids,
                    Err(m) => return nf(m),
                };
                if count_ids(conn, "documents_tag", &ids).await? != ids.len() {
                    return nf(format!("Some tags in {name} don't exist or were specified twice."));
                }
                lists.push(ids);
            }
            let remove = lists.pop().unwrap_or_default();
            let add = lists.pop().unwrap_or_default();
            Ok(Validated::Ok(Params::ModifyTags { add, remove }))
        }
        Method::SetPermissions => validate_set_permissions(conn, parameters).await,
        Method::ModifyCustomFields => validate_modify_custom_fields(conn, user, parameters).await,
        Method::Delete => Ok(Validated::Ok(Params::Delete)),
        Method::RunWorkflow => validate_run_workflow(conn, user, parameters).await,
        Method::Reprocess => {
            let remote_ocr = match parameters.get("remote_ocr") {
                None => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return nf("remote_ocr must be a boolean"),
            };
            let ai_ocr = match parameters.get("ai_ocr") {
                None => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return nf("ai_ocr must be a boolean"),
            };
            Ok(Validated::Ok(Params::Reprocess { remote_ocr, ai_ocr }))
        }
        Method::SetCorrespondent | Method::SetDocumentType | Method::SetStoragePath => {
            Err(ApiError::Uncaught("unreachable fk branch".into()))
        }
    }
}

/// Type `MANUAL` of `WorkflowTrigger.type` (see pl-matching `TRIGGER_MANUAL`).
pub const TRIGGER_TYPE_MANUAL: i16 = 5;

/// Check order as paperless-ngx dev `_validate_parameters_run_workflow`:
/// missing → 400; unknown/not an int → 400; without `view_workflow` → 403; disabled → 400;
/// without manual trigger → 400; `filter_documents` not a bool → 400 (missing → `true`).
async fn validate_run_workflow(
    conn: &mut PgConnection,
    user: &crate::user::CurrentUser,
    parameters: &mut Map<String, Value>,
) -> Result<Validated, ApiError> {
    let Some(v) = parameters.get("workflow") else { return nf("workflow not specified") };
    // Workflow.objects.get(id=…): DoesNotExist, ValueError and TypeError → "does not exist"
    let id = match django_int_lookup(v) {
        Ok(Some(id)) => i32::try_from(id).ok(),
        Ok(None) | Err(_) => None,
    };
    let row: Option<(bool, bool)> = match id {
        None => None,
        Some(id) => {
            sqlx::query_as(
                "SELECT w.enabled, EXISTS (SELECT 1 FROM documents_workflow_triggers wt \
                 JOIN documents_workflowtrigger t ON t.id = wt.workflowtrigger_id \
                 WHERE wt.workflow_id = w.id AND t.type = $2) FROM documents_workflow w WHERE w.id = $1",
            )
            .bind(id)
            .bind(TRIGGER_TYPE_MANUAL)
            .fetch_optional(&mut *conn)
            .await?
        }
    };
    let (Some(id), Some((enabled, has_manual))) = (id, row) else { return nf("Workflow does not exist") };
    if !crate::user::has_perm(user, "view_workflow") {
        return Ok(Validated::Forbidden("Insufficient permissions to run this workflow"));
    }
    if !enabled {
        return nf("Workflow is not enabled");
    }
    if !has_manual {
        return nf("Workflow does not have a manual trigger");
    }
    let filter_documents = match parameters.get("filter_documents") {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return nf("filter_documents must be a boolean"),
    };
    parameters.insert("filter_documents".into(), Value::Bool(filter_documents));
    Ok(Validated::Ok(Params::RunWorkflow { workflow: i64::from(id), filter_documents }))
}

async fn cf_id_list_or_dict(conn: &mut PgConnection, v: &Value, name: &str) -> Result<Result<Vec<i64>, String>, ApiError> {
    let ids: Vec<i64> = match v {
        Value::Object(m) => {
            let mut out = Vec::new();
            for k in m.keys() {
                match drf::py_int_from_str(k) {
                    Some(i) => out.push(i),
                    None => {
                        return Ok(Err(format!(
                            "{name} must be a list of integers or a dict of id:value pairs, see the log for details"
                        )));
                    }
                }
            }
            out
        }
        Value::Array(items) => {
            let mut out = Vec::new();
            for it in items {
                match it {
                    Value::Bool(b) => out.push(i64::from(*b)),
                    Value::Number(n) if n.is_i64() || n.is_u64() => out.push(n.as_i64().unwrap_or(i64::MIN)),
                    _ => return Ok(Err(format!("{name} must be a list of integers or a dict of id:value pairs"))),
                }
            }
            out
        }
        _ => return Ok(Err(format!("{name} must be a list of integers or a dict of id:value pairs"))),
    };
    if count_ids(conn, "documents_customfield", &ids).await? != ids.len() {
        return Ok(Err(format!("Some custom fields in {name} don't exist or were specified twice.")));
    }
    Ok(Ok(ids))
}

/// `_validate_custom_field_id_list_or_dict` + `_validate_custom_field_values`.
async fn validate_modify_custom_fields(
    conn: &mut PgConnection,
    user: &crate::user::CurrentUser,
    parameters: &mut Map<String, Value>,
) -> Result<Validated, ApiError> {
    let Some(add_v) = parameters.get("add_custom_fields").cloned() else {
        return nf("add_custom_fields not specified");
    };
    let add_ids = match cf_id_list_or_dict(conn, &add_v, "add_custom_fields").await? {
        Ok(i) => i,
        Err(m) => return nf(m),
    };
    let add: Vec<(i64, Value)> = match &add_v {
        Value::Object(m) => {
            let mut validated = Vec::new();
            let mut errors = Map::new();
            for (value, field_id) in m.values().zip(add_ids.iter()) {
                let item = json!({"field": field_id, "value": value});
                match crate::serializer::validate_cfi(conn, user, &item, false).await? {
                    Ok(it) => validated.push((*field_id, it.value.unwrap_or(Value::Null))),
                    Err(e) => {
                        errors.insert(field_id.to_string(), e);
                    }
                }
            }
            if !errors.is_empty() {
                let mut m = Map::new();
                m.insert("add_custom_fields".into(), Value::Object(errors));
                return Ok(Validated::Invalid(Value::Object(m)));
            }
            validated
        }
        _ => add_ids.iter().map(|i| (*i, Value::Null)).collect(),
    };
    let Some(rem_v) = parameters.get("remove_custom_fields").cloned() else {
        return nf("remove_custom_fields not specified");
    };
    let remove = match cf_id_list_or_dict(conn, &rem_v, "remove_custom_fields").await? {
        Ok(i) => i,
        Err(m) => return nf(m),
    };
    Ok(Validated::Ok(Params::ModifyCustomFields { add, remove }))
}

/// Context of the methods with model side effects (auditlog, files).
pub struct ExecCtx {
    pub log: crate::auditlog::LogCtx,
    pub media: crate::files::MediaCfg,
}

/// `users`/`groups` of an action: outer `None` = key missing, inner `None` = `null`.
type ParsedIds = Option<Option<Vec<i64>>>;
type ParsedPermissions = Vec<(&'static str, ParsedIds, ParsedIds)>;

/// `SetPermissionsSerializer().run_validation(x)` → Ok(per action (users, groups) as raw values) or error.
fn run_set_permissions_serializer(v: &Value) -> Result<ParsedPermissions, Value> {
    let Value::Object(map) = v else {
        return Err(drf::non_field(format!(
            "Invalid data. Expected a dictionary, but got {}.",
            drf::py_type_name(v)
        )));
    };
    let mut unknown: Vec<&String> = map.keys().filter(|k| *k != "view" && *k != "change").collect();
    if !unknown.is_empty() {
        unknown.sort();
        let mut err = Map::new();
        for k in unknown {
            err.insert(k.clone(), json!(["Unknown permission action."]));
        }
        return Err(Value::Object(err));
    }
    let mut errors = Map::new();
    let mut out = Vec::new();
    for action in ["view", "change"] {
        let Some(entry) = map.get(action) else { continue };
        let Value::Object(entry) = entry else {
            if entry.is_null() {
                errors.insert(action.into(), drf::msg(drf::NULL));
            } else {
                errors.insert(
                    action.into(),
                    drf::non_field(format!("Invalid data. Expected a dictionary, but got {}.", drf::py_type_name(entry))),
                );
            }
            continue;
        };
        let mut entry_errors = Map::new();
        let mut parsed: [ParsedIds; 2] = [None, None];
        for (idx, key) in ["users", "groups"].iter().enumerate() {
            match entry.get(*key) {
                None => {}
                Some(Value::Null) => parsed[idx] = Some(None), // allow_null=True
                Some(v) => match drf::int_list(v) {
                    Ok(ids) => parsed[idx] = Some(Some(ids)),
                    Err(e) => {
                        entry_errors.insert((*key).into(), e);
                    }
                },
            }
        }
        if entry_errors.is_empty() {
            let [users, groups] = parsed;
            out.push((action, users, groups));
        } else {
            errors.insert(action.into(), Value::Object(entry_errors));
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(Value::Object(errors)) }
}

async fn validate_set_permissions(conn: &mut PgConnection, parameters: &mut Map<String, Value>) -> Result<Validated, ApiError> {
    let Some(raw) = parameters.get("set_permissions") else {
        return nf("set_permissions not specified");
    };
    // Without a value both actions remain with empty entries (no-op for guardian).
    let mut spec: PermissionsSpec = vec![("view", GranteesUpdate::default()), ("change", GranteesUpdate::default())];
    if !raw.is_null() {
        let parsed = match run_set_permissions_serializer(raw) {
            Ok(p) => p,
            Err(e) => return Ok(Validated::Invalid(e)),
        };
        spec.clear();
        for (action, users, groups) in parsed {
            let mut entry = GranteesUpdate::default();
            if let Some(users) = users {
                let users = users.unwrap_or_default();
                if count_existing(conn, "auth_user", &users).await? != users.len() {
                    return nf("Some users in don't exist or were specified twice.");
                }
                entry.users = Some(users);
            }
            if let Some(groups) = groups {
                let groups = groups.unwrap_or_default();
                if count_existing(conn, "auth_group", &groups).await? != groups.len() {
                    return nf("Some groups in don't exist or were specified twice.");
                }
                entry.groups = Some(groups);
            }
            spec.push((action, entry));
        }
    }
    let owner = match parameters.get("owner") {
        None | Some(Value::Null) => None,
        Some(v) => {
            // PrimaryKeyRelatedField(queryset=User.objects.all()).run_validation(owner)
            let id = match v {
                Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
                other => django_int_lookup(other).ok().flatten(),
            };
            match id {
                Some(id) if exists(conn, "auth_user", id).await? => Some(id),
                _ => return nf("Specified owner cannot be found"),
            }
        }
    };
    let merge = parameters.get("merge").map(py_truthy).unwrap_or(false);
    Ok(Validated::Ok(Params::SetPermissions { spec, owner, merge }))
}

/// Checks whether `method(documents, **parameters)` is callable with these keys.
pub fn kwargs_accepted(method: Method, parameters: &Map<String, Value>) -> bool {
    let accepted = method.accepted_kwargs();
    parameters.keys().all(|k| accepted.contains(&k.as_str()))
}

/// (id, ancestors root→parent, descendants) per existing tag from `ids` (order as
/// `ids`, without duplicates). Tree via the parent edge (upstream #14612, `pl_consumer::db`).
async fn tag_tree(conn: &mut PgConnection, ids: &[i64]) -> Result<Vec<(i64, Vec<i64>, Vec<i64>)>, ApiError> {
    let mut out = Vec::new();
    for id in ids {
        if out.iter().any(|(x, _, _)| x == id) {
            continue;
        }
        if let Some(anc) = pl_consumer::db::tag_ancestors(conn, *id).await? {
            let desc = pl_consumer::db::tag_descendants(conn, *id).await?;
            out.push((*id, anc, desc));
        }
    }
    Ok(out)
}

async fn existing_doc_ids(conn: &mut PgConnection, ids: &[i64]) -> Result<Vec<i64>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let q = Query::select()
        .column(a("id"))
        .from(a("documents_document"))
        .and_where(Expr::col(a("id")).is_in(i32s(ids.iter().copied())))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows: Vec<(i32,)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
    Ok(rows.into_iter().map(|(i,)| i64::from(i)).collect())
}

/// Create assignments – via the shared building block of the consumer (same SQL text, same
/// parameter types; FINDINGS B3).
async fn insert_tag_rows(conn: &mut PgConnection, pairs: &[(i64, i64)], ignore_conflicts: bool) -> Result<(), ApiError> {
    Ok(pl_consumer::db::insert_document_tags(conn, pairs, ignore_conflicts).await?)
}

/// Bind ids as `integer` (column type; same types as all other paths, FINDINGS B3). ids
/// outside int4 cannot exist.
fn i32s(ids: impl IntoIterator<Item = i64>) -> Vec<i32> {
    ids.into_iter().filter_map(|i| i32::try_from(i).ok()).collect()
}

/// Single id as `integer` (outside int4: matches no row).
fn i32v(id: i64) -> i32 {
    i32::try_from(id).unwrap_or(i32::MIN)
}

/// Does the method change document rows (directly or via triggers of the assignment tables)?
fn writes_documents(params: &Params) -> bool {
    !matches!(params, Params::Reprocess { .. } | Params::RunWorkflow { .. })
}

/// Before every writing method lock all selected document rows in id order
/// (FINDINGS B4/B5): concurrent bulk changes of the same documents then wait for each other
/// instead of deadlocking with the sorting sync trigger or the follow-up work; the second then reads
/// the committed state (no duplicate creation of the same assignment).
pub async fn lock_documents(conn: &mut PgConnection, doc_ids: &[i64], params: &Params) -> Result<(), ApiError> {
    if writes_documents(params) {
        pl_consumer::db::lock_documents_sorted(conn, doc_ids).await?;
    }
    Ok(())
}

fn bulk_update(ids: Vec<i64>) -> Task {
    Task::new(BULK_UPDATE_DOCUMENTS, json!({"document_ids": ids}), TriggerSource::System)
}

/// Execute method (inside the transaction). Returns the Celery tasks to be sent.
pub async fn execute(conn: &mut PgConnection, doc_ids: &[i64], params: &Params, ex: &ExecCtx) -> Result<Vec<Task>, ApiError> {
    lock_documents(conn, doc_ids, params).await?;
    match params {
        Params::ModifyCustomFields { add, remove } => {
            // Document.objects.filter(id__in=doc_ids) (order -created)
            let ids32: Vec<i32> = doc_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
            let affected: Vec<i64> = sqlx::query_scalar::<_, i32>(
                "SELECT id FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY created DESC, id ASC",
            )
            .bind(&ids32)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(i64::from)
            .collect();
            for (field_id, value) in add {
                let Some(field) = crate::cfi::field_def(conn, *field_id).await? else {
                    return Err(ApiError::Uncaught("KeyError: custom field".into()));
                };
                let is_doclink = field.data_type == "documentlink";
                let targets: Vec<i64> =
                    value.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                for doc_id in &affected {
                    if is_doclink && drf::py_truthy(value) && targets.contains(doc_id) {
                        continue;
                    }
                    crate::cfi::update_or_create(conn, &ex.log, &ex.media, *doc_id, &field, value).await?;
                    if is_doclink {
                        crate::cfi::reflect_doclinks(conn, &ex.log, &ex.media, *doc_id, &field, &targets).await?;
                    }
                }
            }
            if !remove.is_empty() && !affected.is_empty() {
                let aff32: Vec<i32> = affected.iter().map(|i| *i as i32).collect();
                let rem32: Vec<i32> = remove.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
                let rows: Vec<(i32, i32)> = sqlx::query_as(
                    "SELECT i.id, i.document_id FROM documents_customfieldinstance i JOIN documents_customfield f ON f.id = i.field_id \
                     WHERE i.document_id = ANY($1) AND i.field_id = ANY($2) AND f.data_type = 'documentlink' \
                     AND i.value_document_ids IS NOT NULL AND i.deleted_at IS NULL ORDER BY i.created, i.id",
                )
                .bind(&aff32)
                .bind(&rem32)
                .fetch_all(&mut *conn)
                .await?;
                for (cid, did) in rows {
                    let Some(c) = crate::cfi::load(conn, i64::from(cid)).await? else { continue };
                    let Some(field) = crate::cfi::field_def(conn, c.field_id).await? else { continue };
                    let targets: Vec<i64> = c
                        .value_document_ids
                        .as_ref()
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_i64).collect())
                        .unwrap_or_default();
                    for t in targets {
                        crate::cfi::remove_doclink(conn, &ex.log, &ex.media, i64::from(did), &field, t).await?;
                    }
                }
                crate::cfi::hard_delete_where(conn, &ex.log, &affected, Some(remove)).await?;
            }
            Ok(vec![bulk_update(affected)])
        }
        Params::Delete => {
            // root_ids + versions; QuerySet.delete(transaction_id=uuid4()) (order -created)
            let ids32: Vec<i32> = doc_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
            let roots: Vec<i32> = sqlx::query_scalar(
                "SELECT DISTINCT id FROM documents_document WHERE id = ANY($1) AND root_document_id IS NULL AND deleted_at IS NULL",
            )
            .bind(&ids32)
            .fetch_all(&mut *conn)
            .await?;
            let versions: Vec<i32> = sqlx::query_scalar(
                "SELECT DISTINCT id FROM documents_document WHERE root_document_id = ANY($1) AND deleted_at IS NULL \
                 AND NOT (id = ANY($2))",
            )
            .bind(&roots)
            .bind(&ids32)
            .fetch_all(&mut *conn)
            .await?;
            let mut delete_ids: Vec<i32> = ids32.clone();
            delete_ids.extend(versions);
            delete_ids.sort_unstable();
            delete_ids.dedup();
            let ordered: Vec<i32> = sqlx::query_scalar(
                "SELECT id FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY created DESC, id ASC",
            )
            .bind(&delete_ids)
            .fetch_all(&mut *conn)
            .await?;
            let tid = uuid::Uuid::new_v4();
            for id in ordered {
                // The iterator has preloaded the instances: versions already deleted via the root
                // are deleted again (without cascade).
                if let Err(e) =
                    crate::softdelete::soft_delete_document(conn, &ex.log, &ex.media, i64::from(id), Some(tid)).await
                {
                    tracing::error!(target: "paperless.bulk_edit", "Error deleting documents: {e:?}");
                    break;
                }
            }
            Ok(Vec::new())
        }

        Params::Fk { column, value } => {
            // ~Q(fk=value): None → IS NOT NULL; otherwise NOT (fk = v AND fk IS NOT NULL)
            let mut sel = Query::select();
            sel.column(a("id"))
                .from(a("documents_document"))
                .and_where(Expr::col(a("id")).is_in(i32s(doc_ids.iter().copied())))
                .and_where(Expr::col(a("deleted_at")).is_null());
            match value {
                None => {
                    sel.and_where(Expr::col(a(column)).is_not_null());
                }
                Some(v) => {
                    sel.cond_where(
                        sea_query::Cond::any()
                            .add(Expr::col(a(column)).is_null())
                            .add(Expr::col(a(column)).ne(i32v(*v))),
                    );
                }
            }
            let (sql, values) = sel.build_sqlx(PostgresQueryBuilder);
            let rows: Vec<(i32,)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
            let affected: Vec<i64> = rows.into_iter().map(|(i,)| i64::from(i)).collect();
            if !affected.is_empty() {
                let upd = Query::update()
                    .table(a(pl_authz::grants::document_row_table()))
                    .value(a(column), value.map(i32v))
                    .and_where(Expr::col(a("id")).is_in(i32s(affected.iter().copied())))
                    .to_owned();
                let (sql, values) = upd.build_sqlx(PostgresQueryBuilder);
                sqlx::query_with(&sql, values).execute(&mut *conn).await?;
            }
            Ok(vec![bulk_update(affected)])
        }
        Params::Tag { id, add } => execute_tag(conn, doc_ids, *id, *add).await,
        Params::ModifyTags { add, remove } => {
            let affected = existing_doc_ids(conn, doc_ids).await?;
            let mut add_set: BTreeSet<i64> = BTreeSet::new();
            for (id, anc, _) in tag_tree(conn, add).await? {
                add_set.insert(id);
                add_set.extend(anc);
            }
            let mut remove_set: BTreeSet<i64> = BTreeSet::new();
            for (id, _, desc) in tag_tree(conn, remove).await? {
                remove_set.insert(id);
                remove_set.extend(desc);
            }
            if !remove_set.is_empty() && !affected.is_empty() {
                let del = Query::delete()
                    .from_table(a("documents_document_tags"))
                    .and_where(Expr::col(a("document_id")).is_in(i32s(affected.iter().copied())))
                    .and_where(Expr::col(a("tag_id")).is_in(i32s(remove_set.iter().copied())))
                    .to_owned();
                let (sql, values) = del.build_sqlx(PostgresQueryBuilder);
                sqlx::query_with(&sql, values).execute(&mut *conn).await?;
            }
            if !add_set.is_empty() && !affected.is_empty() {
                let q = Query::select()
                    .columns([a("document_id"), a("tag_id")])
                    .from(a("documents_document_tags"))
                    .and_where(Expr::col(a("document_id")).is_in(i32s(affected.iter().copied())))
                    .and_where(Expr::col(a("tag_id")).is_in(i32s(add_set.iter().copied())))
                    .to_owned();
                let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
                let existing: HashSet<(i32, i32)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?.into_iter().collect();
                let mut pairs = Vec::new();
                for d in &affected {
                    for t in &add_set {
                        let key = (i32::try_from(*d).unwrap_or(i32::MIN), i32::try_from(*t).unwrap_or(i32::MIN));
                        if !existing.contains(&key) {
                            pairs.push((*d, *t));
                        }
                    }
                }
                insert_tag_rows(conn, &pairs, true).await?;
            }
            Ok(if affected.is_empty() { Vec::new() } else { vec![bulk_update(affected)] })
        }
        Params::SetPermissions { spec, owner, merge } => {
            let mut upd = Query::update();
            upd.table(a(pl_authz::grants::document_row_table()))
                .value(a("owner_id"), owner.map(i32v))
                .and_where(Expr::col(a("id")).is_in(i32s(doc_ids.iter().copied())))
                .and_where(Expr::col(a("deleted_at")).is_null());
            if *merge {
                upd.and_where(Expr::col(a("owner_id")).is_null());
            }
            let (sql, values) = upd.build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, values).execute(&mut *conn).await?;
            let affected = existing_doc_ids(conn, doc_ids).await?;
            set_permissions_for_documents(conn, spec, &affected, *merge).await?;
            Ok(vec![bulk_update(affected)])
        }
        Params::RunWorkflow { workflow, filter_documents } => Ok(vec![Task::new(
            RUN_WORKFLOWS_FOR_DOCUMENTS,
            json!({"document_ids": doc_ids, "workflow_id": workflow, "filter_documents": filter_documents}),
            TriggerSource::Manual,
        )]),
        Params::Reprocess { remote_ocr, ai_ocr } => {
            let sources = reprocess_sources(conn, doc_ids).await?;
            Ok(sources
                .into_iter()
                .map(|id| {
                    let mut kw = json!({"document_id": id, "remote_ocr": remote_ocr});
                    if *ai_ocr {
                        kw["ai_ocr"] = Value::Bool(true);
                    }
                    Task::new(UPDATE_CONTENT_MAYBE_ARCHIVE, kw, TriggerSource::Manual)
                })
                .collect())
        }
    }
}

/// add_tag / remove_tag (need the method in addition to the parameter).
async fn execute_tag(conn: &mut PgConnection, doc_ids: &[i64], tag: i64, add: bool) -> Result<Vec<Task>, ApiError> {
    let tree = tag_tree(conn, &[tag]).await?;
    let Some((_, ancestors, descendants)) = tree.into_iter().next() else {
        return Err(ApiError::Uncaught("Tag matching query does not exist.".into()));
    };
    if add {
        // [tag_obj, *tag_obj.get_ancestors()] – only existing ancestors
        let mut tags = vec![tag];
        let existing_anc: Vec<i64> = tag_tree(conn, &ancestors).await?.into_iter().map(|(id, _, _)| id).collect();
        tags.extend(ancestors.into_iter().filter(|a| existing_anc.contains(a)));
        let mut affected: BTreeSet<i64> = BTreeSet::new();
        let mut pairs = Vec::new();
        for t in tags {
            let has_tag = Query::select()
                .column(a("document_id"))
                .from(a("documents_document_tags"))
                .and_where(Expr::col(a("tag_id")).eq(i32v(t)))
                .to_owned();
            let q = Query::select()
                .column(a("id"))
                .from(a("documents_document"))
                .and_where(Expr::col(a("id")).is_in(i32s(doc_ids.iter().copied())))
                .and_where(Expr::col(a("deleted_at")).is_null())
                .and_where(Expr::col(a("id")).not_in_subquery(has_tag))
                .to_owned();
            let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
            let rows: Vec<(i32,)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
            for (d,) in rows {
                affected.insert(i64::from(d));
                pairs.push((i64::from(d), t));
            }
        }
        // PaperRust: ON CONFLICT DO NOTHING (FINDINGS B5, contract Known differences) – same result
        // ("tag is set"), even if a concurrent change has already created the row.
        insert_tag_rows(conn, &pairs, true).await?;
        Ok(if affected.is_empty() { Vec::new() } else { vec![bulk_update(affected.into_iter().collect())] })
    } else {
        let mut tag_ids = vec![tag];
        tag_ids.extend(descendants);
        let del = Query::delete()
            .from_table(a("documents_document_tags"))
            .and_where(Expr::col(a("document_id")).is_in(i32s(doc_ids.iter().copied())))
            .and_where(Expr::col(a("tag_id")).is_in(i32s(tag_ids.iter().copied())))
            .returning_col(a("document_id"))
            .to_owned();
        let (sql, values) = del.build_sqlx(PostgresQueryBuilder);
        let rows: Vec<(i32,)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
        let affected: BTreeSet<i64> = rows.into_iter().map(|(d,)| i64::from(d)).collect();
        Ok(if affected.is_empty() { Vec::new() } else { vec![bulk_update(affected.into_iter().collect())] })
    }
}

/// `reprocess`: source per document = own id (version) or latest non-deleted version, or id.
///
/// Review 1 M3: one query as upstream (`bulk_edit.py:420-444`, `annotate(source_id=Case(...))
/// .distinct()`) instead of one query per root document. Order by `id` (upstream: undefined
/// `DISTINCT` order).
pub const REPROCESS_SOURCES_SQL: &str = "SELECT CASE WHEN d.root_document_id IS NOT NULL THEN d.id      ELSE COALESCE((SELECT v.id FROM documents_document v WHERE v.root_document_id = d.id      AND v.deleted_at IS NULL ORDER BY v.version_index DESC NULLS LAST, v.id DESC LIMIT 1), d.id) END      FROM documents_document d WHERE d.id = ANY($1) AND d.deleted_at IS NULL ORDER BY d.id";

pub async fn reprocess_sources(conn: &mut PgConnection, doc_ids: &[i64]) -> Result<Vec<i64>, ApiError> {
    if doc_ids.is_empty() {
        return Ok(Vec::new());
    }
    // ids outside int4 cannot exist
    let ids: Vec<i32> = doc_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let rows: Vec<(i32,)> = sqlx::query_as(REPROCESS_SOURCES_SQL).bind(&ids).fetch_all(&mut *conn).await?;
    let mut out: Vec<i64> = Vec::new();
    for (source,) in rows {
        let source = i64::from(source);
        if !out.contains(&source) {
            out.push(source);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_lists() {
        assert_eq!(strict_int_list(&json!([1, true]), "add_tags"), Ok(vec![1, 1]));
        assert_eq!(strict_int_list(&json!("x"), "add_tags"), Err("add_tags must be a list".into()));
        assert_eq!(strict_int_list(&json!([1.0]), "add_tags"), Err("add_tags must be a list of integers".into()));
    }

    #[test]
    fn set_permissions_serializer_errors() {
        assert_eq!(
            run_set_permissions_serializer(&json!(false)).unwrap_err(),
            json!({"non_field_errors": ["Invalid data. Expected a dictionary, but got bool."]})
        );
        assert_eq!(
            run_set_permissions_serializer(&json!({"view": {"users": false}})).unwrap_err(),
            json!({"view": {"users": ["Expected a list of items but got type \"bool\"."]}})
        );
        assert_eq!(
            run_set_permissions_serializer(&json!({"not_a_real_action": {"users": [1]}})).unwrap_err(),
            json!({"not_a_real_action": ["Unknown permission action."]})
        );
        let ok = run_set_permissions_serializer(&json!({"view": {"users": [1, 2], "groups": null}})).unwrap();
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].1, Some(Some(vec![1, 2])));
        assert_eq!(ok[0].2, Some(None));
    }

    #[test]
    fn kwargs() {
        let mut p = Map::new();
        // PaperRust: reprocess additionally accepts ai_ocr (AI text recognition)
        let mut r = Map::new();
        r.insert("remote_ocr".into(), json!(false));
        r.insert("ai_ocr".into(), json!(true));
        assert!(kwargs_accepted(Method::Reprocess, &r));
        p.insert("correspondent".into(), json!(1));
        assert!(kwargs_accepted(Method::SetCorrespondent, &p));
        p.insert("source_mode".into(), json!("latest_version"));
        assert!(!kwargs_accepted(Method::SetCorrespondent, &p));
    }

    #[test]
    fn run_workflow_method() {
        let m = Method::from_choice("run_workflow").unwrap();
        assert_eq!(m, Method::RunWorkflow);
        assert_eq!(m.name(), "run_workflow");
        assert!(METHOD_CHOICES.contains(&"run_workflow"));
        // owners only (like set_permissions/delete), no add/delete permissions needed
        let n = m.needs();
        assert!(n.ownership && !n.add_document && !n.delete_document);
        assert!(m.snapshot().is_none());
        let mut p = Map::new();
        p.insert("workflow".into(), json!(5));
        p.insert("filter_documents".into(), json!(false));
        assert!(kwargs_accepted(m, &p));
        p.insert("remote_ocr".into(), json!(true));
        assert!(!kwargs_accepted(m, &p));
    }

}
