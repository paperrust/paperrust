//! Date/time like DRF/Django.
//!
//! upstream:
//! * Output: rest_framework/fields.py@3.18.3 `DateTimeField.to_representation`
//!   (`enforce_timezone` → current time zone = `TIME_ZONE`, `isoformat()`, `+00:00` → `Z`),
//!   `DateField.to_representation` (`isoformat()`).
//! * Filter input: django/forms/fields.py@5.2 (`DateField`, `DateTimeField`,
//!   `BaseTemporalField.to_python`), django/forms/utils.py `from_current_timezone`,
//!   django_filters/fields.py@26.2 `IsoDateTimeField`, django/utils/dateparse.py `parse_datetime`,
//!   CPython 3.12 `datetime.fromisoformat` and `_strptime`.
//! * Serializer input: rest_framework/fields.py `DateTimeField/DateField.to_internal_value`.
//!
//! Parsers with `Result<_, ()>` emulate Python's `ValueError` (no further content needed).
#![allow(clippy::result_unit_err)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use regex::Regex;

use crate::locale_formats::LOCALE_INPUT_FORMATS;

// =============================================================================================
// Output
// =============================================================================================

fn fmt_offset(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    if s != 0 { format!("{sign}{h:02}:{m:02}:{s:02}") } else { format!("{sign}{h:02}:{m:02}") }
}

/// Python `datetime.isoformat()` for a point in time with a fixed offset.
pub fn python_isoformat<Z: TimeZone>(dt: &DateTime<Z>) -> String {
    let off = dt.offset().fix().local_minus_utc();
    let n = dt.naive_local();
    let mut s = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        n.year(),
        n.month(),
        n.day(),
        n.hour(),
        n.minute(),
        n.second()
    );
    let micros = n.nanosecond() / 1000;
    if micros != 0 {
        s.push_str(&format!(".{micros:06}"));
    }
    s.push_str(&fmt_offset(off));
    s
}

/// DRF `DateTimeField.to_representation`: convert to `tz` (= `PAPERLESS_TIME_ZONE`),
/// `isoformat()`, microseconds only if ≠ 0, `+00:00` → `Z`.
///
/// ```
/// use chrono::{TimeZone, Utc};
/// let dt = Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap();
/// assert_eq!(pl_core::datetime::format_datetime(&dt, &chrono_tz::UTC), "2024-01-02T03:04:05Z");
/// assert_eq!(pl_core::datetime::format_datetime(&dt, &chrono_tz::Europe::Berlin), "2024-01-02T04:04:05+01:00");
/// ```
pub fn format_datetime(dt: &DateTime<Utc>, tz: &Tz) -> String {
    let local = dt.with_timezone(tz);
    let s = python_isoformat(&local);
    match s.strip_suffix("+00:00") {
        Some(base) => format!("{base}Z"),
        None => s,
    }
}

/// `Option` variant: `None` → JSON `null` (DRF emits `None` for empty values).
pub fn format_datetime_opt(dt: Option<&DateTime<Utc>>, tz: &Tz) -> serde_json::Value {
    match dt {
        Some(d) => serde_json::Value::String(format_datetime(d, tz)),
        None => serde_json::Value::Null,
    }
}

/// DRF `DateField.to_representation`: `YYYY-MM-DD`.
pub fn format_date(d: &NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// Python `str(datetime)` (naive): `YYYY-MM-DD HH:MM:SS[.ffffff]`.
pub fn python_str_naive(n: &NaiveDateTime) -> String {
    let mut s = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        n.year(),
        n.month(),
        n.day(),
        n.hour(),
        n.minute(),
        n.second()
    );
    let micros = n.nanosecond() / 1000;
    if micros != 0 {
        s.push_str(&format!(".{micros:06}"));
    }
    s
}

// =============================================================================================
// Parsing: ISO (Python fromisoformat + Django parse_datetime)
// =============================================================================================

/// Result of a date/time parser: naive local time and, if any, a fixed offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedDateTime {
    pub naive: NaiveDateTime,
    pub offset: Option<FixedOffset>,
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn num(s: &str) -> Option<u32> {
    if digits(s) { s.parse().ok() } else { None }
}

