//! ApplicationConfiguration (E3, rule AC, §2.8): the only source of its 45 fields.
//!
//! * Precedence exactly like upstream `paperless/config.py`: booleans apply if the column is
//!   **not NULL** (`x if x is not None else settings.X`); all other columns only if
//!   Python "truthy" (`x or settings.X`: not NULL, not 0/0.0, not `""`, not `{}`/`[]`).
//! * Reading/writing exactly the columns of the Django table `paperless_applicationconfiguration`
//!   (no copy in `paperrust_setting`). Create the row like `get_or_create(pk=1)`.
//! * Validation like the v1 serializer (`ApplicationConfigurationSerializer`): choices, lengths,
//!   `PositiveSmallIntegerField`/`MinValueValidator`, JSON object for `user_args`/`barcode_tag_mapping`.

use serde_json::{Map, Value};
use sqlx::PgConnection;

use crate::schema::{SCHEMA, SettingDef, Ty};

/// SQL type of an AC column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Bool,
    /// `smallint` (PositiveSmallIntegerField, ≤ 32767).
    SmallInt,
    /// `integer` (PositiveIntegerField).
    Int,
    Float,
    /// `varchar(n)`.
    Varchar(usize),
    Jsonb,
}

/// Column → (SQL type, lower bound per model validator).
pub fn column(field: &str) -> Option<(Col, Option<f64>)> {
    use Col::*;
    Some(match field {
        "output_type" => (Varchar(8), None),
        "pages" | "image_dpi" | "barcode_dpi" | "barcode_max_pages" | "llm_embedding_chunk_size" | "llm_request_timeout" => {
            (SmallInt, Some(1.0))
        }
        "llm_context_size" => (Int, Some(1.0)),
        "language"
        | "color_conversion_strategy"
        | "barcode_string"
        | "barcode_asn_prefix"
        | "remote_ocr_engine"
        | "remote_ocr_mode"
        | "llm_output_language" => (Varchar(32), None),
        "mode" | "unpaper_clean" => (Varchar(16), None),
        "archive_file_generation" => (Varchar(8), None),
        "deskew"
        | "rotate_pages"
        | "barcodes_enabled"
        | "barcode_enable_tiff_support"
        | "barcode_retain_split_pages"
        | "barcode_enable_asn"
        | "barcode_enable_tag"
        | "barcode_tag_split"
        | "barcode_store_values"
        | "ai_enabled" => (Bool, None),
        "rotate_pages_threshold" | "max_image_pixels" => (Float, Some(0.0)),
        "barcode_upscale" => (Float, Some(1.0)),
        "user_args" | "barcode_tag_mapping" => (Jsonb, None),
        "app_title" => (Varchar(48), None),
        "app_logo" => (Varchar(100), None),
        "remote_ocr_api_key" | "llm_api_key" | "llm_embedding_api_key" => (Varchar(1024), None),
        "remote_ocr_endpoint" | "llm_endpoint" | "llm_embedding_endpoint" => (Varchar(256), None),
        "llm_backend" | "llm_embedding_backend" | "llm_model" | "llm_embedding_model" => (Varchar(128), None),
        _ => return None,
    })
}

/// All AC entries of the schema.
pub fn defs() -> impl Iterator<Item = &'static SettingDef> {
    SCHEMA.iter().filter(|d| d.ac_field.is_some())
}

/// Python truth value of a column value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Does the column apply (instead of environment/default)? Bool: `is not None`, otherwise "truthy".
pub fn column_wins(def: &SettingDef, v: &Value) -> bool {
    match def.ty {
        Ty::Bool => !v.is_null(),
        _ => truthy(v),
    }
}

/// Column value → canonical env string (for display and the shared snapshot).
pub fn to_env_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Bool(b) => Some(if *b { "true" } else { "false" }.into()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// One row of the table as `column → JSON` (NULL = `Value::Null`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AcRow {
    pub id: Option<i64>,
    pub values: Map<String, Value>,
}

impl AcRow {
    pub fn get(&self, field: &str) -> &Value {
        self.values.get(field).unwrap_or(&Value::Null)
    }
}

/// Result of the precedence rule for an AC field.
#[derive(Debug, Clone, PartialEq)]
pub struct AcResolved {
    /// Effective value (canonical env string) or `None`.
    pub value: Option<String>,
    /// Does the value come from the column?
    pub from_column: bool,
    /// Raw value of the column (even if it does not apply because of "falsy").
    pub column: Value,
}

