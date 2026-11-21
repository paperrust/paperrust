//! `/api/config/` – `ApplicationConfigurationViewSet` (read side; writes: [`crate::config_write`]).
//!
//! upstream: src/paperless/views.py@v3.4.2 (`ApplicationConfigurationViewSet`),
//! src/paperless/serialisers.py@v3.4.2 (`ApplicationConfigurationSerializer`),
//! src/paperless/models.py@v3.4.2 (`ApplicationConfiguration`),
//! src/paperless_mail/serialisers.py@v3.4.2 (`ObfuscatedPasswordField`),
//! rest_framework/fields.py@3.18.3 (`JSONField(binary=True)`, `FileField.to_representation`),
//! django/core/files/storage/filesystem.py@5.2 (`FileSystemStorage.url`).
//! Permissions: `(IsAuthenticated, DjangoModelPermissions)` – in v3.4.2 GET needs no model permission.
//! Upstream fix 53d9aa34a (#14709, after v3.4.2): `PaperlessModelPermissions` – GET/HEAD/OPTIONS
//! need `paperless.view_applicationconfiguration` (adopted here, see [`require_view`]).

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::AuthUser;
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{ApiError, AppState, Drf, RequestContext};
use sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query};
use serde_json::{Map, Value};
use sqlx::Row as _;
use sqlx::postgres::PgRow;

use crate::error::Result;

const TABLE: &str = "paperless_applicationconfiguration";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Str,
    SmallInt,
    Int,
    Float,
    Bool,
    /// `JSONField(binary=True)`: `json.dumps(value)` as a string.
    JsonBinary,
    /// `ObfuscatedPasswordField`: constant `"**********"` (#14784).
    Obfuscated,
    /// `FileField` → absolute URL.
    File,
}

/// Model fields in declaration order (`_meta.fields` without the pk).
const MODEL_FIELDS: &[(&str, Kind)] = &[
    ("output_type", Kind::Str),
    ("pages", Kind::SmallInt),
    ("language", Kind::Str),
    ("mode", Kind::Str),
    ("archive_file_generation", Kind::Str),
    ("image_dpi", Kind::SmallInt),
    ("unpaper_clean", Kind::Str),
    ("deskew", Kind::Bool),
    ("rotate_pages", Kind::Bool),
    ("rotate_pages_threshold", Kind::Float),
    ("max_image_pixels", Kind::Float),
    ("color_conversion_strategy", Kind::Str),
    ("user_args", Kind::JsonBinary),
    ("app_title", Kind::Str),
    ("app_logo", Kind::File),
    ("barcodes_enabled", Kind::Bool),
    ("barcode_enable_tiff_support", Kind::Bool),
    ("barcode_string", Kind::Str),
    ("barcode_retain_split_pages", Kind::Bool),
    ("barcode_enable_asn", Kind::Bool),
    ("barcode_asn_prefix", Kind::Str),
    ("barcode_upscale", Kind::Float),
    ("barcode_dpi", Kind::SmallInt),
    ("barcode_max_pages", Kind::SmallInt),
    ("barcode_enable_tag", Kind::Bool),
    ("barcode_tag_mapping", Kind::JsonBinary),
    ("barcode_tag_split", Kind::Bool),
    ("barcode_store_values", Kind::Bool),
    ("remote_ocr_engine", Kind::Str),
    ("remote_ocr_api_key", Kind::Obfuscated),
    ("remote_ocr_endpoint", Kind::Str),
    ("remote_ocr_mode", Kind::Str),
    ("ai_enabled", Kind::Bool),
    ("llm_embedding_backend", Kind::Str),
    ("llm_embedding_model", Kind::Str),
    ("llm_embedding_api_key", Kind::Obfuscated),
    ("llm_embedding_endpoint", Kind::Str),
    ("llm_embedding_chunk_size", Kind::SmallInt),
    ("llm_context_size", Kind::Int),
    ("llm_backend", Kind::Str),
    ("llm_model", Kind::Str),
    ("llm_api_key", Kind::Obfuscated),
    ("llm_endpoint", Kind::Str),
    ("llm_output_language", Kind::Str),
    ("llm_request_timeout", Kind::SmallInt),
];

/// Declared serializer fields (after `id`), in class order.
const DECLARED: &[&str] =
    &["externally_configured_variables", "user_args", "barcode_tag_mapping", "llm_embedding_api_key", "llm_api_key", "remote_ocr_api_key"];

