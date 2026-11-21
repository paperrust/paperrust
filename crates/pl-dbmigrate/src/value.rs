//! Type conversion source → PostgreSQL and canonical representation for checksums.
//!
//! Every value is brought into a *canonical text form* that
//! (a) is directly usable as PostgreSQL input (COPY text format) and
//! (b) can be reproduced identically from the target (`col::text`).
//! Checksum source = hash over the canonical values before writing,
//! checksum target = hash over the values read back from PostgreSQL, canonicalized again.
//!
//! Django storage formats (upstream Django 5.x, `django/db/backends/*/operations.py`):
//! - SQLite: bool = INTEGER 0/1, DateTimeField = TEXT `YYYY-MM-DD HH:MM:SS[.ffffff]` naive in UTC
//!   (USE_TZ=True), DateField = TEXT `YYYY-MM-DD`, JSONField = TEXT, UUIDField = TEXT 32 hex,
//!   GenericIPAddressField = TEXT, FloatField = REAL.
//! - MySQL/MariaDB: bool = tinyint(1), DATETIME(6) naive in UTC, JSON (MariaDB: LONGTEXT with
//!   json_valid), UUID = char(32).

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde_json::Value as Json;
use std::fmt::Write as _;

/// Column kind in the target (derived from `udt_name` of the Django schema on PostgreSQL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Int,
    Float,
    Bool,
    /// varchar(n) / text; `max` = character limit
    Text {
        max: Option<usize>,
    },
    /// timestamp with time zone (Django USE_TZ=True)
    Ts,
    /// timestamp without time zone
    TsNaive,
    Date,
    Time,
    Json,
    Uuid,
    Bytea,
    Numeric,
    Inet,
}

impl Kind {
    pub fn from_udt(udt: &str, max: Option<usize>) -> Option<Kind> {
        Some(match udt {
            "int2" | "int4" | "int8" => Kind::Int,
            "float4" | "float8" => Kind::Float,
            "bool" => Kind::Bool,
            "varchar" | "text" | "bpchar" => Kind::Text { max },
            "timestamptz" => Kind::Ts,
            "timestamp" => Kind::TsNaive,
            "date" => Kind::Date,
            "time" => Kind::Time,
            "json" | "jsonb" => Kind::Json,
            "uuid" => Kind::Uuid,
            "bytea" => Kind::Bytea,
            "numeric" => Kind::Numeric,
            "inet" => Kind::Inet,
            _ => return None,
        })
    }
}

/// Raw value from the source.
#[derive(Debug, Clone)]
pub enum Raw {
    Null,
    Int(i64),
    Real(f64),
    /// TEXT (bytes, because SQLite can also store invalid UTF-8)
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

/// What to do with NUL characters (U+0000)? PostgreSQL allows them neither in `text` nor in `jsonb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NulPolicy {
    /// Abort and report all affected cells (default).
    Fail,
    /// Replace U+0000 with U+FFFD (every cell is listed in the report).
    Replace,
    /// Remove U+0000 (every cell is listed in the report).
    Strip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Utf8Policy {
    Fail,
    /// Replace invalid byte sequences with U+FFFD (reported).
    Replace,
}

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub nul: NulPolicy,
    pub utf8: Utf8Policy,
}

/// Result of a conversion.
#[derive(Debug)]
pub enum Conv {
    /// Value can be adopted unchanged (canonical text form, `None` = NULL)
    Ok(Option<String>),
    /// Value was changed according to policy – must be reported
    Fixed(Option<String>, String),
    /// Value cannot be adopted
    Err(String),
}

