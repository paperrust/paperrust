//! Python's `json.dumps` for the cases that paperless_ai uses:
//! * `json.dumps(payload, ensure_ascii=False)` (default separators `", "` / `": "`) in
//!   `taxonomy.format_taxonomy_for_prompt`;
//! * `json.dumps(obj, separators=(",", ":"))` (ensure_ascii=True) in the chat trailer.
//!
//! Numbers: integers like Python; floats via `serde_json` (irrelevant for paperless_ai,
//! the payloads contain only ids and texts).

use serde_json::Value;

/// Options like `json.dumps`.
#[derive(Debug, Clone, Copy)]
pub struct Dumps {
    pub item_sep: &'static str,
    pub key_sep: &'static str,
    pub ensure_ascii: bool,
}

/// `json.dumps(v)` Standard (`ensure_ascii=True`, `", "`, `": "`).
pub const DEFAULT: Dumps = Dumps { item_sep: ", ", key_sep: ": ", ensure_ascii: true };

fn push_str(out: &mut String, s: &str, ensure_ascii: bool) {
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
            c if ensure_ascii && (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write(out: &mut String, v: &Value, o: &Dumps) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => push_str(out, s, o.ensure_ascii),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(o.item_sep);
                }
                write(out, x, o);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(o.item_sep);
                }
                push_str(out, k, o.ensure_ascii);
                out.push_str(o.key_sep);
                write(out, x, o);
            }
            out.push('}');
        }
    }
}

/// `json.dumps(v, ...)`.
pub fn dumps(v: &Value, o: &Dumps) -> String {
    let mut s = String::new();
    write(&mut s, v, o);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn like_python() {
        let v = json!({"a": [1, "ä\u{7f}😀"], "b": null});
        let bs = "\\";
        let exp = format!("{{\"a\": [1, \"{bs}u00e4{bs}u007f{bs}ud83d{bs}ude00\"], \"b\": null}}");
        assert_eq!(dumps(&v, &DEFAULT), exp);
        let o = Dumps { item_sep: ",", key_sep: ":", ensure_ascii: false };
        assert_eq!(dumps(&v, &o), "{\"a\":[1,\"ä\u{7f}😀\"],\"b\":null}");
    }
}