/// Python `date.fromisoformat` (3.11+): `YYYY-MM-DD`, `YYYYMMDD`, `YYYY-Www[-D]`, `YYYYWww[D]`.
/// `Err(())` = ValueError.
pub fn python_date_fromisoformat(s: &str) -> Result<NaiveDate, ()> {
    let b = s.as_bytes();
    if !s.is_ascii() {
        return Err(());
    }
    let ymd = |y: &str, m: &str, d: &str| -> Result<NaiveDate, ()> {
        NaiveDate::from_ymd_opt(num(y).ok_or(())? as i32, num(m).ok_or(())?, num(d).ok_or(())?).ok_or(())
    };
    let week = |y: &str, w: &str, d: Option<&str>| -> Result<NaiveDate, ()> {
        let y = num(y).ok_or(())? as i32;
        let w = num(w).ok_or(())?;
        let d = match d {
            Some(d) => num(d).ok_or(())?,
            None => 1,
        };
        if !(1..=7).contains(&d) {
            return Err(());
        }
        let wd = chrono::Weekday::try_from((d - 1) as u8).map_err(|_| ())?;
        NaiveDate::from_isoywd_opt(y, w, wd).ok_or(())
    };
    match b.len() {
        10 if b[4] == b'-' && b[7] == b'-' => ymd(&s[0..4], &s[5..7], &s[8..10]),
        10 if b[4] == b'-' && b[5] == b'W' && b[8] == b'-' => week(&s[0..4], &s[6..8], Some(&s[9..10])),
        8 if b[4] == b'-' && b[5] == b'W' => week(&s[0..4], &s[6..8], None),
        8 if b[4] == b'W' => week(&s[0..4], &s[5..7], Some(&s[7..8])),
        8 => ymd(&s[0..4], &s[4..6], &s[6..8]),
        7 if b[4] == b'W' => week(&s[0..4], &s[5..7], None),
        _ => Err(()),
    }
}

/// Length of the date part for `datetime.fromisoformat` (simplified after CPython
/// `_find_isoformat_datetime_separator`).
fn iso_date_len(s: &[u8]) -> usize {
    if s.len() < 7 {
        return s.len();
    }
    if s.len() == 7 {
        return 7;
    }
    if s[4] == b'-' {
        if s[5] == b'W' {
            if s.len() > 8 && s[8] == b'-' { 10.min(s.len()) } else { 8 }
        } else {
            10.min(s.len())
        }
    } else if s[4] == b'W' {
        if s.len() > 7 && s[7].is_ascii_digit() { 8 } else { 7 }
    } else {
        8
    }
}

/// Time zone suffix `Z`, `±HH`, `±HHMM`, `±HH:MM`, `±HH:MM:SS[.f]`, `±HHMMSS`.
fn parse_tz_suffix(s: &str) -> Option<FixedOffset> {
    if s == "Z" {
        return FixedOffset::east_opt(0);
    }
    let sign = match s.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let body = &s[1..];
    let body = body.split(['.', ',']).next()?; // ignore fractional seconds of the offset
    let parts: Vec<&str> = if body.contains(':') {
        body.split(':').collect()
    } else {
        match body.len() {
            2 => vec![&body[0..2]],
            4 => vec![&body[0..2], &body[2..4]],
            6 => vec![&body[0..2], &body[2..4], &body[4..6]],
            _ => return None,
        }
    };
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.len() != 2) {
        return None;
    }
    let h = num(parts[0])?;
    let m = parts.get(1).map(|p| num(p)).unwrap_or(Some(0))?;
    let sec = parts.get(2).map(|p| num(p)).unwrap_or(Some(0))?;
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    FixedOffset::east_opt(sign * (h * 3600 + m * 60 + sec) as i32)
}