pub fn convert(kind: Kind, raw: &Raw, policy: Policy) -> Conv {
    if let Raw::Null = raw {
        return Conv::Ok(None);
    }
    match kind {
        Kind::Int => match raw {
            Raw::Int(i) => Conv::Ok(Some(i.to_string())),
            Raw::Real(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Conv::Ok(Some((*f as i64).to_string())),
            Raw::Text(b) => match std::str::from_utf8(b).ok().and_then(|s| s.trim().parse::<i64>().ok()) {
                Some(i) => Conv::Ok(Some(i.to_string())),
                None => Conv::Err(format!("keine Ganzzahl: {}", preview_bytes(b))),
            },
            other => Conv::Err(format!("keine Ganzzahl: {}", preview_raw(other))),
        },
        Kind::Float => match raw {
            Raw::Real(f) => Conv::Ok(Some(fmt_float(*f))),
            Raw::Int(i) => Conv::Ok(Some(fmt_float(*i as f64))),
            Raw::Text(b) => match std::str::from_utf8(b).ok().and_then(parse_float) {
                Some(f) => Conv::Ok(Some(fmt_float(f))),
                None => Conv::Err(format!("keine Gleitkommazahl: {}", preview_bytes(b))),
            },
            other => Conv::Err(format!("keine Gleitkommazahl: {}", preview_raw(other))),
        },
        Kind::Bool => {
            let v = match raw {
                Raw::Int(0) => Some(false),
                Raw::Int(1) => Some(true),
                Raw::Text(b) => match std::str::from_utf8(b).unwrap_or("").trim() {
                    "0" | "f" | "false" | "FALSE" | "False" => Some(false),
                    "1" | "t" | "true" | "TRUE" | "True" => Some(true),
                    _ => None,
                },
                _ => None,
            };
            match v {
                Some(true) => Conv::Ok(Some("t".into())),
                Some(false) => Conv::Ok(Some("f".into())),
                None => Conv::Err(format!("kein Wahrheitswert: {}", preview_raw(raw))),
            }
        }
        Kind::Text { max } => {
            let (s, mut fixes) = match text_of(raw, policy) {
                Ok(x) => x,
                Err(e) => return Conv::Err(e),
            };
            let (s, nulfix) = match apply_nul(s, policy.nul) {
                Ok(x) => x,
                Err(e) => return Conv::Err(e),
            };
            if let Some(f) = nulfix {
                fixes.push(f);
            }
            if let Some(m) = max {
                let n = s.chars().count();
                if n > m {
                    return Conv::Err(format!("Text zu lang für varchar({m}): {n} Zeichen"));
                }
            }
            finish(Some(s), fixes)
        }
        Kind::Ts | Kind::TsNaive => match ts_of(raw) {
            Ok(dt) => Conv::Ok(Some(fmt_ts(&dt, kind == Kind::Ts))),
            Err(e) => Conv::Err(e),
        },
        Kind::Date => match raw {
            Raw::Text(b) => match std::str::from_utf8(b).ok().and_then(parse_date) {
                Some(d) => Conv::Ok(Some(d.format("%Y-%m-%d").to_string())),
                None => Conv::Err(format!("kein Datum (YYYY-MM-DD): {}", preview_bytes(b))),
            },
            other => Conv::Err(format!("kein Datum: {}", preview_raw(other))),
        },
        Kind::Time => match raw {
            Raw::Text(b) => match std::str::from_utf8(b).ok().and_then(parse_time) {
                Some(t) => Conv::Ok(Some(fmt_time(&t))),
                None => Conv::Err(format!("keine Uhrzeit: {}", preview_bytes(b))),
            },
            other => Conv::Err(format!("keine Uhrzeit: {}", preview_raw(other))),
        },
        Kind::Json => {
            let (s, mut fixes) = match text_of(raw, policy) {
                Ok(x) => x,
                Err(e) => return Conv::Err(e),
            };
            let mut v: Json = match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(e) => return Conv::Err(format!("ungültiges JSON ({e}): {}", preview_str(&s))),
            };
            let mut n = 0usize;
            count_json_nul(&v, &mut n);
            if n > 0 {
                match policy.nul {
                    NulPolicy::Fail => {
                        return Conv::Err(format!("JSON enthält {n} Zeichenkette(n) mit U+0000 (PostgreSQL jsonb verbietet \\u0000)"));
                    }
                    p => {
                        fix_json_nul(&mut v, p);
                        fixes.push(format!("U+0000 in JSON {}", nul_action(p)));
                    }
                }
            }
            finish(Some(canon_json(&v)), fixes)
        }
        Kind::Uuid => {
            let s = match raw {
                Raw::Text(b) => String::from_utf8_lossy(b).into_owned(),
                Raw::Blob(b) if b.len() == 16 => hex(b),
                other => return Conv::Err(format!("keine UUID: {}", preview_raw(other))),
            };
            match canon_uuid(&s) {
                Some(u) => Conv::Ok(Some(u)),
                None => Conv::Err(format!("keine UUID: {}", preview_str(&s))),
            }
        }
        Kind::Bytea => match raw {
            Raw::Blob(b) => Conv::Ok(Some(format!("\\x{}", hex(b)))),
            // MySQL path returns HEX(col), PostgreSQL path `\x…` as text
            Raw::Text(b) => match unhex(std::str::from_utf8(b).unwrap_or("").trim_start_matches("\\x")) {
                Some(bytes) => Conv::Ok(Some(format!("\\x{}", hex(&bytes)))),
                None => Conv::Ok(Some(format!("\\x{}", hex(b)))),
            },
            other => Conv::Err(format!("keine Binärdaten: {}", preview_raw(other))),
        },
        Kind::Numeric => {
            let s = match raw {
                Raw::Int(i) => i.to_string(),
                Raw::Real(f) => fmt_float(*f),
                Raw::Text(b) => String::from_utf8_lossy(b).trim().to_string(),
                other => return Conv::Err(format!("keine Zahl: {}", preview_raw(other))),
            };
            match canon_numeric(&s) {
                Some(n) => Conv::Ok(Some(n)),
                None => Conv::Err(format!("keine Dezimalzahl: {}", preview_str(&s))),
            }
        }
        Kind::Inet => match raw {
            Raw::Text(b) => {
                let s = String::from_utf8_lossy(b);
                match canon_inet(&s) {
                    Some(v) => Conv::Ok(Some(v)),
                    None => Conv::Err(format!("keine IP-Adresse: {}", preview_str(&s))),
                }
            }
            other => Conv::Err(format!("keine IP-Adresse: {}", preview_raw(other))),
        },
    }
}

