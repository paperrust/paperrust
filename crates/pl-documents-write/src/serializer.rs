// upstream: src/documents/serialisers.py@v3.4.2 (DocumentSerializer: fields, to_internal_value
//           (old created datetime), validate (ASN in trash), CustomFieldInstanceSerializer.validate,
//           validate_documentlink_targets, OwnedObjectSerializer/SetPermissionsMixin.validate_set_permissions,
//           CorrespondentField/TagsField/DocumentTypeField/StoragePathField),
//           rest_framework 3.18.3 (Serializer.to_internal_value/run_validation, Field.validate_empty_values,
//           CharField, DateField, IntegerField + UniqueValidator, PrimaryKeyRelatedField,
//           ManyRelatedField, ListSerializer), django 5.2 (validators, dateparse.parse_date/parse_datetime)
//
//! Input validation of `DocumentSerializer` for PUT/PATCH with DRF error texts in field order.

use chrono::{NaiveDate, TimeZone};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use pl_authz::owned::{PermissionsSpec, parse_set_permissions};

use crate::cfi::FieldDef;
use crate::drf::{self, NULL, REQUIRED, msg, py_str, py_type_name};
use crate::error::ApiError;
use crate::user::CurrentUser;

/// Writable fields in `Meta.fields` order.
pub const WRITABLE: &[&str] = &[
    "correspondent",
    "document_type",
    "storage_path",
    "title",
    "content",
    "tags",
    "created",
    "created_date",
    "archive_serial_number",
    "owner",
    "set_permissions",
    "custom_fields",
    "remove_inbox_tags",
];

/// A validated element of `custom_fields`.
#[derive(Clone, Debug)]
pub struct CfItem {
    pub field: FieldDef,
    /// `None` = key missing (only with partial), otherwise the (possibly converted) value.
    pub value: Option<Value>,
}

/// `serializer.validated_data` (keys in field order; `None` = not contained).
#[derive(Clone, Debug, Default)]
pub struct Validated {
    pub correspondent: Option<Option<i64>>,
    pub document_type: Option<Option<i64>>,
    pub storage_path: Option<Option<i64>>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<i64>>,
    pub created: Option<NaiveDate>,
    pub created_date: Option<NaiveDate>,
    pub archive_serial_number: Option<Option<i64>>,
    pub owner: Option<Option<i64>>,
    pub set_permissions: Option<PermissionsSpec>,
    pub custom_fields: Option<Vec<CfItem>>,
    pub remove_inbox_tags: Option<Option<bool>>,
}

/// Result of the validation.
pub enum Outcome {
    Ok(Box<Validated>),
    /// 400 with `serializer.errors`
    Invalid(Value),
}

/// Errors that DRF does not collect as field errors (PermissionDenied → 403, exceptions → 500).
fn denied() -> ApiError {
    ApiError::Core(pl_core::ApiError::permission_denied_msg("Insufficient permissions."))
}

/// Django `parse_date` (Python 3.12 `date.fromisoformat` + Regex `\d{4}-\d{1,2}-\d{1,2}$`).
#[allow(clippy::result_unit_err)]
pub fn parse_date(s: &str) -> Result<Option<NaiveDate>, ()> {
    // date.fromisoformat
    let b = s.as_bytes();
    let all_digits = |x: &[u8]| x.iter().all(u8::is_ascii_digit);
    if b.len() == 10 && b[4] == b'-' && b[7] == b'-' && all_digits(&b[0..4]) && all_digits(&b[5..7]) && all_digits(&b[8..10]) {
        return NaiveDate::parse_from_str(s, "%Y-%m-%d").map(Some).map_err(|_| ());
    }
    if b.len() == 8 && all_digits(b) {
        return NaiveDate::parse_from_str(s, "%Y%m%d").map(Some).map_err(|_| ());
    }
    // ISO-Woche: YYYY-Www[-D] / YYYYWww[D]
    if let Some(d) = iso_week(s) {
        return d.map(Some);
    }
    // Regex-Fallback
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() == 3
        && parts[0].len() == 4
        && (1..=2).contains(&parts[1].len())
        && (1..=2).contains(&parts[2].len())
        && parts.iter().all(|p| p.bytes().all(|c| c.is_ascii_digit()))
    {
        let y: i32 = parts[0].parse().map_err(|_| ())?;
        let m: u32 = parts[1].parse().map_err(|_| ())?;
        let d: u32 = parts[2].parse().map_err(|_| ())?;
        return NaiveDate::from_ymd_opt(y, m, d).map(Some).ok_or(());
    }
    Ok(None)
}

