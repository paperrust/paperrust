// upstream: rest_framework/fields.py (IntegerField, BooleanField, ListField, DictField, ChoiceField)
//           rest_framework/serializers.py (Serializer.to_internal_value, as_serializer_error)
//           as used by src/documents/serialisers.py@v3.4.2.
//
//! DRF-compatible field validation with the exact error texts.

use serde_json::{Map, Value};

pub const REQUIRED: &str = "This field is required.";
pub const NULL: &str = "This field may not be null.";
pub const INVALID_INT: &str = "A valid integer is required.";
pub const INVALID_BOOL: &str = "Must be a valid boolean.";
pub const MAX_STRING_LENGTH: &str = "String value too large.";

/// Python type name from `type(data).__name__` for a JSON value as `json.loads` returns it.
pub fn py_type_name(v: &Value) -> &'static str {
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

/// Python truth value (`bool(x)`) of a JSON value.
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `int(s)` for strings: outer whitespace, optional sign, digits with single `_`.
pub fn py_int_from_str(s: &str) -> Option<i64> {
    let t = s.trim();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
    if !cleaned.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: i64 = cleaned.parse().ok()?;
    Some(if neg { -n } else { n })
}

/// Python `str(float)` for the values where DRF `IntegerField` applies: we only need to know
/// whether `int(re.sub(r'\.0*\s*$', '', str(f)))` succeeds – that is exactly the case for integral floats
/// with magnitude < 1e16 (above that Python writes exponent notation).
fn float_as_drf_int(f: f64) -> Option<i64> {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 {
        Some(f as i64)
    } else {
        None
    }
}

/// DRF `IntegerField.to_internal_value` (without null/required handling).
pub fn drf_int(v: &Value) -> Result<i64, &'static str> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(f) = n.as_f64() {
                float_as_drf_int(f).ok_or(INVALID_INT)
            } else {
                Err(INVALID_INT)
            }
        }
        Value::String(s) => {
            if s.chars().count() > 1000 {
                return Err(MAX_STRING_LENGTH);
            }
            // re_decimal = r'\.0*\s*$'
            let stripped = strip_decimal_suffix(s);
            py_int_from_str(stripped).ok_or(INVALID_INT)
        }
        // str(True) == "True" -> int() fails; lists/objects likewise.
        _ => Err(INVALID_INT),
    }
}

fn strip_decimal_suffix(s: &str) -> &str {
    let t = s.trim_end();
    let without_zeros = t.trim_end_matches('0');
    if let Some(prefix) = without_zeros.strip_suffix('.') {
        prefix
    } else {
        s
    }
}

/// DRF `BooleanField.to_internal_value`.
pub fn drf_bool(v: &Value) -> Option<bool> {
    // DRF 3.18.3 `BooleanField._lower_if_str`: strings are lowercased before the lookup.
    const TRUE: &[&str] = &["t", "T", "y", "Y", "yes", "Yes", "YES", "true", "True", "TRUE", "on", "On", "ON", "1"];
    const FALSE: &[&str] = &["f", "F", "n", "N", "no", "No", "NO", "false", "False", "FALSE", "off", "Off", "OFF", "0"];
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => {
            // 1 == 1.0 == True in Python sets
            let f = n.as_f64()?;
            if f == 1.0 {
                Some(true)
            } else if f == 0.0 {
                Some(false)
            } else {
                None
            }
        }
        Value::String(s) if TRUE.contains(&s.to_lowercase().as_str()) => Some(true),
        Value::String(s) if FALSE.contains(&s.to_lowercase().as_str()) => Some(false),
        _ => None,
    }
}

/// Error of a single field, as DRF stores it in `serializer.errors[field]`.
pub type FieldError = Value;

pub fn msg(m: impl Into<String>) -> FieldError {
    Value::Array(vec![Value::String(m.into())])
}

/// DRF `ListField(child=IntegerField())` for a present value.
pub fn int_list(v: &Value) -> Result<Vec<i64>, FieldError> {
    match v {
        Value::Null => Err(msg(NULL)),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            let mut errors = Map::new();
            for (idx, item) in items.iter().enumerate() {
                if item.is_null() {
                    errors.insert(idx.to_string(), msg(NULL));
                    continue;
                }
                match drf_int(item) {
                    Ok(i) => out.push(i),
                    Err(e) => {
                        errors.insert(idx.to_string(), msg(e));
                    }
                }
            }
            if errors.is_empty() { Ok(out) } else { Err(Value::Object(errors)) }
        }
        other => Err(msg(format!(
            "Expected a list of items but got type \"{}\".",
            py_type_name(other)
        ))),
    }
}

/// DRF `DictField()` (children unvalidated).
pub fn dict(v: &Value) -> Result<Map<String, Value>, FieldError> {
    match v {
        Value::Null => Err(msg(NULL)),
        Value::Object(o) => Ok(o.clone()),
        other => Err(msg(format!(
            "Expected a dictionary of items but got type \"{}\".",
            py_type_name(other)
        ))),
    }
}

/// DRF `BooleanField` for a present value.
pub fn boolean(v: &Value) -> Result<bool, FieldError> {
    if v.is_null() {
        return Err(msg(NULL));
    }
    drf_bool(v).ok_or_else(|| msg(INVALID_BOOL))
}

/// Representation of a choice input value in `"{input}" is not a valid choice.` (Python `str(x)`).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                py_float_repr(f)
            } else {
                n.to_string()
            }
        }
        other => py_repr(other),
    }
}

