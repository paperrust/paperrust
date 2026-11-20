//! DRF field validation (subset), identical error texts.
//!
//! upstream: rest_framework/fields.py, relations.py (DRF 3.16) – CharField, ChoiceField,
//! BooleanField, IntegerField, JSONField, ListField, PrimaryKeyRelatedField, DateTimeField.

use serde_json::{Map, Value, json};

use crate::shim::{ApiError, parse_dt, py_type_name};

/// Result of a field validation.
#[derive(Debug, Clone, PartialEq)]
pub enum F<T> {
    /// Field not in the request (and not required / partial)
    Absent,
    Val(T),
    /// Error was entered into `Errors`
    Err,
}

impl<T> F<T> {
    pub fn is_present(&self) -> bool {
        matches!(self, F::Val(_))
    }
    pub fn val(&self) -> Option<&T> {
        match self {
            F::Val(v) => Some(v),
            _ => None,
        }
    }
}

/// Errors in field order (DRF `OrderedDict`).
#[derive(Default, Debug)]
pub struct Errors(pub Map<String, Value>);

impl Errors {
    pub fn add(&mut self, field: &str, msg: &str) {
        self.0.insert(field.to_string(), json!([msg]));
    }
    pub fn set(&mut self, field: &str, v: Value) {
        self.0.insert(field.to_string(), v);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn into_result(self) -> Result<(), ApiError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ApiError::bad_request(Value::Object(self.0)))
        }
    }
}

pub const REQUIRED: &str = "This field is required.";
pub const NULL: &str = "This field may not be null.";
pub const BLANK: &str = "This field may not be blank.";

/// Python `str()` of a value parsed from JSON (for `"{input}" is not a valid choice.`).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// Python `repr()` of a value parsed from JSON.
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                py_float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => {
            // Python prefers single quotes
            if s.contains('\'') && !s.contains('"') {
                format!("\"{s}\"")
            } else {
                format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
            }
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_repr(&Value::String(k.clone())), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python `repr(float)` (shortest representation, always with `.0` or exponent).
pub fn py_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        // Exponential notation like Python: 1e+16, 1.5e-05
        let s = format!("{f:e}");
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{mant}e{sign}{:02}", exp.abs());
    }
    let s = format!("{f}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

/// Get the value from the body (`None` = not present).
pub fn get<'a>(data: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    data.get(name)
}

/// Shared part of `Field.validate_empty_values`.
/// Returns `Some(F)` if the result is already determined.
fn empty_values<T>(
    v: Option<&Value>,
    name: &str,
    required: bool,
    partial: bool,
    allow_null: bool,
    errs: &mut Errors,
) -> Option<F<Option<T>>> {
    match v {
        None => {
            if partial || !required {
                Some(F::Absent)
            } else {
                errs.add(name, REQUIRED);
                Some(F::Err)
            }
        }
        Some(Value::Null) => {
            if allow_null {
                Some(F::Val(None))
            } else {
                errs.add(name, NULL);
                Some(F::Err)
            }
        }
        Some(_) => None,
    }
}

#[derive(Clone, Copy)]
pub struct CharOpts {
    pub required: bool,
    pub allow_null: bool,
    pub allow_blank: bool,
    pub max_length: Option<usize>,
}

/// DRF `CharField` (trim_whitespace=True). `Val(None)` = null.
pub fn char_field(
    data: &Map<String, Value>,
    name: &str,
    o: CharOpts,
    partial: bool,
    errs: &mut Errors,
) -> F<Option<String>> {
    let v = get(data, name);
    if let Some(r) = empty_values(v, name, o.required, partial, o.allow_null, errs) {
        return r;
    }
    let Some(v) = v else { return F::Absent };
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                py_float(n.as_f64().unwrap_or(0.0))
            }
        }
        _ => {
            errs.add(name, "Not a valid string.");
            return F::Err;
        }
    };
    let trimmed = s.trim().to_string();
    if trimmed.is_empty() {
        if !o.allow_blank {
            errs.add(name, BLANK);
            return F::Err;
        }
        return F::Val(Some(String::new()));
    }
    if let Some(max) = o.max_length
        && trimmed.chars().count() > max {
            errs.add(
                name,
                &format!("Ensure this field has no more than {max} characters."),
            );
            return F::Err;
        }
    F::Val(Some(trimmed))
}

#[derive(Clone, Copy)]
pub struct ChoiceOpts {
    pub required: bool,
    pub allow_null: bool,
    pub allow_blank: bool,
}