/// `app_config.x or settings.X` or `x if x is not None else settings.X`. `env_value` is the
/// value from environment/file (already `Env::get` semantics), `default` the default.
pub fn resolve(def: &SettingDef, column: &Value, env_value: Option<&str>, default: Option<&str>) -> AcResolved {
    if column_wins(def, column) {
        return AcResolved { value: to_env_string(column), from_column: true, column: column.clone() };
    }
    AcResolved { value: env_value.or(default).map(String::from), from_column: false, column: column.clone() }
}

/// Typed API value → column value, checked like the v1 serializer. `Value::Null` sets the
/// column to NULL (= "reset", afterwards the environment applies).
pub fn validate(def: &SettingDef, v: &Value) -> Result<Value, crate::value::ValueError> {
    let field = def.ac_field.unwrap_or_default();
    let e = |m: String| crate::value::ValueError { key: def.key.to_string(), message: m };
    if v.is_null() {
        return Ok(Value::Null);
    }
    let (col, min) = column(field).ok_or_else(|| e(format!("unknown column {field}")))?;
    let out = match col {
        Col::Bool => Value::Bool(v.as_bool().ok_or_else(|| e("Must be a valid boolean.".into()))?),
        Col::SmallInt | Col::Int => {
            let i = v.as_i64().ok_or_else(|| e("A valid integer is required.".into()))?;
            let max = if col == Col::SmallInt { 32767 } else { 2_147_483_647 };
            if let Some(m) = min.filter(|m| (i as f64) < *m) {
                return Err(e(format!("Ensure this value is greater than or equal to {m}.")));
            }
            if i > max {
                return Err(e(format!("Ensure this value is less than or equal to {max}.")));
            }
            Value::from(i)
        }
        Col::Float => {
            let f = v.as_f64().ok_or_else(|| e("A valid number is required.".into()))?;
            if let Some(m) = min.filter(|m| f < *m) {
                return Err(e(format!("Ensure this value is greater than or equal to {m}.")));
            }
            v.clone()
        }
        Col::Varchar(n) => {
            let s = v.as_str().ok_or_else(|| e("Not a valid string.".into()))?;
            if s.chars().count() > n {
                return Err(e(format!("Ensure this field has no more than {n} characters.")));
            }
            if let Ty::Choice { choices } = def.ty
                && !s.is_empty()
                && !choices.contains(&s)
            {
                return Err(e(format!("\"{s}\" is not a valid choice.")));
            }
            Value::String(s.to_string())
        }
        Col::Jsonb => {
            if !v.is_object() {
                return Err(e("must be a JSON object".into()));
            }
            v.clone()
        }
    };
    Ok(out)
}

/// Reads the (first) row like `ApplicationConfiguration.objects.all().first()`; `None` if
/// there is no row or no table.
pub async fn load(conn: &mut PgConnection) -> Result<Option<AcRow>, sqlx::Error> {
    let has: bool =
        sqlx::query_scalar("SELECT to_regclass('public.paperless_applicationconfiguration') IS NOT NULL").fetch_one(&mut *conn).await?;
    if !has {
        return Ok(None);
    }
    let row: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(c) FROM paperless_applicationconfiguration c ORDER BY c.id LIMIT 1")
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.map(|v| {
        let mut values = match v {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        let id = values.remove("id").and_then(|v| v.as_i64());
        AcRow { id, values }
    }))
}

