//! Python/Django-compatible value parsers and representations.
//!
//! Upstream relies on Python built-ins in many places (`int()`, `float()`,
//! `Decimal()`, `str.strip()`, `repr()`, `json.loads`). So that error cases and limits
//! stay identical, the relevant parts are re-implemented here.
//!
//! Deliberate restriction (see contract, Known differences): Python also accepts non-ASCII
//! digits in `int()`/`float()`/`Decimal()` (e.g. Arabic-Indic digits);
//! here only ASCII digits are accepted.

use std::fmt::Write as _;

/// Python `str.isspace()` (Unicode White_Space plus the control characters 0x1C..0x1F).
pub fn py_is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `str.strip()` without argument.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_is_space)
}

/// Python integer (arbitrarily large). Values outside i128 are kept as `Big` –
/// for all database comparisons the sign is then enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyInt {
    Small(i128),
    Big { negative: bool },
}

impl PyInt {
    pub fn as_i64(self) -> Option<i64> {
        match self {
            PyInt::Small(v) => i64::try_from(v).ok(),
            PyInt::Big { .. } => None,
        }
    }

    pub fn is_negative(self) -> bool {
        match self {
            PyInt::Small(v) => v < 0,
            PyInt::Big { negative } => negative,
        }
    }

    /// Clamps the value into a range `[min, max]` (Django `IntegerFieldOverflow`).
    pub fn range_check(self, min: i64, max: i64) -> RangeCheck {
        match self {
            PyInt::Small(v) if v < min as i128 => RangeCheck::Underflow,
            PyInt::Small(v) if v > max as i128 => RangeCheck::Overflow,
            PyInt::Small(v) => RangeCheck::InRange(v as i64),
            PyInt::Big { negative: true } => RangeCheck::Underflow,
            PyInt::Big { negative: false } => RangeCheck::Overflow,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeCheck {
    InRange(i64),
    Underflow,
    Overflow,
}

/// Digit sequence with Python underscore rules (PEP 515): underscores only singly between
/// digits. Returns the digits without underscores.
fn digits_with_underscores(s: &str) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_digit = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            out.push(c);
            prev_digit = true;
        } else if c == '_' && prev_digit {
            prev_digit = false;
        } else {
            return None;
        }
    }
    if !prev_digit {
        return None;
    }
    Some(out)
}

fn digits_to_pyint(negative: bool, digits: &str) -> PyInt {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        return PyInt::Small(0);
    }
    if trimmed.len() > 38 {
        return PyInt::Big { negative };
    }
    match trimmed.parse::<i128>() {
        Ok(v) => PyInt::Small(if negative { -v } else { v }),
        Err(_) => PyInt::Big { negative },
    }
}