/// DRF `ChoiceField` with string choices.
pub fn choice_str(
    data: &Map<String, Value>,
    name: &str,
    choices: &[&str],
    o: ChoiceOpts,
    partial: bool,
    errs: &mut Errors,
) -> F<Option<String>> {
    let v = get(data, name);
    if let Some(r) = empty_values(v, name, o.required, partial, o.allow_null, errs) {
        return r;
    }
    let Some(v) = v else { return F::Absent };
    if v.as_str() == Some("") && o.allow_blank {
        return F::Val(Some(String::new()));
    }
    let key = py_str(v);
    if choices.contains(&key.as_str()) {
        F::Val(Some(key))
    } else {
        errs.add(name, &format!("\"{key}\" is not a valid choice."));
        F::Err
    }
}

/// `ChoiceField` with integer choices (key comparison via `str()`).
pub fn choice_int(v: &Value, choices: std::ops::RangeInclusive<i64>) -> Result<i64, String> {
    let key = py_str(v);
    match key.parse::<i64>() {
        Ok(i) if choices.contains(&i) && i.to_string() == key => Ok(i),
        _ => Err(format!("\"{key}\" is not a valid choice.")),
    }
}

/// DRF `BooleanField` (allow_null=False).
pub fn bool_field(
    data: &Map<String, Value>,
    name: &str,
    partial: bool,
    errs: &mut Errors,
) -> F<bool> {
    let v = get(data, name);
    match empty_values::<bool>(v, name, false, partial, false, errs) {
        Some(F::Absent) => return F::Absent,
        Some(_) => return F::Err,
        None => {}
    }
    let Some(v) = v else { return F::Absent };
    match parse_bool(v) {
        Some(b) => F::Val(b),
        None => {
            errs.add(name, "Must be a valid boolean.");
            F::Err
        }
    }
}

