//! `POST /api/bulk_edit_objects/`.
//!
//! upstream: src/documents/views.py@v3.4.2 (BulkEditObjectsView.post),
//!           src/documents/serialisers.py@v3.4.2 (BulkEditObjectsSerializer)

use std::collections::HashSet;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use pl_authz::AuthUser;
use pl_authz::owned::{self, PermissionsSpec};
use pl_core::{ApiError, AppState, RequestContext};
use sea_query::{Alias, Cond, Expr};
use serde_json::{Map, Value, json};
use sqlx::Row;

use crate::drf::{self, AuditMeta, ClientAddr, HResult};
use crate::fields::{self, Input, Out, input};
use crate::listing::{self, ALIAS, FilterData};
use crate::model::Kind;
use crate::tagtree::TagTree;
use crate::objects;
use crate::text::{py_str, py_type_name};

fn choice(inp: Input, choices: &[&str]) -> Result<String, Value> {
    match inp {
        Input::Missing => Err(json!(["This field is required."])),
        Input::Null => Err(json!(["This field may not be null."])),
        Input::Val(v) => {
            let s = py_str(v);
            if choices.contains(&s.as_str()) { Ok(s) } else { Err(json!([format!("\"{s}\" is not a valid choice.")])) }
        }
    }
}

/// `ListField(child=IntegerField(), required=False)` – reuses pl-authz's id-list parser via a
/// wrapped `PermissionSetSerializer` payload (identical child semantics).
fn int_list(inp: Input) -> Result<Option<Vec<i64>>, Value> {
    match inp {
        Input::Missing => Ok(None),
        Input::Null => Err(json!(["This field may not be null."])),
        Input::Val(Value::Array(a)) => {
            let wrapped = json!({"view": {"users": a}});
            match owned::parse_set_permissions(&wrapped) {
                Ok(raw) => Ok(Some(
                    raw.view.and_then(|g| g.users).flatten().unwrap_or_default().into_iter().map(|x| x.unwrap_or(i64::MIN)).collect(),
                )),
                Err(e) => Err(e.0["view"]["users"].clone()),
            }
        }
        Input::Val(v) => Err(json!([format!("Expected a list of items but got type \"{}\".", py_type_name(v))])),
    }
}

