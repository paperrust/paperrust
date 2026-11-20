//! Python `repr()` for JSON values (for Celery `argsrepr`/`kwargsrepr`).
//!
//! upstream: CPython `repr(str)` (choice of quotes, escapes), `repr(tuple)` (`(x,)`),
//! celery/utils/saferepr.py (`saferepr(obj, maxlen)` truncates long representations).

use serde_json::Value;

/// Python `repr(str)`.
pub fn repr_str(s: &str) -> String {
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
            c if (0x80..0xa0).contains(&(c as u32)) => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr()` of an object loaded from JSON.
pub fn repr_value(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if !n.is_i64() && !n.is_u64() => crate::json::python_float_repr(f),
            _ => n.to_string(),
        },
        Value::String(s) => repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr_value).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter().map(|(k, v)| format!("{}: {}", repr_str(k), repr_value(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Python `repr(tuple(args))`.
pub fn repr_tuple(items: &[Value]) -> String {
    match items.len() {
        0 => "()".into(),
        1 => format!("({},)", repr_value(&items[0])),
        _ => format!("({})", items.iter().map(repr_value).collect::<Vec<_>>().join(", ")),
    }
}

/// Celery `saferepr`-like truncation to `maxlen` characters (Celery default 1024).
pub fn truncate_repr(s: String, maxlen: usize) -> String {
    if s.chars().count() <= maxlen {
        s
    } else {
        let mut t: String = s.chars().take(maxlen).collect();
        t.push_str("...");
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reprs() {
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("a'\"b"), "'a\\'\"b'");
        assert_eq!(repr_tuple(&[json!(1)]), "(1,)");
        assert_eq!(repr_tuple(&[]), "()");
        assert_eq!(
            repr_value(&json!({"document_ids": [1, 2], "x": null, "f": 1.5, "b": true})),
            "{'document_ids': [1, 2], 'x': None, 'f': 1.5, 'b': True}"
        );
    }
}
