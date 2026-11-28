//! `/api/users/` (`UserViewSet`), incl. `destroy` (standalone, docs/STANDALONE-GAPS.md).
//!
//! upstream: src/paperless/views.py@v3.4.2 (UserViewSet: queryset, create, update,
//! destroy, deactivate_totp, _parse_requested_bool), src/paperless/serialisers.py@v3.4.2
//! (UserSerializer, PasswordValidationMixin), src/paperless/filters.py@v3.4.2 (UserFilterSet),
//! src/documents/permissions.py@v3.4.2 (PaperlessObjectPermissions), DRF mixins
//! (ListModelMixin, RetrieveModelMixin, CreateModelMixin, UpdateModelMixin, DestroyModelMixin),
//! src/documents/signals/handlers.py@v3.4.2 (cleanup_user_deletion), Django deletion Collector
//! (`on_delete` of every FK to `auth_user`, see [`USER_CASCADE`] / [`USER_SET_NULL`]).

use crate::db::a;
use crate::fields::{validate_group_pks, validate_permission_codenames};
use crate::http::{
    Data, FieldInput, FieldOpts, HResult, django_text, drf_bool, drf_char, get_list_value, get_value, is_valid_email, is_valid_username,
    server_error,
};
use crate::password::{UserAttrs, has_real_password, validate_password};
use crate::profile::{begin, obfuscate, user_attrs};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use pl_authz::ObjectModel;
use pl_core::filters::{CHAR_KWARGS, FieldKind, FilterContext, FilterSet, apply_order_terms, parse_ordering};
use pl_core::pagination::{PageRequest, fetch_all_ids, fetch_page, paginated_body};
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{ApiError, AppState, Drf, RequestContext, ValidationErrors};
use sea_query::{Alias, Expr, Func, Order, PostgresQueryBuilder, Query, SelectStatement};
use serde_json::{Map, Value};
use sqlx::{PgPool, Row};

pub const USER_MODEL: ObjectModel =
    ObjectModel { app_label: "auth", model_name: "user", object_name: "User", table: "auth_user", soft_delete: false };

