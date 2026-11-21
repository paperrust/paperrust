//! JSON output like DRF `JSONRenderer`.
//!
//! upstream: rest_framework/renderers.py@3.18.3 (`JSONRenderer.render`), CPython `json.dumps`,
//! CPython `float.__repr__`.
//!
//! * `Content-Type: application/json` (without `charset`, `JSONRenderer.charset = None`).
//! * Compact (`separators=(',', ':')`), `ensure_ascii=False` (raw UTF-8), `allow_nan=False`.
//! * ` `/` ` are emitted as `\\u2028`/`\\u2029`.
//! * Floats like Python `repr(float)`: `1.0`, `1e-05`, `1e+16`, `1234567890123456.0`.
//! * `indent=N` from the Accept header (`application/json; indent=4`) → Python indentation
//!   with `separators=(',', ': ')`.
//! * Key order = insertion order (`serde_json` with `preserve_order`).

use std::io;

use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::ser::Formatter;

/// Formatter that emulates Python `json.dumps` (floats, optional indentation).
struct PyFormatter {
    indent: Option<usize>,
    level: usize,
    has_value: bool,
}

impl PyFormatter {
    fn newline_indent<W: ?Sized + io::Write>(&self, w: &mut W) -> io::Result<()> {
        if let Some(n) = self.indent {
            w.write_all(b"\n")?;
            for _ in 0..(n * self.level) {
                w.write_all(b" ")?;
            }
        }
        Ok(())
    }
}

/// Python `repr(float)` (shortest round-trip representation, exponent for `decpt <= -4 || decpt > 16`).
pub fn python_float_repr(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Rust `{:e}` yields the shortest round-trip digit sequence: "-1.2345e-5".
    let s = format!("{v:e}");
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (neg, mant) = match mant.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mant),
    };
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1; // value = 0.DIGITS × 10^decpt
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        out.push('e');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", e.abs()));
    } else if decpt <= 0 {
        out.push_str("0.");
        for _ in 0..(-decpt) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let d = decpt as usize;
        if digits.len() <= d {
            out.push_str(&digits);
            for _ in digits.len()..d {
                out.push('0');
            }
            out.push_str(".0");
        } else {
            out.push_str(&digits[..d]);
            out.push('.');
            out.push_str(&digits[d..]);
        }
    }
    out
}

impl Formatter for PyFormatter {
    fn write_f64<W: ?Sized + io::Write>(&mut self, w: &mut W, value: f64) -> io::Result<()> {
        w.write_all(python_float_repr(value).as_bytes())
    }
    fn write_f32<W: ?Sized + io::Write>(&mut self, w: &mut W, value: f32) -> io::Result<()> {
        // Python only knows double; f32 is emitted like the corresponding double.
        w.write_all(python_float_repr(value as f64).as_bytes())
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.level += 1;
        self.has_value = false;
        w.write_all(b"[")
    }
    fn end_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.level -= 1;
        if self.has_value {
            self.newline_indent(w)?;
        }
        self.has_value = true;
        w.write_all(b"]")
    }
    fn begin_array_value<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if !first {
            w.write_all(b",")?;
        }
        self.newline_indent(w)
    }
    fn end_array_value<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }
    fn begin_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.level += 1;
        self.has_value = false;
        w.write_all(b"{")
    }
    fn end_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.level -= 1;
        if self.has_value {
            self.newline_indent(w)?;
        }
        self.has_value = true;
        w.write_all(b"}")
    }
    fn begin_object_key<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if !first {
            w.write_all(b",")?;
        }
        self.newline_indent(w)
    }
    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        if self.indent.is_some() { w.write_all(b": ") } else { w.write_all(b":") }
    }
    fn end_object_value<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }
}