/// `get_externally_configured_variables`: sorted variable names (values never exposed).
/// Upstream: `sorted(name for name in os.environ if name.startswith("PAPERLESS_"))`.
/// PaperRust reads `PAPERRUST_X`, `PAPERLESS_X` and the transitional `PAPERLESS_RS_X` as the
/// same setting, so each counts as `PAPERLESS_X` and is listed **once** under that name.
/// PaperRust's own settings (`pl_core::config::OWN_SETTINGS`, `PAPERRUST_*` keys of the settings
/// schema, e.g. `PAPERRUST_BIND`) are not listed: Django has no such setting. An unknown
/// `PAPERRUST_X`/`PAPERLESS_RS_X` (no upstream setting `PAPERLESS_X` in the schema) is
/// PaperRust-specific too; an unknown `PAPERLESS_X` is listed like upstream.
///
/// At startup an unknown `PAPERRUST_X` is mirrored into the process environment as
/// `PAPERLESS_X` (`pl_core::config::apply_env_aliases`); such a mirror is not listed either
/// (parity run 2026-11-20: `PAPERRUST_UPDATE_CHECK` showed up as `PAPERLESS_UPDATE_CHECK`).
pub fn externally_configured_variables(names: impl Iterator<Item = String>) -> Vec<String> {
    use pl_core::config::{LEGACY_PREFIX, PREFIX, UPSTREAM_PREFIX};
    let names: Vec<String> = names.collect();
    let mirrored: std::collections::HashSet<String> = names
        .iter()
        .filter_map(|n| n.strip_prefix(LEGACY_PREFIX).or_else(|| n.strip_prefix(PREFIX)))
        .filter(|rest| !rest.is_empty())
        .map(|rest| format!("{UPSTREAM_PREFIX}{rest}"))
        .filter(|up| !pl_settings::schema::SCHEMA.iter().any(|d| d.key == up.as_str()))
        .collect();
    let mut v: Vec<String> = names.iter().filter(|n| !mirrored.contains(n.as_str())).filter_map(|n| upstream_variable(n)).collect();
    v.sort();
    v.dedup();
    v
}

/// The upstream name (`PAPERLESS_X`) a set variable stands for, `None` if it is not an
/// upstream setting (see [`externally_configured_variables`]).
fn upstream_variable(name: &str) -> Option<String> {
    use pl_core::config::{LEGACY_PREFIX, PREFIX, UPSTREAM_PREFIX, is_own_setting};
    let (rest, own_prefix) = if let Some(r) = name.strip_prefix(LEGACY_PREFIX) {
        (r, true)
    } else if let Some(r) = name.strip_prefix(PREFIX) {
        (r, true)
    } else {
        (name.strip_prefix(UPSTREAM_PREFIX)?, false)
    };
    if rest.is_empty() {
        return None;
    }
    let upstream = format!("{UPSTREAM_PREFIX}{rest}");
    let own = format!("{PREFIX}{rest}");
    let in_schema = |k: &str| pl_settings::schema::SCHEMA.iter().any(|d| d.key == k);
    if is_own_setting(&own) || (in_schema(&own) && !in_schema(&upstream)) {
        return None;
    }
    if own_prefix && !in_schema(&upstream) {
        return None;
    }
    Some(upstream)
}

/// Python `json.dumps(value)` (default separators, `ensure_ascii=True`).
pub fn py_json_dumps(v: &Value) -> String {
    let mut out = String::new();
    dump(v, &mut out);
    out
}

fn dump(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_f64() {
                out.push_str(&pl_core::json::python_float_repr(n.as_f64().unwrap_or(0.0)));
            } else {
                out.push_str(&n.to_string());
            }
        }
        Value::String(s) => dump_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump_str(k, out);
                out.push_str(": ");
                dump(x, out);
            }
            out.push('}');
        }
    }
}