fn respond(r: HResult<Response>) -> Response {
    match r {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// `UserViewSet.queryset` (without ordering): excludes `consumer` and `AnonymousUser`.
fn base_select() -> SelectStatement {
    let mut q = Query::select();
    q.expr_as(Expr::col((a("auth_user"), a("id"))).cast_as(a("int8")), a("id"))
        .from(a("auth_user"))
        .and_where(Expr::col((a("auth_user"), a("username"))).is_not_in(["consumer", "AnonymousUser"]));
    q
}

// Raw SQL: ARRAY(...) sub-selects with their own ORDER BY have no sea-query form.
const REPR_SQL: &str = r#"
SELECT u.id::int8 AS id, u.username, u.email, u.password, u.first_name, u.last_name,
       u.date_joined, u.last_login, u.is_staff, u.is_active, u.is_superuser,
       ARRAY(SELECT ug.group_id::int8 FROM auth_user_groups ug
             WHERE ug.user_id = u.id ORDER BY ug.group_id) AS groups,
       ARRAY(SELECT p.codename FROM auth_user_user_permissions up
             JOIN auth_permission p ON p.id = up.permission_id
             JOIN django_content_type ct ON ct.id = p.content_type_id
             WHERE up.user_id = u.id ORDER BY ct.app_label, ct.model, p.codename) AS user_permissions,
       ARRAY(SELECT DISTINCT ct.app_label || '.' || p.codename
             FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id
             WHERE u.is_active AND (u.is_superuser OR p.id IN (
                 SELECT gp.permission_id FROM auth_group_permissions gp
                 JOIN auth_user_groups ug2 ON ug2.group_id = gp.group_id
                 WHERE ug2.user_id = u.id))
             ORDER BY 1) AS inherited_permissions,
       EXISTS(SELECT 1 FROM mfa_authenticator m WHERE m.user_id = u.id) AS is_mfa_enabled
FROM auth_user u WHERE u.id = ANY($1)"#;

/// `UserSerializer.to_representation` for `ids` (result in the order of `ids`).
pub async fn users_repr(db: &PgPool, tz: &chrono_tz::Tz, ids: &[i64]) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query(REPR_SQL).bind(ids).fetch_all(db).await?;
    let mut by_id = std::collections::HashMap::new();
    for r in &rows {
        let id: i64 = r.try_get("id")?;
        let date_joined: DateTime<Utc> = r.try_get("date_joined")?;
        let password: String = r.try_get("password")?;
        let mut m = Map::new();
        m.insert("id".into(), Value::from(id));
        m.insert("username".into(), Value::String(r.try_get("username")?));
        m.insert("email".into(), Value::String(r.try_get("email")?));
        m.insert("password".into(), Value::String(obfuscate(&password)));
        m.insert("first_name".into(), Value::String(r.try_get("first_name")?));
        m.insert("last_name".into(), Value::String(r.try_get("last_name")?));
        m.insert("date_joined".into(), Value::String(pl_core::datetime::format_datetime(&date_joined, tz)));
        m.insert("is_staff".into(), Value::Bool(r.try_get("is_staff")?));
        m.insert("is_active".into(), Value::Bool(r.try_get("is_active")?));
        m.insert("is_superuser".into(), Value::Bool(r.try_get("is_superuser")?));
        let groups: Vec<i64> = r.try_get("groups")?;
        m.insert("groups".into(), Value::Array(groups.into_iter().map(Value::from).collect()));
        let up: Vec<String> = r.try_get("user_permissions")?;
        m.insert("user_permissions".into(), Value::Array(up.into_iter().map(Value::String).collect()));
        let ip: Vec<String> = r.try_get("inherited_permissions")?;
        m.insert("inherited_permissions".into(), Value::Array(ip.into_iter().map(Value::String).collect()));
        m.insert("is_mfa_enabled".into(), Value::Bool(r.try_get("is_mfa_enabled")?));
        // PaperRust extension (not in upstream `UserSerializer.Meta.fields`): read-only, for the user list of the
        // PaperRust design ("zuletzt angemeldet"); docs/contract/pl-auth-api.md Known differences.
        let last_login: Option<DateTime<Utc>> = r.try_get("last_login")?;
        m.insert("last_login".into(), last_login.map_or(Value::Null, |d| Value::String(pl_core::datetime::format_datetime(&d, tz))));
        by_id.insert(id, Value::Object(m));
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

fn filterset() -> FilterSet {
    FilterSet::new().field("username", Expr::col((a("auth_user"), a("username"))), FieldKind::Char, CHAR_KWARGS)
}

/// `GET /api/users/`
pub async fn list(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            auth.require_model_perms(&USER_MODEL)?;
            let ctx = RequestContext::from_parts(&r.parts, state.config())?;
            let mut q = base_select();
            filterset().apply(&mut q, &ctx.query, &FilterContext::new(&ctx, state.tz()))?;
            match parse_ordering(&ctx.query, &["username"]) {
                Some(terms) => apply_order_terms(&mut q, &terms, |_| Some(Expr::col((a("auth_user"), a("username"))).into())),
                None => {
                    q.order_by_expr(Func::lower(Expr::col((a("auth_user"), a("username")))).into(), Order::Asc);
                }
            }
            let preq = PageRequest::from_query(&ctx.query);
            let (rows, info) = fetch_page(state.db(), &q, &preq).await?;
            let ids: Vec<i64> = rows.iter().map(|r| r.try_get::<i64, _>("id")).collect::<Result<_, _>>()?;
            let results = users_repr(state.db(), state.tz(), &ids).await?;
            let all = if ctx.version().includes_all() {
                let ids = fetch_all_ids(state.db(), &q, Expr::col((a("auth_user"), a("id"))).into()).await?;
                Some(ids.into_iter().map(Value::from).collect())
            } else {
                None
            };
            Ok(Drf::ok(paginated_body(&ctx, &info, results, all)).into_response())
        }
        .await,
    )
}

/// `get_object()`: pk parse (`ValueError` → 404 `Not found.`) and queryset lookup.
pub async fn get_object_id(db: &PgPool, raw: &str) -> HResult<i64> {
    let Some(pk) = pl_core::pagination::python_int(raw).and_then(|i| i32::try_from(i).ok()) else {
        return Err(ApiError::not_found().into());
    };
    let mut q = base_select();
    q.and_where(Expr::col((a("auth_user"), a("id"))).eq(pk));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    match sqlx::query_with(&sql, values).fetch_optional(db).await? {
        Some(r) => Ok(r.try_get::<i64, _>("id")?),
        None => Err(ApiError::not_found_model("User").into()),
    }
}

/// `GET /api/users/{id}/`
pub async fn retrieve(State(state): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    respond(
        async {
            let (_r, auth) = begin(&state, req).await?;
            auth.require_model_perms(&USER_MODEL)?;
            let id = get_object_id(state.db(), &id).await?;
            let body = users_repr(state.db(), state.tz(), &[id]).await?.pop().ok_or_else(|| server_error("user vanished"))?;
            Ok(Drf::ok(body).into_response())
        }
        .await,
    )
}

/// `UserViewSet._parse_requested_bool`: `None` = not provided / invalid.
fn requested_bool(data: &Data, key: &str) -> Option<bool> {
    if !data.contains(key) {
        return None;
    }
    data.get(key).as_ref().and_then(drf_bool)
}

/// Validated `UserSerializer` data (only provided fields are `Some`).
#[derive(Default, Debug)]
struct UserInput {
    username: Option<String>,
    email: Option<String>,
    password: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    date_joined: Option<DateTime<Utc>>,
    is_staff: Option<bool>,
    is_active: Option<bool>,
    is_superuser: Option<bool>,
    groups: Option<Vec<i64>>,
    user_permissions: Option<Vec<i64>>,
}

async fn username_taken(db: &PgPool, username: &str, exclude: Option<i64>) -> Result<bool, sqlx::Error> {
    let mut q = Query::select();
    q.expr(Expr::val(1)).from(a("auth_user")).and_where(Expr::col(a("username")).eq(username));
    if let Some(id) = exclude {
        q.and_where(Expr::col(a("id")).ne(id));
    }
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    Ok(sqlx::query_with(&sql, values).fetch_optional(db).await?.is_some())
}

/// `UserSerializer(data=..., partial=...).is_valid(raise_exception=True)`.
/// `instance` = (id, attrs before the update) for updates; `request_user` for creates.
async fn validate_user(state: &AppState, data: &Data, partial: bool, instance: Option<i64>, pw_user: &UserAttrs) -> HResult<UserInput> {
    data.require_mapping()?;
    let db = state.db();
    let mut errs = ValidationErrors::new();
    let mut out = UserInput::default();

    // username: CharField(max_length=150, validators=[UnicodeUsernameValidator, UniqueValidator])
    match get_value(data, "username", FieldOpts { partial, required: true, ..Default::default() }) {
        FieldInput::Missing => {
            if !partial {
                errs.add("username", "This field is required.");
            }
        }
        FieldInput::Value(v) => match drf_char(&v, false, None, true) {
            Err(m) => errs.add("username", m),
            Ok(s) => {
                let mut msgs = Vec::new();
                if !is_valid_username(&s) {
                    msgs.push(
                        "Enter a valid username. This value may contain only letters, numbers, and @/./+/-/_ characters.".to_string(),
                    );
                }
                if !s.contains('\0') && username_taken(db, &s, instance).await? {
                    msgs.push("A user with that username already exists.".to_string());
                }
                if s.chars().count() > 150 {
                    msgs.push("Ensure this field has no more than 150 characters.".to_string());
                }
                if s.contains('\0') {
                    msgs.push("Null characters are not allowed.".to_string());
                }
                if msgs.is_empty() {
                    out.username = Some(s);
                } else {
                    errs.set("username", Value::Array(msgs.into_iter().map(Value::String).collect()));
                }
            }
        },
    }

    // email: EmailField(allow_blank=True, max_length=254, required=False)
    if let FieldInput::Value(v) = get_value(data, "email", FieldOpts { partial, allow_blank: true, ..Default::default() }) {
        match drf_char(&v, true, None, true) {
            Err(m) => errs.add("email", m),
            Ok(s) if s.is_empty() => out.email = Some(s),
            Ok(s) => {
                let mut msgs = Vec::new();
                if s.chars().count() > 254 {
                    msgs.push("Ensure this field has no more than 254 characters.".to_string());
                }
                if s.contains('\0') {
                    msgs.push("Null characters are not allowed.".to_string());
                }
                if !is_valid_email(&s) {
                    msgs.push("Enter a valid email address.".to_string());
                }
                if msgs.is_empty() {
                    out.email = Some(s);
                } else {
                    errs.set("email", Value::Array(msgs.into_iter().map(Value::String).collect()));
                }
            }
        }
    }

    // password: ObfuscatedPasswordField(required=False) + validate_password
    if let FieldInput::Value(v) = get_value(data, "password", FieldOpts { partial, ..Default::default() }) {
        match &v {
            Value::String(s) if s.trim().is_empty() => errs.add("password", "This field may not be blank."),
            Value::String(s) => {
                if has_real_password(s) {
                    let msgs = validate_password(s, Some(pw_user));
                    if msgs.is_empty() {
                        out.password = Some(s.clone());
                    } else {
                        errs.set("password", Value::Array(msgs.into_iter().map(Value::String).collect()));
                    }
                } else {
                    out.password = Some(s.clone());
                }
            }
            Value::Null => errs.add("password", "This field may not be null."),
            _ => return Err(server_error("password is not a string")),
        }
    }

    for (name, slot) in [("first_name", &mut out.first_name), ("last_name", &mut out.last_name)] {
        if let FieldInput::Value(v) = get_value(data, name, FieldOpts { partial, allow_blank: true, ..Default::default() }) {
            match drf_char(&v, true, Some(150), true) {
                Ok(s) => *slot = Some(s),
                Err(m) => errs.add(name, m),
            }
        }
    }

    if let FieldInput::Value(v) = get_value(data, "date_joined", FieldOpts { partial, ..Default::default() }) {
        match &v {
            Value::Null => errs.add("date_joined", "This field may not be null."),
            Value::String(s) => match pl_core::datetime::drf_parse_datetime(s, state.tz()) {
                Ok(dt) => out.date_joined = Some(dt),
                Err(m) => errs.add("date_joined", m),
            },
            _ => errs.add(
                "date_joined",
                "Datetime has wrong format. Use one of these formats instead: YYYY-MM-DDThh:mm[:ss[.uuuuuu]][+HH:MM|-HH:MM|Z].",
            ),
        }
    }

    for (name, slot) in [("is_staff", &mut out.is_staff), ("is_active", &mut out.is_active), ("is_superuser", &mut out.is_superuser)] {
        if let FieldInput::Value(v) = get_value(data, name, FieldOpts { partial, bool_field: true, ..Default::default() }) {
            match (&v, drf_bool(&v)) {
                (Value::Null, _) => errs.add(name, "This field may not be null."),
                (_, Some(b)) => *slot = Some(b),
                (_, None) => errs.add(name, "Must be a valid boolean."),
            }
        }
    }

    out.groups = validate_group_pks(db, get_list_value(data, "groups", partial), "groups", false, &mut errs).await?;
    out.user_permissions =
        validate_permission_codenames(db, get_list_value(data, "user_permissions", partial), "user_permissions", false, &mut errs)
            .await?
            .map_err(server_error)?;

    errs.into_result()?;
    Ok(out)
}

async fn set_m2m(
    tx: &mut sqlx::PgConnection,
    table: &str,
    owner_col: &str,
    owner: i64,
    target_col: &str,
    targets: &[i64],
) -> Result<(), sqlx::Error> {
    // `.set(objs)`: remove rows not in targets, add missing ones (order of new rows = input order)
    let (sql, values) = Query::delete()
        .from_table(a(table))
        .and_where(Expr::col(a(owner_col)).eq(owner))
        .and_where(Expr::col(a(target_col)).is_not_in(targets.iter().copied()))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *tx).await?;
    if let Some((sql, values)) = m2m_insert(table, owner_col, owner, target_col, targets)? {
        sqlx::query_with(&sql, values).execute(&mut *tx).await?;
    }
    Ok(())
}