fn iso_week(s: &str) -> Option<Result<NaiveDate, ()>> {
    let (year, rest) = (s.get(0..4)?, s.get(4..)?);
    if !year.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (w, d): (&str, Option<&str>) = if let Some(r) = rest.strip_prefix("-W") {
        match r.len() {
            2 => (r, None),
            4 if r.as_bytes()[2] == b'-' => (&r[0..2], Some(&r[3..4])),
            _ => return None,
        }
    } else {
        let r = rest.strip_prefix('W')?;
        match r.len() {
            2 => (r, None),
            3 => (&r[0..2], Some(&r[2..3])),
            _ => return None,
        }
    };
    if !w.bytes().all(|c| c.is_ascii_digit()) || !d.is_none_or(|d| d.bytes().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let y: i32 = year.parse().ok()?;
    let wk: u32 = w.parse().ok()?;
    let day: u32 = d.map(|d| d.parse().unwrap_or(0)).unwrap_or(1);
    let wd = match day {
        1 => chrono::Weekday::Mon,
        2 => chrono::Weekday::Tue,
        3 => chrono::Weekday::Wed,
        4 => chrono::Weekday::Thu,
        5 => chrono::Weekday::Fri,
        6 => chrono::Weekday::Sat,
        7 => chrono::Weekday::Sun,
        _ => return Some(Err(())),
    };
    Some(NaiveDate::from_isoywd_opt(y, wk, wd).ok_or(()))
}

pub const DATE_INVALID: &str = "Date has wrong format. Use one of these formats instead: YYYY-MM-DD.";

/// DRF `DateField.to_internal_value` (input from JSON).
pub fn drf_date(v: &Value) -> Result<NaiveDate, &'static str> {
    match v {
        Value::String(s) => match parse_date(s) {
            Ok(Some(d)) => Ok(d),
            _ => Err(DATE_INVALID),
        },
        _ => Err(DATE_INVALID),
    }
}

/// `DocumentSerializer.to_internal_value`: old datetime strings in `created` → local date.
pub fn convert_old_created(data: &mut Map<String, Value>, tz: chrono_tz::Tz) {
    let Some(Value::String(s)) = data.get("created") else { return };
    if !s.contains(':') {
        return;
    }
    if let Some(d) = parse_datetime_to_local_date(s, tz) {
        data.insert("created".into(), Value::String(d.format("%Y-%m-%d").to_string()));
    }
}

/// Django `parse_datetime` + `make_aware(…, current tz)` + `astimezone().date()`.
fn parse_datetime_to_local_date(s: &str, tz: chrono_tz::Tz) -> Option<NaiveDate> {
    let t = s.trim();
    // datetime.fromisoformat (Python 3.12) – common forms
    let norm = t.replace(' ', "T");
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&norm) {
        return Some(dt.with_timezone(&tz).date_naive());
    }
    for f in ["%Y-%m-%dT%H:%M:%S%.f%:z", "%Y-%m-%dT%H:%M:%S%:z", "%Y-%m-%dT%H:%M%:z", "%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%dT%H:%M%z"] {
        if let Ok(dt) = chrono::DateTime::parse_from_str(&norm, f) {
            return Some(dt.with_timezone(&tz).date_naive());
        }
    }
    let z = norm.strip_suffix('Z').map(|x| format!("{x}+00:00"));
    if let Some(z) = z
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&z)
    {
        return Some(dt.with_timezone(&tz).date_naive());
    }
    for f in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(&norm, f) {
            return tz.from_local_datetime(&n).earliest().map(|d| d.date_naive());
        }
    }
    None
}

