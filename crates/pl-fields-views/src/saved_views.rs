//! `/api/saved_views/`
//!
//! upstream: src/documents/views.py@v3.4.2 (SavedViewViewSet, BulkPermissionMixin, PassUserMixin),
//! src/documents/serialisers.py@v3.4.2 (SavedViewSerializer, SavedViewFilterRuleSerializer,
//! OwnedObjectSerializer), src/documents/models.py@v3.4.2 (SavedView, SavedViewFilterRule,
//! UiSettings).

use std::collections::{BTreeSet, HashMap};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use pl_core::AppState;
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query, SelectStatement};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};

use crate::authz::{
    apply_set_permissions, check_object_perm, grantees, parse_owner, parse_set_permissions,
    require_model_perm, user_can_change, visible,
};
use pl_authz::models::{ObjectModel, SAVED_VIEW};
use pl_authz::owned::{ObjectGrantees, PermissionsSpec};
use crate::fields::{
    CharOpts, ChoiceOpts, Errors, F, IntOpts, bool_field, char_field, choice_int, choice_str,
    int_field, json_field, parse_bool, py_str,
};
use crate::query::{apply_ordering, ordering_terms};
use crate::shim::{
    ApiError, ApiResult, Ctx, finish, json_response, no_content, page_request, paginated,
    parse_body, parse_pk, py_type_name, resolve_page,
};

const OM: ObjectModel = SAVED_VIEW;
const LABEL: &str = "SavedView";
pub const LIST_ALLOW: &str = "GET, POST, HEAD, OPTIONS";
pub const DETAIL_ALLOW: &str = "GET, PUT, PATCH, DELETE, HEAD, OPTIONS";

pub const ICONS: &[&str] = &[
    "archive",
    "bank",
    "basket",
    "bell",
    "bookmark",
    "boxes",
    "briefcase",
    "building",
    "calculator",
    "calendar",
    "camera",
    "card-checklist",
    "cash",
    "chat-left-text",
    "check-circle",
    "clipboard",
    "clock-history",
    "credit-card",
    "download",
    "envelope",
    "exclamation-triangle",
    "file-earmark",
    "file-earmark-check",
    "file-earmark-lock",
    "file-earmark-medical",
    "file-earmark-person",
    "file-earmark-spreadsheet",
    "file-text",
    "files",
    "folder",
    "funnel",
    "gear",
    "globe2",
    "hash",
    "heart",
    "house",
    "inbox",
    "journals",
    "list-task",
    "newspaper",
    "paperclip",
    "people",
    "person",
    "printer",
    "receipt",
    "safe",
    "search",
    "send",
    "shop",
    "stack",
    "stars",
    "tag",
    "tags",
    "telephone",
    "truck",
    "upc-scan",
    "wallet2",
];

pub const DISPLAY_MODES: &[&str] = &["table", "smallCards", "largeCards"];

/// `SavedView.DisplayFields.values`
pub const DISPLAY_FIELDS: &[&str] = &[
    "title",
    "created",
    "added",
    "tag",
    "correspondent",
    "documenttype",
    "storagepath",
    "note",
    "owner",
    "shared",
    "asn",
    "pagecount",
    "custom_field_%d",
];

fn a(s: &str) -> Alias {
    Alias::new(s)
}

#[derive(Clone, Debug)]
struct View {
    id: i64,
    name: String,
    icon: String,
    sort_field: Option<String>,
    sort_reverse: bool,
    page_size: Option<i64>,
    display_mode: Option<String>,
    display_fields: Value,
    owner_id: Option<i64>,
}

fn select_columns(q: &mut SelectStatement) {
    for (e, n) in [
        ("sv.id::int8", "id"),
        ("sv.name", "name"),
        ("sv.icon", "icon"),
        ("sv.sort_field", "sort_field"),
        ("sv.sort_reverse", "sort_reverse"),
        ("sv.page_size::int8", "page_size"),
        ("sv.display_mode", "display_mode"),
        ("sv.display_fields", "display_fields"),
        ("sv.owner_id::int8", "owner_id"),
    ] {
        q.expr_as(Expr::cust(e), a(n));
    }
}

