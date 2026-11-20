//! Create / update / destroy of correspondents, document types, tags and storage paths.
//!
//! upstream: src/documents/serialisers.py@v3.4.2 (MatchingModelSerializer.validate/validate_match,
//!           SetPermissionsMixin, OwnedObjectSerializer.create/update/validate_unique_together,
//!           TagSerializer.validate/validate_color, StoragePathSerializer.validate_path/update),
//!           src/documents/templating/filepath.py@v3.4.2 (validate_filepath_template_and_render),
//!           src/documents/views.py@v3.4.2 (TagViewSet.perform_update,
//!           StoragePathViewSet.destroy), src/documents/tasks.py@v3.4.2 (update_document_parent_tags),
//!           rest_framework 3.18.3 validators.UniqueTogetherValidator (+ ModelSerializer uniqueness kwargs)

use std::collections::HashSet;

use axum::response::Response;
use pl_authz::User;
use pl_authz::owned::{self, PermissionsSpec};
use pl_core::{ApiError, AppState, RequestContext};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::audit;
use crate::drf::{self, AuditMeta};
use crate::fields::{self, CharOpts, Out, PkCandidate, input};
use crate::model::{Kind, MATCH_REGEX, ObjectRow, TagExtra};
use crate::objects;
use crate::sidefx;
use crate::tagtree::TagTree;

/// Parent tag as loaded by the `parent` PrimaryKeyRelatedField.
#[derive(Clone, Debug)]
pub struct ParentTag {
    pub id: i64,
}

/// Validated data (`serializer.validated_data`); `None` = key absent.
#[derive(Clone, Debug, Default)]
pub struct Validated {
    pub name: Option<String>,
    pub color: Option<String>,
    pub r#match: Option<String>,
    pub matching_algorithm: Option<i16>,
    pub is_insensitive: Option<bool>,
    pub is_inbox_tag: Option<bool>,
    pub owner: Option<Option<i64>>,
    pub set_permissions: Option<PermissionsSpec>,
    pub parent: Option<Option<ParentTag>>,
    /// Storage paths: converted template (`validate_path`).
    pub path: Option<String>,
}

/// Python `regex.compile(pattern)` stand-in (Known difference 1 in the contract).
pub fn regex_compiles(p: &str) -> bool {
    fancy_regex::Regex::new(p).is_ok()
}

async fn load_parent(db: &PgPool, id: i64) -> Result<Option<ParentTag>, ApiError> {
    // bind an int4 column as i32 (project rule, FINDINGS N4); outside the range the row does not exist
    let Ok(id) = i32::try_from(id) else { return Ok(None) };
    let row: Option<(i32,)> = sqlx::query_as("SELECT id FROM documents_tag WHERE id = $1").bind(id).fetch_optional(db).await?;
    Ok(row.map(|(id,)| ParentTag { id: i64::from(id) }))
}