fn dump_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `ObfuscatedPasswordField.to_representation` (upstream 64bf7f845 / #14784, after v3.4.2: constant mask; v3.4.2 sent `"*" * max(10, len(value))`, which leaked the length).
pub fn obfuscate(_s: &str) -> String {
    "*".repeat(10)
}

/// Django `filepath_to_uri(name)`.
fn filepath_to_uri(name: &str) -> String {
    pl_core::urlenc::quote(&name.replace('\\', "/"), "/~!*()'")
}

fn field_value(row: &PgRow, name: &str, kind: Kind, ctx: &RequestContext, script_prefix: &str) -> Result<Value> {
    Ok(match kind {
        Kind::Str => row.try_get::<Option<String>, _>(name)?.map_or(Value::Null, Value::from),
        Kind::SmallInt => row.try_get::<Option<i16>, _>(name)?.map_or(Value::Null, Value::from),
        Kind::Int => row.try_get::<Option<i32>, _>(name)?.map_or(Value::Null, Value::from),
        Kind::Float => row.try_get::<Option<f64>, _>(name)?.and_then(serde_json::Number::from_f64).map_or(Value::Null, Value::Number),
        Kind::Bool => row.try_get::<Option<bool>, _>(name)?.map_or(Value::Null, Value::from),
        Kind::JsonBinary => match row.try_get::<Option<Value>, _>(name)? {
            None | Some(Value::Null) => Value::Null,
            Some(v) => Value::from(py_json_dumps(&v)),
        },
        Kind::Obfuscated => row.try_get::<Option<String>, _>(name)?.map_or(Value::Null, |s| Value::from(obfuscate(&s))),
        Kind::File => match row.try_get::<Option<String>, _>(name)? {
            None => Value::Null,
            Some(n) if n.is_empty() => Value::Null,
            Some(n) => {
                // storage.url(): urljoin(MEDIA_URL, filepath_to_uri(name).lstrip("/")); MEDIA_URL is
                // unset → script prefix ("/" or FORCE_SCRIPT_NAME + "/")
                let url = format!("{script_prefix}{}", filepath_to_uri(&n).trim_start_matches('/'));
                Value::from(ctx.build_absolute_uri_for(&url))
            }
        },
    })
}

fn serialize_row(row: &PgRow, ctx: &RequestContext, script_prefix: &str, env: &[String]) -> Result<Value> {
    let mut m = Map::new();
    m.insert("id".into(), Value::from(row.try_get::<i32, _>("id")?));
    for d in DECLARED {
        if *d == "externally_configured_variables" {
            m.insert((*d).into(), Value::from(env.to_vec()));
        } else if let Some((n, k)) = MODEL_FIELDS.iter().find(|(n, _)| n == d) {
            m.insert((*n).into(), field_value(row, n, *k, ctx, script_prefix)?);
        }
    }
    for (n, k) in MODEL_FIELDS {
        if DECLARED.contains(n) {
            continue;
        }
        m.insert((*n).into(), field_value(row, n, *k, ctx, script_prefix)?);
    }
    Ok(Value::Object(m))
}

fn select(pk: Option<i32>) -> sea_query::SelectStatement {
    let mut q = Query::select();
    q.column(Alias::new("id"));
    for (n, _) in MODEL_FIELDS {
        q.column(Alias::new(*n));
    }
    q.from(Alias::new(TABLE));
    if let Some(pk) = pk {
        q.and_where(Expr::col(Alias::new("id")).eq(pk));
    } else {
        // no Meta.ordering: Django emits no ORDER BY; ordering by pk keeps the (singleton) output stable
        q.order_by(Alias::new("id"), Order::Asc);
    }
    q
}

fn script_prefix(state: &AppState) -> String {
    state.config().base_url.clone()
}

/// `os.environ` of Django = process environment + `paperless.conf` (`Config::env`).
fn env_names(state: &AppState) -> Vec<String> {
    externally_configured_variables(state.config().env.vars().keys().cloned())
}

/// Model column names in declaration order (without the pk).
pub fn model_field_names() -> impl Iterator<Item = &'static str> {
    MODEL_FIELDS.iter().map(|(n, _)| *n)
}

/// `serializer.data` of the row `id` (after an update).
pub async fn serialize_by_id(state: &AppState, ctx: &RequestContext, id: i32) -> Result<Value> {
    let (sql, values) = select(Some(id)).build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(state.db()).await?;
    let Some(row) = row else { return Err(ApiError::not_found_model("ApplicationConfiguration").into()) };
    serialize_row(&row, ctx, &script_prefix(state), &env_names(state))
}

/// `PaperlessModelPermissions` for GET/HEAD (upstream fix 53d9aa34a / #14709, after v3.4.2):
/// `paperless.view_applicationconfiguration`, checked before the object lookup.
fn require_view(auth: &AuthUser) -> Result<()> {
    if !auth.user().has_perm("paperless.view_applicationconfiguration") {
        return Err(pl_authz::AuthError::permission_denied().into());
    }
    Ok(())
}