fn py_float_repr(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e16 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

/// Python `repr()` of a JSON value (for Celery `kwargsrepr` and error texts).
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => {
            if n.is_f64() {
                n.as_f64().map(py_float_repr).unwrap_or_else(|| n.to_string())
            } else {
                n.to_string()
            }
        }
        Value::String(s) => py_str_repr(s),
        Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(py_repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(o) => {
            let parts: Vec<String> = o
                .iter()
                .map(|(k, v)| format!("{}: {}", py_str_repr(k), py_repr(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Django would raise `ValueError`/`TypeError` on lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookupTypeError;

/// Value as Django converts it to an integer for `Model.objects.get(id=x)` or `filter(id=x)`
/// (`IntegerField.get_prep_value` → `int(x)`).
///
/// - `Ok(None)`: `x is None` → matches no row.
/// - `Err(LookupTypeError)`: Django raises `ValueError`/`TypeError` (no `DoesNotExist`) → upstream 500.
pub fn django_int_lookup(v: &Value) -> Result<Option<i64>, LookupTypeError> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(i64::from(*b))),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Some(i))
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() && f.abs() < 9.2e18 { Ok(Some(f.trunc() as i64)) } else { Err(LookupTypeError) }
            } else {
                Err(LookupTypeError)
            }
        }
        Value::String(s) => py_int_from_str(s).map(Some).ok_or(LookupTypeError),
        _ => Err(LookupTypeError),
    }
}

/// Error collector in field order (`serializer.errors`).
#[derive(Default, Debug)]
pub struct Errors(pub Map<String, Value>);

impl Errors {
    pub fn add(&mut self, field: &str, err: FieldError) {
        self.0.insert(field.to_string(), err);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `{"non_field_errors": [msg]}`
pub fn non_field(m: impl Into<String>) -> Value {
    let mut map = Map::new();
    map.insert("non_field_errors".into(), msg(m));
    Value::Object(map)
}

/// `as_serializer_error` for a `ValidationError` with a dict detail raised in `validate()`:
/// every value that is not a dict/list becomes a one-element list.
pub fn as_serializer_error(detail: Map<String, Value>) -> Value {
    let mut out = Map::new();
    for (k, v) in detail {
        let v = match v {
            Value::Array(_) | Value::Object(_) => v,
            other => Value::Array(vec![other]),
        };
        out.insert(k, v);
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 1 N4 (checked against DRF 3.18.3 of the reference): `BooleanField` compares in lowercase,
    /// `"tRuE"`/`"yES"` are valid, unknown values are not.
    #[test]
    fn drf_bool_is_case_insensitive_like_drf_3_18() {
        use serde_json::json;
        assert_eq!(drf_bool(&json!("tRuE")), Some(true));
        assert_eq!(drf_bool(&json!("oFF")), Some(false));
        assert_eq!(drf_bool(&json!("maybe")), None);
    }
    use serde_json::json;

    #[test]
    fn ints_like_drf() {
        assert_eq!(drf_int(&json!(5)), Ok(5));
        assert_eq!(drf_int(&json!("5")), Ok(5));
        assert_eq!(drf_int(&json!(" 7 ")), Ok(7));
        assert_eq!(drf_int(&json!("5.000")), Ok(5));
        assert_eq!(drf_int(&json!(5.0)), Ok(5));
        assert_eq!(drf_int(&json!("1_000")), Ok(1000));
        assert_eq!(drf_int(&json!(5.5)), Err(INVALID_INT));
        assert_eq!(drf_int(&json!(true)), Err(INVALID_INT));
        assert_eq!(drf_int(&json!("abc")), Err(INVALID_INT));
        assert_eq!(drf_int(&json!([1])), Err(INVALID_INT));
    }

    #[test]
    fn list_errors_like_drf() {
        assert_eq!(int_list(&json!("x")).unwrap_err(), json!(["Expected a list of items but got type \"str\"."]));
        assert_eq!(int_list(&json!({})).unwrap_err(), json!(["Expected a list of items but got type \"dict\"."]));
        assert_eq!(int_list(&json!([1, "a", null])).unwrap_err(), json!({"1": [INVALID_INT], "2": [NULL]}));
        assert_eq!(int_list(&json!([1, "2"])).unwrap(), vec![1, 2]);
    }

    #[test]
    fn bools_like_drf() {
        assert_eq!(drf_bool(&json!("yes")), Some(true));
        assert_eq!(drf_bool(&json!(0)), Some(false));
        assert_eq!(drf_bool(&json!("maybe")), None);
        assert_eq!(drf_bool(&json!(2)), None);
    }

    #[test]
    fn repr_like_python() {
        assert_eq!(py_repr(&json!({"document_ids": [1, 2], "remote_ocr": false})), "{'document_ids': [1, 2], 'remote_ocr': False}");
        assert_eq!(py_repr(&json!("it's")), "\"it's\"");
        assert_eq!(py_str(&json!(1.0)), "1.0");
    }

    #[test]
    fn django_lookup() {
        assert_eq!(django_int_lookup(&json!(true)), Ok(Some(1)));
        assert_eq!(django_int_lookup(&json!(1.9)), Ok(Some(1)));
        assert_eq!(django_int_lookup(&json!("x")), Err(LookupTypeError));
        assert_eq!(django_int_lookup(&json!(null)), Ok(None));
    }
}
