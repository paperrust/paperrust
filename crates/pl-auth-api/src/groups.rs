//! `/api/groups/` (`GroupViewSet`).
//!
//! upstream: src/paperless/views.py@v3.4.2 (GroupViewSet), src/paperless/serialisers.py@v3.4.2
//! (GroupSerializer), src/paperless/filters.py@v3.4.2 (GroupFilterSet),
//! src/documents/signals/handlers.py@v3.4.2 (cleanup_user_deletion, post_delete Group),
//! Django deletion Collector (m2m through rows and guardian GroupObjectPermission CASCADE).

use crate::db::a;
use crate::fields::validate_permission_codenames;
use crate::http::{Data, FieldInput, FieldOpts, HResult, drf_char, get_list_value, get_value, server_error};
use crate::profile::begin;
use axum::extract::{Path, Request, State};
use axum::response::{IntoResponse, Response};
use pl_authz::ObjectModel;
use pl_core::filters::{CHAR_KWARGS, FieldKind, FilterContext, FilterSet, apply_order_terms, parse_ordering};
use pl_core::pagination::{PageRequest, fetch_all_ids, fetch_page, paginated_body};
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{ApiError, AppState, Drf, RequestContext, ValidationErrors};
use sea_query::{Expr, Func, Order, PostgresQueryBuilder, Query, SelectStatement};
use serde_json::{Map, Value};
use sqlx::{PgPool, Row};

pub const GROUP_MODEL: ObjectModel =
    ObjectModel { app_label: "auth", model_name: "group", object_name: "Group", table: "auth_group", soft_delete: false };

/// Tables with a FK to `auth_group(id)` (reference schema); all CASCADE (m2m through tables
/// and guardian's `GroupObjectPermission`).
pub const GROUP_REFERENCES: &[&str] = &[
    "auth_group_permissions",
    "auth_user_groups",
    "documents_workflowaction_assign_view_groups",
    "documents_workflowaction_remove_view_groups",
    "documents_workflowaction_remove_change_groups",
    "documents_workflowaction_assign_change_groups",
    "guardian_groupobjectpermission",
];

