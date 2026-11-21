//! `/api/mail_accounts/` – MailAccountViewSet.
//!
//! upstream: src/paperless_mail/views.py@v3.4.2 (MailAccountViewSet: list, retrieve, create,
//! update, partial_update, destroy, test, process), src/paperless_mail/serialisers.py@v3.4.2
//! (ObfuscatedPasswordField, MailAccountSerializer), src/paperless_mail/models.py (MailAccount),
//! src/documents/serialisers.py (OwnedObjectSerializer.create/update/validate_unique_together).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use pl_core::AppState;
use serde_json::{Map, Value, json};
use sqlx::PgPool;

use crate::body::{as_object, parse_body};
use crate::ctx::{self, MailUser};
use crate::drf::{self, CharConv, Errors, FieldError, Got, OPTIONAL, OPTIONAL_NULL, REQUIRED, raw};
use crate::error::{ApiError, ApiResult, json_response};
use crate::imap::{self, BeforeLogin, ConnectOptions, TestAccount, TestError};
use crate::listing::{self, ReqInfo};
use crate::perms::{self, MAIL_ACCOUNT, PermFields, ValidSetPermissions};
use crate::{celery, oauth};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountRow {
    pub id: i32,
    pub name: String,
    pub imap_server: String,
    pub imap_port: Option<i32>,
    pub imap_security: i16,
    pub username: String,
    pub password: String,
    pub character_set: String,
    pub is_token: bool,
    pub owner_id: Option<i32>,
    pub account_type: i16,
    pub refresh_token: Option<String>,
    pub expiration: Option<DateTime<Utc>>,
}

const COLS: &str = "a.id, a.name, a.imap_server, a.imap_port, a.imap_security, a.username, a.password, \
    a.character_set, a.is_token, a.owner_id, a.account_type, a.refresh_token, a.expiration";

/// `ObfuscatedPasswordField.to_representation`.
pub fn obfuscate(pw: &str) -> String {
    "*".repeat(pw.chars().count().max(10))
}

fn represent(r: &AccountRow, pf: &PermFields, tz: chrono_tz::Tz) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(r.id));
    m.insert("name".into(), json!(r.name));
    m.insert("imap_server".into(), json!(r.imap_server));
    m.insert("imap_port".into(), json!(r.imap_port));
    m.insert("imap_security".into(), json!(r.imap_security));
    m.insert("username".into(), json!(r.username));
    m.insert("password".into(), json!(obfuscate(&r.password)));
    m.insert("character_set".into(), json!(r.character_set));
    m.insert("is_token".into(), json!(r.is_token));
    m.insert("owner".into(), json!(r.owner_id));
    pf.insert(&mut m, r.id);
    m.insert("account_type".into(), json!(r.account_type));
    m.insert("expiration".into(), drf::fmt_opt_datetime(&r.expiration, tz));
    Value::Object(m)
}

async fn represent_one(db: &PgPool, user: &MailUser, info: &ReqInfo, r: &AccountRow) -> ApiResult<Value> {
    let pf = PermFields::load(db, user, MAIL_ACCOUNT, info.full_perms(), &[(r.id, r.owner_id)]).await?;
    Ok(represent(r, &pf, info.tz))
}

// ---------------------------------------------------------------------------------------------
// Loading

async fn fetch(db: &PgPool, id: i32) -> ApiResult<Option<AccountRow>> {
    let sql = format!("SELECT {COLS} FROM paperless_mail_mailaccount a WHERE a.id = $1");
    Ok(sqlx::query_as(&sql).bind(id).fetch_optional(db).await?)
}

/// `get_object()`: visibility (view) → 404, then object permission for `method`.
pub async fn get_object(db: &PgPool, user: &MailUser, method: &Method, raw_pk: &str) -> ApiResult<AccountRow> {
    let Some(pk) = crate::util::url_pk(raw_pk)? else {
        return Err(ApiError::not_found_model(MAIL_ACCOUNT.object_name));
    };
    let visible = perms::permitted_among(db, user, MAIL_ACCOUNT, "view_mailaccount", &[pk]).await?;
    if !visible.contains(&pk) {
        return Err(ApiError::not_found_model(MAIL_ACCOUNT.object_name));
    }
    let row = fetch(db, pk).await?.ok_or_else(|| ApiError::not_found_model(MAIL_ACCOUNT.object_name))?;
    let ct = perms::content_type_id(db, MAIL_ACCOUNT).await?;
    perms::check_object_permission(db, user, method, MAIL_ACCOUNT, ct, row.id, row.owner_id).await?;
    Ok(row)
}