/// `BooleanField.TRUE_VALUES` / `FALSE_VALUES`
pub fn parse_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => {
            let f = n.as_f64()?;
            if f == 1.0 {
                Some(true)
            } else if f == 0.0 {
                Some(false)
            } else {
                None
            }
        }
        // DRF 3.18.3 `BooleanField._lower_if_str`: lowercased before the set lookup
        Value::String(s) => match s.to_lowercase().as_str() {
            "t" | "T" | "y" | "Y" | "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on"
            | "On" | "ON" | "1" => Some(true),
            "f" | "F" | "n" | "N" | "no" | "No" | "NO" | "false" | "False" | "FALSE"
            | "off" | "Off" | "OFF" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// DRF `IntegerField.to_internal_value` (without bounds).
pub fn parse_int(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(_) => None,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else {
                let f = n.as_f64()?;
                // str(5.0) = "5.0" → re_decimal removes ".0"
                if f.fract() == 0.0 && f.abs() < 1e16 {
                    Some(f as i64)
                } else {
                    None
                }
            }
        }
        Value::String(s) => {
            static RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
            let re = RE
                .get_or_init(|| regex::Regex::new(r"\.0*\s*$").ok())
                .as_ref()?;
            let cleaned = re.replace(s, "").to_string();
            cleaned.trim().parse::<i64>().ok()
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub struct IntOpts {
    pub required: bool,
    pub allow_null: bool,
    pub min: Option<i64>,
    pub max: Option<i64>,
}

pub fn int_field(
    data: &Map<String, Value>,
    name: &str,
    o: IntOpts,
    partial: bool,
    errs: &mut Errors,
) -> F<Option<i64>> {
    let v = get(data, name);
    if let Some(r) = empty_values(v, name, o.required, partial, o.allow_null, errs) {
        return r;
    }
    let Some(v) = v else { return F::Absent };
    match check_int(v, o.min, o.max) {
        Ok(i) => F::Val(Some(i)),
        Err(m) => {
            errs.add(name, &m);
            F::Err
        }
    }
}

pub fn check_int(v: &Value, min: Option<i64>, max: Option<i64>) -> Result<i64, String> {
    let i = parse_int(v).ok_or_else(|| "A valid integer is required.".to_string())?;
    if let Some(m) = max
        && i > m {
            return Err(format!("Ensure this value is less than or equal to {m}."));
        }
    if let Some(m) = min
        && i < m {
            return Err(format!("Ensure this value is greater than or equal to {m}."));
        }
    Ok(i)
}

/// DRF `JSONField` (every JSON value is valid).
pub fn json_field(
    data: &Map<String, Value>,
    name: &str,
    allow_null: bool,
    partial: bool,
    errs: &mut Errors,
) -> F<Value> {
    let v = get(data, name);
    match v {
        None => F::Absent,
        Some(Value::Null) if !allow_null => {
            errs.add(name, NULL);
            F::Err
        }
        Some(v) => {
            let _ = partial;
            F::Val(v.clone())
        }
    }
}

/// Pre-check the PK input of a `PrimaryKeyRelatedField`.
/// `Ok(pk)` → still check for existence; `Err(msg)` → field error.
pub fn pk_input(v: &Value) -> Result<i64, String> {
    let incorrect = |v: &Value| {
        format!(
            "Incorrect type. Expected pk value, received {}.",
            py_type_name(v)
        )
    };
    match v {
        Value::Bool(_) => Err(incorrect(v)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else {
                // Django IntegerField.get_prep_value: int(5.5) == 5
                n.as_f64().map(|f| f.trunc() as i64).ok_or_else(|| incorrect(v))
            }
        }
        Value::String(s) => s.trim().parse::<i64>().map_err(|_| incorrect(v)),
        _ => Err(incorrect(v)),
    }
}

pub fn pk_missing_msg(v: &Value) -> String {
    format!("Invalid pk \"{}\" - object does not exist.", py_str(v))
}

/// DRF `DateTimeField` (allow_null=True, required=False).
pub fn datetime_field(
    data: &Map<String, Value>,
    name: &str,
    partial: bool,
    errs: &mut Errors,
) -> F<Option<chrono::DateTime<chrono::Utc>>> {
    let v = get(data, name);
    if let Some(r) = empty_values(v, name, false, partial, true, errs) {
        return r;
    }
    let Some(v) = v else { return F::Absent };
    match v {
        // DRF: empty string with allow_null → None
        Value::String(s) if s.is_empty() => F::Val(None),
        Value::String(s) => match parse_dt(s) {
            Ok(d) => F::Val(Some(d)),
            Err(msg) => {
                errs.add(name, &msg);
                F::Err
            }
        },
        _ => {
            errs.add(name, pl_core::datetime::DRF_DATETIME_INVALID);
            F::Err
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 1 N4 (checked against DRF 3.18.3 of the reference): `BooleanField` compares lower-case,
    /// `"tRuE"`/`"yES"` are valid, unknown values are not.
    #[test]
    fn drf_bool_is_case_insensitive_like_drf_3_18() {
        use serde_json::json;
        assert_eq!(parse_bool(&json!("tRuE")), Some(true));
        assert_eq!(parse_bool(&json!("nO")), Some(false));
        assert_eq!(parse_bool(&json!("maybe")), None);
    }

    fn m(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn char_rules() {
        let o = CharOpts {
            required: true,
            allow_null: false,
            allow_blank: false,
            max_length: Some(3),
        };
        let mut e = Errors::default();
        assert_eq!(char_field(&m(json!({})), "n", o, false, &mut e), F::Err);
        assert_eq!(e.0["n"], json!(["This field is required."]));
        let mut e = Errors::default();
        assert_eq!(char_field(&m(json!({"n": " "})), "n", o, false, &mut e), F::Err);
        assert_eq!(e.0["n"], json!(["This field may not be blank."]));
        let mut e = Errors::default();
        char_field(&m(json!({"n": "abcd"})), "n", o, false, &mut e);
        assert_eq!(e.0["n"], json!(["Ensure this field has no more than 3 characters."]));
        let mut e = Errors::default();
        assert_eq!(
            char_field(&m(json!({"n": 12})), "n", o, false, &mut e),
            F::Val(Some("12".into()))
        );
        assert_eq!(char_field(&m(json!({})), "n", o, true, &mut e), F::Absent);
    }

    #[test]
    fn choice_and_bool() {
        let o = ChoiceOpts {
            required: false,
            allow_null: false,
            allow_blank: false,
        };
        let mut e = Errors::default();
        choice_str(&m(json!({"c": "x"})), "c", &["a"], o, false, &mut e);
        assert_eq!(e.0["c"], json!(["\"x\" is not a valid choice."]));
        assert_eq!(choice_int(&json!("4"), 0..=50), Ok(4));
        assert_eq!(choice_int(&json!(4), 0..=50), Ok(4));
        assert!(choice_int(&json!(4.0), 0..=50).is_err());
        assert!(choice_int(&json!(99), 0..=50).is_err());
        assert_eq!(parse_bool(&json!("on")), Some(true));
        assert_eq!(parse_bool(&json!(0)), Some(false));
        assert_eq!(parse_bool(&json!("maybe")), None);
    }

    #[test]
    fn ints() {
        assert_eq!(parse_int(&json!("5.0")), Some(5));
        assert_eq!(parse_int(&json!(5.0)), Some(5));
        assert_eq!(parse_int(&json!(5.5)), None);
        assert_eq!(parse_int(&json!(true)), None);
        assert_eq!(
            check_int(&json!(0), Some(1), None),
            Err("Ensure this value is greater than or equal to 1.".into())
        );
    }

    #[test]
    fn repr() {
        assert_eq!(py_repr(&json!(["a", 1, null, true])), "['a', 1, None, True]");
        assert_eq!(py_repr(&json!({"k": 1.5})), "{'k': 1.5}");
        assert_eq!(py_float(1.0), "1.0");
        assert_eq!(py_float(1e16), "1e+16");
        assert_eq!(pk_input(&json!("x")), Err("Incorrect type. Expected pk value, received str.".into()));
        assert_eq!(pk_input(&json!(5.7)), Ok(5));
    }
}
