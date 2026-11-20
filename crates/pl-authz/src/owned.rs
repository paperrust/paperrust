//! `OwnedObjectSerializer` helpers: read fields (`owner`, `permissions`, `user_can_change`,
//! `is_shared_by_requester`) batched per page, and write helpers (`owner`, `set_permissions`).
//!
//! upstream: src/documents/serialisers.py@v3.4.2 (SetPermissionsMixin, SerializerWithPerms,
//!           PermissionSetSerializer, SetPermissionsSerializer, OwnedObjectSerializer.__init__/
//!           get_permissions/get_user_can_change/get_shared_object_pks/get_is_shared_by_requester/
//!           create/update, OwnedObjectListSerializer),
//!           src/documents/views.py@v3.4.2 (PassUserMixin full_perms, BulkPermissionMixin),
//!           src/documents/permissions.py@v3.4.2 (get_permission_grantees, set_permissions_for_objects,
//!           _apply_bulk_permission_entry, _resolve_permissions),
//!           src/documents/utils.py@v3.4.2 (get_boolean),
//!           rest_framework/fields.py@3.16 (IntegerField, ListField), rest_framework/relations.py@3.16
//!           (PrimaryKeyRelatedField), rest_framework/serializers.py@3.16 (Serializer.to_internal_value).

use crate::error::AuthError;
use crate::models::ObjectModel;
use crate::objects::content_type_id_subquery;
use crate::user::User;
use sea_query::{Alias, Expr, OnConflict, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder as _;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, PgPool};
use std::collections::{BTreeSet, HashMap, HashSet};

fn a(s: &str) -> Alias {
    Alias::new(s)
}

// ---------------------------------------------------------------------------------------------
// Read side
// ---------------------------------------------------------------------------------------------

/// `documents.utils.get_boolean(str(query_params.get("full_perms", "false")))`.
pub fn get_boolean(s: &str) -> bool {
    matches!(s.to_lowercase().as_str(), "yes" | "y" | "1" | "t" | "true")
}

/// `full_perms` query parameter as `PassUserMixin` reads it.
pub fn parse_full_perms(value: Option<&str>) -> bool {
    get_boolean(value.unwrap_or("false"))
}

/// Which of the owned-object fields a serializer instance outputs
/// (`OwnedObjectSerializer.__init__`: pops depending on `full_perms` / `all_fields`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnedFields {
    pub permissions: bool,
    pub user_can_change: bool,
    /// Only meaningful for serializers that declare `is_shared_by_requester` (documents).
    pub is_shared_by_requester: bool,
}

impl OwnedFields {
    pub fn new(full_perms: bool, all_fields: bool) -> Self {
        if all_fields {
            Self { permissions: true, user_can_change: true, is_shared_by_requester: true }
        } else if full_perms {
            Self { permissions: true, user_can_change: false, is_shared_by_requester: false }
        } else {
            Self { permissions: false, user_can_change: true, is_shared_by_requester: true }
        }
    }
}

/// Direct grantees of one action (`PermissionGrantees`): ids ascending, no group expansion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grantees {
    pub users: Vec<i64>,
    pub groups: Vec<i64>,
}

/// `{"view": {...}, "change": {...}}` of one object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectGrantees {
    pub view: Grantees,
    pub change: Grantees,
}

impl ObjectGrantees {
    /// JSON exactly like `get_permissions`: `{"view":{"users":[..],"groups":[..]},"change":{..}}`.
    pub fn to_json(&self) -> Value {
        json!({
            "view": {"users": self.view.users, "groups": self.view.groups},
            "change": {"users": self.change.users, "groups": self.change.groups},
        })
    }
}