pub async fn post(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, client: ClientAddr, body: Bytes) -> HResult {
    // permission_classes = (IsAuthenticated,)
    let user = auth.require_authenticated().map_err(drf::auth)?;
    let db = state.db();
    let meta = AuditMeta::new(&ctx, &client, &auth);
    let body = drf::parse_json_body(&ctx, &body)?;
    let Value::Object(data) = &body else { return Err(drf::not_a_dict(&body)) };

    // --- to_internal_value (objects, all, filters, object_type, operation, owner, permissions, merge)
    let mut errors = Map::new();
    let objects_in = int_list(input(data, "objects")).unwrap_or_else(|e| {
        errors.insert("objects".into(), e);
        None
    });
    let all = match fields::bool_field(input(data, "all"), false) {
        Ok(Out::Set(b)) => b,
        Ok(Out::Skip) => false,
        Err(e) => {
            errors.insert("all".into(), e);
            false
        }
    };
    let filters: Option<Map<String, Value>> = match input(data, "filters") {
        Input::Missing => None,
        Input::Null => {
            errors.insert("filters".into(), json!(["This field may not be null."]));
            None
        }
        Input::Val(Value::Object(m)) => Some(m.clone()),
        Input::Val(v) => {
            errors.insert("filters".into(), json!([format!("Expected a dictionary of items but got type \"{}\".", py_type_name(v))]));
            None
        }
    };
    let kind = match choice(input(data, "object_type"), &["tags", "correspondents", "document_types", "storage_paths"]) {
        Ok(s) => Kind::from_object_type(&s),
        Err(e) => {
            errors.insert("object_type".into(), e);
            None
        }
    };
    let operation = match choice(input(data, "operation"), &["set_permissions", "delete"]) {
        Ok(s) => Some(s),
        Err(e) => {
            errors.insert("operation".into(), e);
            None
        }
    };
    // owner: PrimaryKeyRelatedField(required=False, allow_null=True), no default
    let owner: Option<Option<i64>> = match data.get("owner") {
        None => None,
        Some(v) => match owned::parse_owner(db, v).await.map_err(drf::auth)? {
            Ok(o) => Some(o),
            Err(e) => {
                errors.insert("owner".into(), e.0);
                None
            }
        },
    };
    let permissions_raw = match data.get("permissions") {
        None => None,
        Some(v) => match owned::parse_set_permissions(v) {
            Ok(r) => Some(r),
            Err(e) => {
                errors.insert("permissions".into(), e.0);
                None
            }
        },
    };
    let merge = match fields::bool_field(input(data, "merge"), false) {
        Ok(Out::Set(b)) => b,
        Ok(Out::Skip) => false,
        Err(e) => {
            errors.insert("merge".into(), e);
            false
        }
    };
    if !errors.is_empty() {
        return Err(drf::validation(Value::Object(errors)));
    }
    let (Some(kind), Some(operation)) = (kind, operation) else {
        return Err(ApiError::internal("validated choice missing"));
    };

    // --- validate()
    let mut object_ids: Vec<i64> = vec![];
    if !all {
        let Some(objs) = &objects_in else { return Err(ApiError::non_field_error("objects is required unless all is true.")) };
        if objs.is_empty() {
            return Err(ApiError::non_field_error("objects must not be empty"));
        }
        let found: i64 = sqlx::query(&format!("SELECT COUNT(*) FROM {} WHERE id = ANY($1::bigint[])", kind.table()))
            .bind(objs)
            .fetch_one(db)
            .await?
            .try_get(0)?;
        if found as usize != objs.len() {
            return Err(ApiError::non_field_error("Some ids in objects don't exist or were specified twice."));
        }
        object_ids = objs.clone();
    }
    let mut permissions: Option<PermissionsSpec> = None;
    if operation == "set_permissions"
        && let Some(raw) = &permissions_raw {
            if raw.view.is_none() && raw.change.is_none() {
                return Err(ApiError::non_field_error("permissions must not be empty"));
            }
            match owned::validate_set_permissions(db, raw).await.map_err(drf::auth)? {
                Ok(s) => permissions = Some(s),
                Err(e) => {
                    let msg = e.0.get(0).and_then(|m| m.as_str()).unwrap_or_default().to_string();
                    return Err(ApiError::non_field_error(msg));
                }
            }
        }

    // --- view
    let action = if operation == "set_permissions" { "change" } else { "delete" };
    let codename = kind.codename(action);
    if all {
        // object_class.objects.filter(id__in=permitted_object_ids(user, object_class, perm_codename))
        let mut q = listing::select(kind, user, false, false, false);
        q.cond_where(crate::perms::permitted_ids_cond(user, kind, codename, ALIAS));
        // Review 2 N23: the permitted rows are only needed for tag descendants
        let permitted: Vec<i64> = if kind == Kind::Tag {
            listing::fetch(db, kind, &q, false, false).await?.iter().map(|r| r.id).collect()
        } else {
            Vec::new()
        };
        let f = filters.clone().unwrap_or_default();
        let mut fq = q.clone();
        listing::apply_filters(&mut fq, kind, &FilterData::Json(&f))?;
        let objs = listing::fetch(db, kind, &fq, false, false).await?;
        let mut ids: Vec<i64> = objs.iter().map(|r| r.id).collect();
        if kind == Kind::Tag {
            let editable: HashSet<i64> = permitted.iter().copied().collect();
            let mut set: HashSet<i64> = ids.iter().copied().collect();
            // Tag.get_hierarchy().descendants(all_ids) & editable_ids (upstream #14612)
            let tree = TagTree::load_pool(db).await?;
            set.extend(tree.descendants(ids.iter().copied()).into_iter().filter(|d| editable.contains(d)));
            ids = set.into_iter().collect();
            ids.sort_unstable();
        }
        object_ids = ids;
    }

    if kind == Kind::Tag && operation == "delete" {
        // objs = _with_tag_descendants(objs) (upstream fix fa83a4c5d / #14687, after v3.4.2):
        // the ownership check below also covers every descendant the delete cascades to.
        object_ids = objects::tag_ids_with_descendants(db, &object_ids).await?;
    }

    if !user.is_superuser {
        // Limited to the owner (or unowned), same as documents
        let mut foreign = false;
        if !object_ids.is_empty() {
            let mut q = listing::select(kind, user, false, false, false);
            q.and_where(Expr::col((Alias::new(ALIAS), Alias::new("id"))).is_in(object_ids.clone()));
            q.cond_where(
                Cond::all().not().add(
                    Cond::any()
                        .add(Expr::col((Alias::new(ALIAS), Alias::new("owner_id"))).eq(user.id))
                        .add(Expr::col((Alias::new(ALIAS), Alias::new("owner_id"))).is_null()),
                ),
            );
            q.limit(1);
            foreign = !listing::fetch(db, kind, &q, false, false).await?.is_empty();
        }
        if !(user.has_perm(&kind.model().perm(action)) && !foreign) {
            return Err(ApiError::django_text(StatusCode::FORBIDDEN, "Insufficient permissions"));
        }
    }

    if operation == "set_permissions" {
        let res: Result<(), ApiError> = async {
            let mut tx = db.begin().await?;
            if let Some(o) = owner
                && (!merge || o.is_some()) {
                    let only_unowned = if merge { " AND owner_id IS NULL" } else { "" };
                    sqlx::query(&format!(
                        "UPDATE {} SET owner_id = $1::integer WHERE id = ANY($2::bigint[]){only_unowned}",
                        kind.table()
                    ))
                    .bind(o.map(|x| x as i32))
                    .bind(&object_ids)
                    .execute(&mut *tx)
                    .await?;
                }
            if let Some(spec) = &permissions {
                let existing: Vec<i64> = sqlx::query(&format!("SELECT id FROM {} WHERE id = ANY($1::bigint[]) ORDER BY id", kind.table()))
                    .bind(&object_ids)
                    .fetch_all(&mut *tx)
                    .await?
                    .iter()
                    .map(|r| r.try_get::<i32, _>(0).map(i64::from))
                    .collect::<Result<_, _>>()?;
                owned::set_permissions_for_objects(&mut tx, spec, kind.model(), &existing, merge).await.map_err(drf::auth)?;
            }
            tx.commit().await?;
            Ok(())
        }
        .await;
        if let Err(e) = res {
            tracing::warn!("An error occurred performing bulk permissions edit: {e}");
            return Err(ApiError::django_text(
                StatusCode::BAD_REQUEST,
                "Error performing bulk permissions edit, check logs for more detail.",
            ));
        }
    } else {
        let mut tx = db.begin().await?;
        objects::delete_objects(&mut tx, &meta, state.config().audit_log_enabled, user, kind, &object_ids).await?;
        tx.commit().await?;
    }
    Ok(drf::ok(json!({"result": "OK"})))
}