fn from_row(r: &sqlx::postgres::PgRow) -> Result<View, sqlx::Error> {
    Ok(View {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        icon: r.try_get("icon")?,
        sort_field: r.try_get("sort_field")?,
        sort_reverse: r.try_get("sort_reverse")?,
        page_size: r.try_get("page_size")?,
        display_mode: r.try_get("display_mode")?,
        display_fields: r
            .try_get::<Option<sqlx::types::Json<Value>>, _>("display_fields")?
            .map(|j| j.0)
            .unwrap_or(Value::Null),
        owner_id: r.try_get("owner_id")?,
    })
}

fn base(ctx: &Ctx) -> SelectStatement {
    let mut q = Query::select();
    q.from_as(a("documents_savedview"), a("sv"));
    q.cond_where(visible(ctx, &OM, "sv"));
    q
}

async fn load_rules(db: &PgPool, ids: &[i64]) -> ApiResult<HashMap<i64, Vec<Value>>> {
    let mut out: HashMap<i64, Vec<Value>> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let rows = sqlx::query(
        "SELECT saved_view_id::int8 AS sv, rule_type::int8 AS rule_type, value \
         FROM documents_savedviewfilterrule WHERE saved_view_id = ANY($1::int8[]) ORDER BY id",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    for r in rows {
        let sv: i64 = r.try_get("sv")?;
        let rt: i64 = r.try_get("rule_type")?;
        let v: Option<String> = r.try_get("value")?;
        out.entry(sv)
            .or_default()
            .push(json!({"rule_type": rt, "value": v}));
    }
    Ok(out)
}

/// Visibility IDs from `UiSettings` (v9).
struct LegacyVis {
    dashboard: Vec<Value>,
    sidebar: Vec<Value>,
}

async fn load_legacy(db: &PgPool, uid: i64) -> ApiResult<LegacyVis> {
    let s: Option<Option<sqlx::types::Json<Value>>> =
        sqlx::query_scalar("SELECT settings FROM documents_uisettings WHERE user_id = $1::int8")
            .bind(uid)
            .fetch_optional(db)
            .await?;
    let mut lv = LegacyVis {
        dashboard: Vec::new(),
        sidebar: Vec::new(),
    };
    let Some(Some(sqlx::types::Json(Value::Object(settings)))) = s else {
        return Ok(lv);
    };
    if settings.is_empty() {
        return Ok(lv);
    }
    let sv = settings
        .get("saved_views")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let Value::Object(sv) = sv else {
        return Ok(lv);
    };
    for (key, target) in [
        ("dashboard_views_visible_ids", &mut lv.dashboard),
        ("sidebar_views_visible_ids", &mut lv.sidebar),
    ] {
        match sv.get(key) {
            None => {}
            Some(Value::Array(a)) => *target = a.clone(),
            // set(str) / set(dict) → elements are strings → never equal to an int ID
            Some(Value::String(_)) | Some(Value::Object(_)) => {}
            // set(None) / set(5) → TypeError
            Some(_) => return Err(ApiError::ServerError),
        }
    }
    Ok(lv)
}

/// Python `instance.id in set(values)`
fn py_in(values: &[Value], id: i64) -> bool {
    values.iter().any(|v| match v {
        Value::Number(n) => n.as_f64() == Some(id as f64),
        Value::Bool(b) => (*b as i64) == id,
        _ => false,
    })
}

struct SerCtx {
    full_perms: bool,
    version: u32,
    grantees: HashMap<i64, ObjectGrantees>,
    rules: HashMap<i64, Vec<Value>>,
    legacy: Option<LegacyVis>,
}

async fn ser_ctx(db: &PgPool, ctx: &Ctx, ids: &[i64]) -> ApiResult<SerCtx> {
    Ok(SerCtx {
        full_perms: ctx.full_perms(),
        version: ctx.version,
        grantees: grantees(db, &OM, ids).await?,
        rules: load_rules(db, ids).await?,
        legacy: if ctx.version < 10 {
            Some(load_legacy(db, ctx.user.id).await?)
        } else {
            None
        },
    })
}

fn to_json(ctx: &Ctx, sc: &SerCtx, v: &View) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(v.id));
    m.insert("name".into(), json!(v.name));
    m.insert("icon".into(), json!(v.icon));
    m.insert("sort_field".into(), json!(v.sort_field));
    m.insert("sort_reverse".into(), json!(v.sort_reverse));
    m.insert(
        "filter_rules".into(),
        Value::Array(sc.rules.get(&v.id).cloned().unwrap_or_default()),
    );
    m.insert("page_size".into(), json!(v.page_size));
    m.insert("display_mode".into(), json!(v.display_mode));
    m.insert("display_fields".into(), v.display_fields.clone());
    m.insert("owner".into(), json!(v.owner_id));
    let g = sc.grantees.get(&v.id);
    if sc.full_perms {
        m.insert(
            "permissions".into(),
            g.map(ObjectGrantees::to_json)
                .unwrap_or_else(|| ObjectGrantees::default().to_json()),
        );
    } else {
        m.insert(
            "user_can_change".into(),
            json!(user_can_change(ctx, v.owner_id, g)),
        );
    }
    if sc.version < 10 {
        let (d, s) = match &sc.legacy {
            Some(l) => (py_in(&l.dashboard, v.id), py_in(&l.sidebar, v.id)),
            None => (false, false),
        };
        m.insert("show_on_dashboard".into(), json!(d));
        m.insert("show_in_sidebar".into(), json!(s));
    }
    Value::Object(m)
}