/// guardian grant rows of `model` for the objects `pks` (users and groups in one round trip,
/// O1: `UNION ALL` instead of two queries, the pks as one `text[]` parameter instead of one
/// parameter each – one prepared statement for every page size).
///
/// `codenames = Some((view, change))` restricts to those permissions of `model`'s content type
/// (`get_permission_grantees`); `None` returns every grant row (`get_shared_object_pks` needs
/// the pks with any grant), each with `model_perm` = the permission belongs to `model`'s
/// content type. Rows: users first, then groups, each ordered by user/group id.
async fn grant_rows(
    db: &PgPool,
    model: &ObjectModel,
    pks: &[i64],
    codenames: Option<(&str, &str)>,
) -> Result<Vec<GrantRow>, AuthError> {
    const CT: &str = "(SELECT id FROM django_content_type WHERE app_label = $1 AND model = $2)";
    let filter = if codenames.is_some() {
        format!(" AND p.content_type_id IN {CT} AND p.codename IN ($4, $5)")
    } else {
        String::new()
    };
    if crate::grants::use_grants() {
        // L5: index instead of the two guardian tables; same rows (kind, pk, codename, id,
        // model permission), same sorting.
        let sql = format!(
            "SELECT CASE WHEN g.subject > 0 THEN 0 ELSE 1 END::int4 AS kind, g.object_id::text AS object_pk,              p.codename::text AS codename, abs(g.subject)::int8 AS ident, (p.content_type_id = g.ct) AS model_perm              FROM paperrust_grant g JOIN auth_permission p ON p.id = g.perm_id              WHERE g.ct IN {CT} AND g.object_id = ANY($3){filter} ORDER BY kind, ident"
        );
        let mut q = sqlx::query_as::<_, GrantRow>(&sql).bind(model.app_label).bind(model.model_name).bind(pks);
        if let Some((view, change)) = codenames {
            q = q.bind(view).bind(change);
        }
        return Ok(q.fetch_all(db).await?);
    }
    let part = |table: &str, ident: &str, kind: i32| {
        format!(
            "SELECT {kind}::int4 AS kind, g.object_pk::text AS object_pk, p.codename::text AS codename, \
             g.{ident}::int8 AS ident, (p.content_type_id = g.content_type_id) AS model_perm \
             FROM {table} g JOIN auth_permission p ON p.id = g.permission_id \
             WHERE g.content_type_id IN {CT} AND g.object_pk = ANY($3){filter}"
        )
    };
    let sql = format!(
        "{} UNION ALL {} ORDER BY kind, ident",
        part("guardian_userobjectpermission", "user_id", 0),
        part("guardian_groupobjectpermission", "group_id", 1)
    );
    let keys: Vec<String> = pks.iter().map(|p| p.to_string()).collect();
    let mut q = sqlx::query_as::<_, GrantRow>(&sql).bind(model.app_label).bind(model.model_name).bind(keys);
    if let Some((view, change)) = codenames {
        q = q.bind(view).bind(change);
    }
    Ok(q.fetch_all(db).await?)
}

/// One guardian grant (`kind` 0 = user, 1 = group).
#[derive(sqlx::FromRow)]
struct GrantRow {
    kind: i32,
    object_pk: String,
    codename: String,
    ident: i64,
    model_perm: bool,
}

/// Fill `get_permission_grantees` entries from grant rows (only `view`/`change` of the model).
fn add_grantees(out: &mut HashMap<i64, ObjectGrantees>, rows: &[GrantRow], view: &str, change: &str) {
    for r in rows {
        if !r.model_perm || (r.codename != view && r.codename != change) {
            continue;
        }
        let Ok(pk) = r.object_pk.parse::<i64>() else { continue };
        let Some(entry) = out.get_mut(&pk) else { continue };
        let g = if r.codename == view { &mut entry.view } else { &mut entry.change };
        if r.kind == 0 { g.users.push(r.ident) } else { g.groups.push(r.ident) }
    }
}

/// `get_permission_grantees(model, pks)`: one query for any number of objects.
/// Every requested pk gets an entry.
pub async fn get_permission_grantees(
    db: &PgPool,
    model: &ObjectModel,
    pks: &[i64],
) -> Result<HashMap<i64, ObjectGrantees>, AuthError> {
    let mut out: HashMap<i64, ObjectGrantees> =
        pks.iter().map(|pk| (*pk, ObjectGrantees::default())).collect();
    if pks.is_empty() {
        return Ok(out);
    }
    let view = model.codename("view");
    let change = model.codename("change");
    let rows = grant_rows(db, model, pks, Some((&view, &change))).await?;
    add_grantees(&mut out, &rows, &view, &change);
    Ok(out)
}

/// pks (of the requested ones) with any user or group grant.
fn shared_pks(rows: &[GrantRow]) -> HashSet<i64> {
    rows.iter().filter_map(|r| r.object_pk.parse::<i64>().ok()).collect()
}

/// `OwnedObjectSerializer.get_shared_object_pks(objects)`: pks with any user or group grant.
pub async fn get_shared_object_pks(
    db: &PgPool,
    model: &ObjectModel,
    pks: &[i64],
) -> Result<HashSet<i64>, AuthError> {
    if pks.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(shared_pks(&grant_rows(db, model, pks, None).await?))
}