// ---------------------------------------------------------------------------------------------
// Serializer (input)

#[derive(Debug, Clone, PartialEq)]
pub struct Password {
    pub text: String,
    /// upstream keeps the raw value; non-strings lead to a 500 at `.replace`.
    pub is_str: bool,
}

impl Password {
    pub fn only_stars(&self) -> bool {
        self.text.chars().all(|c| c == '*')
    }
}

#[derive(Debug)]
pub struct AccountInput {
    pub name: Got<String>,
    pub imap_server: Got<String>,
    pub imap_port: Got<i128>,
    pub imap_security: Got<i16>,
    pub username: Got<String>,
    pub password: Got<Password>,
    pub character_set: Got<String>,
    pub is_token: Got<bool>,
    pub owner: Got<Option<i32>>,
    pub set_permissions: Got<ValidSetPermissions>,
    pub account_type: Got<i16>,
    pub expiration: Got<Option<DateTime<Utc>>>,
}

fn nn<T>(g: Got<Option<T>>) -> Got<T> {
    match g {
        Got::Set(Some(v)) => Got::Set(v),
        _ => Got::Skip,
    }
}

/// Simple (synchronous) field.
fn field<T>(
    e: &mut Errors,
    data: &Map<String, Value>,
    name: &str,
    opts: drf::Opts,
    partial: bool,
    parse: impl FnOnce(&Value) -> Result<T, FieldError>,
) -> Got<Option<T>> {
    match drf::empty_values::<T>(raw(data, name), opts, partial, None) {
        Err(fe) => {
            e.add(name, fe);
            Got::Skip
        }
        Ok(Err(got)) => got,
        Ok(Ok(v)) => match parse(v) {
            Ok(t) => Got::Set(Some(t)),
            Err(fe) => {
                e.add(name, fe);
                Got::Skip
            }
        },
    }
}

async fn name_exists(db: &PgPool, name: &str, exclude: Option<i32>) -> ApiResult<bool> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM paperless_mail_mailaccount WHERE name = $1 AND ($2::int IS NULL OR id <> $2) LIMIT 1")
            .bind(name)
            .bind(exclude)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

/// `MailAccountSerializer(data=..., partial=...).is_valid()`.
pub async fn validate(
    db: &PgPool,
    data: &Map<String, Value>,
    partial: bool,
    instance: Option<i32>,
    tz: chrono_tz::Tz,
) -> ApiResult<AccountInput> {
    let mut e = Errors::default();

    // name: [UniqueValidator, MaxLength(256), ProhibitNull]
    let name = match drf::empty_values::<String>(raw(data, "name"), REQUIRED, partial, None) {
        Err(fe) => {
            e.add("name", fe);
            Got::Skip
        }
        Ok(Err(g)) => nn(g),
        Ok(Ok(v)) => match drf::char_convert(v, false) {
            Err(fe) => {
                e.add("name", fe);
                Got::Skip
            }
            Ok(CharConv::Blank) => Got::Set(String::new()),
            Ok(CharConv::Value(s)) => {
                let mut errs = Vec::new();
                if name_exists(db, &s, instance).await? {
                    errs.push(json!("mail account with this name already exists."));
                }
                errs.extend(drf::char_validators(&s, Some(256)));
                if errs.is_empty() {
                    Got::Set(s)
                } else {
                    e.add("name", Value::Array(errs));
                    Got::Skip
                }
            }
        },
    };
    let char256 = drf::CharOpts { allow_blank: false, max_length: Some(256) };
    let imap_server = nn(field(&mut e, data, "imap_server", REQUIRED, partial, |v| drf::char_field(v, char256)));
    let imap_port = nn(field(&mut e, data, "imap_port", REQUIRED, partial, |v| drf::integer_field(v, None, None)));
    let imap_security = nn(field(&mut e, data, "imap_security", OPTIONAL, partial, |v| drf::choice_field(v, &[1, 2, 3])));
    let username = nn(field(&mut e, data, "username", REQUIRED, partial, |v| drf::char_field(v, char256)));
    // ObfuscatedPasswordField: CharField empty check, to_internal_value = raw value
    let password = nn(field(&mut e, data, "password", REQUIRED, partial, |v| {
        let text = crate::pyval::to_str(v);
        if crate::pyval::py_strip(&text).is_empty() {
            return Err(drf::err(drf::MSG_BLANK));
        }
        if text.contains('\0') {
            return Err(drf::err(drf::MSG_NULL_CHARS));
        }
        Ok(Password { text, is_str: v.is_string() })
    }));
    let character_set = nn(field(&mut e, data, "character_set", OPTIONAL, partial, |v| drf::char_field(v, char256)));
    let is_token = nn(field(&mut e, data, "is_token", OPTIONAL, partial, drf::boolean_field));
    // owner: PrimaryKeyRelatedField(User), allow_null
    let owner = match drf::empty_values::<i32>(drf::raw_related(data, "owner"), OPTIONAL_NULL, partial, None) {
        Err(fe) => {
            e.add("owner", fe);
            Got::Skip
        }
        Ok(Err(g)) => g,
        Ok(Ok(v)) => match perms::resolve_user_pk(db, v).await {
            Ok(id) => Got::Set(Some(id)),
            Err(fe) => {
                e.add("owner", fe);
                Got::Skip
            }
        },
    };
    let set_permissions = crate::util::set_permissions_field(db, &mut e, data, partial).await;
    let account_type = nn(field(&mut e, data, "account_type", OPTIONAL, partial, |v| drf::choice_field(v, &[1, 2, 3])));
    let expiration = field(&mut e, data, "expiration", OPTIONAL_NULL, partial, |v| drf::datetime_field(v, tz));

    if !e.is_empty() {
        return Err(ApiError::bad_request(e.into_value()));
    }
    Ok(AccountInput {
        name,
        imap_server,
        imap_port,
        imap_security,
        username,
        password,
        character_set,
        is_token,
        owner,
        set_permissions,
        account_type,
        expiration,
    })
}