async fn single_json(db: &PgPool, ctx: &Ctx, v: &View) -> ApiResult<Value> {
    let sc = ser_ctx(db, ctx, &[v.id]).await?;
    Ok(to_json(ctx, &sc, v))
}

/// `get_object()`: visibility (404) + object permission.
async fn get_object(db: &PgPool, ctx: &Ctx, _method: &Method, pk: &str) -> ApiResult<View> {
    let id = parse_pk(pk)?;
    let mut q = base(ctx);
    select_columns(&mut q);
    q.and_where(Expr::cust_with_values("sv.id = $1", [id]));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(db).await?;
    let Some(row) = row else {
        return Err(ApiError::not_found(LABEL));
    };
    let v = from_row(&row)?;
    check_object_perm(db, ctx, &OM, v.id, v.owner_id).await?;
    Ok(v)
}

async fn reload(db: &PgPool, id: i64) -> ApiResult<View> {
    let mut q = Query::select();
    q.from_as(a("documents_savedview"), a("sv"));
    select_columns(&mut q);
    q.and_where(Expr::cust_with_values("sv.id = $1", [id]));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_one(db).await?;
    Ok(from_row(&row)?)
}

// ---------------------------------------------------------------------------
// list / retrieve
// ---------------------------------------------------------------------------

pub async fn list(State(st): State<AppState>, ctx: Ctx) -> Response {
    finish(LIST_ALLOW, list_inner(&st.inner.db, &ctx).await)
}

async fn list_inner(db: &PgPool, ctx: &Ctx) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let mut q = base(ctx);
    let mut cq = q.clone();
    cq.expr(Expr::cust("COUNT(*)::int8"));
    let (sql, values) = cq.build_sqlx(PostgresQueryBuilder);
    let count: i64 = sqlx::query_scalar_with(&sql, values).fetch_one(db).await?;

    match ordering_terms(ctx, &["name"]) {
        Some(t) => apply_ordering(&mut q, &t, |n| format!("sv.{n}")),
        None => {
            q.order_by_expr(Expr::cust("sv.name"), sea_query::Order::Asc);
        }
    }
    let page = resolve_page(&page_request(ctx), count)?;
    let all_ids = if ctx.version < 10 {
        let mut iq = q.clone();
        iq.expr(Expr::cust("sv.id::int8"));
        let (sql, values) = iq.build_sqlx(PostgresQueryBuilder);
        Some(sqlx::query_scalar_with::<_, i64, _>(&sql, values).fetch_all(db).await?)
    } else {
        None
    };
    select_columns(&mut q);
    q.limit(page.page_size).offset(page.offset());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    let views: Vec<View> = rows.iter().map(from_row).collect::<Result<_, _>>()?;
    let ids: Vec<i64> = views.iter().map(|v| v.id).collect();
    let sc = ser_ctx(db, ctx, &ids).await?;
    let results = views.iter().map(|v| to_json(ctx, &sc, v)).collect();
    Ok(paginated(ctx, &page, all_ids, results))
}

pub async fn retrieve(State(st): State<AppState>, Path(pk): Path<String>, ctx: Ctx) -> Response {
    finish(DETAIL_ALLOW, retrieve_inner(&st.inner.db, &ctx, &pk).await)
}