/// `get_user_can_change(obj)` with the page-level batch (`permission_grantees`).
///
/// `user = None` is AnonymousUser; `grantees = None` means "no entry for this pk" (→ false).
pub fn user_can_change(
    user: Option<&User>,
    owner_id: Option<i64>,
    grantees: Option<&ObjectGrantees>,
) -> bool {
    let Some(owner) = owner_id else { return true };
    let Some(user) = user else { return false };
    if owner == user.id {
        return true;
    }
    if user.is_active && user.is_superuser {
        return true;
    }
    let Some(g) = grantees else { return false };
    if g.change.users.contains(&user.id) {
        return true;
    }
    user.groups.iter().any(|gid| g.change.groups.contains(gid))
}

/// `get_is_shared_by_requester(obj)`: `obj.owner == user and obj.id in shared_object_pks`.
pub fn is_shared_by_requester(
    user: Option<&User>,
    owner_id: Option<i64>,
    id: i64,
    shared: &HashSet<i64>,
) -> bool {
    match (user, owner_id) {
        (Some(u), Some(o)) => o == u.id && shared.contains(&id),
        _ => false,
    }
}

/// Everything needed to render the owned-object fields of one page, in one query (grantees
/// and/or shared pks; before O1 up to three).
#[derive(Debug, Clone, Default)]
pub struct OwnedInfo {
    pub grantees: HashMap<i64, ObjectGrantees>,
    pub shared: HashSet<i64>,
}

impl OwnedInfo {
    /// Load grantees (needed for `permissions` and `user_can_change`) and, if
    /// `with_shared` (serializers with `is_shared_by_requester`), the shared pks.
    pub async fn load(
        db: &PgPool,
        model: &ObjectModel,
        pks: &[i64],
        fields: OwnedFields,
        with_shared: bool,
    ) -> Result<Self, AuthError> {
        let need_grantees = fields.permissions || fields.user_can_change;
        let need_shared = with_shared && fields.is_shared_by_requester;
        if need_grantees && need_shared && !pks.is_empty() {
            // O1: both from the same grant rows – one round trip instead of two.
            let view = model.codename("view");
            let change = model.codename("change");
            let rows = grant_rows(db, model, pks, None).await?;
            let mut grantees: HashMap<i64, ObjectGrantees> =
                pks.iter().map(|pk| (*pk, ObjectGrantees::default())).collect();
            add_grantees(&mut grantees, &rows, &view, &change);
            return Ok(Self { grantees, shared: shared_pks(&rows) });
        }
        let grantees = if need_grantees {
            get_permission_grantees(db, model, pks).await?
        } else {
            HashMap::new()
        };
        let shared = if need_shared {
            get_shared_object_pks(db, model, pks).await?
        } else {
            HashSet::new()
        };
        Ok(Self { grantees, shared })
    }

    /// `permissions` value of one object.
    pub fn permissions_json(&self, id: i64) -> Value {
        self.grantees.get(&id).cloned().unwrap_or_default().to_json()
    }

    /// `user_can_change` value of one object.
    pub fn user_can_change(&self, user: Option<&User>, id: i64, owner_id: Option<i64>) -> bool {
        user_can_change(user, owner_id, self.grantees.get(&id))
    }

    /// `is_shared_by_requester` value of one object.
    pub fn is_shared_by_requester(&self, user: Option<&User>, id: i64, owner_id: Option<i64>) -> bool {
        is_shared_by_requester(user, owner_id, id, &self.shared)
    }

    /// Append `owner`, then (as selected) `permissions`, `user_can_change`,
    /// `is_shared_by_requester` – the order all paperless serializers declare them in.
    /// Serializers that interleave other fields must call the single-field getters instead.
    pub fn write_fields(
        &self,
        out: &mut Map<String, Value>,
        fields: OwnedFields,
        with_shared: bool,
        user: Option<&User>,
        id: i64,
        owner_id: Option<i64>,
    ) {
        out.insert("owner".into(), owner_id.map(Value::from).unwrap_or(Value::Null));
        if fields.permissions {
            out.insert("permissions".into(), self.permissions_json(id));
        }
        if fields.user_can_change {
            out.insert("user_can_change".into(), Value::Bool(self.user_can_change(user, id, owner_id)));
        }
        if with_shared && fields.is_shared_by_requester {
            out.insert(
                "is_shared_by_requester".into(),
                Value::Bool(self.is_shared_by_requester(user, id, owner_id)),
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Write side
// ---------------------------------------------------------------------------------------------

/// Field-level validation error detail (DRF shape), to be placed under the field name
/// by the caller, e.g. `{"set_permissions": <detail>}` / `{"owner": <detail>}`.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldError(pub Value);

/// Python `type(x).__name__` for JSON values.
fn py_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python `str(x)` for JSON scalars (used in DRF messages).
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Null => "None".into(),
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                py_float_repr(f)
            } else {
                n.to_string()
            }
        }
        other => other.to_string(),
    }
}