/// Python `time` from ISO: `HH`, `HH:MM`, `HH:MM:SS[.f+]`, `HHMM`, `HHMMSS[.f+]`.
fn parse_iso_time(s: &str) -> Option<NaiveTime> {
    let (main, frac) = match s.find(['.', ',']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let parts: Vec<&str> = if main.contains(':') {
        main.split(':').collect()
    } else {
        match main.len() {
            2 => vec![&main[0..2]],
            4 => vec![&main[0..2], &main[2..4]],
            6 => vec![&main[0..2], &main[2..4], &main[4..6]],
            _ => return None,
        }
    };
    if parts.iter().any(|p| p.len() != 2) || parts.is_empty() || parts.len() > 3 {
        return None;
    }
    if frac.is_some() && parts.len() != 3 {
        return None;
    }
    let h = num(parts[0])?;
    let m = parts.get(1).map(|p| num(p)).unwrap_or(Some(0))?;
    let sec = parts.get(2).map(|p| num(p)).unwrap_or(Some(0))?;
    let micros = match frac {
        Some(f) => {
            if !digits(f) {
                return None;
            }
            let mut f6: String = f.chars().take(6).collect();
            while f6.len() < 6 {
                f6.push('0');
            }
            f6.parse::<u32>().ok()?
        }
        None => 0,
    };
    NaiveTime::from_hms_micro_opt(h, m, sec, micros)
}

/// Python `datetime.fromisoformat` (3.11+). `None` = ValueError.
pub fn python_datetime_fromisoformat(s: &str) -> Option<ParsedDateTime> {
    if !s.is_ascii() || s.len() < 7 {
        return None;
    }
    let b = s.as_bytes();
    let dlen = iso_date_len(b);
    let date = python_date_fromisoformat(s.get(..dlen)?).ok()?;
    if dlen == s.len() {
        return Some(ParsedDateTime { naive: date.and_time(NaiveTime::MIN), offset: None });
    }
    // Any separator character, then time (+ optional time zone).
    let rest = s.get(dlen + 1..)?;
    if rest.is_empty() {
        return None;
    }
    let tz_pos = rest.find(['+', '-', 'Z']);
    let (tpart, tzpart) = match tz_pos {
        Some(i) => (&rest[..i], Some(&rest[i..])),
        None => (rest, None),
    };
    let time = parse_iso_time(tpart)?;
    let offset = match tzpart {
        Some(t) => Some(parse_tz_suffix(t)?),
        None => None,
    };
    Some(ParsedDateTime { naive: date.and_time(time), offset })
}

fn django_datetime_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"^(?P<year>[0-9]{4})-(?P<month>[0-9]{1,2})-(?P<day>[0-9]{1,2})",
            r"[T ](?P<hour>[0-9]{1,2}):(?P<minute>[0-9]{1,2})",
            r"(?::(?P<second>[0-9]{1,2})(?:[.,](?P<microsecond>[0-9]{1,6})[0-9]{0,6})?)?",
            r"\s*(?P<tzinfo>Z|[+-][0-9]{2}(?::?[0-9]{2})?)?$"
        ))
        .expect("static regex")
    })
}

/// Django `parse_datetime`: `Ok(Some)` parsed, `Ok(None)` no match,
/// `Err(())` = well-formed but invalid (ValueError).
pub fn django_parse_datetime(s: &str) -> Result<Option<ParsedDateTime>, ()> {
    if let Some(p) = python_datetime_fromisoformat(s) {
        return Ok(Some(p));
    }
    let Some(c) = django_datetime_re().captures(s) else { return Ok(None) };
    let g = |n: &str| c.name(n).map(|m| m.as_str());
    let p = |n: &str| g(n).map(|v| v.parse::<u32>().map_err(|_| ())).transpose();
    let micro = match g("microsecond") {
        Some(m) => {
            let mut m = m.to_string();
            while m.len() < 6 {
                m.push('0');
            }
            m.parse::<u32>().map_err(|_| ())?
        }
        None => 0,
    };
    let date = NaiveDate::from_ymd_opt(p("year")?.ok_or(())? as i32, p("month")?.ok_or(())?, p("day")?.ok_or(())?).ok_or(())?;
    let time = NaiveTime::from_hms_micro_opt(p("hour")?.ok_or(())?, p("minute")?.ok_or(())?, p("second")?.unwrap_or(0), micro)
        .ok_or(())?;
    let offset = match g("tzinfo") {
        None => None,
        Some("Z") => FixedOffset::east_opt(0),
        Some(t) => {
            let mins: i32 = if t.len() > 3 { t[t.len() - 2..].parse().map_err(|_| ())? } else { 0 };
            let hours: i32 = t[1..3].parse().map_err(|_| ())?;
            let mut off = 60 * hours + mins;
            if t.starts_with('-') {
                off = -off;
            }
            Some(FixedOffset::east_opt(off * 60).ok_or(())?)
        }
    };
    Ok(Some(ParsedDateTime { naive: date.and_time(time), offset }))
}