async fn retrieve_inner(db: &PgPool, ctx: &Ctx, pk: &str) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let v = get_object(db, ctx, &Method::GET, pk).await?;
    Ok(json_response(StatusCode::OK, &single_json(db, ctx, &v).await?))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[derive(Default, Debug)]
struct Attrs {
    name: Option<String>,
    icon: Option<String>,
    sort_field: Option<Option<String>>,
    sort_reverse: Option<bool>,
    filter_rules: Option<Vec<(i64, Option<String>)>>,
    page_size: Option<Option<i64>>,
    display_mode: Option<Option<String>>,
    display_fields: Option<Value>,
    owner: Option<Option<i64>>,
    set_permissions: Option<PermissionsSpec>,
    show_on_dashboard: Option<bool>,
    show_in_sidebar: Option<bool>,
}

/// `filter_rules = SavedViewFilterRuleSerializer(many=True)`.
/// DRF 3.18.3 `ListSerializer.to_internal_value`: `LIST_SERIALIZER_ERRORS_AS_DICT = True`
/// (default) -> errors are `{index: detail}` for the failing items only (W2 parity run).
fn validate_rules(v: &Value) -> Result<Vec<(i64, Option<String>)>, Value> {
    let Value::Array(items) = v else {
        return Err(json!({"non_field_errors": [format!(
            "Expected a list of items but got type \"{}\".",
            py_type_name(v)
        )]}));
    };
    let mut out = Vec::new();
    let mut errs = serde_json::Map::new();
    for (idx, item) in items.iter().enumerate() {
        let Value::Object(m) = item else {
            errs.insert(idx.to_string(), json!({"non_field_errors": [format!(
                "Invalid data. Expected a dictionary, but got {}.",
                py_type_name(item)
            )]}));
            continue;
        };
        let mut e = Errors::default();
        let rt = match m.get("rule_type") {
            None => {
                e.add("rule_type", crate::fields::REQUIRED);
                None
            }
            Some(Value::Null) => {
                e.add("rule_type", crate::fields::NULL);
                None
            }
            Some(x) => match choice_int(x, 0..=50) {
                Ok(i) => Some(i),
                Err(msg) => {
                    e.add("rule_type", &msg);
                    None
                }
            },
        };
        let val = char_field(
            m,
            "value",
            CharOpts {
                required: false,
                allow_null: true,
                allow_blank: true,
                max_length: Some(255),
            },
            false,
            &mut e,
        );
        if e.is_empty() {
            let value = match val {
                F::Val(v) => v,
                _ => None,
            };
            out.push((rt.unwrap_or(0), value));
        } else {
            errs.insert(idx.to_string(), Value::Object(e.0));
        }
    }
    if errs.is_empty() { Ok(out) } else { Err(Value::Object(errs)) }
}

/// `validate()`: display_fields
async fn validate_display_fields(db: &PgPool, df: &Value) -> ApiResult<()> {
    let elems: Vec<Value> = match df {
        Value::Null => return Ok(()),
        Value::Array(a) => a.clone(),
        Value::Object(m) => m.keys().map(|k| Value::String(k.clone())).collect(),
        Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        _ => return Err(ApiError::ServerError),
    };
    let invalid = |f: &Value| {
        ApiError::bad_request(json!({"non_field_errors": [format!("Invalid field: {}", py_str(f))]}))
    };
    for f in &elems {
        match f {
            Value::String(s) => {
                if s.contains("custom_field_") {
                    // re.search(r"\d+", field)[0]; no match → TypeError (500)
                    let digits: String = s
                        .chars()
                        .skip_while(|c| !c.is_ascii_digit())
                        .take_while(char::is_ascii_digit)
                        .collect();
                    if digits.is_empty() {
                        return Err(ApiError::ServerError);
                    }
                    let exists = match digits.parse::<i32>() {
                        Ok(id) => {
                            sqlx::query_scalar::<_, bool>(
                                "SELECT EXISTS (SELECT 1 FROM documents_customfield WHERE id = $1)",
                            )
                            .bind(id)
                            .fetch_one(db)
                            .await?
                        }
                        Err(_) => false,
                    };
                    if !exists {
                        return Err(invalid(f));
                    }
                } else if !DISPLAY_FIELDS.contains(&s.as_str()) {
                    return Err(invalid(f));
                }
            }
            Value::Array(arr) => {
                if arr.iter().any(|x| x.as_str() == Some("custom_field_")) {
                    return Err(ApiError::ServerError);
                }
                return Err(invalid(f));
            }
            Value::Object(m) => {
                if m.contains_key("custom_field_") {
                    return Err(ApiError::ServerError);
                }
                return Err(invalid(f));
            }
            _ => return Err(ApiError::ServerError),
        }
    }
    Ok(())
}