fn py_float_repr(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 {
        format!("{f:.1}")
    } else if f.is_finite() && (f.abs() >= 1e16 || (f != 0.0 && f.abs() < 1e-4)) {
        // Python repr switches to exponent notation: 1e+16, 1e-05
        let s = format!("{f:e}");
        match s.split_once('e') {
            Some((m, e)) => {
                let (sign, digits) = match e.strip_prefix('-') {
                    Some(d) => ("-", d),
                    None => ("+", e),
                };
                format!("{m}e{sign}{digits:0>2}")
            }
            None => s,
        }
    } else {
        format!("{f}")
    }
}

/// Python `int(s)` for a string: whitespace, sign, digits with single underscores.
/// `Ok(None)` = valid integer outside i64 range.
fn py_int_str(s: &str) -> Result<Option<i64>, ()> {
    let t = s.trim();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return Err(());
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    let n: Option<i128> = clean.parse::<i128>().ok();
    Ok(n.map(|n| if neg { -n } else { n }).and_then(|n| i64::try_from(n).ok()))
}

const INT_INVALID: &str = "A valid integer is required.";
const INT_TOO_LONG: &str = "String value too large.";
const NULL_NOT_ALLOWED: &str = "This field may not be null.";

/// DRF `IntegerField().run_validation(v)`; `Ok(None)` = integer outside i64 (cannot exist as id).
fn drf_integer(v: &Value) -> Result<Option<i64>, &'static str> {
    match v {
        Value::Null => Err(NULL_NOT_ALLOWED),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Some(i))
            } else if n.is_u64() {
                Ok(None)
            } else {
                // int(re_decimal.sub('', str(float))): only integral floats printed without
                // exponent (|f| < 1e16) survive.
                match n.as_f64() {
                    Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 => Ok(Some(f as i64)),
                    _ => Err(INT_INVALID),
                }
            }
        }
        Value::String(s) => {
            if s.chars().count() > 1000 {
                return Err(INT_TOO_LONG);
            }
            // re_decimal = r'\.0*\s*$'
            let stripped = strip_re_decimal(s);
            py_int_str(stripped).map_err(|_| INT_INVALID)
        }
        _ => Err(INT_INVALID),
    }
}

fn strip_re_decimal(s: &str) -> &str {
    let t = s.trim_end();
    let t2 = t.trim_end_matches('0');
    match t2.strip_suffix('.') {
        Some(rest) => rest,
        None => s,
    }
}

/// Parsed `PermissionSetSerializer`: `None` = key absent; `Some(None)` = `null`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawGrantees {
    pub users: Option<Option<Vec<Option<i64>>>>,
    pub groups: Option<Option<Vec<Option<i64>>>>,
}

/// Parsed `SetPermissionsSerializer` (`view`/`change` absent = `None`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawSetPermissions {
    pub view: Option<RawGrantees>,
    pub change: Option<RawGrantees>,
}

fn not_a_dict(v: &Value) -> Value {
    json!({"non_field_errors": [format!("Invalid data. Expected a dictionary, but got {}.", py_type_name(v))]})
}

fn parse_id_list(v: &Value) -> Result<Option<Vec<Option<i64>>>, Value> {
    match v {
        Value::Null => Ok(None),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            let mut errors = Map::new();
            for (i, it) in items.iter().enumerate() {
                match drf_integer(it) {
                    Ok(x) => out.push(x),
                    Err(msg) => {
                        errors.insert(i.to_string(), json!([msg]));
                    }
                }
            }
            if errors.is_empty() { Ok(Some(out)) } else { Err(Value::Object(errors)) }
        }
        other => Err(json!([format!(
            "Expected a list of items but got type \"{}\".",
            py_type_name(other)
        )])),
    }
}

