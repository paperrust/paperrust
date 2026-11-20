//! Re-implementation of the required DRF field validation (djangorestframework 3.18.3, `rest_framework/fields.py`,
//! `rest_framework/relations.py`, `rest_framework/serializers.py::Serializer.to_internal_value`).
//! Error texts 1:1 from DRF.

use crate::pyfmt::{self, PyInt};
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

/// Validation error.
#[derive(Debug)]
pub enum VErr {
    /// DRF `ValidationError.detail` (list or object).
    Detail(Value),
    /// Unhandled error (upstream: 500).
    Server(String),
}

impl From<sqlx::Error> for VErr {
    fn from(e: sqlx::Error) -> Self {
        VErr::Server(format!("db: {e}"))
    }
}

pub fn msg(m: impl Into<String>) -> Value {
    Value::Array(vec![Value::String(m.into())])
}

pub fn non_field(m: impl Into<String>) -> VErr {
    VErr::Detail(json!({"non_field_errors": [m.into()]}))
}

/// Choices of a `ChoiceField`: (`str(key)`, internal value).
#[derive(Debug, Clone, Copy)]
pub enum Choices {
    Ints(&'static [i64]),
    Strs(&'static [&'static str]),
}

impl Choices {
    /// `choice_strings_to_values[str(data)]`
    pub fn lookup(&self, key: &str) -> Option<Value> {
        match self {
            Choices::Ints(v) => v.iter().find(|i| i.to_string() == key).map(|i| json!(i)),
            Choices::Strs(v) => v.iter().find(|s| **s == key).map(|s| json!(s)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Int { min: Option<i64>, max: Option<i64> },
    Bool,
    Char { max_len: Option<usize>, allow_blank: bool },
    Choice(Choices),
    MultiChoice(Choices),
    /// `PrimaryKeyRelatedField` on a table (column `id`, AutoField int4)
    Pk(&'static str),
    /// `ManyRelatedField(PrimaryKeyRelatedField)` on a table
    ManyPk(&'static str),
    Json,
}

#[derive(Debug, Clone, Copy)]
pub struct FieldSpec {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
    pub allow_null: bool,
    /// Serializer default (non-partial only, is not validated).
    pub default: Option<fn() -> Value>,
}

impl FieldSpec {
    pub const fn opt(name: &'static str, kind: Kind) -> Self {
        FieldSpec { name, kind, required: false, allow_null: false, default: None }
    }
    pub const fn null(name: &'static str, kind: Kind) -> Self {
        FieldSpec { name, kind, required: false, allow_null: true, default: None }
    }
    pub const fn req(name: &'static str, kind: Kind) -> Self {
        FieldSpec { name, kind, required: true, allow_null: false, default: None }
    }
}

pub enum Out {
    Skip,
    Val(Value),
}

#[derive(Clone, Copy)]
pub struct Ctx {
    pub partial: bool,
}

const INT4_MIN: i64 = -2_147_483_648;
const INT4_MAX: i64 = 2_147_483_647;

/// `Field.run_validation` incl. `validate_empty_values` and validators.
pub async fn run_field(db: &mut PgConnection, ctx: Ctx, spec: &FieldSpec, input: Option<&Value>) -> Result<Out, VErr> {
    let mut data = input.cloned();
    // CharField.run_validation: empty string before everything else
    if let (Kind::Char { allow_blank, .. }, Some(d)) = (&spec.kind, &data) {
        let blank = match d {
            Value::String(s) => pyfmt::py_strip(s).is_empty(),
            Value::Null => false,
            other => pyfmt::py_strip(&pyfmt::py_str(other)).is_empty(),
        };
        if blank {
            if !allow_blank {
                return Err(VErr::Detail(msg("This field may not be blank.")));
            }
            return Ok(Out::Val(json!("")));
        }
    }
    // RelatedField.run_validation: "" → None
    if let (Kind::Pk(_), Some(Value::String(s))) = (&spec.kind, &data)
        && s.is_empty() {
            data = Some(Value::Null);
        }
    let Some(data) = data else {
        if ctx.partial {
            return Ok(Out::Skip);
        }
        if spec.required {
            return Err(VErr::Detail(msg("This field is required.")));
        }
        return Ok(match spec.default {
            Some(f) => Out::Val(f()),
            None => Out::Skip,
        });
    };
    if data.is_null() {
        if !spec.allow_null {
            return Err(VErr::Detail(msg("This field may not be null.")));
        }
        return Ok(Out::Val(Value::Null));
    }
    to_internal(db, spec.kind, &data).await.map(Out::Val)
}

async fn to_internal(db: &mut PgConnection, kind: Kind, data: &Value) -> Result<Value, VErr> {
    match kind {
        Kind::Int { min, max } => int_field(data, min, max),
        Kind::Bool => bool_field(data),
        Kind::Char { max_len, .. } => char_field(data, max_len),
        Kind::Choice(c) => choice_field(c, data),
        Kind::MultiChoice(c) => {
            let items = iter_items(data).ok_or_else(|| {
                VErr::Detail(msg(format!(
                    "Expected a list of items but got type \"{}\".",
                    pyfmt::type_name(data)
                )))
            })?;
            let mut out: Vec<Value> = Vec::new();
            for it in items {
                let v = choice_field(c, &it)?;
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            Ok(Value::Array(out))
        }
        Kind::Pk(table) => pk_lookup(db, table, data).await,
        Kind::ManyPk(table) => {
            let items = iter_items(data).ok_or_else(|| {
                VErr::Detail(msg(format!(
                    "Expected a list of items but got type \"{}\".",
                    pyfmt::type_name(data)
                )))
            })?;
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(pk_lookup(db, table, &it).await?);
            }
            Ok(Value::Array(out))
        }
        Kind::Json => Ok(data.clone()),
    }
}

/// Python iteration over a value: list → elements, dict → keys; str/scalar → None.
fn iter_items(data: &Value) -> Option<Vec<Value>> {
    match data {
        Value::Array(a) => Some(a.clone()),
        Value::Object(o) => Some(o.keys().map(|k| Value::String(k.clone())).collect()),
        _ => None,
    }
}

/// `IntegerField.to_internal_value` + min/max validators.
pub fn int_field(data: &Value, min: Option<i64>, max: Option<i64>) -> Result<Value, VErr> {
    if let Value::String(s) = data
        && s.chars().count() > 1000 {
            return Err(VErr::Detail(msg("String value too large.")));
        }
    let s = pyfmt::py_str(data);
    let s = strip_re_decimal(&s);
    let parsed = match data {
        Value::Bool(_) | Value::Array(_) | Value::Object(_) | Value::Null => PyInt::Invalid,
        _ => pyfmt::parse_int(&s),
    };
    let n: i128 = match parsed {
        PyInt::Invalid => return Err(VErr::Detail(msg("A valid integer is required."))),
        PyInt::Ok(n) => n as i128,
        PyInt::Overflow(neg) => {
            if neg {
                i128::MIN
            } else {
                i128::MAX
            }
        }
    };
    if let Some(mx) = max
        && n > mx as i128 {
            return Err(VErr::Detail(msg(format!("Ensure this value is less than or equal to {mx}."))));
        }
    if let Some(mn) = min
        && n < mn as i128 {
            return Err(VErr::Detail(msg(format!(
                "Ensure this value is greater than or equal to {mn}."
            ))));
        }
    if n > i64::MAX as i128 || n < i64::MIN as i128 {
        // upstream: DB error (integer out of range) → 500
        return Err(VErr::Server("integer out of range".into()));
    }
    Ok(json!(n as i64))
}

/// `re.sub(r'\.0*\s*$', '', s)`
fn strip_re_decimal(s: &str) -> String {
    if let Some(pos) = s.rfind('.') {
        let rest = &s[pos + 1..];
        let after_zeros = rest.trim_start_matches('0');
        if after_zeros.chars().all(char::is_whitespace) {
            return s[..pos].to_string();
        }
    }
    s.to_string()
}

/// `BooleanField.to_internal_value`
pub fn bool_field(data: &Value) -> Result<Value, VErr> {
    let r = match data {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => {
            if n.as_i64() == Some(1) || n.as_f64() == Some(1.0) && n.is_f64() {
                Some(true)
            } else if n.as_i64() == Some(0) || n.is_f64() && n.as_f64() == Some(0.0) {
                Some(false)
            } else {
                None
            }
        }
        Value::String(s) => match s.to_lowercase().as_str() {
            "t" | "y" | "yes" | "true" | "on" | "1" => Some(true),
            "f" | "n" | "no" | "false" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    };
    r.map(Value::Bool).ok_or_else(|| VErr::Detail(msg("Must be a valid boolean.")))
}

/// `CharField.to_internal_value` + validators (max_length, ProhibitNullCharacters).
pub fn char_field(data: &Value, max_len: Option<usize>) -> Result<Value, VErr> {
    let s = match data {
        Value::String(s) => s.clone(),
        Value::Number(_) => pyfmt::py_str(data),
        _ => return Err(VErr::Detail(msg("Not a valid string."))),
    };
    let s = pyfmt::py_strip(&s).to_string();
    let mut errs = Vec::new();
    if let Some(m) = max_len
        && s.chars().count() > m {
            errs.push(json!(format!("Ensure this field has no more than {m} characters.")));
        }
    if s.contains('\0') {
        errs.push(json!("Null characters are not allowed."));
    }
    if !errs.is_empty() {
        return Err(VErr::Detail(Value::Array(errs)));
    }
    Ok(Value::String(s))
}

/// `ChoiceField.to_internal_value`
pub fn choice_field(c: Choices, data: &Value) -> Result<Value, VErr> {
    let key = pyfmt::py_str(data);
    c.lookup(&key).ok_or_else(|| {
        VErr::Detail(msg(format!("\"{}\" is not a valid choice.", key)))
    })
}

/// `PrimaryKeyRelatedField.to_internal_value` (queryset `<table>.objects.all()`).
pub async fn pk_lookup(db: &mut PgConnection, table: &'static str, data: &Value) -> Result<Value, VErr> {
    let incorrect = || {
        VErr::Detail(msg(format!(
            "Incorrect type. Expected pk value, received {}.",
            pyfmt::type_name(data)
        )))
    };
    if data.is_boolean() {
        return Err(incorrect());
    }
    let n = match pyfmt::int_of_value(data) {
        Err(pyfmt::PyTypeError) => return Err(incorrect()),
        Ok(PyInt::Invalid) => return Err(incorrect()),
        Ok(PyInt::Overflow(_)) => None,
        Ok(PyInt::Ok(n)) if !(INT4_MIN..=INT4_MAX).contains(&n) => None,
        Ok(PyInt::Ok(n)) => Some(n),
    };
    let exists = match n {
        None => false,
        Some(id) => row_exists(db, table, id).await?,
    };
    if !exists {
        return Err(VErr::Detail(msg(format!(
            "Invalid pk \"{}\" - object does not exist.",
            pyfmt::py_str(data)
        ))));
    }
    Ok(json!(n))
}

pub async fn row_exists(db: &mut PgConnection, table: &'static str, id: i64) -> Result<bool, sqlx::Error> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(Alias::new(table))
        .and_where(Expr::col(Alias::new("id")).eq(id))
        .limit(1)
        .build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(&mut *db).await?;
    Ok(row.is_some())
}

/// `Serializer.to_internal_value`: fields in order, errors collected.
pub async fn validate_fields(
    db: &mut PgConnection, ctx: Ctx,
    specs: &[FieldSpec],
    data: &Map<String, Value>,
) -> Result<Map<String, Value>, VErr> {
    let mut ret = Map::new();
    let mut errors = Map::new();
    for spec in specs {
        match run_field(db, ctx, spec, data.get(spec.name)).await {
            Ok(Out::Val(v)) => {
                ret.insert(spec.name.to_string(), v);
            }
            Ok(Out::Skip) => {}
            Err(VErr::Detail(d)) => {
                errors.insert(spec.name.to_string(), d);
            }
            Err(e) => return Err(e),
        }
    }
    if !errors.is_empty() {
        return Err(VErr::Detail(Value::Object(errors)));
    }
    Ok(ret)
}

/// Checks that the body is an object (`Serializer.to_internal_value`).
pub fn expect_object(data: &Value) -> Result<&Map<String, Value>, VErr> {
    match data {
        Value::Object(o) => Ok(o),
        other => Err(non_field(format!(
            "Invalid data. Expected a dictionary, but got {}.",
            pyfmt::type_name(other)
        ))),
    }
}

/// `as_serializer_error` for errors from `validate()`.
pub fn as_serializer_error(detail: Value) -> Value {
    match detail {
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, v)| {
                    let v = if v.is_array() || v.is_object() { v } else { Value::Array(vec![v]) };
                    (k, v)
                })
                .collect(),
        ),
        Value::Array(a) => json!({"non_field_errors": a}),
        other => json!({"non_field_errors": [other]}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(e: VErr) -> Value {
        match e {
            VErr::Detail(v) => v,
            VErr::Server(s) => panic!("{s}"),
        }
    }

    #[test]
    fn ints() {
        assert_eq!(int_field(&json!(5), None, None).unwrap(), json!(5));
        assert_eq!(int_field(&json!("5.000 "), None, None).unwrap(), json!(5));
        assert_eq!(int_field(&json!(5.0), None, None).unwrap(), json!(5));
        assert_eq!(d(int_field(&json!(5.5), None, None).unwrap_err()), json!(["A valid integer is required."]));
        assert_eq!(d(int_field(&json!(true), None, None).unwrap_err()), json!(["A valid integer is required."]));
        assert_eq!(
            d(int_field(&json!(0), Some(1), Some(10)).unwrap_err()),
            json!(["Ensure this value is greater than or equal to 1."])
        );
        assert_eq!(
            d(int_field(&json!("99999999999999999999999"), Some(1), Some(10)).unwrap_err()),
            json!(["Ensure this value is less than or equal to 10."])
        );
    }

    #[test]
    fn bools() {
        assert_eq!(bool_field(&json!("YES")).unwrap(), json!(true));
        assert_eq!(bool_field(&json!(0.0)).unwrap(), json!(false));
        assert_eq!(bool_field(&json!(1)).unwrap(), json!(true));
        assert!(bool_field(&json!(2)).is_err());
        assert!(bool_field(&json!([])).is_err());
    }

    #[test]
    fn chars_and_choices() {
        assert_eq!(char_field(&json!("  a "), Some(3)).unwrap(), json!("a"));
        assert_eq!(char_field(&json!(1.5), None).unwrap(), json!("1.5"));
        assert_eq!(d(char_field(&json!(true), None).unwrap_err()), json!(["Not a valid string."]));
        assert_eq!(
            d(char_field(&json!("abcd"), Some(3)).unwrap_err()),
            json!(["Ensure this field has no more than 3 characters."])
        );
        let c = Choices::Ints(&[1, 2]);
        assert_eq!(choice_field(c, &json!("2")).unwrap(), json!(2));
        assert_eq!(d(choice_field(c, &json!(1.0)).unwrap_err()), json!(["\"1.0\" is not a valid choice."]));
        assert_eq!(d(choice_field(c, &json!(null)).unwrap_err()), json!(["\"None\" is not a valid choice."]));
    }
}