/// `Serializer.to_internal_value` for the writable fields (errors collected in field order).
async fn to_internal_value(
    db: &PgPool,
    kind: Kind,
    data: &Map<String, Value>,
    partial: bool,
    tz: chrono_tz::Tz,
) -> Result<Result<Validated, Map<String, Value>>, ApiError> {
    let mut v = Validated::default();
    let mut errors = Map::new();
    let name_opts = CharOpts { required: true, allow_blank: false, max_length: 128 };
    match fields::char_field(input(data, "name"), partial, &name_opts) {
        Ok(Out::Set(s)) => v.name = Some(s),
        Ok(Out::Skip) => {}
        Err(e) => {
            errors.insert("name".into(), e);
        }
    }
    if kind == Kind::StoragePath {
        // `path = models.TextField()` → CharField(required, no max_length) + validate_path
        let o = CharOpts { required: true, allow_blank: false, max_length: usize::MAX };
        match fields::char_field(input(data, "path"), partial, &o) {
            Ok(Out::Set(s)) => match validate_path(&s, tz)? {
                Some(p) => v.path = Some(p),
                None => {
                    errors.insert("path".into(), json!(["Invalid variable detected."]));
                }
            },
            Ok(Out::Skip) => {}
            Err(e) => {
                errors.insert("path".into(), e);
            }
        }
    }
    if kind == Kind::Tag {
        let o = CharOpts { required: false, allow_blank: false, max_length: 7 };
        match fields::char_field(input(data, "color"), partial, &o) {
            Ok(Out::Set(s)) => {
                // validate_color: re.match(r"#[0-9a-fA-F]{6}", color)
                let b = s.as_bytes();
                let ok = b.len() >= 7 && b[0] == b'#' && b[1..7].iter().all(|c| c.is_ascii_hexdigit());
                if ok {
                    v.color = Some(s);
                } else {
                    errors.insert("color".into(), json!(["Invalid color."]));
                }
            }
            Ok(Out::Skip) => {}
            Err(e) => {
                errors.insert("color".into(), e);
            }
        }
    }
    let match_opts = CharOpts { required: false, allow_blank: true, max_length: 256 };
    match fields::char_field(input(data, "match"), partial, &match_opts) {
        Ok(Out::Set(s)) => {
            // validate_match: only when initial_data["matching_algorithm"] == MATCH_REGEX (numeric 4)
            let is_regex = matches!(data.get("matching_algorithm"), Some(Value::Number(n)) if n.as_f64() == Some(MATCH_REGEX as f64));
            if is_regex && !regex_compiles(&s) {
                errors.insert("match".into(), json!(["Invalid regular expression, see log for details."]));
            } else {
                v.r#match = Some(s);
            }
        }
        Ok(Out::Skip) => {}
        Err(e) => {
            errors.insert("match".into(), e);
        }
    }
    match fields::matching_algorithm(input(data, "matching_algorithm"), partial) {
        Ok(Out::Set(a)) => v.matching_algorithm = Some(a),
        Ok(Out::Skip) => {}
        Err(e) => {
            errors.insert("matching_algorithm".into(), e);
        }
    }
    match fields::bool_field(input(data, "is_insensitive"), partial) {
        Ok(Out::Set(b)) => v.is_insensitive = Some(b),
        Ok(Out::Skip) => {}
        Err(e) => {
            errors.insert("is_insensitive".into(), e);
        }
    }
    if kind == Kind::Tag {
        match fields::bool_field(input(data, "is_inbox_tag"), partial) {
            Ok(Out::Set(b)) => v.is_inbox_tag = Some(b),
            Ok(Out::Skip) => {}
            Err(e) => {
                errors.insert("is_inbox_tag".into(), e);
            }
        }
    }
    // owner: PrimaryKeyRelatedField(allow_null=True) with default=None (DRF uniqueness kwargs);
    // the default applies only to non-partial validation.
    match data.get("owner") {
        None => {
            if !partial {
                v.owner = Some(None);
            }
        }
        Some(val) => match owned::parse_owner(db, val).await.map_err(drf::auth)? {
            Ok(o) => v.owner = Some(o),
            Err(e) => {
                errors.insert("owner".into(), e.0);
            }
        },
    }
    if let Some(sp) = data.get("set_permissions") {
        match owned::parse_set_permissions(sp) {
            Ok(raw) => match owned::validate_set_permissions(db, &raw).await.map_err(drf::auth)? {
                Ok(spec) => v.set_permissions = Some(spec),
                Err(e) => {
                    errors.insert("set_permissions".into(), e.0);
                }
            },
            Err(e) => {
                errors.insert("set_permissions".into(), e.0);
            }
        }
    }
    if kind == Kind::Tag {
        match fields::pk_field(input(data, "parent"), partial, false) {
            Ok(Out::Set(None)) => v.parent = Some(None),
            Ok(Out::Set(Some((PkCandidate::Id(id), repr)))) => {
                let p = match id {
                    Some(id) => load_parent(db, id).await?,
                    None => None,
                };
                match p {
                    Some(p) => v.parent = Some(Some(p)),
                    None => {
                        errors.insert("parent".into(), json!([format!("Invalid pk \"{repr}\" - object does not exist.")]));
                    }
                }
            }
            Ok(Out::Skip) => {}
            Err(e) => {
                errors.insert("parent".into(), e);
            }
        }
    }
    Ok(if errors.is_empty() { Ok(v) } else { Err(errors) })
}