fn parse_permission_set(v: &Value) -> Result<RawGrantees, Value> {
    let Value::Object(m) = v else { return Err(not_a_dict(v)) };
    let mut errors = Map::new();
    let mut out = RawGrantees::default();
    if let Some(u) = m.get("users") {
        match parse_id_list(u) {
            Ok(x) => out.users = Some(x),
            Err(e) => {
                errors.insert("users".into(), e);
            }
        }
    }
    if let Some(g) = m.get("groups") {
        match parse_id_list(g) {
            Ok(x) => out.groups = Some(x),
            Err(e) => {
                errors.insert("groups".into(), e);
            }
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(Value::Object(errors)) }
}

/// `SetPermissionsSerializer().run_validation(value)` (JSON input).
pub fn parse_set_permissions(v: &Value) -> Result<RawSetPermissions, FieldError> {
    if v.is_null() {
        return Err(FieldError(json!([NULL_NOT_ALLOWED])));
    }
    let Value::Object(m) = v else { return Err(FieldError(not_a_dict(v))) };
    let unknown: BTreeSet<&String> = m.keys().filter(|k| *k != "view" && *k != "change").collect();
    if !unknown.is_empty() {
        let mut e = Map::new();
        for k in unknown {
            e.insert(k.clone(), Value::String("Unknown permission action.".into()));
        }
        return Err(FieldError(Value::Object(e)));
    }
    let mut errors = Map::new();
    let mut out = RawSetPermissions::default();
    for (key, slot) in [("view", &mut out.view), ("change", &mut out.change)] {
        if let Some(x) = m.get(key) {
            if x.is_null() {
                errors.insert(key.into(), json!([NULL_NOT_ALLOWED]));
                continue;
            }
            match parse_permission_set(x) {
                Ok(g) => *slot = Some(g),
                Err(e) => {
                    errors.insert(key.into(), e);
                }
            }
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(FieldError(Value::Object(errors))) }
}

/// Validated grants of one action: `None` = key absent (leave untouched),
/// `Some(ids)` = replace (or merge) with these ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GranteesUpdate {
    pub users: Option<Vec<i64>>,
    pub groups: Option<Vec<i64>>,
}

/// `validate_set_permissions` result (`PermissionsSpec`): action absent = `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionsSpec {
    pub view: Option<GranteesUpdate>,
    pub change: Option<GranteesUpdate>,
}

async fn validate_ids(
    db: &PgPool,
    table: &str,
    ids: &Option<Vec<Option<i64>>>,
    message: &str,
) -> Result<Result<Vec<i64>, FieldError>, AuthError> {
    let Some(ids) = ids else { return Ok(Ok(Vec::new())) };
    let valid: Vec<i64> = ids.iter().flatten().copied().collect();
    let q = Query::select()
        .expr(Expr::col(a("id")).cast_as(a("int8")))
        .from(a(table))
        .and_where(Expr::col(a("id")).is_in(valid.clone()))
        .to_owned();
    let found: Vec<(i64,)> = if valid.is_empty() {
        Vec::new()
    } else {
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        sqlx::query_as_with(&sql, values).fetch_all(db).await?
    };
    if found.len() != ids.len() {
        return Ok(Err(FieldError(json!([message]))));
    }
    let mut out: Vec<i64> = found.into_iter().map(|(i,)| i).collect();
    out.sort_unstable();
    Ok(Ok(out))
}

/// `OwnedObjectSerializer.validate_set_permissions(set_permissions)`:
/// users/groups must exist and be unique. Outer `Err` = database error, inner `Err` = 400.
pub async fn validate_set_permissions(
    db: &PgPool,
    raw: &RawSetPermissions,
) -> Result<Result<PermissionsSpec, FieldError>, AuthError> {
    let mut spec = PermissionsSpec::default();
    for (src, dst) in [(&raw.view, &mut spec.view), (&raw.change, &mut spec.change)] {
        let Some(g) = src else { continue };
        let mut upd = GranteesUpdate::default();
        if let Some(users) = &g.users {
            match validate_ids(db, "auth_user", users, "Some users in don't exist or were specified twice.")
                .await?
            {
                Ok(v) => upd.users = Some(v),
                Err(e) => return Ok(Err(e)),
            }
        }
        if let Some(groups) = &g.groups {
            match validate_ids(db, "auth_group", groups, "Some groups in don't exist or were specified twice.")
                .await?
            {
                Ok(v) => upd.groups = Some(v),
                Err(e) => return Ok(Err(e)),
            }
        }
        *dst = Some(upd);
    }
    Ok(Ok(spec))
}