fn finish(v: Option<String>, fixes: Vec<String>) -> Conv {
    if fixes.is_empty() { Conv::Ok(v) } else { Conv::Fixed(v, fixes.join("; ")) }
}

fn nul_action(p: NulPolicy) -> &'static str {
    match p {
        NulPolicy::Replace => "durch U+FFFD ersetzt",
        NulPolicy::Strip => "entfernt",
        NulPolicy::Fail => "",
    }
}

fn text_of(raw: &Raw, policy: Policy) -> Result<(String, Vec<String>), String> {
    match raw {
        Raw::Text(b) | Raw::Blob(b) => match String::from_utf8(b.clone()) {
            Ok(s) => Ok((s, vec![])),
            Err(_) => match policy.utf8 {
                Utf8Policy::Fail => Err(format!("ungültiges UTF-8: {}", preview_bytes(b))),
                Utf8Policy::Replace => {
                    Ok((String::from_utf8_lossy(b).into_owned(), vec!["ungültige UTF-8-Bytes durch U+FFFD ersetzt".into()]))
                }
            },
        },
        Raw::Int(i) => Ok((i.to_string(), vec![])),
        Raw::Real(f) => Ok((fmt_float(*f), vec![])),
        Raw::Null => Ok((String::new(), vec![])),
    }
}

fn apply_nul(s: String, p: NulPolicy) -> Result<(String, Option<String>), String> {
    if !s.contains('\0') {
        return Ok((s, None));
    }
    let n = s.matches('\0').count();
    match p {
        NulPolicy::Fail => Err(format!("Text enthält {n} NUL-Zeichen (U+0000), PostgreSQL erlaubt das nicht")),
        NulPolicy::Replace => Ok((s.replace('\0', "\u{FFFD}"), Some(format!("{n}× U+0000 {}", nul_action(p))))),
        NulPolicy::Strip => Ok((s.replace('\0', ""), Some(format!("{n}× U+0000 {}", nul_action(p))))),
    }
}