fn port_i32(p: i128) -> ApiResult<i32> {
    // upstream: no range validator; DB constraint/overflow → 500
    if p < 0 {
        return Err(ApiError::internal("imap_port violates check constraint"));
    }
    i32::try_from(p).map_err(|_| ApiError::internal("imap_port out of range"))
}

async fn owner_name_conflict(db: &PgPool, owner: Option<i32>, name: &str, exclude: Option<i32>) -> ApiResult<bool> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM paperless_mail_mailaccount WHERE owner_id IS NOT DISTINCT FROM $1 AND name = $2 \
         AND ($3::int IS NULL OR id <> $3) LIMIT 1",
    )
    .bind(owner)
    .bind(name)
    .bind(exclude)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

fn unique_together_error() -> ApiError {
    ApiError::bad_request(json!({"error": "Object violates owner / name unique constraint"}))
}

// ---------------------------------------------------------------------------------------------
// Handler

/// GET /api/mail_accounts/
async fn list(State(st): State<AppState>, info: ReqInfo, user: MailUser) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::GET, MAIL_ACCOUNT).await?;
    let ct = perms::content_type_id(db, MAIL_ACCOUNT).await?;
    let (cond, binds) = crate::util::visible_where(&user, "a", MAIL_ACCOUNT, 1);
    let count_sql = format!("SELECT COUNT(*) FROM paperless_mail_mailaccount a WHERE {cond}");
    let count: (i64,) = crate::util::bind_vis(sqlx::query_as(&count_sql), &user, binds, MAIL_ACCOUNT, ct).fetch_one(db).await?;
    let page = listing::paginate(&info, count.0)?;
    let sql = format!(
        "SELECT {COLS} FROM paperless_mail_mailaccount a WHERE {cond} ORDER BY a.id ASC LIMIT {} OFFSET {}",
        page.size,
        page.offset()
    );
    let rows: Vec<AccountRow> = crate::util::bind_vis(sqlx::query_as(&sql), &user, binds, MAIL_ACCOUNT, ct).fetch_all(db).await?;
    let all_ids = if info.api_version < 10 {
        let sql = format!("SELECT a.id FROM paperless_mail_mailaccount a WHERE {cond} ORDER BY a.id ASC");
        let ids: Vec<(i32,)> = crate::util::bind_vis(sqlx::query_as(&sql), &user, binds, MAIL_ACCOUNT, ct).fetch_all(db).await?;
        Some(ids.into_iter().map(|r| r.0).collect())
    } else {
        None
    };
    let objs: Vec<(i32, Option<i32>)> = rows.iter().map(|r| (r.id, r.owner_id)).collect();
    let pf = PermFields::load(db, &user, MAIL_ACCOUNT, info.full_perms(), &objs).await?;
    let results = rows.iter().map(|r| represent(r, &pf, info.tz)).collect();
    Ok(json_response(StatusCode::OK, &listing::paginated(&info, &page, results, all_ids)))
}