/// `set_permissions_for_objects(permissions, model, pks, merge=)`.
///
/// Runs on a connection (use `&mut *tx` inside a transaction). Granting `change` also grants
/// `view`; without `merge`, the given action's direct grants are replaced.
pub async fn set_permissions_for_objects(
    conn: &mut PgConnection,
    spec: &PermissionsSpec,
    model: &ObjectModel,
    pks: &[i64],
    merge: bool,
) -> Result<(), AuthError> {
    if pks.is_empty() {
        return Ok(());
    }
    let object_pks: Vec<String> = pks.iter().map(|p| p.to_string()).collect();
    let ct_q = content_type_id_subquery(model);
    let (ct_sql, ct_vals) = ct_q.build_sqlx(PostgresQueryBuilder);
    let ct: Option<(i32,)> = sqlx::query_as_with(&ct_sql, ct_vals).fetch_optional(&mut *conn).await?;
    let Some((ct,)) = ct else {
        return Err(AuthError::internal(format!(
            "ContentType matching query does not exist: {}.{}",
            model.app_label, model.model_name
        )));
    };

    // Resolve every action before writing (_resolve_permissions).
    let mut resolved: Vec<(&str, &GranteesUpdate, Vec<i32>)> = Vec::new();
    for (action, entry) in [("view", &spec.view), ("change", &spec.change)] {
        let Some(entry) = entry else { continue };
        if entry.users.is_none() && entry.groups.is_none() {
            continue;
        }
        let mut codenames = vec![model.codename(action)];
        if action == "change" {
            codenames.push(model.codename("view"));
        }
        let q = Query::select()
            .column(a("id"))
            .column(a("codename"))
            .from(a("auth_permission"))
            .and_where(Expr::col(a("content_type_id")).eq(ct))
            .and_where(Expr::col(a("codename")).is_in(codenames.clone()))
            .to_owned();
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        let rows: Vec<(i32, String)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
        let mut missing: Vec<&String> =
            codenames.iter().filter(|c| !rows.iter().any(|(_, rc)| rc == *c)).collect();
        if !missing.is_empty() {
            missing.sort();
            let list: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            return Err(AuthError::internal(format!(
                "Permission matching query does not exist for codename(s): {}",
                list.join(", ")
            )));
        }
        resolved.push((action, entry, rows.into_iter().map(|(id, _)| id).collect()));
    }

    for (action, entry, perm_ids) in resolved {
        let codename = model.codename(action);
        for (ids, table, ident, ident_table) in [
            (&entry.users, "guardian_userobjectpermission", "user_id", "auth_user"),
            (&entry.groups, "guardian_groupobjectpermission", "group_id", "auth_group"),
        ] {
            let Some(ids) = ids else { continue };
            // add_ids = existing identities among ids
            let add_ids: Vec<i64> = if ids.is_empty() {
                Vec::new()
            } else {
                let q = Query::select()
                    .expr(Expr::col(a("id")).cast_as(a("int8")))
                    .from(a(ident_table))
                    .and_where(Expr::col(a("id")).is_in(ids.clone()))
                    .to_owned();
                let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
                let rows: Vec<(i64,)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
                rows.into_iter().map(|(i,)| i).collect()
            };
            let codename_ids = Query::select()
                .column(a("id"))
                .from(a("auth_permission"))
                .and_where(Expr::col(a("codename")).eq(codename.clone()))
                .to_owned();
            if !merge {
                let q = Query::select()
                    .distinct()
                    .expr(Expr::col(a(ident)).cast_as(a("int8")))
                    .from(a(table))
                    .and_where(Expr::col(a("content_type_id")).eq(ct))
                    .and_where(Expr::col(a("object_pk")).is_in(object_pks.clone()))
                    .and_where(Expr::col(a("permission_id")).in_subquery(codename_ids.clone()))
                    .to_owned();
                let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
                let existing: Vec<(i64,)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
                let remove: Vec<i64> = existing
                    .into_iter()
                    .map(|(i,)| i)
                    .filter(|i| !add_ids.contains(i))
                    .collect();
                if !remove.is_empty() {
                    let d = Query::delete()
                        .from_table(a(table))
                        .and_where(Expr::col(a("content_type_id")).eq(ct))
                        .and_where(Expr::col(a("object_pk")).is_in(object_pks.clone()))
                        .and_where(Expr::col(a("permission_id")).in_subquery(codename_ids.clone()))
                        .and_where(Expr::col(a(ident)).is_in(remove))
                        .to_owned();
                    let (sql, values) = d.build_sqlx(PostgresQueryBuilder);
                    sqlx::query_with(&sql, values).execute(&mut *conn).await?;
                }
            }
            if add_ids.is_empty() {
                continue;
            }
            // bulk_create(..., ignore_conflicts=True, batch_size=1000)
            let mut rows: Vec<(i32, &String, i64)> = Vec::new();
            for p in &perm_ids {
                for pk in &object_pks {
                    for id in &add_ids {
                        rows.push((*p, pk, *id));
                    }
                }
            }
            for chunk in rows.chunks(1000) {
                let mut ins = Query::insert();
                ins.into_table(a(table))
                    .columns([a("content_type_id"), a("object_pk"), a("permission_id"), a(ident)]);
                for (p, pk, id) in chunk {
                    ins.values_panic([ct.into(), (*pk).clone().into(), (*p).into(), (*id).into()]);
                }
                ins.on_conflict(OnConflict::new().do_nothing().to_owned());
                let (sql, values) = ins.build_sqlx(PostgresQueryBuilder);
                sqlx::query_with(&sql, values).execute(&mut *conn).await?;
            }
        }
    }
    Ok(())
}

