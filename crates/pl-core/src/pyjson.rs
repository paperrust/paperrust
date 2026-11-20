//! Text of DRF's `JSONParser` error for an invalid JSON body, byte-for-byte like CPython 3.12.
//!
//! upstream: rest_framework/parsers.py@3.18.3 (`JSONParser.parse`: `ParseError('JSON parse error - %s'
//! % str(exc))`, `parse_constant=strict_constant`), rest_framework/utils/json.py (`strict_constant`),
//! CPython 3.12 Lib/json/decoder.py (`JSONDecodeError`, `JSONDecoder.decode/raw_decode`) and
//! Modules/_json.c (`scan_once_unicode`, `_parse_object_unicode`, `_parse_array_unicode`,
//! `scanstring_unicode`, `_match_number_unicode`) - the C scanner is what `json.load` uses.
//!
//! [`decode_error`] re-scans the body with the same algorithm and returns the message Python would
//! produce, e.g. `Expecting property name enclosed in double quotes: line 1 column 2 (char 1)`.
//! Positions count code points (Python `str` indices), lines/columns like `JSONDecodeError`.
//! W2 parity run 2026-11-08 (`w2.cf.create.validation`): serde_json's own messages differed.
//!
//! Moved from pl-fields-views (W11); used by [`crate::ApiError::json_body_parse_error`].
//! Golden vectors: `tests/parity/pl-core/pyjson_vectors.py` → `pyjson_vectors.in`.

/// Python's message (without the `JSON parse error - ` prefix) for `body`, or `None` when Python
/// would accept it.
pub fn decode_error(body: &[u8]) -> Option<String> {
    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(e) => return Some(utf8_error(body, &e)),
    };
    let s: Vec<char> = text.chars().collect();
    let mut p = Parser { s: &s, depth: 0 };
    match p.decode() {
        Ok(()) => None,
        Err(Fail::At(msg, pos)) => Some(p.format(msg, pos)),
        Err(Fail::Constant(c)) => Some(format!("Out of range float values are not JSON compliant: '{c}'")),
        Err(Fail::Recursion) => Some("maximum recursion depth exceeded while decoding a JSON object from a unicode string".into()),
    }
}

/// `codecs` UTF-8 reader: `'utf-8' codec can't decode byte 0x.. in position N: <reason>`.
fn utf8_error(body: &[u8], e: &std::str::Utf8Error) -> String {
    let pos = e.valid_up_to();
    let byte = body[pos];
    let reason = match e.error_len() {
        None => "unexpected end of data",
        Some(_) if (0x80..0xc2).contains(&byte) || byte >= 0xf5 => "invalid start byte",
        Some(_) => "invalid continuation byte",
    };
    // Python counts the position in the *incremental* decoder's current chunk; the whole body is
    // one chunk here (json.load -> fp.read()).
    format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {pos}: {reason}")
}

enum Fail {
    At(&'static str, usize),
    Constant(&'static str),
    Recursion,
}

struct Parser<'a> {
    s: &'a [char],
    depth: usize,
}

const WS: [char; 4] = [' ', '\t', '\n', '\r'];
/// `sys.getrecursionlimit()` (1000) - Python raises RecursionError somewhat below; only an
/// approximation for absurdly deep documents.
const MAX_DEPTH: usize = 990;

impl Parser<'_> {
    fn len(&self) -> usize {
        self.s.len()
    }

    fn ws(&self, mut i: usize) -> usize {
        while i < self.len() && WS.contains(&self.s[i]) {
            i += 1;
        }
        i
    }

    /// `JSONDecodeError.__init__`: `'%s: line %d column %d (char %d)'`.
    fn format(&self, msg: &str, pos: usize) -> String {
        let before = &self.s[..pos.min(self.len())];
        let lineno = before.iter().filter(|c| **c == '\n').count() + 1;
        let colno = match before.iter().rposition(|c| *c == '\n') {
            Some(nl) => pos - nl,
            None => pos + 1,
        };
        format!("{msg}: line {lineno} column {colno} (char {pos})")
    }