/// Django `parse_date`: `date.fromisoformat`, otherwise `YYYY-M-D`. `Err(())` = ValueError.
pub fn django_parse_date(s: &str) -> Result<Option<NaiveDate>, ()> {
    if let Ok(d) = python_date_fromisoformat(s) {
        return Ok(Some(d));
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^([0-9]{4})-([0-9]{1,2})-([0-9]{1,2})$").expect("static regex"));
    let Some(c) = re.captures(s) else { return Ok(None) };
    let y: i32 = c[1].parse().map_err(|_| ())?;
    let m: u32 = c[2].parse().map_err(|_| ())?;
    let d: u32 = c[3].parse().map_err(|_| ())?;
    NaiveDate::from_ymd_opt(y, m, d).map(Some).ok_or(())
}

// =============================================================================================
// Time zones
// =============================================================================================

/// Django `forms.utils.from_current_timezone` (USE_TZ=True): interpret naive time in `tz`.
/// Ambiguous/non-existent times → Django error message.
pub fn from_current_timezone(naive: &NaiveDateTime, tz: &Tz) -> Result<DateTime<Utc>, String> {
    match tz.from_local_datetime(naive) {
        chrono::LocalResult::Single(dt) => Ok(dt.with_timezone(&Utc)),
        _ => Err(format!(
            "{} couldn\u{2019}t be interpreted in time zone {}; it may be ambiguous or it may not exist.",
            python_str_naive(naive),
            tz.name()
        )),
    }
}

/// Point in time from [`ParsedDateTime`]: directly with offset, otherwise in `tz` (strict like
/// [`from_current_timezone`]).
pub fn resolve_parsed(p: &ParsedDateTime, tz: &Tz) -> Result<DateTime<Utc>, String> {
    match p.offset {
        Some(off) => off
            .from_local_datetime(&p.naive)
            .single()
            .map(|d| d.with_timezone(&Utc))
            .ok_or_else(|| "invalid offset".to_string()),
        None => from_current_timezone(&p.naive, tz),
    }
}

// =============================================================================================
// strptime (CPython _strptime) and locale input formats
// =============================================================================================

const MONTHS: [&str; 12] = [
    "january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november",
    "december",
];
const MONTHS_ABBR: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const DAYS: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
const DAYS_ABBR: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn seq_re(seq: &[&str], name: &str) -> String {
    let mut v: Vec<&str> = seq.to_vec();
    v.sort_by_key(|s| std::cmp::Reverse(s.len()));
    format!("(?P<{name}>{})", v.join("|"))
}

fn strptime_regex(fmt: &str) -> Option<Regex> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(c) = cache.lock()
        && let Some(r) = c.get(fmt)
    {
        return r.clone();
    }
    let built = build_strptime_regex(fmt);
    if let Ok(mut c) = cache.lock() {
        c.insert(fmt.to_string(), built.clone());
    }
    built
}

fn build_strptime_regex(fmt: &str) -> Option<Regex> {
    let mut pat = String::from("(?i)^");
    let mut seen = Vec::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let d = chars.next()?;
            if d != '%' {
                if seen.contains(&d) {
                    return None; // Python: "redefinition of group name" → ValueError
                }
                seen.push(d);
            }
            let p = match d {
                'd' => "(?P<d>3[01]|[12][0-9]|0[1-9]|[1-9]| [1-9])".to_string(),
                'f' => "(?P<f>[0-9]{1,6})".to_string(),
                'H' => "(?P<H>2[0-3]|[0-1][0-9]|[0-9])".to_string(),
                'm' => "(?P<m>1[0-2]|0[1-9]|[1-9])".to_string(),
                'M' => "(?P<M>[0-5][0-9]|[0-9])".to_string(),
                'S' => "(?P<S>6[0-1]|[0-5][0-9]|[0-9])".to_string(),
                'y' => "(?P<y>[0-9][0-9])".to_string(),
                'Y' => "(?P<Y>[0-9][0-9][0-9][0-9])".to_string(),
                'B' => seq_re(&MONTHS, "B"),
                'b' => seq_re(&MONTHS_ABBR, "b"),
                'A' => seq_re(&DAYS, "A"),
                'a' => seq_re(&DAYS_ABBR, "a"),
                '%' => "%".to_string(),
                _ => return None,
            };
            pat.push_str(&p);
        } else if c.is_whitespace() {
            while chars.peek().is_some_and(|n| n.is_whitespace()) {
                chars.next();
            }
            pat.push_str(r"\s+");
        } else {
            pat.push_str(&regex::escape(&c.to_string()));
        }
    }
    Regex::new(&pat).ok()
}