/// DRF `PrimaryKeyRelatedField(queryset=User.objects.all(), allow_null=True)` for `owner`.
/// `Ok(Ok(id))` (`None` = null / unowned), `Ok(Err(_))` = 400 field error, `Err` = DB error.
pub async fn parse_owner(db: &PgPool, v: &Value) -> Result<Result<Option<i64>, FieldError>, AuthError> {
    let incorrect = |v: &Value| {
        FieldError(json!([format!(
            "Incorrect type. Expected pk value, received {}.",
            py_type_name(v)
        )]))
    };
    let id: Option<i64> = match v {
        Value::Null => return Ok(Ok(None)),
        Value::String(s) if s.is_empty() => return Ok(Ok(None)),
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => return Ok(Err(incorrect(v))),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else if n.is_f64() {
                // AutoField.get_prep_value: int(5.5) == 5
                match n.as_f64() {
                    Some(f) if f.is_finite() => (f.trunc().abs() < 9.2e18).then_some(f.trunc() as i64),
                    _ => return Ok(Err(incorrect(v))),
                }
            } else {
                None
            }
        }
        Value::String(s) => match py_int_str(s) {
            Ok(x) => x,
            Err(()) => return Ok(Err(incorrect(v))),
        },
    };
    let exists = match id {
        None => false,
        // auth_user.id is int4 → bind i32 (same text in pl-documents-write
        // `serializer::exists` with i32; i64 collided in the statement cache, BEFUNDE N4).
        // Outside the int4 range the row does not exist (as the int8 comparison did before).
        Some(id) => match i32::try_from(id) {
            Ok(id32) => {
                let (b,): (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM auth_user WHERE id = $1)")
                    .bind(id32)
                    .fetch_one(db)
                    .await?;
                b
            }
            Err(_) => false,
        },
    };
    match (exists, id) {
        (true, Some(id)) => Ok(Ok(Some(id))),
        _ => Ok(Err(FieldError(json!([format!(
            "Invalid pk \"{}\" - object does not exist.",
            py_str(v)
        )])))),
    }
}

/// `OwnedObjectSerializer.create`: the owner to store. `requested` is the validated `owner`
/// field if the request contained the key (`Some(None)` = explicit null), else `None`.
pub fn owner_on_create(requester: Option<&User>, requested: Option<Option<i64>>) -> Option<i64> {
    match requested {
        Some(v) => v,
        None => requester.map(|u| u.id),
    }
}