/// Python `int(s)` for strings (base 10). `None` corresponds to `ValueError`.
pub fn py_int(s: &str) -> Option<PyInt> {
    let s = py_strip(s);
    let (negative, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits = digits_with_underscores(rest)?;
    Some(digits_to_pyint(negative, &digits))
}

/// Python `decimal.Decimal(s)` for strings.
#[derive(Debug, Clone, PartialEq)]
pub enum PyDecimal {
    Finite {
        negative: bool,
        /// Digits without leading zeros ("0" for zero).
        digits: String,
        /// value = digits × 10^exp
        exp: i64,
    },
    Infinite {
        negative: bool,
    },
    NaN,
}

impl PyDecimal {
    pub fn parse(s: &str) -> Option<PyDecimal> {
        let s = py_strip(s);
        let (negative, rest) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        let lower = rest.to_ascii_lowercase();
        if lower == "inf" || lower == "infinity" {
            return Some(PyDecimal::Infinite { negative });
        }
        for nan in ["nan", "snan"] {
            if let Some(payload) = lower.strip_prefix(nan)
                && (payload.is_empty() || digits_with_underscores(payload).is_some()) {
                    return Some(PyDecimal::NaN);
                }
        }
        let (mantissa, exp_part) = match rest.find(['e', 'E']) {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let (int_part, frac_part) = match mantissa.find('.') {
            Some(i) => (&mantissa[..i], &mantissa[i + 1..]),
            None => (mantissa, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return None;
        }
        let int_digits = if int_part.is_empty() {
            String::new()
        } else {
            digits_with_underscores(int_part)?
        };
        let frac_digits = if frac_part.is_empty() {
            String::new()
        } else {
            digits_with_underscores(frac_part)?
        };
        let mut exp: i64 = 0;
        if let Some(e) = exp_part {
            let (eneg, edigits) = match e.as_bytes().first() {
                Some(b'-') => (true, &e[1..]),
                Some(b'+') => (false, &e[1..]),
                _ => (false, e),
            };
            let ed = digits_with_underscores(edigits)?;
            let ed = ed.trim_start_matches('0');
            // Exponents beyond 10^15 are practically infinitely large or small.
            let v: i64 = if ed.len() > 15 {
                1_000_000_000_000_000
            } else if ed.is_empty() {
                0
            } else {
                ed.parse().ok()?
            };
            exp = if eneg { -v } else { v };
        }
        let mut all = int_digits;
        all.push_str(&frac_digits);
        exp -= frac_digits.len() as i64;
        let trimmed = all.trim_start_matches('0');
        let digits = if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        };
        Some(PyDecimal::Finite {
            negative,
            digits,
            exp,
        })
    }

    pub fn is_finite(&self) -> bool {
        matches!(self, PyDecimal::Finite { .. })
    }

    /// `int(Decimal)` – truncation towards zero. Only for finite values.
    pub fn trunc(&self) -> Option<PyInt> {
        let PyDecimal::Finite {
            negative,
            digits,
            exp,
        } = self
        else {
            return None;
        };
        if digits == "0" {
            return Some(PyInt::Small(0));
        }
        let len = digits.len() as i64;
        if *exp >= 0 {
            if len + exp > 39 {
                return Some(PyInt::Big {
                    negative: *negative,
                });
            }
            let mut s = digits.clone();
            for _ in 0..*exp {
                s.push('0');
            }
            Some(digits_to_pyint(*negative, &s))
        } else {
            let keep = len + exp;
            if keep <= 0 {
                return Some(PyInt::Small(0));
            }
            Some(digits_to_pyint(*negative, &digits[..keep as usize]))
        }
    }

    /// Comparison `self > threshold` for a non-negative integer in decimal notation.
    pub fn gt_integer(&self, threshold: &str) -> bool {
        match self {
            PyDecimal::NaN => false,
            PyDecimal::Infinite { negative } => !negative,
            PyDecimal::Finite {
                negative,
                digits,
                exp,
            } => {
                if *negative || digits == "0" {
                    return false;
                }
                let len = digits.len() as i64;
                let int_len = len + exp;
                let tlen = threshold.len() as i64;
                if int_len > tlen {
                    return true;
                }
                if int_len < tlen {
                    return false;
                }
                // Same number of integer digits: compare digit by digit.
                let int_digits: String = if *exp >= 0 {
                    let mut s = digits.clone();
                    for _ in 0..*exp {
                        s.push('0');
                    }
                    s
                } else {
                    digits[..int_len as usize].to_string()
                };
                match int_digits.as_str().cmp(threshold) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => {
                        *exp < 0 && digits[int_len as usize..].bytes().any(|b| b != b'0')
                    }
                }
            }
        }
    }

    /// Representation for PostgreSQL `numeric` input (`CAST(... AS numeric)`).
    pub fn to_sql_numeric(&self) -> String {
        match self {
            PyDecimal::NaN => "NaN".into(),
            PyDecimal::Infinite { negative: true } => "-Infinity".into(),
            PyDecimal::Infinite { negative: false } => "Infinity".into(),
            PyDecimal::Finite {
                negative,
                digits,
                exp,
            } => format!("{}{}e{}", if *negative { "-" } else { "" }, digits, exp),
        }
    }
}

/// Limit of django-filter's `NumberFilter` (`MaxValueValidator(1e50)`), exactly as the
/// integer of the floating-point value 1e50.
pub const NUMBER_FILTER_MAX: &str = "100000000000000007629769841091887003294964970946560";

/// Python `float(s)` for strings. `None` corresponds to `ValueError`.
pub fn py_float(s: &str) -> Option<f64> {
    let s = py_strip(s);
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let lower = rest.to_ascii_lowercase();
    let v = if lower == "inf" || lower == "infinity" {
        f64::INFINITY
    } else if lower == "nan" {
        f64::NAN
    } else {
        // Same grammar as Decimal (without Inf/NaN), then Rust parser without underscores.
        match PyDecimal::parse(rest)? {
            PyDecimal::Finite { .. } => {}
            _ => return None,
        }
        if rest.starts_with(['+', '-']) {
            return None;
        }
        let cleaned: String = rest.chars().filter(|c| *c != '_').collect();
        cleaned.parse::<f64>().ok()?
    };
    Some(if neg { -v } else { v })
}

/// Python `repr(float)` / `str(float)`.
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    let s = format!("{:e}", f);
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if (-4..16).contains(&exp) {
        if exp >= 0 {
            let e = exp as usize;
            if digits.len() <= e + 1 {
                out.push_str(&digits);
                for _ in digits.len()..=e {
                    out.push('0');
                }
                out.push_str(".0");
            } else {
                out.push_str(&digits[..=e]);
                out.push('.');
                out.push_str(&digits[e + 1..]);
            }
        } else {
            out.push_str("0.");
            for _ in 0..(-exp - 1) {
                out.push('0');
            }
            out.push_str(&digits);
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let _ = write!(
            out,
            "e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.unsigned_abs()
        );
    }
    out
}

fn py_is_printable(c: char) -> bool {
    let cp = c as u32;
    if cp < 0x20 || (0x7f..=0xa0).contains(&cp) || cp == 0xad {
        return false;
    }
    !matches!(
        cp,
        0x2028 | 0x2029 | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064 | 0xfeff | 0xfff9..=0xfffb
    ) && !(0xe000..=0xf8ff).contains(&cp)
        && !(0xf0000..=0x10ffff).contains(&cp)
        && !(0x1680 == cp || (0x2000..=0x200a).contains(&cp) || cp == 0x202f || cp == 0x205f || cp == 0x3000)
}

/// Python `repr(str)`.
pub fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
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
            c if py_is_printable(c) => out.push(c),
            c => {
                let cp = c as u32;
                if cp <= 0xff {
                    let _ = write!(out, "\\x{:02x}", cp);
                } else if cp <= 0xffff {
                    let _ = write!(out, "\\u{:04x}", cp);
                } else {
                    let _ = write!(out, "\\U{:08x}", cp);
                }
            }
        }
    }
    out.push(quote);
    out
}