async fn validate(
    db: &PgPool,
    ctx: &Ctx,
    mut data: Map<String, Value>,
    partial: bool,
) -> ApiResult<Attrs> {
    let mut attrs = Attrs::default();
    // v9: legacy visibility fields up front (to_internal_value)
    if ctx.version < 10 {
        for name in ["show_on_dashboard", "show_in_sidebar"] {
            if let Some(v) = data.remove(name) {
                match parse_bool(&v) {
                    Some(b) => {
                        if name == "show_on_dashboard" {
                            attrs.show_on_dashboard = Some(b);
                        } else {
                            attrs.show_in_sidebar = Some(b);
                        }
                    }
                    None => {
                        return Err(ApiError::bad_request(
                            json!({ name: ["Must be a valid boolean."] }),
                        ));
                    }
                }
            }
        }
    }
    let mut errs = Errors::default();
    if let F::Val(Some(n)) = char_field(
        &data,
        "name",
        CharOpts {
            required: true,
            allow_null: false,
            allow_blank: false,
            max_length: Some(128),
        },
        partial,
        &mut errs,
    ) {
        attrs.name = Some(n);
    }
    if let F::Val(Some(i)) = choice_str(
        &data,
        "icon",
        ICONS,
        ChoiceOpts {
            required: false,
            allow_null: false,
            allow_blank: false,
        },
        partial,
        &mut errs,
    ) {
        attrs.icon = Some(i);
    }
    if let F::Val(s) = char_field(
        &data,
        "sort_field",
        CharOpts {
            required: false,
            allow_null: true,
            allow_blank: true,
            max_length: Some(128),
        },
        partial,
        &mut errs,
    ) {
        attrs.sort_field = Some(s);
    }
    if let F::Val(b) = bool_field(&data, "sort_reverse", partial, &mut errs) {
        attrs.sort_reverse = Some(b);
    }
    match data.get("filter_rules") {
        None => {
            if !partial {
                errs.add("filter_rules", crate::fields::REQUIRED);
            }
        }
        Some(Value::Null) => errs.add("filter_rules", crate::fields::NULL),
        Some(v) => match validate_rules(v) {
            Ok(r) => attrs.filter_rules = Some(r),
            Err(e) => errs.set("filter_rules", e),
        },
    }
    if let F::Val(p) = int_field(
        &data,
        "page_size",
        IntOpts {
            required: false,
            allow_null: true,
            min: Some(1),
            max: Some(2_147_483_647),
        },
        partial,
        &mut errs,
    ) {
        attrs.page_size = Some(p);
    }
    if let F::Val(d) = choice_str(
        &data,
        "display_mode",
        DISPLAY_MODES,
        ChoiceOpts {
            required: false,
            allow_null: true,
            allow_blank: true,
        },
        partial,
        &mut errs,
    ) {
        attrs.display_mode = Some(d);
    }
    if let F::Val(v) = json_field(&data, "display_fields", true, partial, &mut errs) {
        attrs.display_fields = Some(v);
    }
    match data.get("owner") {
        None => {}
        Some(v) => match parse_owner(db, v).await? {
            Ok(o) => attrs.owner = Some(o),
            Err(e) => errs.set("owner", e),
        },
    }
    match data.get("set_permissions") {
        None => {}
        Some(Value::Null) => errs.add("set_permissions", crate::fields::NULL),
        Some(v) => match parse_set_permissions(db, v).await? {
            Ok(spec) => attrs.set_permissions = Some(spec),
            Err(e) => errs.set("set_permissions", e),
        },
    }
    errs.into_result()?;
    if let Some(df) = &attrs.display_fields {
        validate_display_fields(db, df).await?;
    }
    Ok(attrs)
}