/// Serializes like DRF `JSONRenderer` (compact or with `indent`).
pub fn to_drf_json_indent<T: Serialize + ?Sized>(value: &T, indent: Option<usize>) -> serde_json::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(256);
    let fmt = PyFormatter { indent, level: 0, has_value: false };
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    value.serialize(&mut ser)?;
    // U+2028 / U+2029 (UTF-8: E2 80 A8 / E2 80 A9) can only appear in strings.
    if buf.windows(3).any(|w| w[0] == 0xE2 && w[1] == 0x80 && (w[2] == 0xA8 || w[2] == 0xA9)) {
        let mut out = Vec::with_capacity(buf.len() + 8);
        let mut i = 0;
        while i < buf.len() {
            if i + 2 < buf.len() && buf[i] == 0xE2 && buf[i + 1] == 0x80 && (buf[i + 2] == 0xA8 || buf[i + 2] == 0xA9) {
                out.extend_from_slice(if buf[i + 2] == 0xA8 { b"\\u2028" } else { b"\\u2029" });
                i += 3;
            } else {
                out.push(buf[i]);
                i += 1;
            }
        }
        buf = out;
    }
    Ok(buf)
}

/// Serializes compactly like DRF `JSONRenderer`.
pub fn to_drf_json<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<Vec<u8>> {
    to_drf_json_indent(value, None)
}

/// Response with status and DRF JSON body (`Content-Type: application/json`).
/// Serialization error → 500 (logged).
pub fn drf_response<T: Serialize + ?Sized>(status: StatusCode, value: &T) -> Response {
    drf_response_indent(status, value, None)
}

/// Like [`drf_response`], with `indent` from [`crate::Negotiation::indent`].
pub fn drf_response_indent<T: Serialize + ?Sized>(status: StatusCode, value: &T, indent: Option<usize>) -> Response {
    match to_drf_json_indent(value, indent) {
        Ok(body) => {
            let mut r = (status, body).into_response();
            r.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            r
        }
        Err(e) => crate::ApiError::internal(format!("serialize response: {e}")).into_response(),
    }
}

/// DRF's `Response(data)`: 200 + JSON. Use [`Drf::created`] etc. for other statuses.
///
/// ```no_run
/// use pl_core::Drf;
/// async fn handler() -> Drf<serde_json::Value> { Drf::ok(serde_json::json!({"id": 1})) }
/// ```
#[derive(Debug, Clone)]
pub struct Drf<T> {
    pub status: StatusCode,
    pub body: T,
    pub indent: Option<usize>,
}

impl<T> Drf<T> {
    /// 200 OK.
    pub fn ok(body: T) -> Self {
        Self { status: StatusCode::OK, body, indent: None }
    }
    /// 201 Created.
    pub fn created(body: T) -> Self {
        Self { status: StatusCode::CREATED, body, indent: None }
    }
    /// Any status.
    pub fn with_status(status: StatusCode, body: T) -> Self {
        Self { status, body, indent: None }
    }
    /// Take over the indentation from the negotiation.
    pub fn indent(mut self, indent: Option<usize>) -> Self {
        self.indent = indent;
        self
    }
}

impl<T: Serialize> IntoResponse for Drf<T> {
    fn into_response(self) -> Response {
        drf_response_indent(self.status, &self.body, self.indent)
    }
}

/// DRF `Response(status=204)` without data: empty body and **no** `Content-Type`
/// (DRF `Response.rendered_content` removes the header when the renderer returns `b''`).
pub fn drf_no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_like_python() {
        for (v, s) in [
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (123456789.125, "123456789.125"),
            (-2.5e-7, "-2.5e-07"),
            (1.5e300, "1.5e+300"),
            (12345678901234567.0, "1.2345678901234568e+16"),
        ] {
            assert_eq!(python_float_repr(v), s, "{v}");
        }
    }

    #[test]
    fn compact_and_unicode() {
        let v = json!({"a": "ä\u{2028}", "b": [1, 2.0, null, true], "c": {}});
        assert_eq!(String::from_utf8(to_drf_json(&v).unwrap()).unwrap(), "{\"a\":\"ä\\u2028\",\"b\":[1,2.0,null,true],\"c\":{}}");
    }

    #[test]
    fn indent_like_python() {
        let v = json!({"a": [1, {}], "b": []});
        assert_eq!(
            String::from_utf8(to_drf_json_indent(&v, Some(2)).unwrap()).unwrap(),
            "{\n  \"a\": [\n    1,\n    {}\n  ],\n  \"b\": []\n}"
        );
    }

    #[test]
    fn control_chars() {
        let v = json!("\u{1}\n\"\\");
        assert_eq!(String::from_utf8(to_drf_json(&v).unwrap()).unwrap(), "\"\\u0001\\n\\\"\\\\\"");
    }
}