/// Result of Python's `json.loads` (with arbitrarily large integers and NaN/Infinity).
#[derive(Debug, Clone, PartialEq)]
pub enum PyJson {
    Null,
    Bool(bool),
    /// Integer; `digits` normalized (no leading zeros, "0" for zero).
    Int { negative: bool, digits: String },
    Float(f64),
    Str(String),
    List(Vec<PyJson>),
    Dict(Vec<(String, PyJson)>),
}

impl PyJson {
    pub fn int(v: i64) -> PyJson {
        PyJson::Int {
            negative: v < 0,
            digits: v.unsigned_abs().to_string(),
        }
    }

    /// `isinstance(x, int)` (bool counts as int in Python).
    pub fn as_pyint(&self) -> Option<PyInt> {
        match self {
            PyJson::Bool(b) => Some(PyInt::Small(*b as i128)),
            PyJson::Int { negative, digits } => Some(digits_to_pyint(*negative, digits)),
            _ => None,
        }
    }

    /// Python-Typname (`type(x).__name__`).
    pub fn type_name(&self) -> &'static str {
        match self {
            PyJson::Null => "NoneType",
            PyJson::Bool(_) => "bool",
            PyJson::Int { .. } => "int",
            PyJson::Float(_) => "float",
            PyJson::Str(_) => "str",
            PyJson::List(_) => "list",
            PyJson::Dict(_) => "dict",
        }
    }

    /// Python `str(x)`.
    pub fn py_str(&self) -> String {
        match self {
            PyJson::Str(s) => s.clone(),
            other => other.py_repr(),
        }
    }

    /// Python `repr(x)`.
    pub fn py_repr(&self) -> String {
        match self {
            PyJson::Null => "None".into(),
            PyJson::Bool(true) => "True".into(),
            PyJson::Bool(false) => "False".into(),
            PyJson::Int { negative, digits } => {
                if *negative {
                    format!("-{digits}")
                } else {
                    digits.clone()
                }
            }
            PyJson::Float(f) => py_float_repr(*f),
            PyJson::Str(s) => py_str_repr(s),
            PyJson::List(items) => {
                let parts: Vec<String> = items.iter().map(|i| i.py_repr()).collect();
                format!("[{}]", parts.join(", "))
            }
            PyJson::Dict(items) => {
                let parts: Vec<String> = items
                    .iter()
                    .map(|(k, v)| format!("{}: {}", py_str_repr(k), v.py_repr()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }

    /// Python equality with a scalar (for `x in {...}` checks). Lists/dicts are
    /// not hashable in Python; the caller handles that.
    pub fn is_hashable(&self) -> bool {
        !matches!(self, PyJson::List(_) | PyJson::Dict(_))
    }

    /// Numeric value as f64, if a number (bool/int/float).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            PyJson::Bool(b) => Some(*b as i64 as f64),
            PyJson::Int { negative, digits } => {
                let v: f64 = digits.parse().ok()?;
                Some(if *negative { -v } else { v })
            }
            PyJson::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Convert to `serde_json::Value` (for JSONB parameters).
    pub fn to_serde(&self) -> serde_json::Value {
        match self {
            PyJson::Null => serde_json::Value::Null,
            PyJson::Bool(b) => serde_json::Value::Bool(*b),
            PyJson::Int { negative, digits } => {
                let s = if *negative {
                    format!("-{digits}")
                } else {
                    digits.clone()
                };
                s.parse::<i64>()
                    .map(serde_json::Value::from)
                    .or_else(|_| s.parse::<u64>().map(serde_json::Value::from))
                    .unwrap_or_else(|_| {
                        serde_json::Number::from_f64(s.parse::<f64>().unwrap_or(0.0))
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null)
                    })
            }
            PyJson::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            PyJson::Str(s) => serde_json::Value::String(s.clone()),
            PyJson::List(items) => {
                serde_json::Value::Array(items.iter().map(|i| i.to_serde()).collect())
            }
            PyJson::Dict(items) => {
                let mut m = serde_json::Map::new();
                for (k, v) in items {
                    m.insert(k.clone(), v.to_serde());
                }
                serde_json::Value::Object(m)
            }
        }
    }

    /// Python `json.loads(s)` (strict). `None` corresponds to `JSONDecodeError`.
    pub fn loads(s: &str) -> Option<PyJson> {
        let mut p = JsonParser {
            s: s.as_bytes(),
            src: s,
            pos: 0,
            depth: 0,
        };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.pos != p.s.len() {
            return None;
        }
        Some(v)
    }
}

struct JsonParser<'a> {
    s: &'a [u8],
    src: &'a str,
    pos: usize,
    depth: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while let Some(b) = self.s.get(self.pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Option<PyJson> {
        // Python also limits recursion depth (RecursionError → 500); we
        // reject excessively deep documents as invalid JSON.
        if self.depth > 512 {
            return None;
        }
        match *self.s.get(self.pos)? {
            b'{' => {
                self.pos += 1;
                self.depth += 1;
                let mut items: Vec<(String, PyJson)> = Vec::new();
                self.ws();
                if self.eat("}") {
                    self.depth -= 1;
                    return Some(PyJson::Dict(items));
                }
                loop {
                    self.ws();
                    if self.s.get(self.pos) != Some(&b'"') {
                        return None;
                    }
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(":") {
                        return None;
                    }
                    self.ws();
                    let v = self.value()?;
                    if let Some(slot) = items.iter_mut().find(|(ek, _)| *ek == k) {
                        slot.1 = v;
                    } else {
                        items.push((k, v));
                    }
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("}") {
                        break;
                    }
                    return None;
                }
                self.depth -= 1;
                Some(PyJson::Dict(items))
            }
            b'[' => {
                self.pos += 1;
                self.depth += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat("]") {
                    self.depth -= 1;
                    return Some(PyJson::List(items));
                }
                loop {
                    self.ws();
                    items.push(self.value()?);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("]") {
                        break;
                    }
                    return None;
                }
                self.depth -= 1;
                Some(PyJson::List(items))
            }
            b'"' => Some(PyJson::Str(self.string()?)),
            b'n' if self.eat("null") => Some(PyJson::Null),
            b't' if self.eat("true") => Some(PyJson::Bool(true)),
            b'f' if self.eat("false") => Some(PyJson::Bool(false)),
            b'N' if self.eat("NaN") => Some(PyJson::Float(f64::NAN)),
            b'I' if self.eat("Infinity") => Some(PyJson::Float(f64::INFINITY)),
            b'-' if self.s[self.pos..].starts_with(b"-Infinity") => {
                self.pos += 9;
                Some(PyJson::Float(f64::NEG_INFINITY))
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn number(&mut self) -> Option<PyJson> {
        let start = self.pos;
        let negative = self.eat("-");
        let int_start = self.pos;
        match self.s.get(self.pos) {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.s.get(self.pos), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return None,
        }
        let int_end = self.pos;
        let mut is_float = false;
        if self.s.get(self.pos) == Some(&b'.')
            && matches!(self.s.get(self.pos + 1), Some(b'0'..=b'9'))
        {
            is_float = true;
            self.pos += 1;
            while matches!(self.s.get(self.pos), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.s.get(self.pos), Some(b'e' | b'E')) {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.s.get(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if matches!(self.s.get(self.pos), Some(b'0'..=b'9')) {
                is_float = true;
                while matches!(self.s.get(self.pos), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            } else {
                self.pos = save;
            }
        }
        if is_float {
            let txt = &self.src[start..self.pos];
            return Some(PyJson::Float(txt.parse::<f64>().ok()?));
        }
        let digits = self.src[int_start..int_end].to_string();
        let negative = negative && digits != "0";
        Some(PyJson::Int { negative, digits })
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.src.get(self.pos..self.pos + 4)?;
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.pos += 4;
        u32::from_str_radix(h, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        // expects '"' at self.pos
        self.pos += 1;
        let mut out = String::new();
        loop {
            let rest = self.src.get(self.pos..)?;
            let c = rest.chars().next()?;
            match c {
                '"' => {
                    self.pos += 1;
                    return Some(out);
                }
                '\\' => {
                    self.pos += 1;
                    let e = *self.s.get(self.pos)?;
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            if (0xd800..0xdc00).contains(&hi)
                                && self.s[self.pos..].starts_with(b"\\u")
                            {
                                let save = self.pos;
                                self.pos += 2;
                                let lo = self.hex4()?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    let cp = 0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00);
                                    out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                                } else {
                                    self.pos = save;
                                    out.push('\u{fffd}');
                                }
                            } else {
                                // Lone surrogates are allowed in Python, not in Rust.
                                out.push(char::from_u32(hi).unwrap_or('\u{fffd}'));
                            }
                        }
                        _ => return None,
                    }
                }
                c if (c as u32) < 0x20 => return None,
                c => {
                    out.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_parsing() {
        assert_eq!(py_int(" 42 "), Some(PyInt::Small(42)));
        assert_eq!(py_int("+7"), Some(PyInt::Small(7)));
        assert_eq!(py_int("-0"), Some(PyInt::Small(0)));
        assert_eq!(py_int("1_000"), Some(PyInt::Small(1000)));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("_1"), None);
        assert_eq!(py_int("1.0"), None);
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("007"), Some(PyInt::Small(7)));
        assert_eq!(
            py_int("99999999999999999999999999999999999999999999"),
            Some(PyInt::Big { negative: false })
        );
    }

    #[test]
    fn decimal_parsing() {
        let d = PyDecimal::parse(" 1.50 ").unwrap();
        assert_eq!(d.trunc(), Some(PyInt::Small(1)));
        assert_eq!(PyDecimal::parse("-1.9").unwrap().trunc(), Some(PyInt::Small(-1)));
        assert_eq!(PyDecimal::parse("1e3").unwrap().trunc(), Some(PyInt::Small(1000)));
        assert_eq!(PyDecimal::parse(".5").unwrap().trunc(), Some(PyInt::Small(0)));
        assert_eq!(PyDecimal::parse("5.").unwrap().trunc(), Some(PyInt::Small(5)));
        assert!(!PyDecimal::parse("NaN").unwrap().is_finite());
        assert!(!PyDecimal::parse("-Infinity").unwrap().is_finite());
        assert!(PyDecimal::parse("abc").is_none());
        assert!(PyDecimal::parse(".").is_none());
        assert!(PyDecimal::parse("1e").is_none());
        assert!(PyDecimal::parse("1e51").unwrap().gt_integer(NUMBER_FILTER_MAX));
        assert!(!PyDecimal::parse("1e50").unwrap().gt_integer(NUMBER_FILTER_MAX));
        assert!(!PyDecimal::parse("-1e60").unwrap().gt_integer(NUMBER_FILTER_MAX));
        assert_eq!(
            PyDecimal::parse("1e60").unwrap().trunc(),
            Some(PyInt::Big { negative: false })
        );
    }

    #[test]
    fn float_repr_matches_python() {
        assert_eq!(py_float_repr(1.0), "1.0");
        assert_eq!(py_float_repr(1.5), "1.5");
        assert_eq!(py_float_repr(1e16), "1e+16");
        assert_eq!(py_float_repr(1e15), "1000000000000000.0");
        assert_eq!(py_float_repr(0.0001), "0.0001");
        assert_eq!(py_float_repr(0.00001), "1e-05");
        assert_eq!(py_float_repr(-1e9), "-1000000000.0");
        assert_eq!(py_float_repr(1.2345678901234568e17), "1.2345678901234568e+17");
        assert_eq!(py_float_repr(270.0), "270.0");
    }

    #[test]
    fn str_repr_matches_python() {
        assert_eq!(py_str_repr("abc"), "'abc'");
        assert_eq!(py_str_repr("it's"), "\"it's\"");
        assert_eq!(py_str_repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(py_str_repr("a\nb"), "'a\\nb'");
        assert_eq!(py_str_repr("ä"), "'ä'");
        assert_eq!(py_str_repr("\u{1}"), "'\\x01'");
    }

    #[test]
    fn json_loads() {
        assert_eq!(PyJson::loads("not valid json"), None);
        assert_eq!(
            PyJson::loads(" [1, \"a\", true, null] ").unwrap().py_repr(),
            "[1, 'a', True, None]"
        );
        assert!(matches!(PyJson::loads("NaN"), Some(PyJson::Float(f)) if f.is_nan()));
        assert_eq!(
            PyJson::loads("123456789012345678901234567890").unwrap(),
            PyJson::Int {
                negative: false,
                digits: "123456789012345678901234567890".into()
            }
        );
        assert_eq!(PyJson::loads("1.0").unwrap(), PyJson::Float(1.0));
        assert_eq!(PyJson::loads("[1,]"), None);
        assert_eq!(PyJson::loads("\"a\u{1}\""), None);
        assert_eq!(PyJson::loads("1 2"), None);
        assert_eq!(
            PyJson::loads("{\"a\": 1, \"a\": 2}").unwrap().py_repr(),
            "{'a': 2}"
        );
        assert_eq!(
            PyJson::loads("\"\\ud83d\\ude00\"").unwrap(),
            PyJson::Str("😀".into())
        );
    }

    #[test]
    fn float_parsing() {
        assert_eq!(py_float(" 1.5 "), Some(1.5));
        assert_eq!(py_float("1_0.5"), Some(10.5));
        assert_eq!(py_float("-inf"), Some(f64::NEG_INFINITY));
        assert!(py_float("abc").is_none());
        assert!(py_float("--1").is_none());
    }
}