/// CPython `datetime.strptime(value, fmt)` for the directives used by Django
/// (`%d %m %y %Y %H %M %S %f %b %B %a %A %%`, English month names, case-insensitive).
/// `None` = ValueError.
pub fn strptime(value: &str, fmt: &str) -> Option<NaiveDateTime> {
    let re = strptime_regex(fmt)?;
    let c = re.captures(value)?;
    if c.get(0)?.end() != value.len() {
        return None; // "unconverted data remains"
    }
    let g = |n: &str| c.name(n).map(|m| m.as_str());
    let mut year: i32 = 1900;
    if let Some(y) = g("Y") {
        year = y.parse().ok()?;
    } else if let Some(y) = g("y") {
        let y: i32 = y.parse().ok()?;
        year = if y <= 68 { y + 2000 } else { y + 1900 };
    }
    let mut month = 1u32;
    if let Some(m) = g("m") {
        month = m.parse().ok()?;
    } else if let Some(b) = g("B") {
        month = MONTHS.iter().position(|x| x.eq_ignore_ascii_case(b))? as u32 + 1;
    } else if let Some(b) = g("b") {
        month = MONTHS_ABBR.iter().position(|x| x.eq_ignore_ascii_case(b))? as u32 + 1;
    }
    let day: u32 = g("d").map(|d| d.trim().parse().ok()).unwrap_or(Some(1))?;
    let hour: u32 = g("H").map(|v| v.parse().ok()).unwrap_or(Some(0))?;
    let minute: u32 = g("M").map(|v| v.parse().ok()).unwrap_or(Some(0))?;
    let second: u32 = g("S").map(|v| v.parse().ok()).unwrap_or(Some(0))?;
    let micro: u32 = match g("f") {
        Some(f) => format!("{f:0<6}").parse().ok()?,
        None => 0,
    };
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = NaiveTime::from_hms_micro_opt(hour, minute, second, micro)?;
    Some(date.and_time(time))
}

fn formats_for(lang: &str) -> (&'static [&'static str], &'static [&'static str]) {
    LOCALE_INPUT_FORMATS
        .iter()
        .find(|(l, _, _)| *l == lang)
        .or_else(|| LOCALE_INPUT_FORMATS.first())
        .map(|(_, d, dt)| (*d, *dt))
        .unwrap_or((&[], &[]))
}

/// `formats.get_format("DATE_INPUT_FORMATS")` for the active language.
pub fn date_input_formats(lang: &str) -> &'static [&'static str] {
    formats_for(lang).0
}

/// `formats.get_format("DATETIME_INPUT_FORMATS")` for the active language.
pub fn datetime_input_formats(lang: &str) -> &'static [&'static str] {
    formats_for(lang).1
}

/// Django form message `DateField.invalid`.
pub const MSG_INVALID_DATE: &str = "Enter a valid date.";
/// Django form message `DateTimeField.invalid`.
pub const MSG_INVALID_DATETIME: &str = "Enter a valid date/time.";

/// Django `forms.DateField.to_python` (filter `DateFilter`): `Ok(None)` for an empty value,
/// otherwise locale input formats of the active language (`lang`, see
/// [`crate::request::language_from_request`]).
pub fn parse_date_input(value: &str, lang: &str) -> Result<Option<NaiveDate>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    let v = value.trim();
    for f in date_input_formats(lang) {
        if let Some(dt) = strptime(v, f) {
            return Ok(Some(dt.date()));
        }
    }
    Err(MSG_INVALID_DATE.into())
}

/// django-filter `IsoDateTimeField.to_python` (filter `IsoDateTimeFilter`, default for
/// model `DateTimeField` in `rest_framework.FilterSet`): Django `parse_datetime`, naive values in
/// `tz` (strict; ambiguous → Django message).
pub fn parse_iso_datetime_input(value: &str, tz: &Tz) -> Result<Option<DateTime<Utc>>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    let v = value.trim();
    match django_parse_datetime(v) {
        Err(()) | Ok(None) => Err(MSG_INVALID_DATETIME.into()),
        Ok(Some(p)) => resolve_parsed(&p, tz).map(Some),
    }
}