fn count_json_nul(v: &Json, n: &mut usize) {
    match v {
        Json::String(s) => {
            if s.contains('\0') {
                *n += 1
            }
        }
        Json::Array(a) => a.iter().for_each(|x| count_json_nul(x, n)),
        Json::Object(o) => {
            for (k, x) in o {
                if k.contains('\0') {
                    *n += 1;
                }
                count_json_nul(x, n);
            }
        }
        _ => {}
    }
}

fn fix_str(s: &str, p: NulPolicy) -> String {
    match p {
        NulPolicy::Strip => s.replace('\0', ""),
        _ => s.replace('\0', "\u{FFFD}"),
    }
}

fn fix_json_nul(v: &mut Json, p: NulPolicy) {
    match v {
        Json::String(s) => *s = fix_str(s, p),
        Json::Array(a) => a.iter_mut().for_each(|x| fix_json_nul(x, p)),
        Json::Object(o) => {
            let old = std::mem::take(o);
            for (k, mut x) in old {
                fix_json_nul(&mut x, p);
                o.insert(fix_str(&k, p), x);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Canonical forms
// ---------------------------------------------------------------------------

/// Shortest exact representation (Rust `Display` for f64 is round-trip exact),
/// special values in PostgreSQL notation.
pub fn fmt_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else {
        format!("{f}")
    }
}

fn parse_float(s: &str) -> Option<f64> {
    match s.trim() {
        "NaN" | "nan" => Some(f64::NAN),
        "Infinity" | "inf" | "+Infinity" => Some(f64::INFINITY),
        "-Infinity" | "-inf" => Some(f64::NEG_INFINITY),
        t => t.parse::<f64>().ok(),
    }
}

/// UTC timestamp with exactly 6 fractional digits.
pub fn fmt_ts(dt: &NaiveDateTime, with_tz: bool) -> String {
    let base = dt.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    if with_tz { format!("{base}+00") } else { base }
}

fn fmt_time(t: &NaiveTime) -> String {
    t.format("%H:%M:%S%.6f").to_string()
}

/// Timestamp from text. Without offset = UTC (Django USE_TZ=True stores naive UTC times on
/// SQLite/MySQL). With offset → converted to UTC.
pub fn parse_ts(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    let s = s.strip_suffix('Z').map(|x| format!("{x}+00:00")).unwrap_or_else(|| s.to_string());
    for f in ["%Y-%m-%d %H:%M:%S%.f%:z", "%Y-%m-%dT%H:%M:%S%.f%:z", "%Y-%m-%d %H:%M:%S%.f%#z", "%Y-%m-%dT%H:%M:%S%.f%#z"] {
        if let Ok(dt) = chrono::DateTime::parse_from_str(&s, f) {
            return Some(dt.naive_utc());
        }
    }
    for f in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&s, f) {
            return Some(dt);
        }
    }
    None
}

fn ts_of(raw: &Raw) -> Result<NaiveDateTime, String> {
    match raw {
        Raw::Text(b) => {
            let s = std::str::from_utf8(b).map_err(|_| format!("Zeitstempel kein UTF-8: {}", preview_bytes(b)))?;
            let dt = parse_ts(s).ok_or_else(|| format!("kein Zeitstempel: {}", preview_str(s)))?;
            // More than microseconds would be lost
            if !dt.and_utc().timestamp_subsec_nanos().is_multiple_of(1000) {
                return Err(format!("Zeitstempel genauer als Mikrosekunden: {}", preview_str(s)));
            }
            Ok(dt)
        }
        other => Err(format!("kein Zeitstempel (Typ): {}", preview_raw(other))),
    }
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn parse_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    NaiveTime::parse_from_str(s, "%H:%M:%S%.f").or_else(|_| NaiveTime::parse_from_str(s, "%H:%M")).ok()
}

/// JSON with recursively sorted keys, compact. jsonb reorders keys itself and
/// discards duplicates (last wins – like serde_json); therefore we compare only this form.
pub fn canon_json(v: &Json) -> String {
    let mut out = String::new();
    write_json(v, &mut out);
    out
}

fn write_json(v: &Json, out: &mut String) {
    match v {
        Json::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Json::String((*k).clone()).to_string());
                out.push(':');
                write_json(&o[*k], out);
            }
            out.push('}');
        }
        Json::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(x, out);
            }
            out.push(']');
        }
        Json::Number(n) => {
            // jsonb stores numbers as numeric: 1.0 stays "1.0", 1e2 becomes "100".
            // Comparison via f64 if not an integer.
            if n.is_i64() || n.is_u64() {
                out.push_str(&n.to_string());
            } else if let Some(f) = n.as_f64() {
                // jsonb (numeric) has no negative zero: -0.0 becomes 0.0 → compare as 0
                out.push_str(&fmt_float(if f == 0.0 { 0.0 } else { f }));
            } else {
                out.push_str(&n.to_string());
            }
        }
        other => out.push_str(&other.to_string()),
    }
}