/// Review 2 N12: one multi-row `INSERT … ON CONFLICT DO NOTHING` (Django `.set()` uses
/// `bulk_create`) instead of one statement per target; duplicates dropped, input order kept.
pub(crate) fn m2m_insert(
    table: &str,
    owner_col: &str,
    owner: i64,
    target_col: &str,
    targets: &[i64],
) -> Result<Option<(String, sea_query_binder::SqlxValues)>, sqlx::Error> {
    let mut seen = std::collections::HashSet::new();
    let mut q = Query::insert();
    q.into_table(a(table)).columns([a(owner_col), a(target_col)]);
    let mut any = false;
    for t in targets {
        if !seen.insert(*t) {
            continue;
        }
        q.values([owner.into(), (*t).into()]).map_err(crate::db::qerr)?;
        any = true;
    }
    if !any {
        return Ok(None);
    }
    q.on_conflict(sea_query::OnConflict::columns([a(owner_col), a(target_col)]).do_nothing().to_owned());
    Ok(Some(q.build_sqlx(PostgresQueryBuilder)))
}

async fn hash(pw: String) -> HResult<String> {
    tokio::task::spawn_blocking(move || pl_authz::hashers::make_password(&pw)).await.map_err(server_error)
}

/// `POST /api/users/`
pub async fn create(State(state): State<AppState>, req: Request) -> Response {
    respond(
        async {
            let (r, auth) = begin(&state, req).await?;
            let me = auth.require_model_perms(&USER_MODEL)?.clone();
            let data = r.data().await?;
            if !me.is_superuser {
                if requested_bool(&data, "is_superuser") == Some(true) {
                    return Err(django_text(StatusCode::FORBIDDEN, "Superuser status can only be granted by a superuser"));
                }
                if requested_bool(&data, "is_staff") == Some(true) {
                    return Err(django_text(StatusCode::FORBIDDEN, "Staff status can only be granted by a superuser"));
                }
            }
            // PasswordValidationMixin: instance is None → request.user
            let input = validate_user(&state, &data, false, None, &user_attrs(&me)).await?;
            let password = match input.password.filter(|p| has_real_password(p)) {
                Some(pw) => hash(pw).await?,
                None => String::new(),
            };
            let mut tx = state.db().begin().await?;
            let username = input.username.clone().unwrap_or_default();
            let date_joined: sea_query::SimpleExpr = match input.date_joined {
                Some(d) => Expr::val(d).into(),
                None => Expr::current_timestamp().into(),
            };
            let (sql, values) = Query::insert()
                .into_table(a("auth_user"))
                .columns([
                    a("password"),
                    a("last_login"),
                    a("is_superuser"),
                    a("username"),
                    a("first_name"),
                    a("last_name"),
                    a("email"),
                    a("is_staff"),
                    a("is_active"),
                    a("date_joined"),
                ])
                .values([
                    password.into(),
                    Expr::val(Option::<String>::None).cast_as(Alias::new("timestamptz")),
                    input.is_superuser.unwrap_or(false).into(),
                    username.into(),
                    input.first_name.unwrap_or_default().into(),
                    input.last_name.unwrap_or_default().into(),
                    input.email.unwrap_or_default().into(),
                    input.is_staff.unwrap_or(false).into(),
                    input.is_active.unwrap_or(true).into(),
                    date_joined,
                ])
                .map_err(crate::db::qerr)?
                .returning(Query::returning().column(a("id")))
                .build_sqlx(PostgresQueryBuilder);
            let row = sqlx::query_with(&sql, values)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| if crate::db::is_unique_violation(&e) { server_error("IntegrityError: username") } else { e.into() })?;
            let id: i64 = i64::from(row.try_get::<i32, _>("id")?);
            if let Some(g) = input.groups.filter(|g| !g.is_empty()) {
                set_m2m(&mut tx, "auth_user_groups", "user_id", id, "group_id", &g).await?;
            }
            if let Some(p) = input.user_permissions.filter(|p| !p.is_empty()) {
                set_m2m(&mut tx, "auth_user_user_permissions", "user_id", id, "permission_id", &p).await?;
            }
            tx.commit().await?;
            let body = users_repr(state.db(), state.tz(), &[id]).await?.pop().ok_or_else(|| server_error("user vanished"))?;
            Ok(Drf::created(body).into_response())
        }
        .await,
    )
}

