//! Python formatting of JSON values as it ends up in DRF error messages
//! (`str(data)`, `repr(data)`, `type(data).__name__`) and Python `int()` parsing.
//! upstream: behaviour of CPython 3.12 (`str`, `repr`, `int`) for values from `json.loads`.

use serde_json::Value;

/// `type(value).__name__` for a value from `json.loads`.
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) => {
            if n.is_f64() {
                "float"
            } else {
                "int"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python `repr(float)` (shortest representation, Python rules for the exponent).
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Rust `{:e}` yields the shortest round-trip-safe digit sequence, e.g. "1.5e-5".
    let s = format!("{:e}", f);
    let (mant, exp) = match s.split_once('e') {
        Some((m, e)) => (m.to_string(), e.parse::<i32>().unwrap_or(0)),
        None => (s.clone(), 0),
    };
    let neg = mant.starts_with('-');
    let mant = mant.trim_start_matches('-');
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let sign = if neg { "-" } else { "" };
    if (-4..16).contains(&exp) {
        // Fixed point
        let n = digits.len() as i32;
        let point = exp + 1; // Position of the decimal point in `digits`
        let body = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point >= n {
            format!("{}{}.0", digits, "0".repeat((point - n) as usize))
        } else {
            format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
        };
        format!("{sign}{body}")
    } else {
        let m = if digits.len() > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits.clone() };
        let es = if exp < 0 { "-" } else { "+" };
        format!("{sign}{m}e{es}{:02}", exp.abs())
    }
}

fn number_str(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        float_repr(n.as_f64().unwrap_or(0.0))
    }
}

/// Python `repr(str)`.
pub fn str_repr(s: &str) -> String {
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
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if (0x80..0xa0).contains(&(c as u32)) => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr(value)`.
pub fn repr(v: &Value) -> String {
    match v {
        Value::String(s) => str_repr(s),
        Value::Array(a) => {
            let items: Vec<String> = a.iter().map(repr).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(o) => {
            let items: Vec<String> = o.iter().map(|(k, v)| format!("{}: {}", str_repr(k), repr(v))).collect();
            format!("{{{}}}", items.join(", "))
        }
        other => py_str(other),
    }
}

/// Python `str(value)`.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => number_str(n),
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}

/// Python `str.strip()` whitespace (Unicode `isspace`).
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// `TypeError`/`ValueError` from Python `int()`.
#[derive(Debug, PartialEq, Eq)]
pub struct PyTypeError;

/// Result of Python `int(text)`.
#[derive(Debug, PartialEq, Eq)]
pub enum PyInt {
    Ok(i64),
    /// Valid number, but outside i64.
    Overflow(bool /* negative */),
    Invalid,
}

/// Python `int(str)`: whitespace, optional sign, digits with single `_` in between.
pub fn parse_int(text: &str) -> PyInt {
    let t = py_strip(text);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() {
        return PyInt::Invalid;
    }
    let mut prev_us = true; // no leading `_`
    let mut acc: i128 = 0;
    let mut overflow = false;
    for c in digits.chars() {
        if c == '_' {
            if prev_us {
                return PyInt::Invalid;
            }
            prev_us = true;
            continue;
        }
        let Some(d) = c.to_digit(10) else {
            return PyInt::Invalid;
        };
        prev_us = false;
        if !overflow {
            acc = acc * 10 + d as i128;
            if acc > i64::MAX as i128 + 1 {
                overflow = true;
            }
        }
    }
    if prev_us {
        return PyInt::Invalid;
    }
    if overflow {
        return PyInt::Overflow(neg);
    }
    let v = if neg { -acc } else { acc };
    if v > i64::MAX as i128 || v < i64::MIN as i128 { PyInt::Overflow(neg) } else { PyInt::Ok(v as i64) }
}

/// Python `int(value)` for a JSON value (like Django `IntegerField.get_prep_value`).
/// `Err(PyTypeError)` corresponds to `TypeError`/`ValueError`.
pub fn int_of_value(v: &Value) -> Result<PyInt, PyTypeError> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(PyInt::Ok(i))
            } else if n.as_u64().is_some() {
                Ok(PyInt::Overflow(false))
            } else {
                let f = n.as_f64().unwrap_or(0.0);
                if !f.is_finite() {
                    return Err(PyTypeError);
                }
                let t = f.trunc();
                if (-9.2e18..=9.2e18).contains(&t) { Ok(PyInt::Ok(t as i64)) } else { Ok(PyInt::Overflow(t < 0.0)) }
            }
        }
        // bool is an int in Python; the DRF fields catch bool beforehand.
        Value::Bool(b) => Ok(PyInt::Ok(*b as i64)),
        Value::String(s) => match parse_int(s) {
            PyInt::Invalid => Err(PyTypeError),
            other => Ok(other),
        },
        _ => Err(PyTypeError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats() {
        assert_eq!(float_repr(1.0), "1.0");
        assert_eq!(float_repr(1.5), "1.5");
        assert_eq!(float_repr(500.0), "500.0");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1e15), "1000000000000000.0");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(0.00001), "1e-05");
        assert_eq!(float_repr(1.5e-7), "1.5e-07");
        assert_eq!(float_repr(-2.25), "-2.25");
        assert_eq!(float_repr(123456.789), "123456.789");
    }

    #[test]
    fn strs() {
        assert_eq!(py_str(&json!(null)), "None");
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!([1, "a", null])), "[1, 'a', None]");
        assert_eq!(py_str(&json!({"a": 1})), "{'a': 1}");
        assert_eq!(str_repr("it's"), "\"it's\"");
        assert_eq!(str_repr("a'\"b"), "'a\\'\"b'");
    }

    #[test]
    fn ints() {
        assert_eq!(parse_int(" 5 "), PyInt::Ok(5));
        assert_eq!(parse_int("+007"), PyInt::Ok(7));
        assert_eq!(parse_int("1_000"), PyInt::Ok(1000));
        assert_eq!(parse_int("1__0"), PyInt::Invalid);
        assert_eq!(parse_int("_1"), PyInt::Invalid);
        assert_eq!(parse_int("1.5"), PyInt::Invalid);
        assert_eq!(parse_int(""), PyInt::Invalid);
        assert_eq!(parse_int("-"), PyInt::Invalid);
        assert_eq!(parse_int("99999999999999999999"), PyInt::Overflow(false));
    }
}