/// Django `forms.DateTimeField.to_python` (filter `DateTimeFilter`): ISO, otherwise
/// `DATETIME_INPUT_FORMATS` + `DATE_INPUT_FORMATS` of the language.
pub fn parse_datetime_input(value: &str, lang: &str, tz: &Tz) -> Result<Option<DateTime<Utc>>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    let v = value.trim();
    match django_parse_datetime(v) {
        Err(()) => Err(MSG_INVALID_DATETIME.into()),
        Ok(Some(p)) => resolve_parsed(&p, tz).map(Some),
        Ok(None) => {
            let (d, dt) = formats_for(lang);
            for f in dt.iter().chain(d.iter()) {
                if let Some(n) = strptime(v, f) {
                    return from_current_timezone(&n, tz).map(Some);
                }
            }
            Err(MSG_INVALID_DATETIME.into())
        }
    }
}

/// DRF message for a wrong datetime format (serializer input, ISO_8601).
pub const DRF_DATETIME_INVALID: &str =
    "Datetime has wrong format. Use one of these formats instead: YYYY-MM-DDThh:mm[:ss[.uuuuuu]][+HH:MM|-HH:MM|Z].";
/// DRF message for a wrong date format (serializer input, ISO_8601).
pub const DRF_DATE_INVALID: &str = "Date has wrong format. Use one of these formats instead: YYYY-MM-DD.";

/// DRF `DateTimeField.to_internal_value` (input as string, `input_formats=[ISO_8601]`):
/// `parse_datetime` + `enforce_timezone` (naive → `tz`, ambiguous →
/// `Invalid datetime for the timezone "<tz>".`).
pub fn drf_parse_datetime(value: &str, tz: &Tz) -> Result<DateTime<Utc>, String> {
    match django_parse_datetime(value) {
        Ok(Some(p)) => match p.offset {
            Some(_) => resolve_parsed(&p, tz).map_err(|_| DRF_DATETIME_INVALID.to_string()),
            None => match tz.from_local_datetime(&p.naive) {
                chrono::LocalResult::Single(d) => Ok(d.with_timezone(&Utc)),
                _ => Err(format!("Invalid datetime for the timezone \"{}\".", tz.name())),
            },
        },
        _ => Err(DRF_DATETIME_INVALID.into()),
    }
}