/// `PUT` / `PATCH /api/users/{id}/`
async fn update_impl(state: AppState, raw_id: String, req: Request, partial: bool) -> HResult<Response> {
    let (r, auth) = begin(&state, req).await?;
    let me = auth.require_model_perms(&USER_MODEL)?.clone();
    let db = state.db();
    let id = get_object_id(db, &raw_id).await?;
    let Some(target) = pl_authz::user::load_user_by_id(db, id).await? else {
        return Err(ApiError::not_found_model("User").into());
    };
    if !me.is_superuser && target.is_superuser {
        return Err(django_text(StatusCode::FORBIDDEN, "Superusers can only be modified by other superusers"));
    }
    let data = r.data().await?;
    if !me.is_superuser {
        if let Some(b) = requested_bool(&data, "is_superuser")
            && b != target.is_superuser
        {
            return Err(django_text(StatusCode::FORBIDDEN, "Superuser status can only be changed by a superuser"));
        }
        if let Some(b) = requested_bool(&data, "is_staff")
            && b != target.is_staff
        {
            return Err(django_text(StatusCode::FORBIDDEN, "Staff status can only be changed by a superuser"));
        }
    }
    let input = validate_user(&state, &data, partial, Some(id), &user_attrs(&target)).await?;
    let mut q = Query::update();
    q.table(a("auth_user")).and_where(Expr::col(a("id")).eq(id));
    let mut any = false;
    let mut password_changed = false;
    if let Some(pw) = input.password.filter(|p| has_real_password(p)) {
        q.value(a("password"), hash(pw).await?);
        any = true;
        password_changed = true;
    }
    macro_rules! set {
        ($col:literal, $v:expr) => {
            if let Some(v) = $v {
                q.value(a($col), v);
                any = true;
            }
        };
    }
    set!("username", input.username);
    set!("email", input.email);
    set!("first_name", input.first_name);
    set!("last_name", input.last_name);
    set!("date_joined", input.date_joined);
    set!("is_staff", input.is_staff);
    set!("is_active", input.is_active);
    set!("is_superuser", input.is_superuser);
    let mut tx = db.begin().await?;
    // Group membership, flags and direct permissions change under the user row lock, so rights checks
    // that lock this row (pl-api-v2 folders `fresh_caller`) see the change or finish before it.
    lock_user_row(&mut tx, id).await?;
    if any {
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values)
            .execute(&mut *tx)
            .await
            .map_err(|e| if crate::db::is_unique_violation(&e) { server_error("IntegrityError: username") } else { e.into() })?;
    }
    if let Some(g) = input.groups {
        set_m2m(&mut tx, "auth_user_groups", "user_id", id, "group_id", &g).await?;
    }
    if let Some(p) = input.user_permissions {
        set_m2m(&mut tx, "auth_user_user_permissions", "user_id", id, "permission_id", &p).await?;
    }
    if password_changed {
        // PaperRust: revoke the user's device tokens (`prt_…`) in the same transaction as the
        // password UPDATE; DRF token stays as upstream (security review 2026-11-19 F13). A missing
        // table is handled in a savepoint, so it does not abort this transaction.
        pl_authz::device::revoke_all_for_user_tx(&mut tx, id).await?;
    }
    tx.commit().await?;
    let body = users_repr(db, state.tz(), &[id]).await?.pop().ok_or_else(|| server_error("user vanished"))?;
    Ok(Drf::ok(body).into_response())
}