/// `GET /api/config/`
pub async fn list(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response> {
    require_view(&auth)?;
    let (sql, values) = select(None).build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(state.db()).await?;
    let prefix = script_prefix(&state);
    let env = env_names(&state);
    let data: Result<Vec<Value>> = rows.iter().map(|r| serialize_row(r, &ctx, &prefix, &env)).collect();
    Ok(Drf::ok(Value::Array(data?)).into_response())
}

/// `GET /api/config/{pk}/`
pub async fn retrieve(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    UrlPath(pk): UrlPath<String>,
) -> Result<Response> {
    require_view(&auth)?;
    let Some(pk) = crate::py::int(&pk) else {
        return Err(ApiError::not_found().into());
    };
    let Ok(id) = i32::try_from(pk) else {
        return Err(ApiError::not_found_model("ApplicationConfiguration").into());
    };
    let (sql, values) = select(Some(id)).build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(state.db()).await?;
    let Some(row) = row else { return Err(ApiError::not_found_model("ApplicationConfiguration").into()) };
    let v = serialize_row(&row, &ctx, &script_prefix(&state), &env_names(&state))?;
    Ok(Drf::ok(v).into_response())
}

/// `POST /api/config/` – `DjangoModelPermissions` (add) first, then `Response(status=405)`.
pub async fn create(_ctx: RequestContext, auth: AuthUser) -> Result<Response> {
    if !auth.user().has_perm("paperless.add_applicationconfiguration") {
        return Err(pl_authz::AuthError::permission_denied().into());
    }
    // Response(status=405) with data None: empty body, DRF drops the Content-Type
    Ok(StatusCode::METHOD_NOT_ALLOWED.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_binary_like_python() {
        assert_eq!(py_json_dumps(&json!({"a": 1, "b": [1.0, "x"]})), r#"{"a": 1, "b": [1.0, "x"]}"#);
        let key: String = char::from_u32(0xe4).into_iter().collect();
        let val: String = char::from_u32(0x1F600).into_iter().collect();
        let bs = char::from(92u8);
        let mut obj = serde_json::Map::new();
        obj.insert(key, Value::from(val));
        assert_eq!(py_json_dumps(&Value::Object(obj)), format!("{{\"{bs}u00e4\": \"{bs}ud83d{bs}ude00\"}}"));
        assert_eq!(py_json_dumps(&json!({})), "{}");
    }

    #[test]
    fn obfuscation() {
        assert_eq!(obfuscate(""), "**********");
        assert_eq!(obfuscate("abc"), "**********");
        assert_eq!(obfuscate("12345678901"), "**********", "constant mask (#14784)");
    }

    #[test]
    fn env_names_filtered_and_sorted() {
        let v = externally_configured_variables(
            [
                "UNRELATED_SETTING",
                "PAPERLESS_REMOTE_OCR_API_KEY",
                "PAPERLESS_FUTURE_SETTING",
                "PAPERLESS_OCR_LANGUAGE",
                "PAPERLESS_RS_BIND",
                "PAPERLESS_",
            ]
            .into_iter()
            .map(String::from),
        );
        // unknown PAPERLESS_* listed like upstream; PaperRust's own PAPERLESS_RS_BIND not
        assert_eq!(v, vec!["PAPERLESS_FUTURE_SETTING", "PAPERLESS_OCR_LANGUAGE", "PAPERLESS_REMOTE_OCR_API_KEY"]);
    }

    /// Parity rerun 2026-11-20: the startup mirror of an unknown PAPERRUST_X is not listed.
    #[test]
    fn mirror_of_unknown_own_variable_omitted() {
        let v = externally_configured_variables(
            ["PAPERRUST_UPDATE_CHECK", "PAPERLESS_UPDATE_CHECK", "PAPERLESS_TIME_ZONE", "PAPERLESS_FUTURE_SETTING"]
                .into_iter()
                .map(String::from),
        );
        assert_eq!(v, vec!["PAPERLESS_FUTURE_SETTING", "PAPERLESS_TIME_ZONE"]);
    }

    /// Parity run 2026-11-20 F3: every name once, aliases count as the upstream name, PaperRust's
    /// own settings (also mirrored as PAPERLESS_*) are left out.
    #[test]
    fn aliases_once_and_own_settings_omitted() {
        let v = externally_configured_variables(
            [
                "PAPERRUST_OCR_LANGUAGE",
                "PAPERLESS_OCR_LANGUAGE",
                "PAPERRUST_OCR_MODE",
                "PAPERLESS_RS_TIME_ZONE",
                "PAPERRUSTX",
                "PAPERRUST_BIND",
                "PAPERLESS_BIND",
                "PAPERRUST_BRANDING",
                "PAPERLESS_ROUTES",
                "PAPERRUST_LOG",
                "PAPERLESS_DATAMODEL_BACKUP",
                "PAPERRUST_FUTURE_SETTING",
                "PAPERLESS_DBHOST",
            ]
            .into_iter()
            .map(String::from),
        );
        assert_eq!(v, vec!["PAPERLESS_DBHOST", "PAPERLESS_OCR_LANGUAGE", "PAPERLESS_OCR_MODE", "PAPERLESS_TIME_ZONE"]);
    }

    #[test]
    fn field_order() {
        let mut names: Vec<&str> = vec!["id"];
        names.extend(DECLARED);
        names.extend(MODEL_FIELDS.iter().map(|(n, _)| *n).filter(|n| !DECLARED.contains(n)));
        assert_eq!(names.len(), 1 + 1 + MODEL_FIELDS.len());
        assert_eq!(
            &names[..8],
            &[
                "id",
                "externally_configured_variables",
                "user_args",
                "barcode_tag_mapping",
                "llm_embedding_api_key",
                "llm_api_key",
                "remote_ocr_api_key",
                "output_type"
            ]
        );
    }
}