/// GET /api/mail_accounts/{id}/
async fn retrieve(State(st): State<AppState>, info: ReqInfo, user: MailUser, Path(id): Path<String>) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::GET, MAIL_ACCOUNT).await?;
    let row = get_object(db, &user, &Method::GET, &id).await?;
    Ok(json_response(StatusCode::OK, &represent_one(db, &user, &info, &row).await?))
}

/// POST /api/mail_accounts/
async fn create(State(st): State<AppState>, info: ReqInfo, user: MailUser, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::POST, MAIL_ACCOUNT).await?;
    let data = as_object(parse_body(&headers, &body)?)?;
    let inp = validate(db, &data, false, None, info.tz).await?;
    // OwnedObjectSerializer.create: owner missing in the request → request.user
    let owner = if !data.contains_key("owner") {
        Some(user.id)
    } else {
        match &inp.owner {
            Got::Set(o) => *o,
            Got::Skip => None,
        }
    };
    let Got::Set(name) = &inp.name else {
        return Err(ApiError::internal("name missing after validation"));
    };
    if owner_name_conflict(db, owner, name, None).await? {
        return Err(unique_together_error());
    }
    let port = match inp.imap_port {
        Got::Set(p) => port_i32(p)?,
        Got::Skip => return Err(field_error("imap_port", "This field is required.")),
    };
    let s = |g: &Got<String>, d: &str| match g {
        Got::Set(v) => v.clone(),
        Got::Skip => d.to_string(),
    };
    let password = match &inp.password {
        Got::Set(p) => p.text.clone(),
        Got::Skip => String::new(),
    };
    let mut tx = db.begin().await?;
    let (id,): (i32,) = sqlx::query_as(
        "INSERT INTO paperless_mail_mailaccount \
         (name, imap_server, imap_port, imap_security, username, password, is_token, character_set, \
          account_type, refresh_token, expiration, owner_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, $10, $11) RETURNING id",
    )
    .bind(name)
    .bind(s(&inp.imap_server, ""))
    .bind(port)
    .bind(match inp.imap_security {
        Got::Set(v) => v,
        Got::Skip => 2,
    })
    .bind(s(&inp.username, ""))
    .bind(password)
    .bind(match inp.is_token {
        Got::Set(v) => v,
        Got::Skip => false,
    })
    .bind(s(&inp.character_set, "UTF-8"))
    .bind(match inp.account_type {
        Got::Set(v) => v,
        Got::Skip => 1,
    })
    .bind(match inp.expiration {
        Got::Set(v) => v,
        Got::Skip => None,
    })
    .bind(owner)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    if let Got::Set(sp) = &inp.set_permissions {
        let mut tx = db.begin().await?;
        perms::apply_set_permissions(&mut tx, MAIL_ACCOUNT, id, sp).await?;
        tx.commit().await?;
    }
    let row = fetch(db, id).await?.ok_or_else(|| ApiError::internal("created row vanished"))?;
    Ok(json_response(StatusCode::CREATED, &represent_one(db, &user, &info, &row).await?))
}