pub async fn update(State(state): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    respond(update_impl(state, id, req, false).await)
}

pub async fn partial_update(State(state): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    respond(update_impl(state, id, req, true).await)
}

/// `POST /api/users/{id}/deactivate_totp/`
pub async fn deactivate_totp(State(state): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    respond(
        async {
            let (_r, auth) = begin(&state, req).await?;
            let me = auth.require_model_perms(&USER_MODEL)?.clone();
            // User.objects.get(pk=pk): ValueError / DoesNotExist are not handled → 500
            let pk = pl_core::pagination::python_int(&id)
                .and_then(|i| i32::try_from(i).ok())
                .ok_or_else(|| server_error("ValueError: invalid user pk"))?;
            let Some(user) = pl_authz::user::load_user_by_id(state.db(), i64::from(pk)).await? else {
                return Err(server_error("User matching query does not exist."));
            };
            if !me.is_superuser && me.id != user.id {
                return Err(django_text(StatusCode::FORBIDDEN, "You do not have permission to deactivate TOTP for this user"));
            }
            crate::profile::delete_totp_of(&state, user.id).await
        }
        .await,
    )
}

/// m2m through tables and CASCADE relations of `auth_user` (deleted rows), as
/// `(table, column)`; `account_emailconfirmation` / `socialaccount_socialtoken` hang off rows
/// deleted here (see [`destroy`]).
pub const USER_CASCADE: &[(&str, &str)] = &[
    ("auth_user_groups", "user_id"),
    ("auth_user_user_permissions", "user_id"),
    ("documents_workflowaction_assign_view_users", "user_id"),
    ("documents_workflowaction_assign_change_users", "user_id"),
    ("documents_workflowaction_remove_owners", "user_id"),
    ("documents_workflowaction_remove_view_users", "user_id"),
    ("documents_workflowaction_remove_change_users", "user_id"),
    ("account_emailaddress", "user_id"),
    ("django_admin_log", "user_id"),
    ("authtoken_token", "user_id"),
    ("documents_uisettings", "user_id"),
    ("guardian_userobjectpermission", "user_id"),
    ("mfa_authenticator", "user_id"),
    ("socialaccount_socialaccount", "user_id"),
];

