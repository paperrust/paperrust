//! JSON output byte-identical to Python `json.dumps(obj, indent=2, ensure_ascii=False)` (or without
//! indent with default separators) and Python `repr(float)`.
//!
//! upstream: CPython 3.12 Lib/json/encoder.py (`_make_iterencode`, `py_encode_basestring`),
//! Objects/floatobject.c (`float_repr`, format "r": exponent form at decpt ≤ -4 or > 16).

use serde_json::Value;

/// Python `repr(f)` for finite values; `NaN`/`Infinity`/`-Infinity` like `json.dumps`.
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Rust `{:e}` yields the shortest round-trip digit sequence: d[.ddd]e<exp>
    let s = format!("{f:e}");
    let (mant, exp) = s.split_once('e').unwrap_or((s.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let sign = if neg { "-" } else { "" };
    if (-4..16).contains(&exp) {
        // fixed point
        let decpt = exp + 1; // position of the decimal point relative to the digits
        let n = digits.len() as i32;
        if decpt <= 0 {
            format!("{sign}0.{}{digits}", "0".repeat((-decpt) as usize))
        } else if decpt >= n {
            format!("{sign}{digits}{}.0", "0".repeat((decpt - n) as usize))
        } else {
            let (a, b) = digits.split_at(decpt as usize);
            format!("{sign}{a}.{b}")
        }
    } else {
        let (first, rest) = digits.split_at(1);
        let m = if rest.is_empty() { first.to_string() } else { format!("{first}.{rest}") };
        let es = if exp < 0 { "-" } else { "+" };
        format!("{sign}{m}e{es}{:02}", exp.abs())
    }
}

/// `py_encode_basestring` (ensure_ascii=False).
pub fn py_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn number(n: &serde_json::Number, out: &mut String) {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
    } else if let Some(f) = n.as_f64() {
        out.push_str(&py_float_repr(f));
    }
}

fn write(v: &Value, indent: Option<usize>, level: usize, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => number(n, out),
        Value::String(s) => py_str(s, out),
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                newline(indent, level + 1, out);
                write(x, indent, level + 1, out);
            }
            newline(indent, level, out);
            out.push(']');
        }
        Value::Object(m) => {
            if m.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                newline(indent, level + 1, out);
                py_str(k, out);
                out.push_str(": ");
                write(x, indent, level + 1, out);
            }
            newline(indent, level, out);
            out.push('}');
        }
    }
}

fn newline(indent: Option<usize>, level: usize, out: &mut String) {
    if let Some(n) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(n * level));
    }
}

/// `json.dumps(v, indent=indent, ensure_ascii=False)`.
pub fn dumps(v: &Value, indent: Option<usize>) -> String {
    let mut s = String::new();
    write(v, indent, 0, &mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn float_repr_like_python() {
        for (f, s) in [
            (0.8571, "0.8571"),
            (100.0, "100.0"),
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (-2.5e-7, "-2.5e-07"),
            (1.5e300, "1.5e+300"),
            (0.1 + 0.2, "0.30000000000000004"),
            (-0.0, "-0.0"),
            (123.456, "123.456"),
        ] {
            assert_eq!(py_float_repr(f), s, "{f}");
        }
    }

    #[test]
    fn dumps_like_python() {
        let v = json!({"a": [], "b": {}, "c": [1, {"x": "ä\"\n\u{1}"}], "d": 1.0, "e": null});
        assert_eq!(
            dumps(&v, Some(2)),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"x\": \"ä\\\"\\n\\u0001\"\n    }\n  ],\n  \"d\": 1.0,\n  \"e\": null\n}"
        );
        assert_eq!(dumps(&json!([1, {"a": 2}]), None), "[1, {\"a\": 2}]");
    }
}