fn respond(r: HResult<Response>) -> Response {
    match r {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

fn base_select() -> SelectStatement {
    let mut q = Query::select();
    q.expr_as(Expr::col((a("auth_group"), a("id"))).cast_as(a("int8")), a("id")).from(a("auth_group"));
    q
}

// Raw SQL: ARRAY(...) sub-select with ORDER BY has no sea-query form.
const REPR_SQL: &str = r#"
SELECT g.id::int8 AS id, g.name,
       ARRAY(SELECT p.codename FROM auth_group_permissions gp
             JOIN auth_permission p ON p.id = gp.permission_id
             JOIN django_content_type ct ON ct.id = p.content_type_id
             WHERE gp.group_id = g.id ORDER BY ct.app_label, ct.model, p.codename) AS permissions
FROM auth_group g WHERE g.id = ANY($1)"#;

/// `GroupSerializer.to_representation` for `ids` (in that order).
pub async fn groups_repr(db: &PgPool, ids: &[i64]) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query(REPR_SQL).bind(ids).fetch_all(db).await?;
    let mut by_id = std::collections::HashMap::new();
    for r in &rows {
        let id: i64 = r.try_get("id")?;
        let perms: Vec<String> = r.try_get("permissions")?;
        let mut m = Map::new();
        m.insert("id".into(), Value::from(id));
        m.insert("name".into(), Value::String(r.try_get("name")?));
        m.insert("permissions".into(), Value::Array(perms.into_iter().map(Value::String).collect()));
        by_id.insert(id, Value::Object(m));
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

pub async fn list(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            auth.require_model_perms(&GROUP_MODEL)?;
            let ctx = RequestContext::from_parts(&r.parts, state.config())?;
            let mut q = base_select();
            FilterSet::new().field("name", Expr::col((a("auth_group"), a("name"))), FieldKind::Char, CHAR_KWARGS).apply(
                &mut q,
                &ctx.query,
                &FilterContext::new(&ctx, state.tz()),
            )?;
            match parse_ordering(&ctx.query, &["name"]) {
                Some(terms) => apply_order_terms(&mut q, &terms, |_| Some(Expr::col((a("auth_group"), a("name"))).into())),
                None => {
                    q.order_by_expr(Func::lower(Expr::col((a("auth_group"), a("name")))).into(), Order::Asc);
                }
            }
            let (rows, info) = fetch_page(state.db(), &q, &PageRequest::from_query(&ctx.query)).await?;
            let ids: Vec<i64> = rows.iter().map(|r| r.try_get::<i64, _>("id")).collect::<Result<_, _>>()?;
            let results = groups_repr(state.db(), &ids).await?;
            let all = if ctx.version().includes_all() {
                let ids = fetch_all_ids(state.db(), &q, Expr::col((a("auth_group"), a("id"))).into()).await?;
                Some(ids.into_iter().map(Value::from).collect())
            } else {
                None
            };
            Ok(Drf::ok(paginated_body(&ctx, &info, results, all)).into_response())
        }
        .await,
    )
}

async fn get_object_id(db: &PgPool, raw: &str) -> HResult<i64> {
    let Some(pk) = pl_core::pagination::python_int(raw).and_then(|i| i32::try_from(i).ok()) else {
        return Err(ApiError::not_found().into());
    };
    let mut q = base_select();
    q.and_where(Expr::col((a("auth_group"), a("id"))).eq(pk));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    match sqlx::query_with(&sql, values).fetch_optional(db).await? {
        Some(r) => Ok(r.try_get::<i64, _>("id")?),
        None => Err(ApiError::not_found_model("Group").into()),
    }
}

pub async fn retrieve(State(state): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    respond(
        async {
            let (_r, auth) = begin(&state, req).await?;
            auth.require_model_perms(&GROUP_MODEL)?;
            let id = get_object_id(state.db(), &id).await?;
            let body = groups_repr(state.db(), &[id]).await?.pop().ok_or_else(|| server_error("group vanished"))?;
            Ok(Drf::ok(body).into_response())
        }
        .await,
    )
}

struct GroupInput {
    name: Option<String>,
    permissions: Option<Vec<i64>>,
}

async fn validate_group(db: &PgPool, data: &Data, partial: bool, instance: Option<i64>) -> HResult<GroupInput> {
    data.require_mapping()?;
    let mut errs = ValidationErrors::new();
    let mut name = None;
    match get_value(data, "name", FieldOpts { partial, required: true, ..Default::default() }) {
        FieldInput::Missing => {
            if !partial {
                errs.add("name", "This field is required.");
            }
        }
        FieldInput::Value(v) => match drf_char(&v, false, None, true) {
            Err(m) => errs.add("name", m),
            Ok(s) => {
                // validators: UniqueValidator, MaxLengthValidator(150), ProhibitNullCharacters
                let mut msgs = Vec::new();
                if !s.contains('\0') {
                    let mut q = Query::select();
                    q.expr(Expr::val(1)).from(a("auth_group")).and_where(Expr::col(a("name")).eq(s.as_str()));
                    if let Some(id) = instance {
                        q.and_where(Expr::col(a("id")).ne(id));
                    }
                    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
                    if sqlx::query_with(&sql, values).fetch_optional(db).await?.is_some() {
                        msgs.push("group with this name already exists.".to_string());
                    }
                }
                if s.chars().count() > 150 {
                    msgs.push("Ensure this field has no more than 150 characters.".to_string());
                }
                if s.contains('\0') {
                    msgs.push("Null characters are not allowed.".to_string());
                }
                if msgs.is_empty() {
                    name = Some(s);
                } else {
                    errs.set("name", Value::Array(msgs.into_iter().map(Value::String).collect()));
                }
            }
        },
    }
    let permissions = validate_permission_codenames(db, get_list_value(data, "permissions", partial), "permissions", !partial, &mut errs)
        .await?
        .map_err(server_error)?;
    errs.into_result()?;
    Ok(GroupInput { name, permissions })
}

async fn set_group_permissions(tx: &mut sqlx::PgConnection, group_id: i64, perms: &[i64]) -> Result<(), sqlx::Error> {
    let (sql, values) = Query::delete()
        .from_table(a("auth_group_permissions"))
        .and_where(Expr::col(a("group_id")).eq(group_id))
        .and_where(Expr::col(a("permission_id")).is_not_in(perms.iter().copied()))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *tx).await?;
    // Review 2 N12: one multi-row insert
    if let Some((sql, values)) = crate::users::m2m_insert("auth_group_permissions", "group_id", group_id, "permission_id", perms)? {
        sqlx::query_with(&sql, values).execute(&mut *tx).await?;
    }
    Ok(())
}

pub async fn create(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            auth.require_model_perms(&GROUP_MODEL)?;
            let data = r.data().await?;
            let input = validate_group(state.db(), &data, false, None).await?;
            let mut tx = state.db().begin().await?;
            let (sql, values) = Query::insert()
                .into_table(a("auth_group"))
                .columns([a("name")])
                .values([input.name.unwrap_or_default().into()])
                .map_err(crate::db::qerr)?
                .returning(Query::returning().column(a("id")))
                .build_sqlx(PostgresQueryBuilder);
            let row = sqlx::query_with(&sql, values)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| if crate::db::is_unique_violation(&e) { server_error("IntegrityError: group name") } else { e.into() })?;
            let id: i64 = i64::from(row.try_get::<i32, _>("id")?);
            if let Some(p) = &input.permissions {
                set_group_permissions(&mut tx, id, p).await?;
            }
            tx.commit().await?;
            let body = groups_repr(state.db(), &[id]).await?.pop().ok_or_else(|| server_error("group vanished"))?;
            Ok(Drf::created(body).into_response())
        }
        .await,
    )
}

