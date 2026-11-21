//! Small pure helpers that mirror Python/Django string behaviour.
//!
//! upstream: django/utils/text.py@5.2 (slugify),
//!           src/documents/serialisers.py@v3.4.2 (TagSerializer.get_text_color),
//!           src/documents/utils.py@v3.4.2 (get_boolean)

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// Python `str.isspace()` restricted to what can survive the ASCII step of slugify.
fn is_py_ascii_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' | '\x1c'..='\x1f')
}

/// Python `str.isspace()` (Unicode White_Space + bidi B/S/WS, as used by `str.strip()`).
pub fn is_py_space(c: char) -> bool {
    is_py_ascii_space(c) || c == '\u{85}' || c == '\u{a0}' || (c != '\u{200b}' && c.is_whitespace())
}

/// Django `slugify(value, allow_unicode=False)`.
pub fn slugify(value: &str) -> String {
    let ascii: String = value.nfkd().filter(|c| c.is_ascii()).collect();
    let lowered = ascii.to_lowercase();
    // re.sub(r"[^\w\s-]", "", value)
    let kept: String = lowered.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || is_py_ascii_space(*c)).collect();
    // re.sub(r"[-\s]+", "-", value)
    let mut out = String::with_capacity(kept.len());
    let mut in_run = false;
    for c in kept.chars() {
        if c == '-' || is_py_ascii_space(c) {
            if !in_run {
                out.push('-');
                in_run = true;
            }
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out.trim_matches(|c| c == '-' || c == '_').to_string()
}

/// Python `int(s, 16)` for the short slices used by `get_text_color`.
fn py_int_hex(s: &str) -> Option<i64> {
    let t = s.trim_matches(is_py_space);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
    let v = i64::from_str_radix(&cleaned, 16).ok()?;
    Some(if neg { -v } else { v })
}

/// `TagSerializer.get_text_color`.
pub fn text_color(color: &str) -> &'static str {
    let h = color.trim_start_matches('#');
    let slice = |a: usize| -> &str {
        let chars: Vec<(usize, char)> = h.char_indices().collect();
        let start = chars.get(a).map(|(i, _)| *i).unwrap_or(h.len());
        let end = chars.get(a + 2).map(|(i, _)| *i).unwrap_or(h.len());
        &h[start..end]
    };
    let mut rgb = [0f64; 3];
    for (k, i) in [0usize, 2, 4].iter().enumerate() {
        match py_int_hex(slice(*i)) {
            Some(v) => rgb[k] = v as f64 / 256.0,
            None => return "#000000",
        }
    }
    let luminance = (0.299 * (rgb[0] * rgb[0]) + 0.587 * (rgb[1] * rgb[1]) + 0.114 * (rgb[2] * rgb[2])).sqrt();
    if luminance < 0.53 { "#ffffff" } else { "#000000" }
}

/// `documents.utils.get_boolean`.
pub fn get_boolean(s: &str) -> bool {
    matches!(s.to_lowercase().as_str(), "yes" | "y" | "1" | "t" | "true")
}

/// Python `type(x).__name__` for a JSON value.
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

/// Python `repr(float)`.
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Shortest round-trip digits in scientific form, e.g. "1.5e-5".
    let sci = format!("{f:e}");
    let (mant, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let sign = if neg { "-" } else { "" };
    if !(-4..16).contains(&exp) {
        let m = if digits.len() > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits.clone() };
        let es = if exp < 0 { format!("-{:02}", -exp) } else { format!("+{exp:02}") };
        return format!("{sign}{m}e{es}");
    }
    let point = exp + 1; // position of decimal point relative to digits start
    let s = if point <= 0 {
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else if (point as usize) >= digits.len() {
        format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
    } else {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    };
    format!("{sign}{s}")
}

/// Python `str(x)` for a JSON scalar as DRF sees it (`True`, `None`, `1.0`, ...).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) if !n.is_f64() => i.to_string(),
            (_, Some(u)) if !n.is_f64() => u.to_string(),
            _ => py_float_repr(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => s.clone(),
        other => py_repr_composite(other),
    }
}

/// Rough Python `str()` of a list/dict (only used inside error texts).
fn py_repr_composite(v: &Value) -> String {
    fn r(v: &Value) -> String {
        match v {
            Value::String(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
            Value::Array(a) => format!("[{}]", a.iter().map(r).collect::<Vec<_>>().join(", ")),
            Value::Object(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("'{}': {}", k, r(v))).collect::<Vec<_>>().join(", ")),
            other => py_str(other),
        }
    }
    r(v)
}

/// Python `int(str)` (used for DRF pagination and integer parsing of query strings).
pub fn py_int(s: &str) -> Option<i64> {
    let t = s.trim_matches(is_py_space);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut v: i64 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(10)? as i64;
        v = v.checked_mul(10)?.checked_add(d)?;
    }
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slugify_like_django() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("  Ärger -- über_Straße! "), "arger-uber_strae");
        assert_eq!(slugify("_tag_"), "tag");
        assert_eq!(slugify("a\tb\nc"), "a-b-c");
        assert_eq!(slugify("日本"), "");
    }

    #[test]
    fn text_color_like_upstream() {
        assert_eq!(text_color("#a6cee3"), "#000000");
        assert_eq!(text_color("#000000"), "#ffffff");
        assert_eq!(text_color("#ffffff"), "#000000");
        assert_eq!(text_color("#1f78b4"), "#ffffff");
        assert_eq!(text_color("#fff"), "#000000"); // int('') -> ValueError
        assert_eq!(text_color("zzzzzz"), "#000000");
    }

    #[test]
    fn python_str() {
        assert_eq!(py_str(&json!(1.0)), "1.0");
        assert_eq!(py_str(&json!(1.5e-5)), "1.5e-05");
        assert_eq!(py_str(&json!(1e16)), "1e+16");
        assert_eq!(py_str(&json!(123.25)), "123.25");
        assert_eq!(py_str(&json!(0.001)), "0.001");
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!(7)), "7");
        assert_eq!(py_str(&json!(["a", 1])), "['a', 1]");
    }

    #[test]
    fn python_int() {
        assert_eq!(py_int(" 12 "), Some(12));
        assert_eq!(py_int("+3"), Some(3));
        assert_eq!(py_int("1_0"), Some(10));
        assert_eq!(py_int("1.0"), None);
        assert_eq!(py_int(""), None);
    }

    #[test]
    fn booleans() {
        assert!(get_boolean("TRUE"));
        assert!(get_boolean("y"));
        assert!(!get_boolean("on"));
    }
}