/// `on_delete=SET_NULL` relations of `auth_user` (`ModelWithOwner.owner`, `Note.user`,
/// `ShareLink(Bundle).owner`, `WorkflowAction.assign_owner`, auditlog `LogEntry.actor`).
/// PaperRust's own `paperrust_*` tables use `ON DELETE SET NULL` in the database.
pub const USER_SET_NULL: &[(&str, &str)] = &[
    ("auditlog_logentry", "actor_id"),
    ("documents_correspondent", "owner_id"),
    ("documents_documenttype", "owner_id"),
    ("documents_storagepath", "owner_id"),
    ("documents_tag", "owner_id"),
    ("documents_document", "owner_id"),
    ("documents_note", "user_id"),
    ("documents_savedview", "owner_id"),
    ("documents_sharelink", "owner_id"),
    ("documents_sharelinkbundle", "owner_id"),
    ("documents_paperlesstask", "owner_id"),
    ("documents_workflowaction", "assign_owner_id"),
    ("paperless_mail_mailaccount", "owner_id"),
    ("paperless_mail_mailrule", "owner_id"),
    ("paperless_mail_processedmail", "owner_id"),
];

/// `cleanup_user_deletion` for a user: `default_owner == pk` → `None`, first occurrence of pk
/// removed from `default_view_users` / `default_change_users`. `Err` = Python exception text.
pub fn cleanup_user_permissions(settings: &mut Value, pk: i64) -> Result<bool, String> {
    let perms = match settings {
        Value::Object(m) => match m.get_mut("permissions") {
            None => return Ok(false),
            Some(Value::Object(p)) => p,
            Some(other) => return Err(format!("'{}' object has no attribute 'get'", crate::http::py_type_name(other))),
        },
        other => return Err(format!("'{}' object has no attribute 'get'", crate::http::py_type_name(other))),
    };
    let same = |v: &Value| v.as_i64() == Some(pk) || v.as_f64() == Some(pk as f64);
    let mut updated = false;
    if perms.get("default_owner").is_some_and(same) {
        perms.insert("default_owner".into(), Value::Null);
        updated = true;
    }
    for key in ["default_view_users", "default_change_users"] {
        if let Some(Value::Array(list)) = perms.get_mut(key)
            && let Some(pos) = list.iter().position(same)
        {
            list.remove(pos);
            updated = true;
        }
    }
    Ok(updated)
}