async fn update_impl(state: AppState, raw_id: String, req: Request, partial: bool) -> HResult<Response> {
    let (r, auth) = begin(&state, req).await?;
    auth.require_model_perms(&GROUP_MODEL)?;
    let db = state.db();
    let id = get_object_id(db, &raw_id).await?;
    let data = r.data().await?;
    let input = validate_group(db, &data, partial, Some(id)).await?;
    let mut tx = db.begin().await?;
    if let Some(name) = input.name {
        let (sql, values) = Query::update()
            .table(a("auth_group"))
            .value(a("name"), name)
            .and_where(Expr::col(a("id")).eq(id))
            .build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values)
            .execute(&mut *tx)
            .await
            .map_err(|e| if crate::db::is_unique_violation(&e) { server_error("IntegrityError: group name") } else { e.into() })?;
    }
    if let Some(p) = &input.permissions {
        set_group_permissions(&mut tx, id, p).await?;
    }
    tx.commit().await?;
    let body = groups_repr(db, &[id]).await?.pop().ok_or_else(|| server_error("group vanished"))?;
    Ok(Drf::ok(body).into_response())
}

pub async fn update(State(state): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    respond(update_impl(state, id, req, false).await)
}

pub async fn partial_update(State(state): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    respond(update_impl(state, id, req, true).await)
}

/// `cleanup_user_deletion` for a group: remove its pk from `default_view_groups` /
/// `default_change_groups` (first occurrence, like `list.remove`). Errors are only logged.
pub fn cleanup_permission_lists(settings: &mut Value, keys: &[&str], pk: i64) -> bool {
    let Some(perms) = settings.get_mut("permissions").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut updated = false;
    for key in keys {
        if let Some(Value::Array(list)) = perms.get_mut(*key)
            && let Some(pos) = list.iter().position(|v| v.as_i64() == Some(pk) || v.as_f64() == Some(pk as f64))
        {
            list.remove(pos);
            updated = true;
        }
    }
    updated
}

pub async fn destroy(State(state): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    respond(async {
        let (_r, auth) = begin(&state, req).await?;
        auth.require_model_perms(&GROUP_MODEL)?;
        let db = state.db();
        let id = get_object_id(db, &id).await?;
        let mut tx = db.begin().await?;
        for table in GROUP_REFERENCES {
            let (sql, values) = Query::delete()
                .from_table(a(table))
                .and_where(Expr::col(a("group_id")).eq(id))
                .build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, values).execute(&mut *tx).await?;
        }
        let (sql, values) = Query::delete()
            .from_table(a("auth_group"))
            .and_where(Expr::col(a("id")).eq(id))
            .build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values).execute(&mut *tx).await?;

        // post_delete → cleanup_user_deletion(Group)
        let (sql, values) = Query::select()
            .expr_as(Expr::col(a("id")).cast_as(a("int8")), a("id"))
            .column(a("settings"))
            .from(a("documents_uisettings"))
            .order_by(a("id"), Order::Asc)
            .build_sqlx(PostgresQueryBuilder);
        let rows = sqlx::query_with(&sql, values).fetch_all(&mut *tx).await?;
        for row in &rows {
            let ui_id: i64 = row.try_get("id")?;
            let settings: Option<sqlx::types::Json<Value>> = row.try_get("settings")?;
            let Some(sqlx::types::Json(mut settings)) = settings else {
                tracing::error!(target: "paperless.handlers", "Error while cleaning up group {id} from ui_settings: 'NoneType' object has no attribute 'get'");
                continue;
            };
            if cleanup_permission_lists(&mut settings, &["default_view_groups", "default_change_groups"], id) {
                let (sql, values) = Query::update()
                    .table(a("documents_uisettings"))
                    .value(a("settings"), Expr::val(settings.to_string()).cast_as(a("jsonb")))
                    .and_where(Expr::col(a("id")).eq(ui_id))
                    .build_sqlx(PostgresQueryBuilder);
                sqlx::query_with(&sql, values).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(pl_core::json::drf_no_content())
    }
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cleanup_lists() {
        let mut s = json!({"permissions": {"default_view_groups": [3, 5, 5], "default_change_groups": [1]}});
        assert!(cleanup_permission_lists(&mut s, &["default_view_groups", "default_change_groups"], 5));
        assert_eq!(s["permissions"]["default_view_groups"], json!([3, 5]));
        assert!(!cleanup_permission_lists(&mut s, &["default_change_groups"], 7));
        let mut none = json!({"dark_mode": true});
        assert!(!cleanup_permission_lists(&mut none, &["default_view_groups"], 1));
    }
}