/// `OwnedObjectSerializer.validate_unique_together`
async fn unique_violation(
    db: &PgPool,
    name: &str,
    owner: Option<i64>,
    exclude: Option<i64>,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM documents_savedview WHERE name = $1 \
         AND owner_id IS NOT DISTINCT FROM $2::int8 AND id <> $3::int8)",
    )
    .bind(name)
    .bind(owner)
    .bind(exclude.unwrap_or(-1))
    .fetch_one(db)
    .await?)
}

fn unique_error() -> ApiError {
    ApiError::bad_request(json!({"error": "Object violates owner / name unique constraint"}))
}

async fn insert_rules(db: &PgPool, view_id: i64, rules: &[(i64, Option<String>)]) -> ApiResult<()> {
    for (rt, v) in rules {
        sqlx::query(
            "INSERT INTO documents_savedviewfilterrule (rule_type, value, saved_view_id) \
             VALUES ($1::int2, $2, $3::int8)",
        )
        .bind(*rt as i16)
        .bind(v)
        .bind(view_id)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// `_update_legacy_visibility_preferences`
async fn update_legacy(
    db: &PgPool,
    uid: i64,
    view_id: i64,
    dash: Option<bool>,
    side: Option<bool>,
) -> ApiResult<()> {
    if dash.is_none() && side.is_none() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO documents_uisettings (settings, user_id) \
         SELECT '{}'::jsonb, $1::int8 WHERE NOT EXISTS \
         (SELECT 1 FROM documents_uisettings WHERE user_id = $1::int8)",
    )
    .bind(uid)
    .execute(db)
    .await?;
    let cur: Option<sqlx::types::Json<Value>> =
        sqlx::query_scalar("SELECT settings FROM documents_uisettings WHERE user_id = $1::int8")
            .bind(uid)
            .fetch_one(db)
            .await?;
    let mut settings = match cur.map(|j| j.0) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    let mut svs = match settings.get("saved_views") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    let ids = |v: Option<&Value>| -> ApiResult<BTreeSet<i64>> {
        let mut s = BTreeSet::new();
        match v {
            None => {}
            Some(Value::Array(a)) => {
                for x in a {
                    // int(raw) for str(raw).isdigit()
                    let st = py_str(x);
                    if !st.is_empty() && st.chars().all(|c| c.is_ascii_digit())
                        && let Ok(n) = st.parse::<i64>() {
                            s.insert(n);
                        }
                }
            }
            Some(Value::String(st)) => {
                for c in st.chars() {
                    if let Some(d) = c.to_digit(10) {
                        s.insert(d as i64);
                    }
                }
            }
            Some(Value::Object(m)) => {
                for k in m.keys() {
                    if !k.is_empty() && k.chars().all(|c| c.is_ascii_digit())
                        && let Ok(n) = k.parse::<i64>() {
                            s.insert(n);
                        }
                }
            }
            Some(_) => return Err(ApiError::ServerError),
        }
        Ok(s)
    };
    let mut d = ids(svs.get("dashboard_views_visible_ids"))?;
    let mut s = ids(svs.get("sidebar_views_visible_ids"))?;
    if let Some(b) = dash {
        if b {
            d.insert(view_id);
        } else {
            d.remove(&view_id);
        }
    }
    if let Some(b) = side {
        if b {
            s.insert(view_id);
        } else {
            s.remove(&view_id);
        }
    }
    svs.insert(
        "dashboard_views_visible_ids".into(),
        Value::Array(d.into_iter().map(Value::from).collect()),
    );
    svs.insert(
        "sidebar_views_visible_ids".into(),
        Value::Array(s.into_iter().map(Value::from).collect()),
    );
    settings.insert("saved_views".into(), Value::Object(svs));
    sqlx::query("UPDATE documents_uisettings SET settings = $1 WHERE user_id = $2::int8")
        .bind(sqlx::types::Json(Value::Object(settings)))
        .bind(uid)
        .execute(db)
        .await?;
    Ok(())
}

fn json_or_null(v: &Value) -> sea_query::SimpleExpr {
    if v.is_null() {
        sea_query::Value::Json(None).into()
    } else {
        v.clone().into()
    }
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

pub async fn create(State(st): State<AppState>, ctx: Ctx, body: Bytes) -> Response {
    finish(LIST_ALLOW, create_inner(&st.inner.db, &ctx, &body).await)
}

async fn create_inner(db: &PgPool, ctx: &Ctx, body: &[u8]) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let data = crate::shim::require_object(parse_body(&ctx.headers, body)?)?;
    let owner_in_request = data.contains_key("owner");
    let attrs = validate(db, ctx, data, false).await?;
    let Some(name) = attrs.name.clone() else {
        return Err(ApiError::ServerError);
    };
    let owner = if owner_in_request {
        attrs.owner.unwrap_or(None)
    } else {
        Some(ctx.user.id)
    };
    if unique_violation(db, &name, owner, None).await? {
        return Err(unique_error());
    }
    let mut cols = vec![a("name"), a("icon"), a("sort_reverse"), a("owner_id")];
    let mut vals: Vec<sea_query::SimpleExpr> = vec![
        name.into(),
        attrs.icon.clone().unwrap_or_else(|| "funnel".into()).into(),
        attrs.sort_reverse.unwrap_or(false).into(),
        owner.into(),
    ];
    cols.push(a("sort_field"));
    vals.push(attrs.sort_field.clone().unwrap_or(None).into());
    cols.push(a("page_size"));
    vals.push(attrs.page_size.unwrap_or(None).into());
    cols.push(a("display_mode"));
    vals.push(attrs.display_mode.clone().unwrap_or(None).into());
    cols.push(a("display_fields"));
    vals.push(json_or_null(attrs.display_fields.as_ref().unwrap_or(&Value::Null)));
    let mut iq = Query::insert();
    iq.into_table(a("documents_savedview"))
        .columns(cols)
        .values_panic(vals)
        .returning(Query::returning().expr(Expr::cust("id::int8")));
    let (sql, values) = iq.build_sqlx(PostgresQueryBuilder);
    let id: i64 = sqlx::query_scalar_with(&sql, values).fetch_one(db).await?;
    if let Some(spec) = &attrs.set_permissions {
        apply_set_permissions(db, &OM, id, spec).await?;
    }
    if let Some(rules) = &attrs.filter_rules {
        insert_rules(db, id, rules).await?;
    }
    update_legacy(db, ctx.user.id, id, attrs.show_on_dashboard, attrs.show_in_sidebar).await?;
    let v = reload(db, id).await?;
    Ok(json_response(StatusCode::CREATED, &single_json(db, ctx, &v).await?))
}

// ---------------------------------------------------------------------------
// update / partial_update
// ---------------------------------------------------------------------------

pub async fn update(
    State(st): State<AppState>,
    Path(pk): Path<String>,
    ctx: Ctx,
    body: Bytes,
) -> Response {
    finish(DETAIL_ALLOW, update_inner(&st.inner.db, &ctx, &pk, &body, false).await)
}

pub async fn partial_update(
    State(st): State<AppState>,
    Path(pk): Path<String>,
    ctx: Ctx,
    body: Bytes,
) -> Response {
    finish(DETAIL_ALLOW, update_inner(&st.inner.db, &ctx, &pk, &body, true).await)
}

async fn update_inner(
    db: &PgPool,
    ctx: &Ctx,
    pk: &str,
    body: &[u8],
    partial: bool,
) -> ApiResult<Response> {
    let method = if partial { Method::PATCH } else { Method::PUT };
    require_model_perm(ctx, &OM)?;
    let inst = get_object(db, ctx, &method, pk).await?;
    let data = crate::shim::require_object(parse_body(&ctx.headers, body)?)?;
    let attrs = validate(db, ctx, data, partial).await?;

    let is_owner = inst.owner_id == Some(ctx.user.id);
    let is_unowned = inst.owner_id.is_none();
    let owner_changes = attrs.owner.is_some_and(|o| o != inst.owner_id);
    if (owner_changes || attrs.set_permissions.is_some())
        && !(ctx.user.is_superuser || is_owner || is_unowned)
    {
        return Err(ApiError::insufficient());
    }
    if let Some(spec) = &attrs.set_permissions {
        apply_set_permissions(db, &OM, inst.id, spec).await?;
    }
    if let Some(owner) = attrs.owner {
        let name = attrs.name.clone().unwrap_or_else(|| inst.name.clone());
        if unique_violation(db, &name, owner, Some(inst.id)).await? {
            return Err(unique_error());
        }
    }
    let mut uq = Query::update();
    uq.table(a("documents_savedview"));
    let mut any = false;
    let mut set = |col: &str, v: sea_query::SimpleExpr| {
        uq.value(a(col), v);
        any = true;
    };
    if let Some(n) = &attrs.name {
        set("name", n.clone().into());
    }
    if let Some(i) = &attrs.icon {
        set("icon", i.clone().into());
    }
    if let Some(s) = &attrs.sort_field {
        set("sort_field", s.clone().into());
    }
    if let Some(b) = attrs.sort_reverse {
        set("sort_reverse", b.into());
    }
    if let Some(p) = attrs.page_size {
        set("page_size", p.into());
    }
    if let Some(d) = &attrs.display_mode {
        set("display_mode", d.clone().into());
    }
    if let Some(df) = &attrs.display_fields {
        let df = match df {
            Value::Array(arr) if arr.is_empty() => Value::Null,
            other => other.clone(),
        };
        set("display_fields", json_or_null(&df));
    }
    if let Some(o) = attrs.owner {
        set("owner_id", o.into());
    }
    if any {
        uq.and_where(Expr::col(a("id")).eq(inst.id));
        let (sql, values) = uq.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values).execute(db).await?;
    }
    if let Some(rules) = &attrs.filter_rules {
        sqlx::query("DELETE FROM documents_savedviewfilterrule WHERE saved_view_id = $1::int8")
            .bind(inst.id)
            .execute(db)
            .await?;
        insert_rules(db, inst.id, rules).await?;
    }
    update_legacy(
        db,
        ctx.user.id,
        inst.id,
        attrs.show_on_dashboard,
        attrs.show_in_sidebar,
    )
    .await?;
    let v = reload(db, inst.id).await?;
    Ok(json_response(StatusCode::OK, &single_json(db, ctx, &v).await?))
}