/// DRF `DateField.to_internal_value` (string, ISO_8601) with Django `parse_date`.
pub fn drf_parse_date(value: &str) -> Result<NaiveDate, String> {
    match django_parse_date(value) {
        Ok(Some(d)) => Ok(d),
        _ => Err(DRF_DATE_INVALID.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Berlin;

    #[test]
    fn output_format() {
        let dt = Utc.with_ymd_and_hms(2024, 7, 1, 12, 0, 0).unwrap();
        assert_eq!(format_datetime(&dt, &chrono_tz::UTC), "2024-07-01T12:00:00Z");
        assert_eq!(format_datetime(&dt, &Berlin), "2024-07-01T14:00:00+02:00");
        let dt2 = dt + chrono::Duration::microseconds(1500);
        assert_eq!(format_datetime(&dt2, &chrono_tz::UTC), "2024-07-01T12:00:00.001500Z");
        let ny = chrono_tz::America::New_York;
        assert_eq!(format_datetime(&dt, &ny), "2024-07-01T08:00:00-04:00");
        assert_eq!(format_date(&NaiveDate::from_ymd_opt(987, 1, 2).unwrap()), "0987-01-02");
        // Local mean time before 1893 → offset with seconds like Python.
        let old = Utc.with_ymd_and_hms(1850, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(format_datetime(&old, &Berlin), "1850-01-01T00:53:28+00:53:28");
    }

    #[test]
    fn fromisoformat() {
        let p = python_datetime_fromisoformat("2024-01-02T03:04:05.123+01:00").unwrap();
        assert_eq!(p.naive.to_string(), "2024-01-02 03:04:05.123");
        assert_eq!(p.offset, FixedOffset::east_opt(3600));
        assert!(python_datetime_fromisoformat("2024-01-02").is_some());
        assert!(python_datetime_fromisoformat("20240102T0304").is_some());
        assert!(python_datetime_fromisoformat("2024-01-02 03:04Z").is_some());
        assert!(python_datetime_fromisoformat("2024-1-2 03:04").is_none());
        assert!(python_datetime_fromisoformat("2024-02-30").is_none());
        assert_eq!(python_date_fromisoformat("2024-W01-1"), Ok(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()));
    }

    #[test]
    fn django_regex_fallback() {
        let p = django_parse_datetime("2024-1-2 3:04:05,5 +0130").unwrap().unwrap();
        assert_eq!(p.naive.to_string(), "2024-01-02 03:04:05.500");
        assert_eq!(p.offset, FixedOffset::east_opt(5400));
        assert_eq!(django_parse_datetime("2024-13-02 03:04"), Err(()));
        assert_eq!(django_parse_datetime("garbage"), Ok(None));
    }

    #[test]
    fn strptime_like_python() {
        assert_eq!(strptime("10/25/2006", "%m/%d/%Y").unwrap().date(), NaiveDate::from_ymd_opt(2006, 10, 25).unwrap());
        assert_eq!(strptime("1.2.24", "%d.%m.%y").unwrap().date(), NaiveDate::from_ymd_opt(2024, 2, 1).unwrap());
        assert_eq!(strptime("1.2.69", "%d.%m.%y").unwrap().date(), NaiveDate::from_ymd_opt(1969, 2, 1).unwrap());
        assert_eq!(strptime("oct 25,   2006", "%b %d, %Y").unwrap().date(), NaiveDate::from_ymd_opt(2006, 10, 25).unwrap());
        assert!(strptime("2006-10-25x", "%Y-%m-%d").is_none());
        assert!(strptime("2023-02-29", "%Y-%m-%d").is_none());
        assert_eq!(strptime("2006-10-25 14:30:59.5", "%Y-%m-%d %H:%M:%S.%f").unwrap().nanosecond(), 500_000_000);
    }

    #[test]
    fn date_filter_input() {
        assert_eq!(parse_date_input("2024-03-01", "en-us"), Ok(NaiveDate::from_ymd_opt(2024, 3, 1)));
        assert_eq!(parse_date_input("03/01/2024", "en-us"), Ok(NaiveDate::from_ymd_opt(2024, 3, 1)));
        assert_eq!(parse_date_input("01.03.2024", "de-de"), Ok(NaiveDate::from_ymd_opt(2024, 3, 1)));
        assert_eq!(parse_date_input("01.03.2024", "en-us"), Err(MSG_INVALID_DATE.into()));
        assert_eq!(parse_date_input("", "en-us"), Ok(None));
        assert_eq!(parse_date_input("  2024-03-01 ", "en-us"), Ok(NaiveDate::from_ymd_opt(2024, 3, 1)));
    }

    #[test]
    fn iso_datetime_filter_input() {
        let r = parse_iso_datetime_input("2024-03-01T10:00:00", &Berlin).unwrap().unwrap();
        assert_eq!(r, Utc.with_ymd_and_hms(2024, 3, 1, 9, 0, 0).unwrap());
        let r = parse_iso_datetime_input("2024-03-01T10:00:00Z", &Berlin).unwrap().unwrap();
        assert_eq!(r, Utc.with_ymd_and_hms(2024, 3, 1, 10, 0, 0).unwrap());
        assert_eq!(
            parse_iso_datetime_input("2024-03-31T02:30:00", &Berlin),
            Err("2024-03-31 02:30:00 couldn\u{2019}t be interpreted in time zone Europe/Berlin; it may be ambiguous or it may not exist.".into())
        );
        assert_eq!(parse_iso_datetime_input("03/01/2024", &Berlin), Err(MSG_INVALID_DATETIME.into()));
        assert!(parse_datetime_input("03/01/2024 10:00", "en-us", &Berlin).unwrap().is_some());
    }

    #[test]
    fn drf_inputs() {
        assert_eq!(drf_parse_date("2024-1-5"), Ok(NaiveDate::from_ymd_opt(2024, 1, 5).unwrap()));
        assert_eq!(drf_parse_date("05.01.2024"), Err(DRF_DATE_INVALID.into()));
        assert!(drf_parse_datetime("2024-01-05T10:00", &Berlin).is_ok());
        assert_eq!(drf_parse_datetime("x", &Berlin), Err(DRF_DATETIME_INVALID.into()));
        assert_eq!(
            drf_parse_datetime("2024-10-27T02:30:00", &Berlin),
            Err("Invalid datetime for the timezone \"Europe/Berlin\".".into())
        );
    }
}