/// DRF `CharField(allow_blank=True, trim_whitespace=True)` for JSON.
fn char_field(v: &Value, max_length: Option<usize>) -> Result<String, Value> {
    let s = match v {
        Value::Null => return Err(msg(NULL)),
        Value::String(s) => s.clone(),
        Value::Number(_) => py_str(v),
        _ => return Err(msg("Not a valid string.")),
    };
    let s = s.trim().to_string();
    let mut errs = Vec::new();
    if let Some(m) = max_length
        && s.chars().count() > m
    {
        errs.push(Value::String(format!("Ensure this field has no more than {m} characters.")));
    }
    if s.contains('\0') {
        errs.push(Value::String("Null characters are not allowed.".into()));
    }
    if errs.is_empty() { Ok(s) } else { Err(Value::Array(errs)) }
}

/// Python `int()` of the lookup value of a `PrimaryKeyRelatedField` (error → `incorrect_type`).
fn pk_value(v: &Value) -> Result<i64, Value> {
    let incorrect = || msg(format!("Incorrect type. Expected pk value, received {}.", py_type_name(v)));
    match v {
        Value::Bool(_) | Value::Array(_) | Value::Object(_) | Value::Null => Err(incorrect()),
        other => match drf::django_int_lookup(other) {
            Ok(Some(i)) => Ok(i),
            _ => Err(incorrect()),
        },
    }
}

async fn exists(conn: &mut PgConnection, table: &str, id: i64) -> Result<bool, sqlx::Error> {
    if !(i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&id) {
        return Ok(false);
    }
    let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = $1)");
    sqlx::query_scalar(&sql).bind(id as i32).fetch_one(&mut *conn).await
}

/// `PrimaryKeyRelatedField(allow_null=…)` incl. `'' → None`.
async fn pk_field(conn: &mut PgConnection, table: &str, v: &Value, allow_null: bool) -> Result<Result<Option<i64>, Value>, sqlx::Error> {
    let v = if v.as_str() == Some("") { &Value::Null } else { v };
    if v.is_null() {
        return Ok(if allow_null { Ok(None) } else { Err(msg(NULL)) });
    }
    let id = match pk_value(v) {
        Ok(i) => i,
        Err(e) => return Ok(Err(e)),
    };
    if exists(conn, table, id).await? {
        Ok(Ok(Some(id)))
    } else {
        Ok(Err(msg(format!("Invalid pk \"{}\" - object does not exist.", py_str(v)))))
    }
}