/// `DELETE /api/users/{id}/` – `UserViewSet.destroy`: model permission `auth.delete_user`,
/// `get_object()` (404; `User` has no owner → object permission passes), superuser guard, then
/// Django's Collector (m2m/CASCADE rows deleted, SET_NULL columns cleared, then the user) and
/// `cleanup_user_deletion` (post_delete) in one transaction. 204 without body.
pub async fn destroy(State(state): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    respond(async {
        let (_r, auth) = begin(&state, req).await?;
        let me = auth.require_model_perms(&USER_MODEL)?.clone();
        let db = state.db();
        let id = get_object_id(db, &id).await?;
        let Some(target) = pl_authz::user::load_user_by_id(db, id).await? else {
            return Err(ApiError::not_found_model("User").into());
        };
        if !me.is_superuser && target.is_superuser {
            return Err(django_text(StatusCode::FORBIDDEN, "Superusers can only be deleted by other superusers"));
        }
        let mut tx = db.begin().await?;
        // Group membership, flags and direct permissions change under the user row lock, so rights checks
        // that lock this row (pl-api-v2 folders `fresh_caller`) see the change or finish before it.
        lock_user_row(&mut tx, id).await?;
        let names: Vec<String> = USER_CASCADE
            .iter()
            .chain(USER_SET_NULL)
            .map(|(t, _)| (*t).to_string())
            .chain(["account_emailconfirmation".to_string(), "socialaccount_socialtoken".to_string()])
            .collect();
        let existing: Vec<String> = sqlx::query_scalar(
            "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p', 'v') AND c.relname = ANY($1)",
        )
        .bind(&names)
        .fetch_all(&mut *tx)
        .await?;
        let has = |t: &str| existing.iter().any(|e| e == t);
        let uid = i32::try_from(id).map_err(|_| server_error("user id out of range"))?;
        // Children of cascaded rows first (EmailConfirmation → EmailAddress, SocialToken → SocialAccount).
        if has("account_emailconfirmation") && has("account_emailaddress") {
            sqlx::query("DELETE FROM account_emailconfirmation WHERE email_address_id IN (SELECT id FROM account_emailaddress WHERE user_id = $1)")
                .bind(uid)
                .execute(&mut *tx)
                .await?;
        }
        if has("socialaccount_socialtoken") && has("socialaccount_socialaccount") {
            sqlx::query("DELETE FROM socialaccount_socialtoken WHERE account_id IN (SELECT id FROM socialaccount_socialaccount WHERE user_id = $1)")
                .bind(uid)
                .execute(&mut *tx)
                .await?;
        }
        for (table, col) in USER_CASCADE {
            if !has(table) {
                continue;
            }
            let (sql, values) = Query::delete()
                .from_table(a(table))
                .and_where(Expr::col(a(col)).eq(uid))
                .build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, values).execute(&mut *tx).await?;
        }
        for (table, col) in USER_SET_NULL {
            if !has(table) {
                continue;
            }
            let (sql, values) = Query::update()
                .table(a(table))
                .value(a(col), Option::<i32>::None)
                .and_where(Expr::col(a(col)).eq(uid))
                .build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, values).execute(&mut *tx).await?;
        }
        let (sql, values) = Query::delete()
            .from_table(a("auth_user"))
            .and_where(Expr::col(a("id")).eq(uid))
            .build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values).execute(&mut *tx).await?;

        // post_delete → cleanup_user_deletion(User)
        let rows = sqlx::query("SELECT id::int8 AS id, settings FROM documents_uisettings ORDER BY id").fetch_all(&mut *tx).await?;
        for row in &rows {
            let ui_id: i64 = row.try_get("id")?;
            let settings: Option<sqlx::types::Json<Value>> = row.try_get("settings")?;
            let mut settings = settings.map_or(Value::Null, |j| j.0);
            match cleanup_user_permissions(&mut settings, id) {
                Ok(true) => {
                    sqlx::query("UPDATE documents_uisettings SET settings = $1::jsonb WHERE id = $2")
                        .bind(settings.to_string())
                        .bind(ui_id)
                        .execute(&mut *tx)
                        .await?;
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::error!(target: "paperless.handlers", "Error while cleaning up user {id} ({}) from ui_settings: {e}", target.username);
                }
            }
        }
        tx.commit().await?;
        Ok(pl_core::json::drf_no_content())
    }
    .await)
}