// ---------------------------------------------------------------------------
// destroy
// ---------------------------------------------------------------------------

pub async fn destroy(State(st): State<AppState>, Path(pk): Path<String>, ctx: Ctx) -> Response {
    finish(DETAIL_ALLOW, destroy_inner(&st.inner.db, &ctx, &pk).await)
}

async fn destroy_inner(db: &PgPool, ctx: &Ctx, pk: &str) -> ApiResult<Response> {
    require_model_perm(ctx, &OM)?;
    let v = get_object(db, ctx, &Method::DELETE, pk).await?;
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM documents_savedviewfilterrule WHERE saved_view_id = $1::int8")
        .bind(v.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM documents_savedview WHERE id = $1::int8")
        .bind(v.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(no_content())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_validation() {
        assert_eq!(
            validate_rules(&json!([{"rule_type": 4, "value": "x"}, {"rule_type": "6"}])),
            Ok(vec![(4, Some("x".into())), (6, None)])
        );
        assert_eq!(
            validate_rules(&json!([{"rule_type": 99}, {"rule_type": 1}])),
            Err(json!({"0": {"rule_type": ["\"99\" is not a valid choice."]}}))
        );
        assert_eq!(
            validate_rules(&json!("x")),
            Err(json!({"non_field_errors": ["Expected a list of items but got type \"str\"."]}))
        );
        assert_eq!(
            validate_rules(&json!([{}])),
            Err(json!({"0": {"rule_type": ["This field is required."]}}))
        );
        // W2 parity run 2026-11-08 (PUT /api/saved_views/<id>/ with two bad rules): dict, index keys
        assert_eq!(
            validate_rules(&json!([{"rule_type": 1}, {"rule_type": 99}, {"value": "x"}, 5])),
            Err(json!({
                "1": {"rule_type": ["\"99\" is not a valid choice."]},
                "2": {"rule_type": ["This field is required."]},
                "3": {"non_field_errors": ["Invalid data. Expected a dictionary, but got int."]}
            }))
        );
    }

    #[test]
    fn legacy_membership() {
        assert!(py_in(&[json!(3), json!("4")], 3));
        assert!(!py_in(&[json!("4")], 4));
        assert!(py_in(&[json!(4.0)], 4));
        assert!(py_in(&[json!(true)], 1));
    }

    #[test]
    fn icons_complete() {
        assert_eq!(ICONS.len(), 57);
        assert!(ICONS.contains(&"funnel"));
    }
}