/// Sets columns (already checked with [`validate`]) in the same transaction; creates the row like
/// `get_or_create(pk=1)` if it is missing. Returns the old values (for the history).
pub async fn write(conn: &mut PgConnection, changes: &[(&'static SettingDef, Value)]) -> Result<Vec<Value>, sqlx::Error> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id::bigint FROM paperless_applicationconfiguration ORDER BY id LIMIT 1 FOR UPDATE")
        .fetch_optional(&mut *conn)
        .await?;
    let id = match id {
        Some(id) => id,
        None => {
            sqlx::query("INSERT INTO paperless_applicationconfiguration (id) VALUES (1) ON CONFLICT (id) DO NOTHING")
                .execute(&mut *conn)
                .await?;
            1
        }
    };
    let mut old = Vec::with_capacity(changes.len());
    for (def, v) in changes {
        let field = def.ac_field.unwrap_or_default();
        let Some((col, _)) = column(field) else {
            continue;
        };
        let before: Option<Value> =
            sqlx::query_scalar(&format!("SELECT to_jsonb({field}) FROM paperless_applicationconfiguration WHERE id = $1"))
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
                .flatten();
        old.push(before.unwrap_or(Value::Null));
        let cast = match col {
            Col::Bool => "boolean",
            Col::SmallInt => "smallint",
            Col::Int => "integer",
            Col::Float => "double precision",
            Col::Varchar(_) => "text",
            Col::Jsonb => "jsonb",
        };
        // `field` comes from the static schema (no user input).
        let sql = if col == Col::Jsonb {
            format!("UPDATE paperless_applicationconfiguration SET {field} = $1::jsonb WHERE id = $2")
        } else {
            format!("UPDATE paperless_applicationconfiguration SET {field} = ($1::jsonb #>> '{{}}')::{cast} WHERE id = $2")
        };
        let bind: Option<Value> = if v.is_null() { None } else { Some(v.clone()) };
        sqlx::query(&sql).bind(bind).bind(id).execute(&mut *conn).await?;
    }
    Ok(old)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::find_ac;
    use serde_json::json;

    /// `appconfig_or_semantics` (WP1): bool = `is not None`, otherwise Python `or`.
    #[test]
    fn appconfig_or_semantics() {
        let deskew = find_ac("deskew").unwrap();
        // false in the column applies (not NULL) – overrides environment `true`.
        let r = resolve(deskew, &json!(false), Some("true"), Some("true"));
        assert_eq!((r.value.as_deref(), r.from_column), (Some("false"), true));
        let r = resolve(deskew, &Value::Null, Some("no"), Some("true"));
        assert_eq!((r.value.as_deref(), r.from_column), (Some("no"), false));

        let pages = find_ac("pages").unwrap();
        // 0 is falsy → environment
        let r = resolve(pages, &json!(0), Some("3"), Some("0"));
        assert_eq!((r.value.as_deref(), r.from_column), (Some("3"), false));
        let r = resolve(pages, &json!(5), Some("3"), Some("0"));
        assert_eq!(r.value.as_deref(), Some("5"));

        let lang = find_ac("language").unwrap();
        assert_eq!(resolve(lang, &json!(""), Some("deu"), Some("eng")).value.as_deref(), Some("deu"));
        assert_eq!(resolve(lang, &json!(""), None, Some("eng")).value.as_deref(), Some("eng"));
        assert_eq!(resolve(lang, &json!("deu+eng"), Some("deu"), Some("eng")).value.as_deref(), Some("deu+eng"));

        let up = find_ac("barcode_upscale").unwrap();
        assert!(!resolve(up, &json!(0.0), None, Some("0.0")).from_column);
        let map = find_ac("barcode_tag_mapping").unwrap();
        assert!(!resolve(map, &json!({}), None, None).from_column);
        assert!(resolve(map, &json!({"a": "b"}), None, None).from_column);
        assert_eq!(resolve(map, &json!({"a": "b"}), None, None).value.as_deref(), Some(r#"{"a":"b"}"#));
    }

    #[test]
    fn every_ac_field_has_a_column() {
        let mut n = 0;
        for d in defs() {
            assert!(column(d.ac_field.unwrap()).is_some(), "{}", d.key);
            n += 1;
        }
        assert_eq!(n, 45);
    }

    #[test]
    fn serializer_like_validation() {
        let pages = find_ac("pages").unwrap();
        assert!(validate(pages, &json!(0)).is_err());
        assert!(validate(pages, &json!(40000)).is_err());
        assert_eq!(validate(pages, &json!(2)).unwrap(), json!(2));
        assert_eq!(validate(pages, &Value::Null).unwrap(), Value::Null);
        let mode = find_ac("mode").unwrap();
        assert!(validate(mode, &json!("bogus")).is_err() && validate(mode, &json!("redo")).is_ok());
        let title = find_ac("app_title").unwrap();
        assert!(validate(title, &json!("x".repeat(49))).is_err());
        let ua = find_ac("user_args").unwrap();
        assert!(validate(ua, &json!([1])).is_err() && validate(ua, &json!({"a": 1})).is_ok());
        let up = find_ac("barcode_upscale").unwrap();
        assert!(validate(up, &json!(0.5)).is_err());
    }
}