async fn update_impl(
    st: AppState,
    info: ReqInfo,
    user: MailUser,
    id: String,
    headers: HeaderMap,
    body: Bytes,
    method: Method,
) -> ApiResult<Response> {
    let db = &st.inner.db;
    let partial = method == Method::PATCH;
    perms::require_model_perm(db, &user, &method, MAIL_ACCOUNT).await?;
    let mut row = get_object(db, &user, &method, &id).await?;
    let data = as_object(parse_body(&headers, &body)?)?;
    let inp = validate(db, &data, partial, Some(row.id), info.tz).await?;

    // MailAccountSerializer.update: ignore an all-asterisks password
    let mut password = inp.password.clone();
    if let Got::Set(p) = &password {
        if !p.is_str {
            return Err(ApiError::internal("password.replace on non-str"));
        }
        if p.only_stars() {
            password = Got::Skip;
        }
    }
    // OwnedObjectSerializer.update: permissions for owner/set_permissions
    let owner_changes = matches!(&inp.owner, Got::Set(o) if *o != row.owner_id);
    let has_sp = matches!(inp.set_permissions, Got::Set(_));
    let is_owner = row.owner_id == Some(user.id);
    if (owner_changes || has_sp) && !(user.is_superuser || is_owner || row.owner_id.is_none()) {
        return Err(ApiError::detail(StatusCode::FORBIDDEN, "Insufficient permissions."));
    }
    if let Got::Set(sp) = &inp.set_permissions {
        let mut tx = db.begin().await?;
        perms::apply_set_permissions(&mut tx, MAIL_ACCOUNT, row.id, sp).await?;
        tx.commit().await?;
    }
    if let Got::Set(owner) = &inp.owner {
        let name = match &inp.name {
            Got::Set(n) => n.clone(),
            Got::Skip => row.name.clone(),
        };
        if owner_name_conflict(db, *owner, &name, Some(row.id)).await? {
            return Err(unique_together_error());
        }
    }
    if let Got::Set(v) = &inp.name {
        row.name = v.clone();
    }
    if let Got::Set(v) = &inp.imap_server {
        row.imap_server = v.clone();
    }
    if let Got::Set(v) = inp.imap_port {
        row.imap_port = Some(port_i32(v)?);
    }
    if let Got::Set(v) = inp.imap_security {
        row.imap_security = v;
    }
    if let Got::Set(v) = &inp.username {
        row.username = v.clone();
    }
    if let Got::Set(v) = &password {
        row.password = v.text.clone();
    }
    if let Got::Set(v) = &inp.character_set {
        row.character_set = v.clone();
    }
    if let Got::Set(v) = inp.is_token {
        row.is_token = v;
    }
    if let Got::Set(v) = inp.owner {
        row.owner_id = v;
    }
    if let Got::Set(v) = inp.account_type {
        row.account_type = v;
    }
    if let Got::Set(v) = inp.expiration {
        row.expiration = v;
    }
    // instance.save(): all columns
    sqlx::query(
        "UPDATE paperless_mail_mailaccount SET name = $2, imap_server = $3, imap_port = $4, \
         imap_security = $5, username = $6, password = $7, is_token = $8, character_set = $9, \
         account_type = $10, refresh_token = $11, expiration = $12, owner_id = $13 WHERE id = $1",
    )
    .bind(row.id)
    .bind(&row.name)
    .bind(&row.imap_server)
    .bind(row.imap_port)
    .bind(row.imap_security)
    .bind(&row.username)
    .bind(&row.password)
    .bind(row.is_token)
    .bind(&row.character_set)
    .bind(row.account_type)
    .bind(&row.refresh_token)
    .bind(row.expiration)
    .bind(row.owner_id)
    .execute(db)
    .await?;
    Ok(json_response(StatusCode::OK, &represent_one(db, &user, &info, &row).await?))
}

/// PUT /api/mail_accounts/{id}/
async fn update(
    State(st): State<AppState>,
    info: ReqInfo,
    user: MailUser,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    update_impl(st, info, user, id, headers, body, Method::PUT).await
}

/// PATCH /api/mail_accounts/{id}/
async fn partial_update(
    State(st): State<AppState>,
    info: ReqInfo,
    user: MailUser,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    update_impl(st, info, user, id, headers, body, Method::PATCH).await
}