    /// `JSONDecoder.decode`: BOM check, `raw_decode` at the first non-blank, then "Extra data".
    fn decode(&mut self) -> Result<(), Fail> {
        if self.s.first() == Some(&'\u{feff}') {
            return Err(Fail::At("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
        }
        let start = self.ws(0);
        let end = match self.scan_once(start) {
            Ok(e) => e,
            Err(Stop::Iter(i)) => return Err(Fail::At("Expecting value", i)),
            Err(Stop::Fail(f)) => return Err(f),
        };
        let end = self.ws(end);
        if end != self.len() {
            return Err(Fail::At("Extra data", end));
        }
        Ok(())
    }

    fn scan_once(&mut self, idx: usize) -> Result<usize, Stop> {
        let len = self.len();
        if idx >= len {
            return Err(Stop::Iter(idx));
        }
        let lit = |w: &str| -> bool {
            let w: Vec<char> = w.chars().collect();
            idx + w.len() <= len && self.s[idx..idx + w.len()] == w[..]
        };
        match self.s[idx] {
            '"' => return self.scanstring(idx + 1).map_err(Stop::Fail),
            '{' => return self.nested(|p| p.object(idx + 1)),
            '[' => return self.nested(|p| p.array(idx + 1)),
            'n' if lit("null") => return Ok(idx + 4),
            't' if lit("true") => return Ok(idx + 4),
            'f' if lit("false") => return Ok(idx + 5),
            'N' if lit("NaN") => return Err(Stop::Fail(Fail::Constant("NaN"))),
            'I' if lit("Infinity") => return Err(Stop::Fail(Fail::Constant("Infinity"))),
            '-' if lit("-Infinity") => return Err(Stop::Fail(Fail::Constant("-Infinity"))),
            _ => {}
        }
        self.number(idx)
    }

    fn nested(&mut self, f: impl FnOnce(&mut Self) -> Result<usize, Fail>) -> Result<usize, Stop> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Stop::Fail(Fail::Recursion));
        }
        let r = f(self);
        self.depth -= 1;
        r.map_err(Stop::Fail)
    }

    /// `_match_number_unicode`
    fn number(&self, start: usize) -> Result<usize, Stop> {
        let s = self.s;
        let len = s.len();
        let mut idx = start;
        if s[idx] == '-' {
            idx += 1;
            if idx >= len {
                return Err(Stop::Iter(start));
            }
        }
        if ('1'..='9').contains(&s[idx]) {
            idx += 1;
            while idx < len && s[idx].is_ascii_digit() {
                idx += 1;
            }
        } else if s[idx] == '0' {
            idx += 1;
        } else {
            return Err(Stop::Iter(start));
        }
        if idx + 1 < len && s[idx] == '.' && s[idx + 1].is_ascii_digit() {
            idx += 2;
            while idx < len && s[idx].is_ascii_digit() {
                idx += 1;
            }
        }
        if idx + 1 < len && (s[idx] == 'e' || s[idx] == 'E') {
            let e_start = idx;
            idx += 1;
            if idx + 1 < len && (s[idx] == '-' || s[idx] == '+') {
                idx += 1;
            }
            let digits = idx;
            while idx < len && s[idx].is_ascii_digit() {
                idx += 1;
            }
            if idx == digits {
                idx = e_start;
            }
        }
        Ok(idx)
    }

    /// `_parse_object_unicode` (3.12: no "Illegal trailing comma" message yet).
    fn object(&mut self, start: usize) -> Result<usize, Fail> {
        let len = self.len();
        let mut idx = self.ws(start);
        if idx < len && self.s[idx] == '}' {
            return Ok(idx + 1);
        }
        loop {
            if idx >= len || self.s[idx] != '"' {
                return Err(Fail::At("Expecting property name enclosed in double quotes", idx));
            }
            idx = self.scanstring(idx + 1)?;
            idx = self.ws(idx);
            if idx >= len || self.s[idx] != ':' {
                return Err(Fail::At("Expecting ':' delimiter", idx));
            }
            idx = self.ws(idx + 1);
            idx = match self.scan_once(idx) {
                Ok(i) => i,
                Err(Stop::Iter(i)) => return Err(Fail::At("Expecting value", i)),
                Err(Stop::Fail(f)) => return Err(f),
            };
            idx = self.ws(idx);
            if idx < len && self.s[idx] == '}' {
                return Ok(idx + 1);
            }
            if idx >= len || self.s[idx] != ',' {
                return Err(Fail::At("Expecting ',' delimiter", idx));
            }
            idx = self.ws(idx + 1);
        }
    }

    /// `_parse_array_unicode`
    fn array(&mut self, start: usize) -> Result<usize, Fail> {
        let len = self.len();
        let mut idx = self.ws(start);
        if idx < len && self.s[idx] == ']' {
            return Ok(idx + 1);
        }
        loop {
            idx = match self.scan_once(idx) {
                Ok(i) => i,
                Err(Stop::Iter(i)) => return Err(Fail::At("Expecting value", i)),
                Err(Stop::Fail(f)) => return Err(f),
            };
            idx = self.ws(idx);
            if idx < len && self.s[idx] == ']' {
                return Ok(idx + 1);
            }
            if idx >= len || self.s[idx] != ',' {
                return Err(Fail::At("Expecting ',' delimiter", idx));
            }
            idx = self.ws(idx + 1);
        }
    }

    /// `scanstring_unicode` (strict): `end` is the index after the opening quote.
    fn scanstring(&self, end: usize) -> Result<usize, Fail> {
        let s = self.s;
        let len = s.len();
        let begin = end - 1;
        let mut next = end;
        loop {
            while next < len {
                let c = s[next];
                if c == '"' || c == '\\' {
                    break;
                }
                if (c as u32) <= 0x1f {
                    return Err(Fail::At("Invalid control character at", next));
                }
                next += 1;
            }
            if next >= len {
                return Err(Fail::At("Unterminated string starting at", begin));
            }
            if s[next] == '"' {
                return Ok(next + 1);
            }
            // backslash
            next += 1;
            if next >= len {
                return Err(Fail::At("Unterminated string starting at", begin));
            }
            let c = s[next];
            if c != 'u' {
                if !matches!(c, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                    return Err(Fail::At("Invalid \\escape", next - 1));
                }
                next += 1;
                continue;
            }
            // \uXXXX: C `end = next + 4; if (end >= len)` - an error also when the string ends there
            next += 1;
            let hex_end = next + 4;
            if hex_end >= len {
                return Err(Fail::At("Invalid \\uXXXX escape", next - 1));
            }
            let Some(cp) = hex4(&s[next..hex_end]) else {
                return Err(Fail::At("Invalid \\uXXXX escape", next - 1));
            };
            next = hex_end;
            // surrogate pair (C: `end + 6 < len` and `\u` follows): the second escape is read and
            // only kept when it is a low surrogate; invalid hex there is an error.
            if (0xd800..=0xdbff).contains(&cp) && next + 6 < len && s[next] == '\\' && s[next + 1] == 'u' {
                let Some(lo) = hex4(&s[next + 2..next + 6]) else {
                    return Err(Fail::At("Invalid \\uXXXX escape", next + 1));
                };
                if (0xdc00..=0xdfff).contains(&lo) {
                    next += 6;
                }
            }
        }
    }
}

enum Stop {
    /// `StopIteration(idx)` - "Expecting value" at the caller.
    Iter(usize),
    Fail(Fail),
}

fn hex4(cs: &[char]) -> Option<u32> {
    let mut v = 0u32;
    for c in cs {
        v = v * 16 + c.to_digit(16)?;
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::decode_error;

    /// Golden vectors from CPython 3.12.10 (reference venv):
    /// `json.loads(s, parse_constant=strict_constant)` -> `str(exc)`; see
    /// tests/parity/pl-core/pyjson_vectors.py.
    const VECTORS: &[(&str, Option<&str>)] = &include!("pyjson_vectors.in");

    #[test]
    fn matches_cpython() {
        for (input, want) in VECTORS {
            assert_eq!(decode_error(input.as_bytes()).as_deref(), *want, "input {input:?}");
        }
    }

    #[test]
    fn invalid_utf8() {
        assert_eq!(
            decode_error(b"{\"a\": \"\xff\"}").as_deref(),
            Some("'utf-8' codec can't decode byte 0xff in position 7: invalid start byte")
        );
    }
}
