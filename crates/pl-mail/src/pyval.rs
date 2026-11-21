//! Python semantics for JSON values, as DRF uses them in error messages and conversions
//! (`str(x)`, `type(x).__name__`, `int(x)`, `str.strip()`).

use serde_json::Value;

/// `type(x).__name__` of a Python value parsed from JSON.
pub fn type_name(v: &Value) -> &'static str {
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

/// Python `repr(float)` (shortest representation, exponent from 1e16 or below 1e-4).
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
    // Rust `{:e}` yields the shortest digit sequence, e.g. "1.5e20", "-3e-5".
    let sci = format!("{f:e}");
    let (mant, exp) = match sci.split_once('e') {
        Some((m, e)) => (m.to_string(), e.parse::<i32>().unwrap_or(0)),
        None => (sci.clone(), 0),
    };
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let sign = if neg { "-" } else { "" };
    if (-4..16).contains(&exp) {
        // fixed point
        let n = digits.len() as i32;
        let point = exp + 1; // position of the decimal point relative to the first digit
        let s = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point >= n {
            format!("{}{}.0", digits, "0".repeat((point - n) as usize))
        } else {
            format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
        };
        format!("{sign}{s}")
    } else {
        let m = if digits.len() > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits.clone() };
        let es = if exp < 0 { "-" } else { "+" };
        format!("{sign}{m}e{es}{:02}", exp.abs())
    }
}

fn py_repr_str(s: &str) -> String {
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
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr(x)`.
pub fn repr(v: &Value) -> String {
    match v {
        Value::String(s) => py_repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", py_repr_str(k), repr(v))).collect::<Vec<_>>().join(", "))
        }
        other => to_str(other),
    }
}

/// Python `str(x)`.
pub fn to_str(v: &Value) -> String {
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
                float_repr(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => repr(v),
    }
}

/// Characters that Python `str.strip()`/`str.isspace()` counts as whitespace.
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// Python `int(s)` for text (sign, underscores between digits, outer whitespace).
/// `None` = ValueError. Values outside i128 are saturated to `i128::MAX/MIN`
/// (relevant for range checks only).
pub fn parse_py_int(s: &str) -> Option<i128> {
    let t = py_strip(s);
    let (neg, body) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut acc: i128 = 0;
    let mut saturated = false;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(10)? as i128;
        if !saturated {
            match acc.checked_mul(10).and_then(|x| x.checked_add(d)) {
                Some(v) => acc = v,
                None => saturated = true,
            }
        }
    }
    if saturated {
        return Some(if neg { i128::MIN } else { i128::MAX });
    }
    Some(if neg { -acc } else { acc })
}

/// Result of Python `int(value)` for the Django field conversion
/// (`IntegerField.get_prep_value`).
#[derive(Debug, PartialEq, Eq)]
pub enum PyInt {
    Ok(i128),
    /// TypeError/ValueError
    Invalid,
}

pub fn py_int(v: &Value) -> PyInt {
    match v {
        Value::Bool(b) => PyInt::Ok(*b as i128),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                PyInt::Ok(i as i128)
            } else if let Some(u) = n.as_u64() {
                PyInt::Ok(u as i128)
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if f.is_finite() { PyInt::Ok(f.trunc() as i128) } else { PyInt::Invalid }
            }
        }
        Value::String(s) => match parse_py_int(s) {
            Some(i) => PyInt::Ok(i),
            None => PyInt::Invalid,
        },
        _ => PyInt::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn float_repr_matches_python() {
        assert_eq!(float_repr(1.0), "1.0");
        assert_eq!(float_repr(2.5), "2.5");
        assert_eq!(float_repr(1e20), "1e+20");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(123456789012345.0), "123456789012345.0");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(0.00001), "1e-05");
        assert_eq!(float_repr(-1.5e-7), "-1.5e-07");
        assert_eq!(float_repr(0.1), "0.1");
    }

    #[test]
    fn str_and_repr() {
        assert_eq!(to_str(&json!(true)), "True");
        assert_eq!(to_str(&json!([1, "a", null])), "[1, 'a', None]");
        assert_eq!(to_str(&json!({"a": 1})), "{'a': 1}");
        assert_eq!(to_str(&json!("it's")), "it's");
        assert_eq!(repr(&json!("it's")), "\"it's\"");
    }

    #[test]
    fn py_int_parsing() {
        assert_eq!(parse_py_int(" 12 "), Some(12));
        assert_eq!(parse_py_int("1_000"), Some(1000));
        assert_eq!(parse_py_int("-5"), Some(-5));
        assert_eq!(parse_py_int("1__0"), None);
        assert_eq!(parse_py_int("1.5"), None);
        assert_eq!(parse_py_int(""), None);
        assert_eq!(py_int(&json!(1.9)), PyInt::Ok(1));
        assert_eq!(py_int(&json!(true)), PyInt::Ok(1));
        assert_eq!(py_int(&json!([1])), PyInt::Invalid);
    }
}