/// DELETE /api/mail_accounts/{id}/ (Django collector: ProcessedMail → tag M2M → rules → account)
async fn destroy(State(st): State<AppState>, _info: ReqInfo, user: MailUser, Path(id): Path<String>) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::DELETE, MAIL_ACCOUNT).await?;
    let row = get_object(db, &user, &Method::DELETE, &id).await?;
    let mut tx = db.begin().await?;
    delete_account_cascade(&mut tx, row.id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Django's collector for `MailAccount.delete()` (review 2 M3): `MailRule.account` is `CASCADE`
/// (`paperless_mail/models.py`), so every rule of the account is handled like `MailRule.delete()`
/// (incl. `documents_workflowtrigger.filter_mailrule_id` → NULL). According to
/// `reference/schema/postgres.sql` there are no further FKs on `paperless_mail_mailaccount`.
pub(crate) const ACCOUNT_DELETE_SQL: &[&str] = &[
    "UPDATE documents_workflowtrigger SET filter_mailrule_id = NULL WHERE filter_mailrule_id IN      (SELECT id FROM paperless_mail_mailrule WHERE account_id = $1)",
    "DELETE FROM paperless_mail_processedmail WHERE rule_id IN      (SELECT id FROM paperless_mail_mailrule WHERE account_id = $1)",
    "DELETE FROM paperless_mail_mailrule_assign_tags WHERE mailrule_id IN      (SELECT id FROM paperless_mail_mailrule WHERE account_id = $1)",
    "DELETE FROM paperless_mail_mailrule WHERE account_id = $1",
    "DELETE FROM paperless_mail_mailaccount WHERE id = $1",
];

/// Runs [`ACCOUNT_DELETE_SQL`] inside the caller's transaction.
pub async fn delete_account_cascade(conn: &mut sqlx::PgConnection, account_id: i32) -> Result<(), sqlx::Error> {
    for sql in ACCOUNT_DELETE_SQL {
        sqlx::query(sql).bind(account_id).execute(&mut *conn).await?;
    }
    Ok(())
}

const INSUFFICIENT: ApiError = ApiError::Html(StatusCode::FORBIDDEN, "Insufficient permissions");

/// POST /api/mail_accounts/test/
async fn test(State(st): State<AppState>, info: ReqInfo, user: MailUser, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let db = &st.inner.db;
    let cfg = ctx::config(st.config());
    let parsed = parse_body(&headers, &body)?;
    // request.data["name"] = now().isoformat() – upstream raises TypeError on non-objects → 500;
    // PaperRust: 400 like DRF for non-objects (Known differences, B-313).
    let mut data = match parsed {
        Value::Object(m) => m,
        other => return Err(not_a_dict(&other)),
    };
    data.insert("name".into(), Value::String(drf::now_isoformat(info.tz)));
    let inp = validate(db, &data, false, None, info.tz).await?;

    let mut existing: Option<AccountRow> = None;
    match data.get("id") {
        None | Some(Value::Null) => {
            if !perms::has_global_perm(db, &user, "add_mailaccount").await? {
                return Err(INSUFFICIENT);
            }
        }
        Some(raw_id) => {
            let Some(pk) = perms::pk_from_raw(raw_id) else {
                return Err(INSUFFICIENT);
            };
            let Some(acc) = fetch(db, pk).await? else {
                return Err(INSUFFICIENT);
            };
            let ct = perms::content_type_id(db, MAIL_ACCOUNT).await?;
            if !perms::has_global_perm(db, &user, "change_mailaccount").await?
                || !perms::has_perms_owner_aware(db, &user, "change_mailaccount", ct, acc.id, acc.owner_id).await?
            {
                return Err(INSUFFICIENT);
            }
            existing = Some(acc);
        }
    }
    // upstream: MailAccount(**validated_data) with set_permissions → TypeError → 500. For a
    // connection test permissions are irrelevant → PaperRust ignores the field (Known differences).
    let Got::Set(pw) = &inp.password else {
        return Err(field_error("password", "This field is required."));
    };
    if !pw.is_str {
        return Err(field_error("password", "Not a valid string."));
    }
    let mut expiration = match inp.expiration {
        Got::Set(v) => v,
        Got::Skip => None,
    };
    let port = match inp.imap_port {
        Got::Set(p) => p,
        Got::Skip => return Err(field_error("imap_port", "This field is required.")),
    };
    let is_token = matches!(inp.is_token, Got::Set(true));
    let mut acc = TestAccount {
        imap_server: match &inp.imap_server {
            Got::Set(s) => s.clone(),
            Got::Skip => String::new(),
        },
        // Ports outside u16: upstream OverflowError on connect → 500; PaperRust 400
        imap_port: u16::try_from(port).map_err(|_| field_error("imap_port", "Ensure this value is between 0 and 65535."))?,
        imap_security: match inp.imap_security {
            Got::Set(v) => v,
            Got::Skip => 2,
        },
        username: match &inp.username {
            Got::Set(s) => s.clone(),
            Got::Skip => String::new(),
        },
        password: pw.text.clone(),
        is_token,
    };
    // Security review 2026-11-19: use stored credentials (password or OAuth refresh) only for the
    // stored target. With "****", upstream also uses the stored password for a different
    // server/port/user – that would allow sending it to a foreign host.
    let same_target = existing.as_ref().is_some_and(|a| stored_target_matches(a, &acc));
    if pw.only_stars()
        && let Some(stored) = &existing
    {
        if !same_target {
            return Err(field_error("password", STORED_PASSWORD_OTHER_TARGET));
        }
        acc.password = stored.password.clone();
        expiration = stored.expiration;
    }
    let opts = ConnectOptions {
        allow_internal_hosts: cfg.allow_internal_hosts,
        certificate_file: cfg.certificate_file.clone(),
        timeout: cfg.imap_timeout,
        extra_roots: crate::util::test_extra_roots(),
    };
    let needs_refresh = same_target && is_token && expiration.is_some_and(|e| e < Utc::now());
    let stored =
        existing.as_ref().map(|a| oauth::StoredAccount { id: a.id, account_type: a.account_type, refresh_token: a.refresh_token.clone() });
    let db2 = db.clone();
    let result = imap::test_connection(acc, &opts, |mut acc| async move {
        if !needs_refresh {
            return BeforeLogin::Proceed(acc);
        }
        let Some(stored) = stored else { return BeforeLogin::Proceed(acc) };
        match oauth::refresh_account_oauth_token(&db2, &stored, cfg).await {
            Ok(Some(new_pw)) => {
                acc.password = new_pw;
                BeforeLogin::Proceed(acc)
            }
            Ok(None) => {
                tracing::error!(target: "pl_mail", "Mail account connectivity test failed: Unable to refresh oauth token");
                BeforeLogin::Fail("Unable to refresh oauth token".into())
            }
            Err(e) => BeforeLogin::Error(format!("{e:?}")),
        }
    })
    .await;
    test_result_response(result)
}

/// Result of the connection test → response. upstream: only `MailError` → 400, all other
/// exceptions (connection refused, DNS, timeout, TLS) → 500. PaperRust: every failure → 400
/// "Unable to connect to server" (A-230/B-313, Known differences); the reason is logged
/// without credentials.
fn test_result_response(result: Result<(), TestError>) -> ApiResult<Response> {
    match result {
        Ok(()) => Ok(json_response(StatusCode::OK, &json!({"success": true}))),
        Err(TestError::Mail(msg) | TestError::Other(msg)) => {
            tracing::error!(target: "pl_mail", "Mail account connectivity test failed ({})", crate::util::redact(&msg));
            Err(ApiError::Html(StatusCode::BAD_REQUEST, "Unable to connect to server"))
        }
    }
}

/// Error text when the test with "****" names a different target than the stored account
/// (security review 2026-11-19, Known differences pl-mail).
const STORED_PASSWORD_OTHER_TARGET: &str =
    "The stored password can only be used with the saved server, port, security and username. Please enter the password again.";

/// Does the test name the same target as the stored account (server case-insensitive, port,
/// security, user)? Only then may stored credentials be used
/// (security review 2026-11-19).
fn stored_target_matches(stored: &AccountRow, t: &TestAccount) -> bool {
    stored.imap_server.trim().eq_ignore_ascii_case(t.imap_server.trim())
        && stored.imap_port == Some(i32::from(t.imap_port))
        && stored.imap_security == t.imap_security
        && stored.username == t.username
}

fn field_error(field: &str, msg: &str) -> ApiError {
    ApiError::bad_request(json!({ field: [msg] }))
}

/// DRF `Serializer.to_internal_value` for non-objects.
fn not_a_dict(v: &Value) -> ApiError {
    let kind = match v {
        Value::Array(_) => "list",
        Value::String(_) => "str",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        _ => "NoneType",
    };
    ApiError::bad_request(json!({ "non_field_errors": [format!("Invalid data. Expected a dictionary, but got {kind}.")] }))
}

/// POST /api/mail_accounts/{id}/process/
async fn process(State(st): State<AppState>, _info: ReqInfo, user: MailUser, Path(id): Path<String>) -> ApiResult<Response> {
    let db = &st.inner.db;
    perms::require_model_perm(db, &user, &Method::POST, MAIL_ACCOUNT).await?;
    let row = get_object(db, &user, &Method::POST, &id).await?;
    celery::send_process_mail_accounts(&st, &[row.id]).await?;
    Ok(json_response(StatusCode::OK, &json!({"result": "OK"})))
}

pub fn routes() -> Vec<pl_core::ApiRoute> {
    use axum::routing::{delete, get, patch, post, put};
    use pl_core::ApiRoute;
    use pl_core::route::{ALLOW_MODEL_DETAIL, ALLOW_MODEL_LIST, ALLOW_POST_ONLY, drf_view};
    const SRC: &str = "src/paperless_mail/views.py::MailAccountViewSet";
    vec![
        ApiRoute {
            id: "mail_accounts.list",
            method: Method::GET,
            path: "/api/mail_accounts/",
            upstream: "src/paperless_mail/views.py::MailAccountViewSet.list",
            handler: drf_view(get(list), ALLOW_MODEL_LIST),
        },
        ApiRoute {
            id: "mail_accounts.create",
            method: Method::POST,
            path: "/api/mail_accounts/",
            upstream: "src/paperless_mail/views.py::MailAccountViewSet.create",
            handler: drf_view(post(create), ALLOW_MODEL_LIST),
        },
        ApiRoute {
            id: "mail_accounts.test",
            method: Method::POST,
            path: "/api/mail_accounts/test/",
            upstream: "src/paperless_mail/views.py::MailAccountViewSet.test",
            handler: drf_view(post(test), ALLOW_POST_ONLY),
        },
        ApiRoute {
            id: "mail_accounts.retrieve",
            method: Method::GET,
            path: "/api/mail_accounts/{id}/",
            upstream: SRC,
            handler: drf_view(get(retrieve), ALLOW_MODEL_DETAIL),
        },
        ApiRoute {
            id: "mail_accounts.update",
            method: Method::PUT,
            path: "/api/mail_accounts/{id}/",
            upstream: SRC,
            handler: drf_view(put(update), ALLOW_MODEL_DETAIL),
        },
        ApiRoute {
            id: "mail_accounts.partial_update",
            method: Method::PATCH,
            path: "/api/mail_accounts/{id}/",
            upstream: SRC,
            handler: drf_view(patch(partial_update), ALLOW_MODEL_DETAIL),
        },
        ApiRoute {
            id: "mail_accounts.destroy",
            method: Method::DELETE,
            path: "/api/mail_accounts/{id}/",
            upstream: SRC,
            handler: drf_view(delete(destroy), ALLOW_MODEL_DETAIL),
        },
        ApiRoute {
            id: "mail_accounts.process",
            method: Method::POST,
            path: "/api/mail_accounts/{id}/process/",
            upstream: "src/paperless_mail/views.py::MailAccountViewSet.process",
            handler: drf_view(post(process), ALLOW_POST_ONLY),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_masking() {
        assert_eq!(obfuscate("password1"), "**********");
        assert_eq!(obfuscate(""), "**********");
        assert_eq!(obfuscate("abcdefghijkl"), "************");
        assert_eq!(obfuscate("äöü"), "**********");
    }

    /// A-230/B-313: unreachable server (upstream 500) → 400 with message.
    #[test]
    fn test_failures_are_400() {
        for e in [TestError::Other("connect error: refused".into()), TestError::Mail("LOGIN failed".into())] {
            let r = test_result_response(Err(e)).err().map(IntoResponse::into_response);
            assert_eq!(r.map(|r| r.status()), Some(StatusCode::BAD_REQUEST));
        }
        assert!(test_result_response(Ok(())).is_ok());
        let ApiError::Json(st, v) = not_a_dict(&json!([])) else { panic!("json") };
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(v["non_field_errors"][0], "Invalid data. Expected a dictionary, but got list.");
    }

    /// Security review 2026-11-19: "****" in the account test may only send the stored password to
    /// the stored target.
    #[test]
    fn stored_password_only_for_saved_target() {
        let stored = AccountRow {
            id: 1,
            name: "a".into(),
            imap_server: "imap.example.org".into(),
            imap_port: Some(993),
            imap_security: 2,
            username: "alice".into(),
            password: "geheim".into(),
            character_set: "UTF-8".into(),
            is_token: false,
            owner_id: None,
            account_type: 1,
            refresh_token: None,
            expiration: None,
        };
        let t = |server: &str, port: u16, sec: i16, user: &str| TestAccount {
            imap_server: server.into(),
            imap_port: port,
            imap_security: sec,
            username: user.into(),
            password: "****".into(),
            is_token: false,
        };
        assert!(stored_target_matches(&stored, &t("imap.example.org", 993, 2, "alice")));
        assert!(stored_target_matches(&stored, &t("IMAP.Example.org", 993, 2, "alice")));
        assert!(!stored_target_matches(&stored, &t("attacker.example", 993, 2, "alice")));
        assert!(!stored_target_matches(&stored, &t("imap.example.org", 143, 2, "alice")));
        assert!(!stored_target_matches(&stored, &t("imap.example.org", 993, 1, "alice")));
        assert!(!stored_target_matches(&stored, &t("imap.example.org", 993, 2, "mallory")));
        let ApiError::Json(st, v) = field_error("password", STORED_PASSWORD_OTHER_TARGET) else { panic!("json") };
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(v["password"][0].as_str().is_some_and(|m| m.contains("enter the password again")));
    }

    /// Review 2 M3: workflow triggers of the account's rules are cleared before deletion.
    #[test]
    fn account_delete_nulls_workflow_trigger_first() {
        assert!(ACCOUNT_DELETE_SQL[0].starts_with("UPDATE documents_workflowtrigger SET filter_mailrule_id = NULL"));
        assert!(ACCOUNT_DELETE_SQL[0].contains("account_id = $1"));
        assert_eq!(ACCOUNT_DELETE_SQL.last().copied(), Some("DELETE FROM paperless_mail_mailaccount WHERE id = $1"));
    }
}