/// `OwnedObjectSerializer.update` guard: changing `owner` (to a different value) or passing
/// `set_permissions` requires superuser, owner or an unowned object → else 403
/// `Insufficient permissions.`
pub fn check_owner_update(
    requester: Option<&User>,
    instance_owner: Option<i64>,
    new_owner: Option<Option<i64>>,
    has_set_permissions: bool,
) -> Result<(), AuthError> {
    let is_superuser = requester.is_some_and(|u| u.is_superuser);
    let is_owner = requester.is_some_and(|u| instance_owner == Some(u.id));
    let is_unowned = instance_owner.is_none();
    let owner_changed = new_owner.is_some_and(|o| o != instance_owner);
    if (owner_changed || has_set_permissions) && !(is_superuser || is_owner || is_unowned) {
        return Err(AuthError::insufficient_permissions());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_selection() {
        assert_eq!(
            OwnedFields::new(true, false),
            OwnedFields { permissions: true, user_can_change: false, is_shared_by_requester: false }
        );
        assert_eq!(
            OwnedFields::new(false, false),
            OwnedFields { permissions: false, user_can_change: true, is_shared_by_requester: true }
        );
        assert!(parse_full_perms(Some("TRUE")));
        assert!(parse_full_perms(Some("1")));
        assert!(!parse_full_perms(Some("on")));
        assert!(!parse_full_perms(None));
    }

    #[test]
    fn can_change() {
        let mut u = crate::user::test_user(2);
        u.groups = vec![10];
        let mut g = ObjectGrantees::default();
        assert!(user_can_change(Some(&u), None, Some(&g)));
        assert!(user_can_change(Some(&u), Some(2), Some(&g)));
        assert!(!user_can_change(Some(&u), Some(1), Some(&g)));
        assert!(!user_can_change(None, Some(1), Some(&g)));
        g.change.groups = vec![10];
        assert!(user_can_change(Some(&u), Some(1), Some(&g)));
        g.change.groups.clear();
        g.change.users = vec![2];
        assert!(user_can_change(Some(&u), Some(1), Some(&g)));
        assert!(!user_can_change(Some(&u), Some(1), None));
        let mut su = crate::user::test_user(3);
        su.is_superuser = true;
        assert!(user_can_change(Some(&su), Some(1), None));
    }

    #[test]
    fn permissions_json_order() {
        let g = ObjectGrantees {
            view: Grantees { users: vec![1, 2], groups: vec![] },
            change: Grantees { users: vec![], groups: vec![3] },
        };
        assert_eq!(
            g.to_json().to_string(),
            r#"{"view":{"users":[1,2],"groups":[]},"change":{"users":[],"groups":[3]}}"#
        );
    }

    #[test]
    fn write_fields_order() {
        let info = OwnedInfo::default();
        let u = crate::user::test_user(1);
        let mut m = Map::new();
        m.insert("id".into(), json!(5));
        info.write_fields(&mut m, OwnedFields::new(false, false), true, Some(&u), 5, Some(1));
        assert_eq!(
            Value::Object(m).to_string(),
            r#"{"id":5,"owner":1,"user_can_change":true,"is_shared_by_requester":false}"#
        );
    }

    #[test]
    fn set_permissions_parsing() {
        assert_eq!(
            parse_set_permissions(&json!({"foo": 1, "bar": 2})).unwrap_err().0,
            json!({"bar": "Unknown permission action.", "foo": "Unknown permission action."})
        );
        assert_eq!(
            parse_set_permissions(&json!([1])).unwrap_err().0,
            json!({"non_field_errors": ["Invalid data. Expected a dictionary, but got list."]})
        );
        assert_eq!(parse_set_permissions(&Value::Null).unwrap_err().0, json!(["This field may not be null."]));
        assert_eq!(
            parse_set_permissions(&json!({"view": {"users": "x"}})).unwrap_err().0,
            json!({"view": {"users": ["Expected a list of items but got type \"str\"."]}})
        );
        assert_eq!(
            parse_set_permissions(&json!({"view": {"users": [1, "a", null, true]}})).unwrap_err().0,
            json!({"view": {"users": {
                "1": ["A valid integer is required."],
                "2": ["This field may not be null."],
                "3": ["A valid integer is required."]
            }}})
        );
        assert_eq!(
            parse_set_permissions(&json!({"change": 3})).unwrap_err().0,
            json!({"change": {"non_field_errors": ["Invalid data. Expected a dictionary, but got int."]}})
        );
        let ok = parse_set_permissions(&json!({"view": {"users": [1, "2", 3.0, " 4 "], "groups": null}})).unwrap();
        assert_eq!(
            ok.view,
            Some(RawGrantees { users: Some(Some(vec![Some(1), Some(2), Some(3), Some(4)])), groups: Some(None) })
        );
        assert_eq!(ok.change, None);
    }

    #[test]
    fn integers_like_drf() {
        assert_eq!(drf_integer(&json!(5)), Ok(Some(5)));
        assert_eq!(drf_integer(&json!(5.0)), Ok(Some(5)));
        assert_eq!(drf_integer(&json!(5.5)), Err(INT_INVALID));
        assert_eq!(drf_integer(&json!("7.000")), Ok(Some(7)));
        assert_eq!(drf_integer(&json!("1_000")), Ok(Some(1000)));
        assert_eq!(drf_integer(&json!("1e3")), Err(INT_INVALID));
        assert_eq!(drf_integer(&json!("99999999999999999999")), Ok(None));
        assert_eq!(drf_integer(&json!(1e20)), Err(INT_INVALID));
    }

    #[test]
    fn owner_rules() {
        let u = crate::user::test_user(1);
        assert_eq!(owner_on_create(Some(&u), None), Some(1));
        assert_eq!(owner_on_create(Some(&u), Some(None)), None);
        assert_eq!(owner_on_create(Some(&u), Some(Some(4))), Some(4));
        assert!(check_owner_update(Some(&u), Some(2), Some(Some(2)), false).is_ok());
        assert_eq!(
            check_owner_update(Some(&u), Some(2), Some(Some(1)), false).unwrap_err().detail(),
            "Insufficient permissions."
        );
        assert!(check_owner_update(Some(&u), Some(2), None, true).is_err());
        assert!(check_owner_update(Some(&u), None, Some(Some(3)), true).is_ok());
        assert!(check_owner_update(Some(&u), Some(1), Some(None), true).is_ok());
        assert_eq!(py_float_repr(5.0), "5.0");
        assert_eq!(py_float_repr(1e20), "1e+20");
    }
}