/// `StoragePathSerializer.validate_path`: old `{var}` syntax converted, then rendered against the
/// dummy document. `Ok(None)` = "Invalid variable detected."; `Err` = exception outside the
/// `try` of `validate_filepath_template_and_render` (500).
pub fn validate_path(path: &str, tz: chrono_tz::Tz) -> Result<Option<String>, ApiError> {
    let converted = pl_templating::convert_format_str_to_template_format(path);
    if converted != path {
        tracing::warn!(target: "paperless.serializers", "Storage path {path} is not using the new style format, consider updating");
    }
    let opts = pl_templating::Options { time_zone: tz, ..Default::default() };
    match pl_templating::validate_filepath_template_and_render(&converted, None, &opts) {
        Ok(_) => Ok(Some(converted)),
        Err(pl_templating::TemplateError::Context(m)) => Err(ApiError::internal(m)),
        Err(_) => Ok(None),
    }
}

const EXISTS_SQL: &str = "SELECT EXISTS (SELECT 1 FROM {table} WHERE name = $1 AND owner_id IS NOT DISTINCT FROM $2::integer AND ($3::integer IS NULL OR id <> $3::integer))";

async fn exists_name_owner(db: &PgPool, kind: Kind, name: &str, owner: Option<i64>, exclude: Option<i64>) -> Result<bool, ApiError> {
    let sql = EXISTS_SQL.replace("{table}", kind.table());
    let r = sqlx::query(&sql).bind(name).bind(owner.map(|o| o as i32)).bind(exclude.map(|o| o as i32)).fetch_one(db).await?;
    Ok(r.try_get::<bool, _>(0)?)
}

async fn exists_tx(tx: &mut Transaction<'_, Postgres>, kind: Kind, name: &str, owner: Option<i64>, exclude: Option<i64>) -> Result<bool, ApiError> {
    let sql = EXISTS_SQL.replace("{table}", kind.table());
    let r = sqlx::query(&sql).bind(name).bind(owner.map(|o| o as i32)).bind(exclude.map(|o| o as i32)).fetch_one(&mut **tx).await?;
    Ok(r.try_get::<bool, _>(0)?)
}

/// Serializer-level validators + `validate()`. `v` stays as parsed from the request (DRF validates a copy).
async fn run_validators_and_validate(db: &PgPool, kind: Kind, user: &User, v: &Validated, instance: Option<&ObjectRow>) -> Result<(), ApiError> {
    let mut non_field: Vec<Value> = vec![];
    match instance {
        None => {
            let name = v.name.clone().unwrap_or_default();
            let owner = v.owner.unwrap_or(None);
            // UniqueTogetherValidator(name, owner): skipped when owner is None
            if let Some(o) = owner
                && exists_name_owner(db, kind, &name, Some(o), None).await? {
                    non_field.push(json!("The fields name, owner must make a unique set."));
                }
            // UniqueTogetherValidator(name, condition owner IS NULL) never fires: DRF evaluates the
            // condition with `Q.check({"owner": None})`; `Value(None)` has no output field, the
            // `isnull` lookup raises FieldError and `qs_exists_with_condition` returns False.
            // A duplicate with owner null is reported by MatchingModelSerializer.validate
            // (`{"error": [...]}`) – parity cases `*.create.duplicate` step 3.
        }
        Some(inst) => {
            let name_changed = v.name.as_ref().is_some_and(|n| *n != inst.name);
            let owner_changed = v.owner.is_some_and(|o| o != inst.owner_id);
            // filter_queryset() writes missing sources from the instance into attrs – but DRF's
            // `Serializer.run_validators` hands the validators a *copy* (`to_validate`), so
            // `validate()` and `validated_data` never see these values (Review 2 M6).
            let name = v.name.clone().unwrap_or_else(|| inst.name.clone());
            let owner = v.owner.unwrap_or(inst.owner_id);
            // conditional validator (owner IS NULL): never fires, see create above
            if (name_changed || owner_changed)
                && owner.is_some()
                && exists_name_owner(db, kind, &name, owner, Some(inst.id)).await?
            {
                non_field.push(json!("The fields name, owner must make a unique set."));
            }
        }
    }
    if !non_field.is_empty() {
        return Err(drf::validation(json!({"non_field_errors": non_field})));
    }
    // TagSerializer.validate → Tag.clean() on one snapshot of the hierarchy (upstream #14612)
    if kind == Kind::Tag {
        let new_parent: Option<i64> = match &v.parent {
            Some(p) => p.as_ref().map(|p| p.id),
            None => instance.and_then(|i| i.tag.as_ref()).and_then(|t| t.tn_parent_id),
        };
        let tree = TagTree::load_pool(db).await?;
        if let Err(msg) = tree.check_parent(instance.map(|i| i.id), new_parent) {
            return Err(drf::validation(json!({"parent": [msg]})));
        }
    }
    // MatchingModelSerializer.validate: owner = data["owner"] if "owner" in data else self.user
    if let Some((name, owner)) = matching_validate_target(v, instance, user.id)
        && exists_name_owner(db, kind, &name, owner, instance.map(|i| i.id)).await?
    {
        return Err(drf::validation(json!({"error": ["Object violates owner / name unique constraint"]})));
    }
    Ok(())
}