/// Row lock taken by every writer of this user's rights (see pl-api-v2 folders `fresh_caller`).
async fn lock_user_row(tx: &mut sqlx::PgConnection, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT 1 FROM auth_user WHERE id = $1::int8 FOR UPDATE").bind(id).execute(&mut *tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_cleanup_like_upstream() {
        let mut s = serde_json::json!({"permissions": {"default_owner": 4, "default_view_users": [4, 2, 4], "default_change_users": [1]}});
        assert_eq!(cleanup_user_permissions(&mut s, 4), Ok(true));
        assert_eq!(
            s,
            serde_json::json!({"permissions": {"default_owner": null, "default_view_users": [2, 4], "default_change_users": [1]}})
        );
        let mut s = serde_json::json!({"x": 1});
        assert_eq!(cleanup_user_permissions(&mut s, 4), Ok(false));
        assert!(cleanup_user_permissions(&mut Value::Null, 4).unwrap_err().contains("NoneType"));
    }

    /// Review 2 N12: a single INSERT with one row per distinct target, in input order.
    #[test]
    fn m2m_insert_is_one_statement() {
        let (sql, values) = m2m_insert("auth_user_groups", "user_id", 7, "group_id", &[3, 1, 3, 2]).unwrap().unwrap();
        assert_eq!(sql.matches("INSERT").count(), 1);
        assert!(sql.contains("ON CONFLICT"));
        assert_eq!(values.0.0.len(), 6);
        assert!(m2m_insert("auth_user_groups", "user_id", 7, "group_id", &[]).unwrap().is_none());
    }
    use serde_json::json;

    #[test]
    fn requested_bool_like_upstream() {
        let d = Data::Json(json!({"is_superuser": "yes", "is_staff": "maybe"}));
        assert_eq!(requested_bool(&d, "is_superuser"), Some(true));
        assert_eq!(requested_bool(&d, "is_staff"), None);
        assert_eq!(requested_bool(&d, "missing"), None);
    }

    #[test]
    fn base_query_excludes_system_users() {
        let (sql, _) = base_select().build(PostgresQueryBuilder);
        assert!(sql.contains("NOT IN"), "{sql}");
    }
}