fn canon_uuid(s: &str) -> Option<String> {
    let h: String = s.trim().chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
    if h.len() != 32 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]))
}

/// Decimal number without superfluous zeros (scale is only relevant for computed columns in the Django schema).
fn canon_numeric(s: &str) -> Option<String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("nan") {
        return Some("NaN".into());
    }
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if body.contains(['e', 'E']) {
        return body.parse::<f64>().ok().map(|f| fmt_float(if neg { -f } else { f }));
    }
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if int.is_empty() && frac.is_empty() || !int.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let mut out = String::new();
    if neg && !(int == "0" && frac.is_empty()) {
        out.push('-');
    }
    out.push_str(int);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    Some(out)
}

fn canon_inet(s: &str) -> Option<String> {
    let s = s.trim();
    let (addr, mask) = match s.split_once('/') {
        Some((a, m)) => (a, Some(m.parse::<u8>().ok()?)),
        None => (s, None),
    };
    let ip: std::net::IpAddr = addr.parse().ok()?;
    let full = if ip.is_ipv4() { 32 } else { 128 };
    match mask {
        Some(m) if m != full => Some(format!("{ip}/{m}")),
        _ => Some(ip.to_string()),
    }
}

/// Canonical form from the PostgreSQL text output (`col::text`, TimeZone=UTC, bytea_output=hex).
pub fn canon_from_pg(kind: Kind, s: &str) -> Result<String, String> {
    let bad = || format!("unerwarteter PostgreSQL-Wert für {kind:?}: {}", preview_str(s));
    Ok(match kind {
        Kind::Int => s.parse::<i64>().map_err(|_| bad())?.to_string(),
        Kind::Float => fmt_float(parse_float(s).ok_or_else(bad)?),
        Kind::Bool => match s {
            "t" | "true" => "t".into(),
            "f" | "false" => "f".into(),
            _ => return Err(bad()),
        },
        Kind::Text { .. } => s.to_string(),
        Kind::Ts | Kind::TsNaive => fmt_ts(&parse_ts(s).ok_or_else(bad)?, kind == Kind::Ts),
        Kind::Date => parse_date(s).ok_or_else(bad)?.format("%Y-%m-%d").to_string(),
        Kind::Time => fmt_time(&parse_time(s).ok_or_else(bad)?),
        Kind::Json => canon_json(&serde_json::from_str::<Json>(s).map_err(|_| bad())?),
        Kind::Uuid => canon_uuid(s).ok_or_else(bad)?,
        Kind::Bytea => s.to_ascii_lowercase(),
        Kind::Numeric => canon_numeric(s).ok_or_else(bad)?,
        Kind::Inet => canon_inet(s).ok_or_else(bad)?,
    })
}

// ---------------------------------------------------------------------------
// COPY text format
// ---------------------------------------------------------------------------