/// `(name, owner)` probed by `MatchingModelSerializer.validate`, or `None` when neither `name` nor
/// `owner` was in the validated data (`serialisers.py:179-200`).
fn matching_validate_target(v: &Validated, instance: Option<&ObjectRow>, user_id: i64) -> Option<(String, Option<i64>)> {
    if v.name.is_none() && v.owner.is_none() {
        return None;
    }
    let name = v.name.clone().or_else(|| instance.map(|i| i.name.clone())).unwrap_or_default();
    let owner = match v.owner {
        Some(o) => o,
        None => Some(user_id),
    };
    Some((name, owner))
}

/// `validate_unique_together` raises outside `is_valid()`: the detail keeps the plain string.
fn unique_error() -> ApiError {
    drf::validation(json!({"error": "Object violates owner / name unique constraint"}))
}

async fn reload(tx: &mut Transaction<'_, Postgres>, kind: Kind, user: &User, id: i64) -> Result<ObjectRow, ApiError> {
    objects::load_rows_tx(tx, kind, user, &[id]).await?.into_iter().next().ok_or_else(|| ApiError::internal("row vanished"))
}

/// POST: `CreateModelMixin.create`.
pub async fn create(state: &AppState, kind: Kind, user: &User, ctx: &RequestContext, meta: &AuditMeta, body: &Value) -> Result<Response, ApiError> {
    let db = state.db();
    let Value::Object(data) = body else { return Err(drf::not_a_dict(body)) };
    let mut v = match to_internal_value(db, kind, data, false, state.config().tz).await? {
        Ok(v) => v,
        Err(e) => return Err(drf::validation(Value::Object(e))),
    };
    run_validators_and_validate(db, kind, user, &v, None).await?;
    // OwnedObjectSerializer.create: default to the current user when "owner" is not in request.data
    if !data.contains_key("owner") {
        v.owner = Some(Some(user.id));
    }
    let name = v.name.clone().unwrap_or_default();
    let owner = v.owner.unwrap_or(None);
    let mut tx = db.begin().await?;
    if exists_tx(&mut tx, kind, &name, owner, None).await? {
        return Err(unique_error());
    }
    let (cols, extra) = if kind == Kind::Tag {
        (
            ", color, is_inbox_tag, tn_parent_id",
            ", $6, $7, $8::integer",
        )
    } else if kind == Kind::StoragePath {
        (", path", ", $6")
    } else {
        ("", "")
    };
    let sql = format!(
        "INSERT INTO {} (name, match, matching_algorithm, is_insensitive, owner_id{cols}) VALUES ($1, $2, $3, $4, $5::integer{extra}) RETURNING id",
        kind.table()
    );
    let mut q = sqlx::query(&sql)
        .bind(&name)
        .bind(v.r#match.clone().unwrap_or_default())
        .bind(v.matching_algorithm.unwrap_or(1))
        .bind(v.is_insensitive.unwrap_or(true))
        .bind(owner.map(|o| o as i32));
    if kind == Kind::Tag {
        q = q
            .bind(v.color.clone().unwrap_or_else(|| "#a6cee3".into()))
            .bind(v.is_inbox_tag.unwrap_or(false))
            .bind(v.parent.clone().flatten().map(|p| p.id as i32));
    } else if kind == Kind::StoragePath {
        q = q.bind(v.path.clone().unwrap_or_default());
    }
    let id = q.fetch_one(&mut *tx).await?.try_get::<i32, _>(0)? as i64;
    let row = reload(&mut tx, kind, user, id).await?;
    if state.config().audit_log_enabled {
        audit::log(&mut tx, meta, kind, audit::CREATE, &row, audit::diff(kind, None, Some(&row))).await?;
    }
    if let Some(spec) = &v.set_permissions {
        owned::set_permissions_for_objects(&mut tx, spec, kind.model(), &[id], false).await.map_err(drf::auth)?;
    }
    tx.commit().await?;
    let out = objects::serialize_single(db, kind, user, ctx, &row).await?;
    Ok(drf::created(out))
}

/// PUT/PATCH: `UpdateModelMixin.update` (+ `TagViewSet.perform_update`).
#[allow(clippy::too_many_arguments)]
pub async fn update(
    state: &AppState,
    kind: Kind,
    user: &User,
    ctx: &RequestContext,
    meta: &AuditMeta,
    instance: ObjectRow,
    body: &Value,
    partial: bool,
) -> Result<Response, ApiError> {
    let db = state.db();
    let Value::Object(data) = body else { return Err(drf::not_a_dict(body)) };
    let v = match to_internal_value(db, kind, data, partial, state.config().tz).await? {
        Ok(v) => v,
        Err(e) => return Err(drf::validation(Value::Object(e))),
    };
    run_validators_and_validate(db, kind, user, &v, Some(&instance)).await?;

    // StoragePathSerializer.update: documents of the path (default manager) get a
    // bulk_update_documents task (rename/move). Sent after the commit (upstream sends it before
    // `save()`, so a fast worker could still see the old path – Known difference).
    let mut sp_docs: Vec<i64> = vec![];
    if kind == Kind::StoragePath {
        sp_docs = sqlx::query("SELECT id FROM documents_document WHERE storage_path_id = $1::integer AND deleted_at IS NULL ORDER BY id")
            .bind(instance.id as i32)
            .fetch_all(db)
            .await?
            .iter()
            .map(|r| r.try_get::<i32, _>(0).map(i64::from))
            .collect::<Result<_, _>>()?;
    }

    // OwnedObjectSerializer.update
    owned::check_owner_update(Some(user), instance.owner_id, v.owner, v.set_permissions.is_some()).map_err(drf::auth)?;
    let mut tx = db.begin().await?;
    if let Some(spec) = &v.set_permissions {
        owned::set_permissions_for_objects(&mut tx, spec, kind.model(), &[instance.id], false).await.map_err(drf::auth)?;
    }
    let name = v.name.clone().unwrap_or_else(|| instance.name.clone());
    if let Some(owner) = v.owner
        && exists_tx(&mut tx, kind, &name, owner, Some(instance.id)).await? {
            // upstream has already applied set_permissions (no atomic request)
            tx.commit().await?;
            return Err(unique_error());
        }
    let old = reload(&mut tx, kind, user, instance.id).await?;
    let mut new = instance.clone();
    new.name = name;
    if let Some(m) = &v.r#match {
        new.r#match = m.clone();
    }
    if let Some(a) = v.matching_algorithm {
        new.matching_algorithm = a;
    }
    if let Some(b) = v.is_insensitive {
        new.is_insensitive = b;
    }
    if let Some(o) = v.owner {
        new.owner_id = o;
    }
    if let Some(t) = new.tag.as_mut() {
        if let Some(c) = &v.color {
            t.color = c.clone();
        }
        if let Some(b) = v.is_inbox_tag {
            t.is_inbox_tag = b;
        }
        if let Some(p) = &v.parent {
            t.tn_parent_id = p.as_ref().map(|p| p.id);
        }
    }
    if let Some(p) = &v.path {
        new.path = Some(p.clone());
    }
    // instance.save(): UPDATE of all concrete fields
    let tag: TagExtra = new.tag.clone().unwrap_or_default();
    if kind == Kind::Tag {
        sqlx::query(
            "UPDATE documents_tag SET name = $1, match = $2, matching_algorithm = $3, is_insensitive = $4, \
             owner_id = $5::integer, color = $6, is_inbox_tag = $7, tn_parent_id = $8::integer WHERE id = $9::integer",
        )
        .bind(&new.name)
        .bind(&new.r#match)
        .bind(new.matching_algorithm)
        .bind(new.is_insensitive)
        .bind(new.owner_id.map(|o| o as i32))
        .bind(&tag.color)
        .bind(tag.is_inbox_tag)
        .bind(tag.tn_parent_id.map(|p| p as i32))
        .bind(new.id as i32)
        .execute(&mut *tx)
        .await?;
    } else if kind == Kind::StoragePath {
        sqlx::query(
            "UPDATE documents_storagepath SET name = $1, match = $2, matching_algorithm = $3, is_insensitive = $4, \
             owner_id = $5::integer, path = $6 WHERE id = $7::integer",
        )
        .bind(&new.name)
        .bind(&new.r#match)
        .bind(new.matching_algorithm)
        .bind(new.is_insensitive)
        .bind(new.owner_id.map(|o| o as i32))
        .bind(new.path.clone().unwrap_or_default())
        .bind(new.id as i32)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(&format!(
            "UPDATE {} SET name = $1, match = $2, matching_algorithm = $3, is_insensitive = $4, owner_id = $5::integer WHERE id = $6::integer",
            kind.table()
        ))
        .bind(&new.name)
        .bind(&new.r#match)
        .bind(new.matching_algorithm)
        .bind(new.is_insensitive)
        .bind(new.owner_id.map(|o| o as i32))
        .bind(new.id as i32)
        .execute(&mut *tx)
        .await?;
    }
    // log_update (pre_save): DB state vs. instance
    if state.config().audit_log_enabled {
        audit::log(&mut tx, meta, kind, audit::UPDATE, &new, audit::diff(kind, Some(&old), Some(&new))).await?;
    }
    let mut parent_docs: Vec<i64> = vec![];
    if kind == Kind::Tag {
        // TagViewSet.perform_update → update_document_parent_tags(tag, new_parent)
        let old_parent = old.tag.as_ref().and_then(|t| t.tn_parent_id);
        if let Some(np) = tag.tn_parent_id
            && old_parent != Some(np) {
                parent_docs = update_document_parent_tags(&mut tx, new.id, np).await?;
            }
    }
    tx.commit().await?;
    if !parent_docs.is_empty() {
        sidefx::send_bulk_update_documents(state, &parent_docs).await?;
    }
    if !sp_docs.is_empty() {
        sidefx::send_bulk_update_documents(state, &sp_docs).await?;
    }
    let out = objects::serialize_single(db, kind, user, ctx, &new).await?;
    Ok(drf::ok(out))
}

/// `documents.tasks.update_document_parent_tags`; returns the affected document ids.
async fn update_document_parent_tags(tx: &mut Transaction<'_, Postgres>, tag_id: i64, parent_id: i64) -> Result<Vec<i64>, ApiError> {
    // Document.objects.filter(tags=tag): non-deleted documents
    let doc_ids: Vec<i64> = sqlx::query(
        "SELECT d.id FROM documents_document d JOIN documents_document_tags dt ON dt.document_id = d.id \
         WHERE dt.tag_id = $1::integer AND d.deleted_at IS NULL ORDER BY d.id",
    )
    .bind(tag_id as i32)
    .fetch_all(&mut **tx)
    .await?
    .iter()
    .map(|r| r.try_get::<i32, _>(0).map(i64::from))
    .collect::<Result<_, _>>()?;
    if doc_ids.is_empty() {
        return Ok(vec![]);
    }
    // parent_ids = [new_parent.id, *new_parent.get_ancestors_pks()], deduplicated (upstream 8aa99a381)
    let tree = TagTree::load(tx).await?;
    let mut parent_ids = vec![parent_id];
    for a in tree.ancestors(parent_id) {
        if !parent_ids.contains(&a) {
            parent_ids.push(a);
        }
    }
    let existing: HashSet<(i64, i64)> = sqlx::query(
        "SELECT document_id, tag_id FROM documents_document_tags WHERE document_id = ANY($1::bigint[]) AND tag_id = ANY($2::bigint[])",
    )
    .bind(&doc_ids)
    .bind(&parent_ids)
    .fetch_all(&mut **tx)
    .await?
    .iter()
    .map(|r| Ok((r.try_get::<i32, _>(0)? as i64, r.try_get::<i32, _>(1)? as i64)))
    .collect::<Result<_, sqlx::Error>>()?;
    let mut docs_new: Vec<i64> = vec![];
    let mut tags_new: Vec<i64> = vec![];
    let mut affected: Vec<i64> = vec![];
    for d in &doc_ids {
        // Integer pairs on both sides since #14612 (v3.4.2 compared ancestor *strings* and re-added
        // every ancestor for every document) → only really missing pairs, only those documents.
        let mut hit = false;
        for p in &parent_ids {
            if !existing.contains(&(*d, *p)) {
                docs_new.push(*d);
                tags_new.push(*p);
                hit = true;
            }
        }
        if hit {
            affected.push(*d);
        }
    }
    if !docs_new.is_empty() {
        // bulk_create(..., ignore_conflicts=True)
        sqlx::query(
            "INSERT INTO documents_document_tags (document_id, tag_id) \
             SELECT d::integer, t::integer FROM unnest($1::bigint[], $2::bigint[]) AS x(d, t) ON CONFLICT DO NOTHING",
        )
        .bind(&docs_new)
        .bind(&tags_new)
        .execute(&mut **tx)
        .await?;
    }
    Ok(affected)
}

/// DELETE (`DestroyModelMixin` / tag CASCADE over `tn_parent` / `StoragePathViewSet.destroy`).
pub async fn destroy(state: &AppState, kind: Kind, user: &User, meta: &AuditMeta, instance: &ObjectRow) -> Result<Response, ApiError> {
    let db = state.db();
    let mut doc_ids: Vec<i64> = vec![];
    if kind == Kind::StoragePath {
        // instance.documents.all(): default manager (not soft-deleted)
        doc_ids = sqlx::query("SELECT id FROM documents_document WHERE storage_path_id = $1::integer AND deleted_at IS NULL ORDER BY id")
            .bind(instance.id as i32)
            .fetch_all(db)
            .await?
            .iter()
            .map(|r| r.try_get::<i32, _>(0).map(i64::from))
            .collect::<Result<_, _>>()?;
    }
    let mut tx = db.begin().await?;
    objects::delete_objects(&mut tx, meta, state.config().audit_log_enabled, user, kind, &[instance.id]).await?;
    tx.commit().await?;
    if !doc_ids.is_empty() {
        sidefx::send_bulk_update_documents(state, &doc_ids).await?;
    }
    Ok(drf::no_content())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_validation() {
        assert!(regex_compiles(r"^inv\d+"));
        assert!(regex_compiles(r"(?<=a)b"));
        assert!(regex_compiles(r"(a)\1"));
        assert!(!regex_compiles(r"(unclosed"));
        assert!(!regex_compiles(r"[z-a]"));
    }

    fn row(name: &str, owner: Option<i64>) -> ObjectRow {
        ObjectRow {
            id: 7,
            name: name.into(),
            r#match: String::new(),
            matching_algorithm: 0,
            is_insensitive: true,
            owner_id: owner,
            document_count: None,
            last_correspondence: None,
            path: None,
            tag: None,
        }
    }

    /// Review 2 M6: PATCH without `owner` checks against `request.user`, not against the instance owner.
    #[test]
    fn matching_validate_uses_request_user_when_owner_absent() {
        let unowned = row("Bar", None);
        let v = Validated { name: Some("Foo".into()), ..Default::default() };
        assert_eq!(matching_validate_target(&v, Some(&unowned), 3), Some(("Foo".into(), Some(3))));
        // explicit owner: null wins over request.user
        let v = Validated { owner: Some(None), ..Default::default() };
        assert_eq!(matching_validate_target(&v, Some(&unowned), 3), Some(("Bar".into(), None)));
        // neither name nor owner in data → no check
        let v = Validated { color: Some("#ffffff".into()), ..Default::default() };
        assert_eq!(matching_validate_target(&v, Some(&unowned), 3), None);
    }

    #[test]
    fn input_helper() {
        let m: Map<String, Value> = serde_json::from_str(r#"{"a": null, "b": 1}"#).unwrap();
        assert!(matches!(input(&m, "a"), crate::fields::Input::Null));
        assert!(matches!(input(&m, "c"), crate::fields::Input::Missing));
    }
}
