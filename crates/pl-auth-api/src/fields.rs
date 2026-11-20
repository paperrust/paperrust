//! DRF relational fields used by `UserSerializer` / `GroupSerializer`.
//!
//! upstream: DRF 3.18.3 rest_framework/relations.py (ManyRelatedField.to_internal_value,
//! PrimaryKeyRelatedField, SlugRelatedField), src/paperless/serialisers.py@v3.4.2
//! (`Permission.objects.exclude(content_type__app_label="admin")`, slug_field="codename").

use crate::db::a;
use crate::http::{FieldInput, py_type_name};
use pl_core::ValidationErrors;
use sea_query::{Expr, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder as _;
use serde_json::Value;
use sqlx::{PgPool, Row};

/// Python `str()` of a JSON scalar as DRF puts it into messages (`smart_str` / format).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                pl_core::json::python_float_repr(f)
            } else {
                n.to_string()
            }
        }
        other => other.to_string(),
    }
}

/// `ManyRelatedField.to_internal_value` list check. `Err(msg)` = field error.
fn as_list(v: &Value) -> Result<&Vec<Value>, String> {
    match v {
        Value::Array(items) => Ok(items),
        Value::Null => Err("This field may not be null.".into()),
        other => Err(format!("Expected a list of items but got type \"{}\".", py_type_name(other))),
    }
}

/// Django `IntegerField.get_prep_value` (`int(value)`) for pk lookups.
fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::String(s) => pl_core::pagination::python_int(s).and_then(|i| i64::try_from(i).ok()),
        _ => None,
    }
}

/// `groups` (`PrimaryKeyRelatedField(many=True, queryset=Group.objects.all())`).
/// Returns `None` if an error was recorded.
pub async fn validate_group_pks(db: &PgPool, input: FieldInput, field: &str, required: bool, errs: &mut ValidationErrors) -> Result<Option<Vec<i64>>, sqlx::Error> {
    let v = match input {
        FieldInput::Missing => {
            if required {
                errs.add(field, "This field is required.");
            }
            return Ok(None);
        }
        FieldInput::Value(v) => v,
    };
    let items = match as_list(&v) {
        Ok(i) => i,
        Err(m) => {
            errs.add(field, m);
            return Ok(None);
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if matches!(item, Value::Bool(_)) {
            errs.add(field, "Incorrect type. Expected pk value, received bool.");
            return Ok(None);
        }
        if item.is_null() {
            // queryset.get(pk=None) → DoesNotExist
            errs.add(field, "Invalid pk \"None\" - object does not exist.");
            return Ok(None);
        }
        let Some(pk) = py_int(item) else {
            errs.add(field, format!("Incorrect type. Expected pk value, received {}.", py_type_name(item)));
            return Ok(None);
        };
        let exists = if (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&pk) {
            let (sql, values) = Query::select()
                .expr(Expr::val(1))
                .from(a("auth_group"))
                .and_where(Expr::col(a("id")).eq(pk))
                .build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, values).fetch_optional(db).await?.is_some()
        } else {
            false
        };
        if !exists {
            errs.add(field, format!("Invalid pk \"{}\" - object does not exist.", py_str(item)));
            return Ok(None);
        }
        out.push(pk);
    }
    Ok(Some(out))
}

/// `SlugRelatedField(many=True, slug_field="codename", queryset=Permission.objects.exclude(
/// content_type__app_label="admin"))`. Returns permission ids. `Err(String)` inside = 500
/// (MultipleObjectsReturned is not handled by DRF).
pub async fn validate_permission_codenames(
    db: &PgPool,
    input: FieldInput,
    field: &str,
    required: bool,
    errs: &mut ValidationErrors,
) -> Result<Result<Option<Vec<i64>>, String>, sqlx::Error> {
    let v = match input {
        FieldInput::Missing => {
            if required {
                errs.add(field, "This field is required.");
            }
            return Ok(Ok(None));
        }
        FieldInput::Value(v) => v,
    };
    let items = match as_list(&v) {
        Ok(i) => i,
        Err(m) => {
            errs.add(field, m);
            return Ok(Ok(None));
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let codename = match item {
            Value::String(s) => s.clone(),
            Value::Number(_) | Value::Bool(_) => py_str(item),
            Value::Null => {
                // codename=None → IS NULL → DoesNotExist
                errs.add(field, "Object with codename=None does not exist.");
                return Ok(Ok(None));
            }
            _ => {
                errs.add(field, "Invalid value.");
                return Ok(Ok(None));
            }
        };
        if codename.contains('\0') {
            errs.add(field, "Invalid value.");
            return Ok(Ok(None));
        }
        let (sql, values) = Query::select()
            .expr_as(Expr::col((a("p"), a("id"))).cast_as(a("int8")), a("id"))
            .from_as(a("auth_permission"), a("p"))
            .inner_join(
                a("django_content_type"),
                Expr::col((a("django_content_type"), a("id"))).equals((a("p"), a("content_type_id"))),
            )
            .and_where(Expr::col((a("p"), a("codename"))).eq(codename.as_str()))
            .and_where(Expr::col((a("django_content_type"), a("app_label"))).ne("admin"))
            .limit(2)
            .build_sqlx(PostgresQueryBuilder);
        let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
        match rows.len() {
            0 => {
                errs.add(field, format!("Object with codename={codename} does not exist."));
                return Ok(Ok(None));
            }
            1 => out.push(rows[0].try_get::<i64, _>("id")?),
            _ => return Ok(Err(format!("MultipleObjectsReturned for codename {codename:?}"))),
        }
    }
    Ok(Ok(Some(out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn py_strings() {
        assert_eq!(py_str(&json!(1.0)), "1.0");
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_int(&json!(" 7 ")), Some(7));
        assert_eq!(py_int(&json!("x")), None);
        assert_eq!(as_list(&json!("a")).unwrap_err(), "Expected a list of items but got type \"str\".");
    }
}