/// Appends a value in PostgreSQL COPY text format (`\N` = NULL).
pub fn copy_escape(out: &mut Vec<u8>, v: Option<&str>) {
    match v {
        None => out.extend_from_slice(b"\\N"),
        Some(s) => {
            for &b in s.as_bytes() {
                match b {
                    b'\\' => out.extend_from_slice(b"\\\\"),
                    b'\n' => out.extend_from_slice(b"\\n"),
                    b'\r' => out.extend_from_slice(b"\\r"),
                    b'\t' => out.extend_from_slice(b"\\t"),
                    _ => out.push(b),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn preview_str(s: &str) -> String {
    let mut p: String = s.chars().take(60).map(|c| if c == '\0' { '␀' } else { c }).collect();
    if s.chars().count() > 60 {
        p.push('…');
    }
    format!("{p:?}")
}

fn preview_bytes(b: &[u8]) -> String {
    preview_str(&String::from_utf8_lossy(&b[..b.len().min(240)]))
}

fn preview_raw(r: &Raw) -> String {
    match r {
        Raw::Null => "NULL".into(),
        Raw::Int(i) => format!("INTEGER {i}"),
        Raw::Real(f) => format!("REAL {f}"),
        Raw::Text(b) => format!("TEXT {}", preview_bytes(b)),
        Raw::Blob(b) => format!("BLOB ({} Bytes)", b.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Policy = Policy { nul: NulPolicy::Fail, utf8: Utf8Policy::Fail };

    fn ok(kind: Kind, raw: Raw) -> Option<String> {
        match convert(kind, &raw, P) {
            Conv::Ok(v) => v,
            other => panic!("{other:?}"),
        }
    }

    fn t(s: &str) -> Raw {
        Raw::Text(s.as_bytes().to_vec())
    }

    #[test]
    fn sqlite_datetimes() {
        assert_eq!(ok(Kind::Ts, t("2025-01-06 08:00:00")).unwrap(), "2025-01-06 08:00:00.000000+00");
        assert_eq!(ok(Kind::Ts, t("2025-01-06 08:00:00.5")).unwrap(), "2025-01-06 08:00:00.500000+00");
        assert_eq!(ok(Kind::Ts, t("2025-01-06T08:00:00.123456Z")).unwrap(), "2025-01-06 08:00:00.123456+00");
        assert_eq!(ok(Kind::Ts, t("2025-01-06 09:00:00+01:00")).unwrap(), "2025-01-06 08:00:00.000000+00");
        // PostgreSQL output
        assert_eq!(canon_from_pg(Kind::Ts, "2025-01-06 08:00:00+00").unwrap(), "2025-01-06 08:00:00.000000+00");
        assert_eq!(canon_from_pg(Kind::Ts, "2025-01-06 08:00:00.12+00").unwrap(), "2025-01-06 08:00:00.120000+00");
        assert!(matches!(convert(Kind::Ts, &t("gestern"), P), Conv::Err(_)));
        assert!(matches!(convert(Kind::Ts, &t("2025-01-06 08:00:00.1234567"), P), Conv::Err(_)));
    }

    #[test]
    fn bools_ints_floats() {
        assert_eq!(ok(Kind::Bool, Raw::Int(1)).unwrap(), "t");
        assert_eq!(ok(Kind::Bool, Raw::Int(0)).unwrap(), "f");
        assert_eq!(ok(Kind::Bool, t("1")).unwrap(), "t");
        assert!(matches!(convert(Kind::Bool, &Raw::Int(2), P), Conv::Err(_)));
        assert_eq!(ok(Kind::Int, t("42")).unwrap(), "42");
        assert_eq!(ok(Kind::Float, Raw::Real(0.1)).unwrap(), "0.1");
        assert_eq!(canon_from_pg(Kind::Float, "0.1").unwrap(), "0.1");
        assert_eq!(ok(Kind::Float, Raw::Real(1e300)).unwrap(), fmt_float(1e300));
        assert_eq!(canon_from_pg(Kind::Float, "1e+300").unwrap(), fmt_float(1e300));
        assert_eq!(ok(Kind::Float, Raw::Int(3)).unwrap(), canon_from_pg(Kind::Float, "3").unwrap());
    }

    #[test]
    fn text_nul_and_utf8() {
        assert!(matches!(convert(Kind::Text { max: None }, &t("a\0b"), P), Conv::Err(_)));
        let p = Policy { nul: NulPolicy::Replace, utf8: Utf8Policy::Fail };
        match convert(Kind::Text { max: None }, &t("a\0b"), p) {
            Conv::Fixed(Some(v), _) => assert_eq!(v, "a\u{FFFD}b"),
            o => panic!("{o:?}"),
        }
        let bad = Raw::Text(vec![b'a', 0xff, b'b']);
        assert!(matches!(convert(Kind::Text { max: None }, &bad, P), Conv::Err(_)));
        let p = Policy { nul: NulPolicy::Fail, utf8: Utf8Policy::Replace };
        assert!(matches!(convert(Kind::Text { max: None }, &bad, p), Conv::Fixed(..)));
        assert!(matches!(convert(Kind::Text { max: Some(3) }, &t("äöüß"), P), Conv::Err(_)));
        assert_eq!(ok(Kind::Text { max: Some(4) }, t("äöüß")).unwrap(), "äöüß");
        assert_eq!(ok(Kind::Text { max: None }, t("😀 漢字")).unwrap(), "😀 漢字");
    }

    #[test]
    fn json() {
        assert_eq!(ok(Kind::Json, t(r#"{"b": 1, "a": [1.5, "x"]}"#)).unwrap(), r#"{"a":[1.5,"x"],"b":1}"#);
        assert_eq!(canon_from_pg(Kind::Json, r#"{"a": [1.5, "x"], "b": 1}"#).unwrap(), r#"{"a":[1.5,"x"],"b":1}"#);
        assert!(matches!(convert(Kind::Json, &t(r#"{"a": "\u0000"}"#), P), Conv::Err(_)));
        assert!(matches!(convert(Kind::Json, &t("NaN"), P), Conv::Err(_)));
        assert_eq!(
            ok(Kind::Json, t("[-0.0, 1e2, 100000000000000000000]")).unwrap(),
            canon_from_pg(Kind::Json, "[0.0, 100, 100000000000000000000]").unwrap()
        );
        let p = Policy { nul: NulPolicy::Strip, utf8: Utf8Policy::Fail };
        match convert(Kind::Json, &t(r#"{"a\u0000": "x\u0000"}"#), p) {
            Conv::Fixed(Some(v), _) => assert_eq!(v, r#"{"a":"x"}"#),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn misc_types() {
        assert_eq!(ok(Kind::Uuid, t("0123456789abcdef0123456789ABCDEF")).unwrap(), "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(ok(Kind::Inet, t("127.0.0.1")).unwrap(), "127.0.0.1");
        assert_eq!(canon_from_pg(Kind::Inet, "127.0.0.1/32").unwrap(), "127.0.0.1");
        assert_eq!(canon_from_pg(Kind::Inet, "::1/128").unwrap(), "::1");
        assert_eq!(ok(Kind::Numeric, t("12.50")).unwrap(), "12.5");
        assert_eq!(ok(Kind::Bytea, Raw::Blob(vec![0, 255])).unwrap(), "\\x00ff");
        assert_eq!(ok(Kind::Date, t("2025-01-06")).unwrap(), "2025-01-06");
        assert!(matches!(convert(Kind::Date, &t("2025-01-06 10:00:00"), P), Conv::Err(_)));
    }

    #[test]
    fn copy_format() {
        let mut v = Vec::new();
        copy_escape(&mut v, Some("a\\b\tc\nd\re"));
        assert_eq!(v, b"a\\\\b\\tc\\nd\\re");
        let mut v = Vec::new();
        copy_escape(&mut v, None);
        assert_eq!(v, b"\\N");
    }
}