/// Validates `data` like `DocumentSerializer(instance, data, partial).is_valid()`.
///
/// `fields` = `?fields=` (DynamicFieldsModelSerializer removes all other fields, also when writing).
#[allow(clippy::too_many_arguments)]
pub async fn validate(
    conn: &mut PgConnection,
    user: &CurrentUser,
    instance_id: i64,
    data: &Map<String, Value>,
    partial: bool,
    fields: Option<&[String]>,
) -> Result<Outcome, ApiError> {
    let active = |f: &str| fields.is_none_or(|l| l.iter().any(|x| x == f));
    let mut errors = Map::new();
    let mut out = Validated::default();
    for &f in WRITABLE {
        if !active(f) {
            continue;
        }
        let Some(v) = data.get(f) else {
            // required: correspondent, document_type, storage_path, tags (without default)
            let required = matches!(f, "correspondent" | "document_type" | "storage_path" | "tags");
            if !partial {
                if required {
                    errors.insert(f.into(), msg(REQUIRED));
                } else if f == "remove_inbox_tags" {
                    out.remove_inbox_tags = Some(Some(false));
                }
            }
            continue;
        };
        match f {
            "correspondent" | "document_type" | "storage_path" => {
                let table = match f {
                    "correspondent" => "documents_correspondent",
                    "document_type" => "documents_documenttype",
                    _ => "documents_storagepath",
                };
                match pk_field(conn, table, v, true).await? {
                    Ok(id) => match f {
                        "correspondent" => out.correspondent = Some(id),
                        "document_type" => out.document_type = Some(id),
                        _ => out.storage_path = Some(id),
                    },
                    Err(e) => {
                        errors.insert(f.into(), e);
                    }
                }
            }
            "title" => match char_field(v, Some(128)) {
                Ok(s) => out.title = Some(s),
                Err(e) => {
                    errors.insert(f.into(), e);
                }
            },
            "content" => match char_field(v, None) {
                Ok(s) => out.content = Some(s),
                Err(e) => {
                    errors.insert(f.into(), e);
                }
            },
            "tags" => match tags_field(conn, v).await? {
                Ok(t) => out.tags = Some(t),
                Err(e) => {
                    errors.insert(f.into(), e);
                }
            },
            "created" | "created_date" => {
                if v.is_null() {
                    errors.insert(f.into(), msg(NULL));
                    continue;
                }
                match drf_date(v) {
                    Ok(d) => {
                        if f == "created" {
                            out.created = Some(d)
                        } else {
                            out.created_date = Some(d)
                        }
                    }
                    Err(e) => {
                        errors.insert(f.into(), msg(e));
                    }
                }
            }
            "archive_serial_number" => match asn_field(conn, instance_id, v).await? {
                Ok(x) => out.archive_serial_number = Some(x),
                Err(e) => {
                    errors.insert(f.into(), e);
                }
            },
            "owner" => match pk_field(conn, "auth_user", v, true).await? {
                Ok(x) => out.owner = Some(x),
                Err(e) => {
                    errors.insert(f.into(), e);
                }
            },
            "set_permissions" => match parse_set_permissions(v) {
                Err(e) => {
                    errors.insert(f.into(), e.0);
                }
                Ok(raw) => {
                    let pool_res = validate_set_permissions_conn(conn, &raw).await?;
                    match pool_res {
                        Ok(spec) => out.set_permissions = Some(spec),
                        Err(e) => {
                            errors.insert(f.into(), e);
                        }
                    }
                }
            },
            "custom_fields" => match custom_fields_field(conn, user, v, partial).await? {
                Ok(items) => out.custom_fields = Some(items),
                Err(e) => {
                    errors.insert(f.into(), e);
                }
            },
            "remove_inbox_tags" => {
                if v.is_null() {
                    out.remove_inbox_tags = Some(None);
                } else {
                    match drf::drf_bool(v) {
                        Some(b) => out.remove_inbox_tags = Some(Some(b)),
                        None => {
                            errors.insert(f.into(), msg(drf::INVALID_BOOL));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !errors.is_empty() {
        return Ok(Outcome::Invalid(Value::Object(errors)));
    }
    // DocumentSerializer.validate: ASN in trash
    if let Some(Some(asn)) = out.archive_serial_number {
        let in_trash: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM documents_document WHERE archive_serial_number = $1 AND deleted_at IS NOT NULL)",
        )
        .bind(asn)
        .fetch_one(&mut *conn)
        .await?;
        if in_trash {
            return Ok(Outcome::Invalid(json!({
                "archive_serial_number": ["Document with this Archive Serial Number already exists in the trash."]
            })));
        }
    }
    Ok(Outcome::Ok(Box::new(out)))
}

/// `validate_set_permissions` on one connection (users/groups must exist).
async fn validate_set_permissions_conn(
    conn: &mut PgConnection,
    raw: &pl_authz::owned::RawSetPermissions,
) -> Result<Result<PermissionsSpec, Value>, sqlx::Error> {
    let mut spec = PermissionsSpec::default();
    for (src, dst) in [(&raw.view, &mut spec.view), (&raw.change, &mut spec.change)] {
        let Some(g) = src else { continue };
        let mut upd = pl_authz::owned::GranteesUpdate::default();
        for (ids, table, message, slot) in [
            (&g.users, "auth_user", "Some users in don't exist or were specified twice.", 0),
            (&g.groups, "auth_group", "Some groups in don't exist or were specified twice.", 1),
        ] {
            let Some(ids) = ids else { continue };
            let Some(ids) = ids else {
                // users: null → values_list over none() → []
                if slot == 0 { upd.users = Some(vec![]) } else { upd.groups = Some(vec![]) }
                continue;
            };
            let valid: Vec<i32> = ids.iter().flatten().filter_map(|i| i32::try_from(*i).ok()).collect();
            let sql = format!("SELECT id FROM {table} WHERE id = ANY($1) ORDER BY id");
            let found: Vec<i32> = sqlx::query_scalar(&sql).bind(&valid).fetch_all(&mut *conn).await?;
            if found.len() != ids.len() {
                return Ok(Err(msg(message)));
            }
            let found: Vec<i64> = found.into_iter().map(i64::from).collect();
            if slot == 0 { upd.users = Some(found) } else { upd.groups = Some(found) }
        }
        *dst = Some(upd);
    }
    Ok(Ok(spec))
}

/// `TagsField(many=True)` (ManyRelatedField): list, each element an existing tag.
async fn tags_field(conn: &mut PgConnection, v: &Value) -> Result<Result<Vec<i64>, Value>, sqlx::Error> {
    let items: Vec<Value> = match v {
        Value::Null => return Ok(Err(msg(NULL))),
        Value::Array(a) => a.clone(),
        // dict is iterable (keys)
        Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
        other => {
            return Ok(Err(msg(format!("Expected a list of items but got type \"{}\".", py_type_name(other)))));
        }
    };
    let mut out = Vec::new();
    // ManyRelatedField calls child_relation.to_internal_value directly (no null/'' special case)
    for it in &items {
        if it.is_null() {
            return Ok(Err(msg("Invalid pk \"None\" - object does not exist.")));
        }
        let id = match pk_value(it) {
            Ok(i) => i,
            Err(e) => return Ok(Err(e)),
        };
        if !exists(conn, "documents_tag", id).await? {
            return Ok(Err(msg(format!("Invalid pk \"{}\" - object does not exist.", py_str(it)))));
        }
        out.push(id);
    }
    Ok(Ok(out))
}

/// `archive_serial_number`: IntegerField(min 0, max 4294967295, allow_null) + UniqueValidator.
async fn asn_field(conn: &mut PgConnection, instance_id: i64, v: &Value) -> Result<Result<Option<i64>, Value>, sqlx::Error> {
    if v.is_null() {
        return Ok(Ok(None));
    }
    let n = match drf::drf_int(v) {
        Ok(n) => n,
        Err(e) => return Ok(Err(msg(e))),
    };
    if n > 4_294_967_295 {
        return Ok(Err(msg("Ensure this value is less than or equal to 4294967295.")));
    }
    if n < 0 {
        return Ok(Err(msg("Ensure this value is greater than or equal to 0.")));
    }
    if n <= i64::from(i32::MAX) {
        let dup: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM documents_document WHERE archive_serial_number = $1 AND deleted_at IS NULL AND id <> $2)",
        )
        .bind(n as i32)
        .bind(instance_id as i32)
        .fetch_one(&mut *conn)
        .await?;
        if dup {
            return Ok(Err(msg("document with this archive serial number already exists.")));
        }
    }
    Ok(Ok(Some(n)))
}

/// `custom_fields = CustomFieldInstanceSerializer(many=True)`.
async fn custom_fields_field(
    conn: &mut PgConnection,
    user: &CurrentUser,
    v: &Value,
    partial: bool,
) -> Result<Result<Vec<CfItem>, Value>, ApiError> {
    let items = match v {
        Value::Null => return Ok(Err(msg(NULL))),
        Value::Array(a) => a,
        other => {
            return Ok(Err(json!({"non_field_errors": [format!(
                "Expected a list of items but got type \"{}\".",
                py_type_name(other)
            )]})));
        }
    };
    let mut ok = Vec::new();
    let mut errs = Vec::new();
    let mut any_err = false;
    for it in items {
        match validate_cfi(conn, user, it, partial).await? {
            Ok(item) => {
                ok.push(item);
                errs.push(json!({}));
            }
            Err(e) => {
                any_err = true;
                errs.push(e);
            }
        }
    }
    if any_err { Ok(Err(Value::Array(errs))) } else { Ok(Ok(ok)) }
}

/// `CustomFieldInstanceSerializer(data=item).is_valid()` (incl. `validate`).
/// `Err(ApiError)` = PermissionDenied (403) or upstream exception (500).
pub async fn validate_cfi(
    conn: &mut PgConnection,
    user: &CurrentUser,
    it: &Value,
    partial: bool,
) -> Result<Result<CfItem, Value>, ApiError> {
    let Value::Object(m) = it else {
        return Ok(Err(json!({"non_field_errors": [format!(
            "Invalid data. Expected a dictionary, but got {}.",
            py_type_name(it)
        )]})));
    };
    let mut errors = Map::new();
    // Field "value" (ReadWriteSerializerMethodField, allow_null, required)
    let value: Option<Value> = match m.get("value") {
        Some(v) => Some(v.clone()),
        None => {
            if !partial {
                errors.insert("value".into(), msg(REQUIRED));
            }
            None
        }
    };
    let mut field: Option<FieldDef> = None;
    match m.get("field") {
        None => {
            if !partial {
                errors.insert("field".into(), msg(REQUIRED));
            }
        }
        Some(fv) => {
            let fv2 = if fv.as_str() == Some("") { &Value::Null } else { fv };
            if fv2.is_null() {
                errors.insert("field".into(), msg(NULL));
            } else {
                match pk_value(fv2) {
                    Err(e) => {
                        errors.insert("field".into(), e);
                    }
                    Ok(id) => {
                        let def = if (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&id) {
                            crate::cfi::field_def(conn, id).await?
                        } else {
                            None
                        };
                        match def {
                            Some(d) => field = Some(d),
                            None => {
                                errors.insert(
                                    "field".into(),
                                    msg(format!("Invalid pk \"{}\" - object does not exist.", py_str(fv2))),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    if !errors.is_empty() {
        return Ok(Err(Value::Object(errors)));
    }
    let Some(field) = field else {
        // partial without "field": data["field"] → KeyError
        return Err(ApiError::Uncaught("KeyError: 'field'".into()));
    };
    let nf = |m: String| Ok(Err(json!({"non_field_errors": [m]})));
    let mut value = value;
    if let Some(v) = value.clone().filter(|v| !v.is_null()) {
        match field.data_type.as_str() {
            "url" => {
                let Some(s) = v.as_str() else { return Err(ApiError::Uncaught("TypeError: len()".into())) };
                if !s.is_empty()
                    && let Err(e) = uri_validator(s)
                {
                    return nf(e);
                }
            }
            "integer" => {
                // integer_validator on str(value)
                let sv = py_str(&v);
                let ok_re = {
                    let t = sv.strip_prefix('-').unwrap_or(&sv);
                    !t.is_empty() && t.chars().all(|c| c.is_numeric())
                };
                if !ok_re {
                    return nf("Enter a valid integer.".into());
                }
                let n = match drf::django_int_lookup(&v) {
                    Ok(Some(n)) => n,
                    _ => return nf("Enter a valid integer.".into()),
                };
                if n < -2_147_483_648 {
                    return nf("Ensure this value is greater than or equal to -2147483648.".into());
                }
                if n > 2_147_483_647 {
                    return nf("Ensure this value is less than or equal to 2147483647.".into());
                }
            }
            "monetary" => {
                if v.as_str() != Some("") && !decimal_ok(&py_str(&v)) && !monetary_re(&py_str(&v)) {
                    return nf("Must be a two-decimal number with optional currency code e.g. GBP123.45".into());
                }
            }
            "string" => {
                let Some(s) = v.as_str() else { return Err(ApiError::Uncaught("TypeError: len()".into())) };
                let n = s.chars().count();
                if n > 128 {
                    return nf(format!("Ensure this value has at most 128 characters (it has {n})."));
                }
            }
            "select" => {
                let opts = field
                    .extra_data
                    .as_ref()
                    .and_then(|e| e.get("select_options"))
                    .cloned()
                    .ok_or_else(|| ApiError::Uncaught("KeyError: 'select_options'".into()))?;
                let found = opts
                    .as_array()
                    .is_some_and(|a| a.iter().any(|o| o.get("id").is_some_and(|id| py_eq(id, &v))));
                if !found {
                    return nf(format!("Value must be an id of an element in {}", drf::py_repr(&opts)));
                }
            }
            "documentlink" => {
                let Value::Array(ids) = &v else { return nf("Value must be a list".into()) };
                let mut parsed = Vec::new();
                for i in ids {
                    match drf::django_int_lookup(i) {
                        Ok(Some(n)) => parsed.push(n),
                        _ => return Err(ApiError::Uncaught("ValueError in id__in".into())),
                    }
                }
                let p32: Vec<i32> = parsed.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL",
                )
                .bind(&p32)
                .fetch_one(&mut *conn)
                .await?;
                if count as usize != parsed.len() {
                    return nf("Some documents in value don't exist or were specified twice.".into());
                }
                if !permitted_change_all(conn, user, &p32).await? {
                    return Err(denied());
                }
            }
            "date" => match drf_date(&v) {
                Ok(d) => value = Some(Value::String(d.format("%Y-%m-%d").to_string())),
                Err(e) => return nf(e.into()),
            },
            _ => {}
        }
    }
    Ok(Ok(CfItem { field, value }))
}

/// Python `==` between option id and value (number == number, string == string).
fn py_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Bool(x), Value::Number(y)) | (Value::Number(y), Value::Bool(x)) => y.as_f64() == Some(if *x { 1.0 } else { 0.0 }),
        _ => a == b,
    }
}

/// `Document.objects.filter(id__in=ids).exclude(id__in=permitted_document_ids(user, "change_document")).exists()` == false.
async fn permitted_change_all(conn: &mut PgConnection, user: &CurrentUser, ids: &[i32]) -> Result<bool, ApiError> {
    use sea_query::{Expr, PostgresQueryBuilder, Query};
    use pl_core::sqlbind::SqlxBinder;
    if ids.is_empty() {
        return Ok(true);
    }
    let permitted = pl_authz::objects::permitted_document_ids(Some(user), "change_document", false);
    let q = Query::select()
        .expr(Expr::val(1))
        .from(sea_query::Alias::new("documents_document"))
        .and_where(Expr::col(sea_query::Alias::new("id")).is_in(ids.iter().copied()))
        .and_where(Expr::col(sea_query::Alias::new("deleted_at")).is_null())
        .and_where(Expr::col(sea_query::Alias::new("id")).not_in_subquery(permitted))
        .limit(1)
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(&mut *conn).await?;
    Ok(row.is_none())
}

/// `^[A-Z]{3}-?\d+(\.\d{1,2})$` (Python `re.search`, `$` allows a trailing `\n`).
fn monetary_re(s: &str) -> bool {
    let s = s.strip_suffix('\n').unwrap_or(s);
    let b: Vec<char> = s.chars().collect();
    if b.len() < 3 || !b[..3].iter().all(|c| c.is_ascii_uppercase()) {
        return false;
    }
    let mut i = 3;
    if b.get(i) == Some(&'-') {
        i += 1;
    }
    let start = i;
    while i < b.len() && b[i].is_numeric() {
        i += 1;
    }
    if i == start || b.get(i) != Some(&'.') {
        return false;
    }
    let dec = &b[i + 1..];
    (1..=2).contains(&dec.len()) && dec.iter().all(|c| c.is_numeric())
}

/// `DecimalValidator(max_digits=12, decimal_places=2)(Decimal(str(value)))` succeeds.
fn decimal_ok(s: &str) -> bool {
    let t = s.trim();
    let (mant, exp) = match t.find(['e', 'E']) {
        Some(p) => match t[p + 1..].parse::<i64>() {
            Ok(e) => (&t[..p], e),
            Err(_) => return false,
        },
        None => (t, 0),
    };
    let mant = mant.strip_prefix(['+', '-']).unwrap_or(mant);
    let (int_part, frac_part) = match mant.split_once('.') {
        Some((a, b)) => (a, b),
        None => (mant, ""),
    };
    if (int_part.is_empty() && frac_part.is_empty())
        || !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return false;
    }
    let digits_str = format!("{int_part}{frac_part}");
    let exponent = exp - frac_part.len() as i64;
    let trimmed = digits_str.trim_start_matches('0');
    let digit_tuple_len = if trimmed.is_empty() { 1 } else { trimmed.len() } as i64;
    let is_zero = trimmed.is_empty();
    let (digits, decimals) = if exponent >= 0 {
        (if is_zero { 1 } else { digit_tuple_len + exponent }, 0)
    } else if -exponent > digit_tuple_len {
        (-exponent, -exponent)
    } else {
        (digit_tuple_len, -exponent)
    };
    let whole = digits - decimals;
    digits <= 12 && decimals <= 2 && whole <= 10
}

/// `documents.validators.uri_validator` (Python `urllib.parse.urlparse`).
pub fn uri_validator(value: &str) -> Result<(), String> {
    // urlsplit: scheme = characters before ':' if valid
    let mut scheme = "";
    let mut rest = value;
    if let Some(i) = value.find(':')
        && i > 0
    {
        let cand = &value[..i];
        let first_ok = cand.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        if first_ok && cand.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) {
            scheme = cand;
            rest = &value[i + 1..];
        }
    }
    if scheme.is_empty() {
        return Err(format!("Unable to parse URI {value}, missing scheme"));
    }
    let (netloc, path) = if let Some(r) = rest.strip_prefix("//") {
        let end = r.find(['/', '?', '#']).unwrap_or(r.len());
        (&r[..end], &r[end..])
    } else {
        ("", rest)
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    if netloc.is_empty() && path.is_empty() {
        return Err(format!("Unable to parse URI {value}, missing net location or path"));
    }
    // urlsplit raises ValueError for an invalid IPv6 netloc
    if (netloc.contains('[') && !netloc.contains(']')) || (netloc.contains(']') && !netloc.contains('[')) {
        return Err(format!("Unable to parse URI {value}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(parse_date("2024-01-05"), Ok(NaiveDate::from_ymd_opt(2024, 1, 5)));
        assert_eq!(parse_date("20240105"), Ok(NaiveDate::from_ymd_opt(2024, 1, 5)));
        assert_eq!(parse_date("2024-1-5"), Ok(NaiveDate::from_ymd_opt(2024, 1, 5)));
        assert_eq!(parse_date("2024-02-30"), Err(()));
        assert_eq!(parse_date("x"), Ok(None));
        assert_eq!(parse_date("2024-W01-1"), Ok(NaiveDate::from_ymd_opt(2024, 1, 1)));
    }

    #[test]
    fn money() {
        assert!(decimal_ok("12.5"));
        assert!(decimal_ok("1234567890.12"));
        assert!(!decimal_ok("12.345"));
        assert!(!decimal_ok("EUR12.50"));
        assert!(monetary_re("EUR12.50"));
        assert!(!monetary_re("EUR12"));
        assert!(monetary_re("EUR-1.5"));
    }

    #[test]
    fn uris() {
        assert!(uri_validator("https://example.com").is_ok());
        assert!(uri_validator("mailto:x@y").is_ok());
        assert_eq!(uri_validator("example.com").unwrap_err(), "Unable to parse URI example.com, missing scheme");
        assert_eq!(uri_validator("http:").unwrap_err(), "Unable to parse URI http:, missing net location or path");
    }

    #[test]
    fn old_created() {
        let mut m = Map::new();
        m.insert("created".into(), json!("2024-03-01T23:30:00Z"));
        convert_old_created(&mut m, chrono_tz::Europe::Zurich);
        assert_eq!(m["created"], json!("2024-03-02"));
    }
}
